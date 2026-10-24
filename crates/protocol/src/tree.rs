//! A file tree's document, the one form every cozy process writes and reads:
//! `{"entries": [{"blob": {"length", "sha256"}, "kind": "file", "path"}]}`, keys sorted and entries
//! by path. A path is the file's parts below the tree joined with `/`, each part one every OS can
//! name: not empty, `.` or `..`, and without `\`, `:` or NUL.
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The media type of a tree's document.
pub const MEDIA: &str = "application/vnd.cozy.tree-manifest";

/// A file of a tree: its bytes' length and sha256 (lowercase hex).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub length: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
struct Document {
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    blob: Member,
    kind: String,
    path: String,
}

/// The document of `files`.
pub fn document(files: &BTreeMap<String, Member>) -> Vec<u8> {
    let entries = (files.iter())
        .map(|(path, blob)| Entry { blob: blob.clone(), kind: "file".into(), path: path.clone() })
        .collect();
    serde_json::to_vec(&Document { entries }).expect("a tree document serializes")
}

/// A document's files by path: files only, each path portable, named once and inside no other file.
pub fn parse(bytes: &[u8]) -> io::Result<BTreeMap<String, Member>> {
    let invalid = |why: String| io::Error::new(io::ErrorKind::InvalidInput, why);
    let document: Document =
        serde_json::from_slice(bytes).map_err(|e| invalid(format!("a tree document: {e}")))?;
    let hex = |s: &str| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let mut files = BTreeMap::new();
    for Entry { blob, kind, path } in document.entries {
        if kind != "file" {
            return Err(invalid(format!("a tree holds files, not {kind:?}")));
        }
        if !portable(&path) {
            return Err(invalid(format!("tree path {path:?} is not a portable path inside its tree")));
        }
        if !hex(&blob.sha256) {
            return Err(invalid(format!("the tree's {path} names no sha256")));
        }
        if files.insert(path.clone(), blob).is_some() {
            return Err(invalid(format!("a tree names {path} twice")));
        }
    }
    let nested = files.keys().find(|path| path.match_indices('/').any(|(i, _)| files.contains_key(&path[..i])));
    if let Some(path) = nested {
        return Err(invalid(format!("a tree names {path} inside another of its files")));
    }
    Ok(files)
}

/// Every file below `root` by its tree path; a name that is not a portable path is refused.
/// `follow` reads through links, as a person's own folder (a local Tree input) always was read;
/// without it, as for an output spool package code writes, a link is refused. Any other entry that
/// is neither a regular file nor a folder is refused.
pub fn walk(root: &Path, follow: bool) -> io::Result<BTreeMap<String, PathBuf>> {
    let at = |path: &Path, e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let (mut files, mut pending) = (BTreeMap::new(), vec![root.to_path_buf()]);
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| at(&dir, e))? {
            let entry = entry.map_err(|e| at(&dir, e))?;
            let path = entry.path();
            let kind = match follow {
                true => fs::metadata(&path).map(|meta| meta.file_type()),
                false => entry.file_type(),
            }
            .map_err(|e| at(&path, e))?;
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let refused = |why: &str| at(&path, io::Error::new(io::ErrorKind::InvalidInput, why));
            if !kind.is_file() {
                return Err(refused("a tree holds only regular files and folders"));
            }
            let parts: Option<Vec<&str>> =
                path.strip_prefix(root).expect("under its root").iter().map(|p| p.to_str()).collect();
            let name = parts.map(|parts| parts.join("/")).filter(|name| portable(name));
            files.insert(name.ok_or_else(|| refused("not a name every OS can hold"))?, path);
        }
    }
    Ok(files)
}

fn portable(path: &str) -> bool {
    path.split('/').all(|part| !matches!(part, "" | "." | "..") && !part.contains(['\\', ':', '\0']))
}
