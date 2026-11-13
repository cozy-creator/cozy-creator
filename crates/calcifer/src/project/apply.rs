//! `POST /v1/projects/{id}/apply` (tracker design/agent-api.md): a file of nodes merged into the project, each against the rev it was read
//! at, three ways. A value the file changes and the project didn't since is written; one the project changed and the file didn't stands;
//! one both changed conflicts, unless it's words whose edits don't overlap, which merge by character through the document's own text
//! (`Document::typed`, the path two page editors take). Everything is checked first and every error said with its fix; a conflict or an
//! error writes nothing. The writes are the page's own (`assets::made`, `assets::patched`, an import's element, a version joined), in one
//! transaction and one history entry, which the page's Undo takes back whole.
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use api::{
    calcifer::{
        node::{self, Applied, Apply, Changed, Conflict, Outcome, Said},
        project::{AssetPatch, NewAsset, Splice},
        tool::{FieldType, Tool, ToolField},
    },
    error::{Error, Kind},
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
};
use serde_json::{Map, Value, json};

use super::{
    Doc, Project, assets,
    crdt::Document,
    folder::{self, Folder, Lock},
    format::{self, NODE_KIND, TEXT, canonical},
    graph::{self, name_key},
    history,
    nodes::{self, Shown, View},
    written::{Boxed, Written},
};
use crate::api::{Api, Refusal, parse};

/// What a check says, by where in the file.
#[derive(Default)]
struct Told {
    errors: Vec<Said>,
    warnings: Vec<Said>,
    notes: Vec<Said>,
}

impl Told {
    fn error(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(Said::new(path, message));
    }

    fn warn(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(Said::new(path, message));
    }
}

/// One node of the file as written.
struct Wrote {
    name: String,
    rev: Option<String>,
    kind: Option<String>,
    description: Option<String>,
    text: Option<String>,
    /// Its tool block, by the tool as written; a null block takes the tool off.
    tool: Option<(String, Option<Map<String, Value>>)>,
    approved: Option<Option<u32>>,
    /// What its takes import: held files, `sha256:<hex>`, in order.
    files: Vec<String>,
    /// Its other take entries, which are read-only.
    takes: Vec<Value>,
}

impl Wrote {
    /// Whether it says anything but its rev.
    fn says(&self) -> bool {
        self.kind.is_some() || self.description.is_some() || self.text.is_some() || self.tool.is_some() || self.approved.is_some() || !self.files.is_empty()
    }
}

/// Node `name` as the file writes it, each of its keys checked.
fn wrote(name: &str, value: &Value, told: &mut Told) -> Option<Wrote> {
    let Value::Object(node) = value else {
        told.error(name, "a node is a mapping of its fields: kind, its tool's id, approved…");
        return None;
    };
    let mut wrote = Wrote { name: name.to_owned(), rev: None, kind: None, description: None, text: None, tool: None, approved: None, files: vec![], takes: vec![] };
    let words = |key: &str, value: &Value, told: &mut Told| match value {
        Value::String(text) => Some(text.clone()),
        Value::Null => Some(String::new()),
        other => {
            told.error(format!("{name}.{key}"), format!("words, not {other}"));
            None
        }
    };
    let mut tools = vec![];
    for (key, value) in node {
        let path = format!("{name}.{key}");
        match key.as_str() {
            "kind" => match value.as_str().filter(|kind| node::KINDS.contains(kind)) {
                Some(kind) => wrote.kind = Some(kind.to_owned()),
                None => told.error(path, format!("kind is image, video, audio, text or edit, not {value}")),
            },
            "description" => wrote.description = words(key, value, told),
            "text" => wrote.text = words(key, value, told),
            "rev" => match value {
                Value::String(rev) => wrote.rev = Some(rev.clone()),
                Value::Null => {}
                other => told.error(path, format!("a rev is what cozy get printed, not {other}")),
            },
            "approved" => match value {
                Value::Null => wrote.approved = Some(None),
                value => match value.as_u64().and_then(|number| u32::try_from(number).ok()).filter(|number| *number > 0) {
                    Some(number) => wrote.approved = Some(Some(number)),
                    None => told.error(path, format!("approved is a take's number, or null, not {value}")),
                },
            },
            "takes" => match value {
                Value::Null => {}
                Value::Array(takes) => {
                    for (at, take) in takes.iter().enumerate() {
                        match take.as_object() {
                            Some(entry) if entry.len() == 1 && entry.contains_key("file") => match entry["file"].as_str() {
                                Some(file) => wrote.files.push(file.to_owned()),
                                None => told.error(format!("{path}.{at}.file"), "a file is its path"),
                            },
                            Some(_) => wrote.takes.push(take.clone()),
                            None => told.error(format!("{path}.{at}"), "a take is a mapping: {file: <path>} imports one"),
                        }
                    }
                }
                other => told.error(path, format!("takes are a list, not {other}")),
            },
            tool if node::is_tool(tool) => match value {
                Value::Object(block) => tools.push((tool.to_owned(), Some(block.clone()))),
                Value::Null => tools.push((tool.to_owned(), None)),
                other => told.error(path, format!("a tool's block is a mapping of its fields, or null to take it off, not {other}")),
            },
            other => {
                let fix = node::near(other, node::KEYS).map_or_else(String::new, |near| format!(". Did you mean {near}?"));
                told.error(path, format!("a node has no field {other}{fix}"));
            }
        }
    }
    match tools.len() {
        0 | 1 => wrote.tool = tools.pop(),
        n => told.error(name, format!("one tool block, not {n}: {}", tools.iter().map(|(tool, _)| tool.as_str()).collect::<Vec<_>>().join(", "))),
    }
    Some(wrote)
}

/// What a node of the plan writes.
enum Write {
    /// A box made, as a drop of its tool, or of its first file (`element`), then each other file a take of it.
    New { ask: NewAsset, element: bool, files: Vec<String>, approved: Option<u32> },
    /// A box changed: its fields and tool as the page's controls set them, its approval, and each file a take of it.
    Edit { ask: AssetPatch, approved: Option<Option<u32>>, files: Vec<String> },
}

/// One node of the file, worked out.
struct Planned {
    name: String,
    did: &'static str,
    changed: Vec<String>,
    write: Option<Write>,
    /// The box it was worked out on: the act writes only while it's still that.
    on: Option<Boxed>,
}

/// What the plan reads, and what it says.
struct Planning<'a> {
    folder: &'a Folder,
    lock: &'a Lock,
    written: &'a Written,
    document: &'a Document,
    tools: &'a HashMap<String, Result<Tool, Error>>,
    /// Each rev of the file: the heads it stands for, and the boxes then.
    bases: &'a HashMap<String, (Vec<String>, Written)>,
    /// Every name the project and the file have, by key: what a mention can name.
    names: BTreeSet<String>,
    /// What a file take imports, by its source: the field its bytes are.
    fields: &'a HashMap<String, &'static str>,
    force: bool,
    told: Told,
    changes: Vec<Changed>,
    conflicts: Vec<Conflict>,
    kept: Vec<Said>,
}

