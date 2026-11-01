//! GC: mark what the roots and the uses keep (`retention.rs`), then sweep unmarked objects and
//! snapshot folders older than the grace. A collection may first drop uses: those past a TTL, or
//! least recently used first when they free enough bytes; what only a dropped use kept goes in
//! that collection. Each drop and each sweep that frees bytes is logged.
//!
//! A swept object is first renamed into `tmp/gc/`, then deleted only if it is still old: a put
//! or reuse that raced the sweep made it young, and it goes back. A GC killed in between leaves
//! it there, and the next GC puts it back before deciding again.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{self, Read};
use std::time::{Duration, SystemTime};

use super::retention::{self, list, Use};
use super::{fail, invalid, is_digest, same_family, Result, Store};
use crate::format::{self, is_header, Header, ObjectRef, Tree, HEADER_FORMAT, TREE_FORMAT};

/// Documents bigger than this are never read as trees or headers.
const MAX_DOCUMENT: u64 = 256 << 20;

/// What one collection took.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    /// The uses dropped, by name.
    pub uses: Vec<String>,
    pub objects: u64,
    pub bytes: u64,
}

enum Document {
    Tree(Tree),
    Header(Header),
}

impl Store {
    /// What `root` keeps alive, each once: `root`, and for a tree its files and its header's
    /// chunks and assets; for a header, its chunks and assets.
    pub fn closure(&self, root: &ObjectRef) -> Result<Vec<ObjectRef>> {
        let mut reach = BTreeSet::from([root.clone()]);
        match self.document(root)? {
            Some(Document::Tree(tree)) => {
                for (path, file) in tree.files {
                    if is_header(&path) {
                        reach.extend(self.header_objects(&file)?);
                    }
                    reach.insert(file);
                }
            }
            Some(Document::Header(header)) => reach.extend(header.objects().cloned()),
            None => {}
        }
        Ok(reach.into_iter().collect())
    }

