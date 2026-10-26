//! Authored phase entry decorates the existing continuation ABI. No execution queue,
//! Python callback, physical pin or source preparation is introduced by this wrapper.
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pyo3::{
    prelude::*,
    types::{PyCapsule, PyDict},
};

use super::continuation::{self, Continuation, Progress, Status};
use super::{Deferred, PyArena, PyRegion};

struct Policy {
    arena: Arc<varena::Arena>,
    active: Mutex<Option<String>>,
    transitions: AtomicU64,
    definitions: Mutex<BTreeMap<String, Vec<u64>>>,
}
impl Policy {
    fn enter(&self, key: &str, regions: &[Arc<varena::Region>]) -> Result<(), varena::Error> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.as_deref() == Some(key) {
            return Ok(());
        }
        self.arena.enter_phase(regions)?;
        *active = Some(key.to_owned());
        self.transitions.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[pyclass(name = "_VarenaPhaseController", module = "tensorfs._ext", frozen)]
pub(super) struct PhaseController {
    policy: Deferred<Arc<Policy>>,
    pid: u32,
}

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
// SAFETY: the provider retains the opaque context; every operation checks process ownership.
unsafe impl Send for Api {}
#[repr(C)]
struct Capsule {
    api: Api,
    _owner: Arc<Phase>,
}

struct Phase {
    policy: Arc<Policy>,
    key: String,
    regions: Vec<Arc<varena::Region>>,
    inner: Arc<Continuation>,
    module: Arc<Py<PyAny>>,
    pid: u32,
    started: AtomicBool,
    canceled: AtomicBool,
    unsafe_owner: AtomicBool,
    failure: Mutex<Option<(Status, Progress)>>,
}
fn ok() -> Status {
    Status::new(continuation::OK, 0, "")
}
fn native(error: varena::Error) -> Status {
    let text = error.to_string();
    match error {
        varena::Error::Cuda { code, .. } | varena::Error::DeviceFault { code, .. } => {
            Status::new(continuation::CUDA, code, &text)
        }
        varena::Error::Poisoned(_) => Status::unproven(&text),
        varena::Error::OutOfMemory { .. } => Status::new(continuation::CAPACITY, 2, &text),
        _ => Status::new(continuation::INVALID, 0, &text),
    }
}
fn unsafe_status(status: Status) -> bool {
    matches!(
        status.kind,
        continuation::CUDA | continuation::INTERNAL | continuation::UNPROVEN
    )
}
impl Phase {
    fn start(&self) -> Status {
        if self.started.swap(true, Ordering::AcqRel) {
            return Status::new(continuation::INVALID, 0, "phase capability already started");
        }
        if self.canceled.load(Ordering::Acquire) {
            return self.inner.cancel();
        }
        if let Err(error) = self.policy.enter(&self.key, &self.regions) {
            let mut status = native(error);
            let mut progress = Progress::default();
            match self.inner.abort() {
                Ok(()) => {
                    let (_, observed) = self.inner.observe(false);
                    progress = observed;
                    progress.done = u32::from(!unsafe_status(status));
                }
                Err(failure) => status = failure,
            }
            *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some((status, progress));
            return status;
        }
        if self.canceled.load(Ordering::Acquire) {
            self.inner.cancel();
        }
        self.inner.start().map_or_else(|error| error, |_| ok())
    }
    fn observe(&self, wait: bool) -> (Status, Progress) {
        if let Some(result) = *self.failure.lock().unwrap_or_else(|e| e.into_inner()) {
            return result;
        }
        let (status, progress) = self.inner.observe(wait);
        self.inner.checked(status, progress);
        (status, progress)
    }
    fn cancel(&self) -> Status {
        self.canceled.store(true, Ordering::Release);
        self.inner.cancel()
    }
    fn rearm(&self) -> Status {
        if self.canceled.load(Ordering::Acquire)
            || self
                .failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
        {
            return Status::new(
                continuation::CANCELED,
                0,
                "phase cancellation or failure is sticky",
            );
        }
        match self.inner.rearm() {
            Ok(()) => {
                self.started.store(false, Ordering::Release);
                ok()
            }
            Err(error) => error,
        }
    }
}
struct Retire(
    Option<(
        Arc<Continuation>,
        Arc<Policy>,
        Vec<Arc<varena::Region>>,
        Arc<Py<PyAny>>,
    )>,
);
impl Drop for Retire {
    fn drop(&mut self) {
        if let Some(owners) = self.0.take() {
            if !owners.0.releasable() && owners.0.abort().is_err() {
                std::mem::forget(owners);
            }
        }
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        let owners = (
            self.inner.clone(),
            self.policy.clone(),
            self.regions.clone(),
            self.module.clone(),
        );
        if self.pid != std::process::id() {
            std::mem::forget(owners);
        } else {
            crate::cleanup::defer(Retire(Some(owners)));
        }
    }
}
fn call(context: *mut c_void, invoke: impl FnOnce(&Phase) -> Status) -> Status {
    if context.is_null() {
        return Status::new(continuation::INVALID, 0, "null phase capability");
    }
    // SAFETY: same-process importer holds one retained provider context for this call.
    let phase = unsafe { &*context.cast::<Phase>() };
    if phase.pid != std::process::id() {
        return Status::new(continuation::INVALID, 0, "phase used after fork");
    }
    let status = catch_unwind(AssertUnwindSafe(|| invoke(phase)))
        .unwrap_or_else(|_| Status::new(continuation::INTERNAL, 999, "phase entry panicked"));
    if unsafe_status(status) && !phase.unsafe_owner.swap(true, Ordering::AcqRel) {
        // SAFETY: ambiguous completion retains the actual wrapper and its inner owners.
        unsafe {
            Arc::increment_strong_count(context.cast::<Phase>());
        }
    }
    status
}
unsafe extern "C" fn retain(context: *mut c_void) -> Status {
    call(context, |_| {
        // SAFETY: capsule or existing retain keeps this Arc alive.
        unsafe {
            Arc::increment_strong_count(context.cast::<Phase>());
        }
        ok()
    })
}
unsafe extern "C" fn release(context: *mut c_void) -> Status {
    if context.is_null() {
        return Status::new(continuation::INVALID, 0, "null phase release");
    }
    // SAFETY: one importer retain remains until this matching release.
    if unsafe { (&*context.cast::<Phase>()).pid } != std::process::id() {
        return Status::new(continuation::INVALID, 0, "phase released after fork");
    }
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: consume exactly one retained provider Arc in its allocating module.
        unsafe {
            drop(Arc::from_raw(context.cast::<Phase>()));
        }
    })) {
        Ok(()) => ok(),
        Err(_) => Status::unproven("phase release panicked"),
    }
}
unsafe extern "C" fn start(context: *mut c_void) -> Status {
    call(context, Phase::start)
}
unsafe extern "C" fn cancel(context: *mut c_void) -> Status {
    call(context, Phase::cancel)
}
unsafe extern "C" fn rearm(context: *mut c_void) -> Status {
    call(context, Phase::rearm)
}
unsafe extern "C" fn finish(context: *mut c_void) -> Status {
    call(context, |phase| {
        phase.inner.finish().map_or_else(|error| error, |_| ok())
    })
}
fn observe(context: *mut c_void, output: *mut Progress, wait: bool) -> Status {
    if output.is_null() {
        return Status::new(continuation::INVALID, 0, "null phase progress");
    }
    call(context, |phase| {
        let (status, progress) = phase.observe(wait);
        // SAFETY: importer supplied one writable ABI-sized output.
        unsafe {
            output.write(progress);
        }
        status
    })
}
unsafe extern "C" fn poll(context: *mut c_void, output: *mut Progress) -> Status {
    observe(context, output, false)
}
unsafe extern "C" fn wait(context: *mut c_void, output: *mut Progress) -> Status {
    observe(context, output, true)
}

