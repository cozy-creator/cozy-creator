//! What this machine does for runs besides their own work: weights arriving and packages' environments built, each listed once however
//! many runs wait on it, until the last of them stops waiting. Status shows each caller what its runs wait on (`api::machine_status`),
//! and a run waiting on one names it (`Waiting::shown`, cl-1205).
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use api::machine as v1;
use tensorfs::format::ObjectRef;

use crate::origin::{Machine, Remote};

/// What Status shows may have changed: something began (engine activity, an operation, an update). A Status stream with
/// nothing going waits on this (`api::machine_status`).
static STATUS: LazyLock<tokio::sync::watch::Sender<u64>> = LazyLock::new(|| tokio::sync::watch::Sender::new(0));

pub(crate) fn status_changed() {
    STATUS.send_modify(|n| *n = n.wrapping_add(1));
}

pub(crate) fn status_changes() -> tokio::sync::watch::Receiver<u64> {
    STATUS.subscribe()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Download,
    Install,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Download => "download",
            Kind::Install => "install",
        }
    }
}

struct Operation {
    kind: Kind,
    subject: String,
    stage: String,
    /// Its bytes fetched of those it fetches: its objects' (`Objects`), else as its one wait says them (`Waiting::moved`).
    done: u64,
    total: u64,
    /// What of it was here already, which it never fetches, as its first wait to say it found it.
    here: Option<u64>,
    /// A download's objects, each once however many runs wait on it (cl-1205).
    objects: Objects,
    /// What its waits fetch of it: the release, and the components (none: the whole checkpoint).
    release: String,
    components: BTreeSet<String>,
    started_at_ms: i64,
    /// Each run waiting on it, by its caller's id, with how many of its waits are open.
    runs: BTreeMap<String, usize>,
    /// Its byte counts over the last [`WINDOW`]: its rate.
    seen: VecDeque<(Instant, u64)>,
    /// Status has listed it: one that ended before then is listed once, however short it was, unless no one looks for [`UNSEEN`].
    listed: bool,
    ended: Option<Instant>,
}

const WINDOW: Duration = Duration::from_secs(10);
const UNSEEN: Duration = Duration::from_secs(10);
/// How often a download's objects still coming are looked at, by whichever of its waits ticks first.
const LOOK: Duration = Duration::from_millis(250);

/// A download's objects (cl-1205; Paul, 11-17: "it looks like we're downloading it 3 times, but we're actually just downloading it
/// once"): each counted once, however many waits want it, its total as each joins and its bytes as each lands. Those still coming
/// are looked at no more than every [`LOOK`] (`Waiting::look`); Status reads the counts.
#[derive(Default)]
struct Objects {
    /// Where its objects come from: each wait's machine and origin.
    sources: Vec<(Arc<Machine>, Remote)>,
    /// Those still to come, by sha256.
    coming: HashMap<String, Coming>,
    /// Those that landed, by sha256, and their bytes.
    landed: BTreeSet<String>,
    landed_bytes: u64,
    /// The bytes of those coming, and those of them that arrived so far.
    coming_bytes: u64,
    reached: u64,
    looked: Option<Instant>,
}

struct Coming {
    object: ObjectRef,
    source: usize,
    reached: u64,
    /// How many waits want it: it is counted while any does.
    wants: usize,
}

impl Objects {
    fn any(&self) -> bool {
        !self.coming.is_empty() || !self.landed.is_empty()
    }

    fn done(&self) -> u64 {
        self.landed_bytes + self.reached
    }

    fn total(&self) -> u64 {
        self.landed_bytes + self.coming_bytes
    }

    fn land(&mut self, sha256: &str) {
        if let Some(coming) = self.coming.remove(sha256) {
            self.coming_bytes -= coming.object.length;
            self.reached -= coming.reached;
            self.landed_bytes += coming.object.length;
            self.landed.insert(sha256.to_owned());
        }
    }
}

static OPERATIONS: LazyLock<Mutex<BTreeMap<String, Operation>>> = LazyLock::new(Mutex::default);

