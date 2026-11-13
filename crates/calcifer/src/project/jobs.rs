//! What a project's render is doing, asset by asset: the `Board` that `GET /v1/projects/{id}` gives a view to draw, each change told
//! on `GET /v1/events`. It decides nothing. `sync` reads what the walk owes, and `told` and `ran` fold what the walk and its runs say.
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Mutex,
};

use api::{
    calcifer::{Event as RunEvent, NoteLevel, Progress, Run, Status},
    error::Error,
    ids::RunId,
    calcifer::project::{Ahead, Board, Ended, Job, JobState, Preview, Said},
};

use super::graph::name_key;

/// A version a run is making or about to: what the lock's pending entry says of it.
pub(crate) struct Going {
    /// The job it is, as the board knows it.
    pub job: String,
    pub asset: String,
    pub noun: String,
    pub number: u32,
    /// Its run's number; 0 while it is being sent.
    pub run: u64,
    /// What the calls before this one of its operation took.
    pub earlier_ms: u64,
    /// It makes a shot.
    pub shot: bool,
    /// When it should end, as its machine quoted, and how many GPUs it is on.
    pub ends_at_ms: Option<i64>,
    pub gpus: Option<u32>,
}

/// What a walk tells its board.
pub(crate) enum Told {
    /// `name`'s version `number` (its `noun`) was made: the file people mean by it, and its run's execution time.
    Landed { name: String, noun: String, number: u32, path: String, execution_ms: Option<u64> },
    /// A line of the render's, of the asset `name` if it is of one.
    Said { message: String, name: Option<String> },
    /// A warning, of the asset `name` whose run gave it; with none, it names its asset itself, if it is of one.
    Warning { message: String, name: Option<String> },
    /// Something asked could not be made; the walk goes on with what does not read it. `name`, the asset, and `run`, the run that
    /// failed, when it is of one: its row's `failure` holds this for later readers.
    Failed { error: Error, name: Option<String>, run: Option<u64> },
    /// Asset `name`'s cut (`edit.mp4`, and its `.otio`) was made here, in `took_s`: `frames` long at `fps`, the takes in `retimed`
    /// retimed to it. `unchanged`: what it holds did not change, so it was left. `without`: what it was cut without, `<piece>: <why>`.
    Cut { name: String, cut: String, frames: u64, fps: f64, retimed: Vec<String>, took_s: f64, unchanged: bool, without: Vec<String> },
}

pub(crate) struct Jobs {
    held: Mutex<Held>,
    /// The project's id, which its events name.
    project: String,
}

#[derive(Default)]
struct Held {
    /// The board as last told: a client hears it whole when more than a job changed, else each job that did.
    board: Board,
    seq: u64,
    /// The sequence of the active walk's first board.
    active: Option<u64>,
    on: Vec<String>,
    began_at_ms: i64,
    elapsed_ms: Option<u64>,
    /// What has ended, in the order it did.
    ended: Vec<Job>,
    live: Vec<Live>,
    sending: Vec<Job>,
    coming: Vec<Job>,
    /// A cut said, until its version lands or the walk ends.
    cuts: Vec<Job>,
    /// The shots this walk makes, by name key, each as it was first seen: its place here is its number.
    shots: Vec<String>,
    /// The warnings its runs' logs gave, by asset.
    warned: HashSet<(String, String)>,
    /// Each job as last told, by id.
    told: HashMap<String, Job>,
    /// The upcoming jobs' rows: the id their machine's downloads name, what it's for as the text names it, and the job's key (cl-967).
    ahead: Vec<(RunId, String, String, Ahead)>,
    said: Vec<Said>,
    errors: Vec<Error>,
    /// How the walk ended, once it has.
    end: Option<Ended>,
}

/// A job with a run.
struct Live {
    id: RunId,
    job: Job,
    earlier_ms: u64,
    /// Its run ended: what lands now is its result.
    settled: bool,
    /// What landed while it ran and where the run was then, shown once it's seen to go on from there: what lands last is the result.
    landed: Option<(Preview, (Option<String>, Option<String>))>,
    /// Its run, running, says its time left (`eta_s`): its end is that, the time every client shows, no longer its quote.
    timed: bool,
}

