//! A copied menu for one closed leaf operation. Geometry/execution stay with the consumer.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Choice {
    id: String,
    allocations: Vec<usize>,
    backing: usize,
}
#[derive(Clone, Debug, Default)]
struct Sample {
    known: bool,
    peak: usize,
    backing: usize,
    successful: u64,
    aborted: u64,
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
    epoch: u64,
    allocations: BTreeSet<u64>,
    peak: usize,
    backing: usize,
    invalid: Option<String>,
}
#[derive(Default)]
pub(super) struct Splits {
    next: u64,
    active: Option<Box<Active>>,
    sites: BTreeMap<String, Site>,
    pending: Vec<(u64, SplitObservation)>,
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
    pub choice_id: String,
    pub eligible: Vec<String>,
    pub required_bytes: usize,
    pub available_bytes: usize,
    pub basis: &'static str,
    pub reclaimed_working_bytes: usize,
    pub reclaimed_weight_bytes: usize,
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
}

fn backing(owner: &Working, allocations: &[usize]) -> Result<usize> {
    let mut classes = BTreeMap::<usize, usize>::new();
    for &bytes in allocations.iter().filter(|&&bytes| bytes != 0) {
        let class = size_class(bytes, owner.page)?;
        *classes.entry(class).or_default() += 1;
    }
    classes
        .into_iter()
        .try_fold(0usize, |total, (class, count)| {
            let bytes = if class <= 1 << 20 {
                // Use actual slab slots, including its unassigned tail; byte-sum rounding can
                // undercount when a class does not divide the 2 MiB slab evenly.
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
            Ok(Choice {
                backing: backing(owner, &allocations)?,
                id,
                allocations,
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

fn requirements(owner: &Working, key: &str) -> Result<Vec<(String, usize, &'static str)>> {
    let state = owner.lock()?;
    let site = state
        .splits
        .sites
        .get(key)
        .ok_or(Error::Invalid("unknown split configuration"))?;
    Ok(site
        .choices
        .iter()
        .map(
            |choice| match site.samples.get(&choice.id).filter(|sample| sample.known) {
                Some(sample) => (choice.id.clone(), sample.backing, "measured"),
                None => (choice.id.clone(), choice.backing, "declared"),
            },
        )
        .collect())
}

fn available(device: i32, owner: &Working, include_completed_cache: bool) -> Result<usize> {
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let (free, _) = owner.driver.memory_info()?;
    let reusable = if include_completed_cache {
        state.reusable_eager_pages()
    } else {
        0
    };
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
    let managed = limit.saturating_sub(owned.saturating_sub(reusable));
    Ok(free.saturating_add(reusable).min(managed))
}

fn decision(
    requirements: &[(String, usize, &'static str)],
    available: usize,
    preferred: Option<&str>,
    working: usize,
    weights: usize,
) -> Option<SplitSelection> {
    let eligible: Vec<_> = requirements
        .iter()
        .filter(|(_, bytes, _)| *bytes <= available)
        .collect();
    let selected = preferred
        .and_then(|preferred| eligible.iter().find(|(id, _, _)| id == preferred).copied())
        .or_else(|| eligible.first().copied())?;
    Some(SplitSelection {
        choice_id: selected.0.clone(),
        eligible: eligible.iter().map(|row| row.0.clone()).collect(),
        required_bytes: selected.1,
        available_bytes: available,
        basis: selected.2,
        reclaimed_working_bytes: working,
        reclaimed_weight_bytes: weights,
    })
}

/// Select without executing probes. A successful sample replaces only its exact choice's
/// complete initial allocation declaration, never another geometry or an inferred curve.
pub fn split_select(
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    preferred: &str,
) -> Result<SplitSelection> {
    context(device)?;
    let owner = initialize_owner(device)?;
    register(&owner, key, choices, preferred)?;
    let needs = requirements(&owner, key)?;
    if let Some(choice) = decision(&needs, available(device, &owner, true)?, None, 0, 0) {
        return Ok(choice);
    }
    let working = trim(device)?;
    if let Some(choice) = decision(&needs, available(device, &owner, true)?, None, working, 0) {
        return Ok(choice);
    }
    let wanted = needs
        .iter()
        .find(|(id, _, _)| id == preferred)
        .expect("validated preferred")
        .1;
    let before = available(device, &owner, true)?;
    let weights = crate::reclaim(device, wanted.saturating_sub(before))?.reclaimed_bytes;
    let now = available(device, &owner, true)?;
    decision(&needs, now, Some(preferred), working, weights).ok_or(Error::Exhausted {
        requested_bytes: needs
            .iter()
            .map(|(_, bytes, _)| *bytes)
            .min()
            .expect("nonempty menu"),
        available_bytes: now,
    })
}

/// The agreed ID is immutable. Recheck its capacity now; a foreign race may still exhaust
/// a later real allocation, which fails this invocation rather than executing a new choice.
pub fn split_begin(
    device: i32,
    key: &str,
    choice_id: &str,
    completion_owner: Option<u64>,
) -> Result<u64> {
    context(device)?;
    let owner = owner(device)?;
    let needs = requirements(&owner, key)?;
    let required = needs
        .iter()
        .find(|(id, _, _)| id == choice_id)
        .ok_or(Error::Invalid("split choice is not declared"))?
        .1;
    {
        let state = owner.lock()?;
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
                    "split completion owner is not the active eager phase",
                ));
            }
        }
    }
    // Realize required free space before starting a partly mutating operation. Idle working
    // backing precedes idle weights; no request/operator execution is retried here.
    if available(device, &owner, false)? < required {
        trim(device)?;
        let free = available(device, &owner, false)?;
        if free < required {
            crate::reclaim(device, required - free)?;
        }
    }
    let free = available(device, &owner, true)?;
    if free < required {
        return Err(Error::Exhausted {
            requested_bytes: required,
            available_bytes: free,
        });
    }
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let mut state = owner.lock()?;
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
        epoch: captures
            .get(&device)
            .map_or(0, |activity| activity.activity_epoch),
        allocations: BTreeSet::new(),
        peak: 0,
        backing: 0,
        invalid: None,
    }));
    Ok(id)
}

pub(super) fn allocated(state: &mut State, ptr: u64) {
    if let Some(active) = &mut state.splits.active {
        if active.owner.is_some() {
            active.allocations.insert(ptr);
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
    for ptr in &active.allocations {
        if let Some(block) = state.blocks.get(ptr).filter(|block| block.charge() != 0) {
            bytes += block.capacity;
            segments.insert(block.segment);
        }
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
