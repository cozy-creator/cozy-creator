//! A predicted request's load (cl-1123, Paul 11-17: "if the target machine GPU is idle, like load the model weights into memory").
//! Once a list item's run has brought its weights here (`crate::ahead`), and while no call wants a GPU, its executor starts, builds
//! the call's models and puts their weights on a GPU, as a call of it would (the same plan, lane and Load), so that call's first
//! step loads nothing: it takes the executor the load left idle.
//!
//! Only on a dedicated machine (a rental, or a computer its owner gave to Cozy: `machine.mode`), one load at a time, and only into
//! room no one else's model holds: free memory, or what idle executors of the same actor and project hold that no listed item
//! loaded, which varena's own order then takes. A real call yields it at once (`yield_to`): a load of another model ends, its
//! executor killed and its GPU's call slot given back with the kill, not once the executor has exited (5.1 s on an A40,
//! 44050923094): a load runs no code of a package's and starts no kernel a call could meet, and what the dying executor holds
//! comes back at its exit through varena's ledger, as any peer's does. A load of the same model goes on, and the call takes what
//! it made. A list that drops an item loading lets the
//! load finish, as a render of it may have just been asked. It is no run: it never counts as a job, so a rental's idle clock never
//! sees it.
//!
//! An item ready on disk goes on in steps (cl-1318; Paul, 11-19: "warming page cache; get the model into RAM / page-cache so it can be
//! read more quickly"; "begin loading it into VRAM as soon as possible, when it's known to be needed"), each as soon as it can:
//! 1. downloaded: its run (`crate::ahead`);
//! 2. in RAM: its weights read into the page cache (`stage_next`, `crate::stage`) while no GPU takes them, within varena's host number
//!    less what the items listed before it hold there, giving way to any real call;
//! 3. built: where its GPU can't hold it whole (or another model holds the room), its executor started and its models built, mapped
//!    at level `ram` (`try_load`): only a context on the GPU, and the Runtime merges its LoRAs there;
//! 4. loaded: mapped at level `gpu` once its GPU has the room (`try_load`), from RAM, at the bound of PCIe rather than the disk.
use super::*;
use std::collections::BTreeSet;

/// The calls on a GPU that reported a step, by run (`stepped`): past their loads. Until its first step a call reads its weights,
/// and a stage shares the disk with none (cl-1318: a stage that went on beside a call's cold load slowed the load it was for).
static STEPPED: std::sync::LazyLock<Mutex<BTreeSet<String>>> = std::sync::LazyLock::new(Mutex::default);

/// The calls on a GPU whose function TensorD has invoked (`invoked`): built, and into their own code. A construction's legs come
/// timed and counted like steps (a stage restarted 0.4 s into SDXL's, 44655831537), so a step is only one of an invoked call's.
static INVOKED: std::sync::LazyLock<Mutex<BTreeSet<String>>> = std::sync::LazyLock::new(Mutex::default);

/// Run `id`'s function is invoked now.
pub(crate) fn invoked(id: &str) {
    INVOKED.lock().unwrap().insert(id.to_owned());
}

/// Run `id` reported a step (`calls::observe`): once invoked, it is past its loads, a stage may read beside it now, and the load
/// step looks again.
pub(crate) fn stepped(engine: &Engine, id: &str, frame: &crate::device_executor::Frame) {
    if !INVOKED.lock().unwrap().contains(id) {
        return;
    }
    let mut stepped = STEPPED.lock().unwrap();
    if !stepped.contains(id) {
        stepped.insert(id.to_owned());
        drop(stepped);
        tracing::info!(run = %id, "its first step: {} {:?}/{:?}, {:?} ms: a stage may read beside it", frame.stage, frame.position, frame.total, frame.step_ms);
        engine.notify_activity();
    }
}

/// Run `id` left its GPU.
pub(crate) fn unstepped(id: &str) {
    INVOKED.lock().unwrap().remove(id);
    STEPPED.lock().unwrap().remove(id);
}

/// Whom an executor served last: an actor, and its local private project.
pub(super) type Scope = (String, Option<String>);

