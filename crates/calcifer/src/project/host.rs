//! What a project's walk asks of the Calcifer it runs in, and nothing else: the Hub's renderers,
//! sending and following runs, the machines' load, and ffmpeg. A walk hosted elsewhere (the Hub)
//! answers these same calls.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use protocol::{
    calcifer::{On, Run, Status as RunStatus, SubmitRun},
    error::{Error, Kind},
    ids::RunId,
    interface::{self, Slot},
    model_ref::ModelRef,
};
use serde_json::Value;

use sha2::{Digest as _, Sha256};

use crate::{
    api::{Api, Refusal},
    local_code,
};

/// A renderer as the Hub has it (its package's newest release, or the one `@release` names), or
/// local code on this computer (`with: ./dir/function`, from the project's folder); and the
/// longest piece it makes when it renders shots.
#[derive(Clone)]
pub struct Renderer {
    pub package: String,
    pub function: String,
    /// For local code, its source's digest.
    pub release: String,
    /// Local code: its package directory or script, made ready on the machine each call goes to.
    pub local: Option<PathBuf>,
    /// What its request takes, read from its schema.
    pub accepts: Accepts,
    /// Its model slots and those of the calls it makes, for a request's `models:` and `lora:`.
    pub slots: Vec<Slot>,
    pub calls: Vec<Slot>,
}

/// What a function's request takes, as its schema says: a project sends only these fields, and a
/// shot's references only of these kinds, with these fields each.
#[derive(Clone, Debug, Default)]
pub struct Accepts {
    pub fields: BTreeSet<String>,
    /// `references`' kinds (its union's tags), each with its fields.
    pub kinds: BTreeMap<String, BTreeSet<String>>,
    /// A shot renderer's longest and shortest piece (its `duration_s` schema extras).
    pub max_piece_s: Option<f64>,
    pub min_piece_s: Option<f64>,
}

