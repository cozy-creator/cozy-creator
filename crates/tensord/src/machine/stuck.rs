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
    /// Its thread in the kernel (`D`), else one waiting there (the thread that called exit sleeps interruptibly, its kill dropped),
    /// else its first.
    pub(crate) fn thread(&self) -> Option<&Thread> {
        let waiting = || self.threads.iter().find(|t| !t.wchan.is_empty() && t.wchan != "0");
        self.threads.iter().find(|t| t.state == 'D').or_else(waiting).or(self.threads.first())
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
            Some(t) if !t.wchan.is_empty() && t.wchan != "0" => {
                format!("pid {pid} can't exit: thread {} ({}) waits in the kernel at {} (state {})", t.tid, t.name, t.wchan, t.state)
            }
            Some(t) => format!("pid {pid} can't exit: thread {} ({}) is in state {}", t.tid, t.name, t.state),
            None => format!("pid {pid} can't exit"),
        }
    }

    /// It still lives: a thread of that birth hasn't exited. Read from its threads too: a zombie leader whose last thread still
    /// waits in the kernel can read as exited by its own state (stuck cell, 43715143776).
    pub(crate) fn alive(&self) -> bool {
        let same = platform::process::Birth::of(self.birth.pid).is_ok_and(|now| now == self.birth);
        same && (!platform::process::threads(self.birth.pid).is_empty() || self.birth.ended().is_ok_and(|ended| !ended))
    }
}

/// The TensorDs before this service that can't exit, shown in its Status until each does (cl-977). Each exit is waited on where
/// the kernel gives a handle, and judged as Status is read where it doesn't (a leader still exiting may have none). A machine
/// they left broken starts again once the last is gone, so a fresh service opens its GPUs again.
pub(crate) struct Before {
    shown: std::sync::Mutex<Vec<Stuck>>,
    paths: Paths,
    broken: bool,
}

impl Before {
    pub(crate) fn watch(stuck: Vec<Stuck>, paths: Paths, broken: bool) -> std::sync::Arc<Self> {
        let before = std::sync::Arc::new(Self { shown: std::sync::Mutex::new(stuck.clone()), paths, broken });
        for one in stuck {
            let (watched, pid) = (before.clone(), one.birth.pid);
            let watching = std::thread::Builder::new().name(format!("stuck-{pid}")).spawn(move || {
                if let Ok(Some(exact)) = platform::process::Exact::open(&one.birth) {
                    drop(exact.wait());
                }
                // A zombie leader's handle can read exited while a thread of it is still in the kernel (seen on Linux 6.x).
                if one.alive() {
                    return tracing::info!(target: crate::config::MACHINE, "pid {pid}, the TensorD before this one, still has a thread in the kernel; Status judges it as read");
                }
                watched.exited(&one.birth);
            });
            if let Err(error) = watching {
                tracing::warn!("pid {pid}, the TensorD before this one, is judged only as Status is read: {error}");
            }
        }
        before
    }

    /// Those still there.
    pub(crate) fn now(&self) -> Vec<Stuck> {
        let shown = self.shown.lock().unwrap().clone();
        let (alive, gone): (Vec<Stuck>, Vec<Stuck>) = shown.into_iter().partition(Stuck::alive);
        gone.iter().for_each(|one| self.exited(&one.birth));
        alive
    }

    fn exited(&self, birth: &Birth) {
        let left = {
            let mut shown = self.shown.lock().unwrap();
            let before = shown.len();
            shown.retain(|one| one.birth != *birth);
            if shown.len() == before {
                return;
            }
            shown.len()
        };
        tracing::info!(target: crate::config::MACHINE, "pid {}, the TensorD before this one, exited", birth.pid);
        exited(&self.paths, birth);
        crate::activity::status_changed();
        if self.broken && left == 0 {
            tracing::warn!(target: crate::config::MACHINE, "what left this machine broken exited: it starts again, to open its GPUs");
            super::supervise::leave(super::update::REPLACE_EXIT);
        }
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
