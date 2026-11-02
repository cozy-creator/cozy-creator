//! Safe ownership over `varena.h` ABI 1. No allocator implementation is linked here.
//!
//! Link to the same libvarena used by TensorFS. Borrowed CUDA streams require an unsafe
//! import; callers submitting kernels through raw device pointers remain responsible for
//! bounds, CUDA context, immutable weights and joining every consumer before release.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Exhausted,
    Capturing,
    SourceError,
    Fault,
    Invalid,
    Unsupported,
    Busy,
    Unknown(i32),
}
#[derive(Debug)]
pub struct Error {
    pub status: Status,
    pub message: String,
    pub requested_bytes: u64,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
fn check(code: i32) -> Result<()> {
    if code == 0 {
        return Ok(());
    }
    let status = match code {
        1 => Status::Exhausted,
        2 => Status::Capturing,
        3 => Status::SourceError,
        4 => Status::Fault,
        5 => Status::Invalid,
        6 => Status::Unsupported,
        7 => Status::Busy,
        n => Status::Unknown(n),
    };
    Err(Error {
        status,
        message: unsafe { CStr::from_ptr(ffi::vr_v1_last_error()) }
            .to_string_lossy()
            .into_owned(),
        requested_bytes: unsafe { ffi::vr_v1_error_requested_bytes() },
    })
}
fn invalid(message: &str) -> Error {
    Error {
        status: Status::Invalid,
        message: message.into(),
        requested_bytes: 0,
    }
}
fn string(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| invalid("string contains NUL"))
}

fn defer(handle: &mut u64) {
    if *handle != 0 && unsafe { ffi::vr_v1_defer_close(*handle) } == 0 {
        *handle = 0;
    }
}
#[repr(C)]
#[derive(Default, Debug)]
pub struct RetirementStats {
    pub queued: u64,
    pub retired: u64,
    pub quarantined: u64,
    pub abandoned_cache_bytes: u64,
}
pub fn retirement_stats() -> Result<RetirementStats> {
    let mut stats = RetirementStats::default();
    check(unsafe {
        ffi::vr_v1_retirement_stats(&mut stats, std::mem::size_of::<RetirementStats>())
    })?;
    Ok(stats)
}

pub fn core_identity() -> u64 {
    unsafe { ffi::vr_v1_core_identity() }
}
pub fn abi_version() -> u32 {
    unsafe { ffi::vr_v1_abi_version() }
}

