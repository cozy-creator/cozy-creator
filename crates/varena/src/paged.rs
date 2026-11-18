//! PROOF ONLY (var-030 section 8), never merged: working memory the driver pages. Eager working segments
//! are CUDA managed allocations, so one that does not fit the card moves to host memory and back under
//! the driver instead of failing. Weights and graph pools stay as they are. These are its books: the
//! managed bytes handed out, and the host memory that bounds what of them can leave the card.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[derive(Default)]
pub(crate) struct Books {
    on: AtomicBool,
    rebind: AtomicBool,
    /// Bytes of managed working segments. They are in `Device::working` too.
    bytes: AtomicUsize,
    peak: AtomicUsize,
    /// The most our bytes have passed the card by: what the driver had to keep off it.
    over_peak: AtomicUsize,
    /// Segments mapped with nothing left to give for them.
    forced: AtomicU64,
    /// Allocations refused because host memory could not hold what would leave the card.
    host_refusals: AtomicU64,
    /// Whether the last admission left bytes past the card, and host memory in use when none last were.
    over: AtomicBool,
    baseline: AtomicU64,
    rebinds: AtomicU64,
    rebound_bytes: AtomicU64,
}

/// `Books` for a run's record.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PagedStats {
    pub(crate) on: bool,
    pub(crate) bytes: usize,
    pub(crate) peak: usize,
    pub(crate) over_peak: usize,
    pub(crate) forced: u64,
    pub(crate) host_refusals: u64,
    pub(crate) rebinds: u64,
    pub(crate) rebound_bytes: u64,
}

impl Books {
    pub(crate) fn set(&self, on: bool, rebind: bool) {
        self.on.store(on, Ordering::Release);
        self.rebind.store(on && rebind, Ordering::Release);
    }

    #[inline]
    pub(crate) fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    #[inline]
    pub(crate) fn rebinds(&self) -> bool {
        self.rebind.load(Ordering::Relaxed)
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }

    pub(crate) fn mapped(&self, bytes: usize) {
        let now = self.bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        self.peak.fetch_max(now, Ordering::Relaxed);
    }

    pub(crate) fn freed(&self, bytes: usize) {
        self.bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    pub(crate) fn forced(&self, over: usize) {
        self.forced.fetch_add(1, Ordering::Relaxed);
        self.past(over);
    }

    /// Our bytes are `over` the card: a weight's map pushed managed pages off it.
    pub(crate) fn past(&self, over: usize) {
        self.over_peak.fetch_max(over, Ordering::Relaxed);
    }

    pub(crate) fn rebound(&self, bytes: usize) {
        self.rebinds.fetch_add(1, Ordering::Relaxed);
        self.rebound_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub(crate) fn stats(&self) -> PagedStats {
        PagedStats {
            on: self.on(),
            bytes: self.bytes(),
            peak: self.peak.load(Ordering::Relaxed),
            over_peak: self.over_peak.load(Ordering::Relaxed),
            forced: self.forced.load(Ordering::Relaxed),
            host_refusals: self.host_refusals.load(Ordering::Relaxed),
            rebinds: self.rebinds.load(Ordering::Relaxed),
            rebound_bytes: self.rebound_bytes.load(Ordering::Relaxed),
        }
    }

    /// Whether host memory can hold what may leave the card once `bytes` more are mapped: `managed` managed bytes
    /// then, our bytes `over` the card by so much. The driver keeps a host page for every managed page it has
    /// ever moved off the card until that allocation is freed (measured, driver 610.43), so with anything over
    /// the card every managed byte may come to need one. Of them, host memory already shows what it has grown by
    /// since nothing was over the card; the rest must fit what is free of its limit, or the kernel ends the
    /// container with no word (measured). Refused here instead, in one line.
    pub(crate) fn host_admits(&self, device: i32, bytes: usize, managed: usize, over: usize) -> bool {
        if over == 0 {
            self.over.store(false, Ordering::Release);
            return true;
        }
        let Some((used, limit)) = host() else { return true };
        if !self.over.swap(true, Ordering::AcqRel) {
            self.baseline.store(used, Ordering::Release);
        }
        let before = managed.saturating_sub(bytes) as u64;
        let seen = used.saturating_sub(self.baseline.load(Ordering::Acquire)).min(before);
        let need = (managed as u64).saturating_sub(seen);
        let room = limit.saturating_sub(used);
        if need <= room {
            return true;
        }
        self.host_refusals.fetch_add(1, Ordering::Relaxed);
        eprintln!(
            "varena: GPU {device}: {bytes} bytes refused: this working memory fits neither the card nor host memory: \
             {managed} bytes of it, {over} past the card, {seen} already in host memory, {room} of host memory's \
             {limit} bytes free"
        );
        false
    }
}

/// (bytes in use that cannot be given back, the limit) of what bounds this process's host memory: its
/// cgroup's (a container's limit) where that leaves less room than the machine's own.
#[cfg(target_os = "linux")]
fn host() -> Option<(u64, u64)> {
    fn number(path: &str) -> Option<u64> {
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }
    fn field(text: &str, name: &str) -> Option<u64> {
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some(name) {
                return parts.next()?.parse().ok();
            }
        }
        None
    }
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let total = field(&meminfo, "MemTotal:")? << 10;
    let available = field(&meminfo, "MemAvailable:")? << 10;
    let mut bound = (total.saturating_sub(available), total);
    // cgroup v2, then v1. A file cache page is given back under pressure; nothing else is, with no swap.
    let groups = [
        ("/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory.current", "/sys/fs/cgroup/memory.stat",
         ["inactive_file", "active_file"]),
        ("/sys/fs/cgroup/memory/memory.limit_in_bytes", "/sys/fs/cgroup/memory/memory.usage_in_bytes",
         "/sys/fs/cgroup/memory/memory.stat", ["total_inactive_file", "total_active_file"]),
    ];
    for (limit, current, stat, cache) in groups {
        let (Some(limit), Some(current)) = (number(limit), number(current)) else { continue };
        let stat = std::fs::read_to_string(stat).unwrap_or_default();
        let cached: u64 = cache.iter().filter_map(|name| field(&stat, name)).sum();
        let used = current.saturating_sub(cached);
        if limit.saturating_sub(used) < bound.1.saturating_sub(bound.0) {
            bound = (used, limit);
        }
        break;
    }
    Some(bound)
}

#[cfg(not(target_os = "linux"))]
fn host() -> Option<(u64, u64)> {
    None
}
