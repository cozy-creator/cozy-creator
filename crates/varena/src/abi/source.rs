//! Concrete foreign Source custody. Callbacks own resources, never allocation policy.
use super::*;

pub type PrepareFn = unsafe extern "C" fn(*mut c_void, i32, u64, u64, *mut Job) -> i32;
pub type SubmitFn = unsafe extern "C" fn(*mut c_void, u64, u64) -> i32;
pub type RetireFn = unsafe extern "C" fn(*mut c_void) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Provider {
    pub prepare: Option<PrepareFn>,
    pub retire: Option<RetireFn>,
    pub drop: Option<DropFn>,
    pub context: *mut c_void,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Job {
    pub streams: *const u64,
    pub stream_count: usize,
    pub submit: Option<SubmitFn>,
    pub retire: Option<RetireFn>,
    pub drop: Option<DropFn>,
    pub context: *mut c_void,
}
// SAFETY: the provider contract requires thread-safe source callbacks and exclusive owned
// jobs, and retains native stream/source/code owners through checked retirement.
unsafe impl Send for Provider {}
unsafe impl Sync for Provider {}
unsafe impl Send for Job {}

pub(super) struct Owner {
    this: std::sync::Weak<Owner>,
    gpu: Arc<Gpu>,
    provider: Provider,
    pub(super) owned: AtomicBool,
    pid: u32,
}

pub(super) fn validate(provider: &Provider) -> Result<()> {
    if provider.prepare.is_none() || provider.retire.is_none() || provider.drop.is_none() {
        return Err(invalid(
            "Source prepare, checked retire and inert drop are required",
        ));
    }
    Ok(())
}
impl Owner {
    pub(super) fn new(gpu: Arc<Gpu>, provider: Provider) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            gpu,
            provider,
            owned: AtomicBool::new(false),
            pid: std::process::id(),
        })
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if !self.owned.load(Ordering::Acquire) || self.pid != std::process::id() {
            return;
        }
        // The retirer started with the Gpu. No callback occurs here.
        crate::retirement::defer(crate::retirement::Job::Source(Retirement {
            gpu: Some(self.gpu.clone()),
            provider: self.provider,
            active: true,
        }));
    }
}

/// Native queue owns the persistent provider, including every failed prepare resource.
pub(crate) struct Retirement {
    gpu: Option<Arc<Gpu>>,
    provider: Provider,
    active: bool,
}
impl Retirement {
    pub(crate) fn poll(&mut self) -> crate::Result<bool> {
        if !self.active {
            return Ok(true);
        }
        let gpu = self.gpu.as_ref().expect("Source retirement GPU").clone();
        let _retirement = match crate::enter_retirement(gpu.device()) {
            Ok(guard) => guard,
            Err(Error::CaptureActive | Error::Busy) => return Ok(false),
            Err(error) => return Err(error),
        };
        let context = gpu.core.driver().enter()?;
        let status =
            unsafe { self.provider.retire.expect("validated provider")(self.provider.context) };
        if status == BUSY {
            return Ok(false);
        }
        if status != OK {
            return Err(callback_error("Source retire", status));
        }
        // Contract: successful checked retirement left this destructor host-only/inert.
        unsafe {
            self.provider.drop.expect("validated provider")(self.provider.context);
        }
        self.active = false;
        drop(context);
        drop(self.gpu.take());
        drop(gpu);
        Ok(true)
    }
}
fn callback_error(stage: &str, status: i32) -> Error {
    let detail = format!("{stage} returned status {status}");
    if status == FAULT {
        Error::Poisoned(detail)
    } else {
        Error::Callback(detail)
    }
}
fn callback_failure(stage: &str, status: i32, bytes: u64) -> Failure {
    Failure(
        if (EXHAUSTED..=BUSY).contains(&status) {
            status
        } else {
            SOURCE_ERROR
        },
        format!("{stage} returned status {status}"),
        if status == EXHAUSTED { bytes } else { 0 },
    )
}

