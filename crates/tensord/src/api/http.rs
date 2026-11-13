//! The machine API over HTTP (G/API.md, design/machine-http-api.md): one axum Router behind the
//! pinned-TLS listener, every call authorized by one `Cozy-Cap`, every refusal an
//! `error::Envelope`.
use super::{
    auth::{StreamAuthority, VerifiedActor},
    capability::{self, Grant},
    v1, MachineIdentity,
};
use crate::runs::{actor_id, OutputSnapshot, Runs};
use platform::tail as logs;
use axum::{
    body::{Body, Bytes},
    extract::{rejection::QueryRejection, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Request},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post, put},
    Json, Router,
};
use api::error::{Envelope, Error};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use futures_util::{Stream, StreamExt};
use v1::path;
use tower_service::Service as _;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

/// A refusal as its envelope, at the status its kind names unless the protocol names another.
#[derive(Debug)]
pub(crate) struct Refusal(pub Error, Option<StatusCode>);
impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        Self(error, None)
    }
}
impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = self.1.unwrap_or_else(|| {
            StatusCode::from_u16(self.0.kind.http_status()).unwrap_or(StatusCode::BAD_REQUEST)
        });
        (status, Json(Envelope { error: self.0 })).into_response()
    }
}

type Answer = Result<Response, Refusal>;

#[derive(Clone)]
pub(super) struct Machine {
    identity: Arc<MachineIdentity>,
    runs: Arc<Runs>,
}

pub(super) fn router(identity: Arc<MachineIdentity>, runs: Arc<Runs>) -> Router {
    routes()
        .route(path::PRIVATE_CLOSE, post(private_close_request))
        .route(path::LOCAL_PRIVATE, get(private_request).post(private_request).put(private_request))
        .with_state(Machine { identity, runs })
}

fn routes() -> Router<Machine> {
    Router::new()
        .route(path::STATUS, get(status))
        .route(path::KEEPALIVE, post(keepalive))
        .route(path::RUNS, post(update).layer(DefaultBodyLimit::max(16 << 20)).get(list))
        .route(path::RUNS_BATCH, post(submit).layer(DefaultBodyLimit::max(64 << 20)))
        .route(path::EVENTS, get(events))
        .route(path::CANCEL, post(cancel))
        .route(path::OUTPUT, get(output))
        .route(path::PREVIEW, get(preview))
        .route(path::LOG, get(log))
        .route(path::OBJECT, put(put_object).get(follow_object))
        .route(path::OBJECTS_HELD, post(held_objects).layer(DefaultBodyLimit::max(16 << 20)))
        .route(path::UPCOMING, put(upcoming))
        .route(path::PREPARE, put(prepare))
        .route(path::MODELS_REMOVE, post(remove_model))
        .route(path::HEALTH, get(health))
        .fallback(|| async { Refusal::from(Error::not_found("route_not_found", "this machine serves no such path")) })
        .method_not_allowed_fallback(|| async {
            Refusal(Error::validation("method_not_allowed", "this path takes another method"), Some(StatusCode::METHOD_NOT_ALLOWED))
        })

}

async fn private_close_request(State(m): State<Machine>, Path(project): Path<String>, request: Request<Body>) -> Response {
    private_request(State(m), Path((project, "close".into())), request).await
}

