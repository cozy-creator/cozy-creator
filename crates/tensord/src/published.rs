//! Published packages and their models, prepared from a Hub by name (th-245) and reused: a
//! run whose installation and model resolution are held reads nothing from the Hub. Port of the
//! Python worker's release-root preparation (package set from the release's locked
//! requirements with uv; each model's commit as the Hub resolves it for the machine's hardware,
//! then a TensorFS download) with its `release_root_preparing` progress answers.
pub(crate) mod environment;
use crate::{
    api::domain,
    gpu_service::{Plan, Pool, ModelGrant},
    hub::{self, Catalog},
    journal::{Installation, Preparation},
    objects::Refused,
    service::Service,
};
use base64::Engine as _;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};
use crate::{
    held_models::Pulled,
    origin::{self, priority, Machine},
    stop::Stop,
    storage::{self, Store},
};
use tensorfs::{
    err::Refusal,
    fetch::{Hardware, Landed, Progress as Counted},
    format::ObjectRef,
    sha256,
};
use crate::catalog::normalized;

/// How each published environment gets its Runtime SDK. Empty `requirements` keeps the
/// release's own locked SDK rows.
#[derive(Clone, Debug)]
pub struct PackageSdk {
    pub uv: crate::machine::uv::Uv,
    pub python: String,
    pub requirements: Vec<String>,
    pub find_links: Option<PathBuf>,
    /// The development Runtime an update named by its wheel file (`0.24.6+e9baf7a4`): the
    /// operator's choice, which runs every package whatever its own bounds on the pair.
    pub runtime_build: String,
}

impl PackageSdk {
    /// Executors use the selected Runtime/TensorFS pair (an update's, else the image's), so they
    /// speak this machine's protocol; without a complete pair, each release's locked SDK.
    pub fn selected(wheels: &Path, uv: crate::machine::uv::Uv) -> Self {
        // An activation replaces sdk/current, not the immutable candidate directory. Freeze
        // that selection once: both cache identity and the installer's actual wheel inputs
        // must name the same build even when filenames/version labels repeat.
        let wheels = platform::durable::canonical(wheels).unwrap_or_else(|_| wheels.to_path_buf());
        let found: Vec<PathBuf> = fs::read_dir(&wheels)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "whl"))
            .collect();
        let named = |prefix: &str| {
            found.iter().any(|w| w.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix)))
        };
        let complete = named("cozy_runtime-") && named("tensorfs-");
        let mut pair: Vec<String> = found.iter().map(|path| path.display().to_string()).collect();
        pair.sort();
        Self {
            uv,
            python: "3.12".into(),
            runtime_build: match complete {
                true => fs::read_to_string(wheels.join(crate::machine::update::RUNTIME_BUILD)).unwrap_or_default(),
                false => String::new(),
            },
            find_links: complete.then_some(wheels),
            requirements: if complete { pair } else { vec![] },
        }
    }
}

pub struct Prepared {
    pub installation: Installation,
    pub plan: Option<Plan>,
    /// Why the Hub passed over a better lane for the run, held with its resolution.
    pub warnings: Vec<String>,
}

pub struct Request {
    /// The run this prepares for: a run waiting on its install is told so by this id.
    pub run: String,
    /// The run's Hub: a release and a Hub-named model need it; held code and models do not.
    pub source: Option<hub::Source>,
    pub package: String,
    /// Empty: the Hub's newest, as it named it under `catalog_revision`.
    pub release: String,
    /// Unpublished code this machine already holds, in place of the release.
    pub installed: Option<Installation>,
    /// The account an unpublished package's org-relative model names belong to.
    pub owner: String,
    /// The caller's catalog revision: a held resolution is reused only under the same one.
    pub catalog_revision: String,
    /// Provider tokens for source models (memory only).
    pub providers: Providers,
    pub entrypoint: String,
    /// Only explicitly marked fields enter the model resolution key.
    pub payload: Option<Value>,
    pub choices: Vec<domain::ModelChoice>,
    /// 0 selects the widest supported machine group; otherwise exactly this many GPUs.
    pub gpus: u32,
    /// The main model's lane; empty lets the Hub pick for the hardware.
    pub lane: String,
}

impl Request {
    /// The run's Hub, for `what`: refused when the run carries no Hub access.
    fn hub(&self, what: &str) -> Result<&hub::Source, Failure> {
        let absent = || ("hub_access_absent", format!("{what} needs the run's Hub access, and this run carries none"));
        self.source.as_ref().ok_or_else(absent)
    }

    /// Its Hub's origin key; empty without one.
    fn origin(&self) -> String {
        self.source.as_ref().and_then(|source| hub::origin_key(&source.origin)).unwrap_or_default()
    }
}

/// A Hub commit as a run names it (th-245): `repository` at `release` (empty: the newest) in
/// `lane` (empty: the Hub's pick for the hardware), or by hash (`tree` and no release): a commit,
/// or a bare header (no tree). Resolved, it carries the release, lane and commit the Hub named,
/// that commit's header (the checkpoint's id on this machine), and why a better lane was passed
/// over.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Named {
    pub repository: String,
    pub release: String,
    pub lane: String,
    /// The commit; a run's hash names one before it resolves (its length 0 when unsaid).
    pub tree: Option<ObjectRef>,
    /// The commit's cozytensors header, once resolved.
    pub header: Option<ObjectRef>,
    pub reasons: Vec<String>,
}

impl Named {
    pub(crate) fn tree(&self) -> Result<&ObjectRef, Failure> {
        self.tree.as_ref().ok_or_else(|| unresolved(self))
    }

    pub(crate) fn header(&self) -> Result<&ObjectRef, Failure> {
        self.header.as_ref().ok_or_else(|| unresolved(self))
    }

    /// `repository@release lane`, or `repository@sha256:… lane` for a commit by hash.
    fn shown(&self) -> String {
        let at = match (&self.release, &self.tree) {
            (release, _) if !release.is_empty() => release.clone(),
            (_, Some(tree)) => tree.id(),
            _ => "newest".into(),
        };
        format!("{}@{at} {}", self.repository, self.lane).trim_end().to_owned()
    }
}

fn unresolved(named: &Named) -> Failure {
    ("model_resolution_failed", format!("{} is not resolved", named.shown()))
}

/// The caller's provider tokens; empty reads public sources anonymously.
#[derive(Clone, Debug, Default)]
pub struct Providers {
    pub huggingface: String,
    pub civitai: String,
}

/// Where a preparation's stage and bytes are reported as they change.
pub type Observer = Box<dyn Fn(&str, u64, u64) + Send + Sync>;

#[derive(Default)]
pub(crate) struct Job {
    /// The stage its byte counts are reported under.
    pub(crate) stage: Mutex<String>,
    pub(crate) observer: Option<Observer>,
    /// Its run's stop: a cancel ends its downloads and installs.
    pub(crate) stop: Option<Stop>,
    /// Its run as its caller names it, and how that run is told what it waits for
    /// (`Engine::waiting`).
    pub(crate) caller: String,
    pub(crate) waiting: Option<Box<dyn Fn(Option<String>) + Send + Sync>>,
}
impl Job {
    fn set(&self, stage: String, moved: u64, total: u64) {
        if let Some(observe) = &self.observer {
            observe(&stage, moved, total);
        }
        *self.stage.lock().unwrap() = stage;
    }
    fn waiting(&self, reason: Option<String>) {
        if let Some(waiting) = &self.waiting {
            waiting(reason);
        }
    }
    pub(crate) fn stage(&self, stage: String) {
        self.set(stage, 0, 0);
    }
    fn bytes(&self, moved: u64, total: u64) {
        if let Some(observe) = &self.observer {
            observe(&self.stage.lock().unwrap(), moved, total);
        }
    }
}

pub(crate) type Failure = (&'static str, String);

/// A preparation its run's cancel stopped.
pub const CANCELED: &str = "preparation_canceled";
fn canceled() -> Failure {
    (CANCELED, "the run was canceled while it prepared".into())
}

/// What one environment generation is built from: the release's card (its exact pyproject and
/// uv.lock, its carried wheels, Python and interface), what its lock leaves to this machine, and
/// where the release lives.
#[derive(Clone, Copy)]
struct Environment<'a> {
    card: &'a Card,
    locked: &'a Locked,
    package: &'a str,
    release: &'a str,
    source: &'a hub::Source,
    /// What the person sees while it is built (`install_stage`), its steps counted after it.
    stage: &'a str,
}

pub struct Publisher {
    pub(crate) sdk: PackageSdk,
    machine: Arc<Machine>,
    store: Arc<Store>,
    /// One maker per source: a second asker waits, then finds it held (runs 5393/5394).
    making: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// The run building each environment identity, which a second run waits for.
    building: Mutex<HashMap<String, String>>,
    /// The machine's software updates: a package needing a newer Runtime gets one (`environment`).
    updates: Option<Arc<crate::machine::update::Updates>>,
}

/// An identity's builder, forgotten however the build ends.
struct Building<'a>(&'a Mutex<HashMap<String, String>>, String);
impl Drop for Building<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}

impl Publisher {
    pub fn new(sdk: PackageSdk, machine: Arc<Machine>, updates: Option<Arc<crate::machine::update::Updates>>) -> Arc<Self> {
        Arc::new(Self {
            sdk,
            store: machine.store().clone(),
            machine,
            making: Mutex::new(HashMap::new()),
            building: Mutex::new(HashMap::new()),
            updates,
        })
    }

    /// One run's preparation on the calling thread, reported through `observe`: a held
    /// installation and resolution answer at once.
    pub fn prepare_now(
        &self,
        service: &Arc<Service>,
        actor: &str,
        request: &Request,
        observe: Option<Observer>,
        stop: Option<&Stop>,
    ) -> Result<Prepared, Failure> {
        if let Ok(Some(prepared)) = self.held(service, actor, request) {
            return Ok(prepared);
        }
        let engine = service.engine.clone();
        let run = request.run.clone();
        let job = Job {
            observer: observe,
            stop: stop.cloned(),
            caller: service.engine.caller_id(&run),
            waiting: (!run.is_empty()).then(|| {
                Box::new(move |reason| {
                    if let Err(error) = engine.waiting(&run, reason) {
                        tracing::warn!("run {run}: its waiting reason was not shown: {error}");
                    }
                }) as Box<dyn Fn(Option<String>) + Send + Sync>
            }),
            ..Default::default()
        };
        self.work(service, actor, request, &job)
    }

