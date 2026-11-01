//! Process-local working allocations and prepared graph pools, sharing the weight pager's
//! capture exclusion. Streams are native CUstream handles in the device's primary context.
//! A graph owner must call `pool_capture_complete` after CUDA capture actually ends and
//! `pool_record_stream` after every replay/consumer submission before dropping its reference.

#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub(crate) mod peer;

use crate::{Error, GlobalCaptureGuard, Result, cuda};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

mod split;
mod cache;
pub use cache::{Cache, CacheStats};
pub use split::{
    SplitContext, SplitObservation, SplitSelection, split_begin, split_context, split_end,
    split_select,
};

pub type PoolId = (u64, u64);

/// Allocation ranges retained by one actual graph definition. This owns metadata only.
pub struct GraphFootprint {
    id: u64,
    device: i32,
    pool: PoolId,
    pid: u32,
    begun: AtomicBool,
    retired: AtomicBool,
    sealed: Mutex<Option<SealedFootprint>>,
}

#[derive(Clone, Default)]
struct Ranges(BTreeMap<u64, u64>);
impl Ranges {
    fn add(&mut self, mut start: u64, mut end: u64) {
        if let Some((&left, &right)) = self.0.range(..=start).next_back()
            && right >= start
        {
            start = left;
            end = end.max(right);
            self.0.remove(&left);
        }
        while let Some((&left, &right)) = self.0.range(start..).next() {
            if left > end {
                break;
            }
            end = end.max(right);
            self.0.remove(&left);
        }
        self.0.insert(start, end);
    }
    fn extend(&mut self, other: &Self) {
        for (&start, &end) in &other.0 {
            self.add(start, end);
        }
    }
    fn bytes(&self) -> usize {
        self.0
            .iter()
            .map(|(&start, &end)| (end - start) as usize)
            .sum()
    }
    fn pages(&self, page: usize) -> Self {
        let page = page as u64;
        let mut result = Self::default();
        for (&start, &end) in &self.0 {
            result.add(start / page * page, end.div_ceil(page) * page);
        }
        result
    }
}

struct SealedFootprint {
    pool_identity: Weak<()>,
    ranges: Ranges,
    pages: Ranges,
    allocations: u64,
}
struct CaptureTrace {
    footprint: Arc<GraphFootprint>,
    pool_identity: Weak<()>,
    ranges: Ranges,
    allocations: u64,
    invalid: bool,
}

#[derive(Clone, Debug)]
pub struct FootprintStats {
    pub definition: u64,
    pub device: i32,
    pub pool: PoolId,
    pub sealed: bool,
    pub retired: bool,
    pub traced_bytes: usize,
    pub backing_bytes: usize,
    pub allocations: u64,
}

static NEXT_FOOTPRINT: AtomicU64 = AtomicU64::new(0);

impl GraphFootprint {
    pub fn new(device: i32, pool: PoolId) -> Result<Arc<Self>> {
        crate::check_process()?;
        let id = NEXT_FOOTPRINT
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Invalid("graph definition identity overflow"))?
            + 1;
        Ok(Arc::new(Self {
            id,
            device,
            pool,
            pid: std::process::id(),
            begun: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            sealed: Mutex::new(None),
        }))
    }
    fn check(&self) -> Result<()> {
        crate::check_process()?;
        if self.pid != std::process::id() {
            return Err(Error::Invalid("graph footprint used after fork"));
        }
        Ok(())
    }
    pub fn device(&self) -> i32 {
        self.device
    }
    pub fn begin(self: &Arc<Self>, device: i32, pool: PoolId) -> Result<()> {
        self.check()?;
        if device != self.device || pool != self.pool {
            return Err(Error::Invalid("graph footprint device or pool changed"));
        }
        let owner = owner(device)?;
        let captures = crate::capture_counts()?;
        crate::ensure_not_capturing(&captures, device)?;
        let mut state = owner.lock()?;
        if state.trace.is_some() {
            return Err(Error::Invalid("graph footprint recordings cannot overlap"));
        }
        let identity = state
            .pools
            .get(&pool)
            .ok_or(Error::Invalid("prepare the graph pool before tracing"))?
            .identity
            .clone();
        if self.retired.load(Ordering::Acquire) || self.begun.swap(true, Ordering::AcqRel) {
            return Err(Error::Invalid("graph footprint is one capture definition"));
        }
        state.trace = Some(CaptureTrace {
            footprint: self.clone(),
            pool_identity: Arc::downgrade(&identity),
            ranges: Ranges::default(),
            allocations: 0,
            invalid: false,
        });
        Ok(())
    }
    pub fn finish(self: &Arc<Self>, completed: bool) -> Result<()> {
        self.check()?;
        let owner = owner(self.device)?;
        let captures = crate::capture_counts()?;
        if completed {
            crate::ensure_not_capturing(&captures, self.device)?;
        }
        let mut state = owner
            .state
            .lock()
            .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
        if !state
            .trace
            .as_ref()
            .is_some_and(|trace| Arc::ptr_eq(&trace.footprint, self))
        {
            return Err(Error::Invalid("graph footprint is not recording"));
        }
        let trace = state.trace.take().expect("checked footprint");
        if completed
            && !trace.invalid
            && state.poison.is_none()
            && !self.retired.load(Ordering::Acquire)
        {
            let pages = trace.ranges.pages(owner.driver.granularity);
            *self
                .sealed
                .lock()
                .map_err(|_| Error::Poisoned("graph footprint mutex".into()))? =
                Some(SealedFootprint {
                    pool_identity: trace.pool_identity,
                    ranges: trace.ranges,
                    pages,
                    allocations: trace.allocations,
                });
        } else {
            self.retired.store(true, Ordering::Release);
            state
                .phases
                .replay_samples
                .retain(|key, _| !key.definitions.contains(&self.id));
        }
        Ok(())
    }
    /// Called by the owning graph's reset/destroy hook. Metadata only, never a CUDA call.
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        if let Ok(Some(owner)) = existing(self.device)
            && let Ok(mut state) = owner.state.lock()
        {
            state
                .phases
                .replay_samples
                .retain(|key, _| !key.definitions.contains(&self.id));
        }
    }
    pub fn facts(&self) -> Result<FootprintStats> {
        self.check()?;
        let data = self
            .sealed
            .lock()
            .map_err(|_| Error::Poisoned("graph footprint mutex".into()))?;
        Ok(FootprintStats {
            definition: self.id,
            device: self.device,
            pool: self.pool,
            sealed: data.is_some(),
            retired: self.retired.load(Ordering::Acquire),
            traced_bytes: data.as_ref().map_or(0, |data| data.ranges.bytes()),
            backing_bytes: data.as_ref().map_or(0, |data| data.pages.bytes()),
            allocations: data.as_ref().map_or(0, |data| data.allocations),
        })
    }
    fn ranges(&self, state: &State) -> Result<(Ranges, Ranges)> {
        self.check()?;
        if self.retired.load(Ordering::Acquire) {
            return Err(Error::Invalid("graph definition is retired"));
        }
        let data = self
            .sealed
            .lock()
            .map_err(|_| Error::Poisoned("graph footprint mutex".into()))?;
        let data = data
            .as_ref()
            .ok_or(Error::Invalid("graph footprint is not sealed"))?;
        let pool = state
            .pools
            .get(&self.pool)
            .ok_or(Error::Invalid("graph footprint pool retired"))?;
        if !Weak::ptr_eq(&data.pool_identity, &Arc::downgrade(&pool.identity))
            || pool.references == 0
        {
            return Err(Error::Invalid("graph footprint pool incarnation changed"));
        }
        Ok((data.ranges.clone(), data.pages.clone()))
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReplayKey {
    semantic: String,
    definitions: Vec<u64>,
}

/// An ordered source edge carries identity only; capacity is checked at actual lookahead.
#[derive(Clone)]
pub struct ReplayPrefetch {
    device: i32,
    semantic: Option<String>,
    footprints: Vec<Arc<GraphFootprint>>,
}
impl ReplayPrefetch {
    pub fn new(
        device: i32,
        semantic: Option<String>,
        footprints: Vec<Arc<GraphFootprint>>,
    ) -> Result<Self> {
        crate::check_process()?;
        if semantic.as_ref().is_some_and(String::is_empty) {
            return Err(Error::Invalid("replay prefetch key must not be empty"));
        }
        if footprints
            .iter()
            .any(|footprint| footprint.device != device)
        {
            return Err(Error::Invalid("replay prefetch definitions cross devices"));
        }
        Ok(Self {
            device,
            semantic,
            footprints,
        })
    }
}

struct ReplayTrace {
    footprints: Vec<Arc<GraphFootprint>>,
    bytes: usize,
    backing: usize,
}

/// Execution classification is explicit: recording/preparation/replay are not eager samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseMode {
    Eager,
    Capture,
    Replay,
    Preparation,
}