/// New privacy namespace: authenticate and verify a live protected-view proof BEFORE
/// reading any object body. Unsupported/old peers answer404 before receiving privatebytes.
async fn private_request(State(m): State<Machine>, Path((project, rest)): Path<(String, String)>, request: Request<Body>) -> Response {
    let proof = (|| {
        if !m.identity.lifecycle.is_local() {
            return Err(Error::forbidden("local_private_only", "private filesystem execution is supported only by this computer's local machine"));
        }
        let caller = m.caller(request.headers())?;
        caller.machine()?;
        let encoded = request.headers().get(v1::LOCAL_PRIVATE_DESCRIPTOR).and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 12000).ok_or_else(|| Error::forbidden("private_storage_proof", "the private workspace proof is absent"))?;
        let decoded = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| Error::forbidden("private_storage_proof", "the private workspace descriptor is invalid"))?;
        let descriptor: v1::LocalPrivateStorage = serde_json::from_slice(&decoded).map_err(|_| Error::forbidden("private_storage_proof", "the private workspace descriptor is invalid"))?;
        if descriptor.project_id != project { return Err(Error::forbidden("private_storage_scope", "the private workspace names another project")); }
        let token = request.headers().get(v1::LOCAL_PRIVATE_TOKEN).and_then(|value| value.to_str().ok())
            .ok_or_else(|| Error::forbidden("private_storage_proof", "the private workspace proof is absent"))?.to_owned();
        Ok::<_, Error>((descriptor, token, actor_id(caller.actor)))
    })();
    let (descriptor, token, actor) = match proof { Ok(value) => value, Err(error) => return Refusal::from(error).into_response() };
    let selected = async {
        let checked = descriptor.clone();
        let fresh = tokio::task::spawn_blocking(move || crate::private_io::register(&checked, &token, &actor)).await
            .map_err(|_| Error::internal("private_storage_stopped", "private workspace verification stopped"))?
            .map_err(|_| Error::forbidden("private_storage_proof", "the private workspace is locked or its proof is invalid"))?;
        if fresh {
            let runs = m.runs.clone();
            let scope = descriptor.clone();
            tokio::task::spawn_blocking(move || crate::private_io::within(Some(scope.clone()), || {
                runs.service.engine.reconcile()?;
                crate::runs::resume_private(&runs, &scope.project_id)
            })).await.map_err(|_| Error::internal("private_storage_stopped", "private workspace recovery stopped"))?
                .map_err(|_| Error::conflict("private_storage_recovery", "private execution recovery requires the unlocked project"))?;
        }
        Ok::<_, Error>(descriptor)
    }.await;
    let descriptor = match selected { Ok(value) => value, Err(error) => return Refusal::from(error).into_response() };
    if rest == "health" && request.method() == axum::http::Method::GET {
        return StatusCode::NO_CONTENT.into_response();
    }
    if rest == "close" && request.method() == axum::http::Method::POST {
        let headers = request.headers().clone();
        return crate::private_io::request(descriptor, close_private(State(m), headers)).await.into_response();
    }
    // A private scope is data execution, never software updates or global machine log access.
    if rest.starts_with("logs/") || rest == "keepalive" || (rest == "runs" && request.method() != axum::http::Method::GET) {
        return Refusal::from(Error::forbidden("private_storage_operation", "this operation is not part of private project execution")).into_response();
    }
    let uri = format!("/v1/{rest}{}", request.uri().query().map(|query| format!("?{query}")).unwrap_or_default());
    let uri = match uri.parse() { Ok(uri) => uri, Err(_) => return Refusal::from(Error::validation("invalid_request", "invalid private request path")).into_response() };
    // Do not carry the outer wildcard's captured URL parameters into the inner router.
    // Every inner Path extractor must see only its own run/digest parameters.
    let (parts, body) = request.into_parts();
    let mut request = Request::new(body);
    *request.method_mut() = parts.method;
    *request.version_mut() = parts.version;
    *request.headers_mut() = parts.headers;
    *request.uri_mut() = uri;
    let mut routing = routes().with_state(m);
    crate::private_io::request(descriptor, async move {
        let ready = std::future::poll_fn(|context| <Router as tower_service::Service<Request<Body>>>::poll_ready(&mut routing, context)).await;
        match ready { Ok(()) => {}, Err(never) => match never {} }
        match routing.call(request).await { Ok(response) => response, Err(never) => match never {} }
    }).await
}

async fn close_private(State(m): State<Machine>, headers: HeaderMap) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let descriptor = crate::private_io::current().ok_or_else(|| Error::forbidden("private_storage_scope", "no private workspace was selected"))?;
    let actor = actor_id(caller.actor);
    m.call(move |runs| {
        runs.service.engine.assert_private_idle(&descriptor.project_id).map_err(|_| Error::conflict("private_storage_busy", "private execution or output finalization is still active"))?;
        if let Some(pool) = runs.service.pool() { pool.close_private_namespace(&descriptor).map_err(|_| Error::conflict("private_storage_busy", "a private executor is still exiting"))?; }
        runs.service.engine.close_private_storage(&descriptor.project_id).map_err(|_| Error::conflict("private_storage_busy", "private output custody is still active"))?;
        crate::private_io::close(&descriptor, &actor).map_err(|_| Error::conflict("private_storage_busy", "private input/output handles are still in use"))
    }).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Who may do what: a machine-scope cap does everything as its signer; a run-scope cap (a play
/// link) may only read its run's events and its granted outputs, until its expiry.
pub(super) struct Caller {
    pub actor: VerifiedActor,
    grant: Grant,
}
impl Caller {
    fn machine(&self) -> Result<(), Error> {
        match self.grant.action == capability::MACHINE {
            true => Ok(()),
            false => Err(Error::forbidden("capability_scope", "this call needs a machine-scope Cozy-Cap")),
        }
    }
    /// An output-limited run grant (a browser playback link), which sees only those outputs.
    pub(super) fn limit(&self) -> Option<Grant> {
        let limited = self.grant.action != capability::MACHINE && !self.grant.outputs.is_empty();
        limited.then(|| self.grant.clone())
    }
    fn run(&self, id: &str, output: Option<(&str, Option<u32>)>) -> Result<(), Error> {
        let allowed = self.grant.action == capability::MACHINE
            || match output {
                Some((name, index)) => self.grant.allows(id, name, index),
                None => self.grant.run == id,
            };
        match allowed {
            true => Ok(()),
            false => Err(Error::forbidden("capability_scope", "the capability does not grant this")),
        }
    }
}

impl Machine {
    fn caller(&self, headers: &HeaderMap) -> Result<Caller, Error> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix(capability::SCHEME))
            .ok_or_else(|| Error::credential("capability_absent", "the call carries no Cozy-Cap"))?;
        let authority = &self.identity.authority;
        let (grant, signer) = authority
            .keys
            .check(|keys| capability::verify(token, &authority.worker_id, keys, unix_now(), ""))
            .map_err(|refusal| match refusal {
                capability::Refusal::Clock => Error::credential("machine_clock", format!("{refusal} (this machine reads {} Unix seconds): set right whichever is wrong", unix_now())),
                refusal => Error::credential("capability_invalid", refusal.to_string()),
            })?;
        let owner = authority.keys.granted(&signer);
        Ok(Caller { actor: VerifiedActor { public_key: signer.to_bytes(), owner }, grant })
    }

    /// A response stream lives until its key stops authorizing; a play link's also until its
    /// expiry. A machine cap's expiry held at open: a long run, upload or read outlives it.
    pub(super) fn authority(&self, caller: &Caller) -> StreamAuthority {
        let expires = (caller.grant.action != capability::MACHINE).then_some(caller.grant.expires);
        StreamAuthority::new(self.identity.authority.keys.clone(), caller.actor, expires)
    }

    /// Holds idle release, and an update's activation, while a call that may start work runs.
    fn admit(&self) -> Result<crate::machine::lifecycle::Admission, Error> {
        self.identity.lifecycle.admit()
    }

    pub(super) async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Arc<Runs>) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        blocking(&self.runs, f).await
    }
}

