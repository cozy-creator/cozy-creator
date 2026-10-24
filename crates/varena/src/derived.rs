//! Derived weights (design varena-local, "Derived weights"): a page a GPU transform produced (a LoRA
//! bake, a quantization from another source) is written behind, at idle, to its executor's own temp
//! file, and a later fill of that identity reads it back instead of deriving it again. The file is
//! the executor's alone and lives as long as it: unlinked from birth in the OS temp directory, so
//! the OS frees it at exit or crash; no sweeper, eviction or cache directory. Its bytes are capped
//! at the executor's models' own size; past the cap nothing is written and the transform reruns.
//! - M4: only where deriving again takes longer than writing the bytes and reading them back.
//! - M1: a reclaim never waits on a write: the page is pinned only for its device-to-host copy,
//!   and a write whose page went (or holds other bytes) by then is dropped.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};
use tensorfs::format::ObjectRef;
use tensorfs::io::source::{IoTally, Mode, Placement, Source};
use tensorfs::io::{Dst, ItemSource, ObjectRange};

use crate::backend::{Backend, Event, Va};
use crate::layout::Item;
use crate::{Error, Result};

/// One page's derived bytes: its set's fingerprint, its region, and its fill identity's digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub fingerprint: u64,
    pub region: u32,
    pub fill: [u8; 16],
}

impl Key {
    /// `identity` is the Fill::Declared transform identity (LoRA digests, scales, recipe, lane).
    pub fn new(fingerprint: u64, region: u32, identity: &str) -> Key {
        let mut fill = [0u8; 16];
        fill.copy_from_slice(Sha256::digest(identity.as_bytes()).get(..16).unwrap_or(&[0; 16]));
        Key { fingerprint, region, fill }
    }

    fn name(&self) -> String {
        let hex: String = self.fill.iter().map(|b| format!("{b:02x}")).collect();
        format!("{:016x}/{}/{hex}", self.fingerprint, self.region)
    }
}

/// M4: deriving again (`derive_ns`) takes longer than writing `bytes` and reading them back.
pub fn pays(derive_ns: u64, bytes: u64, write_bps: f64, read_bps: f64) -> bool {
    if write_bps <= 0.0 || read_bps <= 0.0 {
        return false;
    }
    let cost_s = bytes as f64 / write_bps + bytes as f64 / read_bps;
    derive_ns as f64 / 1e9 > cost_s
}

/// An executor's derived bytes: one file in the OS temp directory no name reaches, written
/// append-only up to a cap. Entries are never dropped: the file goes with the process.
pub struct Store {
    file: Arc<File>,
    used: u64,
}

impl Store {
    pub fn open() -> io::Result<Store> {
        Ok(Store { file: Arc::new(unlinked(&std::env::temp_dir())?), used: 0 })
    }

    /// Room for `len` more bytes within `cap`: where they go.
    pub fn reserve(&mut self, len: u64, cap: u64) -> Option<(Arc<File>, u64)> {
        let at = self.used;
        (at + len <= cap).then(|| {
            self.used += len;
            (self.file.clone(), at)
        })
    }
}

