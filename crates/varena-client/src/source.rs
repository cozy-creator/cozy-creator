//! Unsafe native providers with one owned job per cold Region fill.
use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRetirement {
    Complete,
    Pending,
}

/// A target-device native source. No Python/GIL callbacks are supported here.
///
/// # Safety
/// Prepare gets no destination and submits no GPU work. On error/panic this provider
/// retains every failed preparation resource until checked retirement. Jobs retain the
/// actual source, stream, staging and code/library owners. Normal Drop must be host-only
/// and preserve/quarantine unreleased GPU owners; successful retire leaves it inert.
/// All GPU resources belong to the target primary context or have independent equivalent
/// native custody. Callbacks must not reenter the same Region or wait for the GIL.
pub unsafe trait Source: Send + Sync + 'static {
    type Job: SourceJob;
    /// # Safety
    /// Called with target primary context current and capture/replay excluded.
    unsafe fn prepare(&self, device: i32, offset: u64, bytes: u64) -> Result<Self::Job>;
    /// # Safety
    /// Called under native retirement exclusion after every job/source user retired.
    /// Pending retains owners; Err is permanent; Complete leaves Drop inert.
    unsafe fn retire(&mut self) -> Result<SourceRetirement>;
}

/// One exclusive producer submission and checked resource owner.
///
/// # Safety
/// Streams are fixed actual handles in the target primary context and their true owners
/// remain retained. Submit initializes every byte on success and closes ALL host submitters
/// on every normal return, including errors. No escaped view lacks its own native Pin.
/// Drop is host-only and preserves unreleased resources; retire is the sole GPU cleanup.
pub unsafe trait SourceJob: Send + 'static {
    fn streams(&self) -> &[Stream];
    /// # Safety
    /// dst is exclusively writable for bytes under native producer exclusion. Return only
    /// after all host submission stops; an error may leave partial, unpublished bytes.
    unsafe fn submit(&mut self, dst: u64, bytes: u64) -> Result<()>;
    /// # Safety
    /// Native producer events proved completion and retirement exclusion is held. No GIL
    /// wait is permitted. Complete leaves Drop inert; Pending/Err retain all owners.
    unsafe fn retire(&mut self) -> Result<SourceRetirement>;
}

struct Provider<S: Source> {
    value: Option<S>,
    failed: AtomicBool,
    failed_jobs: Mutex<Vec<Job<S::Job>>>,
}
struct Job<J: SourceJob> {
    value: Option<J>,
    streams: Vec<u64>,
    failed: bool,
}
fn code(error: Error) -> i32 {
    match error.status {
        Status::Exhausted => 1,
        Status::Capturing => 2,
        Status::SourceError => 3,
        Status::Fault => 4,
        Status::Invalid => 5,
        Status::Unsupported => 6,
        Status::Busy => 7,
        Status::Unknown(value) if value != 0 => value,
        Status::Unknown(_) => 3,
    }
}
fn permanent(error: Error) -> i32 {
    let status = code(error);
    if status == 7 { 3 } else { status } // Pending is explicit, never inferred from an error.
}

