//! The CLI's client of Calcifer: loopback HTTP/1.1 with the bearer from Calcifer's lock record,
//! typed refusals from its envelope, and a run's event stream. Any call starts Calcifer when none
//! runs, from the home's installed `cozy` only: as this user's service where a service manager
//! answers, else as a detached `cozy calcifer`. A start is waited on until Calcifer publishes its
//! record or its refusal, and a long wait is reported. Every request goes by the one retry rule
//! (`Client::send`): a restart is waited out, what may answer next time is asked again within a
//! minute, each attempt said. A request or stream Calcifer leaves unanswered says so, naming its
//! process and what it is doing.
use std::{
    fs::OpenOptions,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use platform::{
    lock::{Kind as LockKind, LockFile},
    paths::Home,
    process::{self, Detached},
    service::{ServiceSpec, Services},
    tools,
};
use api::{
    calcifer::{Down, Event, Holder, LogEvent, MachineList, REFUSED, Run, RunList, StreamEvent, project::{VaultAsk, VaultChanged, VaultEvent}},
    error::{Envelope, Error, Kind, STOPPING},
    ids::Digest,
    machine::{HELD, LogLines, Ranges, parse_held},
};
use reqwest::{RequestBuilder, Response, StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
use tensorfs::fetch::Backoff;

/// `calcifer::api::IGNORED_FIELDS`: the request fields Calcifer did not read.
const IGNORED_FIELDS: &str = "cozy-ignored-fields";
/// What a write is asked under: every send of one call is the same ask, done once (cl-812).
const KEY: &str = "Idempotency-Key";
/// An answer that is not a typed refusal.
const UNTYPED: &str = "untyped_answer";
/// Said of each attempt while Calcifer restarts.
const RESTARTING: &str = "Calcifer restarting";

use crate::{Context, output};

/// How often a wait looks again: a sampling period, never a deadline.
const SAMPLE: Duration = Duration::from_millis(50);
/// How often a wait that goes on says so.
const REPORT: Duration = Duration::from_secs(5);
/// Calcifer's streams carry a keepalive this often (axum's default): one silent for two is a Calcifer
/// that does not answer.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// What a request is to the retry rule.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// Changes nothing.
    Read,
    /// Done once however often it is sent: idempotent by its method, or under a key its route keeps.
    Write,
    /// Done again if sent again: sent again only when Calcifer certainly did not carry it out.
    Unkeyed,
}

/// This home's Calcifer lock as `cozy doctor` reads it.
pub(crate) enum Lock {
    /// Nobody holds the home; `stale`: a record names a Calcifer killed before it erased it.
    Free { stale: bool },
    /// A Calcifer holds the home and its record names it, alive.
    Held(Holder),
    /// The home's lock is held, but by no live Calcifer its record names: a process another one
    /// started keeps it open (a machine a previous Calcifer started, before cl-646's fix).
    Captive,
}

/// One command's connection to Calcifer. Cheap to clone; clones share the connection.
#[derive(Clone)]
pub(crate) struct Client(Arc<Inner>);

/// A vault operation's public progress. Its terminal response, which may carry a recovery secret,
/// is returned separately and never sent to the progress renderer.
pub(crate) enum VaultProgress<'a> {
    Accepted { action: &'a str },
    Progress { phase: &'a str, files_done: u64, bytes_done: u64 },
}

struct Inner {
    http: reqwest::Client,
    home: Home,
    user_home: PathBuf,
    json: bool,
    holder: Mutex<Option<Holder>>,
}

impl Client {
    /// Calcifer, started if none runs.
    pub(crate) async fn connect(ctx: &Context) -> Result<Self, Error> {
        let client = Self::new(ctx)?;
        client.ready().await?;
        Ok(client)
    }

    /// The running Calcifer, or `None`: never starts one.
    pub(crate) async fn running(ctx: &Context) -> Result<Option<Self>, Error> {
        let client = Self::new(ctx)?;
        Ok(client.live().await.map(|holder| {
            client.remember(Some(holder));
            client
        }))
    }

    fn new(ctx: &Context) -> Result<Self, Error> {
        // Who a project's history says made an edit.
        let said = header::HeaderMap::from_iter([(header::HeaderName::from_static("cozy-client"), header::HeaderValue::from_static("cli"))]);
        let http = tensorfs::fetch::http(&[]).expect("the webpki roots alone")
            .no_proxy()
            .default_headers(said)
            .build()
            .map_err(|e| Error::new(Kind::Internal, "http_client", e.to_string()))?;
        Ok(Self(Arc::new(Inner {
            http,
            home: ctx.home.clone(),
            user_home: ctx.user_home.clone(),
            json: ctx.json,
            holder: Mutex::new(None),
        })))
    }

    /// A client that reaches nothing yet (`cozy doctor`, which looks before it starts anything).
    pub(crate) fn unconnected(ctx: &Context) -> Result<Self, Error> {
        Self::new(ctx)
    }

    /// What this home's lock says of its Calcifer, starting nothing.
    pub(crate) fn lock_state(&self) -> Lock {
        let free = self.lock_free();
        let raw = LockFile::new(self.0.home.calcifer_lock()).holder().ok().flatten();
        let alive = |pid: u32| {
            process::Birth::of(pid).and_then(|birth| birth.state()).is_ok_and(|s| s != process::State::Gone)
        };
        match (free, raw.map(|raw| serde_json::from_slice::<Holder>(&raw))) {
            (true, record) => Lock::Free { stale: record.is_some() },
            (false, Some(Ok(holder))) if alive(holder.pid) => Lock::Held(holder),
            (false, _) => Lock::Captive,
        }
    }

    /// Whether the Calcifer `holder` names answers one request: any reply is an answer. Waits as
    /// long as the request does; the caller judges a silence by the process's progress.
    pub(crate) async fn answers(&self, holder: &Holder) -> bool {
        let probe = format!("http://{}/v1/status", holder.address);
        let request = self.0.http.get(probe);
        request.bearer_auth(&holder.token).send().await.is_ok()
    }

    /// Calcifer's service manager for this home, its definition for the installed cozy, and
    /// whether this process is that cozy (only it writes the definition).
    pub(crate) fn service(&self) -> Result<(Services, ServiceSpec, bool), Error> {
        let installed = installed(&self.0.home)?;
        let exe = std::env::current_exe().and_then(|exe| exe.canonicalize());
        let ours = matches!((exe, installed.canonicalize()), (Ok(exe), Ok(it)) if exe == it);
        let services = Services::new(&self.0.home, &self.0.user_home);
        Ok((services, self.spec(installed), ours))
    }

    /// The record of the Calcifer this client last reached.
    pub(crate) fn holder(&self) -> Option<Holder> {
        self.0.holder.lock().expect("holder").clone()
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        self.exchange(Ask::Read, |c, base| c.get(format!("{base}{path}"))).await
    }

    pub(crate) async fn get_query<Q, T>(&self, path: &str, query: &Q) -> Result<T, Error>
    where
        Q: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.exchange(Ask::Read, |c, base| c.get(format!("{base}{path}")).query(query)).await
    }

    /// A POST under `key`, its `Idempotency-Key`: a route that keeps keys does it once however often it
    /// is sent, and a quote buys nothing, so either is asked again by the retry rule. Without one it goes
    /// under the call's own key, and is sent again only when Calcifer certainly did not carry it out.
    pub(crate) async fn post<B, T>(&self, path: &str, body: &B, key: Option<&str>) -> Result<T, Error>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let ask = match key.is_some() || path.ends_with("-quotes") {
            true => Ask::Write,
            false => Ask::Unkeyed,
        };
        let minted = write_key();
        let key = key.unwrap_or(&minted);
        let body = json_body(body)?;
        self.exchange(ask, |c, base| json(c.post(format!("{base}{path}")), &body).header(KEY, key)).await
    }

    /// One vault operation ID for the command's lifetime. Reconnecting the same PUT attaches to
    /// that operation; the server records admission before acknowledging it and never runs it twice.
    /// A controller without this route gets one legacy POST, whose uncertain result is not retried.
    pub(crate) async fn vault(
        &self,
        path: &str,
        request: &VaultAsk,
        mut progress: impl FnMut(VaultProgress<'_>),
    ) -> Result<VaultChanged, Error> {
        let body = json_body(request)?;
        let operation = format!("{path}/operations/{}", write_key());
        let mut reconnects = Backoff::transfer();
        let (mut can_use_legacy, mut accepted) = (true, false);
        let mut last_progress = None;
        loop {
            let mut answer = self.send(Ask::Write, |c, base| {
                json(c.put(format!("{base}{operation}")), &body).header(header::ACCEPT, "text/event-stream")
            }).await.map_err(vault_uncertain)?;
            if !answer.status().is_success() {
                let status = answer.status();
                let bytes = match answer.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        self.vault_reconnect(&mut reconnects, false, "the refusal was interrupted").await?;
                        continue;
                    }
                };
                let error = vault_refusal(status, &bytes);
                if can_use_legacy && status == StatusCode::NOT_FOUND && matches!(error.code.as_str(), "unknown_route" | UNTYPED) {
                    return self.legacy_vault(path, &body).await;
                }
                return Err(error);
            }
            // Even without an Accepted event, a successful operation response may have admitted it.
            // A later missing/expired operation must never turn into a new legacy mutation.
            can_use_legacy = false;
            let content_type = answer.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next()).map(str::trim).unwrap_or("");
            match content_type {
                "application/json" => {
                    let bytes = match answer.bytes().await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            self.vault_reconnect(&mut reconnects, false, "the final response was interrupted").await?;
                            continue;
                        }
                    };
                    return serde_json::from_slice(&bytes).map_err(|_| vault_interrupted("the final response could not be read"));
                }
                "text/event-stream" => {}
                _ => return Err(vault_interrupted("Calcifer returned an unsupported response type")),
            }
            let mut sse = Sse::default();
            let mut moved = false;
            let reason = loop {
                while let Some(data) = sse.next() {
                    // Parse errors can quote values from the response. Keep credential-bearing
                    // event data out of both diagnostics and the progress callback.
                    let event: VaultEvent = serde_json::from_str(&data)
                        .map_err(|_| vault_interrupted("an event could not be read"))?;
                    match event {
                        VaultEvent::Accepted { action } => {
                            if !accepted { progress(VaultProgress::Accepted { action: &action }); }
                            accepted = true;
                        }
                        VaultEvent::Progress { phase, files_done, bytes_done } => {
                            let point = (phase, files_done, bytes_done);
                            if last_progress.as_ref() != Some(&point) {
                                moved = true;
                                progress(VaultProgress::Progress { phase: &point.0, files_done, bytes_done });
                                last_progress = Some(point);
                            }
                        }
                        VaultEvent::Completed { changed } => return Ok(changed),
                        VaultEvent::Failed { error } => return Err(error),
                    }
                }
                // A healthy stream sends keepalives. Silence reattaches to the same recorded
                // operation, without a generic watchdog asking the user to restart Calcifer.
                match tokio::time::timeout(2 * KEEPALIVE, answer.chunk()).await {
                    Ok(Ok(Some(bytes))) => sse.push(&bytes),
                    Ok(Ok(None)) => break "the stream closed before its final result",
                    Ok(Err(_)) => break "the stream was interrupted before its final result",
                    Err(_) => break "the stream missed two keepalives",
                }
            };
            self.vault_reconnect(&mut reconnects, moved, reason).await?;
        }
    }

    async fn vault_reconnect(&self, reconnects: &mut Backoff, moved: bool, reason: &str) -> Result<(), Error> {
        let why = format!("Vault operation: {reason}; reconnecting to the same operation");
        let Some((pause, said)) = reconnects.again(&why, moved, None) else {
            return Err(vault_interrupted(&reconnects.spent(reason)));
        };
        self.remember(None);
        output::warned(&said);
        tokio::time::sleep(pause).await;
        Ok(())
    }

    async fn legacy_vault(&self, path: &str, body: &bytes::Bytes) -> Result<VaultChanged, Error> {
        let answer = self.send(Ask::Unkeyed, |c, base| {
            json(c.post(format!("{base}{path}")), body).header(header::ACCEPT, "application/json")
        }).await.map_err(vault_uncertain)?;
        let status = answer.status();
        let body = answer.bytes().await.map_err(|_| vault_interrupted("the legacy response was interrupted"))?;
        if !status.is_success() { return Err(vault_refusal(status, &body)); }
        serde_json::from_slice(&body).map_err(|_| vault_interrupted("the legacy response could not be read"))
    }

    /// A PUT: idempotent by its path, and under its call's key too.
    pub(crate) async fn put<B, T>(&self, path: &str, body: &B) -> Result<T, Error>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let (key, body) = (write_key(), json_body(body)?);
        self.exchange(Ask::Write, |c, base| json(c.put(format!("{base}{path}")), &body).header(KEY, &key)).await
    }

    pub(crate) async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let key = write_key();
        self.exchange(Ask::Write, |c, base| c.delete(format!("{base}{path}")).header(KEY, &key)).await
    }

    pub(crate) async fn delete_query<Q, T>(&self, path: &str, query: &Q) -> Result<T, Error>
    where
        Q: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let key = write_key();
        self.exchange(Ask::Write, |c, base| c.delete(format!("{base}{path}")).query(query).header(KEY, &key)).await
    }

    /// A PATCH: sets the fields `body` names, under its call's key.
    pub(crate) async fn patch<B, T>(&self, path: &str, body: &B) -> Result<T, Error>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let (key, body) = (write_key(), json_body(body)?);
        self.exchange(Ask::Write, |c, base| json(c.patch(format!("{base}{path}")), &body).header(KEY, &key)).await
    }

    /// What Calcifer holds of the file `digest` names: all of it (`None`), else the ranges held.
    pub(crate) async fn held_for(&self, digest: &Digest, project: Option<&str>) -> Result<Option<Ranges>, Error> {
        let path = match project { Some(_) => format!("/v1/project-files/{digest}"), None => format!("/v1/files/{digest}") };
        let answer = self.send(Ask::Read, &|c: &reqwest::Client, base: &str| {
            let request = c.head(format!("{base}{path}"));
            match project { Some(project) => request.query(&[("project", project)]), None => request }
        }).await?;
        if project.is_some() && answer.status() == StatusCode::NOT_FOUND && !answer.headers().contains_key(HELD) {
            return Err(Error::new(Kind::Unavailable, "project_upload_unsupported", "Calcifer does not support protected project uploads; update it before sending private files"));
        }
        let ranges = || answer.headers().get(HELD).and_then(|v| v.to_str().ok()).and_then(parse_held).unwrap_or_default();
        match answer.status() {
            status if status.is_success() => Ok(None),
            StatusCode::NOT_FOUND => Ok(Some(ranges())),
            _ => Err(envelope(answer).await),
        }
    }

    /// `part`, the bytes of `digest` from `start`, of a file of `length` bytes. Whether the file is now whole.
    pub(crate) async fn put_part(&self, digest: &Digest, part: bytes::Bytes, start: u64, length: u64) -> Result<bool, Error> {
        self.put_part_for(digest, part, start, length, None).await
    }

    pub(crate) async fn put_part_for(&self, digest: &Digest, part: bytes::Bytes, start: u64, length: u64, project: Option<&str>) -> Result<bool, Error> {
        let path = match project { Some(_) => format!("/v1/project-files/{digest}"), None => format!("/v1/files/{digest}") };
        let range = match part.is_empty() {
            true => None,
            false => Some(format!("bytes {start}-{}/{length}", start + part.len() as u64 - 1)),
        };
        let answer = self
            .send(Ask::Write, &|c: &reqwest::Client, base: &str| {
                let put = c.put(format!("{base}{path}")).header(header::CONTENT_TYPE, "application/octet-stream");
                let put = match project { Some(project) => put.query(&[("project", project)]), None => put };
                // A whole file goes by its Content-Length, stated: an empty body carries none of its own.
                match &range {
                    Some(range) if part.len() as u64 != length => put.header(header::CONTENT_RANGE, range),
                    _ => put.header(header::CONTENT_LENGTH, part.len() as u64),
                }
                .body(part.clone())
            })
            .await?;
        Ok(ok(answer).await?.status() != StatusCode::NO_CONTENT)
    }

    /// A plain-text answer, such as a log.
    pub(crate) async fn text(&self, path: &str) -> Result<String, Error> {
        self.read_whole(|| async {
            let answer = self.opened(|c, base| c.get(format!("{base}{path}"))).await?;
            Ok(answer.text().await.map_err(unreachable))
        })
        .await
    }

    /// A file Calcifer serves (an output, one file of a folder output), written to `to` as it comes: what it wrote.
    pub(crate) async fn download(&self, path: &str, to: &std::path::Path) -> Result<u64, Error> {
        self.read_whole(|| async {
            let mut answer = self.opened(|c, base| c.get(format!("{base}{path}"))).await?;
            let unwritable = |e: std::io::Error| Error::new(Kind::Unavailable, "download_unwritable", format!("{}: {e}", to.display()));
            let mut file = std::fs::File::create(to).map_err(unwritable)?;
            let mut written = 0;
            loop {
                match answer.chunk().await {
                    Ok(Some(chunk)) => {
                        file.write_all(&chunk).map_err(unwritable)?;
                        written += chunk.len() as u64;
                    }
                    Ok(None) => break,
                    Err(e) => return Ok(Err(unreachable(e))),
                }
            }
            file.sync_all().map_err(unwritable)?;
            Ok(Ok(written))
        })
        .await
    }

    /// A read whose body was cut off (`Ok(Err)`: Calcifer stopped under it), read again from the start by
    /// the retry rule; what came before its answer `send` already waited out.
    async fn read_whole<T, F>(&self, read: impl Fn() -> F) -> Result<T, Error>
    where
        F: Future<Output = Result<Result<T, Error>, Error>>,
    {
        let mut backoff = Backoff::call();
        loop {
            let cut = match read().await? {
                Ok(read) => return Ok(read),
                Err(cut) => cut,
            };
            self.remember(None);
            match backoff.again(&cut.message, false, None) {
                Some((wait, said)) => {
                    output::warn(&said);
                    tokio::time::sleep(wait).await;
                }
                None => return Err(Error { message: backoff.spent(&cut.message), ..cut }),
            }
        }
    }

    /// One request whose answer is read as `T`. An answer showing this Calcifer is older than this
    /// cozy (a route it does not serve, a body it can't read, or a read's answer without fields this
    /// cozy reads) restarts it once, the normal way, and asks again: none of those carried anything
    /// out. A `write` that succeeded is never sent again: an answer it can't read, or request fields
    /// it did not read (`Cozy-Ignored-Fields`), is said, and Calcifer restarted for the next command.
    /// One that still lacks it is refused as outdated, never as a bare 404.
    async fn exchange<T: DeserializeOwned>(
        &self,
        ask: Ask,
        build: impl Fn(&reqwest::Client, &str) -> RequestBuilder,
    ) -> Result<T, Error> {
        let write = ask != Ask::Read;
        let mut restarted = false;
        loop {
            let answer = self.send(ask, &build).await?;
            let ignored = answer.headers().get(IGNORED_FIELDS).and_then(|v| v.to_str().ok()).map(str::to_owned);
            let lacks = match self.lacks(answer).await? {
                Ok(body) => match serde_json::from_slice::<T>(&body) {
                    Ok(value) => {
                        if let Some(fields) = ignored {
                            let lacks = format!("does not read {fields}");
                            self.say(&format!("{}; it carried out the ask without them", self.lacking(&lacks)));
                            self.restart(&lacks).await?;
                        }
                        return Ok(value);
                    }
                    Err(e) if self.other_version() && write => {
                        let lacks = format!("answers without what this cozy reads ({e})");
                        self.say(&format!("{}; it carried out the ask, which isn't sent again", self.lacking(&lacks)));
                        self.restart(&lacks).await?;
                        let said = format!("The operation completed, but its response could not be read: {e}");
                        return Err(Error::new(Kind::Internal, "answer_unreadable", said));
                    }
                    Err(e) if self.other_version() => format!("answers without what this cozy reads ({e})"),
                    Err(e) => {
                        let said = format!("Cannot read Calcifer's response: {e}");
                        return Err(Error::new(Kind::Internal, "answer_unreadable", said));
                    }
                },
                Err(lacks) => lacks,
            };
            if restarted || !self.restart(&lacks).await? {
                return Err(self.outdated(&lacks));
            }
            restarted = true;
        }
    }

    /// One request whose answer is read as it comes (a stream, text): a route this Calcifer does
    /// not serve restarts it once and asks again, as `exchange` does.
    async fn opened(&self, build: impl Fn(&reqwest::Client, &str) -> RequestBuilder) -> Result<Response, Error> {
        let mut restarted = false;
        loop {
            let answer = self.send(Ask::Read, &build).await?;
            if answer.status() != StatusCode::NOT_FOUND {
                return ok(answer).await;
            }
            let lacks = match self.lacks(answer).await? {
                Ok(_) => unreachable!("a 404 is never an answer"),
                Err(lacks) => lacks,
            };
            if restarted || !self.restart(&lacks).await? {
                return Err(self.outdated(&lacks));
            }
            restarted = true;
        }
    }

    /// An answer's body when it succeeded; `Err(lacks)` when it shows Calcifer is older than this
    /// cozy: a route it does not serve, or (from a Calcifer of another version) a request body it
    /// cannot read, which is this cozy's newer shape. Any other refusal is the outer error.
    async fn lacks(&self, answer: Response) -> Result<Result<Vec<u8>, String>, Error> {
        if answer.status().is_success() {
            return Ok(Ok(answer.bytes().await.map_err(unreachable)?.to_vec()));
        }
        let refusal = envelope(answer).await;
        // A Calcifer of this very version lacks nothing this cozy has: restarting it cannot help.
        match (refusal.code.as_str(), self.other_version()) {
            ("unknown_route", true) => Ok(Err(refusal.message.replacen("Calcifer serves", "serves", 1))),
            ("body_invalid", true) => Ok(Err(format!("cannot read this request ({})", refusal.message))),
            _ => Err(refusal),
        }
    }

    /// Whether the Calcifer this talks to runs another cozy version than this one.
    fn other_version(&self) -> bool {
        self.holder().is_some_and(|h| h.version != env!("CARGO_PKG_VERSION"))
    }

    /// "Calcifer runs cozy X, which <lacks>".
    fn lacking(&self, lacks: &str) -> String {
        let version = self.holder().map_or_else(|| "?".into(), |h| h.version);
        format!("Calcifer runs cozy {version}, which {lacks}")
    }

    /// Restarts Calcifer on the installed cozy because it `lacks` something this cozy asks for;
    /// false when the restart brought back the same version, which would lack it still.
    async fn restart(&self, lacks: &str) -> Result<bool, Error> {
        let Some(holder) = self.holder() else { return Ok(false) };
        self.say(&format!("{}; restarting it on the installed cozy", self.lacking(lacks)));
        if let Err(e) = self.down(&holder).await {
            self.say(&format!("Calcifer did not stop cleanly ({}); starting the installed one", e.message));
        }
        self.remember(None);
        let started = self.endpoint().await?;
        Ok(started.version != holder.version)
    }

    /// Calcifer is older than this cozy and the installed cozy is too.
    fn outdated(&self, lacks: &str) -> Error {
        Error::new(
            Kind::Unavailable,
            "calcifer_outdated",
            format!("{}; this is cozy {}", self.lacking(lacks), env!("CARGO_PKG_VERSION")),
        )
        .next("cozy update")
    }

    /// A run by its id or number.
    pub(crate) async fn run(&self, id: &str) -> Result<Run, Error> {
        self.get(&path(&["v1", "runs", id])).await
    }

    /// Runs, newest first; `query` is the route's filters.
    pub(crate) async fn runs<Q: Serialize + ?Sized>(&self, query: &Q) -> Result<RunList, Error> {
        self.get_query("/v1/runs", query).await
    }

    /// Asks for a run's cancel; its machine's own outcome ends it.
    pub(crate) async fn cancel(&self, id: &str) -> Result<Run, Error> {
        let path = path(&["v1", "runs", id, "cancel"]);
        self.post(&path, &serde_json::json!({}), None).await
    }

    pub(crate) async fn machines(&self) -> Result<MachineList, Error> {
        self.get("/v1/machines").await
    }

    /// A machine log's newest `lines` whole lines (0: all) at `path`, then each batch as the
    /// machine appends it.
    pub(crate) fn follow_log(&self, path: &str, lines: u64) -> LogFollow {
        LogFollow {
            client: self.clone(),
            path: path.to_owned(),
            lines,
            after: None,
            stream: None,
            sse: Sse::default(),
            received: false,
            reconnects: Backoff::transfer(),
        }
    }

    /// A run's events after `starting_after`, to its terminal one.
    pub(crate) fn events(&self, id: &str, starting_after: u64) -> Events {
        Events {
            client: self.clone(),
            path: path(&["v1", "runs", id, "events"]),
            cursor: starting_after,
            stream: None,
            sse: Sse::default(),
            received: false,
            reconnects: Backoff::transfer(),
            opened_on: None,
            restarted: false,
            landing: false,
            ended: false,
            done: false,
        }
    }

    /// A run's events after `starting_after` past its terminal one, until Calcifer ends the
    /// stream: once each output's last revision landed or failed (or Calcifer stopped).
    pub(crate) fn events_until_landed(&self, id: &str, starting_after: u64) -> Events {
        Events {
            landing: true,
            ..self.events(id, starting_after)
        }
    }

    /// `GET /v1/events`, subscribed once this answers: it carries no backlog, so a caller reads what
    /// it holds after it, and reads it with [`Watch::change`].
    pub(crate) async fn changes(&self) -> Result<Watch<StreamEvent>, Error> {
        self.watch("/v1/events").await
    }

    /// Run changes only, without machine snapshots or project subscriptions.
    pub(crate) async fn run_changes(&self) -> Result<Watch<StreamEvent>, Error> {
        self.watch("/v1/events?scope=runs").await
    }

    /// A stream with no place to resume from, subscribed once this answers.
    pub(crate) async fn watch<T: DeserializeOwned>(&self, path: &str) -> Result<Watch<T>, Error> {
        let answer = self.opened(|c, base| {
            c.get(format!("{base}{path}"))
                .header(header::ACCEPT, "text/event-stream")
        });
        Ok(Watch {
            client: self.clone(),
            stream: answer.await?,
            sse: Sse::default(),
            heard: Instant::now(),
            quiet: false,
            behind: false,
            of: std::marker::PhantomData,
        })
    }

    /// Asks Calcifer to stop and waits until that process has let go of the home. Runs on
    /// machines continue; the next Calcifer resumes what it owed.
    pub(crate) async fn stop(&self) -> Result<Down, Error> {
        let holder = self.endpoint().await?;
        let down = self.down(&holder).await?;
        self.remember(None);
        Ok(down)
    }

    async fn down(&self, holder: &Holder) -> Result<Down, Error> {
        let base = format!("http://{}", holder.address);
        let request = self.0.http.post(format!("{base}/v1/down")).bearer_auth(&holder.token);
        let answer = self
            .heard(holder.pid, Instant::now(), REPORT, request.json(&serde_json::json!({})).send())
            .await
            .map_err(unreachable)?;
        let down: Down = decode(answer).await?;
        self.wait("Calcifer to stop", || async { !self.holds(holder) })
            .await;
        Ok(down)
    }

    /// Whether the Calcifer `holder` names runs another cozy than this one, which is the installed
    /// one: something replaced the binary under it (`cozy machine update` on a pod, an installer).
    fn replaced(&self, holder: &Holder) -> bool {
        let me = std::env::current_exe().and_then(|exe| exe.canonicalize());
        let installed = installed(&self.0.home).ok().and_then(|it| it.canonicalize().ok());
        holder.version != env!("CARGO_PKG_VERSION") && matches!((me, installed), (Ok(me), Some(it)) if me == it)
    }

    /// Reaches Calcifer, starting it if none runs.
    pub(crate) async fn ready(&self) -> Result<Holder, Error> {
        self.endpoint().await
    }

    async fn endpoint(&self) -> Result<Holder, Error> {
        if let Some(holder) = self.holder() {
            return Ok(holder);
        }
        let holder = match self.live().await {
            // It restarts on this binary: runs on machines go on, and it resumes what it owed.
            Some(holder) if self.replaced(&holder) => {
                self.say(&format!(
                    "Calcifer runs cozy {}; restarting it on {}",
                    holder.version,
                    env!("CARGO_PKG_VERSION")
                ));
                if let Err(e) = self.down(&holder).await {
                    self.say(&format!("Calcifer did not stop cleanly ({}); starting the installed one", e.message));
                }
                self.start().await?
            }
            Some(holder) => holder,
            None => self.start().await?,
        };
        self.remember(Some(holder.clone()));
        Ok(holder)
    }

    fn remember(&self, holder: Option<Holder>) {
        *self.0.holder.lock().expect("holder") = holder;
    }

    /// The published record, when its Calcifer answers. Asked over the pooled connection, which
    /// stays open for the command: Calcifer stays up while a client is connected.
    async fn live(&self) -> Option<Holder> {
        let holder = self.record()?;
        let probe = format!("http://{}/v1/status", holder.address);
        let request = self.0.http.get(probe);
        let answer = request.bearer_auth(&holder.token).send();
        self.heard(holder.pid, Instant::now(), REPORT, answer).await.ok()?;
        Some(holder)
    }

    fn record(&self) -> Option<Holder> {
        let raw = LockFile::new(self.0.home.calcifer_lock()).holder().ok()??;
        serde_json::from_slice(&raw).ok()
    }

    /// Whether the Calcifer that published `holder` still holds the home. A record its holder
    /// was killed before erasing names nobody once the lock is free.
    fn holds(&self, holder: &Holder) -> bool {
        self.record().as_ref() == Some(holder) && !self.lock_free()
    }

    fn lock_free(&self) -> bool {
        let lock = LockFile::new(self.0.home.calcifer_lock());
        matches!(lock.try_lock(LockKind::Exclusive), Ok(Some(_)))
    }

    /// Starts Calcifer and waits for its record or its refusal.
    async fn start(&self) -> Result<Holder, Error> {
        let home = &self.0.home;
        let installed = installed(home)?;
        let spec = self.spec(installed.clone());
        let failed = |e: platform::Error| {
            Error::new(Kind::Unavailable, "calcifer_start_failed", e.to_string()).next("cozy doctor")
        };
        home.open().map_err(failed)?;
        platform::durable::private_dir(&home.calcifer()).map_err(failed)?;
        let refused = home.calcifer().join(REFUSED);
        if let Err(e) = std::fs::remove_file(&refused)
            && e.kind() != io::ErrorKind::NotFound
        {
            return Err(failed(platform::Error::Io {
                op: "remove",
                path: refused,
                source: e,
            }));
        }
        let services = Services::new(home, &self.0.user_home);
        let mut steps = Steps::new(home.calcifer_log());
        // Another cozy (a build, a copy) never writes the service: it starts it only as already
        // defined for the installed cozy, else runs the installed one detached.
        let exe = std::env::current_exe().and_then(|exe| exe.canonicalize());
        let ours = matches!((exe, installed.canonicalize()), (Ok(exe), Ok(it)) if exe == it);
        let managed = services.available() && (ours || services.current(&spec));
        // Started detached for want of systemctl: no login starts it again. A pod has no login.
        let path: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        if !managed
            && cfg!(target_os = "linux")
            && !api::config::image_machine()
            && tools::on_path("systemctl", &path).is_none()
            && let Some(warning) = tools::warn_once("systemctl")
        {
            output::warn(&warning);
        }
        if managed && ours {
            let wrote = services.ensure(&spec).map_err(failed)?;
            steps.done(match wrote {
                true => "wrote Calcifer's service and reloaded its manager",
                false => "found Calcifer's service current",
            });
        }
        let (mut child, mut asked, mut cleared) = (None, false, false);
        let began = Instant::now();
        let mut reported = began;
        loop {
            // Nobody holds the home: start one. Again too when the last start lost to a
            // Calcifer that was stopping, which held the lock until it went.
            if child.is_none() && self.lock_free() {
                match managed {
                    // The manager stops starting a service killed or failed too often lately
                    // (`Services::clear`): its refusal is cleared once a start, so a Calcifer that
                    // keeps failing by itself is still said.
                    true => services
                        .start(&spec)
                        .or_else(|refused| match std::mem::replace(&mut cleared, true) {
                            true => Err(refused),
                            false => services.clear(&spec).and_then(|()| services.start(&spec)),
                        })
                        .map_err(failed)?,
                    false => child = Some(spawn(&spec).map_err(failed)?),
                }
                if !std::mem::replace(&mut asked, true) {
                    steps.done("started Calcifer");
                }
            }
            tokio::time::sleep(SAMPLE).await;
            if let Some(error) = refusal(&refused) {
                return Err(error);
            }
            if let Some(holder) = self.live().await {
                steps.done("Calcifer answered");
                return Ok(holder);
            }
            if let Some(tree) = child.as_mut()
                && let Some(status) = tree.try_wait().map_err(failed)?
            {
                if !status.success() {
                    return Err(refusal(&refused).unwrap_or_else(|| {
                        Error::new(
                            Kind::Unavailable,
                            "calcifer_start_failed",
                            format!("Calcifer exited before startup completed: {status}"),
                        )
                        .next("cozy machine logs local calcifer")
                    }));
                }
                // Another Calcifer held the lock: wait for its record, or for it to let go.
                child = None;
            }
            if reported.elapsed() >= REPORT {
                reported = Instant::now();
                self.say(&format!(
                    "waiting for Calcifer to start ({}s so far); `cozy machine logs local calcifer` shows what it is doing",
                    began.elapsed().as_secs()
                ));
            }
        }
    }

    /// Calcifer's service: the installed cozy's `cozy calcifer`.
    fn spec(&self, installed: PathBuf) -> ServiceSpec {
        ServiceSpec {
            name: "calcifer".into(),
            program: installed,
            args: vec!["calcifer".into()],
            log: Some(self.0.home.calcifer_log()),
        }
    }

    /// Sends one request by the one retry rule (Paul, 11-09), starting Calcifer when it is gone. A
    /// restart (a refused connection, or Calcifer refusing while it stops) is waited out for any ask:
    /// nothing was carried out. A dropped exchange, 408, 429, 502, 503 or 504 that is not a typed
    /// refusal, or one saying a machine or the Hub did not answer, is asked again only when `ask` is
    /// safe to repeat; an unsafe one says it was not. Each attempt reads Calcifer's record again and is
    /// said, after an exponential backoff with jitter or the wait the answer states, within one deadline
    /// ([`Backoff::call`]); then it fails with its last reason. Any other refusal among those statuses
    /// is the error at once; every other answer is returned.
    async fn send(
        &self,
        ask: Ask,
        build: impl Fn(&reqwest::Client, &str) -> RequestBuilder,
    ) -> Result<Response, Error> {
        let (mut backoff, mut refreshed) = (Backoff::call(), false);
        let repeats = ask != Ask::Unkeyed;
        let unsent = |error: Error| match repeats {
            true => error,
            false => error.remedy("Not retried; check whether the operation completed before repeating it"),
        };
        loop {
            let holder = self.endpoint().await?;
            let base = format!("http://{}", holder.address);
            let request = build(&self.0.http, &base).bearer_auth(&holder.token);
            let sent = self.heard(holder.pid, Instant::now(), REPORT, request.send()).await;
            let (error, stated, restarting) = match sent {
                // A restart minted a new token: asked again once under the record it published. The same
                // record refused is a refusal.
                Ok(answer) if answer.status() == StatusCode::UNAUTHORIZED && !refreshed => match self.record().filter(|now| now != &holder) {
                    Some(now) => {
                        refreshed = true;
                        self.remember(Some(now));
                        continue;
                    }
                    None => return Ok(answer),
                },
                Ok(answer) if !weather(answer.status()) => return Ok(answer),
                Ok(answer) => {
                    let stated = answer.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()?.parse().ok());
                    // Not a typed refusal (a HEAD's carries no body): not now.
                    let refusal = match envelope(answer).await {
                        untyped if untyped.code == UNTYPED => Error::new(Kind::Unavailable, "calcifer_unavailable", untyped.message),
                        refusal => refusal,
                    };
                    let restarting = refusal.code == STOPPING;
                    let transient = refusal.code == "calcifer_unavailable" || refusal.transient();
                    if !(restarting || repeats && transient) {
                        return Err(if transient { unsent(refusal) } else { refusal });
                    }
                    let stated = stated.or(refusal.retry_after_s).map(Duration::from_secs);
                    (refusal, stated, restarting)
                }
                // Nobody took the connection: Calcifer is gone or restarting.
                Err(e) if e.is_builder() => return Err(unreachable(e)),
                Err(e) if e.is_connect() => (unreachable(e), None, true),
                Err(e) if repeats => (unreachable(e), None, false),
                Err(e) => return Err(unsent(unreachable(e))),
            };
            // The next attempt reads Calcifer's record again: a restart publishes a new one.
            self.remember(None);
            let why = if restarting { RESTARTING } else { error.message.as_str() };
            match backoff.again(why, false, stated) {
                Some((wait, said)) => {
                    output::warn(&said);
                    tokio::time::sleep(wait).await;
                }
                None => {
                    let message = backoff.spent(why);
                    return Err(Error { message, ..error });
                }
            }
        }
    }

    /// Samples `done` until it holds, saying what it waits for every few seconds.
    async fn wait<F, Fut>(&self, what: &str, done: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = bool>,
    {
        let began = Instant::now();
        let mut reported = began;
        while !done().await {
            if reported.elapsed() >= REPORT {
                reported = Instant::now();
                let waited = began.elapsed().as_secs();
                self.say(&format!("waiting for {what} ({waited}s so far)"));
            }
            tokio::time::sleep(SAMPLE).await;
        }
    }

    /// The pid of the Calcifer this client last reached.
    fn pid(&self) -> u32 {
        self.holder().map_or(0, |h| h.pid)
    }

    fn say(&self, message: &str) {
        if !self.0.json {
            anstream::eprintln!("{message}");
        }
    }

    /// `answer`, awaited as long as it takes; once Calcifer `pid` has said nothing for `quiet` since
    /// `since`, every few seconds it says so (`unanswered`). A Calcifer that exits closes its
    /// connections, which ends the wait at once; a live one is its user's to judge.
    async fn heard<T>(&self, pid: u32, since: Instant, quiet: Duration, answer: impl Future<Output = T>) -> T {
        tokio::pin!(answer);
        let first = (since + quiet).max(Instant::now());
        let mut report = tokio::time::interval_at(first.into(), REPORT);
        loop {
            tokio::select! {
                out = &mut answer => return out,
                _ = report.tick() => self.say(&unanswered(pid, since.elapsed())),
            }
        }
    }
}

