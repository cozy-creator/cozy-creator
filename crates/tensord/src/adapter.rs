//! Caller LoRA adapters as one view over a base checkpoint (the Python worker's
//! `lora_composition.prepare`), on linear and 2-D convolution layers: base `<path>.weight/.bias`
//! become `<path>.base_layer.*`, factors graft in as `<path>.lora_{A,B}.adapter_<i>.weight`, and
//! each adapted component's asset `model_adapters.json` holds the ordered graph the executor binds
//! with PEFT. Nothing is computed, and large parts keep their objects. Factors must already be
//! canonical PEFT keys (`<path>.lora_A.weight`, `<path>.lora_B.weight`, optional scalar
//! `<path>.alpha`): foreign layouts are normalized when a model is imported, never here. A view is
//! composed by reference and never written: a header in memory whose tensors point at the base's
//! and the factors' own chunks (`storage::view`), and its graph, the only bytes it adds.
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use serde::{Deserialize, Serialize};
use tensorfs::format::{Body, Component, Dtype, Header, ObjectRef, Tensor};
use tensorfs::store::{Checkpoint, Store};

use crate::storage;

pub const FORMAT: &str = "cozy.model.lora/1";
/// The graph's asset in each adapted component of the view.
pub const GRAPH_FILE: &str = "model_adapters.json";

/// One caller adapter, in order. With neither component named, each of the adapter's factor
/// components adapts the base component named like it, else the one its layers fit, of those the
/// call `runs` (empty: all); a named `source_component` alone adapts the one it fits; a named
/// `component` takes the factor component named like it, else `adapter`, else the only one.
#[derive(Clone, Debug)]
pub struct Selection {
    pub checkpoint: ObjectRef,
    pub component: String,
    pub source_component: String,
    pub strength: f64,
    pub runs: Vec<String>,
}

/// What composes a view again, by id: a plan keeps it, since a view lives only in memory (a
/// restart, or the registry's cap, loses it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub base: String,
    pub selections: Vec<Chosen>,
}

/// One [`Selection`] by its checkpoint's id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chosen {
    pub checkpoint: String,
    pub component: String,
    pub source_component: String,
    pub strength: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<String>,
}

/// The view `id`, composed again from `recipe` when this process no longer keeps it.
pub fn ensure(store: &Store, id: &str, recipe: &Recipe) -> io::Result<()> {
    if storage::view(id).is_some() {
        return Ok(());
    }
    let selections = (recipe.selections.iter())
        .map(|c| Ok(Selection {
            checkpoint: storage::held(store, &c.checkpoint)?,
            component: c.component.clone(),
            source_component: c.source_component.clone(),
            strength: c.strength,
            runs: c.runs.clone(),
        }))
        .collect::<io::Result<Vec<_>>>()?;
    let composed = compose(store, &storage::held(store, &recipe.base)?, &selections)?;
    match composed.checkpoint.id().sha256 == id.trim_start_matches("sha256:") {
        true => Ok(()),
        false => Err(io::Error::other(format!("{id} composes again as {}", composed.checkpoint.id().id()))),
    }
}

/// The composed view, `view-<composition>`, open in memory (`storage::view`): it holds the base's
/// and the factors' objects while the caller holds it.
#[derive(Clone, Debug)]
pub struct Composed {
    pub name: String,
    pub checkpoint: Checkpoint,
    /// The base components each selection adapts, in order.
    pub adapted: Vec<Vec<String>>,
    /// What each selection's run is told: a LoRA trained on another component than it adapts.
    pub warnings: Vec<Vec<String>>,
}

fn refuse(code: &str, detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("adapter_{code}: {}", detail.into()))
}

#[derive(Serialize)]
struct Layer {
    component: String,
    target: String,
    adapter: String,
    rank: u64,
    alpha: f64,
    strength: f64,
    dtype: String,
}

#[derive(Serialize)]
struct AdapterRef {
    #[serde(rename = "ref")]
    reference: String,
    scale: f64,
    kind: &'static str,
    component: String,
    source_component: String,
    family: &'static str,
}

#[derive(Serialize)]
struct Graph {
    format: &'static str,
    layers: Vec<Layer>,
    adapters: Vec<AdapterRef>,
}

