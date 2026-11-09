//! `cozy project render` for a person, all drawn from the project's `Board`: on a terminal one live view, then a plain block that stays
//! in the scrollback; anywhere else one line per event, each starting with its asset's name, so runs at once stay readable. Run
//! numbers, machines and raw outputs stay in `--json` and `cozy run show`. Ctrl-C stops following, never the runs.
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{
    error::{Error, Kind},
    calcifer::project::{Ahead, Board, Event, Job, JobState, Rendered},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, TableState},
};

use crate::{Context, calcifer_client::Watch, output, run::stalled_words, tui};

/// How many warnings the live view keeps in sight; the block says them all.
const WARNINGS_SHOWN: usize = 3;
/// How much of a reason the block shows of one, up to its traceback: its `next` reads the rest, and lines off a terminal say it whole.
const REASON_SHOWN: usize = 300;

/// All a client knows of a render.
pub(super) struct Rendering {
    /// The project's folder.
    dir: PathBuf,
    board: Board,
    /// Each distinct warning once, with whose it is.
    warnings: Vec<Warned>,
    said: Vec<Said>,
    /// What failed that is no asset's.
    errors: Vec<Error>,
    /// This render, whose walk's end is its end; None follows whatever walk is going to its end.
    ask: Option<String>,
    ended: Option<(Option<Error>, Option<Rendered>)>,
}

struct Warned {
    names: Vec<String>,
    message: String,
}

struct Said {
    name: Option<String>,
    message: String,
}

impl Rendering {
    pub(super) fn new(dir: PathBuf, board: Board, ask: Option<String>) -> Self {
        Self { dir, board, warnings: vec![], said: vec![], errors: vec![], ask, ended: None }
    }

    /// Takes in `event`; what it newly is for a person to read.
    fn apply(&mut self, event: Event) -> Fresh {
        match event {
            Event::Board { board } if board.seq >= self.board.seq => {
                self.board = board;
                Fresh::Board
            }
            Event::Job { job, seq } if seq >= self.board.seq => {
                self.board.seq = seq;
                match self.board.jobs.iter_mut().find(|held| held.id == job.id) {
                    Some(held) => *held = job,
                    None => self.board.jobs.push(job),
                }
                // What has ended first, in the order it did; then what goes; then what waits.
                self.board.jobs.sort_by_key(|job| match job.state {
                    JobState::Running => 1,
                    JobState::Waiting => 2,
                    _ => 0,
                });
                Fresh::Board
            }
            Event::Ahead { ahead } => {
                match self.board.ahead.iter_mut().find(|held| held.run == ahead.run) {
                    Some(held) => *held = ahead,
                    None => self.board.ahead.push(ahead),
                }
                Fresh::Board
            }
            Event::Warning { message, name } => {
                let line = name.as_ref().map_or_else(|| format!("warning: {message}"), |name| format!("{name}: warning: {message}"));
                match self.warnings.iter_mut().find(|warned| warned.message == message) {
                    Some(warned) => {
                        warned.names.extend(name.filter(|name| !warned.names.contains(name)));
                        Fresh::Nothing
                    }
                    None => {
                        self.warnings.push(Warned { names: name.into_iter().collect(), message });
                        Fresh::Warning(line)
                    }
                }
            }
            Event::Said { message, name } => match self.said.iter().any(|said| said.message == message) {
                true => Fresh::Nothing,
                false => {
                    self.said.push(Said { name, message: message.clone() });
                    Fresh::Said(message)
                }
            },
            Event::Failed { error, name: None, .. } => match self.errors.iter().any(|said| said.message == error.message) {
                true => Fresh::Nothing,
                false => {
                    self.errors.push(error.clone());
                    Fresh::Failed(error)
                }
            },
            // Another walk's end, one ending as this was asked, isn't this render's.
            Event::Ended { error, rendered, asks } if self.ask.as_ref().is_none_or(|ask| asks.contains(ask)) => {
                self.ended = Some((error, rendered));
                Fresh::Ended
            }
            _ => Fresh::Nothing,
        }
    }

