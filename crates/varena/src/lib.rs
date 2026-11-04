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
pub use async_fill::{FillDestination, FillFacts, FillOwners, FillResource, FillRetirement, FillSession, FillState, FillTicket, StreamPreparation};
/// Versioned public ABI. All frontends resolve this shared library.
pub mod abi;
pub mod plan;
pub mod arena_retirement;
mod retirement;
#[doc(hidden)]
pub mod census;
pub use arena_retirement::{ArenaRetirement, ArenaRetirementFacts, ArenaRetirementState};
pub mod working;
pub(crate) mod tensor_consumers;
mod consumer_streams;
pub use consumer_streams::StreamUses;
pub use tensor_consumers::Consumers as TensorConsumers;
/// Preserve an unproven private storage Pin in the existing device retirement queue.
/// This performs no driver call, wait, retry, or reclamation credit.
#[doc(hidden)]
pub fn retain_unproven_pin(pin: Pin, failure: Error) {
    if pin.region().arena.pid != std::process::id() {
        std::mem::forget(pin);
        return;
    }
    let device = pin.device();
    retirement::quarantine(device, retirement::Job::UnprovenPin {
        _pin: pin, failure: failure.clone(),
    }, &failure);
}
pub mod cache_owner;
pub use cache_owner::CacheOwner;
pub use working::{Cache, CacheStats};
#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub mod ledger;
#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub mod cooperative;
#[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
pub mod fork_guard;

