//! Where a job goes (cl-816, Paul 11-07): its machine owns its GPU width, and Calcifer only sends it where it finishes soonest. Each
//! machine a batch of jobs may go to is asked once for all of them (`POST /v1/estimate`), packed with its queue as if all were sent
//! there now; each job goes to the soonest finish, a rental before this computer on a tie (Paul, 11-05). A machine without
//! `estimate/1` is counted on eta at one GPU, and still chooses the width itself. A quote reserves nothing: two clients that pick one
//! machine are settled by its queue. Offers not yet rented (`any`, `new`) are never asked: the Hub keeps no times per SKU.
use std::sync::Arc;

use protocol::{
    calcifer::{MachineEstimate, On},
    machine::{self as v1, Basis, JobSize},
};

use crate::{api::Api, store::now_ms};

/// What a machine that has measured none of a function and is given no prior assumes: a minute (TensorD's `UNKNOWN_MS`).
const UNKNOWN_MS: u64 = 60_000;
/// Within this share of the soonest finish (or a second), two machines finish together.
const TIE: f64 = 0.02;
const ESTIMATE: &str = "estimate/1";

/// One job to place.
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub key: String,
    /// As its run's record names its target, for eta's history: `org/pkg/fn@release`, `local/pkg/fn`.
    pub target: String,
    /// As a machine's estimate names it: `org/pkg/fn@release`, or `sha256:<manifest hex>/fn` for local code.
    pub function: String,
    pub models: Vec<v1::ModelRef>,
    pub size: JobSize,
    /// The user's GPU count; 0: the machine chooses.
    pub gpus: u32,
    /// The machines it may go to.
    pub on: Vec<On>,
}

/// When one machine would run one job, on this computer's clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub machine: On,
    pub fits: bool,
    pub refusal: Option<v1::Reason>,
    pub width: u32,
    pub start_at_ms: i64,
    pub finish_at_ms: i64,
    pub prepare_ms: u64,
    pub queue_position: u32,
    pub basis: Basis,
    /// Its machine quoted it; false: eta at one GPU.
    pub quoted: bool,
}

impl Offer {
    /// How long past its preparation it waits for GPUs, from `now`.
    pub fn waits_ms(&self, now: i64) -> i64 {
        (self.start_at_ms - now - self.prepare_ms as i64).max(0)
    }

    /// As `POST /v1/run-estimates` answers it, from `now`.
    pub fn shown(&self, now: i64) -> MachineEstimate {
        let from = |at: i64| (at - now).max(0) as u64;
        MachineEstimate {
            machine: self.machine.to_string(),
            job: 0,
            fits: self.fits,
            refusal: self.refusal.clone(),
            width: self.width,
            start_ms: from(self.start_at_ms),
            finish_ms: from(self.finish_at_ms),
            prepare_ms: self.prepare_ms,
            queue_position: self.queue_position,
            basis: self.basis,
            quoted: self.quoted,
        }
    }

    /// What eta's clock takes of it once its run is sent.
    pub fn quoted(&self) -> crate::eta::Quoted {
        crate::eta::Quoted { start_at: self.start_at_ms as f64, finish_at: self.finish_at_ms as f64, basis: self.basis }
    }
}

/// Each job's offers, one from each of its machines that answered, in the order the jobs name them; and a line for each machine that
/// didn't. Each machine is asked once, for every job that may go to it.
pub async fn offers(api: &Arc<Api>, jobs: &[Job]) -> (Vec<Vec<Offer>>, Vec<String>) {
    let mut machines: Vec<&On> = vec![];
    for on in jobs.iter().flat_map(|job| &job.on).filter(|on| matches!(on, On::Machine(_))) {
        if !machines.contains(&on) {
            machines.push(on);
        }
    }
    let asked = machines.into_iter().map(|on| async move {
        let mine: Vec<usize> = (0..jobs.len()).filter(|&at| jobs[at].on.contains(on)).collect();
        let batch: Vec<&Job> = mine.iter().map(|&at| &jobs[at]).collect();
        (on, mine, quotes(api, on, &batch).await)
    });
    let mut offers = vec![vec![]; jobs.len()];
    let mut said = vec![];
    for (on, mine, answer) in futures_util::future::join_all(asked).await {
        match answer {
            Ok(quotes) => mine.into_iter().zip(quotes).for_each(|(at, offer)| offers[at].push(offer)),
            Err(why) => said.push(format!("{on} didn't say when it would finish: {why}")),
        }
    }
    for (job, offers) in jobs.iter().zip(&mut offers) {
        offers.sort_by_key(|offer| job.on.iter().position(|on| *on == offer.machine));
    }
    (offers, said)
}

