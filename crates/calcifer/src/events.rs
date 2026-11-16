//! `GET /v1/events`: what a client keeps on screen, on one stream. A browser holds six connections to
//! Calcifer across all its tabs, so runs, machines and projects share this one.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, atomic::{AtomicU64, Ordering}},
};

use axum::{
    extract::{Query, State, rejection::QueryRejection},
    response::sse::{self, KeepAlive, Sse},
};
use futures_util::Stream;
use api::{
    calcifer::{Event, Machine, ProjectEventCursor, ProjectShowReply, ProjectShowRefusal, Run, Shown, StreamEvent},
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
        stirred: Mutex::default(),
        stirring: Notify::new(),
    }
});

/// Moves whenever a project's revision does (a commit of this Calcifer's, or another program's its look read): what a walk looks at
/// its deletions on, never a clock (xs-189).
static EDITS: LazyLock<tokio::sync::watch::Sender<u64>> = LazyLock::new(|| tokio::sync::watch::channel(0).0);

/// Told each time a project's revision moves.
pub(crate) fn edits() -> tokio::sync::watch::Receiver<u64> {
    EDITS.subscribe()
}

struct Projects {
    generation: String,
    sequence: AtomicU64,
    said: broadcast::Sender<StreamEvent>,
    /// Each project's `rev` as last noticed, so a write and the look after it say it once.
    revs: Mutex<HashMap<String, String>>,
    /// The projects to say something of, each by id: its folder, and whether it's to be said though neither file changed.
    owed: Mutex<BTreeMap<String, (PathBuf, bool)>>,
    wake: Notify,
    /// The open projects whose folders changed since they were last looked at (`project_stirred`).
    stirred: Mutex<std::collections::BTreeSet<String>>,
    stirring: Notify,
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

/// Tells every stream if the project in `dir` changed since it last did. A write is never held up working out what it changed.
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
        EDITS.send_modify(|n| *n += 1);
        owe(dir, false);
    }
}

