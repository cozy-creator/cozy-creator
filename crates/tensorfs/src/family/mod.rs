//! Model families: the canonical form of each first-party model, as TensorFS knows it.
//!
//! One `tensorfs/families/<name>.json` per family, generated once from its pinned reference by
//! `tensorfs-py/scripts/families.py` and compiled in. Nothing downloads a reference to learn it.
//! A family lists its components' (key, shape) in the reference's order, the stored forms that
//! edit them (H3's curve and AdaLN-pruned DiTs), the converter tables that write it, and, for the
//! tensors a converter permutes, a layout probe ([`probe`]). Ingest verifies a converted source
//! against these and records [`Record`] in the header (`family`); a LoRA records [`Adapter`]s
//! (`adapter`). Neither is ever guessed: unverified, the field is absent.

pub mod probe;

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::format::{Component, Metadata, Value};

macro_rules! family {
    ($name:literal) => {
        include_str!(concat!("../../families/", $name, ".json"))
    };
}

/// The built-in families.
pub const FAMILIES: [&str; 9] = [
    family!("minimax-h3"),
    family!("sdxl"),
    family!("anima"),
    family!("qwen-image"),
    family!("qwen-image-2"),
    family!("krea-2"),
    family!("moss"),
    family!("ideogram-4"),
    family!("seedvr2"),
];

/// The form a family's reference is in.
pub const CANONICAL: &str = "canonical";
/// Header metadata: the verified family, [`Record`].
pub const FAMILY: &str = "family";
/// Header metadata: each LoRA component's base, [`Adapter`], by the LoRA's component name.
pub const ADAPTER: &str = "adapter";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    pub name: String,
    /// The pinned repository (`hf://org/repo@<commit>`) the components were read from.
    pub reference: String,
    /// The converter tables whose output is this family's (`h3.native`, `h3.lora`).
    #[serde(default)]
    pub sources: Vec<String>,
    /// Class names (a pipeline's `_class_name`, its denoiser's) that make a source in its own
    /// Diffusers or Transformers layout this family's.
    #[serde(default)]
    pub classes: Vec<String>,
    pub components: BTreeMap<String, Def>,
    #[serde(default)]
    pub forms: BTreeMap<String, Form>,
    #[serde(default)]
    pub probe: Option<probe::Probe>,
    /// The producer fields that declare which component a LoRA was trained on.
    #[serde(default)]
    pub trained_on: Vec<TrainedOn>,
}

/// A canonical component, or a variant (`same_as`) sharing another's keys and shapes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Def {
    #[serde(default)]
    pub same_as: Option<String>,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub keys: Vec<(String, Vec<u64>)>,
    /// Keys a stored component may or may not have.
    #[serde(default)]
    pub optional: Vec<String>,
    /// LoRA-only targets, `[path, [out, in]]`: what an adapter may train that no weight is.
    #[serde(default)]
    pub sites: Vec<(String, Vec<u64>)>,
}

/// A stored form: a declared edit of some components' canonical keys. `*` in a key matches any
/// text; a `null` extent any extent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub components: Vec<String>,
    #[serde(default)]
    pub drop: Vec<String>,
    #[serde(default)]
    pub set: Vec<(String, Vec<Option<u64>>)>,
    #[serde(default)]
    pub add: Vec<(String, Vec<Option<u64>>)>,
}

/// A producer's `__metadata__` field whose value names the component a LoRA was trained on.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainedOn {
    pub producer: String,
    pub field: String,
    pub values: BTreeMap<String, String>,
}

/// Every built-in family, parsed once.
pub fn families() -> &'static [Family] {
    static PARSED: std::sync::OnceLock<Vec<Family>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        FAMILIES
            .iter()
            .map(|text| serde_json::from_str(text).expect("a built-in family"))
            .collect()
    })
}

/// The built-in families' digest: an import is also what these families verify it as.
pub fn families_digest() -> &'static str {
    static DIGEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIGEST.get_or_init(|| crate::sha256::hex_digest(FAMILIES.join("\0").as_bytes()))
}

pub fn named(name: &str) -> Option<&'static Family> {
    families().iter().find(|f| f.name == name)
}

