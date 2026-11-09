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
    /// The least this level needs: a refused allocation in its leaf, beside what the leaf held
    /// then. A level is never chosen again under it, so a short estimate fails one call, not each.
    refused: usize,
}
struct Site {
    choices: Vec<Choice>,
    samples: BTreeMap<String, Sample>,
    /// Levels given their one try. For the process: a try that fails or measures worse stays spent.
    tried: BTreeSet<String>,
    /// Moves when a measure here changes what a decision would read.
    measures: u64,
    /// The level chosen here, kept while its room and measures hold: a loop's every step and
    /// every later call of the same program meet the same room, so they look it up.
    decided: Option<Decided>,
}
struct Decided {
    selection: SplitSelection,
    room: u64,
    measures: u64,
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
    refused: usize,
}
#[derive(Default)]
pub(super) struct Splits {
    next: u64,
    active: Option<Box<Active>>,
    sites: BTreeMap<String, Site>,
    pending: Vec<(u64, SplitObservation)>,
    /// The last closed leaf an allocation was refused in, until `split_refused` answers for it.
    refused: Option<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct SplitContext {
    pub eligible: bool,
    pub reason: &'static str,
}

/// Scope eligibility only. No driver initialization/query and no byte policy crosses to Python.
pub fn split_context(device: i32) -> Result<SplitContext> {
    let activity = crate::gpu(device)?;
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

/// What this GPU has right now, from the readings a split chooses by. A report: nothing is held.
#[derive(Clone, Copy, Debug)]
pub struct Room {
    /// The card or its software cap, less what's outside our own bytes.
    pub capacity: usize,
    pub working_live: usize,
    /// Weight bytes held for use now.
    pub pinned: usize,
    pub weights_resident: usize,
    /// What can be mapped now with no reclaim.
    pub free: usize,
}

pub fn room(device: i32) -> Result<Room> {
    let owner = initialize_owner(device)?;
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    let polled = owner.poll(&mut state);
    polled.map_err(|error| owner.fail(&mut state, error))?;
    owner.sample()?;
    let (free, total) = owner.driver().memory_info()?;
    Ok(Room {
        capacity: total.min(owner.capacity()),
        working_live: live_working(&state),
        pinned: owner.held.load(Ordering::Acquire),
        weights_resident: owner.pages.load(Ordering::Acquire),
        free: headroom(owner, free, 0),
    })
}

fn live_working(state: &State) -> usize {
    state.blocks.values().filter(|block| block.life == Life::Live && !block.scratch).map(|block| block.capacity).sum()
}

#[derive(Clone, Debug)]
pub struct SplitSelection {
    pub stream: u64,
    pub allocation_backing_bytes: usize,
    pub choice_id: String,
    pub eligible: Vec<String>,
    pub required_bytes: usize,
    pub available_bytes: usize,
    pub basis: &'static str,
    pub plan_generation: u64,
    pub phase: Option<String>,
    pub phase_weight_bytes: usize,
    pub budget_bytes: usize,
    pub capacity_bytes: usize,
    pub live_bytes: usize,
    /// This level's one try: `Menu::trial`.
    pub trying: bool,
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
    /// The bound a refused allocation in this leaf set on its level's need (0: none was refused).
    pub refused_bytes: usize,
    classes: BTreeMap<usize, usize>,
}

fn classes(owner: &Device, allocations: &[usize]) -> Result<BTreeMap<usize, usize>> {
    let mut classes = BTreeMap::new();
    for &bytes in allocations.iter().filter(|&&bytes| bytes != 0) {
        *classes.entry(size_class(bytes, owner.page())?).or_default() += 1;
    }
    Ok(classes)
}

fn backing(owner: &Device, classes: &BTreeMap<usize, usize>) -> Result<usize> {
    classes.iter().try_fold(0usize, |total, (&class, &count)| {
        let bytes = if class <= 1 << 20 {
            count.div_ceil(owner.page() / class).checked_mul(owner.page())
        } else {
            count.checked_mul(class)
        };
        bytes
            .and_then(|bytes| total.checked_add(bytes))
            .ok_or(Error::Invalid("split allocation declaration overflows"))
    })
}

fn register(
    owner: &Device,
    key: &str,
    declarations: Vec<(String, Vec<usize>)>,
) -> Result<()> {
    if key.is_empty() || declarations.is_empty() {
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
    let mut state = owner.lock()?;
    if state.splits.active.is_some() {
        return Err(Error::Invalid("native split sites cannot nest"));
    }
    if let Some(site) = state.splits.sites.get(key) {
        if site.choices != choices {
            return Err(Error::Invalid(
                "split configuration changed its copied legal menu",
            ));
        }
    } else {
        state.splits.sites.insert(
            key.to_owned(),
            Site {
                choices,
                samples: BTreeMap::new(),
                tried: BTreeSet::new(),
                measures: 0,
                decided: None,
            },
        );
    }
    Ok(())
}

struct Fit {
    id: String,
    basis: &'static str,
    tried: bool,
    full: usize,
    missing: usize,
    preserve: BTreeSet<u64>,
    now: usize,
    after_idle: usize,
    generation: u64,
    phase: Option<crate::plan::Snapshot>,
    after_outside: usize,
    /// Set only when this phase's weights fit the card: then another phase's idle weights may make
    /// room (H3 Turbo on a 4090 runs its heads 2-4 levels coarser and 20% faster for it).
    phase_available: Option<usize>,
    /// This phase's own resident weights not held now, when they exceed the card: they are read
    /// again every step whatever this level does, so taking their room only streams more. Another
    /// phase's resident weights then never make room: Qwen 4 MP's text encoder, evicted for a level
    /// no faster, was read back on the next call's critical path.
    streamed: usize,
}
impl Fit {
    fn fits(&self) -> bool {
        self.missing <= self.after_idle.saturating_add(self.streamed)
    }
    fn measured(&self) -> bool {
        self.basis == "measured"
    }
    /// What this level is compared at: its measure, or an estimate that errs toward fitting
    /// (first uses ran 10-15% over theirs).
    fn want(&self) -> usize {
        if self.measured() { self.full } else { self.full.saturating_mul(5) / 4 }
    }

    fn preserves_phase(&self) -> bool {
        self.phase_available
            .is_some_and(|available| self.missing <= available)
    }
    fn usable(&self) -> bool {
        self.fits() || self.preserves_phase()
    }
    fn short(&self) -> Error {
        Error::Exhausted { requested_bytes: self.missing, available_bytes: self.after_idle }
    }
}

fn headroom(owner: &Device, free: usize, released: usize) -> usize {
    let owned = owner.owned();
    free.saturating_add(released)
        .min(owner.capacity().saturating_sub(owned.saturating_sub(released)))
}

fn fits(
    owner: &Device,
    state: &State,
    key: &str,
    stream: u64,
    free: usize,
    generation: u64,
    phase: Option<&crate::plan::Snapshot>,
) -> Result<Vec<Fit>> {
    // Only pages whose owner released them and whose submitted fence was observed complete.
    // These are reclaimable backing, not reusable working slots or phase working demand.
    let reclaimable_cache = cache::ready_bytes(state, owner.page());
    let card = owner.capacity().min(owner.total.load(Ordering::Acquire));
    let streams = phase.is_some_and(|intent| intent.bytes > card);
    let streamed = phase.filter(|_| streams)
        .map_or(0, |intent| (intent.bytes - intent.missing).saturating_sub(owner.held.load(Ordering::Acquire)));
    let site = state
        .splits
        .sites
        .get(key)
        .ok_or(Error::Invalid("unknown split configuration"))?;
    // A level not yet run is scaled as its nearest measured sibling came out. Otherwise a
    // declaration's excess keeps a coarser level from being tried, and a finer measured one sticks.
    let measured: Vec<(usize, usize)> = site.choices.iter()
        .filter_map(|choice| {
            let sample = site.samples.get(&choice.id).filter(|sample| sample.known)?;
            Some((choice.backing.max(1), sample.peak))
        })
        .collect();
    site.choices
        .iter()
        .map(|choice| {
            let sample = site.samples.get(&choice.id).filter(|sample| sample.known);
            let nearest = measured.iter().min_by_key(|(declared, _)| declared.abs_diff(choice.backing));
            let scaled = |bytes: usize| nearest.map_or(bytes, |(declared, peak)| {
                (bytes as u128 * *peak as u128 / *declared as u128) as usize
            });
            let classes = sample.map_or(&choice.classes, |sample| &sample.classes);
            let full = sample.map_or_else(|| scaled(choice.backing), |sample| sample.peak);
            let mut missing = classes.clone();
            let mut used = BTreeSet::new();
            let mut preserve = BTreeSet::new();
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
                        *count -= 1;
                    }
                }
            }
            // What's left of the shared sizes can be cut from free runs.
            let mut runs: Vec<(usize, u64)> = state.free.range((None, LARGE + 1, None)..)
                .take_while(|((pool, ..), _)| pool.is_none())
                .filter(|((_, _, origin), _)| origin.is_none() || *origin == Some(stream))
                .flat_map(|((_, capacity, origin), slots)| slots.iter().filter_map(|ptr| {
                    let block = state.blocks.get(ptr).filter(|block| block.capacity == *capacity && block.reusable_from(*origin))?;
                    (!used.contains(ptr)).then_some((*capacity, block.segment))
                }))
                .collect();
            for (&class, count) in missing.iter_mut().rev().take_while(|(class, _)| **class > LARGE) {
                while *count != 0 {
                    let Some(run) = runs.iter_mut().filter(|run| run.0 >= class).min_by_key(|run| run.0) else {
                        break;
                    };
                    run.0 -= class;
                    preserve.insert(run.1);
                    *count -= 1;
                }
            }
            let missing = backing(owner, &missing)?;
            let missing = if sample.is_none() { scaled(missing) } else { missing };
            // A refusal's bound over the estimate is missing too.
            let short = site.samples.get(&choice.id).map_or(0, |sample| sample.refused.saturating_sub(full));
            let (full, missing) = (full + short, missing + short);
            let reclaimable: usize = state
                .segments
                .iter()
                .filter(|(base, segment)| segment.pool.is_none() && !preserve.contains(base))
                .map(|(base, segment)| state.releasable(*base, segment))
                .sum();
            let after_outside = headroom(
                owner,
                free,
                reclaimable
                    .saturating_add(reclaimable_cache)
                    .saturating_add(phase.map_or(0, |intent| intent.outside_reclaimable)),
            );
            Ok(Fit {
                id: choice.id.clone(),
                tried: site.tried.contains(&choice.id),
                basis: if sample.is_some() {
                    "measured"
                } else {
                    "declared"
                },
                full,
                missing,
                preserve,
                now: headroom(owner, free, 0),
                after_idle: headroom(owner, free, reclaimable.saturating_add(reclaimable_cache)),
                generation,
                phase: phase.cloned(),
                after_outside,
                phase_available: phase.filter(|_| !streams).and_then(|intent| after_outside.checked_sub(intent.missing)),
                streamed,
            })
        })
        .collect()
}

