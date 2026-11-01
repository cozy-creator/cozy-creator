//! What a run's machine measured (`Outcome.measurements`, `Call.measurements`), worked out into
//! `protocol::report::Report` for `GET /v1/runs/{id}?detail=full`: where each executor's time went,
//! the bytes each GPU moved and at what rate against its ceiling, and what make_room took.
use std::collections::BTreeMap;

use protocol::{
    calcifer::{Call, Run},
    report::{self, Report},
};
use serde::Deserialize;

/// The run's report: its own executor's measurements over its execution, then each call's over
/// its call. None when nothing measured it.
pub(crate) fn of(run: &Run) -> Option<Report> {
    let execution = run.settled_at_ms.zip(run.execution_ms).map(|(end, ms)| (end - ms as i64, end));
    let measured = measured(run.measurements.as_ref().map(|m| (m, execution)), &run.calls);
    if measured.is_empty() {
        return None;
    }
    let mut pins: Vec<String> = (measured.iter())
        .flat_map(|m| &m.m.execution.gpus)
        .map(|gpu| gpu.attention.requested.clone())
        .filter(|pin| !pin.is_empty())
        .collect();
    pins.sort_unstable();
    pins.dedup();
    let measured = measured
        .iter()
        .map(|m| report::Measured {
            by: m.who.clone(),
            stages: stages(m),
            models: models(m),
            components: components(m),
            kernels: kernels(m),
            memory: memory(m),
            steps: steps(m),
            make_room: make_room(m),
        })
        .collect();
    Some(Report { attention_pins: pins, measured })
}

/// What an executor measured (`Outcome.measurements`, `Call.measurements`), read tolerantly: a
/// newer machine's fields are ignored and absent ones are empty.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Measurements {
    #[serde(deserialize_with = "lenient")]
    attribution: Attribution,
    #[serde(deserialize_with = "lenient")]
    execution: Execution,
    #[serde(deserialize_with = "lenient")]
    observations: Vec<Observation>,
    /// The models it ran, as its machine resolved them.
    #[serde(deserialize_with = "lenient")]
    models: Vec<Model>,
    #[serde(deserialize_with = "lenient")]
    memory: Memory,
}

/// A field of a shape Calcifer does not read (`null`, a newer one) reads as empty: it never
/// takes the other measurements with it.
fn lenient<'de, D: serde::Deserializer<'de>, T: Default + serde::de::DeserializeOwned>(d: D) -> Result<T, D::Error> {
    Ok(T::deserialize(serde_json::Value::deserialize(d)?).unwrap_or_default())
}

/// Its executor's memory (TensorD's `call_memory`): torch's high-water marks and, under `stats`,
/// the invoke reply's own `memory`: per GPU, Runtime's record beside its fill ceilings and
/// torch's peak. This CLI projection reads the fields it displays, without loading an allocator.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Memory {
    #[serde(deserialize_with = "lenient")]
    stats: Gpus,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Gpus {
    /// One row per GPU; a row of a shape Calcifer does not read is left out alone.
    #[serde(deserialize_with = "each")]
    varena: Vec<GpuMemory>,
}

/// Each row of a list that reads, the others left out.
fn each<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let rows = Vec::<serde_json::Value>::deserialize(d).unwrap_or_default();
    Ok(rows.into_iter().filter_map(|row| T::deserialize(row).ok()).collect())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GpuMemory {
    gpu: u32,
    ceiling: Option<Ceiling>,
    stats: ResidencyStats,
    torch_peak: Option<u64>,
}

/// TensorFS's measured fill ceilings on that GPU: host to device and disk, GB/s.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Ceiling {
    h2d_gbps: Option<f64>,
    disk_gbps: f64,
}

