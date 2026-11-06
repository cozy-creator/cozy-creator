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
use protocol::{
    calcifer::{Event, Machine, StreamEvent},
    error::Error,
    ids::RunId,
    project::Card,
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
    time::{Instant, Interval, MissedTickBehavior},
};
use tracing::warn;

use crate::{api::Api, machines::Machines, project};

/// How often an open stream looks for edits made outside Calcifer. A look is a few stats, on every OS and on any filesystem, and a hand's
/// save shows on an open canvas within this and the page's one read.
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
    let id = project::project_id(dir);
    let mut revs = PROJECTS.revs.lock().expect("revs lock");
    let rev = project::revision(dir);
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

/// `dir`'s project opens, before anything reads it: only a change from here on is news.
pub(crate) fn project_opened(dir: &Path) {
    let mut revs = PROJECTS.revs.lock().expect("revs lock");
    revs.entry(project::project_id(dir)).or_insert_with(|| project::revision(dir));
}

/// Tells every stream these rows of project `id`'s canvas changed; none is a row gone.
pub fn canvas(id: &str, etag: &str, cards: BTreeMap<String, Option<Card>>) {
    let event = StreamEvent::Canvas { id: id.to_owned(), etag: etag.to_owned(), cards };
    let _ = PROJECTS.said.send(event);
}

/// Tells every stream project `id`'s history moved, and what undo and redo would take now.
pub fn history(id: &str, undo: Option<Box<protocol::project::HistoryEntry>>, redo: Option<Box<protocol::project::HistoryEntry>>) {
    let _ = PROJECTS.said.send(StreamEvent::History { id: id.to_owned(), undo, redo });
}

/// Tells every stream what project `id` is fetching ahead moved.
pub fn ahead_updated(id: &str, ahead: &protocol::project::Ahead) {
    let _ = PROJECTS.said.send(StreamEvent::AheadUpdated { project: id.to_owned(), ahead: Box::new(ahead.clone()) });
}

/// Tells every stream one job of project `id` changed.
pub fn job_updated(id: &str, job: &protocol::project::Job, seq: u64) {
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
    let mut looks = tokio::time::interval(LOOK);
    looks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let hello = StreamEvent::Hello { version: env!("CARGO_PKG_VERSION").into() };
    let watch = Watch {
        pending: VecDeque::from([hello]),
        runs,
        owed: VecDeque::new(),
        machines_changed,
        machines_stale: false,
        shown,
        said,
        looks,
        stop: api.stop_signal(),
        api,
    };
    Sse::new(futures_util::stream::unfold(watch, Watch::next)).keep_alive(KeepAlive::default())
}

/// One subscriber: what it owes, and the machine list as it last told it.
struct Watch {
    pending: VecDeque<StreamEvent>,
    runs: broadcast::Receiver<(RunId, Event)>,
    /// The runs it owes an update, in the order they changed.
    owed: VecDeque<RunId>,
    machines_changed: watch::Receiver<()>,
    machines_stale: bool,
    shown: HashMap<String, Machine>,
    said: broadcast::Receiver<StreamEvent>,
    looks: Interval,
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
                _ = self.looks.tick() => self.api.projects.look(),
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
