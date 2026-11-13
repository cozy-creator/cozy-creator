//! The canonical logical project document. SQLite only commits its Automerge snapshot/change bytes
//! atomically with local execution bookkeeping and disposable query projections. Media stays outside.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, OnceLock},
};

use api::error::{Error, Kind};
use automerge::{
    ActorId, AutoCommit, ChangeHash, ObjId, ObjType, Prop, ROOT, ReadDoc, ScalarValue, Value as AmValue, ValueRef, transaction::Transactable,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{
    db::{CardRow, OpRow, Stored},
    folder::{Asset, File, Lock, RunFile, Version},
    graph::name_key,
    written::{Boxed, Written},
};

pub(super) const FORMAT: i64 = 1;

#[derive(Clone, Debug)]
pub(super) struct Document {
    pub(super) doc: AutoCommit,
    projection: Arc<Projection>,
    conflicts: Arc<OnceLock<Vec<String>>>,
    /// Where it stands: what a typed edit was made on (`typed`).
    heads: Arc<Vec<ChangeHash>>,
}

/// One splice of a box's words, in Unicode code points: `delete` of them at `at`, then `insert`.
pub(super) type Splice = api::calcifer::project::Splice;

// The selected native view shares unchanged roots and records across a write.
// Only the native document is persisted.
type Projection = BTreeMap<String, Group>;
type Records = BTreeMap<String, Arc<Value>>;

#[derive(Clone, Debug)]
enum Group {
    Value(Arc<Value>),
    Records(Arc<Records>),
}

/// What a project from before kept here and project.db's rows now keep alone: its files, versions, runs and what tools make.
const OUTSIDE: [&str; 5] = ["assets", "files", "runs", "reserved", "makes"];

fn record_namespace(key: &str) -> bool {
    matches!(key, "boxes" | "assets" | "files" | "canvas" | "history" | "runs")
}

impl Group {
    fn from_value(key: &str, value: Value) -> Self {
        match value {
            Value::Object(rows) if record_namespace(key) => {
                Self::Records(Arc::new(rows.into_iter().map(|(id, row)| (id, Arc::new(row))).collect()))
            }
            value => Self::Value(Arc::new(value)),
        }
    }
}

fn root_value<'a>(projection: &'a Projection, key: &str) -> Option<&'a Value> {
    match projection.get(key) {
        Some(Group::Value(value)) => Some(value),
        _ => None,
    }
}

fn root_records<'a>(projection: &'a Projection, key: &str) -> Option<&'a Records> {
    match projection.get(key) {
        Some(Group::Records(records)) => Some(records),
        _ => None,
    }
}

fn record_entries<'a>(projection: &'a Projection, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    root_records(projection, key).into_iter().flatten().map(|(id, row)| (id, row.as_ref()))
}

fn set_value(projection: &mut Projection, key: &str, value: Value) {
    if root_value(projection, key) != Some(&value) {
        projection.insert(key.to_owned(), Group::Value(Arc::new(value)));
    }
}

fn records_mut<'a>(projection: &'a mut Projection, key: &str) -> &'a mut Records {
    let group = projection.entry(key.to_owned()).or_insert_with(|| Group::Records(Arc::default()));
    if !matches!(group, Group::Records(_)) {
        *group = Group::Records(Arc::default());
    }
    let Group::Records(records) = group else { unreachable!("record namespace") };
    Arc::make_mut(records)
}

fn invalid(detail: impl std::fmt::Display) -> Error {
    Error::new(Kind::Validation, "project_crdt", detail.to_string())
}

/// Every loaded local replica begins a fresh writer session. Existing actors and causal history
/// stay in the document, but a copied/restored snapshot can never resume an old actor's counters.
fn actor(_at: &Path) -> ActorId {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS has randomness");
    ActorId::from(bytes.as_slice())
}

/// Materialize native winners and inspect conflicts in the same sequential walk. Only
/// conflicting properties need get_all; their alternative objects are still all inspected.
fn materialize(doc: &AutoCommit, object: &ObjId, value: AmValue<'_>, path: &str, mut found: Option<&mut Vec<String>>) -> Result<Value, Error> {
    match value {
        AmValue::Object(ObjType::Map | ObjType::Table) => {
            read_map(doc, object, path, found, |_, child, value, at, nested| {
                materialize(doc, child, value, at, nested)
            }).map(Value::Object)
        }
        AmValue::Object(ObjType::List) => doc.list_range(object, ..).map(|item| {
            let child = (item.conflict || matches!(item.value, ValueRef::Object(_))).then(|| item.id());
            if item.conflict && let Some(found) = found.as_deref_mut() {
                conflict_at(doc, object, Prop::Seq(item.index), path, child.as_ref(), true, found);
            }
            let at = child.as_ref().map(|_| beneath(path, &item.index.to_string())).unwrap_or_default();
            let nested = if item.conflict { None } else { found.as_deref_mut() };
            materialize(doc, child.as_ref().unwrap_or(object), item.value.into_value(), &at, nested)
        }).collect::<Result<Vec<_>, Error>>().map(Value::Array),
        AmValue::Object(ObjType::Text) => doc.text(object).map(Value::String).map_err(invalid),
        AmValue::Scalar(value) => serde_json::to_value(value).map_err(invalid),
    }
}

