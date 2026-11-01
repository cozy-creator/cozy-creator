//! The machine's caches manage themselves, with no purge verb. Each expires a TTL after its
//! last use. A disk is low when no more than its reserve is free (`Disk`). The TensorFS store
//! evicts on its own: orphans, then idle model lanes, least recently used first.
//! On a low disk a sweep drops, least recently used first until it is not low: uv cache
//! entries no environment links, collected result copies, settled logs, memoized stages,
//! update downloads, unused generations (then the uv entries only they linked), and compiled
//! kernels last, only when they alone lift it. Garbage goes every sweep once idle: a generation
//! whose build failed, an editable environment no generation names, a Runtime pair neither
//! selected nor kept to roll back to. Only bytes an unlink frees on the measured filesystem count:
//! single-link files this process does not hold open, on that filesystem. Optional cache
//! writes are skipped on a low disk. Never evicted: uncollected results (durable outputs),
//! journal rows, a generation a run holds or an installation names, a uv entry an environment
//! links, a kernel namespace a live executor or kernel boot holds. Each sweep logs what it
//! freed and its bytes, and measures what fills the disk by kind (`usage`).
use crate::{catalog::Catalog, execution::Engine};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};
use platform::durable::{self, FileId};

pub const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Installed or used within one sweep period counts as in use, even on a low disk: an
/// install is never evicted between its creation and its first run or binding.
pub const IDLE: Duration = Duration::from_secs(10 * 60);

/// 1/50 of the filesystem, at least 1 GiB and at most 10 GiB.
const RESERVE_BYTES: (u64, u64) = (1 << 30, 10 << 30);
const RESERVE_DIVISOR: u64 = 50;
const RESERVE_INODES: u64 = 32;

/// One filesystem: its size and what is free on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    pub capacity: u64,
    pub available: u64,
    /// Zero when the filesystem has no inode limit.
    pub inodes: u64,
    pub available_inodes: u64,
}

impl Disk {
    pub fn measure(root: &Path) -> io::Result<Disk> {
        let space = platform::durable::space(root).map_err(|error| {
            io::Error::other(format!("cannot measure the filesystem under {}: {error}", root.display()))
        })?;
        Ok(Disk {
            capacity: space.total,
            available: space.free,
            inodes: space.inodes,
            available_inodes: space.free_inodes,
        })
    }

    /// Never written into by an optional cache.
    pub fn reserve(&self) -> u64 {
        (self.capacity / RESERVE_DIVISOR).clamp(RESERVE_BYTES.0, RESERVE_BYTES.1)
    }

    /// A low disk has no more than its reserve free: the bytes to free before it has more.
    pub fn short(&self) -> Option<u64> {
        let low = self.available <= self.reserve()
            || (self.inodes > 0 && self.available_inodes <= RESERVE_INODES);
        low.then(|| (self.reserve() + 1).saturating_sub(self.available).max(1))
    }
    /// What a write may still take: what is free past the reserve.
    pub fn free(&self) -> u64 {
        self.available.saturating_sub(self.reserve())
    }
}

/// The filesystem under `path`, now.
pub fn measure(path: &Path) -> io::Result<Disk> {
    Disk::measure(path)
}

/// What one sweep removed: entries and bytes by kind, and the package environments by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    kinds: BTreeMap<Kind, (usize, u64)>,
    environments: Vec<String>,
}

impl Swept {
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub(crate) fn add(&mut self, kind: Kind, count: usize, bytes: u64) {
        if count > 0 {
            let entry = self.kinds.entry(kind).or_default();
            (entry.0, entry.1) = (entry.0 + count, entry.1 + bytes);
        }
    }
}

impl std::fmt::Display for Swept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kinds: Vec<String> = self.kinds.iter().map(|(kind, (n, bytes))| format!("{n} {} {}", kind.name(), size(*bytes))).collect();
        write!(f, "{}", kinds.join(", "))?;
        match self.environments.is_empty() {
            true => Ok(()),
            false => write!(f, " (environments: {})", self.environments.join(", ")),
        }
    }
}

/// Bytes for people: MiB to a tenth below a GiB, else GiB to a hundredth.
pub fn size(bytes: u64) -> String {
    match bytes < 1 << 30 {
        true => format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64),
        false => format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64),
    }
}

