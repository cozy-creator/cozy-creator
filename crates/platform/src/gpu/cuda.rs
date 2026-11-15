//! Each GPU's memory class from the CUDA driver's device attributes (`libcuda.so.1`,
//! `nvcuda.dll`, opened at run time), and whether a process can open it at all.
#![allow(unsafe_code)]
use super::MemoryClass;
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};

const INTEGRATED: c_int = 18;
const PAGEABLE_MEMORY_ACCESS_USES_HOST_PAGE_TABLES: c_int = 100;

#[cfg(windows)]
const LIBRARY: &str = "nvcuda.dll";
#[cfg(not(windows))]
const LIBRARY: &str = "libcuda.so.1";

/// What a process that opened the CUDA driver found of the GPUs at `bus_ids`, in their order: each one's memory class
/// (integrated → `Unified`, else pageable memory through the host's page tables → `Coherent`, else `Discrete`; `None` when an
/// attribute isn't answered). With `contexts` each GPU's primary context is also made current, given 1 MiB and released: the
/// whole path a run's executor takes into the driver. An error names what the driver refused.
///
/// It initializes CUDA in the calling process, whose exit then passes through the driver: TensorD runs it in a process of its
/// own (`tensord probe-gpus`), never in itself (cl-977: a TensorD that had only initialized CUDA could not exit, its last
/// thread waiting in the driver's UVM teardown).
pub fn open(bus_ids: &[String], contexts: bool) -> Result<Vec<Option<MemoryClass>>, String> {
    type Init = unsafe extern "C" fn(c_uint) -> c_int;
    type ByBusId = unsafe extern "C" fn(*mut c_int, *const c_char) -> c_int;
    type Attribute = unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_int;
    type Retain = unsafe extern "C" fn(*mut *mut c_void, c_int) -> c_int;
    type Release = unsafe extern "C" fn(c_int) -> c_int;
    type SetCurrent = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Alloc = unsafe extern "C" fn(*mut u64, usize) -> c_int;
    type Free = unsafe extern "C" fn(u64) -> c_int;
    type Name = unsafe extern "C" fn(c_int, *mut *const c_char) -> c_int;
    // SAFETY: the CUDA driver library, loaded by its documented name.
    let library = unsafe { Library::new(LIBRARY) }.map_err(|e| format!("no CUDA driver ({LIBRARY}): {e}"))?;
    macro_rules! symbol {
        ($ty:ty, $name:literal) => {
            // SAFETY: the driver API's exported signature (cuda.h) for that symbol, copied out as a plain fn pointer.
            unsafe { library.get::<$ty>(concat!($name, "\0").as_bytes()) }
                .map(|s| *s)
                .map_err(|_| format!("the CUDA driver has no {}", $name))
        };
    }
    let (init, by_bus_id, attribute) = (symbol!(Init, "cuInit")?, symbol!(ByBusId, "cuDeviceGetByPCIBusId")?, symbol!(Attribute, "cuDeviceGetAttribute")?);
    let name = symbol!(Name, "cuGetErrorName").ok();
    let said = |op: &str, code: c_int| {
        let mut text: *const c_char = std::ptr::null();
        // SAFETY: an out-param the driver points at a static string, or leaves null.
        let known = name.is_some_and(|name| unsafe { name(code, &mut text) } == 0 && !text.is_null());
        // SAFETY: a NUL-terminated string the driver owns for the process's life.
        let text = if known { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() } else { format!("error {code}") };
        format!("{op}: {text}")
    };
    // SAFETY: cuInit takes flags 0.
    match unsafe { init(0) } {
        0 => {}
        code => return Err(said("cuInit", code)),
    }
    let mut classes = Vec::with_capacity(bus_ids.len());
    for bus_id in bus_ids {
        let id = CString::new(bus_id.as_str()).map_err(|_| format!("bus id {bus_id:?}"))?;
        let mut device: c_int = 0;
        // SAFETY: an out-param and a NUL-terminated string that outlives the call.
        match unsafe { by_bus_id(&mut device, id.as_ptr()) } {
            0 => {}
            code => return Err(said(&format!("GPU {bus_id}"), code)),
        }
        let has = |attr: c_int| read(attribute, device, attr);
        classes.push(match (has(INTEGRATED), has(PAGEABLE_MEMORY_ACCESS_USES_HOST_PAGE_TABLES)) {
            (Some(true), _) => Some(MemoryClass::Unified),
            (Some(false), Some(true)) => Some(MemoryClass::Coherent),
            (Some(false), Some(false)) => Some(MemoryClass::Discrete),
            _ => None,
        });
        if contexts {
            let (retain, release, current) = (symbol!(Retain, "cuDevicePrimaryCtxRetain")?, symbol!(Release, "cuDevicePrimaryCtxRelease_v2")?, symbol!(SetCurrent, "cuCtxSetCurrent")?);
            let (alloc, free) = (symbol!(Alloc, "cuMemAlloc_v2")?, symbol!(Free, "cuMemFree_v2")?);
            let checked = |op: &str, code: c_int| match code {
                0 => Ok(()),
                code => Err(said(&format!("GPU {bus_id}: {op}"), code)),
            };
            let (mut context, mut memory): (*mut c_void, u64) = (std::ptr::null_mut(), 0);
            // SAFETY: out-params the driver fills, on the device it named; the memory is freed and the context released
            // before the next GPU.
            unsafe {
                checked("cuDevicePrimaryCtxRetain", retain(&mut context, device))?;
                checked("cuCtxSetCurrent", current(context))?;
                checked("cuMemAlloc", alloc(&mut memory, 1 << 20))?;
                checked("cuMemFree", free(memory))?;
                checked("cuCtxSetCurrent", current(std::ptr::null_mut()))?;
                checked("cuDevicePrimaryCtxRelease", release(device))?;
            }
        }
    }
    // The driver stays loaded: it registered exit handlers inside itself at cuInit.
    std::mem::forget(library);
    Ok(classes)
}

fn read(
    attribute: unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_int,
    device: c_int,
    attr: c_int,
) -> Option<bool> {
    let mut value: c_int = 0;
    // SAFETY: an out-param for a device the driver just named.
    (unsafe { attribute(&mut value, attr, device) } == 0).then_some(value != 0)
}
