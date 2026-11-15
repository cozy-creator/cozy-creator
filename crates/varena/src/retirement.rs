//! One retirer for the process. An owner that may not be destroyed on an arbitrary stack (GIL
//! held, capture active, a finalizer) comes here and polls until its work is done. A failed job
//! stays here forever: nothing unproven is ever destroyed.
use crate::abi;
use crate::{Error, Pin, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

pub(crate) enum Job {
    Fill(crate::async_fill::Payload),
    Source(abi::source::Retirement),
    Pin { _pin: Pin, failure: Error },
    Handle(u64),
    Owner(#[allow(dead_code)] abi::Owned),
}

impl Job {
    fn poll(&mut self) -> Result<bool> {
        match self {
            Self::Fill(job) => job.retire(),
            Self::Source(job) => job.poll(),
            Self::Pin { failure, .. } => Err(failure.clone()),
            Self::Handle(id) => abi::retire_handle(*id),
            Self::Owner(_) => Ok(true),
        }
    }
    fn failed(&mut self, error: &Error) {
        abi::note_failure(error);
        match self {
            Self::Fill(job) => job.retirement_failed(error),
            Self::Pin { failure, .. } => *failure = error.clone(),
            Self::Handle(id) => abi::handle_failed(*id, error),
            _ => {}
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

struct Retirer {
    sender: mpsc::Sender<Job>,
    quarantine: Mutex<Vec<Job>>,
    queued: AtomicU64,
    retired: AtomicU64,
    abandoned: AtomicU64,
}

static RETIRER: OnceLock<std::result::Result<Arc<Retirer>, String>> = OnceLock::new();

fn retirer() -> Result<&'static Arc<Retirer>> {
    crate::check_process()?;
    let started = RETIRER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        let retirer = Arc::new(Retirer {
            sender,
            quarantine: Mutex::default(),
            queued: AtomicU64::new(0),
            retired: AtomicU64::new(0),
            abandoned: AtomicU64::new(0),
        });
        let worker = retirer.clone();
        std::thread::Builder::new()
            .name("varena-retire".into())
            .spawn(move || run(worker, receiver))
            .map_err(|error| error.to_string())?;
        Ok(retirer)
    });
    started.as_ref().map_err(|error| Error::Unsupported(format!("retirer could not start: {error}")))
}

/// Start before any GPU owner exists, so even a final Drop has a destination.
pub(crate) fn prepare() -> Result<()> {
    retirer().map(|_| ())
}

/// Destructor handoff: never drops a GPU owner on the caller's stack, on any path.
pub(crate) fn defer(job: Job) {
    let Some(Ok(retirer)) = RETIRER.get() else {
        std::mem::forget(job); // No retirer ever started: an inherited owner after fork.
        return;
    };
    retirer.queued.fetch_add(1, Ordering::Relaxed);
    if let Err(error) = retirer.sender.send(job) {
        quarantine(error.0, &Error::Poisoned("retirer stopped".into()));
    }
}

pub(crate) fn quarantine(mut job: Job, error: &Error) {
    job.failed(error);
    match RETIRER.get() {
        Some(Ok(retirer)) => retirer.quarantine.lock().unwrap_or_else(|e| e.into_inner()).push(job),
        _ => std::mem::forget(job),
    }
}

pub(crate) fn abandoned(bytes: u64) {
    if let Some(Ok(retirer)) = RETIRER.get() {
        retirer.abandoned.fetch_add(bytes, Ordering::Relaxed);
    }
}

pub(crate) fn stats() -> Result<Stats> {
    crate::check_process()?;
    let Some(Ok(retirer)) = RETIRER.get() else {
        return Ok(Stats::default());
    };
    let quarantined = retirer.quarantine.lock().unwrap_or_else(|e| e.into_inner()).len() as u64;
    Ok(Stats {
        queued: retirer.queued.load(Ordering::Relaxed),
        retired: retirer.retired.load(Ordering::Relaxed),
        quarantined,
        abandoned_cache_bytes: retirer.abandoned.load(Ordering::Relaxed),
    })
}

fn run(retirer: Arc<Retirer>, receiver: mpsc::Receiver<Job>) {
    let mut pending = Vec::new();
    // A read's demand waits on its fill's retirement, which is usually microseconds behind the first
    // look: the fill's events are done, the read's own end event not quite. At 20 ms each, 124 blocks
    // of SDXL took 2 s of its first step (run 29637). Look again at once, easing off to 12.8 ms. No one
    // event ends them all: GPU work, a capture, another thread's lock.
    let mut quiet = 0u32;
    loop {
        if pending.is_empty() {
            match receiver.recv() {
                Ok(job) => {
                    pending.push(job);
                    quiet = 0;
                }
                Err(_) => return,
            }
        } else {
            match receiver.recv_timeout(Duration::from_micros(100 << quiet.min(7))) {
                Ok(job) => {
                    pending.push(job);
                    quiet = 0;
                }
                Err(_) => quiet += 1,
            }
        }
        pending.extend(receiver.try_iter());
        let mut index = 0;
        while index < pending.len() {
            // Owners stay outside the unwind scope: a panic quarantines only this job.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pending[index].poll()));
            match result {
                Ok(Ok(false)) => index += 1,
                Ok(Ok(true)) => {
                    drop(pending.swap_remove(index));
                    retirer.queued.fetch_sub(1, Ordering::Relaxed);
                    retirer.retired.fetch_add(1, Ordering::Relaxed);
                    quiet = 0;
                }
                failure => {
                    let error = match failure {
                        Ok(Err(error)) => error,
                        _ => Error::Poisoned("retirement job panicked".into()),
                    };
                    retirer.queued.fetch_sub(1, Ordering::Relaxed);
                    quarantine(pending.swap_remove(index), &error);
                }
            }
        }
    }
}
