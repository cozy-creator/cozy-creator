use super::auth::Authority;
use crate::machine::receipt::{self, Readiness};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
    service::TowerToHyperService,
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_rustls::{rustls, TlsAcceptor};

/// Who the machine is, from its first moment: what it answers with while it starts (`booting`).
pub(crate) struct Leaf {
    pub authority: Authority,
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: Vec<u8>,
    pub readiness: Arc<Readiness>,
    pub started_at_ms: u64,
}

impl Leaf {
    /// The pod or computer machine: its lifetime files under the machine root, keys from its
    /// grant (a rental's Hub answer replaces them) and this boot's readiness receipt.
    pub(crate) fn machine(
        worker_id: String,
        keys: super::auth::Keys,
        lifetime: crate::machine::identity::Lifetime,
        readiness: Arc<Readiness>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            authority: Authority { worker_id, boot_id: lifetime.boot_id, keys },
            cert_pem: lifetime.cert_pem,
            key_pem: lifetime.key_pem,
            cert_der: lifetime.cert_der,
            readiness,
            started_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(std::io::Error::other)?
                .as_millis() as u64,
        })
    }
}

/// The machine as its API serves it.
pub(crate) struct MachineIdentity {
    pub authority: Authority,
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: Vec<u8>,
    pub readiness: Arc<Readiness>,
    pub started_at_ms: u64,
    /// Its idle ledger.
    pub lifecycle: Arc<crate::machine::lifecycle::Lifecycle>,
    /// The Hubs this machine is registered with: (origin, worker id there).
    pub hubs: Vec<(String, String)>,
    pub updates: Arc<crate::machine::update::Updates>,
    /// The WebRTC listener: `cozy/1` for browsers (ICE-TCP).
    pub webrtc: Option<std::net::TcpListener>,
    /// Where browsers reach it; Status reports it with the port `serve` bound.
    pub player: crate::machine::player::Reach,
    pub(crate) webrtc_port: Option<u16>,
    /// The store's directory; Status reports its filesystem.
    pub store: std::path::PathBuf,
    /// Where the store's logs rotate, and where TensorD's own log rolls.
    pub store_logs: std::path::PathBuf,
    pub logs: std::path::PathBuf,
    /// The GPUs this boot measured, which its readiness receipt names.
    pub gpus: Vec<receipt::Gpu>,
    /// Why a GPU of this computer is not among them.
    pub gpu_problems: Vec<String>,
    /// The most memory each of them offers (`machine.gpu_memory_limit`).
    pub gpu_memory_limit: Option<u64>,
    /// `shared` or `dedicated` (`machine.mode`, else by its lifetime).
    pub mode: &'static str,
    /// Where its PyTorch layer comes from.
    pub torch: crate::published::layer::Torch,
}

pub(crate) async fn serve(
    listener: tokio::net::TcpListener,
    mut identity: MachineIdentity,
    runs: Arc<crate::runs::Runs>,
) -> Result<(), Box<dyn std::error::Error>> {
    let port = listener.local_addr()?.port();
    let readiness = identity.readiness.clone();
    let webrtc = match identity.webrtc.take() {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            Some(tokio::net::TcpListener::from_std(listener)?)
        }
        None => None,
    };
    identity.webrtc_port = match &webrtc {
        Some(listener) => Some(listener.local_addr()?.port()),
        None => None,
    };
    let mut measured = MeasuredIdentity::of(&identity);
    measured.webrtc_port = identity.webrtc_port;
    tokio::spawn(prove_readiness(port, readiness, measured));
    let tls = TlsAcceptor::from(Arc::new(tls(&identity.key_pem, &identity.cert_der)?));
    let identity = Arc::new(identity);
    if let Some(listener) = webrtc {
        let media = super::cozy1::Media::new(identity.clone(), runs.clone())?;
        tokio::spawn(super::cozy1::serve(media, listener));
    }
    let service = TowerToHyperService::new(super::http::router(identity, runs));
    // No HTTP/2 PING judges a client: one this busy machine answers late is not dead. TCP
    // keepalive (`live`) keeps NAT mappings open through a quiet run and ends a connection whose
    // client's host is gone (its runs go on; a client attaches again).
    let http = Arc::new(auto::Builder::new(TokioExecutor::new()));
    let mut failed = 0u64;
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => {
                if failed > 0 {
                    tracing::warn!("the listener accepts connections again, after {failed} failed accepts");
                    failed = 0;
                }
                stream
            }
            // Out of descriptors, say: accept fails again at once, so pause rather than spin. Said
            // once a streak: while it lasts no client or Hub reaches this machine (th-373).
            Err(error) => {
                if failed == 0 {
                    tracing::warn!("the listener cannot accept connections, so nothing reaches this machine: {error}");
                }
                failed += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        live(&stream);
        let (tls, http, service) = (tls.clone(), http.clone(), service.clone());
        tokio::spawn(async move {
            if let Ok(stream) = tls.accept(stream).await {
                let _ = http.serve_connection(TokioIo::new(stream), service).await;
            }
        });
    }
}

