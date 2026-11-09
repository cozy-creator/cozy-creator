//! Fetching into a store (design §5). An [`Origin`] locates objects ([`Endpoint`], the
//! distribution protocol's client; [`VolumeOrigin`] for a datacenter volume); the [`Fetcher`]
//! walks a commit's closure (tree → its repo files and its header → the wanted components'
//! chunks and assets), reads each object through the tiers local store → volume → origin,
//! and admits it through the store, which verifies it. One HTTP client ([`Client`], reqwest) and one
//! downloader serve every origin; every request, the Endpoint's documents and uploads and an
//! import's sources too, is asked again by one rule (`ask`).

pub(crate) mod ask;
pub mod conform;
mod download;
mod endpoint;
mod net;
mod origin;
pub(crate) mod policy;
pub(crate) mod token;
mod volume;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::stream::{FuturesUnordered, TryStreamExt};

pub(crate) use endpoint::streaming_writer;
pub use endpoint::{upload_commit, Endpoint, Hardware, Payload, Resolved, Stream, UploadControl, Uploaded};
pub use net::Client;
pub(crate) use net::GetError;
pub use origin::{Boxed, Location, Origin};
pub use policy::{is_loopback_origin, SourcePolicy};
pub use token::{bearer, HeaderMap, Request, SigV4, TokenProvider};
pub use volume::VolumeOrigin;

use crate::err::{refuse, Code, Refusal, Result};
use crate::format::{self, Header, ObjectRef, Tree};
pub use ask::ATTEMPTS;
pub use download::{Fetching, Phase};
use download::{Shared, Task};

/// GET `url` from byte `from` into `sink`, for bytes the caller holds to a digest: any public host,
/// redirects followed. A weather answer or a transport failure is asked again from the byte reached,
/// until the retry rule ends it `HUB_UNREACHABLE` ([`ask::get`]). `sink` takes each
/// body frame with the byte it starts at (0 again when a server restarts the body). A `from` at or past
/// the end (416) has nothing left to get: the caller's digest judges what it holds.
pub async fn get_pinned(url: &str, from: u64, sink: &mut (dyn FnMut(u64, &[u8]) -> Result<()> + Send + '_)) -> Result<()> {
    let policy = SourcePolicy { any_host: true, allow_local: policy::is_loopback_origin(url), max_redirects: 5, ..Default::default() };
    get_body(policy, url, &[], from, sink).await
}

/// [`get_pinned`] into the file at `part`, from the bytes it holds (what an earlier fetch reached resumes), telling
/// `moved` each byte position reached; answers the file and its sha256 for the caller to hold to its pin. Once nothing
/// moves for the stall rule it is refused, its bytes kept for the next fetch. Another fetch of the same file holds it:
/// this one is refused.
pub async fn get_pinned_into(url: &str, part: &Path, moved: &mut (dyn FnMut(u64) + Send)) -> std::result::Result<(File, String), String> {
    let failed = |e: std::io::Error| format!("{}: {e}", part.display());
    let mut file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(part).map_err(failed)?;
    if !platform::lock::try_claim(&file).map_err(|e| e.to_string())? {
        return Err(format!("another cozy command is fetching {url} now; run this again once it has"));
    }
    let (from, mut at) = (file.metadata().map_err(failed)?.len(), 0);
    moved(from);
    let mut sink = |start: u64, frame: &[u8]| -> Result<()> {
        let written = (|| {
            if start != at {
                file.set_len(start)?;
                file.seek(SeekFrom::Start(start))?;
            }
            file.write_all(frame)
        })();
        written.map_err(|e| Refusal { code: Code::IO_FAILED, detail: failed(e) })?;
        at = start + frame.len() as u64;
        moved(at);
        Ok(())
    };
    get_pinned(url, from, &mut sink).await.map_err(|e| e.to_string())?;
    file.rewind().map_err(failed)?;
    let mut hash = sha2::Sha256::default();
    std::io::copy(&mut file, &mut hash).map_err(failed)?;
    file.rewind().map_err(failed)?;
    Ok((file, crate::sha256::hex(&sha2::Digest::finalize(hash))))
}

/// GET a link a person wrote into `sink`, for a caller that judges what it gets itself: http or https, a public
/// address at every hop, 5 redirects at most, asked as [`get_pinned`] asks. Nothing makes it local.
pub async fn get_link(url: &str, sink: &mut (dyn FnMut(u64, &[u8]) -> Result<()> + Send + '_)) -> Result<()> {
    let policy = SourcePolicy { any_host: true, plaintext: true, max_redirects: 5, ..Default::default() };
    // Some hosts refuse a request that names no client.
    get_body(policy, url, &[("user-agent".into(), "cozy".into())], 0, sink).await
}

async fn get_body(policy: SourcePolicy, url: &str, headers: &[(String, String)], from: u64, sink: &mut (dyn FnMut(u64, &[u8]) -> Result<()> + Send + '_)) -> Result<()> {
    let range = (from > 0).then_some((from, None));
    let got = ask::get(&Client::new(policy), url, headers, None, range, sink).await;
    match got.map_err(GetError::refusal)? {
        Ok(_) => Ok(()),
        Err(416) if from > 0 => Ok(()),
        Err(status @ (404 | 410)) => refuse(Code::REF_NOT_FOUND, format!("{url}: HTTP {status}")),
        Err(status) => refuse(Code::ORIGIN_REFUSED, format!("{url}: HTTP {status}")),
    }
}

/// Where an object came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    Local,
    Volume,
    Origin,
}

/// Bytes by the tier they came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Landed {
    pub local: u64,
    pub volume: u64,
    pub origin: u64,
}

impl Landed {
    /// An object of `bytes`, `kept` of which an earlier fetch had left here: they did not come again.
    fn add(&mut self, tier: Tier, bytes: u64, kept: u64) {
        let kept = kept.min(bytes);
        self.local += kept;
        match tier {
            Tier::Local => self.local += bytes - kept,
            Tier::Volume => self.volume += bytes - kept,
            Tier::Origin => self.origin += bytes - kept,
        }
    }

    fn join(&mut self, other: Landed) {
        (self.local, self.volume, self.origin) = (
            self.local + other.local,
            self.volume + other.volume,
            self.origin + other.origin,
        );
    }
}

/// How a `Fetcher` runs: its budget of requests in flight.
#[derive(Debug, Clone)]
pub struct Config {
    /// The most requests in flight at once, across every object; the ramp reaches it while that pays.
    /// A request moved 7.5-10 MB/s from R2 on a cpu pod (11-04).
    pub requests: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config { requests: 128 }
    }
}

