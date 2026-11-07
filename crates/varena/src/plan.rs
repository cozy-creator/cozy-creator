//! Region intent on one device: which regions each phase uses, in what order. Holds
//! reservations, never physical pins, and is independent of execution fingerprints.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use crate::{Completion, Contents, Device, Error, Region, Result};

struct Phase {
    name: String,
    regions: Vec<Arc<Region>>,
    ids: BTreeSet<u64>,
    bytes: usize,
}

pub struct Plan {
    id: u64,
    device: &'static Device,
    pid: u32,
    phases: Vec<Phase>,
    // Its Regions' bytes, each once: the request's weights.
    weights: usize,
}

pub(crate) struct Active {
    id: u64,
    plan: Weak<Plan>,
    phase: usize,
}

#[derive(Clone, Debug)]
pub struct PhaseFacts {
    pub name: String,
    pub regions: usize,
    pub bytes: usize,
}

/// The active phase against the table: what it lacks, and what idle memory outside it could give.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub name: String,
    pub bytes: usize,
    pub missing: usize,
    pub outside_reclaimable: usize,
}

/// Owns the active plan's regions before gate and table guards are taken: a Plan's last drop
/// reaches Regions, which need the gate. Declare it first so every later guard drops first.
pub(crate) struct Lease {
    device: &'static Device,
    generation: u64,
    active: Option<(Arc<Plan>, usize)>,
}

impl Lease {
    pub(crate) fn acquire(ordinal: i32) -> Result<Self> {
        let device = crate::gpu(ordinal)?;
        let active = device.plan.lock().map_err(|_| Error::Poisoned("plan mutex".into()))?;
        Ok(Self {
            device,
            generation: device.plan_generation.load(Ordering::Acquire),
            active: active.as_ref().and_then(|active| Some((active.plan.upgrade()?, active.phase))),
        })
    }

    pub(crate) fn validate(&self) -> Result<u64> {
        if self.device.plan_generation.load(Ordering::Acquire) != self.generation {
            return Err(Error::Invalid("phase-region intent changed before admission"));
        }
        Ok(self.generation)
    }
}

impl Plan {
    pub fn new(ordinal: i32, definitions: Vec<(String, Vec<Arc<Region>>)>) -> Result<Arc<Self>> {
        if definitions.is_empty() {
            return Err(Error::Invalid("phase plan is empty"));
        }
        let mut names = BTreeSet::new();
        let mut phases = Vec::with_capacity(definitions.len());
        for (name, regions) in definitions {
            if name.is_empty() || !names.insert(name.clone()) {
                return Err(Error::Invalid("phase plan names must be nonempty and unique"));
            }
            let mut phase = Phase { name, regions: Vec::new(), ids: BTreeSet::new(), bytes: 0 };
            for region in regions {
                if region.device() != ordinal {
                    return Err(Error::Invalid("phase plan regions must belong to this device"));
                }
                if phase.ids.insert(region.id()) {
                    phase.bytes += region.rounded;
                    phase.regions.push(region);
                }
            }
            phases.push(phase);
        }
        let mut seen = BTreeSet::new();
        let weights = phases.iter().flat_map(|phase| &phase.regions)
            .filter(|region| seen.insert(region.id())).map(|region| region.rounded).sum();
        Ok(Arc::new(Self {
            id: crate::next_identity()?,
            device: crate::gpu(ordinal)?,
            pid: std::process::id(),
            phases,
            weights,
        }))
    }

