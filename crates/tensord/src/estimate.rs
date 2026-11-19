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

/// The bytes a fetch ramps its streams up over, and then the bytes more before this machine's own link stands for the default's.
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
    // A started run is its function's first call here when none ran before it (cold), else a later one (warm).
    let time = estimate::time(&runs, &JobSize::of(&invocation.input), 0.0, None, !runs.is_empty());
    Ok((time.basis != v1::Basis::None).then(|| time.at(held_width(record))))
}

/// A root the machine gives its width as it starts: no count asked, and more than one width.
pub(crate) fn chooses(record: &Execution) -> bool {
    let invocation = &record.invocation;
    invocation.parent.is_empty() && invocation.gpus == 0 && invocation.widths.len() > 1
}

/// Its weights' arrival, bytes a second, past the first GiB its downloads brought: a fetch ramps its streams up over that (cl-1107,
/// the user-flow: cibo's first GiB came at 63 MB/s, and it was quoted 933 s for 59 GB; chocola's own came at 68 MB/s, and its 52 GB
/// at 216 MB/s). 0 until a GiB more came.
pub(crate) fn link(service: &Service) -> f64 {
    static RAMPED: std::sync::Mutex<Option<(u64, std::time::Duration)>> = std::sync::Mutex::new(None);
    let Some((bytes, time)) = service.publisher().map(|publisher| publisher.machine().received()) else { return 0.0 };
    if bytes < MEASURED_LINK {
        return 0.0;
    }
    let (ramped_bytes, ramped_time) = *RAMPED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert((bytes, time));
    let (after, took) = (bytes.saturating_sub(ramped_bytes), time.saturating_sub(ramped_time));
    match after >= MEASURED_LINK && !took.is_zero() {
        true => after as f64 / took.as_secs_f64(),
        false => 0.0,
    }
}