/// Every factor value finite (zero-strength inputs too); a scalar's value is returned.
fn validate(checkpoint: &Checkpoint, component: &str, key: &str, tensor: &Tensor) -> io::Result<Option<f64>> {
    let width = match tensor.dtype {
        Dtype::F16 | Dtype::BF16 => 2,
        Dtype::F32 => 4,
        _ => 0,
    };
    if width == 0 || !matches!(tensor.body, Body::Data(_)) {
        return Err(refuse("tensor", format!("{component}.{key}: plain f16, bf16 or f32 factors required")));
    }
    let bytes = checkpoint.read(component, key).map_err(storage::io_error)?;
    let values: Vec<f64> = bytes
        .chunks_exact(width)
        .map(|b| match tensor.dtype {
            Dtype::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Dtype::BF16 => f32::from_bits((u16::from_le_bytes([b[0], b[1]]) as u32) << 16) as f64,
            _ => half(u16::from_le_bytes([b[0], b[1]])),
        })
        .collect();
    if values.iter().any(|v| !v.is_finite()) {
        return Err(refuse("nonfinite", format!("{component}.{key}: factor contains NaN or infinity")));
    }
    Ok(if tensor.shape.is_empty() { values.first().copied() } else { None })
}

fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let fraction = (bits & 0x3ff) as f64;
    match exponent {
        0 => sign * fraction * 2f64.powi(-24),
        31 if fraction == 0.0 => sign * f64::INFINITY,
        31 => f64::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2f64.powi(exponent - 15),
    }
}

/// Factor pairs by target path: role (`a`, `b`, `alpha`) → (key, tensor).
type Pairs<'h> = BTreeMap<String, BTreeMap<&'static str, (String, &'h Tensor)>>;

fn pairs(factors: &BTreeMap<String, Tensor>) -> io::Result<Pairs<'_>> {
    let mut pairs: Pairs = BTreeMap::new();
    for (key, tensor) in factors {
        let (path, role) = [(".lora_A.weight", "a"), (".lora_B.weight", "b"), (".alpha", "alpha")]
            .into_iter()
            .find_map(|(suffix, role)| key.strip_suffix(suffix).map(|path| (path, role)))
            .ok_or_else(|| refuse("key", format!("unsupported adapter tensor {key:?}")))?;
        if path.is_empty() {
            return Err(refuse("target", "an adapter target must name a child linear layer"));
        }
        pairs.entry(path.into()).or_default().insert(role, (key.clone(), tensor));
    }
    Ok(pairs)
}

/// Whether factors `a` and `b` adapt base `weight`: a linear's `[out, in]` as A `[r, in]` and B
/// `[out, r]`, or a convolution's `[out, in, kh, kw]` as A `[r, in, kh, kw]` and B `[out, r, 1, 1]`.
fn fits(a: &Tensor, b: &Tensor, weight: &Tensor) -> bool {
    let (w, rank) = (&weight.shape, a.shape.first().copied().unwrap_or(0));
    matches!(w.len(), 2 | 4)
        && a.shape.len() == w.len()
        && b.shape.len() == w.len()
        && rank > 0
        && a.shape[1..] == w[1..]
        && b.shape[..2] == [w[0], rank]
        && b.shape[2..].iter().all(|k| *k == 1)
}

/// A component's tensors by key.
fn keyed(component: &Component) -> BTreeMap<String, Tensor> {
    component.tensors.iter().map(|t| (t.key.clone(), t.clone())).collect()
}

/// The one base component `factors` fit among those the call `runs`; of several, the one the LoRA
/// was `trained_on` (a family component, through the base's `family` record) when it is one.
fn infer_component(base: &Header, runs: &[String], factors: &BTreeMap<String, Tensor>, trained_on: Option<&str>) -> io::Result<String> {
    let pairs = pairs(factors)?;
    let fits = |tensors: &BTreeMap<String, Tensor>| {
        !pairs.is_empty()
            && pairs.iter().all(|(path, pair)| match (pair.get("a"), pair.get("b"), tensors.get(&format!("{path}.weight"))) {
                (Some((_, a)), Some((_, b)), Some(weight)) => fits(a, b, weight),
                _ => false,
            })
    };
    let run = |c: &&Component| runs.is_empty() || runs.contains(&c.name);
    let matches: Vec<&String> = base.components().iter().filter(run).filter(|c| fits(&keyed(c))).map(|c| &c.name).collect();
    let ran = || match runs.is_empty() {
        true => "the model's components".to_string(),
        false => format!("the components this call runs ({})", runs.join(", ")),
    };
    let family = tensorfs::family::Record::read(&base.metadata);
    let trained = |c: &&&String| trained_on.is_some_and(|t| family.as_ref().map_or(c.as_str(), |f| f.component(c)) == t);
    if let (true, Some(one)) = (matches.len() > 1, matches.iter().find(trained)) {
        return Ok((*one).clone());
    }
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(refuse("component", format!("the LoRA's layers fit none of {}", ran()))),
        _ => Err(refuse(
            "component",
            format!("the LoRA's layers fit more than one of {}: {}", ran(), matches.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
        )),
    }
}

