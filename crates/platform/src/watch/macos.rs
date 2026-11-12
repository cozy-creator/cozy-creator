//! macOS: kqueue vnode events on each directory (an entry added, removed or renamed) and on each
//! file in it whose writes count (written, extended, deleted or renamed), and a user event for a
//! wake. A directory event rearms, so a file or directory replaced is watched from then on.
#![allow(unsafe_code)]
use super::{Dir, Writes};
use crate::{Error, Result};
use std::{
    collections::HashMap,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
    ptr,
    sync::Arc,
    time::Duration,
};

const DIRECTORY: u32 = libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME;
const FILE: u32 = libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_ATTRIB;
/// The user event's ident: only it uses `EVFILT_USER`.
const WAKE: libc::uintptr_t = 0;

pub(super) struct Watch {
    queue: Arc<Wake>,
    dirs: Vec<Watched>,
}

/// The kqueue, which a wake triggers.
pub(super) struct Wake(OwnedFd);

struct Watched {
    dir: Dir,
    /// The directory's descriptor and (device, inode), while it is there.
    at: Option<(OwnedFd, (u64, u64))>,
    /// Each counted file's descriptor, by (device, inode): a renamed file keeps its watch.
    files: HashMap<(u64, u64), OwnedFd>,
}

impl Watch {
    pub(super) fn new(dirs: Vec<Dir>) -> Result<Self> {
        // SAFETY: kqueue takes nothing and returns a new descriptor or -1.
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(Error::os("kqueue")(io::Error::last_os_error()));
        }
        // SAFETY: a descriptor kqueue just returned and nothing else owns.
        let queue = Arc::new(Wake(unsafe { OwnedFd::from_raw_fd(raw) }));
        queue.change(WAKE, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0).map_err(Error::os("kevent"))?;
        let mut watch = Self {
            queue,
            dirs: dirs.into_iter().map(|dir| Watched { dir, at: None, files: HashMap::new() }).collect(),
        };
        watch.arm()?;
        if watch.dirs.first().is_some_and(|watched| watched.at.is_none()) {
            let path = &watch.dirs[0].dir.path;
            return Err(Error::io("watch", path)(open_events(path).err().unwrap_or_else(|| io::ErrorKind::NotFound.into())));
        }
        Ok(watch)
    }

    pub(super) fn wait(&mut self, most: Option<Duration>) -> Result<bool> {
        let Some(changed) = self.next(most)? else {
            return Ok(false);
        };
        let mut rearm = changed;
        // Drain what is already queued: one wake per wait.
        while let Some(more) = self.next(Some(Duration::ZERO))? {
            rearm |= more;
        }
        if rearm {
            self.arm()?;
        }
        Ok(true)
    }

    /// One batch of events within `period`: None when none came or it was woken, else whether a
    /// directory changed or a file left, which rearms.
    fn next(&mut self, period: Option<Duration>) -> Result<Option<bool>> {
        let timeout = period.map(|period| libc::timespec {
            tv_sec: period.as_secs() as libc::time_t,
            tv_nsec: period.subsec_nanos() as libc::c_long,
        });
        // SAFETY: plain-old-data the call fills.
        let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
        // SAFETY: a live queue, no changes, a writable event buffer of the length given.
        let n = unsafe {
            libc::kevent(
                self.queue.0.as_raw_fd(),
                ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as libc::c_int,
                timeout.as_ref().map_or(ptr::null(), |t| t as *const _),
            )
        };
        if n < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::Interrupted => Ok(None),
                _ => Err(Error::os("kevent")(error)),
            };
        }
        let mut vnodes = events[..n as usize].iter().filter(|event| event.filter == libc::EVFILT_VNODE).peekable();
        if vnodes.peek().is_none() {
            return Ok(None);
        }
        let directories: Vec<usize> =
            self.dirs.iter().filter_map(|watched| watched.at.as_ref().map(|(fd, _)| fd.as_raw_fd() as usize)).collect();
        Ok(Some(vnodes.any(|event| {
            let (ident, notes) = (event.ident, event.fflags);
            directories.contains(&ident) || notes & (libc::NOTE_DELETE | libc::NOTE_RENAME) != 0
        })))
    }

    /// Watches each directory there now and each counted file in it, and lets go of what left.
    fn arm(&mut self) -> Result<()> {
        for watched in &mut self.dirs {
            let path = &watched.dir.path;
            let identity = fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
            if identity != watched.at.as_ref().map(|(_, identity)| *identity) {
                // Closing a descriptor removes its kevent.
                watched.at = None;
                watched.files.clear();
                if let Some(identity) = identity
                    && let Ok(directory) = open_events(path)
                {
                    self.queue.change(directory.as_raw_fd() as _, libc::EVFILT_VNODE, libc::EV_ADD | libc::EV_CLEAR, DIRECTORY)
                        .map_err(Error::io("watch", path))?;
                    watched.at = Some((directory, identity));
                }
            }
            if watched.at.is_none() || matches!(watched.dir.writes, Writes::None) {
                continue;
            }
            let mut present = HashMap::new();
            let Ok(entries) = fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                if !watched.dir.writes.counts(entry.file_name().as_bytes()) {
                    continue;
                }
                let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                if metadata.is_file() {
                    present.insert((metadata.dev(), metadata.ino()), entry.path());
                }
            }
            watched.files.retain(|key, _| present.contains_key(key));
            for (key, path) in present {
                if watched.files.contains_key(&key) {
                    continue;
                }
                // A file gone between the listing and the open is the next rearm's business.
                let Ok(file) = open_events(&path) else {
                    continue;
                };
                self.queue.change(file.as_raw_fd() as _, libc::EVFILT_VNODE, libc::EV_ADD | libc::EV_CLEAR, FILE)
                    .map_err(Error::io("watch", &path))?;
                watched.files.insert(key, file);
            }
        }
        Ok(())
    }

    pub(super) fn wake(&self) -> Arc<Wake> {
        self.queue.clone()
    }
}

impl Wake {
    pub(super) fn wake(&self) {
        let _ = self.change(WAKE, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER);
    }

    fn change(&self, ident: libc::uintptr_t, filter: i16, flags: u16, notes: u32) -> io::Result<()> {
        let change = libc::kevent { ident, filter, flags, fflags: notes, data: 0, udata: ptr::null_mut() };
        // SAFETY: a live queue, one change to register and no events asked back.
        let added = unsafe { libc::kevent(self.0.as_raw_fd(), &change, 1, ptr::null_mut(), 0, ptr::null()) };
        match added {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

/// A descriptor only for events: it keeps no volume busy and reads nothing.
fn open_events(path: &Path) -> io::Result<OwnedFd> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: a NUL-terminated path; the descriptor is owned at once.
    let raw = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor open just returned and nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}
