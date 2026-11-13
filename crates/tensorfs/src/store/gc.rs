//! GC: mark what the roots and the uses keep (`retention.rs`), then sweep unmarked objects and
//! snapshot folders older than the grace. A collection may first drop lanes: those past a TTL, or
//! least recently used first when they free enough bytes; what only a dropped lane kept goes in
//! that collection. A lane is one checkpoint's uses, named by its header and by each commit or
//! snapshot that keeps it, dropped whole: a model is never listed without its bytes, nor its bytes
//! kept unlisted. Each drop and each sweep that frees bytes is logged, and a sweep that removed
//! an object stamps `uses/.evicted` (`Store::evicted_at`).
//!
//! A swept object is first renamed into `tmp/gc/`, then deleted only if it is still old: a put
//! or reuse that raced the sweep made it young, and it goes back. A GC killed in between leaves
//! it there, and the next GC puts it back before deciding again.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{self, Read};
use std::time::{Duration, SystemTime};

use super::retention::{list, Use};
use super::{fail, invalid, is_digest, same_family, Result, Store};
use crate::format::{self, is_header, Header, ObjectRef, Tree, HEADER_FORMAT, TREE_FORMAT};

/// What removing one local repository reference did to its cached objects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovedModel {
    pub removed: bool,
    /// Logical bytes unlinked from cache object paths; hardlinks may retain their storage.
    pub cache_bytes_removed: u64,
    /// Candidate bytes still held by readers, other references/uses, or file locks.
    pub retained_bytes: u64,
}

/// Exact verified custody retirement. Unrequested roots and readers always keep their bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetiredObjects {
    pub removed: Vec<ObjectRef>,
    pub retained: Vec<ObjectRef>,
    pub removed_uses: Vec<String>,
}

/// Documents bigger than this are never read as trees or headers.
const MAX_DOCUMENT: u64 = 256 << 20;

/// What one collection took.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    /// The uses dropped, by name.
    pub uses: Vec<String>,
    /// The lanes dropped, for people.
    pub evicted: Vec<Evicted>,
    pub objects: u64,
    pub bytes: u64,
}

impl Collected {
    pub(super) fn add(&mut self, other: Collected) {
        self.uses.extend(other.uses);
        self.evicted.extend(other.evicted);
        (self.objects, self.bytes) = (self.objects + other.objects, self.bytes + other.bytes);
    }
}

/// One lane a collection dropped: its model as people name it, the bytes only it kept, and how
/// long it had gone unused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evicted {
    pub name: String,
    pub bytes: u64,
    pub idle: Duration,
}

/// One checkpoint's uses together (`lanes`): last used when any of them was, kept by all.
pub(super) struct Lane<'a> {
    pub uses: Vec<&'a Use>,
    pub used: SystemTime,
    pub digests: BTreeSet<&'a str>,
}

impl Lane<'_> {
    /// Whether a root of `digest` holds this lane: it names the lane, or an object no other lane
    /// keeps (`count`: how many lanes keep each). A reader of another checkpoint holds the objects
    /// the two share, and those stay, but not this lane: H3's renders held what it shares with
    /// Qwen-Image, so neither could go (valentino, 11-05).
    pub(crate) fn held_by(&self, digest: &str, count: &HashMap<&str, u32>) -> bool {
        self.uses.iter().any(|u| u.name == digest) || (self.digests.contains(digest) && count.get(digest) == Some(&1))
    }
}

/// How many lanes keep each digest.
pub(super) fn keepers<'a>(lanes: &[Lane<'a>]) -> HashMap<&'a str, u32> {
    let mut count: HashMap<&str, u32> = HashMap::new();
    for digest in lanes.iter().flat_map(|l| l.digests.iter().copied()) {
        *count.entry(digest).or_default() += 1;
    }
    count
}

