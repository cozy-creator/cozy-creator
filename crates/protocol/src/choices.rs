//! A run's model choices as people write them, bound to a function's slots: `model.<slot>=` and
//! `--lora [slot=]ref[,strength]`, and the `models:` and `lora:` beside the request's fields in
//! `cozy run --input` and in a project's render blocks. The one binder for `cozy run` and Calcifer.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::{
    error::{Error, Kind},
    interface::Slot,
    machine::ModelChoice,
    model_ref::{self, ModelRef},
};

/// One model slot's choice: its reference as written, and the adapters applied to it in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    /// The root slot's parameter, or another callable's full slot path.
    pub slot: String,
    pub reference: Option<String>,
    pub adapters: Vec<Adapter>,
}

/// A LoRA. The machine applies each of its parts to the component of its slot's model, of those
/// the call runs, named like it or that its layers fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub reference: String,
    /// A canonical decimal.
    pub scale: String,
}

/// The choices for one function: its own slots and those of the calls it makes.
pub struct Choices<'a> {
    /// How refusals name it (`org/pkg/fn`).
    target: &'a str,
    callable: &'a str,
    own: &'a [Slot],
    calls: Vec<&'a Slot>,
    chosen: BTreeMap<String, Choice>,
    /// Unknown keys of `models` and `lora` entries, as people read their paths.
    pub dropped: Vec<String>,
}

impl<'a> Choices<'a> {
    /// `callable`'s choices; `target` names it in refusals (empty: its name).
    pub fn new(target: &'a str, callable: &'a str, own: &'a [Slot], calls: Vec<&'a Slot>) -> Self {
        let target = if target.is_empty() { callable } else { target };
        Self { target, callable, own, calls, chosen: BTreeMap::new(), dropped: vec![] }
    }

    pub fn into_choices(self) -> Vec<Choice> {
        self.chosen.into_values().collect()
    }

    /// Takes `models:` and `lora:` out of `document`, which holds the request's fields beside them.
    pub fn take(&mut self, document: &mut Map<String, Value>) -> Result<(), Error> {
        let keys: Vec<String> = document.keys().filter(|k| matches!(k.as_str(), "models" | "lora")).cloned().collect();
        for key in keys {
            match (key.as_str(), document.remove(&key).expect("a key just listed")) {
                ("models", models) => self.envelope(models)?,
                (_, Value::Array(list)) => self.overlays(None, &list, "lora")?,
                _ => {
                    return Err(usage("lora_malformed", "lora must be a list of {ref, strength}")
                        .remedy("lora: [{ref: org/lora, strength: 0.8}]"));
                }
            }
        }
        Ok(())
    }

    /// `{"<slot>": {"ref": "…", "lora": [{"ref": "…", "strength": 0.8}]}}`.
    fn envelope(&mut self, value: Value) -> Result<(), Error> {
        let malformed = || {
            usage("model_binding_invalid", "models must be an object keyed by model slot").remedy(
                r#"{"models": {"<slot>": {"ref": "org/model@release", "lora": [{"ref": "org/lora", "strength": 0.8}]}}}"#,
            )
        };
        let Value::Object(slots) = value else {
            return Err(malformed());
        };
        for (slot, entry) in slots {
            let Value::Object(entry) = entry else {
                return Err(malformed());
            };
            if let Some(reference) = entry.get("ref").and_then(Value::as_str) {
                self.choose(&slot, reference.to_owned())?;
            }
            match entry.get("lora") {
                None => {}
                Some(Value::Array(list)) => self.overlays(Some(&slot), list, &format!("models.{slot}.lora"))?,
                Some(_) => return Err(malformed()),
            }
            let unknown = entry.keys().filter(|k| !matches!(k.as_str(), "ref" | "lora"));
            self.dropped.extend(unknown.map(|k| format!("models.{slot}.{k}")));
        }
        Ok(())
    }