    /// `3 of 8`, with what failed.
    fn count(&self) -> String {
        let jobs = &self.board.jobs;
        let done = jobs.iter().filter(|job| job.state == JobState::Done).count();
        let failed = jobs.iter().filter(|job| matches!(job.state, JobState::Failed | JobState::Canceled | JobState::NotSent)).count();
        match failed {
            0 => format!("{done} of {}", jobs.len()),
            failed => format!("{done} of {}, {failed} failed", jobs.len()),
        }
    }

    /// `lab2 on sekai`.
    fn title(&self) -> String {
        let name = self.dir.file_name().map_or_else(|| self.dir.display().to_string(), |name| name.to_string_lossy().into_owned());
        match self.board.on.as_slice() {
            [] => name,
            on => format!("{name} on {}", on.join(", ")),
        }
    }

    /// How long it has gone, or took.
    fn elapsed(&self) -> String {
        clock(self.board.elapsed_ms.unwrap_or_else(|| (now_ms() - self.board.began_at_ms).max(0) as u64))
    }

    /// `shot 1/5`, of a job that makes a shot.
    fn shot(&self, job: &Job) -> Option<String> {
        job.shot.map(|shot| format!("shot {shot}/{}", self.board.shots))
    }

    /// Several machines: each job says which.
    fn several(&self) -> bool {
        self.board.on.len() > 1 || self.board.on.iter().any(|on| on == "any")
    }

    /// `path` as a person opens it: a short name that links to the file, where the terminal opens links; the real name elsewhere.
    fn file(&self, ctx: &Context, path: &str) -> String {
        output::named_link(ctx, &self.dir.join(path), &short(path), path)
    }
}

/// What an event newly is for a person to read.
/// One line for what a machine is fetching ahead of its jobs; none once it is all there.
pub(super) fn ahead_line(ahead: &Ahead) -> Option<String> {
    let function = &ahead.function;
    if let Some(why) = &ahead.held_back {
        return Some(format!("{function}: not fetched ahead: {why}"));
    }
    let stage = ahead.stage.as_deref()?;
    Some(match ahead.total {
        0 => format!("{function} on {}, ahead of its turn: {stage}", ahead.machine),
        total => format!("{function} on {}, ahead of its turn: {stage}: {} of {}", ahead.machine, output::bytes(ahead.done), output::bytes(total)),
    })
}

enum Fresh {
    Nothing,
    Board,
    Warning(String),
    Said(String),
    Failed(Error),
    Ended,
}

/// Follows a render to its end, or until Ctrl-C leaves it going: the live view on a terminal, else line by line.
pub(super) async fn follow(ctx: &Context, mut rendering: Rendering, events: Watch<Event>, plain: bool, status: &str) -> Result<ExitCode, Error> {
    let live = !plain && ctx.term.stdout && !rendering.board.jobs.is_empty() && std::env::var_os("TERM").is_none_or(|term| term != "dumb");
    let mut events = events;
    if live {
        let mut view = View { rendering, events: events.quiet(), selected: None, left: false, closed: false };
        match tui::board(ctx, &mut view).await {
            Ok(()) => {}
            // Nowhere to draw: line by line.
            Err(error) if error.code == "terminal" => {
                rendering = view.rendering;
                events = view.events;
                return lines(ctx, rendering, events, status).await;
            }
            Err(error) => return Err(error),
        }
        return view.rendering.block(ctx, view.left, view.closed, status);
    }
    lines(ctx, rendering, events, status).await
}

