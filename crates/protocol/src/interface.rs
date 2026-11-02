//! A package's interface (`cozy.package.interface/1`), as the Runtime describes it: the one place
//! every Rust reader spells its paths. Each accessor is tolerant: a member absent, null or of
//! another shape reads as absent, so no reader refuses a member it does not use.
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// The member that lists the entrypoints, for a reader that rewrites the document.
pub const ENTRYPOINTS: &str = "entrypoints";

/// Its format, `cozy.package.interface/<n>`; empty when it names none.
pub fn format(document: &Value) -> &str {
    document["format"].as_str().unwrap_or_default()
}

/// Its App, `module:attribute`.
pub fn application(document: &Value) -> Option<&str> {
    document["application"].as_str()
}

/// Every entrypoint it declares, in its order.
pub fn entrypoints(document: &Value) -> impl Iterator<Item = &Value> {
    document[ENTRYPOINTS].as_array().into_iter().flatten()
}

/// The entrypoint named `name`.
pub fn entrypoint<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    entrypoints(document).find(|entry| self::name(entry) == Some(name))
}

/// Its declared quantize recipes.
pub fn recipes(document: &Value) -> impl Iterator<Item = &Value> {
    document["recipes"].as_array().into_iter().flatten()
}

/// An entrypoint's name.
pub fn name(entry: &Value) -> Option<&str> {
    entry["name"].as_str()
}

/// Whether it is internal: callable by the package's own code, never offered to people.
pub fn internal(entry: &Value) -> bool {
    entry["internal"].as_bool() == Some(true)
}

/// Whether it publishes media while it runs (`Outputs.publish`).
pub fn publishes(entry: &Value) -> bool {
    entry["publishes"].as_bool() == Some(true)
}

/// Its model slots (`<entrypoint>.models.<param>`).
pub fn models(entry: &Value) -> &[Value] {
    entry["models"].as_array().map(Vec::as_slice).unwrap_or_default()
}

/// A model slot's path, `<entrypoint>.models.<param>`.
pub fn slot_path(slot: &Value) -> Option<&str> {
    slot["path"].as_str()
}

/// Whether it never opens a GPU (`accelerator: false`).
pub fn cpu(entry: &Value) -> bool {
    entry["accelerator"].as_bool() == Some(false)
}

/// Whether `entry` is a CPU function that loads no model, in a package with internal GPU functions:
/// it calls them (H3's shot). Tensorhub's placement (`runplacement`) reads a function the same way.
pub fn orchestrates(document: &Value, entry: &Value) -> bool {
    cpu(entry)
        && models(entry).is_empty()
        && entrypoints(document).any(|e| internal(e) && !cpu(e) && !models(e).is_empty())
}

/// The model slots of the calls an orchestrator makes: its package's internal GPU functions'. A
/// run names one by its parameter, and the machine gives that choice to every call declaring it.
pub fn call_slots<'a>(document: &'a Value, entry: &Value) -> Vec<&'a Value> {
    if !orchestrates(document, entry) {
        return vec![];
    }
    entrypoints(document).filter(|e| internal(e) && !cpu(e)).flat_map(models).collect()
}

/// One declared model slot: `<callable>.models.<param>`.
#[derive(Clone, Debug, Deserialize)]
pub struct Slot {
    pub path: String,
    #[serde(skip)]
    pub param: String,
    #[serde(default, deserialize_with = "or_default")]
    pub class: String,
    /// A `Weights[M]` argument: the raw checkpoint, read and never constructed.
    #[serde(default, deserialize_with = "or_default")]
    pub weights: bool,
    /// The release a run that names none loads: `[org/]model@release`.
    #[serde(default, deserialize_with = "or_default")]
    pub default: Option<String>,
    /// The lane its default loads in; absent: the Hub's pick for the machine.
    #[serde(default, deserialize_with = "or_default")]
    pub lane: Option<String>,
    /// `decode` for an LM token loop; absent is compute.
    #[serde(default, deserialize_with = "or_default")]
    pub phase: Option<String>,
    /// Built, and its weights fetched, when the function first reads it.
    #[serde(default, deserialize_with = "or_default")]
    pub on_demand: bool,
}

impl Slot {
    /// A declared slot, its parameter read from its path; why when it is not
    /// `<callable>.models.<param>`.
    pub fn read(raw: &Value) -> Result<Self, String> {
        let mut slot = Self::deserialize(raw).map_err(|e| e.to_string())?;
        match slot.path.split_once(".models.") {
            Some((callable, param)) if !callable.is_empty() && !param.is_empty() && !param.contains('.') => {
                slot.param = param.to_owned();
                Ok(slot)
            }
            _ => Err(format!("model path {:?} is not <callable>.models.<parameter>", slot.path)),
        }
    }
}

/// A member written as `null` reads as absent: a Go writer spells an empty list that way.
pub fn or_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

/// Its invocable declaration (module, export, defaults, …); null when it declares none.
pub fn invocable(entry: &Value) -> &Value {
    &entry["invocable"]
}

/// The module and export its invocable is called by.
pub fn callable(entry: &Value) -> Option<(&str, &str)> {
    let invocable = invocable(entry);
    Some((invocable["module"].as_str()?, invocable["export"].as_str()?))
}

/// Whether its invocable is memoized (`memoize=True`): its result is reusable for the same request.
pub fn memoized(entry: &Value) -> bool {
    invocable(entry)["memoize"].as_bool() == Some(true)
}

/// Its result schema.
pub fn result(entry: &Value) -> Option<&Value> {
    entry.get("result")
}

/// Its weights outputs: each output's id and the most new bytes it may write, when declared.
pub fn weights_outputs(entry: &Value) -> impl Iterator<Item = (&str, Option<u64>)> {
    entry["weights_outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| Some((row["output_id"].as_str()?, row["max_bytes"].as_u64())))
}
