//! A child that a stable parent starts with a private control stream and its listening sockets, and that never outlives that parent:
//! a Calcifer generation (cl-1113). The parent keeps the sockets open for good, so a connect made while one generation hands over to
//! the next waits in their backlog; none is refused.
//!
//! Unix: the stream and the sockets are inherited at their own numbers, which [`start`] appends to the arguments; the child arms
//! its parent-death signal in [`take`]. Windows: the stream's handle is inherited through an explicit handle list, the sockets follow
//! on it as `WSADuplicateSocketW` records written before the child runs, and every child is in the parent's one Job Object, which
//! ends them with it.
use std::{ffi::OsString, net::TcpListener, path::Path};

use crate::{Error, Result, ipc::LocalStream};

#[cfg(unix)]
#[path = "generation/unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "generation/windows.rs"]
mod sys;

pub use sys::Child;

/// What a child took from its parent.
pub struct Taken {
    pub control: LocalStream,
    pub listeners: Vec<TcpListener>,
}

/// Starts `program args…` and the words naming what it inherits: its end of a new private stream and `listeners`. Its output goes to
/// `log` (appended), its input is null, it starts in the home folder. Returns the child and the parent's end of the stream.
pub fn start(program: &Path, args: &[OsString], log: Option<&Path>, listeners: &[&TcpListener]) -> Result<(Child, LocalStream)> {
    // The child dials: on Windows its end is the client, whose peer is the parent that made the pipe.
    let (child, parent) = crate::ipc::pair()?;
    let dir = std::env::home_dir().filter(|home| home.is_dir()).unwrap_or_else(std::env::temp_dir);
    let started = sys::start(program, args, log, &dir, child, &parent, listeners).map_err(Error::os("start a generation"))?;
    Ok((started, parent))
}

/// Windows: a pipe's handle the parent duplicated into this process ([`Child::give`]), as the value it was told.
#[cfg(windows)]
pub fn handed(value: u64) -> Result<std::fs::File> {
    sys::handed(value).map_err(Error::os("take a handed pipe"))
}

/// In the child: what [`start`] handed it, from the words it appended. Fails when the parent is already gone.
pub fn take(words: &[OsString]) -> Result<Taken> {
    sys::take(words).map_err(Error::os("take what the parent handed"))
}