impl Planning<'_> {
    fn tool(&mut self, path: &str, id: &str) -> Option<Tool> {
        match self.tools.get(id) {
            Some(Ok(tool)) => Some(tool.clone()),
            Some(Err(why)) => {
                let known = self.tools.iter().filter(|(_, read)| read.is_ok()).map(|(id, _)| id.as_str());
                let fix = node::near(id, known).map_or_else(String::new, |near| format!(": did you mean {near}?"));
                self.told.error(path, format!("{id} can't be read: {}{fix}", why.message));
                None
            }
            None => None,
        }
    }

    fn plan(&mut self, wrote: &Wrote) -> Option<Planned> {
        match self.written.get(&wrote.name).cloned() {
            None => self.new_node(wrote),
            Some(boxed) => self.existing(wrote, boxed),
        }
    }

    /// A name the project doesn't have: a node made.
    fn new_node(&mut self, wrote: &Wrote) -> Option<Planned> {
        let name = &wrote.name;
        if let Err(why) = format::new_name(name) {
            self.told.error(name.as_str(), why.message);
            return None;
        }
        if let Some(rev) = &wrote.rev
            && self.bases.get(rev).is_some_and(|(_, then)| then.get(name).is_some())
        {
            self.told.error(name.as_str(), format!("deleted since its rev {rev}: take its rev out to make it anew"));
            return None;
        }
        let mut fields = Map::new();
        if let Some(description) = wrote.description.as_ref().filter(|words| !words.trim().is_empty()) {
            fields.insert("description".to_owned(), json!(description));
        }
        let approved = wrote.approved.flatten();
        if approved.is_some_and(|number| number as usize > wrote.files.len()) {
            self.told.error(format!("{name}.approved"), format!("a new node's takes are its files: it has {}", wrote.files.len()));
        }
        let mut changed: Vec<String> = [("description", wrote.description.is_some()), (TEXT, wrote.text.is_some())].into_iter().filter(|(_, said)| *said).map(|(key, _)| key.to_owned()).collect();
        let (ask, element, kind) = match &wrote.tool {
            Some((id, None)) => {
                self.told.error(format!("{name}.{id}"), "null takes a tool off; a new node has none");
                return None;
            }
            Some((id, Some(block))) => {
                let tool = self.tool(&format!("{name}.{id}"), id)?;
                let kind = node::kind_of(&tool.makes, Some(id)).to_owned();
                if let Some(said) = wrote.kind.as_ref().filter(|said| **said != kind) {
                    self.told.error(format!("{name}.kind"), format!("{id} makes {kind}, not {said}"));
                }
                let empty = nodes::block(&Map::new(), Some(&tool));
                let asked = self.checked_block(name, id, &tool, block)?;
                let mut merged = empty.clone();
                merged.extend(asked);
                fields.extend(writes(&Map::new(), &empty, &merged, &tool));
                changed.push(id.clone());
                let media = if kind == "edit" { "video".to_owned() } else { tool.makes.clone() };
                (NewAsset { tool: id.clone(), name: Some(name.clone()), exact: true, fields, kind: Some(media), ..empty_ask() }, false, kind)
            }
            None => match (wrote.files.first(), wrote.kind.as_deref()) {
                (Some(first), kind) => {
                    let field = self.fields.get(first).copied();
                    if let (Some(field), Some(kind)) = (field, kind)
                        && field != kind
                    {
                        self.told.error(format!("{name}.kind"), format!("its file is {field}, not {kind}"));
                    }
                    let shown = field.or(kind).unwrap_or("image").to_owned();
                    (NewAsset { name: Some(name.clone()), exact: true, fields, ..empty_ask() }, true, shown)
                }
                (None, Some("edit")) => {
                    self.told.error(format!("{name}.kind"), "an edit's tool is cozy/edit: write its block, cozy/edit: {timeline: …}");
                    return None;
                }
                (None, Some(TEXT)) | (None, None) if wrote.text.is_some() || wrote.kind.is_some() => {
                    fields.insert(TEXT.to_owned(), json!(wrote.text.clone().unwrap_or_default()));
                    (NewAsset { name: Some(name.clone()), exact: true, fields, kind: Some(TEXT.to_owned()), ..empty_ask() }, false, TEXT.to_owned())
                }
                (None, Some(kind)) => {
                    if wrote.text.is_some() {
                        self.told.error(format!("{name}.text"), "only a text node has text");
                    }
                    fields.insert(NODE_KIND.to_owned(), json!(kind));
                    (NewAsset { name: Some(name.clone()), exact: true, fields, kind: Some(kind.to_owned()), ..empty_ask() }, false, kind.to_owned())
                }
                (None, None) => {
                    self.told.error(name.as_str(), "a new node needs a kind, a tool's block or a take's file");
                    return None;
                }
            },
        };
        changed.extend((1..=wrote.files.len()).map(|number| format!("take {number}")));
        // As kubectl diff shows an object made: its name and kind, then each field it sets on a line of its own.
        self.changes.push(Changed { path: name.clone(), before: Value::Null, after: json!({"kind": kind}) });
        if !ask.tool.is_empty() {
            self.changes.push(Changed { path: format!("{name}.tool"), before: Value::Null, after: json!(ask.tool) });
        }
        for (field, value) in ask.fields.iter().filter(|(field, _)| *field != NODE_KIND) {
            self.changes.push(Changed { path: format!("{name}.{field}"), before: Value::Null, after: value.clone() });
        }
        for number in 1..=wrote.files.len() {
            self.changes.push(Changed { path: format!("{name}.takes.{}", number - 1), before: Value::Null, after: json!(format!("take {number}, from its file")) });
        }
        Some(Planned { name: name.clone(), did: "created", changed, write: Some(Write::New { ask, element, files: wrote.files.clone(), approved }), on: None })
    }

    /// A node the project has: merged against its rev.
    fn existing(&mut self, wrote: &Wrote, boxed: Boxed) -> Option<Planned> {
        let name = boxed.name.clone();
        let now = nodes::view(&boxed, self.lock, self.folder);
        let tool_now = now.tool.as_ref().and_then(|id| self.tools.get(id)).and_then(|read| read.as_ref().ok()).cloned();
        let Some(rev) = &wrote.rev else {
            if wrote.says() && !self.same_as(wrote, &now, tool_now.as_ref()) {
                self.told.error(name.as_str(), format!("no rev; run `cozy get {name}` first"));
                return None;
            }
            return Some(Planned { name, did: "unchanged", changed: vec![], write: None, on: Some(boxed) });
        };
        let (heads, then) = self.bases.get(rev)?;
        let Some(was) = then.boxes.iter().find(|held| held.id == boxed.id).map(|held| nodes::view(held, self.lock, self.folder)) else {
            self.told.error(name.as_str(), format!("not in the project at its rev {rev}: run `cozy get {name}` and edit that"));
            return None;
        };
        let heads = heads.clone();
        let mut ask = AssetPatch { fields: Map::new(), tool: None, typed: None };
        let mut changed = vec![];
        let path = |key: &str| format!("{name}.{key}");
        if let Some(kind) = &wrote.kind {
            self.kept(&path("kind"), &json!(kind), &json!(was.kind), &json!(now.kind), &name, Some(NODE_KIND), rev);
        }
        if let Some(kind) = &wrote.kind
            && let Some(kind) = self.scalar(&path("kind"), &json!(kind), &json!(was.kind), &json!(now.kind), rev, &name, Some(NODE_KIND))
        {
            let kind = kind.as_str().unwrap_or_default().to_owned();
            if kind != now.kind {
                match (now.tool.is_some() || matches!(&wrote.tool, Some((_, Some(_)))), format::KINDS.contains(&kind.as_str()) && format::KINDS.contains(&now.kind.as_str())) {
                    (false, true) => {
                        ask.fields.insert(NODE_KIND.to_owned(), json!(kind));
                        changed.push(format!("kind {kind}"));
                    }
                    (true, _) => self.told.error(path("kind"), format!("a node's tool makes its kind: {name} is {}", now.kind)),
                    (false, false) => self.told.error(path("kind"), format!("{name} is {}: a {} node can't become {kind}; make a new node", now.kind, now.kind)),
                }
            }
        }
        for (key, mine, was_words, now_words) in [
            ("description", &wrote.description, Some(&was.description), Some(&now.description)),
            (TEXT, &wrote.text, was.text.as_ref(), now.text.as_ref()),
        ] {
            let Some(mine) = mine else { continue };
            let Some(now_words) = now_words else {
                self.told.error(path(key), "only a text node has text");
                continue;
            };
            let was_words = was_words.cloned().unwrap_or_default();
            self.kept(&path(key), &json!(mine), &json!(was_words), &json!(now_words), &name, Some(key), rev);
            if let Some(words) = self.words(&path(key), &now.id, key, &heads, rev, &name, &was_words, mine, now_words)
                && canonical(&json!(words)) != canonical(&json!(now_words))
            {
                ask.fields.insert(key.to_owned(), json!(words));
                self.change(&path(key), json!(now_words), json!(words));
                changed.push(key.to_owned());
            }
        }
        if let Some((id, block)) = &wrote.tool {
            self.tool_block(&name, id, block.as_ref(), &was, &now, &heads, rev, &mut ask, &mut changed);
        }
        let mut approved = None;
        if let Some(mine) = wrote.approved {
            let numbers: Vec<u32> = self.lock.asset(&name).into_iter().flat_map(|held| &held.versions).map(|version| version.number).collect();
            let numbered = self.lock.asset(&name).map_or(0, |held| held.numbered);
            let coming = |number: u32| number > numbered && (number - numbered) as usize <= wrote.files.len();
            match mine {
                Some(number) if !numbers.contains(&number) && !coming(number) => {
                    let has = if numbers.is_empty() { "none".to_owned() } else { numbers.iter().map(u32::to_string).collect::<Vec<_>>().join(", ") };
                    self.told.error(path("approved"), format!("{name} has no take {number}: it has {has}"));
                }
                _ => {
                    self.kept(&path("approved"), &json!(mine), &json!(was.approved), &json!(now.approved), &name, Some("approved"), rev);
                    if let Some(merged) = self.scalar(&path("approved"), &json!(mine), &json!(was.approved), &json!(now.approved), rev, &name, Some("approved"))
                        && merged != json!(now.approved)
                    {
                        let number = merged.as_u64().and_then(|number| u32::try_from(number).ok());
                        approved = Some(number);
                        self.change(&path("approved"), json!(now.approved), merged);
                        changed.push(number.map_or_else(|| "unapproved".to_owned(), |number| format!("approved {number}")));
                    }
                }
            }
        }
        let numbered = self.lock.asset(&name).map_or(0, |held| held.numbered);
        for (at, _) in wrote.files.iter().enumerate() {
            changed.push(format!("take {}", numbered as usize + at + 1));
        }
        if !wrote.files.is_empty() {
            self.change(&path("takes"), Value::Null, json!(wrote.files.len()));
        }
        if !wrote.takes.is_empty() {
            let shown = nodes::takes(&name, self.lock, &self.folder.dir, &[]);
            let listed = |take: &Value| shown.iter().any(|held| take.get("number").and_then(Value::as_u64) == held.number.map(u64::from)
                && take.get("file").is_none_or(|file| file.as_str() == held.file.as_deref()));
            if !wrote.takes.iter().all(listed) {
                self.told.notes.push(Said::new(path("takes"), "a take's own fields are read-only, and leaving one out deletes nothing: left as they are"));
            }
        }
        let write = (!ask.fields.is_empty() || ask.tool.is_some() || approved.is_some() || !wrote.files.is_empty())
            .then(|| Write::Edit { ask, approved, files: wrote.files.clone() });
        let did = if write.is_some() { "configured" } else { "unchanged" };
        Some(Planned { name, did, changed, write, on: Some(boxed) })
    }

    /// Whether what `wrote` says of a node with no rev is what it holds now: then it changes nothing.
    fn same_as(&self, wrote: &Wrote, now: &View, tool: Option<&Tool>) -> bool {
        let words = |mine: &Option<String>, now: &str| mine.as_ref().is_none_or(|mine| canonical(&json!(mine)) == canonical(&json!(now)));
        let block = match &wrote.tool {
            None => true,
            Some((id, Some(block))) if Some(id) == now.tool.as_ref() => {
                let held = nodes::block(&now.fields, tool);
                block.iter().all(|(key, value)| held.get(key).is_some_and(|held| canonical(held) == canonical(value)))
            }
            Some(_) => false,
        };
        wrote.kind.as_ref().is_none_or(|kind| *kind == now.kind)
            && words(&wrote.description, &now.description)
            && words(&wrote.text, now.text.as_deref().unwrap_or_default())
            && block
            && wrote.approved.is_none_or(|approved| approved == now.approved)
            && wrote.files.is_empty()
    }

    /// The tool block of the file against the box's then and now.
    #[allow(clippy::too_many_arguments)]
    fn tool_block(&mut self, name: &str, id: &str, block: Option<&Map<String, Value>>, was: &View, now: &View, heads: &[String], rev: &str, ask: &mut AssetPatch, changed: &mut Vec<String>) {
        let path = format!("{name}.{id}");
        let same_tool = |a: Option<&String>| a.is_some_and(|a| node::plain(a) == node::plain(id) || a == id);
        let Some(block) = block else {
            // Null: the tool off, if it's the one it has.
            match (same_tool(now.tool.as_ref()), same_tool(was.tool.as_ref())) {
                (true, _) => {
                    ask.tool = Some(String::new());
                    self.change(&path, json!(id), Value::Null);
                    changed.push(format!("{id} off"));
                }
                (false, true) => self.told.error(path, format!("its tool is {} since its rev {rev}: run `cozy get {name}` and edit that", now.tool.as_deref().unwrap_or("none"))),
                (false, false) => self.told.error(path, format!("{name} has no tool {id} to take off")),
            }
            return;
        };
        // A tool that can't be read: a block that says what the box holds asks nothing of it.
        if self.tools.get(id).is_some_and(Result::is_err) && same_tool(now.tool.as_ref()) {
            let held = nodes::block(&now.fields, None);
            if block.iter().all(|(key, value)| held.get(key).is_some_and(|held| canonical(held) == canonical(value)) || (value.is_null() || value == &json!([])) && !held.contains_key(key)) {
                return;
            }
        }
        let Some(tool) = self.tool(&path, id) else { return };
        let mine_tool = !same_tool(was.tool.as_ref());
        let theirs_tool = was.tool != now.tool;
        if !mine_tool && theirs_tool {
            // The project changed its tool since: the file's block is of the one it had.
            let held = nodes::block(&was.fields, Some(&tool));
            let untouched = block.iter().all(|(key, value)| held.get(key).is_some_and(|held| canonical(held) == canonical(value)));
            if !untouched {
                self.told.error(path, format!("its tool is {} since its rev {rev}: run `cozy get {name}` and edit that", now.tool.as_deref().unwrap_or("none")));
            }
            return;
        }
        if mine_tool {
            if theirs_tool && !same_tool(now.tool.as_ref()) {
                self.told.error(path, format!("its tool is {} since its rev {rev}: run `cozy get {name}` and edit that", now.tool.as_deref().unwrap_or("none")));
                return;
            }
            let kind = node::kind_of(&tool.makes, Some(id));
            if kind != now.kind && !(now.kind == TEXT && now.tool.is_none()) {
                self.told.error(path, format!("{id} makes {kind}: {name} is {}; put it on a new node that mentions <{name}>", now.kind));
                return;
            }
            // A new tool: its block as written, over its defaults.
            let Some(asked) = self.checked_block(name, id, &tool, block) else { return };
            let empty = nodes::block(&Map::new(), Some(&tool));
            let mut merged = empty.clone();
            merged.extend(asked);
            ask.tool = Some(id.to_owned());
            ask.fields.extend(writes(&Map::new(), &empty, &merged, &tool));
            self.change(&format!("{name}.tool"), json!(now.tool), json!(id));
            changed.push(id.to_owned());
            return;
        }
        // The same tool: field by field.
        let Some(asked) = self.checked_block(name, id, &tool, block) else { return };
        let (then, held) = (nodes::block(&was.fields, Some(&tool)), nodes::block(&now.fields, Some(&tool)));
        let mut merged = held.clone();
        for (key, mine) in asked {
            let field = tool.fields.iter().find(|field| field.field == key);
            let at = format!("{name}.{key}");
            let base = then.get(&key).cloned().unwrap_or(Value::Null);
            let theirs = held.get(&key).cloned().unwrap_or(Value::Null);
            self.kept(&at, &mine, &base, &theirs, name, Some(&key), rev);
            let text = Text { id: &now.id, heads, at: key.clone() };
            let value = self.merge(&at, &mine, &base, &theirs, field, &text, rev, name, &key);
            merged.insert(key, value);
        }
        for (key, value) in &merged {
            let before = held.get(key).cloned().unwrap_or(Value::Null);
            for (at, before, after) in leaves(key, &before, value) {
                self.change(&format!("{name}.{at}"), before, after);
                changed.push(at);
            }
        }
        ask.fields.extend(writes(&now.fields, &held, &merged, &tool));
    }

    /// Each key of `block` known to `tool` (or held by its box), its value checked and fixed where it can be.
    fn checked_block(&mut self, name: &str, id: &str, tool: &Tool, block: &Map<String, Value>) -> Option<Map<String, Value>> {
        let errors = self.told.errors.len();
        let held = self.written.get(name).map(|boxed| nodes::block(&nodes::view(boxed, self.lock, self.folder).fields, Some(tool))).unwrap_or_default();
        let tags = self.lock.reserved.get(&format::plain_tool(id, self.folder)).cloned().unwrap_or_default();
        let slots = node::model_slots(tool);
        let mut checked = Map::new();
        for (key, value) in block {
            let path = format!("{name}.{key}");
            if let Some(field) = tool.fields.iter().find(|field| field.field == *key) {
                if let Some(value) = self.value(&path, field, value, &tags) {
                    checked.insert(key.clone(), value);
                }
            } else if slots.iter().any(|(slot, _)| slot == key) || (held.contains_key(key) && *key != node::LORAS) {
                match value {
                    Value::String(_) | Value::Null => drop(checked.insert(key.clone(), value.clone())),
                    Value::Object(slot) if slot.get("ref").is_some_and(Value::is_string) => drop(checked.insert(key.clone(), value.clone())),
                    other if held.contains_key(key) => drop(checked.insert(key.clone(), other.clone())),
                    other => self.told.error(path, format!("a model, [org/]name@release, not {other}")),
                }
            } else if key == node::LORAS {
                match lora_list(value) {
                    Ok(list) => drop(checked.insert(key.clone(), list)),
                    Err(why) => self.told.error(path, why),
                }
            } else {
                let known = tool.fields.iter().map(|field| field.field.as_str()).chain(slots.iter().map(|(slot, _)| slot.as_str())).chain([node::LORAS]);
                let fix = node::near(key, known).map_or_else(String::new, |near| format!(". Did you mean {near}?"));
                self.told.error(path, format!("{id} has no field {key}{fix}"));
            }
        }
        (self.told.errors.len() == errors).then_some(checked)
    }

    /// `value` of `field` as the file writes it, checked: out of its range clamped, said; what can't be one of it refused.
    fn value(&mut self, path: &str, field: &ToolField, value: &Value, tags: &[String]) -> Option<Value> {
        if value.is_null() {
            return Some(Value::Null);
        }
        let not = |what: &str| format!("{what}, not {value}");
        match field.ty {
            FieldType::Words | FieldType::Prompt => match value {
                Value::String(text) => {
                    if field.ty == FieldType::Prompt {
                        self.mentions(path, text, tags);
                    }
                    Some(value.clone())
                }
                _ => {
                    self.told.error(path, not("words"));
                    None
                }
            },
            FieldType::Toggle => match value {
                Value::Bool(_) => Some(value.clone()),
                _ => {
                    self.told.error(path, not("true or false"));
                    None
                }
            },
            FieldType::Number => {
                let Some(mut number) = value.as_f64() else {
                    self.told.error(path, not("a number"));
                    return None;
                };
                let unit = field.unit.as_ref().map_or_else(String::new, |unit| format!(" {unit}"));
                let shown = |n: f64| if n.fract() == 0.0 { format!("{n:.0}") } else { n.to_string() };
                if field.integer && number.fract() != 0.0 {
                    self.told.warn(path, format!("{} is whole, so {}", shown(number), shown(number.round())));
                    number = number.round();
                }
                if let Some(max) = field.max.filter(|max| number > *max) {
                    self.told.warn(path, format!("{} is over {}{unit}, so {}", shown(number), shown(max), shown(max)));
                    number = max;
                }
                if let Some(min) = field.min.filter(|min| number < *min) {
                    self.told.warn(path, format!("{} is under {}{unit}, so {}", shown(number), shown(min), shown(min)));
                    number = min;
                }
                Some(if number.fract() == 0.0 && number.abs() < 9e15 { json!(number as i64) } else { json!(number) })
            }
            FieldType::Choice => {
                let options = field.options.clone().unwrap_or_default();
                if options.iter().any(|option| canonical(option) == canonical(value)) {
                    return options.into_iter().find(|option| canonical(option) == canonical(value));
                }
                let words: Vec<String> = options.iter().map(|option| option.as_str().map_or_else(|| option.to_string(), str::to_owned)).collect();
                let fix = value.as_str().and_then(|said| node::near(said, words.iter().map(String::as_str))).map_or_else(String::new, |near| format!(". Did you mean {near}?"));
                self.told.error(path, format!("{}{fix}", not(&or_list(&words))));
                None
            }
            FieldType::Shape => {
                let ratio = value.as_str().filter(|said| said.split_once(':').is_some_and(|(w, h)| w.parse::<f64>().is_ok_and(|w| w > 0.0) && h.parse::<f64>().is_ok_and(|h| h > 0.0)));
                match (ratio, &field.ratios) {
                    (Some(said), Some(ratios)) if !ratios.iter().any(|ratio| canonical(ratio) == canonical(value)) => {
                        let words: Vec<String> = ratios.iter().map(|ratio| ratio.as_str().unwrap_or_default().to_owned()).collect();
                        self.told.warn(path, format!("{said} isn't a ratio it lists ({}): a render clamps it to one, and says so", or_list(&words)));
                        Some(value.clone())
                    }
                    (Some(_), _) => Some(value.clone()),
                    (None, _) => {
                        self.told.error(path, not("a ratio, width:height, quoted: \"9:16\""));
                        None
                    }
                }
            }
            FieldType::Slot => {
                let one = |planning: &mut Self, at: &str, item: &Value| match item.as_str().filter(|said| said.starts_with('<') && said.ends_with('>')) {
                    Some(said) => {
                        planning.mentions(at, said, tags);
                        true
                    }
                    None => {
                        planning.told.error(at, format!("a mention of a{} {} node, <name> or <name.N>, not {item}", if matches!(field.accept.as_deref(), Some("image" | "audio")) { "n" } else { "" }, field.accept.as_deref().unwrap_or("media")));
                        false
                    }
                };
                match (field.multiple, value) {
                    (true, Value::Array(items)) => items.iter().enumerate().all(|(at, item)| one(self, &format!("{path}.{at}"), item)).then(|| value.clone()),
                    (true, item) => one(self, path, item).then(|| json!([item])),
                    (false, item) => one(self, path, item).then(|| item.clone()),
                }
            }
            FieldType::Group => {
                let row = |planning: &mut Self, at: &str, row: &Value| -> Option<Value> {
                    let Value::Object(members) = row else {
                        planning.told.error(at, format!("a mapping of {}, not {row}", or_list(&field.members.iter().map(|member| member.field.clone()).collect::<Vec<_>>())));
                        return None;
                    };
                    let mut checked = Map::new();
                    let mut good = true;
                    for (key, value) in members {
                        let at = format!("{at}.{key}");
                        match field.members.iter().find(|member| member.field == *key) {
                            Some(member) => match planning.value(&at, member, value, tags) {
                                Some(value) => drop(checked.insert(key.clone(), value)),
                                None => good = false,
                            },
                            None => {
                                let fix = node::near(key, field.members.iter().map(|member| member.field.as_str())).map_or_else(String::new, |near| format!(". Did you mean {near}?"));
                                planning.told.error(at, format!("{} has no field {key}{fix}", field.field));
                                good = false;
                            }
                        }
                    }
                    good.then_some(Value::Object(checked))
                };
                match (field.list, value) {
                    (true, Value::Array(rows)) => {
                        if let Some(min) = field.min_items.filter(|min| (rows.len() as u64) < *min) {
                            self.told.error(path, format!("at least {min}, not {}", rows.len()));
                            return None;
                        }
                        let mut rows = rows.clone();
                        if let Some(max) = field.max_items.filter(|max| rows.len() as u64 > *max) {
                            self.told.warn(path, format!("at most {max}, so the first {max} of {}", rows.len()));
                            rows.truncate(max as usize);
                        }
                        let checked: Vec<Option<Value>> = rows.iter().enumerate().map(|(at, item)| row(self, &format!("{path}.{at}"), item)).collect();
                        checked.into_iter().collect::<Option<Vec<_>>>().map(Value::Array)
                    }
                    (true, other) => {
                        self.told.error(path, format!("a list, not {other}"));
                        None
                    }
                    (false, value) => row(self, path, value),
                }
            }
            FieldType::Seed => match value.as_u64() {
                Some(_) => Some(value.clone()),
                None => {
                    self.told.error(path, not("a whole number, or null for a new one each render"));
                    None
                }
            },
            FieldType::Timeline => {
                for mention in graph::mentions_in(value) {
                    self.mention(path, &mention, tags);
                }
                Some(value.clone())
            }
        }
    }

    /// Each `<name>` in `text` that names nothing: said, and its render refused, free.
    fn mentions(&mut self, path: &str, text: &str, tags: &[String]) {
        for mention in graph::mentions(text) {
            self.mention(path, mention, tags);
        }
    }

    fn mention(&mut self, path: &str, mention: &str, tags: &[String]) {
        let Some(read) = graph::mention(mention) else { return };
        let key = name_key(read.name);
        if key == graph::SELF || self.names.contains(&key) || tags.iter().any(|tag| name_key(tag) == key) {
            return;
        }
        let known = self.written.boxes.iter().map(|held| held.name.as_str());
        let fix = node::near(read.name, known).map_or_else(String::new, |near| format!(": did you mean <{near}>?"));
        self.told.warn(path, format!("<{mention}> names nothing in the project{fix}; its render is refused, free"));
    }

    /// A value both sides may have changed: the file's, the rev's and the project's now, merged. Where they conflict, said, the
    /// project's stands, or with `--force-conflicts` the file's.
    #[allow(clippy::too_many_arguments)]
    fn merge(&mut self, path: &str, mine: &Value, base: &Value, theirs: &Value, field: Option<&ToolField>, text: &Text, rev: &str, name: &str, top: &str) -> Value {
        let same = |a: &Value, b: &Value| canonical(a) == canonical(b);
        if same(mine, base) {
            return theirs.clone();
        }
        if same(theirs, base) || same(mine, theirs) {
            return mine.clone();
        }
        let ty = field.map(|field| field.ty);
        match (mine, base, theirs) {
            (Value::String(mine), Value::String(base), Value::String(theirs)) if matches!(ty, Some(FieldType::Words | FieldType::Prompt)) => {
                json!(self.words(path, text.id, &text.at, text.heads, rev, name, base, mine, theirs).unwrap_or_else(|| theirs.clone()))
            }
            (Value::Object(mine), Value::Object(base), Value::Object(theirs)) => {
                let mut merged = theirs.clone();
                for (key, value) in mine {
                    let member = field.and_then(|field| field.members.iter().find(|member| member.field == *key));
                    let at = Text { id: text.id, heads: text.heads, at: format!("{}.{key}", text.at) };
                    let value = self.merge(&format!("{path}.{key}"), value, base.get(key).unwrap_or(&Value::Null), theirs.get(key).unwrap_or(&Value::Null), member, &at, rev, name, top);
                    merged.insert(key.clone(), value);
                }
                Value::Object(merged)
            }
            (Value::Array(mine), Value::Array(base), Value::Array(theirs)) => {
                // Lists of groups match by element: a row added meanwhile stays.
                let mut merged = vec![];
                for at in 0..mine.len().max(base.len()).max(theirs.len()) {
                    let row = Text { id: text.id, heads: text.heads, at: format!("{}.{at}", text.at) };
                    let at_path = format!("{path}.{at}");
                    match (mine.get(at), base.get(at), theirs.get(at)) {
                        (Some(mine), Some(base), Some(theirs)) => merged.push(self.merge(&at_path, mine, base, theirs, field, &row, rev, name, top)),
                        (None, Some(base), Some(theirs)) if !same(base, theirs) => {
                            self.conflict(&at_path, Value::Null, theirs.clone(), name, Some(top), rev);
                            if !self.force { merged.push(theirs.clone()); }
                        }
                        (Some(mine), Some(base), None) if !same(mine, base) => {
                            self.conflict(&at_path, mine.clone(), Value::Null, name, Some(top), rev);
                            if self.force { merged.push(mine.clone()); }
                        }
                        (Some(mine), None, Some(theirs)) if !same(mine, theirs) => {
                            self.conflict(&at_path, mine.clone(), theirs.clone(), name, Some(top), rev);
                            merged.push(if self.force { mine.clone() } else { theirs.clone() });
                        }
                        (Some(mine), None, _) => merged.push(mine.clone()),
                        (None, None, Some(theirs)) => merged.push(theirs.clone()),
                        _ => {}
                    }
                }
                Value::Array(merged)
            }
            _ => {
                self.conflict(path, mine.clone(), theirs.clone(), name, Some(top), rev);
                if self.force { mine.clone() } else { theirs.clone() }
            }
        }
    }

    /// One value of the node's own, three ways. None: a conflict, said.
    #[allow(clippy::too_many_arguments)]
    fn scalar(&mut self, path: &str, mine: &Value, base: &Value, theirs: &Value, rev: &str, name: &str, field: Option<&str>) -> Option<Value> {
        let same = |a: &Value, b: &Value| canonical(a) == canonical(b);
        if same(mine, base) {
            return Some(theirs.clone());
        }
        if same(theirs, base) || same(mine, theirs) {
            return Some(mine.clone());
        }
        self.conflict(path, mine.clone(), theirs.clone(), name, field, rev);
        self.force.then(|| mine.clone())
    }

    /// Words both sides may have edited: merged by character through the document's text when the edits don't overlap, else a conflict.
    #[allow(clippy::too_many_arguments)]
    fn words(&mut self, path: &str, id: &str, at: &str, heads: &[String], rev: &str, name: &str, base: &str, mine: &str, theirs: &str) -> Option<String> {
        let same = |a: &str, b: &str| a.trim_end() == b.trim_end();
        if same(mine, base) {
            return Some(theirs.to_owned());
        }
        if same(theirs, base) || same(mine, theirs) {
            return Some(mine.to_owned());
        }
        let (base, mine, theirs) = (base.trim_end(), mine.trim_end(), theirs.trim_end());
        let (ours, others) = (hunks(base, mine), hunks(base, theirs));
        if ours.iter().any(|one| others.iter().any(|other| one != other && overlap(one, other))) {
            self.conflict(path, json!(mine), json!(theirs), name, Some(at.split('.').next().unwrap_or(at)), rev);
            return self.force.then(|| mine.to_owned());
        }
        // From the last to the first, so each one's place is as the rev had it.
        let splices: Vec<Splice> = ours.iter().rev().filter(|one| !others.contains(one))
            .map(|one| Splice { at: one.start as u32, delete: (one.end - one.start) as u32, insert: one.insert.clone() }).collect();
        match self.document.typed(id, at, heads, &splices) {
            Ok(words) => Some(words),
            Err(why) => {
                self.told.error(path, why.message);
                None
            }
        }
    }

    fn change(&mut self, path: &str, before: Value, after: Value) {
        self.changes.push(Changed { path: path.to_owned(), before, after });
    }

    /// Both changed it: with `--force-conflicts` the file's stands, else it's said and nothing is written.
    fn conflict(&mut self, path: &str, yours: Value, theirs: Value, name: &str, field: Option<&str>, rev: &str) {
        let by = self.who(name, field);
        self.conflicts.push(Conflict { path: path.to_owned(), yours, theirs, by, rev: rev.to_owned() });
    }

    /// Who last changed `field` of `name`, as the history says: `the page`.
    fn who(&self, name: &str, field: Option<&str>) -> String {
        history::last_by(self.folder, name, field).map_or_else(|| "the project".to_owned(), |by| match by.via.as_str() {
            "page" => "the page".to_owned(),
            "cli" => "the CLI".to_owned(),
            _ => "the API".to_owned(),
        })
    }

    /// Each value at `path` the project changed since `rev` and the file left as it was then: it stands, and that's said, as nothing
    /// changes silently. A value the file changed too is the merge's to settle.
    #[allow(clippy::too_many_arguments)]
    fn kept(&mut self, path: &str, mine: &Value, base: &Value, theirs: &Value, name: &str, field: Option<&str>, rev: &str) {
        for (at, was, now) in leaves(path, base, theirs) {
            let left = pick(mine, &at[path.len()..]).is_none_or(|mine| canonical(mine) == canonical(&was));
            if left {
                let message = format!("{}'s change since {rev} stands ({})", self.who(name, field), briefly(&now));
                self.kept.push(Said::new(at, message));
            }
        }
    }
}

