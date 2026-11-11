//! The store (design §2): the local cache of cozytensors that any process reads and writes.
//!
//! `<root>/{format, blobs/<aa>/<bb>/<sha256>, refs/<org>/<name>.json, holds/, files/, uses/,
//! derived/<slot>, snapshots/<name>/, tmp/}`
//!
//! - Objects are trees (commits), cozytensors headers, chunks, assets and repo files. A ref
//!   records a model's releases and commits. A cozytensors is opened by its header
//!   ([`Store::checkpoint`]) or its commit ([`Store::at_checkpoint`]); a plain folder enters by
//!   [`Store::admit_folder`] and leaves by [`Store::snapshot`].
//! - One door admits objects ([`Store::admit`], [`Store::put`]): streamed into `tmp/`, hashed,
//!   checked, flushed per [`Durability`] and renamed into `blobs/`. An object is verified there
//!   once and trusted after; [`Store::verify`] rehashes on demand.
//! - One retention regime (`retention.rs`): holds, header files and `local/` refs root objects;
//!   all else is cache, kept for a TTL after its last use and evicted least recently used first,
//!   one checkpoint's lane whole, when the disk runs low. GC marks and sweeps (`gc.rs`) what is older than the grace, so
//!   whatever newly references an object makes it young first: `put` and `reuse` touch it.
//! - Processes share a store with no leases, markers or boot-time rehash: a torn write never has
//!   a name, and an open descriptor outlives an unlink.
//! - A fetch's bytes keep their place in `tmp/` when it is cut or killed ([`Store::partial`]): the
//!   next fetch of the object takes them up, and the object's digest judges them with the rest.

mod checkpoint;
mod derived;
pub use derived::Derived;
pub(crate) mod file;
mod folder;
pub use folder::{copy_checked, files as folder_files};
mod gc;
pub use gc::RemovedModel;
mod records;
pub mod retention;

pub use checkpoint::{wanted, Checkpoint};
pub(crate) use records::is_name;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use platform::durable::{self, DiskClass, FileId};

use crate::err::{Code, Refusal};
use crate::format::{self, is_digest, ObjectRef};
use crate::sha256::{hex, Sha256};

/// [`crate::STORE_FORMAT`] as the store's `format` record.
/// The store format this TensorFS reads and writes (its `format` record).
pub(crate) const FORMAT: &str = "tensorfs.store/3";
const LAYOUT: [&str; 6] = ["blobs", "refs", "holds", "files", "uses", "tmp"];

/// How long an unreferenced object outlives its last write or reuse.
pub(crate) const GRACE: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Durability {
    /// Each object and record is flushed before it is named (local machines).
    Durable,
    /// Nothing is flushed until one `syncfs`, which runs before a record names objects this
    /// handle admitted (pods; owner ruling 2026-09-27, tfs-122). Linux only, else `Durable`.
    Ephemeral,
}

impl Durability {
    /// What the filesystem under `path` calls for: ephemeral on tmpfs and overlay, else durable.
    pub(crate) fn of(path: &Path) -> Durability {
        match DiskClass::of(path) {
            DiskClass::Ephemeral => Durability::Ephemeral.supported(),
            _ => Durability::Durable,
        }
    }

    fn supported(self) -> Durability {
        if cfg!(target_os = "linux") {
            self
        } else {
            Durability::Durable
        }
    }
}