    /// A slot's model made from its provider source (TensorFS `source_model`), kept as the
    /// local repository named by the source and its profiles.
    fn source_grant(
        &self,
        installation: &Installation,
        path: &str,
        choice: &domain::ModelChoice,
        providers: &Providers,
        job: &Job,
    ) -> Result<ModelGrant, Failure> {
        if !choice.repository.is_empty() || choice.manifest.is_some() {
            return Err((
                "model_override_invalid",
                format!("{path} names a source and a catalog checkpoint"),
            ));
        }
        let made = self.make(&choice.source, &choice.profiles, &choice.trained_on, providers, job.stop.as_ref(), &|stage, moved, total| {
            job.set(format!("{stage} for {path}"), moved, total)
        })?;
        job.stage(format!("{} for {path}", made.format));
        Ok(ModelGrant {
            package: installation.package.clone(),
            slot: path.to_string(),
            repository: made.repository,
            release: String::new(),
            lane: String::new(),
            manifest: made.header.id(),
            components: self.components(&made.header.id())?,
            adapted: None,
        })
    }

    /// The component names a held checkpoint's header declares.
    fn components(&self, header: &str) -> Result<Vec<String>, Failure> {
        let checkpoint = storage::checkpoint(&self.store, header).map_err(|e| ("checkpoint_absent", format!("{header}: {e}")))?;
        Ok(checkpoint.header().components().iter().map(|c| c.name.clone()).collect())
    }

    /// A slot's model this machine's store holds, by its header or its tree.
    fn held_grant(&self, installation: &Installation, path: &str, reference: &domain::Ref) -> Result<ModelGrant, Failure> {
        let named = format!("sha256:{}", sha256::hex(&reference.digest));
        let absent = |_| ("checkpoint_absent", format!("{path} names checkpoint {named}, which this machine does not hold"));
        let checkpoint = storage::checkpoint(&self.store, &named).map_err(absent)?;
        Ok(ModelGrant {
            package: installation.package.clone(),
            slot: path.to_string(),
            repository: String::new(),
            release: String::new(),
            lane: String::new(),
            components: checkpoint.header().components().iter().map(|c| c.name.clone()).collect(),
            manifest: checkpoint.id().id(),
            adapted: None,
        })
    }

    /// A checkpoint held here, named by its header: a Hub commit pulled by hash is the request's
    /// only as `commit` serves it; a release, or one made here, is anyone's.
    pub(crate) fn held_for(&self, service: &Service, source: Option<&hub::Source>, header: &ObjectRef) -> Result<(), Failure> {
        let Some(pulled) = Pulled::of(&self.store, header).map_err(io_failure)?.filter(|p| p.release.is_empty()) else {
            return Ok(());
        };
        let named = Named { repository: pulled.repository, lane: pulled.lane, tree: Some(pulled.tree), ..Named::default() };
        match source {
            Some(source) => self.commit(service, source, named).map(drop),
            None => Err(("model_resolution_failed", format!("{} is private, or no such commit: this run carries no Hub access; `cozy login <email>` on the computer that sent it", named.shown()))),
        }
    }

    /// The store models are made and held in.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// The machine's downloader.
    pub fn machine(&self) -> &Arc<Machine> {
        &self.machine
    }

    /// A provider source made into a local model (`make_source`), as a run's refusal.
    pub fn make_source(
        &self,
        source: &str,
        profiles: &[String],
        trained_on: &str,
        providers: &Providers,
        stop: Option<&Stop>,
        progress: &(dyn Fn(&str, u64, u64) + Sync),
    ) -> Result<Made, Refused> {
        self.make(source, profiles, trained_on, providers, stop, progress)
            .map_err(|(code, message)| Refused { code, message })
    }

    /// A provider source made into a local model (`make_source`), one maker per source.
    fn make(&self, source: &str, profiles: &[String], trained_on: &str, providers: &Providers,
        stop: Option<&Stop>, progress: &(dyn Fn(&str, u64, u64) + Sync),
    ) -> Result<Made, Failure> {
        let mut sorted = profiles.to_vec();
        sorted.sort();
        let key = json!([source, sorted, trained_on]).to_string();
        let gate = self.making.lock().unwrap().entry(key.clone()).or_default().clone();
        let made = {
            let _one = gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            make_source(&self.store, source, profiles, trained_on, providers, stop.cloned(), progress)
        };
        let mut making = self.making.lock().unwrap();
        if Arc::strong_count(&gate) == 2 {
            making.remove(&key);
        }
        made
    }

    /// `named` from its Hub into the store, its ref recording the pull: its tree and header, then
    /// the `components` given (all when `None`, none when empty). Answers the bytes that came from
    /// the Hub and from the datacenter volume, and those the store held already (`local`, as the
    /// components' walk found them). Whatever is here moves nothing.
    fn pull(&self, source: &hub::Source, named: &Named, components: Option<&[String]>, stop: Option<&Stop>,
        bytes: &(dyn Fn(u64, u64) + Sync)) -> Result<Landed, Failure> {
        let header = named.header()?;
        let remote = origin::remote(&self.machine, source).map_err(|e| unsigned(source, refused("model_download_failed", e.clone()), &e))?;
        let stop = stop.cloned().unwrap_or_default();
        let failed = |e: Refusal| match stop.is_cancelled() {
            true => canceled(),
            false => unsigned(source, refused("model_download_failed", e.clone()), &e),
        };
        let (started, subject) = (Instant::now(), named.shown());
        let failed = |e: Refusal| {
            storage::transport(&self.store, "pull-failed", &subject, &format!("origin={}: {e}", source.origin));
            failed(e)
        };
        let mut structure = self.machine.pull(&remote, header, Some(&[]), priority::NOW, None, &stop).map_err(failed)?;
        // A header by hash is recorded under a tree naming it alone.
        let tree = match &named.tree {
            Some(tree) => {
                let landed = self.machine.objects(&remote, std::slice::from_ref(tree), priority::NOW, &stop).map_err(failed)?;
                (structure.landed.origin, structure.landed.volume) = (structure.landed.origin + landed.origin, structure.landed.volume + landed.volume);
                structure.landed.local += landed.local;
                tree.clone()
            }
            None => storage::tree_of(&self.store, header).map_err(io_failure)?,
        };
        let logged = |pulled: &tensorfs::fetch::Pulled| {
            let (fetched, reused) = (pulled.landed.origin + pulled.landed.volume, pulled.landed.local);
            let seconds = started.elapsed().as_secs_f64();
            let detail = format!("header={} tree={} objects={} fetched={fetched} reused={reused} seconds={seconds:.1} origin={}",
                header.id(), tree.id(), pulled.objects, source.origin);
            storage::transport(&self.store, "pull", &subject, &detail);
        };
        let pulled = Pulled { repository: named.repository.clone(), release: named.release.clone(), lane: named.lane.clone(), tree: tree.clone() };
        pulled.record(&self.store).map_err(io_failure)?;
        if components == Some(&[]) {
            logged(&structure);
            return Ok(structure.landed);
        }
        let progress = Arc::new(Counted::default());
        let pulled = origin::watched(&progress, bytes, || {
            self.machine.pull(&remote, header, components, priority::NOW, Some(progress.clone()), &stop)
        });
        let pulled = pulled.map_err(failed)?;
        logged(&pulled);
        let landed = pulled.landed;
        Ok(Landed { origin: structure.landed.origin + landed.origin, volume: structure.landed.volume + landed.volume, local: landed.local })
    }

    /// `named` downloaded whole, as a request's weights are: what a caller asked for whole
    /// (`cozy model download`, an adapter).
    fn ensure(&self, source: &hub::Source, named: &Named, stop: Option<&Stop>,
        bytes: &(dyn Fn(u64, u64) + Sync)) -> Result<Landed, Failure> {
        self.pull(source, named, None, stop, bytes)
    }

    /// The Hub checkpoint `header` is, as this machine pulled it (`Pulled`); none for one never
    /// pulled (local).
    fn pulled(&self, header: &ObjectRef) -> io::Result<Option<Named>> {
        Ok(Pulled::of(&self.store, header)?.map(|p| Named {
            repository: p.repository,
            release: p.release,
            lane: p.lane,
            tree: Some(p.tree),
            header: Some(header.clone()),
            reasons: vec![],
        }))
    }

    /// Each Hub checkpoint's header, landed at once: which of its chunks come, and when, its
    /// calls direct (`access`). A held header asks the Hub nothing.
    fn structure(&self, source: &hub::Source, checkpoints: &[Named], job: &Job) -> Result<(), Failure> {
        if checkpoints.is_empty() {
            return Ok(());
        }
        let names: Vec<String> = checkpoints.iter().map(Named::shown).collect();
        job.stage(format!("resolving {}", names.join(" and ")));
        for named in checkpoints {
            self.pull(source, named, Some(&[]), job.stop.as_ref(), &|_, _| ())?;
        }
        Ok(())
    }