fn operations() -> MutexGuard<'static, BTreeMap<String, Operation>> {
    OPERATIONS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One run's wait on one operation, which is listed while any wait on it is open.
pub(crate) struct Waiting {
    id: String,
    run: String,
    /// Its id as Status lists it: what its run names in its progress (`journal::Waits`).
    shown: String,
    /// The objects it wants of a download (`objects`), by sha256.
    wanted: Mutex<Vec<String>>,
}

/// Run `run` (its caller's id) waits on the `kind` of `subject`: the one already listed, or a new one.
pub(crate) fn wait(kind: Kind, subject: &str, run: &str) -> Waiting {
    let id = crate::private_io::resource_key(&format!("{} {subject}", kind.word()));
    let mut listed = operations();
    let operation = listed.entry(id.clone()).or_insert_with(|| Operation {
        kind,
        subject: subject.to_owned(),
        stage: String::new(),
        done: 0,
        total: 0,
        here: None,
        objects: Objects::default(),
        release: String::new(),
        components: BTreeSet::new(),
        started_at_ms: crate::machine::lifecycle::now_ms(),
        runs: BTreeMap::new(),
        seen: VecDeque::new(),
        listed: false,
        ended: None,
    });
    // Joined again after its last wait ended: that wait's run no longer waits.
    if operation.ended.take().is_some() {
        operation.runs.clear();
    }
    *operation.runs.entry(run.to_owned()).or_default() += 1;
    drop(listed);
    status_changed();
    Waiting { shown: crate::private_io::unscoped_key(&id), id, run: run.to_owned(), wanted: Mutex::default() }
}

/// Its bytes `done` at `now` count toward its rate over the last [`WINDOW`].
fn sample(seen: &mut VecDeque<(Instant, u64)>, now: Instant, done: u64) {
    if seen.back().is_some_and(|(_, last)| done < *last) {
        seen.clear();
    }
    seen.push_back((now, done));
    while seen.len() > 2 && seen.front().is_some_and(|(at, _)| now - *at > WINDOW) {
        seen.pop_front();
    }
}

impl Waiting {
    /// Its id as Status lists it (`v1::Operation::id`).
    pub(crate) fn shown(&self) -> &str {
        &self.shown
    }

    /// What of it was here already: a download's bytes are only those it fetches. Its first wait to say found it so.
    pub(crate) fn here(&self, bytes: u64) {
        if let Some(operation) = operations().get_mut(&self.id) {
            operation.here.get_or_insert(bytes);
        }
    }

    /// What it fetches of a download: `release`'s `components` (none: the whole checkpoint).
    pub(crate) fn fetches(&self, release: &str, components: &[String]) {
        if let Some(operation) = operations().get_mut(&self.id) {
            if operation.release.is_empty() {
                release.clone_into(&mut operation.release);
            }
            operation.components.extend(components.iter().cloned());
        }
    }

    /// It waits on `objects` of the download, from `remote` on `machine`: those absent when it wanted them. Each joins the download's
    /// bytes once, whichever wait wants it first; one here now landed since.
    pub(crate) fn objects(&self, machine: &Arc<Machine>, remote: &Remote, objects: &[ObjectRef]) {
        let store = machine.store();
        let mut listed = operations();
        let Some(operation) = listed.get_mut(&self.id) else { return };
        let held = &mut operation.objects;
        let source = held.sources.len();
        held.sources.push((machine.clone(), remote.clone()));
        let mut wanted = self.wanted.lock().unwrap_or_else(PoisonError::into_inner);
        for object in objects.iter().filter(|object| !held.landed.contains(&object.sha256)) {
            if let Some(coming) = held.coming.get_mut(&object.sha256) {
                coming.wants += 1;
                wanted.push(object.sha256.clone());
                continue;
            }
            if store.holds(object) {
                held.landed.insert(object.sha256.clone());
                held.landed_bytes += object.length;
                continue;
            }
            held.coming.insert(object.sha256.clone(), Coming { object: object.clone(), source, reached: 0, wants: 1 });
            held.coming_bytes += object.length;
            wanted.push(object.sha256.clone());
        }
        (operation.done, operation.total) = (held.done(), held.total());
    }

