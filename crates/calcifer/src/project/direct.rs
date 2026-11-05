//! An asset's call, for every function: its typed inputs, from what the asset writes and what it mentions. A text keeps its mentions, a
//! file input takes the file one names, and a list of named structs (`Struct::is_mention`) is filled from what its texts mention, each
//! member by its name. Then what's past a bound is clamped and said. This speaks in inputs and what it reads, and knows no model.
use std::collections::BTreeSet;

use protocol::{
    error::{Error, Kind},
    interface::{Callable, Constraints, Field, Node, Struct, TAKE, Type, Wire},
    tool,
};
use serde_json::{Map, Value, json};

use super::{
    folder::{self, Files, Lock},
    format::Given,
    graph::{mentions, name_key},
};

/// The files a mention is by name: what's main, and a character's voice. Any other role is an output, its package's own.
const ROLES: [&str; 4] = ["image", "video", "audio", "voice"];

/// A file in `files` as a function's input takes it: a digest. `image`, then a list's `image-2`, `image-3`, ... .
pub fn files_of<'a>(files: &'a Files, kind: &str) -> Vec<&'a str> {
    let mut listed: Vec<(u32, &str)> = (files.iter())
        .filter_map(|(key, sha256)| Some((key.strip_prefix(kind)?.strip_prefix('-')?.parse().ok()?, sha256.as_str())))
        .collect();
    listed.sort_unstable();
    files.get(kind).map(String::as_str).into_iter().chain(listed.into_iter().map(|(_, sha256)| sha256)).collect()
}

/// A text as a function reads it: an asset's dotted name is the project's, so `<kyogre.primordial>` reads `<kyogre primordial>`.
fn spoken(text: &str) -> String {
    let mut out = text.to_owned();
    for mention in mentions(text).into_iter().filter(|mention| mention.contains('.')) {
        out = out.replace(&format!("<{mention}>"), &format!("<{}>", mention.replace('.', " ")));
    }
    out
}

/// Whether `made_by`, `org/package/function@release`, is of `package`: a package reads only its own outputs.
fn made_here(made_by: &str, package: &str) -> bool {
    let of = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    let function = made_by.split('@').next().unwrap_or(made_by);
    function.rsplit_once('/').is_some_and(|(made, _)| of(made) == of(package))
}

/// The role of `files` a member takes: its own name, else its kind of file.
fn role<'a>(member: &'a Field, kind: &'a str) -> &'a str {
    if ROLES.contains(&member.name.as_str()) || kind == "file" { &member.name } else { kind }
}

struct Filling<'a> {
    reads: &'a [Given],
    lock: &'a Lock,
    /// What's making the call, whose outputs it may read.
    package: &'a str,
    /// Which of `reads` a field has taken.
    taken: Vec<bool>,
    warnings: Vec<String>,
    name: &'a str,
}

