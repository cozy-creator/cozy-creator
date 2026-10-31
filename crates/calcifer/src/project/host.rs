//! What a project's walk asks of the Calcifer it runs in, and nothing else: the Hub's renderers,
//! sending and following runs, the machines' load, and ffmpeg. A walk hosted elsewhere (the Hub)
//! answers these same calls.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
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

use crate::api::{Api, Refusal};

/// A renderer as the Hub has it: its package's newest release, and the longest piece it makes when
/// it renders shots.
#[derive(Clone)]
pub struct Renderer {
    pub package: String,
    pub function: String,
    pub release: String,
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

pub struct Host {
    api: Arc<Api>,
    /// Read once a walk, by Hub and target.
    renderers: Mutex<HashMap<(String, String), Renderer>>,
}

impl Host {
    pub fn new(api: &Arc<Api>) -> Self {
        Self { api: api.clone(), renderers: Mutex::default() }
    }

    /// `target` (`[org/]package/function`) as `hub` (empty: the default one) has it.
    pub async fn renderer(&self, hub: &str, target: &str) -> Result<Renderer, Error> {
        let at = (hub.to_owned(), target.to_owned());
        if let Some(found) = self.renderers.lock().expect("renderers lock").get(&at) {
            return Ok(found.clone());
        }
        let malformed =
            || Error::new(Kind::Validation, "renderer_malformed", format!("{target} is not [org/]package/function"));
        let parts: Vec<&str> = target.split('/').collect();
        let (org, name, function) = match parts[..] {
            [name, function] => (protocol::names::ROOT, name, function),
            [org, name, function] => (org, name, function),
            _ => return Err(malformed()),
        };
        let hubs = &self.api.machines.hubs;
        let client = hubs.reader(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
        let (org, name) = (org.to_ascii_lowercase(), name.to_ascii_lowercase());
        let package = client.get_package_if_changed(&org, &name, None).await.map_err(hub_refusal)?;
        let release = (package.and_then(|(card, _)| card.package.latest_release)).ok_or_else(|| {
            Error::new(Kind::NotFound, "package_unreleased", format!("{org}/{name} has no release to run"))
        })?;
        let card = client.get_package_release(&org, &name, &release).await.map_err(hub_refusal)?;
        let card = serde_json::to_value(&card).expect("a card serializes");
        let entry = interface::entrypoints(&card["package_interface"])
            .find(|entry| interface::name(entry) == Some(function))
            .ok_or_else(|| {
                Error::new(
                    Kind::NotFound,
                    "callable_unknown",
                    format!("{org}/{name}@{release} has no function {function}"),
                )
            })?;
        let accepts = Accepts::read(&entry["request"]);
        let read = |slots: Vec<&Value>| slots.into_iter().filter_map(|slot| Slot::read(slot).ok()).collect();
        let slots = read(interface::models(entry).iter().collect());
        let calls = read(interface::call_slots(&card["package_interface"], entry));
        let found = Renderer {
            package: format!("{org}/{name}"),
            function: function.into(),
            release,
            accepts,
            slots,
            calls,
        };
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
    pub fn ffmpeg(&self) -> Result<(PathBuf, PathBuf), Error> {
        let tools = self.api.machines.hubs.home().tools();
        let path: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        let (found, ffprobe) = platform::tools::ffmpeg(&tools, &path).ok_or_else(|| {
            Error::new(
                Kind::Unavailable,
                "ffmpeg_unavailable",
                "the edit is cut with ffmpeg, which this computer lacks",
            )
            .next("cozy doctor")
        })?;
        Ok((found.path, ffprobe))
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
