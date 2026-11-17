//! The machine's sole dispatch policy; the engine only journals/supervises. Every run takes
//! one path: its plan (journaled when it binds models, else weightless) on the pool.
use platform::process::Birth;
use crate::{
    catalog::Catalog,
    execution::Engine,
    gpu_service::{Plan, Pool},
    journal::Execution,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::File,
    io,
    path::Path,
    sync::{Arc, Mutex},
};

/// What one public submission calls on its generation.
#[derive(Default)]
pub(crate) struct Call {
    pub entrypoint: String,
    pub input: Value,
    pub attention_kernel: String,
    pub attention_capture: String,
    pub gpus: u32,
    pub lane: String,
    pub inputs: Vec<crate::journal::InputFile>,
    /// GPUs its plan runs on: 0 deviceless.
    pub degree: u32,
    /// A root's widths here (`Invocation::widths`).
    pub widths: Vec<u32>,
    /// A child run's parent execution id.
    pub parent: String,
    /// The callee App a child run calls in its parent's environment; empty for the root's.
    pub application: String,
}

pub(crate) struct Service {
    pub engine: Arc<Engine>,
    pub catalog: Catalog,
    parallelism: usize,
    stopped: Mutex<bool>,
    retained: Mutex<HashMap<String, Arc<File>>>,
    pool: Mutex<Option<Arc<Pool>>>,
    /// The child-call broker and weights grants.
    calls: Mutex<Option<Arc<crate::calls::Calls>>>,
    /// The TensorFS store's caches (`Publisher::reclaim`), swept with the machine's.
    publisher: Mutex<Option<Arc<crate::published::Publisher>>>,
    startup_gpu_births: Mutex<Vec<Birth>>,
    /// This machine's birth: a fenced leader's group members born before it are its
    /// previous incarnation's followers.
    started_ticks: u64,
    queue: Mutex<crate::queue::Queue>,
    /// While it holds (an update activating), no new root run starts: queued runs are journaled
    /// and start after the restart.
    held: std::sync::OnceLock<Box<dyn Fn() -> bool + Send + Sync>>,
    /// Each run left waiting by a dispatch pass: since when, and what for, as this machine's log said it (cl-1086).
    waits: Mutex<HashMap<String, (std::time::Instant, String)>>,
}

const SWEEP_EVERY: std::time::Duration = crate::reclaim::IDLE;