/// `f` on the blocking pool: the journal and files are synchronous.
pub(super) async fn blocking<T: Send + 'static>(
    runs: &Arc<Runs>,
    f: impl FnOnce(&Arc<Runs>) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    let runs = runs.clone();
    let scope = crate::private_io::current();
    tokio::task::spawn_blocking(move || crate::private_io::within_http(scope, || f(&runs)))
        .await
        .map_err(|_| Error::internal("machine_operation_stopped", "machine operation stopped"))?
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs() as i64)
}

fn query<T: DeserializeOwned>(query: Result<Query<T>, QueryRejection>) -> Result<T, Error> {
    query.map(|Query(query)| query).map_err(|e| Error::validation("invalid_request", e.body_text()))
}

fn json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    (status, Json(value)).into_response()
}

/// An SSE response of `events`, a comment every 15 s so idle NAT mappings stay open, ended (not
/// closed cleanly) once the caller's authority lapses.
pub(super) fn sse<S>(authority: StreamAuthority, events: S) -> Response
where
    S: Stream<Item = Result<Event, Error>> + Send + 'static,
{
    Sse::new(authority.wrap(Box::pin(events)))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn wants_events(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/event-stream"))
}

/// `GET /v1/status`: anyone gets the identity frame with the readiness receipt; a
/// machine-scope cap gets the whole picture, as SSE (sent again on change) when it asks.
async fn status(State(m): State<Machine>, headers: HeaderMap) -> Answer {
    let caller = match headers.contains_key(header::AUTHORIZATION) {
        false => None,
        true => Some(m.caller(&headers)?).filter(|c| c.machine().is_ok()),
    };
    let Some(caller) = caller else {
        return Ok(json(StatusCode::OK, &super::machine_status::identity_frame(&m.identity)));
    };
    if !wants_events(&headers) {
        let frame = super::machine_status::frame(&m.identity, &m.runs, caller.actor).await?;
        return Ok(json(StatusCode::OK, &frame));
    }
    let frames = super::machine_status::watch(m.identity.clone(), m.runs.clone(), caller.actor)
        .map(|frame| frame.and_then(|frame| event("status", &frame)));
    Ok(sse(m.authority(&caller), frames))
}

/// One SSE event named `name` whose data is `value`.
pub(super) fn event<T: Serialize>(name: &str, value: &T) -> Result<Event, Error> {
    Event::default()
        .event(name)
        .json_data(value)
        .map_err(|e| Error::internal("event_unencodable", e.to_string()))
}

/// `POST /v1/keepalive`: restarts the idle clock once; answers the frame with the new deadline.
async fn keepalive(State(m): State<Machine>, headers: HeaderMap) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let lifecycle = m.identity.lifecycle.clone();
    // The idle ledger is written durably, so on the blocking pool.
    m.call(move |_| lifecycle.renew()).await?;
    let frame = super::machine_status::frame(&m.identity, &m.runs, caller.actor).await?;
    Ok(json(StatusCode::OK, &frame))
}

