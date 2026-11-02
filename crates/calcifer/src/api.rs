//! Calcifer's loopback API (`docs/CALCIFER-API.md`): HTTP/1.1, JSON and SSE for the CLI and a
//! browser. Every request passes the guard chain in order: a loopback peer, an allowed `Host`, no
//! foreign `Origin`, then (on the routes Calcifer serves) the bearer, from the `Authorization`
//! header or the `cozy_session` cookie `POST /v1/session` sets, and the admission gate a stopping
//! Calcifer closes. Refusals are `protocol::error::Envelope`s. No CORS header is ever sent.
use std::{collections::VecDeque, convert::Infallible, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        ConnectInfo, DefaultBodyLimit, FromRequestParts, Path, Query, Request, State,
        rejection::{BytesRejection, QueryRejection},
    },
    http::{HeaderValue, Method, StatusCode, Uri, header, request::Parts},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{self, KeepAlive, Sse},
    },
    routing::{get, post},
};
use protocol::{
    calcifer::{Down, Event, Run, RunList, RunsEvent, Status, SubmitRun, Submitted},
    error::{Envelope, Error, Kind},
    ids::RunId,
};
use futures_util::Stream;
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use tokio::sync::{RwLock, broadcast, watch};
use tracing::{info, warn};

use crate::{
    Calcifer,
    machines::{self, Machines},
    store::{RunFilter, RunRow, State as RunState, StoreError},
};

// The request fields this request carried that this Calcifer does not read (`parse`), answered as
// `Cozy-Ignored-Fields`: a client newer than Calcifer learns what it lacks.
tokio::task_local! {
    static IGNORED: Arc<std::sync::Mutex<Vec<String>>>;
}

/// The header a client reads to learn which of its request's fields this Calcifer did not read.
pub const IGNORED_FIELDS: &str = "cozy-ignored-fields";

/// A submission's bound: files ride as staged paths, never in the body.
const MAX_BODY: usize = 8 << 20;

/// What every handler shares.
pub struct Api {
    pub calcifer: Arc<Calcifer>,
    pub machines: Arc<Machines>,
    pub projects: crate::project::Projects,
    /// The three spellings of this server's own `host:port`.
    hosts: [String; 3],
    bearer: [u8; 32],
    /// Shut once Calcifer is stopping: a mutation holds it open while it runs, so the exit can
    /// never land between a submission's check and its commit.
    stopping: RwLock<bool>,
    stop: watch::Sender<bool>,
}

impl Api {
    pub fn new(
        calcifer: Arc<Calcifer>,
        machines: Arc<Machines>,
        port: u16,
        token: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            calcifer,
            machines,
            projects: Default::default(),
            hosts: [
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ],
            bearer: Sha256::digest(token).into(),
            stopping: RwLock::new(false),
            stop: watch::channel(false).0,
        })
    }

    /// `down`: admission closes once the mutations in flight are done, then every stream and the
    /// server end. Machines run on. What Calcifer still owes (`resume` takes it up) is answered.
    pub async fn down(&self) -> Vec<String> {
        let mut stopping = self.stopping.write().await;
        *stopping = true;
        let owed = self.calcifer.owed();
        drop(stopping);
        info!(owed = owed.len(), "down: stopping");
        self.stop();
        owed
    }

    /// Ends every stream and the server.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// True once Calcifer is stopping: what ends every stream.
    pub(crate) fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub fn stopped(&self) -> impl Future<Output = ()> + use<> {
        let mut stop = self.stop.subscribe();
        async move {
            let _ = stop.wait_for(|stop| *stop).await;
        }
    }

    /// The bearer, compared by digest so the time taken says nothing about the token.
    fn admits(&self, presented: &str) -> bool {
        let presented: [u8; 32] = Sha256::digest(presented).into();
        presented
            .iter()
            .zip(&self.bearer)
            .fold(0, |differ, (a, b)| differ | (a ^ b))
            == 0
    }
}