/// One line per event, each starting with its asset's name, on the render's way to its end.
async fn lines(ctx: &Context, mut rendering: Rendering, mut events: Watch<Event>, status: &str) -> Result<ExitCode, Error> {
    output::say(ctx, &format!("{}: following the render: Ctrl-C leaves it queued", rendering.title()));
    let mut said = Lines::default();
    said.board(ctx, &rendering);
    if let Some(stalled) = stalled(ctx, &rendering) {
        return Ok(stalled);
    }
    loop {
        let event = tokio::select! {
            event = events.next() => event,
            _ = tokio::signal::ctrl_c() => {
                output::say(ctx, "the project's runs go on in Calcifer");
                output::next(ctx, &[status]);
                return Ok(ExitCode::SUCCESS);
            }
        };
        let Some(event) = event else { return Err(ended_early(status)) };
        match rendering.apply(event) {
            Fresh::Nothing => {}
            Fresh::Board => {
                said.board(ctx, &rendering);
                if let Some(stalled) = stalled(ctx, &rendering) {
                    return Ok(stalled);
                }
            }
            Fresh::Warning(line) => output::warned(&line),
            Fresh::Said(line) => output::say(ctx, &line),
            Fresh::Failed(error) => output::failure(ctx, &error.message, &error.next),
            Fresh::Ended => return rendering.last_line(ctx),
        }
    }
}

/// Following ends with `STALLED` once a job's run has stopped moving, as its run says (its line says so, `Lines`): the render
/// goes on until it's canceled.
fn stalled(ctx: &Context, rendering: &Rendering) -> Option<ExitCode> {
    let job = rendering.board.jobs.iter().find(|job| job.stalled.is_some() && matches!(job.state, JobState::Running | JobState::Waiting))?;
    let cancel = format!("cozy project cancel {}", job.asset);
    let logs = job.machine.as_ref().map(|machine| format!("cozy machine logs {machine} -n 40"));
    output::next(ctx, &[cancel.as_str()].into_iter().chain(logs.as_deref()).collect::<Vec<_>>());
    Some(ExitCode::from(output::STALLED))
}

fn ended_early(status: &str) -> Error {
    Error::new(Kind::Unavailable, "project_events_ended", "Calcifer stopped telling how the runs go").next(status)
}

impl Rendering {
    /// How a render said line by line ends: how far it got, and its exit.
    fn last_line(&self, ctx: &Context) -> Result<ExitCode, Error> {
        let Some((error, rendered)) = &self.ended else { return Ok(ExitCode::SUCCESS) };
        // A render that fell short ends on how far it got, the error's own words.
        match error {
            Some(error) if error.code != "render_incomplete" && self.told(error) => Ok(ExitCode::from(error.exit_code() as u8)),
            Some(error) => Err(error.clone()),
            None => {
                rendered.iter().for_each(|rendered| output::say(ctx, &finished(rendered)));
                Ok(ExitCode::SUCCESS)
            }
        }
    }

    /// Whether `error` was said already.
    fn told(&self, error: &Error) -> bool {
        let of_job = self.board.jobs.iter().filter_map(|job| job.error.as_ref());
        of_job.chain(&self.errors).any(|told| told.message == error.message)
    }

    /// What stays in the scrollback once the live view is gone: each asset, its time and its file; one total line; what failed, last.
    fn block(&self, ctx: &Context, left: bool, closed: bool, status: &str) -> Result<ExitCode, Error> {
        for warned in &self.warnings {
            match warned.names.as_slice() {
                [] => output::warn(&brief(&warned.message)),
                names => output::warn(&format!("{}: {}", names.join(", "), brief(&warned.message))),
            }
        }
        let has_row = |name: &String| self.board.jobs.iter().any(|job| job.asset == *name);
        for said in self.said.iter().filter(|said| !said.name.as_ref().is_some_and(has_row)) {
            output::say(ctx, &said.message);
        }
        self.rows(ctx);
        let Some((error, rendered)) = &self.ended else {
            let going: Vec<&str> = self.board.jobs.iter().filter(|job| matches!(job.state, JobState::Running | JobState::Waiting)).map(|job| job.asset.as_str()).collect();
            if !going.is_empty() {
                anstream::println!("still going: {}", going.join(", "));
            }
            if closed {
                return Err(ended_early(status));
            }
            if left {
                output::say(ctx, "the project's runs go on in Calcifer");
                output::next(ctx, &[status]);
            }
            return Ok(ExitCode::SUCCESS);
        };
        match error {
            Some(error) if error.code == "render_incomplete" => {
                let words = error.message.split_once(": ").map_or(error.message.as_str(), |(words, _)| words);
                anstream::println!("{words}, in {}", clock(self.board.elapsed_ms.unwrap_or(0)));
            }
            _ => rendered.iter().for_each(|rendered| anstream::println!("{}", finished(rendered))),
        }
        self.failures(ctx);
        let Some(error) = error else { return Ok(ExitCode::SUCCESS) };
        if error.code != "render_incomplete" && !self.told(error) {
            return Err(error.clone());
        }
        let shown: HashSet<&String> = self.board.jobs.iter().filter_map(|job| job.error.as_ref()).flat_map(|error| error.next.first()).collect();
        for next in error.next.iter().filter(|next| !shown.contains(next)) {
            output::next(ctx, &[next]);
        }
        Ok(ExitCode::from(error.exit_code() as u8))
    }

