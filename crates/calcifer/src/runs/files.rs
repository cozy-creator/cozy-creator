//! `PUT`/`HEAD /v1/files/sha256:<hex>`: the one way a client's files reach Calcifer, by content, for a run to name (`sha256:<hex>` at
//! an input, or under a path in its `paths`), a project to import, a model to upload or a machine to update, as a machine takes
//! objects: byte ranges in any order, several side by side, resuming from the ranges held. Calcifer reads no client's disk. A file is
//! an object of this computer's TensorFS store, where its own machine holds its runs' objects too: its upload is the store's partial
//! (`Store::partial`), named only once every byte is held and they hash to the name, and kept as the store keeps what is used
//! (`Store::used`). A `HEAD` that finds one held counts as a use.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use futures_util::StreamExt as _;
use api::{
    calcifer::File,
    error::{Error, Kind},
    ids::Digest,
    machine::{HELD, spell_held},
};
use tensorfs::{
    err::Code,
    format::ObjectRef,
    store::{Partial, Store},
};

use crate::api::{Api, Refusal};

#[derive(serde::Deserialize)]
pub(crate) struct UploadScope { project: Option<String> }

static PRIVATE_STORES: LazyLock<Mutex<HashMap<PathBuf, Arc<Store>>>> = LazyLock::new(Mutex::default);

pub(crate) fn release_private(root: &Path) -> Result<(), Error> {
    let active = UPLOADS.lock().expect("uploads lock").iter().any(|((path, _), (_, upload))|
        crate::project::vault::logical(path).starts_with(root) && upload.strong_count() > 0);
    if active { return Err(Error::new(Kind::Conflict, "project_busy", "a private file upload is still finishing")); }
    let mut stores = PRIVATE_STORES.lock().expect("private upload stores");
    let keys: Vec<_> = stores.keys().filter(|path| crate::project::vault::logical(path) == root).cloned().collect();
    let gone: Vec<_> = keys.into_iter().filter_map(|path| stores.remove(&path)).collect();
    drop(stores);
    drop(gone);
    Ok(())
}

fn scoped(api: &Arc<Api>, scope: &UploadScope) -> Result<Arc<Store>, Refusal> {
    let Some(project) = &scope.project else { return Ok(store().map_err(Refusal)?.clone()) };
    let root = crate::project::private_upload_root(api, project).map_err(Refusal)?.ok_or_else(|| Refusal(Error::new(Kind::Conflict,"project_storage_required","the project storage is unavailable")))?;
    private_store(&root).map_err(Refusal)
}

fn private_store(root: &Path) -> Result<Arc<Store>, Error> {
    crate::project::vault::require_path(root)?;
    let mut stores = PRIVATE_STORES.lock().expect("private upload stores");
    if let Some(store) = stores.get(root) { return Ok(store.clone()); }
    let store = Arc::new(Store::open(root.join(".vault-uploads")).map_err(|e| Error::new(Kind::Unavailable, "file_store_unusable", e.to_string()))?);
    stores.insert(root.to_owned(), store.clone());
    Ok(store)
}

pub(crate) fn held_for(root: &Path, digest: &Digest) -> Option<PathBuf> {
    held_in(private_store(root).ok()?.as_ref(), digest)
}

fn held_in(store: &Store, digest: &Digest) -> Option<PathBuf> {
    let path = store.path(&object(digest, 0)).ok()?;
    let object = object(digest, std::fs::metadata(&path).ok()?.len());
    (store.reuse(&object).ok()? && store.used(&object.sha256, [&object]).is_ok()).then_some(path)
}

#[cfg(test)]
#[path = "files_http_tests.rs"]
mod http_tests;

/// Byte ranges `[start, end)`, sorted and apart.
type Ranges = Vec<(u64, u64)>;

/// Each file's upload not yet named: its length, and the upload while writes carry it. One that was cut keeps its length, so what
/// the store kept of it can be said.
static UPLOADS: LazyLock<Mutex<HashMap<(PathBuf, Digest), Begun>>> = LazyLock::new(Mutex::default);
type Begun = (u64, Weak<Upload>);

/// This computer's TensorFS store (`TENSORFS_HOME`, else `~/.cache/tensorfs`), its owner's alone to read.
pub(crate) fn store() -> Result<&'static Arc<Store>, Error> {
    static STORE: OnceLock<Arc<Store>> = OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(store);
    }
    let unusable = |why: String| Error::new(Kind::Unavailable, "file_store_unusable", format!("this computer's TensorFS store: {why}"));
    let root = tensorfs::home::resolve(None).map_err(|e| unusable(e.to_string()))?;
    let store = Arc::new(Store::open(root).map_err(|e| unusable(e.to_string()))?);
    let _ = platform::durable::set_mode(store.root(), 0o700);
    Ok(STORE.get_or_init(|| store))
}

