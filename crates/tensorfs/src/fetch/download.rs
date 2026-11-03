//! The downloader, the way an S3 client does it: one queue of whole-object GETs (objects are
//! chunks of at most 64 MiB), a paced number of workers, and any request that fails, stalls
//! or runs far slower than its peers restarted on a fresh connection from the byte it reached.
//!
//! - **Order.** The queue is a plain number, smaller first; equal numbers go in arrival order.
//!   An object wanted twice is downloaded once. A task nothing wants leaves the queue in the
//!   step that finds it so, then ends: a want after that starts a task of its own (cl-686).
//! - **Tiers.** The local store, then the volume (when one is bound), then the origin. Every
//!   byte enters the store's admission, which hashes it: wrong bytes are never admitted.
//! - **Judging a request** is relative and measured, never a timeout. Each request measures
//!   the rate of every stretch of body it receives between two writes into the store; every
//!   tick the watch drops one with no answer after 8× the fetch's median time to answer, or
//!   whose body moved under an eighth of the median stretch rate over its window (its last
//!   ticks on the network: a write into the store neither counts nor starts it over), and it is
//!   asked again from the byte it reached. An object whose dropped request moved nothing gets
//!   twice the patience next time, so an origin that is honestly slow still answers; one whose
//!   request moved bytes is asked again as at first. A cold R2 object stalls at a MiB boundary
//!   for whoever asks first, again and again, while a fresh ranged ask is answered at once (pod
//!   measurements, 2026-10-01); a patience that grew per drop let such a drip run unjudged for
//!   minutes (tfs-138). A fetch whose every request is slow restarts none.
//! - **A cold object** (about 0.2% of them) answers any one request at 20-70 KB/s, asked again or
//!   left alone, for whoever asks, whatever the URL or front end, while its requests add up: to
//!   0.3-1 MB/s at 32 at once, no more at 256 (tfs-141). So an object whose requests are dropped
//!   twice running having moved under a part is read as parts: ranges of `PART_BYTES`, `PARTS` at
//!   once, each judged only by its silence. They go into the store in order; those ahead wait in
//!   memory. At most `PARTED` objects at a time: the rest stay as they are until one lands. An
//!   object with under two parts to go stays one request: parts could gain it nothing.
//! - **Ending.** An object ends on an answer asking again cannot change (absent, refused after
//!   a fresh location, the same wrong bytes twice), on a dead peer, or when the origin has only
//!   answered "not now" while nothing in the whole fetch landed, for [`ask`]'s stall rule; the
//!   refusal quotes the last answer.
//! - **Pace.** Workers start at 16 and double each window while the queue waits and goodput
//!   grows by 1.2×, up to what the descriptor table holds (tfs-122); a full table halves them.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::future::Future;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{watch, Notify, Semaphore};

use super::ask;
use super::net::{same_origin, Answer, Client, GetError};
use super::origin::{HttpLocation, Location, Origin};
use super::token::TokenProvider;
use super::volume::{blocking, VolumeOrigin, Wrote, STEP};
use super::{Admission, ObjectStore, Tier};
use crate::err::{refuse, Code, Refusal, Result};
use crate::format::ObjectRef;

/// Where an origin redirected an object's GET, reused for restarts until the redirect's max-age.
type Delivery = (String, Option<Instant>);

/// How many times slower than its peers a request may run, or longer than their answer it may
/// wait, before it is dropped and asked again from the byte reached.
const STALL_FACTOR: f64 = 8.0;
/// The noise floor, in ticks, below which a wait for an answer or a silence is no evidence.
const HEAD_TICKS: u32 = 8;
/// Ticks over which a body's rate is judged.
const JUDGE_TICKS: usize = 4;
/// Attempts in a row the origin answered nothing at all (no connection, no answer) before an
/// object ends: a dead peer. Any answer or byte starts the count again.
const DEAD_PEER: u32 = 8;
/// The most a re-ask that moved nothing waits before the next, doubling from one tick.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Rate samples needed before any request is judged slow.
const MIN_SAMPLES: usize = 8;
/// The fewest bytes a stretch of body needs to be a rate sample.
const STRETCH_BYTES: u64 = 64 << 10;
/// Rate and answer-time samples kept.
const SAMPLES: usize = 256;
const MAX_DOUBLINGS: u32 = 6;
/// A part's bytes: under about 1 MiB a cold object's requests move less in all (64 KiB: 113 KB/s
/// at 16 at once, 1 MiB: 534), over it no more (4 MiB: 536).
const PART_BYTES: u64 = 1 << 20;
/// Parts of one object at once: 8 moved 273 KB/s, 16 about 534, 32 about 750, 64 to 256 no more.
const PARTS: usize = 32;
/// How far past its first unfinished part an object's parts may be, in parts: what memory holds.
const PARTS_AHEAD: u64 = 48;
/// Objects read as parts at once, which bounds their sockets and memory.
const PARTED: usize = 4;
/// Dropped requests in a row, each having moved under a part, before an object is read as parts.
const SLOW_ASKS: u32 = 2;
/// A part's patience, as doublings of the wait for an answer: a cold object's silences are longer
/// than a request's, while a dead connection must not hold its object for long.
const PART_DOUBLINGS: u32 = 2;
/// With no samples yet to judge it by, the rate under which a request is dropped, once it has had
/// 4× the noise floor: a fetch of one cold object has nothing else to compare it with.
const CRAWL_BYTES_PER_S: f64 = 1e6;
/// Bytes a request gathers before handing a write to the store.
const WRITE_BYTES: usize = 1 << 20;
/// Writes into the store at once: more add threads stalled in the kernel, not bytes.
const WRITERS: usize = 8;
const VOLUME_READS: usize = 8;
const WRITE_BACKS: usize = 2;
/// Ticks per pace window, and the gain that earns a doubling.
const PACE_TICKS: u32 = 8;
/// Bytes of an error answer's body a refusal keeps.
const SAID: usize = 512;
const PACE_GAIN: f64 = 1.2;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One object being fetched, and everyone waiting for it.
pub(crate) struct Task {
    pub object: ObjectRef,
    origin: Arc<dyn Origin>,
    priority: AtomicU64,
    waiters: AtomicUsize,
    started: AtomicBool,
    /// The most bytes any of its requests reached: only bytes past it are progress.
    reached: AtomicU64,
    /// Where it is (`Phase`), and when it last moved: its phase changed or its bytes grew (Unix ms).
    phase: AtomicU8,
    moved_ms: AtomicU64,
    done: watch::Sender<Option<std::result::Result<Tier, Refusal>>>,
}

impl Task {
    /// The most bytes its requests reached, and every part's.
    pub(crate) fn received(&self) -> u64 {
        self.reached.load(Ordering::Acquire).min(self.object.length)
    }

    fn enter(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
        self.moved_ms.store(unix_ms(), Ordering::Release);
    }
}

/// Where an object's task is: waiting for a worker, a worker looking here and at the datacenter
/// volume, reading the volume, asking the origin (locating, connecting or between attempts), or
/// receiving the origin's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Queued,
    Starting,
    Volume,
    Origin,
    Receiving,
}

