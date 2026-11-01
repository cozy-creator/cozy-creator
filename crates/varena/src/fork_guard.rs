//! Default-off Linux experiment. The default actual-PID contract is unchanged.
//! WIPEONFORK does not detect separate-TGID CLONE_VM with copied authenticated TLS.
#![allow(unsafe_code)]
use std::{
    cell::Cell,
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering},
};

const FALLBACK: *mut AtomicU32 = 1usize as *mut AtomicU32; // tag only, never dereferenced
static PAGE: AtomicPtr<AtomicU32> = AtomicPtr::new(ptr::null_mut());
static PAGE_BYTES: AtomicUsize = AtomicUsize::new(0);
static FIRST_FAILURE: AtomicU32 = AtomicU32::new(0);
thread_local! { static AUTHENTICATED: Cell<u32> = const { Cell::new(0) }; }

fn inherited() -> crate::Error {
    crate::Error::Invalid("CUDA residency cannot be used after fork")
}

/// None preserves the original PID path. A wiped page is rejected before touching TLS.
pub(crate) fn check() -> Option<crate::Result<()>> {
    let page = PAGE.load(Ordering::Acquire);
    if page.is_null() || page == FALLBACK {
        return None;
    }
    // SAFETY: only a fully initialized, read-only AtomicU32 mapping is published. It lives
    // until process exit; a normal fork gives the child a valid zero-filled private page.
    let owner = unsafe { (*page).load(Ordering::Acquire) };
    if owner == 0 {
        return Some(Err(inherited()));
    }
    Some(AUTHENTICATED.with(|authenticated| {
        if authenticated.get() != owner {
            // Proper new TLS threads authenticate the actual PID before using inherited
            // registries. Copied TLS + CLONE_VM is explicitly not covered by this experiment.
            if std::process::id() != owner {
                return Err(inherited());
            }
            authenticated.set(owner);
        }
        Ok(())
    }))
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum Stage {
    PageSize = 1,
    Map = 2,
    Advice = 3,
    Protect = 4,
}

#[derive(Clone, Copy, Debug)]
pub enum PrepareError {
    Inherited,
    System { stage: Stage, errno: i32 },
}

fn system(stage: Stage) -> PrepareError {
    PrepareError::System {
        stage,
        errno: std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EINVAL),
    }
}

/// A real unpublished mapping. Public only under the experimental feature so the CPU proof
/// can hold this exact initialization window; no fake guard or injected syscall is used.
pub struct Prepared {
    page: *mut AtomicU32,
    bytes: usize,
}

impl Prepared {
    pub fn new() -> Result<Self, PrepareError> {
        crate::check_process_pid().map_err(|_| PrepareError::Inherited)?;
        // SAFETY: sysconf reads the current kernel page size; it has no pointer arguments.
        let bytes = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if bytes <= 0 {
            return Err(system(Stage::PageSize));
        }
        let bytes = bytes as usize;
        // SAFETY: one private anonymous mapping, no caller address or existing memory.
        let raw = unsafe {
            libc::mmap(
                ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(system(Stage::Map));
        }
        let ready = Self {
            page: raw.cast(),
            bytes,
        };
        // SAFETY: this exclusive page is writable and suitably aligned. The owner PID was
        // just authenticated, and the initialized integer has a valid all-zero child value.
        unsafe {
            ready
                .page
                .write(AtomicU32::new(crate::PROCESS.load(Ordering::Relaxed)));
        }
        // SAFETY: exactly the private anonymous mapping owned by ready. Failure retains the
        // original PID path; dropping ready unmaps only this unpublished page.
        if unsafe { libc::madvise(raw, bytes, libc::MADV_WIPEONFORK) } != 0 {
            return Err(system(Stage::Advice));
        }
        // SAFETY: remove write permission before publication; no later code writes this page.
        if unsafe { libc::mprotect(raw, bytes, libc::PROT_READ) } != 0 {
            return Err(system(Stage::Protect));
        }
        Ok(ready)
    }

    pub fn mapping(&self) -> (usize, usize) {
        (self.page as usize, self.bytes)
    }

    pub fn publish(mut self) -> crate::Result<bool> {
        // A Prepared copied by fork must never rearm the inherited allocator.
        crate::check_process_pid()?;
        PAGE_BYTES.store(self.bytes, Ordering::Relaxed);
        let published = PAGE
            .compare_exchange(
                ptr::null_mut(),
                self.page,
                Ordering::Release,
                Ordering::Acquire,
            )
            .is_ok();
        if published {
            self.page = ptr::null_mut(); // the published page is process-lifetime ownership
        }
        Ok(published)
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.page.is_null() {
            // SAFETY: this owner has not published its private mapping, or lost publication.
            // No allocator operation can have obtained it from PAGE.
            let _ = unsafe { libc::munmap(self.page.cast(), self.bytes) };
        }
    }
}

pub(crate) fn arm() -> crate::Result<()> {
    if !PAGE.load(Ordering::Acquire).is_null() {
        return Ok(());
    }
    match Prepared::new() {
        Ok(prepared) => {
            prepared.publish()?;
        }
        Err(PrepareError::Inherited) => return Err(inherited()),
        Err(PrepareError::System { stage, errno }) => {
            let failure = ((stage as u32) << 24) | (errno as u32 & 0x00ff_ffff);
            let _ = FIRST_FAILURE.compare_exchange(0, failure, Ordering::AcqRel, Ordering::Acquire);
            let _ = PAGE.compare_exchange(
                ptr::null_mut(),
                FALLBACK,
                Ordering::Release,
                Ordering::Acquire,
            );
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Status {
    pub backend: &'static str,
    pub marker: u32,
    pub thread_authenticated: bool,
    pub page_address: usize,
    pub page_bytes: usize,
    pub failure_stage: u32,
    pub failure_errno: i32,
}

/// Cold metadata for the explicit experimental proof, never called by alloc/free.
pub fn status() -> Status {
    let page = PAGE.load(Ordering::Acquire);
    let mut status = Status {
        backend: "unarmed_pid",
        marker: 0,
        thread_authenticated: false,
        page_address: 0,
        page_bytes: 0,
        failure_stage: 0,
        failure_errno: 0,
    };
    if page == FALLBACK {
        status.backend = "fallback_pid";
        let failure = FIRST_FAILURE.load(Ordering::Acquire);
        status.failure_stage = failure >> 24;
        status.failure_errno = (failure & 0x00ff_ffff) as i32;
    } else if !page.is_null() {
        // SAFETY: the same process-lifetime read-only mapping as check().
        status.marker = unsafe { (*page).load(Ordering::Acquire) };
        status.backend = if status.marker == 0 {
            "inherited_zero"
        } else {
            "kernel_wipe"
        };
        status.page_address = page as usize;
        status.page_bytes = PAGE_BYTES.load(Ordering::Relaxed);
        if status.marker != 0 {
            status.thread_authenticated = AUTHENTICATED.with(|value| value.get() == status.marker);
        }
    }
    status
}
