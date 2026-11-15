//! What a project's walk asks of the Calcifer it runs in, and nothing else: the Hub's packages,
//! sending and following runs, the machines' load, and ffmpeg. A walk hosted elsewhere (the Hub)
//! answers these same calls.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use api::{
    calcifer::{MachineState, On, OutputItem, Run, Status as RunStatus, SubmitRun},
    config::Origin,
    error::{Error, Kind},
    ids::{Digest, RunId},
    interface::{self, Call, Interface, Slot},
    names::shown,
    calcifer::tool,
};
use serde_json::Value;

use sha2::{Digest as _, Sha256};

use crate::{
    api::{Api, Refusal},
    local_code,
    runs::target::{self, Resolved},
    store::State,
};

/// A package as the Hub has it (the release its machine holds, else its newest, or the one `@release` names), or local code on this computer (`with: ./dir`,
/// from the project's folder).
#[derive(Clone)]
pub(crate) struct Package {
    pub name: String,
    /// For local code, its source's digest.
    pub release: String,
    /// Local code: its package directory or script, made ready on the machine each call goes to.
    pub local: Option<PathBuf>,
    /// Local code's manifest, `sha256:<hex>`, as it was made ready.
    pub manifest: String,
    /// What this walk moved to and from, said once: off a yanked release (a warning), onto a newer one than its machine's.
    pub moved: Option<api::calcifer::Note>,
    /// The function its tool names; none: the package alone.
    pub named: Option<String>,
    /// A published release's digest, as its card says (xs-196); empty: local code, or a Hub before it.
    pub digest: String,
    document: Value,
}

impl Package {
    /// Its interface, as its card or its source says.
    pub(crate) fn document(&self) -> &Value {
        &self.document
    }

    /// `function` of it as a machine's estimate names it: `org/pkg/fn@release`, local code by its manifest.
    pub(crate) fn estimated(&self, function: &str) -> String {
        match &self.local {
            None => format!("{}/{function}@{}", self.name, self.release),
            Some(_) => format!("{}/{function}", self.manifest),
        }
    }

    /// The function its tool names, as a run of it is sent. None: it names none.
    pub(crate) fn maker(&self) -> Result<Option<Renderer>, Error> {
        self.named.as_deref().map(|function| self.function(function)).transpose()
    }

    /// The function a job of `kind` will most likely call: the one named, else the first public one that makes it.
    pub(crate) fn first_making(&self, kind: &str) -> Option<Renderer> {
        if let Some(function) = &self.named {
            return self.function(function).ok();
        }
        let interface = self.interface();
        let makes = |name: &&str| interface.callables.iter().find(|callable| callable.name == **name).and_then(tool::output_kind) == Some(kind);
        let first = interface.public().into_iter().find(makes)?;
        self.function(first).ok()
    }

    /// What its named function makes: `image`, `video` or `audio`. None: it names none, or its output says no media.
    pub(crate) fn named_kind(&self) -> Option<String> {
        let interface = self.interface();
        let callable = interface.callables.iter().find(|callable| Some(&callable.name) == self.named.as_ref())?;
        tool::output_kind(callable).map(str::to_owned)
    }

    /// The typed inputs of `function`.
    pub(crate) fn callable(&self, function: &str) -> Option<api::interface::Callable> {
        self.interface().callables.into_iter().find(|callable| callable.name == function)
    }

    fn interface(&self) -> Interface {
        Interface::parse(&self.document).unwrap_or_default()
    }

    /// The tags of the function its tool names (`@app.tool(reserved=…)`): in a text they are text.
    pub(crate) fn reserved(&self) -> Vec<String> {
        let mut tags = self.named.as_deref().and_then(|function| self.callable(function)?.tool).map(|tool| tool.reserved).unwrap_or_default();
        tags.sort_unstable();
        tags
    }

    /// `function` of it.
    pub(crate) fn function(&self, function: &str) -> Result<Renderer, Error> {
        let renderer = Renderer::read(&self.name, function, self.release.clone(), &self.document, self.local.clone())?;
        Ok(Renderer { digest: self.digest.clone(), ..renderer })
    }
}

/// One function of a package, as a run of it is sent.
#[derive(Clone)]
pub(crate) struct Renderer {
    pub package: String,
    pub function: String,
    /// For local code, its source's digest.
    pub release: String,
    /// Local code: its package directory or script, made ready on the machine each call goes to.
    pub local: Option<PathBuf>,
    /// What its request takes, read from its schema.
    pub accepts: BTreeSet<String>,
    /// Its model slots, for a request's `models:` and `lora:`.
    pub slots: Vec<Slot>,
    /// The functions it calls, each its own unit of placement, with their model slots.
    pub calls: Vec<Call>,
    /// Its release's digest, as its card says; empty: none.
    pub digest: String,
}

