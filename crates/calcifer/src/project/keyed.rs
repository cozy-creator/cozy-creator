//! One answer per `Idempotency-Key` for every project write (cl-812). A write sent again under its key, the same ask, is answered what
//! it was the first time, across a restart too, and one still going is waited for; nothing is done twice. The same key for another ask
//! is a conflict. A project's write keeps its key in `project.db`, in the transaction of its effect (cl-803), and its answer on that row
//! just after; `POST /v1/projects`, before there's a project, keeps its answer in `state.db`. Kept a day.
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use protocol::error::{Error, Kind};
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

/// The project a write's path names, if it names one this Calcifer knows.
fn project_of(api: &Arc<Api>, path: &str) -> Option<Arc<super::Project>> {
    let id = path.strip_prefix("/v1/projects/")?.split('/').next().filter(|id| id.starts_with("project-"))?;
    api.projects.get(api, id).ok()
}

/// Around every project route: a write with an `Idempotency-Key` is answered once, and again as it was.
pub(super) async fn answered(State(api): State<Arc<Api>>, request: Request, next: Next) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(request).await;
    }
    let key = request.headers().get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::trim).filter(|k| !k.is_empty()).map(str::to_owned);
    let Some(key) = key else { return next.run(request).await };
    let (parts, body) = request.into_parts();
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
    let project = project_of(&api, parts.uri.path());
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
            let store = api.calcifer.store();
            match store.answer(&key) {
                Ok(Some(kept)) if kept.digest == digest => replay(Some((kept.status, kept.content_type, kept.body))),
                Ok(Some(_)) => conflict(&key),
                found => {
                    if let Err(e) = found {
                        warn!("reading what Idempotency-Key {key} answered: {e}");
                    }
                    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;
                    keep(&api, &key, digest, response).await
                }
            }
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

/// `response`, kept under `key` when it succeeded.
async fn keep(api: &Api, key: &str, digest: String, response: Response) -> Response {
    if !response.status().is_success() {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => return Refusal(Error::new(Kind::Internal, "answer_unread", format!("Idempotency-Key {key}: {e}"))).into_response(),
    };
    let content_type = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    let answer = Answer { digest, status: parts.status.as_u16(), content_type, body: bytes.to_vec() };
    match bytes.len() <= MOST {
        true => {
            if let Err(e) = api.calcifer.store().keep_answer(key, &answer) {
                warn!("keeping what Idempotency-Key {key} answered: {e}");
            }
        }
        false => warn!("Idempotency-Key {key}: its answer is longer than {MOST} bytes and isn't kept"),
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// What a key's write answered, again. One whose effect committed and whose words were lost (Calcifer stopped between) is `{}`: it was done.
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
