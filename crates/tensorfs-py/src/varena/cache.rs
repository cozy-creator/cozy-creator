//! The Runtime's Cache: a reserved device range whose owner maps and unmaps exact pages
//! (`varena::Cache`), with the owner's shrink callback beside it. An engine keeps its K/V and its
//! state slots in one each (cr-336). The same core type the C ABI's `vr_v1_cache_*` reaches.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

/// One exported tensor's hold on its Cache. The address space is not returned while one lives.
struct View {
    views: Arc<AtomicUsize>,
    _cache: Arc<varena::Cache>,
}
impl View {
    fn new(cache: &PyCache) -> Self {
        cache.views.fetch_add(1, Ordering::AcqRel);
        Self { views: Arc::clone(&cache.views), _cache: Arc::clone(&cache.cache) }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        self.views.fetch_sub(1, Ordering::AcqRel);
    }
}

#[pyclass(name = "_VarenaCache", module = "tensorfs._ext", frozen)]
pub(super) struct PyCache {
    cache: Arc<varena::Cache>,
    owner: Option<Deferred<Arc<varena::CacheOwner>>>,
    views: Arc<AtomicUsize>,
    /// Whether `close` returned the address space. `close` holds it from its check of the views to
    /// the core's close, and `dlpack` registers its view under it, so neither runs inside the other.
    closed: Mutex<bool>,
    device: i32,
    pid: u32,
}

impl PyCache {
    fn check(&self) -> PyResult<()> {
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err("a Cache cannot be used after fork"));
        }
        Ok(())
    }
}

#[pymethods]
impl PyCache {
    /// `reserve` bytes of `device`'s address space, rounded up to its pages, with nothing mapped.
    /// `shrink(requested_bytes)` is asked for memory when the core is short: it releases the pages
    /// it chooses and returns; False declines. The core keeps only a weak reference to it.
    #[new]
    #[pyo3(signature = (device, reserve, shrink = None))]
    fn new(
        py: Python<'_>,
        device: PyRef<'_, PyDevice>,
        reserve: usize,
        shrink: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let ordinal = device.owner()?.ordinal();
        let cache = py
            .detach(|| varena::Cache::new(ordinal, reserve))
            .map_err(|error| raise(py, error))?;
        // Its pages are physical: the callback takes the full capture and replay exclusion.
        let owner = shrink
            .map(|shrink| cache_owner::create_owner(py, ordinal, shrink, true).map(Deferred::new))
            .transpose()?;
        Ok(Self {
            cache,
            owner,
            views: Arc::new(AtomicUsize::new(0)),
            closed: Mutex::new(false),
            device: ordinal,
            pid: std::process::id(),
        })
    }
    #[getter]
    fn base(&self, py: Python<'_>) -> PyResult<u64> {
        self.check()?;
        self.cache.base().map_err(|error| raise(py, error))
    }
    #[getter]
    fn granularity(&self) -> usize {
        self.cache.granularity()
    }
    #[getter]
    fn reserved_bytes(&self) -> usize {
        self.cache.reserved_bytes()
    }
    /// Map `bytes` at `offset`: whole pages, none of them mapped, all or none. Short of room the
    /// core reclaims in its one order first, and a refusal leaves every earlier page as it was.
    fn commit(&self, py: Python<'_>, offset: usize, bytes: usize) -> PyResult<()> {
        self.check()?;
        py.detach(|| self.cache.commit(offset, bytes))
            .map_err(|error| raise(py, error))
    }
    /// Give `bytes` at `offset` back: whole live pages, every reader and writer of them joined
    /// into `stream`. They are memory again once `stream` has passed this point.
    fn release(&self, py: Python<'_>, offset: usize, bytes: usize, stream: u64) -> PyResult<()> {
        self.check()?;
        if stream <= 2 {
            return Err(PyValueError::new_err(
                "use an explicit CUDA stream handle, not a default-stream sentinel",
            ));
        }
        py.detach(|| {
            self.cache.release(offset, bytes, stream)?;
            varena::working::seal_stream(self.device, stream)
        })
        .map_err(|error| cache_owner::owner_error(py, error))
    }
    /// Its bytes by state, completed releases collected first.
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.check()?;
        let stats = py
            .detach(|| self.cache.poll())
            .map_err(|error| raise(py, error))?;
        let row = PyDict::new(py);
        row.set_item("reserved_bytes", stats.reserved_bytes)?;
        row.set_item("mapped_bytes", stats.mapped_bytes)?;
        row.set_item("live_bytes", stats.live_bytes)?;
        row.set_item("pending_bytes", stats.pending_bytes)?;
        row.set_item("ready_bytes", stats.ready_bytes)?;
        Ok(row)
    }
    /// A DLPack tensor of `shape` at `byte_offset`, anywhere inside the reservation. It owns no
    /// page: its owner reads and writes only what is committed, and a kernel that touches a page
    /// that isn't faults the device.
    #[pyo3(signature = (code, bits, shape, byte_offset = 0))]
    fn dlpack<'py>(
        &self,
        py: Python<'py>,
        code: u8,
        bits: u8,
        shape: Vec<i64>,
        byte_offset: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.check()?;
        let span = tensor_export::layout_span(code, bits, &shape, None)?;
        if !byte_offset.is_multiple_of(u64::from(bits / 8)) {
            return Err(PyValueError::new_err("tensor byte offset is not aligned to its dtype"));
        }
        let inside = byte_offset
            .checked_add(span)
            .is_some_and(|end| end <= self.cache.reserved_bytes() as u64);
        if !inside {
            return Err(PyValueError::new_err("tensor layout exceeds the Cache's reservation"));
        }
        if !working::installed() {
            return Err(PyValueError::new_err(
                "install tensorfs.varena.torch before exporting Torch storage",
            ));
        }
        let base = self.cache.base().map_err(|error| raise(py, error))?;
        let pointer = usize::try_from(base + byte_offset)
            .map_err(|_| PyValueError::new_err("tensor pointer exceeds the host address width"))?;
        let view = py
            .detach(|| {
                let closed = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
                (!*closed).then(|| View::new(self))
            })
            .ok_or_else(|| PyRuntimeError::new_err("the Cache is closed"))?;
        tensor_export::capsule(py, view, pointer, tensor_export::CUDA, self.device, code, bits, shape, None)
    }
    /// Return the address space: every page released and passed, no tensor of it alive, and never
    /// from its own shrink callback. A refusal changes nothing and may be tried again.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        self.check()?;
        py.detach(|| {
            let mut closed = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
            if self.views.load(Ordering::Acquire) != 0 {
                return Err(varena::Error::Busy);
            }
            let stats = self.cache.poll()?;
            if stats.live_bytes != 0 {
                return Err(varena::Error::Invalid("cache still owns live pages"));
            }
            if stats.pending_bytes != 0 {
                return Err(varena::Error::Busy);
            }
            if let Some(owner) = &self.owner {
                owner.close()?;
            }
            self.cache.close()?;
            *closed = true;
            Ok(())
        })
        .map_err(|error| cache_owner::owner_error(py, error))
    }
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyCache>()
}
