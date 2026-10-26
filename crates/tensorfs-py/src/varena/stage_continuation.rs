//! A deferred source+graph stage exported through the existing continuation.v1 ABI.
//! Runtime's sequence worker calls start outside its locks. That same worker performs
//! native preparation/read/compute; no Python callback or additional scheduler is used.
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use pyo3::{
    prelude::*,
    types::{PyCapsule, PyDict},
};

use super::continuation::{self, Continuation, Progress, Status};
use super::refill::{Failure, PreparedRead, StageSource};

type Op = unsafe extern "C" fn(*mut c_void) -> Status;
type Observe = unsafe extern "C" fn(*mut c_void, *mut Progress) -> Status;
type StartAfter = unsafe extern "C" fn(*mut c_void, *const usize, u32, u64) -> Status;

#[repr(C)]
struct Api {
    version: u32,
    size: u32,
    flags: u64,
    device: i32,
    reserved: u32,
    pid: u64,
    context: *mut c_void,
    retain: Op,
    release: Op,
    start: Op,
    poll: Observe,
    cancel: Op,
    wait: Observe,
    start_after: Option<StartAfter>,
}
// SAFETY: the provider owns the opaque context and all operations use the checked ABI.
unsafe impl Send for Api {}

#[repr(C)]
struct Capsule {
    api: Api,
    _owner: Arc<Stage>,
}

struct Stage {
    source: Arc<StageSource>,
    graph: Arc<Continuation>,
    module: Arc<Py<PyAny>>,
    pid: u32,
    epoch: u64,
    event_ordered: bool,
    gate: AtomicU8, // 0 armed, 1 start committed, 2 canceled before start
    canceled: AtomicBool,
    retained_unsafe: AtomicBool,
    read: OnceLock<Arc<PreparedRead>>,
    result: Mutex<Option<(Status, Progress)>>,
    changed: Condvar,
}

fn ok() -> Status {
    Status::new(continuation::OK, 0, "")
}

fn failure(error: &Failure) -> Status {
    match error {
        Failure::Canceled => Status::new(continuation::CANCELED, 0, &error.detail()),
        Failure::Native(varena::Error::OutOfMemory { .. }) => {
            Status::new(continuation::CAPACITY, 2, &error.detail())
        }
        Failure::Native(
            varena::Error::Cuda { code, .. } | varena::Error::DeviceFault { code, .. },
        ) => Status::new(continuation::CUDA, *code, &error.detail()),
        Failure::Native(varena::Error::Poisoned(_)) => Status::unproven(&error.detail()),
        Failure::Execution(status) => **status,
        // Host source errors remain request errors after checked drain, not device faults.
        _ => Status::new(continuation::INVALID, 0, &error.detail()),
    }
}

impl Stage {
    fn publish(&self, status: Status, mut progress: Progress, complete: bool) -> Status {
        progress.epoch = self.epoch;
        progress.done = u32::from(complete);
        progress.phase = if complete { 2 } else { 3 };
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        if result.is_none() {
            *result = Some((status, progress));
        }
        self.changed.notify_all();
        status
    }

    fn abort_before_read(&self, status: Status) -> Status {
        match self.graph.abort() {
            Ok(()) => {
                let (_, mut progress) = self.graph.observe(false);
                progress.canceled |= u32::from(self.canceled.load(Ordering::Acquire));
                let complete = !matches!(
                    status.kind,
                    continuation::CUDA | continuation::INTERNAL | continuation::UNPROVEN
                );
                self.publish(status, progress, complete)
            }
            Err(error) => self.publish(error, Progress::default(), false),
        }
    }