impl PhaseMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Eager => "eager",
            Self::Capture => "capture",
            Self::Replay => "replay",
            Self::Preparation => "preparation",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PhaseStats {
    pub known: bool,
    /// Newly allocated charged capacity attributable to the last successful phase.
    pub peak_bytes: usize,
    pub total_peak_bytes: usize,
    /// Distinct backing segments containing phase-owned live/pending blocks: admission watermark.
    pub backing_peak_bytes: usize,
    pub successful_passes: u64,
    pub aborted_passes: u64,
    pub aborted_peak_bytes: usize,
    pub aborted_total_peak_bytes: usize,
    pub aborted_backing_peak_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct PhaseObservation {
    pub split_observations: Vec<SplitObservation>,
    pub definitions: Vec<u64>,
    pub token: u64,
    pub mode: PhaseMode,
    pub key: Option<String>,
    pub peak_bytes: usize,
    pub total_peak_bytes: usize,
    pub backing_peak_bytes: usize,
    pub published: bool,
    pub invalidated: bool,
    pub reason: String,
    pub successful_peak_bytes: usize,
    pub successful_backing_peak_bytes: usize,
    pub successful_passes: u64,
}

struct ActivePhase {
    token: u64,
    key: Option<String>,
    mode: PhaseMode,
    activity_epoch: u64,
    charged: usize,
    peak: usize,
    owned_charged: usize,
    owned_peak: usize,
    backing: usize,
    backing_peak: usize,
    invalid: Option<String>,
    replay: Option<ReplayTrace>,
}

#[derive(Default)]
struct Phases {
    next: u64,
    active: Option<ActivePhase>,
    samples: BTreeMap<String, PhaseStats>,
    replay_samples: BTreeMap<ReplayKey, PhaseStats>,
}

impl Phases {
    fn invalidate(&mut self, reason: &str) {
        if let Some(active) = &mut self.active {
            active.invalid.get_or_insert_with(|| reason.to_owned());
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WorkingStats {
    pub allocated_bytes: usize,
    pub allocated_peak: usize,
    pub allocated_capacity: usize,
    pub mapped_bytes: usize,
    pub mapped_peak: usize,
    pub pending_bytes: usize,
    pub free_bytes: usize,
    /// Completed, fully free eager segments only; graph pools and pending work are excluded.
    pub reclaimable_bytes: usize,
    pub unassigned_bytes: usize,
    pub pools_bytes: usize,
    pub pools: usize,
    pub event_handles: usize,
    pub allocations: u64,
    pub frees: u64,
    pub ooms: u64,
    pub driver_memory_calls: u64,
    pub driver_physical_ooms: u64,
    pub event_queries: u64,
    pub event_records: u64,
    pub event_waits: u64,
    pub context_joins: u64,
    pub prefetch_attempts: u64,
    pub prefetch_admissions: u64,
    pub prefetch_declines: u64,
    /// Owner-controlled cache storage is separate from Torch/phase working demand.
    pub cache_reserved_bytes: usize,
    pub cache_mapped_bytes: usize,
    pub cache_live_bytes: usize,
    pub cache_pending_bytes: usize,
    pub cache_ready_bytes: usize,
    pub caches: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Life {
    Live,
    Ready,
    Pending,
    Deferred,
    Captured,
    // An unannotated native/DLPack final owner: join the context before reuse.
    Unjoined,
}

struct Block {
    segment: u64,
    capacity: usize,
    requested: usize,
    stream: u64,
    origin_retired: bool,
    // Allocation generation, not address identity: reuse retags an old/pending block.
    phase: Option<u64>,
    streams: BTreeSet<u64>,
    events: BTreeMap<u64, usize>,
    life: Life,
    // None means all previous work is complete. Some(stream) permits ordered reuse only there.
    reusable_on: Option<u64>,
}

impl Block {
    fn charge(&self) -> usize {
        if self.life != Life::Ready || self.reusable_on.is_some() {
            self.capacity
        } else {
            0
        }
    }

    #[inline]
    fn reusable_from(&self, origin: Option<u64>) -> bool {
        (self.life == Life::Ready
            || (self.life == Life::Captured && origin.is_some())
            || (self.deferred_default() && origin == Some(0)))
            && self.reusable_on == origin
    }

    fn deferred_default(&self) -> bool {
        self.life == Life::Deferred
            && self.reusable_on == Some(0)
            && self.streams.len() == 1
            && self.streams.contains(&0)
    }
}

struct Segment {
    bytes: usize,
    handle: u64,
    pool: Option<PoolId>,
    // Rebuilt at phase entry; maintained only while that phase owns the device observation.
    charged_blocks: usize,
    phase_blocks: usize,
}

#[derive(Default)]
struct Pool {
    identity: Arc<()>,
    prepared: bool,
    references: usize,
    routing: bool,
    capture: Option<GlobalCaptureGuard>,
    streams: BTreeSet<u64>,
    events: BTreeMap<u64, usize>,
    // Free-block resizing transfers its event owners here; CUDA destruction waits for poll.
    retired_events: Vec<usize>,
}

#[derive(Default)]
struct State {
    segments: BTreeMap<u64, Segment>,
    blocks: BTreeMap<u64, Block>,
    // (pool, size class, originating stream). Cross-stream candidates are promoted by poll.
    free: BTreeMap<(Option<PoolId>, usize, Option<u64>), Vec<u64>>,
    addresses: BTreeMap<u64, usize>,
    pools: BTreeMap<PoolId, Pool>,
    stats: WorkingStats,
    phases: Phases,
    trace: Option<CaptureTrace>,
    poison: Option<String>,
    splits: split::Splits,
    caches: BTreeMap<u64, cache::Storage>,
    next_cache: u64,
}

impl State {
    fn charged(&self) -> usize {
        self.blocks.values().map(Block::charge).sum()
    }

    fn reusable_eager_pages(&self) -> usize {
        self.segments
            .iter()
            .filter(|(base, segment)| {
                segment.pool.is_none()
                    && self
                        .blocks
                        .range(**base..**base + segment.bytes as u64)
                        .all(|(_, block)| block.life == Life::Ready && block.reusable_on.is_none())
            })
            .map(|(_, segment)| segment.bytes)
            .sum()
    }

    fn phase_changed(
        &mut self,
        before: usize,
        after: usize,
        prior: Option<u64>,
        next: Option<u64>,
        segment: u64,
    ) {
        let Some(active) = &mut self.phases.active else {
            return;
        };
        active.charged = active.charged - before + after;
        active.peak = active.peak.max(active.charged);
        let was_owned = prior == Some(active.token) && before != 0;
        let is_owned = next == Some(active.token) && after != 0;
        if was_owned {
            active.owned_charged -= before;
        }
        if is_owned {
            active.owned_charged += after;
        }
        active.owned_peak = active.owned_peak.max(active.owned_charged);
        let segment = self
            .segments
            .get_mut(&segment)
            .expect("live allocation segment");
        let held = segment.phase_blocks != 0;
        segment.charged_blocks =
            segment.charged_blocks - usize::from(before != 0) + usize::from(after != 0);
        segment.phase_blocks =
            segment.phase_blocks - usize::from(was_owned) + usize::from(is_owned);
        match (held, segment.phase_blocks != 0) {
            (false, true) => active.backing += segment.bytes,
            (true, false) => active.backing -= segment.bytes,
            _ => {}
        }
        active.backing_peak = active.backing_peak.max(active.backing);
    }

    // Completion and pool resizing already traverse metadata. Rebuild their phase counts once.
    fn refresh_phase(&mut self) {
        if let Some(token) = self.phases.active.as_ref().map(|active| active.token) {
            for segment in self.segments.values_mut() {
                segment.charged_blocks = 0;
                segment.phase_blocks = 0;
            }
            let (mut charged, mut owned) = (0usize, 0usize);
            for block in self.blocks.values().filter(|block| block.charge() != 0) {
                charged += block.capacity;
                let segment = self
                    .segments
                    .get_mut(&block.segment)
                    .expect("live allocation segment");
                segment.charged_blocks += 1;
                if block.phase == Some(token) {
                    owned += block.capacity;
                    segment.phase_blocks += 1;
                }
            }
            let backing = self
                .segments
                .values()
                .filter(|segment| segment.phase_blocks != 0)
                .map(|segment| segment.bytes)
                .sum();
            let active = self.phases.active.as_mut().expect("active phase");
            active.charged = charged;
            active.peak = active.peak.max(charged);
            active.owned_charged = owned;
            active.owned_peak = active.owned_peak.max(owned);
            active.backing = backing;
            active.backing_peak = active.backing_peak.max(backing);
            split::refresh(self);
        }
    }

    fn exclusive_phase_backing(&self) -> usize {
        self.segments
            .values()
            .filter(|segment| {
                segment.pool.is_none()
                    && segment.phase_blocks != 0
                    && segment.charged_blocks == segment.phase_blocks
            })
            .map(|segment| segment.bytes)
            .sum()
    }
}

struct Working {
    driver: cuda::Driver,
    page: usize,
    room: Arc<crate::Room>,
    external_callbacks: Arc<std::sync::atomic::AtomicUsize>,
    state: Mutex<State>,
}

static DEVICES: OnceLock<Mutex<BTreeMap<i32, Arc<Working>>>> = OnceLock::new();

/// Bind integration registration before CUDA under the one_process/1 contract. Linux's
/// default kernel guard is armed here; strict/non-Linux/setup-failure paths check actual PID.
/// A private-VM fork child rejects rather than arming inherited state.
pub fn bind_process() -> Result<()> {
    crate::check_process()?;
    #[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
    crate::fork_guard::arm()?;
    Ok(())
}

fn devices() -> Result<MutexGuard<'static, BTreeMap<i32, Arc<Working>>>> {
    crate::check_process()?;
    lock_devices()
}

// Only after check_process, directly or through capture_counts, on this call's thread.
fn lock_devices() -> Result<MutexGuard<'static, BTreeMap<i32, Arc<Working>>>> {
    DEVICES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("working allocator registry".into()))
}

fn existing(device: i32) -> Result<Option<Arc<Working>>> {
    Ok(devices()?.get(&device).cloned())
}

fn owner(device: i32) -> Result<Arc<Working>> {
    existing(device)?.ok_or(Error::Invalid(
        "working allocator is not initialized on this device",
    ))
}

fn rounded(bytes: usize, align: usize) -> Result<usize> {
    bytes
        .checked_add(align - 1)
        .map(|n| n / align * align)
        .ok_or(Error::OutOfMemory {
            requested_bytes: bytes,
        })
}

fn size_class(bytes: usize, page: usize) -> Result<usize> {
    if bytes > 1 << 20 {
        return rounded(bytes, page);
    }
    let power = bytes
        .max(512)
        .checked_next_power_of_two()
        .ok_or(Error::OutOfMemory {
            requested_bytes: bytes,
        })?;
    let middle = rounded(power / 2 + power / 4, 512)?;
    Ok(if middle >= bytes && middle >= 512 {
        middle
    } else {
        power
    })
}

impl Working {
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
        if let Some(reason) = &state.poison {
            return Err(Error::Poisoned(reason.clone()));
        }
        Ok(state)
    }

    fn callback_mutation(&self, state: &mut State) {
        // The inactive measurement path does not even load the callback counter.
        if state.phases.active.is_some() && self.external_callbacks.load(Ordering::Acquire) != 0 {
            state.phases.invalidate("external_callback_memory");
            split::invalidate(state, "external_callback_memory");
        }
    }

    fn fail(&self, state: &mut State, error: Error) -> Error {
        if !matches!(
            error,
            Error::OutOfMemory { .. }
                | Error::Exhausted { .. }
                | Error::Invalid(_)
                | Error::CaptureActive
                | Error::Busy
        ) {
            state.poison = Some(error.to_string());
        }
        error
    }

    fn record(&self, events: &mut BTreeMap<u64, usize>, stream: u64) -> Result<()> {
        let event = match events.get(&stream) {
            Some(event) => *event,
            None => {
                let event = self.driver.create_event()?;
                events.insert(stream, event);
                event
            }
        };
        self.driver.record(event, stream as usize)
    }

    fn record_free(&self, block: &mut Block) -> Result<()> {
        for stream in &block.streams {
            self.record(&mut block.events, *stream)?;
        }
        block.life = if block.streams.len() <= 1 {
            Life::Ready
        } else {
            Life::Pending
        };
        block.reusable_on = block.streams.first().copied();
        Ok(())
    }

    fn completed(&self, events: &BTreeMap<u64, usize>) -> Result<bool> {
        for event in events.values() {
            if !self.driver.query(*event)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn pool_completed(&self, pool: &Pool) -> Result<bool> {
        if !self.completed(&pool.events)? {
            return Ok(false);
        }
        for event in &pool.retired_events {
            if !self.driver.query(*event)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn poll(&self, state: &mut State) -> Result<()> {
        self.poll_mode(state, true)
    }

    // A peer response may query submitted fences, but never join unknown consumers.
    fn poll_mode(&self, state: &mut State, join_unknown: bool) -> Result<()> {
        cache::poll(self, state)?;
        // The caller holds capture exclusion. Unknown foreign consumers require a context
        // completion proof, but their last-owner retirement itself never calls CUDA.
        if join_unknown && state
            .blocks
            .values()
            .any(|block| block.life == Life::Unjoined)
        {
            state.stats.context_joins += 1;
            self.driver.synchronize()?;
            let mut ready = Vec::new();
            for (ptr, block) in &mut state.blocks {
                if block.life == Life::Unjoined {
                    block.life = Life::Ready;
                    block.reusable_on = None;
                    block.streams.clear();
                    ready.push((*ptr, block.capacity));
                }
            }
            for (ptr, bytes) in ready {
                state.free.entry((None, bytes, None)).or_default().push(ptr);
            }
        }
        for pool in state.pools.values_mut() {
            let mut index = 0;
            while index < pool.retired_events.len() {
                let event = pool.retired_events[index];
                if self.driver.query(event)? {
                    self.driver.destroy_event(event)?;
                    pool.retired_events.swap_remove(index);
                } else {
                    index += 1;
                }
            }
        }
        let mut ready = Vec::new();
        for (ptr, block) in &mut state.blocks {
            if block.life == Life::Deferred {
                self.record_free(block)?;
                if block.life == Life::Ready {
                    ready.push((*ptr, block.capacity, block.reusable_on, block.segment));
                }
            }
            if matches!(block.life, Life::Ready | Life::Pending)
                && block.reusable_on.is_some()
                && self.completed(&block.events)?
            {
                block.life = Life::Ready;
                block.reusable_on = None;
                ready.push((*ptr, block.capacity, None, block.segment));
            }
        }
        for (ptr, bytes, stream, segment) in ready {
            state
                .free
                .entry((state.segments[&segment].pool, bytes, stream))
                .or_default()
                .push(ptr);
        }
        state.refresh_phase();
        Ok(())
    }

    fn take(
        &self,
        state: &mut State,
        bytes: usize,
        requested: usize,
        stream: u64,
        pool: Option<PoolId>,
    ) -> Option<u64> {
        let phase = state.phases.active.as_ref().map(|active| active.token);
        for origin in [Some(stream), None] {
            let slots = state.free.get_mut(&(pool, bytes, origin));
            if let Some(slots) = slots {
                while let Some(ptr) = slots.pop() {
                    let Some(block) = state.blocks.get_mut(&ptr) else {
                        continue;
                    };
                    if block.capacity != bytes || !block.reusable_from(origin) {
                        continue;
                    }
                    // Phase tokens never repeat, and entry rebuilds segment accounting.
                    // Outside an observation an old tag needs neither reading nor clearing.
                    let observed = phase.map(|token| {
                        let before = block.charge();
                        let prior = block.phase;
                        block.phase = Some(token);
                        (before, prior)
                    });
                    block.life = Life::Live;
                    block.stream = stream;
                    block.origin_retired = false;
                    block.requested = requested;
                    if block.streams.len() != 1 || !block.streams.contains(&stream) {
                        block.streams.clear();
                        block.streams.insert(stream);
                    }
                    if let Some((before, prior)) = observed {
                        let (capacity, segment) = (block.capacity, block.segment);
                        state.phase_changed(before, capacity, prior, phase, segment);
                        split::allocated(state, ptr);
                    }
                    return Some(ptr);
                }
            }
        }
        None
    }

    fn activate(state: &mut State, ptr: u64, requested: usize) -> u64 {
        if let Some(trace) = &mut state.trace {
            let block = &state.blocks[&ptr];
            if state.segments[&block.segment].pool == Some(trace.footprint.pool) {
                trace.ranges.add(ptr, ptr + block.capacity as u64);
                trace.allocations += 1;
            } else {
                trace.invalid = true;
            }
        }
        state.stats.allocated_bytes += requested;
        state.stats.allocated_peak = state.stats.allocated_peak.max(state.stats.allocated_bytes);
        state.stats.allocations += 1;
        ptr
    }

    fn address(state: &mut State, bytes: usize) -> Result<u64> {
        let (ptr, len) = state
            .addresses
            .iter()
            .filter(|(_, n)| **n >= bytes)
            .min_by_key(|(_, n)| **n)
            .map(|(p, n)| (*p, *n))
            .ok_or(Error::OutOfMemory {
                requested_bytes: bytes,
            })?;
        state.addresses.remove(&ptr);
        if len > bytes {
            state.addresses.insert(ptr + bytes as u64, len - bytes);
        }
        Ok(ptr)
    }

    fn return_address(state: &mut State, mut ptr: u64, mut bytes: usize) {
        if let Some((left, len)) = state
            .addresses
            .range(..ptr)
            .next_back()
            .map(|(p, n)| (*p, *n))
            && left + len as u64 == ptr
        {
            state.addresses.remove(&left);
            ptr = left;
            bytes += len;
        }
        if let Some(len) = state.addresses.remove(&(ptr + bytes as u64)) {
            bytes += len;
        }
        state.addresses.insert(ptr, bytes);
    }

    fn map(
        &self,
        state: &mut State,
        bytes: usize,
        pool: Option<PoolId>,
        class: usize,
    ) -> crate::AllocationResult<u64> {
        if self.room.over(bytes) != 0 {
            return Err(Error::OutOfMemory {
                requested_bytes: bytes,
            }.into());
        }
        let ptr = Self::address(state, bytes)?;
        let handle = match self.driver.create(bytes) {
            Ok(handle) => handle,
            Err(error) => {
                Self::return_address(state, ptr, bytes);
                return Err(error);
            }
        };
        // Track ownership before map: a partial mapping failure must retain the VA and handle.
        state.segments.insert(
            ptr,
            Segment {
                bytes,
                handle,
                pool,
                charged_blocks: 0,
                phase_blocks: 0,
            },
        );
        self.room
            .working
            .fetch_add(bytes, std::sync::atomic::Ordering::AcqRel);
        state.stats.mapped_bytes += bytes;
        state.stats.mapped_peak = state.stats.mapped_peak.max(state.stats.mapped_bytes);
        self.driver.map(ptr, bytes, handle)?;
        if let Some(pool) = pool {
            state.blocks.insert(
                ptr,
                Block {
                    segment: ptr,
                    capacity: bytes,
                    requested: 0,
                    stream: 0,
                    origin_retired: false,
                    phase: None,
                    streams: BTreeSet::new(),
                    events: BTreeMap::new(),
                    life: Life::Ready,
                    reusable_on: None,
                },
            );
            state
                .free
                .entry((Some(pool), bytes, None))
                .or_default()
                .push(ptr);
        } else {
            for offset in (0..bytes).step_by(class) {
                if bytes - offset < class {
                    break;
                }
                let address = ptr + offset as u64;
                state.blocks.insert(
                    address,
                    Block {
                        segment: ptr,
                        capacity: class,
                        requested: 0,
                        stream: 0,
                        origin_retired: false,
                        phase: None,
                        streams: BTreeSet::new(),
                        events: BTreeMap::new(),
                        life: Life::Ready,
                        reusable_on: None,
                    },
                );
                state
                    .free
                    .entry((None, class, None))
                    .or_default()
                    .push(address);
            }
        }
        Ok(ptr)
    }

    fn fit_pool(
        &self,
        state: &mut State,
        pool: PoolId,
        bytes: usize,
        requested: usize,
        stream: u64,
    ) -> Option<u64> {
        // Only free ranges whose ordering permits this stream can be joined. Live blocks,
        // other-stream pending frees and segment boundaries always end a candidate run.
        let mut best: Option<(u64, usize, u64, bool)> = None;
        let mut consider = |ptr, capacity, segment, complete| {
            if capacity >= bytes && best.is_none_or(|(_, n, _, _)| capacity < n) {
                best = Some((ptr, capacity, segment, complete));
            }
        };
        for (base, segment) in &state.segments {
            if segment.pool != Some(pool) {
                continue;
            }
            let (mut start, mut length, mut complete) = (*base, 0usize, true);
            for (ptr, block) in state.blocks.range(*base..*base + segment.bytes as u64) {
                let ready = block.life == Life::Ready && block.reusable_on.is_none();
                let ordered = block.reusable_on == Some(stream)
                    && (matches!(block.life, Life::Ready | Life::Captured)
                        || block.deferred_default());
                if !ready && !ordered {
                    consider(start, length, *base, complete);
                    length = 0;
                    continue;
                }
                if length != 0 && *ptr != start + length as u64 {
                    consider(start, length, *base, complete);
                    length = 0;
                }
                if length == 0 {
                    start = *ptr;
                    complete = true;
                }
                length += block.capacity;
                complete &= ready;
            }
            consider(start, length, *base, complete);
        }
        let (ptr, capacity, segment, complete) = best?;
        let phase = state.phases.active.as_ref().map(|active| active.token);
        let end = ptr + capacity as u64;
        let parts: Vec<_> = state.blocks.range(ptr..end).map(|(p, _)| *p).collect();
        let mut events = Vec::new();
        for part in parts {
            let block = state
                .blocks
                .remove(&part)
                .expect("selected free pool block");
            events.extend(block.events.into_values());
        }
        state
            .pools
            .get_mut(&pool)
            .expect("owned pool")
            .retired_events
            .extend(events);
        state.free.retain(|(id, _, _), slots| {
            if *id == Some(pool) {
                slots.retain(|p| *p < ptr || *p >= end);
            }
            !slots.is_empty()
        });
        if capacity > bytes {
            let tail = ptr + bytes as u64;
            let order = if complete { None } else { Some(stream) };
            state.blocks.insert(
                tail,
                Block {
                    segment,
                    capacity: capacity - bytes,
                    requested: 0,
                    stream,
                    origin_retired: false,
                    phase: if complete { None } else { phase },
                    streams: if complete {
                        BTreeSet::new()
                    } else {
                        BTreeSet::from([stream])
                    },
                    events: BTreeMap::new(),
                    life: if complete {
                        Life::Ready
                    } else {
                        Life::Captured
                    },
                    reusable_on: order,
                },
            );
            state
                .free
                .entry((Some(pool), capacity - bytes, order))
                .or_default()
                .push(tail);
        }
        state.blocks.insert(
            ptr,
            Block {
                segment,
                capacity: bytes,
                requested,
                stream,
                origin_retired: false,
                phase,
                streams: BTreeSet::from([stream]),
                events: BTreeMap::new(),
                life: Life::Live,
                reusable_on: None,
            },
        );
        state.refresh_phase();
        Some(ptr)
    }

    fn trim(&self, state: &mut State, limit: usize, exclude: Option<PoolId>) -> Result<usize> {
        self.poll(state)?;
        self.trim_ready(state, limit, exclude, &BTreeSet::new())
    }

    // The split chooser has already observed completion and matched exact reusable slots
    // under this same lock. Preserve their backing while releasing unrelated idle pages.
    fn trim_ready(
        &self,
        state: &mut State,
        limit: usize,
        exclude: Option<PoolId>,
        preserve: &BTreeSet<u64>,
    ) -> Result<usize> {
        self.trim_ready_capped(state, limit, exclude, preserve, usize::MAX, &mut 0)
    }

    fn trim_ready_capped(
        &self, state: &mut State, limit: usize, exclude: Option<PoolId>,
        preserve: &BTreeSet<u64>, maximum: usize, completed: &mut usize,
    ) -> Result<usize> {
        let mut released = 0usize;
        let mut candidates = Vec::new();
        for (base, segment) in &state.segments {
            if preserve.contains(base) {
                continue;
            }
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            if let Some(id) = segment.pool {
                let pool = &state.pools[&id];
                if pool.prepared
                    || pool.references != 0
                    || pool.capture.is_some()
                    || !self.pool_completed(pool)?
                {
                    continue;
                }
            }
            if state
                .blocks
                .range(*base..(*base + segment.bytes as u64))
                .all(|(_, block)| block.life == Life::Ready && block.reusable_on.is_none())
            {
                candidates.push(*base);
            }
        }
        for base in candidates {
            if released >= limit {
                break;
            }
            let segment = &state.segments[&base];
            if segment.bytes > maximum.saturating_sub(released) { continue; }
            self.driver.unmap(base, segment.bytes)?;
            self.driver.release(segment.handle)?;
            *completed += segment.bytes;
            let bytes = segment.bytes;
            self.room
                .working
                .fetch_sub(bytes, std::sync::atomic::Ordering::AcqRel);
            self.room.released.fetch_add(bytes as u64, Ordering::AcqRel);
            let blocks: Vec<_> = state
                .blocks
                .range(base..base + bytes as u64)
                .map(|(p, _)| *p)
                .collect();
            for ptr in blocks {
                let block = state.blocks.remove(&ptr).expect("retired block");
                for event in block.events.values() {
                    self.driver.destroy_event(*event)?;
                }
            }
            state.segments.remove(&base);
            state.stats.mapped_bytes -= bytes;
            Self::return_address(state, base, bytes);
            released += bytes;
        }
        state.free.retain(|_, slots| {
            slots.retain(|ptr| state.blocks.contains_key(ptr));
            !slots.is_empty()
        });
        let mut retired = Vec::new();
        for (id, pool) in &state.pools {
            if !pool.prepared
                && pool.references == 0
                && pool.capture.is_none()
                && !state
                    .segments
                    .values()
                    .any(|segment| segment.pool == Some(*id))
                && self.pool_completed(pool)?
            {
                retired.push(*id);
            }
        }
        for id in retired {
            let pool = &state.pools[&id];
            for event in pool.events.values().chain(&pool.retired_events) {
                self.driver.destroy_event(*event)?;
            }
            state.pools.remove(&id);
        }
        released += cache::trim_ready(self, state, limit.saturating_sub(released),
            maximum.saturating_sub(released), completed, None)?;
        Ok(released)
    }

    fn wait_reclaimable(
        &self,
        state: &mut State,
        limit: usize,
        exclude: Option<PoolId>,
    ) -> Result<usize> {
        self.poll(state)?;
        let mut candidates = Vec::new();
        let mut bytes = 0usize;
        for (base, segment) in &state.segments {
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            if let Some(id) = segment.pool {
                let pool = &state.pools[&id];
                if pool.prepared || pool.references != 0 || pool.capture.is_some() {
                    continue;
                }
            }
            if state
                .blocks
                .range(*base..*base + segment.bytes as u64)
                .all(|(_, block)| matches!(block.life, Life::Ready | Life::Pending))
            {
                candidates.push(*base);
                bytes = bytes.saturating_add(segment.bytes);
                if bytes >= limit {
                    break;
                }
            }
        }
        // Every event below has already been submitted. No live allocation, graph reference,
        // callback or future owner action is waited on. Only the allocation slow path calls this.
        for base in candidates {
            let segment = &state.segments[&base];
            if let Some(id) = segment.pool {
                let pool = &state.pools[&id];
                for event in pool.events.values().chain(&pool.retired_events) {
                    if !self.driver.query(*event)? {
                        self.driver.wait_event(*event)?;
                    }
                }
            }
            for (_, block) in state.blocks.range(base..base + segment.bytes as u64) {
                for event in block.events.values() {
                    if !self.driver.query(*event)? {
                        self.driver.wait_event(*event)?;
                    }
                }
            }
        }
        cache::wait_pending(self, state, limit.saturating_sub(bytes))?;
        self.trim(state, limit, exclude)
    }
}

/// Open native device state before any graph capture. Repeated initialization is harmless.
pub fn initialize(device: i32) -> Result<()> {
    initialize_owner(device).map(|_| ())
}

fn initialize_owner(device: i32) -> Result<Arc<Working>> {
    if let Some(owner) = existing(device)? {
        return Ok(owner);
    }
    let mut captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let activity = captures.entry(device).or_default();
    let room = Arc::clone(&activity.room);
    let external_callbacks = Arc::clone(&activity.external_callbacks);
    let mut devices = lock_devices()?;
    if let Some(owner) = devices.get(&device) {
        return Ok(Arc::clone(owner));
    }
    let driver = cuda::Driver::open(device)?;
    let page = rounded(2 << 20, driver.granularity)?;
    let (base, bytes) = {
        let _context = driver.enter()?;
        let (_, total) = driver.memory_info()?;
        let bytes = rounded(
            total
                .checked_mul(8)
                .ok_or(Error::Invalid("virtual arena size overflow"))?,
            page,
        )?;
        (driver.reserve(bytes)?, bytes)
    };
    let mut state = State::default();
    state.addresses.insert(base, bytes);
    let owner = Arc::new(Working {
        driver,
        page,
        room,
        external_callbacks,
        state: Mutex::new(state),
    });
    devices.insert(device, Arc::clone(&owner));
    Ok(owner)
}

fn valid_stream(stream: u64) -> Result<()> {
    if stream == 2 {
        return Err(Error::Invalid(
            "per-thread default CUDA streams are unsupported",
        ));
    }
    Ok(())
}

/// Allocate from a prepared pool during capture, or the device's working slabs otherwise.
pub fn alloc(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>) -> Result<u64> {
    let result = alloc_inner(device, bytes, stream, pool);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "allocation_error");
    }
    result
}

fn alloc_inner(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>) -> Result<u64> {
    let mut recovery = crate::Recovery::default();
    loop {
        let mut physical = None;
        let result = alloc_local(device, bytes, stream, pool, &mut physical);
        if pool.is_none() && let Err(Error::OutOfMemory { requested_bytes }) = &result
            && recovery.next(device, physical.unwrap_or(*requested_bytes), physical.is_some())?
        {
            continue; // All native guards are gone; retry the real allocator, never a grant.
        }
        if matches!(result, Err(Error::OutOfMemory { .. })) {
            owner(device)?.lock()?.stats.ooms += 1;
        }
        return result;
    }
}

fn alloc_local(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>,
               physical: &mut Option<usize>) -> Result<u64> {
    valid_stream(stream)?;
    if bytes == 0 {
        crate::check_process()?;
        return Ok(0);
    }
    let owner = if pool.is_none() {
        initialize_owner(device)?
    } else {
        owner(device)?
    };
    let class = size_class(bytes, owner.page)?;
    {
        let mut state = owner.lock()?;
        owner.callback_mutation(&mut state);
        if let Some(pool) = pool {
            let session = state
                .pools
                .get(&pool)
                .ok_or(Error::Invalid("unprepared graph pool"))?;
            if !session.routing || !session.streams.contains(&stream) {
                return Err(Error::Invalid(
                    "allocation stream is not routed to this capture",
                ));
            }
        }
        if let Some(ptr) = owner.take(&mut state, class, bytes, stream, pool) {
            return Ok(Working::activate(&mut state, ptr, bytes));
        }
        if let Some(id) = pool {
            if let Some(ptr) = owner.fit_pool(&mut state, id, class, bytes, stream) {
                return Ok(Working::activate(&mut state, ptr, bytes));
            }
            return Err(Error::OutOfMemory {
                requested_bytes: bytes,
            });
        }
    }
    // Never hold working state while asking the weight pager to reclaim: it has its own lock.
    for attempt in 0..3 {
        *physical = None;
        let result = {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, device)?;
            let mut state = owner.lock()?;
            owner.callback_mutation(&mut state);
            let _context = owner.driver.enter()?;
            let result = (|| -> crate::AllocationResult<u64> {
                owner.poll(&mut state)?;
                if let Some(ptr) = owner.take(&mut state, class, bytes, stream, None) {
                    return Ok(Working::activate(&mut state, ptr, bytes));
                }
                let mapped = if class <= 1 << 20 { owner.page } else { class };
                if attempt == 2 {
                    owner.wait_reclaimable(&mut state, mapped, None)?;
                } else if attempt != 0 {
                    owner.trim(&mut state, mapped, None)?;
                }
                owner.map(&mut state, mapped, None, class)?;
                let ptr = owner
                    .take(&mut state, class, bytes, stream, None)
                    .expect("new slab has a block");
                Ok(Working::activate(&mut state, ptr, bytes))
            })();
            result.map_err(|failure| {
                *physical = failure.physical_bytes();
                owner.fail(&mut state, failure.into())
            })
        };
        match result {
            Err(Error::OutOfMemory { .. }) if attempt == 0 => {
                // The next attempt trims only the mapping's need from completed working
                // pages. Keep other cached classes, and ask weights only if that retry fails.
                *physical = None;
            }
            Err(Error::OutOfMemory { .. }) if attempt == 1 => {
                *physical = None;
                crate::reclaim(device, class)?;
            }
            Err(error @ Error::OutOfMemory { .. }) => {
                return Err(error);
            }
            result => return result,
        }
    }
    unreachable!("finite allocation attempts")
}

pub fn free(device: i32, ptr: u64, stream: u64) -> Result<()> {
    let result = free_inner(device, ptr, stream);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "free_error");
    }
    result
}