struct Menu {
    plans: Vec<Fit>,
    capacity: usize,
    /// What no eviction can touch: live working bytes and weights held for use.
    live: usize,
}

/// Settle completions, then fit each choice. Holds nothing on return.
fn snapshot(device: i32, owner: &Device, key: &str, stream: u64) -> Result<Menu> {
    // GPU-owning leases precede every guard, including on a rejected generation.
    let lease = crate::plan::Lease::acquire(device)?;
    let gate = crate::gate(device)?;
    gate.not_capturing()?;
    let generation = lease.validate()?;
    let phase = crate::plan::snapshot(&lease)?;
    let mut state = owner.lock()?;
    let _context = owner.driver().enter()?;
    let polled = owner.poll(&mut state);
    polled.map_err(|error| owner.fail(&mut state, error))?;
    owner.sample()?;
    let (free, total) = owner.driver().memory_info()?;
    let plans = fits(owner, &state, key, stream, free, generation, phase.as_ref())?;
    // The most any op has held this request, since this op's own weights may not be held yet; but
    // an op holds only its phase's weights, so an earlier phase's high-water isn't this one's: SDXL's
    // UNet peak squeezed its VAE decode at 6 GiB into 64 bands (run 32599).
    let (now, peak) = owner.note_held();
    let held = phase.as_ref().map_or(peak, |phase| now.max(peak.min(phase.bytes)));
    Ok(Menu { plans, capacity: total.min(owner.capacity()), live: live_working(&state) + held })
}

