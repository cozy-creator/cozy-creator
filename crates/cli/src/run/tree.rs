//! A local directory given to a Tree input: its files, and the tree document naming them that the
//! machine materializes the tree from (`protocol::tree`). Both are sent with the run like any
//! input file. The manifests are a cache that keeps itself: each is touched when staged, one
//! unused for `UNUSED` goes when another is staged, and none is kept while the disk is low.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use protocol::{
    calcifer::StagedInput,
    error::{Error, Kind},
    ids::Digest,
    tree::{self, Member},
};

/// A manifest unused this long leaves the cache the next time one is staged.
const UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A directory staged: its manifest (written under `manifests`) and the files it names.
pub struct Staged {
    pub manifest: StagedInput,
    pub files: Vec<StagedInput>,
}

/// Stages `root`'s files, each digest remembered in `memo` as an input file's is.
pub fn stage(root: &Path, memo: Option<&Path>, manifests: &Path) -> Result<Staged, Error> {
    // Calcifer reads the files, from its own directory: `./folder` is this one's.
    let root = &std::path::absolute(root).map_err(|e| unreadable(root, e))?;
    let mut files = vec![];
    let mut members = BTreeMap::new();
    let found = tree::walk(root, true).map_err(|e| Error::new(Kind::Usage, "tree_unreadable", e.to_string()))?;
    for (name, path) in found {
        let length = fs::metadata(&path).map_err(|e| unreadable(&path, e))?.len();
        let digest = super::digests::of(memo, &path)?;
        let sha256 = hex::encode(digest.bytes());
        members.insert(name, Member { length, sha256 });
        files.push(StagedInput {
            path,
            digest,
            length,
        });
    }
    let bytes = tree::document(&members);
    let digest = Digest::of(&bytes);
    let path = keep(manifests, &hex::encode(digest.bytes()), &bytes)
        .map_err(|e| unreadable(manifests, e))?;
    let length = bytes.len() as u64;
    let manifest = StagedInput {
        path,
        digest,
        length,
    };
    Ok(Staged { manifest, files })
}

/// The manifest's file: kept under `manifests` (a reused one touched, the unused swept), or, on
/// a low disk, a scratch file the cache never holds.
fn keep(manifests: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    if low(manifests) {
        let path = std::env::temp_dir().join(format!("cozy-tree-{name}"));
        fs::write(&path, bytes)?;
        return Ok(path);
    }
    fs::create_dir_all(manifests)?;
    let (path, now) = (manifests.join(name), SystemTime::now());
    match File::options().write(true).open(&path) {
        Ok(file) => file.set_modified(now)?,
        Err(_) => {
            let part = tempfile::NamedTempFile::new_in(manifests)?;
            fs::write(part.path(), bytes)?;
            part.persist(&path).map_err(|e| e.error)?;
        }
    }
    for old in fs::read_dir(manifests)?.flatten() {
        let modified = old.metadata().and_then(|m| m.modified());
        if modified.is_ok_and(|at| at + UNUSED < now) {
            let _ = fs::remove_file(old.path());
        }
    }
    Ok(path)
}

/// Whether the disk holding `dir` has no more free than TensorFS keeps in reserve.
fn low(dir: &Path) -> bool {
    let probe = dir.ancestors().find(|d| d.exists()).unwrap_or(dir);
    let reserve = |total: u64| (total / 50).clamp(1 << 30, 10 << 30);
    platform::durable::space(probe).is_ok_and(|s| s.free <= reserve(s.total))
}

fn unreadable(path: &Path, e: std::io::Error) -> Error {
    Error::new(
        Kind::Usage,
        "tree_unreadable",
        format!("{}: {e}", path.display()),
    )
}
