//! What a checkpoint holds as a model slot reads it (cl-943): each component's class, the modules its tensors fill (and their
//! shapes), its run data and its config. A slot loads a checkpoint that holds what the slot's default holds in the components it
//! reads: the same classes (library and class, any version), every module at the default's shapes, and run data of every kind,
//! however it is spelled (`preprocessor_config.json` for `processor_config.json`). A slot whose only output is an LM's words takes
//! its modules at any size. A family label never decides it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::format::{ClassRef, Component, Header, Tensor};

/// Run data by kind, as the self-contained standard (xs-170) counts it: tokenizers and their vocabularies, processors, chat
/// templates and timestep plans. Docs, ingest records and the prompts a package bundles are not.
const RUN_DATA: [(&str, &str); 13] = [
    ("tokenizer_config.json", "tokenizer"),
    ("tokenizer.json", "vocabulary"),
    ("vocab.json", "vocabulary"),
    ("merges.txt", "vocabulary"),
    ("tokenizer.model", "vocabulary"),
    ("spiece.model", "vocabulary"),
    ("vocab.txt", "vocabulary"),
    ("processor_config.json", "processor"),
    ("preprocessor_config.json", "processor"),
    ("video_preprocessor_config.json", "processor"),
    ("chat_template.jinja", "chat template"),
    ("chat_template.json", "chat template"),
    ("timestep-plan.json", "timestep plan"),
];

/// A checkpoint's pipeline and components, as a slot reads them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Parts {
    /// Its pipeline's class, `library.Class`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,
    #[serde(default)]
    pub components: BTreeMap<String, Part>,
}

/// One component.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Part {
    /// `library.Class`; empty: unknown.
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub tensors: bool,
    /// The modules its tensors fill, each with a digest of its tensors' keys and shapes. A module is a key's path to its first
    /// numbered segment, at most two deep (`model.visual`, `mtp.layers`); a top-level tensor (`lm_head.weight`, which a model with
    /// tied embeddings leaves out) fills none.
    #[serde(default)]
    pub modules: BTreeMap<String, String>,
    /// Its run data by kind, under its folder (`processor/tokenizer`).
    #[serde(default)]
    pub data: BTreeSet<String>,
    #[serde(default)]
    pub config: bool,
    /// What a LoRA keeps beside its factors (a step-distilled one's per-step tables and heads), by key with its numbered
    /// segments as `N`: a slot whose default keeps them takes only a LoRA that keeps them too.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub kept: BTreeSet<String>,
}

/// How a slot reads a checkpoint, as its function declares it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reads<'a> {
    /// The components it fetches; none: every one, and the pipeline's class.
    pub components: &'a [String],
    /// An LM's token loop (`phase: decode`): only its words leave it, so its modules serve at any size.
    pub any_size: bool,
}

fn class(class: &ClassRef) -> String {
    match class.is_unknown() {
        true => String::new(),
        false => format!("{}.{}", class.library, class.name),
    }
}

fn module(key: &str) -> Option<String> {
    let path: Vec<&str> = key.split('.').collect();
    let path = &path[..path.len() - 1];
    if path.len() < 2 {
        return None;
    }
    let named: Vec<&str> = path.iter().take_while(|segment| !segment.bytes().all(|b| b.is_ascii_digit())).take(2).copied().collect();
    (!named.is_empty()).then(|| named.join("."))
}

fn kind(file: &str) -> Option<&'static str> {
    RUN_DATA.iter().find(|(name, _)| *name == file).map(|(_, kind)| *kind)
}

fn modules(tensors: &[Tensor]) -> BTreeMap<String, String> {
    let mut shapes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tensor in tensors {
        if let Some(module) = module(&tensor.key) {
            shapes.entry(module).or_default().push(format!("{} {:?}", tensor.key, tensor.shape));
        }
    }
    (shapes.into_iter())
        .map(|(module, mut lines)| {
            lines.sort();
            let digest = crate::sha256::hex_digest(lines.join("\n").as_bytes());
            (module, digest[..16].to_owned())
        })
        .collect()
}

impl Part {
    fn of(component: &Component) -> Part {
        let mut data: BTreeSet<String> = (component.assets.keys())
            .filter_map(|path| {
                let (folder, file) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
                let kind = kind(file)?;
                Some(if folder.is_empty() { kind.to_owned() } else { format!("{folder}/{kind}") })
            })
            .collect();
        // A folder's config file is the component's config: a processor's, for one.
        if let Some(kind) = component.config_file.as_deref().and_then(kind).filter(|_| !component.config.is_empty()) {
            data.insert(kind.to_owned());
        }
        Part {
            class: class(&component.class),
            tensors: !component.tensors.is_empty(),
            modules: modules(&component.tensors),
            data,
            config: !component.config.is_empty(),
            kept: kept(component),
        }
    }