struct GpuInner {
    handle: u64,
    device: i32,
}
impl Drop for GpuInner {
    fn drop(&mut self) {
        defer(&mut self.handle);
    }
}
#[derive(Clone)]
pub struct Gpu(Arc<GpuInner>);
impl Gpu {
    pub fn open(device: i32) -> Result<Self> {
        if abi_version() != 1 {
            return Err(invalid("Varena ABI 1 is required"));
        }
        let mut handle = 0;
        check(unsafe { ffi::vr_v1_gpu_open(device, &mut handle) })?;
        Ok(Self(Arc::new(GpuInner { handle, device })))
    }
    pub fn device(&self) -> i32 {
        self.0.device
    }
    /// # Safety
    /// Stream must belong to this GPU's primary context and remain live until all
    /// clones and resources retaining it are dropped. It must not be capturing.
    /// This includes native deferred retirement after a Rust finalizer: use checked
    /// close/retry or wait for retirement to drain before destroying the CUDA stream.
    pub unsafe fn borrow_stream(&self, raw: u64) -> Stream {
        Stream {
            gpu: self.clone(),
            raw,
        }
    }
    fn stream(&self, stream: &Stream) -> Result<()> {
        if self.device() != stream.gpu.device() {
            return Err(invalid("stream belongs to another GPU"));
        }
        Ok(())
    }
    /// Copies immutable host bytes now; the core keeps them for subsequent refills.
    pub fn region(&self, bytes: &[u8]) -> Result<Region> {
        let mut handle = 0;
        check(unsafe {
            ffi::vr_v1_region_from_host(self.0.handle, bytes.as_ptr(), bytes.len(), &mut handle)
        })?;
        Ok(Region(Arc::new(RegionInner {
            gpu: self.clone(),
            handle,
        })))
    }
    pub fn use_regions(&self, regions: &[&Region], stream: &Stream) -> Result<Use> {
        self.stream(stream)?;
        let ids = regions.iter().map(|r| r.0.handle).collect::<Vec<_>>();
        let mut handle = 0;
        check(unsafe {
            ffi::vr_v1_use(
                self.0.handle,
                ids.as_ptr(),
                ids.len(),
                stream.raw,
                &mut handle,
            )
        })?;
        Ok(Use {
            handle,
            _stream: stream.clone(),
        })
    }
    pub fn alloc(&self, bytes: u64, stream: &Stream) -> Result<Allocation> {
        self.stream(stream)?;
        let mut handle = 0;
        check(unsafe { ffi::vr_v1_alloc(self.0.handle, bytes, stream.raw, &mut handle) })?;
        Ok(Allocation {
            handle,
            stream: stream.clone(),
        })
    }
    pub fn cache(&self, reserve: u64) -> Result<Cache> {
        self.cache_inner(reserve, ffi::Shrinker::default())
    }
    /// Callback runs on a native worker. Return false to decline; actual releases,
    /// never the return value or requested byte count, determine reclamation.
    pub fn cache_with_shrink<F>(&self, reserve: u64, callback: F) -> Result<Cache>
    where
        F: FnMut(&CacheView, u64) -> Result<bool> + Send + 'static,
    {
        let callback = Box::into_raw(Box::new(Callback {
            gpu: self.clone(),
            callback: Box::new(callback),
        }));
        let value = ffi::Shrinker {
            shrink: Some(shrink),
            drop: Some(drop_callback),
            context: callback.cast(),
        };
        match self.cache_inner(reserve, value) {
            Ok(cache) => Ok(cache),
            Err(error) => {
                unsafe {
                    drop(Box::from_raw(callback));
                }
                Err(error)
            }
        }
    }
    fn cache_inner(&self, reserve: u64, callback: ffi::Shrinker) -> Result<Cache> {
        let mut handle = 0;
        check(unsafe { ffi::vr_v1_cache_new(self.0.handle, reserve, callback, &mut handle) })?;
        Ok(Cache {
            handle,
            gpu: self.clone(),
        })
    }
    pub fn plan(&self, phases: &[Phase<'_>]) -> Result<Plan> {
        let names = phases
            .iter()
            .map(|p| string(p.name))
            .collect::<Result<Vec<_>>>()?;
        let ids = phases
            .iter()
            .map(|p| p.regions.iter().map(|r| r.0.handle).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let descriptors = names
            .iter()
            .zip(&ids)
            .map(|(name, ids)| ffi::Phase {
                name: name.as_ptr(),
                regions: ids.as_ptr(),
                count: ids.len(),
            })
            .collect::<Vec<_>>();
        let mut handle = 0;
        check(unsafe {
            ffi::vr_v1_plan_new(
                self.0.handle,
                descriptors.as_ptr(),
                descriptors.len(),
                &mut handle,
            )
        })?;
        Ok(Plan {
            handle,
            _gpu: self.clone(),
        })
    }
    pub fn stats(&self) -> Result<Stats> {
        let mut result = Stats::default();
        check(unsafe {
            ffi::vr_v1_stats(self.0.handle, &mut result, std::mem::size_of::<Stats>())
        })?;
        Ok(result)
    }
    pub fn trim(&self) -> Result<u64> {
        let mut released = 0;
        check(unsafe { ffi::vr_v1_trim(self.0.handle, &mut released) })?;
        Ok(released)
    }
}

#[derive(Clone)]
pub struct Stream {
    gpu: Gpu,
    raw: u64,
}
impl Stream {
    pub fn raw(&self) -> u64 {
        self.raw
    }
}
struct RegionInner {
    gpu: Gpu,
    handle: u64,
}
impl Drop for RegionInner {
    fn drop(&mut self) {
        defer(&mut self.handle);
    }
}
#[derive(Clone)]
pub struct Region(Arc<RegionInner>);
impl Region {
    pub fn device(&self) -> i32 {
        self.0.gpu.device()
    }
}
pub struct Use {
    handle: u64,
    _stream: Stream,
}
impl Use {
    /// This address is valid for kernel submissions only while this Use lives.
    pub fn ptr(&self, index: usize) -> Result<u64> {
        let mut pointer = 0;
        check(unsafe { ffi::vr_v1_use_ptr(self.handle, index, &mut pointer) })?;
        Ok(pointer)
    }
    pub fn done(&mut self) -> Result<()> {
        check(unsafe { ffi::vr_v1_done(self.handle) })?;
        self.handle = 0;
        Ok(())
    }
}
impl Drop for Use {
    fn drop(&mut self) {
        defer(&mut self.handle);
    }
}
pub struct Allocation {
    handle: u64,
    stream: Stream,
}
impl Allocation {
    pub fn ptr(&self) -> Result<u64> {
        let mut pointer = 0;
        check(unsafe { ffi::vr_v1_allocation_ptr(self.handle, &mut pointer) })?;
        Ok(pointer)
    }
    pub fn record_stream(&self, stream: &Stream) -> Result<()> {
        self.stream.gpu.stream(stream)?;
        check(unsafe { ffi::vr_v1_record_stream(self.handle, stream.raw) })
    }
    pub fn free(&mut self) -> Result<()> {
        check(unsafe { ffi::vr_v1_free(self.handle) })?;
        self.handle = 0;
        Ok(())
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        defer(&mut self.handle);
    }
}

pub struct Cache {
    handle: u64,
    gpu: Gpu,
}
pub struct CacheView {
    handle: u64,
    gpu: Gpu,
}
impl CacheView {
    pub fn info(&self) -> Result<CacheInfo> {
        let mut info = CacheInfo::default();
        check(unsafe {
            ffi::vr_v1_cache_info(self.handle, &mut info, std::mem::size_of::<CacheInfo>())
        })?;
        Ok(info)
    }
    pub fn release(&self, offset: u64, bytes: u64, stream: &Stream) -> Result<()> {
        self.gpu.stream(stream)?;
        check(unsafe { ffi::vr_v1_cache_release(self.handle, offset, bytes, stream.raw) })
    }
}
impl Cache {
    pub fn info(&self) -> Result<CacheInfo> {
        self.view().info()
    }
    fn view(&self) -> CacheView {
        CacheView {
            handle: self.handle,
            gpu: self.gpu.clone(),
        }
    }
    pub fn commit(&self, offset: u64, bytes: u64) -> Result<()> {
        check(unsafe { ffi::vr_v1_cache_commit(self.handle, offset, bytes) })
    }
    pub fn release(&self, offset: u64, bytes: u64, stream: &Stream) -> Result<()> {
        self.view().release(offset, bytes, stream)
    }
    /// Retryable: release every live range and poll before close. Failure retains handle.
    pub fn close(&mut self) -> Result<()> {
        if self.handle != 0 {
            check(unsafe { ffi::vr_v1_cache_close(self.handle) })?;
            self.handle = 0;
        }
        Ok(())
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        defer(&mut self.handle);
    }
}
type ShrinkCallback = dyn FnMut(&CacheView, u64) -> Result<bool> + Send;
struct Callback {
    gpu: Gpu,
    callback: Box<ShrinkCallback>,
}
unsafe extern "C" fn shrink(context: *mut c_void, handle: u64, bytes: u64) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *context.cast::<Callback>() };
        let view = CacheView {
            handle,
            gpu: context.gpu.clone(),
        };
        match (context.callback)(&view, bytes) {
            Ok(true) => 0,
            Ok(false) => 7,
            Err(_) => 3,
        }
    }))
    .unwrap_or(3)
}
unsafe extern "C" fn drop_callback(context: *mut c_void) {
    // Never unwind across C, even if user callback captures have a panicking Drop.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(context.cast::<Callback>()));
    }));
}