    /// Each asset made: its take or version, its time, its file, and what came of it, in columns.
    fn rows(&self, ctx: &Context) {
        let jobs: Vec<&Job> = self.board.jobs.iter().filter(|job| job.state == JobState::Done).collect();
        let name = width(jobs.iter().map(|job| job.asset.chars().count()));
        let made = width(jobs.iter().map(|job| numbered(job).chars().count()));
        let time = width(jobs.iter().map(|job| took(job).chars().count()));
        for job in jobs {
            let mut line = format!("{:<name$}  {:<made$}  {:<time$}  ", job.asset, numbered(job), took(job));
            if let Some(path) = &job.path {
                line += &self.file(ctx, path);
            }
            if let Some(seconds) = job.seconds {
                line += &format!(", {seconds:.1} s");
            }
            if let Some(why) = &job.why {
                line += &format!("  {why}");
            }
            anstream::println!("{}", line.trim_end());
        }
    }

    /// What failed, last, each with what to run next: the same failure of several said once, naming them.
    fn failures(&self, ctx: &Context) {
        let mut said: Vec<(Vec<&str>, &str, &[String])> = vec![];
        for job in &self.board.jobs {
            if job.state == JobState::NotSent {
                continue;
            }
            let Some(error) = &job.error else { continue };
            let reason = (error.message.strip_prefix(&job.asset)).and_then(|rest| rest.strip_prefix(": "));
            let named = error.message.contains(&job.asset);
            let reason = reason.filter(|_| named).unwrap_or(&error.message);
            match said.iter_mut().find(|(_, said, _)| *said == reason) {
                Some((names, ..)) if names.contains(&job.asset.as_str()) => {}
                Some((names, ..)) => names.push(&job.asset),
                None => said.push((vec![&job.asset], reason, &error.next)),
            }
        }
        for (names, reason, next) in said {
            let of = names.join(", ");
            // A reason that names its asset says so itself.
            let line = if reason.contains(names[0]) { reason.to_owned() } else { format!("{of}: {reason}") };
            output::failure(ctx, &brief(&line), next);
        }
        for error in &self.errors {
            output::failure(ctx, &brief(&error.message), &error.next);
        }
        let unsent: Vec<&str> = self.board.jobs.iter().filter(|job| job.state == JobState::NotSent).map(|job| job.asset.as_str()).collect();
        if !unsent.is_empty() {
            anstream::println!("not sent, as its machines stopped: {}", unsent.join(", "));
        }
    }
}

/// A reason as far as a person reads it in a block: up to its traceback, and not past a few lines.
fn brief(reason: &str) -> String {
    let reason = reason.split_once("Traceback (most recent call last)").map_or(reason, |(said, _)| said).trim_end();
    match reason.char_indices().nth(REASON_SHOWN) {
        Some((at, _)) => format!("{}…", &reason[..at]),
        None => reason.to_owned(),
    }
}

fn width(sizes: impl Iterator<Item = usize>) -> usize {
    sizes.max().unwrap_or(0)
}

/// `take 14`, `version 13`, or for a file that comes with another's take, nothing.
fn numbered(job: &Job) -> String {
    job.number.map_or_else(String::new, |number| format!("{} {number}", job.noun))
}

/// A job's time, once it has one.
fn took(job: &Job) -> String {
    match (job.took_ms, job.started_at_ms) {
        (Some(ms), _) => clock(ms),
        (None, Some(at)) if job.state == JobState::Running => clock((now_ms() - at).max(0) as u64),
        _ => String::new(),
    }
}

