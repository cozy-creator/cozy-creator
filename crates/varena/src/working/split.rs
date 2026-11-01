//! A copied menu for one closed leaf operation. Geometry/execution stay with the consumer.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Choice {
    id: String,
    allocations: Vec<usize>,
    backing: usize,
    classes: BTreeMap<usize, usize>,
}
#[derive(Clone, Debug, Default)]
struct Sample {
    known: bool,
    peak: usize,
    backing: usize,
    successful: u64,
    aborted: u64,
    classes: BTreeMap<usize, usize>,
}
struct Site {
    choices: Vec<Choice>,
    preferred: String,
    samples: BTreeMap<String, Sample>,
}
struct Active {
    id: u64,
    key: String,
    choice: String,
    owner: Option<u64>,
    stream: u64,
    epoch: u64,
    allocations: BTreeSet<u64>,
    peak: usize,
    backing: usize,
    invalid: Option<String>,
    classes: BTreeMap<usize, usize>,
}
#[derive(Default)]
pub(super) struct Splits {
    next: u64,
    active: Option<Box<Active>>,
    sites: BTreeMap<String, Site>,
    pending: Vec<(u64, SplitObservation)>,
}

impl Splits {
    pub(super) fn active_is_present(&self) -> bool {
        self.active.is_some()
    }
}

pub(super) fn matches_active(state: &State, key: &str, choice: &str, phase: u64) -> bool {
    state.splits.active.as_ref().is_some_and(|active| {
        active.owner == Some(phase) && active.key == key && active.choice == choice
    })
}

pub(super) fn validate_committed(
    owner: &Working, key: &str, choices: Vec<(String, Vec<usize>)>, preferred: &str,
) -> Result<()> {
    register(owner, key, choices, preferred)
}

