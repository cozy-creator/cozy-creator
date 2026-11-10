//! Process-local CUDA memory. TensorFS supplies weight bytes; [`working`] supplies ordinary
//! allocations and prepared graph pools alongside stable-address weight residency.
//!
//! Every foreign storage must retain its [`Pin`] until the final alias is destroyed. Raw addresses
//! are not ownership. Kernels may finish after the final storage disappears, so reclamation joins
//! the primary CUDA context before unmapping unless every pin was released by a native integration
//! after checked completion, or with last-consumer events on its declared streams, which reclaim
//! waits on instead. Capture/replay roots retain pins for their lifetime by default.
//! Allocation paths import no GPU memory and invoke no Python callbacks. The optional Linux
//! `ledger` module coordinates host metadata only; it performs no CUDA or allocator work.
#![deny(unsafe_code)]

mod cuda;
mod fence;
pub(crate) mod reclaim;
pub use reclaim::set_host_lock_probe;
mod async_fill;
pub use async_fill::{FillDestination, FillFacts, FillOwners, FillResource, FillRetirement, FillSession, FillState, FillTicket};
pub mod source;
pub use source::Source;
/// Versioned public ABI. All frontends resolve this shared library.
pub mod abi;
pub mod plan;
mod retirement;
pub mod working;
pub(crate) mod tensor_consumers;
mod consumer_streams;
pub use consumer_streams::StreamUses;
pub use tensor_consumers::Consumers as TensorConsumers;
/// Preserve an unproven private storage Pin in the existing device retirement queue.
/// This performs no driver call, wait, retry, or reclamation credit.
#[doc(hidden)]
pub fn retain_unproven_pin(pin: Pin, failure: Error) {
    if pin.region().pid != std::process::id() {
        std::mem::forget(pin);
        return;
    }
    retirement::quarantine(retirement::Job::Pin { _pin: pin, failure: failure.clone() }, &failure);
}
pub mod cache_owner;
pub use cache_owner::CacheOwner;
pub use working::{Cache, CacheStats};
#[cfg(target_os = "linux")]
mod cooperative;
#[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
pub mod fork_guard;