/// Whether an executor this machine run does not supervise still fences GPU dispatch: it, or a
/// member of its group born before `before`, is alive and not stuck in the kernel after its kill.
/// One stuck (D state) is shown in the machine's status and fences nothing: what it holds stays
/// held, its GPU memory included, as NVML reads it.
fn holds_gpus(birth: &Birth, before: u64) -> bool {
    let left = left(birth, before);
    !left.is_empty() && !platform::contain::all_stuck(&left)
}
/// `birth`'s group from before `before`, as far as it has not exited.
fn left(birth: &Birth, before: u64) -> Vec<Birth> {
    let mut left: Vec<Birth> = platform::process::group_members(birth)
        .into_iter()
        .filter(|m| m.start_ticks < before && m.state().is_ok_and(|s| s != platform::process::State::Gone))
        .collect();
    if !birth.ended().unwrap_or(false) {
        left.push(birth.clone());
    }
    left
}
impl Service {
    pub(crate) fn open(root: &Path, generations: &Path, parallelism: usize, store: Arc<crate::storage::Store>) -> io::Result<Arc<Self>> {
        if parallelism == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "CPU parallelism must be positive",
            ));
        }
        let engine = Engine::open(&root.join("execution"), store)?;
        // A project whose folder went while this machine wasn't running leaves nothing here (cl-1031).
        engine.forget_gone_projects();
        // A completed request does not prove its retained CUDA context ended. A prior
        // machine's executor cannot be adopted (nothing can reach it), so it is ended now
        // and GPU admission stays fenced until its exit is observed.
        let mut cursor = 0;
        let mut seen = HashSet::new();
        let mut startup_gpu_births = vec![];
        let started_ticks = Birth::own()?.start_ticks;
        loop {
            let page = engine.gpu_births_after(cursor, 256)?;
            if page.is_empty() {
                break;
            }
            for (id, birth) in page {
                cursor = id;
                let alive = !birth.ended().unwrap_or(false)
                    || crate::process::group_outlives(&birth, started_ticks);
                if alive && seen.insert((birth.pid, birth.boot_id.clone(), birth.start_ticks)) {
                    // A follower left behind by a leader that already ended is ended too.
                    for member in platform::process::group_members(&birth) {
                        if member.start_ticks < started_ticks {
                            if let Ok(Some(exact)) = platform::process::Exact::open(&member) {
                                let _ = exact.kill();
                            }
                        }
                    }
                    if let Err(error) = engine.end_orphan(birth.clone()) {
                        tracing::warn!("GPU startup birth remains fenced: {error}");
                    }
                    startup_gpu_births.push(birth);
                }
            }
        }
        // Descendants of those executors (a setsid daemon, a compile worker) live on in the
        // executors' own scopes; every such scope of this machine is ended now.
        match platform::contain::Scope::sweep(&platform::contain::namespace(root)) {
            Ok(0) => (),
            Ok(held) => tracing::info!(target: crate::config::MACHINE, "ended {held} process(es) left in earlier executor scopes"),
            Err(error) if error.is_unsupported() => (),
            Err(error) => return Err(io::Error::other(error)),
        }
        let service = Arc::new(Self {
            engine,
            catalog: Catalog::new(generations)?,
            parallelism,
            stopped: Mutex::new(false),
            retained: Mutex::new(HashMap::new()),
            pool: Mutex::new(None),
            calls: Mutex::new(None),
            publisher: Mutex::new(None),
            startup_gpu_births: Mutex::new(startup_gpu_births),
            started_ticks,
            queue: Default::default(),
            held: Default::default(),
            waits: Default::default(),
        });
        service.engine.reconcile()?;
        // Its first sweep is the reclaim thread's (`reclaim_loop`): a long one never delays serving.
        // Keep queued runs' generations alive, including a prior boot's.
        for record in service.engine.nonterminal(usize::MAX)? {
            if let Ok(held) = service.catalog.resolve(&record.invocation.generation) {
                service
                    .retained
                    .lock()
                    .unwrap()
                    .insert(record.id.clone(), held.retention());
            }
            if let Some(pending) = record.pending_completion {
                for birth in pending.processes {
                    service.engine.end_orphan(birth)?;
                }
            } else if let Some(birth) = record.process {
                service.engine.end_orphan(birth)?;
            }
        }
        let owner = service.clone();
        std::thread::Builder::new()
            .name("machine-dispatch".into())
            .spawn(move || owner.dispatch_loop())?;
        let weak = Arc::downgrade(&service);
        std::thread::Builder::new().name("machine-reclaim".into()).spawn(move || Self::reclaim_loop(weak))?;
        Ok(service)
    }
    /// The executor pool every run runs on, once configured.
    pub(crate) fn pool(&self) -> Option<Arc<Pool>> {
        self.pool.lock().unwrap().clone()
    }
    /// The pool when this machine has GPUs.
    pub(crate) fn gpu(&self) -> Option<Arc<Pool>> {
        self.pool().filter(|pool| pool.width() > 0)
    }
    pub(crate) fn calls(&self) -> Option<Arc<crate::calls::Calls>> {
        self.calls.lock().unwrap().clone()
    }
    pub(crate) fn publisher(&self) -> Option<Arc<crate::published::Publisher>> {
        self.publisher.lock().unwrap().clone()
    }
    pub(crate) fn configure_publisher(&self, publisher: Arc<crate::published::Publisher>) {
        *self.publisher.lock().unwrap() = Some(publisher);
        // The first sweep, now its packages are configured, so the disk is measured early.
        crate::reclaim::ask();
    }
    pub(crate) fn configure_calls(&self, calls: Arc<crate::calls::Calls>) {
        *self.calls.lock().unwrap() = Some(calls);
        self.engine.notify_activity();
    }
    /// Observation only. Unknown births stay reserved; CPU dispatch stays available.
    pub(crate) fn gpu_startup_fences(&self) -> usize {
        let mut births = self.startup_gpu_births.lock().unwrap();
        // A group's followers die with their leader (one group kill); each exit is observed.
        births.retain(|birth| holds_gpus(birth, self.started_ticks));
        births.len()
    }
    pub(crate) fn configure_pool(&self, pool: Arc<Pool>) -> io::Result<()> {
        let mut current = self.pool.lock().unwrap();
        if current.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the executor pool is already configured",
            ));
        }
        *current = Some(pool.clone());
        drop(current);
        pool.wake_on_freed(&self.engine);
        pool.watch_disks();
        if pool.width() > 0 {
            pool.watch_faults();
            pool.watch_stuck();
            // Before this run's first GPU process (its kernel boot): what is in use now isn't its own.
            pool.foreign_at_start();
            // The current image may prepare ahead. Historical environments stay cold;
            // requested installs and executions use their existing kernel preparation paths.
            pool.image_kernel_boot();
        }
        self.changed_environment()
    }
    /// Brings restored warm sets up (`Runs::restore_warm_sets`) once a previous run's executors
    /// still exiting are gone: they fence it, as they fence requests.
    pub(crate) fn rewarm(&self, gpu: &Arc<Pool>) -> io::Result<()> {
        let (fence, started) = (self.startup_gpu_births.lock().unwrap().clone(), self.started_ticks);
        let (gpu, engine) = (gpu.clone(), self.engine.clone());
        std::thread::Builder::new().name("warm-set-restore".into()).spawn(move || {
            // An exit wakes it; each second it looks whether those left are stuck in the kernel, which `holds_gpus` gives up.
            while let Some(birth) = fence.iter().find(|birth| holds_gpus(birth, started)) {
                let exiting = left(birth, started).iter().find_map(|member| platform::process::Exact::open(member).ok().flatten());
                match exiting {
                    Some(exact) => drop(exact.wait_for(std::time::Duration::from_secs(1))),
                    None => std::thread::sleep(std::time::Duration::from_secs(1)),
                }
            }
            if !fence.is_empty() {
                gpu.ledgers_exited();
            }
            gpu.keep_all(&engine);
        })?;
        Ok(())
    }

    /// A run prepared inside itself (`runs`) names its generation, call and model plan, and how
    /// the machine memo met it; one the memo answered (`answer`) completes with that result.
    pub(crate) fn bind_prepared(
        &self,
        id: &str,
        generation: &str,
        call: Call,
        preparation: &str,
        memo: crate::journal::CallMemo,
        answer: Option<crate::journal::ResultRecord>,
    ) -> io::Result<Execution> {
        let held = self.catalog.resolve(generation)?;
        let mut invocation = held.invocation_of(&call.application, &call.entrypoint, call.input)?;
        invocation.attention_kernel = call.attention_kernel;
        invocation.attention_capture = call.attention_capture;
        invocation.gpus = call.gpus;
        invocation.lane = call.lane;
        invocation.inputs = call.inputs;
        invocation.degree = call.degree;
        invocation.widths = call.widths;
        invocation.parent = call.parent;
        let record = self.engine.bind_prepared(id, invocation, preparation, memo, answer)?;
        self.retain(&record, held.retention());
        Ok(record)
    }
    fn retain(&self, record: &Execution, hold: Arc<File>) {
        if !record.state.terminal() {
            self.retained
                .lock()
                .unwrap()
                .insert(record.id.clone(), hold);
        }
    }
    /// An install or GPU configuration changed what can dispatch.
    pub(crate) fn changed_environment(&self) -> io::Result<()> {
        self.engine.notify_activity();
        Ok(())
    }
    /// A package installed, release or local: its environment's bytecode and kernels compile and
    /// its import-only parent imports now, while its weights come, so its first call forks
    /// (`Pool::installed`).
    pub(crate) fn installed(&self, held: &crate::catalog::HeldGeneration) {
        if !crate::private_io::is_private_path(&held.record.python) {
            crate::published::environment::compile_bytecode(&held.record.python);
        }
        if let Some(gpu) = self.gpu() {
            gpu.installed(held.clone());
        }
        self.engine.notify_activity();
    }
    /// The machine started: a dedicated one's installed environments' import-only parents import,
    /// the most recently used first, while the host has room for them, and the rest's import
    /// pages are read ahead; a shared one reads ahead its few most recently used
    /// (`Pool::import_recent`).
    pub(crate) fn import_installed(&self) -> io::Result<()> {
        let Some(gpu) = self.gpu() else { return Ok(()) };
        let bound = self.engine.with_journal(|journal| journal.bound_generations())?;
        gpu.import_recent(self.catalog.recent(bound));
        Ok(())
    }
    /// No job: a run waiting for its sender's access is kept in the journal, not held up here.
    pub(crate) fn idle(&self) -> io::Result<bool> {
        Ok(!self.engine.jobs(i64::MAX)?.0)
    }
    pub(crate) fn hold_while(&self, held: impl Fn() -> bool + Send + Sync + 'static) {
        let _ = self.held.set(Box::new(held));
    }
    /// This computer's machine idle a while: every idle executor outside the warm sets ends, and
    /// what it held goes back.
    pub(crate) fn release_idle(&self) {
        if let Some(pool) = self.pool() {
            pool.release_idle();
        }
    }
    /// This computer's machine idle its whole window yet kept up: a shared one's idle import-only
    /// parents end too.
    pub(crate) fn release_parents(&self) {
        if let Some(pool) = self.pool() {
            pool.release_parents();
        }
    }
    /// A warm set member is kept in memory here: this computer's machine stays up for it.
    pub(crate) fn keeps_warm(&self) -> bool {
        self.pool().is_some_and(|pool| pool.keeps_warm())
    }
    pub(crate) fn stop(&self) -> io::Result<bool> {
        let mut stopped = self.stopped.lock().unwrap();
        if !self.idle()? {
            return Ok(false);
        }
        *stopped = true;
        if let Some(pool) = self.pool() {
            pool.stop()?;
        }
        self.engine.notify_activity();
        crate::reclaim::ask();
        Ok(true)
    }
    fn dispatch_loop(self: Arc<Self>) {
        loop {
            let epoch = self.engine.activity_epoch();
            if *self.stopped.lock().unwrap() {
                return;
            }
            if let Err(error) = self.engine.reconcile() {
                tracing::warn!("executor birth observation: {error}");
            }
            self.retained.lock().unwrap().retain(|id, _| {
                self.engine
                    .get(id)
                    .map(|r| !r.state.terminal())
                    .unwrap_or(true)
            });
            if let Err(error) = self.dispatch_ready() {
                tracing::warn!("dispatch observation: {error}");
            }
            // Dispatch wakes on events only: an arrival, an exit, a cancel, freed room.
            self.engine.wait_activity(epoch, None);
        }
    }
    /// Sweep disk caches each `SWEEP_EVERY` (what each keeps goes once unused that long), the first once its packages are
    /// configured; and measure what fills the disk after a sweep or when a reader saw it move (`reclaim::seen_free`).
    /// Native Varena owns GPU pressure service.
    fn reclaim_loop(weak: std::sync::Weak<Self>) {
        platform::process::background_thread(); // nice 19, idle I/O: a big cleanup never crowds out work
        let mut swept: Option<std::time::Instant> = None;
        loop {
            let asked = crate::reclaim::wait_asked(swept.map(|at| SWEEP_EVERY.saturating_sub(at.elapsed())));
            match weak.upgrade() {
                Some(service) if !*service.stopped.lock().unwrap() => {
                    let due = swept.map_or_else(|| service.publisher.lock().unwrap().is_some(), |at| at.elapsed() >= SWEEP_EVERY);
                    if due {
                        service.reclaim();
                        swept = Some(std::time::Instant::now());
                    }
                    if due || asked {
                        service.measure_usage();
                    }
                }
                _ => return,
            }
        }
    }
    fn measure_usage(&self) {
        let publisher = self.publisher.lock().unwrap().clone();
        let generations = self.catalog.root();
        if let Some(publisher) = publisher {
            crate::reclaim::measure_usage(publisher.store().root(), generations.parent().unwrap_or(generations), Some(&publisher.uv_cache()));
        }
    }
    /// `actor`'s installations `named` leave the journal, their warm set members and the GPU
    /// pool. Each environment no installation or unfinished run names any more is deleted,
    /// its idle executors ended first; one a process still holds is `retained` for reclaim.
    pub(crate) fn uninstall(
        &self,
        actor: &str,
        named: impl Fn(&crate::journal::Installation) -> bool,
    ) -> io::Result<(Vec<crate::journal::Installation>, u64, Vec<String>)> {
        let mut removed = vec![];
        for installed in self.engine.installations(actor)?.into_iter().filter(|i| named(i)) {
            if let Some(gone) = self.engine.with_journal(|j| j.remove_installation(actor, &installed.alias))? {
                removed.push(gone);
            }
        }
        let generations: std::collections::BTreeSet<String> = removed.iter().map(|i| i.generation.clone()).collect();
        // One the actor still installs (an installation this machine's pair moved past without
        // moving its environment) keeps its warm members.
        let kept: std::collections::BTreeSet<String> = self.engine.installations(actor)?.into_iter().map(|i| i.generation).collect();
        if let Some(gpu) = self.gpu() {
            gpu.drop_members(actor, &generations.difference(&kept).cloned().collect());
        }
        let bound = self.engine.with_journal(|j| j.bound_generations())?;
        let (mut reclaimed, mut retained) = (0u64, vec![]);
        for generation in generations.iter().filter(|g| !bound.contains(*g)) {
            if let Some(gpu) = self.gpu() {
                gpu.end_generation(generation);
            }
            match crate::reclaim::remove_generation(self.catalog.root(), generation)? {
                Some(bytes) => reclaimed += bytes,
                None => retained.push(generation.clone()),
            }
        }
        Ok((removed, reclaimed, retained))
    }

    /// Caches manage themselves (`reclaim`): a TTL and a low disk, never a purge verb. Each
    /// sweep logs what it freed.
    pub(crate) fn reclaim(&self) -> crate::reclaim::Swept {
        self.sweep(0, true)
    }
    /// The machine's garbage for a store admit of `incoming` bytes that finds the disk short,
    /// before the store evicts a model (`Store::on_short`): never kernels, memoized stages or
    /// run objects. Then the executors whose open models alone keep the room it needs let them go
    /// (`free_executor_holds`), so the store's eviction can take those models. True when any did.
    pub(crate) fn reclaim_for(&self, incoming: u64) -> bool {
        self.sweep(incoming, false);
        let Some(store) = self.store() else {
            return false;
        };
        !self.free_executor_holds(&store, incoming).is_empty()
    }

    /// Room on the disk for the `missing` bytes of run `run`'s weights, before any is wanted: the
    /// machine's garbage, then the least recently used models no queued or running job holds,
    /// those executors hold open too (an idle one ended, a busy one's idle construction let go
    /// first), never one in use. What it evicted is the
    /// run's warning. Refused, with the sizes and what would hold it, only when even all of that
    /// would not fit; a disk it cannot measure is said and not judged.
    pub(crate) fn room_for(&self, run: &str, missing: u64) -> Result<(), String> {
        let Some(store) = self.store().filter(|_| missing > 0) else {
            return Ok(());
        };
        let size = crate::reclaim::size;
        let warn = |text: String| {
            if let Err(error) = self.engine.append_warning(run, &text) {
                tracing::warn!("run {run}: {text} ({error})");
            }
        };
        let free = || crate::reclaim::measure(store.root()).map(|disk| disk.free());
        match free() {
            Ok(free) if free >= missing => return Ok(()),
            Ok(_) => {}
            Err(error) => {
                warn(format!("Cozy couldn't check whether this render's models ({}) fit on the disk: {error}", size(missing)));
                return Ok(());
            }
        }
        // A run's need comes before every guess at what another may read (cl-905), a caller's too (cl-1104).
        crate::access::drop_guesses();
        crate::ahead::make_way();
        self.sweep(missing, false);
        let room = match store.room() {
            Ok(room) => room,
            Err(error) => {
                warn(format!("Cozy couldn't check whether this render's models ({}) fit on the disk: {error}", size(missing)));
                return Ok(());
            }
        };
        let idle: BTreeSet<u32> = self.pool().map(|pool| pool.idle_executors().into_iter().flat_map(|(pids, _)| pids).collect()).unwrap_or_default();
        let busy = self.pool().map(|pool| pool.busy_models()).unwrap_or_default();
        let all = busy.iter().flat_map(|(pid, models)| models.idle.keys().map(|c| (*pid, c.clone()))).collect();
        let goes = |lane: &tensorfs::store::retention::RoomLane| lets_go(lane, &idle, &busy, &all);
        let evictable = room.freed(goes);
        // A disk below its reserve lacks that too, as the store judges each admit.
        let (now, below) = crate::reclaim::measure(store.root()).map_or((0, 0), |disk| (disk.free(), disk.reserve().saturating_sub(disk.available)));
        let short = missing + below - now.min(missing);
        if evictable < short {
            let past = match below {
                0 => format!("{} free", size(now)),
                below => format!("the disk is {} short of the space Cozy keeps free", size(below)),
            };
            // What keeps the rest, largest first: the models in use and who holds them.
            let me = std::process::id();
            let who = |pid: u32| match pid {
                0 => "a local model or a model file saved on this machine".to_owned(),
                pid if pid == me => "this machine's queued or running renders".to_owned(),
                pid => format!("process {pid}"),
            };
            let mut held: Vec<(u64, String)> = (room.lanes.iter().filter(|lane| !goes(lane)))
                .map(|lane| (room.freed(|l| std::ptr::eq(l, lane)).saturating_sub(room.spare), lane))
                .filter(|(bytes, _)| *bytes > 0)
                .map(|(bytes, lane)| (bytes, format!("{} ({}, held by {})", lane.name, size(bytes), lane.holders.iter().map(|&pid| who(pid)).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", "))))
                .collect();
            held.sort_by(|a, b| b.0.cmp(&a.0));
            let held: Vec<String> = held.into_iter().take(3).map(|(_, said)| said).collect();
            let in_use = if held.is_empty() { String::new() } else { format!(" Still in use: {}.", held.join("; ")) };
            return Err(format!(
                "machine_disk_full: This machine's disk doesn't have room for this render's models. They need {} more, and Cozy can free only {} \
                 ({past}, plus {} in models nothing queued or running needs).{in_use}{}",
                size(missing), size((now + evictable).saturating_sub(below)), size(evictable), crate::reclaim::advice(store.root(), missing)
            ));
        }
        let ended = self.free_executor_holds(&store, missing);
        match store.make_room(missing) {
            Ok(made) if !made.evicted.is_empty() => {
                let each: Vec<String> = made.evicted.iter()
                    .map(|e| format!("{} ({}, unused for {})", e.name, size(e.bytes), api::calcifer::clock(e.idle.as_millis() as i64)))
                    .collect();
                let idle = if ended.is_empty() { "" } else { ", after the processes that had them open let them go" };
                warn(format!("The disk was {} short for this render's models, so Cozy removed {}{idle}.", size(short), each.join(", ")));
            }
            Ok(_) => {}
            Err(error) => warn(format!("Cozy couldn't make room on the disk for this render's models ({}): {error}", size(missing))),
        }
        Ok(())
    }

    /// Executors let go of models, while those only they hold open would give the disk what it lacks for `incoming` more bytes
    /// and its reserve, as the store judges an admit (0: one of unknown length, or none): idle executors end, least recently
    /// used first; then a busy executor lets go of constructions none of its calls runs on (tfs-303: one that ran disk-1 and
    /// runs disk-5 kept both, and a run was refused disk-1's room), never one a queued or running job also holds. Waits, as
    /// long as `RELEASE_WAIT`, for what those let go to leave the store's holds. The models they held, by name.
    fn free_executor_holds(&self, store: &crate::storage::Store, incoming: u64) -> Vec<String> {
        let Some(pool) = self.pool() else {
            return vec![];
        };
        let idle = pool.idle_executors();
        let busy = pool.busy_models();
        // Below its reserve the disk lacks room for any write (kamitsure, 11-16: its idle LTX stayed while outputs went).
        let need = crate::reclaim::measure(store.root()).map_or(0, |disk| (disk.reserve() + incoming.max(1)).saturating_sub(disk.available));
        if (idle.is_empty() && busy.is_empty()) || need == 0 {
            return vec![];
        }
        let room = match store.room() {
            Ok(room) => room,
            Err(error) => {
                tracing::warn!("judging the room executors hold: {error}");
                return vec![];
            }
        };
        type Released = BTreeSet<(u32, String)>;
        let freed = |ended: &BTreeSet<u32>, released: &Released| room.freed(|lane| lets_go(lane, ended, &busy, released));
        let (mut ended, mut released, mut names) = (BTreeSet::new(), Released::new(), vec![]);
        let mut can = freed(&ended, &released);
        for (pids, _) in idle {
            if can >= need {
                break;
            }
            let mut with = ended.clone();
            with.extend(pids.iter().copied());
            let more = freed(&with, &released);
            // The leader's end ends its whole group.
            if more > can && pool.end_idle_executor(pids[0]) {
                names.extend(room.lanes.iter().filter(|lane| pids.iter().any(|pid| lane.holders.contains(pid))).map(|lane| lane.name.clone()));
                (ended, can) = (with, more);
            }
        }
        for (pid, models) in busy.iter().filter(|(pid, _)| !ended.contains(*pid)) {
            for construction in models.idle.keys() {
                if can >= need {
                    break;
                }
                let mut with = released.clone();
                with.insert((*pid, construction.clone()));
                let more = freed(&ended, &with);
                if more > can {
                    (released, can) = (with, more);
                }
            }
            // A model two idle constructions read goes only with both; then each that frees nothing more stays.
            if can < need {
                let mut with = released.clone();
                with.extend(models.idle.keys().map(|c| (*pid, c.clone())));
                let more = freed(&ended, &with);
                if more > can {
                    for construction in models.idle.keys() {
                        let pair = (*pid, construction.clone());
                        with.remove(&pair);
                        if freed(&ended, &with) < more {
                            with.insert(pair);
                        }
                    }
                    (released, can) = (with, more);
                }
            }
        }
        let mut waits: Vec<(u32, BTreeSet<String>)> = vec![];
        for (pid, models) in &busy {
            let which: Vec<String> = released.iter().filter(|(of, _)| of == pid).map(|(_, c)| c.clone()).collect();
            if which.is_empty() || !pool.release_constructions(*pid, &which) {
                continue;
            }
            let headers: BTreeSet<String> = which.iter().flat_map(|c| models.idle[c].iter().cloned()).collect();
            names.extend(room.lanes.iter().filter(|lane| lane.holders.contains(pid) && headers.iter().any(|h| lane.keeps(h))).map(|lane| lane.name.clone()));
            waits.push((*pid, headers));
        }
        if !names.is_empty() {
            tracing::warn!("the disk is short of {}: executors let go of {}", crate::reclaim::size(need), names.join(", "));
        } else {
            let busy: Vec<String> = busy.iter().map(|(pid, models)| format!("{pid}: {} idle", models.idle.len())).collect();
            tracing::info!("the disk is short of {}: no executor let go of a model ({} free without one; executors {})",
                crate::reclaim::size(need), crate::reclaim::size(can), busy.join(", "));
        }
        // What a busy executor let go leaves the store's holds once it closes them.
        let deadline = std::time::Instant::now() + RELEASE_WAIT;
        while !waits.is_empty() && std::time::Instant::now() < deadline {
            let Ok(room) = store.room() else { break };
            waits.retain(|(pid, headers)| room.lanes.iter().any(|lane| lane.holders.contains(pid) && headers.iter().any(|h| lane.keeps(h))));
            if !waits.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        for (pid, _) in &waits {
            tracing::warn!("process {pid} still held the models it was told to let go {} s later: they stay", RELEASE_WAIT.as_secs());
        }
        names
    }

    fn store(&self) -> Option<Arc<crate::storage::Store>> {
        self.publisher.lock().unwrap().as_ref().map(|publisher| publisher.store().clone())
    }
    /// A sweep as though `incoming` more bytes were taken; `all` caches, or the garbage alone.
    fn sweep(&self, incoming: u64, all: bool) -> crate::reclaim::Swept {
        let publisher = self.publisher.lock().unwrap().clone();
        let uv_cache = publisher.as_ref().map(|publisher| publisher.uv_cache());
        let generations = self.catalog.root();
        let engine = generations.parent().unwrap_or(generations);
        if let (true, Some(calls)) = (all, self.calls()) {
            if let Err(error) = calls.sweep_objects() {
                tracing::warn!("reclaim object roots: {error}");
            }
        }
        let sweep = || -> io::Result<crate::reclaim::Swept> {
            let pool = self.pool().filter(|_| all);
            let bound = self.engine.bound_generations()?;
            let kernels = self.gpu().filter(|_| all).map(|gpu| gpu.kernel_caches());
            let environments = [generations.to_path_buf(), engine.join("editable"), crate::machine::toolchain::dir(engine)];
            let disk = || crate::reclaim::measure(&self.engine.root).map(|d| crate::reclaim::Disk { available: d.available.saturating_sub(incoming), ..d });
            let mut swept = crate::reclaim::sweep(&crate::reclaim::Caches {
                engine: &self.engine,
                catalog: &self.catalog,
                bound: &bound,
                kernels: kernels.as_ref(),
                memo: pool.as_ref().map(|pool| pool.memo()),
                uv_cache: uv_cache.as_deref(),
                environments: &environments,
                torch: publisher.as_ref().map(|publisher| &publisher.sdk.torch),
                disk: &disk,
            })?;
            if let Some(calls) = self.calls().filter(|_| all) {
                if disk()?.short().is_some() {
                    // An idle executor's models go before any held output (kamitsure kept its idle LTX's 71.5 GB and let outputs go).
                    if let Some(store) = self.store() {
                        self.free_executor_holds(&store, 0);
                    }
                    let (count, bytes) = calls.make_room()?;
                    swept.add(crate::reclaim::Kind::Object, count, bytes);
                }
            }
            Ok(swept)
        };
        match sweep() {
            Ok(swept) => {
                if !swept.is_empty() {
                    tracing::info!("reclaimed {swept}");
                }
                swept
            }
            Err(error) => {
                tracing::warn!("reclaim: {error}");
                crate::reclaim::Swept::default()
            }
        }
    }
    /// One pass over the ready runs in the queue's order (`queue`). An older request with a
    /// ready GPU step keeps the GPUs it waits for (`claims`) from later offers (Slurm). A deviceless
    /// call takes CPU room; one waiting on its own call takes none. Each root left queued shows
    /// its place in line and what it waits for, which this machine's log says as it changes.
    fn dispatch_ready(&self) -> io::Result<()> {
        let stopped = self.stopped.lock().unwrap();
        if *stopped {
            return Ok(());
        }
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        // Orphan exact process births remain reservations; a new owner does not
        // pretend they are free merely because it has no local supervisor.
        let active = self.engine.active(usize::MAX)?;
        let output = |record: &Execution| record.state == crate::journal::State::Running && record.process.is_none();
        let gpu = |record: &Execution| record.invocation.degree > 0 && !output(record);
        // Every GPU call slot is a permit of this machine run; an earlier run's process still
        // alive holds devices nobody here can name, so it fences them all.
        let gpu_fenced = self.gpu_startup_fences() != 0
            || active.iter().any(|r| {
                gpu(r) && !self.engine.owns(&r.id) && r.process.as_ref().is_none_or(|b| holds_gpus(b, u64::MAX))
            });
        let mut claims = vec![None; pool.width()];
        let calls = self.calls();
        let waits = |r: &Execution| calls.as_ref().is_some_and(|calls| calls.waiting(&self.engine, &r.id));
        // The runs holding the CPU slots: a run left waiting for one names them.
        let cpu_holders = || -> io::Result<Vec<Execution>> {
            Ok(self.engine.active(usize::MAX)?.into_iter()
                .filter(|r| !gpu(r) && (output(r) || !waits(r))).collect())
        };
        let cpu_active = || cpu_holders().map(|holders| holders.len());
        let mut waiting = HashSet::new();
        // CPU output work uses the existing capacity. At capacity one GPU wave may overlap;
        // once it returns, admit no more until CPU work advances.
        // A run whose plan cannot be read is its own model; its offer reports why.
        let model = |r: &Execution| self.plan(&pool, r).map_or_else(|_| r.id.clone(), |plan| plan.construction);
        // Missing required inputs have no runnable work yet. Leave them out of lane claims and
        // model-turn barriers, so another ready request can use the device.
        // The existing fetch completion callback wakes this same event-driven loop.
        let mut source_ready = vec![];
        for record in self.engine.ready()? {
            match calls.as_ref().map_or(crate::access::Ready::Yes, |calls| calls.source_ready(&record.id)) {
                crate::access::Ready::Yes => (),
                crate::access::Ready::Waiting(on) => {
                    // Preserve an active parent's registered family lane; an awaited
                    // child must be able to reenter it once its required source arrives.
                    pool.release_for_fetch(&self.engine, record.root_seq());
                    let reason = match on.is_empty() {
                        true => crate::access::WAITING_FOR_MODELS.to_owned(),
                        false => format!("{}: {on}", crate::access::WAITING_FOR_MODELS),
                    };
                    // A private run's models aren't named in this machine's log.
                    let said = match record.invocation.local_private {
                        Some(_) => crate::access::WAITING_FOR_MODELS,
                        None => reason.as_str(),
                    };
                    self.waits_for(&record, said, &mut waiting);
                    self.engine.waiting(&record.id, Some(reason))?;
                    continue;
                }
                crate::access::Ready::Failed(error) => {
                    let (code, reason) = crate::access::code_of(&error);
                    self.engine.fail_unstarted(&record.id, code, reason.to_owned())?;
                    if let Some(calls) = &calls { calls.ended(&record.id); }
                    continue;
                }
            }
            source_ready.push(record);
        }
        // The widths of roots the machine chooses for, from its packing of the whole queue (cl-816).
        let choosing = pool.width() > 1 && source_ready.iter().any(crate::estimate::chooses);
        let widths = match choosing {
            true => crate::estimate::widths(self, &pool).unwrap_or_else(|error| {
                tracing::warn!("the queue was not packed, so its roots run at their narrowest: {error}");
                Default::default()
            }),
            false => Default::default(),
        };
        let ready = self.queue.lock().unwrap().order(source_ready, model);
        let (mut place, held) = (0, self.held.get().is_some_and(|held| held()));
        let mut next_gpu = None;
        let mut next_gpu_seen = false;
        for record in ready {
            let invocation = &record.invocation;
            // Re-read for every offer, then once more inside Pool after it holds the GPU slot.
            let admit = || cpu_active().map(|used| if invocation.degree == 0 {
                used < self.parallelism
            } else { used <= self.parallelism });
            let root = invocation.parent.is_empty();
            let width = widths.get(&record.id).copied();
            if !(held && root || invocation.degree > 0 && gpu_fenced) && admit()?
                && self.offer(&pool, &record, width, &mut claims, calls.clone(), admit)? {
                if crate::queue::job(&record) {
                    self.queue.lock().unwrap().started(&record);
                }
                self.went(&record);
            } else {
                // An earlier incompatible GPU job remains a barrier. Lookahead never scans
                // past it to improve model affinity at the expense of the existing queue.
                if invocation.degree > 0 && !next_gpu_seen {
                    next_gpu_seen = true;
                    if !held && !gpu_fenced && root {
                        let plan = self.plan(&pool, &record).and_then(|plan| match width {
                            Some(width) => plan.at(width),
                            None => Ok(plan),
                        });
                        next_gpu = plan.ok().map(|plan| (record.clone(), plan));
                    }
                }
                if root {
                    place += 1;
                    // What it waits for, named: a wait no one can see the cause of is a hang to whoever watches it (cl-1086).
                    let holders = cpu_holders()?;
                    let cpu_full = match invocation.degree {
                        0 => holders.len() >= self.parallelism,
                        _ => holders.len() > self.parallelism,
                    };
                    let why = if held {
                        "waiting for this machine to finish updating".to_owned()
                    } else if invocation.degree > 0 && gpu_fenced {
                        "waiting for an earlier process to let go of the GPUs".to_owned()
                    } else if cpu_full {
                        self.cpu_wait(&holders)
                    } else if invocation.degree > 0 {
                        "waiting for a free GPU".to_owned()
                    } else {
                        "waiting for its turn".to_owned()
                    };
                    self.waits_for(&record, &why, &mut waiting);
                    let shown = self.engine.in_line(&record.id, place, why);
                    shown.unwrap_or_else(|error| tracing::warn!("run {}: its place was not shown: {error}", record.id));
                }
            }
        }
        // A run that left the queue some other way (canceled, failed) waits no more.
        self.waits.lock().unwrap().retain(|id, _| waiting.contains(id));
        pool.lookahead(&self.engine, next_gpu.as_ref().map(|(record, plan)| (record, plan)));
        Ok(())
    }
    /// What a deviceless run waits for while this machine's CPU slots are full: the runs holding them, by their callers' ids,
    /// which each caller names as people know them (`waiting for LAB2-12 to finish with this machine's CPU`).
    fn cpu_wait(&self, holders: &[Execution]) -> String {
        let named: Vec<String> = holders.iter().map(|holder| self.engine.caller_id(&holder.id)).collect();
        match named.as_slice() {
            [] => "waiting for this machine's CPU to free up".to_owned(),
            [one] => format!("waiting for {one} to finish with this machine's CPU"),
            [rest @ .., last] => format!("waiting for {} or {last} to finish with this machine's CPU", rest.join(", ")),
        }
    }
    /// Run `record` left waiting `why` this pass: this machine's log says so when what it waits for changes.
    fn waits_for(&self, record: &Execution, why: &str, waiting: &mut HashSet<String>) {
        waiting.insert(record.id.clone());
        let mut waits = self.waits.lock().unwrap();
        let (_, said) = waits.entry(record.id.clone()).or_insert_with(|| (std::time::Instant::now(), String::new()));
        if said.as_str() != why {
            why.clone_into(said);
            drop(waits);
            tracing::info!(run = %self.engine.caller_id(&record.id), "queue: {why}");
        }
    }
    /// Run `record` went: if it waited, this machine's log says how long, and for what last.
    fn went(&self, record: &Execution) {
        let Some((since, why)) = self.waits.lock().unwrap().remove(&record.id) else { return };
        tracing::info!(run = %self.engine.caller_id(&record.id), "queue: goes after {:.1} s {why}", since.elapsed().as_secs_f64());
    }
    /// The plan `record` runs: its journaled preparation's, else a weightless call's.
    fn plan(&self, pool: &Pool, record: &Execution) -> io::Result<Plan> {
        let scope = self.engine.private_storage(&record.id)?;
        crate::private_io::within(scope, || self.plan_scoped(pool, record))
    }
    fn plan_scoped(&self, pool: &Pool, record: &Execution) -> io::Result<Plan> {
        let invocation = &record.invocation;
        match record.submission.as_ref().map(|s| s.preparation_id.as_str()) {
            None | Some("") => Ok(Plan::weightless(&invocation.generation, &invocation.module, &invocation.entrypoint, invocation.degree)),
            Some(id) => {
                let actor = record.submission.as_ref().map(|s| s.actor.as_str()).unwrap_or_default();
                let preparation = self.engine.preparation(actor, id)?.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "accepted preparation is absent")
                })?;
                pool.plan(&preparation)
            }
        }
    }
    /// Start `record` if it can start now: false leaves it queued (or failed, when its
    /// environment is gone).
    fn offer(&self, pool: &Arc<Pool>, record: &Execution, width: Option<u32>, claims: &mut [Option<u64>], calls: Option<Arc<crate::calls::Calls>>, admit: impl FnOnce() -> io::Result<bool>) -> io::Result<bool> {
        let held = match self.catalog.resolve(&record.invocation.generation) {
            Ok(held) => held,
            Err(error) => {
                self.engine.fail_unstarted(&record.id, "environment_unavailable",
                    format!("The package's install on this machine is gone ({error}). Run it again to reinstall it."))?;
                return Ok(false);
            }
        };
        if !held.record.owns(&record.invocation.module, &record.invocation.package) {
            self.engine.fail_unstarted(&record.id, "environment_unavailable",
                "The package installed on this machine isn't the one this run was prepared with. Run it again.")?;
            return Ok(false);
        }
        let plan = self.plan(pool, record)?;
        pool.dispatch(&self.engine, record, held.application(&record.invocation.module)?, plan, width, claims, calls, admit)
    }
}

/// The most `free_executor_holds` waits for what busy executors let go to leave the store's holds.
const RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether `lane` goes once executors let it go: each process holding it is an idle executor `ended` for it, or a busy one
/// (`busy`) holding it only for constructions none of its calls runs on, each of them `released` (tfs-303).
fn lets_go(lane: &tensorfs::store::retention::RoomLane, ended: &BTreeSet<u32>, busy: &BTreeMap<u32, crate::gpu_service::BusyModels>,
    released: &BTreeSet<(u32, String)>) -> bool {
    lane.holders.iter().all(|pid| ended.contains(pid) || busy.get(pid).is_some_and(|models| {
        let mut held = models.idle.iter().filter(|(_, headers)| headers.iter().any(|h| lane.keeps(h))).peekable();
        !models.live.iter().any(|h| lane.keeps(h)) && held.peek().is_some() && held.all(|(c, _)| released.contains(&(*pid, c.clone())))
    }))
}
