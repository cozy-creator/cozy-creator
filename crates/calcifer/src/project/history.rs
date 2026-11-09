//! A project's history (Paul, 11-06): every edit of it, by the page, the CLI or a hand in project.yaml, in plain words, and what puts it
//! back. Undo belongs to the project, not to a tab: an agent's CLI edit is undone from the page, and the page's from the CLI. It's kept
//! beside project.yaml in `.history.json`, the last 200 entries, and never in project.yaml.
//!
//! What an edit changed is read off its writes as they're made: an asset added or gone (its own lines), its fields (each as it was and
//! is), a canvas row, a take discarded, brought back or joined, a rename. A step back puts each back only while it's as the edit left it:
//! a later edit of the same thing refuses it, naming that edit. A render is never undone, and nothing here rents or ends a machine.
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

use super::{Project, assetfile, canvas, folder::Lock, graph::name_key, now_ms, project_id};
use crate::api::{Api, Refusal, parse};

const FILE: &str = ".history.json";
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
enum Change {
    /// An asset added (`before` none), gone (`after` none), or lines that read as no fields: its own lines, as `assetfile::own` has them.
    Asset { name: String, before: Option<String>, after: Option<String> },
    /// Fields of an asset there before and after, each as it was and is; null is none.
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
struct Entry {
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
}

/// `.history.json`.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    next: u64,
    #[serde(default)]
    entries: Vec<Entry>,
}

struct Ledger {
    dir: PathBuf,
    kept: Kept,
    /// project.yaml as Calcifer last wrote or read it, and its stamp then: a file that isn't this, a hand changed.
    seen: Option<(String, String)>,
}

/// One project's history: what's kept, and a turn for undo and redo, one at a time.
struct Book {
    state: Mutex<Ledger>,
    stepping: tokio::sync::Mutex<()>,
}

static BOOKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Book>>>> = LazyLock::new(Mutex::default);

fn book(dir: &FsPath) -> Arc<Book> {
    let key = durable::canonical(dir).unwrap_or_else(|_| dir.to_owned());
    let mut books = BOOKS.lock().unwrap_or_else(|e| e.into_inner());
    books
        .entry(key)
        .or_insert_with(|| {
            let kept = match std::fs::read(dir.join(FILE)) {
                Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                    warn!("{}: {e}: its history starts again", dir.join(FILE).display());
                    Kept::default()
                }),
                Err(_) => Kept::default(),
            };
            Arc::new(Book { state: Mutex::new(Ledger { dir: dir.to_owned(), kept, seen: None }), stepping: tokio::sync::Mutex::default() })
        })
        .clone()
}

/// The project in `dir` is forgotten: its book goes, and is read from `.history.json` if it's opened again.
pub(super) fn let_go(dir: &FsPath) {
    let key = durable::canonical(dir).unwrap_or_else(|_| dir.to_owned());
    BOOKS.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
}

impl Ledger {
    fn save(&self) {
        let bytes = serde_json::to_vec(&self.kept).expect("a history serializes");
        if let Err(e) = durable::replace_soon(&self.dir.join(FILE), &bytes, 0o644) {
            warn!("keeping {}: {e}", self.dir.join(FILE).display());
        }
    }

    /// `changes`, made by `by`: one more entry, or what `edits` began, if one did and it stands.
    fn add(&mut self, by: By, edits: Vec<String>, changes: Vec<Change>, lock: Option<&Lock>) {
        let joins = self.kept.entries.iter_mut().rev().find(|entry| entry.undone.is_none() && entry.edits.iter().any(|edit| edits.contains(edit)));
        if let Some(entry) = joins {
            changes.into_iter().for_each(|change| push(&mut entry.changes, change));
            for edit in edits {
                if !entry.edits.contains(&edit) {
                    entry.edits.push(edit);
                }
            }
            entry.words = words(&entry.changes, lock);
            self.kept.entries.retain(|entry| !entry.changes.is_empty());
            return;
        }
        let mut composed = vec![];
        changes.into_iter().for_each(|change| push(&mut composed, change));
        if composed.is_empty() {
            return;
        }
        self.kept.next = self.kept.next.max(self.kept.entries.last().map_or(0, |entry| entry.id)) + 1;
        let words = words(&composed, lock);
        self.kept.entries.push(Entry { id: self.kept.next, at_ms: now_ms(), by, words, edits, changes: composed, undone: None });
        let over = self.kept.entries.len().saturating_sub(KEPT);
        self.kept.entries.drain(..over);
    }

