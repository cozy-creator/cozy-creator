//! The downloader, as S3's own clients do it (aws-cli, CRT, s5cmd): a queue of objects, each read as
//! 16 MiB ranges (one request for an object of 16 MiB or less), one limit on requests in flight across
//! every object, and each request asked again by the one retry rule ([`ask`]) from the byte it reached.
//! A range is written at its offset in the object's one file, as aws-cli writes parts; nothing is joined.
//! A request place left free with nothing waiting takes the back half of the in-flight range with the most
//! left (aria2's dynamic split, BitTorrent's end game), so a dripping object's tail gets many requests.
//!
//! - **Order.** The queue is a plain number, smaller first; equal numbers go in arrival order. An
//!   object wanted twice is downloaded once. Requests take their places in line as their object
//!   starts, so an object's ranges run together. A task nothing wants stops (cl-686).
//! - **Tiers.** The local store, then the volume (when one is bound), then the origin. Every byte
//!   enters the store's admission, which hashes it: wrong bytes are never admitted. A process sharing
//!   the store that is fetching an object is waited for, not raced (tfs-142).
//! - **Cut or killed.** What landed stays in the store's `tmp/` ([`Store::partial`]), and the next
//!   fetch asks only for the rest (tfs-143).
//! - **Every answer that is not 2xx is logged** with its object, range and attempt, and counted.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{mpsc, oneshot, watch, Notify, Semaphore};

use super::ask::{self, Backoff, ATTEMPTS};
use super::net::{shown, Client, GetError};
use super::origin::{HttpLocation, Location, Origin};
use super::volume::{blocking, VolumeOrigin, Wrote, STEP};
use super::Tier;
use crate::store::{Partial, Store, Stretch};
use crate::err::{refuse, Code, Refusal, Result};
use crate::format::ObjectRef;

/// A range: 16 MiB. A request to R2 for never-read bytes pays a fixed start, so 4 MiB ranges moved
/// 3-4 MB/s each and 8 MiB about 7.5 (11-04). A dripping one is shared out by stealing (`STEAL`).
const PART: u64 = 16 << 20;
/// Bytes a request gathers before writing them: the most a cut loses per request.
const WRITE_BYTES: usize = 256 << 10;
const VOLUME_READS: usize = 8;
const WRITE_BACKS: usize = 2;
/// A volume read that takes no step for this long leaves the volume out (the network's stall rule).
const VOLUME_STALL: Duration = Duration::from_secs(30);
/// How often an object another process is fetching is looked at again.
const CLAIM_POLL: Duration = Duration::from_millis(250);
/// A free place with nothing waiting takes the back half of the in-flight range with the most left,
/// while that half is at least `STEAL` (aria2's dynamic split, BitTorrent's end game).
const STEAL: u64 = 512 << 10;
/// The ramp (AWS CRT's): requests in flight start at `START` and double each `WINDOW` while the bytes
/// received grow by `GAIN`, up to the cap; an error or a timeout halves them, once a window, never
/// below `START`, and the ramp probes again.
const START: usize = 8;
const WINDOW: Duration = Duration::from_millis(500);
const GAIN: f64 = 1.1;

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
    /// Bytes here: those an earlier fetch left, and those arrived since.
    received: AtomicU64,
    /// Those an earlier fetch had left, which this one did not fetch.
    kept: AtomicU64,
    /// Where it is (`Phase`), and when it last moved: its phase changed or its bytes grew (Unix ms).
    phase: AtomicU8,
    moved_ms: AtomicU64,
    /// Its requests, and the attempts in a row that moved nothing.
    asked: AtomicU32,
    unanswered: AtomicU32,
    /// Its slowest range: how long it took, and its bytes.
    slowest: Mutex<(Duration, u64)>,
    /// Where the origin redirected it, and until when (the redirect's `Cache-Control: max-age`): its
    /// other ranges ask there directly, as an HTTP cache would.
    delivery: Mutex<Option<(String, Instant)>>,
    /// One request asks the origin where the object is; its other ranges wait for that answer's head.
    locating: AtomicBool,
    located: watch::Sender<bool>,
    /// Its last waiter left.
    left: Notify,
    done: watch::Sender<Option<std::result::Result<Tier, Refusal>>>,
}

impl Task {
    pub(crate) fn received(&self) -> u64 {
        self.received.load(Ordering::Acquire).min(self.object.length)
    }

    pub(crate) fn kept(&self) -> u64 {
        self.kept.load(Ordering::Acquire)
    }

    fn enter(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
        self.moved_ms.store(unix_ms(), Ordering::Release);
    }

    fn gained(&self, bytes: u64) {
        self.received.fetch_add(bytes, Ordering::AcqRel);
        let _ = self.phase.compare_exchange(Phase::Origin as u8, Phase::Receiving as u8, Ordering::AcqRel, Ordering::Acquire);
        self.moved_ms.store(unix_ms(), Ordering::Release);
    }
}

