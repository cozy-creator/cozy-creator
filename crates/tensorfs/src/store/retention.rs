//! One retention regime for every store. What roots an object:
//!
//! - a live hold: an open checkpoint, a writer, an operation in progress (`holds/<id>`, the
//!   digests it holds, under its holder's `flock`; a dead holder's file is reaped);
//! - a header file a Writer wrote or `open(path)` opened, while that file still holds that header
//!   (`files/<sha256 of the path>`);
//! - a `local/` model ref.
//!
//! Everything else is cache. A use (`uses/<name>`: the digests it keeps, its mtime the last use)
//! keeps them and `snapshots/<name>/` for [`TTL`]; a use a live hold names is in use now. On a low
//! disk the least recently used go first. Nothing is pinned, by hand or forever.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::gc::Collected;
use super::{fail, invalid, is_digest, platform, Result, Store};
use crate::format::{Header, ObjectRef, Model};

/// How long a use keeps what it names.
pub(crate) const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How often maintenance expires uses.
const SWEEP: Duration = Duration::from_secs(10 * 60);

/// Digests kept while a process holds them: never unlocked by hand, so a forked child that
/// inherited the descriptor keeps holding them until it exits or execs.
#[derive(Debug)]
pub struct Hold {
    store: Store,
    file: File,
    held: Mutex<BTreeSet<String>>,
}

impl Hold {
    pub(crate) fn new(store: &Store, digests: impl IntoIterator<Item = String>) -> Result<Arc<Hold>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _fence = acquire(store)?;
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = store.root.join("holds").join(format!("{}-{n}", std::process::id()));
            // Windows locks need read or write access, which append alone does not grant.
            let file = match OpenOptions::new().read(true).append(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(fail("create", &path)(e)),
            };
            file.lock().map_err(fail("lock", &path))?;
            let hold = Hold { store: store.clone(), file, held: Mutex::default() };
            hold.extend(digests)?;
            return Ok(Arc::new(hold));
        }
    }

    /// The short fence over reading metadata and holding what it names.
    pub(crate) fn acquire(&self) -> Result<Fence> {
        acquire(&self.store)
    }

    pub(crate) fn extend(&self, digests: impl IntoIterator<Item = String>) -> Result<()> {
        let _fence = acquire(&self.store)?;
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let mut lines = String::new();
        for digest in digests {
            if !held.contains(&digest) {
                lines.push_str(&digest);
                lines.push('\n');
                held.insert(digest);
            }
        }
        (&self.file)
            .write_all(lines.as_bytes())
            .map_err(fail("write", self.store.root.join("holds")))
    }
}

/// Held shared by whoever reads metadata and then holds or uses what it names; exclusive by a
/// collection, which skips while any is held and never waits.
#[derive(Debug)]
pub(crate) struct Fence(#[allow(dead_code)] File);

pub(super) fn fence_file(store: &Store) -> Result<File> {
    let path = store.root.join("holds").join("gc.lock");
    let opened = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path);
    opened.map_err(fail("open", &path))
}

pub(crate) fn acquire(store: &Store) -> Result<Fence> {
    let file = fence_file(store)?;
    file.lock_shared().map_err(fail("lock", &store.root))?;
    Ok(Fence(file))
}

pub(crate) fn try_collect(store: &Store) -> Result<Option<Fence>> {
    let file = fence_file(store)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Fence(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(fail("lock", &store.root)(e)),
    }
}

/// `files/<id>`: a header file outside the store's own data.
#[derive(Serialize, Deserialize)]
struct FileRoot {
    path: PathBuf,
    header: ObjectRef,
}

/// The header `root`'s file holds, when it still holds it; an unreadable file still holds it.
fn file_header(root: &FileRoot) -> Option<Option<Header>> {
    let kept = |e: io::Error| !matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory);
    match fs::metadata(&root.path) {
        Ok(meta) if !meta.is_file() || meta.len() != root.header.length => None,
        Ok(_) => match fs::read(&root.path) {
            Ok(bytes) if ObjectRef::of(&bytes) == root.header => Some(Header::from_cbor(&bytes).ok()),
            Ok(_) => None,
            Err(e) => kept(e).then_some(None),
        },
        Err(e) => kept(e).then_some(None),
    }
}

/// The lines of `path` (none when absent).
fn lines(path: &Path) -> Result<Option<BTreeSet<String>>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.lines().map(str::to_owned).collect())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(fail("read", path)(e)),
    }
}

pub(super) fn list(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(fail("list", dir)(e)),
    };
    entries.map(|e| e.map(|e| e.path()).map_err(fail("list", dir))).collect()
}

