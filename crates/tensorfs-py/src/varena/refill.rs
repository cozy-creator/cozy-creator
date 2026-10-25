//! Owning native refill through TensorFS's existing bounded staging engine.
//! A task publishes only after checked producer completion; it never exports an unfinished
//! DLPack destination or calls a Python transformer/cleanup callback from its worker.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use tensorfs::io::source::{IoTally, Item, Mode, Placement, Source};
use tensorfs::io::{self, Dst, ReadControl, ReadStats};

use super::continuation::{self, Continuation, Status};
use super::{raise, PyRegion};
use crate::store::PyCheckpoint;

#[derive(Clone)]
pub(super) enum Failure {
    Storage(tensorfs::Failure),
    Native(varena::Error),
    Canceled,
    Execution(Status),
}

impl Failure {
    pub(super) fn raise(&self, py: Python<'_>) -> PyErr {
        match self {
            Self::Storage(error) => crate::errors::raise(py, error.clone()),
            Self::Native(error) => raise(py, error.clone()),
            Self::Canceled => PyRuntimeError::new_err("native refill canceled"),
            Self::Execution(status) => match status.kind {
                continuation::CANCELED => PyRuntimeError::new_err("native execution canceled"),
                continuation::INVALID => PyValueError::new_err(status.detail()),
                continuation::CAPACITY => {
                    raise(py, varena::Error::OutOfMemory { requested_bytes: 0 })
                }
                continuation::CUDA => raise(
                    py,
                    varena::Error::Cuda {
                        operation: "native continuation",
                        code: status.code,
                    },
                ),
                _ => super::VarenaPoisonedError::new_err(status.detail()),
            },
        }
    }
}

