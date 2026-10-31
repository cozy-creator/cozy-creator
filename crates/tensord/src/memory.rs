//! Executor-owned GPU memory: TensorD records measurements and offloads whole idle executors.
//! It creates no CUDA context, GPU allocation, lease listener or page-reclamation thread.
//! GPU permits schedule calls; measured peaks are placement hints, never admission gates.
//! Host RAM uses the existing pressure watcher and per-process growth accounting.
use crate::machine::receipt;
use platform::contain::host_memory;
use platform::contain::host_pressure::{self, Growth, HostMode, Nudge};
use platform::memory::{wait_any, Signal, Waitable};
use platform::process::Exact;
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, OpenOptions}, io::{self, Write}, path::PathBuf,
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

/// Driver-attributed owners, not the unassigned difference between total and free bytes.
pub struct DeviceUse {
    pub total: u64,
    pub free: u64,
    pub own: u64,
    pub foreign: u64,
}

impl Gpu {
    pub fn create(entry: &str, inventory: &[receipt::Gpu], limit: Option<u64>, peaks: &Peaks, log: &MemoryLog) -> Option<Self> {
        let row = row(entry, inventory)?;
        let figures = Figures::new().map_err(|error| log.note(serde_json::json!({"event": "gpu_figures_unavailable", "gpu": entry, "why": error.to_string()}))).ok()?;
        let absent = limit.map_or(0, |limit| row.memory_bytes.saturating_sub(limit));
        let gpu = Self { figures, model: row.device_name.clone(), uuid: row.device_uuid.clone(), absent };
        peaks.restore(&gpu.model, &gpu.figures);
        log.note(serde_json::json!({"event": "memory_mode", "gpu": gpu.uuid, "mode": "executor_owned", "tensorD_gpu_bytes": 0}));
        Some(gpu)
    }