/// Why a store operation failed: I/O, a full disk, an absent or mismatched object, a bad document.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{op} {}: the disk is full", path.display())]
    Full { op: &'static str, path: PathBuf },
    #[error("object {0} is absent")]
    Absent(String),
    #[error("expected {} ({} bytes), got {got}", expected.sha256, expected.length)]
    Mismatch { expected: ObjectRef, got: String },
    #[error("{}: {why}", path.display())]
    Invalid { path: PathBuf, why: String },
    /// A document, or a tensor written or read, breaks cozytensors/1.
    #[error(transparent)]
    Format(#[from] format::Error),
}

impl Error {
}

impl Error {
    /// The refusal the fetch and import layers speak.
    pub fn refusal(self) -> Refusal {
        let code = match &self {
            Error::Io { .. } => Code::IO_FAILED,
            Error::Full { .. } => Code::CAPACITY_EXHAUSTED,
            Error::Absent(_) => Code::OBJECT_ABSENT,
            Error::Mismatch { .. } => Code::OBJECT_ID_MISMATCH,
            Error::Invalid { .. } => Code::UNKNOWN_FORMAT,
            Error::Format(format::Error::UnknownScheme(_)) => Code::UNKNOWN_ENCODING,
            Error::Format(format::Error::Decode(_) | format::Error::Encode(_)) => Code::ENCODING_MISMATCH,
            Error::Format(format::Error::Format { .. }) => {
                Code::UNKNOWN_FORMAT
            }
            Error::Format(format::Error::Invalid(_)) => Code::TENSOR_SCHEMA_MISMATCH,
        };
        Refusal {
            code,
            detail: self.to_string(),
        }
    }
}

/// A result that fails with a store `Error`.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn fail(op: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
    let path = path.into();
    move |source| match source.kind() {
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Error::Full { op, path },
        _ => Error::Io { op, path, source },
    }
}

fn platform(
    op: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(platform::Error) -> Error {
    let path = path.into();
    move |error| fail(op, path)(error.into())
}

pub(crate) fn invalid(path: impl Into<PathBuf>, why: impl ToString) -> Error {
    Error::Invalid {
        path: path.into(),
        why: why.to_string(),
    }
}

/// `family/major[.minor]`: the same family and major as `want`.
pub(crate) fn same_major(tag: &str, want: &str) -> bool {
    fn split(tag: &str) -> Option<(&str, &str)> {
        let (family, version) = tag.split_once('/')?;
        Some((family, version.split('.').next()?))
    }
    split(tag).is_some() && split(tag) == split(want)
}

/// Whether a store format's major is below this TensorFS's.
fn older(tag: &str) -> bool {
    let major = |tag: &str| tag.split_once('/')?.1.split('.').next()?.parse::<u32>().ok();
    major(tag).zip(major(FORMAT)).is_some_and(|(found, ours)| found < ours)
}

pub(crate) fn same_family(tag: &str, want: &str) -> bool {
    tag.split_once('/').map(|t| t.0) == want.split_once('/').map(|w| w.0)
}

/// A handle on the store at one root; clones share its sync, collection and reclaim state.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    durability: Durability,
    grace: Duration,
    /// This handle admitted objects that no `syncfs` covers yet (ephemeral only).
    unsynced: Arc<AtomicBool>,
    /// A cache: writes run its upkeep (expiry, eviction). An origin's (an endpoint's) never do.
    cache: bool,
    /// The collections this handle ran (`maintain`), counted under the lock that serializes them.
    collections: Arc<Mutex<u64>>,
    /// What frees the machine's garbage before this store evicts a model (`on_short`).
    reclaim: Arc<OnceLock<Reclaim>>,
}

/// What frees disk outside a store for bytes coming in, before it evicts a model.
struct Reclaim(Box<dyn Fn(u64) + Send + Sync>);

impl std::fmt::Debug for Reclaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reclaim")
    }
}

