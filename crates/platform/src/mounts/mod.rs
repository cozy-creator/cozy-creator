//! Waking when this computer's mounts change: /proc/self/mountinfo's priority event (Linux), a
//! kqueue filesystem event (macOS). Windows has none: [`Changes::new`] answers Unsupported.
use crate::Result;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

pub struct Changes(imp::Changes);

/// Where `path`'s filesystem is mounted and what it was mounted from, as `statfs` says (macOS): `None` when it can't be read. One system
/// call and no process: what an encrypted project's mount is checked with on each request (cl-1111).
#[cfg(target_os = "macos")]
pub fn mount_of(path: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
    use std::os::unix::ffi::OsStringExt;
    let stat = rustix::fs::statfs(path).ok()?;
    let bytes = |name: &[std::ffi::c_char]| name.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect::<Vec<u8>>();
    let on = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes(&stat.f_mntonname)));
    Some((on, String::from_utf8_lossy(&bytes(&stat.f_mntfromname)).into_owned()))
}

impl Changes {
    pub fn new() -> Result<Self> {
        imp::Changes::new().map(Self)
    }

    /// Blocks until a mount comes or goes after the last wait, or `new`. No timeout.
    pub fn wait(&self) -> Result<()> {
        self.0.wait()
    }
}
