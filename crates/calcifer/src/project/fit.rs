//! Whether this computer would render a tool poorly, said before a render on it alone (`POST /v1/projects/{id}/fit`): no GPU for a
//! function that loads a model on one, a GPU its weights fill more than three quarters of (the Hub's rule for a card that holds them),
//! a GPU that also draws the screen, which a render makes lag (cl-796), or its runs here far slower than on a rented card (cl-897). A
//! tool's weights are what the Hub places a run of its release by; local code's, its models' releases; either with a checkpoint on its box
//! in place of its model's. Its runs here and on rented cards say how long it takes each. Nothing is asked: a render here says it as it
//! goes (cl-1066, Paul: render here; "Slow here" offers a rental).
//! Each box named also gets its typical inference time where a render of it would go now (cl-1004, Paul 11-15: "before the user pushes
//! 'render' we should display to them some sort of estimated time"), by the estimator placement uses, and the tool's recommended GPU
//! when renting it would be far faster.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path as FsPath,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
};
use ::api::hub::types::RentalPlacementRequest;
use api::{
    calcifer::On,
    estimate::References,
    interface::{Interface, Slot},
    machine::{Basis, JobSize},
    model_ref::ModelRef,
    calcifer::project::{BoxEstimate, Fit, FitAsk, Faster, RentFor, ToolFit},
    calcifer::tool,
};
use serde_json::{Map, Value};

use super::{ALL, Doc, Stand, find, host::Host, name_key};
use crate::{
    api::{Api, Refusal, parse},
    placement,
    runs::target,
    store::now_ms,
    toolbox::HUB_LATE,
};

/// What the Hub said is kept this long: a release's weights never change, the price of a card to rent does.
const KEPT: Duration = Duration::from_secs(600);
/// Slow here: more than this many times a rented card's time, and more than `LONGER_S` longer.
const SLOWER: f64 = 3.0;
const LONGER_S: f64 = 60.0;
/// A box's estimate waits this long for its package and its models' cards, from a Hub that may be late: else it has none.
const ESTIMATED: Duration = Duration::from_secs(20);

static PLACED: LazyLock<Mutex<HashMap<String, (Instant, Option<Placed>)>>> = LazyLock::new(Mutex::default);
static SIZED: LazyLock<Mutex<HashMap<String, (Instant, Option<Lane>)>>> = LazyLock::new(Mutex::default);

/// A checkpoint as a Hub card lists it in one lane.
#[derive(Clone, Debug)]
struct Lane {
    /// `sha256:<hex>` of its tree.
    checkpoint: String,
    /// Its whole closure.
    bytes: u64,
    /// Its header, what its components' chunks are read from (cl-1071).
    header: Option<::api::hub::types::ObjectRef>,
}

/// What the Hub places a run of a release's function on, and each model it loads there, by its parameter.
#[derive(Clone)]
struct Placed {
    slots: Vec<(String, String, u64)>,
    pick: Option<RentFor>,
    /// Each machine of one GPU the Hub rents that holds it, its card as `card` says it, with its price: what's recommended instead is
    /// one of these (`recommended`).
    offers: Vec<RentFor>,
    /// Each card that holds it on one GPU, by its name as `RentFor::card` says it, and the Hub's speed for it there.
    speeds: BTreeMap<String, f64>,
}

/// What one tool asks of a machine.
struct Need {
    tool: String,
    label: String,
    bytes: u64,
    pick: Option<RentFor>,
    /// What it takes here, in seconds, by its runs here.
    took_s: Option<f64>,
    /// What it takes on a rented card, in seconds, and the card.
    away: Option<(String, f64)>,
}


