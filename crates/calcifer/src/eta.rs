//! A run's time left (CALCIFER-API.md "Time left"). Before it runs, a prior: what the runs like it took, phase by phase, on this
//! machine, else on others of its GPU. As it runs, that prior blended with its own steps: a stage's first step carries its warm-up and
//! is never its pace; the median of its latest steps is, weighed more with each one. Phases it hasn't reached come from the prior.
//! What's shown eases to a new estimate over a second or two: it never jumps. With no prior and under two steps of its own, it says so.
//! Times are milliseconds on its machine's clock, which this computer's `now` is read on by the least lag seen of its reports. A run
//! sent on its machine's quote (cl-816) waits at least until that quote's finish, and with no runs like it here, the quote is its prior.

use std::collections::BTreeMap;

use api::{
    calcifer::{EtaBasis, Progress},
    machine::{Basis, JobSize},
};
use serde::Deserialize;

/// How many of the newest runs like it a prior stands on.
const RECENT: usize = 5;
/// A run within this share of another's megapixels is its size.
const SAME_SIZE: f64 = 0.15;
/// What a prior weighs against a run's own steps, in steps: from this machine, from another of its GPU.
const MACHINE_WEIGHT: f64 = 3.0;
const GPU_WEIGHT: f64 = 2.0;
/// How long a shown time takes to ease most of the way to a new estimate, in ms.
const EASE_MS: f64 = 1000.0;

/// What one finished run took, phase by phase.
#[derive(Clone, Debug, Default)]
pub(crate) struct Took {
    pub machine: String,
    pub gpu: String,
    pub megapixels: Option<f64>,
    /// The GPUs it ran on, as its executor said: 1 when it didn't.
    pub width: u32,
    /// From its start to its first stage: its executor's start and its models' load.
    pub lead_ms: f64,
    /// Its stages in the order they began, none of them a `host ·` span inside another.
    pub stages: Vec<StageTook>,
    /// From its last stage's end to its own: its outputs encoded and sent.
    pub tail_ms: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StageTook {
    pub name: String,
    pub ms: f64,
    pub steps: Option<Steps>,
}

/// A stage that steps: how many, its first's time and its steady pace.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Steps {
    pub count: u64,
    pub first_ms: f64,
    pub steady_ms: f64,
}

impl Steps {
    fn ms(self, count: u64) -> f64 {
        self.first_ms + count.saturating_sub(1) as f64 * self.steady_ms
    }
}

/// The part of a run's measurements this reads (`attribution`), each span by its wall bounds.
#[derive(Deserialize, Default)]
struct Measured {
    #[serde(default)]
    attribution: Attribution,
    #[serde(default)]
    execution: Execution,
}

#[derive(Deserialize, Default)]
struct Execution {
    #[serde(default)]
    degree: u32,
}

#[derive(Deserialize, Default)]
struct Attribution {
    #[serde(default)]
    stages: BTreeMap<String, Span>,
    #[serde(default)]
    steps: BTreeMap<String, Track>,
}

#[derive(Deserialize)]
struct Span {
    started_unix_ms: f64,
    ended_unix_ms: f64,
    total_ms: f64,
}

#[derive(Deserialize)]
struct Track {
    #[serde(default)]
    series: Vec<(f64, f64)>,
    #[serde(default)]
    count: u64,
    #[serde(default)]
    first_ms: f64,
    #[serde(default)]
    total_ms: f64,
}

impl Took {
    /// A completed run's phases from its executor's `measurements` (JSON), its machine's start (ms) and its execution's time; none
    /// without them.
    pub(crate) fn of(machine: &str, gpu: &str, started_at_ms: i64, execution_ms: u64, measurements: &str, megapixels: Option<f64>) -> Option<Self> {
        if started_at_ms <= 0 || execution_ms == 0 {
            return None;
        }
        let measured: Measured = serde_json::from_str(measurements).ok()?;
        let (started, end) = (started_at_ms as f64, started_at_ms as f64 + execution_ms as f64);
        let attribution = measured.attribution;
        let mut spans: Vec<(&String, &Span)> = attribution.stages.iter().filter(|(name, _)| !name.starts_with("host · ")).collect();
        spans.sort_by(|a, b| a.1.started_unix_ms.total_cmp(&b.1.started_unix_ms));
        let stages: Vec<StageTook> = spans
            .iter()
            .map(|(name, span)| StageTook { name: (*name).clone(), ms: span.total_ms.max(0.0), steps: attribution.steps.get(*name).and_then(steps_of) })
            .collect();
        let (lead_ms, tail_ms) = match (spans.first(), spans.iter().map(|(_, span)| span.ended_unix_ms).reduce(f64::max)) {
            (Some((_, first)), Some(last)) => ((first.started_unix_ms - started).clamp(0.0, end - started), (end - last).max(0.0)),
            _ => (end - started, 0.0),
        };
        Some(Took { machine: machine.into(), gpu: gpu.into(), megapixels, width: measured.execution.degree.max(1), lead_ms, stages, tail_ms })
    }

