//! Finalizers transfer ownership here. Only this worker retries temporary refusals.
use super::*;
use std::collections::VecDeque;
use std::sync::Condvar;
use std::time::Duration;

enum Job {
    TensorWeight(Arc<Weight>),
    Handle(u64),
    Arena(Arc<Arena>, ServiceLease),
    ArenaTicket(RetiringArena),
    Source(super::source::Ticket),
    Fill(crate::FillTicket),
}
struct RetiringArena {
    ticket: crate::ArenaRetirement,
    service: ServiceLease,
}
#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    processing: u64,
    arena_tickets: Vec<RetiringArena>,
    refused_arenas: Vec<(Arc<Arena>, ServiceLease)>,
    source_tickets: Vec<super::source::Ticket>,
    fill_tickets: Vec<crate::FillTicket>,
    retired: u64,
    quarantined: u64,
    abandoned_bytes: u64,
}
pub(super) struct Queue {
    state: Mutex<State>,
    changed: Condvar,
}
static QUEUE: OnceLock<std::result::Result<Arc<Queue>, String>> = OnceLock::new();

pub(super) fn start() -> Result<&'static Arc<Queue>> {
    match QUEUE.get_or_init(|| {
        let queue = Arc::new(Queue {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let owner = queue.clone();
        std::thread::Builder::new()
            .name("varena-api-retire".into())
            .spawn(move || run(owner))
            .map_err(|e| e.to_string())?;
        Ok(queue)
    }) {
        Ok(queue) => Ok(queue),
        Err(error) => Err(Failure(
            UNSUPPORTED,
            format!("cannot start retirement worker: {error}"),
            0,
        )),
    }
}

pub(super) fn arena(arena: Arc<Arena>, service: ServiceLease) {
    // Every public Gpu is constructed only after start succeeds. The queue and worker
    // live for the process; no CUDA/source destructor runs while publishing this job.
    let queue = QUEUE
        .get()
        .and_then(|q| q.as_ref().ok())
        .expect("public Gpu retirement queue");
    queue
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .jobs
        .push_back(Job::Arena(arena, service));
    queue.changed.notify_one();
}

fn observe(job: Job) {
    let queue = QUEUE.get().and_then(|queue| queue.as_ref().ok()).expect("public retirement queue");
    queue.state.lock().unwrap_or_else(|error| error.into_inner()).jobs.push_back(job);
    queue.changed.notify_one();
}
pub(super) fn source(ticket: super::source::Ticket) { observe(Job::Source(ticket)); }
pub(super) fn fill(ticket: crate::FillTicket) { observe(Job::Fill(ticket)); }
// Storage deleters publish exact ownership; they never run final Region/Source/Gpu
// teardown inline. The existing worker performs the ordinary native finalizers.
pub(super) fn tensor_weight(weight: Arc<Weight>) { observe(Job::TensorWeight(weight)); }

pub(super) fn defer(id: u64) -> Result<()> {
    crate::check_process()?; // reject fork before inherited queue/registry locks
    let queue = start()?;
    let owner = {
        let mut handles = registry_unchecked()?;
        let entry = handles
            .get_mut(&id)
            .ok_or_else(|| invalid("invalid or closed handle"))?;
        if entry.disposition != Disposition::Owned {
            return Err(invalid("handle already transferred"));
        }
        let owner = if let Handle::Cache(cache) = &entry.value {
            lock(&cache.owner)?.clone()
        } else {
            None
        };
        entry.disposition = Disposition::Queued;
        owner
    };
    if let Some(owner) = owner {
        // Stop new callbacks before the ownership-transfer call returns. Existing
        // callbacks retain only the dispatcher-identified release/read borrow.
        // No ABI registry lock is held while requesting native owner retirement.
        if let Err(error) = owner.defer_close() {
            let error = Failure::from(error);
            if error.0 == FAULT {
                ABI_FAULT.store(true, Ordering::Release);
            }
            // Ownership has transferred; the queued job records/quarantines a
            // permanent failure rather than handing an unreachable owner back.
        }
    }
    queue
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .jobs
        .push_back(Job::Handle(id));
    queue.changed.notify_one();
    Ok(())
}

enum Outcome {
    Done,
    Retry(Job),
    Quarantined,
}
fn handle(id: u64, queue: &Queue) -> Outcome {
    // close_handle retains the registry owner on every refusal. Cache callback drain
    // is polled here, never waited on, so one busy callback cannot stall other jobs.
    match close_handle(id, true) {
        Ok(()) => Outcome::Done,
        Err(Failure(CAPTURING | BUSY, _, _)) => Outcome::Retry(Job::Handle(id)),
        Err(error) => {
            if error.0 == FAULT {
                ABI_FAULT.store(true, Ordering::Release);
            }
            let cache = if let Ok(mut handles) = registry_unchecked()
                && let Some(entry) = handles.get_mut(&id)
            {
                entry.disposition = Disposition::Quarantined;
                entry.failure = error.0;
                if let Handle::Cache(cache) = &entry.value {
                    Some(cache.clone())
                } else {
                    None
                }
            } else {
                None
            };
            let abandoned = cache
                .and_then(|c| c.storage.stats().ok())
                .map_or(0, |s| s.live_bytes as u64);
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            state.abandoned_bytes = state.abandoned_bytes.saturating_add(abandoned);
            Outcome::Quarantined
        }
    }
}

fn retire_arena(arena: Arc<Arena>, service: ServiceLease, queue: &Queue) -> Outcome {
    // The native job detaches persistent observer roots and preserves genuine owners.
    // The ABI worker never polls Arc counts, waits for GPU work or owns final GPU State.
    // Current-process handoff failure is retained in the native quarantine, not dropped.
    match arena.defer_retirement() {
        Ok(ticket) => observe_arena(RetiringArena { ticket, service }, queue),
        Err((arena, error)) => {
            let error = Failure::from(error);
            if matches!(error.0, CAPTURING | BUSY) {
                return Outcome::Retry(Job::Arena(arena, service));
            }
            if error.0 == FAULT {
                ABI_FAULT.store(true, Ordering::Release);
            }
            queue
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .refused_arenas
                .push((arena, service));
            Outcome::Quarantined
        }
    }
}

fn observe_arena(retiring: RetiringArena, queue: &Queue) -> Outcome {
    // Preserve service accounting until physical Arena completion. Closing the client
    // then polls only its original host Control; this worker never waits on a service.
    let result = retiring
        .ticket
        .try_complete()
        .map_err(Failure::from)
        .and_then(|done| {
            if done {
                retiring.service.try_close()
            } else {
                Ok(false)
            }
        });
    match result {
        Ok(true) => Outcome::Done,
        Ok(false) | Err(Failure(CAPTURING | BUSY, _, _)) => {
            Outcome::Retry(Job::ArenaTicket(retiring))
        }
        Err(error) => {
            if error.0 == FAULT {
                ABI_FAULT.store(true, Ordering::Release);
            }
            // The native queue already retains the exact GPU owners. Keep the host
            // ticket for its failed/unproven result, never infer released bytes here.
            queue
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .arena_tickets
                .push(retiring);
            Outcome::Quarantined
        }
    }
}

fn observe_source(ticket: super::source::Ticket, queue: &Queue) -> Outcome {
    match ticket.try_complete() {
        Ok(true) => Outcome::Done,
        Ok(false) => Outcome::Retry(Job::Source(ticket)),
        Err(error) => {
            if Failure::from(error).0 == FAULT { ABI_FAULT.store(true, Ordering::Release); }
            queue.state.lock().unwrap_or_else(|e| e.into_inner()).source_tickets.push(ticket);
            Outcome::Quarantined
        }
    }
}
fn observe_fill(ticket: crate::FillTicket, queue: &Queue) -> Outcome {
    match ticket.facts() {
        Ok(facts) if facts.state == crate::FillState::Quarantined => {
            ABI_FAULT.store(true, Ordering::Release);
            queue.state.lock().unwrap_or_else(|e| e.into_inner()).fill_tickets.push(ticket);
            Outcome::Quarantined
        }
        Ok(facts) if facts.retirement_finished => Outcome::Done,
        Ok(_) => Outcome::Retry(Job::Fill(ticket)),
        Err(_) => {
            ABI_FAULT.store(true, Ordering::Release);
            queue.state.lock().unwrap_or_else(|e| e.into_inner()).fill_tickets.push(ticket);
            Outcome::Quarantined
        }
    }
}

fn run(queue: Arc<Queue>) {
    loop {
        let jobs = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.jobs.is_empty() {
                state = queue.changed.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            state.processing = state.jobs.len() as u64;
            std::mem::take(&mut state.jobs)
        };
        let mut retry = VecDeque::new();
        let (mut retired, mut quarantined) = (0, 0);
        for job in jobs {
            let outcome = match job {
                Job::TensorWeight(weight) => { drop(weight); Outcome::Done }
                Job::Handle(id) => handle(id, &queue),
                Job::Arena(arena, service) => retire_arena(arena, service, &queue),
                Job::ArenaTicket(ticket) => observe_arena(ticket, &queue),
                Job::Source(ticket) => observe_source(ticket, &queue),
                Job::Fill(ticket) => observe_fill(ticket, &queue),
            };
            match outcome {
                Outcome::Done => retired += 1,
                Outcome::Quarantined => quarantined += 1,
                Outcome::Retry(job) => retry.push_back(job),
            }
        }
        let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
        state.retired += retired;
        state.processing = 0;
        state.quarantined += quarantined;
        state.jobs.extend(retry);
        if !state.jobs.is_empty() {
            // A poll cadence for submitted events/capture counters, never a failure timeout.
            drop(
                queue
                    .changed
                    .wait_timeout(state, Duration::from_millis(20))
                    .unwrap_or_else(|e| e.into_inner()),
            );
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub queued: u64,
    pub retired: u64,
    pub quarantined: u64,
    pub abandoned_cache_bytes: u64,
}
pub(super) fn stats() -> Result<Stats> {
    crate::check_process()?;
    let Some(Ok(queue)) = QUEUE.get() else {
        return Ok(Stats::default());
    };
    let state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Stats {
        queued: state.jobs.len() as u64 + state.processing,
        retired: state.retired,
        quarantined: state.quarantined,
        abandoned_cache_bytes: state.abandoned_bytes,
    })
}
