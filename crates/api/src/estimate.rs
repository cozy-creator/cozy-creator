//! How a machine lays out its GPU work (cl-816, Paul 11-07), and when a job sent there would run (cl-967, Paul 11-12: "calcifer
//! could compute the quote on its end, if it has sufficient knowledge of each machine"): the one model its scheduler gives each root
//! its width by, and Calcifer quotes every machine by from the state it pushes (`quote`), one on its way included (cl-915). Roots
//! start in the order they are ready, acceptance breaking ties, on the GPUs free soonest; an older root waiting for GPUs keeps them
//! from younger ones. As each starts it takes its width in the plan that ends all that waits soonest, of those capping every job at
//! one width; within 2% the wider plan wins, its first results coming sooner. So a lone job runs wide and several queued jobs one
//! GPU each. A machine's downloads and installs each go one at a time in job order (Paul 11-12), so a job's weights come after
//! those of the jobs ahead of it. A function none of whose runs it knows is timed by the caller's prior, else by its release's
//! benchmarks (`References`, cl-1024), else a minute.
use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::machine::{self as v1, Basis, EstimateJob, JobSize, Queued, Quote, StatusFrame, TimedRun};

/// A function's time when none of it ran on the machine and the caller knows none.
pub const UNKNOWN_MS: f64 = 60_000.0;
/// An install the machine has not timed.
pub const INSTALL_MS: f64 = 120_000.0;
/// Weights' arrival, bytes a millisecond, until the machine has fetched enough to know its link.
pub const LINK: f64 = 100_000.0;

/// A job's preparation before it can start on a machine that holds nothing of it, with nothing ahead: its install, then its
/// weights' download at `link` (bytes a millisecond) and their load onto the GPUs.
pub fn prepare_ms(installed: bool, fetch: u64, link: f64, load_ms: f64) -> f64 {
    let install = if installed { 0.0 } else { INSTALL_MS };
    let load = match fetch {
        0 => 0.0,
        fetch => fetch as f64 / link + load_ms,
    };
    install + load
}

/// The widths a function runs at on `gpus` GPUs when its interface isn't held there: the count asked, else one GPU; none on a machine
/// without GPUs.
pub fn unheld_widths(gpus: usize, asked: u32) -> Vec<u32> {
    match (gpus, asked) {
        (0, _) => vec![],
        (_, 0) => vec![1],
        (held, asked) => vec![asked.min(held as u32)],
    }
}

/// One root's work.
#[derive(Clone, Debug)]
pub struct Job {
    /// The widths it runs at, smallest first; none: it holds no GPU.
    pub widths: Vec<u32>,
    /// Its time on one GPU, ms.
    pub one_ms: f64,
    pub scaling: Scaling,
    /// The soonest it can start, ms from now: when its preparation ends.
    pub ready_ms: f64,
    /// It runs: its width and the ms it has left.
    pub running: Option<(u32, f64)>,
}

impl Job {
    pub fn ms(&self, width: u32) -> f64 {
        self.one_ms * self.scaling.at(width)
    }
}

/// How a job goes faster with GPUs: the share of it that doesn't.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scaling {
    pub serial: f64,
}

impl Scaling {
    /// Until two widths have run: 2 GPUs near 1.8x, 4 near 3x, about what fast-linked cards do.
    pub const PRIOR: Scaling = Scaling { serial: 0.1 };

    /// From its times at two widths.
    pub fn of((a, at_a): (u32, f64), (b, at_b): (u32, f64)) -> Self {
        let (a, b, r) = (f64::from(a), f64::from(b), at_a / at_b);
        let under = (1.0 - 1.0 / a) - r * (1.0 - 1.0 / b);
        match under.abs() > f64::EPSILON && a != b {
            true => Scaling { serial: ((r / b - 1.0 / a) / under).clamp(0.0, 1.0) },
            false => Self::PRIOR,
        }
    }

    pub fn at(self, width: u32) -> f64 {
        self.serial + (1.0 - self.serial) / f64::from(width.max(1))
    }
}

/// Where a job goes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placed {
    pub width: u32,
    pub start_ms: f64,
    pub finish_ms: f64,
    /// Waiting jobs that start before it.
    pub position: usize,
}

/// Within this share of the soonest end, the wider plan wins.
const TIE: f64 = 0.02;

/// `jobs` on `gpus` GPUs, in their order (running ones first, then by acceptance).
pub fn pack(gpus: usize, jobs: &[Job]) -> Vec<Placed> {
    let mut free = vec![0.0; gpus];
    let mut placed = vec![Placed::default(); jobs.len()];
    for (at, job) in jobs.iter().enumerate() {
        if let Some((width, left)) = job.running {
            let width = width.min(gpus as u32);
            take(&mut free, width, 0.0, left);
            placed[at] = Placed { width, start_ms: 0.0, finish_ms: left, position: 0 };
        }
    }
    let mut waiting: Vec<usize> = (0..jobs.len()).filter(|&at| jobs[at].running.is_none()).collect();
    waiting.sort_by(|&a, &b| jobs[a].ready_ms.total_cmp(&jobs[b].ready_ms).then(a.cmp(&b)));
    for (position, &at) in waiting.iter().enumerate() {
        let cap = cap(&free, jobs, &waiting[position..]);
        let (width, start_ms, finish_ms) = place(&mut free, &jobs[at], cap);
        placed[at] = Placed { width, start_ms, finish_ms, position };
    }
    placed
}

