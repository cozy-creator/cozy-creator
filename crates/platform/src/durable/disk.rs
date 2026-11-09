//! What a crash can take from a Store, read off the filesystem the Store actually lives on, and
//! what the page cache holds of a file.
//!
//! An fsync protects bytes against the machine going down before its page cache is written.
//! Whether that protection is worth paying for depends on where the bytes live, and only the
//! mount can say that — never a flag, never an environment variable.
//!
//! The rule, from `statfs(2)` of the Store root (Linux):
//!
//! - **Ephemeral** — tmpfs, ramfs, overlayfs. RAM, or a container's writable layer, which the
//!   platform discards with the machine or container (a RunPod container disk is erased when
//!   the pod stops). Nothing a crash could leave behind is ever read again, and a process
//!   crash leaves the page cache intact, so no fsync buys anything.
//! - **Network** — NFS, SMB/CIFS, FUSE, Ceph, 9p, AFS, Coda, GFS2, OCFS2, Lustre, GPFS.
//!   Another machine may read it, and close-to-open is the only ordering that machine sees:
//!   bytes are flushed before their name is published, as before.
//! - **Persistent** — everything else (ext4, xfs, btrfs, zfs, ...), a `statfs` that fails,
//!   and every non-Linux platform. The safe side: it survives a crash of this machine.
#![allow(unsafe_code)]
use std::{fs::File, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiskClass {
    Ephemeral,
    Persistent,
    Network,
}

impl DiskClass {
    /// The class of the filesystem holding `path`.
    pub fn of(path: &Path) -> DiskClass {
        filesystem(path).class
    }

}

/// The filesystem holding a path: its class and the name it was recognised by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filesystem {
    pub class: DiskClass,
    /// `ext4`, `overlayfs`, `nfs`, ...; `other` for an unrecognised magic, `unknown` when
    /// `statfs` failed or the platform has no magic.
    pub kind: &'static str,
}

#[cfg(target_os = "linux")]
pub fn filesystem(path: &Path) -> Filesystem {
    #[allow(clippy::unnecessary_cast)] // f_type's width differs by target
    let magic = rustix::fs::statfs(path).ok().map(|stat| stat.f_type as u64);
    match magic {
        Some(magic) => classify(magic & 0xffff_ffff),
        None => Filesystem {
            class: DiskClass::Persistent,
            kind: "unknown",
        },
    }
}

#[cfg(not(target_os = "linux"))]
pub fn filesystem(_path: &Path) -> Filesystem {
    Filesystem {
        class: DiskClass::Persistent,
        kind: "unknown",
    }
}

/// Superblock magics from `linux/magic.h` and the filesystems' own headers.
#[cfg(target_os = "linux")]
fn classify(magic: u64) -> Filesystem {
    use DiskClass::*;
    let (class, kind) = match magic {
        0x0102_1994 => (Ephemeral, "tmpfs"),
        0x8584_58f6 => (Ephemeral, "ramfs"),
        0x794c_7630 => (Ephemeral, "overlayfs"),
        0x6969 => (Network, "nfs"),
        0x517b => (Network, "smb"),
        0xff53_4d42 => (Network, "cifs"),
        0xfe53_4d42 => (Network, "smb2"),
        0x6573_5546 => (Network, "fuse"),
        0x00c3_6400 => (Network, "ceph"),
        0x0102_1997 => (Network, "9p"),
        0x5346_414f | 0x6b41_4653 => (Network, "afs"),
        0x7375_7245 => (Network, "coda"),
        0x0116_1970 => (Network, "gfs2"),
        0x7461_636f => (Network, "ocfs2"),
        0x0bd0_0bd0 => (Network, "lustre"),
        0x4750_4653 => (Network, "gpfs"),
        0xef53 => (Persistent, "ext4"),
        0x5846_5342 => (Persistent, "xfs"),
        0x9123_683e => (Persistent, "btrfs"),
        0x2fc1_2fc1 => (Persistent, "zfs"),
        0xf2f5_2010 => (Persistent, "f2fs"),
        _ => (Persistent, "other"),
    };
    Filesystem { class, kind }
}

