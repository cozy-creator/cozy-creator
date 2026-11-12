//! A project as a file of nodes (tracker design/agent-api.md): a map of node name to node. Each node is its `kind`, its words, one tool
//! block keyed by the tool's id (its fields, model slots by name, and `loras`), `approved`, its `takes`, and the `rev` it was read at.
//! What a file can hold comes from the tools' own declarations (`GET /v1/tools`), never from running a package: `schema` as JSON Schema
//! 2020-12, `explain` in words.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::tool::{EDIT, FieldType, Tool, ToolField};

/// What a node is: the page's node types.
pub const KINDS: [&str; 5] = ["image", "video", "audio", "text", "edit"];
/// A node's own keys, in the file's order; the tool block goes after `text`.
pub const KEYS: [&str; 6] = ["kind", "description", "text", "approved", "takes", "rev"];
/// A tool block's LoRAs, beside its fields and model slots.
pub const LORAS: &str = "loras";
/// What a take can be.
pub const STATES: [&str; 4] = ["done", "queued", "running", "failed"];

/// A node key that names a tool: `pkg/fn`, `org/pkg/fn@release`, or local code's `./dir/fn`.
pub fn is_tool(key: &str) -> bool {
    key.contains('/')
}

/// A tool id without its `@release`.
pub fn plain(id: &str) -> &str {
    id.split_once('@').map_or(id, |(plain, _)| plain)
}

/// What a node of a box is: an Edit's, else its file's kind.
pub fn kind_of<'a>(media: &'a str, tool: Option<&str>) -> &'a str {
    if tool.is_some_and(|tool| plain(tool) == EDIT) { "edit" } else { media }
}

/// The short id a read of the project at `heads` stamps on each node it gives: Calcifer maps it back to those heads.
pub fn rev_of(heads: &[String]) -> String {
    let mut heads = heads.to_vec();
    heads.sort();
    hex::encode(Sha256::digest(heads.join(",").as_bytes()))[..8].to_owned()
}

/// Edits from `a` to `b`, by character.
pub fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(diagonal + usize::from(x != y));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// What `word` is likely a typo of, among `known`: the closest within two edits, or one that starts with it.
pub fn near<'a>(word: &str, known: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let word = word.to_lowercase();
    let close = known.into_iter().map(|known| (edits(&word, &known.to_lowercase()), known));
    let close = close.filter(|(apart, known)| *apart <= 2.min(known.chars().count() / 2).max(1) || (word.len() >= 3 && known.starts_with(&word)));
    close.min_by_key(|(apart, _)| *apart).map(|(_, known)| known)
}

/// `GET /v1/projects/{id}/nodes`: the nodes asked, in the project's order, as the file holds them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Nodes {
    pub nodes: Map<String, Value>,
    /// How many takes of each node the file leaves out: the comment under its `takes:`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub older: BTreeMap<String, usize>,
    /// The rev each node carries.
    pub rev: String,
    /// What reading them said: a tool that can't be read, whose blocks show only what they set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `POST /v1/projects/{id}/apply`: nodes as a file holds them, each merged against its own rev. A take entry with only `file` names a
/// file held for the project (`sha256:<hex>`), which becomes the node's next take.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Apply {
    pub nodes: Map<String, Value>,
    /// Work it out and write nothing: `cozy diff`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// Where both sides changed a value, keep the file's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_conflicts: bool,
}

/// What an apply did, or a dry run would do. Refused (`errors`, or `conflicts` without `force_conflicts`), it wrote nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// It wrote: one transaction, one entry of the project's history.
    pub applied: bool,
    /// Each node of the file, in its order.
    pub nodes: Vec<Outcome>,
    /// Each value it changes, as it is and as it would be.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Changed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Conflict>,
    /// What refuses it, each with its fix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Said>,
    /// What it fixed and went on: a value clamped into its range, a mention of nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Said>,
    /// What it left as it is: a take's own fields, which are read-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Said>,
    /// What the project changed since a node's rev that the file left as it was: the project's stands, and the file is rewritten to say so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Said>,
    /// Once written, each node of the file as it stands, with its new rev.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged: Option<Nodes>,
    /// The commands that obviously follow: rendering what it changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
}

/// One node of an apply.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub name: String,
    /// `created`, `configured` or `unchanged`.
    pub did: String,
    /// What changed of it: `beats.0.description`, `approved 2`, `take 1`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
}

/// A value an apply changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Changed {
    /// `shot_2.beats.0.description`, or a node's name when it's made.
    pub path: String,
    pub before: Value,
    pub after: Value,
}