/// Visit selected map values in native order and inspect every retained alternative.
fn read_map<T, M: FromIterator<(String, T)>>(
    doc: &AutoCommit,
    object: &ObjId,
    path: &str,
    mut found: Option<&mut Vec<String>>,
    mut read: impl FnMut(&str, &ObjId, AmValue<'_>, &str, Option<&mut Vec<String>>) -> Result<T, Error>,
) -> Result<M, Error> {
    doc.map_range(object, ..).map(|item| {
        let child = (item.conflict || matches!(item.value, ValueRef::Object(_))).then(|| item.id());
        let key = item.key.into_owned();
        if item.conflict && let Some(found) = found.as_deref_mut() {
            conflict_at(doc, object, Prop::Map(key.clone()), path, child.as_ref(), true, found);
        }
        let at = child.as_ref().map(|_| beneath(path, &key)).unwrap_or_default();
        let nested = if item.conflict { None } else { found.as_deref_mut() };
        let value = read(&key, child.as_ref().unwrap_or(object), item.value.into_value(), &at, nested)?;
        Ok((key, value))
    }).collect()
}

fn project(doc: &AutoCommit, found: &mut Vec<String>) -> Result<Projection, Error> {
    read_map(doc, &ROOT, "", Some(found), |key, object, value, path, found| {
        if record_namespace(key) && matches!(value, AmValue::Object(ObjType::Map | ObjType::Table)) {
            let rows = read_map(doc, object, path, found, |_, child, value, at, nested| {
                materialize(doc, child, value, at, nested).map(Arc::new)
            })?;
            Ok(Group::Records(Arc::new(rows)))
        } else {
            materialize(doc, object, value, path, found).map(|value| Group::from_value(key, value))
        }
    })
}

fn beneath(path: &str, label: &str) -> String {
    if path.is_empty() { label.to_owned() } else { format!("{path}/{label}") }
}

fn conflicts_under(doc: &AutoCommit, object: &ObjId, path: &str, found: &mut Vec<String>) {
    match doc.object_type(object) {
        Ok(ObjType::Map | ObjType::Table) => {
            for item in doc.map_range(object, ..) {
                let child = (item.conflict || matches!(item.value, ValueRef::Object(_))).then(|| item.id());
                if item.conflict || child.is_some() {
                    conflict_at(doc, object, Prop::Map(item.key.into_owned()), path, child.as_ref(), item.conflict, found);
                }
            }
        }
        Ok(ObjType::List) => {
            for item in doc.list_range(object, ..) {
                let child = (item.conflict || matches!(item.value, ValueRef::Object(_))).then(|| item.id());
                if item.conflict || child.is_some() {
                    conflict_at(doc, object, Prop::Seq(item.index), path, child.as_ref(), item.conflict, found);
                }
            }
        }
        _ => {}
    }
}

fn conflict_at(doc: &AutoCommit, object: &ObjId, property: Prop, path: &str, child: Option<&ObjId>, conflict: bool, found: &mut Vec<String>) {
    let label = match &property { Prop::Map(key) => key.clone(), Prop::Seq(index) => index.to_string() };
    let at = beneath(path, &label);
    if !conflict {
        if let Some(child) = child { conflicts_under(doc, child, &at, found); }
        return;
    }
    let Ok(values) = doc.get_all(object, property) else { return };
    if values.len() > 1 {
        let alternatives = values.iter().map(|(value, operation)| {
            let value = value.as_str().map(str::to_owned).unwrap_or_else(|| format!("{value:?}"));
            format!("{operation}: {value}")
        }).collect::<Vec<_>>().join(" | ");
        found.push(format!("Concurrent project values at {at}: {alternatives}. The alternatives are retained for explicit resolution."));
    }
    for (value, child) in values {
        if matches!(value, AmValue::Object(_)) { conflicts_under(doc, &child, &at, found); }
    }
}

pub(super) fn head_set(heads: &str) -> Result<BTreeSet<ChangeHash>, Error> {
    let heads: Vec<String> = serde_json::from_str(heads).map_err(invalid)?;
    heads.into_iter().map(|head| head.parse().map_err(invalid)).collect()
}

fn load_native(bytes: &[u8], heads: &str) -> Result<AutoCommit, Error> {
    // The pinned loader rejects a partial trailing chunk. A complete older prefix is
    // syntactically valid, so also require the transaction's expected native heads.
    let mut doc = AutoCommit::load(bytes).map_err(invalid)?;
    if doc.get_heads().into_iter().collect::<BTreeSet<_>>() != head_set(heads)? {
        return Err(invalid("the native project stream does not match its committed heads"));
    }
    Ok(doc)
}

pub(super) fn identity_bytes(bytes: &[u8], heads: &str) -> Option<(String, String)> {
    let doc = load_native(bytes, heads).ok()?;
    let (_, identity) = doc.get(ROOT, "identity").ok()??;
    let (id, _) = doc.get(&identity, "id").ok()??;
    let name = doc
        .get(&identity, "name")
        .ok()
        .flatten()
        .and_then(|(name, _)| name.as_str().map(str::to_owned))
        .unwrap_or_default();
    Some((id.as_str()?.to_owned(), name))
}

impl Document {
    pub(super) fn load(bytes: &[u8], heads: &str, at: &Path) -> Result<Self, Error> {
        let mut doc = load_native(bytes, heads)?.with_actor(actor(at));
        let heads = Arc::new(doc.get_heads());
        let mut found = Vec::new();
        let projection = project(&doc, &mut found)?;
        if root_value(&projection, "schema").and_then(Value::as_u64) != Some(FORMAT as u64) {
            return Err(invalid(
                "unsupported logical project schema; update cozy before opening this project",
            ));
        }
        Ok(Self {
            doc,
            projection: Arc::new(projection),
            conflicts: Arc::new(OnceLock::from(found)),
            heads,
        })
    }

    pub(super) fn new(at: &Path) -> Self {
        Self {
            doc: AutoCommit::new().with_actor(actor(at)),
            projection: Arc::default(),
            conflicts: Arc::new(OnceLock::new()),
            heads: Arc::default(),
        }
    }

    /// The complete initial state of a freshly created or migrated document.
    pub(super) fn initialize(&mut self, value: Value) -> Result<(), Error> {
        let Value::Object(fields) = value else {
            return Err(invalid("logical project root is not a map"));
        };
        self.update_projection(fields.into_iter().map(|(key, value)| {
            let group = Group::from_value(&key, value);
            (key, group)
        }).collect())
    }

    pub(super) fn update_stored(&mut self, before: &Stored, after: &Stored, ops: &[OpRow]) -> Result<(), Error> {
        let projection = changed_projection(before, after, ops, &self.projection, self.identity());
        self.update_projection(projection)
    }

    fn update_projection(&mut self, projection: Projection) -> Result<(), Error> {
        let known_empty = self.conflicts.get().is_some_and(Vec::is_empty);
        reconcile_projection(&mut self.doc, &self.projection, &projection)?;
        self.doc.commit();
        self.heads = Arc::new(self.doc.get_heads());
        self.projection = Arc::new(projection);
        // A local observing mutation cannot introduce concurrency. Imported snapshots seed
        // this cache during materialization; existing alternatives invalidate it on mutation.
        let conflicts = OnceLock::new();
        if known_empty {
            let _ = conflicts.set(Vec::new());
        }
        self.conflicts = Arc::new(conflicts);
        Ok(())
    }

    /// Its heads, as a read names them.
    pub(super) fn heads(&self) -> Vec<String> {
        self.heads.iter().map(ToString::to_string).collect()
    }

    /// What the words at `path` of box `id` (a field, or words in it: `beats.0.description`) say once `splices`, made on them as the
    /// document stood at `heads`, merge with all written since: by character, as Automerge merges text, so two people typing in one
    /// prompt keep both (cl-904). Words that weren't text then (a project from before) merge from what they said then.
    pub(super) fn typed(&self, id: &str, path: &str, heads: &[String], splices: &[Splice]) -> Result<String, Error> {
        let field = path;
        let heads = heads.iter().map(|head| head.parse::<ChangeHash>().map_err(invalid)).collect::<Result<Vec<_>, _>>()?;
        // Typed on the project as it stands, as one person typing is: nothing to merge, and nothing copied.
        if heads.iter().collect::<BTreeSet<_>>() == self.heads.iter().collect::<BTreeSet<_>>() {
            let mut words: Vec<char> = self.projected_words(id, field).chars().collect();
            for splice in splices {
                let at = (splice.at as usize).min(words.len());
                let delete = (splice.delete as usize).min(words.len() - at);
                words.splice(at..at + delete, splice.insert.chars()).for_each(drop);
            }
            return Ok(words.into_iter().collect());
        }
        let mut now = self.doc.clone();
        let mut then = now.fork_at(&heads).map_err(|_| {
            Error::new(Kind::Conflict, "project_heads_unknown", "this edit was made on a project this Calcifer doesn't hold").remedy("read the project again")
        })?;
        if let Some(text) = text_of(&then, id, field)? {
            for splice in splices {
                splice_in(&mut then, &text, splice)?;
            }
            now.merge(&mut then).map_err(invalid)?;
            return words_of(&now, id, field);
        }
        let (base, current) = (words_of(&then, id, field)?, words_of(&now, id, field)?);
        let mut scratch = AutoCommit::new();
        let text = scratch.put_object(ROOT, "words", ObjType::Text).map_err(invalid)?;
        scratch.splice_text(&text, 0, 0, &base).map_err(invalid)?;
        let mut theirs = scratch.fork();
        let (at, delete, insert) = splice_between(&base, &current);
        theirs.splice_text(&text, at, delete as isize, insert).map_err(invalid)?;
        for splice in splices {
            splice_in(&mut scratch, &text, splice)?;
        }
        scratch.merge(&mut theirs).map_err(invalid)?;
        scratch.text(&text).map_err(invalid)
    }

    /// The words at `path` of box `id`, as the projection holds them: "" for none or for what isn't words.
    fn projected_words(&self, id: &str, path: &str) -> String {
        let Some(Group::Records(boxes)) = self.projection.get("boxes") else { return String::new() };
        let found = boxes.get(id).and_then(|held| path.split('.').try_fold(held.get("fields")?, |at, step| match at {
            Value::Array(items) => items.get(step.parse::<usize>().ok()?),
            _ => at.get(step),
        }));
        found.and_then(Value::as_str).unwrap_or_default().to_owned()
    }

    /// Explicit conflict metadata, rather than silently presenting AutoSerde's selected winner.
    pub(super) fn conflicts(&self) -> Vec<String> {
        self.conflicts
            .get_or_init(|| {
                let mut found = Vec::new();
                conflicts_under(&self.doc, &ROOT, "", &mut found);
                found
            })
            .clone()
    }

    pub(super) fn identity(&self) -> Option<(String, String)> {
        let identity = root_value(&self.projection, "identity")?;
        Some((
            identity.get("id")?.as_str()?.to_owned(),
            identity.get("name")?.as_str().unwrap_or_default().to_owned(),
        ))
    }

    pub(super) fn set_identity(&mut self, id: &str, name: &str) -> Result<(), Error> {
        let mut projection = (*self.projection).clone();
        set_value(&mut projection, "identity", json!({ "id": id, "name": name }));
        self.update_projection(projection)
    }

    pub(super) fn ops(&self) -> Result<Vec<OpRow>, Error> {
        let mut rows = Vec::new();
        for (uid, record) in record_entries(&self.projection, "history").filter(|(_, record)| present(record)) {
            let mut row = decode::<OpRow>(&record["record"])?;
            row.uid.clone_from(uid);
            rows.push(row);
        }
        rows.sort_by(|a, b| (&a.id, &a.uid).cmp(&(&b.id, &b.uid)));
        Ok(rows)
    }

    /// What a project from before kept here of its files, versions and runs: project.db's rows take it once (`db::migrate`).
    pub(super) fn legacy_lock(&self) -> Result<Lock, Error> {
        let mut lock = Lock::default();
        for (id, record) in record_entries(&self.projection, "assets").filter(|(_, record)| present(record)) {
            let mut held: Asset = decode(&record["record"])?;
            held.id.clone_from(id);
            for (version_id, version) in entries(record, "versions").filter(|(_, record)| present(record)) {
                let mut value: Version = decode(&version["record"])?;
                value.id.clone_from(version_id);
                match version["discarded"].as_bool().unwrap_or(false) {
                    true => held.discarded.push(value),
                    false => held.versions.push(value),
                }
            }
            held.versions.sort_by(|a, b| (&a.number, &a.id).cmp(&(&b.number, &b.id)));
            held.discarded.sort_by(|a, b| (&a.number, &a.id).cmp(&(&b.number, &b.id)));
            let key = text(record, "key");
            if lock.assets.insert(key, held).is_some() {
                return Err(invalid(
                    "concurrent assets have the same local name; resolve the name conflict before execution",
                ));
            }
        }
        lock.files = record_entries(&self.projection, "files")
            .map(|(hash, value)| decode::<File>(value).map(|file| (hash.clone(), File { row: true, ..file })))
            .collect::<Result<_, _>>()?;
        let empty = json!({});
        lock.reserved = decode(root_value(&self.projection, "reserved").unwrap_or(&empty))?;
        lock.makes = decode(root_value(&self.projection, "makes").unwrap_or(&empty))?;
        let mut runs = Vec::new();
        for (id, record) in record_entries(&self.projection, "runs").filter(|(_, record)| present(record)) {
            let mut run: RunFile = decode(&record["record"])?;
            run.id.clone_from(id);
            runs.push((record["order"].as_u64().unwrap_or_default(), run));
        }
        runs.sort_by(|(a, first), (b, second)| (a, &first.id).cmp(&(b, &second.id)));
        lock.runs = runs.into_iter().map(|(_, run)| run).collect();
        Ok(lock)
    }

    /// The heads `rev` (`api::calcifer::node::rev_of`) stands for: these, or those right after one change of its history.
    pub(super) fn heads_of(&self, rev: &str) -> Option<Vec<String>> {
        let now = self.heads();
        if api::calcifer::node::rev_of(&now) == rev {
            return Some(now);
        }
        let mut doc = self.doc.clone();
        let found: Vec<String> = doc.get_changes_meta(&[]).into_iter().map(|change| change.hash.to_string())
            .filter(|hash| api::calcifer::node::rev_of(std::slice::from_ref(hash)) == rev).collect();
        match found.as_slice() {
            [one] => Some(vec![one.clone()]),
            _ => None,
        }
    }

    /// The project's boxes as they stood at `heads`.
    pub(super) fn written_at(&self, heads: &[String]) -> Result<Written, Error> {
        let heads = heads.iter().map(|head| head.parse::<ChangeHash>().map_err(invalid)).collect::<Result<Vec<_>, _>>()?;
        let then = self.doc.clone().fork_at(&heads).map_err(invalid)?;
        let mut found = Vec::new();
        Ok(written_of(&project(&then, &mut found)?))
    }

    pub(super) fn into_stored(self) -> Result<Stored, Error> {
        let written = written_of(&self.projection);
        let mut canvas = BTreeMap::new();
        for (_, record) in record_entries(&self.projection, "canvas").filter(|(_, record)| present(record)) {
            let row: CardRow = decode(&record["record"])?;
            canvas.insert(text(record, "key"), row);
        }
        Ok(Stored {
            written: Arc::new(written),
            canvas: Arc::new(canvas),
            crdt: Some(Arc::new(self)),
            ..Default::default()
        })
    }
}

/// The boxes and head sections `projection` holds, in their order.
fn written_of(projection: &Projection) -> Written {
    let head = ordered(root_value(projection, "head_order"), root_value(projection, "head").and_then(Value::as_object))
        .into_iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut boxes = Vec::new();
    for (id, record) in record_entries(projection, "boxes").filter(|(_, record)| present(record)) {
        let fields = ordered(record.get("field_order"), object(record, "fields")).into_iter().map(|(key, value)| (key.clone(), value.clone())).collect();
        boxes.push((record["order"].as_u64().unwrap_or_default(), Boxed { id: id.clone(), name: text(record, "name"), fields, at: None }));
    }
    boxes.sort_by(|(a, first), (b, second)| (a, &first.id).cmp(&(b, &second.id)));
    Written { head, boxes: boxes.into_iter().map(|(_, held)| held).collect() }
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, Error> {
    T::deserialize(value).map_err(invalid)
}
fn encoded<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a logical project value serializes")
}
fn object<'a>(value: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    value.get(key).and_then(Value::as_object)
}
fn entries<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    object(value, key).into_iter().flatten()
}
fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}
fn present(value: &Value) -> bool {
    value.get("present").and_then(Value::as_bool).unwrap_or(true)
}
fn ordered<'a>(order: Option<&Value>, values: Option<&'a Map<String, Value>>) -> Vec<(&'a String, &'a Value)> {
    let Some(values) = values else { return Vec::new() };
    let mut rows: Vec<_> = order
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|key| values.get_key_value(key))
        .collect();
    let listed: BTreeSet<_> = rows.iter().map(|(key, _)| key.as_str()).collect();
    rows.extend(values.iter().filter(|(key, _)| !listed.contains(key.as_str())));
    rows
}
fn map_of(values: &[(String, Value)]) -> Value {
    Value::Object(values.iter().cloned().collect())
}
fn keys_of(values: &[(String, Value)]) -> Value {
    encoded(&values.iter().map(|(key, _)| key).collect::<Vec<_>>())
}
fn tombstoned(previous: &Value, field: &str) -> Map<String, Value> {
    let mut all = object(previous, field).cloned().unwrap_or_default();
    for value in all.values_mut() {
        value["present"] = Value::Bool(false);
    }
    all
}

