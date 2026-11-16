//! Whether a process TensorD waits on is alive (cl-977 G9). Nothing judges how fast it goes. Its process tree (its group, and its
//! scope: a builder in a session of its own too) is read each second, thread by thread:
//! - work: a thread gaining [`WORK`] of a core, moving a byte or paging in from disk; its GPUs running a kernel; a thread coming
//!   or going.
//! - alive though still: TensorD answering one of its requests or running a call it made; a thread waiting on a TCP peer that
//!   answers; a timed wait of the call's own threads (the main thread, the Runtime's `call_*`), or of any thread that hasn't woken
//!   since the stillness began (a real sleep).
//! - neither: threads that only tick (wake from a timed poll and sleep again: CUDA's own, a library's monitor, `~` watchers) and
//!   untimed waits.
//! Nothing working and nothing alive for [`WINDOW`] is a hang: the executor is killed, and the run fails `hung`, saying what was
//! seen ([`Hung`]). A thread's wait is read from its syscall, which a host that denies TensorD ptrace keeps from it: an executor's
//! own vigil ([`Line`]) reads itself and answers. Until its line has answered, what can't be read counts as alive. A report can
//! only make an executor look alive, which its code can do anyway by spinning.
use platform::process::{Exact, Life, Meter, Wait};
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
/// The share of a core a thread gains in a sample that is work. Measured on a deadlocked GPU call (jugram, RTX 4090, pod-session
/// 43794289907): CUDA's own threads tick at 0.2-0.5 ms a second, the Runtime's and varena's watchers at 1-3 ms; work takes hundreds.
const WORK: f64 = 0.01;
/// Nothing working and nothing alive for this long is a hang. Several times the longest stillness measured in legitimate calls
/// (cl-977's table); a false kill loses a render, a late one costs minutes of a pod.
pub(crate) const WINDOW: Duration = Duration::from_secs(300);
const SAMPLE: Duration = Duration::from_secs(1);
/// A silent TCP peer is asked whether it is there by a connect of TensorD's own, this often and this long.
const PROBE_EVERY: Duration = Duration::from_secs(10);
const PROBE_WAIT: Duration = Duration::from_secs(2);
/// Probes in a row unanswered: the peer is gone.
const UNANSWERED: u32 = 3;

/// Signs of its life outside the process.
#[derive(Default)]
pub(crate) struct Elsewhere {
    /// Readings of the GPUs it computes on with a kernel running (`platform::gpu::Peak::busy`).
    pub gpu: Option<Arc<AtomicU64>>,
    /// It waits on work TensorD tracks elsewhere: a call it made that hasn't ended.
    pub waits: Option<Box<dyn Fn() -> bool + Send>>,
}

/// It showed no sign of life for the window and was killed: what was seen.
#[derive(Clone, Debug)]
pub(crate) struct Hung(pub String);
impl std::fmt::Display for Hung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; its executor was ended", self.0)
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

/// Watches a process tree while TensorD waits on it; dropped, it stops, and never kills after.
pub(crate) struct Vigil {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    hung: Arc<OnceLock<Hung>>,
}

