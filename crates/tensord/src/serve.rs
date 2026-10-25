//! The machine role: `cozy machine serve` on this computer (Calcifer starts it when something
//! uses it), `--rental` on a pod; and the `tensord` binary's modes, which TensorD's and
//! Calcifer's tests run.
use crate::{
    config::{self, Flags},
    machine::grant::Lifetime,
};
use std::{io, path::PathBuf, process::ExitCode, sync::Arc};
use crate::{origin::Machine, storage::Store};

/// `cozy machine serve [--rental]`, from the arguments after `machine`: this computer's or a
/// pod's machine, until it stops.
pub fn main(args: impl IntoIterator<Item = std::ffi::OsString>) -> ExitCode {
    #[derive(clap::Parser)]
    #[command(name = "cozy machine serve", about = "Run this computer's machine (TensorD)")]
    struct Serve {
        #[command(flatten)]
        flags: Flags,
        /// Windows: the readiness pipe a stable parent handed the service it started.
        #[arg(long, hide = true)]
        service_ready: Option<u64>,
    }
    let serve = <Serve as clap::Parser>::parse_from(args);
    machine(serve.flags.lifetime(), serve.service_ready)
}

/// The machine: its stable parent, then the service it starts (`service_ready`: this is that
/// service, on Windows). Exits the process on failure.
pub fn machine(lifetime: Lifetime, service_ready: Option<u64>) -> ExitCode {
    if let Err(error) = run(lifetime, service_ready) {
        config::init_logs(None);
        tracing::error!("{error}");
        std::process::exit(1);
    }
    ExitCode::SUCCESS
}

fn run(lifetime: Lifetime, service_ready: Option<u64>) -> io::Result<()> {
    use crate::machine::{binary, identity, provider::ProviderSelf, ssh, supervise, update};
    // Where this binary was started from, before anything can rename another over it.
    if let Err(error) = binary::record() {
        eprintln!("cannot record this binary's launch path ({error}): it never activates another");
    }
    // Linux names the calling thread: this one, before any other starts.
    if let Err(error) = platform::process::set_name("tensord") {
        if !error.is_unsupported() {
            return Err(io::Error::other(format!("cannot set TensorD process name: {error}")));
        }
    }
    // Whatever this process inherited (its own re-execution's credential pipe) reaches nothing
    // it starts; descriptors it hands on are handed explicitly. 3 to 5 stay reserved for them: a
    // log file at 5 would be read as the credential pipe. Windows hands on none.
    #[cfg(unix)]
    {
        platform::process::cloexec_from(3);
        platform::process::reserve_handed()?;
    }
    let mut config = config::load(lifetime)?;
    config::init_logs(Some(&config.logs));
    for warning in &config.warnings {
        tracing::warn!("{warning}");
    }
    // Windows forks nothing: this process is the stable parent until the service it starts as
    // this executable again returns here.
    #[cfg(windows)]
    let ready = supervise::supervise(
        &update::Paths::new(&config.grant.layout.engine(), &config.grant.layout.root),
        None,
        service_ready,
    )?;
    #[cfg(unix)]
    let _ = service_ready;
    let grant = &mut config.grant;
    // A pod's provider credential, kept from everything this machine starts: on its first start
    // this re-executes the machine without the key in its environment, before any thread.
    if lifetime == Lifetime::Rental {
        grant.provider = ProviderSelf::take(&grant.layout.root, &config.provider);
    }
    config::forget_receipt_key();
    identity::hold(&grant.layout)?;
    if let Some(key) = grant.developer_key.as_deref() {
        if let Err(error) = ssh::start(key) {
            tracing::warn!("{error}");
        }
    }
    let paths = update::Paths::new(&grant.layout.engine(), &grant.layout.root);
    #[cfg(unix)]
    let ready = supervise::supervise(&paths, grant.provider.clone())?;
    // The service: why it fails reaches its parent's rollback reason.
    config::gpu_memory_limit()
        .and_then(|limit| {
            config.gpu_memory_limit = limit;
            run_machine(config, ready)
        })
        .inspect_err(|error| update::service_failed(&paths, error))
}

/// The machine's async runtime: its API and its lifecycle, where every request in flight is a
/// task, never a thread. Made in the service, after the stable parent's fork. TensorFS fetches
/// on its own runtime (`fetch::runtime`).
fn machine_runtime() -> io::Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("tensord").enable_all().build()?;
    Ok(RUNTIME.get_or_init(|| runtime))
}

/// The state root's one owner: its lock held while the process lives, and the TensorFS store it
/// serves (which may live outside `root`), claimed for this machine alone.
fn own(root: &std::path::Path, store: &std::path::Path) -> io::Result<((std::fs::File, std::fs::File), Arc<Store>)> {
    std::fs::create_dir_all(root)?;
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(root.join("owner.lock"))?;
    lock.try_lock()
        .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "state already has a machine owner"))?;
    platform::durable::set_mode(root, 0o700)?;
    let store = crate::storage::open(store)?;
    // The store is this machine's alone, whatever path another machine names it by.
    let claim = crate::storage::claim(store.root())?;
    Ok(((lock, claim), store))
}

