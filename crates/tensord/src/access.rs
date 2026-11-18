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
    journal::{Account, Waits, Weights},
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
    if hub_pull(machine.store(), header)?.is_none() {
        return Ok(None);
    }
    origin::remote(machine, source).map(Some).map_err(io::Error::other)
}

/// The Hub checkpoint `header` is, as this machine pulled it; a view's (its LoRAs composed on a base, `adapter::compose`) is
/// its base's, whose chunks it reads. None for one never pulled (local).
pub(crate) fn hub_pull(store: &Store, header: &ObjectRef) -> io::Result<Option<Pulled>> {
    match storage::view(&header.sha256) {
        Some(view) => Pulled::of(store, &storage::held(store, &view.recipe.base)?),
        None => Pulled::of(store, header),
    }
}

/// The checkpoint `id` (`sha256:<hex>`), its structure here.
pub(crate) fn checkpoint(store: &Store, id: &str) -> io::Result<Checkpoint> {
    storage::checkpoint(store, id)
}

/// What a call reads of each model it waits on (`Needs::needed`), as a quote of its function counts its download
/// anywhere (cl-967): its header, repository, those components and their bytes. One named whole, or not pulled from a Hub, is left
/// out.
pub(crate) fn reads(store: &Store, selected: &[(String, Vec<String>)]) -> Vec<crate::journal::FunctionRead> {
    (selected.iter().filter(|(_, components)| !components.is_empty()))
        .filter_map(|(header, components)| {
            let (object, parsed) = storage::structure(store, header).ok()?;
            let model = Pulled::of(store, &object).ok().flatten()?.model;
            let bytes = chunks_in(header, &object, components, &parsed).iter().map(|o| o.length).sum();
            Some(crate::journal::FunctionRead { checkpoint: header.clone(), model, components: components.clone(), bytes })
        })
        .collect()
}

/// The chunks `components` of checkpoint `header` read (all, when none), by header and components: read once from its structure,
/// then known for good, as a header names its chunks for good. So they are known once the header went with its model too, while
/// other models still hold some of them (two fine-tunes share a base's VAE; 44185604891: pre-a's VAE stayed under pre-b).
static CHUNKS: Mutex<BTreeMap<(String, Vec<String>), Arc<[ObjectRef]>>> = Mutex::new(BTreeMap::new());

/// The chunks `components` of `header` read, from its structure here (`object` and `parsed`, `storage::structure`), and known from
/// now on (`CHUNKS`).
fn chunks_in(header: &str, object: &ObjectRef, components: &[String], parsed: &tensorfs::format::Header) -> Arc<[ObjectRef]> {
    let key = (header.to_owned(), components.to_vec());
    if let Some(known) = CHUNKS.lock().unwrap().get(&key) {
        return known.clone();
    }
    let chunks: Arc<[ObjectRef]> = storage::chunks_of(&object.sha256, parsed, components).into();
    CHUNKS.lock().unwrap().entry(key).or_insert(chunks).clone()
}

/// The chunks `components` of checkpoint `header` read (all, when none): as its structure here says, else as it said when it was
/// last here. None: its header isn't here and never was read here. Read off disk alone: a reader opens no checkpoint (cl-1202).
fn chunks_of(store: &Store, header: &str, components: &[String]) -> Option<Arc<[ObjectRef]>> {
    if let Some(known) = CHUNKS.lock().unwrap().get(&(header.to_owned(), components.to_vec())) {
        return Some(known.clone());
    }
    let (object, parsed) = storage::structure(store, header).ok()?;
    Some(chunks_in(header, &object, components, &parsed))
}

/// Of what `read` names, the bytes not here now: of its chunks, those the store doesn't hold, though its header went with its model
/// (`chunks_of`). All of them, for a header never read here.
pub(crate) fn missing(store: &Store, read: &crate::journal::FunctionRead) -> u64 {
    if whole(store, &read.checkpoint, &read.components) {
        return 0;
    }
    match chunks_of(store, &read.checkpoint, &read.components) {
        Some(chunks) => chunks.iter().filter(|o| !store.holds(o)).map(|o| o.length).sum(),
        None => read.bytes,
    }
}

