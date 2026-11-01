//! Weak Python cache owners. The GIL is acquired only on the separate callback worker.
//! Runtime's callback must claim its owner/domain nonblocking before dropping GPU references.
use super::*;
use pyo3::types::PyBool;
use varena::cache_owner::{CacheCallback, CallbackResult, CallbackScope, Cause, RequestKey};

pyo3::create_exception!(tensorfs, CacheOwnerDeferredError, PyRuntimeError);

pub(super) fn owner_error(py: Python<'_>, error: varena::Error) -> PyErr {
    match error {
        varena::Error::CaptureActive | varena::Error::Busy => {
            let reason = if matches!(error, varena::Error::CaptureActive) {
                "capture"
            } else {
                "busy"
            };
            let exception = CacheOwnerDeferredError::new_err(error.to_string());
            if let Err(error) = exception.value(py).setattr("reason", reason) {
                return error;
            }
            exception
        }
        _ => raise(py, error),
    }
}

fn callback_error(py: Python<'_>, error: PyErr) -> varena::Error {
    if error.is_instance_of::<CacheOwnerDeferredError>(py) {
        let reason = error
            .value(py)
            .getattr("reason")
            .and_then(|reason| reason.extract::<String>());
        return if reason.is_ok_and(|reason| reason == "capture") {
            varena::Error::CaptureActive
        } else {
            varena::Error::Busy
        };
    }
    varena::Error::Callback(error.to_string())
}

struct PythonCallback {
    // Created by weakref.ref with no weakref callback: this object owns no GPU resource.
    callback: Option<Py<PyAny>>,
}
impl CacheCallback for PythonCallback {
    fn shrink(&mut self, scope: &CallbackScope, bytes: usize) -> varena::Result<CallbackResult> {
        Python::try_attach(|py| {
            // Never acquire exclusion while waiting for the allocating caller's GIL.
            // Strong temporary Python targets are born/dropped within capture exclusion;
            // other sequences retain their independent replay entry and physical Pins.
            let _guard = scope.producing()?;
            let callback = self
                .callback
                .as_ref()
                .ok_or(varena::Error::Invalid("cache callback is retired"))?
                .bind(py)
                .call0()
                .map_err(|error| callback_error(py, error))?;
            if callback.is_none() {
                return Ok(CallbackResult::Declined);
            }
            let value = callback
                .call1((bytes,))
                .map_err(|error| callback_error(py, error))?;
            Ok(
                if value.is_instance_of::<PyBool>() && !value.is_truthy().unwrap_or(true) {
                    CallbackResult::Declined
                } else {
                    CallbackResult::Returned
                },
            )
        })
        .unwrap_or(Err(varena::Error::Busy))
    }

    fn retire(&mut self, scope: &CallbackScope) -> varena::Result<()> {
        Python::try_attach(|py| {
            let _guard = scope.producing()?;
            if let Some(callback) = self.callback.take() {
                callback.drop_ref(py);
            }
            Ok(())
        })
        .unwrap_or(Err(varena::Error::Busy))
    }
}

#[pyclass(name = "_VarenaCacheOwner", module = "tensorfs._ext", frozen)]
pub(super) struct PyCacheOwner {
    owner: Deferred<varena::CacheOwner>,
}

#[pymethods]
impl PyCacheOwner {
    #[classattr]
    const CACHE_OWNER_PROTOCOL: u32 = 1;

    #[new]
    fn new(py: Python<'_>, device: i32, callback: &Bound<'_, PyAny>) -> PyResult<Self> {
        if !callback.is_callable() {
            return Err(PyValueError::new_err("cache shrinker must be callable"));
        }
        let callback = py
            .import("weakref")?
            .getattr("ref")?
            .call1((callback,))?
            .unbind();
        let owner = py
            .detach(|| {
                varena::CacheOwner::with_callback(
                    device,
                    Box::new(PythonCallback {
                        callback: Some(callback),
                    }),
                )
            })
            .map_err(|error| owner_error(py, error))?;
        Ok(Self {
            owner: Deferred::new(owner),
        })
    }

    fn touch(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.owner.touch())
            .map_err(|error| owner_error(py, error))
    }
    fn check(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.owner.check())
            .map_err(|error| owner_error(py, error))
    }
    fn suspend(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.owner.suspend())
            .map_err(|error| owner_error(py, error))
    }
    fn defer_close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.owner.defer_close())
            .map_err(|error| owner_error(py, error))
    }
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.owner.close())
            .map_err(|error| owner_error(py, error))
    }
    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let value = py
            .detach(|| self.owner.facts())
            .map_err(|error| owner_error(py, error))?;
        let row = PyDict::new(py);
        row.set_item("id", value.id)?;
        row.set_item("registered", value.registered)?;
        row.set_item("requestable", value.requestable)?;
        row.set_item("closing", value.closing)?;
        row.set_item("running", value.running)?;
        row.set_item("retired", value.retired)?;
        row.set_item("callbacks", value.callbacks)?;
        row.set_item("returned", value.returned)?;
        row.set_item("declined", value.declined)?;
        row.set_item("deferred", value.deferred)?;
        row.set_item("failed", value.failed)?;
        row.set_item("retirement_failed", value.retirement_failed)?;
        Ok(row)
    }
}

#[pyclass(name = "_VarenaCacheRequest", module = "tensorfs._ext", frozen)]
pub(super) struct PyCacheRequest {
    request: varena::cache_owner::ShrinkRequest,
}
#[pymethods]
impl PyCacheRequest {
    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let value = py
            .detach(|| self.request.facts())
            .map_err(|error| owner_error(py, error))?;
        let row = PyDict::new(py);
        row.set_item("queued", value.queued)?;
        row.set_item("running", value.running)?;
        row.set_item("returned", value.returned)?;
        row.set_item("declined", value.declined)?;
        row.set_item("failed", value.failed)?;
        row.set_item("finished", value.finished)?;
        row.set_item("cancelled", value.cancelled)?;
        row.set_item("observed_release_bytes", value.observed_release_bytes)?;
        Ok(row)
    }
    fn cancel(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.request.cancel())
            .map_err(|error| owner_error(py, error))
    }
}

#[pyfunction]
fn _varena_cache_request(
    py: Python<'_>,
    device: i32,
    bytes: usize,
) -> PyResult<Option<PyCacheRequest>> {
    py.detach(|| {
        let key = RequestKey {
            cause: Cause::Allocation,
            generation: varena::cache_owner::next_generation()?,
        };
        varena::cache_owner::request_shrink(device, key, bytes)
    })
    .map(|request| request.map(|request| PyCacheRequest { request }))
    .map_err(|error| owner_error(py, error))
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyCacheOwner>()?;
    module.add_class::<PyCacheRequest>()?;
    module.add_function(wrap_pyfunction!(_varena_cache_request, module)?)?;
    module.add(
        "CacheOwnerDeferredError",
        module.py().get_type::<CacheOwnerDeferredError>(),
    )?;
    Ok(())
}