    /// A current driver reading, never a reservation or permission to allocate; under a
    /// development `gpu_memory_limit`, a card of that size's.
    pub fn free(&self) -> Option<u64> {
        platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.free()).ok().map(|free| free.saturating_sub(self.absent))
    }

    /// The bytes the driver charges process `pid` on this GPU now; None where it cannot say (a
    /// container whose pids are not the driver's, WDDM) or the process holds nothing here.
    pub fn process_bytes(&self, pid: u32) -> Option<u64> {
        platform::gpu::Device::open(&self.uuid).ok()?.read().ok()?.processes?.get(&pid).copied()
    }

    /// The driver's pids with memory on this GPU now.
    pub fn pids(&self) -> Option<BTreeSet<u32>> {
        let read = platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.read()).ok()?;
        Some(read.processes?.into_keys().collect())
    }

    pub fn usage(&self, tenants: &Tenants) -> Option<DeviceUse> {
        let read = platform::gpu::Device::open(&self.uuid).and_then(|gpu| gpu.read()).ok()?;
        if read.total == 0 { return None }
        let processes = read.processes?;
        let book = tenants.lock().unwrap_or_else(|e| e.into_inner());
        let (mut own, mut foreign) = (0u64, 0u64);
        if book.is_empty() || book.keys().any(|pid| processes.contains_key(pid)) {
            for (pid, bytes) in processes {
                // A package's spawned CUDA worker or a kernel build is also ours. Follow current
                // parentage rather than calling every unregistered PID a foreign application.
                // An unreadable/disappearing process makes this sample unknown.
                let (mut ancestor, mut ours) = (pid, false);
                let mut seen = BTreeSet::new();
                while ancestor > 0 && seen.insert(ancestor) {
                    if book.contains_key(&ancestor) || ancestor == std::process::id() { ours = true; break }
                    ancestor = platform::process::parent_of(ancestor).ok()?;
                }
                if ours { own = own.saturating_add(bytes) }
                else { foreign = foreign.saturating_add(bytes) }
            }
        } else {
            // A container: the driver names host pids, ours by what each executor's start
            // brought up on this GPU (`Registry::started`).
            drop(book);
            let drivers = tenants.drivers(&self.uuid, &processes);
            for (pid, bytes) in processes {
                if drivers.contains(&pid) { own = own.saturating_add(bytes) }
                else { foreign = foreign.saturating_add(bytes) }
            }
        }
        Some(DeviceUse { total: read.total, free: read.free, own, foreign })
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
            Some(protocol::machine::ProcessMemory { pid, executor: format!("{}#{}", tenant.plan, tenant.start), bytes, chunks: 0 })
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
/// Executors by their pid here, and, where the driver names other pids (a container), which
/// driver pids are theirs: a driver pid is ours when it first appears on a GPU during one of our
/// executors' starts there, and only then; an ambiguous one (more new pids than the start
/// brought up processes) is foreign.
#[derive(Default)]
pub struct Registry {
    tenants: Mutex<BTreeMap<u32, Tenant>>,
    drivers: Mutex<Drivers>,
}
#[derive(Default)]
struct Drivers {
    /// Per GPU: our driver pids, each with the executor pids whose start brought it up.
    ours: BTreeMap<String, BTreeMap<u32, Vec<u32>>>,
    /// Per GPU and start: the driver pids present before it began.
    starting: BTreeMap<u64, (String, BTreeSet<u32>)>,
    next: u64,
}
/// One executor start in flight on its GPUs (`Registry::starting`); dropped unsettled, it claims
/// nothing.
pub struct Starting(Arc<Registry>, Vec<u64>);
impl Drop for Starting {
    fn drop(&mut self) {
        let mut drivers = self.0.drivers.lock().unwrap_or_else(|e| e.into_inner());
        for id in self.1.drain(..) {
            drivers.starting.remove(&id);
        }
    }
}
impl Registry {
    pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, BTreeMap<u32, Tenant>>> { self.tenants.lock() }
    pub fn register(&self, tenants: impl IntoIterator<Item = (u32, Tenant)>) {
        self.tenants.lock().unwrap_or_else(|e| e.into_inner()).extend(tenants);
    }

    /// An executor start begins on these GPUs (uuid, the driver's pids there now).
    pub fn starting(self: &Arc<Self>, gpus: Vec<(String, BTreeSet<u32>)>) -> Starting {
        let mut drivers = self.drivers.lock().unwrap_or_else(|e| e.into_inner());
        let ids = gpus.into_iter().map(|gpu| {
            drivers.next += 1;
            let id = drivers.next;
            drivers.starting.insert(id, gpu);
            id
        }).collect();
        Starting(self.clone(), ids)
    }

    /// The start ended (`pids`: its executor and followers, empty if it failed). On each GPU the
    /// driver pids that appeared since it began (`now`) and nothing else claims are theirs when
    /// they are no more than its processes; else they stay foreign. Returns, per GPU, the pids
    /// taken as ours and those judged foreign.
    pub fn started(&self, mut starting: Starting, pids: &[u32], now: impl Fn(&str) -> Option<BTreeSet<u32>>)
        -> Vec<(String, Vec<u32>, Vec<u32>)> {
        let mut drivers = self.drivers.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = vec![];
        for id in std::mem::take(&mut starting.1) {
            let Some((gpu, before)) = drivers.starting.remove(&id) else { continue };
            let Some(after) = now(&gpu) else { continue };
            let claimed: BTreeSet<u32> = drivers.ours.get(&gpu).map(|ours| ours.keys().copied().collect()).unwrap_or_default();
            let new: Vec<u32> = after.iter().filter(|pid| !before.contains(pid) && !claimed.contains(pid)).copied().collect();
            if pids.is_empty() || new.is_empty() { continue }
            if new.len() <= pids.len() {
                let ours = drivers.ours.entry(gpu.clone()).or_default();
                for pid in &new { ours.insert(*pid, pids.to_vec()); }
                out.push((gpu, new, vec![]));
            } else {
                out.push((gpu, vec![], new));
            }
        }
        out
    }

    /// Our driver pids on GPU `uuid` among `processes`: learned ones whose executors still live,
    /// and, while a start is in flight there, any pid that appeared since it began (it is
    /// claimed or judged foreign when the start ends).
    fn drivers(&self, uuid: &str, processes: &BTreeMap<u32, u64>) -> BTreeSet<u32> {
        let live: BTreeSet<u32> = self.tenants.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().collect();
        let mut drivers = self.drivers.lock().unwrap_or_else(|e| e.into_inner());
        let mut ours = BTreeSet::new();
        if let Some(learned) = drivers.ours.get_mut(uuid) {
            learned.retain(|pid, starts| processes.contains_key(pid) && starts.iter().any(|start| live.contains(start)));
            ours.extend(learned.keys().copied());
        }
        for (gpu, before) in drivers.starting.values() {
            if gpu == uuid {
                ours.extend(processes.keys().filter(|pid| !before.contains(pid)));
            }
        }
        ours
    }
}

#[derive(Default)]
struct Book {
    values: BTreeMap<String, u64>,
    dispatched: BTreeMap<String, String>,
}

/// Request-to-key association and observed high-water values, with no allocator policy.
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
    pub fn record_request(&self, request: &str, peak: Option<u64>, compute: Option<u64>, host: Option<u64>) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let Some(key) = book.dispatched.get(request).cloned() else { return };
        if let Some(peak) = peak { larger(&mut book.values, format!("peak/{key}"), peak); }
        if let Some(compute) = compute { larger(&mut book.values, format!("compute/{key}"), compute); }
        if let Some(host) = host { larger(&mut book.values, format!("host/{key}"), host); }
        self.changed.set();
    }
    pub fn host_peak(&self, package: &str, shape: &str) -> Option<u64> { self.figure("host", &format!("{package}|{shape}")) }
    pub fn request_peak(&self, package: &str, shape: &str) -> Option<u64> { self.figure("peak", &format!("{package}|{shape}")) }
    fn figure(&self, kind: &str, key: &str) -> Option<u64> {
        let book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let full = format!("{kind}/{key}");
        if key.ends_with('|') {
            book.values.range(full.clone()..).take_while(|(k, _)| k.starts_with(&full)).map(|(_, v)| *v).max()
        } else { book.values.get(&full).copied() }
    }
    fn restore(&self, peaks: &[Peak]) {
        let mut book = self.book.lock().unwrap_or_else(|e| e.into_inner());
        for peak in peaks { let (key, bytes) = peak.figure(); larger(&mut book.values, key, bytes); }
    }
    fn peaks(&self) -> Vec<Peak> { self.book.lock().unwrap_or_else(|e| e.into_inner()).values.iter().filter_map(Peak::of).collect() }
}
fn larger(figures: &mut BTreeMap<String, u64>, key: String, bytes: u64) {
    let value = figures.entry(key).or_insert(bytes); *value = (*value).max(bytes);
}