fn free_inner(device: i32, ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        crate::check_process()?;
        return Ok(());
    }
    valid_stream(stream)?;
    // capture_counts checks process identity before either inherited registry mutex is touched.
    let captures = crate::capture_counts()?;
    let owner = lock_devices()?.get(&device).cloned().ok_or(Error::Invalid(
        "working allocator is not initialized on this device",
    ))?;
    let capturing = crate::ensure_not_capturing(&captures, device).is_err();
    let mut state = owner.lock()?;
    owner.callback_mutation(&mut state);
    let measured = state.phases.active.is_some();
    let transition = (|| {
        let State {
            blocks,
            segments,
            pools,
            ..
        } = &mut *state;
        let block = blocks
            .get_mut(&ptr)
            .ok_or(Error::Invalid("unknown working allocation"))?;
        if block.life != Life::Live {
            return Err(Error::Invalid("working allocation already freed"));
        }
        let pool = segments[&block.segment].pool;
        let in_pool_capture = pool.is_some_and(|id| pools[&id].routing);
        // The original stream is already tracked unless explicitly retired. Torch supplies
        // that original handle again on free; never reintroduce it after retirement.
        if stream != block.stream {
            block.streams.insert(stream);
        }
        let requested = block.requested;
        block.requested = 0;
        if in_pool_capture {
            block.life = Life::Captured;
            block.reusable_on = if block.streams.len() == 1 {
                block.streams.first().copied()
            } else {
                None
            };
        } else if block.streams.len() == 1 && block.streams.contains(&0) {
            // The legacy default stream cannot be destroyed. poll seals its deferred frees.
            block.life = Life::Deferred;
            block.reusable_on = Some(0);
        } else if capturing {
            block.life = Life::Deferred;
            block.reusable_on = None;
        } else {
            let _context = owner.driver.enter()?;
            owner.record_free(block)?;
        }
        let reusable = block.life == Life::Ready
            || (block.life == Life::Captured && block.reusable_on.is_some())
            || block.deferred_default();
        Ok((
            requested,
            pool,
            block.capacity,
            block.reusable_on,
            reusable,
            measured.then(|| (block.charge(), block.phase, block.segment)),
        ))
    })();
    let (requested, pool, class, reusable_on, reusable, observed) =
        transition.map_err(|error| owner.fail(&mut state, error))?;
    if reusable {
        state
            .free
            .entry((pool, class, reusable_on))
            .or_default()
            .push(ptr);
    }
    if let Some((charge, phase, segment)) = observed {
        state.phase_changed(class, charge, phase, phase, segment);
        split::refresh(&mut state);
    }
    state.stats.allocated_bytes -= requested;
    state.stats.frees += 1;
    Ok(())
}

