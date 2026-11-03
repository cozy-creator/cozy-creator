//! A local event one process sets and its own threads wait on, alongside other handles.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
use std::{fmt, io};

/// The OS operation and its original error, with errno preserved through conversions.
#[derive(Debug)]
pub enum Error {
    Os { op: &'static str, source: io::Error },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn os(op: &'static str) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Os { op, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Os { op, source } = self;
        write!(f, "{op}: {source}")
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let Self::Os { source, .. } = self;
        Some(source)
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::Os { op: "system call", source }
    }
}

impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        let Error::Os { source, .. } = error;
        source
    }
}

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// One item [`wait_any`] found ready: its index, and whether it is broken (hung up or failed with
/// nothing left to read: it stays ready, so the caller drops it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ready {
    pub index: usize,
    pub broken: bool,
}