/// A tensor the view takes: `(component, key, source, source component, source key)`; source 0
/// is the base, `i + 1` adapter `i`.
type Take = (String, String, usize, String, String);

/// What the view takes, in execution order, and its graph, refusing unsupported keys before
/// anything is written.
fn plan(base: &Checkpoint, adapters: &[(Selection, Checkpoint)]) -> io::Result<(Vec<Take>, Graph, Vec<Vec<String>>)> {
    if base.components().iter().any(|c| c.assets.contains_key(GRAPH_FILE)) {
        return Err(refuse("nested", "select the original base and one complete ordered adapter stack"));
    }
    let header = base.header();
    let mut renamed: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut grafts: BTreeMap<String, Vec<Take>> = BTreeMap::new();
    let mut graph = Graph { format: FORMAT, layers: vec![], adapters: vec![] };
    // The base's verified family, and each stored component's family component (tfs-137).
    let family = tensorfs::family::Record::read(&header.metadata);
    let mut warnings = vec![];
    for (index, (selection, adapter)) in adapters.iter().enumerate() {
        let declared = tensorfs::family::Adapter::read_all(&adapter.header().metadata);
        let trained_on = |name: &str| declared.get(name).and_then(|a| a.trained_on.as_ref()).map(|t| t.component.as_str());
        let mut said = vec![];
        if !selection.strength.is_finite() {
            return Err(refuse("scale", "adapter strength must be finite"));
        }
        // What it adapts, (factor component, base component): the components the selection
        // names; else, with only a factor component named, the base component its layers fit;
        // else every factor component, onto the base component named like it (an SDXL LoRA names
        // its components after the base's) that the call runs, else the one its layers fit.
        let components = adapter.header().components();
        let named = |name: &str| adapter.header().component(name);
        let absent = || {
            let has: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
            refuse("components", format!("the adapter has no factor component for this selection; it has: {}", has.join(", ")))
        };
        let runs = |name: &str| selection.runs.is_empty() || selection.runs.iter().any(|r| r == name);
        let targets: Vec<(&Component, String)> = match (selection.component.is_empty(), selection.source_component.is_empty()) {
            (false, true) => {
                let found = (named(&selection.component).or_else(|| named("adapter")))
                    .or_else(|| (components.len() == 1).then(|| &components[0]));
                vec![(found.ok_or_else(absent)?, selection.component.clone())]
            }
            (false, false) => vec![(named(&selection.source_component).ok_or_else(absent)?, selection.component.clone())],
            (true, false) => {
                let found = named(&selection.source_component).ok_or_else(absent)?;
                vec![(found, infer_component(header, &selection.runs, &keyed(found), trained_on(&found.name))?)]
            }
            (true, true) => (components.iter())
                .map(|found| match runs(&found.name) && header.component(&found.name).is_some() {
                    true => Ok((found, found.name.clone())),
                    false => Ok((found, infer_component(header, &selection.runs, &keyed(found), trained_on(&found.name))?)),
                })
                .collect::<io::Result<_>>()?,
        };
        if targets.is_empty() {
            return Err(refuse("empty", "adapter contains no factors"));
        }
        for (found, component) in targets {
            let (source_component, factors) = (&found.name, &keyed(found));
            // A LoRA of another family is refused before any layer is read; one trained on another
            // component of its family than it adapts is applied, with a warning on the run.
            let applied = family.as_ref().map_or(component.as_str(), |f| f.component(&component));
            if let (Some(lora), Some(base)) = (declared.get(source_component.as_str()), &family) {
                if lora.family != base.name {
                    return Err(refuse("family", format!("a {} LoRA, and the base model is {}", lora.family, base.name)));
                }
            }
            if let Some(t) = declared.get(source_component.as_str()).and_then(|a| a.trained_on.as_ref()).filter(|t| t.component != applied) {
                let slot = if applied == component { String::new() } else { format!(" ({applied})") };
                said.push(format!("trained on {} ({}), applied to {component}{slot}", t.component, t.from));
            }
            // PEFT's adapter_config, the LoRA component's config: α where a module has no `.alpha`.
            let lora_alpha = found.config.get("lora_alpha").and_then(serde_json::Value::as_f64);
            if factors.is_empty() {
                return Err(refuse("empty", "adapter contains no factors"));
            }
            let base_tensors = &keyed(
                header.component(&component).ok_or_else(|| refuse("component", format!("base has no component {component:?}")))?,
            );
            let mut scalars = BTreeMap::new();
            for (key, tensor) in factors {
                match (validate(adapter, source_component, key, tensor)?, key.strip_suffix(".alpha")) {
                    (Some(value), Some(path)) => drop(scalars.insert(path.to_string(), value)),
                    (None, Some(_)) => return Err(refuse("alpha", format!("{key}: alpha must be a scalar"))),
                    _ => {}
                }
            }
            let alias = format!("adapter_{}", graph.adapters.len());
            graph.adapters.push(AdapterRef {
                reference: selection.checkpoint.id(),
                scale: selection.strength,
                kind: "lora",
                component: component.clone(),
                source_component: source_component.clone(),
                family: "",
            });
            for (path, pair) in pairs(factors)? {
                let (Some((a_key, a)), Some((b_key, b))) = (pair.get("a"), pair.get("b")) else {
                    return Err(refuse("pair", format!("{component}.{path}: incomplete A/B pair")));
                };
                let Some(weight) = base_tensors.get(&format!("{path}.weight")) else {
                    return Err(refuse("target", format!("{component}.{path}: the base has no such layer")));
                };
                if !matches!(weight.shape.len(), 2 | 4) {
                    return Err(refuse("target_type", format!("{component}.{path}: only linear and 2-D convolution LoRA targets are supported")));
                }
                if !(matches!(weight.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) && a.dtype == b.dtype && fits(a, b, weight)) {
                    return Err(refuse("shape", format!("{component}.{path}: factors do not fit the base projection")));
                }
                let rank = a.shape[0];
                let alpha = scalars.get(&path).copied().or(lora_alpha).unwrap_or(rank as f64);
                if !(alpha / rank as f64 * selection.strength).is_finite() {
                    return Err(refuse("scale", format!("{component}.{path}: effective scale is not finite")));
                }
                if selection.strength == 0.0 {
                    continue;
                }
                for suffix in ["weight", "bias"] {
                    let key = format!("{path}.{suffix}");
                    if base_tensors.contains_key(&key) {
                        renamed.entry((component.clone(), key)).or_insert(format!("{path}.base_layer.{suffix}"));
                    }
                }
                for (role, key) in [("A", a_key), ("B", b_key)] {
                    let grafted = format!("{path}.lora_{role}.{alias}.weight");
                    let take = (component.clone(), grafted, index + 1, source_component.clone(), key.clone());
                    grafts.entry(component.clone()).or_default().push(take);
                }
                graph.layers.push(Layer {
                    component: component.clone(),
                    target: path.clone(),
                    adapter: alias.clone(),
                    rank,
                    alpha,
                    strength: selection.strength,
                    dtype: a.dtype.name().into(),
                });
            }
        }
        warnings.push(said);
    }
    let mut takes = vec![];
    for component in header.components() {
        let (name, mut renames) = (&component.name, vec![]);
        for key in component.tensors.iter().map(|t| &t.key) {
            match renamed.get(&(name.clone(), key.clone())) {
                Some(new) => renames.push((name.clone(), new.clone(), 0, name.clone(), key.clone())),
                None => takes.push((name.clone(), key.clone(), 0, name.clone(), key.clone())),
            }
        }
        takes.extend(renames);
        takes.extend(grafts.remove(name).unwrap_or_default());
    }
    Ok((takes, graph, warnings))
}

