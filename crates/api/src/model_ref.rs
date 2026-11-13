//! The one model-reference grammar, shared by `cozy model …`, `cozy run model.<slot>=…` and
//! `--lora`, and by a package's child calls (`ModelChoice(ref=…, lora=[…])`):
//!
//! ```text
//! [org/]name[@release]          a Hub model by name (no org: first-party, the root's); the Hub
//!                               resolves the release (none: the newest) and its lane
//! [org/]name@sha256:<64 hex>    one of the model's checkpoints by id
//! local/<alias>                 a model a machine holds under that name
//! hf://… civitai://… https://…  a provider source, passed through: the machine pins and makes it
//! ```
use std::fmt;

use crate::error::{Error, Kind};
use crate::machine::{Adapter, ModelChoice};
use crate::names::ROOT;

/// A named model, `org/name`, lowercased (names are case-insensitive); a root one's org is
/// [`ROOT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelId {
    pub org: String,
    pub name: String,
}

/// Whether a named model supplies base weights or an adaptation to another model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Base,
    Lora,
}

impl ModelKind {
    /// Read older catalog/machine roles without treating unknown content as a base model.
    pub fn from_role(role: &str) -> Option<Self> {
        match role {
            "base" | "model" => Some(Self::Base),
            "lora" => Some(Self::Lora),
            _ => None,
        }
    }
}

/// Whether a version is available publicly or only to authorized model owners.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ReleaseVisibility {
    #[default]
    Public,
    Private,
}

/// The immutable tree identity of a checkpoint, distinct from its header digest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct CheckpointId(String);

impl CheckpointId {
    pub fn parse(value: &str) -> Result<Self, Error> {
        checkpoint(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for CheckpointId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&value).map_err(|error| serde::de::Error::custom(error.message))
    }
}

/// One parsed model reference: a Hub release, a checkpoint, a local alias or a provider source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelRef {
    Hub {
        model: ModelId,
        release: Option<String>,
    },
    Checkpoint {
        model: ModelId,
        checkpoint: CheckpointId,
    },
    Local(String),
    Provider(String),
}

const GRAMMAR: &str = "[org/]name[@release], [org/]name@sha256:<checkpoint>, local/<alias>, hf://…, civitai://… or https://…";

fn usage(code: &str, message: String) -> Error {
    Error::new(Kind::Usage, code, message)
}

/// A Hub name segment: 1–64 of `a-z0-9._-`, alphanumeric at both ends.
fn slug(part: &str) -> bool {
    let b = part.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// A release or lane label: alphanumeric first, then up to 63 of `A-Za-z0-9._+!-`.
pub fn label(flag: &str, value: &str) -> Result<String, Error> {
    match tensorfs::format::label(value) {
        true => Ok(value.to_owned()),
        false => Err(usage(
            "model_label_invalid",
            format!("{flag} {value:?}: start alphanumeric, then at most 63 of letters, digits, . _ + ! -"),
        )),
    }
}

/// `--lane`, as clap validates it.
pub fn lane(value: &str) -> Result<String, String> {
    label("--lane", value).map_err(|e| e.message)
}

/// `REF[:COMPONENT,…]`: the reference and the components it names, none when it names none. The last `:` starts them when each
/// is a component name (a letter, then letters, digits and `._-`), unless it is a checkpoint's `sha256:`, a held file's or a
/// run's (`run:N`); a URL's port or path is never one.
pub fn components(raw: &str) -> (&str, Vec<String>) {
    let name = |s: &str| s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    match raw.rsplit_once(':') {
        Some((head, tail)) if !head.is_empty() && !head.ends_with("@sha256") && !["sha256", "run"].contains(&head)
            && tail.split(',').all(name) => (head, tail.split(',').map(str::to_owned).collect()),
        _ => (raw, vec![]),
    }
}

/// A checkpoint id: `sha256:` and 64 lowercase hex digits.
pub fn checkpoint(value: &str) -> Result<String, Error> {
    match value.strip_prefix("sha256:") {
        Some(hex)
            if hex.len() == 64
                && hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) =>
        {
            Ok(value.to_owned())
        }
        _ => Err(usage(
            "checkpoint_invalid",
            format!("{value:?} is not a checkpoint id"),
        )
        .remedy("write sha256:<64 lowercase hex>")),
    }
}

