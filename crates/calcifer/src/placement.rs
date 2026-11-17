//! Where a job goes (cl-816, Paul 11-07): its machine owns its GPU width, and Calcifer only sends it where it finishes soonest. Calcifer
//! quotes every machine itself (cl-967, Paul 11-12) from the Status it pushes (`Picture`), with the estimator the machine packs its own
//! queue by (`api::estimate::quote`): the jobs a machine may take packed with its queue as if all were sent there now; each job goes to
//! the soonest finish of those that can run it at all, on a tie one measured before one not and a rental before this computer (Paul,
//! 11-05). Only a machine on this cozy's minor is quoted (`Machines::current`). A quote reserves nothing: two clients that pick one
//! machine are settled by its queue. This computer's `local`, which stops after 15 idle minutes, starts to be quoted, so its finish counts
//! its start (cl-894). A rental on its way is quoted as it will picture itself once it serves (`coming`, cl-915): a job may wait for one
//! that finishes it sooner than any machine up now. A `new` run weighs a rental against them (`runs::instead_of_renting`).
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use api::{
    calcifer::{MachineEstimate, On},
    error::{Error, Kind},
    estimate::References,
    hub::cards,
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
    /// Its project's scope: its history (`prior`) and what `local` holds of it (`scoped_picture`) are read in its project alone. None:
    /// no project, or a locked one, read machine-wide only.
    pub scope: Option<crate::private_io::Scope>,
    /// Local code's folder (a packages checkout's, or a project's `./dir`): its `benchmarks.json` is its benchmarks until it's published.
    pub source: Option<std::path::PathBuf>,
    /// Of its weights fetched only when the function first reads them, the most it may download besides `models` (cl-1071).
    pub on_demand_bytes: u64,
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
    /// Its weights' bytes it downloads there first (cl-967).
    pub fetch_bytes: u64,
    /// Of its weights fetched only when the function first reads them, the most it may download there besides: in no time (cl-1071).
    pub on_demand_bytes: u64,
    /// When it was quoted.
    pub quoted_at_ms: i64,
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
    /// It as its run's record keeps it (`Run::placed`).
    pub(crate) fn placed(&self) -> api::calcifer::Placed {
        api::calcifer::Placed {
            machine: self.machine.to_string(),
            quoted_at_ms: self.quoted_at_ms,
            finish_at_ms: self.finish_at_ms,
            basis: self.basis,
            warm: self.warm,
            prepare_ms: self.prepare_ms,
            fetch_bytes: self.fetch_bytes,
            on_demand_bytes: self.on_demand_bytes,
        }
    }

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
    /// The benchmarks its quotes stand on where a function's runs don't (cl-1024): those of every job quoted with it (`benched`).
    pub references: Arc<References>,
}

/// The benchmarks a quote of `named` functions stands on (cl-1024): each one's release's (`org/pkg/fn@release`), as its kept card has
/// them, else its package's newest kept release's (a root queued on a machine names no release); and, for each, the Hub's per-card
/// speeds as its placement answer last said them. Read here, never asked: a card is kept when its release is first read.
pub(crate) fn references(hubs: &crate::rentals::Hubs, named: &[String]) -> References {
    let (root, hub) = (hubs.home().root(), hubs.default_origin());
    let mut references = References::default();
    for target in named.iter().filter(|target| !target.starts_with("sha256:")) {
        let (path, release) = target.split_once('@').map_or((target.as_str(), None), |(path, release)| (path, Some(release)));
        let Some((package, _)) = path.rsplit_once('/').filter(|(package, _)| package.contains('/')) else { continue };
        let release = release.map(str::to_owned).or_else(|| tensorfs::format::newest_by(cards::kept_releases(root, &hub, package), |r| r));
        if let Some(card) = release.and_then(|release| cards::kept(root, &hub, package, &release)) {
            references.add(package, &card.benchmarks());
        }
        if let Some(speeds) = crate::project::fit::speeds_now(&hub, target) {
            references.add_speeds(target, speeds);
        }
    }
    for (package, rows) in kept_benchmarks(root, &hub) {
        references.add_fleet(&package, &rows);
    }
    references
}

