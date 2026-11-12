//! macOS: kqueue's filesystem filter, which fires as a volume mounts or unmounts.
#![allow(unsafe_code)]
use crate::{Error, Result};
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    ptr,
};

pub(super) struct Changes(OwnedFd);

impl Changes {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: kqueue takes nothing and returns a new descriptor or -1.
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(Error::os("kqueue")(io::Error::last_os_error()));
        }
        // SAFETY: a descriptor kqueue just returned and nothing else owns.
        let queue = unsafe { OwnedFd::from_raw_fd(raw) };
        let change = libc::kevent {
            ident: 0,
            filter: libc::EVFILT_FS,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: 0,
            data: 0,
            udata: ptr::null_mut(),
        };
        // SAFETY: a live queue, one change to register and no events asked back.
        if unsafe { libc::kevent(queue.as_raw_fd(), &change, 1, ptr::null_mut(), 0, ptr::null()) } != 0 {
            return Err(Error::os("kevent")(io::Error::last_os_error()));
        }
        Ok(Self(queue))
    }

    pub(super) fn wait(&self) -> Result<()> {
        loop {
            // SAFETY: plain-old-data the call fills.
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            // SAFETY: a live queue, no changes, one writable event, no timeout.
            let n = unsafe { libc::kevent(self.0.as_raw_fd(), ptr::null(), 0, &mut event, 1, ptr::null()) };
            if n > 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if n < 0 && error.kind() != io::ErrorKind::Interrupted {
                return Err(Error::os("kevent")(error));
            }
        }
    }
}
