//! A project's history (Paul, 11-06): every edit of it, by the page, the CLI or an import, in plain words, and what puts it back. Undo
//! belongs to the project, not to a tab: an agent's CLI edit is undone from the page, and the page's from the CLI. It's the op log of
//! `project.db` (cl-803): each entry a row of `ops`, written in the transaction of the edit it records, the last 200 kept.
//!
//! What an edit changed is read off its writes as they're made: a box added or gone (its fields), its fields (each as it was and is), a
//! canvas row, a take discarded, brought back or joined, a rename. A step back puts each back only while it's as the edit left it: a later
//! edit of the same thing refuses it, naming that edit. A render is never undone, and nothing here rents or ends a machine.
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::{Path as FsPath, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, Request, State, rejection::BytesRejection},
    http::{HeaderMap, Method},
    middleware::Next,
    response::Response,
};
use platform::durable;
use protocol::{
    error::{Error, Kind},
    project::{By, Card, History, HistoryEntry, HistoryStep, HistoryStepped, Rename, Undone},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::warn;

use super::{
    Project, canvas, db,
    folder::{Folder, Lock, Made},
    graph::name_key,
    now_ms, project_id,
    written::{Boxed, Written},
};
use crate::api::{Api, Refusal, parse};

/// How many entries a project keeps.
const KEPT: usize = 200;
/// How many `GET` lists with no `limit`.
const LISTED: usize = 50;

/// What a take of an asset is: on its list, set aside, or not on record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Took {
    Absent,
    Listed,
    Discarded,
}

impl Took {
    /// Whether a take that is `self` is as `wanted`: one never listed is as good as discarded, where a step back leaves one joined.
    fn is(self, wanted: Took) -> bool {
        self == wanted || (wanted == Took::Absent && self == Took::Discarded)
    }
}

/// One thing an edit changed, as it was and as the edit left it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Change {
    /// A box added (`before` none), gone (`after` none), or rewritten whole.
    Asset { name: String, before: Option<Boxed>, after: Option<Boxed> },
    /// Fields of a box there before and after, each as it was and is; null is none.
    Fields { name: String, fields: BTreeMap<String, (Value, Value)> },
    Card { name: String, before: Option<Card>, after: Option<Card> },
    Take { name: String, number: u32, noun: String, before: Took, after: Took },
    Renamed { from: String, to: String },
}

impl Change {
    fn name(&self) -> &str {
        match self {
            Change::Asset { name, .. } | Change::Fields { name, .. } | Change::Card { name, .. } | Change::Take { name, .. } => name,
            Change::Renamed { to, .. } => to,
        }
    }

