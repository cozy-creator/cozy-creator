//! Calcifer's loopback API (`docs/CALCIFER-API.md`): HTTP/1.1, JSON and SSE for the CLI and a
//! browser. Every request passes the guard chain in order: a loopback peer, this listener's own `Host`
//! and an `Origin`, if any, that matches it; then, on `/v1`, Calcifer's own OS account and its own page
//! or the lock record's bearer (`loopback.rs`, no sessions); then the admission gate a stopping Calcifer
//! closes. Refusals are `api::error::Envelope`s. No CORS header is ever sent.
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use axum::{
    Json, Router,
    body::Body,
    extract::{
        ConnectInfo, DefaultBodyLimit, FromRequestParts, Path, Query, Request, State,
        rejection::{BytesRejection, QueryRejection},
    },
    http::{HeaderValue, Method, StatusCode, Uri, header, request::Parts},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{self, KeepAlive, KeepAliveStream, Sse},
    },
    routing::{get, post, put},
};
use api::{
    calcifer::{CalciferActivity, CalciferStatus, Down, Event, PROCESS_HEADER, Run, RunList, Status},
    error::{Envelope, Error, Kind, STOPPING},
    ids::RunId,
};
use futures_util::{StreamExt as _, stream::BoxStream};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use tokio::sync::{RwLock, broadcast, watch};
use tracing::{info, warn};

use crate::{
    loopback::Peer,
    Calcifer,
    machines::{self, Machines},
    store::{RunFilter, RunRow, State as RunState, StoreError},
};

// The request fields this request carried that this Calcifer does not read (`parse`), answered as
// `Cozy-Ignored-Fields`: a client newer than Calcifer learns what it lacks.
tokio::task_local! {
    static IGNORED: Arc<std::sync::Mutex<Vec<String>>>;
}

/// `work`, spawned by a request, hearing what it ignores as the request does (`Cozy-Ignored-Fields`).
pub(crate) fn carry_ignored<F: std::future::Future>(work: F) -> impl std::future::Future<Output = F::Output> {
    let held = IGNORED.try_with(Arc::clone).ok();
    async move {
        match held {
            Some(held) => IGNORED.scope(held, work).await,
            None => work.await,
        }
    }
}

/// The header a client reads to learn which of its request's fields this Calcifer did not read.
pub(crate) const IGNORED_FIELDS: &str = "cozy-ignored-fields";

/// A submission's bound: files ride as staged paths, never in the body.
const MAX_BODY: usize = 8 << 20;

/// What every handler shares.
pub(crate) struct Api {
    pub calcifer: Arc<Calcifer>,
    pub machines: Arc<Machines>,
    pub projects: crate::project::Projects,
    /// What this computer last read of each package's releases.
    pub(crate) releases: crate::runs::target::Releases,
    /// Each Hub's tools, as last read.
    pub(crate) listing: crate::toolbox::Listing,
    /// Emailed codes waiting for what arrived (`POST /v1/account/codes`).
    pub(crate) codes: crate::hub::Codes,
    pub(crate) browser_address: std::sync::OnceLock<std::net::SocketAddr>,
    pub(crate) development_web: std::sync::OnceLock<std::net::SocketAddr>,
    /// Who uses this Calcifer (`GET /v1/activity`).
    activity: Activity,
    bearer: [u8; 32],
    /// Shut once Calcifer is stopping: a mutation holds it open while it runs, so the exit can
    /// never land between a submission's check and its commit.
    stopping: Arc<RwLock<bool>>,
    stop: watch::Sender<bool>,
    /// Set once cozy updated itself (cl-978): this Calcifer stops to start the installed binary in its place.
    updated: std::sync::atomic::AtomicBool,
    /// When this Calcifer started: its own updates are keyed by it.
    pub(crate) started_ms: i64,
}

/// Nothing Calcifer answers is kept by a browser, and no /v1 answer (every project's data and files) is used by any page but its own: a
/// page on another port of this computer is the same site, and with no resource policy a picture its tab once read could be drawn there
/// without asking Calcifer (web-access, coordinator 11-17). Nothing of ours loads them from another origin: the hosted player streams from
/// the machine, and in dev mode Vite's code comes through Calcifer's own origin. The page's own files carry no data and no policy: an SVG
/// opened as its own sandboxed document is no origin's, and its browser's ask for /favicon.ico was refused out loud (web-svg, e2e
/// 44082601717).
async fn no_store(request: Request, next: Next) -> Response {
    let data = request.uri().path().starts_with("/v1/");
    let mut response = next.run(request).await;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    if data {
        response.headers_mut().insert(header::HeaderName::from_static("cross-origin-resource-policy"), HeaderValue::from_static("same-origin"));
    }
    response
}

impl Api {
    pub(crate) fn new(
        calcifer: Arc<Calcifer>,
        machines: Arc<Machines>,
        token: &str,
    ) -> Arc<Self> {
        let api = Arc::new(Self {
            browser_address: std::sync::OnceLock::new(),
            development_web: std::sync::OnceLock::new(),
            calcifer,
            machines,
            projects: Default::default(),
            releases: Default::default(),
            listing: Default::default(),
            codes: Default::default(),
            activity: Activity::default(),
            bearer: Sha256::digest(token).into(),
            stopping: Arc::new(RwLock::new(false)),
            stop: watch::channel(false).0,
            updated: std::sync::atomic::AtomicBool::new(false),
            started_ms: crate::store::now_ms(),
        });
        api.calcifer.serve_api(&api);
        crate::project::vault::start(&api);
        api
    }

