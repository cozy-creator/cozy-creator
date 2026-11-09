//! The host tier: weight pages in pinned host RAM, stored, baked or cast, so a refill is one DMA
//! and no store read or bake. One per GPU. Process lifetime, never on disk, each page once, within
//! half of the host's RAM and beyond a headroom (`Tier::look`). The tier follows use, not arrival:
//! a page is worth its refills per request, and only pages worth less make room for it.
//!
//! Pinned pages are memory the kernel cannot take back. The tier is a client of varena's host
//! number (`varena_ledger::host`): its room is what that number leaves, clean page cache counted
//! as reclaimable. Every executor's tiers share the host through its ledger page (`host`): idle
//! ones give pages back to a call that is short. On a person's computer nothing pins while the
//! host stalls on memory, and the headroom doubles.
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use platform::contain::host_memory;
use varena_ledger::host::{Budget, budget};
use pyo3::{exceptions::PyRuntimeError, prelude::*, types::PyDict};
use tensorfs::io::cuda::HostPin;
use tensorfs::io::source::{Item, ItemSource, Source as Store};

use super::source::Inner;

/// How stale a reading of host RAM may be.
const LOOK: Duration = Duration::from_millis(100);
/// How much of a page is read or registered before a command may take over: one registration
/// of H3's largest page made a request wait 0.6 s on an RTX 3090.
pub(super) const WINDOW: usize = 64 << 20;
/// Threads faulting a page in: registering untouched memory on the fill path held each of Flux-2's
/// 57 blocks about 0.2 s in step 1 (run 39103). On a 4090 host the driver registered untouched
/// memory at 3 GB/s, one page at a time, touched memory at 28, and faulting in ran at 10.5 GB/s on
/// four threads.
const FAULTERS: usize = 4;

/// Host bytes, page aligned, registered once filled: registering touched memory runs at about
/// twice the rate of untouched memory. A window at a time, and no copy spans two: CUDA refuses it.
pub(super) struct Page {
    ptr: *mut u8,
    len: usize,
    pins: Vec<HostPin>,
}
// SAFETY: an owned allocation; its pin is a registration handle used with its context entered.
unsafe impl Send for Page {}
// SAFETY: as above. Bytes are written only before the page is shared.
unsafe impl Sync for Page {}

impl Page {
    fn layout(len: usize) -> std::alloc::Layout {
        std::alloc::Layout::from_size_align(len.max(1), 4096).expect("a page's layout")
    }
    pub(super) fn new(len: usize) -> PyResult<Self> {
        // SAFETY: the layout is nonzero.
        let ptr = unsafe { std::alloc::alloc(Self::layout(len)) };
        if ptr.is_null() {
            return Err(PyRuntimeError::new_err("host page allocation failed"));
        }
        Ok(Self { ptr, len, pins: Vec::new() })
    }
    /// Fault every byte in, a part on each of `FAULTERS` threads.
    fn populate(&self) {
        let part = self.len.div_ceil(FAULTERS).next_multiple_of(4096).max(4096);
        let base = self.ptr as usize;
        std::thread::scope(|scope| {
            for at in (0..self.len).step_by(part) {
                let len = part.min(self.len - at);
                scope.spawn(move || fault_in(base + at, len));
            }
        });
    }
    pub(super) fn ptr(&self) -> *mut u8 {
        self.ptr
    }
    pub(super) fn len(&self) -> usize {
        self.len
    }
    /// Each window's (offset, length).
    pub(super) fn windows(&self) -> impl Iterator<Item = (usize, usize)> + use<'_> {
        (0..self.len).step_by(WINDOW).map(|at| (at, WINDOW.min(self.len - at)))
    }
    pub(super) fn registered(&self) -> bool {
        self.pins.len() * WINDOW >= self.len
    }
    pub(super) fn register_window(&mut self, device: i32) -> Result<(), tensorfs::io::cuda::Error> {
        let (at, len) = self.windows().nth(self.pins.len()).expect("a window left to register");
        // SAFETY: inside this page, which outlives its pins: `Drop` releases them first.
        self.pins.push(unsafe { tensorfs::io::register(device, self.ptr.add(at), len) }?);
        Ok(())
    }
}
/// `len` bytes from `at`, inside an unshared page and page aligned, faulted in.
fn fault_in(at: usize, len: usize) {
    #[cfg(target_os = "linux")]
    // SAFETY: the caller's range.
    if unsafe { libc::madvise(at as *mut libc::c_void, len, libc::MADV_POPULATE_WRITE) } == 0 {
        return;
    }
    for at in (at..at + len).step_by(4096) {
        // SAFETY: as above; nothing reads it yet.
        unsafe { (at as *mut u8).write_volatile(0) };
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        for pin in &mut self.pins {
            if pin.release().is_err() {
                return; // The driver may still touch it: it stays, with its registrations.
            }
        }
        // SAFETY: allocated in `new` with this layout; nothing else owns it.
        unsafe { std::alloc::dealloc(self.ptr, Self::layout(self.len)) };
    }
}

struct Kept {
    page: Arc<Page>,
    set: String,
    last: u64,
}

/// Bytes moving into or out of the tier that the kernel's figures may not show yet, counted while
/// this lives.
struct Moving {
    count: Arc<AtomicI64>,
    bytes: i64,
}

