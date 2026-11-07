//! What the donor thread needs from Working. Host metadata unless it says otherwise.
use super::*;

/// Driver free and total. The caller holds capture exclusion.
pub(crate) fn memory_info(device: i32) -> Result<(usize, usize)> {
    let owner = owner(device)?;
    let _context = owner.driver().enter()?;
    owner.driver().memory_info()
}

/// Pressure lowers speculation at once. A calm sample only lets the next real phase entry raise
/// it; nothing regrows in the middle of a pass.
pub(crate) fn pressure(device: i32, active: bool, calm: bool) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    if active {
        state.pressure_pending = true;
        state.prefetch_suppressed = true;
    } else if calm {
        state.pressure_pending = false;
    }
    Ok(())
}

/// Whole mappings no live block or held pool keeps, plus idle cache pages. An estimate: the trim
/// decides for real, and a stream still finishing counts as idle here.
pub(crate) fn idle_bytes(device: i32) -> Result<u64> {
    let Some(owner) = existing(device)? else { return Ok(0) };
    let state = owner.lock()?;
    let mut idle = cache::ready_bytes(&state, owner.page()) as u64;
    for (base, segment) in &state.segments {
        let held = segment.pool.is_some_and(|id| {
            let pool = &state.pools[&id];
            pool.prepared || (pool.references != 0 && !Device::pool_idle(&state, id, pool)) || pool.capture.is_some()
        });
        let live = |chunk: &Chunk| state.touching(*base + chunk.offset as u64, *base + (chunk.offset + chunk.bytes) as u64)
            .any(|block| matches!(block.life, Life::Live | Life::Captured | Life::Unjoined(_)) && !block.scratch);
        if !held {
            idle += segment.chunks.iter().filter(|chunk| chunk.handle.is_some() && !live(chunk))
                .map(|chunk| chunk.bytes as u64).sum::<u64>();
        }
    }
    Ok(idle)
}
