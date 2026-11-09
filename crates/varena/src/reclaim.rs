//! The one reclaim order. Every memory consumer tries its real allocation before each step,
//! so nothing is evicted or waited on needlessly. No lock crosses a GPU wait or a callback,
//! and a wait holds a producer guard, so capture entry waits for it.
use crate::working::{self, PoolId};
use crate::{
    AllocationFailure, AllocationResult, Completion, Device, Error,
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
pub(crate) const PHASE_FRESH: u8 = 4;
pub(crate) const PHASE_USED: u8 = 5;
pub(crate) const PHASE_NEXT: u8 = 6;
pub(crate) const HIGH: u8 = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Peer service, owner collection and prefetch: completed memory only.
    No,
    /// May also wait for GPU work already submitted and, before this process's weights go, an
    /// idle peer's answer; never on another thread, or on a process in a request.
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
    /// A weight's demand, or a loop's working one, leaves the free pages the rest of this phase
    /// will use again: they would only map straight back. It takes weights first, these pages last.
    pub(crate) keep_phase: bool,
    /// A guess must not take another guess.
    pub(crate) speculative: bool,
    /// The caller's retry takes free working pages in place: what a wait completes is tried
    /// once before any working page goes.
    pub(crate) reuse_working: bool,
    /// The absent region this is for: a region of its size evicted moves its pages there.
    pub(crate) into: Option<u64>,
    /// No smaller unit stands behind the caller (a Region's fill, a working allocation): with
    /// nothing of ours left to give, it waits for memory held outside this process (`OUTSIDE_WAIT`).
    pub(crate) outside: bool,
}

/// How long a demand with no lever left of its own waits for memory outside this process to come
/// back (a game's, a browser's), re-reading the room every `OUTSIDE_POLL` and trying every lever
/// again once it grew: Paul 11-07, keep trying while any lever can make it fit, give up last.
/// retained-pressure's game holds 18 GiB of a 4090 for 120 s; Qwen's K/V refill failed at once (run
/// 43123).
const OUTSIDE_WAIT: Duration = Duration::from_secs(300);
const OUTSIDE_POLL: Duration = Duration::from_millis(250);

