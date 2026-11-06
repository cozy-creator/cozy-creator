//! SCRATCH, never to land: time in the core's entry points, cumulative, to stderr once a second.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const NAMES: [&str; 28] = [
    "lock_wait", "gate_wait", "poll", "refresh_phase", "alloc", "alloc_slow", "free", "split_select",
    "split_begin", "split_end", "room", "use_on", "acquire", "prefetch_room", "memory_info", "fits",
    "plan_snapshot", "lease", "note_held", "trim_ready", "reslab", "split_run", "fence_poll",
    "slow_pending_ours", "slow_pending_ours_multi", "slow_pending_other", "slow_fragmented", "slow_short",
];
static NS: [AtomicU64; 28] = [const { AtomicU64::new(0) }; 28];
static N: [AtomicU64; 28] = [const { AtomicU64::new(0) }; 28];

/// Count one event under `index`, with `bytes` in the time slot.
pub(crate) fn count(index: usize, bytes: u64) {
    N[index].fetch_add(1, Relaxed);
    NS[index].fetch_add(bytes, Relaxed);
}
static LAST: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();

pub(crate) struct T(usize, Instant);

impl T {
    pub(crate) fn new(index: usize) -> Self {
        T(index, Instant::now())
    }
}

impl Drop for T {
    fn drop(&mut self) {
        NS[self.0].fetch_add(self.1.elapsed().as_nanos() as u64, Relaxed);
        N[self.0].fetch_add(1, Relaxed);
    }
}

pub(crate) fn tick() {
    let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
    let last = LAST.load(Relaxed);
    if now >= last + 1000 && LAST.compare_exchange(last, now, Relaxed, Relaxed).is_ok() {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let mut line = format!("varena-time t={wall}");
        for (i, name) in NAMES.iter().enumerate() {
            line += &format!(" {name}={}/{}", N[i].load(Relaxed), NS[i].load(Relaxed) / 1000);
        }
        eprintln!("{line}");
    }
}