/// `uses` as lanes: a use named by a digest another use keeps (a header its commit keeps, a
/// commit a snapshot keeps) is that one's lane. Least recently used first.
pub(super) fn lanes(uses: &[Use]) -> Vec<Lane<'_>> {
    let at: HashMap<&str, usize> = uses.iter().enumerate().map(|(i, u)| (u.name.as_str(), i)).collect();
    let mut parent: Vec<usize> = (0..uses.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (i, u) in uses.iter().enumerate() {
        for j in u.digests.iter().filter_map(|d| at.get(d.as_str()).copied()) {
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            parent[a] = b;
        }
    }
    let mut lanes: HashMap<usize, Lane> = HashMap::new();
    for (i, u) in uses.iter().enumerate() {
        let lane = lanes.entry(root(&mut parent, i)).or_insert_with(|| Lane { uses: vec![], used: u.used, digests: BTreeSet::new() });
        lane.used = lane.used.max(u.used);
        lane.digests.extend(u.digests.iter().map(String::as_str));
        lane.uses.push(u);
    }
    let mut lanes: Vec<Lane> = lanes.into_values().collect();
    lanes.sort_by(|a, b| (a.used, &a.uses[0].name).cmp(&(b.used, &b.uses[0].name)));
    lanes
}

enum Document {
    Tree(Tree),
    Header(Header),
}