/// Where an object's task is: waiting for a request, looking here and at the datacenter volume,
/// reading the volume, asking the origin, or receiving the origin's bytes.
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
/// of its length, when it last moved (Unix ms), how many wait for it, and the attempts in a row
/// that moved nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetching {
    pub phase: Phase,
    pub reached: u64,
    pub length: u64,
    pub moved_unix_ms: u64,
    pub waiters: usize,
    pub unanswered: u32,
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
    pub(crate) fn task(&self) -> Arc<Task> {
        Arc::clone(&self.task)
    }

    pub(crate) async fn wait(mut self) -> Result<Tier> {
        let answer = self.answer.wait_for(Option::is_some).await;
        match answer.as_deref() {
            Ok(Some(answer)) => answer.clone(),
            _ => refuse(Code::TRANSFER_FAILED, format!("{}: the fetcher closed", self.task.object.sha256)),
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if self.task.waiters.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.task.left.notify_one();
        }
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

/// What the Fetcher shares with its dispatcher and transfers.
pub(crate) struct Shared {
    pub store: Arc<Store>,
    pub client: Client,
    pub volume: Option<VolumeOrigin>,
    queue: Mutex<Queue>,
    queued: Notify,
    closed: AtomicBool,
    closing: Notify,
    /// Requests in flight, across every object, as many as the ramp allows (`Pace`).
    slots: Arc<Slots>,
    pace: Mutex<Pace>,
    /// Ranges in flight, for free places to take from.
    flights: Mutex<Vec<Arc<Flight>>>,
    /// Tasks and write-backs not yet ended.
    busy: AtomicUsize,
    settle: Notify,
    volume_reads: Semaphore,
    write_backs: Semaphore,
    pub stats: Counters,
}

#[derive(Default)]
pub(crate) struct Counters {
    pub requests: AtomicU64,
    pub retries: AtomicU64,
    /// Back halves taken from ranges in flight by free places.
    pub steals: AtomicU64,
    /// Bytes received from the origin, and the pacing windows (in ms) any of them arrived in: the
    /// time the machine was pulling, with no wait in a queue or between pulls in it.
    pub received: AtomicU64,
    pub receiving_ms: AtomicU64,
    pub volume_written: AtomicU64,
    pub volume_skipped: AtomicU64,
    /// Answers that were not 2xx, by status.
    pub statuses: Mutex<BTreeMap<u16, u64>>,
}

/// How one request ended short of its range.
enum Failed {
    /// Ask again (after the answer's stated wait, when it gave one).
    Again(String, Option<Duration>),
    Fatal(Refusal),
}

impl From<Refusal> for Failed {
    fn from(refusal: Refusal) -> Failed {
        Failed::Fatal(refusal)
    }
}

/// How a file read ended.
enum FileRead {
    /// Every byte written, for commit to judge.
    Done,
    Absent,
    /// Abandoned as silent; the operation may still be blocked.
    Stuck(String),
    Failed(Refusal),
}

/// A range in flight as its stealers see it: the bytes it reached and the byte it ends at, which a
/// steal moves back. The object fetches what was taken (`ranges`).
struct Flight {
    span: Mutex<(u64, u64)>,
    taken: Option<mpsc::UnboundedSender<(u64, u64, Slot)>>,
}

/// One range of an object, its bytes written at their offsets as they arrive (`WRITE_BYTES` at a time,
/// off the runtime), registered among the ranges in flight while it lives.
struct Part<'s> {
    shared: &'s Shared,
    flight: Arc<Flight>,
    /// Bytes received; those before `written` are in the store, the rest in `buffer`.
    reached: u64,
    written: u64,
    writer: Option<Stretch>,
    buffer: Vec<u8>,
}

impl<'s> Part<'s> {
    /// A range of `[from, end)`; with `taken`, one free places may take from (a request's, not a file's).
    fn new(shared: &'s Shared, admission: &Partial, from: u64, end: u64, taken: Option<mpsc::UnboundedSender<(u64, u64, Slot)>>) -> Result<Part<'s>> {
        let stealable = taken.is_some();
        let flight = Arc::new(Flight { span: Mutex::new((from, end)), taken });
        if stealable {
            lock(&shared.flights).push(Arc::clone(&flight));
        }
        Ok(Part { shared, flight, reached: from, written: from, writer: Some(admission.writer()?), buffer: Vec::new() })
    }

    fn end(&self) -> u64 {
        lock(&self.flight.span).1
    }

    /// `data` onto the range, up to its end, which a steal may have moved back: whether it wants more.
    async fn push(&mut self, task: &Task, data: &[u8]) -> Result<bool> {
        let (take, end) = {
            let mut span = lock(&self.flight.span);
            let take = (span.1.saturating_sub(span.0) as usize).min(data.len());
            span.0 += take as u64;
            (take, span.1)
        };
        self.buffer.extend_from_slice(&data[..take]);
        self.reached += take as u64;
        task.gained(take as u64);
        if self.buffer.len() >= WRITE_BYTES {
            self.flush().await?;
        }
        Ok(self.reached < end)
    }

    async fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let (Some(mut writer), bytes, at) = (self.writer.take(), std::mem::take(&mut self.buffer), self.written) else {
            return refuse(Code::IO_FAILED, "the range's writer is gone");
        };
        let written = tokio::task::spawn_blocking(move || {
            let written = writer.write_at(at, &bytes);
            (writer, bytes, written)
        })
        .await;
        let (writer, mut bytes, written) = written.map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })?;
        written?;
        self.written += bytes.len() as u64;
        bytes.clear();
        (self.writer, self.buffer) = (Some(writer), bytes);
        Ok(())
    }
}