pub fn router(api: Arc<Api>) -> Router {
    Router::new()
        .route("/v1/runs", post(submit).get(list))
        .route("/v1/runs/events", get(runs_events))
        .route("/v1/runs/{id}", get(show))
        .route("/v1/runs/{id}/cancel", post(cancel))
        .route("/v1/runs/{id}/events", get(events))
        .route("/v1/runs/{id}/outputs/{name}", get(crate::outputs::serve))
        .route("/v1/runs/{id}/play", get(crate::play::play))
        .route("/v1/down", post(down))
        .route("/v1/machines", get(machines::list).post(machines::create))
        .route("/v1/machines/events", get(machines::events))
        .route(
            "/v1/machines/{name}",
            get(machines::show).delete(machines::end),
        )
        .route("/v1/machines/{name}/keepalive", post(machines::keepalive))
        .route("/v1/machines/{name}/logs/{log}", get(machines::logs))
        .route("/v1/rental-quotes", post(machines::quote))
        .route("/v1/run-quotes", post(machines::run_quote))
        .route("/v1/session", post(session).delete(end_session))
        .merge(crate::project::routes())
        // Routes go above this line: the bearer and the admission gate guard only those.
        .route_layer(middleware::from_fn_with_state(api.clone(), admit))
        .fallback(unknown_route)
        .method_not_allowed_fallback(unknown_route)
        .layer(middleware::from_fn_with_state(api.clone(), guard))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(api)
}

/// A refusal, rendered as its envelope with the status its kind names.
#[derive(Debug)]
pub struct Refusal(pub Error);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.kind.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
        let mut response = (status, Json(Envelope { error: self.0 })).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(r#"Bearer realm="cozy""#),
            );
        }
        response
    }
}

pub(crate) fn refuse(kind: Kind, code: &str, message: impl Into<String>) -> Refusal {
    Refusal(Error::new(kind, code, message))
}

impl Refusal {
    fn remedy(self, remedy: &str) -> Self {
        Refusal(self.0.remedy(remedy))
    }
}

impl From<StoreError> for Refusal {
    fn from(e: StoreError) -> Self {
        crate::Error::Store(e).into()
    }
}

impl From<crate::Error> for Refusal {
    fn from(e: crate::Error) -> Self {
        Refusal(match e {
            crate::Error::Store(StoreError::Conflict(key)) => Error::new(
                Kind::Conflict,
                "idempotency_conflict",
                format!("idempotency key {key:?} already names another run"),
            )
            .remedy("submit a different run under a new key"),
            crate::Error::Store(StoreError::UnknownMachine(name)) => Error::new(
                Kind::NotFound,
                "machine_not_found",
                format!("no machine named {name:?}"),
            )
            .next("cozy machine ls"),
            crate::Error::Store(e) => Error::new(Kind::Internal, "calcifer_store", e.to_string()),
            crate::Error::Invalid(message) => Error::new(Kind::Validation, "run_invalid", message),
            e @ crate::Error::Input { .. } => {
                Error::new(Kind::Validation, "input_unreadable", e.to_string())
            }
            crate::Error::Refused(e) => e,
        })
    }
}

impl From<QueryRejection> for Refusal {
    fn from(e: QueryRejection) -> Self {
        refuse(Kind::Usage, "query_invalid", e.body_text())
    }
}

impl From<BytesRejection> for Refusal {
    fn from(e: BytesRejection) -> Self {
        refuse(Kind::Validation, "body_unreadable", e.body_text())
    }
}

