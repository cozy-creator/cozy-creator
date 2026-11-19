//! A load's weights in RAM before it reads them (cl-1318; Paul, 11-19: "warming page cache; get the model into RAM / page-cache so it
//! can be read more quickly"): the chunks it reads that are on disk, read through the page cache in the order its load reads them (the
//! smallest component first, as the Start pass places them). TensorFS then reads each range the cache holds through it
//! (`tensorfs::io::cold`), so the load is bound by PCIe, not the disk.
//!
//! The page cache, not pinned memory or a tier of an executor's: varena's host number leaves clean cache out of what is held
//! (`Budget::held`), so nothing that holds memory has less room beside it, and the kernel takes it back first as anything grows. It
//! needs no process, GPU context or lock ahead, and the executor that loads reads it as it reads the store.
use std::{
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use tensorfs::format::ObjectRef;

use crate::storage::{self, Store};

/// Readers at once: enough requests in flight for an array of disks, as the executor's own reads keep.
const READERS: usize = 4;
/// Each reader's buffer, and how much it reads between looks at whether to stop.
const PIECE: usize = 8 << 20;
/// How long what the page cache holds of it is taken as said (`resident`): a Status read each second looks at hundreds of files.
const LOOKED: Duration = Duration::from_secs(1);

/// One read of a load's chunks into the page cache.
pub(crate) struct Stage {
    chunks: Vec<(PathBuf, u64)>,
    /// What of them the page cache held as it began.
    cached: AtomicU64,
    /// What it read from disk.
    read: AtomicU64,
    /// Readers still reading; it ended at 0.
    live: AtomicUsize,
    stopped: AtomicBool,
    /// It ended short of the room it was given.
    short: AtomicBool,
    began: Instant,
    took_ms: AtomicU64,
    /// What `resident` said last, and when.
    looked: Mutex<Option<(Instant, u64)>>,
}

impl Stage {
    pub(crate) fn total(&self) -> u64 {
        self.chunks.iter().map(|(_, length)| length).sum()
    }
    /// What of it is in RAM as far as it knows: what was there, and what it read.
    pub(crate) fn done(&self) -> u64 {
        self.cached.load(Ordering::Relaxed) + self.read.load(Ordering::Relaxed)
    }
    pub(crate) fn cached(&self) -> u64 {
        self.cached.load(Ordering::Relaxed)
    }
    pub(crate) fn read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }
    pub(crate) fn ended(&self) -> bool {
        self.live.load(Ordering::Acquire) == 0
    }
    pub(crate) fn short(&self) -> bool {
        self.short.load(Ordering::Relaxed)
    }
    pub(crate) fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
    pub(crate) fn took_ms(&self) -> u64 {
        self.took_ms.load(Ordering::Relaxed)
    }
    /// Its readers stop at their next piece.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
    /// What of it the page cache holds now (the kernel may have taken some back since), as looked at within `LOOKED`.
    pub(crate) fn resident(&self) -> u64 {
        let mut looked = self.looked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, bytes)) = *looked {
            if at.elapsed() < LOOKED {
                return bytes;
            }
        }
        let bytes = self.chunks.iter().map(|(path, length)| in_cache(path, *length)).sum();
        *looked = Some((Instant::now(), bytes));
        bytes
    }
}

/// What of `length` bytes of `path` the page cache holds.
fn in_cache(path: &PathBuf, length: u64) -> u64 {
    let Ok(file) = File::open(path) else { return 0 };
    match platform::durable::cached_pages(&file, 0, length) {
        Some((held, of)) if of > 0 => (length as u128 * held as u128 / of as u128) as u64,
        _ => 0,
    }
}

