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
    collections::{HashMap, HashSet},
    fs::File,
    io,
    path::Path,
    sync::{Arc, Mutex},
};

/// What one public submission calls on its generation.
#[derive(Default)]
pub struct Call {
    pub entrypoint: String,
    pub input: Value,
    pub attention_kernel: String,
    pub gpus: u32,
    pub lane: String,
    pub inputs: Vec<crate::journal::InputFile>,
    /// GPUs its plan runs on: 0 deviceless.
    pub degree: u32,
    /// A child run's parent execution id.
    pub parent: String,
    /// The callee App a child run calls in its parent's environment; empty for the root's.
    pub application: String,
}

pub struct Service {
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
}

const SWEEP_EVERY: std::time::Duration = crate::reclaim::IDLE;

/// Whether an executor this machine run does not supervise still fences GPU dispatch: it, or a
/// member of its group born before `before`, is alive and not stuck in the kernel after its kill.
/// One stuck (D state) is shown in the machine's status and fences nothing: what it holds stays
/// held, its GPU memory included, as NVML reads it.
fn holds_gpus(birth: &Birth, before: u64) -> bool {
    let mut left: Vec<Birth> = platform::process::group_members(birth)
        .into_iter()
        .filter(|m| m.start_ticks < before && m.state().is_ok_and(|s| s != platform::process::State::Gone))
        .collect();
    if !birth.ended().unwrap_or(false) {
        left.push(birth.clone());
    }
    !left.is_empty() && !platform::contain::all_stuck(&left)
}
impl Service {
    pub fn open(root: &Path, generations: &Path, parallelism: usize) -> io::Result<Arc<Self>> {
        if parallelism == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "CPU parallelism must be positive",
            ));
        }
        let engine = Engine::open(&root.join("execution"))?;
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
            Ok(held) => tracing::info!("ended {held} process(es) left in earlier executor scopes"),
            Err(error) => tracing::warn!("earlier executor scopes remain: {error}"),
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
        });
        service.engine.reconcile()?;
        service.reclaim();
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
        std::thread::Builder::new()
            .name("machine-reclaim".into())
            .spawn(move || Self::reclaim_loop(weak))?;
        Ok(service)
    }
    /// The executor pool every run runs on, once configured.
    pub fn pool(&self) -> Option<Arc<Pool>> {
        self.pool.lock().unwrap().clone()
    }
    /// The pool when this machine has GPUs.
    pub fn gpu(&self) -> Option<Arc<Pool>> {
        self.pool().filter(|pool| pool.width() > 0)
    }
    pub fn calls(&self) -> Option<Arc<crate::calls::Calls>> {
        self.calls.lock().unwrap().clone()
    }
    pub fn configure_publisher(&self, publisher: Arc<crate::published::Publisher>) {
        *self.publisher.lock().unwrap() = Some(publisher);
    }
    pub fn configure_calls(&self, calls: Arc<crate::calls::Calls>) {
        *self.calls.lock().unwrap() = Some(calls);
        self.engine.notify_activity();
    }
    /// Observation only. Unknown births stay reserved; CPU dispatch stays available.
    pub fn gpu_startup_fences(&self) -> usize {
        let mut births = self.startup_gpu_births.lock().unwrap();
        // A group's followers die with their leader (one group kill); each exit is observed.
        births.retain(|birth| holds_gpus(birth, self.started_ticks));
        births.len()
    }
    pub fn configure_pool(&self, pool: Arc<Pool>) -> io::Result<()> {
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
        if pool.width() > 0 {
            pool.watch_faults();
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
            let mut seen = engine.activity_epoch();
            while fence.iter().any(|birth| holds_gpus(birth, started)) {
                seen = engine.wait_activity(seen, Some(std::time::Duration::from_millis(100)));
            }
            gpu.keep_all(&engine);
        })?;
        Ok(())
    }

    /// A run prepared inside itself (`runs`) names its generation, call and model plan, and how
    /// the machine memo met it; one the memo answered (`answer`) completes with that result.
    pub fn bind_prepared(
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
        invocation.gpus = call.gpus;
        invocation.lane = call.lane;
        invocation.inputs = call.inputs;
        invocation.degree = call.degree;
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
    pub fn changed_environment(&self) -> io::Result<()> {
        self.engine.notify_activity();
        Ok(())
    }
    /// A package installed, release or local: its environment's bytecode and kernels compile and
    /// its import-only parent imports now, while its weights come, so its first call forks
    /// (`Pool::installed`).
    pub fn installed(&self, held: &crate::catalog::HeldGeneration) {
        crate::published::environment::compile_bytecode(&held.record.python);
        if let Some(gpu) = self.gpu() {
            gpu.installed(held.clone());
        }
        self.engine.notify_activity();
    }
    /// The machine started: a dedicated one's installed environments' import-only parents import,
    /// the most recently used first, while the host has room for them, and the rest's import
    /// pages are read ahead; a shared one reads ahead its few most recently used
    /// (`Pool::import_recent`).
    pub fn import_installed(&self) -> io::Result<()> {
        let Some(gpu) = self.gpu() else { return Ok(()) };
        let bound = self.engine.with_journal(|journal| journal.bound_generations())?;
        gpu.import_recent(self.catalog.recent(bound));
        Ok(())
    }
    pub fn idle(&self) -> io::Result<bool> {
        Ok(self.engine.nonterminal(1)?.is_empty())
    }
    pub fn hold_while(&self, held: impl Fn() -> bool + Send + Sync + 'static) {
        let _ = self.held.set(Box::new(held));
    }
    /// This computer's machine idle a while: every idle executor outside the warm sets ends, and
    /// what it held goes back.
    pub fn release_idle(&self) {
        if let Some(pool) = self.pool() {
            pool.release_idle();
        }
    }
    /// This computer's machine idle its whole window yet kept up: a shared one's idle import-only
    /// parents end too.
    pub fn release_parents(&self) {
        if let Some(pool) = self.pool() {
            pool.release_parents();
        }
    }
    /// A warm set member is kept in memory here: this computer's machine stays up for it.
    pub fn keeps_warm(&self) -> bool {
        self.pool().is_some_and(|pool| pool.keeps_warm())
    }
    pub fn stop(&self) -> io::Result<bool> {
        let mut stopped = self.stopped.lock().unwrap();
        if !self.idle()? {
            return Ok(false);
        }
        *stopped = true;
        if let Some(pool) = self.pool() {
            pool.stop()?;
        }
        self.engine.notify_activity();
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
    /// Observe GPU demand on the existing reclaim thread; disk caches retain their TTL cadence.
    fn reclaim_loop(weak: std::sync::Weak<Self>) {
        let mut swept: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            match weak.upgrade() {
                Some(service) if !*service.stopped.lock().unwrap() => {
                    if let Some(pool) = service.gpu() { pool.relieve_pressure(&service.engine); }
                    // The first once its packages are configured, so the disk is measured early.
                    let due = swept.map_or_else(|| service.publisher.lock().unwrap().is_some(), |at| at.elapsed() >= SWEEP_EVERY);
                    if due {
                        service.reclaim();
                        swept = Some(std::time::Instant::now());
                    }
                }
                _ => return,
            }
        }
    }
    /// Caches manage themselves (`reclaim`): a TTL and a low disk, never a purge verb. Each
    /// sweep logs what it freed and measures what fills the disk.
    pub fn reclaim(&self) -> crate::reclaim::Swept {
        self.sweep(0, true)
    }
    /// The machine's garbage for a store admit of `incoming` bytes that finds the disk short,
    /// before the store evicts a model (`Store::on_short`): never kernels, memoized stages or
    /// run objects.
    pub fn reclaim_for(&self, incoming: u64) {
        self.sweep(incoming, false);
    }
    /// A sweep as though `incoming` more bytes were taken; `all` caches, or the garbage alone.
    fn sweep(&self, incoming: u64, all: bool) -> crate::reclaim::Swept {
        let publisher = self.publisher.lock().unwrap().clone();
        let uv_cache = publisher.as_ref().and_then(|publisher| publisher.uv_cache());
        let generations = self.catalog.root();
        let engine = generations.parent().unwrap_or(generations);
        // Run objects no unfinished run needs go first.
        let objects = match (all, self.calls()) {
            (true, Some(calls)) => calls.sweep_objects().unwrap_or_else(|error| {
                tracing::warn!("reclaim object roots: {error}");
                (0, 0)
            }),
            _ => (0, 0),
        };
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
                disk: &disk,
            })?;
            swept.add(crate::reclaim::Kind::Object, objects.0, objects.1);
            Ok(swept)
        };
        let swept = sweep();
        if let Some(publisher) = publisher.as_ref().filter(|_| all) {
            crate::reclaim::measure_usage(publisher.store().root(), engine, uv_cache.as_deref());
        }
        match swept {
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
    /// its place in line.
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
        let cpu_active = || -> io::Result<usize> {
            Ok(self.engine.active(usize::MAX)?.iter()
                .filter(|r| !gpu(r) && (output(r) || !waits(r))).count())
        };
        // CPU output work uses the existing capacity. At capacity one GPU wave may overlap;
        // once it returns, admit no more until CPU work advances.
        // A run whose plan cannot be read is its own model; its offer reports why.
        let model = |r: &Execution| self.plan(&pool, r).map_or_else(|_| r.id.clone(), |plan| plan.construction);
        // Missing required inputs have no runnable GPU work yet. Leave them out of lane
        // claims and model-turn barriers, so another ready request can use the device.
        // The existing fetch completion callback wakes this same event-driven loop.
        let mut source_ready = vec![];
        for record in self.engine.ready()? {
            if record.invocation.degree > 0 {
                match calls.as_ref().map_or(crate::access::Ready::Yes, |calls| calls.source_ready(&record.id)) {
                    crate::access::Ready::Yes => (),
                    crate::access::Ready::Waiting(on) => {
                        // Preserve an active parent's registered family lane; an awaited
                        // child must be able to reenter it once its required source arrives.
                        pool.release_for_fetch(&self.engine, record.root_seq());
                        let reason = match on.is_empty() {
                            true => "waiting for required model inputs".to_owned(),
                            false => format!("waiting for required model inputs: {on}"),
                        };
                        self.engine.waiting(&record.id, Some(reason))?;
                        continue;
                    }
                    crate::access::Ready::Failed(error) => {
                        self.engine.fail_unstarted(&record.id, "model_fetch_failed", error)?;
                        if let Some(calls) = &calls { calls.ended(&record.id); }
                        continue;
                    }
                }
            }
            source_ready.push(record);
        }
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
            if !(held && root || invocation.degree > 0 && gpu_fenced) && admit()?
                && self.offer(&pool, &record, &mut claims, calls.clone(), admit)? {
                if root {
                    self.queue.lock().unwrap().started(&record);
                }
            } else {
                // An earlier incompatible GPU job remains a barrier. Lookahead never scans
                // past it to improve model affinity at the expense of the existing queue.
                if invocation.degree > 0 && !next_gpu_seen {
                    next_gpu_seen = true;
                    if !held && !gpu_fenced && root {
                        next_gpu = self.plan(&pool, &record).ok().map(|plan| (record.clone(), plan));
                    }
                }
                if root {
                    place += 1;
                    let after = if held { ", after the machine's update" } else { "" };
                    let shown = self.engine.waiting(&record.id, Some(format!("position {place} in the queue{after}")));
                    shown.unwrap_or_else(|error| tracing::warn!("run {}: its place was not shown: {error}", record.id));
                }
            }
        }
        pool.lookahead(&self.engine, next_gpu.as_ref().map(|(record, plan)| (record, plan)));
        Ok(())
    }
    /// The plan `record` runs: its journaled preparation's, else a weightless call's.
    fn plan(&self, pool: &Pool, record: &Execution) -> io::Result<Plan> {
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
    fn offer(&self, pool: &Arc<Pool>, record: &Execution, claims: &mut [Option<u64>], calls: Option<Arc<crate::calls::Calls>>, admit: impl FnOnce() -> io::Result<bool>) -> io::Result<bool> {
        let held = match self.catalog.resolve(&record.invocation.generation) {
            Ok(held) => held,
            Err(error) => {
                self.engine.fail_unstarted(&record.id, "environment_unavailable",
                    format!("held generation unavailable: {error}"))?;
                return Ok(false);
            }
        };
        if !held.record.owns(&record.invocation.module, &record.invocation.package) {
            self.engine.fail_unstarted(&record.id, "environment_unavailable",
                "held package identity differs from accepted invocation")?;
            return Ok(false);
        }
        let plan = self.plan(pool, record)?;
        pool.dispatch(&self.engine, record, held.application(&record.invocation.module)?, plan, claims, calls, admit)
    }
}