/// Loopback peer, `Host`, then `Origin`, on every request. A rebinding page is turned away before
/// anything asks whether it holds a credential.
async fn guard(
    State(api): State<Arc<Api>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let refused = refusal(&api, &peer, &request);
    let mut response = match refused {
        Some(refusal) => {
            warn!(
                code = refusal.0.code,
                "refused {} {}",
                request.method(),
                request.uri().path()
            );
            refusal.into_response()
        }
        None => {
            let ignored = Arc::new(std::sync::Mutex::new(vec![]));
            let mut response = IGNORED.scope(ignored.clone(), next.run(request)).await;
            let ignored = ignored.lock().expect("ignored fields").join(",");
            if let (false, Ok(value)) = (ignored.is_empty(), HeaderValue::from_str(&ignored)) {
                response.headers_mut().insert(IGNORED_FIELDS, value);
            }
            response
        }
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn refusal(api: &Api, peer: &SocketAddr, request: &Request) -> Option<Refusal> {
    let header = |name| {
        request
            .headers()
            .get(name)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
    };
    if !peer.ip().is_loopback() {
        Some(refuse(
            Kind::Forbidden,
            "not_loopback",
            "Calcifer serves the loopback interface only",
        ))
    } else if !header(header::HOST).is_some_and(|host| api.hosts.contains(&host)) {
        Some(
            refuse(
                Kind::Forbidden,
                "host_not_allowed",
                format!("Host {:?} is not this Calcifer", header(header::HOST)),
            )
            .remedy("reach it as 127.0.0.1, localhost or [::1] on its own port"),
        )
    // Checked on every request, mutations and stream opens included; an absent Origin passes.
    } else if let Some(origin) = header(header::ORIGIN)
        && !api
            .hosts
            .iter()
            .any(|host| origin == format!("http://{host}"))
    {
        Some(refuse(
            Kind::Forbidden,
            "origin_not_allowed",
            format!("Origin {origin:?} may not reach Calcifer"),
        ))
    } else {
        None
    }
}

/// The browser session's cookie: the bearer itself, for what cannot send a header (`<video>`,
/// `EventSource`). Scoped to the API, never readable by a page's script.
const SESSION: &str = "cozy_session";

/// The bearer, from `Authorization` or the session cookie, then the admission gate for every
/// mutation but `down`, held while it runs. The cookie rides along on any request a browser makes,
/// so a cookie-borne request must also prove it is Calcifer's own page: an unsafe method carries
/// Calcifer's own `Origin` (the guard already refused a foreign one), and a read is never
/// `Sec-Fetch-Site: cross-site`.
async fn admit(State(api): State<Arc<Api>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let cookie = (headers.get_all(header::COOKIE).iter())
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(SESSION)?.strip_prefix('='));
    let admitted = match (bearer, cookie) {
        (Some(token), _) => api.admits(token),
        (None, Some(token)) if api.admits(token) => {
            let safe = matches!(*request.method(), Method::GET | Method::HEAD);
            let origin = headers.get(header::ORIGIN).is_some();
            let cross = headers.get("sec-fetch-site").is_some_and(|site| site == "cross-site");
            if cross || (!safe && !origin) {
                return refuse(
                    Kind::Forbidden,
                    "session_cross_site",
                    "the session cookie serves Calcifer's own pages only",
                )
                .into_response();
            }
            true
        }
        _ => false,
    };
    if !admitted {
        return refuse(
            Kind::Credential,
            "unauthenticated",
            "Calcifer's API needs the bearer from its lock record",
        )
        .remedy("a browser page posts it once to /v1/session")
        .into_response();
    }
    if request.method() == Method::GET || request.uri().path() == "/v1/down" {
        return next.run(request).await;
    }
    let stopping = api.stopping.read().await;
    if *stopping {
        return refuse(
            Kind::Unavailable,
            "calcifer_stopping",
            "Calcifer is stopping and admits nothing new",
        )
        .remedy("start Calcifer again and retry; a submission's key makes the retry safe")
        .into_response();
    }
    next.run(request).await
}

/// The `Idempotency-Key` a durable ask is recorded under; required.
pub struct IdempotencyKey(pub String);

impl<S: Send + Sync> FromRequestParts<S> for IdempotencyKey {
    type Rejection = Refusal;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Refusal> {
        parts
            .headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| Self(key.to_owned()))
            .ok_or_else(|| {
                refuse(
                    Kind::Usage,
                    "idempotency_key_required",
                    "this ask needs an Idempotency-Key header",
                )
                .remedy("one key names one ask forever; retrying under it is safe")
            })
    }
}

