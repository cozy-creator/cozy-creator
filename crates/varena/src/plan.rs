//! Immutable region intent. This is not an execution key or permission to prefetch.
//!
//! The initial contract is one arena on one GPU. Snapshots are taken in the existing
//! capture -> arena -> working lock order, and contain numbers, not physical leases.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::{Arena, Completion, Contents, Error, ReclaimReason, ReclaimReport, Region, Result};

struct Phase {
    name: String,
    regions: Vec<Arc<Region>>,
    ids: BTreeSet<u64>,
    bytes: usize,
}

/// Copied phase definitions retain reservations, never physical Pins or source callbacks.
pub struct Plan {
    arena: Arc<Arena>,
    phases: Vec<Phase>,
}

struct Active {
    plan: Weak<Plan>,
    phase: usize,
}

#[derive(Default)]
struct Device {
    generation: u64,
    arena_generation: u64,
    active: Option<Active>,
}

static ACTIVE: OnceLock<Mutex<BTreeMap<i32, Device>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct PhaseFacts {
    pub name: String,
    pub regions: usize,
    pub bytes: usize,
}

/// Only completed unpinned outside regions provide reclaim credit. Missing phase
/// backing is counted once; its already-mapped pages are already in the device ledger.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub name: String,
    pub bytes: usize,
    pub missing: usize,
    pub outside_reclaimable: usize,
}

fn registry() -> Result<std::sync::MutexGuard<'static, BTreeMap<i32, Device>>> {
    ACTIVE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("phase-plan registry mutex".into()))
}

fn arenas(device: i32) -> Result<Vec<Arc<Arena>>> {
    let Some(found) = crate::ARENAS.get() else {
        return Ok(Vec::new());
    };
    // Called before capture exclusion. Filter by the registered device before
    // upgrading; every acquired owner outlives the later capture/working guards.
    let live: Vec<_> = {
        let registry = found
            .lock()
            .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
        registry
            .iter()
            .filter(|(registered_device, _)| *registered_device == device)
            .filter_map(|(_, arena)| arena.upgrade())
            .collect()
    };
    Ok(live)
}

fn advance(device: &mut Device) -> Result<()> {
    device.generation = device
        .generation
        .checked_add(1)
        .ok_or(Error::Invalid("phase-plan generation exhausted"))?;
    Ok(())
}

/// Arena publication/retirement calls this while capture exclusion is held.
pub(crate) fn arena_changed(device: i32) -> Result<()> {
    let mut registry = registry()?;
    let entry = registry.entry(device).or_default();
    entry.arena_generation = entry
        .arena_generation
        .checked_add(1)
        .ok_or(Error::Invalid("arena inventory generation exhausted"))?;
    if entry
        .active
        .as_ref()
        .is_some_and(|active| active.plan.strong_count() != 0)
    {
        advance(entry)?;
    }
    Ok(())
}

pub(crate) fn generation(device: i32) -> Result<u64> {
    let generation = registry()?.entry(device).or_default().generation;
    if generation == u64::MAX {
        return Err(Error::Invalid("phase-plan generation exhausted"));
    }
    Ok(generation)
}

/// Owning leases are acquired BEFORE any capture/arena/working guard. Every caller
/// declares this first so Rust drops all guards first on success and every error.
/// A snapshot is numbers only; it never upgrades or drops GPU-owning handles.
pub(crate) struct Lease {
    device: i32,
    generation: u64,
    arena_generation: u64,
    active: Option<(Arc<Plan>, usize)>,
    arenas: Vec<Arc<Arena>>,
}

impl Lease {
    pub(crate) fn acquire(device: i32) -> Result<Self> {
        let (generation, arena_generation, active) = {
            let mut registry = registry()?;
            let entry = registry.entry(device).or_default();
            let active = match &entry.active {
                Some(active) => Some((
                    active
                        .plan
                        .upgrade()
                        .ok_or(Error::Invalid("phase-region plan is retiring"))?,
                    active.phase,
                )),
                None => None,
            };
            (entry.generation, entry.arena_generation, active)
        }; // registry unlocks before any owning Arc can be dropped
        let arenas = arenas(device)?;
        Ok(Self {
            device,
            generation,
            arena_generation,
            active,
            arenas,
        })
    }

