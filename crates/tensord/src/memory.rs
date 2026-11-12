//! TensorD decides no GPU bytes: varena's core does, and TensorD ends an idle executor only when
//! a core's ask went unanswered (`gpu_service`). What is here is GPU status, and host RAM, which
//! no core owns: the pressure watcher and each call's measured anonymous peak.
use crate::machine::receipt;
use platform::contain::host_memory;
use platform::contain::host_pressure::{self, Growth, HostMode, Nudge};
use platform::signal::{wait_any, Signal, Waitable};
use platform::process::Exact;
use std::{collections::BTreeMap, fs::{self, OpenOptions}, io::{self, Write}, path::PathBuf,
    sync::{Arc, Condvar, LockResult, Mutex, MutexGuard, PoisonError}, time::{SystemTime, UNIX_EPOCH}};

const ROTATE_BYTES: u64 = 4 << 20;

/// The measurements of one physical GPU. Allocation stays in each executor.
pub(crate) struct Gpu {
    pub figures: Figures,
    pub model: String,
    pub uuid: String,
    /// What the card has beyond the development `gpu_memory_limit`: never free here.
    absent: u64,
}

impl Gpu {
    pub(crate) fn create(entry: &str, inventory: &[receipt::Gpu], limit: Option<u64>, peaks: &Peaks, log: &MemoryLog) -> Option<Self> {
        let row = row(entry, inventory)?;
        let figures = Figures::new().map_err(|error| log.note(serde_json::json!({"event": "gpu_figures_unavailable", "gpu": entry, "why": error.to_string()}))).ok()?;
        let absent = limit.map_or(0, |limit| row.memory_bytes.saturating_sub(limit));
        let gpu = Self { figures, model: row.device_name.clone(), uuid: row.device_uuid.clone(), absent };
        peaks.restore(&gpu.model, &gpu.figures);
        Some(gpu)
    }

    /// A current driver reading, never a reservation or permission to allocate; under a
    /// development `gpu_memory_limit`, a card of that size's.
    pub(crate) fn free(&self) -> Option<u64> {
        platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.free()).ok().map(|free| free.saturating_sub(self.absent))
    }

    /// Driver-attributed process usage. Missing pid accounting is unknown, not zero.
    pub(crate) fn status(&self, tenants: &Tenants) -> Option<api::machine::GpuMemory> {
        let read = platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.read()).ok()?;
        let processes = read.processes?;
        let book = tenants.lock().unwrap_or_else(|e| e.into_inner());
        if !book.is_empty() && !book.keys().any(|pid| processes.contains_key(pid)) {
            return None; // e.g. the driver's host pids differ from this container's
        }
        let processes: Vec<_> = processes.into_iter().filter_map(|(pid, bytes)| {
            let tenant = book.get(&pid)?;
            let visible = match crate::private_io::current() { Some(scope) => tenant.plan.starts_with(&crate::private_io::executor_prefix(&scope)), None => !tenant.plan.starts_with("private-") };
            if !visible { return None; }
            Some(api::machine::ProcessMemory { pid, executor: format!("{}#{}", tenant.plan, tenant.start), bytes })
        }).collect();
        let allocated_bytes: u64 = processes.iter().map(|p| p.bytes).sum();
        Some(api::machine::GpuMemory {
            capacity_bytes: read.total, free_bytes: read.free,
            foreign_bytes: read.total.saturating_sub(read.free).saturating_sub(allocated_bytes),
            allocated_bytes, processes, ..Default::default()
        })
    }
}

/// Inventory identity, as CUDA_VISIBLE_DEVICES names it.
pub(crate) fn row<'a>(entry: &str, inventory: &'a [receipt::Gpu]) -> Option<&'a receipt::Gpu> {
    inventory.iter().find(|g| g.device_index.to_string() == entry || g.device_uuid == entry)
}

pub(crate) struct Tenant {
    pub plan: String,
    pub start: usize,
    /// Its lane's GPUs from `start`.
    pub width: usize,
    pub exact: Exact,
}

