//! Linux ownership boundary for the ledger. No CUDA or application callbacks.
#![allow(unsafe_code)]

use super::{Error, Result};
use std::fs::OpenOptions;
use std::os::fd::IntoRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, Ordering};

const FD_SLOTS: usize = 256;
static FDS: [AtomicPtr<Descriptor>; FD_SLOTS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; FD_SLOTS];
static REGISTRY: AtomicBool = AtomicBool::new(false);
static HOOKS: AtomicI32 = AtomicI32::new(0); // 0 uninstalled, 1 ready, negative errno

extern "C" fn prepare_fork() {
    while REGISTRY
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
}
extern "C" fn parent_fork() {
    REGISTRY.store(false, Ordering::Release);
}
extern "C" fn child_fork() {
    // The prepare handler excludes descriptor creation/destruction. Every pointer is an
    // Arc allocation retained by an actual handle; the child neither allocates nor drops it.
    for slot in &FDS {
        let entry = slot.swap(ptr::null_mut(), Ordering::Relaxed);
        if !entry.is_null() {
            // SAFETY: prepare_fork pins registry entries across fork. Atomic invalidation
            // prevents a later inherited destructor from closing a reused numeric FD.
            let fd = unsafe { (*entry).raw.swap(-1, Ordering::Relaxed) };
            if fd >= 0 {
                // SAFETY: this is one inherited ledger FD, closed once. Never F_UNLCK:
                // the parent's open description still owns its OFD locks.
                unsafe { libc::close(fd) };
            }
        }
    }
    REGISTRY.store(false, Ordering::Release);
}

struct Registry;
impl Registry {
    fn lock() -> Self {
        prepare_fork();
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        parent_fork();
    }
}

pub(super) struct Descriptor {
    raw: AtomicI32,
    slot: usize,
    pid: u32,
}

impl Descriptor {
    pub(super) fn open(path: &Path) -> Result<Arc<Self>> {
        let _registry = Registry::lock();
        hooks()?;
        // Hold across open AND registration: a fork between them must not inherit an
        // unregistered description that a later parent lock could attach to.
        let slot = FDS
            .iter()
            .position(|slot| slot.load(Ordering::Relaxed).is_null())
            .ok_or(Error::Full("process ledger descriptor registry"))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        let descriptor = Arc::new(Self {
            raw: AtomicI32::new(file.into_raw_fd()),
            slot,
            pid: std::process::id(),
        });
        FDS[slot].store(Arc::as_ptr(&descriptor).cast_mut(), Ordering::Release);
        Ok(descriptor)
    }

    fn raw(&self) -> Result<i32> {
        if self.pid != std::process::id() {
            return Err(Error::Inherited);
        }
        let fd = self.raw.load(Ordering::Acquire);
        if fd < 0 {
            Err(Error::Inherited)
        } else {
            Ok(fd)
        }
    }

