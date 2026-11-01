//! A package's interface (`cozy.package.interface/1`), as the Runtime describes it: the one place
//! every Rust reader spells its paths. Each accessor is tolerant: a member absent, null or of
//! another shape reads as absent, so no reader refuses a member it does not use.
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

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

// A package's interface as the Hub's release card carries it, parsed once into typed records. A callable this
// build cannot read is unavailable alone, with why; the rest of the package still runs. Members it does not
// know are ignored, so newer Runtimes' cards load.

#[derive(Debug, Default)]
pub struct Interface {
    pub callables: Vec<Callable>,
    /// Callables this build cannot read, and why.
    pub unavailable: Vec<(String, String)>,
    /// The package's declared quantize recipes, which its `quantize` runs.
    pub recipes: Vec<Recipe>,
}

/// The entrypoint a package with recipes runs them through (`cozy model quantize`).
pub const RECIPES: &str = "quantize";

/// One declared recipe: the lane it writes and the model it reads.
#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub lane: String,
    pub model: RecipeModel,
}

#[derive(Debug, Deserialize)]
pub struct RecipeModel {
    pub class: String,
    #[serde(default, deserialize_with = "or_default")]
    pub component_use: BTreeMap<String, Vec<String>>,
}

impl Recipe {
    /// The `cozy model quantize` flag that runs it.
    pub fn flag(&self) -> String {
        format!("--lane {}", self.lane)
    }

    /// Whether a checkpoint holding `components` holds every component its model reads.
    pub fn reads(&self, components: &[String]) -> bool {
        self.model.component_use.values().flatten().all(|need| components.contains(need))
    }
}

#[derive(Debug, Deserialize)]
pub struct Callable {
    pub name: String,
    /// Its docstring's first paragraph; empty in an interface written before Runtime had it.
    #[serde(default, deserialize_with = "text")]
    pub description: String,
    #[serde(default, deserialize_with = "or_default")]
    pub internal: bool,
    #[serde(default, deserialize_with = "or_default")]
    pub models: Vec<Slot>,
    pub request: Struct,
    #[serde(default, deserialize_with = "or_default")]
    pub result: Struct,
    #[serde(default, deserialize_with = "or_default")]
    pub weights_outputs: Vec<WeightsOutput>,
    #[serde(default, deserialize_with = "or_default")]
    pub assets: Option<AssetsSlot>,
    /// Written only as false: the function never opens a GPU.
    #[serde(default, deserialize_with = "or_default")]
    pub accelerator: Option<bool>,
    #[serde(default, deserialize_with = "or_default")]
    pub invocable: Invocable,
    /// A CPU function that loads no model and calls its package's GPU functions
    /// (`orchestrates`).
    #[serde(skip)]
    pub orchestrates: bool,
    /// The model slots of the calls it makes (`call_slots`).
    #[serde(skip)]
    pub calls: Vec<Slot>,
}

#[derive(Debug, Deserialize)]
pub struct WeightsOutput {
    pub output_id: String,
}

/// The declared collection of attached files: `<parameter>: [{asset, label?, fidelity?}]`.
#[derive(Debug, Deserialize)]
pub struct AssetsSlot {
    pub parameter: String,
    #[serde(default, deserialize_with = "or_default")]
    pub kinds: Vec<AssetsKind>,
}

