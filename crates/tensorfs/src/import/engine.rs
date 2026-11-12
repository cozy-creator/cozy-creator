//! The one converter engine. A family's layout is a data table (`tensorfs/converters/`),
//! generated from upstream's own converter; this file recognizes a source by its tables' key
//! sets and applies a table row by row: `rename | view -> take(axis, idx) | permute -> shape`,
//! then the component's `cast`. A quantized carrier, folded into one encoded tensor (`carriers`),
//! is only renamed. Nothing model-specific lives here.

use platform::durable::FileAt;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;

use serde::Deserialize;

use crate::err::{refuse, Code, Refusal, Result};
use crate::format::{Dtype, Encoding};

use super::source::{Group, SourceSet, Tensor};

macro_rules! table {
    ($name:literal) => {
        include_str!(concat!("../../converters/", $name, ".json"))
    };
}

/// The built-in tables.
pub(crate) const TABLES: [&str; 22] = [
    table!("sdxl.single_file"),
    table!("anima.single_file"),
    table!("qwenimage21.comfy"),
    table!("krea2.native"),
    table!("krea2.comfy"),
    table!("h3.native"),
    table!("h3.comfy"),
    table!("h3.lora"),
    table!("sdxl.lora"),
    table!("krea2.lora"),
    table!("qwenimage21.lora"),
    table!("flux2.lora"),
    table!("flux3klein.lora"),
    table!("fibo2.lora"),
    table!("moss.lora"),
    table!("anima.lora"),
    table!("qwenimage.lora"),
    table!("seedvr2"),
    table!("h3.latent_upscaler"),
    table!("relayvsr"),
    table!("ideogram4.fp8"),
    table!("qwen3.6-35b-a3b.experts"),
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Table {
    pub name: String,
    /// A pinned repository (`hf://org/repo@<commit>`) whose index, configs and tokenizers the
    /// output takes when its source carries none.
    #[serde(default)]
    pub reference: Option<String>,
    /// A layout whose headers match another's byte for byte (H3's fused Q/K/V rows: contiguous
    /// thirds or head-interleaved) is chosen without being named only for a source that declares
    /// one of these producers, whose own code writes it. Absent: its key set alone chooses it.
    #[serde(default)]
    pub producers: Option<Vec<Producer>>,
    /// A LoRA table: rows are modules, any non-empty subset may be present. It claims a file most
    /// of whose keys it knows; any other key refuses the conversion, but for those a component
    /// leaves out by name (`optional`), which are said (`plan`'s `ignored`).
    #[serde(default)]
    pub lora: Option<Lora>,
    /// Documents the output carries, by path.
    #[serde(default)]
    pub files: BTreeMap<String, serde_json::Value>,
    /// The source's `config.json` stays its component's config: the table only re-lays the
    /// tensors of the layout that config builds (Transformers' fused experts, one per expert).
    #[serde(default)]
    pub keep_config: bool,
    /// Fixed tensors the layout needs and no source holds, by name (a row's `table`).
    #[serde(default)]
    pub constants: BTreeMap<String, Constant>,
    pub components: Vec<Component>,
}

/// A trainer or converter, by what its own code writes into a file: the key spelling and the
/// `__metadata__` fields.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Producer {
    pub name: String,
    /// Most of a source's keys start with one of these.
    pub keys: Vec<String>,
    pub metadata: BTreeMap<String, Match>,
    /// The pinned code (or, without public code, the declaration) the layout is read from: for people.
    #[serde(rename = "cite")]
    _cite: Vec<String>,
}

/// A `__metadata__` value a producer writes.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Match {
    Is(String),
    Prefix { prefix: String },
    /// A JSON object holding these fields (ai-toolkit's `software`).
    Json { json: BTreeMap<String, serde_json::Value> },
}

impl Producer {
    /// Whether `group` is this producer's file.
    pub(crate) fn wrote(&self, group: &Group) -> bool {
        let its = (group.tensors.keys()).filter(|k| self.keys.iter().any(|p| k.starts_with(p.as_str()))).count();
        its * 2 > group.tensors.len() && self.metadata.iter().all(|(field, want)| {
            let Some(value) = group.metadata.get(field) else { return false };
            match want {
                Match::Is(v) => value == v,
                Match::Prefix { prefix } => value.starts_with(prefix.as_str()),
                Match::Json { json } => serde_json::from_str::<BTreeMap<String, serde_json::Value>>(value)
                    .is_ok_and(|got| json.iter().all(|(k, v)| got.get(k) == Some(v))),
            }
        })
    }
}

/// A fixed tensor stated in the table: its bytes (base64) and their digest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Constant {
    pub dtype: Dtype,
    pub shape: Vec<u64>,
    pub sha256: String,
    pub base64: String,
}

/// Source key suffixes of each factor; the output spells them PEFT's way. `magnitude` is a DoRA's
/// per-output-channel magnitude (`[out]` or `[out, 1]`), stored as `<module>.lora_magnitude_vector` `[out]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lora {
    pub a: Vec<String>,
    pub b: Vec<String>,
    pub alpha: Vec<String>,
    pub magnitude: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Component {
    pub name: String,
    /// The source keys this component owns in a shared file (longest prefix wins).
    #[serde(default)]
    pub prefix: String,
    /// Output names by source folder suffix, when one layout lands under several names.
    #[serde(default)]
    pub folders: BTreeMap<String, String>,
    #[serde(default)]
    pub cast: Option<Dtype>,
    /// The source file's `__metadata__` field whose JSON object is this component's config
    /// (RelayVSR writes its model config there).
    #[serde(default)]
    pub config_from: Option<String>,
    /// Source keys every matching source has and the output does not take.
    #[serde(default)]
    pub drop: Vec<String>,
    /// Source keys a source may have; never taken. A LoRA table's are left out by name and said.
    #[serde(default)]
    pub optional: Vec<String>,
    /// Shape preconditions on source keys.
    #[serde(default)]
    pub expect: BTreeMap<String, Vec<u64>>,
    #[serde(deserialize_with = "rows")]
    pub rows: Vec<Row>,
    /// A LoRA table's plain tensors, renamed beside the factors: a parallel decoder's per-step
    /// output heads (PDD's `proj_out.weight`), `[steps, out, in]` for a row's `io` `[in, out]`.
    #[serde(default, deserialize_with = "rows")]
    pub keep: Vec<Row>,
}