/// A display projection of Runtime's memory record. Optional values stay unknown when the
/// allocator cannot attribute them to one request (stock Torch figures are process-scoped).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ResidencyStats {
    allocator_scope: String,
    trace_scope: String,
    moved: Moved,
    trace: Vec<Reference>,
    steps: Vec<MemoryStep>,
    stall_ns: u64,
    evicted: [u64; 7],
    peak: Option<u64>,
    compute: Option<u64>,
    leftover: Option<u64>,
    outlived: Option<Vec<Outlived>>,
    make_room: Vec<MakeRoom>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Fills { bytes: u64, ns: u64 }

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Moved { kept: Fills, leased: Fills, store: Fills, derived: Fills }

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Reference { step: u32, bytes: u64, origin: String, speculative: bool }

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MemoryStep { capacity: u64, evicted: [u64; 7] }

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Outlived { bytes: u64 }

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MakeRoom { step: u32, trigger: String, need: u64, freed: u64, waited: u32 }

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Attribution {
    stages: BTreeMap<String, Track>,
    steps: BTreeMap<String, Track>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Track {
    count: u64,
    total_ms: f64,
    started_unix_ms: i64,
    ended_unix_ms: Option<i64>,
    first_ms: Option<f64>,
    last_ms: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Execution {
    gpus: Vec<Gpu>,
    /// The executor's start: its fork, imports and CUDA initialization, one after another.
    executor: Option<Legs>,
    /// Its construction's preparation (qualifying, constructing, registering its models).
    construction: Option<Legs>,
    /// Each on-demand model's build, by slot, made inside the stage that first used it.
    on_demand: BTreeMap<String, Legs>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Legs {
    legs_ms: BTreeMap<String, f64>,
    started_unix_ms: Option<i64>,
    prepare_ms: Option<f64>,
    prepared_unix_ms: Option<i64>,
}

impl Legs {
    /// When it ran: an executor from its start for its legs' sum, a construction for its
    /// `prepare_ms` until it was prepared.
    fn span(&self) -> Option<(f64, f64)> {
        match (self.started_unix_ms, self.prepare_ms, self.prepared_unix_ms) {
            (Some(start), _, _) => Some((
                start as f64,
                start as f64 + self.legs_ms.values().sum::<f64>(),
            )),
            (None, Some(ms), Some(end)) if ms > 0.0 => Some((end as f64 - ms, end as f64)),
            _ => None,
        }
    }

    /// Its three longest legs, a construction's sub-legs aside.
    fn longest(&self) -> Vec<report::Leg> {
        let mut legs: Vec<(&String, &f64)> =
            self.legs_ms.iter().filter(|(name, ms)| **ms >= 1.0 && !name.contains("construct.")).collect();
        legs.sort_by(|a, b| b.1.total_cmp(a.1));
        legs.iter().take(3).map(|(name, ms)| report::Leg { name: (*name).clone(), ms: **ms }).collect()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Gpu {
    gpu: Option<i64>,
    arch: String,
    attention: Attention,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Attention {
    /// The request's pin ("" = the machine's own selection).
    requested: String,
    kernels: Vec<Kernel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Kernel {
    kernel: String,
    state: String,
    served: bool,
    progress: Option<f64>,
    compile_ms: f64,
    detail: String,
}

/// One kernel choice the executor confessed: `attention.selected` and `gemm.selected` per
/// component, `gemm.fallback` when a component's GEMM leaf could not serve (its log warns).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Observation {
    name: String,
    value: serde_json::Value,
    fields: Fields,
}

impl Observation {
    fn value(&self) -> String {
        match &self.value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Fields {
    component: String,
    format: String,
    wanted: String,
    served: String,
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Model {
    slot: String,
    repository: String,
    release: String,
    lane: String,
    /// The header's precision family (`int8`, …), shown beside the lane.
    precision: String,
    manifest: String,
    /// The LoRAs on it, as the run named them.
    adapters: Vec<Adapter>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Adapter {
    model: String,
    strength: f64,
    components: Vec<String>,
}


/// One executor's measurements, by who measured them (`run`, or the call's index), and the
/// span they account for: the run's execution, or the call from its call to its end.
struct Measured {
    who: String,
    span: Option<(i64, i64)>,
    m: Measurements,
}

type Span = Option<(i64, i64)>;

fn measured(own: Option<(&serde_json::Value, Span)>, calls: &[Call]) -> Vec<Measured> {
    let read = |who: String, m: &serde_json::Value, span: Span| {
        let m = Measurements::deserialize(m).ok()?;
        Some(Measured { who, span, m })
    };
    let own = own.and_then(|(m, span)| read("run".into(), m, span));
    let calls = calls.iter().filter_map(|c| {
        let span = (c.finished_at_ms >= c.called_at_ms).then_some((c.called_at_ms, c.finished_at_ms));
        read(c.index.to_string(), c.measurements.as_ref()?, span)
    });
    own.into_iter().chain(calls).collect()
}

/// Where the span's time went, in the order it ran: the executor's start and its construction
/// when they ran inside it (a warm executor's were an earlier call's), each stage with its steps,
/// and last what no measurement covers.
fn stages(measured: &Measured) -> Vec<report::Stage> {
    let (attribution, execution) = (&measured.m.attribution, &measured.m.execution);
    let inside = |(start, end): (f64, f64)| measured.span.is_none_or(|(from, to)| end > from as f64 && start < to as f64);
    // Each row by its start, and the times it covers: a stage met twice covers its first and
    // its last occurrence; those between are counted, not placed.
    let (mut rows, mut covers, mut unplaced) = (vec![], vec![], 0.0);
    let built = execution.on_demand.iter().map(|(slot, legs)| (format!("loading {slot}"), Some(legs)));
    let prepared = [
        ("executor start".to_string(), execution.executor.as_ref()),
        ("construction".to_string(), execution.construction.as_ref()),
    ];
    for (name, legs) in prepared.into_iter().chain(built) {
        if let Some((legs, span)) = legs.and_then(|l| Some((l, l.span()?))).filter(|(_, s)| inside(*s)) {
            covers.push(span);
            let row = report::Stage { name, ms: span.1 - span.0, legs: legs.longest(), ..Default::default() };
            rows.push((span.0, row));
        }
    }
    let mut tracks: Vec<(&String, &Track)> = attribution.stages.iter().collect();
    for (name, track) in &attribution.steps {
        if !attribution.stages.contains_key(name) {
            tracks.push((name, track));
        }
    }
    for (name, track) in tracks {
        let steps = (attribution.steps.get(name))
            .map(|s| report::Steps { count: s.count, each_ms: s.total_ms / s.count.max(1) as f64 });
        let start = track.started_unix_ms as f64;
        let end = track.ended_unix_ms.map_or(start + track.total_ms, |end| end as f64);
        match (track.count, track.first_ms, track.last_ms) {
            (2.., Some(first), Some(last)) => {
                covers.extend([(start, start + first), (end - last, end)]);
                unplaced += track.total_ms - first - last;
            }
            _ => covers.push((start, end)),
        }
        rows.push((start, report::Stage { name: name.clone(), ms: track.total_ms, steps, ..Default::default() }));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    covers.sort_by(|a, b| a.0.total_cmp(&b.0));
    let unaccounted = measured.span.map(|(from, to)| {
        let (from, to) = (from as f64, to as f64);
        let (mut covered, mut reached) = (unplaced, from);
        for (start, end) in covers {
            let (start, end) = (start.max(reached), end.min(to));
            if end > start {
                covered += end - start;
                reached = end;
            }
        }
        to - from - covered
    });
    let mut rows: Vec<report::Stage> = rows.into_iter().map(|(_, row)| row).collect();
    if let Some(ms) = unaccounted.filter(|ms| *ms >= 1.0) {
        rows.push(report::Stage { name: "unaccounted".into(), ms, unaccounted: true, ..Default::default() });
    }
    rows
}

fn models(measured: &Measured) -> Vec<report::Model> {
    let model = |m: &Model| report::Model {
        slot: m.slot.clone(),
        repository: m.repository.clone(),
        release: m.release.clone(),
        manifest: m.manifest.clone(),
        lane: m.lane.clone(),
        precision: m.precision.clone(),
        adapters: (m.adapters.iter())
            .map(|a| report::Adapter { model: a.model.clone(), strength: a.strength, components: a.components.clone() })
            .collect(),
    };
    measured.m.models.iter().map(model).collect()
}

/// Each component's attention and GEMM kernels, as its executor confessed them.
fn components(measured: &Measured) -> Vec<report::Component> {
    let mut components: BTreeMap<&str, report::Component> = BTreeMap::new();
    for o in &measured.m.observations {
        let fields = &o.fields;
        let row = components.entry(&fields.component).or_insert_with(|| report::Component {
            component: fields.component.clone(),
            ..Default::default()
        });
        match o.name.as_str() {
            "attention.selected" => row.attention.push(o.value()),
            "gemm.selected" => row.gemm.push(report::Gemm { kernel: o.value(), format: fields.format.clone(), fallback: None }),
            "gemm.fallback" => {
                let kernel = o.value();
                let served = [&fields.format, &fields.served].into_iter().find(|s| !s.is_empty() && **s != kernel);
                row.gemm.push(report::Gemm {
                    kernel,
                    format: fields.format.clone(),
                    fallback: Some(report::Fallback {
                        served: served.cloned().unwrap_or_default(),
                        wanted: fields.wanted.clone(),
                        reason: fields.reason.clone(),
                    }),
                });
            }
            _ => {}
        }
    }
    components.into_values().filter(|c| !c.attention.is_empty() || !c.gemm.is_empty()).collect()
}

fn kernels(measured: &Measured) -> Vec<report::Kernel> {
    let mut rows = vec![];
    for gpu in &measured.m.execution.gpus {
        for kernel in &gpu.attention.kernels {
            rows.push(report::Kernel {
                gpu: gpu.gpu.filter(|n| *n >= 0).map(|n| n as u32),
                arch: gpu.arch.clone(),
                kernel: kernel.kernel.clone(),
                state: kernel.state.clone(),
                served: kernel.served,
                progress: kernel.progress,
                compile_ms: kernel.compile_ms,
                detail: kernel.detail.clone(),
            });
        }
    }
    rows
}

/// Each GPU's whole request: the bytes moved by source at their rate against the measured
/// ceiling, the stall, the most it held, the compute bytes, what make_room took and what it still
/// held at the end.
fn memory(measured: &Measured) -> Vec<report::Memory> {
    let row = |g: &GpuMemory| {
        let s = &g.stats;
        let h2d = g.ceiling.as_ref().and_then(|c| c.h2d_gbps);
        let disk = g.ceiling.as_ref().map(|c| c.disk_gbps);
        report::Memory {
            gpu: g.gpu,
            scope: if s.allocator_scope.is_empty() { "request".into() } else { s.allocator_scope.clone() },
            kept: moved(&s.moved.kept, None),
            leased: moved(&s.moved.leased, h2d),
            store: moved(&s.moved.store, disk),
            derived: moved(&s.moved.derived, disk),
            stall_ms: s.stall_ns as f64 / 1e6,
            peak: s.peak,
            torch_peak: g.torch_peak,
            compute: s.compute,
            evicted: taken(&s.evicted),
            leftover: s.leftover.map(|n| n.saturating_add(s.outlived.iter().flatten().map(|o| o.bytes).sum::<u64>())),
        }
    };
    measured.m.memory.stats.varena.iter().map(row).collect()
}

/// Bytes and, when they took time, their rate and its share of the ceiling.
fn moved(fills: &Fills, ceiling: Option<f64>) -> report::Moved {
    let gbps = (fills.bytes > 0 && fills.ns > 0).then(|| fills.bytes as f64 / fills.ns as f64);
    let ceiling_share = gbps.zip(ceiling.filter(|c| *c > 0.0)).map(|(gbps, c)| gbps / c);
    report::Moved { bytes: fills.bytes, gbps, ceiling_share }
}

/// What make_room took, by class (F, O, S, N, D, W, P).
fn taken(by_class: &[u64; 7]) -> Vec<report::Taken> {
    (["F", "O", "S", "N", "D", "W", "P"].iter().zip(by_class))
        .filter(|(_, bytes)| **bytes > 0)
        .map(|(class, bytes)| report::Taken { class: (*class).into(), bytes: *bytes })
        .collect()
}

/// Each GPU's plan steps and the bytes its binds found by origin.
fn steps(measured: &Measured) -> Vec<report::Step> {
    let mut rows = vec![];
    for g in &measured.m.memory.stats.varena {
        if g.stats.trace_scope == "fills" {
            // Fill order is not an execution plan step. Totals remain in the memory table.
            continue;
        }
        let mut by_step: BTreeMap<u32, [u64; 5]> = BTreeMap::new();
        for r in &g.stats.trace {
            let cell = by_step.entry(r.step).or_default();
            let at = match r.origin.as_str() {
                "hit" => 0,
                "kept" => 1,
                "store" => 2,
                "derived" => 3,
                _ => continue,
            };
            cell[at] += r.bytes;
            if r.speculative {
                cell[4] += r.bytes;
            }
        }
        for step in 0..g.stats.steps.len() as u32 {
            by_step.entry(step).or_default();
        }
        for (step, [hit, kept, store, derived, prefetched]) in by_step {
            let stats = g.stats.steps.get(step as usize);
            rows.push(report::Step {
                gpu: g.gpu,
                step,
                capacity: stats.map(|s| s.capacity),
                hit,
                kept,
                store,
                derived,
                prefetched,
                evicted: stats.map(|s| taken(&s.evicted)),
            });
        }
    }
    rows
}

fn make_room(measured: &Measured) -> Vec<report::MakeRoom> {
    let mut rows = vec![];
    for g in &measured.m.memory.stats.varena {
        for m in &g.stats.make_room {
            rows.push(report::MakeRoom {
                gpu: g.gpu,
                step: m.step,
                trigger: m.trigger.clone(),
                need: m.need,
                freed: m.freed,
                waited: m.waited,
            });
        }
    }
    rows
}
