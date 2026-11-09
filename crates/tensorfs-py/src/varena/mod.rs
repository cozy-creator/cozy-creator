//! Process-local weight residency bindings. CUDA ownership remains in the Rust core.
mod working;
mod tensor_export;
mod cache_owner;
mod plan;
#[cfg(feature = "store")]
mod continuation;
#[cfg(feature = "store")]
mod host;
#[cfg(feature = "store")]
mod pages;
#[cfg(feature = "store")]
mod phase;
#[cfg(feature = "store")]
mod retained;
#[cfg(feature = "store")]
mod source;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    ffi,
    prelude::*,
    types::PyDict,
};
use std::ffi::c_void;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

pyo3::create_exception!(tensorfs, VarenaPoisonedError, PyRuntimeError);
pyo3::create_exception!(tensorfs, PhaseOverlapError, PyRuntimeError);

// Final CUDA owner teardown may join streams or take native locks. Python/native
// destructors move that owner to the existing cleanup worker, without acquiring Python.
struct Deferred<T: Send + 'static> {
    value: Option<T>,
    pid: u32,
}
impl<T: Send + 'static> Deferred<T> {
    fn new(value: T) -> Self {
        Self {
            value: Some(value),
            pid: std::process::id(),
        }
    }
}
impl<T: Send + 'static> Deref for Deferred<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value.as_ref().expect("live native owner")
    }
}
impl<T: Send + 'static> Drop for Deferred<T> {
    fn drop(&mut self) {
        if let Some(owner) = self.value.take() {
            if self.pid == std::process::id() {
                crate::cleanup::defer(owner);
            } else {
                std::mem::forget(owner);
            }
        }
    }
}

pub(super) fn release_region(region: Arc<varena::Region>) {
    // Unlike a strong-count check followed by drop, into_inner atomically either releases a
    // shared reference or gives us the final Region without running its potentially blocking Drop.
    if let Some(region) = Arc::into_inner(region) {
        crate::cleanup::defer(region.retire());
    }
}

/// Streams declared for one Use family: the Use and every DLPack export taken from it.
#[derive(Clone)]
struct Consumers(varena::StreamUses);
impl Consumers {
    fn new(device: i32) -> Self { Self(varena::StreamUses::new(device)) }
    fn declare(&self, stream: u64) -> varena::Result<()> { self.0.declare(stream) }
    /// The Use's original lease. Every published storage has its own tracked Pin.
    fn release(&self, pin: varena::Pin) {
        let declared = match self.0.declared() {
            Ok(declared) => declared,
            Err(error) => { varena::retain_unproven_pin(pin, error); return; }
        };
        if !declared {
            // Preserve the existing unannotated raw-Use contract. No foreign storage
            // is exported without a declaration; raw-pointer users still require a
            // checked context join before native reclamation.
            release_region(pin.into_unpinned_region());
            return;
        }
        // SAFETY: consume_on declares users of this lease before its final release.
        finish_pin(unsafe { self.0.release_pin(pin) });
    }
}

fn finish_pin(result: std::result::Result<Arc<varena::Region>, (varena::Pin, varena::Error)>) {
    match result {
        Ok(region) => release_region(region),
        Err((pin, error)) => varena::retain_unproven_pin(pin, error),
    }
}

/// One physical use, structurally shared by the private Use and every storage
/// exported from it. Temporary views never independently retire cloned Pins.
struct SharedPin {
    pin: Option<varena::Pin>,
    consumers: Consumers,
    family: Mutex<Option<varena::TensorConsumers>>,
    pid: u32,
}
impl SharedPin {
    fn new(pin: varena::Pin, consumers: Consumers) -> Arc<Self> {
        Arc::new(Self { pin: Some(pin), consumers, family: Mutex::new(None), pid: std::process::id() })
    }
    fn pin(&self) -> &varena::Pin { self.pin.as_ref().expect("owned shared Pin") }
    fn register(&self) -> varena::Result<()> {
        let mut family = self.family.lock().map_err(|_| varena::Error::Poisoned("private storage family".into()))?;
        if family.is_none() {
            *family = Some(varena::TensorConsumers::from_streams(self.pin(), self.consumers.0.clone())?);
        }
        Ok(())
    }
}
impl Drop for SharedPin {
    fn drop(&mut self) {
        let Some(pin) = self.pin.take() else { return; };
        if self.pid != std::process::id() { std::mem::forget(pin); return; }
        let family = match self.family.get_mut() {
            Ok(family) => family.take(),
            Err(_) => {
                varena::retain_unproven_pin(pin, varena::Error::Poisoned("private storage family".into()));
                return;
            }
        };
        if let Some(family) = family {
            // SAFETY: this is the final shared owner: both the Use and every
            // independent exported Storage have gone. No sibling-count inference.
            finish_pin(unsafe { family.release(pin) });
        } else {
            self.consumers.release(pin);
        }
    }
}