/// Assign identities once, preserving them across renames, export/import, undo, and discard/restore.
pub(super) fn identify(before: &Stored, after: &mut Stored) {
    if after.written.boxes.iter().any(|held| held.id.is_empty()) {
        let previous: BTreeMap<_, _> = before
            .written
            .boxes
            .iter()
            .map(|held| (name_key(&held.name), held.id.clone()))
            .collect();
        for held in &mut Arc::make_mut(&mut after.written).boxes {
            if held.id.is_empty() {
                held.id = previous
                    .get(&name_key(&held.name))
                    .filter(|id| !id.is_empty())
                    .cloned()
                    .unwrap_or_else(super::new_key);
            }
        }
    }
    if after.lock.runs.iter().any(|row| row.id.is_empty()) {
        for row in &mut Arc::make_mut(&mut after.lock).runs {
            if row.id.is_empty() { row.id = super::new_key(); }
        }
    }
    if !after
        .lock
        .assets
        .values()
        .any(|held| held.id.is_empty() || held.on_record().any(|version| version.id.is_empty()))
    {
        return;
    }
    let named: BTreeMap<_, _> = after
        .written
        .boxes
        .iter()
        .map(|held| (name_key(&held.name), held.id.clone()))
        .collect();
    for (key, held) in &mut Arc::make_mut(&mut after.lock).assets {
        if held.id.is_empty() {
            held.id = before
                .lock
                .assets
                .get(key)
                .map(|old| old.id.clone())
                .filter(|id| !id.is_empty())
                .or_else(|| named.get(key).cloned())
                .unwrap_or_else(super::new_key);
        }
        let known: BTreeMap<_, _> = before
            .lock
            .assets
            .values()
            .filter(|old| old.id == held.id)
            .flat_map(|old| old.on_record())
            .map(|version| (version.number, version.id.clone()))
            .collect();
        for version in held.versions.iter_mut().chain(&mut held.discarded) {
            if version.id.is_empty() {
                version.id = known
                    .get(&version.number)
                    .filter(|id| !id.is_empty())
                    .cloned()
                    .unwrap_or_else(super::new_key);
            }
        }
    }
}

