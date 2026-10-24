//! The distribution client (`spec/distribution.md`): one endpoint `B` serving
//! `B/blobs/<aa>/<bb>/<sha256>` and a model's commits and resolution, over HTTP(S) or a directory.
//!
//! - **Reads.** Objects as Fetcher locations, whose redirects the client follows without
//!   credentials across origins; [`Endpoint::resolve`] names a model's commit.
//! - **Uploads.** Per object HEAD → PUT (to the HEAD's `Location`, else here) → HEAD; `412` means
//!   present. [`upload_commit`] sends chunks, assets and repo files, then the header, then the
//!   tree, then records the commit as the model's ([`Endpoint::commit`]).
//! - **A directory** writes through a store over it: objects verified, temp and rename; its
//!   records under the store's record lock.
//! - **Weather** (transport failures, 408/425/429/5xx) is asked again at `Retry-After` or a
//!   doubling wait, until nothing has moved for the stall rule ([`super::ask`]).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use base64::engine::{general_purpose::STANDARD, Engine as _};
use super::net::Uri;
use serde_json::Value;

use super::ask::{self, Patience, Tried};
use super::net::{join, same_origin, Body, Client, GetError, HeaderMap};
use super::origin::{Location, Origin};
use super::policy::is_loopback_origin;
use super::token::{default_for, s3_base, Request, TokenProvider};
use crate::err::{refuse, Code, Refusal, Result};
use crate::format::{is_digest, repo_name, Header, ObjectRef, Repo, Tree};
use crate::sha256::hex_digest;
use crate::store::Store;

/// The most bytes a document or an answer may have.
const DOCUMENT_BYTES: usize = 16 << 20;
/// Objects an upload sends at once.
const STREAMS: usize = 16;

/// An endpoint of the distribution protocol.
pub struct Endpoint {
    url: String,
    at: At,
    client: Client,
    token: Option<Arc<dyn TokenProvider>>,
    patience: Patience,
    /// Bytes of file bodies sent, ever: an upload's progress while its objects are on their way.
    sent: Arc<AtomicU64>,
}

enum At {
    Http(String),
    Dir(PathBuf, OnceLock<Store>),
}

/// An object's bytes for an upload.
pub enum Payload {
    Bytes(Bytes),
    File(fs::File),
}

/// What [`upload_commit`] did: the commit, the closure's objects, and those it sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Uploaded {
    pub commit: ObjectRef,
    pub objects: usize,
    pub sent: usize,
    pub bytes: u64,
    /// The model already had the commit.
    pub converged: bool,
}

/// What a consumer runs on, for a resolve without a lane: each GPU's compute capability (`"8.9"`,
/// `"apple9"`; none: the CPU), the free bytes on the smallest of them, and whether it takes plain
/// weights only (code that runs no encoded tensor).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Hardware {
    pub gpus: Vec<String>,
    pub free_vram: Option<u64>,
    #[serde(default)]
    pub plain: bool,
}

/// A model's commit as resolved: its release (empty: none), lane, tree and header, and why a
/// better lane was passed over.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Resolved {
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub lane: String,
    pub commit: ObjectRef,
    pub header: ObjectRef,
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// An answer read whole.
struct Reply {
    url: String,
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }
}

/// One exchange: its method, URL, headers, body, and whether the token provider authorizes it.
struct Ask<'a> {
    method: &'a str,
    url: &'a str,
    headers: Vec<(String, String)>,
    body: Option<&'a (dyn Fn() -> Result<Payload> + Send + Sync)>,
    payload: Option<&'a str>,
    auth: bool,
}