/// `files/950b5ed8.mp4` for `files/<sha256>.mp4`; a film at the root stands as it is.
fn short(path: &str) -> String {
    let file = Path::new(path);
    match (file.parent(), file.file_stem().and_then(|stem| stem.to_str()), file.extension().and_then(|ext| ext.to_str())) {
        (Some(parent), Some(stem), Some(ext)) if stem.len() > 16 && stem.is_ascii() => format!("{}/{}.{ext}", parent.display(), &stem[..8]),
        _ => path.to_owned(),
    }
}

/// `2m07s`, under a minute `38s`, past an hour `1h02m`.
fn clock(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A render's last line: what it made and how long it took, with the generation time summed when renders ran at once.
fn finished(rendered: &Rendered) -> String {
    let count = |n: u32, noun: &str| (n > 0).then(|| format!("{n} {noun}{}", if n == 1 { "" } else { "s" }));
    let made: Vec<String> = [count(rendered.takes, "take"), count(rendered.versions, "version")].into_iter().flatten().collect();
    let generation = (rendered.execution_ms > rendered.elapsed_ms).then(|| format!(" ({} of generation)", clock(rendered.execution_ms)));
    format!("rendered {} in {}{}", made.join(" and "), clock(rendered.elapsed_ms), generation.unwrap_or_default())
}

/// What has been said of each job, so each change is said once. A stage's steps are said at each tenth of the way.
#[derive(Default)]
struct Lines {
    /// What it last said of each thing fetched ahead, by run.
    ahead: HashMap<u64, String>,
    said: HashSet<(String, Option<u32>, String)>,
    tenths: HashMap<(String, Option<u32>), i64>,
    /// A failure said once, whose it was.
    reasons: HashMap<String, String>,
}

impl Lines {
    fn first(&mut self, job: &Job, what: &str) -> bool {
        self.said.insert((job.asset.clone(), job.number, what.to_owned()))
    }

    fn board(&mut self, ctx: &Context, rendering: &Rendering) {
        // What is fetched ahead, as it starts a stage or every quarter of the way.
        for ahead in &rendering.board.ahead {
            // Nothing to say until it has bytes to fetch: what weighs nothing is ready as soon as it's asked.
            if ahead.total == 0 && ahead.held_back.is_none() {
                continue;
            }
            let Some(line) = ahead_line(ahead) else { continue };
            let quarter = if ahead.total > 0 { ahead.done * 4 / ahead.total } else { 0 };
            let at = format!("{:?} {quarter} {:?}", ahead.stage, ahead.held_back);
            if self.ahead.insert(ahead.run, at.clone()).as_ref() != Some(&at) {
                output::say(ctx, &line);
            }
        }
        for job in &rendering.board.jobs {
            let asset = &job.asset;
            let made = numbered(job);
            match job.state {
                JobState::Done if self.first(job, "done") => {
                    let mut line = format!("{asset}:");
                    if !made.is_empty() {
                        line += &format!(" {made}");
                    }
                    if let Some(ms) = job.took_ms {
                        line += &format!(" in {}", clock(ms));
                    }
                    if let Some(path) = &job.path {
                        line += &format!("  {}", rendering.file(ctx, path));
                    }
                    if let Some(seconds) = job.seconds {
                        line += &format!(", {seconds:.1} s");
                    }
                    if let Some(why) = &job.why {
                        line += &format!("  {why}");
                    }
                    output::say(ctx, &line);
                }
                JobState::Failed | JobState::Canceled if self.first(job, "failed") => {
                    let Some(error) = &job.error else { continue };
                    let reason = error.message.strip_prefix(asset.as_str()).unwrap_or(&error.message).trim_start_matches(": ").to_owned();
                    match self.reasons.get(&reason) {
                        Some(first) if first == asset => output::failure(ctx, &format!("{asset}: failed again, the same way"), &[]),
                        Some(first) => output::failure(ctx, &format!("{asset}: failed the same way as {first}"), &[]),
                        None => {
                            let line = if error.message.contains(asset.as_str()) { error.message.clone() } else { format!("{asset}: {}", error.message) };
                            let remedy = error.remedy.as_deref().map(|remedy| format!("\n  {remedy}")).unwrap_or_default();
                            output::failure(ctx, &format!("{line}{remedy}"), &error.next);
                            self.reasons.insert(reason, asset.clone());
                        }
                    }
                }
                JobState::NotSent if self.first(job, "not sent") => output::say(ctx, &format!("{asset}: not sent, as its machines stopped")),
                JobState::Waiting if job.number.is_some() => self.going(ctx, rendering, job),
                JobState::Running => {
                    if self.first(job, "running") {
                        let on = job.machine.as_deref().filter(|_| rendering.several()).map(|machine| format!(" on {machine}")).unwrap_or_default();
                        output::say(ctx, &format!("{asset}: {made} running{on}"));
                    }
                    self.going(ctx, rendering, job);
                }
                _ => {}
            }
        }
    }

    /// What a job on its way says: what holds it up, each reason once, not its bytes; each stage it enters and each tenth of the way;
    /// what it has shown; that it stopped. Its machine's own steps (`preparing`, saving what ended) are for the live view.
    fn going(&mut self, ctx: &Context, rendering: &Rendering, job: &Job) {
        let asset = &job.asset;
        if let Some(why) = job.why.as_deref().filter(|why| why.starts_with("waiting") || why.starts_with("uploading")) {
            let reason = why.split(':').next().unwrap_or(why);
            if self.first(job, reason) {
                output::say(ctx, &format!("{asset}: {why}"));
            }
        }
        if let Some(stage) = job.stage.as_ref().filter(|stage| job.took_ms.is_none() && *stage != "preparing") {
            let tenth = job.fraction.map_or(-1, |fraction| (fraction * 10.0) as i64);
            let entered = self.first(job, stage);
            let shown = self.tenths.entry((asset.clone(), job.number)).or_insert(-1);
            if entered || tenth > *shown {
                *shown = (*shown).max(tenth);
                let detail = job.detail.as_deref().map(|detail| format!(" {detail}")).unwrap_or_default();
                output::say(ctx, &format!("{asset}: {}{stage}{detail}", of_shot(rendering, job)));
            }
        }
        if let Some(preview) = &job.preview
            && self.first(job, &preview.path)
        {
            output::say(ctx, &format!("{asset}: {}{} ready: {}", of_shot(rendering, job), preview.label, rendering.file(ctx, &preview.path)));
        }
        if let Some(stalled) = &job.stalled
            && self.first(job, "stalled")
        {
            let cancel = job.run.map(|run| format!(". It keeps running until you end it: cozy run cancel {run}")).unwrap_or_default();
            output::say(ctx, &format!("{asset}: {}{cancel}", stalled_words(stalled)));
        }
    }
}

/// What a line on a job's way starts with: `shot 1/5 : `, of a job that makes a shot.
fn of_shot(rendering: &Rendering, job: &Job) -> String {
    rendering.shot(job).map(|shot| format!("{shot} : ")).unwrap_or_default()
}

/// The live view of a render: one row per job, drawn from the board.
pub(crate) struct View {
    rendering: Rendering,
    events: Watch<Event>,
    /// The row chosen by ↑/↓, by what it is making: rows move as they end.
    selected: Option<(String, Option<u32>)>,
    /// The person left it going.
    pub(crate) left: bool,
    /// Calcifer stopped telling.
    closed: bool,
}

/// Text over what ratatui drew, at a place on screen: a file's link, which ratatui's cells cannot hold.
pub(crate) struct Overlay {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) text: String,
}