/// A measured peak, as TensorD persists it per GPU model: the largest of each kind and key.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Peak {
    /// A request's largest working set at `<package>|<shape>`.
    Request { key: String, bytes: u64 },
    /// A request's largest allocations beside its weights at `<package>|<shape>`: what placement
    /// keeps free on GPU 0.
    Compute { key: String, bytes: u64 },
    /// A set's resident bytes, by set name.
    Set { key: String, bytes: u64 },
    /// A request's anonymous host high-water at `<package>|<shape>`.
    Host { key: String, bytes: u64 },
}

impl Peak {
    /// Its figure: `<kind>/<key>` and its bytes.
    fn figure(&self) -> (String, u64) {
        match self {
            Peak::Request { key, bytes } => (format!("peak/{key}"), *bytes),
            Peak::Compute { key, bytes } => (format!("compute/{key}"), *bytes),
            Peak::Set { key, bytes } => (format!("set/{key}"), *bytes),
            Peak::Host { key, bytes } => (format!("host/{key}"), *bytes),
        }
    }

    fn of((figure, &bytes): (&String, &u64)) -> Option<Peak> {
        let (kind, key) = figure.split_once('/')?;
        let key = key.to_string();
        Some(match kind {
            "peak" => Peak::Request { key, bytes },
            "compute" => Peak::Compute { key, bytes },
            "set" => Peak::Set { key, bytes },
            "host" => Peak::Host { key, bytes },
            _ => return None,
        })
    }