/// The cap of the plan for `rest` from `free` that ends soonest, the widest within `TIE`.
fn cap(free: &[f64], jobs: &[Job], rest: &[usize]) -> u32 {
    let mut caps: Vec<u32> = rest.iter().flat_map(|&at| jobs[at].widths.iter().copied()).collect();
    caps.sort_unstable();
    caps.dedup();
    let ends: Vec<(u32, f64)> = caps
        .into_iter()
        .map(|cap| {
            let mut line = free.to_vec();
            (cap, rest.iter().map(|&at| place(&mut line, &jobs[at], cap).2).fold(0.0, f64::max))
        })
        .collect();
    let soonest = ends.iter().map(|(_, end)| *end).fold(f64::INFINITY, f64::min);
    ends.iter().filter(|(_, end)| *end <= soonest * (1.0 + TIE)).map(|(cap, _)| *cap).max().unwrap_or(1)
}

/// `job` at its widest under `cap` (its narrowest when none is) on the GPUs of `free` free
/// soonest: its width, start and end.
fn place(free: &mut [f64], job: &Job, cap: u32) -> (u32, f64, f64) {
    let fitting = || job.widths.iter().copied().filter(|width| *width as usize <= free.len());
    let Some(width) = fitting().filter(|width| *width <= cap).max().or_else(|| fitting().min()) else {
        return (0, job.ready_ms, job.ready_ms + job.ms(1));
    };
    let ms = job.ms(width);
    let start = take(free, width, job.ready_ms, ms);
    (width, start, start + ms)
}

/// Holds the `width` GPUs of `free` free soonest, from `ready` or once they are, for `ms`: when it starts.
fn take(free: &mut [f64], width: u32, ready: f64, ms: f64) -> f64 {
    let width = (width as usize).min(free.len());
    if width == 0 {
        return ready;
    }
    free.sort_by(f64::total_cmp);
    let start = ready.max(free[width - 1]);
    free.iter_mut().take(width).for_each(|gpu| *gpu = start + ms);
    start
}

/// The runs a time stands on, newest first, and of them those at one width.
pub const HISTORY: usize = 40;
const RECENT: usize = 5;
/// Within this share of a run's size, it is that size.
const SAME_SIZE: f64 = 0.15;
/// A running root past its time has at least this share of it left.
const OVERRUN: f64 = 0.1;

/// A width proved faster than one GPU on real pods for a function, a card and the link between its GPUs (cr-268): a machine runs a
/// root that wide before it has timed both widths itself. Each row names its proof; a width neither here nor timed faster there is
/// never chosen.
struct Known {
    /// `package/function`, the package without its org.
    function: &'static str,
    /// Part of the card's name as NVML gives it.
    card: &'static str,
    nvlink: bool,
    width: u32,
    /// Its time on one GPU over its time at `width`.
    speedup: f64,
}

const KNOWN: &[Known] = &[
    // h3a-099, rc/0.24.0, 15 s Turbo over host memory: one GPU 300.4 s (45660/45667/45686), two 175.4 s (45740/45759/45779).
    Known { function: "minimax-h3/generate", card: "RTX 4090", nvlink: false, width: 2, speedup: 1.71 },
    // cr-268, Runtime rc/0.25.0 b6633040, 15 s Turbo, copy-engine exchange: one GPU 168.3 s (46701), two 92.2 s (46708, 46713).
    Known { function: "minimax-h3/generate", card: "H100", nvlink: true, width: 2, speedup: 1.83 },
    // cr-268, estimated: the NVLink pair's split work (two GPUs 0.52 of one) plus rabou's exposed exchange over PCIe peers
    // (14.2 s of a 249 s denoise, 46678): about 1.8x denoise, 1.7x whole. Replace with a measured PCIe pair.
    Known { function: "minimax-h3/generate", card: "H100", nvlink: false, width: 2, speedup: 1.7 },
];

/// The widths `KNOWN` proves for `function` on `card` (`nvlink`), with their speed-ups.
fn known(function: &str, card: &str, nvlink: bool) -> Vec<(u32, f64)> {
    KNOWN.iter()
        .filter(|row| (function == row.function || function.ends_with(&format!("/{}", row.function)))
            && card.contains(row.card) && nvlink == row.nvlink)
        .map(|row| (row.width, row.speedup))
        .collect()
}

/// One of a release's benchmarks (cl-1024, Paul 11-15: "for estimates … use package-author benchmarks + our own benchmarks as
/// authoritative"): its publisher's measurement of one of its functions on a GPU, the request it ran and how long it took. A release
/// declares them when it is published, the Hub serves them with it, and they never change. What a machine that never ran the function
/// is quoted by, and what a package page lists.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Benchmark {
    /// A callable of the release.
    pub function: String,
    /// As its driver names it: `NVIDIA GeForce RTX 4090`.
    pub gpu: String,
    pub gpus: u32,
    pub request: JobSize,
    /// A warm run's time: the model loaded, its plan compiled.
    pub warm_ms: u64,
    /// The function's first run on the machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ran_on: Option<BenchmarkRanOn>,
    /// Where it was measured, such as a gate run.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// The software a benchmark ran on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BenchmarkRanOn {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cozy: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub runtime: String,
}

