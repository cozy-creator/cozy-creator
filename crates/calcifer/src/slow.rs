//! A run far slower than its function is known to take on its card and lane says so, naming the likely cause (cl-913). What it's
//! known to take, its bar: this machine's runs of it, else runs on other machines of its card, else runs on another card scaled by
//! the Hub's per-SKU speeds; at its lane and size (megapixels and steps as its time left reads them, and frames, tokens and media
//! duration asked), never from a run itself slow or slowed (a hot GPU, a Runtime without its kernels). A run whose bar has stages is
//! timed from its first one, so a cold start's load is no slowness. Far is 1.4× the bar and 20 s more: said once, as soon as its
//! time so far is and a cause is known (`tick`), else at its end with what its record measured.
use ::api::{
    calcifer::{EtaBasis, Kind, Status, Warning},
    machine as v1,
};
use serde_json::{Map, Value};

use crate::{Calcifer, RunId, eta, lock, project::fit, store};

pub(crate) const SLOW_RUN: &str = "slow_run";
const SLOWER: f64 = 1.4;
const LONGER_MS: f64 = 20_000.0;
/// A GPU busy this long in a call, held back for heat more than this share of it, was hot (`platform::gpu`'s `LOADED_MS`, `HOT_SHARE`).
const LOADED_MS: u64 = 20_000;
const HOT: f32 = 0.5;
/// What a run says that makes it no bar for others, and that its box says.
const SLOWED: [&str; 3] = [SLOW_RUN, "gpu_throttled", "runtime_without_kernels"];
pub(crate) const ON_BOX: [&str; 3] = SLOWED;

/// What a run of its function is known to take on its card and lane.
#[derive(Clone, Debug)]
pub(crate) struct Bar {
    total_ms: f64,
    /// Of `total_ms`, before its first stage; none: its runs have no stages of their own (an orchestrator's).
    lead_ms: Option<f64>,
    /// Its stages' names: a run timed from its first stage is timed from entering one.
    stages: Vec<String>,
    /// The GPUs its runs ran on, the most of them.
    width: u32,
    /// What it stands on, in words.
    basis: String,
    /// What its runs asked beyond megapixels and steps, each unit's median.
    extent: Extent,
}

impl Bar {
    /// What it allows a run asking `extent`, timed from its first stage (`staged`), else from its start: what follows its load
    /// scales with what is asked.
    fn ms(&self, staged: bool, extent: &Extent) -> f64 {
        let scale = extent.ratio(&self.extent);
        match (staged, self.lead_ms) {
            (true, Some(lead)) => (self.total_ms - lead) * scale,
            (false, Some(lead)) => lead + (self.total_ms - lead) * scale,
            (_, None) => self.total_ms * scale,
        }
    }
}

/// What a run asks beyond megapixels and steps, as its input says: `frames`, `tokens`, and its media's seconds (every `duration`
/// in it, summed: a shot's beats).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Extent {
    frames: Option<f64>,
    tokens: Option<f64>,
    duration: Option<f64>,
}

impl Extent {
    fn of(input: &Map<String, Value>) -> Self {
        fn durations(value: &Value) -> f64 {
            match value {
                Value::Object(map) => map.iter().map(|(key, value)| match (key.as_str(), value.as_f64()) {
                    ("duration", Some(seconds)) => seconds.max(0.0),
                    _ => durations(value),
                }).sum(),
                Value::Array(items) => items.iter().map(durations).sum(),
                _ => 0.0,
            }
        }
        let number = |key: &str| input.get(key).and_then(Value::as_f64).filter(|n| *n > 0.0);
        let duration = durations(&Value::Object(input.clone()));
        Extent { frames: number("frames"), tokens: number("tokens"), duration: (duration > 0.0).then_some(duration) }
    }

    /// Each unit's median of `extents`.
    fn median(extents: &[Extent]) -> Self {
        let of = |pick: fn(&Extent) -> Option<f64>| {
            let mut values: Vec<f64> = extents.iter().filter_map(pick).collect();
            values.sort_by(f64::total_cmp);
            values.get(values.len() / 2).copied()
        };
        Extent { frames: of(|e| e.frames), tokens: of(|e| e.tokens), duration: of(|e| e.duration) }
    }

    /// How much more it asks than `other`: the product of each unit both know.
    fn ratio(&self, other: &Extent) -> f64 {
        [(self.frames, other.frames), (self.tokens, other.tokens), (self.duration, other.duration)]
            .into_iter()
            .filter_map(|(a, b)| Some(a? / b.filter(|b| *b > 0.0)?))
            .product()
    }
}

