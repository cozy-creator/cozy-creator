//! Torch callbacks contain no Python; Rust owns allocation and completion state.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use pyo3::{prelude::*, types::PyDict};

static OWNER: AtomicU32 = AtomicU32::new(0);
static CALLBACK_ERROR: OnceLock<varena::Error> = OnceLock::new();

#[repr(C)]
struct Failure {
    kind: i32,
    cuda: i32,
    message: [u8; 512],
}

impl Failure {
    fn from_error(error: &varena::Error) -> Self {
        let (kind, cuda) = match error {
            varena::Error::OutOfMemory { .. } | varena::Error::Exhausted { .. } => (1, 0),
            varena::Error::DeviceFault { code, .. } | varena::Error::Cuda { code, .. } => {
                (2, *code)
            }
            _ => (3, 0),
        };
        let mut failure = Self {
            kind,
            cuda,
            message: [0; 512],
        };
        let text = error.to_string();
        let mut len = text.len().min(failure.message.len() - 1);
        while !text.is_char_boundary(len) {
            len -= 1;
        }
        failure.message[..len].copy_from_slice(&text.as_bytes()[..len]);
        failure
    }
}

pub(super) fn installed() -> bool {
    OWNER.load(Ordering::Acquire) != 0
}

#[pyfunction]
fn _varena_process_guard_status() -> String {
    let status = varena::process_guard_status();
    serde_json::json!({
        "schema": "varena.process_guard/1", "contract": "one_process/1", "backend": status.backend,
        "marker": status.marker, "thread_authenticated": status.thread_authenticated,
        "page_bytes": status.page_bytes, "failure_stage": status.failure_stage,
        "failure_errno": status.failure_errno,
        "shared_vm_child_use": "unsupported; inherited calls are not guaranteed to reject",
    }).to_string()
}

fn check() -> varena::Result<()> {
    if !installed() {
        return Err(varena::Error::Invalid(
            "Torch allocator is not installed in this process",
        ));
    }
    if let Some(error) = CALLBACK_ERROR.get() {
        return Err(error.clone());
    }
    Ok(())
}

fn call<T: Default>(failure: *mut Failure, operation: impl FnOnce() -> varena::Result<T>) -> T {
    let result = catch_unwind(AssertUnwindSafe(|| {
        check()?;
        operation()
    }))
    .unwrap_or_else(|_| {
        Err(varena::Error::Poisoned(
            "panic in allocator callback".into(),
        ))
    });
    match result {
        Ok(value) => value,
        Err(error) => {
            // SAFETY: the shim supplies a writable Failure for this synchronous call.
            unsafe {
                failure.write(Failure::from_error(&error));
            }
            T::default()
        }
    }
}

fn retire(failure: *mut Failure, operation: impl FnOnce() -> varena::Result<()>) -> i32 {
    call(failure, || {
        let result = catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|_| {
            Err(varena::Error::Poisoned(
                "panic in allocator destructor".into(),
            ))
        });
        if let Err(error) = result {
            // Destructor callbacks cannot throw. Preserve the first failure for the next boundary.
            // A fork rejection must not wait on an inherited OnceLock initializer.
            varena::working::bind_process()?;
            let _ = CALLBACK_ERROR.set(error.clone());
            return Err(error);
        }
        Ok(1)
    })
}

unsafe extern "C" fn alloc(
    bytes: usize,
    device: i32,
    stream: u64,
    routed: i32,
    a: u64,
    b: u64,
    failure: *mut Failure,
) -> u64 {
    call(failure, || {
        varena::working::alloc(device, bytes, stream, (routed != 0).then_some((a, b)))
    })
}

unsafe extern "C" fn free(device: i32, ptr: u64, stream: u64, failure: *mut Failure) -> i32 {
    retire(failure, || varena::working::free(device, ptr, stream))
}

unsafe extern "C" fn record_stream(ptr: u64, stream: u64, failure: *mut Failure) -> i32 {
    call(failure, || {
        varena::working::record_stream_any(ptr, stream).map(|()| 1)
    })
}

unsafe extern "C" fn trim(failure: *mut Failure) -> i32 {
    call(failure, || varena::working::trim_all().map(|_| 1))
}

