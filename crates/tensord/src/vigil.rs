//! Whether a process TensorD waits on is alive (cl-977 G9). Nothing judges how fast it goes: it is hung only when, for
//! `platform::contain::STALL`, none of its threads used CPU or moved a byte, none waits on time or on a TCP peer that answers,
//! its GPUs did nothing, and TensorD did no work for it. Then it is killed, and what waited on it says so ([`Hung`]).
use platform::process::{Exact, Wait};
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// A watcher thread's name begins with this (the Runtime's step sampler, varena's donor): its ticks show nothing of the work.
pub(crate) const WATCHER: char = '~';
const SAMPLE: Duration = Duration::from_secs(1);
/// A silent TCP peer is asked whether it is there by a connect of TensorD's own, this often and this long.
const PROBE_EVERY: Duration = Duration::from_secs(10);
const PROBE_WAIT: Duration = Duration::from_secs(2);
/// Probes in a row unanswered: the peer is gone.
const UNANSWERED: u32 = 3;

/// Signs of its life outside the process.
#[derive(Default)]
pub(crate) struct Elsewhere {
    /// Busy readings of the GPUs it computes on (`platform::gpu::Peak::busy`).
    pub gpu: Option<Arc<AtomicU64>>,
    /// It waits on work TensorD tracks elsewhere: a call it made that hasn't ended.
    pub waits: Option<Box<dyn Fn() -> bool + Send>>,
}

/// It showed no sign of life for the window, and was killed.
#[derive(Clone, Debug)]
pub(crate) struct Hung;
impl std::fmt::Display for Hung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no CPU, GPU or I/O for {} s, and nothing it waits on is alive: it was ended", platform::contain::STALL.as_secs())
    }
}
impl std::error::Error for Hung {}

/// `error` is a hang's.
pub(crate) fn hung(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Hung>())
}

/// Watches a process group while TensorD waits on it; dropped, it stops, and never kills after.
pub(crate) struct Vigil {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    hung: Arc<OnceLock<Hung>>,
}

impl Vigil {
    /// `leader` and its process group; `answering` is set while TensorD answers one of its requests.
    pub(crate) fn keep(leader: &Exact, answering: Arc<AtomicBool>, elsewhere: Elsewhere) -> io::Result<Self> {
        let leader = leader.try_clone()?;
        let (stop, stopped) = mpsc::channel::<()>();
        let verdict = Arc::new(OnceLock::new());
        let found = verdict.clone();
        let thread = std::thread::Builder::new().name(format!("vigil-{}", leader.birth().pid)).spawn(move || {
            let mut watch = Watch::new(answering, elsewhere);
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(SAMPLE) {
                if watch.hung(&leader) {
                    tracing::warn!("pid {}: no CPU, GPU or I/O for {} s, and nothing it waits on is alive: killed",
                        leader.birth().pid, platform::contain::STALL.as_secs());
                    let _ = found.set(Hung);
                    let _ = leader.kill();
                    return;
                }
            }
        })?;
        Ok(Self { stop: Some(stop), thread: Some(thread), hung: verdict })
    }

    /// Stops watching: whether it killed the process meanwhile.
    pub(crate) fn end(mut self) -> Option<Hung> {
        self.stop();
        self.hung.get().cloned()
    }

    /// `result`, unless it failed because this killed the process.
    pub(crate) fn judged<T>(self, result: io::Result<T>) -> io::Result<T> {
        match (result, self.end()) {
            (Err(_), Some(hung)) => Err(io::Error::other(hung)),
            (result, _) => result,
        }
    }

