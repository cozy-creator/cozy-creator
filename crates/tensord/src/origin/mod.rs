//! Where the machine's checkpoints come from, and the one downloader that brings them
//! (TensorFS's `Fetcher`, design §5): tiers local store → datacenter volume → origin, whole-object
//! GETs, every object verified as the store admits it. TensorD's threads are synchronous; each
//! call here blocks on the fetch runtime and ends early when its [`Stop`] is cancelled.
//!
//! - [`hub`]: TensorFS's distribution `Endpoint` at the run's Hub, under its request's capability.
mod hub;

use std::{
    collections::HashMap,
    future::Future,
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use tensorfs::{
    err::{refuse, Code, Refusal, Result},
    fetch::{Client, Config, Fetcher, Landed, Origin, Progress, Pulled, SourcePolicy, VolumeOrigin, Want},
    format::{Header, ObjectRef, Tree},
    store::Store,
};

pub(crate) use hub::{object, remote, resolve};

use crate::stop::Stop;

/// How long the datacenter volume keeps what nobody reads.
const VOLUME_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The machine's one download line, smaller first (cl-905): what a reader is blocked on, then each
/// run's needs in job order (acceptance, as its GPU work queues), then what jobs waiting on another's
/// output will read, then guesses, then what a caller's prepare list names in that list's order, then warm
/// sets. The downloader finishes one before the next. Everything after a run's needs is background
/// (`tensorfs::fetch::BACKGROUND`): none of it starts, and what is in flight gives way, while a need is under
/// way (cl-1101).
pub(crate) mod priority {
    // A run's needs are class 0; `ahead` (class 1) is the first background order.
    const _: () = assert!(2 + (1 << 62) == tensorfs::fetch::BACKGROUND);
    /// What a started call's function reads now and isn't here yet (`ModelNeed`): before every weight (cl-966).
    pub(crate) const EMERGENCY: u64 = tensorfs::fetch::EMERGENCY;
    /// Software, no job's weights: the machine's update, a package's files as it installs. Weights yield to it (cl-1007).
    pub(crate) const SOFTWARE: u64 = tensorfs::fetch::SOFTWARE;
    /// The run a call sharing a GPU an ancestor holds is ordered as (`runs::call_order`): ahead of
    /// every queued root, whose order is its acceptance, from 1.
    pub(crate) const STARTED: u64 = 0;
    fn at(class: u64, run: u64, rank: u32) -> u64 {
        2 + (class << 62 | run.min((1 << 30) - 1) << 32 | u64::from(rank))
    }
    /// Next for `run` (by acceptance), `rank` within it.
    pub(crate) fn next(run: u64, rank: u32) -> u64 {
        at(0, run, rank)
    }
    /// What a job waiting on another's output reads, fetched ahead by `run` (a project's warm run):
    /// behind every run's, as that job's own run will be when it is sent.
    pub(crate) fn ahead(run: u64, rank: u32) -> u64 {
        at(1, run, rank)
    }
    /// What `run` may read (an orchestrator's internal functions'): after every sure need.
    pub(crate) fn guess(run: u64, rank: u32) -> u64 {
        at(2, run, rank)
    }
    /// What a caller expects demand for here with no job yet (`PUT /v1/prepare`, cl-967): after every guess, at its item's `place`
    /// in the line (`ahead::place`, cl-1278: its list's order), `rank` within it. What an item may read (`extra`) goes behind what
    /// every item needs.
    pub(crate) fn prepare(place: u64, extra: bool, rank: u32) -> u64 {
        at(3, u64::from(extra) << 29 | place.min(LAST), rank)
    }
    /// The last place in line a prepare item takes.
    pub(crate) const LAST: u64 = (1 << 29) - 1;
    /// Kept for later (a warm set's `downloaded`).
    pub(crate) const BEHIND: u64 = u64::MAX;
    /// What a resolve or a plan reads of a checkpoint at `order` (its header and structure, a few KiB): a run's goes ahead of every
    /// run's weights, as software does, so a cold run's resolve never waits behind another run's download (cl-1185, PRO6-37's 29 s
    /// behind 52 GB of H3). A background order's (fetched ahead, a guess, a prepare, kept) stays in its band: it never jumps a run.
    pub(crate) fn metadata(order: u64) -> u64 {
        match order < tensorfs::fetch::BACKGROUND {
            true => order.min(SOFTWARE),
            false => order,
        }
    }
    /// Whether `order` is a caller's guess's (`prepare`, or kept for later): no run's, so it gives way to every run's, in the
    /// install line too (cl-1104).
    pub(crate) fn prefetch(order: u64) -> bool {
        order.saturating_sub(2) >> 62 == 3
    }
}

/// A checkpoint's origin, with the CAs its hosts may present beyond the public roots (a PEM
/// bundle).
#[derive(Clone)]
pub(crate) struct Remote {
    pub origin: Arc<dyn Origin>,
    pub trust: Option<Arc<Vec<u8>>>,
}

/// A fetcher per trust: the roots beyond the public ones.
type Key = Option<Arc<Vec<u8>>>;

/// Task cancellation or panic must wake a queued owner too, rather than leaving it waiting
/// forever for an ordinary result callback. This owns no work beyond the existing fetch.
struct FetchCompletion<F: FnOnce(&Result<Landed>)>(Option<F>);
impl<F: FnOnce(&Result<Landed>)> Drop for FetchCompletion<F> {
    fn drop(&mut self) {
        if let Some(settled) = self.0.take() {
            settled(&Err(Refusal {
                code: Code::IO_FAILED,
                detail: "source fetch task ended before reporting completion".into(),
            }));
        }
    }
}

/// The machine's downloader: one `Fetcher` per trust (in practice one).
pub(crate) struct Machine {
    store: Arc<Store>,
    runtime: Arc<tokio::runtime::Runtime>,
    volume: Option<VolumeOrigin>,
    fetchers: Mutex<Vec<(Key, Arc<Fetcher>)>>,
    near: Mutex<Near>,
}

/// What the datacenter volume holds of what was asked (cl-1282): objects by sha256 (`Machine::near`) and headers by id
/// (`Machine::header`), None while being looked at. Each as when first looked at since the store last evicted: what the store
/// evicts may be on the volume by now, written there from here.
#[derive(Default)]
struct Near {
    since: Option<SystemTime>,
    objects: HashMap<String, Option<bool>>,
    headers: HashMap<String, Option<Option<Arc<(ObjectRef, Header)>>>>,
    asked: (Vec<ObjectRef>, Vec<String>),
    looking: bool,
}

/// Checkpoint `id`'s header as `volume` holds it: `id` names it, or a tree that names it. None: given up on; Some(None): not there.
async fn drive_header(volume: &VolumeOrigin, id: &str) -> Option<Option<Arc<(ObjectRef, Header)>>> {
    let Some(bytes) = volume.document(id.strip_prefix("sha256:").unwrap_or(id)).await? else { return Some(None) };
    let named = Tree::from_json(&bytes).ok().and_then(|tree| tree.header().ok().map(|(_, header)| header.clone()));
    let (header, bytes) = match named {
        Some(header) => match volume.document(&header.sha256).await? {
            Some(bytes) => (header, bytes),
            None => return Some(None),
        },
        None => (ObjectRef::of(&bytes), bytes),
    };
    Some(Header::from_cbor(&bytes).ok().map(|parsed| Arc::new((header, parsed))))
}

impl Machine {
    /// Fetches into `store`, reading the datacenter volume mounted at `volume` first.
    pub(crate) fn new(store: Arc<Store>, volume: Option<&Path>) -> io::Result<Arc<Machine>> {
        let runtime = tensorfs::fetch::runtime().map_err(io::Error::other)?;
        let volume = volume.and_then(|path| VolumeOrigin::mounted(path, VOLUME_TTL));
        Ok(Arc::new(Machine { store, runtime, volume, fetchers: Mutex::default(), near: Mutex::default() }))
    }

    pub(crate) fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// The client objects of a remote with this trust are fetched over (for an `Endpoint`).
    pub(crate) fn client(&self, trust: Option<&Arc<Vec<u8>>>) -> Result<Client> {
        Ok(self.fetcher(&trust.cloned())?.client().clone())
    }

    fn fetcher(&self, key: &Key) -> Result<Arc<Fetcher>> {
        let mut fetchers = self.fetchers.lock().unwrap();
        if let Some((_, fetcher)) = fetchers.iter().find(|(k, _)| k == key) {
            return Ok(fetcher.clone());
        }
        // Every object is held to its digest: no host vouches for anything, addresses are
        // still checked, at every hop (a local one only at an origin this computer declared).
        let policy = SourcePolicy { any_host: true, max_redirects: 8, ..Default::default() };
        let client = match key {
            Some(pem) => Client::with_roots(policy, pem)?,
            None => Client::new(policy),
        };
        let _entered = self.runtime.enter();
        let store = self.store.clone();
        let fetcher = Arc::new(Fetcher::new(store, client, self.volume.clone(), Config::default()));
        fetchers.push((key.clone(), fetcher.clone()));
        Ok(fetcher)
    }

    /// The bytes every fetcher received from its origin, and the time any of them was arriving.
    pub(crate) fn received(&self) -> (u64, Duration) {
        self.fetchers.lock().unwrap().iter().map(|(_, fetcher)| fetcher.stats()).fold(
            (0, Duration::ZERO),
            |(bytes, time), stats| (bytes + stats.received, time + stats.receiving),
        )
    }

    /// The bytes every fetcher read off the datacenter volume into the store, and the time any of them was coming. None: no volume.
    pub(crate) fn volume_read(&self) -> Option<(u64, Duration)> {
        self.volume.as_ref()?;
        Some(self.fetchers.lock().unwrap().iter().map(|(_, fetcher)| fetcher.stats()).fold(
            (0, Duration::ZERO),
            |(bytes, time), stats| (bytes + stats.volume_read, time + stats.volume_reading),
        ))
    }

    /// Of `absent`, objects the store lacks, the bytes the datacenter volume holds (cl-1282): a fetch reads them there, not from
    /// their origin. Answered from what it has looked at; the rest is looked at behind this answer (`look`). Nothing waits on the
    /// volume here.
    pub(crate) fn near(self: &Arc<Self>, absent: &[ObjectRef]) -> u64 {
        let Some(mut near) = self.looked() else { return 0 };
        let bytes = absent.iter().filter(|object| near.objects.get(&object.sha256) == Some(&Some(true))).map(|object| object.length).sum();
        for object in absent {
            if !near.objects.contains_key(&object.sha256) {
                near.objects.insert(object.sha256.clone(), None);
                near.asked.0.push(object.clone());
            }
        }
        self.look(near);
        bytes
    }

    /// Checkpoint `id`'s header, gone from the store, as the datacenter volume holds it (cl-1282): what the volume holds of a
    /// checkpoint a function read here can then be said after a restart. Looked at behind, as `near`.
    pub(crate) fn header(self: &Arc<Self>, id: &str) -> Option<Arc<(ObjectRef, Header)>> {
        let mut near = self.looked()?;
        let known = match near.headers.get(id) {
            Some(known) => known.clone().flatten(),
            None => {
                near.headers.insert(id.to_owned(), None);
                near.asked.1.push(id.to_owned());
                None
            }
        };
        self.look(near);
        known
    }

    /// What the volume was seen to hold, none of it from before the store last evicted. None: no volume answers.
    fn looked(&self) -> Option<std::sync::MutexGuard<'_, Near>> {
        self.volume.as_ref().filter(|volume| volume.answering())?;
        let mut near = self.near.lock().unwrap();
        let evicted = self.store.evicted_at();
        if near.since != evicted {
            *near = Near { since: evicted, looking: near.looking, ..Near::default() };
        }
        Some(near)
    }

    /// What was asked, looked at on the volume until nothing is, then Status told once. A look given up on forgets what it asked.
    fn look(self: &Arc<Self>, mut near: std::sync::MutexGuard<'_, Near>) {
        let Some(volume) = self.volume.clone() else { return };
        if near.looking || (near.asked.0.is_empty() && near.asked.1.is_empty()) {
            return;
        }
        near.looking = true;
        drop(near);
        let machine = self.clone();
        self.runtime.spawn(async move {
            loop {
                let (objects, ids) = {
                    let mut near = machine.near.lock().unwrap();
                    if near.asked.0.is_empty() && near.asked.1.is_empty() {
                        near.looking = false;
                        break;
                    }
                    std::mem::take(&mut near.asked)
                };
                let held = volume.holds(&objects).await;
                let mut headers = vec![];
                for id in ids.iter().filter(|_| held.is_some()) {
                    headers.push(drive_header(&volume, id).await);
                }
                let mut near = machine.near.lock().unwrap();
                let gone = held.is_none() || headers.len() < ids.len() || headers.iter().any(Option::is_none);
                near.objects.extend(objects.into_iter().map(|object| object.sha256).zip(held.unwrap_or_default().into_iter().map(Some)));
                near.headers.extend(ids.into_iter().zip(headers).filter_map(|(id, header)| Some((id, Some(header?)))));
                if gone {
                    near.objects.retain(|_, held| held.is_some());
                    near.headers.retain(|_, header| header.is_some());
                    (near.asked, near.looking) = (Default::default(), false);
                    break;
                }
            }
            crate::held_models::changed();
            crate::activity::status_changed();
        });
    }

    fn of(&self, remote: &Remote) -> Result<Arc<Fetcher>> {
        self.fetcher(&remote.trust)
    }

    /// Each of `objects`' download from `remote` as it stands (`Fetching`); None for one with no
    /// task open.
    pub(crate) fn fetching(&self, remote: &Remote, objects: &[ObjectRef]) -> Vec<Option<tensorfs::fetch::Fetching>> {
        self.of(remote).map_or_else(|_| vec![None; objects.len()], |fetcher| fetcher.fetching(objects))
    }

    /// Runs `work` on the fetch runtime and waits for it, or for `stop`.
    pub(crate) fn block<T: Send + 'static>(&self, stop: &Stop, work: impl Future<Output = Result<T>> + Send + 'static) -> Result<T> {
        let (sent, answer) = std::sync::mpsc::channel();
        let stop = stop.clone();
        self.runtime.spawn(async move {
            let answered = tokio::select! {
                answered = work => answered,
                () = stop.cancelled() => refuse(Code::TRANSFER_FAILED, "stopped"),
            };
            let _ = sent.send(answered);
        });
        answer.recv().unwrap_or_else(|_| refuse(Code::TRANSFER_FAILED, "the fetch runtime ended"))
    }

    /// The cozytensors whose header is `header`, the `components` given (all when `None`): its
    /// header, then the chunks and assets the components read (the store refuses what its
    /// disk cannot take).
    pub(crate) fn pull(
        &self,
        remote: &Remote,
        header: &ObjectRef,
        components: Option<&[String]>,
        priority: u64,
        progress: Option<Arc<Progress>>,
        stop: &Stop,
    ) -> Result<Pulled> {
        let (fetcher, origin) = (self.of(remote)?, remote.origin.clone());
        let (header, components) = (header.clone(), components.map(<[String]>::to_vec));
        self.block(stop, async move {
            let want = |components| Want { components, priority, progress: progress.as_deref() };
            // Its structure is what a resolve reads: ahead of the weights in line (`priority::metadata`).
            let shape = Want { components: Some(&[]), priority: priority::metadata(priority), progress: progress.as_deref() };
            let structure = fetcher.pull_header(&origin, &header, &shape).await?;
            if components.as_deref() == Some(&[]) {
                return Ok(structure);
            }
            let mut pulled = fetcher.pull_header(&origin, &header, &want(components.as_deref())).await?;
            (pulled.landed.origin, pulled.landed.volume) =
                (pulled.landed.origin + structure.landed.origin, pulled.landed.volume + structure.landed.volume);
            Ok(pulled)
        })
    }

    /// Every one of `wants` (objects from a remote, at an order) here, fetched when absent, all in
    /// line at once: the downloader takes them by their orders. Their bytes count into `progress`.
    /// `lined` hears once each absent one is in the downloader's line, before any is waited for.
    pub(crate) fn all(&self, wants: Vec<(Remote, Vec<ObjectRef>, u64)>, progress: Arc<Progress>, lined: &dyn Fn(), stop: &Stop) -> Result<Landed> {
        let wants = (wants.into_iter())
            .map(|(remote, objects, priority)| Ok((self.of(&remote)?, remote.origin, objects, priority)))
            .collect::<Result<Vec<_>>>()?;
        let mut all = Box::pin(async move {
            let lines: Vec<Want> = wants.iter().map(|(.., priority)| Want { components: None, priority: *priority, progress: Some(progress.as_ref()) }).collect();
            let each = wants.iter().zip(&lines).map(|((fetcher, origin, objects, _), want)| fetcher.objects(origin, objects, want));
            let mut landed = Landed::default();
            for one in futures_util::future::try_join_all(each).await? {
                (landed.origin, landed.volume, landed.local) = (landed.origin + one.origin, landed.volume + one.volume, landed.local + one.local);
            }
            Ok::<_, Refusal>(landed)
        });
        let first = self.line(all.as_mut());
        lined();
        match first {
            std::task::Poll::Ready(landed) => landed,
            std::task::Poll::Pending => self.block(stop, all),
        }
    }

    /// `wanted` polled once, here: a fetch wants each of its absent objects at its first poll (`Fetcher::objects`), so they are
    /// in the downloader's line when this returns, and a want that takes over another's objects never finds one unwanted in
    /// between (cl-1104, `ahead::put`). What it waits on from here is woken on the runtime it is then given to.
    fn line<T>(&self, wanted: std::pin::Pin<&mut (impl Future<Output = T> + ?Sized)>) -> std::task::Poll<T> {
        let _entered = self.runtime.enter();
        wanted.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    }

    /// Each of `objects` here, fetched from `remote` when absent.
    pub(crate) fn objects(&self, remote: &Remote, objects: &[ObjectRef], priority: u64, stop: &Stop) -> Result<Landed> {
        let (fetcher, origin) = (self.of(remote)?, remote.origin.clone());
        let objects = objects.to_vec();
        self.block(stop, async move {
            fetcher.objects(&origin, &objects, &Want { priority, ..Want::default() }).await
        })
    }

    /// Wants `objects` at `priority` until the returned handle is aborted or they land.
    pub(crate) fn want(&self, remote: &Remote, objects: Vec<ObjectRef>, priority: u64) -> Result<tokio::task::JoinHandle<Result<Landed>>> {
        self.want_observed(remote, objects, priority, |_| {})
    }

    /// The same downloader, with one completion notification to the existing dispatch loop.
    pub(crate) fn want_observed(
        &self, remote: &Remote, objects: Vec<ObjectRef>, priority: u64,
        settled: impl FnOnce(&Result<Landed>) + Send + 'static,
    ) -> Result<tokio::task::JoinHandle<Result<Landed>>> {
        let (fetcher, origin) = (self.of(remote)?, remote.origin.clone());
        let completion = FetchCompletion(Some(settled));
        let mut wanted = Box::pin(async move {
            let mut completion = completion;
            let result = fetcher.objects(&origin, &objects, &Want { priority, ..Want::default() }).await;
            if let Some(settled) = completion.0.take() { settled(&result); }
            result
        });
        Ok(match self.line(wanted.as_mut()) {
            std::task::Poll::Ready(result) => self.runtime.spawn(async move { result }),
            std::task::Poll::Pending => self.runtime.spawn(wanted),
        })
    }
}

/// Runs `work`, telling `report` the bytes `progress` has of its total every quarter second
/// meanwhile, and once more at the end.
pub(crate) fn watched<T>(progress: &Progress, report: &(dyn Fn(u64, u64) + Sync), work: impl FnOnce() -> T) -> T {
    use std::sync::atomic::Ordering::Relaxed;
    let done = Stop::default();
    std::thread::scope(|scope| {
        scope.spawn(|| loop {
            report(progress.done(), progress.total.load(Relaxed));
            if done.is_cancelled() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        });
        let answer = work();
        done.cancel();
        answer
    })
}