impl Store {
    /// Opens the store at `root`, creating it when `root` holds none.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(fail("create", &root))?;
        let root = platform::durable::canonical(&root).map_err(|e| fail("resolve", &root)(e.into()))?;
        let record = root.join("format");
        match fs::read(&record) {
            Ok(bytes) => {
                #[derive(serde::Deserialize)]
                struct Record {
                    format: String,
                }
                match serde_json::from_slice::<Record>(&bytes) {
                    Ok(found) if same_major(&found.format, FORMAT) => {}
                    // An older store this one cannot read: moved aside, never deleted (it may hold
                    // a model made here), and this store starts empty.
                    Ok(found) if same_family(&found.format, FORMAT) && older(&found.format) => {
                        let aside = root.with_file_name(format!(
                            "{}.{}",
                            root.file_name().unwrap_or_default().to_string_lossy(),
                            found.format.replace('/', "-")
                        ));
                        fs::rename(&root, &aside).map_err(fail("move aside", &root))?;
                        tracing::warn!("{} was a {} store from an older TensorFS, which this one cannot read: moved to {}, and models are pulled again", root.display(), found.format, aside.display());
                        return Store::open(root);
                    }
                    _ => {
                        let found = String::from_utf8_lossy(&bytes);
                        return Err(invalid(&record, format!("is {found:?}, not {FORMAT}")));
                    }
                }
            }
            // Other files beside it (a mounted disk's `lost+found`) are not the store's business.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let bytes = format!("{{\"format\":\"{FORMAT}\"}}");
                durable::replace(&record, bytes.as_bytes(), 0o644)
                    .map_err(platform("write", &record))?;
            }
            Err(error) => return Err(fail("read", &record)(error)),
        }
        for dir in LAYOUT {
            let path = root.join(dir);
            fs::create_dir_all(&path).map_err(fail("create", &path))?;
        }
        Ok(Store {
            durability: Durability::of(&root),
            grace: GRACE,
            root,
            unsynced: Arc::default(),
            cache: true,
            collections: Arc::default(),
            reclaim: Arc::default(),
        })
    }

    /// Frees the machine's own garbage before this handle (and its clones) evicts a model:
    /// `reclaim(incoming)` runs when an admit of `incoming` bytes finds the disk short. Set once.
    pub fn on_short(&self, reclaim: impl Fn(u64) + Send + Sync + 'static) {
        let _ = self.reclaim.set(Reclaim(Box::new(reclaim)));
    }

    /// This store as an origin, a directory endpoint's: nothing it holds is cache, so its writes
    /// never expire or evict anything (`maintain`).
    pub fn origin(mut self) -> Store {
        self.cache = false;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `object` lives. A malformed digest has no path.
    pub fn path(&self, object: &ObjectRef) -> Result<PathBuf> {
        self.blob(&object.sha256)
    }

    fn blob(&self, sha256: &str) -> Result<PathBuf> {
        if !is_digest(sha256) {
            return Err(invalid(
                &self.root,
                format!("{sha256:?} is not a sha256 digest"),
            ));
        }
        Ok(self.object_path(sha256))
    }

    /// Where the object `sha256`, a digest already checked, lives.
    pub(crate) fn object_path(&self, sha256: &str) -> PathBuf {
        (self.root.join("blobs").join(&sha256[..2]).join(&sha256[2..4])).join(sha256)
    }

    /// The one door. Bytes written to the [`Admission`] become an object at
    /// [`Admission::finish`]; with `expect`, any other bytes are refused.
    pub(crate) fn admit(&self, expect: Option<&ObjectRef>) -> Result<Admission> {
        if let Some(expect) = expect {
            self.path(expect)?;
        }
        if !self.maintain(expect.map_or(0, |object| object.length))? {
            return Err(Error::Full { op: "admit", path: self.root.clone() });
        }
        let (temp, file) = self.temp("put")?;
        platform::lock::share(&file).map_err(platform("lock", &temp))?;
        Ok(Admission {
            store: self.clone(),
            expect: expect.cloned(),
            temp,
            file: Some(file),
            hash: Sha256::new(),
            length: 0,
            over: false,
            paced: durable::Paced::default(),
        })
    }

    /// `object` for a fetch, which writes each range at its offset and can be cut at any moment, a kill
    /// included ([`Partial`]). One that another fetch holds is left to it, and this one keeps nothing: a
    /// cancelled fetch's commit holds it a moment after the next run claims the object (gate 41991226171).
    pub fn partial(&self, object: &ObjectRef) -> Result<Partial> {
        self.path(object)?;
        let temp = self.root.join("tmp").join(format!("partial-{}", object.sha256));
        let (temp, file, journal, keep) = match partial(&temp) {
            Some((file, _)) => {
                let journal = journal_path(&temp);
                let journal = OpenOptions::new().read(true).append(true).create(true).open(&journal).map_err(fail("open", &journal))?;
                let _ = platform::lock::try_claim(&journal);
                (temp, file, Some(journal), true)
            }
            None => {
                let (temp, file) = self.temp("put")?;
                (temp, file, None, false)
            }
        };
        let held = journal.as_ref().map(|journal| read_held(journal, object.length)).unwrap_or_default();
        if !self.maintain(object.length - held.iter().map(|(a, b)| b - a).sum::<u64>())? {
            return Err(Error::Full { op: "admit", path: self.root.clone() });
        }
        file.set_len(object.length).map_err(fail("size", &temp))?;
        Ok(Partial { store: self.clone(), object: object.clone(), temp, file: Some(file), journal: journal.map(|j| Arc::new(Mutex::new(j))), held: Arc::new(Mutex::new(held)), keep })
    }

    /// Admits everything `src` yields. With `expect`, an object already here is reused without
    /// reading `src`.
    pub fn put(&self, mut src: impl Read, expect: Option<&ObjectRef>) -> Result<ObjectRef> {
        if let Some(expect) = expect {
            if self.reuse(expect)? {
                return Ok(expect.clone());
            }
        }
        let mut admission = self.admit(expect)?;
        let mut buffer = vec![0; 1 << 20];
        loop {
            let n = match src.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(fail("read source for", &admission.temp)(error)),
            };
            if let Err(error) = admission.write_all(&buffer[..n]) {
                return Err(admission.refusal(error));
            }
        }
        admission.finish()
    }

    pub fn put_bytes(&self, bytes: &[u8]) -> Result<ObjectRef> {
        self.put(bytes, None)
    }

    /// Whether `object` is here whole, without touching it: a look (a listing), not a use.
    pub fn holds(&self, object: &ObjectRef) -> bool {
        self.path(object).is_ok_and(|path| fs::metadata(path).is_ok_and(|m| m.len() == object.length))
    }

    /// True when `object` is here whole, and then it is young for GC: call it before referencing
    /// an object this process did not just put. The one presence check (offline reads, the
    /// Fetcher, admission): a blob of another length is damaged, so it is removed and reads as
    /// absent, and the next admission of the object replaces it.
    pub fn reuse(&self, object: &ObjectRef) -> Result<bool> {
        let Some(file) = self.young(&object.sha256)? else {
            return Ok(false);
        };
        let path = self.path(object)?;
        let length = file.metadata().map_err(fail("stat", &path))?.len();
        if length == object.length {
            return Ok(true);
        }
        tracing::warn!("tensorfs: {} is {length} bytes, not {}: removed, to be replaced", path.display(), object.length);
        match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(fail("remove", &path)(error)),
            _ => self.stamp_eviction().map(|()| false),
        }
    }

    /// Opens `object` to read. Its bytes were verified when admitted.
    pub fn file(&self, object: &ObjectRef) -> Result<File> {
        let path = self.path(object)?;
        // One a sweep is moving right now still reads, from GC's trash.
        let opened = File::open(&path)
            .or_else(|_| File::open(self.trash(&object.sha256)))
            .or_else(|_| File::open(&path));
        let file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(Error::Absent(object.sha256.clone()))
            }
            Err(error) => return Err(fail("open", &path)(error)),
        };
        file.lock_shared().map_err(fail("lock", &path))?;
        let length = file.metadata().map_err(fail("stat", &path))?.len();
        if length != object.length {
            return Err(Error::Mismatch {
                expected: object.clone(),
                got: format!("{length} bytes on disk"),
            });
        }
        Ok(file)
    }

    pub fn read(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(object.length as usize);
        let path = self.path(object)?;
        self.file(object)?
            .read_to_end(&mut bytes)
            .map_err(fail("read", &path))?;
        Ok(bytes)
    }

    /// Makes every object this handle admitted durable. Ephemeral: one `syncfs`, which the
    /// next record commit also runs. Durable objects already are.
    pub(crate) fn sync(&self) -> Result<()> {
        if !self.unsynced.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let synced = syncfs(&self.root);
        if synced.is_err() {
            self.unsynced.store(true, Ordering::SeqCst);
        }
        synced.map_err(fail("syncfs", &self.root))
    }

    pub(crate) fn flushes(&self) -> bool {
        self.durability == Durability::Durable
    }

    fn temp(&self, kind: &str) -> Result<(PathBuf, File)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = self
                .root
                .join("tmp")
                .join(format!("{kind}-{}-{n}", std::process::id()));
            // Readable: a partial's commit hashes what it wrote from this file.
            match OpenOptions::new().read(true).write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(fail("create", &path)(error)),
            }
        }
    }

    /// Where a sweep moves an object before it decides (`gc.rs`).
    fn trash(&self, sha256: &str) -> PathBuf {
        self.root.join("tmp").join("gc").join(sha256)
    }

    /// Opens object `sha256` and makes it young; `None` when it is gone. One a sweep is moving
    /// right now is taken back from GC's trash. It is held only once, after the touch, its name
    /// names it again: a sweep that moves it from then on finds it young and puts it back.
    fn young(&self, sha256: &str) -> Result<Option<File>> {
        let blob = self.blob(sha256)?;
        let trash = self.trash(sha256);
        loop {
            let file = match touch(&blob)? {
                Some(file) => file,
                None => match touch(&trash)? {
                    Some(file) => match fs::rename(&trash, &blob) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => {
                            return Err(fail("restore", &blob)(error))
                        }
                        _ => file,
                    },
                    None => match touch(&blob)? {
                        Some(file) => file,
                        None => return Ok(None),
                    },
                },
            };
            let named = match FileId::at(&blob).map_err(io::Error::from) {
                Ok(id) => FileId::of(&file).map_err(platform("stat", &blob))? == id,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(fail("stat", &blob)(error)),
            };
            if named {
                return Ok(Some(file));
            }
        }
    }

    /// Names a finished temp `blob` of `object`, unless a whole copy is there already, which is
    /// kept young; a damaged one is replaced.
    fn install(&self, temp: &Path, blob: &Path, object: &ObjectRef) -> Result<()> {
        if self.reuse(object)? {
            let _ = fs::remove_file(temp);
            return Ok(());
        }
        let shard = blob.parent().expect("a blob has a shard");
        let created = !shard.is_dir();
        if created {
            fs::create_dir_all(shard).map_err(fail("create", shard))?;
        }
        fs::rename(temp, blob).map_err(fail("rename into", blob))?;
        if !self.flushes() {
            self.unsynced.store(true, Ordering::SeqCst);
            return Ok(());
        }
        if created {
            let parent = shard.parent().expect("a shard has a parent");
            sync_dir(parent.parent().expect("blobs/<aa> has a parent"))?;
            sync_dir(parent)?;
        }
        sync_dir(shard)
    }

    /// Replaces the record at `path` by a temp and a rename; `flush` makes it durable first.
    fn commit(&self, path: &Path, bytes: &[u8], flush: bool) -> Result<()> {
        let flush = flush && self.flushes();
        let (temp, mut file) = self.temp("record")?;
        platform::lock::share(&file).map_err(platform("lock", &temp))?;
        let written = file
            .write_all(bytes)
            .and_then(|()| if flush { file.sync_data() } else { Ok(()) });
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(fail("write", &temp)(error));
        }
        let dir = path.parent().expect("a record has a directory");
        let created = !dir.is_dir();
        if created {
            fs::create_dir_all(dir).map_err(fail("create", dir))?;
        }
        if let Err(error) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);
            return Err(fail("rename into", path)(error));
        }
        if flush {
            if created {
                sync_dir(dir.parent().expect("a record directory has a parent"))?;
            }
            sync_dir(dir)?;
        }
        Ok(())
    }

    fn remove_record(&self, path: &Path) -> Result<bool> {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(fail("remove", path)(error)),
        }
        if self.flushes() {
            sync_dir(path.parent().expect("a record has a directory"))?;
        }
        Ok(true)
    }
}