/// Project `id`'s folder changed: a file into or out of `files/`, a write to project.db, perhaps another program's. It's looked at
/// (`Projects::look`), and told if that moved its revision.
pub(crate) fn project_stirred(id: &str) {
    PROJECTS.stirred.lock().expect("stirred lock").insert(id.to_owned());
    PROJECTS.stirring.notify_one();
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
        loop {
            PROJECTS.stirring.notified().await;
            let stirred = std::mem::take(&mut *PROJECTS.stirred.lock().expect("stirred lock"));
            // With no one listening there is no one to tell: whoever comes next reads it whole.
            if PROJECTS.said.receiver_count() > 0 {
                let _took = crate::took::Took::new("look_projects");
                looking.projects.look(&stirred);
            }
        }
    });
    tokio::spawn(async move {
        let mut last: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        loop {
            PROJECTS.wake.notified().await;
            let began = Instant::now();
            let took = crate::took::Took::new("tell_projects");
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
            drop(took);
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

/// The streams showing project `id` now.
pub(crate) fn showing(id: &str) -> Vec<String> {
    SHOWN.lock().expect("shown lock").iter()
        .filter(|(_, shows)| shows.readers.iter().any(|reader| reader.id() == id))
        .map(|(stream, _)| stream.clone()).collect()
}

/// Project `id` held again by each of `streams` that's still going (cl-1097): renamed, it was let go, and the pages that showed it
/// go on showing it.
pub(crate) fn show_again(api: &Arc<Api>, streams: &[String], id: &str) {
    for stream in streams {
        let Ok(reader) = api.projects.reader(api, id) else { return };
        if let Some(shows) = SHOWN.lock().expect("shown lock").get_mut(stream) { shows.readers.push(reader); }
    }
}

/// Tells every stream the index changed (cl-1097): a project renamed or deleted.
pub(crate) fn projects_changed() {
    let _ = PROJECTS.said.send(StreamEvent::ProjectsChanged);
}

/// Tells every stream project `id` is gone, deleted here (cl-1097): what showed it no longer holds it.
pub(crate) fn project_deleted(id: &str, path: &std::path::Path) {
    let _ = PROJECTS.said.send(StreamEvent::ProjectGone { id: id.to_owned(), path: path.to_string_lossy().into_owned() });
}

/// Tells every stream how the page looks everywhere now (cl-1097).
pub(crate) fn appearance(appearance: &api::calcifer::Appearance) {
    let _ = PROJECTS.said.send(StreamEvent::Appearance { appearance: appearance.clone() });
}

/// Tells every stream project `id`'s render, whole: more than one job of it changed.
pub(crate) fn board_updated(id: &str, board: &api::calcifer::project::Board) {
    let _ = PROJECTS.said.send(StreamEvent::BoardUpdated { project: id.to_owned(), board: Box::new(board.clone()) });
}

/// Tells every stream showing project `id` that a thumbnail of its file `sha256` was made, or can't be.
pub(crate) fn thumb(id: &str, sha256: &str) {
    let _ = PROJECTS.said.send(StreamEvent::Thumb { project: id.to_owned(), sha256: sha256.to_owned() });
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
/// stream. Runs-only subscribers read neither machines nor projects.
pub(crate) async fn stream(
    State(api): State<Arc<Api>>,
    query: Result<Query<Subscription>, QueryRejection>,
    client: Option<axum::Extension<crate::loopback::Client>>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, crate::api::Refusal> {
    let Query(subscription) = query?;
    let page = matches!(client, Some(axum::Extension(crate::loopback::Client::Page)));
    let watch = subscribe(api, subscription.scope, page).await;
    Ok(Sse::new(futures_util::stream::unfold(watch, Watch::next)).keep_alive(KeepAlive::default()))
}

async fn subscribe(api: Arc<Api>, scope: Scope, page: bool) -> Watch {
    let all = scope == Scope::All;
    let machines_changed = all.then(|| {
        let mut changed = api.calcifer.machines_changed();
        changed.mark_unchanged();
        changed
    });
    let runs = api.calcifer.subscribe();
    let said = all.then(|| PROJECTS.said.subscribe());
    let shown = if all {
        listed(&api.machines, &[]).await.unwrap_or_else(|e| {
            warn!("the events stream cannot list the machines: {}", e.message);
            HashMap::new()
        })
    } else { HashMap::new() };
    let showing = all.then(|| Showing::new(page));
    let id = showing.as_ref().map(|showing| showing.0.clone()).unwrap_or_else(stream_id);
    let hello = StreamEvent::Hello { version: ::api::VERSION.into(), stream: id };
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

/// What each stream shows, by its id: the projects it holds open (`PUT /v1/events/{stream}`), until it ends or shows others (cl-823),
/// and whether it shows a machine's view.
static SHOWN: LazyLock<Mutex<HashMap<String, Shows>>> = LazyLock::new(Mutex::default);

#[derive(Default)]
struct Shows {
    readers: Vec<project::Reader>,
    machines: bool,
    /// Calcifer's page, not this computer's cozy: what it typed is asked for before a project closes (`save_first`).
    page: bool,
}

/// Closing storage releases the readers a still-connected page keeps alive. Drop outside SHOWN:
/// the last reader closes SQLite and may itself publish a project-closed event.
pub(crate) fn release_project_readers(id: &str) {
    let mut gone = Vec::new();
    {
        let mut shown = SHOWN.lock().expect("shown lock");
        for shows in shown.values_mut() {
            let mut kept = Vec::with_capacity(shows.readers.len());
            for reader in std::mem::take(&mut shows.readers) {
                if reader.id() == id { gone.push(reader); } else { kept.push(reader); }
            }
            shows.readers = kept;
        }
    }
    drop(gone);
}

/// What each page was asked to send before its project closes, by stream and project, until it answers or its stream ends.
static SAVING: LazyLock<Mutex<std::collections::HashSet<(String, String)>>> = LazyLock::new(Mutex::default);
/// Told as a page answers, ends or shows other projects.
static SAVED: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Before projects `ids` close (a lock, a storage change, a stop): each page's stream showing one hears `project.saving`, and
/// the page sends what it typed of it, then answers (`saved`). Waits for each answer or stream end, at most `within`: how many didn't answer.
pub(crate) async fn save_first(ids: &[String], within: std::time::Duration) -> usize {
    let asked: Vec<(String, String)> = SHOWN.lock().expect("shown lock").iter().filter(|(_, shows)| shows.page)
        .flat_map(|(stream, shows)| shows.readers.iter().filter(|reader| ids.iter().any(|id| id == reader.id())).map(|reader| (stream.clone(), reader.id().to_owned())))
        .collect();
    if asked.is_empty() { return 0; }
    SAVING.lock().expect("saving lock").extend(asked.iter().cloned());
    // Let go of however this ends, a stop whose caller went away included.
    let _asked = Asked(&asked);
    for id in ids { let _ = PROJECTS.said.send(StreamEvent::ProjectSaving { id: id.clone() }); }
    let until = Instant::now() + within;
    loop {
        let mut told = std::pin::pin!(SAVED.notified());
        told.as_mut().enable();
        let left = asked.iter().filter(|asked| owes(asked)).count();
        if left == 0 || Instant::now() >= until { break left; }
        tokio::select! {
            () = told => {}
            () = tokio::time::sleep_until(until) => {}
        }
    }
}

/// One `save_first`'s asks, let go of as it ends.
struct Asked<'a>(&'a [(String, String)]);

impl Drop for Asked<'_> {
    fn drop(&mut self) {
        let mut saving = SAVING.lock().expect("saving lock");
        for asked in self.0 { saving.remove(asked); }
    }
}

/// Whether `stream` still owes its answer for `project`: asked, not answered, and showing it still.
fn owes(asked: &(String, String)) -> bool {
    let unanswered = SAVING.lock().expect("saving lock").contains(asked);
    unanswered && SHOWN.lock().expect("shown lock").get(&asked.0).is_some_and(|shows| shows.readers.iter().any(|reader| reader.id() == asked.1))
}

#[derive(Deserialize)]
pub(crate) struct Saved {
    project: String,
}

/// `POST /v1/events/{stream}/saved` `{project}`: the page on `stream` sent what it typed of `project` (`save_first`).
pub(crate) async fn saved(
    axum::extract::Path(stream): axum::extract::Path<String>,
    body: Result<axum::body::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<axum::http::StatusCode, crate::api::Refusal> {
    let Saved { project } = crate::api::parse(&body?)?;
    SAVING.lock().expect("saving lock").remove(&(stream, project));
    SAVED.notify_waiters();
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// A stream's place in `SHOWN`, by its id: what it holds is let go when it ends.
struct Showing(String);

fn stream_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS gives random bytes");
    format!("stream-{}", hex::encode(bytes))
}

impl Showing {
    fn new(page: bool) -> Self {
        let id = stream_id();
        SHOWN.lock().expect("shown lock").insert(id.clone(), Shows { page, ..Shows::default() });
        Self(id)
    }

    fn includes(&self, project: &str) -> bool {
        SHOWN.lock().expect("shown lock").get(&self.0)
            .is_some_and(|shows| shows.readers.iter().any(|reader| reader.id() == project))
    }

    /// Whether this stream is Calcifer's page's.
    fn page(&self) -> bool {
        SHOWN.lock().expect("shown lock").get(&self.0).is_some_and(|shows| shows.page)
    }

    /// Whether this stream's page shows a machine's view.
    fn machines(&self) -> bool {
        SHOWN.lock().expect("shown lock").get(&self.0).is_some_and(|shows| shows.machines)
    }

    /// The projects this stream's page shows.
    fn projects(&self) -> Vec<String> {
        SHOWN.lock().expect("shown lock").get(&self.0).map(|shows| shows.readers.iter().map(|reader| reader.id().to_owned()).collect()).unwrap_or_default()
    }

    /// Whether this stream hears `run`: one of a project it shows, or one of no project's while it shows a machine's view. A project's
    /// runs are its own, as its other events are (Paul: projects are firewalled).
    fn hears(&self, run: &Run) -> bool {
        match &run.project_id {
            Some(project) => self.includes(project),
            None => self.machines(),
        }
    }
}

impl Drop for Showing {
    fn drop(&mut self) {
        // Let go of outside the lock: the last reader of a project closes it.
        let held = SHOWN.lock().expect("shown lock").remove(&self.0);
        drop(held);
        SAVED.notify_waiters();
    }
}

/// `PUT /v1/events/{stream}` `{projects, machines}`: what the page on `stream` shows now, each project held open while the stream lasts or
/// until it shows others, and whether a machine's view. The reply names those held and preserves each attempted ID's refusal. `404 stream_unknown` once the stream
/// has ended: the next one's `hello` names it.
pub(crate) async fn show(
    State(api): State<Arc<Api>>,
    axum::extract::Path(stream): axum::extract::Path<String>,
    body: Result<axum::body::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<axum::Json<ProjectShowReply>, crate::api::Refusal> {
    let shown: Shown = crate::api::parse(&body?)?;
    let gone = || crate::api::Refusal(Error::new(api::error::Kind::NotFound, "stream_unknown", format!("The stream {stream} has ended.")));
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
                reply.refused.insert(id.clone(), ProjectShowRefusal { status: error.http_status(), error });
            }
        }
    }
    let was = SHOWN.lock().expect("shown lock").get_mut(&stream).map(|held| {
        let page = held.page;
        std::mem::replace(held, Shows { readers, machines: shown.machines, page })
    });
    match was {
        Some(was) => {
            // A project this page shows now and didn't is used now: the index lists by it (Paul, 11-16: most recently used first).
            for id in reply.held.iter().filter(|id| !was.readers.iter().any(|reader| reader.id() == id.as_str())) {
                project::used_now(&api, id);
            }
            drop(was);
            SAVED.notify_waiters();
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
            StreamEvent::ProjectVault { .. } | StreamEvent::ProjectsChanged | StreamEvent::Appearance { .. } => return true,
            // A page's alone: this computer's cozy has nothing typed to send.
            StreamEvent::ProjectSaving { id } => return self.showing.as_ref().is_some_and(|showing| showing.page() && showing.includes(id)),
            // Out of the index by now, and let go by every stream: told to all (cl-1097).
            StreamEvent::ProjectGone { .. } => return true,
            StreamEvent::ProjectChanged { id, .. } | StreamEvent::Canvas { id, .. } | StreamEvent::History { id, .. } => Some(id),
            StreamEvent::BoardUpdated { project, .. } | StreamEvent::JobUpdated { project, .. } | StreamEvent::AheadUpdated { project, .. } | StreamEvent::Thumb { project, .. } => Some(project),
            StreamEvent::RunUpdated { run } => {
                return self.showing.as_ref().is_none_or(|showing| showing.hears(run))
                    && self.api.calcifer.run_store(run.id.as_str()).and_then(|store| store.run(run.id.as_str())).ok().flatten().is_some_and(|row| project::vault::require_path(&row.out_dir).is_ok());
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
            Ok(run) if self.showing.as_ref().is_some_and(|showing| !showing.hears(&run)) => None,
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
        let _took = crate::took::Took::new("events_machines");
        // What a machine does for the projects this page shows, unlocked, is theirs to see: never another's.
        let projects: Vec<String> = self.showing.as_ref().map(Showing::projects).unwrap_or_default().into_iter()
            .filter(|id| self.api.calcifer.store().known_project(id).ok().flatten().is_some_and(|known| project::vault::require_path(&known.path).is_ok()))
            .collect();
        for project in &projects {
            if self.api.calcifer.project_store(project).and_then(|store| store.working("local")).is_ok_and(|n| n > 0) {
                self.api.machines.follow_project(project);
            }
        }
        let now = match listed(&self.api.machines, &projects).await {
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
async fn listed(machines: &Arc<Machines>, projects: &[String]) -> Result<HashMap<String, Machine>, Error> {
    let list = machines.list_for(false, projects).await?;
    Ok(list.machines.into_iter().map(|m| (m.name.clone(), m)).collect())
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