/// Each value that differs between `before` and `after`, as deep as both are groups or lists, by its path from `at`.
fn leaves(at: &str, before: &Value, after: &Value) -> Vec<(String, Value, Value)> {
    if canonical(before) == canonical(after) {
        return vec![];
    }
    match (before, after) {
        (Value::Object(was), Value::Object(now)) => {
            let keys: BTreeSet<&String> = was.keys().chain(now.keys()).collect();
            keys.into_iter().flat_map(|key| leaves(&format!("{at}.{key}"), was.get(key).unwrap_or(&Value::Null), now.get(key).unwrap_or(&Value::Null))).collect()
        }
        (Value::Array(was), Value::Array(now)) if was.len() == now.len() => {
            was.iter().zip(now).enumerate().flat_map(|(index, (was, now))| leaves(&format!("{at}.{index}"), was, now)).collect()
        }
        _ => vec![(at.to_owned(), before.clone(), after.clone())],
    }
}

/// What `value` holds at `path` (`.beats.0.duration`; empty, itself).
fn pick<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').filter(|step| !step.is_empty()).try_fold(value, |at, step| match at {
        Value::Array(items) => items.get(step.parse::<usize>().ok()?),
        Value::Object(map) => map.get(step),
        _ => None,
    })
}

/// A value in a line: words as they are, cut at 60 characters; anything else as JSON.
fn briefly(value: &Value) -> String {
    let said = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match said.chars().count() > 60 {
        true => format!("{}…", said.chars().take(59).collect::<String>()),
        false => said,
    }
}