    /// `down`: admission closes once the mutations in flight are done, then every stream and the
    /// server end. Machines run on. What Calcifer still owes (`resume` takes it up) is answered.
    pub(crate) async fn down(self: &Arc<Self>) -> Result<Vec<String>, Error> {
        self.down_inner(false).await
    }

    /// A service-manager stop keeps mutation admission closed while existing work/views drain.
    #[cfg(unix)]
    pub(crate) async fn shutdown(self: &Arc<Self>) -> Result<Vec<String>, Error> {
        self.down_inner(true).await
    }

    async fn down_inner(self: &Arc<Self>, keep_admission_closed: bool) -> Result<Vec<String>, Error> {
        // What a page typed in an encrypted project goes in before it locks, asked only while edits are admitted: a stop retried after a
        // busy close keeps admission shut, and its pages' writes and answers would be refused.
        if !*self.stopping.read().await { crate::project::vault::save_open().await; }
        let mut stopping = self.stopping.write().await;
        let was_stopping = *stopping;
        *stopping = true;
        if let Err(error) = tokio::task::block_in_place(|| crate::project::vault::close_all(self)) {
            *stopping = was_stopping || keep_admission_closed;
            return Err(error);
        }
        let owed = self.calcifer.owed();
        drop(stopping);
        info!(owed = owed.len(), "down: stopping");
        self.stop();
        Ok(owed)
    }

    /// What keeps this Calcifer from restarting now but `own`, its update's run (cl-978): runs going (its machines', its own),
    /// and requests being served. None: it's idle; a page with a stream open reconnects.
    pub(crate) fn busy_but(&self, own: &str) -> Result<Option<String>, Error> {
        let internal = |e: StoreError| Error::internal("store_unreadable", e.to_string());
        let runs: usize = self.calcifer.each_store(|store| store.unsettled().map(|ids| ids.len())).map_err(internal)?.into_iter().sum();
        let here: usize = self.calcifer.each_store(|store| Ok(store.unsettled_here()?.into_iter().filter(|id| id.as_str() != own).count())).map_err(internal)?.into_iter().sum();
        let requests = self.activity.requests.load(Ordering::Relaxed);
        let mut busy = vec![];
        match runs + here {
            0 => {}
            1 => busy.push("a run is going".to_owned()),
            n => busy.push(format!("{n} runs are going")),
        }
        if requests > 0 {
            busy.push(format!("{requests} request(s) in flight"));
        }
        Ok((!busy.is_empty()).then(|| busy.join(", ")))
    }

    /// Stops this Calcifer for the cozy it updated to, which starts in its place (`serve::main`): as `down` does, admission closed
    /// once mutations in flight are done, again while an encrypted view is still closing.
    pub(crate) async fn restart_updated(self: &Arc<Self>) {
        self.updated.store(true, Ordering::Relaxed);
        loop {
            let mut closed = std::pin::pin!(crate::project::vault::CLOSED.notified());
            closed.as_mut().enable();
            match self.down().await {
                Ok(_) => return,
                Err(e) => warn!(code = %e.code, "restarting on the updated cozy: {}; again once a view closes", e.message),
            }
            tokio::select! {
                () = self.stopped() => return,
                () = closed => {}
                () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            }
        }
    }

    /// Whether this Calcifer stopped for the cozy it updated to.
    pub(crate) fn updated(&self) -> bool {
        self.updated.load(Ordering::Relaxed)
    }

    /// Ends every stream and the server.
    pub(crate) fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// True once Calcifer is stopping: what ends every stream.
    pub(crate) fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub(crate) fn stopped(&self) -> impl Future<Output = ()> + use<> {
        let mut stop = self.stop.subscribe();
        async move {
            let _ = stop.wait_for(|stop| *stop).await;
        }
    }

    /// The bearer, compared by digest so the time taken says nothing about the token.
    pub(crate) fn admits(&self, presented: &str) -> bool {
        let presented: [u8; 32] = Sha256::digest(presented).into();
        presented
            .iter()
            .zip(&self.bearer)
            .fold(0, |differ, (a, b)| differ | (a ^ b))
            == 0
    }
}

