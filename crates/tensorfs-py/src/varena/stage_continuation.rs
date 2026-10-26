//! A deferred source+graph stage exported through the existing continuation.v1 ABI.
//! Runtime's sequence worker calls start outside its locks. That same worker performs
//! native preparation/read/compute; no Python callback or additional scheduler is used.
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

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
    rearm: Option<Op>,
    finish: Option<Op>,
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
    epoch: AtomicU64,
    event_ordered: bool,
    reusable: bool,
    finished: AtomicBool,
    passes_started: AtomicU64,
    read_control: Arc<tensorfs::io::ReadControl>,
    gate: AtomicU8, // 0 armed, 1 committed/done, 2 canceled, 3 rearming, 4 finishing, 5 finished
    canceled: AtomicBool,
    retained_unsafe: AtomicBool,
    reads: Mutex<ReadLedger>,
    next: Mutex<Option<(Weak<Stage>, usize)>>,
    prefetch: Mutex<Prefetch>,
    prefetch_changed: Condvar,
    prefetch_control: Arc<tensorfs::io::ReadControl>,
    prefetch_totals: Mutex<ReadTotals>,
    prefetch_started: AtomicU64,
    prefetch_skipped: AtomicU64,
    prefetch_used: AtomicU64,
    prefetch_last_clock: Mutex<(u64, u64)>,
    result: Mutex<Option<(Status, Progress)>>,
    changed: Condvar,
}

#[derive(Default)]
struct Prefetch {
    preparing: bool,
    closed: bool,
    pass: u64,
    read: Option<Arc<PreparedRead>>,
    error: Option<Failure>,
}

#[derive(Clone, Default)]
struct ReadTotals {
    bytes: u64,
    input_bytes: Vec<u64>,
    completed_reads: u64,
}
#[derive(Default)]
struct ReadLedger {
    current: Option<Arc<PreparedRead>>,
    totals: ReadTotals,
}
impl ReadLedger {
    fn retire_completed(&mut self) {
        if let Some(read) = self.current.take() {
            let (bytes, inputs, done) = read.completed_read_bytes();
            self.totals.bytes = self.totals.bytes.saturating_add(bytes);
            self.totals.input_bytes.resize(inputs.len(), 0);
            for (sum, value) in self.totals.input_bytes.iter_mut().zip(inputs) {
                *sum = sum.saturating_add(value);
            }
            self.totals.completed_reads += u64::from(done);
            // Completed read Drop must not cancel the next rearmed graph pass.
            drop(read);
        }
    }
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
    fn start_prefetch(&self, budget: usize) {
        if self.canceled.load(Ordering::Acquire)
            || self.finished.load(Ordering::Acquire)
            || self.prefetch_control.canceled()
        {
            return;
        }
        let gate = self.gate.load(Ordering::Acquire);
        let prior_clean = self
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|(status, progress)| {
                status.kind == continuation::OK && progress.done != 0 && progress.canceled == 0
            });
        if gate != 0 && !(gate == 1 && self.reusable && prior_clean) {
            return;
        }
        let pass = self
            .passes_started
            .load(Ordering::Acquire)
            .saturating_add(1);
        {
            let mut state = self.prefetch.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed
                || state.preparing
                || state.read.is_some()
                || state.error.is_some()
                || state.pass >= pass
            {
                return;
            }
            state.pass = pass;
            state.preparing = true;
        }
        // This path runs only on the predecessor's native worker. Cancellation stays atomic
        // while no-reclaim preparation creates the bounded next input; no lock spans CUDA/I/O.
        let prepared = catch_unwind(AssertUnwindSafe(|| {
            if self.canceled.load(Ordering::Acquire) {
                return Ok(None);
            }
            let Some(work) = self.source.prepare_speculative(budget)? else {
                return Ok(None);
            };
            let read = Arc::new(PreparedRead::read_only(
                work,
                self.prefetch_control.clone(),
            )?);
            if self.canceled.load(Ordering::Acquire) {
                read.request_cancel();
            }
            match read.start_native() {
                Ok(()) => Ok(Some(read)),
                Err(Failure::Ineligible(_)) => {
                    let (_, complete) = read.join_native();
                    if complete {
                        Ok(None)
                    } else {
                        Err(Failure::Native(varena::Error::Poisoned(
                            "failed speculative worker cleanup is unproven".into(),
                        )))
                    }
                }
                Err(error) => Err(error),
            }
        }))
        .unwrap_or_else(|_| {
            Err(Failure::Native(varena::Error::Poisoned(
                "native speculative input preparation panicked".into(),
            )))
        });
        let mut state = self.prefetch.lock().unwrap_or_else(|e| e.into_inner());
        state.preparing = false;
        match prepared {
            Ok(Some(read)) => {
                self.prefetch_started.fetch_add(1, Ordering::Relaxed);
                state.read = Some(read);
            }
            Ok(None) => {
                self.prefetch_skipped.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => state.error = Some(error),
        }
        self.prefetch_changed.notify_all();
    }

