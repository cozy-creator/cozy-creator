//! Thin ownership/binding of native phase-region intent; no byte policy in Python.
use std::sync::Arc;

use pyo3::{prelude::*, types::PyDict};

use super::{Deferred, PyArena, PyRegion};

#[pyclass(name = "_VarenaPlan", module = "tensorfs._ext", frozen)]
pub(super) struct PyPlan {
    plan: Deferred<Arc<varena::plan::Plan>>,
    pid: u32,
}

impl PyPlan {
    fn check(&self) -> PyResult<()> {
        if self.pid != std::process::id() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "phase plans cannot be used after fork",
            ));
        }
        Ok(())
    }
}

#[pymethods]
impl PyPlan {
    #[new]
    fn new(
        py: Python<'_>,
        arena: PyRef<'_, PyArena>,
        phases: Vec<(String, Vec<Py<PyRegion>>)>,
    ) -> PyResult<Self> {
        let arena = arena.arena.clone();
        let phases = phases
            .into_iter()
            .map(|(name, regions)| {
                (
                    name,
                    regions
                        .into_iter()
                        .map(|region| region.borrow(py).region.clone())
                        .collect(),
                )
            })
            .collect();
        let plan = py
            .detach(|| varena::plan::Plan::new(arena, phases))
            .map_err(|error| super::raise(py, error))?;
        Ok(Self {
            plan: Deferred::new(plan),
            pid: std::process::id(),
        })
    }

    fn activate(&self, py: Python<'_>, phase: &str) -> PyResult<()> {
        self.check()?;
        py.detach(|| self.plan.activate(phase))
            .map_err(|error| match error {
                varena::Error::Unsupported(message) => {
                    pyo3::exceptions::PyNotImplementedError::new_err(message)
                }
                other => super::raise(py, other),
            })
    }

    fn deactivate(&self, py: Python<'_>) -> PyResult<()> {
        self.check()?;
        py.detach(|| self.plan.deactivate())
            .map_err(|error| super::raise(py, error))
    }

    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.check()?;
        let result = PyDict::new(py);
        result.set_item("device", self.plan.device())?;
        result.set_item("scope", "one_arena")?;
        let phases = self
            .plan
            .facts()
            .into_iter()
            .map(|phase| {
                let row = PyDict::new(py);
                row.set_item("name", phase.name)?;
                row.set_item("regions", phase.regions)?;
                row.set_item("bytes", phase.bytes)?;
                Ok(row)
            })
            .collect::<PyResult<Vec<_>>>()?;
        result.set_item("phases", phases)?;
        Ok(result)
    }
}