/// `selected`'s chunks the store holds, in the order a load reads them: the smallest component first, each component's in its order.
pub(crate) fn ordered(store: &Store, selected: &[(String, Vec<String>)]) -> Vec<ObjectRef> {
    let mut components: Vec<(u64, Vec<ObjectRef>)> = (selected.iter())
        .filter_map(|(header, components)| Some((storage::checkpoint(store, header).ok()?, components)))
        .flat_map(|(checkpoint, components)| {
            let each: Vec<Vec<ObjectRef>> = match components.is_empty() {
                true => vec![storage::chunks(&checkpoint, &[])],
                false => components.iter().map(|component| storage::chunks(&checkpoint, std::slice::from_ref(component))).collect(),
            };
            each.into_iter().map(|objects| (objects.iter().map(|o| o.length).sum(), objects)).collect::<Vec<_>>()
        })
        .collect();
    components.sort_by_key(|(bytes, _)| *bytes);
    let mut seen = std::collections::HashSet::new();
    (components.into_iter().flat_map(|(_, objects)| objects))
        .filter(|object| seen.insert(object.sha256.clone()) && store.holds(object))
        .collect()
}

/// `chunks` read into the page cache on threads of their own, those it holds already skipped. What it reads from disk stays within
/// `room` of what the cache held as it began: past it, it ends short. `live` is asked before each chunk; false ends it, as `stop` does.
/// `background`: at the lowest CPU and I/O priority, for a load not yet asked for. `ended` hears once its readers end.
pub(crate) fn start(
    store: &Store,
    chunks: Vec<ObjectRef>,
    room: impl FnOnce(u64) -> i64 + Send + 'static,
    live: impl Fn() -> bool + Send + Sync + 'static,
    background: bool,
    ended: impl FnOnce(&Stage) + Send + 'static,
) -> Arc<Stage> {
    let chunks: Vec<(PathBuf, u64)> = chunks.iter().filter_map(|object| Some((store.path(object).ok()?, object.length))).collect();
    let stage = Arc::new(Stage {
        chunks,
        cached: AtomicU64::new(0),
        read: AtomicU64::new(0),
        live: AtomicUsize::new(1),
        stopped: AtomicBool::new(false),
        short: AtomicBool::new(false),
        began: Instant::now(),
        took_ms: AtomicU64::new(0),
        looked: Mutex::new(None),
    });
    let owner = stage.clone();
    let started = std::thread::Builder::new().name("stage".into()).spawn(move || {
        let stage = owner;
        if background {
            platform::process::background_thread();
        }
        // What the cache holds already is in RAM; the rest is read, in order, while it fits.
        let cached: Vec<bool> = stage.chunks.iter().map(|(path, length)| in_cache(path, *length) >= *length).collect();
        let there: u64 = stage.chunks.iter().zip(&cached).filter(|(_, cached)| **cached).map(|((_, length), _)| length).sum();
        stage.cached.store(there, Ordering::Relaxed);
        let room = AtomicI64::new(room(there));
        let next = AtomicUsize::new(0);
        let read = |stage: &Stage| {
            if background {
                platform::process::background_thread();
            }
            let mut buf = vec![0u8; PIECE];
            loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some((path, length)) = stage.chunks.get(at) else { return };
                if stage.stopped() || !live() {
                    stage.stop();
                    return;
                }
                if cached[at] {
                    continue;
                }
                let length = i64::try_from(*length).unwrap_or(i64::MAX);
                if room.fetch_sub(length, Ordering::Relaxed) < length {
                    stage.short.store(true, Ordering::Relaxed);
                    return;
                }
                let Ok(mut file) = File::open(path) else { continue };
                while let Ok(n) = file.read(&mut buf) {
                    if n == 0 || stage.stopped() {
                        break;
                    }
                    stage.read.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
        };
        std::thread::scope(|scope| {
            for n in 1..READERS {
                if let Err(error) = std::thread::Builder::new().name(format!("stage-{n}")).spawn_scoped(scope, || read(&stage)) {
                    tracing::warn!("a reader of weights into RAM: {error}");
                }
            }
            read(&stage);
        });
        stage.took_ms.store(stage.began.elapsed().as_millis() as u64, Ordering::Relaxed);
        stage.live.store(0, Ordering::Release);
        ended(&stage);
    });
    if let Err(error) = started {
        tracing::warn!("reading weights into RAM ahead: {error}");
        stage.live.store(0, Ordering::Release);
    }
    stage
}