    /// Whether it changed what asset `name` is, as against where its box sits.
    fn touches(&self, name: &str) -> bool {
        let key = name_key(name);
        match self {
            Change::Renamed { from, to } => name_key(from) == key || name_key(to) == key,
            Change::Card { .. } => false,
            other => name_key(other.name()) == key,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Entry {
    id: u64,
    at_ms: i64,
    by: By,
    words: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    edits: Vec<String>,
    changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    undone: Option<Undone>,
}

impl Entry {
    fn listed(&self) -> HistoryEntry {
        let mut names: Vec<String> = vec![];
        for change in &self.changes {
            if !names.iter().any(|held| name_key(held) == name_key(change.name())) {
                names.push(change.name().to_owned());
            }
        }
        HistoryEntry { id: self.id, at_ms: self.at_ms, by: self.by.clone(), words: self.words.clone(), names, edits: self.edits.clone(), undone: self.undone.clone() }
    }

    fn row(&self) -> db::OpRow {
        db::OpRow { id: self.id, at_ms: self.at_ms, by: json(&self.by), words: self.words.clone(), edits: json(&self.edits), changes: json(&self.changes), undone: self.undone.as_ref().map(json) }
    }

    fn of(row: db::OpRow) -> Option<Self> {
        Some(Self {
            id: row.id,
            at_ms: row.at_ms,
            by: serde_json::from_str(&row.by).ok()?,
            words: row.words,
            edits: serde_json::from_str(&row.edits).unwrap_or_default(),
            changes: serde_json::from_str(&row.changes).ok()?,
            undone: row.undone.and_then(|undone| serde_json::from_str(&undone).ok()),
        })
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("a history row serializes")
}

/// One project's history as its rows say, and a turn for undo and redo, one at a time.
struct Book {
    entries: Mutex<Vec<Entry>>,
    stepping: tokio::sync::Mutex<()>,
}

static BOOKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Book>>>> = LazyLock::new(Mutex::default);

/// The project in `dir` is forgotten: its book goes, and is read from project.db if it's opened again.
pub(super) fn let_go(dir: &FsPath) {
    let key = durable::canonical(dir).unwrap_or_else(|_| dir.to_owned());
    BOOKS.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
}

fn book(folder: &Folder) -> Arc<Book> {
    let key = durable::canonical(&folder.dir).unwrap_or_else(|_| folder.dir.clone());
    let mut books = BOOKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(held) = books.get(&key) {
        return held.clone();
    }
    let entries = match folder.ops() {
        Ok(rows) => rows.into_iter().filter_map(Entry::of).collect(),
        Err(error) => {
            warn!("{}: its history can't be read: {}", folder.dir.display(), error.message);
            vec![]
        }
    };
    let held = Arc::new(Book { entries: Mutex::new(entries), stepping: tokio::sync::Mutex::default() });
    books.insert(key, held.clone());
    held
}

/// `changes`, made by `by`, in `entries`: one more entry, or onto what `edits` began, if one did and it stands. The rows that change.
fn add(entries: &mut Vec<Entry>, by: By, edits: Vec<String>, changes: Vec<Change>, lock: Option<&Lock>) -> (Vec<db::OpRow>, Vec<u64>) {
    let joins = entries.iter_mut().rev().find(|entry| entry.undone.is_none() && entry.edits.iter().any(|edit| edits.contains(edit)));
    if let Some(entry) = joins {
        changes.into_iter().for_each(|change| push(&mut entry.changes, change));
        for edit in edits {
            if !entry.edits.contains(&edit) {
                entry.edits.push(edit);
            }
        }
        entry.words = words(&entry.changes, lock);
        let (id, empty) = (entry.id, entry.changes.is_empty());
        if empty {
            entries.retain(|entry| entry.id != id);
            return (vec![], vec![id]);
        }
        return (entries.iter().filter(|entry| entry.id == id).map(Entry::row).collect(), vec![]);
    }
    let mut composed = vec![];
    changes.into_iter().for_each(|change| push(&mut composed, change));
    if composed.is_empty() {
        return (vec![], vec![]);
    }
    let id = entries.last().map_or(0, |entry| entry.id) + 1;
    let words = words(&composed, lock);
    let entry = Entry { id, at_ms: now_ms(), by, words, edits, changes: composed, undone: None };
    let row = entry.row();
    entries.push(entry);
    let over = entries.len().saturating_sub(KEPT);
    let gone = entries.drain(..over).map(|entry| entry.id).collect();
    (vec![row], gone)
}

/// What a step back takes with nothing named: the newest that stands.
fn undo_head(entries: &[Entry]) -> Option<&Entry> {
    entries.iter().rev().find(|entry| entry.undone.is_none())
}

/// What a step forward takes with nothing named: the last undone, unless an edit came after it.
fn redo_head(entries: &[Entry]) -> Option<&Entry> {
    let last = entries.iter().filter(|entry| entry.undone.is_some()).max_by_key(|entry| (entry.undone.as_ref().map_or(0, |undone| undone.at_ms), std::cmp::Reverse(entry.id)))?;
    let undone_at = last.undone.as_ref().map_or(0, |undone| undone.at_ms);
    let since = entries.iter().any(|entry| entry.undone.is_none() && entry.id > last.id && entry.at_ms > undone_at);
    (!since).then_some(last)
}

fn told(entries: &[Entry]) -> History {
    History { entries: vec![], undo: undo_head(entries).map(|entry| entry.id), redo: redo_head(entries).map(|entry| entry.id) }
}

/// `next` after `changes`: onto the last change of the same thing, if it can go on it, else one more.
fn push(changes: &mut Vec<Change>, next: Change) {
    let key = name_key(next.name());
    let last = |wanted: fn(&Change) -> bool| changes.iter().rposition(|change| wanted(change) && name_key(change.name()) == key);
    let at = match &next {
        Change::Fields { .. } | Change::Asset { .. } => last(|change| matches!(change, Change::Asset { .. } | Change::Fields { .. })),
        Change::Card { .. } => last(|change| matches!(change, Change::Card { .. })),
        Change::Take { number, .. } => changes.iter().rposition(|change| matches!(change, Change::Take { name, number: n, .. } if name_key(name) == key && n == number)),
        Change::Renamed { .. } => None,
    };
    let Some(at) = at else {
        if !matches!(&next, Change::Card { before, after, .. } if before == after) && !matches!(&next, Change::Take { before, after, .. } if before == after) {
            changes.push(next);
        }
        return;
    };
    enum Then {
        Keep,
        Drop,
        Push(Change),
    }
    let then = match (&mut changes[at], next) {
        (Change::Fields { fields: held, .. }, Change::Fields { fields, .. }) => {
            for (field, (before, after)) in fields {
                held.entry(field).or_insert((before, Value::Null)).1 = after;
            }
            held.retain(|_, (before, after)| before != after);
            if held.is_empty() { Then::Drop } else { Then::Keep }
        }
        (Change::Asset { before: None, after: Some(added), .. }, Change::Fields { fields, .. }) => {
            fields.into_iter().for_each(|(field, (_, after))| added.set(&field, after));
            Then::Keep
        }
        // Added and gone in one edit: nothing was.
        (Change::Asset { before: None, .. }, Change::Asset { after: None, .. }) => Then::Drop,
        (Change::Card { before, after: held, .. }, Change::Card { after, .. }) => {
            *held = after;
            if before == held { Then::Drop } else { Then::Keep }
        }
        (Change::Take { before, after: held, .. }, Change::Take { after, .. }) => {
            *held = after;
            if before == held { Then::Drop } else { Then::Keep }
        }
        (_, next) => Then::Push(next),
    };
    match then {
        Then::Keep => {}
        Then::Drop => drop(changes.remove(at)),
        Then::Push(next) => changes.push(next),
    }
}

/// What changed between two writings of the project, box by box.
pub(super) fn diffed(before: &Written, after: &Written) -> Vec<Change> {
    // Each box by its name key, once: a lookup per box of each side made every write of a 5,000-box project take seconds.
    let index = |written: &Written| -> std::collections::HashMap<String, usize> {
        let mut index = std::collections::HashMap::new();
        for (at, held) in written.boxes.iter().enumerate() {
            index.entry(name_key(&held.name)).or_insert(at);
        }
        index
    };
    let (was_at, now_at) = (index(before), index(after));
    let mut keys: Vec<String> = before.boxes.iter().chain(&after.boxes).map(|held| name_key(&held.name)).collect();
    let mut seen = std::collections::HashSet::new();
    keys.retain(|key| seen.insert(key.clone()));
    // A box gone remembers where it was, so a step back puts it there.
    let placed = |written: &Written, at: Option<&usize>| at.map(|&at| Boxed { at: Some(at), ..written.boxes[at].clone() });
    keys.into_iter()
        .filter_map(|key| match (was_at.get(&key).map(|&at| &before.boxes[at]), now_at.get(&key).map(|&at| &after.boxes[at])) {
            (None, Some(now)) => Some(Change::Asset { name: now.name.clone(), before: None, after: placed(after, now_at.get(&key)) }),
            (Some(was), None) => Some(Change::Asset { name: was.name.clone(), before: placed(before, was_at.get(&key)), after: None }),
            (Some(was), Some(now)) if was == now => None,
            (Some(was), Some(now)) if was.name != now.name => Some(Change::Asset { name: now.name.clone(), before: Some(was.clone()), after: Some(now.clone()) }),
            (Some(was), Some(now)) => {
                let (was, now) = (was.map(), now.map());
                let mut fields: Vec<&String> = was.keys().chain(now.keys()).collect();
                fields.sort();
                fields.dedup();
                let field = |held: &Map<String, Value>, field: &str| held.get(field).cloned().unwrap_or(Value::Null);
                let fields: BTreeMap<String, (Value, Value)> = fields.into_iter().map(|name| (name.clone(), (field(&was, name), field(&now, name)))).filter(|(_, (was, now))| was != now).collect();
                (!fields.is_empty()).then(|| Change::Fields { name: now_at.get(&key).map(|&at| after.boxes[at].name.clone()).unwrap_or_default(), fields })
            }
            (None, None) => None,
        })
        .collect()
}

/// `names` as a person says them: `a`, `a and b`, `a, b and c`, `a, b and 4 more`.
fn listing(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

/// A tool as its asset's line says it, by its function: `qwen-image-2/generate_turbo@1.0` is `generate_turbo`.
fn tool_word(tool: &Value) -> String {
    let tool = tool.as_str().unwrap_or_default();
    tool.rsplit(['/', '\\']).next().unwrap_or(tool).split('@').next().unwrap_or(tool).to_owned()
}

/// What `changes` did, in a few words: what matters most of it, as the history says it.
fn words(changes: &[Change], lock: Option<&Lock>) -> String {
    if let Some(Change::Renamed { from, to }) = changes.iter().find(|change| matches!(change, Change::Renamed { .. })) {
        return format!("Renamed {from} to {to}");
    }
    let names = |wanted: fn(&Change) -> bool| -> Vec<&str> { changes.iter().filter(|change| wanted(change)).map(Change::name).collect() };
    let gone = names(|change| matches!(change, Change::Asset { after: None, .. }));
    let added = names(|change| matches!(change, Change::Asset { before: None, .. }));
    let joined = changes.iter().find(|change| matches!(change, Change::Take { before: Took::Absent, after: Took::Listed, .. }));
    if let (false, Some(into)) = (gone.is_empty(), joined) {
        return format!("Merged {} into {}", listing(&gone), into.name());
    }
    if !gone.is_empty() && !added.is_empty() {
        return format!("Replaced {} with {}", listing(&gone), listing(&added));
    }
    if !gone.is_empty() {
        return format!("Deleted {}", listing(&gone));
    }
    let left = changes.iter().find(|change| matches!(change, Change::Take { before: Took::Listed, after: Took::Discarded, .. }));
    if let ([to], Some(Change::Take { name, number, noun, .. })) = (added.as_slice(), left) {
        return format!("Moved {noun} {number} of {name} out as {to}");
    }
    if !added.is_empty() {
        return format!("Added {}", listing(&added));
    }
    let fields: Vec<(&str, &BTreeMap<String, (Value, Value)>)> = changes.iter().filter_map(|change| match change {
        Change::Fields { name, fields } => Some((name.as_str(), fields)),
        _ => None,
    }).collect();
    if let Some((name, (before, after))) = fields.iter().find_map(|(name, fields)| Some((*name, fields.get(super::written::TOOL)?))) {
        return match (before.is_null(), after.is_null()) {
            (true, _) => format!("Put {} on {name}", tool_word(after)),
            (_, true) => format!("Took {} off {name}", tool_word(before)),
            _ => format!("Switched {name} to {}", tool_word(after)),
        };
    }
    if let Some(Change::Take { name, number, noun, before, after }) = changes.iter().find(|change| matches!(change, Change::Take { .. })) {
        return match (before, after) {
            (_, Took::Discarded) | (_, Took::Absent) => format!("Discarded {noun} {number} of {name}"),
            (Took::Absent, _) => format!("Added {noun} {number} to {name}"),
            _ => format!("Brought back {noun} {number} of {name}"),
        };
    }
    if let Some((name, (_, after))) = fields.iter().find_map(|(name, fields)| Some((*name, fields.get("approved")?))) {
        let Some(prefix) = after.as_str() else { return format!("Unapproved {name}") };
        let held = lock.and_then(|lock| lock.asset(name));
        let number = held.and_then(|held| held.versions.iter().find(|version| version.main().is_some_and(|main| main.trim_start_matches("sha256:").starts_with(prefix))));
        let noun = if held.is_some_and(|held| held.kind == "video") { "take" } else { "version" };
        return match number {
            Some(version) => format!("Approved {noun} {} of {name}", version.number),
            None => format!("Approved {name}"),
        };
    }
    if let Some((_, first)) = fields.first() {
        let said: Vec<&str> = first.keys().map(String::as_str).collect();
        let on: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
        return format!("Set {} on {}", listing(&said), listing(&on));
    }
    if let Some(changed) = changes.iter().find(|change| matches!(change, Change::Asset { .. })) {
        return format!("Changed {}", changed.name());
    }
    let cards: Vec<(&str, &Option<Card>, &Option<Card>)> = changes.iter().filter_map(|change| match change {
        Change::Card { name, before, after } => Some((name.as_str(), before, after)),
        _ => None,
    }).collect();
    let resized = cards.iter().all(|(_, before, after)| matches!((before, after), (Some(before), Some(after)) if before.x == after.x && before.y == after.y));
    let boxes: Vec<&str> = cards.iter().map(|(name, ..)| *name).collect();
    match (resized, boxes.len()) {
        (true, _) => format!("Resized {}", listing(&boxes)),
        (_, 0..=3) => format!("Moved {}", listing(&boxes)),
        (_, n) => format!("Moved {n} boxes"),
    }
}

// Writes, as they're made

tokio::task_local! {
    /// Who a request that edits the project is, and what its client calls the edit.
    static EDITING: RefCell<Editing>;
}

/// Who asked one request's edit, and what its client calls it.
#[derive(Clone, Default)]
struct Editing {
    by: By,
    edits: Vec<String>,
    /// The page arranging its own boxes: nothing to undo.
    quiet: bool,
}

/// Rows this request's write set: each as it was and is now.
pub(super) fn cards(folder: &Folder, rows: Vec<(String, Option<Card>, Option<Card>)>) {
    for (name, before, after) in rows.into_iter().filter(|(_, before, after)| before != after) {
        folder.made(|| Made::Change(Change::Card { name, before, after }));
    }
}

/// This request only arranges the page's own boxes.
pub(super) fn quiet() {
    let _ = EDITING.try_with(|editing| editing.borrow_mut().quiet = true);
}

/// Take `number` of `name` went from `before` to `after`.
pub(super) fn take(folder: &Folder, name: &str, number: u32, noun: &str, before: Took, after: Took) {
    folder.made(|| Made::Change(Change::Take { name: name.to_owned(), number, noun: noun.to_owned(), before, after }));
}

/// `from` is called `to` now: a rename back undoes it, whatever else its writes changed.
pub(super) fn renamed(folder: &Folder, from: &str, to: &str) {
    folder.made(|| Made::Change(Change::Renamed { from: from.to_owned(), to: to.to_owned() }));
}

/// Who a request says it is: `Cozy-Client: page <tab>`, `cli`, else another client of the API.
pub(super) fn by_of(headers: &HeaderMap) -> By {
    let said = headers.get("cozy-client").and_then(|value| value.to_str().ok()).unwrap_or_default().trim();
    let (via, client) = said.split_once(' ').map_or((said, ""), |(via, client)| (via, client.trim()));
    let via = if matches!(via, "page" | "cli") { via } else { "api" };
    By { via: via.to_owned(), client: (!client.is_empty()).then(|| client.chars().take(64).collect()) }
}

/// Who this request is, for what records the client that wrote a row.
pub(super) fn by_now() -> String {
    EDITING.try_with(|editing| {
        let by = &editing.borrow().by;
        by.client.as_ref().map_or_else(|| by.via.clone(), |client| format!("{} {client}", by.via))
    }).unwrap_or_default()
}

/// Whether a request to this path is an edit of a project that its history keeps.
fn kept(method: &Method, path: &str) -> bool {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let ["v1", "projects", _, rest @ ..] = parts.as_slice() else { return false };
    match (method.as_str(), rest) {
        ("POST", ["imports" | "assets" | "approve" | "unpin" | "rename"]) => true,
        ("POST", ["assets", _, "versions"]) => true,
        ("PATCH" | "DELETE" | "PUT", ["assets", _]) => true,
        ("DELETE" | "PUT", ["assets", _, "versions", _]) => true,
        ("PATCH", ["canvas"]) | ("PUT", ["yaml"]) => true,
        _ => false,
    }
}

/// Around every project route: who an edit's request is, for the entry its act writes. Every request says who it is, for the canvas rows it writes.
pub(super) async fn record(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let said = |said: &str| said.split(',').map(|edit| edit.trim().chars().take(64).collect::<String>()).filter(|edit| !edit.is_empty()).collect::<Vec<_>>();
    let edits = kept(request.method(), request.uri().path()).then(|| headers.get("cozy-edit").and_then(|value| value.to_str().ok()).map(said).unwrap_or_default());
    let quiet = edits.is_none();
    let editing = Editing { by: by_of(headers), edits: edits.unwrap_or_default(), quiet };
    EDITING.scope(RefCell::new(editing), next.run(request)).await
}

/// What the entry of an act that made `made` will be, its rows in `extra`: none when no edit's request is making it.
pub(super) fn composed(folder: &Folder, made: Vec<Made>, extra: &mut db::Extra) -> Option<Vec<Entry>> {
    let editing = EDITING.try_with(|editing| editing.borrow().clone()).ok()?;
    if editing.quiet || made.is_empty() {
        return None;
    }
    let mut changes = vec![];
    for made in made {
        match made {
            Made::Written(before, after) => diffed(&before, &after).into_iter().for_each(|change| push(&mut changes, change)),
            Made::Change(change) => push(&mut changes, change),
        }
    }
    // A rename's writes are all its own: a rename back is what undoes them.
    if let Some(rename) = changes.iter().find(|change| matches!(change, Change::Renamed { .. })).cloned() {
        changes = vec![rename];
    }
    if changes.is_empty() {
        return None;
    }
    // Only an approval's words need the lock: which version that file is.
    let approves = changes.iter().any(|change| matches!(change, Change::Fields { fields, .. } if fields.contains_key("approved")));
    let lock = approves.then(|| folder.lock().ok()).flatten();
    let book = book(folder);
    let mut entries = book.entries.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (rows, gone) = add(&mut entries, editing.by, editing.edits, changes, lock.as_ref());
    extra.ops.extend(rows);
    extra.ops_gone.extend(gone);
    Some(entries)
}

/// What `composed` planned, now its act has committed: what the history holds, told to every stream.
pub(super) fn committed(folder: &Folder, entries: Option<Vec<Entry>>) {
    let Some(entries) = entries else { return };
    let book = book(folder);
    *book.entries.lock().unwrap_or_else(|e| e.into_inner()) = entries;
    tell(folder, &book);
}

fn tell(folder: &Folder, book: &Book) {
    let entries = book.entries.lock().unwrap_or_else(|e| e.into_inner());
    let entry = |id: Option<u64>| id.and_then(|id| entries.iter().find(|entry| entry.id == id)).map(|entry| Box::new(entry.listed()));
    let now = told(&entries);
    crate::events::history(&project_id(&folder.dir), entry(now.undo), entry(now.redo));
}

/// `.history.json`, from before `project.db`, as rows: each box's lines read into fields, so undo and redo go on across the cut.
pub(super) fn imported(dir: &FsPath) -> Vec<db::OpRow> {
    #[derive(Deserialize)]
    struct Kept {
        #[serde(default)]
        entries: Vec<Value>,
    }
    let Ok(bytes) = std::fs::read(dir.join(".history.json")) else { return vec![] };
    let Ok(kept) = serde_json::from_slice::<Kept>(&bytes) else { return vec![] };
    let lines = |lines: &Value, name: &str| -> Option<Boxed> {
        let lines = lines.as_str()?;
        let written = Written::from_yaml(&format!("version: 4\n{lines}")).ok()?;
        let held = written.boxes.into_iter().next()?;
        Some(Boxed { name: if held.name.is_empty() { name.to_owned() } else { held.name }, fields: held.fields, at: None })
    };
    let entries = kept.entries.into_iter().filter_map(|mut entry| {
        for change in entry.get_mut("changes")?.as_array_mut()? {
            if change.get("kind").and_then(Value::as_str) != Some("asset") {
                continue;
            }
            let name = change.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            for side in ["before", "after"] {
                let held = change.get(side).filter(|value| !value.is_null()).and_then(|value| lines(value, &name));
                change[side] = serde_json::to_value(held).ok()?;
            }
        }
        serde_json::from_value::<Entry>(entry).ok()
    });
    let mut rows: Vec<Entry> = entries.collect();
    let over = rows.len().saturating_sub(KEPT);
    rows.drain(..over);
    rows.iter().map(Entry::row).collect()
}

// The routes

#[derive(Deserialize)]
pub(super) struct Listing {
    limit: Option<usize>,
}

/// `GET /v1/projects/{id}/history`.
pub(super) async fn list(State(api): State<Arc<Api>>, Path(id): Path<String>, Query(asked): Query<Listing>) -> Result<Json<History>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let book = book(&project.folder);
    let entries = book.entries.lock().unwrap_or_else(|e| e.into_inner());
    let limit = asked.limit.unwrap_or(LISTED).clamp(1, KEPT);
    Ok(Json(History { entries: entries.iter().rev().take(limit).map(Entry::listed).collect(), ..told(&entries) }))
}

#[derive(Clone, Copy, PartialEq)]
enum Way {
    Back,
    Forth,
}

impl Way {
    fn verb(self) -> &'static str {
        match self {
            Way::Back => "undo",
            Way::Forth => "redo",
        }
    }
}

/// `POST /v1/projects/{id}/history/undo`.
pub(super) async fn undo(state: State<Arc<Api>>, id: Path<String>, headers: HeaderMap, body: Result<Bytes, BytesRejection>) -> Result<Json<HistoryStepped>, Refusal> {
    step(state, id, headers, body, Way::Back).await
}

/// `POST /v1/projects/{id}/history/redo`.
pub(super) async fn redo(state: State<Arc<Api>>, id: Path<String>, headers: HeaderMap, body: Result<Bytes, BytesRejection>) -> Result<Json<HistoryStepped>, Refusal> {
    step(state, id, headers, body, Way::Forth).await
}

async fn step(State(api): State<Arc<Api>>, Path(id): Path<String>, headers: HeaderMap, body: Result<Bytes, BytesRejection>, way: Way) -> Result<Json<HistoryStepped>, Refusal> {
    let body = body?;
    let asked: HistoryStep = if body.iter().all(u8::is_ascii_whitespace) { HistoryStep::default() } else { parse(&body)? };
    let project = api.projects.get(&api, &id)?;
    let book = book(&project.folder);
    let _one = book.stepping.lock().await;
    let chosen = {
        let entries = book.entries.lock().unwrap_or_else(|e| e.into_inner());
        chosen(&entries, &asked, way).map_err(Refusal)?
    };
    let now = |book: &Book| told(&book.entries.lock().unwrap_or_else(|e| e.into_inner()));
    let Some(chosen) = chosen else {
        let now = now(&book);
        return Ok(Json(HistoryStepped { entries: vec![], already: true, stopped: None, undo: now.undo, redo: now.redo }));
    };
    let by = by_of(&headers);
    let mut done = vec![];
    let mut stopped = None;
    // `walking` before any transaction, as every writer takes them: a rename put back follows a walk going.
    let walking = project.walking.lock().expect("walking lock");
    let follows = *walking;
    for entry in chosen {
        // Each entry one transaction: all of it put back, and marked so, or none of it.
        let stepped = project.folder.act(|| {
            if let Some(what) = moved(&project, &entry, way) {
                return Err(conflict(&book, &entry, &what.0, way, &what.1).0);
            }
            taken(&api, &project, &id, &entry, way, follows).map_err(|Refusal(why)| why)?;
            let mut marked = entry.clone();
            marked.undone = (way == Way::Back).then(|| Undone { at_ms: now_ms(), by: by.clone() });
            project.folder.write_ops(vec![marked.row()], vec![])?;
            Ok(marked)
        });
        match stepped {
            Ok(marked) => {
                let mut entries = book.entries.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(held) = entries.iter_mut().find(|held| held.id == marked.id) {
                    *held = marked.clone();
                }
                drop(entries);
                done.push(marked.listed());
                tell(&project.folder, &book);
            }
            Err(why) => {
                stopped = Some(Refusal(why));
                break;
            }
        }
    }
    drop(walking);
    for name in done.iter().flat_map(|entry| &entry.names) {
        project.walker.lock().expect("walker lock").moved(name);
    }
    project.asked.notify_one();
    match (stopped, done.is_empty()) {
        (Some(why), true) => Err(why),
        (stopped, _) => {
            let now = now(&book);
            Ok(Json(HistoryStepped { entries: done, already: false, stopped: stopped.map(|Refusal(why)| why), undo: now.undo, redo: now.redo }))
        }
    }
}

/// The entries a step takes, in the order it takes them; `None` when what it names is so already.
fn chosen(entries: &[Entry], asked: &HistoryStep, way: Way) -> Result<Option<Vec<Entry>>, Error> {
    let stands = |entry: &Entry| entry.undone.is_none() == (way == Way::Back);
    let unknown = |what: String| Error::new(Kind::NotFound, "history_unknown", format!("this project's history has no {what}: it keeps the last {KEPT} edits")).next("cozy project history");
    let one = |entry: Option<&Entry>, what: String| match entry {
        None => Err(unknown(what)),
        Some(entry) if stands(entry) => Ok(Some(vec![entry.clone()])),
        Some(_) => Ok(None),
    };
    if let Some(id) = asked.entry {
        return one(entries.iter().find(|entry| entry.id == id), format!("entry {id}"));
    }
    if let Some(edit) = &asked.edit {
        return one(entries.iter().rev().find(|entry| entry.edits.contains(edit)), format!("edit {edit}"));
    }
    if let Some(to) = asked.to {
        if !entries.iter().any(|entry| entry.id == to) {
            return Err(unknown(format!("entry {to}")));
        }
        let taken: Vec<Entry> = match way {
            Way::Back => entries.iter().rev().filter(|entry| entry.id >= to && stands(entry)).cloned().collect(),
            // As they were undone, the last first, down to `to`.
            Way::Forth => {
                let mut undone: Vec<&Entry> = entries.iter().filter(|entry| stands(entry)).collect();
                undone.sort_by_key(|entry| (std::cmp::Reverse(entry.undone.as_ref().map_or(0, |undone| undone.at_ms)), entry.id));
                match undone.iter().position(|entry| entry.id == to) {
                    Some(at) => undone[..=at].iter().map(|entry| (*entry).clone()).collect(),
                    None => vec![],
                }
            }
        };
        return Ok((!taken.is_empty()).then_some(taken));
    }
    let head = match way {
        Way::Back => undo_head(entries),
        Way::Forth => redo_head(entries),
    };
    match head {
        Some(entry) => Ok(Some(vec![entry.clone()])),
        None => Err(Error::new(Kind::Conflict, "history_empty", format!("nothing to {}", way.verb())).next("cozy project history")),
    }
}

/// `entry` taken back or done again, each of its changes in turn, inside the act its step is.
fn taken(api: &Arc<Api>, project: &Arc<Project>, id: &str, entry: &Entry, way: Way, follows: bool) -> Result<(), Refusal> {
    let order: Vec<&Change> = match way {
        Way::Back => entry.changes.iter().rev().collect(),
        Way::Forth => entry.changes.iter().collect(),
    };
    order.into_iter().try_for_each(|change| put(api, project, id, change, way, follows))
}

fn conflict(book: &Book, entry: &Entry, change: &Change, way: Way, what: &str) -> Refusal {
    let entries = book.entries.lock().unwrap_or_else(|e| e.into_inner());
    let later = entries.iter().rev().find(|other| other.id > entry.id && other.undone.is_none() && other.changes.iter().any(|held| held.touches(change.name())));
    let verb = way.verb();
    let said = format!("can't {verb} \"{}\": {what}", entry.words);
    let error = match later {
        Some(later) => Error::new(Kind::Conflict, "history_conflict", format!("{said}, by \"{}\" ({})", later.words, said_by(&later.by)))
            .next(format!("cozy project undo {}", later.id))
            .next(format!("cozy project {verb} {}", entry.id)),
        None => Error::new(Kind::Conflict, "history_conflict", format!("{said}, by a render or before this history")).next("cozy project history"),
    };
    Refusal(error)
}

fn said_by(by: &By) -> String {
    match (by.via.as_str(), &by.client) {
        ("page", _) => "the page".to_owned(),
        ("cli", _) => "the CLI".to_owned(),
        ("file", _) => "an import".to_owned(),
        (_, Some(client)) => client.clone(),
        _ => "the API".to_owned(),
    }
}

/// The first change of `entry` that isn't as it should be before going `way`, and what it finds instead.
fn moved<'a>(project: &Project, entry: &'a Entry, way: Way) -> Option<(&'a Change, String)> {
    entry.changes.iter().find_map(|change| Some((change, moved_one(project, change, way)?)))
}

/// What `change` should find there before going `way`, and what it finds instead, if it isn't so.
fn moved_one(project: &Project, change: &Change, way: Way) -> Option<String> {
    let folder = &project.folder;
    let written = folder.written().ok()?;
    match change {
        Change::Asset { name, before, after } => {
            let now = written.get(name);
            match (found(way, before, after), now) {
                (None, Some(_)) => Some(format!("{name} is in the project again")),
                (Some(_), None) => Some(format!("{name} is gone")),
                (Some(wanted), Some(now)) if !same_fields(wanted, now) => Some(format!("{name} changed since")),
                _ => None,
            }
        }
        Change::Fields { name, fields } => {
            let Some(now) = written.get(name).map(Boxed::map) else { return Some(format!("{name} is gone")) };
            let changed: Vec<&str> = fields.iter().filter(|(field, (before, after))| now.get(*field).unwrap_or(&Value::Null) != found(way, before, after)).map(|(field, _)| field.as_str()).collect();
            (!changed.is_empty()).then(|| format!("{name}'s {} changed since", listing(&changed)))
        }
        Change::Card { .. } => None,
        Change::Take { name, number, noun, before, after } => {
            if written.get(name).is_none() {
                return Some(format!("{name} is gone"));
            }
            let now = folder.lock().ok().map_or(Took::Absent, |lock| took(&lock, name, *number));
            let wanted = *found(way, before, after);
            (!now.is(wanted)).then(|| match now {
                Took::Absent => format!("{name}'s {noun} {number} is gone: `reclaim` cleared it"),
                Took::Listed => format!("{name}'s {noun} {number} is back already"),
                Took::Discarded => format!("{name}'s {noun} {number} is discarded"),
            })
        }
        Change::Renamed { from, to } => {
            let (there, free) = if way == Way::Back { (to, from) } else { (from, to) };
            if written.get(there).is_none() {
                return Some(format!("{there} is gone"));
            }
            written.get(free).map(|_| format!("{free} is taken"))
        }
    }
}

/// What a change should find there before it goes `way`: what it left, or what it found.
fn found<T>(way: Way, before: T, after: T) -> T {
    if way == Way::Back { after } else { before }
}

/// What a change going `way` leaves.
fn wanted<T>(way: Way, before: T, after: T) -> T {
    if way == Way::Back { before } else { after }
}

fn same_fields(a: &Boxed, b: &Boxed) -> bool {
    (a.name == b.name && a.fields == b.fields) || super::format::canonical(&Value::Object(a.map())) == super::format::canonical(&Value::Object(b.map()))
}

fn took(lock: &Lock, name: &str, number: u32) -> Took {
    match lock.asset(name) {
        Some(held) if held.version(number).is_some() => Took::Listed,
        Some(held) if held.discarded.iter().any(|version| version.number == number) => Took::Discarded,
        _ => Took::Absent,
    }
}

/// `change` made `way`: the same writes the route that made it makes.
fn put(api: &Arc<Api>, project: &Arc<Project>, id: &str, change: &Change, way: Way, follows: bool) -> Result<(), Refusal> {
    let folder = &project.folder;
    match change {
        Change::Asset { name, before, after } => {
            let there = folder.written().map_err(Refusal)?.get(name).is_some();
            match (wanted(way, before, after), there) {
                (None, _) => super::assets::deleted(api, project, id, name).map(drop),
                (Some(held), false) => super::assets::put_back_box(project, id, held.clone(), None).map(drop),
                (Some(held), true) => folder.edit(|written| written.replaced(name, held)).map_err(Refusal),
            }
        }
        Change::Fields { name, fields } => {
            let set: Map<String, Value> = fields.iter().map(|(field, (before, after))| (field.clone(), wanted(way, before, after).clone())).collect();
            folder.edit(|written| written.set(name, &set).map(drop)).map_err(Refusal)
        }
        // Before its asset is back, as a put back writes it: a page that reads the name has somewhere to put it.
        Change::Card { name, before, after } => canvas::set(folder, id, BTreeMap::from([(name.clone(), *wanted(way, before, after))])).map(drop).map_err(Refusal),
        Change::Take { name, number, before, after, .. } => {
            let wanted = *wanted(way, before, after);
            folder
                .update_lock(|lock| match wanted {
                    Took::Listed => lock.asset_mut(name, "").undiscard(*number),
                    Took::Absent | Took::Discarded => lock.asset_mut(name, "").discard(*number),
                })
                .map_err(Refusal)
        }
        Change::Renamed { from, to } => {
            let (from, to) = if way == Way::Back { (to, from) } else { (from, to) };
            super::renamed_under(project, id, Rename { from: from.clone(), to: to.clone() }, follows).map(drop)
        }
    }
}
