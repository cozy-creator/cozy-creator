//! Process-local working allocations and prepared graph pools, sharing the weight pager's
//! capture exclusion. Streams are native CUstream handles in the device's primary context.
//! A graph owner must call `pool_capture_complete` after CUDA capture actually ends and
//! `pool_record_stream` after every replay/consumer submission before dropping its reference.

#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub(crate) mod peer;

use crate::fence::Waits;
use crate::reclaim::{self, Protect, Wait};
use crate::{DeviceActivity, Error, GlobalCaptureGuard, Result, cuda};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

mod split;
mod cache;
mod execution;
mod observer;
mod packing;
pub use observer::{WorkingBlock, WorkingObserver, WorkingSegment, WorkingSnapshot, observe_working, working_snapshot};
pub use cache::{Cache, CacheStats};
pub use execution::Execution;
pub use split::{
    SplitContext, SplitObservation, SplitSelection, split_begin, split_context, split_end,
    split_select,
};

pub type PoolId = (u64, u64);

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
    /// Empty-pool capacity fitted to a key's first successful eager allocation/free sequence.
    pub capture_pool_bytes: usize,
    pub successful_passes: u64,
    pub aborted_passes: u64,
}

#[derive(Clone, Debug)]
pub struct PhaseObservation {
    pub split_observations: Vec<SplitObservation>,
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
    pub successful_capture_pool_bytes: usize,
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
    packing: Option<packing::Trace>,
    invalid: Option<String>,
    committed_site: Option<(String, String)>,
}

#[derive(Default)]
struct Phases {
    next: u64,
    active: Option<ActivePhase>,
    samples: BTreeMap<String, PhaseStats>,
    replay_samples: BTreeMap<String, PhaseStats>,
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
    // Freed on at most one stream: reusable there at once, elsewhere when `waits` complete.
    Ready,
    // Freed on several streams, or during a capture: reusable when `waits` complete.
    Pending,
    // Freed inside its pool's own capture: no real work exists yet.
    Captured,
    // An unannotated native final owner: a context join started after this mark covers it.
    Unjoined(u64),
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
    waits: Waits,
    // Listed in `State::fenced` for poll to settle.
    fenced: bool,
    life: Life,
    // None means all previous work is complete. Some(stream) permits ordered reuse only there.
    reusable_on: Option<u64>,
}

impl Block {
    fn free(segment: u64, capacity: usize) -> Self {
        Self {
            segment,
            capacity,
            requested: 0,
            stream: 0,
            origin_retired: false,
            phase: None,
            streams: BTreeSet::new(),
            waits: Waits::new(),
            fenced: false,
            life: Life::Ready,
            reusable_on: None,
        }
    }

    fn charge(&self) -> usize {
        if self.life != Life::Ready || self.reusable_on.is_some() {
            self.capacity
        } else {
            0
        }
    }

    #[inline]
    fn reusable_from(&self, origin: Option<u64>) -> bool {
        (self.life == Life::Ready || (self.life == Life::Captured && origin.is_some()))
            && self.reusable_on == origin
    }
}

struct Segment {
    bytes: usize,
    size_class: usize,
    handle: u64,
    pool: Option<PoolId>,
    // Rebuilt at phase entry; maintained only while that phase owns the device observation.
    phase_blocks: usize,
}

#[derive(Default)]
struct Pool {
    prepared: bool,
    // Set by preparation, spent by one capture. A capture that skipped it is a raw Torch graph.
    armed: bool,
    references: usize,
    routing: bool,
    capture: Option<GlobalCaptureGuard>,
    streams: BTreeSet<u64>,
    waits: Waits,
}

