//! Whether a process TensorD waits on is alive (cl-977 G9). Nothing judges how fast it goes: it is hung only when, for
//! `platform::contain::STALL`, none of its threads used CPU or moved a byte, none waits on time or on a TCP peer that answers,
//! its GPUs did nothing, and TensorD did no work for it. Then it is killed, and what waited on it says so ([`Hung`]).
//! A thread's wait is read from its syscall, which a host that denies TensorD ptrace (Yama `ptrace_scope` 2, no CAP_SYS_PTRACE)
//! keeps from it: an executor's own vigil ([`Line`]) reads itself and answers. One whose line never answered can't report, and
//! what can't be read counts as alive; once it has answered, a wait it doesn't say is no sign of life. A report can only make an
//! executor look alive, which its code can do anyway by spinning.
use platform::process::{Exact, Life, Wait};
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex, OnceLock,
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

/// An executor's vigil line (cl-977): its own `~vigil` thread (`tensorfs.executor.vigil`) answers each numbered ask with its
/// threads (`platform::process::Reading`).
pub(crate) struct Line {
    #[cfg(unix)]
    stream: Mutex<(std::os::unix::net::UnixStream, Vec<u8>)>,
    asked: AtomicU64,
    answered: AtomicBool,
}

impl Line {
    #[cfg(unix)]
    pub(crate) fn new(stream: std::os::unix::net::UnixStream) -> Self {
        Self { stream: Mutex::new((stream, vec![])), asked: AtomicU64::new(0), answered: AtomicBool::new(false) }
    }

    /// Its threads now, if it answers within `SAMPLE`; an answer to an earlier ask is passed over.
    #[cfg(unix)]
    fn ask(&self) -> Option<Vec<Life>> {
        use std::io::{Read, Write};
        let mut held = self.stream.lock().ok()?;
        let (stream, pending) = &mut *held;
        let ask = self.asked.fetch_add(1, Ordering::Relaxed) + 1;
        stream.write_all(format!("{ask}\n").as_bytes()).ok()?;
        let until = Instant::now() + SAMPLE;
        loop {
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let answer: Vec<u8> = pending.drain(..=end).collect();
                match serde_json::from_slice::<platform::process::Reading>(&answer) {
                    Ok(reading) if reading.ask == ask => {
                        self.answered.store(true, Ordering::Release);
                        return Some(reading.threads);
                    }
                    _ => {}
                }
            }
            let left = until.checked_duration_since(Instant::now()).filter(|left| !left.is_zero())?;
            stream.set_read_timeout(Some(left)).ok()?;
            let mut chunk = [0; 1 << 16];
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(read) if pending.len() < 16 << 20 => pending.extend_from_slice(&chunk[..read]),
                Ok(_) => pending.clear(),
            }
        }
    }
    #[cfg(not(unix))]
    fn ask(&self) -> Option<Vec<Life>> {
        None
    }

    /// It has answered once: it reports.
    fn reports(&self) -> bool {
        self.answered.load(Ordering::Acquire)
    }
}

/// Watches a process group while TensorD waits on it; dropped, it stops, and never kills after.
pub(crate) struct Vigil {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    hung: Arc<OnceLock<Hung>>,
}

impl Vigil {
    /// `leader` and its process group; `answering` is set while TensorD answers one of its requests; `line`, its vigil's.
    pub(crate) fn keep(leader: &Exact, answering: Arc<AtomicBool>, elsewhere: Elsewhere, line: Option<Arc<Line>>) -> io::Result<Self> {
        let leader = leader.try_clone()?;
        let (stop, stopped) = mpsc::channel::<()>();
        let verdict = Arc::new(OnceLock::new());
        let found = verdict.clone();
        let thread = std::thread::Builder::new().name(format!("vigil-{}", leader.birth().pid)).spawn(move || {
            let mut watch = Watch::new(answering, elsewhere, line);
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

/// A thread as read: its process, itself, and whether its process's vigil read it (a meter is compared only with one read the
/// same way).
type Seen = (u32, u32, bool);

struct Watch {
    answering: Arc<AtomicBool>,
    elsewhere: Elsewhere,
    line: Option<Arc<Line>>,
    /// Each thread's meter, the processes read and how, and the GPUs' busy readings.
    seen: HashMap<Seen, (u64, u64)>,
    read: HashSet<(u32, bool)>,
    gpu: u64,
    /// When something last moved (CPU, bytes, GPU, a thread came or went), and when it last showed any life.
    moved: Instant,
    alive: Instant,
    peers: HashMap<SocketAddr, Peer>,
    said: bool,
}

impl Watch {
    fn new(answering: Arc<AtomicBool>, elsewhere: Elsewhere, line: Option<Arc<Line>>) -> Self {
        let now = Instant::now();
        Self {
            answering, elsewhere, line, seen: HashMap::new(), read: HashSet::new(), gpu: 0, moved: now, alive: now,
            peers: HashMap::new(), said: false,
        }
    }

    fn hung(&mut self, leader: &Exact) -> bool {
        let now = Instant::now();
        let first = leader.birth().pid;
        let pids = std::iter::once(first).chain(platform::process::group_members(leader.birth()).into_iter().map(|m| m.pid));
        let mut own = self.line.as_ref().and_then(|line| line.ask());
        let reports = self.line.as_ref().is_some_and(|line| line.reports());
        let (mut moved, mut why, mut peers) = (false, vec![], vec![]);
        let (mut seen, mut read) = (HashMap::new(), HashSet::new());
        for pid in pids {
            let (lives, vigil) = match own.take().filter(|_| pid == first) {
                Some(threads) => (Some(threads), true),
                None => (platform::process::lives(pid), false),
            };
            // A process it can't read decides nothing.
            let Some(lives) = lives else {
                moved = true;
                continue;
            };
            read.insert((pid, vigil));
            for life in lives.into_iter().filter(|life| !life.name.starts_with(WATCHER)) {
                seen.insert((pid, life.tid, vigil), life.meter);
                match life.wait {
                    // Unreadable (or running, which its meter shows): alive, from an executor that can't report.
                    Wait::Unknown => moved |= !reports,
                    Wait::Time => why.push(format!("thread {} ({}) waits on time", life.tid, life.name)),
                    Wait::Peers { peers: waits } => peers.extend(waits),
                    Wait::Still => {}
                }
            }
        }
        // A thread that moved, or came or went in a process read the same way both times.
        let both = |(pid, _, vigil): &Seen| read.contains(&(*pid, *vigil)) && self.read.contains(&(*pid, *vigil));
        moved |= seen.iter().any(|(key, meter)| self.seen.get(key).is_some_and(|before| before != meter));
        moved |= seen.keys().any(|key| !self.seen.contains_key(key) && both(key));
        moved |= self.seen.keys().any(|key| !seen.contains_key(key) && both(key));
        (self.seen, self.read) = (seen, read);
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
            why.extend(self.peer(&peers));
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
    fn peer(&mut self, waits: &[platform::net::TcpWait]) -> Option<String> {
        for wait in waits {
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
        None
    }
}