#[pymethods]
impl PhaseController {
    #[new]
    fn new(arena: PyRef<'_, PyArena>) -> Self {
        Self {
            policy: Deferred::new(Arc::new(Policy {
                arena: arena.arena.clone(),
                active: Mutex::new(None),
                transitions: AtomicU64::new(0),
                definitions: Mutex::new(BTreeMap::new()),
            })),
            pid: std::process::id(),
        }
    }
    fn wrap(
        &self,
        py: Python<'_>,
        capsule: &Bound<'_, PyAny>,
        key: String,
        regions: Vec<Py<PyRegion>>,
    ) -> PyResult<Py<PyAny>> {
        if self.pid != std::process::id() || key.is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "phase needs same-process nonempty identity",
            ));
        }
        let regions: Vec<_> = regions
            .into_iter()
            .map(|region| region.borrow(py).region.clone())
            .collect();
        if regions
            .iter()
            .any(|region| !self.policy.arena.owns_region(region))
        {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "phase regions belong to another arena",
            ));
        }
        let mut ids: Vec<_> = regions.iter().map(|region| region.ptr()).collect();
        ids.sort_unstable();
        ids.dedup();
        {
            let mut definitions = self
                .policy
                .definitions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(prior) = definitions.get(&key) {
                if prior != &ids {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "phase identity changed its region set",
                    ));
                }
            } else {
                definitions.insert(key.clone(), ids);
            }
        }
        let module = Arc::new(py.import("tensorfs._ext")?.into_any().unbind());
        let inner = Arc::new(Continuation::import(capsule, self.policy.arena.device())?);
        let (status, progress) = py.detach(|| inner.observe(false));
        if status.kind != continuation::OK || progress.phase != 0 || progress.done != 0 {
            inner.abandon();
            return Err(pyo3::exceptions::PyValueError::new_err(
                "phase needs an armed continuation",
            ));
        }
        // Validation here does not change preferences. The arena validates ownership again
        // atomically at actual phase entry, not from borrowed Python pointers.
        if regions
            .iter()
            .any(|region| region.device() != self.policy.arena.device())
        {
            inner.abandon();
            return Err(pyo3::exceptions::PyValueError::new_err(
                "phase regions cross devices",
            ));
        }
        let reusable = inner.supports_reusable_input();
        let owner = Arc::new(Phase {
            policy: self.policy.clone(),
            key,
            regions,
            inner,
            module,
            pid: self.pid,
            started: AtomicBool::new(false),
            canceled: AtomicBool::new(false),
            unsafe_owner: AtomicBool::new(false),
            failure: Mutex::new(None),
        });
        let api = Api {
            version: 1,
            size: std::mem::size_of::<Api>() as u32,
            flags: if reusable { 7 | 16 } else { 7 },
            device: self.policy.arena.device(),
            reserved: 0,
            pid: u64::from(self.pid),
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
        Ok(PyCapsule::new_with_value(
            py,
            Capsule { api, _owner: owner },
            c"cozy.execution.continuation.v1",
        )?
        .into_any()
        .unbind())
    }
    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        if self.pid != std::process::id() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "phase controller observed after fork",
            ));
        }
        let out = PyDict::new(py);
        out.set_item(
            "transitions",
            self.policy.transitions.load(Ordering::Acquire),
        )?;
        out.set_item(
            "active",
            self.policy
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )?;
        out.set_item(
            "policy",
            "ready-only preference; no allocation, eviction or window mutation",
        )?;
        Ok(out)
    }
}
const _: () = assert!(std::mem::size_of::<Api>() == 112);
