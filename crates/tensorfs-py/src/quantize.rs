//! Thin access to TensorFS's versioned tensor encoders. No framework import or model policy.

use std::collections::BTreeMap;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyMapping};
use tensorfs::format::{encode, Dtype, Encoding, PartBytes};

use crate::store::{from_py, OrRaise};
use crate::tensors::PyEncoded;

#[pyfunction]
#[pyo3(signature = (scheme, values, *, dtype, shape, strategy, params = None))]
#[allow(clippy::too_many_arguments)]
fn quantize(
    py: Python<'_>,
    scheme: String,
    values: &Bound<'_, PyBytes>,
    dtype: String,
    shape: Vec<u64>,
    strategy: String,
    params: Option<&Bound<'_, PyMapping>>,
) -> PyResult<PyEncoded> {
    let mut parameters = BTreeMap::new();
    if let Some(params) = params {
        for item in params.items()?.iter() {
            let (key, value): (String, Bound<'_, PyAny>) = item.extract()?;
            parameters.insert(key, from_py(&value)?);
        }
    }
    let params = parameters;
    let encoding = Encoding { scheme, params };
    let dtype = Dtype::parse(&dtype);
    // Immutable Python bytes remain owned by this call while the GIL is released.
    // A mutable exporter would require copying or an exclusive access contract.
    let bytes = values.as_bytes();
    let parts = py
        .detach(|| {
            let arrays = BTreeMap::from([(
                "values",
                PartBytes {
                    dtype: &Dtype::F32,
                    shape: &shape,
                    bytes,
                },
            )]);
            let input = encode::Input {
                dtype: &dtype,
                shape: &shape,
                encoding: &encoding,
                arrays: &arrays,
            };
            encode::quantize_strategy(&input, &strategy).map_err(tensorfs::Failure::from)
        })
        .or_raise(py)?;
    PyEncoded::from_parts(py, &dtype, &shape, &encoding, parts)
}

pub fn install(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(quantize, m)?)
}