    fn total_ms(&self) -> f64 {
        self.lead_ms + self.stages.iter().map(|stage| stage.ms).sum::<f64>() + self.tail_ms
    }

    /// As it would have been at `megapixels`: its steps and what follows them scale with the picture, what comes before doesn't.
    fn at(&self, megapixels: Option<f64>) -> (Self, bool) {
        let (Some(want), Some(had)) = (megapixels, self.megapixels.filter(|mp| *mp > 0.0)) else { return (self.clone(), false) };
        if (want / had - 1.0).abs() <= SAME_SIZE {
            return (self.clone(), false);
        }
        let ratio = want / had;
        let mut scaled = self.clone();
        let from = scaled.stages.iter().position(|stage| stage.steps.is_some()).unwrap_or(scaled.stages.len());
        for stage in &mut scaled.stages[from..] {
            stage.ms *= ratio;
            if let Some(steps) = &mut stage.steps {
                (steps.first_ms, steps.steady_ms) = (steps.first_ms * ratio, steps.steady_ms * ratio);
            }
        }
        scaled.megapixels = Some(want);
        (scaled, true)
    }
}

fn steps_of(track: &Track) -> Option<Steps> {
    let count = track.count.max(track.series.len() as u64);
    if count == 0 {
        return None;
    }
    let after: Vec<f64> = track.series.iter().skip(1).map(|(_, ms)| *ms).collect();
    let first_ms = track.series.first().map_or(track.first_ms, |(_, ms)| *ms);
    let steady_ms = match median(&after) {
        Some(steady) => steady,
        None if count > 1 => (track.total_ms - first_ms) / (count - 1) as f64,
        None => first_ms,
    };
    Some(Steps { count, first_ms, steady_ms })
}

fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    (n > 0).then(|| if n % 2 == 1 { sorted[n / 2] } else { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 })
}

/// What a run like these takes, phase by phase: each the median of the newest runs'.
#[derive(Clone, Debug)]
pub(crate) struct Prior {
    pub basis: EtaBasis,
    pub runs: usize,
    pub lead_ms: f64,
    pub stages: Vec<StageTook>,
    pub tail_ms: f64,
    /// How much it is trusted, 0 to 1, before anything of the run is seen.
    pub confidence: f64,
    /// What it weighs against the run's own steps, in steps.
    weight: f64,
}

impl Prior {
    /// The runs like it, newest first: the newest of this machine's; with none, of other machines of its GPU. Of one size, those of the
    /// run's own (`megapixels`), else all of them scaled to it.
    pub(crate) fn of(history: &[Took], machine: &str, gpu: &str, megapixels: Option<f64>) -> Option<Self> {
        let here: Vec<&Took> = history.iter().filter(|took| took.machine == machine).take(RECENT).collect();
        let (basis, like, weight, trust) = match here.is_empty() {
            false => (EtaBasis::Machine, here, MACHINE_WEIGHT, 0.8),
            true if !gpu.is_empty() => {
                let same: Vec<&Took> = history.iter().filter(|took| took.gpu == gpu).take(RECENT).collect();
                (EtaBasis::Gpu, same, GPU_WEIGHT, 0.6)
            }
            true => return None,
        };
        if like.is_empty() {
            return None;
        }
        let sized: Vec<&Took> = match megapixels {
            Some(want) => like.iter().copied().filter(|took| took.megapixels.is_some_and(|had| (want / had - 1.0).abs() <= SAME_SIZE)).collect(),
            None => vec![],
        };
        let chosen: Vec<(Took, bool)> = if sized.is_empty() { like.iter().map(|took| took.at(megapixels)).collect() } else { sized.iter().map(|took| ((*took).clone(), false)).collect() };
        let scaled = chosen.iter().any(|(_, scaled)| *scaled);
        let tooks: Vec<&Took> = chosen.iter().map(|(took, _)| took).collect();
        let of = |pick: &dyn Fn(&Took) -> Option<f64>| median(&tooks.iter().filter_map(|took| pick(took)).collect::<Vec<_>>());
        let newest = tooks[0];
        let stages = newest
            .stages
            .iter()
            .map(|stage| {
                let named = |took: &Took| took.stages.iter().find(|s| s.name == stage.name).cloned();
                let steps = stage.steps.map(|newest| Steps {
                    count: newest.count,
                    first_ms: of(&|took| named(took)?.steps.map(|s| s.first_ms)).unwrap_or(newest.first_ms),
                    steady_ms: of(&|took| named(took)?.steps.map(|s| s.steady_ms)).unwrap_or(newest.steady_ms),
                });
                StageTook { name: stage.name.clone(), ms: of(&|took| Some(named(took)?.ms)).unwrap_or(stage.ms), steps }
            })
            .collect();
        // Runs that disagree are trusted less: the spread of their totals about their median.
        let totals: Vec<f64> = tooks.iter().map(|took| took.total_ms()).collect();
        let middle = median(&totals).unwrap_or(0.0);
        let spread = median(&totals.iter().map(|total| (total - middle).abs()).collect::<Vec<_>>()).unwrap_or(0.0) / middle.max(1.0);
        let (weight, trust) = if scaled { (weight / 2.0, trust * 0.8) } else { (weight, trust) };
        Some(Prior {
            basis,
            runs: tooks.len(),
            lead_ms: of(&|took| Some(took.lead_ms)).unwrap_or(0.0),
            stages,
            tail_ms: of(&|took| Some(took.tail_ms)).unwrap_or(0.0),
            confidence: trust * (1.0 - spread.min(0.5)),
            weight,
        })
    }

