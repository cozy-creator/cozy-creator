//! What a project says, as `project.db` holds it (cl-803): its head sections and its boxes in order, each box its fields in order. The
//! reader (`format::Doc`) reads it as it read project.yaml; YAML is only its export (`to_yaml`) and import (`from_yaml`). Every edit sets
//! values, so a write changes only the fields it names, and nothing is text surgery.
use api::{
    error::{Error, Kind},
    yaml::{child_keys_at, keys_at, to_json, to_json_and_keys},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::prefix_of,
    format::{FORMAT, INSTRUCTIONS, LEADS, TEXT, canonical, end_trimmed, invalid, names_a_file, yaml_key},
    graph::{mention, mention_moved, name_key},
};

/// Where an asset's tool lands.
pub(super) const TOOL: &str = "tool";
const SECTION: &str = "assets";
/// What a box holds whatever its tool: the platform's words.
const BOX: [&str; 6] = ["tool", "approved", "description", "models", "lora", "text"];

/// A project's boxes and what else it says, in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Written {
    /// Each section but `assets`: `version`, `defaults`.
    pub head: Vec<(String, Value)>,
    pub boxes: Vec<Boxed>,
}

/// One box: its name as written, and its fields in the order they were written.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Boxed {
    /// Stable logical identity; names and positions can change. Absent only while importing a legacy value.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    pub fields: Vec<(String, Value)>,
    /// Where it was in the list, for one taken out: a put back goes there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<usize>,
}

fn unknown(name: &str) -> Error {
    Error::new(Kind::Usage, "project_edit", format!("the project has no box {name}")).next("cozy status")
}

impl PartialEq for Boxed {
    fn eq(&self, other: &Self) -> bool {
        // History from before stable IDs still compares the actual edit, not its storage bookkeeping.
        self.name == other.name && self.fields == other.fields && self.at == other.at
    }
}

impl Boxed {
    pub(crate) fn get(&self, field: &str) -> Option<&Value> {
        self.fields.iter().find(|(held, _)| held == field).map(|(_, value)| value)
    }

    /// `field` set to `value`, where it was or at the end; a null takes it off.
    pub(crate) fn set(&mut self, field: &str, value: Value) {
        let at = self.fields.iter().position(|(held, _)| held == field);
        match (at, value) {
            (Some(at), Value::Null) => drop(self.fields.remove(at)),
            (Some(at), value) => self.fields[at].1 = value,
            (None, Value::Null) => {}
            (None, value) => self.fields.push((field.to_owned(), value)),
        }
    }

    /// Its fields as a map, for what reads them.
    pub(crate) fn map(&self) -> Map<String, Value> {
        self.fields.iter().cloned().collect()
    }

    /// What `PATCH`'s `If-Match` names: the digest of it, by value, so an edit of another moves nothing.
    pub(crate) fn etag(&self) -> String {
        let said = json!({"name": self.name, "fields": canonical(&Value::Object(self.map()))});
        format!("\"{}\"", hex::encode(Sha256::digest(said.to_string())))
    }
}

impl Written {
    /// A new project's: only its format.
    pub(crate) fn blank() -> Self {
        Self { head: vec![("version".to_owned(), json!(FORMAT))], boxes: vec![] }
    }

    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        let key = name_key(name);
        self.boxes.iter().position(|held| name_key(&held.name) == key)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Boxed> {
        self.find(name).map(|at| &self.boxes[at])
    }

    fn get_mut(&mut self, name: &str) -> Result<&mut Boxed, Error> {
        let at = self.find(name).ok_or_else(|| unknown(name))?;
        Ok(&mut self.boxes[at])
    }

    /// Every box's name key.
    pub(crate) fn names(&self) -> std::collections::HashSet<String> {
        self.boxes.iter().map(|held| name_key(&held.name)).collect()
    }

    /// What the reader reads: the sections as one mapping, `assets` among them, and the boxes' names in order.
    pub(crate) fn reading(&self) -> (Value, Vec<String>) {
        let mut top: Map<String, Value> = self.head.iter().cloned().collect();
        let assets: Map<String, Value> = self.boxes.iter().map(|held| (held.name.clone(), if held.fields.is_empty() { Value::Null } else { Value::Object(held.map()) })).collect();
        if !self.boxes.is_empty() {
            top.insert(SECTION.to_owned(), Value::Object(assets));
        }
        (Value::Object(top), self.boxes.iter().map(|held| held.name.clone()).collect())
    }

