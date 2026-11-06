//! Process exit: once `exit` runs its handlers, no varena thread calls the driver again. CUDA's and
//! NVML's libraries tear down after those handlers, under any thread still inside them: a process
//! that took memory from a peer and exited died of SIGSEGV after its last line, 1 exit in 4 on a 5090.
use std::sync::{Once, PoisonError, RwLock, RwLockReadGuard};

static QUIET: RwLock<()> = RwLock::new(());
static REGISTERED: Once = Once::new();

/// Held by a background thread around its driver work. At exit it waits for the work in hand and
/// then never returns, so the thread stops where it stands.
pub(crate) fn driver_work() -> RwLockReadGuard<'static, ()> {
    REGISTERED.call_once(register);
    QUIET.read().unwrap_or_else(PoisonError::into_inner)
}

#[allow(unsafe_code)]
fn register() {
    // SAFETY: `quiesce` is a plain function with no arguments; registered once, after the driver
    // libraries loaded, so it runs before their own exit handlers and destructors.
    unsafe { libc::atexit(quiesce) };
}

extern "C" fn quiesce() {
    std::mem::forget(QUIET.write().unwrap_or_else(PoisonError::into_inner));
}
