//! TensorFS as varena's Source: a Region's bytes from its host page, else its derived page on disk
//! (a derive made it in an earlier process: `Store::derived`), else each stored object range from
//! where it already is (another set's resident Region on this GPU, that set's kept page, or its
//! own bytes the tier read ahead), else the store (fetched if a caller is waiting), then its
//! derive on the thread that called `use`. A worker running ahead of demand never sees Python:
//! what needs a derive declines there. A derive's kept page is also written to disk, behind the
//! fill (`persist`), when its Source names a slot.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use pyo3::{exceptions::PyRuntimeError, ffi, prelude::*};
use tensorfs::io::source::{IoTally, Item, ItemSource, Mode, Placement, Source as Store};
use tensorfs::io::{self, Dst, ObjectRange, ReadControl, ReadOpts, Src};

use super::pages::{self, Held, Page, Persisting, PyPages, Row, Tier, WINDOW};
use super::{Consumers, PyUse};
use crate::store::PyCheckpoint;

const READERS: usize = 8;

/// A fill's failure as its caller's Python should see it, kept for `use` to raise.
pub(super) enum Failed {
    Python(PyErr),
    Storage(tensorfs::Failure),
}
thread_local! {
    static FAILED: RefCell<Option<Failed>> = const { RefCell::new(None) };
}
pub(super) fn take_failure(py: Python<'_>) -> Option<PyErr> {
    FAILED.with_borrow_mut(Option::take).map(|failed| match failed {
        Failed::Python(error) => error,
        Failed::Storage(failure) => crate::errors::raise(py, failure),
    })
}

struct Stored {
    items: Vec<Item>,
    /// Chunks on this machine only: all a worker may touch.
    local: Arc<Store>,
    /// Fetches what it lacks, for a caller who is waiting.
    fetching: Arc<Store>,
    checkpoint: tensorfs::Checkpoint,
}

/// A derived page on disk: its key, and its object as a read-engine source.
pub(super) struct OnDisk {
    pub(super) page: String,
    items: Vec<Item>,
    source: Store,
}

impl OnDisk {
    /// Its bytes into `dst`, which holds the whole page.
    pub(super) fn read(&self, dst: Dst, threads: usize) -> Result<tensorfs::io::ReadStats, tensorfs::io::source::Error> {
        // SAFETY: the caller's destination is exclusive and holds the page; the items tile it.
        unsafe { self.source.read_items(&self.items, 0, dst, threads, Mode::Cache, &IoTally::default()) }
    }
}

/// Where a Source's derived page is: not looked for yet, not on disk, or there.
enum Disk {
    Unknown,
    Absent,
    Here(Arc<OnDisk>),
}

pub(super) struct Inner {
    tier: Arc<Tier>,
    /// Its name in the tier. None: it is never kept.
    page: Option<String>,
    /// Its block under its fill identity, whatever the overlay (a digest): names its derived pages
    /// on disk (`Store::derived`). None: its derive's bytes never leave the process.
    slot: Option<String>,
    set: String,
    block: u32,
    bytes: usize,
    stored: Option<Stored>,
    derive: Option<Py<PyAny>>,
    /// Its derive wrote nothing: the stored bytes are the page.
    plain: AtomicBool,
    disk: Mutex<Disk>,
    reading: Mutex<Option<Arc<ReadControl>>>,
    region: Mutex<Weak<varena::Region>>,
}

fn storage(error: tensorfs::io::source::Error) -> (varena::Error, Option<Failed>) {
    use tensorfs::io::source::Error as Source;
    let read = match error {
        Source::Source(failure) => {
            return (varena::Error::Callback(failure.to_string()), Some(Failed::Storage(failure)));
        }
        Source::Read(read) => read,
    };
    if read.source.kind() == std::io::ErrorKind::Interrupted {
        return (varena::Error::Busy, None);
    }
    if let Some(cuda) = read.source.get_ref().and_then(|e| e.downcast_ref::<io::cuda::Error>()) {
        return (device(cuda.clone()), None);
    }
    let failure = tensorfs::Failure::from(read.source);
    (varena::Error::Callback(failure.to_string()), Some(Failed::Storage(failure)))
}