impl Renderer {
    /// `function` of the package interface `document`.
    fn read(
        package: &str,
        function: &str,
        release: String,
        document: &Value,
        local: Option<PathBuf>,
    ) -> Result<Self, Error> {
        let unknown = || format!("{package}@{release} has no function {function}");
        let entry = (interface::entrypoints(document).find(|entry| interface::name(entry) == Some(function)))
            .ok_or_else(|| Error::new(Kind::NotFound, "callable_unknown", unknown()).next(format!("cozy package install {}", shown(package))))?;
        let read = |slots: Vec<&Value>| slots.into_iter().filter_map(|slot| Slot::read(slot).ok()).collect();
        let fields = entry["request"]["fields"].as_array();
        // A list with no room for anything is not taken.
        let accepts = (fields.into_iter().flatten())
            .filter(|field| field["constraints"]["max_length"].as_u64() != Some(0))
            .filter_map(|field| field["name"].as_str().map(str::to_owned))
            .collect();
        Ok(Self {
            package: package.into(),
            function: function.into(),
            release,
            local,
            accepts,
            slots: read(interface::models(entry).iter().collect()),
            calls: Interface::parse(document).ok().and_then(|parsed| parsed.callables.into_iter().find(|callable| callable.name == function)).map(|callable| callable.calls).unwrap_or_default(),
            digest: String::new(),
        })
    }

    /// The model slots of the calls it makes.
    pub(crate) fn call_slots(&self) -> Vec<&Slot> {
        self.calls.iter().flat_map(|call| &call.models).collect()
    }

    /// Its run's name for people: `org/package/function@release`, or `local/<name>/function`.
    pub(crate) fn target(&self) -> String {
        match self.local {
            None => format!("{}/{}@{}", self.package, self.function, self.release),
            Some(_) => format!("{}/{}", self.package, self.function),
        }
    }
}

/// What a run took, in seconds, by what its executors recorded: none when they recorded no stage. Its run's own time, as the stages of
/// an orchestrator and of its calls are the same time, counted twice.
fn took_s(run: &Run) -> Option<f64> {
    let calls = run.calls.iter().filter_map(|call| call.measurements.as_ref());
    let stages = run.measurements.iter().chain(calls).flat_map(|measured| measured.pointer("/attribution/stages").and_then(Value::as_object).into_iter().flatten());
    let busy: f64 = stages.filter(|(name, _)| !name.starts_with("host · ")).map(|(_, stage)| stage["total_ms"].as_f64().unwrap_or(0.0)).sum();
    let wall = run.execution_ms.map(|ms| ms as f64).filter(|ms| *ms > 0.0).unwrap_or(busy);
    (busy > 0.0).then_some(wall / 1000.0)
}

pub(crate) struct Host {
    api: Arc<Api>,
    /// Read once a walk, by Hub, `with` and machine.
    packages: Mutex<HashMap<(String, String, String), Package>>,
    /// This walk's: the installs its local code asks for are keyed under it, so a failed one is
    /// asked again by the next walk, never replayed.
    walk: String,
    project: Option<String>,
    /// Who waits on those installs.
    installs: Arc<Installs>,
}

impl Host {
    pub(crate) fn new(api: &Arc<Api>) -> Self {
        let mut walk = [0u8; 8];
        getrandom::fill(&mut walk).expect("the OS has randomness");
        Self { api: api.clone(), packages: Mutex::default(), walk: hex::encode(walk), project: crate::private_io::current().map(|scope| scope.storage.project_id), installs: Arc::default() }
    }

