//! The downloader, as S3's own clients do it (aws-cli, CRT, s5cmd): a queue of objects, each read as
//! 16 MiB ranges (one request for an object of 16 MiB or less), one limit on requests in flight across
//! every object, and each request asked again by the one retry rule ([`ask`]) from the byte it reached.
//! A range is written at its offset in the object's one file, as aws-cli writes parts; nothing is joined.
//! A request place left free with nothing waiting takes the back half of the in-flight range with the most
//! left (aria2's dynamic split, BitTorrent's end game), so a dripping object's tail gets many requests; a tail too
//! small to split that moves far slower than its peers is asked again on another connection, racing it (`HEDGES`).
//!
//! - **Order.** The queue is a plain number, smaller first; equal numbers go in arrival order. Its
//!   high 32 bits name a job (TensorD's: a run, in job order), its low ones a part within it. An
//!   object wanted twice is downloaded once, at the more urgent of its numbers. Requests take their
//!   places in line as their object starts, so an object's ranges run together. A place that comes
//!   free while an earlier job's object is still in flight takes the back half of its range instead
//!   of starting a later job's: one job's download at a time at full speed, its tail never shared
//!   with the next (cl-905); a job's own parts follow one another with the link full. A task nothing
//!   wants stops (cl-686).
//! - **Giving way.** Places go to the most urgent waiting range first. Every place taken, a range in flight gives
//!   its place up at its next bytes to one waiting at an earlier job's order, and asks again from the byte it reached.
//!   While an object at order 0 is under way (an emergency: what a running call reads now), every other request gives
//!   way at its next bytes and nothing else starts, so it has the whole link; then the rest go on (cl-966). A transfer
//!   outside the downloader at an order (an install's packages, from their own index) holds what comes after it the
//!   same way while it lasts ([`Shared::give_way`]).
//! - **Tiers.** The local store, then the volume (when one is bound), then the origin. Every byte
//!   enters the store's admission, which hashes it: wrong bytes are never admitted. A process sharing
//!   the store that is fetching an object is waited for, not raced (tfs-142).
//! - **Cut or killed.** What landed stays in the store's `tmp/` ([`Store::partial`]), and the next
//!   fetch asks only for the rest (tfs-143).
//! - **Every answer that is not 2xx is logged** with its object, range and attempt, and counted.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
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
/// A range's tail no steal can split (under `2 * STEAL` left) that moves slower than its peers is asked again on another
/// connection, racing the request it was (a hedged request; AWS's guidance retries the slowest few percent so): judged
/// over each `HEDGE_AFTER`, slow under an eighth of what a range's request moves (`Shared::typical`) and under
/// `HEDGE_FLOOR`, at most `HEDGES` times a range. The first of a race to reach the end stops the others. A tail that
/// sent 16 KiB every 27 s, under the 30 s stall, held a run 15 min (tfs-149, run 47820).
const HEDGE_AFTER: Duration = Duration::from_secs(2);
const SLOWER: f64 = 8.0;
const HEDGE_FLOOR: f64 = 1e6;
const HEDGES: u32 = 3;
/// A request that has had no byte yet is judged only after this: R2's first byte of a cold chunk takes 1-8 s (tfs-141),
/// and at 2 s a pod's 500 small first asks drew 78 hedges through a slow proxy (tfs-149).
const HEDGE_SILENT: Duration = Duration::from_secs(8);
/// The most hedges one look sends (`WINDOW`).
const HEDGES_AT_ONCE: usize = 4;
/// What a request moves before any range of 4 MiB or more has said (`Shared::typical`).
const TYPICAL: f64 = 2e6;
/// The ramp (AWS CRT's): requests in flight start at `START` and double each `WINDOW` while the bytes
/// received grow by `GAIN`, up to the cap; an error or a timeout halves them, once a window, never
/// below `START`, and the ramp probes again.
const START: usize = 8;
const WINDOW: Duration = Duration::from_millis(500);
const GAIN: f64 = 1.1;

