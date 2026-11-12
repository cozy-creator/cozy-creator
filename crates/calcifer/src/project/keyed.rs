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
    if !matches!(*request.method(),Method::GET|Method::HEAD|Method::OPTIONS)
        && request.headers().get(api::calcifer::project::PROJECT_STORAGE_HEADER).and_then(|value|value.to_str().ok())!=Some(api::calcifer::project::PROJECT_STORAGE_VERSION)
    {
        return Refusal(Error::new(Kind::Usage,"project_storage_protocol_required","update this client: project storage protocol 3 is required")).into_response();
    }
    // Opening a project attaches only its admitted journal, using a bounded lease.
    // The middleware releases it before a handler waits for a worker or a stream.
    if request.uri().path()!="/v1/projects" {
        let (mut parts,body)=request.into_parts();
        let project=match project_of(&api,&mut parts).await { Ok(project)=>project,Err(refusal)=>return refusal.into_response() };
        if let Some(project)=project { if let Err(error)=api.projects.store(&api,&project.id).await { return Refusal(error).into_response(); } }
        request=Request::from_parts(parts,body);
    }
    if matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(request).await;
    }
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