pub(crate) type Tenants = Arc<Registry>;
/// Executors by their pid here.
#[derive(Default)]
pub(crate) struct Registry(Mutex<BTreeMap<u32, Tenant>>);
impl Registry {
    pub(crate) fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, BTreeMap<u32, Tenant>>> { self.0.lock() }
    pub(crate) fn register(&self, tenants: impl IntoIterator<Item = (u32, Tenant)>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend(tenants);
    }
}

#[derive(Default)]
struct Book {
    values: BTreeMap<String, u64>,
    dispatched: BTreeMap<String, String>,
}

/// Each `<package>|<shape>`'s measured anonymous host high-water.
pub(crate) struct Figures {
    book: Mutex<Book>,
    changed: Signal,
}

impl Figures {
    fn new() -> io::Result<Self> { Ok(Self { book: Mutex::default(), changed: Signal::new().map_err(io::Error::other)? }) }
    pub(crate) fn dispatch(&self, request: &str, package: &str, shape: &str) {
        self.book.lock().unwrap_or_else(|e| e.into_inner()).dispatched.insert(request.into(), format!("{package}|{shape}"));
    }
    pub(crate) fn undispatch(&self, request: &str) {
        self.book.lock().unwrap_or_else(|e| e.into_inner()).dispatched.remove(request);
    }
    pub(crate) fn record_request(&self, request: &str, host: u64) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let Some(key) = book.dispatched.get(request).cloned() else { return };
        larger(&mut book.values, key, host);
        self.changed.set();
    }
    pub(crate) fn forget_project(&self, workspace: &str) {
        let prefix = format!("{workspace}:");
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.values.retain(|key, _| !key.starts_with(&prefix));
        book.dispatched.retain(|key, _| !key.starts_with(&prefix));
    }
    pub(crate) fn host_peak(&self, package: &str, shape: &str) -> Option<u64> {
        let book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let key = format!("{package}|{shape}");
        if shape.is_empty() {
            book.values.range(key.clone()..).take_while(|(k, _)| k.starts_with(&key)).map(|(_, v)| *v).max()
        } else { book.values.get(&key).copied() }
    }
    fn restore(&self, peaks: &[Peak]) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        for peak in peaks { larger(&mut book.values, peak.key.clone(), peak.bytes); }
    }

}
fn larger(figures: &mut BTreeMap<String, u64>, key: String, bytes: u64) {
    let value = figures.entry(key).or_insert(bytes); *value = (*value).max(bytes);
}

/// A request's anonymous host high-water at `<package>|<shape>`, as TensorD persists it per GPU
/// model.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Peak {
    key: String,
    bytes: u64,
}

impl Peak {
    /// The largest of each key, in key order: what every GPU of one model persists.
    pub(crate) fn merge(peaks: impl IntoIterator<Item = Peak>) -> Vec<Peak> {
        let mut figures = BTreeMap::new();
        for peak in peaks {
            larger(&mut figures, peak.key, peak.bytes);
        }
        figures.into_iter().map(|(key, bytes)| Peak { key, bytes }).collect()
    }
}

/// Measured peaks, persisted per GPU model as `<dir>/<model>.json`: the largest of each key across
/// every GPU of the model. A key names its package release (`<construction>|<shape>`), so one file
/// holds every release's. Each restore and each saved change is a memory-log note.
pub(crate) struct Peaks {
    dir: PathBuf,
    saved: Mutex<BTreeMap<String, Vec<Peak>>>,
    log: MemoryLog,
}

impl Peaks {
    pub(crate) fn at(dir: PathBuf, log: MemoryLog) -> Self {
        Self { dir, saved: Mutex::default(), log }
    }

    fn path(&self, model: &str) -> PathBuf {
        let name: String = model.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        self.dir.join(format!("{name}.json"))
    }

    fn restore(&self, model: &str, figures: &Figures) {
        let mut saved = self.saved.lock().unwrap();
        let held = saved.entry(model.into()).or_insert_with(|| {
            let read = fs::read(self.path(model)).ok().and_then(|bytes| serde_json::from_slice::<Vec<Peak>>(&bytes).ok());
            Peak::merge(read.unwrap_or_default())
        });
        figures.restore(held);
        self.log.note(serde_json::json!({"event": "peaks_restored", "model": model, "peaks": held.len()}));
    }

