//! Final native releases must not wait under the GIL, or on a mover that a release joins.
//! Queue state is process-owned: Runtime imports TensorFS before its CPU-only parent forks.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};
use std::thread::JoinHandle;

use pyo3::prelude::*;

type Task = Box<dyn FnOnce() + Send + 'static>;
struct State {
    pid: u32,
    sender: mpsc::Sender<Task>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

thread_local! {
    /// Set on a worker by `release`: it ends after the task that set it.
    static STOP: Cell<bool> = const { Cell::new(false) };
}
struct Pending {
    pid: u32,
    task: Option<Task>,
    next: *mut Pending,
}

// Published states remain valid for the process lifetime. A forked child must not touch or
// destroy an inherited channel: an absent parent thread might have held one of its locks.
static STATE: AtomicPtr<State> = AtomicPtr::new(std::ptr::null_mut());
static PENDING: AtomicPtr<Pending> = AtomicPtr::new(std::ptr::null_mut());

fn state() -> Result<&'static State, std::io::Error> {
    let pid = std::process::id();
    loop {
        let old = STATE.load(Ordering::Acquire);
        // SAFETY: published boxes are never freed; pid is immutable after publication.
        if !old.is_null() && unsafe { (*old).pid == pid } {
            // SAFETY: as above.
            return Ok(unsafe { &*old });
        }
        let (sender, receiver) = mpsc::channel::<Task>();
        let worker = std::thread::Builder::new()
            .name("tensorfs-cleanup".into())
            .spawn(move || {
                for task in receiver {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        task();
                        // Native waits/locks have finished. Flush PyO3's deferred refs so
                        // provider weakrefs can observe release without another API call.
                        flush();
                    }));
                    if STOP.get() {
                        return;
                    }
                }
            })?;
        let fresh = Box::into_raw(Box::new(State { pid, sender, worker: Mutex::new(Some(worker)) }));
        match STATE.compare_exchange(old, fresh, Ordering::AcqRel, Ordering::Acquire) {
            // SAFETY: `fresh` was just published; published states are never freed.
            Ok(_) => return Ok(unsafe { &*fresh }),
            // SAFETY: this fresh, unpublished state belongs to this process. Closing its
            // unused sender ends its fresh worker; it is not an inherited channel.
            Err(_) => drop(unsafe { Box::from_raw(fresh) }),
        }
    }
}

// The worker attaches only until the interpreter starts to finalize: attaching while it tears
// down crashed CPython 3.14 at exit. An atexit hook ends the attaches and waits one out;
// `ATTACHING` names the process whose worker is attaching, so a fork never waits on a parent's.
static FINAL: AtomicBool = AtomicBool::new(false);
static ATTACHING: AtomicU32 = AtomicU32::new(0);

struct Attaching;
impl Drop for Attaching {
    fn drop(&mut self) {
        ATTACHING.store(0, Ordering::SeqCst);
    }
}

fn flush() {
    ATTACHING.store(std::process::id(), Ordering::SeqCst);
    let _attaching = Attaching;
    if !FINAL.load(Ordering::SeqCst) {
        pyo3::Python::try_attach(|_| {});
    }
}

#[pyfunction]
fn finalizing(py: Python<'_>) {
    FINAL.store(true, Ordering::SeqCst);
    let pid = std::process::id();
    py.detach(|| {
        while ATTACHING.load(Ordering::SeqCst) == pid {
            std::thread::yield_now();
        }
    });
}

/// Registers the atexit hook that ends the worker's attaches.
pub(crate) fn install(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let hook = wrap_pyfunction!(finalizing, m)?;
    m.py().import("atexit")?.call_method1("register", (hook,))?;
    Ok(())
}

fn retain(task: Task) {
    let entry = Box::into_raw(Box::new(Pending {
        pid: std::process::id(),
        task: Some(task),
        next: std::ptr::null_mut(),
    }));
    let mut head = PENDING.load(Ordering::Acquire);
    loop {
        // SAFETY: entry is private until publication. Push never dereferences the old
        // head, so a concurrent drain/reuse of that address cannot produce an ABA read.
        unsafe { (*entry).next = head };
        match PENDING.compare_exchange_weak(head, entry, Ordering::Release, Ordering::Acquire) {
            Ok(_) => break,
            Err(current) => head = current,
        }
    }
}

fn send(state: &State, task: Task) {
    if let Err(error) = state.sender.send(task) {
        // Invalidate a failed current-process receiver without destroying its channel.
        let _ = STATE.compare_exchange(
            std::ptr::from_ref(state).cast_mut(),
            std::ptr::null_mut(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        retain(error.0);
    }
}

fn retry(state: &State) {
    let mut head = PENDING.swap(std::ptr::null_mut(), Ordering::AcqRel);
    let mut tasks = Vec::new();
    while !head.is_null() {
        // SAFETY: swap detached this entire list; push only links a new private node.
        let mut entry = unsafe { Box::from_raw(head) };
        head = entry.next;
        if entry.pid == state.pid {
            if let Some(task) = entry.task.take() {
                tasks.push(task);
            }
        } else {
            // These are another process's in-flight releases, not child-owned work. Its
            // copied payload may contain vanished thread handles or locked mutexes.
            // Leave that copy untouched; the parent still releases its own real payload.
            std::mem::forget(entry);
        }
    }
    for task in tasks.into_iter().rev() {
        send(state, task);
    }
}

fn submit(task: Task, queue: std::io::Result<&State>) {
    match queue {
        Ok(state) => {
            retry(state);
            send(state, task);
        }
        Err(_) => {
            retain(task);
            // Do not enter a process-global logger from a post-fork destructor: its
            // lock may also belong to a vanished parent thread. A later defer retries.
        }
    }
}

/// Ends this process's worker once it has run what was sent before, and joins it; the next
/// `defer` starts another. Call it without the GIL (a task's flush attaches). The state stays
/// published-and-never-freed: a sender still holding it only fails over to `retain`.
pub(crate) fn release() {
    let pid = std::process::id();
    let old = STATE.load(Ordering::Acquire);
    // SAFETY: published boxes are never freed; pid is immutable after publication.
    if old.is_null() || unsafe { (*old).pid } != pid {
        return;
    }
    if STATE.compare_exchange(old, std::ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    // SAFETY: as above; this process published it.
    let state = unsafe { &*old };
    if state.sender.send(Box::new(|| STOP.set(true))).is_ok() {
        let worker = state.worker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

/// Move the actual final owner. Never join, acquire Python, or drop the payload on the
/// calling thread. Thread-start/send failure preserves it for the next successful init/defer.
pub(crate) fn defer<T: Send + 'static>(value: T) {
    submit(Box::new(move || drop(value)), state());
}