    pub(crate) fn validate(&self) -> Result<u64> {
        if generation(self.device)? != self.generation {
            return Err(Error::Invalid(
                "phase-region intent changed before admission",
            ));
        }
        Ok(self.generation)
    }

    fn validate_inventory(&self) -> Result<()> {
        self.validate()?;
        if registry()?.entry(self.device).or_default().arena_generation != self.arena_generation {
            return Err(Error::Invalid(
                "arena inventory changed before plan activation",
            ));
        }
        Ok(())
    }

    fn planned(&self) -> Result<Option<(&Plan, usize)>> {
        self.validate()?;
        let Some((plan, index)) = &self.active else {
            return Ok(None);
        };
        if self.arenas.len() != 1 || !Arc::ptr_eq(&self.arenas[0], &plan.arena) {
            return Ok(None); // another arena's unknown order stays unplanned
        }
        Ok(Some((plan, *index)))
    }
}

impl Plan {
    pub fn new(
        arena: Arc<Arena>,
        definitions: Vec<(String, Vec<Arc<Region>>)>,
    ) -> Result<Arc<Self>> {
        crate::check_process()?;
        if definitions.is_empty() {
            return Err(Error::Invalid("phase plan is empty"));
        }
        let mut names = BTreeSet::new();
        let mut phases = Vec::with_capacity(definitions.len());
        for (name, regions) in definitions {
            if name.is_empty() || !names.insert(name.clone()) {
                return Err(Error::Invalid(
                    "phase plan names must be nonempty and unique",
                ));
            }
            let mut ids = BTreeSet::new();
            let mut retained = Vec::new();
            let mut bytes = 0usize;
            for region in regions {
                if !Arc::ptr_eq(&region.arena, &arena) {
                    return Err(Error::Invalid(
                        "phase plan regions must belong to one arena",
                    ));
                }
                if ids.insert(region.id) {
                    bytes = bytes
                        .checked_add(region.rounded)
                        .ok_or(Error::Invalid("phase region bytes overflow"))?;
                    retained.push(region);
                }
            }
            phases.push(Phase {
                name,
                regions: retained,
                ids,
                bytes,
            });
        }
        Ok(Arc::new(Self { arena, phases }))
    }

    pub fn device(&self) -> i32 {
        self.arena.device
    }

    pub fn facts(&self) -> Vec<PhaseFacts> {
        self.phases
            .iter()
            .map(|phase| PhaseFacts {
                name: phase.name.clone(),
                regions: phase.regions.len(),
                bytes: phase.bytes,
            })
            .collect()
    }

    /// Activate intent only. Caller must deactivate a different live plan first.
    /// No allocation, eviction, read, Pin or execution sample is performed here.
    pub fn activate(self: &Arc<Self>, name: &str) -> Result<()> {
        let lease = Lease::acquire(self.arena.device)?;
        let captures = crate::capture_counts()?;
        crate::ensure_recovery_allowed(&captures, self.arena.device)?;
        lease.validate_inventory()?;
        if lease.arenas.len() != 1 || !Arc::ptr_eq(&lease.arenas[0], &self.arena) {
            return Err(Error::Unsupported(
                "phase plans require one arena on the GPU".into(),
            ));
        }
        let phase = self
            .phases
            .iter()
            .position(|p| p.name == name)
            .ok_or(Error::Invalid("unknown phase in plan"))?;
        let mut registry = registry()?;
        let entry = registry.entry(self.arena.device).or_default();
        if let Some(prior) = &entry.active {
            if prior.plan.strong_count() != 0 && !prior.plan.ptr_eq(&Arc::downgrade(self)) {
                return Err(Error::Invalid("another phase plan is active on this GPU"));
            }
            if prior.plan.ptr_eq(&Arc::downgrade(self)) && prior.phase == phase {
                return Ok(());
            }
        }
        advance(entry)?;
        entry.active = Some(Active {
            plan: Arc::downgrade(self),
            phase,
        });
        Ok(())
    }