unsafe extern "C" fn pool_begin(
    device: i32,
    a: u64,
    b: u64,
    stream: u64,
    failure: *mut Failure,
) -> i32 {
    call(failure, || {
        varena::working::pool_begin(device, (a, b), stream).map(|()| 1)
    })
}

unsafe extern "C" fn pool_end(device: i32, a: u64, b: u64, failure: *mut Failure) -> i32 {
    call(failure, || {
        varena::working::pool_end(device, (a, b)).map(|()| 1)
    })
}

unsafe extern "C" fn pool_release(device: i32, a: u64, b: u64, failure: *mut Failure) -> i32 {
    retire(failure, || varena::working::pool_release(device, (a, b)))
}

#[repr(C)]
struct Bindings {
    abi: u32,
    size: u32,
    alloc: unsafe extern "C" fn(usize, i32, u64, i32, u64, u64, *mut Failure) -> u64,
    free: unsafe extern "C" fn(i32, u64, u64, *mut Failure) -> i32,
    record_stream: unsafe extern "C" fn(u64, u64, *mut Failure) -> i32,
    trim: unsafe extern "C" fn(*mut Failure) -> i32,
    pool_begin: unsafe extern "C" fn(i32, u64, u64, u64, *mut Failure) -> i32,
    pool_end: unsafe extern "C" fn(i32, u64, u64, *mut Failure) -> i32,
    pool_release: unsafe extern "C" fn(i32, u64, u64, *mut Failure) -> i32,
}

static BINDINGS: Bindings = Bindings {
    abi: 1,
    size: std::mem::size_of::<Bindings>() as u32,
    alloc,
    free,
    record_stream,
    trim,
    pool_begin,
    pool_end,
    pool_release,
};

#[pyfunction]
fn _varena_working_bindings() -> PyResult<usize> {
    varena::working::bind_process()
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    let pid = std::process::id();
    let owner = OWNER.load(Ordering::Acquire);
    if owner != 0 && owner != pid {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "Torch allocator cannot be installed after fork",
        ));
    }
    let owner = OWNER
        .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire)
        .unwrap_or_else(|v| v);
    if owner != 0 && owner != pid {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "Torch allocator cannot be installed after fork",
        ));
    }
    Ok(&BINDINGS as *const Bindings as usize)
}

#[pyfunction]
fn _varena_working_initialize(py: Python<'_>, device: i32) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::initialize(device)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_check(py: Python<'_>) -> PyResult<()> {
    varena::working::bind_process()
        .and_then(|()| check())
        .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_stats(
    py: Python<'_>,
    device: i32,
) -> PyResult<std::collections::BTreeMap<&'static str, u64>> {
    let stats = py
        .detach(|| {
            check()?;
            varena::working::stats(device)
        })
        .map_err(|e| super::raise(py, e))?;
    Ok([
        ("allocated_bytes", stats.allocated_bytes as u64),
        ("allocated_peak", stats.allocated_peak as u64),
        ("mapped_bytes", stats.mapped_bytes as u64),
        ("mapped_peak", stats.mapped_peak as u64),
        ("pending_bytes", stats.pending_bytes as u64),
        ("pools_bytes", stats.pools_bytes as u64),
        ("allocated_capacity", stats.allocated_capacity as u64),
        ("free_bytes", stats.free_bytes as u64),
        ("reclaimable_bytes", stats.reclaimable_bytes as u64),
        ("unassigned_bytes", stats.unassigned_bytes as u64),
        ("pools", stats.pools as u64),
        ("event_handles", stats.event_handles as u64),
        ("allocations", stats.allocations),
        ("frees", stats.frees),
        ("ooms", stats.ooms),
        ("driver_memory_calls", stats.driver_memory_calls),
        ("driver_physical_ooms", stats.driver_physical_ooms),
        ("event_queries", stats.event_queries),
        ("event_records", stats.event_records),
        ("event_waits", stats.event_waits),
        ("context_joins", stats.context_joins),
        ("prefetch_attempts", stats.prefetch_attempts),
        ("prefetch_admissions", stats.prefetch_admissions),
        ("prefetch_declines", stats.prefetch_declines),
        ("cache_reserved_bytes", stats.cache_reserved_bytes as u64),
        ("cache_mapped_bytes", stats.cache_mapped_bytes as u64),
        ("cache_live_bytes", stats.cache_live_bytes as u64),
        ("cache_pending_bytes", stats.cache_pending_bytes as u64),
        ("cache_ready_bytes", stats.cache_ready_bytes as u64),
        ("caches", stats.caches as u64),
    ]
    .into())
}