#[derive(Default)]
struct State {
    segments: BTreeMap<u64, Segment>,
    blocks: BTreeMap<u64, Block>,
    // (pool, size class, originating stream). Cross-stream candidates are promoted by poll.
    free: BTreeMap<(Option<PoolId>, usize, Option<u64>), Vec<u64>>,
    addresses: BTreeMap<u64, usize>,
    pools: BTreeMap<PoolId, Pool>,
    fenced: Vec<u64>,
    stats: WorkingStats,
    phases: Phases,
    poison: Option<String>,
    splits: split::Splits,
    caches: BTreeMap<u64, cache::Storage>,
    next_cache: u64,
    pressure_pending: bool,
    prefetch_suppressed: bool,
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
        let undeclared = self.phases.active.as_ref().is_some_and(|phase| {
            phase.committed_site.as_ref().is_some_and(|(key, choice)| {
                !split::matches_active(self, key, choice, phase.token)
            })
        });
        let Some(active) = &mut self.phases.active else {
            return;
        };
        if undeclared && next == Some(active.token) && after > before {
            active.invalid.get_or_insert_with(|| "undeclared_working_allocation".into());
        }
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
                segment.phase_blocks = 0;
            }
            let (mut charged, mut owned) = (0usize, 0usize);
            for block in self.blocks.values().filter(|block| block.charge() != 0) {
                charged += block.capacity;
                let segment = self
                    .segments
                    .get_mut(&block.segment)
                    .expect("live allocation segment");
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
}

pub(crate) struct Working {
    device: i32,
    driver: Arc<cuda::Driver>,
    page: usize,
    activity: &'static DeviceActivity,
    room: Arc<crate::Room>,
    state: Mutex<State>,
    cooperation_bound: AtomicBool,
    #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
    cooperation: Mutex<Option<crate::cooperative::ProcessKeepalive>>,
}

// A map observed by `observe_working` cannot race a device's creation.
static INITIALIZE: Mutex<()> = Mutex::new(());

/// Bind integration registration before CUDA under the one_process/1 contract. Linux's
/// default kernel guard is armed here; strict/non-Linux/setup-failure paths check actual PID.
/// A private-VM fork child rejects rather than arming inherited state.
pub fn bind_process() -> Result<()> {
    crate::check_process()?;
    #[cfg(all(target_os = "linux", feature = "kernel-fork-guard", not(feature = "strict-pid-guard")))]
    crate::fork_guard::arm()?;
    Ok(())
}

fn devices() -> Result<Vec<&'static Working>> {
    crate::check_process()?;
    Ok(crate::ACTIVITY.iter().filter_map(|slot| slot.get()?.working.get()).collect())
}

pub(crate) fn existing(device: i32) -> Result<Option<&'static Working>> {
    Ok(crate::activity(device)?.working.get())
}

fn owner(device: i32) -> Result<&'static Working> {
    existing(device)?.ok_or(Error::Invalid(
        "working allocator is not initialized on this device",
    ))
}

/// Whether an actual process/device service hold is installed. Never initializes CUDA.
pub fn cooperation_bound(device: i32) -> Result<bool> {
    Ok(existing(device)?.is_some_and(|owner| owner.cooperation_bound.load(Ordering::Acquire)))
}