fn object(digest: &Digest, length: u64) -> ObjectRef {
    ObjectRef { sha256: hex::encode(digest.bytes()), length }
}

fn refused(refusal: tensorfs::err::Refusal) -> Refusal {
    let (kind, code) = match refusal.code {
        Code::OBJECT_ID_MISMATCH => (Kind::Validation, "file_digest_mismatch"),
        Code::CAPACITY_EXHAUSTED => (Kind::Capacity, "disk_full"),
        _ => (Kind::Internal, "file_unwritable"),
    };
    Refusal(Error::new(kind, code, refusal.detail))
}

/// One file's upload, shared by the writes that carry its ranges.
struct Upload {
    storage: Arc<Store>,
    digest: Digest,
    length: u64,
    /// The store's partial, until the settle that names the file takes it.
    partial: Mutex<Option<Partial>>,
    /// One settle at a time.
    settling: Mutex<()>,
    /// Named or dropped: later writes land nowhere, and a new upload starts afresh.
    ended: AtomicBool,
}

/// How an upload stands.
enum Settled {
    Partly(Ranges),
    /// Named now.
    Whole,
    /// Another write named it first.
    Held,
}

impl Upload {
    fn held(&self) -> Ranges {
        self.partial.lock().expect("partial lock").as_ref().map_or_else(|| whole(self.length), Partial::held)
    }

    /// Whole, its bytes are verified and it is named.
    fn settle(&self) -> Result<Settled, Refusal> {
        let _settling = self.settling.lock().expect("settling lock");
        if self.ended.load(Ordering::Acquire) {
            return Ok(Settled::Held);
        }
        let held = self.held();
        if held != whole(self.length) {
            return Ok(Settled::Partly(held));
        }
        self.ended.store(true, Ordering::Release);
        let partial = self.partial.lock().expect("partial lock").take().expect("an upload has its partial until it ends");
        let object = object(&self.digest, self.length);
        let named = partial.commit().and_then(|()| self.storage.used(&object.sha256, [&object]).map_err(|e| e.refusal()));
        // Until it is named, a write or a look at the same file finds this upload and waits for this settle; only then does one
        // start afresh. A second sender answered before the first had named it, and an update then found no file (cl-864).
        UPLOADS.lock().expect("uploads lock").remove(&(self.storage.root().to_owned(), self.digest));
        named.map_err(refused)?;
        Ok(Settled::Whole)
    }
}

fn whole(length: u64) -> Ranges {
    match length {
        0 => vec![],
        length => vec![(0, length)],
    }
}

/// The file Calcifer holds under `digest`, if whole. Finding it is a use.
pub(crate) fn held(digest: &Digest) -> Option<PathBuf> {
    held_in(store().ok()?.as_ref(), digest)
}

/// `bytes` as a file Calcifer holds.
pub(crate) fn hold(bytes: &[u8]) -> Result<Digest, Error> {
    let unwritable = |e: tensorfs::store::Error| Error::new(Kind::Internal, "file_unwritable", e.to_string());
    let store = store()?;
    let object = store.put_bytes(bytes).map_err(unwritable)?;
    store.used(&object.sha256, [&object]).map_err(unwritable)?;
    Ok(Digest::of(bytes))
}

/// `digest`'s upload as `length` bytes: going, or begun from what the store kept of an earlier one.
fn open(storage: Arc<Store>, digest: Digest, length: u64) -> Result<Arc<Upload>, Refusal> {
    let mut uploads = UPLOADS.lock().expect("uploads lock");
    let key = (storage.root().to_owned(), digest);
    if let Some((was, upload)) = uploads.get(&key).and_then(|(was, upload)| Some((*was, upload.upgrade()?))) {
        if was != length {
            let said = format!("{digest} is being written as {was} bytes, not {length}");
            return Err(Refusal(Error::new(Kind::Conflict, "file_length_conflict", said)));
        }
        return Ok(upload);
    }
    let partial = storage.partial(&object(&digest, length)).map_err(|e| refused(e.refusal()))?;
    let upload = Arc::new(Upload { storage, digest, length, partial: Mutex::new(Some(partial)), settling: Mutex::default(), ended: AtomicBool::new(false) });
    uploads.insert(key, (length, Arc::downgrade(&upload)));
    Ok(upload)
}

/// `open` off the request's thread: the store may first make room.
async fn opened(storage: Arc<Store>, digest: Digest, length: u64) -> Result<Arc<Upload>, Refusal> {
    tokio::task::spawn_blocking(move || open(storage, digest, length)).await.map_err(|e| Refusal(Error::new(Kind::Internal, "file_unwritable", e.to_string())))?
}