impl Accepts {
    pub fn read(request: &Value) -> Self {
        let fields = |doc: &Value| -> Vec<Value> { doc["fields"].as_array().cloned().unwrap_or_default() };
        let named = |doc: &Value| fields(doc).iter().filter_map(|f| f["name"].as_str().map(str::to_owned)).collect();
        let field = |name: &str| fields(request).into_iter().find(|f| f["name"] == name);
        let extra = |key: &str| field("duration_s").and_then(|f| f["extra"][key].as_f64());
        let union = field("references").map(|f| f["type"]["list"]["union"].clone()).unwrap_or_default();
        let kinds = (union.as_array().into_iter().flatten())
            .filter_map(|member| Some((member["tag"].as_str()?.to_owned(), named(member))))
            .collect();
        Self {
            fields: named(request),
            kinds,
            max_piece_s: extra("max_piece_s"),
            min_piece_s: extra("min_piece_s"),
        }
    }
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
        Ok(Self {
            package: package.into(),
            function: function.into(),
            release,
            local,
            accepts: Accepts::read(&entry["request"]),
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
    /// Read once a walk, by Hub and target.
    renderers: Mutex<HashMap<(String, String), Renderer>>,
    /// This walk's: the installs its local code asks for are keyed under it, so a failed one is
    /// asked again by the next walk, never replayed.
    walk: String,
}

/// How a renderer that is local code is read when no interface is kept for it here.
#[derive(Clone, Copy, PartialEq)]
pub enum Install {
    /// Installed on the first machine it may go to, and waited for: a walk's.
    Wait,
    /// Not installed: a read (status, a check before a walk) says it is not described yet.
    Never,
}

impl Host {
    pub fn new(api: &Arc<Api>) -> Self {
        let mut walk = [0u8; 8];
        getrandom::fill(&mut walk).expect("the OS has randomness");
        Self { api: api.clone(), renderers: Mutex::default(), walk: hex::encode(walk) }
    }

    /// What local code at `path` going `on` a machine is keyed under (`local_code::prepare`).
    pub fn key(&self, path: &Path, on: &On) -> String {
        let at = Sha256::digest(format!("{}\n{on:?}", path.display()));
        format!("project-local-{}-{}", self.walk, &hex::encode(at)[..16])
    }

    /// Local code at `path` made ready `on` a machine for a call, an install waited for.
    pub async fn ready(&self, path: &Path, on: &On, hub: &str) -> Result<local_code::Prepared, Error> {
        local_code::ready(&self.api, path, on, hub, &self.key(path, on)).await
    }

    /// `target` as `cozy run` takes it: local code (`./dir/function` from the project's `folder`,
    /// `~/…`, an absolute path), described on the first of `on`'s machines; else
    /// `[org/]package/function[@release]` as `hub` (empty: the default one) has it.
    pub async fn renderer(
        &self,
        hub: &str,
        target: &str,
        folder: &Path,
        on: &[On],
        install: Install,
    ) -> Result<Renderer, Error> {
        let home = self.api.machines.hubs.home().root().parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(found) = local_code::target(target, folder, &home) {
            let (path, function) = found?;
            let function = function.ok_or_else(|| {
                Error::new(Kind::Validation, "with_unnamed", format!("with: {target} names no function"))
                    .remedy(format!("{target}/<function>"))
            })?;
            let on = on.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
            let prepared = match install {
                Install::Wait => local_code::ready(&self.api, &path, &on, hub, &self.key(&path, &on)).await?,
                Install::Never => local_code::described(&self.api, &path, &on, hub).await?,
            };
            let Some(interface) = prepared.interface else {
                let said = format!("{target} is not described on {on:?} yet: its first render installs it");
                return Err(Error::new(Kind::Unavailable, "renderer_not_described", said));
            };
            let release = prepared.source.manifest.trim_start_matches("sha256:").chars().take(12).collect();
            return Renderer::read(&prepared.package, &function, release, &interface, Some(path));
        }
        let at = (hub.to_owned(), target.to_owned());
        if let Some(found) = self.renderers.lock().expect("renderers lock").get(&at) {
            return Ok(found.clone());
        }
        let malformed = || {
            let said = format!("{target} is not [org/]package/function[@release]");
            Error::new(Kind::Validation, "renderer_malformed", said)
        };
        let (target, pinned) = target.split_once('@').map_or((target, None), |(target, pin)| (target, Some(pin)));
        let parts: Vec<&str> = target.split('/').collect();
        let (org, name, function) = match parts[..] {
            [name, function] => (protocol::names::ROOT, name, function),
            [org, name, function] => (org, name, function),
            _ => return Err(malformed()),
        };
        let hubs = &self.api.machines.hubs;
        let client = hubs.reader(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
        let (org, name) = (org.to_ascii_lowercase(), name.to_ascii_lowercase());
        let release = match pinned {
            Some(release) => release.to_owned(),
            None => {
                let package = client.get_package_if_changed(&org, &name, None).await.map_err(hub_refusal)?;
                (package.and_then(|(card, _)| card.package.latest_release)).ok_or_else(|| {
                    Error::new(Kind::NotFound, "package_unreleased", format!("{org}/{name} has no release to run"))
                })?
            }
        };
        let card = client.get_package_release(&org, &name, &release).await.map_err(hub_refusal)?;
        let card = serde_json::to_value(&card).expect("a card serializes");
        let found = Renderer::read(&format!("{org}/{name}"), function, release, &card["package_interface"], None)?;
        self.renderers.lock().expect("renderers lock").insert(at, found.clone());
        Ok(found)
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
            if matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled) && !owed(&run) {
                return Ok(run);
            }
            tokio::select! {
                _ = tail.recv() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
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

    /// cozy's ffmpeg and ffprobe: the pinned build `cozy` fetched, else PATH's.
    pub async fn ffmpeg(&self) -> Result<(PathBuf, PathBuf), Error> {
        let home = self.api.machines.hubs.home();
        let found = crate::tools::ffmpeg(home, &|line| tracing::info!("{line}")).await?;
        Ok((found.ffmpeg, found.ffprobe))
    }
}

/// An output still owed to this computer: its last revision neither landed nor failed.
fn owed(run: &Run) -> bool {
    run.output.iter().any(|item| !item.is_checkpoint() && item.path.is_none() && item.export_error.is_none())
}

fn refused(error: crate::Error) -> Error {
    Refusal::from(error).0
}

fn hub_refusal(error: hub_client::Error) -> Error {
    error.refusal(|| "cozy login <email>".into())
}