/// The persistent compiled-kernel store (`Seal::prepare`'s `<root>/u<uid>/`). Every executor
/// and kernel boot holds its namespace's lock (`kernel_hold`) shared until it exits; a sweep
/// touches a namespace only while it holds that lock exclusively.
pub struct KernelCaches {
    pub root: PathBuf,
}

/// The lock of one kernel namespace, beside it where its identity cannot reach.
pub fn kernel_hold(kernels: &Path, namespace: &str) -> io::Result<File> {
    File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(kernels.join(format!(".{namespace}.hold")))
}

/// Everything one sweep looks at.
pub struct Caches<'a> {
    pub engine: &'a Engine,
    pub catalog: &'a Catalog,
    /// Generations installations or configured packages still name.
    pub bound: &'a HashSet<String>,
    pub kernels: Option<&'a KernelCaches>,
    pub memo: Option<&'a crate::memo::Memo>,
    /// The uv cache the machine manages (`Publisher::uv_cache`), and where the environments
    /// that may link it are: generations, editable environments, the toolchain.
    pub uv_cache: Option<&'a Path>,
    pub environments: &'a [PathBuf],
    /// The filesystem the machine's state is on (`measure`; a test's own).
    pub disk: &'a dyn Fn() -> io::Result<Disk>,
}

/// What a sweep drops, in a low disk's order: plan entries from `Result` to `Kernel`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Kind {
    Staging,
    Object,
    UvCache,
    Editable,
    Pair,
    Result,
    Log,
    Memo,
    Update,
    Generation,
    Kernel,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Staging => "staging spools",
            Kind::Object => "run objects",
            Kind::UvCache => "uv cache entries",
            Kind::Result => "result copies",
            Kind::Log => "logs",
            Kind::Memo => "memoized stages",
            Kind::Editable => "editable environments no generation names",
            Kind::Pair => "Runtime pairs no longer selected",
            Kind::Update => "update downloads",
            Kind::Generation => "package environments",
            Kind::Kernel => "kernel caches",
        }
    }
}

/// One entry a sweep may drop. A generation's exclusive `.hold` lock, once taken, is kept
/// from planning through deletion.
struct Entry {
    kind: Kind,
    path: PathBuf,
    used: SystemTime,
    hold: Option<File>,
    /// The lock its users take (a kernel namespace's `.hold`): taken exclusively around this
    /// entry's removal alone, never across a sweep. Busy, it stays.
    guard: Option<PathBuf>,
}

impl Entry {
    fn remove(&mut self) -> io::Result<bool> {
        let _guard = match &self.guard {
            Some(guard) => match File::options().write(true).open(guard) {
                Ok(lock) if lock.try_lock().is_ok() => Some(lock),
                _ => return Ok(false),
            },
            None => None,
        };
        if self.kind != Kind::Generation {
            return Ok(remove(&self.path));
        }
        if self.hold.is_none() {
            let Ok(hold) = File::open(self.path.join(".hold")) else {
                return Ok(false);
            };
            if hold.try_lock().is_err() {
                return Ok(false);
            }
            self.hold = Some(hold);
        }
        // Once renamed no resolve can find it; the lock is held until it is gone.
        let name = self.path.file_name().unwrap().to_string_lossy();
        let doomed = self.path.with_file_name(format!(".collect-{name}"));
        // Windows renames no directory with a file open under it, its own hold included: the
        // lock proved no holder, and a resolve that takes the hold in between is refused the
        // same way, so a refused rename is a held generation.
        #[cfg(windows)]
        {
            self.hold = None;
            match fs::rename(&self.path, &doomed) {
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
                renamed => renamed?,
            }
        }
        #[cfg(not(windows))]
        fs::rename(&self.path, &doomed)?;
        fs::remove_dir_all(&doomed)?;
        Ok(true)
    }
}

/// Delete generation `identity` under `generations` now (a removed installation's), unless a
/// process holds it: renamed first, so no resolve finds it. The bytes it freed, or None when
/// it is held (an executor of it still runs); reclaim removes it once nothing names it.
pub fn remove_generation(generations: &Path, identity: &str) -> io::Result<Option<u64>> {
    let path = generations.join(identity);
    let Ok(filesystem) = FileId::at(&path).map(|id| id.dev) else {
        return Ok(Some(0));
    };
    let bytes = releasable(&path, filesystem, &open_files()).unwrap_or(0);
    let label = label(&path);
    let mut entry = Entry { kind: Kind::Generation, path, used: SystemTime::now(), hold: None, guard: None };
    let removed = entry.remove()?;
    if removed {
        tracing::info!("removed package environment {label}: {}", size(bytes));
    }
    Ok(removed.then_some(bytes))
}