impl Vigil {
    /// `leader`'s group and scope while it does `what`; `answering` is set while TensorD answers one of its requests; `line`, its
    /// vigil's.
    pub(crate) fn keep(leader: &Exact, what: &str, answering: Arc<AtomicBool>, elsewhere: Elsewhere, line: Option<Arc<Line>>) -> io::Result<Self> {
        let leader = leader.try_clone()?;
        let (stop, stopped) = mpsc::channel::<()>();
        let verdict = Arc::new(OnceLock::new());
        let (found, what) = (verdict.clone(), what.to_owned());
        let thread = std::thread::Builder::new().name(format!("vigil-{}", leader.birth().pid)).spawn(move || {
            let mut watch = Watch::new(answering, elsewhere, line);
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(SAMPLE) {
                if let Some(seen) = watch.look(&leader) {
                    tracing::warn!("pid {} ({what}): {seen}: killed", leader.birth().pid);
                    let _ = found.set(Hung(seen));
                    let _ = leader.kill();
                    return;
                }
            }
            // What legitimate calls show of stillness: the evidence for `WINDOW`.
            if watch.longest >= Duration::from_secs(10) {
                tracing::info!("pid {} ({what}): still for up to {} s (alive: {})", leader.birth().pid, watch.longest.as_secs(),
                    if watch.why.is_empty() { "nothing" } else { &watch.why });
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
type Key = (u32, u32, bool);

struct Watch {
    answering: Arc<AtomicBool>,
    elsewhere: Elsewhere,
    line: Option<Arc<Line>>,
    seen: HashMap<Key, Meter>,
    read: HashSet<(u32, bool)>,
    gpu: u64,
    sampled: Instant,
    /// Since when nothing worked, when something last showed life, and which threads woke meanwhile.
    still: Instant,
    alive: Instant,
    woke: HashSet<Key>,
    /// The longest stillness seen, and why it was alive then.
    longest: Duration,
    why: String,
    peers: HashMap<SocketAddr, Peer>,
    said: bool,
}

impl Watch {
    fn new(answering: Arc<AtomicBool>, elsewhere: Elsewhere, line: Option<Arc<Line>>) -> Self {
        let now = Instant::now();
        Self {
            answering, elsewhere, line, seen: HashMap::new(), read: HashSet::new(), gpu: 0, sampled: now, still: now, alive: now,
            woke: HashSet::new(), longest: Duration::ZERO, why: String::new(), peers: HashMap::new(), said: false,
        }
    }

    /// One sample: what was seen once it is a hang.
    fn look(&mut self, leader: &Exact) -> Option<String> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled).as_secs_f64();
        self.sampled = now;
        let first = leader.birth().pid;
        let mut own = self.line.as_ref().and_then(|line| line.ask());
        let reports = self.line.as_ref().is_some_and(|line| line.reports());
        let mut pids: Vec<u32> = std::iter::once(first).chain(platform::process::group_members(leader.birth()).into_iter().map(|m| m.pid)).collect();
        let mut threads: Vec<(Key, Life)> = vec![];
        let (mut read, mut worked) = (HashSet::new(), false);
        let mut take = |pid: u32, threads: &mut Vec<(Key, Life)>, read: &mut HashSet<(u32, bool)>, worked: &mut bool| {
            let (lives, vigil) = match own.take().filter(|_| pid == first) {
                Some(lives) => (Some(lives), true),
                None => (platform::process::lives(pid), false),
            };
            // A process gone between its listing and its read: one fewer, which counts as a change.
            let Some(lives) = lives else {
                *worked = true;
                return;
            };
            read.insert((pid, vigil));
            threads.extend(lives.into_iter().filter(|life| !life.name.starts_with(WATCHER)).map(|life| ((pid, life.tid, vigil), life)));
        };
        for pid in &pids {
            take(*pid, &mut threads, &mut read, &mut worked);
        }
        worked |= self.worked(&threads, &read, elapsed);
        // The rest of its scope (a builder in a session of its own), once its group shows no work.
        if !worked {
            for pid in leader.scope().map(|scope| scope.members()).unwrap_or_default() {
                if !pids.contains(&pid) {
                    pids.push(pid);
                    take(pid, &mut threads, &mut read, &mut worked);
                }
            }
            worked |= self.worked(&threads, &read, elapsed);
        }
        if let Some(gpu) = &self.elsewhere.gpu {
            let busy = gpu.load(Ordering::Relaxed);
            worked |= busy != self.gpu;
            self.gpu = busy;
        }
        for (key, life) in &threads {
            if self.seen.get(key).is_some_and(|before| life.meter.runs > before.runs) {
                self.woke.insert(*key);
            }
        }
        self.seen = threads.iter().map(|(key, life)| (*key, life.meter)).collect();
        self.read = read;
        if worked {
            (self.still, self.alive, self.said) = (now, now, false);
            self.woke.clear();
            return None;
        }
        let stillness = now.duration_since(self.still);
        // Alive though still, and what the call itself waits on.
        let (mut why, mut peers, mut untimed) = (vec![], vec![], false);
        for ((pid, tid, _), life) in &threads {
            let call = *tid == *pid && *pid == first || life.name.starts_with("call_");
            match &life.wait {
                Wait::Time if call || !self.woke.contains(&(*pid, *tid, false)) && !self.woke.contains(&(*pid, *tid, true)) => {
                    why.push(format!("thread {tid} ({}) waits on time", life.name));
                }
                Wait::Peers { peers: waits } => peers.extend(waits.iter().cloned()),
                Wait::Unknown if !reports => why.push(format!("thread {tid} ({}) can't be read", life.name)),
                Wait::Still if call => untimed = true,
                _ => {}
            }
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
        if stillness > self.longest {
            (self.longest, self.why) = (stillness, why.join("; "));
        }
        if !why.is_empty() {
            self.alive = now;
            if !self.said && stillness >= WINDOW {
                self.said = true;
                tracing::info!("pid {first}: no work for {} s, alive: {}", stillness.as_secs(), why.join("; "));
            }
            return None;
        }
        if now.duration_since(self.alive) < WINDOW {
            return None;
        }
        let ticking = self.woke.iter().filter(|(pid, tid, _)| !(*tid == *pid && *pid == first)).count();
        Some(format!(
            "{}; no CPU, GPU or I/O for {} s{}",
            if untimed { "its call waits with no deadline" } else { "its call does no work" },
            WINDOW.as_secs(),
            match ticking {
                0 => String::new(),
                1 => " (1 other thread only ticking)".into(),
                n => format!(" ({n} other threads only ticking)"),
            }
        ))
    }

    /// A thread gained `WORK` of a core, moved a byte or paged in, or came or went in a process read the same way both times.
    fn worked(&self, threads: &[(Key, Life)], read: &HashSet<(u32, bool)>, elapsed: f64) -> bool {
        let both = |(pid, _, vigil): &Key| read.contains(&(*pid, *vigil)) && self.read.contains(&(*pid, *vigil));
        let now: HashSet<&Key> = threads.iter().map(|(key, _)| key).collect();
        threads.iter().any(|(key, life)| match self.seen.get(key) {
            Some(before) => {
                let gained = life.meter.cpu_ns.saturating_sub(before.cpu_ns) as f64 / 1e9;
                gained >= WORK * elapsed || life.meter.bytes > before.bytes || life.meter.faults > before.faults
            }
            None => both(key),
        }) || self.seen.keys().any(|key| !now.contains(key) && both(key))
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
