//! Containment scopes exist only on Linux so far (cgroup v2 or a process-tree token).
use crate::{Error, Result, Seam};

#[derive(Debug)]
pub struct Scope(());

impl Scope {
    pub fn inherited(_: &str) -> Result<Self> {
        Err(Error::unsupported(Seam::Containment))
    }

    pub fn create(_: &str) -> Result<Self> {
        Err(Error::unsupported(Seam::Containment))
    }

    /// Namespace-wide process proofs are currently available only on Linux.
    pub fn namespace_processes(_: &str) -> Result<usize> {
        Err(Error::unsupported(crate::Seam::Containment))
    }

    pub fn sweep(_: &str) -> Result<usize> {
        Err(Error::unsupported(Seam::Containment))
    }

    /// Only Linux tells an OOM kill apart.
    pub fn oom_killed(&self) -> bool {
        false
    }

    pub fn cgroup_relative(&self) -> Option<&str> {
        None
    }

    pub fn environment(&self) -> Option<(&'static str, &str)> {
        None
    }

    pub fn trampoline_args(&self) -> Vec<String> {
        vec!["--scope-backend".into(), "inherit".into()]
    }

    pub fn adopt(&self, _: u32) -> Result<()> {
        Err(Error::unsupported(Seam::Containment))
    }

    pub(crate) fn kill(&self) -> Result<()> {
        Err(Error::unsupported(Seam::Containment))
    }

    pub fn end(&self) -> Result<usize> {
        Err(Error::unsupported(Seam::Containment))
    }
}

/// Only Linux tells a member stuck in the kernel apart.
pub fn stuck() -> Vec<super::Stuck> {
    Vec::new()
}

pub fn all_stuck(_: &[crate::process::Birth]) -> bool {
    false
}

/// What a contained machine may use (Linux only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Budget {
    pub max: u64,
    pub reserve: u64,
    pub used: u64,
}

/// Only Linux contains a machine in a cgroup of its own.
pub fn contain(_: &str) -> std::result::Result<std::path::PathBuf, String> {
    Err("no cgroup v2 on this system".into())
}

/// What the kernel accounts for a contained machine; never read here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineFacts {
    pub computer: u64,
    pub outside: u64,
    pub held: u64,
    pub own: u64,
}

pub fn follow(_: impl FnOnce(&MachineFacts) -> (u64, u64)) -> Option<std::io::Result<Budget>> {
    None
}

pub fn budget() -> Option<Budget> {
    None
}