    /// The commit `named` is: one by hash as given, a name as the Hub resolves its release and
    /// lane (or, with no lane, `hardware`); then that commit's header. Journaled per catalog
    /// revision and GPUs (free VRAM moves; the first answer stands) so a warm run, across
    /// restarts too, asks the Hub nothing (th-245).
    pub(crate) fn resolve(&self, service: &Service, source: &hub::Source, named: Named, hardware: &Hardware, revision: &str) -> Result<Named, Failure> {
        if named.tree.is_some() {
            return self.commit(service, source, named);
        }
        let origin = hub::origin_key(&source.origin).unwrap_or_default();
        let name = json!(["resolve-v3", origin, named.repository, named.release, named.lane, hardware.gpus, hardware.plain]).to_string();
        let held = service.engine.with_journal(|j| j.named(&name, revision)).map_err(io_failure)?;
        if let Some((release, lane, tree, header, reasons)) = held.and_then(|held| serde_json::from_str(&held).ok()) {
            return Ok(Named { release, lane, tree: Some(tree), header: Some(header), reasons, ..named });
        }
        let r = origin::resolve(&self.machine, source, &named.repository, &named.release, &named.lane, hardware)
            .map_err(|e| unsigned(source, refused("model_resolution_failed", e.clone()), &e))?;
        let answer = json!([r.release, r.lane, r.commit, r.header, r.reasons]).to_string();
        service.engine.with_journal(|j| j.bind_named(&name, revision, &answer)).map_err(io_failure)?;
        Ok(Named { release: r.release, lane: r.lane, tree: Some(r.commit), header: Some(r.header), reasons: r.reasons, ..named })
    }

    /// A commit (or a bare header) named by its hash, with what a hash alone does not say: its
    /// length and format; a commit resolves to its header and lane. A hash may name a private
    /// commit, so what this machine holds of it serves a request only as its account, or a request
    /// with no sign-in only once the Hub served it to one: the readers journaled with it, across
    /// restarts like the store. Any other request asks the Hub once, as itself (its capability's
    /// account, as the Hub named it when it redeemed it); what the Hub serves it is its account's
    /// here too. The bytes held are reused, and the same account's warm runs ask the Hub nothing.
    pub(crate) fn commit(&self, service: &Service, source: &hub::Source, named: Named) -> Result<Named, Failure> {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Held {
            tree: Option<ObjectRef>,
            lane: String,
            header: ObjectRef,
            /// The accounts the Hub served it to here; [`PUBLIC`] once it served it to no sign-in.
            readers: BTreeSet<String>,
        }
        let tree = named.tree()?.clone();
        let key = json!(["commit-v1", hub::origin_key(&source.origin).unwrap_or_default(), named.repository, tree.id()]).to_string();
        let held: Option<Held> = service.engine.with_journal(|j| j.named(&key, "")).map_err(io_failure)?
            .and_then(|held| serde_json::from_str(&held).ok());
        let reader = match &source.capability {
            Some(capability) => capability.account(),
            None => Some(PUBLIC.to_string()),
        };
        let served = |held: &Held| held.readers.contains(PUBLIC) || reader.as_ref().is_some_and(|r| held.readers.contains(r));
        if let Some(held) = held.as_ref().filter(|held| served(held)) {
            return Ok(Named { tree: held.tree.clone(), lane: held.lane.clone(), header: Some(held.header.clone()), ..named });
        }
        tracing::info!(repository = %named.repository, commit = %tree.id(), reader = reader.as_deref().unwrap_or("-"),
            "a commit asked of the Hub as the request's own reader");
        let failed = |e: Refusal| private(source, &named, refused("model_resolution_failed", e.clone()), &e);
        let object = origin::object(&self.machine, source, &tree.sha256).map_err(failed)?;
        let mut held = match held {
            Some(held) => held,
            None => {
                let remote = origin::remote(&self.machine, source).map_err(failed)?;
                self.machine.objects(&remote, std::slice::from_ref(&object), priority::NOW, &Stop::default()).map_err(failed)?;
                match storage::tree_or_header(&self.store, &object).map_err(|e| ("model_resolution_failed", e.to_string()))? {
                    Some(commit) => {
                        let header = commit.header().map_err(|e| ("model_resolution_failed", format!("{}: {e}", object.id())))?.1.clone();
                        Held { tree: Some(object), lane: commit.lane, header, readers: BTreeSet::new() }
                    }
                    None => Held { tree: None, lane: named.lane.clone(), header: object, readers: BTreeSet::new() },
                }
            }
        };
        // A capability whose account cannot be read now (refused since) is served, not recorded.
        if let Some(reader) = reader {
            held.readers.insert(reader);
            let answer = serde_json::to_string(&held).map_err(|e| io_failure(io::Error::other(e)))?;
            service.engine.with_journal(|j| j.bind_named(&key, "", &answer)).map_err(io_failure)?;
        }
        Ok(Named { tree: held.tree, lane: held.lane, header: Some(held.header), ..named })
    }