pub(crate) fn router(api: Arc<Api>) -> Router {
    Router::new()
        .route("/v1/events", get(crate::events::stream))
        .route("/v1/events/{stream}", put(crate::events::show))
        .route("/v1/events/{stream}/saved", post(crate::events::saved))
        .route("/v1/maintenance/source", post(crate::maintenance_source::capture))
        .route("/v1/runs", post(crate::runs::submit).get(list))
        .route("/v1/runs/{id}", get(show))
        .route("/v1/runs/{id}/cancel", post(cancel))
        .route("/v1/runs/{id}/events", get(events))
        .route("/v1/runs/{id}/outputs/{name}", get(crate::outputs::serve))
        .route("/v1/runs/{id}/preview", get(crate::previews::serve))
        .route("/v1/runs/{id}/play", get(crate::play::play))
        .route("/v1/down", post(down))
        .route("/v1/status", get(status))
        .route("/v1/cut", get(cut))
        .route("/v1/settings", get(crate::reports::show).put(crate::reports::change))
        .route("/v1/appearance", get(appearance).put(set_appearance))
        .route("/v1/activity", get(activity))
        .route("/v1/machines", get(machines::list).post(machines::create))
        .route(
            "/v1/machines/{name}",
            get(machines::show).delete(machines::end),
        )
        .route("/v1/machines/{name}/keepalive", post(machines::keepalive))
        .route("/v1/machines/{name}/logs/{log}", get(machines::logs))
        .route("/v1/machines/{name}/gpus", get(machines::gpus))
        .route("/v1/machines/{name}/packages", get(machines::packages))
        .route("/v1/machines/{name}/models", get(machines::models))
        .route("/v1/machines/{name}/models/remove", post(machines::remove_model))
        .route("/v1/machines/{name}/update", post(crate::update::update))
        .route(
            "/v1/machines/{name}/authorized-keys/{key}",
            put(crate::authorized::add).delete(crate::authorized::remove),
        )
        .route("/v1/rental-skus", get(machines::rental_skus))
        .route("/v1/rentals", get(machines::rentals))
        .route("/v1/rental-quotes", post(machines::quote))
        .route("/v1/run-quotes", post(crate::runs::quote))
        .route("/v1/run-estimates", post(crate::runs::estimate))
        .route("/v1/targets", get(crate::runs::describe))
        .route("/v1/project-targets", get(crate::runs::describe))
        .route("/v1/tools", get(crate::toolbox::list))
        .route("/v1/tools/list", post(crate::toolbox::change))
        .route("/v1/tools/browse", get(crate::toolbox::browse))
        .route("/v1/project-files/held", post(crate::runs::files::project_held_files))
        .route("/v1/files/held", post(crate::runs::files::held_files))
        .route(
            "/v1/project-files/{name}",
            axum::routing::put(crate::runs::files::project_put)
                .head(crate::runs::files::project_head)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/v1/files/{name}",
            axum::routing::put(crate::runs::files::put)
                .head(crate::runs::files::head)
                .layer(DefaultBodyLimit::disable()),
        )
        .merge(crate::project::routes(&api))
        .merge(crate::hub::routes())
        // Routes go above this line: the bearer and the admission gate guard only those.
        .route_layer(middleware::from_fn_with_state(api.clone(), admit))
        .merge(crate::page::routes())
        .fallback(crate::page::fallback)
        .method_not_allowed_fallback(unknown_route)
        .layer(middleware::from_fn(guard))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn(no_store))
        .layer(middleware::from_fn(crate::took::request))
        .with_state(api)
}

/// A refusal, rendered as its envelope with its status (a peer's 429 stays 429), and the wait it states as `Retry-After`.
#[derive(Debug)]
pub(crate) struct Refusal(pub Error);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
        let wait = self.0.retry_after_s;
        let mut response = (status, Json(Envelope { error: self.0 })).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(r#"Bearer realm="cozy""#),
            );
        }
        if let Some(wait) = wait {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(wait));
        }
        response
    }
}

pub(crate) fn refuse(kind: Kind, code: &str, message: impl Into<String>) -> Refusal {
    Refusal(Error::new(kind, code, message))
}

impl Refusal {
    pub(crate) fn remedy(self, remedy: &str) -> Self {
        Refusal(self.0.remedy(remedy))
    }
}

impl From<StoreError> for Refusal {
    fn from(e: StoreError) -> Self {
        crate::Error::Store(e).into()
    }
}

/// A key that already names another ask.
pub(crate) fn conflict(key: &str) -> Error {
    Error::new(Kind::Conflict, "idempotency_conflict", format!("The idempotency key {key:?} was already used for a different run."))
        .remedy("Send a different run with a new key.")
}

/// A project journal that can't be read now, by why (`StoreError::Locked`): "unlock it" only when the project is locked.
fn locked(code: &str) -> Error {
    let (code, said) = match code {
        "project_busy" => ("project_busy", "The project's storage is busy for a moment. Try again."),
        "project_view_changed" => ("project_view_changed", "The project was locked and unlocked again while this was asked. Try again."),
        "project_inspection" => ("project_busy", "Cozy is checking the project's storage for a moment. Try again."),
        "project_acting" => ("project_acting", "The project is busy with another change to it. Try again when that finishes."),
        "project_transition" => ("project_transition", "The project's storage is changing (it's locking, unlocking or converting). Try again when that finishes."),
        // Reached by a run alone (`/v1/runs/{id}`), a locked encrypted project and a plaintext one not open look alike here: nothing
        // here names a locked project's runs (cl-984).
        "project_not_open" | "run_owner_required" => ("project_not_open", "The run's project isn't open in Cozy right now. Unlock it if it's encrypted, or open it, then try again."),
        _ => ("project_locked", "The project is locked. Unlock it to see its run history."),
    };
    Error::new(Kind::Conflict, code, said)
}

