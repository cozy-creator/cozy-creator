//! The host tier: weight pages in pinned host RAM, stored, baked or cast, so a refill is one DMA
//! and no store read or bake. One per GPU. Process lifetime, never on disk, each page once, within
//! half of the host's RAM and beyond a headroom. The tier follows use, not arrival: a page is
//! worth its refills per request, and only pages worth less make room for it.
//!
//! Pinned pages are memory the kernel cannot take back, so on a person's computer (a shared
//! machine) the headroom is what the computer keeps for its other programs
//! (`host_memory::reserve`), and while the host stalls on memory nothing pins and the headroom
//! doubles: the tier gives pages back, never what starves the computer.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use platform::contain::host_memory;
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
    /// The wanted pages this request read from the store.
    read: HashSet<String>,
    pin_failed: u64,
    /// A wanted page read whole when a command arrived: it registers at the next idle.
    filled: Option<(String, String, Page)>,
    refills: HashMap<String, u64>,
    rate: HashMap<String, u64>,
    /// The most active page cache that counts as room, and the limit it was decided under: see
    /// `request_ended`.
    own: i64,
    bound: i64,
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
    /// One open store per checkpoint, for as long as a Source reads it: local, fetching.
    stores: Mutex<HashMap<String, (Weak<Store>, Weak<Store>)>>,
    /// The Sources placing each stored object range (by object, then offset and length), and the
    /// offset in their bytes. Content-addressed, so two sets sharing objects (a LoRA view and its
    /// base, two releases' text encoder) find each other's bytes.
    objects: Mutex<HashMap<String, Ranges>>,
    /// Each set's Sources with stored bytes, for `Inner::read_ahead`.
    sets: Mutex<HashMap<String, Vec<Weak<Inner>>>>,
}

/// An object's ranges (offset, length) -> each Source placing it, and where in its bytes.
type Ranges = HashMap<(u64, u64), Vec<(Weak<Inner>, u64)>>;

/// Where a fill's item already is: another Source holding the stored bytes, and their offset there.
pub(super) struct Held {
    pub(super) source: Arc<Inner>,
    pub(super) at: u64,
}

impl State {
    fn worth(&self, key: &str) -> u64 {
        self.rate.get(key).copied().unwrap_or(1).max(self.refills.get(key).copied().unwrap_or(0))
    }
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
    fn remove(&mut self, key: &str) -> Option<Kept> {
        let kept = self.kept.remove(key)?;
        self.bytes -= kept.page.len as u64;
        Some(kept)
    }
    /// Keys from the least worth, then the least recently used.
    fn cheapest(&self) -> Vec<(u64, u64, String)> {
        let mut keys: Vec<_> = self.kept.iter().map(|(key, kept)| (self.worth(key), kept.last, key.clone())).collect();
        keys.sort_unstable();
        keys
    }
}

impl Tier {
    pub(super) fn new(device: i32, cap: u64, headroom: u64, shared: bool) -> Self {
        let state = State { room: -1, headroom: headroom as i64, ..State::default() };
        Self {
            device, cap, shared, state: Mutex::new(state),
            reserving: Condvar::new(), reservers: OnceLock::new(), canceled: AtomicU64::new(0), failed: AtomicU64::new(0),
            keep_ns: AtomicU64::new(0), reserve_ns: AtomicU64::new(0), stores: Mutex::default(),
            objects: Mutex::default(), sets: Mutex::default(),
        }
    }

    /// `source`'s stored object ranges, so other fills can find its bytes, and it in its set.
    pub(super) fn place(&self, source: &Arc<Inner>, items: &[Item]) {
        {
            let mut sets = self.sets.lock().unwrap_or_else(|error| error.into_inner());
            let members = sets.entry(source.set().to_owned()).or_default();
            members.retain(|member| member.strong_count() != 0);
            members.push(Arc::downgrade(source));
        }
        let mut objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
        for item in items {
            if let ItemSource::Object(range) = &item.source {
                let placed = objects.entry(range.obj.sha256.clone()).or_default().entry((range.off, range.len)).or_default();
                placed.retain(|(placed, _)| placed.strong_count() != 0);
                placed.push((Arc::downgrade(source), item.offset));
            }
        }
    }