/// A predicted request the machine may load: its list item's run, by its caller's id, and the call it plans.
pub(crate) struct Predicted {
    /// `cozy:upcoming:<key>` or `cozy:prepare:<key>`.
    pub caller: String,
    pub actor: String,
    /// Its local private project.
    pub project: Option<String>,
    pub held: HeldGeneration,
    pub plan: Plan,
    /// Its on-demand models its request reads (by parameter; its function's rules on its payload): built ahead too, so its first
    /// step builds nothing.
    pub build: Vec<String>,
}

#[derive(Default)]
pub(super) struct Ahead {
    items: Mutex<Vec<Arc<Item>>>,
    /// Each item planned (by actor and caller id) since this machine started, or since its list last named it.
    planned: Mutex<BTreeSet<(String, String)>>,
    loading: Mutex<Option<Loading>>,
    /// Its thread, started with the first item.
    serving: std::sync::OnceLock<()>,
}

struct Item {
    predicted: Predicted,
    /// What its call reads up front, in bytes: what its load puts on each GPU of its lane.
    need: u64,
    state: Mutex<ItemState>,
}

#[derive(Default)]
struct ItemState {
    /// Why it isn't loading now.
    held_back: String,
    /// Once loaded: its executor, and the most varena's ledger has said it holds since.
    loaded: Option<(u32, u64)>,
    /// Its load failed: it isn't tried again until it comes anew.
    failed: bool,
    /// Its weights read into RAM while the GPU can't take them (`stage_next`).
    staged: Option<Arc<crate::stage::Stage>>,
    /// Built at level `ram` by this executor: its GPU couldn't hold it whole.
    in_ram: Option<u32>,
    /// Built at level `ram`, its GPU load failed with this much free on its GPU (another process held the rest): it waits, built,
    /// until more is free, and never reads as failed for it.
    gpu_short: Option<u64>,
}

struct Loading {
    actor: String,
    caller: String,
    construction: String,
    /// `Gpu`, or `Host`: built and mapped at level `ram`.
    level: Level,
    stop: Arc<Stoppable>,
    /// Its lane's call slots, until it ends or a real call ends it.
    permit: Option<Permit>,
    /// When a real call ended it.
    halted: Option<Instant>,
}

