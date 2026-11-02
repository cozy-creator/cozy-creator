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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn racing_hello_capabilities_never_start_parallel_policies() {
        for _ in 0..32 {
            let modes = Arc::new(Modes::new(1));
            assert!(
                !modes.legacy(0..1),
                "unknown must allow discovery before byte policy"
            );
            let gate = Arc::new(Barrier::new(3));
            let spawn = |capable| {
                let (modes, gate) = (modes.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    modes.bind(0..1, capable)
                })
            };
            let old = spawn(false);
            let new = spawn(true);
            gate.wait();
            let (old, new) = (old.join().unwrap(), new.join().unwrap().unwrap());
            match new {
                Mode::Legacy => assert_eq!(old.unwrap(), Mode::Legacy),
                Mode::Cooperative => {
                    assert_eq!(old.unwrap_err().kind(), io::ErrorKind::Unsupported)
                }
                Mode::Unselected => panic!("a successful Start has one owner"),
            }
            assert_eq!(modes.legacy(0..1), new == Mode::Legacy);
        }
    }

    #[test]
    fn incompatible_lane_does_not_partially_bind_unknown_devices() {
        let modes = Modes::new(3);
        assert_eq!(modes.bind(1..2, true).unwrap(), Mode::Cooperative);
        assert!(modes.bind(0..3, false).is_err());
        assert_eq!(modes.bind(0..1, true).unwrap(), Mode::Cooperative);
        assert_eq!(modes.bind(2..3, false).unwrap(), Mode::Legacy);
        assert!(modes.bind(0..3, true).is_err());
        assert!(!modes.legacy(0..2));
        assert!(modes.legacy(2..3));
    }
}
