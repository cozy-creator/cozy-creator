//! Working memory that outlives its pass: what a model keeps across its forwards as plain tensors
//! (a prefix's K/V, a library's cache), live working blocks no eviction can take. A pass begins at
//! each forward of a model (`pass_mark`). Each eager block is tallied by the pass and site it was
//! born in, and each open loop keeps a running sum of what its passes left live, so an allocation
//! costs O(open loops) and a mark O(1); naming the sites reads the tally, only as a loop's peak grows.
//! Kept across steps means live through a whole pass since its birth: a step's output that the next
//! step replaces is not.
//! Those bytes enter the room check of the loop's next pass, and the request's record names where
//! they were born.
use super::*;
use std::sync::atomic::AtomicU32;

/// Allocations outside any named site.
pub const UNNAMED: u32 = 0;
/// The sites a record names, largest first.
const NAMED: usize = 4;
/// A loop's peak grows this much past its last naming before its sites are read again.
const RENAME: usize = 64 << 20;

static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CURRENT: [AtomicU32; crate::MAX_DEVICES] = [const { AtomicU32::new(UNNAMED) }; crate::MAX_DEVICES];

/// The id of a birth site's name, the same for the process's life.
pub fn site_id(name: &str) -> u32 {
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if names.is_empty() {
        names.push(String::new());
    }
    match names.iter().position(|known| known == name) {
        Some(at) => at as u32,
        None => {
            names.push(name.to_owned());
            (names.len() - 1) as u32
        }
    }
}

fn name(site: u32) -> String {
    NAMES.lock().unwrap_or_else(|e| e.into_inner()).get(site as usize).cloned().unwrap_or_default()
}

/// Where this device's next allocations are born: one relaxed store, read by `current`.
pub fn set_site(device: i32, site: u32) {
    if let Some(slot) = usize::try_from(device).ok().and_then(|at| CURRENT.get(at)) {
        slot.store(site, Ordering::Relaxed);
    }
}

pub(super) fn current(device: i32) -> u32 {
    usize::try_from(device).ok().and_then(|at| CURRENT.get(at)).map_or(UNNAMED, |slot| slot.load(Ordering::Relaxed))
}

#[derive(Default)]
pub(super) struct Escapes {
    pass: u32,
    /// Live tracked bytes by (birth pass, site).
    live: BTreeMap<(u32, u32), usize>,
    total: usize,
    /// The most `total` reached since the last mark.
    peak: usize,
    last_head: Option<u32>,
    loops: BTreeMap<u32, Loop>,
    learned: BTreeMap<(u32, u64), Learned>,
    /// What this process held with the bytes it was last refused (`refused`) and the room it was
    /// refused in, and the refusals so far.
    refusal: usize,
    refusal_room: usize,
    refusals: u64,
    /// The refusals so far that varena answered and the op then completed (a Retry that ran).
    recovered: u64,
}

struct Loop {
    shape: u64,
    first: u32,
    /// The pass its last mark began, and `total` then.
    last: u32,
    start: usize,
    /// Live bytes born since `first`, and since `last`.
    since_first: usize,
    since_last: usize,
    passes: u32,
    carried: usize,
    carried_sites: Vec<(u32, usize)>,
    escaped: usize,
    peak: usize,
    named_at: usize,
    peak_sites: Vec<(u32, usize)>,
    /// The least a later pass added to what stays.
    growth: Option<usize>,
    /// The least a pass held beyond its start and what it left: an estimate that errs low.
    transient: Option<usize>,
    refused: Option<Verdict>,
    /// The device's refused allocations, and the bytes live and held, at its last mark.
    ooms: u64,
    held: usize,
}

#[derive(Clone, Copy)]
struct Learned {
    escaped: usize,
    transient: usize,
    /// A failed pass: what it held when refused, above what was held as it began, and the room it
    /// had above that.
    failed: usize,
    failed_room: Option<usize>,
}

/// Why a pass cannot fit with every evictable byte gone.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    pub need_bytes: usize,
    pub room_bytes: usize,
    /// Live working memory and pinned weights now.
    pub held_bytes: usize,
    /// What earlier passes left live (of `held`), or what the last loop of this shape left.
    pub escaped_bytes: usize,
    pub transient_bytes: usize,
    pub floor_bytes: usize,
    /// Decided at the loop's first forward from what its last run of this shape left.
    pub learned: bool,
    pub sites: Vec<(String, usize)>,
}

/// One loop's record: what it began with and what its passes left live.
#[derive(Clone, Debug, Default)]
pub struct EscapeRecord {
    pub head: String,
    pub passes: u32,
    pub carried_bytes: usize,
    pub carried_sites: Vec<(String, usize)>,
    pub escaped_bytes: usize,
    pub escaped_peak_bytes: usize,
    pub escaped_sites: Vec<(String, usize)>,
    pub growth_bytes: usize,
    pub transient_bytes: Option<usize>,
    pub refused: Option<Verdict>,
}