impl Phase {
    fn of(value: u8) -> Phase {
        match value {
            1 => Phase::Starting,
            2 => Phase::Volume,
            3 => Phase::Origin,
            4 => Phase::Receiving,
            _ => Phase::Queued,
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Phase::Queued => "queued",
            Phase::Starting => "starting",
            Phase::Volume => "reading the datacenter volume",
            Phase::Origin => "connecting to the origin",
            Phase::Receiving => "receiving from the origin",
        })
    }
}

/// An object's task as its waiters see it (`Fetcher::fetching`): where it is, its bytes so far
/// of its length, when it last moved (Unix ms), and how many wait for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetching {
    pub phase: Phase,
    pub reached: u64,
    pub length: u64,
    pub moved_unix_ms: u64,
    pub waiters: usize,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A place in a task's line of waiters; dropping it gives the place up.
pub(crate) struct Waiter {
    task: Arc<Task>,
    answer: watch::Receiver<Option<std::result::Result<Tier, Refusal>>>,
}

impl Waiter {
    pub fn task(&self) -> Arc<Task> {
        Arc::clone(&self.task)
    }

    pub async fn wait(mut self) -> Result<Tier> {
        let answer = self.answer.wait_for(Option::is_some).await;
        match answer.as_deref() {
            Ok(Some(answer)) => answer.clone(),
            _ => refuse(
                Code::TRANSFER_FAILED,
                format!("{}: the fetcher closed", self.task.object.sha256),
            ),
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        self.task.waiters.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct Queue {
    tasks: HashMap<String, Arc<Task>>,
    heap: BinaryHeap<Reverse<(u64, u64, String)>>,
    seq: u64,
}

impl Queue {
    /// `task` leaves the queue, unless a task of its own already took its object's place.
    fn remove(&mut self, task: &Arc<Task>) {
        if self.tasks.get(&task.object.sha256).is_some_and(|t| Arc::ptr_eq(t, task)) {
            self.tasks.remove(&task.object.sha256);
        }
    }

    /// Whether nothing wants `task`; then it leaves the queue in the same step, under the queue's
    /// lock, so a want after that starts a task of its own and never joins one that is ending.
    fn unwanted(&mut self, task: &Arc<Task>) -> bool {
        let unwanted = task.waiters.load(Ordering::Acquire) == 0;
        if unwanted {
            self.remove(task);
        }
        unwanted
    }
}

/// Why the watch dropped a request.
#[derive(Clone)]
enum Dropped {
    Judged(String),
    Unwanted,
}

/// A request in flight, as the watch sees it.
struct Flight {
    task: Arc<Task>,
    file: bool,
    /// One part of an object: judged by its silence, never its rate, and no sample of the rate.
    part: bool,
    began: Instant,
    doublings: u32,
    answered: AtomicBool,
    /// Bytes received, and when the request last moved (nanoseconds since `began`).
    bytes: AtomicU64,
    moved: AtomicU64,
    /// Waiting on the store, not the network: not judged meanwhile.
    local: AtomicBool,
    /// Where the current stretch of body began: bytes, and nanoseconds since `began`.
    stretch: Mutex<(u64, u64)>,
    /// Bytes at each recent tick, newest last.
    window: Mutex<VecDeque<u64>>,
    dropped: Mutex<Option<Dropped>>,
    abort: Notify,
}

impl Flight {
    fn moved(&self) {
        self.moved
            .store(self.began.elapsed().as_nanos() as u64, Ordering::Release);
    }

    /// The stretch of body since the last write ends: its rate is a sample.
    fn stretch_ends(&self, measured: &Mutex<Measured>) {
        if self.part {
            return;
        }
        let (bytes, now) = (
            self.bytes.load(Ordering::Acquire),
            self.began.elapsed().as_nanos() as u64,
        );
        let (from, since) = *lock(&self.stretch);
        if bytes >= from + STRETCH_BYTES && now > since {
            push(
                &mut lock(measured).rates,
                (bytes - from) as f64 / Duration::from_nanos(now - since).as_secs_f64(),
            );
        }
    }

    fn stretch_begins(&self) {
        *lock(&self.stretch) = (
            self.bytes.load(Ordering::Acquire),
            self.began.elapsed().as_nanos() as u64,
        );
    }

    fn silent(&self) -> Duration {
        self.began
            .elapsed()
            .saturating_sub(Duration::from_nanos(self.moved.load(Ordering::Acquire)))
    }

    fn drop_with(&self, why: Dropped) {
        let mut dropped = lock(&self.dropped);
        if dropped.is_none() {
            *dropped = Some(why);
            self.abort.notify_one();
        }
    }

    /// Resolves once the watch drops this request.
    fn aborted(&self) -> impl Future<Output = ()> + '_ {
        self.abort.notified()
    }

    fn why(&self) -> Dropped {
        lock(&self.dropped).clone().unwrap_or(Dropped::Unwanted)
    }
}

/// A flight registered with the watch for as long as this lives.
struct Flying<'s> {
    shared: &'s Shared,
    flight: Arc<Flight>,
}

impl Drop for Flying<'_> {
    fn drop(&mut self) {
        lock(&self.shared.flights).retain(|f| !Arc::ptr_eq(f, &self.flight));
    }
}

#[derive(Default)]
struct Measured {
    /// Seconds from asking to the answer's head.
    heads: VecDeque<f64>,
    /// Bytes per second of one stretch of body.
    rates: VecDeque<f64>,
    /// The longest one volume step took.
    file_step: Duration,
}

