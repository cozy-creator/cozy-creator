//! The canonical logical project document. SQLite only commits its Automerge snapshot/change bytes
//! atomically with local execution bookkeeping and disposable query projections. Media stays outside.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, OnceLock},
};

use api::error::{Error, Kind};
use automerge::{
    ActorId, AutoCommit, AutoSerde, ObjId, ObjType, Prop, ROOT, ReadDoc, ScalarValue, Value as AmValue, transaction::Transactable,
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
    pub(super) value: Arc<Value>,
    conflicts: Arc<OnceLock<Vec<String>>>,
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

pub(super) fn identity_bytes(bytes: &[u8]) -> Option<(String, String)> {
    let doc = AutoCommit::load(bytes).ok()?;
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
    pub(super) fn load(bytes: &[u8], at: &Path) -> Result<Self, Error> {
        let doc = AutoCommit::load(bytes).map_err(invalid)?.with_actor(actor(at));
        let value = serde_json::to_value(AutoSerde::from(&doc)).map_err(invalid)?;
        if value.get("schema").and_then(Value::as_u64) != Some(FORMAT as u64) {
            return Err(invalid(
                "unsupported logical project schema; update cozy before opening this project",
            ));
        }
        Ok(Self {
            doc,
            value: Arc::new(value),
            conflicts: Arc::new(OnceLock::new()),
        })
    }

    pub(super) fn new(at: &Path) -> Self {
        Self {
            doc: AutoCommit::new().with_actor(actor(at)),
            value: Arc::new(json!({})),
            conflicts: Arc::new(OnceLock::new()),
        }
    }

    /// A narrow recursive diff, never one opaque serialized project string. Stable-ID maps keep
    /// independently added entities/versions separate, and scalar conflicts remain multi-value.
    pub(super) fn update(&mut self, value: Value) -> Result<(), Error> {
        let known_empty = self.conflicts.get().is_some_and(Vec::is_empty);
        reconcile_map(&mut self.doc, &ROOT, &self.value, &value)?;
        self.doc.commit();
        self.value = Arc::new(value);
        // A local observing mutation cannot introduce concurrency. Imported snapshots always
        // start with an empty cache and are inspected; existing alternatives invalidate it.
        let conflicts = OnceLock::new();
        if known_empty {
            let _ = conflicts.set(Vec::new());
        }
        self.conflicts = Arc::new(conflicts);
        Ok(())
    }

    /// Explicit conflict metadata, rather than silently presenting AutoSerde's selected winner.
    pub(super) fn conflicts(&self) -> Vec<String> {
        self.conflicts
            .get_or_init(|| {
                let mut found = Vec::new();
                self.conflicts_under(&ROOT, "", &mut found);
                found
            })
            .clone()
    }

    fn conflicts_under(&self, object: &ObjId, path: &str, found: &mut Vec<String>) {
        let properties: Vec<Prop> = match self.doc.object_type(object) {
            Ok(ObjType::Map) => self.doc.keys(object).map(Prop::Map).collect(),
            Ok(ObjType::List) => (0..self.doc.length(object)).map(Prop::Seq).collect(),
            _ => return,
        };
        for property in properties {
            let label = match &property {
                Prop::Map(key) => key.clone(),
                Prop::Seq(index) => index.to_string(),
            };
            let at = if path.is_empty() { label } else { format!("{path}/{label}") };
            let Ok(values) = self.doc.get_all(object, property) else { continue };
            if values.len() > 1 {
                let alternatives = values
                    .iter()
                    .map(|(value, operation)| {
                        let value = value.as_str().map(str::to_owned).unwrap_or_else(|| format!("{value:?}"));
                        format!("{operation}: {value}")
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                found.push(format!(
                    "Concurrent project values at {at}: {alternatives}. The alternatives are retained for explicit resolution."
                ));
            }
            for (value, child) in values {
                if matches!(value, AmValue::Object(_)) {
                    self.conflicts_under(&child, &at, found);
                }
            }
        }
    }

    pub(super) fn identity(&self) -> Option<(String, String)> {
        let identity = self.value.get("identity")?;
        Some((
            identity.get("id")?.as_str()?.to_owned(),
            identity.get("name")?.as_str().unwrap_or_default().to_owned(),
        ))
    }

    pub(super) fn set_identity(&mut self, id: &str, name: &str) -> Result<(), Error> {
        let mut value = (*self.value).clone();
        value["identity"] = json!({ "id": id, "name": name });
        self.update(value)
    }

    pub(super) fn ops(&self) -> Result<Vec<OpRow>, Error> {
        let mut rows = Vec::new();
        for (uid, record) in objects(&self.value, "history").iter().filter(|(_, record)| present(record)) {
            let mut row = decode::<OpRow>(record.get("record").cloned().unwrap_or_default())?;
            row.uid.clone_from(uid);
            rows.push(row);
        }
        rows.sort_by(|a, b| (&a.id, &a.uid).cmp(&(&b.id, &b.uid)));
        Ok(rows)
    }

    pub(super) fn stored(&self) -> Result<Stored, Error> {
        let mut written = Written::default();
        let head = objects(&self.value, "head");
        for key in order(&self.value, "head_order", &head) {
            written.head.push((key.clone(), head[&key].clone()));
        }
        let mut boxes: Vec<(u64, String, Boxed)> = Vec::new();
        for (id, record) in objects(&self.value, "boxes").iter().filter(|(_, record)| present(record)) {
            let fields = objects(record, "fields");
            let ordered = order(record, "field_order", &fields)
                .into_iter()
                .map(|key| (key.clone(), fields[&key].clone()))
                .collect();
            boxes.push((
                record["order"].as_u64().unwrap_or_default(),
                id.clone(),
                Boxed {
                    id: id.clone(),
                    name: text(record, "name"),
                    fields: ordered,
                    at: None,
                },
            ));
        }
        boxes.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        written.boxes = boxes.into_iter().map(|(_, _, held)| held).collect();
        let mut lock = Lock::default();
        for (id, record) in objects(&self.value, "assets").iter().filter(|(_, record)| present(record)) {
            let mut held: Asset = decode(record["record"].clone())?;
            held.id.clone_from(id);
            for (version_id, version) in objects(record, "versions").iter().filter(|(_, record)| present(record)) {
                let mut value: Version = decode(version["record"].clone())?;
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
        lock.files = objects(&self.value, "files")
            .into_iter()
            .map(|(hash, value)| decode::<File>(value).map(|file| (hash, File { row: true, ..file })))
            .collect::<Result<_, _>>()?;
        lock.reserved = decode(self.value.get("reserved").cloned().unwrap_or_else(|| json!({})))?;
        lock.makes = decode(self.value.get("makes").cloned().unwrap_or_else(|| json!({})))?;
        let mut runs: Vec<(u64, String, RunFile)> = Vec::new();
        for (id, record) in objects(&self.value, "runs").into_iter().filter(|(_, record)| present(record)) {
            let mut run: RunFile = decode(record["record"].clone())?;
            run.id.clone_from(&id);
            runs.push((record["order"].as_u64().unwrap_or_default(), id, run));
        }
        runs.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        lock.runs = runs.into_iter().map(|(_, _, run)| run).collect();
        let mut canvas = BTreeMap::new();
        for record in objects(&self.value, "canvas").into_values().filter(present) {
            let row: CardRow = decode(record["record"].clone())?;
            canvas.insert(text(&record, "key"), row);
        }
        Ok(Stored {
            written: Arc::new(written),
            lock: Arc::new(lock),
            canvas: Arc::new(canvas),
            crdt: Some(Arc::new(self.clone())),
            ..Default::default()
        })
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(invalid)
}
fn encoded<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a logical project value serializes")
}
fn objects(value: &Value, key: &str) -> Map<String, Value> {
    value.get(key).and_then(Value::as_object).cloned().unwrap_or_default()
}
fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}
fn present(value: &Value) -> bool {
    value.get("present").and_then(Value::as_bool).unwrap_or(true)
}
fn order(value: &Value, key: &str, values: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| values.contains_key(*key))
        .map(str::to_owned)
        .collect();
    let listed: BTreeSet<_> = keys.iter().cloned().collect();
    keys.extend(values.keys().filter(|key| !listed.contains(*key)).cloned());
    keys
}
fn map_of(values: &[(String, Value)]) -> Value {
    Value::Object(values.iter().cloned().collect())
}
fn keys_of(values: &[(String, Value)]) -> Value {
    encoded(&values.iter().map(|(key, _)| key).collect::<Vec<_>>())
}
fn tombstoned(previous: &Value, field: &str) -> Map<String, Value> {
    let mut all = objects(previous, field);
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
    let mut assets = tombstoned(previous, "assets");
    for (key, held) in &stored.lock.assets {
        let mut versions = tombstoned(assets.get(&held.id).unwrap_or(&Value::Null), "versions");
        for (discarded, version) in held
            .versions
            .iter()
            .map(|version| (false, version))
            .chain(held.discarded.iter().map(|version| (true, version)))
        {
            versions.insert(
                version.id.clone(),
                json!({ "present": true, "discarded": discarded, "record": version }),
            );
        }
        let metadata = Asset {
            versions: vec![],
            discarded: vec![],
            ..held.clone()
        };
        assets.insert(
            held.id.clone(),
            json!({ "present": true, "key": key, "record": metadata, "versions": versions }),
        );
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
    let mut runs = tombstoned(previous, "runs");
    for (index, row) in stored.lock.runs.iter().enumerate() {
        runs.insert(row.id.clone(), json!({ "present": true, "order": index, "record": row }));
    }
    let files: Map<String, Value> = stored
        .lock
        .files
        .iter()
        .filter(|(_, file)| file.row)
        .map(|(hash, file)| (hash.clone(), encoded(file)))
        .collect();
    let (id, name) = identity.unwrap_or_else(|| (super::new_key(), String::new()));
    json!({ "schema": FORMAT, "identity": { "id": id, "name": name }, "head": map_of(&stored.written.head), "head_order": keys_of(&stored.written.head), "boxes": boxes, "assets": assets, "files": files, "canvas": canvas, "history": history, "runs": runs, "reserved": stored.lock.reserved, "makes": stored.lock.makes })
}

fn reconcile_map(doc: &mut AutoCommit, object: &ObjId, before: &Value, after: &Value) -> Result<(), Error> {
    let Some(wanted) = after.as_object() else {
        return Err(invalid("logical project root is not a map"));
    };
    let old = before.as_object();
    let keys: Vec<String> = doc.keys(object).collect();
    for key in keys.iter().filter(|key| !wanted.contains_key(*key)) {
        doc.delete(object, key.as_str()).map_err(invalid)?;
    }
    for (key, value) in wanted {
        put(doc, object, Prop::Map(key.clone()), old.and_then(|old| old.get(key)), value, false)?;
    }
    Ok(())
}

fn put(doc: &mut AutoCommit, parent: &ObjId, prop: Prop, before: Option<&Value>, after: &Value, insert: bool) -> Result<(), Error> {
    if before == Some(after) && !insert {
        return Ok(());
    }
    match after {
        Value::Object(_) | Value::Array(_) => {
            let kind = if after.is_object() { ObjType::Map } else { ObjType::List };
            let held = if insert {
                let Prop::Seq(index) = prop else {
                    return Err(invalid("map insertion requested for a list"));
                };
                doc.insert_object(parent, index, kind).map_err(invalid)?
            } else {
                match doc.get(parent, prop.clone()).map_err(invalid)? {
                    Some((AmValue::Object(found), held)) if found == kind => held,
                    _ => doc.put_object(parent, prop.clone(), kind).map_err(invalid)?,
                }
            };
            if after.is_object() {
                reconcile_map(doc, &held, before.unwrap_or(&Value::Null), after)?;
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
