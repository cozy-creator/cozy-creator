//! How a machine lays out its GPU work (cl-816, Paul 11-07), and when a job sent there would run (cl-967, Paul 11-12: "calcifer
//! could compute the quote on its end, if it has sufficient knowledge of each machine"): the one model its scheduler gives each root
//! its width by, and Calcifer quotes every machine by from the state it pushes (`quote`), one on its way included (cl-915). Roots
//! start in the order they are ready, acceptance breaking ties, on the GPUs free soonest; an older root waiting for GPUs keeps them
//! from younger ones. As each starts it takes its width in the plan that ends all that waits soonest, of those capping every job at
//! one width; within 2% the wider plan wins, its first results coming sooner. So a lone job runs wide and several queued jobs one
//! GPU each. A machine's downloads and installs each go one at a time in job order (Paul 11-12), so a job's weights come after
//! those of the jobs ahead of it.
use std::collections::{BTreeMap, HashMap, HashSet};

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
/// of those that loaded. Until two widths ran there it scales as `known` says (a `KNOWN` speed-up), else as `Scaling::PRIOR`.
pub fn time(runs: &[TimedRun], size: &JobSize, prior_ms: f64, known: Option<Scaling>) -> Time {
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

/// What a machine's jobs are timed and packed by: its GPUs, link, card and runs.
pub struct Machine {
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

impl Machine {
    /// As `frame` pictures it, at `now_ms` on its clock.
    pub fn of(frame: &StatusFrame, now_ms: i64) -> Self {
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
        }
    }

    /// `function` timed here, scaling as `KNOWN` says for this card and link until two widths ran here; and of `widths`, those it
    /// may run at: one GPU, and a wider one only once timed faster than one GPU here or known faster here (cr-268: never wider than
    /// proven). A count asked for (one width) passes as it is.
    pub fn timed(&self, function: &str, size: &JobSize, prior_ms: f64, widths: Vec<u32>) -> (Time, Vec<u32>) {
        let known = known(function, &self.card, self.nvlink);
        let scaling = known.iter().max_by_key(|(width, _)| *width).map(|(width, speedup)| Scaling::of((1, *speedup), (*width, 1.0)));
        let runs = self.times.get(function).map_or(&[][..], Vec::as_slice);
        let time = time(runs, size, prior_ms, scaling);
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
        let jobs = (queue.iter())
            .map(|root| {
                let (time, widths) = self.timed(&root.function, &root.size, 0.0, root.widths.clone());
                let mut job = Job { widths, one_ms: time.one_ms, scaling: time.scaling, ready_ms: 0.0, running: None };
                match (root.running, root.width) {
                    (false, _) => job.ready_ms = line.fetch(&root.models, root.bytes_left, self.link, 0.0),
                    (true, width) => {
                        let whole = job.ms(width);
                        let ran = match root.started_at_unix_ms {
                            0 => 0.0,
                            started => (self.now_ms - started).max(0) as f64,
                        };
                        job.running = Some((width, (whole - ran).max(whole * OVERRUN)));
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

/// The bytes of `model` `frame`'s store lacks: what its held checkpoint lacks of its closure, else all the catalog lists.
fn absent(frame: &StatusFrame, model: &v1::ModelRef) -> u64 {
    let held = frame.models.iter().flat_map(|m| &m.checkpoints).find(|c| c.tree == model.checkpoint || c.manifest == model.checkpoint);
    match held {
        Some(checkpoint) if checkpoint.bytes > 0 => checkpoint.bytes.saturating_sub(checkpoint.held_bytes),
        _ => model.bytes,
    }
}

/// Whether an idle executor of `frame` holds every one of `models` loaded: a job of them loads nothing.
fn resident(frame: &StatusFrame, models: &[v1::ModelRef]) -> bool {
    let header = |model: &v1::ModelRef| {
        let held = frame.models.iter().flat_map(|m| &m.checkpoints).find(|c| c.tree == model.checkpoint || c.manifest == model.checkpoint);
        held.map_or(model.checkpoint.clone(), |c| c.manifest.clone())
    };
    let loaded: HashSet<&String> = frame.resident.iter().flat_map(|r| &r.checkpoints).collect();
    !models.is_empty() && models.iter().all(|model| loaded.contains(&header(model)))
}

fn refused(code: &str, message: String) -> Option<v1::Reason> {
    Some(v1::Reason { code: code.into(), message, origin: "machine".into(), ..Default::default() })
}

/// When each of `jobs` would run on the machine `frame` pictures, were they all sent there now (`now_ms` on its clock), after its
/// queue: each one's install after those ahead of it, its absent weights' download after those ahead of it, then their load,
/// unless an idle executor holds them or a run of the same function goes before it there; packed on its GPUs as its scheduler packs
/// them.
pub fn quote(frame: &StatusFrame, now_ms: i64, jobs: &[EstimateJob]) -> Vec<Quote> {
    let machine = Machine::of(frame, now_ms);
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
        let (time, widths) = machine.timed(&function, &job.size, job.prior_ms as f64, widths);
        quote.basis = time.basis;
        // A machine with no GPU runs it on its CPU, as its run then says (clamp and warn): its time is its runs' there. GPUs out of
        // service (cl-935) leave too few for it: another machine runs it.
        let wanted = (job.gpus as usize).max(usize::from(job.gpu));
        if lost && wanted > machine.gpus {
            quote.refusal = refused("gpu_lost", format!("of this machine's GPUs, {} side by side are in service, and it needs {wanted}", machine.gpus));
            asked.push((None, quote));
            continue;
        }
        let fetch: u64 = job.models.iter().map(|model| absent(frame, model)).sum();
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
        let new: u64 = job.models.iter().filter(|model| !line.lands.contains_key(&model.model)).map(|model| absent(frame, model)).sum();
        let models: Vec<String> = job.models.iter().map(|model| model.model.clone()).collect();
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
