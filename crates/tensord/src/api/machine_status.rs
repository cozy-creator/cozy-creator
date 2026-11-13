//! Status (G/API.md, D2). Anyone who reaches the machine gets one identity frame with the sealed
//! readiness receipt; a machine-scope cap gets the whole picture, sent again whenever it changes.
//! An open stream is not activity: only `keepalive` resets the idle deadline, once.
use super::{auth::VerifiedActor, http::blocking, v1, MachineIdentity};
use crate::runs::Runs;
use api::error::Error;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use futures_util::Stream;

/// While something goes (a run, an operation, an update, the boot, a stuck process), how often the picture is read again:
/// progress moves with no event of its own.
const BUSY_LOOK: Duration = Duration::from_secs(1);
/// With nothing going, the picture is read again on an event (`activity::status_changed`), or after this: other
/// programs' use of the disk, a display plugged in and kernels' verdicts written elsewhere have no event.
const IDLE_LOOK: Duration = Duration::from_secs(30);

/// Nothing in it moves on its own.
fn idle(frame: &v1::StatusFrame) -> bool {
    frame.runs.is_empty() && frame.activity.is_empty() && frame.update.is_none() && frame.phase == "ready" && frame.stuck.is_empty()
}

/// A Status stream's place: what it last sent, and what it waits on.
struct Watching {
    identity: Arc<MachineIdentity>,
    runs: Arc<Runs>,
    actor: VerifiedActor,
    private: Option<api::machine::LocalPrivateStorage>,
    read: Arc<Mutex<Option<(u64, Held)>>>,
    changes: tokio::sync::watch::Receiver<u64>,
    last: Option<v1::StatusFrame>,
    looked: bool,
}

/// The whole picture, sent again whenever it changes, until the caller leaves: read each `BUSY_LOOK` while something
/// goes, else on the next event or after `IDLE_LOOK`. What the engine records (runs, environments, the warm set,
/// models) is read again only after engine activity; each look otherwise reads what the process holds in memory and the
/// disk's free space. Every read is on the blocking pool: a disk the machine's run saturates slows its Status, never
/// the API's own threads.
pub(super) fn watch(
    identity: Arc<MachineIdentity>,
    runs: Arc<Runs>,
    actor: VerifiedActor,
) -> impl Stream<Item = Result<v1::StatusFrame, Error>> + Send {
    let watching = Watching {
        identity,
        runs,
        actor,
        private: crate::private_io::current(),
        read: Arc::default(),
        changes: crate::activity::status_changes(),
        last: None,
        looked: false,
    };
    futures_util::stream::unfold(watching, |mut watching| async move {
        loop {
            if watching.looked {
                match watching.last.as_ref().is_some_and(idle) {
                    false => tokio::time::sleep(BUSY_LOOK).await,
                    true => tokio::select! {
                        _ = watching.changes.changed() => {}
                        () = tokio::time::sleep(IDLE_LOOK) => {}
                    },
                }
            }
            watching.looked = true;
            // Seen before the read: a change while it reads is read again.
            watching.changes.borrow_and_update();
            match look(&watching).await {
                Ok(next) if watching.last.as_ref().is_some_and(|last| !changed(last, &next)) => {}
                Ok(next) => {
                    // The queue and the times go again only when they changed: absent, they stand as last sent.
                    let mut sent = next.clone();
                    if let Some(last) = &watching.last {
                        if last.queue == next.queue {
                            sent.queue = None;
                        }
                        if last.times == next.times {
                            sent.times = None;
                        }
                    }
                    watching.last = Some(next);
                    return Some((Ok(sent), watching));
                }
                Err(error) => return Some((Err(error), watching)),
            }
        }
    })
}

/// One read of the whole picture.
async fn look(watching: &Watching) -> Result<v1::StatusFrame, Error> {
    let (identity, runs, read, actor) = (watching.identity.clone(), watching.runs.clone(), watching.read.clone(), watching.actor);
    crate::private_io::request_optional(watching.private.clone(), async move {
        if let Some(scope) = crate::private_io::current() {
            tokio::task::spawn_blocking(move || crate::private_io::resolve(&scope)).await
                .map_err(|_| Error::unavailable("private_storage_stopped", "private workspace verification stopped"))?
                .map_err(|_| Error::forbidden("private_storage_locked", "the private project is locked"))?;
        }
        // Read before the records: activity meanwhile is read again at the next look.
        let epoch = runs.service.engine.activity_epoch();
        let kept = read.lock().unwrap().clone();
        let held = match kept.filter(|(at, _)| *at == epoch) {
            Some((_, held)) => held,
            None => {
                let held = held_now(&identity, &runs, actor).await?;
                *read.lock().unwrap() = Some((epoch, held.clone()));
                held
            }
        };
        pictured(identity, &runs, held).await
    }).await
}