/// Newest write, or read of a file, anywhere inside: an entry's last use. A read stamps a
/// file's access at most daily (relatime); a filesystem that never does falls back to
/// writes. A directory's own access time is this scan's, so it never counts.
fn last_use(path: &Path) -> SystemTime {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return SystemTime::UNIX_EPOCH;
    };
    let written = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if metadata.is_dir() {
        let children = fs::read_dir(path).into_iter().flatten().flatten();
        children
            .map(|entry| last_use(&entry.path()))
            .fold(written, SystemTime::max)
    } else {
        written.max(metadata.accessed().unwrap_or(SystemTime::UNIX_EPOCH))
    }
}

fn modified(path: &Path) -> SystemTime {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn unused_for(used: SystemTime, age: Duration) -> bool {
    SystemTime::now()
        .duration_since(used)
        .is_ok_and(|unused| unused > age)
}

fn remove(path: &Path) -> bool {
    let removed = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match removed {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!("reclaim {}: {error}", path.display());
            false
        }
    }
}

/// Bytes unlinking `path` would free now (`releasable`), for a removal's log line.
pub fn freeable(path: &Path) -> u64 {
    FileId::at(path).ok().and_then(|id| releasable(path, id.dev, &open_files())).unwrap_or(0)
}

/// Files this process holds open: unlinking one frees nothing yet. Linux lists them; elsewhere
/// none are known, and an open file unlinked there frees its bytes once it closes.
fn open_files() -> HashSet<FileId> {
    let Ok(descriptors) = fs::read_dir("/proc/self/fd") else {
        return HashSet::new();
    };
    descriptors
        .flatten()
        .filter_map(|fd| FileId::at(&fd.path()).ok())
        .collect()
}

/// Bytes unlinking `path` frees on `filesystem`, or None when something under it is open
/// here. Multiply linked files free nothing (uv environments share payload with a cache);
/// symlinks are not followed and another mount is never counted.
fn releasable(path: &Path, filesystem: u64, open: &HashSet<FileId>) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() {
        return Some(0);
    }
    let id = FileId::at(path).ok()?;
    if id.dev != filesystem {
        return Some(0);
    }
    if open.contains(&id) {
        return None;
    }
    let own = durable::allocated(&metadata);
    if metadata.is_dir() {
        fs::read_dir(path)
            .ok()?
            .flatten()
            .try_fold(own, |total, entry| {
                Some(total + releasable(&entry.path(), filesystem, open)?)
            })
    } else if durable::links(path).ok()? == 1 {
        Some(own)
    } else {
        Some(0)
    }
}

/// Kernel entries of every namespace no live process holds, each with its namespace's lock:
/// one compiled kernel (a `triton` or `flash-attn4` entry) or one generation's torch kernels.
/// A namespace is locked only while it is read, and again around each entry's removal.
fn kernel_entries(caches: &KernelCaches) -> Vec<(PathBuf, PathBuf)> {
    let mut entries = vec![];
    let Ok(namespaces) = fs::read_dir(&caches.root) else {
        return entries;
    };
    for namespace in namespaces.flatten() {
        if !namespace.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue; // never through a link, and not a lock file
        }
        let name = namespace.file_name().to_string_lossy().into_owned();
        let lock = kernel_hold(&caches.root, &name);
        let Some(_reading) = lock.ok().filter(|lock| lock.try_lock().is_ok()) else {
            continue;
        };
        let guard = caches.root.join(format!(".{name}.hold"));
        for child in fs::read_dir(namespace.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            match child.file_name().to_str() {
                Some("triton" | "flash-attn4") => entries.extend(
                    fs::read_dir(child.path())
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| (e.path(), guard.clone())),
                ),
                _ => entries.push((child.path(), guard.clone())),
            }
        }
    }
    entries
}

