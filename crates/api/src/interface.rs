//! A package's interface (`cozy.package.interface/1`), as the Runtime describes it: the one place
//! every Rust reader spells its paths. Each accessor is tolerant: a member absent, null or of
//! another shape reads as absent, so no reader refuses a member it does not use.
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
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

/// `name`'s request contract (`cozy run … --describe --json`), with `accelerator: false` for a CPU
/// function: a release's card and local code's interface answer the same.
pub fn request(document: &Value, name: &str) -> Value {
    let Some(entry) = entrypoint(document, name) else {
        return Value::Null;
    };
    let mut request = entry["request"].clone();
    if entry["accelerator"] == Value::Bool(false) {
        request["accelerator"] = Value::Bool(false);
    }
    request
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
        slot.param = Self::param_of(&slot.path)?;
        Ok(slot)
    }

    fn param_of(path: &str) -> Result<String, String> {
        match path.split_once(".models.") {
            Some((callable, param)) if !callable.is_empty() && !param.is_empty() && !param.contains('.') => Ok(param.to_owned()),
            _ => Err(format!("model path {path:?} is not <callable>.models.<parameter>")),
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
    /// `@app.tool`: it's a box. None: it's a function only `cozy run` and other tools call.
    #[serde(default, deserialize_with = "or_default")]
    pub tool: Option<ToolDecl>,
    /// Each function it calls, its own package's or a dependency's: what placement reads of a composed tool's GPU work.
    #[serde(default, deserialize_with = "or_default")]
    pub calls: Vec<Call>,
}

/// `@app.tool`: what a box of it is, beyond its types.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ToolDecl {
    /// Its name in the tool list. Empty: from its package and function.
    #[serde(default, deserialize_with = "or_default")]
    pub label: String,
    /// What a box of it is called, numbered: `shot` makes `shot_1`. Empty: its package.
    #[serde(default, deserialize_with = "or_default")]
    pub named: String,
    /// The model's own tags in a text: never mentions.
    #[serde(default, deserialize_with = "or_default")]
    pub reserved: Vec<String>,
    /// The path in its package of a component that draws its box.
    #[serde(default, deserialize_with = "or_default")]
    pub ui: Option<String>,
}

/// One function a tool calls, each its own unit of placement.
#[derive(Debug, Clone, Deserialize)]
pub struct Call {
    pub package: String,
    pub function: String,
    /// Written only as false: it never opens a GPU.
    #[serde(default, deserialize_with = "or_default")]
    pub accelerator: Option<bool>,
    /// The GPU widths it runs at, `[1, 2, 4]`; absent is `[1]`, or none for a CPU callee.
    #[serde(default, deserialize_with = "or_default")]
    pub gpus: Vec<u32>,
    #[serde(default, deserialize_with = "or_default")]
    pub models: Vec<Slot>,
}

impl Call {
    pub fn gpu(&self) -> bool {
        self.accelerator != Some(false)
    }
}

/// A pickup's frames: `offset + k * every`, from frame `first`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Snap {
    pub every: u32,
    pub offset: u32,
    pub first: u32,
}

impl Default for Snap {
    fn default() -> Self {
        Self { every: 1, offset: 0, first: 0 }
    }
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
    /// The module that declares it, dotted, from its package's root.
    #[serde(default, deserialize_with = "or_default")]
    pub module: String,
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
    /// What the author added for readers: `msgspec.Meta(extra=...)`.
    #[serde(default, deserialize_with = "or_default")]
    pub extra: Map<String, Value>,
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
    /// A file: `image`, `video`, `audio`, `latents` or `file`, by its hash.
    Asset(String),
    /// Text and what it mentions, of these kinds of file.
    Prompt(Vec<String>),
    /// A file of `kind` with its description, and the extras of the version it holds.
    Media { kind: String, extras: Box<Struct> },
    /// What a gesture fills: a take and where it's picked up.
    Pickup(Box<Struct>),
    /// The box's own shape, `"16:9"`.
    Shape,
    Seed,
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
                    Ok(callable) => out.callables.push(callable),
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

    /// Whether `callable` runs on a GPU: its own body, or a call it makes.
    pub fn gpu(&self, callable: &Callable) -> bool {
        callable.accelerator != Some(false) || callable.calls.iter().any(Call::gpu)
    }

    /// Whether `callable` can't run without a GPU: it builds a model on one, or calls a function that does (its declared `calls`;
    /// with none declared, a deviceless root of a package whose own child tasks build one, as TensorD reads its GPU groups). One that
    /// opens a GPU but builds nothing runs on a CPU too.
    pub fn needs_gpu(&self, callable: &Callable) -> bool {
        let builds = |models: &[Slot]| models.iter().any(|slot| !slot.weights);
        if callable.accelerator != Some(false) && builds(&callable.models) {
            return true;
        }
        if !callable.calls.is_empty() {
            return callable.calls.iter().any(|call| call.gpu() && builds(&call.models));
        }
        callable.accelerator == Some(false)
            && self.callables.iter().any(|other| other.internal && other.accelerator != Some(false) && builds(&other.models))
    }

    /// Whether `callable` is a CPU tool that composes calls.
    pub fn composes(&self, callable: &Callable) -> bool {
        callable.accelerator == Some(false) && callable.calls.iter().any(Call::gpu)
    }

    /// The model slots of the calls `callable` makes: a run names one by its parameter, and each call declaring it gets that choice.
    pub fn call_slots<'a>(&self, callable: &'a Callable) -> Vec<&'a Slot> {
        callable.calls.iter().flat_map(|call| &call.models).collect()
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
        for slot in self.calls.iter_mut().flat_map(|call| &mut call.models) {
            slot.param = Slot::param_of(&slot.path)?;
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
    /// It, or what it is when it's there: `T | None` is `T`.
    pub fn plain(&self) -> &Type {
        match self {
            Type::Union { branches, .. } if branches.len() == 2 && branches.contains(&Type::Null) => branches.iter().find(|branch| **branch != Type::Null).unwrap_or(self),
            _ => self,
        }
    }

    /// Whether it may be absent: `T | None`.
    pub fn optional(&self) -> bool {
        matches!(self, Type::Union { branches, .. } if branches.contains(&Type::Null))
    }

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
        if let Some(Value::Array(kinds)) = object.get("prompt") {
            return Type::Prompt(kinds.iter().filter_map(Value::as_str).map(str::to_owned).collect());
        }
        if let Some(kind) = object.get("media").and_then(Value::as_str) {
            let extras = Struct::deserialize(Value::Object(object.clone())).unwrap_or_default();
            return Type::Media { kind: kind.to_owned(), extras: Box::new(extras) };
        }
        if let Some(pickup) = object.get("pickup") {
            return Struct::deserialize(pickup.clone()).map_or(Type::Unknown, |read| Type::Pickup(Box::new(read)));
        }
        match object.get("input").and_then(Value::as_str) {
            Some("tree") => return Type::Tree,
            Some("model") => return Type::Model,
            Some("aspect_ratio") => return Type::Shape,
            Some("seed") => return Type::Seed,
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

