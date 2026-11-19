//! An import decides each plain F32 tensor's width per component, from that component's own stored weights:
//! - A component whose weights are mostly BF16 stores its stray F32 tensors as BF16, unless the class it loads into
//!   keeps them at F32 by its own published rule.
//! - Any other component keeps its F32 as stored: a component stored in F32 is one its authors load in F32, and
//!   narrowing F32 to F16 can overflow.
//!
//! The rules are upstream's, generated into `keeps.json` (`scripts/keeps.py`), and matched as upstream's loaders match
//! them at BF16:
//! - Diffusers' `_keep_in_fp32_modules`: an entry equal to one of the key's dot-separated parts.
//! - Transformers' `_keep_in_fp32_modules_strict`: an entry found in the key, `*` standing for any run. Its
//!   `_keep_in_fp32_modules` applies at F16 only.
//!
//! A class the table does not hold (another library's, an adapter's, none) keeps every tensor as stored.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::engine::Planned;
use super::Folder;
use crate::format::{ClassRef, Dtype};

pub(crate) const KEEPS: &str = include_str!("../../keeps.json");

#[derive(Deserialize)]
struct Table {
    classes: BTreeMap<String, BTreeMap<String, Rule>>,
}

#[derive(Deserialize, Default)]
struct Rule {
    #[serde(default)]
    fp32: Vec<String>,
    #[serde(default)]
    strict: Vec<String>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| serde_json::from_str(KEEPS).expect("keeps.json is generated"));

/// Whether `class` keeps `key` at F32 when loaded at BF16; none for a class the table does not hold.
fn kept(class: &ClassRef, key: &str) -> Option<bool> {
    let rule = TABLE.classes.get(&class.library)?.get(&class.name)?;
    Some(match class.library.as_str() {
        "diffusers" => rule.fp32.iter().any(|m| key.split('.').any(|part| part == m)),
        _ => rule.strict.iter().any(|m| found(m, key)),
    })
}

/// `pattern` somewhere in `key`, each `*` any run.
fn found(pattern: &str, key: &str) -> bool {
    let first = pattern.split('*').next().unwrap_or_default();
    (0..=key.len()).filter(|i| key.is_char_boundary(*i) && key[*i..].starts_with(first)).any(|i| {
        let mut rest = &key[i + first.len()..];
        pattern.split('*').skip(1).all(|piece| match rest.find(piece) {
            Some(at) => {
                rest = &rest[at + piece.len()..];
                true
            }
            None => false,
        })
    })
}

/// The float holding most of each component's planned elements (an encoded tensor by its logical dtype); of two
/// holding the same, the wider.
fn widths(planned: &[Planned]) -> BTreeMap<String, Dtype> {
    let mut held: BTreeMap<&str, BTreeMap<&Dtype, u64>> = BTreeMap::new();
    for p in planned.iter().filter(|p| matches!(p.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32 | Dtype::F64)) {
        *held.entry(p.component.as_str()).or_default().entry(&p.dtype).or_default() += p.shape.iter().product::<u64>();
    }
    (held.into_iter())
        .filter_map(|(c, by)| Some((c.to_owned(), by.into_iter().max_by_key(|(_, n)| *n)?.0.clone())))
        .collect()
}

/// What became of one component's plain F32 tensors.
#[derive(Default)]
struct Done {
    narrowed: u64,
    /// The narrowed tensors' F32 bytes.
    bytes: u64,
    /// Kept by the class's rule.
    ruled: u64,
    /// Kept: the table holds no rule for the class.
    unknown: u64,
    /// Kept: not one range of the source's bytes (`Planned::narrow`).
    unread: u64,
    /// Kept: the component's own width isn't BF16.
    own: u64,
}

/// Narrows to BF16 every plain F32 tensor of a mostly-BF16 component that its class does not keep, and says in one
/// line what became of each component's F32 (none for a model with no BF16 component).
pub(crate) fn narrow(planned: &mut [Planned], components: &[Folder]) -> Option<String> {
    let widths = widths(planned);
    if !widths.values().any(|w| *w == Dtype::BF16) {
        return None;
    }
    let classes: BTreeMap<&str, &ClassRef> = components.iter().map(|c| (c.name.as_str(), &c.class)).collect();
    let mut done: BTreeMap<String, Done> = BTreeMap::new();
    for p in planned.iter_mut().filter(|p| p.dtype == Dtype::F32 && p.encoding.is_none()) {
        let entry = done.entry(p.component.clone()).or_default();
        if widths.get(&p.component) != Some(&Dtype::BF16) {
            entry.own += 1;
            continue;
        }
        let bytes = 4 * p.shape.iter().product::<u64>();
        match classes.get(p.component.as_str()).and_then(|c| kept(c, &p.key)) {
            None => entry.unknown += 1,
            Some(true) => entry.ruled += 1,
            Some(false) if p.narrow(&Dtype::BF16) => {
                entry.narrowed += 1;
                entry.bytes += bytes;
            }
            Some(false) => entry.unread += 1,
        }
    }
    let mb = |b: u64| b as f64 / 1e6;
    let said: Vec<String> = (done.into_iter())
        .map(|(c, d)| {
            let class = classes.get(c.as_str()).map_or("", |c| c.name.as_str());
            if d.own > 0 {
                let width = widths.get(&c).map_or("", Dtype::name);
                return format!("{c}: {} F32 tensors kept as stored, {width} being its component's own width", d.own);
            }
            if d.unknown > 0 {
                let n = d.unknown;
                return format!("{c}: {n} F32 tensors kept as stored: no published rule is known for its class ({class})");
            }
            let mut line = format!(
                "{c}: {} F32 tensors stored as BF16 ({:.1} MB to {:.1} MB), {} kept at F32 by {class}'s published rule",
                d.narrowed,
                mb(d.bytes),
                mb(d.bytes / 2),
                d.ruled
            );
            if d.unread > 0 {
                line += &format!(", {} kept as stored (not one range of the source's bytes)", d.unread);
            }
            line
        })
        .collect();
    (!said.is_empty()).then(|| format!("width: {}", said.join("; ")))
}