    fn prefetch_quiescent(&self) -> bool {
        let state = self.prefetch.lock().unwrap_or_else(|e| e.into_inner());
        // Even a finished speculative reader must be joined and its bytes attributed before
        // a canceled unstarted one-shot cap may certify completion to an older importer.
        !state.preparing && state.error.is_none() && state.read.is_none()
    }

    fn drain_prefetch(&self, demand: bool, close: bool) -> Result<u64, Status> {
        let (read, error) = {
            let mut state = self.prefetch.lock().unwrap_or_else(|e| e.into_inner());
            if close {
                state.closed = true;
            }
            while state.preparing {
                state = self
                    .prefetch_changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
            (state.read.take(), state.error.take())
        };
        if let Some(error) = error {
            let status = failure(&error);
            if demand
                || matches!(
                    status.kind,
                    continuation::CUDA | continuation::INTERNAL | continuation::UNPROVEN
                )
            {
                return Err(status);
            }
        }
        let Some(read) = read else {
            return Ok(0);
        };
        if close {
            read.request_cancel();
        }
        let (result, completed) = read.join_native();
        if !completed {
            // Retain the actual read owner on ambiguous producer completion.
            std::mem::forget(read);
            return Err(Status::unproven(
                "speculative source completion is unproven",
            ));
        }
        let (bytes, inputs, done) = read.completed_read_bytes();
        *self
            .prefetch_last_clock
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = read.read_clock();
        {
            let mut totals = self
                .prefetch_totals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            totals.bytes = totals.bytes.saturating_add(bytes);
            totals.input_bytes.resize(inputs.len(), 0);
            for (sum, value) in totals.input_bytes.iter_mut().zip(inputs) {
                *sum = sum.saturating_add(value);
            }
            totals.completed_reads += u64::from(done);
        }
        if let Err(error) = result {
            let status = failure(&error);
            if demand
                || matches!(
                    status.kind,
                    continuation::CUDA | continuation::INTERNAL | continuation::UNPROVEN
                )
            {
                return Err(status);
            }
        }
        Ok(bytes)
    }

    fn publish(&self, status: Status, mut progress: Progress, complete: bool) -> Status {
        progress.epoch = self.epoch.load(Ordering::Acquire);
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
        self.passes_started.fetch_add(1, Ordering::Relaxed);
        let prefetched = match self.drain_prefetch(true, false) {
            Ok(bytes) => bytes,
            Err(error) => return self.abort_before_read(error),
        };
        let mut work = match self.source.prepare() {
            Ok(work) => work,
            Err(error) => return self.abort_before_read(failure(&error)),
        };
        if prefetched != 0 && work.missing_bytes() == 0 {
            self.prefetch_used.fetch_add(1, Ordering::Relaxed);
        }
        if let Some((next, budget)) = self.next.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            work.set_lookahead(Arc::new(move || {
                if let Some(next) = next.upgrade() {
                    next.start_prefetch(budget);
                }
            }));
        }
        let read = match PreparedRead::composed_with_control(
            work,
            self.graph.clone(),
            self.event_ordered,
            self.read_control.clone(),
        ) {
            Ok(read) => read,
            Err(error) => return self.abort_before_read(failure(&error)),
        };
        let read = Arc::new(read);
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).current = Some(read.clone());
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
        self.read_control.cancel();
        self.prefetch_control.cancel();
        status
    }

    fn rearm(&self) -> Status {
        if !self.reusable || self.finished.load(Ordering::Acquire) {
            return Status::new(continuation::INVALID, 0, "source input cannot rearm");
        }
        if self.canceled.load(Ordering::Acquire) {
            return Status::new(
                continuation::CANCELED,
                0,
                "source input cancellation is sticky",
            );
        }
        let prior = *self.result.lock().unwrap_or_else(|e| e.into_inner());
        if !prior.is_some_and(|(status, progress)| {
            status.kind == continuation::OK && progress.done != 0 && progress.canceled == 0
        }) {
            return Status::new(
                continuation::INVALID,
                0,
                "source input needs clean completed pass",
            );
        }
        if self
            .gate
            .compare_exchange(1, 3, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Status::new(
                continuation::INVALID,
                0,
                "source input is active or already rearmed",
            );
        }
        if let Err(status) = self.graph.rearm() {
            self.gate.store(1, Ordering::Release);
            return status;
        }
        let (status, progress) = self.graph.observe(false);
        let canceled = matches!(status.kind, continuation::OK | continuation::CANCELED)
            && progress.done != 0
            && progress.canceled != 0
            && self.canceled.load(Ordering::Acquire);
        if !canceled
            && (status.kind != continuation::OK || progress.phase != 0 || progress.done != 0)
        {
            return Status::unproven("rearmed graph did not report an armed epoch");
        }
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retire_completed();
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.epoch.store(progress.epoch, Ordering::Release);
        if canceled {
            self.graph.checked(status, progress);
            self.gate.store(2, Ordering::Release);
            return self.publish(
                Status::new(continuation::CANCELED, 0, "source canceled during rearm"),
                progress,
                true,
            );
        }
        self.gate.store(0, Ordering::Release);
        // A cancel racing the rearm must still prevent all later source work.
        if self.canceled.load(Ordering::Acquire) {
            let _ = self
                .gate
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
        }
        ok()
    }

    fn finish(&self) -> Status {
        if self.finished.load(Ordering::Acquire) {
            return ok();
        }
        let result = *self.result.lock().unwrap_or_else(|e| e.into_inner());
        if !result.is_some_and(|(status, progress)| {
            progress.done != 0
                && matches!(
                    status.kind,
                    continuation::OK
                        | continuation::CANCELED
                        | continuation::INVALID
                        | continuation::CAPACITY
                )
        }) {
            return Status::unproven("source input cannot finish without checked completion");
        }
        let gate = self.gate.load(Ordering::Acquire);
        if !matches!(gate, 1 | 2)
            || self
                .gate
                .compare_exchange(gate, 4, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Status::unproven("source input finish raced another transition");
        }
        if let Err(status) = self.drain_prefetch(false, true) {
            return status;
        }
        if let Err(status) = self.graph.finish() {
            return Status::unproven(&status.detail());
        }
        self.finished.store(true, Ordering::Release);
        self.gate.store(5, Ordering::Release);
        ok()
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
                            epoch: self.epoch.load(Ordering::Acquire),
                            ..Progress::default()
                        },
                    );
                }
                2 => {
                    drop(result);
                    if wait {
                        if let Err(status) = self.drain_prefetch(false, true) {
                            self.publish(status, Progress::default(), false);
                            return (status, Progress::default());
                        }
                    } else if !self.prefetch_quiescent() {
                        return (
                            ok(),
                            Progress {
                                phase: 1,
                                canceled: 1,
                                epoch: self.epoch.load(Ordering::Acquire),
                                ..Progress::default()
                            },
                        );
                    }
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
                            epoch: self.epoch.load(Ordering::Acquire),
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
                            epoch: self.epoch.load(Ordering::Acquire),
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
unsafe extern "C" fn rearm(context: *mut c_void) -> Status {
    boundary(context, Stage::rearm)
}
unsafe extern "C" fn finish(context: *mut c_void) -> Status {
    boundary(context, Stage::finish)
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
    #[classattr]
    fn supports_native_lookahead() -> bool {
        true
    }

    #[getter]
    fn reservation_bytes(&self) -> usize {
        self.owner.source.reservation_bytes()
    }

    #[pyo3(signature = (next_job, *, max_prefetch_bytes))]
    fn link_next(
        &self,
        next_job: PyRef<'_, StageJob>,
        max_prefetch_bytes: usize,
    ) -> PyResult<bool> {
        let current = &self.owner;
        let next = &next_job.owner;
        if current.pid != std::process::id() || next.pid != current.pid {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "lookahead belongs to another process",
            ));
        }
        if Arc::ptr_eq(current, next)
            || current.source.device() != next.source.device()
            || current.gate.load(Ordering::Acquire) != 0
            || next.gate.load(Ordering::Acquire) != 0
        {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "lookahead needs distinct unstarted source jobs on one device",
            ));
        }
        if max_prefetch_bytes == 0 || next.source.reservation_bytes() > max_prefetch_bytes {
            return Ok(false);
        }
        let mut link = current.next.lock().unwrap_or_else(|e| e.into_inner());
        if link.is_some() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "source job already has lookahead",
            ));
        }
        *link = Some((Arc::downgrade(next), max_prefetch_bytes));
        Ok(true)
    }

    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        if self.owner.pid != std::process::id() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "stage observer used after fork",
            ));
        }
        let (current, totals) = {
            let ledger = self.owner.reads.lock().unwrap_or_else(|e| e.into_inner());
            (ledger.current.clone(), ledger.totals.clone())
        };
        let (bytes, inputs, read_done) = current
            .as_ref()
            .map_or((0, Vec::new(), false), |read| read.completed_read_bytes());
        let row = match current {
            Some(read) => read.facts(py)?,
            None => PyDict::new(py),
        };
        let mut prefetch_clock = *self
            .owner
            .prefetch_last_clock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut prefetched = self
            .owner
            .prefetch_totals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(read) = self
            .owner
            .prefetch
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read
            .clone()
        {
            prefetch_clock = read.read_clock();
            let (bytes, inputs, done) = read.completed_read_bytes();
            prefetched.completed_reads += u64::from(done);
            prefetched.bytes = prefetched.bytes.saturating_add(bytes);
            prefetched.input_bytes.resize(inputs.len(), 0);
            for (sum, value) in prefetched.input_bytes.iter_mut().zip(inputs) {
                *sum = sum.saturating_add(value);
            }
        }
        let mut input_bytes = totals.input_bytes;
        input_bytes.resize(input_bytes.len().max(inputs.len()), 0);
        for (sum, value) in input_bytes.iter_mut().zip(inputs) {
            *sum = sum.saturating_add(value);
        }
        input_bytes.resize(input_bytes.len().max(prefetched.input_bytes.len()), 0);
        for (sum, value) in input_bytes.iter_mut().zip(&prefetched.input_bytes) {
            *sum = sum.saturating_add(*value);
        }
        input_bytes.resize(self.owner.source.input_count(), 0);
        prefetched
            .input_bytes
            .resize(self.owner.source.input_count(), 0);
        let result = *self.owner.result.lock().unwrap_or_else(|e| e.into_inner());
        let completed_pass = result.is_some_and(|(_, progress)| progress.done != 0);
        let finished = self.owner.finished.load(Ordering::Acquire);
        let unsafe_completion = self.owner.retained_unsafe.load(Ordering::Acquire);
        let complete = completed_pass && (!self.owner.reusable || finished) && !unsafe_completion;
        row.set_item(
            "started",
            self.owner.passes_started.load(Ordering::Acquire) != 0,
        )?;
        row.set_item(
            "passes_started",
            self.owner.passes_started.load(Ordering::Acquire),
        )?;
        row.set_item(
            "completed_reads",
            totals.completed_reads + u64::from(read_done),
        )?;
        row.set_item("pass_epoch", self.owner.epoch.load(Ordering::Acquire))?;
        row.set_item("pass_done", completed_pass)?;
        row.set_item(
            "done",
            if self.owner.reusable {
                finished || unsafe_completion
            } else {
                result.is_some()
            },
        )?;
        row.set_item("completion_proven", complete)?;
        row.set_item(
            "read_done",
            totals.completed_reads != 0 || read_done || prefetched.completed_reads != 0,
        )?;
        row.set_item(
            "bytes",
            totals
                .bytes
                .saturating_add(bytes)
                .saturating_add(prefetched.bytes),
        )?;
        row.set_item(
            "read_bytes",
            totals
                .bytes
                .saturating_add(bytes)
                .saturating_add(prefetched.bytes),
        )?;
        row.set_item("input_read_bytes", input_bytes)?;
        row.set_item(
            "issued_bytes",
            self.owner
                .read_control
                .issued_bytes()
                .saturating_add(self.owner.prefetch_control.issued_bytes()),
        )?;
        row.set_item("prefetch_read_bytes", prefetched.bytes)?;
        row.set_item("prefetch_last_read_issued_clock_ns", prefetch_clock.0)?;
        row.set_item("prefetch_last_read_completed_clock_ns", prefetch_clock.1)?;
        row.set_item("read_bytes_scope", "logical source bytes from completed reads; demand plus speculative, not measured PCIe traffic")?;
        row.set_item("prefetch_input_read_bytes", prefetched.input_bytes)?;
        row.set_item(
            "prefetch_issued_bytes",
            self.owner.prefetch_control.issued_bytes(),
        )?;
        row.set_item(
            "prefetch_started",
            self.owner.prefetch_started.load(Ordering::Acquire),
        )?;
        row.set_item(
            "prefetch_skipped",
            self.owner.prefetch_skipped.load(Ordering::Acquire),
        )?;
        row.set_item(
            "prefetch_used",
            self.owner.prefetch_used.load(Ordering::Acquire),
        )?;
        row.set_item(
            "canceled",
            result.is_some_and(|(_, progress)| progress.canceled != 0),
        )?;
        row.set_item(
            "cancel_requested",
            self.owner.canceled.load(Ordering::Acquire),
        )?;
        row.set_item(
            "failed",
            unsafe_completion
                || result.is_some_and(|(status, _)| {
                    !matches!(status.kind, continuation::OK | continuation::CANCELED)
                }),
        )?;
        row.set_item(
            "clock",
            "elapsed_ns_since_current_pass_read; byte counters cover submission",
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
    let reusable = graph.supports_reusable_input();
    let owner = Arc::new(Stage {
        source,
        graph,
        module,
        pid: std::process::id(),
        epoch: AtomicU64::new(progress.epoch),
        event_ordered,
        reusable,
        finished: AtomicBool::new(false),
        passes_started: AtomicU64::new(0),
        read_control: Arc::new(tensorfs::io::ReadControl::default()),
        gate: AtomicU8::new(0),
        canceled: AtomicBool::new(false),
        retained_unsafe: AtomicBool::new(false),
        reads: Mutex::new(ReadLedger::default()),
        next: Mutex::new(None),
        prefetch: Mutex::new(Prefetch::default()),
        prefetch_changed: Condvar::new(),
        prefetch_control: Arc::new(tensorfs::io::ReadControl::default()),
        prefetch_totals: Mutex::new(ReadTotals::default()),
        prefetch_started: AtomicU64::new(0),
        prefetch_skipped: AtomicU64::new(0),
        prefetch_used: AtomicU64::new(0),
        prefetch_last_clock: Mutex::new((0, 0)),
        result: Mutex::new(None),
        changed: Condvar::new(),
    });
    let api = Api {
        version: 1,
        size: std::mem::size_of::<Api>() as u32,
        flags: if reusable { 7 | 16 } else { 7 },
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
        rearm: reusable.then_some(rearm),
        finish: reusable.then_some(finish),
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

const _: () = assert!(std::mem::size_of::<Api>() == 112);
const _: () = assert!(std::mem::size_of::<Status>() == 256);
const _: () = assert!(std::mem::size_of::<Progress>() == 24);
