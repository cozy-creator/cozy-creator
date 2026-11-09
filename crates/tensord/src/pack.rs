//! How this machine lays out its GPU work (cl-816, Paul 11-07): the one model its scheduler gives
//! each root its width by and its estimate quotes from. Roots start in the order they are ready,
//! acceptance breaking ties, on the GPUs free soonest; an older root waiting for GPUs keeps them
//! from younger ones. As each starts it takes its width in the plan that ends all that waits
//! soonest, of those capping every job at one width; within 2% the wider plan wins, its first
//! results coming sooner. So a lone job runs wide and several queued jobs one GPU each.

/// One root's work.
#[derive(Clone, Debug)]
pub(crate) struct Job {
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
    pub(crate) fn ms(&self, width: u32) -> f64 {
        self.one_ms * self.scaling.at(width)
    }
}

/// How a job goes faster with GPUs: the share of it that doesn't.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scaling {
    pub serial: f64,
}

impl Scaling {
    /// Until two widths have run: 2 GPUs near 1.8x, 4 near 3x, about what fast-linked cards do.
    pub(crate) const PRIOR: Scaling = Scaling { serial: 0.1 };

    /// From its times at two widths.
    pub(crate) fn of((a, at_a): (u32, f64), (b, at_b): (u32, f64)) -> Self {
        let (a, b, r) = (f64::from(a), f64::from(b), at_a / at_b);
        let under = (1.0 - 1.0 / a) - r * (1.0 - 1.0 / b);
        match under.abs() > f64::EPSILON && a != b {
            true => Scaling { serial: ((r / b - 1.0 / a) / under).clamp(0.0, 1.0) },
            false => Self::PRIOR,
        }
    }

    pub(crate) fn at(self, width: u32) -> f64 {
        self.serial + (1.0 - self.serial) / f64::from(width.max(1))
    }
}

/// Where a job goes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Placed {
    pub width: u32,
    pub start_ms: f64,
    pub finish_ms: f64,
    /// Waiting jobs that start before it.
    pub position: usize,
}

/// Within this share of the soonest end, the wider plan wins.
const TIE: f64 = 0.02;

/// `jobs` on `gpus` GPUs, in their order (running ones first, then by acceptance).
pub(crate) fn pack(gpus: usize, jobs: &[Job]) -> Vec<Placed> {
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