#[pyfunction]
fn _varena_working_reset_peak(py: Python<'_>, device: i32) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::reset_peak(device)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_trim(py: Python<'_>, device: i32) -> PyResult<usize> {
    py.detach(|| {
        check()?;
        varena::working::trim(device)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
#[pyo3(signature = (device, limit = None))]
fn _varena_working_set_limit(py: Python<'_>, device: i32, limit: Option<usize>) -> PyResult<usize> {
    py.detach(|| {
        check()?;
        varena::set_limit(device, limit)
    })
    .map(|report| report.reclaimed_bytes)
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_reserve(
    py: Python<'_>,
    device: i32,
    pool: (u64, u64),
    bytes: usize,
) -> PyResult<usize> {
    py.detach(|| {
        check()?;
        varena::working::pool_reserve(device, pool, bytes)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_record_stream(
    py: Python<'_>,
    device: i32,
    pool: (u64, u64),
    stream: u64,
) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_record_stream(device, pool, stream)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_capture_complete(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_capture_complete(device, pool)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_cancel_preparation(
    py: Python<'_>,
    device: i32,
    pool: (u64, u64),
) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_cancel_preparation(device, pool)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_retain(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_retain(device, pool)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_retire_stream(py: Python<'_>, device: i32, stream: u64) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::retire_stream(device, stream)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_release(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_release(device, pool)
    })
    .map_err(|e| super::raise(py, e))
}

fn split_error(py: Python<'_>, error: varena::Error) -> PyErr {
    let capacity = matches!(
        error,
        varena::Error::OutOfMemory { .. } | varena::Error::Exhausted { .. }
    );
    let result = super::raise(py, error);
    if capacity {
        let _ = result.value(py).setattr("varena_no_retry", true);
    }
    result
}

#[pyfunction]
fn _varena_split_context<'py>(py: Python<'py>, device: i32) -> PyResult<Bound<'py, PyDict>> {
    let context = py
        .detach(|| {
            check()?;
            varena::working::split_context(device)
        })
        .map_err(|error| super::raise(py, error))?;
    let row = PyDict::new(py);
    row.set_item("eligible", context.eligible)?;
    row.set_item("reason", context.reason)?;
    Ok(row)
}

#[pyfunction]
#[pyo3(signature = (device, key, choices, *, preferred, stream))]
fn _varena_split_select<'py>(
    py: Python<'py>,
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    preferred: &str,
    stream: u64,
) -> PyResult<Bound<'py, PyDict>> {
    let selected = py
        .detach(|| {
            check()?;
            varena::working::split_select(device, key, choices, preferred, stream)
        })
        .map_err(|error| split_error(py, error))?;
    selection(py, &selected)
}

fn selection<'py>(
    py: Python<'py>, selected: &varena::working::SplitSelection,
) -> PyResult<Bound<'py, PyDict>> {
    let row = PyDict::new(py);
    row.set_item("stream", selected.stream)?;
    row.set_item(
        "allocation_backing_bytes",
        selected.allocation_backing_bytes,
    )?;
    row.set_item(
        "measured_backing_peak_bytes",
        selected.measured_backing_peak_bytes,
    )?;
    row.set_item("reused_capacity_bytes", selected.reused_capacity_bytes)?;
    row.set_item("preserved_backing_bytes", selected.preserved_backing_bytes)?;
    row.set_item(
        "reclaimable_working_bytes",
        selected.reclaimable_working_bytes,
    )?;
    row.set_item("choice_id", &selected.choice_id)?;
    row.set_item("eligible", &selected.eligible)?;
    row.set_item("required_bytes", selected.required_bytes)?;
    row.set_item("available_bytes", selected.available_bytes)?;
    row.set_item("basis", selected.basis)?;
    row.set_item("reclaimed_working_bytes", selected.reclaimed_working_bytes)?;
    row.set_item("reclaimed_weight_bytes", selected.reclaimed_weight_bytes)?;
    row.set_item("plan_generation", selected.plan_generation)?;
    row.set_item("phase", &selected.phase)?;
    row.set_item("phase_weight_bytes", selected.phase_weight_bytes)?;
    row.set_item("phase_missing_bytes", selected.phase_missing_bytes)?;
    row.set_item("phase_outside_reclaimable_bytes", selected.phase_outside_reclaimable_bytes)?;
    row.set_item("preserves_phase_weights", selected.preserves_phase_weights)?;
    Ok(row)
}

#[pyfunction]
#[pyo3(signature = (device, key, choice_id, *, stream, completion_owner = None, plan_generation = None))]
fn _varena_split_begin(
    py: Python<'_>,
    device: i32,
    key: &str,
    choice_id: &str,
    stream: u64,
    completion_owner: Option<u64>,
    plan_generation: Option<u64>,
) -> PyResult<u64> {
    py.detach(|| {
        check()?;
        varena::working::split_begin(device, key, choice_id, completion_owner, stream, plan_generation)
    })
    .map_err(|error| split_error(py, error))
}

#[pyfunction]
#[pyo3(signature = (device, observation_id, *, submitted))]
fn _varena_split_end<'py>(
    py: Python<'py>,
    device: i32,
    observation_id: u64,
    submitted: bool,
) -> PyResult<Bound<'py, PyDict>> {
    // A failed operation still closes its interval after the bridge records the original error.
    let observed = py
        .detach(|| varena::working::split_end(device, observation_id, submitted))
        .map_err(|error| super::raise(py, error))?;
    split_observation(py, &observed)
}

