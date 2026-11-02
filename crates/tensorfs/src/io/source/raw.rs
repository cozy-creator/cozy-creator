//! One owned raw Source operation, fixed engine streams and checked completion.
use super::*;
use crate::io::{cuda, device::OwnedRead};
use std::time::Instant;

pub struct RawFill {
    pub fill: varena::Fill,
    pub items: Vec<Item>,
    pub origin: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RawReadFacts {
    /// Fixed actual producer identities, borrowed only while this operation retains its
    /// native resources. They convey no ownership or right to extend Source submission.
    /// Do not use them after resources_retired, or submit after the operation seals.
    pub producer_streams: Vec<u64>,
    pub submission_closed: bool,
    pub published_regions: usize,
    pub regions: usize,
    pub completed: bool,
    pub resources_retired: bool,
    /// Every submitted Fill ticket has released its producer exclusion and native owners.
    /// Prepared no-write cleanup is separate; this grants no physical-capacity credit.
    pub submitted_fills_retired: bool,
    pub requested_bytes: u64,
    /// Bytes whose device enqueue succeeded, including padding. A failing driver call
    /// may still be ambiguous; zero is never a no-work/completion certificate.
    pub issued_bytes: u64,
    pub submission_ns: u64,
    pub stats: Option<ReadStats>,
    pub failure: Option<String>,
    pub retirement_failure: Option<String>,
}

struct Resources {
    read: OwnedRead,
    plan: ReadPlan,
    _source: Arc<Source>,
    metrics_recorded: bool,
}
impl Resources {
    fn record_completion(&mut self, stats: ReadStats) {
        if self.metrics_recorded {
            return;
        }
        self.metrics_recorded = true;
        #[cfg(unix)]
        if stats.staged != 0
            || stats.cached != 0
            || stats.direct != 0
            || self.read.issued_bytes() != 0
        {
            let copies = self
                .plan
                .copies
                .iter()
                .map(OwnedCopy::borrowed)
                .collect::<Vec<_>>();
            let files = copies
                .iter()
                .filter_map(|copy| match copy.src {
                    Src::File(file, _) => Some(file),
                    _ => None,
                })
                .collect::<Vec<_>>();
            crate::io::metrics::disk_run(
                &files,
                stats.cached + stats.direct,
                stats.ns,
                stats.direct,
                stats.cached,
            );
            // This is one measured mixed raw operation, including padding. Do not label
            // its host submission time completed H2D or use memset bytes to train H2D rate.
            crate::io::metrics::record(crate::io::metrics::Copy {
                kind: "raw",
                gpu: self.read.device(),
                bytes: self.read.issued_bytes(),
                ns: stats.ns,
                busy_ns: stats.busy_ns,
                pin_ns: stats.pin_ns,
                staged: stats.staged,
                cached: stats.cached,
                ..Default::default()
            });
        }
    }
}

struct Shared {
    pid: u32,
    owner: Mutex<Option<Resources>>,
    facts: Mutex<RawReadFacts>,
    failed: Mutex<Option<cuda::Error>>,
}

fn cuda_error(error: cuda::Error) -> Error {
    Error::Read(super::super::Error {
        copy: 0,
        source: std::io::Error::other(error),
    })
}
fn native_error(error: varena::Error) -> Error {
    cuda_error(cuda::Error::Allocation(error))
}
fn canceled() -> Error {
    Error::Read(super::super::Error {
        copy: 0,
        source: std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "native raw Source canceled",
        ),
    })
}
fn native(error: cuda::Error) -> varena::Error {
    match error {
        cuda::Error::Allocation(error) => error,
        cuda::Error::Cuda { op, code, .. } => varena::Error::Cuda {
            operation: op,
            code,
        },
        cuda::Error::CudaUnavailable(detail) => varena::Error::Poisoned(detail),
    }
}

