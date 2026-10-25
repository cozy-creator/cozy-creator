//! Owning native refill through TensorFS's existing bounded staging engine.
//! A task publishes only after checked producer completion; it never exports an unfinished
//! DLPack destination or calls a Python transformer/cleanup callback from its worker.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use tensorfs::io::source::{IoTally, Item, Mode, Placement, Source};
use tensorfs::io::{self, Dst, ReadControl, ReadStats};

use super::{raise, PyRegion};
use crate::store::PyCheckpoint;

#[derive(Clone)]
enum Failure {
    Storage(tensorfs::Failure),
    Native(varena::Error),
    Canceled,
}

impl Failure {
    fn raise(&self, py: Python<'_>) -> PyErr {
        match self {
            Self::Storage(error) => crate::errors::raise(py, error.clone()),
            Self::Native(error) => raise(py, error.clone()),
            Self::Canceled => PyRuntimeError::new_err("native refill canceled"),
        }
    }
}

fn io_failure(error: tensorfs::io::source::Error) -> Failure {
    match error {
        tensorfs::io::source::Error::Source(error) => Failure::Storage(error),
        tensorfs::io::source::Error::Read(error) => {
            if error.source.kind() == std::io::ErrorKind::Interrupted {
                return Failure::Canceled;
            }
            if let Some(cuda) = error
                .source
                .get_ref()
                .and_then(|e| e.downcast_ref::<io::cuda::Error>())
            {
                return cuda_failure(cuda.clone());
            }
            Failure::Storage(error.source.into())
        }
    }
}

fn cuda_failure(error: io::cuda::Error) -> Failure {
    match error {
        io::cuda::Error::Cuda { op, code, .. } => Failure::Native(varena::Error::Cuda {
            operation: op,
            code,
        }),
        io::cuda::Error::CudaUnavailable(detail) => {
            Failure::Native(varena::Error::Unsupported(detail))
        }
    }
}

struct Work {
    _checkpoint: tensorfs::Checkpoint,
    source: Arc<Source>,
    items: Vec<Item>,
    fill: Option<varena::Fill>,
    threads: usize,
    completed: bool,
}

struct State {
    work: Option<Work>,
    started: bool,
    result: Option<Result<ReadStats, Failure>>,
}

struct Shared {
    region: Arc<varena::Region>,
    cancel: ReadControl,
    state: Mutex<State>,
    changed: Condvar,
    pid: u32,
}

impl Shared {
    fn finish(&self, result: Result<ReadStats, Failure>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).result = Some(result);
        self.changed.notify_all();
    }

    fn run(self: Arc<Self>, mut work: Work) {
        let device = self.region.device();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let fill = work.fill.as_ref().expect("worker owns uncommitted fill");
            let placement = Placement {
                items: &work.items,
                origin: 0,
                dst: Dst::Device {
                    device,
                    ptr: fill.ptr(),
                    len: fill.len(),
                    stream: 0,
                },
            };
            // SAFETY: Work owns the checkpoint/source/items and the Fill's physical pin.
            // The engine validates all spans, initializes padding and joins submitted work.
            let stats = unsafe {
                work.source.read_placements_cancelable(
                    &[placement],
                    work.threads,
                    Mode::Cache,
                    &IoTally::default(),
                    &self.cancel,
                )
            }
            .map_err(io_failure)?;
            work.completed = true;
            if self.cancel.canceled() {
                return Err(Failure::Canceled);
            }
            let fill = work.fill.take().expect("successful read owns its fill");
            // SAFETY: the existing engine initialized all logical bytes, including padding,
            // and checked every producer stream. No Python or additional writer exists.
            let pin = unsafe { fill.commit_completed() }.map_err(Failure::Native)?;
            drop(pin); // A completed prefetch remains cached without an idle physical pin.
            Ok(stats)
        }));
        let result = match outcome {
            Ok(result) => result,
            Err(_) => Err(Failure::Native(varena::Error::Poisoned(
                "native refill worker panicked".into(),
            ))),
        };
        if result.is_err() {
            // Errors and cancellation are not a completion certificate. This engine owns
            // every producer stream here: inputs are File/Inline, never borrowed DLPack.
            let drained = if work.completed {
                Ok(())
            } else {
                io::drain(device)
            };
            if let Err(error) = drained {
                let detail = format!("native refill completion is unproven: {error}");
                self.region.poison(detail.clone());
                // Retain the ACTUAL Fill, reservation, checkpoint and source, not pointers.
                std::mem::forget(work);
                self.finish(Err(Failure::Native(varena::Error::Poisoned(detail))));
                return;
            }
            if let Err(Failure::Native(varena::Error::Poisoned(detail))) = &result {
                self.region.poison(detail.clone());
            }
            if let Err(Failure::Native(
                error
                @ (varena::Error::Cuda { code, .. } | varena::Error::DeviceFault { code, .. }),
            )) = &result
            {
                if *code != 2 {
                    self.region
                        .poison(format!("native refill device failure: {error}"));
                }
            }
            // Checked drain succeeded. Abort the unpublished fill before reporting failure.
            drop(work.fill.take());
        }
        crate::cleanup::defer(work);
        self.finish(result);
    }
}