impl From<crate::Error> for Refusal {
    fn from(e: crate::Error) -> Self {
        Refusal(match e {
            crate::Error::Store(StoreError::Conflict(key)) => conflict(&key),
            // Only a locked project's journal says "unlock it"; each other refusal says what it is (cl-1064).
            crate::Error::Store(StoreError::Locked(code)) => locked(&code),
            crate::Error::Store(StoreError::UnknownMachine(name)) => Error::new(
                Kind::NotFound,
                "machine_not_found",
                format!("There's no machine named {name:?}."),
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

/// Loopback peer, `Host`, then `Origin`, on every request (`loopback::refusal`).
async fn guard(
    ConnectInfo(peer): ConnectInfo<Peer>,
    request: Request,
    next: Next,
) -> Response {
    let began = std::time::Instant::now();
    let refused = crate::loopback::refusal(&peer, request.headers());
    let checked = began.elapsed();
    let timing = crate::loopback::TIMING.load(std::sync::atomic::Ordering::Relaxed).then(crate::loopback::Timing::default);
    let mut request = request;
    if let Some(timing) = &timing {
        request.extensions_mut().insert(timing.clone());
    }
    let api_route = request.uri().path().starts_with("/v1/");
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
    // Nothing under /v1 is a page: whatever it holds (a package's preview, a run's output, a file of a project) runs in no origin of
    // Calcifer's, even opened as a URL; an SVG, which can carry script, runs none at all.
    if api_route {
        let svg = headers.get(header::CONTENT_TYPE).and_then(|kind| kind.to_str().ok()).is_some_and(|kind| kind.starts_with("image/svg"));
        let mut policy = headers.get(header::CONTENT_SECURITY_POLICY).and_then(|policy| policy.to_str().ok()).unwrap_or_default().to_owned();
        for directive in ["sandbox"].into_iter().chain(svg.then_some("script-src 'none'")) {
            if !policy.split(';').any(|held| held.trim() == directive) {
                policy = if policy.is_empty() { directive.to_owned() } else { format!("{policy}; {directive}") };
            }
        }
        if let Ok(policy) = HeaderValue::from_str(&policy) {
            headers.insert(header::CONTENT_SECURITY_POLICY, policy);
        }
    }
    headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(said) = timing.and_then(|timing| timing.header(checked, &peer)) {
        headers.insert(header::HeaderName::from_static("server-timing"), said);
    }
    response
}

/// The secret of `Authorization: Bearer`.
pub(crate) fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

/// Calcifer's own page or this computer's cozy, as its own account; then the admission gate for every mutation but `down`, held
/// while it runs. Detached request work retains this existing admission lease until its mutation is finished.
pub(crate) type MutationLease = Arc<tokio::sync::OwnedRwLockReadGuard<bool>>;

async fn admit(State(api): State<Arc<Api>>, ConnectInfo(peer): ConnectInfo<Peer>, mut request: Request, next: Next) -> Response {
    // Calcifer's own account, then its own page or this computer's cozy (`loopback::admitted`).
    let began = std::time::Instant::now();
    let (method, path, headers) = (request.method(), request.uri().path(), request.headers());
    let admitted = crate::loopback::admitted(&api, &peer, method, path, headers).await;
    if let Some(timing) = request.extensions().get::<crate::loopback::Timing>() {
        timing.admitted(began.elapsed());
    }
    let client = match admitted {
        Ok(admitted) => admitted,
        Err(refusal) => {
            warn!(code = refusal.0.code, "refused {} {}", request.method(), request.uri().path());
            return refusal.into_response();
        }
    };
    let page = client == crate::loopback::Client::Page;
    request.extensions_mut().insert(client);
    let process = request.headers().get(PROCESS_HEADER).and_then(|value| value.to_str().ok()).unwrap_or_default();
    api.activity.note(if page { "page" } else { process });
    // Served until its answer is made (a stream's body is the open page's, not this); `/v1/activity` counts no one.
    let _serving = (request.uri().path() != "/v1/activity").then(|| Counted::of(&api.activity.requests));
    // A read is answered while Calcifer stops: a HEAD's refusal would lose its body.
    if matches!(*request.method(), Method::GET | Method::HEAD) || request.uri().path() == "/v1/down" {
        let response = next.run(request).await;
        let stream = response.headers().get(header::CONTENT_TYPE).is_some_and(|kind| kind.as_bytes().starts_with(b"text/event-stream"));
        return match page && stream {
            true => api.activity.held_open(response),
            false => response,
        };
    }
    let stopping = api.stopping.clone().read_owned().await;
    if *stopping {
        return refuse(Kind::Unavailable, STOPPING, "Cozy is stopping, so it didn't do that.")
            .remedy("Try again once it's back.")
            .into_response();
    }
    let lease = Arc::new(stopping);
    request.extensions_mut().insert(lease.clone());
    let response = next.run(request).await;
    drop(lease);
    response
}

/// The `Idempotency-Key` a durable ask is recorded under; required.
pub(crate) struct IdempotencyKey(pub String);

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
                    "This request needs an Idempotency-Key header.",
                )
                .remedy("One key always names the same request, so retrying with it is safe.")
            })
    }
}

