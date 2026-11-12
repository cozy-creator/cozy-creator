//! A Linux tmpfs profile is volatile application storage, not a disk temporary-directory claim.
use std::path::{Path, PathBuf};
use ::api::error::{Error, Kind};
use crate::api::Refusal;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Identity { device: u64, inode: u64 }

pub(crate) struct VolatileProfileLease { path: PathBuf, identity: Identity }

/// Only the owned-browser retirement path can mint this one-use proof.
pub(super) struct BrowserRetired {
    pub(super) profile: Identity,
    pub(super) birth: platform::process::Birth,
    pub(super) session: String,
}

fn refused() -> Refusal {
    Refusal(Error::new(Kind::Unavailable, "private_browser_profile", "Cozy needs a verified volatile private-browser profile on this platform"))
}

impl VolatileProfileLease {
    #[cfg(target_os = "linux")]
    pub(crate) fn create() -> Result<Self, Refusal> {
        use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
        let candidates = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).into_iter().chain([PathBuf::from("/dev/shm")]);
        for parent in candidates {
            if !parent.is_absolute() { continue; }
            let Ok(parent) = parent.canonicalize() else { continue };
            let Ok(stat) = rustix::fs::statfs(&parent) else { continue };
            if stat.f_type as u64 != 0x0102_1994 { continue; } // TMPFS_MAGIC; never disk fallback.
            let mut nonce = [0; 24];
            getrandom::fill(&mut nonce).map_err(|_| refused())?;
            let path = parent.join(format!("cozy-private-{}", hex::encode(nonce)));
            if std::fs::DirBuilder::new().mode(0o700).create(&path).is_err() { continue; }
            let meta = std::fs::symlink_metadata(&path).map_err(|_| refused())?;
            let stat = rustix::fs::statfs(&path).map_err(|_| refused())?;
            if !meta.is_dir() || meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o777 != 0o700 || stat.f_type as u64 != 0x0102_1994 {
                return Err(refused());
            }
            return Ok(Self { path, identity: Identity { device: meta.dev(), inode: meta.ino() } });
        }
        Err(refused())
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn create() -> Result<Self, Refusal> { Err(refused()) }

    pub(crate) fn path(&self) -> &Path { &self.path }
    pub(super) fn identity(&self) -> Identity { self.identity }

    /// No Drop deletion: a profile can be retired only after its exact process/session is gone.
    #[cfg(target_os = "linux")]
    pub(super) fn retire(self, retired: BrowserRetired) -> Result<(), Refusal> {
        use std::os::unix::fs::MetadataExt as _;
        if retired.profile != self.identity || retired.session.is_empty() || !retired.birth.ended().map_err(|_| refused())? { return Err(refused()); }
        let meta = std::fs::symlink_metadata(&self.path).map_err(|_| refused())?;
        if !meta.is_dir() || meta.dev() != self.identity.device || meta.ino() != self.identity.inode || meta.uid() != rustix::process::geteuid().as_raw() { return Err(refused()); }
        std::fs::remove_dir_all(&self.path).map_err(|_| refused())
    }
    #[cfg(not(target_os = "linux"))]
    pub(super) fn retire(self, _retired: BrowserRetired) -> Result<(), Refusal> { Err(refused()) }
}