#[derive(Debug, Deserialize)]
pub struct AssetsKind {
    pub kind: String,
    #[serde(default, deserialize_with = "or_default")]
    pub media_types: Vec<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub max_bytes: Option<u64>,
    #[serde(default, deserialize_with = "or_default")]
    pub max_count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Invocable {
    /// `request/<field>` → the author's default, for people.
    #[serde(default, deserialize_with = "or_default")]
    pub defaults: BTreeMap<String, Value>,
}

/// A rendered msgspec struct; a result may instead be one model (`{"input": "model"}`).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Struct {
    #[serde(default, deserialize_with = "or_default")]
    pub fields: Vec<Field>,
    #[serde(default, deserialize_with = "or_default")]
    pub input: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub tag_field: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub tag: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(default, deserialize_with = "or_default")]
    pub wire: Wire,
    /// Present (even as null) only when the author wrote one.
    #[serde(default, deserialize_with = "present")]
    pub default: Option<Value>,
    #[serde(default, deserialize_with = "or_default")]
    pub constraints: Constraints,
    #[serde(default, deserialize_with = "or_default")]
    pub asset_bound: AssetBound,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wire {
    #[default]
    Required,
    Optional,
    /// Optional, and the model chooses when absent.
    Omissible,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Constraints {
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub gt: Option<f64>,
    pub ge: Option<f64>,
    pub lt: Option<f64>,
    pub le: Option<f64>,
    pub multiple_of: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetBound {
    pub max_bytes: Option<u64>,
    #[serde(default, deserialize_with = "or_default")]
    pub media_types: Vec<String>,
}

/// A rendered type. `Unknown` is a type this build cannot read: its values are left to Runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Str,
    Int,
    Float,
    Bool,
    Null,
    Literal(Vec<Value>),
    Union {
        branches: Vec<Type>,
        tag_field: Option<String>,
    },
    List(Box<Type>),
    Map(Box<Type>),
    Struct(Box<Struct>),
    Asset(String),
    Tree,
    Model,
    Unknown,
}

impl PartialEq for Struct {
    fn eq(&self, other: &Self) -> bool {
        self.fields.len() == other.fields.len()
            && self
                .fields
                .iter()
                .zip(&other.fields)
                .all(|(a, b)| a.name == b.name && a.ty == b.ty)
            && self.tag == other.tag
    }
}

impl Interface {
    /// Reads a card's `package_interface`. Only a document that is not an interface at all refuses.
    pub fn parse(document: &Value) -> Result<Self, String> {
        let format = format(document);
        if !format.starts_with("cozy.package.interface/") {
            return Err(format!("not a package interface (format {format:?})"));
        }
        let mut out = Self::default();
        for raw in entrypoints(document) {
            let name = name(raw).unwrap_or_default().to_owned();
            match Callable::deserialize(raw).map_err(|e| e.to_string()) {
                Ok(callable) => match callable.checked() {
                    Ok(mut callable) => {
                        callable.orchestrates = orchestrates(document, raw);
                        let calls = call_slots(document, raw).into_iter();
                        callable.calls = calls.filter_map(|slot| Slot::read(slot).ok()).collect();
                        out.callables.push(callable)
                    }
                    Err(why) => out.unavailable.push((name, why)),
                },
                Err(why) => out.unavailable.push((name, why)),
            }
        }
        // A recipe this build cannot read is left out; `cozy model quantize` names those it can.
        out.recipes = recipes(document)
            .filter_map(|raw| Recipe::deserialize(raw).ok())
            .collect();
        Ok(out)
    }

    /// The package's functions, GPU before CPU, each group sorted: never an internal callable or
    /// the recipes' `quantize`, which `cozy model quantize` runs.
    pub fn public(&self) -> Vec<&str> {
        let mut public: Vec<&Callable> = self.callables.iter().filter(|c| !c.internal && !self.runs_recipes(c)).collect();
        public.sort_unstable_by_key(|c| (c.accelerator == Some(false), c.name.as_str()));
        public.into_iter().map(|c| c.name.as_str()).collect()
    }
}

impl Interface {
    /// Whether `callable` is the `quantize` that runs this package's recipes.
    pub fn runs_recipes(&self, callable: &Callable) -> bool {
        callable.name == RECIPES && !self.recipes.is_empty()
    }

    /// Whether `callable` runs on a GPU: its own body, or the package's internal GPU functions a
    /// CPU function that loads no model orchestrates (H3's ref2va and long_form). Tensorhub's
    /// placement (`runplacement`) reads a function the same way.
    pub fn gpu(&self, callable: &Callable) -> bool {
        callable.accelerator != Some(false) || self.orchestrates(callable)
    }

    /// A CPU function that loads no model, in a package with internal GPU functions: it calls them.
    pub fn orchestrates(&self, callable: &Callable) -> bool {
        callable.orchestrates
    }

    /// The model slots of the calls an orchestrator makes: its package's internal GPU functions'.
    pub fn call_slots<'a>(&self, callable: &'a Callable) -> Vec<&'a Slot> {
        callable.calls.iter().collect()
    }
}

