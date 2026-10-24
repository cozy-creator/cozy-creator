//! Retained file trees: a run's native inputs, a result's bytes, an uploaded package file, each a
//! `protocol::tree` document and its files. One hold in the run objects' store (`cas`),
//! `tree-<owner>`, keeps the document and every file until the owner releases it.
use std::{collections::BTreeMap, fs::File, io, path::Path};

use protocol::tree::{self, Member};

use crate::cas::{Cas, Hold};
use tensorfs::format::ObjectRef;

/// A held tree: its hold's id, its document and the bytes of its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub hold: String,
    pub manifest: ObjectRef,
    pub content: u64,
}

/// The hold that keeps `owner`'s tree.
fn id(owner: &str) -> String {
    format!("tree-{}", tensorfs::sha256::hex_digest(owner.as_bytes()))
}

/// A tree's document for `files`.
pub fn document(files: &BTreeMap<String, ObjectRef>) -> Vec<u8> {
    let member = |o: &ObjectRef| Member { length: o.length, sha256: o.sha256.clone() };
    tree::document(&files.iter().map(|(path, o)| (path.clone(), member(o))).collect())
}

/// A tree document's files, by path.
pub fn parse(bytes: &[u8]) -> io::Result<BTreeMap<String, ObjectRef>> {
    let files = tree::parse(bytes)?.into_iter();
    Ok(files.map(|(path, Member { length, sha256 })| (path, ObjectRef { sha256, length })).collect())
}

/// Holds the tree of `files` (each already held) for `owner`. Idempotent per owner.
pub fn put(cas: &Cas, owner: &str, files: BTreeMap<String, ObjectRef>) -> io::Result<Tree> {
    let manifest = cas.put_bytes(&document(&files))?;
    hold(cas, owner, manifest, &files)
}

/// Holds `manifest`, a tree of `files` already held, for `owner`.
pub fn hold(cas: &Cas, owner: &str, manifest: ObjectRef, files: &BTreeMap<String, ObjectRef>) -> io::Result<Tree> {
    let mut roots = vec![manifest.clone()];
    for file in files.values() {
        if !roots.contains(file) {
            roots.push(file.clone());
        }
    }
    cas.hold(&id(owner), &Hold { owner: owner.to_string(), roots })?;
    Ok(Tree { hold: id(owner), manifest, content: files.values().map(|f| f.length).sum() })
}

/// Admits each `(path, local file, object)` not yet held, then holds their tree for `owner`.
pub fn import(cas: &Cas, owner: &str, files: &[(String, &Path, ObjectRef)]) -> io::Result<Tree> {
    admit(cas, files)?;
    put(cas, owner, files.iter().map(|(path, _, object)| (path.clone(), object.clone())).collect())
}

/// Admits each `(path, local file, object)` not held yet.
pub fn admit(cas: &Cas, files: &[(String, &Path, ObjectRef)]) -> io::Result<()> {
    for (_, local, object) in files {
        if !cas.reuse(object)? {
            cas.put(File::open(local)?, Some(object))?;
        }
    }
    Ok(())
}

/// `owner`'s held tree, if any: its document is the hold's first root.
pub fn read(cas: &Cas, owner: &str) -> io::Result<Option<Tree>> {
    let Some(manifest) = cas.held(&id(owner))?.and_then(|hold| hold.roots.into_iter().next()) else {
        return Ok(None);
    };
    let files = parse(&cas.read(&manifest)?)?;
    Ok(Some(Tree { hold: id(owner), manifest, content: files.values().map(|f| f.length).sum() }))
}

/// Drops `owner`'s hold: its files age from their last use.
pub fn release(cas: &Cas, owner: &str) -> io::Result<bool> {
    cas.release(&id(owner))
}
