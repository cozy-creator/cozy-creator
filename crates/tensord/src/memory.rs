//! TensorD decides no bytes: varena does, the GPU's in each core and the host's in its number
//! (`varena_ledger::host`), and TensorD ends an idle executor only when an ask went unanswered
//! (`gpu_service`). What is here is GPU status and the host pressure watcher, which applies
//! varena's number to a contained machine and hears when cozy is past it.
use crate::machine::receipt;
use platform::contain::host_pressure::{self, HostMode, Nudge};
use platform::memory::{wait_any, Signal, Waitable};
use platform::process::Exact;
use std::{collections::BTreeMap, fs::{self, OpenOptions}, io::{self, Write}, path::PathBuf,
    sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};

const ROTATE_BYTES: u64 = 4 << 20;

/// The measurements of one physical GPU. Allocation stays in each executor.
pub struct Gpu {
    pub figures: Figures,
    pub model: String,
    pub uuid: String,
    /// What the card has beyond the development `gpu_memory_limit`: never free here.
    absent: u64,
}

impl Gpu {
    pub fn create(entry: &str, inventory: &[receipt::Gpu], limit: Option<u64>, peaks: &Peaks, log: &MemoryLog) -> Option<Self> {
        let row = row(entry, inventory)?;
        let figures = Figures::new().map_err(|error| log.note(serde_json::json!({"event": "gpu_figures_unavailable", "gpu": entry, "why": error.to_string()}))).ok()?;
        let absent = limit.map_or(0, |limit| row.memory_bytes.saturating_sub(limit));
        let gpu = Self { figures, model: row.device_name.clone(), uuid: row.device_uuid.clone(), absent };
        peaks.restore(&gpu.model, &gpu.figures);
        Some(gpu)
    }

    /// A current driver reading, never a reservation or permission to allocate; under a
    /// development `gpu_memory_limit`, a card of that size's.
    pub fn free(&self) -> Option<u64> {
        platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.free()).ok().map(|free| free.saturating_sub(self.absent))
    }

    /// Driver-attributed process usage. Missing pid accounting is unknown, not zero.
    pub fn status(&self, tenants: &Tenants) -> Option<protocol::machine::GpuMemory> {
        let read = platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.read()).ok()?;
        let processes = read.processes?;
        let book = tenants.lock().unwrap_or_else(|e| e.into_inner());
        if !book.is_empty() && !book.keys().any(|pid| processes.contains_key(pid)) {
            return None; // e.g. the driver's host pids differ from this container's
        }
        let processes: Vec<_> = processes.into_iter().filter_map(|(pid, bytes)| {
            let tenant = book.get(&pid)?;
            Some(protocol::machine::ProcessMemory { pid, executor: format!("{}#{}", tenant.plan, tenant.start), bytes })
        }).collect();
        let allocated_bytes: u64 = processes.iter().map(|p| p.bytes).sum();
        Some(protocol::machine::GpuMemory {
            capacity_bytes: read.total, free_bytes: read.free,
            foreign_bytes: read.total.saturating_sub(read.free).saturating_sub(allocated_bytes),
            allocated_bytes, processes, ..Default::default()
        })
    }
}

/// Inventory identity, as CUDA_VISIBLE_DEVICES names it.
pub fn row<'a>(entry: &str, inventory: &'a [receipt::Gpu]) -> Option<&'a receipt::Gpu> {
    inventory.iter().find(|g| g.device_index.to_string() == entry || g.device_uuid == entry)
}

pub struct Tenant {
    pub plan: String,
    pub start: usize,
    pub exact: Exact,
}

pub type Tenants = Arc<Registry>;
/// Executors by their pid here.
#[derive(Default)]
pub struct Registry(Mutex<BTreeMap<u32, Tenant>>);
impl Registry {
    pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, BTreeMap<u32, Tenant>>> { self.0.lock() }
    pub fn register(&self, tenants: impl IntoIterator<Item = (u32, Tenant)>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend(tenants);
    }
}

#[derive(Default)]
struct Book {
    values: BTreeMap<String, u64>,
    dispatched: BTreeMap<String, String>,
}

/// Each `<package>|<shape>`'s measured anonymous host high-water.
pub struct Figures {
    book: Mutex<Book>,
    changed: Signal,
}

impl Figures {
    fn new() -> io::Result<Self> { Ok(Self { book: Mutex::default(), changed: Signal::new().map_err(io::Error::other)? }) }
    pub fn dispatch(&self, request: &str, package: &str, shape: &str) {
        self.book.lock().unwrap_or_else(|e| e.into_inner()).dispatched.insert(request.into(), format!("{package}|{shape}"));
    }
    pub fn undispatch(&self, request: &str) {
        self.book.lock().unwrap_or_else(|e| e.into_inner()).dispatched.remove(request);
    }
    pub fn record_request(&self, request: &str, host: u64) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let Some(key) = book.dispatched.get(request).cloned() else { return };
        larger(&mut book.values, key, host);
        self.changed.set();
    }
    fn restore(&self, peaks: &[Peak]) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        for peak in peaks { larger(&mut book.values, peak.key.clone(), peak.bytes); }
    }
    fn peaks(&self) -> Vec<Peak> {
        let book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        book.values.iter().map(|(key, bytes)| Peak { key: key.clone(), bytes: *bytes }).collect()
    }
}
fn larger(figures: &mut BTreeMap<String, u64>, key: String, bytes: u64) {
    let value = figures.entry(key).or_insert(bytes); *value = (*value).max(bytes);
}

/// A request's anonymous host high-water at `<package>|<shape>`, as TensorD persists it per GPU
/// model.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Peak {
    key: String,
    bytes: u64,
}

