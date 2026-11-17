//! `cozy calcifer`, the role entry. One Calcifer per home: its stable parent (`parent.rs`) holds `calcifer.lock` and the loopback
//! listeners for good, and runs Calcifer as generations, each handed the listeners (cl-1113). A generation opens the store once its
//! parent says to take the home, and answers once the parent has published its `Holder` record, the readiness signal; only then does
//! it take up what an earlier one left (runs, their outputs), while its machines reattach in their own tasks. It serves until
//! `POST /v1/down` or a service stop, or hands the home to the next generation (`POST /v1/restart`). Machines keep running what
//! they accepted either way. Each startup step is logged with its time, so a slow start names itself.
use std::{
    io::ErrorKind,
    net::Ipv4Addr,
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use platform::{
    durable,
    lock::{Held, Kind as LockKind, LockFile},
    paths::Home,
    process,
    service::Services,
};
use ::api::{
    calcifer::REFUSED,
    config::{self, Loaded},
    error::{Envelope, Error, Kind},
};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use crate::{
    Calcifer,
    api::{self, Api},
    generation::{Link, PROTOCOL, ToGeneration, ToParent},
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

/// How a generation ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stopped {
    /// Stopped for good: `POST /v1/down` or a service stop.
    Down,
    /// Handed the home to the next generation (cl-1113).
    HandedOver,
}

/// `cozy calcifer`: the stable parent (`parent.rs`), which runs Calcifer as its generations. Exits 0 unless Calcifer could not start,
/// which `refused.json` then says. Its persistent diagnostics are the closed events in `health.log`.
pub fn main() -> ExitCode {
    // What it inherited never reaches its generations or theirs.
    #[cfg(unix)]
    platform::process::cloexec_from(3);
    std::panic::set_hook(Box::new(|_| crate::health::service_health(crate::health::HealthEvent::Panicked)));
    if let Err(e) = process::set_name("calcifer")
        && !e.is_unsupported()
    {
        warn!("{e}");
    }
    let loaded = match configuration() {
        Ok(loaded) => loaded,
        Err(error) => {
            if let Some(user_home) = std::env::home_dir() {
                refuse(&Home::under(&user_home), error);
            }
            return ExitCode::FAILURE;
        }
    };
    let held = crate::health::install(&loaded.home, loaded.config.calcifer.diagnostics)
        .and_then(|()| working_directory(&loaded))
        .and_then(|()| hold(&loaded));
    match held {
        Ok(Some((held, listeners, address))) => crate::parent::run(&loaded, held, listeners, address),
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            refuse(&loaded.home, error);
            ExitCode::FAILURE
        }
    }
}

/// This home, held for good: its lock, and its loopback listeners. `None`: another Calcifer holds it.
fn hold(loaded: &Loaded) -> Result<Option<(Held, Vec<std::net::TcpListener>, std::net::SocketAddr)>, Error> {
    let (home, port) = (&loaded.home, loaded.config.calcifer.port);
    let lock = LockFile::new(home.calcifer_lock());
    let held = home
        .open()
        .and_then(|()| durable::private_dir(&home.calcifer()))
        .and_then(|()| lock.try_lock(LockKind::Exclusive))
        .map_err(|e| Error::new(Kind::Internal, "calcifer_home", e.to_string()))?;
    let Some(held) = held else {
        info!("another Calcifer holds this home");
        return Ok(None);
    };
    // Only the holder announces: its birth is what `Services::stop` must end.
    Services::new(home, &loaded.user_home)
        .announce("calcifer")
        .map_err(|e| Error::new(Kind::Internal, "calcifer_service", e.to_string()))?;
    let listener = listen(port, PORT).map_err(|e| crate::parent::port_error(port.unwrap_or(PORT), e))?;
    let address = listener.local_addr().map_err(|e| Error::new(Kind::Internal, "calcifer_port", e.to_string()))?;
    // Dev mode: the page's code from this local Vite, served as Calcifer's own (page/development.rs).
    let development_web = loaded.config.calcifer.development_web.filter(|_| loaded.config.dev);
    if development_web.is_some_and(|development| development.port() == address.port()) {
        return Err(Error::new(Kind::Usage, "calcifer_development_web", "Vite and Calcifer must use different ports"));
    }
    let mut listeners = vec![listener];
    // ::1 on the same port too, so no other account can take it and answer `localhost` there (loopback::LOCALHOST).
    if let Ok(listener6) = std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, address.port())) {
        listeners.push(listener6);
    }
    Ok(Some((held, listeners, address)))
}

