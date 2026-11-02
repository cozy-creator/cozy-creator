//! One categorical memory-policy owner per device for this Pool's lifetime.
//! This contains no byte quantities, CUDA calls, admission budgets or mode-switch timer.
use std::{io, ops::Range, sync::Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Unselected,
    Legacy,
    Cooperative,
}

pub(super) struct Modes(Mutex<Vec<Mode>>);
impl Modes {
    pub(super) fn new(devices: usize) -> Self {
        Self(Mutex::new(vec![Mode::Unselected; devices]))
    }

    /// Bind a whole lane atomically. A capable executor can explicitly serve Legacy; an
    /// incapable one cannot enter a Cooperative device, and lanes never mix policy owners.
    pub(super) fn bind(&self, lane: Range<usize>, capable: bool) -> io::Result<Mode> {
        let mut devices = self
            .0
            .lock()
            .map_err(|_| io::Error::other("device memory-policy mutex poisoned"))?;
        let members = devices
            .get_mut(lane)
            .filter(|members| !members.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "empty or invalid device lane")
            })?;
        let legacy = members.contains(&Mode::Legacy);
        let cooperative = members.contains(&Mode::Cooperative);
        if (legacy && cooperative) || (cooperative && !capable) {
            return Err(io::Error::new(io::ErrorKind::Unsupported,
                "device memory policy is cooperative: this executor/lane cannot join that capability; no legacy fallback"));
        }
        let selected = if legacy || (!cooperative && !capable) {
            Mode::Legacy
        } else {
            Mode::Cooperative
        };
        members.fill(selected);
        Ok(selected)
    }

    pub(super) fn legacy(&self, lane: Range<usize>) -> bool {
        self.0.lock().is_ok_and(|devices| {
            devices.get(lane).is_some_and(|members| {
                !members.is_empty() && members.iter().all(|mode| *mode == Mode::Legacy)
            })
        })
    }
}
