//! Private same-process ABI imported from Runtime's cozy_execution_v1.h.
//! The provider owns its context and graph; this consumer never sees Rust or C++ objects.
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::{ffi, prelude::*};

pub(super) const OK: u32 = 0;
pub(super) const CANCELED: u32 = 1;
pub(super) const INVALID: u32 = 2;
pub(super) const CAPACITY: u32 = 3;
pub(super) const CUDA: u32 = 4;
pub(super) const INTERNAL: u32 = 5;
pub(super) const UNPROVEN: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Status {
    pub kind: u32,
    pub code: i32,
    detail_len: u32,
    detail: [u8; 244],
}
impl Status {
    pub(super) fn new(kind: u32, code: i32, message: &str) -> Self {
        let mut status = Self {
            kind,
            code,
            detail_len: 0,
            detail: [0; 244],
        };
        let mut length = message.len().min(status.detail.len());
        while !message.is_char_boundary(length) {
            length -= 1;
        }
        status.detail[..length].copy_from_slice(&message.as_bytes()[..length]);
        status.detail_len = length as u32;
        status
    }

    pub fn unproven(detail: &str) -> Self {
        let mut status = Self {
            kind: UNPROVEN,
            code: 999,
            detail_len: 0,
            detail: [0; 244],
        };
        let len = detail.len().min(status.detail.len());
        status.detail[..len].copy_from_slice(&detail.as_bytes()[..len]);
        status.detail_len = len as u32;
        status
    }
    pub fn detail(&self) -> String {
        String::from_utf8_lossy(&self.detail[..(self.detail_len as usize).min(244)]).into_owned()
    }
    pub fn result(self) -> Result<(), Self> {
        if self.kind == OK { Ok(()) } else { Err(self) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Progress {
    pub phase: u32,
    pub completed_steps: u32,
    pub canceled: u32,
    pub done: u32,
    pub epoch: u64,
}

type Op = unsafe extern "C" fn(*mut c_void) -> Status;
type Observe = unsafe extern "C" fn(*mut c_void, *mut Progress) -> Status;
type StartAfter = unsafe extern "C" fn(*mut c_void, *const usize, u32, u64) -> Status;
#[repr(C)]
#[derive(Clone, Copy)]
struct Abi {
    abi_version: u32,
    struct_size: u32,
    flags: u64,
    device: i32,
    reserved: u32,
    pid: u64,
    context: *mut c_void,
    retain: Option<Op>,
    release: Option<Op>,
    start: Option<Op>,
    poll: Option<Observe>,
    cancel: Option<Op>,
    wait: Option<Observe>,
}
const _: [(); 256] = [(); std::mem::size_of::<Status>()];
const _: [(); 24] = [(); std::mem::size_of::<Progress>()];
#[cfg(target_pointer_width = "64")]
const _: [(); 88] = [(); std::mem::size_of::<Abi>()];

#[repr(C)]
struct EventAbi {
    base: Abi,
    start_after: Option<StartAfter>,
}

#[repr(C)]
struct ReusableAbi {
    event: EventAbi,
    rearm: Option<Op>,
    finish: Option<Op>,
}

pub(super) struct Continuation {
    abi: Abi,
    owner: Option<Py<PyAny>>,
    quiescent: AtomicBool,
    start_after: Option<StartAfter>,
    rearm: Option<Op>,
    finish: Option<Op>,
    finished: AtomicBool,
}
// SAFETY: v1 requires native-only thread-safe operations and a retained same-process context.
unsafe impl Send for Continuation {}
// SAFETY: start/cancel linearize in the provider; observers and retain/release are thread-safe.
unsafe impl Sync for Continuation {}

impl Continuation {
    pub fn import(value: &Bound<'_, PyAny>, device: i32) -> PyResult<Self> {
        let invalid =
            || pyo3::exceptions::PyValueError::new_err("invalid native execution continuation v1");
        // SAFETY: CPython validates the object and exact private capsule name before extraction.
        let ptr = unsafe {
            if ffi::PyCapsule_IsValid(value.as_ptr(), c"cozy.execution.continuation.v1".as_ptr())
                != 1
            {
                return Err(invalid());
            }
            ffi::PyCapsule_GetPointer(value.as_ptr(), c"cozy.execution.continuation.v1".as_ptr())
        };
        if ptr.is_null() || !(ptr as usize).is_multiple_of(std::mem::align_of::<Abi>()) {
            return Err(invalid());
        }
        // SAFETY: the named private capsule promises a live C header. Read the fixed prefix
        // before the remaining fields; no Rust object or function-pointer cast crosses the seam.
        let (version, size) = unsafe { (*(ptr.cast::<u32>()), *(ptr.cast::<u32>().add(1))) };
        if version != 1 || size < std::mem::size_of::<Abi>() as u32 {
            return Err(invalid());
        }
        // SAFETY: the provider's validated header covers this repr(C) value and stays owned below.
        let abi = unsafe { *ptr.cast::<Abi>() };
        if abi.flags & 7 != 7
            || abi.reserved != 0
            || abi.pid != std::process::id() as u64
            || abi.device != device
            || abi.context.is_null()
            || [abi.retain, abi.release, abi.start, abi.cancel]
                .iter()
                .any(Option::is_none)
            || abi.poll.is_none()
            || abi.wait.is_none()
        {
            return Err(invalid());
        }
        let start_after = if abi.flags & 8 != 0 {
            if size < std::mem::size_of::<EventAbi>() as u32 {
                return Err(invalid());
            }
            // SAFETY: the capability and checked full size cover this optional C tail.
            Some(unsafe { (*ptr.cast::<EventAbi>()).start_after }.ok_or_else(invalid)?)
        } else {
            None
        };
        let (rearm, finish) = if abi.flags & 16 != 0 {
            if size < std::mem::size_of::<ReusableAbi>() as u32 {
                return Err(invalid());
            }
            // SAFETY: flag16 plus full112-byte size covers the optional native tail.
            let tail = unsafe { &*ptr.cast::<ReusableAbi>() };
            (
                Some(tail.rearm.ok_or_else(invalid)?),
                Some(tail.finish.ok_or_else(invalid)?),
            )
        } else {
            (None, None)
        };
        let owner = value.clone().unbind();
        // SAFETY: validated v1 retain is a native-only operation on this provider's live context.
        let retained = unsafe { abi.retain.expect("validated retain")(abi.context) };
        if retained.kind != OK {
            // An erroneous retain does not certify whether the provider acquired ownership.
            std::mem::forget(owner);
            return Err(super::VarenaPoisonedError::new_err(retained.detail()));
        }
        Ok(Self {
            abi,
            owner: Some(owner),
            quiescent: AtomicBool::new(false),
            start_after,
            rearm,
            finish,
            finished: AtomicBool::new(false),
        })
    }
    pub fn supports_reusable_input(&self) -> bool {
        self.rearm.is_some() && self.finish.is_some()
    }
    /// A completed one-shot or explicitly finished reusable owner needs no further cancel.
    pub fn releasable(&self) -> bool {
        self.quiescent.load(Ordering::Acquire)
            && (!self.supports_reusable_input() || self.finished.load(Ordering::Acquire))
    }
    pub fn rearm(&self) -> Result<(), Status> {
        if self.finished.load(Ordering::Acquire) {
            return Err(Status::new(INVALID, 0, "graph input already finished"));
        }
        let rearm = self
            .rearm
            .ok_or_else(|| Status::new(INVALID, 0, "graph lacks reusable input"))?;
        self.quiescent.store(false, Ordering::Release);
        // SAFETY: the exclusive retained graph capability checks its completed epoch.
        unsafe { rearm(self.abi.context) }.result()
    }
    pub fn finish(&self) -> Result<(), Status> {
        if self.finished.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(finish) = self.finish {
            // SAFETY: retained provider checks final completion before releasing reservation.
            unsafe { finish(self.abi.context) }.result()?;
        }
        self.finished.store(true, Ordering::Release);
        self.quiescent.store(true, Ordering::Release);
        Ok(())
    }
    pub fn abandon(self: Arc<Self>) {
        self.cancel();
        crate::cleanup::defer(Abandon(Some(self)));
    }
    pub fn start(&self) -> Result<(), Status> {
        // SAFETY: same-process retained capability. The provider atomically arbitrates cancel/start.
        unsafe { self.abi.start.expect("validated start")(self.abi.context) }.result()
    }
    pub fn supports_event_input(&self) -> bool {
        self.start_after.is_some()
    }
    pub fn start_after(&self, streams: &[usize], error: u64) -> Result<(), Status> {
        let start = self
            .start_after
            .ok_or_else(|| Status::unproven("event input capability missing"))?;
        // SAFETY: the owned producer streams and error word stay live through composed
        // completion; the provider copies the temporary handle array during this call.
        unsafe {
            start(
                self.abi.context,
                streams.as_ptr(),
                streams.len() as u32,
                error,
            )
        }
        .result()
    }
    /// v1 cancel is atomic and nonblocking: safe even from a Python-side destructor.
    pub fn cancel(&self) -> Status {
        // SAFETY: retained v1 context; cancel performs no driver call, Python call or blocking wait.
        unsafe { self.abi.cancel.expect("validated cancel")(self.abi.context) }
    }
    pub fn observe(&self, wait: bool) -> (Status, Progress) {
        let mut progress = Progress::default();
        let observe = if wait { self.abi.wait } else { self.abi.poll }.expect("validated observer");
        // SAFETY: the context is retained and progress is writable C-layout storage for this call.
        let status = unsafe { observe(self.abi.context, &mut progress) };
        (status, progress)
    }
    pub fn checked(&self, status: Status, progress: Progress) -> bool {
        let complete =
            progress.done != 0 && matches!(status.kind, OK | CANCELED | INVALID | CAPACITY);
        if complete {
            self.quiescent.store(true, Ordering::Release);
        }
        complete
    }
    pub fn abort(&self) -> Result<(), Status> {
        self.cancel();
        let (status, progress) = self.observe(true);
        if self.checked(status, progress) {
            self.finish()
        } else {
            Err(Status::unproven(&format!(
                "native execution completion is unproven: {}",
                status.detail()
            )))
        }
    }
}

struct Release {
    abi: Abi,
    owner: Option<Py<PyAny>>,
}
// SAFETY: a checked-complete v1 context and its Python capsule remain retained through release.
unsafe impl Send for Release {}
impl Drop for Release {
    fn drop(&mut self) {
        // SAFETY: exactly one matching release for the successful retain, on the cleanup worker.
        let status = unsafe { self.abi.release.expect("validated release")(self.abi.context) };
        if status.kind != OK {
            // A failed release cannot authorize unloading the provider or dropping its owners.
            if let Some(owner) = self.owner.take() {
                std::mem::forget(owner);
            }
        }
    }
}
impl Drop for Continuation {
    fn drop(&mut self) {
        let owner = self.owner.take();
        if self.abi.pid != std::process::id() as u64 || !self.quiescent.load(Ordering::Acquire) {
            std::mem::forget(owner);
        } else {
            crate::cleanup::defer(Release {
                abi: self.abi,
                owner,
            });
        }
    }
}

struct Abandon(Option<Arc<Continuation>>);
impl Drop for Abandon {
    fn drop(&mut self) {
        if let Some(execution) = self.0.take() {
            if execution.abort().is_err() {
                std::mem::forget(execution);
            }
        }
    }
}
