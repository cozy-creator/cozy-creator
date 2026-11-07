//! The CUDA Driver API boundary. No imported handles or foreign reservations.
#![allow(unsafe_code)]

use crate::{Error, Result};
use libloading::Library;
use std::ffi::{c_int, c_uchar, c_uint, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::sync::Arc;

type Status = c_int;
type Context = *mut c_void;
type Object = *mut c_void; // CUevent or CUstream
type Handle = u64;
type Pointer = u64;

#[repr(C)]
#[derive(Clone, Copy)]
struct Location {
    kind: c_int,
    id: c_int,
}

#[repr(C)]
struct AllocationProperties {
    kind: c_int,
    handles: c_int,
    location: Location,
    windows_metadata: *mut c_void,
    flags: [u8; 8],
}

#[repr(C)]
struct Access {
    location: Location,
    flags: c_int,
}

// CUDAAPI is __stdcall on Windows and the C ABI elsewhere.
type Init = unsafe extern "system" fn(c_uint) -> Status;
type DeviceGet = unsafe extern "system" fn(*mut c_int, c_int) -> Status;
#[cfg(target_os = "linux")]
#[repr(C)]
struct Uuid { bytes: [u8; 16] }
#[cfg(target_os = "linux")]
type DeviceUuid = unsafe extern "system" fn(*mut Uuid, c_int) -> Status;
type Retain = unsafe extern "system" fn(*mut Context, c_int) -> Status;
type Push = unsafe extern "system" fn(Context) -> Status;
type Pop = unsafe extern "system" fn(*mut Context) -> Status;
type Synchronize = unsafe extern "system" fn() -> Status;
type Granularity =
    unsafe extern "system" fn(*mut usize, *const AllocationProperties, c_int) -> Status;
type Reserve = unsafe extern "system" fn(*mut Pointer, usize, usize, Pointer, u64) -> Status;
type AddressFree = unsafe extern "system" fn(Pointer, usize) -> Status;
type Create =
    unsafe extern "system" fn(*mut Handle, usize, *const AllocationProperties, u64) -> Status;
type Map = unsafe extern "system" fn(Pointer, usize, usize, Handle, u64) -> Status;
type SetAccess = unsafe extern "system" fn(Pointer, usize, *const Access, usize) -> Status;
type Unmap = unsafe extern "system" fn(Pointer, usize) -> Status;
type Release = unsafe extern "system" fn(Handle) -> Status;
type CopyHostAsync = unsafe extern "system" fn(Pointer, *const c_void, usize, Object) -> Status;
type Fill = unsafe extern "system" fn(Pointer, c_uchar, usize, Object) -> Status;
type StreamCreate = unsafe extern "system" fn(*mut Object, c_uint) -> Status;
type EventCreate = unsafe extern "system" fn(*mut Object, c_uint) -> Status;
type EventRecord = unsafe extern "system" fn(Object, Object) -> Status;
type StreamWaitEvent = unsafe extern "system" fn(Object, Object, c_uint) -> Status;
type StreamContext = unsafe extern "system" fn(Object, *mut Context) -> Status;
type EventOp = unsafe extern "system" fn(Object) -> Status;
type IsCapturing = unsafe extern "system" fn(Object, *mut c_int) -> Status;
type MemoryInfo = unsafe extern "system" fn(*mut usize, *mut usize) -> Status;

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) memory_calls: AtomicU64,
    pub(crate) physical_ooms: AtomicU64,
    /// Development only (`fail_create_after`): calls until cuMemCreate is refused, and how many.
    pub(crate) fail_create: AtomicU64,
    pub(crate) fail_creates: AtomicU64,
    pub(crate) event_queries: AtomicU64,
    pub(crate) event_records: AtomicU64,
    pub(crate) event_waits: AtomicU64,
    pub(crate) context_joins: AtomicU64,
    pub(crate) memory_ns: AtomicU64,
    pub(crate) memory_longest_ns: AtomicU64,
    pub(crate) reclaim_ns: AtomicU64,
    pub(crate) reclaim_longest_ns: AtomicU64,
    pub(crate) reclaim_longest_step: AtomicU64,
    pub(crate) reclaim_step_ns: [AtomicU64; crate::reclaim::STEPS],
}