/// A value changed both in the file and in the project since the file's rev.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub yours: Value,
    pub theirs: Value,
    /// Who changed it since: `the page`, `the CLI`, `the API`.
    pub by: String,
    pub rev: String,
}

/// One line about one place in the file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Said {
    pub path: String,
    pub message: String,
}

impl Said {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { path: path.into(), message: message.into() }
    }
}

/// A tool's model slots as its block names them: each parameter once, a child call's `<call>.models.<parameter>` by its parameter, which a
/// box's `models` names every slot of. Its default with it.
pub fn model_slots(tool: &Tool) -> Vec<(String, Option<String>)> {
    let mut slots: Vec<(String, Option<String>)> = vec![];
    for slot in &tool.models {
        let key = slot.slot.rsplit_once(".models.").map_or(slot.slot.as_str(), |(_, param)| param);
        if !slots.iter().any(|(held, _)| held == key) {
            slots.push((key.to_owned(), slot.default.clone()));
        }
    }
    slots
}

/// A field's value when nothing sets it: its default, `[]` for a list, else null.
pub fn unset(field: &ToolField) -> Value {
    match (&field.default, field.ty) {
        (Some(value), _) => value.clone(),
        (None, FieldType::Group) if field.list => json!([]),
        (None, FieldType::Group) => Value::Object(field.members.iter().map(|member| (member.field.clone(), unset(member))).collect()),
        (None, FieldType::Slot) if field.multiple => json!([]),
        (None, FieldType::Timeline) => json!({"tracks": []}),
        (None, FieldType::Words | FieldType::Prompt) => json!(""),
        (None, _) => Value::Null,
    }
}

// The schema

const SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";
/// What a slot holds: a mention of a node, `<name>` or `<name.N>`.
const MENTION: &str = r"^<[A-Za-z0-9_-]+(\.[0-9]+)?>$";
const NAME: &str = r"^[A-Za-z0-9_-]+$";
const RATIO: &str = r"^[0-9]+(\.[0-9]+)?:[0-9]+(\.[0-9]+)?$";

/// JSON Schema 2020-12 for a file whose tool blocks are `tools`' (`cozy explain --schema`).
pub fn schema(tools: &[Tool]) -> Value {
    let mut defs = Map::new();
    let mut properties = Map::from_iter([
        ("kind".to_owned(), json!({"enum": KINDS, "description": "What it is. Implied by its tool, or by a take's file, on create"})),
        ("description".to_owned(), json!({"type": "string", "description": "What it is, for whatever mentions it"})),
        ("text".to_owned(), json!({"type": "string", "description": "A text node's words: a mention of it reads them in place"})),
        ("approved".to_owned(), json!({"type": ["integer", "null"], "minimum": 1, "description": "The take it stands on"})),
        ("takes".to_owned(), json!({"type": "array", "items": {"$ref": "#/$defs/take"}})),
        ("rev".to_owned(), json!({"type": "string", "readOnly": true, "description": "The version of it this file was read from: `cozy apply` merges against it"})),
    ]);
    let mut patterns = Map::new();
    let mut dependent = Map::new();
    let ids: Vec<&str> = tools.iter().map(|tool| tool.id.as_str()).collect();
    for tool in tools {
        let def = pointer(&tool.id);
        defs.insert(tool.id.clone(), block_schema(tool));
        properties.insert(tool.id.clone(), json!({"$ref": format!("#/$defs/{def}")}));
        patterns.insert(format!("^{}@[^/@]+$", regex_escaped(&tool.id)), json!({"$ref": format!("#/$defs/{def}")}));
        let others: Vec<Value> = ids.iter().filter(|id| **id != tool.id).map(|id| json!({"required": [id]})).collect();
        let mut tie = json!({"properties": {"kind": {"const": kind_of(&tool.makes, Some(&tool.id))}}});
        if !others.is_empty() {
            tie["not"] = json!({"anyOf": others});
        }
        dependent.insert(tool.id.clone(), tie);
    }
    defs.insert("take".to_owned(), json!({
        "type": "object",
        "description": "A take is read-only, but one with only `file` imports that file, relative to the file, as the next take",
        "properties": {
            "number": {"type": "integer", "readOnly": true},
            "state": {"enum": STATES, "readOnly": true},
            "file": {"type": "string"},
            "run": {"type": "integer", "readOnly": true},
        },
        "additionalProperties": false,
    }));
    defs.insert("node".to_owned(), json!({
        "type": "object",
        "properties": properties,
        "patternProperties": patterns,
        "additionalProperties": false,
        "dependentSchemas": dependent,
    }));
    json!({
        "$schema": SCHEMA,
        "title": "A Cozy project file: node name to node",
        "type": "object",
        "propertyNames": {"pattern": NAME},
        "additionalProperties": {"$ref": "#/$defs/node"},
        "$defs": defs,
    })
}