/// Calcifer process `pid` has not answered for `waited`: what that process is doing, as its OS says.
pub(crate) fn unanswered(pid: u32, waited: Duration) -> String {
    let doing = match process::in_kernel(pid) {
        Some(wchan) if wchan.is_empty() => ", in the kernel (D state)".to_owned(),
        Some(wchan) => format!(", in the kernel (D state, {wchan})"),
        None => match process::Birth::of(pid).and_then(|birth| birth.state()) {
            Ok(process::State::Stopped) => ", stopped".to_owned(),
            Ok(process::State::Gone) => ", gone".to_owned(),
            _ => String::new(),
        },
    };
    format!(
        "waiting on Calcifer (pid {pid}{doing}): no answer for {}s; `cozy doctor` restarts a stuck one",
        waited.as_secs()
    )
}

/// A run's events, resumed from the last durable one whenever the connection closes before the
/// terminal event, across a Calcifer restart too. Dropping it cancels nothing.
pub(crate) struct Events {
    client: Client,
    path: String,
    cursor: u64,
    stream: Option<Response>,
    sse: Sse,
    /// Whether this connection has carried an event.
    received: bool,
    /// The pauses before reconnecting: connections in a row to one Calcifer that closed carrying no
    /// event end it.
    reconnects: Backoff,
    /// The Calcifer (its pid) the last connection was opened on.
    opened_on: Option<u32>,
    /// A reconnect found another Calcifer: the one followed restarted.
    restarted: bool,
    /// Reads past the terminal event until Calcifer ends the stream.
    landing: bool,
    /// The terminal event was read.
    ended: bool,
    done: bool,
}

