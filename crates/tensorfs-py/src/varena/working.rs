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
            varena::Error::OutOfMemory { .. }
            | varena::Error::Exhausted { .. }
            | varena::Error::CaptureMap { .. } => (1, 0),
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

/// What the card has now, as the split chooses by: capacity (the card or its cap, less what is
/// outside our bytes), live working bytes, pinned and resident region bytes, and what maps free.
#[pyfunction]
fn _varena_room(py: Python<'_>, device: i32) -> PyResult<std::collections::BTreeMap<&'static str, u64>> {
    let room = py.detach(|| { check()?; varena::working::room(device) }).map_err(|e| super::raise(py, e))?;
    Ok([
        ("capacity", room.capacity as u64),
        ("working_live", room.working_live as u64),
        ("pinned", room.pinned as u64),
        ("weights_resident", room.weights_resident as u64),
        ("free", room.free as u64),
    ].into_iter().collect())
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
        varena::working::alloc_bound(device, bytes, stream, (routed != 0).then_some((a, b)))
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
        varena::working::require_cooperation(device)?;
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
#[derive(Clone, Copy)]
struct Segment {
    device: i32,
    address: u64,
    bytes: usize,
    size_class: usize,
    pool_a: u64,
    pool_b: u64,
}

impl From<varena::working::WorkingSegment> for Segment {
    fn from(value: varena::working::WorkingSegment) -> Self {
        let (pool_a, pool_b) = value.pool.unwrap_or_default();
        Self { device: value.device, address: value.address, bytes: value.bytes, size_class: value.size_class, pool_a, pool_b }
    }
}

#[repr(C)]
struct SnapshotBlock {
    bytes: usize,
    requested: usize,
    allocated: i32,
    active: i32,
}

type Observe = unsafe extern "C" fn(i32, *const Segment, usize, *mut Failure) -> i32;
type Visit = unsafe extern "C" fn(*mut std::ffi::c_void, *const Segment, *const SnapshotBlock, usize, *mut Failure) -> i32;

struct Observer(Observe);

fn observer_error(error: &Failure) -> varena::Error {
    let end = error.message.iter().position(|byte| *byte == 0).unwrap_or(error.message.len());
    varena::Error::Poisoned(format!("Torch Working observer: {}", String::from_utf8_lossy(&error.message[..end])))
}

impl Observer {
    fn notify(&self, action: i32, segments: &[Segment]) -> varena::Result<()> {
        let mut failure = Failure { kind: 0, cuda: 0, message: [0; 512] };
        // SAFETY: the process-lifetime shim retains this callback. The borrowed array and
        // writable failure live through this synchronous non-reentrant, non-unwinding call.
        let result = unsafe { (self.0)(action, segments.as_ptr(), segments.len(), &mut failure) };
        if result == 1 && failure.kind == 0 { Ok(()) } else { Err(observer_error(&failure)) }
    }
}

impl varena::working::WorkingObserver for Observer {
    fn initialize(&self, segments: &[varena::working::WorkingSegment]) -> varena::Result<()> {
        self.notify(0, &segments.iter().copied().map(Segment::from).collect::<Vec<_>>())
    }
    fn mapped(&self, segment: varena::working::WorkingSegment) -> varena::Result<()> {
        self.notify(1, &[segment.into()])
    }
    fn retiring(&self, segment: varena::working::WorkingSegment) -> varena::Result<()> {
        self.notify(2, &[segment.into()])
    }
}

unsafe extern "C" fn observe(observer: Observe, failure: *mut Failure) -> i32 {
    call(failure, || varena::working::observe_working(Arc::new(Observer(observer))).map(|()| 1))
}

unsafe extern "C" fn snapshot(context: *mut std::ffi::c_void, visit: Visit, failure: *mut Failure) -> i32 {
    call(failure, || {
        for value in varena::working::working_snapshot()? {
            let segment = Segment::from(value.segment);
            let blocks = value.blocks.into_iter().map(|block| SnapshotBlock {
                bytes: block.bytes, requested: block.requested,
                allocated: i32::from(block.allocated), active: i32::from(block.active),
            }).collect::<Vec<_>>();
            let mut error = Failure { kind: 0, cuda: 0, message: [0; 512] };
            // SAFETY: the shim owns context/visitor for this synchronous traversal. All
            // descriptor buffers remain live and the visitor contains C++ exceptions.
            let result = unsafe { visit(context, &segment, blocks.as_ptr(), blocks.len(), &mut error) };
            if result != 1 || error.kind != 0 { return Err(observer_error(&error)); }
        }
        Ok(1)
    })
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
    observe: unsafe extern "C" fn(Observe, *mut Failure) -> i32,
    snapshot: unsafe extern "C" fn(*mut std::ffi::c_void, Visit, *mut Failure) -> i32,
}

static BINDINGS: Bindings = Bindings {
    abi: 2,
    size: std::mem::size_of::<Bindings>() as u32,
    alloc,
    free,
    record_stream,
    trim,
    pool_begin,
    pool_end,
    pool_release,
    observe,
    snapshot,
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
        varena::activate(device, true)
    })
    .map_err(|e| super::raise(py, e))
}

