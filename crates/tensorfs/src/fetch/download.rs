//! The downloader, as S3's own clients do it (aws-cli, CRT, s5cmd): a queue of objects, each read as
//! 4 MiB ranges (one request for an object of 4 MiB or less), one limit on requests in flight across
//! every object, and each request asked again by the one retry rule ([`ask`]) from the byte it reached.
//! Nothing judges a request's rate: asking a slow range again doesn't speed it up (tfs-141), while an
//! object's ranges side by side do.
//!
//! - **Order.** The queue is a plain number, smaller first; equal numbers go in arrival order. An
//!   object wanted twice is downloaded once. Requests take their places in line as their object
//!   starts, so an object's ranges run together. A task nothing wants stops (cl-686).
//! - **Tiers.** The local store, then the volume (when one is bound), then the origin. Every byte
//!   enters the store's admission, which hashes it: wrong bytes are never admitted. A process sharing
//!   the store that is fetching an object is waited for, not raced (tfs-142).
//! - **Cut or killed.** What landed stays in the store's `tmp/` ([`ObjectStore::resume`]), ranges
//!   too, and the next fetch asks only for the rest (tfs-143).
//! - **Every answer that is not 2xx is logged** with its object, range and attempt, and counted.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};

use super::ask::{self, Backoff, ATTEMPTS};
use super::net::{shown, Client, GetError};
use super::origin::{HttpLocation, Location, Origin};
use super::volume::{blocking, VolumeOrigin, Wrote, STEP};
use super::{Admission, ObjectStore, Tier};
use crate::err::{refuse, Code, Refusal, Result};
use crate::format::ObjectRef;

/// A range (aws-cli's `multipart_chunksize`). 4 MiB, not their 8: a cold R2 range drips at about
/// 0.3 MB/s (fresh objects, 11-04), so a smaller one ends its object's tail sooner.
const PART: u64 = 4 << 20;
/// Bytes a request gathers before writing them: the most a cut loses per request.
const WRITE_BYTES: usize = 256 << 10;
const VOLUME_READS: usize = 8;
const WRITE_BACKS: usize = 2;
/// A volume read that takes no step for this long leaves the volume out (the network's stall rule).
const VOLUME_STALL: Duration = Duration::from_secs(30);
/// How often an object another process is fetching is looked at again.
const CLAIM_POLL: Duration = Duration::from_millis(250);

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
    pub fn task(&self) -> Arc<Task> {
        Arc::clone(&self.task)
    }

    pub async fn wait(mut self) -> Result<Tier> {
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
    pub store: Arc<dyn ObjectStore>,
    pub client: Client,
    pub volume: Option<VolumeOrigin>,
    queue: Mutex<Queue>,
    queued: Notify,
    closed: AtomicBool,
    closing: Notify,
    /// Requests in flight, across every object.
    requests: Arc<Semaphore>,
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
    Admitted,
    Absent,
    /// The bytes were not the object's.
    Corrupt(Refusal),
    /// Abandoned as silent; the operation may still be blocked.
    Stuck(String),
    Failed(Refusal),
}

/// Bytes gathered for one file, written off the runtime `WRITE_BYTES` at a time.
async fn spill<W: Send + 'static>(out: &mut Option<W>, buffer: &mut Vec<u8>, write: impl FnOnce(&mut W, &[u8]) -> Result<()> + Send + 'static) -> Result<()> {
    if buffer.is_empty() {
        return Ok(());
    }
    let (Some(mut target), mut bytes) = (out.take(), std::mem::take(buffer)) else {
        return refuse(Code::IO_FAILED, "its file is gone");
    };
    let written = tokio::task::spawn_blocking(move || {
        let written = write(&mut target, &bytes);
        bytes.clear();
        (target, bytes, written)
    })
    .await;
    let (target, bytes, written) = written.map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })?;
    (*out, *buffer) = (Some(target), bytes);
    written
}

/// One object's bytes on their way into the store's admission, in order.
struct Sink<'s> {
    shared: &'s Shared,
    task: &'s Task,
    admission: Option<Box<dyn Admission>>,
    buffer: Vec<u8>,
    reached: u64,
}