/// What a quote stands on past a machine's own picture: the benchmarks of what it never ran (cl-1024), each function's release's rows
/// and the Hub's per-card speeds, which scale a row to a card none measured; and the caller's clocks of its runs going there (cl-1073).
/// Calcifer's; a machine packing its own queue has none.
#[derive(Clone, Debug, Default)]
pub struct References {
    /// By function as `package/function` (no org, no release): its release's rows.
    pub rows: HashMap<String, Vec<Benchmark>>,
    /// By function, as `rows`: each card the Hub sells for it, as the Hub names it, and its speed at its lane. A time on one card
    /// times its speed over another's is the other's.
    pub speeds: HashMap<String, BTreeMap<String, f64>>,
    /// By function, as `rows`: every kept package's newest rows (th-377), which say how cards compare (`ratio`).
    pub fleet: HashMap<String, Vec<Benchmark>>,
    /// By a running root's caller id (`Queued::run`): its time left there, as its caller's clock says it.
    pub left: HashMap<String, Left>,
}

/// A running root's time left on its machine as its caller's clock says it (cl-1073): from its steps, its weights moving and the runs
/// like it, where a machine's picture has only its function's time and its executor's start, which comes before its download and load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Left {
    pub ms: f64,
    /// The clock knows only its weights still coming: its compute, not begun, comes after them.
    pub weights_only: bool,
}

/// `[org/]package/function[@release]` as `References` keys it: `package/function`.
pub fn reference_key(function: &str) -> String {
    let path = function.split('@').next().unwrap_or_default();
    let mut parts = path.rsplitn(3, '/');
    match (parts.next(), parts.next()) {
        (Some(callable), Some(package)) => format!("{package}/{callable}"),
        _ => path.to_owned(),
    }
}

impl References {
    /// A release's rows of each of its functions (`package` as `[org/]name`), beside those held.
    pub fn add(&mut self, package: &str, rows: &[Benchmark]) {
        let name = package.rsplit('/').next().unwrap_or(package);
        for row in rows {
            let held = self.rows.entry(format!("{name}/{}", row.function)).or_default();
            if !held.contains(row) {
                held.push(row.clone());
            }
        }
    }

    /// A package's rows (`package` as `[org/]name`) for comparing cards only (`fleet`).
    pub fn add_fleet(&mut self, package: &str, rows: &[Benchmark]) {
        let name = package.rsplit('/').next().unwrap_or(package);
        for row in rows {
            self.fleet.entry(format!("{name}/{}", row.function)).or_default().push(row.clone());
        }
    }

    /// `function`'s per-card speeds as its Hub sells cards for it.
    pub fn add_speeds(&mut self, function: &str, speeds: BTreeMap<String, f64>) {
        self.speeds.entry(reference_key(function)).or_default().extend(speeds);
    }

    fn of(&self, function: &str) -> &[Benchmark] {
        self.rows.get(&reference_key(function)).map_or(&[], Vec::as_slice)
    }

    /// `function`'s time on one GPU of `gpu` at `size`, by its row of that card most like `size`, scaled to it; and the row. None:
    /// no row of that card.
    pub fn measured(&self, function: &str, gpu: &str, size: &JobSize) -> Option<(f64, &Benchmark)> {
        nearest(self.of(function).iter().filter(|row| row.gpus == 1 && is_card(gpu, &row.gpu)), size).map(|(row, ms)| (ms, row))
    }

    /// `function`'s time on one GPU of `gpu` at `size`: `measured` (`Basis::Reference`), else `scaled` (`Basis::Scaled`).
    pub fn time(&self, function: &str, gpu: &str, size: &JobSize) -> Option<(f64, Basis)> {
        match self.measured(function, gpu, size) {
            Some((ms, _)) => Some((ms, Basis::Reference)),
            None => self.scaled(function, gpu, size).map(|ms| (ms, Basis::Scaled)),
        }
    }

    /// `function`'s time on one GPU of `gpu` at `size` where no row measured that card (Paul 11-15: "a rough guess"): its rows of
    /// other cards, each scaled by `ratio`, their median; with no card related so, the row of the card nearest this in the Hub's
    /// spec speeds, scaled by them. A guess: never what a rental is recommended by. None: no row, or nothing relates this card to one.
    pub fn scaled(&self, function: &str, gpu: &str, size: &JobSize) -> Option<f64> {
        if gpu.is_empty() {
            return None;
        }
        let mut cards: Vec<&str> = self.of(function).iter().filter(|row| row.gpus == 1).map(|row| row.gpu.as_str()).collect();
        cards.sort_unstable();
        cards.dedup();
        let at = |card: &str| self.measured(function, card, size).map(|(ms, _)| ms);
        let related: Vec<f64> = cards.iter().filter_map(|card| Some(at(card)? * self.ratio(card, gpu)?.0)).collect();
        if !related.is_empty() {
            return Some(median(&related));
        }
        let speeds = self.speeds.get(&reference_key(function))?;
        let speed = |card: &str| speeds.iter().find(|(sold, _)| is_card(card, sold)).map(|(_, speed)| *speed).filter(|speed| *speed > 0.0);
        let to = speed(gpu)?;
        (cards.into_iter())
            .filter_map(|card| {
                let from = speed(card)?;
                Some(((from / to).ln().abs(), at(card)? * from / to))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, ms)| ms)
    }

