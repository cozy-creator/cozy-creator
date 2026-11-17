//! Bounded pinned host staging; every native device buffer belongs to Varena working memory.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{AcqRel, Relaxed}};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::Instant;

use super::cuda::{self, CUcontext, CUevent, CUstream, CtxGuard};
use super::{host, Copy, Dst, Error, Issued, ReadControl, ReadOpts, ReadStats, Src};

const PIECE: usize = 2 << 20;
const READERS: usize = 8;
/// Slots each reader owns: one fills while the other's copy lands, and both stay in its caches.
/// A shared pool (LIFO or FIFO) measured 8-20% slower warm H2D.
const RING: usize = 2;
const STAGING: usize = READERS * RING;
const STREAMS: usize = 4;

/// A test's faults (cr-318, `_io_fail_pieces`): staged pieces to let pass, then how many to fail.
#[cfg(feature = "fault-hooks")]
static FAULT_AFTER: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "fault-hooks")]
static FAULT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The next `count` staged pieces read on any device fail, after `after` more pass: a reader
/// failing mid-run, for the release gate's fault case. Dev builds only (`fault-hooks`).
#[cfg(feature = "fault-hooks")]
pub fn fail_pieces(after: usize, count: usize) {
    FAULT_AFTER.store(after, Relaxed);
    FAULT_COUNT.store(count, Relaxed);
}

#[cfg(feature = "fault-hooks")]
fn injected() -> bool {
    if FAULT_COUNT.load(Relaxed) == 0 {
        return false;
    }
    if FAULT_AFTER.fetch_update(AcqRel, Relaxed, |n| n.checked_sub(1)).is_ok() {
        return false;
    }
    FAULT_COUNT.fetch_update(AcqRel, Relaxed, |n| n.checked_sub(1)).is_ok()
}