/// Whether a rename over a file in `dir` reaches the disk only after the new file's bytes do, with
/// no flush asked: ext4 in its default ordered mode, whose `auto_da_alloc` writes a file renamed
/// over another before the rename commits, and anything a crash erases. Elsewhere a writer flushes
/// first (`ordered`).
#[cfg(target_os = "linux")]
pub fn renames_after_data(dir: &Path) -> bool {
    use std::{collections::HashMap, os::unix::fs::MetadataExt, sync::Mutex};
    static SEEN: Mutex<Option<HashMap<u64, bool>>> = Mutex::new(None);
    let fs = filesystem(dir);
    if fs.class == DiskClass::Ephemeral {
        return true;
    }
    let Some(dev) = std::fs::metadata(dir).ok().map(|meta| meta.dev()).filter(|_| fs.kind == "ext4") else {
        return false;
    };
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *seen.get_or_insert_with(HashMap::new).entry(dev).or_insert_with(|| ordered_ext4(dev))
}

#[cfg(not(target_os = "linux"))]
pub fn renames_after_data(_dir: &Path) -> bool {
    false
}

/// Whether device `dev`'s ext4 orders data before a rename: neither `noauto_da_alloc` nor
/// `data=writeback` among its mount's options (`/proc/self/mountinfo`).
#[cfg(target_os = "linux")]
fn ordered_ext4(dev: u64) -> bool {
    let (major, minor) = (((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff), (dev & 0xff) | ((dev >> 12) & !0xff));
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") else { return false };
    let named = format!("{major}:{minor}");
    mounts.lines().filter(|line| line.split(' ').nth(2) == Some(named.as_str())).any(|line| {
        let options = line.split(" - ").nth(1).and_then(|rest| rest.split(' ').nth(2)).unwrap_or_default();
        !options.split(',').any(|option| option == "noauto_da_alloc" || option == "data=writeback")
    })
}

/// A second description of `file`'s inode that bypasses the page cache, or `None` where the
/// filesystem refuses it. Linux reopens through `/proc/self/fd` with `O_DIRECT`; elsewhere no
/// direct read is offered and callers read buffered.
pub fn open_direct(file: &File) -> crate::Result<Option<File>> {
    #[cfg(target_os = "linux")]
    {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let path = format!("/proc/self/fd/{}", file.as_raw_fd());
        Ok(File::options().read(true).custom_flags(libc::O_DIRECT).open(path).ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        Ok(None)
    }
}

/// Whether the page cache holds every page of `[off, off+len)`; `None` where the OS cannot say,
/// which callers treat as cold.
pub fn cached(file: &File, off: u64, len: u64) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        linux::cached(file, off, len)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (file, off, len);
        None
    }
}

/// The OS's page size.
pub fn page_size() -> u64 {
    #[cfg(target_os = "linux")]
    {
        linux::page_size()
    }
    #[cfg(not(target_os = "linux"))]
    {
        4096
    }
}

/// The pages of `file` the page cache holds, as `(first page, count)` runs of [`page_size`]
/// bytes, by `mincore(2)` over a mapping (it never faults one in); empty where the OS cannot say.
pub fn cached_runs(file: &File) -> Vec<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        linux::cached_runs(file)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        vec![]
    }
}