/// Whether at least `want` more bytes came free on the card than at `start` before `deadline`.
fn outside_gave(gpu: &Device, want: usize, start: usize, deadline: Instant) -> Result<bool> {
    while Instant::now() < deadline {
        std::thread::sleep(OUTSIDE_POLL);
        if free_now(gpu)? >= start.saturating_add(want) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The driver's free bytes now, the room's outside share sampled with them.
fn free_now(gpu: &Device) -> Result<usize> {
    let gate = crate::gate(gpu.ordinal)?;
    gate.not_capturing()?;
    let _context = gpu.driver().enter()?;
    gpu.sample()?;
    Ok(gpu.driver().memory_info()?.0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    CompletedWorking,
    Contentless,
    IdlePools,
    PendingWorking,
    IdlePeers,
    Weights,
    Join,
    OwnerCaches,
    Peers,
    HighWeights,
}

// Scratch comes before any weight: it costs a remap or a wait, while every evicted weight is
// read again (H3 estimate for pending working: 112 GB per run). Time is measured, not assumed.
pub(crate) const STEPS: usize = 10;
const ORDER: [Step; STEPS] = [
    Step::CompletedWorking,
    Step::Contentless,
    Step::IdlePools,
    Step::PendingWorking,
    Step::IdlePeers,
    Step::Weights,
    Step::Join,
    Step::OwnerCaches,
    Step::Peers,
    Step::HighWeights,
];

static HOST_LOCK: OnceLock<fn() -> bool> = OnceLock::new();

/// A host runtime's "does this thread hold your global lock" (the GIL). Owner callbacks need
/// that lock, so an allocation holding it fails fast rather than waiting for them.
pub fn set_host_lock_probe(probe: fn() -> bool) {
    let _ = HOST_LOCK.set(probe);
}

struct Sequence<'a> {
    device: i32,
    gpu: &'static Device,
    wait: Wait,
    protect: Protect<'a>,
    reason: ReclaimReason,
    maximum: usize,
    step: usize,
    released: usize,
    evicted: usize,
    recovery: Recovery,
    ahead_stopped: bool,
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
            gpu: crate::gpu(device)?,
            wait,
            protect,
            reason,
            maximum,
            step: 0,
            released: 0,
            evicted: 0,
            recovery: Recovery::default(),
            ahead_stopped: false,
        })
    }

    /// Run steps until one makes progress. False: every step is exhausted.
    fn advance(&mut self, want: usize, physical: bool) -> Result<bool> {
        // A fill ahead of demand is a guess: it stops before anything real gives way.
        if self.wait != Wait::No && !std::mem::replace(&mut self.ahead_stopped, true)
            && crate::source::stop_and_wait(self.gpu)
        {
            return Ok(true);
        }
        while let Some(step) = ORDER.get(self.step).copied() {
            let (index, started) = (self.step, Instant::now());
            let done = self.run(step, want, physical);
            self.gpu.driver().note_reclaim(index, started.elapsed());
            if done? {
                return Ok(true);
            }
            self.step += 1;
            if step == Step::Weights && self.protect.keep_phase {
                self.protect.keep_phase = false;
                self.step = 0;
            }
        }
        Ok(false)
    }

    fn working(&mut self, want: usize) -> Result<()> {
        if !self.protect.keep_working {
            let maximum = self.maximum.saturating_sub(self.released);
            self.released += if self.protect.keep_phase {
                working::release_beyond_phase(self.device, want, maximum, self.protect.pool)?
            } else {
                working::release_completed(self.device, want, maximum, self.protect.pool)?
            };
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
            Step::IdlePools => {
                self.released += working::release_idle_pools(self.device, want,
                    self.maximum.saturating_sub(self.released), self.protect.pool)?;
                Ok(self.released > released)
            }
            Step::Weights => {
                // A guess takes no other guess and no region this phase still needs.
                let classes = if self.protect.speculative { UNPLANNED..=PHASE_FRESH } else { PREFETCHED..=PHASE_NEXT };
                self.weights(want, classes, waiting, waiting)
            }
            Step::PendingWorking if waiting => {
                if self.protect.keep_phase && working::phase_known(self.device)? {
                    return Ok(false);
                }
                if !working::wait_pending(self.device, want, self.protect.pool)? {
                    return Ok(false);
                }
                // The retry uses what completed in place; if it can't, the order starts over.
                if std::mem::take(&mut self.protect.reuse_working) {
                    self.step = 0;
                    return Ok(true);
                }
                self.working(want)?;
                Ok(true)
            }
            // Processes in no request hold what nothing is about to use: theirs before our weights.
            Step::IdlePeers if waiting && physical => Ok(idle_peers_gave(self.device, want)),
            Step::Join if waiting => {
                let classes = CONTENTLESS..=PHASE_NEXT;
                let blocks = working::unjoined(self.device)?;
                if self.weights(want, classes.clone(), false, !blocks)? {
                    return Ok(true);
                }
                if !blocks {
                    return Ok(false);
                }
                let hold = crate::enter_producer(self.device)?;
                wait_outside(self.gpu, &[], true)?;
                drop(hold);
                self.working(want)?;
                self.weight_pass(want, &classes)?;
                Ok(true)
            }
            Step::OwnerCaches if self.wait == Wait::Yes => self.recovery.owner_caches(self.device, want),
            Step::Peers if self.wait == Wait::Yes => self.recovery.peers(self.device, want, physical),
            // Only `Yes` has asked the owner caches these rank above.
            Step::HighWeights if self.wait == Wait::Yes && self.gpu.any_high.load(Ordering::Acquire) => {
                self.weights(want, HIGH..=HIGH, true, true)
            }
            _ => Ok(false),
        }
    }

    /// Evict completed idle regions of these classes in rank order. One still in use on the GPU is
    /// passed over: waiting for the block just launched starts the copy only once the GPU has run
    /// dry (Flux-2 on a 4090: 0.4 s a step). Short of completed ones in its class, `wait` waits
    /// outside every lock for those passed over that finish first, before a costlier class goes.
    /// `join` also covers unknown consumers.
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
        let events = if wait { first.events.as_slice() } else { &[] };
        wait_outside(self.gpu, events, joining)?;
        drop(hold);
        self.weight_pass(want, &classes)?;
        Ok(true)
    }

    fn weight_pass(&mut self, want: usize, classes: &RangeInclusive<u8>) -> Result<Pass> {
        // Plan data precedes the gate: a Plan's last drop reaches Regions, which need it.
        let order = plan::order(self.device)?;
        let gate = crate::gate(self.device)?;
        gate.not_capturing()?;
        let mut state = self.gpu.lock()?;
        self.gpu.poll_fences(&mut state)?;
        let prefix = state.prefix(&order);
        let mut candidates: Vec<_> = state.entries.iter()
            .filter(|(id, entry)| entry.idle() && !self.protect.regions.is_some_and(|ids| ids.contains(id)))
            .map(|(id, entry)| (state.rank(*id, entry, &order, prefix), *id, entry.bytes))
            .filter(|(rank, ..)| classes.contains(&rank.0))
            .collect();
        candidates.sort_unstable();
        let start = self.released;
        let mut pass = Pass::default();
        let mut pending: Vec<(u64, u8, u64, usize)> = Vec::new();
        for ((class, _), id, bytes) in candidates {
            if self.released - start >= want {
                break;
            }
            // A costlier class goes only once this one's uses have finished.
            if pending.last().is_some_and(|&(_, other, ..)| other != class) {
                break;
            }
            match self.gpu.completion(&state, id) {
                Completion::Done => {
                    if bytes <= self.maximum.saturating_sub(self.released) {
                        let _context = self.gpu.driver().enter()?;
                        self.gpu.evict(&mut state, id, self.reason, &mut self.released, self.protect.into)?;
                        self.evicted += 1;
                    }
                }
                Completion::Pending => pending.push((state.entries[&id].last_use, class, id, bytes)),
                Completion::Unjoined => pass.unjoined = true,
            }
        }
        // Uses run in order on the GPU: the oldest finishes first.
        pending.sort_unstable();
        let mut short = want.saturating_sub(self.released - start);
        for (_, _, id, bytes) in pending {
            if short == 0 {
                break;
            }
            self.gpu.fences().events(&state.entries[&id].uses.waits(), &mut pass.events);
            short = short.saturating_sub(bytes);
        }
        Ok(pass)
    }
}