/// One row of the view, and each file link in it: where it starts, what it reads, its path.
struct Drawn {
    line: Line<'static>,
    links: Vec<(usize, String, String)>,
}

impl View {
    /// Waits for what Calcifer says next. Cancel-safe.
    pub(crate) async fn changed(&mut self) {
        match self.events.next().await {
            Some(event) => drop(self.rendering.apply(event)),
            None => self.closed = true,
        }
    }

    /// Nothing more will come.
    pub(crate) fn over(&self) -> bool {
        self.closed || self.rendering.ended.is_some()
    }

    fn at(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rendering.board.jobs.iter().position(|job| job.asset == selected.0 && job.number == selected.1)
    }

    /// Moves the chosen row, from none to the first.
    pub(crate) fn select(&mut self, by: isize) {
        let jobs = &self.rendering.board.jobs;
        let at = match self.at() {
            Some(at) => at.saturating_add_signed(by).min(jobs.len().saturating_sub(1)),
            None => 0,
        };
        self.selected = jobs.get(at).map(|job| (job.asset.clone(), job.number));
    }

    /// The run Enter opens: the chosen row's, else the first running.
    pub(crate) fn opens(&self) -> Option<u64> {
        let jobs = &self.rendering.board.jobs;
        match self.at() {
            Some(at) => jobs[at].run,
            None => jobs.iter().find(|job| job.state == JobState::Running).and_then(|job| job.run),
        }
    }