    /// The largest of each kind and key, in key order: what every GPU of one model persists.
    pub fn merge(peaks: impl IntoIterator<Item = Peak>) -> Vec<Peak> {
        let mut figures = BTreeMap::new();
        for peak in peaks {
            let (figure, bytes) = peak.figure();
            larger(&mut figures, figure, bytes);
        }
        figures.iter().filter_map(Peak::of).collect()
    }
}

/// Place component sets from available bytes and observed set sizes; this is a hint, not admission.
pub fn homes(gpus: &[&Gpu], package: &str, shape: &str, sets: &[&str]) -> Vec<u32> {
    let mut room: Vec<u64> = gpus.iter().enumerate().map(|(rank, gpu)| {
        gpu.free().unwrap_or(0).saturating_sub(if rank == 0 { gpu.figures.figure("compute", &format!("{package}|{shape}")).unwrap_or(0) } else { 0 })
    }).collect();
    let sizes: Vec<_> = sets.iter().map(|name| gpus.iter().filter_map(|gpu| gpu.figures.figure("set", name)).max()).collect();
    let mut order: Vec<_> = (0..sets.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sizes[i]));
    let mut homes = vec![0; sets.len()];
    for index in order {
        let size = sizes[index];
        let target = if size.is_some_and(|size| room.first().is_some_and(|free| *free >= size)) { 0 }
            else { room.iter().enumerate().max_by_key(|(rank, free)| (**free, std::cmp::Reverse(*rank))).map_or(0, |(rank, _)| rank) };
        homes[index] = target as u32;
        if let (Some(free), Some(size)) = (room.get_mut(target), size) { *free = free.saturating_sub(size); }
    }
    homes
}

/// Measured peaks, persisted per GPU model as `<dir>/<model>.json`: the
/// largest of each kind and key across every GPU of the model. A key names its package release
/// (`<construction>|<shape>`, or a set's `<package>@<release>/<component>|…`), so one file holds
/// every release's. Each restore and each saved change is a memory-log note.
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

/// One process on a call's GPU. A single process may own several GPUs; a group has one per rank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rank {
    pub pid: u32,
    pub gpu: String,
}

/// One call running on the machine's GPUs: every participant and its `<package>|<shape>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub ranks: Vec<Rank>,
    pub package: String,
    pub shape: String,
}

/// TensorD's calls running until they end, including each follower the host rule must watch.
pub type Running = Arc<Mutex<BTreeMap<String, Call>>>;

/// A process running calls: measured anonymous peak, anonymous bytes now, and call count.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HostCall {
    pub pid: u32,
    pub peak: Option<u64>,
    pub anon: u64,
    pub running: u64,
}

