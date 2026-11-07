//! The lane rule (design `lane-choice.md`, `precision-tiers.md` §2.2; spec §7 "Selection"): which
//! lane of a release this machine serves, from what each lane's header holds, its GPUs' native
//! formats and capacity, what its store holds, and a slot's tolerances. Nothing is keyed by a
//! card's name or a lane's label. The table (kinds, families, GPU features, lane orders) is data,
//! `spec/selection.json`, which the Hub vendors for placement's estimate; this file is the rule.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::err::{refuse, Code, Result};
use crate::format::{Body, Dtype, Header, ObjectRef};

/// `spec/selection.json`. Unknown fields are ignored.
#[derive(Deserialize)]
struct Table {
    features: BTreeMap<String, Feature>,
    kinds: Vec<Kind>,
    families: BTreeMap<String, Family>,
    order: Vec<Order>,
    plain: Vec<Order>,
    schemes: Vec<String>,
}

/// A feature every GPU of a set has: its capability starts with `prefix`, or is from `min` on and
/// not at an `except`; `unread`: a capability that is not a number has it; `cpu`: no GPU has it.
#[derive(Deserialize)]
struct Feature {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    min: String,
    #[serde(default)]
    except: Vec<String>,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    cpu: bool,
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

/// A family's width, the feature every GPU needs to run it (empty: any), the width its matmuls
/// run at (the first `compute` row whose feature the GPUs have, else 16), and the lane it ranks
/// as (`after`: just after the order).
#[derive(Default, Deserialize)]
struct Family {
    bits: u32,
    #[serde(default)]
    needs: String,
    #[serde(default)]
    compute: Vec<Compute>,
    #[serde(default)]
    rank: String,
}

#[derive(Deserialize)]
struct Compute {
    #[serde(default)]
    when: String,
    width: u32,
}

/// A lane order: the first row whose bits, slot and feature match.
#[derive(Deserialize)]
struct Order {
    #[serde(default)]
    bits: Option<u32>,
    /// Only a decode slot's.
    #[serde(default)]
    decode: bool,
    #[serde(default)]
    when: String,
    lanes: Vec<String>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let table: Table = serde_json::from_str(include_str!("../spec/selection.json")).expect("spec/selection.json is the rule's table");
    debug_assert!(
        {
            let mut implemented: Vec<&str> = crate::format::SCHEMES.iter().map(|scheme| scheme.name).collect();
            implemented.sort_unstable();
            implemented == table.schemes.iter().map(String::as_str).collect::<Vec<_>>()
        },
        "spec/selection.json's schemes are not the ones this reader implements"
    );
    table
});

/// A compute capability `"major.minor"` as one number (`"8.9"`: 89); `None` when it is not one.
fn capability(cc: &str) -> Option<u32> {
    let (major, minor) = cc.split_once('.')?;
    if major.is_empty() || minor.is_empty() || major.len() > 4 || minor.len() > 4 || !major.bytes().chain(minor.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(major.parse::<u32>().ok()? * 10 + minor.parse::<u32>().ok()?)
}

impl Feature {
    fn held(&self, ccs: &[&str]) -> bool {
        let min = capability(&self.min).unwrap_or(u32::MAX);
        let except: Vec<u32> = self.except.iter().filter_map(|cc| capability(cc)).collect();
        let one = |cc: &&str| match self.prefix.is_empty() {
            false => cc.starts_with(&self.prefix),
            true => capability(cc).map_or(self.unread, |n| n >= min && !except.contains(&n)),
        };
        if ccs.is_empty() { self.cpu } else { ccs.iter().all(one) }
    }

    /// How a refusal names the GPUs that have it: `sm80+ except sm103`.
    fn named(&self) -> String {
        let sm = |cc: &String| format!("sm{}", cc.replace('.', ""));
        match self.except.is_empty() {
            true => format!("{}+", sm(&self.min)),
            false => format!("{}+ except {}", sm(&self.min), self.except.iter().map(sm).collect::<Vec<_>>().join(", ")),
        }
    }
}

/// The table's row for a family (the empty row for none).
fn row(family: &str) -> &'static Family {
    static NONE: LazyLock<Family> = LazyLock::new(Family::default);
    TABLE.families.get(family).unwrap_or(&NONE)
}

