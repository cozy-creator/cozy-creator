//! Physical Working backing observation. No allocation or reclamation decisions live here.
use super::{INITIALIZE, Life, PoolId, devices};
use crate::Device;
use crate::{Error, Result};
use std::sync::{Arc, OnceLock};

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
        segments.extend(state.segments.iter().filter(|(_, segment)| segment.handle.is_some()).map(|(address, segment)| WorkingSegment {
            device: owner.ordinal, address: *address, bytes: segment.bytes, size_class: segment.size_class, pool: segment.pool,
        }));
    }
    observer.initialize(&segments)?;
    OBSERVER.set(observer).map_err(|_| Error::Invalid("working observer already installed"))
}

/// A coherent per-device inventory of actual backing and live/pending block state.
/// This visits existing devices only and performs no CUDA initialization or collection.
pub fn working_snapshot() -> Result<Vec<WorkingSnapshot>> {
    let owners = devices()?;
    let mut out = Vec::new();
    for owner in owners {
        let state = owner.lock()?;
        for (address, segment) in state.segments.iter().filter(|(_, segment)| segment.handle.is_some()) {
            let end = *address + segment.bytes as u64;
            let mut cursor = *address;
            let mut blocks = Vec::new();
            for (base, block) in state.blocks.range(*address..end) {
                if cursor < *base {
                    blocks.push(WorkingBlock { bytes: (*base - cursor) as usize,
                        requested: 0, allocated: false, active: false });
                }
                blocks.push(WorkingBlock {
                    bytes: block.capacity,
                    requested: block.requested,
                    allocated: block.life == Life::Live,
                    active: block.charge() != 0,
                });
                cursor = *base + block.capacity as u64;
            }
            // Eager classes need not divide a backing slab. This is real mapped
            // but unassigned space, not an allocation slot or a promise of reuse.
            if cursor < end {
                blocks.push(WorkingBlock { bytes: (end - cursor) as usize,
                    requested: 0, allocated: false, active: false });
            }
            out.push(WorkingSnapshot {
                segment: WorkingSegment { device: owner.ordinal, address: *address,
                    bytes: segment.bytes, size_class: segment.size_class, pool: segment.pool }, blocks,
            });
        }
    }
    Ok(out)
}

pub(super) fn mapped(owner: &Device, address: u64, bytes: usize, pool: Option<PoolId>, size_class: usize) -> Result<()> {
    if let Some(observer) = OBSERVER.get() {
        observer.mapped(WorkingSegment { device: owner.ordinal, address, bytes, size_class, pool })
            .map_err(|error| Error::Poisoned(format!("working map observer: {error}")))?;
    }
    Ok(())
}

pub(super) fn retiring(owner: &Device, address: u64, bytes: usize, pool: Option<PoolId>, size_class: usize) -> Result<()> {
    if let Some(observer) = OBSERVER.get() {
        observer.retiring(WorkingSegment { device: owner.ordinal, address, bytes, size_class, pool })
            .map_err(|error| Error::Poisoned(format!("working retirement observer: {error}")))?;
    }
    Ok(())
}
