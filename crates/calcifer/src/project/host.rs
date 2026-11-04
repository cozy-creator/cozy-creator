//! What a project's walk asks of the Calcifer it runs in, and nothing else: the Hub's packages,
//! sending and following runs, the machines' load, and ffmpeg. A walk hosted elsewhere (the Hub)
//! answers these same calls.
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use protocol::{
    calcifer::{MachineState, On, OutputItem, Run, Status as RunStatus, SubmitRun},
    error::{Error, Kind},
    ids::{Digest, RunId},
    interface::{self, ADAPTER, Slot},
    model_ref::ModelRef,
    project::Event,
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
pub struct Package {
    pub name: String,
    /// For local code, its source's digest.
    pub release: String,
    /// Local code: its package directory or script, made ready on the machine each call goes to.
    pub local: Option<PathBuf>,
    /// A yanked release this walk moved off, said once.
    pub moved: Option<String>,
    document: Value,
}

/// The project format its record is.
const FORMAT: u64 = 2;

impl Package {
    /// The function of its package that turns an operation of `kind` into calls, if it reads this format. With none, this package can't make that kind.
    pub fn adapter(&self, kind: &str) -> Option<Renderer> {
        let name = format!("{ADAPTER}{kind}");
        let entry = interface::entrypoints(&self.document).find(|entry| interface::name(entry) == Some(name.as_str()))?;
        let version = entry["request"]["fields"].as_array()?.iter().find(|field| field["name"] == "version")?;
        let reads = version["type"]["literal"].as_array()?.iter().any(|value| value.as_u64() == Some(FORMAT));
        reads.then(|| self.function(&name)).and_then(Result::ok)
    }

    /// The tags its adapters declare as their own (`App.project_adapter(reserved=)`): in a text they are text.
    pub fn reserved(&self) -> Vec<String> {
        let adapters = interface::entrypoints(&self.document).filter(|entry| interface::name(entry).is_some_and(|name| name.starts_with(ADAPTER)));
        let mut tags: Vec<String> = adapters.flat_map(|entry| entry["reserved"].as_array().into_iter().flatten()).filter_map(|tag| Some(tag.as_str()?.to_owned())).collect();
        tags.sort_unstable();
        tags.dedup();
        tags
    }

    /// `function` of it.
    pub fn function(&self, function: &str) -> Result<Renderer, Error> {
        Renderer::read(&self.name, function, self.release.clone(), &self.document, self.local.clone())
    }
}

/// One function of a package, as a run of it is sent.
#[derive(Clone)]
pub struct Renderer {
    pub package: String,
    pub function: String,
    /// For local code, its source's digest.
    pub release: String,
    /// Local code: its package directory or script, made ready on the machine each call goes to.
    pub local: Option<PathBuf>,
    /// What its request takes, read from its schema.
    pub accepts: BTreeSet<String>,
    /// An adapter's typed `fields`, if it types them: a field the asset wrote that isn't one is warned of.
    pub signature: Option<BTreeSet<String>>,
    /// Its model slots and those of the calls it makes, for a request's `models:` and `lora:`.
    pub slots: Vec<Slot>,
    pub calls: Vec<Slot>,
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
            .ok_or_else(|| Error::new(Kind::NotFound, "callable_unknown", unknown()))?;
        let read = |slots: Vec<&Value>| slots.into_iter().filter_map(|slot| Slot::read(slot).ok()).collect();
        let fields = entry["request"]["fields"].as_array();
        // A list with no room for anything is not taken.
        let accepts = (fields.into_iter().flatten())
            .filter(|field| field["constraints"]["max_length"].as_u64() != Some(0))
            .filter_map(|field| field["name"].as_str().map(str::to_owned))
            .collect();
        let typed = fields.and_then(|fields| fields.iter().find(|field| field["name"] == "fields"));
        let signature = typed.and_then(|field| Some(field["type"]["fields"].as_array()?.iter().filter_map(|field| field["name"].as_str().map(str::to_owned)).collect()));
        Ok(Self {
            package: package.into(),
            function: function.into(),
            release,
            local,
            accepts,
            signature,
            slots: read(interface::models(entry).iter().collect()),
            calls: read(interface::call_slots(document, entry)),
        })
    }

    /// Its run's name for people: `org/package/function@release`, or `local/<name>/function`.
    pub fn target(&self) -> String {
        match self.local {
            None => format!("{}/{}@{}", self.package, self.function, self.release),
            Some(_) => format!("{}/{}", self.package, self.function),
        }
    }
}