/// Runtime's Start: this GPU's context and this process's row, in the machine's mode.
/// How large a block a module may be before its children bind instead: one eviction unit.
#[pyfunction]
fn _varena_unit_bytes() -> usize {
    varena::working::UNIT_BYTES
}

/// A pool's size for a capture from an eager pass's measured peak.
#[pyfunction]
fn _varena_pool_bytes(peak: usize) -> usize {
    varena::working::pool_bytes(peak)
}

#[pyfunction]
fn _varena_set_checks(checks: u8) {
    varena::working::checks::set(checks);
}

#[pyfunction]
fn _varena_check_counts() -> (u64, u64, u64) {
    varena::working::checks::counts()
}

#[pyfunction]
fn _varena_poisoned(py: Python<'_>) -> PyResult<Option<String>> {
    py.detach(varena::working::poisoned).map_err(|e| super::raise(py, e))
}

unsafe extern "C" fn release_kept(context: *mut std::ffi::c_void) {
    // SAFETY: from `_varena_public_keep`, once. Off the GIL, PyO3 queues the decref: nothing waits.
    drop(unsafe { Box::from_raw(context.cast::<Py<PyAny>>()) });
}

/// A drop function and context holding `object` for native code to let go of from any thread.
#[pyfunction]
fn _varena_public_keep(object: Py<PyAny>) -> (usize, usize) {
    (release_kept as *const () as usize, Box::into_raw(Box::new(object)) as usize)
}

#[pyfunction]
fn _varena_activate(py: Python<'_>, device: i32, shared: bool) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::activate(device, shared)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_request_boundary(py: Python<'_>, device: i32, begin: bool) {
    py.detach(|| varena::request_boundary(device, begin));
}

#[pyfunction]
fn _varena_working_stats(
    py: Python<'_>,
    device: i32,
) -> PyResult<std::collections::BTreeMap<&'static str, u64>> {
    let (stats, weights_resident, weights_filled, driver_reservation) = py
        .detach(|| {
            check()?;
            Ok::<_, varena::Error>((
                varena::working::stats(device)?,
                varena::resident_bytes(device)?,
                varena::filled_bytes(device)?,
                varena::driver_reservation(device)?,
            ))
        })
        .map_err(|e| super::raise(py, e))?;
    Ok([
        ("weights_resident", weights_resident as u64),
        ("weights_filled", weights_filled),
        ("driver_reservation", driver_reservation as u64),
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
        ("driver_memory_ns", stats.driver_memory_ns),
        ("driver_memory_longest_ns", stats.driver_memory_longest_ns),
        ("observed_mapped", stats.observed_mapped),
        ("observed_retired", stats.observed_retired),
        ("observed_ns", stats.observed_ns),
        ("reclaim_ns", stats.reclaim_ns),
        ("reclaim_longest_ns", stats.reclaim_longest_ns),
        ("reclaim_longest_step", stats.reclaim_longest_step),
        ("reclaim_completed_working_ns", stats.reclaim_step_ns[0]),
        ("reclaim_contentless_ns", stats.reclaim_step_ns[1]),
        ("reclaim_idle_pools_ns", stats.reclaim_step_ns[2]),
        ("reclaim_pending_working_ns", stats.reclaim_step_ns[3]),
        ("reclaim_idle_peers_ns", stats.reclaim_step_ns[4]),
        ("reclaim_weights_ns", stats.reclaim_step_ns[5]),
        ("reclaim_join_ns", stats.reclaim_step_ns[6]),
        ("reclaim_owner_caches_ns", stats.reclaim_step_ns[7]),
        ("reclaim_peers_ns", stats.reclaim_step_ns[8]),
        ("reclaim_high_weights_ns", stats.reclaim_step_ns[9]),
        ("event_queries", stats.event_queries),
        ("event_records", stats.event_records),
        ("event_waits", stats.event_waits),
        ("context_joins", stats.context_joins),
        ("prefetch_attempts", stats.prefetch_attempts),
        ("prefetch_admissions", stats.prefetch_admissions),
        ("prefetch_declines", stats.prefetch_declines),
        ("split_decisions", stats.split_decisions),
        ("split_lookups", stats.split_lookups),
        ("room_moves", stats.room_moves),
        ("room_outside_moves", stats.room_outside_moves),
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
fn _varena_site_id(name: &str) -> u32 {
    varena::working::site_id(name)
}

/// One relaxed store on a model's block entry: where the next allocations are born.
#[pyfunction]
fn _varena_set_site(device: i32, site: u32) {
    varena::working::set_site(device, site);
}

fn sites<'py>(py: Python<'py>, sites: &[(String, usize)]) -> PyResult<Bound<'py, pyo3::types::PyList>> {
    pyo3::types::PyList::new(py, sites.iter().map(|(site, bytes)| (site.as_str(), *bytes)))
}

