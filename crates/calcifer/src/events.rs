//! `GET /v1/events`: what a client keeps on screen, on one stream. A browser holds six connections to
//! Calcifer across all its tabs, so runs, machines and projects share this one.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, atomic::{AtomicU64, Ordering}},
    time::Duration,
};

use axum::{
    extract::{Query, State, rejection::QueryRejection},
    response::sse::{self, KeepAlive, Sse},
};
use futures_util::Stream;
use api::{
    calcifer::{Event, Machine, ProjectEventCursor, ProjectShowReply, ProjectShowRefusal, Shown, StreamEvent},
    error::Error,
    ids::RunId,
    calcifer::project::Card,
};
use tokio::{
    sync::{
        Notify,
        broadcast::{
            self,
            error::{RecvError, TryRecvError},
        },
        watch,
    },
    time::Instant,
};
use tracing::warn;
use serde::Deserialize;

use crate::{api::Api, machines::Machines, project};

/// How often, while anyone listens, what changes in an open project with no write of Calcifer's is looked for: a file into or out of
/// `files/`, another program's commit to project.db. A look is a stat and a pragma a project, and an open canvas shows it within this
/// and its one read.
const LOOK: Duration = Duration::from_millis(250);

/// What projects say to every stream. Process-wide, as `folder::WRITING` is: one process holds one
/// Calcifer, and a `Folder` that writes may be no more than a path.
static PROJECTS: LazyLock<Projects> = LazyLock::new(|| {
    let mut generation = [0u8; 16];
    getrandom::fill(&mut generation).expect("the OS gives random bytes");
    Projects {
        generation: hex::encode(generation),
        sequence: AtomicU64::new(0),
        said: broadcast::channel(1024).0,
        revs: Mutex::default(),
        owed: Mutex::default(),
        wake: Notify::new(),
    }
});

struct Projects {
    generation: String,
    sequence: AtomicU64,
    said: broadcast::Sender<StreamEvent>,
    /// Each project's `rev` as last noticed, so a write and the look after it say it once.
    revs: Mutex<HashMap<String, String>>,
    /// The projects to say something of, each by id: its folder, and whether it's to be said though neither file changed.
    owed: Mutex<BTreeMap<String, (PathBuf, bool)>>,
    wake: Notify,
}

fn cursor(sequence: u64) -> Option<ProjectEventCursor> {
    // The page compares JSON numbers. Outside their exact integer range, keep ordinary
    // refreshes instead of claiming coverage with a rounded or reused sequence.
    (sequence < (1u64 << 53)).then(|| ProjectEventCursor { generation: PROJECTS.generation.clone(), sequence })
}

/// Capture before any read supplying a full state. Never capture after the snapshot: an
/// event published while it was read may describe a commit that snapshot did not include.
pub(crate) fn snapshot_cursor() -> Option<ProjectEventCursor> {
    cursor(PROJECTS.sequence.load(Ordering::Acquire))
}

fn owe(dir: &Path, settled: bool) {
    PROJECTS.owed.lock().expect("owed lock").entry(project::project_id(dir)).or_insert_with(|| (dir.to_owned(), false)).1 |= settled;
    PROJECTS.wake.notify_one();
}

/// Tells every stream if project.yaml or project.lock in `dir` changed since it last did. A write is never held up working out what it changed.
pub(crate) fn project_changed(dir: &Path) {
    moved(dir, || project::revision(dir));
}

/// As `project_changed`, of the revision a look read (`Projects::look`).
pub(crate) fn project_looked(dir: &Path, rev: String) {
    moved(dir, || rev);
}

fn moved(dir: &Path, rev: impl FnOnce() -> String) {
    let id = project::project_id(dir);
    let mut revs = PROJECTS.revs.lock().expect("revs lock");
    let rev = rev();
    if revs.get(&id) != Some(&rev) {
        revs.insert(id, rev);
        drop(revs);
        owe(dir, false);
    }
}

/// Tells every stream to read `dir`'s project again though neither file changed: `moving` ended, say.
pub(crate) fn project_settled(dir: &Path) {
    owe(dir, true);
}