/// `POST /v1/projects/{id}/fit`: what a render of `names` here would do poorly.
pub(super) async fn fit(State(api): State<Arc<Api>>, Path(id): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Json<Fit>, Refusal> {
    let asked: FitAsk = parse(&body?)?;
    Ok(Json(judged(&api, &id, &asked).await?))
}

async fn judged(api: &Arc<Api>, id: &str, asked: &FitAsk) -> Result<Fit, Refusal> {
    // Its runs, which its times here stand on, are its project's own (xs-160): read in its storage.
    let folder = api.projects.get(api, id)?.folder.dir.clone();
    let scope = crate::private_io::of_path(&folder).map_err(Refusal)?;
    crate::private_io::context(scope, judged_in(api, id, asked)).await
}

async fn judged_in(api: &Arc<Api>, id: &str, asked: &FitAsk) -> Result<Fit, Refusal> {
    let project = api.projects.get(api, id)?;
    let all = matches!(asked.names.as_slice(), [only] if name_key(only) == name_key(ALL));
    let (targets, boxes, dir) = {
        let doc = Doc::load(&project.folder).map_err(Refusal)?;
        let lock = project.folder.lock().map_err(Refusal)?;
        let stand = doc.stand(&lock);
        let mut targets: BTreeMap<String, Vec<(String, String, Map<String, Value>)>> = BTreeMap::new();
        let mut boxes = vec![];
        for i in rendered(&doc, &stand, asked) {
            let asset = &doc.assets[i];
            let tool = asset.tool.clone().unwrap_or_else(|| asset.target.clone());
            targets.entry(asset.target.clone()).or_default().push((tool, asset.name.clone(), asset.made.clone()));
            if !all {
                boxes.push(Boxed { name: asset.name.clone(), with: doc.target(i), made: asset.made.clone(), media: doc.media_read(i) });
            }
        }
        (targets, boxes, project.folder.dir.clone())
    };
    let hub = api.machines.hubs.default_origin();
    let estimating = estimates(api, &project.id, &dir, &hub, boxes);
    // A machine that hasn't said what it has can't be judged: nothing is said of it.
    let Some(gpus) = gpus(api) else { return Ok(Fit { estimates: estimating.await, ..Fit::default() }) };
    let largest = gpus.iter().map(|gpu| gpu.1).max();
    let display = gpus.iter().any(|gpu| gpu.2);
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
    let (needs, estimates) = futures_util::future::join(futures_util::future::join_all(judging), estimating).await;
    let mut fit = Fit { estimates, ..Fit::default() };
    let mut picks = vec![];
    for (need, names) in needs.into_iter().flatten() {
        let Some(line) = said(&need, largest, display) else { continue };
        picks.extend(need.pick.clone());
        fit.tools.push(ToolFit { tool: need.tool, label: need.label, names, line, bytes: need.bytes });
    }
    // The GPU for all of them: the biggest of the Hub's picks.
    let them = if fit.tools.len() == 1 { "it" } else { "them" };
    fit.rent = picks.into_iter().reduce(|best, pick| if pick.vram_gb > best.vram_gb { pick } else { best }).map(|pick| RentFor {
        line: format!("A rented {} runs {them} for ${:.2}/h.", pick.card, pick.usd_micros_per_hour as f64 / 1e6),
        ..pick
    });
    Ok(fit)
}

/// A box asked about: its name, tool, fields and how many pictures and videos they read.
struct Boxed {
    name: String,
    with: String,
    made: Map<String, Value>,
    media: u32,
}

/// Each of `boxes`, where a render of it would go now and how long it runs there (cl-1004): its machines as a render naming none has
/// them (`runs::unnamed_set`), each quoted by what it last said, nothing started, updated or rented, and the soonest finish chosen as
/// placement chooses it (`placement::soonest`).
async fn estimates(api: &Arc<Api>, project: &str, dir: &FsPath, hub: &str, boxes: Vec<Boxed>) -> Vec<BoxEstimate> {
    if boxes.is_empty() {
        return vec![];
    }
    let Ok(on) = crate::runs::unnamed_set(api, Some(project)).await else { return vec![] };
    // Each boxed: joined inline, every box's estimate sat in the poll's frames (a release `/fit` measured up to 1.8 of its 2 MiB).
    let each = boxes.into_iter().map(|asked| Box::pin(estimate(api, dir, hub, asked, on.clone())));
    futures_util::future::join_all(each).await.into_iter().flatten().collect()
}

async fn estimate(api: &Arc<Api>, dir: &FsPath, hub: &str, asked: Boxed, on: Vec<On>) -> Option<BoxEstimate> {
    let Boxed { name, with, made, media } = asked;
    let host = Host::new(api);
    let read = super::job_of(&host, dir, name.clone(), on, hub, &with, made, super::PackageRead::Foreseen);
    let mut job = tokio::time::timeout(ESTIMATED, read).await.ok()?.ok()??;
    job.size.refs = Some(media);
    let (offers, pictured) = placement::offers_now(api, std::slice::from_ref(&job)).await;
    let chosen = placement::soonest(offers.first()?, now_ms())?.clone();
    let gpu = (pictured.iter().flatten())
        .find(|picture| picture.on == chosen.machine)
        .and_then(|picture| picture.frame.gpus.first())
        .map(|gpu| gpu.name.clone())
        .unwrap_or_default();
    let run_ms = match chosen.basis {
        Basis::None => 0,
        _ => (chosen.finish_at_ms - chosen.start_at_ms).max(0) as u64,
    };
    let faster = faster(api, hub, &job, &chosen.machine, &gpu, run_ms).await;
    Some(BoxEstimate { name, machine: chosen.machine.to_string(), card: card(&gpu), run_ms, basis: chosen.basis, faster })
}

/// The card to rent instead when `machine`, whose `gpu` (empty: none) takes `run_ms` (0: unknown), runs `job` far slower than a rented
/// one would (`recommended`), or has no GPU for a job that loads a model on one: its time there, both times and its cost a render in a
/// line, and the rent. None: local code, which the Hub places on nothing, or no measured card qualifies.
async fn faster(api: &Arc<Api>, hub: &str, job: &placement::Job, machine: &On, gpu: &str, run_ms: u64) -> Option<Faster> {
    // A packages checkout's first-party package from source (`_/pkg/fn`) is placed as its newest release, and timed by its source's
    // benchmarks; a project's own code (`local/pkg/fn`), which the Hub doesn't have, by nothing.
    let (path, release) = job.target.split_once('@').unwrap_or((&job.target, ""));
    if release.is_empty() && (job.source.is_none() || path.starts_with("local/")) {
        return None;
    }
    let (package, function) = path.rsplit_once('/')?;
    let cpu = gpu.is_empty() && job.gpu;
    if !cpu && run_ms == 0 {
        return None;
    }
    let placed = placed(api, hub, package, release, function).await?;
    let here = run_ms as f64;
    let references = crate::placement::references_for(api, [job], vec![]);
    let (rent, away) = recommended(&references, &job.function, &job.size, &placed.offers, (!cpu).then_some(here))?;
    let there = format!("a rented {}, {} and {} a render", rent.card, about(away / 1e3), per_render(rent.usd_micros_per_hour, away));
    let line = match (cpu, machine.is_local()) {
        (true, true) => format!("This computer has no GPU, so this would run on its CPU, slowly. On {there}."),
        (true, false) => format!("{machine} has no GPU, so this would run on its CPU, slowly. On {there}."),
        (false, true) => format!("On this computer's {} this takes {}; on {there}.", card(gpu), about(here / 1e3)),
        (false, false) => format!("On {machine}'s {} this takes {}; on {there}.", card(gpu), about(here / 1e3)),
    };
    let rent = RentFor { line: format!("A rented {} runs it for ${:.2}/h.", rent.card, rent.usd_micros_per_hour as f64 / 1e6), ..rent };
    Some(Faster { run_ms: away as u64, line, rent })
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

/// This computer's GPUs as its machine last said: each one's name, the memory it may use, and whether it draws a screen. None: it hasn't
/// said.
fn gpus(api: &Api) -> Option<Vec<(String, u64, bool)>> {
    let frame = api.machines.frame("local")?;
    let usable = |gpu: &api::machine::Gpu| match gpu.memory_limit_bytes {
        0 => gpu.memory_bytes,
        limit => limit.min(gpu.memory_bytes),
    };
    Some(frame.gpus.iter().map(|gpu| (gpu.name.clone(), usable(gpu), gpu.display)).collect())
}

/// Whether a GPU of this computer draws its screen now: a render here makes it lag (cl-796).
pub(super) fn draws_screen(api: &Api) -> bool {
    gpus(api).is_some_and(|gpus| gpus.iter().any(|gpu| gpu.2))
}

/// How it would go here, or None when well enough: why first, then how long here against a rented card, then the screen.
fn said(need: &Need, largest: Option<u64>, display: bool) -> Option<String> {
    let slow = need.took_s.zip(need.away.as_ref()).is_some_and(|(here, (_, away))| here > SLOWER * away && here - away > LONGER_S);
    let label = &need.label;
    let mut line = match largest {
        None => format!("This computer has no GPU, so {label} would run on its CPU, slowly."),
        Some(largest) if need.bytes > 0 && need.bytes * 4 > largest * 3 => {
            format!("This computer's GPU has {}, and {label} needs about {}, so it would run slowly here.", gb(largest), gb(need.bytes * 4 / 3))
        }
        Some(_) if slow => format!("{label} runs slowly on this computer's GPU."),
        Some(_) if display => format!("This computer's GPU also draws your screen, so {label} will slow your screen while it runs."),
        Some(_) => return None,
    };
    match (need.took_s, &need.away) {
        (Some(here), Some((card, away))) if here > *away => line.push_str(&format!(" It takes {} here vs {} on a rented {card}.", about(here), about(*away))),
        (Some(here), _) => line.push_str(&format!(" It took {} here last time.", about(here))),
        _ => {}
    }
    if display && largest.is_some() && !line.contains("your screen") {
        line.push_str(" It will slow your screen while it runs.");
    }
    Some(line)
}

/// A card as a person names it, from its driver's name: `RTX 4090`.
pub(crate) fn card(model: &str) -> String {
    model.trim_start_matches("NVIDIA ").trim_start_matches("GeForce ").to_owned()
}

/// What a rented card takes for `target` (`org/pkg/fn@release`, local code `pkg/fn`) like `made`, in seconds, and the card: the one
/// `recommended` against `took_s` here (cl-1024); with no placement (local code), the fastest card it ran on away from here. None:
/// nothing qualifies.
fn away(api: &Api, target: &str, made: &Map<String, Value>, placed: Option<&Placed>, took_s: Option<f64>) -> Option<(String, f64)> {
    let Some(placed) = placed else {
        let base = target.split('@').next().unwrap_or(target);
        let ran = api.calcifer.away_ms(base, made).into_iter().map(|(gpu, ms)| (card(&gpu), ms / 1e3));
        return ran.min_by(|a, b| a.1.total_cmp(&b.1));
    };
    let references = crate::placement::references(&api.machines.hubs, &[target.to_owned()]);
    let (rent, ms) = recommended(&references, target, &size_of(made), &placed.offers, took_s.map(|s| s * 1e3))?;
    Some((rent.card, ms / 1e3))
}

/// What this computer's GPU typically takes for `target` like `made`, in seconds, where it hasn't run it (cl-1004). None: no GPU, or
/// nothing says.
fn typical_here(api: &Api, target: &str, made: &Map<String, Value>) -> Option<f64> {
    let gpu = gpus(api)?.into_iter().next()?.0;
    api.calcifer.typical_ms(target, &gpu, &size_of(made)).map(|(ms, _)| ms / 1e3)
}

fn size_of(made: &Map<String, Value>) -> JobSize {
    JobSize::of(&Value::Object(made.clone()))
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
    let local_code = found.release_from.local();
    let weights = Weights::of(&found.interface, &function, made)?;
    if !local_code {
        placed(api, hub, &found.package, &found.release, &function).await;
    }
    for (written, lane) in weights.refs(placed_now(hub, &found.package, &found.release, &function).as_ref()) {
        sized(api, hub, &written, lane.as_deref()).await;
    }
    let (bytes, pick) = weights.weighed(hub, &found.package, &found.release, &function, local_code);
    let interface = Interface::parse(&found.interface).ok()?;
    let label = tool::tools_of(&found.package, &found.release, &interface, &found.summary).into_iter().find(|tool| tool.function == function).map_or_else(|| function.clone(), |tool| tool.package_title);
    let measured = match local_code {
        true => format!("{}/{function}", found.package),
        false => format!("{}/{function}@", found.package),
    };
    let base = measured.trim_end_matches('@');
    let full = match local_code {
        true => base.to_owned(),
        false => format!("{base}@{}", found.release),
    };
    let took_s = (api.calcifer.here_ms(base, made).map(|ms| ms / 1e3))
        .or_else(|| Host::new(api).last_took(&measured, "local"))
        .or_else(|| typical_here(api, &full, made));
    let placed = (!local_code).then(|| placed_now(hub, &found.package, &found.release, &function)).flatten();
    let away = away(api, &full, made, placed.as_ref(), took_s);
    Some(Need { tool: tool.to_owned(), label, bytes, pick, took_s, away })
}

/// Whether this computer would run `function` of `package` poorly, by what's known here without asking anyone: what the Hub and its
/// model cards said lately (a render on it alone asks `fit` first), and what this computer's machine said it has.
pub(super) fn poorly_now(api: &Api, hub: &str, package: &super::Package, function: &str, made: &Map<String, Value>) -> bool {
    let Some(gpus) = gpus(api) else { return false };
    let Some(weights) = Weights::of(package.document(), function, made) else { return false };
    let (bytes, _) = weights.weighed(hub, &package.name, &package.release, function, package.local.is_some());
    let largest = gpus.iter().map(|gpu| gpu.1).max();
    largest.is_none_or(|largest| bytes > 0 && bytes * 4 > largest * 3)
}

/// What a walk's job sent here alone says first, as `here_now`.
pub(super) fn told_here(api: &Api, hub: &str, package: &super::Package, function: &str, made: &Map<String, Value>) -> Option<String> {
    let interface = Interface::parse(package.document()).ok()?;
    let release = package.local.is_none().then_some(package.release.as_str());
    here_now(api, hub, &package.name, release, function, &interface, &package.summary, made)
}

/// What a run of `function` of `package@release` here says first when this computer would run it poorly, by what's known here without
/// asking anyone, as `fit` says it: for `cozy run` and a walk's job sent here, which ask no one (cl-897). None on a computer with no GPU,
/// which `cozy run` says itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn here_now(api: &Api, hub: &str, package: &str, release: Option<&str>, function: &str, interface: &Interface, summary: &str, made: &Map<String, Value>) -> Option<String> {
    let gpus = gpus(api).filter(|gpus| !gpus.is_empty())?;
    let weights = Weights::of_interface(interface, function, made)?;
    let local_code = release.is_none();
    let release = release.unwrap_or_default();
    let (bytes, _) = weights.weighed(hub, package, release, function, local_code);
    let base = format!("{package}/{function}");
    let full = match local_code {
        true => base.clone(),
        false => format!("{base}@{release}"),
    };
    let placed = (!local_code).then(|| placed_now(hub, package, release, function)).flatten();
    let label = tool::tools_of(package, release, interface, summary).into_iter().find(|tool| tool.function == function).map_or_else(|| base.clone(), |tool| tool.package_title);
    let took_s = api.calcifer.here_ms(&base, made).map(|ms| ms / 1e3).or_else(|| typical_here(api, &full, made));
    let need = Need {
        tool: base.clone(),
        label,
        bytes,
        pick: placed.as_ref().and_then(|placed| placed.pick.clone()),
        took_s,
        away: away(api, &full, made, placed.as_ref(), took_s),
    };
    said(&need, gpus.iter().map(|gpu| gpu.1).max(), gpus.iter().any(|gpu| gpu.2))
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
        Self::of_interface(&Interface::parse(document).ok()?, function, made)
    }

    fn of_interface(interface: &Interface, function: &str, made: &Map<String, Value>) -> Option<Self> {
        let callable = interface.callables.iter().find(|callable| callable.name == function)?;
        if !interface.gpu(callable) {
            return None;
        }
        Self::read(interface, function, made)
    }

    /// The weights `function` reads, whatever runs it: a CPU function's raw `Weights` (Runtime 0.47: raw weights select no GPU) download
    /// before it runs all the same, so a machine without them is quoted that download (cl-1047). None: it reads none.
    fn read(interface: &Interface, function: &str, made: &Map<String, Value>) -> Option<Self> {
        let callable = interface.callables.iter().find(|callable| callable.name == function)?;
        let slots: Vec<Slot> = match callable.models.is_empty() {
            true => interface.call_slots(callable).into_iter().cloned().collect(),
            false => callable.models.clone(),
        };
        if slots.is_empty() {
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
    SIZED.lock().expect("sized lock").get(&format!("{hub}\n{written}\n{}", lane.unwrap_or_default())).and_then(|(_, held)| held.as_ref().map(|lane| lane.bytes))
}

/// The Hub's last word on each card's speed for `target` (`org/pkg/fn@release`), however long ago: what `estimate::References` scales
/// another card's time by (cl-1004). None: it hasn't said, or `target` is local code.
pub(crate) fn speeds_now(hub: &str, target: &str) -> Option<BTreeMap<String, f64>> {
    let (path, release) = target.split_once('@')?;
    let (package, function) = path.rsplit_once('/')?;
    placed_now(hub, package, release, function).map(|placed| placed.speeds).filter(|speeds| !speeds.is_empty())
}

/// The Hub's per-SKU speed of each card that holds a run of `package@release`'s `function` on one GPU, by its card's name as `card`
/// says it: another card's time scaled by them is this one's (cl-897, cl-913). Empty: the Hub didn't say.
pub(crate) async fn speeds(hubs: &crate::rentals::Hubs, hub: &str, package: &str, release: &str, function: &str) -> BTreeMap<String, f64> {
    placed_by(hubs, hub, package, release, function).await.map(|placed| placed.speeds).unwrap_or_default()
}

/// What the Hub places a run of `package@release`'s `function` on, kept a while. None: it didn't say.
async fn placed(api: &Api, hub: &str, package: &str, release: &str, function: &str) -> Option<Placed> {
    placed_by(&api.machines.hubs, hub, package, release, function).await
}

async fn placed_by(hubs: &crate::rentals::Hubs, hub: &str, package: &str, release: &str, function: &str) -> Option<Placed> {
    let key = format!("{hub}\n{package}@{release}/{function}");
    if let Some((_, held)) = PLACED.lock().expect("placed lock").get(&key).filter(|(at, _)| at.elapsed() < KEPT) {
        return held.clone();
    }
    let release = (!release.is_empty()).then(|| release.into());
    let request = RentalPlacementRequest { package: package.into(), release, function: function.into(), lane: None, gpus: None, sku: None };
    let reader = hubs.reader(hub).ok()?;
    let answer = tokio::time::timeout(HUB_LATE, reader.place_rental(&request)).await.ok()?;
    let placed = answer.ok().map(|placement| Placed {
        speeds: (placement.machines.iter())
            .filter(|machine| machine.accelerator_count <= 1)
            .filter_map(|machine| Some((card(&machine.accelerator_model), machine.speed?)))
            .fold(BTreeMap::new(), |mut speeds, (card, speed)| {
                speeds.entry(card).or_insert(speed);
                speeds
            }),
        slots: (placement.weights.iter())
            .map(|slot| (slot.slot.rsplit(".models.").next().unwrap_or_default().to_owned(), slot.lane.clone(), slot.bytes.max(0) as u64))
            .collect(),
        pick: placement.machines.first().map(rent_for),
        offers: placement.machines.iter().filter(|machine| machine.accelerator_count <= 1).map(rent_for).collect(),
    });
    PLACED.lock().expect("placed lock").insert(key, (Instant::now(), placed.clone()));
    placed
}

fn rent_for(machine: &::api::hub::types::RentalPlacementMachine) -> RentFor {
    RentFor {
        sku: machine.name.clone(),
        gpus: machine.accelerator_count.max(1) as u32,
        card: card(&machine.accelerator_model),
        vram_gb: machine.vram_gb,
        usd_micros_per_hour: machine.price_usd_micros_per_hour,
        line: String::new(),
    }
}

/// The card to rent for `function` (as a machine's estimate names it) instead (cl-1024, Paul 11-15: "the cheapest card whose benchmark
/// is meaningfully faster"): of `offers`, the cards the Hub rents that hold it on one GPU, those its benchmarks (its release's, or its
/// source's) measured (`References::measured`, never a speed-scaled guess) that run it in under a third of `here_ms` and a minute sooner
/// (with no GPU here, `None`: any), the cheapest a render (its price times its time there); of those as cheap, the faster. And its time
/// there, ms.
fn recommended(references: &References, function: &str, size: &JobSize, offers: &[RentFor], here_ms: Option<f64>) -> Option<(RentFor, f64)> {
    let cost = |(offer, ms): &(&RentFor, f64)| offer.usd_micros_per_hour as f64 * ms;
    (offers.iter())
        .filter_map(|offer| Some((offer, references.measured(function, &offer.card, size)?.0)))
        .filter(|(_, ms)| here_ms.is_none_or(|here| here > SLOWER * ms && here - ms > LONGER_S * 1e3))
        .min_by(|a, b| cost(a).total_cmp(&cost(b)).then(a.1.total_cmp(&b.1)))
        .map(|(offer, ms)| (offer.clone(), ms))
}

/// A render's cost at `usd_micros_per_hour` for `ms`, as a person reads it: `about $0.08`, `under $0.01`.
fn per_render(usd_micros_per_hour: i64, ms: f64) -> String {
    match usd_micros_per_hour as f64 * ms / 3.6e6 / 1e6 {
        usd if usd < 0.01 => "under $0.01".into(),
        usd => format!("about ${usd:.2}"),
    }
}

/// A checkpoint's bytes in `lane`, else its least: a Hub model's release or checkpoint, by its card, kept a while. None: the Hub doesn't say.
async fn sized(api: &Api, hub: &str, written: &str, lane: Option<&str>) -> Option<u64> {
    checkpoint(api, hub, written, lane).await.map(|lane| lane.bytes)
}

/// The repository (`org/name`) a slot's choice names, as a machine names what it downloads; empty for any other choice.
fn repository(written: &str) -> String {
    match ModelRef::parse(written) {
        Ok(ModelRef::Hub { model, .. } | ModelRef::Checkpoint { model, .. }) => model.to_string(),
        _ => String::new(),
    }
}

/// The checkpoint, bytes and header of a checkpoint in `lane`, else its least, as `sized` reads them.
async fn checkpoint(api: &Api, hub: &str, written: &str, lane: Option<&str>) -> Option<Lane> {
    let key = format!("{hub}\n{written}\n{}", lane.unwrap_or_default());
    if let Some((_, held)) = SIZED.lock().expect("sized lock").get(&key).filter(|(at, _)| at.elapsed() < KEPT) {
        return held.clone();
    }
    let (model_id, release, checkpoint) = match ModelRef::parse(written).ok()? {
        ModelRef::Hub { model: model_id, release } => (model_id, release, None),
        ModelRef::Checkpoint { model: model_id, checkpoint } => (model_id, None, Some(checkpoint.to_string())),
        _ => return None,
    };
    let reader = api.machines.hubs.reader(hub).ok()?;
    let card = tokio::time::timeout(HUB_LATE, reader.get_model(&model_id.org, &model_id.name)).await.ok()?.ok()?;
    let found = match (&checkpoint, &release) {
        (Some(checkpoint), _) => (card.releases.iter().flat_map(|release| release.lanes.values()))
            .find(|held| &held.checkpoint == checkpoint)
            .map(|held| (held.checkpoint.clone(), held.bytes, held.header.clone()))
            .or_else(|| card.checkpoints.iter().find(|held| &held.id == checkpoint).map(|held| (held.id.clone(), held.bytes, held.header.clone()))),
        (None, release) => {
            let release = match release {
                Some(version) => card.releases.iter().find(|held| &held.version == version),
                None => card.releases.first(),
            };
            release
                .and_then(|release| lane.and_then(|lane| release.lanes.get(lane)).or_else(|| release.lanes.values().filter(|held| held.bytes > 0).min_by_key(|held| held.bytes)))
                .map(|held| (held.checkpoint.clone(), held.bytes, held.header.clone()))
        }
    };
    let found = (found.filter(|(_, bytes, _)| *bytes > 0))
        .map(|(checkpoint, bytes, header)| Lane { checkpoint, bytes: bytes as u64, header: Some(header).filter(|header| !header.sha256.is_empty()) });
    SIZED.lock().expect("sized lock").insert(key, (Instant::now(), found.clone()));
    found
}

/// The slots a run of a function waits for before it starts, the one set a quote's download sums over (cl-1071): its needs, as
/// its machine reckons them (TensorD's `access::Needs`, cl-1102). Their source is the rules in its interface, evaluated against the
/// request (`checkpoints`' `made`) by the evaluator TensorD shares (cl-1103): an on-demand slot the request is sure to read is
/// waited for, and its bytes are in the quote's download. Until a function has rules this is the same fallback as TensorD's
/// (`Plan::needs`): each slot not on demand; an on-demand one comes in the background and is fetched at once when first read.
/// `ruled` is what `function`'s rules say of the box as written (`needs::answers_written`): what only its bound request knows (a
/// mention's file, a pickup) leaves a rule unanswered and its slot as it was.
fn waited<'a>(slots: &'a [Slot], function: &'a str, ruled: &'a ::needs::Answers) -> impl Iterator<Item = &'a Slot> {
    let own = move |slot: &Slot| slot.path.split_once(".models.").is_some_and(|(owner, param)| owner == function && ruled.needed.contains(param));
    slots.iter().filter(move |slot| !slot.on_demand || own(slot))
}