/// Where a box's words are, for a merge through its text.
struct Text<'a> {
    id: &'a str,
    heads: &'a [String],
    at: String,
}

/// What a box's fields become for `merged`, a block over `held` (the block of the box's `fields`): each field that changed as the box
/// keeps it, a default it didn't hold left unwritten, model slots in its `models` and `loras` as its `lora`.
fn writes(fields: &Map<String, Value>, held: &Map<String, Value>, merged: &Map<String, Value>, tool: &Tool) -> Map<String, Value> {
    let mut written = Map::new();
    let mut models = fields.get("models").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut models_changed = false;
    let slots = node::model_slots(tool);
    for (key, value) in merged {
        if held.get(key).is_some_and(|held| canonical(held) == canonical(value)) {
            continue;
        }
        if let Some((_, default)) = slots.iter().find(|(slot, _)| slot == key) {
            models_changed = true;
            // One key a slot: what named it by a child call's path goes.
            models.retain(|path, _| !path.ends_with(&format!(".models.{key}")));
            match value {
                Value::Null => drop(models.remove(key)),
                Value::String(reference) if default.as_deref() == Some(reference.as_str()) => drop(models.remove(key)),
                Value::String(reference) => drop(models.insert(key.clone(), json!({"ref": reference}))),
                Value::Object(entry) => {
                    let mut entry = entry.clone();
                    if let Some(lora) = entry.remove(node::LORAS) {
                        entry.insert("lora".to_owned(), lora);
                    }
                    models.insert(key.clone(), Value::Object(entry));
                }
                _ => {}
            }
        } else if key == node::LORAS {
            written.insert("lora".to_owned(), if value.as_array().is_none_or(Vec::is_empty) { Value::Null } else { value.clone() });
        } else {
            written.insert(key.clone(), unfilled(fields.get(key), held.get(key), value).unwrap_or(Value::Null));
        }
    }
    if models_changed {
        written.insert("models".to_owned(), if models.is_empty() { Value::Null } else { Value::Object(models) });
    }
    written
}