impl ModelId {
    /// `name` or `org/name`, lowercased; no org is [`ROOT`].
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let lower = raw.trim().to_ascii_lowercase();
        match lower.split_once('/').unwrap_or((ROOT, &lower)) {
            (org, name) if (slug(org) || org == ROOT) && slug(name) => Ok(ModelId {
                org: org.to_owned(),
                name: name.to_owned(),
            }),
            _ => Err(usage(
                "model_name_invalid",
                format!("{raw:?} is not name or org/name"),
            )
            .remedy("org and name use letters, digits, dots, underscores or hyphens")),
        }
    }

    /// A destination on the Hub: never `local/`.
    pub fn hub(raw: &str) -> Result<Self, Error> {
        let model = Self::parse(raw)?;
        if model.org == "local" {
            return Err(usage(
                "model_local_destination",
                format!("{raw} is a local alias; local/ names stay on a machine"),
            )
            .remedy(format!("name a model in your account, e.g. <you>/{}", model.name)));
        }
        Ok(model)
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.name)
    }
}

impl ModelRef {
    /// One reference in this module's grammar; `@latest` is the same as naming no release.
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let raw = raw.trim();
        if let Some((scheme, _)) = raw.split_once("://") {
            return match scheme {
                "hf" | "civitai" | "https" => Ok(ModelRef::Provider(raw.to_owned())),
                _ => Err(usage(
                    "model_source_scheme_unsupported",
                    format!("{raw:?} uses neither hf://, civitai:// nor https://"),
                )),
            };
        }
        let (name, pin) = match raw.split_once('@') {
            Some((name, pin)) => (name, Some(pin)),
            None => (raw, None),
        };
        let model = ModelId::parse(name).map_err(|_| grammar(raw))?;
        if model.org == "local" {
            return match pin {
                None => Ok(ModelRef::Local(model.name)),
                Some(_) => Err(usage(
                    "model_ref_invalid",
                    format!("{raw:?}: a local alias has no releases"),
                )),
            };
        }
        if let Some(id) = pin.filter(|pin| pin.starts_with("sha256:")) {
            return Ok(ModelRef::Checkpoint { model, checkpoint: CheckpointId::parse(id)? });
        }
        // `@latest` is what naming no release means.
        let release = pin.filter(|pin| !pin.eq_ignore_ascii_case("latest")).map(|release| label("release", release)).transpose()?;
        Ok(ModelRef::Hub { model, release })
    }

    /// The machine's choice for `parameter`.
    pub fn choice(&self, parameter: &str) -> ModelChoice {
        let mut choice = ModelChoice {
            parameter: parameter.to_owned(),
            ..Default::default()
        };
        match self {
            ModelRef::Hub { model, release } => {
                choice.model = model.to_string();
                choice.release = release.clone().unwrap_or_default();
            }
            ModelRef::Checkpoint { model, checkpoint } => {
                choice.model = model.to_string();
                choice.manifest = checkpoint.to_string();
            }
            ModelRef::Local(alias) => choice.model = format!("local/{alias}"),
            ModelRef::Provider(source) => choice.source = source.clone(),
        }
        choice
    }

    /// A LoRA of this model at `strength` (a [`strength`]), as the machine applies it.
    pub fn adapter(&self, strength: &str) -> Adapter {
        let named = self.choice("");
        Adapter {
            model: named.model,
            release: named.release,
            lane: named.lane,
            manifest: named.manifest,
            scale: strength.to_owned(),
            source: named.source,
            ..Default::default()
        }
    }
}