    /// A Hub checkpoint resolved (`resolve`) and downloaded into the store (a warm run's model
    /// choice: `cozy model download`): whole, or its header and tree alone (`structure`: a
    /// composition's base, whose components stay the Hub's). Answers what the Hub named and its
    /// bytes by where they came from (`pull`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn download(
        &self,
        service: &Service,
        source: &hub::Source,
        named: Named,
        hardware: &Hardware,
        revision: &str,
        structure: bool,
        stop: Option<&Stop>,
        bytes: &(dyn Fn(u64, u64) + Sync),
    ) -> Result<(Named, Landed), Refused> {
        let failed = |(code, message)| Refused { code, message };
        let named = self.resolve(service, source, named, hardware, revision).map_err(failed)?;
        // A checkpoint this store already holds whole moves nothing and asks the Hub nothing.
        let components = structure.then_some(&[][..]);
        let moved = self.pull(source, &named, components, stop, bytes).map_err(failed)?;
        Ok((named, moved))
    }

    /// The installation a request names. An unnamed release is the one the Hub named under
    /// the caller's catalog revision (`newest`), so a warm run asks the Hub nothing.
    fn alias(&self, service: &Service, request: &Request) -> io::Result<String> {
        if let Some(installed) = &request.installed {
            return Ok(installed.alias.clone());
        }
        let release = match (request.release.is_empty(), request.catalog_revision.is_empty()) {
            (true, false) => service.engine.with_journal(|j| j.named(&newest_key(request), &request.catalog_revision))?.unwrap_or_default(),
            _ => request.release.clone(),
        };
        Ok(self.release_alias(request, &release))
    }

    /// The Hub's newest release of the request's package, journaled per catalog revision as a
    /// model's name is (`resolve`): a moved revision asks once more. No revision names no catalog
    /// state, so that ask is the Hub's every time: kept, its first answer would stand forever.
    fn newest(&self, service: &Service, request: &Request, catalog: &Catalog, org: &str, name: &str) -> Result<String, Failure> {
        let (key, revision) = (newest_key(request), &request.catalog_revision);
        if revision.is_empty() {
            return newest_release(catalog, org, name);
        }
        if let Some(held) = service.engine.with_journal(|j| j.named(&key, revision)).map_err(io_failure)? {
            return Ok(held);
        }
        let release = newest_release(catalog, org, name)?;
        service.engine.with_journal(|j| j.bind_named(&key, revision, &release)).map_err(io_failure)?;
        Ok(release)
    }

    /// An earlier environment of this release here, which kept its card beside it (`CARD`): an
    /// exact release never changes, so its environment for another SDK reads nothing from the Hub.
    fn earlier(&self, service: &Service, actor: &str, request: &Request, release: &str) -> Option<PathBuf> {
        let origin = request.origin();
        service.engine.installations(actor).ok()?.into_iter()
            .filter(|i| i.package == request.package && i.release == release && i.hub == origin)
            .map(|i| service.catalog.root().join(&i.generation))
            .find(|dir| dir.join(CARD).is_file())
    }

    /// The Runtime and TensorFS versions an environment built here holds: this machine's own
    /// pair's (by its wheels' names), else the lock's.
    fn planned_pair(&self, locked: &Locked) -> BTreeMap<String, String> {
        let own = self.sdk.requirements.iter().filter_map(|wheel| {
            let mut parts = Path::new(wheel).file_name()?.to_str()?.split('-');
            Some((normalized(parts.next()?), parts.next()?.to_string()))
        });
        let pair: BTreeMap<String, String> = match self.sdk.requirements.is_empty() {
            true => locked.packages.clone(),
            false => own.collect(),
        };
        pair.into_iter().filter(|(name, _)| SDK.contains(&name.as_str())).collect()
    }

    /// The installation of `release` of the request's package at its Hub, by this machine's SDK.
    fn release_alias(&self, request: &Request, release: &str) -> String {
        let origin = request.origin();
        let sdk = format!("{:?}{:?}", self.sdk.requirements, self.sdk.find_links);
        let key = format!("{origin}\0{}\0{release}\0{sdk}", request.package);
        format!("hub-{}", &sha256::hex_digest(key.as_bytes())[..32])
    }

    /// A held resolution's key: the request, under the machine's GPUs as the lane rule reads
    /// them (`gpus`), never a card's name.
    fn resolution_key(installation: &Installation, request: &Request, gpus: &str) -> io::Result<String> {
        let alias = &installation.alias;
        let uses = crate::gpu_service::prepared_uses(installation, &request.entrypoint, request.payload.as_ref())?;
        let choices: Vec<_> = request
            .choices
            .iter()
            .map(|c| {
                json!([
                    c.parameter,
                    c.repository,
                    c.release,
                    c.lane,
                    c.manifest
                        .as_ref()
                        .map(|m| (sha256::hex(&m.digest), m.length)),
                    c.source,
                    c.profiles,
                    // Adapter order and scale are part of what the slot runs.
                    c.adapters
                        .iter()
                        .map(|a| [
                            &a.model,
                            &a.release,
                            &a.lane,
                            &a.manifest,
                            &a.component,
                            &a.source_component,
                            &a.scale,
                            &a.source
                        ])
                        .collect::<Vec<_>>()
                ])
            })
            .collect();
        let origin = request.origin();
        let package = request.installed.as_ref().map(|i| i.package.as_str()).unwrap_or(&request.package);
        let release = request.installed.as_ref().map(|i| i.release.as_str()).unwrap_or(&request.release);
        let mut key = json!({"installation":alias,"package":package,"release":release,"hub":origin,"owner":request.owner,"catalog":request.catalog_revision,"entrypoint":request.entrypoint,"choices":choices,"row":gpus,"gpus":request.gpus,"lane":request.lane});
        if !uses.is_empty() { key["prepared_use"] = serde_json::to_value(uses).map_err(io::Error::other)?; }
        Ok(format!("hub-{}", sha256::hex_digest(key.to_string().as_bytes())))
    }

    /// The held installation and model plan, with no Hub read, or None.
    fn held(
        &self,
        service: &Service,
        actor: &str,
        request: &Request,
    ) -> io::Result<Option<Prepared>> {
        let held = match &request.installed {
            Some(installed) => Some(installed.clone()),
            None => service.engine.installation(actor, &self.alias(service, request)?)?,
        };
        let Some(installation) = held else {
            return Ok(None);
        };
        if service.catalog.resolve(&installation.generation).is_err() {
            return Ok(None);
        }
        if !declares_models(&installation, &request.entrypoint) {
            return Ok(Some(Prepared {
                installation,
                plan: None,
                warnings: vec![],
            }));
        }
        // An alias may now name another tree, or be gone. Resolve its current local metadata
        // before building a plan; the raw request key cannot prove a cached header is current.
        if local_aliases(&installation, request)? {
            return Ok(None);
        }
        let Some(gpu) = service.pool() else {
            return Ok(None);
        };
        let key = Self::resolution_key(&installation, request, &machine_row(&gpu))?;
        let Some(id) = service.engine.with_journal(|j| j.resolution(actor, &key))? else {
            return Ok(None);
        };
        let Some(preparation) = service.engine.preparation(actor, &id)? else {
            return Ok(None);
        };
        let plan = gpu.plan(&preparation)?;
        // Its structure may have been reclaimed since; then the model resolves again. An
        // object reclaimed is fetched again by whatever reads it.
        if !gpu.holds(&plan) {
            return Ok(None);
        }
        let warnings = service.engine.with_journal(|j| j.named(&warnings_key(&key), &request.catalog_revision))?;
        Ok(Some(Prepared {
            installation,
            plan: Some(plan),
            warnings: warnings.and_then(|held| serde_json::from_str(&held).ok()).unwrap_or_default(),
        }))
    }

    fn work(
        &self,
        service: &Arc<Service>,
        actor: &str,
        request: &Request,
        job: &Job,
    ) -> Result<Prepared, Failure> {
        let held = match &request.installed {
            Some(installed) => Some(installed.clone()),
            None => service
                .engine
                .installation(actor, &self.alias(service, request).map_err(io_failure)?)
                .map_err(io_failure)?,
        };
        let installation = match held {
            Some(held) if service.catalog.resolve(&held.generation).is_ok() => held,
            _ if request.installed.is_some() => {
                return Err((
                    "release_root_installation_absent",
                    "this owner has not prepared the named installation".into(),
                ))
            }
            _ => self.install(service, actor, request, job)?,
        };
        if !declares_models(&installation, &request.entrypoint) {
            return Ok(Prepared {
                installation,
                plan: None,
                warnings: vec![],
            });
        }
        // Cached installations may have lost their import-only parent. Restart the same
        // parent before model resolution, as a fresh install already does.
        if let Some(gpu) = service.gpu() {
            let held = service.catalog.resolve(&installation.generation).map_err(io_failure)?;
            gpu.prespawn(held);
        }
        let gpu = service.pool().ok_or((
            "capability_unavailable",
            "this machine runs no functions".to_string(),
        ))?;
        let (plan, warnings) = self.model(service, &gpu, actor, &installation, request, job)?;
        Ok(Prepared {
            installation,
            plan: Some(plan),
            warnings,
        })
    }

    /// The request's release installed for this signer: its card and locked requirements as an
    /// earlier installation of it kept them, else read at its Hub by name (th-245); the Hub's
    /// newest release when it names none (`newest`). An installation of that release this
    /// signer holds already is the answer.
    fn install(
        &self,
        service: &Service,
        actor: &str,
        request: &Request,
        job: &Job,
    ) -> Result<Installation, Failure> {
        let (org, name) = request.package.split_once('/').ok_or((
            "release_root_invalid",
            "package must be org/name".to_string(),
        ))?;
        let source = request.hub("a release")?;
        let catalog = || Catalog::new(source).map_err(|e| ("catalog_read_failed", e.0));
        let release = match request.release.is_empty() {
            true => self.newest(service, request, &catalog()?, org, name)?,
            false => request.release.clone(),
        };
        let alias = self.release_alias(request, &release);
        if let Some(held) = service.engine.installation(actor, &alias).map_err(io_failure)? {
            if service.catalog.resolve(&held.generation).is_ok() {
                return Ok(held);
            }
        }
        // An earlier environment of this release (another SDK's): its card, kept beside it, and
        // the Runtime pair it holds. A kept card whose wheels the store no longer holds is read
        // again: its URLs expire.
        let earlier = self.earlier(service, actor, request, &release);
        let kept = earlier.as_ref().and_then(|dir| serde_json::from_slice::<Card>(&fs::read(dir.join(CARD)).ok()?).ok());
        let card = match kept.filter(|card| card.wheels.iter().all(|wheel| self.held_file(wheel))) {
            Some(card) => card,
            None => hub_card(&catalog()?, &request.package, &release, &self.sdk.python)?,
        };
        let locked = read_lock(&card.pyproject, &card.lock, Some(source))?;
        let before = earlier.and_then(|dir| fs::read(dir.join("generation.json")).ok())
            .and_then(|raw| serde_json::from_slice::<crate::catalog::Generation>(&raw).ok())
            .map(|record| record.dependencies.into_iter().map(|d| (normalized(&d.name), d.version)).collect());
        let stage = install_stage(&request.package, &release, before.as_ref(), &self.planned_pair(&locked));
        let wheels: Vec<&str> = card.wheels.iter().map(|wheel| wheel.sha256.as_str()).collect();
        let identity = sha256::hex_digest(json!({"package":request.package,"release":release,"pyproject":card.pyproject,"lock":card.lock,"wheels":wheels,"python":card.python,"hub":hub::origin_key(&source.origin),"sdk":self.sdk.requirements,"links":self.sdk.find_links}).to_string().as_bytes())[..32].to_string();
        let wanted = Environment {
            card: &card,
            locked: &locked,
            package: &request.package,
            release: &release,
            source,
            stage: &stage,
        };
        self.generation(service.catalog.root(), &identity, &wanted, job)?;
        keep_card(&service.catalog.root().join(&identity), &card);
        let held = service.catalog.resolve(&identity).map_err(io_failure)?;
        service.installed(&held);
        service
            .engine
            .bind_installation(Installation {
                actor: actor.into(),
                alias,
                generation: identity,
                package: request.package.clone(),
                release,
                interface: serde_json::to_vec(&held.record.interface)
                    .map_err(|e| io_failure(io::Error::other(e)))?,
                hub: request.origin(),
            })
            .map_err(io_failure)
    }

    /// Builds one immutable environment at its final path (its interpreter embeds the path);
    /// only the atomic `generation.json` makes it resolvable. Concurrent preparations of the
    /// same identity wait for one another.
    ///
    /// The release's own uv.lock is the install: `uv sync --frozen` takes every row this
    /// platform's resolution needs, by the lock's URLs and hashes, so a machine of any platform
    /// the lock covers installs it. The wheels the release's Hub hosts (its project wheel, its
    /// carried git dependencies, its org's packages) come through TensorFS's downloader instead
    /// and install from disk; the machine's own Runtime pair, where it has one, replaces the
    /// lock's (`environment`).
    fn generation(
        &self,
        generations: &Path,
        identity: &str,
        wanted: &Environment,
        job: &Job,
    ) -> Result<(), Failure> {
        let Environment { card, locked, package, release, source, stage } = *wanted;
        let locks = generations.join(".locks");
        fs::create_dir_all(&locks).map_err(io_failure)?;
        let lock = File::create(locks.join(format!("{identity}.lock"))).map_err(io_failure)?;
        if lock.try_lock().is_err() {
            let holder = self.building.lock().unwrap().get(identity).cloned();
            job.waiting(Some(match holder {
                Some(run) => format!("waiting for run {run} to install {package} {release}"),
                None => format!("waiting for another run's install of {package} {release}"),
            }));
            lock.lock().map_err(io_failure)?;
            job.waiting(None);
        }
        self.building.lock().unwrap().insert(identity.into(), job.caller.clone());
        let _building = Building(&self.building, identity.into());
        let dir = generations.join(identity);
        if dir.join("generation.json").is_file() {
            return Ok(());
        }
        let _cache = self.uv_cache_hold().map_err(io_failure)?;
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(io_failure)?; // an interrupted, never-published build
        }
        let project = dir.join("project");
        fs::create_dir_all(&project).map_err(io_failure)?;
        fs::write(project.join("pyproject.toml"), &card.pyproject).map_err(io_failure)?;
        fs::write(project.join("uv.lock"), &card.lock).map_err(io_failure)?;
        let files: Vec<HubFile> = card.wheels.iter().chain(&locked.hub).cloned().collect();
        let fetch = || self.fetch_hub(&files, source, &dir, job).map(drop);
        let (env, wheels, label) = (dir.join("env"), dir.join("wheels"), format!("{package} {release}"));
        let built = self.build(&env, &environment::Project {
            dir: &project,
            python: &card.python,
            locked: Some(locked),
            wheels: (!files.is_empty()).then_some(wheels.as_path()),
            skip: &locked.git,
            editable: false,
            indexes: &BTreeMap::new(),
            label: &label,
            stage,
            fetch: (!files.is_empty()).then_some(&fetch as &dyn Fn() -> Result<(), Failure>),
        }, job)?;
        environment::hold(&dir)?;
        // The Hub package each hosted wheel installs is its own name in the release's org, the
        // one org a Hub lock names; never read from the URL, whose shape is the Hub's to change
        // (th-245 did, and every callee became `local/<name>`: run 5234).
        let org = package.split_once('/').map_or("", |(org, _)| org);
        let hub_packages: BTreeMap<String, String> = files.iter().map(|file| (file.distribution(), format!("{org}/{}", file.distribution()))).collect();
        let described = environment::describe_environment(&platform::paths::venv_python(&env), &locked.distribution, &hub_packages, true)?;
        let record = built.record((identity, identity), (&locked.distribution, release), &env, &card.interface, &self.sdk.runtime_build, described)?;
        environment::publish(&dir, &record)
    }

    /// The release's Hub-hosted wheels through TensorFS's downloader instead of uv's: 1 MiB
    /// ranged parts side by side, a late one asked again, verified by sha256 into the store.
    /// uv asks one GET per wheel and waits out every stall of it (run 5107, oczy: the install
    /// sat in two ~147 s rounds behind the Hub's 302 to R2, while TensorFS moved 51 GiB from
    /// the same R2 in 91 s). Answers each wheel's path, under its own file name.
    fn fetch_hub(&self, files: &[HubFile], source: &hub::Source, dir: &Path, job: &Job) -> Result<Vec<PathBuf>, Failure> {
        if files.is_empty() {
            return Ok(vec![]);
        }
        let wheels = dir.join("wheels");
        fs::create_dir_all(&wheels).map_err(io_failure)?;
        // Bytes as each file lands, of all of them: a person sees an install that crawls.
        let total: u64 = files.iter().map(|file| file.length).sum();
        let moved = std::sync::atomic::AtomicU64::new(0);
        std::thread::scope(|scope| {
            let handles: Vec<_> = files
                .iter()
                .map(|file| {
                    scope.spawn(|| -> Result<PathBuf, Failure> {
                        let path = self.hub_wheel(file, source, &wheels, job.stop.as_ref())?;
                        let landed = moved.fetch_add(file.length, std::sync::atomic::Ordering::Relaxed);
                        job.bytes(landed + file.length, total);
                        Ok(path)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_else(|_| Err(io_failure(io::Error::other("a package file download panicked")))))
                .collect()
        })
    }

    /// Whether the store holds a Hub file (its URL expires; its bytes do not).
    fn held_file(&self, file: &HubFile) -> bool {
        file.length > 0 && self.store.holds(&ObjectRef { sha256: file.sha256.clone(), length: file.length })
    }

    fn hub_wheel(&self, file: &HubFile, source: &hub::Source, wheels: &Path, stop: Option<&Stop>) -> Result<PathBuf, Failure> {
        let stop = stop.cloned().unwrap_or_default();
        let trust = source.ca_der.as_deref().map(|der| hub::pem(der).into_bytes());
        let length = (file.length > 0).then_some(file.length);
        let blob = crate::machine::pypi::object_at(&self.machine, &file.url, &file.sha256, length, trust, &stop)
            .map_err(|e| match stop.is_cancelled() {
                true => canceled(),
                // Named by its file: a plan's URL is presigned, its query a credential.
                false => ("package_file_download_failed", format!("{}: {}", file.name, e.to_string().replace(&file.url, &file.name))),
            })?;
        let path = wheels.join(&file.name);
        fs::hard_link(&blob, &path)
            .or_else(|_| fs::copy(&blob, &path).map(drop))
            .map_err(io_failure)?;
        Ok(path)
    }

    /// This machine's own uv cache, which `reclaim` manages (`Uv::machine_cache`).
    pub fn uv_cache(&self) -> Option<PathBuf> {
        self.sdk.uv.machine_cache().map(Path::to_path_buf)
    }

    /// Held shared through one environment build, so `reclaim` removes nothing from the cache
    /// between uv's own per-command holds either.
    fn uv_cache_hold(&self) -> io::Result<Option<File>> {
        let Some(cache) = self.uv_cache() else {
            return Ok(None);
        };
        fs::create_dir_all(&cache)?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(cache.join(".lock"))?;
        lock.lock_shared()?;
        Ok(Some(lock))
    }

    /// The run's model grants and its precision warnings. Each slot takes the run's choice, else
    /// its declared default (also under a choice of adapters alone); a Hub one is exact as its
    /// choice names it (th-241), else in the first lane of the rule's order its release
    /// publishes (`precision`). The request's GPU
    /// count chooses the group independently of the weight lane; 0 uses the widest group every
    /// slot supports on the machine. A slot with neither is refused.
    fn model(
        &self,
        service: &Service,
        gpu: &Pool,
        actor: &str,
        installation: &Installation,
        request: &Request,
        job: &Job,
    ) -> Result<(Plan, Vec<String>), Failure> {
        let interface: Value = serde_json::from_slice(&installation.interface).map_err(|_| {
            (
                "package_prepare_interface_invalid",
                "held interface is corrupt".to_string(),
            )
        })?;
        let slots = model_slots(&interface, &request.entrypoint)
            .filter(|slots| !slots.is_empty())
            .ok_or((
                "package_prepare_interface_invalid",
                "declared model slot absent".to_string(),
            ))?;
        // The GPUs the call runs on (none for `accelerator=False`): what its models resolve for.
        let entry = protocol::interface::entrypoint(&interface, &request.entrypoint).cloned().unwrap_or_default();
        let degree = gpu.degree(&entry, request.gpus).map_err(|e| ("model_preparation_failed", e.to_string()))?;
        let hardware = gpu.hardware(degree);
        let main = main_model(&slots);
        // One slot's grant, its Hub checkpoint and its warnings.
        let pick = |slot: &Value| -> Result<(ModelGrant, Option<Named>, Vec<String>), Failure> {
            let path = slot.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
            let parameter = path.rsplit('.').next().unwrap_or_default().to_string();
            job.stage(format!("choosing the model for {path}"));
            let asked = request.choices.iter().find(|c| c.parameter == path || c.parameter == parameter)
                .filter(|c| !c.repository.is_empty() || c.manifest.is_some() || !c.source.is_empty());
            let declared = default_choice(slot, &path);
            let choice = asked.or(declared.as_ref()).ok_or_else(|| choice_absent(&path, &installation.package))?;
            if !choice.source.is_empty() {
                return Ok((self.source_grant(installation, &path, choice, &request.providers, job)?, None, vec![]));
            }
            if let (Some(reference), true) = (
                choice.manifest.as_ref().filter(|m| m.digest.len() == 32),
                choice.repository.is_empty(),
            ) {
                // A model passed by value (a ModelArtifact a run here produced): held here.
                let grant = self.held_grant(installation, &path, reference)?;
                let header = storage::held(&self.store, &sha256::hex(&reference.digest)).map_err(io_failure)?;
                self.held_for(service, request.source.as_ref(), &header)?;
                return Ok((grant, None, vec![]));
            }
            if choice.repository.starts_with("local/") && choice.manifest.is_none() {
                let local = held_local(&self.store, &choice.repository)?;
                let header = local.header.id();
                return Ok((ModelGrant {
                    package: installation.package.clone(), slot: path, repository: choice.repository.clone(),
                    release: String::new(), lane: local.lane,
                    components: self.components(&header)?, manifest: header, adapted: None,
                }, None, vec![]));
            }
            let mut named = choose(choice, &path)?;
            // The run's lane is its main model's; another slot's is its choice's or its own.
            if Some(slot) == main && !request.lane.is_empty() {
                named.lane = request.lane.clone();
            }
            // A class that takes no encoded leaves, or raw weights read as data, takes plain lanes only.
            let plain = slot["encoded_leaves"] != "accept" || slot["weights"] == true;
            let hardware = tensorfs::fetch::Hardware { plain, ..hardware.clone() };
            let named = self.resolve(service, request.hub(&format!("the model for {path}"))?, named, &hardware, &request.catalog_revision)?;
            let warnings = named.reasons.iter().map(|why| format!("{}: {why}", named.repository)).collect();
            let grant = ModelGrant {
                package: installation.package.clone(),
                slot: path,
                repository: named.repository.clone(),
                release: named.release.clone(),
                lane: named.lane.clone(),
                manifest: named.header()?.id(),
                components: vec![],
                adapted: None,
            };
            Ok((grant, (!named.repository.starts_with("local/")).then_some(named), warnings))
        };
        // Each checkpoint lands as its structure: what of it comes, and when, its calls direct.
        // Only a Hub names a checkpoint here (`pick` refused it without the run's access).
        let land = |picked: &[(ModelGrant, Option<Named>, Vec<String>)]| {
            let named = distinct(picked.iter().filter_map(|(_, named, _)| named.as_ref()));
            match named.is_empty() {
                true => Ok(()),
                false => self.structure(request.hub("a Hub model")?, &named, job),
            }
        };
        let picked: Vec<_> = slots.iter().map(pick).collect::<Result<_, _>>()?;
        land(&picked)?;
        let (mut grants, mut warnings) = (vec![], vec![]);
        for (grant, _, said) in picked {
            grants.push(grant);
            warnings.extend(said);
        }
        for grant in grants.iter_mut().filter(|g| g.components.is_empty() && !g.repository.is_empty()) {
            grant.components = self.components(&grant.manifest)?;
        }
        for grant in &mut grants {
            let parameter = grant
                .slot
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_string();
            if let Some(choice) = request
                .choices
                .iter()
                .find(|c| c.parameter == grant.slot || c.parameter == parameter)
            {
                // An adapter resolves for the run's hardware: `--lane` is the main model's.
                let resolve = |source: &hub::Source, named| self.resolve(service, source, named, &hardware, &request.catalog_revision);
                // It adapts a component this call runs (`components=`; none declared: any).
                let runs: Vec<String> = (slots.iter())
                    .find(|slot| slot["path"] == grant.slot.as_str())
                    .and_then(|slot| slot["components"].as_array())
                    .map(|names| names.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                opens_views(service, installation, choice)?;
                warnings.extend(apply_adapters(self, request.source.as_ref(), &resolve, choice, grant, &runs, &request.providers, job)?);
            }
        }
        job.stage(format!("preparing {}", request.package));
        let plan = gpu
            .prepare_root(
                installation,
                &request.entrypoint,
                &request.choices,
                request.payload.as_ref(),
                &grants,
                request.gpus,
            )
            .map_err(|e| ("model_preparation_failed", e.to_string()))?;
        service
            .engine
            .bind_preparation(Preparation {
                actor: actor.into(),
                id: plan.id.clone(),
                installation: installation.alias.clone(),
                document: serde_json::to_vec(&plan).map_err(|e| io_failure(io::Error::other(e)))?,
            })
            .map_err(io_failure)?;
        let key = Self::resolution_key(installation, request, &machine_row(gpu)).map_err(io_failure)?;
        let said = Value::from(warnings.clone()).to_string();
        service
            .engine
            .with_journal(|j| {
                j.bind_resolution(actor, &key, &installation.package, &plan.id)?;
                j.bind_named(&warnings_key(&key), &request.catalog_revision, &said)
            })
            .map_err(io_failure)?;
        Ok((plan, warnings))
    }
}

/// A function's main model, the one a run's `--lane` names (owner, 10-22): its first slot
/// constructed up front, neither raw weights nor on demand. Its LoRAs and auxiliary models take
/// their own choice's lane, else the Hub's pick.
fn main_model(slots: &[Value]) -> Option<&Value> {
    slots.iter().find(|slot| crate::gpu_service::constructed(slot) && slot["on_demand"] != true)
}

/// Each checkpoint once.
fn distinct<'a>(named: impl Iterator<Item = &'a Named>) -> Vec<Named> {
    let mut checkpoints = Vec::<Named>::new();
    for named in named {
        if !checkpoints.iter().any(|c| c.header == named.header) {
            checkpoints.push(named.clone());
        }
    }
    checkpoints
}

/// The machine's GPUs as a held resolution is keyed by: how many, and what they run.
fn machine_row(gpu: &Pool) -> String {
    format!("{}x{:?}", gpu.width(), gpu.gpus(gpu.width() as u32))
}

/// Where a held resolution's precision warnings are journaled.
fn warnings_key(resolution: &str) -> String {
    json!(["warnings", resolution]).to_string()
}

/// A Hub model slot's commit as its choice names it (th-245): by hash, else `repository` at
/// `release` in the choice's lane; no lane is the Hub's pick (`resolve`).
pub(crate) fn choose(choice: &domain::ModelChoice, path: &str) -> Result<Named, Failure> {
    if choice.repository.is_empty() {
        return Err(("model_choice_absent", format!("{path}'s choice names no model repository")));
    }
    let tree = choice.manifest.as_ref().map(|m| ObjectRef { sha256: sha256::hex(&m.digest), length: m.length });
    Ok(Named { repository: choice.repository.clone(), release: choice.release.clone(), lane: choice.lane.clone(), tree, ..Named::default() })
}

/// A provider source made into a local model.
#[derive(Clone, Debug)]
pub struct Made {
    /// `local/source-<hash>`, named by the source's pinned spelling and converter.
    pub repository: String,
    pub header: ObjectRef,
    /// The pinned source the provider resolved.
    pub resolved: String,
    pub profiles: Vec<String>,
    /// How TensorFS read the source's layout, and what it verified it as: `detected h3.lora
    /// (musubi-tuner); LoRA of minimax-h3 transformer, trained on transformer_ref (…)`.
    pub format: String,
    /// The words its provider says a LoRA's prompt uses (Civitai's trainedWords).
    pub trigger: Vec<String>,
}

/// `source` imported into a local model (normalized at ingest, design §6), or the one already
/// made from its pinned spelling by the same converter tables: a moving revision is read again,
/// a pinned one is reused.
fn make_source(
    store: &Store,
    source: &str,
    profiles: &[String],
    trained_on: &str,
    providers: &Providers,
    cancellation: Option<Stop>,
    progress: &(dyn Fn(&str, u64, u64) + Sync),
) -> Result<Made, Failure> {
    let bearer = |token: &str| if token.is_empty() { String::new() } else { format!("bearer {token}") };
    let access = tensorfs::import::Access {
        huggingface_token: bearer(&providers.huggingface),
        civitai_token: bearer(&providers.civitai),
        ..Default::default()
    };
    let [converter] = match profiles {
        [] => [None],
        [one] => [Some(one.as_str())],
        _ => return Err(("model_source_failed", "a source is converted by one table".into())),
    };
    let listed = tensorfs::import::remote::parse(source)
        .and_then(|spec| tensorfs::import::remote::list(&spec, &access))
        .map_err(|e| refused("model_source_failed", e))?;
    let tables = tensorfs::import::engine::tables_digest();
    let families = tensorfs::family::families_digest();
    let identity = serde_json_canonicalizer::to_vec(&json!(["cozy.machine-source-model/3", listed.pinned, converter, tables, families, trained_on]))
        .map_err(|e| io_failure(io::Error::other(e)))?;
    let name = format!("source-{}", &sha256::hex_digest(&identity)[..40]);
    let made = |header: ObjectRef| {
        let format = store.source_format(&header).ok().flatten().map(|f| f.to_string()).unwrap_or_default();
        let family = (store.read(&header).ok())
            .and_then(|bytes| tensorfs::format::Header::from_cbor(&bytes).ok())
            .and_then(|h| tensorfs::family::summary(&h.metadata));
        Made {
            repository: format!("local/{name}"),
            format: family.map_or(format.clone(), |f| format!("{format}; {f}")),
            header,
            resolved: listed.pinned.clone(),
            profiles: profiles.to_vec(),
            trigger: listed.trigger.clone(),
        }
    };
    let held = crate::storage::local(store, &format!("local/{name}")).map_err(io_failure)?;
    if let Some(local) = held.filter(|local| store.holds(&local.header)) {
        crate::storage::transport(store, "import", source, &format!("header={} reused", local.header.id()));
        return Ok(made(local.header));
    }
    let stop = || cancellation.as_ref().is_some_and(Stop::is_cancelled);
    // What the import downloaded, for its transport line.
    let downloaded = std::sync::atomic::AtomicU64::new(0);
    let counted = |stage: &str, done: u64, total: u64| {
        if stage == "downloading" {
            downloaded.fetch_max(total, std::sync::atomic::Ordering::Relaxed);
        }
        progress(stage, done, total)
    };
    let request = tensorfs::import::Request {
        sources: &[source.to_string()],
        converter,
        source_variant: None,
        trained_on: (!trained_on.is_empty()).then_some(trained_on),
        access: &access,
        tmp: &store.root().join("tmp"),
        progress: &counted,
        stop: Some(&stop),
    };
    let started = Instant::now();
    let (header, files) = store.import(&request, Some(&name)).map_err(|e| {
        crate::storage::transport(store, "import-failed", source, &e.to_string());
        match stop() {
            true => canceled(),
            false => refused("model_source_failed", e),
        }
    })?;
    let closure = store.closure(&header).unwrap_or_default();
    let detail = format!(
        "header={} objects={} bytes={} downloaded={} seconds={:.1}",
        header.id(),
        closure.len(),
        closure.iter().map(|o| o.length).sum::<u64>(),
        downloaded.load(std::sync::atomic::Ordering::Relaxed),
        started.elapsed().as_secs_f64()
    );
    crate::storage::transport(store, "import", source, &detail);
    crate::storage::set_local(store, &name, &header, &files, "").map_err(io_failure)?;
    Ok(made(header))
}

/// The reader a request with no sign-in reads as: public content only.
const PUBLIC: &str = "*";

/// `failure`, reading `source` under no signer's capability: a private model reads as absent
/// there, so the reason names the sign-in that reads it (a pod's shell, a laptop not logged in).
fn unsigned(source: &hub::Source, failure: Failure, refusal: &tensorfs::err::Refusal) -> Failure {
    use tensorfs::err::Code;
    let absent = matches!(refusal.code, Code::REF_NOT_FOUND | Code::REPOSITORY_ABSENT | Code::CREDENTIAL_REQUIRED);
    match source.capability.is_none() && absent {
        true => (failure.0, format!("{}; this run carried no sign-in, and a private model reads only under one: `cozy login <email>` on the computer that sent it", failure.1)),
        false => failure,
    }
}

/// `failure` of `named`, a commit by hash, read with no sign-in: it is private (or absent).
fn private(source: &hub::Source, named: &Named, failure: Failure, refusal: &tensorfs::err::Refusal) -> Failure {
    match source.capability.is_none() {
        true => (failure.0, format!("{} is private, or no such commit: this run carried no sign-in; `cozy login <email>` on the computer that sent it reads it as its account ({})", named.shown(), refusal)),
        false => unsigned(source, failure, refusal),
    }
}

/// A TensorFS refusal as a run's reason; a download the disk cannot fit is its own.
fn refused(code: &'static str, refusal: tensorfs::err::Refusal) -> Failure {
    match refusal.code {
        tensorfs::err::Code::CAPACITY_EXHAUSTED => ("machine_disk_full", refusal.to_string()),
        // The run's capability was refused (expired); without one a private object is absent.
        tensorfs::err::Code::CREDENTIAL_REQUIRED => ("hub_credential_refused", refusal.detail),
        _ => (code, refusal.to_string()),
    }
}

/// The model `repository` (`local/<name>`) as this machine holds it. Absent, the reason says that
/// a `local/` name lives on the machine that runs it, and how to put it there.
fn held_local(store: &Store, repository: &str) -> Result<storage::Local, Failure> {
    let held = storage::local(store, repository).map_err(io_failure)?.filter(|local| store.holds(&local.header));
    held.ok_or_else(|| ("checkpoint_absent", format!(
        "this machine holds no {repository}: a local/ name is a model downloaded to the machine that runs it; \
         download it there first (`cozy model download <source> {repository} --on <machine>`)")))
}

/// The oldest Runtime and TensorFS whose executors open an adapter view from Load's bytes
/// (`tensorfs.view`, cl-683).
const VIEWS: [(&str, &str, &str); 2] = [("cozy-runtime", "Runtime", "0.35.5"), ("tensorfs", "TensorFS", "0.17.13")];

/// A LoRA'd call runs only where the installation's environment opens adapter views; without
/// LoRAs, any pair runs as before.
fn opens_views(service: &Service, installation: &Installation, choice: &domain::ModelChoice) -> Result<(), Failure> {
    if choice.adapters.is_empty() {
        return Ok(());
    }
    let held = service.catalog.resolve(&installation.generation).map_err(io_failure)?;
    let release = |version: &str| -> Vec<u64> {
        version.split(['+', '-']).next().unwrap_or_default().split('.').map_while(|part| part.parse().ok()).collect()
    };
    let has = |name: &str| (held.record.dependencies.iter()).find(|d| normalized(&d.name) == name).map(|d| d.version.clone()).unwrap_or_default();
    let pair: Vec<(&str, &str, String)> = VIEWS.iter().map(|(name, label, floor)| (*label, *floor, has(name))).collect();
    if pair.iter().all(|(_, floor, version)| release(version) >= release(floor)) {
        return Ok(());
    }
    let runs: Vec<String> = pair.iter().map(|(label, _, version)| format!("{label} {}", if version.is_empty() { "none" } else { version })).collect();
    Err(("model_adapter_refused", format!(
        "LoRAs need Runtime {} and TensorFS {} or newer, and {} runs on {}: update this machine (`cozy machine update`)",
        VIEWS[0].2, VIEWS[1].2, installation.package, runs.join(" / "))))
}

/// A slot's caller adapters applied to its resolved base: each adapter downloaded, then one
/// adapter view composed (`adapter::compose`) over the components the call `runs`; the grant
/// then names the view.
#[allow(clippy::too_many_arguments)]
fn apply_adapters(
    publisher: &Publisher,
    source: Option<&hub::Source>,
    resolve: &dyn Fn(&hub::Source, Named) -> Result<Named, Failure>,
    choice: &domain::ModelChoice,
    grant: &mut ModelGrant,
    runs: &[String],
    providers: &Providers,
    job: &Job,
) -> Result<Vec<String>, Failure> {
    if choice.adapters.is_empty() {
        return Ok(vec![]);
    }
    let store = &publisher.store;
    job.stage(format!("preparing model adapters for {}", grant.slot));
    let object = |header: &str| {
        storage::held(store, header).map_err(|e| ("model_download_failed", format!("checkpoint {header} is not held: {e}")))
    };
    let mut selections = vec![];
    for adapter in &choice.adapters {
        let header = if adapter.source.is_empty() && adapter.model.starts_with("local/") {
            // One this machine holds by name (`cozy model download … local/<name>`).
            held_local(store, &adapter.model)?.header.id()
        } else if adapter.source.is_empty() {
            // By name, resolved at the Hub, or the owner's by hash (th-245).
            if adapter.model.is_empty() {
                return Err(("adapter_absent", format!("an adapter of {} names no model", grant.slot)));
            }
            let tree = (!adapter.manifest.is_empty())
                .then(|| ObjectRef { sha256: adapter.manifest.trim_start_matches("sha256:").into(), length: 0 });
            let source = source.ok_or_else(|| ("hub_access_absent",
                format!("an adapter of {} names a Hub model, and this run carries no Hub access", grant.slot)))?;
            let named = resolve(source, Named {
                repository: adapter.model.clone(),
                release: adapter.release.clone(),
                lane: adapter.lane.clone(),
                tree,
                ..Named::default()
            })?;
            // An adapter is small and its view composes from all of it: it comes whole.
            publisher.ensure(source, &named, job.stop.as_ref(), &|d, t| job.bytes(d, t))?;
            named.header()?.id()
        } else {
            // A provider-source LoRA (civitai://, hf://) is made here, normalized at ingest.
            let made = publisher.make(&adapter.source, &adapter.profiles, "", providers, job.stop.as_ref(), &|stage, moved, total| {
                job.set(format!("{stage} for an adapter of {}", grant.slot), moved, total)
            })?;
            job.stage(format!("{} for an adapter of {}", made.format, grant.slot));
            made.header.id()
        };
        let strength = if adapter.scale.is_empty() {
            1.0
        } else {
            adapter.scale.parse::<f64>().map_err(|_| {
                (
                    "model_override_invalid",
                    format!("adapter scale {:?} is not a decimal", adapter.scale),
                )
            })?
        };
        selections.push(crate::adapter::Selection {
            checkpoint: object(&header)?,
            component: adapter.component.clone(),
            source_component: adapter.source_component.clone(),
            strength,
            runs: runs.to_vec(),
        });
    }
    // The view reads the base's own chunks, from this store: they come first.
    let base = object(&grant.manifest)?;
    if let (Some(pulled), Some(source)) = (publisher.pulled(&base).map_err(io_failure)?, source) {
        publisher.ensure(source, &pulled, job.stop.as_ref(), &|d, t| job.bytes(d, t))?;
    }
    // The view is composed by reference and kept in memory, never written; Load sends its bytes.
    let composed = crate::adapter::compose(store, &base, &selections)
        .map_err(|e| ("model_adapter_refused", e.to_string()))?;
    // The run's record names the base model and what each LoRA adapts (`Plan::models`).
    let adapters = (choice.adapters.iter().zip(&selections).zip(&composed.adapted))
        .map(|((adapter, selection), components)| crate::gpu_service::Applied {
            model: if adapter.source.is_empty() { adapter.model.clone() } else { adapter.source.clone() },
            strength: selection.strength,
            components: components.clone(),
        })
        .collect();
    let warnings = (choice.adapters.iter().zip(&composed.warnings))
        .flat_map(|(adapter, said)| {
            let named = if adapter.source.is_empty() { &adapter.model } else { &adapter.source };
            said.iter().map(move |w| format!("{named}: {w}"))
        })
        .collect();
    let base = std::mem::replace(&mut grant.repository, composed.name);
    grant.adapted = Some(crate::gpu_service::Adapted { repository: base, adapters });
    grant.manifest = composed.checkpoint.id().id();
    Ok(warnings)
}

/// A catalog read the Hub answered 404 to is the thing named absent there; any other failure
/// is the read's, naming what it read and the Hub's whole answer (run 14446 said only "internal:
/// unexpected internal failure").
/// The Hub's own 404 names an absence; one without its envelope (`http_404`: a tunnel or proxy
/// in front of it) names nothing (tfs-101).
fn absent(refusal: hub_client::Error, code: &'static str, read: &str, named: impl FnOnce() -> String) -> Failure {
    let failed = |why: String| ("catalog_read_failed", format!("reading {read}: {why}"));
    match refusal {
        hub_client::Error::Hub { status: 404, code: hub, .. } if !hub.starts_with("http_") => (code, named()),
        hub_client::Error::Hub { status, code, message, remedy } => failed(match remedy.is_empty() {
            true => format!("the Hub answered {status} {code}: {message}"),
            false => format!("the Hub answered {status} {code}: {message} ({remedy})"),
        }),
        other => failed(other.to_string()),
    }
}

/// What a person sees while a release's environment is built here: an install, named the first
/// run of the release on this machine when no environment of it is here yet. The Runtime and
/// TensorFS are named only when one moves from the version an earlier environment of the
/// release holds (an update of this machine's pair): linking the pair in is part of the install.
fn install_stage(package: &str, release: &str, before: Option<&BTreeMap<String, String>>, after: &BTreeMap<String, String>) -> String {
    let Some(before) = before else {
        return format!("installing {package} {release} on this machine (first run of this release)");
    };
    let moved: Vec<String> = after.iter().filter_map(|(name, version)| {
        let was = before.get(name).filter(|was| *was != version)?;
        let label = if name == "cozy-runtime" { "Runtime" } else { "TensorFS" };
        Some(format!("{label} {was} → {version}"))
    }).collect();
    match moved.is_empty() {
        true => format!("installing {package} {release} on this machine"),
        false => format!("updating {package} {release} on this machine: {}", moved.join(", ")),
    }
}

/// The journal name of an unnamed release of the request's package at its Hub.
fn newest_key(request: &Request) -> String {
    json!(["newest", request.origin(), request.package]).to_string()
}


/// What installing a release reads of it, from its install plan: its interface and Python, its
/// exact pyproject and uv.lock, and the wheels its Hub hosts for it (its project wheel and carried
/// dependencies). Kept beside each environment built from it (`CARD`).
#[derive(serde::Deserialize, serde::Serialize)]
struct Card {
    interface: Value,
    python: String,
    pyproject: String,
    lock: String,
    wheels: Vec<HubFile>,
}
const CARD: &str = "release.json";

/// `release`'s install plan, read at its Hub.
fn hub_card(catalog: &Catalog, package: &str, release: &str, python: &str) -> Result<Card, Failure> {
    let (org, name) = package.split_once('/').unwrap_or_default();
    let read = format!("the install plan of {package}@{release} at {}", catalog.origin());
    let plan = catalog.install_plan(org, name, release, None).map_err(|e| absent(e, "release_absent", &read, || {
        format!("{package} has no release {release} at {}", catalog.origin())
    }))?;
    if plan.release != release {
        return Err(("catalog_read_failed", format!("{package}@{release} named another release")));
    }
    let text = |what: &str, document: &hub_client::types::ExactDocument| {
        base64::engine::general_purpose::STANDARD.decode(&document.canonical_bytes).ok().and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(("package_prepare_plan_invalid", format!("the release's {what} is not base64 UTF-8")))
    };
    let interface = serde_json::from_str::<Value>(&text("interface", &plan.package_interface)?).ok().filter(Value::is_object).ok_or((
        "package_prepare_interface_missing",
        "the release carries no package interface".to_string(),
    ))?;
    let wheels = plan.downloads.iter().map(|file| {
        HubFile::new(&file.url, &file.digest, &file.path, file.length.try_into().unwrap_or(0))
            .ok_or(("package_prepare_plan_invalid", format!("the release's plan names an unusable file {}", file.path)))
    });
    Ok(Card {
        interface,
        python: plan.python_version.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| python.to_string()),
        pyproject: text("pyproject.toml", &plan.pyproject)?,
        lock: text("uv.lock", &plan.uv_lock)?,
        wheels: wheels.collect::<Result<_, _>>()?,
    })
}

/// `card` kept beside the environment at `generation` for the release's later environments.
/// A cache, written by rename and never synced: one lost is read from the Hub again.
fn keep_card(generation: &Path, card: &Card) {
    let staged = generation.join(format!(".{CARD}.{}", uuid::Uuid::new_v4().simple()));
    let kept = serde_json::to_vec(card).map_err(io::Error::other)
        .and_then(|bytes| fs::write(&staged, bytes))
        .and_then(|()| fs::rename(&staged, generation.join(CARD)));
    if let Err(error) = kept {
        tracing::warn!("release card {}: {error}", generation.display());
    }
}

/// The newest release of `org/name` at the Hub (a run naming none), by the one version ordering
/// (`newest_by`): the newest final release, else the newest pre-release; yanked ones never.
fn newest_release(catalog: &Catalog, org: &str, name: &str) -> Result<String, Failure> {
    let card = catalog
        .package(org, name)
        .map_err(|e| absent(e, "package_absent", &format!("the releases of {org}/{name} at {}", catalog.origin()),
            || format!("{org}/{name} is not published at {}", catalog.origin())))?;
    let open = card.releases.iter().filter(|row| row.yanked != Some(true) && row.yanked_at.as_deref().is_none_or(str::is_empty));
    tensorfs::format::newest_by(open, |row| row.release.as_str())
        .map(|row| row.release.clone())
        .ok_or(("release_absent", format!("{org}/{name} has no release that is not yanked")))
}

/// A wheel the run's Hub hosts, downloaded by its digest (`fetch_hub`) and installed from disk.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct HubFile {
    url: String,
    sha256: String,
    name: String,
    /// 0 when its source does not say: then asked of the file's host.
    length: u64,
}

