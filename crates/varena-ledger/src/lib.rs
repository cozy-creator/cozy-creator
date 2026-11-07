//! varena's state between processes: a ledger page per GPU and one for the host (`Table`, Linux),
//! and the host's number (`host`), which varena alone sizes.
#[cfg(target_os = "linux")]
mod table;
#[cfg(target_os = "linux")]
pub use table::*;

pub mod host;