/// What a pull wants: components (all when `None`), its place in the queue (smaller first),
/// and where it reports progress.
#[derive(Default)]
pub struct Want<'a> {
    pub components: Option<&'a [String]>,
    pub priority: u64,
    pub progress: Option<&'a Progress>,
}

/// A pull's bytes: all it needs, and those here so far ([`Progress::done`]).
#[derive(Default)]
pub struct Progress {
    pub total: AtomicU64,
    held: AtomicU64,
    /// Objects on their way: their bytes count as they arrive, not only once whole.
    arriving: Mutex<HashMap<String, Arc<Task>>>,
}

impl Progress {
    /// The bytes here, an object still arriving counted to the byte it reached.
    pub fn done(&self) -> u64 {
        let arriving = self.arriving.lock().unwrap();
        self.held.load(Ordering::Relaxed) + arriving.values().map(|task| task.received()).sum::<u64>()
    }

    /// An object that landed counts whole, in one step with no longer counting as arriving.
    fn landed(&self, object: &ObjectRef) {
        let mut arriving = self.arriving.lock().unwrap();
        arriving.remove(&object.sha256);
        self.held.fetch_add(object.length, Ordering::Relaxed);
    }
}

/// What a pull took: the tree (when it walked one), its header, and its objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub tree: Option<ObjectRef>,
    pub header: ObjectRef,
    pub objects: usize,
    pub landed: Landed,
}

/// A fetcher's counts since it started: requests, retries, steals, bytes, statuses, volume writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub requests: u64,
    pub retries: u64,
    pub steals: u64,
    /// Bytes received from the origin, over the time any of them was arriving.
    pub received: u64,
    pub receiving: std::time::Duration,
    /// Answers that were not 2xx, by status.
    pub statuses: std::collections::BTreeMap<u16, u64>,
    pub volume_written: u64,
    pub volume_skipped: u64,
}

/// A store's one downloader. Construct it inside a tokio runtime ([`runtime`] serves callers
/// without one); dropping it stops its workers.
pub struct Fetcher {
    shared: Arc<Shared>,
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        self.shared.close();
    }
}

impl Fetcher {
    /// Fetches into `store` over `client`, reading `volume` first when one is given.
    pub fn new(
        store: Arc<crate::store::Store>,
        client: Client,
        volume: Option<VolumeOrigin>,
        config: Config,
    ) -> Fetcher {
        let shared = Arc::new(Shared::new(store, client, volume, config.requests));
        tokio::spawn(Arc::clone(&shared).dispatch());
        tokio::spawn(Arc::clone(&shared).pace());
        Fetcher { shared }
    }

    pub fn client(&self) -> &Client {
        &self.shared.client
    }

