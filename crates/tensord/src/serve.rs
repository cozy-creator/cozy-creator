//! The machine role: `cozy machine serve` on this computer (Calcifer starts it when something
//! uses it), `--rental` on a pod; and the `tensord` binary's modes, which TensorD's and
//! Calcifer's tests run.
use crate::{
    config::{self, Flags},
    machine::grant::Lifetime,
};
use std::{io, process::ExitCode, sync::Arc};
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
            eprintln!("cannot set TensorD process name: {error}");
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
    config::for_what_it_starts();
    config::reread(&mut config)
        .and_then(|()| run_machine(config, ready))
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
    }
    let record = Version {
        name: "tensord",
        version: env!("CARGO_PKG_VERSION"),
        commit: Some(::api::COMMIT),
        api: ["cozy.machine.v1"],
    };
    println!("{}", serde_json::to_string(&record)?);
    Ok(())
}

/// Runs the machine from its grant: identity and readiness under the machine root, the engine
/// under `var/lib/cozy/tensord`, the API on the granted port.
fn run_machine(config: config::Config, mut ready: crate::machine::supervise::Ready) -> io::Result<()> {
    let config::Config { mut grant, visible_devices, gpu_memory_limit, mode, display_pace, endpoint, logs, .. } = config;
    // A shared computer's own programs come first: from this thread, before the runtime's, the
    // background priority reaches every thread and process the machine starts (executors too).
    if mode == ::api::config::MachineMode::Shared {
        platform::process::background_process();
    }
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
    let identity = crate::api::Leaf::machine(
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
    let engine = layout.engine();
    let generations = engine.join("generations");
    let paths = crate::machine::update::Paths::new(&engine, &layout.root);
    let (_owned, store) = own(&engine, &layout.store)?;
    // The machine's one downloader, held for its life: it reads the datacenter volume before the
    // Hub and fills it. A variable naming no mount point (the volume was not delivered) is none.
    boot.at("cache");
    let machine = Machine::new(store.clone(), layout.cache.as_deref())?;
    boot.at("journal");
    // Before the sweep: earlier executor scopes are found where this machine's go.
    if mode == api::config::MachineMode::Shared {
        contain(&engine);
    }
    // A tool that composes holds a CPU slot while it computes, and every run's output work takes one: one slot for every four cores.
    let cpu_slots = std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 4).max(1));
    let service = crate::service::Service::open(&engine, &generations, cpu_slots, store.clone())?;
    crate::objects::restore(&service.engine, &engine.join("writes"))?;
    // A short store admit frees the machine's garbage before the store evicts a model.
    let reclaimer = Arc::downgrade(&service);
    store.on_short(move |incoming| reclaimer.upgrade().into_iter().for_each(|service| service.reclaim_for(incoming)));
    // The jobs this root held (a preparation the restart ended counts as ended now): Status
    // reports the true deadline from the first frame.
    let (working, ended_ms) = service.engine.jobs(0)?;
    lifecycle.observe(crate::machine::lifecycle::Jobs { working, ended_ms }, lifecycle.epoch());
    // An update's activation holds queued runs; one that ends without its restart lets them start.
    // An admission or a follower letting go may start the idle clock.
    let (holding, executions) = (lifecycle.clone(), service.engine.clone());
    service.hold_while(move || holding.holding());
    lifecycle.on_change(move || executions.notify_activity());
    // An image's base interpreter holds this machine's PyTorch layer; elsewhere the machine owns one.
    let image_torch = layout.root.join("opt/cozy/python");
    let torch = match platform::paths::venv_python(&image_torch).is_file() {
        true => crate::published::layer::Torch::Image(image_torch),
        false => crate::published::layer::Torch::Own { engine: engine.clone(), wanted: Default::default() },
    };
    let sdk = crate::published::PackageSdk::selected(&paths.sdk(), &paths.update("staged"), crate::machine::uv::Uv::of(&layout.root, &engine, rental), torch.clone());
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
    let (readiness, committing) = (identity.readiness.clone(), updates.clone());
    std::thread::Builder::new()
        .name("readiness-report".into())
        .spawn(move || {
            let updates = committing;
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
    let (mut gpus, mut gpu_problems) = receipt::gpus(visible_devices.as_deref());
    // A driver below CUDA 13.0's floor serves no GPU; the machine stays for CPU work.
    if let Some(why) = gpus.first().and_then(|gpu| platform::gpu::below_floor(&gpu.driver_version)) {
        gpu_problems.push(format!("{why}; until then this machine runs on its CPU"));
        gpus.clear();
    }
    gpu_problems.iter().for_each(|problem| tracing::error!("{problem}"));
    boot.at("pool");
    {
        let devices: Vec<String> = gpus.iter().map(|g| g.device_index.to_string()).collect();
        let home = engine.join("executor-home");
        std::fs::create_dir_all(&home)?;
        let config = serde_json::json!({
            "devices": devices.join(","),
            "inventory": gpus,
            "problems": gpu_problems,
            "authorized_device_limit_bytes": null,
            "gpu_memory_limit": gpu_memory_limit,
            "display_pace": display_pace,
            "environment": {"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": home, "COZY_HOME": home},
            // `machine.mode`, else a rented pod is the machine's alone and anywhere else it is a guest.
            "host": {"mode": mode.name()},
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
        if let Err(error) = service.import_installed() {
            tracing::warn!("importing the installed environments: {error}");
        }
    }
    if !rental {
        let (lifecycle, engine, home) = (lifecycle.clone(), service.engine.clone(), grant.home.clone());
        let (released, parents, stopped, warm) = (service.clone(), service.clone(), service.clone(), service.clone());
        // A change of its controllers (`cozy machine authorize`, `--remove`) wakes its lifecycle.
        let (mut changes, woken) = (keys.subscribe(), service.engine.clone());
        runtime.spawn(async move {
            while changes.changed().await.is_ok() {
                woken.notify_activity();
            }
        });
        let (open, configured, authorizes) = (api.local_addr()?.ip().is_unspecified(), grant.listen_configured, home.clone());
        std::thread::Builder::new().name("local-lifecycle".into()).spawn(move || {
            crate::machine::lifecycle::exit_when_idle(
                lifecycle,
                engine,
                move || released.release_idle(),
                move || parents.release_parents(),
                move || stopped.stop(),
                // Kept up for its authorized controllers and for a warm set member it holds, unless
                // `cozy update` replaced its binary: it exits idle and Calcifer starts the new one.
                move || {
                    let swapped = crate::machine::binary::launch().is_some_and(|launch| launch.swapped());
                    (crate::machine::grant::authorized(&home) || warm.keeps_warm()) && !swapped
                },
                // On every interface exactly while another computer's controller is authorized.
                move || !configured && open != crate::machine::grant::authorized(&authorizes),
            )
        })?;
    }
    let own_hub = grant.hub.clone().filter(|_| rental);
    if let Some(hub) = own_hub.clone() {
        let hub = Arc::new(crate::machine::hub::Hub::new(hub)?);
        let lifecycle = lifecycle.clone();
        let service = service.clone();
        let provider = grant.provider.clone();
        let (state, origin) = (layout.state.clone(), machine.clone());
        std::thread::Builder::new()
            .name("rental-lifecycle".into())
            .spawn(move || {
                runtime.block_on(async {
                    tokio::spawn(crate::machine::lifecycle::keep_authority(
                        hub.clone(),
                        keys,
                    ));
                    crate::machine::speed::serve(hub.clone(), state, origin);
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
    let reach = reach(&api, grant.worker_port, endpoint.as_ref());
    let crate::api::Leaf { authority, cert_pem, key_pem, cert_der, readiness, started_at_ms } = identity;
    let identity = crate::api::MachineIdentity {
        authority,
        cert_pem,
        key_pem,
        cert_der,
        readiness,
        started_at_ms,
        lifecycle,
        hubs: own_hub.iter().map(|hub| (hub.origin.clone(), hub.worker_id.clone())).collect(),
        updates,
        webrtc: crate::machine::player::listen(&layout.state, grant.webrtc_port, !rental)?,
        player: grant.webrtc_reach.clone(),
        webrtc_port: None,
        store_logs: store.root().join("logs"),
        store: layout.store.clone(),
        logs,
        gpus,
        gpu_problems,
        gpu_memory_limit,
        mode: mode.name(),
        torch,
    };
    // The boot answer ends before the API takes its listener: one server answers at a time.
    booting.abort();
    let _ = runtime.block_on(booting);
    start_api(
        &engine,
        &generations,
        identity,
        api,
        &machine,
        &service,
        sdk,
        // A run naming no Hub reads this rental's own Hub's public content.
        own_hub,
        reach,
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
    machine: &Arc<Machine>,
    service: &Arc<crate::service::Service>,
    sdk: crate::published::PackageSdk,
    own_hub: Option<crate::machine::grant::HubGrant>,
    reach: Option<String>,
) -> io::Result<()> {
    use crate::api;
    let publisher = crate::published::Publisher::new(sdk, machine.clone());
    service.configure_publisher(publisher.clone());
    let objects = Arc::new(crate::objects::Objects::new(service.engine.clone()));
    service.engine.hold_memos(&objects);
    let runs = Arc::new(crate::runs::Runs {
        service: service.clone(),
        objects: objects.clone(),
        local: Arc::new(crate::local_source::LocalSources::new(objects, publisher.clone(), generations)),
        publisher,
        own_hub: own_hub
            .map(|hub| crate::hub::Source::new(&hub.origin, hub.ca_der, None))
            .transpose()
            .map_err(|e| io::Error::other(e.0))?,
        contexts: Default::default(),
        projection: Default::default(),
    });
    crate::calls::Calls::configure(service, machine.clone(), Some(&runs))?;
    runs.restore_warm_sets()?;
    crate::runs::resume(&runs)?;
    listener.set_nonblocking(true)?;
    #[derive(serde::Serialize)]
    struct Ready<'a> {
        address: String,
        /// Where other computers reach it (`cozy machine add`): a pod's public address as its
        /// provider mapped it, this computer's LAN address when it listens on every interface.
        #[serde(skip_serializing_if = "Option::is_none")]
        reach: Option<String>,
        worker_id: &'a str,
        boot_id: &'a str,
        cert_pem: &'a str,
    }
    let ready = serde_json::to_vec(&Ready {
        reach,
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
                    runs,
                )
                .await
            });
            if let Err(error) = result {
                tracing::error!("machine API stopped: {error}");
            }
        })?;
    Ok(())
}
/// Where other computers reach the API `listener`: the provider's public mapping of `port` on a
/// pod, else this computer's LAN address (the one its default route leaves from) when it listens
/// on every interface; none on loopback.
fn reach(listener: &std::net::TcpListener, port: Option<u16>, provider: Option<&crate::machine::endpoint::Endpoint>) -> Option<String> {
    let bound = listener.local_addr().ok()?;
    if let Some(endpoint) = provider {
        let public = endpoint.ports.get(&port.unwrap_or(bound.port()).to_string())?;
        return Some(format!("{}:{public}", endpoint.public_host));
    }
    if !bound.ip().is_unspecified() {
        return None;
    }
    // A UDP connect picks the route and sends nothing.
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("1.1.1.1:53").ok()?;
    let lan = probe.local_addr().ok()?.ip();
    (!lan.is_loopback() && !lan.is_unspecified()).then(|| format!("{lan}:{}", bound.port()))
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
        let mut backoff = tensorfs::fetch::Backoff::background();
        loop {
            let (why, stated) = match hub.report_endpoint(&endpoint).await {
                Ok(()) => return tracing::info!("reported endpoint {}", endpoint.public_host),
                Err(Refusal::Denied | Refusal::Ended) => return tracing::warn!("the Hub refused the endpoint report"),
                Err(Refusal::NotReady) => ("the Hub is not ready for it".to_owned(), None),
                Err(Refusal::Transport(error, wait)) => (error, wait),
            };
            match backoff.again(&format!("endpoint report: {why}"), false, stated) {
                Some((wait, said)) => {
                    tracing::warn!("{said}");
                    tokio::time::sleep(wait).await;
                }
                None => return tracing::warn!("{}", backoff.spent(&format!("endpoint report: {why}"))),
            }
        }
    });
}

/// A shared machine and its executors under one memory limit the kernel keeps: varena's host
/// number (`varena_ledger::host::machine`), below what this computer's other programs need;
/// the host rule follows it from here. Where the host gives no such cgroup: one warning, and only
/// the in-process limits hold.
fn contain(engine: &std::path::Path) {
    use platform::contain;
    let contained = contain::contain(&contain::namespace(engine))
        .and_then(|root| match contain::follow(varena_ledger::host::machine) {
            Some(Ok(budget)) => Ok((root, budget)),
            Some(Err(error)) => Err(format!("its limit was not written: {error}")),
            None => Err("not contained".into()),
        });
    match contained {
        Ok((root, budget)) => tracing::info!(
            "memory: this machine and its executors are contained in {} at {:.1} GiB ({:.1} GiB kept for the computer's other programs)",
            root.display(), budget.max as f64 / GIB, budget.reserve as f64 / GIB
        ),
        Err(why) => tracing::warn!(
            "memory: the kernel does not contain this machine ({why}); a run that outgrows the computer is stopped only by the machine's own limits"
        ),
    }
}

const GIB: f64 = (1u64 << 30) as f64;