/// How many times its bar `took_ms` is, when far past it.
fn far(took_ms: f64, bar_ms: f64) -> Option<f64> {
    (bar_ms > 0.0 && took_ms > SLOWER * bar_ms && took_ms - bar_ms > LONGER_MS).then(|| took_ms / bar_ms)
}

fn input(row: &store::RunRow) -> Map<String, Value> {
    row.spec.payload.as_ref().and_then(|payload| serde_json::from_str(payload.get()).ok()).unwrap_or_default()
}

/// `org/pkg/fn@release` as its package, release and function.
fn parts(target: &str) -> Option<(&str, &str, &str)> {
    let (path, release) = target.split_once('@')?;
    let (package, function) = path.rsplit_once('/')?;
    Some((package, release, function))
}

fn minutes(ms: f64) -> String {
    match ms / 1e3 {
        s if s < 90.0 => format!("{s:.0} s"),
        s => format!("{:.1} min", s / 60.0),
    }
}

impl Calcifer {
    /// `row`'s bar on `machine`, from what ran before it. None: nothing like it ran, or it is no call.
    pub(crate) async fn bar(&self, row: &store::RunRow, machine: &str) -> Option<Bar> {
        if row.kind != Kind::Call {
            return None;
        }
        let base = row.target.split('@').next()?;
        let filter = store::RunFilter { status: Some(Status::Completed), target: Some(base.to_owned()), limit: 60, ..Default::default() };
        let (rows, ..) = self.store.runs(&filter).ok()?;
        let asked = input(row);
        let (megapixels, steps) = (crate::megapixels(&asked), asked.get("steps").and_then(Value::as_u64));
        let (like, extents): (Vec<eta::Took>, Vec<Extent>) = (rows.iter())
            .filter(|other| other.id != row.id && other.target.split('@').next() == Some(base) && other.spec.lane == row.spec.lane)
            .filter(|other| !other.warnings.iter().any(|warning| SLOWED.contains(&warning.code.as_str())))
            .filter_map(|other| {
                let outcome = other.outcome.as_ref()?;
                let took = eta::Took::of(other.machine.as_deref()?, &outcome.gpu, other.started_at_ms?, outcome.execution_ms, outcome.measurements.as_ref()?.get(), None)?;
                Some((took, Extent::of(&input(other))))
            })
            .unzip();
        let extent_of = |pick: &dyn Fn(&eta::Took) -> bool| Extent::median(&like.iter().zip(&extents).filter(|(took, _)| pick(took)).map(|(_, extent)| *extent).collect::<Vec<_>>());
        let gpu = self.gpu_of(machine);
        let card = fit::card(&gpu);
        // This machine's, else its card's on other machines.
        if let Some(prior) = eta::Prior::of(&like, machine, &gpu, megapixels) {
            let (basis, of, extent): (String, Vec<&eta::Took>, Extent) = match prior.basis {
                EtaBasis::Machine => (format!("{machine}'s last {} runs", prior.runs), like.iter().filter(|took| took.machine == machine).collect(), extent_of(&|took| took.machine == machine)),
                _ => (format!("{} runs on other {card} machines", prior.runs), like.iter().filter(|took| took.gpu == gpu).collect(), extent_of(&|took| took.gpu == gpu)),
            };
            return Some(Self::barred(prior.with_steps(steps), steps, &of, basis, 1.0, extent));
        }
        // Another card's, scaled by the Hub's per-SKU speeds.
        let machines = self.machines()?;
        let hub = row.hub.as_ref()?.to_string();
        let (package, release, function) = parts(&row.target)?;
        let speeds = tokio::time::timeout(crate::toolbox::HUB_LATE, fit::speeds(&machines.hubs, &hub, package, release, function)).await.ok()?;
        let to = *speeds.get(&card).filter(|speed| **speed > 0.0)?;
        let mut cards: Vec<&str> = like.iter().map(|took| took.gpu.as_str()).filter(|other| !other.is_empty()).collect();
        cards.sort_unstable();
        cards.dedup();
        (cards.into_iter())
            .filter_map(|other| {
                let speed = *speeds.get(&fit::card(other))?;
                let prior = eta::Prior::of(&like, "", other, megapixels)?;
                let of: Vec<&eta::Took> = like.iter().filter(|took| took.gpu == other).collect();
                let basis = format!("{} runs on an {} scaled by the Hub's speeds", prior.runs, fit::card(other));
                let extent = extent_of(&|took| took.gpu == other);
                Some((prior.runs, Self::barred(prior.with_steps(steps), steps, &of, basis, speed / to, extent)))
            })
            .max_by_key(|(runs, _)| *runs)
            .map(|(_, bar)| bar)
    }