    /// How much longer a run takes on `to` than on `from` (Paul 11-15: "if a 4090 is usually 20% slower than a 5090, then we go with
    /// that"): the median, over the functions with one-GPU rows on both (`rows`, else `fleet`), of `to`'s time over `from`'s at `from`'s
    /// request; and how many functions that is. None: they share none.
    pub fn ratio(&self, from: &str, to: &str) -> Option<(f64, usize)> {
        let mut functions: HashMap<&String, &Vec<Benchmark>> = self.fleet.iter().collect();
        functions.extend(&self.rows);
        let ratios: Vec<f64> = (functions.into_values())
            .filter_map(|rows| {
                let row = rows.iter().find(|row| row.gpus == 1 && row.warm_ms > 0 && is_card(from, &row.gpu))?;
                let at = nearest(rows.iter().filter(|other| other.gpus == 1 && is_card(to, &other.gpu)), &row.request)?.1;
                Some(at / row.warm_ms as f64)
            })
            .collect();
        (!ratios.is_empty()).then(|| (median(&ratios), ratios.len()))
    }

    /// How `function` goes faster with GPUs on `gpu`, by its rows of that card at one GPU and the widest more: that width and its
    /// speed-up, at the size the two share. None: no such pair.
    fn speedup(&self, function: &str, gpu: &str) -> Option<(u32, f64)> {
        let rows: Vec<&Benchmark> = self.of(function).iter().filter(|row| is_card(gpu, &row.gpu)).collect();
        (rows.iter().filter(|wide| wide.gpus > 1))
            .filter_map(|wide| {
                let one = rows.iter().find(|one| one.gpus == 1 && one.request == wide.request)?;
                Some((wide.gpus, one.warm_ms as f64 / wide.warm_ms.max(1) as f64))
            })
            .max_by_key(|(width, _)| *width)
    }
}

/// A card's name as words: lower case, the vendor's words and punctuation aside (`NVIDIA GeForce RTX 4090`, `rtx-4090`: `rtx 4090`).
fn words(card: &str) -> Vec<String> {
    let lower = card.to_ascii_lowercase();
    let spaced: String = lower.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
    spaced.split_whitespace().filter(|word| !matches!(*word, "nvidia" | "geforce")).map(str::to_owned).collect()
}

/// Whether `gpu`, as its driver or a Hub names it, is the card `card` names: its words begin with the card's, and what follows them
/// names no other model of it (`RTX 4070 Ti` is no `RTX 4070`; `H100 80GB HBM3 SXM` is an `H100 80GB HBM3`).
pub fn is_card(gpu: &str, card: &str) -> bool {
    let (gpu, card) = (words(gpu), words(card));
    !card.is_empty()
        && gpu.starts_with(&card)
        && !gpu.get(card.len()).is_some_and(|next| matches!(next.as_str(), "ti" | "super" | "d" | "laptop" | "mobile" | "max"))
}

/// Of `rows`, the one most like `size`: the nearest pictures read, then the nearest size; and its warm time scaled to `size`.
fn nearest<'r>(rows: impl Iterator<Item = &'r Benchmark>, size: &JobSize) -> Option<(&'r Benchmark, f64)> {
    let refs = size.refs.unwrap_or(0);
    let unlike = |row: &Benchmark| (row.request.refs.unwrap_or(0).abs_diff(refs), size.ratio(&row.request).ln().abs());
    let row = rows.min_by(|a, b| {
        let (a, b) = (unlike(a), unlike(b));
        a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
    })?;
    Some((row, row.warm_ms as f64 * size.ratio(&row.request)))
}

/// A function's time on a machine: its compute on one GPU, how it scales, its load onto the GPUs, and what they stand on.
#[derive(Clone, Debug)]
pub struct Time {
    pub one_ms: f64,
    pub scaling: Scaling,
    pub load_ms: f64,
    pub basis: Basis,
    /// Widths above one timed faster there than one GPU at this size.
    pub faster: Vec<u32>,
}

impl Time {
    /// Its whole time at `width`.
    pub fn at(&self, width: u32) -> f64 {
        self.one_ms * self.scaling.at(width)
    }
}

