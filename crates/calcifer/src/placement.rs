//! Where a job goes (cl-816, Paul 11-07): its machine owns its GPU width, and Calcifer only sends it where it finishes soonest. Calcifer
//! quotes every machine itself (cl-967, Paul 11-12) from the Status it pushes (`Picture`), with the estimator the machine packs its own
//! queue by (`api::estimate::quote`): the jobs a machine may take packed with its queue as if all were sent there now; each job goes to
//! the soonest finish of those that can run it at all, on a tie one measured before one not and a rental before this computer (Paul,
//! 11-05). Only a machine on this cozy's minor is quoted (`Machines::current`). A quote reserves nothing: two clients that pick one
//! machine are settled by its queue. This computer's `local`, which stops after 15 idle minutes, starts to be quoted, so its finish counts
//! its start (cl-894). A rental on its way is quoted as it will picture itself once it serves (`coming`, cl-915): a job may wait for one
//! that finishes it sooner than any machine up now. A `new` run weighs a rental against them (`runs::instead_of_renting`).
use std::sync::Arc;

use api::{
    calcifer::{MachineEstimate, On},
    error::{Error, Kind},
    machine::{self as v1, Basis, JobSize},
};

use crate::{
    api::Api,
    store::now_ms,
    update::{rc_version, same_minor, this_release},
};

/// Within this share of the soonest finish (or a second), two machines finish together.
const TIE: f64 = 0.02;

/// One job to place.
#[derive(Clone, Debug, Default)]
pub(crate) struct Job {
    pub key: String,
    /// As its run's record names its target, for eta's history: `org/pkg/fn@release`, `local/pkg/fn`.
    pub target: String,
    /// As a machine's estimate names it: `org/pkg/fn@release`, or `sha256:<manifest hex>/fn` for local code.
    pub function: String,
    pub models: Vec<v1::ModelRef>,
    pub size: JobSize,
    /// The user's GPU count; 0: the machine chooses.
    pub gpus: u32,
    /// It builds a model on a GPU, itself or a call it makes (`Interface::needs_gpu`): a machine with none can't run it.
    pub gpu: bool,
    /// The machines it may go to.
    pub on: Vec<On>,
}

/// When one machine would run one job, on this computer's clock.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Offer {
    pub machine: On,
    pub fits: bool,
    pub refusal: Option<v1::Reason>,
    pub width: u32,
    pub start_at_ms: i64,
    pub finish_at_ms: i64,
    pub prepare_ms: u64,
    pub queue_position: u32,
    pub basis: Basis,
    /// Its time is its function's warm one there (cl-967).
    pub warm: bool,
    /// When its machine should serve, for one on its way (cl-915); none for one that serves.
    pub ready_at_ms: Option<i64>,
}

impl Offer {
    /// What a job held for a machine on its way says (cl-915): `waits for pluto, ready in about 2 min`. None for one that serves.
    pub(crate) fn waits_for(&self, now: i64) -> Option<String> {
        let at = self.ready_at_ms?;
        Some(format!("waits for {}, ready {}", self.machine, within(at - now)))
    }

    /// How long past its preparation it waits for GPUs, from `now`.
    pub(crate) fn waits_ms(&self, now: i64) -> i64 {
        (self.start_at_ms - now - self.prepare_ms as i64).max(0)
    }

    /// As `POST /v1/run-estimates` answers it, from `now`: one that can't run there has no times.
    pub(crate) fn shown(&self, now: i64) -> MachineEstimate {
        let from = |at: i64| self.fits.then(|| (at - now).max(0) as u64);
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
            ready_ms: self.ready_at_ms.map(|at| (at - now).max(0) as u64),
        }
    }

    /// What eta's clock takes of it once its run is sent.
    pub(crate) fn quoted(&self) -> crate::eta::Quoted {
        crate::eta::Quoted { start_at: self.start_at_ms as f64, finish_at: self.finish_at_ms as f64, basis: self.basis }
    }
}

/// What Calcifer quotes a machine by: its last Status, or for one on its way the one it will push once it serves.
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    pub on: On,
    /// Its machine's name, for its runs' history.
    pub name: String,
    pub frame: v1::StatusFrame,
    /// When it should serve, for one on its way (cl-915); none for one that serves.
    pub ready_at_ms: Option<i64>,
    /// What waits there that its frame doesn't list yet, as jobs ahead of any asked: every run sent to one on its way.
    pub ahead: Vec<v1::EstimateJob>,
}

/// The machines of `jobs`, each named once, in the order they name them.
pub(crate) fn machines_of(jobs: &[Job]) -> Vec<On> {
    let mut machines: Vec<On> = vec![];
    for on in jobs.iter().flat_map(|job| &job.on).filter(|on| matches!(on, On::Machine(_))) {
        if !machines.contains(on) {
            machines.push(on.clone());
        }
    }
    machines
}

