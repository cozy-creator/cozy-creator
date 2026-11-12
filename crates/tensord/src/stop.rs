//! One caller's cooperative stop signal. Clones name the same intent; a resumed operation takes
//! a new one and keeps what the store already holds. A cancel wakes whoever waits on it: `sleep`,
//! `cancelled` (async), TensorFS's import (`flag` and `notify`) and each wake a wait registered
//! (`on_cancel`), so no wait looks at it on a timer.
use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, PoisonError,
    },
    time::Duration,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Stop(Arc<Inner>);

#[derive(Default)]
struct Inner {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    /// Held while `cancelled` is set and while `sleep` looks at it, so a cancel never falls between the look and the wait.
    wakes: Mutex<Vec<(u64, Wake)>>,
    changed: Condvar,
    next: AtomicU64,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stop").field("cancelled", &self.cancelled.load(Ordering::Acquire)).finish_non_exhaustive()
    }
}

/// A wake registered on a stop (`Stop::on_cancel`): it goes when this drops.
pub(crate) struct OnCancel(Arc<Inner>, u64);

impl Drop for OnCancel {
    fn drop(&mut self) {
        self.0.wakes.lock().unwrap_or_else(PoisonError::into_inner).retain(|(id, _)| *id != self.1);
    }
}

impl Stop {
    pub(crate) fn cancel(&self) {
        let wakes: Vec<Wake> = {
            let wakes = self.0.wakes.lock().unwrap_or_else(PoisonError::into_inner);
            if self.0.cancelled.swap(true, Ordering::AcqRel) {
                return;
            }
            wakes.iter().map(|(_, wake)| wake.clone()).collect()
        };
        self.0.changed.notify_all();
        self.0.notify.notify_waiters();
        wakes.iter().for_each(|wake| wake());
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    /// Waits out `pause` unless stopped first: whether it waited all of it.
    pub(crate) fn sleep(&self, pause: Duration) -> bool {
        let wakes = self.0.wakes.lock().unwrap_or_else(PoisonError::into_inner);
        drop(self.0.changed.wait_timeout_while(wakes, pause, |_| !self.is_cancelled()).unwrap_or_else(PoisonError::into_inner));
        !self.is_cancelled()
    }
    /// Returns once stopped; never for a stop no one cancels.
    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
    /// `wake` runs on a cancel while the returned guard lives: a wait on another condition (a condvar of its own) looks at
    /// this stop when woken. A wait must look at `is_cancelled` under its own lock, and `wake` take that lock before it
    /// notifies, so a cancel between the look and the wait isn't missed.
    pub(crate) fn on_cancel(&self, wake: impl Fn() + Send + Sync + 'static) -> OnCancel {
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        self.0.wakes.lock().unwrap_or_else(PoisonError::into_inner).push((id, Arc::new(wake)));
        OnCancel(self.0.clone(), id)
    }
    /// The flag and its wake, as TensorFS's import takes them (`tensorfs::import::Access`): it waits on them, not a timer.
    pub(crate) fn flag(&self) -> (Arc<AtomicBool>, Arc<tokio::sync::Notify>) {
        (self.0.cancelled.clone(), self.0.notify.clone())
    }
}
