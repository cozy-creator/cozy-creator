//! What the walker works out of time (cl-770): a simulation of what is owed, on the machines' GPUs, by how long things took here. It says how many
//! GPUs a job should have and when each should end, one function, so the width chosen and the time shown can't disagree. Wrong, a job
//! takes a little longer, as it may anyway. It knows no model: what it knows of a machine and a width it measured there, or on its card.

/// One job to place.
#[derive(Clone, Debug)]
pub(crate) struct Work {
    /// Those of the list it waits for, each before it.
    pub needs: Vec<usize>,
    /// How long it takes on one GPU of each of its machines, in their order: at a width, by `Scaling`. None: nothing it is like has run on that
    /// machine or its card, so it counts as its slowest known elsewhere, else as what is known of the rest.
    pub one_gpu_s: Vec<Option<f64>>,
    /// The widths it runs at, smallest first: one for all that takes no group. Empty: no GPU.
    pub widths: Vec<u32>,
    pub scaling: Scaling,
    /// The machines it may go to, of the list: of two that end it as soon, the first.
    pub machines: Vec<usize>,
    /// It is running: where, how wide, and how much is left.
    pub running: Option<(usize, u32, f64)>,
}

/// How a function goes faster with GPUs: the share of it that doesn't.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scaling {
    pub serial: f64,
}

impl Scaling {
    /// Until a width has run here: 2 GPUs near 1.8×, 4 near 3×, about what fast-linked cards do.
    pub(crate) const PRIOR: Scaling = Scaling { serial: 0.1 };

    /// What a run at `width` that spent `exchange_share` of its time exchanging between GPUs says: the rest went `width` times faster than one GPU would.
    pub(crate) fn from_exchange(width: u32, exchange_share: f64) -> Self {
        if width <= 1 {
            return Self::PRIOR;
        }
        let d = f64::from(width);
        let one = d * (1.0 - exchange_share.clamp(0.0, 1.0));
        // A group no faster than one GPU: nothing of it scales.
        if one <= 1.0 {
            return Scaling { serial: 1.0 };
        }
        Scaling { serial: ((1.0 / one) - 1.0 / d) / (1.0 - 1.0 / d) }
    }

    pub(crate) fn at(self, width: u32) -> f64 {
        self.serial + (1.0 - self.serial) / f64::from(width.max(1))
    }
}

/// What runs of one function on one machine took, per unit of what they made (a second of a shot, or one of anything else), by width.
#[derive(Clone, Debug, Default)]
pub(crate) struct Learned {
    by_width: std::collections::BTreeMap<u32, f64>,
    /// What the newest run at a width above one spent exchanging between its GPUs, and at which.
    exchange: Option<(u32, f64)>,
}

impl Learned {
    pub(crate) fn took(&mut self, width: u32, per_unit_s: f64, exchange_share: f64) {
        self.by_width.insert(width.max(1), per_unit_s);
        if width > 1 && exchange_share > 0.0 {
            self.exchange = Some((width, exchange_share));
        }
    }

    /// How it scales: two widths measured say it outright; else one run's exchange; else the prior.
    pub(crate) fn scaling(&self) -> Scaling {
        let mut widths = self.by_width.iter();
        if let (Some((&a, &at_a)), Some((&b, &at_b))) = (widths.next(), widths.next_back())
            && a != b
        {
            let (a, b, r) = (f64::from(a), f64::from(b), at_a / at_b);
            let under = (1.0 - 1.0 / a) - r * (1.0 - 1.0 / b);
            if under.abs() > f64::EPSILON {
                return Scaling { serial: ((r / b - 1.0 / a) / under).clamp(0.0, 1.0) };
            }
        }
        self.exchange.map_or(Scaling::PRIOR, |(width, share)| Scaling::from_exchange(width, share))
    }

    /// One unit's time on one GPU, if anything ran.
    pub(crate) fn one_gpu_s(&self) -> Option<f64> {
        let (&width, &took) = self.by_width.iter().next()?;
        Some(took / self.scaling().at(width))
    }
}

