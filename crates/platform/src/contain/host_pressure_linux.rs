//! The Linux trigger: PSI on the system or this cgroup, inotify on each limited cgroup v2's
//! `memory.events`, and an eventfd TensorD's own events write.
use super::{HostMode, WINDOW};
use rustix::{
    event::{EventfdFlags, PollFd, PollFlags, eventfd, poll},
    fs::inotify::{self, CreateFlags, WatchFlags},
    io::{Errno, read, write},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::{fd::OwnedFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

/// What wakes the watcher: a PSI trigger, if this process may arm one, inotify on each limited
/// cgroup v2's `memory.events` it may watch, and its [`Nudge`].
pub struct Pressure {
    psi: Option<File>,
    events: Option<OwnedFd>,
    nudge: Nudge,
}

/// TensorD's own events wake the watcher for a look: the only wakeups where the kernel allows
/// none, and ones between the kernel's (cgroup v1 is read on every look, as v2 is).
#[derive(Clone)]
pub struct Nudge(Arc<OwnedFd>);

impl Nudge {
    pub fn send(&self) {
        // A full counter only coalesces.
        let _ = write(&*self.0, &1u64.to_ne_bytes());
    }
}

impl Pressure {
    /// Armed on whatever this process may watch, each limit on its own. An error only when not
    /// even an eventfd exists.
    pub fn arm(mode: HostMode) -> crate::Result<Self> {
        let nudge = Nudge(Arc::new(eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC).map_err(io::Error::from)?));
        let cgroups = super::super::host_memory::cgroups();
        let (path, kind) = match (mode, own_cgroup().filter(|c| c.join("memory.pressure").exists())) {
            (HostMode::Dedicated, Some(cgroup)) => (cgroup.join("memory.pressure"), "full"),
            // System `full` can stay quiet while one container stalls on a busy host.
            _ => (PathBuf::from("/proc/pressure/memory"), "some"),
        };
        let limited: Vec<PathBuf> = match cgroups {
            Some((groups, limits, "memory.current")) => groups
                .into_iter()
                .filter(|g| limits.iter().any(|l| super::super::host_memory::number(&g.join(l)).is_some()))
                .map(|g| g.join("memory.events"))
                .collect(),
            _ => vec![],
        };
        let psi = trigger(&path, kind).ok();
        let events = watch(&limited);
        Ok(Self { psi, events, nudge })
    }

    pub fn nudge(&self) -> Nudge {
        self.nudge.clone()
    }

    /// Blocks until the kernel reports a stall, a watched `memory.events` changes, or a nudge
    /// comes: no timeout, no work in between.
    pub fn wait(&mut self) -> crate::Result<()> {
        let mut fds = vec![];
        if let Some(psi) = &self.psi {
            fds.push(PollFd::new(psi, PollFlags::PRI));
        }
        if let Some(events) = &self.events {
            fds.push(PollFd::new(events, PollFlags::IN));
        }
        fds.push(PollFd::new(&*self.nudge.0, PollFlags::IN));
        loop {
            match poll(&mut fds, None) {
                Ok(0) | Err(Errno::INTR) => continue,
                Err(error) => return Err(io::Error::from(error).into()),
                Ok(_) => break,
            }
        }
        if self.psi.is_some() && fds[0].revents().contains(PollFlags::ERR) {
            return Err(io::Error::other("the PSI trigger is gone").into());
        }
        drop(fds);
        let mut buf = [0u8; 4096];
        for drained in [self.events.as_ref(), Some(&*self.nudge.0)].into_iter().flatten() {
            // Nonblocking, until empty.
            while read(drained, &mut buf).is_ok_and(|n| n > 0) {}
        }
        Ok(())
    }
}

/// A PSI trigger on one pressure file (`memory.pressure` of a cgroup, or the system's).
fn trigger(path: &Path, kind: &str) -> io::Result<File> {
    let mut trigger = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path)?;
    trigger.write_all(format!("{kind} 1 {}\0", WINDOW.as_micros()).as_bytes())?;
    Ok(trigger)
}

/// One inotify descriptor watching each file it can for IN_MODIFY; None when it watches none.
fn watch(files: &[PathBuf]) -> Option<OwnedFd> {
    if files.is_empty() {
        return None;
    }
    let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC).ok()?;
    let mut watched = false;
    for file in files {
        watched |= inotify::add_watch(&fd, file, WatchFlags::MODIFY).is_ok();
    }
    watched.then_some(fd)
}

/// This process's cgroup v2 directory.
fn own_cgroup() -> Option<PathBuf> {
    let text = fs::read_to_string("/proc/self/cgroup").ok()?;
    let relative = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/')))
}