impl Endpoint {
    /// The endpoint at `url`: `https://…`, `http://` on loopback, `s3://<bucket>[/<prefix>]`
    /// (SigV4), or a directory (`file:///dir` or a path). With no `token`, the default chain's
    /// provider for it, if any.
    pub fn new(client: &Client, url: &str, token: Option<Arc<dyn TokenProvider>>) -> Result<Endpoint> {
        let url = url.trim_end_matches('/').to_owned();
        let at = if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("s3://") {
            let base = match url.starts_with("s3://") {
                true => s3_base(&url)?,
                false => url.clone(),
            };
            if base.starts_with("http://") && !is_loopback_origin(&base) {
                return refuse(Code::SOURCE_NOT_ALLOWED, format!("{base}: plain http only on loopback"));
            }
            client.policy().admit(&base)?;
            At::Http(base)
        } else {
            let path = url.strip_prefix("file://").unwrap_or(&url);
            if !path.starts_with('/') {
                return refuse(Code::SOURCE_NOT_ALLOWED, format!("{url}: not an https://, loopback http:// or file:/// endpoint"));
            }
            At::Dir(PathBuf::from(path), OnceLock::new())
        };
        let token = match at {
            At::Http(_) => token.or_else(|| default_for(&url)),
            At::Dir(..) => None,
        };
        Ok(Endpoint { url, at, client: client.clone(), token, patience: Patience::default(), sent: Arc::default() })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Public reads only, even when the environment configures credentials for this host.
    pub fn anonymous(mut self) -> Self {
        self.token = None;
        self
    }

    /// The commit of model `name` at `release` (none: the newest) in `lane`; with no lane, the
    /// one `hardware` runs best (the Hub's choice; a directory's release has one lane, or its
    /// model no releases: its newest commit).
    pub async fn resolve(&self, name: &str, release: Option<&str>, lane: Option<&str>, hardware: &Hardware) -> Result<Resolved> {
        check_repo_name(name)?;
        let base = match &self.at {
            At::Http(base) => base,
            At::Dir(..) => {
                // The store's own record of the model: the one way refs are read and written.
                let Some(repo) = self.store()?.get_ref(name).map_err(crate::store::Error::refusal)? else {
                    return refuse(Code::REF_NOT_FOUND, format!("no model {name} at {}", self.url));
                };
                let unread = |e: crate::format::Error| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("{name}: {e}") };
                let (release, lane, commit) = repo.resolve(release, lane)
                    .map_err(|e| Refusal { code: Code::REF_NOT_FOUND, detail: e.to_string() })?;
                let tree = Tree::from_json(&self.document(&commit).await?).map_err(unread)?;
                let header = tree.header().map_err(unread)?.1.clone();
                return Ok(Resolved { release, lane: lane.unwrap_or(tree.lane), commit, header, reasons: vec![] });
            }
        };
        let mut url = format!("{}?", repo_url(base, &format!("v1/models/{}/resolve", name.to_ascii_lowercase())));
        let mut query = |key: &str, value: &str| url.push_str(&format!("{key}={}&", query_escape(value)));
        release.into_iter().for_each(|r| query("release", r));
        lane.into_iter().for_each(|l| query("lane", l));
        if !hardware.gpus.is_empty() {
            query("gpus", &hardware.gpus.join(","));
            hardware.free_vram.into_iter().for_each(|free| query("free_vram", &free.to_string()));
        }
        if hardware.plain {
            query("plain", "true");
        }
        url.pop();
        let reply = self.exchange(Ask { method: "GET", url: &url, headers: vec![], body: None, payload: None, auth: true }).await?;
        match reply.status {
            200 => serde_json::from_slice(&reply.body).map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("GET {url}: {e}") }),
            _ => Err(refusal(&reply, "GET")),
        }
    }

    /// Records the uploaded tree `commit` (its closure already here) as one of model `name`'s
    /// commits: true when the model had it already.
    pub async fn commit(&self, name: &str, commit: &ObjectRef) -> Result<bool> {
        check_repo_name(name)?;
        let base = match &self.at {
            At::Http(base) => base,
            At::Dir(..) => {
                let (store, name, commit) = (self.store()?.clone(), name.to_owned(), commit.clone());
                let record = tokio::task::spawn_blocking(move || {
                    let tree = store.tree(&commit)?;
                    tree.header().map_err(crate::store::Error::Format)?;
                    let mut had = false;
                    store.update_ref(&name, |old| {
                        let mut repo = old.unwrap_or_else(|| Repo::new(&name));
                        had = repo.commits.last() == Some(&commit);
                        repo.add_commit(commit.clone());
                        Some(repo)
                    })?;
                    Ok::<_, crate::store::Error>(had)
                });
                let record = record.await.map_err(|e| Refusal { code: Code::IO_FAILED, detail: e.to_string() })?;
                return record.map_err(crate::store::Error::refusal);
            }
        };
        let url = repo_url(base, &format!("v1/models/{}/commits", name.to_ascii_lowercase()));
        let body = Bytes::from(serde_json::to_vec(&serde_json::json!({"commit": commit})).expect("JSON"));
        let payload = hex_digest(&body);
        let make = move || Ok(Payload::Bytes(body.clone()));
        let headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        let reply = self.exchange(Ask { method: "POST", url: &url, headers, body: Some(&make), payload: Some(&payload), auth: true }).await?;
        match reply.status {
            200 => Ok(true),
            201 => Ok(false),
            _ => Err(refusal(&reply, "POST")),
        }
    }

    /// An object named by digest, including its length, or `None` when absent or unreadable.
    pub async fn object(&self, sha256: &str) -> Result<Option<ObjectRef>> {
        let sha256 = sha256.strip_prefix("sha256:").unwrap_or(sha256);
        if !is_digest(sha256) {
            return refuse(Code::MALFORMED_DIGEST, "an object digest must be 64 lowercase hex digits");
        }
        let mut object = ObjectRef { sha256: sha256.to_owned(), length: 0 };
        object.length = match &self.at {
            At::Dir(root, _) => match fs::metadata(root.join(object.key())) {
                Ok(metadata) => metadata.len(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(crate::err::classify_io(self.url(), error)),
            },
            At::Http(base) => {
                let url = format!("{base}/{}", object.key());
                let reply = self.exchange(Ask { method: "HEAD", url: &url, headers: vec![], body: None, payload: None, auth: true }).await?;
                match reply.status {
                    200 => reply.header("content-length").and_then(|length| length.parse().ok())
                        .ok_or_else(|| Refusal { code: Code::LENGTH_MISMATCH, detail: format!("HEAD {url}: no valid Content-Length") })?,
                    404 | 410 => return Ok(None),
                    _ => return Err(refusal(&reply, "HEAD")),
                }
            }
        };
        Ok(Some(object))
    }

    /// A small object (a tree) read whole, following redirects, and verified.
    pub async fn document(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        if object.length > DOCUMENT_BYTES as u64 {
            return refuse(Code::SIZE_CAP, format!("{}: {} B is not a document", object.id(), object.length));
        }
        let bytes = match &self.at {
            At::Dir(root, _) => {
                let path = root.join(object.key());
                fs::read(&path).map_err(|e| crate::err::classify_io(path.display().to_string(), e))?
            }
            At::Http(base) => {
                let url = format!("{base}/{}", object.key());
                let mut body = Vec::new();
                let mut sink = |at: u64, frame: &[u8]| {
                    body.truncate(at as usize);
                    body.extend_from_slice(frame);
                    match body.len() as u64 > object.length {
                        true => refuse(Code::OBJECT_ID_MISMATCH, format!("GET {url}: more than {} B", object.length)),
                        false => Ok(()),
                    }
                };
                let got = ask::get(&self.client, &url, &[], self.token.as_deref(), &self.patience, None, &mut sink).await;
                match got.map_err(GetError::refusal)? {
                    Ok(_) => body,
                    Err(status @ (404 | 410)) => return refuse(Code::OBJECT_ABSENT, format!("GET {url}: {status}")),
                    Err(status) => return refuse(Code::HUB_REFUSED, format!("GET {url}: {status}")),
                }
            }
        };
        match ObjectRef::of(&bytes) == *object {
            true => Ok(bytes),
            false => refuse(Code::OBJECT_ID_MISMATCH, format!("{} does not match its bytes", object.id())),
        }
    }

    /// Whether the endpoint holds `object`.
    pub async fn has(&self, object: &ObjectRef) -> Result<bool> {
        match &self.at {
            At::Dir(..) => self.store()?.reuse(object).map_err(crate::store::Error::refusal),
            At::Http(base) => {
                let url = format!("{base}/{}", object.key());
                let reply = self.exchange(Ask { method: "HEAD", url: &url, headers: vec![], body: None, payload: None, auth: true }).await?;
                match reply.status {
                    200 => held(&reply, object).map(|()| true),
                    404 | 410 => Ok(false),
                    _ => Err(refusal(&reply, "HEAD")),
                }
            }
        }
    }

    /// Sends `object` unless the endpoint holds it: true when it sent it. `payload` gives its
    /// bytes, only when they are needed (once per attempt).
    pub async fn upload(&self, object: &ObjectRef, payload: &(dyn Fn() -> Result<Payload> + Send + Sync)) -> Result<bool> {
        let base = match &self.at {
            At::Http(base) => base,
            At::Dir(..) => {
                if self.has(object).await? {
                    return Ok(false);
                }
                let (store, payload, object) = (self.store()?.clone(), payload()?, object.clone());
                let put = tokio::task::spawn_blocking(move || match payload {
                    Payload::Bytes(bytes) => store.put(&bytes[..], Some(&object)),
                    Payload::File(file) => store.put(file, Some(&object)),
                });
                let put = put.await.map_err(|e| Refusal { code: Code::IO_FAILED, detail: e.to_string() })?;
                put.map_err(crate::store::Error::refusal)?;
                return Ok(true);
            }
        };
        let url = format!("{base}/{}", object.key());
        let head = || Ask { method: "HEAD", url: &url, headers: vec![], body: None, payload: None, auth: true };
        let mut reply = self.exchange(head()).await?;
        // Places the bytes went, without their signatures. A confirming HEAD (where a redirecting
        // server verifies what it was sent) may name another place, as when another account's
        // copy landed first and this writer proves it holds the bytes: they go there too.
        let mut sent: Vec<String> = vec![];
        loop {
            match reply.status {
                200 => return held(&reply, object).map(|()| !sent.is_empty()),
                404 | 410 => {}
                _ => return Err(refusal(&reply, if sent.is_empty() { "HEAD" } else { "the confirming HEAD" })),
            }
            // A writer's HEAD of an absent object may say where to PUT it (a presigned URL).
            let target = match reply.header("location") {
                Some(at) => join(&url.parse::<Uri>().expect("sent"), at),
                None => url.clone(),
            };
            let place = target.split('?').next().unwrap_or_default().to_owned();
            if sent.contains(&place) {
                return Err(refusal(&reply, "the confirming HEAD"));
            }
            let checksum = STANDARD.encode(decode_hex(&object.sha256));
            let headers = vec![
                ("content-length".to_owned(), object.length.to_string()),
                ("x-amz-checksum-sha256".to_owned(), checksum),
                ("if-none-match".to_owned(), "*".to_owned()),
            ];
            let auth = same_origin(&target, &url);
            let ask = Ask { method: "PUT", url: &target, headers, body: Some(payload), payload: Some(&object.sha256), auth };
            let put = self.exchange(ask).await?;
            if !matches!(put.status, 200 | 201 | 204 | 412) {
                return Err(refusal(&put, "PUT"));
            }
            sent.push(place);
            reply = self.exchange(head()).await?;
        }
    }

    /// One authorized exchange at `key` under an HTTP endpoint (`None` for a directory): its
    /// status, headers and body. What the conformance suite checks servers with.
    pub(crate) async fn raw(&self, method: &str, key: &str, headers: Vec<(String, String)>, body: Option<Bytes>) -> Result<Option<(u16, HeaderMap, Vec<u8>)>> {
        let At::Http(base) = &self.at else { return Ok(None) };
        let url = format!("{base}/{key}");
        let payload = body.as_ref().map(|b| hex_digest(b));
        let make = body.map(|b| move || Ok(Payload::Bytes(b.clone())));
        let body = make.as_ref().map(|m| m as &(dyn Fn() -> Result<Payload> + Send + Sync));
        let reply = self.exchange(Ask { method, url: &url, headers, body, payload: payload.as_deref(), auth: true }).await?;
        Ok(Some((reply.status, reply.headers, reply.body)))
    }

    /// The store over a directory endpoint, which its writes go through.
    fn store(&self) -> Result<&Store> {
        let At::Dir(root, store) = &self.at else { unreachable!("a directory endpoint") };
        if let Some(store) = store.get() {
            return Ok(store);
        }
        // An endpoint's store is an origin: what it holds is published, never cache to evict.
        let opened = Store::open(root).map_err(crate::store::Error::refusal)?.origin();
        Ok(store.get_or_init(|| opened))
    }

    /// The provider's headers for `method url`, carrying `headers` with `payload`.
    async fn authorize(&self, method: &str, url: &str, headers: &[(String, String)], payload: Option<&str>) -> Result<Vec<(String, String)>> {
        match &self.token {
            Some(token) => token.headers(&Request { method, url, headers, payload }).await,
            None => Ok(vec![]),
        }
    }

    /// One exchange, its answer read whole, asked again by the one rule ([`ask::again`]); the
    /// provider may ask once more (a nonce, a refresh).
    async fn exchange(&self, ask: Ask<'_>) -> Result<Reply> {
        self.exchange_attempt(ask).await.map_err(GetError::refusal)
    }

    async fn exchange_attempt(&self, ask: Ask<'_>) -> std::result::Result<Reply, GetError> {
        let (ask, challenged, patience) = (&ask, &AtomicBool::new(false), &self.patience);
        ask::again(patience, move |watch| async move {
            let mut headers = ask.headers.clone();
            if ask.auth {
                // A provider's failure is the caller's verdict, not failed network traffic.
                let authorized = ask::watched(patience, &watch, self.authorize(ask.method, ask.url, &ask.headers, ask.payload));
                headers.extend(authorized.await.map_err(GetError::Caller)?);
            }
            let body = match ask.body {
                Some(make) => Some(match make()? {
                    Payload::Bytes(bytes) => Body::Bytes(bytes),
                    Payload::File(file) => {
                        let length = file.metadata().map_err(|e| crate::err::classify_io(ask.url, e))?.len();
                        Body::File { file, length, sent: [Arc::clone(&watch.moved), Arc::clone(&self.sent)] }
                    }
                }),
                None => None,
            };
            let reply = ask::watched(patience, &watch, self.round_trip(ask.method, ask.url, &headers, body, &watch.moved)).await?;
            let sent = Request { method: ask.method, url: ask.url, headers: &headers, payload: ask.payload };
            let again = match (ask.auth, &self.token) {
                (true, Some(token)) => token.retry(&sent, reply.status, &reply.headers).map_err(GetError::Caller)?,
                _ => false,
            };
            let why = format!("{} {}: {}", ask.method, ask.url, reply.status);
            Ok(if again && !challenged.swap(true, Ordering::Relaxed) {
                Tried::Again { why, wait: Some(Duration::ZERO) }
            } else if ask::weather(reply.status) {
                let wait = (reply.header("retry-after")).and_then(|s| s.trim().parse().ok()).map(Duration::from_secs);
                Tried::Again { why, wait }
            } else {
                Tried::Done(reply)
            })
        })
        .await
    }

    async fn round_trip(&self, method: &str, url: &str, headers: &[(String, String)], body: Option<Body>, moved: &AtomicU64) -> Result<Reply> {
        let mut answer = self.client.send(method, url, headers, body).await?;
        let (status, headers) = (answer.status(), answer.response.headers().clone());
        moved.fetch_add(1, Ordering::Relaxed);
        let mut body = Vec::new();
        while let Some(frame) = answer.frame().await? {
            body.extend_from_slice(&frame);
            moved.fetch_add(frame.len() as u64, Ordering::Relaxed);
            if body.len() > DOCUMENT_BYTES {
                return refuse(Code::SIZE_CAP, format!("{method} {url}: an answer over {DOCUMENT_BYTES} B"));
            }
        }
        Ok(Reply { url: url.to_owned(), status, headers, body })
    }
}

