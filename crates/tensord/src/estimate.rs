//! What this machine would do with jobs it may be sent (cl-816): its queue and them, laid out by
//! `pack`, each one's time from its function's runs here (`Journal::run_times`): their compute, and
//! with its weights not here, its download and its measured load (release owner, 11-09). Its scheduler gives each root its width from the same picture (`widths`), so a quote is
//! what happens as far as the times hold.
use crate::{
    journal::{function_key, Execution, Installation, RunTime, State, PREPARING},
    pack::{self, Job, Scaling},
    runs::Runs,
    service::Service,
};
use protocol::{
    error::Error,
    machine::{self as v1, Basis, JobSize},
};
use std::collections::{BTreeMap, HashMap};
use std::io;

/// A function's time when none of it ran here and the caller knows none.
const UNKNOWN_MS: f64 = 60_000.0;
/// An install this machine has not timed.
const INSTALL_MS: f64 = 120_000.0;
/// Weights' arrival, bytes a millisecond, until this machine has fetched enough to know its link.
const LINK: f64 = 100_000.0;
const MEASURED_LINK: u64 = 1 << 30;
/// The runs a time stands on, newest first, and of them those at one width.
const HISTORY: usize = 40;
const RECENT: usize = 5;
/// Within this share of a run's size, it is that size.
const SAME_SIZE: f64 = 0.15;
/// A running root past its time has at least this share of it left.
const OVERRUN: f64 = 0.1;

/// A function's time here: its compute on one GPU, how it scales, its load onto the GPUs, and what
/// they stand on.
struct Time {
    one_ms: f64,
    scaling: Scaling,
    load_ms: f64,
    basis: Basis,
}

/// From its newest runs' compute here (`runs`, newest first) at `size`, else the caller's
/// `prior_ms`; and the median load of those that loaded.
fn time(runs: &[RunTime], size: &JobSize, prior_ms: f64) -> Time {
    if runs.is_empty() {
        return match prior_ms > 0.0 {
            true => Time { one_ms: prior_ms, scaling: Scaling::PRIOR, load_ms: 0.0, basis: Basis::Prior },
            false => Time { one_ms: UNKNOWN_MS, scaling: Scaling::PRIOR, load_ms: 0.0, basis: Basis::None },
        };
    }
    let same: Vec<&RunTime> = runs.iter().filter(|run| (size.ratio(&run.size) - 1.0).abs() <= SAME_SIZE).collect();
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
        _ => Scaling::PRIOR,
    };
    let (width, ms) = by_width.iter().max_by_key(|(width, ms)| (ms.len(), std::cmp::Reverse(**width))).map(|(w, ms)| (*w, median(ms))).expect("a run");
    let loads: Vec<f64> = runs.iter().map(|run| run.load_ms).filter(|ms| *ms > 0.0).take(RECENT).collect();
    let load_ms = if loads.is_empty() { 0.0 } else { median(&loads) };
    Time { one_ms: ms / scaling.at(width), scaling, load_ms, basis: Basis::Measured }
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

/// A root the machine gives its width as it starts: no count asked, and more than one width.
pub(crate) fn chooses(record: &Execution) -> bool {
    let invocation = &record.invocation;
    invocation.parent.is_empty() && invocation.gpus == 0 && invocation.widths.len() > 1
}

/// What the machine reads a queue and jobs with.
struct Here<'a> {
    service: &'a Service,
    gpus: usize,
    /// Bytes a millisecond its weights arrive at.
    link: f64,
    now_ms: u64,
}

impl<'a> Here<'a> {
    fn of(service: &'a Service, gpus: usize) -> Self {
        let link = service.publisher().map(|publisher| publisher.machine().received()).filter(|(bytes, time)| *bytes >= MEASURED_LINK && !time.is_zero());
        Here {
            service,
            gpus,
            link: link.map_or(LINK, |(bytes, time)| bytes as f64 / (time.as_secs_f64() * 1e3)),
            now_ms: crate::machine::lifecycle::now_ms() as u64,
        }
    }

    /// `package` (a generation's, as its runs name it) at `size`; empty: none of it ran here.
    fn time(&self, package: &str, entrypoint: &str, size: &JobSize, prior_ms: f64) -> io::Result<Time> {
        let runs = match package.is_empty() {
            true => vec![],
            false => self.service.engine.run_times(&function_key(package, entrypoint), HISTORY)?,
        };
        Ok(time(&runs, size, prior_ms))
    }