/// Exact partial HOST VMM ownership. There is deliberately no Drop cleanup: the
/// Fill resource retains this state through checked retirement or native quarantine.
#[derive(Default)]
pub(crate) struct HostAllocation {
    pointer: u64,
    handle: u64,
    bytes: usize,
    mapped: bool,
}
impl HostAllocation {
    pub(crate) fn pointer(&self) -> usize { self.pointer as usize }
}

pub(crate) struct Driver {
    // Keep a primary-context retain for the process lifetime. CUDA reset/fork is unsupported.
    context: usize,
    counters: Arc<Counters>,
    device: c_int,
    push: Push,
    pop: Pop,
    synchronize: Synchronize,
    reserve: Reserve,
    address_free: AddressFree,
    create: Create,
    map: Map,
    set_access: SetAccess,
    unmap: Unmap,
    release: Release,
    copy_host_async: CopyHostAsync,
    fill: Fill,
    host_granularity: Granularity,
    stream_create: StreamCreate,
    stream_destroy: EventOp,
    event_create: EventCreate,
    event_record: EventRecord,
    event_query: EventOp,
    event_synchronize: EventOp,
    event_destroy: EventOp,
    is_capturing: IsCapturing,
    stream_query: EventOp,
    stream_wait_event: StreamWaitEvent,
    stream_context: StreamContext,
    memory_info: MemoryInfo,
    pub(crate) granularity: usize,
}

/// A device's UUID before any context exists: a context can itself be refused for memory.
#[cfg(target_os = "linux")]
pub(crate) fn device_uuid(ordinal: i32) -> Result<[u8; 16]> {
    // SAFETY: load the vendor CUDA driver, never application-provided plugin code.
    let library = unsafe { Library::new("libcuda.so.1") }.map_err(|error| Error::Unsupported(error.to_string()))?;
    // SAFETY: each symbol uses the documented CUDA Driver API signature.
    let (init, get_device, device_uuid) = unsafe {
        let symbol = |error: libloading::Error| Error::Unsupported(error.to_string());
        (*library.get::<Init>(b"cuInit\0".as_slice()).map_err(symbol)?,
         *library.get::<DeviceGet>(b"cuDeviceGet\0".as_slice()).map_err(symbol)?,
         *library.get::<DeviceUuid>(b"cuDeviceGetUuid_v2\0".as_slice()).map_err(symbol)?)
    };
    // CUDA registers process exit handlers; unloading its library after init is unsafe.
    std::mem::forget(library);
    let (mut device, mut uuid) = (0, Uuid { bytes: [0; 16] });
    // SAFETY: zero flags; each output points to valid local storage.
    unsafe {
        status("cuInit", init(0))?;
        status("cuDeviceGet", get_device(&mut device, ordinal))?;
        status("cuDeviceGetUuid_v2", device_uuid(&mut uuid, device))?;
    }
    Ok(uuid.bytes)
}

