//! Thin access to TensorFS's versioned tensor encoders. No framework import or model policy.

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyMapping};
use tensorfs::format::{Dtype, Encoding, PartBytes, encode};

use crate::store::{OrRaise, config_to_py, from_py, invalid};
use crate::tensors::PyEncoded;

/// The conversion engine's cancellation token, shared with a caller's request watcher.
#[pyclass(name = "ConversionControl", module = "tensorfs", frozen)]
#[derive(Default)]
pub(crate) struct PyConversionControl {
    pub(crate) inner: Arc<tensorfs::ConversionControl>,
}
#[pymethods]
impl PyConversionControl {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    fn cancel(&self) {
        self.inner.cancel();
    }

    #[getter]
    fn canceled(&self) -> bool {
        self.inner.stopped()
    }

    fn progress(&self) -> (u64, u64, u64) {
        self.inner.progress()
    }
}

#[pyfunction]
#[pyo3(signature = (scheme, values, *, dtype, shape, strategy, params = None, tensor_amax = None))]
#[allow(clippy::too_many_arguments)]
fn quantize(
    py: Python<'_>,
    scheme: String,
    values: &Bound<'_, PyBytes>,
    dtype: String,
    shape: Vec<u64>,
    strategy: String,
    params: Option<&Bound<'_, PyMapping>>,
    tensor_amax: Option<f64>,
) -> PyResult<PyEncoded> {
    Ok(produce(
        py,
        scheme,
        values,
        dtype,
        shape,
        strategy,
        params,
        tensor_amax,
    )?
    .0)
}

#[pyfunction]
#[pyo3(signature = (scheme, values, *, dtype, shape, strategy, params = None, tensor_amax = None))]
#[allow(clippy::too_many_arguments)]
fn quantize_measured(
    py: Python<'_>,
    scheme: String,
    values: &Bound<'_, PyBytes>,
    dtype: String,
    shape: Vec<u64>,
    strategy: String,
    params: Option<&Bound<'_, PyMapping>>,
    tensor_amax: Option<f64>,
) -> PyResult<(PyEncoded, Py<PyAny>)> {
    let (encoded, stats) = produce(
        py,
        scheme,
        values,
        dtype,
        shape,
        strategy,
        params,
        tensor_amax,
    )?;
    let stats = serde_json::to_value(stats).map_err(|e| invalid(py, e.to_string()))?;
    Ok((encoded, config_to_py(py, &stats)?.unbind()))
}

#[allow(clippy::too_many_arguments)]
fn produce(
    py: Python<'_>,
    scheme: String,
    values: &Bound<'_, PyBytes>,
    dtype: String,
    shape: Vec<u64>,
    strategy: String,
    params: Option<&Bound<'_, PyMapping>>,
    tensor_amax: Option<f64>,
) -> PyResult<(PyEncoded, encode::QuantizationStats)> {
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
    let maximum = tensor_amax.map(f64::to_le_bytes);
    let result = py
        .detach(|| {
            let mut arrays = BTreeMap::from([(
                "values",
                PartBytes {
                    dtype: &Dtype::F32,
                    shape: &shape,
                    bytes,
                },
            )]);
            if let Some(bytes) = &maximum {
                arrays.insert(
                    "tensor_amax",
                    PartBytes {
                        dtype: &Dtype::F64,
                        shape: &[],
                        bytes,
                    },
                );
            }
            let input = encode::Input {
                dtype: &dtype,
                shape: &shape,
                encoding: &encoding,
                arrays: &arrays,
            };
            encode::quantize_measured(&input, &strategy).map_err(tensorfs::Failure::from)
        })
        .or_raise(py)?;
    let encoded = PyEncoded::from_parts(py, &dtype, &shape, &encoding, result.parts)?;
    Ok((encoded, result.stats))
}

#[pyfunction]
fn quantization_implementation() -> String {
    #[cfg(feature = "store")]
    {
        tensorfs::quantization_implementation()
    }
    #[cfg(not(feature = "store"))]
    {
        encode::implementation()
    }
}

pub fn install(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(quantization_implementation, m)?)?;
    // Both operations bind the same checked reader/cast implementation family.
    m.add("normalization_implementation", m.getattr("quantization_implementation")?)?;
    m.add_function(wrap_pyfunction!(quantize, m)?)?;
    m.add_function(wrap_pyfunction!(quantize_measured, m)?)
}