#[pyclass(name = "_VarenaPreparedRead", module = "tensorfs._ext", frozen)]
pub(super) struct PreparedRead {
    shared: Option<Arc<Shared>>,
}

impl PreparedRead {
    fn shared(&self) -> PyResult<&Arc<Shared>> {
        let shared = self.shared.as_ref().expect("live prepared read");
        if shared.pid != std::process::id() {
            return Err(PyRuntimeError::new_err(
                "native refill cannot be used after fork",
            ));
        }
        Ok(shared)
    }

    fn snapshot<'py>(&self, py: Python<'py>, wait: bool) -> PyResult<Bound<'py, PyDict>> {
        let shared = self.shared()?.clone();
        let (started, result) = py.detach(|| {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while wait && state.started && state.result.is_none() {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
            (state.started, state.result.clone())
        });
        let row = PyDict::new(py);
        row.set_item("started", started)?;
        row.set_item("issued_bytes", self.shared()?.cancel.issued_bytes())?;
        row.set_item("done", result.is_some())?;
        row.set_item("canceled", matches!(&result, Some(Err(Failure::Canceled))))?;
        match result {
            Some(Ok(stats)) => {
                row.set_item("bytes", stats.bytes)?;
                row.set_item("cached_bytes", stats.cached)?;
                row.set_item("direct_bytes", stats.direct)?;
                row.set_item("staged_bytes", stats.staged)?;
                row.set_item("busy_ns", stats.busy_ns)?;
            }
            Some(Err(Failure::Canceled)) | None => {}
            Some(Err(error)) => return Err(error.raise(py)),
        }
        Ok(row)
    }
}

#[pymethods]
impl PreparedRead {
    #[classattr]
    const NO_RECLAIM: bool = true;