impl Jobs {
    pub(crate) fn new(project: String) -> Self {
        Self { held: Mutex::default(), project }
    }

    pub(crate) fn board(&self) -> Board {
        let held = self.held.lock().expect("jobs lock");
        Board { ahead: held.ahead.iter().map(|(.., row)| row.clone()).collect(), ..held.board.clone() }
    }

    pub(crate) fn active(&self) -> Option<u64> {
        self.held.lock().expect("jobs lock").active
    }

    /// The board an earlier walk ended on, as project.db kept it: what this shows until a walk begins.
    pub(crate) fn ended_on(&self, board: Board) {
        let mut held = self.held.lock().expect("jobs lock");
        held.seq = held.seq.max(board.seq);
        held.board = board;
    }

    /// A walk begins: the board starts empty, its `seq` still rising.
    pub(crate) fn begin(&self) -> u64 {
        let mut held = self.held.lock().expect("jobs lock");
        *held = Held { seq: held.seq, began_at_ms: crate::store::now_ms(), ..Held::default() };
        self.publish(&mut held);
        held.active = Some(held.seq);
        held.seq
    }

    /// The walk is over, as `ended` says: nothing more changes. A cut no version of it came with is what a render ends on: the film
    /// that stands.
    pub(crate) fn end(&self, elapsed_ms: u64, ended: Ended) -> Board {
        let mut held = self.held.lock().expect("jobs lock");
        held.active = None;
        held.elapsed_ms = Some(elapsed_ms);
        held.end = Some(ended);
        // What it fetched ahead goes no further: nothing here hears of it now.
        for (.., row) in held.ahead.iter_mut().filter(|(.., row)| row.stage.is_some()) {
            (row.stage, row.waiting) = (None, None);
        }
        (held.live, held.sending, held.coming) = Default::default();
        for cut in std::mem::take(&mut held.cuts) {
            match held.ended.iter_mut().find(|ended| ended.asset == cut.asset) {
                // Its turn found it standing: this is the film that stands.
                Some(ended) if ended.why.as_deref() == Some(STANDS) => *ended = Job { id: ended.id.clone(), ..cut },
                Some(_) => {}
                None => held.ended.push(cut),
            }
        }
        self.publish(&mut held);
        held.board.clone()
    }

    /// The board as the lock stands: what it sends and what it still owes, each `coming` with whether it makes a shot. A run it
    /// holds no row of is read by `run`. The data is read outside this lock and may belong to a walk that has ended.
    pub(crate) fn sync(&self, active: u64, on: Vec<String>, going: Vec<Going>, coming: Vec<(Job, bool)>, run: impl Fn(u64) -> Option<Run>) {
        let mut held = self.held.lock().expect("jobs lock");
        if held.active != Some(active) {
            return;
        }
        for machine in on {
            if !held.on.contains(&machine) {
                held.on.push(machine);
            }
        }
        // Numbered as first seen: what is going, then what is owed in the order it goes, so a shot keeps its number while it runs and after.
        let shots: Vec<Option<u32>> = going.iter().map(|going| going.shot.then(|| held.shot(&going.asset))).collect();
        held.coming = coming.into_iter().map(|(job, shot)| Job { shot: shot.then(|| held.shot(&job.asset)), ..job }).collect();
        held.live.retain(|live| going.iter().any(|going| Some(going.run) == live.job.run));
        held.sending = going.iter().zip(&shots).filter(|(going, _)| going.run == 0).map(|(going, shot)| Job { shot: *shot, ..sending(going) }).collect();
        for (going, shot) in going.iter().zip(&shots).filter(|(going, _)| going.run > 0) {
            if let Some(live) = held.live.iter_mut().find(|live| live.job.run == Some(going.run)) {
                // Its time left may have come before its row did: read again until it has.
                if !live.timed {
                    live.job.ends_at_ms = going.ends_at_ms;
                    if let Some(progress) = run(going.run).and_then(|run| run.progress) {
                        live.progress(&progress);
                    }
                }
                live.job.gpus = going.gpus;
                continue;
            }
            let Some(run) = run(going.run) else { continue };
            let job = Job { shot: *shot, ..sending(going) };
            let mut live = Live { id: run.id.clone(), job, earlier_ms: going.earlier_ms, settled: false, landed: None, timed: false };
            live.apply(&run);
            held.live.push(live);
        }
        self.publish(&mut held);
    }