/// The family a converter table writes.
pub fn of_table(table: &str) -> Option<&'static Family> {
    families().iter().find(|f| f.sources.iter().any(|s| s == table))
}

/// The family a class names.
pub fn of_class(class: &str) -> Option<&'static Family> {
    families().iter().find(|f| f.classes.iter().any(|c| c == class))
}

/// One stored component: its name and every tensor's key and logical shape.
pub struct Stored<'a> {
    pub name: &'a str,
    pub keys: Vec<(&'a str, &'a [u64])>,
}

type Expected = BTreeMap<String, Vec<Option<u64>>>;

/// `*` matches any run of characters.
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || !text[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for middle in &parts[1..parts.len() - 1] {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    true
}

fn fits(want: &[Option<u64>], got: &[u64]) -> bool {
    want.len() == got.len() && want.iter().zip(got).all(|(w, g)| w.is_none_or(|w| w == *g))
}

fn shown(want: &[Option<u64>]) -> String {
    let dims: Vec<String> = want.iter().map(|d| d.map_or("*".into(), |d| d.to_string())).collect();
    format!("[{}]", dims.join(", "))
}

impl Family {
    /// A component's definition, through `same_as`; with the canonical name it shares.
    pub fn def(&self, component: &str) -> Option<(&str, &Def)> {
        let (name, def) = self.components.get_key_value(component)?;
        match &def.same_as {
            Some(root) => self.components.get_key_value(root.as_str()).map(|(n, d)| (n.as_str(), d)),
            None => Some((name.as_str(), def)),
        }
    }

    /// The components that are variants of `component`'s canonical one, itself included.
    pub fn variants(&self, component: &str) -> Vec<&str> {
        let Some((root, _)) = self.def(component) else { return vec![] };
        (self.components.keys())
            .filter(|c| self.def(c).is_some_and(|(r, _)| r == root))
            .map(String::as_str)
            .collect()
    }

    /// `component`'s keys under `form`.
    fn expected(&self, component: &str, form: &str) -> Option<Expected> {
        let (root, def) = self.def(component)?;
        let mut keys: Expected =
            (def.keys.iter()).map(|(k, s)| (k.clone(), s.iter().map(|d| Some(*d)).collect())).collect();
        if form == CANONICAL {
            return Some(keys);
        }
        let edit = self.forms.get(form)?;
        if !edit.components.iter().any(|c| c == component || c == root) {
            return Some(keys);
        }
        keys.retain(|k, _| !edit.drop.iter().any(|p| glob(p, k)));
        for (pattern, shape) in &edit.set {
            for (_, s) in keys.iter_mut().filter(|(k, _)| glob(pattern, k)) {
                *s = shape.clone();
            }
        }
        keys.extend(edit.add.iter().cloned());
        Some(keys)
    }

    /// Every form's name, canonical first.
    pub fn form_names(&self) -> Vec<&str> {
        std::iter::once(CANONICAL).chain(self.forms.keys().map(String::as_str)).collect()
    }

    /// The differences of `stored` from `form`, the first few named.
    fn differences(&self, stored: &[Stored], form: &str) -> Vec<String> {
        let mut out = Vec::new();
        for s in stored {
            let Some(expected) = self.expected(s.name, form) else {
                out.push(format!("{} is no component of {}", s.name, self.name));
                continue;
            };
            let optional: BTreeSet<&str> = (self.def(s.name).into_iter())
                .flat_map(|(_, d)| d.optional.iter().map(String::as_str))
                .collect();
            let got: BTreeMap<&str, &[u64]> = s.keys.iter().copied().collect();
            for (key, shape) in &got {
                match expected.get(*key) {
                    Some(want) if !fits(want, shape) => {
                        out.push(format!("{}/{key} is {shape:?}, {} {form} has {}", s.name, self.name, shown(want)))
                    }
                    None if !optional.contains(key) => {
                        out.push(format!("{}/{key} is not in {} {form}", s.name, self.name))
                    }
                    _ => {}
                }
            }
            for key in expected.keys().filter(|k| !got.contains_key(k.as_str()) && !optional.contains(k.as_str())) {
                out.push(format!("{}/{key} is missing for {} {form}", s.name, self.name));
            }
        }
        out
    }

    /// The form every stored component is in, or the closest form's differences (a few named,
    /// and how many). A component the family does not list is a difference in every form.
    pub fn form_of(&self, stored: &[Stored]) -> Result<String, String> {
        let mut closest: Option<(usize, &str, Vec<String>)> = None;
        for form in self.form_names() {
            let diffs = self.differences(stored, form);
            if diffs.is_empty() {
                return Ok(form.to_owned());
            }
            if closest.as_ref().is_none_or(|(n, _, _)| diffs.len() < *n) {
                closest = Some((diffs.len(), form, diffs));
            }
        }
        let (n, form, diffs) = closest.expect("at least the canonical form");
        let named: Vec<&str> = diffs.iter().take(3).map(String::as_str).collect();
        Err(format!("closest is {form}, {n} differences: {}", named.join("; ")))
    }

    /// The shapes the base weight or LoRA site `path` has in `component`, one per form that has it:
    /// `[out, in]` (with a convolution's kernel).
    pub fn targets(&self, component: &str, path: &str) -> Vec<Vec<Option<u64>>> {
        let Some((_, def)) = self.def(component) else { return vec![] };
        let weight = format!("{path}.weight");
        let sites = (def.sites.iter()).filter(|(p, _)| p == path).map(|(_, s)| s.iter().map(|d| Some(*d)).collect());
        let forms = (self.form_names().into_iter()).filter_map(|form| self.expected(component, form)?.remove(&weight));
        sites.chain(forms).collect()
    }

    /// The component a LoRA was trained on, by its producer's declared field: `(component,
    /// "field=value")`. Never inferred from structure.
    pub fn trained_on(&self, metadata: &BTreeMap<String, String>) -> Option<(String, String)> {
        self.trained_on.iter().find_map(|rule| {
            let value = metadata.get(&rule.field)?;
            let component = rule.values.get(value)?;
            Some((component.clone(), format!("{} {}={value}", rule.producer, rule.field)))
        })
    }
}

/// `family`: a checkpoint verified as a family's form. `components` maps each stored component
/// to the family component it is (`transformer` → `transformer_ref` for a Ref2VA DiT stored
/// alone), by name or by its layout probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub form: String,
    pub reference: String,
    pub components: BTreeMap<String, String>,
}