    /// A download's objects still coming, looked at now unless one of its waits did within [`LOOK`]: each one the line still
    /// fetches by the bytes it reached, and each it no longer does that the store holds, landed. Read off the line and the store
    /// outside the lock, whose counts Status reads.
    pub(crate) fn look(&self) {
        let now = Instant::now();
        let looking: Vec<(Arc<Machine>, Remote, Vec<ObjectRef>)> = {
            let mut listed = operations();
            let Some(operation) = listed.get_mut(&self.id) else { return };
            let held = &mut operation.objects;
            if held.coming.is_empty() || held.looked.is_some_and(|at| now - at < LOOK) {
                return;
            }
            held.looked = Some(now);
            let mut by: Vec<Vec<ObjectRef>> = vec![vec![]; held.sources.len()];
            for coming in held.coming.values() {
                by[coming.source].push(coming.object.clone());
            }
            (held.sources.iter().zip(by)).filter(|(_, objects)| !objects.is_empty()).map(|((machine, remote), objects)| (machine.clone(), remote.clone(), objects)).collect()
        };
        let mut seen: Vec<(String, Option<u64>)> = vec![];
        for (machine, remote, objects) in looking {
            for (object, state) in objects.iter().zip(machine.fetching(&remote, &objects)) {
                match state {
                    Some(state) => seen.push((object.sha256.clone(), Some(state.reached.min(object.length)))),
                    None if machine.store().holds(object) => seen.push((object.sha256.clone(), None)),
                    None => {}
                }
            }
        }
        let mut listed = operations();
        let Some(operation) = listed.get_mut(&self.id) else { return };
        let held = &mut operation.objects;
        for (sha256, reached) in seen {
            match reached {
                Some(reached) => {
                    if let Some(coming) = held.coming.get_mut(&sha256) {
                        held.reached = held.reached - coming.reached + reached;
                        coming.reached = reached;
                    }
                }
                None => held.land(&sha256),
            }
        }
        (operation.done, operation.total) = (held.done(), held.total());
        let done = operation.done;
        sample(&mut operation.seen, now, done);
    }

    /// Where it is: in words, and, for a wait that knows no objects of it, its bytes fetched of those it fetches.
    pub(crate) fn moved(&self, stage: &str, done: u64, total: u64) {
        let mut listed = operations();
        let Some(operation) = listed.get_mut(&self.id) else { return };
        if operation.stage != stage {
            stage.clone_into(&mut operation.stage);
        }
        // An install's are its tool's output, not what it fetches: its stage says how far it is. A download's objects say its bytes.
        if (done == 0 && total == 0) || operation.kind == Kind::Install || operation.objects.any() {
            return;
        }
        sample(&mut operation.seen, Instant::now(), done);
        (operation.done, operation.total) = (done, total);
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut listed = operations();
        let Some(operation) = listed.get_mut(&self.id) else { return };
        // What only it wanted, still coming, no longer counts: nothing fetches it for this download now.
        let held = &mut operation.objects;
        for sha256 in self.wanted.get_mut().unwrap_or_else(PoisonError::into_inner).drain(..) {
            let Some(coming) = held.coming.get_mut(&sha256) else { continue };
            coming.wants -= 1;
            if coming.wants == 0 {
                let (length, reached) = (coming.object.length, coming.reached);
                held.coming.remove(&sha256);
                held.coming_bytes -= length;
                held.reached -= reached;
            }
        }
        (operation.done, operation.total) = match held.any() {
            true => (held.done(), held.total()),
            false => (operation.done, operation.total),
        };
        let Some(open) = operation.runs.get_mut(&self.run) else { return };
        *open -= 1;
        if *open > 0 {
            return;
        }
        if operation.runs.len() > 1 {
            operation.runs.remove(&self.run);
            return;
        }
        // Its last wait, its run still named while it is listed this once.
        match operation.listed {
            true => drop(listed.remove(&self.id)),
            false => operation.ended = Some(Instant::now()),
        }
    }
}