impl Events {
    /// Reads on after `None` from where it stopped: a landing follower whose run still owes an
    /// output (its Calcifer stopped) waits again.
    pub(crate) fn again(&mut self) {
        self.done = false;
    }

    /// Whether the stream was resumed on a Calcifer that restarted since it was last read; asked
    /// once.
    pub(crate) fn restarted(&mut self) -> bool {
        std::mem::take(&mut self.restarted)
    }

    /// The next event; `None` after the terminal one, or for a landing stream once Calcifer ends
    /// it after that. A dropped call loses no event, but a reconnect's pause and retries start
    /// over: a caller keeps one call going.
    pub(crate) async fn next(&mut self) -> Option<Result<Event, Error>> {
        while !self.done {
            if let Some(event) = self.parsed() {
                if let Some(n) = event.sequence_number() {
                    self.cursor = self.cursor.max(n);
                }
                self.received = true;
                self.ended |= event.is_terminal();
                self.done = self.ended && !self.landing;
                return Some(Ok(event));
            }
            let Some(stream) = self.stream.as_mut() else {
                match self.open().await {
                    Ok(stream) => {
                        // A restart closed the last connection, not a stream with nothing to say.
                        let on = self.client.pid();
                        if self.opened_on.is_some_and(|before| before != on) {
                            (self.restarted, self.reconnects) = (true, Backoff::transfer());
                        }
                        self.opened_on = Some(on);
                        self.stream = Some(stream);
                    }
                    Err(error) => {
                        self.done = true;
                        return Some(Err(error));
                    }
                }
                continue;
            };
            match self.client.heard(self.client.pid(), Instant::now(), 2 * KEEPALIVE, stream.chunk()).await {
                Ok(Some(bytes)) => self.sse.push(&bytes),
                // An ended run's stream ends once its outputs settled; its follower checks them.
                Ok(None) if self.ended => {
                    self.stream = None;
                    self.done = true;
                }
                // A close is not a verdict: reconnect from the cursor, after a pause by the retry rule.
                Ok(None) | Err(_) => {
                    (self.stream, self.sse) = (None, Sse::default());
                    let moved = std::mem::take(&mut self.received);
                    match self.reconnects.after(moved, None) {
                        Some(pause) => tokio::time::sleep(pause).await,
                        None => {
                            self.done = true;
                            let n = self.reconnects.attempts();
                            let said = format!("The run's event stream closed {n} times in a row without an event; the run goes on");
                            return Some(Err(Error::new(Kind::Unavailable, "events_closed", said).next("cozy run ls")));
                        }
                    }
                }
            }
        }
        None
    }

