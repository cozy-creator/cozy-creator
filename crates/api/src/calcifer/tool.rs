//! A tool: a public function declared `@app.tool`, as `GET /v1/tools` lists it. What it makes comes from its result, and its box from its
//! struct's typed fields. Nothing here knows a model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    choices,
    interface::{Callable, Constraints, Field, Interface, Snap, Struct, Type, When, Wire},
    names::shown,
};

/// The kinds of file a box can be.
pub const MEDIA: [&str; 3] = ["image", "video", "audio"];
/// The platform's words on any box, whatever made it: no tool's field goes by one.
pub const PLATFORM: [&str; 9] = ["tool", "approved", "description", "models", "lora", "text", "node_kind", "selected_version", "starred"];

/// The Edit: what Calcifer makes itself, free, with no machine.
pub const EDIT: &str = "cozy/edit";

/// `GET /v1/tools`: the same for every machine, and told changed on `/v1/events` (`tools.changed`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Tools {
    pub tools: Vec<Tool>,
}

/// Your changes to the tools, the same in every project: these packages beside what the Hub lists, and `without`, tools taken off, by a tool's `id`. Installing has nothing to do with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolList {
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
}

/// `POST /v1/tools/list`: a package on the list, all its functions with it, or these tools off it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolListChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

/// One row of `GET /v1/tools/browse`: a package of the Hub that makes media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BrowsedPackage {
    pub package: String,
    /// What it does, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The `id`s of its tools.
    pub tools: Vec<String>,
}

/// `GET /v1/tools/browse`: a page of the Hub's packages that make media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Browsed {
    pub packages: Vec<BrowsedPackage>,
    /// How many more a bigger `limit` reads.
    pub older: i64,
}

/// One tool, as `GET /v1/tools` lists it: what it makes and what its box asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Tool {
    /// What a box names as its `tool`: `qwen-image-2/image`, `acme/sketchlab/draw`.
    pub id: String,
    pub package: String,
    pub function: String,
    /// The release this box was read from: the one its machine holds, else the Hub's newest.
    pub release: String,
    /// What a new box of it is called: `shot_1`, counting up as it's taken.
    #[serde(default)]
    pub name: String,
    /// What it does within its package: `Image`, `Edit`, `Upscale`.
    pub label: String,
    /// Its package in words, as its description writes its name (`MiniMax H3`, `Qwen Image 2.1`), else as `title` spells it (`SDXL`).
    #[serde(default)]
    pub package_title: String,
    /// What a box of it is: `image`, `video`, `audio` or `text`, by its result's first such field.
    pub makes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Local code, named by a box of the project asked about.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// The package a machine installs, where `package` is local code's path: its own name (`local/<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installs: Option<String>,
    /// From a dev home's packages checkout (xs-195): its branch and commit, `rc/0.26.0 @ 1a2b3c4d`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What its box asks for, in its struct's order.
    pub fields: Vec<ToolField>,
    /// It takes a pickup, which Pick up from here fills: the frames one may land on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<Snap>,
    /// What a checkpoint dropped on its box goes on: the one model its function runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ToolModel>,
    /// Every declared model slot, including the full paths of slots in child calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ToolModel>,
}

impl Tool {
    /// What a person reads it as, as the page names it (`toolName`): `Qwen Image 1 · Image`, and its label alone where that names its
    /// package already, `MiniMax H3 Edit`.
    pub fn named(&self) -> String {
        if self.package_title.is_empty() || self.label.to_lowercase().contains(&self.package_title.to_lowercase()) {
            return self.label.clone();
        }
        format!("{} · {}", self.package_title, self.label)
    }
}

/// The model a tool runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolModel {
    /// Its key in a node's `models`.
    pub slot: String,
    /// What its package loads unless told otherwise: `[org/]model@release`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// That checkpoint's family, as its header says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// The kind of checkpoint this slot names by default, verified from its catalog or header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::model_ref::ModelKind>,
    /// The model class declared by the package; absent when an older interface did not name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// This slot reads raw weights rather than constructing a model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub weights: bool,
}