/// A JSON body. Fields this Calcifer does not read are logged and ignored, never refused.
pub fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, Refusal> {
    let mut ignored = vec![];
    let value =
        serde_ignored::deserialize(&mut serde_json::Deserializer::from_slice(body), |path| {
            ignored.push(path.to_string())
        })
        .map_err(|e| refuse(Kind::Validation, "body_invalid", e.to_string()))?;
    if !ignored.is_empty() {
        warn!(
            ?ignored,
            "a request carried fields this Calcifer does not read"
        );
        let _ = IGNORED.try_with(|seen| seen.lock().expect("ignored fields").extend(ignored));
    }
    Ok(value)
}

/// A run by its id or its number.
pub(crate) fn find(api: &Api, key: &str) -> Result<RunRow, Refusal> {
    let store = api.calcifer.store();
    let row = match store.run(key)? {
        Some(row) => Some(row),
        None => match key.parse() {
            Ok(number) => store.run_by_number(number)?,
            Err(_) => None,
        },
    };
    row.ok_or_else(|| {
        Refusal(
            Error::new(Kind::NotFound, "run_not_found", format!("no run {key}"))
                .next("cozy run ls"),
        )
    })
}

async fn submit(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Submitted>), Refusal> {
    let submit: SubmitRun = parse(&body?)?;
    let submitted = api.calcifer.submit(&key, submit).await?;
    let status = if submitted.replay {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, Json(submitted)))
}

#[derive(Deserialize)]
struct Listing {
    status: Option<Status>,
    /// A prefix of the runs' target (`org/pkg`).
    package: Option<String>,
    /// A machine's name, former name or rental id, as a run's `--on` names it.
    machine: Option<String>,
    before: Option<u64>,
    limit: Option<u64>,
}

/// Runs newest first, and how many older ones match.
async fn list(
    State(api): State<Arc<Api>>,
    query: Result<Query<Listing>, QueryRejection>,
) -> Result<Json<RunList>, Refusal> {
    let Query(listing) = query?;
    let store = api.calcifer.store();
    let machine = match listing.machine {
        Some(text) => Some(match api.machines.here(&text).map_err(Refusal)? {
            Some(row) => row.name,
            // Its runs are listed while it is off.
            None if text == "local" => text,
            None => return Err(Refusal(machines::not_found(&text))),
        }),
        None => None,
    };
    let (rows, older, mut unreadable) = store.runs(&RunFilter {
        status: listing.status,
        target: listing.package,
        machine: machine.clone(),
        before: listing.before,
        limit: listing.limit.unwrap_or(50).clamp(1, 1000),
    })?;
    let mut runs = Vec::with_capacity(rows.len());
    for row in &rows {
        match api.calcifer.record(row) {
            Ok(run) => runs.push(run),
            Err(e) => unreadable.push(format!("run {}: {e}", row.number)),
        }
    }
    if let Some(first) = unreadable.first() {
        warn!("{} runs left out of a list, the first {first}", unreadable.len());
    }
    Ok(Json(RunList {
        runs,
        older,
        machine,
        unreadable,
    }))
}

#[derive(Deserialize)]
struct Detail {
    detail: Option<String>,
}

/// The run; `?detail=full` adds its measurements and settled calls.
async fn show(
    State(api): State<Arc<Api>>,
    Path(key): Path<String>,
    query: Result<Query<Detail>, QueryRejection>,
) -> Result<Json<Run>, Refusal> {
    let row = find(&api, &key)?;
    if query?.0.detail.as_deref() != Some("full") {
        return Ok(Json(api.calcifer.record(&row)?));
    }
    let mut run = api.calcifer.store().detail(&row)?;
    for live in api.calcifer.live(&row.id) {
        match live {
            Event::Waiting { reason } => run.waiting = Some(reason),
            Event::Progress { progress } => run.progress = Some(progress),
            _ => {}
        }
    }
    Ok(Json(run))
}