fn verdict<'py>(py: Python<'py>, verdict: &varena::working::Verdict) -> PyResult<Bound<'py, PyDict>> {
    let row = PyDict::new(py);
    row.set_item("need_bytes", verdict.need_bytes)?;
    row.set_item("room_bytes", verdict.room_bytes)?;
    row.set_item("held_bytes", verdict.held_bytes)?;
    row.set_item("escaped_bytes", verdict.escaped_bytes)?;
    row.set_item("transient_bytes", verdict.transient_bytes)?;
    row.set_item("floor_bytes", verdict.floor_bytes)?;
    row.set_item("learned", verdict.learned)?;
    row.set_item("sites", sites(py, &verdict.sites)?)?;
    Ok(row)
}

#[pyfunction]
fn _varena_pass_mark<'py>(
    py: Python<'py>, device: i32, head: u32, shape: u64, floor: usize,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let refused = varena::working::pass_mark(device, head, shape, floor).map_err(|e| super::raise(py, e))?;
    refused.map(|refused| verdict(py, &refused)).transpose()
}

#[pyfunction]
#[pyo3(signature = (device, *, failed))]
fn _varena_escapes_take<'py>(py: Python<'py>, device: i32, failed: bool) -> PyResult<Vec<Bound<'py, PyDict>>> {
    let records = varena::working::escapes_take(device, failed).map_err(|e| super::raise(py, e))?;
    records.iter().map(|record| {
        let row = PyDict::new(py);
        row.set_item("head", &record.head)?;
        row.set_item("passes", record.passes)?;
        row.set_item("carried_bytes", record.carried_bytes)?;
        row.set_item("carried_sites", sites(py, &record.carried_sites)?)?;
        row.set_item("escaped_bytes", record.escaped_bytes)?;
        row.set_item("escaped_peak_bytes", record.escaped_peak_bytes)?;
        row.set_item("escaped_sites", sites(py, &record.escaped_sites)?)?;
        row.set_item("growth_bytes", record.growth_bytes)?;
        row.set_item("transient_bytes", record.transient_bytes)?;
        row.set_item("refused", record.refused.as_ref().map(|refused| verdict(py, refused)).transpose()?)?;
        Ok(row)
    }).collect()
}

#[pyfunction]
fn _varena_fail_create_after(py: Python<'_>, device: i32, n: u64, times: u64) -> PyResult<()> {
    py.detach(|| varena::fail_create_after(device, n, times)).map_err(|e| super::raise(py, e))
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
    end: bool,
) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_record_stream(device, pool, stream, end)
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
fn _varena_working_pool_close(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_close(device, pool)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_short(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<usize> {
    py.detach(|| varena::working::pool_short(device, pool)).map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_arm(py: Python<'_>, device: i32, pool: (u64, u64), primed: Vec<u64>) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_arm(device, pool, &primed)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_pool_scratch(py: Python<'_>, device: i32, pool: (u64, u64), held: Vec<u64>) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_scratch(device, pool, &held)
    })
    .map_err(|e| super::raise(py, e))
}