    fn rows(&self) -> Vec<Drawn> {
        let rendering = &self.rendering;
        let jobs = &rendering.board.jobs;
        let name = width(jobs.iter().map(|job| job.asset.chars().count()));
        let made = width(jobs.iter().map(|job| numbered(job).chars().count()));
        let time = width(jobs.iter().map(|job| took(job).chars().count()));
        let dim = Style::new().add_modifier(Modifier::DIM);
        let row = |job: &Job| {
            let (state, style) = match job.state {
                JobState::Done => ("done", dim),
                JobState::Running => ("running", Style::new().add_modifier(Modifier::BOLD)),
                JobState::Waiting => ("waiting", dim),
                JobState::Failed => ("failed", failed_style()),
                JobState::Canceled => ("canceled", failed_style()),
                JobState::NotSent => ("not sent", failed_style()),
            };
            let mut links = vec![];
            let mut text = format!(" {:<name$}  ", job.asset);
            let waiting = job.number.is_none() && took(job).is_empty();
            if !waiting {
                let at = text.chars().count();
                let number = numbered(job);
                if let (Some(path), false) = (&job.path, number.is_empty()) {
                    links.push((at, number.clone(), path.clone()));
                }
                text += &format!("{number:<made$}  {:<time$}  ", took(job));
            }
            let mut notes: Vec<String> = vec![];
            if let Some(machine) = job.machine.as_deref().filter(|_| rendering.several()) {
                notes.push(format!("on {machine}"));
            }
            if let Some(error) = &job.error {
                let reason = error.message.strip_prefix(job.asset.as_str()).unwrap_or(&error.message).trim_start_matches(": ");
                notes.push(reason.trim_start_matches("failed: ").to_owned());
            }
            notes.extend(job.why.clone());
            // What the walker works out, for what hasn't ended.
            if let (Some(gpus), JobState::Waiting | JobState::Running) = (job.gpus.filter(|gpus| *gpus > 1), job.state) {
                notes.push(format!("{gpus} GPUs"));
            }
            if let (Some(ends), JobState::Waiting | JobState::Running) = (job.ends_at_ms, job.state) {
                let left = (ends - now_ms()).max(0) as u64;
                if left >= 10_000 {
                    notes.push(format!("about {}", clock(left)));
                }
            }
            if let Some(seconds) = job.seconds {
                notes.push(format!("{seconds:.1} s"));
            }
            match &job.stalled {
                Some(stalled) => notes.push(stalled_words(stalled)),
                None => {
                    let stage = job.stage.as_deref().map(|stage| match &job.detail {
                        Some(detail) => format!("{stage} {detail}"),
                        None => stage.to_owned(),
                    });
                    notes.extend(stage);
                }
            }
            // What a running shot is doing starts with which it is: `shot 1/5 : segment 2/2, denoise 14/30`.
            let mut note = notes.join(" · ");
            if let Some(shot) = rendering.shot(job).filter(|_| job.state == JobState::Running) {
                note = if note.is_empty() { shot } else { format!("{shot} : {note}") };
            }
            text += &note;
            if let Some(preview) = &job.preview {
                let file = short(&preview.path);
                if !note.is_empty() {
                    text += " · ";
                }
                text += &format!("{} ready: ", preview.label);
                links.push((text.chars().count(), file.clone(), preview.path.clone()));
                text += &file;
            }
            // The row's text sits behind its state.
            let links = links.into_iter().map(|(at, shown, path)| (2 + 8 + at, shown, path)).collect();
            let line = Line::from(vec![Span::raw("  "), Span::styled(format!("{state:<8}"), style), Span::raw(text)]);
            Drawn { line, links }
        };
        jobs.iter().map(row).collect()
    }

