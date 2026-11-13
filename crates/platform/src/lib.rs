//! Every OS-specific seam of the `cozy` binary (design §6.2). Callers never write `cfg(target_os)`:
//! an OS without an implementation answers [`Error::Unsupported`], never a panic.
//!
//! Moved code keeps `io::Result` inside; the two conversions below let a moved body's `?` and a
//! caller's `?` work across the boundary, an `io::Error` keeping its errno both ways.
//!
//! The workspace's FFI lives here: only its boundary files allow `unsafe`.
//!
//! Every seam is behind the default `seams` feature but [`signal`], which varena links alone.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(unreachable_pub)]
use std::{fmt, io, path::PathBuf};

#[cfg(feature = "seams")]
pub mod alarm;
#[cfg(feature = "seams")]
pub mod contain;
#[cfg(feature = "seams")]
pub mod durable;
#[cfg(feature = "seams")]
pub mod gpu;
#[cfg(feature = "seams")]
pub mod handles;
#[cfg(feature = "seams")]
pub mod heap;
#[cfg(feature = "seams")]
pub mod ipc;
#[cfg(feature = "seams")]
pub mod lock;
#[cfg(all(unix, feature = "seams"))]
pub mod net;
#[cfg(feature = "seams")]
pub mod mounts;
#[cfg(feature = "seams")]
pub mod paths;
#[cfg(feature = "seams")]
pub mod process;
#[cfg(feature = "seams")]
pub mod private_exec;
#[cfg(feature = "fetch")]
pub mod release;
#[cfg(feature = "seams")]
pub mod service;
pub mod signal;
#[cfg(feature = "seams")]
pub mod tail;
#[cfg(feature = "seams")]
pub mod tools;
#[cfg(feature = "seams")]
pub mod watch;

/// A seam's failure: unsupported on this OS, an I/O or OS call, a service manager, or NVML.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// This seam has no implementation on this OS yet.
    #[error("{seam} is not supported on {os}")]
    Unsupported { seam: Seam, os: Os },
    #[error("{op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{op}: {source}")]
    Os {
        op: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{manager}: {detail}")]
    Service {
        manager: &'static str,
        detail: String,
    },
    #[error("NVML {op}: {detail}")]
    Gpu { op: &'static str, detail: String },
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    #[cfg(all(feature = "seams", not(target_os = "linux")))]
    pub(crate) fn unsupported(seam: Seam) -> Self {
        Self::Unsupported {
            seam,
            os: Os::CURRENT,
        }
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }

    #[cfg(feature = "seams")]
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { op, path, source }
    }

    pub(crate) fn os(op: &'static str) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Os { op, source }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::Os {
            op: "system call",
            source,
        }
    }
}

impl From<Error> for io::Error {
    /// An `Io`/`Os` error gives back the `io::Error` it carried, errno intact; `Unsupported` is
    /// `ErrorKind::Unsupported` carrying the typed error.
    fn from(error: Error) -> Self {
        match error {
            Error::Io { source, .. } | Error::Os { source, .. } => source,
            Error::Unsupported { .. } => io::Error::new(io::ErrorKind::Unsupported, error),
            other => io::Error::other(other),
        }
    }
}

/// The rows of design §6.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seam {
    Process,
    Containment,
    Pressure,
    SharedMemory,
    HandlePassing,
    PeerIdentity,
    LocalIpc,
    Durability,
    Gpu,
    Service,
    Paths,
    FileWatch,
    PrivateExecution,
    MountWatch,
}

impl fmt::Display for Seam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Process => "process control",
            Self::Containment => "process containment",
            Self::Pressure => "memory pressure",
            Self::SharedMemory => "shared memory",
            Self::HandlePassing => "handle passing",
            Self::PeerIdentity => "peer identity",
            Self::LocalIpc => "local IPC",
            Self::Durability => "file durability",
            Self::Gpu => "GPU probe",
            Self::Service => "user services",
            Self::Paths => "paths",
            Self::FileWatch => "file watching",
            Self::PrivateExecution => "protected project execution",
            Self::MountWatch => "mount watching",
        })
    }
}

/// The OSes `cozy` builds for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    /// The OS this binary was built for.
    #[cfg(target_os = "linux")]
    pub const CURRENT: Os = Os::Linux;
    #[cfg(target_os = "macos")]
    pub const CURRENT: Os = Os::Macos;
    #[cfg(windows)]
    pub const CURRENT: Os = Os::Windows;

    /// Whether models run on this OS's own machine (`local`). Off, nothing installs packages or
    /// downloads weights here and runs go to a rented Linux GPU; an OS turns on in a release.
    pub const fn local_inference(self) -> bool {
        matches!(self, Os::Linux)
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Linux => "Linux",
            Self::Macos => "macOS",
            Self::Windows => "Windows",
        })
    }
}