/// Each of `machines` pictured, or why it can't be. `update`: work is going now, so a machine on another minor is updated to this
/// one (it is pictured once it is); else it is only said.
pub(crate) async fn pictures(api: &Arc<Api>, machines: &[On], update: bool, now: i64) -> Vec<Result<Picture, String>> {
    futures_util::future::join_all(machines.iter().map(|on| async move {
        // One on its way is pictured as it will be once it serves.
        match on {
            On::Machine(name) => match api.machines.coming(name, now) {
                Some(coming) => Ok(crate::coming::picture(api, &coming).await),
                None => picture(api, on, update).await,
            },
            _ => Err("it isn't rented yet".into()),
        }
    }))
    .await
}

/// What a run of `job` takes on one GPU of `picture`'s kind, as runs like it took elsewhere: what it is timed by where none of it ran.
pub(crate) async fn prior(api: &Arc<Api>, picture: &Picture, job: &Job) -> u64 {
    match &picture.ready_at_ms {
        Some(_) => match api.machines.coming(&picture.name, now_ms()) {
            Some(coming) => crate::coming::card_time(api, &coming.hub, &coming.way, &job.target, &job.size).await.0 as u64,
            None => 0,
        },
        None => api.calcifer.prior_ms(&job.target, &picture.name, &job.size),
    }
}

/// `job` as the estimator asks it, with its `prior_ms` there.
pub(crate) fn asked(job: &Job, prior_ms: u64) -> v1::EstimateJob {
    v1::EstimateJob {
        key: job.key.clone(),
        function: job.function.clone(),
        models: job.models.clone(),
        size: job.size,
        gpus: job.gpus,
        gpu: job.gpu,
        prior_ms,
        after_ms: 0,
    }
}

/// When `picture`'s machine would run each of `jobs` were they all sent there now, after its queue and what is ahead of it there; one on
/// its way begins none before it serves. On this computer's clock.
pub(crate) fn quote(picture: &Picture, jobs: &[v1::EstimateJob], now: i64) -> Vec<Offer> {
    let ready = picture.ready_at_ms.map_or(0, |at| (at - now).max(0) as u64);
    let asks: Vec<v1::EstimateJob> = (picture.ahead.iter().chain(jobs))
        .map(|job| v1::EstimateJob { after_ms: job.after_ms.max(ready), ..job.clone() })
        .collect();
    let quotes = api::estimate::quote(&picture.frame, now, &asks);
    let at = |ms: u64| now + ms as i64;
    (quotes.into_iter().skip(picture.ahead.len()))
        .map(|quote| Offer {
            machine: picture.on.clone(),
            fits: quote.fits,
            refusal: quote.refusal,
            width: quote.width,
            start_at_ms: at(quote.start_ms),
            finish_at_ms: at(quote.finish_ms),
            prepare_ms: quote.prepare_ms,
            queue_position: quote.queue_position,
            basis: quote.basis,
            warm: quote.warm,
            ready_at_ms: picture.ready_at_ms,
        })
        .collect()
}