    /// Project `project`'s walk, whose jobs wait on its installs as `installs` keeps.
    pub(crate) fn for_project(api: &Arc<Api>, project: &str, installs: Arc<Installs>) -> Self {
        Self { project: Some(project.into()), installs, ..Self::new(api) }
    }
    fn journal(&self) -> crate::store::Result<crate::store::RunStore<'_>> {
        match &self.project {
            Some(project) => self.api.calcifer.project_store(project).map(crate::store::RunStore::Project),
            None => Ok(crate::store::RunStore::Maintenance(self.api.calcifer.maintenance_store())),
        }
    }

    /// What it runs in, for what outlives one of its calls.
    pub(crate) fn api(&self) -> Arc<Api> {
        self.api.clone()
    }

    /// What local code at `path` going `on` a machine is keyed under (`local_code::prepare`).
    pub(crate) fn key(&self, path: &Path, on: &On) -> String {
        let at = Sha256::digest(format!("{}\n{on:?}", path.display()));
        format!("project-local-{}-{}", self.walk, &hex::encode(at)[..16])
    }

    /// The project's local code at `path` made ready `on` a machine for a call, the install it needs waited for as `wait` says: read
    /// into a copy as a client's is sent.
    pub(crate) async fn ready(&self, path: &Path, on: &On, hub: &str, wait: Wait<'_>) -> Result<local_code::Prepared, Error> {
        let scope = crate::private_io::current().or(crate::private_io::of_path(path)?);
        crate::private_io::context(scope, async {
            let code = local_code::own(&self.api, path)?;
            let key = self.key(path, on);
            let (job, member) = match wait {
                Wait::Plain => (None, None),
                Wait::Job(job, member) => (Some(job), member),
                Wait::Quote(job) => (Some(job), None),
            };
            loop {
                if job.is_some_and(|job| self.installs.canceled(job)) {
                    return Err(canceled());
                }
                let prepared = local_code::prepare(&self.api, &code, on, hub, false, &key, None, &|run| self.installs.ended(run)).await?;
                let Some(run) = &prepared.install else { return Ok(prepared) };
                if let Wait::Quote(job) = wait {
                    self.installs.asked(&self.api, &run.id, job)?;
                    return Err(Error::new(Kind::Unavailable, INSTALLING, format!("its install, run {}, is going", run.number)));
                }
                let mut told = self.installs.told.subscribe();
                let _waiting = match job {
                    Some(job) => match self.installs.wait(&self.api, &run.id, job)? {
                        Some(waiting) => Some(waiting),
                        // Ended for want of waiters as this came to it: asked again, it installs again.
                        None => continue,
                    },
                    None => None,
                };
                // What its turn sends together goes without it: they can start now (owner, cl-973).
                if let Some(member) = member {
                    member.leave();
                }
                let canceled_meanwhile = async {
                    if let Some(job) = job {
                        while told.changed().await.is_ok() {
                            if self.installs.canceled(job) {
                                return;
                            }
                        }
                    }
                    std::future::pending().await
                };
                let (watcher, began, number, job) = (Host::new(&self.api), std::time::Instant::now(), run.number, job.unwrap_or("-"));
                crate::private_io::diagnose(path, || format!("{job} waits for its install on {on}, run {number}"));
                let said = |how: &str| crate::private_io::diagnose(path, || format!("{job}'s install on {on}, run {number}, {how} in {} ms", began.elapsed().as_millis()));
                tokio::select! {
                    ended = watcher.wait(&run.id) => drop(ended.inspect(|_| said("ended"))?),
                    () = canceled_meanwhile => {
                        said("left: canceled");
                        return Err(canceled());
                    }
                }
            }
        }).await
    }

    /// The package `with` names: local code (`./dir` from the project's `folder`, `~/…`, an absolute path), described on the first of
    /// `on`'s machines; else `[org/]package[@release]` as `hub` (empty: the default one) has it. Local code is made ready as `wait` says
    /// (`ready`).
    pub(crate) async fn package(&self, hub: &str, with: &str, folder: &Path, on: &[On], wait: Wait<'_>) -> Result<Package, Error> {
        let scope = crate::private_io::of_path(folder)?;
        crate::private_io::context(scope, self.package_scoped(hub, with, folder, on, wait)).await
    }

    async fn package_scoped(&self, hub: &str, with: &str, folder: &Path, on: &[On], wait: Wait<'_>) -> Result<Package, Error> {
        let home = self.api.machines.hubs.home().root().parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(found) = local_code::target(with, Some(folder), &home) {
            let (path, named) = found?;
            let on = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
            let prepared = self.ready(&path, &on, hub, wait).await?;
            let Some(document) = prepared.interface else {
                let said = format!("{with} is not described on {on:?} yet: its first render installs it");
                return Err(Error::new(Kind::Unavailable, "renderer_not_described", said));
            };
            let release = prepared.source.manifest.trim_start_matches("sha256:").chars().take(12).collect();
            return Ok(Package { name: prepared.package, release, local: Some(path), manifest: prepared.source.manifest, moved: None, named, digest: String::new(), document });
        }
        let first = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
        self.published(hub, with, Some(&first)).await
    }

    /// The package `with` names for a job waiting for another's output (cl-905), never made ready on a machine for it: local code as
    /// the static describer reads it on the first of `on`'s machines, with the manifest a machine installs; a published one as `package`
    /// has it. Its install is neither asked for nor waited on, so the job is placed and its machine fetches ahead meanwhile.
    pub(crate) async fn foreseen(&self, hub: &str, with: &str, folder: &Path, on: &[On]) -> Result<Package, Error> {
        let scope = crate::private_io::of_path(folder)?;
        crate::private_io::context(scope, async {
            let home = self.api.machines.hubs.home().root().parent().map(Path::to_path_buf).unwrap_or_default();
            let first = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
            let Some(found) = local_code::target(with, Some(folder), &home) else { return self.published(hub, with, Some(&first)).await };
            let (path, named) = found?;
            let code = local_code::own(&self.api, &path)?;
            let hubs = &self.api.machines.hubs;
            let hub = if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() };
            // Its estimate names it by this (`Package::estimated`): without it, no machine quotes it.
            let manifest = local_code::source(hubs.home(), &code, hubs, &Origin::parse(&hub)?).await?.source.manifest;
            let (name, document) = local_code::describe(&self.api, &code, &first, &hub).await?;
            let release = manifest.trim_start_matches("sha256:").chars().take(12).collect();
            Ok(Package { name, release, local: Some(path), manifest, moved: None, named, digest: String::new(), document })
        }).await
    }

    /// Local code `with`, from `folder`, as described for a read that runs nothing: never made ready on a machine.
    pub(crate) async fn described(&self, with: &str, folder: &Path) -> Result<Package, Error> {
        let hub = self.api.machines.hubs.default_origin();
        let found = crate::runs::target::describe(&self.api, &hub, with, None, Some(folder), &crate::runs::default_on(&self.api)).await?;
        Ok(Package { name: found.package, release: found.release, local: None, manifest: String::new(), moved: None, named: found.function, digest: String::new(), document: found.interface })
    }

    /// A published package, by `[org/]package[@release]`, for a run `on` its machine (of several, the
    /// walk's first; another installs the release); none: as this computer knows it, for a read
    /// that runs nothing.
    pub(crate) async fn published(&self, hub: &str, with: &str, on: Option<&On>) -> Result<Package, Error> {
        let at = (hub.to_owned(), with.to_owned(), on.map(On::to_string).unwrap_or_default());
        if let Some(found) = self.packages.lock().expect("packages lock").get(&at) {
            return Ok(found.clone());
        }
        // As `cozy run` resolves it.
        let hub = crate::runs::hub(&self.api, hub);
        let Resolved::Release { package, function, card, mut moved, upgrade, cozy, .. } = target::published(&self.api, &hub, with, on, true).await? else {
            unreachable!("a published target resolves to a release");
        };
        // cozy updates itself first for a release only a newer cozy's line runs (cl-978): the render waits behind it, and Calcifer
        // restarts on the new cozy, whose walk takes it up again.
        if let Some(wanted) = &cozy {
            let (run, note) = match crate::cozy::update(&self.api, wanted).await {
                // Installed already: this Calcifer restarts on it.
                Err(e) if e.code == api::error::STOPPING => return std::future::pending().await,
                updating => updating?,
            };
            tracing::info!("{}: the render waits for run {}", note.message, run.number);
            let ended = self.own_ended(&run.id).await?;
            if ended.status != RunStatus::Completed {
                return Err(crate::cozy::update(&self.api, wanted).await.err().unwrap_or_else(|| Error::new(Kind::Unavailable, "cozy_update_failed", note.message)));
            }
            // Its walk ends with this Calcifer, which restarts on the updated cozy; the next takes the render up where it stands.
            return std::future::pending().await;
        }
        // Its machine's Runtime moves first to one that runs the release (cl-952): the render waits behind that update.
        if let Some(upgrade) = &upgrade
            && self.api.machines.upgrade(upgrade).await?.is_some()
        {
            moved = Some(upgrade.note());
        }
        let found = Package { name: package, release: card.release.clone(), local: None, manifest: String::new(), moved: None, named: function, digest: card.digest().unwrap_or_default().to_owned(), document: card.interface_document().clone() };
        self.packages.lock().expect("packages lock").insert(at, found.clone());
        Ok(Package { moved, ..found })
    }

    /// How long the newest finished run of `target` on `machine` took, in seconds.
    pub(crate) fn last_took(&self, target: &str, machine: &str) -> Option<f64> {
        let store = self.journal().ok()?;
        let filter = crate::store::RunFilter { status: Some(RunStatus::Completed), target: Some(target.to_owned()), machine: Some(machine.to_owned()), out_dir: None, before: None, limit: 20 };
        let (rows, ..) = store.runs(&filter).ok()?;
        rows.iter().find_map(|row| took_s(&store.detail(row).ok()?))
    }

    /// Sends `run` under `key`: one key is one run, however often it is sent.
    pub(crate) async fn submit(&self, key: &str, run: SubmitRun) -> Result<Run, Error> {
        Ok(self.api.calcifer.submit(key, run).await.map_err(refused)?.run)
    }

    /// Runs a turn of its walk sends `on` one machine: they go together (`Together`).
    pub(crate) fn together(&self, on: On) -> Arc<Together> {
        Arc::new(Together { api: self.api.clone(), on, members: Mutex::default() })
    }

    /// The run sent under `key`, if one was.
    pub(crate) fn sent(&self, key: &str) -> Result<Option<Run>, Error> {
        let calcifer = &self.api.calcifer;
        let row = self.journal().map_err(|e| refused(e.into()))?.keyed(key).map_err(|e| refused(e.into()))?;
        row.map(|row| calcifer.record(&row).map_err(refused)).transpose()
    }

    /// Run `number`, as its record says now.
    pub(crate) fn numbered(&self, number: u64) -> Option<Run> {
        let calcifer = &self.api.calcifer;
        calcifer.record(&self.journal().ok()?.run_by_number(number).ok()??).ok()
    }

    /// The run once it ended and each output landed here or failed to.
    pub(crate) async fn wait(&self, id: &RunId) -> Result<Run, Error> {
        self.until(id, |run| !run.output.iter().any(OutputItem::owed)).await
    }

    /// The run once it ended on its machine, whatever of its outputs is still on its way here.
    pub(crate) async fn ended(&self, id: &RunId) -> Result<Run, Error> {
        self.until(id, |_| true).await
    }

    /// Calcifer's own run `id` once it ended: cozy's update (cl-978), kept in no project's journal.
    async fn own_ended(&self, id: &RunId) -> Result<Run, Error> {
        let calcifer = &self.api.calcifer;
        let mut tail = calcifer.subscribe();
        loop {
            let row = calcifer.maintenance_store().run(id.as_str()).map_err(|e| refused(e.into()))?;
            let row = row.ok_or_else(|| Error::new(Kind::NotFound, "run_not_found", format!("Calcifer holds no run {id}")))?;
            let run = calcifer.record(&row).map_err(refused)?;
            if matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled) {
                return Ok(run);
            }
            crate::Calcifer::moved(&mut tail, id).await;
        }
    }

    async fn until(&self, id: &RunId, and: impl Fn(&Run) -> bool) -> Result<Run, Error> {
        let calcifer = &self.api.calcifer;
        let mut tail = calcifer.subscribe();
        loop {
            let row = self.journal().and_then(|store|store.run(id.as_str())).map_err(|e| refused(e.into()))?;
            let row =
                row.ok_or_else(|| Error::new(Kind::NotFound, "run_not_found", format!("Calcifer holds no run {id}")))?;
            let run = calcifer.record(&row).map_err(refused)?;
            if matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled) && and(&run) {
                return Ok(run);
            }
            crate::Calcifer::moved(&mut tail, id).await;
        }
    }

    /// Whether this computer reaches the Hub at all: a connect, no call, so the Hub learns nothing. With no network here, what didn't
    /// answer says nothing of itself.
    pub(crate) async fn online(&self) -> bool {
        // A connect gives no other word of a path that swallows it: the one signal-less wait here, and it only delays the next look.
        const CONNECT: Duration = Duration::from_secs(10);
        let Ok(hub) = reqwest::Url::parse(self.api.machines.hubs.hub.as_str()) else { return true };
        let (Some(host), Some(port)) = (hub.host_str(), hub.port_or_known_default()) else { return true };
        matches!(tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect((host, port))).await, Ok(Ok(_)))
    }

    /// The machine among `on` that already holds these bytes for this computer.
    pub(crate) async fn holding(&self, sha256: &str, length: u64, on: &[On]) -> Option<On> {
        let digest: Digest = sha256.parse().ok()?;
        for machine in on {
            let On::Machine(text) = machine else { continue };
            let machines = &self.api.machines;
            if text == "local" && machines.local_up().await.is_err() {
                continue;
            }
            let Ok((_, link)) = machines.linked(text) else { continue };
            if length > 0 && link.held(&digest, length).await.is_ok_and(|held| held >= length) {
                return Some(machine.clone());
            }
        }
        None
    }

    /// Whether `machine` is there to fetch `sha256` from: it hasn't ended, its Hub doesn't call it offline, and it hasn't evicted them.
    pub(crate) async fn fetchable(&self, sha256: &str, machine: &str) -> bool {
        let machines = &self.api.machines;
        machines.named(machine).is_ok_and(|row| row.is_some_and(|row| row.state != MachineState::Ended))
            && machines.offline(machine).is_none()
            && self.api.calcifer.still_holds(sha256, machine).await
    }

    /// `sha256`, which `machine` keeps, brought here now as an export brings an output: into its project's `files/`. Err: why not.
    pub(crate) async fn fetch(&self, sha256: &str, machine: &str) -> Result<(), String> {
        self.api.calcifer.bring(sha256, machine).await
    }

    /// One file of `project` from `hub`, when it holds it. Err: why not.
    pub(super) async fn pull_one(&self, project: &super::Project, hub: &str, file: super::folder::File) -> Result<(), String> {
        super::remote::pull_one(&self.api, project, hub, file).await
    }

    /// Cancels run `number` on `machine` if it has not begun.
    pub(crate) fn cancel_unstarted(&self, number: u64, machine: &str) {
        let calcifer = &self.api.calcifer;
        let Ok(Some(row)) = self.journal().and_then(|store| store.run_by_number(number)) else { return };
        if row.machine.as_deref() == Some(machine) && row.started_at_ms.is_none() && row.state != crate::store::State::Settled {
            let _ = calcifer.cancel(&row.id);
        }
    }

    /// What run `id`'s machine quoted as it was sent: its time left waits at least until then.
    /// Where the plan sent project `project`'s run `id`, and its offer there, on its record (cl-967).
    pub(crate) fn placed(&self, project: &str, id: &RunId, placed: &api::calcifer::Placed) {
        self.api.calcifer.placed(project, id, placed);
    }

    pub(crate) fn quoted(&self, id: &RunId, quoted: crate::eta::Quoted) {
        self.api.calcifer.quoted(id, quoted);
    }

    /// cozy's home.
    pub(crate) fn home(&self) -> &platform::paths::Home {
        self.api.machines.hubs.home()
    }

    /// cozy's cache folder for this user (`platform::paths::cache`), else one in its home.
    pub(crate) fn cache(&self) -> std::path::PathBuf {
        platform::paths::cache().unwrap_or_else(|| self.api.machines.hubs.home().root().join("cache"))
    }

    /// cozy's ffmpeg and ffprobe: the pinned build `cozy` fetched, else PATH's.
    pub(crate) async fn ffmpeg(&self) -> Result<(PathBuf, PathBuf), Error> {
        let home = self.api.machines.hubs.home();
        let found = crate::tools::ffmpeg(home, &|line| tracing::info!("{line}")).await?;
        Ok((found.ffmpeg, found.ffprobe))
    }
}

