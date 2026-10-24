//! Process-local weight residency bindings. CUDA ownership remains in the Rust core.
mod allocator;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    ffi,
    prelude::*,
    types::PyDict,
};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

pyo3::create_exception!(tensorfs, VarenaPoisonedError, PyRuntimeError);

fn raise(py: Python<'_>, error: varena::Error) -> PyErr {
    let message = error.to_string();
    let class = match error {
        varena::Error::OutOfMemory { .. } => "OutOfMemoryError",
        varena::Error::Poisoned(_) => return VarenaPoisonedError::new_err(message),
        varena::Error::DeviceFault { .. } | varena::Error::Cuda { .. } => "AcceleratorError",
        _ => return PyRuntimeError::new_err(message),
    };
    let result = (|| -> PyResult<PyErr> {
        let class = py.import("torch")?.getattr(class)?;
        // Torch's exception types take the ordinary RuntimeError constructor arguments.
        // The CUDA error code is an attribute, not an accepted constructor keyword.
        let exception = class.call1((message.clone(),))?;
        if let varena::Error::DeviceFault { code, .. } | varena::Error::Cuda { code, .. } = error {
            exception.setattr("error_code", code)?;
        }
        Ok(PyErr::from_value(exception))
    })();
    result.unwrap_or_else(|_| PyRuntimeError::new_err(message))
}

#[pyclass(name = "_VarenaArena", module = "tensorfs._ext", frozen)]
struct PyArena {
    arena: Arc<varena::Arena>,
}
#[pymethods]
impl PyArena {
    #[new]
    fn new(py: Python<'_>, device: i32) -> PyResult<Self> {
        Ok(Self {
            arena: py
                .detach(|| varena::Arena::new(device))
                .map_err(|e| raise(py, e))?,
        })
    }
    #[getter]
    fn granularity(&self) -> usize {
        self.arena.granularity()
    }
    fn reserve(&self, py: Python<'_>, bytes: usize) -> PyResult<PyRegion> {
        Ok(PyRegion {
            region: py
                .detach(|| self.arena.reserve(bytes))
                .map_err(|e| raise(py, e))?,
        })
    }
    fn set_streaming_limit(&self, py: Python<'_>, bytes: usize) -> PyResult<()> {
        self.arena
            .set_streaming_limit(bytes)
            .map_err(|e| raise(py, e))
    }
    fn reclaim(&self, py: Python<'_>, bytes: usize) -> PyResult<usize> {
        py.detach(|| self.arena.reclaim(bytes))
            .map(|r| r.reclaimed_bytes)
            .map_err(|e| raise(py, e))
    }
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let stats = self.arena.stats().map_err(|e| raise(py, e))?;
        let output = PyDict::new(py);
        for (name, value) in [
            ("reserved_bytes", stats.reserved_bytes as u64),
            ("resident_bytes", stats.resident_bytes as u64),
            ("ready_bytes", stats.ready_bytes as u64),
            ("pinned_bytes", stats.pinned_bytes as u64),
            (
                "reclaimable_bytes",
                stats.resident_bytes.saturating_sub(stats.pinned_bytes) as u64,
            ),
            ("preferred_bytes", stats.preferred_bytes as u64),
            ("regions", stats.regions as u64),
            ("fills", stats.fills),
            ("hits", stats.hits),
            ("evictions", stats.evictions),
            ("reclaimed_bytes", stats.reclaimed_bytes),
        ] {
            output.set_item(name, value)?;
        }
        output.set_item("poisoned", stats.poisoned)?;
        Ok(output)
    }
    fn enter_capture(&self, py: Python<'_>) -> PyResult<PyCapture> {
        Ok(PyCapture {
            guard: Mutex::new(Some(self.arena.enter_capture().map_err(|e| raise(py, e))?)),
        })
    }
}

#[pyclass(name = "_VarenaCapture", module = "tensorfs._ext", frozen)]
struct PyCapture {
    guard: Mutex<Option<varena::CaptureGuard>>,
}
#[pymethods]
impl PyCapture {
    fn close(&self) -> PyResult<()> {
        self.guard
            .lock()
            .map_err(|_| PyRuntimeError::new_err("capture guard poisoned"))?
            .take();
        Ok(())
    }
}