/// Whether the rule picks `family` by name; any other is a release's only lane, served silently.
fn named_lane(family: &str) -> bool {
    TABLE.order.iter().chain(&TABLE.plain).any(|order| order.lanes.iter().any(|l| l == family))
}

/// The GPUs a call runs on as the rule reads them: the table's features every one has (`mma`,
/// `int8`, `fp8`, `fp4`, `bf16`, `apple`), how a reason names them (`sm90`, `sm86+sm89`, `no GPU`),
/// and the smallest one's memory: its card's, or the machine's software cap when lower; never what
/// is free (none: unknown). An Apple GPU's capability is its Metal family (`apple9`).
#[derive(Clone, Debug, PartialEq)]
pub struct Gpus {
    pub features: BTreeSet<String>,
    pub name: String,
    pub capabilities: Vec<String>,
    pub capacity: Option<u64>,
}

/// No GPU: the CPU, which runs bf16 and decodes every encoded format.
impl Default for Gpus {
    fn default() -> Self {
        Self::of(&[])
    }
}

impl Gpus {
    /// From each GPU's compute capability (`"9.0"`, `"apple9"`; `""`: unread, which runs no
    /// encoded format).
    pub fn of(ccs: &[&str]) -> Self {
        let named = |cc: &&str| match cc.starts_with("apple") {
            true => cc.to_string(),
            false => format!("sm{}", cc.replace('.', "")),
        };
        let mut names: Vec<String> = ccs.iter().filter(|cc| !cc.is_empty()).map(named).collect();
        names.sort();
        names.dedup();
        let name = if names.is_empty() { "no GPU".into() } else { names.join("+") };
        Self {
            features: TABLE.features.iter().filter(|(_, feature)| feature.held(ccs)).map(|(name, _)| name.clone()).collect(),
            name,
            capabilities: ccs.iter().map(|cc| cc.to_string()).collect(),
            capacity: None,
        }
    }

    /// With the smallest GPU's memory (each GPU holds the weights whole).
    pub fn with_capacity(self, capacity: Option<u64>) -> Self {
        Self { capacity, ..self }
    }

    /// Whether every one has `feature` (the empty feature: always).
    pub fn has(&self, feature: &str) -> bool {
        feature.is_empty() || self.features.contains(feature)
    }

    /// The width a family's matmuls run at here (the table's `compute`): 4 where FP4 tensor cores
    /// run it, 8 for int8 and w4a8 and fp8 where fp8 tensor cores run it, else 16.
    pub fn compute(&self, family: &str) -> u32 {
        row(family).compute.iter().find(|c| self.has(&c.when)).map_or(16, |c| c.width)
    }

    fn cpu(&self) -> bool {
        self.capabilities.is_empty()
    }
}

/// A header as the rule reads it (spec §7): every kind it holds (an encoded tensor's scheme, a
/// plain one's dtype), its stored bytes, and its family: that of the narrowest kind of the table
/// it holds, with its width (`""`, 0 when it holds none, or an encoded kind the table lacks).
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
        // The narrowest of the table's widths it holds: an nvfp4 commit that keeps its sensitive
        // layers int8 is 4 bits, however many bytes those layers hold. Of that width, the kind
        // with the most bytes; of equal ones, the first name.
        let untabled = untabled(held.keys());
        let lane = held.iter().filter(|(kind, _)| !untabled && family(kind).1 > 0)
            .min_by_key(|(kind, n)| (family(kind).1, std::cmp::Reverse(**n), kind.as_str()));
        let (family, bits) = lane.map_or(("", 0), |(kind, _)| family(kind));
        Self { bits, family: family.into(), bytes: held.values().sum(), kinds: held.into_keys().collect() }
    }
}

/// Whether one of `kinds` is encoded in a scheme the table names no family for: its width is not
/// this rule's to guess.
fn untabled<'a>(mut kinds: impl Iterator<Item = &'a String>) -> bool {
    kinds.any(|kind| matches!(Dtype::parse(kind), Dtype::Other(_)) && family(kind).1 == 0)
}