/// A JSON body. Fields this Calcifer does not read are logged and ignored, never refused.
pub(crate) fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, Refusal> {
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
#[derive(Deserialize)]
pub(crate) struct RunPath {
    id: String,
    #[serde(default)]
    run: Option<String>,
}
impl RunPath {
    pub(crate) fn project(&self) -> Option<&str> { self.run.as_ref().map(|_| self.id.as_str()) }
    pub(crate) fn key(&self) -> &str { self.run.as_deref().unwrap_or(&self.id) }
}

/// One of Calcifer's jobs, by its id (cl-1028: a job has no name or number of a person's).
pub(crate) fn find_maintenance(api: &Api, key: &str) -> Result<RunRow, Refusal> {
    let row=api.calcifer.maintenance_store().run(key)?;
    let row=row.ok_or_else(||refuse(Kind::NotFound,"run_not_found","There's no such job going or kept."))?;
    api.calcifer.remember_maintenance_run(row.id.as_str());
    Ok(row)
}

pub(crate) fn find_current(api:&Api,key:&str)->Result<RunRow,Refusal> {
    let Some(scope)=crate::private_io::current() else { return find_maintenance(api,key); };
    let store=api.calcifer.project_store(&scope.storage.project_id)?;
    let row=store.run(key)?.or(match key.parse() { Ok(number)=>store.run_by_number(number)?,Err(_)=>None });
    row.ok_or_else(||refuse(Kind::NotFound,"run_not_found","This project has no such run."))
}

/// The run `KEY-N` names from any scope (cl-987): run N of the project that has or had the key. None when `text` is no such name,
/// or no project here has the key.
pub(crate) async fn named_run(api: &Arc<Api>, text: &str) -> Result<Option<RunRow>, Refusal> {
    let Some((key, number)) = ::api::calcifer::project::run_name(text) else { return Ok(None) };
    let journal = match key.as_str() {
        reserved if ::api::calcifer::project::RESERVED_KEYS.contains(&reserved) => return Ok(None),
        _ => match keyed_project(api, &key).await? {
            Some(id) => crate::store::RunStore::Project(api.projects.store(api, &id).await.map_err(Refusal)?),
            None => return Ok(None),
        },
    };
    let row = journal.run_by_number(number)?.ok_or_else(|| Refusal(Error::new(Kind::NotFound, "run_not_found", format!("There's no run {key}-{number}.")).next("cozy run ls --all")))?;
    Ok(Some(row))
}

/// The project that has or had `key` here. A project's keys are registered as it opens: one not opened since it was unlocked (or
/// since this cozy) opens first. A locked project's are in no index, only in its project.db (Paul 11-15): its key is unknown here.
pub(crate) async fn keyed_project(api: &Arc<Api>, key: &str) -> Result<Option<String>, Refusal> {
    let store = api.calcifer.store();
    if let Some(id) = store.key_owner(key).map_err(store_refusal)? { return Ok(Some(id)); }
    let known = store.known_projects().map_err(store_refusal)?;
    for entry in known.iter().filter(|entry| entry.key.is_empty() && !crate::project::vault::locked_now(&entry.path)) {
        let _ = api.projects.store(api, &entry.id).await;
    }
    store.key_owner(key).map_err(store_refusal)
}

pub(crate) async fn find_scoped(api: &Arc<Api>, project: Option<&str>, key: &str) -> Result<RunRow, Refusal> {
    if let Some(row) = named_run(api, key).await? { return Ok(row); }
    let journal = match project {
        Some(project) => crate::store::RunStore::Project(api.projects.store(api, project).await.map_err(Refusal)?),
        None => crate::store::RunStore::Maintenance(api.calcifer.maintenance_store()),
    };
    // A number is a project's run's; a job of Calcifer's has only its id (cl-1028).
    let row = match journal.run(key)? {
        Some(row) => Some(row),
        None if project.is_some() => match key.parse() { Ok(number) => journal.run_by_number(number)?, Err(_) => None },
        None => None,
    };
    // cozy's own update (cl-978) is no project's: a project's client that asked for it follows it from Calcifer's journal.
    let row = match row {
        None if project.is_some() => api.calcifer.maintenance_store().run(key)?.filter(|row| row.target == ::api::calcifer::hub::job::COZY_UPDATE),
        row => row,
    }.ok_or_else(|| match ::api::calcifer::project::run_name(key) {
        Some((named, _)) => Refusal(Error::new(Kind::NotFound, "run_not_found", format!("No project on this computer has the key {named}.")).next("cozy run ls --all")),
        // A bare number outside any project's folder: run numbers are a project's (cl-1028).
        None if project.is_none() && key.parse::<u64>().is_ok() => Refusal(Error::new(Kind::NotFound, "run_not_found",
            format!("Run {key} is a project's number: run this in its project's folder, or name it KEY-{key}.")).next("cozy run ls --all")),
        None => refuse(Kind::NotFound, "run_not_found", "There's no such run."),
    })?;
    if project.is_none() || row.target == ::api::calcifer::hub::job::COZY_UPDATE { api.calcifer.remember_maintenance_run(row.id.as_str()); }
    Ok(row)
}

#[derive(Deserialize)]
pub(crate) struct Listing {
    status: Option<Status>,
    /// A prefix of the runs' target (`org/pkg`).
    package: Option<String>,
    /// A machine's name, former name or rental id, as a run's `--on` names it.
    machine: Option<String>,
    /// A project's id or name, or its folder on this computer: that project's runs.
    project: Option<String>,
    before: Option<u64>,
    limit: Option<u64>,
}

/// Runs newest first, and how many older ones match.
async fn list(State(_): State<Arc<Api>>, query: Result<Query<Listing>, QueryRejection>) -> Result<Json<RunList>, Refusal> {
    let Query(listing) = query?;
    if listing.project.is_some() { return Err(refuse(Kind::Usage, "project_context_required", "Ask the project for its runs instead.")); }
    // Runs are kept in their projects (cl-1028); Calcifer's jobs are its machines' (`GET /v1/machines`).
    Err(Refusal(Error::new(Kind::Usage, "not_in_a_project", "Runs are kept in their projects. Ask a project for its runs.").next("cozy run ls --all")))
}

pub(crate) async fn project_list(State(api): State<Arc<Api>>, Path(project): Path<String>, query: Result<Query<Listing>, QueryRejection>) -> Result<Json<RunList>, Refusal> {
    list_scoped(&api, &project, query?.0).await
}

async fn list_scoped(api: &Arc<Api>, project: &str, listing: Listing) -> Result<Json<RunList>, Refusal> {
    let journal = crate::store::RunStore::Project(api.projects.store(api, project).await.map_err(Refusal)?);
    let named = match &listing.machine {
        Some(text) => match api.machines.named(text).map_err(Refusal)? {
            Some(row) => Some(row),
            // Its runs are listed while it is off.
            None if text.eq_ignore_ascii_case("local") => None,
            None => return Err(Refusal(machines::not_found(text))),
        },
        None => None,
    };
    let machine = (named.as_ref().map(|row| row.name.clone())).or_else(|| listing.machine.as_ref().map(|_| "local".to_owned()));
    let (rows, older, mut unreadable) = journal.runs(&RunFilter {
        status: listing.status,
        target: listing.package.clone(),
        machine: machine.clone(),
        out_dir: None,
        before: listing.before,
        limit: listing.limit.unwrap_or(50).clamp(1, 1000),
    })?;
    let mut runs = Vec::with_capacity(rows.len());
    for row in &rows {
        match api.calcifer.record(row) { Ok(run) => runs.push(run), Err(_) => unreadable.push("run_unavailable".into()) }
    }
    Ok(Json(RunList { runs, older, machine, unreadable, others: vec![] }))
}

#[derive(Deserialize)]
pub(crate) struct Detail {
    detail: Option<String>,
}

/// The run; `?detail=full` adds its measurements and settled calls.
pub(crate) async fn show(
    State(api): State<Arc<Api>>,
    Path(path): Path<RunPath>,
    query: Result<Query<Detail>, QueryRejection>,
) -> Result<Json<Run>, Refusal> {
    let key = path.key();
    let row = find_scoped(&api, path.project(), key).await?;
    if query?.0.detail.as_deref() != Some("full") {
        return Ok(Json(api.calcifer.record(&row)?));
    }
    let mut run = api.calcifer.row_store(&row)?.detail(&row)?;
    let record = api.calcifer.record(&row)?;
    (run.waiting, run.progress, run.queue_position) = (record.waiting, record.progress, record.queue_position);
    (run.name, run.project, run.project_id, run.stalled) = (record.name, record.project, record.project_id, record.stalled);
    run.request = crate::runs::asked(&row);
    run.video_progress = crate::runs::video_progress::of(&run, &api.calcifer.row_store(&row)?.events_after(row.id.as_str(), 0)?);
    run.report = crate::runs::report::of(&run);
    Ok(Json(run))
}

/// 202 with the run asked to stop; 200 when it had already ended.
pub(crate) async fn cancel(
    State(api): State<Arc<Api>>,
    Path(path): Path<RunPath>,
) -> Result<(StatusCode, Json<Run>), Refusal> {
    let key = path.key();
    let row = find_scoped(&api, path.project(), key).await?;
    if row.state == RunState::Settled {
        return Ok((StatusCode::OK, Json(api.calcifer.record(&row)?)));
    }
    if api.calcifer.row_owner(&row)?.as_ref().is_some_and(|owner|owner.project_id()!=row.owner) { return Err(refuse(Kind::Conflict,"copied_run_inert","This run was copied from another project's history, so canceling it here can't stop the original.")); }
    let run = api.calcifer.cancel(&row.id)?;
    let run =
        run.ok_or_else(|| refuse(Kind::NotFound, "run_not_found", format!("There's no run {key}.")))?;
    Ok((StatusCode::ACCEPTED, Json(run)))
}

/// `GET /v1/appearance` (cl-1097): how the page looks everywhere, for every page of this Calcifer; null until it's set.
async fn appearance(State(api): State<Arc<Api>>) -> Result<Json<Option<api::calcifer::Appearance>>, Refusal> {
    api.calcifer.store().appearance().map(Json).map_err(|e| Refusal(Error::internal("calcifer_store", e.to_string())))
}

/// `PUT /v1/appearance` (cl-1097): every page hears it; a project with its own keeps that.
async fn set_appearance(State(api): State<Arc<Api>>, body: Result<axum::body::Bytes, BytesRejection>) -> Result<Json<api::calcifer::Appearance>, Refusal> {
    let asked: api::calcifer::Appearance = parse(&body?)?;
    asked.check().map_err(|why| Refusal(Error::new(Kind::Usage, "appearance_invalid", why)))?;
    api.calcifer.store().set_appearance(&asked).map_err(|e| Refusal(Error::internal("calcifer_store", e.to_string())))?;
    crate::events::appearance(&asked);
    Ok(Json(asked))
}

/// Stops Calcifer after its encrypted mounts close. Busy views refuse without losing custody.
/// Machines keep what they accepted, and the next Calcifer resumes what is owed.
async fn down(State(api): State<Arc<Api>>) -> Result<Json<Down>, Refusal> {
    let owed = api.down().await.map_err(Refusal)?;
    Ok(Json(Down {
        stopping: true,
        owed,
    }))
}

async fn status(State(api): State<Arc<Api>>) -> Result<Json<CalciferStatus>, Refusal> {
    let hub = crate::hub::origin(&api, None)?;
    Ok(Json(CalciferStatus {
        version: ::api::VERSION.into(),
        os: std::env::consts::OS.into(),
        local_usable: platform::Os::CURRENT.local_inference(),
        signed_in: hub.device_key(api.machines.hubs.home()).exists(),
        heap_bytes: platform::heap::held(),
        home: api.machines.user_home.clone(),
        os_user: crate::loopback::own().map(ToString::to_string).unwrap_or_default(),
    }))
}

/// `GET /v1/cut`: what the legacy cut (cl-1003) still keeps for projects here, for `cozy doctor`: files gone before it could copy them
/// in, and the files it holds for projects not opened since.
async fn cut(State(api): State<Arc<Api>>) -> Result<Json<::api::calcifer::CutFiles>, Refusal> {
    let calcifer = api.machines.hubs.home().calcifer();
    let (lost, (held_projects, held_bytes)) = tokio::task::block_in_place(|| {
        Ok::<_, crate::store::StoreError>((api.calcifer.store().lost_before_upgrade()?, crate::project::held(&calcifer)))
    })?;
    let lost = lost.into_iter().map(|(path, files)| ::api::calcifer::LostFiles { path, files }).collect();
    let (old_outputs_waiting, old_outputs) = crate::old_outputs::waiting(&calcifer);
    Ok(Json(::api::calcifer::CutFiles { lost, held_projects, held_bytes, kept: calcifer.join("kept"), old_outputs_waiting, old_outputs }))
}

/// Who uses this Calcifer now (cl-962): a `cozy` of another version restarts it only when no page, run or other client does.
async fn activity(State(api): State<Arc<Api>>, headers: axum::http::HeaderMap) -> Result<Json<CalciferActivity>, Refusal> {
    // Calcifer's own runs and each open project's, from their journals.
    let counts = api.calcifer.each_store(|store| Ok((store.unsettled()?.len(), store.unsettled_here()?.len()))).map_err(store_refusal)?;
    let here: usize = counts.iter().map(|(_, here)| here).sum();
    let runs = counts.iter().map(|(runs, _)| runs).sum::<usize>() + here;
    let asker = headers.get(PROCESS_HEADER).and_then(|value| value.to_str().ok()).unwrap_or_default();
    Ok(Json(CalciferActivity {
        version: ::api::VERSION.into(),
        pages: api.activity.pages.load(Ordering::Relaxed),
        runs: u32::try_from(runs).unwrap_or(u32::MAX),
        here: u32::try_from(here).unwrap_or(u32::MAX),
        requests: api.activity.requests.load(Ordering::Relaxed),
        others_request_ms: api.activity.others(asker),
        now_ms: crate::store::now_ms(),
    }))
}

fn store_refusal(e: StoreError) -> Refusal {
    refuse(Kind::Internal, "store_unreadable", e.to_string())
}

/// Each client's newest request, by the process it names (`PROCESS_HEADER`; a page is `page`, a client that names none ``), the
/// requests being served, and the pages' open streams.
#[derive(Default)]
struct Activity {
    pages: Arc<AtomicU32>,
    requests: Arc<AtomicU32>,
    seen: Mutex<HashMap<String, i64>>,
}

/// One of `Activity`'s counts, held while this lives.
struct Counted(Arc<AtomicU32>);

impl Counted {
    fn of(count: &Arc<AtomicU32>) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self(count.clone())
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Activity {
    /// Clients kept: the newest this many.
    const KEPT: usize = 64;

    fn note(&self, client: &str) {
        let mut seen = self.seen.lock().expect("activity");
        seen.insert(client.to_owned(), crate::store::now_ms());
        if seen.len() > Self::KEPT
            && let Some(oldest) = seen.iter().min_by_key(|(_, at)| **at).map(|(client, _)| client.clone())
        {
            seen.remove(&oldest);
        }
    }

    fn others(&self, asker: &str) -> Option<i64> {
        let seen = self.seen.lock().expect("activity");
        seen.iter().filter(|(client, _)| asker.is_empty() || client.as_str() != asker).map(|(_, at)| *at).max()
    }

    /// `response`, a page's stream, counted among the open pages while its body lives.
    fn held_open(&self, response: Response) -> Response {
        let open = Counted::of(&self.pages);
        let (parts, body) = response.into_parts();
        let body = Body::from_stream(body.into_data_stream().map(move |chunk| {
            let _open = &open;
            chunk
        }));
        Response::from_parts(parts, body)
    }
}

pub(crate) async fn unknown_route(method: Method, uri: Uri) -> Refusal {
    refuse(
        Kind::NotFound,
        "unknown_route",
        format!("Calcifer serves no {method} {}", uri.path()),
    )
}

#[derive(Deserialize)]
pub(crate) struct Resume {
    #[serde(default)]
    starting_after: u64,
}

/// The run's events from `starting_after`: what the store holds after it, the latest live
/// events, then the broadcast as it happens. The stream ends after the run's terminal event once
/// each of its outputs has landed or carries an `export_error`; Calcifer stopping ends it without
/// one (reconnect from the last `id:`, as `Last-Event-ID` or `?starting_after=`).
pub(crate) async fn events(
    State(api): State<Arc<Api>>,
    Path(path): Path<RunPath>,
    headers: axum::http::HeaderMap,
    query: Result<Query<Resume>, QueryRejection>,
) -> Result<Sse<KeepAliveStream<BoxStream<'static, Result<sse::Event, Infallible>>>>, Refusal> {
    let Query(Resume { starting_after }) = query?;
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok());
    let starting_after = starting_after.max(last.unwrap_or(0));
    let key = path.key();
    // Subscribed first: whatever happens after the store is read is in the tail.
    let tail = api.calcifer.subscribe();
    let row = find_scoped(&api, path.project(), key).await?;
    let mut pending = VecDeque::from(
        api.calcifer.row_store(&row)?.events_after(row.id.as_str(), starting_after)?,
    );
    // Replayed events use the live GPU without giving another project's rows a live tail.
    for event in &mut pending {
        if let Event::Created { run, .. } | Event::Queued { run, .. } | Event::InProgress { run, .. } = event {
            api.calcifer.live_gpu(run);
        }
    }
    let owner = api.calcifer.row_owner(&row)?;
    let named = crate::project_of(&row);
    let inert = owner.as_ref().is_some_and(|owner| owner.project_id() != row.owner);
    if !inert { pending.extend(api.calcifer.live(&row.id)); }
    let follow = Follow {
        owner,
        named,
        inert,
        vault_token: crate::project::vault::readable_token(&row.out_dir),
        _following: (row.state != RunState::Settled).then(|| crate::project::vault::following(&row.out_dir)).flatten(),
        id: row.id,
        pending,
        last: starting_after,
        ended: row.state == RunState::Settled,
        tail,
        stop: api.stop.subscribe(),
        calcifer: api.calcifer.clone(),
    };
    let stream = futures_util::stream::unfold(follow, Follow::next);
    Ok(Sse::new(stream.boxed()).keep_alive(KeepAlive::default()))
}

