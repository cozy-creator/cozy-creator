//! Live host memory: what this process can still allocate before its tightest limit. Read on
//! every decision, never cached: a budget fixed at boot describes a machine nobody else is on.
//! Linux ports Runtime `proctree.host_memory` (cgroup v1 and v2, every parent, and
//! `MemAvailable`); macOS reads the VM statistics, Windows the global memory status.
use serde::Serialize;
use std::io;

#[cfg_attr(target_os = "linux", path = "host_memory_linux.rs")]
#[cfg_attr(target_os = "macos", path = "host_memory_macos.rs")]
#[cfg_attr(windows, path = "host_memory_windows.rs")]
mod sys;

#[cfg(target_os = "linux")]
pub(crate) use sys::{cgroups, events, number};

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct HostMemory {
    /// Bytes left before the tightest limit on this process's cgroup path, or the host's
    /// available memory when that is tighter. -1: unreadable (never 0).
    pub available: i64,
    /// Shared memory (memfd/tmpfs) charged where that limit is: host pages and sealed layouts.
    /// 0 where weights are never held in shared memory (macOS, Windows).
    pub shmem: i64,
    /// The host's available memory (`MemAvailable` on Linux), for the record; -1 when unreadable.
    pub mem_available: i64,
    /// The limit `available` is measured against: the tightest cgroup's, else physical memory.
    pub bound: i64,
    /// Clean active page cache charged where that limit is, left out of `available`: the kernel
    /// takes it back under pressure once it has aged it. A cache holding the bytes that page cache
    /// holds (TensorFS's host tier) counts it as room. 0 where `available` is the host's own.
    pub active: i64,
}

/// Physical memory; -1 when unreadable.
pub fn total() -> i64 {
    sys::total()
}

/// The computer the machine runs on: physical memory, or less where a limit above the machine's
/// own cgroups is (a container's, a unit's `MemoryMax`); never the limit the machine sets itself.
pub fn computer() -> Option<u64> {
    #[cfg(target_os = "linux")]
    return sys::computer();
    #[cfg(not(target_os = "linux"))]
    u64::try_from(total()).ok().filter(|&n| n > 0)
}

/// What the computer has available outside the machine's own cgroups: `MemAvailable`, or less
/// where a limit above them has less room (Linux); elsewhere the live budget. -1 when unreadable.
pub fn available_outside() -> i64 {
    #[cfg(target_os = "linux")]
    return sys::available_outside();
    #[cfg(not(target_os = "linux"))]
    read().available
}

/// What a person's computer keeps for its own programs beside a shared machine: a quarter of it
/// ([`computer`]), from 4 to 16 GiB, plus what its free swap is short of a quarter of its swap (a
/// full swap takes nothing those programs push out). Live: swap moves.
pub fn reserve() -> u64 {
    let (swap, free) = sys::swap();
    (computer().unwrap_or(0) / 4).clamp(4 << 30, 16 << 30) + (swap / 4).saturating_sub(free)
}

/// The host stalling on memory now: PSI's `some avg10` above this (a tenth of the last 10 s with a
/// task waiting on memory). Paul's laptop, swap full all day on 11-06, sampled at most 3.7.
pub const STALLED: f32 = 10.0;

/// How much this process waits on memory: PSI `some avg10`, percent, the most of the system's
/// and of each limited cgroup v2 on its path (a limit stalls only what is under it). Limit events
/// are no measure: page cache filling a limit counts one per reclaim. 0 where unreadable.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct Stall {
    pub some_avg10: f32,
}

impl Stall {
    pub fn pressed(&self) -> bool {
        self.some_avg10 > STALLED
    }
}

pub fn stall() -> Stall {
    sys::stall()
}

/// The most this process's memory can reach: physical memory, or less where a limit on it is
/// (Linux: the tightest cgroup on its path, `memory.max` and a set `memory.high`, v1's
/// `memory.limit_in_bytes`). What is sized to the host (a host table, the RAM a policy may swap
/// to) is sized from this, never from physical memory: past it the kernel kills. None when
/// unreadable.
pub fn limit() -> Option<u64> {
    let physical = u64::try_from(total()).ok().filter(|&n| n > 0);
    physical.into_iter().chain(sys::limit()).min()
}

/// The live budget: the tightest of the host's available memory and, on Linux, every cgroup
/// on this process's path.
pub fn read() -> HostMemory {
    sys::read()
}

/// The page cache a read ahead may still fill: what is available past half the limit. Negative:
/// none (or unreadable).
pub fn cache_room() -> i64 {
    let half = limit().map_or(i64::MAX, |limit| i64::try_from(limit / 2).unwrap_or(i64::MAX));
    read().available.saturating_sub(half)
}

/// Memory nothing holds, page cache included: the tightest of each limited cgroup's limit less
/// all it charges, and the host's `MemFree`. What a store read can cache without displacing
/// anything, its own earlier bytes among them. -1 when unreadable.
pub fn unfilled() -> i64 {
    sys::unfilled()
}

/// One process's resident and proportional set sizes: Linux's PSS counts a shared sealed layout
/// once across every process that maps it; macOS gives its physical footprint and Windows its
/// private commit there, with no shared part.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ProcessMemory {
    pub rss: u64,
    pub pss: u64,
    pub pss_shmem: u64,
}

pub fn process(pid: u32) -> io::Result<ProcessMemory> {
    sys::process(pid)
}

/// One process's resident anonymous bytes now. Linux only; elsewhere `Unsupported`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resident {
    pub anon: u64,
}

pub fn resident(pid: u32) -> io::Result<Resident> {
    sys::resident(pid)
}
