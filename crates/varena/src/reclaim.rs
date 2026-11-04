//! The one reclaim order. Every memory consumer tries its real allocation before each step,
//! so nothing is evicted or waited on needlessly. No lock crosses a GPU wait or a callback,
//! and a wait holds a producer guard, so capture entry waits for it.
use crate::cuda::Driver;
use crate::working::{self, PoolId};
use crate::{
    AllocationFailure, AllocationResult, Completion, DeviceActivity, Error, PeerProgress,
    ReclaimReason, Result, cache_owner, plan,
};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Last resort against an owner callback that can never return, because it waits on something
/// this caller holds. No signal can tell that apart from a slow callback.
pub(crate) const RECOVERY_BUDGET: Duration = Duration::from_secs(2);

// Idle region classes, evicted lowest first. See `State::rank`.
pub(crate) const CONTENTLESS: u8 = 0;
pub(crate) const PREFETCHED: u8 = 1;
pub(crate) const UNPLANNED: u8 = 2;
pub(crate) const LATER_PHASE: u8 = 3;
pub(crate) const PHASE_USED: u8 = 4;
pub(crate) const PHASE_NEXT: u8 = 5;
pub(crate) const PREFERRED: u8 = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Peer service, owner collection and prefetch: completed memory only.
    No,
    /// May also wait for GPU work already submitted, never on another thread or process.
    Local,
    /// May also wait for owner callbacks and peers.
    Yes,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Protect<'a> {
    /// The caller's own demand is never evicted.
    pub(crate) regions: Option<&'a BTreeSet<u64>>,
    /// The graph pool being prepared keeps its pages.
    pub(crate) pool: Option<PoolId>,
    /// The caller's fit already counts completed working pages, or they are not its to take.
    pub(crate) keep_working: bool,
    /// One arena's explicit reclaim.
    pub(crate) arena: Option<u64>,
    /// A guess must not take a preferred or prefetched region.
    pub(crate) speculative: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    CompletedWorking,
    Contentless,
    PendingWorking,
    CompletedWeights,
    PendingWeights,
    PreferredWeights,
    Join,
    OwnerCaches,
    Peers,
}

// Pending working comes before any weight: scratch costs nothing to give back, while every
// evicted weight is read again (H3 estimate: 112 GB per run). Its wait time is measured, not assumed.
const ORDER: [Step; 9] = [
    Step::CompletedWorking,
    Step::Contentless,
    Step::PendingWorking,
    Step::CompletedWeights,
    Step::PendingWeights,
    Step::PreferredWeights,
    Step::Join,
    Step::OwnerCaches,
    Step::Peers,
];

static HOST_LOCK: OnceLock<fn() -> bool> = OnceLock::new();

/// A host runtime's "does this thread hold your global lock" (the GIL). Owner callbacks need
/// that lock, so an allocation holding it fails fast rather than waiting for them.
pub fn set_host_lock_probe(probe: fn() -> bool) {
    let _ = HOST_LOCK.set(probe);
}

struct Sequence<'a> {
    device: i32,
    activity: &'static DeviceActivity,
    wait: Wait,
    protect: Protect<'a>,
    reason: ReclaimReason,
    maximum: usize,
    step: usize,
    released: usize,
    evicted: usize,
    recovery: Recovery,
}

struct Candidate {
    rank: (u8, u64),
    arena: usize,
    id: u64,
    bytes: usize,
}

#[derive(Default)]
struct Pass {
    events: Vec<usize>,
    unjoined: bool,
}

impl<'a> Sequence<'a> {
    fn new(device: i32, maximum: usize, wait: Wait, protect: Protect<'a>, reason: ReclaimReason) -> Result<Self> {
        Ok(Self {
            device,
            activity: crate::activity(device)?,
            wait,
            protect,
            reason,
            maximum,
            step: 0,
            released: 0,
            evicted: 0,
            recovery: Recovery::default(),
        })
    }

    /// Run steps until one makes progress. False: every step is exhausted.
    fn advance(&mut self, want: usize, physical: bool) -> Result<bool> {
        while let Some(step) = ORDER.get(self.step).copied() {
            if self.run(step, want, physical)? {
                return Ok(true);
            }
            self.step += 1;
        }
        Ok(false)
    }

    fn working(&mut self, want: usize) -> Result<()> {
        if !self.protect.keep_working {
            self.released += working::release_completed(self.device, want,
                self.maximum.saturating_sub(self.released), self.protect.pool)?;
        }
        Ok(())
    }

