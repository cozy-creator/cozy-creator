//! Hub access (th-245). A run reads its Hub's package catalog anonymously, and its models through
//! TensorFS's distribution `Endpoint` (`origin`) under its signer's run capability when its request
//! carried one (cl-675: a workload token bound to this machine's key), or anonymously without one. A
//! machine never holds an account's sign-in.
use base64::Engine as _;
use hub_client::{workload::WorkloadKey, Credential};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tensorfs::{
    err::{refuse, Code},
    fetch::token::{HeaderMap, Request, TokenProvider},
};

pub use hub_client::types;

/// `scheme://host:port`, lowercase, default port spelled: one key per origin.
pub fn origin_key(origin: &str) -> Option<String> {
    let origin = origin.trim_end_matches('/');
    let (scheme, rest) = origin.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, Some(port.parse::<u16>().ok()?)),
        _ => (rest, None),
    };
    let port = match (port, scheme.as_str()) {
        (Some(port), _) => port,
        (None, "https") => 443,
        (None, "http") => 80,
        _ => return None,
    };
    Some(format!("{scheme}://{}:{port}", host.to_ascii_lowercase()))
}

fn loopback_host(key: &str) -> bool {
    let host = key
        .split_once("://")
        .and_then(|(_, rest)| rest.rsplit_once(':'))
        .map(|(host, _)| host)
        .unwrap_or_default();
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// An HTTPS origin, or plaintext HTTP on loopback only.
pub fn valid_origin(origin: &str) -> bool {
    match origin_key(origin) {
        Some(key) => {
            key.starts_with("https://") || key.starts_with("http://") && loopback_host(&key)
        }
        None => false,
    }
}

/// The origin of an absolute URL: everything before its path.
fn origin_of(url: &str) -> &str {
    let after = url.find("://").map_or(0, |at| at + 3);
    url[after..]
        .find(['/', '?', '#'])
        .map_or(url, |end| &url[..after + end])
}

/// Whether `url` is a file the run's Hub publishes (its index's file door).
pub fn publishes(source: &Source, url: &str) -> bool {
    origin_key(origin_of(url)).is_some_and(|key| origin_key(&source.origin) == Some(key))
}

pub(crate) fn pem(der: &[u8]) -> String {
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<_> = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        lines.join("\n")
    )
}

/// A Hub refusal: the HTTP-level detail, never a token.
#[derive(Debug, Clone)]
pub struct Refusal(pub String);

/// Blocks on one Hub call. TensorD's run code is synchronous; its Hub calls share one
/// current-thread runtime, made once. A thread already inside an async context hands the call
/// to a helper thread instead.
///
/// The seam xs-040 U11's runtime unification folds into TensorD's main runtime.
pub(crate) fn block<T: Send>(call: impl Future<Output = T> + Send) -> T {
    static HUB: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = HUB.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("the Hub runtime")
    });
    if tokio::runtime::Handle::try_current().is_err() {
        return runtime.block_on(call);
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| runtime.block_on(call))
            .join()
            .expect("a Hub call")
    })
}

/// HTTPS to one Hub: the public roots and its granted CA, no redirects. A live Hub that is
/// merely slow is waited for; a dead one is found by TCP keepalive (probes after 30 s of
/// silence, 5 s apart), as TensorFS's downloader finds its own. For calls on the Hub runtime
/// (`block`).
pub fn http(ca_der: Option<&[u8]>) -> Result<reqwest::Client, Refusal> {
    static CLIENTS: Mutex<Vec<(Option<Vec<u8>>, reqwest::Client)>> = Mutex::new(Vec::new());
    pooled(&CLIENTS, ca_der)
}

/// `http` for the executors' gateway (`gateway`), which runs on its own runtime.
pub fn gateway_http(ca_der: Option<&[u8]>) -> Result<reqwest::Client, Refusal> {
    static CLIENTS: Mutex<Vec<(Option<Vec<u8>>, reqwest::Client)>> = Mutex::new(Vec::new());
    pooled(&CLIENTS, ca_der)
}

type Pool = Mutex<Vec<(Option<Vec<u8>>, reqwest::Client)>>;

