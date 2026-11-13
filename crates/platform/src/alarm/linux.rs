//! Linux: an absolute CLOCK_REALTIME timerfd, which fires after a resume and ends on a clock
//! change (CANCEL_ON_SET), and an eventfd for a wake.
use crate::{Error, Result};
use rustix::{
    event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll},
    io::{Errno, read, write},
    time::{Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, timerfd_create, timerfd_settime},
};
use std::{os::fd::OwnedFd, time::SystemTime};

pub(super) struct Alarm {
    timer: OwnedFd,
    wake: OwnedFd,
}

impl Alarm {
    pub(super) fn new() -> Result<Self> {
        let timer = timerfd_create(TimerfdClockId::Realtime, TimerfdFlags::NONBLOCK | TimerfdFlags::CLOEXEC)
            .map_err(|e| Error::os("timerfd_create")(e.into()))?;
        let wake = eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC).map_err(|e| Error::os("eventfd")(e.into()))?;
        Ok(Self { timer, wake })
    }

    pub(super) fn wait(&self, at: Option<SystemTime>) -> Result<bool> {
        let value = at.map_or(Timespec { tv_sec: 0, tv_nsec: 0 }, |at| {
            let since = super::since_epoch(at);
            Timespec { tv_sec: since.as_secs() as _, tv_nsec: since.subsec_nanos() as _ }
        });
        let spec = Itimerspec { it_interval: Timespec { tv_sec: 0, tv_nsec: 0 }, it_value: value };
        timerfd_settime(&self.timer, TimerfdTimerFlags::ABSTIME | TimerfdTimerFlags::CANCEL_ON_SET, &spec)
            .map_err(|e| Error::os("timerfd_settime")(e.into()))?;
        let mut fds = [PollFd::new(&self.timer, PollFlags::IN), PollFd::new(&self.wake, PollFlags::IN)];
        match poll(&mut fds, None) {
            Err(Errno::INTR) => return Ok(false),
            Err(e) => return Err(Error::os("poll")(e.into())),
            Ok(_) => {}
        }
        if fds[1].revents().contains(PollFlags::IN) {
            let _ = read(&self.wake, &mut [0u8; 8]);
            return Ok(false);
        }
        // A clock change fails the read (ECANCELED): the deadline is read again as if it rang.
        let _ = read(&self.timer, &mut [0u8; 8]);
        Ok(true)
    }

    pub(super) fn wake(&self) {
        let _ = write(&self.wake, &1u64.to_ne_bytes());
    }
}