/// Every process of every call (each rank of a group), with the host peak its own GPU measured.
/// One process on several GPUs is one row at the largest of their figures; the host rule
/// deduplicates a process shared by calls. File-backed model pages remain in the OS budget.
pub fn host_calls(running: &Running, gpus: &[Arc<Gpu>]) -> Vec<HostCall> {
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

/// The host rule (#1851) on the platform's watcher: each pressure event, or a nudge (TensorD's
/// own events, where the kernel gives none), is a look. What the calls running on these GPUs
/// must keep free, each process's measured anonymous peak less its anonymous bytes now plus
/// the growth seen past it ([`Growth`]), against the live budget, page cache counted as room.
/// Short, the events thread hears it ([`Woke::HostShort`]): the pool ends one idle process, and
/// the next event looks again. Another is ended only once the last end measurably raised the
/// budget: when ending cannot help (a growing call alone exceeds the host), the call meets its
/// own limit and the idle processes stay. A look that is not short starts over. No timer.
pub struct Host {
    short: Arc<Signal>,
    nudge: Nudge,
    /// Looks begun and made so far, and a signal for each made.
    looks: Arc<(Mutex<(u64, u64)>, std::sync::Condvar)>,
}

impl Host {
    /// Watches while the process lives; None (a memory-log note says why) where the platform
    /// gives no pressure events.
    pub fn watch(mode: HostMode, gpus: Vec<Arc<Gpu>>, running: Running, log: &MemoryLog) -> Option<Arc<Host>> {
        let armed = host_pressure::Pressure::arm(mode).and_then(|pressure| Ok((pressure, Arc::new(Signal::new()?))));
        let (mut pressure, short) = armed
            .inspect_err(|error| log.note(serde_json::json!({"event": "host_unwatched", "mode": format!("{mode:?}"), "why": error.to_string()})))
            .ok()?;
        let (nudge, looks) = (pressure.nudge(), Arc::new((Mutex::new((0, 0)), std::sync::Condvar::new())));
        let (looked, signal, notes) = (looks.clone(), short.clone(), log.clone());
        let watching = std::thread::Builder::new().name("host-pressure".into()).spawn(move || {
            let mut keep = Keep::default();
            // The budget when short was last signalled: the next signal needs more than it.
            let mut signalled: Option<u64> = None;
            // The processes and peaks the last note named: a note when they change.
            let mut noted: Vec<(u32, Option<u64>)> = vec![];
            while pressure.wait().is_ok() {
                looked.0.lock().unwrap_or_else(|e| e.into_inner()).0 += 1;
                let calls = host_calls(&running, &gpus);
                let named: Vec<(u32, Option<u64>)> = calls.iter().map(|call| (call.pid, call.peak)).collect();
                if named != noted {
                    notes.note(serde_json::json!({"event": "host_calls", "calls": calls}));
                    noted = named;
                }
                let need = keep.look(calls);
                let budget = u64::try_from(host_memory::read().available).unwrap_or(u64::MAX);
                if budget >= need {
                    signalled = None;
                } else if signalled.is_none_or(|was| budget > was) {
                    signal.set();
                    signalled = Some(budget);
                }
                looked.0.lock().unwrap_or_else(|e| e.into_inner()).1 += 1;
                looked.1.notify_all();
            }
        });
        if let Err(error) = watching {
            log.note(serde_json::json!({"event": "host_unwatched", "why": error.to_string()}));
            return None;
        }
        Some(Arc::new(Host { short, nudge, looks }))
    }

    /// Something happened (a call began, an executor's frame came): the watcher looks.
    pub fn nudge(&self) {
        self.nudge.send();
    }

    /// Nudges the watcher and blocks until a look begun since has been made.
    pub fn look(&self) {
        let begun = self.looks.0.lock().unwrap_or_else(|e| e.into_inner()).0;
        self.nudge();
        self.await_look(begun);
    }

    /// Looks made so far.
    pub fn looks(&self) -> u64 {
        self.looks.0.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    /// Blocks until more than `made` looks have been made.
    pub fn await_look(&self, made: u64) {
        let (count, signal) = &*self.looks;
        let mut looks = count.lock().unwrap_or_else(|e| e.into_inner());
        while looks.1 <= made {
            looks = signal.wait(looks).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Whether a look found the host short since this was last asked (the events thread asks).
    pub fn short(&self) -> bool {
        self.short.clear()
    }
}

/// What the host keeps free for the calls running on it: each process's anonymous growth still
/// to come (its requests' measured anonymous peak less its anonymous bytes now) and [`Growth`],
/// the most a look saw them grow past what was measured to come (an unmeasured call's every
/// byte). Page cache is never need: the budget holds it.
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
        need.saturating_add(self.growth.observe(unforeseen))
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
