//! Output export: each output item lands at `<out_dir>/<number>-<name>[-<i>]-<identity>` with its
//! media type's extension, rewritten in place as revisions arrive; a project's run keeps it in the
//! project's `files/` too, by hash, which is the file it names from then on. A revision's bytes are read
//! from the run's machine into a `.part` beside the file (ranged and side by side when big) and
//! replace the file whole once they match the revision's digest; `.part.held` beside it keeps the
//! stretches in place, so a transfer cut off resumes with the rest; a tree output is a directory
//! replaced the same way. One exporter works per run with something to export, started by the
//! run's output events or, at startup, from the store. What stays on its machine (`OutputItem::stays`)
//! is no export's: `bring` fetches it when something needs it. `serve` answers
//! `GET /v1/runs/{id}/outputs/{name}` from the exported file.
use std::{
    collections::HashMap,
    io::{Read as _, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    body::Body,
    extract::{Path as Route, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use futures_util::StreamExt as _;
use ::api::{
    calcifer::{Event, MachineState},
    error::Kind,
    ids::{Digest, RunId},
    machine as v1,
    tree,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncSeekExt as _, AsyncWriteExt as _},
    sync::{Notify, broadcast::error::RecvError, watch},
};
use tracing::{info, warn};

use crate::{
    Calcifer, Error, Link, Pause,
    api::{self, Api, Refusal},
    lanes::{self, Ranged},
    link::LANES,
    machine_client::{self, Bulk},
    lock,
    store::{Measured, OutputRow, RunRow, State as RunState, StoreError},
};

/// Bytes are read in ranges of `clamp(length / (RANGES_PER_LANE * LANES), 1 MiB, RANGE)`, at
/// most `LANES` side by side on their own connections. More ranges than lanes, as an S3 client
/// cuts parts: a lane that finishes early takes the next from the queue, so no lane trails alone.
const RANGE: u64 = 8 << 20;
const RANGES_PER_LANE: usize = 4;
/// How many whole reads of one revision may fail its digest before it is given up.
const WRONG_READS: u32 = 3;

/// An output Calcifer still owes this computer: its run's number, its name and index, its length
/// and the bytes of it in place.
pub(crate) struct Owed {
    pub number: u64,
    pub name: String,
    pub index: u32,
    pub length: u64,
    pub moved: u64,
}

impl std::fmt::Display for Owed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let length = crate::bytes(self.length as f64);
        match self.index {
            0 => write!(f, "run {} {} ({length})", self.number, self.name),
            i => write!(f, "run {} {}/{i} ({length})", self.number, self.name),
        }
    }
}

/// The exporters at work, and what they share.
#[derive(Default)]
pub(crate) struct Exports {
    /// One per run, woken by its output events.
    runs: Mutex<HashMap<RunId, Arc<Notify>>>,
    /// Where a revision extends the one before it, by run, item id and revision.
    appended: Mutex<HashMap<(RunId, String, u64), u64>>,
    /// Orders replacing a file and recording it against `serve` reading both.
    placing: Mutex<()>,
    /// The bytes in place of each export in flight, by run, output and index.
    moving: Mutex<HashMap<(RunId, String, u32), Arc<AtomicU64>>>,
}

/// Why an export stopped short.
#[derive(Clone)]
enum Failure {
    /// A newer revision is recorded: that one is exported instead.
    Superseded,
    /// This revision's bytes cannot be had: recorded with the item and not tried again.
    Lost(String),
    /// Tried again after a pause, or once the machine changes.
    Again(String),
    /// Read whole, and its bytes are not this revision's. Tried again [`WRONG_READS`] times in all:
    /// a link can spoil one read, not every one.
    Wrong(String),
    /// The machine's definitive refusal: final once the run has ended (no newer revision can
    /// come to read instead), tried again before.
    Refused(String),
}

impl From<StoreError> for Failure {
    fn from(e: StoreError) -> Self {
        Failure::Again(e.to_string())
    }
}

fn private_path(path: &Path) -> bool {
    crate::project::vault::logical(path).ancestors().any(crate::project::vault::encrypted)
}

fn private_run(run: &RunRow) -> bool { private_path(&run.out_dir) }

fn export_warning(run: &RunRow, detail: impl std::fmt::Display) {
    if private_run(run) { warn!(run = %run.id, "private output transfer waits; open its project for details"); }
    else { warn!(run = %run.id, "{detail}"); }
}

