//! `GET /v1/events`: what a client keeps on screen, on one stream. A browser holds six connections to
//! Calcifer across all its tabs, so runs, machines and projects share this one.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use axum::{
    extract::State,
    response::sse::{self, KeepAlive, Sse},
};
use futures_util::Stream;
use api::{
    calcifer::{Event, Machine, Shown, StreamEvent},
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

use crate::{api::Api, machines::Machines, project};

/// How often, while anyone listens, what changes in an open project with no write of Calcifer's is looked for: a file into or out of
/// `files/`, another program's commit to project.db. A look is a stat and a pragma a project, and an open canvas shows it within this
/// and its one read.
const LOOK: Duration = Duration::from_millis(250);

/// What projects say to every stream. Process-wide, as `folder::WRITING` is: one process holds one
/// Calcifer, and a `Folder` that writes may be no more than a path.
static PROJECTS: LazyLock<Projects> = LazyLock::new(|| Projects {
    said: broadcast::channel(1024).0,
    revs: Mutex::default(),
    owed: Mutex::default(),
    wake: Notify::new(),
});

struct Projects {
    said: broadcast::Sender<StreamEvent>,
    /// Each project's `rev` as last noticed, so a write and the look after it say it once.
    revs: Mutex<HashMap<String, String>>,
    /// The projects to say something of, each by id: its folder, and whether it's to be said though neither file changed.
    owed: Mutex<BTreeMap<String, (PathBuf, bool)>>,
    wake: Notify,
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
                let event = project::model::told(&api, id.clone(), &dir, settled).await;
                // A write that came as this read the files is told here and owed again: once is enough.
                if let StreamEvent::ProjectChanged { rev, .. } = &event
                    && last.insert(id, rev.clone()).as_ref() == Some(rev)
                {
                    continue;
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

/// Project `id` is forgotten: nothing more is said of it.
pub(crate) fn project_forgotten(id: &str) {
    PROJECTS.revs.lock().expect("revs lock").remove(id);
    PROJECTS.owed.lock().expect("owed lock").remove(id);
}

/// `dir`'s project opens, before anything reads it: only a change from here on is news.
pub(crate) fn project_opened(dir: &Path) {
    let mut revs = PROJECTS.revs.lock().expect("revs lock");
    revs.entry(project::project_id(dir)).or_insert_with(|| project::revision(dir));
}

/// Tells every stream these rows of project `id`'s canvas changed; none is a row gone.
pub fn canvas(id: &str, seq: u64, cards: BTreeMap<String, Option<Card>>) {
    let event = StreamEvent::Canvas { id: id.to_owned(), seq, cards };
    let _ = PROJECTS.said.send(event);
}

/// Tells every stream project `id`'s history moved, and what undo and redo would take now.
pub fn history(id: &str, undo: Option<Box<api::calcifer::project::HistoryEntry>>, redo: Option<Box<api::calcifer::project::HistoryEntry>>) {
    let _ = PROJECTS.said.send(StreamEvent::History { id: id.to_owned(), undo, redo });
}

/// Tells every stream what project `id` is fetching ahead moved.
pub fn ahead_updated(id: &str, ahead: &api::calcifer::project::Ahead) {
    let _ = PROJECTS.said.send(StreamEvent::AheadUpdated { project: id.to_owned(), ahead: Box::new(ahead.clone()) });
}

/// Tells every stream one job of project `id` changed.
pub fn job_updated(id: &str, job: &api::calcifer::project::Job, seq: u64) {
    let _ = PROJECTS.said.send(StreamEvent::JobUpdated { project: id.to_owned(), job: Box::new(job.clone()), seq });
}

/// Subscribed to everything before this answers, so what changes after a client's first read is on
/// the stream. Falling behind sends `resync`; the stream ends only when Calcifer stops.
pub(crate) async fn stream(
    State(api): State<Arc<Api>>,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
    let mut machines_changed = api.calcifer.machines_changed();
    machines_changed.mark_unchanged();
    let runs = api.calcifer.subscribe();
    let said = PROJECTS.said.subscribe();
    let shown = listed(&api.machines).await.unwrap_or_else(|e| {
        warn!("the events stream cannot list the machines: {}", e.message);
        HashMap::new()
    });
    let showing = Showing::new();
    let hello = StreamEvent::Hello { version: env!("CARGO_PKG_VERSION").into(), stream: showing.0.clone() };
    let watch = Watch {
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
    };
    Sse::new(futures_util::stream::unfold(watch, Watch::next)).keep_alive(KeepAlive::default())
}

/// What each stream shows, by its id: the projects it holds open (`PUT /v1/events/{stream}`), until it ends or shows others (cl-823).
static SHOWN: LazyLock<Mutex<HashMap<String, Vec<project::Reader>>>> = LazyLock::new(Mutex::default);

/// A stream's place in `SHOWN`, by its id: what it holds is let go when it ends.
struct Showing(String);

impl Showing {
    fn new() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS gives random bytes");
        let id = format!("stream-{}", hex::encode(bytes));
        SHOWN.lock().expect("shown lock").insert(id.clone(), vec![]);
        Self(id)
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
/// until it shows others. One that doesn't open isn't held: the page's own read of it says why. `404 stream_unknown` once the stream
/// has ended: the next one's `hello` names it.
pub(crate) async fn show(
    State(api): State<Arc<Api>>,
    axum::extract::Path(stream): axum::extract::Path<String>,
    body: Result<axum::body::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<axum::http::StatusCode, crate::api::Refusal> {
    let shown: Shown = crate::api::parse(&body?)?;
    let gone = || crate::api::Refusal(Error::new(api::error::Kind::NotFound, "stream_unknown", format!("no stream {stream} is open: it ended")));
    if !SHOWN.lock().expect("shown lock").contains_key(&stream) {
        return Err(gone());
    }
    let mut ids = shown.projects;
    ids.sort();
    ids.dedup();
    // What it showed already is held until these are: a project shown before and now isn't closed between.
    let readers: Vec<project::Reader> = ids.iter().take(SHOWN_MOST).filter_map(|id| api.projects.reader(&api, id).ok()).collect();
    let was = SHOWN.lock().expect("shown lock").get_mut(&stream).map(|held| std::mem::replace(held, readers));
    match was {
        Some(was) => {
            drop(was);
            Ok(axum::http::StatusCode::NO_CONTENT)
        }
        None => Err(gone()),
    }
}

/// How many projects one stream may hold open: a browser's tabs, each showing one.
const SHOWN_MOST: usize = 64;

/// One subscriber: what it owes, and the machine list as it last told it.
struct Watch {
    /// Its id, and the projects it holds open.
    #[allow(dead_code, reason = "held for its drop")]
    showing: Showing,
    pending: VecDeque<StreamEvent>,
    runs: broadcast::Receiver<(RunId, Event)>,
    /// The runs it owes an update, in the order they changed.
    owed: VecDeque<RunId>,
    machines_changed: watch::Receiver<()>,
    machines_stale: bool,
    shown: HashMap<String, Machine>,
    said: broadcast::Receiver<StreamEvent>,
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
                received = self.said.recv() => match received {
                    Ok(event) => self.pending.push_back(event),
                    Err(RecvError::Lagged(_)) => self.fell_behind(),
                    Err(RecvError::Closed) => return None,
                },
                changed = self.machines_changed.changed() => {
                    changed.ok()?;
                    self.machines_stale = true;
                }
                () = stopping(&mut self.stop) => return None,
            }
        }
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
            match self.said.try_recv() {
                Ok(event) => self.pending.push_back(event),
                Err(TryRecvError::Empty) => return Some(()),
                Err(TryRecvError::Lagged(_)) => self.fell_behind(),
                Err(TryRecvError::Closed) => return None,
            }
        }
    }

    /// Owes `id` an update unless `event` is a log line or byte growth, or it is owed one already. A
    /// run's lifecycle also moves its machine's running and queued counts.
    fn owe(&mut self, id: RunId, event: &Event) {
        self.machines_stale |= matches!(
            event,
            Event::Created { .. }
                | Event::Queued { .. }
                | Event::InProgress { .. }
                | Event::Completed { .. }
                | Event::Failed { .. }
                | Event::Canceled { .. }
        );
        let shown = !matches!(event, Event::Log { .. } | Event::OutputDelta { checkpoint: None, .. });
        if shown && !self.owed.contains(&id) {
            self.owed.push_back(id);
        }
    }

    /// Something was skipped: the client reads again, and so does this, of the machines.
    fn fell_behind(&mut self) {
        self.machines_stale = true;
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
        let run = match calcifer.store().run(id.as_str()) {
            Ok(Some(row)) => calcifer.record(&row),
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