fn pooled(pool: &Pool, ca_der: Option<&[u8]>) -> Result<reqwest::Client, Refusal> {
    let mut clients = pool.lock().unwrap();
    if let Some((_, client)) = clients.iter().find(|(ca, _)| ca.as_deref() == ca_der) {
        return Ok(client.clone());
    }
    let unusable = |why: String| Refusal(format!("the Hub's TLS: {why}"));
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    if let Some(der) = ca_der {
        roots
            .add(der.to_vec().into())
            .map_err(|e| unusable(e.to_string()))?;
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| unusable(e.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    const PROBE: Duration = Duration::from_secs(5);
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(reqwest::redirect::Policy::none())
        .tcp_keepalive(PROBE * 6)
        .tcp_keepalive_interval(PROBE)
        .tcp_keepalive_retries(6)
        .build()
        .map_err(|e| unusable(e.to_string()))?;
    clients.push((ca_der.map(<[u8]>::to_vec), client.clone()));
    Ok(client)
}

/// This machine's workload key (cl-675): made at its start, held in memory only, never given to an
/// executor. Every run capability it redeems binds it, and every token it holds proves it.
pub fn workload_key() -> &'static WorkloadKey {
    static KEY: OnceLock<WorkloadKey> = OnceLock::new();
    KEY.get_or_init(WorkloadKey::generate)
}

/// One signer's run capability at one Hub (cl-675): the newest its computer signed for this
/// machine, redeemed for a token bound to the workload key (AuthKit's jwt-bearer grant), and again
/// before each token expires while a run it serves is not over; the Hub decides each time. Every
/// read and upload for the signer's runs there presents the token with a fresh DPoP proof, its
/// executors' through the gateway (`gateway`), which they reach with `secret`. In memory only.
pub struct Capability {
    actor: String,
    origin: String,
    ca_der: Option<Vec<u8>>,
    /// What the signer's executors present at the gateway for it.
    secret: String,
    held: Mutex<Held>,
    /// Its Hub's token endpoint, once found.
    endpoint: Mutex<Option<String>>,
    /// Wakes its renewal: a capability that grants more, a run that needs it.
    changed: Condvar,
    token: watch::Sender<Token>,
    runs: Mutex<Served>,
}

#[derive(Default)]
struct Held {
    /// The newest capability, when it was signed and what it grants (its operations and rental).
    signed: String,
    issued: i64,
    grants: String,
    /// What the token held was redeemed for.
    redeemed: String,
    /// When to redeem again (unix ms).
    renew_at: i64,
}

/// The runs a capability serves: renewed while one of them is not over.
#[derive(Default)]
struct Served {
    runs: BTreeSet<String>,
    /// Runs being accepted, which the engine knows of only once they are: live meanwhile.
    accepting: BTreeSet<String>,
    /// A renewal runs for them.
    renewing: bool,
}

/// A run being accepted under a capability: live to its renewal until it is dropped, once the
/// engine holds the run (or refused it).
pub struct Accepting(Arc<Capability>, String);

impl Drop for Accepting {
    fn drop(&mut self) {
        self.0.runs.lock().unwrap().accepting.remove(&self.1);
    }
}

/// The token a capability was last redeemed for, and why there is none.
#[derive(Clone, Default)]
struct Token {
    access: String,
    expires_unix_ms: i64,
    /// A renewal runs: an expired or absent token is waited for.
    renewing: bool,
    /// The Hub's last refusal of the capability.
    refused: Option<String>,
}

/// What a capability says of itself, unverified: the Hub checks its signature.
struct Claims {
    issued: i64,
    key: String,
    grants: String,
}

fn claims(capability: &str) -> Option<Claims> {
    let payload = capability.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(Claims {
        issued: claims["iat"].as_i64()?,
        key: claims["cnf"]["jkt"].as_str()?.to_owned(),
        grants: serde_json::json!([claims["authorization_details"], claims["rental"]]).to_string(),
    })
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl Capability {
    /// `actor`'s at `hub`, given `hub.capability` unless the one held was signed later; one that
    /// grants more is redeemed before its runs read. None for a request that carries none. A
    /// capability signed for another workload key (this machine's TensorD started again since) is
    /// refused: its computer signs one for this key.
    pub fn offer(actor: &str, hub: &protocol::machine::HubAccess) -> Result<Option<Arc<Capability>>, (&'static str, String)> {
        let Some(origin) = origin_key(&hub.origin).filter(|_| !hub.capability.is_empty()) else {
            return Ok(None);
        };
        let offered = claims(&hub.capability).ok_or(("invalid_capability", "the run capability is not a signed capability".into()))?;
        if offered.key != workload_key().thumbprint() {
            return Err(("workload_key_changed", "the run capability binds another workload key: this machine's TensorD started again since".into()));
        }
        let capability = HELD.lock().unwrap().entry((actor.to_string(), origin)).or_insert_with(|| {
            let mut secret = [0u8; 32];
            aws_lc_rs::rand::SecureRandom::fill(&aws_lc_rs::rand::SystemRandom::new(), &mut secret).expect("random bytes");
            Arc::new(Capability {
                actor: actor.into(),
                origin: hub.origin.trim_end_matches('/').into(),
                ca_der: Some(hub.ca_der.clone()).filter(|der| !der.is_empty()),
                secret: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
                held: Mutex::default(),
                endpoint: Mutex::default(),
                changed: Condvar::new(),
                token: watch::Sender::new(Token::default()),
                runs: Mutex::default(),
            })
        }).clone();
        let mut held = capability.held.lock().unwrap();
        // Its computer signs one at each send, one send at a time: of two signed in one second, the later
        // to arrive is the newer.
        if offered.issued >= held.issued && hub.capability != held.signed {
            // One that grants otherwise, or follows a refused one, is redeemed before anything reads
            // again: the token held is for the old grant, and reads wait for the new one.
            if offered.grants != held.redeemed || capability.token.borrow().refused.is_some() {
                held.renew_at = 0;
                capability.token.send_modify(|token| *token = Token { renewing: true, ..Token::default() });
            }
            (held.signed, held.issued, held.grants) = (hub.capability.clone(), offered.issued, offered.grants);
            capability.changed.notify_all();
        }
        drop(held);
        Ok(Some(capability))
    }

    /// The one the gateway's `secret` names.
    pub fn by_secret(secret: &str) -> Option<Arc<Capability>> {
        let held = HELD.lock().unwrap();
        held.values().find(|capability| !secret.is_empty() && capability.secret == secret).cloned()
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn ca_der(&self) -> Option<&[u8]> {
        self.ca_der.as_deref()
    }

    /// The account its runs read as: the one the Hub named in the token it redeemed this
    /// capability for (never the capability's own claims), waited for while a renewal runs; none
    /// once the Hub refused it or no run renews it.
    pub fn account(&self) -> Option<String> {
        let token = block(self.current())?;
        let payload = token.split('.').nth(1)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        claims[ACCOUNT_CLAIM].as_str().filter(|account| !account.is_empty()).map(str::to_owned)
    }

    /// It serves run `run` of `engine` too, from before the run is accepted (its preparation reads
    /// at once): it is renewed until every run it serves is over.
    pub fn serves(self: &Arc<Self>, run: &str, engine: &Arc<crate::execution::Engine>) -> Accepting {
        let mut runs = self.runs.lock().unwrap();
        runs.runs.insert(run.to_string());
        runs.accepting.insert(run.to_string());
        let accepting = Accepting(self.clone(), run.to_string());
        if runs.renewing {
            return accepting;
        }
        runs.renewing = true;
        // A renewal asks the Hub again: its reads wait for that answer.
        self.token.send_modify(|token| (token.renewing, token.refused) = (true, None));
        let (capability, engine) = (self.clone(), Arc::downgrade(engine));
        let spawned = std::thread::Builder::new().name("capability-renewal".into()).spawn(move || capability.renew(engine));
        if let Err(error) = spawned {
            runs.renewing = false;
            self.token.send_modify(|token| token.renewing = false);
            tracing::warn!(origin = %self.origin, "renewing the run capability: {error}");
        }
        accepting
    }

    /// Redeems the capability when a token is due, while a run it serves is not over: a refusal
    /// stands until a newer capability comes; a Hub that does not answer is asked again, backing
    /// off to a minute. Its own lifetime is the token's: it renews two thirds into it.
    fn renew(self: Arc<Self>, engine: std::sync::Weak<crate::execution::Engine>) {
        let mut failing = Duration::ZERO;
        loop {
            {
                let mut served = self.runs.lock().unwrap();
                let engine = engine.upgrade();
                let Served { runs, accepting, .. } = &mut *served;
                // A run by its caller's id, as its signer named it.
                runs.retain(|id| {
                    accepting.contains(id)
                        || engine.as_ref().is_some_and(|engine| engine.get_public(&self.actor, id).is_ok_and(|run| !run.state.terminal()))
                });
                if served.runs.is_empty() {
                    served.renewing = false;
                    self.token.send_modify(|token| token.renewing = false);
                    return;
                }
            }
            let held = self.held.lock().unwrap();
            let wait = Duration::from_millis(held.renew_at.saturating_sub(now_ms()).max(0) as u64).max(failing);
            if !wait.is_zero() {
                let _ = self.changed.wait_timeout(held, wait).unwrap();
                continue;
            }
            let (signed, grants) = (held.signed.clone(), held.grants.clone());
            drop(held);
            match self.redeem(&signed) {
                Ok(token) => {
                    failing = Duration::ZERO;
                    let mut held = self.held.lock().unwrap();
                    // A newer capability came meanwhile: its own redemption is due.
                    if held.signed != signed {
                        continue;
                    }
                    let life = token.expires_unix_ms - now_ms();
                    held.renew_at = now_ms() + life * 2 / 3;
                    held.redeemed = grants;
                    drop(held);
                    tracing::info!(origin = %self.origin, expires_in_s = life / 1000, "redeemed the run capability");
                    self.token.send_modify(|held| (held.access, held.expires_unix_ms, held.refused) = (token.access, token.expires_unix_ms, None));
                }
                // A refusal of one a newer capability replaced meanwhile is not that one's.
                Err(_) if self.held.lock().unwrap().signed != signed => continue,
                // It stands until a newer capability comes, whose run starts another renewal.
                Err(error) if !error.retry() => {
                    tracing::warn!(origin = %self.origin, "the Hub refused the run capability: {error}");
                    self.runs.lock().unwrap().renewing = false;
                    self.token.send_modify(|token| (token.refused, token.renewing) = (Some(error.to_string()), false));
                    return;
                }
                Err(error) => {
                    failing = (failing * 2).clamp(Duration::from_secs(1), Duration::from_secs(60));
                    tracing::warn!(origin = %self.origin, retry_s = failing.as_secs(), "redeeming the run capability: {error}");
                }
            }
        }
    }

    /// `signed` redeemed at this Hub's token endpoint for a token bound to the workload key.
    fn redeem(&self, signed: &str) -> Result<Token, hub_client::Error> {
        let http = http(self.ca_der.as_deref()).map_err(|e| hub_client::Error::Call(e.0))?;
        let hub = hub_client::Client::connect(&self.origin, http, Credential::Anonymous);
        let known = self.endpoint.lock().unwrap().clone();
        let endpoint = match known {
            Some(endpoint) => endpoint,
            None => {
                let endpoint = asked(&self.origin, || hub.token_endpoint())?;
                self.endpoint.lock().unwrap().insert(endpoint).clone()
            }
        };
        let token = asked(&self.origin, || hub.redeem(workload_key(), &endpoint, signed))?;
        Ok(Token {
            access: token.access_token,
            expires_unix_ms: now_ms() + token.expires_in * 1000,
            ..Default::default()
        })
    }

    /// The token to present now: a live one, waited for while a renewal runs; none once the Hub
    /// refused the capability or no run renews it, when the read goes anonymously (public releases).
    async fn current(&self) -> Option<String> {
        let mut token = self.token.subscribe();
        loop {
            {
                let held = token.borrow_and_update();
                if !held.access.is_empty() && held.expires_unix_ms > now_ms() + 5_000 {
                    return Some(held.access.clone());
                }
                if held.refused.is_some() || !held.renewing {
                    return None;
                }
            }
            token.changed().await.ok()?;
        }
    }

    /// Why a read under it was refused: the Hub's refusal, or a token no run renews.
    fn why(&self) -> String {
        let held = self.token.borrow();
        match (&held.refused, held.expires_unix_ms) {
            (Some(refused), _) => format!("the Hub refused the run's capability: {refused}"),
            (None, 0) => "the run's capability was never redeemed".into(),
            (None, at) => format!("the run's capability token expired at {}, and no run of its signer here renews it", utc(at)),
        }
    }

    /// The headers that present the token for `method url`: `DPoP <token>` and a fresh proof; none
    /// without a live token.
    pub async fn authorize(&self, method: &str, url: &str) -> Vec<(String, String)> {
        let Some(token) = self.current().await else { return vec![] };
        let proof = workload_key().proof(method, url, Some(&token));
        vec![("authorization".into(), format!("DPoP {token}")), ("dpop".into(), proof)]
    }

    /// Whether a request refused with `status` after presenting `sent` (its authorization) may be
    /// asked again: a token renewed since it was sent.
    pub fn renewed_since(&self, status: u16, sent: &str) -> bool {
        let access = &self.token.borrow().access;
        status == 401 && !sent.is_empty() && !access.is_empty() && sent != format!("DPoP {access}")
    }
}

/// The claim naming the account a workload token reads as (the Hub's `AccountClaim`).
const ACCOUNT_CLAIM: &str = "https://tensorhub.com/claims/account";

/// Each signer's capability at each Hub, for the life of the process.
static HELD: Mutex<BTreeMap<(String, String), Arc<Capability>>> = Mutex::new(BTreeMap::new());

/// `unix_ms` as UTC, to the second.
fn utc(unix_ms: i64) -> String {
    match time::OffsetDateTime::from_unix_timestamp(unix_ms / 1000) {
        Ok(t) => format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", t.year(), t.month() as u8, t.day(), t.hour(), t.minute(), t.second()),
        Err(_) => format!("unix {unix_ms} ms"),
    }
}

#[async_trait::async_trait]
impl TokenProvider for Capability {
    async fn headers(&self, request: &Request<'_>) -> tensorfs::err::Result<Vec<(String, String)>> {
        Ok(self.authorize(request.method, request.url).await)
    }

    /// A 401 for a token renewed since is asked once more; the newest refused fails the read.
    fn retry(&self, sent: &Request<'_>, status: u16, _: &HeaderMap) -> tensorfs::err::Result<bool> {
        if status != 401 {
            return Ok(false);
        }
        let presented = sent.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("authorization")).map(|(_, value)| value.as_str()).unwrap_or_default();
        if self.renewed_since(status, presented) {
            return Ok(true);
        }
        refuse(Code::CREDENTIAL_REQUIRED, self.why())
    }
}

/// Where a run reads a Hub, and its signer's capability there when its request carried one.
#[derive(Clone)]
pub struct Source {
    pub origin: String,
    pub ca_der: Option<Vec<u8>>,
    pub capability: Option<Arc<Capability>>,
}

impl Source {
    pub fn new(origin: &str, ca_der: Option<Vec<u8>>, capability: Option<Arc<Capability>>) -> Result<Self, Refusal> {
        if !valid_origin(origin) {
            return Err(Refusal(format!("{origin} is not an HTTPS origin")));
        }
        Ok(Self { origin: origin.trim_end_matches('/').to_string(), ca_der, capability })
    }

    /// Its capability, for `endpoint` only when that is this Hub.
    pub fn capability_at(&self, endpoint: &str) -> Option<Arc<Capability>> {
        let here = origin_key(endpoint).is_some_and(|key| origin_key(&self.origin) == Some(key));
        self.capability.clone().filter(|_| here)
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// A Hub's package catalog, read anonymously.
pub struct Catalog {
    origin: String,
    hub: hub_client::Client,
}

impl Catalog {
    pub fn new(source: &Source) -> Result<Self, Refusal> {
        let http = http(source.ca_der.as_deref())?;
        let hub = hub_client::Client::connect(&source.origin, http, Credential::Anonymous);
        Ok(Self { origin: source.origin.clone(), hub })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// A package and its releases.
    pub fn package(&self, org: &str, name: &str) -> Result<types::PackageCard, hub_client::Error> {
        asked(&self.origin, || self.hub.get_package(org, name))
    }

    /// The exact files of one committed release (`python_version` picks its project wheel):
    /// its documents' bytes and its wheels behind presigned URLs, which expire.
    pub fn install_plan(
        &self,
        org: &str,
        name: &str,
        release: &str,
        python_version: Option<&str>,
    ) -> Result<types::PackageInstallPlan, hub_client::Error> {
        asked(&self.origin, || self.hub.get_package_download_plan(org, name, release, python_version))
    }
}

/// The last resort (`design/stalls.md`): a catalog answer is one small JSON document, and the
/// Hub gives no progress on it but the answer itself. Each ask gets `ANSWER`; a silent or failed
/// ask is asked again with backoff, and the Hub's own refusal or `ASKS` failures in a row end it.
const ANSWER: Duration = Duration::from_secs(60);
const ASKS: u32 = 3;

fn asked<T: Send, F: Future<Output = Result<T, hub_client::Error>> + Send>(
    origin: &str,
    ask: impl Fn() -> F + Send + Sync,
) -> Result<T, hub_client::Error> {
    block(async {
        let mut attempt = 0;
        loop {
            let failed = match tokio::time::timeout(ANSWER, ask()).await {
                Ok(Ok(answer)) => return Ok(answer),
                Ok(Err(error)) if !error.retry() => return Err(error),
                Ok(Err(error)) => error,
                Err(_) => hub_client::Error::Call(format!("{origin} gave no answer in {} s", ANSWER.as_secs())),
            };
            attempt += 1;
            if attempt == ASKS {
                return Err(failed);
            }
            tracing::warn!(%origin, attempt, "asking the Hub's catalog again: {failed}");
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
        }
    })
}