    /// The live Sources of `set`.
    pub(super) fn members(&self, set: &str) -> Vec<Arc<Inner>> {
        let sets = self.sets.lock().unwrap_or_else(|error| error.into_inner());
        sets.get(set).map_or_else(Vec::new, |members| members.iter().filter_map(Weak::upgrade).collect())
    }

    /// For each of `items`, the other Sources whose bytes are the stored ones and place it.
    pub(super) fn held(&self, source: &Arc<Inner>, items: &[Item]) -> Vec<Vec<Held>> {
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

    /// The host RAM this process can still take, re-read at most every `LOOK`, after kept pages
    /// go, the least worth first, while it is under the headroom. Unreadable: they all go. Active
    /// page cache up to `own` is room: the loop's store reads keep it active, so yielding to it
    /// only feeds a cache the loop's own refills fill.
    fn trim(&self, state: &mut State) -> Vec<Kept> {
        if state.looked.is_some_and(|looked| looked.elapsed() < LOOK) {
            return Vec::new();
        }
        state.looked = Some(Instant::now());
        self.look(state);
        let memory = host_memory::read();
        let own = if memory.bound == state.bound { memory.active.min(state.own) } else { 0 };
        let mut room = if memory.available < 0 { memory.available } else { memory.available + own };
        state.room = room;
        let mut gone = Vec::new();
        if room >= state.headroom {
            return gone;
        }
        for (page, _) in state.spare.drain(..) {
            room += page.len as i64;
            gone.push(Kept { page: Arc::new(page), set: String::new(), last: 0 });
        }
        for (_, _, key) in state.cheapest() {
            if room >= state.headroom && room >= 0 {
                break;
            }
            if let Some(kept) = state.remove(&key) {
                if room >= 0 {
                    room += kept.page.len as i64;
                }
                gone.push(kept);
            }
        }
        state.room = room;
        gone
    }

    /// What stays free now: a dedicated machine's headroom; on a shared one the host's reserve,
    /// twice it while the host stalls on memory.
    fn look(&self, state: &mut State) {
        if !self.shared {
            return;
        }
        state.pressed = host_memory::stall().pressed();
        let reserve = i64::try_from(host_memory::reserve()).unwrap_or(i64::MAX);
        state.headroom = if state.pressed { reserve.saturating_mul(2) } else { reserve };
    }

    fn fits(&self, state: &State, bytes: i64) -> bool {
        !state.pressed && state.bytes as i64 + bytes <= self.cap as i64 && state.room - bytes >= state.headroom
    }

    /// Whether a page of `bytes` that is no weight (a retained region's) may pin now, within the
    /// headroom and never while the host stalls. `shed`: a region's copy waits on it, and kept
    /// weight pages, which the store refills, go first, the least worth first; a page made ahead
    /// takes only free room. The pages that go come with the answer.
    fn room_for(&self, bytes: usize, shed: bool) -> (bool, Vec<Kept>) {
        let mut state = self.state();
        let mut gone = self.trim(&mut state);
        let bytes = bytes as i64;
        if state.pressed {
            return (false, gone);
        }
        for (_, _, key) in if shed { state.cheapest() } else { Vec::new() } {
            if state.room - bytes >= state.headroom {
                break;
            }
            if let Some(kept) = state.remove(&key) {
                state.room += kept.page.len as i64;
                gone.push(kept);
            }
        }
        let fits = state.room - bytes >= state.headroom;
        if fits {
            state.room -= bytes;
        }
        (fits, gone)
    }

    /// Whether `key`, `bytes` of host RAM, enters the tier. Where there is no room it displaces
    /// kept pages worth less, never one worth as much: blocks refill in a fixed order, so
    /// replacing the least recently used of equals would miss every refill.
    fn admit(&self, state: &mut State, key: &str, bytes: u64) -> (bool, Vec<Kept>) {
        let mut gone = self.trim(state);
        if state.off.is_some() {
            return (false, gone);
        }
        let bytes = bytes as i64;
        if state.pressed {
            return (false, gone);
        }
        if self.fits(state, bytes) {
            state.room -= bytes;
            return (true, gone);
        }
        let worth = state.worth(key);
        let (mut freed, mut chosen) = (0, Vec::new());
        for (value, _, kept) in state.cheapest() {
            if value >= worth || self.fits(state, bytes - freed) {
                break;
            }
            freed += state.kept[&kept].page.len as i64;
            chosen.push(kept);
        }
        if !self.fits(state, bytes - freed) {
            return (false, gone);
        }
        for key in chosen {
            gone.extend(state.remove(&key));
        }
        state.room += freed - bytes;
        (true, gone)
    }

    /// `key`'s page of `len` bytes. A GPU `refill` counts whichever way it is served.
    pub(super) fn take(&self, key: &str, len: usize, refill: bool) -> Option<Arc<Page>> {
        let (page, gone) = {
            let mut state = self.state();
            if refill {
                *state.refills.entry(key.to_owned()).or_default() += 1;
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

    pub(super) fn holds(&self, key: &str) -> bool {
        self.state().kept.contains_key(key)
    }

    /// A page with bytes the caller is about to write for `key`, admitted and registered, or
    /// None: `insert` it once they are written. One registered ahead if there is, or will be soon.
    pub(super) fn reserve(&self, key: &str, len: usize) -> PyResult<Option<Page>> {
        let (admitted, gone) = {
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
            self.admit(&mut state, key, len as u64)
        };
        drop(gone);
        if !admitted {
            return Ok(None);
        }
        self.registered(len, false)
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
                let (admitted, gone) = self.admit(&mut state, &key, source.bytes() as u64);
                if admitted {
                    state.bytes += source.bytes() as u64;
                    state.reserving.insert(key.clone());
                }
                (admitted.then_some(key), source, state.forgets, gone)
            };
            drop(gone);
            let Some(key) = key else { continue };
            let page = self.registered(source.bytes(), true).ok().flatten();
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
            state.read.insert(key.to_owned());
            state.wanted.insert(key.to_owned(), Wanted { set: set.to_owned(), source, region });
        }
    }

    /// A request ended: its refills become each page's worth, and the bytes its loop read from the
    /// store and the tier does not keep bound the active page cache that counts as room until the
    /// next ends. Moving the loop's bytes from the page cache into the tier shrinks what the cache
    /// must hold by as much as the cache shrinks, and a kept page always hits: on a 32 GB cgroup
    /// the tier held 17.3 GB of Flux-2 to v1's 21.2 while 3.2 GB of active cache stayed outside
    /// its room, and 5.8 GB a step more came from the store (run 37385: 12.7 s a step against
    /// v1's 3.4). Decided once a request, never per step: the cache the loop keeps active moves
    /// every pass. Other active cache is never room past those bytes.
    pub(super) fn request_ended(&self) {
        let memory = host_memory::read();
        let mut state = self.state();
        state.rate = std::mem::take(&mut state.refills);
        let read = std::mem::take(&mut state.read);
        let streamed = read.iter().filter_map(|key| state.wanted.get(key)?.source.upgrade()).map(|source| source.bytes() as i64).sum();
        (state.own, state.bound, state.looked) = (streamed, memory.bound, None);
        state.last_reads = std::mem::take(&mut state.reads);
        let (fresh, stale): (Vec<_>, Vec<_>) = std::mem::take(&mut state.spare).into_iter().partition(|(_, fresh)| *fresh);
        state.spare = fresh.into_iter().map(|(page, _)| (page, false)).collect();
        drop(state);
        drop(stale); // Unregistering waits on the device: never under the lock.
    }

    /// A registered host page of at least `len` bytes for a retained region's copy: the smallest
    /// spare that holds it, else a new one.
    pub(super) fn retained_page(&self, len: usize) -> Result<Page, String> {
        let spare = {
            let mut state = self.state();
            let best = state.spare.iter().enumerate().filter(|(_, (page, _))| page.len >= len)
                .min_by_key(|(_, (page, _))| page.len).map(|(at, _)| at);
            best.map(|at| state.spare.swap_remove(at).0)
        };
        match spare {
            Some(page) => Ok(page),
            None => self.new_page(len, true),
        }
    }

    /// Faulted in first: the driver registers touched memory about nine times as fast, and
    /// registering holds every other driver call of the process. Only within the headroom
    /// (`room_for`): refused, the region stays on the card.
    fn new_page(&self, len: usize, shed: bool) -> Result<Page, String> {
        let (fits, gone) = self.room_for(len, shed);
        drop(gone); // Unregistering waits on the device: never under the lock.
        if !fits {
            return Err(format!("no host room for {} MiB beyond what the computer keeps free", len >> 20));
        }
        let mut page = Page::new(len).map_err(|error| error.to_string())?;
        page.populate();
        while !page.registered() {
            page.register_window(self.device).map_err(|error| error.to_string())?;
        }
        Ok(page)
    }

    /// A spare page of `len` bytes made now, while nothing waits for it, unless one is there: the
    /// next retained region of a loop is the size of the last.
    pub(super) fn spare_ahead(&self, len: usize) {
        if self.state().spare.iter().any(|(page, _)| page.len >= len) {
            return;
        }
        if let Ok(page) = self.new_page(len, false) {
            self.spare(page);
        }
    }

    pub(super) fn spare(&self, page: Page) {
        self.state().spare.push((page, true));
    }

    /// Host bytes retained regions may take: what is available beyond the headroom, the tier's own
    /// pages and spares included, which yield to them.
    pub(super) fn retained_room(&self) -> i64 {
        let memory = host_memory::read();
        let mut state = self.state();
        self.look(&mut state);
        let spare: u64 = state.spare.iter().map(|(page, _)| page.len as u64).sum();
        memory.available + state.bytes as i64 + spare as i64 - state.headroom
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
            .map(|(key, _)| (state.worth(key), key)).max()?.1.clone();
        let wanted = state.wanted.remove(&key)?;
        Some((key, wanted.source.upgrade()?, wanted.set))
    }

    fn pending(&self) -> bool {
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
        let is_waiting = || -> PyResult<bool> { waiting.call0()?.is_truthy() };
        let filled = self.state().filled.take();
        if let Some(filled) = filled {
            if !self.register(py, &is_waiting, filled)? {
                return Ok(());
            }
        }
        while !is_waiting()? {
            let (next, admitted, gone) = {
                let mut state = self.state();
                let Some((key, source, set)) = Self::next_wanted(&mut state) else { return Ok(()) };
                let (admitted, gone) = self.admit(&mut state, &key, source.bytes() as u64);
                ((key, source, set), admitted, gone)
            };
            drop(gone);
            let (key, source, set) = next;
            if !admitted {
                continue; // Kept pages are worth at least as much; a smaller wanted one may fit.
            }
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
            (keys.iter().filter_map(|key| state.remove(key)).collect::<Vec<_>>(), reserved, spare)
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
    #[pyo3(signature = (device, cap, headroom, shared = false))]
    fn new(device: i32, cap: u64, headroom: u64, shared: bool) -> Self {
        Self { tier: Arc::new(Tier::new(device, cap, headroom, shared)) }
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
        let (pages, bytes, room, wanted, pin_failed, off, headroom, pressed, reserved, reserved_ahead, reads) = {
            let state = self.tier.state();
            (state.kept.len(), state.bytes, state.room, state.wanted.len(), state.pin_failed, state.off.clone(),
                state.headroom, state.pressed, state.reserved.len(), state.reserved_ahead, state.last_reads.clone())
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
        row.set_item("store_reads_scope", "the last request's store reads: set, block, O_DIRECT bytes, cached bytes, ns")?;
        row.set_item("store_reads", reads)?;
        Ok(row)
    }
}

pub(super) fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1e3)
}
