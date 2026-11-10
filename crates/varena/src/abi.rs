//! Public C ABI v1. This module adapts ownership, not allocation policy.
#![allow(unsafe_code)]

pub(crate) mod source;
mod host_source;
mod host_parts;
mod sizing;
mod tensor_export;
#[doc(hidden)]
pub use tensor_export::{TensorExport, export_use};
pub use source::{Job as SourceJob, Provider as SourceProvider};

use crate::retirement;
use crate::{Device, Error, Pin, Region, cache_owner, working};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub const VERSION: u32 = 1;
pub const OK: i32 = 0;
pub const EXHAUSTED: i32 = 1;
pub const CAPTURING: i32 = 2;
pub const SOURCE_ERROR: i32 = 3;
pub const FAULT: i32 = 4;
pub const INVALID: i32 = 5;
pub const UNSUPPORTED: i32 = 6;
pub const BUSY: i32 = 7;

#[derive(Debug)]
struct Failure(i32, String, u64);
type Result<T> = std::result::Result<T, Failure>;
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        let (status, requested) = match &error {
            Error::OutOfMemory { requested_bytes }
            | Error::CaptureMap { requested_bytes }
            | Error::Exhausted {
                requested_bytes, ..
            } => (EXHAUSTED, *requested_bytes as u64),
            Error::CaptureActive => (CAPTURING, 0),
            Error::DeviceFault { .. } | Error::Poisoned(_) => (FAULT, 0),
            Error::Unsupported(_) => (UNSUPPORTED, 0),
            Error::Busy => (BUSY, 0),
            Error::Callback(_) => (SOURCE_ERROR, 0),
            Error::Source { status, bytes, .. } => (*status, *bytes),
            Error::Cuda { .. } | Error::Invalid(_) | Error::PhaseOverlap => (INVALID, 0),
        };
        Self(status, error.to_string(), requested)
    }
}
fn invalid(message: &str) -> Failure {
    Failure(INVALID, message.into(), 0)
}

thread_local! {
    static LAST: RefCell<(CString, u64)> = RefCell::new((CString::default(), 0));
}
static ABI_FAULT: AtomicBool = AtomicBool::new(false);
fn process() -> Result<()> {
    if ABI_FAULT.load(Ordering::Acquire) {
        return Err(Failure(
            FAULT,
            "Varena ABI fault is sticky; retire this process".into(),
            0,
        ));
    }
    crate::check_process()?;
    Ok(())
}
fn boundary(f: impl FnOnce() -> Result<()>) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(f));
    let error = match result {
        Ok(Ok(())) => return OK,
        Ok(Err(error)) => error,
        Err(_) => Failure(
            FAULT,
            "panic contained at Varena C ABI boundary; retire this process".into(),
            0,
        ),
    };
    if error.0 == FAULT {
        ABI_FAULT.store(true, Ordering::Release);
    }
    LAST.with(|last| {
        *last.borrow_mut() = (CString::new(error.1.replace('\0', "?")).unwrap(), error.2)
    });
    error.0
}