/// The control a box draws for a field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    #[default]
    Words,
    /// Text with mention chips.
    Prompt,
    Toggle,
    Number,
    Choice,
    /// The box's own shape.
    Shape,
    /// A drop target for a file.
    Slot,
    /// A block of fields, or a list of rows of them.
    Group,
    Seed,
    /// An Edit's clips.
    Timeline,
}

/// One thing a box asks for: a field of its tool's struct.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolField {
    /// Its key in the box's fields.
    pub field: String,
    #[serde(rename = "type")]
    pub ty: FieldType,
    pub label: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// `| None`: unset is Auto, the tool's choice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// `Advanced()`: drawn in a collapsed Advanced section under More.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    /// `When()`: it applies only while a field beside it has a value; else the box dulls it and keeps its value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
    /// A prompt that takes only these kinds of file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takes: Option<Vec<String>>,
    /// A number's and a shape's ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// What a number counts: `s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub integer: bool,
    /// A number in a list of rows whose sum the box shows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub total: bool,
    /// A choice's values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<Value>>,
    /// A choice's own words for its values, by value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<BTreeMap<String, String>>,
    /// What each of `options` does, by value: drawn as a menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptions: Option<BTreeMap<String, String>>,
    /// A shape that allows only these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratios: Option<Vec<Value>>,
    /// A slot: `image`, `video` or `audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<String>,
    /// A slot that takes the file's description with it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub described: bool,
    /// A slot of several, in order.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
    /// A video slot whose length is this list's timeline: `beats`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spans: Option<String>,
    /// A group's fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<ToolField>,
    /// A group of rows rather than one block.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub list: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
}

/// Timeline, the built-in video composition tool: clips on a track, rendered here.
pub fn edit() -> Tool {
    Tool {
        id: EDIT.to_owned(),
        package: "cozy".to_owned(),
        function: "edit".to_owned(),
        release: String::new(),
        name: "Video Timeline".to_owned(),
        label: "Video Timeline".to_owned(),
        package_title: "Cozy".to_owned(),
        makes: "video".to_owned(),
        description: "Put videos and stills together into one video, free, with no machine".to_owned(),
        local: false,
        installs: None,
        source: None,
        pickup: None,
        model: None,
        models: vec![],
        fields: vec![ToolField { field: "timeline".to_owned(), ty: FieldType::Timeline, label: "Video Timeline".to_owned(), ..ToolField::default() }],
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

/// What of `callable`'s result stays on its machine: its `Latents` fields, by name.
pub fn stays(callable: &Callable) -> Vec<String> {
    callable.result.fields.iter().filter(|field| matches!(field.ty.plain(), Type::Asset(kind) if kind == "latents")).map(|field| field.name.clone()).collect()
}

/// The field of `callable`'s result its words are in, for a box of text: its first plain text but `warnings`.
pub fn text_field(callable: &Callable) -> Option<&str> {
    callable.result.fields.iter().find(|field| field.name != "warnings" && *field.ty.plain() == Type::Str).map(|field| field.name.as_str())
}

/// The words `result` holds in `field`. None: none.
pub fn text_in(result: &Value, field: &str) -> Option<String> {
    result.get(field)?.as_str().filter(|text| !text.trim().is_empty()).map(str::to_owned)
}

/// What a box of `callable` is: its result's first picture, video or sound, else `text` for a result with words.
pub fn makes(callable: &Callable) -> Option<&str> {
    output_kind(callable).or_else(|| text_field(callable).map(|_| "text"))
}

/// A box's main text, where its words are written: its first prompt, else its first words (`str`), as the page's `mainText` finds it.
pub fn main_text(fields: &[ToolField]) -> Option<&ToolField> {
    fields.iter().find(|field| field.ty == FieldType::Prompt).or_else(|| fields.iter().find(|field| field.ty == FieldType::Words))
}

/// Where a box of a tool with `fields` reads a box of `kind`: its first required slot of that kind, else its first prompt. With `able`, only
/// a prompt that takes that kind: words (`text`) any prompt takes, as a mention of them is them.
pub fn reads_kind<'a>(fields: &'a [ToolField], kind: &str, able: bool) -> Option<&'a ToolField> {
    let slot = fields.iter().find(|field| field.required && field.ty == FieldType::Slot && field.accept.as_deref() == Some(kind));
    let takes = |field: &&ToolField| !able || kind == "text" || field.takes.as_ref().is_none_or(|kinds| kinds.iter().any(|one| one == kind));
    slot.or_else(|| fields.iter().filter(|field| field.ty == FieldType::Prompt).find(takes))
}