impl Menu {
    /// The level this operation should run at, whatever is free right now: the largest under
    /// what the card leaves beside the phase's weights (at least a quarter of it), or beside what's
    /// live when the phase's weights exceed the card. Weights make room for it, but what's live can't: the op gets no more than the
    /// rest of the card. Following free memory instead runs at the finest level whenever weights
    /// fill the card, and makes millions of allocations.
    fn target(&self) -> usize {
        self.trial().unwrap_or_else(|| self.fitting())
    }

    /// The coarsest level under the budget; when the phase streams, the coarsest of those that fits,
    /// else the finest. A phase that fits takes its level from the budget alone, and the allocation
    /// makes the room (H3 Turbo on a 4090 at levels 2/0/1/0, 310 s, against 5/2/2/3, 363 s).
    fn fitting(&self) -> usize {
        let cap = self.budget();
        let under = self.plans.iter().position(|plan| plan.want() <= cap).unwrap_or(self.plans.len() - 1);
        if !self.streams() {
            return under;
        }
        self.plans[under..].iter().position(Fit::fits).map_or(self.plans.len() - 1, |finer| under + finer)
    }

    fn streams(&self) -> bool {
        self.plans.first().and_then(|plan| plan.phase.as_ref()).is_some_and(|phase| phase.bytes > self.capacity)
    }

