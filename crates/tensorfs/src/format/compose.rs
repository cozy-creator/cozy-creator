//! A header composed by reference (`cozy model upload --base`): a base header whose components
//! are replaced, or added, by other headers' components. Every entry it keeps is the base's
//! verbatim (class, config, assets, tensors, the same chunk objects); a replaced component takes
//! the supplier's tensors under the base's class, config and assets. No byte is read: chunks are
//! re-indexed. Each source's chunks that a kept range touches stay in their order (a range
//! continues into the next chunk), others drop, and a header that supplies twice is listed once.

use std::collections::{BTreeMap, BTreeSet};

use super::{invalid, Body, Component, Header, Metadata, Result};

/// `component` of `header` becomes the composition's `name`.
pub struct Supply<'h> {
    pub name: String,
    pub header: &'h Header,
    pub component: String,
}

/// `base` with `supplies`, carrying `metadata`.
pub fn compose(base: &Header, supplies: &[Supply<'_>], metadata: Metadata) -> Result<Header> {
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
    // (name, declaration, source, the component whose tensors it takes): the base's order, then
    // the added ones.
    let mut taken: Vec<(&str, &Component, usize, &Component)> = Vec::new();
    for own in base.components() {
        taken.push(match supplied.remove(own.name.as_str()) {
            Some((source, from)) => (&own.name, own, source, from),
            None => (&own.name, own, 0, own),
        });
    }
    taken.extend(supplied.into_iter().map(|(name, (source, from))| (name, from, source, from)));
    let mut touched = vec![BTreeSet::new(); sources.len()];
    for (_, _, source, from) in &taken {
        for (_, at) in from.tensors.iter().flat_map(|t| t.locations()) {
            touched[*source].extend(sources[*source].spans(&at).into_iter().map(|s| s.chunk));
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
        .map(|(name, declared, source, from)| {
            let mut component = Component { name: name.to_owned(), tensors: from.tensors.clone(), ..declared.clone() };
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
    Header::new(chunks, base.pipeline().cloned(), components, metadata)
}
