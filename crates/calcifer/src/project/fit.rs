//! Whether this computer would render a tool poorly, said before a render on it alone (`POST /v1/projects/{id}/fit`): no GPU for a
//! function that loads a model on one, or a GPU its weights fill more than three quarters of, the Hub's rule for a card that holds them. A
//! tool's weights are what the Hub places a run of its release by; local code's, its models' releases; either with a checkpoint on its box
//! in place of its model's. A run of it here already says how long it took. Render here is kept by tool and what this computer has
//! (`state.db`'s `rendered_here`), and asked again only on another computer, or for weights half as big again.
use std::{
    collections::{BTreeMap, HashMap},
    path::Path as FsPath,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
};
use hub_client::types::RentalPlacementRequest;
use api::{
    calcifer::{On, ReleaseFrom},
    error::{Error, Kind},
    interface::{Interface, Slot},
    model_ref::ModelRef,
    calcifer::project::{Fit, FitAsk, RentFor, ToolFit},
    calcifer::tool,
};
use serde_json::{Map, Value};

use super::{ALL, Doc, Stand, find, host::Host, name_key};
use crate::{
    api::{Api, Refusal, parse},
    runs::target,
    toolbox::HUB_LATE,
};

/// What the Hub said is kept this long: a release's weights never change, the price of a card to rent does.
const KEPT: Duration = Duration::from_secs(600);
/// Render here holds for weights up to half as big again as those it was said for.
const GROWN: u64 = 2;

static PLACED: LazyLock<Mutex<HashMap<String, (Instant, Option<Placed>)>>> = LazyLock::new(Mutex::default);
static SIZED: LazyLock<Mutex<HashMap<String, (Instant, Option<u64>)>>> = LazyLock::new(Mutex::default);

/// What the Hub places a run of a release's function on, and each model it loads there, by its parameter.
#[derive(Clone)]
struct Placed {
    slots: Vec<(String, String, u64)>,
    pick: Option<RentFor>,
}

/// What one tool asks of a machine.
struct Need {
    tool: String,
    label: String,
    bytes: u64,
    pick: Option<RentFor>,
    /// Its newest finished run here, in seconds.
    took_s: Option<f64>,
}

/// Whether a Render here said for weights of `kept` bytes holds for `bytes`.
fn holds(kept: u64, bytes: u64) -> bool {
    bytes * GROWN <= kept * (GROWN + 1)
}

