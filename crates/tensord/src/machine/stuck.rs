//! A service that can't exit (cl-977, muromisan: an update's restart waited forever on a TensorD whose last thread sat in the
//! NVIDIA driver). Its stable parent gives a leaving service [`GRACE_S`] to exit, kills it, and calls it stuck once it shows no
//! progress for [`STALL_S`]; it records it here (`update/stuck.json`) and starts the next service beside it. That service reads
//! the record: it takes over what the stuck one still holds (its locks; the parent holds the listeners), checks a fresh process can
//! still open the GPUs, and shows it in Status until it exits. Linux reads threads; elsewhere none is ever recorded.
use platform::process::{Birth, Thread};
use serde::{Deserialize, Serialize};
use std::io;

use super::update::Paths;

/// How long a service that said it is leaving, or was told to stop, may take before it is killed.
pub(crate) const GRACE_S: u32 = 10;
/// How long a killed service may show no progress (CPU, threads) before it is called stuck: cl-935's bound.
pub(crate) const STALL_S: u32 = platform::contain::STALL.as_secs() as u32;
/// A killed service still moving is waited on this many stalls at most.
pub(crate) const STALLS: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stuck {
    pub birth: Birth,
    pub killed_unix_ms: i64,
    /// Its threads when it was called stuck.
    pub threads: Vec<Thread>,
    /// The exit it said it was taking, if it said.
    #[serde(default)]
    pub code: Option<i32>,
}

impl Stuck {
    /// Its thread in the kernel (`D`), else its first.
    pub(crate) fn thread(&self) -> Option<&Thread> {
        self.threads.iter().find(|t| t.state == 'D').or(self.threads.first())
    }

    pub(crate) fn wchan(&self) -> String {
        self.thread().map(|t| t.wchan.clone()).unwrap_or_default()
    }

    /// `pid 93 can't exit: thread 102 (host-pressure) is in the kernel at uvm_parent_gpu_replayable_faults_isr_lock`.
    pub(crate) fn line(&self) -> String {
        let pid = self.birth.pid;
        match self.thread() {
            Some(t) if t.state == 'D' => {
                let at = Some(&t.wchan).filter(|w| !w.is_empty()).map(|w| format!(" at {w}")).unwrap_or_default();
                format!("pid {pid} can't exit: thread {} ({}) is in the kernel{at}", t.tid, t.name)
            }
            Some(t) => format!("pid {pid} can't exit: thread {} ({}) is in state {}", t.tid, t.name, t.state),
            None => format!("pid {pid} can't exit"),
        }
    }

    /// It still lives: a thread of that birth hasn't exited.
    pub(crate) fn alive(&self) -> bool {
        self.birth.ended().is_ok_and(|ended| !ended)
    }
}

/// Adds `stuck` to the record the next service reads.
pub(crate) fn record(paths: &Paths, stuck: Stuck) -> io::Result<()> {
    let mut all = read(paths);
    all.retain(Stuck::alive);
    all.push(stuck);
    write(paths, &all)
}

/// The earlier services still stuck, the record kept to them.
pub(crate) fn current(paths: &Paths) -> Vec<Stuck> {
    let all = read(paths);
    let alive: Vec<Stuck> = all.iter().filter(|stuck| stuck.alive()).cloned().collect();
    if alive.len() != all.len() {
        if let Err(error) = write(paths, &alive) {
            tracing::warn!("the record of stuck services: {error}");
        }
    }
    alive
}

/// One that exited since: the record forgets it.
pub(crate) fn exited(paths: &Paths, birth: &Birth) {
    let mut all = read(paths);
    all.retain(|stuck| stuck.birth != *birth && stuck.alive());
    if let Err(error) = write(paths, &all) {
        tracing::warn!("the record of stuck services: {error}");
    }
}

fn read(paths: &Paths) -> Vec<Stuck> {
    std::fs::read(paths.update("stuck.json")).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()
}

fn write(paths: &Paths, all: &[Stuck]) -> io::Result<()> {
    let path = paths.update("stuck.json");
    if all.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(paths.update(""))?;
    super::identity::write_atomic(&path, &serde_json::to_vec(all)?, 0o600)
}
