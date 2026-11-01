//! Device-local accounting/release for an explicit cooperative service tick.
//! Caller holds capture exclusion. No shared-memory lock is taken here.
use super::*;
use crate::cooperative::Accounting;

pub(crate) fn uuid(device: i32) -> Result<[u8; 16]> {
    owner(device)?.driver.uuid()
}

pub(crate) fn memory_info(device: i32) -> Result<(usize, usize)> {
    let owner = owner(device)?;
    let _context = owner.driver.enter()?;
    owner.driver.memory_info()
}

pub(crate) fn cuda_calls(device: i32) -> Result<u64> {
    Ok(owner(device)?.driver.activity_calls())
}

pub(crate) fn memory_calls(device: i32) -> Result<u64> {
    Ok(owner(device)?.driver.calls().0)
}

/// Pressure can lower speculation immediately. A calm observation merely permits the next
/// real phase entry to raise it; no sampler tick raises retention in an active pass.
pub(crate) fn pressure(device: i32, active: bool, calm: bool) -> Result<bool> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    if active {
        state.pressure_pending = true;
        state.prefetch_suppressed = true;
    } else if calm {
        state.pressure_pending = false;
    }
    Ok(state.prefetch_suppressed)
}

pub(crate) fn prefetch_suppressed(device: i32) -> Result<Option<bool>> {
    existing(device)?.map(|owner| Ok(owner.lock()?.prefetch_suppressed)).transpose()
}

/// Keep working ownership frozen while the caller also snapshots its weight arenas.
/// Refresh only queries/records already submitted consumers; it never joins unknown ones.
pub(crate) fn with_snapshot<T>(
    device: i32,
    refresh: bool,
    operation: impl FnOnce(Accounting) -> Result<T>,
) -> Result<T> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let result = (|| {
        let _context = refresh.then(|| owner.driver.enter()).transpose()?;
        if refresh {
            owner.poll_mode(&mut state, false)?;
        }
        let mut reading = Accounting {
            refreshed: refresh,
            ..Accounting::default()
        };
        cache::accounting(&state, owner.page, &mut reading);
        for (base, segment) in &state.segments {
            let (pool_held, pool_pending) = match segment.pool {
                None => (false, false),
                Some(id) => {
                    let pool = &state.pools[&id];
                    let pending = if refresh {
                        !owner.pool_completed(pool)?
                    } else {
                        !pool.events.is_empty() || !pool.retired_events.is_empty()
                    };
                    (
                        pool.prepared || pool.references != 0 || pool.capture.is_some(),
                        pending,
                    )
                }
            };
            let class = if segment.pool.is_some() {
                &mut reading.physical.graph_pools
            } else {
                &mut reading.physical.working
            };
            let mut assigned = 0;
            let mut live = false;
            let mut unknown = false;
            let mut pending = pool_pending;
            for (_, block) in state.blocks.range(*base..*base + segment.bytes as u64) {
                assigned += block.capacity;
                if block.life == Life::Live {
                    class.live += block.capacity as u64;
                    live = true;
                } else if block.life != Life::Ready || block.reusable_on.is_some() || pool_pending {
                    class.pending += block.capacity as u64;
                    pending = true;
                    unknown |=
                        matches!(block.life, Life::Unjoined | Life::Deferred | Life::Captured);
                } else {
                    class.idle += block.capacity as u64;
                }
            }
            let tail = (segment.bytes - assigned) as u64;
            if pool_pending {
                class.pending += tail;
            } else {
                class.idle += tail;
            }
            if !pool_held && !live && !unknown {
                if pending {
                    reading.pending_release += segment.bytes as u64;
                } else {
                    reading.reclaimable += segment.bytes as u64;
                }
            }
        }
        operation(reading)
    })();
    result.map_err(|error| owner.fail(&mut state, error))
}

/// Release only proven complete whole mappings. The cap keeps a cumulative donation within
/// its ask snapshot even if the owner allocated a larger mapping after that snapshot.
pub(crate) fn release(
    device: i32,
    wanted: usize,
    maximum: usize,
    completed: &mut usize,
) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let result = (|| {
        let _context = owner.driver.enter()?;
        owner.poll_mode(&mut state, false)?;
        owner.trim_ready_capped(
            &mut state,
            wanted,
            None,
            &BTreeSet::new(),
            maximum,
            completed,
        )?;
        Ok(())
    })();
    result.map_err(|error| owner.fail(&mut state, error))
}