enum UsePreparation {
    Resident(Arc<SharedPin>),
    Fill { fill: varena::Fill, exported: Option<Arc<SharedPin>> },
}

fn raise(py: Python<'_>, error: varena::Error) -> PyErr {
    let message = error.to_string();
    let class = match error {
        varena::Error::Callback(_) => return PyRuntimeError::new_err(message),
        // Nothing more could be reclaimed, before the work ran: a typed refusal that is still
        // torch's OOM (`tensorfs.varena.torch._refused`).
        varena::Error::Exhausted { requested_bytes, available_bytes } => {
            return refused(py, &message, requested_bytes, available_bytes);
        }
        varena::Error::OutOfMemory { requested_bytes } => return refused(py, &message, requested_bytes, 0),
        varena::Error::CaptureMap { .. } => "OutOfMemoryError",
        varena::Error::Poisoned(_) => return VarenaPoisonedError::new_err(message),
        varena::Error::PhaseOverlap => return PhaseOverlapError::new_err(message),
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

fn refused(py: Python<'_>, message: &str, requested: usize, available: usize) -> PyErr {
    let built = (|| -> PyResult<PyErr> {
        let make = py.import("tensorfs.varena.torch")?.getattr("_refused")?;
        Ok(PyErr::from_value(make.call1((message, requested, available))?))
    })();
    built.unwrap_or_else(|_| PyRuntimeError::new_err(message.to_owned()))
}

#[pyclass(name = "_VarenaDevice", module = "tensorfs._ext", frozen)]
struct PyDevice {
    device: &'static varena::Device,
    pid: u32,
}

impl PyDevice {
    fn owner(&self) -> PyResult<&'static varena::Device> {
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err("a CUDA device cannot be used after fork"));
        }
        Ok(self.device)
    }
}

#[pymethods]
impl PyDevice {
    #[new]
    fn new(py: Python<'_>, device: i32) -> PyResult<Self> {
        let device = py
            .detach(|| varena::Device::open(device))
            .map_err(|e| raise(py, e))?;
        Ok(Self { device, pid: std::process::id() })
    }
    #[getter]
    fn granularity(&self) -> PyResult<usize> {
        Ok(self.owner()?.granularity())
    }
    /// A Region of `bytes` from `source`: address space only, until it is used.
    #[cfg(feature = "store")]
    fn region(&self, py: Python<'_>, bytes: usize, source: PyRef<'_, source::PySource>) -> PyResult<PyRegion> {
        let device = self.owner()?;
        let inner = source.inner.clone();
        let bound = inner.clone();
        let region = py.detach(|| device.region(bytes, Arc::new(source::PageSource(inner))))
            .map_err(|e| raise(py, e))?;
        source::bind(&bound, &region);
        Ok(PyRegion { region: Deferred::new(region) })
    }
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let device = self.owner()?;
        let stats = py.detach(|| device.stats()).map_err(|e| raise(py, e))?;
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
            ("regions", stats.regions as u64),
            ("fills", stats.fills),
            ("hits", stats.hits),
            ("evictions", stats.evictions),
            ("reclaimed_bytes", stats.reclaimed_bytes),
            ("prefetch_unused_bytes", stats.prefetch_unused_bytes),
            ("context_joins", stats.context_joins),
            ("driver_physical_ooms", stats.driver_physical_ooms),
            ("event_waits", stats.event_waits),
        ] {
            output.set_item(name, value)?;
        }
        let reasons = PyDict::new(py);
        for (name, value) in [
            ("allocator", stats.reclaimed_by_reason.allocator),
            ("room", stats.reclaimed_by_reason.room),
        ] {
            reasons.set_item(name, value)?;
        }
        output.set_item("reclaimed_by_reason", reasons)?;
        output.set_item("poisoned", stats.poisoned)?;
        Ok(output)
    }
}

