//! Immutable device-wide region intent, independent of execution fingerprints.
//! Strong lifetime leases precede gate -> arena -> working guards. Arena locks
//! are visited one at a time in stable identity order, never with an owning drop.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::{Arena, Completion, Contents, Error, Region, Result};

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
    active: Option<Active>,
}

impl Device {
    fn advance(&mut self, inventory: bool) -> Result<()> {
        // An unrelated region's birth or death changes no plan: it never fails a selection.
        if inventory {
            self.inventory_generation += 1;
        } else {
            self.generation += 1;
        }
        Ok(())
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


/// Device inventory metadata for diagnostics; it grants no bytes and fails no selection.
pub(crate) fn inventory_changed(device: i32) -> Result<()> {
    registry()?.entry(device).or_default().advance(true)
}

fn epochs(device: i32) -> Result<(u64, u64)> {
    let mut registry = registry()?;
    let entry = registry.entry(device).or_default();
    Ok((entry.generation, entry.inventory_generation))
}

pub(crate) fn generation(device: i32) -> Result<u64> {
    Ok(epochs(device)?.0)
}

/// Device-consistent arena/region publication/retirement epoch; not a capacity grant.
pub fn inventory_generation(device: i32) -> Result<u64> {
    Ok(epochs(device)?.1)
}

/// Own every referenced GPU owner before acquiring gate/arena/working guards.
/// Callers declare this first so every later guard drops first, including error paths.
pub(crate) struct Lease {
    device: i32,
    generation: u64,
    inventory_generation: u64,
    active: Option<(Arc<Plan>, usize)>,
    arenas: Vec<Arc<Arena>>,
}

impl Lease {
    pub(crate) fn identity(&self) -> (u64, u64, Option<(u64, usize)>) {
        (self.generation, self.inventory_generation,
            self.active.as_ref().map(|(plan, phase)| (plan.id, *phase)))
    }

    pub(crate) fn validate_use(&self, ids: &BTreeSet<u64>) -> Result<()> {
        if let Some((plan, phase)) = self.planned()? && &plan.phases[phase].ids != ids {
            return Err(Error::Invalid("prepared Use must name the active Plan's exact Region set"));
        }
        Ok(())
    }

    pub(crate) fn acquire(device: i32) -> Result<Self> {
        let (generation, inventory_generation, active) = {
            let mut registry = registry()?;
            let entry = registry.entry(device).or_default();
            let active = match &entry.active {
                Some(active) => active.plan.upgrade().map(|plan| (plan, active.phase)),
                None => None,
            };
            (entry.generation, entry.inventory_generation, active)
        };
        let arenas = crate::reclaim_arenas(device)?;
        Ok(Self {
            device,
            generation,
            inventory_generation,
            active,
            arenas,
        })
    }

    pub(crate) fn validate(&self) -> Result<u64> {
        if generation(self.device)? != self.generation {
            return Err(Error::Invalid("phase-region intent changed before admission"));
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
        crate::gate(self.device)?.not_capturing()?;
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
        let _ = self.retire();
    }
}

/// Caller holds the gate; Working has not yet been locked. Visit every arena once,
/// with no simultaneous arena mutexes and no owning Arc temporaries.
pub(crate) fn snapshot(lease: &Lease) -> Result<Option<Snapshot>> {
    let Some((plan, index)) = lease.planned()? else {
        return Ok(None);
    };
    let phase = &plan.phases[index];
    let mut missing = 0usize;
    let mut outside_reclaimable = 0usize;
    let mut found = BTreeSet::new();
    for arena in &lease.arenas {
        let state = arena.lock()?;
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
                    && !matches!(entry.uses.state(), Contents::Loading | Contents::Published)
            })
            .map(|(id, entry)| (*id, entry.bytes))
            .collect();
        for (id, bytes) in candidates {
            if arena.completion(&state, id) == Completion::Done {
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

/// Next use of each planned region: phases ahead of the active one, then position within it.
/// Plain data, so nothing here can run a final Region or Arena drop under a gate.
#[derive(Default)]
pub(crate) struct Order {
    next: BTreeMap<u64, (u64, u64)>,
}

impl Order {
    pub(crate) fn next_use(&self, id: u64) -> Option<(u64, u64)> {
        self.next.get(&id).copied()
    }
    pub(crate) fn planned(&self) -> bool {
        !self.next.is_empty()
    }
}

/// One active plan's order. Call before taking the gate: a Plan's last drop reaches Regions.
pub(crate) fn order(device: i32) -> Result<Order> {
    let active = {
        let registry = registry()?;
        registry.get(&device).and_then(|entry| entry.active.as_ref())
            .and_then(|active| Some((active.plan.upgrade()?, active.phase)))
    };
    let mut order = Order::default();
    if let Some((plan, active)) = active {
        for ahead in 0..plan.phases.len() {
            let phase = &plan.phases[(active + ahead) % plan.phases.len()];
            for (position, region) in phase.regions.iter().enumerate() {
                order.next.entry(region.id()).or_insert((ahead as u64, position as u64));
            }
        }
    }
    Ok(order)
}
