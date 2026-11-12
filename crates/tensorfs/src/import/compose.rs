//! Headers composed by reference. No byte is read: chunks are re-indexed. Each source's chunks
//! that a kept range touches stay in their order (a range continues into the next chunk), others
//! drop, and a header that supplies twice is listed once.
//!
//! - [`compose`] (`cozy model upload --base`): a base header whose components are replaced, or
//!   added, by other headers' components. Every entry it keeps is the base's verbatim (class,
//!   config, assets, tensors, the same chunk objects); a replaced component takes the supplier's
//!   tensors under the base's class, config and assets.
//! - [`inherit`] (a checkpoint resolved for a run): a checkpoint with what it lacks of its base,
//!   whole components and the files of components both have. Its own data always wins.
//!
//! Its `family` is re-verified for the components it holds: each is what the record of the header
//! it came from says.

use std::collections::{BTreeMap, BTreeSet};

use crate::format::{invalid, Body, Component, Header, Metadata, ObjectRef, Result, Value};
use crate::family::{self, Record};

/// `component` of `header` becomes the composition's `name`.
pub(crate) struct Supply<'h> {
    pub name: String,
    pub header: &'h Header,
    pub component: String,
}

/// One component of an assembled header: its declaration (name, class, config, assets) and the
/// component of `sources[source]` whose tensors it takes.
struct Taken<'h> {
    declared: Component,
    source: usize,
    from: &'h Component,
}

/// The chunks and components of `taken`, its tensors re-indexed into one chunk list.
fn assemble(sources: &[&Header], taken: Vec<Taken<'_>>) -> (Vec<ObjectRef>, Vec<Component>) {
    let mut touched = vec![BTreeSet::new(); sources.len()];
    for t in &taken {
        for (_, at) in t.from.tensors.iter().flat_map(|t| t.locations()) {
            touched[t.source].extend(sources[t.source].spans(&at).into_iter().map(|s| s.chunk));
        }
    }
    let (mut chunks, mut index) = (Vec::new(), vec![BTreeMap::new(); sources.len()]);
    for (source, used) in touched.iter().enumerate() {
        for chunk in used {
            index[source].insert(*chunk as u64, chunks.len() as u64);
            chunks.push(sources[source].chunks[*chunk].clone());
        }
    }
    let components = taken
        .into_iter()
        .map(|Taken { declared, source, from }| {
            let mut component = Component { tensors: from.tensors.clone(), ..declared };
            // An empty range touches no chunk; it keeps any index.
            let moved = |chunk: u64| index[source].get(&chunk).copied().unwrap_or_default();
            for tensor in &mut component.tensors {
                match &mut tensor.body {
                    Body::Data(at) => at.chunk = moved(at.chunk),
                    Body::Encoded { parts, .. } => parts.iter_mut().for_each(|p| p.chunk = moved(p.chunk)),
                }
            }
            component
        })
        .collect();
    (chunks, components)
}

/// `base` with `supplies`, carrying `metadata`.
pub(crate) fn compose(base: &Header, supplies: &[Supply<'_>], mut metadata: Metadata) -> Result<Header> {
    // Sources: the base, then each distinct supplying header.
    let mut sources: Vec<&Header> = vec![base];
    let mut supplied: BTreeMap<&str, (usize, &Component)> = BTreeMap::new();
    for s in supplies {
        let source = match sources.iter().position(|h| std::ptr::eq(*h, s.header)) {
            Some(i) => i,
            None => {
                sources.push(s.header);
                sources.len() - 1
            }
        };
        let Some(component) = s.header.component(&s.component) else {
            return invalid(format!("the supplying header has no component {:?}", s.component));
        };
        if supplied.insert(&s.name, (source, component)).is_some() {
            return invalid(format!("component {:?} is supplied twice", s.name));
        }
    }
    // The base's order, then the added ones.
    let mut taken: Vec<Taken> = Vec::new();
    for own in base.components() {
        taken.push(match supplied.remove(own.name.as_str()) {
            Some((source, from)) => Taken { declared: own.clone(), source, from },
            None => Taken { declared: own.clone(), source: 0, from: own },
        });
    }
    taken.extend(supplied.into_iter().map(|(name, (source, from))| Taken {
        declared: Component { name: name.to_owned(), ..from.clone() },
        source,
        from,
    }));
    let (chunks, components) = assemble(&sources, taken);
    let own = Record::read(&base.metadata);
    match own.clone().or_else(|| supplies.iter().find_map(|s| Record::read(&s.header.metadata))) {
        Some(mut record) => {
            record.components = (components.iter())
                .map(|c| {
                    let is = match supplies.iter().find(|s| s.name == c.name) {
                        Some(s) => Record::read(&s.header.metadata).map_or(s.component.clone(), |r| r.component(&s.component).to_owned()),
                        None => own.as_ref().map_or(c.name.clone(), |r| r.component(&c.name).to_owned()),
                    };
                    (c.name.clone(), is)
                })
                .collect();
            metadata.insert(family::FAMILY.into(), record.value());
        }
        None => {
            metadata.remove(family::FAMILY);
        }
    }
    family::reverify(&mut metadata, &components);
    Header::new(chunks, base.pipeline().cloned(), components, metadata)
}

/// Header metadata: what a checkpoint took from its bases at resolve ([`inherit`]):
/// `{checkpoint: {sha256, length}, from: [{base: {...}, components: [...], files: [...]}]}`.
pub const INHERITED: &str = "inherited";

/// What one base gave: whole components, and other components' files (`text_encoder/processor/
/// tokenizer.json`, `vae/config.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inherited {
    pub components: Vec<String>,
    pub files: Vec<String>,
}