/// A LoRA strength as a canonical decimal string: finite, `0` for zero.
pub fn strength(raw: &str) -> Result<String, Error> {
    let malformed = || {
        usage(
            "lora_strength_invalid",
            format!("LoRA strength {raw:?} must be a finite decimal number"),
        )
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c)) {
        return Err(malformed());
    }
    let value: f64 = trimmed.parse().map_err(|_| malformed())?;
    if !value.is_finite() {
        return Err(malformed());
    }
    Ok(if value == 0.0 { "0".into() } else { format!("{value}") })
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelRef::Hub { model, release } => {
                write!(f, "{model}")?;
                if let Some(release) = release {
                    write!(f, "@{release}")?;
                }
                Ok(())
            }
            ModelRef::Checkpoint { model, checkpoint } => write!(f, "{model}@{checkpoint}"),
            ModelRef::Local(alias) => write!(f, "local/{alias}"),
            ModelRef::Provider(source) => f.write_str(source),
        }
    }
}

fn grammar(raw: &str) -> Error {
    usage(
        "model_ref_invalid",
        format!("{raw:?} is not a model: use {GRAMMAR}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_model_and_exact_checkpoint_keep_the_reference_contract() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let reference = ModelRef::parse(&format!("Author/Model@{digest}")).unwrap();
        let ModelRef::Checkpoint { model, checkpoint } = &reference else { panic!("expected checkpoint") };
        assert_eq!(model.to_string(), "author/model");
        assert_eq!(checkpoint.as_str(), digest);
        assert_eq!(reference.to_string(), format!("author/model@{digest}"));
        let choice = reference.choice("weights");
        assert_eq!(choice.model, "author/model");
        assert_eq!(choice.manifest, digest);
        assert_eq!(ModelId::parse("flux.2").unwrap().to_string(), "_/flux.2");
        assert_eq!(ModelRef::parse("local/adapter").unwrap().to_string(), "local/adapter");
        assert!(ModelId::hub("local/adapter").is_err());
    }

    #[test]
    fn checkpoint_ids_are_validated_when_read_from_json() {
        let digest = format!("sha256:{}", "b".repeat(64));
        let json = serde_json::to_string(&CheckpointId::parse(&digest).unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<CheckpointId>(&json).unwrap().as_str(), digest);
        for invalid in ["sha256:1234", "sha256:ABCDEF", "author/model@1.0", ""] {
            assert!(serde_json::from_str::<CheckpointId>(&serde_json::to_string(invalid).unwrap()).is_err());
        }
    }

    #[test]
    fn unknown_roles_do_not_become_base_models() {
        assert_eq!(ModelKind::from_role("model"), Some(ModelKind::Base));
        assert_eq!(ModelKind::from_role("base"), Some(ModelKind::Base));
        assert_eq!(ModelKind::from_role("lora"), Some(ModelKind::Lora));
        assert_eq!(ModelKind::from_role("unknown"), None);
        assert_eq!(serde_json::to_string(&ModelKind::Base).unwrap(), "\"base\"");
    }
}

#[cfg(test)]
mod protocol_names_tests {
    use super::*;

    #[test]
    fn renamed_source_fields_keep_the_supported_machine_protocol() {
        let value = serde_json::to_value(crate::machine::ModelChoice { model: "author/model".into(), ..Default::default() }).unwrap();
        assert_eq!(value["repository"], "author/model");
        assert!(value.get("model").is_none());
        let removed = serde_json::to_value(crate::machine::RemoveModel { model: "author/model".into() }).unwrap();
        assert_eq!(removed["model"], "author/model");
        assert!(removed.get("repository").is_none());
        let reference = crate::machine::CheckpointRef { model_id: "author/model".into(), revision: "sha256:abc".into(), lane: "bf16".into() };
        let encoded = serde_json::to_value(reference).unwrap();
        assert_eq!(encoded["repo_id"], "author/model");
        let legacy = serde_json::json!({"repository": "author/model", "manifest": "sha256:abc"});
        let read: ModelChoice = serde_json::from_value(legacy).unwrap();
        assert_eq!(read.model, "author/model");
    }
}