/// 202 with the run asked to stop; 200 when it had already ended.
async fn cancel(
    State(api): State<Arc<Api>>,
    Path(key): Path<String>,
) -> Result<(StatusCode, Json<Run>), Refusal> {
    let row = find(&api, &key)?;
    if row.state == RunState::Settled {
        return Ok((StatusCode::OK, Json(api.calcifer.record(&row)?)));
    }
    let run = api.calcifer.cancel(&row.id)?;
    let run =
        run.ok_or_else(|| refuse(Kind::NotFound, "run_not_found", format!("no run {key}")))?;
    Ok((StatusCode::ACCEPTED, Json(run)))
}

/// Stops Calcifer. It never refuses and never cancels: machines keep what they accepted, and the
/// next Calcifer resumes what is owed.
async fn down(State(api): State<Arc<Api>>) -> Json<Down> {
    let owed = api.down().await;
    Json(Down {
        stopping: true,
        owed,
    })
}

/// `POST /v1/session`: the bearer a page presented in `Authorization` (from its URL's fragment,
/// which no server or log sees), as the session cookie; `admit` already checked it. It lasts while
/// this Calcifer runs: the next one mints another bearer.
async fn session(headers: axum::http::HeaderMap) -> Result<Response, Refusal> {
    let bearer = (headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()))
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| refuse(Kind::Credential, "session_needs_bearer", "a session starts from the bearer itself"))?;
    let cookie = format!("{SESSION}={bearer}; HttpOnly; SameSite=Strict; Path=/v1");
    let value = HeaderValue::from_str(&cookie).map_err(|_| refuse(Kind::Usage, "bearer_invalid", "not a bearer"))?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, value)]).into_response())
}

