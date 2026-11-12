//! The distribution client (`spec/distribution.md`): one endpoint `B` serving
//! `B/blobs/<aa>/<bb>/<sha256>` and a model's commits and resolution, over HTTP(S) or a directory.
//!
//! - **Reads.** Objects as Fetcher locations, whose redirects the client follows without
//!   credentials across origins; [`Endpoint::resolve`] names a model's commit.
//! - **Uploads.** Per object HEAD → PUT (to the HEAD's `Location`, else here) → HEAD; `412` means
//!   present. [`upload_checkpoint`] sends chunks, assets and repo files, then the header, then the
//!   tree, then records the commit as the model's ([`Endpoint::commit`]).
//! - **A directory** writes through a store over it: objects verified, temp and rename; its
//!   records under the store's record lock.
//! - **Weather** (transport failures, 408/425/429/5xx) is asked again by the one retry rule
//!   ([`super::ask`]).

use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use bytes::Bytes;
use futures_util::StreamExt;
use base64::engine::{general_purpose::STANDARD, Engine as _};
use serde_json::Value;

use super::ask::{self, Backoff};
use super::net::{join, same_origin, shown, Body, Client, GetError, HeaderMap, Pulse};
use super::origin::{Boxed, Location, Origin};
use super::policy::is_loopback_origin;
use super::token::{default_for, s3_base, Request, TokenProvider};
use crate::err::{refuse, Code, Refusal, Result};
use crate::format::{is_digest, model_name, Header, ObjectRef, Model, Tree};
use crate::sha256::hex_digest;
use crate::store::{self, Checkpoint, Store};
use crate::writer::{Upload, Writer};

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
    /// Bytes of file bodies that landed or are on their way ([`Endpoint::sent`]): an upload's
    /// progress while its objects are on their way.
    sent: Arc<AtomicU64>,
    /// When any of its requests last sent a byte: a request is cut for silence only when all of them are
    /// silent, so the streams of one upload on a slow link never cut each other's starved ones.
    pulse: Arc<Pulse>,
}

enum At {
    Http(String),
    Dir(PathBuf, OnceLock<Store>),
}

/// An object's bytes for an upload.
pub enum Payload {
    Bytes(Bytes),
    File(fs::File),
    /// The next `.1` bytes of a file, from where it is positioned.
    Part(fs::File, u64),
}

/// What [`upload_checkpoint`] did: the commit, the closure's objects, and those it sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Uploaded {
    pub checkpoint: ObjectRef,
    pub objects: usize,
    pub sent: usize,
    pub bytes: u64,
    /// The model already had the commit.
    pub converged: bool,
}

/// What a consumer runs on, for a resolve without a lane: each GPU's compute capability (`"8.9"`,
/// `"apple9"`; none: the CPU), the free bytes on the smallest of them, whether it takes plain
/// weights only (code that runs no encoded tensor), and the components it reads (none: all), which
/// are all of a lane its fit counts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Hardware {
    pub gpus: Vec<String>,
    pub free_vram: Option<u64>,
    #[serde(default)]
    pub plain: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
}