/// TCP keepalive: a probe after this long without a packet, then again this often, three times.
const LIVENESS: Duration = Duration::from_secs(20);

/// A starting machine's step, which `booting` answers until the API serves.
pub(crate) struct Boot {
    step: std::sync::Mutex<&'static str>,
    progress: std::sync::atomic::AtomicU64,
}

impl Boot {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { step: std::sync::Mutex::new("starting"), progress: Default::default() })
    }

    /// The machine moved on to `step`: progress, whatever the step.
    pub(crate) fn at(&self, step: &'static str) {
        *self.step.lock().unwrap() = step;
        self.progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn now(&self) -> super::v1::Booting {
        super::v1::Booting {
            step: (*self.step.lock().unwrap()).into(),
            progress: self.progress.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

/// Answers `GET /v1/status` with the machine's identity and boot step (no receipt yet), and
/// everything else `machine_booting`, until the caller aborts it and `serve` takes the listener:
/// the machine answers before it does anything slow, so a watcher hears a slow start, never
/// silence.
pub(crate) fn booting(
    listener: std::net::TcpListener,
    identity: &Leaf,
    boot: Arc<Boot>,
) -> std::io::Result<impl std::future::Future<Output = std::io::Result<()>> + Send + 'static> {
    use axum::{http::StatusCode, routing::get, Json, Router};
    let tls = TlsAcceptor::from(Arc::new(tls(&identity.key_pem, &identity.cert_der)?));
    let frame = super::v1::StatusFrame {
        worker_id: identity.authority.worker_id.clone(),
        boot_id: identity.authority.boot_id.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: api::COMMIT.into(),
        phase: "booting".into(),
        ..Default::default()
    };
    let app = Router::new()
        .route(super::v1::path::STATUS, get(move || {
            let frame = super::v1::StatusFrame { booting: Some(boot.now()), ..frame.clone() };
            async move { Json(frame) }
        }))
        .fallback(|| async {
            let error = api::error::Error::unavailable("machine_booting", "the machine is starting; ask again once it serves");
            (StatusCode::SERVICE_UNAVAILABLE, Json(api::error::Envelope { error }))
        });
    let service = TowerToHyperService::new(app);
    let http = Arc::new(auto::Builder::new(TokioExecutor::new()));
    Ok(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        // Its connections end with it: one a client keeps open (Calcifer's HTTP/2) must not go on
        // answering `machine_booting` after the API serves; closed, the client dials the API.
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            while connections.try_join_next().is_some() {}
            live(&stream);
            let (tls, http, service) = (tls.clone(), http.clone(), service.clone());
            connections.spawn(async move {
                if let Ok(stream) = tls.accept(stream).await {
                    let _ = http.serve_connection(TokioIo::new(stream), service).await;
                }
            });
        }
    })
}