/// Return an eager native allocation after every producer and consumer has completed.
/// No CUDA calls: completed native scratch immediately joins the same working free lists.
///
/// # Safety
/// The caller owns the allocation exclusively and has checked completion of every GPU use.
/// The pointer must not belong to a graph pool, whose replay lifetime is separately owned.
#[allow(unsafe_code)]
pub unsafe fn free_completed(device: i32, ptr: u64) -> Result<()> {
    retire_native(device, ptr, true)
}

/// Retire the last native owner when consumer streams are unknown. The next outside-capture
/// poll joins the primary context before reuse. This call itself performs no CUDA operation.
///
/// # Safety
/// No owner may submit further uses of this pointer; already submitted GPU work may remain.
/// The pointer must be a live eager allocation, never storage retained by a future graph replay.
#[allow(unsafe_code)]
pub unsafe fn free_untracked(device: i32, ptr: u64) -> Result<()> {
    retire_native(device, ptr, false)
}

fn retire_native(device: i32, ptr: u64, completed: bool) -> Result<()> {
    let result = retire_native_inner(device, ptr, completed);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "native_retirement_error");
    }
    result
}

fn retire_native_inner(device: i32, ptr: u64, completed: bool) -> Result<()> {
    if ptr == 0 {
        return crate::check_process();
    }
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    owner.callback_mutation(&mut state);
    let block = state
        .blocks
        .get(&ptr)
        .ok_or(Error::Invalid("unknown working allocation"))?;
    if block.life != Life::Live || state.segments[&block.segment].pool.is_some() {
        return Err(Error::Invalid(
            "native retirement requires a live eager allocation",
        ));
    }
    let block = state.blocks.get_mut(&ptr).expect("checked live allocation");
    let requested = block.requested;
    let capacity = block.capacity;
    let phase = block.phase;
    let segment = block.segment;
    block.requested = 0;
    block.life = if completed {
        Life::Ready
    } else {
        Life::Unjoined
    };
    block.reusable_on = None;
    block.streams.clear();
    if completed {
        state
            .free
            .entry((None, capacity, None))
            .or_default()
            .push(ptr);
    }
    state.phase_changed(
        capacity,
        if completed { 0 } else { capacity },
        phase,
        phase,
        segment,
    );
    if state.phases.active.is_some() {
        split::refresh(&mut state);
    }
    state.stats.allocated_bytes -= requested;
    state.stats.frees += 1;
    Ok(())
}