    /// What the walk said.
    pub(crate) fn told(&self, told: Told) {
        let mut held = self.held.lock().expect("jobs lock");
        match told {
            Told::Said { message, name } => {
                if held.said.iter().any(|said| said.message == message) {
                    return;
                }
                held.said.push(Said { level: NoteLevel::Info, message, names: name.into_iter().collect() });
            }
            Told::Warning { message, name } => {
                if !held.warn(message, name) {
                    return;
                }
            }
            Told::Failed { error, name: None, .. } => {
                if held.errors.iter().any(|told| told.message == error.message) {
                    return;
                }
                held.errors.push(error);
            }
            Told::Landed { name, noun, number, path, execution_ms } => {
                let mut job = held.take(&name, Some(number), None).unwrap_or_default();
                job = Job { id: job.id, asset: name.clone(), state: JobState::Done, noun, number: Some(number), shot: job.shot, took_ms: execution_ms, path: Some(path), run: job.run, machine: job.machine, ..Job::default() };
                if let Some(at) = held.cuts.iter().position(|cut| cut.asset == name) {
                    let cut = held.cuts.remove(at);
                    job = Job { path: cut.path, seconds: cut.seconds, why: cut.why, took_ms: cut.took_ms, ..job };
                }
                held.ended.push(job);
            }
            Told::Failed { error, name: Some(name), run } => {
                let canceled = run.and_then(|run| held.live.iter().find(|live| live.job.run == Some(run))).is_some_and(|live| live.settled && live.job.why.as_deref() == Some(CANCELED));
                let job = held.take(&name, None, run).unwrap_or_default();
                let state = if canceled { JobState::Canceled } else { JobState::Failed };
                let job = Job { id: job.id, asset: name, state, noun: job.noun, number: job.number, shot: job.shot, run: job.run, machine: job.machine, took_ms: job.took_ms, error: Some(error), ..Job::default() };
                held.ended.push(job);
            }
            Told::Cut { name, cut, frames, fps, retimed, took_s, unchanged, without } => {
                let mut why = vec![];
                match (unchanged, retimed.as_slice()) {
                    (true, _) => why.push("(unchanged)".to_owned()),
                    (false, []) => {}
                    (false, takes) => why.push(format!("it retimes {} to {fps} fps", takes.join(", "))),
                }
                if !without.is_empty() {
                    why.push(format!("(without {})", without.join("; ")));
                }
                held.cuts.retain(|held| held.asset != name);
                let job = Job {
                    asset: name,
                    state: JobState::Done,
                    noun: "version".into(),
                    path: Some(cut),
                    seconds: Some(frames as f64 / fps),
                    took_ms: (took_s > 0.0).then_some((took_s * 1000.0) as u64),
                    why: (!why.is_empty()).then(|| why.join(" ")),
                    ..Job::default()
                };
                held.cuts.push(job);
            }
        }
        self.publish(&mut held);
    }

    /// What ended with no file of its own: `name` stands as it is, it came with another's take, or it was never sent.
    pub(crate) fn ended(&self, job: Job) {
        let mut held = self.held.lock().expect("jobs lock");
        if job.state == JobState::NotSent && held.ended.iter().any(|ended| ended.state == JobState::NotSent && ended.asset == job.asset) {
            return;
        }
        let was = held.take(&job.asset, job.number, None).unwrap_or_default();
        let noun = if job.noun.is_empty() { was.noun } else { job.noun.clone() };
        let id = if job.id.is_empty() { was.id } else { job.id.clone() };
        held.ended.push(Job { id, noun, number: job.number.or(was.number), shot: job.shot.or(was.shot), ..job });
        self.publish(&mut held);
    }