impl Inherited {
    pub fn is_empty(&self) -> bool {
        self.components.is_empty() && self.files.is_empty()
    }
}

/// The checkpoint an inherited header was made from, as its [`INHERITED`] record names it.
pub fn inherited_from(header: &Header) -> Option<ObjectRef> {
    let Some(Value::Map(record)) = header.metadata.get(INHERITED) else { return None };
    let Some(Value::Map(checkpoint)) = record.get("checkpoint") else { return None };
    Some(ObjectRef { sha256: checkpoint.get("sha256")?.as_str()?.to_owned(), length: checkpoint.get("length")?.as_u64()? })
}

/// Whether two components are one model's: the same class (library and name; versions differ).
fn same_class(a: &Component, b: &Component) -> bool {
    (&a.class.library, &a.class.name) == (&b.class.library, &b.class.name)
}

/// `child` (whose own header is `checkpoint`) with what it lacks of `base`, recorded as `from`
/// (the base's model, release, lane, header): each component of the base it has not, whole, and
/// for each component both have, each of the base's assets it has not and the base's config when
/// its own is empty. Its own data always wins. None when it lacks nothing of the base, or when
/// they are not one model: no component in common, or one in common of another class.
pub fn inherit(child: &Header, checkpoint: &ObjectRef, base: &Header, from: BTreeMap<String, Value>) -> Result<Option<(Header, Inherited)>> {
    let shared: Vec<(&Component, &Component)> =
        child.components().iter().filter_map(|c| Some((c, base.component(&c.name)?))).collect();
    if shared.is_empty() || shared.iter().any(|(c, b)| !same_class(c, b)) {
        return Ok(None);
    }
    let mut gave = Inherited::default();
    let mut taken: Vec<Taken> = Vec::new();
    for own in child.components() {
        let mut declared = own.clone();
        let (mut source, mut from) = (0, own);
        if let Some(theirs) = base.component(&own.name) {
            // Declared with no tensors where the base has them: the component is the base's.
            if own.tensors.is_empty() && !theirs.tensors.is_empty() {
                (source, from) = (1, theirs);
                gave.components.push(own.name.clone());
            }
            if own.config.is_empty() && !theirs.config.is_empty() && !own.assets.contains_key(theirs.config_name()) {
                declared.config = theirs.config.clone();
                declared.config_file = theirs.config_file.clone();
                gave.files.push(format!("{}/{}", own.name, theirs.config_name()));
            }
            for (name, object) in &theirs.assets {
                if !own.assets.contains_key(name) && declared.config_file.as_deref() != Some(name.as_str()) {
                    declared.assets.insert(name.clone(), object.clone());
                    gave.files.push(format!("{}/{name}", own.name));
                }
            }
        }
        taken.push(Taken { declared, source, from });
    }
    for theirs in base.components().iter().filter(|b| child.component(&b.name).is_none()) {
        taken.push(Taken { declared: theirs.clone(), source: 1, from: theirs });
        gave.components.push(theirs.name.clone());
    }
    if gave.is_empty() {
        return Ok(None);
    }
    let (chunks, components) = assemble(&[child, base], taken);
    let mut metadata = child.metadata.clone();
    let mine = Record::read(&child.metadata);
    let theirs = Record::read(&base.metadata);
    if let Some(mut record) = mine.clone().or_else(|| theirs.clone()) {
        record.components = (components.iter())
            .map(|c| {
                let record = if child.component(&c.name).is_some() { mine.as_ref() } else { theirs.as_ref() };
                (c.name.clone(), record.map_or(c.name.clone(), |r| r.component(&c.name).to_owned()))
            })
            .collect();
        metadata.insert(family::FAMILY.into(), record.value());
    }
    family::reverify(&mut metadata, &components);
    let text = |s: &str| Value::Str(s.to_owned());
    let mut record = match metadata.remove(INHERITED) {
        Some(Value::Map(record)) => record,
        _ => BTreeMap::from([(
            "checkpoint".to_owned(),
            Value::Map(BTreeMap::from([
                ("sha256".to_owned(), text(&checkpoint.sha256)),
                ("length".to_owned(), Value::Int(checkpoint.length.into())),
            ])),
        )]),
    };
    let mut bases = match record.remove("from") {
        Some(Value::List(bases)) => bases,
        _ => vec![],
    };
    bases.push(Value::Map(BTreeMap::from([
        ("base".to_owned(), Value::Map(from)),
        ("components".to_owned(), Value::List(gave.components.iter().map(|c| text(c)).collect())),
        ("files".to_owned(), Value::List(gave.files.iter().map(|f| text(f)).collect())),
    ])));
    record.insert("from".into(), Value::List(bases));
    metadata.insert(INHERITED.into(), Value::Map(record));
    let pipeline = child.pipeline().or(base.pipeline()).cloned();
    Ok(Some((Header::new(chunks, pipeline, components, metadata)?, gave)))
}