/// Removes a local cached repository without stopping any work or changing its origin.
async fn remove_model(State(m): State<Machine>, headers: HeaderMap, body: Bytes) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    if !m.identity.lifecycle.is_local() || crate::private_io::current().is_some() {
        return Err(Error::forbidden("model_remove_local_only", "Model removal is available only in this computer's ordinary local cache").into());
    }
    let asked: v1::RemoveModel = serde_json::from_slice(&body)
        .map_err(|e| Error::validation("invalid_request", format!("Cannot read model removal request: {e}")))?;
    let model = asked.model.trim().to_owned();
    if !tensorfs::format::model_name(&model) || model.contains('@') {
        return Err(Error::validation("model_ref_invalid", "Specify a model repository as org/name, without a release or lane").into());
    }
    let admitted = m.admit()?;
    let removed = m.call(move |runs| {
        let result = runs.service.engine.store.remove_cached_model(&model)
            .map_err(|e| Error::internal("model_remove_failed", e.to_string()))?;
        if result.removed {
            runs.service.engine.with_journal(|journal| journal.move_catalog())
                .map_err(|e| Error::internal("model_remove_failed", e.to_string()))?;
            runs.service.engine.notify_activity();
        }
        Ok(v1::RemovedModel { model, removed: result.removed, cache_bytes_removed: result.cache_bytes_removed, retained_bytes: result.retained_bytes })
    }).await?;
    drop(admitted);
    Ok(json(StatusCode::OK, &removed))
}

fn run_id(id: &str) -> Result<(), Error> {
    match id.is_empty() || id.len() > 256 {
        true => Err(Error::validation("invalid_run_id", "a run id is 1-256 bytes")),
        false => Ok(()),
    }
}

/// `POST /v1/runs`: a machine update, the one request a controller of another release sends:
/// accepted (201), or answered as the one its id names (200).
async fn update(State(m): State<Machine>, headers: HeaderMap, body: Bytes) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let request: v1::RunRequest = serde_json::from_slice(&body)
        .map_err(|e| Error::validation("invalid_request", format!("the run request is not JSON: {e}")))?;
    run_id(&request.id)?;
    if request.spec.kind != v1::RunKind::Update {
        return Err(Error::validation("invalid_kind", format!("work is sent to POST {}", path::RUNS_BATCH)).into());
    }
    let (created, state) = super::machine_update::submit(m.identity.clone(), m.runs.clone(), caller.actor, request).await?;
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok(json(status, &state))
}

/// `POST /v1/runs/batch`: the one admission of work (cl-816). The runs are accepted together
/// before any prepares, so the scheduler packs them as the machine's quote did; each is new or
/// answered as the one its id and spec name, or refused alone. They are admitted before they are
/// accepted: an update's activation and a rental's idle release wait for them.
async fn submit(State(m): State<Machine>, headers: HeaderMap, body: Bytes) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let batch: v1::RunBatch = serde_json::from_slice(&body)
        .map_err(|e| Error::validation("invalid_request", format!("the run batch is not JSON: {e}")))?;
    for request in &batch.runs {
        run_id(&request.id)?;
    }
    let admitted = m.admit()?;
    let actor = caller.actor;
    let accepted = m.call(move |runs| runs.accept(actor, batch.runs)).await?;
    drop(admitted);
    let runs = (accepted.into_iter())
        .map(|admitted| match admitted {
            Ok((created, state)) => v1::Admitted { created, state: Some(state), refusal: None },
            Err(error) => v1::Admitted { refusal: Some(error), ..Default::default() },
        })
        .collect();
    Ok(json(StatusCode::OK, &v1::RunBatchAnswer { runs }))
}

/// `PUT /v1/upcoming` (cl-967): the caller's upcoming jobs here, nearest first, each prepared behind every run (`ahead`).
async fn upcoming(State(m): State<Machine>, headers: HeaderMap, body: Bytes) -> Answer {
    let list: v1::Upcoming = serde_json::from_slice(&body)
        .map_err(|e| Error::validation("invalid_request", format!("the upcoming list is not JSON: {e}")))?;
    listed(m, headers, crate::ahead::UPCOMING, list.jobs).await
}

/// `PUT /v1/prepare` (cl-967): what the caller expects demand for here, with no job yet, behind every upcoming job (`ahead`).
async fn prepare(State(m): State<Machine>, headers: HeaderMap, body: Bytes) -> Answer {
    let list: v1::Prepare = serde_json::from_slice(&body)
        .map_err(|e| Error::validation("invalid_request", format!("the prepare list is not JSON: {e}")))?;
    listed(m, headers, crate::ahead::PREPARE, list.items).await
}

async fn listed(m: Machine, headers: HeaderMap, prefix: &'static str, items: Vec<v1::Ahead>) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    for item in &items {
        run_id(&format!("{prefix}{}", item.key))?;
    }
    let admitted = m.admit()?;
    let actor = caller.actor;
    let answer = m.call(move |runs| crate::ahead::put(runs, actor, prefix, items)).await?;
    drop(admitted);
    Ok(json(StatusCode::OK, &answer))
}


#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    before: u64,
    #[serde(default)]
    limit: u32,
}