/// Each job's offers, one from each of its machines that could be pictured, in the order the jobs name them; and a line for each machine
/// that couldn't. Each machine is quoted once, for every job that may go to it. `update`: as `pictures`.
pub(crate) async fn offers(api: &Arc<Api>, jobs: &[Job], update: bool) -> (Vec<Vec<Offer>>, Vec<String>) {
    let now = now_ms();
    let machines = machines_of(jobs);
    let pictured = pictures(api, &machines, update, now).await;
    let asked = machines.iter().zip(pictured).map(|(on, pictured)| async move {
        let mine: Vec<usize> = (0..jobs.len()).filter(|&at| jobs[at].on.contains(on)).collect();
        let answer = match pictured {
            Ok(picture) => {
                let mut asks = vec![];
                for &at in &mine {
                    asks.push(asked(&jobs[at], prior(api, &picture, &jobs[at]).await));
                }
                Ok(quote(&picture, &asks, now))
            }
            Err(why) => Err(why),
        };
        (on, mine, answer)
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

/// The soonest finish of `offers` that can run there at all (release owner, 11-09: one that can't has no finish); of those within a tie
/// of it, a machine that serves before one on its way, one on its own measured runs before one that isn't, and a rental before this
/// computer.
pub(crate) fn soonest(offers: &[Offer], now: i64) -> Option<&Offer> {
    let fits = || offers.iter().filter(|offer| offer.fits);
    let first = fits().map(|offer| offer.finish_at_ms).min()?;
    let tie = (((first - now).max(0) as f64 * TIE) as i64).max(1000);
    let tied = || fits().filter(|offer| offer.finish_at_ms <= first + tie);
    let rank = |offer: &&Offer| (offer.ready_at_ms.is_some(), offer.basis != Basis::Measured, offer.machine.is_local());
    tied().min_by_key(rank)
}

/// Where a job goes that no machine of `on` quoted a finish for. A run named to one place goes there, which refuses it as it does;
/// of several machines, the first, a rental before this computer, that can take a run at all (one booting or updating waits there),
/// else it is refused naming each and why: never another machine (cl-873).
pub(crate) async fn takes(api: &Arc<Api>, on: &[On], offers: &[Offer]) -> Result<On, Error> {
    let names: Vec<&str> = on.iter().filter_map(|on| match on {
        On::Machine(name) => Some(name.as_str()),
        _ => None,
    }).collect();
    if on.len() < 2 || names.len() < on.len() {
        return Ok(on.iter().find(|machine| !machine.is_local()).or(on.first()).cloned().unwrap_or(On::Any));
    }
    let mut why = vec![];
    for name in names.iter().filter(|name| **name != "local").chain(names.iter().filter(|name| **name == "local")) {
        let machine = On::Machine((*name).to_owned());
        match offers.iter().find(|offer| offer.machine == machine) {
            Some(offer) => why.push(format!("{name} can't run it: {}", offer.refusal.as_ref().map_or("no reason given", |reason| reason.message.as_str()))),
            None => match api.machines.reach(name).await {
                Ok(_) => return Ok(machine),
                Err(e) => why.push(e.message),
            },
        }
    }
    let said = format!("none of {} can take this run: {}", names.join(", "), why.join("; "));
    Err(Error::new(Kind::Unavailable, "machines_unavailable", said).remedy("bring one of them back, or name others with --on").next("cozy machine ls"))
}

/// `on` as its last Status pictures it, read once if none came yet.
async fn picture(api: &Arc<Api>, on: &On, update: bool) -> Result<Picture, String> {
    let On::Machine(text) = on else { return Err("it isn't rented yet".into()) };
    let machines = &api.machines;
    // Stopped, it starts now: what that takes is in its finish, counted from its answer.
    if text == "local" {
        machines.local_up().await.map_err(|e| e.message)?;
    }
    let (row, link) = machines.linked(text).map_err(|e| e.message)?;
    // Its Status as its open stream says it; with none open (it stopped answering, or never did), what it answers now, or no quote.
    let mut frame = match machines.live_frame(&row.name) {
        Some(frame) => frame,
        None => link.status().await.map_err(|e| e.to_string())?,
    };
    // A project's work on this computer's machine runs in its own engine: what it holds, queues and has measured there is
    // in its own Status alone.
    if row.name == "local"
        && let Some(scope) = crate::private_io::current()
        && let Ok(own) = link.with_private(scope).status().await
    {
        frame = with_own(frame, own);
    }
    let own = this_release();
    if !same_minor(&frame.version, own) && !rc_version() {
        if !update {
            return Err(format!("it runs cozy {}, another minor than this cozy's {own}: its first run updates it", frame.version));
        }
        match machines.current(&row.name).await.map_err(|e| e.message)? {
            None => {}
            Some(run) => return Err(format!("updating it to cozy {own} first (run {})", run.number)),
        }
    }
    Ok(Picture { on: on.clone(), name: row.name, frame, ready_at_ms: None, ahead: vec![] })
}

/// `frame`, the machine's own Status, with what a project's own engine there said of itself: its models, environments, warm set,
/// residents, queue and measured times beside the machine's.
fn with_own(mut frame: v1::StatusFrame, own: v1::StatusFrame) -> v1::StatusFrame {
    frame.models.extend(own.models);
    frame.environments.extend(own.environments);
    frame.warm.extend(own.warm);
    frame.resident.extend(own.resident);
    if let Some(queued) = own.queue {
        frame.queue.get_or_insert_with(Vec::new).extend(queued);
    }
    for timed in own.times.into_iter().flatten() {
        let times = frame.times.get_or_insert_with(Vec::new);
        match times.iter_mut().find(|held| held.function == timed.function) {
            Some(held) => held.runs.extend(timed.runs),
            None => times.push(timed),
        }
    }
    if frame.link_bytes_per_s <= 0.0 {
        frame.link_bytes_per_s = own.link_bytes_per_s;
    }
    frame
}

/// An offer in a line of the log: `stand-c finishes in 480 s (Measured, warm)`, `pluto finishes in 390 s (None), ready in 210 s`,
/// `local can't: does_not_fit`.
pub(crate) fn said(offer: &Offer, now: i64) -> String {
    if !offer.fits {
        return format!("{} can't: {}", offer.machine, offer.refusal.as_ref().map_or("no reason given", |reason| reason.code.as_str()));
    }
    let ready = offer.ready_at_ms.map(|at| format!(", ready in {} s", (at - now).max(0) / 1000)).unwrap_or_default();
    let warm = if offer.warm { ", warm" } else { "" };
    format!("{} finishes in {} s ({:?}{warm}){ready}", offer.machine, (offer.finish_at_ms - now).max(0) / 1000, offer.basis)
}

/// How soon, for people: `in under a minute`, `in about 2 min`, `in about 1.5 h`.
pub(crate) fn within(ms: i64) -> String {
    let seconds = ms.max(0) as f64 / 1e3;
    match seconds {
        s if s < 45.0 => "in under a minute".into(),
        s if s < 90.0 * 60.0 => format!("in about {} min", (s / 60.0).round().max(1.0)),
        s => format!("in about {:.1} h", s / 3600.0),
    }
}