/// From its newest runs' compute on the machine (`runs`, newest first) at `size`, else the caller's `prior_ms`; and the median load
/// of those that loaded. Until two widths ran there it scales as `known` says (a `KNOWN` speed-up), else as `Scaling::PRIOR`. `warm`:
/// the machine holds its model with its plan compiled, so its warm runs time it; else its cold one (its first call there, its plan
/// compiling). With only one kind, that one: a cold time alone is the conservative quote.
pub fn time(runs: &[TimedRun], size: &JobSize, prior_ms: f64, known: Option<Scaling>, warm: bool) -> Time {
    let (cold, hot): (Vec<TimedRun>, Vec<TimedRun>) = runs.iter().cloned().partition(|run| run.cold);
    let runs: &[TimedRun] = match (warm, hot.is_empty(), cold.is_empty()) {
        (true, false, _) => &hot,
        (false, _, false) => &cold,
        _ => runs,
    };
    let prior = known.unwrap_or(Scaling::PRIOR);
    if runs.is_empty() {
        return match prior_ms > 0.0 {
            true => Time { one_ms: prior_ms, scaling: prior, load_ms: 0.0, basis: Basis::Prior, faster: vec![] },
            false => Time { one_ms: UNKNOWN_MS, scaling: prior, load_ms: 0.0, basis: Basis::None, faster: vec![] },
        };
    }
    let same: Vec<&TimedRun> = runs.iter().filter(|run| (size.ratio(&run.size) - 1.0).abs() <= SAME_SIZE).collect();
    let like: Vec<(u32, f64)> = match same.is_empty() {
        false => same.iter().map(|run| (run.width.max(1), run.ms)).collect(),
        true => runs.iter().map(|run| (run.width.max(1), run.ms * size.ratio(&run.size))).collect(),
    };
    let mut by_width: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    for (width, ms) in like {
        let each = by_width.entry(width).or_default();
        if each.len() < RECENT {
            each.push(ms);
        }
    }
    let medians: Vec<(u32, f64)> = by_width.iter().map(|(width, ms)| (*width, median(ms))).collect();
    let scaling = match (medians.first(), medians.last()) {
        (Some(a), Some(b)) if a.0 != b.0 => Scaling::of(*a, *b),
        _ => prior,
    };
    let one = medians.iter().find(|(width, _)| *width == 1).map(|(_, ms)| *ms);
    let faster = medians.iter().filter(|(width, ms)| *width > 1 && one.is_some_and(|one| *ms < one)).map(|(width, _)| *width).collect();
    let (width, ms) = by_width.iter().max_by_key(|(width, ms)| (ms.len(), std::cmp::Reverse(**width))).map(|(w, ms)| (*w, median(ms))).expect("a run");
    let loads: Vec<f64> = runs.iter().map(|run| run.load_ms).filter(|ms| *ms > 0.0).take(RECENT).collect();
    let load_ms = if loads.is_empty() { 0.0 } else { median(&loads) };
    Time { one_ms: ms / scaling.at(width), scaling, load_ms, basis: Basis::Measured, faster }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    match n % 2 {
        1 => sorted[n / 2],
        _ => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// What a machine's jobs are timed and packed by: its GPUs, link, card and runs, and the benchmarks of what it never ran.
pub struct Machine<'r> {
    /// The most GPUs side by side in service.
    pub gpus: usize,
    /// Bytes a millisecond its weights arrive at.
    pub link: f64,
    pub card: String,
    pub nvlink: bool,
    /// Each function's newest runs there.
    pub times: HashMap<String, Vec<TimedRun>>,
    /// Its clock now, Unix ms.
    pub now_ms: i64,
    /// What a function none of whose runs it knows is timed by (cl-1024); none: a minute.
    pub references: Option<&'r References>,
}

/// The download line: when it ends, ms from now, and when each repository in it lands.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub end_ms: f64,
    pub lands: HashMap<String, f64>,
}

impl Line {
    /// `bytes` of `models` fetched after what is in the line, from `from` at the soonest; and those already in it waited for:
    /// when the last of them lands.
    fn fetch(&mut self, models: &[String], bytes: u64, link: f64, from: f64) -> f64 {
        let mut end = models.iter().filter_map(|model| self.lands.get(model).copied()).fold(0.0, f64::max);
        if bytes > 0 {
            self.end_ms = self.end_ms.max(from) + bytes as f64 / link;
            end = end.max(self.end_ms);
        }
        for model in models.iter().filter(|model| !model.is_empty()) {
            let at = self.lands.entry(model.clone()).or_insert(end);
            *at = at.max(end);
        }
        end
    }
}

impl<'r> Machine<'r> {
    /// As `frame` pictures it, at `now_ms` on its clock, what it never ran timed by `references`.
    pub fn of(frame: &StatusFrame, now_ms: i64, references: &'r References) -> Self {
        let times = frame.times.iter().flatten().map(|timed| (timed.function.clone(), timed.runs.clone())).collect();
        Machine {
            gpus: frame.serving_gpus as usize,
            link: match frame.link_bytes_per_s > 0.0 {
                true => frame.link_bytes_per_s / 1e3,
                false => LINK,
            },
            card: frame.gpus.first().map(|gpu| gpu.name.clone()).unwrap_or_default(),
            nvlink: frame.nvlink,
            times,
            now_ms,
            references: Some(references),
        }
    }