fn split_observation<'py>(
    py: Python<'py>,
    observed: &varena::working::SplitObservation,
) -> PyResult<Bound<'py, PyDict>> {
    let row = PyDict::new(py);
    row.set_item("observation_id", observed.observation_id)?;
    row.set_item("key", &observed.key)?;
    row.set_item("choice_id", &observed.choice_id)?;
    row.set_item(
        "peak_bytes",
        observed.observed.then_some(observed.peak_bytes),
    )?;
    row.set_item(
        "backing_peak_bytes",
        observed.observed.then_some(observed.backing_peak_bytes),
    )?;
    row.set_item("observed", observed.observed)?;
    row.set_item("pending", observed.pending)?;
    row.set_item("published", observed.published)?;
    row.set_item("reason", &observed.reason)?;
    row.set_item("successful_passes", observed.successful_passes)?;
    Ok(row)
}

/// Owns one complete producer-declared execution. No Python callback enters native code.
#[pyclass(name = "_VarenaExecution", module = "tensorfs._ext", frozen)]
struct PyExecution {
    execution: super::Deferred<varena::working::Execution>,
}

#[pymethods]
impl PyExecution {
    #[new]
    #[pyo3(signature = (device, configuration, site, choices, *, preferred, stream))]
    fn new(
        py: Python<'_>, device: i32, configuration: &str, site: &str,
        choices: Vec<(String, Vec<usize>)>, preferred: &str, stream: u64,
    ) -> PyResult<Self> {
        let execution = py.detach(|| {
            check()?;
            varena::working::Execution::commit(
                device, configuration, site, choices, preferred, stream,
            )
        }).map_err(|error| split_error(py, error))?;
        Ok(Self { execution: super::Deferred::new(execution) })
    }

    #[getter]
    fn key(&self, py: Python<'_>) -> PyResult<String> {
        self.execution.key().map(str::to_owned).map_err(|error| super::raise(py, error))
    }