/// Every tool of a package's interface: each public function that declares `@app.tool`. `summary` is what the package does in a line, its
/// pyproject's description, which spells its name (`title`).
pub fn tools_of(package: &str, release: &str, interface: &Interface, summary: &str) -> Vec<Tool> {
    let callables = interface.public().into_iter().filter_map(|name| interface.callables.iter().find(|c| c.name == name));
    callables.filter_map(|callable| tool_of(package, release, interface, callable, summary)).collect()
}

fn tool_of(package: &str, release: &str, interface: &Interface, callable: &Callable, summary: &str) -> Option<Tool> {
    let declared = callable.tool.as_ref()?;
    let makes = makes(callable)?;
    let package = shown(package);
    let default = |field: &Field| callable.invocable.defaults.get(&format!("request/{}", field.name)).cloned().or_else(|| field.default.clone());
    let drawn = callable.request.fields.iter().filter(|field| !PLATFORM.contains(&field.name.as_str()));
    let called = interface.call_slots(callable);
    let mut seen = std::collections::BTreeSet::new();
    let models = callable.models.iter().map(|slot| (&slot.param, slot))
        .chain(called.iter().map(|slot| (&slot.path, *slot)))
        .filter(|(key, _)| seen.insert((*key).clone()))
        .map(|(key, slot)| model_slot(key, slot)).collect();
    Some(Tool {
        id: format!("{package}/{}", callable.name),
        name: box_name(&package, callable),
        label: if declared.label.is_empty() { capitalized(&words(&callable.name)) } else { declared.label.clone() },
        package_title: title(&package, summary),
        package,
        function: callable.name.clone(),
        release: release.to_owned(),
        makes: makes.to_owned(),
        description: callable.description.clone(),
        local: false,
        installs: None,
        source: None,
        fields: drawn.filter_map(|field| field_of(field, default(field))).collect(),
        pickup: pickup(&callable.request),
        model: choices::main_model(&callable.models, &called).map(|slot| model_slot(&slot.param, slot)),
        models,
    })
}

fn model_slot(key: &str, slot: &crate::interface::Slot) -> ToolModel {
    ToolModel {
        slot: key.to_owned(),
        default: slot.default.clone(),
        family: None,
        kind: None,
        class: (!slot.class.is_empty()).then(|| slot.class.clone()),
        weights: slot.weights,
    }
}

/// The frames a pickup of `request` may land on, when it takes one.
pub(crate) fn pickup(request: &Struct) -> Option<Snap> {
    let field = request.fields.iter().find(|field| matches!(field.ty.plain(), Type::Pickup(_)))?;
    Some(field.extra.get("snap").and_then(|snap| serde_json::from_value(snap.clone()).ok()).unwrap_or_default())
}