impl Peak {
    /// The largest of each key, in key order: what every GPU of one model persists.
    pub fn merge(peaks: impl IntoIterator<Item = Peak>) -> Vec<Peak> {
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
pub struct Peaks {
    dir: PathBuf,
    saved: Mutex<BTreeMap<String, Vec<Peak>>>,
    log: MemoryLog,
}

impl Peaks {
    pub fn at(dir: PathBuf, log: MemoryLog) -> Self {
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

    /// Saves what the GPUs of `model`, each holding what was restored into it, now
    /// hold, when it changed.
    pub fn save<'a>(&self, model: &str, figures: impl Iterator<Item = &'a Figures>) {
        let mut saved = self.saved.lock().unwrap();
        let held = Peak::merge(figures.flat_map(Figures::peaks));
        let before = saved.get(model).map_or(&[][..], Vec::as_slice);
        if before == held {
            return;
        }
        let changed: Vec<&Peak> = held.iter().filter(|peak| !before.contains(peak)).collect();
        let written = fs::create_dir_all(&self.dir)
            .map_err(platform::Error::from)
            .and_then(|()| platform::durable::replace(&self.path(model), &serde_json::to_vec(&held).unwrap_or_default(), 0o600));
        match written {
            Ok(()) => {
                self.log.note(serde_json::json!({"event": "peaks_saved", "model": model, "changed": changed}));
                saved.insert(model.into(), held);
            }
            Err(error) => tracing::warn!("memory: {model}'s peaks not saved: {error}"),
        }
    }
}

/// Memory observations wake the host rule and persistence, never page reclamation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woke { Gpu, HostShort }
pub enum Source { Gpu(Arc<Gpu>), Host(Arc<Host>) }
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
pub fn react(sources: Vec<Source>, wake: impl Fn(Woke) + Send + 'static) -> io::Result<Reacting> {
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
pub struct Reacting(Arc<Signal>);

impl Drop for Reacting {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// varena's host number on the platform's watcher: each pressure event, or a nudge (TensorD's own
/// events, where the kernel gives none), is a look. A contained machine's limit follows the
/// computer's other programs first (`varena_ledger::host::machine`, applied, never sized here).
/// When what cozy holds is past the number (`Budget::room` below zero: the computer's other
/// programs grew, or a process with no tier did), the events thread hears it
/// ([`Woke::HostShort`]): idle tiers are asked through the host's ledger page, and an idle
/// process ends only when that is not enough. The next event looks again; another short is
/// signalled only once the room has moved. No timer, and no budget of TensorD's own.
pub struct Host {
    short: Arc<Signal>,
    /// The bytes the last short look was past the number.
    past: Arc<std::sync::atomic::AtomicU64>,
    nudge: Nudge,
}

impl Host {
    /// Watches while the process lives; None (a memory-log note says why) where the platform
    /// gives no pressure events.
    pub fn watch(mode: HostMode, log: &MemoryLog) -> Option<Arc<Host>> {
        let armed = host_pressure::Pressure::arm(mode).and_then(|pressure| Ok((pressure, Arc::new(Signal::new()?))));
        let (mut pressure, short) = armed
            .inspect_err(|error| log.note(serde_json::json!({"event": "host_unwatched", "mode": format!("{mode:?}"), "why": error.to_string()})))
            .ok()?;
        let nudge = pressure.nudge();
        let past = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (signal, shortfall, notes) = (short.clone(), past.clone(), log.clone());
        let watching = std::thread::Builder::new().name("host-pressure".into()).spawn(move || {
            // The room when short was last signalled: the next signal needs it to have moved.
            let mut signalled: Option<i64> = None;
            // The contained machine's limit the last note named.
            let mut limit: Option<u64> = None;
            while pressure.wait().is_ok() {
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
                let room = varena_ledger::host::budget().room();
                if room >= 0 {
                    signalled = None;
                } else if signalled != Some(room) {
                    shortfall.store(room.unsigned_abs(), std::sync::atomic::Ordering::Release);
                    signal.set();
                    signalled = Some(room);
                }
            }
        });
        if let Err(error) = watching {
            log.note(serde_json::json!({"event": "host_unwatched", "why": error.to_string()}));
            return None;
        }
        Some(Arc::new(Host { short, past, nudge }))
    }

    /// Something happened (a call began, an executor's frame came): the watcher looks.
    pub fn nudge(&self) {
        self.nudge.send();
    }

    /// Whether a look found the host short since this was last asked (the events thread asks).
    pub fn short(&self) -> bool {
        self.short.clear()
    }

    /// The bytes the last short look was past varena's number.
    pub fn past(&self) -> u64 {
        self.past.load(std::sync::atomic::Ordering::Acquire)
    }
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Where one machine's memory events go: `memory.log` in its store's `logs`, beside TensorFS's
/// transport log, read by `cozy machine logs --memory`. None logs to tracing.
#[derive(Clone, Debug, Default)]
pub struct MemoryLog(Option<Arc<PathBuf>>);

impl MemoryLog {
    pub fn at(dir: PathBuf) -> Self {
        Self(Some(Arc::new(dir)))
    }

    pub(crate) fn note(&self, event: serde_json::Value) {
        let Some(dir) = self.0.as_deref() else {
            return tracing::info!("memory: {event}");
        };
        let path = dir.join("memory.log");
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > ROTATE_BYTES) {
            let _ = fs::rename(&path, dir.join("memory.log.1"));
        }
        let line = format!("{:.3} {event}\n", now_ms() as f64 / 1e3);
        let written = fs::create_dir_all(dir)
            .and_then(|_| OpenOptions::new().create(true).append(true).open(&path))
            .and_then(|mut file| file.write_all(line.as_bytes()));
        if let Err(error) = written {
            tracing::warn!("memory: {event} ({}: {error})", path.display());
        }
    }
}
