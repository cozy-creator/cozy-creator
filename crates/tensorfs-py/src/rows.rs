//! A stored tensor's rows (tfs-303), for Python: `Checkpoint.rows` says how a tensor cuts, and
//! `read_rows` reads only those rows' stored bytes: `Checkpoint.read_rows` from the store, a varena
//! Source's from its kept page, else the store. Both parse their reads and outs here.

use pyo3::buffer::PyBuffer;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use tensorfs::format::decode::rows::{Cut, Rows};
use tensorfs::format::Header;
use tensorfs::io::Dst;
use tensorfs::{Failure, Kind};

use crate::store::{invalid, raise, Dlpack};

/// How a stored tensor cuts by rows: `count` along its first dimension, ranges on multiples of
/// `tile`, and each part's bytes.
#[pyclass(name = "RowCut", module = "tensorfs", frozen)]
pub(crate) struct PyRowCut(Rows);

#[pymethods]
impl PyRowCut {
    /// Rows along the logical tensor's first dimension.
    #[getter]
    fn count(&self) -> u64 {
        self.0.count
    }
    /// A range starts on a multiple of it and stops on one or at `count`: from the encoding.
    #[getter]
    fn tile(&self) -> u64 {
        self.0.tile
    }
    /// Each stored part's bytes per tile of rows (per row where `tile` is 1), or None for a part
    /// every range reads whole. `""` is a plain tensor's data.
    #[getter]
    fn tile_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for p in &self.0.parts {
            let bytes = match p.cut {
                Cut::Whole => None,
                Cut::Tiles { rows, bytes } => Some(bytes * (self.0.tile / rows)),
            };
            out.set_item(&p.role, bytes)?;
        }
        Ok(out)
    }
    /// Each part's bytes for rows `[start, stop)`: what its `read_rows` out holds.
    fn nbytes<'py>(&self, py: Python<'py>, start: u64, stop: u64) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for span in self.0.range(start, stop).map_err(|e| invalid(py, e.to_string()))? {
            out.set_item(span.role, span.len)?;
        }
        Ok(out)
    }
    fn __repr__(&self) -> String {
        let roles: Vec<&str> = self.0.parts.iter().map(|p| p.role.as_str()).collect();
        format!("RowCut(count={}, tile={}, parts={roles:?})", self.0.count, self.0.tile)
    }
}

/// `component/key`'s rows, or None where it does not cut.
pub(crate) fn rows(py: Python<'_>, header: &Header, component: &str, key: &str) -> PyResult<Option<Rows>> {
    let tensor = header.tensor(component, key).ok_or_else(|| {
        raise(py, Failure { kind: Kind::NotFound, detail: format!("no tensor {component}/{key}") })
    })?;
    tensor.rows().map_err(|e| invalid(py, e.to_string()))
}

pub(crate) fn cut(py: Python<'_>, header: &Header, component: &str, key: &str) -> PyResult<Option<PyRowCut>> {
    Ok(rows(py, header, component, key)?.map(PyRowCut))
}

/// One part's bytes of a read: `dst.len()` of them from `offset` of the part.
pub(crate) struct Want {
    pub(crate) component: String,
    pub(crate) key: String,
    /// `""`: a plain tensor's data.
    pub(crate) role: String,
    pub(crate) offset: u64,
    pub(crate) dst: Dst,
}

/// What keeps an out exported while it is written.
enum Held {
    Dlpack(Dlpack),
    #[allow(dead_code)] // never read: holding it keeps the buffer exported
    Buffer(PyBuffer<u8>),
}

/// Parsed reads, each out exported and held until `finish`.
pub(crate) struct Reads {
    pub(crate) wants: Vec<Want>,
    held: Vec<Held>,
}

impl Reads {
    /// `reads`: `(component, key, start, stop, {role: out})`. Each out is a writable C-contiguous
    /// buffer or a CPU or CUDA DLPack array of exactly its part's bytes for the range
    /// (`RowCut.nbytes`); a role left out is not read. CUDA outs are written on `stream` (a
    /// CUstream), after its earlier work; with none, each on one of ours, drained at `finish`.
    pub(crate) fn parse<'py>(py: Python<'py>, header: &Header, reads: &Bound<'py, PyAny>, stream: Option<usize>) -> PyResult<Self> {
        let reads: Vec<(String, String, u64, u64, Bound<'py, PyDict>)> = reads.extract()?;
        let mut parsed = Reads { wants: Vec::new(), held: Vec::new() };
        for (component, key, start, stop, outs) in reads {
            let cut = rows(py, header, &component, &key)?
                .ok_or_else(|| invalid(py, format!("{component}/{key} does not cut by rows")))?;
            let spans = cut.range(start, stop).map_err(|e| invalid(py, format!("{component}/{key}: {e}")))?;
            for (role, out) in outs.iter() {
                let role: String = role.extract()?;
                let span = spans.iter().find(|s| s.role == role).ok_or_else(|| {
                    let roles: Vec<&str> = spans.iter().map(|s| s.role).collect();
                    invalid(py, format!("{component}/{key} has no part {role:?} (its parts: {roles:?})"))
                })?;
                let (dst, held) = destination(py, &out, stream)?;
                parsed.held.push(held);
                if dst.len() as u64 != span.len {
                    return Err(invalid(py, format!(
                        "{component}/{key}#{role}: rows {start}..{stop} are {} bytes, its out holds {}",
                        span.len,
                        dst.len()
                    )));
                }
                parsed.wants.push(Want { component: component.clone(), key: key.clone(), role, offset: span.offset, dst });
            }
        }
        Ok(parsed)
    }

    /// After the read: our own streams drained and every out released; after a failed read
    /// (`ok` false), each CUDA out is kept until its device's engine drains.
    pub(crate) fn finish(self, py: Python<'_>, ok: bool) -> PyResult<()> {
        let mut result = Ok(());
        for held in self.held {
            let Held::Dlpack(mut held) = held else { continue };
            if !ok {
                held.record_cuda_error();
            }
            if let Err(failure) = py.detach(move || held.finish()) {
                result = result.and(Err(raise(py, failure)));
            }
        }
        result
    }
}

fn destination(py: Python<'_>, out: &Bound<'_, PyAny>, stream: Option<usize>) -> PyResult<(Dst, Held)> {
    if out.hasattr("__dlpack__")? {
        let held = match stream {
            Some(stream) => Dlpack::on(py, out, stream)?,
            None => Dlpack::of(py, out, None, None)?,
        };
        return Ok((held.destination(), Held::Dlpack(held)));
    }
    let view = py.import("builtins")?.getattr("memoryview")?.call1((out,))?;
    if view.getattr("readonly")?.extract::<bool>()? || !view.getattr("c_contiguous")?.extract::<bool>()? {
        return Err(invalid(py, "an out must be a writable C-contiguous buffer or DLPack array"));
    }
    let buffer: PyBuffer<u8> = PyBuffer::get(&view.call_method1("cast", ("B",))?)?;
    let dst = Dst::Host { ptr: buffer.buf_ptr() as *mut u8, len: buffer.len_bytes() };
    Ok((dst, Held::Buffer(buffer)))
}