/// A PUT's `[start, end)` of a file of `length` bytes: its `Content-Range`, else the whole file its `Content-Length` measures.
fn span(headers: &HeaderMap) -> Result<(u64, u64, u64), Refusal> {
    let text = |name| headers.get(name).and_then(|value: &HeaderValue| value.to_str().ok());
    let invalid = || Refusal(Error::new(Kind::Usage, "invalid_range", "a part is `Content-Range: bytes a-b/length`"));
    let Some(range) = text(header::CONTENT_RANGE) else {
        let length = text(header::CONTENT_LENGTH).and_then(|length| length.parse().ok());
        let length = length.ok_or_else(|| Refusal(Error::new(Kind::Usage, "file_length_required", "a whole file's PUT carries its Content-Length").remedy("or send it as parts, with `Content-Range: bytes a-b/length`")))?;
        return Ok((0, length, length));
    };
    let (span, length) = range.trim().strip_prefix("bytes ").and_then(|range| range.split_once('/')).ok_or_else(invalid)?;
    let (first, last) = span.split_once('-').ok_or_else(invalid)?;
    let number = |n: &str| n.trim().parse::<u64>().map_err(|_| invalid());
    let (start, end, length) = (number(first)?, number(last)?.saturating_add(1), number(length)?);
    match start < end && end <= length {
        true => Ok((start, end, length)),
        false => Err(invalid()),
    }
}

fn held_header(ranges: &[(u64, u64)]) -> (axum::http::HeaderName, HeaderValue) {
    (axum::http::HeaderName::from_static(HELD), HeaderValue::from_str(&spell_held(ranges)).expect("ranges are ASCII"))
}

/// The file `name`: a whole body or one part (`Content-Range`). A part leaves it unnamed: `204`, with what is held in `Cozy-Held`.
/// The write that completes it answers `201` once its bytes hash to its name, and one that finds it held `200`.
pub(crate) async fn project_put(State(api): State<Arc<Api>>, Query(scope): Query<UploadScope>, UrlPath(name): UrlPath<String>, headers: HeaderMap, body: Body) -> Result<Response, Refusal> {
    if scope.project.is_none() { return Err(Refusal(Error::new(Kind::Usage, "project_upload_scope", "a project upload must name its project"))); }
    let _admission = crate::project::vault::admit_project(&api, scope.project.as_deref().expect("checked scope")).await.map_err(Refusal)?;
    put_in(scoped(&api, &scope)?, name, headers, body).await
}

pub(crate) async fn put(UrlPath(name): UrlPath<String>, headers: HeaderMap, body: Body) -> Result<Response, Refusal> {
    put_in(store().map_err(Refusal)?.clone(), name, headers, body).await
}

async fn put_in(storage: Arc<Store>, name: String, headers: HeaderMap, body: Body) -> Result<Response, Refusal> {
    let digest = named(&name)?;
    let (start, end, length) = span(&headers)?;
    let answer = |status: StatusCode, path: &Path| -> Result<Response, Refusal> {
        let file = file(digest, path)?;
        Ok((status, [held_header(&whole(file.length))], Json(file)).into_response())
    };
    if let Some(path) = held_in(&storage, &digest) {
        let held = file(digest, &path)?;
        if held.length != length {
            return Err(Refusal(Error::new(Kind::Conflict, "file_length_conflict", format!("{digest} is held as {} bytes, not {length}", held.length))));
        }
        // An already-held object needs no write, but the caller still sends this body. Leaving
        // it unread makes HTTP/1 close the connection when the remainder is not buffered yet.
        let mut stream = body.into_data_stream();
        let mut at = start;
        while let Some(data) = stream.next().await {
            let data = data.map_err(|error| Refusal(Error::new(Kind::Validation, "body_unreadable", error.to_string())))?;
            if data.len() as u64 > end - at {
                return Err(Refusal(Error::new(Kind::Usage, "invalid_range", "the body is longer than its Content-Range")));
            }
            at += data.len() as u64;
        }
        if at != end {
            return Err(Refusal(Error::new(Kind::Usage, "invalid_range", "the body is shorter than its Content-Range")));
        }
        return answer(StatusCode::OK, &path);
    }
    let upload = opened(storage.clone(), digest, length).await?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Bytes>(8);
    let landing = upload.clone();
    let writer = tokio::task::spawn_blocking(move || -> Result<Settled, Refusal> {
        let (mut at, mut wrote) = (start, Ok(()));
        let mut writer = landing.partial.lock().expect("partial lock").as_ref().map(Partial::writer).transpose().map_err(refused)?;
        while let Some(data) = receiver.blocking_recv() {
            if at + data.len() as u64 > end {
                wrote = Err(Refusal(Error::new(Kind::Usage, "invalid_range", "the body is longer than its Content-Range")));
                break;
            }
            // An upload that ended meanwhile takes no more.
            let live = writer.as_mut().filter(|_| !landing.ended.load(Ordering::Acquire));
            if let Some(Err(e)) = live.map(|writer| writer.write_at(at, &data)) {
                wrote = Err(refused(e));
                break;
            }
            at += data.len() as u64;
        }
        drop(receiver);
        let settled = landing.settle()?;
        wrote.map(|()| settled)
    });
    let (mut stream, mut unread) = (body.into_data_stream(), None);
    while let Some(frame) = stream.next().await {
        match frame {
            // A refused writer drops its receiver; its refusal is the answer.
            Ok(data) => {
                if sender.send(data).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                unread = Some(Refusal(Error::new(Kind::Validation, "body_unreadable", e.to_string())));
                break;
            }
        }
    }
    drop(sender);
    let settled = writer.await.map_err(|e| Refusal(Error::new(Kind::Internal, "file_unwritable", e.to_string())))??;
    if let Some(refusal) = unread {
        return Err(refusal);
    }
    match settled {
        Settled::Partly(held) => Ok((StatusCode::NO_CONTENT, [held_header(&held)]).into_response()),
        Settled::Whole | Settled::Held => {
            let status = if matches!(settled, Settled::Whole) { StatusCode::CREATED } else { StatusCode::OK };
            let path = held_in(&storage, &digest).ok_or_else(|| Refusal(Error::new(Kind::Internal, "file_unwritable", format!("{digest} was named and is gone"))))?;
            answer(status, &path)
        }
    }
}

