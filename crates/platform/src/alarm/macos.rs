//! macOS: a kqueue timer at an absolute wall time counted on continuous time, which goes on
//! through sleep, and a user event for a wake.
#![allow(unsafe_code)]
use crate::{Error, Result};
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    ptr,
    time::SystemTime,
};

const WAKE: libc::uintptr_t = 0;
const TIMER: libc::uintptr_t = 1;

pub(super) struct Alarm(OwnedFd);

impl Alarm {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: kqueue takes nothing and returns a new descriptor or -1.
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(Error::os("kqueue")(io::Error::last_os_error()));
        }
        // SAFETY: a descriptor kqueue just returned and nothing else owns.
        let alarm = Self(unsafe { OwnedFd::from_raw_fd(raw) });
        alarm.change(WAKE, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0, 0).map_err(Error::os("kevent"))?;
        Ok(alarm)
    }

    pub(super) fn wait(&self, at: Option<SystemTime>) -> Result<bool> {
        match at {
            Some(at) => {
                let notes = libc::NOTE_ABSOLUTE | libc::NOTE_USECONDS | libc::NOTE_MACH_CONTINUOUS_TIME;
                let usecs = super::since_epoch(at).as_micros().min(isize::MAX as u128) as isize;
                self.change(TIMER, libc::EVFILT_TIMER, libc::EV_ADD | libc::EV_ONESHOT, notes, usecs).map_err(Error::os("kevent"))?;
            }
            None => {
                let _ = self.change(TIMER, libc::EVFILT_TIMER, libc::EV_DELETE, 0, 0);
            }
        }
        // SAFETY: plain-old-data the call fills.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: a live queue, no changes, one writable event, no timeout.
        let n = unsafe { libc::kevent(self.0.as_raw_fd(), ptr::null(), 0, &mut event, 1, ptr::null()) };
        if n < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::Interrupted => Ok(false),
                _ => Err(Error::os("kevent")(error)),
            };
        }
        let filter = event.filter;
        Ok(n > 0 && filter == libc::EVFILT_TIMER)
    }

    pub(super) fn wake(&self) {
        let _ = self.change(WAKE, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER, 0);
    }

    fn change(&self, ident: libc::uintptr_t, filter: i16, flags: u16, notes: u32, data: isize) -> io::Result<()> {
        let change = libc::kevent { ident, filter, flags, fflags: notes, data, udata: ptr::null_mut() };
        // SAFETY: a live queue, one change to register and no events asked back.
        match unsafe { libc::kevent(self.0.as_raw_fd(), &change, 1, ptr::null_mut(), 0, ptr::null()) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}
