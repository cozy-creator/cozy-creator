//! Sampled foreign pressure, owned by the native service thread. Nothing here grants bytes.
use super::{Peer, Result};
use crate::ledger;
use nvml_wrapper::{Nvml, enums::device::UsedGpuMemory};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;

const NEW_PROCESS: u64 = 256 << 20;
const RESERVE_QUANTUM: u64 = 32 << 20;
// Linux UAPI include/uapi/linux/nsfs.h, enum init_ns_ino. Unlike a matching numeric
// PID in a nested namespace, this establishes that NVML and getpid use host IDs.
const PID_NS_INIT_INO: u64 = 0xefff_fffc;

#[derive(Clone, Copy, Debug)]
pub enum Boundary {
    RequestBegin,
    RequestEnd,
}

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub samples: u64,
    pub observed_at: Option<u64>,
    pub free: Option<u64>,
    pub total: Option<u64>,
    pub own_opaque: Option<u64>,
    pub foreign: Option<u64>,
    pub attribution: Option<&'static str>,
    pub reserve: Option<u64>,
    pub local_reserve: Option<u64>,
    pub measured_requests: u64,
    pub rejected_requests: u64,
    pub measurement: Option<&'static str>,
    pub new_processes: Vec<u32>,
    pub process_signals: u64,
    pub process_signals_served: u64,
    pub process_signals_expired: u64,
    pub suppressed: bool,
    pub round: Option<u64>,
    pub completed: u64,
    pub budget_exhausted: u64,
}

struct Window {
    epoch: u64,
    initial: u64,
    growth: u64,
    valid: bool,
}

pub(super) struct Pressure {
    nvml: Option<Nvml>,
    uuid: String,
    host_pids: bool,
    previous: Option<BTreeMap<u32, u64>>,
    pending_processes: BTreeSet<u32>,
    signal_deadline: Option<std::time::Instant>,
    requests: usize,
    window: Option<Window>,
    last: Option<(u64, u64)>,
    largest_growth: Option<u64>,
    pub facts: Facts,
}