fn device(error: io::cuda::Error) -> varena::Error {
    match error {
        io::cuda::Error::Allocation(error) => error,
        io::cuda::Error::Cuda { op, code, .. } => varena::Error::Cuda { operation: op, code },
        io::cuda::Error::CudaUnavailable(detail) => varena::Error::Unsupported(detail),
    }
}

/// Nothing went through this fill, or its read joined what did: no writer is left.
pub(super) fn decline(fill: varena::Fill) {
    // SAFETY: as above; nothing was published.
    let region = unsafe { fill.abort_completed() };
    let _ = region.release_unready(); // Refused under a capture; reclaim takes them then.
}

/// A failed read: only a drain proves its writers done, and nothing unproven is released.
pub(super) fn abort(fill: varena::Fill, error: varena::Error) -> varena::Error {
    if let Err(drain) = io::drain(fill.device()) {
        let detail = format!("fill completion is unproven: {drain}");
        fill.target().poison(detail.clone());
        std::mem::forget(fill);
        return varena::Error::Poisoned(detail);
    }
    decline(fill);
    error
}

pub(super) fn publish(fill: varena::Fill, consumer: Option<u64>) -> varena::Result<Option<varena::Pin>> {
    // SAFETY: the read returned, which proves every byte landed, padding included.
    let pin = unsafe { fill.commit_completed() }?;
    Ok(match consumer {
        Some(_) => Some(pin),
        None => {
            // SAFETY: the worker's pin has no consumer, and its writes are complete.
            drop(unsafe { pin.into_completed_region() });
            None
        }
    })
}

