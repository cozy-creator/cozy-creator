//! Headers composed by reference. No byte is read: chunks are re-indexed. Each source's chunks
//! that a kept range touches stay in their order (a range continues into the next chunk), others
//! drop, and a header that supplies twice is listed once.
//!
//! - [`compose`] (`cozy model upload <source> --base`, a converted source): a base header whose
//!   components are replaced, or added, by other headers' components. Every entry it keeps is the
//!   base's verbatim (class, config, assets, tensors, the same chunk objects); a replaced component
//!   takes the supplier's tensors under the base's class, config and assets.
//! - [`inherit`] (`cozy model upload <org/name@…> --base`, a Hub checkpoint): a checkpoint made
//!   complete from its base: whole components it lacks, and the files of components both have.
//!   Its own data always wins.
//!
//! Its `family` is re-verified for the components it holds: each is what the record of the header
//! it came from says.

use std::collections::{BTreeMap, BTreeSet};

use crate::format::{invalid, Body, Component, Header, Metadata, ObjectRef, Result, Tensor, Value};
use crate::family::{self, Record};

/// `component` of `header` becomes the composition's `name`, with the base's tensors of `fill`, the
/// keys its source kept in another file.
pub(crate) struct Supply<'h> {
    pub name: String,
    pub header: &'h Header,
    pub component: String,
    pub fill: Vec<String>,
}

/// One component of an assembled header: its declaration (name, class, config, assets) and each of
/// its tensors, with the index of the source header whose chunks hold it.
struct Taken<'h> {
    declared: Component,
    tensors: Vec<(usize, &'h Tensor)>,
}

