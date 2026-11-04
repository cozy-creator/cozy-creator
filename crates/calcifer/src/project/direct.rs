//! An asset of a function no adapter speaks for: its fields are the call, by the function's typed inputs, and each `<mention>` becomes
//! the file it names. This speaks in a node's flat fields and what it reads, and knows nothing of the file they came from.
use protocol::{
    error::{Error, Kind},
    interface::{Callable, Constraints, Field, Struct, Type, Wire},
    tool,
};
use serde_json::{Map, Value, json};

use super::{
    folder::{Files, Lock},
    format::ReadNow,
    graph::{mentions, mentions_in, name_key},
};

/// A file in `files` as a function's input takes it: a digest. `image`, then a list's `image-2`, `image-3`, ... .
pub fn files_of<'a>(files: &'a Files, kind: &str) -> Vec<&'a str> {
    let mut listed: Vec<(u32, &str)> = (files.iter())
        .filter_map(|(key, sha256)| Some((key.strip_prefix(kind)?.strip_prefix('-')?.parse().ok()?, sha256.as_str())))
        .collect();
    listed.sort_unstable();
    files.get(kind).map(String::as_str).into_iter().chain(listed.into_iter().map(|(_, sha256)| sha256)).collect()
}

/// A mention's name as a model reads it: `<coast.2>` is `<coast 2>`.
fn spoken(name: &str) -> String {
    name.replace('.', " ")
}

struct Filling<'a> {
    reads: &'a [ReadNow],
    lock: &'a Lock,
    /// Which of `reads` a field has taken.
    taken: Vec<bool>,
    warnings: Vec<String>,
    name: &'a str,
}