fn execution_failure(status: Status) -> Failure {
    if status.kind == continuation::CANCELED {
        Failure::Canceled
    } else {
        Failure::Execution(status)
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

pub(super) struct Work {
    region: Arc<varena::Region>,
    pin: Option<varena::Pin>,
    continuation: Option<Arc<Continuation>>,
    _checkpoint: tensorfs::Checkpoint,
    source: Arc<Source>,
    items: Vec<Item>,
    fill: Option<varena::Fill>,
    producer: Option<varena::ProducerGuard>,
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
    continuation: Option<Arc<Continuation>>,
    began: Instant,
    read_completed_ns: AtomicU64,
    read_bytes: AtomicU64,
    launch_started_ns: AtomicU64,
    compute_completed_ns: AtomicU64,
    compute_wait_entered_ns: AtomicU64,
    completion_proven: AtomicBool,
}

impl Shared {
    fn elapsed(&self) -> u64 {
        (self.began.elapsed().as_nanos().min(u64::MAX as u128) as u64).saturating_add(1)
    }

    fn cancel(&self) {
        self.cancel.cancel();
        if let Some(execution) = &self.continuation {
            execution.cancel();
        }
    }

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
            self.read_bytes.store(stats.bytes, Ordering::Release);
            self.read_completed_ns
                .store(self.elapsed(), Ordering::Release);
            if self.cancel.canceled() {
                return Err(Failure::Canceled);
            }
            let fill = work.fill.take().expect("successful read owns its fill");
            // SAFETY: the existing engine initialized all logical bytes, including padding,
            // and checked every producer stream. No Python or additional writer exists.
            let pin = unsafe { fill.commit_completed() }.map_err(Failure::Native)?;
            if let Some(execution) = work.continuation.clone() {
                work.pin = Some(pin);
                let launching = self.elapsed();
                execution.start().map_err(execution_failure)?;
                self.launch_started_ns.store(launching, Ordering::Release);
                self.compute_wait_entered_ns
                    .store(self.elapsed(), Ordering::Release);
                let (status, progress) = execution.observe(true);
                if !execution.checked(status, progress) {
                    return Err(Failure::Execution(Status::unproven(&format!(
                        "native continuation did not prove completion: {}",
                        status.detail()
                    ))));
                }
                self.compute_completed_ns
                    .store(self.elapsed(), Ordering::Release);
                status.result().map_err(execution_failure)?;
                if progress.canceled != 0 {
                    return Err(Failure::Canceled);
                }
            } else {
                drop(pin); // Ordinary completed prefetch remains cached without an idle pin.
            }
            Ok(stats)
        }));
        let result = match outcome {
            Ok(result) => result,
            Err(_) => Err(Failure::Native(varena::Error::Poisoned(
                "native refill worker panicked".into(),
            ))),
        };
        if result.is_err() {
            if let Some(execution) = &work.continuation {
                execution.cancel();
            }
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
            if let Some(execution) = &work.continuation {
                if let Err(status) = execution.abort() {
                    let detail = status.detail();
                    self.region.poison(detail.clone());
                    std::mem::forget(work);
                    self.finish(Err(Failure::Native(varena::Error::Poisoned(detail))));
                    return;
                }
                self.compute_completed_ns
                    .store(self.elapsed(), Ordering::Release);
            }
            let sticky = match &result {
                Err(Failure::Native(varena::Error::Poisoned(detail))) => Some(detail.clone()),
                Err(Failure::Native(
                    error @ (varena::Error::Cuda { code, .. }
                    | varena::Error::DeviceFault { code, .. }),
                )) if *code != 2 => Some(error.to_string()),
                Err(Failure::Execution(status))
                    if matches!(status.kind, continuation::INTERNAL | continuation::UNPROVEN)
                        || status.kind == continuation::CUDA && status.code != 2 =>
                {
                    Some(status.detail())
                }
                _ => None,
            };
            if let Some(detail) = sticky.filter(|_| work.continuation.is_some()) {
                // A poisoned native chain cannot supply a reusable ownership certificate,
                // even if a later drain returned success. Keep the actual owners to exit.
                self.region.poison(detail);
                std::mem::forget(work);
                self.finish(result);
                return;
            }
            if let Err(Failure::Execution(status)) = &result {
                if matches!(status.kind, continuation::INTERNAL | continuation::UNPROVEN)
                    || status.kind == continuation::CUDA && status.code != 2
                {
                    self.region.poison(status.detail());
                }
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
        drop(work.pin.take());
        drop(work.producer.take());
        crate::cleanup::defer(work);
        self.completion_proven.store(true, Ordering::Release);
        self.finish(result);
    }
}

pub(super) fn prepare_work(
    region: &Arc<varena::Region>,
    checkpoint: tensorfs::Checkpoint,
    parts: Vec<(String, usize, usize)>,
    threads: usize,
    reclaim: bool,
    initialize: bool,
) -> Result<Option<Work>, Failure> {
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
            .ok_or({ Failure::Native(varena::Error::Invalid("refill target exceeds region")) })?;
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
            at = at
                .checked_add(span.len)
                .ok_or({ Failure::Native(varena::Error::Invalid("refill span overflow")) })?;
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
    let producer = varena::enter_producer(region.device()).map_err(Failure::Native)?;
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
    let mut work = Work {
        region: region.clone(),
        pin: None,
        continuation: None,
        _checkpoint: checkpoint,
        source,
        items,
        fill: Some(fill),
        producer: Some(producer),
        threads: threads.clamp(1, 8),
        completed: false,
    };
    if initialize {
        // The existing read engine owns its explicit copy streams (Dst stream 0 is not PTDS).
        // SAFETY: this dedicated unpublished Fill is exclusive and has no exported view yet.
        let initialized = unsafe {
            io::read(
                &[io::Copy {
                    src: io::Src::Zero,
                    dst: Dst::Device {
                        device: region.device(),
                        ptr: work.fill.as_ref().expect("fill").ptr(),
                        len: region.len(),
                        stream: 0,
                    },
                }],
                &io::ReadOpts {
                    threads: 1,
                    ..Default::default()
                },
            )
        };
        if let Err(error) = initialized {
            if let Err(drain) = io::drain(region.device()) {
                let detail = format!("capture-slot initialization is unproven: {drain}");
                region.poison(detail.clone());
                std::mem::forget(work);
                return Err(Failure::Native(varena::Error::Poisoned(detail)));
            }
            return Err(io_failure(tensorfs::io::source::Error::Read(error)));
        }
        // No producer lease survives into body capture. Actual source I/O acquires its own
        // producer lease only after capture, when the sealed slot is submitted.
        drop(work.producer.take());
    }
    Ok(Some(work))
}