/// The soonest finish of `offers` that fits; of those within a tie of it, a rental before this computer, then the first.
pub fn soonest(offers: &[Offer], now: i64) -> Option<&Offer> {
    let fits = || offers.iter().filter(|offer| offer.fits);
    let first = fits().map(|offer| offer.finish_at_ms).min()?;
    let tie = (((first - now).max(0) as f64 * TIE) as i64).max(1000);
    let tied = || fits().filter(|offer| offer.finish_at_ms <= first + tie);
    tied().find(|offer| !offer.machine.is_local()).or_else(|| tied().next())
}

/// What `on` says of `jobs`, in their order: its quote, else eta's at one GPU.
async fn quotes(api: &Arc<Api>, on: &On, jobs: &[&Job]) -> Result<Vec<Offer>, String> {
    let On::Machine(text) = on else { return Err("it isn't rented yet".into()) };
    let machines = &api.machines;
    if text == "local" {
        machines.local_up().await.map_err(|e| e.message)?;
    }
    let (row, link) = machines.linked(text).map_err(|e| e.message)?;
    let frame = match machines.frame(&row.name) {
        Some(frame) => frame,
        None => link.status().await.map_err(|e| e.to_string())?,
    };
    let prior = |job: &Job| api.calcifer.prior_ms(&job.target, &row.name, &job.size);
    if !frame.capabilities.iter().any(|capability| capability == ESTIMATE) {
        let now = now_ms();
        return Ok(jobs.iter().map(|job| eta_offer(on, prior(job), now)).collect());
    }
    let request = v1::EstimateRequest {
        jobs: (jobs.iter())
            .map(|job| v1::EstimateJob {
                key: job.key.clone(),
                function: job.function.clone(),
                models: job.models.clone(),
                size: job.size,
                gpus: job.gpus,
                prior_ms: prior(job),
            })
            .collect(),
    };
    let answer = link.estimate(&request).await.map_err(|e| e.to_string())?;
    let received = now_ms();
    let at = |ms: u64| received + ms as i64;
    jobs.iter()
        .map(|job| {
            let quote = answer.jobs.iter().find(|quote| quote.key == job.key).ok_or("it answered for other jobs than asked")?;
            Ok(Offer {
                machine: on.clone(),
                fits: quote.fits,
                refusal: quote.refusal.clone(),
                width: quote.width,
                start_at_ms: at(quote.start_ms),
                finish_at_ms: at(quote.finish_ms),
                prepare_ms: quote.prepare_ms,
                queue_position: quote.queue_position,
                basis: quote.basis,
                quoted: true,
            })
        })
        .collect::<Result<_, &str>>()
        .map_err(str::to_owned)
}

/// A machine that quotes nothing, counted as eta has it at one GPU, starting now.
fn eta_offer(on: &On, prior_ms: u64, now: i64) -> Offer {
    let (ms, basis) = match prior_ms {
        0 => (UNKNOWN_MS, Basis::None),
        ms => (ms, Basis::Prior),
    };
    Offer {
        machine: on.clone(),
        fits: true,
        refusal: None,
        width: 1,
        start_at_ms: now,
        finish_at_ms: now + ms as i64,
        prepare_ms: 0,
        queue_position: 0,
        basis,
        quoted: false,
    }
}