/// `cozy calcifer generation <what its parent handed it>`: Calcifer itself, as one of its parent's generations (`parent.rs`).
pub fn generation(words: &[std::ffi::OsString]) -> ExitCode {
    let taken = match platform::generation::take(words) {
        Ok(taken) => taken,
        Err(e) => {
            eprintln!("Calcifer runs as a generation of `cozy calcifer` only: {e}");
            return ExitCode::FAILURE;
        }
    };
    // What Calcifer inherited never reaches its worker or helper children.
    #[cfg(unix)]
    platform::process::cloexec_from(3);
    // The controller holds keybags and project data: it writes no core, but stays dumpable (cl-984).
    #[cfg(unix)]
    let _ = platform::private_exec::limit_own_cores();
    // The panic payload and location never enter stderr or the health sink.
    std::panic::set_hook(Box::new(|_| crate::health::service_health(crate::health::HealthEvent::Panicked)));
    // Before the runtime's workers start: Linux names the calling thread.
    if let Err(e) = process::set_name("calcifer")
        && !e.is_unsupported()
    {
        warn!("{e}");
    }
    let served = configuration().and_then(|loaded| {
        loaded
            .config
            .declare_local()
            .and_then(|()| crate::health::install(&loaded.home, loaded.config.calcifer.diagnostics))
            .and_then(|()| working_directory(&loaded))
            .and_then(|()| runtime())
            // Stopped, it exits now: nothing left on the runtime holds the process past its end.
            .and_then(|runtime| {
                let served = runtime.block_on(serve(&loaded, taken));
                runtime.shutdown_background();
                served
            })
    });
    match served {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            error!(code = %e.code, "Calcifer ended: {}", e.message);
            ExitCode::FAILURE
        }
    }
}

/// Release Windows' working-directory lock before any thread starts.
fn working_directory(_loaded: &Loaded) -> Result<(), Error> {
    #[cfg(windows)]
    {
        let directory = _loaded.home.calcifer();
        durable::private_dir(&directory).map_err(|e| Error::new(Kind::Internal, "calcifer_home", e.to_string()))?;
        std::env::set_current_dir(directory).map_err(|e| Error::new(Kind::Internal, "calcifer_working_directory", e.to_string()))?;
    }
    Ok(())
}

/// A worker that parks gives its own freed memory back (cl-870): what it allocated for a project no other thread can.
fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    // Its threads' stacks (workers' and the blocking pool's): a render walk's poll nests large futures, about 9.4 MiB deep in a dev
    // build (scripts/dev-local.sh) and a `/fit` estimate up to 1.8 MiB in a release, measured from frame prologues (cl-1069).
    // Tokio's 2 MiB default crashed dev homes. Pages are committed only as touched.
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(32 << 20)
        .enable_all()
        .on_thread_park(platform::heap::parked)
        .build()
        .map_err(|e| Error::new(Kind::Internal, "calcifer_runtime", e.to_string()))
}