impl Filling<'_> {
    /// The read a mention names, marked taken.
    fn read(&mut self, mention: &str) -> Option<&ReadNow> {
        let key = name_key(mention);
        let at = self.reads.iter().position(|read| name_key(&read.name) == key)?;
        self.taken[at] = true;
        Some(&self.reads[at])
    }

    /// A file field's value: each mention in it as the file it names of `kind`, or a hash as the file it starts.
    fn files(&mut self, value: &Value, kind: &str) -> Vec<Value> {
        match value {
            Value::Array(items) => items.iter().flat_map(|item| self.files(item, kind)).collect(),
            Value::String(text) if mentions(text).is_empty() => match self.lock.matching(&text.to_ascii_lowercase()).as_slice() {
                [file] => vec![json!(file.sha256)],
                _ => vec![value.clone()],
            },
            Value::String(text) => {
                let name = self.name.to_owned();
                let mut found = vec![];
                for mention in mentions(text) {
                    match self.read(mention).map(|read| files_of(&read.files, kind).into_iter().map(|sha256| json!(sha256)).collect::<Vec<_>>()) {
                        Some(files) if !files.is_empty() => found.extend(files),
                        Some(_) => self.warnings.push(format!("{name}: <{mention}> has no {kind}, so it isn't sent")),
                        None => {}
                    }
                }
                found
            }
            _ => vec![],
        }
    }

    /// `value` as a field of type `ty` takes it.
    fn typed(&mut self, ty: &Type, value: &Value) -> Value {
        match (ty, value) {
            (Type::Asset(kind), _) => self.files(value, kind).into_iter().next().unwrap_or(Value::Null),
            (Type::List(item), _) if matches!(&**item, Type::Asset(_)) => {
                let Type::Asset(kind) = &**item else { unreachable!() };
                Value::Array(self.files(value, kind))
            }
            (Type::List(item), Value::Array(values)) => Value::Array(values.iter().map(|value| self.typed(item, value)).collect()),
            (Type::Struct(members), Value::Object(values)) => Value::Object(self.members(members, values)),
            (Type::Union { branches, .. }, _) if branches.len() == 2 && branches.contains(&Type::Null) && !value.is_null() => {
                let other = branches.iter().find(|branch| **branch != Type::Null).expect("two branches");
                self.typed(other, value)
            }
            _ => value.clone(),
        }
    }

    fn members(&mut self, members: &Struct, values: &Map<String, Value>) -> Map<String, Value> {
        let typed = |member: &str| members.fields.iter().find(|field| field.name == member);
        values.iter().map(|(key, value)| (key.clone(), typed(key).map_or_else(|| value.clone(), |field| self.typed(&field.ty, value)))).collect()
    }

    /// A list of named files, `[{name, <file>}]`, of what's left that has one: each with the name the text says.
    fn named(&mut self, members: &Struct) -> Option<Vec<Value>> {
        members.fields.iter().find(|field| field.name == "name" && field.ty == Type::Str)?;
        let member = members.fields.iter().find(|field| field.ty.media().is_some_and(|(_, many)| !many))?;
        let (kind, _) = member.ty.media()?;
        let mut entries = vec![];
        for (at, read) in self.reads.iter().enumerate().filter(|(at, _)| !self.taken[*at]).map(|(at, read)| (at, read)).collect::<Vec<_>>() {
            let files = files_of(&read.files, kind);
            self.taken[at] |= !files.is_empty();
            let name = spoken(&read.name);
            entries.extend(files.iter().enumerate().map(|(number, sha256)| {
                let shown = if number > 0 { format!("{name} {}", number + 1) } else { name.clone() };
                json!({"name": shown, member.name.as_str(): sha256})
            }));
        }
        Some(entries)
    }

    /// What's left, for a field no file named: a file field takes the first that has one, a list all.
    fn fill(&mut self, callable: &Callable, field: &Field) -> Option<Value> {
        if callable.is_assets(&field.name) {
            let kinds = callable.assets.as_ref().into_iter().flat_map(|assets| assets.kinds.iter().map(|kind| kind.kind.clone())).collect::<Vec<_>>();
            let mut entries = vec![];
            for (at, read) in self.reads.iter().enumerate().filter(|(at, _)| !self.taken[*at]).map(|(at, read)| (at, read)).collect::<Vec<_>>() {
                for kind in &kinds {
                    let files = files_of(&read.files, kind);
                    self.taken[at] |= !files.is_empty();
                    entries.extend(files.iter().map(|sha256| json!({"asset": sha256, "label": spoken(&read.name)})));
                }
            }
            return (!entries.is_empty()).then_some(Value::Array(entries));
        }
        match plain(&field.ty) {
            Type::Asset(kind) => {
                let at = (0..self.reads.len()).find(|&at| !self.taken[at] && !files_of(&self.reads[at].files, kind).is_empty())?;
                self.taken[at] = true;
                files_of(&self.reads[at].files, kind).first().map(|sha256| json!(sha256))
            }
            Type::List(item) => match &**item {
                Type::Asset(kind) => {
                    let mut files = vec![];
                    for at in 0..self.reads.len() {
                        let found = files_of(&self.reads[at].files, kind);
                        if !self.taken[at] && !found.is_empty() {
                            self.taken[at] = true;
                            files.extend(found.iter().map(|sha256| json!(sha256)));
                        }
                    }
                    (!files.is_empty()).then_some(Value::Array(files))
                }
                Type::Struct(members) => self.named(members).filter(|entries| !entries.is_empty()).map(Value::Array),
                _ => None,
            },
            _ => None,
        }
    }
}

fn plain(ty: &Type) -> &Type {
    match ty {
        Type::Union { branches, .. } if branches.len() == 2 && branches.contains(&Type::Null) => branches.iter().find(|branch| **branch != Type::Null).unwrap_or(ty),
        _ => ty,
    }
}