impl Pressure {
    pub fn new(descriptor: ledger::Device) -> Self {
        let prefix = match descriptor.kind {
            ledger::DeviceKind::Gpu => "GPU",
            ledger::DeviceKind::Mig => "MIG",
        };
        let b = descriptor.uuid;
        let uuid = format!(
            "{prefix}-{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        );
        Self {
            // Called only after Peer::open on the GPU service thread, never CPU Hello.
            nvml: Nvml::init().ok(),
            uuid,
            host_pids: initial_pid_namespace(),
            previous: None,
            pending_processes: BTreeSet::new(),
            signal_deadline: None,
            requests: 0,
            window: None,
            last: None,
            largest_growth: None,
            facts: Facts::default(),
        }
    }

    fn processes(&self) -> Option<BTreeMap<u32, u64>> {
        if !self.host_pids {
            return None;
        }
        let device = self
            .nvml
            .as_ref()?
            .device_by_uuid(self.uuid.as_str())
            .ok()?;
        let mut rows = BTreeMap::<u32, u64>::new();
        // Neither partial process lists nor unavailable per-process values qualify.
        for process in device
            .running_compute_processes()
            .ok()?
            .into_iter()
            .chain(device.running_graphics_processes().ok()?)
        {
            let UsedGpuMemory::Used(bytes) = process.used_gpu_memory else {
                return None;
            };
            let value = rows.entry(process.pid).or_default();
            *value = (*value).max(bytes); // Compute + graphics can report the same process.
        }
        Some(rows)
    }

    /// Sample outside shared locks. Native physical-call epochs bracket process accounting;
    /// windows with any VMM change are rejected rather than guessing an accounting delay.
    /// NVML still is a sampled source, not an atomic CUDA/NVML snapshot.
    pub fn observe(&mut self, peer: &Peer) -> Result<ledger::Sample> {
        let before = peer.memory_calls()?;
        let reading = peer.publish(0)?;
        let sample = peer.sample_reserve(ledger::monotonic_ns()?, self.facts.local_reserve)?;
        let rows = peer.observation(|| self.processes())?;
        let after = peer.memory_calls()?;
        let table = peer.snapshot()?;
        self.facts.samples += 1;
        self.facts.observed_at = Some(sample.observed_at);
        self.facts.free = Some(sample.total - sample.driver_used);
        self.facts.total = Some(sample.total);
        self.facts.reserve = sample.reserve;
        self.last = None;
        self.facts.own_opaque = None;
        self.facts.foreign = None;
        self.facts.attribution = Some(if self.host_pids {
            "process_accounting_unavailable"
        } else {
            "pid_namespace_unknown"
        });
        if let Some(rows) = rows {
            let peers: BTreeSet<_> = table.rows.iter().map(|row| row.pid).collect();
            let foreign: BTreeMap<_, _> = rows
                .iter()
                .filter(|(pid, _)| !peers.contains(pid))
                .map(|(&pid, &bytes)| (pid, bytes))
                .collect();
            self.facts.foreign = foreign
                .values()
                .try_fold(0u64, |sum, bytes| sum.checked_add(*bytes));
            if let Some(previous) = &self.previous {
                for (&pid, &bytes) in &foreign {
                    if bytes >= NEW_PROCESS
                        && previous.get(&pid).copied().unwrap_or(0) < NEW_PROCESS
                    {
                        if self.pending_processes.insert(pid) {
                            self.facts.process_signals += 1;
                            self.signal_deadline.get_or_insert_with(|| std::time::Instant::now() + crate::RECOVERY_BUDGET);
                        }
                    }
                }
            }
            self.pending_processes
                .retain(|pid| foreign.get(pid).is_some_and(|bytes| *bytes >= NEW_PROCESS));
            if self.pending_processes.is_empty() { self.signal_deadline = None; }
            self.previous = Some(foreign);
            self.facts.attribution = Some("nvml_host_pid_sampled");
            if before == after && reading.refreshed {
                self.facts.own_opaque = rows
                    .get(&std::process::id())
                    .and_then(|own| own.checked_sub(reading.physical.total().ok()?));
                self.last = self.facts.own_opaque.map(|opaque| (after, opaque));
            }
        }
        self.facts.new_processes = self.pending_processes.iter().copied().collect();
        if let Some(window) = &mut self.window {
            match self.last {
                Some((epoch, opaque)) if epoch == window.epoch => {
                    window.growth = window.growth.max(opaque.saturating_sub(window.initial));
                }
                _ => window.valid = false,
            }
        }
        Ok(sample)
    }

    pub fn boundary(&mut self, kind: Boundary) -> crate::Result<()> {
        match kind {
            Boundary::RequestBegin => {
                self.requests = self
                    .requests
                    .checked_add(1)
                    .ok_or(crate::Error::Invalid("request count overflow"))?;
                if self.requests == 1 {
                    self.window = self.last.map(|(epoch, initial)| Window {
                        epoch,
                        initial,
                        growth: 0,
                        valid: true,
                    });
                } else if let Some(window) = &mut self.window {
                    window.valid = false; // Overlap is not one attributable request.
                }
            }
            Boundary::RequestEnd => {
                self.requests = self
                    .requests
                    .checked_sub(1)
                    .ok_or(crate::Error::Invalid("request end without begin"))?;
                if self.requests == 0 {
                    if let Some(window) = self.window.take().filter(|window| window.valid) {
                        let growth = self
                            .largest_growth
                            .map_or(window.growth, |old| old.max(window.growth));
                        if let Some(reserve) = growth
                            .checked_mul(2)
                            .and_then(|bytes| bytes.checked_add(RESERVE_QUANTUM - 1))
                        {
                            let reserve = reserve / RESERVE_QUANTUM * RESERVE_QUANTUM;
                            if self.facts.total.is_some_and(|total| reserve <= total) {
                                self.largest_growth = Some(growth);
                                self.facts.local_reserve = Some(reserve);
                                self.facts.measured_requests += 1;
                                self.facts.measurement = Some("qualified_nvml_stable_native_epoch");
                            } else {
                                self.facts.rejected_requests += 1;
                                self.facts.measurement = Some("reserve_exceeds_capacity");
                            }
                        } else {
                            self.facts.rejected_requests += 1;
                            self.facts.measurement = Some("reserve_overflow");
                        }
                    } else {
                        self.facts.rejected_requests += 1;
                        self.facts.measurement = Some("unattributable_request_window");
                    }
                }
            }
        }
        Ok(())
    }

    /// A client disappeared without completing its request. This is not a successful
    /// boundary and cannot publish a reserve from the interrupted measurement window.
    pub fn abandon(&mut self, requests: usize) -> crate::Result<()> {
        if requests == 0 { return Ok(()); }
        self.requests = self.requests.checked_sub(requests)
            .ok_or(crate::Error::Invalid("abandoned request count exceeds active requests"))?;
        self.invalidate_observation();
        if self.requests == 0 { self.window = None; }
        self.facts.rejected_requests += 1;
        self.facts.measurement = Some("abandoned_request_window");
        Ok(())
    }

    pub fn invalidate_observation(&mut self) {
        self.last = None;
        if let Some(window) = &mut self.window {
            window.valid = false;
        }
    }

    pub fn wanted(&self) -> u64 {
        if !self.pending_processes.is_empty() {
            return self.facts.total.unwrap_or(0);
        }
        match (self.facts.free, self.facts.reserve) {
            (Some(free), Some(reserve)) if free < reserve => {
                reserve.saturating_mul(2).saturating_sub(free)
            }
            _ => 0,
        }
    }

    pub fn calm(&self) -> bool {
        // A threshold crossing is one release episode, not a permanent foreign-presence
        // drain. Thereafter fresh free/H governs recovery even if that process stays alive.
        self.pending_processes.is_empty()
            && self.facts.free.is_some()
            && self
                .facts
                .free
                .zip(self.facts.reserve)
                .is_none_or(|(free, reserve)| free >= reserve.saturating_mul(2))
    }

    pub fn consume_process_signals(&mut self) {
        self.facts.process_signals_served += self.pending_processes.len() as u64;
        self.pending_processes.clear();
        self.facts.new_processes.clear();
        self.signal_deadline = None;
    }

    pub fn expire_process_signals(&mut self) {
        if self.signal_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            self.facts.process_signals_expired += self.pending_processes.len() as u64;
            self.pending_processes.clear();
            self.facts.new_processes.clear();
            self.signal_deadline = None;
        }
    }

    pub fn signal_deadline(&self) -> Option<std::time::Instant> { self.signal_deadline }
}

/// No CUDA/NVML initialization. Unavailable or nested namespace identity stays unknown.
pub fn initial_pid_namespace() -> bool {
    std::fs::metadata("/proc/self/ns/pid").is_ok_and(|meta| meta.ino() == PID_NS_INIT_INO)
}