    #[getter]
    fn selection<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        selection(py, self.execution.selection().map_err(|error| super::raise(py, error))?)
    }

    fn enter(&self, py: Python<'_>) -> PyResult<u64> {
        py.detach(|| { check()?; self.execution.enter() })
            .map_err(|error| split_error(py, error))
    }

    fn begin(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| { check()?; self.execution.begin() })
            .map_err(|error| split_error(py, error))
    }

    #[pyo3(signature = (*, submitted))]
    fn end<'py>(&self, py: Python<'py>, submitted: bool) -> PyResult<Bound<'py, PyDict>> {
        let observed = py.detach(|| self.execution.end(submitted))
            .map_err(|error| super::raise(py, error))?;
        split_observation(py, &observed)
    }

    #[pyo3(signature = (*, completed))]
    fn finish<'py>(&self, py: Python<'py>, completed: bool) -> PyResult<Bound<'py, PyDict>> {
        let observed = py.detach(|| self.execution.finish(completed))
            .map_err(|error| super::raise(py, error))?;
        observation(py, &observed)
    }

    fn abort<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        // Cleanup remains possible after the original allocation error was latched.
        py.detach(|| self.execution.abort()).map_err(|error| super::raise(py, error))?
            .as_ref().map(|observed| observation(py, observed)).transpose()
    }
}

#[pyclass(name = "_VarenaCallbackScope", module = "tensorfs._ext", frozen)]
struct CallbackScope {
    guard: Mutex<Option<varena::GlobalCaptureGuard>>,
}
#[pymethods]
impl CallbackScope {
    #[new]
    fn new(py: Python<'_>, device: i32) -> PyResult<Self> {
        let guard =
            varena::enter_external_callback(device).map_err(|error| super::raise(py, error))?;
        Ok(Self {
            guard: Mutex::new(Some(guard)),
        })
    }
    fn close(&self) {
        self.guard.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

#[pyclass(name = "_VarenaGraphFootprint", module = "tensorfs._ext", frozen)]
pub(super) struct PyGraphFootprint {
    pub(super) footprint: Arc<varena::working::GraphFootprint>,
}
#[pymethods]
impl PyGraphFootprint {
    #[new]
    fn new(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<Self> {
        check().map_err(|error| super::raise(py, error))?;
        Ok(Self {
            footprint: varena::working::GraphFootprint::new(device, pool)
                .map_err(|error| super::raise(py, error))?,
        })
    }
    fn _begin(&self, py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<()> {
        py.detach(|| self.footprint.begin(device, pool))
            .map_err(|error| super::raise(py, error))
    }
    fn _finish(&self, py: Python<'_>, completed: bool) -> PyResult<()> {
        py.detach(|| self.footprint.finish(completed))
            .map_err(|error| super::raise(py, error))
    }
    fn retire(&self) {
        self.footprint.retire();
    }
    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let facts = self
            .footprint
            .facts()
            .map_err(|error| super::raise(py, error))?;
        let row = PyDict::new(py);
        row.set_item("definition", facts.definition)?;
        row.set_item("device", facts.device)?;
        row.set_item("pool", facts.pool)?;
        row.set_item("sealed", facts.sealed)?;
        row.set_item("retired", facts.retired)?;
        row.set_item("traced_bytes", facts.traced_bytes)?;
        row.set_item("backing_bytes", facts.backing_bytes)?;
        row.set_item("allocations", facts.allocations)?;
        Ok(row)
    }
}
impl Drop for PyGraphFootprint {
    fn drop(&mut self) {
        self.footprint.retire();
        // An abandoned recorder owns metadata only. Cleanup must not issue CUDA operations.
        let _ = self.footprint.finish(false);
    }
}

#[pyfunction]
#[pyo3(signature = (device, key = None, *, mode = "eager"))]
fn _varena_phase_enter(
    py: Python<'_>,
    device: i32,
    key: Option<String>,
    mode: &str,
) -> PyResult<u64> {
    use varena::working::PhaseMode;
    let mode = match mode {
        "eager" => PhaseMode::Eager,
        "capture" => PhaseMode::Capture,
        "replay" => PhaseMode::Replay,
        "preparation" => PhaseMode::Preparation,
        _ => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "unknown execution phase mode",
            ));
        }
    };
    py.detach(|| {
        check()?;
        varena::working::phase_enter(device, key, mode)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
#[pyo3(signature = (device, token, *, completed, key = None))]
fn _varena_phase_leave<'py>(
    py: Python<'py>,
    device: i32,
    token: u64,
    completed: bool,
    key: Option<String>,
) -> PyResult<Bound<'py, PyDict>> {
    // Cleanup must remain possible after an allocator callback has latched its original error.
    let observed = py
        .detach(|| varena::working::phase_leave(device, token, completed, key))
        .map_err(|e| super::raise(py, e))?;
    observation(py, &observed)
}

pub(super) fn observation<'py>(
    py: Python<'py>,
    observed: &varena::working::PhaseObservation,
) -> PyResult<Bound<'py, PyDict>> {
    let row = PyDict::new(py);
    let splits = observed
        .split_observations
        .iter()
        .map(|item| split_observation(py, item))
        .collect::<PyResult<Vec<_>>>()?;
    row.set_item("split_observations", splits)?;
    row.set_item("definitions", &observed.definitions)?;
    row.set_item("observation_id", observed.token)?;
    row.set_item("mode", observed.mode.name())?;
    row.set_item("key", observed.key.as_deref())?;
    row.set_item("peak_bytes", observed.peak_bytes)?;
    row.set_item("total_peak_bytes", observed.total_peak_bytes)?;
    row.set_item("backing_peak_bytes", observed.backing_peak_bytes)?;
    row.set_item("published", observed.published)?;
    row.set_item("invalidated", observed.invalidated)?;
    row.set_item("reason", &observed.reason)?;
    row.set_item("successful_peak_bytes", observed.successful_peak_bytes)?;
    row.set_item(
        "successful_backing_peak_bytes",
        observed.successful_backing_peak_bytes,
    )?;
    row.set_item("successful_passes", observed.successful_passes)?;
    Ok(row)
}