    fn holds_anything(&self) -> bool {
        !self.class.is_empty() || self.tensors || !self.data.is_empty() || self.config
    }
}

/// A LoRA component's tensors besides its factors, by key with numbered segments as `N`; none for a component of no LoRA.
fn kept(component: &Component) -> BTreeSet<String> {
    if !super::paired(component) {
        return BTreeSet::new();
    }
    (component.tensors.iter())
        .filter(|tensor| !super::factor(&tensor.key))
        .map(|tensor| tensor.key.split('.').map(|segment| if segment.bytes().all(|b| b.is_ascii_digit()) { "N" } else { segment }).collect::<Vec<_>>().join("."))
        .collect()
}

impl Parts {
    pub fn of(header: &Header) -> Parts {
        Parts {
            pipeline: header.pipeline().map(|pipeline| class(&pipeline.class)).filter(|class| !class.is_empty()),
            components: header.components().iter().map(|component| (component.name.clone(), Part::of(component))).collect(),
        }
    }

    /// What `other`, a LoRA, lacks of what this one keeps beside its factors in the components `reads` names, in words; none when
    /// it keeps all of it. A LoRA slot whose default is step-distilled (H3 Turbo) takes no ordinary LoRA.
    pub fn keeps_lacking(&self, other: &Parts, reads: Reads<'_>) -> Option<String> {
        let read = |name: &str| reads.components.is_empty() || reads.components.iter().any(|held| held == name);
        for (name, part) in self.components.iter().filter(|(name, part)| !part.kept.is_empty() && read(name)) {
            let theirs = other.components.get(name).map(|theirs| &theirs.kept);
            let missing: Vec<&str> = part.kept.iter().filter(|key| !theirs.is_some_and(|held| held.contains(*key))).map(String::as_str).collect();
            if !missing.is_empty() {
                return Some(format!("it keeps no {name} {}{}", missing[..missing.len().min(3)].join(", "), if missing.len() > 3 { ", …" } else { "" }));
            }
        }
        None
    }

    /// What `other` lacks to load where this checkpoint loads, read as `reads` says, in words; none when it loads. A slot that
    /// names its components reads every component without weights (a processor, a scheduler) too.
    pub fn lacking(&self, other: &Parts, reads: Reads<'_>) -> Option<String> {
        let read = |name: &str, part: &Part| reads.components.is_empty() || !part.tensors || reads.components.iter().any(|held| held == name);
        if let Some(pipeline) = self.pipeline.as_ref().filter(|_| reads.components.is_empty()) {
            match &other.pipeline {
                None => return Some(format!("it has no {pipeline} pipeline")),
                Some(theirs) if theirs != pipeline => return Some(format!("its pipeline is {theirs}, not {pipeline}")),
                Some(_) => {}
            }
        }
        for (name, part) in self.components.iter().filter(|(name, part)| part.holds_anything() && read(name, part)) {
            let Some(theirs) = other.components.get(name) else { return Some(format!("it has no {name}")) };
            if theirs.class != part.class {
                let shown = |class: &str| if class.is_empty() { "of no known class".to_owned() } else { class.to_owned() };
                return Some(format!("its {name} is {}, not {}", shown(&theirs.class), shown(&part.class)));
            }
            if part.tensors && !theirs.tensors {
                return Some(format!("its {name} holds no weights"));
            }
            let missing: Vec<&str> = part.modules.keys().filter(|module| !theirs.modules.contains_key(*module)).map(String::as_str).collect();
            if !missing.is_empty() {
                return Some(format!("its {name} holds no {}", missing.join(", ")));
            }
            let resized: Vec<&str> = (part.modules.iter())
                .filter(|(module, digest)| !reads.any_size && theirs.modules.get(*module) != Some(*digest))
                .map(|(module, _)| module.as_str())
                .collect();
            if !resized.is_empty() {
                return Some(format!("its {name} has other shapes ({})", resized.join(", ")));
            }
            let missing: Vec<&str> = part.data.difference(&theirs.data).map(String::as_str).collect();
            if !missing.is_empty() {
                return Some(format!("its {name} carries no {}", missing.join(", ")));
            }
            if part.config && !theirs.config {
                return Some(format!("its {name} has no config"));
            }
        }
        None
    }
}
