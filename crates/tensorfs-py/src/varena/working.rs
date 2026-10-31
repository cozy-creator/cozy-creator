//! Torch callbacks contain no Python; Rust owns allocation and completion state.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use pyo3::prelude::*;

static OWNER: AtomicU32 = AtomicU32::new(0);
static CALLBACK_ERROR: Mutex<Option<varena::Error>> = Mutex::new(None);

#[repr(C)]
struct Failure {
    kind: i32,
    cuda: i32,
    message: [u8; 512],
}

impl Failure {
    fn from_error(error: &varena::Error) -> Self {
        let (kind, cuda) = match error {
            varena::Error::OutOfMemory { .. } => (1, 0),
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

fn check() -> varena::Result<()> {
    if OWNER.load(Ordering::Acquire) != std::process::id() {
        return Err(varena::Error::Invalid(
            "Torch allocator is not installed in this process",
        ));
    }
    let failure = CALLBACK_ERROR
        .lock()
        .map_err(|_| varena::Error::Poisoned("allocator callback status".into()))?;
    if let Some(error) = failure.as_ref() {
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
            if let Ok(mut recorded) = CALLBACK_ERROR.lock() {
                recorded.get_or_insert_with(|| error.clone());
            }
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
    let pid = std::process::id();
    let owner = OWNER.load(Ordering::Acquire);
    if owner != 0 && owner != pid {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "Torch allocator cannot be installed after fork",
        ));
    }
    let _install = super::allocator::INSTALL.lock().map_err(|_| {
        pyo3::exceptions::PyRuntimeError::new_err("allocator installation lock poisoned")
    })?;
    if super::allocator::installed() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "the legacy CUDA allocation hook is already installed",
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
    check().map_err(|e| super::raise(py, e))
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
        ("unassigned_bytes", stats.unassigned_bytes as u64),
        ("pools", stats.pools as u64),
        ("event_handles", stats.event_handles as u64),
        ("allocations", stats.allocations),
        ("frees", stats.frees),
        ("ooms", stats.ooms),
        ("driver_memory_calls", stats.driver_memory_calls),
        ("event_queries", stats.event_queries),
        ("event_records", stats.event_records),
        ("event_waits", stats.event_waits),
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
fn _varena_working_pool_release(py: Python<'_>, device: i32, pool: (u64, u64)) -> PyResult<()> {
    py.detach(|| {
        check()?;
        varena::working::pool_release(device, pool)
    })
    .map_err(|e| super::raise(py, e))
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
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
