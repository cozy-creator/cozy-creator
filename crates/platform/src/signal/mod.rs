//! A local event one process sets and its own threads wait on, alongside other handles. The one
//! module built without the `seams` feature: varena's shared core links it.
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