impl Moving {
    fn new(count: &Arc<AtomicI64>, bytes: i64) -> Self {
        count.fetch_add(bytes, Ordering::Relaxed);
        Self { count: count.clone(), bytes }
    }
    fn add(&mut self, bytes: i64) {
        self.count.fetch_add(bytes, Ordering::Relaxed);
        self.bytes += bytes;
    }
}

impl Drop for Moving {
    fn drop(&mut self) {
        self.count.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// Pages the tier let go, unregistered and freed when this drops, never under the lock. Until
/// then their bytes are `freeing`: the pages drop before the count does (field order).
pub(super) struct Gone {
    pages: Vec<Kept>,
    freeing: Moving,
}

impl Gone {
    fn new(tier: &Tier) -> Self {
        Self { pages: Vec::new(), freeing: Moving::new(&tier.freeing, 0) }
    }
    fn push(&mut self, kept: Kept) {
        self.freeing.add(kept.page.len as i64);
        self.pages.push(kept);
    }
    fn extend(&mut self, kept: Option<Kept>) {
        if let Some(kept) = kept {
            self.push(kept);
        }
    }
}

/// A stored page read once: pinned at idle if it has left the card by then.
struct Wanted {
    set: String,
    source: Weak<Inner>,
    region: Weak<varena::Region>,
}

/// A page registered ahead for a derive's bytes, not yet written.
struct Reserved {
    set: String,
    page: Page,
}

/// One fill, for the request's record.
pub(super) struct Row {
    pub(super) set: String,
    pub(super) block: u32,
    pub(super) bytes: u64,
    pub(super) origin: &'static str,
    pub(super) ahead: bool,
    pub(super) ns: u64,
    pub(super) at_ms: f64,
}

#[derive(Default)]
struct State {
    kept: HashMap<String, Kept>,
    bytes: u64,
    room: i64,
    /// What stays free, as the last look decided it.
    headroom: i64,
    /// The host stalled on memory at the last look (a shared machine): `host_memory::Stall`.
    pressed: bool,
    looked: Option<Instant>,
    off: Option<String>,
    wanted: HashMap<String, Wanted>,
    pin_failed: u64,
    /// A wanted page read whole when a command arrived: it registers at the next idle.
    filled: Option<(String, String, Page)>,
    refills: HashMap<String, u64>,
    rate: HashMap<String, u64>,
    /// Each set's pass this request, from 1: a page taken again in the same pass begins the next.
    passes: HashMap<String, u64>,
    /// The pass of its set each page was last taken in.
    taken: HashMap<String, u64>,
    /// Bytes of pages let go this process, by why: short of room, displaced, given, shed, forgotten.
    let_go: HashMap<&'static str, u64>,
    clock: u64,
    log: Vec<Row>,
    /// Pages derives will keep, to make while their fills run, in the order the fills began.
    ahead: VecDeque<(String, Weak<Inner>)>,
    reserving: HashSet<String>,
    reserved: HashMap<String, Reserved>,
    /// Counts `forget`s: a page registered across one is not kept.
    forgets: u64,
    /// Keeps that found their page registered ahead.
    reserved_ahead: u64,
    /// Each store read's (set, block, O_DIRECT bytes, cached bytes, ns): this request's, the last one's.
    reads: Vec<(String, u32, u64, u64, u64)>,
    last_reads: Vec<(String, u32, u64, u64, u64)>,
    /// Registered pages a retained region gave back, and whether since the last request ended: the
    /// next request of the same shape takes them without registering again, else they go.
    spare: Vec<(Page, bool)>,
}

pub(super) struct Tier {
    device: i32,
    cap: u64,
    /// A person's computer: the headroom is its reserve, read at each look (`look`).
    shared: bool,
    state: Mutex<State>,
    /// An ahead reservation was queued or ended.
    reserving: Condvar,
    reservers: OnceLock<()>,
    canceled: AtomicU64,
    failed: AtomicU64,
    /// Fill-path time in `keep`, and of it in `reserve`.
    pub(super) keep_ns: AtomicU64,
    pub(super) reserve_ns: AtomicU64,
    /// Bytes admitted and not yet made (`Moving`): the kernel charges them once they are touched.
    making: Arc<AtomicI64>,
    /// Bytes let go and not yet freed (`Gone`): the kernel charges them until the unregister ends.
    freeing: Arc<AtomicI64>,
    /// One open store per checkpoint, for as long as a Source reads it: local, fetching.
    stores: Mutex<HashMap<String, (Weak<Store>, Weak<Store>)>>,
    /// The Sources placing each stored object range (by object, then offset and length), and the
    /// offset in their bytes. Content-addressed, so two sets sharing objects (a LoRA view and its
    /// base, two releases' text encoder) find each other's bytes.
    objects: Mutex<HashMap<String, Ranges>>,
}

/// An object's ranges (offset, length) -> each Source placing it, and where in its bytes.
type Ranges = HashMap<(u64, u64), Vec<(Weak<Inner>, u64)>>;

/// Where a fill's item already is: another Source holding the stored bytes, and their offset there.
pub(super) struct Held {
    pub(super) source: Arc<Inner>,
    pub(super) at: u64,
}

impl State {
    /// `key`'s refills per request: the last request's, or this one's, where a page its set's pass
    /// has not reached yet counts that pass too. Blocks refill in a fixed order, so counted as
    /// taken, the blocks a step reached next were worth one less than those it had passed, and a
    /// tier short of room let them go first, each just before its use: Flux-2 refilled none of
    /// its 57 blocks from the tier in 8 steps (pasan, run 42449).
    fn worth(&self, key: &str, set: &str) -> u64 {
        let refills = self.refills.get(key).copied().unwrap_or(0);
        let behind = self.taken.get(key).is_some_and(|taken| self.passes.get(set).is_some_and(|pass| taken < pass));
        self.rate.get(key).copied().unwrap_or(1).max(refills + u64::from(behind))
    }
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
    /// One refill of `key`, in its set's pass: taken again in the same pass, the next begins.
    fn count(&mut self, key: &str, set: &str) {
        let pass = self.passes.entry(set.to_owned()).or_insert(1);
        if self.taken.get(key).copied() == Some(*pass) {
            *pass += 1;
        }
        self.taken.insert(key.to_owned(), *pass);
        *self.refills.entry(key.to_owned()).or_default() += 1;
    }
    fn remove(&mut self, key: &str, why: &'static str) -> Option<Kept> {
        let kept = self.kept.remove(key)?;
        self.bytes -= kept.page.len as u64;
        *self.let_go.entry(why).or_default() += kept.page.len as u64;
        Some(kept)
    }
    /// Keys from the least worth, then the most recently used: of pages a loop refills alike, the
    /// one it used last is the one it needs last.
    fn cheapest(&self) -> Vec<(u64, Reverse<u64>, String)> {
        let mut keys: Vec<_> = self.kept.iter()
            .map(|(key, kept)| (self.worth(key, &kept.set), Reverse(kept.last), key.clone())).collect();
        keys.sort_unstable();
        keys
    }
}

impl Tier {
    /// This GPU's tier, within `cap` (half of physical memory by default), in the host's ledger page.
    pub(super) fn new(device: i32, cap: Option<u64>, shared: bool) -> Arc<Self> {
        let cap = cap.unwrap_or(u64::try_from(host_memory::total()).unwrap_or(0) / 2);
        let tier = Arc::new(Self {
            device, cap, shared, state: Mutex::new(State { room: -1, ..State::default() }),
            reserving: Condvar::new(), reservers: OnceLock::new(), canceled: AtomicU64::new(0), failed: AtomicU64::new(0),
            keep_ns: AtomicU64::new(0), reserve_ns: AtomicU64::new(0), making: Arc::default(), freeing: Arc::default(),
            stores: Mutex::default(), objects: Mutex::default(),
        });
        tier.look(&mut tier.state(), &budget());
        super::host::join(Arc::downgrade(&tier));
        tier
    }

    /// `source`'s stored object ranges, so other fills can find its bytes.
    pub(super) fn place(&self, source: &Arc<Inner>, items: &[Item]) {
        let mut objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
        for item in items {
            if let ItemSource::Object(range) = &item.source {
                let placed = objects.entry(range.obj.sha256.clone()).or_default().entry((range.off, range.len)).or_default();
                placed.retain(|(placed, _)| placed.strong_count() != 0);
                placed.push((Arc::downgrade(source), item.offset));
            }
        }
    }

    /// For each of `items`, the other Sources whose bytes are the stored ones and place it.
    pub(super) fn holders(&self, source: &Arc<Inner>, items: &[Item]) -> Vec<Vec<Held>> {
        let objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
        items.iter().map(|item| {
            let ItemSource::Object(range) = &item.source else { return Vec::new() };
            let Some(placed) = objects.get(&range.obj.sha256).and_then(|ranges| ranges.get(&(range.off, range.len))) else {
                return Vec::new();
            };
            placed.iter().filter(|(other, _)| !std::ptr::eq(other.as_ptr(), Arc::as_ptr(source)))
                .filter_map(|(other, at)| {
                    let other = other.upgrade().filter(|other| other.stored_bytes())?;
                    Some(Held { source: other, at: *at })
                })
                .collect()
        }).collect()
    }

    /// `checkpoint`'s open stores: chunk files stay open once however many Regions read them.
    pub(super) fn stores(&self, checkpoint: &tensorfs::Checkpoint) -> (Arc<Store>, Arc<Store>) {
        let mut stores = self.stores.lock().unwrap_or_else(|error| error.into_inner());
        stores.retain(|_, (local, _)| local.strong_count() != 0);
        let key = checkpoint.id().sha256.clone();
        if let Some((local, fetching)) = stores.get(&key) {
            if let (Some(local), Some(fetching)) = (local.upgrade(), fetching.upgrade()) {
                return (local, fetching);
            }
        }
        let (local, fetching) = (Arc::new(checkpoint.local_source(true)), Arc::new(checkpoint.source(true)));
        stores.insert(key, (Arc::downgrade(&local), Arc::downgrade(&fetching)));
        (local, fetching)
    }
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// What varena's host number leaves this process, re-read at most every `LOOK`, after kept
    /// pages go, the least worth first, while it is under the headroom. Unreadable: they all go.
    /// Clean page cache is room, active or not: the kernel takes it back before it kills, and
    /// pinned pages hold the bytes a cached copy would. In a request, short of the headroom: idle
    /// peers are asked for their pages, and this tier's go only for what they do not hold; their
    /// pages are worth less than any of a running call's.
    fn trim(&self, state: &mut State) -> Gone {
        let mut gone = Gone::new(self);
        if state.looked.is_some_and(|looked| looked.elapsed() < LOOK) {
            return gone;
        }
        state.looked = Some(Instant::now());
        let budget = budget();
        self.look(state, &budget);
        let (room, mut left) = self.rooms(&budget);
        state.room = room;
        if left >= state.headroom {
            return gone;
        }
        let idle = if left >= 0 { super::host::short((state.headroom - left) as u64) } else { 0 };
        let idle = i64::try_from(idle).unwrap_or(i64::MAX);
        let headroom = state.headroom;
        let enough = |left: i64| left >= 0 && left.saturating_add(idle) >= headroom;
        while !enough(left) {
            let Some((page, _)) = state.spare.pop() else { break };
            left += page.len as i64;
            *state.let_go.entry("short").or_default() += page.len as u64;
            gone.push(Kept { page: Arc::new(page), set: String::new(), last: 0 });
        }
        for (_, _, key) in state.cheapest() {
            if enough(left) {
                break;
            }
            if let Some(kept) = state.remove(&key, "short") {
                if left >= 0 {
                    left += kept.page.len as i64;
                }
                gone.push(kept);
            }
        }
        gone
    }

    /// The room varena's number leaves this tier, as the kernel would show it with the tier's own
    /// moves landed: to pin more, less what it admitted and has not touched yet; to decide what goes,
    /// plus what it let go and has not freed yet. Read straight, each look before an unregister
    /// ended took more pages again, and Flux-2's tier emptied every step (pasan, run 42449).
    fn rooms(&self, budget: &Budget) -> (i64, i64) {
        if budget.total <= 0 {
            return (-1, -1);
        }
        let room = budget.total - budget.held;
        (room - self.making.load(Ordering::Relaxed).max(0), room + self.freeing.load(Ordering::Relaxed).max(0))
    }

    /// What stays free beside pinned pages: the number's headroom (`Budget::headroom`). On a
    /// person's computer (`shared`), while the host stalls on memory: no pins, and a sixteenth of
    /// the number more.
    fn look(&self, state: &mut State, budget: &Budget) {
        state.pressed = self.shared && host_memory::stall().pressed();
        let stalled = if state.pressed { (budget.total / 16).max(512 << 20) } else { 0 };
        state.headroom = budget.headroom + stalled;
    }

    /// The bytes this tier pins: its pages, those made ahead and its spares.
    pub(super) fn held(&self) -> u64 {
        let state = self.state();
        state.bytes + state.spare.iter().map(|(page, _)| page.len as u64).sum::<u64>()
    }

    /// Give about `bytes` back, spares first, then the pages worth least: an idle process's, to a
    /// call that asked (`host`). The bytes given.
    pub(super) fn give(&self, bytes: u64) -> u64 {
        let (given, gone) = {
            let mut state = self.state();
            let mut gone = Gone::new(self);
            let mut given = 0;
            while given < bytes {
                let Some((page, _)) = state.spare.pop() else { break };
                given += page.len as u64;
                *state.let_go.entry("given").or_default() += page.len as u64;
                gone.push(Kept { page: Arc::new(page), set: String::new(), last: 0 });
            }
            for (_, _, key) in state.cheapest() {
                if given >= bytes {
                    break;
                }
                if let Some(kept) = state.remove(&key, "given") {
                    given += kept.page.len as u64;
                    gone.push(kept);
                }
            }
            state.looked = None;
            (given, gone)
        };
        drop(gone); // Unregistering waits on the device: never under the lock.
        given
    }

    /// Look at the host now: what grew since takes pages back.
    pub(super) fn relook(&self) {
        let gone = {
            let mut state = self.state();
            state.looked = None;
            self.trim(&mut state)
        };
        drop(gone);
    }

    /// Short of the headroom for `bytes` more while the cap allows them: idle peers are asked.
    fn ask_room(&self, state: &State, bytes: i64) {
        if state.room >= 0 && state.bytes as i64 + bytes <= self.cap as i64 {
            super::host::short((state.headroom + bytes - state.room).max(0) as u64);
        }
    }

    fn fits(&self, state: &State, bytes: i64) -> bool {
        !state.pressed && state.bytes as i64 + bytes <= self.cap as i64 && state.room - bytes >= state.headroom
    }

    /// Whether a page of `bytes` that is no weight (a retained region's) may pin now, within the
    /// headroom. `shed`: a region's copy waits on it, and kept weight pages, which the store
    /// refills, go first, the least worth first; a page made ahead takes only free room. While the
    /// host stalls nothing grows: a copy pins only in place of the weight pages it sheds. The
    /// pages that go come with the answer, and the bytes are `making` while the guard lives.
    fn room_for(&self, bytes: usize, shed: bool) -> (Option<Moving>, Gone) {
        let mut state = self.state();
        let mut gone = self.trim(&mut state);
        let bytes = bytes as i64;
        let pressed = state.pressed;
        if pressed && !shed {
            return (None, gone);
        }
        let short = |state: &State, shed: i64| if pressed { shed < bytes } else { state.room - bytes < state.headroom };
        let mut freed = 0;
        for (_, _, key) in if shed { state.cheapest() } else { Vec::new() } {
            if !short(&state, freed) {
                break;
            }
            if let Some(kept) = state.remove(&key, "shed") {
                freed += kept.page.len as i64;
                state.room += kept.page.len as i64;
                gone.push(kept);
            }
        }
        if short(&state, freed) {
            self.ask_room(&state, bytes);
            return (None, gone);
        }
        state.room -= bytes;
        (Some(Moving::new(&self.making, bytes)), gone)
    }

    /// Whether `key` of `set`, `bytes` of host RAM, enters the tier. Where there is no room it
    /// displaces kept pages worth less, never one worth as much: blocks refill in a fixed order,
    /// so replacing the least recently used of equals would miss every refill. Admitted, the
    /// bytes are `making` while the guard lives.
    fn admit(&self, state: &mut State, key: &str, set: &str, bytes: u64) -> (Option<Moving>, Gone) {
        let mut gone = self.trim(state);
        if state.off.is_some() {
            return (None, gone);
        }
        let bytes = bytes as i64;
        if state.pressed {
            return (None, gone);
        }
        if self.fits(state, bytes) {
            state.room -= bytes;
            return (Some(Moving::new(&self.making, bytes)), gone);
        }
        self.ask_room(state, bytes);
        let worth = state.worth(key, set);
        let (mut freed, mut chosen) = (0, Vec::new());
        for (value, _, kept) in state.cheapest() {
            if value >= worth || self.fits(state, bytes - freed) {
                break;
            }
            freed += state.kept[&kept].page.len as i64;
            chosen.push(kept);
        }
        if !self.fits(state, bytes - freed) {
            return (None, gone);
        }
        for key in chosen {
            gone.extend(state.remove(&key, "displaced"));
        }
        state.room += freed - bytes;
        (Some(Moving::new(&self.making, bytes)), gone)
    }

    /// `key`'s page of `len` bytes. A GPU `refill` counts whichever way it is served, in the pass
    /// of `set` it is in.
    pub(super) fn take(&self, key: &str, set: &str, len: usize, refill: bool) -> Option<Arc<Page>> {
        let (page, gone) = {
            let mut state = self.state();
            if refill {
                state.count(key, set);
            }
            let gone = self.trim(&mut state);
            let tick = state.tick();
            let page = state.kept.get_mut(key).filter(|kept| kept.page.len == len).map(|kept| {
                kept.last = tick;
                Arc::clone(&kept.page)
            });
            (page, gone)
        };
        drop(gone); // Unregistering waits on the device: never under the lock.
        page
    }

    /// A GPU refill of `key` in `set` that `take` did not count.
    pub(super) fn count(&self, key: &str, set: &str) {
        self.state().count(key, set);
    }

    pub(super) fn holds(&self, key: &str) -> bool {
        self.state().kept.contains_key(key)
    }

    /// A page with bytes the caller is about to write for `key`, admitted and registered, or
    /// None: `insert` it once they are written. One registered ahead if there is, or will be soon.
    pub(super) fn reserve(&self, key: &str, set: &str, len: usize) -> PyResult<Option<Page>> {
        let (making, gone) = {
            let mut state = self.state();
            while state.reserving.contains(key) {
                state = self.reserving.wait(state).unwrap_or_else(|error| error.into_inner());
            }
            state.ahead.retain(|(queued, _)| queued != key);
            if state.kept.contains_key(key) {
                return Ok(None);
            }
            if let Some(reserved) = state.reserved.remove(key) {
                state.bytes -= reserved.page.len as u64;
                state.reserved_ahead += 1;
                return Ok(Some(reserved.page));
            }
            self.admit(&mut state, key, set, len as u64)
        };
        drop(gone);
        let Some(making) = making else { return Ok(None) };
        let page = self.registered(len, false);
        drop(making);
        page
    }

    /// A page of `len` bytes, registered, faulted in first if `populate`. None: the driver refused,
    /// and the tier is off.
    fn registered(&self, len: usize, populate: bool) -> PyResult<Option<Page>> {
        let mut page = Page::new(len)?;
        if populate {
            page.populate();
        }
        while !page.registered() {
            if let Err(error) = page.register_window(self.device) {
                self.state().off = Some(error.to_string());
                return Ok(None);
            }
        }
        Ok(Some(page))
    }

    /// `source`'s fill began and its derive will keep the bytes: its page is made on the
    /// tier's thread while the fill reads and derives, for `keep` to take.
    pub(super) fn expect(self: &Arc<Self>, key: String, source: &Arc<Inner>) {
        self.state().ahead.push_back((key, Arc::downgrade(source)));
        self.reserving.notify_all();
        self.reservers.get_or_init(|| {
            let tier = Arc::clone(self);
            let spawned = std::thread::Builder::new().name("tensorfs-reserve".into()).spawn(move || tier.reserve_ahead());
            if let Err(error) = spawned {
                tracing::warn!("host pages register on their fills: {error}");
            }
        });
    }

    /// Make the expected pages, the first expected first, while the tier admits them.
    fn reserve_ahead(&self) {
        loop {
            let (key, source, forgets, gone) = {
                let mut state = self.state();
                let (key, source) = loop {
                    if state.off.is_some() {
                        return;
                    }
                    match state.ahead.pop_front() {
                        Some((key, source)) => break (key, source),
                        None => state = self.reserving.wait(state).unwrap_or_else(|error| error.into_inner()),
                    }
                };
                let Some(source) = source.upgrade() else { continue };
                if state.kept.contains_key(&key) || state.reserved.contains_key(&key) || state.reserving.contains(&key) {
                    continue;
                }
                let (making, gone) = self.admit(&mut state, &key, source.set(), source.bytes() as u64);
                if making.is_some() {
                    state.bytes += source.bytes() as u64;
                    state.reserving.insert(key.clone());
                }
                (making.map(|making| (key, making)), source, state.forgets, gone)
            };
            drop(gone);
            let Some((key, making)) = key else { continue };
            let page = self.registered(source.bytes(), true).ok().flatten();
            drop(making);
            let unused = {
                let mut state = self.state();
                state.reserving.remove(&key);
                let page = match page {
                    Some(page) if state.forgets == forgets && !state.kept.contains_key(&key) && source.keeps() => {
                        state.reserved.insert(key, Reserved { set: source.set().to_owned(), page });
                        None
                    }
                    page => {
                        state.bytes -= source.bytes() as u64;
                        page
                    }
                };
                self.reserving.notify_all();
                page
            };
            drop(unused);
        }
    }

    /// `key`'s derive wrote nothing: its stored bytes are pinned at idle, if it streams.
    pub(super) fn unreserve(&self, key: &str) {
        let reserved = {
            let mut state = self.state();
            state.ahead.retain(|(queued, _)| queued != key);
            let reserved = state.reserved.remove(key);
            if let Some(reserved) = &reserved {
                state.bytes -= reserved.page.len as u64;
            }
            reserved
        };
        drop(reserved);
    }

    pub(super) fn insert(&self, key: &str, set: &str, page: Page) {
        let replaced = {
            let mut state = self.state();
            let tick = state.tick();
            state.bytes += page.len as u64;
            let kept = Kept { page: Arc::new(page), set: set.to_owned(), last: tick };
            let replaced = state.kept.insert(key.to_owned(), kept);
            if let Some(replaced) = &replaced {
                state.bytes -= replaced.page.len as u64;
            }
            replaced
        };
        drop(replaced);
    }

    /// A store read of `key` completed.
    pub(super) fn want(&self, key: &str, set: &str, source: Weak<Inner>, region: Weak<varena::Region>) {
        let mut state = self.state();
        if !state.kept.contains_key(key) {
            state.wanted.insert(key.to_owned(), Wanted { set: set.to_owned(), source, region });
        }
    }

    /// A request ended: its refills become each page's worth.
    pub(super) fn request_ended(&self) {
        let mut state = self.state();
        state.rate = std::mem::take(&mut state.refills);
        state.passes.clear();
        state.taken.clear();
        state.looked = None;
        state.last_reads = std::mem::take(&mut state.reads);
        let (fresh, stale): (Vec<_>, Vec<_>) = std::mem::take(&mut state.spare).into_iter().partition(|(_, fresh)| *fresh);
        state.spare = fresh.into_iter().map(|(page, _)| (page, false)).collect();
        drop(state);
        drop(stale); // Unregistering waits on the device: never under the lock.
    }

    /// A registered host page of at least `len` bytes for a retained region's copy: the smallest
    /// spare that holds it, else a new one. Short of room, idle peers are asked first and it
    /// tries again once they answered. `Ok(None)`: no room now; the region stays spillable.
    pub(super) fn retained_page(&self, len: usize) -> Result<Option<Page>, String> {
        let spare = {
            let mut state = self.state();
            let best = state.spare.iter().enumerate().filter(|(_, (page, _))| page.len >= len)
                .min_by_key(|(_, (page, _))| page.len).map(|(at, _)| at);
            best.map(|at| state.spare.swap_remove(at).0)
        };
        if let Some(page) = spare {
            return Ok(Some(page));
        }
        match self.new_page(len, true)? {
            Some(page) => Ok(Some(page)),
            None if super::host::ask_idle(len as u64) => self.new_page(len, true),
            None => Ok(None),
        }
    }

    /// Faulted in first: the driver registers touched memory about nine times as fast, and
    /// registering holds every other driver call of the process. Only within the headroom
    /// (`room_for`): None when there is none.
    fn new_page(&self, len: usize, shed: bool) -> Result<Option<Page>, String> {
        let (making, gone) = self.room_for(len, shed);
        drop(gone); // Unregistering waits on the device: never under the lock.
        let Some(making) = making else { return Ok(None) };
        let mut page = Page::new(len).map_err(|error| error.to_string())?;
        page.populate();
        while !page.registered() {
            page.register_window(self.device).map_err(|error| error.to_string())?;
        }
        drop(making);
        Ok(Some(page))
    }

    /// A spare page of `len` bytes made now, while nothing waits for it, unless one is there: the
    /// next retained region of a loop is the size of the last.
    pub(super) fn spare_ahead(&self, len: usize) {
        if self.state().spare.iter().any(|(page, _)| page.len >= len) {
            return;
        }
        if let Ok(Some(page)) = self.new_page(len, false) {
            self.spare(page);
        }
    }

    pub(super) fn spare(&self, page: Page) {
        self.state().spare.push((page, true));
    }

    /// Host bytes retained regions may take: what is available beyond the headroom, the tier's own
    /// pages and spares included, which yield to them.
    pub(super) fn retained_room(&self) -> i64 {
        let budget = budget();
        let mut state = self.state();
        self.look(&mut state, &budget);
        let spare: u64 = state.spare.iter().map(|(page, _)| page.len as u64).sum();
        budget.total - budget.held + state.bytes as i64 + spare as i64 - state.headroom
    }

    /// Where a store read's bytes came from: past the page cache, or through it.
    pub(super) fn read_source(&self, set: &str, block: u32, stats: &tensorfs::io::ReadStats) {
        self.state().reads.push((set.to_owned(), block, stats.direct, stats.cached, stats.ns));
    }

    pub(super) fn row(&self, row: Row) {
        self.state().log.push(row);
    }

    pub(super) fn canceled(&self) {
        self.canceled.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    /// The wanted page worth most that has left the card.
    fn next_wanted(state: &mut State) -> Option<(String, Arc<Inner>, String)> {
        state.wanted.retain(|_, wanted| wanted.source.strong_count() != 0 && wanted.region.strong_count() != 0);
        let key = state.wanted.iter()
            .filter(|(_, wanted)| wanted.region.upgrade().is_some_and(|region| !region.holds_contents()))
            .map(|(key, wanted)| (state.worth(key, &wanted.set), key)).max()?.1.clone();
        let wanted = state.wanted.remove(&key)?;
        Some((key, wanted.source.upgrade()?, wanted.set))
    }

    /// Pages to pin at idle, while no other process on the machine is in a request: its call has
    /// the host's bytes first.
    fn pending(&self) -> bool {
        if super::host::peers_running() {
            return false;
        }
        let state = self.state();
        state.off.is_none() && (state.filled.is_some() || state.wanted.values().any(|wanted| {
            wanted.region.upgrade().is_some_and(|region| !region.holds_contents())
        }))
    }

    /// Keep `page`, read whole, once the rest of it is registered. False stops `pin_idle`: a
    /// command is waiting, and the next idle takes the page up where it was, or pinning failed
    /// and the tier is off.
    fn register(
        &self,
        py: Python<'_>,
        waiting: &impl Fn() -> PyResult<bool>,
        (key, set, mut page): (String, String, Page),
    ) -> PyResult<bool> {
        while !page.registered() {
            if waiting()? {
                self.state().filled = Some((key, set, page));
                return Ok(false);
            }
            if let Err(error) = py.detach(|| page.register_window(self.device)) {
                self.state().off = Some(error.to_string());
                return Ok(false);
            }
        }
        self.insert(&key, &set, page);
        Ok(true)
    }

    /// Pin the wanted pages until a command arrives: each read again from the store into host RAM,
    /// and registered once filled, window by window. The page refilled most per request goes first.
    fn pin_idle(&self, py: Python<'_>, waiting: &Bound<'_, PyAny>) -> PyResult<()> {
        let is_waiting = || -> PyResult<bool> { Ok(waiting.call0()?.is_truthy()? || super::host::peers_running()) };
        let filled = self.state().filled.take();
        if let Some(filled) = filled {
            if !self.register(py, &is_waiting, filled)? {
                return Ok(());
            }
        }
        while !is_waiting()? {
            let (next, making, gone) = {
                let mut state = self.state();
                let Some((key, source, set)) = Self::next_wanted(&mut state) else { return Ok(()) };
                let (making, gone) = self.admit(&mut state, &key, &set, source.bytes() as u64);
                ((key, source, set), making, gone)
            };
            drop(gone);
            let (key, source, set) = next;
            let Some(_making) = making else {
                continue; // Kept pages are worth at least as much; a smaller wanted one may fit.
            };
            let page = Page::new(source.bytes())?;
            let (mut at, mut read) = (0, Ok(()));
            while at < page.len && read.is_ok() {
                if is_waiting()? {
                    return self.requeue(key, set, &source);
                }
                let len = WINDOW.min(page.len - at);
                let (ptr, whole) = (page.ptr as usize, page.len);
                read = py.detach(|| source.read_window(ptr as *mut u8, whole, at, len));
                at += len;
            }
            if let Err(error) = read {
                self.state().pin_failed += 1;
                tracing::warn!("a host page did not pin: {error}");
                continue;
            }
            if !self.register(py, &is_waiting, (key, set, page))? {
                return Ok(());
            }
        }
        Ok(())
    }

    /// The page a command interrupted is wanted again.
    fn requeue(&self, key: String, set: String, source: &Arc<Inner>) -> PyResult<()> {
        let region = source.region();
        self.state().wanted.insert(key, Wanted { set, source: Arc::downgrade(source), region });
        Ok(())
    }

    /// Every page goes, or a set's: its fill changed, or it closed.
    fn forget(&self, set: Option<&str>) {
        let gone = {
            let mut state = self.state();
            let keys: Vec<_> = state.kept.iter().filter(|(_, kept)| set.is_none_or(|set| kept.set == set))
                .map(|(key, _)| key.clone()).collect();
            state.wanted.retain(|_, wanted| set.is_some_and(|set| wanted.set != set));
            let spare = if set.is_none() {
                state.filled = None;
                std::mem::take(&mut state.spare)
            } else {
                Vec::new()
            };
            state.forgets += 1;
            state.ahead.retain(|(_, source)| set.is_some_and(|set| source.upgrade().is_some_and(|source| source.set() != set)));
            let reserved: Vec<_> = state.reserved.iter().filter(|(_, reserved)| set.is_none_or(|set| reserved.set == set))
                .map(|(key, _)| key.clone()).collect();
            let reserved: Vec<_> = reserved.iter().filter_map(|key| state.reserved.remove(key)).collect();
            state.bytes -= reserved.iter().map(|reserved| reserved.page.len as u64).sum::<u64>();
            (keys.iter().filter_map(|key| state.remove(key, "forgotten")).collect::<Vec<_>>(), reserved, spare)
        };
        drop(gone);
    }
}

#[pyclass(name = "_VarenaPages", module = "tensorfs._ext", frozen)]
pub(super) struct PyPages {
    pub(super) tier: Arc<Tier>,
}

#[pymethods]
impl PyPages {
    #[new]
    #[pyo3(signature = (device, cap = None, shared = false))]
    fn new(device: i32, cap: Option<u64>, shared: bool) -> Self {
        Self { tier: Tier::new(device, cap, shared) }
    }
    /// Whether pages wait for an idle executor to pin them.
    fn pending(&self, py: Python<'_>) -> bool {
        py.detach(|| self.tier.pending())
    }
    fn pin_idle(&self, py: Python<'_>, waiting: &Bound<'_, PyAny>) -> PyResult<()> {
        self.tier.pin_idle(py, waiting)
    }
    /// A set's pages go, or every page with none named. A reading of `set` alone keeps the rest.
    #[pyo3(signature = (set = None))]
    fn forget(&self, py: Python<'_>, set: Option<String>) {
        py.detach(|| self.tier.forget(set.as_deref()));
    }
    /// A request ended: see `Tier::request_ended`.
    fn request_ended(&self, py: Python<'_>) {
        py.detach(|| self.tier.request_ended());
    }
    /// Host bytes retained regions may take now: see `Tier::retained_room`.
    fn retained_room(&self, py: Python<'_>) -> i64 {
        py.detach(|| self.tier.retained_room())
    }
    /// The fills since the last call: (set, block, bytes, origin, ahead, ns, unix ms).
    fn drain(&self) -> Vec<(String, u32, u64, &'static str, bool, u64, f64)> {
        std::mem::take(&mut self.tier.state().log).into_iter()
            .map(|row| (row.set, row.block, row.bytes, row.origin, row.ahead, row.ns, row.at_ms)).collect()
    }
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let (pages, bytes, room, wanted, pin_failed, off, headroom, pressed, reserved, reserved_ahead, reads, let_go) = {
            let state = self.tier.state();
            (state.kept.len(), state.bytes, state.room, state.wanted.len(), state.pin_failed, state.off.clone(),
                state.headroom, state.pressed, state.reserved.len(), state.reserved_ahead, state.last_reads.clone(),
                state.let_go.clone())
        };
        let row = PyDict::new(py);
        row.set_item("scope", "the host tier: every streamed page's bytes, pinned")?;
        row.set_item("pages", pages)?;
        row.set_item("bytes", bytes)?;
        row.set_item("room", room)?;
        row.set_item("cap", self.tier.cap)?;
        row.set_item("headroom", headroom)?;
        row.set_item("pressed", pressed)?;
        row.set_item("pinned", off.is_none())?;
        row.set_item("refused", off)?;
        row.set_item("wanted", wanted)?;
        row.set_item("pin_failed", pin_failed)?;
        row.set_item("reserved", reserved)?;
        row.set_item("reserved_ahead", reserved_ahead)?;
        row.set_item("ahead_canceled", self.tier.canceled.load(Ordering::Relaxed))?;
        row.set_item("ahead_failed", self.tier.failed.load(Ordering::Relaxed))?;
        row.set_item("keep_ns", self.tier.keep_ns.load(Ordering::Relaxed))?;
        row.set_item("reserve_ns", self.tier.reserve_ns.load(Ordering::Relaxed))?;
        row.set_item("let_go_scope", "bytes of pages let go this process, by why")?;
        row.set_item("let_go", let_go)?;
        row.set_item("making", self.tier.making.load(Ordering::Relaxed))?;
        row.set_item("freeing", self.tier.freeing.load(Ordering::Relaxed))?;
        row.set_item("store_reads_scope", "the last request's store reads: set, block, O_DIRECT bytes, cached bytes, ns")?;
        row.set_item("store_reads", reads)?;
        Ok(row)
    }
}

pub(super) fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1e3)
}