    /// `function` timed here (else by the caller's prior, else by its release's benchmarks for this card, `References::time`),
    /// scaling as its benchmarks at more GPUs on this card say, else as `KNOWN` says for this card and link, until two widths ran here;
    /// and of `widths`, those it may run at: one GPU, and a wider one only once timed faster than one GPU here or known faster here
    /// (cr-268: never wider than proven). A count asked for (one width) passes as it is.
    pub fn timed(&self, function: &str, size: &JobSize, prior_ms: f64, widths: Vec<u32>, warm: bool) -> (Time, Vec<u32>) {
        let mut known = known(function, &self.card, self.nvlink);
        known.extend(self.references.and_then(|references| references.speedup(function, &self.card)));
        let scaling = known.iter().max_by_key(|(width, _)| *width).map(|(width, speedup)| Scaling::of((1, *speedup), (*width, 1.0)));
        let runs = self.times.get(function).map_or(&[][..], Vec::as_slice);
        let mut time = time(runs, size, prior_ms, scaling, warm);
        // Nothing of it timed here and no prior: its release's benchmark of this card (cl-1024), else one scaled from another card.
        if time.basis == Basis::None
            && let Some(references) = self.references
        {
            if let Some((ms, _)) = references.measured(function, &self.card, size) {
                time = Time { one_ms: ms, basis: Basis::Reference, ..time };
            } else if let Some(ms) = references.scaled(function, &self.card, size) {
                time = Time { one_ms: ms, basis: Basis::Scaled, ..time };
            }
        }
        let widths = match widths.len() {
            0 | 1 => widths,
            _ => widths.into_iter().filter(|width| *width == 1 || time.faster.contains(width) || known.iter().any(|(w, _)| w == width)).collect(),
        };
        (time, widths)
    }

    /// `queue` as `pack` sees it, in its order, and the download line it leaves: each waiting root's weights after those of the
    /// roots ahead of it.
    pub fn queue(&self, queue: &[Queued]) -> (Vec<Job>, Line) {
        let mut line = Line::default();
        // A root after another of its function here runs warm.
        let mut seen: HashSet<&str> = HashSet::new();
        let jobs = (queue.iter())
            .map(|root| {
                let warm = !root.function.is_empty() && !seen.insert(&root.function);
                let (time, widths) = self.timed(&root.function, &root.size, 0.0, root.widths.clone(), warm);
                let mut job = Job { widths, one_ms: time.one_ms, scaling: time.scaling, ready_ms: 0.0, running: None };
                match (root.running, root.width) {
                    (false, _) => job.ready_ms = line.fetch(&root.models, root.bytes_left, self.link, 0.0),
                    (true, width) => {
                        let whole = job.ms(width);
                        let ran = match root.started_at_unix_ms {
                            0 => 0.0,
                            started => (self.now_ms - started).max(0) as f64,
                        };
                        // Counted from its executor's start, a root still fetching its weights is all but done (cl-1073): its caller's
                        // clock says more.
                        let clock = self.references.filter(|_| !root.run.is_empty()).and_then(|references| references.left.get(&root.run));
                        let left = match clock {
                            Some(left) if left.weights_only => left.ms + whole,
                            Some(left) if left.ms > 0.0 => left.ms,
                            _ => (whole - ran).max(whole * OVERRUN),
                        };
                        job.running = Some((width, left));
                    }
                }
                job
            })
            .collect();
        (jobs, line)
    }
}

fn invalid(function: &str) -> v1::Reason {
    let message = format!("{function:?} is not org/package/function[@release] or sha256:<hex>/function");
    v1::Reason { code: "invalid_function".into(), message, origin: "request".into(), ..Default::default() }
}

/// `function` as `EstimateJob::function` names it: its code (`sha256:<hex>`, or a release's package), release (empty: any) and
/// callable.
fn named(function: &str) -> Option<(&str, &str, &str)> {
    if function.starts_with("sha256:") {
        let (code, entrypoint) = function.split_once('/').filter(|(code, e)| code.len() == 71 && !e.is_empty())?;
        return Some((code, "", entrypoint));
    }
    let (path, release) = function.split_once('@').unwrap_or((function, ""));
    match path.rsplit_once('/') {
        Some((package, entrypoint)) if package.contains('/') && !entrypoint.is_empty() => Some((package, release, entrypoint)),
        _ => None,
    }
}

/// The installation of `frame` holding code as a job names it: local code by its manifest's digest, a release by its package and
/// its release when one is named, else any release of it, whose widths and times are about its own.
fn installed<'f>(frame: &'f StatusFrame, code: &str, release: &str) -> (Option<&'f v1::Environment>, bool) {
    let find = |held: &dyn Fn(&v1::Environment) -> bool| frame.environments.iter().rev().find(|e| held(e));
    if code.starts_with("sha256:") {
        let found = find(&|e| e.manifest == code);
        return (found, found.is_some());
    }
    match find(&|e| e.package == code && (release.is_empty() || e.release == release)) {
        Some(found) => (Some(found), true),
        None => (find(&|e| e.package == code), false),
    }
}

/// The checkpoint of `model` that `frame`'s store holds: the one the job names, else the one of its repository this machine pulled,
/// the most of it held: the lane it chose for its card, which a run of it there reads again (cl-967: a job named one lane of its model,
/// the machine held another, and every quote there counted the named one's whole download).
fn held<'f>(frame: &'f StatusFrame, model: &v1::ModelRef) -> Option<&'f v1::Checkpoint> {
    let all = || frame.models.iter().flat_map(|m| m.checkpoints.iter().map(move |c| (m, c)));
    let named = all().find(|(_, c)| c.tree == model.checkpoint || c.manifest == model.checkpoint);
    let pulled = || all().filter(|(m, c)| !model.model.is_empty() && m.model == model.model && c.role != "lora").max_by_key(|(_, c)| c.held_bytes);
    named.or_else(pulled).map(|(_, c)| c)
}