/// `tmp/partial-<sha256>` for one fetch to write, locked until it closes, and whether it was there
/// already. None: another fetch has it, or it can't be had. It may have been finished or swept
/// between the open and the lock, so what is locked must still be what the name names.
fn partial(path: &Path) -> Option<(File, bool)> {
    loop {
        let open = |new| OpenOptions::new().read(true).write(true).create_new(new).open(path);
        let (file, found) = match open(true) {
            Ok(file) => (file, false),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => match open(false) {
                Ok(file) => (file, true),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return None,
            },
            Err(_) => return None,
        };
        if !matches!(platform::lock::try_claim(&file), Ok(true)) {
            return None;
        }
        match (FileId::of(&file), FileId::at(path)) {
            (Ok(held), Ok(named)) if held == named => return Some((file, found)),
            (Ok(_), Ok(_)) | (_, Err(_)) => continue,
            (Err(_), _) => return None,
        }
    }
}

/// Bytes on their way into the store. Dropped unfinished, it leaves nothing behind.
pub(crate) struct Admission {
    store: Store,
    expect: Option<ObjectRef>,
    temp: PathBuf,
    file: Option<File>,
    hash: Sha256,
    length: u64,
    over: bool,
    paced: durable::Paced,
}

impl Admission {
    /// Hashes, checks, flushes and names the bytes written. Whatever the end, they stay nowhere else.
    pub(crate) fn finish(mut self) -> Result<ObjectRef> {
        let got = ObjectRef {
            length: self.length,
            sha256: hex(&std::mem::take(&mut self.hash).finish()),
        };
        if let Some(expect) = &self.expect {
            if *expect != got {
                return Err(Error::Mismatch {
                    expected: expect.clone(),
                    got: format!("{} ({} bytes)", got.sha256, got.length),
                });
            }
        }
        let file = self
            .file
            .take()
            .expect("an admission has its file until it finishes");
        if self.store.flushes() {
            file.sync_data().map_err(fail("sync", &self.temp))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o444))
                .map_err(fail("protect", &self.temp))?;
        }
        drop(file);
        let blob = self.store.blob(&got.sha256)?;
        self.store.install(&self.temp, &blob, &got)?;
        Ok(got)
    }

    fn refusal(&self, error: io::Error) -> Error {
        match (&self.expect, self.over) {
            (Some(expect), true) => Error::Mismatch {
                expected: expect.clone(),
                got: "more bytes".into(),
            },
            _ => fail("write", &self.temp)(error),
        }
    }
}

