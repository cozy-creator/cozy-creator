//! Plain folders in and out of the store. A folder is a commit on disk: repo files at their
//! paths, one `.cozytensors` header, and `chunks/<sha256>` for the chunks and assets it cites
//! (thin: some may be in the store instead).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use platform::durable::FileId;

use super::{fail, Error, Result, Store};
use crate::format::{check_file, is_header, Header, ObjectRef, Tree, CHUNKS_DIR};

impl Store {
    /// Puts the tree document; its files need not be here.
    pub fn put_tree(&self, tree: &Tree) -> Result<ObjectRef> {
        self.put_bytes(&tree.to_json()?)
    }

    /// Admits the plain folder `dir` as a commit in lane `lane`: every object its header cites
    /// that `chunks/` has, then the header and repo files, then the tree. Every byte is hashed
    /// once, here. Returns the tree and the cited objects in neither the folder nor this store (a
    /// thin folder whose bytes an endpoint may already hold).
    pub fn admit_folder(&self, dir: &Path, lane: &str) -> Result<(ObjectRef, Vec<ObjectRef>)> {
        let (mut tree, mut absent) = (Tree { lane: lane.into(), ..Tree::default() }, Vec::new());
        for path in files(dir, "")? {
            let at = dir.join(&path);
            if is_header(&path) {
                let bytes = fs::read(&at).map_err(fail("read", &at))?;
                for object in Header::from_cbor(&bytes)?.objects() {
                    let chunk = dir.join(CHUNKS_DIR).join(&object.sha256);
                    match File::open(&chunk) {
                        Ok(file) => drop(self.put(file, Some(object))?),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {
                            if !self.reuse(object)? && !absent.contains(object) {
                                absent.push(object.clone());
                            }
                        }
                        Err(e) => return Err(fail("open", &chunk)(e)),
                    }
                }
                tree.files.insert(path, self.put_bytes(&bytes)?);
            } else {
                let file = File::open(&at).map_err(fail("open", &at))?;
                tree.files.insert(path, self.put(file, None)?);
            }
        }
        tree.header()?;
        Ok((self.put_tree(&tree)?, absent))
    }

    /// The commit `tree` as a plain folder in `dir`: its files, and its header's objects this
    /// store holds.
    pub fn snapshot(&self, tree: &ObjectRef, dir: &Path) -> Result<()> {
        let tree = self.tree(tree)?;
        let mut payload = Vec::new();
        for object in Header::from_cbor(&self.read(tree.header()?.1)?)?.objects() {
            if self.reuse(object)? {
                payload.push(object.clone());
            }
        }
        self.write_folder(&tree, dir, &payload)
    }

    /// The commit `tree` as a plain folder in `dir`: its repo files, `payload` under `chunks/`,
    /// then its header last, so a folder never has a header without what it names.
    pub fn write_folder(&self, tree: &Tree, dir: &Path, payload: &[ObjectRef]) -> Result<()> {
        let chunks = dir.join(CHUNKS_DIR);
        fs::create_dir_all(&chunks).map_err(fail("create", &chunks))?;
        for (path, object) in tree.repo_files() {
            self.materialize(object, &dir.join(path))?;
        }
        for object in payload {
            self.materialize(object, &chunks.join(&object.sha256))?;
        }
        let (path, header) = tree.header()?;
        self.materialize(header, &dir.join(path))
    }

    /// `object` at `dest`, the one store-to-folder writer: inside this store a hard link of its
    /// blob; in a folder its caller owns a checked copy, where an edit must not reach the stored
    /// object. A file already there stays only when it holds exactly these bytes.
    pub fn materialize(&self, object: &ObjectRef, dest: &Path) -> Result<()> {
        let src = self.path(object)?;
        if dest.starts_with(self.root()) {
            if FileId::at(dest).ok().is_some_and(|at| FileId::at(&src).is_ok_and(|blob| blob == at)) {
                return Ok(());
            }
            let (temp, file) = temp_beside(dest)?;
            drop(file);
            let _ = fs::remove_file(&temp);
            if fs::hard_link(&src, &temp).is_ok() {
                return fs::rename(&temp, dest).map_err(fail("rename into", dest));
            }
        } else if File::open(dest).ok().is_some_and(|mut held| same_bytes(&mut held, object)) {
            return Ok(());
        }
        copy_checked(&mut self.file(object)?, dest, Some(object))
    }
}

/// A new, unique temp file beside `dest`, its directory made.
fn temp_beside(dest: &Path) -> Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = dest.parent().expect("a file has a directory");
    fs::create_dir_all(parent).map_err(fail("create", parent))?;
    loop {
        let temp = parent.join(format!(".tensorfs-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fail("create", &temp)(e)),
        }
    }
}

/// Whether `file` holds exactly `object`'s bytes.
fn same_bytes(file: &mut File, object: &ObjectRef) -> bool {
    let mut hashing = Hashing { source: file, hash: crate::sha256::Sha256::new() };
    io::copy(&mut hashing, &mut io::sink()).is_ok_and(|length| length == object.length)
        && crate::sha256::hex(&hashing.hash.finish()) == object.sha256
}

/// `from` copied to `dest` by a unique temp, flushed and renamed in, its directory synced; with
/// `expected`, the bytes are checked against it first.
pub(crate) fn copy_checked(from: &mut File, dest: &Path, expected: Option<&ObjectRef>) -> Result<()> {
    let (temp, mut out) = temp_beside(dest)?;
    let mut source = Hashing { source: from, hash: crate::sha256::Sha256::new() };
    let copied = io::copy(&mut source, &mut out).map_err(fail("copy", &temp)).and_then(|length| {
        let got = ObjectRef { sha256: crate::sha256::hex(&source.hash.finish()), length };
        match expected.filter(|expected| **expected != got) {
            Some(expected) => Err(Error::Mismatch { expected: expected.clone(), got: format!("{} ({length} bytes) copied to {}", got.sha256, dest.display()) }),
            None => out.sync_all().map_err(fail("sync", &temp)),
        }
    });
    drop(out);
    let done = copied.and_then(|()| fs::rename(&temp, dest).map_err(fail("rename into", dest)));
    if done.is_err() {
        let _ = fs::remove_file(&temp);
    }
    done?;
    super::sync_dir(dest.parent().expect("a file has a directory"))
}

struct Hashing<'a> {
    source: &'a mut File,
    hash: crate::sha256::Sha256,
}

impl Read for Hashing<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.source.read(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
}

/// Every regular file under `dir`, as tree paths, but `chunks/` at the root.
pub(crate) fn files(dir: &Path, prefix: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(fail("list", dir))? {
        let entry = entry.map_err(fail("list", dir))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = match prefix {
            "" if name == CHUNKS_DIR => continue,
            "" => name,
            p => format!("{p}/{name}"),
        };
        let kind = entry.file_type().map_err(fail("stat", entry.path()))?;
        if kind.is_dir() {
            out.extend(files(&entry.path(), &path)?);
        } else if kind.is_file() {
            check_file(&path)?;
            out.push(path);
        }
    }
    Ok(out)
}