/// The bytes of `model` `frame`'s store lacks: what its held checkpoint lacks of its closure, no more than the call fetches of it; else all
/// the call fetches of it (`ModelRef::bytes`).
fn absent(frame: &StatusFrame, model: &v1::ModelRef) -> u64 {
    match held(frame, model) {
        // What the checkpoint lacks here, no more than the call fetches of it (cl-1071: its components, its objects no other slot reads).
        Some(checkpoint) if checkpoint.bytes > 0 => checkpoint.bytes.saturating_sub(checkpoint.held_bytes).min(model.bytes),
        _ => model.bytes,
    }
}

/// The bytes of `model` a job of `function` fetches on `frame`'s machine (cl-967): none while an idle executor holds it loaded; else,
/// of what its function's newest run here read of it, those not here now; else what its held checkpoint lacks, no more than a run of
/// the function read of it elsewhere (`ModelRef::reads`). A lane's closure holds components a function never reads: counted whole,
/// a machine that ran it was quoted their download every time.
fn fetches(frame: &StatusFrame, function: &str, model: &v1::ModelRef) -> u64 {
    let header = header(frame, model);
    if frame.resident.iter().any(|resident| resident.checkpoints.contains(&header)) {
        return 0;
    }
    let timed = frame.times.iter().flatten().find(|timed| !function.is_empty() && timed.function == function);
    let here = timed.and_then(|timed| timed.reads.iter().find(|read| read.checkpoint == header || (!model.model.is_empty() && read.model == model.model)));
    match (here, model.reads) {
        (Some(read), _) => read.missing,
        (None, 0) => absent(frame, model),
        (None, reads) => reads.min(absent(frame, model)),
    }
}

/// Each of `job`'s models' `reads` not known yet (cl-967): the most a run of its function read of it on any of `frames`' machines.
/// Local code's function is known by its package only where it is installed: its machine's own reads count it.
pub fn learn_reads(frames: &[&StatusFrame], job: &mut EstimateJob) {
    let Some((code, _, entrypoint)) = named(&job.function).filter(|(code, ..)| !code.starts_with("sha256:")) else { return };
    let function = format!("{code}/{entrypoint}");
    for model in job.models.iter_mut().filter(|model| model.reads == 0 && !model.model.is_empty()) {
        model.reads = (frames.iter().flat_map(|frame| frame.times.iter().flatten()))
            .filter(|timed| timed.function == function)
            .flat_map(|timed| &timed.reads)
            .filter(|read| read.model == model.model)
            .map(|read| read.bytes)
            .max()
            .unwrap_or(0);
    }
}

/// `model`'s header as an executor that loaded it names it.
fn header(frame: &StatusFrame, model: &v1::ModelRef) -> String {
    held(frame, model).map_or(model.checkpoint.clone(), |c| c.manifest.clone())
}

/// Whether an idle executor of `frame` holds any of `models` loaded: their function's plan compiled there, its model warm.
fn warm_there(frame: &StatusFrame, models: &[v1::ModelRef]) -> bool {
    let loaded: HashSet<&String> = frame.resident.iter().flat_map(|r| &r.checkpoints).collect();
    models.iter().any(|model| loaded.contains(&header(frame, model)))
}

/// Whether an idle executor of `frame` holds every one of `models` loaded: a job of them loads nothing.
fn resident(frame: &StatusFrame, models: &[v1::ModelRef]) -> bool {
    let loaded: HashSet<&String> = frame.resident.iter().flat_map(|r| &r.checkpoints).collect();
    !models.is_empty() && models.iter().all(|model| loaded.contains(&header(frame, model)))
}

fn refused(code: &str, message: String) -> Option<v1::Reason> {
    Some(v1::Reason { code: code.into(), message, origin: "machine".into(), ..Default::default() })
}

