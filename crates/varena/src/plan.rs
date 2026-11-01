//! Immutable device-wide region intent, independent of execution fingerprints.
//! Strong lifetime leases precede capture -> arena -> working guards. Arena locks
//! are visited one at a time in stable identity order, never with an owning drop.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::{Arena, Completion, Contents, Error, ReclaimReason, ReclaimReport, Region, Result};

struct Phase {
    name: String,
    regions: Vec<Arc<Region>>,
    ids: BTreeSet<u64>,
    bytes: usize,
}

/// Copied same-device phase definitions retain reservations, never physical Pins.
/// Lifetime identities/generations validate intent; they are not compute fingerprints.
pub struct Plan {
    id: u64,
    device: i32,
    pid: u32,
    arenas: BTreeMap<u64, Arc<Arena>>,
    phases: Vec<Phase>,
}

struct Active {
    id: u64,
    plan: Weak<Plan>,
    phase: usize,
}

#[derive(Default)]
struct Device {
    generation: u64,
    inventory_generation: u64,
    unusable: bool,
    active: Option<Active>,
}

impl Device {
    fn check(&self) -> Result<()> {
        if self.unusable {
            Err(Error::Invalid("phase-plan inventory epoch exhausted"))
        } else {
            Ok(())
        }
    }

    fn advance(&mut self, inventory: bool) -> Result<()> {
        self.check()?;
        let generation = self
            .generation
            .checked_add(1)
            .filter(|value| *value != u64::MAX);
        let next_inventory = if inventory {
            self.inventory_generation
                .checked_add(1)
                .filter(|value| *value != u64::MAX)
        } else {
            Some(self.inventory_generation)
        };
        match (generation, next_inventory) {
            (Some(generation), Some(inventory_generation)) => {
                self.generation = generation;
                self.inventory_generation = inventory_generation;
                Ok(())
            }
            _ => {
                // Destructor callers cannot return this error. Latch it so an old
                // lease cannot remain valid after an unrepresentable retirement.
                self.unusable = true;
                Err(Error::Invalid("phase-plan inventory epoch exhausted"))
            }
        }
    }
}

static ACTIVE: OnceLock<Mutex<BTreeMap<i32, Device>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct PhaseFacts {
    pub name: String,
    pub regions: usize,
    pub bytes: usize,
}

/// Completed unpinned outside regions provide credit only for their actual mappings.
/// A missing planned region is counted once, regardless of its arena or duplicate input.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub name: String,
    pub bytes: usize,
    pub missing: usize,
    pub outside_reclaimable: usize,
}

fn registry() -> Result<std::sync::MutexGuard<'static, BTreeMap<i32, Device>>> {
    crate::check_process()?;
    ACTIVE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("phase-plan registry mutex".into()))
}

fn arenas(device:i32) -> Result<Vec<Arc<Arena>>> { Ok(crate::arena_inventory(device)?.arenas) }

/// Called under capture exclusion, or the registration mutex for an accepted final
/// last-Arc handoff that no live owning snapshot can concurrently reference.
/// An epoch can conservatively advance before a failed publication; it grants no bytes.
pub(crate) fn inventory_changed(device: i32) -> Result<()> {
    registry()?.entry(device).or_default().advance(true)
}

fn epochs(device: i32) -> Result<(u64, u64)> {
    let mut registry = registry()?;
    let entry = registry.entry(device).or_default();
    entry.check()?;
    Ok((entry.generation, entry.inventory_generation))
}

pub(crate) fn generation(device: i32) -> Result<u64> {
    Ok(epochs(device)?.0)
}

/// Device-consistent arena/region publication/retirement epoch; not a capacity grant.
pub fn inventory_generation(device: i32) -> Result<u64> {
    Ok(epochs(device)?.1)
}

/// Own every referenced GPU owner before acquiring capture/arena/working guards.
/// Callers declare this first so every later guard drops first, including error paths.
pub(crate) struct Lease {
    device: i32,
    generation: u64,
    inventory_generation: u64,
    active: Option<(Arc<Plan>, usize)>,
    arenas: Vec<Arc<Arena>>,
}

impl Lease {
    pub(crate) fn acquire(device: i32) -> Result<Self> {
        let (generation, inventory_generation, active) = {
            let mut registry = registry()?;
            let entry = registry.entry(device).or_default();
            entry.check()?;
            let active = match &entry.active {
                Some(active) => Some((active.plan.upgrade().ok_or(Error::Busy)?, active.phase)),
                None => None,
            };
            (entry.generation, entry.inventory_generation, active)
        };
        let arenas = arenas(device)?;
        Ok(Self {
            device,
            generation,
            inventory_generation,
            active,
            arenas,
        })
    }

    pub(crate) fn validate(&self) -> Result<u64> {
        if epochs(self.device)? != (self.generation, self.inventory_generation) {
            return Err(Error::Invalid(
                "phase-region intent or inventory changed before admission",
            ));
        }
        Ok(self.generation)
    }

    fn planned(&self) -> Result<Option<(&Plan, usize)>> {
        self.validate()?;
        let Some((plan, phase)) = &self.active else {
            return Ok(None);
        };
        plan.validate_arenas(&self.arenas)?;
        Ok(Some((plan, *phase)))
    }
}

