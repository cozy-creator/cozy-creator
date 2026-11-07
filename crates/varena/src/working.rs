//! Process-local working allocations and prepared graph pools, sharing the weight pager's
//! capture exclusion. Streams are native CUstream handles in the device's primary context.
//! A graph owner must call `pool_capture_complete` after CUDA capture actually ends and
//! `pool_record_stream` after every replay/consumer submission before dropping its reference.

#[cfg(target_os = "linux")]
pub(crate) mod peer;

use crate::fence::Waits;
use crate::reclaim::{self, Protect, Wait};
use crate::{Device, Error, GlobalCaptureGuard, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

mod split;
mod cache;
pub mod checks;
mod observer;
mod escape;
pub use escape::{EscapeRecord, Verdict, escapes_take, pass_mark, set_site, site_id};
pub use observer::{WorkingBlock, WorkingObserver, WorkingSegment, WorkingSnapshot, observe_working, working_snapshot};
pub use cache::{Cache, CacheStats};
pub use split::{
    Entered, Exit, Leaf, Refusal, Room, SplitContext, SplitObservation, SplitSelection, admission_refusal, room, site_choose,
    site_enter, site_exit, site_register, split_begin, split_context, split_end, split_select,
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
    pub driver_memory_ns: u64,
    pub driver_memory_longest_ns: u64,
    pub observed_mapped: u64,
    pub observed_retired: u64,
    pub observed_ns: u64,
    pub reclaim_ns: u64,
    pub reclaim_longest_ns: u64,
    pub reclaim_longest_step: u64,
    pub reclaim_step_ns: [u64; crate::reclaim::STEPS],
    pub event_queries: u64,
    pub event_records: u64,
    pub event_waits: u64,
    pub context_joins: u64,
    pub prefetch_attempts: u64,
    pub prefetch_admissions: u64,
    pub prefetch_declines: u64,
    /// Split levels chosen by reading the room, and those looked up as already chosen for it.
    pub split_decisions: u64,
    pub split_lookups: u64,
    /// Moves of the room decisions are kept for, and of those, growing outside use's.
    pub room_moves: u64,
    pub room_outside_moves: u64,
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

#[derive(Clone)]
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
    workspace: bool,
    // Live, but its owner declared its contents dead between uses of its pool.
    scratch: bool,
    life: Life,
    // None means all previous work is complete. Some(stream) permits ordered reuse only there.
    reusable_on: Option<u64>,
    // A live eager block's (birth pass, site) in `escape`'s tally.
    tally: Option<(u32, u32)>,
}

impl Segment {
    fn new(bytes: usize, size_class: usize, pool: Option<PoolId>) -> Self {
        let chunk = if pool.is_none() { bytes.min(CHUNK) } else { bytes };
        let chunks = (0..bytes).step_by(chunk)
            .map(|offset| Chunk { offset, bytes: chunk.min(bytes - offset), handle: None })
            .collect();
        Self { bytes, size_class, chunks, pool, phase_blocks: 0, touched: 0, guard: 0 }
    }

    fn backed(&self) -> bool {
        self.chunks.iter().any(|chunk| chunk.handle.is_some())
    }

    fn unbacked(&self) -> usize {
        self.chunks.iter().filter(|chunk| chunk.handle.is_none()).map(|chunk| chunk.bytes).sum()
    }
}

impl Pool {
    /// A graph here may have bound a stream's cached workspace.
    fn names_stream(&self, stream: u64) -> bool {
        (self.references != 0 || self.capture.is_some()) && self.streams.contains(&stream)
    }
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
            workspace: false,
            scratch: false,
            life: Life::Ready,
            reusable_on: None,
            tally: None,
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

#[derive(Clone, Copy)]
struct Chunk {
    offset: usize,
    bytes: usize,
    // None: its pages went back. The VA stays until the segment goes, or an idle pool maps again.
    handle: Option<u64>,
}

struct Segment {
    bytes: usize,
    size_class: usize,
    // One handle's pages come back only whole, so an eager segment has one per CHUNK.
    chunks: Vec<Chunk>,
    pool: Option<PoolId>,
    // Rebuilt at phase entry; maintained only while that phase owns the device observation.
    phase_blocks: usize,
    // The plan phase (`State::plan_epoch`) that last held a phase's block here. A loop keeps it.
    touched: u64,
    // Unmapped VA after it (`checks::guard`), which goes back with it.
    guard: usize,
}

#[derive(Default)]
struct Pool {
    prepared: bool,
    // Set by preparation, spent by one capture. A capture that skipped it is a raw Torch graph.
    armed: bool,
    // The owner declared every allocation here dead between uses: the core may unmap it idle.
    scratch: bool,
    // From `pool_reserve` until the owner's last use is stamped.
    in_use: bool,
    unmapped: bool,
    last_use: u64,
    references: usize,
    routing: bool,
    capture: Option<GlobalCaptureGuard>,
    streams: BTreeSet<u64>,
    // Streams whose library workspaces Torch already holds for this capture (`pool_arm`): an
    // allocation there is ordinary whatever its size.
    primed: BTreeSet<u64>,
    // The largest allocation this capture's pool had no room for (`pool_short`).
    short: usize,
    waits: Waits,
}

/// The one table of every byte on this device: weight regions, working blocks, graph pools,
/// owner caches.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) entries: BTreeMap<u64, crate::Entry>,
    pub(crate) phase_boundary: Option<u64>,
    pub(crate) plan_phase: Option<crate::PlanPhase>,
    // Counts plan phase entries.
    pub(crate) plan_epoch: u64,
    // Whether each plan phase looped last time it ran.
    pub(crate) passes: BTreeMap<String, bool>,
    // The steps of the loop running now, as its caller declared them (`crate::repeating`).
    pub(crate) repeats: Option<u32>,
    // The Region used last: its `next` is this use's.
    pub(crate) last_used: Option<u64>,
    pub(crate) fills: u64,
    pub(crate) filled_bytes: u64,
    pub(crate) hits: u64,
    pub(crate) evictions: u64,
    pub(crate) reclaimed_bytes: u64,
    pub(crate) prefetch_unused_bytes: u64,
    pub(crate) reclaimed_by_reason: crate::ReclaimedBytes,
    segments: BTreeMap<u64, Segment>,
    blocks: BTreeMap<u64, Block>,
    // (pool, size class, originating stream). Cross-stream candidates are promoted by poll.
    free: BTreeMap<(Option<PoolId>, usize, Option<u64>), Vec<u64>>,
    // The free blocks of eager runs by (capacity, address): a cut's best fit in O(log n).
    runs: BTreeSet<(usize, u64)>,
    addresses: BTreeMap<u64, usize>,
    pools: BTreeMap<PoolId, Pool>,
    fenced: Vec<u64>,
    stats: WorkingStats,
    phases: Phases,
    pub(crate) poison: Option<String>,
    splits: split::Splits,
    caches: BTreeMap<u64, cache::Storage>,
    next_cache: u64,
    pressure_pending: bool,
    prefetch_suppressed: bool,
    // A cached library workspace's exact sizes, when Torch caches them per stream.
    workspace_sizes: Vec<usize>,
    // Freed workspaces by (stream, address): a graph on that stream may still name them.
    parked_workspaces: BTreeSet<(u64, u64)>,
    escapes: escape::Escapes,
}

impl State {
    /// A free block onto its list, and a run's onto the cut index too. The block is in the table.
    fn list(&mut self, key: (Option<PoolId>, usize, Option<u64>), ptr: u64) {
        self.free.entry(key).or_default().push(ptr);
        if key.0.is_none() && self.blocks.get(&ptr).is_some_and(|block| self.segments[&block.segment].size_class > LARGE) {
            self.runs.insert((key.1, ptr));
        }
    }

    fn unlist(&mut self, key: (Option<PoolId>, usize, Option<u64>), ptr: u64) {
        self.runs.remove(&(key.1, ptr));
        if let Some(slots) = self.free.get_mut(&key) {
            if let Some(index) = slots.iter().rposition(|listed| *listed == ptr) {
                slots.swap_remove(index);
            }
            if slots.is_empty() {
                self.free.remove(&key);
            }
        }
    }

    /// Adjacent free blocks with the same ordering. A completed block has no phase charge, so its
    /// tag is stale and doesn't matter; a pending one's charge stays with its tag.
    fn joinable(&self, left: u64, right: u64) -> bool {
        let (Some(a), Some(b)) = (self.blocks.get(&left), self.blocks.get(&right)) else { return false };
        a.segment == b.segment && a.life == Life::Ready && b.life == Life::Ready
            && a.reusable_on == b.reusable_on && (a.reusable_on.is_none() || a.phase == b.phase)
    }

    fn join(&mut self, left: u64, right: u64) {
        let token = self.phases.active.as_ref().map(|active| active.token);
        let gone = self.blocks.remove(&right).expect("joined block");
        self.unlist((None, gone.capacity, gone.reusable_on), right);
        if gone.charge() != 0 && gone.phase.is_some() && gone.phase == token {
            let segment = self.segments.get_mut(&gone.segment).expect("owned segment");
            segment.phase_blocks = segment.phase_blocks.saturating_sub(1);
        }
        let (capacity, origin) = {
            let block = &self.blocks[&left];
            (block.capacity, block.reusable_on)
        };
        self.unlist((None, capacity, origin), left);
        let block = self.blocks.get_mut(&left).expect("joined block");
        block.capacity += gone.capacity;
        block.streams.extend(gone.streams);
        for (id, sequence) in gone.waits {
            match block.waits.iter_mut().find(|(known, _)| *known == id) {
                Some(wait) => wait.1 = wait.1.max(sequence),
                None => block.waits.push((id, sequence)),
            }
        }
        let fence = gone.fenced && !block.fenced;
        block.fenced |= gone.fenced;
        let capacity = block.capacity;
        if fence {
            self.fenced.push(left);
        }
        self.list((None, capacity, origin), left);
    }

    fn charged(&self) -> usize {
        self.blocks.values().map(Block::charge).sum()
    }

    /// The blocks on any part of `[start, end)`, the one starting before it included.
    fn touching(&self, start: u64, end: u64) -> impl Iterator<Item = &Block> {
        self.blocks.range(..end).rev()
            .take_while(move |(ptr, block)| **ptr + block.capacity as u64 > start)
            .map(|(_, block)| block)
    }

    /// Whether this handle's pages could go back now: only completed free blocks on them.
    fn chunk_free(&self, base: u64, chunk: &Chunk) -> bool {
        let start = base + chunk.offset as u64;
        self.touching(start, start + chunk.bytes as u64)
            .all(|block| block.life == Life::Ready && block.reusable_on.is_none())
    }

    /// What of this segment could go back now.
    fn releasable(&self, base: u64, segment: &Segment) -> usize {
        segment.chunks.iter()
            .filter(|chunk| chunk.handle.is_some() && self.chunk_free(base, chunk))
            .map(|chunk| chunk.bytes)
            .sum()
    }

    fn reusable_eager_pages(&self) -> usize {
        self.segments.iter()
            .filter(|(_, segment)| segment.pool.is_none())
            .map(|(base, segment)| self.releasable(*base, segment))
            .sum()
    }

    /// A loop keeps the working pages its phases touched: what one step used the next uses again,
    /// and mapping it back costs driver calls on the submitting thread (Qwen 2 MP at 8 GiB: 950 a
    /// step, and a GPU drain for each wait on its own frees).
    pub(super) fn looping(&self) -> bool {
        self.plan_phase.as_ref().is_some_and(crate::PlanPhase::keeps)
    }

    /// The segments a loop keeps whole.
    pub(super) fn kept(&self) -> BTreeSet<u64> {
        if !self.looping() {
            return BTreeSet::new();
        }
        self.segments.iter().filter(|(_, segment)| segment.pool.is_none() && segment.touched == self.plan_epoch)
            .map(|(base, _)| *base).collect()
    }

    /// What the active eager phase still needs of working memory, if measured: its last pass's
    /// peak, or in a loop, the most it has held.
    fn phase_need(&self) -> Option<usize> {
        let active = self.phases.active.as_ref().filter(|active| active.invalid.is_none() && active.mode == PhaseMode::Eager)?;
        let sample = active.key.as_ref().and_then(|key| self.phases.samples.get(key)).filter(|sample| sample.known);
        let peak = sample.map(|sample| sample.peak_bytes).max(self.looping().then_some(active.owned_peak))?;
        Some(peak.saturating_sub(active.owned_charged))
    }

    fn phase_known(&self) -> bool {
        self.looping() || self.phase_need().is_some()
    }

    /// Completed free pages beyond what the active phase needs, and the segments a loop keeps
    /// whole. With no measure of it, all of them.
    fn beyond_phase(&self) -> (usize, BTreeSet<u64>) {
        let need = self.phase_need();
        if need.is_none() && !self.looping() {
            return (usize::MAX, BTreeSet::new());
        }
        let kept = self.kept();
        let (mut reusable, mut outside) = (0usize, 0usize);
        for (base, segment) in self.segments.iter().filter(|(_, segment)| segment.pool.is_none()) {
            let bytes = self.releasable(*base, segment);
            reusable += bytes;
            if !kept.contains(base) {
                outside += bytes;
            }
        }
        (reusable.saturating_sub(need.unwrap_or(0)).min(outside), kept)
    }

    /// A completed free block across `at` becomes two, so a handle's pages can leave alone.
    fn carve(&mut self, at: u64) {
        let Some((&ptr, block)) = self.blocks.range(..at).next_back() else { return };
        let (capacity, end) = (block.capacity, ptr + block.capacity as u64);
        if end <= at {
            return;
        }
        let mut tail = block.clone();
        tail.capacity = (end - at) as usize;
        tail.fenced = false;
        self.unlist((None, capacity, None), ptr);
        self.blocks.get_mut(&ptr).expect("carved block").capacity = (at - ptr) as usize;
        let rest = tail.capacity;
        self.blocks.insert(at, tail);
        self.list((None, (at - ptr) as usize, None), ptr);
        self.list((None, rest, None), at);
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
        segment.phase_blocks =
            segment.phase_blocks - usize::from(was_owned) + usize::from(is_owned);
        if is_owned {
            segment.touched = self.plan_epoch;
        }
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
                    segment.touched = self.plan_epoch;
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

fn devices() -> Result<Vec<&'static Device>> {
    crate::check_process()?;
    Ok(crate::DEVICES.iter().filter_map(|slot| slot.get()).filter(|device| device.page() != 0).collect())
}

pub(crate) fn existing(ordinal: i32) -> Result<Option<&'static Device>> {
    let device = crate::gpu(ordinal)?;
    Ok((device.page() != 0).then_some(device))
}

fn owner(ordinal: i32) -> Result<&'static Device> {
    existing(ordinal)?.ok_or(Error::Invalid(
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

pub(crate) fn mark_cooperation_bound(device: i32) -> Result<()> {
    owner(device)?.cooperation_bound.store(true, Ordering::Release);
    Ok(())
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

// The size class of Torch's small RNG tensors.
const AUXILIARY_CLASS: usize = 512;
/// How large a block a whole module may be before its children bind instead. Eviction takes whole
/// units, and a smaller one moves fewer bytes for more calls.
pub const UNIT_BYTES: usize = 512 << 20;

/// A pool's size for a capture from an eager pass's measured peak: a capture can't grow it.
pub fn pool_bytes(peak: usize) -> usize {
    peak.saturating_mul(5) / 4
}

// Above this, blocks are runs that split and merge; below it, slabs of one class.
const LARGE: usize = 1 << 20;
// Smaller runs share segments of this size.
const SEGMENT: usize = 64 << 20;
// What a block can hold beyond itself at each end: a handle's pages come back only whole. Against it,
// mapping 1 GiB of these takes 6 ms and releasing it 6 ms, on an RTX 3060.
const CHUNK: usize = 16 << 20;

impl Device {
    pub(crate) fn page(&self) -> usize {
        self.page.load(Ordering::Acquire)
    }

    /// Weights held for use now, and this request's high-water mark of them.
    pub(super) fn note_held(&self) -> (usize, usize) {
        let held = self.held.load(Ordering::Acquire);
        (held, self.held_peak.fetch_max(held, Ordering::AcqRel).max(held))
    }

    /// Parked workspaces on this stream become ordinary free blocks, once it has passed them.
    fn release_parked(&self, state: &mut State, stream: u64) {
        let parked: Vec<_> = state.parked_workspaces.iter().filter(|(on, _)| *on == stream).copied().collect();
        let mut fences = self.fences();
        for key in parked {
            state.parked_workspaces.remove(&key);
            let block = state.blocks.get_mut(&key.1).expect("parked workspace");
            block.workspace = false;
            block.life = Life::Pending;
            block.reusable_on = None;
            fences.stamp(stream, &mut block.waits);
            if !block.fenced {
                block.fenced = true;
                state.fenced.push(key.1);
            }
        }
    }

    fn free_block(state: &State, class: usize) -> Option<u64> {
        state.free.get(&(None, class, None))?.iter().rev().copied().find(|ptr| {
            state.blocks.get(ptr).is_some_and(|block| block.capacity == class && block.reusable_from(None))
        })
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
                | Error::CaptureMap { .. }
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
        self.fences().complete(&pool.waits)
    }

    /// Settle what completed. The caller holds the gate with no capture active.
    fn poll(&self, state: &mut State) -> Result<()> {
        let mut fences = self.fences();
        fences.poll(self.driver())?;
        cache::poll(state, &fences);
        let mut ready = Vec::new();
        for ptr in std::mem::take(&mut state.fenced) {
            let Some(block) = state.blocks.get_mut(&ptr).filter(|block| block.fenced) else {
                continue;
            };
            let complete = match block.life {
                Life::Ready | Life::Pending => fences.complete(&block.waits),
                Life::Unjoined(mark) => self.joined(mark),
                Life::Live | Life::Captured => {
                    block.fenced = false;
                    continue;
                }
            };
            if complete {
                let charged = (block.charge(), block.phase);
                block.fenced = false;
                block.life = Life::Ready;
                block.reusable_on = None;
                block.waits.clear();
                ready.push((ptr, block.capacity, block.segment, charged));
            } else {
                state.fenced.push(ptr);
            }
        }
        drop(fences);
        for &(ptr, bytes, segment, (charge, phase)) in &ready {
            state.list((state.segments[&segment].pool, bytes, None), ptr);
            if charge != 0 {
                state.phase_changed(charge, 0, phase, phase, segment);
            }
        }
        for (ptr, ..) in ready {
            Self::merge_run(state, ptr);
        }
        Ok(())
    }

    /// A free block, or with no room by the last reading (`full`), a cut or slab page from mapped
    /// free pages: the fast path decides as the slow one would, so a full card's loop never polls.
    fn take(
        state: &mut State,
        bytes: usize,
        requested: usize,
        stream: u64,
        pool: Option<PoolId>,
        full: impl FnOnce() -> bool,
    ) -> Option<u64> {
        if let Some(ptr) = Self::take_class(state, bytes, requested, stream, pool) {
            return Some(ptr);
        }
        if pool.is_some() || checks::on(checks::GUARDS) {
            return None;
        }
        if full() {
            return Self::reuse(state, bytes, requested, stream);
        }
        if bytes <= LARGE {
            return None;
        }
        Self::split_run(state, bytes, stream, false)?;
        Self::take_class(state, bytes, requested, stream, None)
    }

    /// Best fit: cut the smallest free run this stream may use down to `bytes`. The rest stays
    /// free with the same ordering. A block holds only the handles it touches, so with room to spare
    /// the shared sizes keep to their own segments, and a large block's pages stay mapped until
    /// something needs them. Each size holding its own pages cost H3 2.5 times the driver calls.
    fn split_run(state: &mut State, bytes: usize, stream: u64, any: bool) -> Option<()> {
        let most = if any || bytes >= SEGMENT { usize::MAX } else { SEGMENT };
        let mut stale = Vec::new();
        let found = state.runs.range((bytes + 1, 0)..=(most, u64::MAX)).find_map(|&(class, ptr)| {
            match state.blocks.get(&ptr) {
                Some(block) if block.capacity == class && block.life == Life::Ready => {
                    let origin = block.reusable_on;
                    (origin.is_none() || origin == Some(stream)).then_some((ptr, class, origin))
                }
                _ => {
                    stale.push((class, ptr));
                    None
                }
            }
        });
        for entry in stale {
            state.runs.remove(&entry);
        }
        let (ptr, class, origin) = found?;
        state.unlist((None, class, origin), ptr);
        let token = state.phases.active.as_ref().map(|active| active.token);
        let block = state.blocks.get_mut(&ptr).expect("listed free run");
        let mut tail = block.clone();
        block.capacity = bytes;
        tail.capacity = class - bytes;
        let at = ptr + bytes as u64;
        if tail.fenced {
            state.fenced.push(at);
        }
        if tail.charge() != 0 && tail.phase.is_some() && tail.phase == token {
            state.segments.get_mut(&tail.segment).expect("owned segment").phase_blocks += 1;
        }
        let rest = tail.capacity;
        state.blocks.insert(at, tail);
        state.list((None, rest, origin), at);
        state.list((None, bytes, origin), ptr);
        Some(())
    }

    /// With no room for new pages, free pages already mapped serve first (design 4.2): a wholly free
    /// slab page of another class, else a cut from any free run. A cut holds only the handles it
    /// touches, and rejoins its run when freed.
    fn reuse(state: &mut State, class: usize, requested: usize, stream: u64) -> Option<u64> {
        if Self::split_run(state, class, stream, true).is_none() {
            if class > LARGE {
                return None;
            }
            Self::reslab(state, class)?;
        }
        Self::take_class(state, class, requested, stream, None)
    }

    /// A wholly free completed slab page of another class becomes this class's: it is one handle.
    fn reslab(state: &mut State, class: usize) -> Option<()> {
        let (base, bytes) = state.segments.iter()
            .filter(|(_, segment)| segment.pool.is_none() && segment.size_class <= LARGE && segment.size_class != class && segment.backed())
            .find(|(base, segment)| state.blocks.range(**base..**base + segment.bytes as u64)
                .all(|(_, block)| block.life == Life::Ready && block.reusable_on.is_none()))
            .map(|(base, segment)| (*base, segment.bytes))?;
        let old: Vec<_> = state.blocks.range(base..base + bytes as u64).map(|(ptr, block)| (*ptr, block.capacity)).collect();
        for (ptr, capacity) in old {
            state.blocks.remove(&ptr);
            state.unlist((None, capacity, None), ptr);
        }
        state.segments.get_mut(&base).expect("free slab").size_class = class;
        Self::slab(state, base, bytes, class);
        Some(())
    }

    fn slab(state: &mut State, base: u64, bytes: usize, class: usize) {
        for offset in (0..=bytes - class).step_by(class) {
            let address = base + offset as u64;
            state.blocks.insert(address, Block::free(base, class));
            state.list((None, class, None), address);
        }
    }

    /// A free block in a run joins its free neighbours in the segment, for a later, larger request.
    fn merge_run(state: &mut State, ptr: u64) {
        let Some(block) = state.blocks.get(&ptr) else { return };
        let segment = block.segment;
        let held = &state.segments[&segment];
        if held.pool.is_some() || held.size_class <= LARGE {
            return;
        }
        let next = ptr + block.capacity as u64;
        if state.joinable(ptr, next) {
            state.join(ptr, next);
        }
        if let Some((&left, before)) = state.blocks.range(segment..ptr).next_back()
            && left + before.capacity as u64 == ptr
            && state.joinable(left, ptr)
        {
            state.join(left, ptr);
        }
    }

    fn take_class(
        state: &mut State,
        bytes: usize,
        requested: usize,
        stream: u64,
        pool: Option<PoolId>,
    ) -> Option<u64> {
        let phase = state.phases.active.as_ref().map(|active| active.token);
        // Under guards a large block comes only from a segment of its own: one carved before they
        // were on has its neighbour right after it (an RTX 5090's and an H100's setup did).
        let guarded = pool.is_none() && bytes > LARGE && checks::on(checks::GUARDS);
        for origin in [Some(stream), None] {
            let slots = state.free.get_mut(&(pool, bytes, origin));
            if let Some(slots) = slots {
                let mut unguarded = Vec::new();
                while let Some(ptr) = slots.pop() {
                    let Some(block) = state.blocks.get_mut(&ptr) else {
                        continue;
                    };
                    if block.capacity != bytes || !block.reusable_from(origin) {
                        continue;
                    }
                    if guarded && state.segments.get(&block.segment).is_none_or(|s| s.guard == 0 || s.bytes != bytes) {
                        unguarded.push(ptr);
                        continue;
                    }
                    slots.extend(unguarded.drain(..));  // still free, for any other request
                    state.runs.remove(&(bytes, ptr));
                    // Phase tokens never repeat, and entry rebuilds segment accounting.
                    // Outside an observation an old tag needs neither reading nor clearing.
                    let observed = phase.map(|token| {
                        let before = block.charge();
                        let prior = block.phase;
                        block.phase = Some(token);
                        (before, prior)
                    });
                    block.life = Life::Live;
                    block.scratch = false;
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
                slots.extend(unguarded);
            }
        }
        None
    }

    /// A live eager block is tallied where it was born (`escape`); a pool's or workspace's is not.
    fn activate(state: &mut State, ptr: u64, requested: usize, site: u32) -> u64 {
        let block = &state.blocks[&ptr];
        if !block.workspace && state.segments[&block.segment].pool.is_none() {
            let key = state.escapes.born(block.capacity, site);
            state.blocks.get_mut(&ptr).expect("taken block").tally = Some(key);
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
        let guard = checks::guard(self, pool, class);
        let ptr = Self::address(state, bytes + guard)?;
        state.segments.insert(
            ptr,
            Segment { guard, ..Segment::new(bytes, class, pool) },
        );
        if let Err(error) = self.back(state, ptr) {
            // Short of pages midway: what did map is unused. A failed map itself stays owned.
            if error.is_oom() {
                self.unback(state, ptr)?;
            }
            if !state.segments[&ptr].backed() {
                state.segments.remove(&ptr);
                Self::return_address(state, ptr, bytes + guard);
            }
            return Err(error);
        }
        if let Some(pool) = pool {
            state.blocks.insert(ptr, Block::free(ptr, bytes));
            state.list((Some(pool), bytes, None), ptr);
        } else if class > LARGE {
            state.blocks.insert(ptr, Block::free(ptr, bytes));
            state.list((None, bytes, None), ptr);
        } else {
            Self::slab(state, ptr, bytes, class);
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
        let mut best: Option<(u64, usize, u64)> = None;
        let mut consider = |ptr, capacity, segment| {
            if capacity >= bytes && best.is_none_or(|(_, n, _)| capacity < n) {
                best = Some((ptr, capacity, segment));
            }
        };
        for (base, segment) in &state.segments {
            if segment.pool != Some(pool) {
                continue;
            }
            let (mut start, mut length) = (*base, 0usize);
            for (ptr, block) in state.blocks.range(*base..*base + segment.bytes as u64) {
                let ready = block.life == Life::Ready && block.reusable_on.is_none();
                let ordered = block.reusable_on == Some(stream)
                    && matches!(block.life, Life::Ready | Life::Captured);
                if !ready && !ordered {
                    consider(start, length, *base);
                    length = 0;
                    continue;
                }
                if length != 0 && *ptr != start + length as u64 {
                    consider(start, length, *base);
                    length = 0;
                }
                if length == 0 {
                    start = *ptr;
                }
                length += block.capacity;
            }
            consider(start, length, *base);
        }
        let (ptr, _, segment) = best?;
        let phase = state.phases.active.as_ref().map(|active| active.token);
        let end = ptr + bytes as u64;
        let parts: Vec<_> = state.blocks.range(ptr..end).map(|(p, _)| *p).collect();
        let held = state.pools.get_mut(&pool).expect("owned pool");
        let mut tail = None;
        for part in parts {
            let mut block = state.blocks.remove(&part).expect("selected free pool block");
            let stop = part + block.capacity as u64;
            if stop > end {
                // What this allocation does not use keeps its own state: untouched pool stays
                // open to every stream, whatever ordering the bytes before it needed.
                block.capacity = (stop - end) as usize;
                tail = Some((block, stop));
                continue;
            }
            // Merged blocks hand their stamps to the pool, which unmaps only after them all.
            for (id, sequence) in block.waits {
                match held.waits.iter_mut().find(|(known, _)| *known == id) {
                    Some(wait) => wait.1 = wait.1.max(sequence),
                    None => held.waits.push((id, sequence)),
                }
            }
        }
        let last = tail.as_ref().map_or(end, |(_, stop)| *stop);
        state.free.retain(|(id, _, _), slots| {
            if *id == Some(pool) {
                slots.retain(|p| *p < ptr || *p >= last);
            }
            !slots.is_empty()
        });
        if let Some((mut block, _)) = tail {
            state.free.entry((Some(pool), block.capacity, block.reusable_on)).or_default().push(end);
            block.fenced = !block.waits.is_empty();
            if block.fenced {
                state.fenced.push(end);
            }
            state.blocks.insert(end, block);
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

    // Physical pages for a segment's kept VA.
    fn back(&self, state: &mut State, base: u64) -> crate::AllocationResult<()> {
        let segment = state.segments.get_mut(&base).expect("owned segment");
        let (pool, class) = (segment.pool, segment.size_class);
        self.sample()?;
        let missing = segment.unbacked();
        if self.over(missing) != 0 {
            return Err(Error::OutOfMemory { requested_bytes: missing }.into());
        }
        // All or nothing here: an eager segment never maps again, and a pool has one handle.
        let mut mapped = Vec::new();
        let result = (|| {
            for chunk in segment.chunks.iter_mut().filter(|chunk| chunk.handle.is_none()) {
                let handle = self.driver().create(chunk.bytes)?;
                // Track ownership before map: a partial mapping failure must retain the VA and handle.
                chunk.handle = Some(handle);
                self.working.fetch_add(chunk.bytes, Ordering::AcqRel);
                state.stats.mapped_bytes += chunk.bytes;
                state.stats.mapped_peak = state.stats.mapped_peak.max(state.stats.mapped_bytes);
                let at = base + chunk.offset as u64;
                self.driver().map_only(at, chunk.bytes, handle)?;
                mapped.push((at, chunk.bytes));
            }
            Ok(())
        })();
        // After the grant: an observer may hand these to NCCL, which looks each address up. Short
        // of pages midway, what did map is reported all the same, since its unmapping will be.
        match &result {
            Ok(()) => self.driver().grant(base, segment.bytes)?,
            Err(_) => for &(at, bytes) in &mapped {
                self.driver().grant(at, bytes)?;
            },
        }
        for (at, bytes) in mapped {
            observer::mapped(self, at, bytes, pool, class)?;
        }
        result
    }

    fn unback(&self, state: &mut State, base: u64) -> Result<usize> {
        let mut released = 0;
        for index in 0..state.segments[&base].chunks.len() {
            released += self.unback_chunk(state, base, index)?;
        }
        Ok(released)
    }

    fn unback_chunk(&self, state: &mut State, base: u64, index: usize) -> Result<usize> {
        let segment = state.segments.get_mut(&base).expect("owned segment");
        let chunk = segment.chunks[index];
        let Some(handle) = chunk.handle else { return Ok(0) };
        let at = base + chunk.offset as u64;
        observer::retiring(self, at, chunk.bytes, segment.pool, segment.size_class)?;
        self.driver().unmap(at, chunk.bytes)?;
        self.driver().release(handle)?;
        segment.chunks[index].handle = None;
        self.working.fetch_sub(chunk.bytes, Ordering::AcqRel);
        self.released.fetch_add(chunk.bytes as u64, Ordering::AcqRel);
        state.stats.mapped_bytes -= chunk.bytes;
        Ok(chunk.bytes)
    }

    /// This large segment's handles with only completed free blocks on them go back, last first
    /// so what stays is one run, and no more of them than asked: free pages stay until needed.
    fn trim_chunks(&self, state: &mut State, base: u64, limit: usize, maximum: usize) -> Result<usize> {
        let mut released = 0;
        for index in (0..state.segments[&base].chunks.len()).rev() {
            let chunk = state.segments[&base].chunks[index];
            if released >= limit {
                break;
            }
            if chunk.handle.is_none() || chunk.bytes > maximum.saturating_sub(released) || !state.chunk_free(base, &chunk) {
                continue;
            }
            let (start, end) = (base + chunk.offset as u64, base + (chunk.offset + chunk.bytes) as u64);
            state.carve(start);
            state.carve(end);
            let pieces: Vec<_> = state.blocks.range(start..end).map(|(ptr, _)| *ptr).collect();
            for ptr in pieces {
                let piece = state.blocks.remove(&ptr).expect("carved piece");
                state.unlist((None, piece.capacity, None), ptr);
            }
            released += self.unback_chunk(state, base, index)?;
        }
        if !state.segments[&base].backed() {
            let segment = state.segments.remove(&base).expect("owned segment");
            Self::return_address(state, base, segment.bytes + segment.guard);
        }
        Ok(released)
    }

    // Unmap completed free segments until `limit` bytes are gone, keeping `preserve`, the
    // `exclude` pool, and anything over `maximum`. Only actual releases count.
    fn trim_ready(
        &self, state: &mut State, limit: usize, exclude: Option<PoolId>,
        preserve: &BTreeSet<u64>, maximum: usize,
    ) -> Result<usize> {
        let mut released = 0usize;
        let mut candidates = Vec::new();
        let mut large = Vec::new();
        for (base, segment) in &state.segments {
            if preserve.contains(base) {
                continue;
            }
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            // Phase tokens only rise: the pages longest out of use go first, not the lowest addresses.
            let newest = state.blocks.range(*base..(*base + segment.bytes as u64))
                .filter_map(|(_, block)| block.phase).max();
            if segment.chunks.len() > 1 {
                large.push((newest, *base));
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
                candidates.push((newest, *base));
            }
        }
        candidates.sort_unstable();
        large.sort_unstable();
        for (_, base) in candidates {
            if released >= limit {
                break;
            }
            let segment = &state.segments[&base];
            let bytes = segment.bytes;
            if segment.backed() && bytes > maximum.saturating_sub(released) { continue; }
            released += self.unback(state, base)?;
            let blocks: Vec<_> = state
                .blocks
                .range(base..base + bytes as u64)
                .map(|(p, _)| *p)
                .collect();
            for ptr in blocks {
                state.blocks.remove(&ptr);
            }
            let guard = state.segments.remove(&base).expect("owned segment").guard;
            Self::return_address(state, base, bytes + guard);
        }
        for (_, base) in large {
            if released >= limit {
                break;
            }
            released += self.trim_chunks(state, base, limit - released, maximum.saturating_sub(released))?;
        }
        state.free.retain(|_, slots| {
            slots.retain(|ptr| state.blocks.contains_key(ptr));
            !slots.is_empty()
        });
        let blocks = &state.blocks;
        state.runs.retain(|(_, ptr)| blocks.contains_key(ptr));
        let segments = &state.segments;
        let fences = self.fences();
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

    // Unmap idle pools, least recently replayed first, whole pool at a time. Their VA, blocks
    // and graph stay; a replay maps them again. Contents were declared dead, so nothing is read.
    fn unmap_idle_pools(&self, state: &mut State, limit: usize, keep: Option<PoolId>, maximum: usize) -> Result<usize> {
        let mut pools: Vec<_> = state.pools.iter()
            .filter(|(id, pool)| Some(**id) != keep && self.pool_reclaimable(state, **id, pool))
            .map(|(id, pool)| (pool.last_use, *id))
            .collect();
        pools.sort_unstable();
        let mut released = 0usize;
        for (_, id) in pools {
            if released >= limit {
                break;
            }
            let bases: Vec<_> = state.segments.iter().filter(|(_, segment)| segment.pool == Some(id))
                .map(|(base, segment)| (*base, segment.bytes)).collect();
            if bases.iter().map(|(_, bytes)| bytes).sum::<usize>() > maximum.saturating_sub(released) {
                continue;
            }
            for (base, _) in bases {
                released += self.unback(state, base)?;
            }
            state.pools.get_mut(&id).expect("selected pool").unmapped = true;
        }
        Ok(released)
    }

    // Between uses, with every live allocation declared dead. Its fences are checked apart.
    fn pool_idle(state: &State, id: PoolId, pool: &Pool) -> bool {
        pool.scratch && !pool.in_use && !pool.unmapped && !pool.prepared && pool.capture.is_none()
            && state.segments.iter().filter(|(_, segment)| segment.pool == Some(id))
                .all(|(base, segment)| state.blocks.range(*base..*base + segment.bytes as u64)
                    .all(|(_, block)| block.life != Life::Live || block.scratch))
    }

    fn pool_reclaimable(&self, state: &State, id: PoolId, pool: &Pool) -> bool {
        if !Self::pool_idle(state, id, pool) {
            return false;
        }
        let fences = self.fences();
        fences.complete(&pool.waits)
            && state.segments.iter().filter(|(_, segment)| segment.pool == Some(id))
                .all(|(base, segment)| state.blocks.range(*base..*base + segment.bytes as u64)
                    .all(|(_, block)| fences.complete(&block.waits)))
    }

    // Events whose completion would free `limit` bytes of whole segments and released pages.
    // Each is already submitted: no live allocation, graph reference or owner action is awaited.
    fn pending_events(&self, state: &State, limit: usize, exclude: Option<PoolId>) -> Vec<usize> {
        let fences = self.fences();
        let (mut events, mut bytes) = (Vec::new(), 0usize);
        for (base, segment) in &state.segments {
            if bytes >= limit {
                break;
            }
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            let pool = segment.pool.map(|id| &state.pools[&id]);
            let idle = segment.pool.is_some_and(|id| Self::pool_idle(state, id, &state.pools[&id]));
            if !idle && pool.is_some_and(|pool| pool.prepared || pool.references != 0 || pool.capture.is_some()) {
                continue;
            }
            let mut blocks = state.blocks.range(*base..*base + segment.bytes as u64);
            if !idle && segment.chunks.len() == 1 && !blocks.clone().all(|(_, block)| matches!(block.life, Life::Ready | Life::Pending)) {
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
        cache::pending_events(state, &fences, self.page(), limit.saturating_sub(bytes), &mut events);
        events
    }
}

/// Open native device state before any graph capture. Repeated initialization is harmless.
pub fn initialize(device: i32) -> Result<()> {
    initialize_owner(device).map(|_| ())
}

/// One GPU's driver and working address space, once.
pub(crate) fn open(ordinal: i32) -> Result<&'static Device> {
    initialize_owner(ordinal)
}

fn initialize_owner(ordinal: i32) -> Result<&'static Device> {
    let device = crate::gpu(ordinal)?;
    if device.page() != 0 {
        return Ok(device);
    }
    crate::gate(ordinal)?.not_capturing()?;
    let _initializing = INITIALIZE.lock().map_err(|_| Error::Poisoned("device initialization".into()))?;
    if device.page() != 0 {
        return Ok(device);
    }
    if device.driver.get().is_none() {
        let opened = Arc::new(crate::cuda::Driver::open(ordinal)?);
        let _ = device.driver.set(opened);
    }
    let driver = device.driver();
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
    device.lock()?.addresses.insert(base, bytes);
    device.page.store(page, Ordering::Release);
    Ok(device)
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
    // Another stream's allocation during a capture that would need the driver: no room for it now.
    let result = alloc_inner(device, bytes, stream, pool, bound).map_err(|error| match error {
        Error::CaptureActive => Error::CaptureMap { requested_bytes: bytes },
        error => error,
    });
    if result.is_err() {
        let _ = invalidate_current_phase(device, "allocation_error");
    }
    if pool.is_none() && checks::on(checks::PERTURB) && let Ok(ptr @ 1..) = result.as_ref().copied() {
        checks::perturb(device, ptr, stream, false)?;
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
    let class = size_class(bytes, owner.page())?;
    let exact = if class <= LARGE { owner.page() } else { class };
    let mut pool = pool;
    let (workspace, looping);
    {
        let mut state = owner.lock()?;
        looping = state.looping();
        owner.callback_mutation(&mut state);
        // A capture's primed stream holds its workspaces already: nothing it allocates is one.
        let primed = pool.is_some_and(|id| state.pools.get(&id).is_some_and(|p| p.routing && p.primed.contains(&stream)));
        workspace = !primed && state.workspace_sizes.contains(&bytes);
        if workspace {
            // Bound into every graph on its stream, and freed by any one graph's reset: it never
            // lives in a pool, and stays reserved while a graph on its stream may name it. An
            // ordinary allocation of exactly this size shares that treatment, costing only a block
            // outside capture; under one it needs a free block there (`Error::CaptureMap`).
            pool = None;
        }
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
        if let Some(ptr) = Device::take(&mut state, class, bytes, stream, pool, || owner.over(exact) != 0) {
            state.blocks.get_mut(&ptr).expect("taken block").workspace = workspace;
            return Ok(Device::activate(&mut state, ptr, bytes, escape::current(device)));
        }
        if let Some(id) = pool {
            if let Some(ptr) = Device::fit_pool(&mut state, id, class, bytes, stream) {
                return Ok(Device::activate(&mut state, ptr, bytes, escape::UNNAMED));
            }
            state.stats.ooms += 1;
            let session = state.pools.get_mut(&id).expect("routed pool");
            session.short = session.short.max(bytes);
            return Err(Error::OutOfMemory {
                requested_bytes: bytes,
            });
        }
    }
    let segment = rounded(class.max(SEGMENT), owner.page())?;
    // A loop's working allocation, like its weights, takes weights before its other pages: what one
    // step freed the next uses again.
    let protect = Protect { reuse_working: true, keep_phase: looping, ..Default::default() };
    let result = reclaim::acquire(device, exact, Wait::Yes, protect, || {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        owner.callback_mutation(&mut state);
        let _context = owner.driver().enter()?;
        let result = (|| -> crate::AllocationResult<u64> {
            owner.poll(&mut state)?;
            owner.note_held();
            // The room as last read: a map reads the driver again, and so does reclaim.
            let ptr = match Device::take(&mut state, class, bytes, stream, None, || owner.over(exact) != 0) {
                Some(ptr) => ptr,
                None => {
                    // Room to spare takes a whole segment, so blocks share pages. No weight goes for it.
                    let whole = class > LARGE && owner.headroom() >= segment && !checks::on(checks::GUARDS);
                    let mapped = if whole { segment } else { exact };
                    owner.map(&mut state, mapped, None, class)?;
                    Device::take(&mut state, class, bytes, stream, None, || false).expect("new slab has a block")
                }
            };
            state.blocks.get_mut(&ptr).expect("taken block").workspace = workspace;
            Ok(Device::activate(&mut state, ptr, bytes, escape::current(device)))
        })();
        result.map_err(|failure| owner.fail_allocation(&mut state, failure))
    });
    if matches!(result, Err(Error::OutOfMemory { .. } | Error::Exhausted { .. })) {
        let mut state = owner.lock()?;
        state.stats.ooms += 1;
        state.escapes.refused(owner.owned().saturating_add(bytes), escape::room_held(owner));
        split::refused(&mut state, bytes, stream);
    }
    result
}

pub fn free(device: i32, ptr: u64, stream: u64) -> Result<()> {
    if ptr != 0 && checks::on(checks::PERTURB) {
        checks::perturb(device, ptr, stream, true)?;
    }
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
    let capturing = owner.captures.load(Ordering::Acquire) != 0;
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
    if block.workspace && !block.origin_retired
        && pools.values().any(|pool| pool.names_stream(block.stream))
    {
        let (origin, requested) = (block.stream, block.requested);
        block.requested = 0;
        state.stats.allocated_bytes -= requested;
        state.stats.frees += 1;
        state.parked_workspaces.insert((origin, ptr));
        return Ok(());
    }
    block.workspace = false;
    let (pool, run) = (segments[&block.segment].pool, segments[&block.segment].size_class > LARGE);
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
        let mut fences = owner.fences();
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
    if let Some(key) = block.tally.take() {
        state.escapes.died(key, class);
    }
    if reusable {
        state.list((pool, class, reusable_on), ptr);
    }
    if let Some((charge, phase, segment)) = observed {
        state.phase_changed(class, charge, phase, phase, segment);
        split::refresh(&mut state);
    }
    if reusable && run {
        Device::merge_run(&mut state, ptr);
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
    let tally = block.tally.take();
    block.requested = 0;
    block.life = if completed {
        Life::Ready
    } else {
        Life::Unjoined(owner.unjoined_mark())
    };
    block.reusable_on = None;
    block.streams.clear();
    block.waits.clear();
    if !completed && !block.fenced {
        block.fenced = true;
        state.fenced.push(ptr);
    }
    if let Some(key) = tally {
        state.escapes.died(key, capacity);
    }
    if completed {
        state.list((None, capacity, None), ptr);
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
    if completed {
        Device::merge_run(&mut state, ptr);
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
/// Seals what a public client just stamped on its own stream (`Fences::seal`), on its thread: no
/// later poll records on a stream it may destroy next (public-priority's take, SIGSEGV in the donor's
/// cuEventRecord after its owner destroyed the stream it freed on, 1 exit in 5 on a 5090).
pub fn seal_stream(device: i32, stream: u64) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let _state = owner.lock()?;
    let _context = owner.driver().enter()?;
    owner.fences().seal(owner.driver(), stream)
}

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

fn retire_stream_inner(owner: &Device, device: i32, stream: u64) -> Result<()> {
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    if owner.driver().is_capturing(stream as usize)? {
        return Err(Error::CaptureActive);
    }
    if !owner.driver().query_stream(stream as usize)? {
        return Err(Error::Busy);
    }
    owner.release_parked(&mut state, stream);
    let mut fences = owner.fences();
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
        state.list((pool, bytes, stream), ptr);
    }
    state.refresh_phase();
    Ok(())
}

/// `torch.cuda.empty_cache`: the caller's request to the core, for completed free pages only.
pub fn trim_all() -> Result<usize> {
    devices()?.into_iter().try_fold(0usize, |total, owner| {
        if crate::is_capturing(owner.ordinal)? {
            return Ok(total);
        }
        Ok(total.saturating_add(release_completed(owner.ordinal, usize::MAX, usize::MAX, None)?))
    })
}

/// The first step of the reclaim order: completed free pages, with no waiting.
pub(crate) fn release_completed(device: i32, want: usize, maximum: usize, keep: Option<PoolId>) -> Result<usize> {
    release_free(device, want, maximum, keep, false)
}

/// Completed free pages the active phase can spare (`State::beyond_phase`).
pub(crate) fn release_beyond_phase(device: i32, want: usize, maximum: usize, keep: Option<PoolId>) -> Result<usize> {
    release_free(device, want, maximum, keep, true)
}

fn release_free(device: i32, want: usize, maximum: usize, keep: Option<PoolId>, phase: bool) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    let result = owner.poll(&mut state).and_then(|()| {
        let (beyond, kept) = if phase { state.beyond_phase() } else { (usize::MAX, BTreeSet::new()) };
        owner.trim_ready(&mut state, want.min(beyond), keep, &kept, maximum)
    });
    result.map_err(|error| owner.fail(&mut state, error))
}

/// Idle pools' pages: nothing to read back and no wait, the cheapest memory after free pages.
pub(crate) fn release_idle_pools(device: i32, want: usize, maximum: usize, keep: Option<PoolId>) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    let result = owner.poll(&mut state)
        .and_then(|()| owner.unmap_idle_pools(&mut state, want, keep, maximum));
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
        let _context = owner.driver().enter()?;
        let polled = owner.poll(&mut state);
        polled.map_err(|error| owner.fail(&mut state, error))?;
        let events = owner.pending_events(&state, want, keep);
        if events.is_empty() {
            return Ok(false);
        }
        (events, gate.hold()?)
    };
    reclaim::wait_outside(owner, &events, false)?;
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
        matches!(block.life, Life::Unjoined(mark) if !owner.joined(mark))
    }))
}

/// Begin one device-wide execution observation. None means the final configuration is not
/// known yet: observe demand, but do not admit speculation from an earlier guessed key.
pub fn phase_enter(device: i32, key: Option<String>, mode: PhaseMode) -> Result<u64> {
    if key.as_ref().is_some_and(String::is_empty) {
        return Err(Error::Invalid("phase execution key must not be empty"));
    }
    let owner = initialize_owner(device)?;
    let mut state = owner.lock()?;
    if state.phases.active.is_some() {
        return Err(Error::PhaseOverlap);
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
    let invalid = (mode == PhaseMode::Eager && owner.graph_active())
        .then(|| "capture_or_replay_active".to_owned());
    state.phases.active = Some(ActivePhase {
        token,
        key,
        mode,
        activity_epoch: owner.epoch(),
        charged,
        peak: charged,
        owned_charged: 0,
        owned_peak: 0,
        backing: 0,
        backing_peak: 0,
        invalid,
    });
    state.refresh_phase();
    Ok(token)
}

fn invalidate_eager_phase(device: i32, reason: &str) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let eager = owner.lock()?.phases.active.as_ref().is_some_and(|active| active.mode == PhaseMode::Eager);
    if eager {
        invalidate_current_phase(device, reason)?;
    }
    Ok(())
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
    if active.mode == PhaseMode::Eager && active.activity_epoch != owner.epoch() {
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

/// What the rest of this phase still needs of working memory (`State::phase_need`), the completed
/// free pages already mapped for it, those it can spare, and whether it knew. With no measure it says
/// a quarter of the card, the same floor a split has. None declines: capture, or pressure.
pub(crate) fn prefetch_gap(device: i32) -> Result<Option<(usize, usize, usize, bool)>> {
    let Some(owner) = existing(device)? else {
        return Ok(None);
    };
    let mut state = owner.lock()?;
    state.stats.prefetch_attempts += 1;
    let answer = (|| {
        if state.prefetch_suppressed {
            return None;
        }
        let reusable = state.reusable_eager_pages();
        let unknown = Some((owner.capacity() / 4, reusable, 0, false));
        let Some(active) = state.phases.active.as_ref().filter(|active| active.invalid.is_none()) else {
            return unknown;
        };
        match active.mode {
            PhaseMode::Eager => {
                let Some(need) = state.phase_need() else { return unknown };
                Some((need, reusable, state.beyond_phase().0, true))
            }
            PhaseMode::Replay => {
                let sample = active.key.as_ref().and_then(|key| state.phases.replay_samples.get(key));
                let Some(sample) = sample.filter(|sample| sample.known) else { return unknown };
                let need = sample.peak_bytes.saturating_sub(active.owned_charged);
                Some((need, reusable, reusable.saturating_sub(need), true))
            }
            _ => None,
        }
    })();
    if answer.is_none() {
        state.stats.prefetch_declines += 1;
    }
    Ok(answer)
}

/// Whether the active phase's working need is known (`State::phase_need`).
pub(crate) fn phase_known(device: i32) -> Result<bool> {
    let Some(owner) = existing(device)? else {
        return Ok(false);
    };
    Ok(owner.lock()?.phase_known())
}

pub(crate) fn prefetch_declined(device: i32) -> Result<()> {
    if let Some(owner) = existing(device)? {
        owner.lock()?.stats.prefetch_declines += 1;
    }
    Ok(())
}

pub(crate) fn prefetch_admitted(device: i32) -> Result<()> {
    if let Some(owner) = existing(device)? {
        owner.lock()?.stats.prefetch_admissions += 1;
    }
    Ok(())
}

/// Why this process's native state can't be trusted, if it can't. Opens nothing.
pub fn poisoned() -> Result<Option<String>> {
    for device in devices()? {
        let state = device.state.lock().map_err(|_| Error::Poisoned("device table mutex".into()))?;
        if let Some(why) = &state.poison {
            return Ok(Some(why.clone()));
        }
    }
    Ok(None)
}

pub fn stats(device: i32) -> Result<WorkingStats> {
    let Some(owner) = existing(device)? else {
        return Ok(WorkingStats::default());
    };
    let state = owner.lock()?;
    let mut stats = state.stats;
    stats.room_moves = owner.room_epoch();
    stats.room_outside_moves = owner.room_outside.load(Ordering::Relaxed);
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
    let unmapped: usize = state.segments.values().map(Segment::unbacked).sum();
    stats.unassigned_bytes = (stats.mapped_bytes + unmapped)
        .saturating_sub(stats.allocated_capacity + stats.pending_bytes + stats.free_bytes);
    stats.pools_bytes = state
        .segments
        .values()
        .filter(|s| s.pool.is_some() && s.backed())
        .map(|s| s.bytes)
        .sum();
    stats.pools = state.pools.len();
    cache::working_stats(&state, owner.page(), &mut stats);
    stats.event_handles = owner.fences().handles();
    stats.context_joins = owner.driver().context_joins();
    (
        stats.driver_memory_calls,
        stats.event_queries,
        stats.event_records,
        stats.event_waits,
    ) = owner.driver().calls();
    stats.driver_physical_ooms = owner.driver().physical_ooms();
    (
        stats.driver_memory_ns,
        stats.driver_memory_longest_ns,
        stats.reclaim_ns,
        stats.reclaim_longest_ns,
        stats.reclaim_longest_step,
    ) = owner.driver().times();
    stats.reclaim_step_ns = owner.driver().reclaim_steps();
    (stats.observed_mapped, stats.observed_retired, stats.observed_ns) = observer::observed();
    Ok(stats)
}

/// Allocations this device refused since it opened (`WorkingStats::ooms`), read alone: the package
/// contract compares it at each component scope's entry and exit for a refusal a package returned past.
pub fn ooms(device: i32) -> Result<u64> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    Ok(owner.lock()?.stats.ooms)
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
        pool.in_use = false;
    }
    Ok(())
}

/// The owner reset every graph it captured here and allocates here no more: its references and
/// preparation go, so its pages and its entry go at the next trim, once its blocks are free and
/// complete. A capture whose end failed is never reset by Torch, and its reference would hold
/// the pool mapped for the process's life. Refused while a capture on it is open.
pub fn pool_close(device: i32, id: PoolId) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let mut state = owner.lock()?;
    let Some(pool) = state.pools.get_mut(&id) else {
        return Ok(());
    };
    if pool.capture.is_some() || pool.routing {
        return Err(Error::Invalid("close a graph pool after its capture ends"));
    }
    pool.references = 0;
    pool.prepared = false;
    pool.armed = false;
    pool.in_use = false;
    let streams: Vec<u64> = pool.streams.iter().copied().collect();
    for stream in streams {
        if !state.pools.values().any(|pool| pool.names_stream(stream)) {
            owner.release_parked(&mut state, stream);
        }
    }
    Ok(())
}

pub fn pool_reserve(device: i32, pool: PoolId, bytes: usize) -> Result<usize> {
    if bytes != 0 {  // Zero is a use of a recorded pool: an LM's token, inside its eager phase.
        invalidate_eager_phase(device, "capture_preparation")?;
    }
    let owner = initialize_owner(device)?;
    let wanted = rounded(bytes, owner.page())?;
    let keep = Protect { pool: Some(pool), ..Default::default() };
    let scramble = bytes == 0 && checks::before_use(owner, device, pool)?;
    {
        let mut state = owner.lock()?;
        let session = state.pools.entry(pool).or_default();
        session.in_use = true;
        session.last_use = owner.next_use();
        if session.references == 0 {
            session.prepared = true;
        }
    }
    let (unbacked, total) = owner.lock()?.segments.values().filter(|s| s.pool == Some(pool))
        .fold((0, 0), |(unbacked, total), s| (unbacked + s.unbacked(), total + s.bytes));
    let need = wanted + unbacked;
    // A recording in no request, an idle period's, is a guess: it takes only room already free,
    // its own completed pages included, and asks nobody. Refused, the call stays eager.
    #[cfg(target_os = "linux")]
    let guess = crate::cooperative::in_request(device) == Some(false);
    #[cfg(not(target_os = "linux"))]
    let guess = false;
    // Its new pages take completed free working pages' place first, whatever the card has free: a
    // recorded program is what runs from now on, and mapped beside them its pool summed with
    // them (SDXL uncapped on a 4090: a 2.7 GiB pool beside 3.7 GiB of eager pages, 11.6 GiB). At
    // idle no call is running that would reuse any of them, so they all go.
    let fresh = unbacked + wanted.saturating_sub(total);
    if bytes != 0 && fresh != 0 {
        release_completed(device, if guess { usize::MAX } else { fresh }, usize::MAX, Some(pool))?;
    }
    let reserve = || {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        let _context = owner.driver().enter()?;
        let result = (|| -> crate::AllocationResult<usize> {
            let have = pool_back(owner, &mut state, pool)?;
            if have < wanted {
                owner.map(&mut state, wanted - have, Some(pool), 0)?;
            }
            Ok(have.max(wanted))
        })();
        result.map_err(|failure| owner.fail_allocation(&mut state, failure))
    };
    let mapped = if guess {
        match reserve() {
            Ok(mapped) => mapped,
            Err(failure) if failure.is_oom() => {
                release_completed(device, need, usize::MAX, Some(pool))?;
                reserve()?
            }
            Err(failure) => return Err(failure.into()),
        }
    } else {
        reclaim::acquire(device, need, Wait::Yes, keep, reserve)?
    };
    if scramble {
        checks::scramble(owner, pool)?;
    }
    Ok(mapped)
}

/// The next capture on this pool is this module's own. Torch's lazily created RNG tensors,
/// on another stream, need a free small block: a capture never maps. `primed`: the streams
/// whose library workspaces its caller made for this capture.
pub fn pool_arm(device: i32, id: PoolId, primed: &[u64]) -> Result<()> {
    let owner = owner(device)?;
    reclaim::acquire(device, owner.page(), Wait::Yes, Protect { pool: Some(id), ..Default::default() }, || {
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        let _context = owner.driver().enter()?;
        let session = state.pools.entry(id).or_default();
        session.armed = true;
        session.primed = primed.iter().copied().collect();
        session.short = 0;
        if Device::free_block(&state, AUXILIARY_CLASS).is_none() {
            let mapped = owner.map(&mut state, owner.page(), None, AUXILIARY_CLASS);
            mapped.map_err(|failure| owner.fail_allocation(&mut state, failure))?;
        }
        Ok(())
    })
}

/// The largest allocation the pool's last capture had no room for, 0 when none: what its next
/// reservation adds.
pub fn pool_short(device: i32, id: PoolId) -> Result<usize> {
    Ok(owner(device)?.lock()?.pools.get(&id).map_or(0, |pool| pool.short))
}

// Pages for every segment of this pool that has none. Returns the pool's total bytes.
fn pool_back(owner: &Device, state: &mut State, id: PoolId) -> crate::AllocationResult<usize> {
    let bases: Vec<_> = state.segments.iter().filter(|(_, segment)| segment.pool == Some(id))
        .map(|(base, segment)| (*base, segment.bytes, !segment.backed())).collect();
    for (base, _, unmapped) in &bases {
        if *unmapped {
            owner.back(state, *base)?;
        }
    }
    state.pools.get_mut(&id).expect("reserved pool").unmapped = false;
    Ok(bases.iter().map(|(_, bytes, _)| bytes).sum())
}

/// The owner declares the allocations it holds here dead between uses of this pool: nothing
/// reads them outside `pool_reserve` .. `pool_record_stream(end)`. The core may then release
/// the pool's pages when idle, keeping its VA, blocks and graph. Any other live allocation
/// here could be an escaped alias a kernel would still touch: that refuses.
pub fn pool_scratch(device: i32, id: PoolId, held: &[u64]) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let bases: Vec<_> = state.segments.iter().filter(|(_, segment)| segment.pool == Some(id))
        .map(|(base, segment)| (*base, segment.bytes)).collect();
    for ptr in held {
        // A held tensor outside this pool does not depend on its pages.
        if bases.iter().any(|(base, bytes)| *ptr >= *base && *ptr < *base + *bytes as u64) {
            let block = state.blocks.get_mut(ptr).filter(|block| block.life == Life::Live)
                .ok_or(Error::Invalid("a declared allocation is not live in this graph pool"))?;
            block.scratch = true;
        }
    }
    for (base, bytes) in bases {
        if state.blocks.range(base..base + bytes as u64).any(|(_, block)| block.life == Life::Live && !block.scratch) {
            return Err(Error::Invalid("a live allocation outside its owner's declaration holds this graph pool"));
        }
    }
    let pool = state.pools.get_mut(&id).ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 || pool.capture.is_some() {
        return Err(Error::Invalid("only a captured graph pool's owner can declare it scratch"));
    }
    pool.scratch = true;
    Ok(())
}

/// Torch's cached library workspace sizes on this device. Never initializes CUDA.
pub fn library_workspaces(device: i32, sizes: Vec<usize>) -> Result<()> {
    owner(device)?.lock()?.workspace_sizes = sizes;
    Ok(())
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
    if pool.unmapped {
        return Err(Error::Invalid("map this graph pool before capture"));
    }
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
        let _context = owner.driver().enter()?;
        for stream in &pool.streams {
            if owner.driver().is_capturing(*stream as usize)? {
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
    let mut fences = owner.fences();
    for stream in &pool.streams {
        fences.stamp(*stream, &mut pool.waits);
    }
    drop(fences);
    pool.in_use = false;
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
    if owner.captures.load(Ordering::Acquire) != 0 {
        return Err(Error::CaptureActive);
    }
    let mut state = owner.lock()?;
    if state.phases.active.as_ref().is_some_and(|active| active.mode == PhaseMode::Eager) {
        state.phases.invalidate("graph_replay");
    }
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 {
        return Err(Error::Invalid("graph pool has no owner"));
    }
    if pool.unmapped {
        return Err(Error::Invalid("map this graph pool before replay"));
    }
    pool.last_use = owner.next_use();
    pool.references = pool
        .references
        .checked_add(1)
        .ok_or(Error::Invalid("graph reference overflow"))?;
    Ok(())
}

/// A replay or consumer was submitted on this stream. `end`: the owner's last for now.
/// Metadata only: no driver call.
pub fn pool_record_stream(device: i32, id: PoolId, stream: u64, end: bool) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    if owner.captures.load(Ordering::Acquire) != 0 {
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
    owner.fences().stamp(stream, &mut pool.waits);
    if end {
        pool.in_use = false;
    }
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
    // The last graph on a stream is gone: nothing names its parked workspaces now.
    let streams: Vec<u64> = pool.streams.iter().copied().collect();
    for stream in streams {
        if !state.pools.values().any(|pool| pool.names_stream(stream)) {
            owner.release_parked(&mut state, stream);
        }
    }
    Ok(())
}