impl Write for Admission {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(expect) = &self.expect {
            if self.length + bytes.len() as u64 > expect.length {
                self.over = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "longer than the expected object",
                ));
            }
        }
        let file = self
            .file
            .as_mut()
            .expect("an admission has its file until it finishes");
        let n = file.write(bytes)?;
        self.paced.wrote(file, self.length, n as u64);
        self.hash.update(&bytes[..n]);
        self.length += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.temp);
    }
}

/// An object a fetch or an upload writes at its offsets (`tmp/partial-<sha256>`, as long as the object),
/// kept across a cut or a kill: `.held` beside it records each stretch once written, so the next asks
/// only for the rest. Nothing is hashed until commit, which reads the whole file back: only the object's digest
/// lets it in, and wrong bytes leave nothing kept.
pub struct Partial {
    store: Store,
    object: ObjectRef,
    temp: PathBuf,
    file: Option<File>,
    journal: Option<Arc<Mutex<File>>>,
    held: Arc<Mutex<Vec<(u64, u64)>>>,
    /// Whether what it holds outlives it.
    keep: bool,
}

/// Bytes written at their offsets, each stretch recorded once written.
pub struct Stretch {
    file: File,
    journal: Option<Arc<Mutex<File>>>,
    held: Arc<Mutex<Vec<(u64, u64)>>>,
    path: PathBuf,
    paced: durable::Paced,
}

