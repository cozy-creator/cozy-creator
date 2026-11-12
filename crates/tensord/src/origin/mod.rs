//! Where the machine's checkpoints come from, and the one downloader that brings them
//! (TensorFS's `Fetcher`, design §5): tiers local store → datacenter volume → origin, whole-object
//! GETs, every object verified as the store admits it. TensorD's threads are synchronous; each
//! call here blocks on the fetch runtime and ends early when its [`Stop`] is cancelled.
//!
//! - [`hub`]: TensorFS's distribution `Endpoint` at the run's Hub, under its request's capability.
mod hub;

use std::{
    future::Future,
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use tensorfs::{
    err::{refuse, Code, Refusal, Result},
    fetch::{Client, Config, Fetcher, Landed, Origin, Progress, Pulled, SourcePolicy, VolumeOrigin, Want},
    format::ObjectRef,
    store::Store,
};

pub(crate) use hub::{object, remote, resolve};

use crate::stop::Stop;

/// How long the datacenter volume keeps what nobody reads.
const VOLUME_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How often a blocked call looks at its stop.
const TICK: Duration = Duration::from_millis(100);

/// The machine's one download line, smaller first (cl-905): what a reader is blocked on, then each
/// run's needs in job order (acceptance, as its GPU work queues), then what jobs waiting on another's
/// output will read, then guesses, then warm sets. The downloader finishes one before the next.
pub(crate) mod priority {
    /// A reader is blocked on it.
    pub(crate) const NOW: u64 = 0;
    /// The order of what a started run fetches (its calls' weights): ahead of every queued root,
    /// whose order is its acceptance, from 1.
    pub(crate) const STARTED: u64 = 0;
    fn at(class: u64, run: u64, rank: u32) -> u64 {
        1 + (class << 62 | run.min((1 << 30) - 1) << 32 | u64::from(rank))
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
    /// Kept for later (a warm set's `downloaded`).
    pub(crate) const BEHIND: u64 = u64::MAX;
}

/// A checkpoint's origin, with the CAs its hosts may present beyond the public roots (a PEM
/// bundle) and whether its deployment declared itself local (loopback plaintext allowed).
#[derive(Clone)]
pub(crate) struct Remote {
    pub origin: Arc<dyn Origin>,
    pub trust: Option<Arc<Vec<u8>>>,
    pub local: bool,
}

type Key = (Option<Arc<Vec<u8>>>, bool);

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
}

impl Machine {
    /// Fetches into `store`, reading the datacenter volume mounted at `volume` first.
    pub(crate) fn new(store: Arc<Store>, volume: Option<&Path>) -> io::Result<Arc<Machine>> {
        let runtime = tensorfs::fetch::runtime().map_err(io::Error::other)?;
        let volume = volume.and_then(|path| VolumeOrigin::mounted(path, VOLUME_TTL));
        Ok(Arc::new(Machine { store, runtime, volume, fetchers: Mutex::default() }))
    }

    pub(crate) fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// The client objects of a remote with this trust are fetched over (for an `Endpoint`).
    pub(crate) fn client(&self, trust: Option<&Arc<Vec<u8>>>, local: bool) -> Result<Client> {
        Ok(self.fetcher(&(trust.cloned(), local))?.client().clone())
    }

    fn fetcher(&self, key: &Key) -> Result<Arc<Fetcher>> {
        let mut fetchers = self.fetchers.lock().unwrap();
        if let Some((_, fetcher)) = fetchers.iter().find(|(k, _)| k == key) {
            return Ok(fetcher.clone());
        }
        // Every object is held to its digest: no host vouches for anything, addresses are
        // still checked.
        let policy = SourcePolicy { any_host: true, allow_local: key.1, max_redirects: 8, ..Default::default() };
        let client = match &key.0 {
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

    fn of(&self, remote: &Remote) -> Result<Arc<Fetcher>> {
        self.fetcher(&(remote.trust.clone(), remote.local))
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
            let stopped = async {
                let mut tick = tokio::time::interval(TICK);
                while !stop.is_cancelled() {
                    tick.tick().await;
                }
            };
            let answered = tokio::select! {
                answered = work => answered,
                () = stopped => refuse(Code::TRANSFER_FAILED, "stopped"),
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
            let structure = fetcher.pull_header(&origin, &header, &want(Some(&[]))).await?;
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
    pub(crate) fn all(&self, wants: Vec<(Remote, Vec<ObjectRef>, u64)>, progress: Arc<Progress>, stop: &Stop) -> Result<Landed> {
        let wants = (wants.into_iter())
            .map(|(remote, objects, priority)| Ok((self.of(&remote)?, remote.origin, objects, priority)))
            .collect::<Result<Vec<_>>>()?;
        self.block(stop, async move {
            let lines: Vec<Want> = wants.iter().map(|(.., priority)| Want { components: None, priority: *priority, progress: Some(progress.as_ref()) }).collect();
            let each = wants.iter().zip(&lines).map(|((fetcher, origin, objects, _), want)| fetcher.objects(origin, objects, want));
            let mut landed = Landed::default();
            for one in futures_util::future::try_join_all(each).await? {
                (landed.origin, landed.volume, landed.local) = (landed.origin + one.origin, landed.volume + one.volume, landed.local + one.local);
            }
            Ok(landed)
        })
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
        Ok(self.runtime.spawn(async move {
            let mut completion = completion;
            let result = fetcher.objects(&origin, &objects, &Want { priority, ..Want::default() }).await;
            if let Some(settled) = completion.0.take() { settled(&result); }
            result
        }))
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
