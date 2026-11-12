//! This machine's roots as its Status pictures them (cl-967) and its scheduler packs them (cl-816): each one's function, size,
//! widths, place and the weights it still waits for, timed by `api::estimate` from its function's runs here. Calcifer quotes from
//! the same picture with the same code, so what it plans is what happens here as far as the times hold.
use crate::{
    execution::Engine,
    journal::{function_key, Execution, Installation, State, PREPARING},
    service::Service,
};
use api::{
    estimate::{self, HISTORY, LINK},
    machine::{self as v1, JobSize},
};
use std::collections::HashMap;
use std::io;

/// The bytes fetched before this machine's own link stands for the default's.
const MEASURED_LINK: u64 = 1 << 30;

/// The GPUs a started run holds.
fn held_width(record: &Execution) -> u32 {
    match record.width {
        0 => record.invocation.degree,
        width => width,
    }
}

/// A started run's whole time here at its width, from its function's runs (`estimate::time`); None when none of it ran here
/// (Paul 11-09, 2a: the overall bar is the machine's).
pub(crate) fn whole_ms(engine: &Engine, record: &Execution) -> io::Result<Option<f64>> {
    let invocation = &record.invocation;
    let runs = match invocation.package.is_empty() {
        true => vec![],
        false => engine.run_times(&function_key(&invocation.package, &invocation.entrypoint), HISTORY)?,
    };
    let time = estimate::time(&runs, &JobSize::of(&invocation.input), 0.0, None);
    Ok((time.basis != v1::Basis::None).then(|| time.at(held_width(record))))
}

/// A root the machine gives its width as it starts: no count asked, and more than one width.
pub(crate) fn chooses(record: &Execution) -> bool {
    let invocation = &record.invocation;
    invocation.parent.is_empty() && invocation.gpus == 0 && invocation.widths.len() > 1
}

/// Its weights' arrival, bytes a second, once 1 GiB came; 0 before.
pub(crate) fn link(service: &Service) -> f64 {
    let received = service.publisher().map(|publisher| publisher.machine().received());
    match received.filter(|(bytes, time)| *bytes >= MEASURED_LINK && !time.is_zero()) {
        Some((bytes, time)) => bytes as f64 / time.as_secs_f64(),
        None => 0.0,
    }
}

/// A root not ended: its id, its caller and its caller's id for it, and as Status lists it.
pub(crate) struct Root {
    pub id: String,
    pub actor: String,
    pub caller_id: String,
    pub queued: v1::Queued,
}

/// `actor`'s installation here (of `installations`, theirs) of code as a run names it: local code by its manifest's digest
/// (`local:sha256:<hex>`), as the alias its install takes; a release by its package; an installation by its alias.
fn installation<'i>(service: &Service, installations: &'i [Installation], code: &str) -> Option<&'i Installation> {
    let find = |held: &dyn Fn(&Installation) -> bool| installations.iter().rev().find(|i| held(i));
    let digest = code.strip_prefix("local:").unwrap_or(code);
    if digest.starts_with("sha256:") {
        let alias = crate::local_source::alias(&*service.publisher()?, digest);
        return find(&|i| i.alias == alias);
    }
    find(&|i| i.alias == code || i.package == code)
}

/// What `entrypoint` of `installation` takes here at `gpus` asked: its widths, and the package its runs are timed under, its
/// generation's (`Invocation::package` once bound).
fn installed(service: &Service, installation: &Installation, entrypoint: &str, gpus: u32) -> (Option<Vec<u32>>, String) {
    let Ok(generation) = service.catalog.record(&installation.generation) else { return (None, String::new()) };
    let interface: Option<serde_json::Value> = serde_json::from_slice(&installation.interface).ok();
    let entry = interface.as_ref().and_then(|interface| api::interface::entrypoint(interface, entrypoint));
    let widths = entry.zip(service.pool()).map(|(entry, pool)| pool.widths(entry, &generation, gpus));
    (widths, generation.package)
}

/// Each of `installation`'s functions as this machine times and packs it (`Environment::functions`).
pub(crate) fn functions(service: &Service, installation: &Installation, entrypoints: &[String]) -> Vec<v1::HeldFunction> {
    (entrypoints.iter())
        .filter_map(|entrypoint| {
            let (widths, package) = installed(service, installation, entrypoint, 0);
            (!package.is_empty()).then(|| v1::HeldFunction { entrypoint: entrypoint.clone(), timed_as: function_key(&package, entrypoint), widths: widths.unwrap_or_default() })
        })
        .collect()
}