/// The job a queue order names: its high 32 bits.
fn job(priority: u64) -> u64 {
    priority >> 32
}

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
    /// Its requests, the attempts in a row that moved nothing, and the slow tails asked again (`HEDGES`).
    asked: AtomicU32,
    unanswered: AtomicU32,
    hedged: AtomicU32,
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
/// of its length, when it last moved (Unix ms), how many wait for it, the attempts in a row
/// that moved nothing, and its slow tails asked again on another connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetching {
    pub phase: Phase,
    pub reached: u64,
    pub length: u64,
    pub moved_unix_ms: u64,
    pub waiters: usize,
    pub unanswered: u32,
    pub hedged: u32,
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

/// A transfer outside the downloader at its order (`Shared::give_way`): dropped, what waited for it goes on.
pub struct GiveWay {
    shared: Arc<Shared>,
    order: u64,
}

impl Drop for GiveWay {
    fn drop(&mut self) {
        let mut queue = lock(&self.shared.queue);
        if let Some(count) = queue.aside.get_mut(&self.order) {
            *count -= 1;
            if *count == 0 {
                queue.aside.remove(&self.order);
            }
        }
        self.shared.calmed(&queue);
        tracing::info!(order = self.order, "given way: the transfer outside ended");
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
    /// Its tasks at order 0 (emergencies).
    urgent: usize,
    /// Transfers outside the downloader, by order (`Shared::give_way`).
    aside: BTreeMap<u64, usize>,
}

impl Queue {
    /// The order past which every request waits: 0 during an emergency, else the most urgent transfer outside.
    fn floor(&self) -> u64 {
        match self.urgent {
            0 => self.aside.keys().next().copied().unwrap_or(u64::MAX),
            _ => 0,
        }
    }