impl Filling<'_> {
    /// The read a mention names, marked taken.
    fn read(&mut self, mention: &str) -> Option<&Given> {
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
            (Type::Str, Value::String(text)) => json!(spoken(text)),
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

    /// The main file of a read, measured.
    fn measured(&self, read: &Given) -> Option<&folder::File> {
        self.lock.files.get(folder::main_of(&read.files)?)
    }

    /// What `member` of a mention is, in the platform's words or the asset's own, but for its files.
    fn word(&self, read: &Given, member: &str) -> Option<Value> {
        let text = |text: &str| (!text.is_empty()).then(|| json!(text));
        match member {
            "name" => Some(json!(read.name.replace('.', " "))),
            "description" => text(&read.description),
            "asset_type" => text(&read.asset_type),
            "made_by" => text(&read.made_by),
            "alpha" => Some(self.measured(read)?.alpha).filter(|alpha| *alpha != 0.0).map(|alpha| json!(alpha)),
            "width" => Some(self.measured(read)?.width).filter(|width| *width != 0).map(|width| json!(width)),
            "height" => Some(self.measured(read)?.height).filter(|height| *height != 0).map(|height| json!(height)),
            field => read.inputs.get(field).cloned(),
        }
    }

    /// A read's file of `role`, in the package that made it if it's no name of a mention's own.
    fn output<'r>(&self, read: &'r Given, role: &str) -> Vec<&'r str> {
        let open = ROLES.contains(&role) || made_here(&read.made_by, self.package);
        if open { files_of(&read.files, role) } else { vec![] }
    }

    /// What's mentioned in a text, for a list of `members`: an entry per read, member by member. With one file member, an entry per
    /// file, the rest `<name 2>`, `<name 3>`, and what only words can say goes on the first. With more, each takes its first.
    fn mentioned(&mut self, members: &Struct, said: &BTreeSet<String>) -> Vec<Value> {
        let holds: Vec<(&Field, &str)> = members.fields.iter().filter_map(|field| Some((field, field.ty.media().filter(|(_, many)| !many)?.0))).collect();
        let words: Vec<&Field> = members.fields.iter().filter(|field| field.ty.media().is_none()).collect();
        let mut entries = vec![];
        for at in 0..self.reads.len() {
            let read = &self.reads[at];
            if !said.contains(&name_key(&read.name)) {
                continue;
            }
            let entry = |this: &Self| -> Map<String, Value> { words.iter().filter_map(|field| Some((field.name.clone(), this.word(read, &field.name)?))).collect() };
            let mut sent = false;
            if let [(member, kind)] = holds.as_slice() {
                let files = self.output(read, role(member, kind));
                let shown = read.name.replace('.', " ");
                for (number, sha256) in files.iter().enumerate() {
                    let mut one = if number == 0 { entry(self) } else { Map::from_iter([("name".to_owned(), json!(format!("{shown} {}", number + 1)))]) };
                    one.insert(member.name.clone(), json!(sha256));
                    entries.push(Value::Object(one));
                    sent = true;
                }
                if files.is_empty() {
                    match member.wire {
                        Wire::Required => self.warnings.push(format!("{}: <{}> has no {kind}, so it isn't sent", self.name, read.name)),
                        _ => entries.push(Value::Object(entry(self))),
                    }
                }
            } else {
                let mut one = entry(self);
                for (member, kind) in &holds {
                    let files = self.output(read, role(member, kind));
                    if files.len() > 1 {
                        self.warnings.push(format!("{}: <{}> has {} files in `{}`, and this takes one: it uses the first", self.name, read.name, files.len(), member.name));
                    }
                    if let Some(sha256) = files.first() {
                        one.insert(member.name.clone(), json!(sha256));
                        sent = true;
                    }
                }
                entries.push(Value::Object(one));
            }
            self.taken[at] |= sent;
        }
        entries
    }

    /// `source`, `<input>.<member>`: that of what `input` mentions.
    fn from_mention(&mut self, inputs: &Map<String, Value>, source: &str) -> Option<Value> {
        let (input, member) = source.split_once('.')?;
        let mention = mentions(inputs.get(input)?.as_str()?).first().copied()?.to_owned();
        let read = self.read(&mention)?.clone();
        let files = self.output(&read, member);
        match files.first() {
            Some(sha256) => Some(json!(sha256)),
            None => self.word(&read, member),
        }
    }

    /// The take a new one keeps the start of, as `members` takes it: a file member its take hasn't got means it can't be continued.
    fn take(&self, members: &Struct, take: &Value) -> Result<Value, Error> {
        let made_by = take["made_by"].as_str().unwrap_or_default();
        let mut kept = Map::new();
        for member in &members.fields {
            match (member.ty.media(), member.name.as_str()) {
                (Some(_), field) => match take[field]["sha256"].as_str().filter(|_| ROLES.contains(&field) || made_here(made_by, self.package)) {
                    Some(sha256) => drop(kept.insert(field.to_owned(), json!(sha256))),
                    None => {
                        let (name, number) = (self.name, &take["take"]);
                        let said = format!("{name}: take {number} has no {field} this package can read, so it can't be continued: nothing is made");
                        return Err(Error::new(Kind::Conflict, "take_not_continued", said).next(format!("cozy project render {name}")));
                    }
                },
                (None, field) if !take[field].is_null() => drop(kept.insert(field.to_owned(), take[field].clone())),
                _ => {}
            }
        }
        Ok(Value::Object(kept))
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
                    entries.extend(files.iter().map(|sha256| json!({"asset": sha256, "label": read.name})));
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

/// Every text under `value`, by its type: what a mention in it is a mention to read, and what a run's words are.
fn texts<'v>(ty: &Type, value: &'v Value, found: &mut Vec<&'v str>) {
    match (plain(ty), value) {
        (Type::Str, Value::String(text)) => found.push(text),
        (Type::List(item), Value::Array(values)) => values.iter().for_each(|value| texts(item, value, found)),
        (Type::Struct(members), Value::Object(values)) => {
            for field in &members.fields {
                if let Some(value) = values.get(&field.name) {
                    texts(&field.ty, value, found);
                }
            }
        }
        _ => {}
    }
}

/// Whether a field asks for words: a text, or rows with one.
fn wants_words(ty: &Type) -> bool {
    match plain(ty) {
        Type::Str => true,
        Type::List(item) => matches!(&**item, Type::Struct(row) if !row.is_mention() && row.fields.iter().any(|member| *plain(&member.ty) == Type::Str)),
        _ => false,
    }
}

/// One asset's call: what it writes, and what it reads.
pub struct Call<'a> {
    pub name: &'a str,
    /// As the file writes it.
    pub tool: &'a str,
    pub package: &'a str,
    pub callable: &'a Callable,
    pub inputs: &'a Map<String, Value>,
    /// The inputs it wrote itself: what `defaults` gave it and the function doesn't read isn't said.
    pub own: &'a BTreeSet<String>,
    pub reads: &'a [Given],
    pub lock: &'a Lock,
    /// `Doc::keep`, the take a new one keeps the start of.
    pub take: Option<&'a Value>,
}