fn refused(error: crate::Error) -> Error {
    Refusal::from(error).0
}

/// How making local code ready waits for the install it needs (`Host::ready`).
#[derive(Clone, Copy)]
pub(crate) enum Wait<'a> {
    /// It waits, for no job.
    Plain,
    /// It waits for job `job`, until that's canceled (`Installs`), out of the runs its turn sends together meanwhile (`Member::leave`).
    Job(&'a str, Option<&'a Member>),
    /// It doesn't: `INSTALLING` answers, and the install ends with job `job` as one it waited on would (`Installs::asked`). A plan's
    /// quote, which every job of its turn waits on.
    Quote(&'a str),
}

/// What local code made ready without waiting (`Wait::Quote`) answers while its install is going.
pub(crate) const INSTALLING: &str = "renderer_installing";

/// What a job waiting on its install answers once it was canceled (`Installs`): it sends nothing, and fails nothing.
pub(crate) const CANCELED: &str = "render_canceled";

fn canceled() -> Error {
    Error::new(Kind::Conflict, CANCELED, "canceled while it waited for its install")
}

/// The installs a project's walk asks for its local code (`Host::ready`), and the jobs waiting on each or whose quotes asked for it. An
/// install that only canceled jobs wanted ends with the last of them, unless a run of its package that hasn't begun goes to its machine
/// too (owner, cl-973): an install a person asked for is their own run, under their own key, which no job waits on. Its installs are
/// keyed by the walk, so a walk starts it empty.
#[derive(Default)]
pub(crate) struct Installs {
    waits: Mutex<Waits>,
    /// Bumped as jobs are canceled: a job waiting looks whether it was one.
    told: tokio::sync::watch::Sender<u64>,
}