impl Callable {
    fn checked(mut self) -> Result<Self, String> {
        let prefix = format!("{}.models.", self.name);
        for slot in &mut self.models {
            match slot.path.strip_prefix(&prefix) {
                Some(param) if !param.is_empty() && !param.contains('.') => {
                    slot.param = param.to_owned()
                }
                _ => {
                    return Err(format!(
                        "model path {:?} is not {prefix}<parameter>",
                        slot.path
                    ));
                }
            }
        }
        Ok(self)
    }

    /// A conversion reads `<input-model> [<org/model>]`: it has model slots and writes weights.
    pub fn is_conversion(&self) -> bool {
        !self.models.is_empty() && !self.weights_outputs.is_empty()
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.request.fields.iter().find(|f| f.name == name)
    }

    /// Whether `field` is the declared Assets collection.
    pub fn is_assets(&self, field: &str) -> bool {
        self.assets.as_ref().is_some_and(|a| a.parameter == field)
    }
}

impl Type {
    /// An asset's kind, through an optional union with null; `list` is a list of them.
    pub fn media(&self) -> Option<(&str, bool)> {
        match self {
            Type::Asset(kind) => Some((kind, false)),
            Type::List(item) => match item.as_ref() {
                Type::Asset(kind) => Some((kind, true)),
                _ => None,
            },
            Type::Union { branches, .. } if branches.len() == 2 => {
                let other = branches.iter().find(|b| **b != Type::Null)?;
                if branches.contains(&Type::Null) {
                    other.media()
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Type {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Type::read(&Value::deserialize(d)?))
    }
}

impl Type {
    fn read(value: &Value) -> Self {
        match value {
            Value::String(name) => match name.as_str() {
                "str" => Type::Str,
                "int" => Type::Int,
                "float" => Type::Float,
                "bool" => Type::Bool,
                "null" => Type::Null,
                _ => Type::Unknown,
            },
            Value::Object(object) => Type::read_object(object),
            _ => Type::Unknown,
        }
    }

    fn read_object(object: &Map<String, Value>) -> Self {
        if let Some(kind) = object.get("asset") {
            return Type::Asset(kind.as_str().unwrap_or("file").to_owned());
        }
        match object.get("input").and_then(Value::as_str) {
            Some("tree") => return Type::Tree,
            Some("model") => return Type::Model,
            _ => {}
        }
        if let Some(Value::Array(members)) = object.get("literal") {
            return Type::Literal(members.clone());
        }
        if let Some(Value::Array(branches)) = object.get("union") {
            return Type::Union {
                branches: branches.iter().map(Type::read).collect(),
                tag_field: object
                    .get("tag_field")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
        }
        if let Some(item) = object.get("list") {
            return Type::List(Box::new(Type::read(item)));
        }
        if let Some(pair) = object.get("map") {
            return Type::Map(Box::new(Type::read(&pair["value"])));
        }
        if object.contains_key("fields")
            && let Ok(nested) = Struct::deserialize(Value::Object(object.clone()))
        {
            return Type::Struct(Box::new(nested));
        }
        Type::Unknown
    }
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// A string, else empty: text for people never makes a callable unreadable.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Value::deserialize(d)?.as_str().unwrap_or_default().to_owned())
}