/// A recorded use: its name and when it was last used, and what it keeps.
pub(super) struct Use {
    pub name: String,
    pub used: SystemTime,
    pub digests: BTreeSet<String>,
}

impl Store {
    pub fn hold(&self, objects: &[ObjectRef]) -> Result<Arc<Hold>> {
        Hold::new(self, objects.iter().map(|o| o.sha256.clone()))
    }

    /// Records a use of `objects` under `name` (a digest): they and `snapshots/<name>/` stay for
    /// the TTL after the last use. A use only ever adds what it keeps.
    pub fn used<'a>(&self, name: &str, objects: impl IntoIterator<Item = &'a ObjectRef>) -> Result<()> {
        let path = self.root.join("uses").join(name);
        if !is_digest(name) {
            return Err(invalid(&path, "a use is named by a digest"));
        }
        let _fence = acquire(self)?;
        let _lock = self.lock("uses")?;
        let old = lines(&path)?;
        let mut keep = old.clone().unwrap_or_default();
        keep.extend(objects.into_iter().map(|o| o.sha256.clone()));
        if old.as_ref() == Some(&keep) {
            return touch(&path);
        }
        let text: String = keep.iter().map(|d| format!("{d}\n")).collect();
        self.commit(&path, text.as_bytes(), false)
    }

    /// Roots `header` while the file at `path` (absolute) holds it: one record per path, so saving
    /// another header there replaces it.
    pub fn root_file(&self, path: &Path, header: &ObjectRef) -> Result<()> {
        let path = platform::durable::canonical(path).map_err(|e| fail("resolve", path)(e.into()))?;
        let id = ObjectRef::of(path.as_os_str().as_encoded_bytes()).sha256;
        let record = self.root.join("files").join(id);
        let root = FileRoot { path, header: header.clone() };
        let bytes = serde_json::to_vec(&root).map_err(|e| invalid(&record, e))?;
        let _fence = acquire(self)?;
        self.commit(&record, &bytes, true)
    }

    /// What the roots keep, each digest once; reaps dead holds and files that let go.
    pub(super) fn roots(&self) -> Result<BTreeSet<String>> {
        Ok(self.holders()?.into_keys().collect())
    }

    /// What the roots keep, with the processes whose live holds keep each: 0 for a header file
    /// or a `local/` ref, which no process ending lets go. Reaps dead holds and files that let go.
    pub(super) fn holders(&self) -> Result<HashMap<String, BTreeSet<u32>>> {
        self.holders_except(None)
    }

    /// A targeted removal releases its own local ref, but no other retention source.
    pub(super) fn holders_except(&self, removed_ref: Option<&str>) -> Result<HashMap<String, BTreeSet<u32>>> {
        let mut roots: HashMap<String, BTreeSet<u32>> = HashMap::new();
        let mut root = |digest: String, pid: u32| {
            roots.entry(digest).or_default().insert(pid);
        };
        for path in list(&self.root.join("holds"))? {
            let Some(name) = path.file_name().and_then(|n| n.to_str()).filter(|n| *n != "gc.lock") else {
                continue;
            };
            // `<pid>-<n>` (`Hold::new`).
            let pid = name.split('-').next().and_then(|p| p.parse().ok()).unwrap_or(0);
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(fail("open", &path)(e)),
            };
            match file.try_lock() {
                Ok(()) => drop(fs::remove_file(&path)),
                Err(TryLockError::WouldBlock) => lines(&path)?.unwrap_or_default().into_iter().for_each(|d| root(d, pid)),
                Err(TryLockError::Error(e)) => return Err(fail("lock", &path)(e)),
            }
        }
        for record in list(&self.root.join("files"))? {
            let bytes = fs::read(&record).map_err(fail("read", &record))?;
            let file: FileRoot = serde_json::from_slice(&bytes).map_err(|e| invalid(&record, e))?;
            match file_header(&file) {
                Some(header) => {
                    header.iter().flat_map(Header::objects).for_each(|o| root(o.sha256.clone(), 0));
                    root(file.header.sha256.clone(), 0);
                    // A header file inside a snapshot keeps its folder.
                    if let Ok(inside) = file.path.strip_prefix(self.root.join("snapshots")) {
                        inside.iter().next().and_then(|n| n.to_str()).into_iter().for_each(|n| root(n.to_owned(), 0));
                    }
                }
                None => drop(self.remove_record(&record)?),
            }
        }
        for model in self.refs()?.iter().filter(|r| local(r) && removed_ref.is_none_or(|name| !r.name.eq_ignore_ascii_case(name))) {
            for tree in model.roots() {
                self.closure(tree)?.into_iter().for_each(|o| root(o.sha256, 0));
            }
        }
        Ok(roots)
    }

    /// Every recorded use; one a root names is used now.
    pub(super) fn uses(&self, roots: &BTreeSet<String>) -> Result<Vec<Use>> {
        let mut uses = Vec::new();
        for path in list(&self.root.join("uses"))? {
            let Some(name) = path.file_name().and_then(|n| n.to_str()).filter(|n| is_digest(n)) else {
                continue;
            };
            let Some(digests) = lines(&path)? else { continue };
            let used = if roots.contains(name) {
                touch(&path)?;
                SystemTime::now()
            } else {
                fs::metadata(&path).and_then(|m| m.modified()).map_err(fail("stat", &path))?
            };
            uses.push(Use { name: name.to_owned(), used, digests });
        }
        Ok(uses)
    }

    /// The cache's upkeep, run as bytes come in and checkpoints open: once per sweep period,
    /// expire uses past the TTL; then, while the disk is short of `incoming` bytes and a reserve,
    /// drop the least recently used. False when `incoming` still does not fit: skip the write.
    /// An origin's handle runs none. A handle collects once at a time: an admit that finds one
    /// collecting waits for it and judges the disk by what it freed, never refused meanwhile.
    /// Short, the machine's garbage goes first (`on_short`), then the store's own.
    pub fn maintain(&self, incoming: u64) -> Result<bool> {
        if !self.cache {
            return Ok(true);
        }
        let short = || shortfall(&self.root, incoming);
        let stamp = self.root.join("uses").join(".swept");
        let due = || {
            fs::metadata(&stamp).and_then(|m| m.modified()).map_or(true, |t| t.elapsed().map_or(true, |age| age >= SWEEP))
        };
        if !due() && short()? == 0 {
            return Ok(true);
        }
        let seen = *self.collections.lock().unwrap_or_else(PoisonError::into_inner);
        let mut collections = self.collections.lock().unwrap_or_else(PoisonError::into_inner);
        if *collections == seen {
            // The machine's garbage goes before any model (`on_short`).
            if let (Some(reclaim), 1..) = (self.reclaim.get(), short()?) {
                (reclaim.0)(incoming);
            }
            if let Some(_gc) = try_collect(self)? {
                // A store GC cannot walk (a ref or header of another format) keeps all it holds.
                if let Err(e) = self.upkeep(due(), &stamp, &short) {
                    tracing::warn!("tensorfs: cache upkeep skipped: {e}");
                }
            }
            *collections += 1;
        }
        drop(collections);
        Ok(short()? == 0)
    }

    /// Frees room for `incoming` bytes now, as an admit that finds the disk short would: what cut
    /// fetches kept, then the least recently used lanes nothing holds, until it fits. What it
    /// dropped, for people. Never from inside `on_short`'s reclaim, which a collection runs.
    pub fn make_room(&self, incoming: u64) -> Result<Collected> {
        if !self.cache {
            return Ok(Collected::default());
        }
        let short = || shortfall(&self.root, incoming);
        let mut collections = self.collections.lock().unwrap_or_else(PoisonError::into_inner);
        // Waits out readers that are taking holds, so what they hold is judged.
        let fence = fence_file(self)?;
        fence.lock().map_err(fail("lock", &self.root))?;
        let mut all = Collected::default();
        if short()? > 0 {
            self.drop_partials()?;
        }
        loop {
            let need = short()?;
            if need == 0 {
                break;
            }
            let collected = self.collect_locked(need, Duration::ZERO)?;
            let done = collected.uses.is_empty();
            all.add(collected);
            if done {
                break;
            }
        }
        *collections += 1;
        Ok(all)
    }

    /// What the cache could give up now (`Room`): each lane, what it keeps and who holds it, and
    /// what nothing keeps (unreferenced objects past the grace, cut fetches no fetch holds).
    pub fn room(&self) -> Result<Room> {
        let _fence = acquire(self)?;
        let holders = self.holders()?;
        let roots: BTreeSet<String> = holders.keys().cloned().collect();
        let uses = self.uses(&roots)?;
        let objects = self.objects()?;
        let names = self.names();
        let cutoff = SystemTime::now().checked_sub(self.grace).unwrap_or(SystemTime::UNIX_EPOCH);
        let lanes = super::gc::lanes(&uses);
        let count = super::gc::keepers(&lanes);
        let kept: BTreeSet<&str> = count.keys().copied().collect();
        let mut spare: u64 = objects.iter()
            .filter(|(sha, (_, modified))| *modified < cutoff && !kept.contains(sha.as_str()) && !roots.contains(*sha))
            .map(|(_, (length, _))| length).sum();
        for entry in list(&self.root.join("tmp"))? {
            if entry.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("partial-")) {
                let unheld = File::open(&entry).is_ok_and(|f| f.try_lock().is_ok());
                spare += unheld.then(|| fs::metadata(&entry).map(|m| platform::durable::allocated(&m)).unwrap_or(0)).unwrap_or(0);
            }
        }
        let lanes = lanes.iter().map(|lane| RoomLane {
            name: lane.uses.iter().find_map(|u| names.get(&u.name)).cloned()
                .unwrap_or_else(|| format!("cache entry {}", &lane.uses[0].name[..12])),
            used: lane.used,
            holders: holders.iter().filter(|(digest, _)| lane.held_by(digest, &count)).flat_map(|(_, pids)| pids.iter().copied()).collect(),
            digests: lane.digests.iter().map(|d| d.to_string()).collect(),
        }).collect();
        let lengths = objects.into_iter().map(|(sha, (length, _))| (sha, length)).collect();
        Ok(Room { lanes, spare, lengths })
    }

    /// When a collection last removed an object (`stamp_eviction`), in any process: what was
    /// known to be here may not be since.
    pub fn evicted_at(&self) -> Option<SystemTime> {
        fs::metadata(self.root.join("uses").join(".evicted")).and_then(|m| m.modified()).ok()
    }

    pub(super) fn stamp_eviction(&self) -> Result<()> {
        let path = self.root.join("uses").join(".evicted");
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(fail("create", &path))?;
        file.set_modified(SystemTime::now()).map_err(fail("touch", &path))
    }

    fn upkeep(&self, due: bool, stamp: &Path, short: &dyn Fn() -> Result<u64>) -> Result<()> {
        if due {
            let created = File::create(stamp).and_then(|f| f.set_modified(SystemTime::now()));
            created.map_err(fail("create", stamp))?;
            self.sweep_writers(TTL)?;
            self.collect_locked(u64::MAX, TTL)?;
        }
        // What a cut fetch kept can be asked for again, so it goes before any model.
        if short()? > 0 {
            self.drop_partials()?;
        }
        loop {
            let need = short()?;
            if need == 0 || self.collect_locked(need, Duration::ZERO)?.uses.is_empty() {
                return Ok(());
            }
        }
    }
}