/// The adapter view of `base` with `selections`, composed by reference (the same bytes, so the
/// same id, each time) and kept in this process's memory, never the store.
pub fn compose(store: &Store, base: &ObjectRef, selections: &[Selection]) -> io::Result<Composed> {
    let checkpoint = |header: &ObjectRef| store.checkpoint(header).map_err(storage::io_error);
    let base_ref = base;
    let base = checkpoint(base)?;
    let adapters = selections.iter().map(|s| Ok((s.clone(), checkpoint(&s.checkpoint)?))).collect::<io::Result<Vec<_>>>()?;
    let (takes, graph, warnings) = plan(&base, &adapters)?;
    let each = (selections.iter())
        .map(|s| {
            let id = s.checkpoint.id();
            (graph.adapters.iter()).filter(|a| a.reference == id).map(|a| a.component.clone()).collect()
        })
        .collect();
    // Named by what it applies, the components as resolved.
    let rows: Vec<_> = (graph.adapters.iter())
        .map(|a| serde_json::json!([&a.reference, a.component, a.source_component, a.scale.to_string()]))
        .collect();
    let identity = tensorfs::sha256::hex_digest(
        &serde_json_canonicalizer::to_vec(&serde_json::json!([FORMAT, base_ref.id(), rows])).map_err(io::Error::other)?,
    );
    let id = format!("view-{}", &identity[..40]);
    let adapted: BTreeSet<&str> = graph.adapters.iter().map(|a| a.component.as_str()).collect();
    let graph = serde_json_canonicalizer::to_vec(&graph).map_err(io::Error::other)?;
    let sources: Vec<&Header> = std::iter::once(base.header()).chain(adapters.iter().map(|(_, a)| a.header())).collect();
    let header = by_reference(&sources, &takes, &adapted, ObjectRef::of(&graph))?.to_cbor().map_err(io::Error::other)?;
    let recipe = Recipe {
        base: base_ref.id(),
        selections: (selections.iter())
            .map(|s| Chosen {
                checkpoint: s.checkpoint.id(),
                component: s.component.clone(),
                source_component: s.source_component.clone(),
                strength: s.strength,
                runs: s.runs.clone(),
            })
            .collect(),
    };
    let checkpoint = storage::keep_view(store, header, vec![graph], recipe)?;
    Ok(Composed { name: id, checkpoint, adapted: each, warnings })
}