/// Identity is the address of the real core registry, not a frontend-local counter.
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_core_identity() -> u64 {
    &crate::DEVICES as *const _ as usize as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_abi_version() -> u32 {
    VERSION
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_last_error() -> *const c_char {
    LAST.with(|v| v.borrow().0.as_ptr())
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_error_requested_bytes() -> u64 {
    LAST.with(|v| v.borrow().1)
}

/// A foreign finalizer's last reference, destroyed on the retirer's stack.
pub(crate) enum Owned {
    Weight { _weight: Arc<Weight> },
    Tensor { _pin: Pin, _consumers: crate::tensor_consumers::Consumers, _weight: Arc<Weight> },
}

pub(crate) fn note_failure(error: &Error) {
    if Failure::from(error.clone()).0 == FAULT {
        ABI_FAULT.store(true, Ordering::Release);
    }
}

pub(crate) fn fault() {
    ABI_FAULT.store(true, Ordering::Release);
}

/// A finalizer's close: only this path retries temporary refusals. Callback drain is polled,
/// never waited on, so one busy callback cannot stall other jobs.
pub(crate) fn retire_handle(id: u64) -> crate::Result<bool> {
    match close_handle(id, true) {
        Ok(()) => Ok(true),
        Err(Failure(CAPTURING | BUSY, _, _)) => Ok(false),
        Err(error) => Err(Error::Poisoned(error.1)),
    }
}

pub(crate) fn handle_failed(id: u64, error: &Error) {
    let Ok(mut handles) = registry_unchecked() else { return };
    let Some(entry) = handles.get_mut(&id) else { return };
    entry.disposition = Disposition::Quarantined;
    entry.failure = Failure::from(error.clone()).0;
    if let Handle::Cache(cache) = &entry.value {
        let cache = cache.clone();
        drop(handles);
        retirement::abandoned(cache.storage.stats().map_or(0, |stats| stats.live_bytes as u64));
    }
}

fn defer_handle(id: u64) -> Result<()> {
    crate::check_process()?; // reject fork before inherited registry locks
    retirement::prepare()?;
    let owner = {
        let mut handles = registry_unchecked()?;
        let entry = handles.get_mut(&id).ok_or_else(|| invalid("invalid or closed handle"))?;
        if entry.disposition != Disposition::Owned {
            return Err(invalid("handle already transferred"));
        }
        entry.disposition = Disposition::Queued;
        if let Handle::Cache(cache) = &entry.value { lock(&cache.owner)?.clone() } else { None }
    };
    // Stop new callbacks before the transfer returns. Ownership has transferred: the queued job
    // records a failure rather than handing an unreachable owner back.
    if let Some(owner) = owner
        && let Err(error) = owner.defer_close()
        && Failure::from(error).0 == FAULT
    {
        ABI_FAULT.store(true, Ordering::Release);
    }
    retirement::defer(retirement::Job::Handle(id));
    Ok(())
}


struct Gpu {
    core: &'static Device,
}
impl Gpu {
    fn device(&self) -> i32 {
        self.core.ordinal()
    }
    fn eager(&self, stream: u64) -> Result<()> {
        if stream <= 2 {
            return Err(invalid("use an explicit CUDA stream handle, not a default-stream sentinel"));
        }
        crate::gate(self.device())?.not_capturing()?;
        let _context = self.core.driver().enter()?;
        if self.core.driver().is_capturing(size(stream)?)? {
            return Err(Error::CaptureActive.into());
        }
        Ok(())
    }
}
static HANDLES: OnceLock<Mutex<BTreeMap<u64, Entry>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
enum Handle {
    Gpu(Arc<Gpu>),
    Region(Arc<Weight>),
    Use(Arc<Use>),
    Allocation(Arc<Allocation>),
    Cache(Arc<Cache>),
    Plan(Arc<Plan>),
}
#[derive(Clone, Copy, PartialEq)]
enum Disposition {
    Owned,
    Queued,
    Quarantined,
}
struct Entry {
    value: Handle,
    disposition: Disposition,
    failure: i32,
}
fn registry_unchecked() -> Result<std::sync::MutexGuard<'static, BTreeMap<u64, Entry>>> {
    crate::check_process()?;
    HANDLES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Failure(FAULT, "ABI registry poisoned".into(), 0))
}
fn insert(value: Handle) -> Result<u64> {
    retirement::prepare()?;
    let id = NEXT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .map_err(|_| invalid("handle space exhausted"))?;
    registry_unchecked()?.insert(
        id,
        Entry {
            value,
            disposition: Disposition::Owned,
            failure: 0,
        },
    );
    Ok(id)
}
// Field order keeps retirement exclusion alive through every possibly-final Arc drop,
// including an operation racing a checked close from another caller.
struct HandleLease {
    value: Handle,
}
struct Lease<T> {
    value: Arc<T>,
}
impl<T> std::ops::Deref for Lease<T> {
    type Target = Arc<T>;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
#[derive(Clone, Copy)]
enum Access {
    Owned,
    Retirement,
    CallbackBorrow,
}
fn get_access(id: u64, access: Access) -> Result<HandleLease> {
    process()?;
    let handles = registry_unchecked()?;
    let entry = handles
        .get(&id)
        .ok_or_else(|| invalid("invalid or closed handle"))?;
    let allowed = match access {
        Access::Owned => entry.disposition == Disposition::Owned,
        Access::Retirement => entry.disposition == Disposition::Queued,
        Access::CallbackBorrow => {
            entry.disposition == Disposition::Owned
                || (entry.disposition == Disposition::Queued
                    && match &entry.value {
                        Handle::Cache(cache) => lock(&cache.owner)?
                            .as_ref()
                            .is_some_and(|owner| cache_owner::in_callback(owner)),
                        _ => false,
                    })
        }
    };
    if !allowed {
        return Err(invalid("handle ownership has transferred"));
    }
    Ok(HandleLease {
        value: entry.value.clone(),
    })
}
fn get(id: u64, deferred: bool) -> Result<HandleLease> {
    get_access(
        id,
        if deferred {
            Access::Retirement
        } else {
            Access::Owned
        },
    )
}
fn remove(id: u64) -> Result<()> {
    let removed = registry_unchecked()?
        .remove(&id)
        .ok_or_else(|| invalid("invalid or closed handle"))?;
    drop(removed); // Never destroy GPU/source/callback owners while holding the registry.
    Ok(())
}
macro_rules! getter {
    ($fn:ident, $variant:ident, $ty:ty) => {
        fn $fn(id: u64) -> Result<Lease<$ty>> {
            let held = get(id, false)?;
            match &held.value {
                Handle::$variant(v) => Ok(Lease {
                    value: v.clone(),
                }),
                _ => Err(invalid("wrong handle type")),
            }
        }
    };
}
getter!(gpu, Gpu, Gpu);
getter!(weight, Region, Weight);
getter!(usage, Use, Use);
getter!(allocation, Allocation, Allocation);
getter!(cache, Cache, Cache);
getter!(plan, Plan, Plan);
fn cache_borrow(id: u64) -> Result<Lease<Cache>> {
    let held = get_access(id, Access::CallbackBorrow)?;
    match &held.value {
        Handle::Cache(cache) => Ok(Lease {
            value: cache.clone(),
        }),
        _ => Err(invalid("wrong handle type")),
    }
}

/// Private same-cohort bridge: attach Python's weak callback owner to the ABI's
/// actual Cache. Public C allocation/layout compatibility does not use this symbol.
#[doc(hidden)]
pub fn attach_cache_owner(id: u64, owner: Arc<crate::CacheOwner>) -> crate::Result<()> {
    let native_error = |failure: Failure| match failure.0 {
        CAPTURING => Error::CaptureActive,
        BUSY => Error::Busy,
        FAULT => Error::Poisoned(failure.1),
        _ => Error::Invalid("invalid public Cache handle"),
    };
    let cache = cache(id).map_err(native_error)?;
    if owner.device() != cache._gpu.device() {
        return Err(Error::Invalid("callback owner belongs to another device"));
    }
    let mut slot = cache
        .owner
        .lock()
        .map_err(|_| Error::Poisoned("public Cache owner mutex".into()))?;
    if slot.is_some() {
        return Err(Error::Invalid("public Cache already has a callback owner"));
    }
    *slot = Some(owner);
    Ok(())
}
fn lock<T>(v: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    v.lock()
        .map_err(|_| Failure(FAULT, "ABI owner mutex poisoned".into(), 0))
}
unsafe fn output<T: Default>(out: *mut T) -> Result<()> {
    if out.is_null() {
        return Err(invalid("null output"));
    }
    unsafe {
        out.write(T::default());
    }
    Ok(())
}
unsafe fn text(value: *const c_char) -> Result<String> {
    if value.is_null() {
        return Err(invalid("null string"));
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| invalid("invalid UTF-8"))
}
unsafe fn slice<'a, T>(ptr: *const T, n: usize) -> Result<&'a [T]> {
    if n == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() || n > isize::MAX as usize / std::mem::size_of::<T>() {
        return Err(invalid("invalid input array"));
    }
    Ok(unsafe { std::slice::from_raw_parts(ptr, n) })
}
fn size(v: u64) -> Result<usize> {
    usize::try_from(v).map_err(|_| invalid("size overflow"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_gpu_open(device: i32, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        if device < 0 {
            return Err(invalid("device must be nonnegative"));
        }
        process()?;
        retirement::prepare()?;
        // Peer startup must happen before retirement exclusion; existing verified
        // services are inherited without replacing their lifecycle configuration.
        crate::activate(device, true)?;
        let _retirement = crate::enter_retirement(device)?;
        working::initialize(device)?;
        let owner = Arc::new(Gpu {
            core: Device::open(device)?,
        });
        unsafe {
            out.write(insert(Handle::Gpu(owner))?);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_gpu_close(id: u64) -> i32 {
    boundary(|| {
        let held = get(id, false)?;
        if !matches!(held.value, Handle::Gpu(_)) {
            return Err(invalid("wrong handle type"));
        }
        drop(held);
        close_handle(id, false)
    })
}

pub type DropFn = unsafe extern "C" fn(*mut c_void);
pub(crate) struct Weight {
    gpu: Arc<Gpu>,
    region: Arc<Region>,
}
struct Use {
    _weights: Vec<Arc<Weight>>,
    pins: Mutex<Option<Vec<Pin>>>,
    stream: u64,
}
impl Drop for Use {
    fn drop(&mut self) {
        if let Ok(pins) = self.pins.get_mut()
            && let Some(pins) = pins.take()
        {
            for pin in pins {
                unsafe {
                    pin.into_fenced_region(&[self.stream]);
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_region_from_host(
    g: u64,
    bytes: *const u8,
    len: usize,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let gpu = gpu(g)?;
        let bytes: Arc<[u8]> = unsafe { slice(bytes, len)? }.into();
        let region = host_source::region(&gpu, host_source::Payload::Copied(bytes))?;
        unsafe {
            out.write(insert(Handle::Region(Arc::new(Weight { gpu: gpu.clone(), region })))?);
        }
        Ok(())
    })
}
/// Snapshot ordered host parts into one immutable Region. Outputs change only on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_region_from_host_parts(
    g: u64,
    parts: *const host_parts::HostPart,
    count: usize,
    offsets: *mut u64,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        if out.is_null() || offsets.is_null() {
            return Err(invalid("null host parts output"));
        }
        let parts = unsafe { slice(parts, count)? };
        let gpu = gpu(g)?;
        let (bytes, layout) = unsafe { host_parts::snapshot(parts)? };
        let region = host_source::region(&gpu, host_source::Payload::Copied(bytes))?;
        let handle = insert(Handle::Region(Arc::new(Weight { gpu: gpu.clone(), region })))?;
        // SAFETY: caller provides disjoint writable outputs; every fallible step is
        // complete. The snapshots no longer depend on any caller input allocation.
        unsafe {
            std::ptr::copy_nonoverlapping(layout.as_ptr(), offsets, count);
            out.write(handle);
        }
        Ok(())
    })
}
/// One Region filled from the caller's host ranges without copying them; `drop(context)` runs
/// once when the last fill and handle let go, from any thread, also if this call fails.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_region_borrow_host_parts(
    g: u64,
    parts: *const host_parts::HostPart,
    count: usize,
    offsets: *mut u64,
    drop: DropFn,
    context: *mut c_void,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        let parts = unsafe { slice(parts, count) };
        let (borrowed, layout) = unsafe { host_parts::Borrowed::new(parts?, drop, context)? };
        if out.is_null() || offsets.is_null() {
            return Err(invalid("null host parts output"));
        }
        let gpu = gpu(g)?;
        let region = host_source::region(&gpu, host_source::Payload::Borrowed(borrowed))?;
        let handle = insert(Handle::Region(Arc::new(Weight { gpu: gpu.clone(), region })))?;
        // SAFETY: caller provides disjoint writable outputs; every fallible step is complete.
        unsafe {
            std::ptr::copy_nonoverlapping(layout.as_ptr(), offsets, count);
            out.write(handle);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_region_from_source(g: u64, len: u64,
    provider: *const SourceProvider, provider_size: usize, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe { output(out)?; }
        if provider.is_null() || provider_size != std::mem::size_of::<SourceProvider>() {
            return Err(invalid("Source provider size/pointer mismatch"));
        }
        let provider = unsafe { provider.read() };
        source::validate(&provider)?;
        let gpu = gpu(g)?;
        let source = source::Owner::new(gpu.clone(), provider);
        let region = gpu.core.region(size(len)?, source.clone())?;
        let id = insert(Handle::Region(Arc::new(Weight { gpu: gpu.clone(), region })))?;
        source.owned.store(true, Ordering::Release);
        unsafe { out.write(id); }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_region_close(id: u64) -> i32 {
    boundary(|| {
        let held = get(id, false)?;
        if !matches!(held.value, Handle::Region(_)) {
            return Err(invalid("wrong handle type"));
        }
        drop(held);
        close_handle(id, false)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_region_ptr(id: u64, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
            out.write(weight(id)?.region.ptr());
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_region_set_priority(id: u64, priority: i32) -> i32 {
    boundary(|| {
        let high = match priority {
            0 => false,
            1 => true,
            _ => return Err(invalid("priority must be VR_PRIORITY_NORMAL or VR_PRIORITY_HIGH")),
        };
        weight(id)?.region.set_high(high)?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_use(
    g: u64,
    ids: *const u64,
    n: usize,
    stream: u64,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let gpu = gpu(g)?;
        let weights = unsafe { slice(ids, n)? }
            .iter()
            .map(|id| weight(*id))
            .collect::<Result<Vec<_>>>()?;
        if weights.is_empty() || weights.iter().any(|w| gpu.device() != w.gpu.device()) {
            return Err(invalid("use requires regions on this GPU"));
        }
        gpu.eager(stream)?;
        unsafe {
            out.write(acquire_use(&weights, stream)?);
        }
        Ok(())
    })
}

/// Explicit pre-body service; unlike ordinary Use, may await a finite cache-owner pass.
fn acquire_use(weights: &[Lease<Weight>], stream: u64) -> Result<u64> {
    let mut pins = Vec::with_capacity(weights.len());
    let acquired = (|| -> Result<()> {
        for weight in weights {
            pins.push(unsafe { weight.region.use_on(stream)? });
        }
        Ok(())
    })();
    if let Err(error) = acquired {
        for pin in pins {
            // No consumer pointer/Use was exposed. Producer work retains its own Pin;
            // these unpublished consumer Pins issued only ordering, never reads.
            unsafe { drop(pin.into_completed_region()); }
        }
        return Err(error);
    }
    insert(Handle::Use(Arc::new(Use {
        _weights: weights.iter().map(|weight| Arc::clone(&weight.value)).collect(),
        pins: Mutex::new(Some(pins)), stream,
    })))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_use_ptr(id: u64, index: usize, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let used = usage(id)?;
        let pins = lock(&used.pins)?;
        let pin = pins
            .as_ref()
            .and_then(|p| p.get(index))
            .ok_or_else(|| invalid("Use ended or index out of range"))?;
        unsafe {
            out.write(pin.ptr());
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_use_wait_ready(id: u64) -> i32 {
    boundary(|| {
        // The lease excludes capture/replay without retaining the capture mutex.
        // Keeping pins locked prevents concurrent close from ending this Use.
        let used = usage(id)?;
        used._weights[0].gpu.eager(used.stream)?;
        let pins = lock(&used.pins)?;
        let pins = pins.as_ref().ok_or_else(|| invalid("Use has ended"))?;
        crate::async_fill::wait_use_ready(pins)?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_done(id: u64) -> i32 {
    boundary(|| {
        let held = get(id, false)?;
        if !matches!(held.value, Handle::Use(_)) {
            return Err(invalid("wrong handle type"));
        }
        drop(held);
        close_handle(id, false)
    })
}

struct Allocation {
    gpu: Arc<Gpu>,
    pointer: u64,
    stream: u64,
    live: Mutex<bool>,
}
impl Drop for Allocation {
    fn drop(&mut self) {
        if let Ok(live) = self.live.get_mut()
            && *live
        {
            let _ = working::free(self.gpu.device(), self.pointer, self.stream);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_alloc(g: u64, bytes: u64, stream: u64, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let gpu = gpu(g)?;
        if bytes == 0 {
            return Err(invalid("allocation must be nonempty"));
        }
        gpu.eager(stream)?;
        let pointer = working::alloc(gpu.device(), size(bytes)?, stream, None)?;
        unsafe {
            out.write(insert(Handle::Allocation(Arc::new(Allocation {
                gpu: gpu.clone(),
                pointer,
                stream,
                live: Mutex::new(true),
            })))?);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_allocation_ptr(id: u64, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let a = allocation(id)?;
        if !*lock(&a.live)? {
            return Err(invalid("allocation freed"));
        }
        unsafe {
            out.write(a.pointer);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_record_stream(id: u64, stream: u64) -> i32 {
    boundary(|| {
        let a = allocation(id)?;
        let live = lock(&a.live)?;
        if !*live {
            return Err(invalid("allocation freed"));
        }
        a.gpu.eager(stream)?;
        working::record_stream(a.gpu.device(), a.pointer, stream)?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_free(id: u64) -> i32 {
    boundary(|| {
        let held = get(id, false)?;
        if !matches!(held.value, Handle::Allocation(_)) {
            return Err(invalid("wrong handle type"));
        }
        drop(held);
        close_handle(id, false)
    })
}

pub type ShrinkFn = unsafe extern "C" fn(*mut c_void, u64, u64) -> i32;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Shrinker {
    pub shrink: Option<ShrinkFn>,
    pub drop: Option<DropFn>,
    pub context: *mut c_void,
}
// SAFETY: the native per-device callback worker invokes this FnMut owner exclusively.
// Registration requires the caller to transfer callback ownership across threads and
// retain its context/code until checked retirement has quiesced that worker.
unsafe impl Send for Shrinker {}
struct ForeignShrink {
    value: Shrinker,
    id: u64,
    owned: Arc<AtomicBool>,
}
impl Drop for ForeignShrink {
    fn drop(&mut self) {
        if self.owned.load(Ordering::Acquire)
            && let Some(drop) = self.value.drop
        {
            unsafe {
                drop(self.value.context);
            }
        }
    }
}
impl cache_owner::CacheCallback for ForeignShrink {
    fn shrink(
        &mut self,
        scope: &cache_owner::CallbackScope,
        bytes: usize,
    ) -> crate::Result<cache_owner::CallbackResult> {
        let _guard = scope.enter()?;
        if !self.owned.load(Ordering::Acquire) {
            return Ok(cache_owner::CallbackResult::Declined);
        }
        match unsafe { self.value.shrink.unwrap()(self.value.context, self.id, bytes as u64) } {
            OK => Ok(cache_owner::CallbackResult::Returned),
            BUSY => Ok(cache_owner::CallbackResult::Declined),
            _ => Err(Error::Callback(
                "foreign cache shrink callback failed".into(),
            )),
        }
    }
    fn retire(&mut self, scope: &cache_owner::CallbackScope) -> crate::Result<()> {
        let _guard = scope.enter()?;
        if self.owned.swap(false, Ordering::AcqRel)
            && let Some(drop) = self.value.drop.take()
        {
            unsafe {
                drop(self.value.context);
            }
        }
        Ok(())
    }
}
struct Cache {
    _gpu: Arc<Gpu>,
    storage: Arc<working::Cache>,
    owner: Mutex<Option<Arc<crate::CacheOwner>>>,
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_cache_new(
    g: u64,
    reserve: u64,
    shrinker: Shrinker,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        if shrinker.shrink.is_none() && shrinker.drop.is_some() {
            return Err(invalid("drop requires shrink callback"));
        }
        let gpu = gpu(g)?;
        let device = gpu.device();
        let value = Arc::new(Cache {
            storage: working::Cache::new(device, size(reserve)?)?,
            _gpu: gpu.clone(),
            owner: Mutex::new(None),
        });
        let id = insert(Handle::Cache(value.clone()))?;
        if shrinker.shrink.is_some() {
            let owned = Arc::new(AtomicBool::new(false));
            match crate::CacheOwner::with_callback(
                device,
                Box::new(ForeignShrink {
                    value: shrinker,
                    id,
                    owned: owned.clone(),
                }),
            ) {
                Ok(owner) => {
                    *lock(&value.owner)? = Some(Arc::new(owner));
                    owned.store(true, Ordering::Release);
                }
                Err(error) => {
                    remove(id)?;
                    return Err(error.into());
                }
            }
        }
        unsafe {
            out.write(id);
        }
        Ok(())
    })
}
#[repr(C)]
#[derive(Default)]
pub struct CacheInfo {
    pub base: u64,
    pub granularity: u64,
    pub reserved: u64,
    pub mapped: u64,
    pub live: u64,
    pub pending: u64,
    pub ready: u64,
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_cache_info(id: u64, out: *mut CacheInfo, out_size: usize) -> i32 {
    boundary(|| {
        if out_size != std::mem::size_of::<CacheInfo>() {
            return Err(invalid("CacheInfo size mismatch"));
        }
        unsafe {
            output(out)?;
        }
        let c = cache_borrow(id)?;
        let s = c.storage.poll()?;
        unsafe {
            out.write(CacheInfo {
                base: c.storage.base()?,
                granularity: c.storage.granularity() as u64,
                reserved: s.reserved_bytes as u64,
                mapped: s.mapped_bytes as u64,
                live: s.live_bytes as u64,
                pending: s.pending_bytes as u64,
                ready: s.ready_bytes as u64,
            });
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_cache_commit(id: u64, offset: u64, bytes: u64) -> i32 {
    boundary(|| {
        cache(id)?.storage.commit(size(offset)?, size(bytes)?)?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_cache_release(id: u64, offset: u64, bytes: u64, stream: u64) -> i32 {
    boundary(|| {
        let c = cache_borrow(id)?;
        c._gpu.eager(stream)?;
        c.storage.release(size(offset)?, size(bytes)?, stream)?;
        working::seal_stream(c._gpu.device(), stream)?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_cache_close(id: u64) -> i32 {
    boundary(|| {
        let held = get(id, false)?;
        if !matches!(held.value, Handle::Cache(_)) {
            return Err(invalid("wrong handle type"));
        }
        drop(held);
        close_handle(id, false)
    })
}

#[repr(C)]
pub struct Phase {
    pub name: *const c_char,
    pub regions: *const u64,
    pub count: usize,
}
struct Plan {
    gpu: Arc<Gpu>,
    native: Arc<crate::plan::Plan>,
    _weights: Vec<Arc<Weight>>,
    phase: Mutex<Option<u64>>,
}
impl Drop for Plan {
    fn drop(&mut self) {
        if let Ok(phase) = self.phase.get_mut()
            && let Some(token) = phase.take()
        {
            let _ = working::phase_leave(self.gpu.device(), token, false, None);
        }
        let _ = self.native.deactivate();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_plan_new(
    g: u64,
    phases: *const Phase,
    n: usize,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let gpu = gpu(g)?;
        let mut definitions = Vec::new();
        let mut weights = Vec::new();
        for phase in unsafe { slice(phases, n)? } {
            let name = unsafe { text(phase.name)? };
            let uses = unsafe { slice(phase.regions, phase.count)? }
                .iter()
                .map(|id| weight(*id))
                .collect::<Result<Vec<_>>>()?;
            if uses.iter().any(|w| gpu.device() != w.gpu.device()) {
                return Err(invalid("plan regions belong to another GPU"));
            }
            definitions.push((name, uses.iter().map(|w| w.region.clone()).collect()));
            weights.extend(uses.iter().map(|w| Arc::clone(w)));
        }
        let native = crate::plan::Plan::new(gpu.device(), definitions)?;
        unsafe {
            out.write(insert(Handle::Plan(Arc::new(Plan {
                gpu: gpu.clone(),
                native,
                _weights: weights.iter().map(|w| Arc::clone(w)).collect(),
                phase: Mutex::new(None),
            })))?);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_plan_activate(id: u64, name: *const c_char) -> i32 {
    boundary(|| {
        let p = plan(id)?;
        let phase = lock(&p.phase)?;
        if phase.is_some() {
            return Err(invalid(
                "intent cannot replace an active execution observation",
            ));
        }
        p.native.activate(&unsafe { text(name)? })?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_plan_deactivate(id: u64) -> i32 {
    boundary(|| {
        let p = plan(id)?;
        let phase = lock(&p.phase)?;
        if phase.is_some() {
            return Err(invalid(
                "intent cannot replace an active execution observation",
            ));
        }
        p.native.deactivate()?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_phase_enter(
    id: u64,
    name: *const c_char,
    key: *const c_char,
) -> i32 {
    boundary(|| {
        let p = plan(id)?;
        let mut phase = lock(&p.phase)?;
        if phase.is_some() {
            return Err(invalid("phase already active"));
        }
        let name = unsafe { text(name)? };
        let key = unsafe { text(key)? };
        p.native.activate(&name)?;
        match working::phase_enter(p.gpu.device(), Some(key), working::PhaseMode::Eager) {
            Ok(token) => *phase = Some(token),
            Err(error) => {
                p.native.deactivate()?;
                return Err(error.into());
            }
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_phase_leave(id: u64, completed: i32) -> i32 {
    boundary(|| {
        if completed != 0 && completed != 1 {
            return Err(invalid("completed must be 0 or 1"));
        }
        let p = plan(id)?;
        let mut phase = lock(&p.phase)?;
        let token = phase.ok_or_else(|| invalid("no active phase"))?;
        working::phase_leave(p.gpu.device(), token, completed == 1, None)?;
        *phase = None;
        p.native.deactivate()?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_plan_close(id: u64) -> i32 {
    boundary(|| {
        let held = get(id, false)?;
        if !matches!(held.value, Handle::Plan(_)) {
            return Err(invalid("wrong handle type"));
        }
        drop(held);
        close_handle(id, false)
    })
}

#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub weights_resident: u64,
    pub working_live: u64,
    pub working_capacity: u64,
    pub working_mapped: u64,
    pub working_pending: u64,
    pub working_free: u64,
    pub working_peak: u64,
    pub pools: u64,
    pub caches: u64,
    pub cache_live: u64,
    pub cache_pending: u64,
    pub cache_ready: u64,
    pub allocations: u64,
    pub frees: u64,
    pub driver_calls: u64,
    pub driver_ooms: u64,
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_stats(g: u64, out: *mut Stats, out_size: usize) -> i32 {
    boundary(|| {
        if out_size != std::mem::size_of::<Stats>() {
            return Err(invalid("Stats size mismatch"));
        }
        unsafe {
            output(out)?;
        }
        let device = gpu(g)?.device();
        let s = working::stats(device)?;
        unsafe {
            out.write(Stats {
                weights_resident: crate::resident_bytes(device)? as u64,
                working_live: s.allocated_bytes as u64,
                working_capacity: s.allocated_capacity as u64,
                working_mapped: s.mapped_bytes as u64,
                working_pending: s.pending_bytes as u64,
                working_free: s.free_bytes as u64,
                working_peak: s.allocated_peak as u64,
                pools: s.pools_bytes as u64,
                caches: s.cache_mapped_bytes as u64,
                cache_live: s.cache_live_bytes as u64,
                cache_pending: s.cache_pending_bytes as u64,
                cache_ready: s.cache_ready_bytes as u64,
                allocations: s.allocations,
                frees: s.frees,
                driver_calls: s.driver_memory_calls,
                driver_ooms: s.driver_physical_ooms,
            });
        }
        Ok(())
    })
}

fn close_handle(id: u64, deferred: bool) -> Result<()> {
    let held = get(id, deferred)?;
    match &held.value {
        Handle::Gpu(_) | Handle::Region(_) => {}
        Handle::Use(used) => {
            used._weights[0].gpu.eager(used.stream)?;
            let pins = lock(&used.pins)?
                .take()
                .ok_or_else(|| invalid("use already ended"))?;
            for pin in pins {
                unsafe {
                    pin.into_fenced_region(&[used.stream]);
                }
            }
            working::seal_stream(used._weights[0].gpu.device(), used.stream)?;
        }
        Handle::Allocation(a) => {
            let mut live = lock(&a.live)?;
            if *live {
                a.gpu.eager(a.stream)?;
                working::free(a.gpu.device(), a.pointer, a.stream)?;
                *live = false;
                working::seal_stream(a.gpu.device(), a.stream)?;
            }
        }
        Handle::Cache(c) => {
            let owner = lock(&c.owner)?.clone();
            if let Some(owner) = owner {
                if deferred {
                    owner.defer_close()?;
                    let facts = owner.facts()?;
                    if let Some(reason) = facts.retirement_failed {
                        return Err(Error::Poisoned(reason).into());
                    }
                    if !facts.retired {
                        return Err(Error::Busy.into());
                    }
                } else {
                    owner.close()?;
                }
            }
            // Live Cache pages have no completion proof. Invalid quarantines them;
            // only submitted pending events may be retried by the deferred worker.
            c.storage.close()?;
        }
        Handle::Plan(p) => {
            let mut phase = lock(&p.phase)?;
            if let Some(token) = *phase {
                working::phase_leave(p.gpu.device(), token, false, None)?;
                *phase = None;
            }
            p.native.deactivate()?;
        }
    }
    remove(id)
}

/// Transfer a handle to native retirement. Success invalidates the caller's handle.
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_defer_close(id: u64) -> i32 {
    boundary(|| defer_handle(id))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_retirement_stats(
    out: *mut retirement::Stats,
    out_size: usize,
) -> i32 {
    boundary(|| {
        if out_size != std::mem::size_of::<retirement::Stats>() {
            return Err(invalid("retirement stats size mismatch"));
        }
        unsafe {
            output(out)?;
            out.write(retirement::stats()?);
        }
        Ok(())
    })
}