/// The start of an error answer's body, its whitespace collapsed: what the origin said. Stops
/// at `SAID` bytes, the body's end or the flight's drop.
async fn said(answer: &mut Answer, flight: &Flight) -> String {
    let mut body = Vec::new();
    while body.len() < SAID {
        let frame = tokio::select! {
            frame = answer.frame() => frame,
            () = flight.aborted() => break,
        };
        match frame {
            Ok(Some(data)) => body.extend_from_slice(&data),
            Ok(None) | Err(_) => break,
        }
    }
    body.truncate(SAID);
    String::from_utf8_lossy(&body).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push(samples: &mut VecDeque<f64>, value: f64) {
    if samples.len() >= SAMPLES {
        samples.pop_front();
    }
    samples.push_back(value);
}

fn median(samples: &VecDeque<f64>) -> Option<f64> {
    let mut sorted: Vec<f64> = samples.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied()
}

struct Pace {
    level: usize,
    max: usize,
    /// Goodput at the last level, while growing.
    last: Option<f64>,
    settled: bool,
    /// Permits to retire as they come back.
    debt: usize,
}

/// What the Fetcher shares with its dispatcher, watch and transfers.
pub(crate) struct Shared {
    pub store: Arc<dyn ObjectStore>,
    pub client: Client,
    pub volume: Option<VolumeOrigin>,
    pub tick: Duration,
    patience: ask::Patience,
    queue: Mutex<Queue>,
    queued: Notify,
    closed: AtomicBool,
    closing: Notify,
    permits: Arc<Semaphore>,
    pace: Mutex<Pace>,
    flights: Mutex<Vec<Arc<Flight>>>,
    measured: Mutex<Measured>,
    /// Transfers running.
    active: AtomicUsize,
    /// Tasks and write-backs not yet ended.
    busy: AtomicUsize,
    settle: Notify,
    writes: Semaphore,
    volume_reads: Semaphore,
    write_backs: Semaphore,
    parted: Semaphore,
    pub stats: Counters,
}

#[derive(Default)]
pub(crate) struct Counters {
    pub requests: AtomicU64,
    pub restarts: AtomicU64,
    pub landed: AtomicU64,
    pub volume_written: AtomicU64,
    pub volume_skipped: AtomicU64,
}

enum Attempt {
    Done,
    /// Ask again: after `wait` when the origin stated one; `dropped` when the watch judged it.
    Again {
        why: String,
        wait: Option<Duration>,
        dropped: bool,
        /// The origin answered: a dead peer answers nothing.
        heard: bool,
    },
    /// The origin said "not now": ask again, after `wait` when it stated one.
    Weather { why: String, wait: Option<Duration> },
    /// 401/403: the location may be stale.
    Refused(Refusal),
    /// A part got the whole object: this origin ignores ranges, so the object is one GET.
    NoRanges,
    /// Where an origin redirected this object answers no more (`why`): ask the origin again.
    Stale(String),
    Fatal(Refusal),
}

impl From<Refusal> for Attempt {
    fn from(refusal: Refusal) -> Attempt {
        match refusal.code {
            Code::TRANSFER_FAILED | Code::HUB_UNREACHABLE | Code::FD_HEADROOM => Attempt::Again {
                why: refusal.to_string(),
                wait: None,
                dropped: false,
                heard: false,
            },
            _ => Attempt::Fatal(refusal),
        }
    }
}

/// An object's re-asks. A request that stalls is dropped and asked again from the byte reached
/// (`judge`), and an origin that answers "not now" is asked again when it says. Two things end
/// it: a dead peer, `DEAD_PEER` attempts in a row that it answered nothing at all; and an origin
/// that says "not now" once nothing in the fetch has landed for the stall rule, or for twice the
/// longest wait it stated. A re-ask that moved nothing waits longer each time, up to `MAX_BACKOFF`.
struct Retry {
    tick: Duration,
    unheard: u32,
    quiet: u32,
    /// The fetch's bytes landed when last read, when they last changed, and the longest wait stated.
    landed: u64,
    still: Instant,
    stated_most: Duration,
}

impl Retry {
    fn new(tick: Duration, landed: u64) -> Self {
        Self { tick, unheard: 0, quiet: 0, landed, still: Instant::now(), stated_most: Duration::ZERO }
    }

    /// The pause before asking again after a "not now", or the origin's refusal: it is not a
    /// network failure for a caller to wait out, and asking on would only keep a machine billing.
    /// As in [`ask::again`], a pause stays under an eighth of what it is judged by, so the verdict
    /// rests on many answers and not on the last two.
    fn weather(&mut self, task: &Task, why: &str, wait: Option<Duration>, landed: u64, patience: Duration) -> Result<Duration> {
        if landed != self.landed {
            (self.landed, self.still) = (landed, Instant::now());
        }
        self.stated_most = self.stated_most.max(wait.unwrap_or_default());
        let (still, allowed) = (self.still.elapsed(), patience.max(self.stated_most * 2));
        if still > allowed {
            let detail = format!("{}: nothing landed for {:.0} s and the origin only said \"not now\"; the last: {why}", task.object.sha256, still.as_secs_f64());
            return refuse(Code::ORIGIN_REFUSED, detail);
        }
        self.unheard = 0;
        self.quiet = (self.quiet + 1).min(16);
        Ok(wait.unwrap_or((self.tick * (1 << (self.quiet - 1))).min(allowed / 8)))
    }

    /// The pause before the next attempt (None: at once), or the dead peer's refusal.
    fn again(&mut self, task: &Task, why: &str, heard: bool, moved: bool, wait: Option<Duration>) -> Result<Option<Duration>> {
        self.unheard = if heard || moved { 0 } else { self.unheard + 1 };
        if self.unheard >= DEAD_PEER {
            let detail = format!("{}: the origin answered none of {DEAD_PEER} attempts in a row; the last: {why}", task.object.sha256);
            return refuse(Code::TRANSFER_FAILED, detail);
        }
        if moved {
            self.quiet = 0;
            return Ok(wait);
        }
        self.quiet = (self.quiet + 1).min(16);
        Ok(Some(wait.unwrap_or((self.tick * (1 << (self.quiet - 1))).min(MAX_BACKOFF))))
    }
}

/// How a file read ended.
enum FileRead {
    Admitted,
    Absent,
    /// The bytes were not the object's.
    Corrupt(Refusal),
    /// Abandoned as silent; the operation may still be blocked.
    Stuck(String),
    Failed(Refusal),
}

/// One object's bytes on their way into the store's admission.
struct Sink<'s> {
    shared: &'s Shared,
    task: &'s Task,
    admission: Option<Box<dyn Admission>>,
    buffer: Vec<u8>,
    reached: u64,
}

impl<'s> Sink<'s> {
    fn open(shared: &'s Shared, task: &'s Task) -> Result<Sink<'s>> {
        let admission = Some(shared.store.admit(&task.object)?);
        Ok(Sink {
            shared,
            task,
            admission,
            buffer: Vec::with_capacity(WRITE_BYTES),
            reached: 0,
        })
    }

    /// Start the object over.
    fn fresh(&mut self) -> Result<()> {
        self.admission = Some(self.shared.store.admit(&self.task.object)?);
        self.buffer.clear();
        self.reached = 0;
        Ok(())
    }

    async fn push(&mut self, flight: &Flight, data: &[u8]) -> Result<()> {
        let length = self.task.object.length;
        if self.reached + data.len() as u64 > length {
            return refuse(
                Code::LENGTH_MISMATCH,
                format!(
                    "{}: the origin sent more than its {length} B",
                    self.task.object.sha256
                ),
            );
        }
        self.buffer.extend_from_slice(data);
        self.reached += data.len() as u64;
        self.shared.advance(self.task, self.reached);
        if self.buffer.len() >= WRITE_BYTES {
            self.flush(Some(flight)).await?;
        }
        Ok(())
    }

    /// The next bytes, from parts whose arrival has counted already.
    async fn deliver(&mut self, bytes: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(bytes);
        self.reached += bytes.len() as u64;
        if self.buffer.len() >= WRITE_BYTES {
            self.flush(None).await?;
        }
        Ok(())
    }

    async fn flush(&mut self, flight: Option<&Flight>) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let Some(mut admission) = self.admission.take() else {
            return refuse(Code::IO_FAILED, "the admission is gone");
        };
        let buffer = std::mem::take(&mut self.buffer);
        if let Some(flight) = flight {
            flight.local.store(true, Ordering::Release);
            flight.stretch_ends(&self.shared.measured);
        }
        let _writing = self.shared.writes.acquire().await;
        let written = tokio::task::spawn_blocking(move || {
            let written = admission.write(&buffer);
            (admission, buffer, written)
        })
        .await;
        if let Some(flight) = flight {
            flight.moved();
            flight.stretch_begins();
            flight.local.store(false, Ordering::Release);
        }
        let (admission, mut buffer, written) = written.map_err(|error| Refusal {
            code: Code::IO_FAILED,
            detail: error.to_string(),
        })?;
        buffer.clear();
        (self.admission, self.buffer) = (Some(admission), buffer);
        written
    }

    /// Everything written, hashed and admitted; the sink is spent until `fresh`.
    async fn commit(&mut self) -> Result<()> {
        self.flush(None).await?;
        let Some(admission) = self.admission.take() else {
            return refuse(Code::IO_FAILED, "the admission is gone");
        };
        let _writing = self.shared.writes.acquire().await;
        tokio::task::spawn_blocking(move || admission.commit())
            .await
            .map_err(|error| Refusal {
                code: Code::IO_FAILED,
                detail: error.to_string(),
            })?
    }
}