fn named(sites: &[(u32, usize)]) -> Vec<(String, usize)> {
    sites.iter().map(|(site, bytes)| (name(*site), *bytes)).collect()
}

impl Escapes {
    /// A live eager block born now, at `site`: its tally's key.
    pub(super) fn born(&mut self, bytes: usize, site: u32) -> (u32, u32) {
        let key = (self.pass, site);
        *self.live.entry(key).or_default() += bytes;
        self.total += bytes;
        self.peak = self.peak.max(self.total);
        for held in self.loops.values_mut() {
            held.since_first += bytes;
            held.since_last += bytes;
        }
        key
    }

    pub(super) fn died(&mut self, key: (u32, u32), bytes: usize) {
        if let Some(held) = self.live.get_mut(&key) {
            *held = held.saturating_sub(bytes);
            if *held == 0 {
                self.live.remove(&key);
            }
        }
        self.total = self.total.saturating_sub(bytes);
        for held in self.loops.values_mut().filter(|held| key.0 >= held.first) {
            held.since_first = held.since_first.saturating_sub(bytes);
            if key.0 >= held.last {
                held.since_last = held.since_last.saturating_sub(bytes);
            }
        }
    }

    /// The largest sites of what is live, born in passes `from..to`.
    fn sites(&self, from: u32, to: u32) -> Vec<(u32, usize)> {
        let mut by = BTreeMap::<u32, usize>::new();
        for (&(_, site), &bytes) in self.live.range((from, 0)..(to, 0)) {
            *by.entry(site).or_default() += bytes;
        }
        let mut sites: Vec<_> = by.into_iter().collect();
        sites.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
        sites.truncate(NAMED);
        sites
    }

    /// An allocation was refused with `owned` (this process's bytes and the request) not fitting.
    pub(super) fn refused(&mut self, owned: usize, room: usize) {
        self.refusal = owned;
        self.refusal_room = room;
        self.refusals += 1;
    }

    /// Every refusal so far was answered: its op ran again and completed, or it waits its turn.
    pub(super) fn answered(&mut self) {
        self.recovered = self.refusals;
    }

    /// What the next loop of this head and shape will need. One whose last pass was refused memory
    /// (`failed`) needed more than this process held then, above what it held as that pass began:
    /// a first call whose cache outgrew the card in step 0 has the next refused at its first forward.
    fn learn(&mut self, head: u32, held: &Loop, failed: bool) {
        let failed_room = failed.then(|| self.refusal_room.saturating_sub(held.held));
        let failed = if failed { self.refusal.saturating_sub(held.held) } else { 0 };
        if held.passes > 1 || failed_room.is_some() {
            let transient = held.transient.unwrap_or(0);
            self.learned.insert((head, held.shape), Learned { escaped: held.peak, transient, failed, failed_room });
        }
    }

    fn record(head: u32, held: Loop) -> EscapeRecord {
        EscapeRecord {
            head: name(head),
            passes: held.passes,
            carried_bytes: held.carried,
            carried_sites: named(&held.carried_sites),
            escaped_bytes: held.escaped,
            escaped_peak_bytes: held.peak,
            escaped_sites: named(&held.peak_sites),
            growth_bytes: held.growth.unwrap_or(0),
            transient_bytes: held.transient,
            refused: held.refused,
        }
    }
}

/// The most our own bytes could reach on this card, as last sampled (no driver call): its cap, else
/// the card, with what other processes hold counted as theirs to give back, as reclaim asks them to
/// (an idle peer executor's weights). A refusal by it never takes a pass that could fit.
fn room_now(owner: &Device) -> usize {
    let cap = owner.capacity();
    if cap != usize::MAX {
        return cap.saturating_add(owner.outside.load(Ordering::Acquire));
    }
    owner.total.load(Ordering::Acquire)
}

/// The room an allocation was refused against: the cap, else the card, less what others held then.
pub(super) fn room_held(owner: &Device) -> usize {
    let cap = owner.capacity();
    if cap != usize::MAX {
        return cap;
    }
    owner.total.load(Ordering::Acquire).saturating_sub(owner.outside.load(Ordering::Acquire))
}