#[pyclass(name = "_VarenaRegion", module = "tensorfs._ext", frozen)]
struct PyRegion {
    region: Arc<varena::Region>,
}
#[pymethods]
impl PyRegion {
    #[getter]
    fn resident(&self, py: Python<'_>) -> PyResult<bool> {
        self.region.is_resident().map_err(|e| raise(py, e))
    }
    #[getter]
    fn preferred(&self, py: Python<'_>) -> PyResult<bool> {
        self.region.is_preferred().map_err(|e| raise(py, e))
    }
    #[getter]
    fn allocated_bytes(&self) -> usize {
        self.region.allocated_bytes()
    }
    #[getter]
    fn ptr(&self) -> u64 {
        self.region.ptr()
    }
    fn set_preferred(&self, py: Python<'_>, preferred: bool) -> PyResult<()> {
        self.region
            .set_preferred(preferred)
            .map_err(|e| raise(py, e))
    }
    fn prepare(&self, py: Python<'_>) -> PyResult<PyUse> {
        let preparation = py
            .detach(|| self.region.prepare())
            .map_err(|e| raise(py, e))?;
        let needs_fill = matches!(preparation, varena::Preparation::Fill(_));
        Ok(PyUse {
            preparation: Mutex::new(Some(preparation)),
            needs_fill,
            device: self.region.device(),
        })
    }
}

#[pyclass(name = "_VarenaUse", module = "tensorfs._ext", frozen)]
struct PyUse {
    preparation: Mutex<Option<varena::Preparation>>,
    needs_fill: bool,
    device: i32,
}
#[pymethods]
impl PyUse {
    #[getter]
    fn needs_fill(&self) -> bool {
        self.needs_fill
    }
    fn commit(&self, py: Python<'_>) -> PyResult<()> {
        // Never retain this mutex while detach reacquires the GIL: another Python thread
        // may export the same Use meanwhile. A commit in progress refuses that export.
        let current = self
            .preparation
            .lock()
            .map_err(|_| PyRuntimeError::new_err("fill owner poisoned"))?
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("fill is committing or already failed"))?;
        let ready = match current {
            varena::Preparation::Resident(pin) => varena::Preparation::Resident(pin),
            varena::Preparation::Fill(fill) => {
                // SAFETY: the Python Use API requires the complete byte tensor to be filled before
                // commit; the core joins every CUDA stream before publishing ready contents.
                let pin = py
                    .detach(|| unsafe { fill.commit_external() })
                    .map_err(|e| raise(py, e))?;
                varena::Preparation::Resident(pin)
            }
        };
        *self
            .preparation
            .lock()
            .map_err(|_| PyRuntimeError::new_err("fill owner poisoned"))? = Some(ready);
        Ok(())
    }
    fn __dlpack_device__(&self) -> (i32, i32) {
        (2, self.device)
    }
    #[pyo3(signature = (stream = None, max_version = None, dl_device = None, copy = None))]
    fn __dlpack__<'py>(
        &self,
        py: Python<'py>,
        stream: Option<i64>,
        max_version: Option<(u32, u32)>,
        dl_device: Option<(i32, i32)>,
        copy: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let _ = (stream, max_version); // Ready fills are context-synchronized; empty fills have no producer.
        if copy == Some(true) || dl_device.is_some_and(|d| d != (2, self.device)) {
            return Err(PyValueError::new_err(
                "Varena exports its original CUDA storage only",
            ));
        }
        let preparation = self
            .preparation
            .lock()
            .map_err(|_| PyRuntimeError::new_err("fill owner poisoned"))?;
        let pin = match preparation
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("fill failed"))?
        {
            varena::Preparation::Resident(pin) => pin.try_clone(),
            varena::Preparation::Fill(fill) => fill.pin(),
        }
        .map_err(|e| raise(py, e))?;
        capsule(py, pin)
    }
}

#[repr(C)]
struct Device {
    kind: i32,
    ordinal: i32,
}
#[repr(C)]
struct Dtype {
    code: u8,
    bits: u8,
    lanes: u16,
}
#[repr(C)]
struct Tensor {
    data: *mut c_void,
    device: Device,
    ndim: i32,
    dtype: Dtype,
    shape: *mut i64,
    strides: *mut i64,
    offset: u64,
}
#[repr(C)]
struct Managed {
    tensor: Tensor,
    context: *mut c_void,
    deleter: Option<unsafe extern "C" fn(*mut Managed)>,
}
struct Held {
    _pin: varena::Pin,
    length: i64,
}