impl Driver {
    pub(crate) fn open(ordinal: i32) -> Result<Self> {
        #[cfg(windows)]
        let name = "nvcuda.dll";
        #[cfg(not(windows))]
        let name = "libcuda.so.1";
        // SAFETY: load the vendor CUDA driver, never application-provided plugin code.
        let library =
            unsafe { Library::new(name) }.map_err(|error| Error::Unsupported(error.to_string()))?;
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                // SAFETY: each symbol uses the documented CUDA Driver API signature.
                unsafe { library.get::<$ty>(concat!($name, "\0").as_bytes()) }
                    .map(|symbol| *symbol)
                    .map_err(|error| Error::Unsupported(error.to_string()))?
            }};
        }
        let init = symbol!("cuInit", Init);
        let get_device = symbol!("cuDeviceGet", DeviceGet);
        let retain = symbol!("cuDevicePrimaryCtxRetain", Retain);
        let granularity = symbol!("cuMemGetAllocationGranularity", Granularity);
        let mut driver = Self {
            context: 0,
            counters: Arc::new(Counters::default()),
            device: ordinal,
            push: symbol!("cuCtxPushCurrent_v2", Push),
            pop: symbol!("cuCtxPopCurrent_v2", Pop),
            synchronize: symbol!("cuCtxSynchronize", Synchronize),
            reserve: symbol!("cuMemAddressReserve", Reserve),
            address_free: symbol!("cuMemAddressFree", AddressFree),
            create: symbol!("cuMemCreate", Create),
            map: symbol!("cuMemMap", Map),
            set_access: symbol!("cuMemSetAccess", SetAccess),
            unmap: symbol!("cuMemUnmap", Unmap),
            release: symbol!("cuMemRelease", Release),
            copy_host_async: symbol!("cuMemcpyHtoDAsync_v2", CopyHostAsync),
            fill: symbol!("cuMemsetD8Async", Fill),
            host_granularity: granularity,
            stream_create: symbol!("cuStreamCreate", StreamCreate),
            stream_destroy: symbol!("cuStreamDestroy_v2", EventOp),
            event_create: symbol!("cuEventCreate", EventCreate),
            event_record: symbol!("cuEventRecord", EventRecord),
            event_query: symbol!("cuEventQuery", EventOp),
            event_synchronize: symbol!("cuEventSynchronize", EventOp),
            event_destroy: symbol!("cuEventDestroy_v2", EventOp),
            is_capturing: symbol!("cuStreamIsCapturing", IsCapturing),
            stream_query: symbol!("cuStreamQuery", EventOp),
            stream_wait_event: symbol!("cuStreamWaitEvent", StreamWaitEvent),
            stream_context: symbol!("cuStreamGetCtx", StreamContext),
            memory_info: symbol!("cuMemGetInfo_v2", MemoryInfo),
            granularity: 0,
        };
        // CUDA registers process exit handlers; unloading its library after init is unsafe.
        std::mem::forget(library);
        // SAFETY: zero flags and valid output storage, as required by CUDA.
        status("cuInit", unsafe { init(0) })?;
        // SAFETY: get_device writes a device handle for the supplied ordinal.
        status("cuDeviceGet", unsafe {
            get_device(&mut driver.device, ordinal)
        })?;
        let mut context = std::ptr::null_mut();
        // SAFETY: primary context retain with an initialized device and valid output pointer.
        status("cuDevicePrimaryCtxRetain", unsafe {
            retain(&mut context, driver.device)
        })?;
        driver.context = context as usize;
        // SAFETY: properties initialized to pinned, local, non-exportable device memory.
        status("cuMemGetAllocationGranularity", unsafe {
            granularity(&mut driver.granularity, &driver.properties(), 0)
        })?;
        if driver.granularity == 0 {
            return Err(Error::Unsupported(
                "CUDA returned zero allocation granularity".into(),
            ));
        }
        Ok(driver)
    }

    fn properties(&self) -> AllocationProperties {
        AllocationProperties {
            kind: 1, // CU_MEM_ALLOCATION_TYPE_PINNED
            handles: 0,
            location: Location {
                kind: 1,
                id: self.device,
            },
            windows_metadata: std::ptr::null_mut(),
            flags: [0; 8],
        }
    }

    pub(crate) fn enter(&self) -> Result<ContextGuard<'_>> {
        // SAFETY: the context is retained for the process lifetime. Push/pop is thread local.
        status("cuCtxPushCurrent", unsafe {
            (self.push)(self.context as Context)
        })?;
        Ok(ContextGuard { driver: self })
    }

    pub(crate) fn synchronize(&self) -> Result<()> {
        self.counters.context_joins.fetch_add(1, Ordering::Relaxed);
        // SAFETY: every caller enters this driver's primary context first.
        status("cuCtxSynchronize", unsafe { (self.synchronize)() })
    }

    pub(crate) fn memory_info(&self) -> Result<(usize, usize)> {
        let (mut free, mut total) = (0, 0);
        // SAFETY: valid output storage; the caller entered this driver's primary context.
        status("cuMemGetInfo", unsafe {
            (self.memory_info)(&mut free, &mut total)
        })?;
        Ok((free, total))
    }

    pub(crate) fn reserve(&self, bytes: usize) -> Result<u64> {
        let mut ptr = 0;
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: valid output pointer; size is rounded to this driver's allocation granularity.
        status("cuMemAddressReserve", unsafe {
            (self.reserve)(&mut ptr, bytes, 0, 0, 0)
        })?;
        Ok(ptr)
    }

    pub(crate) fn free_address(&self, ptr: u64, bytes: usize) -> Result<()> {
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: only a successfully unmapped owned reservation reaches here.
        status("cuMemAddressFree", unsafe {
            (self.address_free)(ptr, bytes)
        })
    }

    fn timed<T>(&self, call: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = call();
        let ns = started.elapsed().as_nanos() as u64;
        self.counters.memory_ns.fetch_add(ns, Ordering::Relaxed);
        self.counters.memory_longest_ns.fetch_max(ns, Ordering::Relaxed);
        value
    }

    /// A reclaim step's time, waits included.
    pub(crate) fn note_reclaim(&self, step: usize, elapsed: Duration) {
        let ns = elapsed.as_nanos() as u64;
        self.counters.reclaim_ns.fetch_add(ns, Ordering::Relaxed);
        self.counters.reclaim_step_ns[step].fetch_add(ns, Ordering::Relaxed);
        if self.counters.reclaim_longest_ns.fetch_max(ns, Ordering::Relaxed) < ns {
            self.counters.reclaim_longest_step.store(step as u64, Ordering::Relaxed);
        }
    }

    /// Reclaim time by step, in ORDER.
    pub(crate) fn reclaim_steps(&self) -> [u64; crate::reclaim::STEPS] {
        std::array::from_fn(|step| self.counters.reclaim_step_ns[step].load(Ordering::Relaxed))
    }

    /// Driver memory call time and longest; reclaim time, longest and that step's place in ORDER.
    pub(crate) fn times(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.counters.memory_ns.load(Ordering::Relaxed),
            self.counters.memory_longest_ns.load(Ordering::Relaxed),
            self.counters.reclaim_ns.load(Ordering::Relaxed),
            self.counters.reclaim_longest_ns.load(Ordering::Relaxed),
            self.counters.reclaim_longest_step.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn create(&self, bytes: usize) -> crate::AllocationResult<u64> {
        let mut handle = 0;
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        match self.counters.fail_create.load(Ordering::Acquire) {
            0 => {}
            1 if self.counters.fail_creates.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_ok() => {
                self.counters.physical_ooms.fetch_add(1, Ordering::Relaxed);
                return Err(crate::AllocationFailure::DriverOutOfMemory(bytes));
            }
            1 => self.counters.fail_create.store(0, Ordering::Release),
            _ => { self.counters.fail_create.fetch_sub(1, Ordering::AcqRel); }
        }
        // SAFETY: valid output pointer and properties; bytes satisfy granularity.
        let code = self.timed(|| unsafe { (self.create)(&mut handle, bytes, &self.properties(), 0) });
        if code == 2 {
            self.counters.physical_ooms.fetch_add(1, Ordering::Relaxed);
            return Err(crate::AllocationFailure::DriverOutOfMemory(bytes));
        }
        status("cuMemCreate", code)?;
        Ok(handle)
    }

    pub(crate) fn map(&self, ptr: u64, bytes: usize, handle: u64) -> Result<()> {
        self.map_only(ptr, bytes, handle)?;
        self.grant(ptr, bytes)
    }

    pub(crate) fn map_only(&self, ptr: u64, bytes: usize, handle: u64) -> Result<()> {
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned, unoccupied reservation and equally-sized owned allocation.
        status("cuMemMap", self.timed(|| unsafe { (self.map)(ptr, bytes, 0, handle, 0) }))
    }

    /// One call over a whole mapped range, however many handles back it: per handle it costs
    /// 1.4 ms for 64 MiB, and over 1 GiB of them 4 ms.
    pub(crate) fn grant(&self, ptr: u64, bytes: usize) -> Result<()> {
        let access = Access {
            location: Location {
                kind: 1,
                id: self.device,
            },
            flags: 3, // CU_MEM_ACCESS_FLAGS_PROT_READWRITE
        };
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the entire range just mapped above; one descriptor grants local device access.
        status("cuMemSetAccess", self.timed(|| unsafe { (self.set_access)(ptr, bytes, &access, 1) }))
    }

    pub(crate) fn unmap(&self, ptr: u64, bytes: usize) -> Result<()> {
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: owned whole mapping, no pins, and checked completion of all uses before call.
        status("cuMemUnmap", self.timed(|| unsafe { (self.unmap)(ptr, bytes) }))
    }

    pub(crate) fn release(&self, handle: u64) -> Result<()> {
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: owned allocation handle; caller has confirmed unmap before releasing.
        status("cuMemRelease", self.timed(|| unsafe { (self.release)(handle) }))
    }

    /// A gate cell's pattern, in `stream`'s order; 0 is the default stream.
    pub(crate) fn fill(&self, ptr: u64, value: u8, bytes: usize, stream: usize) -> Result<()> {
        // SAFETY: an owned, mapped range whose owner gave up its contents.
        status("cuMemsetD8Async", unsafe { (self.fill)(ptr, value, bytes, stream as Object) })
    }

    pub(crate) fn create_event(&self) -> Result<usize> {
        let mut event = std::ptr::null_mut();
        // SAFETY: valid output pointer; CU_EVENT_DISABLE_TIMING in the entered primary context.
        status("cuEventCreate", unsafe {
            (self.event_create)(&mut event, 2)
        })?;
        Ok(event as usize)
    }

    pub(crate) fn record(&self, event: usize, stream: usize) -> Result<()> {
        self.counters.event_records.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned live event; the caller vouches for a live stream of this context.
        status("cuEventRecord", unsafe {
            (self.event_record)(event as Object, stream as Object)
        })
    }

    /// Whether all work captured by the event's latest record has completed.
    pub(crate) fn query(&self, event: usize) -> Result<bool> {
        self.counters.event_queries.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned live event; callers exclude CUDA graph capture.
        match unsafe { (self.event_query)(event as Object) } {
            600 => Ok(false), // CUDA_ERROR_NOT_READY
            code => status("cuEventQuery", code).map(|()| true),
        }
    }

    pub(crate) fn wait_event(&self, event: usize) -> Result<()> {
        self.counters.event_waits.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned live event; callers exclude CUDA graph capture.
        status("cuEventSynchronize", unsafe {
            (self.event_synchronize)(event as Object)
        })
    }

    pub(crate) fn destroy_event(&self, event: usize) -> Result<()> {
        // SAFETY: the sole owner destroys this event once and never uses it again.
        status("cuEventDestroy", unsafe {
            (self.event_destroy)(event as Object)
        })
    }

    pub(crate) fn is_capturing(&self, stream: usize) -> Result<bool> {
        let mut capture = 0;
        // SAFETY: valid output pointer; the caller vouches for a live stream of this context.
        status("cuStreamIsCapturing", unsafe {
            (self.is_capturing)(stream as Object, &mut capture)
        })?;
        Ok(capture != 0)
    }

    pub(crate) fn query_stream(&self, stream: usize) -> Result<bool> {
        // SAFETY: retirement's caller still owns this live stream and has stopped submissions.
        match unsafe { (self.stream_query)(stream as Object) } {
            600 => Ok(false),
            code => status("cuStreamQuery", code).map(|()| true),
        }
    }

    pub(crate) fn validate_stream(&self, stream: usize) -> Result<()> {
        let mut context = std::ptr::null_mut();
        // SAFETY: writable output and caller-owned stream; this primary context is entered.
        status("cuStreamGetCtx", unsafe {
            (self.stream_context)(stream as Object, &mut context)
        })?;
        if context as usize != self.context {
            return Err(Error::Invalid("fill stream is not in the arena primary context"));
        }
        Ok(())
    }

    pub(crate) fn order_event(&self, stream: usize, event: usize) -> Result<()> {
        // SAFETY: live same-context consumer and recorded event, retained across this call.
        status("cuStreamWaitEvent", unsafe {
            (self.stream_wait_event)(stream as Object, event as Object, 0)
        })
    }

    pub(crate) fn create_stream(&self) -> Result<usize> {
        let mut stream = std::ptr::null_mut();
        // SAFETY: writable output and entered primary context; always nonblocking.
        status("cuStreamCreate(nonblocking)", unsafe { (self.stream_create)(&mut stream, 1) })?;
        Ok(stream as usize)
    }
    pub(crate) fn destroy_stream(&self, stream: usize) -> Result<()> {
        // SAFETY: caller retains this exact stream and checked every producer complete.
        status("cuStreamDestroy_v2", unsafe { (self.stream_destroy)(stream as Object) })
    }
    pub(crate) fn prepare_host(&self, allocation: &mut HostAllocation, requested: usize) -> Result<()> {
        const MAXIMUM: usize = 2 << 20;
        if requested == 0 || requested > MAXIMUM || allocation.pointer != 0 || allocation.handle != 0 {
            return Err(Error::Invalid("host stage must be empty and between one byte and two MiB"));
        }
        let properties = AllocationProperties {
            kind: 1, // CU_MEM_ALLOCATION_TYPE_PINNED
            handles: 0,
            location: Location { kind: 2, id: 0 }, // CU_MEM_LOCATION_TYPE_HOST
            windows_metadata: std::ptr::null_mut(),
            flags: [0; 8],
        };
        let mut granularity = 0;
        // SAFETY: official HOST properties and writable output; no NUMA identity is guessed.
        status("cuMemGetAllocationGranularity(HOST)", unsafe {
            (self.host_granularity)(&mut granularity, &properties, 0)
        })?;
        if granularity == 0 || granularity > MAXIMUM {
            return Err(Error::Unsupported("HOST VMM granularity exceeds the two MiB staging bound".into()));
        }
        let bytes = requested.checked_add(granularity - 1)
            .and_then(|value| value.checked_div(granularity))
            .and_then(|value| value.checked_mul(granularity))
            .filter(|value| *value <= MAXIMUM)
            .ok_or_else(|| Error::Unsupported("HOST VMM rounding exceeds the two MiB staging bound".into()))?;
        let mut handle = 0;
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: writable handle output; actual HOST granularity determines the backing.
        // Host failure is not a GPU physical shortage and must never ask GPU peers.
        status("cuMemCreate(HOST)", unsafe { (self.create)(&mut handle, bytes, &properties, 0) })?;
        allocation.handle = handle;
        allocation.bytes = bytes;
        allocation.pointer = self.reserve(bytes)?;
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: both reservation and physical handle are owned in allocation.
        status("cuMemMap(HOST)", unsafe {
            (self.map)(allocation.pointer, bytes, 0, handle, 0)
        })?;
        // Record mapping before granting access: access refusal must still unmap it.
        allocation.mapped = true;
        let access = [
            Access { location: Location { kind: 2, id: 0 }, flags: 3 },
            Access { location: Location { kind: 1, id: self.device }, flags: 3 },
        ];
        self.counters.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the whole mapping is owned; grant CPU and this GPU read/write access.
        status("cuMemSetAccess(HOST+device)", unsafe {
            (self.set_access)(allocation.pointer, bytes, access.as_ptr(), access.len())
        })
    }

    /// Called only after native producer completion (or a proved no-write prepare failure).
    /// Successful stages clear their ownership; any refusal preserves every remaining field.
    pub(crate) fn release_host(&self, allocation: &mut HostAllocation) -> Result<()> {
        if allocation.mapped {
            self.unmap(allocation.pointer, allocation.bytes)?;
            allocation.mapped = false;
        }
        if allocation.handle != 0 {
            self.release(allocation.handle)?;
            allocation.handle = 0;
        }
        if allocation.pointer != 0 {
            self.free_address(allocation.pointer, allocation.bytes)?;
            allocation.pointer = 0;
        }
        allocation.bytes = 0;
        Ok(())
    }
    pub(crate) unsafe fn copy_pinned_async(&self, dst: u64, host: usize, bytes: usize, stream: usize) -> Result<()> {
        // SAFETY: caller retains the whole pinned source and exclusive destination spans.
        status("cuMemcpyHtoDAsync_v2", unsafe { (self.copy_host_async)(dst, host as *const c_void, bytes, stream as Object) })
    }

    pub(crate) fn context_joins(&self) -> u64 {
        self.counters.context_joins.load(Ordering::Relaxed)
    }

    pub(crate) fn fail_create_after(&self, n: u64, times: u64) {
        self.counters.fail_creates.store(times, Ordering::Release);
        self.counters.fail_create.store(n, Ordering::Release);
    }

    pub(crate) fn physical_ooms(&self) -> u64 {
        self.counters.physical_ooms.load(Ordering::Relaxed)
    }

    pub(crate) fn calls(&self) -> (u64, u64, u64, u64) {
        (
            self.counters.memory_calls.load(Ordering::Relaxed),
            self.counters.event_queries.load(Ordering::Relaxed),
            self.counters.event_records.load(Ordering::Relaxed),
            self.counters.event_waits.load(Ordering::Relaxed),
        )
    }
}

pub(crate) struct ContextGuard<'a> {
    driver: &'a Driver,
}

impl Drop for ContextGuard<'_> {
    fn drop(&mut self) {
        let mut previous = std::ptr::null_mut();
        // SAFETY: one matching pop on the same thread for the push which constructed this guard.
        // A failing pop never triggers memory reuse; any subsequent operation must push afresh.
        let _ = unsafe { (self.driver.pop)(&mut previous) };
    }
}

fn status(operation: &'static str, code: i32) -> Result<()> {
    match code {
        0 => Ok(()),
        700 | 702 | 709 | 710 | 714..=719 => Err(Error::DeviceFault { operation, code }),
        _ => Err(Error::Cuda { operation, code }),
    }
}