/// Asks the kernel to read `[off, off+len)` of `file` into the page cache in the background
/// (`POSIX_FADV_WILLNEED`): reclaimable cache, nothing read into this process. Nothing where the
/// OS has no such advice.
pub fn will_need(file: &File, off: u64, len: u64) {
    #[cfg(target_os = "linux")]
    {
        // Advice: it reads nothing into this process.
        let _ = rustix::fs::fadvise(file, off, std::num::NonZeroU64::new(len), rustix::fs::Advice::WillNeed);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (file, off, len);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::os::fd::AsRawFd;

    /// From `cachestat(2)`: a pure query (unlike an `RWF_NOWAIT` read, which starts readahead on
    /// a miss). Where that cannot answer, the pages are counted through a mapping: a kernel
    /// without it (< 6.5), a container whose seccomp filter refuses it, and overlayfs (a
    /// container's root), whose files report no pages of their own.
    pub(super) fn cached(file: &File, off: u64, len: u64) -> Option<bool> {
        #[repr(C)]
        struct Range {
            off: u64,
            len: u64,
        }
        #[repr(C)]
        #[derive(Default)]
        struct Stat {
            nr_cache: u64,
            nr_dirty: u64,
            nr_writeback: u64,
            nr_evicted: u64,
            nr_recently_evicted: u64,
        }
        const SYS_CACHESTAT: libc::c_long = 451;
        let page = page_size();
        let range = Range { off, len };
        let mut stat = Stat::default();
        // SAFETY: the documented cachestat ABI; both structs outlive the call.
        let rc = unsafe { libc::syscall(SYS_CACHESTAT, file.as_raw_fd(), &range, &mut stat, 0) };
        if rc == 0 && stat.nr_cache >= (off + len).div_ceil(page) - off / page {
            return Some(true);
        }
        Some((rc != 0 || overlay(file)) && resident(file, off, len, page))
    }

    pub(super) fn page_size() -> u64 {
        (rustix::param::page_size() as u64).max(4096)
    }

    pub(super) fn cached_runs(file: &File) -> Vec<(u64, u64)> {
        let Ok(len) = file.metadata().map(|m| m.len() as usize) else { return vec![] };
        if len == 0 {
            return vec![];
        }
        // SAFETY: a read-only shared mapping of our descriptor, unmapped below.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        if p == libc::MAP_FAILED {
            return vec![];
        }
        let mut pages = vec![0u8; len.div_ceil(page_size() as usize)];
        // SAFETY: the vector holds one byte per page of the mapping.
        let ok = unsafe { libc::mincore(p, len, pages.as_mut_ptr()) } == 0;
        // SAFETY: unmaps the mapping made above, which nothing else references.
        unsafe { libc::munmap(p, len) };
        if !ok {
            return vec![];
        }
        let (mut runs, mut start) = (vec![], None);
        for (i, byte) in pages.iter().chain([&0]).enumerate() {
            match (byte & 1 == 1, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    runs.push((s as u64, (i - s) as u64));
                    start = None;
                }
                _ => (),
            }
        }
        runs
    }

    fn overlay(file: &File) -> bool {
        const OVERLAYFS_SUPER_MAGIC: i64 = 0x794c7630;
        #[allow(clippy::unnecessary_cast)] // f_type is not an i64 on every target
        rustix::fs::fstatfs(file).is_ok_and(|fs| fs.f_type as i64 == OVERLAYFS_SUPER_MAGIC)
    }

    /// Every page of `[off, off+len)` is in core, by `mincore(2)` over a mapping (never faults).
    fn resident(file: &File, off: u64, len: u64, page: u64) -> bool {
        let start = off - off % page;
        let n = (off + len - start) as usize;
        if n == 0 {
            return true;
        }
        // SAFETY: a read-only shared mapping of our descriptor, unmapped below.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                n,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                start as libc::off_t,
            )
        };
        if p == libc::MAP_FAILED {
            return false;
        }
        let mut pages = vec![0u8; n.div_ceil(page as usize)];
        // SAFETY: the vector holds one byte per page of the mapping.
        let ok = unsafe { libc::mincore(p, n, pages.as_mut_ptr()) } == 0
            && pages.iter().all(|b| b & 1 == 1);
        // SAFETY: unmaps the mapping made above, which nothing else references.
        unsafe { libc::munmap(p, n) };
        ok
    }
}