/// Disk use moves all the time; it alone is news once it moved by a hundredth of the disk.
/// Page accounting moves with every allocation: never news alone.
fn changed(last: &v1::StatusFrame, next: &v1::StatusFrame) -> bool {
    let moved = match (&last.disk, &next.disk) {
        (Some(a), Some(b)) => a.free_bytes.abs_diff(b.free_bytes) >= b.total_bytes / 100,
        (a, b) => a != b,
    };
    let still = |frame: &v1::StatusFrame| v1::StatusFrame {
        disk: None,
        gpus: frame.gpus.iter().map(|gpu| v1::Gpu { memory: None, ..gpu.clone() }).collect(),
        ..frame.clone()
    };
    moved || still(last) != still(next)
}

pub(super) fn identity_frame(identity: &MachineIdentity) -> v1::StatusFrame {
    v1::StatusFrame {
        worker_id: identity.authority.worker_id.clone(),
        boot_id: identity.authority.boot_id.clone(),
        version: ::api::VERSION.into(),
        commit: api::COMMIT.into(),
        dev: identity.updates.dev(),
        workload_key: crate::hub::workload_key().thumbprint().into(),
        receipt: identity.readiness.facts().unwrap_or_default(),
        ..Default::default()
    }
}

/// What the engine records for the caller: its live runs, environments, warm set and models.
#[derive(Clone)]
struct Held {
    runs: Vec<v1::RunState>,
    environments: Vec<v1::Environment>,
    warm: Vec<v1::WarmItem>,
    models: crate::held_models::Listing,
    /// Each GPU's page accounting by UUID.
    memory: std::collections::BTreeMap<String, v1::GpuMemory>,
    /// GPUs out of service by the stuck pid holding them.
    lost: std::collections::BTreeMap<u32, crate::gpu_service::Lost>,
    /// Why each GPU out of service is, by UUID.
    out: std::collections::BTreeMap<String, String>,
    /// Every root not ended, as the caller reads it, and each function's times (cl-967).
    queue: Vec<v1::Queued>,
    times: Vec<v1::Timed>,
    link: f64,
    serving: u32,
    nvlink: bool,
    /// The caller's lists' runs (`ahead`), never among `runs`: their states, and their downloads in its activity.
    ahead: Vec<v1::RunState>,
}

async fn held_now(identity: &Arc<MachineIdentity>, runs: &Arc<Runs>, actor: VerifiedActor) -> Result<Held, Error> {
    let store = identity.store.clone();
    blocking(runs, move |runs| {
        let service = &runs.service;
        let problem = |error: std::io::Error| Error::internal("status_unreadable", error.to_string());
        let reader = crate::runs::actor_id(actor);
        let queue = (crate::estimate::roots(service).map_err(problem)?.into_iter())
            .map(|root| v1::Queued {
                run: match actor.owner || root.actor == reader {
                    true => root.caller_id,
                    false => String::new(),
                },
                ..root.queued
            })
            .collect();
        Ok(Held {
            queue,
            times: service.engine.all_run_times(api::estimate::HISTORY).map_err(problem)?,
            link: crate::estimate::link(service),
            serving: service.pool().map_or(0, |pool| pool.serving_width() as u32),
            nvlink: service.pool().is_some_and(|pool| pool.hardware_class().1),
            ahead: crate::ahead::listed(&reader).iter().filter_map(|id| runs.state(actor, id).ok()).collect(),
            runs: runs.live(actor)?,
            environments: runs.environments(actor)?,
            warm: runs.warm(actor)?,
            models: crate::held_models::listing(&store),
            memory: runs.memory(),
            lost: runs.lost(),
            out: runs.out_of_service(),
        })
    })
    .await
}

pub(super) async fn frame(identity: &Arc<MachineIdentity>, runs: &Arc<Runs>, actor: VerifiedActor) -> Result<v1::StatusFrame, Error> {
    pictured(identity.clone(), runs, held_now(identity, runs, actor).await?).await
}

/// `picture` on the blocking pool: it reads the disk (the store's space, the installed software,
/// the kernels' verdicts) and `/proc`.
async fn pictured(identity: Arc<MachineIdentity>, runs: &Arc<Runs>, held: Held) -> Result<v1::StatusFrame, Error> {
    blocking(runs, move |runs| {
        let resident = runs.service.pool().map(|pool| pool.resident()).unwrap_or_default();
        Ok(picture(&identity, held, resident))
    })
    .await
}

