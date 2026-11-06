//! Retained regions: state a model writes on the card once and reads every step (a prefix's
//! K/V). The core keeps one resident while there is room. A pinned host copy, taken on its own
//! stream behind compute, refills it: once that copy lands the core may evict the region and
//! fill it ahead of its next use, like a weight. Until then it stays pinned, so no byte is lost.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock, Weak};

use pyo3::{exceptions::PyValueError, prelude::*};
use tensorfs::io::{self, cuda, Dst, ReadOpts, Src};

use super::pages::{Page, PyPages, Tier};
use super::{raise, release_region, source, use_region, Deferred, PyDevice, PyUse};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

struct Retained {
    tier: Arc<Tier>,
    /// The one write: (offset, device address, bytes) of each tensor, taken by the first fill.
    writes: Mutex<Option<Vec<(u64, u64, usize)>>>,
    /// The host copy, once it landed.
    host: Mutex<Option<Arc<Page>>>,
    /// The region's pin when its copy failed: it stays on the card until its owner goes.
    held: Mutex<Option<varena::Pin>>,
}

impl Drop for Retained {
    fn drop(&mut self) {
        if let Some(page) = lock(&self.host).take().and_then(Arc::into_inner) {
            self.tier.spare(page);
        }
    }
}

struct RetainedSource(Arc<Retained>);

impl varena::Source for RetainedSource {
    fn ahead(&self) -> bool {
        lock(&self.0.host).is_some()
    }

    /// A demand fill copies on the consumer's stream, never through the reads ahead.
    fn reads(&self) -> bool {
        false
    }

    fn fill(&self, fill: varena::Fill, consumer: Option<u64>) -> varena::Result<Option<varena::Pin>> {
        let host = lock(&self.0.host).clone();
        match (host, consumer) {
            (Some(page), Some(consumer)) => {
                let len = fill.len();
                let copies: Vec<_> = page.windows().take_while(|(at, _)| *at < len)
                    .map(|(at, window)| Part::Host(at as u64, page.ptr() as u64 + at as u64, window.min(len - at))).collect();
                stream_fill(fill, consumer, &copies, Box::new(Holds(page))).map(Some)
            }
            (Some(page), None) => {
                if let Err(error) = refill(&fill, &page) {
                    return Err(source::abort(fill, varena::Error::Callback(error.to_string())));
                }
                source::publish(fill, None)
            }
            (None, Some(consumer)) => {
                let Some(writes) = lock(&self.0.writes).take() else {
                    source::decline(fill);
                    return Err(varena::Error::Invalid("a retained region's bytes are gone"));
                };
                let copies: Vec<_> = writes.iter().map(|&(at, from, len)| Part::Device(at, from, len)).collect();
                let pin = stream_fill(fill, consumer, &copies, Box::new(Holds(())))?;
                spill(&self.0, pin.try_clone()?, consumer);
                Ok(Some(pin))
            }
            (None, None) => {
                source::decline(fill);
                Err(varena::Error::Busy)
            }
        }
    }
}

/// What a stream-ordered fill's copies read, held until they complete.
struct Holds<T: Send + 'static>(T);
// SAFETY: retiring drops only host memory or nothing; the core retires it after its producers.
unsafe impl<T: Send + 'static> varena::FillResource for Holds<T> {
    fn retire(&mut self) -> varena::Result<varena::FillRetirement> {
        Ok(varena::FillRetirement::Complete)
    }
}

/// One copy into the region at an offset: from the card or from a registered host page.
enum Part {
    Device(u64, u64, usize),
    Host(u64, u64, usize),
}

/// The ahead worker's refill: one DMA per registered window of its host copy.
fn refill(fill: &varena::Fill, page: &Page) -> Result<(), io::Error> {
    let len = fill.len();
    let copies: Vec<_> = page.windows().take_while(|(at, _)| *at < len).map(|(at, window)| io::Copy {
        // SAFETY: `at` lies inside the page.
        src: Src::Host(unsafe { page.ptr().add(at) }),
        dst: Dst::Device { device: fill.device(), ptr: fill.ptr() + at as u64, len: window.min(len - at), stream: 0 },
    }).collect();
    // SAFETY: the Fill is exclusive, and the Retained holds the page through this read.
    unsafe { io::read(&copies, &ReadOpts::default()) }.map(drop)
}