impl Plan {
    pub fn new(
        anchor: Arc<Arena>,
        definitions: Vec<(String, Vec<Arc<Region>>)>,
    ) -> Result<Arc<Self>> {
        crate::check_process()?;
        if definitions.is_empty() {
            return Err(Error::Invalid("phase plan is empty"));
        }
        let device = anchor.device;
        let mut arenas = BTreeMap::from([(anchor.id(), anchor)]);
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
                if region.device() != device {
                    return Err(Error::Invalid(
                        "phase plan regions must belong to the anchor device",
                    ));
                }
                if ids.insert(region.id()) {
                    bytes = bytes
                        .checked_add(region.rounded)
                        .ok_or(Error::Invalid("phase region bytes overflow"))?;
                    arenas
                        .entry(region.arena_id())
                        .or_insert_with(|| region.arena.clone());
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
        Ok(Arc::new(Self {
            id: crate::next_identity()?,
            device,
            pid: std::process::id(),
            arenas,
            phases,
        }))
    }

    pub fn device(&self) -> i32 {
        self.device
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

    fn validate_arenas(&self, live: &[Arc<Arena>]) -> Result<()> {
        let found: BTreeSet<_> = live.iter().map(|arena| arena.id()).collect();
        if self.arenas.keys().any(|id| !found.contains(id)) {
            return Err(Error::Invalid(
                "planned arena is not in the device inventory",
            ));
        }
        Ok(())
    }

    /// Activate one device-wide phase. No allocation, Pin, fill or sample is performed.
    pub fn activate(self: &Arc<Self>, name: &str) -> Result<()> {
        let lease = Lease::acquire(self.device)?;
        let captures = crate::capture_counts()?;
        crate::ensure_recovery_allowed(&captures, self.device)?;
        lease.validate()?;
        self.validate_arenas(&lease.arenas)?;
        let phase = self
            .phases
            .iter()
            .position(|phase| phase.name == name)
            .ok_or(Error::Invalid("unknown phase in plan"))?;
        let mut registry = registry()?;
        let entry = registry.entry(self.device).or_default();
        if let Some(prior) = &entry.active {
            if prior.id != self.id {
                return Err(Error::Invalid("another phase plan is active on this GPU"));
            }
            if prior.phase == phase {
                return Ok(());
            }
        }
        entry.advance(false)?;
        entry.active = Some(Active {
            id: self.id,
            plan: Arc::downgrade(self),
            phase,
        });
        Ok(())
    }

    pub fn deactivate(self: &Arc<Self>) -> Result<()> {
        let _captures = crate::capture_counts()?;
        self.retire()
    }

    fn retire(&self) -> Result<()> {
        let mut registry = registry()?;
        let entry = registry.entry(self.device).or_default();
        if entry
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id)
        {
            let result = entry.advance(false);
            entry.active = None;
            return result;
        }
        Ok(())
    }
}

impl Drop for Plan {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            return;
        }
        // Owning fields are destroyed only after this body's capture guard releases.
        if let Ok(_captures) = crate::capture_counts() {
            let _ = self.retire();
        }
    }
}

/// Caller holds capture exclusion; Working has not yet been locked. Visit every
/// arena once, with no simultaneous arena mutexes and no owning Arc temporaries.
pub(crate) fn snapshot(lease: &Lease) -> Result<Option<Snapshot>> {
    let Some((plan, index)) = lease.planned()? else {
        return Ok(None);
    };
    let phase = &plan.phases[index];
    let mut missing = 0usize;
    let mut outside_reclaimable = 0usize;
    let mut found = BTreeSet::new();
    for arena in &lease.arenas {
        let mut state = arena.lock()?;
        let _context = arena.driver.enter()?;
        for id in &phase.ids {
            if let Some(entry) = state.entries.get(id) {
                if entry.uses.state() == Contents::Retired {
                    return Err(Error::Invalid("planned region is retired"));
                }
                found.insert(*id);
                if entry.handle.is_none() {
                    missing = missing
                        .checked_add(entry.bytes)
                        .ok_or(Error::Invalid("phase missing bytes overflow"))?;
                }
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
        for (id, bytes) in candidates {
            if arena.completion(&mut state, id)? == Completion::Done {
                outside_reclaimable = outside_reclaimable
                    .checked_add(bytes)
                    .ok_or(Error::Invalid("outside phase bytes overflow"))?;
            }
        }
    }
    if found != phase.ids {
        return Err(Error::Invalid("planned region is missing from inventory"));
    }
    Ok(Some(Snapshot {
        name: phase.name.clone(),
        bytes: phase.bytes,
        missing,
        outside_reclaimable,
    }))
}

/// Native outside-phase order: stable Arena ID, then that arena's existing eviction
/// key. Independent per-arena clocks are NOT compared as a fictitious global LRU.
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
    let mut report = ReclaimReport {
        requested_bytes: bytes,
        ..ReclaimReport::default()
    };
    for arena in &lease.arenas {
        if report.reclaimed_bytes >= bytes {
            break;
        }
        let mut state = arena.lock()?;
        let _context = arena.driver.enter()?;
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
        candidates.sort_by_key(|(id, key)| (*key, *id));
        for (id, _) in candidates {
            if report.reclaimed_bytes >= bytes {
                break;
            }
            if arena.completion(&mut state, id)? == Completion::Done {
                arena.evict(&mut state, id, ReclaimReason::Allocator, &mut report)?;
            }
        }
    }
    Ok(report.reclaimed_bytes)
}
