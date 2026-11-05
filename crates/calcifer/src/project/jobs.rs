//! What a project's render is doing, asset by asset: the `Board` that `/events` and `GET /v1/projects/{id}` give a view to draw. It
//! decides nothing. `sync` reads what the walk owes, and `told` and `ran` fold what the walk and its runs say.
use std::{collections::HashSet, path::Path, sync::Mutex};

use protocol::{
    calcifer::{Event as RunEvent, Progress, Run, Status},
    ids::RunId,
    project::{Board, Event, Job, JobState, Preview},
};
use tokio::sync::watch;

use super::graph::name_key;

/// A version a run is making or about to: what project.lock's pending entry says of it.
pub struct Going {
    pub asset: String,
    pub noun: String,
    pub number: u32,
    /// Its run's number; 0 while it is being sent.
    pub run: u64,
    /// What the calls before this one of its operation took.
    pub earlier_ms: u64,
    /// It makes a shot.
    pub shot: bool,
}

pub struct Jobs {
    held: Mutex<Held>,
    board: watch::Sender<Board>,
}

#[derive(Default)]
struct Held {
    seq: u64,
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
}

impl Default for Jobs {
    fn default() -> Self {
        Self { held: Mutex::default(), board: watch::Sender::new(Board::default()) }
    }
}

impl Jobs {
    pub fn board(&self) -> Board {
        self.board.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Board> {
        self.board.subscribe()
    }

    /// A walk begins: the board starts empty, its `seq` still rising.
    pub fn begin(&self) {
        let mut held = self.held.lock().expect("jobs lock");
        *held = Held { seq: held.seq, began_at_ms: crate::store::now_ms(), ..Held::default() };
        self.publish(&mut held);
    }

    /// The walk is over: nothing more changes. A cut no version of it came with is what a render ends on: the film that stands.
    pub fn end(&self, elapsed_ms: u64) {
        let mut held = self.held.lock().expect("jobs lock");
        held.elapsed_ms = Some(elapsed_ms);
        (held.live, held.sending, held.coming) = Default::default();
        for cut in std::mem::take(&mut held.cuts) {
            if !held.ended.iter().any(|ended| ended.asset == cut.asset) {
                held.ended.push(cut);
            }
        }
        self.publish(&mut held);
    }

    /// The board as project.lock stands: what it sends and what it still owes, each `coming` with whether it makes a shot. A run it
    /// holds no row of is read by `run`.
    pub fn sync(&self, on: Vec<String>, going: Vec<Going>, coming: Vec<(Job, bool)>, run: impl Fn(u64) -> Option<Run>) {
        let mut held = self.held.lock().expect("jobs lock");
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
            if held.live.iter().any(|live| live.job.run == Some(going.run)) {
                continue;
            }
            let Some(run) = run(going.run) else { continue };
            let job = Job { shot: *shot, ..sending(going) };
            let mut live = Live { id: run.id.clone(), job, earlier_ms: going.earlier_ms, settled: false, landed: None };
            live.apply(&run);
            held.live.push(live);
        }
        self.publish(&mut held);
    }

    /// What the walk said.
    pub fn told(&self, event: &Event) {
        let mut held = self.held.lock().expect("jobs lock");
        match event {
            Event::Landed { name, noun, number, path, execution_ms, .. } => {
                let mut job = held.take(name, Some(*number), None).unwrap_or_default();
                job = Job { asset: name.clone(), state: JobState::Done, noun: noun.clone(), number: Some(*number), shot: job.shot, took_ms: *execution_ms, path: Some(path.clone()), run: job.run, machine: job.machine, ..Job::default() };
                if let Some(at) = held.cuts.iter().position(|cut| cut.asset == *name) {
                    let cut = held.cuts.remove(at);
                    job = Job { path: cut.path, seconds: cut.seconds, why: cut.why, took_ms: cut.took_ms, ..job };
                }
                held.ended.push(job);
            }
            Event::Failed { error, name: Some(name), run } => {
                let canceled = run.and_then(|run| held.live.iter().find(|live| live.job.run == Some(run))).is_some_and(|live| live.settled && live.job.why.as_deref() == Some(CANCELED));
                let job = held.take(name, None, *run).unwrap_or_default();
                let state = if canceled { JobState::Canceled } else { JobState::Failed };
                let job = Job { asset: name.clone(), state, noun: job.noun, number: job.number, shot: job.shot, run: job.run, machine: job.machine, took_ms: job.took_ms, error: Some(error.clone()), ..Job::default() };
                held.ended.push(job);
            }
            Event::Cut { name, cut, frames, fps, retimed, took_s, unchanged, without } => {
                let mut why = vec![];
                match (*unchanged, retimed.as_slice()) {
                    (true, _) => why.push("(unchanged)".to_owned()),
                    (false, []) => {}
                    (false, takes) => why.push(format!("it retimes {} to {fps} fps", takes.join(", "))),
                }
                if !without.is_empty() {
                    why.push(format!("(without {})", without.join("; ")));
                }
                let job = Job {
                    asset: name.clone(),
                    state: JobState::Done,
                    noun: "version".into(),
                    path: Some(cut.clone()),
                    seconds: Some(*frames as f64 / fps),
                    took_ms: (*took_s > 0.0).then_some((took_s * 1000.0) as u64),
                    why: (!why.is_empty()).then(|| why.join(" ")),
                    ..Job::default()
                };
                held.cuts.retain(|cut| cut.asset != *name);
                held.cuts.push(job);
            }
            _ => return,
        }
        self.publish(&mut held);
    }