/// When each of `jobs` would run on the machine `frame` pictures, were they all sent there now (`now_ms` on its clock), after its
/// queue: each one's install after those ahead of it, its absent weights' download after those ahead of it, then their load,
/// unless an idle executor holds them or a run of the same function goes before it there; packed on its GPUs as its scheduler packs
/// them.
pub fn quote(frame: &StatusFrame, now_ms: i64, jobs: &[EstimateJob], references: &References) -> Vec<Quote> {
    // An update going here waits for every root it holds to end, then installs, and nothing asked is sent before it has (cl-1073:
    // chartreux, so, held a run 20 minutes it was quoted to finish in 209 s): what is asked starts after both, on the machine as it
    // then is. Its preparation is its own: it waits that long for the machine first.
    if frame.update.as_ref().is_some_and(|update| !matches!(update.state.as_str(), "succeeded" | "failed" | "canceled")) {
        let machine = Machine::of(frame, now_ms, references);
        let (held, _) = machine.queue(frame.queue.as_deref().unwrap_or_default());
        let drained = (pack(machine.gpus, &held).iter().map(|place| place.finish_ms).fold(0.0, f64::max) + INSTALL_MS) as u64;
        let after = StatusFrame { queue: None, update: None, ..frame.clone() };
        let asks: Vec<EstimateJob> = jobs.iter().map(|job| EstimateJob { after_ms: job.after_ms.saturating_sub(drained), ..job.clone() }).collect();
        return (quote(&after, now_ms, &asks, references).into_iter())
            .map(|mut quote| {
                if quote.fits {
                    (quote.start_ms, quote.finish_ms) = (quote.start_ms + drained, quote.finish_ms + drained);
                }
                quote
            })
            .collect();
    }
    let machine = Machine::of(frame, now_ms, references);
    let queue = frame.queue.as_deref().unwrap_or_default();
    let (mut packed, mut line) = machine.queue(queue);
    let mut warm: HashSet<String> = queue.iter().map(|root| root.function.clone()).filter(|function| !function.is_empty()).collect();
    let free = frame.disk.as_ref().map_or(u64::MAX, |disk| disk.free_bytes);
    let lost = frame.gpus.iter().any(|gpu| !gpu.out_of_service.is_empty());
    // Each code's install, once, after those ahead of it.
    let (mut installs, mut installing) = (0.0, HashMap::<&str, f64>::new());
    let mut asked: Vec<(Option<usize>, Quote)> = vec![];
    for job in jobs {
        let mut quote = Quote { key: job.key.clone(), ..Default::default() };
        let Some((code, release, entrypoint)) = named(&job.function) else {
            quote.refusal = Some(invalid(&job.function));
            asked.push((None, quote));
            continue;
        };
        let (installation, held) = installed(frame, code, release);
        let held_function = installation.and_then(|i| i.functions.iter().find(|f| f.entrypoint == entrypoint));
        let function = match held_function {
            Some(held) => held.timed_as.clone(),
            None if code.starts_with("sha256:") => String::new(),
            None => format!("{code}/{entrypoint}"),
        };
        let widths = match (job.gpus, held_function) {
            (0, Some(held)) => held.widths.iter().map(|width| (*width).min(machine.gpus as u32)).filter(|width| *width > 0).collect(),
            _ => unheld_widths(machine.gpus, job.gpus),
        };
        // Warm: a job of its function goes before it here, or an idle executor holds its model; else its first call's cold time.
        let hot = !function.is_empty() && (warm.contains(&function) || warm_there(frame, &job.models));
        let (time, widths) = machine.timed(&function, &job.size, job.prior_ms as f64, widths, hot);
        // This machine's own runs, which its frame doesn't time (a project's, cl-1047), measure it all the same.
        quote.basis = match (time.basis, job.prior_runs) {
            (Basis::Prior, 1..) => Basis::Measured,
            (basis, _) => basis,
        };
        quote.warm = hot && machine.times.get(&function).is_some_and(|runs| runs.iter().any(|run| !run.cold));
        // A machine with no GPU runs it on its CPU, as its run then says (clamp and warn): its time is its runs' there. GPUs out of
        // service (cl-935) leave too few for it: another machine runs it.
        let wanted = (job.gpus as usize).max(usize::from(job.gpu));
        if lost && wanted > machine.gpus {
            quote.refusal = refused("gpu_lost", format!("of this machine's GPUs, {} side by side are in service, and it needs {wanted}", machine.gpus));
            asked.push((None, quote));
            continue;
        }
        let fetch: u64 = job.models.iter().map(|model| fetches(frame, &function, model)).sum();
        if fetch > free {
            quote.refusal = refused("does_not_fit", format!("its weights' {fetch} bytes don't fit in this machine's {free} free"));
            asked.push((None, quote));
            continue;
        }
        quote.fits = true;
        // Its install after those ahead of it; its download after its install, as the machine resolves its weights once it is
        // built, and after those ahead of it; what a run ahead brings is not fetched twice.
        let install_end = match held {
            true => 0.0,
            false => *installing.entry(code).or_insert_with(|| {
                installs += INSTALL_MS;
                installs
            }),
        };
        let new: u64 = job.models.iter().filter(|model| !line.lands.contains_key(&model.model)).map(|model| fetches(frame, &function, model)).sum();
        let models: Vec<String> = job.models.iter().map(|model| model.model.clone()).collect();
        quote.fetch_bytes = new;
        let fetched = line.fetch(&models, new, machine.link, install_end);
        let load = match (!function.is_empty() && warm.contains(&function)) || resident(frame, &job.models) || job.models.is_empty() {
            true => 0.0,
            false => time.load_ms,
        };
        warm.insert(function);
        let ready = install_end.max(fetched).max(job.after_ms as f64) + load;
        quote.prepare_ms = ready as u64;
        packed.push(Job { widths, one_ms: time.one_ms, scaling: time.scaling, ready_ms: ready, running: None });
        asked.push((Some(packed.len() - 1), quote));
    }
    let placed = pack(machine.gpus, &packed);
    (asked.into_iter())
        .map(|(at, mut quote)| {
            if let Some(place) = at.map(|at| placed[at]) {
                (quote.width, quote.start_ms, quote.finish_ms) = (place.width, place.start_ms as u64, place.finish_ms as u64);
                quote.queue_position = place.position as u32;
            }
            quote
        })
        .collect()
}