    pub fn device(&self) -> i32 {
        self.device.ordinal()
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

    /// No allocation, pin, fill or sample.
    pub fn activate(self: &Arc<Self>, name: &str) -> Result<()> {
        let phase = self.phases.iter().position(|phase| phase.name == name)
            .ok_or(Error::Invalid("unknown phase in plan"))?;
        crate::gate(self.device())?.not_capturing()?;
        let mut active = self.device.plan.lock().map_err(|_| Error::Poisoned("plan mutex".into()))?;
        if let Some(prior) = active.as_ref() {
            if prior.id != self.id && prior.plan.strong_count() != 0 {
                return Err(Error::Invalid("another phase plan is active on this GPU"));
            }
            if prior.id == self.id && prior.phase == phase {
                return Ok(());
            }
        }
        *active = Some(Active { id: self.id, plan: Arc::downgrade(self), phase });
        self.device.plan_generation.fetch_add(1, Ordering::AcqRel);
        drop(active);
        // A new plan or phase is the boundary: what ran before it is no longer in its loop.
        crate::gate(self.device())?.not_capturing()?;
        self.device.enter_plan_phase(Some(&self.phases[phase].name))?;
        self.device.entered_plan(self.weights);
        Ok(())
    }

    pub fn deactivate(&self) -> Result<()> {
        let mut active = self.device.plan.lock().map_err(|_| Error::Poisoned("plan mutex".into()))?;
        if active.as_ref().is_some_and(|active| active.id == self.id) {
            *active = None;
            self.device.plan_generation.fetch_add(1, Ordering::AcqRel);
            drop(active);
            let _ = self.device.enter_plan_phase(None); // Metadata: a poisoned table keeps its own.
        }
        Ok(())
    }
}

impl Drop for Plan {
    fn drop(&mut self) {
        if self.pid == std::process::id() {
            let _ = self.deactivate();
        }
    }
}

/// Caller holds the gate; the table is not locked.
pub(crate) fn snapshot(lease: &Lease) -> Result<Option<Snapshot>> {
    lease.validate()?;
    let Some((plan, index)) = &lease.active else {
        return Ok(None);
    };
    let phase = &plan.phases[*index];
    let state = lease.device.lock()?;
    let (mut missing, mut outside_reclaimable) = (0usize, 0usize);
    for id in &phase.ids {
        let entry = state.entries.get(id).ok_or(Error::Invalid("planned region is missing from the table"))?;
        if entry.uses.state() == Contents::Retired {
            return Err(Error::Invalid("planned region is retired"));
        }
        if entry.handle.is_none() {
            missing += entry.bytes;
        }
    }
    for (id, entry) in &state.entries {
        if !phase.ids.contains(id) && entry.idle() && lease.device.completion(&state, *id) == Completion::Done {
            outside_reclaimable += entry.bytes;
        }
    }
    Ok(Some(Snapshot { name: phase.name.clone(), bytes: phase.bytes, missing, outside_reclaimable }))
}

/// Next use of each planned region: phases ahead of the active one, then position within it.
/// Plain data, so nothing here can run a final Region drop under a gate.
#[derive(Default)]
pub(crate) struct Order {
    next: BTreeMap<u64, (u64, u64)>,
    // For the active phase's regions: their first use in a later phase.
    later: BTreeMap<u64, (u64, u64)>,
    // Phase names, from the active one.
    names: Vec<String>,
}

impl Order {
    pub(crate) fn next_use(&self, id: u64) -> Option<(u64, u64)> {
        self.next.get(&id).copied()
    }
    pub(crate) fn planned(&self) -> bool {
        !self.next.is_empty()
    }
    /// An active phase's region's first use in a later phase.
    pub(crate) fn later_use(&self, id: u64) -> Option<(u64, u64)> {
        self.later.get(&id).copied()
    }
    /// The phase `ahead` of the active one.
    pub(crate) fn name(&self, ahead: u64) -> Option<&str> {
        self.names.get(ahead as usize).map(String::as_str)
    }
}

/// The Region after `id` in the active phase, or the next phase's first: what the plan says
/// comes next. Call with no lock held.
pub(crate) fn after(ordinal: i32, id: u64) -> Result<Option<Arc<Region>>> {
    let lease = Lease::acquire(ordinal)?;
    let Some((plan, active)) = &lease.active else { return Ok(None) };
    let regions = &plan.phases[*active].regions;
    let Some(at) = regions.iter().position(|region| region.id() == id) else { return Ok(None) };
    let next = regions.get(at + 1).or_else(|| plan.phases.get(active + 1)?.regions.first());
    Ok(next.cloned())
}

/// Call before taking the gate.
pub(crate) fn order(ordinal: i32) -> Result<Order> {
    let lease = Lease::acquire(ordinal)?;
    let mut order = Order::default();
    if let Some((plan, active)) = &lease.active {
        for ahead in 0..plan.phases.len() {
            let phase = &plan.phases[(active + ahead) % plan.phases.len()];
            order.names.push(phase.name.clone());
            for (position, region) in phase.regions.iter().enumerate() {
                let at = (ahead as u64, position as u64);
                if ahead != 0 && order.next.get(&region.id()).is_some_and(|(phases, _)| *phases == 0) {
                    order.later.entry(region.id()).or_insert(at);
                }
                order.next.entry(region.id()).or_insert(at);
            }
        }
    }
    Ok(order)
}
