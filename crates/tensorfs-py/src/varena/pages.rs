//! The host tier: weight pages in pinned host RAM, stored, baked or cast, so a refill is one DMA
//! and no store read or bake. One per GPU. Process lifetime, never on disk, each page once, within
//! half of the host's RAM and beyond a headroom. The tier follows use, not arrival: a page is
//! worth its refills per request, and only pages worth less make room for it.
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use platform::contain::host_memory;
use pyo3::{exceptions::PyRuntimeError, prelude::*, types::PyDict};
use tensorfs::io::cuda::HostPin;
use tensorfs::io::{self, Dst, ReadOpts, Src};
use tensorfs::io::source::Source as Store;

use super::source::Inner;

/// How stale a reading of host RAM may be.
const LOOK: Duration = Duration::from_millis(100);
/// How much of a page is read or registered before a command may take over: one registration
/// of H3's largest page made a request wait 0.6 s on an RTX 3090.
const WINDOW: usize = 64 << 20;

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
    fn registered(&self) -> bool {
        self.pins.len() * WINDOW >= self.len
    }
    fn register_window(&mut self, device: i32) -> Result<(), tensorfs::io::cuda::Error> {
        let (at, len) = self.windows().nth(self.pins.len()).expect("a window left to register");
        // SAFETY: inside this page, which outlives its pins: `Drop` releases them first.
        self.pins.push(unsafe { tensorfs::io::register(device, self.ptr.add(at), len) }?);
        Ok(())
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

/// A bake's bytes to keep, whole on the card, once `use` has gone on.
struct Behind {
    key: String,
    set: String,
    region: Weak<varena::Region>,
    len: usize,
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
    looked: Option<Instant>,
    off: Option<String>,
    wanted: HashMap<String, Wanted>,
    pin_failed: u64,
    /// Pages to keep behind `use`, and whether a thread is keeping them.
    behind: VecDeque<Behind>,
    keeping: bool,
    /// A wanted page read whole when a command arrived: it registers at the next idle.
    filled: Option<(String, String, Page)>,
    refills: HashMap<String, u64>,
    rate: HashMap<String, u64>,
    clock: u64,
    log: Vec<Row>,
}

pub(super) struct Tier {
    device: i32,
    cap: u64,
    headroom: i64,
    state: Mutex<State>,
    canceled: AtomicU64,
    failed: AtomicU64,
    /// One open store per checkpoint, for as long as a Source reads it: local, fetching.
    stores: Mutex<HashMap<String, (Weak<Store>, Weak<Store>)>>,
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
    pub(super) fn new(device: i32, cap: u64, headroom: u64) -> Self {
        let state = State { room: -1, ..State::default() };
        Self {
            device, cap, headroom: headroom as i64, state: Mutex::new(state),
            canceled: AtomicU64::new(0), failed: AtomicU64::new(0), stores: Mutex::default(),
        }
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
    /// go, the least worth first, while it is under the headroom. Unreadable: they all go.
    fn trim(&self, state: &mut State) -> Vec<Kept> {
        if state.looked.is_some_and(|looked| looked.elapsed() < LOOK) {
            return Vec::new();
        }
        state.looked = Some(Instant::now());
        let mut room = host_memory::read().available;
        state.room = room;
        let mut gone = Vec::new();
        if room >= self.headroom {
            return gone;
        }
        for (_, _, key) in state.cheapest() {
            if room >= self.headroom && room >= 0 {
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

    fn fits(&self, state: &State, bytes: i64) -> bool {
        state.bytes as i64 + bytes <= self.cap as i64 && state.room - bytes >= self.headroom
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
        if self.fits(state, bytes) {
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
        state.room += freed;
        (true, gone)
    }

    /// `key`'s page of `len` bytes, for a refill: a refill counts whichever way it is served.
    pub(super) fn take(&self, key: &str, len: usize) -> Option<Arc<Page>> {
        let (page, gone) = {
            let mut state = self.state();
            *state.refills.entry(key.to_owned()).or_default() += 1;
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
    /// None: `insert` it once they are written.
    pub(super) fn reserve(&self, key: &str, len: usize) -> PyResult<Option<Page>> {
        let (admitted, gone) = {
            let mut state = self.state();
            if state.kept.contains_key(key) {
                return Ok(None);
            }
            self.admit(&mut state, key, len as u64)
        };
        drop(gone);
        if !admitted {
            return Ok(None);
        }
        let mut page = Page::new(len)?;
        while !page.registered() {
            if let Err(error) = page.register_window(self.device) {
                self.state().off = Some(error.to_string());
                return Ok(None);
            }
        }
        Ok(Some(page))
    }

    /// Keep `region`'s bytes as `key` behind the thread that called `use`: first touching 21 GB of
    /// pages there left the card idle for 17 s of H3's first step. The thread ends with its queue.
    pub(super) fn keep_behind(self: &Arc<Self>, key: &str, set: &str, region: Weak<varena::Region>, len: usize) {
        let mut state = self.state();
        state.behind.push_back(Behind { key: key.to_owned(), set: set.to_owned(), region, len });
        if !std::mem::replace(&mut state.keeping, true) {
            let tier = self.clone();
            let spawned = std::thread::Builder::new().name(format!("varena-keep-{}", self.device))
                .spawn(move || while let Some(job) = tier.next_behind() { tier.keep(job) });
            if spawned.is_err() {
                state.behind.clear(); // Those pages' next fills bake again.
                state.keeping = false;
            }
        }
    }

    fn next_behind(&self) -> Option<Behind> {
        let mut state = self.state();
        let job = state.behind.pop_front();
        state.keeping = job.is_some();
        job
    }

    /// The page is set up before the pin, which stops an eviction only for the copy. A Region
    /// that went first, or a capture, costs its next fill a bake, never a wrong page: its bytes
    /// never change, and `key` names them.
    fn keep(&self, job: Behind) {
        let Ok(Some(page)) = self.reserve(&job.key, job.len) else { return };
        let Some(region) = job.region.upgrade() else { return };
        let Ok(_producer) = varena::enter_producer(self.device) else { return };
        let Ok(Some(pin)) = region.try_pin() else { return };
        let copies: Vec<_> = page.windows().map(|(at, len)| io::Copy {
            src: Src::Device { device: self.device, ptr: pin.ptr() + at as u64, stream: 0 },
            // SAFETY: `at` lies inside the page.
            dst: Dst::Host { ptr: unsafe { page.ptr().add(at) }, len },
        }).collect();
        // SAFETY: the pin holds the source, ready and never written again; the page is this
        // thread's, registered window by window.
        let copied = unsafe { io::read(&copies, &ReadOpts::default()) };
        // SAFETY: the read returned, so nothing reads through this pin.
        drop(unsafe { pin.into_completed_region() });
        if copied.is_ok() {
            self.insert(&job.key, &job.set, page);
        }
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
        let gone: Vec<_> = {
            let mut state = self.state();
            let keys: Vec<_> = state.kept.iter().filter(|(_, kept)| set.is_none_or(|set| kept.set == set))
                .map(|(key, _)| key.clone()).collect();
            state.wanted.retain(|_, wanted| set.is_some_and(|set| wanted.set != set));
            state.behind.retain(|job| set.is_some_and(|set| job.set != set));
            if set.is_none() {
                state.filled = None;
            }
            keys.iter().filter_map(|key| state.remove(key)).collect()
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
    fn new(device: i32, cap: u64, headroom: u64) -> Self {
        Self { tier: Arc::new(Tier::new(device, cap, headroom)) }
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
    /// A request ended: its refills become each page's worth.
    fn request_ended(&self) {
        let mut state = self.tier.state();
        let refills = std::mem::take(&mut state.refills);
        state.rate.extend(refills);
    }
    /// The fills since the last call: (set, block, bytes, origin, ahead, ns, unix ms).
    fn drain(&self) -> Vec<(String, u32, u64, &'static str, bool, u64, f64)> {
        std::mem::take(&mut self.tier.state().log).into_iter()
            .map(|row| (row.set, row.block, row.bytes, row.origin, row.ahead, row.ns, row.at_ms)).collect()
    }
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let (pages, bytes, room, wanted, pin_failed, off) = {
            let state = self.tier.state();
            (state.kept.len(), state.bytes, state.room, state.wanted.len(), state.pin_failed, state.off.clone())
        };
        let row = PyDict::new(py);
        row.set_item("scope", "the host tier: every streamed page's bytes, pinned")?;
        row.set_item("pages", pages)?;
        row.set_item("bytes", bytes)?;
        row.set_item("room", room)?;
        row.set_item("cap", self.tier.cap)?;
        row.set_item("pinned", off.is_none())?;
        row.set_item("refused", off)?;
        row.set_item("wanted", wanted)?;
        row.set_item("pin_failed", pin_failed)?;
        let host = |at: usize| tensorfs::io::cuda::HOST_PINS[at].load(Ordering::Relaxed);
        row.set_item("host_registers", (host(0), host(1)))?;
        row.set_item("host_unregisters", (host(2), host(3)))?;
        row.set_item("host_allocs", (host(4), host(5)))?;
        row.set_item("ahead_canceled", self.tier.canceled.load(Ordering::Relaxed))?;
        row.set_item("ahead_failed", self.tier.failed.load(Ordering::Relaxed))?;
        Ok(row)
    }
}

pub(super) fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1e3)
}