/// `tensord version --json`: what a candidate binary declares to the machine's update.
pub fn version() -> io::Result<()> {
    #[derive(serde::Serialize)]
    struct Version {
        name: &'static str,
        version: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<&'static str>,
        api: [&'static str; 1],
        capabilities: Vec<&'static str>,
    }
    // Machine contracts: clients choose by capability.
    let capabilities = crate::api::CAPABILITIES.to_vec();
    let record = Version {
        name: "tensord",
        version: env!("CARGO_PKG_VERSION"),
        commit: Some(protocol::COMMIT),
        api: ["cozy.machine.v1"],
        capabilities,
    };
    println!("{}", serde_json::to_string(&record)?);
    Ok(())
}

/// Runs the machine from its grant: identity and readiness under the machine root, the engine
/// under `var/lib/cozy/tensord`, the API on the granted port.
fn run_machine(config: config::Config, mut ready: crate::machine::supervise::Ready) -> io::Result<()> {
    let config::Config { mut grant, visible_devices, gpu_memory_limit, endpoint, logs, .. } = config;
    let runtime = machine_runtime()?;
    use crate::machine::{identity, receipt};
    let layout = grant.layout.clone();
    let lifetime = identity::open(&layout)?;
    let rental = grant.lifetime == Lifetime::Rental;
    let readiness = receipt::Readiness::open(
        Some(layout.bootstrap.join("readiness-envelope.json")),
        grant.receipt_key.take(),
        rental,
    )?;
    if let Some(attested) = readiness.attested() {
        if receipt::boot_id(&attested).as_deref() != Some(lifetime.boot_id.as_str()) {
            return Err(io::Error::other(
                "the retained readiness envelope names another boot than this machine root",
            ));
        }
    }
    let fresh = readiness.attested().is_none();
    let keys = grant.keys()?;
    let mut identity = crate::api::MachineIdentity::machine(
        grant.worker_id.clone(),
        keys.clone(),
        lifetime,
        readiness,
    )?;
    // The machine answers before it does anything slow: its listener first, then its step as it
    // starts, so whoever watches it (a rental's Hub, Calcifer) hears a slow start, never silence.
    let api = crate::machine::net::kept_listener(&layout.state.join("api-port"), grant.listen_host, grant.worker_port)?;
    let boot = crate::api::Boot::new();
    api.set_nonblocking(true)?;
    let booting = runtime.spawn(crate::api::booting(api.try_clone()?, &identity, boot.clone())?);
    boot.at("store");
    let lifecycle = crate::machine::lifecycle::Lifecycle::open(
        layout.state.join("idle.json"),
        rental,
        fresh,
        grant.idle_ms,
    )?;
    identity.lifecycle = Some(lifecycle.clone());
    let engine = layout.engine();
    identity.store = Some(layout.store.clone());
    let generations = engine.join("generations");
    let paths = crate::machine::update::Paths::new(&engine, &layout.root);
    let (_owned, store) = own(&engine, &layout.store)?;
    // The machine's one downloader, held for its life: it reads the datacenter volume before the
    // Hub and fills it. A variable naming no mount point (the volume was not delivered) is none.
    boot.at("cache");
    let machine = Machine::new(store.clone(), layout.cache.as_deref())?;
    boot.at("journal");
    let service = crate::service::Service::open(&engine, &generations, 1)?;
    // The jobs this root held (a preparation the restart ended counts as ended now): Status
    // reports the true deadline from the first frame.
    let (working, ended_ms) = service.engine.jobs(0)?;
    lifecycle.observe(crate::machine::lifecycle::Jobs { working, ended_ms }, lifecycle.epoch());
    // An update's activation holds queued runs; one that ends without its restart lets them start.
    // An admission or a follower letting go may start the idle clock.
    let (holding, executions) = (lifecycle.clone(), service.engine.clone());
    service.hold_while(move || holding.holding());
    lifecycle.on_change(move || executions.notify_activity());
    let sdk = crate::published::PackageSdk::selected(&paths.sdk(), crate::machine::uv::Uv::of(&layout.root, &engine, rental));
    boot.at("updates");
    let running = {
        let engine = service.engine.clone();
        Box::new(move || engine.with_journal(|journal| journal.started()).unwrap_or_default())
    };
    let updates = crate::machine::update::Updates::open(
        paths.clone(),
        sdk.uv.clone(),
        sdk.python.clone(),
        running,
        Some(lifecycle.clone()),
        Some(machine.clone()),
    )?;
    identity.updates = Some(updates.clone());
    let readiness = identity.readiness.clone();
    std::thread::Builder::new()
        .name("readiness-report".into())
        .spawn(move || {
            readiness.wait_proved();
            // A Runtime update this process started on is now committed.
            if let Err(error) = updates.commit() {
                tracing::warn!("Runtime update commit: {error}");
            }
            // An update queued behind the one this restart ended starts now.
            updates.next(|code| std::process::exit(code));
            ready.report();
        })?;
    // A machine serves every package on its pool: its GPUs (the envelope, sorted by index; a
    // degree-K plan uses the first K) and deviceless executors, with the executor environment
    // the pod proofs used.
    boot.at("gpus");
    let mut gpus = receipt::gpus(visible_devices.as_deref())?;
    // A driver below CUDA 13.0's floor serves no GPU; the machine stays for CPU work.
    if let Some(why) = gpus.first().and_then(|gpu| platform::gpu::below_floor(&gpu.driver_version)) {
        tracing::error!("{why}; this machine offers no GPU");
        gpus.clear();
    }
    identity.gpus = gpus.clone();
    identity.gpu_memory_limit = gpu_memory_limit;
    boot.at("pool");
    {
        let devices: Vec<String> = gpus.iter().map(|g| g.device_index.to_string()).collect();
        let home = engine.join("executor-home");
        std::fs::create_dir_all(&home)?;
        let config = serde_json::json!({
            "devices": devices.join(","),
            "inventory": gpus,
            "authorized_device_limit_bytes": null,
            "gpu_memory_limit": gpu_memory_limit,
            "environment": {"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": home, "COZY_HOME": home},
            // A rented pod is the machine's alone (its grant says so); anywhere else it is a guest.
            "host": {"mode": if rental { "dedicated" } else { "shared" }},
            "image_kernels": (!gpus.is_empty()).then(|| image_kernels(&layout.root, &paths.sdk())).flatten(),
            "toolchain": crate::machine::toolchain::dir(&engine),
        });
        let config = serde_json::from_value::<crate::gpu_service::GpuConfig>(config)
            .map_err(io::Error::other)?
            .checked()?;
        let store = store.clone();
        service.configure_pool(crate::gpu_service::Pool::new(
            &engine.join("gpu"),
            config,
            store,
        )?)?;
    }
    if !rental {
        let (lifecycle, engine, home) = (lifecycle.clone(), service.engine.clone(), grant.home.clone());
        let (released, stopped) = (service.clone(), service.clone());
        std::thread::Builder::new().name("local-lifecycle".into()).spawn(move || {
            crate::machine::lifecycle::exit_when_idle(
                lifecycle,
                engine,
                move || released.release_idle(),
                move || stopped.stop(),
                // Kept up for its authorized controllers, unless `cozy update` replaced its binary:
                // it exits idle and Calcifer starts the new one.
                move || {
                    let swapped = crate::machine::binary::launch().is_some_and(|launch| launch.swapped());
                    crate::machine::grant::authorized(&home) && !swapped
                },
            )
        })?;
    }
    if let Some(hub) = grant.hub.clone().filter(|_| rental) {
        identity.hubs = vec![(hub.origin.clone(), hub.worker_id.clone())];
        let hub = Arc::new(crate::machine::hub::Hub::new(hub)?);
        let service = service.clone();
        let provider = grant.provider.clone();
        std::thread::Builder::new()
            .name("rental-lifecycle".into())
            .spawn(move || {
                runtime.block_on(async {
                    tokio::spawn(crate::machine::lifecycle::keep_authority(
                        hub.clone(),
                        keys,
                    ));
                    let jobs = move |since| {
                        let (working, ended_ms) = service.engine.jobs(since)?;
                        Ok(crate::machine::lifecycle::Jobs { working, ended_ms })
                    };
                    crate::machine::lifecycle::release_when_idle(lifecycle, hub, provider, jobs).await;
                });
                // The Hub accepted the release, or the rental ended itself: this process ends.
                std::process::exit(0);
            })?;
    }
    boot.at("api");
    identity.webrtc = crate::machine::player::listen(&layout.state, grant.webrtc_port, !rental)?;
    identity.player = grant.webrtc_reach.clone();
    // The boot answer ends before the API takes its listener: one server answers at a time.
    booting.abort();
    let _ = runtime.block_on(booting);
    start_api(
        &engine,
        &generations,
        identity,
        api,
        &store,
        &machine,
        &service,
        sdk,
        // A run naming no Hub reads this rental's own Hub's public content.
        grant.hub.clone().filter(|_| rental),
        Some(logs),
    )?;
    // The Hub probes the endpoint the provider put in this pod's environment at once,
    // instead of waiting for the provider to report it.
    if let (Some(hub), Some(endpoint), true) = (grant.hub.clone(), endpoint, rental) {
        std::thread::Builder::new()
            .name("endpoint-report".into())
            .spawn(move || report_endpoint(hub, endpoint))?;
    }
    drop(service);
    // The machine runs on its own threads until it is stopped.
    loop {
        std::thread::park();
    }
}

/// The image's Runtime interpreter and the executors' Runtime wheel, when both are here: the
/// machine compiles its kernels with them as it starts.
fn image_kernels(root: &std::path::Path, sdk: &std::path::Path) -> Option<serde_json::Value> {
    let python = root.join("opt/cozy/python/bin/python");
    let wheel = std::fs::read_dir(sdk).ok()?.flatten().map(|entry| entry.path()).find(|path| {
        path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with("cozy_runtime-") && name.ends_with(".whl")
        })
    })?;
    python.is_file().then(|| serde_json::json!({"python": python, "runtime_wheel": wheel}))
}