/// A forward of the model `head` begins, its inputs of `shape` (0: unknown, and nothing is learned for
/// it); `floor` is the most of its weights one block holds at once. Its first forward this request
/// begins a loop (keyed by that forward's shape),
/// any later one a pass of it, whatever its own shape (a prefix cached in step 0 is absent after). A
/// pass that cannot fit with every evictable byte gone says why, once per loop: live working memory
/// and pinned weights, what a pass holds beyond them, and one block's weights, over the room.
/// Retained regions and unpinned weights are evictable. Nothing is marked under a capture or replay.
pub fn pass_mark(device: i32, head: u32, shape: u64, floor: usize) -> Result<Option<Verdict>> {
    let Some(owner) = existing(device)? else { return Ok(None) };
    if owner.graph_active() {
        return Ok(None);
    }
    let mut state = owner.lock()?;
    let held = state.stats.allocated_bytes.saturating_add(owner.held.load(Ordering::Acquire));
    let ooms = state.escapes.refusals;
    let room = room_now(owner);
    let escapes = &mut state.escapes;
    escapes.pass = escapes.pass.wrapping_add(1);
    let pass = escapes.pass;
    let interleaved = escapes.last_head.replace(head) != Some(head);
    let (peak, total) = (std::mem::replace(&mut escapes.peak, escapes.total), escapes.total);
    if !escapes.loops.contains_key(&head) {
        let learned = (shape != 0).then(|| escapes.learned.get(&(head, shape)).copied()).flatten();
        let strict = room_held(owner);
        let verdict = learned.and_then(|learned| {
            let planned = learned.escaped.saturating_add(learned.transient).saturating_add(floor);
            let need = held.saturating_add(planned.max(learned.failed));
            // A loop of this shape was refused memory with this much room above what it held: with no
            // more now, it is refused again, judged by the room it ran out in.
            let again = learned.failed_room.is_some_and(|had| strict.saturating_sub(held) <= had);
            let (need, room) = if again { (need.max(strict.saturating_add(1)), strict) } else { (need, room) };
            // What the last run held when refused counts as its working memory.
            let transient = learned.transient.max(learned.failed.saturating_sub(learned.escaped).saturating_sub(floor));
            (need > room).then(|| Verdict {
                need_bytes: need, room_bytes: room, held_bytes: held, escaped_bytes: learned.escaped,
                transient_bytes: transient, floor_bytes: floor, learned: true, sites: vec![],
            })
        });
        let carried_sites = escapes.sites(0, pass);
        escapes.loops.insert(head, Loop {
            shape, first: pass, last: pass, start: total, since_first: 0, since_last: 0, passes: 1,
            carried: total, carried_sites, escaped: 0, peak: 0, named_at: 0, peak_sites: vec![],
            growth: None, transient: None, refused: verdict.clone(), ooms, held,
        });
        return Ok(verdict);
    }
    // Kept across steps: born in an earlier pass and live through a whole pass since. What the last
    // pass made and the next replaces (a step's output, the sampler's state) is not.
    let (first, last) = (escapes.loops[&head].first, escapes.loops[&head].last);
    let kept = escapes.loops[&head].since_first.saturating_sub(escapes.loops[&head].since_last);
    let rename = kept > escapes.loops[&head].named_at.saturating_add(RENAME);
    let sites = rename.then(|| escapes.sites(first, last));
    let held_loop = escapes.loops.get_mut(&head).expect("this head's loop");
    let escaped = kept;
    if !interleaved {
        let transient = peak.saturating_sub(held_loop.start).saturating_sub(held_loop.since_last);
        held_loop.transient = Some(held_loop.transient.map_or(transient, |least| least.min(transient)));
    }
    if held_loop.passes >= 2 {
        let grew = escaped.saturating_sub(held_loop.escaped);
        held_loop.growth = Some(held_loop.growth.map_or(grew, |least| least.min(grew)));
    }
    held_loop.passes += 1;
    held_loop.last = pass;
    held_loop.start = total;
    held_loop.since_last = 0;
    held_loop.ooms = ooms;
    held_loop.held = held;
    held_loop.escaped = escaped;
    held_loop.peak = held_loop.peak.max(escaped);
    if let Some(sites) = sites {
        held_loop.named_at = escaped;
        held_loop.peak_sites = sites;
    }
    let transient = held_loop.transient.unwrap_or(0);
    let need = held.saturating_add(transient).saturating_add(held_loop.growth.unwrap_or(0)).saturating_add(floor);
    if need <= room || held_loop.refused.is_some() {
        return Ok(None);
    }
    let verdict = Verdict {
        need_bytes: need, room_bytes: room, held_bytes: held, escaped_bytes: escaped,
        transient_bytes: transient, floor_bytes: floor, learned: false, sites: named(&held_loop.peak_sites),
    };
    held_loop.refused = Some(verdict.clone());
    Ok(Some(verdict))
}

/// This request's loops, ended: each one's record. What they left is learned for the next loop of
/// the same head and shape, and so is a refusal in the last loop's last pass that nothing answered:
/// the request ended on it, in this process whichever rank or executor it is, so its next loop of
/// that shape is refused at its first forward unless the card has more room. A refusal varena
/// answered (a Retry that completed) teaches nothing.
pub fn escapes_take(device: i32) -> Result<Vec<EscapeRecord>> {
    let Some(owner) = existing(device)? else { return Ok(vec![]) };
    let mut state = owner.lock()?;
    let escapes = &mut state.escapes;
    let (ooms, recovered) = (escapes.refusals, escapes.recovered);
    let last = escapes.last_head.take();
    let ended = std::mem::take(&mut escapes.loops);
    let records = ended.into_iter().map(|(head, held)| {
        escapes.learn(head, &held, last == Some(head) && ooms > held.ooms.max(recovered));
        Escapes::record(head, held)
    }).collect();
    escapes.recovered = escapes.refusals;
    escapes.peak = escapes.total;
    Ok(records)
}
