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
mod async_fill;
pub use async_fill::{FillDestination, FillFacts, FillOwners, FillResource, FillRetirement, FillSession, FillState, FillTicket, StreamPreparation};
/// Versioned public ABI. All frontends resolve this shared library.
pub mod abi;
pub mod plan;
pub mod working;
pub mod cache_owner;
pub use cache_owner::CacheOwner;
pub use working::{Cache, CacheStats};
#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub mod ledger;
#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub mod cooperative;
#[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
pub mod fork_guard;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
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
    #[error("invalid residency operation: {0}")]
    Invalid(&'static str),
    #[error("CUDA virtual memory unavailable: {0}")]
    Unsupported(String),
    #[error("cache owner callback failed: {0}")]
    Callback(String),
}

/// Private admission cause: another process cannot repair a local cap or VA refusal.
enum AllocationFailure {
    DriverOutOfMemory(usize),
    Other(Error),
}

type AllocationResult<T> = std::result::Result<T, AllocationFailure>;

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

enum PeerProgress {
    Unavailable,
    Retry,
    FinalRetry,
}

const RECOVERY_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Default)]
struct Recovery {
    deadline: Option<std::time::Instant>,
    observed: Option<(u64, u64)>,
    terminal: bool,
    cache_request: Option<cache_owner::ShrinkRequest>,
    #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
    participation: Option<cooperative::Participation>,
}

impl Recovery {
    /// Called after finite local retries fail. Owner requests are always asynchronous;
    /// only physical refusals can enter peer recovery. Local caps never ask other PIDs.
    fn next(&mut self, device: i32, bytes: usize, physical: bool) -> Result<bool> {
        if self.terminal { return Ok(false); }
        {
            let captures = capture_counts()?;
            ensure_recovery_allowed(&captures, device)?;
        }
        let limit = *self.deadline.get_or_insert_with(|| std::time::Instant::now() + RECOVERY_BUDGET);
        if std::time::Instant::now() >= limit { self.terminal = true; return Ok(false); }
        let release_before = released_bytes(device)?;
        if cache_owner::requestable(device)? != 0 {
            self.cache_request = cache_owner::request_shrink(device, cache_owner::RequestKey {
                cause: cache_owner::Cause::Allocation, generation: cache_owner::next_generation()?,
            }, bytes)?;
        }
        #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
        let progress = if physical {
            cooperative::recover(device, bytes, &mut self.deadline, &mut self.observed,
                &mut self.participation, self.cache_request.as_ref().map(|_| release_before))?
        } else { PeerProgress::Unavailable };
        #[cfg(not(all(target_os = "linux", target_has_atomic = "64")))]
        let progress = { let _ = (device, bytes, physical, &mut self.deadline, &mut self.observed); PeerProgress::Unavailable };
        let cache_progress = self.cache_request.is_some() && released_bytes(device)? > release_before;
        self.terminal = std::time::Instant::now() >= limit
            || (!cache_progress && !matches!(progress, PeerProgress::Retry));
        #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
        if self.terminal { drop(self.participation.take()); }
        Ok(cache_progress || !matches!(progress, PeerProgress::Unavailable))
    }

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
    pub reclaimed_by_reason: ReclaimedBytes,
    /// cuCtxSynchronize calls made before reclaiming or refilling unfenced releases.
    pub context_joins: u64,
    /// Actual cuMemCreate allocation refusals, excluding local room and VA limits.
    pub driver_physical_ooms: u64,
    /// Last-consumer events host-waited before reclaiming or refilling fenced releases.
    pub event_waits: u64,
    pub poisoned: bool,
}

/// Cumulative successfully released bytes, classified by the entry point that reclaimed them.
/// Allocator recovery includes local allocation ceilings as well as driver capacity failures;
/// these counters describe the trigger, not physical transfer traffic or time spent waiting.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReclaimedBytes {
    pub requested: u64,
    pub streaming_limit: u64,
    pub weight_allocation: u64,
    pub allocator: u64,
    pub room: u64,
}

#[derive(Clone, Copy)]
enum ReclaimReason {
    Requested,
    StreamingLimit,
    WeightAllocation,
    Allocator,
    Room,
}

impl ReclaimedBytes {
    fn add(&mut self, reason: ReclaimReason, bytes: usize) {
        let counter = match reason {
            ReclaimReason::Requested => &mut self.requested,
            ReclaimReason::StreamingLimit => &mut self.streaming_limit,
            ReclaimReason::WeightAllocation => &mut self.weight_allocation,
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
    // Only a native caller with a checked last-consumer fence can release without this bit.
    unjoined: AtomicBool,
    // One reusable last-consumer event per declared stream. A release records it before its
    // pin decrement, so a zero-pin reclaim candidate's events cannot be re-recorded under it.
    fences: Mutex<Vec<Fence>>,
    generation: AtomicU64,
    submission: Mutex<Option<Arc<async_fill::Control>>>,
}

#[derive(Clone, Copy)]
struct Fence {
    stream: usize,
    event: usize,
}

/// What a fill may do for room: anything (`Any`), nothing (`None`), or evict only idle
/// streamed regions, waiting at most on their own consumer events (`Completed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    Any,
    Completed,
    None,
    Prefetch,
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
            unjoined: AtomicBool::new(false),
            fences: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
            submission: Mutex::new(None),
        }
    }
    fn fences(&self) -> MutexGuard<'_, Vec<Fence>> {
        self.fences.lock().unwrap_or_else(|e| e.into_inner())
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
    bytes: usize,
    handle: Option<u64>,
    uses: Arc<Uses>,
    preferred: bool,
    last_use: u64,
}

impl Entry {
    fn preferred(&self) -> bool {
        self.preferred && self.uses.state() != Contents::Retired
    }
}

struct State {
    entries: BTreeMap<u64, Entry>,
    clock: u64,
    streaming_limit: usize,
    phase_boundary: Option<u64>,
    preferred_end: Option<u64>,
    poison: Option<String>,
    fills: u64,
    hits: u64,
    evictions: u64,
    reclaimed_bytes: u64,
    reclaimed_by_reason: ReclaimedBytes,
    context_joins: u64,
    event_waits: u64,
}

impl State {
    fn standby(&self, entry: &Entry) -> bool {
        self.phase_boundary.is_some_and(|boundary| {
            self.clock != u64::MAX
                && entry.last_use <= boundary
                && entry.uses.state() == Contents::Ready
        })
    }