/// Whether this table names a family for a header holding `kinds`: a Hub whose table did not
/// (skew) leaves the header to profile here.
pub fn tabled(kinds: &[String]) -> bool {
    !untabled(kinds.iter()) && kinds.iter().any(|kind| family(kind).1 > 0)
}

/// A family's weight width (16, 8 or 4; the table's `families`), 0 for one it does not name.
pub fn bits(family: &str) -> u32 {
    row(family).bits
}

/// A kind's family and width (the table's `kinds`); `("", 0)` outside it.
fn family(kind: &str) -> (&'static str, u32) {
    let found = TABLE.kinds.iter().find(|k| !k.kind.is_empty() && k.kind == kind || !k.prefix.is_empty() && kind.starts_with(&k.prefix));
    found.map_or(("", 0), |k| (k.family.as_str(), row(&k.family).bits))
}

fn implemented(kind: &str) -> bool {
    TABLE.schemes.iter().any(|scheme| scheme == kind)
}

/// Whether `gpus` run encoded `kind`: a scheme this reader implements, where every GPU has its
/// family's feature; the CPU decodes every one.
fn runs(kind: &str, gpus: &Gpus) -> bool {
    implemented(kind) && (gpus.cpu() || gpus.has(&row(family(kind).0).needs))
}

/// What a refusal says encoded `kind` needs.
fn needs(kind: &str) -> String {
    match TABLE.features.get(&row(family(kind).0).needs) {
        _ if !implemented(kind) => "a reader that implements it".into(),
        Some(feature) => feature.named(),
        None => "a GPU reader".into(),
    }
}

/// A lane of a release: its label, commit, header, profile, and whether this machine's store
/// holds it whole (never sent or journaled: each choice reads the store).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub label: String,
    pub commit: ObjectRef,
    pub header: ObjectRef,
    pub profile: Profile,
    #[serde(skip)]
    pub held: bool,
}

impl Lane {
    /// Its label, and its family where the label says otherwise.
    fn shown(&self) -> String {
        match self.profile.family.is_empty() || self.profile.family == self.label {
            true => self.label.clone(),
            false => format!("{} ({})", self.label, self.profile.family),
        }
    }
}

/// What a choice is asked: a `pin` (a lane label, only it serves: run `--lane` for the main
/// model, `cozy model download --lane`, `tensorfs.open(lane=)`), else the slot's tolerances: the
/// fewest bits the machine picks by itself (16, 8 or 4), whether it is a decode loop, whether it
/// takes plain weights only; and `offline`: only lanes the store holds serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub pin: Option<String>,
    pub min_bits: u32,
    pub decode: bool,
    pub plain: bool,
    pub offline: bool,
}

impl Default for Ask {
    fn default() -> Self {
        Self { pin: None, min_bits: 8, decode: false, plain: false, offline: false }
    }
}

impl Ask {
    /// A package slot's (its interface's `min_bits`, 8 when omitted, and `phase`): plain lanes only
    /// for a class that takes no encoded leaves, or raw weights read as data.
    pub fn of(slot: &Value) -> Self {
        Self {
            min_bits: match slot["min_bits"].as_u64() {
                Some(16) => 16,
                Some(4) => 4,
                _ => 8,
            },
            decode: slot["phase"] == "decode",
            plain: slot["encoded_leaves"] != "accept" || slot["weights"] == true,
            ..Self::default()
        }
    }