/// The uv cache's unpacked wheels (`archive-v0/<id>`) no environment links, each removed past
/// the TTL since it was last linked or let go (its newest ctime), or on a low disk least
/// recently used first until it is not. Environments hard-link a machine cache's files and
/// symlink an image's seeded one (so overlayfs never copies its payload): an entry with a file
/// linked twice (a hard-linked environment's, the image's own interpreter's) or one an
/// environment under `environments` symlinks stays. uv takes the cache's `.lock` shared for
/// each command and the publisher for a whole environment build; the pass holds it exclusively
/// throughout, and is skipped while either holds it. An entry removed with its pointers is a
/// cache miss to uv, which unpacks the wheel again.
fn uv_pass(cache: &Path, environments: &[PathBuf], disk: &dyn Fn() -> io::Result<Disk>, swept: &mut Swept) -> io::Result<()> {
    let lock = File::options().write(true).open(cache.join(".lock"));
    let Some(_held) = lock.ok().filter(|lock| lock.try_lock().is_ok()) else {
        return Ok(());
    };
    let archives = cache.join("archive-v0");
    let mut linked = HashSet::new();
    for root in environments {
        symlinked(root, &archives, 3, &mut linked);
    }
    let mut pointers = HashMap::new();
    for bucket in fs::read_dir(cache).into_iter().flatten().flatten() {
        if bucket.file_name() != "archive-v0" {
            uv_pointers(&bucket.path(), &mut pointers);
        }
    }
    let unlinked = fs::read_dir(&archives).into_iter().flatten().flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()) && !linked.contains(&entry.file_name()));
    let mut entries: Vec<(SystemTime, PathBuf, Vec<PathBuf>)> = unlinked
        .filter_map(|entry| Some((unlinked_since(&entry.path())?, entry.path(), pointers.remove(&entry.file_name()).unwrap_or_default())))
        .collect();
    entries.sort();
    let filesystem = FileId::at(cache)?.dev;
    for (used, path, pointers) in entries {
        if !unused_for(used, TTL) && disk()?.short().is_none() {
            break;
        }
        let bytes = releasable(&path, filesystem, &HashSet::new()).unwrap_or(0);
        for pointer in &pointers {
            remove_pointer(pointer);
        }
        if remove(&path) {
            swept.add(Kind::UvCache, 1, bytes);
        }
    }
    Ok(())
}

/// The uv archives (`archive-v0/<id>` under `archives`) the environments under `dir` link by
/// symlink: a venv's (`pyvenv.cfg`) or an install target's (the toolchain's) installed
/// distributions, each by its `METADATA`. Looks `depth` directories down, never through a link.
fn symlinked(dir: &Path, archives: &Path, depth: usize, found: &mut HashSet<OsString>) {
    let venv = dir.join("pyvenv.cfg").is_file();
    let site = match venv {
        true => platform::paths::venv_site_packages(dir),
        false => Some(dir.to_path_buf()),
    };
    for entry in site.into_iter().flat_map(|site| fs::read_dir(site).into_iter().flatten().flatten()) {
        if entry.file_name().to_string_lossy().ends_with(".dist-info") {
            let target = fs::read_link(entry.path().join("METADATA")).ok();
            if let Some(id) = target.as_deref().and_then(|t| t.strip_prefix(archives).ok()).and_then(|r| r.iter().next()) {
                found.insert(id.to_owned());
            }
        } else if !venv && depth > 0 && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            symlinked(&entry.path(), archives, depth - 1, found);
        }
    }
}

/// uv's pointers under `dir` to an unpacked wheel in `archive-v0`, by the archive's name: a
/// symlink, or where uv makes none (Windows) a file whose whole text is `archive-v0/<id>`. Each
/// has sidecars beside it (`<pointer>.rev`, `.http`, `.msgpack`).
fn uv_pointers(dir: &Path, found: &mut HashMap<std::ffi::OsString, Vec<PathBuf>>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            uv_pointers(&entry.path(), found);
            continue;
        }
        let target = if kind.is_symlink() {
            fs::read_link(entry.path()).ok()
        } else {
            entry
                .metadata()
                .is_ok_and(|m| m.len() < 256)
                .then(|| fs::read_to_string(entry.path()).ok())
                .flatten()
                .map(|text| PathBuf::from(text.trim()))
        };
        let archive = target
            .as_deref()
            .filter(|t| t.parent().and_then(Path::file_name) == Some("archive-v0".as_ref()));
        if let Some(name) = archive.and_then(Path::file_name) {
            found.entry(name.to_owned()).or_default().push(entry.path());
        }
    }
}

/// A uv pointer and its sidecars, before the entry it names: never a pointer to nothing.
fn remove_pointer(pointer: &Path) {
    let (Some(dir), Some(name)) = (pointer.parent(), pointer.file_name()) else {
        return;
    };
    let sidecar = format!("{}.", name.to_string_lossy());
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&sidecar) {
            remove(&entry.path());
        }
    }
    remove(pointer);
}