/// The schema of `tool`'s block: each field, each model slot, and `loras`.
pub fn block_schema(tool: &Tool) -> Value {
    let mut properties: Map<String, Value> = tool.fields.iter().map(|field| (field.field.clone(), field_schema(field))).collect();
    for (slot, default) in model_slots(tool) {
        let mut model = json!({"type": ["string", "null"], "description": "A model, [org/]name@release"});
        if let Some(default) = default {
            model["default"] = json!(default);
        }
        properties.insert(slot, model);
    }
    properties.insert(LORAS.to_owned(), json!({
        "type": "array",
        "items": {"type": "object", "properties": {"ref": {"type": "string"}, "strength": {"type": "number"}}, "required": ["ref"], "additionalProperties": false},
    }));
    let mut block = json!({"type": "object", "properties": properties, "additionalProperties": false});
    let description = [tool.label.as_str(), tool.description.as_str()].into_iter().filter(|said| !said.is_empty()).collect::<Vec<_>>().join(": ");
    if !description.is_empty() {
        block["description"] = json!(format!("{description}. Makes {}", tool.makes));
    }
    block
}

/// The schema of one field of a tool's struct.
pub fn field_schema(field: &ToolField) -> Value {
    let nullable = |ty: &str| if field.auto || !field.required { json!([ty, "null"]) } else { json!(ty) };
    let mut said = match field.ty {
        FieldType::Words | FieldType::Prompt => json!({"type": "string"}),
        FieldType::Toggle => json!({"type": nullable("boolean")}),
        FieldType::Number => {
            let mut number = json!({"type": nullable(if field.integer { "integer" } else { "number" })});
            if let Some(min) = field.min { number["minimum"] = json!(min); }
            if let Some(max) = field.max { number["maximum"] = json!(max); }
            if let Some(step) = field.step.filter(|step| *step > 0.0) { number["multipleOf"] = json!(step); }
            number
        }
        FieldType::Choice => {
            let mut options = field.options.clone().unwrap_or_default();
            if field.auto { options.push(Value::Null); }
            json!({"enum": options})
        }
        FieldType::Shape => match &field.ratios {
            Some(ratios) => json!({"enum": ratios.iter().cloned().chain([Value::Null]).collect::<Vec<_>>()}),
            None => json!({"type": ["string", "null"], "pattern": RATIO}),
        },
        FieldType::Slot => {
            let one = json!({"type": "string", "pattern": MENTION, "description": format!("a mention of a{} {} node", if field.accept.as_deref() == Some("image") || field.accept.as_deref() == Some("audio") { "n" } else { "" }, field.accept.as_deref().unwrap_or("media"))});
            if field.multiple { json!({"type": "array", "items": one}) } else { json!({"anyOf": [one, {"type": "null"}]}) }
        }
        FieldType::Group => {
            let members: Map<String, Value> = field.members.iter().map(|member| (member.field.clone(), field_schema(member))).collect();
            let row = json!({"type": "object", "properties": members, "additionalProperties": false});
            match field.list {
                true => {
                    let mut list = json!({"type": "array", "items": row});
                    if let Some(min) = field.min_items { list["minItems"] = json!(min); }
                    if let Some(max) = field.max_items { list["maxItems"] = json!(max); }
                    list
                }
                false => row,
            }
        }
        FieldType::Seed => json!({"type": ["integer", "null"], "minimum": 0, "description": "Null draws a new one each render"}),
        FieldType::Timeline => json!({
            "type": "object",
            "properties": {"tracks": {"type": "array", "items": {"type": "object", "properties": {"clips": {"type": "array", "items": {
                "type": "object",
                "properties": {"clip": {"type": "string", "pattern": MENTION}, "in": {"type": "integer", "minimum": 0}, "out": {"type": "integer", "minimum": 1}, "frames": {"type": "integer", "minimum": 1}},
                "required": ["clip"],
                "additionalProperties": false,
            }}}, "additionalProperties": false}}},
            "additionalProperties": false,
        }),
    };
    if field.label.to_lowercase() != field.field.replace('_', " ") {
        said["title"] = json!(field.label);
    }
    if let Some(default) = &field.default {
        said["default"] = default.clone();
    }
    said
}

