//! TensorFS's core: the cozytensors format, the local store, its writer and the read engine.
//! No network, no async runtime, no model families: those are `tensorfs`'s.

// Unsafe code lives only in platform, the OS seam.
#![deny(unsafe_code)]

/// File descriptor headroom: how many a process may hold open, and what's left.
pub mod descriptors;
pub mod err;
pub mod failure;
pub use failure::{Failure, Kind, Result};
pub mod format;
pub mod home;
/// The read engine: byte ranges of files into host or device destinations (xs-055).
pub mod io;
pub mod kernels;
pub mod sha256;
pub mod store;
pub mod writer;
/// A checkpoint's objects held to their digests.
pub mod verification;
pub use verification::VerificationReport;

/// The on-disk store this build reads and writes, shared by every process on a machine. A
/// change an older build would misread bumps it (`tensorfs.store/3`: trees and D10 headers).
pub const STORE_FORMAT: u32 = 3;