    /// A project.yaml of format 4 as rows. What doesn't read is refused with its line, and nothing changes.
    pub(crate) fn from_yaml(text: &str) -> Result<Self, Error> {
        let (value, listed) = to_json_and_keys(text, &[SECTION], names_a_file).map_err(|why| {
            let line = why.split("line ").nth(1).and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse::<usize>().ok());
            let written = line.and_then(|n| text.lines().nth(n.saturating_sub(1))).map(|line| format!(": `{}`", line.trim())).unwrap_or_default();
            invalid("project_yaml", format!("project.yaml: {why}{written}"))
        })?;
        let top = match value {
            Value::Object(top) => top,
            Value::Null => Map::new(),
            _ => return Err(invalid("project_yaml", "project.yaml is no mapping of sections".to_owned())),
        };
        let order = keys_at(text, &[]).unwrap_or_default();
        let ordered = |map: &Map<String, Value>, order: &[String]| -> Vec<(String, Value)> {
            let mut fields: Vec<(String, Value)> = order.iter().filter_map(|key| Some((key.clone(), map.get(key)?.clone()))).collect();
            fields.extend(map.iter().filter(|(key, _)| !order.contains(key)).map(|(key, value)| (key.clone(), value.clone())));
            fields
        };
        let head = ordered(&top, &order).into_iter().filter(|(key, _)| key != SECTION).collect();
        let empty = Map::new();
        let assets = top.get(SECTION).and_then(Value::as_object).unwrap_or(&empty);
        // Each box's fields in the order the text writes them, from one more read of it: a read per box made a 5,000-box import take minutes.
        let orders: std::collections::HashMap<String, Vec<String>> = child_keys_at(text, &[SECTION]).unwrap_or_default().into_iter().collect();
        let boxes = listed
            .iter()
            .filter_map(|name| {
                let fields = match assets.get(name)? {
                    Value::Object(fields) => ordered(fields, orders.get(name).map_or(&[][..], Vec::as_slice)),
                    _ => vec![],
                };
                Some(Boxed { name: name.clone(), fields, ..Default::default() })
            })
            .collect();
        Ok(Self { head, boxes })
    }

    /// As project.yaml, format 4: each section, then each box, each field on lines of its own. `from_yaml` reads it back the same.
    pub(crate) fn to_yaml(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.head {
            out.extend(emitted(key, value, 0));
        }
        if !self.boxes.is_empty() {
            out.push_str("\nassets:\n");
        }
        for (n, held) in self.boxes.iter().enumerate() {
            if n > 0 {
                out.push('\n');
            }
            match held.fields.is_empty() {
                true => out.push_str(&format!("  {}: {{}}\n", yaml_key(&held.name))),
                false => {
                    out.push_str(&format!("  {}:\n", yaml_key(&held.name)));
                    held.fields.iter().for_each(|(field, value)| out.extend(emitted(field, value, 4)));
                }
            }
        }
        out
    }

    /// Box `name`'s fields set, a null cleared, and what was said doing so. Setting what it holds changes nothing. A text is kept with
    /// nothing after its last word, as everything reads it: the space a chip leaves to type on isn't words, and a `|` block can't hold it.
    pub(crate) fn set(&mut self, name: &str, fields: &Map<String, Value>) -> Result<Vec<String>, Error> {
        let (translated, said) = super::tools::in_the_file(fields)?;
        let held = self.get_mut(name)?;
        for (field, mut value) in translated {
            end_trimmed(&mut value);
            if held.get(&field).map_or(value.is_null(), |now| canonical(now) == canonical(&value)) {
                continue;
            }
            held.set(&field, value);
        }
        Ok(said)
    }

    /// What a change of its tool writes of box `name`, with what `fields` ask, `tool` empty taking it off, and its fields with it (Paul,
    /// 11-06). Its pictures, numbers, approval and description stay. Only a box with none of those pictures has its words in another field:
    /// `text` with no tool, `instructions` with one. A tool going on a picture, `holds`, starts by reading it where `start` says.
    pub(crate) fn with_tool(&self, name: &str, tool: &str, holds: Option<u32>, start: Option<Start>, fields: &mut Map<String, Value>) -> Result<(), Error> {
        let held = self.get(name).ok_or_else(|| unknown(name))?.map();
        let said = fields.contains_key(INSTRUCTIONS) || fields.contains_key(TEXT);
        let moved = |fields: &mut Map<String, Value>, from: &str, to: &str| {
            if let Some(words) = held.get(from).filter(|_| !said) {
                fields.insert(to.to_owned(), words.clone());
                fields.insert(from.to_owned(), Value::Null);
            }
        };
        match (tool.is_empty(), holds) {
            (true, None) => LEADS.iter().rev().find(|field| held.contains_key(**field)).into_iter().for_each(|field| moved(fields, field, TEXT)),
            (false, None) => moved(fields, TEXT, INSTRUCTIONS),
            (false, Some(number)) if !said && !held.contains_key(TOOL) => {
                let own = format!("<self.{number}>");
                match start {
                    Some(Start::Slot(field)) if !held.contains_key(&field) => drop(fields.insert(field, json!(own))),
                    Some(Start::Prompt(field)) => {
                        let words = held.get(&field).and_then(Value::as_str).filter(|words| !words.contains(&own));
                        fields.insert(field, json!(words.map_or_else(|| own.clone(), |words| format!("{own} {words}"))));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        // Off, its fields go with it; what it was given back by Undo is a write of them.
        if tool.is_empty() {
            let gone: Vec<String> = held.keys().filter(|field| !BOX.contains(&field.as_str()) && !fields.contains_key(*field)).cloned().collect();
            fields.extend(gone.into_iter().map(|field| (field, Value::Null)));
        }
        fields.insert(TOOL.to_owned(), if tool.is_empty() { Value::Null } else { json!(tool) });
        Ok(())
    }

    /// A new box at the end, made by `tool`, or with none a text box.
    pub(crate) fn added(&mut self, name: &str, tool: &str, fields: &Map<String, Value>) -> Result<Vec<String>, Error> {
        let first = match tool.is_empty() {
            true => (TEXT.to_owned(), json!("")),
            false => (TOOL.to_owned(), json!(tool)),
        };
        self.push(Boxed { id: super::new_key(), name: name.to_owned(), fields: vec![first], at: None })?;
        self.set(name, fields)
    }

    /// A box holding one file: a name and the file it approves.
    pub(crate) fn constant(&mut self, name: &str, sha256: &str) -> Result<(), Error> {
        self.push(Boxed { id: super::new_key(), name: name.to_owned(), fields: vec![("approved".to_owned(), json!(prefix_of(sha256)))], at: None })
    }

    fn push(&mut self, held: Boxed) -> Result<(), Error> {
        if self.find(&held.name).is_some() {
            return Err(Error::new(Kind::Conflict, "asset_exists", format!("the project has a {} already", held.name)));
        }
        self.boxes.push(held);
        Ok(())
    }

    pub(crate) fn approved(&mut self, name: &str, sha256: &str) -> Result<(), Error> {
        self.set(name, &Map::from_iter([("approved".to_owned(), json!(prefix_of(sha256)))])).map(drop)
    }

    pub(crate) fn unapproved(&mut self, name: &str) -> Result<(), Error> {
        self.set(name, &Map::from_iter([("approved".to_owned(), Value::Null)])).map(drop)
    }

    /// Box `from` called `to`: its name, and every `<from>` and `<from.2>` in any box's words.
    pub(crate) fn renamed(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let at = self.find(from).ok_or_else(|| unknown(from))?;
        // `<from.2>` is a version of it, unless something from before names couldn't end so goes by that.
        let held = self.names();
        let renamed = |name: &str| mention_moved(name, from, to).filter(|_| name_key(name) == name_key(from) || !held.contains(&name_key(name)));
        for (_, value) in self.boxes.iter_mut().flat_map(|held| held.fields.iter_mut()).chain(self.head.iter_mut()) {
            mentions_renamed(value, &renamed);
        }
        to.clone_into(&mut self.boxes[at].name);
        Ok(())
    }

    /// Box `name` gone, and what puts it back where it was.
    pub(crate) fn removed(&mut self, name: &str) -> Result<Boxed, Error> {
        let at = self.find(name).ok_or_else(|| unknown(name))?;
        Ok(Boxed { at: Some(at), ..self.boxes.remove(at) })
    }

    /// What `removed` took, back where it was, else at the end, under the name it had.
    pub(crate) fn restored(&mut self, mut back: Boxed) -> Result<(), Error> {
        if self.find(&back.name).is_some() {
            return Err(Error::new(Kind::Conflict, "asset_exists", format!("the project has a {} already", back.name)));
        }
        let at = back.at.take().unwrap_or(self.boxes.len()).min(self.boxes.len());
        self.boxes.insert(at, back);
        Ok(())
    }

    /// Box `name`'s fields what they were once, where it is.
    pub(crate) fn replaced(&mut self, name: &str, back: &Boxed) -> Result<(), Error> {
        self.get_mut(name)?.fields.clone_from(&back.fields);
        Ok(())
    }
}

/// Where a tool going on a picture starts from it: its first required picture slot, else the start of its first prompt.
#[derive(Clone)]
pub(crate) enum Start {
    Slot(String),
    Prompt(String),
}

/// `value` with each `<name>` in it as `renamed` says.
pub(super) fn mentions_renamed(value: &mut Value, renamed: &dyn Fn(&str) -> Option<String>) {
    match value {
        Value::String(text) => *text = text_renamed(text, renamed),
        Value::Array(items) => items.iter_mut().for_each(|item| mentions_renamed(item, renamed)),
        Value::Object(fields) => fields.values_mut().for_each(|field| mentions_renamed(field, renamed)),
        _ => {}
    }
}

/// `text` with each `<name>` in it as `renamed` says.
pub(super) fn text_renamed(text: &str, renamed: &dyn Fn(&str) -> Option<String>) -> String {
    let (mut said, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let inner = after.find(['<', '>', '\n']).filter(|&end| after.as_bytes()[end] == b'>').map(|end| &after[..end]);
        said.push_str(&rest[..=open]);
        match inner.filter(|inner| mention(inner).is_some()).and_then(|inner| Some((inner, renamed(inner)?))) {
            Some((inner, to)) => {
                said.push_str(&to);
                rest = &after[inner.len()..];
            }
            None => rest = after,
        }
    }
    said.push_str(rest);
    said
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

/// A string as it's written on one line: plain where YAML reads it back as that string, else quoted as JSON, which is YAML.
fn scalar(text: &str) -> String {
    // Anything starting with a digit may be a number to some reader (`1:1` is 61 to YAML 1.1), so it's quoted.
    let indicator = text.starts_with(['-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', ' ', '\t', '.', '+']) || text.starts_with(|c: char| c.is_ascii_digit());
    let word = matches!(text.to_ascii_lowercase().as_str(), "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "y" | "n" | "~");
    let plain = !text.is_empty()
        && !indicator
        && !word
        && !text.ends_with([' ', ':', '\t'])
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.chars().any(char::is_control)
        && to_json(text).is_ok_and(|read| read == json!(text));
    if plain { text.to_owned() } else { Value::from(text).to_string() }
}

/// Words with a line break go as a `|` block where it reads back the same, else quoted.
fn block(text: &str) -> Option<(&'static str, Vec<&str>)> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let fits = text.contains('\n')
        && !text.starts_with([' ', '\n'])
        && !body.ends_with('\n')
        && !text.chars().any(|c| c.is_control() && c != '\n')
        && body.split('\n').all(|line| line == line.trim_end());
    fits.then(|| (if text.ends_with('\n') { "|" } else { "|-" }, body.split('\n').collect()))
}

fn value_lines(head: &str, value: &Value, indent: usize) -> Vec<String> {
    match value {
        Value::Array(items) if !items.is_empty() => std::iter::once(format!("{head}\n")).chain(items.iter().flat_map(|item| item_lines(item, indent))).collect(),
        Value::Object(fields) if !fields.is_empty() => std::iter::once(format!("{head}\n")).chain(fields.iter().flat_map(|(field, value)| emitted(field, value, indent))).collect(),
        Value::String(text) => match block(text) {
            Some((style, lines)) => std::iter::once(format!("{head} {style}\n")).chain(lines.into_iter().map(|line| if line.is_empty() { "\n".to_owned() } else { format!("{}{line}\n", pad(indent)) })).collect(),
            None => vec![format!("{head} {}\n", scalar(text))],
        },
        Value::Array(_) => vec![format!("{head} []\n")],
        Value::Object(_) => vec![format!("{head} {{}}\n")],
        other => vec![format!("{head} {other}\n")],
    }
}

/// `field: value` as lines of its own at `indent`.
fn emitted(field: &str, value: &Value, indent: usize) -> Vec<String> {
    value_lines(&format!("{}{}:", pad(indent), scalar(field)), value, indent + 2)
}

/// A list's item at `indent`: a mapping's first field on the dash's line.
fn item_lines(item: &Value, indent: usize) -> Vec<String> {
    match item {
        Value::Object(fields) if !fields.is_empty() => {
            let (first, rest): (Vec<_>, Vec<_>) = fields.iter().partition(|(field, _)| field.as_str() == "duration");
            let mut lines: Vec<String> = first.into_iter().chain(rest).flat_map(|(field, value)| emitted(field, value, indent + 2)).collect();
            lines[0] = format!("{}- {}", pad(indent), &lines[0][indent + 2..]);
            lines
        }
        item => value_lines(&format!("{}-", pad(indent)), item, indent + 2),
    }
}
