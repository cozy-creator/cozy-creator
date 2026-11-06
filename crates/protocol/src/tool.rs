//! A tool: a public function that makes a picture, a video or a sound, as `GET /v1/tools` lists it. What it makes comes from its
//! output, and its box from its own typed inputs, with the little its `Node` adds. Nothing here knows a model.
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    choices,
    interface::{Callable, Constraints, Cuts, Field, Interface, Node, Struct, TAKE, Type, Wire},
    names::shown,
};

/// The kinds of file a node can be.
pub const MEDIA: [&str; 3] = ["image", "video", "audio"];
/// What no tool's box asks for: the platform's words on any asset, whatever made it.
const NOT_ASKED: [&str; 6] = ["seed", "lora", "models", "description", "asset_type", "voice"];
/// What a node's text is called, whatever input of its function it goes to.
pub const TEXT: &str = "instructions";

/// The Edit: what Calcifer makes itself, free, with no machine.
pub const EDIT: &str = "cozy/edit";

/// `GET /v1/tools`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tools {
    pub tools: Vec<Tool>,
    /// Why the list holds only what's installed: the Hub didn't answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Your own tools, the same in every project: these packages' media functions, less `without`, by a tool's `id`. Installing has nothing to do with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolList {
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
}

/// `POST /v1/tools/list`: a package on the list, all its functions with it, or these tools off it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolListChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

/// One row of `GET /v1/tools/browse`: a package of the Hub that makes media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowsedPackage {
    pub package: String,
    /// What it does, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The `id`s of its tools.
    pub tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Browsed {
    pub packages: Vec<BrowsedPackage>,
    /// How many more a bigger `limit` reads.
    pub older: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// What a node names as its `tool`: `qwen-image-2/generate_turbo`, `acme/sketchlab/draw`.
    pub id: String,
    pub package: String,
    pub function: String,
    /// The release this box was read from: the one its machine holds, else the Hub's newest.
    pub release: String,
    /// What a new box of it is called: `shot_1`, counting up as it's taken.
    #[serde(default)]
    pub name: String,
    pub label: String,
    /// What a node of it is: `image`, `video` or `audio`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Local code, named by a node of the project asked about.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// The package a machine installs, where `package` is local code's path: its own name (`local/<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installs: Option<String>,
    pub controls: Vec<Control>,
    /// The kinds its words' mentions are sent as: `image`. A mention of another kind isn't sent; a text box is words. Empty: it sends none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    /// The path in its package of a component that draws its box, where it ships one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<String>,
    /// A new take can keep the start of one of its: it says `fill: {.. take}`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub takes: bool,
    /// The frames a kept take can end on, if it can't end on any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuts: Option<Cuts>,
    /// With another tool of its package of the same `label` this is one box, and a toggle by this word turns it on: `Turbo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// What a checkpoint dropped on its box goes on: the one model its function runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ToolModel>,
}

/// The model a tool runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolModel {
    /// Its key in a node's `models`.
    pub slot: String,
    /// What its package loads unless told otherwise: `[org/]model@release`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// That checkpoint's family, as its header says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
}

/// `GET /v1/tools/{id}/ui`: a tool's component, ready for the page to import.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolUi {
    /// Its entry module on this computer's Calcifer, named by its hash.
    pub module: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    #[default]
    Text,
    /// A list of rows, each its `members`.
    Group,
    Shape,
    Range,
    Choice,
    Toggle,
    File,
    /// An Edit's clips: `fields.timeline`.
    Timeline,
}

/// One thing a box asks for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// What a node's fields call it.
    pub field: String,
    #[serde(rename = "type")]
    pub ty: ControlType,
    pub label: String,
    /// `choice`, and a `shape` that allows only these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<Value>>,
    /// `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// `file`: what it takes, `image`, `video` or `audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<String>,
    /// `file`: several, in order.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
    /// `group`: a row's controls, and how few and how many rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Control>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    /// A group's member whose sum it shows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub total: bool,
    /// What an empty text reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// What a number counts: `s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// On its box's default face: its first text, what's required, and what its `Node` shows. The rest folds away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub face: bool,
}

/// The Edit as a tool, listed before any function: clips on a track, rendered here.
pub fn edit() -> Tool {
    Tool {
        id: EDIT.to_owned(),
        package: "cozy".to_owned(),
        function: "edit".to_owned(),
        release: String::new(),
        name: "Edit".to_owned(),
        label: "Edit".to_owned(),
        kind: "video".to_owned(),
        description: "Put videos and stills together into one video, free, with no machine".to_owned(),
        local: false,
        installs: None,
        references: vec![],
        ui: None,
        takes: false,
        cuts: None,
        variant: None,
        model: None,
        controls: vec![Control { field: "timeline".to_owned(), ty: ControlType::Timeline, label: "Edit".to_owned(), face: true, ..Control::default() }],
    }
}

