//! Public C ABI v1. This module adapts ownership, not allocation policy.
#![allow(unsafe_code)]

use crate::{Arena, Error, Pin, Preparation, Region, cache_owner, working};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

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
            | Error::Exhausted {
                requested_bytes, ..
            } => (EXHAUSTED, *requested_bytes as u64),
            Error::CaptureActive => (CAPTURING, 0),
            Error::DeviceFault { .. } | Error::Poisoned(_) => (FAULT, 0),
            Error::Unsupported(_) => (UNSUPPORTED, 0),
            Error::Busy => (BUSY, 0),
            Error::Callback(_) => (SOURCE_ERROR, 0),
            Error::Cuda { .. } | Error::Invalid(_) => (INVALID, 0),
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
    &crate::ARENAS as *const _ as usize as u64
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

struct Gpu {
    arena: Arc<Arena>,
}
impl Gpu {
    fn device(&self) -> i32 {
        self.arena.device()
    }
    fn eager(&self, stream: u64) -> Result<()> {
        if stream == 1 || stream == 2 {
            return Err(invalid(
                "use an explicit CUstream or legacy default stream 0",
            ));
        }
        let captures = crate::capture_counts()?;
        crate::ensure_recovery_allowed(&captures, self.device())?;
        let _context = self.arena.driver.enter()?;
        if self.arena.driver.is_capturing(size(stream)?)? {
            return Err(Error::CaptureActive.into());
        }
        Ok(())
    }
}
static GPUS: OnceLock<Mutex<BTreeMap<i32, Weak<Gpu>>>> = OnceLock::new();
static HANDLES: OnceLock<Mutex<BTreeMap<u64, Handle>>> = OnceLock::new();
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
fn registry() -> Result<std::sync::MutexGuard<'static, BTreeMap<u64, Handle>>> {
    process()?;
    HANDLES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Failure(FAULT, "ABI registry poisoned".into(), 0))
}
fn insert(value: Handle) -> Result<u64> {
    let id = NEXT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .map_err(|_| invalid("handle space exhausted"))?;
    registry()?.insert(id, value);
    Ok(id)
}
fn get(id: u64) -> Result<Handle> {
    registry()?
        .get(&id)
        .cloned()
        .ok_or_else(|| invalid("invalid or closed handle"))
}
fn remove(id: u64) -> Result<()> {
    let removed = registry()?
        .remove(&id)
        .ok_or_else(|| invalid("invalid or closed handle"))?;
    drop(removed); // Never destroy CUDA/source/callback owners while holding the registry.
    Ok(())
}
macro_rules! getter {
    ($fn:ident, $variant:ident, $ty:ty) => {
        fn $fn(id: u64) -> Result<Arc<$ty>> {
            match get(id)? {
                Handle::$variant(v) => Ok(v),
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
        let mut devices = lock(GPUS.get_or_init(Mutex::default))?;
        let owner = match devices.get(&device).and_then(Weak::upgrade) {
            Some(owner) => owner,
            None => {
                working::initialize(device)?;
                let owner = Arc::new(Gpu {
                    arena: Arena::new(device)?,
                });
                devices.insert(device, Arc::downgrade(&owner));
                owner
            }
        };
        drop(devices);
        unsafe {
            out.write(insert(Handle::Gpu(owner))?);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_gpu_close(id: u64) -> i32 {
    boundary(|| {
        gpu(id)?;
        remove(id)
    })
}

pub type FillFn = unsafe extern "C" fn(*mut c_void, u64, u64, u64, u64) -> i32;
pub type DropFn = unsafe extern "C" fn(*mut c_void);
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Source {
    pub fill: Option<FillFn>,
    pub drop: Option<DropFn>,
    pub context: *mut c_void,
}
// C callers guarantee thread-safe callbacks/context. No foreign reference is dereferenced in Rust.
unsafe impl Send for Source {}
unsafe impl Sync for Source {}
struct ForeignSource {
    source: Source,
    owned: AtomicBool,
}
impl Drop for ForeignSource {
    fn drop(&mut self) {
        if self.owned.load(Ordering::Acquire)
            && let Some(drop) = self.source.drop
        {
            unsafe {
                drop(self.source.context);
            }
        }
    }
}
enum Bytes {
    Host(Vec<u8>),
    Foreign(Arc<ForeignSource>),
}
struct Weight {
    gpu: Arc<Gpu>,
    region: Arc<Region>,
    bytes: Bytes,
    fill: Mutex<()>,
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
        let bytes = unsafe { slice(bytes, len)? }.to_vec();
        let region = gpu.arena.reserve(len)?;
        unsafe {
            out.write(insert(Handle::Region(Arc::new(Weight {
                gpu,
                region,
                bytes: Bytes::Host(bytes),
                fill: Mutex::new(()),
            })))?);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_region_new(g: u64, len: u64, source: Source, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        if source.fill.is_none() {
            return Err(invalid("source.fill is required"));
        }
        let gpu = gpu(g)?;
        let region = gpu.arena.reserve(size(len)?)?;
        let source = Arc::new(ForeignSource {
            source,
            owned: AtomicBool::new(false),
        });
        let id = insert(Handle::Region(Arc::new(Weight {
            gpu,
            region,
            bytes: Bytes::Foreign(source.clone()),
            fill: Mutex::new(()),
        })))?;
        source.owned.store(true, Ordering::Release);
        unsafe {
            out.write(id);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_region_close(id: u64) -> i32 {
    boundary(|| {
        weight(id)?;
        remove(id)
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
        gpu.eager(stream)?;
        if weights.is_empty() || weights.iter().any(|w| !Arc::ptr_eq(&gpu, &w.gpu)) {
            return Err(invalid("use requires regions from this GPU"));
        }
        let mut pins = Vec::with_capacity(n);
        for w in &weights {
            let _fill = lock(&w.fill)?;
            let pin = match w.region.prepare()? {
                Preparation::Resident(pin) => pin,
                Preparation::Fill(mut fill) => match &w.bytes {
                    Bytes::Host(bytes) => {
                        fill.copy_from_host(bytes)?;
                        fill.commit()?
                    }
                    Bytes::Foreign(source) => {
                        // The current core publishes only completed fills. The adapter conservatively
                        // joins the primary context; asynchronous source publication is not claimed.
                        let _context = w.gpu.arena.driver.enter()?;
                        let status = unsafe {
                            source.source.fill.unwrap()(
                                source.source.context,
                                0,
                                fill.len() as u64,
                                fill.ptr(),
                                stream,
                            )
                        };
                        if status != OK {
                            drop(fill); // Cold pages retain the core's conservative completion fence.
                            let code = if (EXHAUSTED..=BUSY).contains(&status) {
                                status
                            } else {
                                SOURCE_ERROR
                            };
                            if code == FAULT {
                                w.region.poison("foreign source reported a device fault");
                            }
                            return Err(Failure(
                                code,
                                format!("source fill returned status {status}"),
                                if code == EXHAUSTED {
                                    w.region.len() as u64
                                } else {
                                    0
                                },
                            ));
                        }
                        unsafe { fill.commit_external()? }
                    }
                },
            };
            pins.push(pin);
        }
        unsafe {
            out.write(insert(Handle::Use(Arc::new(Use {
                _weights: weights,
                pins: Mutex::new(Some(pins)),
                stream,
            })))?);
        }
        Ok(())
    })
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
pub extern "C" fn vr_v1_done(id: u64) -> i32 {
    boundary(|| {
        let used = usage(id)?;
        used._weights[0].gpu.eager(used.stream)?;
        let pins = lock(&used.pins)?
            .take()
            .ok_or_else(|| invalid("use already ended"))?;
        for pin in pins {
            unsafe {
                pin.into_fenced_region(&[used.stream]);
            }
        }
        remove(id)
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
                gpu,
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
        let a = allocation(id)?;
        let mut live = lock(&a.live)?;
        if !*live {
            return Err(invalid("allocation freed"));
        }
        a.gpu.eager(a.stream)?;
        working::free(a.gpu.device(), a.pointer, a.stream)?;
        *live = false;
        drop(live);
        remove(id)
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
            _gpu: gpu,
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
        let c = cache(id)?;
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
        let c = cache(id)?;
        c._gpu.eager(stream)?;
        c.storage.release(size(offset)?, size(bytes)?, stream)?;
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn vr_v1_cache_close(id: u64) -> i32 {
    boundary(|| {
        let c = cache(id)?;
        let owner = lock(&c.owner)?.clone();
        // No binding lock is held while a foreign callback may run or acquire its GIL.
        if let Some(owner) = owner {
            owner.close()?;
        }
        c.storage.close()?;
        remove(id)
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
            if uses.iter().any(|w| !Arc::ptr_eq(&gpu, &w.gpu)) {
                return Err(invalid("plan regions belong to another GPU"));
            }
            definitions.push((name, uses.iter().map(|w| w.region.clone()).collect()));
            weights.extend(uses);
        }
        let native = crate::plan::Plan::new(gpu.arena.clone(), definitions)?;
        unsafe {
            out.write(insert(Handle::Plan(Arc::new(Plan {
                gpu,
                native,
                _weights: weights,
                phase: Mutex::new(None),
            })))?);
        }
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
        plan(id)?;
        remove(id)
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_trim(g: u64, out: *mut u64) -> i32 {
    boundary(|| {
        unsafe {
            output(out)?;
        }
        let device = gpu(g)?.device();
        let working = working::trim(device)? as u64;
        let weights = crate::reclaim(device, usize::MAX)?.reclaimed_bytes as u64;
        unsafe {
            out.write(working.saturating_add(weights));
        }
        Ok(())
    })
}
