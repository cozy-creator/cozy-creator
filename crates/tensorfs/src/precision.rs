//! What a header holds, as the lane rule's table reads it (`spec/selection.json`: kinds and their
//! families). The Hub vendors the table byte for byte and picks a release's lane for a machine's
//! hardware with it; here it names a commit's precision family and how GPUs are named.
use std::{collections::BTreeMap, sync::LazyLock};

use serde::{Deserialize, Serialize};

use crate::format::{Body, Dtype, Header};

/// `spec/selection.json`, the parts read here. Unknown fields are ignored.
#[derive(Deserialize)]
struct Table {
    kinds: Vec<Kind>,
    families: BTreeMap<String, Family>,
}

/// A kind (`kind` exactly, or any starting with `prefix`) and its family.
#[derive(Deserialize)]
struct Kind {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    prefix: String,
    family: String,
}

#[derive(Deserialize)]
struct Family {
    bits: u32,
}

static TABLE: LazyLock<Table> =
    LazyLock::new(|| serde_json::from_str(include_str!("../spec/selection.json")).expect("spec/selection.json is the rule's table"));

/// GPUs by name: `sm90`, `sm86+sm89`, `apple9`, or `no GPU`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpus {
    pub name: String,
    pub capabilities: Vec<String>,
}

impl Gpus {
    /// From each GPU's compute capability (`"9.0"`, `"apple9"`).
    pub fn of(ccs: &[&str]) -> Self {
        let named = |cc: &&str| match cc.starts_with("apple") {
            true => cc.to_string(),
            false => format!("sm{}", cc.replace('.', "")),
        };
        let mut names: Vec<String> = ccs.iter().filter(|cc| !cc.is_empty()).map(named).collect();
        names.sort();
        names.dedup();
        let name = if names.is_empty() { "no GPU".into() } else { names.join("+") };
        Self { name, capabilities: ccs.iter().map(|cc| cc.to_string()).collect() }
    }
}

/// A header as the rule reads it (spec §7): every kind it holds (an encoded tensor's scheme, a
/// plain one's dtype), its stored bytes, and its family: that of the narrowest kind of the table
/// it holds, with its width (`""`, 0 when it holds none).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub bits: u32,
    pub family: String,
    pub bytes: u64,
    pub kinds: Vec<String>,
}

impl Profile {
    pub fn of(header: &Header) -> Self {
        let mut held: BTreeMap<String, u64> = BTreeMap::new();
        for (_, tensor) in header.tensors() {
            let kind = match &tensor.body {
                Body::Encoded { encoding, .. } => encoding.scheme.clone(),
                Body::Data(_) => tensor.dtype.name().to_owned(),
            };
            *held.entry(kind).or_default() += tensor.locations().iter().map(|(_, at)| at.length).sum::<u64>();
        }
        // Its family is the narrowest of the table's widths it holds: an nvfp4 commit that keeps
        // its sensitive layers int8 is 4 bits, however many bytes those layers hold. Of that
        // width, the kind with the most bytes; of equal ones, the first name. An encoded kind the
        // table does not name leaves it none: its width is not this rule's to guess.
        let untabled = held.keys().any(|kind| matches!(Dtype::parse(kind), Dtype::Other(_)) && family(kind).1 == 0);
        let lane = held.iter().filter(|(kind, _)| !untabled && family(kind).1 > 0)
            .min_by_key(|(kind, n)| (family(kind).1, std::cmp::Reverse(**n), kind.as_str()));
        let (family, bits) = lane.map_or(("", 0), |(kind, _)| family(kind));
        Self { bits, family: family.into(), bytes: held.values().sum(), kinds: held.into_keys().collect() }
    }
}

/// A kind's family and width (the table's `kinds`); `("", 0)` outside it.
fn family(kind: &str) -> (&'static str, u32) {
    let found = TABLE.kinds.iter().find(|k| !k.kind.is_empty() && k.kind == kind || !k.prefix.is_empty() && kind.starts_with(&k.prefix));
    found.map_or(("", 0), |k| (k.family.as_str(), TABLE.families.get(&k.family).map_or(0, |f| f.bits)))
}
