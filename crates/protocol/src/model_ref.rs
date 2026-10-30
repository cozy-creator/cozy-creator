//! The one model-reference grammar, shared by `cozy model …`, `cozy run model.<slot>=…` and
//! `--lora`, and by a package's child calls (`ModelChoice(ref=…, lora=[…])`):
//!
//! ```text
//! [org/]name[@release]          a Hub model by name (no org: first-party, the root's); the Hub
//!                               resolves the release (none: the newest) and its lane
//! [org/]name@sha256:<64 hex>    one of the model's commits by id
//! local/<alias>                 a model a machine holds under that name
//! hf://… civitai://… https://…  a provider source, passed through: the machine pins and makes it
//! ```
use std::fmt;

use crate::error::{Error, Kind};
use crate::machine::{Adapter, ModelChoice};
use crate::names::ROOT;

/// A Hub repository, `org/name`, lowercased (names are case-insensitive); a root one's org is
/// [`ROOT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub org: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelRef {
    Hub {
        repo: Repo,
        release: Option<String>,
    },
    Commit {
        repo: Repo,
        commit: String,
    },
    Local(String),
    Provider(String),
}

const GRAMMAR: &str = "[org/]name[@release], [org/]name@sha256:<commit>, local/<alias>, hf://…, civitai://… or https://…";

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

/// A commit id: `sha256:` and 64 lowercase hex digits.
pub fn commit(value: &str) -> Result<String, Error> {
    match value.strip_prefix("sha256:") {
        Some(hex)
            if hex.len() == 64
                && hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) =>
        {
            Ok(value.to_owned())
        }
        _ => Err(usage(
            "checkpoint_invalid",
            format!("{value:?} is not a commit id"),
        )
        .remedy("write sha256:<64 lowercase hex>")),
    }
}

impl Repo {
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let lower = raw.trim().to_ascii_lowercase();
        match lower.split_once('/').unwrap_or((ROOT, &lower)) {
            (org, name) if (slug(org) || org == ROOT) && slug(name) => Ok(Repo {
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
        let repo = Self::parse(raw)?;
        if repo.org == "local" {
            return Err(usage(
                "model_local_destination",
                format!("{raw} is a local alias; local/ names stay on a machine"),
            )
            .remedy(format!("name a repository of your account, e.g. <you>/{}", repo.name)));
        }
        Ok(repo)
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.name)
    }
}

impl ModelRef {
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
        let repo = Repo::parse(name).map_err(|_| grammar(raw))?;
        if repo.org == "local" {
            return match pin {
                None => Ok(ModelRef::Local(repo.name)),
                Some(_) => Err(usage(
                    "model_ref_invalid",
                    format!("{raw:?}: a local alias has no releases"),
                )),
            };
        }
        if let Some(id) = pin.filter(|pin| pin.starts_with("sha256:")) {
            return Ok(ModelRef::Commit { repo, commit: commit(id)? });
        }
        let release = pin.map(|release| label("release", release)).transpose()?;
        Ok(ModelRef::Hub { repo, release })
    }

    /// The machine's choice for `parameter`.
    pub fn choice(&self, parameter: &str) -> ModelChoice {
        let mut choice = ModelChoice {
            parameter: parameter.to_owned(),
            ..Default::default()
        };
        match self {
            ModelRef::Hub { repo, release } => {
                choice.repository = repo.to_string();
                choice.release = release.clone().unwrap_or_default();
            }
            ModelRef::Commit { repo, commit } => {
                choice.repository = repo.to_string();
                choice.manifest = commit.clone();
            }
            ModelRef::Local(alias) => choice.repository = format!("local/{alias}"),
            ModelRef::Provider(source) => choice.source = source.clone(),
        }
        choice
    }

    /// A LoRA of this model at `strength` (a [`strength`]), as the machine applies it.
    pub fn adapter(&self, strength: &str) -> Adapter {
        let named = self.choice("");
        Adapter {
            model: named.repository,
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
            ModelRef::Hub { repo, release } => {
                write!(f, "{repo}")?;
                if let Some(release) = release {
                    write!(f, "@{release}")?;
                }
                Ok(())
            }
            ModelRef::Commit { repo, commit } => write!(f, "{repo}@{commit}"),
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