/// What `callable` is called with. Err: it needs what it hasn't got, and no run is sent.
pub fn input(call: Call) -> Result<(Map<String, Value>, Vec<String>), Error> {
    let Call { name, tool, package, callable, inputs, own, reads, lock, take } = call;
    let mut filling = Filling { reads, lock, package, taken: vec![false; reads.len()], warnings: vec![], name };
    let node = callable.node.clone().unwrap_or_else(Node::default);
    let declared = &callable.request.fields;
    let text = tool::text_input(&callable.request).map(|field| field.name.as_str());
    let is_text = |field: &str| Some(field) == text;
    let mut input = Map::new();
    let mut written = vec![];
    for field in declared {
        let named = if is_text(&field.name) { inputs.get(tool::TEXT) } else { None };
        let Some(value) = named.or_else(|| inputs.get(&field.name)) else { continue };
        texts(&field.ty, value, &mut written);
        input.insert(field.name.clone(), filling.typed(&field.ty, value));
    }
    let said: BTreeSet<String> = written.iter().flat_map(|text| mentions(text)).map(name_key).collect();
    for (field, source) in &node.fill {
        let Some(declared) = callable.field(field) else { continue };
        if input.contains_key(field) {
            continue;
        }
        let filled = match (source.as_str(), plain(&declared.ty), take) {
            (TAKE, Type::Struct(members), Some(take)) => Some(filling.take(members, take)?),
            (TAKE, ..) => None,
            _ => filling.from_mention(inputs, source),
        };
        if let Some(value) = filled {
            input.insert(field.clone(), value);
        }
    }
    for field in declared {
        if input.contains_key(&field.name) {
            continue;
        }
        let filled = match plain(&field.ty) {
            Type::List(item) => match &**item {
                Type::Struct(row) if row.is_mention() => Some(Value::Array(filling.mentioned(row, &said))).filter(|entries| entries.as_array().is_some_and(|entries| !entries.is_empty())),
                _ => filling.fill(callable, field),
            },
            _ => filling.fill(callable, field),
        };
        if let Some(value) = filled {
            input.insert(field.name.clone(), value);
        }
    }
    // A list of rows is as long as its type says: a box opens with those, and what they don't say is their defaults.
    for field in declared {
        if let (Type::List(item), Some(least)) = (plain(&field.ty), field.constraints.min_length)
            && matches!(&**item, Type::Struct(row) if !row.is_mention())
            && (field.wire == Wire::Required || input.contains_key(&field.name))
            && let Value::Array(rows) = input.entry(field.name.clone()).or_insert_with(|| json!([]))
        {
            rows.resize_with((least as usize).max(rows.len()), || json!({}));
        }
    }
    filling.warnings.extend(conform(name, &callable.request, &mut input));
    let reads_field = |written: &str| declared.iter().any(|field| field.name == written) || (written == tool::TEXT && text.is_some());
    let mut warnings = filling.warnings;
    warnings.extend(own.iter().filter(|field| !reads_field(field)).map(|field| format!("{name}: {tool} reads no input {field}")));
    for (read, _) in reads.iter().zip(&filling.taken).filter(|(read, taken)| !**taken && !said.contains(&name_key(&read.name)) && !read.files.is_empty()) {
        warnings.push(format!("{name}: {tool} has nowhere for <{}>'s file, so it isn't sent", read.name));
    }
    // A function that wants words and has none has nothing to make, and it costs nothing to say so.
    let wants = declared.iter().filter(|field| field.wire == Wire::Required && !node.fill.contains_key(&field.name)).any(|field| wants_words(&field.ty));
    if wants && written.iter().all(|text| text.trim().is_empty()) {
        let field = declared.iter().find(|field| wants_words(&field.ty)).map_or(tool::TEXT, |field| if is_text(&field.name) { tool::TEXT } else { &field.name });
        return Err(Error::new(Kind::Usage, "tool_input_missing", format!("{name} says nothing to make: write its {field}")));
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

/// `fields` held to what `declared` allows, with a line for each that moved: a number past its bounds to the nearest, a choice it doesn't
/// offer to no value, so its function's default stands, a list past its length to its first, and each row of a list the same.
fn conform(name: &str, declared: &Struct, fields: &mut Map<String, Value>) -> Vec<String> {
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
                if let (Some(most), Value::Array(items)) = (field.constraints.max_length, &mut *value)
                    && items.len() as u64 > most
                {
                    said.push(format!("{name}'s {key}: {} given and it takes {most}, so it uses the first {most}", items.len()));
                    items.truncate(most as usize);
                }
                if let (Type::List(item), Value::Array(items)) = (plain(&field.ty), value)
                    && let Type::Struct(row) = &**item
                {
                    for (at, one) in items.iter_mut().enumerate() {
                        if let Value::Object(one) = one {
                            said.extend(conform(&format!("{name}'s {key} {}", at + 1), row, one));
                        }
                    }
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