    fn start(&self) -> Status {
        if let Err(gate) = self
            .gate
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            return Status::new(
                if gate == 2 {
                    continuation::CANCELED
                } else {
                    continuation::INVALID
                },
                0,
                "source continuation is one-shot or canceled before start",
            );
        }
        if self.canceled.load(Ordering::Acquire) {
            return self.abort_before_read(Status::new(
                continuation::CANCELED,
                0,
                "source stage canceled",
            ));
        }
        let work = match self.source.prepare() {
            Ok(work) => work,
            Err(error) => return self.abort_before_read(failure(&error)),
        };
        let read = match PreparedRead::composed(work, self.graph.clone(), self.event_ordered) {
            Ok(read) => read,
            Err(error) => return self.abort_before_read(failure(&error)),
        };
        let read = Arc::new(read);
        let _ = self.read.set(read.clone());
        if self.canceled.load(Ordering::Acquire) {
            read.request_cancel();
        }
        // Runs on the caller's native sequence worker. Cancellation uses only atomics and
        // the published weak read control, so it never waits behind this source/graph work.
        let (result, completion_proven) = read.run_inline();
        let (observed, progress) = self.graph.observe(false);
        let status = match result {
            Ok(_) => observed,
            Err(error) => failure(&error),
        };
        let complete = completion_proven
            && progress.done != 0
            && matches!(
                status.kind,
                continuation::OK
                    | continuation::CANCELED
                    | continuation::INVALID
                    | continuation::CAPACITY
            );
        self.publish(
            if complete {
                status
            } else {
                Status::unproven(&status.detail())
            },
            progress,
            complete,
        )
    }

    fn cancel(&self) -> Status {
        self.canceled.store(true, Ordering::Release);
        let _ = self
            .gate
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
        let status = self.graph.cancel();
        if let Some(read) = self.read.get() {
            read.request_cancel();
        }
        status
    }

    fn observe(&self, wait: bool) -> (Status, Progress) {
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(value) = *result {
                return value;
            }
            match self.gate.load(Ordering::Acquire) {
                0 => {
                    return (
                        ok(),
                        Progress {
                            epoch: self.epoch,
                            ..Progress::default()
                        },
                    );
                }
                2 => {
                    drop(result);
                    // The inner graph's atomic cancel gate prevents launch. Its poll only
                    // retires the armed epoch; no source or CUDA consumer was submitted.
                    let (status, mut progress) = self.graph.observe(wait);
                    progress.canceled = 1;
                    let complete = self.graph.checked(status, progress);
                    let status = if complete {
                        Status::new(
                            continuation::CANCELED,
                            0,
                            "source stage canceled before start",
                        )
                    } else if wait {
                        Status::unproven(&status.detail())
                    } else {
                        status
                    };
                    if complete || wait {
                        self.publish(status, progress, complete);
                    }
                    return (
                        status,
                        Progress {
                            epoch: self.epoch,
                            ..progress
                        },
                    );
                }
                _ if !wait => {
                    return (
                        ok(),
                        Progress {
                            phase: 1,
                            canceled: u32::from(self.canceled.load(Ordering::Acquire)),
                            epoch: self.epoch,
                            ..Progress::default()
                        },
                    );
                }
                _ => result = self.changed.wait(result).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }
}

struct Retire(Option<(Arc<StageSource>, Arc<Continuation>, Arc<Py<PyAny>>)>);
impl Drop for Retire {
    fn drop(&mut self) {
        if let Some(owners) = self.0.take() {
            if owners.1.abort().is_err() {
                std::mem::forget(owners);
            }
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let owners = (self.source.clone(), self.graph.clone(), self.module.clone());
        if self.pid != std::process::id() {
            std::mem::forget(owners);
        } else {
            self.cancel();
            crate::cleanup::defer(Retire(Some(owners)));
        }
    }
}

fn boundary(context: *mut c_void, call: impl FnOnce(&Stage) -> Status) -> Status {
    if context.is_null() {
        return Status::new(continuation::INVALID, 0, "null source continuation");
    }
    // SAFETY: the same-process capability contract lends a retained provider context.
    let stage = unsafe { &*context.cast::<Stage>() };
    if stage.pid != std::process::id() {
        return Status::new(
            continuation::INVALID,
            0,
            "source continuation used after fork",
        );
    }
    let status = match catch_unwind(AssertUnwindSafe(|| call(stage))) {
        Ok(status) => status,
        Err(_) => {
            let status = Status::new(continuation::INTERNAL, 999, "source continuation panicked");
            stage.publish(status, Progress::default(), false);
            status
        }
    };
    if matches!(
        status.kind,
        continuation::CUDA | continuation::INTERNAL | continuation::UNPROVEN
    ) && !stage.retained_unsafe.swap(true, Ordering::AcqRel)
    {
        // SAFETY: retain the actual descriptor, graph and provider module to process exit
        // when cleanup is unproven; never leave only a pointer or a diagnostic flag.
        unsafe {
            Arc::increment_strong_count(context.cast::<Stage>());
        }
    }
    status
}

unsafe extern "C" fn retain(context: *mut c_void) -> Status {
    boundary(context, |_| {
        // SAFETY: capsule or prior successful retain keeps this provider Arc alive.
        unsafe {
            Arc::increment_strong_count(context.cast::<Stage>());
        }
        ok()
    })
}
unsafe extern "C" fn release(context: *mut c_void) -> Status {
    if context.is_null() {
        return Status::new(continuation::INVALID, 0, "null source continuation");
    }
    // SAFETY: one previous retain keeps the context valid through the pid check.
    if unsafe { (&*context.cast::<Stage>()).pid } != std::process::id() {
        return Status::new(
            continuation::INVALID,
            0,
            "source continuation released after fork",
        );
    }
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: consume exactly one successful retain in its allocating provider.
        unsafe {
            drop(Arc::from_raw(context.cast::<Stage>()));
        }
    })) {
        Ok(()) => ok(),
        Err(_) => Status::new(
            continuation::INTERNAL,
            999,
            "source continuation release panicked",
        ),
    }
}
unsafe extern "C" fn start(context: *mut c_void) -> Status {
    boundary(context, Stage::start)
}
unsafe extern "C" fn cancel(context: *mut c_void) -> Status {
    boundary(context, Stage::cancel)
}
fn observe(context: *mut c_void, out: *mut Progress, wait: bool) -> Status {
    if out.is_null() {
        return Status::new(continuation::INVALID, 0, "null source progress");
    }
    boundary(context, |stage| {
        let (status, progress) = stage.observe(wait);
        // SAFETY: importer provides one writable ABI-sized progress output.
        unsafe {
            out.write(progress);
        }
        status
    })
}
unsafe extern "C" fn poll(context: *mut c_void, out: *mut Progress) -> Status {
    observe(context, out, false)
}
unsafe extern "C" fn wait(context: *mut c_void, out: *mut Progress) -> Status {
    observe(context, out, true)
}

