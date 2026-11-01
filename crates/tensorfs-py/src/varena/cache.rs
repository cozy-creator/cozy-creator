//! Thin owner-addressed Cache binding; no Python memory policy or shrink dispatcher.
use super::cache_owner::owner_error;
use super::*;

#[pyclass(name = "_VarenaCache", module = "tensorfs._ext", frozen)]
pub(super) struct PyCache {
    // Native Cache finalization is metadata-only. It never releases unproven live pages.
    cache: Deferred<Arc<varena::Cache>>,
}

fn stats(py: Python<'_>, value: varena::CacheStats) -> PyResult<Bound<'_, PyDict>> {
    let output = PyDict::new(py);
    for (name, bytes) in [
        ("reserved_bytes", value.reserved_bytes),
        ("mapped_bytes", value.mapped_bytes),
        ("live_bytes", value.live_bytes),
        ("pending_bytes", value.pending_bytes),
        ("ready_bytes", value.ready_bytes),
    ] {
        output.set_item(name, bytes)?;
    }
    Ok(output)
}

#[pymethods]
impl PyCache {
    #[new]
    fn new(py: Python<'_>, device: i32, reserve_bytes: usize) -> PyResult<Self> {
        let cache = py
            .detach(|| varena::Cache::new(device, reserve_bytes))
            .map_err(|error| owner_error(py, error))?;
        Ok(Self {
            cache: Deferred::new(cache),
        })
    }

    #[getter]
    fn base(&self, py: Python<'_>) -> PyResult<u64> {
        self.cache.base().map_err(|error| owner_error(py, error))
    }

    #[getter]
    fn granularity(&self) -> usize {
        self.cache.granularity()
    }

    #[getter]
    fn reserved_bytes(&self) -> usize {
        self.cache.reserved_bytes()
    }

    fn commit(&self, py: Python<'_>, offset: usize, bytes: usize) -> PyResult<()> {
        py.detach(|| self.cache.commit(offset, bytes))
            .map_err(|error| owner_error(py, error))
    }

    fn release(&self, py: Python<'_>, offset: usize, bytes: usize, stream: u64) -> PyResult<()> {
        py.detach(|| self.cache.release(offset, bytes, stream))
            .map_err(|error| owner_error(py, error))
    }

    fn poll<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        stats(
            py,
            py.detach(|| self.cache.poll())
                .map_err(|error| owner_error(py, error))?,
        )
    }

    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        stats(
            py,
            py.detach(|| self.cache.stats())
                .map_err(|error| owner_error(py, error))?,
        )
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.cache.close())
            .map_err(|error| owner_error(py, error))
    }
}