/// What `callable` makes: the first field of its output that is a picture, a video or a sound, by its type, or a plain file by its name, as a
/// version takes it. None: it makes no media.
pub fn output_kind(callable: &Callable) -> Option<&str> {
    let of = |field: &'_ Field| {
        let (kind, _) = field.ty.media()?;
        MEDIA.iter().copied().find(|media| *media == kind || (kind == "file" && *media == field.name))
    };
    callable.result.fields.iter().find_map(of)
}

/// What `callable` answered in words: its output's first text, or a list of texts' first. `warnings` is no answer.
pub fn output_text(callable: &Callable, result: &Value) -> Option<String> {
    let said = |field: &Field| match (optional(&field.ty), &result[field.name.as_str()]) {
        (Type::Str, Value::String(text)) => Some(text.clone()),
        (Type::List(item), Value::Array(texts)) if **item == Type::Str => texts.first()?.as_str().map(str::to_owned),
        _ => None,
    };
    callable.result.fields.iter().filter(|field| field.name != "warnings").find_map(said).filter(|text| !text.trim().is_empty())
}

/// The input that takes an asset's `instructions`: the one its author calls that or `prompt`, else its first required text.
pub fn text_input(request: &Struct) -> Option<&Field> {
    let named = request.fields.iter().find(|field| matches!(field.name.as_str(), "instructions" | "prompt"));
    named.or_else(|| request.fields.iter().find(|field| field.wire == Wire::Required && *optional(&field.ty) == Type::Str))
}

/// Every tool of a package's interface: what a person runs of it that makes media, and only what declares a `Node` if anything does.
pub fn tools_of(package: &str, release: &str, interface: &Interface) -> Vec<Tool> {
    let declared = interface.callables.iter().any(|callable| callable.node.is_some());
    let callables = interface.public().into_iter().filter_map(|name| interface.callables.iter().find(|c| c.name == name));
    callables.filter(|callable| !declared || callable.node.is_some()).filter_map(|callable| tool_of(package, release, callable)).collect()
}

fn tool_of(package: &str, release: &str, callable: &Callable) -> Option<Tool> {
    let kind = output_kind(callable)?;
    let package = shown(package);
    let node = callable.node.clone().unwrap_or_default();
    Some(Tool {
        id: format!("{package}/{}", callable.name),
        name: box_name(&package, callable),
        label: label_of(&package, callable),
        package,
        function: callable.name.clone(),
        release: release.to_owned(),
        kind: kind.to_owned(),
        description: callable.description.clone(),
        local: false,
        installs: None,
        controls: controls(callable, &node),
        references: references(callable),
        takes: node.fill.values().any(|source| source == TAKE),
        cuts: node.cuts,
        ui: node.ui,
        variant: Some(node.variant).filter(|word| !word.is_empty()),
        model: choices::main_model(&callable.models, &callable.calls.iter().collect::<Vec<_>>()).map(|slot| ToolModel {
            slot: slot.param.clone(),
            default: slot.default.clone(),
            family: None,
        }),
    })
}

/// What a box of `callable` says it is: its `Node`'s label, else its package's words and the function's.
pub fn label_of(package: &str, callable: &Callable) -> String {
    match callable.node.as_ref().map(|node| node.label.as_str()).filter(|label| !label.is_empty()) {
        Some(label) => label.to_owned(),
        None => label(package, &callable.name),
    }
}

/// The kinds `callable`'s mentions are sent as: the files of each list filled from mentions, and its `assets`.
pub fn references(callable: &Callable) -> Vec<String> {
    let mentioned = callable.request.fields.iter().filter_map(|field| match optional(&field.ty) {
        Type::List(item) => match &**item {
            Type::Struct(row) if row.is_mention() => Some(row),
            _ => None,
        },
        _ => None,
    });
    let mut kinds: Vec<String> = mentioned.flat_map(|row| row.fields.iter().filter_map(|member| Some(member.ty.media()?.0.to_owned()))).collect();
    kinds.extend(callable.assets.iter().flat_map(|assets| assets.kinds.iter().map(|kind| kind.kind.clone())));
    kinds.sort();
    kinds.dedup();
    kinds
}

/// What a new box of `callable` is called, which counts up as it's taken: its `Node`'s word, else its package's, numbered.
pub fn box_name(package: &str, callable: &Callable) -> String {
    let said = callable.node.as_ref().map(|node| node.name.as_str()).filter(|name| !name.is_empty());
    first_of(said.unwrap_or_else(|| package_word(package)))
}

/// The word of a package: the last of its path, with no `.py` if it's a script.
pub fn package_word(package: &str) -> &str {
    let last = package.rsplit('/').find(|word| !word.is_empty()).unwrap_or(package);
    last.strip_suffix(".py").unwrap_or(last)
}

/// `shot` as the first of its boxes.
pub fn first_of(word: &str) -> String {
    format!("{word}_1")
}