/// Exclusive real resources leave the metadata mutex before any CUDA query/release.
/// Unwinding restores the exact owner instead of dropping GPU resources on that stack.
struct Claim<'a> {
    shared: &'a Shared,
    owner: Option<Resources>,
}
impl Claim<'_> {
    fn get(&mut self) -> &mut Resources {
        self.owner.as_mut().expect("exclusive read owner")
    }
}
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            *self
                .shared
                .owner
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(owner);
        }
    }
}
impl Shared {
    fn check(&self) -> cuda::Result<()> {
        if self.pid != std::process::id() {
            return Err(cuda::Error::CudaUnavailable(
                "raw Source inherited after fork".into(),
            ));
        }
        if let Some(error) = &*self
            .failed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
        {
            return Err(error.clone());
        }
        Ok(())
    }
    fn claim(&self) -> cuda::Result<Option<Claim<'_>>> {
        self.check()?;
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        Ok(owner.map(|owner| Claim {
            shared: self,
            owner: Some(owner),
        }))
    }
    fn fail(&self, error: cuda::Error) {
        *self
            .failed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(error.clone());
        self.facts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retirement_failure = Some(error.to_string());
    }
    fn close_unsubmitted(&self) -> cuda::Result<()> {
        let mut claim = self.claim()?.ok_or_else(|| {
            cuda::Error::CudaUnavailable(
                "raw Source preparation still has an active submitter".into(),
            )
        })?;
        claim.get().read.close_unsubmitted();
        self.facts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .submission_closed = true;
        Ok(())
    }
    fn record_stats(&self, stats: ReadStats) {
        let mut facts = self.facts.lock().unwrap_or_else(|error| error.into_inner());
        facts.stats = Some(stats);
    }
    fn poll(&self) -> cuda::Result<Option<ReadStats>> {
        self.check()?;
        if let Some(stats) = self
            .facts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stats
        {
            return Ok(Some(stats));
        }
        let Some(mut claim) = self.claim()? else {
            return Ok(None);
        };
        match claim.get().read.poll() {
            Ok(stats) => {
                if let Some(stats) = stats {
                    claim.get().record_completion(stats);
                    self.record_stats(stats);
                }
                Ok(stats)
            }
            Err(error) => {
                self.fail(error.clone());
                Err(error)
            }
        }
    }
    fn retire(&self) -> cuda::Result<bool> {
        self.check()?;
        if self
            .facts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .resources_retired
        {
            return Ok(true);
        }
        let Some(mut claim) = self.claim()? else {
            return Ok(false);
        };
        let result: cuda::Result<bool> = (|| {
            let resources = claim.get();
            let Some(stats) = resources.read.poll()? else {
                return Ok(false);
            };
            resources.record_completion(stats);
            self.record_stats(stats);
            if !resources.read.retire()? {
                return Ok(false);
            }
            // All native resources are inert/returned to their owning engine pool. Source
            // files/checkpoint context drop here, outside every metadata/GIL lock.
            drop(claim.owner.take());
            self.facts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .resources_retired = true;
            Ok(true)
        })();
        if let Err(error) = &result {
            self.fail(error.clone());
        }
        result
    }
}

struct Resource(Arc<Shared>);
// SAFETY: the native read owns all source/staging/engine resources. Its own host-closed
// state and recorded end-event set gate checked retirement, even for a Prepared peer Fill.
unsafe impl varena::FillResource for Resource {
    fn retire(&mut self) -> varena::Result<varena::FillRetirement> {
        self.0
            .retire()
            .map(|complete| {
                if complete {
                    varena::FillRetirement::Complete
                } else {
                    varena::FillRetirement::Pending
                }
            })
            .map_err(native)
    }
}

pub struct RawRead {
    shared: Arc<Shared>,
    sessions: Vec<varena::FillSession>,
    tickets: Vec<varena::FillTicket>,
    options: ReadOpts,
    started: bool,
    closed: bool,
}

impl Source {
    /// Prepare a raw one-device read and return untouched Fill owners on refusal.
    ///
    /// # Safety
    /// Fills have no earlier unaccounted writer. No caller submits through their pointers
    /// afterward. Every escaped destination view retains a separate native Pin.
    pub unsafe fn prepare_raw(
        self: &Arc<Self>,
        inputs: Vec<RawFill>,
        threads: usize,
        mode: Mode,
    ) -> std::result::Result<RawRead, (Vec<RawFill>, Error)> {
        let prepared = (|| {
            let device = inputs
                .first()
                .ok_or_else(|| invalid("raw Source needs at least one Fill"))?
                .fill
                .device();
            if inputs.iter().any(|input| input.fill.device() != device) {
                return Err(invalid("raw Source Fills must share one GPU"));
            }
            // Refuse capture before initializing streams/staging or any other driver call.
            let producer = varena::enter_producer(device).map_err(native_error)?;
            let options = ReadOpts {
                threads,
                direct: self.direct_io && mode == Mode::Cache,
                ..ReadOpts::default()
            };
            let read = OwnedRead::prepare(device, options.streams).map_err(cuda_error)?;
            let streams = read.streams();
            let placements = inputs
                .iter()
                .map(|input| Placement {
                    items: &input.items,
                    origin: input.origin,
                    dst: Dst::Device {
                        device,
                        ptr: input.fill.ptr(),
                        len: input.fill.len(),
                        stream: streams[0] as usize,
                    },
                })
                .collect::<Vec<_>>();
            let plan = self.plan_placements(&placements)?;
            let shared = Arc::new(Shared {
                pid: std::process::id(),
                facts: Mutex::new(RawReadFacts {
                    producer_streams: streams.clone(),
                    regions: inputs.len(),
                    requested_bytes: plan.bytes,
                    ..RawReadFacts::default()
                }),
                owner: Mutex::new(Some(Resources {
                    read,
                    plan,
                    _source: Arc::clone(self),
                    metrics_recorded: false,
                })),
                failed: Mutex::new(None),
            });
            Ok((producer, options, streams, shared))
        })();
        let (producer, options, streams, shared) = match prepared {
            Ok(value) => value,
            Err(error) => return Err((inputs, error)),
        };
        let sessions = inputs
            .into_iter()
            .map(|input| unsafe {
                input.fill.into_async(varena::FillOwners::new(
                    streams.clone(),
                    vec![Box::new(Resource(Arc::clone(&shared)))],
                ))
            })
            .collect();
        drop(producer); // No destination exposed; each begin acquires its own producer lease.
        Ok(RawRead {
            shared,
            sessions,
            tickets: Vec::new(),
            options,
            started: false,
            closed: false,
        })
    }
}