    fn run(&mut self, step: Step, want: usize, physical: bool) -> Result<bool> {
        let waiting = self.wait != Wait::No;
        let released = self.released;
        match step {
            Step::CompletedWorking => {
                self.working(want)?;
                Ok(self.released > released)
            }
            Step::Contentless => self.weights(want, CONTENTLESS..=CONTENTLESS, waiting, waiting),
            Step::CompletedWeights => {
                let first = if self.protect.speculative { UNPLANNED } else { PREFETCHED };
                self.weights(want, first..=PHASE_NEXT, false, false)
            }
            Step::PendingWorking if waiting && self.protect.arena.is_none() => {
                if !working::wait_pending(self.device, want, self.protect.pool)? {
                    return Ok(false);
                }
                self.working(want)?;
                Ok(true)
            }
            Step::PendingWeights if waiting => self.weights(want, PREFETCHED..=PHASE_NEXT, true, false),
            Step::PreferredWeights if !self.protect.speculative => {
                self.weights(want, PREFERRED..=PREFERRED, waiting, false)
            }
            Step::Join if waiting => {
                let classes = CONTENTLESS..=PREFERRED;
                let blocks = self.protect.arena.is_none() && working::unjoined(self.device)?;
                if self.weights(want, classes.clone(), false, !blocks)? {
                    return Ok(true);
                }
                if !blocks {
                    return Ok(false);
                }
                let hold = crate::enter_producer(self.device)?;
                let driver = self.activity.driver(self.device)?;
                wait_outside(self.device, self.activity, &driver, &[], true)?;
                drop(hold);
                self.working(want)?;
                self.weight_pass(want, &classes)?;
                Ok(true)
            }
            Step::OwnerCaches if self.wait == Wait::Yes => self.recovery.owner_caches(self.device, want),
            Step::Peers if self.wait == Wait::Yes => self.recovery.peers(self.device, want, physical),
            _ => Ok(false),
        }
    }

    /// Evict completed idle regions of these classes in rank order. May then wait for pending
    /// ones outside every lock and take them; `join` also covers unknown consumers.
    fn weights(&mut self, want: usize, classes: RangeInclusive<u8>, wait: bool, join: bool) -> Result<bool> {
        let released = self.released;
        let first = self.weight_pass(want, &classes)?;
        if self.released > released {
            return Ok(true);
        }
        let joining = join && first.unjoined;
        if (!wait || first.events.is_empty()) && !joining {
            return Ok(false);
        }
        let hold = crate::enter_producer(self.device)?;
        let driver = self.activity.driver(self.device)?;
        let events = if wait { first.events.as_slice() } else { &[] };
        wait_outside(self.device, self.activity, &driver, events, joining)?;
        drop(hold);
        self.weight_pass(want, &classes)?;
        Ok(true)
    }

    fn weight_pass(&mut self, want: usize, classes: &RangeInclusive<u8>) -> Result<Pass> {
        // GPU owners and plan data precede the gate, so a last owner's drop never runs under it.
        let arenas = crate::reclaim_arenas(self.device)?;
        let order = plan::order(self.device)?;
        let gate = crate::gate(self.device)?;
        gate.not_capturing()?;
        let mut candidates = Vec::new();
        for (index, arena) in arenas.iter().enumerate() {
            if self.protect.arena.is_some_and(|id| id != arena.id()) {
                continue;
            }
            let mut state = arena.lock()?;
            if index == 0 {
                arena.poll_fences(&mut state)?;
            }
            for (id, entry) in &state.entries {
                let rank = state.rank(*id, entry, &order);
                if entry.idle()
                    && classes.contains(&rank.0)
                    && !self.protect.regions.is_some_and(|ids| ids.contains(id))
                {
                    candidates.push(Candidate { rank, arena: index, id: *id, bytes: entry.bytes });
                }
            }
        }
        candidates.sort_by_key(|candidate| candidate.rank);
        // The preferred suffix keeps strict order: each eviction shortens the retained prefix.
        let strict = *classes.start() == PREFERRED;
        let start = self.released;
        let mut pass = Pass::default();
        let mut held = 0usize;
        for candidate in candidates {
            if self.released - start + held >= want {
                break;
            }
            let arena = &arenas[candidate.arena];
            let mut state = arena.lock()?;
            if !state.entries.get(&candidate.id).is_some_and(crate::Entry::idle) {
                continue;
            }
            match arena.completion(&state, candidate.id) {
                Completion::Done => {
                    if candidate.bytes <= self.maximum.saturating_sub(self.released) {
                        let _context = arena.driver.enter()?;
                        arena.evict(&mut state, candidate.id, self.reason, &mut self.released)?;
                        self.evicted += 1;
                    }
                }
                Completion::Pending => {
                    let uses = &state.entries[&candidate.id].uses;
                    self.activity.fences().events(&uses.waits(), &mut pass.events);
                    held += candidate.bytes;
                    if strict {
                        break;
                    }
                }
                Completion::Unjoined => {
                    pass.unjoined = true;
                    if strict {
                        break;
                    }
                }
            }
        }
        Ok(pass)
    }
}

