//! Transformers' stacked experts, one tensor per expert. A mixture of experts in Transformers' fused
//! layout stores each layer's experts as two banks, `….experts.gate_up_proj` [E, 2I, H] and
//! `….experts.down_proj` [E, H, I]; cozy-runtime's `moe.split` builds one `RoutedExpert` per expert
//! (`gate_and_up_proj` [2I, H], `down_proj` [H, I]) and pages them one at a time, so the checkpoint
//! stores them that way: `….experts.{e}.gate_and_up_proj.weight` and `….experts.{e}.down_proj.weight`.
//! One rule for every model in that layout, keyed by the layout and its folder's `config.json` (the
//! expert count and hidden width a bank must have), never by a checkpoint. Each expert is a byte range
//! of its bank (dim 0 of a row-major tensor), so nothing is read or copied.
//!
//! A layer's banks split only as a pair: both in that layout and agreeing on the expert width I. One bank
//! alone can have a split's shape in another layout (gpt-oss stores down [E, I, H], square when I = H), and
//! splitting it would name each expert's weight transposed. A bank left fused is said, with why: a loader
//! that reads the banks fused still can.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::source::{SourceSet, Tensor};
use crate::err::Result;
use crate::format::CONFIG_FILE;

/// The format an import that split banks names (`SourceFormat`).
pub(crate) const FORMAT: &str = "transformers.stacked-experts";
/// This rule's identity in the tables' digest (`engine::tables_digest`): changed whenever what it writes is.
pub(crate) const RULE: &str = "transformers.stacked-experts/1";
const GATE_UP: &str = "gate_up_proj";
const DOWN: &str = "down_proj";
/// The fields a Transformers config states its routed expert count in, by family.
const EXPERT_COUNTS: [&str; 3] = ["num_experts", "num_local_experts", "n_routed_experts"];

/// What the rule did to a source.
#[derive(Default)]
pub(crate) struct Split {
    /// Whether it split any layer's banks.
    pub(crate) any: bool,
    /// Each fused bank it left, by why: the first layer's stem and how many layers more.
    pub(crate) left: BTreeMap<String, (String, usize)>,
}

impl Split {
    /// The import's warning for the banks left fused, if any.
    pub(crate) fn warning(&self) -> Option<String> {
        let each: Vec<String> = self.left.iter().map(|(why, (stem, more))| match more {
            0 => format!("{stem}.experts: {why}"),
            more => format!("{stem}.experts and {more} more: {why}"),
        }).collect();
        (!each.is_empty()).then(|| format!(
            "warning: Transformers' fused expert banks are kept fused, so only a loader that reads them fused can use them: {}",
            each.join("; ")
        ))
    }

    fn leave(&mut self, stem: &str, why: String) {
        self.left.entry(why).and_modify(|(_, more)| *more += 1).or_insert_with(|| (stem.to_owned(), 0));
    }
}

/// What a config states of its routed experts: their count E, the hidden width H and, where it says it, their width I.
struct Stated {
    experts: u64,
    hidden: u64,
    width: Option<u64>,
}

/// Every group's stacked banks split in place, a layer's pair where its folder's config states their shape.
pub(crate) fn split(set: &mut SourceSet) -> Result<Split> {
    let mut split = Split::default();
    for g in 0..set.groups.len() {
        let stems: BTreeSet<String> = set.groups[g].tensors.keys().filter_map(|key| bank_of(key).map(|(stem, _)| stem.to_owned())).collect();
        if stems.is_empty() {
            continue;
        }
        let folder = set.groups[g].folder.clone();
        let config = match set.plain.iter().position(|p| p.path == config_path(&folder)) {
            // A config that doesn't parse states nothing: the source keeps its banks as they are.
            Some(plain) => serde_json::from_slice(&set.read_plain(&set.plain[plain])?).unwrap_or(Value::Null),
            None => Value::Null,
        };
        let Some(stated) = stated(&config) else {
            let why = format!("its {} states no expert count ({}) and hidden_size", config_path(&folder), EXPERT_COUNTS.join(", "));
            stems.iter().for_each(|stem| split.leave(stem, why.clone()));
            continue;
        };
        let group = &mut set.groups[g];
        for stem in stems {
            let key = |role: &str| format!("{stem}.experts.{role}");
            let (gate_up, down) = (group.tensors.get(&key(GATE_UP)), group.tensors.get(&key(DOWN)));
            match pair(gate_up, down, &stated) {
                Err(why) => split.leave(&stem, why),
                Ok(()) => {
                    for (role, linear) in [(GATE_UP, "gate_and_up_proj"), (DOWN, "down_proj")] {
                        let bank = group.tensors.remove(&key(role)).expect("a bank just paired");
                        let each = bank.len / stated.experts;
                        for e in 0..stated.experts {
                            let tensor = Tensor { shape: bank.shape[1..].to_vec(), start: bank.start + e * each, len: each, ..bank.clone() };
                            group.tensors.insert(format!("{stem}.experts.{e}.{linear}.weight"), tensor);
                        }
                    }
                    split.any = true;
                }
            }
        }
    }
    Ok(split)
}

/// `key`'s stem (before `.experts.`) and its role, when it names a fused bank.
fn bank_of(key: &str) -> Option<(&str, &str)> {
    key.rsplit_once(".experts.").filter(|(_, role)| [GATE_UP, DOWN].contains(role))
}

fn config_path(folder: &str) -> String {
    match folder {
        "" => CONFIG_FILE.to_owned(),
        folder => format!("{folder}/{CONFIG_FILE}"),
    }
}

/// The routed experts a config states, of the model or of its text model.
fn stated(config: &Value) -> Option<Stated> {
    [config, &config["text_config"]].into_iter().find_map(|c| {
        let experts = EXPERT_COUNTS.iter().find_map(|f| c[f].as_u64()).filter(|e| *e > 1)?;
        Some(Stated { experts, hidden: c["hidden_size"].as_u64()?, width: c["moe_intermediate_size"].as_u64() })
    })
}

/// Whether a layer's banks are a pair in Transformers' layout: gate-then-up [E, 2I, H] and down [E, H, I], plain, of the stated E
/// and H, agreeing on I (and on the config's, where it states one). Why not, otherwise.
fn pair(gate_up: Option<&Tensor>, down: Option<&Tensor>, stated: &Stated) -> std::result::Result<(), String> {
    let (Some(gate_up), Some(down)) = (gate_up, down) else {
        return Err(format!("only its {}: a layer splits as a pair", if gate_up.is_some() { GATE_UP } else { DOWN }));
    };
    let (e, h) = (stated.experts, stated.hidden);
    for (role, bank) in [(GATE_UP, gate_up), (DOWN, down)] {
        if bank.encoded.is_some() || bank.derive.is_some() {
            return Err(format!("its {role} is encoded or derived"));
        }
        if bank.len % e != 0 {
            return Err(format!("its {role}'s bytes don't divide into {e} experts"));
        }
    }
    let width = match (gate_up.shape.as_slice(), down.shape.as_slice()) {
        ([ge, double, gh], [de, dh, width]) if *ge == e && *gh == h && *de == e && *dh == h && *double == 2 * *width => *width,
        (g, d) => return Err(format!("{GATE_UP} {g:?} and {DOWN} {d:?} aren't [E, 2I, H] and [E, H, I] for E = {e}, H = {h}")),
    };
    match stated.width {
        Some(stated) if stated != width => Err(format!("its expert width {width} isn't the config's moe_intermediate_size {stated}")),
        _ => Ok(()),
    }
}