pub(super) fn identify_history(stored: &Stored, ops: &mut [OpRow]) {
    let mut ids: BTreeMap<String, String> = stored
        .written
        .boxes
        .iter()
        .map(|held| (name_key(&held.name), held.id.clone()))
        .collect();
    for op in ops.iter_mut().rev() {
        if op.uid.is_empty() {
            op.uid = super::new_key();
        }
        let Ok(mut changes) = serde_json::from_str::<Value>(&op.changes) else {
            continue;
        };
        let Some(changes_array) = changes.as_array_mut() else { continue };
        for change in changes_array.iter_mut().rev() {
            if change["kind"] == "renamed" {
                let (from, to) = (text(change, "from"), text(change, "to"));
                let id = ids.remove(&name_key(&to)).unwrap_or_else(super::new_key);
                ids.insert(name_key(&from), id);
                continue;
            }
            if change["kind"] != "asset" {
                continue;
            }
            let name = text(change, "name");
            let key = name_key(&name);
            let id = ["after", "before"]
                .into_iter()
                .find_map(|side| change.get(side)?.get("id")?.as_str().filter(|id| !id.is_empty()).map(str::to_owned))
                .or_else(|| ids.get(&key).cloned())
                .unwrap_or_else(super::new_key);
            for side in ["before", "after"] {
                if let Some(body) = change.get_mut(side).and_then(Value::as_object_mut) {
                    body.insert("id".into(), id.clone().into());
                }
            }
            if change["before"].is_null() && !change["after"].is_null() {
                ids.remove(&key);
            } else {
                ids.insert(key, id);
            }
        }
        op.changes = changes.to_string();
    }
}