impl HubFile {
    fn new(url: &str, sha256: &str, name: &str, length: u64) -> Option<Self> {
        let sha256 = sha256.strip_prefix("sha256:").unwrap_or(sha256);
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+');
        (name.ends_with(".whl")
            && !name.starts_with('.')
            && name.chars().all(plain)
            && sha256.len() == 64
            && sha256.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| Self { url: url.into(), sha256: sha256.to_ascii_lowercase(), name: name.into(), length })
    }

    /// The distribution the wheel installs, from its file name (`<name>-<version>-...whl`).
    fn distribution(&self) -> String {
        normalized(self.name.split('-').next().unwrap_or_default())
    }
}

/// One file named by its digest, into this machine's store: a download that stops moving
/// fails on its measured stillness, `ledger`'s stop ends it, and what landed stays for the
/// next ask. Answers the blob's path.
pub(crate) fn io_failure(error: io::Error) -> Failure {
    ("release_root_preparation_failed", error.to_string())
}

/// A slot's declared default as a choice (its `default`: `[org/]model@release`, or a local
/// model `local/<name>`, its newest): what a run that names no model for the slot gets, in its
/// declared `lane`, else the Hub's pick. A model with no org is first-party, the root's (xs-071).
pub(crate) fn default_choice(slot: &Value, path: &str) -> Option<domain::ModelChoice> {
    let default = slot.get("default")?.as_str()?;
    let (model, release) = match default.split_once('@') {
        Some(named) => named,
        None => (default.starts_with("local/").then_some(default)?, ""),
    };
    let repository = match model.contains('/') {
        true => model.into(),
        false => format!("{}/{model}", protocol::names::ROOT),
    };
    let lane = slot.get("lane").and_then(Value::as_str).unwrap_or_default().into();
    Some(domain::ModelChoice { parameter: path.into(), repository, release: release.into(), lane, ..Default::default() })
}

