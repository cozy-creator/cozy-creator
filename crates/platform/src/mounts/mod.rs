//! Waking when this computer's mounts change: /proc/self/mountinfo's priority event (Linux), a
//! kqueue filesystem event (macOS). Windows has none: [`Changes::new`] answers Unsupported.
use crate::Result;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

pub struct Changes(imp::Changes);

impl Changes {
    pub fn new() -> Result<Self> {
        imp::Changes::new().map(Self)
    }

    /// Blocks until a mount comes or goes after the last wait, or `new`. No timeout.
    pub fn wait(&self) -> Result<()> {
        self.0.wait()
    }
}