    /// A level that never ran is only an estimate, which can sit well over what it measures, and
    /// levels need not shrink in order: H3's Kitchen heads hold 4.6 GiB at level 0, 6.1 at level 1
    /// and 3.8 at level 2, against a 5.7 GiB budget, and level 2 costs 3.8 s a step. So once a
    /// level has measured, one call runs the nearest coarser one that never ran, if the card could
    /// hold it, and that measure decides.
    fn trial(&self) -> Option<usize> {
        let at = self.fitting();
        let room = self.capacity.saturating_sub(self.live);
        self.plans[at].measured().then(|| {
            self.plans[..at].iter().rposition(|plan| !plan.measured() && !plan.tried && plan.want() <= room && (!self.streams() || plan.fits()))
        })?
    }

    /// Weights over the card give room for free while the link has headroom (H3 Turbo at 8 GiB ran
    /// its heads at level 6 under a quarter-card floor, 26% more GPU work than v1); weights that fit
    /// keep at least a quarter of the card for the level (H3 Turbo's 20.7 GiB on a 4090).
    fn budget(&self) -> usize {
        let weights = self.plans.first().and_then(|plan| plan.phase.as_ref()).map_or(0, |phase| phase.bytes);
        let room = self.capacity.saturating_sub(self.live);
        if self.streams() { room } else { self.capacity.saturating_sub(weights).max(self.capacity / 4).min(room) }
    }

    fn selection(&self, selected: &Fit, target: usize, stream: u64) -> SplitSelection {
        SplitSelection {
            stream,
            choice_id: selected.id.clone(),
            eligible: self.plans[target..].iter().filter(|plan| plan.usable()).map(|plan| plan.id.clone()).collect(),
            required_bytes: selected.missing,
            available_bytes: if selected.preserves_phase() {
                selected.after_outside
            } else {
                selected.after_idle.saturating_add(selected.streamed)
            },
            allocation_backing_bytes: selected.full,
            basis: selected.basis,
            plan_generation: selected.generation,
            phase: selected.phase.as_ref().map(|p| p.name.clone()),
            phase_weight_bytes: selected.phase.as_ref().map_or(0, |p| p.bytes),
            budget_bytes: self.budget(),
            capacity_bytes: self.capacity,
            live_bytes: self.live,
            trying: self.trial().is_some_and(|at| self.plans[at].id == selected.id),
        }
    }
}

/// Evaluate exact allocation classes on the supplied execution stream. Compatible slots
/// satisfy requests once; global pending totals and unrelated classes never provide credit.
pub fn split_select(
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    stream: u64,
) -> Result<SplitSelection> {
    let result = split_select_inner(device, key, choices, stream);
    if result.is_err() {
        let _ = invalidate_current_phase(device, "split_select_error");
    }
    result
}

fn split_select_inner(
    device: i32,
    key: &str,
    choices: Vec<(String, Vec<usize>)>,
    stream: u64,
) -> Result<SplitSelection> {
    valid_stream(stream)?;
    context(device)?;
    let owner = initialize_owner(device)?;
    register(owner, key, choices)?;
    let room = owner.room_epoch();
    {
        let mut state = owner.lock()?;
        if let Some(mut selection) = state.splits.sites.get(key).and_then(|site| site.kept(room, stream)) {
            state.stats.split_lookups += 1;
            selection.plan_generation = owner.plan_generation.load(Ordering::Acquire);
            return Ok(selection);
        }
    }
    let selection = select(device, owner, key, stream)?;
    let mut state = owner.lock()?;
    state.stats.split_decisions += 1;
    if let Some(site) = state.splits.sites.get_mut(key) {
        if selection.trying {
            // Spent now, whatever comes of it; the next step decides again without it.
            site.tried.insert(selection.choice_id.clone());
        } else {
            site.decided = Some(Decided { selection: selection.clone(), room, measures: site.measures });
        }
    }
    Ok(selection)
}

