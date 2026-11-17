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
/// commit a snapshot keeps) is that one's lane; one that keeps only its own object (a run's input
/// or output) is its own, whatever model holds the same bytes. Least recently used first.
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
    // A run's object equal to a model's file (H3's tokenizer, in the package run from source) made the model's whole lane
    // the run's, held by TensorD while it keeps the run's objects (group-disk 43875722356).
    let alone = |u: &Use| u.digests.len() == 1 && u.digests.contains(&u.name);
    for (i, u) in uses.iter().enumerate() {
        for j in u.digests.iter().filter_map(|d| at.get(d.as_str()).copied()).filter(|&j| j == i || !alone(&uses[j])) {
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
    /// roots, live holds, authoritative peer requests and open file readers. A use past the
    /// TTL keeps nothing: only a root kept it from the last collection, and what a root holds
    /// is retained above. A use naming candidates beside nothing but manifests listing only
    /// candidates (a machine's record of a run's held files, `[manifest, files]`) goes with
    /// them, its manifests too; one naming anything else keeps what it names.
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
        let expired = SystemTime::now().checked_sub(super::retention::TTL).unwrap_or(SystemTime::UNIX_EPOCH);
        let mut manifests = BTreeSet::new();
        for used in uses {
            let others: Vec<&String> = used.digests.iter().filter(|digest| !candidates.contains(*digest)).collect();
            let free = !keep.contains(&used.name) && used.digests.iter().all(|digest| !keep.contains(digest));
            let owned = requested.contains(&used.name) && free && others.is_empty();
            let listing = free && others.len() < used.digests.len() && !others.is_empty()
                && others.iter().all(|digest| self.lists_only(digest, &candidates));
            if owned || listing {
                self.remove_record(&self.root.join("uses").join(&used.name))?;
                manifests.extend(others.into_iter().cloned());
                retired.removed_uses.push(used.name);
            } else if used.used >= expired {
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
        for manifest in manifests.into_iter().filter(|manifest| !keep.contains(manifest)) {
            match self.discard(&manifest, before)? {
                Some(length) => retired.removed.push(ObjectRef { sha256: manifest, length }),
                // A reader still has it: kept, said, for the caller to retire again.
                None => if let Ok(metadata) = fs::metadata(self.blob(&manifest)?) {
                    retired.retained.push(ObjectRef { sha256: manifest, length: metadata.len() });
                }
            }
        }
        if !retired.removed.is_empty() {
            self.stamp_eviction()?;
        }
        *collections += 1;
        Ok(retired)
    }

    /// Whether `digest` is a small JSON document listing files by their digests (a tree, or a machine's manifest of a run's held
    /// files), every one of them in `files`.
    fn lists_only(&self, digest: &str, files: &BTreeSet<String>) -> bool {
        fn named(value: &serde_json::Value, found: &mut BTreeSet<String>) {
            match value {
                serde_json::Value::Object(map) => map.iter().for_each(|(key, value)| match (key.as_str(), value) {
                    ("sha256", serde_json::Value::String(digest)) => { found.insert(digest.trim_start_matches("sha256:").to_owned()); }
                    _ => named(value, found),
                }),
                serde_json::Value::Array(items) => items.iter().for_each(|item| named(item, found)),
                _ => {}
            }
        }
        let Ok(path) = self.blob(digest) else { return false };
        let Ok(bytes) = fs::read(&path) else { return false };
        if bytes.len() > 1 << 16 || bytes.first() != Some(&b'{') { return false; }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return false };
        let mut found = BTreeSet::new();
        named(&value, &mut found);
        !found.is_empty() && found.is_subset(files)
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
        self.collect(need, older_than, false)
    }

    /// As `collect_locked`, of the checkpoints only a guess brought (`Store::guess`) alone, least recently used first, as far
    /// as `need` asks: they go whether or not they free it all.
    pub(super) fn collect_guessed(&self, need: u64) -> Result<Collected> {
        // With none, as on any store no guess has reached, there is nothing to walk for.
        if self.guesses().is_empty() {
            return Ok(Collected::default());
        }
        self.collect(need, Duration::ZERO, true)
    }

    fn collect(&self, need: u64, older_than: Duration, guessed_only: bool) -> Result<Collected> {
        let now = SystemTime::now();
        let cutoff = now.checked_sub(self.grace).unwrap_or(SystemTime::UNIX_EPOCH);
        self.restore_trash()?;
        let mut marked = self.roots()?;
        let mut uses = self.uses(&marked)?;
        let objects = self.objects()?;
        let (dropped, evicted, lanes) = self.drop_uses(&mut uses, &marked, &objects, need, older_than, cutoff, guessed_only)?;
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
    /// free `need` (`u64::MAX`: every one, the TTL's). A lane a root holds any of stays. What only a guess brought
    /// (`Store::guess`) goes before any a call read; `guessed_only`: those alone, as far as `need` asks.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn drop_uses(
        &self,
        uses: &mut Vec<Use>,
        roots: &BTreeSet<String>,
        objects: &HashMap<String, (u64, SystemTime)>,
        need: u64,
        older_than: Duration,
        cutoff: SystemTime,
        guessed_only: bool,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>, Vec<Evicted>)> {
        let none = (BTreeSet::new(), BTreeSet::new(), vec![]);
        let Some(since) = SystemTime::now().checked_sub(older_than).filter(|_| need > 0) else {
            return Ok(none);
        };
        let guessed = self.guesses();
        let is_guess = |lane: &Lane<'_>| lane.digests.iter().any(|digest| guessed.contains(*digest));
        let mut lanes = lanes(uses);
        lanes.sort_by_key(|lane| !is_guess(lane));
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
        for (lane, _) in lanes.iter().zip(&in_use).filter(|(l, held)| l.used <= since && !**held && (!guessed_only || is_guess(*l))) {
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
        if free < need && need != u64::MAX && !guessed_only {
            return Ok(none);
        }
        let names = if plan.is_empty() { HashMap::new() } else { self.names() };
        let mut dropped_lanes = vec![];
        for (lane, bytes) in &plan {
            for u in &lane.uses {
                self.remove_record(&self.root.join("uses").join(&u.name))?;
            }
            for digest in lane.digests.iter().filter(|digest| guessed.contains(**digest)) {
                let _ = fs::remove_file(self.root.join("uses").join(super::retention::GUESSED).join(digest));
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

    /// Derived pages (`Store::keep_derived`), least recently used first, while `short()` says the
    /// disk needs bytes; what they freed. A bake is made again from bytes already on this disk, so
    /// a short disk gives them up before any model, whatever lane keeps them: a lane in use is never
    /// dropped, and its bakes of every LoRA it ran with held 61 GB of kamitsure's 200 (cl-1081). One a
    /// reader has open stays (`discard`).
    pub(super) fn drop_derived(&self, short: &dyn Fn() -> Result<u64>) -> Result<u64> {
        self.restore_trash()?;
        let mut freed = 0;
        for (page, object) in self.derived_pages()? {
            if short()? == 0 {
                break;
            }
            if let Some(object) = &object {
                match self.discard(&object.sha256, SystemTime::now())? {
                    Some(bytes) => freed += bytes,
                    None if self.holds(object) => continue, // a reader has it open
                    None => {}
                }
            }
            self.remove_record(&page)?;
        }
        let _ = fs::remove_dir(self.root.join("tmp").join("gc"));
        if freed > 0 {
            self.stamp_eviction()?;
            tracing::info!("tensorfs: freed {} of derived pages, least recently used first", size(freed));
        }
        Ok(freed)
    }

    /// Every derived page's record and the object it names (None: not one), least recently used first.
    pub(super) fn derived_pages(&self) -> Result<Vec<(std::path::PathBuf, Option<ObjectRef>)>> {
        let mut pages: Vec<(SystemTime, std::path::PathBuf)> = Vec::new();
        for slot in list(&self.root.join("derived"))? {
            for page in list(&slot)? {
                if page.file_name().and_then(|n| n.to_str()).is_some_and(is_digest) {
                    let used = fs::metadata(&page).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    pages.push((used, page));
                }
            }
        }
        pages.sort();
        let mut named = Vec::with_capacity(pages.len());
        for (_, page) in pages {
            match fs::read(&page) {
                Ok(bytes) => named.push((page, serde_json::from_slice::<ObjectRef>(&bytes).ok())),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(fail("read", &page)(error)),
            }
        }
        Ok(named)
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
pub(super) fn size(bytes: u64) -> String {
    match bytes < 1 << 30 {
        true => format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64),
        false => format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64),
    }
}