    /// `[{ref, strength}]` at `path`.
    fn overlays(&mut self, slot: Option<&str>, list: &[Value], path: &str) -> Result<(), Error> {
        for (i, entry) in list.iter().enumerate() {
            let at = format!("{path}[{i}]");
            let Value::Object(entry) = entry else {
                return Err(usage("lora_malformed", format!("{at} is not {{ref, strength}}")));
            };
            let reference = entry.get("ref").and_then(Value::as_str).unwrap_or_default();
            let strength = match entry.get("strength") {
                None => "1".to_owned(),
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::String(s)) => s.clone(),
                Some(_) => return Err(usage("lora_strength_invalid", format!("{at}.strength is not a number"))),
            };
            let unknown = entry.keys().filter(|k| !matches!(k.as_str(), "ref" | "strength"));
            self.dropped.extend(unknown.map(|k| format!("{at}.{k}")));
            let written = match slot {
                Some(slot) => format!("{slot}={reference},{strength}"),
                None => format!("{reference},{strength}"),
            };
            self.lora(slot, reference, &strength, &written)?;
        }
        Ok(())
    }

    /// One LoRA on the slot named, else on the function's model ([`Self::lora_slots`]); `written`
    /// is how the person wrote it.
    pub fn lora(&mut self, slot: Option<&str>, reference: &str, strength: &str, written: &str) -> Result<(), Error> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Err(usage("lora_malformed", format!("--lora {written} names no LoRA")));
        }
        let slots = match slot {
            Some(slot) if ident(slot) => self.slots(slot)?,
            Some(slot) => {
                return Err(usage("lora_malformed", format!("--lora {written}: {slot:?} is not a model slot")).remedy(
                    format!(
                        "--lora <org/lora>[,strength] adapts the function's model; <slot>=<org/lora> names one of: {}",
                        self.slot_names().join(", ")
                    ),
                ));
            }
            None => self.lora_slots(written)?,
        };
        let adapter = Adapter { reference: reference.to_owned(), scale: model_ref::strength(strength)? };
        self.at(slots, |choice| {
            choice.adapters.push(adapter.clone());
            Ok(())
        })
    }

    /// `model.<slot>=<reference>`.
    pub fn choose(&mut self, slot: &str, reference: String) -> Result<(), Error> {
        let slots = self.slots(slot)?;
        self.at(slots, |choice| {
            if choice.reference.is_some() {
                return Err(usage("model_slot_repeated", format!("model slot {slot} was chosen more than once")));
            }
            choice.reference = Some(reference.clone());
            Ok(())
        })
    }

    fn at(&mut self, slots: Vec<String>, mut f: impl FnMut(&mut Choice) -> Result<(), Error>) -> Result<(), Error> {
        for slot in slots {
            f(self.chosen.entry(slot.clone()).or_insert_with(|| Choice { slot, ..Choice::default() }))?;
        }
        Ok(())
    }

    fn own(&self, asked: &str) -> Option<&Slot> {
        self.own.iter().find(|s| s.param == asked || s.path == asked)
    }

    /// Where a LoRA naming no slot goes: each call's main model (its first constructed slot, the
    /// one `--lane` names), when those are one model. More than one is refused, naming each.
    fn lora_slots(&self, written: &str) -> Result<Vec<String>, Error> {
        let candidates: Vec<(String, &Slot)> = match main_slot(self.own.iter()) {
            Some(own) => vec![(own.param.clone(), own)],
            None => {
                let mut callees: Vec<&str> = self.calls.iter().map(|s| callee(&s.path)).collect();
                callees.dedup();
                (callees.into_iter())
                    .filter_map(|c| main_slot(self.calls.iter().copied().filter(|s| callee(&s.path) == c)))
                    .map(|s| (s.path.clone(), s))
                    .collect()
            }
        };
        // One model: the same default `org/name`, else (no default) the same class.
        let model = |s: &Slot| match &s.default {
            Some(default) => default.split('@').next().unwrap_or(default).to_ascii_lowercase(),
            None => s.class.clone(),
        };
        let mut models: Vec<(String, Vec<&str>)> = Vec::new();
        for (_, slot) in &candidates {
            match models.iter_mut().find(|(m, _)| *m == model(slot)) {
                Some((_, params)) if params.contains(&slot.param.as_str()) => {}
                Some((_, params)) => params.push(&slot.param),
                None => models.push((model(slot), vec![&slot.param])),
            }
        }
        let name = self.target;
        match models.len() {
            0 => Err(usage("lora_slot_absent", format!("--lora {written}: {name} loads no model a LoRA applies to"))),
            1 => Ok(candidates.into_iter().map(|(slot, _)| slot).collect()),
            n => {
                let each = |f: &dyn Fn(&str, &str) -> String| -> Vec<String> {
                    (models.iter()).flat_map(|(m, params)| params.iter().map(|p| f(p, m))).collect()
                };
                Err(usage(
                    "lora_slot_ambiguous",
                    format!(
                        "--lora {written}: {name} runs {n} models a LoRA could adapt: {}; name one",
                        each(&|p, m| format!("{p} ({m})")).join(", ")
                    ),
                )
                .remedy(each(&|p, _| format!("--lora {p}={written}")).join("\n  ")))
            }
        }
    }

    /// The short slot names a run may choose: its own, and those of the calls it makes.
    fn slot_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.own.iter().map(|s| s.param.as_str()).collect();
        for slot in &self.calls {
            if !names.contains(&slot.param.as_str()) {
                names.push(&slot.param);
            }
        }
        names
    }

    /// The slots `asked` names: a declared slot (its parameter or path), another callable's full
    /// slot path (`[org/pkg/]callable.models.param`), whose machine checks it, or a parameter the
    /// calls it makes declare (every one of them, which must load one model class).
    fn slots(&self, asked: &str) -> Result<Vec<String>, Error> {
        if let Some(slot) = self.own(asked) {
            return Ok(vec![slot.param.clone()]);
        }
        if child_slot(asked) && !asked.starts_with(&format!("{}.", self.callable)) {
            return Ok(vec![asked.to_owned()]);
        }
        let called: Vec<&Slot> = self.calls.iter().copied().filter(|s| s.param == asked).collect();
        if let Some(first) = called.first() {
            if let Some(other) = called.iter().find(|s| s.class != first.class) {
                return Err(usage(
                    "model_slot_ambiguous",
                    format!(
                        "{asked} names slots of different models in {}'s calls: {} ({}) and {} ({})",
                        self.callable, first.path, first.class, other.path, other.class
                    ),
                )
                .remedy("name one by its path, `<callable>.models.<parameter>`"));
            }
            return Ok(called.iter().map(|s| s.path.clone()).collect());
        }
        let mut declared: Vec<&str> = self.own.iter().map(|s| s.param.as_str()).collect();
        let through: BTreeSet<&str> = self.calls.iter().map(|s| s.param.as_str()).collect();
        declared.extend(through.iter().filter(|p| !declared.contains(p)).collect::<Vec<_>>());
        let remedy = match declared.is_empty() {
            true => format!("{} declares no model slots", self.callable),
            false => format!("{} takes: {}", self.callable, declared.join(", ")),
        };
        Err(usage("model_slot_unknown", format!("no model slot {asked:?} for {}", self.callable)).remedy(remedy))
    }
}

