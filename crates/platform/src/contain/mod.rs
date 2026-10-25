//! An executor's containment and the host's memory, moved from TensorD (`scope.rs`,
//! `host_memory.rs`, `host_pressure.rs`). A scope is a cgroup or token on Linux and a Job Object
//! on Windows; elsewhere a scope and a pressure trigger answer `Unsupported`, and host memory
//! reads as unreadable (-1).
use sha2::{Digest, Sha256};
use std::path::Path;

pub mod host_memory;
pub mod host_pressure;
#[cfg_attr(target_os = "linux", path = "scope.rs")]
#[cfg_attr(windows, path = "scope_windows.rs")]
#[cfg_attr(not(any(target_os = "linux", windows)), path = "scope_unsupported.rs")]
mod scope;

pub use scope::{Scope, stuck};

/// A killed scope member stuck in the kernel (Linux `D`: a driver after an Xid, a hung network
/// filesystem). It acts on its kill only once that wait ends, and nothing waits on it meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stuck {
    pub pid: u32,
    pub command: String,
    /// The kernel function it sleeps in; empty when the kernel hides it.
    pub wchan: String,
    /// When an end found it stuck.
    pub since_unix_ms: i64,
}

/// Runtime `child_env.EXECUTOR_SCOPE_ENV`: one complete inherited environment entry.
pub const TOKEN_ENV: &str = "COZY_EXECUTOR_SCOPE";

/// Opaque, stable per state root, so a machine only ever sweeps its own scopes.
pub fn namespace(state: &Path) -> String {
    let path = state.canonicalize().unwrap_or_else(|_| state.to_path_buf());
    hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))[..16].to_string()
}
