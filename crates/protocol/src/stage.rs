//! A folder a client sends (a Tree input, a CozyTensors folder): its files, and the tree document
//! naming them (`protocol::tree`), which goes after them as a file of its own.
use std::{collections::BTreeMap, fs, path::Path};

use crate::{
    calcifer::StagedInput,
    error::{Error, Kind},
    tree::{self, Member},
};

/// A folder read: its tree document and the files it names.
pub struct Staged {
    pub document: Vec<u8>,
    pub files: Vec<StagedInput>,
}

/// `root`'s files, through links, each digest remembered in `memo` as an input file's is.
pub fn stage(root: &Path, memo: Option<&Path>) -> Result<Staged, Error> {
    let unreadable = |path: &Path, e: std::io::Error| Error::new(Kind::Usage, "tree_unreadable", format!("{}: {e}", path.display()));
    let found = tree::walk(root, true).map_err(|e| Error::new(Kind::Usage, "tree_unreadable", e.to_string()))?;
    let (mut files, mut members) = (vec![], BTreeMap::new());
    for (name, path) in found {
        let length = fs::metadata(&path).map_err(|e| unreadable(&path, e))?.len();
        let digest = crate::digests::of(memo, &path)?;
        members.insert(name, Member { length, sha256: hex::encode(digest.bytes()), executable: false });
        files.push(StagedInput { path, digest, length });
    }
    Ok(Staged { document: tree::document(&members), files })
}