/// `copies` into the region on the consumer's stream, published in that stream's order: the
/// caller's thread waits for nothing.
fn stream_fill(fill: varena::Fill, consumer: u64, copies: &[Part], holds: Box<dyn varena::FillResource>) -> varena::Result<varena::Pin> {
    let device = fill.device();
    // SAFETY: the consumer stream is the one writer; `holds` keeps the host source until it is done.
    let mut session = unsafe { fill.into_async(varena::FillOwners::new(vec![consumer], vec![holds])) };
    let destination = session.begin()?;
    let copied = (|| -> cuda::Result<()> {
        let driver = cuda::driver()?;
        let _context = cuda::CtxGuard::enter(spiller(device)?.context as cuda::CUcontext)?;
        let stream = consumer as cuda::CUstream;
        for copy in copies {
            // SAFETY: live device spans of this context, or a registered host page `holds` keeps;
            // the fill owns the destination.
            match *copy {
                Part::Device(at, from, len) => cuda::check("cuMemcpyDtoDAsync", unsafe {
                    (driver.memcpy_dtod_async)(destination.ptr + at, from, len, stream)
                })?,
                Part::Host(at, from, len) => cuda::check("cuMemcpyHtoDAsync", unsafe {
                    (driver.memcpy_htod_async)(destination.ptr + at, from as *const std::ffi::c_void, len, stream)
                })?,
            }
        }
        Ok(())
    })();
    if let Err(error) = copied {
        let cause = varena::Error::Callback(error.to_string());
        // SAFETY: nothing submits again; what was enqueued went on the consumer stream.
        let _ = unsafe { session.abort(cause.clone()) };
        return Err(cause);
    }
    // SAFETY: every byte is enqueued on the consumer, and nothing else writes.
    unsafe { session.publish_to(consumer) }
}

struct Job {
    retained: Weak<Retained>,
    pin: varena::Pin,
    /// Recorded on the writer's stream after its copies.
    ready: usize,
}

struct Spiller {
    context: usize,
    jobs: Mutex<mpsc::Sender<Job>>,
}

/// This device's spill worker: one stream, one copy at a time, never the caller's thread.
fn spiller(device: i32) -> cuda::Result<Arc<Spiller>> {
    static SPILLERS: OnceLock<Mutex<HashMap<i32, Arc<Spiller>>>> = OnceLock::new();
    let mut spillers = lock(SPILLERS.get_or_init(Mutex::default));
    if let Some(spiller) = spillers.get(&device) {
        return Ok(spiller.clone());
    }
    let (_, context) = cuda::primary_context(device)?;
    let (sender, jobs) = mpsc::channel();
    let raw = context as usize;
    std::thread::Builder::new().name(format!("varena-spill-{device}"))
        .spawn(move || run(device, raw, jobs))
        .map_err(|error| cuda::Error::CudaUnavailable(format!("spill worker could not start: {error}")))?;
    let spiller = Arc::new(Spiller { context: context as usize, jobs: Mutex::new(sender) });
    spillers.insert(device, spiller.clone());
    Ok(spiller)
}

static WARNED: AtomicBool = AtomicBool::new(false);

/// Its copy failed: the region keeps this pin, and stays on the card until its owner goes.
fn keep(retained: &Weak<Retained>, pin: varena::Pin, error: &str) {
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!("a retained region stays on the card: its host copy failed: {error}");
    }
    if let Some(retained) = retained.upgrade() {
        *lock(&retained.held) = Some(pin);
    }
}

/// Queue the host copy of a region just written on `consumer`.
fn spill(retained: &Arc<Retained>, pin: varena::Pin, consumer: u64) {
    let weak = Arc::downgrade(retained);
    let ready = (|| -> Result<(Arc<Spiller>, usize), String> {
        let spiller = spiller(pin.device()).map_err(|error| error.to_string())?;
        let driver = cuda::driver().map_err(|error| error.to_string())?;
        let _context = cuda::CtxGuard::enter(spiller.context as cuda::CUcontext).map_err(|error| error.to_string())?;
        let mut event = std::ptr::null_mut();
        // SAFETY: an out-parameter, then a live event and stream of this context.
        unsafe {
            cuda::check("cuEventCreate", (driver.event_create)(&mut event, cuda::EVENT_DISABLE_TIMING))
                .map_err(|error| error.to_string())?;
            if let Err(error) = cuda::check("cuEventRecord", (driver.event_record)(event, consumer as cuda::CUstream)) {
                (driver.event_destroy)(event);
                return Err(error.to_string());
            }
        }
        Ok((spiller, event as usize))
    })();
    let (spiller, ready) = match ready {
        Ok(ready) => ready,
        Err(error) => return keep(&weak, pin, &error),
    };
    let sent = lock(&spiller.jobs).send(Job { retained: weak, pin, ready });
    if let Err(mpsc::SendError(job)) = sent {
        keep(&job.retained, job.pin, "the spill worker is gone");
    }
}