pub struct Phase<'a> {
    pub name: &'a str,
    pub regions: &'a [&'a Region],
}
pub struct Plan {
    handle: u64,
    _gpu: Gpu,
}
impl Plan {
    pub fn phase(&mut self, name: &str, execution_key: &str) -> Result<Pass<'_>> {
        let name = string(name)?;
        let key = string(execution_key)?;
        check(unsafe { ffi::vr_v1_phase_enter(self.handle, name.as_ptr(), key.as_ptr()) })?;
        Ok(Pass {
            plan: self,
            active: true,
        })
    }
}
impl Drop for Plan {
    fn drop(&mut self) {
        defer(&mut self.handle);
    }
}
pub struct Pass<'a> {
    plan: &'a mut Plan,
    active: bool,
}
impl Pass<'_> {
    pub fn abort(&mut self) -> Result<()> {
        if self.active {
            check(unsafe { ffi::vr_v1_phase_leave(self.plan.handle, 0) })?;
            self.active = false;
        }
        Ok(())
    }
    pub fn complete(&mut self) -> Result<()> {
        check(unsafe { ffi::vr_v1_phase_leave(self.plan.handle, 1) })?;
        self.active = false;
        Ok(())
    }
}
impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if self.active && self.abort().is_err() {
            // An abandoned pass cannot leave an unreachable phase token. Closing
            // the whole Plan asynchronously retains its regions and exact token.
            defer(&mut self.plan.handle);
        }
    }
}

