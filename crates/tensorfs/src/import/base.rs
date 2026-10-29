//! A model composed by reference on another's header (`cozy model upload <source> <org/name>
//! --base <model>[:<component>,…]`): which of the source's components go where, the header
//! metadata that records it, and the header itself (`format::compose`). Only the base's header
//! is read: the components it keeps stay its objects, wherever they are held.
use std::collections::BTreeMap;

use crate::err::{refuse, Code, Result};
use crate::format::compose::{compose, Supply};
use crate::format::{Header, ObjectRef, Provenance, Value};
use crate::store::Store;

use super::{FILE_METADATA, SOURCE};

/// Header metadata: the base a composition started from and each component's source.
pub const COMPOSED: &str = "composed";

/// The base a composition started from, as its header records it (empty fields are left out).
#[derive(Clone, Debug, Default)]
pub struct Base {
    pub model: String,
    pub release: String,
    pub lane: String,
    pub commit: String,
    pub header: String,
}

/// The composed header, stored, and the source each of its supplied components came from.
#[derive(Clone, Debug)]
pub struct Composed {
    pub header: ObjectRef,
    pub components: BTreeMap<String, String>,
}

fn read(store: &Store, header: &ObjectRef) -> Result<Header> {
    let bytes = store.read(header).map_err(crate::store::Error::refusal)?;
    Header::from_cbor(&bytes).or_else(|e| refuse(Code::UNKNOWN_FORMAT, format!("{}: {e}", header.id())))
}

/// A metadata value as its list of values.
fn listed(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::List(values)) => values.clone(),
        Some(value) => vec![value.clone()],
        None => vec![],
    }
}

/// `base` with `source`'s components, written to `store`: with `names`, each that the source
/// has (with tensors) from it by that name, or else its one component with tensors under every
/// name; with none, each of its components with tensors in place of the base's of that name. The header keeps the base's metadata but its lane recipe's, adds the
/// source's pinned sources, its file metadata under each new name, and [`COMPOSED`].
pub fn on_base(store: &Store, base: &ObjectRef, source: &ObjectRef, names: &[String], record: &Base) -> Result<Composed> {
    let (on, from) = (read(store, base)?, read(store, source)?);
    let weighted: Vec<&str> = (from.components().iter()).filter(|c| !c.tensors.is_empty()).map(|c| c.name.as_str()).collect();
    let has: Vec<&str> = on.components().iter().map(|c| c.name.as_str()).collect();
    let pairs: Vec<(String, &str)> = match (names, weighted.as_slice()) {
        ([], _) => match weighted.iter().find(|c| !has.contains(c)) {
            Some(c) => return refuse(Code::MISSING_FIELD, format!(
                "the source's {c} is not one of the base's components ({}); name where it goes: --base <model>:<component>",
                has.join(", "))),
            None => weighted.iter().map(|c| (c.to_string(), *c)).collect(),
        },
        (_, _) if names.iter().all(|n| weighted.contains(&n.as_str())) => names.iter().map(|n| (n.clone(), n.as_str())).collect(),
        (_, [one]) => names.iter().map(|name| (name.clone(), *one)).collect(),
        (_, many) => return refuse(Code::AMBIGUOUS_CLASSIFICATION, format!(
            "the source has {} components with tensors ({}): list only those, or take a source with one",
            many.len(), many.join(", "))),
    };
    let supplies: Vec<Supply> = (pairs.iter())
        .map(|(name, component)| Supply { name: name.clone(), header: &from, component: component.to_string() })
        .collect();
    let mut metadata = on.metadata.clone();
    metadata.remove(Provenance::KEY);
    let mut sources = listed(metadata.get(SOURCE));
    let theirs = listed(from.metadata.get(SOURCE));
    for source in &theirs {
        if !sources.contains(source) {
            sources.push(source.clone());
        }
    }
    if !sources.is_empty() {
        metadata.insert(SOURCE.into(), if sources.len() == 1 { sources.remove(0) } else { Value::List(sources) });
    }
    let mut files = match metadata.remove(FILE_METADATA) {
        Some(Value::Map(map)) => map,
        _ => BTreeMap::new(),
    };
    let own = match from.metadata.get(FILE_METADATA) {
        Some(Value::Map(map)) => map.clone(),
        _ => BTreeMap::new(),
    };
    let pinned = theirs.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ");
    let mut components = BTreeMap::new();
    for (name, component) in &pairs {
        files.remove(name);
        if let Some(entry) = own.get(*component) {
            files.insert(name.clone(), entry.clone());
        }
        components.insert(name.clone(), pinned.clone());
    }
    if !files.is_empty() {
        metadata.insert(FILE_METADATA.into(), Value::Map(files));
    }
    let text = |s: &str| Value::Str(s.to_owned());
    let fields = [("model", &record.model), ("release", &record.release), ("lane", &record.lane),
        ("commit", &record.commit), ("header", &record.header)];
    metadata.insert(COMPOSED.into(), Value::Map(BTreeMap::from([
        ("base".to_owned(), Value::Map(fields.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.to_string(), text(v))).collect())),
        ("components".to_owned(), Value::Map(components.iter().map(|(k, v)| (k.clone(), text(v))).collect())),
    ])));
    let header = compose(&on, &supplies, metadata).or_else(|e| refuse(Code::UNKNOWN_FORMAT, e.to_string()))?;
    let bytes = header.to_cbor().or_else(|e| refuse(Code::UNKNOWN_FORMAT, e.to_string()))?;
    let header = store.put_bytes(&bytes).map_err(crate::store::Error::refusal)?;
    Ok(Composed { header, components })
}