impl Calcifer {
    /// Starts an exporter for each run that owes one, then for each run whose outputs change.
    pub(crate) fn export_outputs(self: &Arc<Self>) -> Result<(), Error> {
        let mut events = self.subscribe();
        for (id, _) in self.each_store(|store| store.exports_owed())?.into_iter().flatten().collect::<Vec<_>>() {
            self.export(&id);
        }
        let me = Arc::downgrade(self);
        self.spawn_task(async move {
            loop {
                let received = events.recv().await;
                let Some(me) = me.upgrade() else { return };
                match received {
                    Ok((
                        id,
                        Event::OutputDelta {
                            item_id,
                            rev,
                            appended_from: Some(from),
                            ..
                        },
                    )) => {
                        lock(&me.exports.appended).insert((id.clone(), item_id, rev), from);
                        me.export(&id);
                    }
                    Ok((
                        id,
                        Event::OutputAdded { .. }
                        | Event::OutputDelta { .. }
                        | Event::OutputDone { .. },
                    )) => me.export(&id),
                    // Untyped bytes were waiting for this: they stay on the machine, or are all that came.
                    Ok((id, event)) if event.is_terminal() => {
                        me.note_stays(&id);
                        me.export(&id);
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => match me.each_store(|store| store.exports_owed()).map(|lists| lists.into_iter().flatten().collect::<Vec<_>>()) {
                        Ok(owed) => owed.iter().for_each(|(id, _)| me.export(id)),
                        Err(e) => warn!("{e}"),
                    },
                    Err(RecvError::Closed) => return,
                }
            }
        });
        Ok(())
    }

    fn spawn_task(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = lock(&self.tasks);
        while tasks.try_join_next().is_some() {}
        tasks.spawn(task);
    }

    /// Starts the run's exporter, or wakes it.
    pub(super) fn export(self: &Arc<Self>, id: &RunId) {
        let wake = {
            let mut runs = lock(&self.exports.runs);
            if let Some(wake) = runs.get(id) {
                wake.notify_one();
                return;
            }
            let wake = Arc::new(Notify::new());
            runs.insert(id.clone(), wake.clone());
            wake
        };
        let (me, id) = (self.clone(), id.clone());
        self.spawn_task(async move { me.export_run(&id, &wake).await });
    }

    /// A project's run that ended: what stays on its machine, in project.lock with who holds it.
    fn note_stays(&self, id: &RunId) {
        let Ok(Some(row)) = self.with_run_store(id.as_str(), |store| store.run(id.as_str())) else { return };
        let (Some(project), Some(machine)) = (crate::project::of_landing(&row.out_dir), &row.machine) else { return };
        let Ok(run) = self.record(&row) else { return };
        for item in run.output.iter().filter(|item| item.stays && item.path.is_none()) {
            let Some(sha256) = &item.sha256 else { continue };
            if let Err(why) = crate::project::stays(project, &sha256.to_string(), machine, item.length) {
                export_warning(&row, format_args!("output {} not in its project lock: {why}", item.name));
            }
        }
    }

    fn making_scoped(&self, digest: &str, machine: &str) -> crate::store::Result<Option<(RunRow, OutputRow)>> {
        match crate::private_io::current() {
            Some(scope) => self.project_store(&scope.storage.project_id)?.making(digest, machine),
            None => self.maintenance_store().making(digest, machine),
        }
    }

    /// Whether `machine` still has what stays on it, a byte of it read as `bring` reads it, or it's on its way here. Only its saying
    /// it holds that no more is a no: one out of reach may have it yet.
    pub(crate) async fn still_holds(&self, digest: &str, machine: &str) -> bool {
        let Ok(Some((run, item))) = self.making_scoped(digest, machine) else { return false };
        // Its export going: `bring` waits for it, so a reader bound now has it (cl-815).
        let owed = self.with_run_store(run.id.as_str(), |store| store.outputs_to_export(run.id.as_str())).unwrap_or_default();
        if owed.iter().any(|owed| owed.name == item.name && owed.index == item.index) {
            return true;
        }
        if run.machine.as_deref() == Some("local")
            && let Some(machines) = self.machines()
            && machines.local_up().await.is_err()
        {
            return true;
        }
        let link = match self.reach(&run, &mut None) {
            Ok(link) => link,
            Err(Failure::Lost(_)) => return false,
            Err(_) => return true,
        };
        let Some(lane) = link.lane(0, false) else { return true };
        let target = v1::OutputTarget { run: run.id.to_string(), output: item.name.clone(), index: item.index, member: String::new() };
        !matches!(lane.get(&target, (0, 1), item.rev).await, Err(machine_client::Error::Refused { status: 404, .. }))
    }

    /// What stays on `machine` brought here now, because something needs it, as an export brings an output: its project's `files/`
    /// holds it after. A machine out of reach is tried again; Err says why it can't be had.
    pub(crate) async fn bring(&self, digest: &str, machine: &str) -> Result<(), String> {
        let made = self.making_scoped(digest, machine).map_err(|e| e.to_string())?;
        let Some((run, item)) = made else {
            return Err(format!("Calcifer knows of no run on {machine} that made it"));
        };
        // On its way here already: that export's landing, and no second read of it.
        let mut events = self.subscribe();
        let mut exporting = false;
        while (self.with_run_store(run.id.as_str(), |store| store.outputs_to_export(run.id.as_str())).map_err(|e| e.to_string())?.iter()).any(|owed| owed.name == item.name && owed.index == item.index) {
            exporting = true;
            tokio::select! {
                _ = events.recv() => {}
                () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
        }
        if exporting {
            let now = self.with_run_store(run.id.as_str(), |store| store.output(run.id.as_str(), &item.name, item.index)).map_err(|e| e.to_string())?;
            return match now.and_then(|now| now.export_error) {
                Some(why) => Err(why),
                None => Ok(()),
            };
        }
        if run.machine.as_deref() == Some("local")
            && let Some(machines) = self.machines()
        {
            machines.local_up().await.map_err(|e| e.message)?;
        }
        let mut backoff = tensorfs::fetch::Backoff::background();
        loop {
            let mut changed = None;
            let done = match self.reach(&run, &mut changed) {
                Ok(link) => self.export_item(&run, &item, &link, changed).await,
                Err(failure) => Err(failure),
            };
            let why = match done {
                Ok(()) => return Ok(()),
                Err(Failure::Superseded) => return Err("a newer revision of it is recorded".into()),
                Err(Failure::Lost(message) | Failure::Refused(message)) => return Err(message),
                Err(Failure::Again(message) | Failure::Wrong(message)) => message,
            };
            let Some((wait, said)) = backoff.again(&why, false, None) else { return Err(why) };
            if !private_run(&run) { info!(run = %run.id, output = %item.name, "bringing it here: {said}"); }
            tokio::time::sleep(wait).await;
        }
    }

    /// Exports the run's outputs, each at its newest revision, until none is left: up to `LANES`
    /// items side by side, so a run's many small outputs cost one round trip together, not one
    /// each. Paces retries while nothing moves; never gives up without recording why.
    async fn export_run(&self, id: &RunId, wake: &Notify) {
        let mut pause = Pause::default();
        let mut wrong: HashMap<(String, u32, u64), u32> = HashMap::new();
        loop {
            let (run, items) = match self.next_exports(id) {
                Ok(Some(next)) => next,
                Ok(None) => return,
                Err(e) => {
                    warn!(run = %id, "{e}");
                    pause.wait(wake, None).await;
                    continue;
                }
            };
            // Outputs left on this computer's machine after it exited idle: it starts for them.
            if run.machine.as_deref() == Some("local")
                && let Some(machines) = self.machines()
                && let Err(e) = machines.local_up().await
            {
                export_warning(&run, format_args!("exporting outputs: {}", e.message));
                pause.wait(wake, None).await;
                continue;
            }
            let mut changed = None;
            let exported = match self.reach(&run, &mut changed) {
                Ok(link) => {
                    let exports = (items.iter())
                        .map(|item| self.export_item(&run, item, &link, changed.clone()));
                    futures_util::future::join_all(exports).await
                }
                Err(failure) => items.iter().map(|_| Err(failure.clone())).collect(),
            };
            let mut again = false;
            for (item, exported) in items.iter().zip(exported) {
                let (name, index, rev) = (&item.name, item.index, item.rev);
                let exported = match exported {
                    Err(Failure::Refused(message)) if run.state == RunState::Settled => {
                        Err(Failure::Lost(format!("the machine refused it ({message})")))
                    }
                    Err(Failure::Wrong(message)) => {
                        let reads = wrong.entry((name.clone(), index, rev)).or_default();
                        *reads += 1;
                        match *reads >= WRONG_READS {
                            true => Err(Failure::Lost(format!("read {reads} times, never its bytes: {message}"))),
                            false => Err(Failure::Again(message)),
                        }
                    }
                    exported => exported,
                };
                match exported {
                    Ok(()) | Err(Failure::Superseded) => {}
                    Err(Failure::Lost(reason)) => {
                        export_warning(&run, format_args!("output {name} revision {rev} not exported: {reason}"));
                        if let Ok(out_dir) = crate::project::vault::resolve_path(&run.out_dir) {
                            let dest = out_dir.join(file_name(run.number, name, index, &item.media_type));
                            remove(&suffixed(&dest, ".part"));
                            remove(&suffixed(&dest, HELD));
                            remove(&suffixed(&dest, ".manifest.part"));
                        }
                        match (self.store).export_failed(id.as_str(), name, index, rev, &reason) {
                            Ok(events) => self.publish(id, events),
                            Err(e) => {
                                warn!(run = %id, "{e}");
                                again = true;
                            }
                        }
                    }
                    Err(Failure::Again(message) | Failure::Refused(message) | Failure::Wrong(message)) => {
                        export_warning(&run, format_args!("output {name} revision {rev} waits: {message}"));
                        again = true;
                    }
                }
            }
            let offline = (run.machine.as_deref()).is_some_and(|m| self.offline(m).is_some());
            match again {
                // Its machine is offline: tried again once it comes back or ends.
                true if offline => tokio::select! {
                    () = machine_changed(&mut changed) => {}
                    () = wake.notified() => {}
                },
                true => pause.wait(wake, changed.as_mut()).await,
                false => pause.reset(),
            }
        }
    }

    /// The run and its first `LANES` outputs to export; none ends the exporter, under the lock
    /// `export` takes, so an event after this starts another.
    fn next_exports(&self, id: &RunId) -> Result<Option<(RunRow, Vec<OutputRow>)>, StoreError> {
        let mut runs = lock(&self.exports.runs);
        let next = match self.with_run_store(id.as_str(), |store| store.run(id.as_str()))? {
            Some(run) => {
                let mut items = self.with_run_store(id.as_str(), |store| store.outputs_to_export(id.as_str()))?;
                items.truncate(LANES);
                Some((run, items)).filter(|(_, items)| !items.is_empty())
            }
            None => None,
        };
        if next.is_none() {
            runs.remove(id);
            lock(&self.exports.appended).retain(|(run, ..), _| run != id);
        }
        Ok(next)
    }

    /// The run's machine's connection, whose bulk lanes read; `changed` is told when the
    /// machine changes.
    fn reach(
        &self,
        run: &RunRow,
        changed: &mut Option<watch::Receiver<()>>,
    ) -> Result<Link, Failure> {
        let Some(name) = &run.machine else {
            return Err(Failure::Lost("the run never reached a machine".into()));
        };
        let (machine, link, watch, _) = self.machine(name).map_err(Failure::Again)?;
        *changed = Some(watch);
        if machine.state == MachineState::Ended {
            let reason = machine.reason.as_deref().unwrap_or("no reason was given");
            return Err(Failure::Lost(format!("its machine {name} ended: {reason}")));
        }
        let link = link.ok_or_else(|| Failure::Again(format!("waiting for {name} to start")))?;
        if name == "local" {
            if let Some(scope) = crate::private_io::of_output(&run.out_dir).map_err(|error| Failure::Again(error.message))? {
                return Ok(link.with_private(scope));
            }
        }
        Ok(link)
    }

    /// Makes the item's file (a directory for a tree) this revision's bytes. A newer revision
    /// stops it; so does its machine's end (`changed` tells of it), which loses it.
    async fn export_item(
        &self,
        run: &RunRow,
        item: &OutputRow,
        link: &Link,
        mut changed: Option<watch::Receiver<()>>,
    ) -> Result<(), Failure> {
        let mut events = self.subscribe();
        let superseded = || -> Result<bool, Failure> {
            let now = self.with_run_store(run.id.as_str(), |store| store.output(run.id.as_str(), &item.name, item.index))?;
            Ok(now.is_none_or(|now| now.rev != item.rev))
        };
        if superseded()? {
            return Err(Failure::Superseded);
        }
        let began = Instant::now();
        if !private_run(run) { info!(run = %run.id, output = %item.name, index = item.index, rev = item.rev,
            bytes = item.length, phase = "output_export", state = "begin"); }
        let name = file_name(run.number, &item.name, item.index, &item.media_type);
        let out_dir = crate::project::vault::resolve_path(&run.out_dir).map_err(|error| Failure::Again(error.message))?;
        let dest = out_dir.join(name);
        let target = v1::OutputTarget {
            run: run.id.to_string(),
            output: item.name.clone(),
            index: item.index,
            member: String::new(),
        };
        let moved = Arc::new(AtomicU64::new(0));
        let key = (run.id.clone(), item.name.clone(), item.index);
        lock(&self.exports.moving).insert(key.clone(), moved.clone());
        let work = async {
            tokio::fs::create_dir_all(&out_dir).await.map_err(|e| {
                Failure::Again(format!("cannot create {}: {e}", out_dir.display()))
            })?;
            let staged = if item.media_type == tree::MEDIA {
                fetch_tree(link, &target, item, &dest, &moved).await?
            } else {
                let appended = (run.id.clone(), item_id(run.number, item), item.rev);
                let appended = lock(&self.exports.appended).get(&appended).copied();
                fetch_file(link, &target, item, &dest, appended, &moved).await?
            };
            let measured = self.measured(item, &staged).await;
            // It waits on its project's turn and the store's commit: off the async workers, which answer the page.
            tokio::task::block_in_place(|| self.land(run, item, &staged, &dest, measured.as_ref()))?;
            remove(&suffixed(&dest, HELD));
            Ok(())
        };
        tokio::pin!(work);
        let done = loop {
            let told = tokio::select! {
                done = &mut work => break done,
                told = events.recv() => told,
                () = machine_changed(&mut changed) => {
                    if let Some(name) = &run.machine
                        && let Ok(Some(machine)) = self.store.machine(name)
                        && machine.state == MachineState::Ended
                    {
                        let reason = machine.reason.as_deref().unwrap_or("no reason was given");
                        break Err(Failure::Lost(format!("its machine {name} ended: {reason}")));
                    }
                    continue;
                }
            };
            let newer = match told {
                Ok((id, event)) => {
                    id == run.id
                        && matches!(
                            event,
                            Event::OutputAdded { .. }
                                | Event::OutputDelta { .. }
                                | Event::OutputDone { .. }
                        )
                }
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => break (&mut work).await,
            };
            match newer.then(superseded) {
                Some(Ok(true)) => break Err(Failure::Superseded),
                Some(Err(e)) => break Err(e),
                _ => {}
            }
        };
        lock(&self.exports.moving).remove(&key);
        if !private_run(run) { info!(run = %run.id, output = %item.name, index = item.index, rev = item.rev,
            bytes = moved.load(Ordering::Relaxed), phase = "output_export", state = "end",
            ok = done.is_ok(), elapsed_ms = began.elapsed().as_secs_f64() * 1000.0); }
        done
    }

    /// The outputs of runs on `machine` that have not landed and are not given up, each with the
    /// bytes its export has in place.
    pub(crate) fn owed_on(&self, machine: &str) -> Result<Vec<Owed>, StoreError> {
        let owed = self.each_store(|store| store.exports_owed_on(machine))?.into_iter().flatten().collect::<Vec<_>>();
        let moving = lock(&self.exports.moving);
        let owed = owed.into_iter().map(|owed| {
            let key = (owed.run, owed.name, owed.index);
            let moved = (moving.get(&key)).map_or(0, |moved| moved.load(Ordering::Relaxed));
            let (_, name, index) = key;
            Owed {
                number: owed.number,
                name,
                index,
                length: owed.length,
                moved,
            }
        });
        Ok(owed.collect())
    }

    /// What's measured of a revision about to land, a picture, video or sound, once, as its project measures its files: the run's record
    /// and its project both say it from the moment it lands. Nothing with no ffprobe here, or of what it doesn't read.
    async fn measured(&self, item: &OutputRow, staged: &Path) -> Option<crate::project::Facts> {
        let machines = self.machines()?;
        let (tools, staged, media_type) = (machines.hubs.home().tools(), staged.to_owned(), item.media_type.clone());
        tokio::task::spawn_blocking(move || crate::project::measured(&tools, &staged, &media_type)).await.ok().flatten()
    }

    /// Puts the staged bytes in the item's place and records them, one step as `serve` sees it: its project's file measured before it's
    /// there, the run's record once it is.
    fn land(
        &self,
        run: &RunRow,
        item: &OutputRow,
        staged: &Path,
        dest: &Path,
        measured: Option<&crate::project::Facts>,
    ) -> Result<(), Failure> {
        let began = Instant::now();
        let old = suffixed(dest, ".old");
        let unplaced =
            |e: std::io::Error| Failure::Again(format!("cannot place {}: {e}", dest.display()));
        let timings = {
            let _placing = lock(&self.exports.placing);
            let lock_ms = began.elapsed().as_secs_f64() * 1000.0;
            let rename_began = Instant::now();
            if let Err(e) = std::fs::rename(staged, dest) {
                // A directory is not replaced by a rename: the old one moves aside first.
                if !(dest.is_dir() || staged.is_dir() && dest.symlink_metadata().is_ok()) {
                    return Err(unplaced(e));
                }
                remove(&old);
                std::fs::rename(dest, &old).map_err(unplaced)?;
                if let Err(e) = std::fs::rename(staged, dest) {
                    let _ = std::fs::rename(&old, dest);
                    return Err(unplaced(e));
                }
            }
            let rename_ms = rename_began.elapsed().as_secs_f64() * 1000.0;
            // Its bytes reached the disk before its name (`check`): the name is durable soon, and no landing waits on everyone's write-back.
            if let Some(directory) = dest.parent() {
                platform::durable::soon(directory);
            }
            let (id, name, index, rev) = (run.id.as_str(), &item.name, item.index, item.rev);
            let project_began = Instant::now();
            let mut unlisted = None;
            let kept = match crate::project::of_landing(&run.out_dir).filter(|_| item.media_type != tree::MEDIA) {
                Some(project) => {
                    let made = crate::project::Landed { run_id: &run.id, run: run.number, target: &run.target, output: name, index, digest: &item.digest, measured };
                    match crate::project::landed(project, &made, dest) {
                        Ok((kept, said)) => {
                            unlisted = said;
                            kept
                        }
                        // The file stands where it landed if `files/` itself can't take it.
                        Err(why) => {
                            unlisted = Some(format!("{name} stands in .runs/, and its project can't keep it: {why}"));
                            dest.to_owned()
                        }
                    }
                }
                None => dest.to_owned(),
            };
            let project_ms = project_began.elapsed().as_secs_f64() * 1000.0;
            let store_began = Instant::now();
            let measured = measured.map_or_else(Measured::default, |facts| Measured {
                width: facts.width as u32,
                height: facts.height as u32,
                duration_ms: facts.lasts_ms(),
            });
            let mut events = self.with_run_store(id, |store| store.exported(id, name, index, rev, &kept, measured))?;
            if let Some(said) = unlisted {
                export_warning(run, &said);
                events.extend(self.with_run_store(id, |store| store.warned(id, &::api::calcifer::Warning { code: "output_unlisted".to_owned(), message: said }))?);
            }
            let store_ms = store_began.elapsed().as_secs_f64() * 1000.0;
            let publish_began = Instant::now();
            self.publish(&run.id, events);
            (
                lock_ms,
                rename_ms,
                project_ms,
                store_ms,
                publish_began.elapsed().as_secs_f64() * 1000.0,
            )
        };
        let (lock_ms, rename_ms, project_ms, store_ms, publish_ms) = timings;
        if !private_run(run) { info!(run = %run.id, output = %item.name, index = item.index, rev = item.rev,
            phase = "output_land", lock_ms, rename_ms, project_ms, store_ms, publish_ms,
            elapsed_ms = began.elapsed().as_secs_f64() * 1000.0); }
        remove(&old);
        Ok(())
    }
}

/// Which item of a list output; 0 (the default) for a single output. A folder's `member`: one of its files, by its tree path.
#[derive(Deserialize)]
pub(crate) struct Item {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    member: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct OutputPath {
    id: String,
    #[serde(default)]
    run: Option<String>,
    name: String,
}

/// `GET /v1/runs/{id|number}/outputs/{name}[?index=i]`: the output's exported file, whole or one
/// `Range` of it, with `ETag: "r<rev>"` naming the revision it holds; otherwise why not. A folder's is its tree document
/// (`api::tree`), and `?member=` one of its files: a client fetches what it asks for, by content.
pub(crate) async fn serve(
    State(api): State<Arc<Api>>,
    Route(path): Route<OutputPath>,
    query: Result<Query<Item>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let Item { index, member } = query?.0;
    let run = path.run.as_deref().unwrap_or(&path.id);
    let project = path.run.as_ref().map(|_| path.id.as_str());
    let name = &path.name;
    let row = api::find_scoped(&api, project, run).await?;
    crate::project::vault::require_path(&row.out_dir).map_err(Refusal)?;
    let readable = crate::project::vault::readable_token(&row.out_dir);
    let number = row.number;
    let (item, file, landed_folder) = {
        let _placing = lock(&api.calcifer.exports.placing);
        let item = api.calcifer.run_store(row.id.as_str())?.output(row.id.as_str(), name, index)?;
        let item = item.ok_or_else(|| {
            let message = format!("run {number} has no output {name} at index {index}");
            Refusal(api::refuse(Kind::NotFound, "output_not_found", message).0.next(format!("cozy run watch {number}")))
        })?;
        let (file, landed_folder) = match (&item.checkpoint, &item.path, item.exported_rev) {
            (None, Some(path), Some(_)) if item.media_type == tree::MEDIA => (None, Some(crate::project::vault::resolve_path(path).map_err(Refusal)?)),
            (None, Some(path), Some(_)) => (Some(std::fs::File::open(crate::project::vault::resolve_path(path).map_err(Refusal)?).map_err(|e| {
                let message = format!("its file {} is gone: {e}", path.display());
                Refusal(api::refuse(Kind::NotFound, "output_file_missing", message).0.next(format!("cozy run watch {number}")))
            })?), None),
            _ => (None, None),
        };
        (item, file, landed_folder)
    };
    if let Some(dir) = landed_folder {
        return folder(&dir, member.as_deref(), number, &name, &headers).await.map(|response| protect(response, readable));
    }
    if let Some(checkpoint) = &item.checkpoint {
        let message = format!(
            "output {name} of run {number} is a TensorFS checkpoint reference: {}@{}",
            checkpoint.model_id, checkpoint.revision
        );
        return Err(api::refuse(
            Kind::Conflict,
            "output_is_a_checkpoint",
            message,
        ));
    }
    let (Some(file), Some(rev)) = (file, item.exported_rev) else {
        let record = api.calcifer.record(&row)?;
        if record.output.iter().any(|kept| kept.stays && kept.name == *name && kept.index == index) {
            let on = row.machine.as_deref().unwrap_or("its machine");
            let message = format!("output {name} of run {number} stays on {on}: it isn't brought to this computer");
            return Err(Refusal(api::refuse(Kind::Conflict, "output_on_its_machine", message).0.next(format!("cozy run watch {number}"))));
        }
        if let Some(reason) = item.export_error {
            let message = format!("output {name} of run {number} will not land: {reason}");
            return Err(Refusal(api::refuse(Kind::Unavailable, "output_not_exported", message).0.next(format!("cozy run watch {number}"))));
        }
        let state = match row.state {
            RunState::Settled => "has ended",
            RunState::Accepted => "is running",
            _ => "is queued",
        };
        let message = format!("output {name} of run {number} has not landed yet; the run {state}");
        let refusal = api::refuse(Kind::Conflict, "output_not_ready", message);
        return Err(Refusal(refusal.0.next(format!("cozy run watch {number}"))));
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut response = crate::files::send(file, len, &item.media_type, &format!("\"r{rev}\""), &headers)
        .map_err(|e| api::refuse(Kind::Internal, "output_unreadable", e.to_string()))?;
    if item.done && rev == item.rev
        && let Ok(digest) = HeaderValue::from_str(&item.digest)
    {
        response.headers_mut().insert("x-cozy-digest", digest);
    }
    Ok(protect(response, readable))
}

/// Every chunk checks the viewing lease, including an already-open range response.
fn protect(mut response: Response, readable: Option<Arc<AtomicBool>>) -> Response {
    let Some(readable) = readable else { return response };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    let body = std::mem::replace(response.body_mut(), Body::empty());
    let chunks = body.into_data_stream().map(move |chunk| {
        if !readable.load(Ordering::Acquire) {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the project is locked"))
        } else { chunk.map_err(std::io::Error::other) }
    });
    *response.body_mut() = Body::from_stream(chunks);
    response
}

/// A folder output: its tree document, or with `member` one of its files, each named by its content so a client caches it.
async fn folder(dir: &Path, member: Option<&str>, number: u64, name: &str, headers: &HeaderMap) -> Result<Response, Refusal> {
    let gone = |e: std::io::Error| {
        let message = format!("its folder {} is gone: {e}", dir.display());
        Refusal(api::refuse(Kind::NotFound, "output_file_missing", message).0.next(format!("cozy run watch {number}")))
    };
    let at = dir.to_owned();
    let staged = tokio::task::spawn_blocking(move || ::api::stage::stage(&at, None))
        .await
        .map_err(|e| api::refuse(Kind::Internal, "output_unreadable", e.to_string()))?
        .map_err(Refusal)?;
    let (file, media_type, etag) = match member {
        None => {
            let digest = ::api::ids::Digest::of(&staged.document);
            let private = crate::private_io::of_path(dir).map_err(Refusal)?.is_some();
            let mut part = if private { tempfile::tempfile_in(dir.parent().unwrap_or(dir)) } else { tempfile::tempfile() }.map_err(gone)?;
            std::io::Write::write_all(&mut part, &staged.document).and_then(|()| std::io::Seek::rewind(&mut part)).map_err(gone)?;
            (part, tree::MEDIA.to_owned(), format!("\"{digest}\""))
        }
        Some(member) => {
            let parts: Vec<&str> = member.split('/').collect();
            let path = parts.iter().fold(dir.to_owned(), |path, part| path.join(part));
            let held = staged.files.iter().find(|file| file.path == path).filter(|_| !parts.iter().any(|part| matches!(*part, "" | "." | "..")));
            let Some(held) = held else {
                let message = format!("output {name} of run {number} holds no {member}");
                return Err(Refusal(api::refuse(Kind::NotFound, "output_member_not_found", message).0.next(format!("cozy run watch {number}"))));
            };
            let media = crate::runs::bind::sniff(&held.path).map(|sniffed| sniffed.media_type).unwrap_or_else(|_| "application/octet-stream".into());
            (std::fs::File::open(&held.path).map_err(gone)?, media, format!("\"{}\"", held.digest))
        }
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    crate::files::send(file, len, &media_type, &etag, headers).map_err(|e| api::refuse(Kind::Internal, "output_unreadable", e.to_string()))
}

/// Where a run that names no directory puts its outputs: its package's folder, `<org>-<name>` as
/// the Go CLI named it (`fidika-minimax-h3`, `local-my-script`), from a target
/// `<org>/<name>/<function>[@release]` (a root package's: `<name>`); the outputs folder itself for
/// any other target.
pub(crate) fn package_outputs(outputs: &Path, target: &str) -> PathBuf {
    let named = |part: &&str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    match target.split('/').collect::<Vec<_>>().as_slice() {
        [org, name, function]
            if *org == ::api::names::ROOT && named(name) && !function.is_empty() =>
        {
            outputs.join(name)
        }
        [org, name, function] if named(org) && named(name) && !function.is_empty() => {
            outputs.join(format!("{org}-{name}"))
        }
        _ => outputs.to_owned(),
    }
}

/// The item's name in its run's output directory (`api::calcifer::output_file_name`).
pub(crate) fn file_name(number: u64, name: &str, index: u32, media_type: &str) -> String {
    ::api::calcifer::output_file_name(number, name, index, media_type)
}

fn item_id(number: u64, item: &OutputRow) -> String {
    match item.index {
        0 => format!("{number}/{}", item.name),
        i => format!("{number}/{}/{i}", item.name),
    }
}

/// Resolves when the machine changes; never without a receiver or once its sender is gone.
async fn machine_changed(changed: &mut Option<watch::Receiver<()>>) {
    let changed = match changed {
        Some(changed) => changed.changed().await.is_ok(),
        None => false,
    };
    if !changed {
        std::future::pending::<()>().await;
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    path.into()
}

fn remove(path: &Path) {
    let _ = std::fs::remove_dir_all(path).or_else(|_| std::fs::remove_file(path));
}

/// This revision's bytes in `<file>.part`, checked against its digest. A revision that extends
/// the one the file holds reads only its new tail; an attempt cut off before resumes with what it
/// left in place (`.part.held`), cleared by a digest that doesn't match.
async fn fetch_file(
    link: &Link,
    target: &v1::OutputTarget,
    item: &OutputRow,
    dest: &Path,
    appended: Option<u64>,
    moved: &Arc<AtomicU64>,
) -> Result<PathBuf, Failure> {
    let (part, journal) = (suffixed(dest, ".part"), suffixed(dest, HELD));
    let follows = item.exported_rev.is_some_and(|held| held + 1 == item.rev);
    let mut from = appended.filter(|_| follows).unwrap_or(0);
    loop {
        let prepare_began = Instant::now();
        let prepared = blocking({
            let (part, dest, length, digest) = (part.clone(), dest.to_owned(), item.length, item.digest.clone());
            move || prepare(&part, &dest, from, length, &digest)
        })
        .await;
        let held = match prepared {
            Ok(held) => held,
            Err(e) if from == 0 => {
                return Err(Failure::Again(format!(
                    "cannot write {}: {e}",
                    part.display()
                )));
            }
            Err(_) => {
                from = 0;
                continue;
            }
        };
        let in_place = v1::held_bytes(&held);
        if !private_path(dest) { info!(run = %target.run, output = %target.output, index = target.index, rev = item.rev,
            phase = "output_prepare_file", bytes = item.length, appended_from = from, in_place,
            elapsed_ms = prepare_began.elapsed().as_secs_f64() * 1000.0); }
        moved.store(in_place, Ordering::Relaxed);
        fetch(link, target, item.rev, &held, item.length, &part, Some(&journal), moved).await?;
        match check(&part, item.length, &item.digest).await {
            Ok(()) => return Ok(part),
            // What was in place before (an earlier attempt's, the revision this one extends) was
            // not this revision's: read whole once more.
            Err(Failure::Wrong(_)) if in_place > 0 => {
                remove(&journal);
                from = 0;
            }
            Err(failure) => {
                remove(&journal);
                return Err(failure);
            }
        }
    }
}

/// This revision's tree in `<dir>.part/`: its manifest, then each file it names, each checked
/// against its digest; a file the tree names twice is read once.
async fn fetch_tree(
    link: &Link,
    target: &v1::OutputTarget,
    item: &OutputRow,
    dest: &Path,
    moved: &Arc<AtomicU64>,
) -> Result<PathBuf, Failure> {
    let (stage, manifest) = (suffixed(dest, ".part"), suffixed(dest, ".manifest.part"));
    let unwritable =
        |e: std::io::Error| Failure::Again(format!("cannot write {}: {e}", stage.display()));
    let prepared = blocking({
        let (stage, manifest, length) = (stage.clone(), manifest.clone(), item.length);
        move || {
            remove(&stage);
            std::fs::create_dir_all(&stage)?;
            let empty = std::fs::File::create(&manifest)?;
            empty.set_len(length)
        }
    });
    prepared.await.map_err(unwritable)?;
    fetch(link, target, item.rev, &[], item.length, &manifest, None, moved).await?;
    check(&manifest, item.length, &item.digest).await?;
    let listed = std::fs::read(&manifest).map_err(unwritable)?;
    let _ = std::fs::remove_file(&manifest);
    let mut read: HashMap<String, PathBuf> = HashMap::new();
    let members =
        tree::parse(&listed).map_err(|e| Failure::Lost(format!("the tree's manifest: {e}")))?;
    for (path, tree::Member { length, sha256, .. }) in members {
        let digest = format!("sha256:{sha256}");
        let file = stage.join(&path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(unwritable)?;
        }
        if let Some(first) = read.get(&digest) {
            std::fs::copy(first, &file).map_err(unwritable)?;
            continue;
        }
        let empty = std::fs::File::create(&file).and_then(|f| f.set_len(length));
        empty.map_err(unwritable)?;
        let member = v1::OutputTarget {
            member: path,
            ..target.clone()
        };
        fetch(link, &member, item.rev, &[], length, &file, None, moved).await?;
        check(&file, length, &digest).await?;
        read.insert(digest, file);
    }
    Ok(stage)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
}

/// Beside a `.part`: the bytes it is for (`<digest> <length>`), then each stretch `<start> <end>`
/// in place, a line as each range stops.
const HELD: &str = ".part.held";

/// The `part` this revision's `length` bytes are read into, and the stretches of it in place: an
/// earlier attempt's for the same bytes kept as it left them (its `.held` names `digest`), else a
/// fresh one, its first `from` copied from `dest`.
fn prepare(part: &Path, dest: &Path, from: u64, length: u64, digest: &str) -> std::io::Result<Vec<(u64, u64)>> {
    let journal = suffixed(part, ".held");
    let named = format!("{digest} {length}");
    let kept = std::fs::read_to_string(&journal).ok().filter(|kept| kept.lines().next() == Some(named.as_str()));
    if let Some(kept) = kept.filter(|_| std::fs::metadata(part).is_ok_and(|m| m.len() == length)) {
        let stretches = kept.lines().skip(1).filter_map(|line| {
            let (start, end) = line.split_once(' ')?;
            Some((start.parse().ok()?, end.parse::<u64>().ok()?.min(length)))
        });
        return Ok(merged(stretches.collect()));
    }
    let _ = std::fs::remove_file(part);
    let mut file = std::fs::File::create(part)?;
    if from > 0 {
        let copied = std::io::copy(&mut std::fs::File::open(dest)?.take(from), &mut file)?;
        if copied != from {
            return Err(std::io::Error::other(
                "the file is shorter than what extends it",
            ));
        }
    }
    file.set_len(length)?;
    let held = if from > 0 { vec![(0, from)] } else { vec![] };
    let lines: String = held.iter().map(|(start, end)| format!("{start} {end}\n")).collect();
    std::fs::write(&journal, format!("{named}\n{lines}"))?;
    Ok(held)
}

/// `stretches` sorted, those that touch as one.
fn merged(mut stretches: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    stretches.retain(|(start, end)| start < end);
    stretches.sort_unstable();
    let mut merged: Vec<(u64, u64)> = vec![];
    for (start, end) in stretches {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Whether `path` holds exactly `length` bytes with `digest`; they are on the disk if so, before any name is.
async fn check(path: &Path, length: u64, digest: &str) -> Result<(), Failure> {
    let private = private_path(path);
    let (path, digest) = (path.to_owned(), digest.to_owned());
    let reported_path = path.clone();
    let queued = Instant::now();
    let checked = blocking(move || {
        let blocking_queue_ms = queued.elapsed().as_secs_f64() * 1000.0;
        let hash_began = Instant::now();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)?;
        let (mut hasher, mut held, mut buffer) = (Sha256::new(), 0u64, vec![0; 1 << 20]);
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            held += read as u64;
        }
        let got = Digest::from_bytes(hasher.finalize().into()).to_string();
        let hash_ms = hash_began.elapsed().as_secs_f64() * 1000.0;
        if held != length || got != digest {
            return Ok(Err(Failure::Wrong(format!(
                "read {held} bytes with digest {got}, not {length} with {digest}"
            ))));
        }
        let sync_began = Instant::now();
        platform::durable::ordered(&file)?;
        if !private { info!(path = %path.display(), phase = "output_check", bytes = held,
            blocking_queue_ms, hash_ms, sync_ms = sync_began.elapsed().as_secs_f64() * 1000.0); }
        Ok(Ok(()))
    });
    let result = checked.await.unwrap_or_else(|e| Err(Failure::Again(e.to_string())));
    if !private { info!(path = %reported_path.display(), phase = "output_check_result", ok = result.is_ok(),
        elapsed_ms = queued.elapsed().as_secs_f64() * 1000.0); }
    result
}

/// Reads the bytes of `target` at revision `rev` that `held` leaves of `[0, length)` into `part` at
/// their offsets: ranges cut as `lanes::parts` cuts them (at most `RANGE`), `LANES` side by side on
/// the machine's bulk lanes (`lanes::transfer`). Each stretch in place is told to `journal`.
#[allow(clippy::too_many_arguments)]
async fn fetch(
    link: &Link,
    target: &v1::OutputTarget,
    rev: u64,
    held: &[(u64, u64)],
    length: u64,
    part: &Path,
    journal: Option<&Path>,
    moved: &Arc<AtomicU64>,
) -> Result<(), Failure> {
    let ranges = lanes::parts(held, length, RANGES_PER_LANE * LANES, RANGE);
    let read = lanes::transfer(ranges, LANES, |leg| {
        let lane = link.lane(leg.lane, leg.fresh);
        let (target, part, moved) = (target.clone(), part.to_owned(), moved.clone());
        let stretch = Stretch { journal: journal.map(Path::to_owned), first: leg.first, at: leg.at.clone() };
        async move {
            let _stretch = stretch;
            let lane = lane.ok_or_else(|| Ranged::Again("no connection to the machine".into()))?;
            let range = (leg.first, leg.end, length);
            read_range(lane, target, rev, range, part, leg.at, &moved).await
        }
    });
    match read.await {
        Ok(()) => Ok(()),
        Err(Ranged::Stop(failure)) => Err(failure),
        Err(Ranged::Again(message)) => Err(Failure::Again(message)),
    }
}

/// A range's bytes in place, told to its `.part.held` when it stops however it stops: done, failed,
/// or dropped as slow.
struct Stretch {
    journal: Option<PathBuf>,
    first: u64,
    at: Arc<AtomicU64>,
}

impl Drop for Stretch {
    fn drop(&mut self) {
        let at = self.at.load(Ordering::Relaxed);
        if let (Some(journal), true) = (&self.journal, at > self.first) {
            let line = format!("{} {at}\n", self.first);
            let appended = std::fs::OpenOptions::new().append(true).open(journal);
            if let Err(e) = appended.and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes())) {
                if private_path(journal) { warn!("private output range progress could not be saved"); }
                else { warn!(path = %journal.display(), "what a range left in place is not kept: {e}"); }
            }
        }
    }
}

/// Logs even when a range fails or a slow/replaced transfer drops its future.
struct RangeTiming<'a> {
    private: bool,
    target: &'a v1::OutputTarget,
    rev: u64,
    first: u64,
    end: u64,
    at: &'a AtomicU64,
    began: Instant,
    phase: &'static str,
    headers_ms: Option<f64>,
    file_ready_ms: Option<f64>,
    first_body_ms: Option<f64>,
    body_wait_ms: f64,
    file_write_ms: f64,
    chunks: u64,
}

impl Drop for RangeTiming<'_> {
    fn drop(&mut self) {
        if self.private { return; }
        info!(run = %self.target.run, output = %self.target.output, index = self.target.index,
            rev = self.rev, member = %self.target.member, phase = "output_range",
            stopped_at = self.phase, first = self.first, end = self.end,
            bytes_in_place = self.at.load(Ordering::Relaxed).saturating_sub(self.first),
            headers_ms = self.headers_ms, file_ready_ms = self.file_ready_ms,
            first_body_ms = self.first_body_ms, body_wait_ms = self.body_wait_ms,
            file_write_ms = self.file_write_ms, chunks = self.chunks,
            elapsed_ms = self.began.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Reads `[first, end)` of an output `length` bytes long into `part`, moving `at` past each byte
/// in place.
async fn read_range(
    lane: Bulk,
    target: v1::OutputTarget,
    rev: u64,
    (first, end, length): (u64, u64, u64),
    part: PathBuf,
    at: Arc<AtomicU64>,
    moved: &AtomicU64,
) -> Result<(), Ranged<Failure>> {
    let mut timing = RangeTiming {
        private: private_path(&part),
        target: &target,
        rev,
        first,
        end,
        at: &at,
        began: Instant::now(),
        phase: "headers",
        headers_ms: None,
        file_ready_ms: None,
        first_body_ms: None,
        body_wait_ms: 0.0,
        file_write_ms: 0.0,
        chunks: 0,
    };
    let response = lane.get(&target, (first, end), rev).await;
    timing.headers_ms = Some(timing.began.elapsed().as_secs_f64() * 1000.0);
    let mut output = response.map_err(refused)?;
    timing.phase = "validate_headers";
    if output.length != length {
        let held = format!(
            "the machine holds {} bytes of revision {rev}, not {length}",
            output.length
        );
        return Err(Ranged::Stop(Failure::Lost(held)));
    }
    if output.range.0 != first {
        return Err(Ranged::Again(format!(
            "the machine answered bytes from {}, not {first}",
            output.range.0
        )));
    }
    let unwritable = |e: std::io::Error| {
        Ranged::Stop(Failure::Again(format!(
            "cannot write {}: {e}",
            part.display()
        )))
    };
    timing.phase = "open_file";
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&part)
        .await
        .map_err(unwritable)?;
    file.seek(SeekFrom::Start(first))
        .await
        .map_err(unwritable)?;
    timing.file_ready_ms = Some(timing.began.elapsed().as_secs_f64() * 1000.0);
    let mut done = first;
    while done < end {
        timing.phase = "read_body";
        let body_began = Instant::now();
        let next = output.body.next().await;
        timing.body_wait_ms += body_began.elapsed().as_secs_f64() * 1000.0;
        let data = match next {
            Some(Ok(data)) => data,
            Some(Err(e)) => return Err(Ranged::Again(e.to_string())),
            None => return Err(Ranged::Again("the machine ended the range early".into())),
        };
        timing
            .first_body_ms
            .get_or_insert_with(|| timing.began.elapsed().as_secs_f64() * 1000.0);
        let data = &data[..data.len().min((end - done) as usize)];
        timing.phase = "write_file";
        let write_began = Instant::now();
        file.write_all(data).await.map_err(unwritable)?;
        timing.phase = "flush_file";
        file.flush().await.map_err(unwritable)?;
        timing.file_write_ms += write_began.elapsed().as_secs_f64() * 1000.0;
        timing.chunks += 1;
        done += data.len() as u64;
        at.store(done, Ordering::Relaxed);
        // Only bytes in place count, each once: a range asked again starts where these stopped.
        moved.fetch_add(data.len() as u64, Ordering::Relaxed);
    }
    timing.phase = "complete";
    Ok(())
}

/// A machine's refusal of a read, as the read acts on it.
fn refused(e: machine_client::Error) -> Ranged<Failure> {
    match e {
        machine_client::Error::Refused { status: 404, error } => Ranged::Stop(Failure::Lost(format!(
            "the machine no longer holds it ({}: {})",
            error.code, error.message
        ))),
        // Definitive: the machine will not serve this read (403, 400, 416, 501) or holds the
        // output at another revision (412). While the run goes on, its events bring the newer
        // one; once it ended, none comes.
        machine_client::Error::Refused {
            status: 400 | 403 | 412 | 416 | 501,
            error,
        } => Ranged::Stop(Failure::Refused(format!(
            "{}: {}",
            error.code, error.message
        ))),
        // Anything else may pass (its key not admitted yet, a refusal while it starts): asked
        // again, paced, until it serves or the machine ends, which loses the item.
        machine_client::Error::Refused { status, error } if status < 500 => {
            Ranged::Stop(Failure::Again(format!("{}: {}", error.code, error.message)))
        }
        machine_client::Error::Answer(message) => Ranged::Stop(Failure::Again(message)),
        e => Ranged::Again(e.to_string()),
    }
}

#[cfg(test)]
mod security_tests {

    #[test]
    #[ignore = "requires ffmpeg and ffprobe on PATH"]
    fn output_probe_does_not_follow_downloaded_playlist() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=32x32:r=1", "-t", "1", "-c:v", "libx264", "-threads", "1"])
            .arg(&marker).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        let valid = crate::project::measured(dir.path(), &marker, "video/mp4").expect("valid video measured");
        assert_eq!((valid.width, valid.height), (32, 32));
        let downloaded = dir.path().join("42-video.mp4.part");
        std::fs::write(&downloaded, "ffconcat version 1.0\nfile marker.mp4\n").unwrap();
        let measured = crate::project::measured(dir.path(), &downloaded, "video/mp4");
        assert!(measured.is_none(), "a downloaded playlist is not measured as a media file");
    }
}