    /// Measurements derived from calls are process-local. A machine-wide disk cache cannot
    /// distinguish a public calibration from a project-derived construction or shape.
    pub(crate) fn save<'a>(&self, _: &str, _: impl Iterator<Item = &'a Figures>) {}

}

/// One process on a call's GPU. A single process may own several GPUs; a group has one per rank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rank {
    pub pid: u32,
    pub gpu: String,
}

/// One call running on the machine's GPUs: every participant and its `<package>|<shape>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Call {
    pub ranks: Vec<Rank>,
    pub package: String,
    pub shape: String,
}

/// TensorD's calls running until they end, including each follower the host rule must watch. One starting wakes
/// `wait_busy`.
#[derive(Clone, Default)]
pub(crate) struct Running(Arc<(Mutex<BTreeMap<String, Call>>, Condvar)>);

impl Running {
    pub(crate) fn lock(&self) -> LockResult<MutexGuard<'_, BTreeMap<String, Call>>> {
        self.0.0.lock()
    }

    pub(crate) fn start(&self, id: String, call: Call) {
        self.lock().unwrap_or_else(PoisonError::into_inner).insert(id, call);
        self.0.1.notify_all();
    }

    /// Returns once a call runs: at once if one does.
    pub(crate) fn wait_busy(&self) {
        let calls = self.lock().unwrap_or_else(PoisonError::into_inner);
        drop(self.0.1.wait_while(calls, |calls| calls.is_empty()).unwrap_or_else(PoisonError::into_inner));
    }
}

/// A process running calls: measured anonymous peak, anonymous bytes now, and call count.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HostCall {
    pub pid: u32,
    pub peak: Option<u64>,
    pub anon: u64,
    pub running: u64,
}

/// Every process of every call (each rank of a group), with the host peak its own GPU measured.
/// One process on several GPUs is one row at the largest of their figures; the host rule
/// deduplicates a process shared by calls. File-backed model pages remain in the OS budget.
pub(crate) fn host_calls(running: &Running, gpus: &[Arc<Gpu>]) -> Vec<HostCall> {
    let calls: Vec<Call> = running.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
    let mut rows = vec![];
    for call in calls {
        let mut processes = BTreeMap::<u32, Option<u64>>::new();
        for rank in &call.ranks {
            let gpu = gpus.iter().find(|gpu| gpu.uuid == rank.gpu);
            let peak = gpu.and_then(|gpu| gpu.figures.host_peak(&call.package, &call.shape));
            let held = processes.entry(rank.pid).or_default();
            *held = (*held).max(peak);
        }
        rows.extend(processes.into_iter().map(|(pid, peak)| {
            let anon = host_memory::resident(pid).map_or(0, |now| now.anon);
            HostCall { pid, peak, anon, running: 1 }
        }));
    }
    rows
}

/// Memory observations wake the host rule and persistence, never page reclamation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Woke { Gpu, HostShort }
pub(crate) enum Source { Gpu(Arc<Gpu>), Host(Arc<Host>) }
impl Source {
    fn waitable(&self) -> Waitable<'_> {
        match self { Source::Gpu(gpu) => gpu.figures.changed.waitable(), Source::Host(host) => host.short.waitable() }
    }
    fn clear(&self) -> Woke {
        match self {
            Source::Gpu(gpu) => { gpu.figures.changed.clear(); Woke::Gpu }
            Source::Host(host) => { host.short(); Woke::HostShort }
        }
    }
}

/// Blocks on every source until the returned guard drops; `wake` runs once per wake with what
/// woke it. Every ready source is cleared on each wake, so none starves; one that broke (hung up,
/// failed) is dropped, never spun on. It never times out.
pub(crate) fn react(sources: Vec<Source>, wake: impl Fn(Woke) + Send + 'static) -> io::Result<Reacting> {
    let stop = Arc::new(Signal::new().map_err(io::Error::other)?);
    let stopped = stop.clone();
    std::thread::Builder::new().name("memory-events".into()).spawn(move || {
        let mut live: Vec<&Source> = sources.iter().collect();
        loop {
            let waitables: Vec<Waitable<'_>> = std::iter::once(stopped.waitable()).chain(live.iter().map(|source| source.waitable())).collect();
            let ready = match wait_any(&waitables) {
                Ok(ready) => ready,
                Err(error) => return tracing::warn!("memory: events: {error}"),
            };
            if ready.iter().any(|ready| ready.index == 0) {
                return;
            }
            let mut woke = vec![];
            for ready in ready.iter().filter(|ready| !ready.broken) {
                let what = live[ready.index - 1].clear();
                if !woke.contains(&what) {
                    woke.push(what);
                }
            }
            let broken: Vec<usize> = ready.iter().filter(|ready| ready.broken).map(|ready| ready.index - 1).collect();
            live = live.into_iter().enumerate().filter(|(i, _)| !broken.contains(i)).map(|(_, source)| source).collect();
            woke.into_iter().for_each(&wake);
        }
    })?;
    Ok(Reacting(stop))
}

