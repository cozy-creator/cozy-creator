//! Python token providers and `Endpoint`, over the public Rust ones.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use tensorfs::raw::block_on;
use tensorfs::{bearer, Config, Endpoint, Failure, HeaderMap, Kind, Request, Result, TokenProvider};

use crate::store::{config_to_py, OrRaise};

pub(crate) fn config(
    endpoint: Option<String>,
    cache_dir: Option<PathBuf>,
    token: Option<&Bound<'_, PyAny>>,
    ca_der: Option<Vec<u8>>,
) -> PyResult<Config> {
    let mut config = Config::from_env();
    if let Some(endpoint) = endpoint {
        config.endpoint = endpoint;
    }
    if let Some(root) = cache_dir {
        config.cache_dir = root;
    }
    config.token = provider(token)?;
    config.ca_der = ca_der;
    Ok(config)
}

fn failed(error: PyErr) -> Failure {
    Python::attach(|py| {
        if !error.is_instance_of::<crate::errors::Error>(py) {
            return Failure {
                kind: Kind::Denied,
                detail: "the Python token provider failed".into(),
            };
        }
        let value = error.value(py);
        let family: String = value
            .getattr("code")
            .and_then(|code| code.extract())
            .unwrap_or_default();
        let kind = Kind::ALL
            .into_iter()
            .find(|kind| kind.code() == family)
            .unwrap_or(Kind::Denied);
        let detail = value
            .getattr("detail")
            .and_then(|detail| detail.extract())
            .unwrap_or_else(|_| {
                value
                    .str()
                    .map(|text| text.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "the TensorFS token provider failed".into())
            });
        Failure { kind, detail }
    })
}

struct PythonProvider(Py<PyAny>);

#[async_trait]
impl TokenProvider for PythonProvider {
    async fn headers(&self, request: &Request<'_>) -> Result<Vec<(String, String)>> {
        Python::attach(|py| {
            let provider = self.0.bind(py);
            let headers = PyDict::new(py);
            for (name, value) in request.headers {
                headers.set_item(name, value)?;
            }
            let args = (request.method, request.url, headers, request.payload);
            let value = if provider.hasattr("headers")? {
                provider.call_method1("headers", args)?
            } else {
                provider.call1(args)?
            };
            if let Ok(dict) = value.cast::<PyDict>() {
                dict.iter()
                    .map(|(k, v)| Ok((k.extract()?, v.extract()?)))
                    .collect()
            } else {
                value.extract()
            }
        })
        .map_err(failed)
    }

    fn retry(&self, sent: &Request<'_>, status: u16, headers: &HeaderMap) -> Result<bool> {
        Python::attach(|py| -> PyResult<bool> {
            let provider = self.0.bind(py);
            if !provider.hasattr("retry")? {
                return Ok(false);
            }
            let answer = PyDict::new(py);
            for (name, value) in headers {
                if let Ok(value) = value.to_str() {
                    answer.set_item(name.as_str(), value)?;
                }
            }
            let request = PyDict::new(py);
            for (name, value) in sent.headers {
                request.set_item(name, value)?;
            }
            provider
                .call_method1(
                    "retry",
                    (sent.method, sent.url, request, sent.payload, status, answer),
                )?
                .extract()
        })
        .map_err(failed)
    }
}

pub(crate) fn provider(
    token: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Arc<dyn TokenProvider>>> {
    Ok(match token {
        None => None,
        Some(token) if token.is_none() => None,
        Some(token) => {
            if let Ok(token) = token.extract::<String>() {
                Some(bearer(token))
            } else {
                Some(Arc::new(PythonProvider(token.clone().unbind())))
            }
        }
    })
}

#[pyclass(name = "Endpoint", module = "tensorfs", frozen)]
pub struct PyEndpoint {
    endpoint: Endpoint,
}

#[pymethods]
impl PyEndpoint {
    #[new]
    #[pyo3(signature = (url, token = None, *, ca_der = None, cache_dir = None))]
    fn new(
        py: Python<'_>,
        url: &str,
        token: Option<&Bound<'_, PyAny>>,
        ca_der: Option<Vec<u8>>,
        cache_dir: Option<PathBuf>,
    ) -> PyResult<Self> {
        let config = config(Some(url.to_owned()), cache_dir, token, ca_der)?;
        Ok(Self {
            endpoint: Endpoint::with_config(url, &config).or_raise(py)?,
        })
    }

    /// The commit of `repo_id` at `release` (none: the newest) in `lane` (none: what `gpus`
    /// run best, of plain lanes only with `plain`; none: the CPU): `{release, lane, commit,
    /// header, reasons}`.
    #[pyo3(signature = (repo_id, *, release = None, lane = None, gpus = None, plain = false))]
    fn resolve<'py>(
        &self,
        py: Python<'py>,
        repo_id: &str,
        release: Option<String>,
        lane: Option<String>,
        gpus: Option<Vec<String>>,
        plain: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let hardware = tensorfs::fetch::Hardware { gpus: gpus.unwrap_or_default(), free_vram: None, plain };
        let resolved = py
            .detach(|| block_on(self.endpoint.resolve(repo_id, release.as_deref(), lane.as_deref(), &hardware)))
            .or_raise(py)?;
        config_to_py(py, &serde_json::to_value(resolved).expect("Resolved is serializable"))
    }
}

pub fn install(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyEndpoint>()
}
