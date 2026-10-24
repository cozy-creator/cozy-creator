//! Process-local weight residency. TensorFS supplies bytes; this crate owns only CUDA memory.
//!
//! Every foreign storage must retain its [`Pin`] until the final alias is destroyed. Raw addresses
//! are not ownership. Kernels may finish after the final storage disappears, so reclamation joins
//! the primary CUDA context before unmapping. Capture/replay roots retain pins for their lifetime.
//! No imported memory, IPC, background work, file I/O, or Python callbacks occur here.
#![deny(unsafe_code)]

mod cuda;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
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
    pub poisoned: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReclaimReport {
    pub requested_bytes: usize,
    pub reclaimed_bytes: usize,
    pub evicted_regions: usize,
}

struct Entry {
    ptr: u64,
    bytes: usize,
    handle: Option<u64>,
    ready: bool,
    filling: bool,
    pins: usize,
    preferred: bool,
    retired: bool,
    last_use: u64,
}

struct State {
    entries: BTreeMap<u64, Entry>,
    next_id: u64,
    clock: u64,
    streaming_limit: usize,
    preferred_end: Option<u64>,
    poison: Option<String>,
    fills: u64,
    hits: u64,
    evictions: u64,
    reclaimed_bytes: u64,
}

/// A manager owned by one executor. It may serve multiple threads of that process.
pub struct Arena {
    driver: cuda::Driver,
    device: i32,
    pid: u32,
    state: Mutex<State>,
}

static ARENAS: OnceLock<Mutex<Vec<Weak<Arena>>>> = OnceLock::new();
static CAPTURES: OnceLock<Mutex<BTreeMap<i32, usize>>> = OnceLock::new();
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
fn capture_counts() -> Result<MutexGuard<'static, BTreeMap<i32, usize>>> {
    check_process()?;
    CAPTURES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("capture registry mutex".into()))
}

fn ensure_not_capturing(counts: &BTreeMap<i32, usize>, device: i32) -> Result<()> {
    if counts.get(&device).copied().unwrap_or(0) != 0 {
        return Err(Error::CaptureActive);
    }
    Ok(())
}

/// Guard every native arena on this device, including captures before any Arena exists.
pub fn enter_capture(device: i32) -> Result<GlobalCaptureGuard> {
    let mut counts = capture_counts()?;
    let count = counts.entry(device).or_default();
    *count = count
        .checked_add(1)
        .ok_or(Error::Invalid("capture count overflow"))?;
    Ok(GlobalCaptureGuard {
        device,
        pid: std::process::id(),
    })
}

pub fn is_capturing(device: i32) -> Result<bool> {
    Ok(capture_counts()?.get(&device).copied().unwrap_or(0) != 0)
}

pub struct GlobalCaptureGuard {
    device: i32,
    pid: u32,
}

impl Drop for GlobalCaptureGuard {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            return;
        }
        if let Ok(mut counts) = capture_counts()
            && let Some(count) = counts.get_mut(&self.device)
        {
            *count = count.saturating_sub(1);
        }
    }
}