/// Where a job goes in the plan chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Placed {
    pub machine: usize,
    pub width: u32,
    pub start_s: f64,
    pub end_s: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub placed: Vec<Placed>,
    pub end_s: f64,
}

/// Within this share, the wider plan wins: its first results come sooner.
const TIE: f64 = 0.02;

/// The plan that ends soonest of those capping every job at one width, as what's left shrinks the widest wins: a lone job gets every GPU.
pub(crate) fn plan(works: &[Work], gpus: &[u32]) -> Plan {
    let mut caps: Vec<u32> = works.iter().flat_map(|work| work.widths.iter().copied()).collect();
    caps.sort_unstable();
    caps.dedup();
    if caps.is_empty() {
        caps.push(1);
    }
    let known: Vec<f64> = works.iter().flat_map(|work| work.one_gpu_s.iter().flatten().copied()).collect();
    let usual = if known.is_empty() { 60.0 } else { known.iter().sum::<f64>() / known.len() as f64 };
    let mut best: Option<Plan> = None;
    for &cap in &caps {
        let tried = placed(works, gpus, cap, usual);
        let better = best.as_ref().is_none_or(|best| tried.end_s <= best.end_s * (1.0 + TIE));
        if better {
            best = Some(tried);
        }
    }
    best.unwrap_or_default()
}

/// Each job in turn, at the widest it runs at under `cap`, on the machine where it ends soonest.
fn placed(works: &[Work], gpus: &[u32], cap: u32, usual: f64) -> Plan {
    // When each GPU of each machine comes free.
    let mut free: Vec<Vec<f64>> = gpus.iter().map(|&count| vec![0.0; count.max(1) as usize]).collect();
    let mut placed = vec![Placed::default(); works.len()];
    // What is running holds its GPUs first.
    for (at, work) in works.iter().enumerate() {
        if let Some((machine, width, left)) = work.running {
            let line = &mut free[machine];
            line.sort_by(f64::total_cmp);
            line.iter_mut().take(width.max(1) as usize).for_each(|gpu| *gpu = gpu.max(left));
            placed[at] = Placed { machine, width, start_s: 0.0, end_s: left };
        }
    }
    let mut end_s: f64 = placed.iter().map(|place| place.end_s).fold(0.0, f64::max);
    for (at, work) in works.iter().enumerate().filter(|(_, work)| work.running.is_none()) {
        let ready = work.needs.iter().map(|&need| placed[need].end_s).fold(0.0, f64::max);
        let slowest = work.one_gpu_s.iter().flatten().copied().reduce(f64::max).unwrap_or(usual);
        let one_on = |k: usize| work.one_gpu_s.get(k).copied().flatten().unwrap_or(slowest);
        // A job with no GPU takes its time, in no one's way.
        if work.widths.is_empty() {
            let one = one_on(0);
            placed[at] = Placed { machine: work.machines.first().copied().unwrap_or(0), width: 0, start_s: ready, end_s: ready + one };
            end_s = end_s.max(ready + one);
            continue;
        }
        let mut chosen: Option<(usize, u32, f64, f64)> = None;
        for (k, &machine) in work.machines.iter().enumerate() {
            let one = one_on(k);
            let line = &mut free[machine];
            line.sort_by(f64::total_cmp);
            let widest = cap.min(line.len() as u32);
            let width = work.widths.iter().copied().filter(|width| *width <= widest).max().unwrap_or(1);
            let start = ready.max(line[(width as usize - 1).min(line.len() - 1)]);
            let end = start + one * work.scaling.at(width);
            if chosen.is_none_or(|(_, _, _, best)| end < best) {
                chosen = Some((machine, width, start, end));
            }
        }
        let Some((machine, width, start, end)) = chosen else { continue };
        let line = &mut free[machine];
        line.iter_mut().take(width as usize).for_each(|gpu| *gpu = end);
        placed[at] = Placed { machine, width, start_s: start, end_s: end };
        end_s = end_s.max(end);
    }
    Plan { placed, end_s }
}