/// A provider's source imported for a run: the machine's download while it is fetched, and no longer once it is converted and committed,
/// which is its run's own work. A file of this computer's (`input://`) is no download.
pub(crate) struct Import {
    subject: String,
    run: String,
    held: Mutex<Option<Waiting>>,
}

impl Import {
    pub(crate) fn new(source: &str, run: &str) -> Self {
        Import { subject: source.to_owned(), run: run.to_owned(), held: Mutex::default() }
    }

    /// TensorFS's import stage, and its bytes.
    pub(crate) fn moved(&self, stage: &str, done: u64, total: u64) {
        if self.subject.starts_with("input://") {
            return;
        }
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let (done, total) = match stage {
            "downloading" => (done, total),
            "resolving" | "reading headers" => (0, 0),
            _ => return drop(held.take()),
        };
        held.get_or_insert_with(|| wait(Kind::Download, &self.subject, &self.run)).moved(stage, done, total);
    }
}

/// What another run's work here that `run` (its caller's id) may be behind is, in words: the download or install going for other
/// runs with the most left, `minimaxai/minimax-h3's download for run-1b589…, 40 GB left` (cl-1185). None: none goes.
pub(crate) fn ahead_of(run: &str) -> Option<String> {
    let operations = operations();
    let busiest = (operations.values())
        .filter(|operation| operation.ended.is_none() && !operation.runs.is_empty() && !operation.runs.contains_key(run))
        .max_by_key(|operation| operation.total.saturating_sub(operation.done))?;
    let runs: Vec<&str> = busiest.runs.keys().map(String::as_str).collect();
    let left = busiest.total.saturating_sub(busiest.done);
    Some(format!("{}'s {} for {} to finish, {} left", busiest.subject, busiest.kind.word(), runs.join(", "), crate::reclaim::size(left)))
}

/// What the runs `mine` admits wait on, each with only those runs. One that ended unlisted is listed this once.
/// The repositories each run (by its caller's id) waits to download now.
pub(crate) fn downloading() -> std::collections::HashMap<String, Vec<String>> {
    let mut waits: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for operation in operations().values().filter(|operation| operation.kind == Kind::Download && operation.ended.is_none()) {
        for run in operation.runs.keys() {
            waits.entry(run.clone()).or_default().push(operation.subject.clone());
        }
    }
    waits
}

pub(crate) fn listed(mine: impl Fn(&str) -> bool) -> Vec<v1::Operation> {
    let mut operations = operations();
    let listed = operations
        .iter_mut()
        .filter_map(|(id, operation)| {
            let runs: Vec<String> = operation.runs.keys().filter(|run| mine(run)).cloned().collect();
            operation.listed |= !runs.is_empty();
            let bytes_per_s = rate(&operation.seen);
            let left = operation.total.saturating_sub(operation.done);
            (!runs.is_empty()).then(|| v1::Operation {
                id: crate::private_io::unscoped_key(id),
                kind: operation.kind.word().into(),
                subject: operation.subject.clone(),
                release: operation.release.clone(),
                components: operation.components.iter().cloned().collect(),
                stage: operation.stage.clone(),
                bytes_done: operation.done,
                bytes_total: operation.total,
                bytes_here: operation.here.unwrap_or(0),
                bytes_per_s,
                eta_s: (bytes_per_s >= 1.0 && left > 0).then(|| (left as f64 / bytes_per_s).ceil() as i64),
                started_at_unix_ms: operation.started_at_ms,
                runs,
            })
        })
        .collect();
    operations.retain(|_, operation| operation.ended.is_none_or(|at| !operation.listed && at.elapsed() < UNSEEN));
    listed
}

/// Bytes a second across `seen`, once it spans a second.
fn rate(seen: &VecDeque<(Instant, u64)>) -> f64 {
    let (Some((first_at, first)), Some((last_at, last))) = (seen.front(), seen.back()) else {
        return 0.0;
    };
    let secs = (*last_at - *first_at).as_secs_f64();
    if secs < 1.0 {
        return 0.0;
    }
    (last - first) as f64 / secs
}