/// Whether Calcifer holds `name`: `200` with its length and media type, else `404`. `Cozy-Held` says which bytes either way.
pub(crate) async fn project_head(State(api): State<Arc<Api>>, Query(scope): Query<UploadScope>, UrlPath(name): UrlPath<String>) -> Result<Response, Refusal> {
    if scope.project.is_none() { return Err(Refusal(Error::new(Kind::Usage, "project_upload_scope", "a project upload must name its project"))); }
    let _admission = crate::project::vault::admit_project(&api, scope.project.as_deref().expect("checked scope")).await.map_err(Refusal)?;
    head_in(scoped(&api, &scope)?, name).await
}

pub(crate) async fn head(UrlPath(name): UrlPath<String>) -> Result<Response, Refusal> {
    head_in(store().map_err(Refusal)?.clone(), name).await
}

async fn head_in(storage: Arc<Store>, name: String) -> Result<Response, Refusal> {
    let digest = named(&name)?;
    // One that was cut: what the store kept of it.
    let begun = UPLOADS.lock().expect("uploads lock").get(&(storage.root().to_owned(), digest)).map(|(length, upload)| (*length, upload.upgrade()));
    let upload = match begun {
        Some((_, Some(upload))) => Some(upload),
        Some((length, None)) if held_in(&storage, &digest).is_none() => Some(opened(storage.clone(), digest, length).await?),
        _ => None,
    };
    let ranges = match upload {
        Some(upload) if upload.held() == whole(upload.length) => {
            // All landed: this waits for it to be verified, and says how that went.
            let settling = upload.clone();
            match tokio::task::spawn_blocking(move || settling.settle()).await.map_err(|e| Refusal(Error::new(Kind::Internal, "file_unwritable", e.to_string())))? {
                Ok(Settled::Partly(ranges)) => ranges,
                _ => vec![],
            }
        }
        Some(upload) => upload.held(),
        None => vec![],
    };
    let Some(path) = held_in(&storage, &digest) else {
        let refusal = Refusal(Error::new(Kind::NotFound, "file_not_found", format!("Calcifer holds no file {digest}")));
        return Ok((StatusCode::NOT_FOUND, [held_header(&ranges)], refusal).into_response());
    };
    let held = file(digest, &path)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(held.length));
    if let Ok(media_type) = HeaderValue::from_str(&held.media_type) {
        headers.insert(header::CONTENT_TYPE, media_type);
    }
    let (name, value) = held_header(&whole(held.length));
    headers.insert(name, value);
    Ok(headers.into_response())
}

fn named(name: &str) -> Result<Digest, Refusal> {
    name.parse().map_err(|why: &str| {
        Refusal(Error::new(Kind::Usage, "file_name_invalid", format!("{name:?}: {why}")).remedy("name a file sha256:<hex>"))
    })
}

fn file(digest: Digest, path: &Path) -> Result<File, Refusal> {
    let sniffed = super::bind::sniff(path).map_err(Refusal)?;
    Ok(File { digest, length: sniffed.length, media_type: sniffed.media_type })
}