    fn barred(prior: eta::Prior, steps: Option<u64>, of: &[&eta::Took], basis: String, scale: f64, extent: Extent) -> Bar {
        let mut widths: Vec<u32> = of.iter().map(|took| took.width).collect();
        widths.sort_unstable();
        Bar {
            total_ms: prior.total_ms(steps) * scale,
            lead_ms: (!prior.stages.is_empty()).then_some(prior.lead_ms * scale),
            stages: prior.stages.iter().map(|stage| stage.name.clone()).collect(),
            width: widths.get(widths.len() / 2).copied().unwrap_or(1),
            basis,
            extent,
        }
    }

    /// Its bar, worked out once as its machine is followed.
    pub(crate) async fn set_bar(&self, row: &store::RunRow, machine: &str) {
        if lock(&self.live).get(&row.id).is_some_and(|live| live.barred) {
            return;
        }
        let bar = self.bar(row, machine).await;
        let mut live = lock(&self.live);
        let live = live.entry(row.id.clone()).or_default();
        (live.bar, live.barred) = (bar, true);
    }

    /// A stage it entered, on its machine's clock, each name's first: the first of its bar's starts its staged time.
    pub(crate) fn staged(&self, id: &RunId, stage: &str, at_ms: i64) {
        let mut live = lock(&self.live);
        let entered = &mut live.entry(id.clone()).or_default().entered;
        if entered.len() < 64 && !entered.iter().any(|(named, _)| named == stage) {
            entered.push((stage.to_owned(), at_ms));
        }
    }

    /// As it runs: far past its bar already, it says so now.
    pub(crate) fn slow_so_far(&self, id: &RunId) {
        let (bar, since, width) = {
            let live = lock(&self.live);
            let Some(live) = live.get(id).filter(|live| !live.slow_said) else { return };
            let Some(bar) = live.bar.clone() else { return };
            let first = live.entered.iter().find(|(stage, _)| bar.stages.contains(stage)).map(|(_, at)| *at);
            (bar, first, live.width)
        };
        let Ok(Some(row)) = self.store.run(id.as_str()) else { return };
        let (staged, since) = match (bar.lead_ms, since, row.started_at_ms) {
            (Some(_), Some(since), _) => (true, since),
            (None, _, Some(started)) => (false, started),
            _ => return,
        };
        let took = (store::now_ms() - since) as f64;
        let allowed = bar.ms(staged, &Extent::of(&input(&row)));
        let Some(times) = far(took, allowed) else { return };
        // As it runs, only with a cause known: what its input doesn't say of its size, its end does.
        let machine = row.machine.clone().unwrap_or_default();
        let causes = self.causes(&row, &machine, None, width, &bar);
        if causes.is_empty() {
            return;
        }
        let message = format!(
            "run {} has run {} so far, {times:.1}× the {} {} takes here ({}){}",
            row.number,
            minutes(took),
            minutes(allowed),
            row.target.split('@').next().unwrap_or_default(),
            bar.basis,
            likely(&causes)
        );
        self.say_slow(id, message);
    }

    /// As it ends with `outcome`, before it settles (what lands of it carries its warnings): far past its bar, it says so with what
    /// its record measured.
    pub(crate) fn slow_at_end(&self, id: &RunId, outcome: &v1::Outcome) {
        if outcome.status != "succeeded" || lock(&self.live).get(id).is_some_and(|live| live.slow_said) {
            return;
        }
        let Some(bar) = lock(&self.live).get(id).and_then(|live| live.bar.clone()) else { return };
        let Ok(Some(row)) = self.store.run(id.as_str()) else { return };
        let (Some(machine), Some(started)) = (row.machine.clone(), row.started_at_ms) else { return };
        let took = (outcome.measurements.as_ref())
            .and_then(|measured| eta::Took::of(&machine, &outcome.gpu, started, outcome.execution_ms, measured.get(), None));
        let (staged, took_ms) = match (&took, bar.lead_ms) {
            (Some(took), Some(_)) if !took.stages.is_empty() => (true, took.total_ms() - took.lead_ms),
            _ => (false, outcome.execution_ms as f64),
        };
        let allowed = bar.ms(staged, &Extent::of(&input(&row)));
        let Some(times) = far(took_ms, allowed) else { return };
        let width = lock(&self.live).get(id).map_or(0, |live| live.width);
        let causes = self.causes(&row, &machine, Some(outcome), width, &bar);
        let message = format!(
            "run {} took {}, {times:.1}× the {} {} takes here ({}){}",
            row.number,
            minutes(took_ms),
            minutes(allowed),
            row.target.split('@').next().unwrap_or_default(),
            bar.basis,
            likely(&causes)
        );
        self.say_slow(id, message);
    }