impl<'h> Taken<'h> {
    /// `declared` with every tensor of `from`, a component of `sources[source]`.
    fn whole(declared: Component, source: usize, from: &'h Component) -> Taken<'h> {
        Taken { declared, tensors: from.tensors.iter().map(|t| (source, t)).collect() }
    }
}

/// The chunks and components of `taken`, its tensors re-indexed into one chunk list.
fn assemble(sources: &[&Header], taken: Vec<Taken<'_>>) -> (Vec<ObjectRef>, Vec<Component>) {
    let mut touched = vec![BTreeSet::new(); sources.len()];
    for t in &taken {
        for (source, tensor) in &t.tensors {
            for (_, at) in tensor.locations() {
                touched[*source].extend(sources[*source].spans(&at).into_iter().map(|s| s.chunk));
            }
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
        .map(|Taken { declared, tensors }| {
            let tensors = (tensors.into_iter())
                .map(|(source, tensor)| {
                    let mut tensor = tensor.clone();
                    // An empty range touches no chunk; it keeps any index.
                    let moved = |chunk: u64| index[source].get(&chunk).copied().unwrap_or_default();
                    match &mut tensor.body {
                        Body::Data(at) => at.chunk = moved(at.chunk),
                        Body::Encoded { parts, .. } => parts.iter_mut().for_each(|p| p.chunk = moved(p.chunk)),
                    }
                    tensor
                })
                .collect();
            Component { tensors, ..declared }
        })
        .collect();
    (chunks, components)
}

/// The base's `own` with `from`'s tensors (of `sources[source]`), and its own of `fill`, in its order.
fn filled<'h>(own: &'h Component, source: usize, from: &'h Component, fill: &[String]) -> Result<Taken<'h>> {
    let keyed = |c: &'h Component| c.tensors.iter().map(|t| (t.key.as_str(), t)).collect::<BTreeMap<&str, &Tensor>>();
    let (mine, theirs) = (keyed(own), keyed(from));
    if let Some(key) = fill.iter().find(|k| !mine.contains_key(k.as_str())) {
        return invalid(format!("the base's {} has no {key}, which the source left to it", own.name));
    }
    let mut tensors: Vec<(usize, &Tensor)> = Vec::new();
    for t in &own.tensors {
        match theirs.get(t.key.as_str()) {
            Some(supplied) => tensors.push((source, *supplied)),
            None if fill.contains(&t.key) => tensors.push((0, t)),
            None => {}
        }
    }
    tensors.extend(from.tensors.iter().filter(|t| !mine.contains_key(t.key.as_str())).map(|t| (source, t)));
    Ok(Taken { declared: own.clone(), tensors })
}

/// `base` with `supplies`, carrying `metadata`.
pub(crate) fn compose(base: &Header, supplies: &[Supply<'_>], mut metadata: Metadata) -> Result<Header> {
    // Sources: the base, then each distinct supplying header.
    let mut sources: Vec<&Header> = vec![base];
    let mut supplied: BTreeMap<&str, (usize, &Component, &[String])> = BTreeMap::new();
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
        if supplied.insert(&s.name, (source, component, &s.fill)).is_some() {
            return invalid(format!("component {:?} is supplied twice", s.name));
        }
    }
    // The base's order, then the added ones.
    let mut taken: Vec<Taken> = Vec::new();
    for own in base.components() {
        taken.push(match supplied.remove(own.name.as_str()) {
            Some((source, from, [])) => Taken::whole(own.clone(), source, from),
            Some((source, from, fill)) => filled(own, source, from, fill)?,
            None => Taken::whole(own.clone(), 0, own),
        });
    }
    for (name, (source, from, fill)) in supplied {
        if let Some(key) = fill.first() {
            return invalid(format!("the base has no {name}, whose {key} the source left to it"));
        }
        taken.push(Taken::whole(Component { name: name.to_owned(), ..from.clone() }, source, from));
    }
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

/// The chunks and components of `header` keeping only `kept`, each `(name, its component)`: the chunks their tensors touch, re-indexed.
pub(crate) fn cut(header: &Header, kept: &[(String, &str)]) -> Result<(Vec<ObjectRef>, Vec<Component>)> {
    let mut taken = Vec::new();
    for (name, from) in kept {
        let Some(component) = header.component(from) else { return invalid(format!("the header has no component {from:?}")) };
        taken.push(Taken::whole(Component { name: name.clone(), ..component.clone() }, 0, component));
    }
    Ok(assemble(&[header], taken))
}

/// Header metadata: what a checkpoint took from its base when it was made complete ([`inherit`]):
/// `{checkpoint: {sha256, length}, from: [{base: {model, release, lane, header}, components, files}]}`.
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

/// Whether two components are one model's: the same class (library and name; versions differ).
fn same_class(a: &Component, b: &Component) -> bool {
    (&a.class.library, &a.class.name) == (&b.class.library, &b.class.name)
}

/// Whether a component is a LoRA's: every tensor a factor, an alpha or one it keeps or carries.
fn adapter(component: &Component) -> bool {
    !component.tensors.is_empty() && family::is_lora(component)
}

/// `child` (whose own header is `checkpoint`) with what it lacks of `base`, recorded as `from`
/// (the base's model, release, lane, header): each component of the base it has not, whole, and
/// for each component both have, each of the base's assets it has not and the base's config when
/// its own is empty. Its own data always wins. None when it lacks nothing of the base. Refused
/// when either is a LoRA (one is applied to a base, never filled from another), or when they are
/// not one model: no component in common, or one in common of another class.
pub fn inherit(child: &Header, checkpoint: &ObjectRef, base: &Header, from: BTreeMap<String, Value>) -> Result<Option<(Header, Inherited)>> {
    if child.components().iter().chain(base.components()).any(adapter) {
        return invalid("a LoRA is applied to a base, never filled from one");
    }
    let shared: Vec<(&Component, &Component)> =
        child.components().iter().filter_map(|c| Some((c, base.component(&c.name)?))).collect();
    if shared.is_empty() {
        return invalid("the base has none of its components: it is another model");
    }
    if let Some((c, b)) = shared.iter().find(|(c, b)| !same_class(c, b)) {
        return invalid(format!("its {} is {}, and the base's is {}: the base is another model", c.name, c.class.name, b.class.name));
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
        taken.push(Taken::whole(declared, source, from));
    }
    for theirs in base.components().iter().filter(|b| child.component(&b.name).is_none()) {
        taken.push(Taken::whole(theirs.clone(), 1, theirs));
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

/// Header metadata: the files added to a checkpoint as assets (`cozy model upload --add`), each `<component>/<path>`.
pub const ADDED: &str = "added";

/// `header` with each `(component, path, object)` as that component's asset, in place of one of that path; a component it
/// lacks is added with these files alone (no class, config or tensors). Chunks and tensors stay as they are.
pub fn with_assets(header: &Header, added: &[(String, String, ObjectRef)]) -> Result<Header> {
    let mut components = header.components().to_vec();
    for (component, path, object) in added {
        let at = match components.iter().position(|c| &c.name == component) {
            Some(at) => at,
            None => {
                components.push(Component { name: component.clone(), ..Component::default() });
                components.len() - 1
            }
        };
        components[at].assets.insert(path.clone(), object.clone());
    }
    let mut metadata = header.metadata.clone();
    let mut listed = match metadata.remove(ADDED) {
        Some(Value::List(listed)) => listed,
        _ => vec![],
    };
    for (component, path, _) in added {
        let file = Value::Str(format!("{component}/{path}"));
        if !listed.contains(&file) {
            listed.push(file);
        }
    }
    metadata.insert(ADDED.into(), Value::List(listed));
    Header::new(header.chunks.clone(), header.pipeline().cloned(), components, metadata)
}