fn local_aliases(installation: &Installation, request: &Request) -> io::Result<bool> {
    let interface: Value = serde_json::from_slice(&installation.interface).map_err(io::Error::other)?;
    Ok(model_slots(&interface, &request.entrypoint).unwrap_or_default().iter().any(|slot| {
        let path = slot["path"].as_str().unwrap_or_default();
        let parameter = path.rsplit('.').next().unwrap_or_default();
        let asked = request.choices.iter().find(|c| c.parameter == path || c.parameter == parameter)
            .filter(|c| !c.repository.is_empty() || c.manifest.is_some() || !c.source.is_empty());
        let declared = default_choice(slot, path);
        asked.or(declared.as_ref()).is_some_and(|choice| choice.source.is_empty()
            && choice.manifest.is_none() && choice.repository.starts_with("local/"))
    }))
}

/// A slot with neither a choice nor a declared default.
pub(crate) fn choice_absent(path: &str, package: &str) -> Failure {
    ("model_choice_absent", format!("the run names no model for {path}, and {package} declares no default for it"))
}

fn model_slots(interface: &Value, entrypoint: &str) -> Option<Vec<Value>> {
    let entry = protocol::interface::entrypoint(interface, entrypoint)?;
    entry["models"].is_array().then(|| protocol::interface::models(entry).to_vec())
}