    /// The lanes the rule wants at `width`, in order (the table's `order`): the width's encoded
    /// lanes every GPU runs, unless plain, then the table's `plain` order.
    fn order(&self, width: u32, gpus: &Gpus) -> Vec<&'static str> {
        let first = |orders: &'static [Order], bits: Option<u32>| {
            orders.iter().find(|o| o.bits == bits && (!o.decode || self.decode) && gpus.has(&o.when)).map_or(&[][..], |o| &o.lanes[..])
        };
        let encoded = first(&TABLE.order, Some(width)).iter().filter(|lane| !self.plain && gpus.has(&row(lane).needs));
        encoded.chain(first(&TABLE.plain, None)).map(String::as_str).collect()
    }

    /// Why a lane of profile `p` does not serve here, or `None`: encoded weights where none are
    /// taken, or an encoding the GPUs do not run.
    fn refusal(&self, p: &Profile, gpus: &Gpus) -> Option<String> {
        let encoded: Vec<&str> = p.kinds.iter().map(String::as_str).filter(|kind| matches!(Dtype::parse(kind), Dtype::Other(_))).collect();
        if self.plain && !encoded.is_empty() {
            return Some(format!("holds encoded {}", encoded.join(", ")));
        }
        let unrun: Vec<String> = encoded.iter().filter(|kind| !runs(kind, gpus)).map(|kind| format!("{} for {kind}", needs(kind))).collect();
        (!unrun.is_empty()).then(|| format!("needs {}", unrun.join(" and ")))
    }
}

/// A choice: the release, its lane served, and why that is not what the rule wanted first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub release: String,
    pub lane: Lane,
    pub reasons: Vec<String>,
}

/// Its place in `order` (a family ranks as its `rank`; `after`: just after the order; outside it
/// after every one in it), then narrower than `width`, weight-only, not held, bytes, label: the least first.
fn rank<'a>(order: &[&str], width: u32, gpus: &Gpus, lane: &'a Lane) -> (usize, bool, bool, bool, u64, &'a str) {
    let p = &lane.profile;
    let place = match row(&p.family).rank.as_str() {
        "after" => Some(order.len()),
        "" => order.iter().position(|f| *f == p.family),
        rank => order.iter().position(|f| *f == rank),
    };
    (place.unwrap_or(order.len() + 1), p.bits > 0 && p.bits < width, weight_only(p, gpus), !lane.held, p.bytes, lane.label.as_str())
}

