//! `cozy calcifer`, the role entry. One Calcifer per home, held by `calcifer.lock`. It serves the
//! loopback API as soon as its store is open and publishes its `Holder` record, the readiness
//! signal; only then does it take up what an earlier Calcifer left (runs, their outputs), while its
//! machines reattach in their own tasks. It runs until `POST /v1/down`, an update or a crash: the
//! login service keeps it up. Machines keep running what they accepted either way. Each startup
//! step is logged with its time, so a slow start names itself.
use std::{
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddr},
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use platform::{
    durable,
    lock::{Kind as LockKind, LockFile},
    paths::Home,
    process,
    service::Services,
};
use ::api::{
    calcifer::{Holder, REFUSED},
    config::{self, Loaded},
    error::{Envelope, Error, Kind},
};
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::{
    Calcifer,
    api::{self, Api},
    machines::Machines,
};

#[cfg(not(windows))]
use ::api::config::load as configuration;

#[cfg(windows)]
fn configuration() -> Result<Loaded, Error> {
    let failed = |e: std::io::Error| Error::new(Kind::Internal, "calcifer_path", e.to_string());
    // Capture relative path settings before the configuration's environment snapshot
    // and before leaving the launching directory. Children inherit these paths too.
    for name in [
        "COZY_MACHINE_ROOT", "COZY_TENSORFS_ROOT", "COZY_TENSORFS_CACHE", "TENSORFS_HOME",
        "COZY_SECUREFS_PROVIDER", "TMPDIR", "TEMP", "TMP", "XDG_CACHE_HOME",
        "UV_CACHE_DIR", "UV_TOOL_DIR", "UV_PYTHON_INSTALL_DIR", "SSL_CERT_FILE",
    ] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            let path = std::path::absolute(value).map_err(failed)?;
            process::set_environment(std::ffi::OsStr::new(name), path.as_os_str());
        }
    }
    for (name, empty_is_current_directory) in [("PATH", true), ("SSL_CERT_DIR", false)] {
        if let Some(value) = std::env::var_os(name) {
            let paths = std::env::split_paths(&value)
                .map(|path| {
                    if path.as_os_str().is_empty() {
                        if !empty_is_current_directory { return Ok(path); }
                        return std::path::absolute(".");
                    }
                    std::path::absolute(path)
                })
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(failed)?;
            let paths = std::env::join_paths(paths)
                .map_err(|e| Error::new(Kind::Internal, "calcifer_path", e.to_string()))?;
            process::set_environment(std::ffi::OsStr::new(name), &paths);
        }
    }
    let mut loaded = config::load()?;
    for (name, path) in [
        ("COZY_MACHINE_ROOT", &mut loaded.config.machine.root),
        ("COZY_TENSORFS_ROOT", &mut loaded.config.machine.store),
        ("COZY_TENSORFS_CACHE", &mut loaded.config.machine.cache),
    ] {
        if let Some(path) = path {
            let directory = if path.as_os_str().is_empty() { std::path::Path::new(".") } else { path.as_path() };
            *path = std::path::absolute(directory).map_err(failed)?;
            process::set_environment(std::ffi::OsStr::new(name), path.as_os_str());
        }
    }
    Ok(loaded)
}

/// Why `serve` returned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stopped {
    /// Another Calcifer holds this home.
    AnotherHolds,
    /// `POST /v1/down`.
    Down,
}