impl Site {
    fn kept(&self, room: u64, stream: u64) -> Option<SplitSelection> {
        self.decided.as_ref()
            .filter(|kept| kept.room == room && kept.measures == self.measures && kept.selection.stream == stream)
            .map(|kept| kept.selection.clone())
    }
}

fn select(device: i32, owner: &'static Device, key: &str, stream: u64) -> Result<SplitSelection> {
    select_from(device, owner, key, stream, 0)
}

/// `select` among the levels from index `least` on.
fn select_from(device: i32, owner: &'static Device, key: &str, stream: u64, least: usize) -> Result<SplitSelection> {
    // A split's fit already counts completed working pages; split_begin trims what it needs.
    let keep = Protect { keep_working: true, ..Default::default() };
    // Make room for the target with memory this process can give up by itself.
    let at_target = reclaim::acquire(device, 0, Wait::Local, keep, || {
        let menu = snapshot(device, owner, key, stream)?;
        let target = menu.target().max(least);
        let selected = &menu.plans[target];
        if !selected.usable() {
            return Err(selected.short().into());
        }
        Ok(menu.selection(selected, target, stream))
    });
    match at_target {
        Err(Error::Exhausted { .. }) => {}
        other => return other,
    }
    // Only now a smaller legal choice, or what other threads and processes can give up.
    reclaim::acquire(device, 0, Wait::Yes, keep, || {
        let menu = snapshot(device, owner, key, stream)?;
        let target = menu.target().max(least);
        let choices = &menu.plans[target..];
        if let Some(selected) = choices.iter().find(|plan| plan.usable()) {
            return Ok(menu.selection(selected, target, stream));
        }
        let least = choices.iter().min_by_key(|plan| plan.missing.saturating_sub(plan.after_idle))
            .expect("copied menu");
        Err(least.short().into())
    })
}

/// After `split_end` closed a leaf of `key` at `choice_id` in which an allocation was refused with
/// everything reclaimable gone: the level a pure operation runs again at, finer than that one, with
/// its room made (Retry). None when no allocation was refused in that leaf (the failure is the
/// caller's, and final); `Exhausted` when no finer level can be given room (final).
pub fn split_refused(device: i32, key: &str, choice_id: &str, stream: u64) -> Result<Option<SplitSelection>> {
    valid_stream(stream)?;
    context(device)?;
    let owner = owner(device)?;
    let least = {
        let mut state = owner.lock()?;
        let last = state.splits.refused.take();
        if last.as_ref().is_none_or(|(k, c)| k != key || c != choice_id) {
            return Ok(None);
        }
        let site = state.splits.sites.get(key).ok_or(Error::Invalid("unknown split configuration"))?;
        let at = site.choices.iter().position(|choice| choice.id == choice_id)
            .ok_or(Error::Invalid("split choice is not declared"))?;
        let finest = site.choices.len() - 1;
        let refused = site.samples.get(choice_id).map_or(0, |sample| sample.refused);
        if at == finest {
            return Err(Error::Exhausted { requested_bytes: refused, available_bytes: 0 });
        }
        at + 1
    };
    let selection = select_from(device, owner, key, stream, least)?;
    owner.lock()?.stats.split_decisions += 1;
    Ok(Some(selection))
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
    {
        // The level kept for this room: its leaf opens with nothing read again.
        let room = owner.room_epoch();
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        match state.splits.sites.get(key).and_then(|site| site.kept(room, stream)) {
            Some(kept) if kept.choice_id == choice_id => {
                selected_generation(plan_generation, owner.plan_generation.load(Ordering::Acquire))?;
                return open_leaf(&mut state, key, choice_id, completion_owner, stream, gate.epoch());
            }
            // Another level than the kept one is a finer retry: the next step decides again.
            Some(_) => {
                if let Some(site) = state.splits.sites.get_mut(key) {
                    site.decided = None;
                }
            }
            None => {}
        }
    }
    let looping = owner.lock()?.looping();
    let keep = Protect { keep_working: true, keep_phase: looping, ..Default::default() };
    reclaim::acquire(device, 0, Wait::Yes, keep, || {
        // Keep GPU-owning leases outside and longer-lived than every exclusion guard.
        let lease = crate::plan::Lease::acquire(device)?;
        let gate = crate::gate(device)?;
        gate.not_capturing()?;
        let generation = lease.validate()?;
        let phase = crate::plan::snapshot(&lease)?;
        let mut state = owner.lock()?;
        selected_generation(plan_generation, generation)?;
        let _context = owner.driver().enter()?;
        let result = (|| -> Result<u64> {
            owner.poll(&mut state)?;
            owner.sample()?;
            let (free, _) = owner.driver().memory_info()?;
            let plans = fits(owner, &state, key, stream, free, generation, phase.as_ref())?;
            let plan = plans.iter().find(|plan| plan.id == choice_id)
                .ok_or(Error::Invalid("split choice is not declared"))?;
            if plan.missing > plan.now {
                // Matched compatible slots stay mapped, and so does a loop's; only other completed backing goes.
                let preserve = plan.preserve.union(&state.kept()).copied().collect();
                owner.trim_ready(&mut state, plan.missing - plan.now, None, &preserve, usize::MAX)?;
                let (free, _) = owner.driver().memory_info()?;
                let plans = fits(owner, &state, key, stream, free, generation, phase.as_ref())?;
                let plan = plans.iter().find(|plan| plan.id == choice_id).expect("copied choice");
                if plan.missing > plan.now {
                    return Err(Error::Exhausted {
                        requested_bytes: plan.missing,
                        available_bytes: plan.now,
                    });
                }
            }
            open_leaf(&mut state, key, choice_id, completion_owner, stream, gate.epoch())
        })();
        result.map_err(|error| owner.fail(&mut state, error).into())
    })
}