/// The benchmarks a package's source holds, its `benchmarks.json` (what `cozy package publish` attaches): a release's rows before it is
/// one. None there, or one that doesn't read (said): none.
pub(crate) fn source_benchmarks(dir: &std::path::Path) -> Vec<api::estimate::Benchmark> {
    let Ok(text) = std::fs::read(dir.join(crate::hub::author::BENCHMARKS)) else { return vec![] };
    serde_json::from_slice(&text)
        .inspect_err(|e| tracing::warn!("{}: its {} doesn't read, so its source has no benchmarks: {e}", dir.display(), crate::hub::author::BENCHMARKS))
        .unwrap_or_default()
}

/// The benchmarks `jobs` are timed by where no run of theirs is (cl-1024): a published release's from the Hub (`references`), local code's
/// from its source (`source_benchmarks`), by the function a machine's estimate names (`sha256:<manifest>/fn`): rows travel with the
/// package, in its source before it is published, on the Hub after. And those of `named`, functions queued or ahead elsewhere.
pub(crate) fn references_for<'j>(api: &Api, jobs: impl IntoIterator<Item = &'j Job>, mut named: Vec<String>) -> References {
    let mut sourced = vec![];
    for job in jobs {
        named.push(job.target.clone());
        if let Some(dir) = &job.source {
            sourced.push((job.function.clone(), source_benchmarks(dir)));
        }
    }
    named.sort();
    named.dedup();
    let mut references = references(&api.machines.hubs, &named);
    for (function, rows) in sourced {
        if let Some((code, _)) = function.split('@').next().and_then(|path| path.rsplit_once('/')) {
            references.add(code, &rows);
        }
    }
    references
}

/// Every package's benchmarks at its newest release this computer keeps a card of (th-377): how cards compare across functions
/// (`References::ratio`). A card never changes, so each is read once.
fn kept_benchmarks(root: &std::path::Path, hub: &str) -> Vec<(String, Vec<api::estimate::Benchmark>)> {
    type Read = HashMap<(String, String), (String, Vec<api::estimate::Benchmark>)>;
    static READ: LazyLock<Mutex<Read>> = LazyLock::new(Default::default);
    let mut read = READ.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    (cards::kept_packages(root, hub).into_iter())
        .filter_map(|package| {
            let newest = tensorfs::format::newest_by(cards::kept_releases(root, hub, &package), |r| r)?;
            let key = (hub.to_owned(), package.clone());
            if read.get(&key).is_none_or(|(release, _)| *release != newest) {
                let rows = cards::kept(root, hub, &package, &newest).map(|card| card.benchmarks()).unwrap_or_default();
                read.insert(key.clone(), (newest, rows));
            }
            Some((package, read[&key].1.clone())).filter(|(_, rows)| !rows.is_empty())
        })
        .collect()
}

