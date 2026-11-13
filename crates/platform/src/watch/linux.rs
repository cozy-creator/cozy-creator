//! Linux: inotify on each directory, and an eventfd for a wake.
use super::Dir;
use crate::{Error, Result};
use rustix::{
    event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll},
    fs::inotify::{self, CreateFlags, ReadFlags, Reader, WatchFlags},
    io::{Errno, read, write},
};
use std::{mem::MaybeUninit, os::fd::OwnedFd, sync::Arc, time::Duration};

const ENTRIES: WatchFlags = WatchFlags::CREATE
    .union(WatchFlags::DELETE)
    .union(WatchFlags::MOVED_FROM)
    .union(WatchFlags::MOVED_TO)
    .union(WatchFlags::DELETE_SELF)
    .union(WatchFlags::MOVE_SELF);

pub(super) struct Watch {
    fd: OwnedFd,
    wake: Arc<Wake>,
    /// Each directory and its watch, while it has one.
    dirs: Vec<(Dir, Option<i32>)>,
}

pub(super) struct Wake(OwnedFd);

impl Watch {
    pub(super) fn new(dirs: Vec<Dir>) -> Result<Self> {
        let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)
            .map_err(|e| Error::os("inotify_init1")(e.into()))?;
        let wake = eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC)
            .map_err(|e| Error::os("eventfd")(e.into()))?;
        let mut watch = Self { fd, wake: Arc::new(Wake(wake)), dirs: dirs.into_iter().map(|dir| (dir, None)).collect() };
        if let Err(error) = watch.arm() {
            return Err(Error::io("watch", &watch.dirs[0].0.path)(error.into()));
        }
        Ok(watch)
    }

    /// Watches each directory there now, so one replaced since keeps no stale watch; the first's failure is said.
    fn arm(&mut self) -> std::result::Result<(), Errno> {
        let mut first = Ok(());
        for (at, (dir, wd)) in self.dirs.iter_mut().enumerate() {
            let mask = match dir.writes {
                super::Writes::None => ENTRIES,
                _ => ENTRIES | WatchFlags::MODIFY,
            };
            let added = inotify::add_watch(&self.fd, &dir.path, mask);
            if at == 0 {
                first = added.map(drop);
            }
            *wd = added.ok();
        }
        first
    }

    pub(super) fn wait(&mut self, most: Option<Duration>) -> Result<bool> {
        let timeout = most.map(|most| Timespec::try_from(most).unwrap_or(Timespec { tv_sec: i32::MAX as _, tv_nsec: 0 }));
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN), PollFd::new(&self.wake.0, PollFlags::IN)];
        match poll(&mut fds, timeout.as_ref()) {
            Ok(0) | Err(Errno::INTR) => return Ok(false),
            Err(e) => return Err(Error::os("poll")(e.into())),
            Ok(_) => {}
        }
        if fds[1].revents().contains(PollFlags::IN) {
            let _ = read(&self.wake.0, &mut [0u8; 8]);
            return Ok(false);
        }
        let (mut changed, mut entries) = (false, false);
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut events = Reader::new(&self.fd, &mut buffer);
        while let Ok(event) = events.next() {
            let flags = event.events();
            if flags == ReadFlags::MODIFY {
                let dir = self.dirs.iter().find(|(_, wd)| *wd == Some(event.wd()));
                changed |= dir.is_some_and(|(dir, _)| dir.writes.counts(event.file_name().map_or(&[][..], |name| name.to_bytes())));
            } else {
                changed = true;
                entries = true;
            }
        }
        if entries {
            let _ = self.arm();
        }
        Ok(changed)
    }

    pub(super) fn wake(&self) -> Arc<Wake> {
        self.wake.clone()
    }
}

impl Wake {
    pub(super) fn wake(&self) {
        let _ = write(&self.0, &1u64.to_ne_bytes());
    }
}