/// Whether a GPU drives a display, read from the driver at most every 10 s: a monitor is plugged in rarely, and each read opens NVML.
fn drives_display(uuid: &str) -> bool {
    static READ: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> = LazyLock::new(Mutex::default);
    let mut read = READ.lock().unwrap();
    if let Some((_, display)) = read.get(uuid).filter(|(at, _)| at.elapsed() < Duration::from_secs(10)) {
        return *display;
    }
    let display = platform::gpu::Device::open(uuid).and_then(|device| device.display_active()).unwrap_or(false);
    read.insert(uuid.to_owned(), (Instant::now(), display));
    display
}

fn picture(identity: &MachineIdentity, held: Held, resident: Vec<v1::Resident>) -> v1::StatusFrame {
    let Held { runs, environments, warm, models, mut memory, lost, mut out, queue, times, link, serving, nvlink, ahead } = held;
    let (updates, lifecycle) = (&identity.updates, &identity.lifecycle);
    let software = updates.software();
    let activity = crate::activity::listed(|id| runs.iter().any(|run| run.id == id) || ahead.iter().any(|own| own.id == id));
    v1::StatusFrame {
        idle_deadline_unix_ms: lifecycle.deadline_ms(),
        runs,
        activity,
        update: updates.current().map(|update| super::machine_update::state(&update.operation, &update)),
        phase: match (lifecycle.released(), identity.readiness.proved()) {
            (true, _) => "releasing",
            (false, false) => "booting",
            (false, true) => "ready",
        }
        .into(),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        started_at_unix_ms: identity.started_at_ms as i64,
        gpus: identity
            .gpus
            .iter()
            .map(|g| v1::Gpu {
                index: g.device_index,
                name: g.device_name.clone(),
                uuid: g.device_uuid.clone(),
                pci_bus_id: g.pci_bus_id.clone(),
                memory_bytes: g.memory_bytes,
                driver: g.driver_version.clone(),
                family: match g.compute_cap.as_str() {
                    "" => String::new(),
                    cc => crate::precision::Gpus::of(&[cc]).name,
                },
                memory_class: g.memory_class.clone(),
                memory_limit_bytes: identity.gpu_memory_limit.filter(|&l| l < g.memory_bytes).unwrap_or(0),
                display: drives_display(&g.device_uuid),
                memory: memory.remove(&g.device_uuid),
                out_of_service: out.remove(&g.device_uuid).unwrap_or_default(),
            })
            .collect(),
        hubs: identity
            .hubs
            .iter()
            .map(|(origin, machine_id)| v1::Hub {
                origin: origin.clone(),
                machine_id: machine_id.clone(),
            })
            .collect(),
        runtime: software.runtime,
        tensorfs: software.tensorfs,
        torch: identity.torch.version(),
        environments,
        warm,
        disk: disk(&identity.store),
        models: models.models,
        models_bytes: models.bytes,
        webrtc: identity.webrtc_port.map(|port| {
            crate::machine::player::endpoint(port, &identity.player, &identity.cert_der)
        }),
        mode: identity.mode.into(),
        driver_floor: match identity.gpus.is_empty() {
            true => String::new(),
            false => platform::gpu::DRIVER_FLOOR.into(),
        },
        gpu_problems: identity.gpu_problems.clone(),
        toolchain: updates.toolchain(),
        kernels: updates.kernels(),
        tensord_pid: std::process::id(),
        stuck: platform::contain::stuck()
            .into_iter()
            .map(|s| {
                let gpus = lost.get(&s.pid).map(|l| l.gpus.clone()).unwrap_or_default();
                v1::StuckProcess {
                    pid: s.pid, command: s.command, wchan: s.wchan, since_unix_ms: s.since_unix_ms, resident_bytes: s.resident_bytes,
                    gpu_bytes: gpus.iter().map(|(_, bytes)| bytes).sum(), gpus: gpus.into_iter().map(|(gpu, _)| gpu).collect(),
                }
            })
            .collect(),
        heat: crate::slowed::heat(),
        runtime_kernels: crate::slowed::runtime_kernels(),
        queue: Some(queue),
        times: Some(times),
        resident,
        link_bytes_per_s: link,
        serving_gpus: serving,
        nvlink,
        ahead,
        ..identity_frame(identity)
    }
}

fn disk(path: &std::path::Path) -> Option<v1::Disk> {
    let space = platform::durable::space(path).ok()?;
    crate::reclaim::seen_free(space.free);
    let usage = crate::reclaim::usage().unwrap_or_default();
    Some(v1::Disk {
        total_bytes: space.total,
        free_bytes: space.free,
        models_bytes: usage.models,
        environments_bytes: usage.environments,
        runtime_bytes: usage.runtime,
        runs_bytes: usage.runs,
    })
}