#[async_trait]
impl Origin for Endpoint {
    async fn locate(&self, objects: &[ObjectRef]) -> Result<Vec<Location>> {
        let mut out = Vec::with_capacity(objects.len());
        for object in objects {
            out.push(match &self.at {
                At::Dir(root, _) => Location::File(root.join(object.key())),
                At::Http(base) => {
                    let url = format!("{base}/{}", object.key());
                    match &self.token {
                        Some(provider) => Location::Authenticated { url, provider: Arc::clone(provider) },
                        None => Location::Url { url, headers: vec![], expires: None },
                    }
                }
            });
        }
        Ok(out)
    }

    fn public(&self) -> bool {
        self.token.is_none()
    }
}

/// Cooperative control of one upload. `commit` is called once after the objects land, before
/// the commit is recorded; once it accepts, cancellation no longer interrupts. Progress counts
/// verified bytes, including objects the endpoint already holds.
pub struct UploadControl<'a> {
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    pub progress: &'a (dyn Fn(u64, u64) + Sync),
    pub commit: &'a (dyn Fn() -> bool + Sync),
}

/// Sends what `endpoint` lacks of commit `tree`'s closure in `store` (chunks, assets and repo
/// files, then the header, then the tree), then records it as model `name`'s
/// ([`Endpoint::commit`]).
pub async fn upload_commit(endpoint: &Endpoint, name: &str, tree: &ObjectRef, store: &Store, control: &UploadControl<'_>) -> Result<Uploaded> {
    let stopped = || Refusal { code: Code::TRANSFER_FAILED, detail: "upload stopped before its commit; admitted objects remain available".into() };
    if (control.cancelled)() {
        return Err(stopped());
    }
    let refusal = crate::store::Error::refusal;
    let parsed = store.tree(tree).map_err(refusal)?;
    let unread = |e: crate::format::Error| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("{}: {e}", tree.id()) };
    let header = parsed.header().map_err(unread)?.1.clone();
    let mut leaves: Vec<ObjectRef> = Header::from_cbor(&store.read(&header).map_err(refusal)?).map_err(unread)?.objects().cloned().collect();
    // The header, then the tree: a tree present implies its closure was sent.
    let documents = [header, tree.clone()];
    leaves.extend(parsed.repo_files().map(|(_, file)| file.clone()));
    leaves.sort();
    leaves.dedup();
    let (mut sent, mut bytes) = (0, 0);
    let objects = leaves.len() + documents.len();
    let total = leaves.iter().chain(&documents)
        .try_fold(0u64, |sum, object| sum.checked_add(object.length))
        .ok_or_else(|| Refusal { code: Code::ARITH_OVERFLOW, detail: "upload closure byte count overflows".into() })?;
    // Done: the objects it holds or that landed, and the bytes on their way meanwhile, so a slow
    // upload is never silent between objects.
    let (mut verified, mut landed, before) = (0, 0, endpoint.sent.load(Ordering::Relaxed));
    let moving = |verified: u64, landed: u64| {
        let sending = endpoint.sent.load(Ordering::Relaxed).saturating_sub(before).saturating_sub(landed);
        (control.progress)(verified.saturating_add(sending).min(total), total);
    };
    (control.progress)(0, total);
    let [header, tree_object] = documents;
    for stage in [leaves, vec![header], vec![tree_object]] {
        let mut sending = futures::stream::iter(stage)
            .map(|object| async move {
                let (from, at) = (store.clone(), object.clone());
                let payload = move || from.file(&at).map(Payload::File).map_err(crate::store::Error::refusal);
                endpoint.upload(&object, &payload).await.map(|s| (s, object.length))
            })
            .buffer_unordered(STREAMS);
        loop {
            if (control.cancelled)() {
                return Err(stopped());
            }
            let done = tokio::select! {
                done = sending.next() => done,
                () = tokio::time::sleep(ask::TICK) => {
                    moving(verified, landed);
                    continue;
                }
            };
            let Some(done) = done else { break };
            let (pushed, length) = done?;
            if pushed {
                (sent, bytes, landed) = (sent + 1, bytes + length, landed + length);
            }
            verified += length;
            moving(verified, landed);
        }
    }
    if (control.cancelled)() || !(control.commit)() {
        return Err(stopped());
    }
    let converged = endpoint.commit(name, tree).await?;
    Ok(Uploaded { commit: tree.clone(), objects, sent, bytes, converged })
}

