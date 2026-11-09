//! Unix: Unix sockets.
use crate::{Error, Result, process::Exact};
use rustix::{
    event::{PollFd, PollFlags},
    io::Errno,
};
use std::{
    os::{
        fd::AsFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::Path,
};

pub type LocalStream = UnixStream;

/// A listener at a path an executor dials.
pub struct LocalListener(UnixListener);

impl LocalListener {
    pub fn bind(path: &Path) -> Result<Self> {
        UnixListener::bind(path)
            .map(Self)
            .map_err(Error::io("listen", path))
    }

    /// Every dialer, one at a time, as `UnixListener::incoming` gives them.
    pub(crate) fn incoming(&self) -> impl Iterator<Item = Result<LocalStream>> + '_ {
        std::iter::repeat_with(|| self.accept())
    }

    pub(crate) fn accept(&self) -> Result<LocalStream> {
        Ok(self.0.accept().map_err(Error::os("accept"))?.0)
    }

    /// A dialer, or `None` once `process` has ended first: woken by the kernel, never a timer.
    pub fn accept_unless(&self, process: &Exact) -> Result<Option<LocalStream>> {
        loop {
            let mut poll = [
                PollFd::new(&self.0, PollFlags::IN),
                PollFd::new(process, PollFlags::IN),
            ];
            match rustix::event::poll(&mut poll, None) {
                Err(Errno::INTR) => continue,
                Err(error) => return Err(Error::os("wait for a dialer")(error.into())),
                Ok(_) => {}
            }
            if poll[0].revents().contains(PollFlags::IN) {
                return self.accept().map(Some);
            }
            if !poll[1].revents().is_empty() {
                return Ok(None);
            }
        }
    }
}

impl AsFd for LocalListener {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.0.as_fd()
    }
}

/// Two connected ends.
pub fn pair() -> Result<(LocalStream, LocalStream)> {
    Ok(LocalStream::pair()?)
}