#[pyfunction]
fn _varena_working_library_workspaces(py: Python<'_>, device: i32, sizes: Vec<usize>) -> PyResult<()> {
    py.detach(|| varena::working::library_workspaces(device, sizes))
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
#[pyo3(signature = (device, key, choices, *, stream))]
fn _varena_split_select<'py>(
    py: Python<'py>,
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    stream: u64,
) -> PyResult<Bound<'py, PyDict>> {
    let selected = py
        .detach(|| {
            check()?;
            varena::working::split_select(device, key, choices, stream)
        })
        .map_err(|error| super::raise(py, error))?;
    selection(py, &selected)
}

/// The level a pure op runs again at after its leaf was refused memory (Retry), None (final: no
/// allocation of that leaf was refused), or `Exhausted` (final: no finer level has room).
#[pyfunction]
#[pyo3(signature = (device, key, choice_id, *, stream))]
fn _varena_split_refused<'py>(
    py: Python<'py>,
    device: i32,
    key: &str,
    choice_id: &str,
    stream: u64,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let selected = py
        .detach(|| {
            check()?;
            varena::working::split_refused(device, key, choice_id, stream)
        })
        .map_err(|error| super::raise(py, error))?;
    selected.map(|selected| selection(py, &selected)).transpose()
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
    row.set_item("choice_id", &selected.choice_id)?;
    row.set_item("eligible", &selected.eligible)?;
    row.set_item("required_bytes", selected.required_bytes)?;
    row.set_item("available_bytes", selected.available_bytes)?;
    row.set_item("basis", selected.basis)?;
    row.set_item("plan_generation", selected.plan_generation)?;
    row.set_item("phase", &selected.phase)?;
    row.set_item("phase_weight_bytes", selected.phase_weight_bytes)?;
    row.set_item("budget_bytes", selected.budget_bytes)?;
    row.set_item("capacity_bytes", selected.capacity_bytes)?;
    row.set_item("live_bytes", selected.live_bytes)?;
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
    .map_err(|error| super::raise(py, error))
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
    row.set_item("refused_bytes", observed.refused_bytes)?;
    Ok(row)
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
fn _varena_phase_forget(py: Python<'_>, device: i32, key: &str) -> PyResult<bool> {
    py.detach(|| varena::working::phase_forget(device, key))
        .map_err(|e| super::raise(py, e))
}

static GIL_CHECK: OnceLock<unsafe extern "C" fn() -> std::ffi::c_int> = OnceLock::new();

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    // Owner callbacks need the GIL, so an allocation holding it must not wait for them.
    // PyGILState_Check is outside the limited API (Windows' python3.dll lacks it), so it is
    // this interpreter's own, found through ctypes.
    let ctypes = module.py().import("ctypes")?;
    let check = ctypes.getattr("pythonapi")?.getattr("PyGILState_Check")?;
    let address: usize = ctypes.call_method1("cast", (check, ctypes.getattr("c_void_p")?))?.getattr("value")?.extract()?;
    // SAFETY: CPython's `int PyGILState_Check(void)`, callable from any thread; it only reads this thread's state.
    let _ = GIL_CHECK.set(unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> std::ffi::c_int>(address) });
    varena::set_host_lock_probe(|| GIL_CHECK.get().is_some_and(|check| unsafe { check() } != 0));
    module.add_function(wrap_pyfunction!(_varena_room, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_context, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_select, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_begin, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_refused, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_split_end, module)?)?;
    module.add_class::<CallbackScope>()?;
    module.add_function(wrap_pyfunction!(_varena_phase_enter, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_leave, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_invalidate, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_phase_forget, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_set_limit, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_fail_create_after, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_retire_stream, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_arm, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_short, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_close, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_scratch, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_library_workspaces, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_retain, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_pool_release, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_bindings, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_initialize, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_activate, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_poisoned, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_set_checks, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_check_counts, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_unit_bytes, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_pool_bytes, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_public_keep, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_request_boundary, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_stats, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_working_reset_peak, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_site_id, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_set_site, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_pass_mark, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_escapes_take, module)?)?;
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