/// A model's commit as resolved: its release (empty: none), lane, tree and header, and why a
/// better lane was passed over.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Resolved {
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub lane: String,
    #[serde(rename = "commit", alias = "checkpoint")]
    pub checkpoint: ObjectRef,
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
    /// The headers it was asked with, the provider's included.
    presented: Vec<(String, String)>,
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
        Ok(Endpoint { url, at, client: client.clone(), token, sent: Arc::default(), pulse: Pulse::new() })
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
        check_model_name(name)?;
        let base = match &self.at {
            At::Http(base) => base,
            At::Dir(..) => {
                // The store's own record of the model: the one way refs are read and written.
                let Some(model) = self.store()?.get_ref(name).map_err(crate::store::Error::refusal)? else {
                    return refuse(Code::REF_NOT_FOUND, format!("no model {name} at {}", self.url));
                };
                let unread = |e: crate::format::Error| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("{name}: {e}") };
                let (release, lane, commit) = model.resolve(release, lane)
                    .map_err(|e| Refusal { code: Code::REF_NOT_FOUND, detail: e.to_string() })?;
                let tree = Tree::from_json(&self.document(&commit).await?).map_err(unread)?;
                let header = tree.header().map_err(unread)?.1.clone();
                return Ok(Resolved { release, lane: lane.unwrap_or(tree.lane), checkpoint: commit, header, reasons: vec![] });
            }
        };
        let mut url = format!("{}?", repo_url(base, &format!("v1/models/{}/resolve", name.to_ascii_lowercase())));
        let mut query = |key: &str, value: &str| url.push_str(&format!("{key}={}&", query_escape(value)));
        release.into_iter().for_each(|r| query("release", r));
        lane.into_iter().for_each(|l| query("lane", l));
        if !hardware.gpus.is_empty() {
            query("gpus", &hardware.gpus.join(","));
            hardware.free_vram.into_iter().for_each(|free| query("free_vram", &free.to_string()));
            if !hardware.components.is_empty() {
                query("components", &hardware.components.join(","));
            }
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
        check_model_name(name)?;
        let base = match &self.at {
            At::Http(base) => base,
            At::Dir(..) => {
                let (store, name, commit) = (self.store()?.clone(), name.to_owned(), commit.clone());
                let record = tokio::task::spawn_blocking(move || {
                    let tree = store.tree(&commit)?;
                    tree.header().map_err(crate::store::Error::Format)?;
                    let mut had = false;
                    store.update_ref(&name, |old| {
                        let mut model = old.unwrap_or_else(|| Model::new(&name));
                        had = model.checkpoints.last() == Some(&commit);
                        model.add_checkpoint(commit.clone());
                        Some(model)
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
    pub(crate) async fn document(&self, object: &ObjectRef) -> Result<Vec<u8>> {
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
                let got = ask::get(&self.client, &url, &[], self.token.as_deref(), None, &mut sink).await;
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

    /// `object` from byte `from` into `sink`, for a caller that holds it to its digest: each body frame
    /// with the byte it starts at (0 again when a server restarts the body), asked again from the byte
    /// reached.
    pub async fn read(&self, object: &ObjectRef, from: u64, sink: &mut (dyn FnMut(u64, &[u8]) -> Result<()> + Send + '_)) -> Result<()> {
        let At::Http(base) = &self.at else { return refuse(Code::SOURCE_NOT_ALLOWED, format!("{}: not an HTTP endpoint", self.url)) };
        let url = format!("{base}/{}", object.key());
        let range = (from > 0).then_some((from, None));
        let got = ask::get(&self.client, &url, &[], self.token.as_deref(), range, sink).await;
        match got.map_err(GetError::refusal)? {
            Ok(_) => Ok(()),
            Err(status @ (404 | 410)) => refuse(Code::OBJECT_ABSENT, format!("GET {url}: {status}")),
            Err(status) => refuse(Code::HUB_REFUSED, format!("GET {url}: {status}")),
        }
    }

    /// The bytes of file bodies sent that landed or are on their way: a failed attempt's are given back.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Whether the endpoint holds `object`.
    pub(crate) async fn has(&self, object: &ObjectRef) -> Result<bool> {
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
        self.upload_noted(object, payload, &|_| {}).await
    }

    /// [`Endpoint::upload`], each attempt asked again said to `note` (`<why>; attempt 2/10 in 3 s`).
    ///
    /// A HEAD of an absent object may answer where to PUT it: a presigned URL, a grant with a
    /// lifetime. Each PUT spends the grant the HEAD just before it minted, and a PUT asked again asks
    /// for a new one, so no transfer, however slow or retried, outlives its grant; storage refusing
    /// the grant (R2's `403 ExpiredRequest`) is asked again the same way.
    pub(crate) async fn upload_noted(&self, object: &ObjectRef, payload: &(dyn Fn() -> Result<Payload> + Send + Sync), note: &(dyn Fn(&str) + Sync)) -> Result<bool> {
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
                    Payload::Part(file, length) => store.put(file.take(length), Some(&object)),
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
        let mut backoff = Backoff::transfer();
        loop {
            match reply.status {
                200 => return held(&reply, object).map(|()| !sent.is_empty()),
                404 | 410 => {}
                _ => return Err(refusal(&reply, if sent.is_empty() { "HEAD" } else { "the confirming HEAD" })),
            }
            let target = reply.header("location").and_then(|at| join(&url, at)).unwrap_or_else(|| url.clone());
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
            // A presigned target carries its own authority; the endpoint's credential stays with it.
            let auth = same_origin(&target, &url);
            let ask = Ask { method: "PUT", url: &target, headers, body: Some(payload), payload: Some(&object.sha256), auth };
            let (why, wait) = match self.challenged(&ask).await {
                Ok(put) if matches!(put.status, 200 | 201 | 204 | 412) => {
                    sent.push(place);
                    reply = self.exchange(head()).await?;
                    continue;
                }
                Ok(put) if ask::weather(put.status) || !auth && matches!(put.status, 401 | 403) => {
                    let wait = put.header("retry-after").and_then(|s| s.trim().parse().ok()).map(std::time::Duration::from_secs);
                    (refusal(&put, "PUT"), wait)
                }
                Ok(put) => return Err(refusal(&put, "PUT")),
                Err(GetError::Exchange(refusal)) if ask::transient(&refusal) => (refusal, None),
                Err(error) => return Err(error.refusal()),
            };
            // Bytes a failed PUT sent are not kept: it never moved the upload on.
            let Some((pause, line)) = backoff.again(&why.detail, false, wait) else {
                return Err(Refusal { code: why.code, detail: backoff.spent(&why.detail) });
            };
            tracing::info!(object = %object.id(), "{line}");
            note(&line);
            tokio::time::sleep(pause).await;
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

    /// One exchange, its answer read whole, asked again by the one rule ([`ask`]). Nothing an
    /// exchange sends or reads is kept from a failed attempt, so each counts toward [`Backoff::transfer`]'s.
    async fn exchange(&self, ask: Ask<'_>) -> Result<Reply> {
        let mut backoff = Backoff::transfer();
        loop {
            let (why, wait) = match self.challenged(&ask).await {
                Ok(reply) if !ask::weather(reply.status) => return Ok(reply),
                Ok(reply) => {
                    let wait = reply.header("retry-after").and_then(|s| s.trim().parse().ok()).map(std::time::Duration::from_secs);
                    (format!("{} {}: {}", ask.method, shown(ask.url), reply.status), wait)
                }
                Err(GetError::Exchange(refusal)) if ask::transient(&refusal) => (refusal.detail, None),
                Err(error) => return Err(error.refusal()),
            };
            match backoff.again(&why, false, wait) {
                Some((pause, line)) => {
                    tracing::info!("{line}");
                    tokio::time::sleep(pause).await;
                }
                None => return refuse(Code::HUB_UNREACHABLE, backoff.spent(&why)),
            }
        }
    }

    /// One attempt, made once more when the provider asks to (a nonce, a refresh).
    async fn challenged(&self, ask: &Ask<'_>) -> std::result::Result<Reply, GetError> {
        let reply = self.attempt(ask).await?;
        let presented = Request { method: ask.method, url: ask.url, headers: &reply.presented, payload: ask.payload };
        match (ask.auth, &self.token) {
            (true, Some(token)) if token.retry(&presented, reply.status, &reply.headers).map_err(GetError::Caller)? => self.attempt(ask).await,
            _ => Ok(reply),
        }
    }

    async fn attempt(&self, ask: &Ask<'_>) -> std::result::Result<Reply, GetError> {
        let mut headers = ask.headers.clone();
        if ask.auth {
            // A provider's failure is the caller's verdict, not failed network traffic.
            headers.extend(self.authorize(ask.method, ask.url, &ask.headers, ask.payload).await.map_err(GetError::Caller)?);
        }
        // This attempt's bytes, given back when it fails: what is on its way or landed, never twice.
        let mine = Arc::new(AtomicU64::new(0));
        let sent = || vec![Arc::clone(&self.sent), Arc::clone(&mine)];
        let body = match ask.body {
            Some(make) => Some(match make()? {
                Payload::Bytes(bytes) => Body::Bytes(bytes),
                Payload::File(file) => {
                    let length = file.metadata().map_err(|e| crate::err::classify_io(ask.url, e))?.len();
                    Body::File { file, length, sent: sent() }
                }
                Payload::Part(file, length) => Body::File { file, length, sent: sent() },
            }),
            None => None,
        };
        let reply = self.round_trip(ask.method, ask.url, &headers, body).await;
        if !reply.as_ref().is_ok_and(|reply| matches!(reply.status, 200..=299 | 412)) {
            self.sent.fetch_sub(mine.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        let mut reply = reply?;
        reply.presented = headers;
        Ok(reply)
    }

    async fn round_trip(&self, method: &str, url: &str, headers: &[(String, String)], body: Option<Body>) -> Result<Reply> {
        let mut answer = self.client.send(method, url, headers, body, Some(&self.pulse)).await?;
        let (status, headers) = (answer.status(), answer.response.headers().clone());
        let mut body = Vec::new();
        while let Some(frame) = answer.frame().await? {
            body.extend_from_slice(&frame);
            if body.len() > DOCUMENT_BYTES {
                return refuse(Code::SIZE_CAP, format!("{method} {}: an answer over {DOCUMENT_BYTES} B", shown(url)));
            }
        }
        Ok(Reply { url: url.to_owned(), status, headers, body, presented: vec![] })
    }
}

impl Origin for Endpoint {
    fn locate<'a>(&'a self, objects: &'a [ObjectRef]) -> Boxed<'a, Result<Vec<Location>>> {
        Box::pin(async move { Ok(self.located(objects)) })
    }

    fn public(&self) -> bool {
        self.token.is_none()
    }
}

impl Endpoint {
    fn located(&self, objects: &[ObjectRef]) -> Vec<Location> {
        let mut out = Vec::with_capacity(objects.len());
        for object in objects {
            out.push(match &self.at {
                At::Dir(root, _) => Location::File(root.join(object.key())),
                At::Http(base) => {
                    let url = format!("{base}/{}", object.key());
                    match &self.token {
                        Some(provider) => Location::Authenticated { url, provider: Arc::clone(provider) },
                        None => Location::Url { url, headers: vec![] },
                    }
                }
            });
        }
        out
    }
}

/// Cooperative control of one upload. `commit` is called once after the objects land, before
/// the commit is recorded; once it accepts, cancellation no longer interrupts. Progress counts
/// verified bytes, including objects the endpoint already holds. `retrying` hears each object asked
/// again (`<why>; attempt 2/10 in 3 s`).
pub struct UploadControl<'a> {
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    pub progress: &'a (dyn Fn(u64, u64) + Sync),
    pub retrying: &'a (dyn Fn(&str) + Sync),
    pub commit: &'a (dyn Fn() -> bool + Sync),
}

/// Sends what `endpoint` lacks of commit `tree`'s closure in `store` (chunks, assets and repo
/// files, then the header, then the tree), then records it as model `name`'s
/// ([`Endpoint::commit`]).
pub async fn upload_checkpoint(endpoint: &Endpoint, name: &str, tree: &ObjectRef, store: &Store, control: &UploadControl<'_>) -> Result<Uploaded> {
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
        let mut sending = futures_util::stream::iter(stage)
            .map(|object| async move {
                let (from, at) = (store.clone(), object.clone());
                let payload = move || from.file(&at).map(Payload::File).map_err(crate::store::Error::refusal);
                endpoint.upload_noted(&object, &payload, control.retrying).await.map(|s| (s, object.length))
            })
            .buffer_unordered(STREAMS);
        loop {
            if (control.cancelled)() {
                return Err(stopped());
            }
            let done = tokio::select! {
                done = sending.next() => done,
                () = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
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
    Ok(Uploaded { checkpoint: tree.clone(), objects, sent, bytes, converged })
}

/// A writer's objects on their way to an endpoint (`Writer(model_id=…)`): each is sent (HEAD →
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
    pub(crate) fn new(endpoint: Arc<Endpoint>, runtime: tokio::runtime::Handle) -> Stream {
        Stream {
            endpoint,
            runtime,
            sent: Mutex::default(),
            sending: Mutex::default(),
            failed: Mutex::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT)),
        }
    }

    /// Sends `bytes` as an object, once; its ref at once.
    pub(crate) fn put(&self, bytes: Vec<u8>) -> Result<ObjectRef> {
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
    pub(crate) fn flush(&self) -> Result<()> {
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
    pub(crate) fn commit(&self, name: &str, lane: &str, header: Vec<u8>, files: &BTreeMap<String, ObjectRef>) -> Result<(ObjectRef, Tree)> {
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
fn check_model_name(name: &str) -> Result<()> {
    match model_name(name) {
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
            format!("{}: {length} B, not {}", shown(&reply.url), object.length),
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
    Refusal { code, detail: format!("{what} {}: {}{named}: {message}{detail}", shown(&reply.url), reply.status) }
}

/// A streaming writer's upload: each object to `stream` as it seals, the commit into repo `name` in lane `lane`.
#[derive(Debug)]
struct Streamed {
    stream: Arc<Stream>,
    name: String,
    lane: String,
}

impl Upload for Streamed {
    fn sealed(&self, store: &Store, object: &ObjectRef) -> Result<()> {
        self.stream.put_stored(store, object)
    }

    fn commit(&self, header: Vec<u8>, repo: &BTreeMap<String, ObjectRef>) -> Result<Tree> {
        self.stream.commit(&self.name, &self.lane, header, repo).map(|(_, tree)| tree)
    }
}

/// A writer of `store` whose objects go to `stream` as they seal and whose commit (the header with
/// its repo files, in lane `lane`) becomes one of repo `name`'s there. The local checkpoint commits
/// first and remains available when the upload fails. With `key`, it resumes.
pub(crate) fn streaming_writer(store: &Store, stream: Arc<Stream>, name: &str, lane: &str, base: Option<&Checkpoint>, key: Option<&str>) -> store::Result<Writer> {
    if !crate::format::label(lane) {
        return Err(crate::format::Error::Invalid(format!("lane {lane:?} is not a label")).into());
    }
    let mut writer = crate::writer(store, base, key)?;
    writer.uploading(Arc::new(Streamed { stream, name: name.to_owned(), lane: lane.to_owned() }));
    Ok(writer)
}

#[cfg(test)]
mod checkpoint_wire_tests {
    #[test]
    fn resolved_checkpoint_reads_both_names_and_emits_the_supported_wire_field() {
        let object = crate::format::ObjectRef::of(b"fixture");
        let legacy = serde_json::json!({"commit":object,"header":object});
        let canonical = serde_json::json!({"checkpoint":object,"header":object});
        let resolved: super::Resolved = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::from_value::<super::Resolved>(canonical).unwrap(), resolved);
        let wire = serde_json::to_value(&resolved).unwrap();
        assert!(wire.get("commit").is_some());
        assert!(wire.get("checkpoint").is_none());
        let mut ambiguous = legacy;
        ambiguous["checkpoint"] = serde_json::to_value(object).unwrap();
        assert!(serde_json::from_value::<super::Resolved>(ambiguous).is_err());
    }
}
