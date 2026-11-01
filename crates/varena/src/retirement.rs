//! Owned per-device final resources. No event poller or allocator metadata owns these jobs.
//! Async fill adds its typed FillResources variant here; event submission stays separate.
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

pub(crate) enum Job {
    Arena(crate::arena_retirement::Payload),
}

impl Job {
    fn poll(&mut self) -> Result<bool> {
        match self {
            Self::Arena(job) => job.poll(),
        }
    }
    fn failed(&mut self, error: &Error) {
        match self {
            Self::Arena(job) => job.failed(error),
        }
    }
}

struct Endpoint {
    sender: mpsc::Sender<Job>,
    // A disconnected worker never authorizes GPU destruction on a caller's stack.
    // These exact owned resources remain reachable here until process retirement.
    quarantine: Mutex<Vec<Job>>,
}

static QUEUES: OnceLock<Mutex<BTreeMap<i32, Arc<Endpoint>>>> = OnceLock::new();
static ORPHANS: OnceLock<Mutex<Vec<Job>>> = OnceLock::new();

fn endpoint(device: i32) -> Result<Arc<Endpoint>> {
    crate::check_process()?;
    let mut queues = QUEUES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("retirement queue registry mutex".into()))?;
    if let Some(queue) = queues.get(&device) {
        return Ok(queue.clone());
    }
    let (sender, receiver) = mpsc::channel();
    let queue = Arc::new(Endpoint {
        sender,
        quarantine: Mutex::new(Vec::new()),
    });
    let worker = queue.clone();
    std::thread::Builder::new()
        .name(format!("varena-retire-{device}"))
        .spawn(move || run(worker, receiver))
        .map_err(|error| {
            Error::Unsupported(format!("retirement worker could not start: {error}"))
        })?;
    queues.insert(device, queue.clone());
    Ok(queue)
}

/// Prepare before native GPU construction, so even final Drop has an owned destination.
pub(crate) fn prepare(device: i32) -> Result<()> {
    endpoint(device).map(|_| ())
}

pub(crate) fn enqueue(device: i32, job: Job) -> std::result::Result<(), (Error, Job)> {
    let queue = match endpoint(device) {
        Ok(queue) => queue,
        Err(error) => return Err((error, job)),
    };
    queue
        .sender
        .send(job)
        .map_err(|error| (Error::Poisoned("retirement worker stopped".into()), error.0))
}

/// Destructor handoff. The queue was prepared before GPU creation; every error retains
/// its exact job, reports failure, and never recursively drops GPU owners under a lock.
pub(crate) fn defer(device: i32, job: Job) {
    if let Err((error, mut job)) = enqueue(device, job) {
        job.failed(&error);
        // Poison recovery is only for preserving owned jobs, never admitting GPU work.
        let queues = QUEUES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(queue) = queues.get(&device) {
            queue
                .quarantine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(job);
        } else {
            // Even a violated construction contract retains reachable native ownership.
            ORPHANS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(job);
        }
    }
}

fn run(queue: Arc<Endpoint>, receiver: mpsc::Receiver<Job>) {
    let mut pending = Vec::new();
    loop {
        if pending.is_empty() {
            match receiver.recv() {
                Ok(job) => pending.push(job),
                Err(_) => return,
            }
        } else if let Ok(job) = receiver.recv_timeout(Duration::from_millis(20)) {
            pending.push(job);
        }
        pending.extend(receiver.try_iter());
        let mut index = 0;
        while index < pending.len() {
            // Keep owned resources outside the unwind scope. A callback/resource error
            // quarantines only that job; this worker continues servicing other owners.
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pending[index].poll()));
            match result {
                Ok(Ok(false)) => index += 1,
                Ok(Ok(true)) => {
                    drop(pending.swap_remove(index));
                }
                failure => {
                    let error = match failure {
                        Ok(Err(error)) => error,
                        _ => Error::Poisoned("native retirement job panicked".into()),
                    };
                    let mut job = pending.swap_remove(index);
                    job.failed(&error);
                    queue
                        .quarantine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(job);
                }
            }
        }
    }
}