    /// What ended with no file of its own: `name` stands as it is, it came with another's take, or it was never sent.
    pub fn ended(&self, job: Job) {
        let mut held = self.held.lock().expect("jobs lock");
        if job.state == JobState::NotSent && held.ended.iter().any(|ended| ended.state == JobState::NotSent && ended.asset == job.asset) {
            return;
        }
        let was = held.take(&job.asset, job.number, None).unwrap_or_default();
        let noun = if job.noun.is_empty() { was.noun } else { job.noun.clone() };
        held.ended.push(Job { noun, number: job.number.or(was.number), shot: job.shot.or(was.shot), ..job });
        self.publish(&mut held);
    }

    /// Says what came of a version already landed.
    pub fn why(&self, asset: &str, number: u32, why: String) {
        let mut held = self.held.lock().expect("jobs lock");
        if let Some(job) = held.ended.iter_mut().find(|job| job.asset == asset && job.number == Some(number)) {
            job.why = Some(why);
        }
        self.publish(&mut held);
    }

    /// The files of a version listed before they landed came, or never will: it's no version then, and its failure says so.
    pub fn arrived(&self, asset: &str, number: u32, came: bool) {
        let mut held = self.held.lock().expect("jobs lock");
        let mine = |job: &Job| job.asset == asset && job.number == Some(number);
        match came {
            true => held.ended.iter_mut().filter(|job| mine(job)).for_each(|job| job.why = None),
            false => held.ended.retain(|job| !mine(job)),
        }
        self.publish(&mut held);
    }

    /// Whether `asset` has said `message` already; it has now.
    pub fn warned(&self, asset: &str, message: &str) -> bool {
        !self.held.lock().expect("jobs lock").warned.insert((asset.to_owned(), message.to_owned()))
    }

    /// What run `id` did, when it is one of this board's, under `dir`; `Some`: a warning it gave for the first time, and whose it is.
    pub fn ran(&self, id: &RunId, event: &RunEvent, dir: &Path) -> Option<(String, String)> {
        let mut held = self.held.lock().expect("jobs lock");
        let live = held.live.iter_mut().find(|live| live.id == *id)?;
        match event {
            RunEvent::Created { run, .. }
            | RunEvent::Queued { run, .. }
            | RunEvent::InProgress { run, .. }
            | RunEvent::Completed { run, .. }
            | RunEvent::Failed { run, .. }
            | RunEvent::Canceled { run, .. } => live.apply(run),
            RunEvent::Progress { progress } => live.progress(progress),
            RunEvent::Waiting { reason } => live.job.why = Some(reason.clone()).filter(|reason| !reason.is_empty()),
            RunEvent::OutputLanded { item, .. } => {
                if live.settled || item.stays {
                    return None;
                }
                let path = item.path.as_ref()?;
                let path = path.strip_prefix(dir).unwrap_or(path).display().to_string();
                let label = words(if item.label.is_empty() { &item.name } else { &item.label });
                live.landed = Some((Preview { label, path }, (live.job.stage.clone(), live.job.detail.clone())));
                return None;
            }
            // A stage shows in its row, and an input reused needn't show at all.
            RunEvent::Log { level, text, .. } if !matches!(level.as_str(), "info" | "debug" | "stage" | "inputs") => {
                let asset = live.job.asset.clone();
                return held.warned.insert((asset.clone(), text.clone())).then(|| (asset, text.clone()));
            }
            _ => return None,
        }
        self.publish(&mut held);
        None
    }

    /// The board now, told if it changed. `seq` rises across a restart, being at least the clock.
    fn publish(&self, held: &mut Held) {
        let mut going: Vec<Job> = held.live.iter().map(|live| live.job.clone()).chain(held.sending.iter().cloned()).collect();
        going.sort_by_key(|job| job.state != JobState::Running);
        let names: Vec<(u64, String)> = going.iter().filter_map(|job| Some((job.run?, job.asset.clone()))).collect();
        for job in &mut going {
            job.why = job.why.take().map(|why| of_assets(&why, &names));
        }
        let jobs: Vec<Job> = held.ended.iter().cloned().chain(going).chain(held.coming.iter().cloned()).collect();
        let stands = self.board.borrow();
        let shots = held.shots.len() as u32;
        let same = stands.seq > 0 && stands.jobs == jobs && stands.shots == shots && stands.on == held.on && stands.elapsed_ms == held.elapsed_ms && stands.began_at_ms == held.began_at_ms;
        drop(stands);
        if same {
            return;
        }
        held.seq = (held.seq + 1).max(crate::store::now_ms() as u64);
        let board = Board { seq: held.seq, on: held.on.clone(), began_at_ms: held.began_at_ms, elapsed_ms: held.elapsed_ms, shots, jobs };
        self.board.send_replace(board);
    }
}

/// Why a job canceled reads so in its row until it fails.
const CANCELED: &str = "canceled";

impl Held {
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
        job.stalled = None;
        let at = (job.stage.clone(), job.detail.clone());
        if let Some((landed, _)) = self.landed.take_if(|(_, then)| *then != at) {
            job.preview = Some(landed);
        }
    }
}

fn sending(going: &Going) -> Job {
    Job {
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