impl<'s> Sink<'s> {
    /// The object's door, with whatever an earlier fetch left: taking that up hashes it, off the runtime.
    async fn open(shared: &'s Shared, task: &'s Task) -> Result<Sink<'s>> {
        let (store, object) = (Arc::clone(&shared.store), task.object.clone());
        let admission = tokio::task::spawn_blocking(move || store.resume(&object))
            .await
            .map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })??;
        let (reached, ahead) = admission.held();
        if reached + ahead > 0 {
            tracing::info!(object = %task.object.sha256, reached, ahead, "an earlier fetch's bytes taken up");
        }
        task.kept.store(reached + ahead, Ordering::Release);
        task.received.store(reached + ahead, Ordering::Release);
        Ok(Sink { shared, task, admission: Some(admission), buffer: Vec::with_capacity(WRITE_BYTES), reached })
    }

    /// Start the object over, nothing kept.
    fn fresh(&mut self) -> Result<()> {
        if let Some(admission) = self.admission.take() {
            admission.discard();
        }
        self.admission = Some(self.shared.store.resume(&self.task.object)?);
        self.task.kept.store(0, Ordering::Release);
        self.task.received.store(0, Ordering::Release);
        self.buffer.clear();
        self.reached = 0;
        Ok(())
    }

    /// Whether ranges have somewhere to land.
    fn keeps(&self) -> bool {
        self.admission.as_ref().is_some_and(|admission| admission.keeps())
    }

    /// The file for the range at byte `at`, and what it holds.
    fn range(&mut self, at: u64) -> Result<Option<(std::fs::File, u64)>> {
        match &mut self.admission {
            Some(admission) => admission.range(at),
            None => refuse(Code::IO_FAILED, "the admission is gone"),
        }
    }

    async fn push(&mut self, data: &[u8]) -> Result<()> {
        let length = self.task.object.length;
        if self.reached + data.len() as u64 > length {
            return refuse(Code::LENGTH_MISMATCH, format!("{}: the origin sent more than its {length} B", self.task.object.sha256));
        }
        self.buffer.extend_from_slice(data);
        self.reached += data.len() as u64;
        self.task.gained(data.len() as u64);
        if self.buffer.len() >= WRITE_BYTES {
            self.flush().await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        spill(&mut self.admission, &mut self.buffer, |admission, bytes| admission.write(bytes)).await
    }

    /// The whole range at the byte reached, whose arrival has counted already, joins the bytes in order.
    async fn take(&mut self) -> Result<()> {
        self.flush().await?;
        let Some(mut admission) = self.admission.take() else {
            return refuse(Code::IO_FAILED, "the admission is gone");
        };
        let taken = tokio::task::spawn_blocking(move || {
            let taken = admission.take();
            (admission, taken)
        })
        .await;
        let (admission, taken) = taken.map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })?;
        self.admission = Some(admission);
        self.reached = taken?;
        Ok(())
    }

    /// Everything written, hashed and admitted; the sink is spent until `fresh`.
    async fn commit(&mut self) -> Result<()> {
        self.flush().await?;
        let Some(admission) = self.admission.take() else {
            return refuse(Code::IO_FAILED, "the admission is gone");
        };
        tokio::task::spawn_blocking(move || admission.commit())
            .await
            .map_err(|error| Refusal { code: Code::IO_FAILED, detail: error.to_string() })?
    }
}

/// One range of an object, landing in its own file.
struct Part {
    at: u64,
    reached: u64,
    end: u64,
    file: Option<std::fs::File>,
    buffer: Vec<u8>,
}

impl Part {
    async fn flush(&mut self) -> Result<()> {
        let at = self.at;
        spill(&mut self.file, &mut self.buffer, move |file, bytes| {
            file.write_all(bytes).map_err(|error| crate::store::fail("write", format!("the range at {at}"))(error).refusal())
        })
        .await
    }
}

