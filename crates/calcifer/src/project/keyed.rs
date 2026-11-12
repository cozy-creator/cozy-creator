//! One answer per `Idempotency-Key` for every project write (cl-812). A write sent again under its key, the same ask, is answered what
//! it was the first time, across a restart too, and one still going is waited for; nothing is done twice. The same key for another ask
//! is a conflict. A project's write keeps its key in `project.db`, in the transaction of its effect (cl-803), and its answer on that row
//! just after; `POST /v1/projects` can keep only a non-secret response inside the created project, never globally.
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    body::{Body, to_bytes},
    extract::{FromRequestParts as _, RawPathParams, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use api::error::{Error, Kind};
use sha2::{Digest as _, Sha256};
use tracing::warn;
use rusqlite::OptionalExtension as _;

use crate::{
    api::{Api, Refusal},
    store::Answer,
};

/// What a request or an answer kept may hold; a longer answer is answered once and not kept.
const MOST: usize = 8 << 20;
/// Said on an answer given again.
const REPLAYED: &str = "idempotent-replayed";

/// The writes under each key now going: a repeat waits for the first.
static GOING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Mutex::default);
#[derive(Clone)]
pub(super) struct CreationRetry(pub(super) std::path::PathBuf);
enum Creation { Fresh, Replay(Response), Resume(CreationRetry) }

tokio::task_local! {
    /// The key of the project write going, and its ask's digest, until its first act takes it.
    static CLAIM: RefCell<Option<(String, String)>>;
}

/// The key of the write this act is of, once: it commits with the act's effect.
pub(super) fn claimed() -> Option<(String, String)> {
    CLAIM.try_with(|claim| claim.borrow_mut().take()).ok().flatten()
}

/// The project a write's route names (`{id}`), if it names one this Calcifer knows.
async fn project_of(api: &Arc<Api>, parts: &mut axum::http::request::Parts) -> Result<Option<Arc<super::Project>>, Refusal> {
    if parts.uri.path() == "/v1/projects" { return Ok(None); }
    let params = RawPathParams::from_request_parts(parts, api).await.map_err(|_| Refusal(Error::new(Kind::Validation, "project_context_required", "the project route is invalid")))?;
    let Some(named) = params.iter().find(|(key, _)| *key == "id").map(|(_, value)| value.to_owned()) else { return Ok(None) };
    api.projects.get(api, &named).map(Some)
}

/// Around every project route: a write with an `Idempotency-Key` is answered once, and again as it was.
pub(super) async fn answered(State(api): State<Arc<Api>>, mut request: Request, next: Next) -> Response {
    let mut _activity=None;
    if !matches!(*request.method(),Method::GET|Method::HEAD|Method::OPTIONS) && !crate::api::speaks_project_storage(request.headers()) {
        return crate::api::project_storage_protocol_required().into_response();
    }
    // Opening a project attaches only its admitted journal, using a bounded lease.
    // The middleware releases it before a handler waits for a worker or a stream.
    if request.uri().path()!="/v1/projects" {
        let (mut parts,body)=request.into_parts();
        let project=match project_of(&api,&mut parts).await { Ok(project)=>project,Err(refusal)=>return refusal.into_response() };
        if let Some(project)=project {
            let store=match api.projects.store(&api,&project.id).await {Ok(store)=>store,Err(error)=>return Refusal(error).into_response()};
            _activity=Some(api.projects.track_request(&api,store.execution_scope().1));
        }
        request=Request::from_parts(parts,body);
    }
    if matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(request).await;
    }
    // Generic invocation already has its canonical journal key. Caching a
    // preparing reply here would prevent its retry from ever submitting the call.
    if request.uri().path().ends_with("/runs/submit") {return next.run(request).await;}
    let key = request.headers().get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::trim).filter(|k| !k.is_empty()).map(str::to_owned);
    let Some(key) = key else { return next.run(request).await };
    let (mut parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, MOST).await else {
        return Refusal(Error::new(Kind::Validation, "body_too_large", format!("a write is at most {MOST} bytes"))).into_response();
    };
    let mut hasher = Sha256::new();
    hasher.update(parts.method.as_str());
    hasher.update(b" ");
    hasher.update(parts.uri.path_and_query().map_or("", |p| p.as_str()));
    hasher.update(b"\n");
    hasher.update(&bytes);
    let digest = hex::encode(hasher.finalize());
    let one = Arc::clone(GOING.lock().unwrap_or_else(|e| e.into_inner()).entry(key.clone()).or_default());
    let held = one.lock().await;
    let project = match project_of(&api, &mut parts).await { Ok(project) => project, Err(refusal) => return refusal.into_response() };
    let response = match &project {
        Some(project) => {
            let folder = &project.folder;
            match folder.keyed(&key) {
                Some(kept) if kept.digest != digest => conflict(&key),
                Some(kept) => replay(kept.answer),
                None => {
                    let request = Request::from_parts(parts, Body::from(bytes));
                    let response = CLAIM.scope(RefCell::new(Some((key.clone(), digest.clone()))), next.run(request)).await;
                    keep_here(folder, &key, digest, response).await
                }
            }
        }
        None => {
            // Creation has no project yet. Its effect/response belongs only to the
            // created project; recovery-secret responses are never cached.
            match creation(&api,&bytes,&key,&digest).await {
                Ok(Creation::Replay(response))=>return response,
                Ok(Creation::Resume(retry))=>{parts.extensions.insert(retry);},
                Ok(Creation::Fresh)=>{},
                Err(refusal)=>return refusal.into_response(),
            }
            let request = Request::from_parts(parts, Body::from(bytes));
            let response = CLAIM.scope(RefCell::new(Some((key.clone(), digest.clone()))), next.run(request)).await;
            keep_created(&api, &key, digest, response).await
        }
    };
    drop(held);
    let mut going = GOING.lock().unwrap_or_else(|e| e.into_inner());
    if Arc::strong_count(&one) == 2 {
        going.remove(&key);
    }
    response
}