#[repr(C)]
#[derive(Default, Debug)]
pub struct CacheInfo {
    pub base: u64,
    pub granularity: u64,
    pub reserved: u64,
    pub mapped: u64,
    pub live: u64,
    pub pending: u64,
    pub ready: u64,
}
#[repr(C)]
#[derive(Default, Debug)]
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
mod ffi {
    use super::*;
    #[repr(C)]
    #[derive(Default)]
    pub struct Shrinker {
        pub shrink: Option<unsafe extern "C" fn(*mut c_void, u64, u64) -> i32>,
        pub drop: Option<unsafe extern "C" fn(*mut c_void)>,
        pub context: *mut c_void,
    }
    #[repr(C)]
    pub struct Phase {
        pub name: *const c_char,
        pub regions: *const u64,
        pub count: usize,
    }
    #[link(name = "varena")]
    unsafe extern "C" {
        pub fn vr_v1_defer_close(handle: u64) -> i32;
        pub fn vr_v1_retirement_stats(out: *mut RetirementStats, size: usize) -> i32;
        pub fn vr_v1_abi_version() -> u32;
        pub fn vr_v1_core_identity() -> u64;
        pub fn vr_v1_last_error() -> *const c_char;
        pub fn vr_v1_error_requested_bytes() -> u64;
        pub fn vr_v1_gpu_open(device: i32, out: *mut u64) -> i32;
        pub fn vr_v1_region_from_host(gpu: u64, bytes: *const u8, n: usize, out: *mut u64) -> i32;
        pub fn vr_v1_use_ptr(used: u64, index: usize, out: *mut u64) -> i32;
        pub fn vr_v1_use(
            gpu: u64,
            regions: *const u64,
            n: usize,
            stream: u64,
            out: *mut u64,
        ) -> i32;
        pub fn vr_v1_done(used: u64) -> i32;
        pub fn vr_v1_alloc(gpu: u64, bytes: u64, stream: u64, out: *mut u64) -> i32;
        pub fn vr_v1_allocation_ptr(allocation: u64, out: *mut u64) -> i32;
        pub fn vr_v1_record_stream(allocation: u64, stream: u64) -> i32;
        pub fn vr_v1_free(allocation: u64) -> i32;
        pub fn vr_v1_cache_new(gpu: u64, reserve: u64, shrinker: Shrinker, out: *mut u64) -> i32;
        pub fn vr_v1_cache_info(cache: u64, out: *mut CacheInfo, size: usize) -> i32;
        pub fn vr_v1_cache_commit(cache: u64, offset: u64, bytes: u64) -> i32;
        pub fn vr_v1_cache_release(cache: u64, offset: u64, bytes: u64, stream: u64) -> i32;
        pub fn vr_v1_cache_close(cache: u64) -> i32;
        pub fn vr_v1_plan_new(gpu: u64, phases: *const Phase, n: usize, out: *mut u64) -> i32;
        pub fn vr_v1_phase_enter(plan: u64, name: *const c_char, key: *const c_char) -> i32;
        pub fn vr_v1_phase_leave(plan: u64, completed: i32) -> i32;
        pub fn vr_v1_stats(gpu: u64, out: *mut Stats, size: usize) -> i32;
        pub fn vr_v1_trim(gpu: u64, out: *mut u64) -> i32;
    }
}
