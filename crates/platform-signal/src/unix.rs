use crate::{Error, Result};
use rustix::{
    event::{PollFd, PollFlags},
    io::Errno,
};

/// What [`wait_any`] waits on: a descriptor that turns readable.
pub type Waitable<'a> = std::os::fd::BorrowedFd<'a>;

/// A local wake one process sets and its own threads wait on: a socket pair whose read end is
/// readable from a set until it is cleared.
pub struct Signal {
    read: std::os::unix::net::UnixStream,
    write: std::os::unix::net::UnixStream,
}

impl Signal {
    pub fn new() -> Result<Signal> {
        let (read, write) = std::os::unix::net::UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        Ok(Signal { read, write })
    }

    /// A full pair is set already: the byte that did not fit is not needed.
    pub fn set(&self) {
        let _ = std::io::Write::write(&mut &self.write, &[1]);
    }

    /// Clears it; true when it was set.
    pub fn clear(&self) -> bool {
        let mut buf = [0u8; 64];
        let mut set = false;
        while let Ok(1..) = std::io::Read::read(&mut &self.read, &mut buf) {
            set = true;
        }
        set
    }

    pub fn waitable(&self) -> Waitable<'_> {
        std::os::fd::AsFd::as_fd(&self.read)
    }
}

/// Blocks until any of `items` is readable, hung up or failed, then answers every one that is, in
/// order, so none starves behind a lower one. No timeout.
pub fn wait_any(items: &[Waitable<'_>]) -> Result<Vec<super::Ready>> {
    let mut fds: Vec<PollFd<'_>> = items.iter().map(|fd| PollFd::new(fd, PollFlags::IN)).collect();
    loop {
        match rustix::event::poll(&mut fds, None) {
            Err(Errno::INTR) => continue,
            Err(error) => return Err(Error::os("wait")(error.into())),
            Ok(_) => {}
        }
        let broken = PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL;
        let ready: Vec<super::Ready> = fds
            .iter()
            .enumerate()
            .filter(|(_, fd)| !fd.revents().is_empty())
            .map(|(index, fd)| super::Ready { index, broken: fd.revents().intersects(broken) })
            .collect();
        if !ready.is_empty() {
            return Ok(ready);
        }
    }
}