/// Resolve creation retry identity only inside the requested candidate folders.
/// A locked/unavailable candidate or incomplete stage is not permission to make _2.
async fn creation(api:&Arc<Api>,bytes:&[u8],key:&str,digest:&str)->Result<Creation,Refusal> {
    let ask:api::calcifer::project::Open=crate::api::parse(bytes)?;
    if !ask.unencrypted {return Ok(Creation::Fresh);}
    let wanted=ask.name.as_ref().map(|name|super::format::plain_name(name).unwrap_or_else(||"untitled".into()));
    let pending=||Refusal(Error::new(Kind::Conflict,"project_creation_pending","the original project or creation stage must be available before this retry can be resolved"));
    let mut index=1u64;
    loop {
        let path=match &ask.path {
            Some(path) if path.is_absolute()=>path.clone(),
            Some(_)=>return Ok(Creation::Fresh),
            None=>{let Some(wanted)=&wanted else {return Ok(Creation::Fresh);}; api.machines.user_home.join(if index==1 {wanted.clone()} else {format!("{wanted}_{index}")})},
        };
        if !path.try_exists().map_err(|_|pending())? {break;}
        if path.join(format!("{}.new",super::db::DB)).try_exists().map_err(|_|pending())? {return Err(pending());}
        if path.join(super::db::DB).try_exists().map_err(|_|pending())? || path.join(project_vault::KEYBAG_FILE).try_exists().map_err(|_|pending())? {
            let store=api.projects.store(api,&path.to_string_lossy()).await.map_err(Refusal)?;
            let found=store.read(|conn|Ok(conn.query_row("SELECT digest FROM request_keys WHERE key=?1",[key],|row|row.get::<_,String>(0)).optional()?))?;
            if let Some(found)=found {
                if found!=digest {return Err(Refusal(Error::new(Kind::Conflict,"idempotency_conflict","this key already names another project creation")));}
                if let Some(answer)=store.answer(key)? {return Ok(Creation::Replay(replay(Some((answer.status,answer.content_type,answer.body)))));}
                return Ok(Creation::Resume(CreationRetry(path)));
            }
        }
        if ask.path.is_some() {break;}
        index=index.checked_add(1).ok_or_else(pending)?;
    }
    let legacy=api.calcifer.maintenance_store().read(|conn| {
        let exists=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='legacy_project_answers')",[],|row|row.get::<_,bool>(0))?;
        if !exists {return Ok(false);}
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM legacy_project_answers WHERE key=?1)",[key],|row|row.get::<_,bool>(0))?)
    })?;
    if legacy {Err(pending())} else {Ok(Creation::Fresh)}
}

fn conflict(key: &str) -> Response {
    Refusal(Error::new(Kind::Conflict, "idempotency_conflict", format!("Idempotency-Key {key} asked another write already"))).into_response()
}

/// A project write's answer, kept on its key's row: when it succeeded, or when its effect committed under the key whatever it answered.
async fn keep_here(folder: &super::Folder, key: &str, digest: String, response: Response) -> Response {
    let committed = folder.keyed(key).is_some();
    if !response.status().is_success() && !committed {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => return Refusal(Error::new(Kind::Internal, "answer_unread", format!("Idempotency-Key {key}: {e}"))).into_response(),
    };
    let content_type = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    match bytes.len() <= MOST {
        true => {
            if let Err(e) = folder.keep_answer(key, &digest, parts.status.as_u16(), &content_type, &bytes) {
                warn!("keeping what Idempotency-Key {key} answered: {}", e.message);
            }
        }
        false => warn!("Idempotency-Key {key}: its answer is longer than {MOST} bytes and isn't kept"),
    }
    Response::from_parts(parts, Body::from(bytes))
}

async fn keep_created(api: &Arc<Api>, key: &str, digest: String, response: Response) -> Response {
    if !response.status().is_success() { return response; }
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, MOST).await {
        Ok(bytes) => bytes,
        Err(_) => return Refusal(Error::new(Kind::Internal, "answer_unread", "the creation reply could not be read")).into_response(),
    };
    if let Ok(opened) = serde_json::from_slice::<api::calcifer::project::Opened>(&bytes)
        && opened.recovery_secret.is_none()
    {
        let content_type = parts.headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default().to_owned();
        if let Ok(store) = api.projects.store(api, &opened.id).await {
            let answer = Answer { digest, status: parts.status.as_u16(), content_type, body: bytes.to_vec() };
            let _ = store.keep_answer(key, &answer);
        }
    }
    Response::from_parts(parts, Body::from(bytes))
}

fn replay(kept: Option<(u16, String, Vec<u8>)>) -> Response {
    let (status, content_type, body) = kept.unwrap_or_else(|| (200, "application/json".to_owned(), b"{}".to_vec()));
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(REPLAYED, HeaderValue::from_static("true"));
    response
}