/// One submitted stage's observation owner, never a descriptor-global last result.
#[pyclass(name = "_VarenaStageJob", module = "tensorfs._ext", frozen)]
pub(super) struct StageJob {
    owner: Arc<Stage>,
}
#[pymethods]
impl StageJob {
    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        if self.owner.pid != std::process::id() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "stage observer used after fork",
            ));
        }
        if let Some(read) = self.owner.read.get() {
            return read.facts(py);
        }
        let result = *self.owner.result.lock().unwrap_or_else(|e| e.into_inner());
        let row = PyDict::new(py);
        row.set_item("started", self.owner.gate.load(Ordering::Acquire) == 1)?;
        row.set_item("done", result.is_some())?;
        row.set_item("read_done", false)?;
        row.set_item("read_bytes", 0)?;
        row.set_item("input_read_bytes", Vec::<u64>::new())?;
        row.set_item(
            "completion_proven",
            result.is_some_and(|(_, progress)| progress.done != 0),
        )?;
        row.set_item("canceled", self.owner.canceled.load(Ordering::Acquire))?;
        row.set_item(
            "failed",
            result.is_some_and(|(status, _)| {
                !matches!(status.kind, continuation::OK | continuation::CANCELED)
            }),
        )?;
        Ok(row)
    }
}

pub(super) fn export(
    py: Python<'_>,
    source: Arc<StageSource>,
    graph: &Bound<'_, PyAny>,
    event_ordered: bool,
) -> PyResult<(Py<PyAny>, Py<StageJob>)> {
    // Take the provider module before importing/arming ownership so Python allocation
    // failure cannot strand a retained graph epoch.
    let module = Arc::new(py.import("tensorfs._ext")?.into_any().unbind());
    let graph = Arc::new(Continuation::import(graph, source.device())?);
    if event_ordered && !graph.supports_event_input() {
        graph.abandon();
        return Err(pyo3::exceptions::PyValueError::new_err(
            "stage graph lacks event input",
        ));
    }
    let (status, progress) = py.detach(|| graph.observe(false));
    if status.kind != continuation::OK || progress.phase != 0 || progress.done != 0 {
        graph.abandon();
        return Err(pyo3::exceptions::PyValueError::new_err(
            "source stage needs a fresh armed graph",
        ));
    }
    let owner = Arc::new(Stage {
        source,
        graph,
        module,
        pid: std::process::id(),
        epoch: progress.epoch,
        event_ordered,
        gate: AtomicU8::new(0),
        canceled: AtomicBool::new(false),
        retained_unsafe: AtomicBool::new(false),
        read: OnceLock::new(),
        result: Mutex::new(None),
        changed: Condvar::new(),
    });
    let api = Api {
        version: 1,
        size: std::mem::size_of::<Api>() as u32,
        flags: 7,
        device: owner.source.device(),
        reserved: 0,
        pid: u64::from(owner.pid),
        context: Arc::as_ptr(&owner) as *mut c_void,
        retain,
        release,
        start,
        poll,
        cancel,
        wait,
        start_after: None,
    };
    let observer = Py::new(
        py,
        StageJob {
            owner: owner.clone(),
        },
    )?;
    let capsule = PyCapsule::new_with_value(
        py,
        Capsule { api, _owner: owner },
        c"cozy.execution.continuation.v1",
    )?
    .into_any()
    .unbind();
    Ok((capsule, observer))
}

const _: () = assert!(std::mem::size_of::<Api>() == 96);
const _: () = assert!(std::mem::size_of::<Status>() == 256);
const _: () = assert!(std::mem::size_of::<Progress>() == 24);
