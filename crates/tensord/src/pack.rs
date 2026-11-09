//! How this machine lays out its GPU work (cl-816, Paul 11-07): the one model its scheduler gives
//! each root its width by and its estimate quotes from. Roots start in the order they are ready,
//! acceptance breaking ties, on the GPUs free soonest; an older root waiting for GPUs keeps them
//! from younger ones. As each starts it takes its width in the plan that ends all that waits
//! soonest, of those capping every job at one width; within 2% the narrower plan wins. A width this
//! machine has not measured is taken to gain nothing over the nearest it has (release owner,
//! 11-09), so a job runs on one GPU until a run at a wider width measured that one faster.
use std::collections::BTreeMap;

/// One root's work.
#[derive(Clone, Debug)]
pub struct Job {
    /// The widths it runs at, smallest first; none: it holds no GPU.
    pub widths: Vec<u32>,
    pub times: Times,
    /// The soonest it can start, ms from now: when its preparation ends.
    pub ready_ms: f64,
    /// It runs: its width and the ms it has left.
    pub running: Option<(u32, f64)>,
}

impl Job {
    pub fn ms(&self, width: u32) -> f64 {
        self.times.at(width)
    }
}

/// A job's time at each width: what this machine measured at it, else at the nearest width it
/// measured, a narrower one first; with none measured, `ms` at every width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Times {
    pub measured: BTreeMap<u32, f64>,
    pub ms: f64,
}

impl Times {
    pub fn at(&self, width: u32) -> f64 {
        let width = width.max(1);
        let narrower = self.measured.range(..=width).next_back();
        narrower.or_else(|| self.measured.range(width..).next()).map_or(self.ms, |(_, ms)| *ms)
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

/// Within this share of the soonest end, the narrower plan wins.
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

/// The cap of the plan for `rest` from `free` that ends soonest, the narrowest within `TIE`.
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
    ends.iter().filter(|(_, end)| *end <= soonest * (1.0 + TIE)).map(|(cap, _)| *cap).min().unwrap_or(1)
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