fn idle_peers_gave(device: i32, bytes: usize) -> bool {
    #[cfg(target_os = "linux")]
    return crate::cooperative::peers_idle(device) && crate::cooperative::ask_idle_peers(device, bytes);
    #[cfg(not(target_os = "linux"))]
    { let _ = (device, bytes); false }
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
    let mut waiting: Option<Instant> = None;
    loop {
        let failure = match try_map() {
            Ok(value) => return Ok(value),
            Err(failure) if failure.is_oom() => failure,
            Err(failure) => return Err(failure.into()),
        };
        let want = shortfall(need, &failure, sequence.gpu)?.max(floor);
        sequence.released = 0;
        let physical = !sequence.gpu.capped() || failure.physical_bytes().is_some()
            || sequence.gpu.peers_can_lift(want);
        if !sequence.advance(want, physical)? {
            // Nothing of ours is left to give. What came onto the card outside this process since
            // its quietest sample (a game's, a peer's) may go again: wait for it, and once the card
            // has `want` more free, every lever again from the first.
            if protect.outside && wait == Wait::Yes {
                let start = free_now(sequence.gpu)?;
                let came = sequence.gpu.outside.load(Ordering::Acquire)
                    .saturating_sub(sequence.gpu.outside_floor.load(Ordering::Acquire));
                if came >= want {
                    let began = *waiting.get_or_insert_with(Instant::now);
                    if outside_gave(sequence.gpu, want, start, began + OUTSIDE_WAIT)? {
                        sequence = Sequence::new(device, usize::MAX, wait, protect, ReclaimReason::Allocator)?;
                        continue;
                    }
                    eprintln!(
                        "varena: GPU {device}: {need} bytes found no room after every lever and {} s waiting \
                         for the {came} bytes that came outside this process: {start} bytes free",
                        began.elapsed().as_secs(),
                    );
                }
            }
            return Err(failure.into());
        }
        // Gave what the arithmetic said and still short: the driver's free reading is only
        // approximate. A wait that released nothing asks for no more: that overshoot evicted GBs.
        if sequence.released >= want {
            floor = want.saturating_mul(2);
        }
    }
}