/// What a new box of `callable` is called, which counts up as it's taken: its `Node`'s word, else its package's, numbered.
pub fn box_name(package: &str, callable: &Callable) -> String {
    let said = callable.tool.as_ref().map(|tool| tool.named.as_str()).filter(|name| !name.is_empty());
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

/// A package in words: its name as its own description (`summary`) writes it, `minimax-h3`'s "MiniMax H3 video with sound…" MiniMax H3 and
/// `qwen-image-2`'s "…with Qwen Image 2.1" Qwen Image 2.1. One whose description doesn't name it: its last part's words, `krea-2` Krea 2,
/// and a word with no vowel upper case, `sdxl` SDXL.
pub fn title(package: &str, summary: &str) -> String {
    let name = package_word(package);
    if let Some(written) = written_in(name, summary) {
        return written.to_owned();
    }
    let words = name.split(['-', '_']).filter(|word| !word.is_empty());
    let word = |word: &str| match word.chars().any(|c| "aeiouAEIOU".contains(c)) {
        true => capitalized(word),
        false => word.to_uppercase(),
    };
    words.map(word).collect::<Vec<_>>().join(" ")
}

/// `name` where `text` first writes it: its letters and digits in any case, from the start of a word to the end of one, each `-`, `_`, `.`
/// or space of it written as any one of those or none, and its version: after a last number its `.1` (`qwen-image-2` as Qwen Image 2.1), after
/// a letter a number of its own (`qwen-image` as Qwen Image 1).
fn written_in<'a>(name: &str, text: &'a str) -> Option<&'a str> {
    const GAPS: [char; 4] = ['-', '_', '.', ' '];
    let wanted: Vec<char> = name.chars().collect();
    if wanted.iter().all(|c| GAPS.contains(c)) {
        return None;
    }
    let starts = text.char_indices().map(|(at, _)| at).filter(|&at| text[..at].chars().next_back().is_none_or(|before| !before.is_alphanumeric()));
    'start: for start in starts {
        let mut rest = text[start..].chars().peekable();
        let mut end = start;
        for want in wanted.iter().copied() {
            if GAPS.contains(&want) {
                if let Some(gap) = rest.next_if(|next| GAPS.contains(next)) {
                    end += gap.len_utf8();
                }
                continue;
            }
            match rest.next() {
                Some(next) if next.eq_ignore_ascii_case(&want) => end += next.len_utf8(),
                _ => continue 'start,
            }
        }
        if wanted.last().is_some_and(|last| last.is_ascii_digit()) {
            loop {
                let mut ahead = rest.clone();
                if !(ahead.next() == Some('.') && ahead.next().is_some_and(|digit| digit.is_ascii_digit())) {
                    break;
                }
                rest.next();
                end += 1;
                while rest.next_if(|digit| digit.is_ascii_digit()).is_some() {
                    end += 1;
                }
            }
        }
        if rest.peek().is_some_and(|next| next.is_alphanumeric()) {
            continue;
        }
        if !wanted.last().is_some_and(char::is_ascii_digit) {
            end += version_word(&text[end..]);
        }
        return Some(&text[start..end]);
    }
    None
}

/// The bytes of a version that starts `text` as a word of its own after a space, ` 1` or ` 2.1`; 0: none.
fn version_word(text: &str) -> usize {
    let Some(number) = text.strip_prefix(' ') else { return 0 };
    let bytes = number.as_bytes();
    let mut end = 0;
    loop {
        let digits = bytes[end..].iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits == 0 {
            break;
        }
        end += digits;
        if bytes.get(end) != Some(&b'.') || !bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
            break;
        }
        end += 1;
    }
    if end > 0 && number[end..].chars().next().is_none_or(|next| !next.is_alphanumeric()) { 1 + end } else { 0 }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// `field` as the box draws it, its author's `default` over its own. None: no box can draw it, so it keeps its default.