fn failed(copy: usize, error: cuda::Error) -> Error {
    let kind = match error {
        cuda::Error::Cuda {
            code: cuda::ERROR_OUT_OF_MEMORY,
            ..
        } => io::ErrorKind::OutOfMemory,
        cuda::Error::Allocation(varena::Error::OutOfMemory { .. }) => io::ErrorKind::OutOfMemory,
        cuda::Error::Allocation(varena::Error::Unsupported(_)) | cuda::Error::CudaUnavailable(_) => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    Error {
        copy,
        source: io::Error::new(kind, error),
    }
}

pub(super) struct Event {
    ctx: CUcontext,
    raw: CUevent,
    timing: bool,
    release_failed: bool,
}

impl Event {
    pub(super) fn new(ctx: CUcontext, timing: bool) -> cuda::Result<Event> {
        let _ctx = CtxGuard::enter(ctx)?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is a writable out-pointer and the event's context is current.
        cuda::check("cuEventCreate", unsafe {
            (cuda::driver()?.event_create)(
                &mut raw,
                if timing {
                    cuda::EVENT_DEFAULT
                } else {
                    cuda::EVENT_DISABLE_TIMING
                },
            )
        })?;
        Ok(Event {
            ctx,
            raw,
            timing,
            release_failed: false,
        })
    }
    pub(super) fn record(&self, stream: CUstream) -> cuda::Result<()> {
        let _ctx = CtxGuard::enter(self.ctx)?;
        // SAFETY: a live event and stream of this context, which is current.
        cuda::check("cuEventRecord", unsafe {
            (cuda::driver()?.event_record)(self.raw, stream)
        })
    }
    fn sync(&self) -> cuda::Result<()> {
        let _ctx = CtxGuard::enter(self.ctx)?;
        // SAFETY: a live event of this context, which is current.
        cuda::check("cuEventSynchronize", unsafe {
            (cuda::driver()?.event_synchronize)(self.raw)
        })
    }
    pub(super) fn wait_on(&self, stream: CUstream) -> cuda::Result<()> {
        let _ctx = CtxGuard::enter(self.ctx)?;
        // SAFETY: both handles are retained in this context; record precedes every wait.
        cuda::check("cuStreamWaitEvent", unsafe {
            (cuda::driver()?.stream_wait_event)(stream, self.raw, 0)
        })
    }
    pub(super) fn release_checked(&mut self) -> cuda::Result<()> {
        if self.release_failed {
            return Err(cuda::Error::CudaUnavailable(
                "event release was already unproven".into(),
            ));
        }
        if self.raw.is_null() {
            return Ok(());
        }
        self.release_failed = true;
        let _ctx = CtxGuard::enter(self.ctx)?;
        // SAFETY: the sole owner destroys this event once; failure forbids any retry.
        cuda::check("cuEventDestroy(checked)", unsafe {
            (cuda::driver()?.event_destroy)(self.raw)
        })?;
        self.raw = std::ptr::null_mut();
        self.release_failed = false;
        Ok(())
    }
    fn since(&self, start: &Event) -> cuda::Result<u64> {
        let mut ms = 0.0f32;
        // SAFETY: `ms` is writable and both events are live.
        cuda::check("cuEventElapsedTime", unsafe {
            (cuda::driver()?.event_elapsed_time)(&mut ms, start.raw, self.raw)
        })?;
        Ok((ms as f64 * 1e6) as u64)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        if self.release_failed || self.raw.is_null() {
            return;
        }
        if let (Ok(d), Ok(_ctx)) = (cuda::driver(), CtxGuard::enter(self.ctx)) {
            // SAFETY: the event is live in this context, which is current; destroyed once, here.
            unsafe { (d.event_destroy)(self.raw) };
        }
    }
}

struct Slot {
    ptr: *mut u8,
    done: Event,
    pending: bool,
    recorded: bool,
}

impl Slot {
    fn new(ctx: CUcontext) -> cuda::Result<Slot> {
        let done = Event::new(ctx, false)?;
        let mut ptr = std::ptr::null_mut();
        // SAFETY: `ptr` is a writable out-pointer and the slot's context is current.
        cuda::check("cuMemHostAlloc", unsafe {
            (cuda::driver()?.mem_host_alloc)(&mut ptr, PIECE, cuda::MEMHOSTALLOC_PORTABLE)
        })?;
        Ok(Slot {
            ptr: ptr.cast(),
            done,
            pending: false,
            recorded: false,
        })
    }
}

// SAFETY: pinned host memory and an event of the engine's context, used only with it entered.
unsafe impl Send for Slot {}

impl Drop for Slot {
    fn drop(&mut self) {
        if let (Ok(d), Ok(_ctx)) = (cuda::driver(), CtxGuard::enter(self.done.ctx)) {
            if self.pending && (!self.recorded || self.done.sync().is_err()) {
                // The driver may still touch this allocation. Retain it until process exit
                // rather than free an unproved in-flight stage during unwinding.
                return;
            }
            // SAFETY: this slot allocated `ptr` and every copy using it has completed (checked above).
            unsafe { (d.mem_free_host)(self.ptr.cast()) };
        }
    }
}

/// The CPUs of the GPU's own NUMA node that this process may use. Readers run there and staging
/// is allocated there: on a two-socket EPYC, readers and slots left to the scheduler measured
/// 64-94% of the H2D ceiling from run to run, kept local 92-94%.
#[derive(Clone, Copy)]
struct Near(#[cfg(target_os = "linux")] Option<rustix::thread::CpuSet>);

impl Near {
    #[cfg(target_os = "linux")]
    fn of(device: i32) -> Near {
        let list = super::metrics::pci_address(device).and_then(|pci| {
            std::fs::read_to_string(format!("/sys/bus/pci/devices/{pci}/local_cpulist")).ok()
        });
        use rustix::thread::CpuSet;
        let (Some(list), Ok(allowed)) = (list, rustix::thread::sched_getaffinity(None)) else {
            return Near(None);
        };
        let mut near = CpuSet::new();
        for range in list.trim().split(',') {
            let (lo, hi) = range.split_once('-').unwrap_or((range, range));
            let (Ok(lo), Ok(hi)) = (lo.parse::<usize>(), hi.parse::<usize>()) else {
                return Near(None);
            };
            for cpu in (lo..=hi).filter(|&cpu| cpu < CpuSet::MAX_CPU && allowed.is_set(cpu)) {
                near.set(cpu);
            }
        }
        Near((near.count() > 0).then_some(near))
    }
    #[cfg(not(target_os = "linux"))]
    fn of(_: i32) -> Near {
        Near()
    }
    /// Moves this thread there (TensorFS's own threads only).
    fn enter(&self) {
        #[cfg(target_os = "linux")]
        if let Some(set) = &self.0 {
            let _ = rustix::thread::sched_setaffinity(None, set);
        }
    }
}

struct Engine {
    device: i32,
    ctx: CUcontext,
    /// Lane sets no run holds. A run takes one (or makes one) and gives it back, so its copies,
    /// its timing and its completion are its own, whatever runs beside it (cr-318).
    lanes: Mutex<Vec<Lanes>>,
    near: Near,
    free: Mutex<Vec<Slot>>,
    /// Told whenever slots go back to `free`: a run waiting for a ring takes one then.
    freed: Condvar,
    /// Shared by runs; exclusive for `recover` and `drain`, which assume no engine work in flight.
    serial: RwLock<()>,
    /// Runs holding staging rings now: with none, a run that finds no slot free never gets one.
    holding: AtomicUsize,
    /// The engine's reader threads, started as runs need them and kept.
    readers: Arc<Readers>,
    seen: Mutex<VecDeque<(usize, usize)>>,
    held: Mutex<Vec<Retained>>,
    /// Completed events of this context, reused: a run records a start and an end on every
    /// stream, and a wait for each foreign stream.
    events: Mutex<Vec<Event>>,
}

/// Completed events an engine keeps for reuse; more than this go.
const EVENTS: usize = 64;

/// A Varena working allocation. DLPack holders share it; completed decoder owners return it
/// directly. An unannotated final owner joins its primary context before permitting reuse.
pub struct DeviceBuffer {
    ptr: u64,
    len: usize,
    engine: Arc<Engine>,
    release_failed: bool,
}

impl DeviceBuffer {
    pub fn new(device: i32, len: usize) -> cuda::Result<DeviceBuffer> {
        let _producer = varena::enter_producer(device)?;
        let engine = Engine::get(device)?;
        let _ctx = CtxGuard::enter(engine.ctx)?;
        let ptr = cuda::allocate_ready(device, len)?;
        Ok(DeviceBuffer {
            ptr,
            len,
            engine,
            release_failed: false,
        })
    }
    pub fn ptr(&self) -> u64 {
        self.ptr
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn device(&self) -> i32 {
        self.engine.device
    }
    /// Free a completed native-owned allocation, checking the driver result.
    ///
    /// # Safety
    /// Every producer and consumer is checked complete; no alias may access the pointer.
    /// The caller excludes actual CUDA capture. On error retain this owner until process
    /// retirement; an ambiguous free must not be retried, including by Drop.
    pub(crate) unsafe fn release_completed(&mut self) -> cuda::Result<()> {
        if self.release_failed {
            return Err(cuda::Error::CudaUnavailable(
                "allocation release was already unproven".into(),
            ));
        }
        if self.ptr == 0 {
            return Ok(());
        }
        self.release_failed = true;
        // SAFETY: the caller certified every use complete and exclusively owns the pointer.
        unsafe { varena::working::free_completed(self.engine.device, self.ptr) }?;
        self.ptr = 0;
        self.len = 0;
        self.release_failed = false;
        Ok(())
    }
}

impl Drop for DeviceBuffer {
    fn drop(&mut self) {
        if self.ptr != 0 && !self.release_failed {
            // Unknown DLPack consumers may still run. Varena records retirement without CUDA
            // work, including during capture, and joins the context before reuse or unmap.
            // SAFETY: this is the final native owner; no further submissions may use its pointer.
            let _ = unsafe { varena::working::free_untracked(self.engine.device, self.ptr) };
        }
        // A failed metadata transition leaves the live block retained by Varena.
    }
}

#[derive(Default)]
struct Retained {
    slots: Vec<Slot>,
    pins: Vec<cuda::HostPin>,
    events: Vec<Event>,
    streams: Vec<cuda::Stream>,
    _buffers: Vec<DeviceBuffer>,
}

// A failed check leaves the actual owners in place, not just their pointer identities.
fn checked_take<T, E>(
    owners: &mut Vec<T>,
    check: impl FnOnce(&mut [T]) -> Result<(), E>,
) -> Result<Vec<T>, E> {
    check(owners)?;
    Ok(std::mem::take(owners))
}

// A reader may unwind before its normal ring return. Keep those actual slots in its run's
// retained set, never another run's, instead of running GPU-owning Slot destructors there.
struct ReaderRing<'a> {
    staged: &'a Mutex<Vec<Slot>>,
    slots: Vec<Slot>,
}
impl Drop for ReaderRing<'_> {
    fn drop(&mut self) {
        self.staged.lock().unwrap_or_else(|e| e.into_inner()).append(&mut self.slots);
    }
}

struct RunHold<'a> {
    engine: &'a Engine,
    owners: Option<Retained>,
    /// This run's slots a failed or unwound reader kept: they leave with this run's owners only.
    staged: &'a Mutex<Vec<Slot>>,
    lanes: &'a Lanes,
}