/// Each of `read`'s components: its bytes, and of them those not here now (cl-1104: a quote of one slot counts only those it reads).
/// None for a header never read here (`chunks_of`).
pub(crate) fn parts(store: &Store, read: &crate::journal::FunctionRead) -> Vec<(String, u64, u64)> {
    let whole = whole(store, &read.checkpoint, &read.components);
    (read.components.iter())
        .map(|component| {
            let chunks = chunks_of(store, &read.checkpoint, std::slice::from_ref(component))?;
            let bytes: u64 = chunks.iter().map(|o| o.length).sum();
            let absent: u64 = match whole {
                true => 0,
                false => chunks.iter().filter(|o| !store.holds(o)).map(|o| o.length).sum(),
            };
            Some((component.clone(), bytes, absent))
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// Whether every chunk of `components` of `header` is here (`downloaded`). Once true it is
/// remembered (Status asks every second) until the store next evicts, in any process. Judged from the store's files: Status and
/// the dispatcher ask it, and a reader opens no checkpoint (cl-1202).
pub(crate) fn whole(store: &Store, header: &str, components: &[String]) -> bool {
    static WHOLE: Mutex<BTreeMap<String, Option<std::time::SystemTime>>> = Mutex::new(BTreeMap::new());
    let key = format!("{}\0{header}\0{components:?}", store.root().display());
    let evicted = store.evicted_at();
    if WHOLE.lock().unwrap().get(&key).is_some_and(|known| *known == evicted) {
        return true;
    }
    let whole = storage::present(store, header) && chunks_of(store, header, components).is_some_and(|chunks| chunks.iter().all(|o| store.holds(o)));
    match whole {
        true => WHOLE.lock().unwrap().insert(key, evicted),
        false => WHOLE.lock().unwrap().remove(&key),
    };
    whole
}

/// What a queued run waits on while the models it needs download, with what it has and needs (`Fetch::needs`); TensorD's log says
/// how far each is (`waiting_on`), and the machine's activity its bytes (cl-1205).
pub(crate) const WAITING_FOR_MODELS: &str = "waiting for the models to download";

/// Bytes for people (`reclaim::size`).
fn gib(bytes: u64) -> String {
    crate::reclaim::size(bytes)
}

/// A Unix ms instant as its UTC clock time, `HH:MM:SSZ`.
fn clock(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}Z", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

pub(crate) fn failed(refusal: Refusal) -> io::Error {
    let failure = match refusal.code {
        Code::CAPACITY_EXHAUSTED => format!("machine_disk_full: This machine's disk is full ({}).", refusal.detail),
        Code::CREDENTIAL_REQUIRED => format!("hub_credential_refused: The Hub refused access to a private model ({}).", refusal.detail),
        _ => format!("model_fetch_failed: Couldn't download the model ({refusal})."),
    };
    io::Error::other(failure)
}

/// A failure as `failed` words it: its code (a full disk, else a failed fetch) and what it says.
pub(crate) fn code_of(failure: &str) -> (&'static str, &str) {
    let words = |code: &str| failure.strip_prefix(code).and_then(|rest| rest.strip_prefix(": "));
    match words("machine_disk_full") {
        Some(why) => ("machine_disk_full", why),
        None => ("model_fetch_failed", ["model_fetch_failed", "hub_credential_refused"].into_iter().find_map(words).unwrap_or(failure)),
    }
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
    /// What the machine had of its needs and what it fetches, as `ahead` reckoned it.
    account: Account,
}

/// One required component's want: what it is, where its chunks come from, the chunks that were
/// absent when it was wanted, and whether the downloader has answered it.
struct RequiredWant {
    label: String,
    remote: Remote,
    objects: Vec<ObjectRef>,
    settled: bool,
}

/// What a call loads: the one input of its downloads, its progress and its record (cl-1102; Paul, 11-16: "my machine has (X) but
/// needs (Y). And then we download just Y, and nothing more. We can download extras (Z) in the background … they shouldn't be waited
/// on"). `needed` is what it is sure to read: what of it isn't here is Y, all its call waits on. `extras` are what it may read, each
/// by name (Z): fetched behind every run's needs, never waited on. Its source is the rules in its function's published interface,
/// evaluated against the request (cl-1103; Paul, 11-16: "plan, without having to install packages and run arbitrary code"): which
/// slots, components and called functions this request uses. Needed from extra is decided in one place, the `needs` crate's walk
/// (`needs::wants`), which Calcifer walks too: a list item comes with Calcifer's (`Ahead::wants`), any other call is walked here
/// from its request; `Plan::needs` and `Runs::needs` only resolve its slots to checkpoints.
#[derive(Clone, Default)]
pub(crate) struct Needs {
    pub needed: Selected,
    pub extras: Vec<(String, Selected)>,
}

impl Needs {
    /// What the one walk of a call's wants resolved to (`needs::resolved`, cl-1103).
    pub(crate) fn of(resolved: needs::Resolved) -> Self {
        Self { needed: resolved.needed, extras: resolved.extras }
    }
}

/// Checkpoints by header, each with the components read (all, when none).
pub(crate) type Selected = Vec<(String, Vec<String>)>;

/// `Needs` in the machine's download line (`Fetch::lined`), the one reckoning of what a call has and fetches: each needed and each
/// extra component's order and chunks, the extras' checkpoints, what the machine has of the needed and fetches (`account`), each
/// extra held back, in words, and a needed checkpoint that isn't all here and comes from no Hub (`unsourced`).
#[derive(Default)]
pub(crate) struct Lined {
    wants: Vec<(u64, Lining)>,
    guessed: Vec<(u64, Lining)>,
    guesses: Vec<Checkpoint>,
    pub account: Account,
    pub held_back: Vec<String>,
    unsourced: Option<String>,
}

/// Every fetch with extras on the way now (`Fetch::guess`), for a need that has to make room to drop them all.
static GUESSING: Mutex<Vec<std::sync::Weak<Fetch>>> = Mutex::new(Vec::new());

/// Every guess on this machine ends: its downloads stop and its checkpoints may go. A run's need
/// that has to make room comes before any guess (cl-905).
pub(crate) fn drop_guesses() {
    let guessing = std::mem::take(&mut *GUESSING.lock().unwrap());
    for fetch in guessing.iter().filter_map(std::sync::Weak::upgrade) {
        fetch.drop_guessed();
    }
}

/// An extra's downloads, which end without failing what waits on them, and its checkpoints. No run waits on one, so the machine
/// lists none as a run's download (cl-1102): a run's progress counts its needs alone.
#[derive(Default)]
struct Guessed {
    stops: Vec<tokio::task::AbortHandle>,
    tasks: Vec<tokio::task::JoinHandle<tensorfs::err::Result<tensorfs::fetch::Landed>>>,
    held: Vec<Checkpoint>,
}

/// One component in line: its bytes, origin, repository, its release and component (none: the whole checkpoint), name for people,
/// chunks, and those absent.
struct Lining {
    bytes: u64,
    remote: Remote,
    model: String,
    release: String,
    component: Vec<String>,
    label: String,
    objects: Vec<ObjectRef>,
    absent: Vec<ObjectRef>,
}

impl Lining {
    /// It arriving, for the machine's list.
    fn arriving(self) -> Arriving {
        let fetch = self.absent.iter().map(|o| o.length).sum::<u64>();
        Arriving {
            model: self.model, release: self.release, components: self.component, remote: self.remote, fetch, here: self.bytes - fetch,
            wanted: self.absent.clone(), absent: self.absent,
        }
    }

    /// The machine's download of its repository waits on it (cl-1205): its objects absent when wanted, its release and component.
    fn join(&self, machine: &Arc<Machine>, wait: &crate::activity::Waiting) {
        wait.objects(machine, &self.remote, &self.absent);
        wait.fetches(&self.release, &self.component);
    }
}

impl Lined {
    /// The machine's download of each repository its needs bring (cl-1205), waited on by run `caller`: one wait each, its objects
    /// those absent now, as `download` wants them. A repository all here has none.
    pub(crate) fn waits(&self, machine: &Arc<Machine>, caller: &str) -> BTreeMap<String, crate::activity::Waiting> {
        let mut waits = BTreeMap::new();
        for (_, part) in self.wants.iter().filter(|(_, part)| !part.absent.is_empty()) {
            let wait = waits.entry(part.model.clone()).or_insert_with(|| crate::activity::wait(crate::activity::Kind::Download, &part.model, caller));
            part.join(machine, wait);
        }
        waits
    }
}

/// What run `id` waits on, as `waits` (by repository) say it (`Engine::observe_waits`), with `left` bytes of its own still to come;
/// none once it waits on nothing.
pub(crate) fn waits_of(waits: &BTreeMap<String, crate::activity::Waiting>, left: u64) -> Option<crate::journal::Waits> {
    (!waits.is_empty()).then(|| crate::journal::Waits {
        on: waits.values().map(|wait| wait.shown().to_owned()).collect(),
        models: waits.keys().cloned().collect(),
        left,
    })
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
}

/// A checkpoint not all here when it was wanted: its repository, release and components and where it comes from, the bytes it had
/// to fetch and those it held already, its objects absent when wanted, and those still absent.
struct Arriving {
    model: String,
    release: String,
    components: Vec<String>,
    remote: Remote,
    fetch: u64,
    here: u64,
    wanted: Vec<ObjectRef>,
    absent: Vec<ObjectRef>,
}

impl Arriving {
    /// The machine's download of its repository waits on it (cl-1205): its objects absent when it was wanted.
    fn join(&self, machine: &Arc<Machine>, wait: &crate::activity::Waiting) {
        wait.objects(machine, &self.remote, &self.wanted);
        wait.fetches(&self.release, &self.components);
    }
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

    /// What `previous` listed as its run's download (`hint`: the same needs, wanted before its code installed) is this call's to
    /// say from here on (`meter`), from where it stood: one download, counted once (cl-1102). Both said it, each with its own
    /// total, and the machine's list showed one then the other.
    pub(crate) fn carries(&self, previous: &Fetch) {
        let listed = std::mem::take(&mut *previous.hinted.lock().unwrap());
        if !listed.is_empty() {
            *self.arriving.lock().unwrap() = listed;
        }
    }

    /// What it listed as its run's download is said no more: a call of its run says what it waits on from here (`meter`).
    pub(crate) fn unlist(&self) {
        self.hinted.lock().unwrap().clear();
    }

    /// Want what a call `needs` (cl-1102), before its executor reads it. What of `needed` isn't here (Y) comes next, in Runtime's
    /// Start-pass order, the smallest component first, held until the call ends: its run waits on that and nothing else. `room` is
    /// asked for Y's bytes before any is wanted: it frees them on the disk or refuses, and a refusal fails the call before it
    /// downloads or starts. Its `extras` (Z) come behind every run's needs, at `extras`' orders, while they fit in its free disk
    /// bytes beside Y; with none given they are on their way already. For a `cold` call (no executor holds its construction) the
    /// chunks already on disk are read into the page cache in the same order meanwhile (`read_ahead`). Answers what the machine had
    /// and fetches (`Account`), and each extra held back, in words.
    pub(crate) fn ahead(
        self: &Arc<Self>, needs: &Needs, cold: bool, engine: &Arc<Engine>, room: &dyn Fn(u64) -> Result<(), String>,
        extras: Option<(&dyn Fn(u32) -> u64, u64)>,
    ) -> io::Result<(Account, Vec<String>)> {
        if cold {
            self.read_ahead(&needs.needed);
        }
        let refuse = |why: String| {
            self.required.lock().unwrap().failure = Some(why.clone());
            engine.notify_activity();
            io::Error::other(why)
        };
        let order = self.order;
        let lined = match extras {
            Some((guess, free)) => self.lined(needs, &|rank| priority::next(order, rank), guess, free),
            None => self.lined(&Needs { needed: needs.needed.clone(), extras: vec![] }, &|rank| priority::next(order, rank), &|_| priority::BEHIND, 0),
        };
        let mut lined = lined.map_err(|error| refuse(error.to_string()))?;
        if let Some(unsourced) = lined.unsourced.take() {
            return Err(refuse(unsourced));
        }
        // Its checkpoints are held from `lined` on: making room never evicts its own.
        let account = lined.account;
        room(account.needs).map_err(|why| refuse(why))?;
        let wanted: Vec<(u64, Lining)> = std::mem::take(&mut lined.wants).into_iter().filter(|(_, part)| !part.absent.is_empty()).collect();
        {
            let mut state = self.required.lock().unwrap();
            state.pending = wanted.len();
            state.account = account;
            state.wants = wanted.iter().map(|(_, part)| RequiredWant {
                label: part.label.clone(), remote: part.remote.clone(), objects: part.absent.clone(), settled: false,
            }).collect();
        }
        let missing = account.needs;
        let mut kept = self.kept.lock().unwrap();
        for (rank, (order, part)) in wanted.into_iter().enumerate() {
            let weak = Arc::downgrade(self);
            let wake = Arc::downgrade(engine);
            let started = self.machine.want_observed(&part.remote, part.absent.clone(), order, move |result| {
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
                Err(error) => return Err(refuse(failed(error).to_string())),
            }
            // What its run waits on (`meter`), under the repository it came from.
            self.arriving.lock().unwrap().push(part.arriving());
        }
        drop(kept);
        if extras.is_some() {
            self.guess(std::mem::take(&mut lined.guessed), std::mem::take(&mut lined.guesses))?;
        }
        Ok((account, lined.held_back))
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
            None => Ready::Waiting(self.needs()),
        }
    }

    /// What its run needs and what was here, in words (cl-1102): what its run says while it waits, which doesn't move as bytes
    /// arrive (cl-1205: those are its downloads', in the machine's activity).
    pub(crate) fn needs(&self) -> String {
        let account = self.required.lock().unwrap().account;
        match account.has {
            0 => format!("{} to download", gib(account.needs)),
            has => format!("{} to download, {} already on this machine", gib(account.needs), gib(has)),
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
                return format!("{label} is downloaded and being set up ({} pieces on disk)", objects.len());
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
            let last = if moved == 0 { String::new() } else { format!("; last moved at {}", clock(moved)) };
            let attempt = match unanswered {
                0 => String::new(),
                unanswered => format!("; {unanswered} of {} tries in a row got nothing", tensorfs::fetch::ATTEMPTS),
            };
            let hedges = match hedged {
                0 => String::new(),
                1 => "; 1 slow request tried again on another connection".to_owned(),
                hedged => format!("; {hedged} slow requests tried again on another connection"),
            };
            format!("{label}, {} of {} ({} of {} pieces left: {}{last}{attempt}{hedges})",
                gib(reached), gib(length), absent.len(), objects.len(), phases.join(", "))
        }).collect();
        // What it needs and what was here, first (cl-1102).
        Some(format!("{}; {}", self.needs(), each.join("; ")))
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

    /// `needs` in the machine's download line, each checkpoint held until the fetch ends: its needed components at `sure(rank)`,
    /// smallest first as a Start pass reads them; then each extra's at `guess(rank)`, in turn, unless its bytes absent don't fit in
    /// `free` beside all before it (an extra never makes room; a wrong one costs bytes only). A component twice comes once, and a
    /// checkpoint with no components named comes whole. Every fetch of a call reckons what it has and fetches here, once.
    pub(crate) fn lined(&self, needs: &Needs, sure: &dyn Fn(u32) -> u64, guess: &dyn Fn(u32) -> u64, free: u64) -> io::Result<Lined> {
        let (mut lined, mut seen, mut counted, mut taken, mut rank) = (Lined::default(), HashSet::new(), HashSet::new(), 0u64, 0u32);
        // An extra's checkpoints stay held only while it does (`guess`): they may go when a need makes room.
        let sets = std::iter::once((None, &needs.needed)).chain(needs.extras.iter().map(|(name, set)| (Some(name.as_str()), set)));
        for (guessed, set) in sets {
            let (mut parts, mut held) = (vec![], vec![]);
            for (header, components) in set {
                let checkpoint = checkpoint(self.store(), header)?;
                match self.source(checkpoint.id())? {
                    Some(remote) => {
                        let (model, release) = hub_pull(self.store(), checkpoint.id())?.map_or_else(|| (header.clone(), String::new()), |p| (p.model, p.release));
                        let each: Vec<Vec<String>> = match components.is_empty() {
                            true => vec![vec![]],
                            false => components.iter().map(|component| vec![component.clone()]).collect(),
                        };
                        for component in each.into_iter().filter(|component| seen.insert((header.clone(), component.clone()))) {
                            let objects = storage::chunks(&checkpoint, &component);
                            let absent = objects.iter().filter(|o| !self.store().holds(o)).cloned().collect();
                            let bytes = objects.iter().map(|o| o.length).sum();
                            let label = match component.first() {
                                Some(component) => format!("{model} {component}"),
                                None => model.clone(),
                            };
                            parts.push(Lining {
                                bytes, remote: remote.clone(), model: model.clone(), release: release.clone(), component, label, objects, absent,
                            });
                        }
                    }
                    // One no Hub brings is all here, or its call can't run.
                    None if guessed.is_none() && storage::chunks(&checkpoint, components).iter().any(|o| !self.store().holds(o)) => {
                        lined.unsourced.get_or_insert_with(|| "required local checkpoint input is missing and has no fetch source".to_owned());
                    }
                    None => {}
                }
                held.push(checkpoint);
            }
            parts.sort_by_key(|part| part.bytes);
            let (mut new, mut all) = (HashSet::new(), HashSet::new());
            let absent: u64 = (parts.iter().flat_map(|part| &part.absent))
                .filter(|o| !counted.contains(&o.sha256) && new.insert(o.sha256.clone()))
                .map(|o| o.length)
                .sum();
            if let Some(name) = guessed.filter(|_| taken + absent > free) {
                lined.held_back.push(format!("{name} ({}) isn't downloaded ahead of time: only {} of disk is free beside what's needed first", gib(absent), gib(free.saturating_sub(taken))));
                continue;
            }
            taken += absent;
            counted.extend(new);
            let (wants, order): (&mut Vec<_>, &dyn Fn(u32) -> u64) = match guessed {
                None => {
                    // What of its needs is here already, each object once.
                    let whole: u64 = (parts.iter().flat_map(|part| &part.objects)).filter(|o| all.insert(o.sha256.clone())).map(|o| o.length).sum();
                    lined.account = Account { has: whole.saturating_sub(absent), needs: absent, extras: 0 };
                    self.held.lock().unwrap().extend(held);
                    (&mut lined.wants, sure)
                }
                Some(_) => {
                    lined.account.extras += absent;
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

    /// Every needed component `lined` holds here, each at its order, reporting its bytes (a warm run of
    /// a function): those that came from the Hub and from the datacenter volume, and those held. Its
    /// extras are wanted beside them and waited for until they land or are dropped (`guess`): a warm run
    /// is itself a fetch ahead, no one's request. `wanted` hears once every one of them is in the download line.
    pub(crate) fn download(self: &Arc<Self>, mut lined: Lined, stop: &Stop, progress: &(dyn Fn(u64, u64) + Sync), wanted: &dyn Fn()) -> io::Result<(tensorfs::fetch::Landed, Extras)> {
        self.guess(std::mem::take(&mut lined.guessed), std::mem::take(&mut lined.guesses))?;
        let counted = Arc::new(tensorfs::fetch::Progress::default());
        let wants = lined.wants.into_iter().map(|(order, part)| (part.remote, part.objects, order)).collect();
        let landed = origin::watched(&counted, progress, || self.machine.all(wants, counted.clone(), wanted, stop).map_err(failed))?;
        Ok((landed, Extras(self.clone())))
    }

    /// Wants what `lined` holds absent here, each at its order, none of which gates its run yet: an
    /// orchestrator's internal functions' weights, while its root starts. The needed ones are held until
    /// the fetch ends, its extras until they are dropped (`guess`). While a needed one is absent the machine
    /// lists its download for run `caller` (`activity`); with none (a running call's hint of its next,
    /// which it doesn't wait on now), for no run.
    pub(crate) fn hint(self: &Arc<Self>, mut lined: Lined, caller: Option<&str>) -> io::Result<()> {
        self.guess(std::mem::take(&mut lined.guessed), std::mem::take(&mut lined.guesses))?;
        let mut kept = self.kept.lock().unwrap();
        if self.ended() {
            return Ok(());
        }
        for (order, part) in lined.wants.into_iter().filter(|(_, part)| !part.absent.is_empty()) {
            kept.push(self.machine.want(&part.remote, part.absent.clone(), order).map_err(failed)?);
            if caller.is_some() {
                self.hinted.lock().unwrap().push(part.arriving());
            }
        }
        drop(kept);
        match caller {
            Some(caller) => self.watch(caller),
            None => Ok(()),
        }
    }

    /// Wants `parts` (extras, at their orders) and holds their `checkpoints` until the fetch ends or
    /// they are dropped: by its run's first call, which says which function it is, or for any
    /// need that has to make room (`drop_guesses`). An extra never makes room itself, and no run is
    /// listed as waiting on one.
    fn guess(self: &Arc<Self>, parts: Vec<(u64, Lining)>, checkpoints: Vec<Checkpoint>) -> io::Result<()> {
        let mut guessed = self.guessed.lock().unwrap();
        if self.ended() || parts.iter().all(|(_, part)| part.absent.is_empty()) {
            return Ok(());
        }
        guessed.held.extend(checkpoints);
        for (order, part) in parts.into_iter().filter(|(_, part)| !part.absent.is_empty()) {
            let task = self.machine.want(&part.remote, part.absent.clone(), order).map_err(failed)?;
            guessed.stops.push(task.abort_handle());
            guessed.tasks.push(task);
        }
        drop(guessed);
        let mut guessing = GUESSING.lock().unwrap();
        guessing.retain(|fetch| fetch.strong_count() > 0);
        guessing.push(Arc::downgrade(self));
        Ok(())
    }

    /// Its extras end: their downloads stop and their checkpoints may go.
    pub(crate) fn drop_guessed(&self) {
        let mut guessed = self.guessed.lock().unwrap();
        guessed.stops.drain(..).for_each(|stop| stop.abort());
        guessed.tasks.clear();
        guessed.held.clear();
    }

    /// What it hinted of its needs, listed as the machine's download for run `caller` (`activity`)
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
                each.retain(|each| each.absent);
                if each.is_empty() {
                    live.watching.store(false, Ordering::Release);
                    return;
                }
                waits.retain(|model, _| each.iter().any(|e| e.model == *model));
                for e in &each {
                    let wait = waits.entry(e.model.clone()).or_insert_with(|| {
                        let wait = crate::activity::wait(crate::activity::Kind::Download, &e.model, &caller);
                        live.join(&live.hinted, &e.model, &wait);
                        wait
                    });
                    wait.here(e.here);
                    wait.look();
                }
                drop(live);
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })?;
        Ok(())
    }

    /// The machine's download of `model` waits on what of it `list` holds (cl-1205): each checkpoint's objects absent when wanted.
    fn join(&self, list: &Mutex<Vec<Arriving>>, model: &str, wait: &crate::activity::Waiting) {
        for arriving in list.lock().unwrap().iter().filter(|arriving| arriving.model == model) {
            arriving.join(&self.machine, wait);
        }
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
        // A call's weights show on its root's progress too, the box a person watches (cl-921: H3's segment downloaded 2.5 min
        // with no bytes on its shot): over its own stage, until they're here.
        let root = engine.get(&id).ok().map(|record| record.invocation.root).filter(|root| !root.is_empty() && *root != id);
        // What its run waits on, as this machine lists it (`activity`), by repository, under its root: the run its caller knows. A
        // call's own id named no run there, and its download wasn't shown (cl-1205).
        let caller = engine.caller_id(root.as_deref().unwrap_or(&id));
        let tell = |waits: Option<Waits>| {
            if let Some(root) = &root {
                if let Err(error) = engine.observe_waits(root, waits.clone()) {
                    tracing::warn!("run {root}: what its call waits on: {error}");
                }
            }
            engine.observe_waits(&id, waits)
        };
        let mut waits: BTreeMap<String, crate::activity::Waiting> = BTreeMap::new();
        loop {
            let live = fetch.upgrade().filter(|f| !f.ended());
            let (weights, waited) = live.as_ref().map(|f| f.arriving()).unwrap_or_default();
            waits.retain(|model, _| waited.iter().any(|each| each.model == *model));
            for each in &waited {
                let wait = waits.entry(each.model.clone()).or_insert_with(|| {
                    let wait = crate::activity::wait(crate::activity::Kind::Download, &each.model, &caller);
                    if let Some(live) = &live {
                        live.join(&live.arriving, &each.model, &wait);
                    }
                    wait
                });
                wait.here(each.here);
                wait.look();
            }
            // It says which downloads it waits on, never their bytes: those are the machine's, once (Paul, 11-17).
            let left = weights.as_ref().map_or(0, |weights| weights.total.saturating_sub(weights.done));
            if let Err(error) = tell(waits_of(&waits, left)) {
                tracing::warn!("run {id}: what it waits on: {error}");
                return;
            }
            let waiting = live.as_ref().and_then(|f| f.waiting_on());
            if waiting != shown {
                if let Some(on) = &waiting {
                    // The log line's state is its phases and counts: bytes and times change it not.
                    let state = phases(on);
                    if state != said {
                        tracing::info!("run {id}: {WAITING_FOR_MODELS}: {on}");
                        said = state;
                    }
                    // Its own words say what it has and needs, which don't move; how far each is, the log and its downloads say.
                    if let Some(needs) = live.as_ref().map(|f| f.needs()) {
                        if let Err(error) = engine.waiting(&id, Some(format!("{WAITING_FOR_MODELS}: {needs}"))) {
                            tracing::warn!("run {id}: what it waits on: {error}");
                        }
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

/// A download's extras (Z), still coming once its needs (Y) landed: never waited on (cl-1102; cl-1123: a predicted request is
/// `ready`, and may load, the moment its needs are here).
pub(crate) struct Extras(Arc<Fetch>);

impl Extras {
    /// They come on, behind every need, on a thread of their own, until they land, are dropped (`drop_guesses`), or `stop` ends them.
    pub(crate) fn follow(self, stop: Stop) {
        let tasks = std::mem::take(&mut self.0.guessed.lock().unwrap().tasks);
        if tasks.is_empty() {
            return;
        }
        let fetch = self.0;
        let started = std::thread::Builder::new().name("extras".into()).spawn(move || {
            let machine = fetch.machine.clone();
            // A dropped guess ends its task: that ends no wait here.
            let _ = machine.block(&stop, async move {
                for task in tasks {
                    let _ = task.await;
                }
                Ok(())
            });
            drop(fetch);
        });
        if let Err(error) = started {
            tracing::warn!("its extras aren't followed: {error}");
        }
    }
}