pub struct Host {
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
    pub fn new(api: &Arc<Api>) -> Self {
        let mut walk = [0u8; 8];
        getrandom::fill(&mut walk).expect("the OS has randomness");
        Self { api: api.clone(), packages: Mutex::default(), walk: hex::encode(walk), events: None }
    }

    /// A host for a walk, whose listeners on `events` hear what it does.
    pub fn telling(api: &Arc<Api>, events: broadcast::Sender<Event>) -> Self {
        Self { events: Some(events), ..Self::new(api) }
    }

    /// What local code at `path` going `on` a machine is keyed under (`local_code::prepare`).
    pub fn key(&self, path: &Path, on: &On) -> String {
        let at = Sha256::digest(format!("{}\n{on:?}", path.display()));
        format!("project-local-{}-{}", self.walk, &hex::encode(at)[..16])
    }

    /// Local code at `path` made ready `on` a machine for a call, an install waited for.
    pub async fn ready(&self, path: &Path, on: &On, hub: &str) -> Result<local_code::Prepared, Error> {
        local_code::ready(&self.api, path, on, hub, &self.key(path, on), &|message| self.tell(message)).await
    }

    /// What a walk's listeners hear of what this does for it.
    fn tell(&self, message: String) {
        if let Some(events) = &self.events {
            let _ = events.send(Event::Warning { message });
        }
    }