/// The events thread's life: dropped, it ends.
pub(crate) struct Reacting(Arc<Signal>);

impl Drop for Reacting {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// The host rule (#1851) on the platform's watcher: each pressure event, or a nudge (TensorD's
/// own events, where the kernel gives none), is a look. What the calls running on these GPUs
/// must keep free, each process's measured anonymous peak less its anonymous bytes now plus
/// the growth seen past it ([`Growth`]), against the live budget, page cache counted as room.
/// Short, the events thread hears it ([`Woke::HostShort`]): the pool ends one idle process, and
/// the next event looks again. Another is ended only once the last end measurably raised the
/// budget: when ending cannot help (a growing call alone exceeds the host), the call meets its
/// own limit and the idle processes stay. A look that is not short starts over. No timer.
pub(crate) struct Host {
    short: Arc<Signal>,
    nudge: Nudge,
}

impl Host {
    /// Watches while the process lives; None (a memory-log note says why) where the platform
    /// gives no pressure events.
    pub(crate) fn watch(mode: HostMode, gpus: Vec<Arc<Gpu>>, running: Running, log: &MemoryLog) -> Option<Arc<Host>> {
        let armed = host_pressure::Pressure::arm(mode).and_then(|pressure| Ok((pressure, Arc::new(Signal::new()?))));
        let (mut pressure, short) = armed
            .inspect_err(|error| log.note(serde_json::json!({"event": "host_unwatched", "mode": format!("{mode:?}"), "why": error.to_string()})))
            .ok()?;
        let nudge = pressure.nudge();
        let (signal, notes) = (short.clone(), log.clone());
        let watching = std::thread::Builder::new().name("host-pressure".into()).spawn(move || {
            let mut keep = Keep::default();
            // The room when short was last signalled: the next signal needs more than it.
            let mut signalled: Option<i64> = None;
            // The processes and peaks the last note named: a note when they change.
            let mut noted: Vec<(u32, Option<u64>)> = vec![];
            // The contained machine's limit the last note named.
            let mut limit: Option<u64> = None;
            while pressure.wait().is_ok() {
                // A contained machine's limit follows the computer's other programs first: the
                // budget read below is then the kernel's.
                let reserve = varena_ledger::host::budget().reserve.max(0) as u64;
                match platform::contain::follow(varena_ledger::host::machine) {
                    Some(Ok(budget)) if limit != Some(budget.max) => {
                        notes.note(serde_json::json!({"event": "host_limit", "budget": budget}));
                        limit = Some(budget.max);
                    }
                    Some(Err(error)) if limit.is_some() => {
                        notes.note(serde_json::json!({"event": "host_limit_unwritten", "why": error.to_string()}));
                        limit = None;
                    }
                    _ => (),
                }
                let calls = host_calls(&running, &gpus);
                let named: Vec<(u32, Option<u64>)> = calls.iter().map(|call| (call.pid, call.peak)).collect();
                if named != noted {
                    notes.note(serde_json::json!({"event": "host_calls", "calls": calls}));
                    noted = named;
                }
                let need = i64::try_from(keep.look(calls)).unwrap_or(i64::MAX);
                let available = host_memory::read().available;
                let mut room = if available < 0 { i64::MAX } else { available.saturating_sub(need) };
                // A person's computer keeps its reserve for its own programs with no call running
                // too: an idle executor's pinned pages are what gives it back.
                let outside = host_memory::available_outside();
                if mode == HostMode::Shared && outside >= 0 {
                    room = room.min(outside - i64::try_from(reserve).unwrap_or(i64::MAX));
                }
                if room >= 0 {
                    signalled = None;
                } else if signalled.is_none_or(|was| room > was) {
                    signal.set();
                    signalled = Some(room);
                }
            }
        });
        if let Err(error) = watching {
            log.note(serde_json::json!({"event": "host_unwatched", "why": error.to_string()}));
            return None;
        }
        Some(Arc::new(Host { short, nudge }))
    }