/// `GET /v1/runs`: the runs the caller reads, newest first, before the run numbered `?before=`.
async fn list(State(m): State<Machine>, headers: HeaderMap, page: Result<Query<Page>, QueryRejection>) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let (Page { before, limit }, actor) = (query(page)?, caller.actor);
    let list = m.call(move |runs| runs.list(actor, before, limit)).await?;
    Ok(json(StatusCode::OK, &list))
}

#[derive(Deserialize)]
struct After {
    #[serde(default)]
    after: u64,
}

/// `GET /v1/runs/{id}/events`: the run's log as SSE after `Last-Event-ID` (or `?after=`), its
/// snapshot first. Closing it never cancels; it holds no admission, only the idle clock.
async fn events(
    State(m): State<Machine>,
    Path(id): Path<String>,
    headers: HeaderMap,
    after: Result<Query<After>, QueryRejection>,
) -> Answer {
    let caller = m.caller(&headers)?;
    caller.run(&id, None)?;
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok()?.trim().parse().ok());
    let after = last.unwrap_or(query(after)?.after);
    let (identity, actor, owned) = (m.identity.clone(), caller.actor, id.clone());
    if m.call(move |runs| super::machine_update::owns(&identity, runs, actor, &owned)).await? {
        caller.machine()?;
        let events = super::machine_update::events(m.identity.clone(), id, after).await?;
        let events = followed(&m.identity, events.map(|e| e.and_then(|e| run_event(&e))));
        return Ok(sse(m.authority(&caller), events));
    }
    let events = super::machine_runs::follow(m.runs.clone(), caller.actor, id, after, caller.limit()).await?;
    let events = received(events);
    Ok(sse(m.authority(&caller), followed(&m.identity, events.map(|e| e.and_then(|e| run_event(&e))))))
}

/// What `receiver` receives, as a stream.
pub(super) fn received<T>(mut receiver: tokio::sync::mpsc::Receiver<T>) -> impl Stream<Item = T> {
    futures_util::stream::poll_fn(move |cx| receiver.poll_recv(cx))
}

/// `events` holding the machine's idle clock while a client follows them.
fn followed<S: Stream + Send + 'static>(identity: &MachineIdentity, events: S) -> impl Stream<Item = S::Item> + Send + 'static {
    let held = identity.lifecycle.follow();
    events.map(move |event| {
        let _held = &held;
        event
    })
}

/// A run event as SSE: named by its variant, its sequence as the id when it is durable.
pub(super) fn run_event(event: &v1::RunEvent) -> Result<Event, Error> {
    let name = event.event.name();
    let durable = !matches!(name, "state" | "progress" | "gpu") && event.sequence > 0;
    let sse = self::event(name, event)?;
    Ok(if durable { sse.id(event.sequence.to_string()) } else { sse })
}

/// `POST /v1/runs/{id}/cancel`. An unknown id is tombstoned as canceled.
async fn cancel(State(m): State<Machine>, Path(id): Path<String>, headers: HeaderMap) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    run_id(&id)?;
    let (identity, actor, owned) = (m.identity.clone(), caller.actor, id.clone());
    if m.call(move |runs| super::machine_update::owns(&identity, runs, actor, &owned)).await? {
        return Ok(json(StatusCode::OK, &super::machine_update::cancel(&m.identity, &id).await?));
    }
    Ok(json(StatusCode::OK, &m.call(move |runs| runs.cancel(actor, &id)).await?))
}

/// `GET /v1/runs/{id}/preview`: the run's newest preview, `ETag: "p<n>"`; 204 while it has none.
/// Each ask keeps its executor making them for a while (cr-282).
async fn preview(State(m): State<Machine>, Path(id): Path<String>, headers: HeaderMap) -> Answer {
    let caller = m.caller(&headers)?;
    caller.run(&id, None)?;
    run_id(&id)?;
    let actor = caller.actor;
    let Some(preview) = m.call(move |runs| runs.preview(actor, &id)).await? else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let tag = format!("\"p{}\"", preview.n);
    let held = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    if held.is_some_and(|held| held.split(',').any(|t| t.trim() == tag)) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, tag)]).into_response());
    }
    let step = format!("{}/{}", preview.position, preview.total);
    let headers = [
        (header::CONTENT_TYPE, preview.media_type.clone()),
        (header::ETAG, tag),
        (header::CACHE_CONTROL, "no-store".to_owned()),
        (header::HeaderName::from_static(v1::PREVIEW_STEP), step),
    ];
    Ok((StatusCode::OK, headers, preview.bytes.clone()).into_response())
}

#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    member: String,
}