    /// The tree or header `object` is; `None` for any other object or an absent one. One of a
    /// kind this build cannot walk (another major) stops GC rather than sweep
    /// what it reaches.
    fn document(&self, object: &ObjectRef) -> Result<Option<Document>> {
        let path = self.path(object)?;
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(fail("open", &path)(error)),
        };
        let length = file.metadata().map_err(fail("stat", &path))?.len();
        let mut first = [0u8];
        if length > MAX_DOCUMENT || file.read_exact(&mut first).is_err() {
            return Ok(None);
        }
        // A tree is a JSON object; a header a CBOR array.
        if first[0] != b'{' && !(0x85..=0x9f).contains(&first[0]) {
            return Ok(None);
        }
        let mut bytes = first.to_vec();
        file.read_to_end(&mut bytes).map_err(fail("read", &path))?;
        let unwalkable =
            |found: &str| Err(invalid(&path, format!("cannot walk a {found} document")));
        if first[0] == b'{' {
            return match Tree::from_json(&bytes) {
                Ok(tree) => Ok(Some(Document::Tree(tree))),
                Err(format::Error::Format { found, .. }) if same_family(&found, TREE_FORMAT) => {
                    unwalkable(&found)
                }
                Err(_) => Ok(None),
            };
        }
        match Header::from_cbor(&bytes) {
            Ok(header) => Ok(Some(Document::Header(header))),
            Err(format::Error::Format { found, .. }) if same_family(&found, HEADER_FORMAT) => {
                unwalkable(&found)
            }
            Err(_) => Ok(None),
        }
    }

    /// A header's chunks and assets; none when the header is absent.
    fn header_objects(&self, header: &ObjectRef) -> Result<Vec<ObjectRef>> {
        let path = self.path(header)?;
        match fs::read(&path) {
            Ok(bytes) => Header::from_cbor(&bytes)
                .map(|header| header.objects().cloned().collect())
                .map_err(|error| invalid(&path, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(vec![]),
            Err(error) => Err(fail("read", &path)(error)),
        }
    }

    /// Every object under `blobs/`: its length and when it was last written or reused.
    fn objects(&self) -> Result<HashMap<String, (u64, SystemTime)>> {
        let mut objects = HashMap::new();
        for top in list(&self.root.join("blobs"))? {
            for shard in list(&top)? {
                for entry in list(&shard)? {
                    let Some(sha256) = entry.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let metadata = match fs::symlink_metadata(&entry) {
                        Ok(metadata) if metadata.is_file() && is_digest(sha256) => metadata,
                        Ok(_) => continue,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(fail("stat", &entry)(error)),
                    };
                    let modified = metadata.modified().map_err(fail("stat", &entry))?;
                    objects.insert(sha256.to_owned(), (metadata.len(), modified));
                }
            }
        }
        Ok(objects)
    }

    /// Sweeps what neither a root nor a use keeps and is older than the grace.
    pub fn gc(&self) -> Result<Collected> {
        self.collect(0, Duration::MAX)
    }

    /// Drops the uses unused for `older_than`, least recently used first, until `need` bytes
    /// would be free (`u64::MAX`: every one), then sweeps. Roots are never dropped. Skips while
    /// another process reads metadata to hold it.
    pub fn collect(&self, need: u64, older_than: Duration) -> Result<Collected> {
        let Some(_gc) = retention::try_collect(self)? else {
            return Ok(Collected::default());
        };
        self.collect_locked(need, older_than)
    }

    /// The caller holds the collection fence for the whole mark and sweep.
    pub(super) fn collect_locked(&self, need: u64, older_than: Duration) -> Result<Collected> {
        let now = SystemTime::now();
        let cutoff = now.checked_sub(self.grace).unwrap_or(SystemTime::UNIX_EPOCH);
        self.restore_trash()?;
        let mut marked = self.roots()?;
        let mut uses = self.uses(&marked)?;
        let objects = self.objects()?;
        let (dropped, evicted) = self.drop_uses(&mut uses, &marked, &objects, need, older_than, cutoff)?;
        for kept in uses {
            marked.extend(kept.digests);
            marked.insert(kept.name);
        }
        // Unreferenced objects past the grace, and what the dropped uses alone kept, now: what
        // the grace protects is an object not yet named, never one a dropped use named.
        let mut collected = Collected { uses: dropped.into_iter().collect(), ..Collected::default() };
        for (sha256, (_, modified)) in objects.iter().filter(|(sha, _)| !marked.contains(*sha)) {
            let since = if evicted.contains(sha256) { now } else { cutoff };
            if *modified >= since {
                continue;
            }
            if let Some(length) = self.discard(sha256, since)? {
                collected.objects += 1;
                collected.bytes += length;
            }
        }
        for folder in list(&self.root.join("snapshots"))? {
            let name = folder.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let old = fs::symlink_metadata(&folder)
                .is_ok_and(|m| m.is_dir() && m.modified().is_ok_and(|m| m < cutoff));
            if (old || collected.uses.iter().any(|u| u == name)) && is_digest(name) && !marked.contains(name) {
                match fs::remove_dir_all(&folder) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => {
                        return Err(fail("remove", &folder)(e))
                    }
                    _ => {}
                }
            }
        }
        self.sweep_tmp(cutoff)?;
        // The trash exists only during a collection (it holds what a killed one left).
        let _ = fs::remove_dir(self.root.join("tmp").join("gc"));
        if collected.bytes > 0 {
            let evicted = collected.uses.len();
            tracing::info!("tensorfs: freed {} in {} objects ({evicted} evicted, the rest unreferenced)", size(collected.bytes), collected.objects);
        }
        Ok(collected)
    }

    /// Removes the uses `need` asks for from the store and from `uses`, each logged: their
    /// names, and the objects only they kept, which go now. The least recently used unused for
    /// `older_than` go first, and only when, with the unreferenced objects past the grace, they
    /// free `need` (`u64::MAX`: every one, the TTL's). A use a root holds any of stays.
    fn drop_uses(
        &self,
        uses: &mut Vec<Use>,
        roots: &BTreeSet<String>,
        objects: &HashMap<String, (u64, SystemTime)>,
        need: u64,
        older_than: Duration,
        cutoff: SystemTime,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
        let none = (BTreeSet::new(), BTreeSet::new());
        let Some(since) = SystemTime::now().checked_sub(older_than).filter(|_| need > 0) else {
            return Ok(none);
        };
        uses.sort_by(|a, b| (a.used, &a.name).cmp(&(b.used, &b.name)));
        let mut count: HashMap<&str, u32> = HashMap::new();
        for digest in uses.iter().flat_map(|u| &u.digests) {
            *count.entry(digest).or_default() += 1;
        }
        let length = |sha: &str| objects.get(sha).map_or(0, |(length, _)| *length);
        let old = |sha: &str| objects.get(sha).is_some_and(|(_, modified)| *modified < cutoff);
        let orphan = |sha: &&String| !count.contains_key(sha.as_str()) && !roots.contains(*sha) && old(sha);
        let mut free: u64 = objects.keys().filter(orphan).map(|sha| length(sha)).sum();
        let (mut plan, mut evicted) = (vec![], BTreeSet::new());
        // A use whose name or any object a root holds is in use (a running or queued call's).
        let in_use = |u: &Use| roots.contains(&u.name) || u.digests.iter().any(|d| roots.contains(d));
        for u in uses.iter().filter(|u| u.used <= since && !in_use(u)) {
            if free >= need {
                break;
            }
            let mut bytes = 0;
            for digest in &u.digests {
                let n = count.get_mut(digest.as_str()).expect("a use counts what it keeps");
                *n -= 1;
                if *n == 0 && length(digest) > 0 {
                    bytes += length(digest);
                    evicted.insert(digest.clone());
                }
            }
            free = free.saturating_add(bytes);
            plan.push((u, bytes));
        }
        if free < need && need != u64::MAX {
            return Ok(none);
        }
        let names = if plan.is_empty() { HashMap::new() } else { self.names() };
        for (u, bytes) in &plan {
            self.remove_record(&self.root.join("uses").join(&u.name))?;
            let name = names.get(&u.name).cloned().unwrap_or_else(|| format!("cache entry {}", &u.name[..12]));
            let idle = u.used.elapsed().unwrap_or_default().as_secs() / 60;
            let why = if need == u64::MAX { "past its TTL".into() } else { format!("the disk {} short", size(need)) };
            tracing::info!("tensorfs: evicted {name}: {}, idle {idle} min, {why}", size(*bytes));
        }
        let dropped: BTreeSet<String> = plan.into_iter().map(|(u, _)| u.name.clone()).collect();
        uses.retain(|u| !dropped.contains(&u.name));
        Ok((dropped, evicted))
    }

    /// Each model lane the refs name, `org/name@release lane`, by its commit's digest and its
    /// header's: what a use is, for people.
    fn names(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for repo in self.refs().unwrap_or_default() {
            let lanes = repo.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, tree)| (format!("{}@{} {lane}", repo.name, r.version), tree)));
            for (name, tree) in lanes.chain(repo.commits.iter().map(|tree| (repo.name.clone(), tree))) {
                if let Some(header) = self.tree(tree).ok().and_then(|t| t.header().ok().map(|(_, h)| h.sha256.clone())) {
                    names.insert(header, name.clone());
                }
                names.insert(tree.sha256.clone(), name);
            }
        }
        names
    }

    /// Deletes `sha256` unless it became young since it was listed; its length when deleted.
    fn discard(&self, sha256: &str, cutoff: SystemTime) -> Result<Option<u64>> {
        let blob = self.blob(sha256)?;
        // A reader's shared lock keeps it; holding this one keeps readers out until decided.
        let Some(_held) = exclusive(&blob)? else {
            return Ok(None);
        };
        let trash = self.trash(sha256);
        // Windows refuses to move a file someone holds open; it is collected another time.
        if fs::rename(&blob, &trash).is_err() {
            return Ok(None);
        }
        // From here a put or reuse may take it back (`young`): gone from the trash is kept.
        let gone = |error: &io::Error| error.kind() == io::ErrorKind::NotFound;
        let metadata = match fs::metadata(&trash) {
            Ok(metadata) => metadata,
            Err(error) if gone(&error) => return Ok(None),
            Err(error) => return Err(fail("stat", &trash)(error)),
        };
        if metadata.modified().map_err(fail("stat", &trash))? >= cutoff {
            return match fs::rename(&trash, &blob) {
                Err(error) if !gone(&error) => Err(fail("restore", &blob)(error)),
                _ => Ok(None),
            };
        }
        match fs::remove_file(&trash) {
            Ok(()) => Ok(Some(metadata.len())),
            Err(error) if gone(&error) => Ok(None),
            Err(error) => Err(fail("remove", &trash)(error)),
        }
    }

    /// Puts back what a GC killed mid-sweep left in `tmp/gc/`.
    fn restore_trash(&self) -> Result<()> {
        let trash = self.root.join("tmp").join("gc");
        fs::create_dir_all(&trash).map_err(fail("create", &trash))?;
        for entry in list(&trash)? {
            let Some(blob) = entry
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|sha| self.blob(sha).ok())
            else {
                continue;
            };
            let shard = blob.parent().expect("a blob has a shard");
            fs::create_dir_all(shard).map_err(fail("create", shard))?;
            match fs::rename(&entry, &blob) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(fail("restore", &blob)(error)),
            }
        }
        Ok(())
    }

    /// Removes the temps of puts and commits that died before the grace. Directories under
    /// `tmp/` belong to the layers that made them.
    fn sweep_tmp(&self, cutoff: SystemTime) -> Result<()> {
        for entry in list(&self.root.join("tmp"))? {
            let Ok(metadata) = fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_file() && metadata.modified().is_ok_and(|m| m < cutoff) {
                if let Some(_held) = exclusive(&entry)? {
                    let _ = fs::remove_file(&entry);
                }
            }
        }
        Ok(())
    }
}

fn exclusive(path: &std::path::Path) -> Result<Option<File>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail("open collection candidate", path)(error)),
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(fail("lock collection candidate", path)(error))
        }
    }
}

/// Bytes for people: MiB to a tenth below a GiB, else GiB to a hundredth.
fn size(bytes: u64) -> String {
    match bytes < 1 << 30 {
        true => format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64),
        false => format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64),
    }
}