impl Inner {
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(super) fn set(&self) -> &str {
        &self.set
    }
    /// Its bytes are the stored ones (no derive, or one that wrote nothing): a fill of another set
    /// may copy them.
    pub(super) fn stored_bytes(&self) -> bool {
        self.stored.is_some() && (self.derive.is_none() || self.plain.load(Ordering::Acquire))
    }
    /// Its derive's bytes are kept: none are known to be the stored ones.
    pub(super) fn keeps(&self) -> bool {
        self.page.is_some() && self.derive.is_some() && !self.plain.load(Ordering::Acquire)
    }
    pub(super) fn region(&self) -> Weak<varena::Region> {
        self.region.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub(super) fn derives(&self) -> bool {
        self.derive.is_some() && !self.plain.load(Ordering::Acquire)
    }
    pub(super) fn page(&self) -> Option<&str> {
        self.page.as_deref()
    }

    /// Its slot's page `page` on disk, or with none the one used last, under whatever overlay.
    fn derived_page(&self, page: Option<&str>) -> Option<OnDisk> {
        let (slot, stored) = (self.slot.as_ref()?, self.stored.as_ref()?);
        let found = stored.checkpoint.derived(slot, page, true);
        let (derived, source) = found.inspect_err(|error| tracing::warn!("a derived page could not be looked up: {error}")).ok()??;
        if derived.object.length != self.bytes as u64 {
            return None;
        }
        let items = vec![Item {
            offset: 0,
            len: derived.object.length,
            source: ItemSource::Object(ObjectRange { off: 0, len: derived.object.length, obj: derived.object }),
            part_end: true,
        }];
        Some(OnDisk { page: derived.page, items, source })
    }

    /// The page its slot's block was derived as last, under whatever overlay: what a read ahead
    /// reads for it before its use's overlay is known.
    pub(super) fn slot_page(&self) -> Option<OnDisk> {
        self.derived_page(None)
    }

    /// Its own derived page on disk, made in an earlier process: what a fill reads instead of
    /// the store and its derive. Looked for once.
    fn on_disk(&self) -> Option<Arc<OnDisk>> {
        if self.derive.is_none() || self.page.is_none() {
            return None;
        }
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        if let Disk::Unknown = *disk {
            *disk = match self.derived_page(self.page.as_deref()) {
                Some(found) => Disk::Here(Arc::new(found)),
                None => Disk::Absent,
            };
        }
        match &*disk {
            Disk::Here(found) => Some(found.clone()),
            _ => None,
        }
    }

    /// Its derived page was written: the next look finds it.
    pub(super) fn persisted(&self) {
        *self.disk.lock().unwrap_or_else(|e| e.into_inner()) = Disk::Unknown;
    }

    /// The tier's name for its stored bytes read ahead (`Tier::read_ahead`), when they are not its page:
    /// a derive makes the page from them. Every overlay of its block shares it.
    pub(super) fn stored_key(&self) -> Option<String> {
        self.slot.as_ref().filter(|_| self.stored.is_some()).map(|slot| format!("stored:{slot}"))
    }

    /// Its stored bytes into `page` (a host page of its size), gaps zeroed.
    pub(super) fn read_stored(&self, page: *mut u8, threads: usize) -> Result<(), String> {
        let stored = self.stored.as_ref().ok_or("a page with no stored bytes")?;
        let dst = Dst::Host { ptr: page, len: self.bytes };
        // SAFETY: the tier's unshared page holds `bytes` bytes, which the items tile.
        unsafe { stored.local.read_items(&stored.items, 0, dst, threads, Mode::Cache, &IoTally::default()) }
            .map(drop).map_err(|error| error.to_string())
    }

    /// Its derived page from disk: read into a page the tier keeps, then one DMA; straight onto the
    /// card when the tier has no room.
    fn from_disk(&self, disk: &OnDisk, fill: &varena::Fill, ahead: bool) -> Result<(), tensorfs::io::source::Error> {
        let began = Instant::now();
        let key = self.page.as_ref().expect("a derived page is kept by its key");
        let page = self.tier.reserve(key, &self.set, self.bytes, true).ok().flatten();
        if let Some(page) = page {
            disk.read(Dst::Host { ptr: page.ptr(), len: self.bytes }, READERS)?;
            let page = self.tier.insert(key, &self.set, page);
            self.from_page(fill, &page)?;
            self.row(self.bytes as u64, "derived", ahead, began);
            return Ok(());
        }
        let control = Arc::new(ReadControl::default());
        *self.reading.lock().unwrap_or_else(|e| e.into_inner()) = Some(control.clone());
        let dst = Dst::Device { device: fill.device(), ptr: fill.ptr(), len: fill.len(), stream: 0 };
        let placements = [Placement { items: &disk.items, origin: 0, dst }];
        // SAFETY: the Fill is exclusive; `abort` drains before a failed fill's pages are released.
        let read = unsafe { disk.source.read_placements_cancelable(&placements, READERS, Mode::Cache, &IoTally::default(), &control) };
        *self.reading.lock().unwrap_or_else(|e| e.into_inner()) = None;
        read?;
        self.row(self.bytes as u64, "derived", ahead, began);
        Ok(())
    }

    /// A failed read: canceled, it declines; else it aborts, its failure kept for `use` to raise.
    fn failed(&self, fill: varena::Fill, error: varena::Error, failed: Option<Failed>, ahead: bool) -> varena::Error {
        if matches!(error, varena::Error::Busy) {
            self.tier.canceled();
            decline(fill); // A canceled read joined what it had submitted.
            return error;
        }
        if ahead {
            self.tier.failed();
        } else {
            FAILED.with_borrow_mut(|slot| *slot = failed);
        }
        abort(fill, error)
    }

    fn row(&self, bytes: u64, origin: &'static str, ahead: bool, began: Instant) {
        self.tier.row(Row {
            set: self.set.clone(), block: self.block, bytes, origin, ahead,
            ns: began.elapsed().as_nanos() as u64, at_ms: pages::now_ms(),
        });
    }

    /// DMA from its kept page, each registered window its own copy.
    fn from_page(&self, fill: &varena::Fill, page: &Page) -> Result<(), io::Error> {
        let copies: Vec<_> = page.windows().map(|(at, len)| io::Copy {
            // SAFETY: `at` lies inside the page.
            src: Src::Host(unsafe { page.ptr().add(at) }),
            dst: Dst::Device { device: fill.device(), ptr: fill.ptr() + at as u64, len, stream: 0 },
        }).collect();
        // SAFETY: the Fill is exclusive, and the caller holds the page through this read.
        unsafe { io::read(&copies, &ReadOpts::default()) }.map(drop)
    }

    /// Its stored bytes, each item from where it already is: another set's resident Region on this
    /// GPU (a device copy that waits for nothing), else that set's kept page (a pinned host copy),
    /// else the store. The items tile the Region in runs from one place, each gap zeroed by the
    /// run it falls in, and each place's bytes are a fill row of their own.
    fn read(self: &Arc<Self>, stored: &Stored, fill: &varena::Fill, ahead: bool) -> Result<(), tensorfs::io::source::Error> {
        let device = fill.device();
        let dst = |start: u64, end: u64| Dst::Device { device, ptr: fill.ptr() + start, len: (end - start) as usize, stream: 0 };
        let mut sources: Vec<(Arc<Inner>, Option<Bytes>)> = Vec::new();
        // Each item's first holder with its bytes on the card or in a kept page, each holder
        // looked up once.
        let from: Vec<Option<(usize, u64)>> = self.tier.holders(self, &stored.items).into_iter().map(|held| {
            held.into_iter().find_map(|Held { source, at }| {
                let index = sources.iter().position(|(known, _)| Arc::ptr_eq(known, &source)).unwrap_or_else(|| {
                    let bytes = self.bytes_of(&source, device);
                    sources.push((source, bytes));
                    sources.len() - 1
                });
                sources[index].1.is_some().then_some((index, at))
            })
        }).collect();
        let sorted = stored.items.windows(2).all(|pair| pair[0].offset + pair[0].len <= pair[1].offset);
        let found = sorted && from.iter().any(Option::is_some);
        // Runs of items from one place, (from the store, start, end, items), tiling the Region.
        let mut runs: Vec<(bool, u64, u64, std::ops::Range<usize>)> = Vec::new();
        if found {
            let mut cursor = 0;
            for (index, item) in stored.items.iter().enumerate() {
                let store = from[index].is_none();
                match runs.last_mut() {
                    Some(run) if run.0 == store => (run.2, run.3.end) = (item.offset + item.len, index + 1),
                    _ => runs.push((store, cursor, item.offset + item.len, index..index + 1)),
                }
                cursor = item.offset + item.len;
            }
            if let Some(run) = runs.last_mut() {
                run.2 = fill.len() as u64;
            }
        } else {
            runs.push((true, 0, fill.len() as u64, 0..stored.items.len()));
        }
        let (mut card, mut host, mut moved) = (Vec::new(), Vec::new(), [0u64; 2]);
        for (_, start, end, items) in runs.iter().filter(|run| !run.0) {
            let mut at = *start;
            for (index, item) in stored.items[items.clone()].iter().enumerate() {
                if item.offset > at {
                    card.push(io::Copy { src: Src::Zero, dst: dst(at, item.offset) });
                }
                let (source, held) = from[items.start + index].expect("a run of held items");
                match sources[source].1.as_ref().expect("held items' bytes") {
                    Bytes::Card(pin) => {
                        let src = Src::Device { device, ptr: pin.ptr() + held, stream: io::COMPLETE };
                        card.push(io::Copy { src, dst: dst(item.offset, item.offset + item.len) });
                        moved[0] += item.len;
                    }
                    Bytes::Host(page) => {
                        // No copy spans two of the page's registered windows: CUDA refuses it.
                        let mut done = 0;
                        while done < item.len {
                            let offset = (held + done) as usize;
                            let len = ((WINDOW - offset % WINDOW) as u64).min(item.len - done);
                            // SAFETY: inside the page, which `sources` holds through the copy.
                            let src = Src::Host(unsafe { page.ptr().add(offset) });
                            host.push(io::Copy { src, dst: dst(item.offset + done, item.offset + done + len) });
                            done += len;
                        }
                        moved[1] += item.len;
                    }
                }
                at = item.offset + item.len;
            }
            if *end > at {
                card.push(io::Copy { src: Src::Zero, dst: dst(at, *end) });
            }
        }
        let copied = (|| -> Result<(), io::Error> {
            for (copies, bytes, origin) in [(&card, moved[0], "resident"), (&host, moved[1], "host")] {
                if copies.is_empty() {
                    continue;
                }
                let began = Instant::now();
                // SAFETY: the Fill is exclusive and the copies disjoint; each source is a pinned
                // Region or a kept page that `sources` holds until the read proves them done.
                unsafe { io::read(copies, &ReadOpts::default()) }?;
                if bytes != 0 {
                    self.row(bytes, origin, ahead, began);
                }
            }
            Ok(())
        })();
        if copied.is_err() && io::drain(device).is_err() {
            std::mem::forget(sources); // Unproven: their pins and pages stay with the poisoned fill.
            return copied.map_err(Into::into);
        }
        for (_, bytes) in sources {
            if let Some(Bytes::Card(pin)) = bytes {
                // SAFETY: the read returned, or the drain did: the copies from it are complete.
                super::release_region(unsafe { pin.into_completed_region() });
            }
        }
        copied?;
        let placements: Vec<_> = runs.iter().filter(|run| run.0).map(|(_, start, end, items)| {
            Placement { items: &stored.items[items.clone()], origin: *start, dst: dst(*start, *end) }
        }).collect();
        if placements.is_empty() {
            return Ok(());
        }
        let began = Instant::now();
        let control = Arc::new(ReadControl::default());
        *self.reading.lock().unwrap_or_else(|e| e.into_inner()) = Some(control.clone());
        let mode = if fill.target().refilled() { Mode::Buffered } else { Mode::Cache };
        let source = if ahead { &stored.local } else { &stored.fetching };
        // SAFETY: the Fill is exclusive; `abort` drains before a failed fill's pages are released.
        let read = unsafe { source.read_placements_cancelable(&placements, READERS, mode, &IoTally::default(), &control) };
        *self.reading.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let stats = read?;
        self.row(stats.bytes, "store", ahead, began);
        self.tier.read_source(&self.set, self.block, &stats);
        Ok(())
    }

    /// Where `other`'s stored bytes are now: its Region on this GPU, pinned while ready (no use of
    /// it: a copied page keeps its reclaim order), else its kept page.
    fn bytes_of(&self, other: &Arc<Inner>, device: i32) -> Option<Bytes> {
        if let Some(region) = other.region().upgrade() {
            let pin = region.try_pin_unused().ok().flatten().filter(|pin| pin.device() == device);
            super::release_region(region);
            if let Some(pin) = pin {
                return Some(Bytes::Card(pin));
            }
        }
        let key = other.page.as_ref()?;
        self.tier.take(key, &other.set, other.bytes, false).map(Bytes::Host)
    }

    /// Bytes `at..at + len` of its stored bytes into a host page of `whole` bytes, gaps zeroed.
    pub(super) fn read_window(&self, page: *mut u8, whole: usize, at: usize, len: usize) -> Result<(), String> {
        let stored = self.stored.as_ref().ok_or("a page with no stored bytes")?;
        let window = window(&stored.items, at as u64, len as u64);
        debug_assert!(at + len <= whole);
        // SAFETY: the tier's unshared page holds `whole` bytes.
        let dst = Dst::Host { ptr: unsafe { page.add(at) }, len };
        // SAFETY: as above, and `window` lies inside it.
        unsafe { stored.local.read_items(&window, at as u64, dst, READERS, Mode::Buffered, &IoTally::default()) }
            .map(drop).map_err(|error| error.to_string())
    }

    /// The derive's bytes into a page the tier keeps, and that page: `spare` (its stored bytes'
    /// page, read ahead and served) when there is one, as no page is registered or let go on the
    /// fill's thread then; else one the tier admits.
    fn keep(&self, key: &str, py: Python<'_>, view: &Bound<'_, PyAny>, spare: Option<Page>) -> PyResult<Option<Arc<Page>>> {
        let began = Instant::now();
        let page = match spare {
            Some(page) => Some(page),
            None => {
                let page = py.detach(|| self.tier.reserve(key, &self.set, self.bytes, false))?;
                self.tier.reserve_ns.fetch_add(began.elapsed().as_nanos() as u64, Ordering::Relaxed);
                page
            }
        };
        let Some(page) = page else { return Ok(None) };
        // SAFETY: the memoryview ends with this call's objects, which the blocking copy outlives.
        let host = unsafe {
            Bound::from_owned_ptr_or_err(py, ffi::PyMemoryView_FromMemory(page.ptr().cast(), self.bytes as isize, ffi::PyBUF_WRITE))
        }?;
        let torch = py.import("torch")?;
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs.set_item("dtype", torch.getattr("uint8")?)?;
        let kept = torch.call_method("frombuffer", (host,), Some(&kwargs))?;
        for (at, len) in page.windows() {
            let window = pyo3::types::PySlice::new(py, at as isize, (at + len) as isize, 1);
            kept.get_item(&window)?.call_method1("copy_", (view.get_item(&window)?,))?;
        }
        let page = self.tier.insert(key, &self.set, page);
        self.tier.keep_ns.fetch_add(began.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Ok(Some(page))
    }

    /// The derive on this thread, which holds no lock, with the bytes so far as a Tensor. `spare`:
    /// the page its stored bytes were read ahead into, which keeps what it makes (or, when it
    /// makes nothing, its stored bytes).
    fn derive(
        &self, derive: &Py<PyAny>, fill: varena::Fill, consumer: u64, this: Weak<Inner>, spare: Option<Page>,
    ) -> varena::Result<varena::Pin> {
        let device = fill.device();
        let region = Arc::downgrade(fill.target());
        let mut spare = spare;
        Python::attach(|py| {
            let done = (|| -> PyResult<(bool, varena::Pin)> {
                let consumers = Consumers::new(device);
                consumers.declare(consumer).map_err(|error| super::raise(py, error))?;
                let used = Py::new(py, PyUse::filling(fill, consumers))?;
                let view = py.import("torch")?.call_method1("from_dlpack", (used.clone_ref(py),))?;
                let changed = derive.call1(py, (&view,))?.is_truthy(py)?;
                if changed || self.stored.is_none() {
                    if let Some(key) = &self.page {
                        let kept = self.keep(key, py, &view, spare.take())?;
                        if let (Some(_), Some(slot), Some(stored), true) = (kept, &self.slot, &self.stored, changed) {
                            self.tier.persist(Persisting {
                                checkpoint: stored.checkpoint.clone(), slot: slot.clone(), key: key.clone(),
                                set: self.set.clone(), len: self.bytes, source: this.clone(),
                            });
                        }
                    }
                }
                let pin = used.borrow(py).finish(py)?;
                Ok((changed, pin))
            })();
            match done {
                Ok((changed, pin)) => {
                    if !changed && self.stored.is_some() {
                        self.plain.store(true, Ordering::Release);
                        if let Some(key) = &self.page {
                            py.detach(|| self.tier.unreserve(key));
                            match spare.take() {
                                // Its stored bytes, read ahead, are its page.
                                Some(page) => drop(self.tier.insert(key, &self.set, page)),
                                None => self.tier.want(key, &self.set, this, region),
                            }
                        }
                    }
                    Ok(pin)
                }
                Err(error) => {
                    FAILED.with_borrow_mut(|failed| *failed = Some(Failed::Python(error)));
                    Err(varena::Error::Callback("a Source's derive failed".into()))
                }
            }
        })
    }
}

/// `items` cut to the byte range `at..at + len` of their destination.
fn window(items: &[Item], at: u64, len: u64) -> Vec<Item> {
    items.iter().filter_map(|item| {
        let start = item.offset.max(at);
        let end = (item.offset + item.len).min(at + len);
        (start < end).then(|| Item {
            offset: start,
            len: end - start,
            part_end: false,
            source: match &item.source {
                ItemSource::Object(range) => ItemSource::Object(ObjectRange {
                    obj: range.obj.clone(), off: range.off + (start - item.offset), len: end - start,
                }),
                ItemSource::Inline(bytes) => ItemSource::Inline(
                    bytes[(start - item.offset) as usize..(end - item.offset) as usize].into(),
                ),
            },
        })
    }).collect()
}

/// Where a held item's bytes are copied from: a pinned Region, or a kept page.
enum Bytes {
    Card(varena::Pin),
    Host(Arc<Page>),
}

pub(super) struct PageSource(pub(super) Arc<Inner>);

impl varena::Source for PageSource {
    fn ahead(&self) -> bool {
        let this = &self.0;
        this.page.as_ref().is_some_and(|key| this.tier.holds(key))
            || (this.stored.is_some() && (this.derive.is_none() || this.plain.load(Ordering::Acquire)))
            || this.on_disk().is_some()
    }

    fn stop(&self) {
        if let Some(control) = &*self.0.reading.lock().unwrap_or_else(|e| e.into_inner()) {
            control.cancel();
        }
    }

    fn fill(&self, fill: varena::Fill, consumer: Option<u64>) -> varena::Result<Option<varena::Pin>> {
        let this = &self.0;
        let began = Instant::now();
        let ahead = consumer.is_none();
        // A fill ahead counts once it goes on: one its caller makes again counts there.
        let kept = this.page.as_ref().and_then(|key| this.tier.take(key, &this.set, fill.len(), !ahead));
        let count = || if let (true, Some(key)) = (ahead, &this.page) { this.tier.count(key, &this.set) };
        if let Some(page) = kept {
            count();
            if let Err(error) = this.from_page(&fill, &page) {
                if ahead { this.tier.failed(); }
                let failure = tensorfs::Failure::from(error.source);
                let detail = failure.to_string();
                FAILED.with_borrow_mut(|failed| *failed = Some(Failed::Storage(failure)));
                return Err(abort(fill, varena::Error::Callback(detail)));
            }
            this.row(page.len() as u64, "baked", ahead, began);
            return publish(fill, consumer);
        }
        let derive = this.derive.as_ref().filter(|_| !this.plain.load(Ordering::Acquire));
        let disk = derive.and_then(|_| this.on_disk());
        if ahead && ((derive.is_some() && disk.is_none()) || this.stored.is_none()) {
            decline(fill); // Its page went since the core asked: this one needs its caller.
            return Err(varena::Error::Busy);
        }
        count();
        if let Some(disk) = disk {
            match this.from_disk(&disk, &fill, ahead) {
                Ok(()) => return publish(fill, consumer),
                Err(error) => {
                    let (error, failed) = storage(error);
                    // A page gone from disk, or unreadable, is made again: never a failed request.
                    if ahead || !matches!(error, varena::Error::Callback(_)) || io::drain(fill.device()).is_err() {
                        return Err(this.failed(fill, error, failed, ahead));
                    }
                    tracing::warn!("a derived page is made again: {error}");
                    *this.disk.lock().unwrap_or_else(|e| e.into_inner()) = Disk::Absent;
                }
            }
        }
        // A derive's stored bytes the tier read ahead (`Tier::read_ahead`), else the store's.
        let staged = derive.and(this.stored_key())
            .and_then(|key| this.tier.take(&key, &this.set, fill.len(), false).map(|page| (key, page)));
        if let (Some(key), Some(_), None) = (&this.page, derive, &staged) {
            this.tier.expect(key.clone(), this);
        }
        let mut spare = None;
        if let Some(stored) = &this.stored {
            let read: Result<(), tensorfs::io::source::Error> = match staged {
                Some((key, page)) => {
                    let read = this.from_page(&fill, &page).map_err(tensorfs::io::source::Error::from);
                    if read.is_ok() {
                        this.row(page.len() as u64, "host", ahead, began);
                        // Out of the tier, its page keeps what the derive makes.
                        spare = this.tier.take_out(&key, page);
                    } else {
                        drop(page);
                        this.tier.let_go(&key, "served");
                    }
                    read
                }
                None => this.read(stored, &fill, ahead),
            };
            if let Err(error) = read {
                let (error, failed) = storage(error);
                return Err(this.failed(fill, error, failed, ahead));
            }
        }
        let weak = Arc::downgrade(this);
        if let (Some(derive), Some(consumer)) = (derive, consumer) {
            return this.derive(derive, fill, consumer, weak, spare).map(Some);
        }
        if let Some(key) = &this.page {
            this.tier.want(key, &this.set, weak, Arc::downgrade(fill.target()));
        }
        publish(fill, consumer)
    }
}

fn items_of(checkpoint: &tensorfs::Checkpoint, parts: Vec<(String, usize, usize)>, bytes: usize) -> PyResult<Vec<Item>> {
    let invalid = |detail: &str| PyRuntimeError::new_err(detail.to_owned());
    let names: Vec<String> = parts.iter().map(|part| part.0.clone()).collect();
    let spans = checkpoint.spans(&[names]).map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    let mut by_name = BTreeMap::<String, Vec<_>>::new();
    for span in spans {
        by_name.entry(span.what.clone()).or_default().push(span);
    }
    let mut items = Vec::new();
    for (name, offset, length) in parts {
        let mut at = offset as u64;
        for span in by_name.get(&name).map_or(&[][..], Vec::as_slice) {
            items.push(Item { offset: at, len: span.len, source: span.source.clone(), part_end: false });
            at += span.len;
        }
        if at != (offset + length) as u64 || offset + length > bytes {
            return Err(invalid("a part's stored length differs from its place in the region"));
        }
        if let Some(item) = items.last_mut() {
            item.part_end = true;
        }
    }
    items.retain(|item| item.len != 0);
    Ok(items)
}

#[pyclass(name = "_VarenaSource", module = "tensorfs._ext", frozen)]
pub(super) struct PySource {
    pub(super) inner: Arc<Inner>,
}

#[pymethods]
impl PySource {
    /// `parts` are (component/key#role, offset, length). `page` names it in the tier. `derive`
    /// runs on `use`'s thread after any stored bytes land; true: its bytes are not the stored ones.
    /// `slot` (a digest, and `page` one too) names its block whatever the overlay: a derive's kept
    /// page is written to disk under it, and read from there by a later process.
    #[new]
    #[pyo3(signature = (pages, set, block, bytes, *, checkpoint = None, parts = Vec::new(), page = None, derive = None, slot = None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        pages: &PyPages,
        set: String,
        block: u32,
        bytes: usize,
        checkpoint: Option<PyRef<'_, PyCheckpoint>>,
        parts: Vec<(String, usize, usize)>,
        page: Option<String>,
        derive: Option<Py<PyAny>>,
        slot: Option<String>,
    ) -> PyResult<Self> {
        let stored = match checkpoint {
            Some(checkpoint) if !parts.is_empty() => {
                let core = checkpoint.core(py)?;
                let items = items_of(&core, parts, bytes)?;
                let (local, fetching) = pages.tier.stores(&core);
                Some(Stored { items, local, fetching, checkpoint: core })
            }
            _ => None,
        };
        let digest = |name: &Option<String>| name.as_deref().is_none_or(tensorfs::format::is_digest);
        if slot.is_some() && !(digest(&slot) && digest(&page) && page.is_some()) {
            return Err(PyRuntimeError::new_err("a Source's slot and page are sha256 digests"));
        }
        let inner = Arc::new(Inner {
            tier: pages.tier.clone(), page, slot, set, block, bytes, stored, derive,
            plain: AtomicBool::new(false), disk: Mutex::new(Disk::Unknown), reading: Mutex::new(None),
            region: Mutex::new(Weak::new()),
        });
        if let Some(stored) = &inner.stored {
            pages.tier.place(&inner, &stored.items);
        }
        Ok(Self { inner })
    }

    /// Its bytes in a writable host buffer: its kept page unless `stored`, else the store.
    #[pyo3(signature = (out, *, stored = false))]
    fn read_host(&self, py: Python<'_>, out: &Bound<'_, PyAny>, stored: bool) -> PyResult<()> {
        let view = py.import("builtins")?.getattr("memoryview")?.call1((out,))?;
        let buffer = pyo3::buffer::PyBuffer::<u8>::get(&view.call_method1("cast", ("B",))?)?;
        let (ptr, len) = (buffer.buf_ptr() as usize, buffer.len_bytes());
        let this = &self.inner;
        let page = this.page.as_ref().filter(|_| !stored).and_then(|key| this.tier.take(key, &this.set, len, false));
        if let Some(page) = page {
            // SAFETY: both hold `len` bytes, and the exporter keeps `out` alive through this.
            py.detach(|| unsafe { std::ptr::copy_nonoverlapping(page.ptr(), ptr as *mut u8, len) });
            return Ok(());
        }
        let stored = this.stored.as_ref().ok_or_else(|| PyRuntimeError::new_err("a Source with no stored bytes"))?;
        let dst = Dst::Host { ptr: ptr as *mut u8, len };
        // SAFETY: as above.
        py.detach(|| unsafe { stored.fetching.read_items(&stored.items, 0, dst, READERS, Mode::Buffered, &IoTally::default()) })
            .map(drop).map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

pub(super) fn bind(inner: &Arc<Inner>, region: &Arc<varena::Region>) {
    *inner.region.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(region);
}
