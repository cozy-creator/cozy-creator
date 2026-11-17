//! [`super`] on Unix.
#![allow(unsafe_code)]
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io,
    net::TcpListener,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, RawFd},
        unix::net::UnixStream,
    },
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

use super::Taken;
use crate::ipc::LocalStream;

/// A started generation: only its own process is signaled, never its group, whose mount supervisors may serve the next one.
#[derive(Debug)]
pub struct Child(std::process::Child);

impl Child {
    pub fn pid(&self) -> u32 {
        self.0.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.0.wait()
    }
    /// SIGKILL to this process alone.
    pub fn kill(&mut self) -> io::Result<()> {
        match rustix::process::kill_process(rustix::process::Pid::from_raw(self.0.id() as i32).expect("a child's pid"), rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

pub(super) fn start(
    program: &Path,
    args: &[OsString],
    log: Option<&Path>,
    dir: &Path,
    child: LocalStream,
    _parent: &LocalStream,
    listeners: &[&TcpListener],
) -> io::Result<Child> {
    let fds: Vec<RawFd> = std::iter::once(child.as_raw_fd()).chain(listeners.iter().map(|listener| listener.as_raw_fd())).collect();
    let mut command = Command::new(program);
    command.args(args).args(fds.iter().map(|fd| fd.to_string())).current_dir(dir).stdin(Stdio::null());
    match log {
        Some(path) => {
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            command.stdout(file.try_clone()?).stderr(file);
        }
        None => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    crate::process::inherit_nothing(&mut command);
    crate::process::keep_open(&mut command, fds);
    // A terminal's ^C reaches the parent, which tells its generation; never both.
    crate::process::own_group(&mut command);
    let spawned = command.spawn()?;
    drop(child);
    Ok(Child(spawned))
}

pub(super) fn take(words: &[OsString]) -> io::Result<Taken> {
    let parent = std::os::unix::process::parent_id();
    if !crate::process::die_with_parent(parent).map_err(io::Error::other)? {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the parent ended before this generation started"));
    }
    let fds = words
        .iter()
        .map(|word| word.to_str().and_then(|word| word.parse::<RawFd>().ok()).filter(|fd| *fd > 2))
        .collect::<Option<Vec<_>>>()
        .filter(|fds| !fds.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a generation is given its descriptors' numbers"))?;
    for &fd in &fds {
        // SAFETY: only inspected; an fd number that is not open fails here.
        let stat = rustix::fs::fstat(unsafe { BorrowedFd::borrow_raw(fd) })?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Socket {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("descriptor {fd} is not a socket")));
        }
    }
    // SAFETY: open sockets this process inherited for exactly this, owned from here on.
    let control = unsafe { UnixStream::from_raw_fd(fds[0]) };
    let listeners = fds[1..].iter().map(|&fd| unsafe { TcpListener::from_raw_fd(fd) }).collect();
    Ok(Taken { control, listeners })
}