unsafe extern "C" fn prepare<S: Source>(
    context: *mut c_void,
    device: i32,
    offset: u64,
    bytes: u64,
    out: *mut ffi::SourceJob,
) -> i32 {
    if context.is_null() || out.is_null() {
        return 5;
    }
    let owner = unsafe { &*(context as *const Provider<S>) };
    if owner.failed.load(Ordering::Acquire) {
        return 4;
    }
    unsafe {
        out.write(ffi::SourceJob::default());
    }
    let mut prepared = None;
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        prepared = Some(unsafe {
            owner
                .value
                .as_ref()
                .expect("live Source")
                .prepare(device, offset, bytes)?
        });
        let job = prepared.as_ref().expect("prepared job");
        if job.streams().is_empty()
            || job
                .streams()
                .iter()
                .any(|stream| stream.raw <= 2 || stream.gpu.device() != device)
        {
            return Err(invalid(
                "Source requires actual target-device producer streams",
            ));
        }
        let streams = job.streams().iter().map(Stream::raw).collect::<Vec<_>>();
        let job = Box::new(Job {
            value: prepared.take(),
            streams,
            failed: false,
        });
        let result = ffi::SourceJob {
            streams: job.streams.as_ptr(),
            stream_count: job.streams.len(),
            submit: Some(submit::<S::Job>),
            retire: Some(retire_job::<S::Job>),
            drop: Some(drop_job::<S::Job>),
            context: Box::into_raw(job).cast(),
        };
        unsafe {
            out.write(result);
        }
        Ok(())
    }));
    match result {
        Ok(Ok(())) => 0,
        other => {
            if let Some(job) = prepared {
                owner
                    .failed_jobs
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(Job {
                        value: Some(job),
                        streams: Vec::new(),
                        failed: false,
                    });
            }
            match other {
                Ok(Err(error)) => code(error),
                _ => {
                    owner.failed.store(true, Ordering::Release);
                    4
                }
            }
        }
    }
}
unsafe extern "C" fn submit<J: SourceJob>(context: *mut c_void, dst: u64, bytes: u64) -> i32 {
    let job = unsafe { &mut *(context as *mut Job<J>) };
    if job.failed {
        return 4;
    }
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        job.value.as_mut().expect("owned job").submit(dst, bytes)
    })) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => code(error),
        Err(_) => {
            job.failed = true;
            4
        }
    }
}
fn finish_job<J: SourceJob>(job: &mut Job<J>) -> i32 {
    if job.failed {
        return 4;
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<SourceRetirement> {
        let Some(value) = job.value.as_mut() else {
            return Ok(SourceRetirement::Complete);
        };
        let state = unsafe { value.retire()? };
        if state == SourceRetirement::Complete {
            drop(job.value.take());
        }
        Ok(state)
    }));
    match result {
        Ok(Ok(SourceRetirement::Complete)) => 0,
        Ok(Ok(SourceRetirement::Pending)) => 7,
        Ok(Err(error)) => {
            job.failed = true;
            permanent(error)
        }
        Err(_) => {
            job.failed = true;
            4
        }
    }
}
unsafe extern "C" fn retire_job<J: SourceJob>(context: *mut c_void) -> i32 {
    finish_job(unsafe { &mut *(context as *mut Job<J>) })
}
unsafe extern "C" fn drop_job<J: SourceJob>(context: *mut c_void) {
    let job = unsafe { Box::from_raw(context as *mut Job<J>) };
    if job.value.is_some() {
        std::mem::forget(job);
    } // A contract violation never runs GPU Drop here.
}
unsafe extern "C" fn retire_source<S: Source>(context: *mut c_void) -> i32 {
    let source = unsafe { &mut *(context as *mut Provider<S>) };
    if source.failed.load(Ordering::Acquire) {
        return 4;
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<SourceRetirement> {
        // Final Source custody is exclusive. No mutex guard crosses an external callback.
        let jobs = source
            .failed_jobs
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        let mut index = 0;
        let mut pending = false;
        while index < jobs.len() {
            match finish_job(&mut jobs[index]) {
                0 => {
                    jobs.swap_remove(index);
                }
                7 => {
                    pending = true;
                    index += 1;
                }
                _ => {
                    return Err(Error {
                        status: Status::Fault,
                        message: "failed preparation cleanup is unproven".into(),
                        requested_bytes: 0,
                    });
                }
            }
        }
        if pending {
            return Ok(SourceRetirement::Pending);
        }
        let Some(value) = source.value.as_mut() else {
            return Ok(SourceRetirement::Complete);
        };
        let state = unsafe { value.retire()? };
        if state == SourceRetirement::Complete {
            drop(source.value.take());
        }
        Ok(state)
    }));
    match result {
        Ok(Ok(SourceRetirement::Complete)) => 0,
        Ok(Ok(SourceRetirement::Pending)) => 7,
        Ok(Err(error)) => {
            source.failed.store(true, Ordering::Release);
            permanent(error)
        }
        Err(_) => {
            source.failed.store(true, Ordering::Release);
            4
        }
    }
}
unsafe extern "C" fn drop_source<S: Source>(context: *mut c_void) {
    let source = unsafe { Box::from_raw(context as *mut Provider<S>) };
    if source.value.is_some()
        || !source
            .failed_jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    {
        std::mem::forget(source);
    }
}

impl Gpu {
    /// Register a native Source. Refusal returns the original provider; it is not dropped
    /// or silently transferred. The provider's unsafe contract remains its responsibility.
    pub fn region_from_source<S: Source>(
        &self,
        bytes: u64,
        source: S,
    ) -> std::result::Result<Region, (S, Error)> {
        let owner = Box::into_raw(Box::new(Provider {
            value: Some(source),
            failed: AtomicBool::new(false),
            failed_jobs: Mutex::new(Vec::new()),
        }));
        let provider = ffi::SourceProvider {
            prepare: Some(prepare::<S>),
            retire: Some(retire_source::<S>),
            drop: Some(drop_source::<S>),
            context: owner.cast(),
        };
        let mut handle = 0;
        if let Err(error) = check(unsafe {
            ffi::vr_v1_region_from_source(
                self.0.handle,
                bytes,
                &provider,
                std::mem::size_of::<ffi::SourceProvider>(),
                &mut handle,
            )
        }) {
            let mut owner = unsafe { Box::from_raw(owner) };
            // Constructor never calls prepare; failed construction retained our exact owner.
            return Err((owner.value.take().expect("untransferred Source"), error));
        }
        Ok(Region(Arc::new(RegionInner {
            gpu: self.clone(),
            handle,
        })))
    }
}