/// `cozy calcifer`: exits 0 unless Calcifer could not start, which `refused.json` then says.
/// Its persistent diagnostics are the closed events in `health.log`.
pub fn main() -> ExitCode {
    // Inherited native proof IPC stays available here, never in browser/worker/helper children.
    #[cfg(unix)]
    platform::process::cloexec_from(3);
    // The trusted controller never executes project code in-process. Stop OS-managed dump
    // capture before configuration, keybags, state recovery or native proof payloads are read.
    if platform::private_exec::disable_project_parent_dumps().is_err() {
        return ExitCode::FAILURE;
    }
    // The panic payload and location never enter stderr or the health sink.
    std::panic::set_hook(Box::new(|_| crate::health::service_health(crate::health::HealthEvent::Panicked)));
    // Before the runtime's workers start: Linux names the calling thread.
    if let Err(e) = process::set_name("calcifer")
        && !e.is_unsupported()
    {
        warn!("{e}");
    }
    let served = match configuration() {
        Ok(loaded) => match crate::health::install(&loaded.home).and_then(|()| runtime(&loaded)) {
            // Stopped, Calcifer exits now: nothing left on the runtime holds the process (or its
            // service) past `down`.
            Ok(runtime) => {
                let served = runtime.block_on(serve(&loaded));
                runtime.shutdown_background();
                served
            }
            Err(error) => Err(refuse(&loaded.home, error)),
        },
        Err(error) => match std::env::home_dir() {
            Some(user_home) => Err(refuse(&Home::under(&user_home), error)),
            None => Err(error),
        },
    };
    match served {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Release Windows' working-directory lock before any runtime worker starts. A worker that parks gives its own freed memory
/// back (cl-870): what it allocated for a project no other thread can.
fn runtime(_loaded: &Loaded) -> Result<tokio::runtime::Runtime, Error> {
    #[cfg(windows)]
    {
        let directory = _loaded.home.calcifer();
        durable::private_dir(&directory)
            .map_err(|e| Error::new(Kind::Internal, "calcifer_home", e.to_string()))?;
        std::env::set_current_dir(directory)
            .map_err(|e| Error::new(Kind::Internal, "calcifer_working_directory", e.to_string()))?;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_park(platform::heap::parked)
        .build()
        .map_err(|e| Error::new(Kind::Internal, "calcifer_runtime", e.to_string()))
}

/// Logs why Calcifer cannot start and leaves it in `<home>/calcifer/refused.json` for the CLI.
fn refuse(home: &Home, error: Error) -> Error {
    crate::health::service_health(crate::health::HealthEvent::StartRefused);
    let envelope = serde_json::to_vec(&Envelope {
        error: Error::new(Kind::Unavailable, "calcifer_start_failed", "Calcifer could not start").next("cozy doctor"),
    })
    .expect("serializes");
    let written = home
        .open()
        .and_then(|()| durable::private_dir(&home.calcifer()))
        .and_then(|()| durable::replace(&home.calcifer().join(REFUSED), &envelope, 0o600));
    let _ = written;
    error
}

/// Where Calcifer listens unless `calcifer.port` says: the same port each start, so a page's origin,
/// and what it keeps there, outlive a restart.
const PORT: u16 = config::DEFAULT_CALCIFER_PORT;

/// The port asked for and no other; none asked for, the default or the next free one.
async fn listen(asked: Option<u16>, default: u16) -> std::io::Result<TcpListener> {
    let mut port = asked.unwrap_or(default);
    if port == 0 {
        return Err(std::io::Error::new(ErrorKind::InvalidInput, "Calcifer's port must be between 1 and 65535"));
    }
    loop {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            // Windows answers a reserved port with permission denied.
            Err(e) if asked.is_none() && port < u16::MAX && matches!(e.kind(), ErrorKind::AddrInUse | ErrorKind::PermissionDenied) => port += 1,
            bound => return bound,
        }
    }
}

#[cfg(test)]
mod port_tests {
    use super::*;

    #[tokio::test]
    async fn explicit_port_is_exact_and_busy_port_is_refused() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(matches!(listen(Some(port), PORT).await.unwrap_err().kind(), ErrorKind::AddrInUse | ErrorKind::PermissionDenied));
        drop(held);
        let listener = listen(Some(port), PORT).await.unwrap();
        assert_eq!(listener.local_addr().unwrap(), (Ipv4Addr::LOCALHOST, port).into());
    }

    #[tokio::test]
    async fn unconfigured_port_uses_the_next_free_loopback_port() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(port < u16::MAX);
        let listener = listen(None, port).await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        assert!(address.port() > port);
        assert_eq!(PORT, 8818);
        drop(held);
        let at_default = listen(None, port).await.unwrap();
        assert_eq!(at_default.local_addr().unwrap().port(), port);
    }

    #[tokio::test]
    async fn zero_never_requests_a_random_port() {
        assert_eq!(listen(Some(0), PORT).await.unwrap_err().kind(), ErrorKind::InvalidInput);
    }
}

