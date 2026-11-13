//! An alarm at a wall-clock time, which also rings once a suspend or a clock change passed it,
//! and which another thread wakes: a timerfd on CLOCK_REALTIME (Linux), a kqueue timer on
//! continuous time (macOS), a waitable timer (Windows). For deadlines a person's clock sets,
//! which a monotonic sleep misses across a suspend.
use crate::Result;
use std::time::SystemTime;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

pub struct Alarm(imp::Alarm);

impl Alarm {
    pub fn new() -> Result<Self> {
        imp::Alarm::new().map(Self)
    }

    /// Blocks until `at` passes (true) or a wake (false). With no `at`, only a wake ends it. A
    /// wake before the wait ends the next one at once.
    pub fn wait(&self, at: Option<SystemTime>) -> Result<bool> {
        self.0.wait(at)
    }

    pub fn wake(&self) {
        self.0.wake();
    }
}

/// `at` as time since the epoch, never zero: zero would disarm a timer.
#[cfg_attr(windows, allow(dead_code))]
fn since_epoch(at: SystemTime) -> std::time::Duration {
    at.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().max(std::time::Duration::from_nanos(1))
}