pub fn require_cooperation(device: i32) -> Result<()> {
    if !cooperation_bound(device)? {
        return Err(Error::Invalid(
            "activate this Varena device before Torch GPU work",
        ));
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
pub(crate) fn retain_cooperation(
    device: i32,
    keepalive: crate::cooperative::ProcessKeepalive,
) -> Result<()> {
    let owner = owner(device)?; // Existing initialized Working only; no GPU creation here.
    let mut candidate = Some(keepalive);
    let result = {
        let mut held = owner
            .cooperation
            .lock()
            .map_err(|_| Error::Poisoned("working service custody".into()))?;
        match held.as_ref() {
            Some(existing)
                if !existing.same_service(candidate.as_ref().expect("owned candidate")) =>
            {
                Err(Error::Invalid(
                    "working storage already belongs to another service",
                ))
            }
            Some(_) => Ok(()),
            None => {
                *held = candidate.take();
                owner.cooperation_bound.store(true, Ordering::Release);
                Ok(())
            }
        }
    };
    drop(candidate); // No Working/capture/allocator/ledger guard owns a final host release.
    result
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
        if state.phases.active.is_some() && self.activity.external_callbacks.load(Ordering::Acquire) != 0 {
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
                | Error::PhaseOverlap
                | Error::CaptureActive
                | Error::Busy
        ) {
            state.poison = Some(error.to_string());
        }
        error
    }

    fn fail_allocation(&self, state: &mut State, failure: crate::AllocationFailure) -> crate::AllocationFailure {
        if failure.is_oom() {
            failure
        } else {
            self.fail(state, failure.into()).into()
        }
    }

    fn pool_completed(&self, pool: &Pool) -> bool {
        self.activity.fences().complete(&pool.waits)
    }

    /// Settle what completed. The caller holds the gate with no capture active.
    fn poll(&self, state: &mut State) -> Result<()> {
        let mut fences = self.activity.fences();
        fences.poll(&self.driver)?;
        cache::poll(state, &fences);
        let mut ready = Vec::new();
        for ptr in std::mem::take(&mut state.fenced) {
            let Some(block) = state.blocks.get_mut(&ptr) else {
                continue;
            };
            let complete = match block.life {
                Life::Ready | Life::Pending => fences.complete(&block.waits),
                Life::Unjoined(mark) => self.activity.joined(mark),
                Life::Live | Life::Captured => {
                    block.fenced = false;
                    continue;
                }
            };
            if complete {
                block.fenced = false;
                block.life = Life::Ready;
                block.reusable_on = None;
                block.waits.clear();
                ready.push((ptr, block.capacity, block.segment));
            } else {
                state.fenced.push(ptr);
            }
        }
        drop(fences);
        for (ptr, bytes, segment) in ready {
            state
                .free
                .entry((state.segments[&segment].pool, bytes, None))
                .or_default()
                .push(ptr);
        }
        state.refresh_phase();
        Ok(())
    }

    fn take(
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
                    block.waits.clear();
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
        packing::allocated(state, ptr);
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
                size_class: class,
                handle,
                pool,
                phase_blocks: 0,
            },
        );
        self.room
            .working
            .fetch_add(bytes, std::sync::atomic::Ordering::AcqRel);
        state.stats.mapped_bytes += bytes;
        state.stats.mapped_peak = state.stats.mapped_peak.max(state.stats.mapped_bytes);
        self.driver.map(ptr, bytes, handle)?;
        observer::mapped(self, ptr, bytes, pool, class)?;
        if let Some(pool) = pool {
            state.blocks.insert(ptr, Block::free(ptr, bytes));
            state.free.entry((Some(pool), bytes, None)).or_default().push(ptr);
        } else {
            for offset in (0..bytes).step_by(class) {
                if bytes - offset < class {
                    break;
                }
                let address = ptr + offset as u64;
                state.blocks.insert(address, Block::free(ptr, class));
                state.free.entry((None, class, None)).or_default().push(address);
            }
        }
        Ok(ptr)
    }

    fn fit_pool(
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
                    && matches!(block.life, Life::Ready | Life::Captured);
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
        let held = state.pools.get_mut(&pool).expect("owned pool");
        for part in parts {
            let block = state.blocks.remove(&part).expect("selected free pool block");
            // Merged blocks hand their stamps to the pool, which unmaps only after them all.
            for (id, sequence) in block.waits {
                match held.waits.iter_mut().find(|(known, _)| *known == id) {
                    Some(wait) => wait.1 = wait.1.max(sequence),
                    None => held.waits.push((id, sequence)),
                }
            }
        }
        state.free.retain(|(id, _, _), slots| {
            if *id == Some(pool) {
                slots.retain(|p| *p < ptr || *p >= end);
            }
            !slots.is_empty()
        });
        if capacity > bytes {
            let tail = ptr + bytes as u64;
            let order = if complete { None } else { Some(stream) };
            let mut block = Block::free(segment, capacity - bytes);
            block.stream = stream;
            if !complete {
                block.phase = phase;
                block.streams.insert(stream);
                block.life = Life::Captured;
                block.reusable_on = order;
            }
            state.blocks.insert(tail, block);
            state
                .free
                .entry((Some(pool), capacity - bytes, order))
                .or_default()
                .push(tail);
        }
        let mut block = Block::free(segment, bytes);
        block.requested = requested;
        block.stream = stream;
        block.phase = phase;
        block.streams.insert(stream);
        block.life = Life::Live;
        state.blocks.insert(ptr, block);
        state.refresh_phase();
        Some(ptr)
    }

    // Unmap completed free segments until `limit` bytes are gone, keeping `preserve`, the
    // `exclude` pool, and anything over `maximum`. Only actual releases count.
    fn trim_ready(
        &self, state: &mut State, limit: usize, exclude: Option<PoolId>,
        preserve: &BTreeSet<u64>, maximum: usize,
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
                    || !self.pool_completed(pool)
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
            observer::retiring(self, base, segment.bytes, segment.pool, segment.size_class)?;
            self.driver.unmap(base, segment.bytes)?;
            self.driver.release(segment.handle)?;
            let bytes = segment.bytes;
            self.room.working.fetch_sub(bytes, Ordering::AcqRel);
            self.room.released.fetch_add(bytes as u64, Ordering::AcqRel);
            let blocks: Vec<_> = state
                .blocks
                .range(base..base + bytes as u64)
                .map(|(p, _)| *p)
                .collect();
            for ptr in blocks {
                state.blocks.remove(&ptr);
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
        let segments = &state.segments;
        let fences = self.activity.fences();
        state.pools.retain(|id, pool| {
            pool.prepared
                || pool.references != 0
                || pool.capture.is_some()
                || segments.values().any(|segment| segment.pool == Some(*id))
                || !fences.complete(&pool.waits)
        });
        drop(fences);
        released += cache::trim_ready(self, state, limit.saturating_sub(released),
            maximum.saturating_sub(released), None)?;
        Ok(released)
    }

    // Events whose completion would free `limit` bytes of whole segments and released pages.
    // Each is already submitted: no live allocation, graph reference or owner action is awaited.
    fn pending_events(&self, state: &State, limit: usize, exclude: Option<PoolId>) -> Vec<usize> {
        let fences = self.activity.fences();
        let (mut events, mut bytes) = (Vec::new(), 0usize);
        for (base, segment) in &state.segments {
            if bytes >= limit {
                break;
            }
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            let pool = segment.pool.map(|id| &state.pools[&id]);
            if pool.is_some_and(|pool| pool.prepared || pool.references != 0 || pool.capture.is_some()) {
                continue;
            }
            let mut blocks = state.blocks.range(*base..*base + segment.bytes as u64);
            if !blocks.clone().all(|(_, block)| matches!(block.life, Life::Ready | Life::Pending)) {
                continue;
            }
            let before = events.len();
            for (_, block) in &mut blocks {
                fences.events(&block.waits, &mut events);
            }
            if let Some(pool) = pool {
                fences.events(&pool.waits, &mut events);
            }
            if events.len() != before {
                bytes += segment.bytes;
            }
        }
        cache::pending_events(state, &fences, self.page, limit.saturating_sub(bytes), &mut events);
        events
    }
}