use fence::{Fences, Waits};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use working::State;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("CUDA capacity exhausted allocating {requested_bytes} bytes")]
    OutOfMemory { requested_bytes: usize },
    /// A capture's allocation outside its pool found no free block, and a capture never maps.
    #[error("a CUDA capture cannot map {requested_bytes} bytes: no free block outside its pool")]
    CaptureMap { requested_bytes: usize },
    #[error("CUDA split capacity exhausted: required {requested_bytes} bytes, available {available_bytes} bytes")]
    Exhausted { requested_bytes: usize, available_bytes: usize },
    #[error("CUDA device fault in {operation}: CUresult {code}")]
    DeviceFault { operation: &'static str, code: i32 },
    #[error("CUDA driver failure in {operation}: CUresult {code}")]
    Cuda { operation: &'static str, code: i32 },
    #[error("device poisoned; reservations retained: {0}")]
    Poisoned(String),
    #[error("region is already being filled")]
    Busy,
    #[error("residency changes are forbidden during CUDA graph capture")]
    CaptureActive,
    #[error("execution phases cannot overlap on one device")]
    PhaseOverlap,
    #[error("invalid residency operation: {0}")]
    Invalid(&'static str),
    #[error("CUDA virtual memory unavailable: {0}")]
    Unsupported(String),
    #[error("cache owner callback failed: {0}")]
    Callback(String),
    /// A Source's own failure: its ABI status, its words and, for capacity, the bytes it asked.
    #[error("{detail}")]
    Source { status: i32, detail: String, bytes: u64 },
}

/// Private admission cause: another process cannot repair a local cap or VA refusal.
pub(crate) enum AllocationFailure {
    DriverOutOfMemory(usize),
    Other(Error),
}

pub(crate) type AllocationResult<T> = std::result::Result<T, AllocationFailure>;

impl AllocationFailure {
    fn physical_bytes(&self) -> Option<usize> {
        match self {
            Self::DriverOutOfMemory(bytes) => Some(*bytes),
            Self::Other(_) => None,
        }
    }

    fn is_oom(&self) -> bool {
        matches!(self, Self::DriverOutOfMemory(_)
            | Self::Other(Error::OutOfMemory { .. } | Error::Exhausted { .. }))
    }
}

impl From<Error> for AllocationFailure {
    fn from(error: Error) -> Self { Self::Other(error) }
}

impl From<AllocationFailure> for Error {
    fn from(failure: AllocationFailure) -> Self {
        match failure {
            AllocationFailure::DriverOutOfMemory(requested_bytes) => Self::OutOfMemory { requested_bytes },
            AllocationFailure::Other(error) => error,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub reserved_bytes: usize,
    pub resident_bytes: usize,
    pub ready_bytes: usize,
    pub pinned_bytes: usize,
    pub regions: usize,
    pub fills: u64,
    /// Bytes every completed fill wrote: host-to-device traffic.
    pub filled_bytes: u64,
    pub hits: u64,
    pub evictions: u64,
    pub reclaimed_bytes: u64,
    /// Prefetched regions evicted before any use: read for nothing.
    pub prefetch_unused_bytes: u64,
    pub reclaimed_by_reason: ReclaimedBytes,
    /// This device's cuCtxSynchronize calls.
    pub context_joins: u64,
    /// This device's cuMemCreate refusals, excluding local room and VA limits.
    pub driver_physical_ooms: u64,
    /// This device's cuEventSynchronize calls.
    pub event_waits: u64,
    pub poisoned: bool,
}

/// Cumulative successfully released bytes, classified by the entry point that reclaimed them.
/// Allocator recovery includes local allocation ceilings as well as driver capacity failures;
/// these counters describe the trigger, not physical transfer traffic or time spent waiting.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReclaimedBytes {
    pub allocator: u64,
    pub room: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum ReclaimReason {
    Allocator,
    Room,
}

impl ReclaimedBytes {
    fn add(&mut self, reason: ReclaimReason, bytes: usize) {
        let counter = match reason {
            ReclaimReason::Allocator => &mut self.allocator,
            ReclaimReason::Room => &mut self.room,
        };
        *counter = counter.saturating_add(bytes as u64);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReclaimReport {
    pub requested_bytes: usize,
    pub reclaimed_bytes: usize,
    pub evicted_regions: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Contents {
    Cold,
    Loading,
    Ready,
    Retired,
    Published,
}

struct Uses {
    pins: AtomicUsize,
    contents: AtomicU8,
    // Ordinary foreign storage may disappear before its submitted CUDA work completes.
    // A context join that starts at or after this device join number covers the release.
    // Zero: only a native caller with a checked last-consumer fence releases without it.
    unjoined: AtomicU64,
    // Last-consumer stamps, written before the pin decrement.
    waits: Mutex<Waits>,
    generation: AtomicU64,
    submission: Mutex<Option<Arc<async_fill::Control>>>,
    // Filled ahead, evicted unused: no second guess until a use asks for it.
    unused: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Completion {
    Done,
    Pending,
    Unjoined,
}

impl Uses {
    fn new() -> Self {
        Self {
            pins: AtomicUsize::new(0),
            contents: AtomicU8::new(Contents::Cold as u8),
            unjoined: AtomicU64::new(0),
            waits: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
            submission: Mutex::new(None),
            unused: AtomicBool::new(false),
        }
    }
    fn waits(&self) -> MutexGuard<'_, Waits> {
        self.waits.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn state(&self) -> Contents {
        match self.contents.load(Ordering::Acquire) {
            0 => Contents::Cold,
            1 => Contents::Loading,
            2 => Contents::Ready,
            3 => Contents::Retired,
            4 => Contents::Published,
            _ => unreachable!("native contents state"),
        }
    }
    fn set(&self, state: Contents) {
        self.contents.store(state as u8, Ordering::Release);
    }
    fn invalidate(&self) {
        // Final Region retirement may happen without the arena lock. Never replace that
        // terminal state when reclaim invalidates formerly ready physical contents.
        let _ = self.contents.compare_exchange(
            Contents::Ready as u8,
            Contents::Cold as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    fn pins(&self) -> usize {
        self.pins.load(Ordering::Acquire)
    }
}

impl Region {
    /// One pin fewer; the last one leaves the held bytes.
    fn unpinned(&self) {
        let prior = self.uses.pins.try_update(Ordering::AcqRel, Ordering::Acquire, |value| value.checked_sub(1));
        assert!(prior.is_ok(), "native owner count underflow");
        if prior == Ok(1) {
            self.gpu.held.fetch_sub(self.rounded, Ordering::AcqRel);
        }
    }
}

fn decrement(counter: &AtomicUsize) {
    assert!(
        counter
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| value
                .checked_sub(1))
            .is_ok(),
        "native owner count underflow"
    );
}

struct Entry {
    ptr: u64,
    bytes: usize,
    handle: Option<u64>,
    uses: Arc<Uses>,
    region: Weak<Region>,
    // The Region used right after this one, last time.
    next: Option<u64>,
    // Clock ticks between its last two uses.
    period: u64,
    // Filled ahead of demand and not yet used: the first to go.
    prefetched: bool,
    last_use: u64,
    // The clock when its contents were filled: one filled this pass is what the loop streams.
    filled: u64,
    // Its owner ranks it above rebuilding a cache: `reclaim::HIGH`.
    high: bool,
    // State a loop keeps across steps, refilled from its pinned host copy.
    retained: bool,
}

impl Entry {
    fn idle(&self) -> bool {
        self.uses.pins() == 0
            && self.handle.is_some()
            && !matches!(self.uses.state(), Contents::Loading | Contents::Published)
    }
}

impl State {
    /// A use of this region (a clone of a live pin is not one). The core finds loops by itself,
    /// phase declared or not. A region back
    /// again with no boundary yet began a loop at its last use: from there eviction goes most
    /// recent first, keeping what the loop reaches next. One that recurs at a steady period
    /// narrows that to one pass, so what ran before the loop goes first; a small loop inside a
    /// larger one, which would leave most of the pass's bytes outside, does not.
    fn used(&mut self, id: u64, now: u64) {
        if let Some(last) = self.last_used.replace(id).filter(|last| *last != id)
            && let Some(entry) = self.entries.get_mut(&last)
        {
            entry.next = Some(id);
        }
        let entry = self.entries.get_mut(&id).expect("live region");
        entry.uses.unused.store(false, Ordering::Relaxed);
        let (previous, period) = (entry.last_use, entry.period);
        entry.last_use = now;
        if previous == 0 {
            return;
        }
        let interval = now - previous;
        entry.period = interval;
        if let Some(phase) = &mut self.plan_phase && previous > phase.entered {
            phase.recurred = true;
        }
        let start = previous - 1;
        let Some(boundary) = self.phase_boundary else {
            self.phase_boundary = Some(start);
            return;
        };
        if start > boundary && interval.abs_diff(period) * 4 <= interval {
            let recent = |since: u64| -> usize {
                self.entries.values().filter(|entry| entry.last_use > since).map(|entry| entry.bytes).sum()
            };
            if recent(start) * 2 >= recent(boundary) {
                self.phase_boundary = Some(start);
            }
        }
    }

    /// Where a planned loop's kept prefix ends: the latest plan position of a region that stayed
    /// through the pass with more bytes kept than streamed up to it. None in a loop's first pass,
    /// or with no plan. A region kept far past the streamed ones does not stretch it.
    pub(crate) fn prefix(&self, order: &plan::Order) -> Option<u64> {
        let boundary = self.phase_boundary?;
        if !self.plan_phase.as_ref().is_none_or(PlanPhase::loops) {
            return None;
        }
        let mut regions: Vec<(u64, usize, bool)> = self.entries.iter()
            .filter(|(_, entry)| entry.last_use > boundary && !entry.retained && !entry.high && !entry.prefetched
                && !matches!(entry.uses.state(), Contents::Cold | Contents::Retired))
            .filter_map(|(id, entry)| match order.next_use(*id) {
                Some((0, position)) => Some((position, entry.bytes, entry.filled <= boundary)),
                _ => None,
            })
            .collect();
        regions.sort_unstable();
        let (mut kept, mut streamed, mut end) = (0usize, 0usize, None);
        for (position, bytes, stayed) in regions {
            if stayed {
                kept += bytes;
                if kept > streamed {
                    end = Some(position);
                }
            } else {
                streamed += bytes;
            }
        }
        end
    }

    /// Eviction order for idle regions, lowest first. With a plan, the furthest next use goes
    /// first; in a phase's loop that is most recently used first, as the loop reaches its
    /// oldest use next, and of those what this pass filled before what stayed through it: the
    /// loop streams the same regions each pass, so the bytes it reads back fit the page cache.
    /// Taking whatever finished last let the resident set wander, and on a 26 GB cgroup Flux-2
    /// read all 57 blocks back from disk in turn (24 GB a step against v1's 13).
    ///
    /// A planned loop keeps a prefix of its plan, as v1 did: what it refilled inside the prefix
    /// (`prefix`) is the prefix's again, and the prefix gives way from its end, the region the next
    /// pass needs last. By fill alone, a region another phase had evicted was streamed every pass
    /// after: Klein's upsampler refilled its 1.9 GB embedding each token, 6.2 GB a pass to v1's
    /// 4.7 (pasan, run 42322).
    fn rank(&self, id: u64, entry: &Entry, order: &plan::Order, prefix: Option<u64>) -> (u8, u64) {
        if matches!(entry.uses.state(), Contents::Cold | Contents::Retired) {
            return (reclaim::CONTENTLESS, entry.last_use);
        }
        if entry.high {
            return (reclaim::HIGH, entry.last_use);
        }
        if entry.prefetched {
            // Of unused fills, what the plan needs last goes first: the nearest is about to be used.
            let next = order.next_use(id).map_or(0, |(phases, position)| phases << 32 | position);
            return (reclaim::PREFETCHED, u64::MAX - next);
        }
        let touched = self.phase_boundary.is_some_and(|boundary| entry.last_use > boundary);
        let looping = self.plan_phase.as_ref().is_none_or(PlanPhase::loops);
        let used = match self.phase_boundary.is_some_and(|boundary| entry.filled > boundary) {
            true => reclaim::PHASE_FRESH,
            false => reclaim::PHASE_USED,
        };
        // A loop's retained state is its own, read again next step like its weights: most recent
        // first, with what this pass streamed, as its refill is one DMA from pinned host RAM.
        if entry.retained && touched {
            return (reclaim::PHASE_FRESH, u64::MAX - entry.last_use);
        }
        match order.next_use(id) {
            None if order.planned() || !touched => (reclaim::UNPLANNED, entry.last_use),
            None => (used, u64::MAX - entry.last_use),
            Some((0, position)) if touched && looping => match prefix {
                Some(end) if used == reclaim::PHASE_USED || position < end => (reclaim::PHASE_USED, u64::MAX - position),
                _ => (used, u64::MAX - entry.last_use),
            },
            Some((0, position)) if !touched => (reclaim::PHASE_NEXT, u64::MAX - position),
            // A single pass uses what it used next in a later phase, else next cycle, as late as what no
            // plan names; most recent first, so the next pass finds its start. Ranked as a loop's, Qwen
            // Turbo's text encoder evicted 4 GB of transformer a call, read back in step 1.
            Some((0, _)) => order.later_use(id)
                .map_or((reclaim::UNPLANNED, u64::MAX - entry.last_use), |later| self.later(order, later)),
            Some(next) => self.later(order, next),
        }
    }

    /// Another phase's region, furthest first, and a loop's after every single pass's: a single pass
    /// reads what doesn't fit once a cycle whatever goes, while a loop that stays never reads again.
    fn later(&self, order: &plan::Order, (phases, position): (u64, u64)) -> (u8, u64) {
        let single = order.name(phases).is_some_and(|name| self.passes.get(name) == Some(&false));
        (reclaim::LATER_PHASE, u64::from(!single) << 63 | ((u64::MAX >> 1) - (phases << 32 | position)))
    }
}

/// Make this GPU usable by this process: its context, and its row in the GPU's ledger.
/// Idempotent; the first caller's `shared` holds (true: yield idle memory to foreign programs).
pub fn activate(device: i32, shared: bool) -> Result<()> {
    check_process()?;
    if device < 0 {
        return Err(Error::Invalid("device must be nonnegative"));
    }
    #[cfg(target_os = "linux")]
    cooperative::join(device, shared)?;
    #[cfg(not(target_os = "linux"))]
    { let _ = shared; working::initialize(device)?; }
    working::mark_cooperation_bound(device)
}

/// A request started or ended on `device`, which orders who donates first. The
/// first one running here is also a phase boundary, for eager callers too: the last call's regions
/// are no longer in this one's loop, so they go first, by last use (hidream-o1's generate kept its
/// turbo streaming in the same process, run 39070).
pub fn request_boundary(device: i32, begin: bool) {
    if let Ok(device) = gpu(device) {
        if !begin {
            let _ = device.requests.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        } else {
            device.held_peak.store(0, Ordering::Release);
            if device.requests.fetch_add(1, Ordering::AcqRel) == 0 && let Ok(mut state) = device.lock() {
                state.phase_boundary = Some(device.clock.load(Ordering::Relaxed));
            }
        }
    }
    #[cfg(target_os = "linux")]
    cooperative::request_boundary(device, begin);
    #[cfg(not(target_os = "linux"))]
    let _ = (device, begin);
}

/// The active plan phase. One that ran through last time with no region coming back is a single
/// pass: its used weights are wanted again only in a later phase, a loop's in its next step.
pub(crate) struct PlanPhase {
    name: String,
    entered: u64,
    // A region came back since entry.
    recurred: bool,
    // Whether it looped last time.
    looped: Option<bool>,
}

impl PlanPhase {
    /// For eviction: unless it is known to be a single pass.
    fn loops(&self) -> bool {
        self.recurred || self.looped != Some(false)
    }

    /// For keeping working pages: once seen looping.
    pub(crate) fn keeps(&self) -> bool {
        self.recurred || self.looped == Some(true)
    }
}

/// What the driver's own charge may grow by between samples under a development cap.
const CAP_SLACK: usize = 128 << 20;

// One process/core-wide namespace for reservation and Plan lifetimes. Never use
// addresses or these identities in compute fingerprints or reusable measurements.
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

fn next_identity() -> Result<u64> {
    check_process()?;
    NEXT_IDENTITY.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
        .map_err(|_| Error::Invalid("native object identity exhausted"))
}

/// One GPU's memory authority, for the process lifetime: every byte this process holds on the
/// card is in its one table, and only `reclaim`'s one sequence gives bytes up. Different GPUs
/// share none of it.
pub struct Device {
    ordinal: i32,
    gate: Mutex<()>,
    state: Mutex<State>,
    fences: Mutex<Fences>,
    driver: OnceLock<Arc<cuda::Driver>>,
    // Nonzero once this device's working address space is reserved.
    page: AtomicUsize,
    activity_epoch: AtomicU64,
    external_callbacks: AtomicUsize,
    captures: AtomicUsize,
    replays: AtomicUsize,
    producers: AtomicUsize,
    retirements: AtomicUsize,
    /// `set_limit`: the card as a smaller card would be, the driver's own charge included.
    ceiling: AtomicUsize,
    /// The card itself, once sampled: its driver's refusal is never how we learn it is full.
    total: AtomicUsize,
    /// A real card of `ceiling`'s size would keep its driver's reservation out of it too.
    reservation: OnceLock<usize>,
    /// That charge at the last sample: what the card uses beyond our own bytes.
    outside: AtomicUsize,
    /// The least of it any sample read: this process's own context and what was there before it.
    /// What `outside` holds past it came since, and can go again (a game's, a peer's).
    outside_floor: AtomicUsize,
    pages: AtomicUsize,
    working: AtomicUsize,
    // The most weight bytes held for use at once this request.
    held_peak: AtomicUsize,
    // Requests running here: the first to begin is a phase boundary.
    requests: AtomicUsize,
    // Weight bytes held for use now: regions with a pin.
    held: AtomicUsize,
    // Moves when the room this process can have shrinks for a reason outside its program: a cap,
    // foreign use, a refused allocation or a peer's ask. A split's chosen level holds until it does.
    room: AtomicU64,
    // Of those moves, the ones growing outside use made at a sample.
    room_outside: AtomicU64,
    // A region here was ever marked high. Until then the reclaim sequence has no such step.
    any_high: AtomicBool,
    // Actual successful physical releases only. An observation permits a real retry, never a grant.
    released: AtomicU64,
    clock: AtomicU64,
    joins_started: AtomicU64,
    joins_finished: AtomicU64,
    plan: Mutex<Option<plan::Active>>,
    plan_generation: AtomicU64,
    cooperation_bound: AtomicBool,
    ahead: source::Ahead,
}

impl Device {
    fn new(ordinal: i32) -> Self {
        Self {
            ordinal,
            gate: Mutex::default(),
            state: Mutex::default(),
            fences: Mutex::default(),
            driver: OnceLock::new(),
            page: AtomicUsize::new(0),
            activity_epoch: AtomicU64::new(0),
            external_callbacks: AtomicUsize::new(0),
            captures: AtomicUsize::new(0),
            replays: AtomicUsize::new(0),
            producers: AtomicUsize::new(0),
            retirements: AtomicUsize::new(0),
            ceiling: AtomicUsize::new(usize::MAX),
            total: AtomicUsize::new(usize::MAX),
            reservation: OnceLock::new(),
            outside: AtomicUsize::new(0),
            outside_floor: AtomicUsize::new(usize::MAX),
            pages: AtomicUsize::new(0),
            working: AtomicUsize::new(0),
            held_peak: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            held: AtomicUsize::new(0),
            room: AtomicU64::new(0),
            room_outside: AtomicU64::new(0),
            any_high: AtomicBool::new(false),
            released: AtomicU64::new(0),
            clock: AtomicU64::new(0),
            joins_started: AtomicU64::new(0),
            joins_finished: AtomicU64::new(0),
            plan: Mutex::new(None),
            plan_generation: AtomicU64::new(0),
            cooperation_bound: AtomicBool::new(false),
            ahead: source::Ahead::default(),
        }
    }

    /// This GPU, activated: its driver open and its working address space reserved.
    pub fn open(ordinal: i32) -> Result<&'static Self> {
        retirement::prepare()?;
        working::open(ordinal)
    }

    pub fn ordinal(&self) -> i32 {
        self.ordinal
    }

    fn driver(&self) -> &Arc<cuda::Driver> {
        self.driver.get().expect("an opened device")
    }

    pub fn granularity(&self) -> usize {
        self.driver().granularity
    }

    fn epoch(&self) -> u64 {
        self.activity_epoch.load(Ordering::Acquire)
    }

    fn graph_active(&self) -> bool {
        self.captures.load(Ordering::Acquire) != 0 || self.replays.load(Ordering::Acquire) != 0
    }

    pub(crate) fn fences(&self) -> MutexGuard<'_, Fences> {
        self.fences.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Poisoned("device table mutex".into()))?;
        if let Some(reason) = &state.poison {
            return Err(Error::Poisoned(reason.clone()));
        }
        Ok(state)
    }

    fn next_use(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// A release with unknown consumers: only a join started after this call covers it.
    fn unjoined_mark(&self) -> u64 {
        self.joins_started.load(Ordering::Acquire) + 1
    }

    fn joined(&self, mark: u64) -> bool {
        mark <= self.joins_finished.load(Ordering::Acquire)
    }

    fn guard(&'static self, count: &'static AtomicUsize) -> Result<GlobalCaptureGuard> {
        count.try_update(Ordering::AcqRel, Ordering::Acquire, |value| value.checked_add(1))
            .map_err(|_| Error::Invalid("device activity count overflow"))?;
        Ok(GlobalCaptureGuard { count, pid: std::process::id() })
    }

    fn owned(&self) -> usize {
        self.pages.load(Ordering::Acquire).saturating_add(self.working.load(Ordering::Acquire))
    }

    /// The most our own bytes may reach right now.
    fn capacity(&self) -> usize {
        let reserved = self.reservation.get().copied().unwrap_or(0);
        let ceiling = self.ceiling.load(Ordering::Acquire);
        let ceiling = if ceiling == usize::MAX { ceiling } else { ceiling.saturating_sub(reserved) };
        let ceiling = ceiling.min(self.total.load(Ordering::Acquire));
        let cap = if ceiling == usize::MAX {
            usize::MAX
        } else {
            ceiling.saturating_sub(self.outside.load(Ordering::Acquire).saturating_add(CAP_SLACK))
        };
        cap
    }

    fn headroom(&self) -> usize {
        self.capacity().saturating_sub(self.owned())
    }

    /// Bytes `more` would put over the limit.
    fn over(&self, more: usize) -> usize {
        self.owned().saturating_add(more).saturating_sub(self.capacity())
    }

    /// What this process has no say over moves as contexts and kernels load and peers come and
    /// go. A slow path calls this before it reads the room; outside capture, in the context.
    fn sample(&self) -> Result<()> {
        let (free, total) = self.driver().memory_info()?;
        self.total.store(total, Ordering::Release);
        if self.ceiling.load(Ordering::Acquire) != usize::MAX {
            #[cfg(target_os = "linux")]
            self.reservation.get_or_init(|| cooperative::driver_reservation(self.ordinal, total));
        }
        let outside = total.saturating_sub(free).saturating_sub(self.owned());
        self.outside_floor.fetch_min(outside, Ordering::AcqRel);
        if outside > self.outside.swap(outside, Ordering::AcqRel) {
            self.room_outside.fetch_add(1, Ordering::Relaxed);
            self.room_shrank();
        }
        Ok(())
    }

    /// The room this process can have shrank for a reason outside its program.
    fn room_shrank(&self) {
        self.room.fetch_add(1, Ordering::AcqRel);
    }

    /// Which room a decision was made against: it changes only when that room shrinks.
    fn room_epoch(&self) -> u64 {
        self.room.load(Ordering::Acquire) + self.driver.get().map_or(0, |driver| driver.physical_ooms())
    }

    /// Other processes' bytes stand between us and `more`, so asking them can help.
    fn peers_can_lift(&self, more: usize) -> bool {
        let wanted = self.owned().saturating_add(more);
        wanted <= self.capacity().saturating_add(self.outside.load(Ordering::Acquire))
    }

    /// A request runs here (`request_boundary`): what is not one, idle work, waits for nothing.
    fn in_request(&self) -> bool {
        self.requests.load(Ordering::Acquire) != 0
    }

    /// A limit someone set on the machine.
    fn capped(&self) -> bool {
        self.ceiling.load(Ordering::Acquire) != usize::MAX
    }

    /// Physical pages for one region, inside this process's room: over the room reads as the
    /// driver's out of memory, so each admission treats a smaller room as a smaller card.
    fn create(&self, bytes: usize) -> AllocationResult<u64> {
        self.sample()?;
        if self.over(bytes) != 0 {
            return Err(Error::OutOfMemory { requested_bytes: bytes }.into());
        }
        let handle = self.driver().create(bytes)?;
        self.pages.fetch_add(bytes, Ordering::AcqRel);
        Ok(handle)
    }

    /// A Region of `bytes` from `source`: address space only, no physical memory. One eviction
    /// group. `use_on` fills it.
    pub fn region(&'static self, bytes: usize, source: Arc<dyn Source>) -> Result<Arc<Region>> {
        if bytes == 0 {
            return Err(Error::Invalid("zero-length region"));
        }
        let granularity = self.granularity();
        let rounded = bytes
            .checked_add(granularity - 1)
            .ok_or(Error::Invalid("region size overflow"))?
            / granularity
            * granularity;
        let gate = gate(self.ordinal)?;
        gate.not_capturing()?;
        let mut state = self.lock()?;
        let id = next_identity()?;
        let _context = self.driver().enter()?;
        let ptr = self.driver().reserve(rounded)?;
        let uses = Arc::new(Uses::new());
        Ok(Arc::new_cyclic(|region| {
            state.entries.insert(
                id,
                Entry {
                    ptr,
                    bytes: rounded,
                    handle: None,
                    uses: Arc::clone(&uses),
                    region: region.clone(),
                    next: None,
                    period: 0,
                    prefetched: false,
                    last_use: 0,
                    filled: 0,
                    high: false,
                    retained: false,
                },
            );
            Region {
                gpu: self,
                pid: std::process::id(),
                id,
                ptr,
                bytes,
                rounded,
                uses,
                source,
            }
        }))
    }

    /// A new phase begins: what was used before it is no longer in its loop. Metadata only.
    pub fn enter_phase(&self) -> Result<()> {
        gate(self.ordinal)?.not_capturing()?;
        self.lock()?.phase_boundary = Some(self.clock.load(Ordering::Relaxed));
        Ok(())
    }

    /// The plan's phase `name` begins, a boundary, or with None the plan ends. Metadata only.
    fn enter_plan_phase(&self, name: Option<&str>) -> Result<()> {
        let mut state = self.lock()?;
        let now = self.clock.load(Ordering::Relaxed);
        if let Some(ended) = state.plan_phase.take() {
            state.passes.insert(ended.name, ended.recurred);
        }
        if let Some(name) = name {
            state.phase_boundary = Some(now);
            state.plan_epoch += 1;
            let looped = state.passes.get(name).copied();
            state.plan_phase = Some(PlanPhase { name: name.to_owned(), entered: now, recurred: false, looped });
        }
        Ok(())
    }

    /// As known at the last fence poll. No driver call.
    fn completion(&self, state: &State, id: u64) -> Completion {
        let uses = &state.entries[&id].uses;
        if !self.joined(uses.unjoined.load(Ordering::Acquire)) {
            return Completion::Unjoined;
        }
        if self.fences().complete(&uses.waits()) {
            Completion::Done
        } else {
            Completion::Pending
        }
    }

    /// Caller holds the gate with no capture active. A failed fence record or query poisons.
    fn poll_fences(&self, state: &mut State) -> Result<()> {
        let _context = self.driver().enter()?;
        let result = self.fences().poll(self.driver());
        result.map_err(|error| poison(state, error))
    }

    /// Unmap one completed, unpinned region, and release its pages, or move them to `into` when
    /// that is an absent region of its size. `completed` records the physical effect before any
    /// later fallible address retirement.
    fn evict(&self, state: &mut State, id: u64, reason: ReclaimReason, completed: &mut usize, into: Option<u64>) -> Result<()> {
        let prefetched = state.entries[&id].prefetched;
        let bytes = state.entries[&id].bytes;
        let into = into.filter(|to| state.entries.get(to).is_some_and(|to| to.handle.is_none() && to.bytes == bytes));
        let size = self.release_backing_counted(state, id, completed, into)?;
        if prefetched {
            state.prefetch_unused_bytes = state.prefetch_unused_bytes.saturating_add(size as u64);
            if let Some(entry) = state.entries.get(&id) {
                entry.uses.unused.store(true, Ordering::Relaxed);
            }
        }
        state.evictions = state.evictions.saturating_add(1);
        state.reclaimed_bytes = state.reclaimed_bytes.saturating_add(size as u64);
        state.reclaimed_by_reason.add(reason, size);
        Ok(())
    }

    /// Unmap a completed, unpinned region's pages and release them, or map them at `into`, an
    /// absent region of its size: a refill then costs a remap, never a release and a create. A
    /// retired region's address goes too.
    fn release_backing_counted(&self, state: &mut State, id: u64, completed: &mut usize, into: Option<u64>) -> Result<usize> {
        let entry = state.entries.get(&id).expect("candidate in the table");
        let (ptr, size, handle) = (entry.ptr, entry.bytes, entry.handle.expect("mapped candidate"));
        if let Err(error) = self.driver().unmap(ptr, size) {
            return Err(poison(state, error));
        }
        if let Some(to) = into {
            // Custody before map, as a fill records it: an ambiguous map failure keeps both.
            let to = state.entries.get_mut(&to).expect("live region");
            to.handle = Some(handle);
            let target = to.ptr;
            if let Err(error) = self.driver().map(target, size, handle) {
                return Err(poison(state, error));
            }
        // If release fails, do not retry or reuse either handle or reservation.
        } else if let Err(error) = self.driver().release(handle) {
            return Err(poison(state, error));
        } else {
            self.pages.fetch_sub(size, Ordering::AcqRel);
            self.released.fetch_add(size as u64, Ordering::AcqRel);
        }
        *completed += size;
        let entry = state.entries.get_mut(&id).expect("candidate in the table");
        entry.uses.waits().clear();
        entry.prefetched = false;
        entry.handle = None;
        entry.uses.invalidate();
        if entry.uses.state() == Contents::Retired {
            if let Err(error) = self.driver().free_address(ptr, size) {
                return Err(poison(state, error));
            }
            state.entries.remove(&id);
        }
        Ok(size)
    }

    pub fn stats(&self) -> Result<Stats> {
        let state = self.state.lock().map_err(|_| Error::Poisoned("device table mutex".into()))?;
        let (driver_physical_ooms, context_joins, event_waits) = self.driver.get()
            .map_or((0, 0, 0), |driver| (driver.physical_ooms(), driver.context_joins(), driver.calls().3));
        let mut stats = Stats {
            regions: state.entries.len(),
            fills: state.fills,
            filled_bytes: state.filled_bytes,
            hits: state.hits,
            evictions: state.evictions,
            reclaimed_bytes: state.reclaimed_bytes,
            prefetch_unused_bytes: state.prefetch_unused_bytes,
            reclaimed_by_reason: state.reclaimed_by_reason,
            context_joins,
            driver_physical_ooms,
            event_waits,
            poisoned: state.poison.is_some(),
            ..Stats::default()
        };
        for entry in state.entries.values() {
            stats.reserved_bytes += entry.bytes;
            if entry.handle.is_some() {
                stats.resident_bytes += entry.bytes;
                if entry.uses.state() == Contents::Ready {
                    stats.ready_bytes += entry.bytes;
                }
                if entry.uses.pins() != 0 {
                    stats.pinned_bytes += entry.bytes;
                }
            }
        }
        Ok(stats)
    }
}

const MAX_DEVICES: usize = 64;
static DEVICES: [OnceLock<Device>; MAX_DEVICES] = [const { OnceLock::new() }; MAX_DEVICES];
static PROCESS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn gpu(ordinal: i32) -> Result<&'static Device> {
    check_process()?;
    let slot = usize::try_from(ordinal).ok().and_then(|index| DEVICES.get(index))
        .ok_or(Error::Invalid("CUDA device ordinal out of range"))?;
    Ok(slot.get_or_init(|| Device::new(ordinal)))
}

/// Observed process-check backend. Reading this never initializes CUDA or arms a guard.
#[derive(Debug)]
pub struct ProcessGuardStatus {
    pub backend: &'static str,
    pub marker: u32,
    pub thread_authenticated: bool,
    pub page_address: usize,
    pub page_bytes: usize,
    pub failure_stage: u32,
    pub failure_errno: i32,
}

pub fn process_guard_status() -> ProcessGuardStatus {
    #[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
    {
        fork_guard::status()
    }
    #[cfg(not(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard"))))]
    {
        ProcessGuardStatus {
            backend: "actual_pid",
            marker: 0,
            thread_authenticated: false,
            page_address: 0,
            page_bytes: 0,
            failure_stage: 0,
            failure_errno: 0,
        }
    }
}

fn check_process() -> Result<()> {
    #[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
    if let Some(result) = fork_guard::check() {
        return result;
    }
    check_process_pid()
}

fn check_process_pid() -> Result<()> {
    let pid = std::process::id();
    let owner = match PROCESS.load(Ordering::Relaxed) {
        0 => PROCESS
            .compare_exchange(0, pid, Ordering::Relaxed, Ordering::Relaxed)
            .unwrap_or_else(|owner| owner),
        owner => owner,
    };
    if owner != 0 && owner != pid {
        return Err(Error::Invalid("CUDA residency cannot be used after fork"));
    }
    Ok(())
}

/// Excludes capture entry on this device. Lock order: gate, device table, fences,
/// region waits. Hold it across short driver calls, never across a GPU wait or a callback.
pub(crate) struct Gate {
    device: &'static Device,
    _held: MutexGuard<'static, ()>,
}

impl Gate {
    pub(crate) fn not_capturing(&self) -> Result<()> {
        if self.device.captures.load(Ordering::Acquire) != 0 {
            return Err(Error::CaptureActive);
        }
        Ok(())
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.device.epoch()
    }


    /// Keeps capture out while this thread waits on the GPU with the gate released.
    pub(crate) fn hold(&self) -> Result<ProducerGuard> {
        self.not_capturing()?;
        self.device.guard(&self.device.producers)
    }
}

pub(crate) fn gate(ordinal: i32) -> Result<Gate> {
    let device = gpu(ordinal)?;
    let held = device.gate.lock().map_err(|_| Error::Poisoned("device gate mutex".into()))?;
    Ok(Gate { device, _held: held })
}

/// Exclude driver work on this device while a capture runs, even before it is open.
/// Short-lived holders finish first. The bound is a last resort: a holder may be this very
/// thread, with no signal to tell. A refused capture falls back to eager.
pub fn enter_capture(device: i32) -> Result<GlobalCaptureGuard> {
    let deadline = std::time::Instant::now() + reclaim::RECOVERY_BUDGET;
    loop {
        source::stop(device)?;
        {
            let gate = gate(device)?;
            let state = gate.device;
            let producers = state.producers.load(Ordering::Acquire);
            if producers == 0
                && state.replays.load(Ordering::Acquire) == 0
                && state.retirements.load(Ordering::Acquire) == 0
            {
                state.activity_epoch.fetch_add(1, Ordering::AcqRel);
                return state.guard(&state.captures);
            }
            if std::time::Instant::now() >= deadline {
                return Err(if producers != 0 {
                    Error::Invalid("native producers must finish before CUDA graph capture")
                } else {
                    Error::CaptureActive
                });
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Bound a user callback's host interval. Only actual working alloc/free invalidates a
/// measurement; CPU-only callbacks add no failure and this guard performs no CUDA work.
pub fn enter_external_callback(device: i32) -> Result<GlobalCaptureGuard> {
    let state = gpu(device)?;
    state.guard(&state.external_callbacks)
}

/// A resident graph launch. Its retained Pins and pool references, not this host guard,
/// own its device dependencies, so reclaim continues beside it. Only capture entry waits.
pub fn enter_replay(device: i32) -> Result<GlobalCaptureGuard> {
    let gate = gate(device)?;
    gate.not_capturing()?;
    gate.device.activity_epoch.fetch_add(1, Ordering::AcqRel);
    gate.device.guard(&gate.device.replays)
}

pub fn is_capturing(device: i32) -> Result<bool> {
    Ok(gpu(device)?.captures.load(Ordering::Acquire) != 0)
}

/// Capture entry waits for a final owner's destruction, without this thread holding the
/// gate that destruction may need. Replay and recovery continue beside it.
fn enter_retirement(device: i32) -> Result<GlobalCaptureGuard> {
    let gate = gate(device)?;
    gate.not_capturing()?;
    gate.device.guard(&gate.device.retirements)
}

pub struct GlobalCaptureGuard {
    count: &'static AtomicUsize,
    pid: u32,
}

impl Drop for GlobalCaptureGuard {
    fn drop(&mut self) {
        if self.pid == std::process::id() {
            decrement(self.count);
        }
    }
}

/// Own the right to submit native producer work outside actual CUDA capture.
/// The caller retains this lease until every producer has checked completion; uncertain
/// completion retains the lease along with the actual Fill/source owners. Graph replay
/// is unaffected because its dependencies have separate physical pins.
pub fn enter_producer(device: i32) -> Result<ProducerGuard> {
    gate(device)?.hold()
}

pub type ProducerGuard = GlobalCaptureGuard;

fn poison(state: &mut State, error: Error) -> Error {
    state.poison = Some(error.to_string());
    error
}

/// A stable reservation. Dropping Runtime's views leaves this region's physical cache intact.
pub struct Region {
    gpu: &'static Device,
    pid: u32,
    id: u64,
    ptr: u64,
    bytes: usize,
    rounded: usize,
    uses: Arc<Uses>,
    source: Arc<dyn Source>,
}

/// Final reservation ownership with no remaining public views or handles.
pub struct RetiredRegion {
    _region: Region,
}

impl Region {
    /// Never-reused process/core-wide reservation identity; independent of its VA.
    pub fn id(&self) -> u64 { self.id }

    // Caller holds the gate.
    fn retire_intent(&self) {
        self.uses.set(Contents::Retired);
    }

    pub fn retire(self) -> RetiredRegion {
        if self.pid == std::process::id() {
            // Frontends may call this while holding their GIL/owner lock, before
            // handing RetiredRegion to deferred cleanup. Never wait for the gate here;
            // the eventual Drop linearizes retirement.
            if let Ok(_gate) = self.gpu.gate.try_lock() {
                self.retire_intent();
            }
        }
        RetiredRegion { _region: self }
    }
    /// Refuse further use when a native integration cannot prove completion of its writers.
    /// The integration must also retain its actual Fill/source owners; this is not cleanup.
    pub fn poison(&self, detail: impl Into<String>) {
        if self.pid != std::process::id() {
            return;
        }
        let mut state = self.gpu.state.lock().unwrap_or_else(|e| e.into_inner());
        state.poison.get_or_insert_with(|| detail.into());
    }

    /// Stamp this region's last consumers. No driver call, gate or wait.
    /// Whatever the contents: an abandoned fill's copy runs on its declared stream all the same.
    fn stamp_consumers(&self, streams: &[u64]) -> Result<()> {
        if streams.is_empty() {
            return Err(Error::Invalid("release has no recordable consumer event"));
        }
        let mut fences = self.gpu.fences();
        let mut waits = self.uses.waits();
        for &stream in streams {
            fences.stamp(stream, &mut waits);
        }
        Ok(())
    }

    fn mark_unjoined(&self) {
        self.uses.unjoined.store(self.gpu.unjoined_mark(), Ordering::Release);
    }

    pub fn ptr(&self) -> u64 {
        self.ptr
    }
    pub fn len(&self) -> usize {
        self.bytes
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn allocated_bytes(&self) -> usize {
        self.rounded
    }
    pub fn device(&self) -> i32 {
        self.gpu.ordinal
    }

    pub fn is_resident(&self) -> Result<bool> {
        Ok(self.gpu.lock()?
            .entries
            .get(&self.id)
            .expect("live region")
            .uses
            .state()
            == Contents::Ready)
    }

    /// Release this region's pages now if nothing needs them: no pin, no contents, and every
    /// user known complete (a fill aborted after its writers were checked). `CaptureActive`
    /// during a capture; while an earlier release is unfenced they stay (false) for
    /// reclamation, which takes them before any Ready page.
    pub fn release_unready(&self) -> Result<bool> {
        let gate = gate(self.gpu.ordinal)?;
        gate.not_capturing()?;
        let mut state = self.gpu.lock()?;
        self.gpu.poll_fences(&mut state)?;
        let Some(entry) = state.entries.get(&self.id) else {
            return Ok(false);
        };
        if entry.handle.is_none()
            || entry.uses.pins() != 0
            || !matches!(entry.uses.state(), Contents::Cold | Contents::Retired)
            || self.gpu.completion(&state, self.id) != Completion::Done
        {
            return Ok(false);
        }
        let _context = self.gpu.driver().enter()?;
        self.gpu.release_backing_counted(&mut state, self.id, &mut 0, None)?;
        Ok(true)
    }

    /// Whether its bytes are on the card, as far as an atomic read can tell.
    pub fn holds_contents(&self) -> bool {
        matches!(self.uses.state(), Contents::Ready | Contents::Published | Contents::Loading)
    }

    /// High: idle, it goes only once owner caches and peers were asked. Metadata only.
    pub fn set_high(&self, high: bool) -> Result<()> {
        self.gpu.lock()?.entries.get_mut(&self.id).expect("live region").high = high;
        self.gpu.any_high.fetch_or(high, Ordering::AcqRel);
        Ok(())
    }

    /// State a loop keeps across steps: once used in the loop it ranks as the loop's own.
    /// Metadata only.
    pub fn set_retained(&self) -> Result<()> {
        self.gpu.lock()?.entries.get_mut(&self.id).expect("live region").retained = true;
        Ok(())
    }

    /// This fill is not the region's first: it has been evicted and read back.
    pub fn refilled(&self) -> bool {
        self.uses.generation.load(Ordering::Acquire) > 1
    }

    /// Demand: anything idle may go to make room, in the one reclaim order.
    fn prepare(self: &Arc<Self>) -> Result<Preparation> {
        let protected = std::collections::BTreeSet::from([self.id]);
        let protect = reclaim::Protect {
            regions: Some(&protected), keep_phase: true, into: Some(self.id), outside: true, ..Default::default()
        };
        let result = (|| loop {
            let prepared = reclaim::acquire(self.gpu.ordinal, self.rounded, reclaim::Wait::Yes, protect, || {
                let gate = gate(self.gpu.ordinal)?;
                let mut state = self.gpu.lock()?;
                self.prepare_locked(&gate, &mut state, false)
            })?;
            match prepared {
                Some(prepared) => return Ok(prepared),
                None => reclaim::settle(self)?,
            }
        })();
        // `Busy` is a fill already running, which `use_on` waits for: nothing was refused.
        if result.as_ref().is_err_and(|error| !matches!(error, Error::Busy)) {
            let _ = working::invalidate_current_phase(self.gpu.ordinal, "weight_admission_error");
        }
        result
    }

    /// Pin this Region for `consumer`, which orders its next work behind the bytes, calling its
    /// Source if they are absent. The one way a use gets them.
    ///
    /// # Safety
    /// `consumer` is live in this primary context and reads nothing before this returns.
    #[allow(unsafe_code)]
    pub unsafe fn use_on(self: &Arc<Self>, consumer: u64) -> Result<Pin> {
        let pin = loop {
            // SAFETY: this method's own contract.
            match unsafe { self.prepare_on(consumer) } {
                Ok(async_fill::StreamPreparation::Ready(pin) | async_fill::StreamPreparation::Ordered(pin)) => break pin,
                Ok(async_fill::StreamPreparation::Fill(fill)) => {
                    if self.source.reads() {
                        source::yield_to(self);
                    }
                    // The worker takes up what comes next while this thread reads: asked only after the
                    // read, it lost the race to this thread's next use every time.
                    let _ = source::after_use(self);
                    break self.source.fill(fill, Some(consumer))?
                        .ok_or(Error::Invalid("a Source returned no pin to its consumer"))?;
                }
                Err(Error::Busy) if source::wait_for(self) => {}
                Err(error) => return Err(error),
            }
        };
        let _ = source::after_use(self); // Only ever a guess: its failure is not this use's.
        Ok(pin)
    }

    /// A fill ahead of demand, if the rule allows (`reclaim::prefetch_room`). A wrong guess costs
    /// a read, never a refusal: None declines without IO or waiting.
    #[allow(unsafe_code)]
    fn prepare_ahead(self: &Arc<Self>, free_only: bool) -> Result<Option<Fill>> {
        let (device, arena) = (self.device(), self.gpu);
        let missing = {
            let gate = gate(device)?;
            if gate.not_capturing().is_err() {
                return Ok(None);
            }
            let mut state = arena.lock()?;
            arena.poll_fences(&mut state)?;
            let entry = state.entries.get(&self.id).expect("live region");
            if entry.uses.state() != Contents::Cold || entry.uses.pins() != 0
                || (entry.handle.is_some() && arena.completion(&state, self.id) != Completion::Done)
            {
                return Ok(None);
            }
            if entry.handle.is_none() { self.rounded } else { 0 }
        };
        if !reclaim::prefetch_room(device, missing, &std::collections::BTreeSet::from([self.id]), free_only)? {
            working::prefetch_declined(device)?;
            return Ok(None);
        }
        let gate = gate(device)?;
        let mut state = arena.lock()?;
        let fill = match self.prepare_locked(&gate, &mut state, true) {
            Ok(Some(Preparation::Fill(fill))) => fill,
            // SAFETY: this temporary pin was never handed to a consumer.
            Ok(Some(Preparation::Resident(pin))) => { drop(unsafe { pin.into_completed_region() }); return Ok(None) }
            Ok(None) => return Ok(None),
            Err(failure) if failure.is_oom() => return Ok(None),
            Err(failure) => return match Error::from(failure) {
                Error::Busy | Error::CaptureActive => Ok(None),
                // Ambiguous device faults retain tracked mappings in the poisoned arena.
                error => Err(error),
            },
        };
        drop(state);
        drop(gate);
        working::prefetch_admitted(device)?;
        Ok(Some(fill))
    }

    /// One attempt, with no reclaim. None: this region's own earlier users have not completed.
    fn prepare_locked(
        self: &Arc<Self>,
        gate: &Gate,
        state: &mut State,
        prefetched: bool,
    ) -> AllocationResult<Option<Preparation>> {
        let entry = state.entries.get(&self.id).expect("live region");
        if matches!(entry.uses.state(), Contents::Loading | Contents::Published) {
            return Err(Error::Busy.into());
        }
        if entry.uses.state() == Contents::Ready {
            state.hits = state.hits.saturating_add(1);
            state.used(self.id, self.gpu.next_use());
            return Ok(Some(Preparation::Resident(self.pin_locked(state)?)));
        }
        // A failed fill can leave escaped foreign storage. It owns these bytes until destruction.
        if entry.uses.pins() != 0 {
            return Err(Error::Busy.into());
        }
        gate.not_capturing()?;
        let _context = self.gpu.driver().enter()?;
        if entry.handle.is_none() {
            let handle = match self.gpu.create(self.rounded) {
                Ok(handle) => handle,
                Err(failure) if failure.is_oom() => return Err(failure),
                Err(failure) => return Err(poison(state, failure.into()).into()),
            };
            // Record custody before map. Even an ambiguous map failure keeps both handle and VA.
            state.entries.get_mut(&self.id).expect("live region").handle = Some(handle);
            if let Err(error) = self.gpu.driver().map(self.ptr, self.rounded, handle) {
                return Err(poison(state, error).into());
            }
        } else {
            // An abandoned external fill may still have queued writes.
            self.gpu.poll_fences(state)?;
            if self.gpu.completion(state, self.id) != Completion::Done {
                return Ok(None);
            }
        }
        self.uses.generation.try_update(Ordering::AcqRel, Ordering::Acquire,
            |generation| generation.checked_add(1))
            .map_err(|_| Error::Invalid("fill generation overflow"))?;
        *self.uses.submission.lock().map_err(|_| Error::Poisoned("fill submission mutex".into()))? = None;
        if !prefetched {
            state.used(self.id, self.gpu.next_use());
        }
        let pin = self.pin_locked(state)?;
        let filled = self.gpu.clock.load(Ordering::Relaxed);
        let entry = state.entries.get_mut(&self.id).expect("live region");
        entry.prefetched = prefetched;
        entry.filled = filled;
        self.uses.set(Contents::Loading);
        Ok(Some(Preparation::Fill(Fill { pin: Some(pin) })))
    }

    /// Borrow ready contents without creating a fill or doing device work.
    pub fn pin(self: &Arc<Self>) -> Result<Pin> {
        self.try_pin()?.ok_or(Error::Invalid("region is not ready"))
    }

    /// Atomically borrow ready contents, or report a miss without mapping or filling.
    /// Fork, poison and ownership errors remain failures, never residency misses.
    pub fn try_pin(self: &Arc<Self>) -> Result<Option<Pin>> {
        let mut state = self.gpu.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if entry.uses.state() != Contents::Ready {
            return Ok(None);
        }
        state.used(self.id, self.gpu.next_use());
        self.pin_locked(&mut state).map(Some)
    }

    /// `try_pin` that is no use: its place in the reclaim order stays where its last use left it.
    /// Another Region copying its bytes (TensorFS's content-addressed fill) is not a use of it.
    pub fn try_pin_unused(self: &Arc<Self>) -> Result<Option<Pin>> {
        let mut state = self.gpu.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if entry.uses.state() != Contents::Ready {
            return Ok(None);
        }
        let prefetched = entry.prefetched;
        let pin = self.pin_locked(&mut state)?;
        state.entries.get_mut(&self.id).expect("live region").prefetched = prefetched;
        Ok(Some(pin))
    }

    fn pin_locked(self: &Arc<Self>, state: &mut State) -> Result<Pin> {
        let entry = state.entries.get_mut(&self.id).expect("live region");
        entry
            .uses
            .pins
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Invalid("pin count overflow"))
            .map(|prior| if prior == 0 { self.gpu.held.fetch_add(self.rounded, Ordering::AcqRel); })?;
        entry.prefetched = false;
        Ok(Pin {
            region: Some(Arc::clone(self)),
            submission: None,
        })
    }
}

enum Preparation {
    Resident(Pin),
    Fill(Fill),
}

/// Exclusive initialization of a mapped region. Uncommitted contents are never published.
pub struct Fill {
    pin: Option<Pin>,
}

impl Fill {
    fn region(&self) -> &Region {
        self.pin.as_ref().expect("uncommitted fill").region()
    }
    pub fn ptr(&self) -> u64 {
        self.region().ptr
    }
    pub fn len(&self) -> usize {
        self.region().bytes
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn device(&self) -> i32 {
        self.region().device()
    }
    /// The Region this fills.
    pub fn target(&self) -> &Arc<Region> {
        self.pin.as_ref().expect("uncommitted fill").region()
    }

    /// Retain the in-flight mapping for a foreign fill tensor, including escaped failed-fill views.
    pub fn pin(&self) -> Result<Pin> {
        self.pin.as_ref().expect("uncommitted fill").try_clone()
    }

    /// Publish a fill performed through the raw pointer by a native integration.
    ///
    /// # Safety
    /// Every logical byte must have been initialized in this arena's primary CUDA context, and
    /// no writer may continue submitting work after this call. No pointer/view may escape the fill
    /// without retaining the returned Pin. Completion is joined here before publishing.
    #[allow(unsafe_code)]
    pub unsafe fn commit_external(self) -> Result<Pin> {
        self.finish(true)
    }

    /// Publish a native producer whose checked completion already joined every writer.
    ///
    /// # Safety
    /// Every logical byte must be initialized, including padding. Completion of every
    /// producer stream must be proven, and no writer may submit more work. Retain all
    /// source/staging/destination owners until that proof. Unlike `commit_external`, this
    /// does not join unrelated work in the context. It does not change consumer lifetime
    /// or the conservative reclamation fence for foreign storage aliases.
    #[allow(unsafe_code)]
    pub unsafe fn commit_completed(self) -> Result<Pin> {
        self.finish(false)
    }

    fn finish(mut self, join: bool) -> Result<Pin> {
        let region = self.region();
        if join {
            reclaim::join(region.gpu)?;
        }
        let gate = gate(region.gpu.ordinal)?;
        gate.not_capturing()?;
        let mut state = region.gpu.lock()?;
        let entry = state.entries.get_mut(&region.id).expect("fill owns region");
        entry.uses.set(Contents::Ready);
        let bytes = entry.bytes as u64;
        state.fills = state.fills.saturating_add(1);
        state.filled_bytes = state.filled_bytes.saturating_add(bytes);
        drop(state);
        drop(gate);
        Ok(self.pin.take().expect("committed fill"))
    }
}

impl Fill {
    /// Abort a fill whose writers are all checked complete: the pages hold no contents, and
    /// `Region::release_unready` may release them at once instead of after a context join.
    ///
    /// # Safety
    /// Every write through this fill's mapping completed in this device's primary context and
    /// nothing submits through it again. An escaped view keeps its own Pin.
    #[allow(unsafe_code)]
    pub unsafe fn abort_completed(mut self) -> Arc<Region> {
        let pin = self.pin.take().expect("uncommitted fill");
        if pin.region().pid == std::process::id() {
            pin.region().uses.set(Contents::Cold);
        }
        // SAFETY: the caller proved every writer complete; unpublished contents have no reader.
        unsafe { pin.into_completed_region() }
    }

    /// Abort, handing back the fill's own pin to release with what its owner knows of its writers.
    pub fn abandon(mut self) -> Pin {
        let pin = self.pin.take().expect("uncommitted fill");
        if pin.region().pid == std::process::id() {
            pin.region().uses.set(Contents::Cold);
        }
        pin
    }

    /// Abort without waiting, returning reservation ownership for deferred final teardown.
    pub fn into_unpinned_region(mut self) -> Arc<Region> {
        let pin = self.pin.take().expect("uncommitted fill");
        if pin.region().pid == std::process::id() {
            pin.region().uses.set(Contents::Cold);
        }
        pin.into_unpinned_region()
    }
}

impl Drop for Fill {
    fn drop(&mut self) {
        if let Some(pin) = &self.pin
            && pin.region().pid == std::process::id()
        {
            // Cold is visible before the fill pin is released. Escaped aliases stay pinned.
            pin.region().uses.set(Contents::Cold);
        }
    }
}

/// Physical pin + reservation ownership. The deleter of foreign storage must own this value.
pub struct Pin {
    region: Option<Arc<Region>>,
    submission: Option<Arc<async_fill::Control>>,
}

impl Pin {
    pub fn ptr(&self) -> u64 {
        self.region().ptr
    }
    pub fn len(&self) -> usize {
        self.region().bytes
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn device(&self) -> i32 {
        self.region().device()
    }
    pub fn region(&self) -> &Arc<Region> {
        self.region.as_ref().expect("live pin")
    }

    pub fn try_clone(&self) -> Result<Self> {
        let mut state = self.region().gpu.lock()?;
        let mut pin = self.region().pin_locked(&mut state)?;
        pin.submission = self.submission.clone();
        Ok(pin)
    }

    /// End physical ownership immediately, retaining the actual VA owner for deferred drop.
    /// Reclamation still joins queued GPU readers before unmapping the now-unpinned pages.
    pub fn into_unpinned_region(mut self) -> Arc<Region> {
        let region = self.region.take().expect("live pin");
        if region.pid == std::process::id() {
            region.mark_unjoined();
            region.unpinned();
        }
        region
    }

    /// End this physical use after native checked completion, retaining the reservation.
    ///
    /// # Safety
    /// Every producer and consumer using this pin must have completed in this device's
    /// primary context. The caller must own and successfully join the last-consumer event
    /// (including side streams) before calling, and prevent further submissions through
    /// this pin. Every escaped pointer/view must retain a separate live Pin; no alias may
    /// rely on the consumed Pin. A failed or uncertain event wait cannot use this method.
    /// Other ordinary pin releases still force a context fence before reuse or unmapping.
    #[allow(unsafe_code)]
    pub unsafe fn into_completed_region(mut self) -> Arc<Region> {
        let region = self.region.take().expect("live pin");
        if region.pid == std::process::id() {
            region.unpinned();
        }
        region
    }

    /// End this use after stamping each last-consumer stream. Reclaim then waits on those
    /// streams' fences, never the context. With no stream, this is `into_unpinned_region`.
    /// No driver call or wait, so storage deleters may call it.
    ///
    /// # Safety
    /// Every consumer of this pin, including escaped aliases sharing it, submitted all of its
    /// work to these streams (or joined it into one of them) and submits nothing after this
    /// call. Each stream is live and belongs to this device's primary context.
    #[allow(unsafe_code)]
    pub unsafe fn into_fenced_region(mut self, streams: &[u64]) -> Arc<Region> {
        let region = self.region.take().expect("live pin");
        if region.pid == std::process::id() {
            if region.stamp_consumers(streams).is_err() {
                region.mark_unjoined();
            }
            region.unpinned();
        }
        region
    }

    /// Record final consumers, retaining the exact physical Pin on any refusal.
    /// Unlike `into_fenced_region`, failure never selects a context-join fallback.
    /// A storage owner that cannot retry must transfer the returned Pin and its
    /// Source/service custody to explicit quarantine; it must not simply drop it.
    ///
    /// # Safety
    /// The caller has closed submission by every alias of this physical use.
    /// Every consumer is represented by a live stream in this device's primary
    /// context. No consumer may submit after this call, including after failure.
    #[allow(unsafe_code)]
    pub unsafe fn try_into_fenced_region(
        mut self,
        streams: &[u64],
    ) -> std::result::Result<Arc<Region>, (Self, Error)> {
        if self.region().pid != std::process::id() {
            return Err((self, Error::Invalid("physical Pin inherited after fork")));
        }
        let recorded = self.region().stamp_consumers(streams);
        if let Err(error) = recorded {
            return Err((self, error));
        }
        let region = self.region.take().expect("live pin");
        region.unpinned();
        Ok(region)
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        if let Some(region) = &self.region
            && region.pid == std::process::id()
        {
            region.mark_unjoined();
            region.unpinned();
        }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            return;
        }
        let Ok(gate) = gate(self.gpu.ordinal) else {
            return;
        };
        self.retire_intent();
        let Ok(mut state) = self.gpu.lock() else {
            return;
        };
        // Reclamation may already have retired this unowned VA while teardown was deferred.
        let Some(entry) = state.entries.get_mut(&self.id) else {
            return;
        };
        // Defer physical teardown to reclamation; destructors cannot block capture.
        if entry.handle.is_some() || gate.not_capturing().is_err() {
            return;
        }
        let Ok(_context) = self.gpu.driver().enter() else {
            return;
        };
        if let Err(error) = self.gpu.driver().free_address(self.ptr, self.rounded) {
            poison(&mut state, error);
            return;
        }
        state.entries.remove(&self.id);
    }
}

/// Host-only cumulative physical-release observation. Wrap is handled conservatively by
/// request-local checked subtraction; queued owner actions never change this counter.
pub fn released_bytes(device: i32) -> Result<u64> {
    Ok(gpu(device)?.released.load(Ordering::Acquire))
}

fn reclaim_completed(device: i32, wanted: usize) -> Result<usize> {
    let mut completed = 0;
    reclaim::release(device, wanted, usize::MAX, reclaim::Wait::No, reclaim::Protect::default(),
        ReclaimReason::Allocator, &mut completed)?;
    Ok(completed)
}

/// Development only, for the fault suite: from the `n`th next cuMemCreate on `device`, `times`
/// of them fail as a physical out of memory would. `times` 0 clears it.
pub fn fail_create_after(device: i32, n: u64, times: u64) -> Result<()> {
    gpu(device)?.driver.get().ok_or(Error::Invalid("device not opened"))?.fail_create_after(n, times);
    Ok(())
}

/// Development only: use at most this much of the card, as a smaller card would. The core
/// keeps it true against the driver's own charge, which it samples at every slow path.
pub fn set_limit(device: i32, limit: Option<usize>) -> Result<ReclaimReport> {
    let state = gpu(device)?;
    state.ceiling.store(limit.unwrap_or(usize::MAX), Ordering::Release);
    state.room_shrank();
    if let Some(driver) = state.driver.get()
        && !is_capturing(device)?
    {
        let _context = driver.enter()?;
        state.sample()?;
    }
    reclaim_room(device, state)
}

fn reclaim_room(device: i32, state: &Device) -> Result<ReclaimReport> {
    let over = state.over(0);
    let mut report = ReclaimReport {
        requested_bytes: over,
        ..ReclaimReport::default()
    };
    if over == 0 || is_capturing(device)? {
        return Ok(report);
    }
    // What is free to take now: a control thread never waits on GPU work, and the limit holds.
    report.evicted_regions = reclaim::release(device, over, usize::MAX, reclaim::Wait::No,
        reclaim::Protect::default(), ReclaimReason::Room, &mut report.reclaimed_bytes)?;
    Ok(report)
}

/// What a development cap sets aside for the driver's own reservation; 0 until one is sampled.
pub fn driver_reservation(device: i32) -> Result<usize> {
    Ok(gpu(device)?.reservation.get().copied().unwrap_or(0))
}

/// Bytes every completed fill has written on this device: host-to-device traffic.
pub fn filled_bytes(device: i32) -> Result<u64> {
    Ok(gpu(device)?.lock()?.filled_bytes)
}

/// Physical bytes of resident regions on this device.
pub fn resident_bytes(device: i32) -> Result<usize> {
    Ok(gpu(device)?.pages.load(Ordering::Acquire))
}
