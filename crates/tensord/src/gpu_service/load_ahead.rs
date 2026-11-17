//! A predicted request's load (cl-1123, Paul 11-17: "if the target machine GPU is idle, like load the model weights into memory").
//! Once a list item's run has brought its weights here (`crate::ahead`), and while no call wants a GPU, its executor starts, builds
//! the call's models and puts their weights on a GPU, as a call of it would (the same plan, lane and Load), so that call's first
//! step loads nothing: it takes the executor the load left idle.
//!
//! Only on a dedicated machine (a rental, or a computer its owner gave to Cozy: `machine.mode`), one load at a time, and only into
//! room no one else's model holds: free memory, or what idle executors of the same actor and project hold that no listed item
//! loaded, which varena's own order then takes. A real call yields it at once (`yield_to`): a load of another model ends, its
//! executor with it; a load of the same model goes on, and the call takes what it made. A list that drops an item loading lets the
//! load finish, as a render of it may have just been asked. It is no run: it never counts as a job, so a rental's idle clock never
//! sees it.
use super::*;
use std::collections::BTreeSet;

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
    /// Once loaded: its executor, and what that held on its first GPU then.
    loaded: Option<(u32, u64)>,
    /// Its load failed: it isn't tried again until it comes anew.
    failed: bool,
}

struct Loading {
    actor: String,
    caller: String,
    construction: String,
    stop: Arc<Stoppable>,
    /// When a real call ended it.
    halted: Option<Instant>,
}