    /// Job `id` of `asset` ended with no version: done, if nothing else was said of it.
    pub(crate) fn ended_if_open(&self, id: &str, asset: &str) {
        let mut held = self.held.lock().expect("jobs lock");
        if !held.coming.iter().any(|job| job.id == id) {
            return;
        }
        // A cut said what it made, or that it holds what stood: that is what it ended on.
        if let Some(at) = held.cuts.iter().position(|cut| cut.asset == asset) {
            let cut = held.cuts.remove(at);
            held.take(asset, None, None);
            held.ended.push(Job { id: id.to_owned(), ..cut });
            return self.publish(&mut held);
        }
        drop(held);
        self.ended(Job { id: id.to_owned(), asset: asset.to_owned(), state: JobState::Done, noun: "version".into(), why: Some("nothing to do".into()), ..Job::default() });
    }

    /// Says what came of a version already landed.
    pub(crate) fn why(&self, asset: &str, number: u32, why: String) {
        let mut held = self.held.lock().expect("jobs lock");
        if let Some(job) = held.ended.iter_mut().find(|job| job.asset == asset && job.number == Some(number)) {
            job.why = Some(why);
        }
        self.publish(&mut held);
    }

    /// The files of a version listed before they landed came, or never will: it's no version then, and its failure says so.
    pub(crate) fn arrived(&self, asset: &str, number: u32, came: bool) {
        let mut held = self.held.lock().expect("jobs lock");
        let mine = |job: &Job| job.asset == asset && job.number == Some(number);
        match came {
            true => held.ended.iter_mut().filter(|job| mine(job)).for_each(|job| job.why = None),
            false => held.ended.retain(|job| !mine(job)),
        }
        self.publish(&mut held);
    }

    /// `from` is called `to` now: the board says so, of what's going and what has ended.
    pub(crate) fn renamed(&self, from: &str, to: &str) {
        let mut held = self.held.lock().expect("jobs lock");
        let held = &mut *held;
        let key = name_key(from);
        let called = |job: &mut Job| {
            if name_key(&job.asset) == key {
                to.clone_into(&mut job.asset);
            }
        };
        held.ended.iter_mut().chain(&mut held.sending).chain(&mut held.coming).chain(&mut held.cuts).chain(held.told.values_mut()).for_each(called);
        held.live.iter_mut().for_each(|live| called(&mut live.job));
        held.shots.iter_mut().filter(|shot| **shot == key).for_each(|shot| *shot = name_key(to));
        held.warned = std::mem::take(&mut held.warned).into_iter().map(|(asset, said)| (if name_key(&asset) == key { to.to_owned() } else { asset }, said)).collect();
    }

    /// Whether `asset` has said `message` already; it has now.
    pub(crate) fn warned(&self, asset: &str, message: &str) -> bool {
        !self.held.lock().expect("jobs lock").warned.insert((asset.to_owned(), message.to_owned()))
    }

    /// Upcoming job `key`'s row on its machine (cl-967), as `id` its machine names it: shown, and said when it moved.
    pub(crate) fn upcoming(&self, id: &str, key: &str, row: Ahead) {
        let Ok(id) = RunId::try_from(id.to_owned()) else { return };
        let mut held = self.held.lock().expect("jobs lock");
        // Each row its own number, as the page keys them, the newest the greatest.
        let next = held.ahead.iter().map(|(.., row)| row.run).max().unwrap_or(0) + 1;
        let row = match held.ahead.iter_mut().find(|(run, ..)| *run == id) {
            Some((.., was)) if Ahead { run: was.run, ..row.clone() } == *was => return,
            Some((.., was)) => {
                *was = Ahead { run: was.run, ..row };
                was.clone()
            }
            None => {
                let row = Ahead { run: next, ..row };
                held.ahead.push((id, String::new(), key.to_owned(), row.clone()));
                row
            }
        };
        crate::events::ahead_updated(&self.project, &row);
        self.publish(&mut held);
    }

    /// The upcoming jobs' rows, but those of `keys`, leave the board: their jobs went or ended.
    pub(crate) fn upcoming_kept(&self, keys: &HashSet<String>) {
        let mut held = self.held.lock().expect("jobs lock");
        let before = held.ahead.len();
        held.ahead.retain(|(id, _, key, _)| !id.as_str().starts_with(crate::ahead::UPCOMING) || keys.contains(key));
        if held.ahead.len() != before {
            self.publish(&mut held);
        }
    }