    async fn open(&self) -> Result<Response, Error> {
        let cursor = self.cursor;
        let answer = self.client.send(Ask::Read, |c, base| {
            c.get(format!("{base}{}", self.path))
                .query(&[("starting_after", cursor)])
                .header(header::ACCEPT, "text/event-stream")
        });
        ok(answer.await?).await
    }

    /// The next event this build reads; event types it does not know are skipped.
    fn parsed(&mut self) -> Option<Event> {
        while let Some(data) = self.sse.next() {
            if let Ok(event) = serde_json::from_str(&data) {
                return Some(event);
            }
        }
        None
    }
}

/// One list-level stream (`Client::watch`).
pub(crate) struct Watch<T> {
    client: Client,
    stream: Response,
    sse: Sse,
    /// When it last carried anything, a keepalive included.
    heard: Instant,
    /// A live view's: it shows `silence` in its title instead of saying it.
    pub(crate) quiet: bool,
    /// It said `resync`.
    behind: bool,
    of: std::marker::PhantomData<T>,
}

impl<T: DeserializeOwned> Watch<T> {
    /// The next item, skipping types this build does not read; `None` once the stream closed.
    /// Cancel-safe.
    pub(crate) async fn next(&mut self) -> Option<T> {
        loop {
            while let Some(data) = self.sse.next() {
                if let Ok(item) = serde_json::from_str(&data) {
                    return Some(item);
                }
            }
            let chunk = match self.quiet {
                true => self.stream.chunk().await,
                false => {
                    let pid = self.client.pid();
                    self.client.heard(pid, self.heard, 2 * KEEPALIVE, self.stream.chunk()).await
                }
            };
            match chunk {
                Ok(Some(bytes)) => {
                    self.heard = Instant::now();
                    self.sse.push(&bytes);
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }

    pub(crate) fn quiet(self) -> Self {
        Self { quiet: true, ..self }
    }

    /// Calcifer not answering on this stream past two keepalives, said for a view's title.
    pub(crate) fn silence(&self) -> Option<String> {
        let waited = self.heard.elapsed();
        (waited >= 2 * KEEPALIVE).then(|| unanswered(self.client.pid(), waited))
    }
}

impl Watch<StreamEvent> {
    /// The next change; `None` once the stream closed or fell behind, and from then on: either way
    /// something may have been missed, so the caller subscribes and reads again. Cancel-safe: a
    /// `resync` read just before a call was dropped still counts.
    pub(crate) async fn change(&mut self) -> Option<StreamEvent> {
        if self.behind {
            return None;
        }
        let event = self.next().await;
        self.behind = matches!(event, Some(StreamEvent::Resync));
        event.filter(|_| !self.behind)
    }
}

/// A followed machine log. A closed stream is opened again after the last batch's place, across
/// a Calcifer restart too, so no line arrives twice.
pub(crate) struct LogFollow {
    client: Client,
    path: String,
    lines: u64,
    after: Option<String>,
    stream: Option<Response>,
    sse: Sse,
    /// Whether this connection has carried lines.
    received: bool,
    /// The pauses before reconnecting: connections in a row that closed carrying no line end it.
    reconnects: Backoff,
}

impl LogFollow {
    pub(crate) async fn next(&mut self) -> Result<LogLines, Error> {
        loop {
            if let Some(data) = self.sse.next() {
                if let Ok(LogEvent::Lines { after, text }) = serde_json::from_str(&data) {
                    self.after = Some(after.clone());
                    self.received = true;
                    return Ok(LogLines { after, text });
                }
                continue;
            }
            let Some(stream) = self.stream.as_mut() else {
                self.stream = Some(self.open().await?);
                continue;
            };
            match self.client.heard(self.client.pid(), Instant::now(), 2 * KEEPALIVE, stream.chunk()).await {
                Ok(Some(bytes)) => self.sse.push(&bytes),
                Ok(None) | Err(_) => {
                    (self.stream, self.sse) = (None, Sse::default());
                    let moved = std::mem::take(&mut self.received);
                    match self.reconnects.after(moved, None) {
                        Some(pause) => tokio::time::sleep(pause).await,
                        None => {
                            let n = self.reconnects.attempts();
                            return Err(Error::new(Kind::Unavailable, "log_closed", format!("the log stream closed {n} times in a row without a line")));
                        }
                    }
                }
            }
        }
    }

    async fn open(&self) -> Result<Response, Error> {
        let answer = self.client.send(Ask::Read, |c, base| {
            let request = c
                .get(format!("{base}{}", self.path))
                .header(header::ACCEPT, "text/event-stream");
            match &self.after {
                Some(after) => request.query(&[("after", after)]),
                None => request.query(&[("lines", self.lines)]),
            }
        });
        ok(answer.await?).await
    }
}

/// Server-sent events: the `data:` lines of each event, up to its blank line.
#[derive(Default)]
struct Sse {
    buffer: Vec<u8>,
    data: String,
}

impl Sse {
    fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    fn next(&mut self) -> Option<String> {
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    return Some(std::mem::take(&mut self.data));
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        None
    }
}

/// A path from segments, each percent-encoded.
/// `cozy calcifer`, detached, saying anything before its own log opens into that log.
fn spawn(spec: &ServiceSpec) -> platform::Result<Detached> {
    process::detach(&spec.program, &spec.args, spec.log.as_deref())
}

pub(crate) fn path(segments: &[&str]) -> String {
    let mut url = url::Url::parse("http://calcifer/").expect("a base URL");
    url.path_segments_mut()
        .expect("a base URL")
        .extend(segments);
    url.path().to_owned()
}

/// The home's installed `cozy`, the only one Calcifer and the machine run: the path the home
/// records. With none recorded, or a recorded file gone (a package manager moved it), it is the
/// installer's `<home>/bin/cozy` when that is there, else this cozy (Homebrew's stable path, winget,
/// a distro package), and that is recorded. One rule for every installer; another cozy never
/// replaces a recorded one that is there.
pub(crate) fn installed(home: &Home) -> Result<PathBuf, Error> {
    let record = home.installed();
    let recorded = std::fs::read_to_string(&record).ok().map(|text| PathBuf::from(text.trim_end()));
    if let Some(recorded) = recorded.filter(|path| path.is_file()) {
        return Ok(recorded);
    }
    let failed = |e: String| Error::new(Kind::Internal, "installed_unrecorded", e);
    let chosen = match home.binary() {
        binary if binary.is_file() => binary,
        _ => {
            let exe = std::env::current_exe().map_err(|e| failed(e.to_string()))?;
            exe.canonicalize().ok().and_then(|exe| homebrew(&exe)).unwrap_or(exe)
        }
    };
    home.open().map_err(|e| failed(e.to_string()))?;
    platform::durable::replace(&record, chosen.to_string_lossy().as_bytes(), 0o600)
        .map_err(|e| failed(e.to_string()))?;
    Ok(chosen)
}

/// Homebrew's `<prefix>/bin/cozy` for a cozy it installed at `<prefix>/Cellar/<formula>/<version>/bin/cozy`,
/// a path every upgrade moves.
pub(crate) fn homebrew(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    let cellar = bin.parent()?.parent()?.parent()?;
    if bin.file_name()? != "bin" || cellar.file_name()? != "Cellar" {
        return None;
    }
    Some(cellar.parent()?.join("bin").join(exe.file_name()?))
}

/// The refusal a Calcifer that could not start left.
fn refusal(path: &Path) -> Option<Error> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice::<Envelope>(&raw)
        .ok()
        .map(|e| e.error)
}

/// An answer that means "not now": a timeout, a rate limit, or a gateway or server that isn't serving.
fn weather(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 502 | 503 | 504)
}

/// `body` as JSON, encoded once: every send of a call is the same bytes under its key.
fn json_body(body: &(impl Serialize + ?Sized)) -> Result<bytes::Bytes, Error> {
    let encoded = serde_json::to_vec(body).map_err(|e| Error::new(Kind::Validation, "body_invalid", format!("Cannot encode the request: {e}")))?;
    Ok(encoded.into())
}

fn json(request: RequestBuilder, body: &bytes::Bytes) -> RequestBuilder {
    request.header(header::CONTENT_TYPE, "application/json").body(body.clone())
}

fn unreachable(error: reqwest::Error) -> Error {
    if error.is_builder() {
        return Error::new(Kind::Validation, "request_invalid", format!("Cannot build the request: {error}"));
    }
    Error::new(
        Kind::Unavailable,
        "calcifer_unreachable",
        format!("Calcifer stopped answering: {error}"),
    )
    .remedy("Run the command again")
}

fn vault_uncertain(error: Error) -> Error {
    error.remedy("Check the project's vault status before starting another operation")
}

fn vault_interrupted(reason: &str) -> Error {
    vault_uncertain(Error::new(Kind::Unavailable, "vault_stream_interrupted", format!("Vault operation: {reason}")))
}

fn vault_refusal(status: StatusCode, body: &[u8]) -> Error {
    match serde_json::from_slice::<Envelope>(body) {
        Ok(envelope) => envelope.error,
        // An untyped body may contain a recovery secret; never include it in diagnostics.
        Err(_) => Error::new(Kind::Internal, UNTYPED, format!("Calcifer returned {status} for the vault operation")),
    }
}

async fn ok(answer: Response) -> Result<Response, Error> {
    if answer.status().is_success() {
        Ok(answer)
    } else {
        Err(envelope(answer).await)
    }
}


async fn decode<T: DeserializeOwned>(answer: Response) -> Result<T, Error> {
    let answer = ok(answer).await?;
    let body = answer.bytes().await.map_err(unreachable)?;
    serde_json::from_slice(&body).map_err(|e| {
        Error::new(Kind::Internal, UNTYPED, format!("Cannot read Calcifer's response: {e}"))
    })
}

/// A refusal's typed envelope; a body that is not one is reported as what it was.
async fn envelope(answer: Response) -> Error {
    let status = answer.status();
    let body = answer.bytes().await.unwrap_or_default();
    match serde_json::from_slice::<Envelope>(&body) {
        Ok(envelope) => envelope.error,
        Err(_) => {
            let text = String::from_utf8_lossy(&body);
            let message = match text.trim() {
                "" => format!("Calcifer returned {status}"),
                text => format!("Calcifer returned {status}: {text}"),
            };
            Error::new(Kind::Internal, UNTYPED, message)
        }
    }
}

/// The steps of a start, each with its time, appended to Calcifer's log (`cozy machine logs local calcifer`)
/// beside Calcifer's own: a slow start names its slow step.
struct Steps {
    log: PathBuf,
    since: Instant,
}

impl Steps {
    fn new(log: PathBuf) -> Self {
        Self {
            log,
            since: Instant::now(),
        }
    }

    fn done(&mut self, step: &str) {
        let ms = self.since.elapsed().as_millis();
        let line = format!("{}  INFO cozy: {step} in {ms} ms\n", jiff::Timestamp::now());
        let log = OpenOptions::new().create(true).append(true).open(&self.log);
        if let Ok(mut log) = log {
            let _ = log.write_all(line.as_bytes());
        }
        self.since = Instant::now();
    }
}

/// A write's own `Idempotency-Key`, made once per call.
fn write_key() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS has randomness");
    format!("cli-{}", hex::encode(bytes))
}

#[cfg(test)]
#[path = "calcifer_client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "calcifer_restart_tests.rs"]
mod restart_tests;
