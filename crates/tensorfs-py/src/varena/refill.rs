//! Owning native refill through TensorFS's existing bounded staging engine.
//! A task publishes only after checked producer completion; it never exports an unfinished
//! DLPack destination or calls a Python transformer/cleanup callback from its worker.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use tensorfs::io::source::{IoTally, Item, Mode, Placement, Source};
use tensorfs::io::{self, Dst, ReadControl, ReadStats};

use super::continuation::{self, Continuation, Status};
use super::{PyRegion, raise};
use crate::store::PyCheckpoint;

static OBSERVATION_CLOCK: OnceLock<Instant> = OnceLock::new();
fn monotonic_ns() -> u64 {
    (OBSERVATION_CLOCK
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u64::MAX as u128) as u64)
        .saturating_add(1)
}

#[derive(Clone)]
pub(super) enum Failure {
    Storage(tensorfs::Failure),
    Native(varena::Error),
    Canceled,
    Execution(Box<Status>),
    /// A stored chunk a region load needs is not on local disk. Non-fatal: the frontend
    /// falls back to an eager, fetch-capable read that downloads it.
    NotLocal(String),
    /// A pre-submission region-load refusal (busy, capture-active, ineligible descriptor).
    /// Non-fatal: the frontend falls back to an eager resident load.
    Ineligible(String),
}