/// Form the logical state. Retain explicit entity/version/history tombstones rather than losing
/// a concurrent child's bytes when a parent disappears from the materialized view.
pub(super) fn value(stored: &Stored, ops: &[OpRow], previous: &Value, identity: Option<(String, String)>) -> Value {
    let mut boxes = tombstoned(previous, "boxes");
    for (index, held) in stored.written.boxes.iter().enumerate() {
        boxes.insert(held.id.clone(), json!({ "present": true, "name": held.name, "order": index, "fields": map_of(&held.fields), "field_order": keys_of(&held.fields) }));
    }
    let identities: BTreeMap<_, _> = stored
        .written
        .boxes
        .iter()
        .map(|held| (name_key(&held.name), held.id.clone()))
        .chain(stored.lock.assets.iter().map(|(key, held)| (key.clone(), held.id.clone())))
        .collect();
    let mut canvas = tombstoned(previous, "canvas");
    for (key, row) in stored.canvas.iter() {
        let id = identities
            .get(key)
            .cloned()
            .or_else(|| {
                canvas
                    .iter()
                    .find(|(_, record)| text(record, "key") == *key)
                    .map(|(id, _)| id.clone())
            })
            .unwrap_or_else(super::new_key);
        canvas.insert(id, json!({ "present": true, "key": key, "record": row }));
    }
    let mut history = tombstoned(previous, "history");
    for row in ops {
        // Display numbers can collide after an offline merge. Only the immutable global
        // history identity chooses the map entry; loading carries this key back into OpRow.
        let id = if row.uid.is_empty() { super::new_key() } else { row.uid.clone() };
        let mut record = encoded(row);
        record["uid"] = id.clone().into();
        history.insert(id, json!({ "present": true, "record": record }));
    }
    let (id, name) = identity.unwrap_or_else(|| (super::new_key(), String::new()));
    json!({ "schema": FORMAT, "identity": { "id": id, "name": name }, "head": map_of(&stored.written.head), "head_order": keys_of(&stored.written.head), "boxes": boxes, "canvas": canvas, "history": history })
}