/// `POST /v1/projects/{id}/fit`: what a render of `names` here would do poorly.
pub(super) async fn fit(State(api): State<Arc<Api>>, Path(id): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Json<Fit>, Refusal> {
    let asked: FitAsk = parse(&body?)?;
    Ok(Json(judged(&api, &id, &asked, false).await?))
}

/// `POST /v1/projects/{id}/fit/here`: the same, each tool kept as rendered here, so nothing asks again.
pub(super) async fn here(State(api): State<Arc<Api>>, Path(id): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Json<Fit>, Refusal> {
    let asked: FitAsk = parse(&body?)?;
    Ok(Json(judged(&api, &id, &asked, true).await?))
}

async fn judged(api: &Arc<Api>, id: &str, asked: &FitAsk, keep: bool) -> Result<Fit, Refusal> {
    let project = api.projects.get(api, id)?;
    // A machine that hasn't said what it has can't be judged: nothing is said of it.
    let Some(gpus) = gpus(api) else { return Ok(Fit::default()) };
    let (targets, dir) = {
        let text = project.folder.text().map_err(Refusal)?;
        let doc = Doc::read(&project.folder, &text).map_err(Refusal)?;
        let lock = project.folder.lock().map_err(Refusal)?;
        let stand = doc.stand(&lock);
        let mut targets: BTreeMap<String, Vec<(String, String, Map<String, Value>)>> = BTreeMap::new();
        for i in rendered(&doc, &stand, asked) {
            let asset = &doc.assets[i];
            let tool = asset.tool.clone().unwrap_or_else(|| asset.target.clone());
            targets.entry(asset.target.clone()).or_default().push((tool, asset.name.clone(), asset.made.clone()));
        }
        (targets, project.folder.dir.clone())
    };
    let hub = api.machines.hubs.default_origin();
    let largest = gpus.iter().map(|(_, bytes)| *bytes).max();
    let machine = machine_of(&gpus);
    let judging = targets.into_iter().map(|(target, assets)| {
        let (api, hub, dir) = (api.clone(), hub.clone(), dir.clone());
        async move {
            let mut need: Option<Need> = None;
            for (tool, _, made) in &assets {
                let Some(one) = need_of(&api, &hub, &dir, &target, tool, made).await else { return None };
                if need.as_ref().is_none_or(|had| one.bytes > had.bytes) {
                    need = Some(one);
                }
            }
            Some((need?, assets.into_iter().map(|(_, name, _)| name).collect::<Vec<_>>()))
        }
    });
    let needs = futures_util::future::join_all(judging).await;
    let kept = api.calcifer.store().rendered_here(&machine)?;
    let mut fit = Fit::default();
    let mut picks = vec![];
    for (need, names) in needs.into_iter().flatten() {
        let Some(line) = said(&need, largest) else { continue };
        let here = kept.get(&need.tool).is_some_and(|kept| holds(*kept, need.bytes));
        picks.push((here, need.pick.clone()));
        fit.tools.push(ToolFit { tool: need.tool, label: need.label, names, line, bytes: need.bytes, here });
    }
    // The GPU for what's asked about, else for all of them: the biggest of the Hub's picks.
    let asked_of = picks.iter().any(|(here, _)| !here);
    let picks: Vec<RentFor> = picks.into_iter().filter(|(here, _)| !asked_of || !here).filter_map(|(_, pick)| pick).collect();
    let them = if fit.tools.iter().filter(|tool| !asked_of || !tool.here).count() == 1 { "it" } else { "them" };
    fit.rent = picks.into_iter().reduce(|best, pick| if pick.vram_gb > best.vram_gb { pick } else { best }).map(|pick| RentFor {
        line: format!("A rented {} runs {them} for ${:.2}/h.", pick.card, pick.usd_micros_per_hour as f64 / 1e6),
        ..pick
    });
    if keep && !fit.tools.is_empty() {
        let tools: Vec<(String, u64)> = fit.tools.iter().map(|tool| (tool.tool.clone(), tool.bytes)).collect();
        let kept = api.calcifer.store().keep_rendered_here(&machine, &tools);
        kept.map_err(|why| Refusal(Error::new(Kind::Unavailable, "here_io", why.to_string())))?;
        fit.tools.iter_mut().for_each(|tool| tool.here = true);
    }
    Ok(fit)
}

/// What a render of `asked` makes, as the render reads it: `all` is everything with an operation not approved, and with `outdated` what's
/// approved and out of date. Nothing Calcifer makes itself needs a machine.
fn rendered(doc: &Doc, stand: &[super::Standing], asked: &FitAsk) -> Vec<usize> {
    let all = matches!(asked.names.as_slice(), [only] if name_key(only) == name_key(ALL));
    let named: Vec<usize> = match all {
        true => (doc.order.iter().copied())
            .filter(|&i| match &stand[i].state {
                _ if doc.assets[i].pin.is_none() => true,
                Stand::Pinned { version: Some(_), changed } => asked.outdated && !changed.is_empty(),
                _ => false,
            })
            .collect(),
        false => asked.names.iter().filter_map(|name| find(doc, name).ok()).collect(),
    };
    named.into_iter().filter(|&i| doc.makes(i) && doc.tool(i).is_none()).collect()
}

/// This computer's GPUs as its machine last said: each one's name and the memory it may use. None: it hasn't said.
fn gpus(api: &Api) -> Option<Vec<(String, u64)>> {
    let frame = api.machines.frame("local")?;
    let usable = |gpu: &api::machine::Gpu| match gpu.memory_limit_bytes {
        0 => gpu.memory_bytes,
        limit => limit.min(gpu.memory_bytes),
    };
    Some(frame.gpus.iter().map(|gpu| (gpu.name.clone(), usable(gpu))).collect())
}

/// What a Render here holds for: another computer, or a GPU more or less, is another question.
fn machine_of(gpus: &[(String, u64)]) -> String {
    match gpus {
        [] => "no GPU".into(),
        gpus => gpus.iter().map(|(name, bytes)| format!("{name} {} MiB", bytes >> 20)).collect::<Vec<_>>().join(", "),
    }
}

/// How it would go here, or None when well enough.
fn said(need: &Need, largest: Option<u64>) -> Option<String> {
    let mut line = match largest {
        None => format!("This computer has no GPU, so {} would run on its CPU, slowly.", need.label),
        Some(largest) if need.bytes > 0 && need.bytes * 4 > largest * 3 => {
            format!("This computer's GPU has {}, and {} needs about {}, so it would run slowly here.", gb(largest), need.label, gb(need.bytes * 4 / 3))
        }
        Some(_) => return None,
    };
    if let Some(seconds) = need.took_s {
        line.push_str(&format!(" It took {} here last time.", about(seconds)));
    }
    Some(line)
}

/// As a card's memory is sold: a GB is 2^30 bytes.
fn gb(bytes: u64) -> String {
    format!("{} GB", (bytes as f64 / (1u64 << 30) as f64).round().max(1.0))
}

fn about(seconds: f64) -> String {
    match seconds {
        s if s < 90.0 => format!("about {} s", s.round().max(1.0)),
        s if s < 90.0 * 60.0 => format!("about {} min", (s / 60.0).round()),
        s => format!("about {:.1} h", s / 3600.0),
    }
}

/// What `target`, a box's tool as its file writes it, asks of a machine, with what's on the box in place of its models. None: nothing
/// says it loads a model on a GPU, or it can't be read.
async fn need_of(api: &Arc<Api>, hub: &str, dir: &FsPath, target: &str, tool: &str, made: &Map<String, Value>) -> Option<Need> {
    let local = On::Machine("local".into());
    // Local code is read here, however long its first reading takes; a release, from a Hub that may be late.
    let described = target::describe(api, hub, target, None, Some(dir), &local);
    let found = match crate::local_code::target(target, Some(dir), &api.machines.user_home) {
        Some(_) => described.await.ok()?,
        None => tokio::time::timeout(HUB_LATE, described).await.ok()?.ok()?,
    };
    let function = found.function.clone()?;
    let local_code = found.release_from == ReleaseFrom::Local;
    let weights = Weights::of(&found.interface, &function, made)?;
    if !local_code {
        placed(api, hub, &found.package, &found.release, &function).await;
    }
    for (written, lane) in weights.refs(placed_now(hub, &found.package, &found.release, &function).as_ref()) {
        sized(api, hub, &written, lane.as_deref()).await;
    }
    let (bytes, pick) = weights.weighed(hub, &found.package, &found.release, &function, local_code);
    let interface = Interface::parse(&found.interface).ok()?;
    let label = tool::tools_of(&found.package, &found.release, &interface).into_iter().find(|tool| tool.function == function).map_or_else(|| function.clone(), |tool| tool.package_title);
    let measured = match local_code {
        true => format!("{}/{function}", found.package),
        false => format!("{}/{function}@", found.package),
    };
    let took_s = Host::new(api).last_measured(&measured, "local").first().map(|measured| measured.busy_s);
    Some(Need { tool: tool.to_owned(), label, bytes, pick, took_s })
}

/// Whether this computer would run `function` of `package` poorly, by what's known here without asking anyone: what the Hub and its
/// model cards said lately (a render on it alone asks `fit` first), and what this computer's machine said it has.
pub(super) fn poorly_now(api: &Api, hub: &str, package: &super::Package, function: &str, made: &Map<String, Value>) -> bool {
    let Some(gpus) = gpus(api) else { return false };
    let Some(weights) = Weights::of(package.document(), function, made) else { return false };
    let (bytes, _) = weights.weighed(hub, &package.name, &package.release, function, package.local.is_some());
    let largest = gpus.iter().map(|(_, bytes)| *bytes).max();
    largest.is_none_or(|largest| bytes > 0 && bytes * 4 > largest * 3)
}

/// The model slots a function loads on a GPU, its own or, for one that orchestrates, those of what it calls; and what's on its box in their
/// place.
struct Weights {
    slots: Vec<Slot>,
    /// A checkpoint on the box, by the parameter it takes the place of.
    on_box: BTreeMap<String, String>,
}

impl Weights {
    /// None: nothing says it loads a model on a GPU.
    fn of(document: &Value, function: &str, made: &Map<String, Value>) -> Option<Self> {
        let interface = Interface::parse(document).ok()?;
        let callable = interface.callables.iter().find(|callable| callable.name == function)?;
        let slots: Vec<Slot> = match callable.models.is_empty() {
            true => interface.call_slots(callable).into_iter().cloned().collect(),
            false => callable.models.clone(),
        };
        if !interface.gpu(callable) || slots.is_empty() {
            return None;
        }
        let on_box = (made.get("models").and_then(Value::as_object).into_iter().flatten())
            .filter_map(|(param, slot)| Some((param.clone(), slot["ref"].as_str()?.to_owned())))
            .collect();
        Some(Self { slots, on_box })
    }

    fn param(slot: &Slot) -> &str {
        slot.path.rsplit(".models.").next().unwrap_or_default()
    }

    /// Each checkpoint whose bytes it needs, and the lane it'd load: what's on the box, and with no placement its slots' own defaults.
    fn refs(&self, placed: Option<&Placed>) -> Vec<(String, Option<String>)> {
        let lane = |param: &str| placed.and_then(|placed| placed.slots.iter().find(|(held, ..)| held == param)).map(|(_, lane, _)| lane.clone());
        (self.slots.iter())
            .filter_map(|slot| {
                let param = Self::param(slot);
                let written = self.on_box.get(param).cloned().or_else(|| placed.is_none().then(|| slot.default.clone()).flatten())?;
                Some((written, lane(param).or_else(|| slot.lane.clone())))
            })
            .collect()
    }

    /// Its weights' bytes as what's kept here says (0: unknown), and the GPU the Hub would rent for it. A release is what the Hub places it
    /// by; local code, its models' releases, an orchestrator as big as the biggest function it calls, as the Hub places one.
    fn weighed(&self, hub: &str, package: &str, release: &str, function: &str, local_code: bool) -> (u64, Option<RentFor>) {
        let on_box = |param: &str, lane: Option<&str>| self.on_box.get(param).and_then(|written| sized_now(hub, written, lane));
        if !local_code {
            let Some(placed) = placed_now(hub, package, release, function) else { return (0, None) };
            let bytes = placed.slots.iter().map(|(param, lane, bytes)| on_box(param, Some(lane)).unwrap_or(*bytes)).sum();
            return (bytes, placed.pick);
        }
        let mut groups: BTreeMap<&str, u64> = BTreeMap::new();
        for slot in &self.slots {
            let param = Self::param(slot);
            let written = self.on_box.get(param).or(slot.default.as_ref());
            let bytes = written.and_then(|written| sized_now(hub, written, slot.lane.as_deref())).unwrap_or(0);
            *groups.entry(slot.path.split(".models.").next().unwrap_or_default()).or_default() += bytes;
        }
        (groups.into_values().max().unwrap_or(0), None)
    }
}

/// What the Hub last said of where it places a run of `package@release`'s `function`, however long ago.
fn placed_now(hub: &str, package: &str, release: &str, function: &str) -> Option<Placed> {
    PLACED.lock().expect("placed lock").get(&format!("{hub}\n{package}@{release}/{function}")).and_then(|(_, held)| held.clone())
}

/// What a model card last said of a checkpoint's bytes, however long ago.
fn sized_now(hub: &str, written: &str, lane: Option<&str>) -> Option<u64> {
    SIZED.lock().expect("sized lock").get(&format!("{hub}\n{written}\n{}", lane.unwrap_or_default())).and_then(|(_, held)| *held)
}

/// What the Hub places a run of `package@release`'s `function` on, kept a while. None: it didn't say.
async fn placed(api: &Api, hub: &str, package: &str, release: &str, function: &str) -> Option<Placed> {
    let key = format!("{hub}\n{package}@{release}/{function}");
    if let Some((_, held)) = PLACED.lock().expect("placed lock").get(&key).filter(|(at, _)| at.elapsed() < KEPT) {
        return held.clone();
    }
    let request = RentalPlacementRequest { package: package.into(), release: Some(release.into()), function: function.into(), lane: None, gpus: None, sku: None };
    let reader = api.machines.hubs.reader(hub).ok()?;
    let answer = tokio::time::timeout(HUB_LATE, reader.place_rental(&request)).await.ok()?;
    let placed = answer.ok().map(|placement| Placed {
        slots: (placement.weights.iter())
            .map(|slot| (slot.slot.rsplit(".models.").next().unwrap_or_default().to_owned(), slot.lane.clone(), slot.bytes.max(0) as u64))
            .collect(),
        pick: placement.machines.first().map(|machine| RentFor {
            sku: machine.name.clone(),
            gpus: machine.accelerator_count.max(1) as u32,
            card: machine.accelerator_model.trim_start_matches("NVIDIA ").trim_start_matches("GeForce ").to_owned(),
            vram_gb: machine.vram_gb,
            usd_micros_per_hour: machine.price_usd_micros_per_hour,
            line: String::new(),
        }),
    });
    PLACED.lock().expect("placed lock").insert(key, (Instant::now(), placed.clone()));
    placed
}

/// A checkpoint's bytes in `lane`, else its least: a Hub model's release or commit, by its card, kept a while. None: the Hub doesn't say.
async fn sized(api: &Api, hub: &str, written: &str, lane: Option<&str>) -> Option<u64> {
    let key = format!("{hub}\n{written}\n{}", lane.unwrap_or_default());
    if let Some((_, held)) = SIZED.lock().expect("sized lock").get(&key).filter(|(at, _)| at.elapsed() < KEPT) {
        return *held;
    }
    let (repo, release, commit) = match ModelRef::parse(written).ok()? {
        ModelRef::Hub { repo, release } => (repo, release, None),
        ModelRef::Commit { repo, commit } => (repo, None, Some(commit)),
        _ => return None,
    };
    let reader = api.machines.hubs.reader(hub).ok()?;
    let card = tokio::time::timeout(HUB_LATE, reader.get_model(&repo.org, &repo.name)).await.ok()?.ok()?;
    let bytes = match (&commit, &release) {
        (Some(commit), _) => (card.releases.iter().flat_map(|release| release.lanes.values()))
            .find(|held| &held.commit == commit)
            .map(|held| held.bytes)
            .or_else(|| card.commits.iter().find(|held| &held.id == commit).map(|held| held.bytes)),
        (None, release) => {
            let release = match release {
                Some(version) => card.releases.iter().find(|held| &held.version == version),
                None => card.releases.first(),
            };
            release.and_then(|release| lane.and_then(|lane| release.lanes.get(lane)).or_else(|| release.lanes.values().filter(|held| held.bytes > 0).min_by_key(|held| held.bytes)).map(|held| held.bytes))
        }
    };
    let bytes = bytes.filter(|bytes| *bytes > 0).map(|bytes| bytes as u64);
    SIZED.lock().expect("sized lock").insert(key, (Instant::now(), bytes));
    bytes
}