/// Whether `slot`, `function`'s own or a function's it calls, is one its rules say this request doesn't read: nothing of it is
/// fetched, so nothing of it is quoted.
fn ruled_out(ruled: &::needs::Answers, function: &str, slot: &Slot) -> bool {
    match slot.path.split_once(".models.") {
        Some((owner, param)) if owner == function => ruled.unneeded.contains(param),
        Some((called, _)) => ruled.unneeded.contains(called),
        None => false,
    }
}

/// What `function` of `package` fetches before it starts, as TensorD fetches it (cl-1071): each slot it waits for (`waited`), by its Hub
/// card's lane checkpoint, and the bytes of the chunks its components read that no slot before it reads, each object once across
/// checkpoints (a fine-tune composed of a base's parts shares them); and, of its on-demand slots, those it may fetch besides, which a
/// quote can't know it reads. A header that can't be read counts its checkpoint whole. What a machine's estimate counts the fetch of
/// (cl-816, cl-1047); what the Hub doesn't say in time is left out.
pub(super) async fn checkpoints(api: &Api, hub: &str, package: &super::Package, function: &str, made: &Map<String, Value>) -> Fetched {
    let Some(mut weights) = Interface::parse(package.document()).ok().and_then(|interface| Weights::read(&interface, function, made)) else { return Fetched::default() };
    // A request not known here (`cozy run`'s placement names none) answers no rule.
    let ruled = match made.is_empty() {
        true => ::needs::Answers::default(),
        false => (::api::interface::entrypoint(package.document(), function))
            .map(|entry| ::needs::answers_written(entry, &Value::Object(made.clone())))
            .unwrap_or_default(),
    };
    weights.slots.retain(|slot| !ruled_out(&ruled, function, slot));
    let placed = match package.local {
        None => placed(api, hub, &package.name, &package.release, function).await,
        Some(_) => None,
    };
    let mut counted: HashSet<String> = HashSet::new();
    let mut fetched = Fetched::default();
    let waited: Vec<&Slot> = waited(&weights.slots, function, &ruled).collect();
    let on_demand = weights.slots.iter().filter(|slot| !waited.iter().any(|held| std::ptr::eq(*held, *slot)));
    for (slot, ahead) in waited.iter().map(|slot| (*slot, true)).chain(on_demand.map(|slot| (slot, false))) {
        let param = Weights::param(slot);
        let Some(written) = weights.on_box.get(param).or(slot.default.as_ref()) else { continue };
        let placed = placed.as_ref().and_then(|placed| placed.slots.iter().find(|(held, ..)| held == param));
        let lane = placed.map(|(_, lane, _)| lane.clone()).or_else(|| slot.lane.clone());
        let Some(held) = checkpoint(api, hub, written, lane.as_deref()).await else { continue };
        let chunks = match &held.header {
            Some(header) => crate::hub::headers::wanted(api, hub, header, &slot.fetched_components()).await,
            None => None,
        };
        let bytes = match chunks {
            Some(chunks) => chunks.iter().filter(|chunk| counted.insert(chunk.sha256.clone())).map(|chunk| chunk.length).sum(),
            None => held.bytes,
        };
        match ahead {
            true => fetched.models.push(api::machine::ModelRef { checkpoint: held.checkpoint, bytes, model: repository(written), reads: 0 }),
            false => fetched.on_demand_bytes += bytes,
        }
    }
    fetched
}

/// What a function fetches (`checkpoints`): what it waits for before it starts, and the most it may fetch on demand besides.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fetched {
    pub models: Vec<api::machine::ModelRef>,
    pub on_demand_bytes: u64,
}