    /// A whole run's time and nothing of its phases: its machine's quote, standing on `basis`.
    fn whole(ms: f64, basis: EtaBasis) -> Self {
        let confidence = if basis == EtaBasis::Machine { 0.6 } else { 0.4 };
        Prior { basis, runs: 0, lead_ms: ms, stages: vec![], tail_ms: 0.0, confidence, weight: GPU_WEIGHT }
    }

    /// As asked for `steps` steps, where its input says: each stage that steps takes that many.
    pub(crate) fn with_steps(mut self, steps: Option<u64>) -> Self {
        for each in self.stages.iter_mut().filter_map(|stage| stage.steps.as_mut()) {
            each.count = steps.filter(|n| *n > 0).unwrap_or(each.count);
        }
        self
    }

    /// Its stage's time, at `steps` steps when it steps and they're known.
    fn stage_ms(&self, at: usize, steps: Option<u64>) -> f64 {
        let stage = &self.stages[at];
        match (stage.steps, steps) {
            (Some(each), Some(count)) => each.ms(count),
            (Some(each), None) => each.ms(each.count),
            _ => stage.ms,
        }
    }

    /// The whole run, from its start: at `steps` steps where it steps.
    pub(crate) fn total_ms(&self, steps: Option<u64>) -> f64 {
        self.lead_ms + (0..self.stages.len()).map(|at| self.stage_ms(at, steps)).sum::<f64>() + self.tail_ms
    }

    /// What follows stage `after` (none: all of them), to its end.
    fn rest_ms(&self, after: Option<usize>) -> f64 {
        let from = after.map_or(0, |at| at + 1);
        (from..self.stages.len()).map(|at| self.stage_ms(at, None)).sum::<f64>() + self.tail_ms
    }
}

/// What a run of `size` takes on one GPU, by the runs like it at one width on `machine`, else on its GPU: a machine's
/// estimate's `prior_ms`, used where that machine ran none of it.
pub(crate) fn one_gpu_ms(history: &[Took], machine: &str, gpu: &str, size: &JobSize) -> Option<f64> {
    let one: Vec<Took> = history.iter().filter(|took| took.width <= 1).cloned().collect();
    Some(Prior::of(&one, machine, gpu, size.megapixels)?.with_steps(size.steps).total_ms(size.steps))
}

/// What a run's machine quoted as it was sent (cl-816), on this computer's clock: when it would start and finish, and what that
/// stands on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Quoted {
    pub start_at: f64,
    pub finish_at: f64,
    pub basis: Basis,
}

/// An estimate of what a run has left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Estimate {
    pub left_ms: f64,
    pub confidence: f64,
    pub basis: EtaBasis,
}

/// A running run's own steps in the stage it steps in.
#[derive(Clone, Debug, Default)]
struct Stepping {
    stage: String,
    completed: u64,
    total: u64,
    /// When its latest step ended (its stage's start before one did).
    last_at: f64,
    /// Its steps after the first, as seen here, the latest few.
    own: Vec<f64>,
    /// Its machine's steady pace, when it says.
    steady_ms: f64,
}