impl Arena {
    pub fn new(device: i32) -> Result<Arc<Self>> {
        let captures = capture_counts()?;
        ensure_not_capturing(&captures, device)?;
        let driver = cuda::Driver::open(device)?;
        drop(captures);
        let arena = Arc::new(Self {
            driver,
            device,
            pid: std::process::id(),
            state: Mutex::new(State {
                entries: BTreeMap::new(),
                next_id: 1,
                clock: 0,
                streaming_limit: usize::MAX,
                preferred_end: None,
                poison: None,
                fills: 0,
                hits: 0,
                evictions: 0,
                reclaimed_bytes: 0,
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
        ensure_not_capturing(&captures, self.device)?;
        let mut state = self.lock()?;
        // A new model may choose a fresh prefix only after every old reservation owner is gone.
        // Escaped storage and graph Pins own their Region, so they prevent this reset.
        if state.entries.values().all(|entry| entry.retired) {
            state.preferred_end = None;
        }
        let id = state.next_id;
        state.next_id = id
            .checked_add(1)
            .ok_or(Error::Invalid("region id overflow"))?;
        let _context = self.driver.enter()?;
        let ptr = self.driver.reserve(rounded)?;
        state.entries.insert(
            id,
            Entry {
                ptr,
                bytes: rounded,
                handle: None,
                ready: false,
                filling: false,
                pins: 0,
                preferred: false,
                retired: false,
                last_use: 0,
            },
        );
        Ok(Arc::new(Region {
            arena: Arc::clone(self),
            id,
            ptr,
            bytes,
            rounded,
        }))
    }

    /// Bound the idle nonpreferred cache plus the next group, checked before allocation.
    /// Active/graph pins are working memory and excluded; real CUDA capacity still bounds them.
    /// A single larger group is permitted. Older idle groups go first; preferred groups survive.
    pub fn set_streaming_limit(&self, bytes: usize) -> Result<()> {
        self.lock()?.streaming_limit = bytes;
        Ok(())
    }

    /// Hold this guard over capture. Dependencies must already be prepared and separately pinned.
    pub fn enter_capture(self: &Arc<Self>) -> Result<CaptureGuard> {
        let global = enter_capture(self.device)?;
        drop(self.lock()?);
        Ok(CaptureGuard {
            _global: global,
            _arena: Arc::clone(self),
        })
    }

    /// Release at least the requested bytes when possible. Pins are never overridden.
    /// Idle nonpreferred pages go first; preferred pages go from the end of the reserved prefix.
    pub fn reclaim(&self, bytes: usize) -> Result<ReclaimReport> {
        let captures = capture_counts()?;
        ensure_not_capturing(&captures, self.device)?;
        let mut state = self.lock()?;
        let _context = self.driver.enter()?;
        self.reclaim_locked(&mut state, bytes, true, None)
    }

    fn reclaim_locked(
        &self,
        state: &mut State,
        bytes: usize,
        preferred: bool,
        exclude: Option<u64>,
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
                    && entry.pins == 0
                    && !entry.filling
                    && entry.handle.is_some()
                    && (preferred || !entry.preferred)
            })
            .map(|(id, entry)| (*id, entry.preferred, entry.last_use))
            .collect();
        candidates.sort_by_key(|(id, preferred, age)| {
            (*preferred, if *preferred { u64::MAX - id } else { *age })
        });
        if candidates.is_empty() {
            return Ok(report);
        }
        // Storage destruction does not prove all side streams completed. Join before any unmap.
        if let Err(error) = self.driver.synchronize() {
            return Err(poison(state, error));
        }
        for (id, _, _) in candidates {
            if report.reclaimed_bytes >= bytes {
                break;
            }
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
            entry.handle = None;
            entry.ready = false;
            let retired = entry.retired;
            let was_preferred = entry.preferred;
            if was_preferred {
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
            if retired {
                if let Err(error) = self.driver.free_address(ptr, size) {
                    return Err(poison(state, error));
                }
                state.entries.remove(&id);
            }
        }
        Ok(report)
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
            poisoned: state.poison.is_some(),
            ..Stats::default()
        };
        for entry in state.entries.values() {
            stats.reserved_bytes += entry.bytes;
            if entry.handle.is_some() {
                stats.resident_bytes += entry.bytes;
                if entry.ready {
                    stats.ready_bytes += entry.bytes;
                }
                if entry.pins != 0 {
                    stats.pinned_bytes += entry.bytes;
                }
                if entry.preferred {
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
}

impl Region {
    /// Refuse further use when a native integration cannot prove completion of its writers.
    /// The integration must also retain its actual Fill/source owners; this is not cleanup.
    pub fn poison(&self, detail: impl Into<String>) {
        if self.arena.pid != std::process::id() {
            return;
        }
        let mut state = self.arena.state.lock().unwrap_or_else(|e| e.into_inner());
        state.poison.get_or_insert_with(|| detail.into());
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
            .preferred)
    }

    pub fn is_resident(&self) -> Result<bool> {
        Ok(self
            .arena
            .lock()?
            .entries
            .get(&self.id)
            .expect("live region")
            .ready)
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
        let captures = capture_counts()?;
        let mut state = self.arena.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if entry.filling {
            return Err(Error::Busy);
        }
        if entry.ready {
            state.hits = state.hits.saturating_add(1);
            return Ok(Preparation::Resident(self.pin_locked(&mut state)?));
        }
        // A failed fill can leave escaped foreign storage. It owns these bytes until destruction.
        if entry.pins != 0 {
            return Err(Error::Busy);
        }
        ensure_not_capturing(&captures, self.arena.device)?;
        let _context = self.arena.driver.enter()?;
        let has_handle = entry.handle.is_some();
        if !has_handle {
            if !entry.preferred && state.streaming_limit != usize::MAX {
                let resident: usize = state
                    .entries
                    .values()
                    .filter(|entry| {
                        !entry.preferred
                            && entry.handle.is_some()
                            && entry.pins == 0
                            && !entry.filling
                    })
                    .map(|entry| entry.bytes)
                    .sum();
                let limit = state.streaming_limit.max(self.rounded);
                let excess = resident.saturating_add(self.rounded).saturating_sub(limit);
                self.arena
                    .reclaim_locked(&mut state, excess, false, Some(self.id))?;
                let remaining: usize = state
                    .entries
                    .values()
                    .filter(|entry| {
                        !entry.preferred
                            && entry.handle.is_some()
                            && entry.pins == 0
                            && !entry.filling
                    })
                    .map(|entry| entry.bytes)
                    .sum();
                if remaining.saturating_add(self.rounded) > limit {
                    return Err(Error::OutOfMemory {
                        requested_bytes: self.rounded,
                    });
                }
            }
            let handle = match self.arena.driver.create(self.rounded) {
                Ok(handle) => handle,
                Err(Error::OutOfMemory { .. }) => {
                    self.arena
                        .reclaim_locked(&mut state, self.rounded, false, Some(self.id))?;
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
            // An abandoned external fill may still have queued writes. Join before overwriting.
            if let Err(error) = self.arena.driver.synchronize() {
                return Err(poison(&mut state, error));
            }
        }
        let pin = self.pin_locked(&mut state)?;
        state
            .entries
            .get_mut(&self.id)
            .expect("live region")
            .filling = true;
        Ok(Preparation::Fill(Fill {
            pin: Some(pin),
            copied: false,
        }))
    }

    /// Borrow ready contents without creating a fill or doing device work.
    pub fn pin(self: &Arc<Self>) -> Result<Pin> {
        let mut state = self.arena.lock()?;
        let entry = state.entries.get(&self.id).expect("live region");
        if !entry.ready || entry.filling {
            return Err(Error::Invalid("region is not ready"));
        }
        self.pin_locked(&mut state)
    }

    fn pin_locked(self: &Arc<Self>, state: &mut State) -> Result<Pin> {
        state.clock = state.clock.saturating_add(1);
        let entry = state.entries.get_mut(&self.id).expect("live region");
        entry.pins = entry
            .pins
            .checked_add(1)
            .ok_or(Error::Invalid("pin count overflow"))?;
        entry.last_use = state.clock;
        Ok(Pin {
            region: Arc::clone(self),
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
        &self.pin.as_ref().expect("uncommitted fill").region
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
        ensure_not_capturing(&captures, region.arena.device)?;
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
        ensure_not_capturing(&captures, region.arena.device)?;
        let mut state = region.arena.lock()?;
        let _context = region.arena.driver.enter()?;
        if join && let Err(error) = region.arena.driver.synchronize() {
            return Err(poison(&mut state, error));
        }
        let entry = state.entries.get_mut(&region.id).expect("fill owns region");
        entry.filling = false;
        entry.ready = true;
        state.fills = state.fills.saturating_add(1);
        drop(state);
        drop(_context);
        Ok(self.pin.take().expect("committed fill"))
    }
}

impl Drop for Fill {
    fn drop(&mut self) {
        let Some(pin) = &self.pin else {
            return;
        };
        if pin.region.arena.pid != std::process::id() {
            return;
        }
        // Even poison permits bookkeeping; never touch CUDA or publish partial bytes on drop.
        if let Ok(mut state) = pin.region.arena.state.lock()
            && let Some(entry) = state.entries.get_mut(&pin.region.id)
        {
            entry.filling = false;
            entry.ready = false;
        }
    }
}

/// Physical pin + reservation ownership. The deleter of foreign storage must own this value.
pub struct Pin {
    region: Arc<Region>,
}

impl Pin {
    pub fn ptr(&self) -> u64 {
        self.region.ptr
    }
    pub fn len(&self) -> usize {
        self.region.bytes
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn device(&self) -> i32 {
        self.region.device()
    }
    pub fn region(&self) -> &Arc<Region> {
        &self.region
    }

    pub fn try_clone(&self) -> Result<Self> {
        let mut state = self.region.arena.lock()?;
        self.region.pin_locked(&mut state)
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        if self.region.arena.pid != std::process::id() {
            return;
        }
        if let Ok(mut state) = self.region.arena.state.lock()
            && let Some(entry) = state.entries.get_mut(&self.region.id)
        {
            entry.pins = entry.pins.saturating_sub(1);
        }
    }
}

pub struct CaptureGuard {
    _global: GlobalCaptureGuard,
    _arena: Arc<Arena>,
}

impl Drop for Region {
    fn drop(&mut self) {
        let Ok(captures) = capture_counts() else {
            return;
        };
        let Ok(mut state) = self.arena.lock() else {
            return;
        };
        let entry = state.entries.get_mut(&self.id).expect("last region owner");
        entry.retired = true;
        entry.preferred = false;
        // Defer physical teardown to reclamation or Arena drop; destructors cannot block capture.
        if entry.handle.is_some() || ensure_not_capturing(&captures, self.arena.device).is_err() {
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
        if ensure_not_capturing(&captures, self.device).is_err() {
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
    if is_capturing(device)? {
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
        let next = arena.reclaim(bytes - report.reclaimed_bytes)?;
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
