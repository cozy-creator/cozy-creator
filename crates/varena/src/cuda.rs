//! The CUDA Driver API boundary. No imported handles or foreign reservations.
#![allow(unsafe_code)]

use crate::{Error, Result};
use libloading::Library;
use std::ffi::{c_int, c_uint, c_void};
use std::sync::atomic::{AtomicU64, Ordering};

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
type CopyHost = unsafe extern "system" fn(Pointer, *const c_void, usize) -> Status;
type EventCreate = unsafe extern "system" fn(*mut Object, c_uint) -> Status;
type EventRecord = unsafe extern "system" fn(Object, Object) -> Status;
type EventOp = unsafe extern "system" fn(Object) -> Status;
type IsCapturing = unsafe extern "system" fn(Object, *mut c_int) -> Status;
type MemoryInfo = unsafe extern "system" fn(*mut usize, *mut usize) -> Status;

pub(crate) struct Driver {
    // Keep a primary-context retain for the process lifetime. CUDA reset/fork is unsupported.
    context: usize,
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
    copy_host: CopyHost,
    event_create: EventCreate,
    event_record: EventRecord,
    event_query: EventOp,
    event_synchronize: EventOp,
    event_destroy: EventOp,
    is_capturing: IsCapturing,
    stream_query: EventOp,
    memory_info: MemoryInfo,
    memory_calls: AtomicU64,
    event_queries: AtomicU64,
    event_records: AtomicU64,
    event_waits: AtomicU64,
    pub(crate) granularity: usize,
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
            copy_host: symbol!("cuMemcpyHtoD_v2", CopyHost),
            event_create: symbol!("cuEventCreate", EventCreate),
            event_record: symbol!("cuEventRecord", EventRecord),
            event_query: symbol!("cuEventQuery", EventOp),
            event_synchronize: symbol!("cuEventSynchronize", EventOp),
            event_destroy: symbol!("cuEventDestroy_v2", EventOp),
            is_capturing: symbol!("cuStreamIsCapturing", IsCapturing),
            stream_query: symbol!("cuStreamQuery", EventOp),
            memory_info: symbol!("cuMemGetInfo_v2", MemoryInfo),
            memory_calls: AtomicU64::new(0),
            event_queries: AtomicU64::new(0),
            event_records: AtomicU64::new(0),
            event_waits: AtomicU64::new(0),
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
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: valid output pointer; size is rounded to this driver's allocation granularity.
        status("cuMemAddressReserve", unsafe {
            (self.reserve)(&mut ptr, bytes, 0, 0, 0)
        })?;
        Ok(ptr)
    }

    pub(crate) fn free_address(&self, ptr: u64, bytes: usize) -> Result<()> {
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: only a successfully unmapped owned reservation reaches here.
        status("cuMemAddressFree", unsafe {
            (self.address_free)(ptr, bytes)
        })
    }

    pub(crate) fn create(&self, bytes: usize) -> Result<u64> {
        let mut handle = 0;
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: valid output pointer and properties; bytes satisfy granularity.
        let code = unsafe { (self.create)(&mut handle, bytes, &self.properties(), 0) };
        if code == 2 {
            return Err(Error::OutOfMemory {
                requested_bytes: bytes,
            });
        }
        status("cuMemCreate", code)?;
        Ok(handle)
    }

    pub(crate) fn map(&self, ptr: u64, bytes: usize, handle: u64) -> Result<()> {
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned, unoccupied reservation and equally-sized owned allocation.
        status("cuMemMap", unsafe { (self.map)(ptr, bytes, 0, handle, 0) })?;
        let access = Access {
            location: Location {
                kind: 1,
                id: self.device,
            },
            flags: 3, // CU_MEM_ACCESS_FLAGS_PROT_READWRITE
        };
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the entire range just mapped above; one descriptor grants local device access.
        status("cuMemSetAccess", unsafe {
            (self.set_access)(ptr, bytes, &access, 1)
        })
    }

    pub(crate) fn unmap(&self, ptr: u64, bytes: usize) -> Result<()> {
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: owned whole mapping, no pins, and checked completion of all uses before call.
        status("cuMemUnmap", unsafe { (self.unmap)(ptr, bytes) })
    }

    pub(crate) fn release(&self, handle: u64) -> Result<()> {
        self.memory_calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: owned allocation handle; caller has confirmed unmap before releasing.
        status("cuMemRelease", unsafe { (self.release)(handle) })
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
        self.event_records.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned live event; the caller vouches for a live stream of this context.
        status("cuEventRecord", unsafe {
            (self.event_record)(event as Object, stream as Object)
        })
    }

    /// Whether all work captured by the event's latest record has completed.
    pub(crate) fn query(&self, event: usize) -> Result<bool> {
        self.event_queries.fetch_add(1, Ordering::Relaxed);
        // SAFETY: an owned live event; callers exclude CUDA graph capture.
        match unsafe { (self.event_query)(event as Object) } {
            600 => Ok(false), // CUDA_ERROR_NOT_READY
            code => status("cuEventQuery", code).map(|()| true),
        }
    }

    pub(crate) fn wait_event(&self, event: usize) -> Result<()> {
        self.event_waits.fetch_add(1, Ordering::Relaxed);
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

    pub(crate) fn copy_from_host(&self, ptr: u64, bytes: &[u8]) -> Result<()> {
        // SAFETY: Fill owns the exclusive mapped destination; slice lives through synchronous copy.
        status("cuMemcpyHtoD", unsafe {
            (self.copy_host)(ptr, bytes.as_ptr().cast(), bytes.len())
        })
    }

    pub(crate) fn calls(&self) -> (u64, u64, u64, u64) {
        (
            self.memory_calls.load(Ordering::Relaxed),
            self.event_queries.load(Ordering::Relaxed),
            self.event_records.load(Ordering::Relaxed),
            self.event_waits.load(Ordering::Relaxed),
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