    /// Something happened (a call began, an executor's frame came): the watcher looks.
    pub(crate) fn nudge(&self) {
        self.nudge.send();
    }

    /// Whether a look found the host short since this was last asked (the events thread asks).
    pub(crate) fn short(&self) -> bool {
        self.short.clear()
    }
}

/// What the host keeps free for the calls running on it: each process's anonymous growth still
/// to come (its requests' measured anonymous peak less its anonymous bytes now) and [`Growth`],
/// the most a look saw them grow past what was measured to come (an unmeasured call's every
/// byte). Page cache is never need: the budget holds it. With no call running nothing can grow,
/// so nothing is kept: an idle executor the next call would use is not ended for growth to come.
#[derive(Default)]
struct Keep {
    growth: Growth,
    /// Each process's requests running at the last look: each that ended settles the growth.
    running: BTreeMap<u32, u64>,
}

impl Keep {
    /// A look at the processes running calls: what must stay free.
    fn look(&mut self, calls: Vec<HostCall>) -> u64 {
        // A process running several calls is one entry.
        let mut by: BTreeMap<u32, HostCall> = BTreeMap::new();
        for call in calls {
            by.entry(call.pid)
                .and_modify(|seen| {
                    seen.peak = seen.peak.zip(call.peak).map(|(a, b)| a.max(b));
                    seen.running += call.running;
                })
                .or_insert(call);
        }
        let (mut need, mut unforeseen) = (0u64, BTreeMap::new());
        for call in by.values() {
            let peak = call.peak.unwrap_or(0);
            need = need.saturating_add(peak.saturating_sub(call.anon));
            unforeseen.insert(call.pid.to_string(), call.anon.saturating_sub(peak));
        }
        for (pid, was) in &self.running {
            for _ in by.get(pid).map_or(0, |call| call.running)..*was {
                self.growth.settle();
            }
        }
        self.running = by.values().map(|call| (call.pid, call.running)).collect();
        let growth = self.growth.observe(unforeseen);
        if by.is_empty() { 0 } else { need.saturating_add(growth) }
    }
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Where one machine's memory events go: `memory.log` in its store's `logs`, beside TensorFS's
/// transport log, read by `cozy machine logs --memory`. None logs to tracing.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryLog(Option<Arc<PathBuf>>);

impl MemoryLog {
    pub(crate) fn at(dir: PathBuf) -> Self {
        Self(Some(Arc::new(dir)))
    }

    pub(crate) fn note(&self, event: serde_json::Value) {
        let private = crate::private_io::current();
        let selected = private.as_ref().and_then(|scope| crate::private_io::root(scope).ok()).map(|root| root.join("logs"));
        if private.is_some() && selected.is_none() { return; }
        // Unscoped asynchronous pool events may contain plans/requests from any project.
        // Persist only the explicitly system-owned host budget globally.
        if selected.is_none() && event["event"] != "host_limit" { return; }
        let Some(dir) = selected.as_deref().or(self.0.as_deref().map(|p| p.as_path())) else { return; };
        let Ok(writing) = crate::config::legacy_diagnostics_read() else { return };
        let path = dir.join("memory.log");
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > ROTATE_BYTES) {
            let _ = fs::rename(&path, dir.join("memory.log.1"));
        }
        let line = format!("{:.3} {event}\n", now_ms() as f64 / 1e3);
        let written = fs::create_dir_all(dir)
            .and_then(|_| OpenOptions::new().create(true).append(true).open(&path))
            .and_then(|mut file| file.write_all(line.as_bytes()));
        drop(writing);
        if let Err(error) = written {
            if private.is_none() { tracing::warn!("memory log unavailable: {error}"); }
        }
    }
}