#[derive(Default)]
struct Waits {
    next: u64,
    /// Each install's waiters, by token, with the job each waits for.
    waiting: HashMap<RunId, Vec<(u64, String)>>,
    /// The jobs canceled: none waits again.
    canceled: HashSet<String>,
    /// The installs ended for want of waiters: one asked again installs again (`local_code::install_through`).
    ended: HashSet<RunId>,
    /// The install each job's quote found going, and didn't wait on (`Wait::Quote`).
    asked: HashMap<String, RunId>,
}

impl Waits {
    /// Whether a job waits on install `run`, or its quote asked for it.
    fn wanted(&self, run: &RunId) -> bool {
        self.waiting.contains_key(run) || self.asked.values().any(|asked| asked == run)
    }
}

/// A job's wait on an install, until it drops.
struct Waiting<'a> {
    installs: &'a Installs,
    run: RunId,
    token: u64,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut waits = self.installs.lock();
        if let Some(waiters) = waits.waiting.get_mut(&self.run) {
            waiters.retain(|(token, _)| *token != self.token);
            if waiters.is_empty() {
                waits.waiting.remove(&self.run);
            }
        }
    }
}

impl Installs {
    fn lock(&self) -> std::sync::MutexGuard<'_, Waits> {
        self.waits.lock().expect("installs lock")
    }

    /// A walk starting: what the last one's jobs waited on is over with it.
    pub(crate) fn clear(&self) {
        *self.lock() = Waits::default();
    }

    fn canceled(&self, job: &str) -> bool {
        self.lock().canceled.contains(job)
    }

    /// Whether install `run` ended for want of waiters.
    fn ended(&self, run: &RunId) -> bool {
        self.lock().ended.contains(run)
    }

    /// Job `job` waiting on install `run`; none: it ended for want of waiters as this came to it. A job canceled meanwhile waits on
    /// nothing, and the install it asked for ends unless something else waits on it.
    fn wait(&self, api: &Api, run: &RunId, job: &str) -> Result<Option<Waiting<'_>>, Error> {
        let mut waits = self.lock();
        if waits.ended.contains(run) {
            return Ok(None);
        }
        if waits.canceled.contains(job) {
            let alone = !waits.wanted(run);
            drop(waits);
            if alone {
                self.end(api, run);
            }
            return Err(canceled());
        }
        waits.next += 1;
        let token = waits.next;
        waits.waiting.entry(run.clone()).or_default().push((token, job.to_owned()));
        Ok(Some(Waiting { installs: self, run: run.clone(), token }))
    }

    /// Job `job`'s quote found install `run` going, and didn't wait on it: it ends with the job, as one the job waited on would. A job
    /// canceled meanwhile asked for nothing, and the install ends unless something else waits on it.
    fn asked(&self, api: &Api, run: &RunId, job: &str) -> Result<(), Error> {
        let mut waits = self.lock();
        if !waits.canceled.contains(job) {
            waits.asked.insert(job.to_owned(), run.clone());
            return Ok(());
        }
        let alone = !waits.wanted(run);
        drop(waits);
        if alone {
            self.end(api, run);
        }
        Err(canceled())
    }

    /// Jobs `jobs` canceled (`stop_jobs`): none waits again, and each install only they waited on, or their quotes asked for, ends with
    /// them.
    pub(crate) fn cancel(&self, api: &Api, jobs: &[String]) {
        if jobs.is_empty() {
            return;
        }
        let alone: HashSet<RunId> = {
            let mut waits = self.lock();
            waits.canceled.extend(jobs.iter().cloned());
            let mut theirs = HashSet::new();
            waits.waiting.retain(|run, waiters| {
                let before = waiters.len();
                waiters.retain(|(_, job)| !jobs.contains(job));
                if waiters.len() < before {
                    theirs.insert(run.clone());
                }
                !waiters.is_empty()
            });
            theirs.extend(jobs.iter().filter_map(|job| waits.asked.remove(job)));
            theirs.into_iter().filter(|run| !waits.wanted(run)).collect()
        };
        self.told.send_modify(|told| *told += 1);
        for run in &alone {
            self.end(api, run);
        }
    }

    /// Install `run`, which no job waits on, canceled: unless a run of its package that hasn't begun goes to its machine too, as it
    /// waits there on this install (TensorD installs one package's code in turn), or a job came to wait on it meanwhile.
    fn end(&self, api: &Api, run: &RunId) {
        if wanted_elsewhere(api, run) {
            return;
        }
        {
            let mut waits = self.lock();
            if waits.wanted(run) {
                return;
            }
            waits.ended.insert(run.clone());
        }
        if let Err(error) = api.calcifer.cancel(run) {
            self.lock().ended.remove(run);
            tracing::warn!("install {run}, which no render waits on now, goes on: its cancel wasn't recorded: {error}");
        }
    }
}

