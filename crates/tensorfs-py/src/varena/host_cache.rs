//! Host memory an engine pages K/V into when the card can't hold it (cr-336 V2, the host half): a
//! reservation at a base that never moves, whose owner maps and unmaps whole windows. A mapped
//! window is pinned host memory registered so that the device addresses it at its own address: a
//! kernel reads it in place, and a copy from it is one transfer. Its bytes count against the
//! GPU's host tier while it is mapped, as a retained region's copy does (`Tier::stage`).
//!
//! It exists only where the device reports that it reads registered host memory at the host's
//! address and a registered page's device address is that address (`probe`). Nothing is
//! page-locked for the reservation: a window is, when it is committed.
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::PyDict,
};
use tensorfs::io::cuda::HostPin;

use super::pages::{fault_in, PyPages, Tier, WINDOW};
use super::tensor_export;

/// Anonymous address space with no memory behind it until a range is opened.
struct Space {
    ptr: *mut u8,
    len: usize,
    /// A registration the driver would not release still names it: it is never unmapped.
    kept: AtomicBool,
}
// SAFETY: an owned mapping; its ranges are opened and shut by one owner (`Inner::windows`).
unsafe impl Send for Space {}
// SAFETY: as above.
unsafe impl Sync for Space {}

impl Space {
    #[cfg(target_os = "linux")]
    fn new(len: usize) -> Result<Self, String> {
        // SAFETY: a fresh anonymous mapping with no access; nothing else names it.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap of {len} bytes: {}", std::io::Error::last_os_error()));
        }
        Ok(Self { ptr: ptr.cast(), len, kept: AtomicBool::new(false) })
    }
    #[cfg(not(target_os = "linux"))]
    fn new(_: usize) -> Result<Self, String> {
        Err("host K/V reserves anonymous memory on Linux only".into())
    }

    /// `[at, at + len)` readable, writable and faulted in.
    #[cfg(target_os = "linux")]
    fn open(&self, at: usize, len: usize) -> Result<(), String> {
        // SAFETY: inside this mapping (callers check their ranges); nothing reads it yet.
        let rc = unsafe {
            libc::mprotect(self.ptr.add(at).cast(), len, libc::PROT_READ | libc::PROT_WRITE)
        };
        if rc != 0 {
            return Err(format!("mprotect of {len} bytes: {}", std::io::Error::last_os_error()));
        }
        fault_in(self.ptr as usize + at, len);
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    fn open(&self, _: usize, _: usize) -> Result<(), String> {
        Err("host K/V reserves anonymous memory on Linux only".into())
    }

    /// `[at, at + len)`'s memory given back, the range without access again.
    fn shut(&self, at: usize, len: usize) {
        #[cfg(target_os = "linux")]
        // SAFETY: inside this mapping; its registration is gone, so nothing reads or writes it.
        unsafe {
            libc::madvise(self.ptr.add(at).cast(), len, libc::MADV_DONTNEED);
            libc::mprotect(self.ptr.add(at).cast(), len, libc::PROT_NONE);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (at, len);
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        if self.kept.load(Ordering::Acquire) {
            return;
        }
        #[cfg(target_os = "linux")]
        // SAFETY: mapped in `new` with this length; every registration inside it is gone.
        unsafe {
            libc::munmap(self.ptr.cast(), self.len)
        };
    }
}

/// What a device answers: its attribute, and whether one registered page sits at its own address
/// there. Each is the driver's own refusal where it refuses.
fn probe(device: i32) -> (Result<i32, String>, Result<bool, String>) {
    let attribute = tensorfs::io::host_pointer_attribute(device).map_err(|error| error.to_string());
    let registration = (|| {
        let len = 4096;
        let space = Space::new(len)?;
        space.open(0, len)?;
        // SAFETY: the mapping outlives the pin, which is released before `space` drops.
        let (mut pin, at) = unsafe { tensorfs::io::register_mapped(device, space.ptr, len) }
            .map_err(|error| error.to_string())?;
        if pin.release().is_err() {
            space.kept.store(true, Ordering::Release);
        }
        Ok(at == space.ptr as u64)
    })();
    (attribute, registration)
}

fn supported(answer: &(Result<i32, String>, Result<bool, String>)) -> Result<(), String> {
    match answer {
        (Err(error), _) => Err(format!("its attribute can't be read ({error})")),
        (Ok(0), _) => Err("the device doesn't read registered host memory at the host's address".into()),
        (_, Err(error)) => Err(format!("a device-mapped registration is refused ({error})")),
        (_, Ok(false)) => Err("a registered page's device address is not its host address".into()),
        (Ok(_), Ok(true)) => Ok(()),
    }
}

struct Inner {
    space: Space,
    tier: Arc<Tier>,
    /// Each mapped window's registration, by its offset.
    windows: Mutex<BTreeMap<usize, HostPin>>,
    views: AtomicUsize,
    pid: u32,
}

impl Inner {
    fn windows(&self) -> MutexGuard<'_, BTreeMap<usize, HostPin>> {
        self.windows.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// One window opened, registered for the device and found at its own address there.
    fn map(&self, at: usize) -> Result<HostPin, String> {
        self.space.open(at, WINDOW)?;
        // SAFETY: inside the reservation, which outlives the pin (`unmap`, `Drop`).
        let made = unsafe {
            tensorfs::io::register_mapped(self.tier.device(), self.space.ptr.add(at), WINDOW)
        };
        let (pin, device) = match made {
            Ok(made) => made,
            Err(error) => {
                self.space.shut(at, WINDOW);
                return Err(error.to_string());
            }
        };
        if device == self.space.ptr as u64 + at as u64 {
            return Ok(pin);
        }
        self.unmap(at, pin).map_err(|(_, error)| error)?;
        Err(format!("the device addresses a registered window at {device:#x}, not its host address"))
    }

    /// The window unregistered, which waits for the device, and its memory given back. Refused, it
    /// stays as it was and its memory is never unmapped.
    fn unmap(&self, at: usize, mut pin: HostPin) -> Result<(), (HostPin, String)> {
        if let Err(error) = pin.release() {
            self.space.kept.store(true, Ordering::Release);
            return Err((pin, error.to_string()));
        }
        self.space.shut(at, WINDOW);
        Ok(())
    }

    fn commit(&self, range: Range<usize>) -> Result<bool, String> {
        let mut windows = self.windows();
        if range.clone().step_by(WINDOW).any(|at| windows.contains_key(&at)) {
            return Err("host cache commit overlaps mapped windows".into());
        }
        let mut made = Vec::new();
        let admitted = self.tier.stage(range.len(), || {
            for at in range.clone().step_by(WINDOW) {
                match self.map(at) {
                    Ok(pin) => made.push((at, pin)),
                    Err(error) => {
                        for (at, pin) in made.drain(..) {
                            let _ = self.unmap(at, pin);
                        }
                        return Err(error);
                    }
                }
            }
            Ok(())
        })?;
        windows.extend(made);
        Ok(admitted)
    }

    fn release(&self, range: Range<usize>) -> Result<(), String> {
        let mut windows = self.windows();
        if range.clone().step_by(WINDOW).any(|at| !windows.contains_key(&at)) {
            return Err("host cache release requires entirely mapped windows".into());
        }
        for at in range.step_by(WINDOW) {
            let pin = windows.remove(&at).expect("a mapped window, checked above");
            match self.unmap(at, pin) {
                Ok(()) => self.tier.unstage(WINDOW),
                Err((pin, error)) => {
                    windows.insert(at, pin);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let windows = std::mem::take(self.windows.get_mut().unwrap_or_else(|error| error.into_inner()));
        if self.pid != std::process::id() {
            // A forked child has no driver to unregister with, and the pages are its own copy.
            std::mem::forget(windows);
            return;
        }
        for (at, pin) in windows {
            if self.unmap(at, pin).is_ok() {
                self.tier.unstage(WINDOW);
            }
        }
    }
}

/// One exported tensor's hold on its reservation.
struct View(Arc<Inner>);
impl View {
    fn new(inner: &Arc<Inner>) -> Self {
        inner.views.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(inner))
    }
}
impl Drop for View {
    fn drop(&mut self) {
        self.0.views.fetch_sub(1, Ordering::AcqRel);
    }
}

#[pyclass(name = "_VarenaHostCache", module = "tensorfs._ext", frozen)]
pub(super) struct PyHostCache {
    inner: Arc<Inner>,
}

impl PyHostCache {
    fn range(&self, offset: usize, bytes: usize) -> PyResult<Range<usize>> {
        if self.inner.pid != std::process::id() {
            return Err(PyRuntimeError::new_err("a host cache cannot be used after fork"));
        }
        let end = offset.checked_add(bytes).filter(|&end| end <= self.inner.space.len);
        match end {
            Some(end) if bytes != 0 && offset.is_multiple_of(WINDOW) && bytes.is_multiple_of(WINDOW) => {
                Ok(offset..end)
            }
            _ => Err(PyValueError::new_err(
                "host cache range must be nonempty, whole windows and inside its reservation",
            )),
        }
    }
}

#[pymethods]
impl PyHostCache {
    /// `reserve` bytes of host address space for `pages`' GPU, rounded up to windows, with nothing
    /// mapped. Refused, in one line, where the device can't read host memory in place (`probe`).
    #[new]
    fn new(py: Python<'_>, pages: PyRef<'_, PyPages>, reserve: usize) -> PyResult<Self> {
        let tier = Arc::clone(&pages.tier);
        let made = py.detach(|| {
            supported(&probe(tier.device())).map_err(|why| format!("no host K/V on this GPU: {why}"))?;
            let len = reserve.max(1).next_multiple_of(WINDOW);
            Ok::<_, String>(Inner {
                space: Space::new(len)?,
                tier,
                windows: Mutex::new(BTreeMap::new()),
                views: AtomicUsize::new(0),
                pid: std::process::id(),
            })
        });
        Ok(Self { inner: Arc::new(made.map_err(PyRuntimeError::new_err)?) })
    }
    #[getter]
    fn base(&self) -> usize {
        self.inner.space.ptr as usize
    }
    #[getter]
    fn granularity(&self) -> usize {
        WINDOW
    }
    #[getter]
    fn reserved_bytes(&self) -> usize {
        self.inner.space.len
    }
    /// Map `bytes` at `offset`: whole windows, none mapped, all or none. False: the host has no
    /// room for them now, and nothing was mapped.
    fn commit(&self, py: Python<'_>, offset: usize, bytes: usize) -> PyResult<bool> {
        let range = self.range(offset, bytes)?;
        py.detach(|| self.inner.commit(range)).map_err(PyRuntimeError::new_err)
    }
    /// Give `bytes` at `offset` back: whole mapped windows nothing queued after this call reads or
    /// writes. Unregistering one waits for the device's queued work.
    fn release(&self, py: Python<'_>, offset: usize, bytes: usize) -> PyResult<()> {
        let range = self.range(offset, bytes)?;
        py.detach(|| self.inner.release(range)).map_err(PyRuntimeError::new_err)
    }
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let row = PyDict::new(py);
        row.set_item("reserved_bytes", self.inner.space.len)?;
        row.set_item("mapped_bytes", self.inner.windows().len() * WINDOW)?;
        Ok(row)
    }
    /// A DLPack tensor of `shape` at `byte_offset`, anywhere in the reservation: as the device
    /// addresses it (a CUDA tensor a kernel reads in place), or with `host`, as host memory (what
    /// a copy to the card reads). It owns no window: only mapped ones may be touched.
    #[pyo3(signature = (code, bits, shape, byte_offset = 0, host = false))]
    fn dlpack<'py>(
        &self,
        py: Python<'py>,
        code: u8,
        bits: u8,
        shape: Vec<i64>,
        byte_offset: u64,
        host: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = &self.inner;
        let span = tensor_export::layout_span(code, bits, &shape, None)?;
        if !byte_offset.is_multiple_of(u64::from(bits / 8)) {
            return Err(PyValueError::new_err("tensor byte offset is not aligned to its dtype"));
        }
        if !byte_offset.checked_add(span).is_some_and(|end| end <= inner.space.len as u64) {
            return Err(PyValueError::new_err("tensor layout exceeds the host cache's reservation"));
        }
        let pointer = inner.space.ptr as usize + byte_offset as usize;
        let kind = if host { tensor_export::CPU } else { tensor_export::CUDA };
        let device = if host { 0 } else { inner.tier.device() };
        tensor_export::capsule(py, View::new(inner), pointer, kind, device, code, bits, shape, None)
    }
    /// Checks that it can go: no window mapped and no tensor of it alive. Its address space is
    /// returned when the object is.
    fn close(&self) -> PyResult<()> {
        if self.inner.views.load(Ordering::Acquire) != 0 || !self.inner.windows().is_empty() {
            return Err(PyRuntimeError::new_err(
                "host cache still has mapped windows or live tensors",
            ));
        }
        Ok(())
    }
}

/// What `device` answers about reading registered host memory in place: its attribute
/// (`CU_DEVICE_ATTRIBUTE_CAN_USE_HOST_POINTER_FOR_REGISTERED_MEM`), whether a page registered with
/// the device-map flag sits at its own address there, each call's refusal where it refused, and
/// `supported` with `why` when it isn't.
#[pyfunction]
fn _varena_host_cache_probe(py: Python<'_>, device: i32) -> PyResult<Bound<'_, PyDict>> {
    let answer = py.detach(|| probe(device));
    let row = PyDict::new(py);
    row.set_item("attribute", answer.0.as_ref().ok())?;
    row.set_item("attribute_error", answer.0.as_ref().err())?;
    row.set_item("same_address", answer.1.as_ref().ok())?;
    row.set_item("registration_error", answer.1.as_ref().err())?;
    let verdict = supported(&answer);
    row.set_item("supported", verdict.is_ok())?;
    row.set_item("why", verdict.err())?;
    Ok(row)
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyHostCache>()?;
    module.add_function(wrap_pyfunction!(_varena_host_cache_probe, module)?)
}