/// `GET /v1/runs/{id}/outputs/{output}`: an output's current bytes, ranged, readable while it
/// grows. `ETag` is its revision; `If-Match` refuses once the output moved past it.
async fn output(
    State(m): State<Machine>,
    Path((id, output)): Path<(String, String)>,
    item: Result<Query<Item>, QueryRejection>,
    headers: HeaderMap,
) -> Answer {
    let caller = m.caller(&headers)?;
    let Item { index, member } = query(item)?;
    let index = (index > 0).then_some(index);
    caller.run(&id, Some((&output, index)))?;
    let actor = caller.actor;
    let snapshot = m
        .call(move |runs| match member.as_str() {
            "" => runs.open_output(actor, &id, &output, index),
            member => runs.open_member(actor, &id, &output, index, member),
        })
        .await?;
    let tag = format!("\"{}\"", snapshot.rev);
    let wanted = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    if wanted.is_some_and(|wanted| wanted.trim() != "*" && !wanted.split(',').any(|t| t.trim() == tag)) {
        let moved = Error::conflict("output_revision_moved", format!("the output is at revision {}", snapshot.rev));
        return Err(Refusal(moved, Some(StatusCode::PRECONDITION_FAILED)));
    }
    let OutputSnapshot { parts, length, media_type, .. } = snapshot;
    let mut response = ranged(&headers, parts, length, m.authority(&caller), m.identity.lifecycle.clone())?;
    let headers = response.headers_mut();
    headers.insert(header::ETAG, HeaderValue::from_str(&tag).expect("a revision is ASCII"));
    if let Ok(media_type) = HeaderValue::from_str(&media_type) {
        headers.insert(header::CONTENT_TYPE, media_type);
    }
    Ok(response)
}

/// An output's bytes on disk: its parts in order, each with its length.
type Parts = Vec<(std::fs::File, u64)>;

/// `Range: bytes=a-b` (or `a-`, `-n`) of `length` bytes as `[start, end)`; None for the whole.
fn range(headers: &HeaderMap, length: u64) -> Result<Option<(u64, u64)>, Refusal> {
    let Some(spec) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let unsatisfiable = || {
        let error = Error::validation("range_unsatisfiable", format!("the output is {length} bytes"));
        Refusal(error, Some(StatusCode::RANGE_NOT_SATISFIABLE))
    };
    let Some((first, last)) = spec.trim().strip_prefix("bytes=").and_then(|r| r.split_once('-')) else {
        return Err(unsatisfiable());
    };
    let number = |n: &str| n.trim().parse::<u64>().map_err(|_| unsatisfiable());
    let (start, end) = match (first.trim(), last.trim()) {
        ("", suffix) => (length.saturating_sub(number(suffix)?), length),
        (start, "") => (number(start)?, length),
        (start, last) => (number(start)?, number(last)?.saturating_add(1).min(length)),
    };
    match start < end {
        true => Ok(Some((start, end))),
        false => Err(unsatisfiable()),
    }
}

/// `parts` as 200, or the range the request names as 206, streamed from the blocking pool.
/// Each chunk the reader takes counts as the rental's use (`Lifecycle::transferred`).
fn ranged(
    headers: &HeaderMap,
    parts: Parts,
    length: u64,
    authority: StreamAuthority,
    lifecycle: Arc<crate::machine::lifecycle::Lifecycle>,
) -> Answer {
    let range = range(headers, length)?;
    let (start, end) = range.unwrap_or((0, length));
    let reader = from_offset(parts, start)
        .map_err(|e| Error::internal("output_unreadable", e.to_string()))?
        .take(end - start);
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    tokio::task::spawn_blocking(move || {
        let (mut reader, mut buffer, mut left) = (reader, vec![0; 1 << 20], end - start);
        while left > 0 {
            // Bytes that end before the revision's length fail the body, never a short success.
            let read = match reader.read(&mut buffer) {
                Ok(0) => Err(Error::internal("output_short", "the output's bytes ended before its length")),
                Ok(n) => Ok(Bytes::copy_from_slice(&buffer[..n])),
                Err(error) => Err(Error::internal("output_unreadable", error.to_string())),
            };
            left = match &read {
                Ok(bytes) => left - bytes.len() as u64,
                Err(_) => 0,
            };
            if sender.blocking_send(read).is_err() {
                return;
            }
        }
    });
    let chunks = received(receiver).map(move |chunk| {
        if chunk.is_ok() {
            lifecycle.transferred();
        }
        chunk
    });
    let body = authority.wrap(Box::pin(chunks));
    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
    if range.is_some() {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        let span = format!("bytes {start}-{}/{length}", end - 1);
        response.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&span).expect("ASCII"));
    }
    Ok(response)
}

/// The bytes from `offset`. Parts before it are skipped and the part it falls in is seeked
/// into, so a resumed download costs the machine only its tail.
fn from_offset(parts: Parts, mut offset: u64) -> std::io::Result<Box<dyn Read + Send>> {
    let mut reader: Box<dyn Read + Send> = Box::new(std::io::empty());
    for (mut file, length) in parts {
        if offset >= length {
            offset -= length;
            continue;
        }
        file.seek(SeekFrom::Current(offset as i64))?;
        reader = Box::new(reader.chain(file.take(length - offset)));
        offset = 0;
    }
    Ok(reader)
}