fn shortfall(need: usize, failure: &AllocationFailure, gpu: &Device) -> Result<usize> {
    if let AllocationFailure::Other(Error::Exhausted { requested_bytes, available_bytes }) = failure {
        return Ok(requested_bytes.saturating_sub(*available_bytes));
    }
    let over = gpu.over(need);
    if over != 0 {
        return Ok(over);
    }
    let gate = crate::gate(gpu.ordinal)?;
    gate.not_capturing()?;
    let _context = gpu.driver().enter()?;
    Ok(need.saturating_sub(gpu.driver().memory_info()?.0))
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
    if working::existing(device)?.is_none() {
        return Ok(0); // Never opened: no bytes here to give up.
    }
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

/// A peer's ask, once this process's owner caches have answered: completed high regions may go.
pub(crate) fn release_high(device: i32, want: usize) -> Result<()> {
    let mut sequence = Sequence::new(device, usize::MAX, Wait::No, Protect::default(), ReclaimReason::Allocator)?;
    if sequence.gpu.any_high.load(Ordering::Acquire) {
        sequence.weights(want, HIGH..=HIGH, false, false)?;
    }
    Ok(())
}

/// The prefetch rule: free >= bytes + what this phase's last pass still needs of working
/// memory, where free counts completed free working pages and completed idle regions that
/// reclaim would take anyway. With nothing measured yet, a first pass, it reads ahead only
/// into room already free, and so does `free_only`: a fill past the one a use asked for. False
/// declines with no IO and no waiting.
pub(crate) fn prefetch_room(device: i32, bytes: usize, group: &BTreeSet<u64>, free_only: bool) -> Result<bool> {
    let Some((gap, reusable, excess, known)) = working::prefetch_gap(device)? else {
        return Ok(false);
    };
    // Mapped free pages serve the phase's working demand first.
    let needed = bytes + gap.saturating_sub(reusable);
    let gpu = crate::gpu(device)?;
    let into = group.first().copied();
    let protect = Protect { regions: Some(group), speculative: true, keep_working: true, into, ..Default::default() };
    let (room, idle) = {
        let order = plan::order(device)?;
        let gate = crate::gate(device)?;
        if gate.not_capturing().is_err() {
            return Ok(false);
        }
        let _context = gpu.driver().enter()?;
        let room = gpu.driver().memory_info()?.0.min(gpu.headroom());
        let state = gpu.lock()?;
        let prefix = state.prefix(&order);
        let idle = state.entries.iter()
            .filter(|(id, entry)| {
                let class = state.rank(**id, entry, &order, prefix).0;
                entry.idle() && (UNPLANNED..=PHASE_FRESH).contains(&class) && !group.contains(id)
                    && gpu.completion(&state, **id) == Completion::Done
            })
            .map(|(_, entry)| entry.bytes)
            .sum::<usize>();
        (room, idle)
    };
    if needed <= room {
        return Ok(true);
    }
    let short = needed - room;
    if !known || free_only || short > idle + excess {
        return Ok(false);
    }
    let trimmed = working::release_beyond_phase(device, short.min(excess), usize::MAX, None)?;
    if trimmed < short {
        let mut released = 0;
        release(device, short - trimmed, usize::MAX, Wait::No, protect, ReclaimReason::Allocator, &mut released)?;
    }
    Ok(true)
}

/// Wait for this region's own abandoned users, outside every lock.
pub(crate) fn settle(region: &crate::Region) -> Result<()> {
    let gpu = region.gpu;
    let (events, joining, hold) = {
        let gate = crate::gate(gpu.ordinal)?;
        gate.not_capturing()?;
        let mut state = gpu.lock()?;
        gpu.poll_fences(&mut state)?;
        let mut events = Vec::new();
        gpu.fences().events(&region.uses.waits(), &mut events);
        let joining = !gpu.joined(region.uses.unjoined.load(Ordering::Acquire));
        (events, joining, gate.hold()?)
    };
    wait_outside(gpu, &events, joining)?;
    drop(hold);
    Ok(())
}

/// A context join outside every lock, covering every release marked before it starts.
pub(crate) fn join(gpu: &Device) -> Result<()> {
    let hold = crate::enter_producer(gpu.ordinal)?;
    wait_outside(gpu, &[], true)?;
    drop(hold);
    Ok(())
}

pub(crate) fn wait_outside(gpu: &Device, events: &[usize], join: bool) -> Result<()> {
    let result = (|| {
        let driver = gpu.driver();
        let _context = driver.enter()?;
        for event in events {
            driver.wait_event(*event)?;
        }
        if join {
            let started = gpu.joins_started.fetch_add(1, Ordering::AcqRel) + 1;
            driver.synchronize()?;
            gpu.joins_finished.fetch_max(started, Ordering::AcqRel);
        }
        Ok(())
    })();
    // A failed wait is a device fault: nothing here can prove completion any more.
    result.inspect_err(|error: &Error| {
        if let Ok(mut state) = gpu.state.lock() {
            state.poison.get_or_insert_with(|| error.to_string());
        }
    })
}

/// Owner caches and peers: the only steps that wait on other threads or processes.
#[derive(Default)]
struct Recovery {
    deadline: Option<Instant>,
    cache_request: Option<cache_owner::ShrinkRequest>,
    peers_done: bool,
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
            generation: cache_owner::next_generation()?,
        }, bytes)?;
        let Some(request) = self.cache_request.as_ref() else { return Ok(false) };
        // Owners whose callbacks take the host lock cannot run while this thread holds it; native
        // ones can, so with only those this waits.
        if HOST_LOCK.get().is_some_and(|holds| holds()) && cache_owner::hosted(device)? != 0 {
            return Ok(crate::released_bytes(device)? > before);
        }
        working::invalidate_current_phase(device, "cache_recovery")?;
        request.wait_release(before, limit)
    }

    /// Only a physical refusal asks other processes: their memory cannot lift a local cap. A
    /// final answer still earns one last try of the real allocation; any progress starts anew.
    fn peers(&mut self, device: i32, bytes: usize, physical: bool) -> Result<bool> {
        if !physical || self.peers_done {
            return Ok(false);
        }
        #[cfg(target_os = "linux")]
        {
            let limit = *self.deadline.get_or_insert_with(|| Instant::now() + RECOVERY_BUDGET);
            if crate::cooperative::recover(device, bytes, limit) == varena_ledger::Recovery::Retry {
                self.deadline = None;
            } else {
                self.peers_done = true;
            }
            Ok(true)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (device, bytes);
            Ok(false)
        }
    }
}