/// Serves `loaded.home` on `127.0.0.1`, at [`listen`]'s port, until `POST /v1/down`.
pub(crate) async fn serve(loaded: &Loaded) -> Result<Stopped, Error> {
    let (home, port) = (&loaded.home, loaded.config.calcifer.port);
    let (began, mut steps) = (Instant::now(), Steps(Instant::now()));
    let failed = |error| refuse(home, error);
    let lock = LockFile::new(home.calcifer_lock());
    let held = home
        .open()
        .and_then(|()| durable::private_dir(&home.calcifer()))
        .and_then(|()| lock.try_lock(LockKind::Exclusive))
        .map_err(|e| failed(Error::new(Kind::Internal, "calcifer_home", e.to_string())))?;
    let Some(held) = held else {
        info!("another Calcifer holds this home");
        return Ok(Stopped::AnotherHolds);
    };
    crate::browser::recover_profiles(&home.calcifer()).map_err(|error| failed(error.0))?;
    // Old raw captures are disposable, unlike the authoritative run journal. A
    // retained/unknown artifact remains visible through migration_required.
    let _ = crate::legacy_cleanup::retire(&home.calcifer());
    crate::project::bind_calcifer(&home.calcifer()).map_err(failed)?;
    // Only the holder announces: its birth is what `Services::stop` must end.
    Services::new(home, &loaded.user_home)
        .announce("calcifer")
        .map_err(|e| {
            failed(Error::new(
                Kind::Internal,
                "calcifer_service",
                e.to_string(),
            ))
        })?;
    for warning in &loaded.warnings {
        warn!("{warning}");
    }
    steps.done("home");

    let calcifer = Calcifer::open(&home.calcifer(), &home.outputs())
        .map_err(|e| failed(Error::new(Kind::Internal, "calcifer_store", e.to_string())))?;
    steps.done("store");
    let listener = match listen(port, PORT).await {
        Ok(listener) => listener,
        Err(e) => {
            calcifer.close();
            return Err(failed(
                Error::new(
                    Kind::Unavailable,
                    "calcifer_port",
                    format!("Calcifer cannot listen on 127.0.0.1:{}: {e}", port.unwrap_or(PORT)),
                )
                .remedy("free the port, or take calcifer.port out of config.toml to use the next free one"),
            ));
        }
    };
    let address = listener
        .local_addr()
        .map_err(|e| failed(Error::new(Kind::Internal, "calcifer_port", e.to_string())))?;
    if loaded.config.calcifer.development_web.is_some_and(|development| development.port() == address.port()) {
        calcifer.close();
        return Err(failed(Error::new(Kind::Usage, "calcifer_development_web", "Vite and Calcifer must use different ports")));
    }
    steps.done("listen");
    let mut token = [0; 32];
    getrandom::fill(&mut token).expect("the OS provides randomness");
    let token = hex::encode(token);
    let machines = Machines::start(calcifer.clone(), home.clone(), &loaded.user_home, &loaded.config);
    steps.done("machines");
    tokio::spawn(drop_old_measurements(calcifer.clone()));
    let api = Api::new(calcifer.clone(), machines.clone(), &token);
    api.browser_address.set(address).expect("listener address set once");
    if let Some(development) = loaded.config.calcifer.development_web {
        api.development_web.set(development).expect("development web address set once");
    }
    // A service manager's stop (SIGTERM) is `down`, as graceful: the machine runs on.
    #[cfg(unix)]
    if let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        let stopping = api.clone();
        tokio::spawn(async move {
            if term.recv().await.is_none() { return; }
            info!("SIGTERM: stopping as down does");
            shutdown_after_signal(stopping).await;
        });
    }
    let app = api::router(api.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let server = tokio::spawn(
        axum::serve(listener, app)
            .with_graceful_shutdown(api.stopped())
            .into_future(),
    );

    let holder = Holder {
        address,
        token,
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
        started_at_ms: crate::store::now_ms(),
    };
    let mut record = serde_json::to_vec(&holder).expect("serializes");
    record.push(b'\n');
    if let Err(e) = held.publish(&record) {
        api.stop();
        let _ = server.await;
        machines.close();
        calcifer.close();
        return Err(failed(Error::new(
            Kind::Internal,
            "calcifer_home",
            e.to_string(),
        )));
    }
    let _ = std::fs::remove_file(home.calcifer().join(REFUSED));
    steps.done("record");
    crate::browser::hosted_proof(api.clone());
    crate::health::service_health(crate::health::HealthEvent::Started);
    info!(%address, ms = began.elapsed().as_millis() as u64, "Calcifer serves");
    match calcifer.resume() {
        Ok(runs) => steps.done(&format!("resume ({runs} runs)")),
        Err(e) => warn!("resuming what an earlier Calcifer left: {e}"),
    }
    tokio::spawn(crate::hub::jobs::resume(api.clone()));
    crate::events::tell(api.clone());
    crate::project::previews(api.clone());
    crate::project::resume(&api);

    api.stopped().await;
    crate::health::service_health(crate::health::HealthEvent::Stopping);
    tokio::task::block_in_place(|| crate::project::vault::close_all(&api)).map_err(failed)?;
    if let Ok(Err(e)) = server.await {
        warn!("the API server ended: {e}");
    }
    machines.close();
    calcifer.close();
    drop(held);
    crate::health::service_health(crate::health::HealthEvent::Stopped);
    Ok(Stopped::Down)
}