#[derive(Deserialize)]
struct Last {
    #[serde(default)]
    lines: u64,
}

/// `GET /v1/logs/{name}?lines=N`: a machine log's newest N whole lines (0: all it keeps). As SSE,
/// those and then each batch as it is appended, each event's id the place after it: a
/// `Last-Event-ID` resumes there.
async fn log(
    State(m): State<Machine>,
    Path(name): Path<String>,
    last: Result<Query<Last>, QueryRejection>,
    headers: HeaderMap,
) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    // TensorFS and TensorD's memory notes append to the store's logs, each rotating to one older
    // file; TensorD's own log rolls daily.
    let log = match name.as_str() {
        "tensorfs-transport" => logs::Log::rotated(m.identity.store_logs.clone(), "transport.log"),
        "memory" => logs::Log::rotated(m.identity.store_logs.clone(), "memory.log"),
        "tensord" => logs::Log::daily(m.identity.logs.clone(), "tensord"),
        _ => return Err(Error::not_found("log_not_found", format!("this machine keeps no log {name}")).into()),
    };
    let start = match headers.get("last-event-id") {
        None => logs::Start::Last(query(last)?.lines),
        Some(id) => logs::Start::After(
            id.to_str()
                .ok()
                .and_then(|id| id.trim().parse().ok())
                .ok_or_else(|| Error::validation("invalid_request", "Last-Event-ID is not a log cursor"))?,
        ),
    };
    let unreadable = |e: std::io::Error| Error::internal("log_unreadable", e.to_string());
    if !wants_events(&headers) {
        let text = m.call(move |_| log.read(start).map_err(unreadable)).await?;
        return Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response());
    }
    let follow = m
        .call(move |_| match log.follow(start) {
            Ok(follow) => Ok(follow),
            Err(e) if e.is_unsupported() => Err(Error::unavailable("log_follow_unsupported", e.to_string())),
            Err(e) => Err(Error::internal("log_unreadable", e.to_string())),
        })
        .await?;
    let (sender, lines) = tokio::sync::mpsc::channel(16);
    tokio::task::spawn_blocking(move || {
        let read = follow.run(|lines| sender.blocking_send(Ok(lines)).is_ok(), || !sender.is_closed());
        if let Err(e) = read {
            let _ = sender.blocking_send(Err(e));
        }
    });
    let lines = received(lines).map(move |lines| match lines {
        Ok(lines) => {
            let after = lines.after.to_string();
            Ok(event("lines", &v1::LogLines { after: after.clone(), text: lines.text })?.id(after))
        }
        Err(e) => Err(unreadable(e)),
    });
    Ok(sse(m.authority(&caller), followed(&m.identity, lines)))
}

fn held(ranges: &[(u64, u64)]) -> HeaderValue {
    HeaderValue::from_str(&v1::spell_held(ranges)).expect("ranges are ASCII")
}

#[derive(Deserialize)]
struct Length {
    length: u64,
}

/// `GET /v1/objects/sha256:<hex>?length=N` as SSE: `held` events, the bytes this signer's upload of
/// the object holds as they land (after each MiB more), ending once it holds all N. A stored object
/// answers N at once.
async fn follow_object(
    State(m): State<Machine>,
    Path(digest): Path<String>,
    length: Result<Query<Length>, QueryRejection>,
    headers: HeaderMap,
) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let length = query(length)?.length;
    let (runs, actor) = (m.runs.clone(), actor_id(caller.actor));
    let upload = blocking(&runs, move |runs| runs.objects.upload(&actor, &digest, length).map_err(Into::into)).await?;
    let mut landed = match &upload {
        Some(upload) => upload.follow(),
        None => tokio::sync::watch::channel(length).1,
    };
    let (sender, held) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        // Joined while it is followed; let go once its follower is.
        let _upload = upload;
        let mut sent = None;
        loop {
            let now = *landed.borrow_and_update();
            if sent.is_none_or(|sent| now >= length || now >= sent + (1 << 20)) {
                if sender.send(event("held", &v1::HeldBytes { held: now })).await.is_err() || now >= length {
                    return;
                }
                sent = Some(now);
            }
            tokio::select! {
                changed = landed.changed() => if changed.is_err() { return },
                () = sender.closed() => return,
            }
        }
    });
    Ok(sse(m.authority(&caller), received(held)))
}