#[pyclass(name = "_VarenaRegion", module = "tensorfs._ext", frozen)]
struct PyRegion {
    region: Deferred<Arc<varena::Region>>,
}
impl Drop for PyRegion {
    fn drop(&mut self) {
        if let Some(region) = self.region.value.take() {
            if self.region.pid == std::process::id() {
                release_region(region);
            } else {
                std::mem::forget(region);
            }
        }
    }
}
#[pymethods]
impl PyRegion {
    #[getter]
    fn resident(&self, py: Python<'_>) -> PyResult<bool> {
        py.detach(|| self.region.is_resident())
            .map_err(|e| raise(py, e))
    }
    #[getter]
    fn allocated_bytes(&self) -> usize {
        self.region.allocated_bytes()
    }
    #[getter]
    fn ptr(&self) -> u64 {
        self.region.ptr()
    }
    /// Pinned for `stream`, which is ordered behind its bytes. Absent, its Source fills it,
    /// and only the core chooses what else moves.
    #[cfg(feature = "store")]
    #[pyo3(name = "use_on")]
    fn use_on(&self, py: Python<'_>, stream: u64) -> PyResult<PyUse> {
        use_region(py, &self.region, stream)
    }
    fn pin(&self, py: Python<'_>) -> PyResult<Option<PyUse>> {
        let pin = py
            .detach(|| self.region.try_pin())
            .map_err(|e| raise(py, e))?;
        Ok(pin.map(|pin| {
            let consumers = Consumers::new(self.region.device());
            PyUse {
                preparation: Mutex::new(Some(UsePreparation::Resident(SharedPin::new(pin, consumers.clone())))),
                device: self.region.device(), pid: std::process::id(), consumers,
            }
        }))
    }
}

/// `region` pinned for `stream`, filled by its Source if absent.
#[cfg(feature = "store")]
fn use_region(py: Python<'_>, region: &Arc<varena::Region>, stream: u64) -> PyResult<PyUse> {
    // SAFETY: the caller's live stream, which reads nothing before this returns.
    let pin = py.detach(|| unsafe { region.use_on(stream) })
        .map_err(|error| source::take_failure(py).unwrap_or_else(|| raise(py, error)))?;
    let consumers = Consumers::new(region.device());
    consumers.declare(stream).map_err(|error| raise(py, error))?;
    Ok(PyUse {
        preparation: Mutex::new(Some(UsePreparation::Resident(SharedPin::new(pin, consumers.clone())))),
        device: region.device(), pid: std::process::id(), consumers,
    })
}

