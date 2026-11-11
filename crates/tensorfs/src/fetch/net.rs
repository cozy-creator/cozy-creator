//! The one HTTP stack. [`http`] starts every outbound client cozy makes: its name, 5 s to connect, a dead
//! peer found by TCP keepalive, and rustls over the webpki roots. [`Client`] is the one that moves
//! bytes: a request that receives nothing for [`STALL`] fails (`read_timeout`, which restarts with
//! every read), no proxy, and every address is the egress policy's before it is dialed
//! ([`SourcePolicy`]). Redirects are followed here, not by reqwest: credentials go only to the origin
//! they were given for, and reqwest's own list (Authorization, Cookie) would carry a DPoP proof or
//! SigV4 headers on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
pub use reqwest::header::HeaderMap;
pub(crate) use reqwest::Url;
use rustls::pki_types::CertificateDer;
use tokio::io::AsyncReadExt;

use super::policy::SourcePolicy;
use super::token::{Request as Presented, TokenProvider};
use crate::err::{refuse, Code, Refusal, Result};

const CONNECT: Duration = Duration::from_secs(5);
/// No bytes for this long fails a request (CRT's and Ollama's stall rule).
pub const STALL: Duration = Duration::from_secs(30);
/// A silent connection is probed this often, six times, after six of these with nothing on it.
const PROBE: Duration = Duration::from_secs(5);

/// How every outbound client starts, so no caller picks a name, a connect timeout or TLS: `cozy/<version>` as
/// its User-Agent, 5 s to connect, a peer that went away found by TCP keepalive (a minute at most), and rustls
/// trusting the webpki roots and each DER certificate in `extra`. A caller adds only what is its own: its
/// redirects, [`STALL`] where an answer is one exchange, a pinned verifier.
pub fn http(extra: &[&[u8]]) -> Result<reqwest::ClientBuilder> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in extra {
        roots.add(CertificateDer::from(der.to_vec())).map_err(|error| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("not a CA certificate: {error}") })?;
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("rustls ships its default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(reqwest::Client::builder()
        .user_agent(concat!("cozy/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT)
        .tcp_keepalive(PROBE * 6)
        .tcp_keepalive_interval(PROBE)
        .tcp_keepalive_retries(6)
        .tls_backend_preconfigured(tls))
}

/// The DER certificates of a PEM bundle, for [`http`]; an empty bundle is refused.
pub fn pem_roots(pem: &[u8]) -> Result<Vec<Vec<u8>>> {
    let roots: Vec<CertificateDer<'static>> = rustls::pki_types::pem::PemObject::pem_slice_iter(pem)
        .collect::<std::result::Result<_, _>>()
        .map_err(|error| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("not a PEM certificate bundle: {error}") })?;
    if roots.is_empty() {
        return refuse(Code::UNKNOWN_FORMAT, "the PEM bundle holds no certificate");
    }
    Ok(roots.iter().map(|root| root.to_vec()).collect())
}

/// A response, its body still on the wire, and the URL that answered it.
pub(crate) struct Answer {
    pub url: String,
    pub response: reqwest::Response,
    /// When a redirect led here: until when its `Cache-Control: max-age` lets `url` be asked directly.
    pub until: Option<Instant>,
}

/// An exchange failure may be asked again; the caller's own verdict (its credential callback,
/// its sink) never is.
pub(crate) enum GetError {
    Caller(Refusal),
    Exchange(Refusal),
}

impl GetError {
    pub(crate) fn refusal(self) -> Refusal {
        match self {
            Self::Caller(refusal) | Self::Exchange(refusal) => refusal,
        }
    }
}

impl From<Refusal> for GetError {
    fn from(refusal: Refusal) -> Self {
        Self::Exchange(refusal)
    }
}

impl Answer {
    pub(crate) fn status(&self) -> u16 {
        self.response.status().as_u16()
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.response.headers().get(name)?.to_str().ok()
    }

    /// The wait an answer states (`Retry-After` in seconds).
    pub(crate) fn stated_wait(&self) -> Option<Duration> {
        self.header("retry-after")?.trim().parse().ok().map(Duration::from_secs)
    }

    /// `Content-Range: bytes <first>-<last>/<total>`: its first byte, and its total unless `*`.
    pub(crate) fn content_range(&self) -> Option<(u64, Option<u64>)> {
        let (span, total) = self.header("content-range")?.strip_prefix("bytes ")?.split_once('/')?;
        let first = span.split_once('-')?.0.trim().parse().ok()?;
        let total = match total.trim() {
            "*" => None,
            total => Some(total.parse().ok()?),
        };
        Some((first, total))
    }

    /// The next piece of body, `None` at the end.
    pub(crate) async fn frame(&mut self) -> Result<Option<Bytes>> {
        self.response.chunk().await.map_err(|error| Refusal { code: Code::TRANSFER_FAILED, detail: format!("the body failed: {}", chain(&error)) })
    }

