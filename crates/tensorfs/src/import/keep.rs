//! An import stores each plain F32 tensor at its model's compute width, BF16 when that is the float holding most of
//! its weights, unless the class its component loads into keeps it at F32 by its own published rule. The rules are
//! upstream's, generated into `keeps.json` (`scripts/keeps.py`), and matched as upstream's loaders match them at BF16:
//! - Diffusers' `_keep_in_fp32_modules`: an entry equal to one of the key's dot-separated parts.
//! - Transformers' `_keep_in_fp32_modules_strict`: an entry found in the key, `*` standing for any run. Its
//!   `_keep_in_fp32_modules` applies at F16 only.
//!
//! A class the table does not hold (another library's, an adapter's, none) keeps every tensor as stored, and so does a
//! model whose weights are mostly F16 or F32: narrowing F32 to F16 can overflow.

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

/// Whether `class` keeps `key` at F32 in a BF16 model; none for a class the table does not hold.
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

/// Whether BF16 holds more of the planned weights' elements than any other float (an encoded tensor by its logical
/// dtype).
fn bf16(planned: &[Planned]) -> bool {
    let mut held: BTreeMap<&str, u64> = BTreeMap::new();
    for p in planned.iter().filter(|p| matches!(p.dtype, Dtype::BF16 | Dtype::F16 | Dtype::F32 | Dtype::F64)) {
        *held.entry(p.dtype.name()).or_default() += p.shape.iter().product::<u64>();
    }
    held.into_iter().max_by_key(|(_, n)| *n).is_some_and(|(name, _)| name == Dtype::BF16.name())
}

/// Narrows every plain F32 tensor of a BF16 model that its component's class does not keep to BF16, and says what it
/// did, a line a component.
pub(crate) fn narrow(planned: &mut [Planned], components: &[Folder]) -> Vec<String> {
    if !bf16(planned) {
        return vec![];
    }
    let classes: BTreeMap<&str, &ClassRef> = components.iter().map(|c| (c.name.as_str(), &c.class)).collect();
    // component → (narrowed, their F32 bytes, kept by the rule, kept for want of one)
    let mut done: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    for p in planned.iter_mut().filter(|p| p.dtype == Dtype::F32 && p.encoding.is_none()) {
        let class = classes.get(p.component.as_str()).map_or_else(ClassRef::default, |c| (*c).clone());
        let bytes = 4 * p.shape.iter().product::<u64>();
        let entry = done.entry(p.component.clone()).or_default();
        let rule = kept(&class, &p.key);
        if rule == Some(false) && p.narrow(&Dtype::BF16) {
            entry.0 += 1;
            entry.1 += bytes;
        } else if rule.is_some() {
            entry.2 += 1;
        } else {
            entry.3 += 1;
        }
    }
    let class = |c: &str| classes.get(c).map_or_else(String::new, |c| c.name.clone());
    let mb = |b: u64| b as f64 / 1e6;
    (done.into_iter())
        .map(|(c, (n, bytes, kept, unknown))| match unknown {
            0 => format!(
                "width: {c}: {n} F32 tensors stored as BF16 ({:.1} MB to {:.1} MB), {kept} kept at F32 by {}'s published rule",
                mb(bytes),
                mb(bytes / 2),
                class(&c)
            ),
            _ => format!("width: {c}: {unknown} F32 tensors kept as stored: no published rule is known for its class ({})", class(&c)),
        })
        .collect()
}