/// `id` as one step of a JSON pointer.
fn pointer(id: &str) -> String {
    id.replace('~', "~0").replace('/', "~1")
}

fn regex_escaped(text: &str) -> String {
    text.chars().fold(String::new(), |mut out, c| {
        if "\\.^$|?*+()[]{}/".contains(c) {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

// Explain

/// `cozy explain` with nothing: what a file holds, and the tools a block can be.
pub fn explain_file(tools: &[Tool]) -> Vec<String> {
    let mut lines = vec![
        "A file: node name to node, each in this order:".to_owned(),
        row("kind", "image, video, audio, text or edit; implied by its tool or a take's file"),
        row("description", "text, for whatever mentions it"),
        row("text", "a text node's words"),
        row("<tool id>", "its fields, model slots and loras: cozy explain <tool id>"),
        row("approved", "a take number, or null"),
        row("takes", "read-only; an entry with only file: imports that file as the next take"),
        row("rev", "read-only: what apply merges against"),
        String::new(),
    ];
    lines.extend(tool_rows(tools.iter()));
    lines
}

/// `cozy explain <kind>`: a node of that kind, and the tools that make one.
pub fn explain_kind(kind: &str, tools: &[Tool]) -> Vec<String> {
    let making: Vec<&Tool> = tools.iter().filter(|tool| kind_of(&tool.makes, Some(&tool.id)) == kind).collect();
    let mut lines = vec![match kind {
        "text" => "text: words a mention reads in place; no tool, no takes".to_owned(),
        "edit" => "edit: videos and stills joined into one, by cozy/edit, free, with no machine".to_owned(),
        media => format!("{media}: a {media} file per take, made by its tool, or imported"),
    }];
    lines.extend(tool_rows(making.into_iter()));
    lines
}

fn tool_rows<'a>(tools: impl Iterator<Item = &'a Tool>) -> Vec<String> {
    let tools: Vec<&Tool> = tools.collect();
    if tools.is_empty() {
        return vec!["no tools listed: cozy package ls".to_owned()];
    }
    let width = tools.iter().map(|tool| tool.id.len()).max().unwrap_or(0);
    tools.iter().map(|tool| format!("{:width$}  {}, makes {}", tool.id, tool.label, kind_of(&tool.makes, Some(&tool.id)))).collect()
}

/// `cozy explain <tool>[.field…]`: the tool's block, or one field of it, in words.
pub fn explain_tool(tool: &Tool, path: &[&str]) -> Result<Vec<String>, String> {
    let slots = model_slots(tool);
    let models = || slots.iter().map(|(slot, _)| slot.as_str());
    let Some((first, rest)) = path.split_first() else {
        let release = if tool.release.is_empty() { String::new() } else { format!(" {}", tool.release) };
        let first = tool.description.split(['.', ':', '(']).next().unwrap_or_default().trim();
        let said = if first.is_empty() { tool.label.clone() } else { first.to_owned() };
        let mut lines = vec![format!("{}{release}: {said}. Makes {}", tool.id, kind_of(&tool.makes, Some(&tool.id)))];
        let mut rows = vec![];
        fields(&tool.fields, 0, &mut rows);
        for (slot, default) in &slots {
            rows.push((0, slot.clone(), default.as_ref().map_or_else(|| "model".to_owned(), |default| format!("model, default {default}"))));
        }
        rows.push((0, LORAS.to_owned(), "list of ref and strength".to_owned()));
        lines.extend(aligned(&rows));
        return Ok(lines);
    };
    let Some(field) = tool.fields.iter().find(|field| field.field == *first) else {
        if let Some((_, default)) = slots.iter().find(|(slot, _)| slot == first) {
            return Ok(vec![format!("{}.{first}: model{}", tool.id, default.as_ref().map_or_else(String::new, |default| format!(", default {default}")))]);
        }
        if *first == LORAS {
            return Ok(vec![format!("{}.{LORAS}: list of ref (a LoRA, [org/]name@release) and strength (default 1)", tool.id)]);
        }
        let known = tool.fields.iter().map(|field| field.field.as_str()).chain(models()).chain([LORAS]);
        return Err(no_field(&tool.id, first, near(first, known)));
    };
    let mut field = field;
    let mut at = format!("{}.{first}", tool.id);
    for step in rest {
        let Some(member) = field.members.iter().find(|member| member.field == *step) else {
            return Err(no_field(&at, step, near(step, field.members.iter().map(|member| member.field.as_str()))));
        };
        at = format!("{at}.{step}");
        field = member;
    }
    let mut lines = vec![format!("{at}: {}", words(field))];
    let mut rows = vec![];
    fields(&field.members, 0, &mut rows);
    lines.extend(aligned(&rows));
    Ok(lines)
}

fn no_field(at: &str, field: &str, near: Option<&str>) -> String {
    match near {
        Some(near) => format!("{at} has no field {field}. Did you mean {near}?"),
        None => format!("{at} has no field {field}"),
    }
}

fn fields(fields: &[ToolField], depth: usize, rows: &mut Vec<(usize, String, String)>) {
    for field in fields {
        rows.push((depth, field.field.clone(), words(field)));
        fields_of_group(field, depth, rows);
    }
}

fn fields_of_group(field: &ToolField, depth: usize, rows: &mut Vec<(usize, String, String)>) {
    if field.ty == FieldType::Group {
        fields(&field.members, depth + 1, rows);
    }
}

fn aligned(rows: &[(usize, String, String)]) -> Vec<String> {
    let width = rows.iter().map(|(depth, name, _)| depth * 2 + name.len()).max().unwrap_or(0) + 2;
    rows.iter().map(|(depth, name, said)| format!("{:width$}{said}", format!("{}{name}", "  ".repeat(*depth)))).collect()
}

fn row(name: &str, said: &str) -> String {
    format!("{name:13}{said}")
}

/// A field's type in words, its range, choices and default.
pub fn words(field: &ToolField) -> String {
    let default = |shown: &str| if field.default.is_some() { format!(", default {shown}") } else { String::new() };
    let value = |value: &Value| value.as_str().map_or_else(|| value.to_string(), str::to_owned);
    let default_value = field.default.as_ref().map(value).unwrap_or_default();
    let said = match field.ty {
        FieldType::Words => "text".to_owned(),
        FieldType::Prompt => match &field.takes {
            Some(kinds) => format!("text; <name> mentions a node ({} only)", or_list(kinds)),
            None => "text; <name> mentions a node".to_owned(),
        },
        FieldType::Toggle => format!("true or false{}{}", if field.auto { ", or null for the tool's choice" } else { "" }, default(&default_value)),
        FieldType::Number => {
            let number = |n: f64| if n.fract() == 0.0 { format!("{n:.0}") } else { n.to_string() };
            let range = match (field.min, field.max) {
                (Some(min), Some(max)) => format!("{}-{}", number(min), number(max)),
                _ => "a number".to_owned(),
            };
            let unit = field.unit.as_ref().map_or_else(String::new, |unit| format!(" {unit}"));
            let integer = if field.integer && field.step.is_none() { ", whole" } else { "" };
            let step = field.step.filter(|step| *step != 1.0).map_or_else(String::new, |step| format!(", in steps of {}", number(step)));
            format!("{range}{unit}{integer}{step}{}", default(&default_value))
        }
        FieldType::Choice => {
            let options: Vec<String> = field.options.iter().flatten().map(value).collect();
            format!("{}{}", or_list(&options), default(&default_value))
        }
        FieldType::Shape => match &field.ratios {
            Some(ratios) => format!("a ratio: {}{}", or_list(&ratios.iter().map(value).collect::<Vec<_>>()), default(&default_value)),
            None => format!("a ratio, width:height{}", default(&default_value)),
        },
        FieldType::Slot => {
            let one = format!("<{} node>", field.accept.as_deref().unwrap_or("media"));
            let described = if field.described { ", with its description" } else { "" };
            if field.multiple { format!("list of {one}{described}") } else { format!("{one}{described}") }
        }
        FieldType::Group if field.list => {
            let mut list = "list".to_owned();
            if let Some(min) = field.min_items.filter(|min| *min > 0) { list.push_str(&format!(", at least {min}")); }
            if let Some(max) = field.max_items { list.push_str(&format!(", at most {max}")); }
            list
        }
        FieldType::Group => "group".to_owned(),
        FieldType::Seed => "integer, default new each render".to_owned(),
        FieldType::Timeline => "tracks of clips, each clip: <name>, with in and out frames for part of a video, or frames for a still".to_owned(),
    };
    let label = (field.label.to_lowercase() != field.field.replace('_', " ")).then(|| field.label.to_lowercase());
    match (field.advanced, label) {
        (true, Some(label)) => format!("advanced: {label}; {said}"),
        (true, None) => format!("advanced; {said}"),
        (false, _) => said,
    }
}

/// `a`, `a or b`, `a, b or c`.
fn or_list(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}