/// What Calcifer follows of one run to say its time left.
#[derive(Clone, Debug, Default)]
pub(crate) struct Clock {
    prior: Option<Prior>,
    /// When its execution began.
    started: Option<f64>,
    /// The stage it's in: its name, where the prior has it, and since when.
    stage: Option<(String, Option<usize>, f64)>,
    /// The furthest of the prior's stages it has reached.
    passed: Option<usize>,
    steps: Option<Stepping>,
    /// The download it waits on before it starts: ms left at its rate.
    fetching_ms: Option<f64>,
    /// What was last shown, and when.
    shown: Option<(f64, f64)>,
    /// How far this computer's clock is ahead of its machine's, as the least lag of a report seen.
    ahead: Option<f64>,
    /// Its machine's quote: it waits until at least its finish.
    quoted: Option<Quoted>,
}

impl Clock {
    pub(crate) fn new(prior: Option<Prior>) -> Self {
        Self { prior, ..Self::default() }
    }

    /// What its machine quoted: with no runs like it here, its run's time is the prior, unless that stands on nothing.
    pub(crate) fn quoted(&mut self, quoted: Quoted) {
        let basis = match quoted.basis {
            Basis::Measured => Some(EtaBasis::Machine),
            Basis::Prior => Some(EtaBasis::Gpu),
            Basis::None => None,
        };
        if let (None, Some(basis)) = (&self.prior, basis) {
            self.prior = Some(Prior::whole((quoted.finish_at - quoted.start_at).max(0.0), basis));
        }
        self.quoted = Some(quoted);
    }

    /// Its execution began at `at`.
    pub(crate) fn started(&mut self, at: f64) {
        self.started.get_or_insert(at);
    }

    /// A progress report its machine made at `at`, received here at `received`.
    pub(crate) fn observe(&mut self, progress: &Progress, at: f64, received: f64) {
        self.ahead = Some(self.ahead.map_or(received - at, |ahead| ahead.min(received - at)));
        // A stage of its executor's says it began, if its machine never did.
        if self.started.is_none() && self.prior.as_ref().is_some_and(|prior| prior.stages.iter().any(|stage| stage.name == progress.stage)) {
            self.started = Some(at);
        }
        if self.started.is_none() {
            self.fetching_ms = (progress.bytes_per_s.filter(|rate| *rate >= 1.0))
                .map(|rate| progress.bytes_total.saturating_sub(progress.bytes_done) as f64 / rate * 1000.0);
            return;
        }
        if progress.stage.is_empty() {
            return;
        }
        if self.stage.as_ref().is_none_or(|(name, ..)| *name != progress.stage) {
            let found = self.prior.as_ref().and_then(|prior| prior.stages.iter().position(|stage| stage.name == progress.stage));
            // A stage it enters again, or one the prior has before where it got, is where it is: it never goes back.
            let index = found.filter(|at| self.passed.is_none_or(|passed| *at >= passed));
            if index.is_some() {
                self.passed = index;
            }
            self.stage = Some((progress.stage.clone(), index, at));
        }
        if progress.total == 0 {
            return;
        }
        let steps = self.steps.get_or_insert_with(Stepping::default);
        if steps.stage != progress.stage {
            *steps = Stepping { stage: progress.stage.clone(), last_at: at, ..Stepping::default() };
        }
        if progress.completed != steps.completed {
            if progress.completed >= 2 && progress.step_ms > 0.0 {
                steps.own.push(progress.step_ms);
                if steps.own.len() > RECENT {
                    steps.own.remove(0);
                }
            }
            (steps.completed, steps.last_at) = (progress.completed, at);
        }
        (steps.total, steps.steady_ms) = (progress.total, progress.step_steady_ms);
    }