fn text(s: &str) -> Value {
    Value::Str(s.to_owned())
}

fn texts(m: &BTreeMap<String, String>) -> Value {
    Value::Map(m.iter().map(|(k, v)| (k.clone(), text(v))).collect())
}

fn field<'a>(m: &'a BTreeMap<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k)?.as_str()
}

fn strings(v: Option<&Value>) -> BTreeMap<String, String> {
    match v {
        Some(Value::Map(m)) => m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect(),
        _ => BTreeMap::new(),
    }
}

impl Record {
    pub fn value(&self) -> Value {
        Value::Map(BTreeMap::from([
            ("name".to_owned(), text(&self.name)),
            ("form".to_owned(), text(&self.form)),
            ("reference".to_owned(), text(&self.reference)),
            ("components".to_owned(), texts(&self.components)),
        ]))
    }

    /// A header's `family`; none when absent or malformed.
    pub fn read(metadata: &Metadata) -> Option<Record> {
        let Some(Value::Map(m)) = metadata.get(FAMILY) else { return None };
        Some(Record {
            name: field(m, "name")?.to_owned(),
            form: field(m, "form")?.to_owned(),
            reference: field(m, "reference")?.to_owned(),
            components: strings(m.get("components")),
        })
    }

    /// The family component a stored one is: what `components` records, else its own name.
    pub fn component<'a>(&'a self, stored: &'a str) -> &'a str {
        self.components.get(stored).map_or(stored, String::as_str)
    }
}

