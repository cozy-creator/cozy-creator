//! Size-aware allocation recovery beneath Torch's existing caching allocator.
//! Only verified CUDA runtime import slots are replaced; no Torch C++ ABI is linked.
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "linux")]
#[path = "elf.rs"]
mod patch;
#[cfg(target_os = "windows")]
#[path = "iat.rs"]
mod patch;

type Malloc = unsafe extern "C" fn(*mut *mut c_void, usize) -> i32;
type Free = unsafe extern "C" fn(*mut c_void) -> i32;
type GetDevice = unsafe extern "C" fn(*mut i32) -> i32;
type GetLastError = unsafe extern "C" fn() -> i32;

pub(super) struct Functions {
    pub malloc: Malloc,
    pub free: Free,
    pub device: GetDevice,
    pub last_error: GetLastError,
}
static OWNER_PID: AtomicU32 = AtomicU32::new(0);
static FUNCTIONS: OnceLock<Functions> = OnceLock::new();
static BOOK: OnceLock<Mutex<Book>> = OnceLock::new();
static INSTALL: Mutex<()> = Mutex::new(());
static INSTALLED: OnceLock<Result<usize, String>> = OnceLock::new();

#[derive(Default)]
struct Device {
    reserved: usize,
    limit: Option<usize>,
    /// The size Torch was last refused, until its next allocation on this device.
    refused: Option<usize>,
    calls: u64,
    retries: u64,
    reclaimed: usize,
    refusals: u64,
}
#[derive(Default)]
struct Book {
    pointers: BTreeMap<usize, (i32, usize)>,
    devices: BTreeMap<i32, Device>,
    pid: u32,
}

fn book() -> &'static Mutex<Book> {
    BOOK.get_or_init(|| {
        OWNER_PID.store(std::process::id(), Ordering::Release);
        Mutex::new(Book {
            pid: std::process::id(),
            ..Book::default()
        })
    })
}

fn error_code(error: varena::Error) -> i32 {
    match error {
        varena::Error::OutOfMemory { .. } => 2,
        varena::Error::DeviceFault { code, .. } | varena::Error::Cuda { code, .. } => code,
        varena::Error::CaptureActive => 2,
        _ => 999,
    }
}

/// Called synchronously inside native Torch allocation, including worker threads. Never enters
/// Python, performs I/O, or reexecutes any package operation. CUDA owns the result/error code.
unsafe extern "C" fn malloc(output: *mut *mut c_void, bytes: usize) -> i32 {
    std::panic::catch_unwind(|| allocate(output, bytes)).unwrap_or(999)
}

fn allocate(output: *mut *mut c_void, bytes: usize) -> i32 {
    if OWNER_PID.load(Ordering::Acquire) != std::process::id() {
        return 999;
    }
    let Some(api) = FUNCTIONS.get() else {
        return 999;
    };
    let mut device = -1;
    // SAFETY: CUDA's stable runtime ABI writes the current ordinal to this stack cell.
    let status = unsafe { (api.device)(&mut device) };
    if status != 0 {
        return status;
    }
    let Ok(mut book) = book().lock() else {
        return 999;
    };
    if book.pid != std::process::id() {
        return 999;
    }
    let row = book.devices.entry(device).or_default();
    row.calls += 1;
    // Weights go only after Torch's own cache. Torch's caching allocator answers a refused
    // cudaMalloc by freeing every cached block it holds (`release_cached_blocks`) and asking once
    // more for the same size, and raises OOM only if that fails too. So a first refusal goes back
    // to Torch untouched, and only its retry may evict weights.
    let retry = row.refused.take() == Some(bytes);
    if let Some(limit) = row.limit {
        let resident = match varena::resident_bytes(device) {
            Ok(n) => n,
            Err(e) => return error_code(e),
        };
        let need = resident
            .saturating_add(row.reserved)
            .saturating_add(bytes)
            .saturating_sub(limit);
        if need > 0 {
            match varena::is_recovery_blocked(device) {
                Ok(true) => return 2,
                Ok(false) => (),
                Err(error) => return error_code(error),
            }
            if !retry {
                row.refused = Some(bytes);
                return 2;
            }
            match varena::reclaim(device, need) {
                Ok(report) => {
                    row.reclaimed += report.reclaimed_bytes;
                }
                Err(error) => return error_code(error),
            }
            let resident = match varena::resident_bytes(device) {
                Ok(n) => n,
                Err(e) => return error_code(e),
            };
            if resident.saturating_add(row.reserved).saturating_add(bytes) > limit {
                row.refusals += 1;
                return 2;
            }
        }
    }
    // SAFETY: the caller supplies CUDA's output pointer; the original ABI receives it unchanged.
    let mut status = unsafe { (api.malloc)(output, bytes) };
    if status == 2 {
        match varena::is_recovery_blocked(device) {
            Ok(true) => return status,
            Ok(false) => (),
            Err(error) => return error_code(error),
        }
        if !retry {
            // Torch clears CUDA's OOM error itself before it frees its cache.
            row.refused = Some(bytes);
            return status;
        }
        // Torch normally consumes a failed cudaMalloc's error before recovery. The hook
        // hides that failure on success, so consume its OOM before retrying it ourselves.
        // SAFETY: this function belongs to the original allocation's CUDA runtime instance.
        let pending = unsafe { (api.last_error)() };
        if pending != 0 && pending != 2 {
            return pending;
        }
        match varena::reclaim(device, bytes) {
            Ok(report) if report.reclaimed_bytes > 0 => {
                row.retries += 1;
                row.reclaimed += report.reclaimed_bytes;
                // SAFETY: only the failed CUDA allocation is retried, once, with the same args.
                status = unsafe { (api.malloc)(output, bytes) };
            }
            Ok(_) => (),
            Err(error) => return error_code(error),
        }
    }
    if status == 0 {
        // SAFETY: a successful CUDA allocation has initialized the caller's output cell.
        let pointer = unsafe { *output } as usize;
        if pointer != 0 {
            row.reserved = row.reserved.saturating_add(bytes);
            book.pointers.insert(pointer, (device, bytes));
        }
    }
    status
}