/// The machine's leaf, offering HTTP/2 for control and HTTP/1.1 for bulk lanes.
fn tls(key_pem: &str, cert_der: &[u8]) -> std::io::Result<rustls::ServerConfig> {
    use rustls::pki_types::{pem::PemObject, PrivateKeyDer};
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|e| std::io::Error::other(format!("the machine leaf's private key: {e}")))?;
    let leaf = rustls::pki_types::CertificateDer::from(cert_der.to_vec());
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|config| config.with_no_client_auth().with_single_cert(vec![leaf], key))
        .map_err(std::io::Error::other)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// Nagle off, as Go's net package sets every TCP connection: a response's small trailing frames
/// otherwise wait for the ACK of the frames before them, one round trip each on a far link. TCP
/// keepalive, whose probes the client's kernel answers however busy its process is, ends a
/// connection whose client vanished.
fn live(stream: &tokio::net::TcpStream) {
    let _ = stream.set_nodelay(true);
    #[cfg(unix)]
    keepalive(stream);
    #[cfg(target_os = "linux")]
    little_unsent(stream);
}

/// Bytes count as a rental's use as they're handed to the kernel (`Lifecycle::transferred`), so what it holds unsent is bytes the idle
/// clock has already counted. Left to itself it can hold a lot: session 41816382607's clock stopped 139 s before a 1000 KiB/s reader's last
/// byte. A reader slow enough to take over 15 minutes on what's left would lose its pod with the file's tail unsent. With this, session
/// 41911501719's stopped 14 s before it, on another host. 4 MiB is plenty for a fast link between two wakeups.
#[cfg(target_os = "linux")]
fn little_unsent(stream: &tokio::net::TcpStream) {
    let _ = socket2::SockRef::from(stream).set_tcp_notsent_lowat(4 << 20);
}

#[cfg(unix)]
fn keepalive(stream: &tokio::net::TcpStream) {
    use rustix::net::sockopt::{set_socket_keepalive, set_tcp_keepcnt, set_tcp_keepidle, set_tcp_keepintvl};
    let _ = set_socket_keepalive(stream, true);
    let _ = set_tcp_keepidle(stream, LIVENESS);
    let _ = set_tcp_keepintvl(stream, LIVENESS);
    let _ = set_tcp_keepcnt(stream, 3);
}

struct MeasuredIdentity {
    worker_id: String,
    boot_id: String,
    cert_der: Vec<u8>,
    webrtc_port: Option<u16>,
    gpus: Vec<receipt::Gpu>,
}
impl MeasuredIdentity {
    fn of(identity: &MachineIdentity) -> Self {
        Self {
            worker_id: identity.authority.worker_id.clone(),
            boot_id: identity.authority.boot_id.clone(),
            cert_der: identity.cert_der.clone(),
            webrtc_port: None,
            gpus: identity.gpus.clone(),
        }
    }
}

/// Observes this listener as the Hub requires and seals the result once. A retained envelope
/// of this boot is kept as is; a measurement that contradicts it is reported, never signed.
async fn prove_readiness(port: u16, readiness: Arc<Readiness>, id: MeasuredIdentity) {
    let listener_bound = crate::machine::probe::presents_leaf(port, &id.cert_der).await;
    let foreign_credential_refused =
        crate::machine::probe::refuses_foreign_capability(port, &id.cert_der, &id.worker_id).await;
    let gpus = match readiness.measures_gpus() {
        false => vec![],
        true => id.gpus,
    };
    let result = readiness.seal(
        receipt::Measured {
            boot_id: &id.boot_id,
            worker_port: port,
            cert_der: &id.cert_der,
            gpus,
            listener_bound,
            foreign_credential_refused,
            webrtc_port: id.webrtc_port,
        }
        .payload(),
    );
    match result {
        Ok(true) => tracing::info!("readiness sealed (listener {listener_bound}, foreign capability refused {foreign_credential_refused})"),
        Ok(false) => tracing::info!("this boot's retained readiness still holds"),
        Err(error) => {
            // A service that cannot prove its boot ends, so its parent restarts or rolls it back
            // and whoever launched it hears the reason, instead of waiting on a machine that
            // will never be ready.
            tracing::error!("readiness not proved: {error}");
            std::process::exit(78);
        }
    }
}