pub fn record_stream(device: i32, ptr: u64, stream: u64) -> Result<()> {
    let result = record_stream_inner(device, ptr, stream);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "record_stream_error");
    }
    result
}

fn record_stream_inner(device: i32, ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        crate::check_process()?;
        return Ok(());
    }
    valid_stream(stream)?;
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let block = state
        .blocks
        .get_mut(&ptr)
        .ok_or(Error::Invalid("unknown working allocation"))?;
    if block.life != Life::Live {
        return Err(Error::Invalid("record_stream after free"));
    }
    block.streams.insert(stream);
    Ok(())
}

pub fn record_stream_any(ptr: u64, stream: u64) -> Result<()> {
    let result = record_stream_any_inner(ptr, stream);
    if result.is_err()
        && let Ok(owners) = devices().map(|devices| devices.values().cloned().collect::<Vec<_>>())
    {
        for owner in owners {
            if let Ok(mut state) = owner.state.lock() {
                state.phases.invalidate("record_stream_error");
            }
        }
    }
    result
}

fn record_stream_any_inner(ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        crate::check_process()?;
        return Ok(());
    }
    valid_stream(stream)?;
    let candidates: Vec<_> = devices()?.values().cloned().collect();
    for owner in candidates {
        let mut state = owner.lock()?;
        if let Some((base, block)) = state.blocks.range_mut(..=ptr).next_back()
            && ptr - *base < block.capacity as u64
        {
            if block.life != Life::Live || ptr - *base >= block.requested as u64 {
                return Err(Error::Invalid(
                    "record_stream outside a live working allocation",
                ));
            }
            block.streams.insert(stream);
            return Ok(());
        }
        let mapped = state
            .segments
            .range(..=ptr)
            .next_back()
            .is_some_and(|(base, segment)| ptr - *base < segment.bytes as u64);
        let reserved = state
            .addresses
            .range(..=ptr)
            .next_back()
            .is_some_and(|(base, bytes)| ptr - *base < *bytes as u64);
        if mapped || reserved {
            return Err(Error::Invalid(
                "record_stream on unallocated working address space",
            ));
        }
    }
    // Native Torch ignores foreign DataPtr deleters. Their storage owner (for example a
    // TensorFS Region Pin) retains its own consumer/lifetime contract.
    Ok(())
}