/// Open native device state before any graph capture. Repeated initialization is harmless.
pub fn initialize(device: i32) -> Result<()> {
    initialize_owner(device).map(|_| ())
}

fn initialize_owner(device: i32) -> Result<&'static Working> {
    let activity = crate::activity(device)?;
    if let Some(owner) = activity.working.get() {
        return Ok(owner);
    }
    crate::gate(device)?.not_capturing()?;
    let _initializing = INITIALIZE.lock().map_err(|_| Error::Poisoned("working initialization".into()))?;
    if let Some(owner) = activity.working.get() {
        return Ok(owner);
    }
    let driver = activity.driver(device)?;
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
    Ok(activity.working.get_or_init(|| Working {
        device,
        driver,
        page,
        activity,
        room: activity.room.clone(),
        state: Mutex::new(state),
        cooperation_bound: AtomicBool::new(false),
        #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
        cooperation: Mutex::new(None),
    }))
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
    alloc_mode(device, bytes, stream, pool, false)
}

/// Torch callbacks must not initialize an unactivated device or create a service.
pub fn alloc_bound(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>) -> Result<u64> {
    alloc_mode(device, bytes, stream, pool, true)
}

fn alloc_mode(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>, bound: bool) -> Result<u64> {
    let result = alloc_inner(device, bytes, stream, pool, bound);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "allocation_error");
    }
    result
}

