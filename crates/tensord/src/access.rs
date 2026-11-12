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
    collections::{BTreeMap, HashMap, HashSet},
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

/// What a function's run reads, by how sure (cl-905): `sure`, what its calls read up front (an
/// orchestrator's: what all its internal functions do); then `guesses`, each internal function's
/// own by name, in its package's order: which of them it calls only it knows.
#[derive(Default)]
pub(crate) struct Reads {
    pub sure: Selected,
    pub guesses: Vec<(String, Selected)>,
}

/// Checkpoints by header, each with the components read (all, when none).
pub(crate) type Selected = Vec<(String, Vec<String>)>;

/// `Reads` in the machine's download line (`Fetch::lined`): each sure and each guessed component's
/// order and chunks, the guesses' checkpoints, the bytes of the sure ones absent here, and each
/// guess held back, in words.
#[derive(Default)]
pub(crate) struct Lined {
    wants: Vec<(u64, Lining)>,
    guessed: Vec<(u64, Lining)>,
    guesses: Vec<Checkpoint>,
    pub sure: u64,
    pub held_back: Vec<String>,
}

/// Every fetch guessing now (`Fetch::guess`), for a need that has to make room to drop them all.
static GUESSING: Mutex<Vec<std::sync::Weak<Fetch>>> = Mutex::new(Vec::new());

/// Every guess on this machine ends: its downloads stop and its checkpoints may go. A run's need
/// that has to make room comes before any guess (cl-905).
pub(crate) fn drop_guesses() {
    let guessing = std::mem::take(&mut *GUESSING.lock().unwrap());
    for fetch in guessing.iter().filter_map(std::sync::Weak::upgrade) {
        fetch.drop_guessed();
    }
}

/// A guess's downloads, which end without failing what waits on them, its checkpoints, and what of
/// it the machine lists (`activity`).
#[derive(Default)]
struct Guessed {
    stops: Vec<tokio::task::AbortHandle>,
    tasks: Vec<tokio::task::JoinHandle<tensorfs::err::Result<tensorfs::fetch::Landed>>>,
    held: Vec<Checkpoint>,
    listed: Mutex<Vec<Arriving>>,
}

/// One component in line: its bytes, origin, repository, chunks, and those absent.
struct Lining {
    bytes: u64,
    remote: Remote,
    model: String,
    objects: Vec<ObjectRef>,
    absent: Vec<ObjectRef>,
}

impl Lining {
    /// It arriving, for the machine's list.
    fn arriving(self) -> Arriving {
        let fetch = self.absent.iter().map(|o| o.length).sum::<u64>();
        Arriving { model: self.model, remote: self.remote, fetch, here: self.bytes - fetch, absent: self.absent }
    }
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
    /// What it wants that its run doesn't wait on (`hint`), not all here when it was wanted.
    hinted: Mutex<Vec<Arriving>>,
    guessed: Mutex<Guessed>,
    /// A thread lists what it hinted while any is absent.
    watching: std::sync::atomic::AtomicBool,
    required: Mutex<Required>,
    /// What its call builds on demand that no other process has begun reading (`on_demand`).
    demand: Mutex<Vec<OnDemand>>,
}

