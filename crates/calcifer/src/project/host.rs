//! What a project's walk asks of the Calcifer it runs in, and nothing else: the Hub's packages,
//! sending and following runs, the machines' load, and ffmpeg. A walk hosted elsewhere (the Hub)
//! answers these same calls.
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use api::{
    calcifer::{MachineState, On, OutputItem, Run, Status as RunStatus, SubmitRun},
    error::{Error, Kind},
    ids::{Digest, RunId},
    interface::{self, Call, Interface, Slot},
    model_ref::ModelRef,
    names::shown,
    calcifer::project::Event,
    calcifer::tool,
};
use serde_json::Value;

use sha2::{Digest as _, Sha256};
use tokio::sync::broadcast;

use crate::{
    api::{Api, Refusal},
    local_code,
    runs::target::{self, Resolved},
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
    /// A yanked release this walk moved off, said once.
    pub moved: Option<String>,
    /// The function its tool names; none: the package alone.
    pub named: Option<String>,
    document: Value,
}

impl Package {
    /// Its interface, as its card or its source says.
    pub(crate) fn document(&self) -> &Value {
        &self.document
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

    /// The widths `function` runs at, smallest first: what it loads declares, one for all. A tool that composes calls runs each where its
    /// GPUs are; while a render lands on one machine, that's the widths of its first call that takes a GPU. Empty: it needs no GPU.
    pub(crate) fn widths(&self, function: &str) -> Vec<u32> {
        let document = &self.document;
        let Some(entry) = interface::entrypoints(document).find(|entry| interface::name(entry) == Some(function)) else { return vec![] };
        if interface::cpu(entry) {
            let calls = self.callable(function).map(|callable| callable.calls).unwrap_or_default();
            return calls.iter().find(|call| call.gpu()).map(Call::widths).unwrap_or_default();
        }
        let mut common: BTreeSet<u32> = (1..=64).collect();
        for model in interface::models(entry).iter().filter(|model| model["weights"] != true) {
            let mut declared = BTreeSet::from([1u32]);
            for degree in model.pointer("/sequence_parallel/degrees").and_then(Value::as_array).into_iter().flatten() {
                declared.extend(degree.as_u64().map(|degree| degree as u32));
            }
            common = common.intersection(&declared).copied().collect();
        }
        if common.len() == 64 {
            return vec![1];
        }
        common.into_iter().collect()
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
        Renderer::read(&self.name, function, self.release.clone(), &self.document, self.local.clone())
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
            .ok_or_else(|| Error::new(Kind::NotFound, "callable_unknown", unknown()).next(format!("cozy package update {}", shown(package))))?;
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

/// What a run measured.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Measured {
    pub busy_s: f64,
    pub width: u32,
    pub exchange_share: f64,
}

impl Measured {
    /// From what its executors recorded: a stage is time it worked; a `host ·` row inside one is where that went, and one in `parallel` is exchange. A wait counts nowhere.
    fn of(run: &Run) -> Option<Self> {
        let (mut busy, mut exchange, mut width) = (0.0, 0.0, 0u32);
        let calls = run.calls.iter().filter_map(|call| call.measurements.as_ref());
        for measured in run.measurements.iter().chain(calls) {
            for (name, stage) in measured.pointer("/attribution/stages").and_then(Value::as_object).into_iter().flatten() {
                let ms = stage["total_ms"].as_f64().unwrap_or(0.0);
                match name.strip_prefix("host · ") {
                    Some(code) if code.contains(".parallel.") => exchange += ms,
                    Some(_) => {}
                    None => busy += ms,
                }
            }
            width = width.max(measured.pointer("/execution/degree").and_then(Value::as_u64).unwrap_or(0) as u32);
        }
        // What it took is its run's own time: the stages of an orchestrator and of its calls are the same time, counted twice.
        let wall = run.execution_ms.map(|ms| ms as f64).filter(|ms| *ms > 0.0).unwrap_or(busy);
        (busy > 0.0).then(|| Self { busy_s: wall / 1000.0, width: width.max(1), exchange_share: (exchange / busy).min(1.0) })
    }
}

pub(crate) struct Host {
    api: Arc<Api>,
    /// Read once a walk, by Hub, `with` and machine.
    packages: Mutex<HashMap<(String, String, String), Package>>,
    /// This walk's: the installs its local code asks for are keyed under it, so a failed one is
    /// asked again by the next walk, never replayed.
    walk: String,
    /// Where a walk's listeners hear.
    events: Option<broadcast::Sender<Event>>,
}

impl Host {
    pub(crate) fn new(api: &Arc<Api>) -> Self {
        let mut walk = [0u8; 8];
        getrandom::fill(&mut walk).expect("the OS has randomness");
        Self { api: api.clone(), packages: Mutex::default(), walk: hex::encode(walk), events: None }
    }

    /// A host for a walk, whose listeners on `events` hear what it does.
    pub(crate) fn telling(api: &Arc<Api>, events: broadcast::Sender<Event>) -> Self {
        Self { events: Some(events), ..Self::new(api) }
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

    /// The project's local code at `path` made ready `on` a machine for a call, an install waited for: read into a copy as a
    /// client's is sent.
    pub(crate) async fn ready(&self, path: &Path, on: &On, hub: &str) -> Result<local_code::Prepared, Error> {
        let code = local_code::own(self.api.machines.hubs.home(), path)?;
        local_code::ready(&self.api, &code, on, hub, &self.key(path, on), &|message| self.tell(message)).await
    }

    /// What a walk's listeners hear of what this does for it.
    fn tell(&self, message: String) {
        if let Some(events) = &self.events {
            let _ = events.send(Event::Warning { message, name: None });
        }
    }

    /// A release's first run on a machine older than this Calcifer waits for its install (`update::release_first`).
    pub(crate) async fn release_ready(&self, machine: &str, package: &str, release: &str, hub: &str) -> Result<(), Error> {
        let at = Sha256::digest(format!("{package}@{release}\n{machine}"));
        let key = format!("project-release-{}-{}", self.walk, &hex::encode(at)[..16]);
        while let Some(run) = crate::update::release_first(&self.api, &key, machine, hub, package, release).await? {
            local_code::said_on(&run).for_each(|message| self.tell(message));
            self.wait(&run.id).await?;
        }
        Ok(())
    }

    /// The package `with` names: local code (`./dir` from the project's `folder`, `~/…`, an absolute path), described on the first of
    /// `on`'s machines; else `[org/]package[@release]` as `hub` (empty: the default one) has it.
    pub(crate) async fn package(&self, hub: &str, with: &str, folder: &Path, on: &[On]) -> Result<Package, Error> {
        let home = self.api.machines.hubs.home().root().parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(found) = local_code::target(with, Some(folder), &home) {
            let (path, named) = found?;
            let on = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
            let prepared = self.ready(&path, &on, hub).await?;
            let Some(document) = prepared.interface else {
                let said = format!("{with} is not described on {on:?} yet: its first render installs it");
                return Err(Error::new(Kind::Unavailable, "renderer_not_described", said));
            };
            let release = prepared.source.manifest.trim_start_matches("sha256:").chars().take(12).collect();
            return Ok(Package { name: prepared.package, release, local: Some(path), moved: None, named, document });
        }
        let first = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
        self.published(hub, with, Some(&first)).await
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
        let Resolved::Release { package, function, card, moved, .. } = target::published(&self.api, &hub, with, on, true).await? else {
            unreachable!("a published target resolves to a release");
        };
        let found = Package { name: package, release: card.release.clone(), local: None, moved: None, named: function, document: card.interface_document().clone() };
        self.packages.lock().expect("packages lock").insert(at, found.clone());
        Ok(Package { moved, ..found })
    }

    /// How many GPUs a machine has, as it last said. One that hasn't said is taken as wide: a job's width is a cap, which its machine brings down to what there is.
    pub(crate) fn gpus(&self, on: &On) -> u32 {
        let On::Machine(name) = on else { return 64 };
        self.api.machines.frame(name).map_or(64, |frame| (frame.gpus.len() as u32).max(1))
    }

    /// The card a machine has, as it last said: its first GPU's name. None: it hasn't said, or has none.
    pub(crate) fn card(&self, machine: &str) -> Option<String> {
        self.api.machines.frame(machine)?.gpus.first().map(|gpu| gpu.name.clone())
    }

    /// What run `number` measured: its time on GPUs and in what it did itself, the width it had, and the share of it spent exchanging between GPUs.
    pub(crate) fn measured(&self, number: u64) -> Option<Measured> {
        let store = self.api.calcifer.store();
        let row = store.run_by_number(number).ok()??;
        let run = store.detail(&row).ok()?;
        (run.status == RunStatus::Completed).then(|| Measured::of(&run)).flatten()
    }

    /// What the newest finished run of `target` on `machine` measured.
    /// What the newest finished runs of `target` on `machine` measured, the newest at each width.
    pub(crate) fn last_measured(&self, target: &str, machine: &str) -> Vec<Measured> {
        let store = self.api.calcifer.store();
        let filter = crate::store::RunFilter { status: Some(RunStatus::Completed), target: Some(target.to_owned()), machine: Some(machine.to_owned()), out_dir: None, before: None, limit: 20 };
        let Ok((rows, ..)) = store.runs(&filter) else { return vec![] };
        let mut found: Vec<Measured> = vec![];
        for row in &rows {
            let Ok(run) = store.detail(row) else { continue };
            let Some(measured) = Measured::of(&run) else { continue };
            if found.iter().any(|had| had.width == measured.width) {
                continue;
            }
            found.push(measured);
        }
        found
    }

    /// Stops a run fetching ahead, however far it got. Nothing depends on it.
    pub(crate) fn cancel_ahead(&self, run: &RunId) {
        let _ = self.api.calcifer.cancel(run);
    }

    /// Sends `run` under `key`: one key is one run, however often it is sent.
    pub(crate) async fn submit(&self, key: &str, run: SubmitRun) -> Result<Run, Error> {
        Ok(self.api.calcifer.submit(key, run).await.map_err(refused)?.run)
    }

    /// The run sent under `key`, if one was.
    pub(crate) fn sent(&self, key: &str) -> Result<Option<Run>, Error> {
        let calcifer = &self.api.calcifer;
        let row = calcifer.store().keyed(key).map_err(|e| refused(e.into()))?;
        row.map(|row| calcifer.record(&row).map_err(refused)).transpose()
    }

    /// Run `number`, as its record says now.
    pub(crate) fn numbered(&self, number: u64) -> Option<Run> {
        let calcifer = &self.api.calcifer;
        calcifer.record(&calcifer.store().run_by_number(number).ok()??).ok()
    }

    /// The run once it ended and each output landed here or failed to.
    pub(crate) async fn wait(&self, id: &RunId) -> Result<Run, Error> {
        self.until(id, |run| !run.output.iter().any(OutputItem::owed)).await
    }

    /// The run once it ended on its machine, whatever of its outputs is still on its way here.
    pub(crate) async fn ended(&self, id: &RunId) -> Result<Run, Error> {
        self.until(id, |_| true).await
    }

    async fn until(&self, id: &RunId, and: impl Fn(&Run) -> bool) -> Result<Run, Error> {
        let calcifer = &self.api.calcifer;
        let mut tail = calcifer.subscribe();
        loop {
            let row = calcifer.store().run(id.as_str()).map_err(|e| refused(e.into()))?;
            let row =
                row.ok_or_else(|| Error::new(Kind::NotFound, "run_not_found", format!("Calcifer holds no run {id}")))?;
            let run = calcifer.record(&row).map_err(refused)?;
            if matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled) && and(&run) {
                return Ok(run);
            }
            tokio::select! {
                _ = tail.recv() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
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
        let Ok(Some(row)) = calcifer.store().run_by_number(number) else { return };
        if row.machine.as_deref() == Some(machine) && row.started_at_ms.is_none() && row.state != crate::store::State::Settled {
            let _ = calcifer.cancel(&row.id);
        }
    }

    /// Where a run of `renderer` goes among `on`: one is where it goes; of several, the machine
    /// with the fewest runs queued or running, one without the renderer's models counting one
    /// more, a warm one first among equals, then the first listed.
    pub(crate) fn place(&self, on: &[On], renderer: &Renderer) -> On {
        if let [one] = on {
            return one.clone();
        }
        let load = self.api.calcifer.store().machine_load().unwrap_or_default();
        let models: Vec<String> = (renderer.slots.iter().chain(renderer.call_slots()))
            .filter_map(|slot| match ModelRef::parse(slot.default.as_deref()?) {
                Ok(ModelRef::Hub { repo, .. }) => Some(repo.to_string()),
                _ => None,
            })
            .collect();
        let machines = &self.api.machines;
        let cost = |on: &On| {
            let On::Machine(text) = on else { return (u32::MAX, true) };
            let Ok(Some(row)) = machines.named(text) else { return (u32::MAX, true) };
            let (running, queued) = load.get(&row.name).copied().unwrap_or_default();
            let held = |frame: api::machine::StatusFrame| {
                models.iter().all(|model| frame.models.iter().any(|held| held.repository == *model))
            };
            let cold = !machines.frame(&row.name).is_some_and(held);
            (running + queued + u32::from(cold), cold)
        };
        on.iter().min_by_key(|on| cost(on)).cloned().unwrap_or(On::Any)
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