unsafe extern "C" fn free(pointer: *mut c_void) -> i32 {
    if OWNER_PID.load(Ordering::Acquire) != std::process::id() {
        return 999;
    }
    std::panic::catch_unwind(|| {
        let Some(api) = FUNCTIONS.get() else {
            return 999;
        };
        let Ok(mut book) = book().lock() else {
            return 999;
        };
        if book.pid != std::process::id() {
            return 999;
        }
        // SAFETY: forward the original CUDA pointer; no ownership changes until CUDA succeeds.
        let status = unsafe { (api.free)(pointer) };
        if status == 0 {
            if let Some((device, bytes)) = book.pointers.remove(&(pointer as usize)) {
                let row = book.devices.entry(device).or_default();
                row.reserved = row.reserved.saturating_sub(bytes);
            }
        }
        status
    })
    .unwrap_or(999)
}

pub(super) fn install() -> Result<usize, String> {
    let pid = OWNER_PID.load(Ordering::Acquire);
    if pid != 0 && pid != std::process::id() {
        return Err("CUDA allocation hooks cannot be used after fork".into());
    }
    let _lock = INSTALL
        .lock()
        .map_err(|_| "allocator installation mutex poisoned")?;
    if let Some(result) = INSTALLED.get() {
        return result.clone();
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let plan = patch::discover()?;
        let functions = plan.functions()?;
        let _ = book();
        FUNCTIONS
            .set(functions)
            .map_err(|_| "allocator already installed")?;
        // Failure leaves original slots or a functioning subset; installation never reports
        // success until both allocation and release slots on each supported Torch DSO are set.
        let result = plan.install(malloc as *const () as usize, free as *const () as usize);
        let _ = INSTALLED.set(result.clone());
        result
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    Err("native Torch allocation hook is not implemented on this platform".into())
}

pub(super) fn set_limit(device: i32, limit: Option<usize>) -> Result<(), String> {
    if FUNCTIONS.get().is_none() {
        return Err("install the native allocation hook first".into());
    }
    if OWNER_PID.load(Ordering::Acquire) != std::process::id() {
        return Err("CUDA allocation hooks cannot be used after fork".into());
    }
    let mut book = book().lock().map_err(|_| "allocator ledger poisoned")?;
    let row = book.devices.entry(device).or_default();
    if row.reserved != 0 {
        return Err("configure the managed allocation limit before Torch reserves memory".into());
    }
    if limit == Some(0) {
        return Err("managed allocation limit must be positive".into());
    }
    row.limit = limit;
    Ok(())
}

pub(super) fn stats(device: i32) -> Result<BTreeMap<&'static str, u64>, String> {
    let owner = OWNER_PID.load(Ordering::Acquire);
    if owner != 0 && owner != std::process::id() {
        return Err("CUDA allocation hooks cannot be used after fork".into());
    }
    let book = book().lock().map_err(|_| "allocator ledger poisoned")?;
    let default = Device::default();
    let row = book.devices.get(&device).unwrap_or(&default);
    Ok(BTreeMap::from([
        ("torch_reserved_bytes", row.reserved as u64),
        ("allocation_calls", row.calls),
        ("oom_retries", row.retries),
        ("reclaimed_bytes", row.reclaimed as u64),
        ("budget_refusals", row.refusals),
        ("activation_admission_limit", row.limit.unwrap_or(0) as u64),
    ]))
}