impl Failure {
    pub(super) fn detail(&self) -> String {
        match self {
            Self::Storage(error) => error.to_string(),
            Self::Native(error) => error.to_string(),
            Self::Canceled => "native refill canceled".into(),
            Self::Execution(status) => status.detail(),
            Self::NotLocal(detail) | Self::Ineligible(detail) => detail.clone(),
        }
    }
    /// Pre-submission region-load refusals are not fatal: the frontend can still load the
    /// component eagerly. `Busy`/`CaptureActive`/an ineligible descriptor become a marked
    /// refusal so Python falls back; OOM, device faults and poisoning stay fatal.
    pub(super) fn region_refusal(self) -> Self {
        match self {
            Self::Native(varena::Error::Busy) => Self::Ineligible(
                "native load region is busy (concurrent producer or live capture alias)".into(),
            ),
            Self::Native(varena::Error::CaptureActive) => {
                Self::Ineligible("native load cannot prepare during CUDA graph capture".into())
            }
            Self::Native(varena::Error::Invalid(detail)) => {
                Self::Ineligible(format!("native load input is ineligible: {detail}"))
            }
            other => other,
        }
    }
    pub(super) fn raise(&self, py: Python<'_>) -> PyErr {
        match self {
            Self::Storage(error) => crate::errors::raise(py, error.clone()),
            Self::Native(error) => raise(py, error.clone()),
            Self::Canceled => PyRuntimeError::new_err("native refill canceled"),
            // A non-fatal refusal, marked so Runtime's `_native_error` maps it to the matching
            // CapabilityError and SDXL falls back to eager instead of failing the request.
            Self::NotLocal(detail) => {
                let err = PyRuntimeError::new_err(detail.clone());
                let _ = err.value(py).setattr("load_unavailable", true);
                err
            }
            Self::Ineligible(detail) => {
                let err = PyRuntimeError::new_err(detail.clone());
                let _ = err.value(py).setattr("eligibility_refusal", true);
                err
            }
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
        Failure::Execution(Box::new(status))
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

struct Input {
    region: Arc<varena::Region>,
    items: Vec<Item>,
    fill: Option<varena::Fill>,
    pin: Option<varena::Pin>,
}

pub(super) struct Work {
    region: Arc<varena::Region>,
    inputs: Vec<Input>,
    continuation: Option<Arc<Continuation>>,
    _checkpoint: tensorfs::Checkpoint,
    source: Arc<Source>,
    producer: Option<varena::ProducerGuard>,
    threads: usize,
    completed: bool,
    decode: Option<Decode>,
    event_ordered: bool,
    prepared_decode: Option<Mutex<tensorfs::raw::DeviceDecode>>,
    lookahead: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// A captured stage owns addresses/source identity, not graph-lifetime physical pins.
/// Created only by consuming a sealed, never-submitted raw capture Work.
pub(super) struct StageSource {
    inputs: Vec<(Arc<varena::Region>, Vec<Item>)>,
    checkpoint: tensorfs::Checkpoint,
    source: Arc<Source>,
    threads: usize,
}

impl StageSource {
    pub(super) fn device(&self) -> i32 {
        self.inputs[0].0.device()
    }

    pub(super) fn reservation_bytes(&self) -> usize {
        self.inputs.iter().fold(0usize, |total, (region, _)| {
            total.saturating_add(region.allocated_bytes())
        })
    }

    pub(super) fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub(super) fn prepare(&self) -> Result<Work, Failure> {
        self.prepare_with_reclaim(true)
    }

    /// No allocation until the caller has explicitly admitted this bounded future stage.
    /// Existing arena no-reclaim admission enforces its current streaming window as well.
    pub(super) fn prepare_speculative(&self, budget: usize) -> Result<Option<Work>, Failure> {
        if self.reservation_bytes() > budget {
            return Ok(None);
        }
        if self
            .inputs
            .iter()
            .try_fold(true, |ready, (region, _)| {
                region.is_resident().map(|resident| ready && resident)
            })
            .map_err(Failure::Native)?
        {
            return Ok(None); // Demand atomically pins ready inputs; do not add a no-op worker.
        }
        match self.prepare_with_reclaim(false) {
            Ok(work) => Ok(Some(work)),
            Err(Failure::Native(
                varena::Error::OutOfMemory { .. }
                | varena::Error::Busy
                | varena::Error::CaptureActive,
            )) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn prepare_with_reclaim(&self, reclaim: bool) -> Result<Work, Failure> {
        // Keep ordinary/source-identity ownership: no new region or private bank exists.
        let mut inputs: Vec<_> = self
            .inputs
            .iter()
            .map(|(region, items)| Input {
                region: region.clone(),
                items: items.clone(),
                fill: None,
                pin: None,
            })
            .collect();
        for input in &mut inputs {
            input.pin = input.region.try_pin().map_err(Failure::Native)?;
        }
        let producer = varena::enter_producer(self.device()).map_err(Failure::Native)?;
        for input in &mut inputs {
            if input.pin.is_none() {
                match (if reclaim {
                    input.region.prepare()
                } else {
                    input.region.prepare_without_reclaim()
                })
                .map_err(Failure::Native)?
                {
                    varena::Preparation::Resident(pin) => input.pin = Some(pin),
                    varena::Preparation::Fill(fill) => input.fill = Some(fill),
                }
            }
        }
        io::prepare_device(self.device()).map_err(cuda_failure)?;
        Ok(Work {
            region: inputs[0].region.clone(),
            inputs,
            continuation: None,
            _checkpoint: self.checkpoint.clone(),
            source: self.source.clone(),
            producer: Some(producer),
            threads: self.threads,
            completed: false,
            decode: None,
            event_ordered: false,
            prepared_decode: None,
            lookahead: None,
        })
    }
}

struct Decode {
    encoding: tensorfs::format::Encoding,
    dtype: tensorfs::format::Dtype,
    shape: Vec<u64>,
    parts: Vec<(tensorfs::format::Part, usize)>,
    storage: io::DeviceBuffer,
    scratch_bytes: usize,
}

fn decode_failure(error: tensorfs::Failure) -> Failure {
    match error.kind {
        // This stage performs GPU decoding only, never filesystem writes. The shared
        // decoder erases CUresult, so an I/O-family driver error cannot prove healthy
        // ownership even if a subsequent drain succeeds. Retain the same actual Work.
        tensorfs::Kind::Io => Failure::Native(varena::Error::Poisoned(error.detail)),
        tensorfs::Kind::Full => Failure::Native(varena::Error::OutOfMemory { requested_bytes: 0 }),
        _ => Failure::Storage(error),
    }
}

struct State {
    work: Option<Work>,
    started: bool,
    result: Option<Result<ReadStats, Failure>>,
}

struct Shared {
    region: Arc<varena::Region>,
    cancel: Arc<ReadControl>,
    state: Mutex<State>,
    changed: Condvar,
    pid: u32,
    continuation: Option<Arc<Continuation>>,
    began_ns: u64,
    read_issued_ns: AtomicU64,
    read_completed_ns: AtomicU64,
    read_bytes: AtomicU64,
    input_bytes: Vec<u64>,
    decode_completed_ns: AtomicU64,
    decode_issued_ns: AtomicU64,
    decode_checked_ns: AtomicU64,
    decoded_bytes: usize,
    scratch_bytes: usize,
    decode_scheme: Option<String>,
    launch_started_ns: AtomicU64,
    launch_submitted_ns: AtomicU64,
    compute_completed_ns: AtomicU64,
    compute_wait_entered_ns: AtomicU64,
    lookahead_triggered_clock_ns: AtomicU64,
    completion_proven: AtomicBool,
    event_ordered: bool,
}

impl Shared {
    fn elapsed(&self) -> u64 {
        monotonic_ns()
            .saturating_sub(self.began_ns)
            .saturating_add(1)
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
            if self.cancel.canceled() {
                return Err(Failure::Canceled);
            }
            let issued_result = Mutex::new(None);
            // Own every actual input across source, decoder and graph completion. Ready
            // inputs were pinned atomically during preparation; a missing input stays private.
            for input in &mut work.inputs {
                if input.pin.is_none() {
                    input.pin = Some(
                        input
                            .fill
                            .as_ref()
                            .expect("missing input fill")
                            .pin()
                            .map_err(Failure::Native)?,
                    );
                }
            }
            if work.event_ordered {
                if let Some(decode) = &work.decode {
                    let parts: Vec<_> = decode
                        .parts
                        .iter()
                        .map(|(part, offset)| tensorfs::raw::DecodePart {
                            role: &part.role,
                            dtype: &part.dtype,
                            shape: &part.shape,
                            src: io::Src::Device {
                                device,
                                ptr: decode.storage.ptr() + *offset as u64,
                                stream: 0,
                            },
                            len: part.length as usize,
                        })
                        .collect();
                    let fill = work.inputs[0].fill.as_ref().expect("fill");
                    // SAFETY: Work owns all packed spans and the exclusive final Fill. This
                    // prepares metadata only; the later callback supplies actual producer streams.
                    work.prepared_decode = Some(Mutex::new(
                        unsafe {
                            tensorfs::raw::prepare_device_decode(
                                &decode.encoding,
                                &decode.dtype,
                                &decode.shape,
                                &parts,
                                tensorfs::raw::DecodeOutput {
                                    dtype: &decode.dtype,
                                    shape: &decode.shape,
                                    dst: Dst::Device {
                                        device,
                                        ptr: fill.ptr(),
                                        len: fill.len(),
                                        stream: 0,
                                    },
                                },
                            )
                        }
                        .map_err(decode_failure)?,
                    ));
                }
            }
            let placements: Vec<_> = work
                .inputs
                .iter()
                .filter_map(|input| {
                    let fill = input.fill.as_ref()?;
                    let (ptr, len) = work
                        .decode
                        .as_ref()
                        .map_or((fill.ptr(), fill.len()), |decode| {
                            (decode.storage.ptr(), decode.storage.len())
                        });
                    Some(Placement {
                        items: &input.items,
                        origin: 0,
                        dst: Dst::Device {
                            device,
                            ptr,
                            len,
                            stream: 0,
                        },
                    })
                })
                .collect();
            // Every region is nonempty, so padding supplies at least one copy even when
            // no stored item has bytes. Source reports each placement exactly once.
            let remaining = AtomicUsize::new(placements.len());
            // Source's counters cover every coalesced copy and padding span per placement.
            // Checked mode keeps the historical observation-only callback.
            let issued = |_: usize, streams: &[usize]| {
                if remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
                    return;
                }
                self.read_issued_ns.store(self.elapsed(), Ordering::Release);
                if work.event_ordered {
                    let result = if self.cancel.canceled() {
                        Err(Failure::Canceled)
                    } else {
                        let launching = self.elapsed();
                        let result = (|| {
                            if let Some(decode) = &work.prepared_decode {
                                let mut decode = decode.lock().unwrap_or_else(|e| e.into_inner());
                                // SAFETY: source's last-issue callback names all actual copy
                                // streams. Work retains decoder, packed bytes and final Fill.
                                let (stream, error) = unsafe { decode.enqueue_after(streams) }
                                    .map_err(decode_failure)?;
                                self.decode_issued_ns
                                    .store(self.elapsed(), Ordering::Release);
                                work.continuation
                                    .as_ref()
                                    .expect("event continuation")
                                    .start_after(&[stream], error)
                                    .map_err(execution_failure)
                            } else {
                                work.continuation
                                    .as_ref()
                                    .expect("event continuation")
                                    .start_after(streams, 0)
                                    .map_err(execution_failure)
                            }
                        })();
                        if result.is_ok() {
                            self.launch_submitted_ns
                                .store(self.elapsed(), Ordering::Release);
                            self.launch_started_ns.store(launching, Ordering::Release);
                        }
                        result
                    };
                    *issued_result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            };
            let issued = (!placements.is_empty() && (work.decode.is_none() || work.event_ordered))
                .then_some(&issued as io::Issued<'_>);
            // Non-preferred weights are the streaming window. Populate the OS page cache
            // on their misses so later passes do not repeatedly bypass it with O_DIRECT.
            // A mixed placement read uses buffered I/O as one existing engine operation.
            let mode = if work
                .inputs
                .iter()
                .filter(|input| input.fill.is_some())
                .try_fold(false, |streamed, input| {
                    input
                        .region
                        .is_preferred()
                        .map(|preferred| streamed || !preferred)
                })
                .map_err(Failure::Native)?
            {
                Mode::Buffered
            } else {
                Mode::Cache
            };
            // SAFETY: Work owns the checkpoint/source/items and the Fill's physical pin.
            // The engine validates all spans, initializes padding and joins submitted work.
            let stats = unsafe {
                work.source.read_placements_cancelable_observed(
                    &placements,
                    work.threads,
                    mode,
                    &IoTally::default(),
                    &self.cancel,
                    issued,
                )
            }
            .map_err(io_failure)?;
            work.completed = true;
            self.read_bytes.store(stats.bytes, Ordering::Release);
            self.read_completed_ns
                .store(self.elapsed(), Ordering::Release);
            if work.event_ordered && placements.is_empty() {
                // Every input was atomically pinned Ready. No copy/decoder event is needed,
                // but the continuation still owns the caller-ready dependency and epoch.
                let launching = self.elapsed();
                work.continuation
                    .as_ref()
                    .expect("event continuation")
                    .start()
                    .map_err(execution_failure)?;
                self.launch_started_ns.store(launching, Ordering::Release);
                self.launch_submitted_ns
                    .store(self.elapsed(), Ordering::Release);
            } else if work.event_ordered {
                issued_result
                    .into_inner()
                    .unwrap_or_else(|e| e.into_inner())
                    .ok_or_else(|| {
                        Failure::Native(varena::Error::Poisoned("event input never issued".into()))
                    })??;
            }
            if self.cancel.canceled() {
                return Err(Failure::Canceled);
            }
            if let Some(decode) = work.decode.as_ref().filter(|_| !work.event_ordered) {
                let fill = work.inputs[0].fill.as_ref().expect("encoded input fill");
                // Read completion does not certify decoder stream/scratch completion.
                // Any decoder error or panic must take the existing checked-drain path.
                work.completed = false;
                let parts: Vec<_> = decode
                    .parts
                    .iter()
                    .map(|(part, offset)| tensorfs::raw::DecodePart {
                        role: &part.role,
                        dtype: &part.dtype,
                        shape: &part.shape,
                        src: io::Src::Device {
                            device,
                            ptr: decode.storage.ptr() + *offset as u64,
                            stream: 0,
                        },
                        len: part.length as usize,
                    })
                    .collect();
                // SAFETY: Work retains all packed buffers and the exclusive final Fill;
                // the read already joined all producers. Separate allocations cannot
                // overlap. The shared decoder checks metadata, streams and completion.
                unsafe {
                    tensorfs::raw::decode_into(
                        &decode.encoding,
                        &decode.dtype,
                        &decode.shape,
                        &parts,
                        tensorfs::raw::DecodeOutput {
                            dtype: &decode.dtype,
                            shape: &decode.shape,
                            dst: Dst::Device {
                                device,
                                ptr: fill.ptr(),
                                len: fill.len(),
                                stream: 0,
                            },
                        },
                    )
                }
                .map_err(decode_failure)?;
                work.completed = true;
                self.decode_completed_ns
                    .store(self.elapsed(), Ordering::Release);
                if self.cancel.canceled() {
                    return Err(Failure::Canceled);
                }
            }
            if !work.event_ordered {
                // Individual valid cache entries may publish before a later commit fails.
                // No invalid input is published; failure still yields no request output.
                work.publish_inputs()?;
            }
            if let Some(execution) = work.continuation.clone() {
                if !work.event_ordered {
                    let launching = self.elapsed();
                    execution.start().map_err(execution_failure)?;
                    self.launch_submitted_ns
                        .store(self.elapsed(), Ordering::Release);
                    self.launch_started_ns.store(launching, Ordering::Release);
                }
                // The current demand copy is checked and its graph is already running.
                // A bounded adjacent source may now read on the existing refill worker;
                // it never launches the next graph or calls Python.
                if let Some(lookahead) = work.lookahead.take() {
                    self.lookahead_triggered_clock_ns
                        .store(monotonic_ns(), Ordering::Release);
                    lookahead();
                }
                self.compute_wait_entered_ns
                    .store(self.elapsed(), Ordering::Release);
                let (status, progress) = execution.observe(true);
                if !execution.checked(status, progress) {
                    return Err(execution_failure(Status::unproven(&format!(
                        "native continuation did not prove completion: {}",
                        status.detail()
                    ))));
                }
                self.compute_completed_ns
                    .store(self.elapsed(), Ordering::Release);
                if let Some(decode) = &work.prepared_decode {
                    let checked = decode.lock().unwrap_or_else(|e| e.into_inner()).finish();
                    self.decode_checked_ns
                        .store(self.elapsed(), Ordering::Release);
                    checked.map_err(decode_failure)?;
                    self.decode_completed_ns
                        .store(self.elapsed(), Ordering::Release);
                }
                status.result().map_err(execution_failure)?;
                if progress.canceled != 0 {
                    return Err(Failure::Canceled);
                }
            }
            if work.event_ordered {
                work.release_private_gpu()?;
                // Source and dependent execution are checked complete. Only now may
                // ordinary readers observe new ready inputs, each under its existing region.
                work.publish_inputs()?;
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
            if let Some(decode) = &work.prepared_decode {
                let mut decode = decode.lock().unwrap_or_else(|e| e.into_inner());
                if decode.submitted() {
                    if let Err(error) = decode.finish() {
                        if error.kind != tensorfs::Kind::Decode {
                            let detail = format!("native decoder cleanup is unproven: {error}");
                            drop(decode);
                            self.region.poison(detail.clone());
                            std::mem::forget(work);
                            self.finish(Err(Failure::Native(varena::Error::Poisoned(detail))));
                            return;
                        }
                    }
                }
            }
            if work.event_ordered && self.launch_submitted_ns.load(Ordering::Acquire) != 0 {
                if let Err(Failure::Native(error)) = &result {
                    let detail =
                        format!("event-ordered input ownership failed after enqueue: {error}");
                    self.region.poison(detail.clone());
                    std::mem::forget(work);
                    self.finish(Err(Failure::Native(varena::Error::Poisoned(detail))));
                    return;
                }
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
            for input in &mut work.inputs {
                drop(input.fill.take());
            }
        }
        if let Err(error) = work.release_private_gpu() {
            let detail = format!("native refill GPU cleanup is unproven: {}", error.detail());
            self.region.poison(detail.clone());
            std::mem::forget(work);
            self.finish(Err(Failure::Native(varena::Error::Poisoned(detail))));
            return;
        }
        for input in &mut work.inputs {
            if let Some(pin) = input.pin.take() {
                if result.is_ok() {
                    // SAFETY: successful read/decode completion and, when composed, the
                    // checked continuation wait above joined every producer and consumer.
                    // Work submits nothing after this point. Exported capture/storage
                    // aliases own independent Pins and keep their conservative fence.
                    drop(unsafe { pin.into_completed_region() });
                } else {
                    // Keep the generic fence for failure paths even after a checked drain.
                    // A failed Fill or foreign alias is never certified by a sibling Pin.
                    drop(pin);
                }
            }
        }
        drop(work.producer.take());
        crate::cleanup::defer(work);
        self.completion_proven.store(true, Ordering::Release);
        self.finish(result);
    }
}

fn prepare_items(
    checkpoint: &tensorfs::Checkpoint,
    parts: Vec<(String, usize, usize)>,
    bytes: usize,
) -> Result<Vec<Item>, Failure> {
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
            .filter(|end| *end <= bytes)
            .ok_or(Failure::Native(varena::Error::Invalid(
                "refill target exceeds region",
            )))?;
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
                .ok_or(Failure::Native(varena::Error::Invalid(
                    "refill span overflow",
                )))?;
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
    // Keep validation above for named empty parts, but omit no-op copies: the
    // device engine issues no callback for them. Region padding still does.
    items.retain(|item| item.len != 0);
    Ok(items)
}

pub(super) fn prepare_encoded(
    arena: &Arc<varena::Arena>,
    checkpoint: tensorfs::Checkpoint,
    component: &str,
    key: &str,
    dtype: tensorfs::format::Dtype,
    max_scratch_bytes: usize,
    threads: usize,
) -> Result<(Arc<varena::Region>, Work), Failure> {
    use tensorfs::format::{Body, Dtype};
    let invalid = |detail| Failure::Native(varena::Error::Invalid(detail));
    if !matches!(dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) {
        return Err(invalid("encoded load output must be F16, BF16 or F32"));
    }
    let tensor = checkpoint
        .header()
        .tensor(component, key)
        .ok_or_else(|| invalid("encoded load tensor is absent"))?;
    let shape = tensor.shape.clone();
    let bytes = dtype
        .byte_len(&shape)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0 && *n <= isize::MAX as usize)
        .ok_or_else(|| invalid("encoded load output is empty or exceeds address space"))?;
    let Body::Encoded { encoding, parts } = &tensor.body else {
        return Err(invalid(
            "encoded load requires an encoded checkpoint tensor",
        ));
    };
    let lengths = parts
        .iter()
        .map(|part| {
            usize::try_from(part.length)
                .map_err(|_| invalid("encoded part length exceeds address space"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let metadata: Vec<_> = parts
        .iter()
        .zip(&lengths)
        .map(|(part, &len)| tensorfs::format::decode::PartMetadata {
            role: &part.role,
            dtype: &part.dtype,
            shape: &part.shape,
            len,
        })
        .collect();
    let private = tensorfs::raw::decode_device_scratch_bytes(
        arena.device(),
        encoding,
        &dtype,
        &shape,
        &metadata,
    )
    .map_err(|error| match error.kind {
        tensorfs::Kind::Full => Failure::Native(varena::Error::OutOfMemory { requested_bytes: 0 }),
        _ => Failure::Storage(error), // Preflight has submitted no GPU work.
    })?;
    let (offsets, packed) = io::pack(lengths.iter().copied(), io::DEVICE_ALIGN)
        .ok_or_else(|| invalid("encoded load packed size overflows"))?;
    if packed == 0 || packed > isize::MAX as usize {
        return Err(invalid(
            "encoded load packed storage is empty or exceeds address space",
        ));
    }
    let required = packed
        .checked_add(private)
        .ok_or_else(|| invalid("encoded load scratch size overflows"))?;
    if required > max_scratch_bytes {
        return Err(Failure::Native(varena::Error::OutOfMemory {
            requested_bytes: required,
        }));
    }
    let named = parts
        .iter()
        .zip(&offsets)
        .zip(lengths)
        .map(|((part, &offset), len)| (format!("{component}/{key}#{}", part.role), offset, len))
        .collect();
    let items = prepare_items(&checkpoint, named, packed)?;
    let encoding = encoding.clone();
    let parts = parts.iter().cloned().zip(offsets).collect();
    let region = arena.reserve(bytes).map_err(Failure::Native)?;
    let mut work = prepare_work(&region, checkpoint, Vec::new(), threads, true, false, true)?
        .ok_or_else(|| invalid("fresh encoded load slot was already resident"))?;
    // Final zero initialization has already completed. This fresh scratch has no
    // pending GPU work if allocation fails; later submission keeps it inside Work.
    let scratch_producer = varena::enter_producer(region.device()).map_err(Failure::Native)?;
    let storage = io::DeviceBuffer::new(region.device(), packed).map_err(cuda_failure)?;
    drop(scratch_producer);
    work.inputs[0].items = items;
    work.decode = Some(Decode {
        encoding,
        dtype,
        shape,
        parts,
        storage,
        scratch_bytes: required,
    });
    Ok((region, work))
}

pub(super) fn prepare_work(
    region: &Arc<varena::Region>,
    checkpoint: tensorfs::Checkpoint,
    parts: Vec<(String, usize, usize)>,
    threads: usize,
    reclaim: bool,
    evict_completed: bool,
    initialize: bool,
) -> Result<Option<Work>, Failure> {
    let items = prepare_items(&checkpoint, parts, region.len())?;
    let producer = varena::enter_producer(region.device()).map_err(Failure::Native)?;
    let preparation = if reclaim {
        region.prepare()
    } else if evict_completed {
        region.prepare_evicting_completed()
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
        inputs: vec![Input {
            region: region.clone(),
            items,
            fill: Some(fill),
            pin: None,
        }],
        continuation: None,
        _checkpoint: checkpoint,
        source,
        producer: Some(producer),
        threads: threads.clamp(1, 8),
        completed: false,
        decode: None,
        event_ordered: false,
        prepared_decode: None,
        lookahead: None,
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
                        ptr: work.inputs[0].fill.as_ref().expect("fill").ptr(),
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

/// Prepare one component's existing raw block regions without changing their identities.
/// Ready inputs remain pinned before any cold input may allocate or reclaim backing.
/// Unless keep_ready is requested for stage capture, all-ready returns None and the caller
/// must reacquire/revalidate ordinary resident bindings.
pub(super) fn prepare_regions(
    checkpoint: tensorfs::Checkpoint,
    groups: Vec<(Arc<varena::Region>, Vec<(String, usize, usize)>)>,
    threads: usize,
    keep_ready: bool,
) -> Result<Option<Work>, Failure> {
    let invalid = |detail| Failure::Native(varena::Error::Invalid(detail));
    let Some((first, _)) = groups.first() else {
        return Err(invalid("load needs input regions"));
    };
    let device = first.device();
    let mut seen = std::collections::BTreeSet::new();
    // Validate every descriptor before allocation. Same-device reservations cannot overlap;
    // repeating a region would otherwise turn its second preparation into a busy writer.
    let mut inputs = groups
        .into_iter()
        .map(|(region, parts)| {
            if region.device() != device || !seen.insert(region.ptr()) {
                return Err(invalid("load regions must be distinct and on one device"));
            }
            let items = prepare_items(&checkpoint, parts, region.len())?;
            Ok(Input {
                region,
                items,
                fill: None,
                pin: None,
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    for input in &mut inputs {
        input.pin = input.region.try_pin().map_err(Failure::Native)?;
    }
    if !keep_ready && inputs.iter().all(|input| input.pin.is_some()) {
        return Ok(None);
    }
    let producer = varena::enter_producer(device).map_err(Failure::Native)?;
    for input in &mut inputs {
        if input.pin.is_none() {
            match input.region.prepare().map_err(Failure::Native)? {
                varena::Preparation::Resident(pin) => input.pin = Some(pin),
                varena::Preparation::Fill(fill) => input.fill = Some(fill),
            }
        }
    }
    if !keep_ready && inputs.iter().all(|input| input.fill.is_none()) {
        return Ok(None);
    }
    io::prepare_device(device).map_err(cuda_failure)?;
    let source = Arc::new(checkpoint.local_source(true));
    // Every stored chunk these inputs read must already be on local disk. Verify it through
    // the same local-only load path the worker uses: a missing chunk errors here, before
    // submission, and the opened descriptors are cached for the worker to reuse. Refuse as
    // not-local so the request falls back to the eager, fetch-capable reader (which downloads)
    // instead of failing a submitted read that SDXL will not retry. Remote I/O stays out of
    // the admitted native interval.
    let objects: Vec<_> = inputs
        .iter()
        .flat_map(|input| &input.items)
        .filter_map(|item| match &item.source {
            io::ItemSource::Object(range) => Some((range.obj.sha256.clone(), range.obj.clone())),
            io::ItemSource::Inline(_) => None,
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    if let Err(error) = source.verify_local(&objects) {
        return Err(Failure::NotLocal(format!(
            "native component inputs are not all local: {error}"
        )));
    }
    let mut work = Work {
        region: inputs[0].region.clone(),
        inputs,
        continuation: None,
        _checkpoint: checkpoint,
        source,
        producer: Some(producer),
        threads: threads.clamp(1, 8),
        completed: false,
        decode: None,
        event_ordered: false,
        prepared_decode: None,
        lookahead: None,
    };
    let copies: Vec<_> = work
        .inputs
        .iter()
        .filter_map(|input| {
            input.fill.as_ref().map(|fill| io::Copy {
                src: io::Src::Zero,
                dst: Dst::Device {
                    device,
                    ptr: fill.ptr(),
                    len: fill.len(),
                    stream: 0,
                },
            })
        })
        .collect();
    // SAFETY: each nonoverlapping Fill is exclusive; ready inputs are not destinations.
    // Initialization completes before any capture-only alias can be exported.
    let initialized = unsafe {
        io::read(
            &copies,
            &io::ReadOpts {
                threads: 1,
                ..Default::default()
            },
        )
    };
    if let Err(error) = initialized {
        if let Err(drain) = io::drain(device) {
            let detail = format!("capture input initialization is unproven: {drain}");
            work.region.poison(detail.clone());
            std::mem::forget(work);
            return Err(Failure::Native(varena::Error::Poisoned(detail)));
        }
        return Err(io_failure(tensorfs::io::source::Error::Read(error)));
    }
    drop(work.producer.take()); // Actual submission reacquires producer exclusion after capture.
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
    pub(super) fn missing_bytes(&self) -> u64 {
        self.inputs
            .iter()
            .filter(|input| input.fill.is_some())
            .flat_map(|input| &input.items)
            .map(|item| item.len)
            .sum()
    }

    pub(super) fn set_lookahead(&mut self, call: Arc<dyn Fn() + Send + Sync>) {
        self.lookahead = Some(call);
    }

    pub(super) fn detach_capture(mut self) -> Result<StageSource, Failure> {
        if self.continuation.is_some() || self.decode.is_some() || self.prepared_decode.is_some() {
            self.abandon();
            return Err(Failure::Ineligible(
                "only an unused raw capture may detach".into(),
            ));
        }
        // Initialization was checked before exporting capture aliases. Captured Torch work
        // is not assumed complete here: ordinary releases preserve the context fence.
        for input in &mut self.inputs {
            drop(input.fill.take());
            drop(input.pin.take());
        }
        drop(self.producer.take());
        for input in &self.inputs {
            if input.region.has_live_pins().map_err(Failure::Native)? {
                return Err(Failure::Ineligible(
                    "capture input storage escaped; streamed stage must remain resident".into(),
                ));
            }
        }
        Ok(StageSource {
            inputs: self
                .inputs
                .into_iter()
                .map(|input| (input.region, input.items))
                .collect(),
            checkpoint: self._checkpoint,
            source: self.source,
            threads: self.threads,
        })
    }

    /// Private stream/buffer destructors call CUDA. Keep them inside producer exclusion,
    /// including when a slot never submitted and initialization already released its lease.
    fn release_private_gpu(&mut self) -> Result<(), Failure> {
        if self.prepared_decode.is_none() && self.decode.is_none() {
            return Ok(());
        }
        if self.producer.is_none() {
            self.producer =
                Some(varena::enter_producer(self.region.device()).map_err(Failure::Native)?);
        }
        if let Some(prepared) = &mut self.prepared_decode {
            let prepared = prepared.get_mut().unwrap_or_else(|e| e.into_inner());
            // SAFETY: normal completion or checked abort proved every GPU reader done;
            // the producer guard above excludes actual capture through these releases.
            unsafe { prepared.release_completed() }.map_err(decode_failure)?;
        }
        if let Some(decode) = &mut self.decode {
            // SAFETY: the packed input's decoder and dependent consumer are checked complete,
            // or this slot never submitted any reads; producer exclusion is still held.
            unsafe { decode.storage.release_completed() }.map_err(cuda_failure)?;
        }
        drop(self.prepared_decode.take());
        drop(self.decode.take());
        Ok(())
    }

    fn publish_inputs(&mut self) -> Result<(), Failure> {
        for input in &mut self.inputs {
            if let Some(fill) = input.fill.take() {
                // SAFETY: caller checked the whole source and, for events, its consumer.
                // The independent input pin remains live if publication reports an error.
                let published = unsafe { fill.commit_completed() }.map_err(Failure::Native)?;
                if let Some(previous) = input.pin.replace(published) {
                    // SAFETY: this independent source pin has no outstanding reader here:
                    // checked ordering has not launched the consumer; event ordering joins
                    // the consumer before publication. Both joined source/decode streams.
                    // Capture aliases own separate Pins and are not released by this call.
                    drop(unsafe { previous.into_completed_region() });
                }
            }
        }
        Ok(())
    }

    pub(super) fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub(super) fn capture_pin(&self) -> Result<varena::Pin, Failure> {
        self.capture_input(0)
    }

    pub(super) fn capture_input(&self, index: usize) -> Result<varena::Pin, Failure> {
        let input = self
            .inputs
            .get(index)
            .ok_or(Failure::Native(varena::Error::Invalid(
                "load input index is out of range",
            )))?;
        match (&input.fill, &input.pin) {
            (Some(fill), _) => fill.pin().map_err(Failure::Native),
            (None, Some(pin)) => pin.try_clone().map_err(Failure::Native),
            _ => Err(Failure::Native(varena::Error::Invalid(
                "load input has no owned storage",
            ))),
        }
    }
    pub(super) fn abandon(mut self) {
        if self.continuation.is_some() || self.decode.is_some() || self.prepared_decode.is_some() {
            crate::cleanup::defer(Abandoned(Some(self)));
        } else {
            for input in &mut self.inputs {
                drop(input.fill.take());
                drop(input.pin.take());
            }
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
            match work.release_private_gpu() {
                Ok(()) => {}
                Err(Failure::Native(varena::Error::CaptureActive)) => {
                    // Capture must not wait for this destructor. Return to the same queue
                    // so other releases can proceed; no state/arena/Python lock is held.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    crate::cleanup::defer(Abandoned(Some(work)));
                    return;
                }
                Err(error) => {
                    work.region.poison(format!(
                        "abandoned GPU cleanup is unproven: {}",
                        error.detail()
                    ));
                    std::mem::forget(work);
                    return;
                }
            }
            for input in &mut work.inputs {
                drop(input.fill.take());
            }
            for input in &mut work.inputs {
                drop(input.pin.take());
            }
            drop(work.producer.take());
        }
    }
}

impl PreparedRead {
    /// Run the existing producer on an already-owned native sequence worker. No Python,
    /// new scheduler or additional refill thread participates in this path.
    pub(super) fn completed_read_bytes(&self) -> (u64, Vec<u64>, bool) {
        let shared = self.shared.as_ref().expect("live native read");
        let done = shared.read_completed_ns.load(Ordering::Acquire) != 0;
        (
            shared.read_bytes.load(Ordering::Acquire),
            shared
                .input_bytes
                .iter()
                .map(|bytes| if done { *bytes } else { 0 })
                .collect(),
            done,
        )
    }

    pub(super) fn run_inline(&self) -> (Result<ReadStats, Failure>, bool) {
        let shared = self.shared.as_ref().expect("live native read").clone();
        let work = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            assert!(!state.started, "native read is one-shot");
            state.started = true;
            state.work.take().expect("inline composed read owns work")
        };
        shared.clone().run(work);
        let result = shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .result
            .clone()
            .expect("inline run publishes completion");
        (result, shared.completion_proven.load(Ordering::Acquire))
    }

    pub(super) fn start_native(&self) -> Result<(), Failure> {
        let shared = self.shared.as_ref().expect("live native read").clone();
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.started {
            return Err(Failure::Native(varena::Error::Invalid(
                "native refill already started",
            )));
        }
        state.started = true;
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
        if spawned.is_err() {
            // No worker exists. Check cleanup synchronously without issuing speculative I/O.
            shared.cancel.cancel();
            let work = state.work.take().expect("failed start owns work");
            drop(state);
            shared.run(work);
            return Err(Failure::Ineligible(
                "native prefetch worker could not start".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn read_clock(&self) -> (u64, u64) {
        let shared = self.shared.as_ref().expect("live native read");
        let absolute = |relative: u64| {
            if relative == 0 {
                0
            } else {
                shared.began_ns.saturating_add(relative - 1)
            }
        };
        (
            absolute(shared.read_issued_ns.load(Ordering::Acquire)),
            absolute(shared.read_completed_ns.load(Ordering::Acquire)),
        )
    }

    pub(super) fn join_native(&self) -> (Result<ReadStats, Failure>, bool) {
        let shared = self.shared.as_ref().expect("live native read");
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.started && state.result.is_none() {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        let result = state.result.clone().unwrap_or_else(|| {
            Err(Failure::Native(varena::Error::Invalid(
                "prefetch was never started",
            )))
        });
        (result, shared.completion_proven.load(Ordering::Acquire))
    }

    pub(super) fn control(&self) -> Control {
        Control(Arc::downgrade(
            self.shared.as_ref().expect("live prepared read"),
        ))
    }
    pub(super) fn request_cancel(&self) {
        self.shared.as_ref().expect("live prepared read").cancel();
    }
    pub(super) fn composed(
        work: Work,
        execution: Arc<Continuation>,
        event_ordered: bool,
    ) -> Result<Self, Failure> {
        Self::composed_with_control(
            work,
            execution,
            event_ordered,
            Arc::new(ReadControl::default()),
        )
    }

    pub(super) fn composed_with_control(
        work: Work,
        execution: Arc<Continuation>,
        event_ordered: bool,
        cancel: Arc<ReadControl>,
    ) -> Result<Self, Failure> {
        Self::from_work(work, Some(execution), event_ordered, cancel)
    }

    pub(super) fn read_only(work: Work, cancel: Arc<ReadControl>) -> Result<Self, Failure> {
        Self::from_work(work, None, false, cancel)
    }

    fn from_work(
        mut work: Work,
        execution: Option<Arc<Continuation>>,
        event_ordered: bool,
        cancel: Arc<ReadControl>,
    ) -> Result<Self, Failure> {
        work.continuation = execution.clone();
        work.event_ordered = event_ordered;
        let producer = match varena::enter_producer(work.region.device()) {
            Ok(producer) => producer,
            Err(error) => {
                work.abandon();
                return Err(Failure::Native(error));
            }
        };
        work.producer = Some(producer);
        let (decoded_bytes, scratch_bytes, decode_scheme) =
            work.decode.as_ref().map_or((0, 0, None), |decode| {
                (
                    work.region.len(),
                    decode.scratch_bytes,
                    Some(decode.encoding.scheme.clone()),
                )
            });
        let input_bytes = work
            .inputs
            .iter()
            .map(|input| {
                if input.fill.is_some() {
                    input.items.iter().map(|item| item.len).sum()
                } else {
                    0
                }
            })
            .collect();
        Ok(Self {
            shared: Some(Arc::new(Shared {
                region: work.region.clone(),
                cancel,
                state: Mutex::new(State {
                    work: Some(work),
                    started: false,
                    result: None,
                }),
                changed: Condvar::new(),
                pid: std::process::id(),
                continuation: execution,
                began_ns: monotonic_ns(),
                read_issued_ns: AtomicU64::new(0),
                read_completed_ns: AtomicU64::new(0),
                read_bytes: AtomicU64::new(0),
                input_bytes,
                decode_completed_ns: AtomicU64::new(0),
                decode_issued_ns: AtomicU64::new(0),
                decode_checked_ns: AtomicU64::new(0),
                decoded_bytes,
                scratch_bytes,
                decode_scheme,
                launch_started_ns: AtomicU64::new(0),
                launch_submitted_ns: AtomicU64::new(0),
                compute_completed_ns: AtomicU64::new(0),
                compute_wait_entered_ns: AtomicU64::new(0),
                lookahead_triggered_clock_ns: AtomicU64::new(0),
                completion_proven: AtomicBool::new(false),
                event_ordered,
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
            // The original terminal source error remains observable even after the
            // checked execution arm has been closed by Runtime.
            if let Some(Err(error)) = result.as_ref() {
                if !matches!(error, Failure::Canceled) {
                    return Err(error.clone());
                }
            }
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
            // Provider cancel can settle its armed epoch while a copy is still draining.
            // Only the whole worker's result is the composed completion certificate.
            Ok((progress.completed_steps, canceled, result.is_some()))
        })
        .map_err(|error| error.raise(py))
    }
    pub(super) fn facts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let shared = self.shared()?;
        let row = PyDict::new(py);
        row.set_item("clock_origin_ns", shared.began_ns)?;
        row.set_item(
            "clock_origin_scope",
            "process-local TensorFS monotonic host observation clock; not GPU event time",
        )?;
        row.set_item(
            "lookahead_triggered_clock_ns",
            shared.lookahead_triggered_clock_ns.load(Ordering::Acquire),
        )?;
        let result = py.detach(|| {
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .result
                .clone()
        });
        row.set_item("issued_bytes", shared.cancel.issued_bytes())?;
        row.set_item(
            "ordering",
            if shared.event_ordered {
                "events"
            } else {
                "checked"
            },
        )?;
        row.set_item(
            "execution_committed",
            shared.launch_submitted_ns.load(Ordering::Acquire) != 0,
        )?;
        row.set_item(
            "failure_boundary",
            match &result {
                Some(Err(Failure::Storage(error))) if error.kind == tensorfs::Kind::Decode => {
                    Some("input_value")
                }
                Some(Err(_)) if shared.launch_submitted_ns.load(Ordering::Acquire) != 0 => {
                    Some("after_enqueue")
                }
                Some(Err(_)) => Some("before_enqueue"),
                _ => None,
            },
        )?;
        row.set_item("cancel_requested", shared.cancel.canceled())?;
        let bytes = shared.read_bytes.load(Ordering::Acquire);
        row.set_item("bytes", bytes)?;
        row.set_item("read_bytes", bytes)?;
        row.set_item(
            "input_read_bytes",
            shared
                .input_bytes
                .iter()
                .map(|bytes| {
                    if shared.read_completed_ns.load(Ordering::Acquire) == 0 {
                        0
                    } else {
                        *bytes
                    }
                })
                .collect::<Vec<_>>(),
        )?;
        row.set_item(
            "decoded_bytes",
            if shared.decode_completed_ns.load(Ordering::Acquire) == 0 {
                0
            } else {
                shared.decoded_bytes
            },
        )?;
        row.set_item("scratch_bytes", shared.scratch_bytes)?;
        row.set_item("decode_scheme", shared.decode_scheme.as_deref())?;
        row.set_item(
            "canceled",
            matches!(result, Some(Err(Failure::Canceled)))
                || result.is_none() && shared.cancel.canceled(),
        )?;
        for (name, clock) in [
            ("read_issued_ns", &shared.read_issued_ns),
            ("read_completed_ns", &shared.read_completed_ns),
            ("decode_completed_ns", &shared.decode_completed_ns),
            ("decode_issued_ns", &shared.decode_issued_ns),
            ("decode_checked_ns", &shared.decode_checked_ns),
            ("launch_started_ns", &shared.launch_started_ns),
            ("launch_submitted_ns", &shared.launch_submitted_ns),
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
        row.set_item(
            "failed",
            matches!(result, Some(Err(error)) if !matches!(error, Failure::Canceled)),
        )?;
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
    #[pyo3(signature = (region, checkpoint, parts, threads = 2, reclaim = true, evict_completed = false))]
    fn new(
        py: Python<'_>,
        region: PyRef<'_, PyRegion>,
        checkpoint: PyRef<'_, PyCheckpoint>,
        parts: Vec<(String, usize, usize)>,
        threads: usize,
        reclaim: bool,
        evict_completed: bool,
    ) -> PyResult<Self> {
        let region = region.region.clone();
        let checkpoint = checkpoint.core(py)?;
        let prepared = py
            .detach(|| {
                prepare_work(
                    &region,
                    checkpoint,
                    parts,
                    threads,
                    reclaim,
                    evict_completed,
                    false,
                )
            })
            .map_err(|e| e.raise(py))?;
        let cached = prepared.is_none();
        let input_bytes = vec![prepared.as_ref().map_or(0, |work| {
            work.inputs[0].items.iter().map(|item| item.len).sum()
        })];
        Ok(Self {
            shared: Some(Arc::new(Shared {
                region,
                cancel: Arc::new(ReadControl::default()),
                state: Mutex::new(State {
                    work: prepared,
                    started: false,
                    result: cached.then_some(Ok(ReadStats::default())),
                }),
                changed: Condvar::new(),
                pid: std::process::id(),
                continuation: None,
                began_ns: monotonic_ns(),
                read_issued_ns: AtomicU64::new(0),
                read_completed_ns: AtomicU64::new(0),
                read_bytes: AtomicU64::new(0),
                input_bytes,
                decode_completed_ns: AtomicU64::new(0),
                decode_issued_ns: AtomicU64::new(0),
                decode_checked_ns: AtomicU64::new(0),
                decoded_bytes: 0,
                scratch_bytes: 0,
                decode_scheme: None,
                launch_started_ns: AtomicU64::new(0),
                launch_submitted_ns: AtomicU64::new(0),
                compute_completed_ns: AtomicU64::new(0),
                compute_wait_entered_ns: AtomicU64::new(0),
                lookahead_triggered_clock_ns: AtomicU64::new(0),
                completion_proven: AtomicBool::new(false),
                event_ordered: false,
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
                let work = state.work.take();
                drop(state);
                if let Some(work) = work {
                    work.abandon();
                }
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
                let work = state.work.take();
                if let Some(work) = work {
                    work.abandon();
                }
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
                // A completed pass must not cancel a rearmed continuation using the same
                // immutable source. Unproven/in-flight work still receives cancellation.
                if !shared.completion_proven.load(Ordering::Acquire) {
                    shared.cancel();
                }
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