/// What its downloads read off its NFS drive (cl-1282), bytes a second, once 1 GiB came; 0 before. None: it has none.
pub(crate) fn drive(service: &Service) -> Option<f64> {
    let (bytes, time) = service.publisher()?.machine().volume_read()?;
    Some(match bytes >= MEASURED_LINK && !time.is_zero() {
        true => bytes as f64 / time.as_secs_f64(),
        false => 0.0,
    })
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

/// When a started run took its GPUs: its claim, else its start.
fn claimed(record: &Execution) -> u64 {
    match record.starting_at_ms {
        0 => record.started_at_ms,
        at => at,
    }
}

/// The bytes of weights a queued run still waits on, as its progress says: of the downloads it waits on, its own still to come
/// (`journal::Waits`, cl-1205), and what its own preparation transfers.
fn fetching(record: &Execution) -> u64 {
    let progress: serde_json::Value = record.progress.as_deref().and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
    let count = |key: &str| progress.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
    count("bytes_left") + count("bytes_total").saturating_sub(count("bytes_done"))
}

/// How far a run is, as its latest progress says: its stage (none for a private run's), and its step of its steps.
fn stepped(record: &Execution, private: bool) -> v1::Queued {
    let progress: serde_json::Value = record.progress.as_deref().and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
    let count = |key: &str| progress.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let stage = match private {
        true => String::new(),
        false => progress.get("stage").and_then(serde_json::Value::as_str).unwrap_or_default().chars().take(120).collect(),
    };
    v1::Queued { stage, step: count("position"), steps: count("total"), ..Default::default() }
}

/// Every root here not ended, every caller's, by acceptance.
pub(crate) fn roots(service: &Service) -> io::Result<Vec<Root>> {
    let engine = &service.engine;
    let gpus = service.pool().map_or(0, |pool| pool.serving_width());
    let downloading = crate::activity::downloading();
    let on_drive = crate::activity::drive_left();
    let mut held: HashMap<String, Vec<Installation>> = HashMap::new();
    let mut roots = vec![];
    let records = engine.nonterminal(usize::MAX)?;
    // Each root's calls not ended, by its id: an orchestrator's GPU work is theirs, and so are the weights it waits for (cl-1107).
    let mut calls: HashMap<&str, Vec<&Execution>> = HashMap::new();
    for call in records.iter().filter(|record| !record.invocation.parent.is_empty()) {
        calls.entry(call.invocation.root.as_str()).or_default().push(call);
    }
    for record in &records {
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
        // An encrypted project's: its function, models and stage are its project's, as `api::calcifer::Run::outsider` rules for every
        // outsider (cl-1242). A plaintext project's are said, as `times` says them.
        let private = invocation.local_private.as_ref().is_some_and(|scope| !scope.ordinary);
        let caller = |record: &Execution| {
            (record.submission.as_ref().map(|s| s.request_id.clone()))
                .filter(|request| !request.is_empty())
                .unwrap_or_else(|| record.id.clone())
        };
        let caller_id = caller(record);
        let mine: &[&Execution] = calls.get(record.id.as_str()).map_or(&[][..], Vec::as_slice);
        // When its work took its GPUs: its own claim; a root on a lane for its calls alone (an orchestrator, which starts on its CPU
        // at once), its first GPU call's claim, ended or not. Until then all its work waits, for its calls' weights or a GPU.
        let lane = running && invocation.degree == 0 && record.width > 0;
        let gpu_at = match (running, lane) {
            (false, _) => 0,
            (true, false) => claimed(record),
            (true, true) => engine.first_gpu_claim(&record.id)?,
        };
        let models = match private {
            true => vec![],
            false => {
                let ids: Vec<String> = std::iter::once(caller_id.clone()).chain(mine.iter().map(|&call| caller(call))).collect();
                let mut models: Vec<String> = ids.iter().filter_map(|id| downloading.get(id)).flatten().cloned().collect();
                models.sort();
                models.dedup();
                models
            }
        };
        let bytes_left = match running {
            // Its own are here; what its calls not started wait for is still to come.
            true => mine.iter().filter(|call| call.state == State::Queued).map(|call| fetching(call)).sum(),
            false => fetching(record),
        };
        // Of them, the NFS drive's share of what the downloads it waits on have left (cl-1282).
        let (left, drive) = models.iter().filter_map(|model| on_drive.get(model)).fold((0, 0), |all: (u64, u64), one| (all.0 + one.0, all.1 + one.1));
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
            width: if running { held_width(record) } else { 0 },
            started_at_unix_ms: record.started_at_ms as i64,
            gpu_at_unix_ms: gpu_at as i64,
            gpu_waits: lane && gpu_at == 0,
            models,
            bytes_left,
            drive_left: (u128::from(bytes_left) * u128::from(drive.min(left)) / u128::from(left.max(1))) as u64,
            client: crate::clients::name(&actor),
            ..stepped(record, private)
        };
        roots.push(Root { id: record.id.clone(), actor, caller_id, queued });
    }
    Ok(roots)
}

/// A waiting root's width as the machine packs its queue now, and about how long until it starts, ms; None: not known, as a run it
/// waits for has never been timed here.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Start {
    pub width: u32,
    pub wait_ms: Option<u64>,
}

/// Each waiting root's start, of those the machine chooses for (`chooses`), from the same picture Calcifer quotes by: a root queued,
/// or an orchestrator whose calls have yet to take its GPUs (cl-1187).
pub(crate) fn widths(service: &Service, pool: &crate::gpu_service::Pool) -> io::Result<HashMap<String, Start>> {
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
        drive: drive(service).unwrap_or(0.0) / 1e3,
        card,
        memory: 0,
        nvlink,
        times,
        now_ms: crate::machine::lifecycle::now_ms(),
        // Its own packing knows no release's benchmarks (cl-1024): what it never ran here is a minute; Calcifer's quotes know them.
        references: None,
        named: HashMap::new(),
    };
    let queued: Vec<v1::Queued> = roots.iter().map(|root| root.queued.clone()).collect();
    let (jobs, _) = machine.queue(&queued);
    let placed = estimate::pack(machine.gpus, &jobs);
    Ok((roots.into_iter().zip(jobs).zip(placed))
        .filter(|((root, job), place)| job.running.is_none() && root.queued.chooses && place.width > 0)
        .map(|((root, _), place)| (root.id, Start { width: place.width, wait_ms: place.known.then_some(place.start_ms.max(0.0) as u64) }))
        .collect())
}