use fence::{Fences, Waits};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("CUDA capacity exhausted allocating {requested_bytes} bytes")]
    OutOfMemory { requested_bytes: usize },
    #[error("CUDA split capacity exhausted: required {requested_bytes} bytes, available {available_bytes} bytes")]
    Exhausted { requested_bytes: usize, available_bytes: usize },
    #[error("CUDA device fault in {operation}: CUresult {code}")]
    DeviceFault { operation: &'static str, code: i32 },
    #[error("CUDA driver failure in {operation}: CUresult {code}")]
    Cuda { operation: &'static str, code: i32 },
    #[error("residency arena poisoned; reservations retained: {0}")]
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
        matches!(self, Self::DriverOutOfMemory(_) | Self::Other(Error::OutOfMemory { .. }))
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

pub(crate) enum PeerProgress {
    Unavailable,
    Retry,
    FinalRetry,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub reserved_bytes: usize,
    pub resident_bytes: usize,
    pub ready_bytes: usize,
    pub pinned_bytes: usize,
    pub preferred_bytes: usize,
    pub regions: usize,
    pub fills: u64,
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
    pub requested: u64,
    pub allocator: u64,
    pub room: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum ReclaimReason {
    Requested,
    Allocator,
    Room,
}

impl ReclaimedBytes {
    fn add(&mut self, reason: ReclaimReason, bytes: usize) {
        let counter = match reason {
            ReclaimReason::Requested => &mut self.requested,
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
    // Logical size survives the final Region owner for read-only residency diagnostics.
    requested_bytes: usize,
    bytes: usize,
    handle: Option<u64>,
    uses: Arc<Uses>,
    preferred: bool,
    // Filled ahead of demand and not yet used: the first to go.
    prefetched: bool,
    last_use: u64,
}

impl Entry {
    fn preferred(&self) -> bool {
        self.preferred && self.uses.state() != Contents::Retired
    }

    fn idle(&self) -> bool {
        self.uses.pins() == 0
            && self.handle.is_some()
            && !matches!(self.uses.state(), Contents::Loading | Contents::Published)
    }
}

struct State {
    entries: BTreeMap<u64, Entry>,
    phase_boundary: Option<u64>,
    preferred_end: Option<u64>,
    poison: Option<String>,
    fills: u64,
    hits: u64,
    evictions: u64,
    reclaimed_bytes: u64,
    prefetch_unused_bytes: u64,
    reclaimed_by_reason: ReclaimedBytes,
}

impl State {
    fn empty() -> Self {
        Self { entries:BTreeMap::new(), phase_boundary:None, preferred_end:None, poison:None,
            fills:0, hits:0, evictions:0, reclaimed_bytes:0, prefetch_unused_bytes:0,
            reclaimed_by_reason:ReclaimedBytes::default() }
    }

    /// Eviction order for idle regions, lowest first. The device clock makes last uses
    /// comparable across arenas. With a plan, the furthest next use goes first; in a phase's
    /// loop that is most recently used first, as the loop reaches its oldest use next.
    /// Preferred regions go last, from the end of their prefix.
    fn rank(&self, id: u64, entry: &Entry, order: &plan::Order) -> (u8, u64) {
        if entry.preferred() {
            return (reclaim::PREFERRED, u64::MAX - id);
        }
        if matches!(entry.uses.state(), Contents::Cold | Contents::Retired) {
            return (reclaim::CONTENTLESS, entry.last_use);
        }
        if entry.prefetched {
            return (reclaim::PREFETCHED, entry.last_use);
        }
        let touched = self.phase_boundary.is_some_and(|boundary| entry.last_use > boundary);
        match order.next_use(id) {
            None if order.planned() || !touched => (reclaim::UNPLANNED, entry.last_use),
            None => (reclaim::PHASE_USED, u64::MAX - entry.last_use),
            Some((0, _)) if touched => (reclaim::PHASE_USED, u64::MAX - entry.last_use),
            Some((0, position)) => (reclaim::PHASE_NEXT, u64::MAX - position),
            Some((phases, position)) => (reclaim::LATER_PHASE, u64::MAX - (phases << 32 | position)),
        }
    }
}

/// A manager owned by one executor. It may serve multiple threads of that process.
pub struct Arena {
    id: u64,
    driver: Arc<cuda::Driver>,
    metadata: Arc<arena_retirement::Metadata>,
    device: i32,
    pid: u32,
    activity: &'static DeviceActivity,
    room: Arc<Room>,
    state: Mutex<State>,
}

/// This process's GPU bytes on one device against the room it may hold (`make_room`): its mapped
/// weight pages and working allocations. A smaller room reads as a
/// smaller card to every fill and every working mapping.
struct Room {
    /// usize::MAX: no limit.
    limit: AtomicUsize,
    pages: AtomicUsize,
    working: AtomicUsize,
    ceiling: AtomicUsize,
    // Actual successful physical releases only. Observations permit a real retry,
    // never a grant or attribution to one callback/peer.
    released: AtomicU64,
}

impl Default for Room {
    fn default() -> Self {
        Self {
            limit: AtomicUsize::new(usize::MAX),
            pages: AtomicUsize::new(0),
            working: AtomicUsize::new(0),
            ceiling: AtomicUsize::new(usize::MAX),
            released: AtomicU64::new(0),
        }
    }
}

impl Room {
    fn headroom(&self) -> usize {
        self.limit.load(Ordering::Acquire).min(self.ceiling.load(Ordering::Acquire))
            .saturating_sub(self.pages.load(Ordering::Acquire).saturating_add(self.working.load(Ordering::Acquire)))
    }

    /// Bytes `more` would put over the limit.
    fn over(&self, more: usize) -> usize {
        self.pages
            .load(Ordering::Acquire)
            .saturating_add(self.working.load(Ordering::Acquire))
            .saturating_add(more)
            .saturating_sub(
                self.limit.load(Ordering::Acquire).min(self.ceiling.load(Ordering::Acquire)),
            )
    }
}

struct RegisteredArena {
    device: i32,
    id: u64,
    owner: Weak<Arena>,
    metadata: Arc<arena_retirement::Metadata>,
}

type ArenaRegistry = Vec<RegisteredArena>;
static ARENAS: OnceLock<Mutex<ArenaRegistry>> = OnceLock::new();

// One process/core-wide namespace for reservation and Plan lifetimes. Never use
// addresses or these identities in compute fingerprints or reusable measurements.
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

fn next_identity() -> Result<u64> {
    check_process()?;
    NEXT_IDENTITY.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
        .map_err(|_| Error::Invalid("native object identity exhausted"))
}

/// Acquire matching GPU owners before capture/arena/working guards. No recovery or
/// last-owner destruction runs under the registry; callers keep these leases outside
/// every later guard, including early-return/error paths.
struct ArenaInventory {
    arenas: Vec<Arc<Arena>>,
    finalizing: Vec<Arc<arena_retirement::Metadata>>,
}

fn arena_inventory(device:i32) -> Result<ArenaInventory> {
    check_process()?;
    let Some(registry)=ARENAS.get() else { return Ok(ArenaInventory { arenas:Vec::new(),finalizing:Vec::new() }); };
    let (mut arenas,finalizing,gap)={
        let registry=registry.lock().map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
        let (mut arenas,mut finalizing,mut gap)=(Vec::new(),Vec::new(),false);
        for entry in registry.iter().filter(|entry| entry.device==device) {
            if let Some(arena)=entry.owner.upgrade() { arenas.push(arena); }
            else if entry.metadata.finalizing.load(Ordering::Acquire) { finalizing.push(entry.metadata.clone()); }
            else { gap=true; }
        }
        (arenas,finalizing,gap)
    };
    // An accepted native resource job is known, nonreclaimable inventory. Only an
    // unknown last-Arc/handoff gap refuses a partial snapshot. Owning drops are outside.
    if gap { return Err(Error::Busy); }
    arenas.sort_by_key(|arena| arena.id());
    Ok(ArenaInventory { arenas,finalizing })
}

/// Reclaim's view: a last-owner handoff in progress has nothing to give up and refuses nothing.
pub(crate) fn reclaim_arenas(device: i32) -> Result<Vec<Arc<Arena>>> {
    check_process()?;
    let Some(registry) = ARENAS.get() else { return Ok(Vec::new()); };
    let registry = registry.lock().map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
    Ok(registry.iter().filter(|entry| entry.device == device)
        .filter_map(|entry| entry.owner.upgrade()).collect())
}

/// Per-device coordination, for the process lifetime. Different GPUs share none of it.
#[derive(Default)]
pub(crate) struct DeviceActivity {
    gate: Mutex<()>,
    driver: OnceLock<Arc<cuda::Driver>>,
    working: OnceLock<working::Working>,
    activity_epoch: AtomicU64,
    external_callbacks: AtomicUsize,
    captures: AtomicUsize,
    replays: AtomicUsize,
    producers: AtomicUsize,
    retirements: AtomicUsize,
    room: Arc<Room>,
    fences: Mutex<Fences>,
    // Compares last uses across this device's arenas.
    clock: AtomicU64,
    joins_started: AtomicU64,
    joins_finished: AtomicU64,
}

impl DeviceActivity {
    fn driver(&self, device: i32) -> Result<Arc<cuda::Driver>> {
        if let Some(driver) = self.driver.get() {
            return Ok(driver.clone());
        }
        let driver = Arc::new(cuda::Driver::open(device)?);
        Ok(self.driver.get_or_init(|| driver).clone())
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
}

const MAX_DEVICES: usize = 64;
static ACTIVITY: [OnceLock<DeviceActivity>; MAX_DEVICES] = [const { OnceLock::new() }; MAX_DEVICES];
static PROCESS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn activity(device: i32) -> Result<&'static DeviceActivity> {
    check_process()?;
    let slot = usize::try_from(device).ok().and_then(|index| ACTIVITY.get(index))
        .ok_or(Error::Invalid("CUDA device ordinal out of range"))?;
    Ok(slot.get_or_init(DeviceActivity::default))
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

/// Excludes capture entry on this device. Lock order: gate, arena state, working state,
/// fences. Hold it across short driver calls, never across a GPU wait or a callback.
pub(crate) struct Gate {
    activity: &'static DeviceActivity,
    _held: MutexGuard<'static, ()>,
}

impl Gate {
    pub(crate) fn not_capturing(&self) -> Result<()> {
        if self.activity.captures.load(Ordering::Acquire) != 0 {
            return Err(Error::CaptureActive);
        }
        Ok(())
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.activity.epoch()
    }

    pub(crate) fn retiring(&self) -> bool {
        self.activity.retirements.load(Ordering::Acquire) != 0
    }

    /// Keeps capture out while this thread waits on the GPU with the gate released.
    pub(crate) fn hold(&self) -> Result<ProducerGuard> {
        self.not_capturing()?;
        self.activity.guard(&self.activity.producers)
    }
}

pub(crate) fn gate(device: i32) -> Result<Gate> {
    let activity = activity(device)?;
    let held = activity.gate.lock().map_err(|_| Error::Poisoned("device gate mutex".into()))?;
    Ok(Gate { activity, _held: held })
}

/// Guard every native arena on this device, including captures before any Arena exists.
/// Short-lived holders finish first. The bound is a last resort: a holder may be this very
/// thread, with no signal to tell. A refused capture falls back to eager.
pub fn enter_capture(device: i32) -> Result<GlobalCaptureGuard> {
    let deadline = std::time::Instant::now() + reclaim::RECOVERY_BUDGET;
    loop {
        {
            let gate = gate(device)?;
            let activity = gate.activity;
            let producers = activity.producers.load(Ordering::Acquire);
            if producers == 0
                && activity.replays.load(Ordering::Acquire) == 0
                && activity.retirements.load(Ordering::Acquire) == 0
            {
                activity.activity_epoch.fetch_add(1, Ordering::AcqRel);
                return activity.guard(&activity.captures);
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
    let activity = activity(device)?;
    activity.guard(&activity.external_callbacks)
}

/// A resident graph launch. Its retained Pins and pool references, not this host guard,
/// own its device dependencies, so reclaim continues beside it. Only capture entry waits.
pub fn enter_replay(device: i32) -> Result<GlobalCaptureGuard> {
    let gate = gate(device)?;
    gate.not_capturing()?;
    gate.activity.activity_epoch.fetch_add(1, Ordering::AcqRel);
    gate.activity.guard(&gate.activity.replays)
}

pub fn is_capturing(device: i32) -> Result<bool> {
    Ok(activity(device)?.captures.load(Ordering::Acquire) != 0)
}

/// Capture entry waits for a final owner's destruction, without this thread holding the
/// gate that destruction may need. Replay and recovery continue beside it.
fn enter_retirement(device: i32) -> Result<GlobalCaptureGuard> {
    let gate = gate(device)?;
    gate.not_capturing()?;
    gate.activity.guard(&gate.activity.retirements)
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

impl Arena {
    pub fn new(device: i32) -> Result<Arc<Self>> {
        let id = next_identity()?;
        retirement::prepare(device)?;
        let gate = gate(device)?;
        gate.not_capturing()?;
        let activity = gate.activity;
        let driver = activity.driver(device)?;
        let metadata = arena_retirement::Metadata::new(id,device,driver.counters());
        let arena = Arc::new(Self {
            id,
            driver,
            metadata,
            device,
            pid: std::process::id(),
            activity,
            room: activity.room.clone(),
            state: Mutex::new(State::empty()),
        });
        let mut registry = ARENAS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
        plan::inventory_changed(device)?;
        registry.push(RegisteredArena { device, id, owner: Arc::downgrade(&arena),metadata:arena.metadata.clone() });
        drop(registry);
        drop(gate);
        Ok(arena)
    }

    /// Never-reused lifetime identity within this process's one native core.
    pub fn id(&self) -> u64 { self.id }

    pub fn request_retirement(self:Arc<Self>) -> std::result::Result<ArenaRetirement,(Arc<Self>,Error)> {
        arena_retirement::request(self)
    }

    /// Drop-custody handoff. Current-process failures remain in native quarantine
    /// with a Failed ticket. Inherited rejection returns this Arc before any lock.
    pub fn defer_retirement(self:Arc<Self>) -> std::result::Result<ArenaRetirement,(Arc<Self>,Error)> {
        arena_retirement::defer(self)
    }

    pub fn device(&self) -> i32 {
        self.device
    }

    /// Physical pages for one region, inside this process's room on the device: over the room
    /// reads as the driver's out of memory, so each admission treats a smaller room as a smaller
    /// card (reclaims what it may, else refuses).
    fn create(&self, bytes: usize) -> AllocationResult<u64> {
        if self.room.over(bytes) != 0 {
            return Err(Error::OutOfMemory { requested_bytes: bytes }.into());
        }
        let handle = self.driver.create(bytes)?;
        self.room.pages.fetch_add(bytes, Ordering::AcqRel);
        self.metadata.physical.fetch_add(bytes, Ordering::AcqRel);
        Ok(handle)
    }

    pub fn granularity(&self) -> usize {
        self.driver.granularity
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        if self.pid != std::process::id() {
            return Err(Error::Invalid("CUDA arenas cannot be used after fork"));
        }
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Poisoned("arena mutex".into()))?;
        if let Some(reason) = &state.poison {
            return Err(Error::Poisoned(reason.clone()));
        }
        Ok(state)
    }

    /// Reserve address space without allocating physical memory. A region is one eviction group.
    pub fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Arc<Region>> {
        if bytes == 0 {
            return Err(Error::Invalid("zero-length region"));
        }
        let rounded = bytes
            .checked_add(self.granularity() - 1)
            .ok_or(Error::Invalid("region size overflow"))?
            / self.granularity()
            * self.granularity();
        let gate = gate(self.device)?;
        gate.not_capturing()?;
        let mut state = self.lock()?;
        // A new model may choose a fresh prefix only after every old reservation owner is gone.
        // Escaped storage and graph Pins own their Region, so they prevent this reset.
        if state
            .entries
            .values()
            .all(|entry| entry.uses.state() == Contents::Retired)
        {
            state.preferred_end = None;
        }
        let id = next_identity()?;
        // Advance before publication/driver work: failure may conservatively invalidate
        // a selection, but no new reservation can be published with an old epoch.
        plan::inventory_changed(self.device)?;
        let _context = self.driver.enter()?;
        let ptr = self.driver.reserve(rounded)?;
        let uses = Arc::new(Uses::new());
        state.entries.insert(
            id,
            Entry {
                ptr,
                requested_bytes: bytes,
                bytes: rounded,
                handle: None,
                uses: Arc::clone(&uses),
                preferred: false,
                prefetched: false,
                last_use: 0,
            },
        );
        Ok(Arc::new(Region {
            arena: Arc::clone(self),
            id,
            ptr,
            bytes,
            rounded,
            uses,
        }))
    }

    /// Enter an authored component phase without allocating, evicting or changing its
    /// admitted window. Only incoming bytes actually Ready now receive preference.
    pub fn enter_phase(&self, incoming: &[Arc<Region>]) -> Result<()> {
        gate(self.device)?.not_capturing()?;
        if incoming
            .iter()
            .any(|region| self.id != region.arena.id)
        {
            return Err(Error::Invalid(
                "phase regions must belong to the same arena",
            ));
        }
        let mut state = self.lock()?;
        state.preferred_end = None;
        state.phase_boundary = Some(self.activity.clock.load(Ordering::Relaxed));
        for entry in state.entries.values_mut() {
            entry.preferred = false;
        }
        for region in incoming {
            let entry = state
                .entries
                .get_mut(&region.id)
                .ok_or(Error::Invalid("phase region is retired"))?;
            entry.preferred = entry.handle.is_some() && entry.uses.state() == Contents::Ready;
        }
        Ok(())
    }

    pub fn owns_region(&self, region: &Region) -> bool {
        self.id == region.arena.id
    }

    /// Hold this guard over capture. Dependencies must already be prepared and separately pinned.
    pub fn enter_capture(self: &Arc<Self>) -> Result<CaptureGuard> {
        let global = enter_capture(self.device)?;
        drop(self.lock()?);
        Ok(CaptureGuard {
            global: Some(global),
            arena: Some(Arc::clone(self)),
        })
    }

    /// Release at least the requested bytes when possible. Pins are never overridden.
    /// Idle nonpreferred pages go first; preferred pages go from the end of the reserved prefix.
    pub fn reclaim(&self, bytes: usize) -> Result<ReclaimReport> {
        let mut report = ReclaimReport { requested_bytes: bytes, ..ReclaimReport::default() };
        report.evicted_regions = reclaim::release(self.device, bytes, usize::MAX, reclaim::Wait::Local,
            reclaim::Protect { arena: Some(self.id), keep_working: true, ..Default::default() },
            ReclaimReason::Requested, &mut report.reclaimed_bytes)?;
        Ok(report)
    }

    /// Start a phase: regions used before it are no longer in its loop. Metadata only.
    pub fn reset_preferences(&self) -> Result<()> {
        gate(self.device)?.not_capturing()?;
        let mut state = self.lock()?;
        state.preferred_end = None;
        state.phase_boundary = Some(self.activity.clock.load(Ordering::Relaxed));
        for entry in state.entries.values_mut() {
            entry.preferred = false;
        }
        Ok(())
    }

    /// As known at the last fence poll. No driver call.
    fn completion(&self, state: &State, id: u64) -> Completion {
        let uses = &state.entries[&id].uses;
        if !self.activity.joined(uses.unjoined.load(Ordering::Acquire)) {
            return Completion::Unjoined;
        }
        if self.activity.fences().complete(&uses.waits()) {
            Completion::Done
        } else {
            Completion::Pending
        }
    }

    /// Caller holds the gate with no capture active. A failed fence record or query poisons.
    fn poll_fences(&self, state: &mut State) -> Result<()> {
        let _context = self.driver.enter()?;
        let result = self.activity.fences().poll(&self.driver);
        result.map_err(|error| poison(state, error))
    }

    /// Unmap and release one joined, unpinned candidate.
    /// Unmap and release one completed, unpinned region. `completed` records the physical
    /// effect before any later fallible address retirement.
    fn evict(&self, state: &mut State, id: u64, reason: ReclaimReason, completed: &mut usize) -> Result<()> {
        let was_preferred = state.entries[&id].preferred();
        let prefetched = state.entries[&id].prefetched;
        let (size, retired) = self.release_backing_counted(state, id, completed)?;
        if prefetched {
            state.prefetch_unused_bytes = state.prefetch_unused_bytes.saturating_add(size as u64);
        }
        if was_preferred && !retired {
            // Pressure permanently shortens the retained prefix. Later scans cannot rebuild it.
            state.preferred_end = Some(state.preferred_end.map_or(id, |end| end.min(id)));
            for (_, later) in state.entries.range_mut(id..) {
                later.preferred = false;
            }
        }
        state.evictions = state.evictions.saturating_add(1);
        state.reclaimed_bytes = state.reclaimed_bytes.saturating_add(size as u64);
        state.reclaimed_by_reason.add(reason, size);
        Ok(())
    }

    /// Unmap and release a joined, unpinned region's physical pages; a retired region's
    /// address goes too. Returns its bytes and whether it was retired.
    fn release_backing(&self, state: &mut State, id: u64) -> Result<(usize, bool)> {
        self.release_backing_counted(state, id, &mut 0)
    }

    fn release_backing_counted(&self, state: &mut State, id: u64, completed: &mut usize) -> Result<(usize, bool)> {
        let entry = state
            .entries
            .get(&id)
            .expect("candidate belongs to locked arena");
        let (ptr, size, handle) = (
            entry.ptr,
            entry.bytes,
            entry.handle.expect("mapped candidate"),
        );
        if let Err(error) = self.driver.unmap(ptr, size) {
            return Err(poison(state, error));
        }
        // If release fails, do not retry or reuse either handle or reservation.
        if let Err(error) = self.driver.release(handle) {
            return Err(poison(state, error));
        }
        *completed += size;
        self.room.pages.fetch_sub(size, Ordering::AcqRel);
        self.metadata.physical.fetch_sub(size, Ordering::AcqRel);
        self.room.released.fetch_add(size as u64, Ordering::AcqRel);
        let entry = state.entries.get_mut(&id).expect("locked candidate");
        entry.uses.waits().clear();
        entry.prefetched = false;
        entry.handle = None;
        entry.uses.invalidate();
        let retired = entry.uses.state() == Contents::Retired;
        if retired {
            if let Err(error) = self.driver.free_address(ptr, size) {
                return Err(poison(state, error));
            }
            state.entries.remove(&id);
        }
        Ok((size, retired))
    }

    pub fn stats(&self) -> Result<Stats> {
        if self.pid != std::process::id() {
            return Err(Error::Invalid("CUDA arenas cannot be used after fork"));
        }
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Poisoned("arena mutex".into()))?;
        let mut stats = Stats {
            regions: state.entries.len(),
            fills: state.fills,
            hits: state.hits,
            evictions: state.evictions,
            reclaimed_bytes: state.reclaimed_bytes,
            prefetch_unused_bytes: state.prefetch_unused_bytes,
            reclaimed_by_reason: state.reclaimed_by_reason,
            context_joins: self.driver.context_joins(),
            driver_physical_ooms: self.driver.physical_ooms(),
            event_waits: self.driver.calls().3,
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
                if entry.preferred() {
                    stats.preferred_bytes += entry.bytes;
                }
            }
        }
        Ok(stats)
    }
}

fn poison(state: &mut State, error: Error) -> Error {
    state.poison = Some(error.to_string());
    error
}

/// A stable reservation. Dropping Runtime's views leaves this region's physical cache intact.
pub struct Region {
    arena: Arc<Arena>,
    id: u64,
    ptr: u64,
    bytes: usize,
    rounded: usize,
    uses: Arc<Uses>,
}

/// Final reservation ownership with no remaining public views or handles.
pub struct RetiredRegion {
    _region: Region,
}

impl Region {
    /// Never-reused process/core-wide reservation identity; independent of its VA.
    pub fn id(&self) -> u64 { self.id }
    pub fn arena_id(&self) -> u64 { self.arena.id }

    // Caller holds the gate.
    fn retire_intent(&self) {
        if self.uses.state() != Contents::Retired {
            let _ = plan::inventory_changed(self.arena.device);
            self.uses.set(Contents::Retired);
        }
    }

    pub fn retire(self) -> RetiredRegion {
        if self.arena.pid == std::process::id() {
            // Frontends may call this while holding their GIL/owner lock, before
            // handing RetiredRegion to deferred cleanup. Never wait for the gate here;
            // the eventual Drop linearizes retirement.
            if let Ok(_gate) = self.arena.activity.gate.try_lock() {
                self.retire_intent();
            }
        }
        RetiredRegion { _region: self }
    }
    /// Refuse further use when a native integration cannot prove completion of its writers.
    /// The integration must also retain its actual Fill/source owners; this is not cleanup.
    pub fn poison(&self, detail: impl Into<String>) {
        if self.arena.pid != std::process::id() {
            return;
        }
        let mut state = self.arena.state.lock().unwrap_or_else(|e| e.into_inner());
        state.poison.get_or_insert_with(|| detail.into());
    }

    /// Stamp this region's last consumers. No driver call, gate or wait.
    fn stamp_consumers(&self, streams: &[u64], submission: Option<&async_fill::Control>) -> Result<()> {
        let ordered = submission.is_some_and(|control| {
            control.generation == self.uses.generation.load(Ordering::Acquire)
                && self.uses.state() == Contents::Published
        });
        if streams.is_empty() || (self.uses.state() != Contents::Ready && !ordered) {
            return Err(Error::Invalid("release has no recordable consumer event"));
        }
        let mut fences = self.arena.activity.fences();
        let mut waits = self.uses.waits();
        for &stream in streams {
            fences.stamp(stream, &mut waits);
        }
        Ok(())
    }

    fn mark_unjoined(&self) {
        self.uses.unjoined.store(self.arena.activity.unjoined_mark(), Ordering::Release);
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
        self.arena.device
    }

    pub fn is_preferred(&self) -> Result<bool> {
        Ok(self
            .arena
            .lock()?
            .entries
            .get(&self.id)
            .expect("live region")
            .preferred())
    }

    /// Advisory admission check; physical pin acquisition and reclamation remain atomic.
    /// Native capture detachment uses this only to refuse surviving foreign storage.
    pub fn has_live_pins(&self) -> Result<bool> {
        let state = self.arena.lock()?;
        Ok(state
            .entries
            .get(&self.id)
            .expect("live region")
            .uses
            .pins()
            != 0)
    }

    pub fn is_resident(&self) -> Result<bool> {
        Ok(self
            .arena
            .lock()?
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
        let gate = gate(self.arena.device)?;
        gate.not_capturing()?;
        let mut state = self.arena.lock()?;
        self.arena.poll_fences(&mut state)?;
        let Some(entry) = state.entries.get(&self.id) else {
            return Ok(false);
        };
        if entry.handle.is_none()
            || entry.uses.pins() != 0
            || !matches!(entry.uses.state(), Contents::Cold | Contents::Retired)
            || self.arena.completion(&state, self.id) != Completion::Done
        {
            return Ok(false);
        }
        let _context = self.arena.driver.enter()?;
        self.arena.release_backing(&mut state, self.id)?;
        Ok(true)
    }

    pub fn set_preferred(&self, preferred: bool) -> Result<()> {
        let mut state = self.arena.lock()?;
        let preferred = preferred && state.preferred_end.is_none_or(|end| self.id < end);
        state
            .entries
            .get_mut(&self.id)
            .expect("live region")
            .preferred = preferred;
        Ok(())
    }

    /// Demand: anything idle may go to make room, in the one reclaim order.
    pub fn prepare(self: &Arc<Self>) -> Result<Preparation> {
        let protected = std::collections::BTreeSet::from([self.id]);
        let protect = reclaim::Protect { regions: Some(&protected), ..Default::default() };
        let result = (|| loop {
            let prepared = reclaim::acquire(self.arena.device, self.rounded, reclaim::Wait::Yes, protect, || {
                let gate = gate(self.arena.device)?;
                let mut state = self.arena.lock()?;
                self.prepare_locked(&gate, &mut state, false)
            })?;
            match prepared {
                Some(prepared) => return Ok(prepared),
                None => reclaim::settle(self)?,
            }
        })();
        if result.is_err() {
            let _ = working::invalidate_current_phase(self.arena.device, "weight_admission_error");
        }
        result
    }

    /// Admit speculative bytes only if no eviction or prior-writer join is required.
    /// Capacity refusal leaves existing ready contents and the preference watermark intact.
    pub fn prepare_without_reclaim(self: &Arc<Self>) -> Result<Preparation> {
        let gate = gate(self.arena.device)?;
        let mut state = self.arena.lock()?;
        match self.prepare_locked(&gate, &mut state, false) {
            Ok(Some(prepared)) => Ok(prepared),
            Ok(None) => Err(Error::OutOfMemory { requested_bytes: self.rounded }),
            Err(failure) => Err(failure.into()),
        }
    }

    /// Prefetch one immutable group, all or nothing, if the rule allows (`reclaim::prefetch_room`).
    /// A wrong guess costs a read, never a refusal: None declines without IO or waiting.
    #[allow(unsafe_code)]
    pub fn prepare_prefetch(regions: &[Arc<Self>]) -> Result<Option<Vec<Preparation>>> {
        let Some(first) = regions.first() else {
            return Ok(None);
        };
        if regions.iter().any(|region| !Arc::ptr_eq(&region.arena, &first.arena)) {
            return Ok(None); // Mixed arenas keep demand loading; no speculative partial group.
        }
        let ids: std::collections::BTreeSet<_> = regions.iter().map(|region| region.id).collect();
        if ids.len() != regions.len() {
            return Err(Error::Invalid("prefetch regions must be distinct"));
        }
        let (device, arena) = (first.device(), &first.arena);
        let missing = {
            let gate = gate(device)?;
            if gate.not_capturing().is_err() {
                return Ok(None);
            }
            let mut state = arena.lock()?;
            arena.poll_fences(&mut state)?;
            let (mut missing, mut cold) = (0usize, false);
            for region in regions {
                let entry = state.entries.get(&region.id).expect("source owns its region");
                if entry.uses.state() == Contents::Ready {
                    continue;
                }
                if matches!(entry.uses.state(), Contents::Loading | Contents::Published)
                    || entry.uses.pins() != 0
                    || (entry.handle.is_some() && arena.completion(&state, region.id) != Completion::Done)
                {
                    return Ok(None);
                }
                cold = true;
                if entry.handle.is_none() {
                    missing += region.rounded;
                }
            }
            if !cold {
                return Ok(None);
            }
            missing
        };
        if !reclaim::prefetch_room(device, missing, &ids)? {
            return Ok(None);
        }
        let gate = gate(device)?;
        let mut state = arena.lock()?;
        let (mut prepared, mut created) = (Vec::with_capacity(regions.len()), Vec::new());
        for region in regions {
            let fresh = state.entries[&region.id].handle.is_none();
            let outcome = region.prepare_locked(&gate, &mut state, true);
            if let Ok(Some(value)) = outcome {
                if fresh {
                    created.push(region.id);
                }
                prepared.push(value);
                continue;
            }
            // No producer or consumer has seen these pins. Abort the admitted prefix.
            for value in prepared {
                match value {
                    // SAFETY: group admission has issued no writes or exports.
                    Preparation::Fill(fill) => drop(unsafe { fill.abort_completed() }),
                    // SAFETY: this temporary pin was never handed to a consumer.
                    Preparation::Resident(pin) => drop(unsafe { pin.into_completed_region() }),
                }
            }
            let _context = arena.driver.enter()?;
            for id in created {
                arena.release_backing(&mut state, id)?;
            }
            return match outcome {
                Ok(_) => Ok(None),
                Err(failure) if failure.is_oom() => Ok(None),
                Err(failure) => match Error::from(failure) {
                    Error::Busy | Error::CaptureActive => Ok(None),
                    // Ambiguous device faults retain tracked mappings in the poisoned arena.
                    error => Err(error),
                },
            };
        }
        drop(state);
        working::prefetch_admitted(device)?;
        Ok(Some(prepared))
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
            return Ok(Some(Preparation::Resident(self.pin_locked(state)?)));
        }
        // A failed fill can leave escaped foreign storage. It owns these bytes until destruction.
        if entry.uses.pins() != 0 {
            return Err(Error::Busy.into());
        }
        gate.not_capturing()?;
        let _context = self.arena.driver.enter()?;
        if entry.handle.is_none() {
            let handle = match self.arena.create(self.rounded) {
                Ok(handle) => handle,
                Err(failure) if failure.is_oom() => return Err(failure),
                Err(failure) => return Err(poison(state, failure.into()).into()),
            };
            // Record custody before map. Even an ambiguous map failure keeps both handle and VA.
            state.entries.get_mut(&self.id).expect("live region").handle = Some(handle);
            if let Err(error) = self.arena.driver.map(self.ptr, self.rounded, handle) {
                return Err(poison(state, error).into());
            }
        } else {
            // An abandoned external fill may still have queued writes.
            self.arena.poll_fences(state)?;
            if self.arena.completion(state, self.id) != Completion::Done {
                return Ok(None);
            }
        }
        self.uses.generation.try_update(Ordering::AcqRel, Ordering::Acquire,
            |generation| generation.checked_add(1))
            .map_err(|_| Error::Invalid("fill generation overflow"))?;
        *self.uses.submission.lock().map_err(|_| Error::Poisoned("fill submission mutex".into()))? = None;
        let pin = self.pin_locked(state)?;
        state.entries.get_mut(&self.id).expect("live region").prefetched = prefetched;
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
        let mut state = self.arena.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if entry.uses.state() != Contents::Ready {
            return Ok(None);
        }
        self.pin_locked(&mut state).map(Some)
    }

    fn pin_locked(self: &Arc<Self>, state: &mut State) -> Result<Pin> {
        let entry = state.entries.get_mut(&self.id).expect("live region");
        entry
            .uses
            .pins
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Invalid("pin count overflow"))?;
        entry.last_use = self.arena.activity.next_use();
        entry.prefetched = false;
        Ok(Pin {
            region: Some(Arc::clone(self)),
            submission: None,
        })
    }
}

pub enum Preparation {
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
            reclaim::join(region.device(), &region.arena.driver)?;
        }
        let gate = gate(region.arena.device)?;
        gate.not_capturing()?;
        let mut state = region.arena.lock()?;
        let entry = state.entries.get_mut(&region.id).expect("fill owns region");
        entry.uses.set(Contents::Ready);
        state.fills = state.fills.saturating_add(1);
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
        if pin.region().arena.pid == std::process::id() {
            pin.region().uses.set(Contents::Cold);
        }
        // SAFETY: the caller proved every writer complete; unpublished contents have no reader.
        unsafe { pin.into_completed_region() }
    }

    /// Abort without waiting, returning reservation ownership for deferred final teardown.
    pub fn into_unpinned_region(mut self) -> Arc<Region> {
        let pin = self.pin.take().expect("uncommitted fill");
        if pin.region().arena.pid == std::process::id() {
            pin.region().uses.set(Contents::Cold);
        }
        pin.into_unpinned_region()
    }
}

impl Drop for Fill {
    fn drop(&mut self) {
        if let Some(pin) = &self.pin
            && pin.region().arena.pid == std::process::id()
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
        let mut state = self.region().arena.lock()?;
        let mut pin = self.region().pin_locked(&mut state)?;
        pin.submission = self.submission.clone();
        Ok(pin)
    }

    /// End physical ownership immediately, retaining the actual VA owner for deferred drop.
    /// Reclamation still joins queued GPU readers before unmapping the now-unpinned pages.
    pub fn into_unpinned_region(mut self) -> Arc<Region> {
        let region = self.region.take().expect("live pin");
        if region.arena.pid == std::process::id() {
            region.mark_unjoined();
            decrement(&region.uses.pins);
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
        if region.arena.pid == std::process::id() {
            decrement(&region.uses.pins);
        }
        region
    }

    /// End this use after stamping each last-consumer stream. Reclaim then waits on those
    /// streams' fences, never the context. With no stream or unready contents, this is
    /// `into_unpinned_region`. No driver call or wait, so storage deleters may call it.
    ///
    /// # Safety
    /// Every consumer of this pin, including escaped aliases sharing it, submitted all of its
    /// work to these streams (or joined it into one of them) and submits nothing after this
    /// call. Each stream is live and belongs to this device's primary context.
    #[allow(unsafe_code)]
    pub unsafe fn into_fenced_region(mut self, streams: &[u64]) -> Arc<Region> {
        let region = self.region.take().expect("live pin");
        if region.arena.pid == std::process::id() {
            if region.stamp_consumers(streams, self.submission.as_deref()).is_err() {
                region.mark_unjoined();
            }
            decrement(&region.uses.pins);
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
        if self.region().arena.pid != std::process::id() {
            return Err((self, Error::Invalid("physical Pin inherited after fork")));
        }
        let recorded = self.region().stamp_consumers(streams, self.submission.as_deref());
        if let Err(error) = recorded {
            return Err((self, error));
        }
        let region = self.region.take().expect("live pin");
        decrement(&region.uses.pins);
        Ok(region)
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        if let Some(region) = &self.region
            && region.arena.pid == std::process::id()
        {
            region.mark_unjoined();
            decrement(&region.uses.pins);
        }
    }
}

impl Preparation {
    pub fn into_unpinned_region(self) -> Arc<Region> {
        match self {
            Self::Resident(pin) => pin.into_unpinned_region(),
            Self::Fill(fill) => fill.into_unpinned_region(),
        }
    }
}

pub struct CaptureGuard {
    global: Option<GlobalCaptureGuard>,
    arena: Option<Arc<Arena>>,
}

impl CaptureGuard {
    pub fn into_arena(mut self) -> Arc<Arena> {
        drop(self.global.take());
        self.arena.take().expect("live capture owner")
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        if self.arena.pid != std::process::id() {
            return;
        }
        let Ok(gate) = gate(self.arena.device) else {
            return;
        };
        self.retire_intent();
        let Ok(mut state) = self.arena.lock() else {
            return;
        };
        // Reclamation may already have retired this unowned VA while teardown was deferred.
        let Some(entry) = state.entries.get_mut(&self.id) else {
            return;
        };
        entry.preferred = false;
        // Defer physical teardown to reclamation or Arena drop; destructors cannot block capture.
        if entry.handle.is_some() || gate.not_capturing().is_err() {
            return;
        }
        let Ok(_context) = self.arena.driver.enter() else {
            return;
        };
        if let Err(error) = self.arena.driver.free_address(self.ptr, self.rounded) {
            poison(&mut state, error);
            return;
        }
        state.entries.remove(&self.id);
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        if self.pid != std::process::id() { return; }
        arena_retirement::final_owner(self);
    }
}

fn room(device: i32) -> Result<Arc<Room>> {
    Ok(Arc::clone(&activity(device)?.room))
}

/// Host-only cumulative physical-release observation. Wrap is handled conservatively by
/// request-local checked subtraction; queued owner actions never change this counter.
pub fn released_bytes(device: i32) -> Result<u64> {
    Ok(room(device)?.released.load(Ordering::Acquire))
}

fn reclaim_completed(device: i32, wanted: usize, retained: &mut Vec<Arc<arena_retirement::Observation>>) -> Result<usize> {
    // The callback worker retains observed Arena leases exactly as Peer does.
    retain_arenas(device, retained)?;
    let mut completed = 0;
    reclaim::release(device, wanted, usize::MAX, reclaim::Wait::No, reclaim::Protect::default(),
        ReclaimReason::Allocator, &mut completed)?;
    Ok(completed)
}

fn observe_inventory(inventory:&ArenaInventory, retained:&mut Vec<Arc<arena_retirement::Observation>>) -> Result<()> {
    for arena in &inventory.arenas {
        if !retained.iter().any(|held| held.id==arena.id()) { retained.push(arena.metadata.observe(Some(arena))?); }
    }
    for metadata in &inventory.finalizing {
        if !retained.iter().any(|held| held.id==metadata.id) { retained.push(metadata.observe(None)?); }
    }
    Ok(())
}

fn retain_arenas(device:i32, retained:&mut Vec<Arc<arena_retirement::Observation>>) -> Result<ArenaInventory> {
    let inventory=arena_inventory(device)?;
    observe_inventory(&inventory,retained)?;
    Ok(inventory)
}

/// The room this process may hold on `device` (weight pages plus mapped working allocations),
/// None for no limit, and at once its pages above it back, in reclaim order (pins are never
/// overridden; under a capture they wait for the next fill or allocation, which the room
/// bounds from now on). What a machine asks of an executor when another program needs the
/// GPU (design/vram-yield.md): the one make_room for foreign pressure.
pub fn make_room(device: i32, limit: Option<usize>) -> Result<ReclaimReport> {
    check_process()?;
    let room = room(device)?;
    room.limit.store(limit.unwrap_or(usize::MAX), Ordering::Release);
    reclaim_room(device, &room)
}

/// Bound managed physical memory, independently of the temporary room requested by a peer.
/// Native decode, Torch working allocations and resident weight pages all count.
/// Contexts and allocations made by other CUDA libraries are outside this ceiling.
pub fn set_limit(device: i32, limit: Option<usize>) -> Result<ReclaimReport> {
    let room = room(device)?;
    room.ceiling.store(limit.unwrap_or(usize::MAX), Ordering::Release);
    reclaim_room(device, &room)
}

fn reclaim_room(device: i32, room: &Room) -> Result<ReclaimReport> {
    let over = room.over(0);
    let mut report = ReclaimReport {
        requested_bytes: over,
        ..ReclaimReport::default()
    };
    if over == 0 || is_capturing(device)? {
        return Ok(report);
    }
    report.evicted_regions = reclaim::release(device, over, usize::MAX, reclaim::Wait::Local,
        reclaim::Protect::default(), ReclaimReason::Room, &mut report.reclaimed_bytes)?;
    Ok(report)
}

/// Every idle byte this device can give up, in reclaim order. Never unmaps what is in use.
pub fn reclaim(device: i32, bytes: usize) -> Result<ReclaimReport> {
    let mut report = ReclaimReport { requested_bytes: bytes, ..ReclaimReport::default() };
    report.evicted_regions = reclaim::release(device, bytes, usize::MAX, reclaim::Wait::Local,
        reclaim::Protect::default(), ReclaimReason::Allocator, &mut report.reclaimed_bytes)?;
    Ok(report)
}

/// Physical bytes owned by live arenas on this device, for a local managed-memory ceiling.
pub fn resident_bytes(device: i32) -> Result<usize> {
    check_process()?;
    let inventory=arena_inventory(device)?;
    let mut bytes=0usize;
    for arena in &inventory.arenas {
        bytes=bytes.checked_add(arena.stats()?.resident_bytes).ok_or(Error::Invalid("resident byte count overflow"))?;
    }
    for metadata in &inventory.finalizing {
        bytes=bytes.checked_add(metadata.physical.load(Ordering::Acquire)).ok_or(Error::Invalid("resident byte count overflow"))?;
    }
    Ok(bytes)
}