fn open_leaf(
    state: &mut State,
    key: &str,
    choice_id: &str,
    completion_owner: Option<u64>,
    stream: u64,
    epoch: u64,
) -> Result<u64> {
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
        epoch,
        allocations: BTreeSet::new(),
        peak: 0,
        backing: 0,
        invalid: None,
        classes: BTreeMap::new(),
        refused: 0,
    }));
    Ok(id)
}

/// An allocation of `bytes` on the open leaf's stream was refused with everything reclaimable
/// gone: its level needs at least that beside what the leaf holds live now.
pub(super) fn refused(state: &mut State, bytes: usize, stream: u64) {
    let Some(active) = state.splits.active.as_ref().filter(|active| active.stream == stream) else {
        return;
    };
    let live: usize = active.allocations.iter()
        .filter_map(|ptr| state.blocks.get(ptr).filter(|block| block.life == Life::Live))
        .map(|block| block.requested)
        .sum();
    let active = state.splits.active.as_mut().expect("open leaf");
    active.refused = active.refused.max(active.peak.max(live.saturating_add(bytes)));
}

/// What a leaf's refusal taught its level, at once: it is a bound on need, not a measure, so it
/// needs no completed pass, and every decision kept for this site goes.
fn learn(state: &mut State, key: &str, choice: &str, refused: usize) {
    if refused == 0 {
        return;
    }
    state.splits.refused = Some((key.to_owned(), choice.to_owned()));
    if let Some(site) = state.splits.sites.get_mut(key) {
        let sample = site.samples.entry(choice.to_owned()).or_default();
        if refused > sample.refused {
            sample.refused = refused;
            site.measures += 1;
            site.decided = None;
        }
    }
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
        // Live, as requested: a free still running on the GPU, or the larger block best-fit handed
        // out, would tie the sample to what's free, and the same request would next pick differently.
        if let Some(block) = state.blocks.get(ptr).filter(|block| block.life == Life::Live) {
            bytes += block.requested;
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
    learn(&mut state, &active.key, &active.choice, active.refused);
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
        } else if active.epoch != owner.epoch() {
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
        refused_bytes: active.refused,
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
        learn(state, &active.key, &active.choice, active.refused);
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
                refused_bytes: active.refused,
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
                // Backing follows where blocks happened to land; a choice reads only the peak and classes.
                let changed = !sample.known || sample.peak != row.peak_bytes || sample.classes != row.classes;
                sample.known = true;
                sample.peak = row.peak_bytes;
                sample.backing = row.backing_peak_bytes;
                sample.classes = row.classes.clone();
                sample.successful += 1;
                site.measures += u64::from(changed);
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