/// `metadata`'s `family` re-read for the base components a writer lays out: the form they are
/// now (a lane its recipe pruned is the pruned form), or no `family` when they are no form of it.
/// A derived lane carries its base's metadata; this keeps `family` true of what was written.
pub fn reverify(metadata: &mut Metadata, components: &[Component]) {
    let lora = |c: &&Component| c.tensors.iter().all(|t| [".lora_A.weight", ".lora_B.weight", ".alpha"].iter().any(|f| t.key.ends_with(f)));
    let bases: Vec<&Component> = components.iter().filter(|c| !c.tensors.is_empty() && !lora(c)).collect();
    let Some(mut record) = Record::read(metadata).filter(|_| !bases.is_empty()) else {
        metadata.remove(FAMILY);
        return;
    };
    let Some(family) = named(&record.name) else {
        metadata.remove(FAMILY);
        return;
    };
    let stored: Vec<Stored> = (bases.iter())
        .map(|c| Stored { name: &c.name, keys: c.tensors.iter().map(|t| (t.key.as_str(), t.shape.as_slice())).collect() })
        .collect();
    match family.form_of(&stored) {
        Ok(form) => {
            record.form = form;
            record.components.retain(|c, _| bases.iter().any(|b| &b.name == c));
            for b in &bases {
                record.components.entry(b.name.clone()).or_insert_with(|| b.name.clone());
            }
            metadata.insert(FAMILY.into(), record.value());
        }
        Err(why) => {
            tracing::info!("no {} family on this checkpoint: {why}", record.name);
            metadata.remove(FAMILY);
        }
    }
}

/// A header's `family` and `adapter` records in one line, for a run's record: `minimax-h3
/// canonical, transformer is transformer_ref`, `LoRA of minimax-h3 transformer, trained on
/// transformer_ref (musubi-tuner ss_h3_training_mode=ref2va)`.
pub fn summary(metadata: &Metadata) -> Option<String> {
    let mut out = Vec::new();
    if let Some(r) = Record::read(metadata) {
        let mut line = format!("{} {}", r.name, r.form);
        for (stored, is) in r.components.iter().filter(|(s, i)| s != i) {
            line.push_str(&format!(", {stored} is {is}"));
        }
        out.push(line);
    }
    for a in Adapter::read_all(metadata).values() {
        let mut line = format!("LoRA of {} {}", a.family, a.component);
        if let Some(t) = &a.trained_on {
            line.push_str(&format!(", trained on {} ({})", t.component, t.from));
        }
        out.push(line);
    }
    (!out.is_empty()).then(|| out.join("; "))
}

/// What a LoRA component trained on, and the evidence: a producer's declared field, or
/// `--trained-on`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainedOnRecord {
    pub component: String,
    pub from: String,
}

/// `adapter[<LoRA component>]`: the family and base component its factors fit, and what it was
/// trained on when declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub family: String,
    pub component: String,
    pub trained_on: Option<TrainedOnRecord>,
}

impl Adapter {
    pub fn value(&self) -> Value {
        let mut m = BTreeMap::from([
            ("family".to_owned(), text(&self.family)),
            ("component".to_owned(), text(&self.component)),
        ]);
        if let Some(t) = &self.trained_on {
            let on = BTreeMap::from([("component".to_owned(), text(&t.component)), ("from".to_owned(), text(&t.from))]);
            m.insert("trained_on".into(), Value::Map(on));
        }
        Value::Map(m)
    }

    fn read(v: &Value) -> Option<Adapter> {
        let Value::Map(m) = v else { return None };
        let trained_on = match m.get("trained_on") {
            Some(Value::Map(t)) => Some(TrainedOnRecord {
                component: field(t, "component")?.to_owned(),
                from: field(t, "from")?.to_owned(),
            }),
            _ => None,
        };
        Some(Adapter {
            family: field(m, "family")?.to_owned(),
            component: field(m, "component")?.to_owned(),
            trained_on,
        })
    }

    /// A header's `adapter` records by LoRA component; malformed ones are skipped.
    pub fn read_all(metadata: &Metadata) -> BTreeMap<String, Adapter> {
        let Some(Value::Map(m)) = metadata.get(ADAPTER) else { return BTreeMap::new() };
        m.iter().filter_map(|(c, v)| Some((c.clone(), Adapter::read(v)?))).collect()
    }

    pub fn value_all(adapters: &BTreeMap<String, Adapter>) -> Value {
        Value::Map(adapters.iter().map(|(c, a)| (c.clone(), a.value())).collect())
    }
}
