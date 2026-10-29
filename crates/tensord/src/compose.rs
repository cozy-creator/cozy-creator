//! A model composed of other models' components (`cozy model upload … --component NAME=MODEL`):
//! the base's components in its order, each one a part names taking that part's tensors under
//! the base's class, config and assets for it; a name the base lacks is the part's component
//! whole. Nothing is computed: large parts keep their objects, so one part under several names is
//! stored, and uploaded, once. A composition is kept like an import, as `local/compose-<id>`.
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use tensorfs::format::{Component, ObjectRef, Provenance, Value};
use tensorfs::import::{FILE_METADATA, SOURCE};
use tensorfs::store::{Checkpoint, Store};

use crate::storage;

/// One component of the composition and the checkpoint that supplies it.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub checkpoint: ObjectRef,
}

/// The composition, kept: its local name, header and commit (with the base's repo files).
#[derive(Clone, Debug)]
pub struct Composed {
    pub name: String,
    pub header: ObjectRef,
    pub tree: ObjectRef,
}

fn refuse(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, detail.into())
}

/// `part`'s component for `name`: the one so named, else its only one with tensors.
fn supplied<'c>(part: &'c Checkpoint, name: &str) -> io::Result<&'c Component> {
    if let Some(named) = part.header().component(name) {
        return Ok(named);
    }
    let weighted: Vec<&Component> = part.components().iter().filter(|c| !c.tensors.is_empty()).collect();
    match weighted.as_slice() {
        [one] => Ok(one),
        _ => Err(refuse(format!(
            "{} has no component {name} and {} with tensors ({}); name one of them",
            part.id().id(),
            weighted.len(),
            weighted.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "),
        ))),
    }
}

/// A metadata value as its list of values.
fn listed(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::List(values)) => values.clone(),
        Some(value) => vec![value.clone()],
        None => vec![],
    }
}

/// `base` with `parts`, kept as `local/compose-<id>` with the base's repo `files`: the same
/// inputs make the same header, and one already kept is reused.
pub fn compose(store: &Store, base: &ObjectRef, parts: &[Part], files: &BTreeMap<String, ObjectRef>) -> io::Result<Composed> {
    let rows: Vec<_> = parts.iter().map(|p| serde_json::json!([p.name, p.checkpoint.id()])).collect();
    let identity = serde_json_canonicalizer::to_vec(&serde_json::json!(["cozy.machine-compose/1", base.id(), rows, files]))
        .map_err(io::Error::other)?;
    let name = format!("compose-{}", &tensorfs::sha256::hex_digest(&identity)[..40]);
    if let Some(kept) = storage::local(store, &format!("local/{name}"))?.filter(|kept| store.holds(&kept.header)) {
        return Ok(Composed { name: format!("local/{name}"), header: kept.header, tree: kept.tree });
    }
    let open = |header: &ObjectRef| store.checkpoint(header).map_err(storage::io_error);
    let base = open(base)?;
    let held = parts.iter().map(|p| open(&p.checkpoint)).collect::<io::Result<Vec<_>>>()?;
    let mut chosen: BTreeMap<&str, (&Checkpoint, &Component)> = BTreeMap::new();
    for (part, checkpoint) in parts.iter().zip(&held) {
        if chosen.insert(&part.name, (checkpoint, supplied(checkpoint, &part.name)?)).is_some() {
            return Err(refuse(format!("two parts supply {}", part.name)));
        }
    }
    let mut writer = store.writer(None, Some(&name)).map_err(storage::io_error)?;
    if let Some(pipeline) = base.pipeline() {
        writer.pipeline(pipeline.class.clone(), pipeline.config.clone());
    }
    // (component, key, source checkpoint, source component) in header order: the base's
    // components, then the added ones.
    let mut takes: Vec<(&str, &str, &Checkpoint, &str)> = vec![];
    let added = chosen.iter().filter(|(name, _)| base.header().component(name).is_none());
    let declared = base.components().iter().map(|c| (c.name.as_str(), c)).chain(added.map(|(name, (_, c))| (*name, *c)));
    for (name, own) in declared {
        let mut component = writer.component(name, own.class.clone(), own.config.clone(), own.config_file.as_deref())
            .map_err(storage::io_error)?;
        for (asset, object) in &own.assets {
            component.reuse_asset(asset, object).map_err(storage::io_error)?;
        }
        let (from, tensors) = match chosen.get(name) {
            Some((checkpoint, supplied)) => (*checkpoint, *supplied),
            None => (&base, own),
        };
        takes.extend(tensors.tensors.iter().map(|t| (name, t.key.as_str(), from, tensors.name.as_str())));
    }
    let order: Vec<(&str, &str)> = takes.iter().map(|t| (t.0, t.1)).collect();
    writer.set_order(&order);
    for (component, key, from, from_component) in &takes {
        writer.get(component).copy_as(key, from, from_component, key).map_err(storage::io_error)?;
    }
    // The base's metadata, but its lane recipe's; each part's sources, and its file's own
    // metadata under the name it now has.
    let mut metadata = base.header().metadata.clone();
    metadata.remove(Provenance::KEY);
    let mut sources = listed(metadata.get(SOURCE));
    let mut files_metadata = match metadata.remove(FILE_METADATA) {
        Some(Value::Map(map)) => map,
        _ => BTreeMap::new(),
    };
    for (name, (checkpoint, supplied)) in &chosen {
        let theirs = &checkpoint.header().metadata;
        for source in listed(theirs.get(SOURCE)) {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        files_metadata.remove(*name);
        if let Some(Value::Map(map)) = theirs.get(FILE_METADATA) {
            if let Some(own) = map.get(&supplied.name) {
                files_metadata.insert(name.to_string(), own.clone());
            }
        }
    }
    match sources.len() {
        0 => drop(metadata.remove(SOURCE)),
        1 => drop(metadata.insert(SOURCE.into(), sources.remove(0))),
        _ => drop(metadata.insert(SOURCE.into(), Value::List(sources))),
    }
    if !files_metadata.is_empty() {
        metadata.insert(FILE_METADATA.into(), Value::Map(files_metadata));
    }
    for (key, value) in metadata {
        writer.set_metadata(&key, value);
    }
    let header = writer.commit().map_err(storage::io_error)?;
    let tree = storage::set_local(store, &name, &header, files, "")?;
    Ok(Composed { name: format!("local/{name}"), header, tree })
}

/// A checkpoint's bytes counted per component, and each object once: what one copy under
/// several names saves.
pub fn bytes(store: &Store, header: &ObjectRef) -> io::Result<(u64, u64)> {
    let checkpoint = store.checkpoint(header).map_err(storage::io_error)?;
    let (mut logical, mut unique) = (0, BTreeSet::new());
    for component in checkpoint.components() {
        for object in storage::chunks(&checkpoint, std::slice::from_ref(&component.name)) {
            logical += object.length;
            unique.insert(object);
        }
    }
    Ok((logical, unique.iter().map(|o| o.length).sum()))
}