    /// Idle nonpreferred eviction order, ascending: other phases' cache oldest first, then the
    /// running phase's own regions most recently used first. A phase loops over its streamed
    /// blocks (every denoise step); with more of them than the window holds, oldest-first
    /// evicts each block just before its next use and refills all of them every pass, while
    /// newest-first keeps the ones the loop reaches next (Qwen-Image int8 on 8 GB: 3 of 3
    /// streamed blocks refilled every step). Without a phase there is no loop: oldest first.
    fn eviction_key(&self, entry: &Entry) -> (bool, u64) {
        match self.phase_boundary {
            Some(boundary) if self.clock != u64::MAX && entry.last_use > boundary => {
                (true, u64::MAX - entry.last_use)
            }
            _ => (false, entry.last_use),
        }
    }

    fn reclaim_candidates(&self, preferred: bool, exclude: Option<u64>, reason: ReclaimReason)
        -> Vec<(u64, bool, (bool, u64))> {
        let mut candidates: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, entry)| {
                Some(**id) != exclude
                    && entry.uses.pins() == 0
                    && !matches!(entry.uses.state(), Contents::Loading | Contents::Published)
                    && entry.handle.is_some()
                    && (preferred || !entry.preferred())
                    && (!matches!(reason, ReclaimReason::StreamingLimit) || !self.standby(entry))
            })
            .map(|(id, entry)| {
                let preferred = entry.preferred();
                let key = if preferred {
                    (false, u64::MAX - id)
                } else {
                    self.eviction_key(entry)
                };
                (*id, preferred, key)
            })
            .collect();
        candidates.sort_by_key(|(_, preferred, key)| (*preferred, *key));
        candidates
    }

    fn contentless_candidates(&self, preferred: bool, exclude: Option<u64>) -> Vec<u64> {
        let mut idle: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, entry)| {
                Some(**id) != exclude
                    && entry.uses.pins() == 0
                    && matches!(entry.uses.state(), Contents::Cold | Contents::Retired)
                    && entry.handle.is_some()
                    && (preferred || !entry.preferred())
            })
            .map(|(id, entry)| (entry.last_use, *id))
            .collect();
        idle.sort_unstable();
        idle.into_iter().map(|(_, id)| id).collect()
    }

    fn streaming_bytes(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| {
                !entry.preferred()
                    && entry.handle.is_some()
                    && entry.uses.pins() == 0
                    && !matches!(entry.uses.state(), Contents::Loading | Contents::Published)
                    && !self.standby(entry)
            })
            .map(|entry| entry.bytes)
            .sum()
    }
}

/// A manager owned by one executor. It may serve multiple threads of that process.
pub struct Arena {
    id: u64,
    driver: cuda::Driver,
    device: i32,
    pid: u32,
    // Lock-free view of this device's capture guards, for release paths that never wait.
    captures: Arc<AtomicUsize>,
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
fn arena_leases(device: i32) -> Result<Vec<Arc<Arena>>> {
    check_process()?;
    let Some(registry) = ARENAS.get() else { return Ok(Vec::new()); };
    let registry = registry.lock().map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
    let leases = registry.iter().filter(|entry| entry.device == device)
        .filter_map(|entry| entry.owner.upgrade()).collect();
    drop(registry);
    Ok(leases)
}
#[derive(Default)]
struct DeviceActivity {
    activity_epoch: u64,
    external_callbacks: Arc<AtomicUsize>,
    captures: Arc<AtomicUsize>,
    replays: Arc<AtomicUsize>,
    producers: Arc<AtomicUsize>,
    retirements: Arc<AtomicUsize>,
    room: Arc<Room>,
}

static CAPTURES: OnceLock<Mutex<BTreeMap<i32, DeviceActivity>>> = OnceLock::new();
static PROCESS: AtomicU32 = AtomicU32::new(0);

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

// Lock order for CUDA work: capture registry, then arena state. Keep this guard through the
// driver calls so capture entry cannot race a context join or residency mutation.
fn capture_counts() -> Result<MutexGuard<'static, BTreeMap<i32, DeviceActivity>>> {
    check_process()?;
    CAPTURES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("capture registry mutex".into()))
}

fn ensure_recovery_allowed(counts: &BTreeMap<i32, DeviceActivity>, device: i32) -> Result<()> {
    if counts.get(&device).is_some_and(|entry| {
        entry.captures.load(Ordering::Acquire) != 0 || entry.replays.load(Ordering::Acquire) != 0
    }) {
        return Err(Error::CaptureActive);
    }
    Ok(())
}

fn ensure_not_capturing(counts: &BTreeMap<i32, DeviceActivity>, device: i32) -> Result<()> {
    if counts
        .get(&device)
        .is_some_and(|entry| entry.captures.load(Ordering::Acquire) != 0)
    {
        return Err(Error::CaptureActive);
    }
    Ok(())
}

/// Guard every native arena on this device, including captures before any Arena exists.
pub fn enter_capture(device: i32) -> Result<GlobalCaptureGuard> {
    let mut counts = capture_counts()?;
    let activity = counts.entry(device).or_default();
    if activity.replays.load(Ordering::Acquire) != 0 || activity.retirements.load(Ordering::Acquire) != 0 {
        return Err(Error::CaptureActive);
    }
    if activity.producers.load(Ordering::Acquire) != 0 {
        return Err(Error::Invalid(
            "native producers must finish before CUDA graph capture",
        ));
    }
    activity.activity_epoch = activity.activity_epoch.wrapping_add(1);
    activity
        .captures
        .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map_err(|_| Error::Invalid("capture count overflow"))?;
    Ok(GlobalCaptureGuard {
        count: Arc::clone(&activity.captures),
        pid: std::process::id(),
    })
}

/// Suppress allocation recovery during a resident graph launch without excluding producers.
/// The graph's retained Pins, rather than this short host guard, own its device dependencies.
/// Bound a user callback's host interval. Only actual working alloc/free invalidates a
/// measurement; CPU-only callbacks add no failure and this guard performs no CUDA work.
pub fn enter_external_callback(device: i32) -> Result<GlobalCaptureGuard> {
    let mut counts = capture_counts()?;
    let activity = counts.entry(device).or_default();
    activity.external_callbacks.try_update(Ordering::AcqRel, Ordering::Acquire,
        |value| value.checked_add(1)).map_err(|_| Error::Invalid("callback scope overflow"))?;
    Ok(GlobalCaptureGuard { count: activity.external_callbacks.clone(), pid: std::process::id() })
}

pub fn enter_replay(device: i32) -> Result<GlobalCaptureGuard> {
    let mut counts = capture_counts()?;
    ensure_not_capturing(&counts, device)?;
    let activity = counts.entry(device).or_default();
    if activity.retirements.load(Ordering::Acquire) != 0 {
        return Err(Error::CaptureActive);
    }
    activity.activity_epoch = activity.activity_epoch.wrapping_add(1);
    activity
        .replays
        .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map_err(|_| Error::Invalid("replay count overflow"))?;
    Ok(GlobalCaptureGuard {
        count: Arc::clone(&activity.replays),
        pid: std::process::id(),
    })
}