/// Each of `pictures` with what its quotes stand on: the benchmarks of `jobs` and of each function queued or ahead on any of them
/// (`references_for`), and each run of this computer's going on one, its time left as its clock says it (cl-1073, Paul 11-16: runs went
/// to a busy machine while a faster one sat idle, each run going there counted seconds from done while it fetched its weights).
pub(crate) fn benched<'j, 'p>(api: &Api, jobs: impl IntoIterator<Item = &'j Job>, pictures: impl IntoIterator<Item = &'p mut Picture>) {
    let jobs: Vec<&Job> = jobs.into_iter().collect();
    let mut pictures: Vec<&mut Picture> = pictures.into_iter().collect();
    let mut named: Vec<String> = vec![];
    for picture in &pictures {
        named.extend(picture.frame.queue.iter().flatten().map(|root| root.function.clone()));
        named.extend(picture.ahead.iter().map(|job| job.function.clone()));
    }
    let asked: Vec<String> = jobs.iter().map(|job| job.function.clone()).chain(pictures.iter().flat_map(|picture| picture.ahead.iter().map(|job| job.function.clone()))).collect();
    let mut references = references_for(api, jobs.iter().copied(), named);
    for root in pictures.iter().flat_map(|picture| picture.frame.queue.iter().flatten()).filter(|root| root.running && !root.run.is_empty()) {
        if let Some(left) = api.calcifer.left_there(crate::store::calcifer_id(&root.run)) {
            references.left.insert(root.run.clone(), left);
        }
    }
    // What each of them timed, with its card: a function one machine timed isn't a minute on another (cl-1107, the user-flow: lillie
    // was quoted 60 s for H3's generate on no timing, chocola having just timed it at 247 s). Local code is known by its install.
    for picture in &pictures {
        let card = picture.frame.gpus.first().map(|gpu| gpu.name.clone()).unwrap_or_default();
        for timed in picture.frame.times.iter().flatten().filter(|timed| !card.is_empty() && !timed.runs.is_empty()) {
            references.timed.entry(timed.function.clone()).or_default().push((card.clone(), timed.runs.clone()));
        }
        for function in &asked {
            if let Some(name) = api::estimate::timed_name(&picture.frame, function) {
                references.timed_as.entry(function.clone()).or_insert(name);
            }
        }
    }
    let references = Arc::new(references);
    pictures.iter_mut().for_each(|picture| picture.references = references.clone());
}

/// How long `frame`'s machine stays busy with the runs it holds (`api::estimate::busy_ms`, cl-1107), timed as its quotes time them:
/// with the benchmarks of what it never ran and this computer's clocks of its runs there.
pub(crate) fn busy_ms(machines: &crate::machines::Machines, frame: &v1::StatusFrame) -> Option<u64> {
    let queue = frame.queue.as_deref().unwrap_or_default();
    if queue.is_empty() {
        return None;
    }
    let named: Vec<String> = queue.iter().map(|root| root.function.clone()).filter(|function| !function.is_empty()).collect();
    let mut references = references(&machines.hubs, &named);
    for root in queue.iter().filter(|root| root.running && !root.run.is_empty()) {
        if let Some(left) = machines.calcifer.left_there(crate::store::calcifer_id(&root.run)) {
            references.left.insert(root.run.clone(), left);
        }
    }
    api::estimate::busy_ms(frame, now_ms(), &references)
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

/// A job's time on one GPU of a machine's kind where that machine's frame times none of it (`prior`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Prior {
    pub ms: u64,
    /// How many of that machine's own runs it is (a project's, which its frame never times, cl-1047): its quote is Measured. 0: runs
    /// elsewhere.
    pub runs: u32,
}

/// What a run of `job` takes on one GPU of `picture`'s kind, as runs like it took: what it is timed by where its frame times none of it.
/// In its project's scope (`Job::scope`), whose runs here only it keeps (cl-1047).
pub(crate) async fn prior(api: &Arc<Api>, picture: &Picture, job: &Job) -> Prior {
    let timed = async {
        match &picture.ready_at_ms {
            Some(_) => match api.machines.coming(&picture.name, now_ms()) {
                Some(coming) => Prior { ms: crate::coming::card_time(api, &coming.way, &job.target, &job.size).0 as u64, runs: 0 },
                None => Prior::default(),
            },
            None => {
                let (ms, runs) = api.calcifer.prior_ms(&job.target, &picture.name, &job.size);
                Prior { ms, runs }
            }
        }
    };
    crate::private_io::context(job.scope.clone().or_else(crate::private_io::current), timed).await
}

/// `picture` as `scope`'s project sees it (cl-1047): `local` with the installs its project's runs made in that project's own worker area,
/// which the machine's frame never lists, so a job of its local code isn't quoted an install it has. Any other machine, one on its way, or
/// no scope: as it is.
pub(crate) async fn scoped_picture(api: &Arc<Api>, picture: &Picture, scope: Option<&crate::private_io::Scope>) -> Picture {
    let mut scoped = picture.clone();
    let Some(scope) = scope.filter(|_| picture.ready_at_ms.is_none() && picture.name.eq_ignore_ascii_case("local")) else { return scoped };
    for held in api.machines.project_holds(scope.clone()).await {
        if !scoped.frame.environments.iter().any(|listed| listed.installation == held.installation) {
            scoped.frame.environments.push(held);
        }
    }
    scoped
}