/// A component's rows. `[target, source, ops, {"e": n}]` stands for n rows, `e` = 0..n: `{e}`
/// in its strings is the number, and so is a `"{e}"` value (one row per expert of a fused
/// tensor: `{"take": [0, ["{e}"]]}`).
fn rows<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<Row>, D::Error> {
    use serde::de::Error;
    let mut out = Vec::new();
    for value in Vec::<serde_json::Value>::deserialize(d)? {
        let serde_json::Value::Array(parts) = &value else {
            out.push(Row::deserialize(value).map_err(D::Error::custom)?);
            continue;
        };
        let [target, source, ops, each] = parts.as_slice() else {
            out.push(Row::deserialize(value).map_err(D::Error::custom)?);
            continue;
        };
        let each: BTreeMap<String, u64> =
            serde_json::from_value(each.clone()).map_err(D::Error::custom)?;
        let mut texts = vec![serde_json::json!([target, source, ops]).to_string()];
        for (name, n) in &each {
            let mark = format!("{{{name}}}");
            let value = format!("\"{mark}\"");
            texts = (texts.iter())
                .flat_map(|t| {
                    (0..*n).map(|i| {
                        t.replace(&value, &i.to_string())
                            .replace(&mark, &i.to_string())
                    })
                })
                .collect();
        }
        for text in texts {
            out.push(serde_json::from_str(&text).map_err(D::Error::custom)?);
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
#[serde(from = "RowRepr")]
pub(crate) struct Row {
    pub target: String,
    pub source: Src,
    pub ops: Ops,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RowRepr {
    Same((String,)),
    Rename((String, Src)),
    Op((String, Src, Ops)),
}

impl From<RowRepr> for Row {
    fn from(r: RowRepr) -> Row {
        let (target, source, ops) = match r {
            RowRepr::Same((k,)) => (k.clone(), Src::Key(k), Ops::default()),
            RowRepr::Rename((t, s)) => (t, s, Ops::default()),
            RowRepr::Op((t, s, o)) => (t, s, o),
        };
        Row {
            target,
            source,
            ops,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Src {
    Key(String),
    /// A byte range of a file the source set holds, found by its digest (no parser runs).
    Raw(Raw),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Raw {
    pub sha256: String,
    pub length: u64,
    pub offset: u64,
    pub dtype: Dtype,
    pub shape: Vec<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ops {
    #[serde(default)]
    pub view: Option<Vec<i64>>,
    #[serde(default)]
    pub take: Option<(usize, Vec<u64>)>,
    #[serde(default)]
    pub permute: Option<Vec<usize>>,
    #[serde(default)]
    pub shape: Option<Vec<i64>>,
    /// LoRA rows: the base module's `[in, out]`, so A is `[r, in]` and B `[out, r]`.
    #[serde(default)]
    pub io: Option<(u64, u64)>,
    /// LoRA rows of a convolution: its kernel, so A is `[r, in, kh, kw]` and B `[out, r, 1, 1]`.
    #[serde(default)]
    pub kernel: Vec<u64>,
    /// LoRA rows: the ops applied to B.
    #[serde(default)]
    pub b: Option<Box<Ops>>,
    /// LoRA rows whose input is a fixed table, not the module's input (H3's AdaLN curve): the
    /// table's constant, written beside the factors as `<target>.table`.
    #[serde(default)]
    pub table: Option<String>,
}

/// Every built-in table, parsed once.
pub(crate) fn tables() -> &'static [Table] {
    static PARSED: std::sync::OnceLock<Vec<Table>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        TABLES
            .iter()
            .map(|text| serde_json::from_str(text).expect("a built-in converter table"))
            .collect()
    })
}

/// The built-in tables' digest: an import is what its source's bytes and these tables make.
pub fn tables_digest() -> &'static str {
    static DIGEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIGEST.get_or_init(|| crate::sha256::hex_digest(TABLES.join("\0").as_bytes()))
}

// ------------------------------------------------------------------------------ recognition

/// One table component matched to one source group.
#[derive(Debug, Clone)]
pub(crate) struct Claim {
    pub table: usize,
    pub component: usize,
    pub group: usize,
    /// The output component's name.
    pub name: String,
}

#[derive(Debug, Default)]
pub(crate) struct Recognized {
    pub claims: Vec<Claim>,
    /// Table indices used, in order.
    pub tables: Vec<usize>,
    /// For a table with [`Table::producers`] that recognition chose, the producer that let it.
    pub producers: BTreeMap<usize, String>,
    /// Groups no table claims.
    pub unclaimed: Vec<usize>,
}

impl Recognized {
    /// The tables used, each with the producer that chose it: `h3.lora (musubi-tuner)`.
    pub(crate) fn formats(&self, tables: &[Table]) -> Vec<String> {
        (self.tables.iter())
            .map(|t| match self.producers.get(t) {
                Some(p) => format!("{} ({p})", tables[*t].name),
                None => tables[*t].name.clone(),
            })
            .collect()
    }
}

/// The format a caller names to keep a source's keys as they are, for code that reads them raw.
pub(crate) const AS_IS: &str = "as-is";

fn lora_split<'k>(lora: &Lora, key: &'k str) -> Option<(&'k str, Factor)> {
    let mut best: Option<(&str, Factor)> = None;
    for (suffixes, factor) in [
        (&lora.a, Factor::A),
        (&lora.b, Factor::B),
        (&lora.alpha, Factor::Alpha),
        (&lora.magnitude, Factor::Magnitude),
    ] {
        for s in suffixes {
            if let Some(stem) = key.strip_suffix(s.as_str()) {
                if best.is_none_or(|(b, _)| stem.len() < b.len()) {
                    best = Some((stem, factor));
                }
            }
        }
    }
    best.or_else(|| LOKR.iter().find_map(|(suffix, factor)| key.strip_suffix(suffix).map(|stem| (stem, *factor))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Factor {
    A,
    B,
    Alpha,
    Magnitude,
    /// A LoKr's (LyCORIS' Kronecker adapter, `ΔW = kron(W1, W2)`) factors, each whole or an `_a @ _b` pair.
    W1,
    W2,
    W1A,
    W1B,
    W2A,
    W2B,
}

/// LyCORIS' LoKr factor names, the same under every table's prefixes and module names; stored as they are.
const LOKR: [(&str, Factor); 6] = [
    (".lokr_w1", Factor::W1),
    (".lokr_w2", Factor::W2),
    (".lokr_w1_a", Factor::W1A),
    (".lokr_w1_b", Factor::W1B),
    (".lokr_w2_a", Factor::W2A),
    (".lokr_w2_b", Factor::W2B),
];

/// Whether `key`, past `c`'s prefix, is a factor of one of its modules shaped for it (A
/// `[r, in, kernel…]`, B `[out, r, 1…]` as stored, a scalar alpha, a magnitude `[out]`, `[out, 1]`
/// or another axis's `[1, in]`), or a tensor it keeps. By shape, so families whose module names
/// coincide (H3's and Qwen-Image 2.1's Diffusers `transformer_blocks.*.attn.to_q`) claim only their own files.
fn lora_module(lora: &Lora, c: &Component, key: &str, t: &Tensor) -> bool {
    if let Some(r) = c.keep.iter().find(|r| r.key() == Some(key)) {
        return kept(r, t);
    }
    let Some((stem, factor)) = lora_split(lora, key) else { return false };
    let shaped = |r: &Row| {
        let (Some((input, output)), kernel, s) = (r.ops.io, &r.ops.kernel, &t.shape) else { return true };
        let plain = s.len() == 2 + kernel.len();
        match factor {
            Factor::A => plain && s[0] > 0 && s[1] == input && s[2..] == kernel[..],
            Factor::B => plain && s[0] == output && s[1] > 0 && s[2..].iter().all(|k| *k == 1),
            Factor::Alpha => s.is_empty(),
            Factor::Magnitude => matches!(s.as_slice(), [o] | [o, 1] if *o == output) || s.as_slice() == [1, input],
            // A Kronecker factor's rows divide the module's output and its columns its input; the pair is checked when planned.
            Factor::W1 | Factor::W2 => kernel.is_empty() && matches!(s.as_slice(), [rows, cols] if *rows > 0 && *cols > 0 && output % rows == 0 && input % cols == 0),
            Factor::W1A | Factor::W2A => kernel.is_empty() && matches!(s.as_slice(), [rows, rank] if *rows > 0 && *rank > 0 && output % rows == 0),
            Factor::W1B | Factor::W2B => kernel.is_empty() && matches!(s.as_slice(), [rank, cols] if *rank > 0 && *cols > 0 && input % cols == 0),
        }
    };
    c.rows.iter().any(|r| r.key() == Some(stem) && shaped(r))
}

/// Whether `t` is the tensor keep row `r` names: `[steps, out, in]` for its `io`.
fn kept(r: &Row, t: &Tensor) -> bool {
    r.ops.io.is_none_or(|(input, output)| t.shape.len() == 3 && t.shape[1..] == [output, input])
}

/// The component of LoRA `table` that owns `key`: the longest prefix.
fn owner<'t>(table: &'t Table, key: &str) -> Option<&'t Component> {
    (table.components.iter()).filter(|c| key.starts_with(c.prefix.as_str())).max_by_key(|c| c.prefix.len())
}

/// The keys of `group` no module of LoRA `table` holds, and those it leaves out by name (`optional`).
fn lora_strays<'g>(table: &Table, lora: &Lora, group: &'g Group) -> (Vec<&'g str>, Vec<&'g str>) {
    let (mut strays, mut ignored) = (vec![], vec![]);
    for (key, t) in &group.tensors {
        match owner(table, key) {
            Some(c) if lora_module(lora, c, &key[c.prefix.len()..], t) => {}
            Some(c) if c.optional.iter().any(|o| *o == key[c.prefix.len()..]) => ignored.push(key.as_str()),
            _ => strays.push(key.as_str()),
        }
    }
    (strays, ignored)
}

/// Why one bucket of keys is not one component's layout: unknown and missing keys.
fn mismatch(
    table: &Table,
    c: &Component,
    keys: &BTreeSet<&str>,
    group: &Group,
) -> Option<(Vec<String>, Vec<String>)> {
    if let Some(lora) = &table.lora {
        let known = |k: &str| lora_module(lora, c, k, &group.tensors[&format!("{}{k}", c.prefix)]) || c.optional.iter().any(|o| o == k);
        let unknown: Vec<String> = keys.iter().filter(|k| !known(k)).map(|k| k.to_string()).collect();
        return (unknown.len() == keys.len()).then_some((unknown, vec![]));
    }
    let required: BTreeSet<&str> = c
        .rows
        .iter()
        .filter_map(|r| r.key())
        .chain(c.drop.iter().map(String::as_str))
        .collect();
    let unknown: Vec<String> = keys
        .iter()
        .filter(|k| !required.contains(**k) && !c.optional.iter().any(|o| o == **k))
        .map(|k| k.to_string())
        .collect();
    let missing: Vec<String> = required
        .iter()
        .filter(|k| !keys.contains(**k))
        .map(|k| k.to_string())
        .collect();
    (!unknown.is_empty() || !missing.is_empty()).then_some((unknown, missing))
}

impl Row {
    fn key(&self) -> Option<&str> {
        match &self.source {
            Src::Key(k) => Some(k),
            Src::Raw(_) => None,
        }
    }
}

/// How `table` projects `group`: each prefix bucket of its keys, matched to the one
/// component whose layout it is. `Err` names the closest mismatch.
fn project(table: &Table, group: &Group) -> std::result::Result<Vec<usize>, String> {
    if let Some(lora) = &table.lora {
        let (strays, _) = lora_strays(table, lora, group);
        if strays.len() * 2 >= group.tensors.len() {
            return Err(format!("{} of {} keys are no module's", strays.len(), group.tensors.len()));
        }
    }
    let prefixes: BTreeSet<&str> = table
        .components
        .iter()
        .filter(|c| c.rows.iter().any(|r| r.key().is_some()))
        .map(|c| c.prefix.as_str())
        .collect();
    let mut buckets: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for key in group.tensors.keys() {
        let Some(prefix) = prefixes
            .iter()
            .filter(|p| key.starts_with(**p))
            .max_by_key(|p| p.len())
        else {
            if table.lora.is_some() {
                continue;
            }
            return Err(format!("{key:?} is under no component prefix"));
        };
        buckets
            .entry(prefix)
            .or_default()
            .insert(&key[prefix.len()..]);
    }
    let mut out = Vec::new();
    for (prefix, keys) in &buckets {
        let mut closest = (usize::MAX, String::new());
        let fits: Vec<usize> = (0..table.components.len())
            .filter(|i| {
                let c = &table.components[*i];
                if c.prefix != *prefix {
                    return false;
                }
                match mismatch(table, c, keys, group) {
                    None => true,
                    Some((unknown, missing)) => {
                        if unknown.len() + missing.len() < closest.0 {
                            let detail = format!(
                                "{}: {} unknown {:?}, {} missing {:?}",
                                c.name,
                                unknown.len(),
                                &unknown[..unknown.len().min(3)],
                                missing.len(),
                                &missing[..missing.len().min(3)]
                            );
                            closest = (unknown.len() + missing.len(), detail);
                        }
                        false
                    }
                }
            })
            .collect();
        match fits.as_slice() {
            [one] => out.push(*one),
            [] => return Err(closest.1),
            _ => {
                return Err(format!(
                    "keys under {prefix:?} fit {} components",
                    fits.len()
                ))
            }
        }
    }
    Ok(out)
}

fn output_name(c: &Component, folder: &str) -> String {
    c.folders
        .iter()
        .find(|(suffix, _)| folder == suffix.as_str() || folder.ends_with(&format!("/{suffix}")))
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| c.name.clone())
}

/// Which tables claim which groups. `named` restricts recognition to that table and makes
/// a mismatch a refusal ([`AS_IS`]: to none); otherwise an unmatched group is unclaimed.
pub(crate) fn recognize(set: &SourceSet, tables: &[Table], named: Option<&str>) -> Result<Recognized> {
    if named == Some(AS_IS) {
        let unclaimed = (0..set.groups.len()).collect();
        return Ok(Recognized { unclaimed, ..Recognized::default() });
    }
    let candidates: Vec<usize> = match named {
        Some(name) => match tables.iter().position(|t| t.name == name) {
            Some(i) => vec![i],
            None => {
                let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
                return refuse(
                    Code::UNKNOWN_FIELD,
                    format!("no converter {name:?}; built in: {names:?}"),
                );
            }
        },
        None => (0..tables.len()).collect(),
    };
    let mut out = Recognized::default();
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    for t in candidates {
        let table = &tables[t];
        let mut claims = Vec::new();
        let mut why = Vec::new();
        for (g, group) in set.groups.iter().enumerate() {
            match project(table, group) {
                Ok(components) => claims.extend(components.into_iter().map(|c| Claim {
                    table: t,
                    component: c,
                    group: g,
                    name: output_name(&table.components[c], &group.folder),
                })),
                Err(e) => why.push(format!("{}: {e}", group.label())),
            }
        }
        let keyed = |c: &Component| c.rows.iter().any(|r| r.key().is_some());
        let complete = match table.lora {
            Some(_) => !claims.is_empty(),
            None => table
                .components
                .iter()
                .enumerate()
                .filter(|(_, c)| keyed(c))
                .all(|(i, _)| claims.iter().any(|claim| claim.component == i)),
        };
        let raw = table
            .components
            .iter()
            .all(|c| keyed(c) || raw_present(set, c));
        if !(complete && raw) {
            if named.is_some() {
                return refuse(
                    Code::MISSING_TENSOR,
                    format!(
                        "the source is not the {} layout: {}",
                        table.name,
                        why.join("; ")
                    ),
                );
            }
            continue;
        }
        if let (Some(producers), None) = (&table.producers, named) {
            let groups: BTreeSet<usize> = claims.iter().map(|c| c.group).collect();
            let wrote = |p: &&Producer| groups.iter().all(|g| p.wrote(&set.groups[*g]));
            match producers.iter().find(wrote) {
                Some(p) => drop(out.producers.insert(t, p.name.clone())),
                None => {
                    let known: BTreeSet<&str> = producers.iter().map(|p| p.name.as_str()).collect();
                    return refuse(
                        Code::AMBIGUOUS_CLASSIFICATION,
                        format!(
                            "the source matches {0}, whose headers are identical to other layouts', and \
                             declares none of its known producers {known:?}; name it to convert \
                             (--source-format {0}), or keep it unconverted (--source-format {AS_IS})",
                            table.name
                        ),
                    );
                }
            }
        }
        let mut names = BTreeSet::new();
        for claim in &claims {
            if let Some(other) = owner.insert(claim.group, t).filter(|o| *o != t) {
                return refuse(
                    Code::AMBIGUOUS_CLASSIFICATION,
                    format!(
                        "{} matches both {} and {}",
                        set.groups[claim.group].label(),
                        tables[other].name,
                        table.name
                    ),
                );
            }
            if !names.insert(claim.name.clone()) && table.lora.is_none() {
                return refuse(
                    Code::DUPLICATE_KEY,
                    format!("{}: two sources for component {}", table.name, claim.name),
                );
            }
        }
        out.claims.extend(claims);
        out.tables.push(t);
    }
    out.unclaimed = (0..set.groups.len())
        .filter(|g| !owner.contains_key(g))
        .collect();
    Ok(out)
}

/// The family whose LoRA tables claim a LoRA's tensors, by key and shape as recognition does: one kept as its
/// source spelled it (stored before a table read it) says its family too. None when no family's do, or two's.
pub(crate) fn lora_family<'a>(tensors: impl Iterator<Item = (&'a str, &'a [u64])>) -> Option<&'static crate::family::Family> {
    let tensor = |shape: &[u64]| Tensor { dtype: Dtype::BF16, shape: shape.to_vec(), file: 0, start: 0, len: 0, encoded: None, derive: None };
    let tensors = tensors.map(|(key, shape)| (key.to_owned(), tensor(shape))).collect();
    let group = Group { folder: String::new(), stem: String::new(), tensors, metadata: BTreeMap::new() };
    let claim = |t: &&Table| t.lora.is_some() && project(t, &group).is_ok_and(|claimed| !claimed.is_empty());
    let names: BTreeSet<&str> = (tables().iter().filter(claim)).filter_map(|t| crate::family::of_table(&t.name)).map(|f| f.name.as_str()).collect();
    let mut names = names.into_iter();
    names.next().filter(|_| names.next().is_none()).and_then(crate::family::named)
}

/// The tables that know any of `group`'s keys, by how many, most first: `h3.lora (580 of 600
/// keys)`.
pub(crate) fn closest(group: &Group, tables: &[Table]) -> Vec<String> {
    let knows = |table: &Table, key: &str| {
        table.components.iter().any(|c| {
            let Some(rest) = key.strip_prefix(c.prefix.as_str()) else { return false };
            let known = |k: &str| c.rows.iter().any(|r| r.key() == Some(k));
            match &table.lora {
                Some(lora) => lora_split(lora, rest).is_some_and(|(stem, _)| known(stem)),
                None => known(rest) || c.drop.iter().chain(&c.optional).any(|k| k == rest),
            }
        })
    };
    let total = group.tensors.len();
    let mut near: Vec<(usize, &str)> = (tables.iter())
        .map(|t| (group.tensors.keys().filter(|k| knows(t, k)).count(), t.name.as_str()))
        .filter(|(n, _)| *n > 0)
        .collect();
    near.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    near.iter().take(3).map(|(n, name)| format!("{name} ({n} of {total} keys)")).collect()
}

fn raw_present(set: &SourceSet, c: &Component) -> bool {
    c.rows.iter().all(|r| match &r.source {
        Src::Raw(raw) => set.find_raw(&raw.sha256, raw.length).is_some(),
        Src::Key(_) => true,
    })
}

// ------------------------------------------------------------------------------ planning

/// One output tensor and how its bytes are read.
#[derive(Debug, Clone)]
pub(crate) struct Planned {
    pub component: String,
    pub key: String,
    pub dtype: Dtype,
    pub shape: Vec<u64>,
    pub encoding: Option<Encoding>,
    /// A plain tensor's one part (role `""`), or an encoded tensor's parts in order.
    pub parts: Vec<PlannedPart>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedPart {
    pub role: String,
    pub dtype: Dtype,
    pub shape: Vec<u64>,
    pub read: Reading,
}

#[derive(Debug, Clone)]
pub(crate) struct Reading {
    pub file: usize,
    /// The source tensor's first byte in `file`, and its length.
    pub start: u64,
    pub len: u64,
    /// Bytes per run unit: the element, or one byte for a tensor read as it is.
    pub unit: u64,
    pub runs: Runs,
    pub source: Dtype,
    pub cast: Option<Dtype>,
    /// The bytes themselves, for a part no source tensor holds (`FoldedPart::inline`).
    pub inline: Option<Vec<u8>>,
    /// The bytes computed from the source range (`Tensor::derive`).
    pub derive: Option<super::source::Derive>,
}

impl Reading {
    /// Bytes no source tensor holds, as they are (`file` is never read).
    pub(super) fn inline(bytes: Vec<u8>, source: Dtype) -> Reading {
        let len = bytes.len() as u64;
        Reading {
            file: 0,
            start: 0,
            len,
            unit: 1,
            runs: Runs {
                run: len,
                base: 0,
                outer: vec![],
            },
            source,
            cast: None,
            inline: Some(bytes),
            derive: None,
        }
    }

    /// A source range read as it is, or what its derive computes from it.
    fn whole(t: &Tensor) -> Reading {
        Reading {
            file: t.file,
            start: t.start,
            len: t.len,
            unit: 1,
            runs: Runs {
                run: t.derive.as_ref().map_or(t.len, |d| d.len),
                base: 0,
                outer: vec![],
            },
            source: t.dtype.clone(),
            cast: None,
            inline: None,
            derive: t.derive.clone(),
        }
    }
}

/// The output as contiguous source runs: `base + Σ index·stride` over the outer dims.
#[derive(Debug, Clone)]
pub(crate) struct Runs {
    pub run: u64,
    base: u64,
    outer: Vec<Dim>,
}

#[derive(Debug, Clone)]
struct Dim {
    size: u64,
    stride: u64,
    idx: Option<Vec<u64>>,
}

impl Runs {
    pub(crate) fn count(&self) -> u64 {
        self.outer.iter().map(|d| d.size).product()
    }

    /// Element offset of run `n`.
    pub(crate) fn offset(&self, mut n: u64) -> u64 {
        let mut at = self.base;
        for d in self.outer.iter().rev() {
            let i = n % d.size;
            n /= d.size;
            at += d.stride * d.idx.as_ref().map_or(i, |idx| idx[i as usize]);
        }
        at
    }
}

fn resolve_dims(what: &str, dims: &[i64], numel: u64) -> Result<Vec<u64>> {
    let known: u64 = dims.iter().filter(|d| **d > 0).map(|d| *d as u64).product();
    let free = dims.iter().filter(|d| **d == -1).count();
    if dims.iter().any(|d| *d == 0 || *d < -1) || free > 1 || known == 0 {
        return refuse(Code::SHAPE_MISMATCH, format!("{what}: dims {dims:?}"));
    }
    let fill = numel / known;
    if (free == 0 && known != numel) || (free == 1 && (!numel.is_multiple_of(known) || fill == 0)) {
        return refuse(
            Code::SHAPE_MISMATCH,
            format!("{what}: {numel} elements do not view as {dims:?}"),
        );
    }
    Ok(dims
        .iter()
        .map(|d| if *d == -1 { fill } else { *d as u64 })
        .collect())
}

/// The output shape and source runs of `ops` over a contiguous `shape`.
pub(crate) fn layout(what: &str, shape: &[u64], ops: &Ops) -> Result<(Vec<u64>, Runs)> {
    let numel: u64 = shape.iter().product();
    let dims = match &ops.view {
        Some(v) => resolve_dims(what, v, numel)?,
        None => shape.to_vec(),
    };
    let mut stride = 1;
    let mut layout: Vec<Dim> = dims
        .iter()
        .rev()
        .map(|&size| {
            let d = Dim {
                size,
                stride,
                idx: None,
            };
            stride *= size;
            d
        })
        .collect();
    layout.reverse();
    let mut base = 0;
    if let Some((axis, idx)) = &ops.take {
        let Some(d) = layout.get_mut(*axis) else {
            return refuse(
                Code::SHAPE_MISMATCH,
                format!("{what}: take axis {axis} of {dims:?}"),
            );
        };
        if idx.is_empty() || idx.iter().any(|i| *i >= d.size) {
            return refuse(
                Code::SHAPE_MISMATCH,
                format!("{what}: take {idx:?} of {}", d.size),
            );
        }
        if idx.windows(2).all(|w| w[1] == w[0] + 1) {
            base += idx[0] * d.stride;
        } else {
            d.idx = Some(idx.clone());
        }
        d.size = idx.len() as u64;
    }
    if let Some(p) = &ops.permute {
        let mut seen = p.clone();
        seen.sort();
        if seen != (0..layout.len()).collect::<Vec<_>>() {
            return refuse(
                Code::SHAPE_MISMATCH,
                format!("{what}: permute {p:?} of rank {}", layout.len()),
            );
        }
        layout = p.iter().map(|i| layout[*i].clone()).collect();
    }
    let taken: u64 = layout.iter().map(|d| d.size).product();
    let out = match &ops.shape {
        Some(s) => resolve_dims(what, s, taken)?,
        None => layout.iter().map(|d| d.size).collect(),
    };
    let mut run = 1;
    while let Some(d) = layout.last() {
        if d.idx.is_some() || d.stride != run {
            break;
        }
        run *= d.size;
        layout.pop();
    }
    Ok((
        out,
        Runs {
            run,
            base,
            outer: layout,
        },
    ))
}

/// The output dtype, and the narrowing that reaches it if any.
fn cast_to(what: &str, from: &Dtype, to: Option<&Dtype>) -> Result<(Dtype, Option<Dtype>)> {
    match to {
        None => Ok((from.clone(), None)),
        Some(to) if to == from => Ok((from.clone(), None)),
        Some(Dtype::F16) if matches!(from, Dtype::F32 | Dtype::BF16) => {
            Ok((Dtype::F16, Some(Dtype::F16)))
        }
        Some(to) => refuse(
            Code::DTYPE_MISMATCH,
            format!("{what}: no narrowing from {from} to {to}"),
        ),
    }
}

/// The source rows a row's `ops` select from a `[rows, cols]` tensor, when its output is those
/// whole rows (a fused projection's thirds), or why not.
fn selected_rows(what: &str, shape: &[u64], ops: &Ops) -> Result<Vec<u64>> {
    let [rows, cols] = shape else {
        return refuse(Code::SEAM_UNBANKED, format!("{what}: rank {} rows", shape.len()));
    };
    let (out, _) = layout(what, shape, ops)?;
    // The same ops over one element per row: each output element is the source row it takes.
    let (picked, runs) = layout(what, &[*rows, 1], ops)?;
    let selected: Vec<u64> = (0..runs.count())
        .flat_map(|r| (0..runs.run).map(move |i| (r, i)))
        .map(|(r, i)| runs.offset(r) + i)
        .collect();
    if ops.permute.is_some() || picked.last() != Some(&1) || out != [selected.len() as u64, *cols] {
        return refuse(
            Code::SEAM_UNBANKED,
            format!("{what}: {ops:?} of {shape:?} is not a selection of whole rows"),
        );
    }
    Ok(selected)
}

/// An encoded tensor's part cut to `rows` of its `count` rows: every part of a row-wise encoding
/// leads with the rows.
fn part_rows(
    what: &str,
    p: &super::carriers::FoldedPart,
    count: u64,
    rows: &[u64],
) -> Result<(Vec<u64>, Reading)> {
    let what = format!("{what} {}", p.role);
    let bits = p.at.dtype.bits().filter(|b| b.is_multiple_of(8) && p.dtype == p.at.dtype);
    let (Some(bits), None, None, Some(&count_here)) = (bits, &p.inline, &p.at.derive, p.shape.first()) else {
        return refuse(Code::SEAM_UNBANKED, format!("{what}: a part that is not whole rows of bytes"));
    };
    if count_here != count {
        return refuse(
            Code::SHAPE_MISMATCH,
            format!("{what}: {:?} does not lead with the tensor's {count} rows", p.shape),
        );
    }
    let take = Ops { take: Some((0, rows.to_vec())), ..Ops::default() };
    let (shape, runs) = layout(&what, &p.shape, &take)?;
    let read = Reading {
        file: p.at.file,
        start: p.at.start,
        len: p.at.len,
        unit: bits / 8,
        runs,
        source: p.at.dtype.clone(),
        cast: None,
        inline: None,
        derive: None,
    };
    Ok((shape, read))
}

/// `component/key` read from `t` through `ops` and `cast`. A tensor whose bytes do not
/// move is one run of bytes, so it may be any dtype. An encoded tensor is renamed, or, for a
/// row-wise encoding, cut into whole rows (each part's rows with them): its parts are the
/// producer's bytes, and `cast` names its logical dtype.
fn planned(
    component: &str,
    key: &str,
    t: &Tensor,
    ops: &Ops,
    cast: Option<&Dtype>,
) -> Result<Planned> {
    let what = format!("{component}/{key}");
    if let Some(folded) = &t.encoded {
        let cut = ops.view.is_some() || ops.take.is_some() || ops.permute.is_some() || ops.shape.is_some();
        let rows = match cut {
            false => None,
            true if folded.encoding.scheme.ends_with(".rowwise") || folded.encoding.scheme == "int8.convrot" => {
                Some(selected_rows(&what, &t.shape, ops)?)
            }
            true => {
                return refuse(
                    Code::SEAM_UNBANKED,
                    format!(
                        "{what}: a {} tensor; a table row may rename it, never cut or reshape it",
                        folded.encoding.scheme
                    ),
                )
            }
        };
        let mut parts = Vec::new();
        for p in &folded.parts {
            let (shape, read) = match &rows {
                None => (p.shape.clone(), Reading { inline: p.inline.clone(), ..Reading::whole(&p.at) }),
                Some(rows) => part_rows(&what, p, t.shape[0], rows)?,
            };
            parts.push(PlannedPart { role: p.role.clone(), dtype: p.dtype.clone(), shape, read });
        }
        let mut shape = t.shape.clone();
        if let Some(rows) = &rows {
            shape[0] = rows.len() as u64;
        }
        return Ok(Planned {
            component: component.into(),
            key: key.into(),
            dtype: cast.unwrap_or(&t.dtype).clone(),
            shape,
            encoding: Some(folded.encoding.clone()),
            parts,
        });
    }
    let (dtype, cast) = cast_to(&what, &t.dtype, cast)?;
    let (shape, runs) = layout(&what, &t.shape, ops)?;
    let moves = ops.view.is_some() || ops.take.is_some() || ops.permute.is_some();
    if t.derive.is_some() && (moves || cast.is_some()) {
        return refuse(Code::SEAM_UNBANKED, format!("{what}: a computed tensor; a table row may rename it, never cut or cast it"));
    }
    let read = match (moves || cast.is_some(), t.dtype.bits()) {
        (true, Some(bits)) if bits.is_multiple_of(8) => Reading {
            file: t.file,
            start: t.start,
            len: t.len,
            unit: bits / 8,
            runs,
            source: t.dtype.clone(),
            cast,
            inline: None,
            derive: None,
        },
        (true, _) => {
            return refuse(
                Code::DTYPE_MISMATCH,
                format!("{what}: {} elements are not whole bytes", t.dtype),
            )
        }
        (false, _) => Reading::whole(t),
    };
    Ok(Planned {
        component: component.into(),
        key: key.into(),
        dtype: dtype.clone(),
        shape: shape.clone(),
        encoding: None,
        parts: vec![PlannedPart {
            role: String::new(),
            dtype,
            shape,
            read,
        }],
    })
}

/// Every output tensor, in construction order: claimed groups through their tables' rows in
/// row order (the target's `state_dict()` order), then unclaimed groups as they are, under
/// `identity(group)`'s component name.
pub(crate) fn plan(
    set: &SourceSet,
    tables: &[Table],
    r: &Recognized,
    identity: &dyn Fn(&Group) -> String,
    ignored: &mut Vec<String>,
) -> Result<Vec<Planned>> {
    let mut out = Vec::new();
    for &t in &r.tables {
        let table = &tables[t];
        for (ci, c) in table.components.iter().enumerate() {
            let claims: Vec<&Claim> = r
                .claims
                .iter()
                .filter(|x| x.table == t && x.component == ci)
                .collect();
            if claims.is_empty() && c.rows.iter().all(|row| row.key().is_none()) {
                plan_raw(set, c, &mut out)?;
            }
            for claim in claims {
                let group = &set.groups[claim.group];
                let local = |k: &str| group.tensors.get(&format!("{}{k}", c.prefix));
                for (key, shape) in &c.expect {
                    if let Some(t) = local(key).filter(|t| &t.shape != shape) {
                        return refuse(
                            Code::SHAPE_MISMATCH,
                            format!(
                                "{}: {key} is {:?}, {} expects {shape:?}",
                                claim.name, t.shape, table.name
                            ),
                        );
                    }
                }
                match &table.lora {
                    Some(lora) => plan_lora(table, lora, c, claim, group, &mut out)?,
                    None => {
                        for row in &c.rows {
                            let Src::Key(k) = &row.source else { continue };
                            let t = local(k).expect("recognition checked the key set");
                            out.push(planned(
                                &claim.name,
                                &row.target,
                                t,
                                &row.ops,
                                c.cast.as_ref(),
                            )?);
                        }
                    }
                }
            }
        }
    }
    let mut told = BTreeSet::new();
    for claim in r.claims.iter().filter(|claim| told.insert(claim.group)) {
        let (table, group) = (&tables[claim.table], &set.groups[claim.group]);
        let Some(lora) = &table.lora else { continue };
        let (strays, left) = lora_strays(table, lora, group);
        if !strays.is_empty() {
            return refuse(Code::UNPLACED_KEYS, format!(
                "{}: {} of its {} keys are no module's of {}: {}. Keep it unconverted (--source-format {AS_IS}) to store every key as it is",
                group.label(), strays.len(), group.tensors.len(), table.name, named(&strays)));
        }
        if !left.is_empty() {
            ignored.push(format!("ignored: {}: {} keys {} leaves out by name: {}", group.label(), left.len(), table.name, named(&left)));
        }
    }
    // Unclaimed groups are stored as they are, in the construction order of the class their
    // config names; keys its reference lacks keep the order of their bytes in the source.
    for &g in &r.unclaimed {
        let group = &set.groups[g];
        let name = identity(group);
        let keys: Vec<(&str, (usize, u64))> = (group.tensors.iter())
            .map(|(k, t)| (k.as_str(), (t.file, t.start)))
            .collect();
        let class = set.class_of(group);
        let ordered = super::constructor_order(class.as_deref(), &keys);
        if !ordered.unknown.is_empty() {
            tracing::warn!(
                "{name}: {} of {} tensors have no constructor order for {}; they keep the source's order",
                ordered.unknown.len(),
                keys.len(),
                class.as_deref().unwrap_or("an unnamed class"),
            );
        }
        for i in ordered.order {
            let key = keys[i].0;
            out.push(planned(
                &name,
                key,
                &group.tensors[key],
                &Ops::default(),
                None,
            )?);
        }
    }
    let mut seen = BTreeSet::new();
    for p in &out {
        if !seen.insert((p.component.as_str(), p.key.as_str())) {
            return refuse(
                Code::DUPLICATE_KEY,
                format!("{}/{} is produced twice", p.component, p.key),
            );
        }
    }
    Ok(out)
}

/// The first few of `keys`, and how many more.
fn named(keys: &[&str]) -> String {
    let shown = keys[..keys.len().min(4)].join(", ");
    match keys.len() > 4 {
        true => format!("{shown} and {} more", keys.len() - 4),
        false => shown,
    }
}

fn plan_raw(set: &SourceSet, c: &Component, out: &mut Vec<Planned>) -> Result<()> {
    for row in &c.rows {
        let Src::Raw(raw) = &row.source else { continue };
        let (file, len) = set
            .find_raw(&raw.sha256, raw.length)
            .expect("recognition found it");
        let bytes = raw.dtype.byte_len(&raw.shape).unwrap_or(u64::MAX);
        if raw.offset.checked_add(bytes).is_none_or(|end| end > len) {
            return refuse(
                Code::SHAPE_MISMATCH,
                format!("{}/{}: range outside its file", c.name, row.target),
            );
        }
        let t = Tensor {
            dtype: raw.dtype.clone(),
            shape: raw.shape.clone(),
            file,
            start: raw.offset,
            len: bytes,
            encoded: None,
            derive: None,
        };
        out.push(planned(
            &c.name,
            &row.target,
            &t,
            &row.ops,
            c.cast.as_ref(),
        )?);
    }
    Ok(())
}

fn plan_lora(
    table: &Table,
    lora: &Lora,
    c: &Component,
    claim: &Claim,
    group: &Group,
    out: &mut Vec<Planned>,
) -> Result<()> {
    let mut factors: BTreeMap<&str, [Option<(&str, &Tensor)>; 4]> = BTreeMap::new();
    let mut kron: BTreeMap<&str, [Option<(&str, &Tensor)>; 6]> = BTreeMap::new();
    let longest = |key: &str| {
        table
            .components
            .iter()
            .map(|o| o.prefix.as_str())
            .filter(|p| key.starts_with(p))
            .max_by_key(|p| p.len())
    };
    for (key, t) in group
        .tensors
        .iter()
        .filter(|(k, _)| longest(k) == Some(c.prefix.as_str()))
    {
        let local = &key[c.prefix.len()..];
        let Some((stem, factor)) = lora_split(lora, local) else { continue };
        let slot = match factor as usize {
            i @ 0..4 => &mut factors.entry(stem).or_default()[i],
            i => &mut kron.entry(stem).or_default()[i - 4],
        };
        if slot.replace((key, t)).is_some() {
            return refuse(
                Code::DUPLICATE_KEY,
                format!("{key}: a second {factor:?} factor for {stem}"),
            );
        }
    }
    for row in &c.rows {
        if let Some(k) = row.key().and_then(|stem| kron.get(stem)) {
            let lora = row.key().and_then(|stem| factors.get(stem));
            if lora.is_some_and(|[a, b, _, m]| a.is_some() || b.is_some() || m.is_some()) {
                return refuse(Code::UNPLACED_KEYS, format!("{}: both LoRA and LoKr factors for one module", row.target));
            }
            plan_lokr(&claim.name, row, k, lora.and_then(|f| f[2]), out)?;
            continue;
        }
        let Some([a, b, alpha, magnitude]) = row.key().and_then(|k| factors.get(k)) else {
            continue;
        };
        let (Some((ka, a)), Some((_, b))) = (a, b) else {
            let held = [a, b, alpha, magnitude].into_iter().flatten().map(|(k, _)| *k).collect::<Vec<_>>();
            return refuse(Code::UNPLACED_KEYS, format!("{}: {} has no A and B factor pair to apply", row.target, named(&held)));
        };
        let Some((input, output)) = row.ops.io else {
            return refuse(
                Code::MISSING_FIELD,
                format!("{}: LoRA row without io", table.name),
            );
        };
        let float = |d: &Dtype| matches!(d, Dtype::F16 | Dtype::BF16 | Dtype::F32);
        let rank = a.shape.first().copied().unwrap_or(0);
        let kernel = &row.ops.kernel;
        let want_a: Vec<u64> = [rank, input].iter().chain(kernel).copied().collect();
        let want_b: Vec<u64> = [output, rank].into_iter().chain(kernel.iter().map(|_| 1)).collect();
        if a.encoded.is_some()
            || b.encoded.is_some()
            || rank == 0
            || a.shape != want_a
            || b.shape != want_b
            || a.dtype != b.dtype
            || !float(&a.dtype)
            || alpha.is_some_and(|(_, t)| !t.shape.is_empty() || !float(&t.dtype))
        {
            return refuse(Code::UNPLACED_KEYS, format!("{ka}: not float A {want_a:?}, B {want_b:?} and a scalar alpha"));
        }
        let none = Ops::default();
        for (suffix, t, ops) in [
            (".lora_A.weight", Some(*a), &none),
            (
                ".lora_B.weight",
                Some(*b),
                row.ops.b.as_deref().unwrap_or(&none),
            ),
            (".alpha", alpha.map(|(_, t)| t), &none),
        ] {
            let Some(t) = t else { continue };
            let key = format!("{}{suffix}", row.target);
            out.push(planned(&claim.name, &key, t, ops, None)?);
        }
        if let Some((km, m)) = magnitude {
            out.push(plan_magnitude(&claim.name, row, km, m, output)?);
        }
        if let Some(name) = &row.ops.table {
            out.push(constant(table, name, &claim.name, &format!("{}.table", row.target))?);
        }
    }
    for row in &c.keep {
        let Some(t) = row.key().and_then(|k| group.tensors.get(&format!("{}{k}", c.prefix))) else { continue };
        if kept(row, t) && t.encoded.is_none() {
            out.push(planned(&claim.name, &row.target, t, &Ops::default(), None)?);
        }
    }
    Ok(())
}

/// A LoKr's factors for `row` (LyCORIS' Kronecker adapter: `ΔW = kron(W1, W2)`, each factor whole or an `_a @ _b`
/// pair), stored as they are under `<target>.lokr_w1` …, with their alpha; for a row that takes part of its module's
/// output (a fused Q/K/V's third, a SwiGLU's swapped halves), `<target>.lokr_rows`: the rows of `kron(W1, W2)` it
/// takes, in order, as I32.
fn plan_lokr(
    component: &str,
    row: &Row,
    k: &[Option<(&str, &Tensor)>; 6],
    alpha: Option<(&str, &Tensor)>,
    out: &mut Vec<Planned>,
) -> Result<()> {
    let refused = |why: String| refuse(Code::ADAPTER_TARGET_UNSUPPORTED, format!("{}: {why}", row.target));
    let Some((input, output)) = row.ops.io else { return refused("a LoKr row without io".into()) };
    if !row.ops.kernel.is_empty() || row.ops.table.is_some() {
        return refused("a LoKr on a convolution or a curve site; only a linear's applies".into());
    }
    let [w1, w2, w1a, w1b, w2a, w2b] = k;
    let dims = |whole: &Option<(&str, &Tensor)>, a: &Option<(&str, &Tensor)>, b: &Option<(&str, &Tensor)>| match (whole, a, b) {
        (Some((_, t)), None, None) => Some((t.shape[0], t.shape[1])),
        (None, Some((_, ta)), Some((_, tb))) if ta.shape[1] == tb.shape[0] => Some((ta.shape[0], tb.shape[1])),
        _ => None,
    };
    let (Some((a, b)), Some((c, d))) = (dims(w1, w1a, w1b), dims(w2, w2a, w2b)) else {
        return refused("its LoKr factors are not each one whole matrix or one _a/_b pair".into());
    };
    if a * c != output || b * d != input {
        return refused(format!("kron({a}x{b}, {c}x{d}) is not its module's {output}x{input}"));
    }
    let float = |t: &Tensor| t.encoded.is_none() && t.shape.len() == 2 && matches!(t.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32);
    let named = [("lokr_w1", w1), ("lokr_w2", w2), ("lokr_w1_a", w1a), ("lokr_w1_b", w1b), ("lokr_w2_a", w2a), ("lokr_w2_b", w2b)];
    for (name, factor) in named {
        let Some((key, t)) = factor else { continue };
        if !float(t) {
            return refused(format!("{key}: a LoKr factor that is not a plain float matrix"));
        }
        out.push(planned(component, &format!("{}.{name}", row.target), t, &Ops::default(), None)?);
    }
    if let Some((key, t)) = alpha {
        if !t.shape.is_empty() || !matches!(t.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) {
            return refused(format!("{key}: not a scalar float alpha"));
        }
        out.push(planned(component, &format!("{}.alpha", row.target), t, &Ops::default(), None)?);
    }
    if let Some(ops) = row.ops.b.as_deref() {
        let rows = selected(&row.target, output, ops)?;
        let bytes: Vec<u8> = rows.iter().flat_map(|r| (*r as i32).to_le_bytes()).collect();
        let shape = vec![rows.len() as u64];
        let read = Reading::inline(bytes, Dtype::I32);
        let part = PlannedPart { role: String::new(), dtype: Dtype::I32, shape: shape.clone(), read };
        out.push(Planned {
            component: component.to_owned(),
            key: format!("{}.lokr_rows", row.target),
            dtype: Dtype::I32,
            shape,
            encoding: None,
            parts: vec![part],
        });
    }
    Ok(())
}

/// The rows of a module's `output` a row's B op takes, in order: its view `[n, -1]` cuts them into n equal blocks and
/// its take `[0, [j, …]]` keeps those blocks (a fused Q/K/V's third, a SwiGLU's swapped halves). Another op is no
/// selection of whole rows.
fn selected(target: &str, output: u64, ops: &Ops) -> Result<Vec<u64>> {
    match (&ops.view, &ops.take, &ops.permute) {
        (Some(view), Some((0, blocks)), None) if view.len() == 2 && view[1] == -1 && view[0] > 0 && output % view[0] as u64 == 0 => {
            let size = output / view[0] as u64;
            Ok(blocks.iter().flat_map(|j| j * size..(j + 1) * size).collect())
        }
        _ => refuse(
            Code::ADAPTER_TARGET_UNSUPPORTED,
            format!("{target}: a LoKr on a module this table re-lays by more than whole row blocks"),
        ),
    }
}

/// A DoRA's magnitude for `row`, `<target>.lora_magnitude_vector` `[out]`: per output channel, so it is cut as
/// B's rows are. Another axis's (LyCORIS' `wd_on_out=False`, `[1, in]`), a convolution's or an AdaLN curve's
/// is no PEFT DoRA, and refused.
fn plan_magnitude(component: &str, row: &Row, key: &str, m: &Tensor, output: u64) -> Result<Planned> {
    let target = format!("{}.lora_magnitude_vector", row.target);
    let unsupported = |why: &str| refuse(Code::ADAPTER_TARGET_UNSUPPORTED, format!("{key}: {why}"));
    if !row.ops.kernel.is_empty() || row.ops.table.is_some() {
        return unsupported("a DoRA magnitude on a convolution or a curve site; only a linear's applies");
    }
    if m.shape.first() != Some(&output) {
        return unsupported("a DoRA magnitude of the input axis (LyCORIS wd_on_out=False); only the output axis's (PEFT's) applies");
    }
    if m.encoded.is_some() || !matches!(m.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) {
        return unsupported("a DoRA magnitude that is not plain float");
    }
    let mut ops = row.ops.b.as_deref().cloned().unwrap_or_default();
    ops.shape = Some(vec![-1]);
    planned(component, &target, m, &ops, None)
}

/// A table's constant as one planned tensor, its bytes checked against their digest.
fn constant(table: &Table, name: &str, component: &str, key: &str) -> Result<Planned> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let Some(c) = table.constants.get(name) else {
        return refuse(Code::MISSING_FIELD, format!("{}: no constant {name:?}", table.name));
    };
    let bytes = STANDARD.decode(&c.base64).ok().filter(|b| {
        c.dtype.byte_len(&c.shape) == Some(b.len() as u64)
            && crate::sha256::hex_digest(b) == c.sha256
    });
    let Some(bytes) = bytes else {
        return refuse(Code::SHAPE_MISMATCH, format!("{}: constant {name:?} differs from its digest", table.name));
    };
    let read = Reading::inline(bytes, c.dtype.clone());
    let part = PlannedPart { role: String::new(), dtype: c.dtype.clone(), shape: c.shape.clone(), read };
    Ok(Planned {
        component: component.to_owned(),
        key: key.to_owned(),
        dtype: c.dtype.clone(),
        shape: c.shape.clone(),
        encoding: None,
        parts: vec![part],
    })
}

// ------------------------------------------------------------------------------ bytes

/// Streams one planned part's bytes out of its source file.
pub(crate) struct TensorReader<'f> {
    file: &'f File,
    r: &'f Reading,
    what: String,
    /// Next run, and units of it already read.
    run: u64,
    within: u64,
    /// The source tensor whole, when its runs are too short to read one by one.
    whole: Option<Vec<u8>>,
    raw: Vec<u8>,
    out: Vec<u8>,
    at: usize,
    seen: u64,
}

/// Runs shorter than this are gathered from the whole source tensor in memory.
const SHORT_RUN: u64 = 4096;
/// Bytes read from the file per step.
const STEP: u64 = 8 << 20;

impl<'f> TensorReader<'f> {
    /// `what` names the tensor in errors.
    pub(crate) fn new(file: &'f File, r: &'f Reading, what: String) -> Result<TensorReader<'f>> {
        let whole = if let Some(bytes) = &r.inline {
            Some(bytes.clone())
        } else if let Some(d) = &r.derive {
            let read = |(start, len): (u64, u64)| -> Result<Vec<u8>> {
                let mut buf = vec![0u8; len as usize];
                file.read_exact_at(&mut buf, start).map_err(|e| io(&what, e))?;
                Ok(buf)
            };
            let sources = [(r.start, r.len)].into_iter().chain(d.with.iter().copied()).map(read).collect::<Result<Vec<_>>>()?;
            let out = (d.f)(&sources.iter().map(Vec::as_slice).collect::<Vec<_>>());
            debug_assert_eq!(out.len() as u64, d.len, "{what}");
            Some(out)
        } else if r.runs.count() > 1 && r.runs.run * r.unit < SHORT_RUN {
            let mut buf = vec![0u8; r.len as usize];
            file.read_exact_at(&mut buf, r.start)
                .map_err(|e| io(&what, e))?;
            Some(buf)
        } else {
            None
        };
        Ok(TensorReader {
            file,
            r,
            what,
            run: 0,
            within: 0,
            whole,
            raw: vec![],
            out: vec![],
            at: 0,
            seen: 0,
        })
    }

    fn fill(&mut self) -> std::io::Result<bool> {
        let r = self.r;
        if self.run >= r.runs.count() || r.runs.run == 0 {
            return Ok(false);
        }
        self.raw.clear();
        let unit = r.unit;
        if let Some(whole) = &self.whole {
            let run = (r.runs.run * unit) as usize;
            while self.run < r.runs.count() && (self.raw.len() as u64) < STEP {
                let at = (r.runs.offset(self.run) * unit) as usize;
                self.raw.extend_from_slice(&whole[at..at + run]);
                self.run += 1;
            }
        } else {
            let take = (r.runs.run - self.within).min((STEP / unit).max(1));
            let at = r.start + (r.runs.offset(self.run) + self.within) * unit;
            self.raw.resize((take * unit) as usize, 0);
            self.file.read_exact_at(&mut self.raw, at)?;
            self.within += take;
            if self.within == r.runs.run {
                self.run += 1;
                self.within = 0;
            }
        }
        self.at = 0;
        if r.cast.is_none() {
            std::mem::swap(&mut self.out, &mut self.raw);
            return Ok(true);
        }
        self.out.clear();
        let wide = r.source == Dtype::F32;
        for e in self.raw.chunks_exact(unit as usize) {
            let bits = if wide {
                u32::from_le_bytes([e[0], e[1], e[2], e[3]])
            } else {
                u32::from(u16::from_le_bytes([e[0], e[1]])) << 16
            };
            let half = f32_to_f16(bits);
            if half & 0x7fff == 0x7c00 && bits & 0x7f80_0000 != 0x7f80_0000 {
                return Err(std::io::Error::other(format!(
                    "{}: element {} ({}) overflows F16",
                    self.what,
                    self.seen,
                    f32::from_bits(bits)
                )));
            }
            self.seen += 1;
            self.out.extend_from_slice(&half.to_le_bytes());
        }
        Ok(true)
    }
}

impl Read for TensorReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at == self.out.len() {
            if !self.fill()? {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.out.len() - self.at);
        buf[..n].copy_from_slice(&self.out[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

fn io(what: &str, e: std::io::Error) -> Refusal {
    Refusal {
        code: Code::IO_FAILED,
        detail: format!("{what}: {e}"),
    }
}

/// numpy's `astype(float16)` from f32 bits: round to nearest even, NaN payload kept.
pub(crate) fn f32_to_f16(f: u32) -> u16 {
    let sign = ((f & 0x8000_0000) >> 16) as u16;
    let exp = f & 0x7f80_0000;
    if exp >= 0x4780_0000 {
        let sig = f & 0x007f_ffff;
        if exp == 0x7f80_0000 && sig != 0 {
            let nan = 0x7c00 + (sig >> 13) as u16;
            return sign + if nan == 0x7c00 { nan + 1 } else { nan };
        }
        return sign + 0x7c00;
    }
    if exp <= 0x3800_0000 {
        if exp < 0x3300_0000 {
            return sign;
        }
        let mut sig = (0x0080_0000 + (f & 0x007f_ffff)) >> (113 - (exp >> 23));
        if (sig & 0x3fff) != 0x1000 || (f & 0x7ff) != 0 {
            sig += 0x1000;
        }
        return sign + (sig >> 13) as u16;
    }
    let mut sig = f & 0x007f_ffff;
    if (sig & 0x3fff) != 0x1000 {
        sig += 0x1000;
    }
    sign + ((sig >> 13) as u16 + ((exp - 0x3800_0000) >> 13) as u16)
}