/// Call `try_map` until it succeeds, running the reclaim order between attempts. `need` is what
/// it will ask of the driver; a split refusal states its own deficit instead.
pub(crate) fn acquire<T>(
    device: i32,
    need: usize,
    wait: Wait,
    protect: Protect<'_>,
    mut try_map: impl FnMut() -> AllocationResult<T>,
) -> Result<T> {
    let mut sequence = Sequence::new(device, usize::MAX, wait, protect, ReclaimReason::Allocator)?;
    let mut floor = 2usize << 20;
    loop {
        let failure = match try_map() {
            Ok(value) => return Ok(value),
            Err(failure) if failure.is_oom() => failure,
            Err(failure) => return Err(failure.into()),
        };
        let want = shortfall(device, need, &failure, sequence.activity)?.max(floor);
        sequence.released = 0;
        // Other processes' memory cannot lift a local cap.
        let physical = failure.physical_bytes().is_some()
            || matches!(failure, AllocationFailure::Other(Error::Exhausted { .. }))
                && sequence.activity.room.over(want) == 0;
        if !sequence.advance(want, physical)? {
            return Err(failure.into());
        }
        // Short of what the arithmetic said: the driver's free reading is only approximate.
        floor = floor.saturating_mul(2);
    }
}

fn shortfall(device: i32, need: usize, failure: &AllocationFailure, activity: &DeviceActivity) -> Result<usize> {
    if let AllocationFailure::Other(Error::Exhausted { requested_bytes, available_bytes }) = failure {
        return Ok(requested_bytes.saturating_sub(*available_bytes));
    }
    let over = activity.room.over(need);
    if over != 0 {
        return Ok(over);
    }
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let driver = activity.driver(device)?;
    let _context = driver.enter()?;
    Ok(need.saturating_sub(driver.memory_info()?.0))
}

/// No allocation to retry: give up `want` bytes in the same order. Returns regions evicted.
/// `completed` keeps every real release, even if a later step fails.
pub(crate) fn release(
    device: i32,
    want: usize,
    maximum: usize,
    wait: Wait,
    protect: Protect<'_>,
    reason: ReclaimReason,
    completed: &mut usize,
) -> Result<usize> {
    let mut sequence = Sequence::new(device, maximum, wait, protect, reason)?;
    let result = (|| {
        while sequence.released < want && sequence.step < ORDER.len() {
            let left = want - sequence.released;
            if !sequence.run(ORDER[sequence.step], left, false)? {
                sequence.step += 1;
            }
        }
        Ok(())
    })();
    *completed += sequence.released;
    result.map(|()| sequence.evicted)
}

/// The prefetch rule: free >= bytes + what this phase's last pass still needs of working
/// memory, where free counts completed free working pages and completed idle regions that
/// reclaim would take anyway. False declines with no IO and no waiting.
pub(crate) fn prefetch_room(device: i32, bytes: usize, group: &BTreeSet<u64>) -> Result<bool> {
    let Some((gap, reusable)) = working::prefetch_gap(device)? else {
        return Ok(false);
    };
    // Mapped free pages serve the phase's working demand first.
    let needed = bytes + gap.saturating_sub(reusable);
    let excess = reusable.saturating_sub(gap);
    let activity = crate::activity(device)?;
    let driver = activity.driver(device)?;
    let protect = Protect { regions: Some(group), speculative: true, keep_working: true, ..Default::default() };
    let (room, idle) = {
        let arenas = crate::reclaim_arenas(device)?;
        let order = plan::order(device)?;
        let gate = crate::gate(device)?;
        if gate.not_capturing().is_err() {
            return Ok(false);
        }
        let _context = driver.enter()?;
        let room = driver.memory_info()?.0.min(activity.room.headroom());
        let mut idle = 0usize;
        for arena in &arenas {
            let state = arena.lock()?;
            for (id, entry) in &state.entries {
                let class = state.rank(*id, entry, &order).0;
                if entry.idle() && class <= PHASE_NEXT && class != PREFETCHED && !group.contains(id)
                    && arena.completion(&state, *id) == Completion::Done
                {
                    idle += entry.bytes;
                }
            }
        }
        (room, idle)
    };
    if needed <= room {
        return Ok(true);
    }
    let short = needed - room;
    if short > idle + excess {
        return Ok(false);
    }
    let trimmed = working::release_completed(device, short.min(excess), usize::MAX, None)?;
    if trimmed < short {
        let mut released = 0;
        release(device, short - trimmed, usize::MAX, Wait::No, protect, ReclaimReason::Allocator, &mut released)?;
    }
    Ok(true)
}