fn alloc_inner(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>, bound: bool) -> Result<u64> {
    valid_stream(stream)?;
    if bytes == 0 && !bound {
        crate::check_process()?;
        return Ok(0);
    }
    let owner = if bound {
        existing(device)?.ok_or(Error::Invalid("activate this Varena device before Torch GPU work"))?
    } else if pool.is_none() {
        initialize_owner(device)?
    } else {
        owner(device)?
    };
    if bound && !owner.cooperation_bound.load(Ordering::Acquire) {
        return Err(Error::Invalid("activate this Varena device before Torch GPU work"));
    }
    if bytes == 0 { return Ok(0); }
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
        if let Some(ptr) = Working::take(&mut state, class, bytes, stream, pool) {
            return Ok(Working::activate(&mut state, ptr, bytes));
        }
        if let Some(id) = pool {
            if let Some(ptr) = Working::fit_pool(&mut state, id, class, bytes, stream) {
                return Ok(Working::activate(&mut state, ptr, bytes));
            }
            state.stats.ooms += 1;
            return Err(Error::OutOfMemory {
                requested_bytes: bytes,
            });
        }
    }
    let mapped = if class <= 1 << 20 { owner.page } else { class };
    let result = reclaim::acquire(device, mapped, Wait::Yes, Protect::default(), || {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        owner.callback_mutation(&mut state);
        let _context = owner.driver.enter()?;
        let result = (|| -> crate::AllocationResult<u64> {
            owner.poll(&mut state)?;
            if let Some(ptr) = Working::take(&mut state, class, bytes, stream, None) {
                return Ok(Working::activate(&mut state, ptr, bytes));
            }
            owner.map(&mut state, mapped, None, class)?;
            let ptr = Working::take(&mut state, class, bytes, stream, None)
                .expect("new slab has a block");
            Ok(Working::activate(&mut state, ptr, bytes))
        })();
        result.map_err(|failure| owner.fail_allocation(&mut state, failure))
    });
    if matches!(result, Err(Error::OutOfMemory { .. })) {
        owner.lock()?.stats.ooms += 1;
    }
    result
}

pub fn free(device: i32, ptr: u64, stream: u64) -> Result<()> {
    let result = free_inner(device, ptr, stream);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "free_error");
    }
    result
}

/// Metadata only: no driver call, no gate. A stamp covers everything the stream has been given.
fn free_inner(device: i32, ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        crate::check_process()?;
        return Ok(());
    }
    valid_stream(stream)?;
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    // Under the state lock, so a capture entered after this read has not begun on any stream.
    let capturing = owner.activity.captures.load(Ordering::Acquire) != 0;
    owner.callback_mutation(&mut state);
    let measured = state.phases.active.is_some();
    let State {
        blocks,
        segments,
        pools,
        fenced,
        ..
    } = &mut *state;
    let block = blocks
        .get_mut(&ptr)
        .ok_or(Error::Invalid("unknown working allocation"))?;
    if block.life != Life::Live {
        return Err(Error::Invalid("working allocation already freed"));
    }
    let pool = segments[&block.segment].pool;
    // The original stream is already tracked unless explicitly retired. Torch supplies
    // that original handle again on free; never reintroduce it after retirement.
    if stream != block.stream {
        block.streams.insert(stream);
    }
    let requested = block.requested;
    block.requested = 0;
    if pool.is_some_and(|id| pools[&id].routing) {
        block.life = Life::Captured;
        block.reusable_on = if block.streams.len() == 1 {
            block.streams.first().copied()
        } else {
            None
        };
    } else {
        block.waits.clear();
        let mut fences = owner.activity.fences();
        for consumer in &block.streams {
            fences.stamp(*consumer, &mut block.waits);
        }
        drop(fences);
        let single = block.waits.len() <= 1 && !capturing;
        block.life = if single || block.waits.is_empty() { Life::Ready } else { Life::Pending };
        block.reusable_on = if capturing { None } else { block.streams.first().copied() };
        if !block.fenced && !block.waits.is_empty() {
            block.fenced = true;
            fenced.push(ptr);
        }
    }
    let reusable = block.life == Life::Ready
        || (block.life == Life::Captured && block.reusable_on.is_some());
    let class = block.capacity;
    let reusable_on = block.reusable_on;
    let observed = measured.then(|| (block.charge(), block.phase, block.segment));
    packing::freed(&mut state, ptr, false);
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
        Life::Unjoined(owner.activity.unjoined_mark())
    };
    block.reusable_on = None;
    block.streams.clear();
    block.waits.clear();
    if !completed && !block.fenced {
        block.fenced = true;
        state.fenced.push(ptr);
    }
    packing::freed(&mut state, ptr, completed);
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
        && let Ok(owners) = devices()
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
    if crate::tensor_consumers::record_pointer(ptr, stream)? {
        return Ok(());
    }
    valid_stream(stream)?;
    for owner in devices()? {
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
    // Unregistered foreign DataPtrs retain their own consumer/lifetime contract.
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
        if crate::consumer_streams::tracked(device, stream)? {
            return Err(Error::Invalid("tracked consumer retirement needs an initialized working device"));
        }
        return Ok(());
    };
    crate::consumer_streams::retire(device, stream, || retire_stream_inner(owner, device, stream))
}