/// Says what's owed, one project at a time, however many changes that is by now. A project with no one listening isn't worked out: whoever
/// comes next reads it whole.
pub(crate) fn tell(api: Arc<Api>) {
    let looking = api.clone();
    tokio::spawn(async move {
        let mut looks = tokio::time::interval(LOOK);
        looks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            looks.tick().await;
            if PROJECTS.said.receiver_count() > 0 {
                looking.projects.look();
            }
        }
    });
    tokio::spawn(async move {
        let mut last: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        loop {
            PROJECTS.wake.notified().await;
            let began = Instant::now();
            let owed = std::mem::take(&mut *PROJECTS.owed.lock().expect("owed lock"));
            for (id, (dir, settled)) in owed {
                if PROJECTS.said.receiver_count() == 0 {
                    project::model::forget(&api, &id);
                    continue;
                }
                // Redundant work must leave Told untouched: a later commit can arrive
                // while told reads, so its computed diff must always be published.
                if !settled && last.get(&id) == Some(&project::revision(&dir)) {
                    continue;
                }
                let mut event = project::model::told(&api, id.clone(), &dir, settled).await;
                if let StreamEvent::ProjectChanged { rev, .. } = &event {
                    last.insert(id, rev.clone());
                }
                if let StreamEvent::ProjectChanged { cursor: position, .. } = &mut event {
                    *position = PROJECTS.sequence.try_update(Ordering::AcqRel, Ordering::Acquire, |sequence| sequence.checked_add(1))
                        .ok().and_then(|sequence| cursor(sequence + 1));
                }
                let _ = PROJECTS.said.send(event);
            }
            // As long again before the next: however big a project, telling of it doesn't keep a core busy.
            tokio::time::sleep(began.elapsed()).await;
        }
    });
}

/// Project `id` closed: its `rev` goes, and its next open takes another. What's owed of it is still said.
pub(crate) fn project_closed(id: &str) {
    PROJECTS.revs.lock().expect("revs lock").remove(id);
}

/// `dir`'s project opens, before anything reads it: only a change from here on is news.
pub(crate) fn project_opened(dir: &Path) {
    let mut revs = PROJECTS.revs.lock().expect("revs lock");
    revs.entry(project::project_id(dir)).or_insert_with(|| project::revision(dir));
}

/// Tells every stream these rows of project `id`'s canvas changed; none is a row gone.
pub(crate) fn canvas(id: &str, seq: u64, cards: BTreeMap<String, Option<Card>>) {
    let event = StreamEvent::Canvas { id: id.to_owned(), seq, cards };
    let _ = PROJECTS.said.send(event);
}

/// Tells every stream project `id`'s history moved, and what undo and redo would take now.
pub(crate) fn history(id: &str, undo: Option<Box<api::calcifer::project::HistoryEntry>>, redo: Option<Box<api::calcifer::project::HistoryEntry>>) {
    let _ = PROJECTS.said.send(StreamEvent::History { id: id.to_owned(), undo, redo });
}

/// Tells every stream what project `id` is fetching ahead moved.
pub(crate) fn ahead_updated(id: &str, ahead: &api::calcifer::project::Ahead) {
    let _ = PROJECTS.said.send(StreamEvent::AheadUpdated { project: id.to_owned(), ahead: Box::new(ahead.clone()) });
}

/// Protection changes must reach inactive/global views too; they contain no private
/// project content and remain visible after the project's read authority is closed.
pub(crate) fn project_vault(id: &str, state: &str) {
    let _ = PROJECTS.said.send(StreamEvent::ProjectVault { id: id.to_owned(), state: state.to_owned() });
}

/// Tells every stream project `id`'s render, whole: more than one job of it changed.
pub(crate) fn board_updated(id: &str, board: &api::calcifer::project::Board) {
    let _ = PROJECTS.said.send(StreamEvent::BoardUpdated { project: id.to_owned(), board: Box::new(board.clone()) });
}