#[allow(clippy::too_many_arguments)]
fn start_api(
    root: &std::path::Path,
    generations: &std::path::Path,
    identity: crate::api::MachineIdentity,
    listener: std::net::TcpListener,
    store: &Arc<Store>,
    machine: &Arc<Machine>,
    service: &Arc<crate::service::Service>,
    sdk: crate::published::PackageSdk,
    own_hub: Option<crate::machine::grant::HubGrant>,
    logs: Option<PathBuf>,
) -> io::Result<()> {
    use crate::{api, machine_api::NativeBackend};
    let store = store.clone();
    let mut backend = NativeBackend::new(service.clone(), identity.authority.clone(), store.clone());
    backend.logs = logs;
    let publisher = crate::published::Publisher::new(sdk, machine.clone(), identity.updates.clone());
    service.configure_publisher(publisher.clone());
    backend.publisher = Some(publisher.clone());
    let objects = Arc::new(crate::objects::Objects::new(&root.join("writes"), service.engine.clone())?);
    service.engine.hold_memos(&objects);
    backend.runs = Some(Arc::new(crate::runs::Runs {
        service: service.clone(),
        objects: objects.clone(),
        local: Arc::new(crate::local_source::LocalSources::new(objects, publisher.clone(), generations)),
        publisher,
        own_hub: own_hub
            .map(|hub| crate::hub::Source::new(&hub.origin, hub.ca_der, None))
            .transpose()
            .map_err(|e| io::Error::other(e.0))?,
        contexts: Default::default(),
    }));
    crate::calls::Calls::configure(service, machine.clone(), backend.runs.as_ref())?;
    if let Some(runs) = &backend.runs {
        runs.restore_warm_sets()?;
        crate::api::machine_runs::resume(runs)?;
    }
    listener.set_nonblocking(true)?;
    #[derive(serde::Serialize)]
    struct Ready<'a> {
        address: String,
        worker_id: &'a str,
        boot_id: &'a str,
        cert_pem: &'a str,
    }
    let ready = serde_json::to_vec(&Ready {
        address: listener.local_addr()?.to_string(),
        worker_id: &identity.authority.worker_id,
        boot_id: &identity.authority.boot_id,
        cert_pem: &identity.cert_pem,
    })?;
    crate::machine::identity::write_atomic(&root.join("api-ready.json"), &ready, 0o600)?;
    let runtime = machine_runtime()?;
    std::thread::Builder::new()
        .name("machine-api".into())
        .spawn(move || {
            let result = runtime.block_on(async {
                api::serve(
                    tokio::net::TcpListener::from_std(listener)?,
                    identity,
                    Arc::new(backend),
                )
                .await
            });
            if let Err(error) = result {
                tracing::error!("machine API stopped: {error}");
            }
        })?;
    Ok(())
}
/// Tells the Hub this machine's endpoint, trying again while the Hub cannot be reached. The
/// report only saves the provider's lag: after a few misses the provider's view serves.
fn report_endpoint(grant: crate::machine::grant::HubGrant, endpoint: crate::machine::endpoint::Endpoint) {
    use crate::machine::hub::{Hub, Refusal};
    let runtime = match machine_runtime() {
        Ok(runtime) => runtime,
        Err(error) => return tracing::warn!("endpoint report: {error}"),
    };
    let hub = match Hub::new(grant) {
        Ok(hub) => hub,
        Err(error) => return tracing::warn!("endpoint report: {error}"),
    };
    runtime.block_on(async {
        for attempt in 0..5u32 {
            match hub.report_endpoint(&endpoint).await {
                Ok(()) => return tracing::info!("reported endpoint {}", endpoint.public_host),
                Err(Refusal::Denied) => return tracing::warn!("the Hub refused the endpoint report"),
                Err(Refusal::Transport(error)) => {
                    tracing::warn!("endpoint report: {error}");
                    tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
                }
            }
        }
    });
}