    /// The start of the body, its whitespace collapsed: what the server said.
    pub(crate) async fn said(&mut self) -> String {
        let mut body = Vec::new();
        while body.len() < 512 {
            match self.frame().await {
                Ok(Some(data)) => body.extend_from_slice(&data),
                _ => break,
            }
        }
        body.truncate(512);
        String::from_utf8_lossy(&body).split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// A request body: bytes, or the first `length` bytes of a file, counted into `sent`.
pub(crate) enum Body {
    Bytes(Bytes),
    File { file: std::fs::File, length: u64, sent: Arc<AtomicU64> },
}

impl From<Body> for reqwest::Body {
    fn from(body: Body) -> reqwest::Body {
        let (file, length, sent) = match body {
            Body::Bytes(bytes) => return bytes.into(),
            Body::File { file, length, sent } => (tokio::fs::File::from_std(file), length, sent),
        };
        let frames = futures_util::stream::unfold((file, length), move |(mut file, left)| {
            let sent = sent.clone();
            async move {
                if left == 0 {
                    return None;
                }
                let mut buf = vec![0; left.min(1 << 20) as usize];
                let (frame, left) = match file.read(&mut buf).await {
                    Ok(0) => (Err(std::io::ErrorKind::UnexpectedEof.into()), 0),
                    Ok(n) => {
                        buf.truncate(n);
                        sent.fetch_add(n as u64, Ordering::Relaxed);
                        (Ok(Bytes::from(buf)), left - n as u64)
                    }
                    Err(error) => (Err::<Bytes, std::io::Error>(error), 0),
                };
                Some((frame, (file, left)))
            }
        });
        reqwest::Body::wrap_stream(frames)
    }
}

/// The client every fetch uses. Cheap to clone: clones share one pool.
#[derive(Clone)]
pub struct Client {
    inner: reqwest::Client,
    policy: SourcePolicy,
    /// The CA certificates it trusts beyond the webpki roots (DER).
    roots: Arc<[Vec<u8>]>,
}

impl Client {
    /// A client bound by `policy`, trusting the webpki roots.
    pub fn new(policy: SourcePolicy) -> Client {
        Client::trusting(policy, &[]).expect("the webpki roots alone")
    }

    /// A client that also trusts the CA certificates in `pem` (a private origin's).
    pub fn with_roots(policy: SourcePolicy, pem: &[u8]) -> Result<Client> {
        let roots = pem_roots(pem)?;
        Client::trusting(policy, &roots.iter().map(Vec::as_slice).collect::<Vec<_>>())
    }

    fn trusting(policy: SourcePolicy, extra: &[&[u8]]) -> Result<Client> {
        let inner = http(extra)?
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .read_timeout(STALL)
            .dns_resolver(Arc::new(Checked(policy.clone())))
            .build()
            .expect("reqwest builds with a rustls config");
        Ok(Client { inner, policy, roots: extra.iter().map(|root| root.to_vec()).collect() })
    }

    pub(crate) fn policy(&self) -> &SourcePolicy {
        &self.policy
    }

    /// Whether `other` reaches the same hosts the same way: its policy and its roots.
    pub(crate) fn same(&self, other: &Client) -> bool {
        let (a, b) = (&self.policy, &other.policy);
        (a.any_host, a.allow_local, a.plaintext, a.max_redirects, &a.allowed_hosts, &self.roots)
            == (b.any_host, b.allow_local, b.plaintext, b.max_redirects, &b.allowed_hosts, &other.roots)
    }

    /// GET `url`, following the redirects the policy allows. Credentials go only to the origin they
    /// were given for: another origin gets only the `range`.
    pub(crate) async fn get_authorized(&self, url: &str, headers: &[(String, String)], mut provider: Option<&dyn TokenProvider>) -> std::result::Result<Answer, GetError> {
        let (mut url, mut headers, mut challenged, mut until) = (url.to_owned(), headers.to_vec(), false, None::<Instant>);
        for _ in 0..=self.policy.max_redirects + 1 {
            let mut presented = headers.clone();
            if let Some(provider) = provider {
                presented.extend(provider.headers(&Presented { method: "GET", url: &url, headers: &headers, payload: None }).await.map_err(GetError::Caller)?);
            }
            let mut answer = self.send("GET", &url, &presented, None).await?;
            let status = answer.status();
            if let Some(provider) = provider {
                let sent = Presented { method: "GET", url: &url, headers: &presented, payload: None };
                if provider.retry(&sent, status, answer.response.headers()).map_err(GetError::Caller)? && !challenged {
                    tracing::info!(status, url = %shown(&url), "asked again with a renewed credential");
                    challenged = true;
                    continue;
                }
            }
            if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                if let Some(encoding) = answer.header("content-encoding").filter(|e| !e.trim().is_empty() && !e.trim().eq_ignore_ascii_case("identity")) {
                    return Err(GetError::Exchange(Refusal { code: Code::TRANSFER_FAILED, detail: format!("{}: content-encoding {encoding:?} against `accept-encoding: identity`", shown(&url)) }));
                }
                answer.until = until;
                return Ok(answer);
            }
            let Some(next) = answer.header("location").and_then(|at| join(&url, at)) else {
                return Err(GetError::Exchange(Refusal { code: Code::REDIRECT_REFUSED, detail: format!("{}: {status} without a usable Location", shown(&url)) }));
            };
            let age = max_age(answer.response.headers()).map(|age| Instant::now() + age);
            until = match (until, age) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            // Read to its end, so its connection goes back to the pool.
            let _ = answer.said().await;
            if !same_origin(&url, &next) {
                headers.retain(|(name, _)| name.eq_ignore_ascii_case("range"));
                provider = None;
            }
            (url, challenged) = (next, false);
        }
        Err(GetError::Exchange(Refusal { code: Code::REDIRECT_REFUSED, detail: format!("{}: redirected past the policy's {} hops", shown(&url), self.policy.max_redirects) }))
    }

    /// One request, redirects not followed: `body` (none: empty) with `headers`.
    pub(crate) async fn send(&self, method: &str, url: &str, headers: &[(String, String)], body: Option<Body>) -> Result<Answer> {
        // The scheme, the allowlist and an address literal: a named host is checked as it resolves.
        let (policy, checking) = (self.policy.clone(), url.to_owned());
        tokio::task::spawn_blocking(move || policy.check(&checking))
            .await
            .map_err(|error| Refusal { code: Code::TRANSFER_FAILED, detail: format!("name resolution: {error}") })??;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| Refusal { code: Code::SOURCE_NOT_ALLOWED, detail: format!("{method}: {error}") })?;
        let mut request = self.inner.request(method, url).header("accept-encoding", "identity");
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(body) = body {
            request = request.body(reqwest::Body::from(body));
        }
        match request.send().await {
            Ok(response) => Ok(Answer { url: url.to_owned(), response, until: None }),
            Err(error) => {
                if error.is_connect() {
                    // Answers nothing connects to are asked of the resolver again next time.
                    if let Some(host) = Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)) {
                        self.policy.resolved.forget(&host);
                    }
                }
                Err(refusal(url, &error))
            }
        }
    }
}