fn run(device: i32, context: usize, jobs: mpsc::Receiver<Job>) {
    let stream = cuda::Stream::new(device);
    // The last copy's tier and size: idle, the worker makes the next one's page.
    let mut last: Option<(Arc<Tier>, usize)> = None;
    loop {
        let job = match jobs.try_recv() {
            Ok(job) => job,
            Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {
                if let Some((tier, len)) = last.take() {
                    tier.spare_ahead(len);
                }
                match jobs.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            }
        };
        last = job.retained.upgrade().map(|retained| (retained.tier.clone(), job.pin.len()));
        let copied = match &stream {
            Ok(stream) => copy(&job, stream, context),
            Err(error) => Err(error.to_string()),
        };
        if let Ok(driver) = cuda::driver() {
            // SAFETY: recorded by `spill`, waited on by this stream's copies, which finished or failed.
            unsafe { (driver.event_destroy)(job.ready as cuda::CUevent) };
        }
        match copied {
            Ok(page) => {
                match job.retained.upgrade() {
                    Some(retained) => *lock(&retained.host) = Some(Arc::new(page)),
                    None => drop(page),
                }
                // SAFETY: the writer finished before the copy began, and the copy has completed.
                release_region(unsafe { job.pin.into_completed_region() });
            }
            Err(error) => keep(&job.retained, job.pin, &error),
        }
    }
}

/// The region's bytes into a registered host page, after its writer, window by window.
fn copy(job: &Job, stream: &cuda::Stream, context: usize) -> Result<Page, String> {
    let len = job.pin.len();
    let tier = job.retained.upgrade().map(|retained| retained.tier.clone()).ok_or("its owner went")?;
    let page = tier.retained_page(len)?;
    let copied = (|| -> cuda::Result<()> {
        let driver = cuda::driver()?;
        let _context = cuda::CtxGuard::enter(context as cuda::CUcontext)?;
        let raw = stream.raw() as cuda::CUstream;
        // SAFETY: a live stream and an event recorded after the writer's copies.
        cuda::check("cuStreamWaitEvent", unsafe { (driver.stream_wait_event)(raw, job.ready as cuda::CUevent, 0) })?;
        for (at, window) in page.windows().take_while(|(at, _)| *at < len) {
            // SAFETY: the pin keeps the device span mapped; `at` lies inside the registered page.
            cuda::check("cuMemcpyDtoHAsync", unsafe {
                (driver.memcpy_dtoh_async)(page.ptr().add(at).cast(), job.pin.ptr() + at as u64, window.min(len - at), raw)
            })?;
        }
        stream.drain()
    })();
    match copied {
        Ok(()) => Ok(page),
        Err(error) => {
            // Unproven: the driver may still write the page, so it is never freed.
            std::mem::forget(page);
            Err(error.to_string())
        }
    }
}

#[pyclass(name = "_VarenaRetained", module = "tensorfs._ext", frozen)]
pub(super) struct PyRetained {
    region: Deferred<Arc<varena::Region>>,
    retained: Arc<Retained>,
}

impl Drop for PyRetained {
    fn drop(&mut self) {
        drop(lock(&self.retained.held).take());
        if let Some(region) = self.region.value.take() {
            if self.region.pid == std::process::id() {
                release_region(region);
            } else {
                std::mem::forget(region);
            }
        }
    }
}

#[pymethods]
impl PyRetained {
    /// A retained Region of `bytes` on `device`, its host copy in `pages`' memory.
    #[new]
    fn new(py: Python<'_>, device: PyRef<'_, PyDevice>, pages: PyRef<'_, PyPages>, bytes: usize) -> PyResult<Self> {
        let owner = device.owner()?;
        let retained = Arc::new(Retained {
            tier: pages.tier.clone(), writes: Mutex::new(None), host: Mutex::new(None), held: Mutex::new(None),
        });
        let source = Arc::new(RetainedSource(retained.clone()));
        let region = py.detach(|| owner.region(bytes, source).and_then(|region| region.set_retained().map(|()| region)))
            .map_err(|error| raise(py, error))?;
        Ok(Self { region: Deferred::new(region), retained })
    }

    /// Copy `parts`, (offset, device address, bytes) of live tensors ordered on `stream`, into the
    /// region on `stream`, pinned for it. Once, before any other use.
    fn write(&self, py: Python<'_>, stream: u64, parts: Vec<(u64, u64, usize)>) -> PyResult<PyUse> {
        if parts.iter().any(|&(at, _, len)| at as usize + len > self.region.len()) {
            return Err(PyValueError::new_err("a retained part lies past its region"));
        }
        *lock(&self.retained.writes) = Some(parts);
        use_region(py, &self.region, stream)
    }

    /// Pinned for `stream`: refilled from its host copy if it was evicted.
    fn use_on(&self, py: Python<'_>, stream: u64) -> PyResult<PyUse> {
        use_region(py, &self.region, stream)
    }

    /// Whether its host copy has landed.
    #[getter]
    fn landed(&self) -> bool {
        lock(&self.retained.host).is_some()
    }

    #[getter]
    fn resident(&self, py: Python<'_>) -> PyResult<bool> {
        py.detach(|| self.region.is_resident()).map_err(|error| raise(py, error))
    }
}
