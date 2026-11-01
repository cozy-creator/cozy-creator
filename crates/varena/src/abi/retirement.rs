//! Finalizers transfer ownership here. Only this worker retries temporary refusals.
use super::*;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::Condvar;
use std::time::Duration;

enum Job {
    Handle(u64),
    Arena(Arc<Arena>),
}
#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    processing: u64,
    arenas: Vec<ManuallyDrop<Arena>>,
    refused_arenas: Vec<Arc<Arena>>,
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

pub(super) fn arena(arena: Arc<Arena>) {
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
        .push_back(Job::Arena(arena));
    queue.changed.notify_one();
}

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

fn retire_arena(arena: Arc<Arena>, queue: &Queue) -> Outcome {
    let exclusion = match process()
        .and_then(|()| crate::enter_retirement(arena.device()).map_err(Into::into))
    {
        Ok(guard) => guard,
        Err(Failure(CAPTURING | BUSY, _, _)) => return Outcome::Retry(Job::Arena(arena)),
        Err(_) => {
            queue
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .refused_arenas
                .push(arena);
            return Outcome::Quarantined;
        }
    };
    // Atomic against Weak::upgrade by native arena snapshots. A last snapshot may
    // disappear later, but this queue continues retaining the Arena until then.
    let mut arena = match Arc::try_unwrap(arena) {
        Ok(arena) => ManuallyDrop::new(arena),
        Err(arena) => return Outcome::Retry(Job::Arena(arena)),
    };
    if let Err(error) = arena.finish_retirement() {
        if Failure::from(error).0 == FAULT {
            ABI_FAULT.store(true, Ordering::Release);
        }
        // A sticky driver fault retains the remaining exact handles; it is never retried.
        queue
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .arenas
            .push(arena);
        return Outcome::Quarantined;
    }
    // finish_retirement emptied the entries. Keep capture/replay excluded through
    // destruction, including the driver's and native Plan registry's final ownership.
    unsafe {
        ManuallyDrop::drop(&mut arena);
    }
    drop(exclusion);
    Outcome::Done
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
                Job::Handle(id) => handle(id, &queue),
                Job::Arena(arena) => retire_arena(arena, &queue),
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