    pub fn stats(&self) -> Stats {
        let counters = &self.shared.stats;
        Stats {
            requests: counters.requests.load(Ordering::Relaxed),
            retries: counters.retries.load(Ordering::Relaxed),
            steals: counters.steals.load(Ordering::Relaxed),
            received: counters.received.load(Ordering::Relaxed),
            receiving: std::time::Duration::from_millis(counters.receiving_ms.load(Ordering::Relaxed)),
            statuses: counters.statuses.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
            volume_written: counters.volume_written.load(Ordering::Relaxed),
            volume_skipped: counters.volume_skipped.load(Ordering::Relaxed),
        }
    }

    /// Each of `objects`' task as its waiters see it (`Fetching`): None for one with no task open
    /// (here already, landed, failed, or wanted by nobody).
    pub fn fetching(&self, objects: &[ObjectRef]) -> Vec<Option<Fetching>> {
        objects.iter().map(|object| self.shared.fetching(object)).collect()
    }

    /// Resolves once nothing is queued, downloading or being written back to the volume.
    pub(crate) async fn settled(&self) {
        self.shared.settled().await
    }

    /// The commit `tree`: its repo files, and its header with the components `want` selects.
    pub(crate) async fn pull_tree(&self, origin: &Arc<dyn Origin>, tree: &ObjectRef, want: &Want<'_>) -> Result<Pulled> {
        let mut landed = self.objects(origin, std::slice::from_ref(tree), want).await?;
        let parsed = Tree::from_json(&self.read(tree).await?).map_err(|e| invalid(tree, e))?;
        let header = parsed.header().map_err(|e| invalid(tree, e))?.1.clone();
        let files: Vec<ObjectRef> = parsed.repo_files().map(|(_, r)| r.clone()).collect();
        landed.join(self.objects(origin, &files, want).await?);
        let mut pulled = self.pull_header(origin, &header, want).await?;
        pulled.landed.join(landed);
        pulled.objects += 1 + files.len();
        pulled.tree = Some(tree.clone());
        Ok(pulled)
    }

    /// The cozytensors whose header is `header`, the components `want` selects.
    pub async fn pull_header(
        &self,
        origin: &Arc<dyn Origin>,
        header: &ObjectRef,
        want: &Want<'_>,
    ) -> Result<Pulled> {
        let mut landed = self.objects(origin, std::slice::from_ref(header), want).await?;
        let parsed = Header::from_cbor(&self.read(header).await?).map_err(|e| invalid(header, e))?;
        let wanted = wanted(&parsed, want.components)?;
        landed.join(self.objects(origin, &wanted, want).await?);
        Ok(Pulled {
            tree: None,
            header: header.clone(),
            objects: wanted.len() + 1,
            landed,
        })
    }

    /// Each of `objects` in the store, fetched from `origin` when absent.
    pub async fn objects(
        &self,
        origin: &Arc<dyn Origin>,
        objects: &[ObjectRef],
        want: &Want<'_>,
    ) -> Result<Landed> {
        let mut landed = Landed::default();
        let mut waiting = Vec::new();
        for object in objects {
            if let Some(progress) = want.progress {
                progress.total.fetch_add(object.length, Ordering::Relaxed);
            }
            match self.shared.store.reuse(object).unwrap_or(false) {
                true => {
                    landed.add(Tier::Local, object.length, 0);
                    if let Some(progress) = want.progress {
                        progress.landed(object);
                    }
                }
                false => {
                    let waiter = self.shared.want(object, origin, want.priority);
                    if let Some(progress) = want.progress {
                        progress.arriving.lock().unwrap().insert(object.sha256.clone(), waiter.task());
                    }
                    waiting.push((object, waiter));
                }
            }
        }
        let landed = Mutex::new(landed);
        // Unordered: the first refusal ends the pull when it comes, not once every object listed
        // before it has landed (try_join_all of more than 30 is ordered).
        let waits: FuturesUnordered<_> = waiting
            .into_iter()
            .map(|(object, waiter)| {
                let landed = &landed;
                async move {
                    let task = waiter.task();
                    let tier = waiter.wait().await?;
                    landed.lock().unwrap().add(tier, object.length, task.kept());
                    if let Some(progress) = want.progress {
                        progress.landed(object);
                    }
                    Ok::<(), Refusal>(())
                }
            })
            .collect();
        waits.try_collect::<()>().await?;
        Ok(landed.into_inner().unwrap())
    }

    async fn read(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        let (store, object) = (Arc::clone(&self.shared.store), object.clone());
        let read = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::with_capacity(object.length as usize);
            volume::open(&store, &object)?.read_to_end(&mut bytes).map(|_| bytes)
        });
        read.await
            .map_err(|error| Refusal {
                code: Code::IO_FAILED,
                detail: error.to_string(),
            })?
            .map_err(|error| Refusal {
                code: Code::IO_FAILED,
                detail: format!("reading an admitted document: {error}"),
            })
    }
}

