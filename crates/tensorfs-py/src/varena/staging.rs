//! Host bytes the executors of a group exchange through: the Runtime's Ulysses all-to-all staged in
//! host memory, where its GPUs have no peer access. One shared mapping per GPU, mapped and
//! registered by every executor of the group. Its owner makes it from its GPU's host tier as it
//! would a retained region's copy (`Tier::stage`: weight pages shed first, idle peers asked) and
//! holds it until it closes: varena's host number counts it (`Tier::held`, the host's ledger
//! page) and it is never given back. A peer maps the owner's bytes and counts nothing.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pyo3::{exceptions::PyRuntimeError, prelude::*};
use tensorfs::io::cuda::HostPin;

use super::pages::Tier;

/// Names this process's mappings apart.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A shared mapping, registered with one GPU's context.
struct Mapping {
    ptr: *mut u8,
    len: usize,
    pin: Option<HostPin>,
}
// SAFETY: an owned mapping; its pin is a registration handle used with its context entered.
unsafe impl Send for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        if let Some(pin) = self.pin.as_mut() {
            if pin.release().is_err() {
                return; // The driver may still touch it: the mapping stays, with its registration.
            }
        }
        #[cfg(target_os = "linux")]
        // SAFETY: mapped in `map` with this length; nothing else owns it.
        unsafe {
            libc::munmap(self.ptr.cast(), self.len)
        };
    }
}

#[cfg(target_os = "linux")]
fn shm_name(name: &str) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(name).map_err(|error| error.to_string())
}

/// `name`'s `len` bytes mapped, faulted in and registered for `device`; `create` makes them, sized
/// now so a short `/dev/shm` refuses here, never with a SIGBUS in a copy.
#[cfg(target_os = "linux")]
fn map(name: &str, len: usize, create: bool, device: i32) -> Result<Mapping, String> {
    use std::io::Error;
    let c = shm_name(name)?;
    let flags = if create {
        libc::O_CREAT | libc::O_EXCL | libc::O_RDWR
    } else {
        libc::O_RDWR
    };
    // SAFETY: a valid C string.
    let fd = unsafe { libc::shm_open(c.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(format!("shm_open {name}: {}", Error::last_os_error()));
    }
    let sized = if create {
        // SAFETY: an open descriptor.
        match unsafe { libc::posix_fallocate(fd, 0, len as libc::off_t) } {
            0 => Ok(()),
            code => Err(format!(
                "{name}: {} bytes: {}",
                len,
                Error::from_raw_os_error(code)
            )),
        }
    } else {
        // SAFETY: a zeroed stat is valid storage for fstat to fill.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: an open descriptor and a writable stat.
        if unsafe { libc::fstat(fd, &mut stat) } != 0 || (stat.st_size as u64) < len as u64 {
            Err(format!("{name} holds fewer than {len} bytes"))
        } else {
            Ok(())
        }
    };
    let ptr = sized.and_then(|()| {
        // SAFETY: an open descriptor, mapped shared for its length.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            Err(format!("mmap {name}: {}", Error::last_os_error()))
        } else {
            Ok(ptr)
        }
    });
    // SAFETY: the descriptor is ours; the mapping keeps its own reference.
    unsafe { libc::close(fd) };
    let ptr = match ptr {
        Ok(ptr) => ptr,
        Err(error) => {
            if create {
                // SAFETY: a valid C string; the name this call made.
                unsafe { libc::shm_unlink(c.as_ptr()) };
            }
            return Err(error);
        }
    };
    let mut mapping = Mapping {
        ptr: ptr.cast(),
        len,
        pin: None,
    };
    super::pages::fault_in(ptr as usize, len);
    // SAFETY: the mapping outlives its pin: `Drop` releases it first.
    let pin = unsafe { tensorfs::io::register(device, mapping.ptr, len) }
        .map_err(|error| error.to_string());
    match pin {
        Ok(pin) => mapping.pin = Some(pin),
        Err(error) => {
            drop(mapping);
            if create {
                // SAFETY: a valid C string; the name this call made.
                unsafe { libc::shm_unlink(c.as_ptr()) };
            }
            return Err(error);
        }
    }
    Ok(mapping)
}

#[cfg(not(target_os = "linux"))]
fn map(_: &str, _: usize, _: bool, _: i32) -> Result<Mapping, String> {
    Err("host staging maps shared memory on Linux only".into())
}

#[pyclass(name = "_VarenaStaging", module = "tensorfs._ext", frozen)]
pub(super) struct PyStaging {
    mapping: Mutex<Option<Mapping>>,
    /// The owner's tier, which counts the bytes while they are mapped; None for a peer's mapping.
    owner: Option<Arc<Tier>>,
    linked: Mutex<bool>,
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    bytes: usize,
    #[pyo3(get)]
    ptr: usize,
}

impl PyStaging {
    fn new(mapping: Mapping, name: String, owner: Option<Arc<Tier>>) -> Self {
        let (ptr, bytes) = (mapping.ptr as usize, mapping.len);
        let linked = Mutex::new(owner.is_some());
        Self {
            mapping: Mutex::new(Some(mapping)),
            owner,
            linked,
            name,
            bytes,
            ptr,
        }
    }

    fn unlink_name(&self) {
        let mut linked = self
            .linked
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if std::mem::take(&mut *linked) {
            #[cfg(target_os = "linux")]
            if let Ok(c) = shm_name(&self.name) {
                // SAFETY: a valid C string; the name this owner made.
                unsafe { libc::shm_unlink(c.as_ptr()) };
            }
        }
    }

    fn end(&self) {
        let mapping = self
            .mapping
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if mapping.is_none() {
            return;
        }
        drop(mapping); // Unregistering waits on the device.
        self.unlink_name();
        if let Some(tier) = &self.owner {
            tier.unstage(self.bytes);
        }
    }
}

impl Drop for PyStaging {
    fn drop(&mut self) {
        self.end();
    }
}

/// `bytes` of staging (page rounded) made from `tier`'s room for its GPU, or None: no room now.
pub(super) fn make(tier: &Arc<Tier>, bytes: usize) -> Result<Option<PyStaging>, String> {
    let len = bytes.max(1).next_multiple_of(4096);
    let name = format!(
        "/cozy-staging-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut made = None;
    if !tier.stage(len, || {
        made = Some(map(&name, len, true, tier.device())?);
        Ok(())
    })? {
        return Ok(None);
    }
    let mapping = made.ok_or("staging was admitted and not made")?;
    Ok(Some(PyStaging::new(mapping, name, Some(tier.clone()))))
}

#[pymethods]
impl PyStaging {
    /// Another executor's staging `name`, its `bytes` mapped and registered for `device`.
    #[staticmethod]
    fn open(py: Python<'_>, device: i32, name: String, bytes: usize) -> PyResult<Self> {
        let mapping = py
            .detach(|| map(&name, bytes, false, device))
            .map_err(PyRuntimeError::new_err)?;
        Ok(Self::new(mapping, name, None))
    }
    /// Its name goes: once every peer has opened it, so nothing outlives the processes.
    fn unlink(&self) {
        self.unlink_name();
    }
    /// Unregistered and unmapped; the owner's bytes leave its tier.
    fn close(&self, py: Python<'_>) {
        py.detach(|| self.end());
    }
}