/// Whether install `id` is wanted by another run: one of its package, not begun, going to its machine or to none chosen yet. Also when
/// it can't be read, or has ended: nothing of it is ended then.
fn wanted_elsewhere(api: &Api, id: &RunId) -> bool {
    let calcifer = &api.calcifer;
    let Ok(Some(install)) = calcifer.run_store(id.as_str()).and_then(|store| store.run(id.as_str())) else { return true };
    if install.state == State::Settled {
        return true;
    }
    let package = local_code::package_of(&install.target);
    let waits = |row: &crate::store::RunRow| {
        row.id != *id && row.started_at_ms.is_none() && (row.machine.is_none() || row.machine == install.machine) && local_code::package_of(&row.target) == package
    };
    let found = calcifer.each_store(|store| {
        let rows = (store.unsettled()?.into_iter()).map(|other| store.run(other.as_str())).collect::<crate::store::Result<Vec<_>>>()?;
        Ok(rows.iter().flatten().any(waits))
    });
    found.map_or(true, |found| found.contains(&true))
}

/// The runs one turn of a walk sends to one machine, which go there in one admission (cl-816): it packs them together, as it
/// quoted them. Each member submits its run or leaves, as it drops; once each has, they go. No clock: every member's send ends.
pub(crate) struct Together {
    api: Arc<Api>,
    /// The machine they go to, as their quotes chose it.
    pub on: On,
    members: Mutex<Members>,
}