#[pyclass(name = "_VarenaUse", module = "tensorfs._ext", frozen)]
struct PyUse {
    preparation: Mutex<Option<UsePreparation>>,
    device: i32,
    pid: u32,
    consumers: Consumers,
}
#[cfg(feature = "store")]
impl PyUse {
    /// A fill exposed to a Source's derive, on the thread that called `use`.
    fn filling(fill: varena::Fill, consumers: Consumers) -> Self {
        let device = fill.device();
        Self {
            preparation: Mutex::new(Some(UsePreparation::Fill { fill, exported: None })),
            device, pid: std::process::id(), consumers,
        }
    }
    /// The derive returned: every byte is initialized. Joins its writers and publishes. The
    /// returned Pin is its caller's; what was exported keeps its own.
    fn finish(&self, py: Python<'_>) -> PyResult<varena::Pin> {
        // All owner/native locks leave scope before detach reacquires Python.
        py.detach(|| -> varena::Result<varena::Pin> {
            let current = self.preparation.lock()
                .map_err(|_| varena::Error::Poisoned("fill owner mutex".into()))?
                .take().ok_or(varena::Error::Invalid("fill already ended"))?;
            let UsePreparation::Fill { fill, exported } = current else {
                return Err(varena::Error::Invalid("fill already ended"));
            };
            // SAFETY: the derive initialized every byte; this joins all its writers.
            let pin = unsafe { fill.commit_external() }?;
            *self.preparation.lock()
                .map_err(|_| varena::Error::Poisoned("fill owner mutex".into()))? =
                exported.map(UsePreparation::Resident);
            Ok(pin)
        }).map_err(|e| raise(py, e))
    }
}
#[pymethods]
impl PyUse {
    /// Declare a CUDA stream handle that uses this storage. The final shared owner
    /// records live-stream events or consumes checked retirement witnesses.
    fn consume_on(&self, py: Python<'_>, stream: u64) -> PyResult<()> {
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err(
                "CUDA storage cannot be used after fork",
            ));
        }
        self.consumers.declare(stream).map_err(|error| raise(py, error))
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
        let _ = max_version; // Ready fills are context-synchronized; empty fills have no producer.
        if copy == Some(true) || dl_device.is_some_and(|d| d != (2, self.device)) {
            return Err(PyValueError::new_err(
                "Varena exports its original CUDA storage only",
            ));
        }
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err(
                "CUDA storage cannot be used after fork",
            ));
        }
        if let Some(stream) = stream {
            let stream = u64::try_from(stream).map_err(|_| PyValueError::new_err("CUDA consumer stream must be nonnegative"))?;
            self.consumers.declare(stream).map_err(|error| raise(py, error))?;
        }
        if !self.consumers.0.declared().map_err(|error| raise(py, error))? {
            return Err(PyValueError::new_err("declare a CUDA consumer with consume_on or the DLPack stream argument"));
        }
        let owner = py
            .detach(|| {
                let mut preparation = self
                    .preparation
                    .lock()
                    .map_err(|_| varena::Error::Poisoned("fill owner mutex".into()))?;
                match preparation
                    .as_mut()
                    .ok_or(varena::Error::Invalid("fill failed"))?
                {
                    UsePreparation::Resident(owner) => Ok(Arc::clone(owner)),
                    UsePreparation::Fill { fill, exported } => {
                        if exported.is_none() {
                            *exported = Some(SharedPin::new(fill.pin()?, self.consumers.clone()));
                        }
                        Ok(Arc::clone(exported.as_ref().expect("exported Fill owner")))
                    }
                }
            })
            .map_err(|e| raise(py, e))?;
        export(py, owner)
    }
}
impl Drop for PyUse {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            std::mem::forget(std::mem::replace(&mut self.preparation, Mutex::new(None)));
            return;
        }
        if let Some(preparation) = self
            .preparation
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            // Pin count/fill abort becomes visible now, before any later allocation recovery.
            match preparation {
                UsePreparation::Resident(owner) => drop(owner),
                UsePreparation::Fill { fill, exported } => {
                    // Its declared streams carry its copy: their fences free it, with no join.
                    self.consumers.release(fill.abandon());
                    drop(exported);
                }
            }
        }
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
    _owner: Arc<SharedPin>,
    length: i64,
    pid: u32,
}