fn declares_models(installation: &Installation, entrypoint: &str) -> bool {
    serde_json::from_slice(&installation.interface)
        .ok()
        .and_then(|i: Value| model_slots(&i, entrypoint))
        .is_some_and(|m| !m.is_empty())
}

/// What a uv.lock leaves to this machine: its git-sourced packages (a release's are installed from
/// its carried wheels), its local directories (built again with their changed code), the rows the
/// run's Hub hosts (each one wheel, moved by TensorFS), and every package it names.
#[derive(Debug, Default)]
pub(crate) struct Locked {
    distribution: String,
    git: Vec<String>,
    local: Vec<String>,
    hub: Vec<HubFile>,
    /// Each package it names, at its version.
    packages: BTreeMap<String, String>,
}

/// The Runtime SDK pair: the machine's own, where it has one.
const SDK: [&str; 2] = ["cozy-runtime", "tensorfs"];

/// A project's pyproject and uv.lock read for what its environment's build hands uv and what it
/// moves itself: the rows a release's Hub (`source`) hosts (each one wheel; a row with one per
/// platform is uv's to choose).
pub(crate) fn read_lock(pyproject: &str, lock: &str, source: Option<&hub::Source>) -> Result<Locked, Failure> {
    #[derive(serde::Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<Row>,
        #[serde(default, rename = "supported-markers")]
        supported: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        #[serde(default)]
        version: String,
        #[serde(default)]
        source: BTreeMap<String, toml::Value>,
        #[serde(default)]
        wheels: Vec<Wheel>,
    }
    #[derive(serde::Deserialize)]
    struct Wheel {
        url: Option<String>,
        hash: Option<String>,
        #[serde(default)]
        size: u64,
    }
    let project = distribution(pyproject)?;
    let lock = toml::from_str::<Lock>(lock).map_err(|e| invalid("uv.lock", e))?;
    // Refused before anything is fetched: a Mac or Windows computer told at once where it runs.
    if let Some(there) = elsewhere(&lock.supported, uv_platform()) {
        let here = format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH);
        return Err(("release_platform_unsupported",
            format!("this package runs on {there}, not this machine ({here}): `--on new` rents a machine for it")));
    }
    let rows = lock.package;
    let mut locked = Locked { distribution: project, ..Default::default() };
    for row in rows {
        let name = normalized(&row.name);
        let local = ["directory", "editable", "path"].iter().any(|key| row.source.contains_key(*key));
        if row.source.contains_key("git") {
            locked.git.push(name.clone());
        } else if local && row.source.get("editable").and_then(|e| e.as_str()) != Some(".") {
            locked.local.push(name.clone());
        } else if let ([Wheel { url: Some(url), hash: Some(hash), size }], Some(source)) = (row.wheels.as_slice(), source) {
            let file = url.rsplit('/').next().unwrap_or_default();
            if let Some(file) = HubFile::new(url, hash, file, *size).filter(|_| hub::publishes(source, url)) {
                locked.hub.push(file);
            }
        }
        locked.packages.insert(name, row.version);
    }
    Ok(locked)
}