fn retire_stream_inner(owner: &Working, device: i32, stream: u64) -> Result<()> {
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    if owner.driver.is_capturing(stream as usize)? {
        return Err(Error::CaptureActive);
    }
    if !owner.driver.query_stream(stream as usize)? {
        return Err(Error::Busy);
    }
    let mut fences = owner.activity.fences();
    fences.retire(stream);
    for block in state.blocks.values_mut() {
        let origin = block.stream == stream && !block.origin_retired;
        if !origin && !block.streams.contains(&stream) && block.reusable_on != Some(stream) {
            continue;
        }
        if origin {
            block.origin_retired = true;
        }
        block.streams.remove(&stream);
        if matches!(block.life, Life::Live | Life::Captured | Life::Unjoined(_)) {
            // Retiring one known stream cannot prove completion of unknown DLPack users.
            continue;
        }
        if fences.complete(&block.waits) {
            block.life = Life::Ready;
            block.reusable_on = None;
            block.waits.clear();
        } else {
            block.life = if block.streams.len() == 1 {
                Life::Ready
            } else {
                Life::Pending
            };
            block.reusable_on = Some(block.streams.first().copied().unwrap_or(block.stream));
        }
    }
    drop(fences);
    for pool in state.pools.values_mut() {
        pool.streams.remove(&stream);
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
}

pub fn trim(device: i32) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let gate = crate::gate(device)?;
    if gate.not_capturing().is_err() {
        return Ok(0);
    }
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let result = owner.poll(&mut state)
        .and_then(|()| owner.trim_ready(&mut state, usize::MAX, None, &BTreeSet::new(), usize::MAX));
    result.map_err(|error| owner.fail(&mut state, error))
}

pub fn trim_all() -> Result<usize> {
    devices()?.into_iter().try_fold(0usize, |total, owner| {
        Ok(total.saturating_add(trim(owner.device)?))
    })
}

/// The first step of the reclaim order: completed free pages, with no waiting.
pub(crate) fn release_completed(device: i32, want: usize, maximum: usize, keep: Option<PoolId>) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let result = owner.poll(&mut state)
        .and_then(|()| owner.trim_ready(&mut state, want, keep, &BTreeSet::new(), maximum));
    result.map_err(|error| owner.fail(&mut state, error))
}

/// Wait outside every lock for submitted work on memory already freed. False: nothing pending.
pub(crate) fn wait_pending(device: i32, want: usize, keep: Option<PoolId>) -> Result<bool> {
    let Some(owner) = existing(device)? else {
        return Ok(false);
    };
    let (events, hold) = {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        let _context = owner.driver.enter()?;
        let polled = owner.poll(&mut state);
        polled.map_err(|error| owner.fail(&mut state, error))?;
        let events = owner.pending_events(&state, want, keep);
        if events.is_empty() {
            return Ok(false);
        }
        (events, gate.hold()?)
    };
    reclaim::wait_outside(device, owner.activity, &owner.driver, &events, false)?;
    drop(hold);
    Ok(true)
}

/// Whether a context join would release anything here.
pub(crate) fn unjoined(device: i32) -> Result<bool> {
    let Some(owner) = existing(device)? else {
        return Ok(false);
    };
    let state = owner.lock()?;
    Ok(state.fenced.iter().filter_map(|ptr| state.blocks.get(ptr)).any(|block| {
        matches!(block.life, Life::Unjoined(mark) if !owner.activity.joined(mark))
    }))
}

/// A failed GPU wait: nothing here can prove completion any more.
pub(crate) fn poison(device: i32, error: &Error) {
    if let Ok(Some(owner)) = existing(device)
        && let Ok(mut state) = owner.state.lock()
    {
        state.poison.get_or_insert_with(|| error.to_string());
    }
}

/// Begin one device-wide execution observation. None means the final configuration is not
/// known yet: observe demand, but do not admit speculation from an earlier guessed key.
pub fn phase_enter(device: i32, key: Option<String>, mode: PhaseMode) -> Result<u64> {
    phase_enter_inner(device, key, mode, None, None)
}

