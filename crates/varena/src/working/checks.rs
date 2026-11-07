//! Test tools for a gate cell, each enabled by one explicit `set` at its start. Off, an allocation
//! or a free reads one relaxed byte. Nothing in a product path sets them.
//!
//! - `UNMAP_IDLE`: a scratch pool's use begins with every idle pool's pages gone.
//! - `PERTURB`: a freed block, a reused one and a scratch pool's pages read as `PATTERN`, in stream
//!   order, rather than old values or the driver's zeros, which can look plausible.
//! - `GUARDS`: unmapped VA after each pool segment and large block, and each large block in a
//!   segment of its own, so an overrun faults instead of landing on a neighbour.
use super::{LARGE, Life, PoolId};
use crate::{Device, Result};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

pub const UNMAP_IDLE: u8 = 1;
pub const PERTURB: u8 = 2;
pub const GUARDS: u8 = 4;
const PATTERN: u8 = 0xA5;

static CHECKS: AtomicU8 = AtomicU8::new(0);
static USES: AtomicU64 = AtomicU64::new(0);
static UNMAPPED: AtomicU64 = AtomicU64::new(0);
static PERTURBED: AtomicU64 = AtomicU64::new(0);

pub fn set(checks: u8) {
    CHECKS.store(checks, Ordering::Relaxed);
}

/// Scratch pool uses these reached, bytes unmapped for them, bytes given the pattern.
pub fn counts() -> (u64, u64, u64) {
    (USES.load(Ordering::Relaxed), UNMAPPED.load(Ordering::Relaxed), PERTURBED.load(Ordering::Relaxed))
}

#[inline]
pub(super) fn on(check: u8) -> bool {
    CHECKS.load(Ordering::Relaxed) & check != 0
}

/// The VA a large block or pool leaves unmapped after it, while it lives.
pub(super) fn guard(owner: &Device, pool: Option<PoolId>, class: usize) -> usize {
    if on(GUARDS) && (pool.is_some() || class > LARGE) { owner.page() } else { 0 }
}

fn fill(owner: &Device, ptr: u64, bytes: usize, stream: u64) -> Result<()> {
    PERTURBED.fetch_add(bytes as u64, Ordering::Relaxed);
    let _context = owner.driver().enter()?;
    owner.driver().fill(ptr, PATTERN, bytes, stream as usize)
}

/// An eager block on its way to the free list, or to a new owner: in `stream`'s order, so behind
/// its last use there and ahead of the next. A capture would record the fill. A block other
/// streams used keeps its bytes until it is reused: this one's order says nothing of theirs.
pub(super) fn perturb(device: i32, ptr: u64, stream: u64, freeing: bool) -> Result<()> {
    let owner = super::owner(device)?;
    let bytes = {
        let state = owner.lock()?;
        let Some(block) = state.blocks.get(&ptr) else { return Ok(()) };
        let eager = block.life == Life::Live && state.segments[&block.segment].pool.is_none();
        let sole = block.streams.len() == 1 && block.streams.contains(&stream);
        if !eager || !sole || (freeing && block.workspace) || owner.captures.load(Ordering::Acquire) != 0 {
            return Ok(());
        }
        block.capacity
    };
    fill(owner, ptr, bytes, stream)
}

/// Before a recorded scratch pool's use: true when this is its first preparation. Everything
/// submitted is finished, so what `UNMAP_IDLE` takes is what pressure could take.
pub(super) fn before_use(owner: &Device, device: i32, id: PoolId) -> Result<bool> {
    let scratch = owner.lock()?.pools.get(&id).is_some_and(|pool| pool.scratch && !pool.in_use);
    if !scratch || !on(UNMAP_IDLE | PERTURB) {
        return Ok(false);
    }
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    for _ in 0..2 {  // A stamp seals at one poll and reads as done at a later one.
        owner.poll(&mut state)?;
        owner.driver().synchronize()?;
    }
    owner.poll(&mut state)?;
    if on(UNMAP_IDLE) {
        let gone = owner.unmap_idle_pools(&mut state, usize::MAX, None, usize::MAX)?;
        UNMAPPED.fetch_add(gone as u64, Ordering::Relaxed);
    }
    USES.fetch_add(1, Ordering::Relaxed);
    Ok(true)
}

/// A scratch pool mapped for its use: the pattern over all of it, before the owner copies in.
pub(super) fn scramble(owner: &Device, id: PoolId) -> Result<()> {
    if !on(PERTURB) {
        return Ok(());
    }
    let state = owner.lock()?;
    for (base, segment) in state.segments.iter().filter(|(_, segment)| segment.pool == Some(id)) {
        fill(owner, *base, segment.bytes, 0)?;
    }
    let _context = owner.driver().enter()?;
    owner.driver().synchronize()
}