    /// `actor`'s installation here (of `installations`, theirs) of code as a run or a job names it: local code by its
    /// manifest's digest (`sha256:<hex>`, a run's `local:sha256:<hex>`), as the alias its install takes; a release by its
    /// package, and its release when one is named; an installation by its alias.
    fn installation<'i>(&self, installations: &'i [Installation], code: &str, release: &str) -> Option<&'i Installation> {
        let find = |held: &dyn Fn(&Installation) -> bool| installations.iter().rev().find(|i| held(i));
        let digest = code.strip_prefix("local:").unwrap_or(code);
        if digest.starts_with("sha256:") {
            let alias = crate::local_source::alias(&*self.service.publisher()?, digest);
            return find(&|i| i.alias == alias);
        }
        find(&|i| i.alias == code || (i.package == code && (release.is_empty() || i.release == release)))
    }

    /// What `entrypoint` of `installation` takes here at `gpus` asked: its widths, and the package its runs are timed under,
    /// its generation's (`Invocation::package` once bound).
    fn installed(&self, installation: &Installation, entrypoint: &str, gpus: u32) -> (Option<Vec<u32>>, String) {
        let Ok(generation) = self.service.catalog.record(&installation.generation) else { return (None, String::new()) };
        let interface: Option<serde_json::Value> = serde_json::from_slice(&installation.interface).ok();
        let entry = interface.as_ref().and_then(|interface| protocol::interface::entrypoint(interface, entrypoint));
        let widths = entry.zip(self.service.pool()).map(|(entry, pool)| pool.widths(entry, &generation, gpus));
        (widths, generation.package)
    }

    /// Widths when the function's interface is not held: the count asked, else one GPU.
    fn unknown_widths(&self, gpus: u32) -> Vec<u32> {
        match (self.gpus, gpus) {
            (0, _) => vec![],
            (_, 0) => vec![1],
            (held, asked) => vec![asked.min(held as u32)],
        }
    }

    /// The machine's roots as `pack` sees them, by acceptance: their ids and jobs.
    fn queue(&self) -> io::Result<Vec<(String, Job)>> {
        let engine = &self.service.engine;
        let mut installed: HashMap<String, Vec<Installation>> = HashMap::new();
        let mut queue = vec![];
        for record in engine.nonterminal(usize::MAX)? {
            let invocation = &record.invocation;
            if !invocation.parent.is_empty() || invocation.prepares || invocation.entrypoint.is_empty() {
                continue;
            }
            let preparing = record.state == State::Queued && record.waiting_reason.as_deref() == Some(PREPARING);
            let (widths, package) = match preparing {
                // As the installation of the code it names holds it, until it binds its own.
                true => {
                    let actor = record.submission.as_ref().map(|s| s.actor.clone()).unwrap_or_default();
                    if !installed.contains_key(&actor) {
                        installed.insert(actor.clone(), engine.installations(&actor)?);
                    }
                    let (widths, package) = (self.installation(&installed[&actor], &invocation.package, ""))
                        .map_or((None, String::new()), |i| self.installed(i, &invocation.entrypoint, invocation.gpus));
                    (widths.unwrap_or_else(|| self.unknown_widths(invocation.gpus)), package)
                }
                false => (invocation.widths.clone(), invocation.package.clone()),
            };
            let time = self.time(&package, &invocation.entrypoint, &JobSize::of(&invocation.input), 0.0)?;
            let mut job = Job { widths, one_ms: time.one_ms, scaling: time.scaling, ready_ms: 0.0, running: None };
            match record.state {
                State::Starting | State::Running => {
                    let width = match record.width {
                        0 => invocation.degree,
                        width => width,
                    };
                    let whole = job.ms(width);
                    let ran = match record.started_at_ms {
                        0 => 0.0,
                        started => self.now_ms.saturating_sub(started) as f64,
                    };
                    job.running = Some((width, (whole - ran).max(whole * OVERRUN)));
                }
                _ => job.ready_ms = fetching(&record) as f64 / self.link,
            }
            queue.push((record.id.clone(), job));
        }
        Ok(queue)
    }
}

/// The bytes of weights a queued run still waits on, as its progress says.
fn fetching(record: &Execution) -> u64 {
    let progress: serde_json::Value = record.progress.as_deref().and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
    let count = |key: &str| progress.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
    count("bytes_total").saturating_sub(count("bytes_done"))
}

/// Each waiting root's width as it would start, of those the machine chooses for (`chooses`).
pub(crate) fn widths(service: &Service, pool: &crate::gpu_service::Pool) -> io::Result<HashMap<String, u32>> {
    let queue = Here::of(service, pool.width()).queue()?;
    let jobs: Vec<Job> = queue.iter().map(|(_, job)| job.clone()).collect();
    let placed = pack::pack(pool.width(), &jobs);
    Ok(queue.into_iter().zip(placed)
        .filter(|((_, job), place)| job.running.is_none() && job.widths.len() > 1 && place.width > 0)
        .map(|((id, _), place)| (id, place.width))
        .collect())
}