/// `job` as the estimator asks it, with its `prior` there.
pub(crate) fn asked(job: &Job, prior: Prior) -> v1::EstimateJob {
    v1::EstimateJob {
        key: job.key.clone(),
        function: job.function.clone(),
        models: job.models.clone(),
        size: job.size,
        gpus: job.gpus,
        gpu: job.gpu,
        prior_ms: prior.ms,
        prior_runs: prior.runs,
        on_demand_bytes: job.on_demand_bytes,
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
    let quotes = api::estimate::quote(&picture.frame, now, &asks, &picture.references);
    let at = |ms: u64| now + ms as i64;
    (quotes.into_iter().skip(picture.ahead.len()).zip(jobs))
        .map(|(quote, job)| Offer {
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
            fetch_bytes: quote.fetch_bytes,
            on_demand_bytes: if quote.fits { job.on_demand_bytes } else { 0 },
            quoted_at_ms: now,
            ready_at_ms: picture.ready_at_ms,
        })
        .collect()
}

/// Each job's offers, one from each of its machines that could be pictured, in the order the jobs name them; and a line for each machine
/// that couldn't. Each machine is quoted once, for every job that may go to it. `update`: as `pictures`.
pub(crate) async fn offers(api: &Arc<Api>, jobs: &[Job], update: bool) -> (Vec<Vec<Offer>>, Vec<String>) {
    let now = now_ms();
    let machines = machines_of(jobs);
    let mut pictured = pictures(api, &machines, update, now).await;
    benched(api, jobs, pictured.iter_mut().flatten());
    offered(api, jobs, &machines, &pictured, now).await
}

/// `offers` from what each machine last said, for a quote that changes nothing (cl-1004: an estimate shown before a render): a stopped
/// `local` isn't started, a machine on another minor isn't updated, and none is asked; one on its way as it will picture itself. `local`
/// as the jobs' project sees it (`scoped_picture`).
pub(crate) async fn offers_now(api: &Arc<Api>, jobs: &[Job]) -> (Vec<Vec<Offer>>, Vec<Result<Picture, String>>) {
    let now = now_ms();
    let machines = machines_of(jobs);
    let scope = jobs.iter().find_map(|job| job.scope.clone());
    let mut pictured = vec![];
    for on in &machines {
        pictured.push(match on {
            On::Machine(name) => match api.machines.coming(name, now) {
                Some(coming) => Ok(crate::coming::picture(api, &coming).await),
                None => match last_picture(api, on, name) {
                    Ok(picture) => Ok(scoped_picture(api, &picture, scope.as_ref()).await),
                    unpictured => unpictured,
                },
            },
            _ => Err("it isn't rented yet".into()),
        });
    }
    benched(api, jobs, pictured.iter_mut().flatten());
    let (offers, _) = offered(api, jobs, &machines, &pictured, now).await;
    (offers, pictured)
}

/// `on` as its last Status pictured it, however long ago; none for one that never said, or is broken.
fn last_picture(api: &Arc<Api>, on: &On, name: &str) -> Result<Picture, String> {
    let frame = api.machines.frame(name).ok_or("it hasn't reported what it has yet")?;
    if let Some(broken) = &frame.broken {
        return Err(format!("it's broken: {}", broken.message));
    }
    Ok(Picture { on: on.clone(), name: name.to_owned(), frame: linked(api, name, frame), ready_at_ms: None, ahead: vec![], references: Arc::default() })
}

/// `frame` with the link its machine would download at where it hasn't measured one: what this account's rentals beside it measured
/// (`Machines::link_guess`), else the estimator's flat default.
pub(crate) fn linked(api: &Arc<Api>, name: &str, mut frame: v1::StatusFrame) -> v1::StatusFrame {
    if frame.link_bytes_per_s <= 0.0
        && let Some(guess) = api.machines.link_guess(name)
    {
        frame.link_bytes_per_s = guess;
    }
    frame
}

/// Each job quoted on each of `machines` as `pictured`.
async fn offered(api: &Arc<Api>, jobs: &[Job], machines: &[On], pictured: &[Result<Picture, String>], now: i64) -> (Vec<Vec<Offer>>, Vec<String>) {
    // What each job's function read of its models on any of them (cl-967).
    let frames: Vec<&v1::StatusFrame> = pictured.iter().filter_map(|pictured| pictured.as_ref().ok()).map(|picture| &picture.frame).collect();
    let frames = &frames;
    let asked = machines.iter().zip(pictured).map(|(on, pictured)| async move {
        let mine: Vec<usize> = (0..jobs.len()).filter(|&at| jobs[at].on.contains(on)).collect();
        let answer = match pictured {
            Ok(picture) => {
                let mut asks = vec![];
                for &at in &mine {
                    let mut ask = asked(&jobs[at], prior(api, picture, &jobs[at]).await);
                    api::estimate::learn_reads(frames, &mut ask);
                    asks.push(ask);
                }
                Ok(quote(picture, &asks, now))
            }
            Err(why) => Err(why.clone()),
        };
        (on, mine, answer)
    });
    let mut offers = vec![vec![]; jobs.len()];
    let mut said = vec![];
    for (on, mine, answer) in futures_util::future::join_all(asked).await {
        match answer {
            Ok(quotes) => mine.into_iter().zip(quotes).for_each(|(at, offer)| offers[at].push(offer)),
            Err(why) => said.push(format!("{on} didn't say when it could finish: {why}")),
        }
    }
    for (job, offers) in jobs.iter().zip(&mut offers) {
        offers.sort_by_key(|offer| job.on.iter().position(|on| *on == offer.machine));
    }
    (offers, said)
}

/// The soonest finish of `offers` that can run there at all (release owner, 11-09: one that can't has no finish); of those within a tie
/// of it, a machine that serves before one on its way, one on its own measured runs before one that isn't, a rental before this
/// computer, then the sooner: never the first listed for being first (cl-1073).
pub(crate) fn soonest(offers: &[Offer], now: i64) -> Option<&Offer> {
    let fits = || offers.iter().filter(|offer| offer.fits);
    let first = fits().map(|offer| offer.finish_at_ms).min()?;
    let tie = (((first - now).max(0) as f64 * TIE) as i64).max(1000);
    let tied = || fits().filter(|offer| offer.finish_at_ms <= first + tie);
    let rank = |offer: &&Offer| (offer.ready_at_ms.is_some(), offer.basis != Basis::Measured, offer.machine.is_local(), offer.finish_at_ms);
    tied().min_by_key(rank)
}

/// Where a job goes that no machine of `on` quoted a finish for. A run named to one place goes there, which refuses it as it does;
/// of several machines, the first, a rental before this computer, that can take a run at all (one booting or updating waits there).
/// With none of them, where a run naming none would go, said once (fix, don't refuse; cl-1066): refused only when nothing can take it.
pub(crate) async fn takes(api: &Arc<Api>, on: &[On], offers: &[Offer]) -> Result<(On, Option<String>), Error> {
    let names: Vec<&str> = on.iter().filter_map(|on| match on {
        On::Machine(name) => Some(name.as_str()),
        _ => None,
    }).collect();
    if on.len() < 2 || names.len() < on.len() {
        return Ok((on.iter().find(|machine| !machine.is_local()).or(on.first()).cloned().unwrap_or(On::Any), None));
    }
    let mut why = vec![];
    for name in names.iter().filter(|name| **name != "local").chain(names.iter().filter(|name| **name == "local")) {
        let machine = On::Machine((*name).to_owned());
        match offers.iter().find(|offer| offer.machine == machine) {
            Some(offer) => why.push(format!("{name} can't run it ({})", offer.refusal.as_ref().map_or("no reason given", |reason| reason.message.as_str()))),
            None => match api.machines.reach(name).await {
                Ok(_) => return Ok((machine, None)),
                Err(e) => why.push(e.message),
            },
        }
    }
    let said = format!("None of {} can take this run: {}.", names.join(", "), why.join("; "));
    // Where a render naming none goes, this computer and this account's rentals alone, never a rental it would buy: the first there now.
    for other in api.machines.fallback().into_iter().filter(|other| !on.contains(other)) {
        let On::Machine(name) = &other else { continue };
        if api.machines.reach(name).await.is_ok() {
            return Ok((other.clone(), Some(format!("{said} It goes to {name}."))));
        }
    }
    Err(Error::new(Kind::Unavailable, "machines_unavailable", said).remedy("Bring one of them back, or pick other machines.").next("cozy machine ls"))
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
    if let Some(broken) = machines.broken(&row.name) {
        return Err(format!("it isn't working ({})", broken.message));
    }
    // Its Status as its open stream says it; with none open (it stopped answering, or never did), what it answers now, or no quote: one
    // that doesn't answer isn't a candidate, never placed on what it said before, until it answers again (xs-199).
    let frame = match machines.live_frame(&row.name) {
        Some(frame) => frame,
        None => machines.status_read(&row.name, &link).await?,
    };
    if let Some(broken) = &frame.broken {
        return Err(format!("it isn't working ({})", broken.message));
    }
    // Any version takes work (cl-1069); one on an older minor this computer may move is updated first, and placed once it's done.
    let own = this_release();
    if update
        && !same_minor(&frame.version, own)
        && !rc_version()
        && let Some(run) = machines.current(&row.name).await.map_err(|e| e.message)?
    {
        return Err(format!("it's updating to Cozy Creator {own} first (run {})", run.name));
    }
    let frame = linked(api, &row.name, frame);
    Ok(Picture { on: on.clone(), name: row.name, frame, ready_at_ms: None, ahead: vec![], references: Arc::default() })
}

/// An offer in a line of the log: `stand-c finishes in 480 s (Measured, warm)`, `pluto finishes in 390 s (None; 120 s preparing,
/// 12.0 GiB to fetch), ready in 210 s`, `local can't: does_not_fit`. Preparing: its install, download and load.
pub(crate) fn said(offer: &Offer, now: i64) -> String {
    if !offer.fits {
        return format!("{} can't run it ({})", offer.machine, offer.refusal.as_ref().map_or("no reason given", |reason| reason.message.as_str()));
    }
    let ready = offer.ready_at_ms.map(|at| format!(", ready in {} s", (at - now).max(0) / 1000)).unwrap_or_default();
    let warm = if offer.warm { ", warm" } else { "" };
    let fetch = match offer.fetch_bytes {
        0 => String::new(),
        bytes => format!(", {} to download", crate::bytes(bytes as f64)),
    };
    let preparing = match offer.prepare_ms / 1000 {
        0 => String::new(),
        s => format!("; {s} s preparing{fetch}"),
    };
    let demand = match offer.on_demand_bytes {
        0 => String::new(),
        bytes => format!("; plus up to {} on demand", crate::bytes(bytes as f64)),
    };
    format!("{} finishes in {} s ({:?}{warm}{preparing}{demand}){ready}", offer.machine, (offer.finish_at_ms - now).max(0) / 1000, offer.basis)
}

/// How soon, for people: `in under a minute`, `in about 2 min`, `in about 1.5 h`.
pub(crate) fn within(ms: i64) -> String {
    let seconds = ms.max(0) as f64 / 1e3;
    match seconds {
        s if s < 45.0 => "in under a minute".into(),
        s if s < 90.0 * 60.0 => format!("in about {}m", (s / 60.0).round().max(1.0)),
        s => format!("in about {:.1}h", s / 3600.0),
    }
}