    #[new]
    #[pyo3(signature = (region, checkpoint, parts, threads = 2, reclaim = true))]
    fn new(
        py: Python<'_>,
        region: PyRef<'_, PyRegion>,
        checkpoint: PyRef<'_, PyCheckpoint>,
        parts: Vec<(String, usize, usize)>,
        threads: usize,
        reclaim: bool,
    ) -> PyResult<Self> {
        let region = region.region.clone();
        let checkpoint = checkpoint.core(py)?;
        let prepared = py
            .detach(|| -> Result<_, Failure> {
                let names: Vec<String> = parts.iter().map(|p| p.0.clone()).collect();
                let spans = checkpoint.spans(&[names]).map_err(Failure::Storage)?;
                let mut by_name = BTreeMap::<String, Vec<_>>::new();
                for span in spans {
                    by_name.entry(span.what.clone()).or_default().push(span);
                }
                let mut items = Vec::new();
                let mut ranges = Vec::new();
                for (name, offset, length) in parts {
                    let end = offset
                        .checked_add(length)
                        .filter(|end| *end <= region.len())
                        .ok_or({
                            Failure::Native(varena::Error::Invalid("refill target exceeds region"))
                        })?;
                    let spans = by_name.get(&name).map_or(&[][..], Vec::as_slice);
                    if spans.is_empty() && length != 0 {
                        return Err(Failure::Native(varena::Error::Invalid(
                            "refill names no stored part",
                        )));
                    }
                    let mut at = offset as u64;
                    for span in spans {
                        items.push(Item {
                            offset: at,
                            len: span.len,
                            source: span.source.clone(),
                            part_end: false,
                        });
                        at = at.checked_add(span.len).ok_or({
                            Failure::Native(varena::Error::Invalid("refill span overflow"))
                        })?;
                    }
                    if at != end as u64 {
                        return Err(Failure::Native(varena::Error::Invalid(
                            "refill length differs from stored part",
                        )));
                    }
                    if let Some(item) = items.last_mut() {
                        item.part_end = true;
                    }
                    if length != 0 {
                        ranges.push((offset, end));
                    }
                }
                ranges.sort_unstable();
                if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                    return Err(Failure::Native(varena::Error::Invalid(
                        "refill targets overlap",
                    )));
                }
                let preparation = if reclaim {
                    region.prepare()
                } else {
                    region.prepare_without_reclaim()
                }
                .map_err(Failure::Native)?;
                let fill = match preparation {
                    varena::Preparation::Resident(pin) => {
                        drop(pin);
                        return Ok(None);
                    }
                    varena::Preparation::Fill(fill) => fill,
                };
                // This really initializes bounded staging and streams, before a resident graph.
                io::prepare_device(region.device()).map_err(cuda_failure)?;
                let source = Arc::new(checkpoint.local_source(true));
                Ok(Some(Work {
                    _checkpoint: checkpoint,
                    source,
                    items,
                    fill: Some(fill),
                    threads: threads.clamp(1, 8),
                    completed: false,
                }))
            })
            .map_err(|e| e.raise(py))?;
        let cached = prepared.is_none();
        Ok(Self {
            shared: Some(Arc::new(Shared {
                region,
                cancel: ReadControl::default(),
                state: Mutex::new(State {
                    work: prepared,
                    started: false,
                    result: cached.then_some(Ok(ReadStats::default())),
                }),
                changed: Condvar::new(),
                pid: std::process::id(),
            })),
        })
    }

    fn start(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared()?.clone();
        py.detach(|| {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.started {
                return Err("native refill already started".to_owned());
            }
            state.started = true;
            if state.result.is_some() {
                return Ok(());
            }
            if shared.cancel.canceled() {
                let mut work = state.work.take();
                drop(state);
                if let Some(work) = &mut work {
                    drop(work.fill.take());
                }
                crate::cleanup::defer(work);
                shared.finish(Err(Failure::Canceled));
                return Ok(());
            }
            // Keep Work recoverable if the operating system refuses to start a worker.
            let task = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("tensorfs-refill".into())
                .spawn(move || {
                    let work = task
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .work
                        .take()
                        .expect("started refill owns work");
                    task.run(work);
                });
            if let Err(error) = spawned {
                state.started = false;
                return Err(format!("native refill worker could not start: {error}"));
            }
            Ok(())
        })
        .map_err(PyRuntimeError::new_err)
    }

    fn cancel(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared()?.clone();
        shared.cancel.cancel();
        py.detach(|| {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.started && state.result.is_none() {
                // No worker can own or issue this fill while the state lock is held.
                // Release its pin before publishing canceled completion; source teardown
                // remains on the native cleanup queue, never under the GIL.
                let mut work = state.work.take();
                if let Some(work) = &mut work {
                    drop(work.fill.take());
                }
                crate::cleanup::defer(work);
                state.result = Some(Err(Failure::Canceled));
            }
        });
        shared.changed.notify_all();
        Ok(())
    }

    fn poll<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.snapshot(py, false)
    }

    fn wait<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let shared = self.shared()?;
        let unstarted = {
            let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            !state.started && state.result.is_none()
        };
        if unstarted {
            return Err(PyValueError::new_err("start native refill before waiting"));
        }
        self.snapshot(py, true)
    }
}

impl Drop for PreparedRead {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            if shared.pid != std::process::id() {
                std::mem::forget(shared);
            } else {
                shared.cancel.cancel();
                crate::cleanup::defer(shared);
            }
        }
    }
}