fn committed_operation(
    state: &State, key: &str, choice: &str, completion_owner: Option<u64>,
) -> Result<()> {
    if let Some(phase) = &state.phases.active
        && let Some((expected_key, expected_choice)) = &phase.committed_site
        && (phase.invalid.is_some() || expected_key != key || expected_choice != choice
            || completion_owner != Some(phase.token))
    {
        return Err(Error::Invalid("operation differs from its committed execution"));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SplitContext {
    pub eligible: bool,
    pub reason: &'static str,
}

/// Scope eligibility only. No driver initialization/query and no byte policy crosses to Python.
pub fn split_context(device: i32) -> Result<SplitContext> {
    let captures = crate::capture_counts()?;
    if let Some(activity) = captures.get(&device) {
        if activity.captures.load(Ordering::Acquire) != 0 {
            return Ok(SplitContext {
                eligible: false,
                reason: "capture_active",
            });
        }
        if activity.replays.load(Ordering::Acquire) != 0 {
            return Ok(SplitContext {
                eligible: false,
                reason: "replay_active",
            });
        }
    }
    if let Some(owner) = existing(device)? {
        let state = owner.lock()?;
        if state.splits.active.is_some() {
            return Ok(SplitContext {
                eligible: false,
                reason: "leaf_active",
            });
        }
        if state
            .phases
            .active
            .as_ref()
            .is_some_and(|phase| phase.mode != PhaseMode::Eager)
        {
            return Ok(SplitContext {
                eligible: false,
                reason: "non_eager_phase",
            });
        }
    }
    Ok(SplitContext {
        eligible: true,
        reason: "",
    })
}

fn context(device: i32) -> Result<()> {
    let current = split_context(device)?;
    if current.eligible {
        Ok(())
    } else {
        Err(Error::Invalid(current.reason))
    }
}

#[derive(Clone, Debug)]
pub struct SplitSelection {
    pub stream: u64,
    pub allocation_backing_bytes: usize,
    pub measured_backing_peak_bytes: Option<usize>,
    pub reused_capacity_bytes: usize,
    pub preserved_backing_bytes: usize,
    pub reclaimable_working_bytes: usize,
    pub choice_id: String,
    pub eligible: Vec<String>,
    pub required_bytes: usize,
    pub available_bytes: usize,
    pub basis: &'static str,
    pub reclaimed_working_bytes: usize,
    pub reclaimed_weight_bytes: usize,
    pub plan_generation: u64,
    pub phase: Option<String>,
    pub phase_weight_bytes: usize,
    pub phase_missing_bytes: usize,
    pub phase_outside_reclaimable_bytes: usize,
    pub preserves_phase_weights: bool,
}
#[derive(Clone, Debug)]
pub struct SplitObservation {
    pub observation_id: u64,
    pub key: String,
    pub choice_id: String,
    pub peak_bytes: usize,
    pub backing_peak_bytes: usize,
    pub observed: bool,
    pub pending: bool,
    pub published: bool,
    pub reason: String,
    pub successful_passes: u64,
    classes: BTreeMap<usize, usize>,
}

fn classes(owner: &Working, allocations: &[usize]) -> Result<BTreeMap<usize, usize>> {
    let mut classes = BTreeMap::new();
    for &bytes in allocations.iter().filter(|&&bytes| bytes != 0) {
        *classes.entry(size_class(bytes, owner.page)?).or_default() += 1;
    }
    Ok(classes)
}

fn backing(owner: &Working, classes: &BTreeMap<usize, usize>) -> Result<usize> {
    classes.iter().try_fold(0usize, |total, (&class, &count)| {
        let bytes = if class <= 1 << 20 {
            count.div_ceil(owner.page / class).checked_mul(owner.page)
        } else {
            count.checked_mul(class)
        };
        bytes
            .and_then(|bytes| total.checked_add(bytes))
            .ok_or(Error::Invalid("split allocation declaration overflows"))
    })
}

fn register(
    owner: &Working,
    key: &str,
    declarations: Vec<(String, Vec<usize>)>,
    preferred: &str,
) -> Result<()> {
    if key.is_empty() || declarations.is_empty() || preferred.is_empty() {
        return Err(Error::Invalid(
            "split needs a nonempty configuration and legal menu",
        ));
    }
    let mut ids = BTreeSet::new();
    let choices = declarations
        .into_iter()
        .map(|(id, allocations)| {
            if id.is_empty() || !ids.insert(id.clone()) {
                return Err(Error::Invalid(
                    "split choice IDs must be nonempty and distinct",
                ));
            }
            let classes = classes(owner, &allocations)?;
            Ok(Choice {
                backing: backing(owner, &classes)?,
                id,
                allocations,
                classes,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if !ids.contains(preferred) {
        return Err(Error::Invalid("preferred split is not declared"));
    }
    let mut state = owner.lock()?;
    if state.splits.active.is_some() {
        return Err(Error::Invalid("native split sites cannot nest"));
    }
    if let Some(site) = state.splits.sites.get(key) {
        if site.choices != choices || site.preferred != preferred {
            return Err(Error::Invalid(
                "split configuration changed its copied legal menu",
            ));
        }
    } else {
        state.splits.sites.insert(
            key.to_owned(),
            Site {
                choices,
                preferred: preferred.to_owned(),
                samples: BTreeMap::new(),
            },
        );
    }
    Ok(())
}

struct Fit {
    id: String,
    basis: &'static str,
    full: usize,
    measured: Option<usize>,
    missing: usize,
    reused: usize,
    preserve: BTreeSet<u64>,
    preserved_bytes: usize,
    reclaimable: usize,
    now: usize,
    after_idle: usize,
    generation: u64,
    phase: Option<crate::plan::Snapshot>,
    after_outside: usize,
    phase_available: Option<usize>,
}
impl Fit {
    fn fits(&self) -> bool {
        self.missing <= self.after_idle
    }
    fn preserves_phase(&self) -> bool {
        self.phase_available
            .is_some_and(|available| self.missing <= available)
    }
}

fn headroom(owner: &Working, free: usize, released: usize) -> usize {
    let owned = owner
        .room
        .pages
        .load(Ordering::Acquire)
        .saturating_add(owner.room.working.load(Ordering::Acquire));
    let limit = owner
        .room
        .limit
        .load(Ordering::Acquire)
        .min(owner.room.ceiling.load(Ordering::Acquire));
    free.saturating_add(released)
        .min(limit.saturating_sub(owned.saturating_sub(released)))
}

fn fits(
    owner: &Working,
    state: &State,
    key: &str,
    stream: u64,
    free: usize,
    generation: u64,
    phase: Option<&crate::plan::Snapshot>,
) -> Result<Vec<Fit>> {
    let site = state
        .splits
        .sites
        .get(key)
        .ok_or(Error::Invalid("unknown split configuration"))?;
    site.choices
        .iter()
        .map(|choice| {
            let sample = site.samples.get(&choice.id).filter(|sample| sample.known);
            let classes = sample.map_or(&choice.classes, |sample| &sample.classes);
            let full = if sample.is_none() {
                choice.backing
            } else {
                backing(owner, classes)?
            };
            let mut missing = classes.clone();
            let mut used = BTreeSet::new();
            let mut preserve = BTreeSet::new();
            let mut reused = 0usize;
            for (&class, count) in &mut missing {
                for origin in [Some(stream), None] {
                    let Some(slots) = state.free.get(&(None, class, origin)) else {
                        continue;
                    };
                    for ptr in slots.iter().rev() {
                        if *count == 0 {
                            break;
                        }
                        if used.contains(ptr) {
                            continue;
                        }
                        let Some(block) = state.blocks.get(ptr) else {
                            continue;
                        };
                        if block.capacity != class
                            || !block.reusable_from(origin)
                            || state.segments[&block.segment].pool.is_some()
                        {
                            continue;
                        }
                        used.insert(*ptr);
                        preserve.insert(block.segment);
                        reused += class;
                        *count -= 1;
                    }
                }
            }
            let missing = backing(owner, &missing)?;
            let preserved_bytes = preserve.iter().map(|base| state.segments[base].bytes).sum();
            let reclaimable: usize = state
                .segments
                .iter()
                .filter(|(base, segment)| {
                    segment.pool.is_none()
                        && !preserve.contains(base)
                        && state
                            .blocks
                            .range(**base..**base + segment.bytes as u64)
                            .all(|(_, block)| {
                                block.life == Life::Ready && block.reusable_on.is_none()
                            })
                })
                .map(|(_, segment)| segment.bytes)
                .sum();
            let after_outside = headroom(
                owner,
                free,
                reclaimable.saturating_add(phase.map_or(0, |intent| intent.outside_reclaimable)),
            );
            Ok(Fit {
                id: choice.id.clone(),
                basis: if sample.is_some() {
                    "measured"
                } else {
                    "declared"
                },
                full,
                measured: sample.map(|sample| sample.backing),
                missing,
                reused,
                preserve,
                preserved_bytes,
                reclaimable,
                now: headroom(owner, free, 0),
                after_idle: headroom(owner, free, reclaimable),
                generation,
                phase: phase.cloned(),
                after_outside,
                phase_available: phase.and_then(|intent| after_outside.checked_sub(intent.missing)),
            })
        })
        .collect()
}

fn snapshot(device: i32, owner: &Working, key: &str, stream: u64) -> Result<Vec<Fit>> {
    // GPU-owning leases precede every guard, including on a rejected generation.
    let lease = crate::plan::Lease::acquire(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let generation = lease.validate()?;
    let phase = crate::plan::snapshot(&lease)?;
    let state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let (free, _) = owner.driver.memory_info()?;
    fits(owner, &state, key, stream, free, generation, phase.as_ref())
}

fn decision(
    plans: &[Fit],
    stream: u64,
    preferred: Option<&str>,
    working: usize,
    weights: usize,
) -> Option<SplitSelection> {
    let eligible: Vec<_> = plans
        .iter()
        .filter(|plan| plan.fits() || plan.preserves_phase())
        .collect();
    // Menu order is largest to smallest. Phase residency wins before a streaming
    // preference; when no choice can preserve it, the existing actual-capacity
    // selection remains. No efficient chunk is invented by this memory layer.
    let selected = eligible
        .iter()
        .find(|plan| plan.preserves_phase())
        .copied()
        .or_else(|| preferred.and_then(|id| eligible.iter().find(|plan| plan.id == id).copied()))
        .or_else(|| eligible.first().copied())?;
    Some(SplitSelection {
        stream,
        choice_id: selected.id.clone(),
        eligible: eligible.iter().map(|plan| plan.id.clone()).collect(),
        required_bytes: selected.missing,
        available_bytes: if selected.preserves_phase() {
            selected.after_outside
        } else {
            selected.after_idle
        },
        allocation_backing_bytes: selected.full,
        measured_backing_peak_bytes: selected.measured,
        reused_capacity_bytes: selected.reused,
        preserved_backing_bytes: selected.preserved_bytes,
        reclaimable_working_bytes: selected.reclaimable,
        basis: selected.basis,
        reclaimed_working_bytes: working,
        reclaimed_weight_bytes: weights,
        plan_generation: selected.generation,
        phase: selected.phase.as_ref().map(|p| p.name.clone()),
        phase_weight_bytes: selected.phase.as_ref().map_or(0, |p| p.bytes),
        phase_missing_bytes: selected.phase.as_ref().map_or(0, |p| p.missing),
        phase_outside_reclaimable_bytes: selected
            .phase
            .as_ref()
            .map_or(0, |p| p.outside_reclaimable),
        preserves_phase_weights: selected.preserves_phase(),
    })
}

fn trim_for(device: i32, owner: &Working, key: &str, choice: &str, stream: u64) -> Result<usize> {
    let lease = crate::plan::Lease::acquire(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let generation = lease.validate()?;
    let phase = crate::plan::snapshot(&lease)?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let result = (|| {
        owner.poll(&mut state)?;
        let (free, _) = owner.driver.memory_info()?;
        let plans = fits(owner, &state, key, stream, free, generation, phase.as_ref())?;
        let plan = plans
            .iter()
            .find(|plan| plan.id == choice)
            .ok_or(Error::Invalid("split choice is not declared"))?;
        let needed = plan.missing.saturating_sub(plan.now);
        owner.trim_ready(&mut state, needed, None, &plan.preserve)
    })();
    result.map_err(|error| owner.fail(&mut state, error))
}

fn exhausted(plans: &[Fit]) -> Error {
    let plan = plans
        .iter()
        .min_by_key(|plan| plan.missing.saturating_sub(plan.after_idle))
        .expect("copied menu");
    Error::Exhausted {
        requested_bytes: plan.missing,
        available_bytes: plan.after_idle,
    }
}

/// Evaluate exact allocation classes on the supplied execution stream. Compatible slots
/// satisfy requests once; global pending totals and unrelated classes never provide credit.
pub fn split_select(
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    preferred: &str,
    stream: u64,
) -> Result<SplitSelection> {
    let result = split_select_inner(device, key, choices, preferred, stream);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "split_select_error");
    }
    result
}

fn split_select_inner(
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    preferred: &str,
    stream: u64,
) -> Result<SplitSelection> {
    valid_stream(stream)?;
    context(device)?;
    let owner = initialize_owner(device)?;
    if owner.lock()?.phases.active.as_ref().is_some_and(|phase| phase.committed_site.is_some()) {
        return Err(Error::Invalid("a committed execution cannot select another choice"));
    }
    register(&owner, key, choices, preferred)?;
    let plans = snapshot(device, &owner, key, stream)?;
    if let Some(choice) = decision(&plans, stream, None, 0, 0) {
        return Ok(choice);
    }
    let working = trim_for(device, &owner, key, preferred, stream)?;
    let plans = snapshot(device, &owner, key, stream)?;
    if let Some(choice) = decision(&plans, stream, None, working, 0) {
        return Ok(choice);
    }
    let preferred_plan = plans
        .iter()
        .find(|plan| plan.id == preferred)
        .expect("copied preferred");
    let needed = preferred_plan
        .missing
        .saturating_sub(preferred_plan.after_idle);
    let weights = crate::reclaim(device, needed)?.reclaimed_bytes;
    let plans = snapshot(device, &owner, key, stream)?;
    decision(&plans, stream, Some(preferred), working, weights).ok_or_else(|| exhausted(&plans))
}

/// Re-evaluate the exact agreed geometry and stream immediately before opening the leaf.
/// Matched compatible cache stays mapped; only unmatched completed backing may be trimmed.
pub fn split_begin(
    device: i32,
    key: &str,
    choice_id: &str,
    completion_owner: Option<u64>,
    stream: u64,
    plan_generation: Option<u64>,
) -> Result<u64> {
    let result = split_begin_inner(
        device,
        key,
        choice_id,
        completion_owner,
        stream,
        plan_generation,
    );
    if result.is_err() {
        let _ = invalidate_current_phase(device, "split_begin_error");
    }
    result
}

fn selected_generation(expected: Option<u64>, current: u64) -> Result<()> {
    if expected == Some(current) || (expected.is_none() && current == 0) {
        return Ok(());
    }
    Err(Error::Invalid(
        "split begin requires its unchanged selected phase-plan generation",
    ))
}

fn split_begin_inner(
    device: i32,
    key: &str,
    choice_id: &str,
    completion_owner: Option<u64>,
    stream: u64,
    plan_generation: Option<u64>,
) -> Result<u64> {
    valid_stream(stream)?;
    context(device)?;
    let owner = owner(device)?;
    {
        let state = owner.lock()?;
        committed_operation(&state, key, choice_id, completion_owner)?;
        if let Some(id) = completion_owner {
            if !state
                .phases
                .active
                .as_ref()
                .is_some_and(|phase| phase.token == id && phase.mode == PhaseMode::Eager)
            {
                return Err(Error::Invalid(
                    "split completion owner is not the active eager phase",
                ));
            }
        }
    }
    let plans = snapshot(device, &owner, key, stream)?;
    let plan = plans
        .iter()
        .find(|plan| plan.id == choice_id)
        .ok_or(Error::Invalid("split choice is not declared"))?;
    selected_generation(plan_generation, plan.generation)?;
    if plan.missing > plan.now {
        trim_for(device, &owner, key, choice_id, stream)?;
        let plans = snapshot(device, &owner, key, stream)?;
        let plan = plans
            .iter()
            .find(|plan| plan.id == choice_id)
            .expect("copied choice");
        if plan.missing > plan.now && plan.preserves_phase() {
            crate::plan::reclaim_outside(device, plan.missing - plan.now)?;
        }
        let plans = snapshot(device, &owner, key, stream)?;
        let plan = plans
            .iter()
            .find(|plan| plan.id == choice_id)
            .expect("copied choice");
        if plan.missing > plan.now {
            crate::reclaim(device, plan.missing - plan.now)?;
        }
    }
    // Keep GPU-owning leases outside and longer-lived than every exclusion guard.
    let lease = crate::plan::Lease::acquire(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let generation = lease.validate()?;
    let phase = crate::plan::snapshot(&lease)?;
    let mut state = owner.lock()?;
    committed_operation(&state, key, choice_id, completion_owner)?;
    selected_generation(plan_generation, generation)?;
    let _context = owner.driver.enter()?;
    let (free, _) = owner.driver.memory_info()?;
    let plans = fits(
        &owner,
        &state,
        key,
        stream,
        free,
        generation,
        phase.as_ref(),
    )?;
    let plan = plans
        .iter()
        .find(|plan| plan.id == choice_id)
        .ok_or(Error::Invalid("split choice is not declared"))?;
    if plan.missing > plan.now {
        return Err(Error::Exhausted {
            requested_bytes: plan.missing,
            available_bytes: plan.now,
        });
    }
    if state.splits.active.is_some() {
        return Err(Error::Invalid("native split sites cannot nest"));
    }
    if let Some(id) = completion_owner {
        if !state
            .phases
            .active
            .as_ref()
            .is_some_and(|phase| phase.token == id && phase.mode == PhaseMode::Eager)
        {
            return Err(Error::Invalid(
                "split completion owner changed before execution",
            ));
        }
    }
    state.splits.next = state
        .splits
        .next
        .checked_add(1)
        .ok_or(Error::Invalid("split observation overflow"))?;
    let id = state.splits.next;
    state.splits.active = Some(Box::new(Active {
        id,
        key: key.to_owned(),
        choice: choice_id.to_owned(),
        owner: completion_owner,
        stream,
        epoch: captures
            .get(&device)
            .map_or(0, |activity| activity.activity_epoch),
        allocations: BTreeSet::new(),
        peak: 0,
        backing: 0,
        invalid: None,
        classes: BTreeMap::new(),
    }));
    Ok(id)
}

pub(super) fn allocated(state: &mut State, ptr: u64) {
    if let Some(active) = &mut state.splits.active {
        if active.owner.is_some() {
            active.allocations.insert(ptr);
            if state.blocks[&ptr].stream != active.stream {
                active
                    .invalid
                    .get_or_insert_with(|| "split_allocation_stream_changed".into());
                state.phases.invalidate("split_allocation_stream_changed");
            }
        }
    }
    refresh(state);
}

pub(super) fn refresh(state: &mut State) {
    let Some(active) = &mut state.splits.active else {
        return;
    };
    if active.owner.is_none() {
        return;
    }
    let mut bytes = 0usize;
    let mut segments = BTreeSet::new();
    let mut classes = BTreeMap::<usize, usize>::new();
    for ptr in &active.allocations {
        if let Some(block) = state.blocks.get(ptr).filter(|block| block.charge() != 0) {
            bytes += block.capacity;
            *classes.entry(block.capacity).or_default() += 1;
            segments.insert(block.segment);
        }
    }
    for (class, count) in classes {
        let peak = active.classes.entry(class).or_default();
        *peak = (*peak).max(count);
    }
    active.peak = active.peak.max(bytes);
    active.backing = active.backing.max(
        segments
            .into_iter()
            .map(|segment| state.segments[&segment].bytes)
            .sum(),
    );
}

pub(super) fn invalidate(state: &mut State, reason: &str) {
    if let Some(active) = &mut state.splits.active {
        active.invalid.get_or_insert_with(|| reason.to_owned());
    }
}

/// Host submission is not completion. Queue eligible candidates under the explicit phase
/// owner; phase_leave's already-proven completion publishes them without a per-leaf sync.
pub fn split_end(device: i32, observation_id: u64, submitted: bool) -> Result<SplitObservation> {
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    let mut state = owner
        .state
        .lock()
        .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
    if !state
        .splits
        .active
        .as_ref()
        .is_some_and(|active| active.id == observation_id)
    {
        return Err(Error::Invalid("unknown split observation"));
    }
    refresh(&mut state);
    let active = state
        .splits
        .active
        .take()
        .expect("checked split observation");
    let parent = active.owner.filter(|id| {
        state
            .phases
            .active
            .as_ref()
            .is_some_and(|phase| phase.token == *id && phase.mode == PhaseMode::Eager)
    });
    let reason = active.invalid.clone().unwrap_or_else(|| {
        if !submitted {
            "operation_not_submitted".into()
        } else if state.poison.is_some() {
            "allocator_poisoned".into()
        } else if active.epoch
            != captures
                .get(&device)
                .map_or(0, |activity| activity.activity_epoch)
        {
            "capture_or_replay_attempt".into()
        } else if parent.is_none() {
            "completion_owner_missing".into()
        } else {
            String::new()
        }
    });
    let row = SplitObservation {
        observation_id,
        key: active.key,
        choice_id: active.choice,
        peak_bytes: active.peak,
        backing_peak_bytes: active.backing,
        observed: active.owner.is_some(),
        pending: parent.is_some(),
        published: false,
        reason,
        successful_passes: 0,
        classes: active.classes,
    };
    if let Some(parent) = parent {
        if !submitted {
            state.phases.invalidate("split_operation_failed");
        }
        state.splits.pending.push((parent, row.clone()));
    }
    Ok(row)
}

pub(super) fn open_for(state: &State, parent: u64) -> bool {
    state
        .splits
        .active
        .as_ref()
        .is_some_and(|active| active.owner == Some(parent))
}

pub(super) fn phase_finished(
    state: &mut State,
    parent: u64,
    completed: bool,
    reason: &str,
) -> Vec<SplitObservation> {
    if open_for(state, parent) {
        refresh(state);
        let active = state.splits.active.take().expect("open split");
        state.splits.pending.push((
            parent,
            SplitObservation {
                observation_id: active.id,
                key: active.key,
                choice_id: active.choice,
                peak_bytes: active.peak,
                backing_peak_bytes: active.backing,
                observed: true,
                pending: true,
                published: false,
                reason: "split_interval_open".into(),
                successful_passes: 0,
                classes: active.classes,
            },
        ));
    }
    let mut kept = Vec::new();
    let mut result = Vec::new();
    for (owner, mut row) in std::mem::take(&mut state.splits.pending) {
        if owner != parent {
            kept.push((owner, row));
            continue;
        }
        row.pending = false;
        if !completed && row.reason.is_empty() {
            row.reason = if reason.is_empty() {
                "phase_aborted"
            } else {
                reason
            }
            .to_owned();
        }
        row.published = completed && row.reason.is_empty();
        if let Some(site) = state.splits.sites.get_mut(&row.key) {
            let sample = site.samples.entry(row.choice_id.clone()).or_default();
            if row.published {
                sample.known = true;
                sample.peak = row.peak_bytes;
                sample.backing = row.backing_peak_bytes;
                sample.classes = row.classes.clone();
                sample.successful += 1;
            } else {
                sample.aborted += 1;
            }
            row.successful_passes = sample.successful;
        } else {
            row.published = false;
            row.reason = "split_owner_retired".into();
        }
        result.push(row);
    }
    state.splits.pending = kept;
    result
}

pub(super) fn forget(state: &mut State, key: &str) -> Result<bool> {
    if state
        .splits
        .active
        .as_ref()
        .is_some_and(|active| active.key == key)
        || state.splits.pending.iter().any(|(_, row)| row.key == key)
    {
        return Err(Error::Busy);
    }
    Ok(state.splits.sites.remove(key).is_some())
}
