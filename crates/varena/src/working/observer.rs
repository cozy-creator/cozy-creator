//! Physical Working backing observation. No allocation or reclamation decisions live here.
use super::{INITIALIZE, Life, PoolId, devices};
use crate::Device;
use crate::{Error, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

#[derive(Clone, Copy)]
pub struct WorkingSegment {
    pub device: i32,
    pub address: u64,
    pub bytes: usize,
    /// Eager block class; zero for an unpartitioned prepared graph pool.
    pub size_class: usize,
    pub pool: Option<PoolId>,
}

pub struct WorkingBlock {
    pub bytes: usize,
    pub requested: usize,
    pub allocated: bool,
    pub active: bool,
}

pub struct WorkingSnapshot {
    pub segment: WorkingSegment,
    pub blocks: Vec<WorkingBlock>,
}

/// Called under native allocator locks. Implementations must not re-enter Varena,
/// acquire Python's GIL, or unwind. `initialize` publishes its inventory atomically
/// or leaves no subscription; mapped/retiring failures poison the native owner.
pub trait WorkingObserver: Send + Sync {
    fn initialize(&self, segments: &[WorkingSegment]) -> Result<()>;
    fn mapped(&self, segment: WorkingSegment) -> Result<()>;
    fn retiring(&self, segment: WorkingSegment) -> Result<()>;
}

static OBSERVER: OnceLock<Arc<dyn WorkingObserver>> = OnceLock::new();
/// Handles reported mapped and retiring, and the time those calls took: under a group each is an
/// NCCL registration or deregistration.
static MAPPED: AtomicU64 = AtomicU64::new(0);
static RETIRED: AtomicU64 = AtomicU64::new(0);
static OBSERVED_NS: AtomicU64 = AtomicU64::new(0);

pub(super) fn observed() -> (u64, u64, u64) {
    (MAPPED.load(Ordering::Relaxed), RETIRED.load(Ordering::Relaxed), OBSERVED_NS.load(Ordering::Relaxed))
}

fn timed(count: &AtomicU64, notify: impl FnOnce() -> Result<()>) -> Result<()> {
    let began = Instant::now();
    let result = notify();
    OBSERVED_NS.fetch_add(began.elapsed().as_nanos() as u64, Ordering::Relaxed);
    count.fetch_add(1, Ordering::Relaxed);
    result
}

/// Install one process-lifetime observer with an atomic existing/future handoff.
/// The registry prevents new devices; all existing state locks prevent maps/frees.
pub fn observe_working(observer: Arc<dyn WorkingObserver>) -> Result<()> {
    let _initializing = INITIALIZE.lock().map_err(|_| Error::Poisoned("working initialization".into()))?;
    let owners = devices()?;
    if OBSERVER.get().is_some() {
        return Err(Error::Invalid("working observer already installed"));
    }
    let states = owners.iter().map(|owner| owner.lock()).collect::<Result<Vec<_>>>()?;
    let mut segments = Vec::new();
    for (owner, state) in owners.iter().zip(&states) {
        // By handle, as each comes and goes.
        for (address, segment) in &state.segments {
            segments.extend(segment.chunks.iter().filter(|chunk| chunk.handle.is_some()).map(|chunk| WorkingSegment {
                device: owner.ordinal, address: *address + chunk.offset as u64, bytes: chunk.bytes,
                size_class: segment.size_class, pool: segment.pool,
            }));
        }
    }
    observer.initialize(&segments)?;
    OBSERVER.set(observer).map_err(|_| Error::Invalid("working observer already installed"))
}

/// A coherent per-device inventory of actual backing and live/pending block state, one row per
/// handle as `mapped` and `retiring` report them: NCCL registers what it reads here, and a later
/// retirement names that same address. A block across two handles shows on each what it holds.
/// This visits existing devices only and performs no CUDA initialization or collection.
pub fn working_snapshot() -> Result<Vec<WorkingSnapshot>> {
    let owners = devices()?;
    let mut out = Vec::new();
    for owner in owners {
        let state = owner.lock()?;
        for (address, segment) in &state.segments {
            for chunk in segment.chunks.iter().filter(|chunk| chunk.handle.is_some()) {
                let start = *address + chunk.offset as u64;
                let end = start + chunk.bytes as u64;
                let mut cursor = start;
                let mut blocks = Vec::new();
                for (base, block) in state.blocks.range(*address..end) {
                    let (from, to) = ((*base).max(start), (*base + block.capacity as u64).min(end));
                    if from >= to {
                        continue;
                    }
                    if cursor < from {
                        blocks.push(WorkingBlock { bytes: (from - cursor) as usize,
                            requested: 0, allocated: false, active: false });
                    }
                    let bytes = (to - from) as usize;
                    blocks.push(WorkingBlock {
                        bytes,
                        requested: block.requested.saturating_sub((from - *base) as usize).min(bytes),
                        allocated: block.life == Life::Live,
                        active: block.charge() != 0,
                    });
                    cursor = to;
                }
                // Eager classes need not divide a backing slab. This is real mapped
                // but unassigned space, not an allocation slot or a promise of reuse.
                if cursor < end {
                    blocks.push(WorkingBlock { bytes: (end - cursor) as usize,
                        requested: 0, allocated: false, active: false });
                }
                out.push(WorkingSnapshot {
                    segment: WorkingSegment { device: owner.ordinal, address: start, bytes: chunk.bytes,
                        size_class: segment.size_class, pool: segment.pool }, blocks,
                });
            }
        }
    }
    Ok(out)
}

pub(super) fn mapped(owner: &Device, address: u64, bytes: usize, pool: Option<PoolId>, size_class: usize) -> Result<()> {
    if let Some(observer) = OBSERVER.get() {
        timed(&MAPPED, || observer.mapped(WorkingSegment { device: owner.ordinal, address, bytes, size_class, pool }))
            .map_err(|error| Error::Poisoned(format!("working map observer: {error}")))?;
    }
    Ok(())
}

pub(super) fn retiring(owner: &Device, address: u64, bytes: usize, pool: Option<PoolId>, size_class: usize) -> Result<()> {
    if let Some(observer) = OBSERVER.get() {
        timed(&RETIRED, || observer.retiring(WorkingSegment { device: owner.ordinal, address, bytes, size_class, pool }))
            .map_err(|error| Error::Poisoned(format!("working retirement observer: {error}")))?;
    }
    Ok(())
}