    /// What run `id` did, when it is one of this board's, under `dir`: a warning it gave is said once, as its asset's.
    pub(crate) fn ran(&self, active: u64, id: &RunId, event: &RunEvent, dir: &Path) {
        let mut held = self.held.lock().expect("jobs lock");
        if held.active != Some(active) {
            return;
        }
        let Some(live) = held.live.iter_mut().find(|live| live.id == *id) else { return };
        match event {
            RunEvent::Created { run, .. }
            | RunEvent::Queued { run, .. }
            | RunEvent::InProgress { run, .. }
            | RunEvent::Completed { run, .. }
            | RunEvent::Failed { run, .. }
            | RunEvent::Canceled { run, .. } => {
                live.apply(run);
                // What made it slow, as its record says (cl-913), is said on its box too.
                let asset = live.job.asset.clone();
                let slowed = run.warnings.iter().filter(|warning| crate::slow::ON_BOX.contains(&warning.code.as_str()));
                for message in slowed.map(|warning| warning.message.clone()).collect::<Vec<_>>() {
                    if held.warned.insert((asset.clone(), message.clone())) {
                        held.warn(message, Some(asset.clone()));
                    }
                }
            }
            RunEvent::Progress { progress } => live.progress(progress),
            RunEvent::Waiting { reason } => live.job.why = Some(reason.clone()).filter(|reason| !reason.is_empty()),
            RunEvent::OutputLanded { item, .. } => {
                let Some(path) = item.path.as_ref().filter(|_| !live.settled && !item.stays) else { return };
                let path = path.strip_prefix(dir).unwrap_or(path).display().to_string();
                let label = words(if item.label.is_empty() { &item.name } else { &item.label });
                live.landed = Some((Preview { label, path }, (live.job.stage.clone(), live.job.detail.clone())));
                return;
            }
            // A stage shows in its row, and an input reused needn't show at all.
            RunEvent::Log { level, text, .. } if !matches!(level.as_str(), "info" | "debug" | "stage" | "inputs") => {
                let asset = live.job.asset.clone();
                if !held.warned.insert((asset.clone(), text.clone())) || !held.warn(text.clone(), Some(asset)) {
                    return;
                }
            }
            _ => return,
        }
        self.publish(&mut held);
    }

    /// The board now, told if it changed: whole when more than its jobs did, else each job that changed. `seq` rises across a restart,
    /// being at least the clock.
    fn publish(&self, held: &mut Held) {
        let mut going: Vec<Job> = held.live.iter().map(|live| live.job.clone()).chain(held.sending.iter().cloned()).collect();
        going.sort_by_key(|job| job.state != JobState::Running);
        let names: Vec<(u64, String)> = going.iter().filter_map(|job| Some((job.run?, job.asset.clone()))).collect();
        for job in &mut going {
            job.why = job.why.take().map(|why| of_assets(&why, &names));
        }
        let mut jobs: Vec<Job> = held.ended.iter().cloned().chain(going).chain(held.coming.iter().cloned()).collect();
        for job in &mut jobs {
            if job.id.is_empty() {
                job.id = format!("{}#{}", job.asset, job.number.unwrap_or_default());
            }
        }
        let changed: Vec<&Job> = jobs.iter().filter(|job| held.told.get(&job.id) != Some(job)).collect();
        // What left the board with no end of its own was dropped.
        let left: Vec<Job> = (held.told.values())
            .filter(|was| !jobs.iter().any(|job| job.id == was.id))
            .map(|was| Job { state: JobState::Canceled, ..was.clone() })
            .collect();
        let shots = held.shots.len() as u32;
        let stands = &held.board;
        let same_head = stands.seq > 0
            && stands.shots == shots
            && stands.on == held.on
            && stands.elapsed_ms == held.elapsed_ms
            && stands.began_at_ms == held.began_at_ms
            && stands.said == held.said
            && stands.errors == held.errors
            && stands.ended == held.end;
        if same_head && changed.is_empty() && left.is_empty() {
            return;
        }
        held.seq = (held.seq + 1).max(crate::store::now_ms() as u64);
        let seq = held.seq;
        if same_head {
            for job in changed.iter().copied().chain(&left) {
                crate::events::job_updated(&self.project, job, seq);
            }
        }
        held.told = jobs.iter().map(|job| (job.id.clone(), job.clone())).collect();
        let ahead = held.ahead.iter().map(|(.., row)| row.clone()).collect();
        let (said, errors, ended) = (held.said.clone(), held.errors.clone(), held.end.clone());
        held.board = Board { seq, on: held.on.clone(), began_at_ms: held.began_at_ms, elapsed_ms: held.elapsed_ms, shots, jobs, ahead, said, errors, ended };
        if !same_head {
            crate::events::board_updated(&self.project, &held.board);
        }
    }
}