/// The distribution a pyproject.toml names (`[project] name`).
pub(crate) fn distribution(pyproject: &str) -> Result<String, Failure> {
    #[derive(serde::Deserialize)]
    struct Pyproject {
        project: Project,
    }
    #[derive(serde::Deserialize)]
    struct Project {
        name: String,
    }
    Ok(toml::from_str::<Pyproject>(pyproject).map_err(|e| invalid("pyproject.toml", e))?.project.name)
}

fn invalid(what: &str, e: toml::de::Error) -> Failure {
    ("package_prepare_lock_invalid", format!("the package's {what} does not parse: {e}"))
}

/// This machine's `sys_platform`, as a lock's markers spell it.
fn uv_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        os => os,
    }
}

/// Where a lock's `supported-markers` say it runs, when none of them is `here`'s OS: `Linux
/// x86_64, Linux aarch64`. A lock without them, or one naming no OS, runs anywhere uv resolves it.
fn elsewhere(markers: &[String], here: &str) -> Option<String> {
    let value = |marker: &str, key: &str| Some(marker.split(&format!("{key} == '")).nth(1)?.split('\'').next()?.to_owned());
    let platforms: Vec<Option<String>> = markers.iter().map(|m| value(m, "sys_platform")).collect();
    if platforms.is_empty() || platforms.iter().any(|os| os.as_deref().is_none_or(|os| os == here)) {
        return None;
    }
    let named = |marker: &String| {
        let os = match value(marker, "sys_platform").as_deref() {
            Some("linux") => "Linux".to_owned(),
            Some("darwin") => "macOS".to_owned(),
            Some("win32") => "Windows".to_owned(),
            os => os.unwrap_or_default().to_owned(),
        };
        value(marker, "platform_machine").map_or(os.clone(), |arch| format!("{os} {arch}"))
    };
    Some(markers.iter().map(named).collect::<Vec<_>>().join(", "))
}