impl Item {
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

/// Its executor shares the GPU through varena's ledger (TensorFS 0.21 on): one older than that sizes itself by what is free, and a
/// call of it ends every idle executor first (`room_outside_ledger`), which a guess never does.
fn ledgered(held: &HeldGeneration) -> bool {
    held.record.dependencies.iter().find(|dependency| dependency.name == "tensorfs").is_none_or(|tensorfs| in_ledger(&tensorfs.version))
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
        let need = predicted.plan.slots.iter().filter(|slot| !slot.on_demand).map(|slot| slot.selected_encoded_bytes).sum();
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

    /// A predicted request is loading now.
    pub(crate) fn loading_ahead(&self) -> bool {
        self.ahead.loading.lock().unwrap().is_some()
    }

    /// The calls left waiting for a GPU this dispatch pass, by construction: a load of any other ends now; one of theirs goes on,
    /// and the call takes its executor.
    pub(crate) fn yield_to(&self, waiting: &[String]) {
        let stop = {
            let mut loading = self.ahead.loading.lock().unwrap();
            let other = |loading: &&mut Loading| loading.halted.is_none() && waiting.iter().any(|construction| *construction != loading.construction);
            let Some(loading) = loading.as_mut().filter(other) else { return };
            loading.halted = Some(Instant::now());
            loading.stop.clone()
        };
        stop.halt();
    }

    /// How ready the machine is for `actor`'s item `caller` beyond its weights: `loading`, `loaded` or `evicted`; else `ready`
    /// with why it isn't loading, when it was looked at. None: no load step for it.
    pub(crate) fn readiness(&self, actor: &str, caller: &str) -> Option<api::machine::Readiness> {
        let item = self.ahead.items.lock().unwrap().iter().find(|item| item.predicted.actor == actor && item.predicted.caller == caller).cloned()?;
        let at = |stage: &str, reason: String| Some(api::machine::Readiness { stage: stage.into(), total: item.need, reason, ..Default::default() });
        if self.ahead.loading.lock().unwrap().as_ref().is_some_and(|loading| loading.actor == actor && loading.caller == caller) {
            return at("loading", String::new());
        }
        let resident = self.resident(&item);
        let state = item.state.lock().unwrap();
        match (resident, state.loaded.is_some()) {
            (Some(true), _) => at("loaded", String::new()).map(|loaded| api::machine::Readiness { done: item.need, ..loaded }),
            (Some(false), _) => at("evicted", "a run took its GPU memory; it loads again once the GPU is idle".into()),
            (None, true) => at("evicted", "its executor ended for a run's room; it loads again once the GPU is idle".into()),
            (None, false) if !state.held_back.is_empty() => at("ready", state.held_back.clone()),
            (None, false) => None,
        }
    }

    /// Whether `item`'s models are on a GPU (an executor holds its construction there): None if not; false once a run took that
    /// memory from the executor its load left idle, as varena's ledger shows.
    fn resident(&self, item: &Item) -> Option<bool> {
        let construction = &item.predicted.plan.construction;
        let (start, pid) = {
            let levels = self.levels.lock().unwrap();
            let (key, (_, _, birth)) = levels.iter().find(|(key, (_, level, _))| key.split('@').next() == Some(construction.as_str()) && *level == Level::Gpu)?;
            (key.split_once('@').and_then(|(_, start)| start.parse().ok()).unwrap_or(0usize), birth.pid)
        };
        let Some((loaded, held)) = item.state.lock().unwrap().loaded else { return Some(true) };
        // A call or a warm set made it, or a call has it now: the pool's own account stands.
        let idle = self.sessions.lock().unwrap().values().any(|session| session.executor.birth.pid == pid);
        if loaded != pid || !idle {
            return Some(true);
        }
        Some(self.holdings(start, pid).is_none_or(|(now, _)| now.saturating_mul(10) >= held.saturating_mul(9)))
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
            if self.resident(item) == Some(true) {
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
        changed
    }

    /// `item` loaded now, or why not and whether that's the GPU being in use.
    fn try_load(self: &Arc<Self>, engine: &Arc<Engine>, item: &Arc<Item>, listed: &BTreeSet<String>) -> Result<(), (String, bool)> {
        let plan = &item.predicted.plan;
        if !ledgered(&item.predicted.held) {
            return Err(("its package's TensorFS predates varena's GPU ledger".into(), false));
        }
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
        let mut starts: Vec<usize> = self.hosting(plan).into_iter().map(|(start, _)| start).collect();
        starts.extend((0..(self.width() + 1).saturating_sub(degree)).filter(|start| !starts.contains(start)));
        let mut short = None;
        let fits: Vec<usize> = starts.into_iter().filter(|&start| match self.room(item, start..start + degree, listed) {
            Ok(()) => true,
            Err(why) => {
                short.get_or_insert(why);
                false
            }
        }).collect();
        if fits.is_empty() {
            return Err((short.unwrap_or_else(|| "no GPU here serves it".into()), false));
        }
        // In flight before it takes a GPU, so a call queued meanwhile ends it (`yield_to`).
        let stop = Arc::new(engine.stoppable(&format!("load-ahead:{}:{}", item.predicted.actor, item.predicted.caller)));
        *self.ahead.loading.lock().unwrap() = Some(Loading {
            actor: item.predicted.actor.clone(),
            caller: item.predicted.caller.clone(),
            construction: plan.construction.clone(),
            stop: stop.clone(),
            halted: None,
        });
        let placed = fits.iter().find_map(|&start| self.place(plan, (None, None), Some(start..start + degree), &mut vec![None; self.width()]));
        let Some((start, permit)) = placed.filter(|_| !stop.stop.is_cancelled()) else {
            self.ahead.loading.lock().unwrap().take();
            return Err(("a run holds the GPU".into(), true));
        };
        let lane = start..start + degree;
        item.hold_back("");
        engine.notify_activity();
        let began = Instant::now();
        let free = self.free(&lane);
        let result = self.load_item(engine, item, start, &stop);
        drop(permit);
        let halted = self.ahead.loading.lock().unwrap().take().and_then(|loading| loading.halted);
        drop(stop);
        let (caller, construction) = (&item.predicted.caller, &plan.construction);
        let gpus = self.lane_devices(start, plan.degree).unwrap_or_default();
        let ms = began.elapsed().as_millis() as u64;
        if let Some(halted) = halted {
            // From the halt to its GPU back: what a real call waited on it.
            let yield_ms = halted.elapsed().as_millis() as u64;
            tracing::info!(run = %caller, "load ahead: ended for a run, GPU {gpus} free {yield_ms} ms later ({ms} ms after it began)");
            self.notes.note(serde_json::json!({"event": "load_ahead_yield", "run": caller, "plan": construction, "gpus": gpus,
                "yield_ms": yield_ms, "loading_ms": ms}));
            item.hold_back("a run took the GPU; it loads again once the GPU is idle");
        }
        match result {
            Ok(pid) => {
                let held = self.holdings(start, pid).map_or(0, |(held, _)| held);
                if let Some(session) = self.sessions.lock().unwrap().get_mut(&session_key(&plan.executor, start)) {
                    session.user = Some(item.scope());
                }
                item.state.lock().unwrap().loaded = Some((pid, held));
                tracing::info!(run = %caller, "load ahead: its models are on GPU {gpus} in {ms} ms ({} held, {} free before)",
                    gib(held), free.map_or("?".into(), gib));
                self.notes.note(serde_json::json!({"event": "load_ahead", "run": caller, "plan": construction, "gpus": gpus,
                    "ms": ms, "need": item.need, "held": held, "free_before": free}));
            }
            Err(_) if halted.is_some() => {}
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
    /// environment there), its models built, their weights on the GPU. Its executor's pid, or why not.
    fn load_item(self: &Arc<Self>, engine: &Arc<Engine>, item: &Item, start: usize, stop: &Stoppable) -> Result<u32, String> {
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
        let member = Member::new(item.predicted.held.clone(), Some(plan.clone()), Level::Gpu, None, None);
        match self.map_member(engine, plan, start, &member, Some(stop)).map_err(|error| error.to_string())? {
            "mapped" => {}
            why => return Err(why.into()),
        }
        let sessions = self.sessions.lock().unwrap();
        sessions.get(&session_key(&plan.executor, start)).map(|session| session.executor.birth.pid).ok_or_else(|| "its executor ended".into())
    }

    /// Whether `item` fits on `lane` now, else why not. It needs its weights and a context on each GPU: free there, or what idle
    /// executors there hold when every one served its own actor and project last and holds no listed item, so that what varena
    /// takes for it (idle first, oldest use first) is theirs. Another's model, or another listed item's, is never taken.
    fn room(&self, item: &Item, lane: std::ops::Range<usize>, listed: &BTreeSet<String>) -> Result<(), String> {
        let need = item.need + IDLE_USE;
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
            let holders: Vec<&Session> = sessions.values().filter(|session| session.degree != 0 && session.lane().contains(&device)).collect();
            let theirs = holders.iter().any(|session| session.user.as_ref().is_some_and(|user| *user != scope));
            let kept = holders.iter().any(|session| session.loaded.keys().any(|loaded| loaded != own && (listed.contains(loaded) || warm.contains(loaded))));
            let reclaimable: u64 = holders.iter()
                .flat_map(|session| executor_births(&session.executor.birth, &session.followers))
                .filter_map(|birth| self.holdings(device, birth.pid)).map(|(held, _)| held).sum();
            let short = format!("no spare GPU memory: {} free of the {} it needs", gib(free), gib(need));
            match () {
                () if theirs => return Err(format!("{short}; another project's model holds the rest")),
                () if kept => return Err(format!("{short}; another predicted request or a warm set holds the rest")),
                () if free + reclaimable < need => return Err(short),
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