pub(super) struct Control(std::sync::Weak<Shared>);
impl Control {
    pub(super) fn cancel(&self) {
        if let Some(shared) = self.0.upgrade() {
            if shared.pid == std::process::id() {
                shared.cancel();
            }
        }
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

impl Work {
    pub(super) fn capture_pin(&self) -> Result<varena::Pin, Failure> {
        self.fill
            .as_ref()
            .ok_or(Failure::Native(varena::Error::Invalid(
                "load slot has no fill",
            )))?
            .pin()
            .map_err(Failure::Native)
    }
    pub(super) fn abandon(mut self) {
        if self.continuation.is_some() {
            crate::cleanup::defer(Abandoned(Some(self)));
        } else {
            drop(self.fill.take());
            drop(self.producer.take());
            crate::cleanup::defer(self);
        }
    }
}
struct Abandoned(Option<Work>);
impl Drop for Abandoned {
    fn drop(&mut self) {
        if let Some(mut work) = self.0.take() {
            if let Some(execution) = &work.continuation {
                if let Err(status) = execution.abort() {
                    work.region.poison(status.detail());
                    std::mem::forget(work);
                    return;
                }
            }
            drop(work.fill.take());
            drop(work.pin.take());
            drop(work.producer.take());
        }
    }
}

impl PreparedRead {
    pub(super) fn control(&self) -> Control {
        Control(Arc::downgrade(
            self.shared.as_ref().expect("live prepared read"),
        ))
    }
    pub(super) fn request_cancel(&self) {
        self.shared.as_ref().expect("live prepared read").cancel();
    }
    pub(super) fn composed(mut work: Work, execution: Arc<Continuation>) -> Result<Self, Failure> {
        work.continuation = Some(execution.clone());
        let producer = match varena::enter_producer(work.region.device()) {
            Ok(producer) => producer,
            Err(error) => {
                work.abandon();
                return Err(Failure::Native(error));
            }
        };
        work.producer = Some(producer);
        Ok(Self {
            shared: Some(Arc::new(Shared {
                region: work.region.clone(),
                cancel: ReadControl::default(),
                state: Mutex::new(State {
                    work: Some(work),
                    started: false,
                    result: None,
                }),
                changed: Condvar::new(),
                pid: std::process::id(),
                continuation: Some(execution),
                began: Instant::now(),
                read_completed_ns: AtomicU64::new(0),
                read_bytes: AtomicU64::new(0),
                launch_started_ns: AtomicU64::new(0),
                compute_completed_ns: AtomicU64::new(0),
                compute_wait_entered_ns: AtomicU64::new(0),
                completion_proven: AtomicBool::new(false),
            })),
        })
    }
    pub(super) fn progress(&self, py: Python<'_>, wait: bool) -> PyResult<(u32, bool, bool)> {
        let shared = self.shared()?.clone();
        py.detach(|| {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while wait && state.started && state.result.is_none() {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
            let result = state.result.clone();
            drop(state);
            let (status, progress) = shared
                .continuation
                .as_ref()
                .expect("composed read")
                .observe(false);
            if !matches!(status.kind, continuation::OK | continuation::CANCELED) {
                return Err(execution_failure(status));
            }
            let canceled =
                matches!(&result, Some(Err(Failure::Canceled))) || progress.canceled != 0;
            if let Some(Err(error)) = result.as_ref() {
                if !matches!(error, Failure::Canceled) {
                    return Err(error.clone());
                }
            }
            // Provider cancel can settle its armed epoch while a copy is still draining.
            // Only the whole worker's result is the composed completion certificate.
            Ok((progress.completed_steps, canceled, result.is_some()))
        })
        .map_err(|error| error.raise(py))
    }
    pub(super) fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let shared = self.shared()?;
        let row = PyDict::new(py);
        let result = py.detach(|| {
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .result
                .clone()
        });
        row.set_item("issued_bytes", shared.cancel.issued_bytes())?;
        let bytes = shared.read_bytes.load(Ordering::Acquire);
        row.set_item("bytes", bytes)?;
        row.set_item("read_bytes", bytes)?;
        row.set_item(
            "canceled",
            shared.cancel.canceled() || matches!(result, Some(Err(Failure::Canceled))),
        )?;
        for (name, clock) in [
            ("read_completed_ns", &shared.read_completed_ns),
            ("launch_started_ns", &shared.launch_started_ns),
            ("compute_completed_ns", &shared.compute_completed_ns),
            ("compute_wait_entered_ns", &shared.compute_wait_entered_ns),
        ] {
            row.set_item(name, clock.load(Ordering::Acquire))?;
        }
        row.set_item("clock", "elapsed_ns_since_load_submit")?;
        row.set_item(
            "read_done",
            shared.read_completed_ns.load(Ordering::Acquire) != 0,
        )?;
        row.set_item(
            "compute_wait_entered",
            shared.compute_wait_entered_ns.load(Ordering::Acquire) != 0,
        )?;
        row.set_item(
            "compute_done",
            shared.compute_completed_ns.load(Ordering::Acquire) != 0,
        )?;
        row.set_item(
            "completion_proven",
            shared.completion_proven.load(Ordering::Acquire),
        )?;
        row.set_item("done", result.is_some())?;
        row.set_item("failed", matches!(result, Some(Err(_))))?;
        Ok(row)
    }
}

#[pymethods]
impl PreparedRead {
    #[classattr]
    const NO_RECLAIM: bool = true;
    #[classattr]
    const CAPTURE_GUARD: bool = true;

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
            .detach(|| prepare_work(&region, checkpoint, parts, threads, reclaim, false))
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
                continuation: None,
                began: Instant::now(),
                read_completed_ns: AtomicU64::new(0),
                read_bytes: AtomicU64::new(0),
                launch_started_ns: AtomicU64::new(0),
                compute_completed_ns: AtomicU64::new(0),
                compute_wait_entered_ns: AtomicU64::new(0),
                completion_proven: AtomicBool::new(false),
            })),
        })
    }

    pub(super) fn start(&self, py: Python<'_>) -> PyResult<()> {
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
            if shared.cancel.canceled() && shared.continuation.is_none() {
                let mut work = state.work.take();
                drop(state);
                if let Some(work) = &mut work {
                    drop(work.fill.take());
                    drop(work.producer.take());
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

    pub(super) fn cancel(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared()?.clone();
        shared.cancel();
        py.detach(|| {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.started && state.result.is_none() {
                // No worker can own or issue this fill while the state lock is held.
                // Release its pin before publishing canceled completion; source teardown
                // remains on the native cleanup queue, never under the GIL.
                let mut work = state.work.take();
                if let Some(work) = &mut work {
                    drop(work.fill.take());
                    drop(work.producer.take());
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
        let unstarted = py.detach(|| {
            let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            !state.started && state.result.is_none()
        });
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
                shared.cancel();
                // A running worker owns another Arc and performs its own checked cleanup.
                // If this is the last owner, no worker/waiter can hold State's mutex.
                if let Some(shared) = Arc::into_inner(shared) {
                    let Shared { region, state, .. } = shared;
                    let mut work = state.into_inner().unwrap_or_else(|e| e.into_inner()).work;
                    if let Some(work) = work.take() {
                        work.abandon();
                    }
                    super::release_region(region);
                }
            }
        }
    }
}