impl Item {
    /// What its load reads: its built slots' checkpoints and components.
    fn selected(&self) -> Vec<(String, Vec<String>)> {
        let predicted = &self.predicted;
        let built = |slot: &&PlanSlot| !slot.on_demand || predicted.build.contains(&slot.binding.model_parameter_name);
        merged(predicted.plan.slots.iter().filter(built)).into_iter().map(|selected| (selected.manifest, selected.components)).collect()
    }
    fn scope(&self) -> Scope {
        (self.predicted.actor.clone(), self.predicted.project.clone())
    }
    /// Why it isn't loading now; whether that changed.
    fn hold_back(&self, why: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let changed = state.held_back != why;
        if changed {
            why.clone_into(&mut state.held_back);
        }
        changed
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

impl Pool {
    /// `predicted`'s weights are here: it loads when a GPU is idle. Its like (the same caller's item) is replaced. On a shared
    /// machine (this computer, unless its owner made it dedicated) it never loads.
    pub(crate) fn predict(self: &Arc<Self>, engine: &Arc<Engine>, predicted: Predicted) {
        if self.config.host.mode != HostMode::Dedicated || predicted.plan.degree == 0 || predicted.plan.slots.is_empty() {
            return;
        }
        let built = |slot: &&PlanSlot| !slot.on_demand || predicted.build.contains(&slot.binding.model_parameter_name);
        let need = predicted.plan.slots.iter().filter(built).map(|slot| slot.selected_encoded_bytes).sum();
        self.ahead.planned.lock().unwrap().insert((predicted.actor.clone(), predicted.caller.clone()));
        {
            let mut items = self.ahead.items.lock().unwrap();
            items.retain(|item| !(item.predicted.actor == predicted.actor && item.predicted.caller == predicted.caller));
            items.push(Arc::new(Item { predicted, need, state: Mutex::default() }));
        }
        let (pool, woken) = (Arc::downgrade(self), Arc::downgrade(engine));
        self.ahead.serving.get_or_init(|| {
            let started = std::thread::Builder::new().name("load-ahead".into()).spawn(move || serve(&pool, &woken));
            if let Err(error) = started {
                tracing::warn!("predicted requests won't load ahead: {error}");
            }
        });
        engine.notify_activity();
    }

    /// `actor`'s lists changed: what they no longer name leaves. One loading finishes: the list may have dropped it for a render of
    /// it, which takes its executor.
    pub(crate) fn unlist(&self, actor: &str) {
        let named = |item: &Arc<Item>| item.predicted.actor != actor || crate::ahead::rank(actor, &item.predicted.caller).is_some();
        self.ahead.items.lock().unwrap().retain(named);
        self.ahead.planned.lock().unwrap().retain(|(of, caller)| of != actor || crate::ahead::rank(actor, caller).is_some());
    }

    /// `actor`'s item `caller`, whose run made it ready, was never planned for its load since this machine started or since its
    /// list last named it: it is to be, once (`Runs::replan`). Never on a machine that doesn't load ahead.
    pub(crate) fn plans_again(&self, actor: &str, caller: &str) -> bool {
        self.config.host.mode == HostMode::Dedicated && self.ahead.planned.lock().unwrap().insert((actor.to_owned(), caller.to_owned()))
    }

    /// A predicted request is loading now, or its weights are being read into RAM.
    pub(crate) fn loading_ahead(&self) -> bool {
        let loading = self.ahead.loading.lock().unwrap().is_some();
        loading || self.ahead.items.lock().unwrap().iter().any(|item| item.state.lock().unwrap().staged.as_ref().is_some_and(|stage| !stage.ended()))
    }

    /// The calls left waiting for a GPU this dispatch pass, by construction: a load of any other ends now; one of theirs goes on,
    /// and the call takes its executor.
    pub(crate) fn yield_to(&self, waiting: &[String]) {
        // A stage of another model gives the disk to the call, and goes on once no call waits (`stage_next`).
        for item in self.ahead.items.lock().unwrap().iter() {
            let staged = item.state.lock().unwrap().staged.clone();
            if let Some(stage) = staged.filter(|stage| !stage.ended() && !stage.stopped()) {
                if waiting.iter().any(|construction| *construction != item.predicted.plan.construction) {
                    stage.stop();
                    tracing::info!(run = %item.predicted.caller, "stage ahead: stopped for a run waiting for the GPU, {} of {} in RAM", gib(stage.done()), gib(stage.total()));
                }
            }
        }
        let (stop, permit) = {
            let mut loading = self.ahead.loading.lock().unwrap();
            let other = |loading: &&mut Loading| loading.halted.is_none() && waiting.iter().any(|construction| *construction != loading.construction);
            let Some(loading) = loading.as_mut().filter(other) else { return };
            loading.halted = Some(Instant::now());
            (loading.stop.clone(), loading.permit.take())
        };
        // Killed first (or at its birth, if it has none yet): then its slot is the call's.
        stop.halt();
        if let Some(permit) = permit {
            drop(permit);
            tracing::info!(run = %stop.id(), "load ahead: its GPU went to a run waiting for it");
        }
    }

    /// How ready the machine is for `actor`'s item `caller` beyond its weights: `loading`, `loaded` or `evicted`; else `ready`
    /// with why it isn't loading, when it was looked at. None: no load step for it.
    pub(crate) fn readiness(&self, actor: &str, caller: &str) -> Option<api::machine::Readiness> {
        let item = self.ahead.items.lock().unwrap().iter().find(|item| item.predicted.actor == actor && item.predicted.caller == caller).cloned()?;
        let at = |stage: &str, reason: String| Some(api::machine::Readiness { stage: stage.into(), total: item.need, reason, ..Default::default() });
        let loading = self.ahead.loading.lock().unwrap().as_ref().filter(|loading| loading.actor == actor && loading.caller == caller).map(|loading| loading.level);
        match loading {
            Some(Level::Gpu) => return at("loading", String::new()),
            Some(_) => return at("loading", "into RAM: its executor and models, as its GPU can't hold it now".into()),
            None => {}
        }
        let on_gpu = self.on_gpu(&item);
        let in_ram = self.in_ram(&item);
        let state = item.state.lock().unwrap();
        // In RAM, or on its way there, while no GPU holds it: its load then reads RAM, at the bound of PCIe.
        let staged = |stage: &crate::stage::Stage| {
            let (name, done) = match stage.ended() {
                false => ("staging", stage.done()),
                true => ("staged", stage.resident()),
            };
            Some(api::machine::Readiness { stage: name.into(), done, total: stage.total(), reason: state.held_back.clone() })
        };
        match (on_gpu, state.loaded.is_some(), &state.staged) {
            (Some(true), ..) => at("loaded", String::new()).map(|loaded| api::machine::Readiness { done: item.need, ..loaded }),
            // Its executor and models are built, at level `ram`: what of its weights the page cache holds is `done`.
            _ if in_ram => Some(api::machine::Readiness { stage: "built".into(), done: state.staged.as_ref().map_or(0, |stage| stage.resident()),
                total: item.need, reason: state.held_back.clone() }),
            (_, _, Some(stage)) if stage.done() > 0 || !stage.ended() => staged(stage),
            (Some(false), ..) => at("evicted", "a run took its GPU memory; it loads again once the GPU is idle".into()),
            (None, true, _) => at("evicted", "its executor ended for a run's room; it loads again once the GPU is idle".into()),
            (None, false, _) if !state.held_back.is_empty() => at("ready", state.held_back.clone()),
            (None, false, _) => None,
        }
    }

    /// Whether `item`'s models are on a GPU (an executor holds its construction there): None if not; false once a run took that
    /// memory from the executor its load left idle, as varena's ledger shows: what it holds there under 90% of the most it has
    /// said it held since. An idle executor says what it holds only once asked for memory, as a run's ask takes it.
    fn on_gpu(&self, item: &Item) -> Option<bool> {
        let construction = &item.predicted.plan.construction;
        let (start, pid) = {
            let levels = self.levels.lock().unwrap();
            let (key, (_, _, birth)) = levels.iter().find(|(key, (_, level, _))| key.split('@').next() == Some(construction.as_str()) && *level == Level::Gpu)?;
            (key.split_once('@').and_then(|(_, start)| start.parse().ok()).unwrap_or(0usize), birth.pid)
        };
        // An idle executor holds it: its request's on-demand models are built there too, or it is to load (they are built then).
        let built = |session: &Session| item.predicted.build.iter().all(|parameter| session.built.contains(&format!("{construction}#{parameter}")));
        let idle = self.sessions.lock().unwrap().values().find(|session| session.executor.birth.pid == pid).map(built);
        if idle == Some(false) {
            return None;
        }
        let mut state = item.state.lock().unwrap();
        // A call or a warm set made it, or a call has it now: the pool's own account stands.
        let Some((_, most)) = state.loaded.as_mut().filter(|(loaded, _)| *loaded == pid && idle.is_some()) else { return Some(true) };
        let held = self.holdings(start, pid).map_or(0, |(held, _)| held);
        *most = (*most).max(held);
        Some(held.saturating_mul(10) >= most.saturating_mul(9))
    }

    /// Whether the executor its load built it in at level `ram` holds it still, at that level.
    fn in_ram(&self, item: &Item) -> bool {
        let Some(pid) = item.state.lock().unwrap().in_ram else { return false };
        let construction = &item.predicted.plan.construction;
        self.levels.lock().unwrap().iter()
            .any(|(key, (_, level, birth))| key.split('@').next() == Some(construction.as_str()) && *level == Level::Host && birth.pid == pid)
    }

    /// What process `pid` holds on GPU `device`, and of it what is idle, as varena's ledger says.
    #[cfg(target_os = "linux")]
    fn holdings(&self, device: usize, pid: u32) -> Option<(u64, u64)> {
        self.devices.get(device)?.ledger.as_ref()?.holdings(pid)
    }
    #[cfg(not(target_os = "linux"))]
    fn holdings(&self, _: usize, _: u32) -> Option<(u64, u64)> {
        None
    }

    /// The listed items in their order (upcoming jobs first, each list in its), those no list names now gone.
    fn ranked(&self) -> Vec<Arc<Item>> {
        let mut items = self.ahead.items.lock().unwrap();
        items.retain(|item| crate::ahead::rank(&item.predicted.actor, &item.predicted.caller).is_some());
        let mut ranked = items.clone();
        ranked.sort_by_key(|item| crate::ahead::rank(&item.predicted.actor, &item.predicted.caller));
        ranked
    }

    /// One look: the first listed item not on a GPU loads, if a GPU is idle and it fits. Whether anything changed.
    fn load_next(self: &Arc<Self>, engine: &Arc<Engine>) -> bool {
        let items = self.ranked();
        let listed: BTreeSet<String> = items.iter().map(|item| item.predicted.plan.construction.clone()).collect();
        let mut changed = false;
        for item in &items {
            if item.state.lock().unwrap().failed {
                continue;
            }
            if self.on_gpu(item) == Some(true) {
                changed |= item.hold_back("");
                continue;
            }
            match self.try_load(engine, item, &listed) {
                Ok(()) => return true,
                // A GPU wanted elsewhere holds back every item; room, only this one.
                Err((why, busy)) => {
                    changed |= item.hold_back(&why);
                    if busy {
                        break;
                    }
                }
            }
        }
        self.stage_next(engine, &items) || changed
    }

    /// A real call reads its weights now: one waits for a GPU (it reads ahead meanwhile), or one is on a GPU and hasn't reported a
    /// step. A stage reads only when none does, by construction: an idle I/O class is no order under the `none` scheduler most NVMe
    /// disks use. It still reads beside another call's steps.
    fn loading_call(&self, engine: &Engine) -> bool {
        if gpu_request_queued(engine, None) {
            return true;
        }
        let mut stepped = STEPPED.lock().unwrap();
        let lanes = self.lanes.lock().unwrap();
        // A run off any GPU here (a CPU call's) is no one's to keep.
        stepped.retain(|id| lanes.contains_key(id));
        INVOKED.lock().unwrap().retain(|id| lanes.contains_key(id));
        lanes.keys().any(|id| !stepped.contains(id))
    }

    /// The first listed item no GPU has taken, whose weights are here, read into RAM (`crate::stage`), one at a time and never while
    /// a real call reads its weights (`loading_call`; one that starts stops it at its next chunk): within the host number's room
    /// (what may still be held: clean cache isn't held, so this displaces only cache) less what the items listed before it hold
    /// there. One that ended whole or short stands; one a call stopped goes on later. Whether one began.
    fn stage_next(self: &Arc<Self>, engine: &Arc<Engine>, items: &[Arc<Item>]) -> bool {
        if self.loading_call(engine) {
            return false;
        }
        let mut before = 0u64;
        for item in items {
            if self.on_gpu(item) == Some(true) {
                continue;
            }
            let held_back = {
                let state = item.state.lock().unwrap();
                match &state.staged {
                    _ if state.failed => continue,
                    Some(stage) if !stage.ended() => return false,
                    Some(stage) if !stage.stopped() => {
                        before += stage.done();
                        continue;
                    }
                    _ => state.held_back.clone(),
                }
            };
            let chunks = crate::stage::ordered(&self.store, &item.selected());
            if chunks.is_empty() {
                continue;
            }
            let room = move |cached: u64| varena_ledger::host::budget().room().saturating_sub(i64::try_from(before.saturating_add(cached)).unwrap_or(i64::MAX));
            let (listed, pool, woken) = (Arc::downgrade(item), Arc::downgrade(self), Arc::downgrade(engine));
            let (looks, wakes) = (Arc::downgrade(self), Arc::downgrade(engine));
            let live = move || {
                let calls = |pool: Arc<Pool>| wakes.upgrade().is_some_and(|engine| !pool.loading_call(&engine));
                listed.strong_count() > 0 && looks.upgrade().is_some_and(calls)
            };
            let (caller, construction) = (item.predicted.caller.clone(), item.predicted.plan.construction.clone());
            let ended = move |stage: &crate::stage::Stage| {
                let (read, ms) = (stage.read(), stage.took_ms());
                let why = match (stage.stopped(), stage.short()) {
                    (true, _) => "; stopped",
                    (false, true) => "; the rest didn't fit in the host's room",
                    _ => "",
                };
                tracing::info!(run = %caller, "stage ahead: {} of {} in RAM in {ms} ms ({} there before, {} read from disk at {:.2} GB/s){why}",
                    gib(stage.done()), gib(stage.total()), gib(stage.cached()), gib(read), read as f64 / (ms.max(1) as f64 * 1e6));
                if let Some(pool) = pool.upgrade() {
                    pool.notes.note(serde_json::json!({"event": "stage_ahead", "run": caller, "plan": construction, "total": stage.total(),
                        "cached": stage.cached(), "read": read, "ms": ms, "stopped": stage.stopped(), "short": stage.short()}));
                }
                if let Some(engine) = woken.upgrade() {
                    engine.notify_activity();
                }
            };
            tracing::info!(run = %item.predicted.caller, "stage ahead: its weights into RAM, as no GPU takes them now ({held_back})");
            item.state.lock().unwrap().staged = Some(crate::stage::start(&self.store, chunks, room, live, true, ended));
            return true;
        }
        false
    }

    /// `item` loaded now, or why not and whether that's the GPU being in use.
    fn try_load(self: &Arc<Self>, engine: &Arc<Engine>, item: &Arc<Item>, listed: &BTreeSet<String>) -> Result<(), (String, bool)> {
        let plan = &item.predicted.plan;
        let absent = merged(plan.slots.iter().filter(|slot| !slot.on_demand)).into_iter()
            .any(|selected| !crate::access::whole(&self.store, &selected.manifest, &selected.components));
        if absent {
            return Err(("its weights aren't all here".into(), false));
        }
        if gpu_request_queued(engine, None) {
            return Err(("a run is waiting for the GPU".into(), true));
        }
        // The lanes a call of it would take (one its environment's executor is on first), those with room for it. Room is looked
        // at before any GPU is taken: a call slot taken and given back wakes every waiter, this one too.
        let degree = plan.degree as usize;
        let hosts: Vec<usize> = self.hosting(plan).into_iter().map(|(start, _)| start).collect();
        let others = (0..(self.width() + 1).saturating_sub(degree)).filter(|start| !hosts.contains(start));
        let starts: Vec<usize> = hosts.iter().copied().chain(others).collect();
        let fit = |level: Level, short: &mut Option<String>| -> Vec<usize> {
            starts.iter().copied().filter(|&start| match self.room(item, start..start + degree, listed, level) {
                Ok(()) => true,
                Err(why) => {
                    short.get_or_insert(why);
                    false
                }
            }).collect()
        };
        let mut short = None;
        let mut gpu = fit(Level::Gpu, &mut short);
        let freed = |start: &usize| self.free(&(*start..*start + degree));
        if let Some(was) = item.state.lock().unwrap().gpu_short.filter(|_| self.in_ram(item)) {
            gpu.retain(|start| freed(start).is_some_and(|free| free > was));
            if gpu.is_empty() {
                short.get_or_insert_with(|| format!("built, its GPU load waits for more than the {} it had free", gib(was)));
            }
        }
        let short = short.unwrap_or_else(|| "no GPU here serves it".into());
        // Its GPU can't hold it whole now: its executor and models are built at level `ram`, once, beside what holds the room.
        let (level, fits) = match gpu.is_empty() {
            false => (Level::Gpu, gpu),
            true if self.in_ram(item) => return Err((short, false)),
            true => match fit(Level::Host, &mut None) {
                ram if ram.is_empty() => return Err((short, false)),
                ram => (Level::Host, ram),
            },
        };
        // In flight before it takes a GPU, so a call queued meanwhile ends it (`yield_to`).
        let stop = Arc::new(engine.stoppable(&format!("load-ahead:{}:{}", item.predicted.actor, item.predicted.caller)));
        *self.ahead.loading.lock().unwrap() = Some(Loading {
            actor: item.predicted.actor.clone(),
            caller: item.predicted.caller.clone(),
            construction: plan.construction.clone(),
            level,
            stop: stop.clone(),
            permit: None,
            halted: None,
        });
        let placed = fits.iter().find_map(|&start| self.place(plan, (None, None), Some(start..start + degree), &mut vec![None; self.width()]));
        let Some((start, permit)) = placed.filter(|_| !stop.stop.is_cancelled()) else {
            self.ahead.loading.lock().unwrap().take();
            return Err(("a run holds the GPU".into(), true));
        };
        // Its slot is the load's to give back from here: a call that came while it was placed has it at once.
        let mut loading = self.ahead.loading.lock().unwrap();
        match loading.as_mut().filter(|loading| loading.halted.is_none()) {
            Some(loading) => loading.permit = Some(permit),
            None => {
                *loading = None;
                drop(loading);
                drop(permit);
                return Err(("a run is waiting for the GPU".into(), true));
            }
        }
        drop(loading);
        let lane = start..start + degree;
        match level {
            // Its load reads the disk now, ahead of any stage (one of it would read the same chunks beside it); they go on after.
            Level::Gpu => {
                for listed in self.ahead.items.lock().unwrap().iter() {
                    if let Some(stage) = listed.state.lock().unwrap().staged.as_ref().filter(|stage| !stage.ended()) {
                        stage.stop();
                    }
                }
                item.hold_back("");
            }
            // Its weights come into RAM while its executor starts and its models build.
            _ => {
                item.hold_back(&short);
                self.stage_next(engine, &self.ranked());
            }
        }
        engine.notify_activity();
        let began = Instant::now();
        let free = self.free(&lane);
        let result = self.load_item(engine, item, start, &stop, level);
        // Its slot, unless a call took it.
        let loading = self.ahead.loading.lock().unwrap().take();
        let halted = loading.as_ref().and_then(|loading| loading.halted);
        drop(loading);
        drop(stop);
        let (caller, construction) = (&item.predicted.caller, &plan.construction);
        let gpus = self.lane_devices(start, plan.degree).unwrap_or_default();
        let ms = began.elapsed().as_millis() as u64;
        if let Some(halted) = halted {
            // Its slot went to the call with the kill (`yield_to`); this is how long its executor took to exit after it.
            let exit_ms = halted.elapsed().as_millis() as u64;
            tracing::info!(run = %caller, "load ahead: ended for a run {ms} ms after it began; its executor exited {exit_ms} ms after the GPU went to the run");
            self.notes.note(serde_json::json!({"event": "load_ahead_yield", "run": caller, "plan": construction, "gpus": gpus,
                "exit_ms": exit_ms, "loading_ms": ms}));
            item.hold_back("a run took the GPU; it loads again once the GPU is idle");
        }
        match result {
            Ok(pid) if level != Level::Gpu => {
                if let Some(session) = self.sessions.lock().unwrap().get_mut(&session_key(&plan.executor, start)) {
                    session.user = Some(item.scope());
                }
                item.state.lock().unwrap().in_ram = Some(pid);
                tracing::info!(run = %caller, "load ahead: its executor and models are built at level ram on GPU {gpus} in {ms} ms, its on-demand {:?} built ahead ({short})",
                    item.predicted.build);
                self.notes.note(serde_json::json!({"event": "load_ahead_ram", "run": caller, "plan": construction, "gpus": gpus,
                    "ms": ms, "need": item.need, "built": item.predicted.build, "free_before": free}));
            }
            Ok(pid) => {
                let held = self.holdings(start, pid).map_or(0, |(held, _)| held);
                if let Some(session) = self.sessions.lock().unwrap().get_mut(&session_key(&plan.executor, start)) {
                    session.user = Some(item.scope());
                }
                item.state.lock().unwrap().loaded = Some((pid, held));
                tracing::info!(run = %caller, "load ahead: its models are on GPU {gpus} in {ms} ms, its on-demand {:?} built ahead ({} free before)",
                    item.predicted.build, free.map_or("?".into(), gib));
                self.notes.note(serde_json::json!({"event": "load_ahead", "run": caller, "plan": construction, "gpus": gpus,
                    "ms": ms, "need": item.need, "held": held, "built": item.predicted.build, "free_before": free}));
            }
            Err(_) if halted.is_some() => {}
            // Built at level `ram`, it stays built: what holds the GPU's memory isn't its to take, so it waits for more room.
            Err(why) if level == Level::Gpu && self.in_ram(item) => {
                tracing::info!(run = %caller, "load ahead: built, its GPU load waits for room ({why}, {} free)", free.map_or("?".into(), gib));
                let mut state = item.state.lock().unwrap();
                state.gpu_short = Some(free.unwrap_or(0));
                state.held_back = format!("built; on its GPU once it has the room ({why})");
            }
            Err(why) => {
                tracing::warn!(run = %caller, "load ahead: {why}");
                self.notes.note(serde_json::json!({"event": "load_ahead_failed", "run": caller, "plan": construction, "why": why}));
                let mut state = item.state.lock().unwrap();
                state.failed = true;
                state.held_back = format!("its load failed: {why}");
            }
        }
        Ok(())
    }

    /// `item` loaded on the lane from `start`, whose call slots the caller holds: its executor started (or the idle one of its
    /// environment there), its models built, mapped at `level`: their weights on the GPU, or (`Host`) level `ram`, where the Runtime
    /// merges their LoRAs. Its executor's pid, or why not.
    fn load_item(self: &Arc<Self>, engine: &Arc<Engine>, item: &Item, start: usize, stop: &Stoppable, level: Level) -> Result<u32, String> {
        let plan = &item.predicted.plan;
        let key = session_key(&plan.construction, start);
        // Loaded and then evicted: its executor holds it still, and maps it again.
        if let Some(entry) = self.levels.lock().unwrap().get_mut(&key) {
            entry.1 = entry.1.min(Level::Host);
        }
        self.prewarm(engine, &item.predicted.held, plan.clone(), start, None, Some(stop)).map_err(|error| error.to_string())?;
        if stop.stop.is_cancelled() {
            return Err("ended".into());
        }
        // Built, its construction stands at `Host`; its map at level `ram` is sent still, as that is where its LoRAs merge.
        if level == Level::Host {
            if let Some(entry) = self.levels.lock().unwrap().get_mut(&key) {
                entry.1 = entry.1.min(Level::Imported);
            }
        }
        let member = Member::new(item.predicted.held.clone(), Some(plan.clone()), level, None, None);
        match self.map_member(engine, plan, start, &member, Some(stop), &item.predicted.build).map_err(|error| error.to_string())? {
            "mapped" => {}
            why => return Err(why.into()),
        }
        let sessions = self.sessions.lock().unwrap();
        sessions.get(&session_key(&plan.executor, start)).map(|session| session.executor.birth.pid).ok_or_else(|| "its executor ended".into())
    }

    /// Whether `item` fits on `lane` at `level` now, else why not. It needs a context on each GPU, and at `Gpu` its weights too:
    /// free there, or, on a card that holds them, what idle executors there hold when every one served its own actor and project
    /// last and holds no listed or warm item, so that what varena takes for it (idle first, oldest use first) is theirs. Another's
    /// model, or another listed item's, is never taken.
    fn room(&self, item: &Item, lane: std::ops::Range<usize>, listed: &BTreeSet<String>, level: Level) -> Result<(), String> {
        let need = match level {
            Level::Gpu => item.need + IDLE_USE,
            _ => IDLE_USE,
        };
        let scope = item.scope();
        let own = &item.predicted.plan.construction;
        let warm: BTreeSet<String> = self.members.lock().unwrap().values().flatten()
            .filter_map(|member| member.plan.as_ref().map(|plan| plan.construction.clone())).collect();
        for device in lane {
            let Some(free) = self.devices.get(device).and_then(|d| d.memory.as_ref()).and_then(|gpu| gpu.free()) else {
                return Err("its GPU's free memory is unknown".into());
            };
            if free >= need {
                continue;
            }
            let sessions = self.sessions.lock().unwrap();
            // Its own executor (built at level `ram`) holds no memory its load could take: only another's models are room.
            let holders: Vec<&Session> = (sessions.values())
                .filter(|session| session.degree != 0 && session.lane().contains(&device) && session.loaded.keys().any(|loaded| loaded != own))
                .collect();
            let theirs = holders.iter().any(|session| session.user.as_ref().is_some_and(|user| *user != scope));
            let kept = holders.iter().any(|session| session.loaded.keys().any(|loaded| loaded != own && (listed.contains(loaded) || warm.contains(loaded))));
            // Every idle one there is the item's to take (an idle executor says what it holds only once asked): what the card
            // holds bounds it.
            let card = self.devices.get(device).and_then(|d| self.inventory(&d.entry)).map_or(0, |gpu| gpu.memory_bytes);
            let short = format!("no spare GPU memory: {} free of the {} it needs", gib(free), gib(need));
            match () {
                () if theirs => return Err(format!("{short}; another project's model holds the rest")),
                () if kept => return Err(format!("{short}; another predicted request or a warm set holds the rest")),
                () if holders.is_empty() || card < need => return Err(short),
                () => {}
            }
        }
        Ok(())
    }
}

/// The load step's one thread: a look at each engine event (a list's run ended, a call came or went, an executor exited).
fn serve(pool: &Weak<Pool>, woken: &Weak<Engine>) {
    loop {
        let (Some(this), Some(engine)) = (pool.upgrade(), woken.upgrade()) else { return };
        if this.stopped.load(Ordering::Acquire) {
            return;
        }
        let epoch = engine.activity_epoch();
        if this.load_next(&engine) {
            engine.notify_activity();
        }
        drop(this);
        engine.wait_activity(epoch, None);
    }
}
