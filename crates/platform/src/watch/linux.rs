//! Linux: inotify on the directory.
use crate::Result;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::inotify::{self, CreateFlags, WatchFlags},
    io::{Errno, read},
};
use std::{os::fd::OwnedFd, path::Path, time::Duration};

pub(super) struct Watch {
    fd: OwnedFd,
}

impl Watch {
    pub(super) fn new(dir: &Path) -> Result<Self> {
        let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)
            .map_err(|e| crate::Error::os("inotify_init1")(e.into()))?;
        let mask = WatchFlags::MODIFY | WatchFlags::CREATE | WatchFlags::MOVED_TO | WatchFlags::DELETE;
        inotify::add_watch(&fd, dir, mask).map_err(|e| crate::Error::io("watch", dir)(e.into()))?;
        Ok(Self { fd })
    }

    pub(super) fn wait(&mut self, most: Duration) -> Result<bool> {
        let timeout = Timespec::try_from(most).unwrap_or(Timespec { tv_sec: i32::MAX as _, tv_nsec: 0 });
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(Errno::INTR) => return Ok(false),
            Err(e) => return Err(crate::Error::os("poll")(e.into())),
            Ok(_) => {}
        }
        // What changed does not matter, only that something did: drain the queue.
        let mut events = [0u8; 4096];
        while read(&self.fd, &mut events).is_ok_and(|n| n > 0) {}
        Ok(true)
    }
}