/// A new file in `dir` no name reaches: `O_TMPFILE` where the filesystem has it.
#[cfg(target_os = "linux")]
fn unlinked(dir: &Path) -> io::Result<File> {
    match OpenOptions::new().read(true).write(true).custom_flags(libc::O_TMPFILE).mode(0o600).open(dir) {
        Ok(file) => Ok(file),
        Err(error) if matches!(error.raw_os_error(), Some(libc::EOPNOTSUPP | libc::EISDIR | libc::EINVAL)) => named_then_unlinked(dir),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
fn unlinked(dir: &Path) -> io::Result<File> {
    named_then_unlinked(dir)
}

/// Created under a fresh name and unlinked at once (a filesystem without `O_TMPFILE`).
fn named_then_unlinked(dir: &Path) -> io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = dir.join(format!(".varena-derived-{}-{}", std::process::id(), NEXT.fetch_add(1, Relaxed)));
        match OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path) {
            Ok(file) => {
                std::fs::remove_file(&path)?;
                return Ok(file);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// What the writer asks of its holder.
pub trait Hold: Send + Sync {
    /// Pins the page for its device-to-host copy; dropping the guard unpins. `None` once it is
    /// gone or holds other bytes: its write is dropped (M1).
    fn pin(&self, key: &Key) -> Option<Box<dyn Send>>;
    /// No request runs: writes go only now.
    fn idle(&self) -> bool;
    /// Written whole at `at` of `file`: from now on fills of `key` read it.
    fn written(&self, key: Key, file: Arc<File>, at: u64);
}

/// One page to write: where in the store's file, where its bytes are mapped, and the event its
/// transform completed by.
pub struct Job {
    pub key: Key,
    pub file: Arc<File>,
    pub at: u64,
    pub va: Va,
    pub ready: Event,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Bumped by every offer and wake: a waiter that read it before its check misses none.
    gen: u64,
    stop: bool,
}

struct Shared {
    backend: Arc<dyn Backend>,
    hold: Arc<dyn Hold>,
    queue: Mutex<Queue>,
    woke: Condvar,
    /// Measured device-to-disk bytes per second; 0 until a write lands.
    bps: AtomicU64,
    /// A write failed (a full or unwritable temp directory): nothing more is offered.
    failed: AtomicBool,
}

/// The write-behind thread.
pub struct Writer {
    sh: Arc<Shared>,
}

impl Writer {
    pub fn new(backend: Arc<dyn Backend>, hold: Arc<dyn Hold>) -> Result<Writer> {
        let sh = Arc::new(Shared { backend, hold, queue: Mutex::new(Queue::default()), woke: Condvar::new(), bps: AtomicU64::new(0), failed: AtomicBool::new(false) });
        let run = sh.clone();
        std::thread::Builder::new().name("varena-derived".into()).spawn(move || run.run()).map_err(|e| Error::io("spawn the derived writer", e))?;
        Ok(Writer { sh })
    }

    /// Queues a page; it is written at the next idle moment.
    pub fn offer(&self, job: Job) {
        let mut q = self.sh.queue.lock();
        q.jobs.push_back(job);
        q.gen += 1;
        self.sh.woke.notify_all();
    }

    /// Whether writes still go: none after one failed.
    pub fn usable(&self) -> bool {
        !self.sh.failed.load(Relaxed)
    }

    /// The holder went idle.
    pub fn wake(&self) {
        let mut q = self.sh.queue.lock();
        q.gen += 1;
        self.sh.woke.notify_all();
    }

    /// Measured device-to-disk bytes per second (M4's write side), once a write landed.
    pub fn write_bps(&self) -> Option<f64> {
        let bps = self.sh.bps.load(Relaxed);
        (bps > 0).then_some(bps as f64)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let mut q = self.sh.queue.lock();
        q.stop = true;
        self.sh.woke.notify_all();
    }
}

impl Shared {
    fn run(&self) {
        loop {
            let job = {
                let mut q = self.queue.lock();
                loop {
                    if q.stop {
                        return;
                    }
                    let gen = q.gen;
                    if !q.jobs.is_empty() {
                        // The holder's idle test runs with no lock of ours held.
                        drop(q);
                        let idle = self.hold.idle();
                        q = self.queue.lock();
                        if idle {
                            if let Some(job) = q.jobs.pop_front() {
                                break job;
                            }
                        }
                    }
                    while q.gen == gen && !q.stop {
                        self.woke.wait(&mut q);
                    }
                }
            };
            if let Err(e) = self.write(job) {
                tracing::warn!("varena: a derived page's write failed; no more are written: {e}");
                self.failed.store(true, Relaxed);
                self.queue.lock().jobs.clear();
            }
        }
    }

    fn write(&self, job: Job) -> Result<()> {
        let Some(pin) = self.hold.pin(&job.key) else { return Ok(()) };
        let start = Instant::now();
        let mut bytes = vec![0u8; usize::try_from(job.va.len).map_err(|_| Error::Invalid("a page beyond the address space".into()))?];
        self.backend.copy_out(job.va, 0, &mut bytes, job.ready)?;
        // The bytes are ours: a reclaim may take the page now, while the disk write runs.
        drop(pin);
        job.file.write_all_at(&bytes, job.at).map_err(|e| Error::io("write a derived page", e))?;
        let secs = start.elapsed().as_secs_f64().max(1e-9);
        let now = (job.va.len as f64 / secs) as u64;
        let was = self.bps.load(Relaxed);
        self.bps.store(if was == 0 { now } else { (was * 3 + now) / 4 }, Relaxed);
        self.hold.written(job.key, job.file, job.at);
        Ok(())
    }
}

/// A refill from a written entry: one item over the page's bytes in the store's file, read
/// through the page cache exactly as a store fill reads.
pub struct Refill {
    source: Source,
    items: Vec<Item>,
}

impl Refill {
    pub fn new(key: &Key, file: &File, at: u64, len: u64) -> Result<Refill> {
        let file = file.try_clone().map_err(|e| Error::io("open a derived page", e))?;
        let obj = ObjectRef { sha256: key.name(), length: at + len };
        let items = vec![Item { offset: 0, len, source: ItemSource::Object(ObjectRange { obj: obj.clone(), off: at, len }), part_end: true }];
        Ok(Refill { source: Source::files(vec![(obj, file)], false, 0), items })
    }

    /// Reads the entry into `dst`.
    ///
    /// # Safety
    /// `dst` is mapped, as long as the entry, and only this refill writes it until it returns.
    pub unsafe fn read(&self, dst: Dst, tally: &IoTally) -> Result<()> {
        let placement = Placement { items: &self.items, origin: 0, dst };
        // SAFETY: the caller's contract on `dst`.
        unsafe { self.source.read_placements(&[placement], tensorfs::io::ReadOpts::default().threads, Mode::Buffered, tally, None) }?;
        Ok(())
    }
}