/// `Qwen Image 2 Turbo`, `Minimax H3 Upscale`: a package's words and what the function adds to `generate`.
fn label(package: &str, function: &str) -> String {
    let mut out = String::new();
    for word in package.rsplit('/').next().unwrap_or(package).split(['-', '_']).filter(|word| !word.is_empty()) {
        let _ = write!(out, "{} ", capitalized(word));
    }
    let added = function.strip_prefix("generate").map_or(function, |rest| rest.trim_start_matches('_'));
    out.push_str(&capitalized(&added.replace('_', " ")));
    out.trim_end().to_owned()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The box of `callable`: every input a person can set by hand, by these same rules however it nests.
fn controls(callable: &Callable, node: &Node) -> Vec<Control> {
    let request = &callable.request;
    let undrawn = |field: &&Field| !NOT_ASKED.contains(&field.name.as_str()) && !callable.is_assets(&field.name) && !node.hide.contains(&field.name) && !node.fill.contains_key(&field.name);
    let default = |field: &Field| callable.invocable.defaults.get(&format!("request/{}", field.name)).cloned();
    let text = text_input(request).filter(|text| undrawn(text)).or_else(|| request.fields.iter().filter(undrawn).find(|field| *optional(&field.ty) == Type::Str));
    let mut controls = vec![];
    for field in request.fields.iter().filter(undrawn) {
        let Some(mut control) = control(field, default(field)) else { continue };
        control.face = control.required || node.show.contains(&field.name) || text.is_some_and(|text| text.name == field.name);
        if text.is_some_and(|text| text.name == field.name) && matches!(field.name.as_str(), "instructions" | "prompt") {
            control.field = TEXT.to_owned();
            if !field.extra.contains_key("label") {
                control.label = capitalized(TEXT);
            }
        }
        controls.push(control);
    }
    controls
}

/// A field as its control, its author's `default` over its own. None: a person couldn't set it by hand, or mentions fill it.
fn control(field: &Field, default: Option<Value>) -> Option<Control> {
    let ty = optional(&field.ty);
    let mut control = Control {
        field: field.name.clone(),
        label: field.extra.get("label").and_then(Value::as_str).map_or_else(|| capitalized(&words(&field.name)), str::to_owned),
        placeholder: field.extra.get("placeholder").and_then(Value::as_str).map(str::to_owned),
        unit: field.extra.get("unit").and_then(Value::as_str).map(str::to_owned),
        total: field.extra.get("total").and_then(Value::as_bool) == Some(true),
        default: default.or(field.default.clone()).filter(|value| !value.is_null()),
        required: field.wire == Wire::Required,
        ..Control::default()
    };
    if field.name == "aspect_ratio" {
        control.ty = ControlType::Shape;
        control.options = choices(ty).or_else(|| field.extra.get("options").and_then(Value::as_array).cloned());
        (control.min, control.max) = (number(field, "min"), number(field, "max"));
        return Some(control);
    }
    match ty {
        Type::Str => {}
        Type::Literal(_) => {
            control.ty = ControlType::Choice;
            control.options = choices(ty);
        }
        Type::Bool => control.ty = ControlType::Toggle,
        Type::Int | Type::Float => {
            let (min, max) = bounds(field)?;
            control.ty = ControlType::Range;
            (control.min, control.max) = (Some(min), Some(max));
            control.step = field.constraints.multiple_of.or_else(|| number(field, "step")).or((*ty == Type::Int).then_some(1.0));
        }
        Type::Asset(kind) if MEDIA.contains(&kind.as_str()) => {
            control.ty = ControlType::File;
            control.accept = Some(kind.clone());
        }
        Type::List(item) => match &**item {
            Type::Asset(kind) if MEDIA.contains(&kind.as_str()) => {
                control.ty = ControlType::File;
                control.accept = Some(kind.clone());
                control.multiple = true;
            }
            Type::Struct(row) if !row.is_mention() => {
                control.ty = ControlType::Group;
                control.members = row.fields.iter().filter_map(|member| self::control(member, None)).collect();
                control.min_items = field.constraints.min_length;
                control.max_items = field.constraints.max_length;
            }
            _ => return None,
        },
        _ => return None,
    }
    Some(control)
}

fn words(name: &str) -> String {
    name.replace('_', " ")
}

/// `ty`, or what it is when it's there: `T | None` is `T`.
fn optional(ty: &Type) -> &Type {
    match ty {
        Type::Union { branches, .. } if branches.len() == 2 && branches.contains(&Type::Null) => branches.iter().find(|branch| **branch != Type::Null).unwrap_or(ty),
        _ => ty,
    }
}

fn choices(ty: &Type) -> Option<Vec<Value>> {
    let Type::Literal(members) = ty else { return None };
    Some(members.iter().filter(|member| !member.is_null()).cloned().collect())
}

/// A number's two ends, if it has them: what it declares, else what its author told a box, which a run clamps to rather than refuses.
fn bounds(field: &Field) -> Option<(f64, f64)> {
    let Constraints { ge, gt, le, lt, .. } = &field.constraints;
    Some(((*ge).or(*gt).or_else(|| number(field, "min"))?, (*le).or(*lt).or_else(|| number(field, "max"))?))
}

fn number(field: &Field, hint: &str) -> Option<f64> {
    field.extra.get(hint).and_then(Value::as_f64)
}