fn invalid(object: &ObjectRef, error: format::Error) -> Refusal {
    Refusal {
        code: Code::UNKNOWN_FORMAT,
        detail: format!("{}: {error}", object.sha256),
    }
}

/// The chunks the tensors of `components` (every component when `None`) live in, then their
/// assets; each once.
pub fn wanted(header: &Header, components: Option<&[String]>) -> Result<Vec<ObjectRef>> {
    if let Some(name) = components.into_iter().flatten().find(|n| header.component(n).is_none()) {
        let has: Vec<&str> = header.components().iter().map(|c| c.name.as_str()).collect();
        return refuse(Code::REF_NOT_FOUND, format!("no component {name:?} (has {has:?})"));
    }
    crate::store::wanted(header, components).map_err(crate::store::Error::refusal)
}

/// A runtime for callers without one (a CLI, Python): two threads for sockets and timers.
pub fn runtime() -> Result<Arc<tokio::runtime::Runtime>> {
    let mut held = runtime_slot()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(runtime) = held.as_ref() {
        return Ok(Arc::clone(runtime));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tensorfs-fetch")
        .enable_all()
        .build()
        .map_err(|error| Refusal {
            code: Code::TRANSFER_FAILED,
            detail: format!("the fetch runtime could not start: {error}"),
        })?;
    Ok(Arc::clone(held.insert(Arc::new(runtime))))
}

type RuntimeSlot = Mutex<Option<Arc<tokio::runtime::Runtime>>>;

#[cfg(not(unix))]
fn runtime_slot() -> Result<&'static RuntimeSlot> {
    static RUNTIME: RuntimeSlot = Mutex::new(None);
    Ok(&RUNTIME)
}

#[cfg(unix)]
fn runtime_slot() -> Result<&'static RuntimeSlot> {
    process_runtime::slot()
}

/// The POSIX process boundary for the lazy network runtime. An inherited Tokio runtime
/// has no workers in a fork child, and its mutex may be owned by a vanished parent thread.
#[cfg(unix)]
#[allow(unsafe_code)]
mod process_runtime {
    use super::{Code, Refusal, Result, RuntimeSlot};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

    static SLOT: AtomicPtr<RuntimeSlot> = AtomicPtr::new(ptr::null_mut());
    static REGISTERED: AtomicBool = AtomicBool::new(false);

    extern "C" fn after_fork() {
        // Only an atomic store: no allocation, lock, refcount change or destructor is
        // permitted here. Deliberately abandon the inherited holder in this child.
        SLOT.store(ptr::null_mut(), Ordering::Release);
    }

    pub(super) fn slot() -> Result<&'static RuntimeSlot> {
        if !REGISTERED.load(Ordering::Acquire) {
            // SAFETY: after_fork has a static C ABI and touches only the lock-free pointer.
            // Register before publishing a holder. There is deliberately no "initializing"
            // state or Once lock for a concurrent fork to inherit. Racing first callers may
            // register the same idempotent callback; every registration completes before use.
            let error = unsafe { libc::pthread_atfork(None, None, Some(after_fork)) };
            if error != 0 {
                return Err(Refusal {
                    code: Code::TRANSFER_FAILED,
                    detail: format!("the fetch runtime fork hook could not register: {}", std::io::Error::from_raw_os_error(error)),
                });
            }
            REGISTERED.store(true, Ordering::Release);
        }
        // The registered handler itself is inherited, so REGISTERED stays true in children.
        // Resetting the pointer on every fork also covers descendant forks and PID reuse;
        // no process identity comparison can mistake an ancestor's holder for a live one.
        let mut slot = SLOT.load(Ordering::Acquire);
        if slot.is_null() {
            let candidate: *mut RuntimeSlot = Box::into_raw(Box::new(RuntimeSlot::new(None)));
            slot = match SLOT.compare_exchange(ptr::null_mut(), candidate, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => candidate,
                Err(published) => {
                    // SAFETY: a losing candidate was never published and contains no runtime.
                    // Its allocation belongs only to this thread in the current process.
                    unsafe { drop(Box::from_raw(candidate)); }
                    published
                }
            };
        }
        // SAFETY: published holders are never freed. Threads in this process share its
        // mutex; a fork child clears SLOT before returning to callers and never locks or
        // drops the inherited holder. Keeping its Arc also prevents an inherited temporary
        // Arc from being the last owner and dropping the parent-created Tokio runtime.
        Ok(unsafe { &*slot })
    }
}
