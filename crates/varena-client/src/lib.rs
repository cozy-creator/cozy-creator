//! Safe ownership over `varena.h` ABI 1. No allocator implementation is linked here.
//!
//! Link to the same libvarena used by TensorFS. Borrowed CUDA streams require an unsafe
//! import; callers submitting kernels through raw device pointers remain responsible for
//! bounds, CUDA context, immutable weights and joining every consumer before release.

#![deny(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;

#[allow(unsafe_code)]
mod source;
pub use source::{Source, SourceJob, SourceRetirement};

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
#[allow(unsafe_code)]
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

#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
pub fn retirement_stats() -> Result<RetirementStats> {
    let mut stats = RetirementStats::default();
    check(unsafe {
        ffi::vr_v1_retirement_stats(&mut stats, std::mem::size_of::<RetirementStats>())
    })?;
    Ok(stats)
}

#[allow(unsafe_code)]
pub fn core_identity() -> u64 {
    unsafe { ffi::vr_v1_core_identity() }
}
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
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
    /// Snapshot parts into one Region with zero-filled gaps and 256-byte-aligned offsets.
    /// Empty members are allowed, but the aggregate must contain bytes. Capture refuses
    /// before copying; inputs are no longer borrowed after this returns.
    pub fn region_parts(&self, parts: &[&[u8]]) -> Result<(Region, Vec<u64>)> {
        let parts = parts.iter().map(|part| ffi::HostPart {
            data: part.as_ptr(), bytes: part.len(),
        }).collect::<Vec<_>>();
        let mut offsets = vec![0; parts.len()];
        let mut handle = 0;
        check(unsafe {
            ffi::vr_v1_region_from_host_parts(self.0.handle, parts.as_ptr(), parts.len(),
                offsets.as_mut_ptr(), &mut handle)
        })?;
        Ok((Region(Arc::new(RegionInner { gpu: self.clone(), handle })), offsets))
    }
    /// Orders source production on this stream only. Another reader stream needs its
    /// own Use or a dependency behind this stream before reading; join side work before done.
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
    /// What this GPU has right now. A report: nothing is reserved.
    pub fn room(&self) -> Result<Room> {
        let mut room = Room::default();
        check(unsafe {
            ffi::vr_v1_room(self.0.handle, &mut room, std::mem::size_of::<Room>())
        })?;
        Ok(room)
    }
    /// The core picks the largest level under its budget and returns its index in `choices`.
    pub fn split_select(
        &self,
        key: &str,
        choices: &[SplitChoice<'_>],
        stream: &Stream,
    ) -> Result<SplitSelection> {
        self.stream(stream)?;
        let key = string(key)?;
        let ids = choices.iter().map(|choice| string(choice.id)).collect::<Result<Vec<_>>>()?;
        let menu: Vec<ffi::SplitChoice> = ids.iter().zip(choices)
            .map(|(id, choice)| ffi::SplitChoice { id: id.as_ptr(), bytes: choice.bytes })
            .collect();
        let mut selection = SplitSelection::default();
        check(unsafe {
            ffi::vr_v1_split_select(self.0.handle, key.as_ptr(), menu.as_ptr(), menu.len(),
                stream.raw, &mut selection, std::mem::size_of::<SplitSelection>())
        })?;
        Ok(selection)
    }
    pub fn stats(&self) -> Result<Stats> {
        let mut result = Stats::default();
        check(unsafe {
            ffi::vr_v1_stats(self.0.handle, &mut result, std::mem::size_of::<Stats>())
        })?;
        Ok(result)
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
#[allow(unsafe_code)]
impl Region {
    /// `High` goes only once owner Caches and peers were asked to give memory.
    pub fn set_priority(&self, priority: Priority) -> Result<()> {
        check(unsafe { ffi::vr_v1_region_set_priority(self.0.handle, priority as i32) })
    }
    pub fn device(&self) -> i32 {
        self.0.gpu.device()
    }
}
pub struct Use {
    handle: u64,
    _stream: Stream,
}
#[allow(unsafe_code)]
impl Use {
    /// Wait only this Use's producers and checked native resource retirement.
    /// Call outside capture/replay; this does not wait for consumers or promise
    /// capture admission against other producers. Refusal retains this Use.
    pub fn wait_ready(&self) -> Result<()> {
        check(unsafe { ffi::vr_v1_use_wait_ready(self.handle) })
    }
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
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
impl Plan {
    /// Declare copied Region intent, without publishing an execution sample.
    pub fn activate(&mut self, name: &str) -> Result<()> {
        let name = string(name)?;
        check(unsafe { ffi::vr_v1_plan_activate(self.handle, name.as_ptr()) })
    }

    pub fn deactivate(&mut self) -> Result<()> {
        check(unsafe { ffi::vr_v1_plan_deactivate(self.handle) })
    }

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
#[allow(unsafe_code)]
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
    /// Measure a selected level's allocations, all on `stream`, until `end`.
    pub fn split(&mut self, key: &str, id: &str, stream: &Stream, generation: u64) -> Result<Split<'_>> {
        let (key, id) = (string(key)?, string(id)?);
        let mut handle = 0;
        check(unsafe {
            ffi::vr_v1_split_begin(self.plan.handle, key.as_ptr(), id.as_ptr(), stream.raw, generation, &mut handle)
        })?;
        Ok(Split { plan: &*self.plan, handle })
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

pub struct Split<'a> {
    plan: &'a Plan,
    handle: u64,
}
#[allow(unsafe_code)]
impl Split<'_> {
    fn finish(&mut self, submitted: bool) -> Result<SplitEnd> {
        let mut end = SplitEnd::default();
        check(unsafe {
            ffi::vr_v1_split_end(self.plan.handle, self.handle, i32::from(submitted), &mut end,
                std::mem::size_of::<SplitEnd>())
        })?;
        self.handle = 0;
        Ok(end)
    }
    /// `submitted` false keeps no measure, as dropping it does.
    pub fn end(mut self, submitted: bool) -> Result<SplitEnd> {
        self.finish(submitted)
    }
}
impl Drop for Split<'_> {
    fn drop(&mut self) {
        if self.handle != 0 {
            let _ = self.finish(false);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Normal = 0,
    High = 1,
}
#[derive(Clone, Copy)]
pub struct SplitChoice<'a> {
    pub id: &'a str,
    /// Estimated full working bytes.
    pub bytes: u64,
}
#[repr(C)]
#[derive(Default, Debug)]
pub struct SplitSelection {
    pub index: u64,
    pub bytes: u64,
    pub measured: u64,
    pub budget: u64,
    pub generation: u64,
}
#[repr(C)]
#[derive(Default, Debug)]
pub struct SplitEnd {
    pub peak: u64,
    pub counted: u64,
}
#[repr(C)]
#[derive(Default, Debug)]
pub struct Room {
    pub capacity: u64,
    pub working_live: u64,
    pub pinned: u64,
    pub weights_resident: u64,
    pub free: u64,
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
#[allow(unsafe_code)]
mod ffi {
    use super::*;
    #[repr(C)]
    pub struct SplitChoice {
        pub id: *const c_char,
        pub bytes: u64,
    }
    #[repr(C)]
    pub struct HostPart {
        pub data: *const u8,
        pub bytes: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct SourceJob {
        pub streams: *const u64,
        pub stream_count: usize,
        pub submit: Option<unsafe extern "C" fn(*mut c_void, u64, u64) -> i32>,
        pub retire: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        pub drop: Option<unsafe extern "C" fn(*mut c_void)>,
        pub context: *mut c_void,
    }
    #[repr(C)]
    pub struct SourceProvider {
        pub prepare: Option<unsafe extern "C" fn(*mut c_void, i32, u64, u64, *mut SourceJob) -> i32>,
        pub retire: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        pub drop: Option<unsafe extern "C" fn(*mut c_void)>,
        pub context: *mut c_void,
    }
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
        pub fn vr_v1_region_from_source(gpu: u64, bytes: u64, provider: *const SourceProvider,
            provider_size: usize, out: *mut u64) -> i32;
        pub fn vr_v1_region_from_host(gpu: u64, bytes: *const u8, n: usize, out: *mut u64) -> i32;
        pub fn vr_v1_region_from_host_parts(gpu: u64, parts: *const HostPart, count: usize,
            offsets: *mut u64, out: *mut u64) -> i32;
        pub fn vr_v1_use_ptr(used: u64, index: usize, out: *mut u64) -> i32;
        pub fn vr_v1_use(
            gpu: u64,
            regions: *const u64,
            n: usize,
            stream: u64,
            out: *mut u64,
        ) -> i32;
        pub fn vr_v1_done(used: u64) -> i32;
        pub fn vr_v1_use_wait_ready(used: u64) -> i32;
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
        pub fn vr_v1_plan_activate(plan: u64, name: *const c_char) -> i32;
        pub fn vr_v1_plan_deactivate(plan: u64) -> i32;
        pub fn vr_v1_phase_enter(plan: u64, name: *const c_char, key: *const c_char) -> i32;
        pub fn vr_v1_phase_leave(plan: u64, completed: i32) -> i32;
        pub fn vr_v1_stats(gpu: u64, out: *mut Stats, size: usize) -> i32;
        pub fn vr_v1_room(gpu: u64, out: *mut Room, size: usize) -> i32;
        pub fn vr_v1_region_set_priority(region: u64, priority: i32) -> i32;
        pub fn vr_v1_split_select(gpu: u64, key: *const c_char, choices: *const SplitChoice, n: usize,
            stream: u64, out: *mut SplitSelection, size: usize) -> i32;
        pub fn vr_v1_split_begin(plan: u64, key: *const c_char, choice: *const c_char, stream: u64,
            generation: u64, out: *mut u64) -> i32;
        pub fn vr_v1_split_end(plan: u64, split: u64, submitted: i32, out: *mut SplitEnd, size: usize) -> i32;
    }
}
