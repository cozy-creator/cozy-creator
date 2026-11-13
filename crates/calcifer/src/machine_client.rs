//! Calcifer's client of one machine's API (`api::machine`). TLS admits exactly the machine's
//! pinned leaf. Control calls and SSE share one HTTP/2 connection; bulk bytes go on [`Bulk`]
//! HTTP/1.1 connections of their own, so parts and ranges move side by side on separate TCP connections. Nothing here
//! judges the machine by its silence: a busy one answers late, never wrongly. Every connection
//! carries TCP keepalive, which the peer's kernel answers however busy its process is, so only a
//! host or path that is gone ends one; bulk callers judge their lanes by measured progress.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use ed25519_dalek::SigningKey;
use futures_util::{Stream, StreamExt as _, TryStreamExt as _};
use reqwest::{Method, RequestBuilder, Response, StatusCode, header};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};
use url::Url;

use api::{
    error::{Envelope, Kind},
    machine::{
        Event, HELD, HeldAnswer, HeldBytes, HeldObject, HeldRequest, LogLines, OutputTarget, Ranges,
        PREVIEW_STEP, RunBatch, RunBatchAnswer, RunEvent, RunList, RunRequest, RunState, SERVER_NAME, StatusFrame, capability, parse_held,
        path,
    },
};

/// A cap is checked when a request opens; an open stream outlives it.
const CAP_SECONDS: i64 = 300;
/// TCP keepalive: a probe after this long without a packet, and again this often; the
/// connection ends once `PROBES` in a row go unanswered by the peer's kernel.
const PROBE: Duration = Duration::from_secs(20);
const PROBES: u32 = 3;

#[derive(Clone, Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The machine refused: its HTTP status and envelope.
    #[error("the machine refused it ({}): {}", .error.code, .error.message)]
    Refused {
        status: u16,
        error: Box<api::error::Error>,
    },
    /// No connection could be made: refused, reset or timed out while connecting, a name that
    /// does not resolve, or TLS that failed.
    #[error("{0}")]
    Unreachable(String),
    /// A connection was made and broke, or the request failed on it.
    #[error("{0}")]
    Transport(String),
    /// It answered what this API does not say: another service on its port, a proxy's error page.
    #[error("{0}")]
    Answer(String),
}

impl Error {
}

/// How each request is authorized.
#[derive(Clone)]
pub(crate) enum Auth {
    /// Nothing: `status` answers identity and the readiness receipt only.
    None,
    /// A fresh machine-scope cap for every request, signed by `key` for the machine `worker`.
    Key {
        key: Arc<SigningKey>,
        worker: String,
    },
}

impl Auth {
    fn header(&self) -> Option<String> {
        let token = match self {
            Auth::None => return None,
            Auth::Key { key, worker } => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                let grant = capability::Grant {
                    machine: worker.clone(),
                    action: capability::MACHINE.into(),
                    expires: now + CAP_SECONDS,
                    ..Default::default()
                };
                capability::mint(key, grant)
            }
        };
        Some(format!("{}{token}", capability::SCHEME))
    }
}

/// One machine, reached at `address` (`host:port`) through its pinned leaf.
#[derive(Clone)]
pub(crate) struct Machine {
    origin: Url,
    pin: [u8; 32],
    auth: Auth,
    control: reqwest::Client,
    private: Option<crate::private_io::Scope>,
}

/// What a ranged `GET` of an output answered: the revision, the output's whole length, the
/// range its body carries (`[first, end)`), and the bytes.
/// A run's newest preview, as its machine answered it: its `ETag`, its step (`<position>/<total>`)
/// and its picture.
pub(crate) struct Preview {
    pub tag: String,
    pub step: String,
    pub media_type: String,
    pub body: Bytes,
}

pub(crate) struct Output {
    pub rev: u64,
    pub length: u64,
    pub range: (u64, u64),
    pub media_type: String,
    pub body: std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>,
}

