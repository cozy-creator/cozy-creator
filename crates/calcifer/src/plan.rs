//! Which machine runs which job (cl-967, Paul 11-12: "calcifer could compute the quote on its end, if it has sufficient knowledge of
//! each machine, and use that to find the fastest path in its planner"). Every machine is quoted from the Status it pushes, with the
//! estimator it packs its own queue by (`placement::quote`), so trying an assignment costs no network. Each job, in dependency order,
//! goes where it finishes soonest after the jobs placed before it, beginning no sooner than what it reads ends; then single jobs move to
//! another machine while that ends the whole sooner. A job whose machine already fetches its weights ahead stays there unless another
//! finishes it clearly sooner. A finish unknown (nothing measured times it) is never sooner than a known one, and what waits for it
//! starts at a time unknown too.
use api::{calcifer::On, machine as v1};

use crate::placement::{Offer, Picture};

/// The nearest jobs not yet sent that a plan places; the rest wait for a later one.
pub(crate) const PLANNED: usize = 64;
/// The most machine quotes one plan makes.
const QUOTES: usize = 4096;
/// Passes of single-job moves after the first placement.
const PASSES: usize = 2;
/// Within this share of the soonest finish (or a second), two machines finish together.
const TIE: f64 = 0.02;
/// A job leaves the machine fetching ahead for it only for one that finishes it this much sooner, both.
const STAY: f64 = 0.10;
pub(crate) const STAY_MS: i64 = 30_000;

/// One job to place.
#[derive(Clone, Debug)]
pub(crate) struct Job {
    /// What it asks of a machine; its `prior` and `after_ms` are set here.
    pub ask: v1::EstimateJob,
    /// Its time on one GPU of each picture's kind from runs elsewhere, by picture.
    pub priors: Vec<crate::placement::Prior>,
    /// The pictures it may go to.
    pub machines: Vec<usize>,
    /// The jobs of this plan whose output it reads.
    pub needs: Vec<usize>,
    /// When what it reads that was sent already ends, on this computer's clock; and whether one of those ends at a time unknown.
    pub after_at_ms: Option<i64>,
    pub after_untimed: bool,
    /// The picture fetching its weights ahead.
    pub stay: Option<usize>,
    /// Its request as its function's estimate reads it (`placement::Job::cost`, cl-1286).
    pub cost: Option<std::sync::Arc<::needs::Cost>>,
}

/// Where one job goes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Placement {
    /// Its picture and what it would do there; none: none of its pictures can run it.
    pub chosen: Option<(usize, Offer)>,
    /// What each of its pictures would do with it after the jobs placed before it.
    pub offers: Vec<Offer>,
}

/// Each job's end where it was placed, on this computer's clock: none, it isn't; `Some(None)`, at a time unknown.
type Ends = Vec<Option<Option<i64>>>;

/// Where each of `jobs` goes, in their order.
pub(crate) fn plan(pictures: &[Picture], jobs: &[Job], now: i64) -> Vec<Placement> {
    let order = ordered(jobs);
    let based = &based(pictures, jobs);
    let mut quotes = 0;
    let mut assigned: Vec<Option<usize>> = vec![None; jobs.len()];
    // First each job where it ends soonest after those before it.
    let mut asks: Vec<Vec<v1::EstimateJob>> = vec![vec![]; pictures.len()];
    let mut ends: Ends = vec![None; jobs.len()];
    let mut offered: Vec<Vec<Offer>> = vec![vec![]; jobs.len()];
    for &j in &order {
        let after = after_ms(&jobs[j], &ends, now);
        let offers: Vec<(usize, Offer)> = (jobs[j].machines.iter())
            .map(|&m| {
                quotes += 1;
                let mut tried = asks[m].clone();
                tried.push(ask(&based[j][&m], after));
                (m, crate::placement::quote(&pictures[m], &tried, now).pop().expect("one quoted"))
            })
            .collect();
        if let Some((m, offer)) = choose(&offers, jobs[j].stay, now) {
            asks[m].push(ask(&based[j][&m], after));
            ends[j] = Some(offer.finish_at_ms);
            assigned[j] = Some(m);
        }
        offered[j] = offers.into_iter().map(|(_, offer)| offer).collect();
    }
    let mut best = evaluate(pictures, jobs, based, &order, &assigned, now, &mut quotes);
    // Then single moves, the latest finish first, while the whole ends sooner and quotes are left.
    for _ in 0..PASSES {
        let mut moved = false;
        let mut latest: Vec<usize> = (0..jobs.len()).filter(|&j| best.0[j].is_some()).collect();
        latest.sort_by_key(|&j| std::cmp::Reverse(best.0[j].as_ref().map(|(_, offer)| (offer.finish_at_ms.is_none(), offer.finish_at_ms))));
        for j in latest {
            let others: Vec<usize> = jobs[j].machines.iter().copied().filter(|&m| Some(m) != assigned[j]).collect();
            for m in others {
                if quotes + order.len() > QUOTES {
                    return placements(best.0, offered);
                }
                let mut tried = assigned.clone();
                tried[j] = Some(m);
                let trial = evaluate(pictures, jobs, based, &order, &tried, now, &mut quotes);
                if better(&trial, &best) && stays(&jobs[j], &best.0[j], &trial.0[j], now) {
                    (assigned, best, moved) = (tried, trial, true);
                }
            }
        }
        if !moved {
            break;
        }
    }
    placements(best.0, offered)
}