impl RunHold<'_> {
    fn take(&mut self) -> Retained {
        let mut owners = self.owners.take().expect("run owners taken once");
        owners
            .slots
            .append(&mut self.staged.lock().unwrap_or_else(|e| e.into_inner()));
        owners
    }

    fn finish(mut self, complete: bool) -> cuda::Result<()> {
        let owners = self.take();
        self.engine.release(vec![owners], complete, std::slice::from_ref(self.lanes))
    }
}

impl Drop for RunHold<'_> {
    fn drop(&mut self) {
        if self.owners.is_some() {
            let owners = self.take();
            self.engine
                .held
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(owners);
        }
    }
}

fn registry() -> &'static Mutex<BTreeMap<i32, Arc<Engine>>> {
    static ENGINES: OnceLock<Mutex<BTreeMap<i32, Arc<Engine>>>> = OnceLock::new();
    ENGINES.get_or_init(Mutex::default)
}

/// # Safety
/// As `cuda::pin_region`, whose context this enters.
pub(super) unsafe fn register(device: i32, ptr: *mut u8, len: usize) -> cuda::Result<cuda::HostPin> {
    let engine = Engine::get(device)?;
    let _ctx = CtxGuard::enter(engine.ctx)?;
    // SAFETY: this function's own contract.
    unsafe { cuda::pin_region(ptr, len, false) }
}

fn interrupted(copy: usize) -> Error {
    Error { copy, source: io::Error::new(io::ErrorKind::Interrupted, "native read canceled") }
}

/// Noninitializing: no driver/context is created when this device has no engine.
pub(super) fn drain(device: i32) -> cuda::Result<()> {
    let engine = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&device)
        .cloned();
    match engine {
        Some(engine) => {
            let _only = engine.serial.write().unwrap_or_else(|e| e.into_inner());
            engine.recover()
        }
        None => Ok(()),
    }
}

pub(super) fn retain_decode(stream: Option<cuda::Stream>, buffers: Vec<DeviceBuffer>) {
    let engine = buffers
        .first()
        .expect("decode retains its output")
        .engine
        .clone();
    engine
        .held
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Retained {
            streams: stream.into_iter().collect(),
            _buffers: buffers,
            ..Retained::default()
        });
}

// Handles are used only after entering their primary context; calls on a stream are ordered.
// SAFETY: handles are used only after entering their primary context, and calls on one stream are ordered by its lock.
unsafe impl Send for Engine {}
// SAFETY: as above.
unsafe impl Sync for Engine {}