fn invalid(function: &str) -> Error {
    Error::validation("invalid_function", format!("{function:?} is not org/package/function[@release] or sha256:<hex>/function"))
}

/// `function` as `EstimateJob::function` names it: its code (`sha256:<hex>`, or a release's package), release (empty: any)
/// and callable.
fn named(function: &str) -> Result<(&str, &str, &str), Error> {
    if function.starts_with("sha256:") {
        let (code, entrypoint) = function.split_once('/').filter(|(code, e)| code.len() == 71 && !e.is_empty()).ok_or_else(|| invalid(function))?;
        return Ok((code, "", entrypoint));
    }
    let (path, release) = function.split_once('@').unwrap_or((function, ""));
    match path.rsplit_once('/') {
        Some((package, entrypoint)) if package.contains('/') && !entrypoint.is_empty() => Ok((package, release, entrypoint)),
        _ => Err(invalid(function)),
    }
}

/// The bytes of `model` this machine lacks: what its held structure names and its store lacks,
/// else all the catalog lists.
fn absent(store: &tensorfs::store::Store, model: &v1::ModelRef) -> u64 {
    match crate::storage::checkpoint(store, &model.commit) {
        Ok(checkpoint) => crate::storage::chunks(&checkpoint, &[]).iter().filter(|o| !store.holds(o)).map(|o| o.length).sum(),
        Err(_) => model.bytes,
    }
}

/// When each of `request`'s jobs would run here were `actor` to send them all now, after this
/// machine's queue (`POST /v1/estimate`).
pub(crate) fn quote(runs: &Runs, actor: &str, request: v1::EstimateRequest) -> Result<v1::EstimateAnswer, Error> {
    let internal = |error: io::Error| Error::internal("estimate_unreadable", error.to_string());
    let service = &runs.service;
    let here = Here::of(service, service.pool().map_or(0, |pool| pool.width()));
    let mut queue: Vec<Job> = here.queue().map_err(internal)?.into_iter().map(|(_, job)| job).collect();
    let installations = service.engine.installations(actor).map_err(internal)?;
    let store = runs.publisher.store();
    let free = crate::reclaim::Disk::measure(store.root()).map(|disk| disk.free()).map_err(internal)?;
    let mut quotes = vec![];
    // Each job's place in `queue` once it fits, and its quote so far.
    let mut asked: Vec<(Option<usize>, v1::Quote)> = vec![];
    for job in request.jobs {
        let (code, release, entrypoint) = named(&job.function)?;
        // Its code as held here, else another release of its package: their widths and times are about its own.
        let installation = here.installation(&installations, code, release);
        let (widths, package) = (installation.or_else(|| here.installation(&installations, code, "")))
            .map_or((None, String::new()), |i| here.installed(i, entrypoint, job.gpus));
        let widths = widths.unwrap_or_else(|| here.unknown_widths(job.gpus));
        let fetch: u64 = job.models.iter().map(|model| absent(store, model)).sum();
        let time = here.time(&package, entrypoint, &job.size, job.prior_ms as f64).map_err(internal)?;
        // Weights not here: their download at this machine's link, and the load its runs measured putting them on its GPUs.
        let load_ms = match fetch {
            0 => 0.0,
            fetch => fetch as f64 / here.link + time.load_ms,
        };
        let prepare_ms = if installation.is_some() { 0.0 } else { INSTALL_MS } + load_ms;
        let mut quote = v1::Quote { key: job.key, basis: time.basis, prepare_ms: prepare_ms as u64, ..Default::default() };
        if job.gpu && here.gpus == 0 {
            quote.refusal = Some(v1::Reason {
                code: "no_gpu".into(),
                message: "this machine has no GPU it may use, and it builds a model on one".into(),
                origin: "machine".into(),
                ..Default::default()
            });
            asked.push((None, quote));
            continue;
        }
        if fetch > free {
            quote.refusal = Some(v1::Reason {
                code: "does_not_fit".into(),
                message: format!("its weights' {fetch} bytes don't fit in this machine's {free} free"),
                origin: "machine".into(),
                ..Default::default()
            });
            asked.push((None, quote));
            continue;
        }
        quote.fits = true;
        queue.push(Job { widths, one_ms: time.one_ms, scaling: time.scaling, ready_ms: prepare_ms, running: None });
        asked.push((Some(queue.len() - 1), quote));
    }
    let placed = pack::pack(here.gpus, &queue);
    for (at, mut quote) in asked {
        if let Some(place) = at.map(|at| placed[at]) {
            (quote.width, quote.start_ms, quote.finish_ms) = (place.width, place.start_ms as u64, place.finish_ms as u64);
            quote.queue_position = place.position as u32;
        }
        quotes.push(quote);
    }
    Ok(v1::EstimateAnswer { jobs: quotes })
}