/// Forget a stream only after its owner stopped submissions and synchronized it, before
/// destroying its handle. Live tensors may outlive the stream; new uses need record_stream.
pub fn retire_stream(device: i32, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    if stream == 0 || stream == 1 {
        return Err(Error::Invalid(
            "the legacy default stream cannot be retired",
        ));
    }
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let captures = crate::capture_counts()?;
    crate::ensure_not_capturing(&captures, device)?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    if owner.driver.is_capturing(stream as usize)? {
        return Err(Error::CaptureActive);
    }
    if !owner.driver.query_stream(stream as usize)? {
        return Err(Error::Busy);
    }
    let result = (|| {
        for block in state.blocks.values_mut() {
            let origin = block.stream == stream && !block.origin_retired;
            if !origin
                && !block.streams.contains(&stream)
                && !block.events.contains_key(&stream)
                && block.reusable_on != Some(stream)
            {
                continue;
            }
            if origin {
                block.origin_retired = true;
            }
            block.streams.remove(&stream);
            if let Some(event) = block.events.remove(&stream) {
                owner.driver.destroy_event(event)?;
            }
            if matches!(block.life, Life::Live | Life::Captured | Life::Unjoined) {
                // Retiring one known stream cannot prove completion of unknown DLPack users.
                continue;
            }
            if block.life == Life::Deferred {
                owner.record_free(block)?;
            }
            if owner.completed(&block.events)? {
                block.life = Life::Ready;
                block.reusable_on = None;
            } else {
                block.life = if block.streams.len() == 1 {
                    Life::Ready
                } else {
                    Life::Pending
                };
                block.reusable_on = Some(block.streams.first().copied().unwrap_or(block.stream));
            }
        }
        for pool in state.pools.values_mut() {
            pool.streams.remove(&stream);
            if let Some(event) = pool.events.remove(&stream) {
                owner.driver.destroy_event(event)?;
            }
        }
        state.free.clear();
        let ready: Vec<_> = state
            .blocks
            .iter()
            .filter(|(_, block)| block.life == Life::Ready)
            .map(|(ptr, block)| (*ptr, block.capacity, block.reusable_on, block.segment))
            .collect();
        for (ptr, bytes, stream, segment) in ready {
            let pool = state.segments[&segment].pool;
            state
                .free
                .entry((pool, bytes, stream))
                .or_default()
                .push(ptr);
        }
        state.refresh_phase();
        Ok(())
    })();
    result.map_err(|error| owner.fail(&mut state, error))
}

pub fn trim(device: i32) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let captures = crate::capture_counts()?;
    if crate::ensure_recovery_allowed(&captures, device).is_err() {
        return Ok(0);
    }
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    owner
        .trim(&mut state, usize::MAX, None)
        .map_err(|error| owner.fail(&mut state, error))
}

/// A cache-owner action has returned. Reclaim only known completed native backing;
/// no callback, future owner action or unknown-consumer context join is awaited here.
pub(crate) fn reclaim_completed(device: i32, wanted: usize) -> Result<usize> {
    let Some(owner) = existing(device)? else { return Ok(0); };
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let result = (|| {
        owner.poll_mode(&mut state, false)?;
        owner.trim_ready(&mut state, wanted, None, &BTreeSet::new())
    })();
    result.map_err(|error| owner.fail(&mut state, error))
}

pub fn trim_all() -> Result<usize> {
    let devices: Vec<_> = devices()?.keys().copied().collect();
    devices.into_iter().try_fold(0usize, |total, device| {
        Ok(total.saturating_add(trim(device)?))
    })
}

/// Weight fills call this while holding capture exclusion, before asking the driver for pages.
pub(crate) fn reclaim_for_weights(device: i32, bytes: usize) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let reclaimed = (|| {
        let completed = owner.trim(&mut state, bytes, None)?;
        if completed >= bytes {
            return Ok(completed);
        }
        let pending = owner.wait_reclaimable(&mut state, bytes - completed, None)?;
        Ok(completed.saturating_add(pending))
    })();
    reclaimed.map_err(|error| owner.fail(&mut state, error))
}

/// Begin one device-wide execution observation. None means the final configuration is not
/// known yet: observe demand, but do not admit speculation from an earlier guessed key.
pub fn phase_enter(device: i32, key: Option<String>, mode: PhaseMode) -> Result<u64> {
    if key.as_ref().is_some_and(String::is_empty) {
        return Err(Error::Invalid("phase execution key must not be empty"));
    }
    let owner = initialize_owner(device)?;
    let captures = crate::capture_counts()?;
    let mut state = owner.lock()?;
    if state.phases.active.is_some() {
        return Err(Error::Invalid(
            "execution phases cannot overlap on one device",
        ));
    }
    let token = state
        .phases
        .next
        .checked_add(1)
        .ok_or(Error::Invalid("phase token overflow"))?;
    state.phases.next = token;
    let charged = state.charged();
    let activity_epoch = captures
        .get(&device)
        .map_or(0, |activity| activity.activity_epoch);
    let invalid = (mode == PhaseMode::Eager
        && crate::ensure_recovery_allowed(&captures, device).is_err())
    .then(|| "capture_or_replay_active".to_owned());
    state.phases.active = Some(ActivePhase {
        token,
        key,
        mode,
        activity_epoch,
        charged,
        peak: charged,
        owned_charged: 0,
        owned_peak: 0,
        backing: 0,
        backing_peak: 0,
        invalid,
        replay: None,
    });
    state.refresh_phase();
    Ok(token)
}

/// One native continuation owns this observation until checked terminal completion.
/// Definition metadata adds no physical pin; current graph/output/event owners remain authoritative.
pub fn phase_enter_replay(
    device: i32,
    key: Option<String>,
    footprints: Vec<Arc<GraphFootprint>>,
) -> Result<u64> {
    let token = phase_enter(device, key, PhaseMode::Replay)?;
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let mut ranges = Ranges::default();
    let mut pages = Ranges::default();
    let mut invalid = footprints.is_empty();
    for footprint in &footprints {
        if footprint.device != device {
            invalid = true;
            break;
        }
        match footprint.ranges(&state) {
            Ok((used, mapped)) => {
                ranges.extend(&used);
                pages.extend(&mapped);
            }
            Err(_) => {
                invalid = true;
                break;
            }
        }
    }
    let active = state.phases.active.as_mut().expect("entered replay phase");
    if invalid {
        active.invalid = Some("graph_footprint_unqualified".into());
    } else {
        active.replay = Some(ReplayTrace {
            footprints,
            bytes: ranges.bytes(),
            backing: pages.bytes(),
        });
    }
    Ok(token)
}

/// Mark a caught retry/failed dispatch without replacing its caller's original exception.
/// A later successful outer return must not publish the contaminated peak.
pub fn phase_invalidate(device: i32, token: u64, reason: &str) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner
        .state
        .lock()
        .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
    if state
        .phases
        .active
        .as_ref()
        .is_none_or(|active| active.token != token)
    {
        return Err(Error::Invalid("unknown execution phase token"));
    }
    state.phases.invalidate(reason);
    split::invalidate(&mut state, reason);
    Ok(())
}