impl Engine {
    fn get(device: i32) -> cuda::Result<Arc<Engine>> {
        let mut engines = registry().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(engine) = engines.get(&device) {
            return Ok(engine.clone());
        }
        let lanes = Lanes::new(device)?;
        let ctx = lanes.streams[0].context();
        let near = Near::of(device);
        // Pinned pages land on the allocating thread's node.
        let at = ctx as usize;
        let free = std::thread::scope(|s| {
            s.spawn(|| {
                near.enter();
                let ctx = at as CUcontext;
                let _ctx = CtxGuard::enter(ctx)?;
                (0..STAGING)
                    .map(|_| Slot::new(ctx))
                    .collect::<cuda::Result<Vec<_>>>()
            })
            .join()
            .expect("staging allocation does not panic")
        })?;
        let engine = Arc::new(Engine {
            device,
            ctx,
            lanes: Mutex::new(vec![lanes]),
            near,
            free: Mutex::new(free),
            freed: Condvar::new(),
            serial: RwLock::new(()),
            holding: AtomicUsize::new(0),
            readers: Arc::default(),
            seen: Mutex::new(VecDeque::new()),
            held: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        });
        engines.insert(device, engine.clone());
        Ok(engine)
    }

    fn reused(&self, ptr: usize, len: usize) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.contains(&(ptr, len)) {
            return true;
        }
        if seen.len() >= 256 {
            seen.pop_front();
        }
        seen.push_back((ptr, len));
        false
    }

    fn wait(&self, lanes: &Lanes, event: &Event, streams: usize) -> cuda::Result<()> {
        let _ctx = CtxGuard::enter(self.ctx)?;
        for stream in &lanes.streams[..streams] {
            // SAFETY: a live stream and event of this engine's context, which is current.
            cuda::check("cuStreamWaitEvent", unsafe {
                (cuda::driver()?.stream_wait_event)(stream.raw() as CUstream, event.raw, 0)
            })?;
        }
        Ok(())
    }

    fn synchronize(&self, owners: &[Retained], lanes: &[Lanes]) -> cuda::Result<()> {
        let mut first = None;
        for stream in lanes
            .iter()
            .flat_map(|l| &l.streams)
            .chain(owners.iter().flat_map(|r| &r.streams))
        {
            if let Err(error) = stream.drain() {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn checked_release(&self, owners: &mut [Retained], complete: bool, lanes: &[Lanes]) -> cuda::Result<()> {
        if !complete {
            self.synchronize(owners, lanes)?;
        }
        let mut first = None;
        for buffer in owners.iter_mut().flat_map(|r| &mut r._buffers) {
            // SAFETY: all private streams completed; these are quarantined private owners.
            if let Err(error) = unsafe { buffer.release_completed() } {
                first.get_or_insert(error);
            }
        }
        for pin in owners.iter_mut().flat_map(|r| &mut r.pins) {
            if let Err(error) = pin.release() {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn released(&self, owners: Vec<Retained>) {
        for mut owner in owners {
            for mut slot in owner.slots.drain(..) {
                slot.pending = false;
                slot.recorded = false;
                self.free().push(slot);
            }
            self.freed.notify_all();
            // Checked complete: its events are free to record again.
            let mut pool = self.events.lock().unwrap_or_else(|e| e.into_inner());
            for event in owner.events.drain(..).filter(|e| e.ctx == self.ctx) {
                if pool.len() < EVENTS {
                    pool.push(event);
                }
            }
            drop(pool);
            // The remaining event/stream/allocation owners drop only after checked completion.
            drop(owner);
        }
    }

    fn release(&self, mut owners: Vec<Retained>, complete: bool, lanes: &[Lanes]) -> cuda::Result<()> {
        match checked_take(&mut owners, |r| self.checked_release(r, complete, lanes)) {
            Ok(released) => {
                self.released(released);
                Ok(())
            }
            Err(error) => {
                self.held
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(owners);
                Err(error)
            }
        }
    }

    // Caller holds serial exclusively: no run is in flight, every lane set is idle.
    fn recover(&self) -> cuda::Result<()> {
        let lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let released = checked_take(&mut held, |r| self.checked_release(r, false, &lanes))?;
        drop(lanes);
        drop(held);
        // All previous engine work is now complete, including slots already returned to the free list.
        for slot in self.free().iter_mut() {
            slot.pending = false;
            slot.recorded = false;
        }
        self.released(released);
        Ok(())
    }

    /// An event of this context, `timing` or not: a completed one again, else a new one.
    fn event(&self, timing: bool) -> cuda::Result<Event> {
        let reused = {
            let mut pool = self.events.lock().unwrap_or_else(|e| e.into_inner());
            pool.iter().position(|e| e.timing == timing).map(|at| pool.swap_remove(at))
        };
        reused.map_or_else(|| Event::new(self.ctx, timing), Ok)
    }

    fn free(&self) -> std::sync::MutexGuard<'_, Vec<Slot>> {
        self.free.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Up to `wanted` rings of `RING` slots, as many as are free: none free, it waits for
    /// another run's, unless no run holds any (they are all retained by failed copies).
    fn rings(&self, wanted: usize) -> Option<Vec<Vec<Slot>>> {
        let mut free = self.free();
        loop {
            let n = wanted.min(free.len() / RING);
            if n > 0 {
                self.holding.fetch_add(1, AcqRel);
                return Some((0..n).map(|_| {
                    let at = free.len() - RING;
                    free.split_off(at)
                }).collect());
            }
            if self.holding.load(Relaxed) == 0 {
                return None;
            }
            free = self.freed.wait_timeout(free, std::time::Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// A lane set for one run: an idle one, else a new one.
    fn take_lanes(&self) -> cuda::Result<Lanes> {
        let idle = self.lanes.lock().unwrap_or_else(|e| e.into_inner()).pop();
        idle.map_or_else(|| Lanes::new(self.device), Ok)
    }

    /// `n` calls of `work` on the engine's reader threads, returned once every one has run. A
    /// reader that panicked panics here, as a scoped thread's join would.
    fn on_readers(&self, n: usize, work: &(dyn Fn() + Sync)) {
        let done = Arc::new((Mutex::new(0usize), Condvar::new()));
        let panicked = Arc::new(AtomicBool::new(false));
        // SAFETY: each task runs `work` once and counts itself done after it, and this function
        // returns only once all `n` have counted (unwinds included): the borrow outlives each use.
        let work: &'static (dyn Fn() + Sync) = unsafe {
            std::mem::transmute::<&(dyn Fn() + Sync), &'static (dyn Fn() + Sync)>(work)
        };
        for _ in 0..n {
            let (done, panicked) = (done.clone(), panicked.clone());
            self.readers.submit(Box::new(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
                    panicked.store(true, Relaxed);
                }
                let (count, all) = &*done;
                *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                all.notify_all();
            }));
        }
        let (count, all) = &*done;
        let mut finished = count.lock().unwrap_or_else(|e| e.into_inner());
        while *finished < n {
            finished = all.wait(finished).unwrap_or_else(|e| e.into_inner());
        }
        drop(finished);
        assert!(!panicked.load(Relaxed), "a TensorFS reader panicked");
    }

    fn run(
        self: &Arc<Self>,
        copies: &[(usize, Copy)],
        cold: &[bool],
        opts: &ReadOpts,
        staged_host: bool,
        issued: Option<Issued>,
        cancel: Option<&ReadControl>,
    ) -> Result<ReadStats, Error> {
        let first = copies[0].0;
        let canceled = || cancel.is_some_and(ReadControl::canceled);
        if canceled() {
            return Err(interrupted(first));
        }
        // Runs share the engine; a failed run's held owners are recovered with none in flight.
        let mut shared = self.serial.read().unwrap_or_else(|e| e.into_inner());
        while !self.held.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            drop(shared);
            {
                let _only = self.serial.write().unwrap_or_else(|e| e.into_inner());
                if !self.held.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                    self.recover().map_err(|e| failed(first, e))?;
                }
            }
            shared = self.serial.read().unwrap_or_else(|e| e.into_inner());
        }
        let _shared = shared;
        let _ctx = CtxGuard::enter(self.ctx).map_err(|e| failed(first, e))?;
        let streams = opts.streams.clamp(1, STREAMS);
        let lanes = LanesHold { engine: self, lanes: Some(self.take_lanes().map_err(|e| failed(first, e))?) };
        let staged = Mutex::new(Vec::new());
        let mut hold = RunHold {
            engine: self,
            owners: Some(Retained::default()),
            staged: &staged,
            lanes: lanes.get(),
        };
        let mut complete = false;
        let result = (|| {
            let owners = hold.owners.as_mut().expect("run owners live");
            let mut jobs = Vec::new();
            let tally = Tally::default();
            for (local, &(copy, c)) in copies.iter().enumerate() {
                if let Some(Src::Device { device, stream, .. }) = c.src.ordered() {
                    let source = Engine::get(device).map_err(|e| failed(copy, e))?;
                    owners
                        .events
                        .push(Event::new(source.ctx, false).map_err(|e| failed(copy, e))?);
                    let event = owners.events.last().expect("source event inserted");
                    event
                        .record(stream as CUstream)
                        .map_err(|e| failed(copy, e))?;
                    self.wait(lanes.get(), event, streams).map_err(|e| failed(copy, e))?;
                }
                if let Dst::Device { stream, .. } = c.dst {
                    if stream != 0 {
                        owners
                            .events
                            .push(self.event(false).map_err(|e| failed(copy, e))?);
                        let event = owners.events.last().expect("destination event inserted");
                        event
                            .record(stream as CUstream)
                            .map_err(|e| failed(copy, e))?;
                        self.wait(lanes.get(), event, streams).map_err(|e| failed(copy, e))?;
                    }
                }
                let mut direct = matches!(
                    (c.src, c.dst),
                    (Src::Device { .. } | Src::Zero, Dst::Device { .. })
                );
                if !staged_host {
                    if let (Src::Host(ptr), Dst::Device { .. }) = (c.src, c.dst) {
                        let reused = self.reused(ptr as usize, c.dst.len());
                        // SAFETY: `ptr` is the caller's live host source for this read (read's contract).
                        if unsafe { cuda::registered(ptr) }.map_err(|e| failed(copy, e))? {
                            direct = true;
                        } else if reused {
                            let start = Instant::now();
                            let pin =
                                // SAFETY: `[ptr, ptr+len)` is the caller's live host source until the copy completes (read's contract).
                                unsafe { cuda::pin_region(ptr.cast_mut(), c.dst.len(), true) };
                            tally
                                .pin_ns
                                .fetch_add(start.elapsed().as_nanos() as u64, Relaxed);
                            if let Ok(pin) = pin {
                                owners.pins.push(pin);
                                direct = true;
                            }
                        }
                    }
                }
                let step = if direct { c.dst.len() } else { PIECE };
                for at in (0..c.dst.len()).step_by(step.max(1)) {
                    jobs.push(Job {
                        copy,
                        local,
                        c,
                        at,
                        len: step.min(c.dst.len() - at),
                        direct,
                        cold: cold[copy],
                    });
                }
            }
            let start = owners.events.len();
            for stream in &lanes.get().streams[..streams] {
                owners
                    .events
                    .push(self.event(true).map_err(|e| failed(first, e))?);
                owners
                    .events
                    .last()
                    .expect("start event inserted")
                    .record(stream.raw() as CUstream)
                    .map_err(|e| failed(first, e))?;
            }
            let next = AtomicUsize::new(0);
            let stopped = AtomicBool::new(false);
            let error = Mutex::new(None);
            // Each copy's jobs not yet issued: the one that issues its last tells `issued`.
            let left: Vec<AtomicUsize> = (0..copies.len()).map(|_| AtomicUsize::new(0)).collect();
            jobs.iter().for_each(|job| _ = left[job.local].fetch_add(1, Relaxed));
            let raw: Vec<usize> = lanes.get().streams[..streams].iter().map(|s| s.raw()).collect();
            let done = |job: &Job| {
                if let Some(control) = cancel { control.issue(job.len); }
                if let Some(issued) = issued {
                    if left[job.local].fetch_sub(1, AcqRel) == 1 {
                        issued(job.copy, &raw);
                    }
                }
            };
            // Direct copies (device, zero or registered host sources) need no staging: this
            // thread issues them, round-robin over the streams, before any reader starts.
            let (direct, jobs): (Vec<Job>, Vec<Job>) = jobs.into_iter().partition(|job| job.direct);
            for (at, job) in direct.iter().enumerate() {
                if canceled() {
                    error.lock().unwrap().get_or_insert_with(|| interrupted(first));
                    stopped.store(true, Relaxed);
                    break;
                }
                if let Err(failure) = self.issue(lanes.get(), job, lane(at, streams)) {
                    error.lock().unwrap().get_or_insert(failure);
                    stopped.store(true, Relaxed);
                    break;
                }
                done(job);
            }
            // This run's rings, taken whole before any reader starts: no slot crosses runs. A failed
            // copy's slots stay retained until checked completion: read with the rest.
            let wanted = readers(jobs.len(), opts);
            let rings = match wanted {
                0 => Vec::new(),
                _ if stopped.load(Relaxed) => Vec::new(),
                _ => match self.rings(wanted) {
                    Some(rings) => rings,
                    None => {
                        let lost = io::Error::other("all staging is still retained by failed copies");
                        return Err(Error { copy: first, source: lost });
                    }
                },
            };
            let held_rings = !rings.is_empty();
            let readers = rings.len();
            let rings = Mutex::new(rings);
            let work = || {
                self.near.enter();
                let mut ring = ReaderRing {
                    staged: &staged,
                    slots: rings.lock().unwrap_or_else(|e| e.into_inner()).pop().expect("a ring for each reader"),
                };
                let mut turn = 0;
                while !stopped.load(Relaxed) && !canceled() {
                    let at = next.fetch_add(1, Relaxed);
                    let Some(job) = jobs.get(at) else { break };
                    turn += 1;
                    match self.piece(lanes.get(), job, at % streams, opts, &tally, &mut ring.slots[turn % RING]) {
                        Ok(()) => done(job),
                        Err(failure) => {
                            error.lock().unwrap().get_or_insert(failure);
                            stopped.store(true, Relaxed);
                        }
                    }
                }
                // After any failure a slot may still be in flight: kept with this run until its
                // checked completion. Else it goes back, and a run waiting for a ring is told.
                if !stopped.load(Relaxed) {
                    self.free().append(&mut ring.slots);
                    self.freed.notify_all();
                }
            };
            // Every reader is TensorFS's own thread, so moving it to the GPU's CPUs moves no caller.
            self.on_readers(readers, &work);
            if held_rings {
                self.holding.fetch_sub(1, AcqRel);
                self.freed.notify_all();
            }
            if canceled() {
                error.lock().unwrap().get_or_insert_with(|| interrupted(first));
            }
            let mut busy_ns = 0;
            let mut all_complete = true;
            for (lane, stream) in lanes.get().streams[..streams].iter().enumerate() {
                let drained = (|| {
                    owners.events.push(self.event(true)?);
                    let index = owners.events.len() - 1;
                    let end = &owners.events[index];
                    end.record(stream.raw() as CUstream)?;
                    end.sync()?;
                    end.since(&owners.events[start + lane])
                })();
                match drained {
                    Ok(ns) => busy_ns += ns,
                    Err(e) => {
                        all_complete = false;
                        error.lock().unwrap().get_or_insert(failed(first, e));
                    }
                }
            }
            complete = all_complete;
            let stats = ReadStats {
                direct: tally.direct.load(Relaxed),
                cached: tally.cached.load(Relaxed),
                bounced: tally.bounced.load(Relaxed),
                pin_ns: tally.pin_ns.load(Relaxed),
                staged: tally.staged.load(Relaxed),
                busy_ns,
                ..ReadStats::default()
            };
            if let Some(error) = error.into_inner().unwrap() {
                return Err(error);
            }
            Ok(stats)
        })();
        let released = hold.finish(complete).map_err(|e| failed(first, e));
        let stats = result?;
        released?;
        Ok(stats)
    }

    /// A direct copy, whole, on stream `lane`: a memset, a device copy, a peer copy or a copy
    /// from registered host memory. Nothing is staged.
    fn issue(&self, lanes: &Lanes, job: &Job, lane: usize) -> Result<(), Error> {
        let _ctx = CtxGuard::enter(self.ctx).map_err(|e| failed(job.copy, e))?;
        let d = cuda::driver().map_err(|e| failed(job.copy, e))?;
        let stream = lanes.streams[lane].raw() as CUstream;
        let _queue = lanes.queues[lane].lock().unwrap();
        let Dst::Device { ptr: dst, .. } = job.c.dst else {
            unreachable!("a direct copy has a device destination")
        };
        let result = match job.c.src {
            // SAFETY: the checked destination covers job.len bytes and remains owned until drain.
            Src::Zero => unsafe { (d.memset_d8_async)(dst, 0, job.len, stream) },
            // SAFETY: direct host sources are registered and retained through checked completion.
            Src::Host(ptr) => unsafe { (d.memcpy_htod_async)(dst, ptr.cast(), job.len, stream) },
            // SAFETY: both checked device spans belong to this context and outlive the copy.
            Src::Device { device, ptr, .. } if device == self.device => unsafe {
                (d.memcpy_dtod_async)(dst, ptr, job.len, stream)
            },
            Src::Device { device, ptr, .. } => {
                let source = Engine::get(device).map_err(|e| failed(job.copy, e))?;
                // SAFETY: both contexts and checked peer spans stay owned until completion.
                unsafe { (d.memcpy_peer_async)(dst, self.ctx, ptr, source.ctx, job.len, stream) }
            }
            Src::File(..) => unreachable!("a file source is staged"),
        };
        cuda::check("cuMemcpyAsync", result).map_err(|e| failed(job.copy, e))
    }

    fn piece(
        &self,
        lanes: &Lanes,
        job: &Job,
        lane: usize,
        opts: &ReadOpts,
        tally: &Tally,
        slot: &mut Slot,
    ) -> Result<(), Error> {
        let _ctx = CtxGuard::enter(self.ctx).map_err(|e| failed(job.copy, e))?;
        let d = cuda::driver().map_err(|e| failed(job.copy, e))?;
        let stream = lanes.streams[lane].raw() as CUstream;
        if slot.pending {
            slot.done.sync().map_err(|e| failed(job.copy, e))?;
            slot.pending = false;
        }
        match job.c.src {
            #[cfg(feature = "fault-hooks")]
            Src::File(..) if injected() => {
                let source = io::Error::other("an injected staged-piece fault (release gate)");
                return Err(Error { copy: job.copy, source });
            }
            Src::File(file, offset) => {
                let copy = Copy {
                    src: Src::File(file, offset + job.at as u64),
                    dst: Dst::Host {
                        ptr: slot.ptr,
                        len: job.len,
                    },
                };
                let options = ReadOpts {
                    threads: 1,
                    ..opts.clone()
                };
                // SAFETY: the staging slot is this engine's own and the source is the caller's (read's contract).
                let stats = unsafe { host::fill(&[copy], &[job.cold], &options, None) }.map_err(|e| Error {
                    copy: job.copy,
                    source: e.source,
                })?;
                tally.direct.fetch_add(stats.direct, Relaxed);
                tally.cached.fetch_add(stats.cached, Relaxed);
                tally.bounced.fetch_add(stats.bounced, Relaxed);
            }
            // SAFETY: the host source covers `job.at + job.len` bytes (read's contract) and the slot holds `PIECE >= job.len`.
            Src::Host(ptr) => unsafe {
                std::ptr::copy_nonoverlapping(ptr.add(job.at), slot.ptr, job.len)
            },
            Src::Device { .. } => {}
            Src::Zero => unreachable!("zero uses native device memset"),
        }
        let _queue = lanes.queues[lane].lock().unwrap();
        let rc = match (job.c.src, job.c.dst) {
            // SAFETY: the device destination covers `job.at + job.len` bytes and the slot stays pending until the event syncs.
            (_, Dst::Device { ptr, .. }) => unsafe {
                (d.memcpy_htod_async)(ptr + job.at as u64, slot.ptr.cast(), job.len, stream)
            },
            // SAFETY: the device source covers `job.at + job.len` bytes and the slot stays pending until the event syncs.
            (Src::Device { ptr, .. }, Dst::Host { .. }) => unsafe {
                (d.memcpy_dtoh_async)(slot.ptr.cast(), ptr + job.at as u64, job.len, stream)
            },
            _ => unreachable!(),
        };
        slot.pending = true;
        slot.recorded = false;
        cuda::check("cuMemcpyAsync", rc).map_err(|e| failed(job.copy, e))?;
        slot.done.record(stream).map_err(|e| failed(job.copy, e))?;
        slot.recorded = true;
        if let Dst::Host { ptr, .. } = job.c.dst {
            slot.done.sync().map_err(|e| failed(job.copy, e))?;
            slot.pending = false;
            // SAFETY: the copy into the slot completed (synced above); the host destination covers `job.at + job.len` bytes.
            unsafe { std::ptr::copy_nonoverlapping(slot.ptr, ptr.add(job.at), job.len) };
        }
        tally.staged.fetch_add(job.len as u64, Relaxed);
        Ok(())
    }
}

/// The stream the `at`-th direct copy of a run goes on: round-robin over the run's `streams`.
fn lane(at: usize, streams: usize) -> usize {
    at % streams.max(1)
}

/// Readers a run wants for its `staged` pieces: none without any, else as many as its threads
/// allow (`Engine::rings` gives it as many as the free staging holds).
fn readers(staged: usize, opts: &ReadOpts) -> usize {
    if staged == 0 { 0 } else { opts.threads.clamp(1, READERS).min(staged) }
}

/// One run's lanes: `STREAMS` streams, each with the lock its copies issue under.
struct Lanes {
    streams: Vec<cuda::Stream>,
    queues: Vec<Mutex<()>>,
}

// SAFETY: as the Engine's: its handles are used only after entering their primary context, and
// calls on one stream are ordered by its queue lock.
unsafe impl Send for Lanes {}
// SAFETY: as above.
unsafe impl Sync for Lanes {}

impl Lanes {
    fn new(device: i32) -> cuda::Result<Lanes> {
        Ok(Lanes {
            streams: (0..STREAMS).map(|_| cuda::Stream::new(device)).collect::<cuda::Result<_>>()?,
            queues: (0..STREAMS).map(|_| Mutex::new(())).collect(),
        })
    }
}

/// A run's lane set, back to the engine's idle ones when the run ends however it ends: anything
/// still in flight on it is waited for by the next run's own completion or an exclusive recover.
struct LanesHold<'a> {
    engine: &'a Engine,
    lanes: Option<Lanes>,
}

impl LanesHold<'_> {
    fn get(&self) -> &Lanes {
        self.lanes.as_ref().expect("a run's lanes")
    }
}

impl Drop for LanesHold<'_> {
    fn drop(&mut self) {
        if let Some(lanes) = self.lanes.take() {
            self.engine.lanes.lock().unwrap_or_else(|e| e.into_inner()).push(lanes);
        }
    }
}