/// Share untouched roots and keep the encoded value of each unchanged record.
fn changed_projection(before: &Stored, after: &Stored, ops: &[OpRow], previous: &Projection, identity: Option<(String, String)>) -> Projection {
    let mut next = previous.clone();
    if !Arc::ptr_eq(&before.written, &after.written) {
        if before.written.head != after.written.head {
            set_value(&mut next, "head", map_of(&after.written.head));
            set_value(&mut next, "head_order", keys_of(&after.written.head));
        }
        let old: BTreeMap<_, _> = before.written.boxes.iter().map(|held| (held.id.as_str(), held)).collect();
        let boxes = records_mut(&mut next, "boxes");
        retire_records(boxes, after.written.boxes.iter().map(|held| held.id.as_str()));
        for (index, held) in after.written.boxes.iter().enumerate() {
            let unchanged = old.get(held.id.as_str()).is_some_and(|old| *old == held)
                && boxes.get(&held.id).is_some_and(|row| present(row));
            if !unchanged {
                boxes.insert(held.id.clone(), Arc::new(json!({"present":true,"name":held.name,"order":index,"fields":map_of(&held.fields),"field_order":keys_of(&held.fields)})));
            }
        }
    }
    // Files, versions and runs are rows of project.db, never node state (cl-904, Paul 11-12): what a project from before held of
    // them here goes.
    for key in OUTSIDE {
        next.remove(key);
    }
    let identities = |stored: &Stored| -> BTreeMap<String, String> {
        stored.written.boxes.iter().map(|held| (name_key(&held.name), held.id.clone()))
            .chain(stored.lock.assets.iter().map(|(key, held)| (key.clone(), held.id.clone()))).collect()
    };
    let same_owners = || {
        let same_boxes = Arc::ptr_eq(&before.written, &after.written)
            || (before.written.boxes.len() == after.written.boxes.len()
                && before.written.boxes.iter().zip(&after.written.boxes)
                    .all(|(old, new)| old.id == new.id && old.name == new.name));
        let same_assets = Arc::ptr_eq(&before.lock, &after.lock)
            || (before.lock.assets.len() == after.lock.assets.len()
                && before.lock.assets.iter().zip(&after.lock.assets)
                    .all(|((old_key, old), (new_key, new))| old_key == new_key && old.id == new.id));
        same_boxes && same_assets
    };
    // Exact owner sequences imply the same normalized mapping. A rename or replacement
    // still needs the complete comparison even when the canvas Arc was retained.
    if !Arc::ptr_eq(&before.canvas, &after.canvas)
        || (!same_owners() && identities(before) != identities(after)) {
        let identities = identities(after);
        let canvas = records_mut(&mut next, "canvas");
        let mut live = BTreeSet::new();
        for (key, row) in after.canvas.iter() {
            let id = identities.get(key).cloned().or_else(|| canvas.iter().find(|(_, record)| text(record, "key") == *key).map(|(id, _)| id.clone())).unwrap_or_else(super::new_key);
            live.insert(id.clone());
            let record = encoded(row);
            if !canvas.get(&id).is_some_and(|value| present(value) && text(value, "key") == *key && value["record"] == record) {
                canvas.insert(id, Arc::new(json!({"present":true,"key":key,"record":record})));
            }
        }
        retire_records(canvas, live.iter().map(String::as_str));
    }
    let history = records_mut(&mut next, "history");
    retire_records(history, ops.iter().map(|row| row.uid.as_str()));
    for row in ops {
        let id = if row.uid.is_empty() { super::new_key() } else { row.uid.clone() };
        let mut record = encoded(row);
        record["uid"] = id.clone().into();
        if !history.get(&id).is_some_and(|old| present(old) && old["record"] == record) {
            history.insert(id, Arc::new(json!({"present":true,"record":record})));
        }
    }
    // A peer can leave equal display orders even when this edit changed another
    // branch. Check every live order and copy only records needing normalization.
    normalize_orders(&mut next, "boxes", after.written.boxes.iter().map(|held| &held.id));
    let (id, name) = identity.unwrap_or_else(|| (super::new_key(), String::new()));
    set_value(&mut next, "schema", FORMAT.into());
    set_value(&mut next, "identity", json!({"id":id,"name":name}));
    next
}

fn normalize_orders<'a>(projection: &mut Projection, key: &str, ids: impl Iterator<Item = &'a String>) {
    for (order, id) in ids.enumerate() {
        let value = root_records(projection, key).and_then(|rows| rows.get(id)).expect("current record");
        if value["order"].as_u64() != Some(order as u64) {
            let row = records_mut(projection, key).get_mut(id).expect("current record");
            Arc::make_mut(row)["order"] = json!(order);
        }
    }
}

fn retire_records<'a>(records: &mut Records, live: impl Iterator<Item = &'a str>) {
    let live: BTreeSet<_> = live.collect();
    for (_, row) in records.iter_mut().filter(|(id, row)| !live.contains(id.as_str()) && present(row)) {
        Arc::make_mut(row)["present"] = false.into();
    }
}

fn reconcile_projection(doc: &mut AutoCommit, before: &Projection, after: &Projection) -> Result<(), Error> {
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        doc.delete(ROOT, key.as_str()).map_err(invalid)?;
    }
    for (key, group) in after {
        match group {
            Group::Value(value) => {
                let previous = match before.get(key) {
                    Some(Group::Value(old)) => {
                        if Arc::ptr_eq(old, value) || old.as_ref() == value.as_ref() { continue; }
                        Some(old.as_ref())
                    }
                    _ => None,
                };
                put_changed(doc, &ROOT, Prop::Map(key.clone()), previous, value, false)?;
            }
            Group::Records(records) => {
                let old = match before.get(key) {
                    Some(Group::Records(old)) if Arc::ptr_eq(old, records) => continue,
                    Some(Group::Records(old)) => Some(old),
                    _ => None,
                };
                let object = match doc.get(ROOT, key.as_str()).map_err(invalid)? {
                    Some((AmValue::Object(ObjType::Map), object)) => object,
                    _ => {
                        // Keep an equal selected value in its original native shape.
                        if old.is_some_and(|old| old.as_ref() == records.as_ref()) { continue; }
                        let object = doc.put_object(ROOT, key.as_str(), ObjType::Map).map_err(invalid)?;
                        for (id, value) in records.iter() {
                            put_record(doc, key, &object, id, None, value)?;
                        }
                        continue;
                    }
                };
                if let Some(old) = old {
                    for id in old.keys().filter(|id| !records.contains_key(*id)) {
                        doc.delete(&object, id.as_str()).map_err(invalid)?;
                    }
                } else {
                    let keys: Vec<_> = doc.keys(&object).collect();
                    for id in keys.iter().filter(|id| !records.contains_key(*id)) {
                        doc.delete(&object, id.as_str()).map_err(invalid)?;
                    }
                }
                for (id, value) in records.iter() {
                    let previous = old.and_then(|rows| rows.get(id));
                    if previous.is_some_and(|row| Arc::ptr_eq(row, value) || row.as_ref() == value.as_ref()) {
                        continue;
                    }
                    put_record(doc, key, &object, id, previous.map(Arc::as_ref), value)?;
                }
            }
        }
    }
    Ok(())
}