/// Where a request's body goes: the object, in order, or one of its ranges.
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

    fn end(&self, length: u64) -> u64 {
        match self {
            Target::Sink(_) => length,
            Target::Part(part) => part.end,
        }
    }

    async fn push(&mut self, task: &Task, data: &[u8]) -> Result<()> {
        match self {
            Target::Sink(sink) => sink.push(data).await,
            Target::Part(part) => {
                if part.reached + data.len() as u64 > part.end {
                    return refuse(Code::LENGTH_MISMATCH, format!("{}: the origin sent more than the range asked", task.object.sha256));
                }
                part.buffer.extend_from_slice(data);
                part.reached += data.len() as u64;
                task.gained(data.len() as u64);
                match part.buffer.len() >= WRITE_BYTES {
                    true => part.flush().await,
                    false => Ok(()),
                }
            }
        }
    }

    async fn flush(&mut self) -> Result<()> {
        match self {
            Target::Sink(sink) => sink.flush().await,
            Target::Part(part) => part.flush().await,
        }
    }
}

impl Shared {
    pub fn new(store: Arc<dyn ObjectStore>, client: Client, volume: Option<VolumeOrigin>, requests: usize) -> Shared {
        Shared {
            store,
            client,
            volume,
            queue: Mutex::default(),
            queued: Notify::new(),
            closed: AtomicBool::new(false),
            closing: Notify::new(),
            requests: Arc::new(Semaphore::new(crate::descriptors::requests(requests.max(1)))),
            busy: AtomicUsize::new(0),
            settle: Notify::new(),
            volume_reads: Semaphore::new(VOLUME_READS),
            write_backs: Semaphore::new(WRITE_BACKS),
            stats: Counters::default(),
        }
    }