/// `merged` as a box keeps it: what it held where nothing changed, and a default it didn't hold left out. None: leave it out.
fn unfilled(held: Option<&Value>, filled: Option<&Value>, merged: &Value) -> Option<Value> {
    if filled.is_some_and(|filled| canonical(filled) == canonical(merged)) {
        return held.cloned();
    }
    match (merged, filled) {
        (Value::Object(merged), Some(Value::Object(filled))) => {
            let held = held.and_then(Value::as_object);
            Some(Value::Object(merged.iter().filter_map(|(key, value)| Some((key.clone(), unfilled(held.and_then(|held| held.get(key)), filled.get(key), value)?))).collect()))
        }
        (Value::Array(merged), Some(Value::Array(filled))) => {
            let held = held.and_then(Value::as_array);
            Some(Value::Array(merged.iter().enumerate().map(|(at, value)| {
                unfilled(held.and_then(|held| held.get(at)), filled.get(at), value).unwrap_or_else(|| filled.get(at).cloned().unwrap_or(Value::Null))
            }).collect()))
        }
        _ => Some(merged.clone()),
    }
}

/// `loras` as the file writes them: each `{ref, strength}`.
fn lora_list(value: &Value) -> Result<Value, String> {
    match value {
        Value::Null => Ok(json!([])),
        Value::Array(items) => {
            for item in items {
                let good = item.as_object().is_some_and(|entry| entry.get("ref").is_some_and(Value::is_string)
                    && entry.get("strength").is_none_or(Value::is_number) && entry.keys().all(|key| key == "ref" || key == "strength"));
                if !good {
                    return Err(format!("each LoRA is {{ref, strength}}, not {item}"));
                }
            }
            Ok(value.clone())
        }
        other => Err(format!("a list of {{ref, strength}}, not {other}")),
    }
}

