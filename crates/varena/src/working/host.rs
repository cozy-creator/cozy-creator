//! The last resort (var-030 W8): an eager working allocation in a request that the whole reclaim
//! order refused, where its caller has no cheaper way out (`set_fallback`), goes to pinned host memory
//! the card reads and writes directly over the link. Nothing faults or migrates. It takes no room on
//! the card, so it counts in nothing that judges the card: no phase, split sample, pass tally, live
//! sum or `allocated_bytes`. Its free blocks serve only this step again; its segments go back when no
//! request runs, or for a new one when host memory is short.
use super::{Block, Chunk, Device, LARGE, Life, Segment, State};
use crate::{Error, HostShort, Result};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

/// A host segment's one chunk "handle": its pages are the pin's.
const PINNED: u64 = u64::MAX;

thread_local! {
    static FALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread's allocations have a cheaper way out than pinned host memory (a finer split
/// level, a split's whole on host, a library's own smaller choice, a graph that goes): a refusal
/// comes back to the caller, and counts as answered. The previous setting.
pub fn set_fallback(on: bool) -> bool {
    FALLBACK.with(|fallback| fallback.replace(on))
}

pub fn fallback() -> bool {
    FALLBACK.with(Cell::get)
}

/// Said once: this process can't pin host memory the card addresses.
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

impl Device {
    /// An allocation of `class` (`bytes` asked) on `stream`: a free host block, else a new host
    /// segment of its exact size, once the machine's host admission takes it. Err(Some): host memory
    /// is short too; Err(None): no pin here, and the refusal stands.
    pub(super) fn last_resort(
        &self, device: i32, class: usize, bytes: usize, stream: u64,
    ) -> Result<std::result::Result<u64, Option<HostShort>>> {
        let mapped = if class > LARGE { class } else { self.page() };
        {
            let gate = crate::gate(device)?;
            gate.not_capturing()?;
            let mut state = self.lock()?;
            let _context = self.driver().enter()?;
            self.poll(&mut state)?;
            if let Some(ptr) = take(&mut state, class, bytes, stream) {
                return Ok(Ok(ptr));
            }
            // None free fits: the free ones go first, for the new one's room.
            self.release_host_segments(&mut state)?;
        }
        // Outside every lock: idle executors may be asked, for a few seconds at most.
        if let Err(short) = crate::host_admit(mapped as u64) {
            self.lock()?.stats.host_working_refusals += 1;
            return Ok(Err(Some(short)));
        }
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = self.lock()?;
        let _context = self.driver().enter()?;
        let ptr = match self.driver().alloc_host(mapped) {
            Ok(ptr) => ptr,
            Err(failure) => {
                let error = Error::from(failure);
                if matches!(error, Error::DeviceFault { .. }) {
                    return Err(error);
                }
                if !UNAVAILABLE.swap(true, Ordering::AcqRel) {
                    eprintln!("varena: GPU {device}: no pinned host memory for what the card refused ({error})");
                }
                return Ok(Err(None));
            }
        };
        let mut segment = Segment::new(mapped, class, None);
        segment.host = true;
        segment.chunks = vec![Chunk { offset: 0, bytes: mapped, handle: Some(PINNED) }];
        state.segments.insert(ptr, segment);
        let stats = &mut state.stats;
        stats.host_working_bytes += mapped;
        stats.host_working_peak = stats.host_working_peak.max(stats.host_working_bytes);
        stats.host_working_allocs += 1;
        let each = if class > LARGE { mapped } else { class };
        for at in (0..=mapped - each).step_by(each).map(|offset| ptr + offset as u64) {
            state.blocks.insert(at, Block { host: true, ..Block::free(ptr, each) });
            state.list((None, each, None), at);
        }
        Ok(Ok(take(&mut state, class, bytes, stream).expect("a new host segment has a block")))
    }

    /// Host segments whose blocks are all free and done go back. The caller holds the lock with the
    /// context entered, after a poll.
    fn release_host_segments(&self, state: &mut State) -> Result<()> {
        let done: Vec<(u64, usize)> = state.segments.iter()
            .filter(|(_, segment)| segment.host)
            .filter(|(base, segment)| state.blocks.range(**base..**base + segment.bytes as u64)
                .all(|(_, block)| block.life == Life::Ready && block.reusable_on.is_none()))
            .map(|(base, segment)| (*base, segment.bytes))
            .collect();
        for (base, bytes) in done {
            let blocks: Vec<(u64, usize)> = state.blocks.range(base..base + bytes as u64)
                .map(|(ptr, block)| (*ptr, block.capacity)).collect();
            for (ptr, capacity) in blocks {
                state.blocks.remove(&ptr);
                state.host_free.remove(&(capacity, ptr));
            }
            state.segments.remove(&base);
            state.stats.host_working_bytes -= bytes;
            self.driver().free_host(base)?;
        }
        Ok(())
    }
}

/// The best fitting free host block `stream` may use, cut to `class` and live.
fn take(state: &mut State, class: usize, requested: usize, stream: u64) -> Option<u64> {
    let (capacity, ptr) = state.host_free.range((class, 0)..).copied().find(|(capacity, ptr)| {
        state.blocks.get(ptr).is_some_and(|block| {
            block.capacity == *capacity && block.life == Life::Ready
                && block.reusable_on.is_none_or(|on| on == stream) && (class > LARGE || *capacity == class)
        })
    })?;
    state.host_free.remove(&(capacity, ptr));
    if capacity > class {
        let block = state.blocks.get_mut(&ptr).expect("listed host block");
        let mut tail = block.clone();
        block.capacity = class;
        tail.capacity = capacity - class;
        let (at, origin) = (ptr + class as u64, tail.reusable_on);
        if tail.fenced {
            state.fenced.push(at);
        }
        state.blocks.insert(at, tail);
        state.list((None, capacity - class, origin), at);
    }
    let block = state.blocks.get_mut(&ptr).expect("taken host block");
    block.life = Life::Live;
    block.scratch = false;
    block.workspace = false;
    block.stream = stream;
    block.origin_retired = false;
    block.requested = requested;
    block.phase = None;
    block.waits.clear();
    block.streams.clear();
    block.streams.insert(stream);
    let stats = &mut state.stats;
    stats.allocations += 1;
    stats.host_working_served += 1;
    stats.host_working_served_bytes += requested as u64;
    Some(ptr)
}

/// No request runs on `device`: the host memory its last resort pinned goes back, but for blocks
/// whose work has not completed, which the next release takes.
pub fn release_host(device: i32) -> Result<()> {
    let Some(owner) = super::existing(device)? else { return Ok(()) };
    if !owner.lock()?.segments.values().any(|segment| segment.host) {
        return Ok(());
    }
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    owner.poll(&mut state)?;
    owner.release_host_segments(&mut state)
}