/// What a job says that found its state standing at its turn.
pub(crate) const STANDS: &str = "it stands";

/// Why a job canceled reads so in its row until it fails.
const CANCELED: &str = "canceled";

impl Held {
    /// `message` said once, naming each asset whose it is: whether that changed what's said.
    fn warn(&mut self, message: String, name: Option<String>) -> bool {
        match self.said.iter_mut().find(|said| said.level == NoteLevel::Warning && said.message == message) {
            Some(said) => match name.filter(|name| !said.names.contains(name)) {
                Some(name) => {
                    said.names.push(name);
                    true
                }
                None => false,
            },
            None => {
                self.said.push(Said { level: NoteLevel::Warning, message, names: name.into_iter().collect() });
                true
            }
        }
    }

    /// The number of the shot of `asset`: its place among the shots seen, a new one at the end.
    fn shot(&mut self, asset: &str) -> u32 {
        let key = name_key(asset);
        let at = self.shots.iter().position(|shot| *shot == key).unwrap_or_else(|| {
            self.shots.push(key);
            self.shots.len() - 1
        });
        at as u32 + 1
    }

    /// The job making `asset`'s version `number`, or run `run`, or else what the asset has coming, out of where it is.
    fn take(&mut self, asset: &str, number: Option<u32>, run: Option<u64>) -> Option<Job> {
        let fits = |job: &Job| match (run, number) {
            (Some(_), _) => job.run == run,
            (None, Some(_)) => job.asset == asset && job.number == number,
            (None, None) => job.asset == asset,
        };
        if let Some(at) = self.live.iter().position(|live| fits(&live.job)) {
            return Some(self.live.remove(at).job);
        }
        let by_name = |job: &Job| job.asset == asset;
        if let Some(at) = self.sending.iter().position(by_name) {
            return Some(self.sending.remove(at));
        }
        self.coming.iter().position(by_name).map(|at| self.coming.remove(at))
    }
}

impl Live {
    /// `run` as its record says now.
    fn apply(&mut self, run: &Run) {
        let job = &mut self.job;
        job.run = Some(run.number);
        job.machine.clone_from(&run.machine);
        job.stalled.clone_from(&run.stalled);
        job.started_at_ms = run.started_at_ms.map(|at| at - self.earlier_ms as i64);
        self.settled = matches!(run.status, Status::Completed | Status::Failed | Status::Canceled);
        if self.settled {
            job.state = if run.started_at_ms.is_some() { JobState::Running } else { JobState::Waiting };
            job.took_ms = Some(self.earlier_ms + run.execution_ms.unwrap_or(0));
            job.why = (run.status == Status::Canceled).then(|| CANCELED.to_owned());
            (job.fraction, job.detail, job.stage) = (None, None, Some("saving its file here".to_owned()));
            (self.landed, job.preview) = (None, None);
            return;
        }
        let queued = run.started_at_ms.is_none() || run.machine_state.as_deref() == Some("queued");
        job.state = if queued { JobState::Waiting } else { JobState::Running };
        job.why = run.waiting.clone().or_else(|| {
            queued.then(|| match (run.machine_state.as_deref(), run.machine.as_deref()) {
                (Some(state), Some(machine)) => format!("{state} on {machine}"),
                (None, Some(machine)) => format!("sent to {machine}"),
                _ => "sent".to_owned(),
            })
        });
        if let Some(progress) = &run.progress {
            self.progress(progress);
        }
    }