/// A writer's objects on their way to an endpoint (`Writer(repo_id=…)`): each is sent (HEAD →
/// PUT → HEAD) as soon as it is sealed while the writer goes on, at most [`IN_FLIGHT`] at once;
/// at commit the header, then the tree, then the commit recorded.
/// File-backed objects queue without blocking the caller's runtime. Byte-backed puts and
/// finalization block, so the finalizer runs outside the runtime that sends.
pub struct Stream {
    endpoint: Arc<Endpoint>,
    runtime: tokio::runtime::Handle,
    sent: Mutex<HashSet<ObjectRef>>,
    sending: Mutex<Vec<tokio::task::JoinHandle<Result<()>>>>,
    failed: Mutex<Option<Refusal>>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Stream({})", self.endpoint.url)
    }
}

/// Concurrent transfers; byte-backed objects are also bounded by this limit.
const IN_FLIGHT: usize = 4;

impl Stream {
    pub fn new(endpoint: Arc<Endpoint>, runtime: tokio::runtime::Handle) -> Stream {
        Stream {
            endpoint,
            runtime,
            sent: Mutex::default(),
            sending: Mutex::default(),
            failed: Mutex::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT)),
        }
    }

    /// Whether `object` was given to [`Stream::put`].
    pub fn has(&self, object: &ObjectRef) -> bool {
        self.sent.lock().unwrap().contains(object)
    }

    /// Sends `bytes` as an object, once; its ref at once.
    pub fn put(&self, bytes: Vec<u8>) -> Result<ObjectRef> {
        let object = ObjectRef::of(&bytes);
        let bytes = Bytes::from(bytes);
        self.send(&object, move || Ok(Payload::Bytes(bytes.clone())))?;
        Ok(object)
    }

    /// Queues an admitted object without copying its bytes into an upload buffer.
    pub(crate) fn put_stored(&self, store: &Store, object: &ObjectRef) -> Result<()> {
        if !self.sent.lock().unwrap().insert(object.clone()) {
            return Ok(());
        }
        let (store, at) = (store.clone(), object.clone());
        let (endpoint, permits) = (Arc::clone(&self.endpoint), Arc::clone(&self.permits));
        let task = self.runtime.spawn(async move {
            let _permit = permits.acquire_owned().await.expect("never closed");
            let make = || store.file(&at).map(Payload::File).map_err(crate::store::Error::refusal);
            endpoint.upload(&at, &make).await.map(drop)
        });
        self.sending.lock().unwrap().push(task);
        Ok(())
    }

    fn send(&self, object: &ObjectRef, make: impl Fn() -> Result<Payload> + Send + Sync + 'static) -> Result<()> {
        if !self.sent.lock().unwrap().insert(object.clone()) {
            return Ok(());
        }
        let permit = self.runtime.block_on(Arc::clone(&self.permits).acquire_owned()).expect("never closed");
        let (endpoint, at) = (Arc::clone(&self.endpoint), object.clone());
        let task = self.runtime.spawn(async move {
            let _permit = permit;
            endpoint.upload(&at, &make).await.map(drop)
        });
        self.sending.lock().unwrap().push(task);
        Ok(())
    }

    /// Waits for every object put so far: the first failure.
    pub fn flush(&self) -> Result<()> {
        let tasks = std::mem::take(&mut *self.sending.lock().unwrap());
        let mut first = self.failed.lock().unwrap().clone().map_or(Ok(()), Err);
        for task in tasks {
            let done = match self.runtime.block_on(task) {
                Ok(done) => done,
                Err(e) => Err(Refusal { code: Code::TRANSFER_FAILED, detail: e.to_string() }),
            };
            if first.is_ok() {
                first = done;
            }
        }
        if let Err(ref failure) = first {
            *self.failed.lock().unwrap() = Some(failure.clone());
        }
        first
    }

    /// A writer's commit: its `header`, sent after what it cites, in lane `lane` beside `files`,
    /// recorded as one of model `name`'s commits: its tree.
    pub fn commit(&self, name: &str, lane: &str, header: Vec<u8>, files: &BTreeMap<String, ObjectRef>) -> Result<(ObjectRef, Tree)> {
        self.flush()?;
        let header = self.put(header)?;
        let tree = Tree::of(lane, header, files).map_err(|e| Refusal { code: Code::KEY_GRAMMAR, detail: e.to_string() })?;
        let commit = self.put(tree.to_json().map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: e.to_string() })?)?;
        self.flush()?;
        self.runtime.block_on(self.endpoint.commit(name, &commit))?;
        Ok((commit, tree))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        for task in self.sending.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).drain(..) {
            task.abort();
        }
    }
}