pub fn is_capturing(device: i32) -> Result<bool> {
    Ok(capture_counts()?
        .get(&device)
        .is_some_and(|entry| entry.captures.load(Ordering::Acquire) != 0))
}

/// Exclude new capture/replay through final owner destruction without holding the capture
/// mutex that Arena::drop reacquires. Normal recovery/teardown remains allowed under it.
fn enter_retirement(device: i32) -> Result<GlobalCaptureGuard> {
    let mut counts = capture_counts()?;
    ensure_recovery_allowed(&counts, device)?;
    let activity = counts.entry(device).or_default();
    activity.retirements.try_update(Ordering::AcqRel, Ordering::Acquire, |value| value.checked_add(1))
        .map_err(|_| Error::Invalid("retirement count overflow"))?;
    Ok(GlobalCaptureGuard { count: Arc::clone(&activity.retirements), pid: std::process::id() })
}

pub fn is_recovery_blocked(device: i32) -> Result<bool> {
    Ok(capture_counts()?.get(&device).is_some_and(|entry| {
        entry.captures.load(Ordering::Acquire) != 0 || entry.replays.load(Ordering::Acquire) != 0
    }))
}

pub struct GlobalCaptureGuard {
    count: Arc<AtomicUsize>,
    pid: u32,
}

impl Drop for GlobalCaptureGuard {
    fn drop(&mut self) {
        if self.pid == std::process::id() {
            decrement(&self.count);
        }
    }
}

/// Own the right to submit native producer work outside actual CUDA capture.
/// The caller retains this lease until every producer has checked completion; uncertain
/// completion retains the lease along with the actual Fill/source owners. Graph replay
/// is unaffected because its dependencies have separate physical pins.
pub fn enter_producer(device: i32) -> Result<ProducerGuard> {
    let mut counts = capture_counts()?;
    ensure_not_capturing(&counts, device)?;
    let activity = counts.entry(device).or_default();
    activity
        .producers
        .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map_err(|_| Error::Invalid("producer count overflow"))?;
    Ok(ProducerGuard {
        count: Arc::clone(&activity.producers),
        pid: std::process::id(),
    })
}

pub struct ProducerGuard {
    count: Arc<AtomicUsize>,
    pid: u32,
}

impl Drop for ProducerGuard {
    fn drop(&mut self) {
        if self.pid == std::process::id() {
            decrement(&self.count);
        }
    }
}

