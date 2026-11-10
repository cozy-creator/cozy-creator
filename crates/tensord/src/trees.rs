//! Retained file trees: a run's native inputs, a result's bytes, each a `api::tree` document
//! and its files. One use in the machine's store, named by its owner, keeps the document and every
//! file past a restart; the machine holds them while it runs (`Objects::sweep`).
use std::{collections::BTreeMap, io};

use api::tree::{self, Member};

use crate::{execution::Engine, storage};
use tensorfs::format::ObjectRef;

/// A held tree: its hold's id, its document and the bytes of its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tree {
    pub hold: String,
    pub manifest: ObjectRef,
    pub content: u64,
}

/// The use that keeps `owner`'s tree.
pub(crate) fn name(owner: &str) -> String {
    tensorfs::sha256::hex_digest(owner.as_bytes())
}

/// A tree's document for `files`.
pub(crate) fn document(files: &BTreeMap<String, ObjectRef>) -> Vec<u8> {
    let member = |o: &ObjectRef| Member { length: o.length, sha256: o.sha256.clone(), executable: false };
    tree::document(&files.iter().map(|(path, o)| (path.clone(), member(o))).collect())
}

/// A tree document's files, by path.
pub(crate) fn parse(bytes: &[u8]) -> io::Result<BTreeMap<String, ObjectRef>> {
    let files = tree::parse(bytes)?.into_iter();
    Ok(files.map(|(path, Member { length, sha256, .. })| (path, ObjectRef { sha256, length })).collect())
}

/// Holds the tree of `files` (each already held) for `owner`. Idempotent per owner.
pub(crate) fn put(engine: &Engine, owner: &str, files: BTreeMap<String, ObjectRef>) -> io::Result<Tree> {
    let store = match crate::private_io::current() {
        Some(scope) => crate::private_io::store(&scope)?,
        None => engine.store.clone(),
    };
    put_in(&store, owner, files)
}

/// A run's products use its admitted store even on background execution threads.
pub(crate) fn put_for(engine: &Engine, run: &str, owner: &str, files: BTreeMap<String, ObjectRef>) -> io::Result<Tree> {
    let store = engine.store_for(run)?;
    put_in(&store, owner, files)
}

fn put_in(store: &storage::Store, owner: &str, files: BTreeMap<String, ObjectRef>) -> io::Result<Tree> {
    let manifest = storage::put(store, &document(&files)[..], None)?;
    let mut roots = vec![manifest.clone()];
    for file in files.values() {
        if !store.reuse(file).map_err(storage::io_error)? {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("object {} is absent", file.id())));
        }
        if !roots.contains(file) {
            roots.push(file.clone());
        }
    }
    store.used(&name(owner), &roots).map_err(storage::io_error)?;
    Ok(Tree { hold: format!("tree-{}", name(owner)), manifest, content: files.values().map(|f| f.length).sum() })
}

/// A held tree's document and files, when its document is still here.
pub(crate) fn roots(store: &storage::Store, manifest: &ObjectRef) -> Option<Vec<ObjectRef>> {
    let files = parse(&store.read(manifest).ok()?).ok()?;
    let mut roots = vec![manifest.clone()];
    roots.extend(files.into_values().filter(|file| file != manifest));
    Some(roots)
}