/// What `function` of a node `name` makes is called with: its `fields`, flat, by the function's typed inputs. Each `<mention>` in a file
/// input is the file it names; what's left of `reads` fills what has no value, a list of named pictures or a file input, in the order
/// the text says them, and a text keeps its mentions as words. Past a bound is clamped and said. Err: it needs what it hasn't got, and
/// no run is sent.
pub fn input(name: &str, tool: &str, callable: &Callable, fields: &Map<String, Value>, reads: &[ReadNow], lock: &Lock) -> Result<(Map<String, Value>, Vec<String>), Error> {
    let mut filling = Filling { reads, lock, taken: vec![false; reads.len()], warnings: vec![], name };
    let mut input = Map::new();
    let declared = &callable.request.fields;
    let text = tool::text_input(&callable.request).map(|field| field.name.as_str());
    let is_text = |field: &str| Some(field) == text;
    for field in declared {
        let named = if is_text(&field.name) { fields.get(tool::TEXT) } else { None };
        let Some(value) = named.or_else(|| fields.get(&field.name)) else { continue };
        let value = match plain(&field.ty) {
            Type::Str => spoken_in(value),
            _ => filling.typed(&field.ty, value),
        };
        input.insert(field.name.clone(), value);
    }
    for field in declared {
        if !input.contains_key(&field.name)
            && let Some(value) = filling.fill(callable, field)
        {
            input.insert(field.name.clone(), value);
        }
    }
    filling.warnings.extend(conform(name, &callable.request, &mut input));
    let reads_field = |written: &str| declared.iter().any(|field| field.name == written) || (written == tool::TEXT && text.is_some());
    let mut warnings = filling.warnings;
    // A node's description is for what mentions it, never its own maker.
    warnings.extend(fields.keys().filter(|field| *field != "description" && !reads_field(field)).map(|field| format!("{name}: {tool} reads no field {field}")));
    for (read, _) in reads.iter().zip(&filling.taken).filter(|(read, taken)| !**taken && !read.files.is_empty()) {
        warnings.push(format!("{name}: {tool} has nowhere for <{}>'s file, so it isn't sent", read.name));
    }
    let missing: Vec<&str> = (declared.iter())
        .filter(|field| field.wire == Wire::Required && !input.contains_key(&field.name) && field.ty != Type::Model && field.ty != Type::Tree)
        .map(|field| field.name.as_str())
        .collect();
    if !missing.is_empty() {
        let said = format!("{name} can't run: {tool} needs {}", missing.join(", "));
        return Err(Error::new(Kind::Usage, "tool_input_missing", said));
    }
    Ok((input, warnings))
}

fn spoken_in(value: &Value) -> Value {
    let Value::String(text) = value else { return value.clone() };
    let mut out = text.clone();
    for mention in mentions_in(value) {
        out = out.replace(&format!("<{mention}>"), &format!("<{}>", spoken(&mention)));
    }
    Value::String(out)
}

/// `fields` held to what `declared` allows, with a line for each that moved: a number past its bounds to the nearest, a choice it doesn't
/// offer to no value, so its function's default stands, and a list past its length to its first.
pub fn conform(name: &str, declared: &Struct, fields: &mut Map<String, Value>) -> Vec<String> {
    let mut said = vec![];
    for field in &declared.fields {
        let key = field.name.as_str();
        let Some(value) = fields.get_mut(key) else { continue };
        match plain(&field.ty) {
            Type::Int | Type::Float if value.is_number() => {
                let (low, high) = limits(&field.constraints, field);
                let at = value.as_f64().unwrap_or_default();
                let held = at.clamp(low.unwrap_or(f64::NEG_INFINITY), high.unwrap_or(f64::INFINITY));
                if held != at {
                    said.push(format!("{name}'s {key}: {at} is past what it takes, so it's {held}"));
                    *value = if *plain(&field.ty) == Type::Int { json!(held as i64) } else { json!(held) };
                }
            }
            Type::Literal(members) if !value.is_null() && !members.contains(value) => {
                let offered: Vec<String> = members.iter().filter(|member| !member.is_null()).map(|member| member.as_str().map_or_else(|| member.to_string(), str::to_owned)).collect();
                said.push(format!("{name}'s {key}: {value} isn't one of {}, so it's ignored", offered.join(", ")));
                fields.remove(key);
            }
            Type::List(_) => {
                if let (Some(most), Value::Array(items)) = (field.constraints.max_length, value)
                    && items.len() as u64 > most
                {
                    said.push(format!("{name}'s {key}: {} given and it takes {most}, so it uses the first {most}", items.len()));
                    items.truncate(most as usize);
                }
            }
            _ => {}
        }
    }
    said
}

/// A number's ends: what it declares, else what a box is told.
fn limits(constraints: &Constraints, field: &Field) -> (Option<f64>, Option<f64>) {
    let extra = |key: &str| field.extra.get(key).and_then(Value::as_f64);
    (constraints.ge.or(constraints.gt).or_else(|| extra("min")), constraints.le.or(constraints.lt).or_else(|| extra("max")))
}