    pub(super) fn identity(&self) -> Result<(u64, u64, u64)> {
        // SAFETY: fstat/fstatfs fill valid local output structs for our live descriptor.
        let (mut stat, mut filesystem): (libc::stat, libc::statfs) = unsafe { std::mem::zeroed() };
        let fd = self.raw()?;
        // SAFETY: this live descriptor and local stat output belong to the synchronous call.
        let status = unsafe { libc::fstat(fd, &mut stat) };
        // SAFETY: this live descriptor and local filesystem output belong to the call.
        let filesystem_status = unsafe { libc::fstatfs(fd, &mut filesystem) };
        if status != 0 || filesystem_status != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: geteuid has no memory arguments.
        if stat.st_uid != unsafe { libc::geteuid() }
            || stat.st_mode & 0o077 != 0
            || stat.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return Err(Error::Incompatible(
                "ledger must be this effective UID's private regular file",
            ));
        }
        if filesystem.f_type != libc::TMPFS_MAGIC {
            return Err(Error::Incompatible(
                "ledger backing directory must be tmpfs",
            ));
        }
        Ok((stat.st_dev, stat.st_ino, stat.st_size as u64))
    }

    pub(super) fn resize(&self, bytes: usize) -> Result<()> {
        // SAFETY: the caller holds the publication lock on a new/empty ledger file.
        if unsafe { libc::ftruncate(self.raw()?, bytes as libc::off_t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(super) fn lock(self: &Arc<Self>, byte: i64, wait: bool) -> Result<Option<Locked>> {
        let fd = self.raw()?;
        loop {
            // SAFETY: a zeroed flock is filled with a one-byte absolute write lock.
            let mut range: libc::flock = unsafe { std::mem::zeroed() };
            range.l_type = libc::F_WRLCK as _;
            range.l_whence = libc::SEEK_SET as _;
            range.l_start = byte;
            range.l_len = 1;
            // SAFETY: OFD fcntl borrows this valid flock for this synchronous call.
            if unsafe {
                libc::fcntl(
                    fd,
                    if wait {
                        libc::F_OFD_SETLKW
                    } else {
                        libc::F_OFD_SETLK
                    },
                    &range,
                )
            } == 0
            {
                return Ok(Some(Locked {
                    descriptor: self.clone(),
                    byte,
                }));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if !wait && matches!(error.raw_os_error(), Some(libc::EACCES | libc::EAGAIN)) {
                return Ok(None);
            }
            return Err(error.into());
        }
    }
}

fn hooks() -> Result<()> {
    // Caller holds REGISTRY. The application must install this first hook before fork can
    // race initialization: a fork already in progress may have read the old handler list.
    if HOOKS.load(Ordering::Relaxed) == 0 {
        // SAFETY: static C handlers use only atomics and close in the child.
        let code = unsafe {
            libc::pthread_atfork(Some(prepare_fork), Some(parent_fork), Some(child_fork))
        };
        // Once installed, readiness precedes releasing REGISTRY. A fork invoking our prepare
        // handler cannot copy an in-progress OnceLock into its child.
        HOOKS.store(if code == 0 { 1 } else { -code }, Ordering::Release);
    }
    let hooks = HOOKS.load(Ordering::Acquire);
    if hooks < 0 {
        Err(Error::Io(std::io::Error::from_raw_os_error(-hooks)))
    } else {
        Ok(())
    }
}

pub(super) fn initialize() -> Result<()> {
    let _registry = Registry::lock();
    hooks()
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        let _registry = Registry::lock();
        let _ = FDS[self.slot].compare_exchange(
            self as *mut Self,
            ptr::null_mut(),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        let fd = self.raw.swap(-1, Ordering::AcqRel);
        if fd >= 0 {
            // SAFETY: exclusive final ownership. Inherited descriptors were invalidated
            // in child_fork, so this cannot close a descriptor reused by the child.
            unsafe { libc::close(fd) };
        }
    }
}

pub(super) struct Locked {
    descriptor: Arc<Descriptor>,
    byte: i64,
}
impl Drop for Locked {
    fn drop(&mut self) {
        let Ok(fd) = self.descriptor.raw() else {
            return;
        };
        // SAFETY: unlock only this description's range, never from an inherited process.
        let mut range: libc::flock = unsafe { std::mem::zeroed() };
        range.l_type = libc::F_UNLCK as _;
        range.l_whence = libc::SEEK_SET as _;
        range.l_start = self.byte;
        range.l_len = 1;
        // SAFETY: the initialized flock unlocks only this live description's owned byte.
        unsafe { libc::fcntl(fd, libc::F_OFD_SETLK, &range) };
    }
}

pub(super) struct Mapping {
    base: NonNull<AtomicU64>,
    words: usize,
}
// SAFETY: the fixed mapping never moves; all accesses are aligned AtomicU64 operations.
unsafe impl Send for Mapping {}
// SAFETY: shared publication is separately serialized; atomics also cover forked readers.
unsafe impl Sync for Mapping {}

impl Mapping {
    pub(super) fn new(file: &Descriptor, words: usize) -> Result<Self> {
        // SAFETY: map the validated, publication-locked file's fixed-size storage shared.
        let address = unsafe {
            libc::mmap(
                ptr::null_mut(),
                words * 8,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.raw()?,
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self {
            base: NonNull::new(address.cast()).expect("non-null mmap"),
            words,
        })
    }
    pub(super) fn load(&self, word: usize, order: Ordering) -> u64 {
        assert!(word < self.words);
        // SAFETY: page-aligned mmap, fixed 64-bit layout, live mapping, all accesses atomic.
        unsafe { self.base.as_ptr().add(word).as_ref().unwrap().load(order) }
    }
    pub(super) fn store(&self, word: usize, value: u64, order: Ordering) {
        assert!(word < self.words);
        // SAFETY: same fixed live atomic mapping as load; caller owns publication ordering.
        unsafe {
            self.base
                .as_ptr()
                .add(word)
                .as_ref()
                .unwrap()
                .store(value, order)
        };
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the final owner unmaps only its own process's fixed mapping.
        unsafe { libc::munmap(self.base.as_ptr().cast(), self.words * 8) };
    }
}

pub(super) fn uid() -> u32 {
    // SAFETY: geteuid has no memory arguments.
    unsafe { libc::geteuid() }
}