impl Drop for Part<'_> {
    fn drop(&mut self) {
        lock(&self.shared.flights).retain(|flight| !Arc::ptr_eq(flight, &self.flight));
    }
}

/// `[from, end)` stretches of `length` not in `held`, cut at multiples of `PART`.
fn lacking(held: &[(u64, u64)], length: u64) -> Vec<(u64, u64)> {
    let mut gaps = Vec::new();
    let mut at = 0;
    for &(a, b) in held.iter().chain(std::iter::once(&(length, length))) {
        let mut from = at;
        while from < a {
            let end = ((from / PART + 1) * PART).min(a);
            gaps.push((from, end));
            from = end;
        }
        at = at.max(b);
    }
    gaps
}

#[derive(Default)]
struct Pace {
    level: usize,
    max: usize,
    /// The rate at the last doubling, and the bytes received when it last looked.
    last: Option<f64>,
    seen: u64,
    /// Windows in a row since the last doubling that grew under `GAIN`.
    flat: u32,
    settled: bool,
    /// Nothing waits, and every place is taking from ranges in flight: a new probe.
    tail: bool,
    halved: Option<Instant>,
}

/// The places for requests in flight: as many as the ramp allows, each given back as it drops, in the
/// order asked.
struct Slots(Mutex<Places>);

#[derive(Default)]
struct Places {
    free: usize,
    /// Places the ramp gave up that are still out: each is kept as it comes back.
    owed: usize,
    waiting: VecDeque<oneshot::Sender<Slot>>,
}

/// A place among the requests in flight.
struct Slot(Arc<Slots>);

impl Slots {
    fn new(free: usize) -> Arc<Slots> {
        Arc::new(Slots(Mutex::new(Places { free, ..Places::default() })))
    }

    async fn take(self: &Arc<Self>) -> Slot {
        let waiting = {
            let mut places = lock(&self.0);
            if places.free > 0 {
                places.free -= 1;
                return Slot(Arc::clone(self));
            }
            let (sent, waiting) = oneshot::channel();
            places.waiting.push_back(sent);
            waiting
        };
        waiting.await.expect("a place's sender lives as long as its queue")
    }

    /// A free place, when nothing waits for one.
    fn idle(self: &Arc<Self>) -> Option<Slot> {
        let mut places = lock(&self.0);
        places.waiting.retain(|waiter| !waiter.is_closed());
        (places.free > 0 && places.waiting.is_empty()).then(|| {
            places.free -= 1;
            Slot(Arc::clone(self))
        })
    }

    /// A place back: owed to the ramp, else to the first waiter still waiting, else free.
    fn give(self: &Arc<Self>, places: &mut Places) {
        if places.owed > 0 {
            places.owed -= 1;
            return;
        }
        while let Some(waiter) = places.waiting.pop_front() {
            match waiter.send(Slot(Arc::clone(self))) {
                Ok(()) => return,
                // Its waiter left: the same place goes on to the next.
                Err(slot) => std::mem::forget(slot),
            }
        }
        places.free += 1;
    }

    fn add(self: &Arc<Self>, more: usize) {
        let mut places = lock(&self.0);
        (0..more).for_each(|_| self.give(&mut places));
    }

    fn forget(&self, gone: usize) {
        let mut places = lock(&self.0);
        let now = gone.min(places.free);
        places.free -= now;
        places.owed += gone - now;
    }

    /// Every place taken.
    fn busy(&self) -> bool {
        lock(&self.0).free == 0
    }

    /// Every place taken, with requests waiting for one.
    fn full(&self) -> bool {
        let places = lock(&self.0);
        places.free == 0 && !places.waiting.is_empty()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let slots = Arc::clone(&self.0);
        slots.give(&mut lock(&slots.0));
    }
}

impl Shared {
    pub(crate) fn new(store: Arc<Store>, client: Client, volume: Option<VolumeOrigin>, requests: usize) -> Shared {
        Shared {
            store,
            client,
            volume,
            queue: Mutex::default(),
            queued: Notify::new(),
            closed: AtomicBool::new(false),
            closing: Notify::new(),
            slots: Slots::new(START.min(requests.max(1))),
            flights: Mutex::default(),
            pace: Mutex::new(Pace { level: START.min(requests.max(1)), max: crate::descriptors::requests(requests.max(1)), ..Pace::default() }),
            busy: AtomicUsize::new(0),
            settle: Notify::new(),
            volume_reads: Semaphore::new(VOLUME_READS),
            write_backs: Semaphore::new(WRITE_BACKS),
            stats: Counters::default(),
        }
    }