    /// Synchronous metadata retirement; actual Region ownership lasts until Plan drop.
    pub fn deactivate(self: &Arc<Self>) -> Result<()> {
        let _captures = crate::capture_counts()?;
        self.retire()
    }

    fn retire(&self) -> Result<()> {
        let mut registry = registry()?;
        let entry = registry.entry(self.arena.device).or_default();
        if entry
            .active
            .as_ref()
            .is_some_and(|p| std::ptr::eq(p.plan.as_ptr(), self))
        {
            advance(entry)?;
            entry.active = None;
        }
        Ok(())
    }
}

impl Drop for Plan {
    fn drop(&mut self) {
        if self.arena.pid != std::process::id() {
            return;
        }
        // Only public owners / pre-guard Lease values own Plan. Region fields are
        // destroyed after this body, hence after this capture guard has released.
        if let Ok(_captures) = crate::capture_counts() {
            let _ = self.retire();
        }
    }
}

/// The caller holds capture exclusion and has not acquired the working mutex.
pub(crate) fn snapshot(lease: &Lease) -> Result<Option<Snapshot>> {
    let Some((plan, index)) = lease.planned()? else {
        return Ok(None);
    };
    let phase = &plan.phases[index];
    let mut state = plan.arena.lock()?;
    let _context = plan.arena.driver.enter()?;
    let mut missing = 0usize;
    for id in &phase.ids {
        let entry = state
            .entries
            .get(id)
            .ok_or(Error::Invalid("planned region is retired"))?;
        if entry.handle.is_none() {
            missing += entry.bytes;
        }
    }
    let candidates: Vec<_> = state
        .entries
        .iter()
        .filter(|(id, entry)| {
            !phase.ids.contains(id)
                && entry.handle.is_some()
                && entry.uses.pins() == 0
                && entry.uses.state() != Contents::Loading
        })
        .map(|(id, entry)| (*id, entry.bytes))
        .collect();
    let mut outside_reclaimable = 0usize;
    for (id, bytes) in candidates {
        if plan.arena.completion(&mut state, id)? == Completion::Done {
            outside_reclaimable += bytes;
        }
    }
    Ok(Some(Snapshot {
        name: phase.name.clone(),
        bytes: phase.bytes,
        missing,
        outside_reclaimable,
    }))
}

/// Release completed outside-phase cache only. This does not pin phase weights or
/// promise capacity; ordinary allocation recovery still handles later pressure.
pub(crate) fn reclaim_outside(device: i32, bytes: usize) -> Result<usize> {
    if bytes == 0 {
        return Ok(0);
    }
    let lease = Lease::acquire(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let Some((plan, index)) = lease.planned()? else {
        return Ok(0);
    };
    let phase = &plan.phases[index];
    let mut state = plan.arena.lock()?;
    let _context = plan.arena.driver.enter()?;
    let mut candidates: Vec<_> = state
        .entries
        .iter()
        .filter(|(id, entry)| {
            !phase.ids.contains(id)
                && entry.handle.is_some()
                && entry.uses.pins() == 0
                && entry.uses.state() != Contents::Loading
        })
        .map(|(id, entry)| (*id, state.eviction_key(entry)))
        .collect();
    candidates.sort_by_key(|(_, key)| *key);
    let mut report = ReclaimReport {
        requested_bytes: bytes,
        ..ReclaimReport::default()
    };
    for (id, _) in candidates {
        if report.reclaimed_bytes >= bytes {
            break;
        }
        if plan.arena.completion(&mut state, id)? == Completion::Done {
            plan.arena
                .evict(&mut state, id, ReclaimReason::Allocator, &mut report)?;
        }
    }
    Ok(report.reclaimed_bytes)
}