pub(crate) fn invalidate_current_phase(device: i32, reason: &str) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let mut state = owner
        .state
        .lock()
        .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
    state.phases.invalidate(reason);
    split::invalidate(&mut state, reason);
    if let Some(trace) = &mut state.trace {
        trace.invalid = true;
    }
    Ok(())
}

/// Publish only a successfully completed eager execution. The owner proves completion at its
/// existing stream/continuation boundary; capture completion is not execution completion.
/// Metadata cleanup remains possible after an allocator fault, retaining the prior sample.
pub fn phase_leave(
    device: i32,
    token: u64,
    completed: bool,
    key: Option<String>,
) -> Result<PhaseObservation> {
    if key.as_ref().is_some_and(String::is_empty) {
        return Err(Error::Invalid("phase execution key must not be empty"));
    }
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    let mut state = owner
        .state
        .lock()
        .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
    if state
        .phases
        .active
        .as_ref()
        .is_none_or(|active| active.token != token)
    {
        return Err(Error::Invalid("unknown execution phase token"));
    }
    state.refresh_phase();
    let mut active = state.phases.active.take().expect("checked phase token");
    if state.poison.is_some() {
        active
            .invalid
            .get_or_insert_with(|| "allocator_poisoned".to_owned());
    }
    if active.mode == PhaseMode::Eager
        && active.activity_epoch
            != captures
                .get(&device)
                .map_or(0, |activity| activity.activity_epoch)
    {
        active
            .invalid
            .get_or_insert_with(|| "capture_or_replay_attempt".to_owned());
    }
    if let Some(trace) = &active.replay {
        if trace
            .footprints
            .iter()
            .any(|footprint| footprint.ranges(&state).is_err())
        {
            active
                .invalid
                .get_or_insert_with(|| "graph_definition_retired".to_owned());
        }
        // Trace pages stay backed for the full stage. Any separately allocated native working
        // storage is disjoint eager backing; its observed peak therefore coexists with this set.
        active.owned_peak += trace.bytes;
        active.backing_peak += trace.backing;
    }
    if split::open_for(&state, active.token) {
        active
            .invalid
            .get_or_insert_with(|| "split_interval_open".to_owned());
    }
    let mismatch = active
        .key
        .as_ref()
        .zip(key.as_ref())
        .is_some_and(|(expected, actual)| expected != actual);
    if mismatch {
        active
            .invalid
            .get_or_insert_with(|| "execution_key_changed".to_owned());
    }
    let observed = if mismatch {
        active.key.clone()
    } else {
        key.or_else(|| active.key.clone())
    };
    let reason = active.invalid.clone().unwrap_or_else(|| {
        if active.mode != PhaseMode::Eager && active.replay.is_none() {
            format!("{}_unqualified", active.mode.name())
        } else if !completed {
            "aborted".to_owned()
        } else if observed.is_none() {
            "execution_key_unknown".to_owned()
        } else {
            String::new()
        }
    });
    let split_observations = split::phase_finished(
        &mut state,
        active.token,
        completed && active.mode == PhaseMode::Eager && active.invalid.is_none(),
        &reason,
    );
    let published = reason.is_empty() && active.invalid.is_none();
    let definitions: Vec<_> = active.replay.as_ref().map_or_else(Vec::new, |trace| {
        trace
            .footprints
            .iter()
            .map(|footprint| footprint.id)
            .collect()
    });
    let mut sample = PhaseStats::default();
    let record = if let Some(key) = &observed {
        if active.mode == PhaseMode::Eager {
            Some(state.phases.samples.entry(key.clone()).or_default())
        } else if active.mode == PhaseMode::Replay
            && !definitions.is_empty()
            && active.replay.as_ref().is_some_and(|trace| {
                trace
                    .footprints
                    .iter()
                    .all(|f| !f.retired.load(Ordering::Acquire))
            })
        {
            Some(
                state
                    .phases
                    .replay_samples
                    .entry(ReplayKey {
                        semantic: key.clone(),
                        definitions: definitions.clone(),
                    })
                    .or_default(),
            )
        } else {
            None
        }
    } else {
        None
    };
    if let Some(record) = record {
        if published {
            record.known = true;
            record.peak_bytes = active.owned_peak;
            record.total_peak_bytes = active.peak;
            record.backing_peak_bytes = active.backing_peak;
            record.successful_passes += 1;
        } else {
            record.aborted_passes += 1;
            record.aborted_peak_bytes = active.owned_peak;
            record.aborted_total_peak_bytes = active.peak;
            record.aborted_backing_peak_bytes = active.backing_peak;
        }
        sample = record.clone();
    }
    Ok(PhaseObservation {
        split_observations,
        definitions,
        token,
        mode: active.mode,
        key: observed,
        peak_bytes: active.owned_peak,
        total_peak_bytes: active.peak,
        backing_peak_bytes: active.backing_peak,
        published,
        invalidated: active.invalid.is_some(),
        reason,
        successful_peak_bytes: sample.peak_bytes,
        successful_backing_peak_bytes: sample.backing_peak_bytes,
        successful_passes: sample.successful_passes,
    })
}

pub fn phase_stats(device: i32, key: &str) -> Result<PhaseStats> {
    let Some(owner) = existing(device)? else {
        return Ok(PhaseStats::default());
    };
    let state = owner.lock()?;
    Ok(state.phases.samples.get(key).cloned().unwrap_or_default())
}

/// Retire measurements with their model/plan owner, never by an arbitrary age or timer.
pub fn phase_forget(device: i32, key: &str) -> Result<bool> {
    let Some(owner) = existing(device)? else {
        return Ok(false);
    };
    let mut state = owner.lock()?;
    if state
        .phases
        .active
        .as_ref()
        .is_some_and(|active| active.key.as_deref().is_none_or(|held| held == key))
    {
        return Err(Error::Busy);
    }
    let split_removed = split::forget(&mut state, key)?;
    let removed = state.phases.samples.remove(key).is_some();
    let previous = state.phases.replay_samples.len();
    state
        .phases
        .replay_samples
        .retain(|record, _| record.semantic != key);
    Ok(removed || split_removed || previous != state.phases.replay_samples.len())
}

/// Caller holds the capture registry and region state. Hold working metadata through physical
/// admission too, so another same-process working transition cannot spend this observation.
/// The closure performs only native no-reclaim page admission; never source IO or callbacks.
pub(crate) fn admit_prefetch<T>(
    device: i32,
    activity_epoch: u64,
    bytes: usize,
    admit: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    let Some(owner) = existing(device)? else {
        return Ok(None);
    };
    let mut state = owner.lock()?;
    state.stats.prefetch_attempts += 1;
    let gap = state.phases.active.as_ref().and_then(|active| {
        if active.mode != PhaseMode::Eager
            || active.invalid.is_some()
            || active.activity_epoch != activity_epoch
        {
            return None;
        }
        let sample = state.phases.samples.get(active.key.as_ref()?)?;
        if !sample.known {
            return None;
        }
        // Reserve measured physical backing, not just the sum of small block classes.
        // A currently shared outside slab cannot cover the remaining phase allocation need.
        // Graph-private cache is never general eager capacity.
        Some(
            sample.backing_peak_bytes.saturating_sub(
                state
                    .exclusive_phase_backing()
                    .saturating_add(state.reusable_eager_pages()),
            ),
        )
    });
    admit_with_gap(&owner, &mut state, bytes, gap, admit)
}

/// An adjacent source may use only the predecessor definition's current, qualified replay
/// sample. Neither a same-name recapture nor an unrelated active phase authorizes this edge.
pub(crate) fn admit_replay_prefetch<T>(
    device: i32,
    capability: &ReplayPrefetch,
    bytes: usize,
    admit: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    let Some(owner) = existing(device)? else {
        return Ok(None);
    };
    let mut state = owner.lock()?;
    state.stats.prefetch_attempts += 1;
    let gap = (|| {
        if capability.device != device || owner.external_callbacks.load(Ordering::Acquire) != 0 {
            return None;
        }
        let key = capability.semantic.as_ref()?;
        let active = state.phases.active.as_ref()?;
        if active.mode != PhaseMode::Replay
            || active.invalid.is_some()
            || active.key.as_ref() != Some(key)
        {
            return None;
        }
        let trace = active.replay.as_ref()?;
        let expected: Vec<_> = capability.footprints.iter().map(|f| f.id).collect();
        let actual: Vec<_> = trace.footprints.iter().map(|f| f.id).collect();
        if expected.is_empty()
            || expected != actual
            || capability
                .footprints
                .iter()
                .any(|footprint| footprint.ranges(&state).is_err())
        {
            return None;
        }
        let sample = state.phases.replay_samples.get(&ReplayKey {
            semantic: key.clone(),
            definitions: expected,
        })?;
        if !sample.known {
            return None;
        }
        // Actual trace pages are already backed by their graph owners. Exclusive current
        // eager backing and completed whole eager segments cover additional measured demand.
        let covered = trace
            .backing
            .saturating_add(state.exclusive_phase_backing())
            .saturating_add(state.reusable_eager_pages());
        Some(sample.backing_peak_bytes.saturating_sub(covered))
    })();
    admit_with_gap(&owner, &mut state, bytes, gap, admit)
}

