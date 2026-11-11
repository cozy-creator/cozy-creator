//! What a call's weights are prefetched under (one-request-kind §2.7). TensorD chooses each
//! slot's checkpoint, lands its structure and wants its chunks ahead of the call; the Runtime
//! reads them through TensorFS, at the run's Hub under its signer's newest capability when its request
//! carried one.
//! Its checkpoints stay open, so the store keeps their bytes, until
//! the call ends (or its run is canceled): then what it wanted drops. While what it wants is not
//! all here, its run's progress says so (`meter`).
use crate::{
    execution::Engine,
    held_models::Pulled,
    hub,
    journal::Weights,
    origin::{self, priority, Machine, Remote},
    stop::Stop,
    storage::{self, Store},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    sync::{
        atomic::{AtomicI64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

/// Readers of a cold call's weights ahead of its executor: enough requests in flight for an
/// array of disks, as the executor's own reads keep.
const READ_AHEAD_THREADS: usize = 4;
use tensorfs::{
    err::{Code, Refusal},
    format::ObjectRef,
    store::Checkpoint,
};

/// Where `header`, a checkpoint this machine pulled from a Hub (`held_models::Pulled`), comes
/// from: `source`'s Hub. None for one never pulled (local).
pub(crate) fn remote(machine: &Machine, source: &hub::Source, header: &ObjectRef) -> io::Result<Option<Remote>> {
    if Pulled::of(machine.store(), header)?.is_none() {
        return Ok(None);
    }
    origin::remote(machine, source).map(Some).map_err(io::Error::other)
}

/// The checkpoint `id` (`sha256:<hex>`), its structure here.
pub(crate) fn checkpoint(store: &Store, id: &str) -> io::Result<Checkpoint> {
    storage::checkpoint(store, id)
}

/// Whether every chunk of `components` of `header` is here (`downloaded`). Once true it is
/// remembered (Status asks every second) until the store next evicts, in any process.
pub(crate) fn whole(store: &Store, header: &str, components: &[String]) -> bool {
    static WHOLE: Mutex<BTreeMap<String, Option<std::time::SystemTime>>> = Mutex::new(BTreeMap::new());
    let key = format!("{}\0{header}\0{components:?}", store.root().display());
    let evicted = store.evicted_at();
    if WHOLE.lock().unwrap().get(&key).is_some_and(|known| *known == evicted) {
        return true;
    }
    let whole = checkpoint(store, header)
        .is_ok_and(|ck| storage::chunks(&ck, components).iter().all(|o| store.holds(o)));
    match whole {
        true => WHOLE.lock().unwrap().insert(key, evicted),
        false => WHOLE.lock().unwrap().remove(&key),
    };
    whole
}

/// Bytes as GiB to a tenth, for people.
fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

/// A Unix ms instant as its UTC clock time, `HH:MM:SSZ`.
fn clock(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}Z", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

pub(crate) fn failed(refusal: Refusal) -> io::Error {
    let failure = match refusal.code {
        Code::CAPACITY_EXHAUSTED => format!("machine_disk_full: {refusal}"),
        Code::CREDENTIAL_REQUIRED => format!("hub_credential_refused: {}", refusal.detail),
        _ => format!("model_fetch_failed: {refusal}"),
    };
    io::Error::other(failure)
}

/// Only upfront required inputs gate GPU admission; on-demand ones come when a stage reads them.
/// A run waiting says on what (`Fetch::waiting_on`).
#[derive(Clone, Debug)]
pub(crate) enum Ready {
    Yes,
    Waiting(String),
    Failed(String),
}

#[derive(Default)]
struct Required {
    pending: usize,
    failure: Option<String>,
    /// Each required component's want, in `ahead`'s order.
    wants: Vec<RequiredWant>,
}

/// One required component's want: what it is, where its chunks come from, the chunks that were
/// absent when it was wanted, and whether the downloader has answered it.
struct RequiredWant {
    label: String,
    remote: Remote,
    objects: Vec<ObjectRef>,
    settled: bool,
}

/// A component to want: its bytes (the order), its origin, its absent chunks, and its label.
struct Wanted {
    bytes: u64,
    remote: Remote,
    objects: Vec<ObjectRef>,
    label: String,
}

/// One call's fetches: its checkpoints, where each comes from, and what it wants held.
pub(crate) struct Fetch {
    machine: Arc<Machine>,
    /// Where its checkpoints come from.
    hub: Option<hub::Source>,
    /// Its schedules' order: the root run's acceptance, a started run's (`priority::STARTED`) ahead
    /// of them, or after every run (no run).
    order: u64,
    stop: Stop,
    kept: Mutex<Vec<tokio::task::JoinHandle<tensorfs::err::Result<tensorfs::fetch::Landed>>>>,
    /// Its checkpoints, open until it ends: the store keeps their bytes.
    held: Mutex<Vec<Checkpoint>>,
    /// What fetched for this call before it started: ended with it.
    replaced: Mutex<Option<Arc<Fetch>>>,
    /// Each wanted checkpoint not all here when it was wanted.
    arriving: Mutex<Vec<Arriving>>,
    required: Mutex<Required>,
}

/// A checkpoint not all here when it was wanted: its repository and where it comes from, the bytes it had to fetch and those it held
/// already, and its objects still absent.
struct Arriving {
    model: String,
    remote: Remote,
    fetch: u64,
    here: u64,
    absent: Vec<ObjectRef>,
}

/// A repository's checkpoints arriving: the bytes fetched of those to fetch, those that were here, and whether any is still absent.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModelTransfer {
    pub model: String,
    pub done: u64,
    pub total: u64,
    pub here: u64,
    absent: bool,
}

impl Fetch {
    /// Checkpoints fetched from `hub` for a run ordered `order`.
    pub(crate) fn new(machine: Arc<Machine>, hub: Option<hub::Source>, order: u64) -> Arc<Self> {
        Arc::new(Self {
            machine,
            hub,
            order,
            stop: Stop::default(),
            kept: Mutex::default(),
            held: Mutex::default(),
            replaced: Mutex::default(),
            arriving: Mutex::default(),
            required: Mutex::default(),
        })
    }

    fn store(&self) -> &Arc<Store> {
        self.machine.store()
    }

    /// Where the absent objects of `header` come from.
    fn source(&self, header: &ObjectRef) -> io::Result<Option<Remote>> {
        let Some(hub) = &self.hub else {
            return Ok(None);
        };
        remote(&self.machine, hub, header)
    }

    /// `previous` (a run's preparation's fetches) ends with the call.
    pub(crate) fn replaces(&self, previous: Option<Arc<Fetch>>) {
        *self.replaced.lock().unwrap() = previous;
    }

    /// Want each of `selected`'s (header, components) components next, before the call's
    /// executor reads them, in Runtime's Start-pass order: the smallest component first. Held until
    /// the call ends; its run waits on them. Its on-demand models are not: a stage that reads one
    /// fetches it then, and one no stage reads costs nothing (cl-804). For a `cold` call (no
    /// executor holds its construction) the chunks already on disk are read into the page cache in
    /// the same order meanwhile (`read_ahead`). `room` is asked for the bytes absent before any is
    /// wanted: it frees them on the disk or refuses, and a refusal fails the call before it
    /// downloads or starts.
    pub(crate) fn ahead(
        self: &Arc<Self>, selected: &[(String, Vec<String>)], cold: bool,
        engine: &Arc<Engine>, room: &dyn Fn(u64) -> Result<(), String>,
    ) -> io::Result<()> {
        if cold {
            self.read_ahead(selected);
        }
        let mut wanted = match self.components(selected) {
            Ok(wanted) => wanted,
            Err(error) => {
                self.required.lock().unwrap().failure = Some(error.to_string());
                engine.notify_activity();
                return Err(error);
            }
        };
        wanted.sort_by_key(|w| w.bytes);
        let required = wanted.len();
        let missing = wanted.iter().flat_map(|w| &w.objects).map(|o| o.length).sum::<u64>();
        // Its checkpoints are held from `components` on: making room never evicts its own.
        if let Err(refusal) = room(missing) {
            self.required.lock().unwrap().failure = Some(refusal.clone());
            engine.notify_activity();
            return Err(io::Error::other(refusal));
        }
        {
            let mut state = self.required.lock().unwrap();
            state.pending = required;
            state.wants = wanted.iter().map(|w| RequiredWant {
                label: w.label.clone(), remote: w.remote.clone(), objects: w.objects.clone(), settled: false,
            }).collect();
        }
        let mut kept = self.kept.lock().unwrap();
        for (rank, Wanted { remote, objects, .. }) in wanted.into_iter().enumerate() {
            let weak = Arc::downgrade(self);
            let wake = Arc::downgrade(engine);
            let started = self.machine.want_observed(&remote, objects, priority::next(self.order, rank as u32), move |result| {
                if let Some(fetch) = weak.upgrade() {
                    let mut state = fetch.required.lock().unwrap();
                    state.pending = state.pending.saturating_sub(1);
                    if let Some(want) = state.wants.get_mut(rank) {
                        want.settled = true;
                    }
                    if let Err(error) = result {
                        // A full disk says what fills it and the disk that would hold the call.
                        let advice = match error.code == Code::CAPACITY_EXHAUSTED {
                            true => crate::reclaim::advice(fetch.store().root(), missing),
                            false => String::new(),
                        };
                        state.failure.get_or_insert_with(|| failed(error.clone()).to_string() + &advice);
                    }
                }
                if let Some(engine) = wake.upgrade() { engine.notify_activity(); }
            });
            match started {
                Ok(task) => kept.push(task),
                Err(error) => {
                    let error = failed(error);
                    self.required.lock().unwrap().failure = Some(error.to_string());
                    engine.notify_activity();
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// The page cache takes `selected`'s chunks on disk, the smallest component first, while the call's executor forks, imports and opens its GPU: its Start pass and first
    /// phase then read memory, not the disk (a cold Qwen-Image text encoder is 8.4 GB). Read on
    /// threads of their own, a chunk the cache holds skipped, within half of the host's available
    /// memory, ending with the call.
    fn read_ahead(self: &Arc<Self>, selected: &[(String, Vec<String>)]) {
        let store = self.store().clone();
        let ordered = |groups: &[(String, Vec<String>)]| {
            let mut components: Vec<(u64, Vec<ObjectRef>)> = groups.iter()
                .filter_map(|(header, components)| Some((checkpoint(&store, header).ok()?, components)))
                .flat_map(|(checkpoint, components)| components.iter().map(move |component| {
                    let objects = storage::chunks(&checkpoint, std::slice::from_ref(component));
                    (objects.iter().map(|o| o.length).sum(), objects)
                }).collect::<Vec<_>>())
                .collect();
            components.sort_by_key(|(bytes, _)| *bytes);
            components.into_iter().flat_map(|(_, objects)| objects).collect::<Vec<_>>()
        };
        let chunks: Vec<ObjectRef> = ordered(selected).into_iter().filter(|o| store.holds(o)).collect();
        if chunks.is_empty() {
            return;
        }
        let room = Arc::new(AtomicI64::new(varena_ledger::host::budget().cache()));
        let (chunks, next) = (Arc::new(chunks), Arc::new(AtomicUsize::new(0)));
        for reader in 0..READ_AHEAD_THREADS {
            let (store, chunks, next, room, call) = (store.clone(), chunks.clone(), next.clone(), room.clone(), Arc::downgrade(self));
            let started = std::thread::Builder::new().name(format!("read-ahead-{reader}")).spawn(move || {
                let mut buf = vec![0u8; 8 << 20];
                while let Some(object) = chunks.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if call.upgrade().is_none_or(|call| call.stop.is_cancelled()) {
                        return;
                    }
                    let Some(mut file) = store.path(object).ok().and_then(|path| File::open(path).ok()) else { continue };
                    if platform::durable::cached(&file, 0, object.length) == Some(true) {
                        continue;
                    }
                    if room.fetch_sub(i64::try_from(object.length).unwrap_or(i64::MAX), Ordering::Relaxed) < 0 {
                        return;
                    }
                    while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
                }
            });
            if let Err(error) = started {
                tracing::warn!("reading a call's weights ahead: {error}");
            }
        }
    }

    pub(crate) fn ready(&self) -> Ready {
        let failure = {
            let state = self.required.lock().unwrap();
            if state.pending == 0 && state.failure.is_none() {
                return Ready::Yes;
            }
            state.failure.clone()
        };
        match failure {
            Some(error) => Ready::Failed(error),
            None => Ready::Waiting(self.waiting_on().unwrap_or_default()),
        }
    }

    /// What its run waits on while a required component's want is unanswered, for people: per
    /// component, its chunks still absent and where each one's download is (`Phase`), with its bytes
    /// so far and when it last moved; or that every chunk is on disk but its want has not been
    /// answered (it is not bound yet). None once nothing is waited on.
    pub(crate) fn waiting_on(&self) -> Option<String> {
        let wants: Vec<(String, Remote, Vec<ObjectRef>)> = self.required.lock().unwrap().wants.iter()
            .filter(|w| !w.settled).map(|w| (w.label.clone(), w.remote.clone(), w.objects.clone())).collect();
        if wants.is_empty() {
            return None;
        }
        let each: Vec<String> = wants.into_iter().map(|(label, remote, objects)| {
            let absent: Vec<ObjectRef> = objects.iter().filter(|o| !self.store().holds(o)).cloned().collect();
            if absent.is_empty() {
                return format!("{label}: all {} chunks on disk, not yet bound (its want is unanswered)", objects.len());
            }
            let states = self.machine.fetching(&remote, &absent);
            let mut by_phase: Vec<(String, usize)> = vec![];
            let (mut reached, mut length, mut moved, mut unanswered) = (0u64, 0u64, 0u64, 0);
            for (object, state) in absent.iter().zip(&states) {
                let phase = state.map_or_else(|| "not yet with the downloader".to_owned(), |s| s.phase.to_string());
                match by_phase.iter_mut().find(|(p, _)| *p == phase) {
                    Some((_, n)) => *n += 1,
                    None => by_phase.push((phase, 1)),
                }
                reached += state.map_or(0, |s| s.reached);
                length += object.length;
                moved = moved.max(state.map_or(0, |s| s.moved_unix_ms));
                unanswered = unanswered.max(state.map_or(0, |s| s.unanswered));
            }
            let phases: Vec<String> = by_phase.iter().map(|(p, n)| format!("{n} {p}")).collect();
            let last = if moved == 0 { String::new() } else { format!(", last moved {}", clock(moved)) };
            let attempt = match unanswered {
                0 => String::new(),
                unanswered => format!(", {unanswered} attempts in a row moved nothing (of {})", tensorfs::fetch::ATTEMPTS),
            };
            format!("{label}: {} of {} chunks to fetch, {} of their {} received ({}){last}{attempt}",
                absent.len(), objects.len(), gib(reached), gib(length), phases.join(", "))
        }).collect();
        Some(each.join("; "))
    }
    /// Each component of `selected` (header, components) from a Hub: its bytes, origin and
    /// chunks, its checkpoint held until the call ends. What is absent now is what its run waits
    /// on (`meter`), under the repository it came from.
    fn components(&self, selected: &[(String, Vec<String>)]) -> io::Result<Vec<Wanted>> {
        let mut wanted = vec![];
        for (header, components) in selected {
            let checkpoint = checkpoint(self.store(), header)?;
            let Some(remote) = self.source(checkpoint.id())? else {
                if storage::chunks(&checkpoint, components).iter().any(|o| !self.store().holds(o)) {
                    return Err(io::Error::other("required local checkpoint input is missing and has no fetch source"));
                }
                // Retain local checkpoints too, so GC cannot remove required inputs between
                // readiness and an executor's own source lease.
                self.held.lock().unwrap().push(checkpoint);
                continue;
            };
            // A checkpoint known whole is looked at no more: nothing of it is wanted.
            if components.is_empty() || whole(self.store(), header, components) {
                self.held.lock().unwrap().push(checkpoint);
                continue;
            }
            let pulled = Pulled::of(self.store(), checkpoint.id())?;
            let model = pulled.as_ref().map_or_else(|| header.clone(), |p| p.model.clone());
            let (mut absent_all, mut here_all) = (vec![], 0);
            for component in components {
                let objects = storage::chunks(&checkpoint, std::slice::from_ref(component));
                let bytes: u64 = objects.iter().map(|o| o.length).sum();
                // Only absent chunks are wanted: a chunk on disk needs no download, and a component
                // whole here needs no want at all, so its run never waits on the downloader for it.
                let absent: Vec<ObjectRef> = objects.into_iter().filter(|o| !self.store().holds(o)).collect();
                here_all += bytes - absent.iter().map(|o| o.length).sum::<u64>();
                if absent.is_empty() {
                    continue;
                }
                absent_all.extend(absent.iter().cloned());
                wanted.push(Wanted { bytes, remote: remote.clone(), objects: absent, label: format!("{model} {component}") });
            }
            if let (false, Some(pulled)) = (absent_all.is_empty(), pulled) {
                // What it has to fetch, never what was here already (shared with another model, or an earlier pull's).
                let fetch = absent_all.iter().map(|o| o.length).sum();
                self.arriving.lock().unwrap().push(Arriving { model: pulled.model, remote: remote.clone(), fetch, here: here_all, absent: absent_all });
            }
            self.held.lock().unwrap().push(checkpoint);
        }
        Ok(wanted)
    }

    /// Want `components` of each of `selected` (header, components) behind everything, held
    /// until the call ends: a warm set member's `downloaded`.
    pub(crate) fn keep(&self, selected: &[(String, Vec<String>)]) -> io::Result<()> {
        let mut kept = self.kept.lock().unwrap();
        if !kept.is_empty() {
            return Ok(());
        }
        for (header, components) in selected {
            let checkpoint = checkpoint(self.store(), header)?;
            if let Some(remote) = self.source(checkpoint.id())? {
                let objects = storage::chunks(&checkpoint, components);
                kept.push(self.machine.want(&remote, objects, priority::BEHIND).map_err(failed)?);
            }
            self.held.lock().unwrap().push(checkpoint);
        }
        Ok(())
    }

    /// The bytes of `selected` (header, components) not here.
    pub(crate) fn absent(&self, selected: &[(String, Vec<String>)]) -> io::Result<u64> {
        let mut absent = 0;
        for (header, components) in selected {
            let checkpoint = checkpoint(self.store(), header)?;
            let objects = storage::chunks(&checkpoint, components);
            absent += objects.iter().filter(|object| !self.store().holds(object)).map(|object| object.length).sum::<u64>();
        }
        Ok(absent)
    }

    /// Every chunk of `selected` (header, components) here, reporting its bytes (a warm run
    /// of a function): those that came from the Hub and from the datacenter volume, and those held.
    pub(crate) fn download(&self, selected: &[(String, Vec<String>)], stop: &Stop, progress: &(dyn Fn(u64, u64) + Sync)) -> io::Result<tensorfs::fetch::Landed> {
        let counted = Arc::new(tensorfs::fetch::Progress::default());
        origin::watched(&counted, progress, || {
            let mut landed = tensorfs::fetch::Landed::default();
            for (header, components) in selected {
                let held = storage::held(self.store(), header)?;
                let Some(remote) = self.source(&held)? else {
                    continue;
                };
                let components = (!components.is_empty()).then_some(components.as_slice());
                let pulled = self.machine.pull(&remote, &held, components, priority::BEHIND, Some(counted.clone()), stop).map_err(failed)?;
                (landed.origin, landed.volume, landed.local) =
                    (landed.origin + pulled.landed.origin, landed.volume + pulled.landed.volume, landed.local + pulled.landed.local);
            }
            Ok(landed)
        })
    }

    /// The weights its run waits on that are not all here yet, while any are not; and each
    /// repository of them.
    pub(crate) fn arriving(&self) -> (Option<Weights>, Vec<ModelTransfer>) {
        let waited = self.each(&self.arriving);
        (counted(&waited), waited.into_iter().filter(|each| each.absent).collect())
    }

    /// Each of `list`'s repositories, its bytes fetched (an object still arriving counted to the byte it reached) of those it
    /// found absent. An object here stays: its checkpoint is held.
    fn each(&self, list: &Mutex<Vec<Arriving>>) -> Vec<ModelTransfer> {
        let mut arriving = list.lock().unwrap();
        let mut each: Vec<ModelTransfer> = vec![];
        for Arriving { model, remote, fetch, here, absent } in arriving.iter_mut() {
            // Each download as it stands, read before the store: one landing meanwhile counts whole.
            let mut states = self.machine.fetching(remote, absent).into_iter();
            let mut missing = 0;
            absent.retain(|object| {
                let reached = states.next().flatten().map_or(0, |state| state.reached);
                let held = self.store().holds(object);
                if !held {
                    missing += object.length - reached.min(object.length);
                }
                !held
            });
            let at = match each.iter().position(|seen| seen.model == *model) {
                Some(at) => at,
                None => {
                    each.push(ModelTransfer { model: model.clone(), ..ModelTransfer::default() });
                    each.len() - 1
                }
            };
            let row = &mut each[at];
            (row.done, row.total, row.here) = (row.done + *fetch - missing, row.total + *fetch, row.here + *here);
            row.absent |= !absent.is_empty();
        }
        each
    }

    /// Whether it ended (`end`).
    pub(crate) fn ended(&self) -> bool {
        self.stop.is_cancelled()
    }

    /// Everything it wanted drops and its checkpoints close.
    pub(crate) fn end(&self) {
        self.stop.cancel();
        for want in self.kept.lock().unwrap().drain(..) {
            want.abort();
        }
        self.held.lock().unwrap().clear();
        if let Some(replaced) = self.replaced.lock().unwrap().take() {
            replaced.end();
        }
    }
}

/// The repositories of `each` not all here, and the bytes fetched of all they had to fetch.
fn counted(each: &[ModelTransfer]) -> Option<Weights> {
    let models: Vec<&str> = each.iter().filter(|each| each.absent).map(|each| each.model.as_str()).collect();
    let (done, total) = each.iter().fold((0, 0), |(done, total), each| (done + each.done, total + each.total));
    (!models.is_empty()).then(|| Weights { models: models.join(" and "), done, total })
}

/// While what `fetch` wants is not all here, run `id`'s progress says which weights it waits on
/// and how many of their bytes are here, from a quarter-second look at the store; once they are,
/// or the fetch ends, its own stage shows again. While a required want is unanswered, the run's
/// waiting reason says on what (`Fetch::waiting_on`), and TensorD's log says so each time where
/// its chunks are changes. A run whose weights are here hears nothing.
pub(crate) fn meter(engine: Arc<Engine>, id: String, fetch: &Arc<Fetch>) {
    if fetch.arriving.lock().unwrap().is_empty() && fetch.required.lock().unwrap().pending == 0 {
        return;
    }
    let (store, fetch, began) = (fetch.store().clone(), Arc::downgrade(fetch), std::time::Instant::now());
    let began_ms = crate::machine::lifecycle::now_ms() as u64;
    let started = std::thread::Builder::new().name("weights-meter".into()).spawn(move || {
        let (mut last, mut said, mut shown) = (None, String::new(), None::<String>);
        // What its run waits on, as this machine lists it (`activity`), by repository.
        let caller = engine.caller_id(&id);
        let mut waits: std::collections::HashMap<String, crate::activity::Waiting> = Default::default();
        loop {
            let live = fetch.upgrade().filter(|f| !f.ended());
            let (weights, waited) = live.as_ref().map(|f| f.arriving()).unwrap_or_default();
            waits.retain(|model, _| waited.iter().any(|each| each.model == *model));
            for each in &waited {
                let wait = waits.entry(each.model.clone()).or_insert_with(|| crate::activity::wait(crate::activity::Kind::Download, &each.model, &caller));
                wait.here(each.here);
                wait.moved("", each.done, each.total);
            }
            if let Err(error) = engine.observe_weights(&id, weights.clone()) {
                tracing::warn!("run {id}: its weights' progress: {error}");
                return;
            }
            let waiting = live.as_ref().and_then(|f| f.waiting_on());
            if waiting != shown {
                if let Some(on) = &waiting {
                    // The log line's state is its phases and counts: bytes and times change it not.
                    let state = phases(on);
                    if state != said {
                        tracing::info!("run {id}: waiting for required model inputs: {on}");
                        said = state;
                    }
                    if let Err(error) = engine.waiting(&id, Some(format!("waiting for required model inputs: {on}"))) {
                        tracing::warn!("run {id}: what it waits on: {error}");
                    }
                }
                shown = waiting.clone();
            }
            // What the run waited for lands in the transport log, as a pull's does.
            if weights.is_none() && waiting.is_none() {
                if let (Some(Weights { models, total, .. }), true) = (last, live.is_some()) {
                    let waited = format!("bytes={total} seconds={:.1} run={id}", began.elapsed().as_secs_f64());
                    storage::transport(&store, "arrived", &models, &waited);
                    // Its time waiting on them is no part of its compute (`journal::Ran`).
                    if let Err(error) = engine.fetched(&id, began_ms, crate::machine::lifecycle::now_ms() as u64) {
                        tracing::warn!("run {id}: when its weights arrived: {error}");
                    }
                }
                return;
            }
            if engine.get(&id).is_ok_and(|record| record.state.terminal()) {
                return;
            }
            drop(live);
            if weights.is_some() {
                last = weights;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
    if let Err(error) = started {
        tracing::warn!("its weights' progress: {error}");
    }
}

/// `waiting_on`'s text as a state: each component's label and the phases its chunks are in (not
/// their counts, bytes or times), or that it is on disk but not bound. The log says each change.
fn phases(waiting: &str) -> String {
    waiting.split("; ").map(|part| {
        let label = part.split(": ").next().unwrap_or(part);
        if part.contains("not yet bound") {
            return format!("{label}: on disk, not yet bound");
        }
        let inside = part.rsplit_once(" (").map_or("", |(_, t)| t.split(')').next().unwrap_or(""));
        let mut names: Vec<&str> = inside.split(", ").map(|p| p.split_once(' ').map_or(p, |(_, name)| name)).collect();
        names.sort_unstable();
        names.dedup();
        format!("{label}: {}", names.join(", "))
    }).collect::<Vec<_>>().join("; ")
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.end();
    }
}