type Task = Box<dyn FnOnce() + Send + 'static>;

/// An engine's reader threads: started as runs need them, kept, each pinned to the GPU's CPUs by
/// the work it runs. A run's tasks never wait on another task, so more threads only add overlap.
#[derive(Default)]
struct Readers {
    queue: Mutex<ReaderQueue>,
    wake: Condvar,
}

#[derive(Default)]
struct ReaderQueue {
    tasks: VecDeque<Task>,
    idle: usize,
}

impl Readers {
    fn submit(self: &Arc<Self>, task: Task) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.tasks.push_back(task);
        if queue.idle >= queue.tasks.len() {
            drop(queue);
            self.wake.notify_one();
            return;
        }
        drop(queue);
        let readers = Arc::clone(self);
        let started = std::thread::Builder::new().name("tensorfs-reader".into()).spawn(move || readers.work());
        if started.is_err() {
            // No thread: the caller runs what it queued, as a scope with no room would.
            while let Some(task) = self.queue.lock().unwrap_or_else(|e| e.into_inner()).tasks.pop_front() {
                task();
            }
        }
    }

    fn work(&self) {
        loop {
            let task = {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(task) = queue.tasks.pop_front() {
                        break task;
                    }
                    queue.idle += 1;
                    queue = self.wake.wait(queue).unwrap_or_else(|e| e.into_inner());
                    queue.idle -= 1;
                }
            };
            task();
        }
    }
}