/// Each job's place, its chosen offer standing for its picture among the others'.
fn placements(chosen: Vec<Option<(usize, Offer)>>, offered: Vec<Vec<Offer>>) -> Vec<Placement> {
    (chosen.into_iter().zip(offered))
        .map(|(chosen, mut offers)| {
            if let Some((_, offer)) = &chosen {
                offers.retain(|other| other.machine != offer.machine);
                offers.insert(0, offer.clone());
            }
            Placement { chosen, offers }
        })
        .collect()
}

/// `jobs` with each after the jobs it reads, in their order otherwise.
fn ordered(jobs: &[Job]) -> Vec<usize> {
    let mut placed = vec![false; jobs.len()];
    let mut order = Vec::with_capacity(jobs.len());
    while order.len() < jobs.len() {
        let next = (0..jobs.len()).find(|&j| !placed[j] && jobs[j].needs.iter().all(|&need| need >= jobs.len() || placed[need]));
        // A cycle can't be read: what's left goes in its order.
        let next = next.unwrap_or_else(|| (0..jobs.len()).find(|&j| !placed[j]).expect("one left"));
        placed[next] = true;
        order.push(next);
    }
    order
}

/// The soonest `job` can start, ms from `now`: once what it reads ends. None: one of those ends at a time unknown.
fn after_ms(job: &Job, ends: &Ends, now: i64) -> Option<u64> {
    let needs: Option<Vec<i64>> = job.needs.iter().filter_map(|&need| ends.get(need).copied().flatten()).collect();
    let after = needs?.into_iter().chain(job.after_at_ms).map(|at| (at - now).max(0) as u64).max().unwrap_or(0);
    (!job.after_untimed).then_some(after)
}

/// Each job's ask of each picture it may go to, by picture: its runs there and its formula's times read once (`placement::formula`),
/// whatever it waits for.
fn based(pictures: &[Picture], jobs: &[Job]) -> Vec<std::collections::HashMap<usize, v1::EstimateJob>> {
    (jobs.iter())
        .map(|job| {
            (job.machines.iter().enumerate())
                .map(|(at, &m)| {
                    let prior = job.priors.get(at).cloned().unwrap_or_default();
                    let mut ask = v1::EstimateJob { prior: prior.runs, prior_runs: prior.own, ..job.ask.clone() };
                    crate::placement::formula(&mut ask, job.cost.as_deref(), &pictures[m]);
                    (m, ask)
                })
                .collect()
        })
        .collect()
}

/// `based` beginning once what it reads ends, `after_ms` from now (none: at a time unknown).
fn ask(based: &v1::EstimateJob, after_ms: Option<u64>) -> v1::EstimateJob {
    v1::EstimateJob { after_ms: after_ms.unwrap_or(0), after_untimed: after_ms.is_none(), ..based.clone() }
}

/// A plan's finish: how many of its jobs finish at a time unknown, then when the last known one does and all of them summed, ms from now.
type Score = (usize, i64, i64);