/// Logs each startup step with the time it took.
struct Steps(Instant);

impl Steps {
    fn done(&mut self, step: &str) {
        info!(step, ms = self.0.elapsed().as_millis() as u64, "started");
        self.0 = Instant::now();
    }
}

/// How long a settled run keeps its measurements, the timing `cozy run watch` breaks down. They are
/// most of what a run stores, about 70 MB a day on a busy computer. Nothing else of a run is ever
/// dropped: its row, result, reason, outputs, logs and calls stay for good.
const MEASURED_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// A service stop owns its drain until views close; only transient close refusals retry.
#[cfg(unix)]
async fn shutdown_after_signal(api: Arc<Api>) {
    let began = Instant::now();
    let mut reported: Option<(String, Instant)> = None;
    loop {
        let error = match api.shutdown().await {
            Ok(_) => return,
            Err(error) => error,
        };
        if !matches!(error.code.as_str(), "project_busy" | "project_locked") {
            warn!(code = %error.code, "SIGTERM: cannot finish closing encrypted views; shutdown remains blocked");
            return;
        }
        if reported.as_ref().is_none_or(|(code, at)| code != &error.code || at.elapsed() >= Duration::from_secs(5)) {
            info!(code = %error.code, elapsed_ms = began.elapsed().as_millis() as u64,
                "SIGTERM: waiting for encrypted views to close");
            reported = Some((error.code, Instant::now()));
        }
        tokio::select! {
            () = api.stopped() => return,
            () = tokio::time::sleep(Duration::from_millis(250)) => {},
        }
    }
}

/// Drops measurements past [`MEASURED_FOR`], now and daily. Each write is followed by a pause as long
/// as it took, so the store's one writer is never held more than half the time.
async fn drop_old_measurements(calcifer: Arc<Calcifer>) {
    let mut after = 0;
    loop {
        let began = Instant::now();
        let before_ms = crate::store::now_ms() - MEASURED_FOR.as_millis() as i64;
        match calcifer.store().drop_measurements(after, before_ms) {
            Ok(Some(next)) => {
                after = next;
                tokio::time::sleep(began.elapsed()).await;
            }
            Ok(None) => tokio::time::sleep(Duration::from_secs(24 * 3600)).await,
            Err(e) => {
                warn!("dropping old measurements: {e}");
                tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
            }
        }
    }
}