// Generic endpoints use format segments, independent of a Hub's catalog name policy.
fn check_repo_name(name: &str) -> Result<()> {
    match repo_name(name) {
        true => Ok(()),
        false => refuse(Code::KEY_GRAMMAR, format!("{name:?} is not <org>/<name>")),
    }
}

// Escapes a route's segments: keeping only the separators and URI-unreserved bytes avoids
// interpreting literal percent signs, query/fragment delimiters or UTF-8 bytes as URL structure.
fn repo_url(base: &str, key: &str) -> String {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut url = format!("{base}/");
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            url.push(char::from(byte));
        } else {
            url.push('%');
            url.push(char::from(HEX[usize::from(byte >> 4)]));
            url.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    url
}

fn held(reply: &Reply, object: &ObjectRef) -> Result<()> {
    match reply.header("content-length").and_then(|v| v.parse::<u64>().ok()) {
        Some(length) if length != object.length => refuse(
            Code::LENGTH_MISMATCH,
            format!("{}: {length} B, not {}", reply.url, object.length),
        ),
        _ => Ok(()),
    }
}

/// A query value, percent-encoded but for URI-unreserved bytes.
fn query_escape(value: &str) -> String {
    value.bytes().map(|b| match b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
        true => char::from(b).to_string(),
        false => format!("%{b:02X}"),
    }).collect()
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap_or(0)).collect()
}