fn or_list(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

fn empty_ask() -> NewAsset {
    NewAsset { tool: String::new(), at: None, name: None, exact: true, fields: Map::new(), kind: None, moves: Default::default(), placed: Default::default() }
}

/// One edit of some words: `start..end` of the old, by character, as `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hunk {
    start: usize,
    end: usize,
    insert: String,
}

/// Whether two edits of the same words touch: ranges that overlap, or an insertion at or in the other's range.
fn overlap(a: &Hunk, b: &Hunk) -> bool {
    let inside = |point: usize, other: &Hunk| other.start <= point && point <= other.end;
    (a.start < b.end && b.start < a.end) || (a.start == a.end && inside(a.start, b)) || (b.start == b.end && inside(b.start, a))
}

/// The edits from `old` to `new`, by character: their common start and end aside, a longest common subsequence of the rest, else one edit.
fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    if a.is_empty() && b.is_empty() {
        return vec![];
    }
    let one = || vec![Hunk { start, end: start + a.len(), insert: b.iter().collect() }];
    if a.is_empty() || b.is_empty() || a.len() * b.len() > 4_000_000 {
        return one();
    }
    // lengths[i][j]: the longest common subsequence of a[i..] and b[j..].
    let width = b.len() + 1;
    let mut lengths = vec![0u16; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lengths[i * width + j] = if a[i] == b[j] { lengths[(i + 1) * width + j + 1].saturating_add(1) } else { lengths[(i + 1) * width + j].max(lengths[i * width + j + 1]) };
        }
    }
    let (mut i, mut j, mut found, mut open): (usize, usize, Vec<Hunk>, Option<Hunk>) = (0, 0, vec![], None);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            found.extend(open.take());
            i += 1;
            j += 1;
            continue;
        }
        let hunk = open.get_or_insert_with(|| Hunk { start: start + i, end: start + i, insert: String::new() });
        if j < b.len() && (i == a.len() || lengths[i * width + j + 1] >= lengths[(i + 1) * width + j]) {
            hunk.insert.push(b[j]);
            j += 1;
        } else {
            i += 1;
            hunk.end = start + i;
        }
    }
    found.extend(open);
    found
}