/// Tells every stream one job of project `id` changed.
pub(crate) fn job_updated(id: &str, job: &api::calcifer::project::Job, seq: u64) {
    let _ = PROJECTS.said.send(StreamEvent::JobUpdated { project: id.to_owned(), job: Box::new(job.clone()), seq });
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Scope {
    #[default]
    All,
    Runs,
}

#[derive(Default, Deserialize)]
pub(crate) struct Subscription {
    #[serde(default)]
    scope: Scope,
}

/// Subscribed before this answers, so what changes after a client's first read is on the
/// stream. Runs-only subscribers neither read machines nor enable project polling.
pub(crate) async fn stream(
    State(api): State<Arc<Api>>,
    query: Result<Query<Subscription>, QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, crate::api::Refusal> {
    let Query(subscription) = query?;
    let watch = subscribe(api, subscription.scope).await;
    Ok(Sse::new(futures_util::stream::unfold(watch, Watch::next)).keep_alive(KeepAlive::default()))
}

async fn subscribe(api: Arc<Api>, scope: Scope) -> Watch {
    let all = scope == Scope::All;
    let machines_changed = all.then(|| {
        let mut changed = api.calcifer.machines_changed();
        changed.mark_unchanged();
        changed
    });
    let runs = api.calcifer.subscribe();
    let said = all.then(|| PROJECTS.said.subscribe());
    let shown = if all {
        listed(&api.machines).await.unwrap_or_else(|e| {
            warn!("the events stream cannot list the machines: {}", e.message);
            HashMap::new()
        })
    } else { HashMap::new() };
    let showing = all.then(Showing::new);
    let id = showing.as_ref().map(|showing| showing.0.clone()).unwrap_or_else(stream_id);
    let hello = StreamEvent::Hello { version: env!("CARGO_PKG_VERSION").into(), stream: id };
    Watch {
        showing,
        pending: VecDeque::from([hello]),
        runs,
        owed: VecDeque::new(),
        machines_changed,
        machines_stale: false,
        shown,
        said,
        stop: api.stop_signal(),
        api,
    }
}

/// What each stream shows, by its id: the projects it holds open (`PUT /v1/events/{stream}`), until it ends or shows others (cl-823).
static SHOWN: LazyLock<Mutex<HashMap<String, Vec<project::Reader>>>> = LazyLock::new(Mutex::default);

/// Closing storage releases the readers a still-connected page keeps alive. Drop outside SHOWN:
/// the last reader closes SQLite and may itself publish a project-closed event.
pub(crate) fn release_project_readers(id: &str) {
    let mut gone = Vec::new();
    {
        let mut shown = SHOWN.lock().expect("shown lock");
        for readers in shown.values_mut() {
            let mut kept = Vec::with_capacity(readers.len());
            for reader in std::mem::take(readers) {
                if reader.id() == id { gone.push(reader); } else { kept.push(reader); }
            }
            *readers = kept;
        }
    }
    drop(gone);
}

/// A stream's place in `SHOWN`, by its id: what it holds is let go when it ends.
struct Showing(String);

fn stream_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS gives random bytes");
    format!("stream-{}", hex::encode(bytes))
}

impl Showing {
    fn new() -> Self {
        let id = stream_id();
        SHOWN.lock().expect("shown lock").insert(id.clone(), vec![]);
        Self(id)
    }

    fn includes(&self, project: &str) -> bool {
        SHOWN.lock().expect("shown lock").get(&self.0)
            .is_some_and(|readers| readers.iter().any(|reader| reader.id() == project))
    }
}

impl Drop for Showing {
    fn drop(&mut self) {
        // Let go of outside the lock: the last reader of a project closes it.
        let held = SHOWN.lock().expect("shown lock").remove(&self.0);
        drop(held);
    }
}

/// `PUT /v1/events/{stream}` `{projects}`: what the page on `stream` shows now, each project held open while the stream lasts or
/// until it shows others. The reply names those held and preserves each attempted ID's refusal. `404 stream_unknown` once the stream
/// has ended: the next one's `hello` names it.
pub(crate) async fn show(
    State(api): State<Arc<Api>>,
    axum::extract::Path(stream): axum::extract::Path<String>,
    body: Result<axum::body::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<axum::Json<ProjectShowReply>, crate::api::Refusal> {
    let shown: Shown = crate::api::parse(&body?)?;
    let gone = || crate::api::Refusal(Error::new(api::error::Kind::NotFound, "stream_unknown", format!("no stream {stream} is open: it ended")));
    if !SHOWN.lock().expect("shown lock").contains_key(&stream) {
        return Err(gone());
    }
    let mut ids = shown.projects;
    ids.sort();
    ids.dedup();
    // What it showed already is held until these are: a project shown before and now isn't closed between.
    let mut readers = Vec::new();
    let mut reply = ProjectShowReply::default();
    for id in ids.iter().take(SHOWN_MOST) {
        match api.projects.reader(&api, id) {
            Ok(reader) => {
                readers.push(reader);
                reply.held.push(id.clone());
            }
            Err(crate::api::Refusal(error)) => {
                reply.refused.insert(id.clone(), ProjectShowRefusal { status: error.kind.http_status(), error });
            }
        }
    }
    let was = SHOWN.lock().expect("shown lock").get_mut(&stream).map(|held| std::mem::replace(held, readers));
    match was {
        Some(was) => {
            drop(was);
            Ok(axum::Json(reply))
        }
        None => Err(gone()),
    }
}

/// How many projects one stream may hold open: a browser's tabs, each showing one.
const SHOWN_MOST: usize = 64;

/// One subscriber: what it owes, and the machine list as it last told it.
struct Watch {
    /// Its id, and the projects it holds open.
    showing: Option<Showing>,
    pending: VecDeque<StreamEvent>,
    runs: broadcast::Receiver<(RunId, Event)>,
    /// The runs it owes an update, in the order they changed.
    owed: VecDeque<RunId>,
    machines_changed: Option<watch::Receiver<()>>,
    machines_stale: bool,
    shown: HashMap<String, Machine>,
    said: Option<broadcast::Receiver<StreamEvent>>,
    stop: watch::Receiver<bool>,
    api: Arc<Api>,
}

impl Watch {
    async fn next(mut self) -> Option<(Result<sse::Event, Infallible>, Self)> {
        loop {
            if *self.stop.borrow() {
                return None;
            }
            self.arrived()?;
            if let Some(event) = self.pending.pop_front() {
                if !self.visible(&event) { continue; }
                return Some((Ok(frame(&event)), self));
            }
            if let Some(id) = self.owed.pop_front() {
                match self.run(&id) {
                    Some(event) => return Some((Ok(frame(&event)), self)),
                    None => continue,
                }
            }
            if std::mem::take(&mut self.machines_stale) {
                self.list_machines().await;
                continue;
            }
            tokio::select! {
                received = self.runs.recv() => match received {
                    Ok((id, event)) => self.owe(id, &event),
                    Err(RecvError::Lagged(_)) => self.fell_behind(),
                    Err(RecvError::Closed) => return None,
                },
                received = async { match self.said.as_mut() {
                    Some(said) => said.recv().await,
                    None => std::future::pending().await,
                }} => match received {
                    Ok(event) => self.pending.push_back(event),
                    Err(RecvError::Lagged(_)) => self.fell_behind(),
                    Err(RecvError::Closed) => return None,
                },
                changed = async { match self.machines_changed.as_mut() {
                    Some(changed) => changed.changed().await,
                    None => std::future::pending().await,
                }} => {
                    changed.ok()?;
                    self.machines_stale = true;
                }
                () = stopping(&mut self.stop) => return None,
            }
        }
    }

    fn visible(&self, event: &StreamEvent) -> bool {
        let project_id = match event {
            StreamEvent::ProjectVault { .. } => return true,
            StreamEvent::ProjectChanged { id, .. } | StreamEvent::Canvas { id, .. } | StreamEvent::History { id, .. } => Some(id),
            StreamEvent::BoardUpdated { project, .. } | StreamEvent::JobUpdated { project, .. } | StreamEvent::AheadUpdated { project, .. } => Some(project),
            StreamEvent::RunUpdated { run } => {
                return self.api.calcifer.run_store(run.id.as_str()).and_then(|store| store.run(run.id.as_str())).ok().flatten().is_some_and(|row| project::vault::require_path(&row.out_dir).is_ok());
            }
            _ => None,
        };
        project_id.is_none_or(|id| {
            self.showing.as_ref().is_some_and(|showing| showing.includes(id))
                && self.api.calcifer.store().known_project(id).ok().flatten()
                    .is_some_and(|known| project::vault::require_path(&known.path).is_ok())
        })
    }

    /// Takes in whatever else has arrived: a run is sent once, as it is now, and nothing waits behind
    /// a busy run. `None` once Calcifer is going.
    fn arrived(&mut self) -> Option<()> {
        loop {
            match self.runs.try_recv() {
                Ok((id, event)) => self.owe(id, &event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged(_)) => self.fell_behind(),
                Err(TryRecvError::Closed) => return None,
            }
        }
        loop {
            let Some(said) = self.said.as_mut() else { return Some(()) };
            match said.try_recv() {
                Ok(event) => self.pending.push_back(event),
                Err(TryRecvError::Empty) => return Some(()),
                Err(TryRecvError::Lagged(_)) => self.fell_behind(),
                Err(TryRecvError::Closed) => return None,
            }
        }
    }

    /// Owes `id` an update unless `event` is a log line that warns of nothing or byte growth, or it is owed one already. A
    /// run's lifecycle also moves its machine's running and queued counts.
    fn owe(&mut self, id: RunId, event: &Event) {
        self.machines_stale |= self.machines_changed.is_some() && matches!(
            event,
            Event::Created { .. }
                | Event::Queued { .. }
                | Event::InProgress { .. }
                | Event::Completed { .. }
                | Event::Failed { .. }
                | Event::Canceled { .. }
        );
        // A warning in its log is said on its record (`Calcifer::record`): the rest of its log is not.
        let shown = match event {
            Event::Log { level, .. } => crate::warns(level),
            Event::OutputDelta { checkpoint: None, .. } => false,
            _ => true,
        };
        if shown && !self.owed.contains(&id) {
            self.owed.push_back(id);
        }
    }

    /// Something was skipped: the client reads again, and so does this, of the machines.
    fn fell_behind(&mut self) {
        self.machines_stale = self.machines_changed.is_some();
        self.resync();
    }

    fn resync(&mut self) {
        if !self.pending.iter().any(|event| matches!(event, StreamEvent::Resync)) {
            self.pending.push_back(StreamEvent::Resync);
        }
    }

    /// `id` as it is now; none when it is gone, or can't be read, which says `resync`.
    fn run(&mut self, id: &RunId) -> Option<StreamEvent> {
        let calcifer = &self.api.calcifer;
        let run = match calcifer.run_store(id.as_str()).and_then(|store| store.run(id.as_str())) {
            Ok(Some(row)) if project::vault::require_path(&row.out_dir).is_ok() => calcifer.record(&row),
            Ok(Some(_)) => return None,
            Ok(None) => return None,
            Err(e) => Err(crate::Error::Store(e)),
        };
        match run {
            Ok(run) => Some(StreamEvent::RunUpdated { run: Box::new(run) }),
            Err(e) => {
                warn!(run = %id, "the events stream cannot read the run: {e}");
                self.resync();
                None
            }
        }
    }

    /// Owes each machine that differs from the list as last told, and each that left it.
    async fn list_machines(&mut self) {
        let now = match listed(&self.api.machines).await {
            Ok(now) => now,
            Err(e) => {
                warn!("the events stream cannot list the machines: {}", e.message);
                return self.resync();
            }
        };
        for (name, machine) in &now {
            if self.shown.get(name) != Some(machine) {
                let machine = Box::new(machine.clone());
                self.pending.push_back(StreamEvent::MachineUpdated { machine });
            }
        }
        for name in self.shown.keys().filter(|name| !now.contains_key(*name)) {
            let name = name.clone();
            self.pending.push_back(StreamEvent::MachineRemoved { name });
        }
        self.shown = now;
    }
}

async fn stopping(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

fn frame(event: &StreamEvent) -> sse::Event {
    sse::Event::default().data(serde_json::to_string(event).expect("an event serializes"))
}

/// The machines a list shows now, by name, from what Calcifer holds (no Hub is asked).
async fn listed(machines: &Arc<Machines>) -> Result<HashMap<String, Machine>, Error> {
    let list = machines.list(false).await?;
    Ok(list.machines.into_iter().map(|m| (m.name.clone(), m)).collect())
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