/// `DELETE /v1/session`: the cookie gone from this browser.
async fn end_session() -> Response {
    let gone = format!("{SESSION}=; HttpOnly; SameSite=Strict; Path=/v1; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, HeaderValue::from_str(&gone).expect("a cookie"))]).into_response()
}

async fn unknown_route(method: Method, uri: Uri) -> Refusal {
    refuse(
        Kind::NotFound,
        "unknown_route",
        format!("Calcifer serves no {method} {}", uri.path()),
    )
}

#[derive(Deserialize)]
struct Resume {
    #[serde(default)]
    starting_after: u64,
}

/// The run's events from `starting_after`: what the store holds after it, the latest live
/// events, then the broadcast as it happens. The stream ends after the run's terminal event once
/// each of its outputs has landed or carries an `export_error`; Calcifer stopping ends it without
/// one (reconnect from the last `id:`).
async fn events(
    State(api): State<Arc<Api>>,
    Path(key): Path<String>,
    query: Result<Query<Resume>, QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, Refusal> {
    let Query(Resume { starting_after }) = query?;
    // Subscribed first: whatever happens after the store is read is in the tail.
    let tail = api.calcifer.subscribe();
    let row = find(&api, &key)?;
    let mut pending = VecDeque::from(
        api.calcifer
            .store()
            .events_after(row.id.as_str(), starting_after)?,
    );
    pending.extend(api.calcifer.live(&row.id));
    let follow = Follow {
        id: row.id,
        pending,
        last: starting_after,
        ended: row.state == RunState::Settled,
        tail,
        stop: api.stop.subscribe(),
        calcifer: api.calcifer.clone(),
    };
    let stream = futures_util::stream::unfold(follow, Follow::next);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Every run's changes as `run.updated`, each run whole: its lifecycle, outputs and live progress or
/// wait. No backlog: a client subscribes, then lists. Changes that arrive together are sent once
/// per run. A subscriber that falls behind is ended, never skipped past: it lists again.
async fn runs_events(
    State(api): State<Arc<Api>>,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
    let watch = RunsWatch {
        tail: api.calcifer.subscribe(),
        stop: api.stop_signal(),
        calcifer: api.calcifer.clone(),
        pending: VecDeque::new(),
    };
    Sse::new(futures_util::stream::unfold(watch, RunsWatch::next)).keep_alive(KeepAlive::default())
}

/// One `runs_events` subscriber: the runs it owes an update, in the order they changed.
struct RunsWatch {
    tail: broadcast::Receiver<(RunId, Event)>,
    stop: watch::Receiver<bool>,
    calcifer: Arc<Calcifer>,
    pending: VecDeque<RunId>,
}

impl RunsWatch {
    async fn next(mut self) -> Option<(Result<sse::Event, Infallible>, Self)> {
        loop {
            // Whatever else already arrived joins the batch: a run is sent once, as it is now.
            loop {
                match self.tail.try_recv() {
                    Ok((id, event)) => self.owe(id, &event),
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(_) => return None,
                }
            }
            if let Some(id) = self.pending.pop_front() {
                let run = match self.calcifer.store().run(id.as_str()) {
                    Ok(Some(row)) => self.calcifer.record(&row),
                    Ok(None) => continue,
                    Err(e) => Err(crate::Error::Store(e)),
                };
                let run = match run {
                    Ok(run) => run,
                    Err(e) => {
                        warn!(run = %id, "the runs stream cannot read the run: {e}");
                        return None;
                    }
                };
                let data = serde_json::to_string(&RunsEvent::Updated { run })
                    .expect("an event serializes");
                return Some((Ok(sse::Event::default().data(data)), self));
            }
            let received = tokio::select! {
                received = self.tail.recv() => received.ok(),
                _ = self.stop.wait_for(|stop| *stop) => None,
            };
            match received {
                Some((id, event)) => self.owe(id, &event),
                None => return None,
            }
        }
    }

    /// Owes `id` an update unless `event` is a log line or byte growth, or it is owed one already.
    fn owe(&mut self, id: RunId, event: &Event) {
        let shown = !matches!(
            event,
            Event::Log { .. }
                | Event::OutputDelta {
                    checkpoint: None,
                    ..
                }
        );
        if shown && !self.pending.contains(&id) {
            self.pending.push_back(id);
        }
    }
}

/// One SSE subscriber's place in its run's stream.
struct Follow {
    id: RunId,
    pending: VecDeque<Event>,
    /// The highest sequence number sent; a durable event at or below it is already known.
    last: u64,
    /// The run has ended (its terminal event was sent, or it had ended when the stream opened).
    ended: bool,
    tail: broadcast::Receiver<(RunId, Event)>,
    stop: watch::Receiver<bool>,
    calcifer: Arc<Calcifer>,
}

impl Follow {
    async fn next(mut self) -> Option<(Result<sse::Event, Infallible>, Self)> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                self.ended |= event.is_terminal();
                let mut sent = sse::Event::default();
                if let Some(sequence) = event.sequence_number() {
                    if sequence <= self.last {
                        continue;
                    }
                    self.last = sequence;
                    sent = sent.id(sequence.to_string());
                }
                let data = serde_json::to_string(&event).expect("an event serializes");
                return Some((Ok(sent.data(data)), self));
            }
            // An ended run's stream waits only for its outputs to land (or a store it cannot read).
            let owed = self.calcifer.store().outputs_to_export(self.id.as_str());
            if self.ended && !owed.is_ok_and(|outputs| !outputs.is_empty()) {
                return None;
            }
            tokio::select! {
                received = self.tail.recv() => match received {
                    Ok((id, event)) if id == self.id => self.pending.push_back(event),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        match self.calcifer.store().events_after(self.id.as_str(), self.last) {
                            Ok(events) => self.pending.extend(events),
                            Err(e) => {
                                warn!(run = %self.id, "the event stream cannot catch up: {e}");
                                return None;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
                _ = self.stop.wait_for(|stop| *stop) => return None,
            }
        }
    }
}