/// The newest ctime under `path` (a link or unlink changes it), or None while a file under it
/// has another link, or cannot be read.
fn unlinked_since(path: &Path) -> Option<SystemTime> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let changed = durable::changed(path).ok()?;
    if metadata.is_dir() {
        let children = fs::read_dir(path).ok()?.flatten();
        children
            .map(|entry| unlinked_since(&entry.path()))
            .try_fold(changed, |newest, child| Some(newest.max(child?)))
    } else if metadata.is_file() && durable::links(path).ok()? > 1 {
        None
    } else {
        Some(changed)
    }
}

/// TTL always; on a low disk, least recently used first until it is not (`Kind`'s order).
pub fn sweep(caches: &Caches) -> io::Result<Swept> {
    // One at a time: the reclaim thread's and a store admit's (`Service::reclaim_for`).
    static SWEEPING: Mutex<()> = Mutex::new(());
    let _one = SWEEPING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut swept = Swept::default();
    let engine = caches.engine;
    let root = &engine.root;
    let filesystem = FileId::at(root)?.dev;
    let open = open_files();
    // Spools of settled or unknown runs are leftovers of a crash, never needed again.
    for entry in fs::read_dir(root.join("staging"))?.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        let settled = match engine.get(&id) {
            Ok(record) => record.state.terminal(),
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        };
        let bytes = releasable(&entry.path(), filesystem, &open).unwrap_or(0);
        if settled && remove(&entry.path()) {
            swept.add(Kind::Staging, 1, bytes);
        }
    }
    // Garbage first: uv entries no environment links.
    let uv = |swept: &mut Swept| caches.uv_cache.map_or(Ok(()), |cache| uv_pass(cache, caches.environments, caches.disk, swept));
    uv(&mut swept)?;
    let mut entries = vec![];
    let mut add = |kind, path: PathBuf, used, guard| entries.push(Entry { kind, path, used, hold: None, guard });
    // Result copies the client collected; uncollected ones are durable outputs.
    for entry in fs::read_dir(root.join("results"))?.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if engine.get(&id).is_ok_and(|record| record.collected) {
            add(Kind::Result, entry.path(), modified(&entry.path()), None);
        }
    }
    // Logs no unfinished run names (machine logs, rotated copies): diagnostics.
    if let Ok(logs) = fs::read_dir(root.join("logs")) {
        for entry in logs.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.split('.').next().unwrap_or_default();
            let settled = match engine.get(id) {
                Ok(record) => record.state.terminal(),
                Err(error) => error.kind() == io::ErrorKind::NotFound,
            };
            if settled {
                add(Kind::Log, entry.path(), modified(&entry.path()), None);
            }
        }
    }
    if let Some(memo) = caches.memo {
        let (count, bytes) = memo.sweep(); // its own TTL and cap
        swept.add(Kind::Memo, count, bytes);
        for home in memo.homes() {
            let used = last_use(&home);
            add(Kind::Memo, home, used, None);
        }
    }
    // Garbage once idle, each sweep: a generation whose build failed or was cut short (no
    // record, its build lock free), an editable environment no generation names, a Runtime pair
    // neither selected nor kept to roll back to.
    let mut garbage = |kind, path: &Path, used| {
        let bytes = releasable(path, filesystem, &open).unwrap_or(0);
        if unused_for(used, IDLE) && remove(path) {
            swept.add(kind, 1, bytes);
        }
    };
    let generations = caches.catalog.root();
    let engine_dir = generations.parent().unwrap_or(generations);
    let mut named = HashSet::new();
    // Generations unbound by any installation, unfinished run or configured package: a
    // resolve touches `.hold`, and a holder's shared lock on it refuses collection.
    for entry in fs::read_dir(generations)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        named.extend(fs::read_link(entry.path().join("env")).ok().and_then(|env| durable::canonical(&env).ok()));
        if name.starts_with(".collect-") {
            remove(&entry.path()); // an interrupted collection
        } else if name.len() == 32
            && name.bytes().all(|b| b.is_ascii_hexdigit())
            && !caches.bound.contains(&name)
        {
            if entry.path().join("generation.json").is_file() {
                let used = modified(&entry.path().join(".hold"));
                add(Kind::Generation, entry.path(), used, None);
                continue;
            }
            let lock = File::open(generations.join(".locks").join(format!("{name}.lock")));
            let _building = match lock {
                Ok(lock) if lock.try_lock().is_err() => continue,
                lock => lock,
            };
            garbage(Kind::Generation, &entry.path(), modified(&entry.path()));
        }
    }
    let homes = fs::read_dir(engine_dir.join("editable")).into_iter().flatten().flatten();
    let homes = homes.flat_map(|actor| fs::read_dir(actor.path()).into_iter().flatten().flatten());
    for env in homes.flat_map(|home| fs::read_dir(home.path()).into_iter().flatten().flatten()) {
        let unnamed = durable::canonical(&env.path()).is_ok_and(|path| !named.contains(&path));
        if env.file_name().to_string_lossy().starts_with("env-") && unnamed {
            garbage(Kind::Editable, &env.path(), modified(&env.path()));
            remove(&env.path().with_extension("wheels"));
        }
    }
    let kept = crate::machine::update::kept_pairs(engine_dir);
    for pair in fs::read_dir(engine_dir.join("sdk")).into_iter().flatten().flatten() {
        let name = pair.file_name();
        if name.to_string_lossy().starts_with("candidate-") && !kept.iter().any(|k| k.file_name() == Some(&name)) {
            // A pair being staged is written to throughout.
            garbage(Kind::Pair, &pair.path(), last_use(&pair.path()));
        }
    }
    // An update's staged and downloaded files: a dev build sent again, a release fetched again.
    for dir in ["staged", "downloads"] {
        for entry in fs::read_dir(engine_dir.join("update").join(dir)).into_iter().flatten().flatten() {
            let used = last_use(&entry.path());
            add(Kind::Update, entry.path(), used, None);
        }
    }
    // No lock is held across the sweep: each kernel entry takes its users' lock around its own
    // removal, and one in use stays for the next sweep.
    for (path, guard) in caches.kernels.map(kernel_entries).unwrap_or_default() {
        let used = last_use(&path);
        add(Kind::Kernel, path, used, Some(guard));
    }
    let discard = |entry: &mut Entry, swept: &mut Swept| -> io::Result<()> {
        let bytes = releasable(&entry.path, filesystem, &open).unwrap_or(0);
        let label = (entry.kind == Kind::Generation).then(|| label(&entry.path));
        if entry.remove()? {
            swept.add(entry.kind, 1, bytes);
            swept.environments.extend(label);
        }
        Ok(())
    };
    let mut kept = vec![];
    for mut entry in entries {
        match unused_for(entry.used, TTL) {
            true => discard(&mut entry, &mut swept)?,
            false => kept.push(entry),
        }
    }
    if (caches.disk)()?.short().is_some() {
        // Droppable now, each kind least recently used first; a generation once idle and unheld.
        let mut plan = vec![];
        for mut entry in kept {
            if entry.kind == Kind::Update && !unused_for(entry.used, IDLE) {
                continue;
            }
            if entry.kind == Kind::Generation {
                let hold = File::open(entry.path.join(".hold")).ok().filter(|hold| hold.try_lock().is_ok());
                if !unused_for(entry.used, IDLE) || hold.is_none() {
                    continue;
                }
                entry.hold = hold;
            }
            if let Some(bytes) = releasable(&entry.path, filesystem, &open).filter(|b| *b > 0) {
                plan.push((entry, bytes));
            }
        }
        plan.sort_by_key(|(entry, _)| (entry.kind, entry.used));
        let (kernels, before): (Vec<_>, Vec<_>) = plan.into_iter().partition(|(entry, _)| entry.kind == Kind::Kernel);
        for (mut entry, _) in before {
            if (caches.disk)()?.short().is_none() {
                break;
            }
            discard(&mut entry, &mut swept)?;
        }
        // Kernels go only if they alone cover what is still missing.
        let cover: u64 = kernels.iter().map(|(_, bytes)| bytes).sum();
        for (mut entry, _) in kernels {
            match (caches.disk)()?.short() {
                Some(short) if cover >= short => discard(&mut entry, &mut swept)?,
                _ => break,
            }
        }
    }
    // The uv entries only the environments just dropped linked.
    if !swept.environments.is_empty() {
        uv(&mut swept)?;
    }
    Ok(swept)
}