fn admit_with_gap<T>(
    owner: &Working,
    state: &mut State,
    bytes: usize,
    gap: Option<usize>,
    admit: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    let Some(gap) = gap else {
        state.stats.prefetch_declines += 1;
        return Ok(None);
    };
    let Some(needed) = bytes.checked_add(gap) else {
        state.stats.prefetch_declines += 1;
        return Ok(None);
    };
    let _context = owner.driver.enter()?;
    let (free, _) = owner.driver.memory_info()?;
    if needed > free || owner.room.over(needed) != 0 {
        state.stats.prefetch_declines += 1;
        return Ok(None);
    }
    match admit() {
        Ok(prepared) => {
            state.stats.prefetch_admissions += 1;
            Ok(Some(prepared))
        }
        Err(Error::OutOfMemory { .. } | Error::Busy | Error::CaptureActive) => {
            state.stats.prefetch_declines += 1;
            Ok(None)
        }
        Err(error) => {
            state.phases.invalidate("prefetch_fault");
            Err(error)
        }
    }
}

pub fn stats(device: i32) -> Result<WorkingStats> {
    let Some(owner) = existing(device)? else {
        return Ok(WorkingStats::default());
    };
    let state = owner.lock()?;
    let mut stats = state.stats;
    stats.allocated_capacity = state
        .blocks
        .values()
        .filter(|b| b.life == Life::Live)
        .map(|b| b.capacity)
        .sum();
    stats.pending_bytes = state
        .blocks
        .values()
        .filter(|b| b.life != Life::Live && (b.life != Life::Ready || b.reusable_on.is_some()))
        .map(|b| b.capacity)
        .sum();
    stats.free_bytes = state
        .blocks
        .values()
        .filter(|b| b.life == Life::Ready && b.reusable_on.is_none())
        .map(|b| b.capacity)
        .sum();
    stats.reclaimable_bytes = state.reusable_eager_pages();
    stats.unassigned_bytes =
        stats.mapped_bytes - stats.allocated_capacity - stats.pending_bytes - stats.free_bytes;
    stats.pools_bytes = state
        .segments
        .values()
        .filter(|s| s.pool.is_some())
        .map(|s| s.bytes)
        .sum();
    stats.pools = state.pools.len();
    cache::working_stats(&state, owner.page, &mut stats);
    stats.event_handles = state
        .blocks
        .values()
        .map(|block| block.events.len())
        .sum::<usize>()
        + state
            .pools
            .values()
            .map(|pool| pool.events.len() + pool.retired_events.len())
            .sum::<usize>()
        + state.caches.values().map(|cache| cache.events.len()).sum::<usize>();
    (
        stats.driver_memory_calls,
        stats.event_queries,
        stats.event_records,
        stats.event_waits,
    ) = owner.driver.calls();
    stats.driver_physical_ooms = owner.driver.physical_ooms();
    Ok(stats)
}

pub fn reset_peak(device: i32) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let mut state = owner.lock()?;
    state.stats.allocated_peak = state.stats.allocated_bytes;
    state.stats.mapped_peak = state.stats.mapped_bytes;
    Ok(())
}

/// Abandon unused preparation without dropping any graph's reference to a shared pool.
pub fn pool_cancel_preparation(device: i32, pool: PoolId) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let mut state = owner.lock()?;
    if let Some(pool) = state.pools.get_mut(&pool) {
        pool.prepared = false;
    }
    Ok(())
}
pub fn pool_reserve(device: i32, pool: PoolId, bytes: usize) -> Result<usize> {
    invalidate_current_phase(device, "capture_preparation")?;
    initialize(device)?;
    let owner = owner(device)?;
    let wanted = rounded(bytes, owner.page)?;
    let mut recovery = crate::Recovery::default();
    loop {
        let mut physical = None;
        let result = pool_reserve_local(device, pool, wanted, &mut physical);
        if let Err(Error::OutOfMemory { requested_bytes }) = &result
            && recovery.next(device, physical.unwrap_or(*requested_bytes), physical.is_some())? {
            continue;
        }
        return result;
    }
}

fn pool_reserve_local(device: i32, pool: PoolId, wanted: usize,
                      physical: &mut Option<usize>) -> Result<usize> {
    let owner = owner(device)?;
    for attempt in 0..3 {
        *physical = None;
        let result = {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, device)?;
            let mut state = owner.lock()?;
            let session = state.pools.entry(pool).or_default();
            if session.references == 0 {
                session.prepared = true;
            }
            let have: usize = state
                .segments
                .values()
                .filter(|segment| segment.pool == Some(pool))
                .map(|segment| segment.bytes)
                .sum();
            if have >= wanted {
                return Ok(have);
            }
            let _context = owner.driver.enter()?;
            let result = (|| -> crate::AllocationResult<usize> {
                if attempt == 2 {
                    owner.wait_reclaimable(&mut state, wanted - have, Some(pool))?;
                } else {
                    owner.trim(&mut state, wanted - have, Some(pool))?;
                }
                owner.map(&mut state, wanted - have, Some(pool), 0)?;
                Ok(wanted)
            })();
            result.map_err(|failure| {
                *physical = failure.physical_bytes();
                owner.fail(&mut state, failure.into())
            })
        };
        match result {
            Err(Error::OutOfMemory { .. }) if attempt == 0 => {
                crate::reclaim(device, wanted)?;
            }
            Err(Error::OutOfMemory { .. }) if attempt == 1 => continue,
            other => return other,
        }
    }
    unreachable!("finite pool reserve attempts")
}

/// Register routing only. Repeated begin calls add streams to the same capture session.
pub fn pool_begin(device: i32, pool: PoolId, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    let guard = crate::enter_capture(device)?;
    let mut state = owner.lock()?;
    if state
        .pools
        .iter()
        .any(|(id, session)| *id != pool && session.capture.is_some())
    {
        return Err(Error::Invalid(
            "P0 capture sessions on one device must be sequential",
        ));
    }
    let pool = state.pools.entry(pool).or_default();
    if pool.capture.is_some() {
        if !pool.routing {
            return Err(Error::Invalid("previous CUDA capture has not completed"));
        }
    } else {
        pool.references = pool
            .references
            .checked_add(1)
            .ok_or(Error::Invalid("graph reference overflow"))?;
        pool.capture = Some(guard);
        pool.prepared = false;
        pool.routing = true;
        pool.streams.clear();
    }
    pool.streams.insert(stream);
    Ok(())
}

/// End allocation routing. This hook alone never proves CUDA capture has ended.
pub fn pool_end(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if !pool.routing {
        return Err(Error::Invalid("graph pool routing is not active"));
    }
    pool.routing = false;
    Ok(())
}

/// The graph owner calls this only after cudaStreamEndCapture, including failed captures.
pub fn pool_capture_complete(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let _captures = crate::capture_counts()?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.routing {
        return Err(Error::Invalid("graph pool routing is still active"));
    }
    if pool.capture.is_none() {
        return Err(Error::Invalid("graph capture is already complete"));
    }
    // Auxiliary streams (Torch RNG setup, for example) execute eagerly during capture.
    // Their frees are metadata-only until now, so fence them before permitting pool retirement.
    let completion = (|| {
        let _context = owner.driver.enter()?;
        for stream in &pool.streams {
            if owner.driver.is_capturing(*stream as usize)? {
                return Err(Error::CaptureActive);
            }
        }
        for stream in &pool.streams {
            owner.record(&mut pool.events, *stream)?;
        }
        Ok(())
    })();
    if let Err(error) = completion {
        return Err(owner.fail(&mut state, error));
    }
    let guard = pool
        .capture
        .take()
        .ok_or(Error::Invalid("graph capture is already complete"))?;
    // Cross-stream capture frees may serve only a later, sequential capture/replay, never an
    // unordered stream in this capture. Live outputs remain live blocks.
    let mut ready = Vec::new();
    let segments: BTreeSet<_> = state
        .segments
        .iter()
        .filter(|(_, segment)| segment.pool == Some(id))
        .map(|(base, _)| *base)
        .collect();
    for (ptr, block) in &mut state.blocks {
        if segments.contains(&block.segment) && block.life == Life::Captured {
            block.life = Life::Ready;
            block.reusable_on = None;
            ready.push((*ptr, block.capacity));
        }
    }
    for (ptr, bytes) in ready {
        state
            .free
            .entry((Some(id), bytes, None))
            .or_default()
            .push(ptr);
    }
    state.refresh_phase();
    drop(guard);
    Ok(())
}

/// Hold backing across a replay call whose end hooks may reset the original graph owner.
pub fn pool_retain(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_not_capturing(&captures, device)?;
    let mut state = owner.lock()?;
    state.phases.invalidate("graph_replay");
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 {
        return Err(Error::Invalid("graph pool has no owner"));
    }
    pool.references = pool
        .references
        .checked_add(1)
        .ok_or(Error::Invalid("graph reference overflow"))?;
    Ok(())
}

pub fn pool_record_stream(device: i32, id: PoolId, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_not_capturing(&captures, device)?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 {
        return Err(Error::Invalid("graph pool has no owner"));
    }
    let result = owner.record(&mut pool.events, stream);
    result.map_err(|error| {
        // A replay may already be running. Even event-allocation OOM cannot release backing
        // without a completion fence; retain the arena until its process is retired.
        let reason = format!("graph completion fence failed: {error}");
        state.poison = Some(reason.clone());
        Error::Poisoned(reason)
    })
}

/// Release one graph reference. Physical reclamation is deferred to trim/pressure.
pub fn pool_release(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 && pool.prepared {
        pool.prepared = false;
        return Ok(());
    }
    pool.references = pool
        .references
        .checked_sub(1)
        .ok_or(Error::Invalid("graph pool reference underflow"))?;
    Ok(())
}