impl Arena {
    pub fn new(device: i32) -> Result<Arc<Self>> {
        let id = next_identity()?;
        let mut captures = capture_counts()?;
        ensure_recovery_allowed(&captures, device)?;
        let driver = cuda::Driver::open(device)?;
        let activity = captures.entry(device).or_default();
        let (capturing, room) = (Arc::clone(&activity.captures), Arc::clone(&activity.room));
        drop(captures);
        let arena = Arc::new(Self {
            id,
            driver,
            device,
            pid: std::process::id(),
            captures: capturing,
            room,
            state: Mutex::new(State {
                entries: BTreeMap::new(),
                clock: 0,
                streaming_limit: usize::MAX,
                phase_boundary: None,
                preferred_end: None,
                poison: None,
                fills: 0,
                hits: 0,
                evictions: 0,
                reclaimed_bytes: 0,
                reclaimed_by_reason: ReclaimedBytes::default(),
                context_joins: 0,
                event_waits: 0,
            }),
        });
        // Registration changes the bounded phase-plan arena inventory. This guard
        // is declared after `arena`: every error drops it before the owning Arc.
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, device)?;
        let mut registry = ARENAS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
        plan::inventory_changed(device)?;
        registry.push(RegisteredArena { device, id, owner: Arc::downgrade(&arena) });
        Ok(arena)
    }

    /// Never-reused lifetime identity within this process's one native core.
    pub fn id(&self) -> u64 { self.id }

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
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, self.device)?;
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
                bytes: rounded,
                handle: None,
                uses: Arc::clone(&uses),
                preferred: false,
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

    /// Bound the current phase's idle nonpreferred cache plus the next group before allocation.
    /// Ready cache last pinned before reset_preferences is standby: only capacity pressure
    /// or explicit reclamation evicts it. Without a phase reset the bound remains global.
    /// Active/graph pins are working memory and excluded; real CUDA capacity still bounds them.
    /// A single larger group is permitted. The most recently used idle groups go first
    /// (`State::eviction_key`); preferred groups survive.
    pub fn set_streaming_limit(&self, bytes: usize) -> Result<()> {
        self.lock()?.streaming_limit = bytes;
        Ok(())
    }

    /// Clear only the legacy idle-cache window. This is intent metadata: no CUDA call or
    /// reclaim occurs, and process managed/development/peer capacity limits remain in force.
    pub fn clear_streaming_limit(&self) -> Result<()> {
        self.lock()?.streaming_limit = usize::MAX;
        Ok(())
    }

    /// Actual current policy, for a scoped caller to preserve and conditionally restore.
    pub fn streaming_limit(&self) -> Result<usize> {
        Ok(self.lock()?.streaming_limit)
    }

    /// Enter an authored component phase without allocating, evicting or changing its
    /// admitted window. Only incoming bytes actually Ready now receive preference.
    pub fn enter_phase(&self, incoming: &[Arc<Region>]) -> Result<()> {
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, self.device)?;
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
        state.phase_boundary = (state.clock != u64::MAX).then_some(state.clock);
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
        self.reclaim_with_preference(bytes, true, ReclaimReason::Requested)
    }

    /// Start a phase: earlier ready cache becomes standby, still reclaimable under pressure.
    /// This changes metadata only, never physical/VA ownership. A saturated clock falls back
    /// to the global streaming bound because later touches cannot establish a new phase.
    pub fn reset_preferences(&self) -> Result<()> {
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, self.device)?;
        let mut state = self.lock()?;
        state.preferred_end = None;
        state.phase_boundary = (state.clock != u64::MAX).then_some(state.clock);
        for entry in state.entries.values_mut() {
            entry.preferred = false;
        }
        Ok(())
    }

    /// Make phase headroom without evicting the incoming phase's preferred cached bytes.
    pub fn reclaim_unpreferred(&self, bytes: usize) -> Result<ReclaimReport> {
        self.reclaim_with_preference(bytes, false, ReclaimReason::Requested)
    }

    fn reclaim_with_preference(
        &self,
        bytes: usize,
        preferred: bool,
        reason: ReclaimReason,
    ) -> Result<ReclaimReport> {
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, self.device)?;
        let mut state = self.lock()?;
        let _context = self.driver.enter()?;
        self.reclaim_locked(&mut state, bytes, preferred, None, reason)
    }

    /// One strictly nonwaiting pass. Caller holds capture exclusion and retained Arena
    /// ownership; cold/retired backing is still ineligible when Pending or Unjoined.
    fn reclaim_ready_counted(&self, captures: &BTreeMap<i32, DeviceActivity>, wanted: usize,
                            maximum: usize, reason: ReclaimReason, completed: &mut usize)
        -> Result<ReclaimReport> {
        ensure_recovery_allowed(captures, self.device)?;
        let mut state = self.lock()?;
        let result = (|| {
            let _context = self.driver.enter()?;
            let mut report = ReclaimReport { requested_bytes: wanted, ..ReclaimReport::default() };
            let contentless = state.contentless_candidates(true, None);
            let candidates = state.reclaim_candidates(true, None, reason);
            for id in &contentless {
                if report.reclaimed_bytes >= wanted { break; }
                if state.entries[id].bytes <= maximum.saturating_sub(report.reclaimed_bytes)
                    && self.completion(&mut state, *id)? == Completion::Done {
                    self.evict_counted(&mut state, *id, reason, &mut report, completed)?;
                }
            }
            for (id, preferred, _) in candidates {
                if report.reclaimed_bytes >= wanted { break; }
                if contentless.contains(&id) { continue; }
                if state.entries[&id].bytes > maximum.saturating_sub(report.reclaimed_bytes) { continue; }
                if self.completion(&mut state, id)? == Completion::Done {
                    self.evict_counted(&mut state, id, reason, &mut report, completed)?;
                } else if preferred {
                    break; // Do not jump a pending/unjoined preferred suffix or wait for it.
                }
            }
            Ok(report)
        })();
        result.map_err(|error| poison(&mut state, error))
    }

    fn reclaim_locked(
        &self,
        state: &mut State,
        bytes: usize,
        preferred: bool,
        exclude: Option<u64>,
        reason: ReclaimReason,
    ) -> Result<ReclaimReport> {
        let mut report = ReclaimReport {
            requested_bytes: bytes,
            ..ReclaimReport::default()
        };
        if bytes == 0 {
            return Ok(report);
        }
        self.reclaim_contentless(state, bytes, preferred, exclude, reason, &mut report)?;
        if report.reclaimed_bytes >= bytes {
            return Ok(report);
        }
        let candidates = state.reclaim_candidates(preferred, exclude, reason);
        // The arena lock excludes new pins; each candidate already has zero physical pins.
        // Idle cache goes in `eviction_key` order, completed-first, then after its own events,
        // and joins the context only for unfenced releases. The preferred suffix keeps strict
        // order, since each of its evictions permanently shortens the retained prefix.
        let idle = candidates.partition_point(|(_, preferred, _)| !preferred);
        let (mut pending, mut unjoined) = (Vec::new(), Vec::new());
        for &(id, _, _) in &candidates[..idle] {
            if report.reclaimed_bytes >= bytes {
                return Ok(report);
            }
            match self.completion(state, id)? {
                Completion::Done => self.evict(state, id, reason, &mut report)?,
                Completion::Pending => pending.push(id),
                Completion::Unjoined => unjoined.push(id),
            }
        }
        for id in pending {
            if report.reclaimed_bytes >= bytes {
                return Ok(report);
            }
            self.wait_fences(state, id)?;
            self.evict(state, id, reason, &mut report)?;
        }
        if report.reclaimed_bytes < bytes && !unjoined.is_empty() {
            self.join_context(state, &unjoined)?;
            for id in unjoined {
                if report.reclaimed_bytes >= bytes {
                    return Ok(report);
                }
                self.evict(state, id, reason, &mut report)?;
            }
        }
        let kept: Vec<_> = candidates[idle..].iter().map(|(id, _, _)| *id).collect();
        for (index, &id) in kept.iter().enumerate() {
            if report.reclaimed_bytes >= bytes {
                break;
            }
            match self.completion(state, id)? {
                Completion::Done => {}
                Completion::Pending => self.wait_fences(state, id)?,
                // One join completes every later candidate's work as well.
                Completion::Unjoined => self.join_context(state, &kept[index..])?,
            }
            self.evict(state, id, reason, &mut report)?;
        }
        Ok(report)
    }

    /// Evict idle, non-preferred regions in `State::eviction_key` order until `bytes` are free:
    /// completed ones first, then, if they together free enough, ones whose own last-consumer
    /// events are pending, after those events. Never joins the context.
    fn reclaim_completed_locked(
        &self,
        state: &mut State,
        bytes: usize,
        exclude: u64,
        reason: ReclaimReason,
    ) -> Result<ReclaimReport> {
        let mut report = ReclaimReport {
            requested_bytes: bytes,
            ..ReclaimReport::default()
        };
        // Backing nothing needs goes before any Ready page here too, as in `reclaim_contentless`.
        let mut candidates: Vec<_> = state
            .entries
            .iter()
            .filter(|(id, entry)| {
                **id != exclude
                    && entry.uses.pins() == 0
                    && !matches!(entry.uses.state(), Contents::Loading | Contents::Published)
                    && entry.handle.is_some()
                    && !entry.preferred()
                    && (!matches!(reason, ReclaimReason::StreamingLimit) || !state.standby(entry))
            })
            .map(|(id, entry)| {
                let ready = !matches!(entry.uses.state(), Contents::Cold | Contents::Retired);
                (*id, (ready, state.eviction_key(entry)))
            })
            .collect();
        candidates.sort_by_key(|(_, key)| *key);
        let (mut pending, mut held) = (Vec::new(), 0);
        for (id, _) in candidates {
            if report.reclaimed_bytes >= bytes {
                return Ok(report);
            }
            match self.completion(state, id)? {
                Completion::Done => self.evict(state, id, reason, &mut report)?,
                Completion::Pending => {
                    held += state.entries[&id].bytes;
                    pending.push(id);
                }
                Completion::Unjoined => {}
            }
        }
        // A loop's newest idle block is often still being read when the next one is prefetched
        // (Qwen-Image int8 on 8 GB, once a step). Refused, the prefetch emptied Torch's cache to
        // retry; waiting for that block's own events is shorter than that full synchronize.
        if report.reclaimed_bytes + held < bytes {
            return Ok(report);
        }
        for id in pending {
            if report.reclaimed_bytes >= bytes {
                break;
            }
            self.wait_fences(state, id)?;
            self.evict(state, id, reason, &mut report)?;
        }
        Ok(report)
    }

    /// Backing whose contents nothing needs (a canceled or failed fill, a dropped region) goes
    /// before any Ready page, oldest first, waiting or joining for it if it must. Reclaim order
    /// otherwise takes every completed page first, so unfenced leftovers outlived resident
    /// weights (SDXL int8 at 6 GiB free: 618 MiB of canceled prefetches cost 656 MiB of UNet
    /// every request).
    fn reclaim_contentless(
        &self,
        state: &mut State,
        bytes: usize,
        preferred: bool,
        exclude: Option<u64>,
        reason: ReclaimReason,
        report: &mut ReclaimReport,
    ) -> Result<()> {
        let idle = state.contentless_candidates(preferred, exclude);
        let mut unjoined = Vec::new();
        for id in idle {
            if report.reclaimed_bytes >= bytes {
                return Ok(());
            }
            match self.completion(state, id)? {
                Completion::Done => self.evict(state, id, reason, report)?,
                Completion::Pending => {
                    self.wait_fences(state, id)?;
                    self.evict(state, id, reason, report)?;
                }
                Completion::Unjoined => unjoined.push(id),
            }
        }
        if report.reclaimed_bytes < bytes && !unjoined.is_empty() {
            self.join_context(state, &unjoined)?;
            for id in unjoined {
                if report.reclaimed_bytes >= bytes {
                    break;
                }
                self.evict(state, id, reason, report)?;
            }
        }
        Ok(())
    }

    /// Whether every consumer of an unpinned region is known complete, without waiting.
    fn completion(&self, state: &mut State, id: u64) -> Result<Completion> {
        let uses = Arc::clone(&state.entries[&id].uses);
        if uses.unjoined.load(Ordering::Acquire) {
            return Ok(Completion::Unjoined);
        }
        for fence in uses.fences().iter() {
            match self.driver.query(fence.event) {
                Ok(true) => {}
                Ok(false) => return Ok(Completion::Pending),
                Err(error) => return Err(poison(state, error)),
            }
        }
        Ok(Completion::Done)
    }

    /// Host-wait only this region's own last-consumer events, never unrelated work.
    fn wait_fences(&self, state: &mut State, id: u64) -> Result<()> {
        let uses = Arc::clone(&state.entries[&id].uses);
        for fence in uses.fences().iter() {
            if let Err(error) = self.driver.wait_event(fence.event) {
                return Err(poison(state, error));
            }
            state.event_waits = state.event_waits.saturating_add(1);
        }
        Ok(())
    }

    /// The conservative fence for releases that recorded no consumer event.
    fn join_context(&self, state: &mut State, ids: &[u64]) -> Result<()> {
        if let Err(error) = self.driver.synchronize() {
            return Err(poison(state, error));
        }
        state.context_joins = state.context_joins.saturating_add(1);
        for id in ids {
            state.entries[id]
                .uses
                .unjoined
                .store(false, Ordering::Release);
        }
        Ok(())
    }

    /// Unmap and release one joined, unpinned candidate.
    fn evict(
        &self,
        state: &mut State,
        id: u64,
        reason: ReclaimReason,
        report: &mut ReclaimReport,
    ) -> Result<()> {
        self.evict_counted(state, id, reason, report, &mut 0)
    }

    // Record the physical effect before any later fallible address retirement/reporting.
    fn evict_counted(&self, state: &mut State, id: u64, reason: ReclaimReason,
        report: &mut ReclaimReport, completed: &mut usize) -> Result<()> {
        let was_preferred = state.entries[&id].preferred();
        let (size, retired) = self.release_backing_counted(state, id, completed)?;
        if was_preferred && !retired {
            // Pressure permanently shortens the retained prefix. Later scans cannot rebuild it.
            state.preferred_end = Some(state.preferred_end.map_or(id, |end| end.min(id)));
            for (_, later) in state.entries.range_mut(id..) {
                later.preferred = false;
            }
        }
        report.reclaimed_bytes += size;
        report.evicted_regions += 1;
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
        self.room.released.fetch_add(size as u64, Ordering::AcqRel);
        let entry = state.entries.get_mut(&id).expect("locked candidate");
        // Every event is complete here; a destroy failure only leaks the event.
        for fence in entry.uses.fences().drain(..) {
            let _ = self.driver.destroy_event(fence.event);
        }
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
            reclaimed_by_reason: state.reclaimed_by_reason,
            context_joins: state.context_joins,
            driver_physical_ooms: self.driver.physical_ooms(),
            event_waits: state.event_waits,
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

    // Caller holds capture exclusion. Overflow latches the device epoch unusable,
    // including on these destructor paths where an error cannot be returned.
    fn retire_intent(&self) {
        if self.uses.state() != Contents::Retired {
            let _ = plan::inventory_changed(self.arena.device);
            self.uses.set(Contents::Retired);
        }
    }

    pub fn retire(self) -> RetiredRegion {
        if self.arena.pid == std::process::id() {
            // Frontends may call this while holding their GIL/owner lock, before
            // handing RetiredRegion to deferred cleanup. Never wait on a GPU teardown
            // that holds capture exclusion; its eventual Drop linearizes retirement.
            if let Some(captures) = CAPTURES.get()
                && let Ok(_captures) = captures.try_lock()
            {
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

    /// Record this region's reusable last-consumer event on each stream. Never waits.
    fn record_fences(&self, streams: &[u64], submission: Option<&async_fill::Control>) -> Result<()> {
        // Only the new ordered-pin route needs this stronger publication/fence exclusion;
        // ordinary completed pins keep their qualified release path unchanged.
        let captures = submission.map(|_| capture_counts()).transpose()?;
        if let Some(captures) = &captures {
            ensure_not_capturing(captures, self.device())?;
        }
        let ordered = submission.is_some_and(|control| {
            control.generation == self.uses.generation.load(Ordering::Acquire)
                && self.uses.state() == Contents::Published
        });
        if streams.is_empty()
            || (self.uses.state() != Contents::Ready && !ordered)
            || self.arena.captures.load(Ordering::Acquire) != 0
        {
            return Err(Error::Invalid("release has no recordable consumer event"));
        }
        let driver = &self.arena.driver;
        let _context = driver.enter()?;
        let mut fences = self.uses.fences();
        for &stream in streams {
            let stream = stream as usize;
            // A record inside capture would be a graph node, not a completion fence.
            if driver.is_capturing(stream)? {
                return Err(Error::CaptureActive);
            }
            let known = fences
                .iter()
                .find(|fence| fence.stream == stream)
                .map(|fence| fence.event);
            let event = match known {
                Some(event) => event,
                None => driver.create_event()?,
            };
            if let Err(error) = driver.record(event, stream) {
                if known.is_none() {
                    let _ = driver.destroy_event(event);
                }
                return Err(error);
            }
            if known.is_none() {
                fences.push(Fence { stream, event });
            }
        }
        Ok(())
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
    /// during a capture or replay; while an earlier release is unfenced they stay (false) for
    /// reclamation, which takes them before any Ready page (`reclaim_contentless`).
    pub fn release_unready(&self) -> Result<bool> {
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, self.arena.device)?;
        let mut state = self.arena.lock()?;
        let Some(entry) = state.entries.get(&self.id) else {
            return Ok(false);
        };
        if entry.handle.is_none()
            || entry.uses.pins() != 0
            || !matches!(entry.uses.state(), Contents::Cold | Contents::Retired)
            || self.arena.completion(&mut state, self.id)? != Completion::Done
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

    pub fn prepare(self: &Arc<Self>) -> Result<Preparation> {
        self.prepare_with_reclaim(Admission::Any)
    }

    /// Admit speculative bytes only if no eviction or prior-writer join is required.
    /// Capacity refusal leaves existing ready contents and the preference watermark intact.
    pub fn prepare_without_reclaim(self: &Arc<Self>) -> Result<Preparation> {
        self.prepare_with_reclaim(Admission::None)
    }

    /// Admit a streamed block's read ahead of its use: it may evict idle streamed regions
    /// (`State::eviction_key` order), completed ones first, then ones whose own last-consumer
    /// events are pending, after waiting on those events; it never joins. The retained prefix
    /// stays; otherwise this refuses like `prepare_without_reclaim`.
    pub fn prepare_evicting_completed(self: &Arc<Self>) -> Result<Preparation> {
        self.prepare_with_reclaim(Admission::Completed)
    }

    /// Optional immutable raw prefetch. Unknown/unqualified phase configurations decline
    /// without IO or reclaim. The native check and page admission are one locked operation.
    pub fn prepare_prefetch(self: &Arc<Self>) -> Result<Option<Preparation>> {
        let captures = capture_counts()?;
        if ensure_recovery_allowed(&captures, self.arena.device).is_err() {
            return Ok(None);
        }
        let mut state = self.arena.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if entry.uses.state() == Contents::Ready {
            return Ok(None); // Already ready: no speculative Pin or source worker is needed.
        }
        let bytes = if entry.handle.is_some() { 0 } else { self.rounded };
        let epoch = captures.get(&self.arena.device).map_or(0, |activity| activity.activity_epoch);
        working::admit_prefetch(self.arena.device, epoch, bytes, || {
            self.prepare_locked(Admission::Prefetch, &captures, &mut state, &mut None)
        })
    }

    /// Atomically admit all missing regions of one immutable raw stage, without reclaim.
    /// The source adapter has not issued writes or exported these new Fill pins yet.
    #[allow(unsafe_code)]
    pub fn prepare_replay_prefetch(
        regions: &[Arc<Self>],
        capability: &working::ReplayPrefetch,
    ) -> Result<Option<Vec<Preparation>>> {
        let Some(first) = regions.first() else {
            return Ok(None);
        };
        if regions
            .iter()
            .any(|region| !Arc::ptr_eq(&region.arena, &first.arena))
        {
            return Ok(None); // Mixed arenas keep ordinary demand loading; no speculative partial group.
        }
        let mut ids = std::collections::BTreeSet::new();
        if regions.iter().any(|region| !ids.insert(region.id)) {
            return Err(Error::Invalid("raw prefetch regions must be distinct"));
        }
        let captures = capture_counts()?;
        if ensure_not_capturing(&captures, first.device()).is_err() {
            return Ok(None);
        }
        let arena = &first.arena;
        let mut state = arena.lock()?;
        let _context = arena.driver.enter()?;
        let mut missing = 0usize;
        let mut cold = false;
        for region in regions {
            let entry = state
                .entries
                .get(&region.id)
                .expect("source owns its region");
            if entry.uses.state() == Contents::Ready {
                continue;
            }
            if matches!(entry.uses.state(), Contents::Loading | Contents::Published) || entry.uses.pins() != 0 {
                return Ok(None);
            }
            cold = true;
            if entry.handle.is_none() {
                let Some(total) = missing.checked_add(region.rounded) else {
                    return Ok(None);
                };
                missing = total;
            } else if arena.completion(&mut state, region.id)? != Completion::Done {
                return Ok(None); // A speculative read never waits for an earlier writer/consumer.
            }
        }
        if !cold {
            return Ok(None);
        }
        working::admit_replay_prefetch(first.device(), capability, missing, || {
            let mut prepared = Vec::with_capacity(regions.len());
            let mut created = Vec::new();
            for region in regions {
                let fresh = state.entries[&region.id].handle.is_none();
                match region.prepare_locked(Admission::Prefetch, &captures, &mut state, &mut None) {
                    Ok(value) => {
                        if fresh {
                            created.push(region.id);
                        }
                        prepared.push(value);
                    }
                    Err(error) => {
                        // No producer or consumer has seen these pins. Abort the complete
                        // admitted prefix before returning a decline; no partial IO starts.
                        for value in prepared {
                            match value {
                                Preparation::Fill(fill) => {
                                    // SAFETY: group admission has issued no writes or exports.
                                    drop(unsafe { fill.abort_completed() });
                                }
                                Preparation::Resident(pin) => {
                                    // SAFETY: this temporary pin was never handed to a consumer.
                                    drop(unsafe { pin.into_completed_region() });
                                }
                            }
                        }
                        if matches!(
                            error,
                            Error::OutOfMemory { .. } | Error::Busy | Error::CaptureActive
                        ) {
                            for id in created {
                                arena.release_backing(&mut state, id)?;
                            }
                        }
                        // Ambiguous device faults retain tracked mappings in the poisoned arena.
                        return Err(error);
                    }
                }
            }
            Ok(prepared)
        })
    }

    fn prepare_with_reclaim(self: &Arc<Self>, admission: Admission) -> Result<Preparation> {
        let result = (|| {
            let mut recovery = Recovery::default();
            loop {
                let mut physical = None;
                let result = {
                    let captures = capture_counts()?;
                    let mut state = self.arena.lock()?;
                    self.prepare_locked(admission, &captures, &mut state, &mut physical)
                };
                if admission == Admission::Any
                    && let Err(Error::OutOfMemory { requested_bytes }) = &result
                    && recovery.next(self.arena.device, physical.unwrap_or(*requested_bytes), physical.is_some())?
                {
                    continue; // Revalidate admission after peer progress, outside every guard.
                }
                return result;
            }
        })();
        if result.is_err() {
            let _ = working::invalidate_current_phase(self.arena.device, "weight_admission_error");
        }
        result
    }

    fn prepare_locked(
        self: &Arc<Self>,
        admission: Admission,
        captures: &BTreeMap<i32, DeviceActivity>,
        state: &mut State,
        physical: &mut Option<usize>,
    ) -> Result<Preparation> {
        let reclaim = admission == Admission::Any;
        let entry = state.entries.get(&self.id).expect("live region");
        if matches!(entry.uses.state(), Contents::Loading | Contents::Published) {
            return Err(Error::Busy);
        }
        if entry.uses.state() == Contents::Ready {
            state.hits = state.hits.saturating_add(1);
            return Ok(Preparation::Resident(self.pin_locked(state)?));
        }
        // A failed fill can leave escaped foreign storage. It owns these bytes until destruction.
        if entry.uses.pins() != 0 {
            return Err(Error::Busy);
        }
        if reclaim {
            ensure_recovery_allowed(captures, self.arena.device)?;
        } else {
            ensure_not_capturing(captures, self.arena.device)?;
        }
        let _context = self.arena.driver.enter()?;
        let has_handle = entry.handle.is_some();
        if !has_handle {
            if admission != Admission::Prefetch && !entry.preferred() && state.streaming_limit != usize::MAX {
                let resident = state.streaming_bytes();
                let limit = state.streaming_limit.max(self.rounded);
                let excess = resident.saturating_add(self.rounded).saturating_sub(limit);
                match admission {
                    Admission::Any => {
                        self.arena.reclaim_locked(
                            state,
                            excess,
                            false,
                            Some(self.id),
                            ReclaimReason::StreamingLimit,
                        )?;
                    }
                    Admission::Completed => {
                        self.arena.reclaim_completed_locked(
                            state,
                            excess,
                            self.id,
                            ReclaimReason::StreamingLimit,
                        )?;
                    }
                    Admission::None | Admission::Prefetch if excess != 0 => {
                        return Err(Error::OutOfMemory {
                            requested_bytes: self.rounded,
                        });
                    }
                    Admission::None | Admission::Prefetch => {}
                }
                let remaining = state.streaming_bytes();
                if remaining.saturating_add(self.rounded) > limit {
                    return Err(Error::OutOfMemory {
                        requested_bytes: self.rounded,
                    });
                }
            }
            let allocation = match self.arena.create(self.rounded) {
                Err(error) if error.is_oom() && matches!(admission, Admission::Any | Admission::Completed) => {
                    working::reclaim_for_weights(self.arena.device, self.rounded)?;
                    self.arena.create(self.rounded)
                }
                result => result,
            };
            let handle = match allocation {
                Ok(handle) => handle,
                Err(error) if error.is_oom() => {
                    match admission {
                        Admission::None | Admission::Prefetch => {
                            return Err(Error::OutOfMemory {
                                requested_bytes: self.rounded,
                            });
                        }
                        Admission::Completed => {
                            self.arena.reclaim_completed_locked(
                                state,
                                self.rounded,
                                self.id,
                                ReclaimReason::WeightAllocation,
                            )?;
                        }
                        Admission::Any => {
                            self.arena.reclaim_locked(
                                state,
                                self.rounded,
                                false,
                                Some(self.id),
                                ReclaimReason::WeightAllocation,
                            )?;
                        }
                    }
                    match self.arena.create(self.rounded) {
                        Ok(handle) => handle,
                        Err(error) if error.is_oom() => {
                            *physical = error.physical_bytes();
                            return Err(error.into());
                        }
                        Err(error) => return Err(poison(state, error.into())),
                    }
                }
                Err(error) => return Err(poison(state, error.into())),
            };
            // Record custody before map. Even an ambiguous map failure keeps both handle and VA.
            state.entries.get_mut(&self.id).expect("live region").handle = Some(handle);
            if let Err(error) = self.arena.driver.map(self.ptr, self.rounded, handle) {
                return Err(poison(state, error));
            }
        } else {
            // An abandoned external fill may still have queued writes. Only a checked
            // native last-use release can remove this join; ordinary storage remains guarded.
            match (admission, self.arena.completion(state, self.id)?) {
                (_, Completion::Done) if admission != Admission::None => {}
                (Admission::Any, Completion::Pending) => {
                    self.arena.wait_fences(state, self.id)?
                }
                (Admission::Any, Completion::Unjoined) => {
                    self.arena.join_context(state, &[self.id])?
                }
                // Even an abandoned fill with no remaining pins may have queued writers.
                _ => {
                    return Err(Error::OutOfMemory {
                        requested_bytes: self.rounded,
                    });
                }
            }
        }
        self.uses.generation.try_update(Ordering::AcqRel, Ordering::Acquire,
            |generation| generation.checked_add(1))
            .map_err(|_| Error::Invalid("fill generation overflow"))?;
        *self.uses.submission.lock().map_err(|_| Error::Poisoned("fill submission mutex".into()))? = None;
        let pin = self.pin_locked(state)?;
        self.uses.set(Contents::Loading);
        Ok(Preparation::Fill(Fill {
            pin: Some(pin),
            copied: false,
        }))
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
        state.clock = state.clock.saturating_add(1);
        let entry = state.entries.get_mut(&self.id).expect("live region");
        entry
            .uses
            .pins
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Invalid("pin count overflow"))?;
        entry.last_use = state.clock;
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
    copied: bool,
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

    pub fn copy_from_host(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() != self.len() {
            return Err(Error::Invalid("fill must cover the whole logical region"));
        }
        self.copied = false;
        let region = self.region();
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, region.arena.device)?;
        let mut state = region.arena.lock()?;
        let _context = region.arena.driver.enter()?;
        if let Err(error) = region.arena.driver.copy_from_host(region.ptr, bytes) {
            return Err(poison(&mut state, error));
        }
        drop(state);
        drop(_context);
        self.copied = true;
        Ok(())
    }

    pub fn commit(self) -> Result<Pin> {
        if !self.copied {
            return Err(Error::Invalid("fill has not initialized the entire region"));
        }
        self.finish(true)
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
        let captures = capture_counts()?;
        if join {
            ensure_recovery_allowed(&captures, region.arena.device)?;
        } else {
            ensure_not_capturing(&captures, region.arena.device)?;
        }
        let mut state = region.arena.lock()?;
        let _context = region.arena.driver.enter()?;
        if join && let Err(error) = region.arena.driver.synchronize() {
            return Err(poison(&mut state, error));
        }
        let entry = state.entries.get_mut(&region.id).expect("fill owns region");
        entry.uses.set(Contents::Ready);
        state.fills = state.fills.saturating_add(1);
        drop(state);
        drop(_context);
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
            region.uses.unjoined.store(true, Ordering::Release);
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

    /// End this use after recording a Varena-owned last-consumer event on each stream.
    /// Reclaim then waits on those events, never the context. With no stream, unready
    /// contents, active capture or any driver failure, this is `into_unpinned_region`.
    /// Never waits, so storage deleters may call it.
    ///
    /// # Safety
    /// Every consumer of this pin, including escaped aliases sharing it, submitted all of its
    /// work to these streams (or joined it into one of them) and submits nothing after this
    /// call. Each stream is live and belongs to this device's primary context.
    #[allow(unsafe_code)]
    pub unsafe fn into_fenced_region(mut self, streams: &[u64]) -> Arc<Region> {
        let region = self.region.take().expect("live pin");
        if region.arena.pid == std::process::id() {
            if region.record_fences(streams, self.submission.as_deref()).is_err() {
                region.uses.unjoined.store(true, Ordering::Release);
            }
            decrement(&region.uses.pins);
        }
        region
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        if let Some(region) = &self.region
            && region.arena.pid == std::process::id()
        {
            region.uses.unjoined.store(true, Ordering::Release);
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
        let Ok(captures) = capture_counts() else {
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
        if entry.handle.is_some() || ensure_recovery_allowed(&captures, self.arena.device).is_err()
        {
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
        if self.pid != std::process::id() {
            return;
        }
        let Ok(captures) = capture_counts() else {
            return;
        };
        if let Some(registry) = ARENAS.get() {
            let Ok(mut registry) = registry.lock() else { return; };
            if let Some(index) = registry.iter().position(|entry| entry.id == self.id) {
                let _ = plan::inventory_changed(self.device);
                registry.remove(index);
            }
        }
        if ensure_recovery_allowed(&captures, self.device).is_err() {
            return;
        }
        let Ok(state) = self.state.get_mut() else {
            return;
        };
        if state.poison.is_some() {
            return;
        }
        let Ok(_context) = self.driver.enter() else {
            return;
        };
        if self.driver.synchronize().is_err() {
            return;
        }
        for entry in state.entries.values() {
            // Last Arc<Arena> means no Region, Pin, Fill, or CaptureGuard can still exist.
            for fence in entry.uses.fences().drain(..) {
                let _ = self.driver.destroy_event(fence.event);
            }
            if let Some(handle) = entry.handle {
                if self.driver.unmap(entry.ptr, entry.bytes).is_err() {
                    return;
                }
                if self.driver.release(handle).is_err() {
                    return;
                }
                self.room.pages.fetch_sub(entry.bytes, Ordering::AcqRel);
                self.room.released.fetch_add(entry.bytes as u64, Ordering::AcqRel);
            }
            if self.driver.free_address(entry.ptr, entry.bytes).is_err() {
                return;
            }
        }
    }
}

fn room(device: i32) -> Result<Arc<Room>> {
    Ok(Arc::clone(&capture_counts()?.entry(device).or_default().room))
}

/// Host-only cumulative physical-release observation. Wrap is handled conservatively by
/// request-local checked subtraction; queued owner actions never change this counter.
pub fn released_bytes(device: i32) -> Result<u64> {
    Ok(room(device)?.released.load(Ordering::Acquire))
}

fn reclaim_completed(device: i32, wanted: usize, retained: &mut Vec<Arc<Arena>>) -> Result<usize> {
    // The callback worker retains observed Arena leases exactly as Peer does. Otherwise a
    // temporary snapshot could become the last owner and context-join from this no-wait pass.
    retain_arenas(device, retained)?;
    let working = working::reclaim_completed(device, wanted)?;
    let mut completed = working;
    if completed < wanted {
        let captures = capture_counts()?;
        ensure_recovery_allowed(&captures, device)?;
        for arena in retained.iter() {
            if completed >= wanted { break; }
            arena.reclaim_ready_counted(&captures, wanted - completed, usize::MAX,
                ReclaimReason::Allocator, &mut completed)?;
        }
    }
    Ok(completed)
}

fn retain_arenas(device: i32, retained: &mut Vec<Arc<Arena>>) -> Result<()> {
    for arena in arena_leases(device)? {
        if !retained.iter().any(|held| Arc::ptr_eq(held, &arena)) { retained.push(arena); }
    }
    Ok(())
}

/// The room this process may hold on `device` (weight pages plus mapped working allocations),
/// None for no limit, and at once its pages above it back, in reclaim order (pins are never
/// overridden; under a capture or replay they wait for the next fill or allocation, which the
/// room bounds from now on). What a machine asks of an executor when another program needs the
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
    if over == 0 || is_recovery_blocked(device)? {
        return Ok(report);
    }
    report.reclaimed_bytes += working::trim(device)?;
    for arena in arena_leases(device)? {
        let need = room.over(0);
        if need == 0 {
            break;
        }
        let next = arena.reclaim_with_preference(need, true, ReclaimReason::Room)?;
        report.reclaimed_bytes += next.reclaimed_bytes;
        report.evicted_regions += next.evicted_regions;
    }
    Ok(report)
}

/// `make_room`'s current limit on `device`, None for none.
pub fn room_limit(device: i32) -> Result<Option<usize>> {
    let limit = room(device)?.limit.load(Ordering::Acquire);
    Ok((limit != usize::MAX).then_some(limit))
}

/// Allocator-pressure entry point: one native pass, with no callbacks. It may wait for
/// submitted consumer fences or join unannotated GPU work; owner collection instead uses
/// the separate strictly ready-only pass. Actual reclaimed bytes are observations, not grants.
pub fn reclaim(device: i32, bytes: usize) -> Result<ReclaimReport> {
    check_process()?;
    if is_recovery_blocked(device)? {
        return Err(Error::CaptureActive);
    }
    let mut report = ReclaimReport {
        requested_bytes: bytes,
        ..ReclaimReport::default()
    };
    for arena in arena_leases(device)? {
        if report.reclaimed_bytes >= bytes {
            break;
        }
        let next = arena.reclaim_with_preference(
            bytes - report.reclaimed_bytes,
            true,
            ReclaimReason::Allocator,
        )?;
        report.reclaimed_bytes += next.reclaimed_bytes;
        report.evicted_regions += next.evicted_regions;
    }
    Ok(report)
}

/// Physical bytes owned by live arenas on this device, for a local managed-memory ceiling.
pub fn resident_bytes(device: i32) -> Result<usize> {
    check_process()?;
    arena_leases(device)?.into_iter()
        .try_fold(0usize, |bytes, arena| {
            bytes
                .checked_add(arena.stats()?.resident_bytes)
                .ok_or(Error::Invalid("resident byte count overflow"))
        })
}