    fn progress(&mut self, progress: &Progress) {
        let job = &mut self.job;
        job.stage = (!progress.stage.is_empty()).then(|| words(&progress.stage));
        job.detail = detail(progress);
        job.fraction = progress.done_fraction;
        // Once it runs: before, its time left is its whole run's, and its machine's quote says when that starts.
        if let (Some(left), JobState::Running) = (progress.eta_s, job.state) {
            job.ends_at_ms = Some(crate::store::now_ms() + left * 1000);
            self.timed = true;
        }
        // A move since it stopped clears it; the progress a run's record carries is from before.
        if job.stalled.as_ref().is_some_and(|stalled| progress.at_ms > stalled.since_ms) {
            job.stalled = None;
        }
        let at = (job.stage.clone(), job.detail.clone());
        if let Some((landed, _)) = self.landed.take_if(|(_, then)| *then != at) {
            job.preview = Some(landed);
        }
    }
}

fn sending(going: &Going) -> Job {
    Job {
        id: going.job.clone(),
        ends_at_ms: going.ends_at_ms,
        gpus: going.gpus,
        asset: going.asset.clone(),
        noun: going.noun.clone(),
        number: Some(going.number),
        run: (going.run > 0).then_some(going.run),
        ..Job::default()
    }
}

/// A package's words in a sentence: `Segment 2 of 2 / denoise` is `segment 2/2, denoise`. A word of capitals stays one.
fn words(stage: &str) -> String {
    let lower = |part: &str| -> String {
        let mut chars = part.chars();
        match (chars.next(), chars.next()) {
            (Some(first), Some(second)) if second.is_lowercase() => first.to_lowercase().chain(part[first.len_utf8()..].chars()).collect(),
            _ => part.to_owned(),
        }
    };
    stage.split(" / ").map(|part| counted(&lower(part))).collect::<Vec<_>>().join(", ")
}

/// `segment 2 of 2` as `segment 2/2`.
fn counted(text: &str) -> String {
    let number = |word: &str| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit());
    let mut said: Vec<String> = vec![];
    let mut words = text.split(' ').peekable();
    while let Some(word) = words.next() {
        match said.last_mut() {
            Some(last) if word == "of" && number(last) && words.peek().is_some_and(|next| number(next)) => {
                *last = format!("{last}/{}", words.next().unwrap_or_default());
            }
            _ => said.push(word.to_owned()),
        }
    }
    said.join(" ")
}

/// How far through its stage: its steps of their total, its share, its bytes, and how fast they move.
fn detail(progress: &Progress) -> Option<String> {
    let (done, total) = (progress.bytes_done, progress.bytes_total);
    let bytes = (done > 0 || total > 0).then(|| {
        let mut said = match total {
            0 => crate::bytes(done as f64),
            total => format!("{}/{}", crate::bytes(done as f64), crate::bytes(total as f64)),
        };
        if let Some(rate) = progress.bytes_per_s {
            said += &format!(" at {}/s", crate::bytes(rate));
        }
        said
    });
    let steps = (progress.total > 0).then(|| format!("{}/{}", progress.completed, progress.total));
    let share = progress.stage_fraction.filter(|_| steps.is_none() && bytes.is_none()).map(|share| format!("{:.0}%", share.clamp(0.0, 1.0) * 100.0));
    let said: Vec<String> = [steps, share, bytes].into_iter().flatten().collect();
    (!said.is_empty()).then(|| said.join(" "))
}

/// `why` with each `run <number>` of `names` its asset's name: the machine says a run holds the GPUs, a person reads what it is making.
fn of_assets(why: &str, names: &[(u64, String)]) -> String {
    let mut said = String::new();
    let mut words = why.split(' ').peekable();
    while let Some(word) = words.next() {
        let number = (word == "run").then(|| words.peek()?.trim_end_matches(',').parse::<u64>().ok()).flatten();
        let named = number.and_then(|number| names.iter().find(|(run, _)| *run == number));
        if !said.is_empty() {
            said.push(' ');
        }
        match named {
            Some((_, asset)) => {
                said += asset;
                if words.next().is_some_and(|next| next.ends_with(',')) {
                    said.push(',');
                }
            }
            None => said += word,
        }
    }
    said
}