/// A record of `namespace`. A box's words are Automerge text, each change to them a splice, so concurrent typing merges by character;
/// its other fields, and every other record, are values (cl-904).
fn put_record(doc: &mut AutoCommit, namespace: &str, records: &ObjId, id: &str, before: Option<&Value>, after: &Value) -> Result<(), Error> {
    let (true, Some(wanted)) = (namespace == "boxes", after.as_object()) else {
        return put_changed(doc, records, Prop::Map(id.to_owned()), before, after, false);
    };
    let held = match doc.get(records, id).map_err(invalid)? {
        Some((AmValue::Object(ObjType::Map), held)) => held,
        _ => doc.put_object(records, id, ObjType::Map).map_err(invalid)?,
    };
    let keys: Vec<String> = doc.keys(&held).collect();
    for key in keys.iter().filter(|key| !wanted.contains_key(*key)) {
        doc.delete(&held, key.as_str()).map_err(invalid)?;
    }
    for (key, value) in wanted {
        let old = before.and_then(|before| before.get(key));
        match (key.as_str(), value.as_object()) {
            ("fields", Some(fields)) => {
                let (object, reused) = match doc.get(&held, "fields").map_err(invalid)? {
                    Some((AmValue::Object(ObjType::Map), object)) => (object, true),
                    _ => (doc.put_object(&held, "fields", ObjType::Map).map_err(invalid)?, false),
                };
                let keys: Vec<String> = doc.keys(&object).collect();
                for field in keys.iter().filter(|field| !fields.contains_key(*field)) {
                    doc.delete(&object, field.as_str()).map_err(invalid)?;
                }
                for (field, value) in fields {
                    let old = old.filter(|_| reused).and_then(|old| old.get(field));
                    put_node(doc, &object, Prop::Map(field.clone()), old, value, false)?;
                }
            }
            _ => put(doc, &held, Prop::Map(key.clone()), old, value, false)?,
        }
    }
    Ok(())
}

/// A box's field, or what's in it: its words, at any depth, are Automerge text, each change to them the one splice that makes it; the maps
/// and lists holding them stay the same objects, so what's typed in one row merges with what's typed in another.
fn put_node(doc: &mut AutoCommit, parent: &ObjId, prop: Prop, before: Option<&Value>, after: &Value, insert: bool) -> Result<(), Error> {
    let found = if insert { None } else { doc.get(parent, prop.clone()).map_err(invalid)? };
    let made = |doc: &mut AutoCommit, kind: ObjType| -> Result<ObjId, Error> {
        match (&prop, insert) {
            (Prop::Seq(index), true) => doc.insert_object(parent, *index, kind),
            _ => doc.put_object(parent, prop.clone(), kind),
        }
        .map_err(invalid)
    };
    match after {
        Value::String(words) => {
            let Some((AmValue::Object(ObjType::Text), text)) = found else {
                let text = made(doc, ObjType::Text)?;
                return doc.splice_text(&text, 0, 0, words).map_err(invalid);
            };
            let held = doc.text(&text).map_err(invalid)?;
            if held == *words {
                return Ok(());
            }
            let (at, delete, insert) = splice_between(&held, words);
            doc.splice_text(&text, at, delete as isize, insert).map_err(invalid)
        }
        Value::Object(wanted) => {
            let (object, reused) = match found {
                Some((AmValue::Object(ObjType::Map), object)) => (object, true),
                _ => (made(doc, ObjType::Map)?, false),
            };
            let keys: Vec<String> = doc.keys(&object).collect();
            for key in keys.iter().filter(|key| !wanted.contains_key(*key)) {
                doc.delete(&object, key.as_str()).map_err(invalid)?;
            }
            for (key, value) in wanted {
                put_node(doc, &object, Prop::Map(key.clone()), before.filter(|_| reused).and_then(|before| before.get(key)), value, false)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            let (list, reused) = match found {
                Some((AmValue::Object(ObjType::List), list)) => (list, true),
                _ => (made(doc, ObjType::List)?, false),
            };
            let length = doc.length(&list);
            for (index, value) in items.iter().enumerate() {
                put_node(doc, &list, Prop::Seq(index), before.filter(|_| reused).and_then(|before| before.get(index)), value, index >= length)?;
            }
            for index in (items.len()..length).rev() {
                doc.delete(&list, index).map_err(invalid)?;
            }
            Ok(())
        }
        _ => put(doc, parent, prop, before.filter(|_| found.is_some()), after, insert),
    }
}

/// What turns `from` into `to`, in code points: where they first differ, how much of `from` goes, and what comes in its place.
fn splice_between<'a>(from: &str, to: &'a str) -> (usize, usize, &'a str) {
    let prefix = from.chars().zip(to.chars()).take_while(|(a, b)| a == b).count();
    let (from_rest, to_rest) = (from.chars().count() - prefix, to.chars().count() - prefix);
    let suffix = from.chars().rev().zip(to.chars().rev()).take(from_rest.min(to_rest)).take_while(|(a, b)| a == b).count();
    let start = to.char_indices().nth(prefix).map_or(to.len(), |(at, _)| at);
    let end = to.char_indices().nth(prefix + to_rest - suffix).map_or(to.len(), |(at, _)| at);
    (prefix, from_rest - suffix, &to[start..end])
}