impl Stretch {
    /// `bytes` at `at`, the stretch recorded once written.
    pub fn write_at(&mut self, at: u64, bytes: &[u8]) -> crate::err::Result<()> {
        write_all_at(&self.file, bytes, at).map_err(|error| fail("write", &self.path)(error).refusal())?;
        self.paced.wrote(&self.file, at, bytes.len() as u64);
        if let Some(journal) = &self.journal {
            let mut journal = journal.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            writeln!(journal, "{at} {}", at + bytes.len() as u64).map_err(|error| fail("write", journal_path(&self.path))(error).refusal())?;
        }
        let mut held = self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        held.push((at, at + bytes.len() as u64));
        *held = merged(std::mem::take(&mut held));
        Ok(())
    }
}

impl Partial {
    /// What it holds now, an earlier one's bytes and this one's writes: sorted, disjoint `[start, end)`
    /// stretches. `commit` judges them all.
    pub fn held(&self) -> Vec<(u64, u64)> {
        self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// A writer of bytes at their offsets, one per request.
    pub fn writer(&self) -> crate::err::Result<Stretch> {
        let file = self.file.as_ref().expect("a partial has its file until it commits").try_clone().map_err(|error| fail("open", &self.temp)(error).refusal())?;
        Ok(Stretch { file, journal: self.journal.clone(), held: self.held.clone(), path: self.temp.clone(), paced: durable::Paced::default() })
    }

    /// Hashes what was written and installs it: only the object's digest lets it in.
    pub fn commit(mut self) -> crate::err::Result<()> {
        let mut file = self.file.take().expect("a partial has its file until it commits");
        let got = (|| -> io::Result<ObjectRef> {
            file.seek(SeekFrom::Start(0))?;
            let (mut hash, mut length, mut buffer) = (Sha256::new(), 0u64, vec![0; 1 << 20]);
            loop {
                let n = match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                };
                hash.update(&buffer[..n]);
                length += n as u64;
            }
            Ok(ObjectRef { length, sha256: hex(&hash.finish()) })
        })()
        .map_err(|error| fail("read", &self.temp)(error).refusal())?;
        if got != self.object {
            // Wrong bytes are as suspect as what an earlier fetch kept: nothing stays.
            self.keep = false;
            let expected = self.object.clone();
            return Err(Error::Mismatch { expected, got: format!("{} ({} bytes)", got.sha256, got.length) }.refusal());
        }
        let installed = (|| -> Result<()> {
            if self.store.flushes() {
                file.sync_data().map_err(fail("sync", &self.temp))?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o444)).map_err(fail("protect", &self.temp))?;
            }
            drop(file);
            let blob = self.store.blob(&got.sha256)?;
            self.store.install(&self.temp, &blob, &got)
        })();
        self.keep = false;
        installed.map_err(Error::refusal)
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        self.file.take();
        self.journal.take();
        if !self.keep {
            let _ = fs::remove_file(&self.temp);
            let _ = fs::remove_file(journal_path(&self.temp));
        }
    }
}