/// Each slot's choice as the machine takes it, its adapters by the same reference grammar.
pub fn wire(models: &[Choice]) -> Result<Vec<ModelChoice>, Error> {
    models
        .iter()
        .map(|choice| {
            let mut out = match &choice.reference {
                Some(reference) => ModelRef::parse(reference)?.choice(&choice.slot),
                None => ModelChoice { parameter: choice.slot.clone(), ..Default::default() },
            };
            for adapter in &choice.adapters {
                out.adapters.push(ModelRef::parse(&adapter.reference)?.adapter(&adapter.scale));
            }
            Ok(out)
        })
        .collect()
}

fn ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A call's main model: its first constructed slot that loads with the call, which `--lane`
/// names (TensorD's `main_model`).
fn main_slot<'s>(mut slots: impl Iterator<Item = &'s Slot>) -> Option<&'s Slot> {
    slots.find(|s| !s.weights && !s.on_demand)
}

/// The callable a slot path (`<callable>.models.<param>`) belongs to.
fn callee(path: &str) -> &str {
    path.split(".models.").next().unwrap_or(path)
}

/// `[org/pkg/]callable.models.param`: another callable's slot, which its machine checks.
fn child_slot(asked: &str) -> bool {
    let tail = asked.rsplit('/').next().unwrap_or(asked);
    let mut parts = tail.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(callable), Some("models"), Some(param), None) if ident(callable) && ident(param)
    ) && asked.matches('/').count() != 1
}

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}