    /// A release's first run on a machine older than this Calcifer waits for its install (`update::release_first`).
    pub async fn release_ready(&self, machine: &str, package: &str, release: &str, hub: &str) -> Result<(), Error> {
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
    pub async fn package(&self, hub: &str, with: &str, folder: &Path, on: &[On]) -> Result<Package, Error> {
        let home = self.api.machines.hubs.home().root().parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(found) = local_code::target(with, folder, &home) {
            let (path, _) = found?;
            let on = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
            let prepared = self.ready(&path, &on, hub).await?;
            let Some(document) = prepared.interface else {
                let said = format!("{with} is not described on {on:?} yet: its first render installs it");
                return Err(Error::new(Kind::Unavailable, "renderer_not_described", said));
            };
            let release = prepared.source.manifest.trim_start_matches("sha256:").chars().take(12).collect();
            return Ok(Package { name: prepared.package, release, local: Some(path), moved: None, document });
        }
        let first = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
        self.published(hub, with, Some(&first)).await
    }

    /// A published package, by `[org/]package[@release]`, for a run `on` its machine (of several, the
    /// walk's first; another installs the release); none: as this computer knows it, for a read
    /// that runs nothing.
    pub async fn published(&self, hub: &str, with: &str, on: Option<&On>) -> Result<Package, Error> {
        let at = (hub.to_owned(), with.to_owned(), on.map(On::to_string).unwrap_or_default());
        if let Some(found) = self.packages.lock().expect("packages lock").get(&at) {
            return Ok(found.clone());
        }
        // As `cozy run` resolves it.
        let hub = crate::runs::hub(&self.api, hub);
        let Resolved::Release { package, card, moved, .. } = target::published(&self.api, &hub, with, on).await? else {
            unreachable!("a published target resolves to a release");
        };
        let found = Package { name: package, release: card.release.clone(), local: None, moved: None, document: card.interface_document().clone() };
        self.packages.lock().expect("packages lock").insert(at, found.clone());
        Ok(Package { moved, ..found })
    }

    /// Sends `run` under `key`: one key is one run, however often it is sent.
    pub async fn submit(&self, key: &str, run: SubmitRun) -> Result<Run, Error> {
        Ok(self.api.calcifer.submit(key, run).await.map_err(refused)?.run)
    }

    /// The run sent under `key`, if one was.
    pub fn sent(&self, key: &str) -> Result<Option<Run>, Error> {
        let calcifer = &self.api.calcifer;
        let row = calcifer.store().keyed(key).map_err(|e| refused(e.into()))?;
        row.map(|row| calcifer.record(&row).map_err(refused)).transpose()
    }

    /// The run once it ended and each output landed here or failed to.
    pub async fn wait(&self, id: &RunId) -> Result<Run, Error> {
        let calcifer = &self.api.calcifer;
        let mut tail = calcifer.subscribe();
        loop {
            let row = calcifer.store().run(id.as_str()).map_err(|e| refused(e.into()))?;
            let row =
                row.ok_or_else(|| Error::new(Kind::NotFound, "run_not_found", format!("Calcifer holds no run {id}")))?;
            let run = calcifer.record(&row).map_err(refused)?;
            if matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled) && !run.output.iter().any(OutputItem::owed) {
                return Ok(run);
            }
            tokio::select! {
                _ = tail.recv() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }

    /// The machine among `on` that already holds these bytes for this computer.
    pub async fn holding(&self, sha256: &str, length: u64, on: &[On]) -> Option<On> {
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

    /// Whether `machine` is there to fetch `sha256` from: Calcifer knows the run on it that made these bytes, it hasn't ended, and its
    /// Hub doesn't call it offline.
    pub fn fetchable(&self, sha256: &str, machine: &str) -> bool {
        let (machines, store) = (&self.api.machines, self.api.calcifer.store());
        store.making(sha256, machine).is_ok_and(|made| made.is_some())
            && machines.here(machine).is_ok_and(|row| row.is_some_and(|row| row.state != MachineState::Ended))
            && machines.offline(machine).is_none()
    }

    /// `sha256`, which `machine` keeps, brought here now as an export brings an output: into its project's `files/`. Err: why not.
    pub async fn fetch(&self, sha256: &str, machine: &str) -> Result<(), String> {
        self.api.calcifer.bring(sha256, machine).await
    }

    /// One file of `project` from `hub`, when it holds it. Err: why not.
    pub(super) async fn pull_one(&self, project: &super::Project, hub: &str, file: super::folder::File) -> Result<(), String> {
        super::remote::pull_one(&self.api, project, hub, file).await
    }

    /// Cancels run `number` on `machine` if it has not begun.
    pub fn cancel_unstarted(&self, number: u64, machine: &str) {
        let calcifer = &self.api.calcifer;
        let Ok(Some(row)) = calcifer.store().run_by_number(number) else { return };
        if row.machine.as_deref() == Some(machine) && row.started_at_ms.is_none() && row.state != crate::store::State::Settled {
            let _ = calcifer.cancel(&row.id);
        }
    }

    /// Where a run of `renderer` goes among `on`: one is where it goes; of several, the machine
    /// with the fewest runs queued or running, one without the renderer's models counting one
    /// more, a warm one first among equals, then the first listed.
    pub fn place(&self, on: &[On], renderer: &Renderer) -> On {
        if let [one] = on {
            return one.clone();
        }
        let load = self.api.calcifer.store().machine_load().unwrap_or_default();
        let models: Vec<String> = (renderer.slots.iter().chain(&renderer.calls))
            .filter_map(|slot| match ModelRef::parse(slot.default.as_deref()?) {
                Ok(ModelRef::Hub { repo, .. }) => Some(repo.to_string()),
                _ => None,
            })
            .collect();
        let machines = &self.api.machines;
        let cost = |on: &On| {
            let On::Machine(text) = on else { return (u32::MAX, true) };
            let Ok(Some(row)) = machines.here(text) else { return (u32::MAX, true) };
            let (running, queued) = load.get(&row.name).copied().unwrap_or_default();
            let held = |frame: protocol::machine::StatusFrame| {
                models.iter().all(|model| frame.models.iter().any(|held| held.repository == *model))
            };
            let cold = !machines.frame(&row.name).is_some_and(held);
            (running + queued + u32::from(cold), cold)
        };
        on.iter().min_by_key(|on| cost(on)).cloned().unwrap_or(On::Any)
    }

    /// cozy's cache folder for this user (`platform::paths::cache`), else one in its home.
    pub fn cache(&self) -> std::path::PathBuf {
        platform::paths::cache().unwrap_or_else(|| self.api.machines.hubs.home().root().join("cache"))
    }

    /// cozy's ffmpeg and ffprobe: the pinned build `cozy` fetched, else PATH's.
    pub async fn ffmpeg(&self) -> Result<(PathBuf, PathBuf), Error> {
        let home = self.api.machines.hubs.home();
        let found = crate::tools::ffmpeg(home, &|line| tracing::info!("{line}")).await?;
        Ok((found.ffmpeg, found.ffprobe))
    }
}

fn refused(error: crate::Error) -> Error {
    Refusal::from(error).0
}