/// The protocol's error envelope: `{"error": {"code", "message", "detail"?}}`.
fn envelope(body: &[u8]) -> Option<(String, String, Value)> {
    let parsed: Value = serde_json::from_slice(body).ok()?;
    let error = parsed.get("error")?;
    let code = error.get("code")?.as_str()?.to_owned();
    let message = error.get("message").and_then(Value::as_str).unwrap_or_default().to_owned();
    Some((code, message, error.get("detail").cloned().unwrap_or(Value::Null)))
}

/// A refusal for `reply`: the envelope's code when there is one, else the status's.
fn refusal(reply: &Reply, what: &str) -> Refusal {
    let (named, message, detail) = envelope(&reply.body).unwrap_or_else(|| {
        let text = String::from_utf8_lossy(&reply.body[..reply.body.len().min(200)]).trim().to_owned();
        (String::new(), text, Value::Null)
    });
    let code = match (named.as_str(), reply.status) {
        ("digest_mismatch", _) => Code::OBJECT_ID_MISMATCH,
        ("closure_incomplete", _) => Code::CHECKPOINT_CONFLICT,
        ("policy", _) if detail.get("reason").and_then(Value::as_str) == Some("capability_exceeded") => Code::CREDENTIAL_REQUIRED,
        ("precondition_failed" | "precondition_required" | "policy", _) | (_, 409 | 412 | 428) => Code::REPOSITORY_CONFLICT,
        ("not_found", _) | (_, 404 | 410) => Code::REF_NOT_FOUND,
        ("unauthorized" | "denied", _) | (_, 401 | 403) => Code::CREDENTIAL_REQUIRED,
        ("too_large", _) | (_, 413) => Code::SIZE_CAP,
        ("rate_limited" | "unavailable", _) => Code::HUB_UNREACHABLE,
        _ => Code::HUB_REFUSED,
    };
    let named = if named.is_empty() { String::new() } else { format!(" {named}") };
    let detail = if detail.is_null() { String::new() } else { format!(" {detail}") };
    Refusal { code, detail: format!("{what} {}: {}{named}: {message}{detail}", reply.url, reply.status) }
}
