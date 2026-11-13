//! Linux: the mount table's file polls POLLPRI once for each change since its last poll.
use crate::{Error, Result};
use rustix::{
    event::{PollFd, PollFlags, poll},
    io::Errno,
};
use std::fs::File;

pub(super) struct Changes(File);

impl Changes {
    pub(super) fn new() -> Result<Self> {
        File::open("/proc/self/mountinfo").map(Self).map_err(Error::io("open", "/proc/self/mountinfo"))
    }

    pub(super) fn wait(&self) -> Result<()> {
        let mut fds = [PollFd::new(&self.0, PollFlags::PRI)];
        loop {
            match poll(&mut fds, None) {
                Err(Errno::INTR) => continue,
                Err(e) => return Err(Error::os("poll")(e.into())),
                Ok(_) => return Ok(()),
            }
        }
    }
}