    /// What a step back takes with nothing named: the newest that stands.
    fn undo_head(&self) -> Option<&Entry> {
        self.kept.entries.iter().rev().find(|entry| entry.undone.is_none())
    }

    /// What a step forward takes with nothing named: the last undone, unless an edit came after it.
    fn redo_head(&self) -> Option<&Entry> {
        let last = self.kept.entries.iter().filter(|entry| entry.undone.is_some()).max_by_key(|entry| (entry.undone.as_ref().map_or(0, |undone| undone.at_ms), std::cmp::Reverse(entry.id)))?;
        let undone_at = last.undone.as_ref().map_or(0, |undone| undone.at_ms);
        let since = self.kept.entries.iter().any(|entry| entry.undone.is_none() && entry.id > last.id && entry.at_ms > undone_at);
        (!since).then_some(last)
    }

    fn told(&self) -> History {
        History { entries: vec![], undo: self.undo_head().map(|entry| entry.id), redo: self.redo_head().map(|entry| entry.id) }
    }
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
        (Change::Asset { name, before: None, after: Some(lines) }, Change::Fields { fields, .. }) => {
            let set: Map<String, Value> = fields.iter().map(|(field, (_, after))| (field.clone(), after.clone())).collect();
            match assetfile::set(lines, name, &set) {
                Ok((edited, _)) => {
                    *lines = edited;
                    Then::Keep
                }
                Err(_) => Then::Push(Change::Fields { name: name.clone(), fields }),
            }
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

/// What changed between two texts of project.yaml, asset by asset.
fn diffed(before: &str, after: &str) -> Vec<Change> {
    let (was, now) = (assetfile::etags(before), assetfile::etags(after));
    let mut keys: Vec<&String> = was.keys().chain(now.keys()).collect();
    keys.sort();
    keys.dedup();
    let changed = keys.into_iter().filter(|key| was.get(*key) != now.get(*key));
    changed
        .filter_map(|key| match (assetfile::own(before, key), assetfile::own(after, key)) {
            (None, Some((name, lines))) => Some(Change::Asset { name, before: None, after: Some(lines) }),
            (Some((name, lines)), None) => Some(Change::Asset { name, before: Some(lines), after: None }),
            (Some((_, was)), Some((name, now))) => match (assetfile::fields_of(&was), assetfile::fields_of(&now)) {
                (Some(was), Some(now)) => {
                    let mut fields: Vec<&String> = was.keys().chain(now.keys()).collect();
                    fields.sort();
                    fields.dedup();
                    let field = |held: &Map<String, Value>, field: &str| held.get(field).cloned().unwrap_or(Value::Null);
                    let fields: BTreeMap<String, (Value, Value)> = fields.into_iter().map(|name| (name.clone(), (field(&was, name), field(&now, name)))).filter(|(_, (was, now))| was != now).collect();
                    (!fields.is_empty()).then_some(Change::Fields { name, fields })
                }
                _ => Some(Change::Asset { name, before: Some(was), after: Some(now) }),
            },
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
    if let Some((name, (before, after))) = fields.iter().find_map(|(name, fields)| Some((*name, fields.get(assetfile::TOOL)?))) {
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
    /// The edit a request is making, its writes told here as each is made.
    static EDITING: RefCell<Editing>;
}

/// What one request's writes changed, in their order, and who asked.
#[derive(Default)]
struct Editing {
    by: By,
    edits: Vec<String>,
    dir: Option<PathBuf>,
    made: Vec<Made>,
    /// The page arranging its own boxes: nothing to undo.
    quiet: bool,
}

enum Made {
    Text(String, String),
    Change(Change),
}

/// What a write made, told to the edit going, if there is one: nothing is copied for a write of no request's.
fn made(dir: &FsPath, made: impl FnOnce() -> Made) {
    let _ = EDITING.try_with(|editing| {
        let mut editing = editing.borrow_mut();
        editing.dir.get_or_insert_with(|| dir.to_owned());
        editing.made.push(made());
    });
}

/// A write of project.yaml, under its turn. What a hand did since the last is an entry of its own, first.
pub(super) fn wrote(dir: &FsPath, before: &str, after: &str, stamp: String) {
    made(dir, || Made::Text(before.to_owned(), after.to_owned()));
    let book = book(dir);
    let mut state = book.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((seen, _)) = state.seen.take()
        && seen != before
    {
        by_hand(&mut state, &seen, before);
    }
    state.seen = Some((after.to_owned(), stamp));
}

/// Whether project.yaml may be another file than Calcifer last wrote or saw.
pub(super) fn moved_since(dir: &FsPath, stamp: &str) -> bool {
    let book = book(dir);
    let state = book.state.lock().unwrap_or_else(|e| e.into_inner());
    state.seen.as_ref().is_none_or(|(_, seen)| seen != stamp)
}

/// project.yaml as it is now, under its turn: a hand's change, once it reads, is an entry.
pub(super) fn seen(dir: &FsPath, text: &str, stamp: String) {
    let book = book(dir);
    let mut state = book.state.lock().unwrap_or_else(|e| e.into_inner());
    match state.seen.take() {
        Some((seen, _)) if seen != text && !assetfile::reads(text) => {
            state.seen = Some((seen, String::new()));
            return;
        }
        Some((seen, _)) if seen != text => by_hand(&mut state, &seen, text),
        _ => {}
    }
    state.seen = Some((text.to_owned(), stamp));
}

fn by_hand(state: &mut Ledger, was: &str, now: &str) {
    let changes = diffed(was, now);
    if changes.is_empty() {
        return;
    }
    state.add(By { via: "file".into(), client: None }, vec![], changes, None);
    state.save();
    tell(state);
}

/// Rows this request's write set: each as it was and is now.
pub(super) fn cards(dir: &FsPath, rows: Vec<(String, Option<Card>, Option<Card>)>) {
    for (name, before, after) in rows.into_iter().filter(|(_, before, after)| before != after) {
        made(dir, || Made::Change(Change::Card { name, before, after }));
    }
}

/// This request only arranges the page's own boxes.
pub(super) fn quiet() {
    let _ = EDITING.try_with(|editing| editing.borrow_mut().quiet = true);
}

/// Take `number` of `name` went from `before` to `after`.
pub(super) fn take(dir: &FsPath, name: &str, number: u32, noun: &str, before: Took, after: Took) {
    made(dir, || Made::Change(Change::Take { name: name.to_owned(), number, noun: noun.to_owned(), before, after }));
}

/// `from` is called `to` now: a rename back undoes it, whatever else its writes changed.
pub(super) fn renamed(dir: &FsPath, from: &str, to: &str) {
    made(dir, || Made::Change(Change::Renamed { from: from.to_owned(), to: to.to_owned() }));
}

/// Who a request says it is: `Cozy-Client: page <tab>`, `cli`, else another client of the API.
fn by_of(headers: &HeaderMap) -> By {
    let said = headers.get("cozy-client").and_then(|value| value.to_str().ok()).unwrap_or_default().trim();
    let (via, client) = said.split_once(' ').map_or((said, ""), |(via, client)| (via, client.trim()));
    let via = if matches!(via, "page" | "cli") { via } else { "api" };
    By { via: via.to_owned(), client: (!client.is_empty()).then(|| client.chars().take(64).collect()) }
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

/// Around every project route: an edit's writes are told to its history once it's answered, as one entry, or onto what its `Cozy-Edit` began.
pub(super) async fn record(request: Request, next: Next) -> Response {
    if !kept(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    let headers = request.headers();
    let edits = headers.get("cozy-edit").and_then(|value| value.to_str().ok()).map(|said| said.split(',').map(|edit| edit.trim().chars().take(64).collect::<String>()).filter(|edit| !edit.is_empty()).collect()).unwrap_or_default();
    let editing = Editing { by: by_of(headers), edits, ..Editing::default() };
    EDITING
        .scope(RefCell::new(editing), async move {
            let response = next.run(request).await;
            let editing = EDITING.with(RefCell::take);
            if response.status().is_success() {
                commit(editing);
            }
            response
        })
        .await
}

fn commit(editing: Editing) {
    let Some(dir) = editing.dir else { return };
    if editing.quiet {
        return;
    }
    let mut changes = vec![];
    for made in editing.made {
        match made {
            Made::Text(before, after) => diffed(&before, &after).into_iter().for_each(|change| push(&mut changes, change)),
            Made::Change(change) => push(&mut changes, change),
        }
    }
    // A rename's writes are all its own: a rename back is what undoes them.
    if let Some(rename) = changes.iter().find(|change| matches!(change, Change::Renamed { .. })).cloned() {
        changes = vec![rename];
    }
    if changes.is_empty() {
        return;
    }
    // Only an approval's words need the lock: which version that file is.
    let approves = changes.iter().any(|change| matches!(change, Change::Fields { fields, .. } if fields.contains_key("approved")));
    let lock = approves.then(|| super::Folder::at(&dir).lock().ok()).flatten();
    let book = book(&dir);
    let mut state = book.state.lock().unwrap_or_else(|e| e.into_inner());
    state.add(editing.by, editing.edits, changes, lock.as_ref());
    state.save();
    tell(&state);
}

fn tell(state: &Ledger) {
    let entry = |id: Option<u64>| id.and_then(|id| state.kept.entries.iter().find(|entry| entry.id == id)).map(|entry| Box::new(entry.listed()));
    let told = state.told();
    crate::events::history(&project_id(&state.dir), entry(told.undo), entry(told.redo));
}

// The routes

#[derive(Deserialize)]
pub(super) struct Listing {
    limit: Option<usize>,
}

/// `GET /v1/projects/{id}/history`.
pub(super) async fn list(State(api): State<Arc<Api>>, Path(id): Path<String>, Query(asked): Query<Listing>) -> Result<Json<History>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    project.folder.look_by_hand();
    let book = book(&project.folder.dir);
    let state = book.state.lock().unwrap_or_else(|e| e.into_inner());
    let limit = asked.limit.unwrap_or(LISTED).clamp(1, KEPT);
    Ok(Json(History { entries: state.kept.entries.iter().rev().take(limit).map(Entry::listed).collect(), ..state.told() }))
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
    project.folder.look_by_hand();
    let book = book(&project.folder.dir);
    let _one = book.stepping.lock().await;
    let chosen = {
        let state = book.state.lock().unwrap_or_else(|e| e.into_inner());
        chosen(&state, &asked, way).map_err(Refusal)?
    };
    let Some(entries) = chosen else {
        let state = book.state.lock().unwrap_or_else(|e| e.into_inner());
        return Ok(Json(HistoryStepped { entries: vec![], already: true, stopped: None, undo: state.told().undo, redo: state.told().redo }));
    };
    let by = by_of(&headers);
    let mut done = vec![];
    let mut stopped = None;
    for entry in entries {
        if let Err(why) = taken(&api, &project, &id, &book, &entry, way).await {
            stopped = Some(why);
            break;
        }
        let mut state = book.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = state.kept.entries.iter_mut().find(|held| held.id == entry.id) {
            held.undone = (way == Way::Back).then(|| Undone { at_ms: now_ms(), by: by.clone() });
            done.push(held.listed());
        }
        state.save();
        tell(&state);
    }
    for name in done.iter().flat_map(|entry| &entry.names) {
        project.walker.lock().expect("walker lock").moved(name);
    }
    project.asked.notify_one();
    match (stopped, done.is_empty()) {
        (Some(why), true) => Err(why),
        (stopped, _) => {
            let state = book.state.lock().unwrap_or_else(|e| e.into_inner());
            Ok(Json(HistoryStepped { entries: done, already: false, stopped: stopped.map(|Refusal(why)| why), undo: state.told().undo, redo: state.told().redo }))
        }
    }
}

/// The entries a step takes, in the order it takes them; `None` when what it names is so already.
fn chosen(state: &Ledger, asked: &HistoryStep, way: Way) -> Result<Option<Vec<Entry>>, Error> {
    let entries = &state.kept.entries;
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
        Way::Back => state.undo_head(),
        Way::Forth => state.redo_head(),
    };
    match head {
        Some(entry) => Ok(Some(vec![entry.clone()])),
        None => Err(Error::new(Kind::Conflict, "history_empty", format!("nothing to {}", way.verb())).next("cozy project history")),
    }
}

/// `entry` taken back or done again, each of its changes in turn, only while each is as the other way left it. Refused part way, what it
/// changed is put as it was.
async fn taken(api: &Arc<Api>, project: &Arc<Project>, id: &str, book: &Book, entry: &Entry, way: Way) -> Result<(), Refusal> {
    let order: Vec<&Change> = match way {
        Way::Back => entry.changes.iter().rev().collect(),
        Way::Forth => entry.changes.iter().collect(),
    };
    // What each did, to put back if a later one is refused: a row as it was just now, whatever moved it, or the change the other way.
    let mut done: Vec<Change> = vec![];
    for change in order {
        let was = match change {
            Change::Card { name, .. } => Some(Change::Card { name: name.clone(), before: canvas::row(&project.folder, name).map_err(Refusal)?, after: None }),
            _ => None,
        };
        let refused = match moved(project, change, way) {
            Some(what) => Some(conflict(book, entry, change, way, &what)),
            None => put(api, project, id, change, way).await.err(),
        };
        if let Some(refused) = refused {
            let back = if way == Way::Back { Way::Forth } else { Way::Back };
            for (change, toward) in done.iter().rev().map(|change| match change {
                Change::Card { .. } => (change, Way::Back),
                change => (change, back),
            }) {
                if let Err(Refusal(why)) = put(api, project, id, change, toward).await {
                    warn!("putting back what a refused {} changed: {}", way.verb(), why.message);
                }
            }
            return Err(refused);
        }
        done.push(was.unwrap_or_else(|| change.clone()));
    }
    Ok(())
}

fn conflict(book: &Book, entry: &Entry, change: &Change, way: Way, what: &str) -> Refusal {
    let state = book.state.lock().unwrap_or_else(|e| e.into_inner());
    let later = state.kept.entries.iter().rev().find(|other| other.id > entry.id && other.undone.is_none() && other.changes.iter().any(|held| held.touches(change.name())));
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
        ("file", _) => "project.yaml, by hand".to_owned(),
        (_, Some(client)) => client.clone(),
        _ => "the API".to_owned(),
    }
}

/// What `change` should find there before going `way`, and what it finds instead, if it isn't so.
fn moved(project: &Project, change: &Change, way: Way) -> Option<String> {
    let folder = &project.folder;
    match change {
        Change::Asset { name, before, after } => {
            let text = folder.text().ok()?;
            let now = assetfile::own(&text, name).map(|(_, lines)| lines);
            match (found(way, before, after), now) {
                (None, Some(_)) => Some(format!("{name} is in the project again")),
                (Some(_), None) => Some(format!("{name} is gone")),
                (Some(wanted), Some(now)) if !same_lines(wanted, &now) => Some(format!("{name} changed since")),
                _ => None,
            }
        }
        Change::Fields { name, fields } => {
            let text = folder.text().ok()?;
            let Some(now) = assetfile::own(&text, name).and_then(|(_, lines)| assetfile::fields_of(&lines)) else { return Some(format!("{name} is gone")) };
            let changed: Vec<&str> = fields.iter().filter(|(field, (before, after))| now.get(*field).unwrap_or(&Value::Null) != found(way, before, after)).map(|(field, _)| field.as_str()).collect();
            (!changed.is_empty()).then(|| format!("{name}'s {} changed since", listing(&changed)))
        }
        Change::Card { .. } => None,
        Change::Take { name, number, noun, before, after } => {
            if folder.text().is_ok_and(|text| assetfile::own(&text, name).is_none()) {
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
            let text = folder.text().ok()?;
            let (there, free) = if way == Way::Back { (to, from) } else { (from, to) };
            if assetfile::own(&text, there).is_none() {
                return Some(format!("{there} is gone"));
            }
            assetfile::own(&text, free).map(|_| format!("{free} is taken"))
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

fn same_lines(a: &str, b: &str) -> bool {
    a == b || matches!((assetfile::fields_of(a), assetfile::fields_of(b)), (Some(a), Some(b)) if a == b)
}

fn took(lock: &Lock, name: &str, number: u32) -> Took {
    match lock.asset(name) {
        Some(held) if held.version(number).is_some() => Took::Listed,
        Some(held) if held.discarded.iter().any(|version| version.number == number) => Took::Discarded,
        _ => Took::Absent,
    }
}

/// `change` made `way`: the same writes the route that made it makes.
async fn put(api: &Arc<Api>, project: &Arc<Project>, id: &str, change: &Change, way: Way) -> Result<(), Refusal> {
    let folder = &project.folder;
    match change {
        Change::Asset { name, before, after } => {
            let there = assetfile::own(&folder.text().map_err(Refusal)?, name).is_some();
            match (wanted(way, before, after), there) {
                (None, _) => super::assets::deleted(api, project, id, name).map(drop),
                (Some(lines), false) => super::assets::put_back_lines(project, id, name, lines, None).map(drop),
                (Some(lines), true) => folder.edit_project(|text| assetfile::replaced(text, name, lines)).map(drop).map_err(Refusal),
            }
        }
        Change::Fields { name, fields } => {
            let set: Map<String, Value> = fields.iter().map(|(field, (before, after))| (field.clone(), wanted(way, before, after).clone())).collect();
            folder.edit_project(|text| assetfile::set(text, name, &set).map(|(edited, _)| edited)).map(drop).map_err(Refusal)
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
            super::rename_now(project, id, Rename { from: from.clone(), to: to.clone() }).await.map(drop)
        }
    }
}