/// One range of an object, gathered in memory.
struct Part {
    reached: u64,
    end: u64,
    bytes: Vec<u8>,
}

/// Where a request's body goes: the object, in order, or one of its parts.
enum Target<'a, 's> {
    Sink(&'a mut Sink<'s>),
    Part(&'a mut Part),
}

impl Target<'_, '_> {
    fn reached(&self) -> u64 {
        match self {
            Target::Sink(sink) => sink.reached,
            Target::Part(part) => part.reached,
        }
    }

    /// The byte after the last it wants.
    fn end(&self, length: u64) -> u64 {
        match self {
            Target::Sink(_) => length,
            Target::Part(part) => part.end,
        }
    }

    async fn push(&mut self, shared: &Shared, task: &Task, flight: &Flight, data: &[u8]) -> Result<()> {
        match self {
            Target::Sink(sink) => sink.push(flight, data).await,
            Target::Part(part) => {
                if part.reached + data.len() as u64 > part.end {
                    return refuse(
                        Code::LENGTH_MISMATCH,
                        format!("{}: the origin sent more than the range asked", task.object.sha256),
                    );
                }
                part.bytes.extend_from_slice(data);
                part.reached += data.len() as u64;
                shared.gained(task, data.len() as u64);
                Ok(())
            }
        }
    }
}

impl Shared {
    pub fn new(
        store: Arc<dyn ObjectStore>,
        client: Client,
        volume: Option<VolumeOrigin>,
        tick: Duration,
        streams: usize,
        max_streams: usize,
    ) -> Shared {
        let max = crate::descriptors::requests(max_streams.max(1));
        let level = streams.clamp(1, max);
        Shared {
            store,
            client,
            volume,
            tick: tick.max(Duration::from_millis(1)),
            patience: ask::Patience::new(tick.max(Duration::from_millis(1))),
            queue: Mutex::default(),
            queued: Notify::new(),
            closed: AtomicBool::new(false),
            closing: Notify::new(),
            permits: Arc::new(Semaphore::new(level)),
            pace: Mutex::new(Pace {
                level,
                max,
                last: None,
                settled: false,
                debt: 0,
            }),
            flights: Mutex::default(),
            measured: Mutex::default(),
            active: AtomicUsize::new(0),
            busy: AtomicUsize::new(0),
            settle: Notify::new(),
            writes: Semaphore::new(WRITERS),
            volume_reads: Semaphore::new(VOLUME_READS),
            write_backs: Semaphore::new(WRITE_BACKS),
            parted: Semaphore::new(PARTED),
            stats: Counters::default(),
        }
    }

    pub fn streams(&self) -> usize {
        lock(&self.pace).level
    }

    /// Stops the dispatcher and the watch, and every request in flight.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.closing.notify_waiters();
        self.queued.notify_waiters();
        for flight in lock(&self.flights).iter() {
            flight.drop_with(Dropped::Unwanted);
        }
    }