/// What a rev of the file names: the heads it stands for and the boxes then, or why not.
fn bases(document: &Document, wrote: &[Wrote], told: &mut Told) -> HashMap<String, (Vec<String>, Written)> {
    let mut bases = HashMap::new();
    for wrote in wrote {
        let Some(rev) = wrote.rev.as_ref().filter(|rev| !bases.contains_key(*rev)) else { continue };
        match document.heads_of(rev).map(|heads| document.written_at(&heads).map(|then| (heads, then))) {
            Some(Ok(base)) => drop(bases.insert(rev.clone(), base)),
            Some(Err(why)) => told.error(wrote.name.as_str(), format!("its rev {rev} can't be read: {}", why.message)),
            None => told.error(wrote.name.as_str(), format!("rev {rev} isn't one of this project's: run `cozy get {}` again", wrote.name)),
        }
    }
    bases
}

/// `POST /v1/projects/{id}/apply`: `nodes` merged into the project as one edit, or with `dry_run` what it would do.
pub(super) async fn apply(State(api): State<Arc<Api>>, Path(id): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Json<Applied>, Refusal> {
    let asked: Apply = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let document = folder.document().map_err(Refusal)?.ok_or_else(|| Refusal(Error::new(Kind::Validation, "project_crdt", "the canonical project document is missing")))?;
    let written = folder.written().map_err(Refusal)?;
    let lock = folder.lock().map_err(Refusal)?;
    let mut told = Told::default();
    let wrote: Vec<Wrote> = asked.nodes.iter().filter_map(|(name, value)| wrote(name, value, &mut told)).collect();
    let bases = bases(&document, &wrote, &mut told);
    // Every tool the file, the boxes now and at their revs name.
    let mut named: Vec<String> = wrote.iter().filter_map(|wrote| Some(wrote.tool.as_ref()?.0.clone())).collect();
    for boxes in std::iter::once(&written).chain(bases.values().map(|(_, then)| then)) {
        named.extend(wrote.iter().filter_map(|wrote| boxes.get(&wrote.name)?.get("tool")?.as_str().map(str::to_owned)));
    }
    named.retain(|tool| !tool.trim().is_empty());
    let tools = nodes::tools(&api, named, &folder.dir).await;
    // What each file take is, from its bytes; kept for the project once the apply will write.
    let sources: BTreeSet<String> = wrote.iter().flat_map(|wrote| wrote.files.iter().cloned()).collect();
    let mut kept: HashMap<String, folder::Kept> = HashMap::new();
    let mut fields: HashMap<String, &'static str> = HashMap::new();
    for source in &sources {
        let read = match asked.dry_run {
            true => held_field(folder, source),
            false => super::keep_file(folder, source).await.map(|file| {
                let field = file.field;
                kept.insert(source.clone(), file);
                field
            }).map_err(|Refusal(why)| why),
        };
        match read {
            Ok(field) => drop(fields.insert(source.clone(), field)),
            Err(why) => told.error(format!("takes.{source}"), why.message),
        }
    }
    let names = written.boxes.iter().map(|held| name_key(&held.name)).chain(asked.nodes.keys().map(|name| name_key(name))).collect();
    let mut planning = Planning { folder, lock: &lock, written: &written, document: &document, tools: &tools, bases: &bases, names, fields: &fields, force: asked.force_conflicts, told, changes: vec![], conflicts: vec![], kept: vec![] };
    let planned: Vec<Planned> = wrote.iter().filter_map(|wrote| planning.plan(wrote)).collect();
    let Planning { told, changes, conflicts, kept: stood, .. } = planning;
    let mut answer = Applied {
        applied: false,
        nodes: planned.iter().map(|planned| Outcome { name: planned.name.clone(), did: planned.did.to_owned(), changed: planned.changed.clone() }).collect(),
        changes,
        conflicts,
        errors: told.errors,
        warnings: told.warnings,
        notes: told.notes,
        kept: stood,
        merged: None,
        next: vec![],
    };
    let refused = !answer.errors.is_empty() || (!answer.conflicts.is_empty() && !asked.force_conflicts);
    let writes: Vec<&Planned> = planned.iter().filter(|planned| planned.write.is_some()).collect();
    if refused || asked.dry_run {
        return Ok(Json(answer));
    }
    // What each write reads first, outside the transaction, as the page's own routes read it.
    let mut readied = vec![];
    for planned in &writes {
        let ready = match planned.write.as_ref().expect("a write") {
            // A file's node is an import's element: nothing to read first.
            Write::New { element: true, .. } => Ready::Element,
            Write::New { ask, .. } => {
                let mut ask = ask.clone();
                Ready::New(Box::new((assets::ready_new(&api, folder, &mut ask).await.map_err(Refusal)?, ask)))
            }
            Write::Edit { ask, .. } if !ask.fields.is_empty() || ask.tool.is_some() => Ready::Edit(Some(assets::ready_patch(&api, folder, &planned.name, ask).await.map_err(Refusal)?)),
            Write::Edit { .. } => Ready::Edit(None),
        };
        readied.push(ready);
    }
    let approving = writes.iter().any(|planned| matches!(&planned.write, Some(Write::Edit { approved: Some(_), .. })));
    let made: Vec<String> = writes.iter().filter(|planned| planned.did == "created").map(|planned| planned.name.clone()).collect();
    folder.act(|| {
        // Worked out on the boxes as they were read: one changed since is applied again, never over.
        for planned in &planned {
            let now = folder.reading_boxes(|written| written.get(&planned.name).cloned())?;
            if now.as_ref().map(|held| (&held.id, &held.fields)) != planned.on.as_ref().map(|held| (&held.id, &held.fields)) {
                return Err(Error::new(Kind::Conflict, "apply_raced", format!("{} changed while this applied", planned.name)).next("cozy apply again"));
            }
        }
        for (planned, ready) in writes.iter().zip(&readied) {
            write(&project, folder, &planned.name, planned.write.as_ref().expect("a write"), ready, &kept)?;
        }
        Ok(())
    }).map_err(Refusal)?;
    answer.applied = true;
    for reading in project.walker.lock().expect("walker lock").readings.values_mut() {
        made.iter().for_each(|name| reading.moved(name));
    }
    if !kept.is_empty() {
        super::measure_soon(&api, &project);
    }
    if approving {
        answer.warnings.extend(super::settle(&api, &project).await.into_iter().map(|said| Said::new("", said)));
    }
    let names: Vec<String> = asked.nodes.keys().cloned().collect();
    answer.merged = Some(nodes::nodes_of(&api, &project, &names, Shown::Some).await.map_err(Refusal)?);
    answer.next = next(folder, &planned).map_err(Refusal)?;
    Ok(Json(answer))
}

/// What a write read first.
enum Ready {
    Element,
    New(Box<(assets::NewReady, NewAsset)>),
    Edit(Option<assets::PatchReady>),
}

/// One node's write, in the apply's transaction.
fn write(project: &Arc<Project>, folder: &Folder, name: &str, write: &Write, ready: &Ready, kept: &HashMap<String, folder::Kept>) -> Result<(), Error> {
    let file = |source: &String| kept.get(source).ok_or_else(|| Error::new(Kind::Internal, "apply_file", format!("{source} wasn't kept")));
    let (files, approved) = match (write, ready) {
        (Write::New { ask, files, approved, .. }, Ready::Element) => {
            let first = files.first().ok_or_else(|| Error::new(Kind::Internal, "apply_file", format!("{name} has no file")))?;
            let description = ask.fields.get("description").and_then(Value::as_str);
            super::element(project, &project.id, name, None, description, copied(file(first)?), true, Default::default(), &Default::default()).map_err(|Refusal(why)| why)?;
            (files[1..].to_vec(), approved.map(Some))
        }
        (Write::New { files, approved, .. }, Ready::New(readied)) => {
            assets::made(folder, &project.id, &readied.1, &readied.0)?;
            (files.clone(), approved.map(Some))
        }
        (Write::Edit { ask, approved, files }, Ready::Edit(ready)) => {
            if let Some(ready) = ready {
                assets::patched(folder, name, ask, ready)?;
            }
            (files.clone(), *approved)
        }
        _ => unreachable!("each write is readied as itself"),
    };
    for source in &files {
        let doc = Doc::load(folder)?;
        let i = doc.find(name).ok_or_else(|| Error::new(Kind::NotFound, "asset_unknown", format!("the project has nothing named {name}")))?;
        assets::joined(folder, &doc, i, file(source)?)?;
    }
    match approved {
        None => {}
        Some(None) => folder.edit(|written| written.unapproved(name))?,
        Some(Some(number)) => {
            let lock = folder.lock()?;
            let version = lock.asset(name).and_then(|held| held.version(number)).ok_or_else(|| Error::new(Kind::Usage, "version_unknown", format!("{name} has no take {number}")))?;
            let sha256 = version.main().cloned().ok_or_else(|| Error::new(Kind::Usage, "version_unknown", format!("{name}'s take {number} has no file")))?;
            folder.edit(|written| written.selected(name, number, &sha256))?;
        }
    }
    Ok(())
}

fn copied(kept: &folder::Kept) -> folder::Kept {
    folder::Kept { file: kept.file.clone(), field: kept.field, already: kept.already }
}

/// What a held file's bytes are, read without keeping it: what a dry run says of a file take.
fn held_field(folder: &Folder, source: &str) -> Result<&'static str, Error> {
    let digest: api::ids::Digest = source.parse().map_err(|why: &str| Error::new(Kind::Usage, "file_name_invalid", format!("{source}: {why}")))?;
    if let Some(kept) = folder.own(&digest.to_string()) {
        return Ok(kept.field);
    }
    let at = crate::runs::files::held_for(&folder.dir, &digest).ok_or_else(|| Error::new(Kind::NotFound, "file_not_held", format!("Calcifer holds no file {source}")))?;
    Folder::field_of(&at).map_err(|why| Error::new(Kind::Validation, "import_failed", why))
}