#[derive(Default)]
struct Members {
    left: usize,
    runs: Vec<Submitted>,
}

/// A run submitted under its key, and where its answer goes.
type Submitted = (String, SubmitRun, tokio::sync::oneshot::Sender<Result<Run, Error>>);

/// What a member's submit answers when the runs it went with were dropped unsent.
pub(crate) const UNSENT: &str = "run_unsent";

/// One run's place among those that go together, until it submits or leaves.
pub(crate) struct Member(Mutex<Option<Arc<Together>>>);

impl Together {
    pub(crate) fn member(self: &Arc<Self>) -> Member {
        self.members.lock().expect("members lock").left += 1;
        Member(Mutex::new(Some(self.clone())))
    }

    /// One member submitted `run`, or left: the last sends what was submitted.
    fn left(&self, run: Option<Submitted>) {
        let runs = {
            let mut members = self.members.lock().expect("members lock");
            members.runs.extend(run);
            members.left -= 1;
            match members.left == 0 && !members.runs.is_empty() {
                true => std::mem::take(&mut members.runs),
                false => return,
            }
        };
        let (Ok(runtime), calcifer) = (tokio::runtime::Handle::try_current(), self.api.calcifer.clone()) else { return };
        runtime.spawn(async move {
            let (asks, answers): (Vec<_>, Vec<_>) = runs.into_iter().map(|(key, run, answer)| ((key, run), answer)).unzip();
            for (answer, submitted) in answers.into_iter().zip(calcifer.submit_batch(asks).await) {
                let _ = answer.send(submitted.map(|submitted| submitted.run).map_err(refused));
            }
        });
    }
}

impl Member {
    fn take(&self) -> Option<Arc<Together>> {
        self.0.lock().expect("member lock").take()
    }

    /// The machine its run goes to with the others; none once it left them.
    pub(crate) fn on(&self) -> Option<On> {
        self.0.lock().expect("member lock").as_ref().map(|together| together.on.clone())
    }

    /// Leaves the others, which go without it: it waits for an install, and is sent alone once that's done (owner, cl-973).
    pub(crate) fn leave(&self) {
        if let Some(together) = self.take() {
            together.left(None);
        }
    }

    /// Submits `run` under `key` with the others: its run, once they all have.
    pub(crate) async fn submit(self, key: &str, run: SubmitRun) -> Result<Run, Error> {
        let unsent = || Error::new(Kind::Unavailable, UNSENT, "the runs it went with were dropped before they were sent");
        let Some(together) = self.take() else { return Err(unsent()) };
        let (answer, answered) = tokio::sync::oneshot::channel();
        together.left(Some((key.to_owned(), run, answer)));
        answered.await.unwrap_or_else(|_| Err(unsent()))
    }
}

impl Drop for Member {
    fn drop(&mut self) {
        self.leave();
    }
}