    async fn closed(&self) {
        loop {
            let notified = self.closing.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Resolves once no task or write-back is left.
    pub async fn settled(&self) {
        loop {
            let notified = self.settle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.busy.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    fn unbusy(&self) {
        if self.busy.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.settle.notify_waiters();
        }
    }

    /// A place in line for `object`, joining its task when one is open.
    pub fn want(&self, object: &ObjectRef, origin: &Arc<dyn Origin>, priority: u64) -> Waiter {
        let mut queue = lock(&self.queue);
        let task = match queue.tasks.get(&object.sha256).cloned() {
            Some(task) => {
                task.waiters.fetch_add(1, Ordering::AcqRel);
                if priority < task.priority.load(Ordering::Acquire)
                    && !task.started.load(Ordering::Acquire)
                {
                    task.priority.store(priority, Ordering::Release);
                    queue.seq += 1;
                    let seq = queue.seq;
                    queue
                        .heap
                        .push(Reverse((priority, seq, object.sha256.clone())));
                }
                task
            }
            None => {
                let task = Arc::new(Task {
                    object: object.clone(),
                    origin: Arc::clone(origin),
                    priority: AtomicU64::new(priority),
                    waiters: AtomicUsize::new(1),
                    started: AtomicBool::new(false),
                    reached: AtomicU64::new(0),
                    phase: AtomicU8::new(Phase::Queued as u8),
                    moved_ms: AtomicU64::new(unix_ms()),
                    done: watch::channel(None).0,
                });
                queue.seq += 1;
                let seq = queue.seq;
                queue
                    .heap
                    .push(Reverse((priority, seq, object.sha256.clone())));
                queue.tasks.insert(object.sha256.clone(), Arc::clone(&task));
                self.busy.fetch_add(1, Ordering::AcqRel);
                self.queued.notify_one();
                task
            }
        };
        Waiter {
            answer: task.done.subscribe(),
            task,
        }
    }

    /// The most urgent task nobody has started that someone still wants.
    fn pop(&self) -> Option<Arc<Task>> {
        let mut queue = lock(&self.queue);
        while let Some(Reverse((priority, _, sha256))) = queue.heap.pop() {
            let Some(task) = queue.tasks.get(&sha256).cloned() else {
                continue;
            };
            if task.started.load(Ordering::Acquire)
                || task.priority.load(Ordering::Acquire) != priority
            {
                continue;
            }
            if queue.unwanted(&task) {
                self.unbusy();
                continue;
            }
            task.started.store(true, Ordering::Release);
            task.enter(Phase::Starting);
            return Some(task);
        }
        None
    }

    fn waiting(&self) -> bool {
        !lock(&self.queue).heap.is_empty()
    }

    /// `object`'s task as its waiters see it; None when none is open (landed, failed or unwanted).
    pub fn fetching(&self, object: &ObjectRef) -> Option<Fetching> {
        let task = lock(&self.queue).tasks.get(&object.sha256).cloned()?;
        Some(Fetching {
            phase: Phase::of(task.phase.load(Ordering::Acquire)),
            reached: task.received(),
            length: task.object.length,
            moved_unix_ms: task.moved_ms.load(Ordering::Acquire),
            waiters: task.waiters.load(Ordering::Acquire),
        })
    }

    fn finish(&self, task: &Arc<Task>, outcome: Result<Tier>) {
        lock(&self.queue).remove(task);
        task.done.send_replace(Some(outcome));
        self.unbusy();
    }

    /// Whether nothing wants `task` any more; then it has left the queue (`Queue::unwanted`).
    fn unwanted(&self, task: &Arc<Task>) -> bool {
        lock(&self.queue).unwanted(task)
    }

    /// Starts tasks as workers come free, most urgent first.
    pub async fn dispatch(self: Arc<Self>) {
        loop {
            let permit = tokio::select! {
                permit = Arc::clone(&self.permits).acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => return,
                },
                () = self.closed() => return,
            };
            let task = loop {
                let notified = self.queued.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.closed.load(Ordering::Acquire) {
                    return;
                }
                if let Some(task) = self.pop() {
                    break task;
                }
                notified.await;
            };
            let shared = Arc::clone(&self);
            tokio::spawn(async move {
                let outcome = shared.run(&task).await;
                let mut pace = lock(&shared.pace);
                if pace.debt > 0 {
                    pace.debt -= 1;
                    permit.forget();
                }
                drop(pace);
                shared.finish(&task, outcome);
            });
        }
    }

    async fn run(self: &Arc<Self>, task: &Arc<Task>) -> Result<Tier> {
        struct Active<'a>(&'a AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        let _active = Active(&self.active);
        let outcome = self.fetch(task).await;
        if let Err(refusal) = &outcome {
            tracing::info!(object = %task.object.sha256, %refusal, "fetch refused");
        }
        outcome
    }

    /// `task`'s object from the first tier that has it.
    async fn fetch(self: &Arc<Self>, task: &Arc<Task>) -> Result<Tier> {
        if self.store.has(&task.object) {
            return Ok(Tier::Local);
        }
        let mut replace = false;
        if let Some(volume) = self.volume.clone().filter(VolumeOrigin::answering) {
            let _reading = self.volume_reads.acquire().await;
            if volume.answering() {
                task.enter(Phase::Volume);
                match self
                    .read_file(task, &volume, volume.blob(&task.object), 0)
                    .await
                {
                    FileRead::Admitted => {
                        let (touching, object) = (volume.clone(), task.object.clone());
                        tokio::task::spawn_blocking(move || touching.touch(&object));
                        return Ok(Tier::Volume);
                    }
                    FileRead::Corrupt(refusal) => {
                        tracing::info!(object = %task.object.sha256, %refusal, "volume copy refused");
                        replace = true;
                    }
                    FileRead::Absent | FileRead::Stuck(_) => {}
                    FileRead::Failed(refusal) => return Err(refusal),
                }
            }
        }
        self.from_origin(task).await?;
        if task.origin.public() {
            self.write_back(task.object.clone(), replace);
        }
        Ok(Tier::Origin)
    }

    /// The origin's copy, asked again from the byte reached until it lands or ends.
    async fn from_origin(self: &Arc<Self>, task: &Arc<Task>) -> Result<()> {
        let mut sink = Sink::open(self, task)?;
        let (mut location, mut delivery, mut relocated) = (None::<Location>, None::<Delivery>, false);
        let (mut doublings, mut wrong) = (0, None::<String>);
        let mut retry = Retry::new(self.tick, self.stats.landed.load(Ordering::Relaxed));
        // Dropped requests in a row that moved under a part; its place among the parted objects;
        // whether this origin redirects: a part learns no redirect, so one request asks it anew.
        let (mut slow, mut ranges, mut parted, mut redirects) = (0, true, None, false);
        loop {
            if self.unwanted(task) {
                return refuse(Code::TRANSFER_FAILED, "nothing wants it");
            }
            let here = match location.take().filter(|l| !l.expired()) {
                Some(here) => here,
                None => {
                    delivery = None;
                    task.enter(Phase::Origin);
                    let located = task.origin.locate(std::slice::from_ref(&task.object)).await;
                    match located.map(|mut located| located.pop()) {
                        Ok(Some(here)) => here,
                        Ok(None) => {
                            let what =
                                format!("{}: the origin located nothing", task.object.sha256);
                            return refuse(Code::REF_NOT_FOUND, what);
                        }
                        // The origin's weather is asked again, as any unanswered attempt is.
                        Err(refusal) => match Attempt::from(refusal) {
                            Attempt::Again { why, .. } => {
                                let pause = retry.again(task, &why, false, false, None)?;
                                tokio::time::sleep(pause.unwrap_or(self.tick)).await;
                                continue;
                            }
                            Attempt::Fatal(refusal) | Attempt::Refused(refusal) => {
                                return Err(refusal)
                            }
                            Attempt::Done | Attempt::Weather { .. } | Attempt::NoRanges | Attempt::Stale(_) => {
                                unreachable!("a refusal is never done")
                            }
                        },
                    }
                }
            };
            let reached = task.received();
            let from = sink.reached;
            let attempt = match &here {
                Location::File(path) => match self
                    .read_file_into(task, &mut sink, path.clone(), doublings, None)
                    .await
                {
                    FileRead::Admitted => return Ok(()),
                    FileRead::Absent => {
                        return refuse(Code::REF_NOT_FOUND, format!("{}: absent", path.display()))
                    }
                    FileRead::Corrupt(refusal) => Attempt::Fatal(refusal),
                    FileRead::Stuck(why) => Attempt::Again {
                        why,
                        wait: None,
                        dropped: true,
                        heard: true,
                    },
                    FileRead::Failed(refusal) => Attempt::Fatal(refusal),
                },
                http => {
                    let HttpLocation { url, headers, provider } = http.http().expect("a URL location");
                    delivery = delivery.filter(|(_, until)| until.is_none_or(|u| u > Instant::now()));
                    let (delivered, asked) = (
                        delivery.is_some(),
                        delivery.clone().map_or_else(|| url.to_owned(), |(at, _)| at),
                    );
                    let same = same_origin(url, &asked);
                    let headers: Vec<_> = headers.iter().filter(|(name, _)| same || name.eq_ignore_ascii_case("range")).cloned().collect();
                    let provider = if same { provider } else { None };
                    redirects |= delivered;
                    if parted.is_none() && ranges && slow >= SLOW_ASKS && task.object.length - sink.reached >= 2 * PART_BYTES {
                        parted = self.parted.try_acquire().ok();
                        if parted.is_some() {
                            tracing::info!(object = %task.object.sha256, at = sink.reached, "parts");
                        }
                    }
                    match parted.is_some() && (delivered || !redirects) {
                        true => self.parts(task, &mut sink, &asked, &headers, provider, delivered).await,
                        false => {
                            self.request(task, Target::Sink(&mut sink), &asked, &headers, provider, doublings, delivered, &mut delivery)
                                .await
                        }
                    }
                }
            };
            location = Some(here);
            match attempt {
                Attempt::Done => match sink.commit().await {
                    Ok(()) => return Ok(()),
                    Err(refusal) if refusal.code == Code::OBJECT_ID_MISMATCH => {
                        // Different wrong bytes may be the link's; the same twice are the origin's.
                        if wrong.as_ref() == Some(&refusal.detail) {
                            return Err(refusal);
                        }
                        tracing::info!(object = %task.object.sha256, %refusal, "refused, asked again");
                        wrong = Some(refusal.detail.clone());
                        sink.fresh()?;
                    }
                    Err(refusal) => return Err(refusal),
                },
                Attempt::Again { why, wait, dropped, heard } => {
                    let moved = task.received() > reached;
                    if dropped {
                        self.stats.restarts.fetch_add(1, Ordering::Relaxed);
                        doublings = if moved { 0 } else { (doublings + 1).min(MAX_DOUBLINGS) };
                        slow = if sink.reached.saturating_sub(from) < PART_BYTES { slow + 1 } else { 0 };
                        tracing::info!(object = %task.object.sha256, at = sink.reached, %why, "restart");
                    }
                    if let Some(pause) = retry.again(task, &why, heard, moved || dropped, wait)? {
                        tokio::time::sleep(pause).await;
                    }
                }
                Attempt::Weather { why, wait } => {
                    let landed = self.stats.landed.load(Ordering::Relaxed);
                    tokio::time::sleep(retry.weather(task, &why, wait, landed, self.patience.get())?).await;
                }
                Attempt::Refused(refusal) => {
                    if relocated {
                        return Err(refusal);
                    }
                    (relocated, location) = (true, None);
                }
                Attempt::NoRanges => {
                    tracing::info!(object = %task.object.sha256, "the origin ignores ranges: one GET");
                    (ranges, parted) = (false, None);
                }
                Attempt::Stale(why) => {
                    delivery = None;
                    if let Some(pause) = retry.again(task, &why, true, false, None)? {
                        tokio::time::sleep(pause).await;
                    }
                }
                Attempt::Fatal(refusal) => return Err(refusal),
            }
        }
    }

    /// What `sink` lacks, as ranged parts: `PARTS` at once, into the sink in order. Any answer a
    /// part cannot take itself ends them all, for the caller to act on.
    async fn parts(
        &self,
        task: &Arc<Task>,
        sink: &mut Sink<'_>,
        url: &str,
        headers: &[(String, String)],
        provider: Option<&dyn TokenProvider>,
        delivered: bool,
    ) -> Attempt {
        let length = task.object.length;
        let began = Instant::now();
        let (mut next, from) = (sink.reached, sink.reached);
        let mut ahead = BTreeMap::<u64, Vec<u8>>::new();
        let mut flying = FuturesUnordered::new();
        loop {
            while flying.len() < PARTS && next < length && next < sink.reached + PARTS_AHEAD * PART_BYTES {
                let end = (next + PART_BYTES).min(length);
                flying.push(self.part(task, url, headers, provider, delivered, next, end));
                next = end;
            }
            let Some((at, got)) = flying.next().await else {
                break;
            };
            match got {
                Ok(bytes) => {
                    ahead.insert(at, bytes);
                    while let Some(bytes) = ahead.remove(&sink.reached) {
                        if let Err(refusal) = sink.deliver(&bytes).await {
                            return refusal.into();
                        }
                    }
                }
                Err(attempt) => return attempt,
            }
        }
        let seconds = began.elapsed().as_secs_f64();
        let moved = sink.reached - from;
        tracing::info!(object = %task.object.sha256, moved, seconds, kbps = moved as f64 / seconds.max(1e-3) / 1e3, "parts landed");
        Attempt::Done
    }

    /// One part, asked again from the byte reached until it is whole; its offset and its bytes,
    /// or what it cannot deal with.
    #[allow(clippy::too_many_arguments)]
    async fn part(
        &self,
        task: &Arc<Task>,
        url: &str,
        headers: &[(String, String)],
        provider: Option<&dyn TokenProvider>,
        delivered: bool,
        at: u64,
        end: u64,
    ) -> (u64, std::result::Result<Vec<u8>, Attempt>) {
        let mut part = Part { reached: at, end, bytes: Vec::with_capacity((end - at) as usize) };
        let mut retry = Retry::new(self.tick, self.stats.landed.load(Ordering::Relaxed));
        let mut doublings = PART_DOUBLINGS;
        // A part never takes a new delivery: `Attempt::Stale` sends the object back to the origin.
        let mut redirected = None;
        loop {
            let reached = part.reached;
            let attempt = self.request(task, Target::Part(&mut part), url, headers, provider, doublings, delivered, &mut redirected).await;
            let pause = match attempt {
                Attempt::Done => return (at, Ok(part.bytes)),
                Attempt::Again { why, wait, dropped, heard } => {
                    let moved = part.reached > reached;
                    if dropped {
                        self.stats.restarts.fetch_add(1, Ordering::Relaxed);
                        doublings = if moved { PART_DOUBLINGS } else { (doublings + 1).min(MAX_DOUBLINGS) };
                    }
                    retry.again(task, &why, heard, moved || dropped, wait).map(|pause| pause.unwrap_or_default())
                }
                Attempt::Weather { why, wait } => {
                    retry.weather(task, &why, wait, self.stats.landed.load(Ordering::Relaxed), self.patience.get())
                }
                other => return (at, Err(other)),
            };
            match pause {
                Ok(pause) => tokio::time::sleep(pause).await,
                Err(refusal) => return (at, Err(Attempt::Fatal(refusal))),
            }
        }
    }

    /// `task` reached `reached` bytes: what is past its earlier best is progress.
    fn advance(&self, task: &Task, reached: u64) {
        let before = task.reached.fetch_max(reached, Ordering::AcqRel);
        if reached > before {
            self.stats
                .landed
                .fetch_add(reached - before, Ordering::Relaxed);
            let _ = task.phase.compare_exchange(Phase::Origin as u8, Phase::Receiving as u8, Ordering::AcqRel, Ordering::Acquire);
            task.moved_ms.store(unix_ms(), Ordering::Release);
        }
    }

    /// `bytes` of a part of `task` arrived: progress, though they are not yet in order.
    fn gained(&self, task: &Task, bytes: u64) {
        task.reached.fetch_add(bytes, Ordering::AcqRel);
        self.stats.landed.fetch_add(bytes, Ordering::Relaxed);
        let _ = task.phase.compare_exchange(Phase::Origin as u8, Phase::Receiving as u8, Ordering::AcqRel, Ordering::Acquire);
        task.moved_ms.store(unix_ms(), Ordering::Release);
    }

    fn fly(&self, task: &Arc<Task>, file: bool, part: bool, doublings: u32) -> Flying<'_> {
        let flight = Arc::new(Flight {
            task: Arc::clone(task),
            file,
            part,
            began: Instant::now(),
            doublings,
            answered: AtomicBool::new(false),
            bytes: AtomicU64::new(0),
            moved: AtomicU64::new(0),
            local: AtomicBool::new(false),
            stretch: Mutex::default(),
            window: Mutex::default(),
            dropped: Mutex::new(None),
            abort: Notify::new(),
        });
        lock(&self.flights).push(Arc::clone(&flight));
        Flying {
            shared: self,
            flight,
        }
    }

    fn dropped(flight: &Flight) -> Attempt {
        match flight.why() {
            Dropped::Judged(why) => Attempt::Again {
                why,
                wait: None,
                dropped: true,
                heard: true,
            },
            Dropped::Unwanted => Attempt::Fatal(Refusal {
                code: Code::TRANSFER_FAILED,
                detail: "nothing wants it".into(),
            }),
        }
    }

    /// One GET of what `into` still lacks. `delivered`: `url` is where the origin redirected an
    /// earlier ask, which a refusal sends back to the origin.
    #[allow(clippy::too_many_arguments)]
    async fn request(
        &self,
        task: &Arc<Task>,
        mut into: Target<'_, '_>,
        url: &str,
        headers: &[(String, String)],
        provider: Option<&dyn TokenProvider>,
        doublings: u32,
        delivered: bool,
        delivery: &mut Option<Delivery>,
    ) -> Attempt {
        let object = &task.object;
        let (from, end) = (into.reached(), into.end(object.length));
        // Nothing left to ask (an answer whose last bytes came but whose end did not): a range
        // past the end is refused (R2: 400 for `bytes=N-(N-1)`), so what is here is committed.
        if from == end {
            return Attempt::Done;
        }
        let flying = self.fly(task, false, matches!(into, Target::Part(_)), doublings);
        let flight = &flying.flight;
        self.stats.requests.fetch_add(1, Ordering::Relaxed);
        let mut headers = headers.to_vec();
        let range = (from > 0 || end < object.length).then(|| format!("bytes={from}-{}", end - 1));
        if let Some(range) = &range {
            headers.push(("range".into(), range.clone()));
        }
        let answer = tokio::select! {
            answer = self.client.get_authorized(url, &headers, provider) => answer,
            () = flight.aborted() => return Shared::dropped(flight),
        };
        let mut answer = match answer {
            Ok(answer) => answer,
            Err(GetError::Caller(refusal)) => return Attempt::Fatal(refusal),
            Err(GetError::Exchange(refusal)) => {
                if refusal.code == Code::FD_HEADROOM {
                    self.backoff();
                }
                return refusal.into();
            }
        };
        let head = flight.began.elapsed();
        self.patience.saw(head);
        push(&mut lock(&self.measured).heads, head.as_secs_f64());
        flight.answered.store(true, Ordering::Release);
        flight.moved();
        flight.stretch_begins();
        if answer.url != url {
            *delivery = Some((answer.url.clone(), answer.until));
        }
        let status = answer.status();
        let what = || format!("{}: {status} from {url}", object.sha256);
        match status {
            200 => {
                match &mut into {
                    Target::Part(_) => return Attempt::NoRanges,
                    // The origin ignored the range: the object starts over from this answer.
                    Target::Sink(sink) if from > 0 => {
                        if let Err(refusal) = sink.fresh() {
                            return Attempt::Fatal(refusal);
                        }
                    }
                    Target::Sink(_) => {}
                }
                let length = answer
                    .header("content-length")
                    .and_then(|v| v.parse::<u64>().ok());
                if length.is_some_and(|length| length != object.length) {
                    return Attempt::Fatal(Refusal {
                        code: Code::LENGTH_MISMATCH,
                        detail: format!("{}: {length:?} B, not {}", what(), object.length),
                    });
                }
            }
            206 => {
                let range = answer.content_range();
                if !range.is_some_and(|(first, total)| first == from && total.is_none_or(|t| t == object.length)) {
                    return Attempt::Fatal(Refusal {
                        code: Code::LENGTH_MISMATCH,
                        detail: format!("{}: content-range {range:?} for byte {from}", what()),
                    });
                }
            }
            status => {
                // The answer's body says why (S3 and R2: `<Error><Code>…`), with the range asked.
                let wait = answer.stated_wait();
                let said = said(&mut answer, flight).await;
                let why = format!("{}; range {}; said {said:?}", what(), range.as_deref().unwrap_or("none"));
                return match status {
                    401 | 403 | 404 | 410 if delivered => Attempt::Stale(why),
                    401 | 403 => Attempt::Refused(Refusal { code: Code::CREDENTIAL_REQUIRED, detail: why }),
                    404 | 410 => Attempt::Fatal(Refusal { code: Code::REF_NOT_FOUND, detail: why }),
                    416 => Attempt::Fatal(Refusal { code: Code::LENGTH_MISMATCH, detail: why }),
                    status if ask::weather(status) => Attempt::Weather { why, wait },
                    _ => Attempt::Fatal(Refusal { code: Code::HUB_REFUSED, detail: why }),
                };
            }
        }
        loop {
            let frame = tokio::select! {
                frame = answer.frame() => frame,
                () = flight.aborted() => return Shared::dropped(flight),
            };
            match frame {
                Ok(Some(data)) => {
                    flight.bytes.fetch_add(data.len() as u64, Ordering::AcqRel);
                    flight.moved();
                    if let Err(refusal) = into.push(self, task, flight, &data).await {
                        return refusal.into();
                    }
                }
                Ok(None) => {
                    flight.stretch_ends(&self.measured);
                    break;
                }
                Err(refusal) => {
                    let why = format!(
                        "{}: {} at {} of {} B",
                        what(),
                        refusal.detail,
                        into.reached(),
                        object.length
                    );
                    return Attempt::Again {
                        why,
                        wait: None,
                        dropped: false,
                        heard: true,
                    };
                }
            }
        }
        match into.reached() == end {
            true => Attempt::Done,
            false => Attempt::Again {
                why: format!(
                    "{}: the body ended at {} of {end} B",
                    what(),
                    into.reached(),
                ),
                wait: None,
                dropped: false,
                heard: true,
            },
        }
    }

    /// `object` from a volume, judged by its silence.
    async fn read_file(
        &self,
        task: &Arc<Task>,
        volume: &VolumeOrigin,
        path: PathBuf,
        doublings: u32,
    ) -> FileRead {
        let mut sink = match Sink::open(self, task) {
            Ok(sink) => sink,
            Err(refusal) => return FileRead::Failed(refusal),
        };
        self.read_file_into(task, &mut sink, path, doublings, Some(volume))
            .await
    }

    async fn read_file_into(
        &self,
        task: &Arc<Task>,
        sink: &mut Sink<'_>,
        path: PathBuf,
        doublings: u32,
        volume: Option<&VolumeOrigin>,
    ) -> FileRead {
        let flying = self.fly(task, true, false, doublings);
        let flight = &flying.flight;
        if sink.reached > 0 {
            if let Err(refusal) = sink.fresh() {
                return FileRead::Failed(refusal);
            }
        }
        let length = task.object.length;
        let opened = blocking(
            volume,
            move || -> std::io::Result<std::fs::File> {
                let file = std::fs::File::open(&path)?;
                let metadata = file.metadata()?;
                match metadata.is_file() && metadata.len() == length {
                    true => Ok(file),
                    false => Err(std::io::ErrorKind::NotFound.into()),
                }
            },
            flight.aborted(),
        )
        .await;
        let mut file = match opened {
            None => return self.file_dropped(flight),
            Some(Err(_)) => return FileRead::Absent,
            Some(Ok(file)) => file,
        };
        flight.answered.store(true, Ordering::Release);
        flight.moved();
        while sink.reached < length {
            let want = (length - sink.reached).min(STEP as u64) as usize;
            let began = Instant::now();
            let step = blocking(
                volume,
                move || {
                    let mut buffer = vec![0; want];
                    let read = file.read_exact(&mut buffer);
                    (file, buffer, read)
                },
                flight.aborted(),
            )
            .await;
            let Some((back, buffer, read)) = step else {
                return self.file_dropped(flight);
            };
            if read.is_err() {
                return FileRead::Absent;
            }
            file = back;
            {
                let mut measured = lock(&self.measured);
                measured.file_step = measured.file_step.max(began.elapsed());
            }
            flight
                .bytes
                .fetch_add(buffer.len() as u64, Ordering::AcqRel);
            flight.moved();
            if let Err(refusal) = sink.push(flight, &buffer).await {
                return FileRead::Failed(refusal);
            }
        }
        match sink.commit().await {
            Ok(()) => FileRead::Admitted,
            Err(refusal) if refusal.code == Code::OBJECT_ID_MISMATCH => FileRead::Corrupt(refusal),
            Err(refusal) => FileRead::Failed(refusal),
        }
    }

    fn file_dropped(&self, flight: &Flight) -> FileRead {
        match flight.why() {
            Dropped::Judged(why) => FileRead::Stuck(why),
            Dropped::Unwanted => FileRead::Failed(Refusal {
                code: Code::TRANSFER_FAILED,
                detail: "nothing wants it".into(),
            }),
        }
    }

    /// A public object onto the volume, behind everything else.
    fn write_back(self: &Arc<Self>, object: ObjectRef, replace: bool) {
        let Some(volume) = self.volume.clone() else {
            return;
        };
        self.busy.fetch_add(1, Ordering::AcqRel);
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            if let Ok(_writing) = shared.write_backs.acquire().await {
                if volume.answering() {
                    let store = Arc::clone(&shared.store);
                    let wrote = tokio::task::spawn_blocking(move || {
                        volume.write(&*store, &object, replace)
                    })
                    .await;
                    match wrote {
                        Ok(Wrote::Written) => {
                            shared.stats.volume_written.fetch_add(1, Ordering::Relaxed)
                        }
                        Ok(Wrote::Skipped) => {
                            shared.stats.volume_skipped.fetch_add(1, Ordering::Relaxed)
                        }
                        _ => 0,
                    };
                }
            }
            shared.unbusy();
        });
    }

    /// The descriptor table is full: half the workers go.
    fn backoff(&self) {
        let mut pace = lock(&self.pace);
        let gone = pace.level / 2;
        if gone == 0 {
            return;
        }
        (pace.level, pace.settled, pace.last) = (pace.level - gone, true, None);
        let forgotten = self.permits.forget_permits(gone);
        pace.debt += gone - forgotten;
        tracing::info!(streams = pace.level, "descriptor table full: fewer streams");
    }

    /// Samples and judges every request in flight, each tick, and paces the workers.
    pub async fn watch(self: Arc<Self>) {
        let (mut ticks, mut landed) = (0u32, 0u64);
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.tick) => {}
                () = self.closed() => return,
            }
            self.judge();
            ticks += 1;
            if ticks.is_multiple_of(PACE_TICKS) {
                let now = self.stats.landed.load(Ordering::Relaxed);
                let goodput = (now - landed) as f64 / (self.tick * PACE_TICKS).as_secs_f64();
                landed = now;
                self.pace(goodput);
            }
        }
    }

    fn pace(&self, goodput: f64) {
        let saturated = self.permits.available_permits() == 0 && self.waiting();
        let mut pace = lock(&self.pace);
        if self.active.load(Ordering::Acquire) == 0 && !self.waiting() {
            (pace.last, pace.settled) = (None, false);
            return;
        }
        if !saturated || pace.settled || pace.level >= pace.max || pace.debt > 0 {
            return;
        }
        if pace.last.is_some_and(|last| goodput < last * PACE_GAIN) {
            pace.settled = true;
            return;
        }
        let grow = (pace.level * 2).min(pace.max) - pace.level;
        (pace.last, pace.level) = (Some(goodput), pace.level + grow);
        self.permits.add_permits(grow);
        tracing::debug!(streams = pace.level, goodput, "more streams");
    }

    fn judge(&self) {
        let flights: Vec<Arc<Flight>> = lock(&self.flights).clone();
        let tick = self.tick.as_secs_f64();
        for flight in flights.iter().filter(|f| {
            !f.file && f.answered.load(Ordering::Acquire) && !f.local.load(Ordering::Acquire)
        }) {
            let mut window = lock(&flight.window);
            window.push_back(flight.bytes.load(Ordering::Acquire));
            if window.len() > (JUDGE_TICKS << flight.doublings) + 1 {
                window.pop_front();
            }
        }
        let measured = lock(&self.measured);
        let floor = self.tick * HEAD_TICKS;
        let head = median(&measured.heads).map_or(floor, |h| {
            floor.max(Duration::from_secs_f64(h * STALL_FACTOR))
        });
        let reference = (measured.rates.len() >= MIN_SAMPLES)
            .then(|| median(&measured.rates))
            .flatten();
        let file_patience = floor.max(measured.file_step.mul_f64(STALL_FACTOR));
        drop(measured);
        for flight in flights {
            if lock(&flight.dropped).is_some() {
                continue;
            }
            if self.unwanted(&flight.task) {
                flight.drop_with(Dropped::Unwanted);
                continue;
            }
            if flight.local.load(Ordering::Acquire) {
                continue;
            }
            let scale = 1u32 << flight.doublings;
            let silent = flight.silent();
            let judged =
                |why: String| Dropped::Judged(format!("{}: {why}", flight.task.object.sha256));
            if flight.file {
                if silent > file_patience * scale {
                    flight.drop_with(judged(format!(
                        "the volume was silent {:.1} s against {:.1} s",
                        silent.as_secs_f64(),
                        (file_patience * scale).as_secs_f64()
                    )));
                }
                continue;
            }
            if !flight.answered.load(Ordering::Acquire) {
                let waited = flight.began.elapsed();
                if waited > head * scale {
                    flight.drop_with(judged(format!(
                        "no answer after {:.2} s against the {:.2} s answers take here",
                        waited.as_secs_f64(),
                        (head * scale).as_secs_f64()
                    )));
                }
                continue;
            }
            let window = lock(&flight.window).clone();
            let full = (JUDGE_TICKS << flight.doublings) + 1;
            let elapsed = flight.began.elapsed();
            let average = flight.bytes.load(Ordering::Acquire) as f64 / elapsed.as_secs_f64().max(1e-3);
            match reference.filter(|_| !flight.part) {
                Some(reference) if window.len() >= full => {
                    let rate =
                        (window[window.len() - 1] - window[0]) as f64 / (tick * (full - 1) as f64);
                    if rate * STALL_FACTOR < reference {
                        flight.drop_with(judged(format!(
                            "{:.0} KB/s against the {:.0} KB/s requests move here",
                            rate / 1e3,
                            reference / 1e3
                        )));
                    }
                }
                None if silent > head * scale => {
                    flight.drop_with(judged(format!(
                        "silent {:.2} s against {:.2} s",
                        silent.as_secs_f64(),
                        (head * scale).as_secs_f64()
                    )));
                }
                None if !flight.part && reference.is_none() && elapsed > floor * 4 && average < CRAWL_BYTES_PER_S => {
                    flight.drop_with(judged(format!("{:.0} KB/s, with nothing yet to compare it to", average / 1e3)));
                }
                _ => {}
            }
        }
    }
}