/// Logs why Calcifer cannot start (`calcifer/health.log`) and leaves it in
/// `<home>/calcifer/refused.json` for the CLI: the failed step's code as its cause, never its message.
fn refuse(home: &Home, error: Error) -> Error {
    crate::health::service_health(crate::health::HealthEvent::StartRefused);
    // Its code only (a message may name a project: health.rs, never project detail), and the legacy cut's reason, a fixed sentence
    // that names none (cl-1003).
    let said = match error.code.as_str() {
        "legacy_cut" => format!("Calcifer could not start: {}", error.message),
        _ => "Calcifer could not start".to_owned(),
    };
    error!(target: "cozy.service_health", code = %error.code, "{said}");
    let envelope = serde_json::to_vec(&Envelope {
        error: Error::new(Kind::Unavailable, "calcifer_start_failed", said)
            .cause(error.code.clone())
            .next("cozy doctor"),
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
fn listen(asked: Option<u16>, default: u16) -> std::io::Result<std::net::TcpListener> {
    let mut port = asked.unwrap_or(default);
    if port == 0 {
        return Err(std::io::Error::new(ErrorKind::InvalidInput, "Calcifer's port must be between 1 and 65535"));
    }
    loop {
        match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            // Windows answers a reserved port with permission denied.
            Err(e) if asked.is_none() && port < u16::MAX && matches!(e.kind(), ErrorKind::AddrInUse | ErrorKind::PermissionDenied) => port += 1,
            bound => return bound,
        }
    }
}

#[cfg(test)]
mod port_tests {
    use super::*;

    #[test]
    fn explicit_port_is_exact_and_busy_port_is_refused() {
        let held = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(matches!(listen(Some(port), PORT).unwrap_err().kind(), ErrorKind::AddrInUse | ErrorKind::PermissionDenied));
        drop(held);
        let listener = listen(Some(port), PORT).unwrap();
        assert_eq!(listener.local_addr().unwrap(), (Ipv4Addr::LOCALHOST, port).into());
    }

    #[test]
    fn unconfigured_port_uses_the_next_free_loopback_port() {
        let held = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(port < u16::MAX);
        let listener = listen(None, port).unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        assert!(address.port() > port);
        assert_eq!(PORT, 8818);
        drop(held);
        let at_default = listen(None, port).unwrap();
        assert_eq!(at_default.local_addr().unwrap().port(), port);
    }

    #[test]
    fn zero_never_requests_a_random_port() {
        assert_eq!(listen(Some(0), PORT).unwrap_err().kind(), ErrorKind::InvalidInput);
    }
}

/// A generation's whole course (cl-1113): prepared, it takes the home when its parent says, answers once its record is published,
/// and serves until it stops for good (`down`, a service stop) or hands the home to the next generation.
pub(crate) async fn serve(loaded: &Loaded, taken: platform::generation::Taken) -> Result<Stopped, Error> {
    // What Calcifer makes is its account's alone: a project's database and files, and what its children write (calcifer-access.md).
    #[cfg(unix)]
    let _ = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    let home = &loaded.home;
    crate::project::purge::set_window(loaded.config.calcifer.undo_seconds);
    let parent = |e: std::io::Error| Error::new(Kind::Internal, "calcifer_parent", e.to_string());
    let (link, mut heard) = Link::open(taken.control).map_err(parent)?;
    let Some(ToGeneration::Start { protocol: _, token, address }) = heard.recv().await else {
        return Err(Error::new(Kind::Internal, "calcifer_parent", "the parent said nothing to start with"));
    };
    link.send(&ToParent::Prepared { protocol: PROTOCOL, version: ::api::VERSION.into() }).map_err(parent)?;
    let views = loop {
        match heard.recv().await {
            Some(ToGeneration::TakeOver { views }) => break views,
            Some(ToGeneration::Stop) | None => return Ok(Stopped::Down),
            Some(_) => {}
        }
    };
    let (began, mut steps) = (Instant::now(), Steps(Instant::now()));
    let failed = |error| refuse(home, error);
    crate::project::bind_calcifer(&home.calcifer()).map_err(failed)?;
    for warning in &loaded.warnings {
        warn!("{warning}");
    }
    steps.done("home");
    let calcifer = Calcifer::open(&home.calcifer()).map_err(|e| {
        failed(match e {
            // The legacy cut stopped before it replaced the old store (cl-1003): what to do, and the old store as it was.
            crate::Error::Store(crate::store::StoreError::Cut(why)) => Error::new(Kind::Unavailable, "legacy_cut", why),
            e => Error::new(Kind::Internal, "calcifer_store", e.to_string()),
        })
    })?;
    steps.done("store");
    let mut listeners = taken
        .listeners
        .into_iter()
        .map(|listener| listener.set_nonblocking(true).and_then(|()| TcpListener::from_std(listener)))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| failed(Error::new(Kind::Internal, "calcifer_port", e.to_string())))?;
    if listeners.is_empty() {
        return Err(failed(Error::new(Kind::Internal, "calcifer_port", "the parent handed no listener")));
    }
    let listener = listeners.remove(0);
    let listener6 = listeners.pop();
    crate::loopback::LOCALHOST.store(listener6.is_some(), std::sync::atomic::Ordering::Relaxed);
    crate::loopback::TIMING.store(loaded.config.calcifer.diagnostics, std::sync::atomic::Ordering::Relaxed);
    steps.done("listen");
    crate::checkout::start(&loaded.config);
    let machines = Machines::start(calcifer.clone(), home.clone(), &loaded.user_home, &loaded.config);
    steps.done("machines");
    tokio::spawn(drop_old_measurements(calcifer.clone()));
    let api = Api::new(calcifer.clone(), machines.clone(), &token);
    api.browser_address.set(address).expect("listener address set once");
    if let Some(development) = loaded.config.calcifer.development_web.filter(|_| loaded.config.dev) {
        api.development_web.set(development).expect("development web address set once");
    }
    let _ = api.parent.set(link.clone());
    crate::project::vault::adopt(&api, views);
    // The record before the first answer (cl-885): the parent publishes it, naming this generation, then says `Accept`.
    link.send(&ToParent::Ready { pid: std::process::id(), version: ::api::VERSION.into(), started_at_ms: api.started_ms }).map_err(parent)?;
    loop {
        match heard.recv().await {
            Some(ToGeneration::Accept) => break,
            Some(ToGeneration::Stop) | None => {
                let _ = link.send(&ToParent::Down);
                close(&api, None, None).await;
                return Ok(Stopped::Down);
            }
            Some(_) => {}
        }
    }
    steps.done("record");
    let app = api::router(api.clone()).into_make_service_with_connect_info::<crate::loopback::Peer>();
    // Accepting ends with Calcifer, or before it hands over: a connect then waits in the parent's backlog for the next generation.
    let accepting = tokio::sync::watch::channel(true).0;
    let closing = || {
        let (mut accepting, stopped) = (accepting.subscribe(), api.stopped());
        async move {
            tokio::select! {
                _ = accepting.wait_for(|accepting| !*accepting) => {}
                () = stopped => {}
            }
        }
    };
    let server = tokio::spawn(axum::serve(listener, app.clone()).with_graceful_shutdown(closing()).into_future());
    // `localhost` stops being Calcifer's the moment ::1's server ends.
    let server6 = listener6.map(|listener| {
        let serving = axum::serve(listener, app).with_graceful_shutdown(closing()).into_future();
        tokio::spawn(async move {
            let ended = serving.await;
            crate::loopback::LOCALHOST.store(false, std::sync::atomic::Ordering::Relaxed);
            ended
        })
    });
    crate::health::service_health(crate::health::HealthEvent::Started);
    info!(%address, ms = began.elapsed().as_millis() as u64, "Calcifer serves");
    // What an earlier Calcifer left is taken up beside the answers, never ahead of them: a big store's reads take what they
    // take on a cold, busy disk (cl-1010).
    let resuming = calcifer.clone();
    calcifer.spawn_task(async move {
        let began = Instant::now();
        let pruning = resuming.clone();
        match tokio::task::spawn_blocking(move || resuming.resume()).await {
            Ok(Ok(_)) => info!(target: "cozy.service_health", step = "resume", ms = began.elapsed().as_millis() as u64, "started"),
            Ok(Err(e)) => warn!("resuming what an earlier Calcifer left: {e}"),
            Err(e) => warn!("resuming what an earlier Calcifer left: {e}"),
        }
        let _ = tokio::task::spawn_blocking(move || pruning.prune_ended_machines()).await;
    });
    tokio::spawn(crate::hub::jobs::resume(api.clone()));
    crate::events::tell(api.clone());
    crate::project::previews(api.clone());
    crate::project::resume(&api);
    crate::project::purge::start(api.clone());
    let ended = loop {
        tokio::select! {
            () = api.stopped() => break Stopped::Down,
            message = heard.recv() => match message {
                Some(ToGeneration::HandOver { .. }) => break Stopped::HandedOver,
                // A service stop is `down`, as graceful: the machine runs on.
                Some(ToGeneration::Stop) => {
                    info!("stop: stopping as down does");
                    tokio::spawn(shutdown_after_signal(api.clone()));
                }
                Some(_) => {}
                None => break Stopped::Down,
            },
        }
    };
    match ended {
        Stopped::Down => {
            let _ = link.send(&ToParent::Down);
            close(&api, Some(server), server6).await;
        }
        Stopped::HandedOver => {
            accepting.send_replace(false);
            let views = hand_over(&api, server, server6).await;
            let _ = link.send(&ToParent::Released { views });
            // Until the next one serves: the parent says `Exit`, or ends.
            while let Some(message) = heard.recv().await {
                if matches!(message, ToGeneration::Exit) {
                    break;
                }
            }
        }
    }
    Ok(ended)
}

type Server = tokio::task::JoinHandle<std::io::Result<()>>;

/// Stopping for good: views closed, servers and stores ended.
async fn close(api: &Arc<Api>, server: Option<Server>, server6: Option<Server>) {
    crate::health::service_health(crate::health::HealthEvent::Stopping);
    api.stop();
    if let Err(e) = tokio::task::block_in_place(|| crate::project::vault::close_all(api)) {
        warn!(code = %e.code, "closing encrypted views: {}", e.message);
    }
    ended(server, server6).await;
    // Projects held after their last request close now, their WAL emptied, no request left to hold one again: a stop just after a
    // request leaves no -wal or -shm in a person's folder, nor commits only there.
    tokio::task::block_in_place(|| api.projects.close_lingering());
    api.machines.close();
    api.calcifer.close();
    crate::health::service_health(crate::health::HealthEvent::Stopped);
}

/// Letting go of the home for the next generation (cl-1113): the page's streams are told to come straight back, the servers stop
/// accepting (the parent's listeners stay open, so a connect waits for the next one) and finish what they answer, then the stores
/// close. The open views go to the next generation.
async fn hand_over(api: &Arc<Api>, server: Server, server6: Option<Server>) -> Vec<crate::project::vault::Carried> {
    info!("handing over to the next Calcifer");
    crate::events::restarting();
    // Each stream ends once it has said so; what's left ends with the stop.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    api.stop();
    ended(Some(server), server6).await;
    let views = tokio::task::block_in_place(|| crate::project::vault::carry(api));
    tokio::task::block_in_place(|| api.projects.close_lingering());
    api.machines.close();
    api.calcifer.close();
    views
}

async fn ended(server: Option<Server>, server6: Option<Server>) {
    if let Some(server) = server
        && let Ok(Err(e)) = server.await
    {
        warn!("the API server ended: {e}");
    }
    if let Some(server6) = server6
        && let Ok(Err(e)) = server6.await
    {
        warn!("the API server on ::1 ended: {e}");
    }
}

/// Logs each startup step with the time it took.
struct Steps(Instant);

impl Steps {
    /// A start's step and how long it took, in the closed health log: a slow start names its slow step.
    fn done(&mut self, step: &'static str) {
        info!(target: "cozy.service_health", step, ms = self.0.elapsed().as_millis() as u64, "started");
        self.0 = Instant::now();
    }
}

/// How long a settled run keeps its measurements, the timing `cozy run watch` breaks down. They are
/// most of what a run stores, about 70 MB a day on a busy computer. Nothing else of a run is ever
/// dropped: its row, result, reason, outputs, logs and calls stay for good.
const MEASURED_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// A service stop owns its drain until views close; only transient close refusals retry.
async fn shutdown_after_signal(api: Arc<Api>) {
    let began = Instant::now();
    let mut reported: Option<(String, Instant)> = None;
    loop {
        // Listening before it asks: a view that closes meanwhile isn't missed. The vault clock tries each close again.
        let mut closed = std::pin::pin!(crate::project::vault::CLOSED.notified());
        closed.as_mut().enable();
        let error = match api.shutdown().await {
            Ok(_) => return,
            Err(error) => error,
        };
        if !matches!(error.code.as_str(), "project_busy" | "project_locked") {
            warn!(code = %error.code, "stop: cannot finish closing encrypted views; shutdown remains blocked");
            return;
        }
        if reported.as_ref().is_none_or(|(code, at)| code != &error.code || at.elapsed() >= Duration::from_secs(5)) {
            info!(code = %error.code, elapsed_ms = began.elapsed().as_millis() as u64,
                "stop: waiting for encrypted views to close");
            reported = Some((error.code, Instant::now()));
        }
        tokio::select! {
            () = api.stopped() => return,
            () = closed => {},
        }
    }
}

/// Drops measurements past [`MEASURED_FOR`], now and daily. Each write is followed by a pause as long
/// as it took, so the store's one writer is never held more than half the time.
async fn drop_old_measurements(calcifer: Arc<Calcifer>) {
    loop {
        let began = Instant::now();
        let before_ms = crate::store::now_ms() - MEASURED_FOR.as_millis() as i64;
        let dropped = {
            let _took = crate::took::Took::new("drop_measurements");
            calcifer.store().drop_measurements(before_ms)
        };
        match dropped {
            Ok(true) => tokio::time::sleep(began.elapsed()).await,
            Ok(false) => tokio::time::sleep(Duration::from_secs(24 * 3600)).await,
            Err(e) => {
                warn!("dropping old measurements: {e}");
                tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
            }
        }
    }
}