    fn stop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Vigil {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Peer {
    asked: Instant,
    unanswered: u32,
}

struct Watch {
    answering: Arc<AtomicBool>,
    elsewhere: Elsewhere,
    /// Each thread's meter by (pid, tid), and the GPUs' busy readings.
    seen: HashMap<(u32, u32), (u64, u64)>,
    gpu: u64,
    /// When something last moved (CPU, bytes, GPU, a thread came or went), and when it last showed any life.
    moved: Instant,
    alive: Instant,
    peers: HashMap<SocketAddr, Peer>,
    said: bool,
}

impl Watch {
    fn new(answering: Arc<AtomicBool>, elsewhere: Elsewhere) -> Self {
        let now = Instant::now();
        Self { answering, elsewhere, seen: HashMap::new(), gpu: 0, moved: now, alive: now, peers: HashMap::new(), said: false }
    }

    fn hung(&mut self, leader: &Exact) -> bool {
        let now = Instant::now();
        let pids = std::iter::once(leader.birth().pid).chain(platform::process::group_members(leader.birth()).into_iter().map(|m| m.pid));
        let (mut moved, mut why, mut waits) = (false, vec![], vec![]);
        let mut present = HashSet::new();
        for pid in pids {
            // A process it can't read decides nothing.
            let Some(lives) = platform::process::lives(pid) else {
                moved = true;
                continue;
            };
            for life in lives.into_iter().filter(|life| !life.name.starts_with(WATCHER)) {
                present.insert((pid, life.tid));
                moved |= self.seen.insert((pid, life.tid), life.meter) != Some(life.meter);
                match life.wait {
                    Wait::Unknown => moved = true,
                    Wait::Time => why.push(format!("thread {} ({}) waits on time", life.tid, life.name)),
                    Wait::On(fds) => waits.push((pid, fds)),
                    Wait::Still => {}
                }
            }
        }
        let threads = self.seen.len();
        self.seen.retain(|key, _| present.contains(key));
        moved |= self.seen.len() != threads;
        if let Some(gpu) = &self.elsewhere.gpu {
            let busy = gpu.load(Ordering::Relaxed);
            moved |= busy != self.gpu;
            self.gpu = busy;
        }
        if moved {
            (self.moved, self.alive, self.said) = (now, now, false);
            return false;
        }
        if self.answering.load(Ordering::Acquire) {
            why.push("TensorD answers its request".into());
        }
        if self.elsewhere.waits.as_ref().is_some_and(|waits| waits()) {
            why.push("a call it made goes on".into());
        }
        if why.is_empty() {
            why.extend(self.peer(&waits));
        }
        if why.is_empty() {
            return now.duration_since(self.alive) >= platform::contain::STALL;
        }
        self.alive = now;
        if !self.said && now.duration_since(self.moved) >= platform::contain::STALL {
            self.said = true;
            tracing::info!("pid {}: no CPU, GPU or I/O for {} s, alive: {}", leader.birth().pid,
                platform::contain::STALL.as_secs(), why.join("; "));
        }
        false
    }

    /// A TCP peer it waits on that is there: one acknowledging what it sent, or, with nothing unacknowledged, one that answers a
    /// connect of TensorD's own (a SYN-ACK). A refusal, or `UNANSWERED` probes in a row, and it is gone.
    fn peer(&mut self, waits: &[(u32, Vec<i32>)]) -> Option<String> {
        for (pid, fds) in waits {
            for wait in platform::net::tcp_waits(*pid, fds) {
                if wait.unacked {
                    match wait.backoff {
                        false => return Some(format!("{} acknowledges what it sent", wait.remote)),
                        true => continue,
                    }
                }
                let peer = self.peers.entry(wait.remote).or_insert(Peer { asked: Instant::now() - PROBE_EVERY, unanswered: 0 });
                if peer.asked.elapsed() >= PROBE_EVERY {
                    peer.asked = Instant::now();
                    match TcpStream::connect_timeout(&wait.remote, PROBE_WAIT) {
                        Ok(_) => peer.unanswered = 0,
                        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => peer.unanswered = UNANSWERED,
                        Err(_) => peer.unanswered += 1,
                    }
                }
                if peer.unanswered < UNANSWERED {
                    return Some(format!("it waits on {}, which answers", wait.remote));
                }
            }
        }
        None
    }
}