    /// Stops the dispatcher and every transfer.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.closing.notify_waiters();
        self.queued.notify_waiters();
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
    pub(crate) async fn settled(&self) {
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
            let statuses = lock(&self.stats.statuses).clone();
            let (requests, retries, steals) = (self.stats.requests.load(Ordering::Relaxed), self.stats.retries.load(Ordering::Relaxed), self.stats.steals.load(Ordering::Relaxed));
            tracing::info!(requests, retries, steals, ?statuses, "downloads settled (counts since start)");
            self.settle.notify_waiters();
        }
    }

    /// The ramp, once a window: more requests while the bytes received grow, back to `START` when idle.
    pub(crate) async fn pace(self: Arc<Self>) {
        loop {
            tokio::select! {
                () = tokio::time::sleep(WINDOW) => {}
                () = self.closed() => return,
            }
            self.steal();
            let received = self.stats.received.load(Ordering::Relaxed);
            let mut pace = lock(&self.pace);
            let rate = (received - pace.seen) as f64 / WINDOW.as_secs_f64();
            if received > pace.seen {
                self.stats.receiving_ms.fetch_add(WINDOW.as_millis() as u64, Ordering::Relaxed);
            }
            pace.seen = received;
            if self.busy.load(Ordering::Acquire) == 0 {
                let gone = pace.level.saturating_sub(START.min(pace.max));
                self.shrink(&mut pace, gone);
                (pace.last, pace.settled, pace.flat) = (None, false, 0);
                continue;
            }
            // Demand is requests waiting, or ranges in flight with halves left to take; the tail is a new
            // phase, probed afresh from its own rate.
            let (waiting, stealable) = (self.slots.full(), self.stealable());
            let tail = !waiting && stealable && self.slots.busy();
            if tail && !pace.tail {
                (pace.last, pace.settled, pace.flat) = (None, false, 0);
            }
            pace.tail = tail;
            if !(waiting || tail) || pace.settled || pace.level >= pace.max {
                continue;
            }
            // A doubling's new requests take more than a window to deliver: two flat windows settle it.
            if pace.last.is_some_and(|last| rate < last * GAIN) {
                pace.flat += 1;
                if pace.flat >= 2 {
                    pace.settled = true;
                    tracing::info!(streams = pace.level, mb_s = rate / 1e6, "streams settled");
                }
                continue;
            }
            pace.flat = 0;
            let grow = (pace.level * 2).min(pace.max) - pace.level;
            (pace.level, pace.last) = (pace.level + grow, Some(rate));
            self.slots.add(grow);
            tracing::info!(streams = pace.level, mb_s = rate / 1e6, "more streams");
        }
    }

    /// Each free place with nothing waiting takes the back half of the in-flight range with the most
    /// left, while that half is at least `STEAL`; the range's own request stops where the half begins.
    fn steal(&self) {
        while let Some(slot) = self.slots.idle() {
            let flights = lock(&self.flights).clone();
            let most = flights.iter().max_by_key(|flight| {
                let span = lock(&flight.span);
                span.1.saturating_sub(span.0)
            });
            let Some(flight) = most else { return };
            let mut span = lock(&flight.span);
            let left = span.1.saturating_sub(span.0);
            if left < 2 * STEAL {
                return;
            }
            let (half, end) = (span.0 + left / 2, span.1);
            span.1 = half;
            // Sent while the span is held: its range can't end and its object can't finish before.
            let Some(taken) = &flight.taken else { return };
            if let Err(error) = taken.send((half, end, slot)) {
                span.1 = end;
                drop(span);
                drop(error);
                return;
            }
            self.stats.steals.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(from = half, end, "a free place took the back half of a range");
        }
    }

    /// Whether a range in flight has a back half left to take.
    fn stealable(&self) -> bool {
        lock(&self.flights).iter().any(|flight| {
            let span = lock(&flight.span);
            span.1.saturating_sub(span.0) >= 2 * STEAL
        })
    }

    /// An error or a timeout: half the requests go, once a window, never below `START`; the ramp then
    /// probes again (settled, a burst of 503s left it at one request for good: run 32321).
    fn congested(&self) {
        let mut pace = lock(&self.pace);
        let floor = START.min(pace.max);
        if pace.level <= floor || pace.halved.is_some_and(|at| at.elapsed() < WINDOW) {
            return;
        }
        let gone = (pace.level / 2).min(pace.level - floor);
        self.shrink(&mut pace, gone);
        (pace.halved, pace.last, pace.settled, pace.flat) = (Some(Instant::now()), None, false, 0);
        tracing::info!(streams = pace.level, "fewer streams: an error or a timeout");
    }

    fn shrink(&self, pace: &mut Pace, gone: usize) {
        pace.level -= gone;
        self.slots.forget(gone);
    }

    /// A place in line for `object`, joining its task when one is open.
    pub(crate) fn want(&self, object: &ObjectRef, origin: &Arc<dyn Origin>, priority: u64) -> Waiter {
        let mut queue = lock(&self.queue);
        let task = match queue.tasks.get(&object.sha256).cloned() {
            Some(task) => {
                task.waiters.fetch_add(1, Ordering::AcqRel);
                if priority < task.priority.load(Ordering::Acquire) && !task.started.load(Ordering::Acquire) {
                    task.priority.store(priority, Ordering::Release);
                    queue.seq += 1;
                    let seq = queue.seq;
                    queue.heap.push(Reverse((priority, seq, object.sha256.clone())));
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
                    received: AtomicU64::new(0),
                    kept: AtomicU64::new(0),
                    phase: AtomicU8::new(Phase::Queued as u8),
                    moved_ms: AtomicU64::new(unix_ms()),
                    asked: AtomicU32::new(0),
                    unanswered: AtomicU32::new(0),
                    slowest: Mutex::default(),
                    delivery: Mutex::default(),
                    locating: AtomicBool::new(false),
                    located: watch::channel(false).0,
                    left: Notify::new(),
                    done: watch::channel(None).0,
                });
                queue.seq += 1;
                let seq = queue.seq;
                queue.heap.push(Reverse((priority, seq, object.sha256.clone())));
                queue.tasks.insert(object.sha256.clone(), Arc::clone(&task));
                self.busy.fetch_add(1, Ordering::AcqRel);
                self.queued.notify_one();
                task
            }
        };
        Waiter { answer: task.done.subscribe(), task }
    }

    /// The most urgent task nobody has started that someone still wants.
    fn pop(&self) -> Option<Arc<Task>> {
        let mut queue = lock(&self.queue);
        while let Some(Reverse((priority, _, sha256))) = queue.heap.pop() {
            let Some(task) = queue.tasks.get(&sha256).cloned() else {
                continue;
            };
            if task.started.load(Ordering::Acquire) || task.priority.load(Ordering::Acquire) != priority {
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

    /// `object`'s task as its waiters see it; None when none is open (landed, failed or unwanted).
    pub(crate) fn fetching(&self, object: &ObjectRef) -> Option<Fetching> {
        let task = lock(&self.queue).tasks.get(&object.sha256).cloned()?;
        Some(Fetching {
            phase: Phase::of(task.phase.load(Ordering::Acquire)),
            reached: task.received(),
            length: task.object.length,
            moved_unix_ms: task.moved_ms.load(Ordering::Acquire),
            waiters: task.waiters.load(Ordering::Acquire),
            unanswered: task.unanswered.load(Ordering::Acquire),
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

    /// Resolves once nothing wants `task`.
    async fn abandoned(&self, task: &Arc<Task>) {
        while !self.unwanted(task) {
            task.left.notified().await;
        }
    }

    /// Starts tasks as requests come free, most urgent first: the request it was started with is its first.
    pub(crate) async fn dispatch(self: Arc<Self>) {
        loop {
            let permit = tokio::select! {
                slot = self.slots.take() => slot,
                () = self.closed() => return,
            };
            let notified = self.queued.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            // Nothing to start: the place goes back to the ranges waiting for one until a want comes.
            // Held here at the tail it starved them for good (run 32321).
            let Some(task) = self.pop() else {
                drop(permit);
                notified.await;
                continue;
            };
            let shared = Arc::clone(&self);
            tokio::spawn(async move {
                let began = Instant::now();
                let outcome = tokio::select! {
                    outcome = shared.fetch(&task, permit) => outcome,
                    () = shared.abandoned(&task) => refuse(Code::TRANSFER_FAILED, "nothing wants it"),
                    () = shared.closed() => refuse(Code::TRANSFER_FAILED, "the fetcher closed"),
                };
                match &outcome {
                    Err(refusal) => tracing::info!(object = %task.object.sha256, %refusal, "fetch refused"),
                    Ok(Tier::Local) => {}
                    Ok(tier) => {
                        let (bytes, kept, requests) = (task.object.length, task.kept(), task.asked.load(Ordering::Relaxed));
                        let (took, range) = *lock(&task.slowest);
                        let (slowest_range_s, slowest_range_kbps) = (took.as_secs_f64(), range as f64 / took.as_secs_f64().max(1e-3) / 1e3);
                        tracing::info!(object = %task.object.sha256, bytes, kept, ?tier, seconds = began.elapsed().as_secs_f64(), requests, slowest_range_s, slowest_range_kbps, "landed");
                    }
                }
                shared.finish(&task, outcome);
            });
        }
    }

    /// `task`'s object from the first tier that has it. Another process on this store fetching it
    /// now (TensorD, for an executor's read) is waited for, so it downloads once on this machine.
    async fn fetch(self: &Arc<Self>, task: &Arc<Task>, permit: Slot) -> Result<Tier> {
        if self.store.reuse(&task.object).unwrap_or(false) {
            return Ok(Tier::Local);
        }
        let _claim = loop {
            if let Some(claim) = self.store.claim(&task.object) {
                break claim;
            }
            tokio::time::sleep(CLAIM_POLL).await;
        };
        // It may have landed while another held the claim.
        if self.store.reuse(&task.object).unwrap_or(false) {
            return Ok(Tier::Local);
        }
        let admission = self.admit(task).await?;
        self.through_tiers(task, admission, permit).await
    }

    /// The object's door, with what an earlier fetch left: those bytes count as here already.
    async fn admit(&self, task: &Task) -> Result<Partial> {
        let (store, object) = (Arc::clone(&self.store), task.object.clone());
        let admission = tokio::task::spawn_blocking(move || store.partial(&object).map_err(crate::store::Error::refusal))
            .await
            .map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })??;
        let held: u64 = admission.held().iter().map(|(a, b)| b - a).sum();
        if held > 0 {
            tracing::info!(object = %task.object.sha256, held, "an earlier fetch's bytes taken up");
        }
        task.kept.store(held, Ordering::Release);
        task.received.store(held, Ordering::Release);
        Ok(admission)
    }

    /// Hashes and admits what was written: only the object's digest lets it in.
    async fn commit(admission: Partial) -> Result<()> {
        tokio::task::spawn_blocking(move || admission.commit())
            .await
            .map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })?
    }

    /// The object from the volume, else the origin.
    async fn through_tiers(self: &Arc<Self>, task: &Arc<Task>, mut admission: Partial, permit: Slot) -> Result<Tier> {
        let mut replace = false;
        if let Some(volume) = self.volume.clone().filter(VolumeOrigin::answering) {
            let _reading = self.volume_reads.acquire().await;
            if volume.answering() {
                task.enter(Phase::Volume);
                match self.read_file(task, &admission, volume.blob(&task.object), Some(&volume)).await {
                    FileRead::Done => match Shared::commit(admission).await {
                        Ok(()) => {
                            let (touching, object) = (volume.clone(), task.object.clone());
                            tokio::task::spawn_blocking(move || touching.touch(&object));
                            return Ok(Tier::Volume);
                        }
                        Err(refusal) if refusal.code == Code::OBJECT_ID_MISMATCH => {
                            tracing::info!(object = %task.object.sha256, %refusal, "volume copy refused");
                            // Bytes kept from an earlier fetch are as suspect as the copy: only a whole
                            // read of the volume blames it.
                            replace = task.kept() == 0;
                            admission = self.admit(task).await?;
                        }
                        Err(refusal) => return Err(refusal),
                    },
                    FileRead::Absent | FileRead::Stuck(_) => {}
                    FileRead::Failed(refusal) => return Err(refusal),
                }
            }
        }
        self.origin_copy(task, admission, permit).await?;
        if task.origin.public() {
            self.write_back(task.object.clone(), replace);
        }
        Ok(Tier::Origin)
    }

    /// The origin's copy. Wrong bytes are asked for once more, from nothing: different wrong bytes
    /// may be the link's, the same twice are the origin's.
    async fn origin_copy(&self, task: &Arc<Task>, mut admission: Partial, permit: Slot) -> Result<()> {
        let (mut permit, mut wrong) = (Some(permit), None::<String>);
        loop {
            task.enter(Phase::Origin);
            let located = task.origin.locate(std::slice::from_ref(&task.object)).await;
            let Some(here) = located?.pop() else {
                return refuse(Code::REF_NOT_FOUND, format!("{}: the origin located nothing", task.object.sha256));
            };
            match &here {
                Location::File(path) => match self.read_file(task, &admission, path.clone(), None).await {
                    FileRead::Done => {}
                    FileRead::Absent => return refuse(Code::REF_NOT_FOUND, format!("{}: absent", path.display())),
                    FileRead::Stuck(why) => return refuse(Code::TRANSFER_FAILED, why),
                    FileRead::Failed(refusal) => return Err(refusal),
                },
                http => self.ranges(task, &admission, &http.http().expect("a URL location"), &mut permit).await?,
            }
            match Shared::commit(admission).await {
                Ok(()) => return Ok(()),
                Err(refusal) if refusal.code == Code::OBJECT_ID_MISMATCH && wrong.as_ref() != Some(&refusal.detail) => {
                    tracing::info!(object = %task.object.sha256, %refusal, "refused, asked again");
                    wrong = Some(refusal.detail.clone());
                    admission = self.admit(task).await?;
                }
                Err(refusal) => return Err(refusal),
            }
        }
    }

    /// What the object lacks, as `PART` ranges side by side, each written at its offset, and the back
    /// halves free places take from them (`steal`).
    async fn ranges(&self, task: &Task, admission: &Partial, here: &HttpLocation<'_>, first: &mut Option<Slot>) -> Result<()> {
        let (taken, mut takes) = mpsc::unbounded_channel();
        let mut flying = FuturesUnordered::new();
        for (from, end) in lacking(&admission.held(), task.object.length) {
            flying.push(self.part(task, Part::new(self, admission, from, end, Some(taken.clone()))?, here, first.take()));
        }
        loop {
            tokio::select! {
                biased;
                Some((from, end, slot)) = takes.recv() => {
                    flying.push(self.part(task, Part::new(self, admission, from, end, Some(taken.clone()))?, here, Some(slot)));
                }
                done = flying.next() => match done {
                    Some(done) => done?,
                    None if takes.is_empty() => return Ok(()),
                    None => {}
                },
            }
        }
    }

    /// One range in a place among the requests in flight (`slot` when it has one: the task's first, or
    /// the free place that took it).
    async fn part(&self, task: &Task, mut part: Part<'_>, here: &HttpLocation<'_>, slot: Option<Slot>) -> Result<()> {
        let _slot = match slot {
            Some(slot) => slot,
            None => self.slots.take().await,
        };
        self.range(task, here, &mut part).await
    }

    /// One range, asked again by the one rule (`ask`) from the byte reached, to its end.
    async fn range(&self, task: &Task, here: &HttpLocation<'_>, part: &mut Part<'_>) -> Result<()> {
        let (mut backoff, mut attempt, mut still) = (Backoff::new(), 0, 0);
        let (began, first) = (Instant::now(), part.reached);
        loop {
            attempt += 1;
            let from = part.reached;
            let failed = self.request(task, here, part, attempt).await;
            let (why, wait, moved) = match failed {
                Ok(()) => {
                    part.flush().await?;
                    let took = began.elapsed();
                    let mut slowest = lock(&task.slowest);
                    if took > slowest.0 {
                        *slowest = (took, part.reached - first);
                    }
                    return Ok(());
                }
                Err(Failed::Fatal(refusal)) => return Err(refusal),
                Err(Failed::Again(why, wait)) => (why, wait, part.reached > from),
            };
            still = if moved { 0 } else { still + 1 };
            task.unanswered.store(still, Ordering::Release);
            self.stats.retries.fetch_add(1, Ordering::Relaxed);
            match backoff.after(moved, wait) {
                Some(pause) => tokio::time::sleep(pause).await,
                None => {
                    let detail = format!("{}: {ATTEMPTS} attempts in a row moved nothing; the last: {why}", task.object.sha256);
                    return refuse(Code::TRANSFER_FAILED, detail);
                }
            }
        }
    }

    /// One GET of what `part` still lacks, its body read to the end. With `split`, one moving under an
    /// eighth of what a request moves now after `SLOW_AFTER` is dropped, for its object to split.
    async fn request(&self, task: &Task, here: &HttpLocation<'_>, part: &mut Part<'_>, attempt: u32) -> std::result::Result<(), Failed> {
        let (object, length) = (&task.object.sha256, task.object.length);
        let (from, end) = (part.reached, part.end());
        // Nothing left to ask: a range past the end is refused (R2: 400 for `bytes=N-(N-1)`).
        if from == end {
            return Ok(());
        }
        self.stats.requests.fetch_add(1, Ordering::Relaxed);
        task.asked.fetch_add(1, Ordering::Relaxed);
        let range = (from > 0 || end < length).then(|| format!("bytes={from}-{}", end - 1));
        let shown_range = range.as_deref().unwrap_or("all");
        let ranged: Vec<_> = range.clone().map(|range| ("range".to_owned(), range)).into_iter().collect();
        // Where the origin redirected this object while that holds, with only the range (credentials stay
        // with the origin); else the origin.
        let current = || lock(&task.delivery).clone().filter(|(_, until)| *until > Instant::now());
        let mut delivered = current();
        if delivered.is_none() && task.locating.swap(true, Ordering::AcqRel) {
            let _ = task.located.subscribe().wait_for(|located| *located).await;
            delivered = current();
        }
        let asked = match &delivered {
            Some((at, _)) => self.client.get_authorized(at, &ranged, None).await,
            None => self.client.get_authorized(here.url, &[here.headers, &ranged].concat(), here.provider).await,
        };
        task.located.send_replace(true);
        let mut answer = match asked {
            Ok(answer) => {
                if let (None, Some(until)) = (&delivered, answer.until) {
                    *lock(&task.delivery) = Some((answer.url.clone(), until));
                }
                answer
            }
            Err(GetError::Exchange(refusal)) if ask::transient(&refusal) => {
                tracing::info!(object, range = shown_range, attempt, why = %refusal.detail, "request failed");
                self.congested();
                return Err(Failed::Again(refusal.detail, None));
            }
            Err(error) => return Err(Failed::Fatal(error.refusal())),
        };
        let status = answer.status();
        if !(200..300).contains(&status) {
            let (wait, said) = (answer.stated_wait(), answer.said().await);
            *lock(&self.stats.statuses).entry(status).or_default() += 1;
            tracing::warn!(object, status, range = shown_range, attempt, from = shown(&answer.url), %said, "answer not 2xx");
            let why = format!("{object}: {status} from {}; range {shown_range}; said {said:?}", shown(&answer.url));
            let code = match status {
                _ if ask::weather(status) => {
                    self.congested();
                    return Err(Failed::Again(why, wait));
                }
                // From storage the origin sent us to: the next attempt asks the origin for a fresh link.
                401 | 403 | 404 | 410 if answer.url != here.url => {
                    *lock(&task.delivery) = None;
                    return Err(Failed::Again(why, None));
                }
                401 | 403 => Code::CREDENTIAL_REQUIRED,
                404 | 410 => Code::REF_NOT_FOUND,
                416 => Code::LENGTH_MISMATCH,
                _ => Code::HUB_REFUSED,
            };
            return Err(Failed::Fatal(Refusal { code, detail: why }));
        }
        let mismatch = |detail: String| Failed::Fatal(Refusal { code: Code::LENGTH_MISMATCH, detail: format!("{object}: {detail}") });
        match status {
            206 => {
                let got = answer.content_range();
                if !got.is_some_and(|(first, total)| first == from && total.is_none_or(|t| t == length)) {
                    return Err(mismatch(format!("content-range {got:?} for byte {from}")));
                }
            }
            200 if range.is_some() => return Err(mismatch("the origin ignores ranges".into())),
            200 => {
                if let Some(sent) = answer.header("content-length").and_then(|v| v.parse::<u64>().ok()).filter(|sent| *sent != length) {
                    return Err(mismatch(format!("{sent} B, not {length}")));
                }
            }
            _ => return Err(mismatch(format!("{status} to a GET"))),
        }
        loop {
            match answer.frame().await {
                Ok(Some(data)) => {
                    self.stats.received.fetch_add(data.len() as u64, Ordering::Relaxed);
                    // A steal moved the end back: what is past it is another request's.
                    if !part.push(task, &data).await? {
                        break;
                    }
                }
                Ok(None) => break,
                Err(refusal) => {
                    let at = part.reached;
                    tracing::info!(object, range = shown_range, attempt, at, why = %refusal.detail, "body failed");
                    self.congested();
                    return Err(Failed::Again(format!("{object}: {} at {at}", refusal.detail), None));
                }
            }
        }
        let end = part.end();
        match part.reached >= end {
            true => Ok(()),
            false => Err(Failed::Again(format!("{object}: the body ended at {} of {end}", part.reached), None)),
        }
    }

    /// The object read whole from a file. A step that takes `VOLUME_STALL` is abandoned, and a
    /// volume's abandoned read keeps it out of every fetch until it returns.
    async fn read_file(&self, task: &Task, admission: &Partial, path: PathBuf, volume: Option<&VolumeOrigin>) -> FileRead {
        let length = task.object.length;
        let stuck = || FileRead::Stuck(format!("{}: no step in {} s", path.display(), VOLUME_STALL.as_secs()));
        let mut part = match Part::new(self, admission, 0, length, None) {
            Ok(part) => part,
            Err(refusal) => return FileRead::Failed(refusal),
        };
        let opening = path.clone();
        let opened = blocking(
            volume,
            move || -> std::io::Result<std::fs::File> {
                let file = std::fs::File::open(&opening)?;
                let metadata = file.metadata()?;
                if !metadata.is_file() || metadata.len() != length {
                    return Err(std::io::ErrorKind::NotFound.into());
                }
                Ok(file)
            },
            tokio::time::sleep(VOLUME_STALL),
        )
        .await;
        let mut file = match opened {
            None => return stuck(),
            Some(Err(_)) => return FileRead::Absent,
            Some(Ok(file)) => file,
        };
        // The file's bytes replace whatever an earlier fetch kept, and count as they come.
        task.received.store(0, Ordering::Release);
        task.kept.store(0, Ordering::Release);
        while part.reached < length {
            let want = (length - part.reached).min(STEP as u64) as usize;
            let step = blocking(
                volume,
                move || {
                    let mut buffer = vec![0; want];
                    let read = file.read_exact(&mut buffer);
                    (file, buffer, read)
                },
                tokio::time::sleep(VOLUME_STALL),
            )
            .await;
            let Some((back, buffer, read)) = step else {
                return stuck();
            };
            if read.is_err() {
                return FileRead::Absent;
            }
            file = back;
            if let Err(refusal) = part.push(task, &buffer).await {
                return FileRead::Failed(refusal);
            }
        }
        match part.flush().await {
            Ok(()) => FileRead::Done,
            Err(refusal) => FileRead::Failed(refusal),
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
                    let wrote = tokio::task::spawn_blocking(move || volume.write(&*store, &object, replace)).await;
                    match wrote {
                        Ok(Wrote::Written) => shared.stats.volume_written.fetch_add(1, Ordering::Relaxed),
                        Ok(Wrote::Skipped) => shared.stats.volume_skipped.fetch_add(1, Ordering::Relaxed),
                        _ => 0,
                    };
                }
            }
            shared.unbusy();
        });
    }
}