/// Wait for this region's own abandoned users, outside every lock.
pub(crate) fn settle(region: &crate::Region) -> Result<()> {
    let device = region.device();
    let arena = &region.arena;
    let (events, joining, hold) = {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = arena.lock()?;
        arena.poll_fences(&mut state)?;
        let mut events = Vec::new();
        arena.activity.fences().events(&region.uses.waits(), &mut events);
        let joining = !arena.activity.joined(region.uses.unjoined.load(Ordering::Acquire));
        (events, joining, gate.hold()?)
    };
    wait_outside(device, arena.activity, &arena.driver, &events, joining)?;
    drop(hold);
    Ok(())
}

/// A context join outside every lock, covering every release marked before it starts.
pub(crate) fn join(device: i32, driver: &Driver) -> Result<()> {
    let hold = crate::enter_producer(device)?;
    wait_outside(device, crate::activity(device)?, driver, &[], true)?;
    drop(hold);
    Ok(())
}

pub(crate) fn wait_outside(device: i32, activity: &DeviceActivity, driver: &Driver, events: &[usize], join: bool) -> Result<()> {
    let result = (|| {
        let _context = driver.enter()?;
        for event in events {
            driver.wait_event(*event)?;
        }
        if join {
            let started = activity.joins_started.fetch_add(1, Ordering::AcqRel) + 1;
            driver.synchronize()?;
            activity.joins_finished.fetch_max(started, Ordering::AcqRel);
        }
        Ok(())
    })();
    result.inspect_err(|error| fault(device, error))
}

/// A failed wait is a device fault: nothing on this device can prove completion any more.
fn fault(device: i32, error: &Error) {
    working::poison(device, error);
    if let Ok(arenas) = crate::reclaim_arenas(device) {
        for arena in arenas {
            if let Ok(mut state) = arena.state.lock() {
                state.poison.get_or_insert_with(|| error.to_string());
            }
        }
    }
}

/// Owner caches and peers: the only steps that wait on other threads or processes.
#[derive(Default)]
struct Recovery {
    deadline: Option<Instant>,
    observed: Option<(u64, u64)>,
    cache_request: Option<cache_owner::ShrinkRequest>,
    peers_done: bool,
    #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
    participation: Option<crate::cooperative::Participation>,
}

impl Recovery {
    /// Queue owner shrinks and wait for a release, for the shrink to finish, or for the last
    /// resort bound, on a physical card as under a local cap. No wait where owner callbacks
    /// cannot run: this thread is the dispatcher (no request) or holds the host lock.
    fn owner_caches(&mut self, device: i32, bytes: usize) -> Result<bool> {
        let limit = *self.deadline.get_or_insert_with(|| Instant::now() + RECOVERY_BUDGET);
        if Instant::now() >= limit || cache_owner::requestable(device)? == 0 {
            return Ok(false);
        }
        let before = crate::released_bytes(device)?;
        self.cache_request = cache_owner::request_shrink(device, cache_owner::RequestKey {
            cause: cache_owner::Cause::Allocation, generation: cache_owner::next_generation()?,
        }, bytes)?;
        let Some(request) = &self.cache_request else { return Ok(false) };
        if HOST_LOCK.get().is_some_and(|holds| holds()) {
            return Ok(crate::released_bytes(device)? > before);
        }
        working::invalidate_current_phase(device, "cache_recovery")?;
        request.wait_release(before, limit)
    }

    /// Only a physical refusal asks other processes: their memory cannot lift a local cap.
    fn peers(&mut self, device: i32, bytes: usize, physical: bool) -> Result<bool> {
        if !physical || self.peers_done {
            return Ok(false);
        }
        #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
        {
            let before = crate::released_bytes(device)?;
            let progress = crate::cooperative::recover(device, bytes, &mut self.deadline, &mut self.observed,
                &mut self.participation, self.cache_request.as_ref().map(|_| before))?;
            self.peers_done = !matches!(progress, PeerProgress::Retry);
            if self.peers_done {
                drop(self.participation.take());
            }
            // A final answer still earns one last try of the real allocation.
            Ok(!matches!(progress, PeerProgress::Unavailable))
        }
        #[cfg(not(all(target_os = "linux", target_has_atomic = "64")))]
        {
            let _ = (device, bytes, &mut self.deadline, &mut self.observed);
            Ok(false)
        }
    }
}