struct Resource {
    job: Option<Job>,
    _source: Arc<Owner>,
}
// SAFETY: preparation exposes no destination/work; submission closes every host writer.
// The native Fill proves actual producer completion before this checked resource callback.
unsafe impl crate::FillResource for Resource {
    fn retire(&mut self) -> crate::Result<crate::FillRetirement> {
        let Some(job) = &self.job else {
            return Ok(crate::FillRetirement::Complete);
        };
        let retire = job
            .retire
            .ok_or_else(|| Error::Poisoned("Source job lacks checked retire".into()))?;
        let drop = job
            .drop
            .ok_or_else(|| Error::Poisoned("Source job lacks inert drop".into()))?;
        let status = unsafe { retire(job.context) };
        if status == BUSY {
            return Ok(crate::FillRetirement::Pending);
        }
        if status != OK {
            return Err(callback_error("Source job retire", status));
        }
        // No GIL wait, CUDA destructor or ownership release is permitted after this guard.
        unsafe {
            drop(job.context);
        }
        self.job = None;
        Ok(crate::FillRetirement::Complete)
    }
}
impl Drop for Resource {
    fn drop(&mut self) {
        // Native quarantine retains this wrapper on failure. Never invoke foreign GPU
        // cleanup from an unexpected Rust unwinding/finalizer stack.
        if self.job.take().is_some() {
            // Keep the persistent provider alive too if an unexpected stack drops a
            // still-owned job; never run its final cleanup ahead of that job.
            std::mem::forget(self._source.clone());
        }
    }
}

impl crate::Source for Owner {
    fn fill(&self, fill: crate::Fill, consumer: Option<u64>) -> crate::Result<Option<Pin>> {
        let consumer = consumer.ok_or(Error::Busy)?; // A foreign Source runs only for its caller.
        match acquire(self, fill, consumer) {
            Ok(pin) => Ok(Some(pin)),
            Err(Failure(status, detail, bytes)) => Err(Error::Source { status, detail, bytes }),
        }
    }
}

fn acquire(source: &Owner, fill: crate::Fill, consumer_stream: u64) -> Result<Pin> {
    let source = source.this.upgrade().expect("a Source in use lives in its Region");
    let bytes = fill.len() as u64;
    let mut job = Job::default();
    let prepared = (|| -> Result<i32> {
        let _context = source.gpu.core.driver().enter()?;
        // No destination is exposed. Any failure transfers NOTHING; the persistent
        // provider retains its failed-prepare owners for its checked final retirement.
        Ok(unsafe {
            source.provider.prepare.expect("validated Source")(
                source.provider.context,
                source.gpu.device(),
                0,
                bytes,
                &mut job,
            )
        })
    })();
    let status = match prepared {
        Ok(OK) => OK,
        result => {
            // No prior or callback writer exists, including a refused context entry.
            let region = unsafe { fill.abort_completed() };
            let _ = region.release_unready()?;
            return Err(match result {
                Ok(status) => callback_failure("Source prepare", status, bytes),
                Err(error) => error,
            });
        }
    };
    debug_assert_eq!(status, OK);
    // Ownership transfers on successful prepare, before any fallible validation. A
    // malformed success is still retained/checked, never dropped as a raw C pointer.
    let resource = Resource {
        job: Some(job),
        _source: source.clone(),
    };
    let (streams, mut validation) = match unsafe { slice(job.streams, job.stream_count) } {
        Ok(streams) => (streams.to_vec(), Ok(())),
        Err(error) => (Vec::new(), Err(error)),
    };
    if validation.is_ok() {
        validation = if streams.is_empty() || streams.iter().any(|stream| *stream <= 2) {
            Err(invalid(
                "Source requires actual nonzero producer stream handles",
            ))
        } else if job.submit.is_none() || job.retire.is_none() || job.drop.is_none() {
            Err(invalid(
                "Source job submit, checked retire and inert drop are required",
            ))
        } else {
            Ok(())
        };
    }
    let mut session =
        unsafe { fill.into_async(crate::FillOwners::new(streams, vec![Box::new(resource)])) };
    validation?;
    let destination = session.begin()?;
    let status = {
        let _context = source.gpu.core.driver().enter()?;
        unsafe {
            job.submit.expect("validated job")(
                job.context,
                destination.ptr,
                destination.bytes as u64,
            )
        }
    };
    if status == FAULT {
        // A provider panic/unproven closure cannot be converted into a closed abort.
        // Writing Session Drop quarantines real owners, and its observer reports failure.
        return Err(callback_failure(
            "Source submit (closure unproven)",
            status,
            bytes,
        ));
    }
    if status != OK {
        unsafe {
            session.abort(callback_error("Source submit", status))?;
        }
        return Err(callback_failure("Source submit", status, bytes));
    }
    Ok(unsafe { session.publish_to(consumer_stream)? })
}
