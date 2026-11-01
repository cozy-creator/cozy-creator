//! Native GPU cooperation. Peer is the explicit publication/service mechanism; Owner runs
//! that mechanism and sampled pressure on the library-owned thread. No daemon or hook.
use crate::{Arena, Completion, Contents, ReclaimReason, ledger, working};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, atomic::Ordering};

mod owner;
mod pressure;
pub use owner::{Facts, Owner, RecoveryOutcome, RecoveryReport};
pub use pressure::{Boundary, Facts as PressureFacts, initial_pid_namespace};
pub(crate) use owner::{Participation, recover};
pub(crate) use owner::wake_cache_actions;

fn deferred(error: &Error) -> bool {
    matches!(error,
        Error::Native(crate::Error::CaptureActive | crate::Error::Busy)
        | Error::Ledger(ledger::Error::Busy | ledger::Error::Stale | ledger::Error::StaleSample))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Native(#[from] crate::Error),
    #[error(transparent)]
    Ledger(#[from] ledger::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Default)]
pub struct Accounting {
    pub physical: ledger::Physical,
    pub reclaimable: u64,
    /// Whole mappings whose only remaining obstacle is known submitted consumer work.
    /// Unknown consumers and live graph owners are excluded.
    pub pending_release: u64,
    /// False means metadata only: capture/replay excluded every CUDA call.
    pub refreshed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Service {
    pub round: Option<u64>,
    pub released: u64,
    pub completed: u64,
    pub pending: u64,
    pub deferred: bool,
    pub accounting: Option<Accounting>,
    pub owner_action: bool,
    pub owner_pending: bool,
    pub owner_returned: u64,
    pub owner_declined: u64,
    pub owner_failed: u64,
}

struct Shrink {
    round: u64,
    key: crate::cache_owner::RequestKey,
    request: Option<crate::cache_owner::ShrinkRequest>,
    queueing: bool,
    start_completed: u64,
    target: usize,
}
impl Drop for Shrink {
    fn drop(&mut self) {
        if let Some(request) = &self.request { let _ = request.cancel(); }
    }
}

#[derive(Default)]
struct State {
    // Host-only handles cancel before the retained GPU leases are dropped.
    shrink: Option<Shrink>,
    // Retain observed arenas for this participant's explicit lifetime. A temporary snapshot
    // Arc must never become the final owner and run Arena::drop/context sync during service.
    arenas: Vec<Arc<Arena>>,
    last_use: u64,
    progress: Option<(u64, u64)>,
    activity: ledger::Activity,
}

impl State {
    fn cancel_shrink(&mut self) -> Result<()> {
        if let Some(shrink) = &self.shrink {
            if shrink.queueing { return Err(crate::Error::Busy.into()); }
            if let Some(request) = &shrink.request {
                request.cancel()?;
                if request.facts()?.running { return Err(crate::Error::Busy.into()); }
            }
        }
        self.shrink = None;
        Ok(())
    }
}

struct QueueAdmission<'a> {
    peer: &'a Peer,
    key: crate::cache_owner::RequestKey,
    armed: bool,
}
impl Drop for QueueAdmission<'_> {
    fn drop(&mut self) {
        if self.armed && self.peer.pid == std::process::id() {
            let mut state = self.peer.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(shrink) = state.shrink.as_mut().filter(|shrink| shrink.key == self.key) {
                shrink.queueing = false;
            }
        }
    }
}

/// One explicit participant for a process/device. The typed descriptor is verified against
/// cuDeviceGetUuid_v2; directory/UID/namespaces must satisfy ledger-foundation.md.
/// Dropping this owner may run ordinary final arena teardown: do it outside application,
/// capture and allocator locks. Service itself never drops its retained arena owners.
/// Use retire/try_close for capture-aware retirement; raw Rust Drop inside capture is not
/// qualified. The Python adapter retains and retries a refused owner on its existing queue.
pub struct Peer {
    device: i32,
    pid: u32,
    participant: ledger::Participant,
    ledger: ledger::Ledger,
    state: Mutex<State>,
}

/// Drop outside application locks. Field order keeps capture/replay excluded until every
/// retained Arena has been dropped, without holding the capture mutex across Arena::drop.
pub struct Retirement {
    _owner: Peer,
    _guard: crate::GlobalCaptureGuard,
}

impl Peer {
    /// Acquire retirement exclusion without destroying anything. On refusal the complete
    /// owner is returned unchanged. This lets an adapter unlock before actual final drops.
    pub fn retire(self) -> std::result::Result<Retirement, (Self, Error)> {
        let ready = self.lock().and_then(|mut state| state.cancel_shrink());
        if let Err(error) = ready { return Err((self, error)); }
        match crate::enter_retirement(self.device) {
            Ok(guard) => Ok(Retirement {
                _owner: self,
                _guard: guard,
            }),
            Err(error) => Err((self, error.into())),
        }
    }

    pub fn try_close(self) -> std::result::Result<(), (Self, Error)> {
        self.retire().map(drop)
    }

    pub fn open(
        device: i32,
        descriptor: ledger::Device,
        directory: &Path,
        sharing: ledger::Sharing,
        priority: i32,
    ) -> Result<Self> {
        crate::check_process()?;
        // initialize() opens no CUDA and must have run before concurrent fork producers.
        ledger::initialize()?;
        working::initialize(device)?;
        {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, device)?;
            if captures
                .get(&device)
                .is_some_and(|activity| activity.retirements.load(Ordering::Acquire) != 0)
            {
                return Err(crate::Error::Busy.into());
            }
            if working::peer::uuid(device)? != descriptor.uuid {
                return Err(
                    crate::Error::Invalid("ledger UUID does not match the CUDA device").into(),
                );
            }
        }
        // Native capture/context guards are gone before opening any shared lock.
        let ledger = ledger::Ledger::open_nonblocking(directory, descriptor)?;
        ledger.reap_dead()?;
        let participant = ledger.register(sharing, priority)?;
        let peer = Self {
            device,
            pid: std::process::id(),
            participant,
            ledger,
            state: Mutex::new(State::default()),
        };
        peer.publish(0)?;
        Ok(peer)
    }

    fn check(&self) -> Result<()> {
        if self.pid != std::process::id() {
            return Err(ledger::Error::Inherited.into());
        }
        crate::check_process()?;
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.check()?;
        self.state
            .lock()
            .map_err(|_| crate::Error::Poisoned("GPU participant mutex".into()).into())
    }

    pub fn id(&self) -> ledger::RowId {
        self.participant.id()
    }

    pub fn snapshot(&self) -> Result<ledger::Snapshot> {
        self.check()?;
        Ok(self.ledger.snapshot()?)
    }

    pub fn configure(&self, sharing: ledger::Sharing, priority: i32) -> Result<()> {
        // Serialize with the complete donor pass, including its captured assignment,
        // physical effects and acknowledgement. Once Dedicated returns, an earlier
        // Shared snapshot cannot start another release on this Peer.
        let mut state = self.lock()?;
        if sharing == ledger::Sharing::Dedicated { state.cancel_shrink()?; }
        Ok(self.participant.configure(sharing, priority)?)
    }

    /// Host lifecycle only. Publication carries the latest category; this never queries
    /// CUDA or waits for shared metadata on the caller's boundary.
    pub fn set_activity(&self, activity: ledger::Activity) -> Result<()> {
        self.lock()?.activity = activity;
        Ok(())
    }

    /// Actual host-monotonic last-use time supplied by the owner. Do not advance it merely
    /// because a periodic publication runs. No shared IPC is added to alloc/free.
    pub fn publish(&self, last_use: u64) -> Result<Accounting> {
        let mut state = self.lock()?;
        state.last_use = state.last_use.max(last_use);
        let reading = self.native_tick(&mut state, 0, 0, &mut 0)?;
        self.participant.publish_policy(reading.physical, reading.reclaimable, state.last_use,
            Some((state.activity, crate::cache_owner::requestable(self.device)? != 0)))?;
        Ok(reading)
    }

    /// Read actual CUDA free/total outside shared locks, then optimistically commit against
    /// the generation read before that observation. H/attribution remain unknown here.
    pub fn sample(&self, observed_at: u64) -> Result<ledger::Sample> {
        self.sample_reserve(observed_at, None)
    }

    fn sample_reserve(&self, observed_at: u64, reserve: Option<u64>) -> Result<ledger::Sample> {
        self.check()?;
        // Commit exact departures first, separately from optimistic sample validation.
        self.ledger.reap_dead()?;
        let expected = self.ledger.snapshot()?.generation;
        let (free, total) = {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, self.device)?;
            working::peer::memory_info(self.device)?
        };
        let used = total.checked_sub(free).ok_or(crate::Error::Invalid(
            "inconsistent CUDA capacity observation",
        ))?;
        Ok(self.participant.sample(
            expected,
            total as u64,
            used as u64,
            observed_at,
            reserve,
            reserve.is_some(),
        )?)
    }

    pub fn ask(&self, sample_generation: u64, wanted: u64) -> Result<ledger::Round> {
        self.check()?;
        Ok(self.participant.ask(sample_generation, wanted)?)
    }

    /// Coordinated device pressure, including this Shared owner's completed idle backing.
    pub fn ask_device(&self, sample_generation: u64, wanted: u64) -> Result<ledger::Round> {
        self.check()?;
        Ok(self.participant.ask_device(sample_generation, wanted)?)
    }

    pub fn ask_idle(&self, sample_generation: u64) -> Result<ledger::Round> {
        self.check()?;
        Ok(self.participant.ask_idle(sample_generation)?)
    }

    pub fn withdraw(&self, round: u64) -> Result<bool> {
        self.check()?;
        Ok(self.participant.withdraw(round)?)
    }

    pub fn clear_want(&self) -> Result<()> {
        self.check()?;
        Ok(self.participant.clear_want()?)
    }

    /// One finite no-wait donor pass. Copies the assignment before CUDA; publishes and acks
    /// only after native context/capture/allocator guards are gone. An ack is never a grant.
    pub fn service(&self) -> Result<Service> {
        self.check()?;
        if crate::cache_owner::is_dispatcher_thread(self.device)? {
            return Err(crate::Error::Busy.into());
        }
        let mut state = self.lock()?;
        if let Some(shrink) = state.shrink.as_ref().filter(|shrink| shrink.queueing) {
            return Ok(Service { round: Some(shrink.round), deferred: true, ..Service::default() });
        }
        self.ledger.reap_dead()?;
        let table = self.ledger.snapshot()?;
        let Some(round) = table.round.filter(|round| !round.aborted) else {
            state.cancel_shrink()?;
            return Ok(Service::default());
        };
        if state.shrink.as_ref().is_some_and(|shrink| shrink.round != round.generation) {
            state.cancel_shrink()?;
        }
        let Some(donor) = round
            .donations
            .iter()
            .find(|donor| donor.owner == self.id())
        else {
            state.cancel_shrink()?;
            return Ok(Service::default());
        };
        let mut result = Service {
            round: Some(round.generation),
            completed: donor.completed,
            pending: donor.pending,
            owner_action: donor.action,
            ..Service::default()
        };
        if matches!(
            donor.answer,
            ledger::Answer::Unasked | ledger::Answer::Complete | ledger::Answer::Refused | ledger::Answer::OwnerGone
        ) {
            state.cancel_shrink()?;
            return Ok(result);
        }
        let completed = state
            .progress
            .filter(|(id, _)| *id == round.generation)
            .map_or(donor.completed, |(_, done)| done.max(donor.completed));
        if table.rows.iter().find(|row| row.id == self.id())
            .is_some_and(|row| row.sharing == ledger::Sharing::Dedicated) {
            state.cancel_shrink()?;
            // Lifecycle configuration applies before any further native release. Preserve
            // prior actual progress, but an old assignment is not ongoing Shared consent.
            self.participant.acknowledge_stage(round.generation, donor.action, completed, 0, ledger::Answer::Refused)?;
            result.completed = completed;
            result.pending = 0;
            return Ok(result);
        }
        let maximum = donor.held_at_ask.saturating_sub(completed);
        let wanted = donor.requested.saturating_sub(completed);
        let observed_action = state.shrink.as_ref().and_then(|shrink| shrink.request.as_ref())
            .map(|request| request.facts().map(|facts| facts.action_epoch)).transpose()?.unwrap_or(0);
        let mut released = 0;
        let outcome =
            self.native_tick(&mut state, wanted as usize, maximum as usize, &mut released);
        // Preserve each successful physical effect even if the subsequent snapshot or
        // publication fails; a retry must not spend this donor snapshot a second time.
        result.released = released as u64;
        result.completed = completed + result.released;
        state.progress = Some((round.generation, result.completed));
        let reading = outcome?;
        result.deferred = !reading.refreshed;
        result.accounting = Some(reading);
        self.participant.publish_policy(reading.physical, reading.reclaimable, state.last_use,
            Some((state.activity, crate::cache_owner::requestable(self.device)? != 0)))?;
        if result.deferred {
            return Ok(result);
        }
        result.pending = if result.completed >= donor.requested {
            0
        } else {
            reading
                .pending_release
                .min(donor.held_at_ask.saturating_sub(result.completed))
        };
        if donor.action && result.completed < donor.requested {
            let key = match &state.shrink {
                Some(shrink) => shrink.key,
                None => {
                    let key = crate::cache_owner::RequestKey {
                        cause: match round.kind {
                            ledger::AskKind::Allocation => crate::cache_owner::Cause::Cooperative,
                            ledger::AskKind::DevicePressure | ledger::AskKind::AllIdle => crate::cache_owner::Cause::Foreign,
                        },
                        generation: crate::cache_owner::next_generation()?,
                    };
                    state.shrink = Some(Shrink { round: round.generation, key, request: None, queueing: false,
                        start_completed: result.completed, target: donor.requested.saturating_sub(result.completed) as usize });
                    key
                }
            };
            state.shrink.as_mut().expect("associated round").queueing = true;
            let shrink = state.shrink.as_ref().expect("associated round");
            let existing = shrink.request.clone();
            let target = shrink.target;
            let serviced = result.completed.saturating_sub(shrink.start_completed).min(target as u64) as usize;
            drop(state);
            // Queue only after the capped native pass and outside Peer, capture, allocator
            // and ledger guards. The distinct cache dispatcher owns callback/GIL execution.
            let mut admission = QueueAdmission { peer: self, key, armed: true };
            let queued = match existing {
                Some(request) => request.service_progress(serviced, observed_action).map(|()| Some(request)),
                None => crate::cache_owner::request_shrink(self.device, key, target),
            };
            state = self.lock()?;
            admission.armed = false;
            let shrink = state.shrink.as_mut().filter(|shrink| shrink.key == key)
                .ok_or(crate::Error::Busy)?;
            shrink.queueing = false;
            // A closing owner can make requestable zero while its admitted callback is
            // still running. Retain the old handle so Dedicated/retirement can see it.
            if let Some(request) = queued? { shrink.request = Some(request); }
            if let Some(request) = &shrink.request {
                let facts = request.facts()?;
                result.owner_pending = facts.running || (!facts.cancelled && !facts.finished);
                result.owner_returned = facts.returned;
                result.owner_declined = facts.declined;
                result.owner_failed = facts.failed;
                // observed_release_bytes is global process progress, never donor credit.
            }
        }
        let answer = if result.completed >= donor.requested {
            ledger::Answer::Complete
        } else if result.pending != 0 || result.owner_pending {
            ledger::Answer::Pending
        } else {
            ledger::Answer::Refused
        };
        if answer != ledger::Answer::Pending {
            if let Some(request) = state.shrink.as_ref().and_then(|shrink| shrink.request.as_ref()) {
                request.cancel()?;
            }
        }
        self.participant
            .acknowledge_stage(round.generation, donor.action, result.completed, result.pending, answer)?;
        Ok(result)
    }

    /// Metadata-only instrumentation, including initialized driver wrapper context/sampling
    /// calls. Used to prove a service tick inside capture/replay performs no CUDA operation.
    pub fn cuda_calls(&self) -> Result<u64> {
        let state = self.lock()?;
        Ok(working::peer::cuda_calls(self.device)?
            + state
                .arenas
                .iter()
                .map(|arena| arena.driver.activity_calls())
                .sum::<u64>())
    }

    fn memory_calls(&self) -> Result<u64> {
        let state = self.lock()?;
        Ok(working::peer::memory_calls(self.device)? + state.arenas.iter()
            .map(|arena| arena.driver.calls().0).sum::<u64>())
    }

    fn observation<T>(&self, operation: impl FnOnce() -> T) -> Result<T> {
        self.check()?;
        let captures = crate::capture_counts()?;
        crate::ensure_recovery_allowed(&captures, self.device)?;
        Ok(operation())
    }

    fn native_tick(
        &self,
        state: &mut State,
        wanted: usize,
        maximum: usize,
        released: &mut usize,
    ) -> crate::Result<Accounting> {
        let captures = crate::capture_counts()?;
        let refresh = crate::ensure_recovery_allowed(&captures, self.device).is_ok();
        // Filter before upgrading: never acquire/drop an unrelated device's last Arc here.
        if let Some(registry) = crate::ARENAS.get() {
            for entry in registry
                .lock()
                .map_err(|_| crate::Error::Poisoned("arena registry mutex".into()))?
                .iter()
                .filter(|entry| entry.device == self.device)
            {
                if !state.arenas.iter().any(|arena| arena.id() == entry.id)
                    && let Some(arena) = entry.owner.upgrade()
                {
                    state.arenas.push(arena);
                }
            }
        }
        if refresh && wanted != 0 {
            working::peer::release(self.device, wanted, maximum, released)?;
            for arena in &state.arenas {
                if *released >= wanted {
                    break;
                }
                arena.reclaim_ready_counted(&captures, wanted - *released, maximum - *released,
                    ReclaimReason::Room, released)?;
            }
        }
        let reading = working::peer::with_snapshot(self.device, refresh, |mut reading| {
            // Lock order: capture registry → working state → arena states in registration order.
            let mut states = state
                .arenas
                .iter()
                .map(|arena| arena.lock())
                .collect::<crate::Result<Vec<_>>>()?;
            for (arena, held) in state.arenas.iter().zip(&mut states) {
                let _context = refresh
                    .then(|| arena.driver.enter())
                    .transpose()
                    .map_err(|error| crate::poison(held, error))?;
                let ids: Vec<_> = held.entries.keys().copied().collect();
                for id in ids {
                    let entry = &held.entries[&id];
                    if entry.handle.is_none() {
                        continue;
                    }
                    let bytes = entry.bytes as u64;
                    if entry.uses.pins() != 0 || entry.uses.state() == Contents::Loading {
                        reading.physical.weights.live += bytes;
                        continue;
                    }
                    let completion = if refresh {
                        arena.completion(held, id)?
                    } else if entry.uses.unjoined.load(Ordering::Acquire) {
                        Completion::Unjoined
                    } else if entry.uses.fences().is_empty() {
                        Completion::Done
                    } else {
                        Completion::Pending
                    };
                    match completion {
                        Completion::Done => {
                            reading.physical.weights.idle += bytes;
                            reading.reclaimable += bytes;
                        }
                        Completion::Pending => {
                            reading.physical.weights.pending += bytes;
                            reading.pending_release += bytes;
                        }
                        Completion::Unjoined => reading.physical.weights.pending += bytes,
                    }
                }
            }
            let physical = reading
                .physical
                .total()
                .map_err(|_| crate::Error::Invalid("physical snapshot overflow"))?;
            let room = &captures.get(&self.device).expect("initialized device").room;
            if physical
                != (room.pages.load(Ordering::Acquire) + room.working.load(Ordering::Acquire))
                    as u64
            {
                // An arena already in its final Drop cannot be upgraded from Weak; its
                // teardown waits for this capture guard. Publish nothing, let that owner
                // finish outside this tick, and retry from a fresh observation.
                return Err(crate::Error::Busy);
            }
            Ok(reading)
        })?;
        Ok(reading)
    }
}