impl Machine {
    pub(crate) fn new(address: &str, pin: [u8; 32], auth: Auth) -> Result<Self, Error> {
        let origin = Url::parse(&format!("https://{address}/"))
            .map_err(|e| Error::Unreachable(format!("the machine's address {address}: {e}")))?;
        let control = builder(pin, &[b"h2"]).build().map_err(transport)?;
        Ok(Self {
            origin,
            pin,
            auth,
            control,
            private: None,
        })
    }

    pub(crate) fn with_private(&self, scope: crate::private_io::Scope) -> Self {
        Self { private: Some(scope), ..self.clone() }
    }

    /// Feature/authentication probe before any private file bytes cross the wire.
    pub(crate) async fn probe_private(&self) -> Result<(), Error> {
        self.migrate_private().await?;
        let capability: api::machine::ProjectExecutionCapabilities =
            json(send(self.request(Method::GET, path::HEALTH, &[])).await?).await?;
        if !capability.ready() {
            return Err(Error::Refused { status: 409, error: Box::new(api::error::Error::new(
                Kind::Conflict, "project_execution_unavailable", "This worker cannot protect project execution storage.",
            ).next("cozy doctor")) });
        }
        Ok(())
    }

    pub(crate) async fn migrate_private(&self) -> Result<(), Error> {
        let scope = self.private.as_ref().ok_or_else(|| Error::Answer("migration requires a selected project proof".into()))?;
        send(self.request(Method::POST, path::PROJECT_MIGRATE, &[&scope.storage.project_id])).await.map(|_| ())
    }

    pub(crate) async fn legacy_project_bindings(&self, request_ids: &[String]) -> Result<api::machine::ProjectExecutionLegacyLookupAnswer, Error> {
        let scope = self.private.as_ref().ok_or_else(|| Error::Answer("legacy binding lookup requires a selected project proof".into()))?;
        let query = api::machine::ProjectExecutionLegacyLookup { request_ids: request_ids.to_vec() };
        json(send(self.request(Method::POST, path::PROJECT_LEGACY_BINDINGS, &[&scope.storage.project_id]).json(&query)).await?).await
    }

    pub(crate) async fn legacy_bindings(&self, request_ids: &[String]) -> Result<api::machine::ProjectExecutionLegacyLookupAnswer, Error> { self.legacy_project_bindings(request_ids).await }
    pub(crate) async fn migrate_private_bindings(&self, verified: &api::machine::ProjectExecutionMigrationReceipt) -> Result<(), Error> {
        let scope = self.private.as_ref().ok_or_else(|| Error::Answer("verified adoption requires a selected project proof".into()))?;
        let request = api::machine::ProjectExecutionMigrationRequest { legacy_requests: verified.bindings.clone() };
        send(self.request(Method::POST, path::PROJECT_MIGRATE, &[&scope.storage.project_id]).json(&request)).await.map(|_| ())
    }

    pub(crate) async fn migration_receipt(&self) -> Result<api::machine::ProjectExecutionMigrationReceipt, Error> {
        let scope = self.private.as_ref().ok_or_else(|| Error::Answer("migration coverage requires a selected project proof".into()))?;
        json(send(self.request(Method::GET, path::PROJECT_MIGRATE, &[&scope.storage.project_id])).await?).await
    }

    pub(crate) async fn close_private(&self) -> Result<(), Error> {
        self.migrate_private().await?;
        let scope = self.private.as_ref().ok_or_else(|| Error::Answer("closing private storage requires a selected project".into()))?;
        send(self.request(Method::POST, path::PRIVATE_CLOSE, &[&scope.storage.project_id])).await.map(|_| ())
    }

    fn private_headers(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.private {
            Some(scope) => {
                let mut token = header::HeaderValue::from_str(scope.token()).expect("a verified hexadecimal proof token");
                token.set_sensitive(true);
                request.header(api::machine::LOCAL_PRIVATE_DESCRIPTOR, scope.descriptor_header())
                    .header(api::machine::LOCAL_PRIVATE_TOKEN, token)
            },
            None => request,
        }
    }

    /// A bulk lane: HTTP/1.1 connections of its own. Dropping it closes them.
    pub(crate) fn bulk(&self) -> Result<Bulk, Error> {
        let client = builder(self.pin, &[])
            .http1_only()
            .build()
            .map_err(transport)?;
        Ok(Bulk {
            machine: self.clone(),
            client,
        })
    }