/// A model a call builds on demand: its checkpoint and the components the call reads; their chunks once looked up
/// (`Fetch::promote`), never on the call's way to its executor.
struct OnDemand {
    header: String,
    components: Vec<String>,
    chunks: Option<Vec<ObjectRef>>,
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
            hinted: Mutex::default(),
            guessed: Mutex::default(),
            watching: Default::default(),
            required: Mutex::default(),
            demand: Mutex::default(),
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
            let (mut reached, mut length, mut moved, mut unanswered, mut hedged) = (0u64, 0u64, 0u64, 0, 0);
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
                hedged += state.map_or(0, |s| s.hedged);
            }
            let phases: Vec<String> = by_phase.iter().map(|(p, n)| format!("{n} {p}")).collect();
            let last = if moved == 0 { String::new() } else { format!(", last moved {}", clock(moved)) };
            let attempt = match unanswered {
                0 => String::new(),
                unanswered => format!(", {unanswered} attempts in a row moved nothing (of {})", tensorfs::fetch::ATTEMPTS),
            };
            let hedges = match hedged {
                0 => String::new(),
                1 => ", 1 slow request asked again on another connection".to_owned(),
                hedged => format!(", {hedged} slow requests asked again on another connection"),
            };
            format!("{label}: {} of {} chunks to fetch, {} of their {} received ({}){last}{attempt}{hedges}",
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

    /// `reads` in the machine's download line, each checkpoint held until the fetch ends: its sure
    /// components at `sure(rank)`, smallest first as a Start pass reads them; then each guess's at
    /// `guess(rank)`, in turn, unless its bytes absent don't fit in `free` beside all before it (a
    /// guess never makes room; a wrong one costs bytes only). A component twice comes once.
    pub(crate) fn lined(&self, reads: &Reads, sure: &dyn Fn(u32) -> u64, guess: &dyn Fn(u32) -> u64, free: u64) -> io::Result<Lined> {
        let (mut lined, mut seen, mut counted, mut taken, mut rank) = (Lined::default(), HashSet::new(), HashSet::new(), 0u64, 0u32);
        // A guess's checkpoints stay held only while it does (`guess`): they may go when a need makes room.
        let sets = std::iter::once((None, &reads.sure)).chain(reads.guesses.iter().map(|(name, set)| (Some(name.as_str()), set)));
        for (guessed, set) in sets {
            let (mut parts, mut held) = (vec![], vec![]);
            for (header, components) in set {
                let checkpoint = checkpoint(self.store(), header)?;
                if let Some(remote) = self.source(checkpoint.id())? {
                    let model = Pulled::of(self.store(), checkpoint.id())?.map_or_else(|| header.clone(), |p| p.model);
                    let each: Vec<Vec<String>> = match components.is_empty() {
                        true => vec![vec![]],
                        false => components.iter().map(|component| vec![component.clone()]).collect(),
                    };
                    for component in each.into_iter().filter(|component| seen.insert((header.clone(), component.clone()))) {
                        let objects = storage::chunks(&checkpoint, &component);
                        let absent = objects.iter().filter(|o| !self.store().holds(o)).cloned().collect();
                        let bytes = objects.iter().map(|o| o.length).sum();
                        parts.push(Lining { bytes, remote: remote.clone(), model: model.clone(), objects, absent });
                    }
                }
                held.push(checkpoint);
            }
            parts.sort_by_key(|part| part.bytes);
            let mut new = HashSet::new();
            let absent: u64 = (parts.iter().flat_map(|part| &part.absent))
                .filter(|o| !counted.contains(&o.sha256) && new.insert(o.sha256.clone()))
                .map(|o| o.length)
                .sum();
            if let Some(name) = guessed.filter(|_| taken + absent > free) {
                lined.held_back.push(format!("{name}'s weights ({}) aren't fetched ahead: {} free beside what comes first", gib(absent), gib(free.saturating_sub(taken))));
                continue;
            }
            taken += absent;
            counted.extend(new);
            let (wants, order): (&mut Vec<_>, &dyn Fn(u32) -> u64) = match guessed {
                None => {
                    lined.sure = absent;
                    self.held.lock().unwrap().extend(held);
                    (&mut lined.wants, sure)
                }
                Some(_) => {
                    lined.guesses.extend(held);
                    (&mut lined.guessed, guess)
                }
            };
            for part in parts {
                wants.push((order(rank), part));
                rank += 1;
            }
        }
        Ok(lined)
    }

    /// Every sure component `lined` holds here, each at its order, reporting its bytes (a warm run of
    /// a function): those that came from the Hub and from the datacenter volume, and those held. Its
    /// guesses are wanted beside them and waited for until they land or are dropped (`guess`); the
    /// machine lists them for run `caller`.
    pub(crate) fn download(self: &Arc<Self>, mut lined: Lined, caller: &str, stop: &Stop, progress: &(dyn Fn(u64, u64) + Sync)) -> io::Result<tensorfs::fetch::Landed> {
        self.guess(std::mem::take(&mut lined.guessed), std::mem::take(&mut lined.guesses), caller)?;
        let counted = Arc::new(tensorfs::fetch::Progress::default());
        let wants = lined.wants.into_iter().map(|(order, part)| (part.remote, part.objects, order)).collect();
        let landed = origin::watched(&counted, progress, || self.machine.all(wants, counted.clone(), stop).map_err(failed))?;
        let tasks = std::mem::take(&mut self.guessed.lock().unwrap().tasks);
        // A dropped guess ends its task: that ends no wait here.
        self.machine.block(stop, async move {
            for task in tasks {
                let _ = task.await;
            }
            Ok(())
        }).map_err(failed)?;
        Ok(landed)
    }

    /// Wants what `lined` holds absent here, each at its order, none of which its run waits on: an
    /// orchestrator's internal functions' weights, while its root starts. The sure ones are held until
    /// the fetch ends, its guesses until they are dropped (`guess`). While any is absent the machine
    /// lists their download for run `caller` (`activity`).
    pub(crate) fn hint(self: &Arc<Self>, mut lined: Lined, caller: &str) -> io::Result<()> {
        self.guess(std::mem::take(&mut lined.guessed), std::mem::take(&mut lined.guesses), caller)?;
        let mut kept = self.kept.lock().unwrap();
        if self.ended() {
            return Ok(());
        }
        for (order, part) in lined.wants.into_iter().filter(|(_, part)| !part.absent.is_empty()) {
            kept.push(self.machine.want(&part.remote, part.absent.clone(), order).map_err(failed)?);
            self.hinted.lock().unwrap().push(part.arriving());
        }
        drop(kept);
        self.watch(caller)
    }

    /// Wants `parts` (guesses, at their orders) and holds their `checkpoints` until the fetch ends or
    /// the guesses are dropped: by its run's first call, which says which function it is, or for any
    /// need that has to make room (`drop_guesses`). A guess never makes room itself.
    fn guess(self: &Arc<Self>, parts: Vec<(u64, Lining)>, checkpoints: Vec<Checkpoint>, caller: &str) -> io::Result<()> {
        let mut guessed = self.guessed.lock().unwrap();
        if self.ended() || parts.iter().all(|(_, part)| part.absent.is_empty()) {
            return Ok(());
        }
        guessed.held.extend(checkpoints);
        for (order, part) in parts.into_iter().filter(|(_, part)| !part.absent.is_empty()) {
            let task = self.machine.want(&part.remote, part.absent.clone(), order).map_err(failed)?;
            guessed.stops.push(task.abort_handle());
            guessed.tasks.push(task);
            guessed.listed.lock().unwrap().push(part.arriving());
        }
        drop(guessed);
        let mut guessing = GUESSING.lock().unwrap();
        guessing.retain(|fetch| fetch.strong_count() > 0);
        guessing.push(Arc::downgrade(self));
        drop(guessing);
        self.watch(caller)
    }

    /// Its guesses end: their downloads stop and their checkpoints may go.
    pub(crate) fn drop_guessed(&self) {
        let mut guessed = self.guessed.lock().unwrap();
        guessed.stops.drain(..).for_each(|stop| stop.abort());
        guessed.tasks.clear();
        guessed.held.clear();
        guessed.listed.lock().unwrap().clear();
    }

    /// What it hinted and guesses, listed as the machine's download for run `caller` (`activity`)
    /// while any is absent, by a thread of its own.
    fn watch(self: &Arc<Self>, caller: &str) -> io::Result<()> {
        if self.watching.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let (fetch, caller) = (Arc::downgrade(self), caller.to_owned());
        std::thread::Builder::new().name("weights-ahead".into()).spawn(move || {
            let mut waits: HashMap<String, crate::activity::Waiting> = HashMap::new();
            loop {
                let Some(live) = fetch.upgrade().filter(|f| !f.ended()) else { return };
                let mut each = live.each(&live.hinted);
                each.extend(live.each(&live.guessed.lock().unwrap().listed));
                each.retain(|each| each.absent);
                if each.is_empty() {
                    live.watching.store(false, Ordering::Release);
                    return;
                }
                drop(live);
                waits.retain(|model, _| each.iter().any(|e| e.model == *model));
                for e in &each {
                    let wait = waits.entry(e.model.clone()).or_insert_with(|| crate::activity::wait(crate::activity::Kind::Download, &e.model, &caller));
                    wait.here(e.here);
                    wait.moved("", e.done, e.total);
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })?;
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

    /// What its call builds on demand (`selected`: header, components), fetched only once another process sharing the
    /// store begins reading it (its executor's own fetch, outside the machine's line), never one no stage reads (cl-804).
    pub(crate) fn on_demand(self: &Arc<Self>, selected: &[(String, Vec<String>)]) {
        if selected.is_empty() || self.hub.is_none() || self.ended() {
            return;
        }
        let each = selected.iter().map(|(header, components)| OnDemand { header: header.clone(), components: components.clone(), chunks: None });
        self.demand.lock().unwrap().extend(each);
        let mut reads = READS.lock().unwrap();
        reads.0.push(Arc::downgrade(self));
        if !reads.1 {
            let store = self.store().clone();
            match std::thread::Builder::new().name("on-demand-reads".into()).spawn(move || watch_reads(store)) {
                Ok(_) => reads.1 = true,
                Err(error) => tracing::warn!("watching on-demand reads: {error}"),
            }
        }
    }

    /// Each on-demand model of which another process fetches an object (`elsewhere`) comes whole now, at a started run's
    /// order with the line's every request: a later job's download goes on with what the link has left. What that
    /// process claimed is its own; the rest it waits for, or takes up, as it reads (cl-905).
    fn promote(&self, elsewhere: &HashSet<String>) {
        let read: Vec<(String, Vec<ObjectRef>)> = {
            let mut demand = self.demand.lock().unwrap();
            demand.retain_mut(|d| {
                let chunks = d.chunks.get_or_insert_with(|| checkpoint(self.store(), &d.header).map(|c| storage::chunks(&c, &d.components)).unwrap_or_default());
                !chunks.is_empty()
            });
            let (read, unread) = std::mem::take(&mut *demand).into_iter()
                .partition(|d| d.chunks.iter().flatten().any(|o| elsewhere.contains(&o.sha256)));
            *demand = unread;
            read.into_iter().map(|d| (d.header, d.chunks.unwrap_or_default())).collect()
        };
        let mut kept = self.kept.lock().unwrap();
        for (header, chunks) in read {
            let Ok(checkpoint) = checkpoint(self.store(), &header) else { continue };
            let Ok(Some(remote)) = self.source(checkpoint.id()) else { continue };
            if self.ended() {
                return;
            }
            let model = Pulled::of(self.store(), checkpoint.id()).ok().flatten().map_or(header, |p| p.model);
            let absent: Vec<ObjectRef> = chunks.into_iter().filter(|o| !elsewhere.contains(&o.sha256) && !self.store().holds(o)).collect();
            let bytes: u64 = absent.iter().map(|o| o.length).sum();
            tracing::info!(bytes, "another process began reading {model}, which its call builds on demand: the rest comes now, ahead of every queued run");
            if !absent.is_empty() {
                match self.machine.want(&remote, absent, priority::next(priority::STARTED, 0)) {
                    Ok(task) => kept.push(task),
                    Err(error) => tracing::warn!("{model}: {error}"),
                }
            }
            self.held.lock().unwrap().push(checkpoint);
        }
    }

    /// Whether it ended (`end`).
    pub(crate) fn ended(&self) -> bool {
        self.stop.is_cancelled()
    }

    /// Everything it wanted drops and its checkpoints close.
    pub(crate) fn end(&self) {
        self.stop.cancel();
        self.drop_guessed();
        for want in self.kept.lock().unwrap().drain(..) {
            want.abort();
        }
        self.held.lock().unwrap().clear();
        if let Some(replaced) = self.replaced.lock().unwrap().take() {
            replaced.end();
        }
    }
}

/// Calls whose on-demand models no other process has begun reading (`Fetch::on_demand`), and whether a thread looks.
static READS: Mutex<(Vec<std::sync::Weak<Fetch>>, bool)> = Mutex::new((Vec::new(), false));

/// How often the store's claims are looked at while a call has an on-demand model unread.
const READS_LOOK: std::time::Duration = std::time::Duration::from_millis(250);

/// While any call has an on-demand model unread, each `READS_LOOK` the objects another process sharing `store` fetches
/// (an executor's own read) go to the calls that read them (`Fetch::promote`).
fn watch_reads(store: Arc<Store>) {
    loop {
        std::thread::sleep(READS_LOOK);
        let live: Vec<Arc<Fetch>> = {
            let mut reads = READS.lock().unwrap();
            reads.0.retain(|fetch| fetch.upgrade().is_some_and(|fetch| !fetch.ended() && !fetch.demand.lock().unwrap().is_empty()));
            if reads.0.is_empty() {
                reads.1 = false;
                return;
            }
            reads.0.iter().filter_map(std::sync::Weak::upgrade).collect()
        };
        let elsewhere = store.claimed_elsewhere();
        if !elsewhere.is_empty() {
            live.iter().for_each(|fetch| fetch.promote(&elsewhere));
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
        // A call's weights show on its root's progress too, the box a person watches (cl-921: H3's segment downloaded 2.5 min
        // with no bytes on its shot): over its own stage, until they're here.
        let root = engine.get(&id).ok().map(|record| record.invocation.root).filter(|root| !root.is_empty() && *root != id);
        let tell = |weights: Option<Weights>| {
            if let Some(root) = &root {
                if let Err(error) = engine.observe_weights(root, weights.clone()) {
                    tracing::warn!("run {root}: its call's weights' progress: {error}");
                }
            }
            engine.observe_weights(&id, weights)
        };
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
            if let Err(error) = tell(weights.clone()) {
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
                let _ = tell(None);
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