/// The bytes of weights a queued run still waits on, as its progress says.
fn fetching(record: &Execution) -> u64 {
    let progress: serde_json::Value = record.progress.as_deref().and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
    let count = |key: &str| progress.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
    count("bytes_total").saturating_sub(count("bytes_done"))
}

/// Every root here not ended, every caller's, by acceptance.
pub(crate) fn roots(service: &Service) -> io::Result<Vec<Root>> {
    let engine = &service.engine;
    let gpus = service.pool().map_or(0, |pool| pool.serving_width());
    let downloading = crate::activity::downloading();
    let mut held: HashMap<String, Vec<Installation>> = HashMap::new();
    let mut roots = vec![];
    for record in engine.nonterminal(usize::MAX)? {
        let invocation = &record.invocation;
        // A project's fetch ahead (a function's warm run) fetches behind every run; it holds no place in the line.
        if !invocation.parent.is_empty() || (invocation.prepares && !invocation.entrypoint.is_empty()) {
            continue;
        }
        let actor = record.submission.as_ref().map(|s| s.actor.clone()).unwrap_or_default();
        let preparing = record.state == State::Queued && record.waiting_reason.as_deref() == Some(PREPARING);
        let (widths, package) = match (invocation.prepares || invocation.entrypoint.is_empty(), preparing) {
            // An install or a model's download: in the lines, never on a GPU.
            (true, _) => (vec![], String::new()),
            // As the installation of the code it names holds it, until it binds its own.
            (false, true) => {
                if !held.contains_key(&actor) {
                    held.insert(actor.clone(), engine.installations(&actor)?);
                }
                let (widths, package) = installation(service, &held[&actor], &invocation.package)
                    .map_or((None, String::new()), |i| installed(service, i, &invocation.entrypoint, invocation.gpus));
                (widths.unwrap_or_else(|| estimate::unheld_widths(gpus, invocation.gpus)), package)
            }
            (false, false) => (invocation.widths.clone(), invocation.package.clone()),
        };
        let running = matches!(record.state, State::Starting | State::Running);
        let private = invocation.local_private.is_some();
        let caller_id = (record.submission.as_ref().map(|s| s.request_id.clone()))
            .filter(|request| !request.is_empty())
            .unwrap_or_else(|| record.id.clone());
        let queued = v1::Queued {
            run: String::new(),
            function: match private || package.is_empty() {
                true => String::new(),
                false => function_key(&package, &invocation.entrypoint),
            },
            size: JobSize::of(&invocation.input),
            chooses: invocation.gpus == 0 && widths.len() > 1,
            widths,
            running,
            width: if running { held_width(&record) } else { 0 },
            started_at_unix_ms: record.started_at_ms as i64,
            models: match private {
                true => vec![],
                false => downloading.get(&caller_id).cloned().unwrap_or_default(),
            },
            bytes_left: if running { 0 } else { fetching(&record) },
        };
        roots.push(Root { id: record.id.clone(), actor, caller_id, queued });
    }
    Ok(roots)
}

/// Each waiting root's width as it would start, of those the machine chooses for (`chooses`), from the same picture Calcifer
/// quotes by.
pub(crate) fn widths(service: &Service, pool: &crate::gpu_service::Pool) -> io::Result<HashMap<String, u32>> {
    let roots = roots(service)?;
    let mut times = HashMap::new();
    for root in roots.iter().filter(|root| !root.queued.function.is_empty()) {
        if !times.contains_key(&root.queued.function) {
            times.insert(root.queued.function.clone(), service.engine.run_times(&root.queued.function, HISTORY)?);
        }
    }
    let (card, nvlink) = pool.hardware_class();
    let link = link(service);
    let machine = estimate::Machine {
        gpus: pool.serving_width(),
        link: if link > 0.0 { link / 1e3 } else { LINK },
        card,
        nvlink,
        times,
        now_ms: crate::machine::lifecycle::now_ms(),
    };
    let queued: Vec<v1::Queued> = roots.iter().map(|root| root.queued.clone()).collect();
    let (jobs, _) = machine.queue(&queued);
    let placed = estimate::pack(machine.gpus, &jobs);
    Ok((roots.into_iter().zip(jobs).zip(placed))
        .filter(|((root, job), place)| job.running.is_none() && root.queued.chooses && place.width > 0)
        .map(|((root, _), place)| (root.id, place.width))
        .collect())
}