    /// One frame. What it can't draw itself is the links; they are returned to go over it.
    pub(crate) fn draw(&self, ctx: &Context, frame: &mut Frame, table: &mut TableState) -> Vec<Overlay> {
        let rendering = &self.rendering;
        let warned: Vec<String> = (rendering.warnings.iter().rev().take(WARNINGS_SHOWN).rev())
            .map(|warned| match warned.names.as_slice() {
                [] => format!("warning: {}", warned.message),
                names => format!("warning: {}: {}", names.join(", "), warned.message),
            })
            .collect();
        let said = rendering.said.last().filter(|said| said.name.is_none()).map(|said| said.message.clone());
        let ahead: Vec<String> = rendering.board.ahead.iter().filter_map(ahead_line).collect();
        let foot = warned.len() as u16 + ahead.len() as u16 + u16::from(said.is_some()) + 1;
        let [top, body, bottom] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(foot)]).areas(frame.area());

        let silence = self.events.silence().map(|silence| format!(" · {silence}")).unwrap_or_default();
        frame.render_widget(Line::from(format!("{}{silence}", rendering.title())).style(Style::new().add_modifier(Modifier::BOLD)), top);
        frame.render_widget(Line::from(format!("{} · {}", rendering.count(), rendering.elapsed())).right_aligned(), top);

        let rows = self.rows();
        let selected = self.at();
        table.select(selected);
        if selected.is_none() {
            // What is still going stays in sight: the first rows to go are what has ended.
            let going = rendering.board.jobs.iter().position(|job| matches!(job.state, JobState::Running | JobState::Waiting)).unwrap_or(rows.len());
            *table.offset_mut() = going.min(rows.len().saturating_sub(body.height as usize));
        }
        let widget = Table::new(rows.iter().map(|row| Row::new([Cell::from(row.line.clone())])), [Constraint::Fill(1)])
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(widget, body, table);

        let dim = Style::new().add_modifier(Modifier::DIM);
        let mut lines: Vec<Line> = warned.into_iter().map(Line::from).collect();
        lines.extend(ahead.into_iter().map(|line| Line::styled(line, dim)));
        lines.extend(said.map(|said| Line::styled(said, dim)));
        lines.push(Line::styled("↑/↓, Enter opens a run, q leaves it rendering", dim));
        frame.render_widget(Paragraph::new(lines), bottom);

        self.overlays(ctx, &rows, body, table.offset(), selected)
    }

    /// Each row's links where its row is on screen, as far as the row is in sight.
    fn overlays(&self, ctx: &Context, rows: &[Drawn], body: Rect, offset: usize, selected: Option<usize>) -> Vec<Overlay> {
        if !ctx.term.links() {
            return vec![];
        }
        let mut over = vec![];
        for (i, row) in rows.iter().enumerate().skip(offset).take(body.height as usize) {
            let y = body.y + (i - offset) as u16;
            for (at, shown, path) in &row.links {
                let room = (body.width as usize).saturating_sub(*at);
                if room == 0 {
                    continue;
                }
                let shown: String = shown.chars().take(room).collect();
                let Some(link) = output::hyperlink(ctx.term, &self.rendering.dir.join(path), &shown) else { continue };
                let text = if selected == Some(i) { format!("\x1b[7m{link}\x1b[27m") } else { link };
                over.push(Overlay { x: body.x + *at as u16, y, text });
            }
        }
        over
    }
}

fn failed_style() -> Style {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    match std::env::var_os("NO_COLOR") {
        Some(_) => bold,
        None => bold.fg(Color::Red),
    }
}