    /// `task` leaves the queue, unless a task of its own already took its object's place.
    fn remove(&mut self, task: &Arc<Task>) {
        if self.tasks.get(&task.object.sha256).is_some_and(|t| Arc::ptr_eq(t, task)) {
            self.tasks.remove(&task.object.sha256);
            if task.priority.load(Ordering::Acquire) == 0 {
                self.urgent -= 1;
            }
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
    /// A task's order moved earlier (`want`), or the floor rose: what waits for a place asks again.
    moved: Notify,
    /// `Queue::floor`, read without the queue's lock.
    floor: AtomicU64,
    closed: AtomicBool,
    closing: Notify,
    /// Requests in flight, across every object, as many as the ramp allows (`Pace`).
    slots: Arc<Slots>,
    pace: Mutex<Pace>,
    /// Ranges in flight, for free places to take from.
    flights: Mutex<Vec<Arc<Flight>>>,
    /// What a range's request moves, bytes/s: the mean of ranges of 4 MiB or more, the latest weighing a fifth.
    typical: Mutex<Option<f64>>,
    /// Tasks and write-backs not yet ended.
    busy: AtomicUsize,
    settle: Notify,
    /// `busy` left 0: the ramp wakes.
    started: Notify,
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
    /// Slow tails asked again on another connection (`HEDGES`).
    pub hedges: AtomicU64,
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
    /// It gave its place up to a more urgent range (`Slots::outranked`): its bytes so far stay, and it asks again from there.
    GaveWay,
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
/// steal moves back. A request's has its object's task, whose order says who helps it first, and
/// where what was taken goes (`ranges`).
struct Flight {
    span: Mutex<(u64, u64)>,
    taken: Option<Taken>,
    /// The requests racing over its bytes, itself among them.
    race: Arc<Race>,
    /// How its race moves, for `Shared::hedge`.
    judged: Mutex<Judged>,
    /// Another of its race reached the end: its request stops where it is.
    cut: AtomicBool,
    stop: Notify,
}

/// Where a range began and when, its race's front when last judged and when, and its hedges so far.
struct Judged {
    from: u64,
    began: Instant,
    seen: u64,
    since: Instant,
    hedges: u32,
}

impl Flight {
    fn stopped(&self) -> bool {
        self.cut.load(Ordering::Acquire)
    }

    /// Another of its race reached the end: its request stops where it is.
    fn halt(&self) {
        let mut span = lock(&self.span);
        span.1 = span.0;
        self.cut.store(true, Ordering::Release);
        self.stop.notify_one();
    }
}

/// A range's request and its hedges, asking for the same tail: what it counts is past the furthest any reached (a byte
/// two of them receive counts once), and the first to reach its end stops the rest.
struct Race {
    front: Mutex<u64>,
    members: Mutex<Vec<std::sync::Weak<Flight>>>,
    /// One reached its end: a hedge that joins now stops at once.
    done: AtomicBool,
}

impl Race {
    fn front(&self) -> u64 {
        *lock(&self.front)
    }

    /// The bytes `reached` moves the race past its front.
    fn advance(&self, reached: u64) -> u64 {
        let mut front = lock(&self.front);
        let gained = reached.saturating_sub(*front);
        *front = (*front).max(reached);
        gained
    }

    /// `winner` reached its end: every other member stops where it is, and one that joins later at once.
    fn won(&self, winner: &Flight) {
        let members = lock(&self.members);
        self.done.store(true, Ordering::Release);
        for member in members.iter().filter_map(std::sync::Weak::upgrade) {
            if !std::ptr::eq(Arc::as_ptr(&member), winner) {
                member.halt();
            }
        }
    }

    /// `flight` runs in this race, stopped at once when it is over (a hedge sent as its first reached the end).
    fn join(&self, flight: &Arc<Flight>) {
        let mut members = lock(&self.members);
        members.push(Arc::downgrade(flight));
        if self.done.load(Ordering::Acquire) {
            flight.halt();
        }
    }
}

/// What a free place does for a range in flight: takes its back half, or races its slow tail (a hedge).
enum Take {
    Half(u64, u64, Slot),
    Hedge(Arc<Race>, u64, u64, Slot),
}

impl Take {
    fn slot(self) -> Slot {
        match self {
            Take::Half(_, _, slot) | Take::Hedge(_, _, _, slot) => slot,
        }
    }
}

/// A request's range's object, and where a back half taken from it, or a hedge of its tail, goes.
type Taken = (Arc<Task>, mpsc::UnboundedSender<Take>);

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
    /// A range of `[from, end)`; with `taken`, one free places may take from or hedge (a request's, not a file's); with
    /// `race`, a hedge in that range's race.
    fn new(shared: &'s Shared, admission: &Partial, from: u64, end: u64, taken: Option<Taken>, race: Option<Arc<Race>>) -> Result<Part<'s>> {
        let stealable = taken.is_some();
        let race = race.unwrap_or_else(|| Arc::new(Race { front: Mutex::new(from), members: Mutex::default(), done: AtomicBool::new(false) }));
        let flight = Arc::new(Flight {
            span: Mutex::new((from, end)),
            taken,
            race: Arc::clone(&race),
            judged: Mutex::new(Judged { from, began: Instant::now(), seen: from, since: Instant::now(), hedges: 0 }),
            cut: AtomicBool::new(false),
            stop: Notify::new(),
        });
        race.join(&flight);
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
        task.gained(self.flight.race.advance(self.reached));
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
    /// Who waits for a place, by order and then arrival: the most urgent first.
    waiting: BTreeMap<(u64, u64), oneshot::Sender<Slot>>,
    arrivals: u64,
}

/// A place among the requests in flight.
struct Slot(Arc<Slots>);

/// A range's ask for a place (`Shared::place`).
type Asking<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Slot> + Send + 'a>>;

impl Slots {
    fn new(free: usize) -> Arc<Slots> {
        Arc::new(Slots(Mutex::new(Places { free, ..Places::default() })))
    }

    /// A place for a range at `order`.
    async fn take(self: &Arc<Self>, order: u64) -> Slot {
        let waiting = {
            let mut places = lock(&self.0);
            if places.free > 0 {
                places.free -= 1;
                return Slot(Arc::clone(self));
            }
            let (sent, waiting) = oneshot::channel();
            places.arrivals += 1;
            let arrival = places.arrivals;
            places.waiting.insert((order, arrival), sent);
            waiting
        };
        waiting.await.expect("a place's sender lives as long as its queue")
    }

    /// Whether a range in flight at `order` gives its place up: every place is taken, and a range waits for one at an
    /// earlier job's order, or at 0 (what a running call reads now).
    fn outranked(&self, order: u64) -> bool {
        let mut places = lock(&self.0);
        if places.free > 0 {
            return false;
        }
        while let Some(first) = places.waiting.first_entry() {
            if first.get().is_closed() {
                first.remove();
                continue;
            }
            let urgent = first.key().0;
            return urgent < order && (urgent == 0 || job(urgent) < job(order));
        }
        false
    }

    /// A free place, when nothing waits for one.
    fn idle(self: &Arc<Self>) -> Option<Slot> {
        let mut places = lock(&self.0);
        places.waiting.retain(|_, waiter| !waiter.is_closed());
        (places.free > 0 && places.waiting.is_empty()).then(|| {
            places.free -= 1;
            Slot(Arc::clone(self))
        })
    }

    /// A place back: owed to the ramp, else to the most urgent waiter still waiting, else free.
    fn give(self: &Arc<Self>, places: &mut Places) {
        if places.owed > 0 {
            places.owed -= 1;
            return;
        }
        while let Some((_, waiter)) = places.waiting.pop_first() {
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
            moved: Notify::new(),
            floor: AtomicU64::new(u64::MAX),
            closed: AtomicBool::new(false),
            closing: Notify::new(),
            slots: Slots::new(START.min(requests.max(1))),
            flights: Mutex::default(),
            typical: Mutex::default(),
            pace: Mutex::new(Pace { level: START.min(requests.max(1)), max: crate::descriptors::requests(requests.max(1)), ..Pace::default() }),
            busy: AtomicUsize::new(0),
            settle: Notify::new(),
            started: Notify::new(),
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

    fn busy_one(&self) {
        if self.busy.fetch_add(1, Ordering::AcqRel) == 0 {
            self.started.notify_waiters();
        }
    }

    /// Resolves once a task or write-back is under way, or the fetcher closed.
    async fn active(&self) {
        loop {
            let notified = self.started.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.busy.load(Ordering::Acquire) > 0 {
                return;
            }
            tokio::select! {
                () = notified => {}
                () = self.closed() => return,
            }
        }
    }

    fn unbusy(&self) {
        if self.busy.fetch_sub(1, Ordering::AcqRel) == 1 {
            let statuses = lock(&self.stats.statuses).clone();
            let (requests, retries, steals) = (self.stats.requests.load(Ordering::Relaxed), self.stats.retries.load(Ordering::Relaxed), self.stats.steals.load(Ordering::Relaxed));
            let hedges = self.stats.hedges.load(Ordering::Relaxed);
            tracing::info!(requests, retries, steals, hedges, ?statuses, "downloads settled (counts since start)");
            self.settle.notify_waiters();
        }
    }

    /// The ramp, once a window while anything downloads: more requests while the bytes received grow, back to `START`
    /// when idle. Idle, it waits for the next download.
    pub(crate) async fn pace(self: Arc<Self>) {
        loop {
            tokio::select! {
                () = tokio::time::sleep(WINDOW) => {}
                () = self.closed() => return,
            }
            if self.window() {
                self.active().await;
            }
        }
    }

    /// One window of the ramp: true when nothing downloads.
    fn window(&self) -> bool {
        self.steal();
        self.hedge();
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
            return true;
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
            return false;
        }
        // A doubling's new requests take more than a window to deliver: two flat windows settle it.
        if pace.last.is_some_and(|last| rate < last * GAIN) {
            pace.flat += 1;
            if pace.flat >= 2 {
                pace.settled = true;
                tracing::info!(streams = pace.level, mb_s = rate / 1e6, "streams settled");
            }
            return false;
        }
        pace.flat = 0;
        let grow = (pace.level * 2).min(pace.max) - pace.level;
        (pace.level, pace.last) = (pace.level + grow, Some(rate));
        self.slots.add(grow);
        tracing::info!(streams = pace.level, mb_s = rate / 1e6, "more streams");
        false
    }

    /// Each free place with nothing waiting takes the back half of the in-flight range with the most
    /// left, while that half is at least `STEAL`; the range's own request stops where the half begins.
    fn steal(&self) {
        while let Some(slot) = self.slots.idle() {
            if self.take_half(slot, None).is_some() {
                return;
            }
        }
    }

    /// `slot` takes the back half of the in-flight range with the most left of those not held whose object is an
    /// earlier job's than order `before` (of any, when None), while that half is at least `STEAL`. The
    /// slot back when there is none.
    fn take_half(&self, slot: Slot, before: Option<u64>) -> Option<Slot> {
        let flights = lock(&self.flights).clone();
        let urgent = |flight: &&Arc<Flight>| {
            flight.taken.as_ref().is_some_and(|(task, _)| {
                let order = task.priority.load(Ordering::Acquire);
                !self.held(order) && before.is_none_or(|before| job(order) < job(before))
            })
        };
        let most = flights.iter().filter(urgent).max_by_key(|flight| {
            let span = lock(&flight.span);
            span.1.saturating_sub(span.0)
        });
        let Some((flight, (_, taken))) = most.and_then(|flight| Some((flight, flight.taken.as_ref()?))) else { return Some(slot) };
        let mut span = lock(&flight.span);
        let left = span.1.saturating_sub(span.0);
        if left < 2 * STEAL {
            return Some(slot);
        }
        let (half, end) = (span.0 + left / 2, span.1);
        span.1 = half;
        // Sent while the span is held: its range can't end and its object can't finish before.
        if let Err(error) = taken.send(Take::Half(half, end, slot)) {
            span.1 = end;
            return Some(error.0.slot());
        }
        self.stats.steals.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(from = half, end, "a free place took the back half of a range");
        None
    }

    /// Each free place with nothing waiting races the slow tail of a range no steal can split (`HEDGE_AFTER`): another
    /// request on another connection from the furthest its race reached, at most `HEDGES` times a range and
    /// `HEDGES_AT_ONCE` a look.
    fn hedge(&self) {
        let floor = (lock(&self.typical).unwrap_or(TYPICAL) / SLOWER).min(HEDGE_FLOOR);
        let mut sent = 0;
        for flight in lock(&self.flights).clone() {
            let Some((task, taken)) = flight.taken.as_ref() else { continue };
            let (reached, end) = *lock(&flight.span);
            let front = flight.race.front();
            let mut judged = lock(&flight.judged);
            let took = judged.since.elapsed();
            // No byte yet: a first byte may take seconds, so it waits longer than a tail that stopped.
            if took < HEDGE_AFTER || (front == judged.from && judged.began.elapsed() < HEDGE_SILENT) {
                continue;
            }
            let rate = front.saturating_sub(judged.seen) as f64 / took.as_secs_f64();
            (judged.seen, judged.since) = (front, Instant::now());
            if flight.stopped() || self.held(task.priority.load(Ordering::Acquire)) || reached >= end || end - reached >= 2 * STEAL || rate >= floor || judged.hedges >= HEDGES {
                continue;
            }
            if sent >= HEDGES_AT_ONCE {
                return;
            }
            let Some(slot) = self.slots.idle() else { return };
            if let Err(error) = taken.send(Take::Hedge(Arc::clone(&flight.race), front, end, slot)) {
                drop(error.0.slot());
                continue;
            }
            sent += 1;
            judged.hedges += 1;
            self.stats.hedges.fetch_add(1, Ordering::Relaxed);
            task.hedged.fetch_add(1, Ordering::Relaxed);
            tracing::info!(object = %task.object.sha256, from = front, end, kb_s = rate / 1e3, floor_kb_s = floor / 1e3,
                hedge = judged.hedges, "a slow tail asked again on another connection");
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

    /// A place in line for `object`, joining its task when one is open: at the more urgent of the two
    /// orders, an unstarted one's place in line and a started one's claim on the places that come free.
    pub(crate) fn want(&self, object: &ObjectRef, origin: &Arc<dyn Origin>, priority: u64) -> Waiter {
        let mut queue = lock(&self.queue);
        let task = match queue.tasks.get(&object.sha256).cloned() {
            Some(task) => {
                task.waiters.fetch_add(1, Ordering::AcqRel);
                if priority < task.priority.load(Ordering::Acquire) {
                    task.priority.store(priority, Ordering::Release);
                    if priority == 0 {
                        queue.urgent += 1;
                        self.calmed(&queue);
                    }
                    if !task.started.load(Ordering::Acquire) {
                        queue.seq += 1;
                        let seq = queue.seq;
                        queue.heap.push(Reverse((priority, seq, object.sha256.clone())));
                    }
                    // What waits for a place at its old order asks again: the dispatcher, and its ranges.
                    self.queued.notify_one();
                    self.moved.notify_waiters();
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
                    hedged: AtomicU32::new(0),
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
                if priority == 0 {
                    queue.urgent += 1;
                    self.calmed(&queue);
                }
                self.busy_one();
                self.queued.notify_one();
                task
            }
        };
        Waiter { answer: task.done.subscribe(), task }
    }

    /// The order of the most urgent task nobody has started that someone still wants, which stays in line.
    fn next(&self) -> Option<u64> {
        let mut queue = lock(&self.queue);
        let mut next = None;
        while let Some(Reverse((priority, _, sha256))) = queue.heap.peek().cloned() {
            if let Some(task) = queue.tasks.get(&sha256).cloned() {
                if !task.started.load(Ordering::Acquire) && task.priority.load(Ordering::Acquire) == priority {
                    if !queue.unwanted(&task) {
                        next = Some(priority);
                        break;
                    }
                    self.unbusy();
                }
            }
            queue.heap.pop();
        }
        self.calmed(&queue);
        next
    }

    /// `queue`'s floor, as read without its lock; risen, what waits under it wakes.
    fn calmed(&self, queue: &Queue) {
        let floor = queue.floor();
        if self.floor.swap(floor, Ordering::AcqRel) < floor {
            self.moved.notify_waiters();
            self.queued.notify_waiters();
        }
    }

    /// Whether a request at `order` waits: an emergency (order 0) or a transfer outside comes before it.
    fn held(&self, order: u64) -> bool {
        order > self.floor.load(Ordering::Acquire)
    }

    /// Until the guard drops, what comes after `order` waits as during an emergency: a transfer outside the
    /// downloader at that order (an install's packages, from their index) has the link.
    pub(crate) fn give_way(self: &Arc<Self>, order: u64) -> GiveWay {
        let mut queue = lock(&self.queue);
        *queue.aside.entry(order).or_default() += 1;
        self.calmed(&queue);
        tracing::info!(order, "giving way: what comes after a transfer outside waits for it");
        GiveWay { shared: Arc::clone(self), order }
    }

    /// The most urgent task nobody has started that someone still wants, unless the floor holds it:
    /// read under the queue's lock, as the floor moves, so none starts past one that fell meanwhile.
    fn pop(&self) -> Option<Arc<Task>> {
        let mut queue = lock(&self.queue);
        let mut popped = None;
        while let Some(Reverse((priority, _, sha256))) = queue.heap.peek().cloned() {
            let Some(task) = queue.tasks.get(&sha256).cloned() else {
                queue.heap.pop();
                continue;
            };
            if task.started.load(Ordering::Acquire) || task.priority.load(Ordering::Acquire) != priority {
                queue.heap.pop();
                continue;
            }
            if queue.unwanted(&task) {
                self.unbusy();
                queue.heap.pop();
                continue;
            }
            if priority > queue.floor() {
                break;
            }
            queue.heap.pop();
            task.started.store(true, Ordering::Release);
            task.enter(Phase::Starting);
            // Under the lock the floor moves under: its line comes in the order they happened.
            tracing::info!(object = %task.object.sha256, bytes = task.object.length, priority, "fetching");
            popped = Some(task);
            break;
        }
        self.calmed(&queue);
        popped
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
            hedged: task.hedged.load(Ordering::Acquire),
        })
    }

    fn finish(&self, task: &Arc<Task>, outcome: Result<Tier>) {
        {
            let mut queue = lock(&self.queue);
            queue.remove(task);
            self.calmed(&queue);
        }
        task.done.send_replace(Some(outcome));
        self.unbusy();
    }

    /// Whether nothing wants `task` any more; then it has left the queue (`Queue::unwanted`).
    fn unwanted(&self, task: &Arc<Task>) -> bool {
        let mut queue = lock(&self.queue);
        let unwanted = queue.unwanted(task);
        self.calmed(&queue);
        unwanted
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
            // A place at the order of what is next, asked again as something more urgent comes.
            // During an emergency, nothing else starts.
            let permit = loop {
                let notified = self.queued.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let next = self.next();
                if next.is_some_and(|next| self.held(next)) {
                    tokio::select! {
                        () = notified => continue,
                        () = self.closed() => return,
                    }
                }
                tokio::select! {
                    slot = self.slots.take(next.unwrap_or(u64::MAX)) => break slot,
                    () = notified => {}
                    () = self.closed() => return,
                }
            };
            let notified = self.queued.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            // An earlier job's range in flight comes before a later job's object: the place takes its back half.
            let permit = match self.next() {
                Some(next) => match self.take_half(permit, Some(next)) {
                    Some(permit) => permit,
                    None => continue,
                },
                None => permit,
            };
            // Nothing to start: the place goes back to the ranges waiting for one until a want comes.
            // Held here at the tail it starved them for good (run 32321).
            let Some(task) = self.pop() else {
                drop(permit);
                notified.await;
                continue;
            };
            let priority = task.priority.load(Ordering::Acquire);
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
                        let hedged = task.hedged.load(Ordering::Relaxed);
                        let (took, range) = *lock(&task.slowest);
                        let (slowest_range_s, slowest_range_kbps) = (took.as_secs_f64(), range as f64 / took.as_secs_f64().max(1e-3) / 1e3);
                        tracing::info!(object = %task.object.sha256, bytes, kept, ?tier, seconds = began.elapsed().as_secs_f64(), requests, hedged, slowest_range_s, slowest_range_kbps, priority, "landed");
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
    async fn ranges(&self, task: &Arc<Task>, admission: &Partial, here: &HttpLocation<'_>, first: &mut Option<Slot>) -> Result<()> {
        let (taken, mut takes) = mpsc::unbounded_channel();
        let taken = (Arc::clone(task), taken);
        let mut flying = FuturesUnordered::new();
        for (from, end) in lacking(&admission.held(), task.object.length) {
            flying.push(self.part(task, Part::new(self, admission, from, end, Some(taken.clone()), None)?, here, first.take()));
        }
        loop {
            tokio::select! {
                biased;
                Some(take) = takes.recv() => {
                    let (part, slot) = match take {
                        Take::Half(from, end, slot) => (Part::new(self, admission, from, end, Some(taken.clone()), None)?, slot),
                        // A hedge is no range of its own: nothing takes from it, and its race stops it.
                        Take::Hedge(race, from, end, slot) => (Part::new(self, admission, from, end, None, Some(race))?, slot),
                    };
                    flying.push(self.part(task, part, here, Some(slot)));
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
    /// Given way to a more urgent range (`Slots::outranked`), it asks for a place again at its own order.
    async fn part(&self, task: &Task, mut part: Part<'_>, here: &HttpLocation<'_>, slot: Option<Slot>) -> Result<()> {
        let mut slot = slot;
        loop {
            let _place = match slot.take() {
                Some(place) => place,
                None => self.place(task).await,
            };
            if self.range(task, here, &mut part).await? {
                return Ok(());
            }
        }
    }

    /// A place for one of `task`'s ranges at its order, asked again when that order moves earlier (`want`).
    async fn place(&self, task: &Task) -> Slot {
        let mut asked: Option<(u64, Asking<'_>)> = None;
        loop {
            let moved = self.moved.notified();
            tokio::pin!(moved);
            moved.as_mut().enable();
            let order = task.priority.load(Ordering::Acquire);
            if self.held(order) {
                asked = None;
                moved.await;
                continue;
            }
            if asked.as_ref().is_none_or(|(at, _)| *at != order) {
                asked = Some((order, Box::pin(self.slots.take(order))));
            }
            let (_, waiting) = asked.as_mut().expect("asked above");
            tokio::select! {
                place = waiting => return place,
                () = moved => {}
            }
        }
    }

    /// One range, asked again by the one rule (`ask`) from the byte reached, to its end. False: it gave its place up
    /// (`Failed::GaveWay`), its bytes so far written.
    async fn range(&self, task: &Task, here: &HttpLocation<'_>, part: &mut Part<'_>) -> Result<bool> {
        let (mut backoff, mut attempt, mut still) = (Backoff::transfer(), 0, 0);
        let (began, first) = (Instant::now(), part.reached);
        loop {
            attempt += 1;
            let from = part.reached;
            let failed = self.request(task, here, part, attempt).await;
            let (why, wait, moved) = match failed {
                Ok(()) => {
                    part.flush().await?;
                    if part.flight.stopped() {
                        return Ok(true);
                    }
                    // Its race's others stop: what they lack, it has.
                    part.flight.race.won(&part.flight);
                    let (took, bytes) = (began.elapsed(), part.reached - first);
                    {
                        let mut slowest = lock(&task.slowest);
                        if took > slowest.0 {
                            *slowest = (took, bytes);
                        }
                    }
                    if bytes >= 4 << 20 {
                        let rate = bytes as f64 / took.as_secs_f64().max(1e-3);
                        let mut typical = lock(&self.typical);
                        *typical = Some(typical.map_or(rate, |was| 0.8 * was + 0.2 * rate));
                    }
                    return Ok(true);
                }
                Err(Failed::GaveWay) => {
                    part.flush().await?;
                    return Ok(false);
                }
                Err(Failed::Fatal(refusal)) => return Err(refusal),
                Err(Failed::Again(why, wait)) => (why, wait, part.reached > from),
            };
            still = if moved { 0 } else { still + 1 };
            task.unanswered.store(still, Ordering::Release);
            self.stats.retries.fetch_add(1, Ordering::Relaxed);
            match backoff.after(moved, wait) {
                // Its race may end meanwhile: the next request then asks for nothing.
                Some(pause) => {
                    tokio::select! {
                        () = tokio::time::sleep(pause) => {}
                        () = part.flight.stop.notified() => {}
                    }
                }
                None => {
                    let detail = format!("{}: {ATTEMPTS} attempts in a row moved nothing; the last: {why}", task.object.sha256);
                    return refuse(Code::TRANSFER_FAILED, detail);
                }
            }
        }
    }

    /// One GET of what `part` still lacks, its body read to the end, or until another of its race reaches the end.
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
            let frame = tokio::select! {
                frame = answer.frame() => frame,
                () = part.flight.stop.notified() => break,
            };
            match frame {
                Ok(Some(data)) => {
                    self.stats.received.fetch_add(data.len() as u64, Ordering::Relaxed);
                    // A steal moved the end back: what is past it is another request's.
                    if !part.push(task, &data).await? {
                        break;
                    }
                    let order = task.priority.load(Ordering::Acquire);
                    if self.held(order) || self.slots.outranked(order) {
                        return Err(Failed::GaveWay);
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
        let mut part = match Part::new(self, admission, 0, length, None, None) {
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
        self.busy_one();
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