#[pyfunction]
#[pyo3(signature = (device, token, reason = "execution_retry"))]
fn _varena_phase_invalidate(py: Python<'_>, device: i32, token: u64, reason: &str) -> PyResult<()> {
    py.detach(|| varena::working::phase_invalidate(device, token, reason))
        .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_phase_stats<'py>(
    py: Python<'py>,
    device: i32,
    key: &str,
) -> PyResult<Bound<'py, PyDict>> {
    let sample = py
        .detach(|| {
            check()?;
            varena::working::phase_stats(device, key)
        })
        .map_err(|e| super::raise(py, e))?;
    let row = PyDict::new(py);
    row.set_item("known", sample.known)?;
    row.set_item("peak_bytes", sample.peak_bytes)?;
    row.set_item("total_peak_bytes", sample.total_peak_bytes)?;
    row.set_item("backing_peak_bytes", sample.backing_peak_bytes)?;
    row.set_item("successful_passes", sample.successful_passes)?;
    row.set_item("aborted_passes", sample.aborted_passes)?;
    row.set_item("aborted_peak_bytes", sample.aborted_peak_bytes)?;
    row.set_item("aborted_total_peak_bytes", sample.aborted_total_peak_bytes)?;
    row.set_item(
        "aborted_backing_peak_bytes",
        sample.aborted_backing_peak_bytes,
    )?;
    Ok(row)
}

#[pyfunction]
fn _varena_phase_forget(py: Python<'_>, device: i32, key: &str) -> PyResult<bool> {
    py.detach(|| varena::working::phase_forget(device, key))
        .map_err(|e| super::raise(py, e))
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_varena_process_guard_status, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_context, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_select, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_begin, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_end, module)?)?;
    module.add_class::<PyGraphFootprint>()?;
    module.add_class::<CallbackScope>()?;
    module.add_class::<PyExecution>()?;
    module.add_function(wrap_pyfunction!(_varena_phase_enter, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_leave, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_invalidate, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_stats, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_forget, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_trim, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_set_limit, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_retire_stream, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_retain, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_release, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_bindings, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_initialize, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_check, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_stats, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_reset_peak, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_reserve, module)?)?;
    module.add_function(wrap_pyfunction!(
        _varena_working_pool_record_stream,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(_varena_working_capture_complete, module)?)?;
    module.add_function(wrap_pyfunction!(
        _varena_working_pool_cancel_preparation,
        module
    )?)?;
    Ok(())
}