/// The policy as reqwest's resolver: only checked addresses are dialed.
struct Checked(SourcePolicy);

impl reqwest::dns::Resolve for Checked {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let (policy, host) = (self.0.clone(), name.as_str().to_owned());
        Box::pin(async move {
            let addrs = tokio::task::spawn_blocking(move || policy.addresses(&host)).await??;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A response's `Cache-Control: max-age`.
fn max_age(headers: &HeaderMap) -> Option<Duration> {
    let control = headers.get("cache-control")?.to_str().ok()?;
    let age = control.split(',').find_map(|d| d.trim().strip_prefix("max-age="))?;
    age.trim().parse().ok().map(Duration::from_secs)
}

/// `url` without its query: a presigned URL's query is its credential.
pub(crate) fn shown(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

pub(crate) fn same_origin(a: &str, b: &str) -> bool {
    let origin = |url: &str| Url::parse(url).ok().map(|url| url.origin());
    origin(a).is_some_and(|a| Some(a) == origin(b))
}

/// `location` against the URL that answered with it.
pub(crate) fn join(base: &str, location: &str) -> Option<String> {
    Url::parse(base).ok()?.join(location).ok().map(String::from)
}

/// A failed exchange as a refusal: the policy's own where it made one, else the error's chain.
fn refusal(url: &str, error: &reqwest::Error) -> Refusal {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        if let Some(refusal) = cause.downcast_ref::<Refusal>() {
            return refusal.clone();
        }
        if cause.downcast_ref::<std::io::Error>().is_some_and(crate::descriptors::exhausted) {
            return Refusal { code: Code::FD_HEADROOM, detail: format!("{}: {}", shown(url), chain(error)) };
        }
        if let Some(tls) = cause.downcast_ref::<rustls::Error>() {
            if matches!(tls, rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented | rustls::Error::PeerIncompatible(_)) {
                return Refusal { code: Code::TLS_UNTRUSTED, detail: format!("{}: TLS refused: {tls}", shown(url)) };
            }
        }
        source = cause.source();
    }
    Refusal { code: Code::TRANSFER_FAILED, detail: format!("{}: {}", shown(url), chain(error)) }
}

/// An error and its causes, without any URL (a presigned one is a credential).
fn chain(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    if let Some(url) = error.url() {
        text = text.replace(url.as_str(), shown(url.as_str()));
    }
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}
