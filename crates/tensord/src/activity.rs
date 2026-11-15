//! What this machine does for runs besides their own work: weights arriving and packages' environments built, each listed once however
//! many runs wait on it, until the last of them stops waiting. Status shows each caller what its runs wait on (`api::machine_status`).
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use api::machine as v1;

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
    done: u64,
    total: u64,
    /// What of it was here already, which it never fetches.
    here: u64,
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

static OPERATIONS: LazyLock<Mutex<BTreeMap<String, Operation>>> = LazyLock::new(Mutex::default);

fn operations() -> MutexGuard<'static, BTreeMap<String, Operation>> {
    OPERATIONS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One run's wait on one operation, which is listed while any wait on it is open.
pub(crate) struct Waiting {
    id: String,
    run: String,
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
        here: 0,
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
    Waiting { id, run: run.to_owned() }
}

impl Waiting {
    /// What of it was here already: a download's bytes are only those it fetches.
    pub(crate) fn here(&self, bytes: u64) {
        if let Some(operation) = operations().get_mut(&self.id) {
            operation.here = bytes;
        }
    }

    /// Where it is: in words, and a download by its bytes fetched of those it fetches.
    pub(crate) fn moved(&self, stage: &str, done: u64, total: u64) {
        let mut listed = operations();
        let Some(operation) = listed.get_mut(&self.id) else { return };
        if operation.stage != stage {
            stage.clone_into(&mut operation.stage);
        }
        // An install's are its tool's output, not what it fetches: its stage says how far it is.
        if (done == 0 && total == 0) || operation.kind == Kind::Install {
            return;
        }
        let now = Instant::now();
        let seen = &mut operation.seen;
        if seen.back().is_some_and(|(_, last)| done < *last) {
            seen.clear();
        }
        seen.push_back((now, done));
        while seen.len() > 2 && seen.front().is_some_and(|(at, _)| now - *at > WINDOW) {
            seen.pop_front();
        }
        (operation.done, operation.total) = (done, total);
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut listed = operations();
        let Some(operation) = listed.get_mut(&self.id) else { return };
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
            (!runs.is_empty()).then(|| v1::Operation {
                id: crate::private_io::unscoped_key(id),
                kind: operation.kind.word().into(),
                subject: operation.subject.clone(),
                stage: operation.stage.clone(),
                bytes_done: operation.done,
                bytes_total: operation.total,
                bytes_here: operation.here,
                bytes_per_s: rate(&operation.seen),
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