/// One SSE subscriber's place in its run's stream.
struct Follow {
    owner:Option<crate::project::RunOwner>,
    /// Its project's id and name, which each record it sends names as GET's does: the store's records name none.
    named: Option<(String, String)>,
    inert:bool,
    vault_token: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// A run still going, followed: its encrypted project stays unlocked meanwhile.
    _following: Option<crate::project::vault::Following>,
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
    fn store(&self)->crate::store::Result<crate::store::RunStore<'_>> {match &self.owner {Some(owner)=>owner.store().map(crate::store::RunStore::Project),None=>Ok(crate::store::RunStore::Maintenance(self.calcifer.maintenance_store()))}}
    async fn next(mut self) -> Option<(Result<sse::Event, Infallible>, Self)> {
        loop {
            if self.vault_token.as_ref().is_some_and(|token| !token.load(std::sync::atomic::Ordering::Acquire)) { return None; }
            if let Some(mut event) = self.pending.pop_front() {
                if let (Some((id, name)), Event::Created { run, .. } | Event::Queued { run, .. } | Event::InProgress { run, .. }
                    | Event::Completed { run, .. } | Event::Failed { run, .. } | Event::Canceled { run, .. }) = (&self.named, &mut event)
                {
                    (run.project_id, run.project) = (Some(id.clone()), Some(name.clone()));
                }
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
            if self.inert { return None; }
            // An ended run's stream waits only for its outputs to land (or a store it cannot read).
            let owed = self.store().and_then(|store| store.outputs_to_export(self.id.as_str()));
            if self.ended && !owed.is_ok_and(|outputs| !outputs.is_empty()) {
                return None;
            }
            tokio::select! {
                received = self.tail.recv() => match received {
                    Ok((id, event)) if id == self.id => self.pending.push_back(event),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let journal=match &self.owner {Some(owner)=>owner.store().map(crate::store::RunStore::Project),None=>Ok(crate::store::RunStore::Maintenance(self.calcifer.maintenance_store()))};
                        match journal.and_then(|store| store.events_after(self.id.as_str(), self.last)) {
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