    /// The machine's frame. Without a credential: identity, version, capabilities and receipt.
    pub(crate) async fn status(&self) -> Result<StatusFrame, Error> {
        let request = self.request(Method::GET, path::STATUS, &[]);
        json(send(request.header(header::ACCEPT, "application/json")).await?).await
    }

    /// The machine's frame, then again whenever it changes.
    pub(crate) async fn watch(
        &self,
    ) -> Result<impl Stream<Item = Result<StatusFrame, Error>> + Send + 'static, Error> {
        let request = self.request(Method::GET, path::STATUS, &[]);
        let events = sse(send(request.header(header::ACCEPT, "text/event-stream")).await?)?;
        Ok(events.filter_map(|event| async move {
            match event {
                Ok(event) if event.event == "status" => Some(parse(&event.data)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        }))
    }

    /// Starts a rental's idle clock over; answers the frame with the new deadline.
    pub(crate) async fn keepalive(&self) -> Result<StatusFrame, Error> {
        json(send(self.request(Method::POST, path::KEEPALIVE, &[])).await?).await
    }

    pub(crate) async fn remove_model(&self, asked: &api::machine::RemoveModel) -> Result<api::machine::RemovedModel, Error> {
        json(send(self.request(Method::POST, path::MODELS_REMOVE, &[]).json(asked)).await?).await
    }

    /// Submits a run; its state, also when it was accepted before.
    pub(crate) async fn submit(&self, run: &RunRequest) -> Result<RunState, Error> {
        json(send(self.request(Method::POST, path::RUNS, &[]).json(run)).await?).await
    }

    /// Sends `runs` as one admission (`run-batch/1`): each one's state, or its refusal, in their order.
    pub(crate) async fn submit_batch(&self, runs: &RunBatch) -> Result<Vec<Result<RunState, Error>>, Error> {
        let answer: RunBatchAnswer = json(send(self.request(Method::POST, path::RUNS_BATCH, &[]).json(runs)).await?).await?;
        if answer.runs.len() != runs.runs.len() {
            return Err(Error::Answer(format!("the machine answered {} runs of {} sent", answer.runs.len(), runs.runs.len())));
        }
        Ok((answer.runs.into_iter())
            .map(|admitted| match (admitted.state, admitted.refusal) {
                (_, Some(error)) => Err(Error::Refused { status: error.kind.http_status(), error: Box::new(error) }),
                (Some(state), None) => Ok(state),
                (None, None) => Err(Error::Answer("the machine answered a run with neither its state nor a refusal".into())),
            })
            .collect())
    }

    /// This key's upcoming jobs here, nearest first, replacing its last list (cl-967).
    pub(crate) async fn upcoming(&self, list: &api::machine::Upcoming) -> Result<api::machine::AheadAnswer, Error> {
        json(send(self.request(Method::PUT, path::UPCOMING, &[]).json(list)).await?).await
    }

    /// What this key expects demand for here with no job yet, replacing its last set (cl-967).
    pub(crate) async fn prepare(&self, list: &api::machine::Prepare) -> Result<api::machine::AheadAnswer, Error> {
        json(send(self.request(Method::PUT, path::PREPARE, &[]).json(list)).await?).await
    }

    /// The runs this key reads, newest first, before the run numbered `before` (0: the newest).
    pub(crate) async fn runs(&self, before: u64, limit: u32) -> Result<RunList, Error> {
        let request = self.request(Method::GET, path::RUNS, &[]).query(&[("before", before), ("limit", u64::from(limit))]);
        json(send(request).await?).await
    }

    /// The run's log after `after`: its state first, then its events until its outcome. A run
    /// the machine does not have is a 404 refusal.
    pub(crate) async fn events(
        &self,
        id: &str,
        after: u64,
    ) -> Result<impl Stream<Item = Result<RunEvent, Error>> + Send + 'static, Error> {
        let mut request = self.request(Method::GET, path::EVENTS, &[id]);
        request = request.header(header::ACCEPT, "text/event-stream");
        if after > 0 {
            request = request.header("last-event-id", after.to_string());
        }
        let events = sse(send(request).await?)?;
        Ok(events.filter_map(|event| async move {
            match event {
                Ok(event) if Event::NAMES.contains(&event.event.as_str()) => {
                    Some(parse(&event.data))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        }))
    }

    /// The run's newest preview (`path::PREVIEW`); none while it has none. Asking keeps the machine
    /// making them for a while.
    pub(crate) async fn preview(&self, id: &str) -> Result<Option<Preview>, Error> {
        let response = send(self.request(Method::GET, path::PREVIEW, &[id])).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let text = |name: &str| {
            response.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
        };
        let (tag, step, media_type) = (text("etag"), text(PREVIEW_STEP), text("content-type"));
        let bytes = response.bytes().await.map_err(transport)?;
        Ok(Some(Preview { tag, step, media_type, body: bytes }))
    }

    /// Cancels the run; an id the machine never received is recorded canceled.
    pub(crate) async fn cancel(&self, id: &str) -> Result<RunState, Error> {
        json(send(self.request(Method::POST, path::CANCEL, &[id])).await?).await
    }

    /// The newest `lines` whole lines (0: all) of a log the machine keeps.
    pub(crate) async fn log(&self, name: &str, lines: u64) -> Result<Bytes, Error> {
        let request = self.request(Method::GET, path::LOG, &[name]);
        bytes(send(request.query(&[("lines", lines)])).await?).await
    }

    /// A log's lines as [`Self::log`] reads them, or those after `after` (a batch's place), then
    /// each batch as the machine appends it.
    pub(crate) async fn follow_log(
        &self,
        name: &str,
        lines: u64,
        after: Option<&str>,
    ) -> Result<impl Stream<Item = Result<LogLines, Error>> + Send + 'static, Error> {
        let mut request = self.request(Method::GET, path::LOG, &[name]);
        request = match after {
            Some(after) => request.header("last-event-id", after),
            None => request.query(&[("lines", lines)]),
        };
        let events = sse(send(request.header(header::ACCEPT, "text/event-stream")).await?)?;
        Ok(events.filter_map(|event| async move {
            match event {
                Ok(event) if event.event == "lines" => Some(parse(&event.data)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        }))
    }

    /// The ranges of the object (`sha256:<hex>`) the machine holds for this signer; a stored
    /// object is held whole.
    pub(crate) async fn held(&self, digest: &str) -> Result<Ranges, Error> {
        let held = self.held_many(&[digest.to_owned()]).await?;
        (held.into_iter().next().map(|object| object.ranges))
            .ok_or_else(|| Error::Answer("the machine answered for no object".into()))
    }

    /// What the machine holds of the object (`sha256:<hex>`, `length` bytes) as its upload lands,
    /// until it holds all of it.
    pub(crate) async fn follow_held(
        &self,
        digest: &str,
        length: u64,
    ) -> Result<impl Stream<Item = Result<u64, Error>> + Send + 'static, Error> {
        let request = self.request(Method::GET, path::OBJECT, &[digest]);
        let request = request.query(&[("length", length)]);
        let events = sse(send(request.header(header::ACCEPT, "text/event-stream")).await?)?;
        Ok(events.filter_map(|event| async move {
            match event {
                Ok(event) if event.event == "held" => {
                    Some(parse::<HeldBytes>(&event.data).map(|h| h.held))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        }))
    }

    /// What the machine holds of each of `digests`, in one request, in the order asked.
    pub(crate) async fn held_many(&self, digests: &[String]) -> Result<Vec<HeldObject>, Error> {
        let ask = HeldRequest {
            digests: digests.to_vec(),
        };
        let request = self
            .request(Method::POST, path::OBJECTS_HELD, &[])
            .json(&ask);
        let answer: HeldAnswer = json(send(request).await?).await?;
        Ok(answer.held)
    }

    fn request(&self, method: Method, template: &str, args: &[&str]) -> RequestBuilder {
        self.private_headers(authorized(
            self.control.request(method, self.url(template, args)),
            &self.auth,
        ))
    }

    /// `template` with each `{parameter}` filled from `args`, in order, as one path segment.
    fn url(&self, template: &str, args: &[&str]) -> Url {
        let mut url = self.origin.clone();
        let mut args = args.iter();
        let mut segments = url.path_segments_mut().expect("an https URL has a path");
        segments.clear();
        let selected = self.private.as_ref().filter(|_| !template.starts_with(api::machine::PROJECT_EXECUTION_PREFIX));
        let path = if let Some(scope) = selected {
            segments.extend(["v1", "project-execution", scope.storage.project_id.as_str()]);
            template.strip_prefix("/v1/").unwrap_or(template.trim_start_matches('/'))
        } else { template.trim_start_matches('/') };
        segments.extend(path.split('/').map(|part| {
                match part.starts_with('{') {
                    true => *args.next().expect("an argument for each parameter"),
                    false => part,
                }
            }));
        drop(segments);
        url
    }
}

/// HTTP/1.1 connections of one bulk lane.
#[derive(Clone)]
pub(crate) struct Bulk {
    machine: Machine,
    client: reqwest::Client,
}

impl Bulk {
    /// Lands `[first, first + length)` of the object (`sha256:<hex>`, `total` bytes) from `body`;
    /// answers the ranges held after it.
    pub(crate) async fn put(
        &self,
        digest: &str,
        (first, length, total): (u64, u64, u64),
        body: impl Into<reqwest::Body>,
    ) -> Result<Ranges, Error> {
        let mut request = self.request(Method::PUT, path::OBJECT, &[digest]);
        if total > 0 {
            let last = first + length.max(1) - 1;
            request = request.header(
                header::CONTENT_RANGE,
                format!("bytes {first}-{last}/{total}"),
            );
        }
        let request = request
            .header(header::CONTENT_LENGTH, length)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(body);
        held(&send(request).await?)
    }

    /// `range` (`[first, end)`; an empty one: all of it) of an output at revision `rev` (0: its
    /// newest). A moved revision is a 412 refusal, a range past its end a 416.
    pub(crate) async fn get(
        &self,
        target: &OutputTarget,
        (first, end): (u64, u64),
        rev: u64,
    ) -> Result<Output, Error> {
        let mut request = self.request(Method::GET, path::OUTPUT, &[&target.run, &target.output]);
        if target.index > 0 {
            request = request.query(&[("index", target.index)]);
        }
        if !target.member.is_empty() {
            request = request.query(&[("member", &target.member)]);
        }
        if end > first {
            request = request.header(header::RANGE, format!("bytes={first}-{}", end - 1));
        }
        if rev > 0 {
            request = request.header(header::IF_MATCH, format!("\"{rev}\""));
        }
        let response = send(request).await?;
        let headers = response.headers();
        let text = |name: header::HeaderName| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let unexpected = |what: &str| Error::Answer(format!("an output read answered {what}"));
        let rev = text(header::ETAG)
            .trim_matches('"')
            .parse()
            .map_err(|_| unexpected("no revision"))?;
        let media_type = text(header::CONTENT_TYPE);
        let (range, length) = match response.status() {
            StatusCode::PARTIAL_CONTENT => {
                let spelled = text(header::CONTENT_RANGE);
                content_range(&spelled)
                    .ok_or_else(|| unexpected(&format!("Content-Range {spelled:?}")))?
            }
            _ => {
                let length = response
                    .content_length()
                    .ok_or_else(|| unexpected("no length"))?;
                ((0, length), length)
            }
        };
        let body = response.bytes_stream().map_err(transport);
        Ok(Output {
            rev,
            length,
            range,
            media_type,
            body: Box::pin(body),
        })
    }

    fn request(&self, method: Method, template: &str, args: &[&str]) -> RequestBuilder {
        let url = self.machine.url(template, args);
        self.machine.private_headers(authorized(self.client.request(method, url), &self.machine.auth))
    }
}

/// `bytes first-last/length` as (`[first, end)`, length).
fn content_range(value: &str) -> Option<((u64, u64), u64)> {
    let (range, length) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let (first, last): (u64, u64) = (first.parse().ok()?, last.parse().ok()?);
    Some(((first, last + 1), length.parse().ok()?))
}

fn authorized(request: RequestBuilder, auth: &Auth) -> RequestBuilder {
    match auth.header() {
        Some(value) => request.header(header::AUTHORIZATION, value),
        None => request,
    }
}

fn builder(pin: [u8; 32], alpn: &[&[u8]]) -> reqwest::ClientBuilder {
    let mut tls = tls(Arc::new(Pinned::new(pin)));
    tls.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    // A machine's answer never sends this client anywhere else: a pod is not trusted that far.
    tensorfs::fetch::http(&[]).expect("the webpki roots alone")
        .redirect(reqwest::redirect::Policy::none())
        .tls_backend_preconfigured(tls)
        .tcp_nodelay(true)
        .tcp_keepalive(PROBE)
        .tcp_keepalive_interval(PROBE)
        .tcp_keepalive_retries(PROBES)
        .no_proxy()
}

/// Sends `request`: a 2xx response, or the machine's refusal.
async fn send(request: RequestBuilder) -> Result<Response, Error> {
    let response = request.send().await.map_err(transport)?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.bytes().await.unwrap_or_default();
    let error = match serde_json::from_slice::<Envelope>(&body) {
        Ok(envelope) => envelope.error,
        // A HEAD's refusal has no body; another service's is not ours.
        Err(_) if body.is_empty() => api::error::Error::new(
            kind(status),
            format!("http_{}", status.as_u16()),
            status.canonical_reason().unwrap_or("refused"),
        ),
        Err(_) => {
            return Err(Error::Answer(format!(
                "the machine answered {status} without the error envelope: {}",
                String::from_utf8_lossy(&body[..body.len().min(200)])
            )));
        }
    };
    Err(Error::Refused {
        status: status.as_u16(),
        error: Box::new(error),
    })
}

fn kind(status: StatusCode) -> Kind {
    match status.as_u16() {
        401 => Kind::Credential,
        403 => Kind::Forbidden,
        404 => Kind::NotFound,
        409 | 412 => Kind::Conflict,
        507 => Kind::Capacity,
        400..=499 => Kind::Validation,
        503 => Kind::Unavailable,
        _ => Kind::Internal,
    }
}

async fn json<T: DeserializeOwned>(response: Response) -> Result<T, Error> {
    parse(&String::from_utf8_lossy(&bytes(response).await?))
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    serde_json::from_str(text)
        .map_err(|e| Error::Answer(format!("the machine answered unreadable JSON: {e}")))
}

async fn bytes(response: Response) -> Result<Bytes, Error> {
    response.bytes().await.map_err(transport)
}

fn held(response: &Response) -> Result<Ranges, Error> {
    let value = response
        .headers()
        .get(HELD)
        .map(|v| v.to_str().unwrap_or("-"));
    value
        .and_then(parse_held)
        .ok_or_else(|| Error::Answer(format!("the machine answered {HELD} {value:?}")))
}

fn sse(response: Response) -> Result<impl Stream<Item = Result<sse::Event, Error>> + Send + 'static, Error> {
    let streams = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if !streams {
        return Err(Error::Answer(
            "the machine answered a stream that is not SSE".into(),
        ));
    }
    let decoded = (response.bytes_stream(), sse::Decoder::default(), false);
    Ok(futures_util::stream::unfold(decoded, |(mut bytes, mut decoder, mut ended)| async move {
        loop {
            match decoder.next() {
                Some(Ok(event)) => return Some((Ok(event), (bytes, decoder, ended))),
                Some(Err(e)) => return Some((Err(Error::Answer(format!("the machine's SSE: {e}"))), (bytes, decoder, true))),
                None if ended => return None,
                None => {}
            }
            match bytes.next().await {
                Some(Ok(chunk)) => decoder.feed(&chunk),
                Some(Err(e)) => return Some((Err(transport(e)), (bytes, decoder, true))),
                None => ended = true,
            }
        }
    }))
}

/// A transport failure with its causes: `Unreachable` when no connection was made.
fn transport(error: reqwest::Error) -> Error {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    match error.is_connect() {
        true => Error::Unreachable(message),
        false => Error::Transport(message),
    }
}

/// The TLS leaf (DER) the machine at `address` presents, whatever it is. Nothing is sent: the
/// handshake is refused once the leaf is seen.
pub(crate) async fn presented_leaf(address: &str) -> Result<Vec<u8>, Error> {
    let unreachable = |e: std::io::Error| Error::Unreachable(format!("{address}: {e}"));
    let pinned = Arc::new(Pinned::new([0; 32]));
    let tcp = tokio::net::TcpStream::connect(address)
        .await
        .map_err(unreachable)?;
    let name = ServerName::try_from(SERVER_NAME).expect("a DNS name");
    let refused = tokio_rustls::TlsConnector::from(Arc::new(tls(pinned.clone())))
        .connect(name, tcp)
        .await;
    let seen = pinned.seen.lock().unwrap_or_else(|e| e.into_inner()).take();
    match (seen, refused) {
        (Some(leaf), _) => Ok(leaf),
        (None, Err(e)) => Err(unreachable(e)),
        (None, Ok(_)) => Err(Error::Unreachable(format!("{address} presented no TLS leaf"))),
    }
}

/// TLS that admits exactly the leaf whose sha256 is the pin, whatever its name or dates say.
fn tls(pinned: Arc<Pinned>) -> rustls::ClientConfig {
    rustls::ClientConfig::builder_with_provider(pinned.provider.clone())
        .with_safe_default_protocol_versions()
        .expect("aws-lc-rs offers the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(pinned)
        .with_no_client_auth()
}

#[derive(Debug)]
struct Pinned {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
    /// The leaf the peer presented last.
    seen: Mutex<Option<Vec<u8>>>,
}

impl Pinned {
    fn new(pin: [u8; 32]) -> Self {
        Self {
            pin,
            provider: Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
            seen: Mutex::default(),
        }
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(end_entity.to_vec());
        if Sha256::digest(end_entity.as_ref()).as_slice() == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "the machine presented another TLS leaf than the one pinned".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Server-sent events, as the WHATWG `text/event-stream` format reads them: lines end at CRLF, LF or
/// CR, a blank line dispatches an event that has data, and a stream's leading BOM is dropped.
mod sse {
    use std::collections::VecDeque;

    /// One dispatched event: its name (`message` when none was given) and its data lines joined by `\n`.
    #[derive(Debug, Default)]
    pub(crate) struct Event {
        pub event: String,
        pub data: String,
    }

    /// Events from bytes fed in pieces of any size.
    #[derive(Default)]
    pub(crate) struct Decoder {
        line: Vec<u8>,
        pending: Event,
        after_cr: bool,
        started: bool,
        ready: VecDeque<Result<Event, String>>,
    }

    impl Decoder {
        pub(crate) fn feed(&mut self, bytes: &[u8]) {
            for &byte in bytes {
                match byte {
                    b'\n' if std::mem::take(&mut self.after_cr) => {}
                    b'\n' | b'\r' => {
                        self.after_cr = byte == b'\r';
                        let line = std::mem::take(&mut self.line);
                        self.line_ended(&line);
                    }
                    _ => {
                        self.after_cr = false;
                        self.line.push(byte);
                    }
                }
            }
        }

        /// The next event decoded so far, or a line that is not UTF-8.
        pub(crate) fn next(&mut self) -> Option<Result<Event, String>> {
            self.ready.pop_front()
        }

        fn line_ended(&mut self, line: &[u8]) {
            let Ok(mut line) = std::str::from_utf8(line) else {
                self.ready.push_back(Err("a line that is not UTF-8".into()));
                return;
            };
            if !std::mem::replace(&mut self.started, true) {
                line = line.strip_prefix('\u{feff}').unwrap_or(line);
            }
            if line.is_empty() {
                let mut event = std::mem::take(&mut self.pending);
                if event.data.pop().is_some() {
                    if event.event.is_empty() {
                        event.event = "message".into();
                    }
                    self.ready.push_back(Ok(event));
                }
                return;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.pending.event = value.into(),
                "data" => {
                    self.pending.data.push_str(value);
                    self.pending.data.push('\n');
                }
                // Comments (an empty field), `id` and `retry`: nothing here reads them.
                _ => {}
            }
        }
    }
}
