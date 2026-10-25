//! One dedicated, initialized capture slot followed by exactly one native load/execution.
use std::sync::{Arc, Mutex};

use pyo3::{exceptions::PyValueError, prelude::*, types::PyDict};

use super::continuation::Continuation;
use super::{
    capsule,
    refill::{self, Control, PreparedRead, Work},
    Deferred, PyArena,
};
use crate::store::PyCheckpoint;

struct State {
    work: Option<Work>,
    sealed: bool,
    canceled: bool,
    submitted: bool,
    control: Option<Control>,
}

#[pyclass(name = "_VarenaLoadSlot", module = "tensorfs._ext", frozen)]
pub(super) struct LoadSlot {
    region: Deferred<Arc<varena::Region>>,
    state: Mutex<State>,
    pid: u32,
}
impl LoadSlot {
    fn valid(&self) -> PyResult<()> {
        if self.pid != std::process::id() {
            return Err(PyValueError::new_err("load slot cannot be used after fork"));
        }
        Ok(())
    }
}
#[pymethods]
impl LoadSlot {
    #[new]
    #[pyo3(signature = (arena, checkpoint, parts, bytes, threads = 2))]
    fn new(
        py: Python<'_>,
        arena: PyRef<'_, PyArena>,
        checkpoint: PyRef<'_, PyCheckpoint>,
        parts: Vec<(String, usize, usize)>,
        bytes: usize,
        threads: usize,
    ) -> PyResult<Self> {
        if bytes == 0 || !parts.iter().any(|p| p.2 != 0) {
            return Err(PyValueError::new_err(
                "load slot needs nonempty stored parts and destination",
            ));
        }
        let arena = arena.arena.clone();
        let checkpoint = checkpoint.core(py)?;
        let (region, work) = py
            .detach(|| {
                let region = arena.reserve(bytes).map_err(refill::Failure::Native)?;
                let work = refill::prepare_work(&region, checkpoint, parts, threads, true, true)?
                    .ok_or(refill::Failure::Native(varena::Error::Invalid(
                    "fresh load slot was already resident",
                )))?;
                Ok::<_, refill::Failure>((region, work))
            })
            .map_err(|e| e.raise(py))?;
        Ok(Self {
            region: Deferred::new(region),
            pid: std::process::id(),
            state: Mutex::new(State {
                work: Some(work),
                sealed: false,
                canceled: false,
                submitted: false,
                control: None,
            }),
        })
    }
    #[getter]
    fn device(&self) -> i32 {
        self.region.device()
    }
    #[getter]
    fn ptr(&self) -> u64 {
        self.region.ptr()
    }
    fn __dlpack_device__(&self) -> (i32, i32) {
        (2, self.region.device())
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
        self.valid()?;
        // Initialization joined the engine's explicit streams before exposing this capture-only view.
        let _ = (stream, max_version);
        if copy == Some(true) || dl_device.is_some_and(|d| d != (2, self.region.device())) {
            return Err(PyValueError::new_err(
                "load slot exports its original capture storage only",
            ));
        }
        let pin = py
            .detach(|| {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.sealed {
                    return Err(refill::Failure::Native(varena::Error::Invalid(
                        "load slot capture is sealed",
                    )));
                }
                state
                    .work
                    .as_ref()
                    .ok_or(refill::Failure::Native(varena::Error::Invalid(
                        "load slot is closed",
                    )))?
                    .capture_pin()
            })
            .map_err(|e| e.raise(py))?;
        capsule(py, pin)
    }
    fn seal(&self) -> PyResult<()> {
        self.valid()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.work.is_none() || state.sealed {
            return Err(PyValueError::new_err(
                "load slot is closed or already sealed",
            ));
        }
        state.sealed = true;
        Ok(())
    }
    fn submit_native(&self, py: Python<'_>, capsule: &Bound<'_, PyAny>) -> PyResult<LoadJob> {
        self.valid()?;
        let execution = Arc::new(Continuation::import(capsule, self.region.device())?);
        let taken = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.sealed || state.submitted {
                None
            } else {
                state.submitted = true;
                state.work.take()
            }
        };
        let Some(work) = taken else {
            // No read can have started. Retire this arm on the existing cleanup worker.
            execution.abandon();
            return Err(PyValueError::new_err(
                "seal load slot before its one submission",
            ));
        };
        let read = py
            .detach(|| PreparedRead::composed(work, execution))
            .map_err(|e| e.raise(py))?;
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.control = Some(read.control());
            if state.canceled {
                read.request_cancel();
            }
        }
        read.start(py)?;
        Ok(LoadJob { read })
    }
    fn cancel(&self) -> PyResult<()> {
        self.valid()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.canceled = true;
        if let Some(control) = &state.control {
            control.cancel();
        }
        Ok(())
    }
    fn close(&self) -> PyResult<()> {
        self.valid()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.canceled = true;
        if let Some(control) = &state.control {
            control.cancel();
        }
        if let Some(work) = state.work.take() {
            work.abandon();
        }
        Ok(())
    }
}
impl Drop for LoadSlot {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if self.pid != std::process::id() {
            std::mem::forget(state.work.take());
            return;
        }
        if let Some(control) = &state.control {
            control.cancel();
        }
        if let Some(work) = state.work.take() {
            work.abandon();
        }
    }
}

#[pyclass(name = "_VarenaLoadJob", module = "tensorfs._ext", frozen)]
pub(super) struct LoadJob {
    read: PreparedRead,
}
#[pymethods]
impl LoadJob {
    fn poll(&self, py: Python<'_>) -> PyResult<(u32, bool, bool)> {
        self.read.progress(py, false)
    }
    fn wait(&self, py: Python<'_>) -> PyResult<(u32, bool)> {
        self.read
            .progress(py, true)
            .map(|(steps, canceled, _)| (steps, canceled))
    }
    fn cancel(&self, py: Python<'_>) -> PyResult<()> {
        self.read.cancel(py)
    }
    fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.read.facts(py)
    }
}