/// The view's header: each take's tensor as its source header has it, renamed, its parts in the
/// source's own chunks. Every source's chunks that a take touches keep their order (a range runs
/// on into the next chunk); no byte is read. The base's components, its pipeline and metadata
/// (less the recipe that wrote its lane), and `graph` as each adapted component's asset.
fn by_reference(sources: &[&Header], takes: &[Take], adapted: &BTreeSet<&str>, graph: ObjectRef) -> io::Result<Header> {
    let base = sources[0];
    let found = |(component, key, source, from_component, from_key): &Take| -> io::Result<Tensor> {
        let tensor = sources[*source].tensor(from_component, from_key)
            .ok_or_else(|| io::Error::other(format!("{component}.{key}: its source has no {from_component}.{from_key}")))?;
        Ok(Tensor { key: key.clone(), ..tensor.clone() })
    };
    let tensors = takes.iter().map(found).collect::<io::Result<Vec<_>>>()?;
    let mut touched = vec![BTreeSet::new(); sources.len()];
    for (take, tensor) in takes.iter().zip(&tensors) {
        for (_, at) in tensor.locations() {
            touched[take.2].extend(sources[take.2].spans(&at).into_iter().map(|s| s.chunk as u64));
        }
    }
    let (mut chunks, mut index) = (vec![], vec![BTreeMap::new(); sources.len()]);
    for (source, used) in touched.iter().enumerate() {
        for chunk in used {
            index[source].insert(*chunk, chunks.len() as u64);
            chunks.push(sources[source].chunks[*chunk as usize].clone());
        }
    }
    let mut by_component: BTreeMap<&str, Vec<Tensor>> = BTreeMap::new();
    for (take, mut tensor) in takes.iter().zip(tensors) {
        // An empty range touches no chunk and keeps any index.
        let moved = |chunk: u64| index[take.2].get(&chunk).copied().unwrap_or_default();
        match &mut tensor.body {
            Body::Data(at) => at.chunk = moved(at.chunk),
            Body::Encoded { parts, .. } => parts.iter_mut().for_each(|p| p.chunk = moved(p.chunk)),
        }
        by_component.entry(&take.0).or_default().push(tensor);
    }
    let components = (base.components().iter())
        .map(|c| {
            let mut component = Component { tensors: by_component.remove(c.name.as_str()).unwrap_or_default(), ..c.clone() };
            if adapted.contains(c.name.as_str()) {
                component.assets.insert(GRAPH_FILE.into(), graph.clone());
            }
            component
        })
        .collect();
    // A composition is not the lane its base's recipe wrote.
    let mut metadata = base.metadata.clone();
    metadata.remove(tensorfs::format::Provenance::KEY);
    Header::new(chunks, base.pipeline().cloned(), components, metadata).map_err(io::Error::other)
}