fn phase_enter_inner(
    device: i32,
    key: Option<String>,
    mode: PhaseMode,
    committed_site: Option<(String, String)>,
    expected_plan_generation: Option<u64>,
) -> Result<u64> {
    if key.as_ref().is_some_and(String::is_empty) {
        return Err(Error::Invalid("phase execution key must not be empty"));
    }
    let owner = initialize_owner(device)?;
    if let Some(expected) = expected_plan_generation
        && crate::plan::generation(device)? != expected
    {
        return Err(Error::Invalid("committed phase-plan inventory changed before entry"));
    }
    let mut state = owner.lock()?;
    if state.phases.active.is_some() {
        return Err(Error::PhaseOverlap);
    }
    if committed_site.is_some() && state.splits.active_is_present() {
        return Err(Error::Invalid("committed execution cannot overlap an active split"));
    }
    if !state.pressure_pending {
        state.prefetch_suppressed = false;
    }
    let token = state
        .phases
        .next
        .checked_add(1)
        .ok_or(Error::Invalid("phase token overflow"))?;
    state.phases.next = token;
    let charged = state.charged();
    let activity = owner.activity;
    let invalid = (mode == PhaseMode::Eager && activity.graph_active())
        .then(|| "capture_or_replay_active".to_owned());
    // Packing simulates the whole pass, so only a key's first successful one pays for it.
    let wanted = key.as_ref().is_some_and(|key| !state.phases.samples.get(key).is_some_and(|sample| sample.known));
    state.phases.active = Some(ActivePhase {
        token,
        key,
        mode,
        activity_epoch: activity.epoch(),
        charged,
        peak: charged,
        owned_charged: 0,
        owned_peak: 0,
        backing: 0,
        backing_peak: 0,
        packing: (mode == PhaseMode::Eager && wanted).then(packing::Trace::default),
        invalid,
        committed_site,
    });
    state.refresh_phase();
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
    if active.mode == PhaseMode::Eager && active.activity_epoch != owner.activity.epoch() {
        active
            .invalid
            .get_or_insert_with(|| "capture_or_replay_attempt".to_owned());
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
    let qualified = matches!(active.mode, PhaseMode::Eager | PhaseMode::Replay);
    let reason = active.invalid.clone().unwrap_or_else(|| {
        if !qualified {
            format!("{}_unqualified", active.mode.name())
        } else if !completed {
            "aborted".to_owned()
        } else if observed.is_none() {
            "execution_key_unknown".to_owned()
        } else {
            String::new()
        }
    });
    let published = reason.is_empty();
    let capture_pool_bytes = if published {
        let measured = active.packing.as_ref()
            .map(|trace| trace.capacity(active.owned_peak, owner.page))
            .transpose();
        match measured {
            Ok(bytes) => bytes,
            Err(error) => {
                split::phase_finished(&mut state, active.token, false, "capture_packing_invalid");
                return Err(error);
            }
        }
    } else {
        None
    };
    let split_observations = split::phase_finished(
        &mut state,
        active.token,
        completed && active.mode == PhaseMode::Eager && active.invalid.is_none(),
        &reason,
    );
    let mut sample = PhaseStats::default();
    let record = observed.as_ref().filter(|_| qualified).map(|key| {
        let samples = if active.mode == PhaseMode::Replay {
            &mut state.phases.replay_samples
        } else {
            &mut state.phases.samples
        };
        samples.entry(key.clone()).or_default()
    });
    if let Some(record) = record {
        if published {
            record.known = true;
            record.peak_bytes = active.owned_peak;
            record.total_peak_bytes = active.peak;
            record.backing_peak_bytes = active.backing_peak;
            if let Some(bytes) = capture_pool_bytes {
                record.capture_pool_bytes = bytes;
            }
            record.successful_passes += 1;
        } else {
            record.aborted_passes += 1;
        }
        sample = record.clone();
    }
    Ok(PhaseObservation {
        split_observations,
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
        successful_capture_pool_bytes: sample.capture_pool_bytes,
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
    let replayed = state.phases.replay_samples.remove(key).is_some();
    Ok(removed || replayed || split_removed)
}

/// What the rest of this phase still needs of working memory, per its last pass, and the
/// completed free pages already mapped for it. None declines: unknown phase, or pressure.
pub(crate) fn prefetch_gap(device: i32) -> Result<Option<(usize, usize)>> {
    let Some(owner) = existing(device)? else {
        return Ok(None);
    };
    let mut state = owner.lock()?;
    state.stats.prefetch_attempts += 1;
    let answer = (|| {
        if state.prefetch_suppressed {
            return None;
        }
        let active = state.phases.active.as_ref().filter(|active| active.invalid.is_none())?;
        let samples = match active.mode {
            PhaseMode::Eager => &state.phases.samples,
            PhaseMode::Replay => &state.phases.replay_samples,
            _ => return None,
        };
        let sample = samples.get(active.key.as_ref()?).filter(|sample| sample.known)?;
        Some((sample.backing_peak_bytes.saturating_sub(active.backing), state.reusable_eager_pages()))
    })();
    if answer.is_none() {
        state.stats.prefetch_declines += 1;
    }
    Ok(answer)
}

pub(crate) fn prefetch_admitted(device: i32) -> Result<()> {
    if let Some(owner) = existing(device)? {
        owner.lock()?.stats.prefetch_admissions += 1;
    }
    Ok(())
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
    stats.event_handles = owner.activity.fences().handles();
    stats.context_joins = owner.driver.context_joins();
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
        pool.armed = false;
    }
    Ok(())
}
pub fn pool_reserve(device: i32, pool: PoolId, bytes: usize) -> Result<usize> {
    invalidate_current_phase(device, "capture_preparation")?;
    let owner = initialize_owner(device)?;
    let wanted = rounded(bytes, owner.page)?;
    let keep = Protect { pool: Some(pool), ..Default::default() };
    reclaim::acquire(device, wanted, Wait::Yes, keep, || {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        let session = state.pools.entry(pool).or_default();
        session.armed = true;
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
        let result = owner.map(&mut state, wanted - have, Some(pool), 0).map(|_| wanted);
        result.map_err(|failure| owner.fail_allocation(&mut state, failure))
    })
}

/// Register routing only. Repeated begin calls add streams to the same capture session.
/// A fresh capture must be prepared: a raw Torch graph supplies none of the lifetime signals
/// this needs, and would otherwise hold capture exclusion forever.
pub fn pool_begin(device: i32, pool: PoolId, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    {
        let mut state = owner.lock()?;
        if let Some(session) = state.pools.get_mut(&pool) {
            if session.capture.is_some() {
                if !session.routing {
                    return Err(Error::Invalid("previous CUDA capture has not completed"));
                }
                session.streams.insert(stream);
                return Ok(());
            }
        }
        if !state.pools.get(&pool).is_some_and(|session| session.armed) {
            return Err(Error::Invalid(
                "raw torch.cuda.graph is unsupported under Varena: capture with tensorfs.varena.torch.graph",
            ));
        }
    }
    let guard = crate::enter_capture(device)?;
    let mut state = owner.lock()?;
    if state
        .pools
        .iter()
        .any(|(id, session)| *id != pool && session.capture.is_some())
    {
        return Err(Error::Invalid(
            "capture sessions on one device must be sequential",
        ));
    }
    let pool = state.pools.entry(pool).or_default();
    pool.references = pool
        .references
        .checked_add(1)
        .ok_or(Error::Invalid("graph reference overflow"))?;
    pool.capture = Some(guard);
    pool.armed = false;
    pool.prepared = false;
    pool.routing = true;
    pool.streams.clear();
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
    let _gate = crate::gate(device)?;
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
    let completion = (|| {
        let _context = owner.driver.enter()?;
        for stream in &pool.streams {
            if owner.driver.is_capturing(*stream as usize)? {
                return Err(Error::CaptureActive);
            }
        }
        Ok(())
    })();
    if let Err(error) = completion {
        return Err(owner.fail(&mut state, error));
    }
    // Auxiliary streams (Torch RNG setup, for example) execute eagerly during capture.
    // Their frees were metadata-only until now: stamp them before permitting pool retirement.
    let mut fences = owner.activity.fences();
    for stream in &pool.streams {
        fences.stamp(*stream, &mut pool.waits);
    }
    drop(fences);
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
    if owner.activity.captures.load(Ordering::Acquire) != 0 {
        return Err(Error::CaptureActive);
    }
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

/// A replay or consumer was submitted on this stream. Metadata only: no driver call.
pub fn pool_record_stream(device: i32, id: PoolId, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    if owner.activity.captures.load(Ordering::Acquire) != 0 {
        return Err(Error::CaptureActive);
    }
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 {
        return Err(Error::Invalid("graph pool has no owner"));
    }
    owner.activity.fences().stamp(stream, &mut pool.waits);
    Ok(())
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