struct Job<'a> {
    copy: usize,
    /// Its copy's place in the run.
    local: usize,
    c: Copy<'a>,
    at: usize,
    len: usize,
    direct: bool,
    /// Its file bytes read past the page cache.
    cold: bool,
}

#[derive(Default)]
struct Tally {
    direct: AtomicU64,
    cached: AtomicU64,
    bounced: AtomicU64,
    pin_ns: AtomicU64,
    staged: AtomicU64,
}

/// Same bounded B7 staging, with checked completion and retained native resources on error.
/// `cold` is by each copy's index (`super::cold`).
pub(super) unsafe fn fill(
    copies: &[(usize, Copy)],
    cold: &[bool],
    opts: &ReadOpts,
    staged_host: bool,
    issued: Option<Issued>,
    cancel: Option<&ReadControl>,
) -> Result<ReadStats, Error> {
    let mut groups: BTreeMap<(i32, &str), Vec<(usize, Copy)>> = BTreeMap::new();
    for &(index, copy) in copies {
        if copy.dst.is_empty() {
            continue;
        }
        let device = match (copy.src, copy.dst) {
            (_, Dst::Device { device, .. }) | (Src::Device { device, .. }, _) => device,
            _ => unreachable!(),
        };
        let kind = match (copy.src, copy.dst) {
            (Src::Zero, _) => "zero",
            (Src::Device { .. }, Dst::Host { .. }) => "d2h",
            (Src::Device { device: source, .. }, Dst::Device { device: target, .. })
                if source != target =>
            {
                "peer"
            }
            (Src::Device { .. }, Dst::Device { .. }) => "d2d",
            _ => "h2d",
        };
        groups
            .entry((device, kind))
            .or_default()
            .push((index, copy));
    }
    let mut total = ReadStats::default();
    // Zeroes first: a placement's padding is done before its file bytes go out, so the last of
    // those (`issued`) completes it.
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by_key(|((_, kind), _)| *kind != "zero");
    for ((device, kind), copies) in groups {
        let engine = Engine::get(device).map_err(|e| failed(copies[0].0, e))?;
        let start = Instant::now();
        let stats = engine.run(&copies, cold, opts, staged_host, issued, cancel)?;
        #[cfg(unix)]
        {
            let ns = start.elapsed().as_nanos() as u64;
            let plain: Vec<Copy> = copies.iter().map(|&(_, c)| c).collect();
            super::disk_run(&plain, &stats, ns);
            if kind == "h2d" {
                super::metrics::h2d_run(device, copies.iter().map(|(_, c)| c.dst.len() as u64).sum(), ns);
            }
        }
        #[cfg(not(unix))]
        let _ = (kind, start);
        total.direct += stats.direct;
        total.cached += stats.cached;
        total.bounced += stats.bounced;
        total.pin_ns += stats.pin_ns;
        total.staged += stats.staged;
        total.busy_ns += stats.busy_ns;
    }
    Ok(total)
}