impl RawRead {
    /// Close every host submitter before returning. Only wholly initialized Regions publish.
    /// Publication across Regions is not atomic: an earlier valid Fill can publish before
    /// a later seal fails. That failure retains ownership and never makes partial bytes Ready.
    pub fn submit(&mut self, cancel: &ReadControl) -> Result<()> {
        self.shared.check().map_err(cuda_error)?;
        if self.started {
            return Err(invalid("raw Source operation was already submitted"));
        }
        self.started = true;
        let began = Instant::now();
        let result = (|| {
            if cancel.canceled() {
                return Err(canceled());
            }
            for session in &mut self.sessions {
                session.begin().map_err(native_error)?;
            }
            let mut claim = self
                .shared
                .claim()
                .map_err(cuda_error)?
                .ok_or_else(|| invalid("raw Source owner is already in use"))?;
            let resources = claim.get();
            let (copies, _) = super::super::coalesce(
                resources
                    .plan
                    .copies
                    .iter()
                    .map(OwnedCopy::borrowed)
                    .collect(),
            );
            let result = unsafe { resources.read.submit(&copies, &self.options, cancel) }
                .map_err(Error::Read);
            self.closed = resources.read.closed();
            self.shared
                .facts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .issued_bytes = resources.read.issued_bytes();
            if result.is_ok() && !cancel.canceled() {
                resources.read.logical_bytes(resources.plan.bytes);
            }
            result?;
            if cancel.canceled() {
                return Err(canceled());
            }
            Ok(())
        })();
        if !self.closed {
            // Begin/validation/cancellation refused before the engine accepted submission.
            self.shared.close_unsubmitted().map_err(cuda_error)?;
            self.closed = true;
        }
        {
            let mut facts = self
                .shared
                .facts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            facts.submission_closed = true;
            facts.submission_ns = began.elapsed().as_nanos() as u64;
            facts.failure = result.as_ref().err().map(ToString::to_string);
        }
        let mut seal_error = None;
        for session in &mut self.sessions {
            if session.facts().map_err(native_error)?.state != varena::FillState::Writing {
                continue;
            }
            let sealed = unsafe {
                if result.is_ok() && seal_error.is_none() {
                    session.publish()
                } else {
                    session.abort(varena::Error::Invalid(
                        "raw Source submission failed or canceled",
                    ))
                }
            };
            match sealed {
                Ok(ticket) => self.tickets.push(ticket),
                Err(error) => {
                    seal_error.get_or_insert(error);
                }
            }
        }
        // Submitted sessions are empty. Unstarted Prepared sessions queue no-write cleanup;
        // any unsealed Writing failure keeps its actual owners quarantined by native Drop.
        self.sessions.clear();
        if let Some(error) = seal_error {
            self.shared.fail(cuda::Error::Allocation(error.clone()));
            return Err(native_error(error));
        }
        result
    }

    /// Pure host observations. Resource retirement is separate from source completion.
    pub fn facts(&self) -> Result<RawReadFacts> {
        if self.shared.pid != std::process::id() {
            return Err(invalid("raw Source inherited after fork"));
        }
        let mut facts = self
            .shared
            .facts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let tickets = self
            .tickets
            .iter()
            .map(varena::FillTicket::facts)
            .collect::<varena::Result<Vec<_>>>()
            .map_err(native_error)?;
        facts.published_regions = tickets.iter().filter(|ticket| ticket.published).count();
        facts.completed = facts.stats.is_some() && tickets.iter().all(|ticket| ticket.completed);
        facts.submitted_fills_retired = tickets.iter().all(|ticket| ticket.retirement_finished);
        Ok(facts)
    }

    /// Query this operation's real events without any mutex held or stream/context wait.
    /// The existing PreparedRead adapter uses this on its native worker, never under the GIL.
    pub fn poll_complete(&self) -> Result<Option<ReadStats>> {
        let stats = self.shared.poll().map_err(cuda_error)?;
        for ticket in &self.tickets {
            let facts = ticket.facts().map_err(native_error)?;
            if facts.state == varena::FillState::Quarantined {
                return Err(native_error(
                    facts
                        .failure
                        .or(facts.retirement_failure)
                        .unwrap_or_else(|| varena::Error::Poisoned("raw Fill quarantined".into())),
                ));
            }
            if !facts.completed {
                return Ok(None);
            }
        }
        Ok(stats)
    }
}
impl Drop for RawRead {
    fn drop(&mut self) {
        if self.shared.pid != std::process::id() {
            return;
        }
        if !self.started {
            let _ = self.shared.close_unsubmitted();
        } else if !self.closed {
            self.shared.fail(cuda::Error::CudaUnavailable(
                "raw Source submission closure is unproven".into(),
            ));
        }
        // Session Drop owns prepared cleanup/unsealed quarantine; Shared is never the
        // sole GPU owner while a native session or submitted payload remains unfinished.
    }
}