unsafe extern "C" fn delete(tensor: *mut Managed) {
    if tensor.is_null() {
        return;
    }
    // SAFETY: a DLPack consumer owns these two boxes exactly once after capsule consumption.
    let tensor = unsafe { Box::from_raw(tensor) };
    // SAFETY: capsule creates context as a Held box; its physical Pin outlives every tensor alias.
    let held = unsafe { Box::from_raw(tensor.context as *mut Held) };
    if held.pid != std::process::id() {
        std::mem::forget(held);
    }
    // Its Arc drops without attaching Python. Only the last actual owner retires the Pin.
}
unsafe extern "C" fn capsule_delete(capsule: *mut ffi::PyObject) {
    // SAFETY: Python calls this on a live capsule. Consumed capsules have a different name.
    unsafe {
        if ffi::PyCapsule_IsValid(capsule, c"dltensor".as_ptr()) == 1 {
            delete(ffi::PyCapsule_GetPointer(capsule, c"dltensor".as_ptr()) as *mut Managed);
        }
    }
}
fn export(py: Python<'_>, owner: Arc<SharedPin>) -> PyResult<Bound<'_, PyAny>> {
    let pin = owner.pin();
    let length = match i64::try_from(pin.len()) {
        Ok(length) => length,
        Err(_) => {
            return Err(PyValueError::new_err("tensor length exceeds DLPack i64"));
        }
    };
    let (pointer, device) = (pin.ptr(), pin.device());
    owner.register().map_err(|error| raise(py, error))?;
    let held = Box::into_raw(Box::new(Held {
        _owner: owner,
        length,
        pid: std::process::id(),
    }));
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
    pid: u32,
}
#[pymethods]
impl PyAllocatorCapture {
    #[new]
    #[pyo3(signature = (device, *, replay = false))]
    fn new(py: Python<'_>, device: i32, replay: bool) -> PyResult<Self> {
        let native = py
            .detach(|| {
                if replay {
                    varena::enter_replay(device)
                } else {
                    varena::enter_capture(device)
                }
            })
            .map_err(|e| raise(py, e))?;
        Ok(Self {
            guard: Mutex::new(Some(native)),
            pid: std::process::id(),
        })
    }
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err(
                "CUDA capture guard cannot be used after fork",
            ));
        }
        py.detach(|| {
            self.guard
                .lock()
                .map_err(|_| PyRuntimeError::new_err("capture guard poisoned"))?
                .take();
            Ok(())
        })
    }
}
impl Drop for PyAllocatorCapture {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            std::mem::forget(std::mem::replace(&mut self.guard, Mutex::new(None)));
        }
        // Same-process guard release is an atomic decrement, with no native lock or CUDA work.
    }
}

/// No CUDA capture may begin on the device while it is held, and none is in progress: a release
/// of Torch's cache (`cudaFree`) here never lands inside one.
#[pyclass(name = "_VarenaNoCapture", module = "tensorfs._ext", frozen)]
struct PyNoCapture {
    guard: Mutex<Option<varena::ProducerGuard>>,
}
#[pymethods]
impl PyNoCapture {
    #[new]
    fn new(py: Python<'_>, device: i32) -> PyResult<Self> {
        let guard = py.detach(|| varena::enter_producer(device)).map_err(|e| raise(py, e))?;
        Ok(Self { guard: Mutex::new(Some(guard)) })
    }
    fn close(&self) {
        self.guard.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// Sets this process's room on `device` (None lifts it) and returns the pages it took back.
#[pyfunction]
fn _varena_core_identity() -> u64 {
    varena::abi::vr_v1_core_identity()
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_varena_core_identity, module)?)?;
    cache_owner::install(module)?;
    working::install(module)?;
    tensor_export::install(module)?;
    let poisoned = module.py().get_type::<VarenaPoisonedError>();
    poisoned.setattr("__module__", "tensorfs._ext")?;
    poisoned.setattr("sticky", true)?;
    module.add("VarenaPoisonedError", poisoned)?;
    let overlap = module.py().get_type::<PhaseOverlapError>();
    overlap.setattr("__module__", "tensorfs._ext")?;
    module.add("PhaseOverlapError", overlap)?;
    module.add_class::<PyDevice>()?;
    module.add_class::<plan::PyPlan>()?;
    module.add_class::<PyRegion>()?;
    module.add_class::<PyUse>()?;
    module.add_class::<PyAllocatorCapture>()?;
    module.add_class::<PyNoCapture>()?;
    #[cfg(feature = "store")]
    module.add_class::<pages::PyPages>()?;
    #[cfg(feature = "store")]
    module.add_class::<source::PySource>()?;
    #[cfg(feature = "store")]
    module.add_class::<retained::PyRetained>()?;
    #[cfg(feature = "store")]
    module.add_class::<phase::PhaseController>()?;
    Ok(())
}