/// Where a partial records the stretches it holds.
fn journal_path(temp: &Path) -> PathBuf {
    let mut name = temp.as_os_str().to_owned();
    name.push(".held");
    name.into()
}

/// The stretches a journal records, merged, within `length`. A torn last line is ignored.
fn read_held(mut journal: &File, length: u64) -> Vec<(u64, u64)> {
    let mut text = String::new();
    let _ = journal.read_to_string(&mut text);
    let stretches = text
        .lines()
        .filter_map(|line| {
            let (a, b) = line.split_once(' ')?;
            Some((a.parse().ok()?, b.parse::<u64>().ok()?.min(length)))
        })
        .collect();
    merged(stretches)
}

/// `stretches` sorted, those that touch as one.
fn merged(mut stretches: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    stretches.retain(|(a, b)| a < b);
    stretches.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (a, b) in stretches {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

#[cfg(unix)]
fn write_all_at(file: &File, bytes: &[u8], at: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, bytes, at)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut bytes: &[u8], mut at: u64) -> io::Result<()> {
    while !bytes.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_write(file, bytes, at)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        (bytes, at) = (&bytes[n..], at + n as u64);
    }
    Ok(())
}

pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    durable::sync_dir(dir).map_err(platform("sync directory", dir))
}

impl Store {
    /// Claims `object`'s download among everything sharing this store, until the claim drops. None:
    /// another holds it now. A lock on `tmp/fetching-<sha256>`, which an exit lets go. A claim that can't be made is no
    /// reason to wait: the worst of one is a second download.
    pub fn claim(&self, object: &ObjectRef) -> Option<Box<dyn Send + Sync>> {
        struct Claim(#[allow(dead_code)] File, PathBuf);
        impl Drop for Claim {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.1);
            }
        }
        let path = self.root.join("tmp").join(format!("fetching-{}", object.sha256));
        let Ok(file) = OpenOptions::new().write(true).create(true).truncate(false).open(&path) else {
            return Some(Box::new(()));
        };
        match platform::lock::try_claim(&file) {
            Ok(false) => None,
            _ => Some(Box::new(Claim(file, path))),
        }
    }
}

/// Makes an existing file young for GC; `None` when it is absent.
fn touch(path: &Path) -> Result<Option<File>> {
    #[cfg(windows)]
    let opened = {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES | 0x80000000)
            .open(path)
    };
    #[cfg(not(windows))]
    let opened = File::open(path);
    let file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail("open", path)(error)),
    };
    file.set_modified(SystemTime::now())
        .map_err(fail("touch", path))?;
    Ok(Some(file))
}

#[cfg(target_os = "linux")]
fn syncfs(root: &Path) -> io::Result<()> {
    rustix::fs::syncfs(File::open(root)?).map_err(io::Error::from)
}

#[cfg(not(target_os = "linux"))]
fn syncfs(_: &Path) -> io::Result<()> {
    Ok(())
}
