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
    /// W8: working memory placed in pinned host memory the card addresses, its most, and the allocations.
    host_bytes: AtomicUsize,
    host_peak: AtomicUsize,
    host_allocs: AtomicU64,
    /// Freed blocks whose dead pages were dropped (and their bytes): their host pages go with them.
    discards: AtomicU64,
    discarded_bytes: AtomicU64,
    /// Something was dropped: a large block taken again is placed on the card first, never left to first-touch faults.
    dropped: AtomicBool,
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
    pub(crate) discards: u64,
    pub(crate) discarded_bytes: u64,
    pub(crate) host_bytes: usize,
    pub(crate) host_peak: usize,
    pub(crate) host_allocs: u64,
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

    pub(crate) fn discarded(&self, bytes: usize) {
        self.discards.fetch_add(1, Ordering::Relaxed);
        self.discarded_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.dropped.store(true, Ordering::Release);
    }

    pub(crate) fn host_mapped(&self, bytes: usize) {
        let now = self.host_bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        self.host_peak.fetch_max(now, Ordering::Relaxed);
        self.host_allocs.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn host_freed(&self, bytes: usize) {
        self.host_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// W8: room for `bytes` of working memory in host memory: the machine's one host admission, else refused in one line.
    pub(crate) fn host_takes(&self, device: i32, bytes: usize) -> bool {
        match crate::host_admit(bytes as u64) {
            None | Some(Ok(())) => true,
            Some(Err(line)) => {
                self.host_refusals.fetch_add(1, Ordering::Relaxed);
                eprintln!("varena: GPU {device}: {bytes} bytes refused: {line}");
                false
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn dropped(&self) -> bool {
        self.dropped.load(Ordering::Acquire)
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
            discards: self.discards.load(Ordering::Relaxed),
            discarded_bytes: self.discarded_bytes.load(Ordering::Relaxed),
            host_bytes: self.host_bytes.load(Ordering::Relaxed),
            host_peak: self.host_peak.load(Ordering::Relaxed),
            host_allocs: self.host_allocs.load(Ordering::Relaxed),
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
        // The machine's one host admission (tensorfs-py `host.rs`, the host page): every executor's holdings counted,
        // idle executors asked to trim their tiers, else a refusal in one line.
        if let Some(admitted) = crate::host_admit(need) {
            return match admitted {
                Ok(()) => true,
                Err(line) => {
                    self.host_refusals.fetch_add(1, Ordering::Relaxed);
                    eprintln!("varena: GPU {device}: {bytes} bytes refused: {line}");
                    false
                }
            };
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

/// (bytes held that the kernel cannot take back, what they may reach) under the tightest limit on this process's
/// path: varena-ledger's host number (`host::budget`), which TensorD writes as the machine's `memory.max` and reads
/// on every cgroup up the path. The executor's own cgroup is one of them, so reading the container's root file alone
/// (the first proof build) saw no limit and let the kernel kill at 32 GB. The headroom is what a call grows by
/// before anyone looks again.
#[cfg(target_os = "linux")]
fn host() -> Option<(u64, u64)> {
    let budget = varena_ledger::host::budget();
    if budget.total <= 0 {
        return None;
    }
    let limit = budget.total.saturating_sub(budget.headroom).max(0);
    Some((u64::try_from(budget.held.max(0)).unwrap_or(0), u64::try_from(limit).unwrap_or(0)))
}

#[cfg(not(target_os = "linux"))]
fn host() -> Option<(u64, u64)> {
    None
}
