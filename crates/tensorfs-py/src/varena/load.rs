//! One dedicated, initialized capture slot followed by exactly one native load/execution.
use std::sync::{Arc, Mutex};

use pyo3::{exceptions::PyValueError, prelude::*, types::PyDict};

use super::continuation::Continuation;
use super::{
    Deferred, PyArena, PyRegion, capsule,
    refill::{self, Control, PreparedRead, Work},
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
    inputs: usize,
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
    #[classattr]
    fn supports_event_input() -> bool {
        true
    }

    #[classattr]
    fn supports_ready_stage() -> bool {
        true
    }

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
            inputs: 1,
            state: Mutex::new(State {
                work: Some(work),
                sealed: false,
                canceled: false,
                submitted: false,
                control: None,
            }),
        })
    }
    #[staticmethod]
    #[pyo3(signature = (arena, checkpoint, component, key, *, dtype, max_scratch_bytes, threads = 2))]
    #[allow(clippy::too_many_arguments)]
    fn encoded(
        py: Python<'_>,
        arena: PyRef<'_, PyArena>,
        checkpoint: PyRef<'_, PyCheckpoint>,
        component: String,
        key: String,
        dtype: String,
        max_scratch_bytes: usize,
        threads: usize,
    ) -> PyResult<Self> {
        let arena = arena.arena.clone();
        let checkpoint = checkpoint.core(py)?;
        let (region, work) = py
            .detach(|| {
                refill::prepare_encoded(
                    &arena,
                    checkpoint,
                    &component,
                    &key,
                    tensorfs::format::Dtype::parse(&dtype),
                    max_scratch_bytes,
                    threads,
                )
            })
            .map_err(|error| error.raise(py))?;
        Ok(Self {
            region: Deferred::new(region),
            pid: std::process::id(),
            inputs: 1,
            state: Mutex::new(State {
                work: Some(work),
                sealed: false,
                canceled: false,
                submitted: false,
                control: None,
            }),
        })
    }
    #[staticmethod]
    #[pyo3(signature = (checkpoint, groups, threads = 2, *, keep_ready = false))]
    #[allow(clippy::type_complexity)]
    fn from_regions(
        py: Python<'_>,
        checkpoint: PyRef<'_, PyCheckpoint>,
        groups: Vec<(Py<PyRegion>, Vec<(String, usize, usize)>)>,
        threads: usize,
        keep_ready: bool,
    ) -> PyResult<Option<Self>> {
        let groups: Vec<_> = groups
            .into_iter()
            .map(|(region, parts)| (region.borrow(py).region.clone(), parts))
            .collect();
        let region = groups
            .first()
            .map(|(region, _)| region.clone())
            .ok_or_else(|| PyValueError::new_err("load needs input regions"))?;
        let checkpoint = checkpoint.core(py)?;
        let work = py
            .detach(|| refill::prepare_regions(checkpoint, groups, threads, keep_ready))
            .map_err(|error| error.region_refusal().raise(py))?;
        Ok(work.map(|work| Self {
            region: Deferred::new(region),
            inputs: work.input_count(),
            pid: std::process::id(),
            state: Mutex::new(State {
                work: Some(work),
                sealed: false,
                canceled: false,
                submitted: false,
                control: None,
            }),
        }))
    }
    #[getter]
    fn input_count(&self) -> usize {
        self.inputs
    }
    fn capture_inputs(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        self.valid()?;
        let pins = py
            .detach(|| {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.sealed {
                    return Err(refill::Failure::Native(varena::Error::Invalid(
                        "load slot capture is sealed",
                    )));
                }
                let work =
                    state
                        .work
                        .as_ref()
                        .ok_or(refill::Failure::Native(varena::Error::Invalid(
                            "load slot is closed",
                        )))?;
                (0..self.inputs)
                    .map(|index| work.capture_input(index))
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| error.raise(py))?;
        pins.into_iter()
            .map(|pin| capsule(py, pin).map(Bound::unbind))
            .collect()
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
        if self.inputs != 1 {
            return Err(PyValueError::new_err(
                "use capture_inputs for multiple load regions",
            ));
        }
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
    fn detach_source(&self, py: Python<'_>) -> PyResult<StageSource> {
        self.valid()?;
        let work = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.sealed || state.submitted || state.canceled {
                return Err(PyValueError::new_err(
                    "detach a sealed, unused capture slot",
                ));
            }
            state.canceled = true;
            state
                .work
                .take()
                .ok_or_else(|| PyValueError::new_err("capture slot is closed"))?
        };
        let source = py
            .detach(|| work.detach_capture())
            .map_err(|error| error.raise(py))?;
        Ok(StageSource {
            source: Deferred::new(Arc::new(source)),
            pid: std::process::id(),
        })
    }

    #[pyo3(signature = (capsule, *, event_ordered = false))]
    fn submit_native(
        &self,
        py: Python<'_>,
        capsule: &Bound<'_, PyAny>,
        event_ordered: bool,
    ) -> PyResult<LoadJob> {
        self.valid()?;
        let execution = Arc::new(Continuation::import(capsule, self.region.device())?);
        if event_ordered && !execution.supports_event_input() {
            execution.abandon();
            return Err(PyValueError::new_err(
                "event-ordered loading requires a matching native continuation",
            ));
        }
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
            .detach(|| PreparedRead::composed(work, execution, event_ordered))
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

/// Internal raw stage descriptor. It owns actual Region reservations and immutable source
/// spans; every submission reacquires physical pins before its native continuation.
#[pyclass(name = "_VarenaStageSource", module = "tensorfs._ext", frozen)]
pub(super) struct StageSource {
    source: Deferred<Arc<refill::StageSource>>,
    pid: u32,
}
#[pymethods]
impl StageSource {
    #[classattr]
    fn supports_reusable_input() -> bool {
        true
    }

    #[pyo3(signature = (capsule, *, event_ordered = false))]
    fn continuation(
        &self,
        py: Python<'_>,
        capsule: &Bound<'_, PyAny>,
        event_ordered: bool,
    ) -> PyResult<(Py<PyAny>, Py<super::stage_continuation::StageJob>)> {
        if self.pid != std::process::id() {
            return Err(PyValueError::new_err(
                "stage source cannot be used after fork",
            ));
        }
        super::stage_continuation::export(py, self.source.clone(), capsule, event_ordered)
    }

    #[pyo3(signature = (capsule, *, event_ordered = false))]
    fn submit_native(
        &self,
        py: Python<'_>,
        capsule: &Bound<'_, PyAny>,
        event_ordered: bool,
    ) -> PyResult<LoadJob> {
        if self.pid != std::process::id() {
            return Err(PyValueError::new_err(
                "stage source cannot be used after fork",
            ));
        }
        let execution = Arc::new(Continuation::import(capsule, self.source.device())?);
        if event_ordered && !execution.supports_event_input() {
            execution.abandon();
            return Err(PyValueError::new_err(
                "event-ordered stage requires matching continuation",
            ));
        }
        let source = self.source.clone();
        let work = match py.detach(|| source.prepare()) {
            Ok(work) => work,
            Err(error) => {
                execution.abandon();
                return Err(error.raise(py));
            }
        };
        let read = py
            .detach(|| PreparedRead::composed(work, execution, event_ordered))
            .map_err(|error| error.raise(py))?;
        read.start(py)?;
        Ok(LoadJob { read })
    }
}