/// `POST /v1/objects/held`: the ranges this signer holds of each object (all of one once stored,
/// its roots renewed so a run sent next finds it), in the order asked.
async fn held_objects(State(m): State<Machine>, headers: HeaderMap, body: Bytes) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    let ask: v1::HeldRequest = serde_json::from_slice(&body)
        .map_err(|e| Error::validation("invalid_request", format!("the held request is not JSON: {e}")))?;
    let (runs, actor) = (m.runs.clone(), actor_id(caller.actor));
    let held = blocking(&runs, move |runs| {
        let held = ask.digests.into_iter().map(|digest| {
            let ranges = runs.objects.held(&actor, &digest)?;
            Ok(v1::HeldObject { digest, ranges })
        });
        held.collect::<Result<Vec<_>, crate::objects::Refused>>().map_err(Into::into)
    })
    .await?;
    Ok(json(StatusCode::OK, &v1::HeldAnswer { held }))
}

/// `PUT /v1/objects/sha256:<hex>`: lands the body at the offset its `Content-Range` names (the
/// whole object without one). What landed is synced and recorded when the body ends, however it
/// ends; the object is verified and stored once every byte is held. Uploads hold no admission.
async fn put_object(
    State(m): State<Machine>,
    Path(digest): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Answer {
    let caller = m.caller(&headers)?;
    caller.machine()?;
    drop(m.admit()?);
    let (start, end, length) = span(&headers)?;
    let (runs, actor) = (m.runs.clone(), actor_id(caller.actor));
    let objects = runs.clone();
    let Some(upload) = blocking(&objects, move |runs| runs.objects.upload(&actor, &digest, length).map_err(Into::into)).await? else {
        return Ok((StatusCode::NO_CONTENT, [(v1::HELD, held(&[(0, length)]))]).into_response());
    };
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Bytes>(8);
    let landing = upload.clone();
    let private = crate::private_io::current();
    let writer = tokio::task::spawn_blocking(move || crate::private_io::within(private, || -> Result<Vec<(u64, u64)>, crate::objects::Refused> {
        let mut at = start;
        let mut writer = landing.writer()?;
        let mut wrote = Ok(());
        while let Some(data) = receiver.blocking_recv() {
            if at + data.len() as u64 > end {
                wrote = Err(crate::objects::Refused {
                    code: "object_length_exceeded",
                    message: "the body is longer than its Content-Range".into(),
                });
                break;
            }
            if let Err(error) = landing.write_at(&mut writer, at, &data) {
                wrote = Err(error);
                break;
            }
            at += data.len() as u64;
        }
        drop(receiver);
        let held = runs.objects.settle(&landing)?;
        wrote.map(|()| held)
    }));
    let mut frames = m.authority(&caller).wrap(Box::pin(body.into_data_stream().map(|frame| {
        frame.map_err(|e| Error::validation("request_body_failed", e.to_string()))
    })));
    let mut failed = None;
    while let Some(frame) = frames.next().await {
        match frame {
            // A refused writer drops its receiver; its refusal is the answer.
            Ok(data) => {
                // An upload moving bytes holds the idle clock, as an output read does; a stalled one does not.
                m.identity.lifecycle.transferred();
                if sender.send(data).await.is_err() {
                    break;
                }
            }
            Err(error) => {
                failed = Some(error);
                break;
            }
        }
    }
    drop(sender);
    let held = writer
        .await
        .map_err(|_| Error::internal("machine_operation_stopped", "machine operation stopped"))?
        .map_err(Error::from)?;
    if let Some(error) = failed {
        return Err(error.into());
    }
    Ok((StatusCode::NO_CONTENT, [(v1::HELD, self::held(&held))]).into_response())
}

/// A PUT's `[start, end)` of an object of `length` bytes: its `Content-Range`, else the whole
/// object its `Content-Length` measures.
fn span(headers: &HeaderMap) -> Result<(u64, u64, u64), Error> {
    let text = |name| headers.get(name).and_then(|v: &HeaderValue| v.to_str().ok());
    let invalid = || Error::validation("invalid_range", "a part is `Content-Range: bytes a-b/length`");
    match text(header::CONTENT_RANGE) {
        Some(range) => {
            let (span, length) = range.trim().strip_prefix("bytes ").and_then(|r| r.split_once('/')).ok_or_else(invalid)?;
            let (first, last) = span.split_once('-').ok_or_else(invalid)?;
            let number = |n: &str| n.trim().parse::<u64>().map_err(|_| invalid());
            let (start, end, length) = (number(first)?, number(last)? + 1, number(length)?);
            match start < end && end <= length {
                true => Ok((start, end, length)),
                false => Err(invalid()),
            }
        }
        None => {
            let length = text(header::CONTENT_LENGTH).and_then(|l| l.parse().ok()).ok_or_else(|| {
                Error::validation("invalid_range", "a whole object's PUT carries its Content-Length")
            })?;
            Ok((0, length, length))
        }
    }
}

/// `/v1/health`: answers a bare probe, refuses one that carries a credential.
async fn health(headers: HeaderMap) -> Result<StatusCode, Refusal> {
    match headers.contains_key(header::AUTHORIZATION) {
        true => Err(Error::credential("health_takes_no_credential", "health answers a bare probe").into()),
        false => Ok(StatusCode::NO_CONTENT),
    }
}
