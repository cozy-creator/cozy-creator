//! One caller's cooperative stop signal. Clones name the same intent; a resumed operation takes
//! a new one and keeps what the store already holds.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Stop(Arc<AtomicBool>);

impl Stop {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// Waits out `pause` unless stopped first: whether it waited all of it.
    pub(crate) fn sleep(&self, pause: std::time::Duration) -> bool {
        let until = std::time::Instant::now() + pause;
        while !self.is_cancelled() {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(std::time::Duration::from_millis(100)));
        }
        false
    }
}