    fn say_slow(&self, id: &RunId, message: String) {
        if let Some(live) = lock(&self.live).get_mut(id) {
            live.slow_said = true;
        }
        tracing::warn!(run = %id, "{message}");
        let warning = Warning { code: SLOW_RUN.into(), message };
        match self.store.warn(id.as_str(), &warning) {
            Ok(events) => self.publish(id, events),
            Err(error) => tracing::warn!(run = %id, "{SLOW_RUN} not kept: {error}"),
        }
    }

    /// What likely made `row` slow, by its own warnings, its machine's word now, and once it ended (`ended`) what its record
    /// measured: heat, a Runtime without its kernels, kernels still building, another degree than its bar's.
    fn causes(&self, row: &store::RunRow, machine: &str, ended: Option<&v1::Outcome>, width: u32, bar: &Bar) -> Vec<String> {
        let mut causes: Vec<String> = vec![];
        let codes: Vec<&str> = row.warnings.iter().map(|warning| warning.code.as_str()).collect();
        let frame = self.machines().and_then(|machines| machines.frame(machine));
        let mut hot: Vec<(u32, f32)> = frame.iter().flat_map(|frame| frame.heat.iter().map(|hot| (hot.gpu, hot.share))).collect();
        let mut without = codes.contains(&"runtime_without_kernels") || frame.as_ref().is_some_and(|frame| !frame.runtime_kernels.is_empty());
        let mut degree = width;
        let mut building = 0u64;
        let mut compiled = 0usize;
        if let Some(outcome) = ended {
            // Its own measurements and its calls', which its log holds by now.
            let own: Option<Value> = outcome.measurements.as_ref().and_then(|measured| serde_json::from_str(measured.get()).ok());
            let calls = self.store.detail(row).map(|run| run.calls).unwrap_or_default();
            let measured: Vec<&Value> = own.iter().chain(calls.iter().filter_map(|call| call.measurements.as_ref())).collect();
            for measurements in measured {
                for gpu in measurements["memory"]["driver"].as_array().into_iter().flatten() {
                    let share = gpu["heat_share"].as_f64().unwrap_or(0.0) as f32;
                    if share > HOT && gpu["busy_ms"].as_u64().unwrap_or(0) >= LOADED_MS {
                        hot.push((gpu["gpu"].as_u64().unwrap_or(0) as u32, share));
                    }
                }
                for observed in measurements["observations"].as_array().into_iter().flatten() {
                    match observed["name"].as_str().unwrap_or_default() {
                        "gemm.fallback" if observed.to_string().contains("carries no kernels") || observed.to_string().contains("kernels do not load") => without = true,
                        "attention.sol.calls" => building += observed["fields"]["dense_building"].as_u64().unwrap_or(0),
                        _ => {}
                    }
                }
                let execution = &measurements["execution"];
                degree = degree.max(execution["degree"].as_u64().unwrap_or(0) as u32);
                compiled += (execution["gpus"].as_array().into_iter().flatten())
                    .flat_map(|gpu| gpu["kernels"].as_array().into_iter().flatten())
                    .filter(|kernel| kernel["source"] == "compiled")
                    .count();
            }
        }
        hot.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        hot.dedup_by_key(|(gpu, _)| *gpu);
        for (gpu, share) in hot {
            causes.push(format!("GPU {gpu} throttled for heat {:.0}% of the time", share * 100.0));
        }
        if codes.contains(&"gpu_throttled") && causes.is_empty() {
            causes.push("a GPU throttled for heat".into());
        }
        if without {
            causes.push("its machine's Runtime serves no native kernels (slower GEMMs, no FlashAttention-3)".into());
        }
        if building > 0 {
            causes.push(format!("Sol attention ran dense in {building} calls while its kernel built"));
        }
        if compiled > 0 {
            causes.push(format!("{compiled} kernels compiled during it"));
        }
        if degree > 0 && degree != bar.width {
            causes.push(format!("it ran on {degree} GPUs, its bar's runs on {}", bar.width));
        }
        causes
    }
}

fn likely(causes: &[String]) -> String {
    match causes {
        [] => ". No cause found in its record: cozy run watch --full shows it".into(),
        causes => format!(". Likely: {}", causes.join("; ")),
    }
}