/// An encoded lane whose matmuls run at 16 bits here.
fn weight_only(p: &Profile, gpus: &Gpus) -> bool {
    p.bits > 0 && p.bits < 16 && gpus.compute(&p.family) == 16
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// The lane of `model`'s release `release` (of `lanes`) this machine serves for `ask` on `gpus`
/// (design lane-choice, "New flow"; Paul 11-06: the fastest within the quality floor), and why it
/// is not the one the rule wanted first. A pin serves its lane. Otherwise, of the lanes the GPUs
/// run (plain ones only for a plain slot; held ones only offline), none narrower than `min_bits`
/// while one meets it (else the widest, warned: a floor is a preference, 11-03), weight-only ones
/// only when no native one runs (a decode loop is bytes-bound: it keeps them): the first in the
/// order for its width, which is 16 at `min_bits` 16; 8 at 8; and at 4, 4 for a decode loop, on
/// FP4 tensor cores, or when the 8-bit lane is larger than the smallest GPU, else 8. Held breaks
/// ties only. Memory never lowers a lane otherwise: paging fits weights (10-22).
pub fn choose(model: &str, release: &str, lanes: &[Lane], gpus: &Gpus, ask: &Ask) -> Result<Chosen> {
    let named = if release.is_empty() { model.to_owned() } else { format!("{model}@{release}") };
    let chosen = |lane: &Lane, reasons: Vec<String>| -> Result<Chosen> { Ok(Chosen { release: release.to_owned(), lane: lane.clone(), reasons }) };
    if let Some(pin) = &ask.pin {
        let labels: Vec<&str> = lanes.iter().map(|l| l.label.as_str()).collect();
        return match lanes.iter().find(|l| l.label == *pin) {
            None => refuse(Code::REF_NOT_FOUND, format!("{named} has no lane {pin}: name one of {}", labels.join(", "))),
            Some(lane) if ask.offline && !lane.held => refuse(Code::OBJECT_ABSENT,
                format!("{named}'s {pin} lane is not on this machine: `cozy model download {model} --lane {pin}` while the Hub is reachable")),
            Some(lane) => chosen(lane, vec![]),
        };
    }
    let candidates: Vec<&Lane> = lanes.iter().filter(|l| !ask.offline || l.held).collect();
    if candidates.is_empty() {
        return match ask.offline {
            true => refuse(Code::OBJECT_ABSENT, format!("no lane of {named} is on this machine: `cozy model download {model}` while the Hub is reachable")),
            false => refuse(Code::REF_NOT_FOUND, format!("{named} has no lanes")),
        };
    }
    let mut runs: Vec<&Lane> = candidates.iter().copied().filter(|l| ask.refusal(&l.profile, gpus).is_none()).collect();
    if runs.is_empty() {
        let each: Vec<String> = candidates.iter().map(|l| format!("{} {}", l.shown(), ask.refusal(&l.profile, gpus).unwrap_or_default())).collect();
        return refuse(Code::CAPABILITY_UNQUALIFIED, format!(
            "{named} has no lane that runs on {}: {}; pin one with --lane <label>", gpus.name, each.join("; ")));
    }
    // The floor holds while a lane meets it; else the widest that runs serves.
    let below = |l: &&Lane| l.profile.bits > 0 && l.profile.bits < ask.min_bits;
    let short = runs.iter().all(below).then(|| runs.iter().map(|l| l.profile.bits).max().unwrap_or_default());
    runs.retain(|l| short.map_or(!below(l), |widest| l.profile.bits == widest));
    if !ask.decode && runs.iter().any(|l| !weight_only(&l.profile, gpus)) {
        runs.retain(|l| !weight_only(&l.profile, gpus));
    }
    let best = |width: u32| {
        let order = ask.order(width, gpus);
        let lane = runs.iter().copied().min_by_key(|l| rank(&order, width, gpus, l)).expect("a lane runs");
        (lane, order)
    };
    // Why 4 bits are worth it here: bytes bound a decode loop; FP4 tensor cores run them fastest;
    // the 8-bit lane is larger than a GPU.
    let four = match ask.min_bits {
        4 if ask.decode => Some("decode".to_owned()),
        4 if gpus.has("fp4") => Some("FP4 tensor cores".to_owned()),
        4 => gpus.capacity.and_then(|capacity| {
            let (eight, _) = best(8);
            (eight.profile.bytes > capacity).then(|| format!("{} lane {} > {} card", eight.label, gb(eight.profile.bytes), gb(capacity)))
        }),
        _ => None,
    };
    let width = match (ask.min_bits, short) {
        // An Apple GPU's only width is 16 bits, until Metal kernels exist.
        _ if gpus.has("apple") => 16,
        (_, Some(widest)) => widest,
        (16, _) => 16,
        _ if four.is_some() => 4,
        _ => 8,
    };
    let (served, order) = best(width);
    let shown = served.shown();
    let family = served.profile.family.as_str();
    let mut reasons = vec![];
    if short.is_some() {
        reasons.push(format!("no lane meets min_bits {}; served {shown}", ask.min_bits));
    }
    if let (Some(why), 4) = (&four, served.profile.bits) {
        reasons.push(format!("served {shown}: min_bits 4, {why}"));
    }
    // What the order wanted ahead of it that the release does not have (a lane it has but passed
    // over is not missing; a model of one lane has nothing to pass over).
    let ahead = rank(&order, width, gpus, served).0.min(order.len());
    let missing: Vec<&str> = order[..ahead].iter().copied().filter(|f| !lanes.iter().any(|l| l.profile.family == *f)).collect();
    if let (Some(last), true) = (missing.last(), named_lane(family) && lanes.len() > 1 && !ask.offline) {
        let list = match missing.len() {
            1 => last.to_string(),
            n => format!("{} or {last}", missing[..n - 1].join(", ")),
        };
        reasons.push(format!("no {list} lane for {}; served {shown}", gpus.name));
    }
    if !ask.decode && weight_only(&served.profile, gpus) {
        reasons.push(format!("{} has no tensor cores for {family}: served {shown} weight-only", gpus.name));
    }
    if family == "fp16" && !gpus.has("bf16") {
        reasons.push(format!("{} runs no bf16; served {shown}", gpus.name));
    }
    chosen(served, reasons)
}