/// A generation as people name it: its record's package and version, else its identity.
fn label(generation: &Path) -> String {
    let record: Option<serde_json::Value> = fs::read(generation.join("generation.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let name = generation.file_name().unwrap_or_default().to_string_lossy();
    match record.as_ref().and_then(|r| Some((r["package"].as_str()?, r["version"].as_str()?))) {
        Some((package, version)) => format!("{package} {version}"),
        None => name.into_owned(),
    }
}

/// What fills the machine's disk, by kind, as the last sweep measured it (`measure_usage`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub models: u64,
    pub environments: u64,
    pub runtime: u64,
}

static USAGE: Mutex<Option<Usage>> = Mutex::new(None);

/// The last measure of what fills the disk; none before the first sweep.
pub fn usage() -> Option<Usage> {
    *USAGE.lock().unwrap()
}

/// Measures what fills the disk: the TensorFS store at `store`; the package environments under
/// `engine` (generations, editable ones: their own files, `own`) and the entries this machine
/// added to `uv_cache` that no environment hard-links; the rest of `engine` (the Runtime pair,
/// kernels, the toolchain, Pythons) but its runs' files (other).
pub fn measure_usage(store: &Path, engine: &Path, uv_cache: Option<&Path>) {
    let generations = engine.join("generations");
    let editable = engine.join("editable");
    let skip: Vec<PathBuf> = [generations.clone(), editable.clone(), engine.join("execution")].into_iter().chain(uv_cache.map(Path::to_path_buf)).collect();
    let named = |e: &fs::DirEntry| !e.file_name().to_string_lossy().starts_with('.');
    let mut environments: u64 = fs::read_dir(&generations).into_iter().flatten().flatten().filter(named).map(|g| own(&g.path())).sum();
    let homes = fs::read_dir(&editable).into_iter().flatten().flatten().flat_map(|a| fs::read_dir(a.path()).into_iter().flatten().flatten());
    let envs = homes.flat_map(|home| fs::read_dir(home.path()).into_iter().flatten().flatten()).filter(|e| e.file_name().to_string_lossy().starts_with("env-"));
    environments += envs.map(|env| own(&env.path())).sum::<u64>();
    let added = uv_cache.into_iter().flat_map(|cache| fs::read_dir(cache.join("archive-v0")).into_iter().flatten().flatten());
    environments += added.filter(|a| unlinked_since(&a.path()).is_some()).map(|a| du(&a.path(), &[])).sum::<u64>();
    let usage = Usage { models: du(store, &[]), environments, runtime: du(engine, &skip) };
    *USAGE.lock().unwrap() = Some(usage);
}

/// The bytes only environment `dir` holds (`releasable`: neither its links into a uv cache nor
/// what it shares), measured once it settled (its bytecode compiled, `IDLE` after its last
/// change) and kept beside it in `.bytes`.
fn own(dir: &Path) -> u64 {
    let kept = dir.join(".bytes");
    if let Some(bytes) = fs::read_to_string(&kept).ok().and_then(|text| text.trim().parse().ok()) {
        return bytes;
    }
    let bytes = FileId::at(dir).ok().and_then(|id| releasable(dir, id.dev, &HashSet::new())).unwrap_or(0);
    if unused_for(modified(dir), IDLE) {
        let _ = fs::write(&kept, bytes.to_string());
    }
    bytes
}

/// Bytes allocated under `path` but `skip`, never through a link.
fn du(path: &Path, skip: &[PathBuf]) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() || skip.iter().any(|s| s == path) {
        return 0;
    }
    let children = || fs::read_dir(path).into_iter().flatten().flatten();
    durable::allocated(&metadata) + if metadata.is_dir() { children().map(|e| du(&e.path(), skip)).sum() } else { 0 }
}

/// A refusal's account of the disk under `path`: what fills it and the `--disk-gb` that would
/// also hold `missing` bytes more.
pub fn advice(path: &Path, missing: u64) -> String {
    let Ok(disk) = measure(path) else {
        return String::new();
    };
    let held = usage().map_or(String::new(), |u| {
        format!("models {}, package environments {}, Runtime {}; ", size(u.models), size(u.environments), size(u.runtime))
    });
    let gb = (disk.capacity.saturating_sub(disk.available) + missing + disk.reserve()).div_ceil(10_000_000_000) * 10;
    format!(" (the disk holds {held}{} of {} free; --disk-gb {gb} would hold this call)", size(disk.available), size(disk.capacity))
}