impl Store {
    /// Removes one model's ref and its direct checkpoint uses, then only its unshared cached
    /// objects. Live holds, external header files, other refs and other uses keep their bytes.
    /// A missing ref is a no-op. Origin stores cannot be changed through this operation.
    pub fn remove_cached_model(&self, name: &str) -> Result<RemovedModel> {
        if !self.cache {
            return Err(invalid(self.root(), "local removal requires a cache store"));
        }
        let mut collections = self.collections.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fence = super::retention::exclusive(self)?;
        let _refs = self.lock("refs")?;
        self.restore_trash()?;
        let Some(model) = self.get_ref(name)? else { return Ok(RemovedModel::default()) };
        let mut candidates = BTreeSet::new();
        let mut own_uses = BTreeSet::new();
        for object in model.roots() {
            own_uses.insert(object.sha256.clone());
            if let Some(Document::Tree(tree)) = self.document(object)? {
                own_uses.extend(tree.files.iter().filter(|(path, _)| is_header(path)).map(|(_, header)| header.sha256.clone()));
            }
            candidates.extend(self.closure(object)?);
        }
        // Read every keeper before changing metadata: an unreadable retention source fails
        // closed. Cached refs count here too, even though ordinary pressure GC may evict them.
        let mut keep: BTreeSet<String> = self.holders_except(Some(&model.name))?.into_keys().collect();
        let mut other_models = BTreeSet::new();
        for other in self.refs()?.into_iter().filter(|other| !other.name.eq_ignore_ascii_case(&model.name)) {
            for object in other.roots() {
                other_models.extend(self.closure(object)?.into_iter().map(|object| object.sha256));
            }
        }
        keep.extend(other_models.iter().cloned());
        let uses = self.uses(&keep)?;
        let mut remove_uses = Vec::new();
        for used in uses {
            if own_uses.contains(&used.name) && !other_models.contains(&used.name) {
                remove_uses.push(used.name);
            } else {
                keep.insert(used.name);
                keep.extend(used.digests);
            }
        }
        self.remove_record(&self.ref_path(&model.name)?)?;
        for name in remove_uses {
            self.remove_record(&self.root.join("uses").join(name))?;
        }
        let mut removed = RemovedModel { removed: true, ..Default::default() };
        let before = SystemTime::now();
        for object in candidates {
            let path = self.path(&object)?;
            let length = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => metadata.len(),
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(fail("stat", &path)(error)),
            };
            let discarded = if keep.contains(&object.sha256) { None } else { self.discard(&object.sha256, before)? };
            match discarded {
                Some(bytes) => removed.cache_bytes_removed += bytes,
                None => removed.retained_bytes += length,
            }
        }
        self.stamp_eviction()?;
        *collections += 1;
        Ok(removed)
    }

    /// Retire only caller-proven use keys and object candidates after their verified copy.
    /// Uses containing an unrequested digest are retained, as are independent refs, header
    /// roots, live holds, authoritative peer requests and open file readers. No grace or TTL
    /// is used to infer ownership. This reuses the normal collection/deletion engine.
    pub fn retire_objects(
        &self,
        objects: &[ObjectRef],
        owned_uses: &[String],
        authoritative_keep: &[ObjectRef],
    ) -> Result<RetiredObjects> {
        if !self.cache {
            return Err(invalid(
                self.root(),
                "custody retirement requires a cache store",
            ));
        }
        let mut collections = self
            .collections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fence = super::retention::exclusive(self)?;
        let _refs = self.lock("refs")?;
        let _uses = self.lock("uses")?;
        self.restore_trash()?;
        let candidates = objects
            .iter()
            .map(|object| object.sha256.clone())
            .collect::<BTreeSet<_>>();
        let requested = owned_uses.iter().cloned().collect::<BTreeSet<_>>();
        if requested.iter().any(|name| !is_digest(name)) {
            return Err(invalid(self.root(), "owned use has an invalid digest"));
        }
        let mut keep = self.holders()?.into_keys().collect::<BTreeSet<_>>();
        // Cached refs are independent registrations too; pressure GC's TTL does not confer
        // permission to erase them as part of another project's migration.
        for model in self.refs()? {
            for root in model.roots() {
                keep.extend(self.closure(root)?.into_iter().map(|object| object.sha256));
            }
        }
        for root in authoritative_keep {
            keep.extend(self.closure(root)?.into_iter().map(|object| object.sha256));
        }
        let mut retired = RetiredObjects::default();
        let uses = self.uses(&BTreeSet::new())?;
        for used in uses {
            if requested.contains(&used.name)
                && !keep.contains(&used.name)
                && used
                    .digests
                    .iter()
                    .all(|digest| candidates.contains(digest))
            {
                self.remove_record(&self.root.join("uses").join(&used.name))?;
                retired.removed_uses.push(used.name);
            } else {
                keep.insert(used.name);
                keep.extend(used.digests);
            }
        }
        let before = SystemTime::now();
        for object in objects {
            if keep.contains(&object.sha256) {
                retired.retained.push(object.clone());
                continue;
            }
            let path = self.path(object)?;
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Ok(metadata) if metadata.is_file() && metadata.len() == object.length => (),
                Ok(_) => {
                    return Err(invalid(
                        &path,
                        "retirement candidate changed type or length",
                    ));
                }
                Err(error) => return Err(fail("stat", &path)(error)),
            }
            match self.discard(&object.sha256, before)? {
                Some(_) => retired.removed.push(object.clone()),
                None => retired.retained.push(object.clone()),
            }
        }
        if !retired.removed.is_empty() {
            self.stamp_eviction()?;
        }
        *collections += 1;
        Ok(retired)
    }

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
    pub(super) fn objects(&self) -> Result<HashMap<String, (u64, SystemTime)>> {
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

    /// The caller holds the collection fence for the whole mark and sweep.
    pub(super) fn collect_locked(&self, need: u64, older_than: Duration) -> Result<Collected> {
        let now = SystemTime::now();
        let cutoff = now.checked_sub(self.grace).unwrap_or(SystemTime::UNIX_EPOCH);
        self.restore_trash()?;
        let mut marked = self.roots()?;
        let mut uses = self.uses(&marked)?;
        let objects = self.objects()?;
        let (dropped, evicted, lanes) = self.drop_uses(&mut uses, &marked, &objects, need, older_than, cutoff)?;
        for kept in uses {
            marked.extend(kept.digests);
            marked.insert(kept.name);
        }
        // Unreferenced objects past the grace, and what the dropped lanes alone kept, now: what
        // the grace protects is an object not yet named, never one a dropped lane named.
        let mut collected = Collected { uses: dropped.into_iter().collect(), evicted: lanes, ..Collected::default() };
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
        if collected.objects > 0 {
            self.stamp_eviction()?;
        }
        if collected.bytes > 0 {
            let evicted = collected.evicted.len();
            tracing::info!("tensorfs: freed {} in {} objects ({evicted} models evicted, the rest unreferenced)", size(collected.bytes), collected.objects);
        }
        Ok(collected)
    }

    /// Removes the lanes `need` asks for from the store and from `uses`, each logged: their
    /// names, and the objects only they kept, which go now. The least recently used unused for
    /// `older_than` go first, and only when, with the unreferenced objects past the grace, they
    /// free `need` (`u64::MAX`: every one, the TTL's). A lane a root holds any of stays.
    #[allow(clippy::type_complexity)]
    fn drop_uses(
        &self,
        uses: &mut Vec<Use>,
        roots: &BTreeSet<String>,
        objects: &HashMap<String, (u64, SystemTime)>,
        need: u64,
        older_than: Duration,
        cutoff: SystemTime,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>, Vec<Evicted>)> {
        let none = (BTreeSet::new(), BTreeSet::new(), vec![]);
        let Some(since) = SystemTime::now().checked_sub(older_than).filter(|_| need > 0) else {
            return Ok(none);
        };
        let lanes = lanes(uses);
        let mut count = keepers(&lanes);
        let length = |sha: &str| objects.get(sha).map_or(0, |(length, _)| *length);
        let old = |sha: &str| objects.get(sha).is_some_and(|(_, modified)| *modified < cutoff);
        let orphan = |sha: &&String| !count.contains_key(sha.as_str()) && !roots.contains(*sha) && old(sha);
        let mut free: u64 = objects.keys().filter(orphan).map(|sha| length(sha)).sum();
        let (mut plan, mut evicted) = (vec![], BTreeSet::new());
        // A lane a root holds is in use (a running or queued call's): what it shares with one that
        // is held stays either way, as everything a root holds does.
        let in_use: Vec<bool> = lanes.iter().map(|l| {
            l.uses.iter().any(|u| roots.contains(&u.name)) || l.digests.iter().any(|d| count[d] == 1 && roots.contains(*d))
        }).collect();
        for (lane, _) in lanes.iter().zip(&in_use).filter(|(l, held)| l.used <= since && !**held) {
            if free >= need {
                break;
            }
            let mut bytes = 0;
            for digest in &lane.digests {
                let n = count.get_mut(digest).expect("a lane counts what it keeps");
                *n -= 1;
                if *n == 0 && length(digest) > 0 {
                    bytes += length(digest);
                    evicted.insert(digest.to_string());
                }
            }
            free = free.saturating_add(bytes);
            plan.push((lane, bytes));
        }
        if free < need && need != u64::MAX {
            return Ok(none);
        }
        let names = if plan.is_empty() { HashMap::new() } else { self.names() };
        let mut dropped_lanes = vec![];
        for (lane, bytes) in &plan {
            for u in &lane.uses {
                self.remove_record(&self.root.join("uses").join(&u.name))?;
            }
            let name = lane.uses.iter().find_map(|u| names.get(&u.name)).cloned()
                .unwrap_or_else(|| format!("cache entry {}", &lane.uses[0].name[..12]));
            let idle = lane.used.elapsed().unwrap_or_default();
            let why = if need == u64::MAX { "past its TTL".into() } else { format!("the disk {} short", size(need)) };
            tracing::info!("tensorfs: evicted {name}: {}, idle {} min, {why}", size(*bytes), idle.as_secs() / 60);
            dropped_lanes.push(Evicted { name, bytes: *bytes, idle });
        }
        let dropped: BTreeSet<String> = plan.iter().flat_map(|(lane, _)| lane.uses.iter().map(|u| u.name.clone())).collect();
        uses.retain(|u| !dropped.contains(&u.name));
        Ok((dropped, evicted, dropped_lanes))
    }

    /// Each model lane the refs name, `org/name@release lane`, by its commit's digest and its
    /// header's: what a use is, for people.
    pub(super) fn names(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for model in self.refs().unwrap_or_default() {
            let lanes = model.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, tree)| (format!("{}@{} {lane}", model.name, r.version), tree)));
            for (name, tree) in lanes.chain(model.checkpoints.iter().map(|tree| (model.name.clone(), tree))) {
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

    /// Removes what fetches that ended unfinished kept, those running aside. Their ranges hold no
    /// lock: a fetch that finds one gone asks for it again.
    pub(super) fn drop_partials(&self) -> Result<()> {
        for entry in list(&self.root.join("tmp"))? {
            let partial = entry.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("partial-"));
            if partial {
                if let Some(_held) = exclusive(&entry)? {
                    let _ = fs::remove_file(&entry);
                }
            }
        }
        Ok(())
    }

    /// Removes the temps of puts and commits that died before the grace, and what fetches that
    /// ended unfinished kept. Directories under `tmp/` belong to the layers that made them.
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

#[cfg(test)]
#[path = "removal_tests.rs"]
mod removal_tests;

/// Bytes for people: MiB to a tenth below a GiB, else GiB to a hundredth.
fn size(bytes: u64) -> String {
    match bytes < 1 << 30 {
        true => format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64),
        false => format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64),
    }
}