/// What's at `path` in box `id`'s fields: a field, then `.`, and the keys and list indexes in it.
fn node_at(doc: &AutoCommit, id: &str, path: &str) -> Option<(AmValue<'static>, ObjId)> {
    let get = |parent: &ObjId, prop: Prop| doc.get(parent, prop).ok().flatten().map(|(value, id)| (value.to_owned(), id));
    let mut at = ROOT;
    for step in ["boxes", id, "fields"] {
        let Some((AmValue::Object(ObjType::Map), next)) = get(&at, Prop::Map(step.to_owned())) else { return None };
        at = next;
    }
    let mut found = None;
    for step in path.split('.') {
        if let Some((value, next)) = found.take() {
            let AmValue::Object(_) = value else { return None };
            at = next;
        }
        let prop = match doc.object_type(&at) {
            Ok(ObjType::List) => Prop::Seq(step.parse().ok()?),
            _ => Prop::Map(step.to_owned()),
        };
        found = Some(get(&at, prop)?);
    }
    found
}

/// The words at `path` of box `id`, where they're text.
fn text_of(doc: &AutoCommit, id: &str, path: &str) -> Result<Option<ObjId>, Error> {
    Ok(match node_at(doc, id, path) {
        Some((AmValue::Object(ObjType::Text), text)) => Some(text),
        _ => None,
    })
}

/// What the words at `path` of box `id` say, text or a plain value; nothing is "".
fn words_of(doc: &AutoCommit, id: &str, path: &str) -> Result<String, Error> {
    Ok(match node_at(doc, id, path) {
        Some((AmValue::Object(ObjType::Text), text)) => doc.text(&text).map_err(invalid)?,
        Some((value, _)) => value.into_string().unwrap_or_default(),
        None => String::new(),
    })
}

/// `splice` on `text`, held to what's there.
fn splice_in(doc: &mut AutoCommit, text: &ObjId, splice: &Splice) -> Result<(), Error> {
    let length = doc.length(text);
    let at = (splice.at as usize).min(length);
    let delete = (splice.delete as usize).min(length - at);
    doc.splice_text(text, at, delete as isize, &splice.insert).map_err(invalid)
}

fn reconcile_map(doc: &mut AutoCommit, object: &ObjId, before: &Value, after: &Value, reused: bool) -> Result<(), Error> {
    let Some(wanted) = after.as_object() else {
        return Err(invalid("logical project root is not a map"));
    };
    let old = before.as_object();
    if reused && let Some(old) = old {
        // The old JSON map is the selected native view of this very object. Sort only
        // removed keys to keep native map deletion order without cloning every key.
        let mut removed: Vec<_> = old.keys().filter(|key| !wanted.contains_key(*key)).collect();
        removed.sort_unstable();
        for key in removed {
            doc.delete(object, key.as_str()).map_err(invalid)?;
        }
        for (key, value) in wanted {
            let previous = old.get(key);
            if previous != Some(value) {
                put_changed(doc, object, Prop::Map(key.clone()), previous, value, false)?;
            }
        }
    } else {
        let keys: Vec<String> = doc.keys(object).collect();
        for key in keys.iter().filter(|key| !wanted.contains_key(*key)) {
            doc.delete(object, key.as_str()).map_err(invalid)?;
        }
        for (key, value) in wanted {
            put(doc, object, Prop::Map(key.clone()), old.and_then(|old| old.get(key)), value, false)?;
        }
    }
    Ok(())
}

fn put(doc: &mut AutoCommit, parent: &ObjId, prop: Prop, before: Option<&Value>, after: &Value, insert: bool) -> Result<(), Error> {
    if before == Some(after) && !insert {
        return Ok(());
    }
    put_changed(doc, parent, prop, before, after, insert)
}

fn put_changed(doc: &mut AutoCommit, parent: &ObjId, prop: Prop, before: Option<&Value>, after: &Value, insert: bool) -> Result<(), Error> {
    match after {
        Value::Object(_) | Value::Array(_) => {
            let kind = if after.is_object() { ObjType::Map } else { ObjType::List };
            let (held, reused) = if insert {
                let Prop::Seq(index) = prop else {
                    return Err(invalid("map insertion requested for a list"));
                };
                (doc.insert_object(parent, index, kind).map_err(invalid)?, false)
            } else {
                match doc.get(parent, prop.clone()).map_err(invalid)? {
                    Some((AmValue::Object(found), held)) if found == kind => (held, true),
                    _ => (doc.put_object(parent, prop.clone(), kind).map_err(invalid)?, false),
                }
            };
            let before = if reused { before } else { None };
            if after.is_object() {
                reconcile_map(doc, &held, before.unwrap_or(&Value::Null), after, reused)?;
            } else {
                let values = after.as_array().expect("array");
                let old = before.and_then(Value::as_array);
                let length = doc.length(&held);
                for (index, value) in values.iter().enumerate() {
                    put(
                        doc,
                        &held,
                        Prop::Seq(index),
                        old.and_then(|old| old.get(index)),
                        value,
                        index >= length,
                    )?;
                }
                for index in (values.len()..length).rev() {
                    doc.delete(&held, index).map_err(invalid)?;
                }
            }
        }
        _ => {
            let scalar = match after {
                Value::Null => ScalarValue::Null,
                Value::Bool(value) => ScalarValue::Boolean(*value),
                Value::String(value) => ScalarValue::Str(value.clone().into()),
                Value::Number(value) if value.is_u64() => ScalarValue::Uint(value.as_u64().expect("unsigned")),
                Value::Number(value) if value.is_i64() => ScalarValue::Int(value.as_i64().expect("signed")),
                Value::Number(value) => ScalarValue::F64(value.as_f64().expect("float")),
                _ => unreachable!("scalar"),
            };
            if insert {
                let Prop::Seq(index) = prop else {
                    return Err(invalid("map insertion requested for a scalar"));
                };
                doc.insert(parent, index, scalar).map_err(invalid)?;
            } else {
                doc.put(parent, prop, scalar).map_err(invalid)?;
            }
        }
    }
    Ok(())
}