unsafe extern "C" fn delete(tensor: *mut Managed) {
    if tensor.is_null() {
        return;
    }
    // SAFETY: a DLPack consumer owns these two boxes exactly once after capsule consumption.
    let tensor = unsafe { Box::from_raw(tensor) };
    // SAFETY: capsule creates context as a Held box; its physical Pin outlives every tensor alias.
    drop(unsafe { Box::from_raw(tensor.context as *mut Held) });
}
unsafe extern "C" fn capsule_delete(capsule: *mut ffi::PyObject) {
    // SAFETY: Python calls this on a live capsule. Consumed capsules have a different name.
    unsafe {
        if ffi::PyCapsule_IsValid(capsule, c"dltensor".as_ptr()) == 1 {
            delete(ffi::PyCapsule_GetPointer(capsule, c"dltensor".as_ptr()) as *mut Managed);
        }
    }
}
fn capsule(py: Python<'_>, pin: varena::Pin) -> PyResult<Bound<'_, PyAny>> {
    let length = i64::try_from(pin.len())
        .map_err(|_| PyValueError::new_err("tensor length exceeds DLPack i64"))?;
    let (pointer, device) = (pin.ptr(), pin.device());
    let held = Box::into_raw(Box::new(Held { _pin: pin, length }));
    let tensor = Box::into_raw(Box::new(Managed {
        tensor: Tensor {
            data: pointer as usize as *mut c_void,
            device: Device {
                kind: 2,
                ordinal: device,
            },
            ndim: 1,
            dtype: Dtype {
                code: 1,
                bits: 8,
                lanes: 1,
            },
            // SAFETY: the shape cell stays at a stable address in the owned Held box.
            shape: unsafe { &mut (*held).length },
            strides: std::ptr::null_mut(),
            offset: 0,
        },
        context: held.cast(),
        deleter: Some(delete),
    }));
    // SAFETY: capsule uniquely owns tensor until consumed; errors release both boxes here.
    unsafe {
        let capsule = ffi::PyCapsule_New(tensor.cast(), c"dltensor".as_ptr(), Some(capsule_delete));
        if capsule.is_null() {
            delete(tensor);
            return Err(PyErr::fetch(py));
        }
        Ok(Bound::from_owned_ptr(py, capsule))
    }
}

#[pyclass(name = "_VarenaAllocatorCapture", module = "tensorfs._ext", frozen)]
struct PyAllocatorCapture {
    guard: Mutex<Option<varena::GlobalCaptureGuard>>,
}
#[pymethods]
impl PyAllocatorCapture {
    #[new]
    fn new(py: Python<'_>, device: i32) -> PyResult<Self> {
        let native = varena::enter_capture(device).map_err(|e| raise(py, e))?;
        Ok(Self {
            guard: Mutex::new(Some(native)),
        })
    }
    fn close(&self) -> PyResult<()> {
        self.guard
            .lock()
            .map_err(|_| PyRuntimeError::new_err("capture guard poisoned"))?
            .take();
        Ok(())
    }
}

#[pyfunction]
fn _varena_install_allocator(py: Python<'_>) -> PyResult<usize> {
    py.detach(allocator::install)
        .map_err(PyRuntimeError::new_err)
}
#[pyfunction]
#[pyo3(signature = (device, limit = None))]
fn _varena_set_activation_limit(device: i32, limit: Option<usize>) -> PyResult<()> {
    allocator::set_limit(device, limit).map_err(PyRuntimeError::new_err)
}
#[pyfunction]
fn _varena_allocator_stats(device: i32) -> PyResult<std::collections::BTreeMap<&'static str, u64>> {
    allocator::stats(device).map_err(PyRuntimeError::new_err)
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    let poisoned = module.py().get_type::<VarenaPoisonedError>();
    poisoned.setattr("__module__", "tensorfs._ext")?;
    poisoned.setattr("sticky", true)?;
    module.add("VarenaPoisonedError", poisoned)?;
    module.add_class::<PyArena>()?;
    module.add_class::<PyRegion>()?;
    module.add_class::<PyUse>()?;
    module.add_class::<PyCapture>()?;
    module.add_class::<PyAllocatorCapture>()?;
    module.add_function(wrap_pyfunction!(_varena_install_allocator, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_set_activation_limit, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_allocator_stats, module)?)?;
    Ok(())
}