/// `cozy render` of what reads what this changed, and nothing reads in turn: what brings it all up to date.
fn next(folder: &Folder, planned: &[Planned]) -> Result<Vec<String>, Error> {
    let written = folder.written()?;
    let changed: BTreeSet<String> = planned.iter().filter(|planned| planned.changed.iter().any(|said| !said.starts_with("approved") && said != "unapproved" && !said.starts_with("take ")))
        .map(|planned| name_key(&planned.name)).collect();
    if changed.is_empty() {
        return Ok(vec![]);
    }
    let reads = |held: &Boxed| -> BTreeSet<String> {
        held.fields.iter().flat_map(|(_, value)| graph::mentions_in(value)).filter_map(|mention| graph::mention(&mention).filter(|read| read.version.is_none()).map(|read| name_key(read.name))).collect()
    };
    let mut reached = changed.clone();
    loop {
        let more: Vec<String> = written.boxes.iter().filter(|held| !reached.contains(&name_key(&held.name)) && !reads(held).is_disjoint(&reached)).map(|held| name_key(&held.name)).collect();
        if more.is_empty() {
            break;
        }
        reached.extend(more);
    }
    let renders = |held: &Boxed| held.get("tool").and_then(Value::as_str).is_some_and(|tool| !tool.trim().is_empty());
    let within: Vec<&Boxed> = written.boxes.iter().filter(|held| reached.contains(&name_key(&held.name)) && renders(held)).collect();
    let sinks: Vec<&str> = within.iter().filter(|held| !within.iter().any(|other| other.name != held.name && reads(other).contains(&name_key(&held.name)))).map(|held| held.name.as_str()).collect();
    Ok(if sinks.is_empty() { vec![] } else { vec![format!("cozy render {}", sinks.join(" "))] })
}
