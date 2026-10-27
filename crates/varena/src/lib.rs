//! Process-local weight residency. TensorFS supplies bytes; this crate owns only CUDA memory.
//!
//! Every foreign storage must retain its [`Pin`] until the final alias is destroyed. Raw addresses
//! are not ownership. Kernels may finish after the final storage disappears, so reclamation joins
//! the primary CUDA context before unmapping unless every pin was released by a native integration
//! after checked completion, or with last-consumer events on its declared streams, which reclaim
//! waits on instead. Capture/replay roots retain pins for their lifetime by default.
//! No imported memory, IPC, background work, file I/O, or Python callbacks occur here.
#![deny(unsafe_code)]

mod cuda;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("CUDA capacity exhausted allocating {requested_bytes} bytes")]
    OutOfMemory { requested_bytes: usize },
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
}

#[derive(Clone, Copy)]
enum ReclaimReason {
    Requested,
    StreamingLimit,
    WeightAllocation,
    Allocator,
}

impl ReclaimedBytes {
    fn add(&mut self, reason: ReclaimReason, bytes: usize) {
        let counter = match reason {
            ReclaimReason::Requested => &mut self.requested,
            ReclaimReason::StreamingLimit => &mut self.streaming_limit,
            ReclaimReason::WeightAllocation => &mut self.weight_allocation,
            ReclaimReason::Allocator => &mut self.allocator,
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
}

#[derive(Clone, Copy)]
struct Fence {
    stream: usize,
    event: usize,
}

/// What a fill may do for room: anything (`Any`), nothing (`None`), or evict only idle
/// streamed regions whose consumers are already complete (`Completed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    Any,
    Completed,
    None,
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
    next_id: u64,
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

    fn streaming_bytes(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| {
                !entry.preferred()
                    && entry.handle.is_some()
                    && entry.uses.pins() == 0
                    && entry.uses.state() != Contents::Loading
                    && !self.standby(entry)
            })
            .map(|entry| entry.bytes)
            .sum()
    }
}

/// A manager owned by one executor. It may serve multiple threads of that process.
pub struct Arena {
    driver: cuda::Driver,
    device: i32,
    pid: u32,
    // Lock-free view of this device's capture guards, for release paths that never wait.
    captures: Arc<AtomicUsize>,
    state: Mutex<State>,
}

static ARENAS: OnceLock<Mutex<Vec<Weak<Arena>>>> = OnceLock::new();
#[derive(Default)]
struct DeviceActivity {
    captures: Arc<AtomicUsize>,
    replays: Arc<AtomicUsize>,
    producers: Arc<AtomicUsize>,
}

static CAPTURES: OnceLock<Mutex<BTreeMap<i32, DeviceActivity>>> = OnceLock::new();
static PROCESS: AtomicU32 = AtomicU32::new(0);