/// What a store could give up for bytes coming in (`Store::room`).
pub struct Room {
    /// Each lane, least recently used first.
    pub lanes: Vec<RoomLane>,
    /// What nothing keeps, freed by any collection.
    pub spare: u64,
    lengths: HashMap<String, u64>,
}

/// One lane of a `Room`: its model as people name it, its last use, and the processes whose live
/// holds keep it (0: a root no process ending lets go).
pub struct RoomLane {
    pub name: String,
    pub used: SystemTime,
    pub holders: BTreeSet<u32>,
    digests: BTreeSet<String>,
}

impl Room {
    /// The bytes dropping every lane `may` takes would free, with what nothing keeps: an object
    /// goes only once no other lane keeps it.
    pub fn freed(&self, may: impl Fn(&RoomLane) -> bool) -> u64 {
        let kept: BTreeSet<&str> = self.lanes.iter().filter(|l| !may(l)).flat_map(|l| l.digests.iter().map(String::as_str)).collect();
        let gone: BTreeSet<&str> = self.lanes.iter().filter(|l| may(l)).flat_map(|l| l.digests.iter().map(String::as_str)).filter(|d| !kept.contains(d)).collect();
        self.spare + gone.iter().map(|d| self.lengths.get(*d).copied().unwrap_or(0)).sum::<u64>()
    }
}

/// `local/` repos are never evicted.
fn local(model: &Model) -> bool {
    model.name.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("local/"))
}

/// Bytes missing for `incoming` and a reserve of 2% of the disk, within 1 to 10 GiB.
fn shortfall(root: &Path, incoming: u64) -> Result<u64> {
    let space = platform::durable::space(root).map_err(platform("measure space", root))?;
    let reserve = (space.total / 50).clamp(1 << 30, 10 << 30);
    Ok(reserve.saturating_add(incoming.max(1)).saturating_sub(space.free))
}

fn touch(path: &Path) -> Result<()> {
    let file = OpenOptions::new().append(true).open(path).map_err(fail("open", path))?;
    file.set_modified(SystemTime::now()).map_err(fail("touch", path))
}