fn field_of(field: &Field, default: Option<Value>) -> Option<ToolField> {
    let extra = |key: &str| field.extra.get(key);
    let text = |key: &str| extra(key).and_then(Value::as_str).map(str::to_owned);
    let mut out = ToolField {
        field: field.name.clone(),
        label: text("label").unwrap_or_else(|| capitalized(&words(&field.name))),
        required: field.wire == Wire::Required,
        default: default.filter(|value| !value.is_null()),
        auto: field.ty.optional(),
        placeholder: text("placeholder"),
        advanced: extra("advanced").and_then(Value::as_bool) == Some(true),
        when: field.when(),
        ..ToolField::default()
    };
    let slot = |out: &mut ToolField, kind: &str, described: bool, multiple: bool| {
        out.ty = FieldType::Slot;
        out.accept = Some(kind.to_owned());
        (out.described, out.multiple, out.auto) = (described, multiple, false);
        out.spans = text("spans");
    };
    match field.ty.plain() {
        Type::Str => out.auto = false,
        Type::Prompt(kinds) => {
            out.ty = FieldType::Prompt;
            out.auto = false;
            // A prompt is written as its text.
            out.default = out.default.take().map(|value| value.get("text").cloned().unwrap_or(value)).filter(|text| text.as_str() != Some(""));
            out.takes = Some(kinds.clone()).filter(|kinds| !MEDIA.iter().all(|media| kinds.iter().any(|kind| kind == media)));
        }
        Type::Bool => out.ty = FieldType::Toggle,
        ty @ (Type::Int | Type::Float) => {
            let (min, max) = bounds(field)?;
            out.ty = FieldType::Number;
            (out.min, out.max) = (Some(min), Some(max));
            out.integer = *ty == Type::Int;
            out.step = field.constraints.multiple_of.or_else(|| number(field, "step"));
            out.unit = text("unit");
            out.total = extra("total").and_then(Value::as_bool) == Some(true);
        }
        Type::Literal(_) => {
            out.ty = FieldType::Choice;
            out.options = choices(field.ty.plain());
            let said = extra("options").and_then(Value::as_object);
            let key = |value: &Value| value.as_str().map_or_else(|| value.to_string(), str::to_owned);
            let of = |value: &Value| said.and_then(|said| said.get(&key(value)));
            let options = out.options.clone().unwrap_or_default();
            let words: BTreeMap<String, String> = options.iter().filter_map(|value| Some((key(value), of(value).and_then(|said| said.get("words").or(Some(said)))?.as_str()?.to_owned()))).collect();
            let descriptions: BTreeMap<String, String> = options.iter().filter_map(|value| Some((key(value), of(value)?.get("description")?.as_str()?.to_owned()))).collect();
            out.words = Some(words).filter(|words| !words.is_empty());
            out.descriptions = Some(descriptions).filter(|said| !said.is_empty());
            out.auto = false;
        }
        Type::Shape => {
            out.ty = FieldType::Shape;
            let ratios = extra("ratios");
            out.ratios = ratios.and_then(|ratios| ratios["allowed"].as_array()).cloned().filter(|allowed| !allowed.is_empty());
            (out.min, out.max) = (ratios.and_then(|ratios| ratios["min"].as_f64()), ratios.and_then(|ratios| ratios["max"].as_f64()));
            out.auto = true;
        }
        Type::Asset(kind) if MEDIA.contains(&kind.as_str()) => slot(&mut out, kind, false, false),
        Type::Media { kind, .. } => slot(&mut out, kind, true, false),
        Type::List(item) => match item.plain() {
            Type::Asset(kind) if MEDIA.contains(&kind.as_str()) => slot(&mut out, kind, false, true),
            Type::Media { kind, .. } => slot(&mut out, kind, true, true),
            Type::Struct(row) => {
                out.ty = FieldType::Group;
                out.list = true;
                out.auto = false;
                out.members = members(row);
                if out.members.is_empty() {
                    return None;
                }
                out.min_items = field.constraints.min_length;
                out.max_items = field.constraints.max_length;
            }
            _ => return None,
        },
        Type::Struct(row) => {
            out.ty = FieldType::Group;
            out.members = members(row);
            if out.members.is_empty() {
                return None;
            }
        }
        Type::Seed => {
            out.ty = FieldType::Seed;
            out.auto = false;
        }
        _ => return None,
    }
    Some(out)
}

/// What a box asks of a required `field` it has nothing in, in the words the box draws it with: the step (`choose`, `write`, …), its
/// label, and a choice's options as it shows them, `Character swap, Head swap or Style transfer`.
pub fn asked(field: &Field) -> (&'static str, String, Option<String>) {
    let Some(drawn) = field_of(field, None) else { return ("write", capitalized(&words(&field.name)), None) };
    let step = match drawn.ty {
        FieldType::Choice => "choose",
        FieldType::Slot => "add",
        FieldType::Number | FieldType::Toggle | FieldType::Shape | FieldType::Seed => "set",
        FieldType::Group => "fill in",
        FieldType::Words | FieldType::Prompt | FieldType::Timeline => "write",
    };
    let key = |value: &Value| value.as_str().map_or_else(|| value.to_string(), str::to_owned);
    let shown: Vec<String> = (drawn.options.iter().flatten())
        .map(|value| drawn.words.as_ref().and_then(|words| words.get(&key(value))).cloned().unwrap_or_else(|| key(value)))
        .collect();
    let options = match shown.split_last() {
        None => None,
        Some((only, [])) => Some(only.clone()),
        Some((last, rest)) => Some(format!("{} or {last}", rest.join(", "))),
    };
    (step, drawn.label, options)
}

/// A group's members as the box draws them.
fn members(row: &Struct) -> Vec<ToolField> {
    row.fields.iter().filter_map(|member| field_of(member, member.default.clone())).collect()
}

fn words(name: &str) -> String {
    name.replace('_', " ")
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