fn check_process() -> Result<()> {
    let pid = std::process::id();
    let owner = PROCESS
        .compare_exchange(0, pid, Ordering::Relaxed, Ordering::Relaxed)
        .unwrap_or_else(|owner| owner);
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
    if activity.replays.load(Ordering::Acquire) != 0 {
        return Err(Error::CaptureActive);
    }
    if activity.producers.load(Ordering::Acquire) != 0 {
        return Err(Error::Invalid(
            "native producers must finish before CUDA graph capture",
        ));
    }
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
pub fn enter_replay(device: i32) -> Result<GlobalCaptureGuard> {
    let mut counts = capture_counts()?;
    ensure_not_capturing(&counts, device)?;
    let activity = counts.entry(device).or_default();
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
        let mut captures = capture_counts()?;
        ensure_recovery_allowed(&captures, device)?;
        let driver = cuda::Driver::open(device)?;
        let capturing = Arc::clone(&captures.entry(device).or_default().captures);
        drop(captures);
        let arena = Arc::new(Self {
            driver,
            device,
            pid: std::process::id(),
            captures: capturing,
            state: Mutex::new(State {
                entries: BTreeMap::new(),
                next_id: 1,
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
        let mut registry = ARENAS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
        registry.retain(|entry| entry.strong_count() != 0);
        registry.push(Arc::downgrade(&arena));
        Ok(arena)
    }

    pub fn device(&self) -> i32 {
        self.device
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
        let id = state.next_id;
        state.next_id = id
            .checked_add(1)
            .ok_or(Error::Invalid("region id overflow"))?;
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
    /// A single larger group is permitted. Older idle groups go first; preferred groups survive.
    pub fn set_streaming_limit(&self, bytes: usize) -> Result<()> {
        self.lock()?.streaming_limit = bytes;
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
        if incoming.iter().any(|region| !std::ptr::eq(self, Arc::as_ptr(&region.arena))) {
            return Err(Error::Invalid("phase regions must belong to the same arena"));
        }
        let mut state = self.lock()?;
        state.preferred_end = None;
        state.phase_boundary = (state.clock != u64::MAX).then_some(state.clock);
        for entry in state.entries.values_mut() {
            entry.preferred = false;
        }
        for region in incoming {
            let entry = state.entries.get_mut(&region.id).ok_or(Error::Invalid("phase region is retired"))?;
            entry.preferred = entry.handle.is_some() && entry.uses.state() == Contents::Ready;
        }
        Ok(())
    }

    pub fn owns_region(&self, region: &Region) -> bool {
        std::ptr::eq(self, Arc::as_ptr(&region.arena))
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
        let mut candidates: Vec<_> = state
            .entries
            .iter()
            .filter(|(id, entry)| {
                Some(**id) != exclude
                    && entry.uses.pins() == 0
                    && entry.uses.state() != Contents::Loading
                    && entry.handle.is_some()
                    && (preferred || !entry.preferred())
                    && (!matches!(reason, ReclaimReason::StreamingLimit) || !state.standby(entry))
            })
            .map(|(id, entry)| (*id, entry.preferred(), entry.last_use))
            .collect();
        candidates.sort_by_key(|(id, preferred, age)| {
            (*preferred, if *preferred { u64::MAX - id } else { *age })
        });
        // The arena lock excludes new pins; each candidate already has zero physical pins.
        // Idle cache goes completed-first, then after its own events, and joins the context
        // only for unfenced releases. The preferred suffix keeps strict order, since each of
        // its evictions permanently shortens the retained prefix.
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

    /// Evict idle, non-preferred regions whose last-consumer events have all completed, oldest
    /// first, until `bytes` are free. Never waits on an event or joins the context.
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
        let mut candidates: Vec<_> = state
            .entries
            .iter()
            .filter(|(id, entry)| {
                **id != exclude
                    && entry.uses.pins() == 0
                    && entry.uses.state() != Contents::Loading
                    && entry.handle.is_some()
                    && !entry.preferred()
                    && (!matches!(reason, ReclaimReason::StreamingLimit) || !state.standby(entry))
            })
            .map(|(id, entry)| (*id, entry.last_use))
            .collect();
        candidates.sort_by_key(|(_, age)| *age);
        for (id, _) in candidates {
            if report.reclaimed_bytes >= bytes {
                break;
            }
            if self.completion(state, id)? == Completion::Done {
                self.evict(state, id, reason, &mut report)?;
            }
        }
        Ok(report)
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
        let entry = state.entries.get_mut(&id).expect("locked candidate");
        // Every event is complete here; a destroy failure only leaks the event.
        for fence in entry.uses.fences().drain(..) {
            let _ = self.driver.destroy_event(fence.event);
        }
        let was_preferred = entry.preferred();
        entry.handle = None;
        entry.uses.invalidate();
        let retired = entry.uses.state() == Contents::Retired;
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
        if retired {
            if let Err(error) = self.driver.free_address(ptr, size) {
                return Err(poison(state, error));
            }
            state.entries.remove(&id);
        }
        Ok(())
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
    pub fn retire(self) -> RetiredRegion {
        if self.arena.pid == std::process::id() {
            self.uses.set(Contents::Retired);
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
    fn record_fences(&self, streams: &[u64]) -> Result<()> {
        if streams.is_empty()
            || self.uses.state() != Contents::Ready
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
        Ok(state.entries.get(&self.id).expect("live region").uses.pins() != 0)
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

    /// Admit a streamed block's read ahead of its use: it may evict idle streamed regions whose
    /// last-consumer events are already complete, oldest first, and never waits or joins. The
    /// retained prefix and every pending region stay; otherwise this refuses like
    /// `prepare_without_reclaim`.
    pub fn prepare_evicting_completed(self: &Arc<Self>) -> Result<Preparation> {
        self.prepare_with_reclaim(Admission::Completed)
    }

    fn prepare_with_reclaim(self: &Arc<Self>, admission: Admission) -> Result<Preparation> {
        let reclaim = admission == Admission::Any;
        let captures = capture_counts()?;
        let mut state = self.arena.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if entry.uses.state() == Contents::Loading {
            return Err(Error::Busy);
        }
        if entry.uses.state() == Contents::Ready {
            state.hits = state.hits.saturating_add(1);
            return Ok(Preparation::Resident(self.pin_locked(&mut state)?));
        }
        // A failed fill can leave escaped foreign storage. It owns these bytes until destruction.
        if entry.uses.pins() != 0 {
            return Err(Error::Busy);
        }
        if reclaim {
            ensure_recovery_allowed(&captures, self.arena.device)?;
        } else {
            ensure_not_capturing(&captures, self.arena.device)?;
        }
        let _context = self.arena.driver.enter()?;
        let has_handle = entry.handle.is_some();
        if !has_handle {
            if !entry.preferred() && state.streaming_limit != usize::MAX {
                let resident = state.streaming_bytes();
                let limit = state.streaming_limit.max(self.rounded);
                let excess = resident.saturating_add(self.rounded).saturating_sub(limit);
                match admission {
                    Admission::Any => {
                        self.arena.reclaim_locked(
                            &mut state,
                            excess,
                            false,
                            Some(self.id),
                            ReclaimReason::StreamingLimit,
                        )?;
                    }
                    Admission::Completed => {
                        self.arena.reclaim_completed_locked(
                            &mut state,
                            excess,
                            self.id,
                            ReclaimReason::StreamingLimit,
                        )?;
                    }
                    Admission::None if excess != 0 => {
                        return Err(Error::OutOfMemory {
                            requested_bytes: self.rounded,
                        });
                    }
                    Admission::None => {}
                }
                let remaining = state.streaming_bytes();
                if remaining.saturating_add(self.rounded) > limit {
                    return Err(Error::OutOfMemory {
                        requested_bytes: self.rounded,
                    });
                }
            }
            let handle = match self.arena.driver.create(self.rounded) {
                Ok(handle) => handle,
                Err(Error::OutOfMemory { .. }) => {
                    match admission {
                        Admission::None => {
                            return Err(Error::OutOfMemory {
                                requested_bytes: self.rounded,
                            });
                        }
                        Admission::Completed => {
                            self.arena.reclaim_completed_locked(
                                &mut state,
                                self.rounded,
                                self.id,
                                ReclaimReason::WeightAllocation,
                            )?;
                        }
                        Admission::Any => {
                            self.arena.reclaim_locked(
                                &mut state,
                                self.rounded,
                                false,
                                Some(self.id),
                                ReclaimReason::WeightAllocation,
                            )?;
                        }
                    }
                    match self.arena.driver.create(self.rounded) {
                        Ok(handle) => handle,
                        Err(error @ Error::OutOfMemory { .. }) => return Err(error),
                        Err(error) => return Err(poison(&mut state, error)),
                    }
                }
                Err(error) => return Err(poison(&mut state, error)),
            };
            // Record custody before map. Even an ambiguous map failure keeps both handle and VA.
            state.entries.get_mut(&self.id).expect("live region").handle = Some(handle);
            if let Err(error) = self.arena.driver.map(self.ptr, self.rounded, handle) {
                return Err(poison(&mut state, error));
            }
        } else {
            // An abandoned external fill may still have queued writes. Only a checked
            // native last-use release can remove this join; ordinary storage remains guarded.
            match (admission, self.arena.completion(&mut state, self.id)?) {
                (_, Completion::Done) if admission != Admission::None => {}
                (Admission::Any, Completion::Pending) => {
                    self.arena.wait_fences(&mut state, self.id)?
                }
                (Admission::Any, Completion::Unjoined) => {
                    self.arena.join_context(&mut state, &[self.id])?
                }
                // Even an abandoned fill with no remaining pins may have queued writers.
                _ => {
                    return Err(Error::OutOfMemory {
                        requested_bytes: self.rounded,
                    });
                }
            }
        }
        let pin = self.pin_locked(&mut state)?;
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
        self.region().pin_locked(&mut state)
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
            if region.record_fences(streams).is_err() {
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
        self.uses.set(Contents::Retired);
        let Ok(captures) = capture_counts() else {
            return;
        };
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
            }
            if self.driver.free_address(entry.ptr, entry.bytes).is_err() {
                return;
            }
        }
    }
}

/// Allocator-pressure entry point: native-only, one finite pass, no callbacks or external waits.
/// The caller retries its allocation once only if this returns enough useful reclaimed bytes.
pub fn reclaim(device: i32, bytes: usize) -> Result<ReclaimReport> {
    check_process()?;
    if is_recovery_blocked(device)? {
        return Err(Error::CaptureActive);
    }
    let mut report = ReclaimReport {
        requested_bytes: bytes,
        ..ReclaimReport::default()
    };
    let Some(registry) = ARENAS.get() else {
        return Ok(report);
    };
    let registry = registry
        .lock()
        .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
    for arena in registry
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|arena| arena.device == device)
    {
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
    let Some(registry) = ARENAS.get() else {
        return Ok(0);
    };
    let registry = registry
        .lock()
        .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
    registry
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|arena| arena.device == device)
        .try_fold(0usize, |bytes, arena| {
            bytes
                .checked_add(arena.stats()?.resident_bytes)
                .ok_or(Error::Invalid("resident byte count overflow"))
        })
}