    /// What it has left at `now` (this computer's), as best known; none with no basis.
    pub(crate) fn estimate(&self, here: f64) -> Option<Estimate> {
        let now = here - self.ahead.unwrap_or(0.0);
        let prior = self.prior.as_ref();
        let whole = prior.filter(|prior| prior.stages.is_empty());
        // What's left of the phase it's in, and how well that phase keeps to what was expected of it.
        let ((current, fit), rest) = match (self.started, &self.stage, whole) {
            (None, ..) => {
                let queued = self.quoted.map_or(0.0, |quoted| quoted.finish_at - here);
                let fetching = self.fetching_ms.unwrap_or(0.0);
                ((fetching, 1.0), (prior?.total_ms(None)).max(queued - fetching))
            }
            // The whole run's time alone: what's left of it, or what its own steps say once they say more.
            (Some(started), stage, Some(whole)) => {
                let counted = left_of(whole.lead_ms, now - started);
                let stepped = stage.as_ref().and_then(|(name, _, at)| {
                    let steps = self.steps.as_ref().filter(|steps| steps.stage == *name && steps.total > 0)?;
                    self.steps_left(steps, None, *at, now)
                });
                match stepped {
                    Some((left, _)) if left > counted.0 => ((left, counted.1), 0.0),
                    _ => (counted, 0.0),
                }
            }
            (Some(started), None, None) => (left_of(prior?.lead_ms, now - started), prior?.rest_ms(None)),
            (Some(_), Some((name, index, at)), None) => {
                let stepping = self.steps.as_ref().filter(|steps| steps.stage == *name && steps.total > 0);
                let current = match (stepping, index) {
                    (Some(steps), _) => self.steps_left(steps, *index, *at, now)?,
                    (None, Some(index)) => left_of(prior?.stage_ms(*index, None), now - at),
                    // A stage the prior doesn't know: nothing is known of it.
                    (None, None) => {
                        prior?;
                        (0.0, 1.0)
                    }
                };
                (current, prior.map_or(0.0, |prior| prior.rest_ms(index.or(self.passed))))
            }
        };
        let left = current + rest;
        let basis = prior.map_or(EtaBasis::Steps, |prior| prior.basis);
        // As trusted as what it stands on, less while a phase runs past what was expected, and more with each step of its own.
        let trust = prior.map_or(0.3, |prior| prior.confidence) * fit;
        let weight = prior.map_or(MACHINE_WEIGHT, |prior| prior.weight);
        let seen = self.steps.as_ref().map_or(0.0, |steps| steps.completed.saturating_sub(1) as f64);
        let own = seen / (weight + seen).max(1.0);
        Some(Estimate { left_ms: left.max(0.0), confidence: (trust + (1.0 - trust) * own).clamp(0.0, 1.0), basis })
    }

    /// What its stepping stage has left: its current step and those after it, at a pace that is the prior's at first and its own
    /// more with each step; never its first step's alone.
    fn steps_left(&self, steps: &Stepping, index: Option<usize>, entered: f64, now: f64) -> Option<(f64, f64)> {
        if steps.completed >= steps.total {
            return Some((0.0, 1.0));
        }
        let before = index.and_then(|at| self.prior.as_ref()?.stages[at].steps);
        let weight = self.prior.as_ref().map_or(0.0, |prior| prior.weight);
        let seen = steps.completed.saturating_sub(1) as f64;
        let own = Some(steps.steady_ms).filter(|ms| *ms > 0.0).or_else(|| median(&steps.own));
        let pace = match (before, own) {
            (Some(before), Some(own)) if seen >= 1.0 => (weight * before.steady_ms + seen * own) / (weight + seen),
            (Some(before), _) => before.steady_ms,
            (None, Some(own)) if seen >= 1.0 => own,
            _ => return None,
        };
        let (current, fit) = match steps.completed {
            0 => left_of(before?.first_ms, now - entered),
            _ => left_of(pace, now - steps.last_at),
        };
        Some((current + (steps.total - steps.completed - 1) as f64 * pace, fit))
    }

    /// What to show at `now`, in seconds: the estimate, eased from what was shown when it moved a lot.
    pub(crate) fn shown(&mut self, now: f64) -> Option<(i64, f64, EtaBasis)> {
        let estimate = self.estimate(now)?;
        let next = match self.shown {
            None => estimate.left_ms,
            Some((was, at)) => {
                let since = (now - at).max(0.0);
                let counted = (was - since).max(0.0);
                let gap = estimate.left_ms - counted;
                match gap.abs() <= (0.1 * counted).max(2000.0) {
                    true => estimate.left_ms,
                    false => counted + gap * (1.0 - (-since / EASE_MS).exp()),
                }
            }
        };
        self.shown = Some((next, now));
        Some(((next / 1000.0).round() as i64, (estimate.confidence * 100.0).round() / 100.0, estimate.basis))
    }
}

/// A phase expected to take `expected` that has run `ran`: what's left of it, none once it overran, and how well it keeps to it (1:
/// within it; past it, `expected` over `ran`).
fn left_of(expected: f64, ran: f64) -> (f64, f64) {
    let ran = ran.max(0.0);
    ((expected - ran).max(0.0), if ran > expected { expected / ran } else { 1.0 })
}