    /// Stops the dispatcher and every transfer.
    pub fn close(&self) {
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
            let statuses = lock(&self.stats.statuses).clone();
            let (requests, retries) = (self.stats.requests.load(Ordering::Relaxed), self.stats.retries.load(Ordering::Relaxed));
            tracing::info!(requests, retries, ?statuses, "downloads settled (counts since start)");
            self.settle.notify_waiters();
        }
    }

    /// A place in line for `object`, joining its task when one is open.
    pub fn want(&self, object: &ObjectRef, origin: &Arc<dyn Origin>, priority: u64) -> Waiter {
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
    pub fn fetching(&self, object: &ObjectRef) -> Option<Fetching> {
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
    pub async fn dispatch(self: Arc<Self>) {
        loop {
            let permit = tokio::select! {
                permit = Arc::clone(&self.requests).acquire_owned() => match permit {
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
    async fn fetch(self: &Arc<Self>, task: &Arc<Task>, permit: OwnedSemaphorePermit) -> Result<Tier> {
        if self.store.has(&task.object) {
            return Ok(Tier::Local);
        }
        let _claim = loop {
            if let Some(claim) = self.store.claim(&task.object) {
                break claim;
            }
            tokio::time::sleep(CLAIM_POLL).await;
        };
        // It may have landed while another held the claim.
        if self.store.has(&task.object) {
            return Ok(Tier::Local);
        }
        let mut sink = Sink::open(self, task).await?;
        let fetched = self.through_tiers(task, &mut sink, permit).await;
        if fetched.is_err() {
            // What arrived and was waiting for a write is kept with the rest.
            let _ = sink.flush().await;
        }
        fetched
    }

    /// `sink`'s object from the volume, else the origin, from the byte it holds.
    async fn through_tiers(self: &Arc<Self>, task: &Arc<Task>, sink: &mut Sink<'_>, permit: OwnedSemaphorePermit) -> Result<Tier> {
        let mut replace = false;
        if let Some(volume) = self.volume.clone().filter(VolumeOrigin::answering) {
            let _reading = self.volume_reads.acquire().await;
            if volume.answering() {
                task.enter(Phase::Volume);
                match self.read_file_into(sink, volume.blob(&task.object), Some(&volume)).await {
                    FileRead::Admitted => {
                        let (touching, object) = (volume.clone(), task.object.clone());
                        tokio::task::spawn_blocking(move || touching.touch(&object));
                        return Ok(Tier::Volume);
                    }
                    FileRead::Corrupt(refusal) => {
                        tracing::info!(object = %task.object.sha256, %refusal, "volume copy refused");
                        // Bytes kept from an earlier fetch are as suspect as the copy: only a whole
                        // read of the volume blames it.
                        replace = task.kept() == 0;
                        sink.fresh()?;
                    }
                    FileRead::Absent | FileRead::Stuck(_) => {}
                    FileRead::Failed(refusal) => return Err(refusal),
                }
            }
        }
        self.origin_copy(task, sink, permit).await?;
        if task.origin.public() {
            self.write_back(task.object.clone(), replace);
        }
        Ok(Tier::Origin)
    }

    /// The origin's copy. Wrong bytes are asked for once more, from nothing: different wrong bytes
    /// may be the link's, the same twice are the origin's.
    async fn origin_copy(&self, task: &Arc<Task>, sink: &mut Sink<'_>, permit: OwnedSemaphorePermit) -> Result<()> {
        let (mut permit, mut wrong) = (Some(permit), None::<String>);
        loop {
            task.enter(Phase::Origin);
            let located = task.origin.locate(std::slice::from_ref(&task.object)).await;
            let Some(here) = located?.pop() else {
                return refuse(Code::REF_NOT_FOUND, format!("{}: the origin located nothing", task.object.sha256));
            };
            match &here {
                Location::File(path) => match self.read_file_into(sink, path.clone(), None).await {
                    FileRead::Admitted => return Ok(()),
                    FileRead::Absent => return refuse(Code::REF_NOT_FOUND, format!("{}: absent", path.display())),
                    // Bytes kept from an earlier fetch are as suspect as the file: read it whole.
                    FileRead::Corrupt(_) if task.kept() > 0 => {
                        sink.fresh()?;
                        continue;
                    }
                    FileRead::Corrupt(refusal) | FileRead::Failed(refusal) => return Err(refusal),
                    FileRead::Stuck(why) => return refuse(Code::TRANSFER_FAILED, why),
                },
                http => self.ranges(task, sink, &http.http().expect("a URL location"), &mut permit).await?,
            }
            match sink.commit().await {
                Ok(()) => return Ok(()),
                Err(refusal) if refusal.code == Code::OBJECT_ID_MISMATCH && wrong.as_ref() != Some(&refusal.detail) => {
                    tracing::info!(object = %task.object.sha256, %refusal, "refused, asked again");
                    wrong = Some(refusal.detail.clone());
                    sink.fresh()?;
                }
                Err(refusal) => return Err(refusal),
            }
        }
    }

    /// What `sink` lacks: one request when it is one range or the store keeps no ranges, else every
    /// `PART` range in a file of its own, side by side, each joining the bytes in order once whole.
    async fn ranges(&self, task: &Task, sink: &mut Sink<'_>, here: &HttpLocation<'_>, first: &mut Option<OwnedSemaphorePermit>) -> Result<()> {
        let length = task.object.length;
        if length - sink.reached <= PART || !sink.keeps() {
            let _permit = self.request_slot(first).await?;
            return self.range(task, here, &mut Target::Sink(sink)).await;
        }
        sink.flush().await?;
        let mut parts = Vec::new();
        let mut at = sink.reached;
        while at < length {
            let end = ((at / PART + 1) * PART).min(length);
            let Some((file, held)) = sink.range(at)? else {
                return refuse(Code::IO_FAILED, "the store keeps no ranges");
            };
            parts.push(Part { at, reached: at + held.min(end - at), end, file: Some(file), buffer: Vec::new() });
            at = end;
        }
        let mut flying: FuturesUnordered<_> = parts
            .into_iter()
            .map(|mut part| {
                let first = first.take();
                async move {
                    if part.reached < part.end {
                        let _permit = match first {
                            Some(permit) => permit,
                            None => self.request_slot(&mut None).await?,
                        };
                        self.range(task, here, &mut Target::Part(&mut part)).await?;
                    }
                    Ok::<u64, Refusal>(part.at)
                }
            })
            .collect();
        let mut whole = BTreeSet::new();
        while let Some(done) = flying.next().await {
            whole.insert(done?);
            while whole.remove(&sink.reached) {
                sink.take().await?;
            }
        }
        match sink.reached == length {
            true => Ok(()),
            // A disk short of space drops ranges: the next fetch asks for them again.
            false => refuse(Code::TRANSFER_FAILED, format!("{}: a range went missing at {}", task.object.sha256, sink.reached)),
        }
    }

    /// A place among the requests in flight: `first` when the task still holds the one it started with.
    async fn request_slot(&self, first: &mut Option<OwnedSemaphorePermit>) -> Result<OwnedSemaphorePermit> {
        if let Some(permit) = first.take() {
            return Ok(permit);
        }
        Arc::clone(&self.requests).acquire_owned().await.map_err(|_| Refusal { code: Code::TRANSFER_FAILED, detail: "the fetcher closed".into() })
    }

    /// One range into `into`, asked again by the one rule (`ask`) from the byte reached.
    async fn range(&self, task: &Task, here: &HttpLocation<'_>, into: &mut Target<'_, '_>) -> Result<()> {
        let (mut backoff, mut attempt, mut still) = (Backoff::new(), 0, 0);
        let (began, first) = (Instant::now(), into.reached());
        loop {
            attempt += 1;
            let from = into.reached();
            let (why, wait) = match self.request(task, here, into, attempt).await {
                Ok(()) => {
                    {
                        let mut slowest = lock(&task.slowest);
                        if began.elapsed() > slowest.0 {
                            *slowest = (began.elapsed(), into.reached() - first);
                        }
                    }
                    return into.flush().await;
                }
                Err(Failed::Fatal(refusal)) => return Err(refusal),
                Err(Failed::Again(why, wait)) => (why, wait),
            };
            let moved = into.reached() > from;
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

    /// One GET of what `into` still lacks, its body read to the end.
    async fn request(&self, task: &Task, here: &HttpLocation<'_>, into: &mut Target<'_, '_>, attempt: u32) -> std::result::Result<(), Failed> {
        let (object, length) = (&task.object.sha256, task.object.length);
        let (from, end) = (into.reached(), into.end(length));
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
                _ if ask::weather(status) => return Err(Failed::Again(why, wait)),
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
        match (status, &mut *into) {
            (206, _) => {
                let got = answer.content_range();
                if !got.is_some_and(|(first, total)| first == from && total.is_none_or(|t| t == length)) {
                    return Err(mismatch(format!("content-range {got:?} for byte {from}")));
                }
            }
            (200, Target::Part(_)) => return Err(mismatch("the origin ignores ranges".into())),
            (200, Target::Sink(sink)) => {
                if let Some(sent) = answer.header("content-length").and_then(|v| v.parse::<u64>().ok()).filter(|sent| *sent != length) {
                    return Err(mismatch(format!("{sent} B, not {length}")));
                }
                // The origin ignored the range: the object starts over from this answer.
                if from > 0 {
                    sink.fresh()?;
                }
            }
            _ => return Err(mismatch(format!("{status} to a GET"))),
        }
        loop {
            match answer.frame().await {
                Ok(Some(data)) => into.push(task, &data).await?,
                Ok(None) => break,
                Err(refusal) => {
                    let at = into.reached();
                    tracing::info!(object, range = shown_range, attempt, at, why = %refusal.detail, "body failed");
                    return Err(Failed::Again(format!("{object}: {} at {at}", refusal.detail), None));
                }
            }
        }
        match into.reached() == into.end(length) {
            true => Ok(()),
            false => Err(Failed::Again(format!("{object}: the body ended at {} of {}", into.reached(), into.end(length)), None)),
        }
    }

    /// `sink`'s object from a file, from the byte it holds. A step that takes `VOLUME_STALL` is
    /// abandoned, and a volume's abandoned read keeps it out of every fetch until it returns.
    async fn read_file_into(&self, sink: &mut Sink<'_>, path: PathBuf, volume: Option<&VolumeOrigin>) -> FileRead {
        let (length, from) = (sink.task.object.length, sink.reached);
        let stuck = || FileRead::Stuck(format!("{}: no step in {} s", path.display(), VOLUME_STALL.as_secs()));
        let opening = path.clone();
        let opened = blocking(
            volume,
            move || -> std::io::Result<std::fs::File> {
                let mut file = std::fs::File::open(&opening)?;
                let metadata = file.metadata()?;
                if !metadata.is_file() || metadata.len() != length {
                    return Err(std::io::ErrorKind::NotFound.into());
                }
                file.seek(SeekFrom::Start(from))?;
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
        while sink.reached < length {
            let want = (length - sink.reached).min(STEP as u64) as usize;
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
            if let Err(refusal) = sink.push(&buffer).await {
                return FileRead::Failed(refusal);
            }
        }
        match sink.commit().await {
            Ok(()) => FileRead::Admitted,
            Err(refusal) if refusal.code == Code::OBJECT_ID_MISMATCH => FileRead::Corrupt(refusal),
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