/// Each job's picture and offer with `assigned`, and the plan's (latest, summed) finish: each machine's jobs in dependency order,
/// each beginning once what it reads ends there or elsewhere.
#[allow(clippy::type_complexity)]
fn evaluate(pictures: &[Picture], jobs: &[Job], based: &[std::collections::HashMap<usize, v1::EstimateJob>], order: &[usize], assigned: &[Option<usize>], now: i64, quotes: &mut usize) -> (Vec<Option<(usize, Offer)>>, Score) {
    let mut asks: Vec<Vec<(usize, v1::EstimateJob)>> = vec![vec![]; pictures.len()];
    let mut ends: Ends = vec![None; jobs.len()];
    for &j in order {
        let Some(m) = assigned[j] else { continue };
        asks[m].push((j, ask(&based[j][&m], after_ms(&jobs[j], &ends, now))));
        *quotes += 1;
        let tried: Vec<v1::EstimateJob> = asks[m].iter().map(|(_, ask)| ask.clone()).collect();
        let offer = crate::placement::quote(&pictures[m], &tried, now).pop().expect("one quoted");
        ends[j] = offer.fits.then_some(offer.finish_at_ms);
    }
    // Each machine's jobs as it would run them all, what a later one does to an earlier one's place included.
    let mut placed: Vec<Option<(usize, Offer)>> = vec![None; jobs.len()];
    for (m, mine) in asks.iter().enumerate().filter(|(_, mine)| !mine.is_empty()) {
        *quotes += 1;
        let tried: Vec<v1::EstimateJob> = mine.iter().map(|(_, ask)| ask.clone()).collect();
        for ((j, _), offer) in mine.iter().zip(crate::placement::quote(&pictures[m], &tried, now)) {
            placed[*j] = offer.fits.then_some((m, offer));
        }
    }
    let finishes: Vec<i64> = placed.iter().flatten().filter_map(|(_, offer)| offer.finish_at_ms).map(|at| at - now).collect();
    let untimed = placed.iter().flatten().filter(|(_, offer)| offer.finish_at_ms.is_none()).count();
    let score = (untimed, finishes.iter().copied().max().unwrap_or(0), finishes.iter().sum());
    (placed, score)
}

/// Whether plan `trial` beats `best`: it places more of the jobs, or as many and fewer at a time unknown, or ends the whole sooner. A
/// job a plan leaves unplaced (its machine can't run it: no room on its disk, cl-1269) never counts as finishing early.
#[allow(clippy::type_complexity)]
fn better(trial: &(Vec<Option<(usize, Offer)>>, Score), best: &(Vec<Option<(usize, Offer)>>, Score)) -> bool {
    let placed = |plan: &[Option<(usize, Offer)>]| plan.iter().flatten().count();
    match placed(&trial.0).cmp(&placed(&best.0)).then(best.1.0.cmp(&trial.1.0)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => sooner(&trial.1, &best.1),
    }
}

/// Whether `trial` ends the whole sooner than `best`, by more than a tie.
fn sooner(trial: &Score, best: &Score) -> bool {
    let tie = |ms: i64| ((ms.max(0) as f64 * TIE) as i64).max(1000);
    trial.1 + tie(best.1) < best.1 || (trial.1 <= best.1 + tie(best.1) && trial.2 + tie(best.2) < best.2)
}

/// Whether a move of a job fetching ahead somewhere keeps to `STAY`: it leaves only for a finish that much sooner.
fn stays(job: &Job, was: &Option<(usize, Offer)>, now_placed: &Option<(usize, Offer)>, now: i64) -> bool {
    match (job.stay, was, now_placed) {
        (Some(stay), Some((from, before)), Some((_, after))) if *from == stay => clearly_sooner(after, before, now),
        _ => true,
    }
}

/// `sooner` finishes clearly before `than`: 10% and 30 s. Where neither's finish is known, by when each starts, as `placement::soonest`
/// chooses then (free soonest).
pub(crate) fn clearly_sooner(sooner: &Offer, than: &Offer, now: i64) -> bool {
    match (sooner.finish_at_ms, than.finish_at_ms) {
        (None, None) => finishes_clearly_sooner(sooner.start_at_ms, than.start_at_ms, now),
        (sooner, than) => finishes_clearly_sooner(sooner, than, now),
    }
}

/// A finish at `sooner` is clearly before one at `than` (`clearly_sooner`): a known one before an unknown one; an unknown one never.
pub(crate) fn finishes_clearly_sooner(sooner: Option<i64>, than: Option<i64>, now: i64) -> bool {
    match (sooner, than) {
        (Some(sooner), Some(than)) => {
            let gain = than - sooner;
            gain >= STAY_MS && gain as f64 >= STAY * (than - now).max(0) as f64
        }
        (sooner, than) => sooner.is_some() && than.is_none(),
    }
}

/// Of `offers`, where a job goes: as `placement::soonest` chooses; and the machine fetching ahead for it unless another is clearly
/// sooner.
fn choose(offers: &[(usize, Offer)], stay: Option<usize>, now: i64) -> Option<(usize, Offer)> {
    let only: Vec<Offer> = offers.iter().map(|(_, offer)| offer.clone()).collect();
    let soonest = crate::placement::soonest(&only, now)?;
    let soonest = offers.iter().find(|(_, offer)| offer.machine == soonest.machine)?;
    let fits = || offers.iter().filter(|(_, offer)| offer.fits);
    let kept = stay.and_then(|stay| fits().find(|(m, _)| *m == stay)).filter(|(_, held)| !clearly_sooner(&soonest.1, held, now));
    kept.or(Some(soonest)).cloned()
}

/// The machine `on` names among `pictures`.
pub(crate) fn index(pictures: &[Picture], on: &On) -> Option<usize> {
    pictures.iter().position(|picture| picture.on == *on)
}
