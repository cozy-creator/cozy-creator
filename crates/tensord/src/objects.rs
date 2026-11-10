//! A machine's run objects (inputs, outputs, trees, products, local package sources) live in its
//! TensorFS store, with its models. The store's own retention keeps them:
//!
//! - an object a signer wrote, or a run was handed, is a use under its own digest (`storage::keep`):
//!   kept for the store's TTL after its last use;
//! - a run's output is a use under its owner's name (`trees::put`), kept the same way;
//! - while the machine runs it holds what the journal needs now and every output (`root`), so the
//!   store's eviction on a low disk never takes them. An output goes last: only once the disk is
//!   still low after the store evicted what it could, and nothing used it for `IDLE` (`make_room`).
//!
//! An upload lands byte ranges in any order, several side by side, and resumes from the ranges
//! held. Bytes stage per signer and enter the store only once every byte is held and the digest
//! verifies. A run names only objects its own signer wrote, so another signer's object is neither
//! readable nor reported as held.
use crate::{
    execution::Engine,
    journal::{Artifact, Execution, HeldMemo, ResultRecord},
    storage,
};
use platform::durable::FileAt;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, Weak,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use crate::reclaim::Disk;
use tensorfs::format::ObjectRef;
use tensorfs::sha256;

/// The largest memoized result record this machine holds for reuse.
const MEMO_RESULT_BYTES: usize = 48 << 10;

/// Byte ranges `[start, end)`, sorted and apart.
pub(crate) type Ranges = Vec<(u64, u64)>;

pub(crate) struct Objects {
    root: PathBuf,
    engine: Arc<Engine>,
    /// Uploads under way, by part file.
    uploads: Mutex<HashMap<PathBuf, Weak<Upload>>>,
}

/// Why a write was refused, as the API's typed reason.
#[derive(Debug)]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
}
fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
    }
}
impl From<io::Error> for Refused {
    fn from(error: io::Error) -> Self {
        refused("object_storage_failed", error.to_string())
    }
}

/// One object's upload, shared by the writes that carry its ranges.
pub(crate) struct Upload {
    actor: String,
    object: ObjectRef,
    part: PathBuf,
    file: File,
    held: Mutex<Ranges>,
    /// The bytes held, for those following the upload as it lands.
    landed: tokio::sync::watch::Sender<u64>,
    /// One settle at a time records what landed.
    settling: Mutex<()>,
    /// Stored or dropped: later writes to it land nowhere, and a new upload starts afresh.
    ended: AtomicBool,
}

impl Upload {
    /// Lands `data` at `offset`.
    pub(crate) fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), Refused> {
        let end = offset + data.len() as u64;
        if end > self.object.length {
            return Err(refused(
                "object_length_exceeded",
                "more bytes than the object's declared length",
            ));
        }
        if self.ended.load(Ordering::Acquire) {
            return Ok(());
        }
        self.file.write_all_at(data, offset)?;
        let mut held = self.held.lock().unwrap();
        insert(&mut held, (offset, end));
        self.landed.send_replace(held.iter().map(|(s, e)| e - s).sum());
        Ok(())
    }

    /// The bytes held, now and as more land.
    pub(crate) fn follow(&self) -> tokio::sync::watch::Receiver<u64> {
        self.landed.subscribe()
    }

    /// What has landed, synced or not.
    pub(crate) fn held(&self) -> Ranges {
        self.held.lock().unwrap().clone()
    }
}

/// Adds `[start, end)` to `ranges`, merging what it touches.
fn insert(ranges: &mut Ranges, (mut start, mut end): (u64, u64)) {
    if start >= end {
        return;
    }
    ranges.retain(|&(s, e)| {
        let apart = e < start || s > end;
        if !apart {
            (start, end) = (start.min(s), end.max(e));
        }
        apart
    });
    let at = ranges.partition_point(|&(s, _)| s < start);
    ranges.insert(at, (start, end));
}

fn whole(length: u64) -> Ranges {
    match length {
        0 => vec![],
        length => vec![(0, length)],
    }
}

/// The record of what a part holds: its object's length and the ranges synced.
fn held_path(part: &Path) -> PathBuf {
    part.with_extension("held")
}
fn read_held(part: &Path) -> io::Result<Option<(u64, Ranges)>> {
    match fs::read(held_path(part)) {
        Ok(raw) => Ok(serde_json::from_slice(&raw).ok()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn write_held(part: &Path, length: u64, held: &Ranges) -> io::Result<()> {
    let path = held_path(part);
    let next = path.with_extension("held.next");
    fs::write(&next, serde_json::to_vec(&(length, held))?)?;
    fs::rename(next, path)
}

/// The journal's time `age` ago.
fn since(age: Duration) -> i64 {
    let before = SystemTime::now().checked_sub(age).unwrap_or(UNIX_EPOCH);
    before.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Holds what the journal needs now (`Journal::needed_objects`) and every run's held output,
/// and nothing else. An output whose document and files all went unused for `unused`, and
/// that no run needs, is let go, least recently used first, until those let go cover `room`
/// bytes: the store then keeps it as cache. Called with the object custody held.
fn root(engine: &Engine, unused: Duration, room: u64) -> io::Result<()> {
    let store = &engine.store;
    let mut keep = engine.with_journal(|j| j.needed_objects(since(crate::reclaim::IDLE)))?;
    let mut spare = vec![];
    for record in engine.with_journal(|j| j.native_outputs())? {
        let request = crate::stored::decode::<crate::stored::NativeByteRetentionRequest>(&record).ok();
        let Some(manifest) = request.and_then(|r| r.source?.manifest) else {
            continue;
        };
        let manifest = ObjectRef { sha256: sha256::hex(&manifest.digest), length: manifest.length };
        let Some(roots) = crate::trees::roots(store, &manifest) else {
            continue;
        };
        let used = roots.iter().filter_map(|root| fs::metadata(store.path(root).ok()?).ok()?.modified().ok()).max();
        let mut free = used.is_none_or(|at| at.elapsed().is_ok_and(|idle| idle >= unused));
        for root in &roots {
            free = free && engine.with_journal(|j| j.object_releasable(&root.sha256, since(unused)))?;
        }
        match free {
            true => spare.push((used, roots)),
            false => keep.extend(roots),
        }
    }
    spare.sort_by_key(|(used, _)| *used);
    let mut freed = 0u64;
    for (_, roots) in spare {
        match freed >= room {
            true => keep.extend(roots),
            false => freed = freed.saturating_add(roots.iter().map(|root| root.length).sum()),
        }
    }
    engine.reroot_objects(&keep)
}

/// The run objects' own store a machine before 0.23 kept (`objects/` beside the journal): what
/// a hold kept (a run's output) and what the journal still needs move into the TensorFS store,
/// by rename, each under its use before it moves. The rest goes: unheld objects, and uploads
/// under way (`writes/`), which their senders still have. A move that fails leaves the folder
/// as it is for the next start.
fn upgrade(engine: &Engine, writes: &Path) {
    #[derive(serde::Deserialize)]
    struct Hold {
        owner: String,
        roots: Vec<ObjectRef>,
    }
    let old = engine.root.join("objects");
    if !old.is_dir() {
        return;
    }
    let store = &engine.store;
    let moved = (|| -> io::Result<usize> {
        let mut objects = engine.with_journal(|j| j.needed_objects(since(crate::reclaim::IDLE)))?;
        for object in &objects {
            storage::keep(store, object)?;
        }
        for entry in fs::read_dir(old.join("holds")).into_iter().flatten() {
            let hold: Hold = serde_json::from_slice(&fs::read(entry?.path())?).map_err(io::Error::other)?;
            store.used(&crate::trees::name(&hold.owner), &hold.roots).map_err(storage::io_error)?;
            objects.extend(hold.roots);
        }
        engine.root_objects(&objects)?;
        let mut moved = 0;
        for object in &objects {
            let from = old.join(&object.sha256[..2]).join(&object.sha256);
            if !fs::metadata(&from).is_ok_and(|meta| meta.len() == object.length) {
                continue; // moved by an earlier start, or gone before the update
            }
            let to = store.path(object).map_err(storage::io_error)?;
            fs::create_dir_all(to.parent().expect("a blob has a shard"))?;
            // Another filesystem: copied in through the store's own door, which checks it.
            if store.holds(object) || fs::rename(&from, &to).is_err() {
                storage::put(store, File::open(&from)?, Some(object))?;
            }
            moved += 1;
        }
        Ok(moved)
    })();
    match moved {
        Ok(moved) => {
            tracing::info!("moved {moved} run objects into the TensorFS store at {}; the old run object folder is removed", store.root().display());
            let removed = fs::remove_dir_all(&old).and_then(|()| fs::remove_dir_all(writes));
            if let Err(error) = removed {
                tracing::warn!("removing {}: {error}", old.display());
            }
        }
        Err(error) => tracing::warn!("run objects stay in {} until the next start: moving them into the TensorFS store failed: {error}", old.display()),
    }
}

/// At the machine's start, before anything is admitted: a machine updated from before 0.23 moves
/// its run objects into the store (`upgrade`), and what must not be evicted is held (`root`).
pub(crate) fn restore(engine: &Engine, writes: &Path) -> io::Result<()> {
    upgrade(engine, writes);
    let _custody = engine.object_custody.lock().unwrap();
    root(engine, crate::reclaim::TTL, u64::MAX)
}

impl Objects {
    pub(crate) fn new(root: &Path, engine: Arc<Engine>) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        Ok(Self { root: root.to_path_buf(), engine, uploads: Mutex::default() })
    }

    /// The store the objects are held in.
    pub(crate) fn store(&self) -> &storage::Store {
        &self.engine.store
    }

    /// Excludes root release while a Write or an acceptance takes references.
    pub(crate) fn guard(&self) -> MutexGuard<'_, ()> {
        self.engine.object_custody.lock().unwrap()
    }
    /// Renews `objects`, each here, and holds them, so nothing evicts them before their run is
    /// journaled; called with `guard` held.
    pub(crate) fn retain(&self, objects: &[ObjectRef]) -> io::Result<()> {
        for object in objects {
            if !self.store().reuse(object).map_err(storage::io_error)? {
                return Err(io::Error::new(io::ErrorKind::NotFound, format!("object {} is absent", object.id())));
            }
            storage::keep(self.store(), object)?;
        }
        self.engine.root_objects(objects)
    }

    /// Holds what is needed now, and lets go of outputs nothing used for the TTL: the store
    /// expires those with the rest of its cache.
    pub(crate) fn sweep(&self) -> io::Result<()> {
        let _custody = self.guard();
        root(&self.engine, crate::reclaim::TTL, u64::MAX)
    }

    /// A disk still low after every cache: the store evicts what it may (idle models, objects no
    /// run needs), and only then held outputs nothing used for `IDLE`, least recently used first,
    /// until it isn't. How many objects the outputs let go freed, and their bytes.
    pub(crate) fn make_room(&self) -> io::Result<(usize, u64)> {
        // What the caches just freed shows a moment later.
        std::thread::sleep(Duration::from_secs(1));
        let Some(short) = Disk::measure(self.store().root())?.short() else {
            return Ok((0, 0));
        };
        let freed = self.store().make_room(0).map_err(storage::io_error)?.bytes;
        if freed >= short {
            return Ok((0, 0));
        }
        {
            let _custody = self.guard();
            root(&self.engine, crate::reclaim::IDLE, short - freed)?;
        }
        let freed = self.store().make_room(0).map_err(storage::io_error)?;
        Ok((freed.objects as usize, freed.bytes))
    }

    /// The stored object this signer wrote, or None.
    pub(crate) fn path(&self, actor: &str, digest: &str) -> io::Result<Option<(PathBuf, u64)>> {
        let Some(hex) = hex(digest) else {
            return Ok(None);
        };
        let Some(length) = self.engine.with_journal(|j| j.object(actor, &hex))? else {
            return Ok(None);
        };
        let path = self.store().path(&ObjectRef { sha256: hex, length }).map_err(storage::io_error)?;
        Ok(path.is_file().then_some((path, length)))
    }

    /// The success a memoized computation left for this signer, its files linked into `run`'s
    /// custody as that run's own result; None when there is none or a file is gone (the memo
    /// goes with it).
    pub(crate) fn recall(&self, actor: &str, computation: &str, run: &str) -> io::Result<Option<ResultRecord>> {
        let _custody = self.guard();
        let Some(raw) = self.engine.with_journal(|j| j.held_memo(actor, computation))? else {
            return Ok(None);
        };
        let Ok(mut record) = serde_json::from_slice::<ResultRecord>(&raw) else {
            return Ok(None);
        };
        let custody = self.engine.root.join("results").join(run);
        for (index, artifact) in record.artifacts.iter_mut().enumerate() {
            let Some((held, _)) = self
                .path(actor, &format!("sha256:{}", artifact.sha256))?
                .filter(|(_, length)| *length == artifact.length)
            else {
                self.engine.with_journal(|j| j.drop_held_memos(&artifact.sha256))?;
                return Ok(None);
            };
            fs::create_dir_all(&custody)?;
            let name = format!("{index}-{}", artifact.sha256);
            if let Err(error) = fs::hard_link(&held, custody.join(&name)) {
                if error.kind() != io::ErrorKind::AlreadyExists {
                    fs::copy(&held, custody.join(&name))?;
                }
            }
            artifact.path = format!("results/{run}/{name}");
        }
        Ok(Some(record))
    }

    /// A memoizable run's success `result`, its files kept for its signer, to hold under its
    /// computation while this machine holds every file it names. A tree's members are not held
    /// as one result, nor is a record over `MEMO_RESULT_BYTES`.
    pub(crate) fn keep(&self, record: &Execution, result: &ResultRecord) -> io::Result<Option<HeldMemo>> {
        let Some(submission) = &record.submission else {
            return Ok(None);
        };
        if result.asset_bindings.iter().any(|b| b.media_type == api::tree::MEDIA) {
            return Ok(None);
        }
        let bound: Vec<_> = result
            .artifacts
            .iter()
            .filter(|a| result.asset_bindings.iter().any(|b| b.relative_path == a.name))
            .collect();
        let held = ResultRecord {
            value: result.value.clone(),
            artifacts: bound.iter().map(|a| Artifact { path: String::new(), ..(*a).clone() }).collect(),
            asset_bindings: result.asset_bindings.clone(),
            adjustments: result.adjustments.clone(),
        };
        let raw = serde_json::to_vec(&held)?;
        if raw.len() > MEMO_RESULT_BYTES {
            return Ok(None);
        }
        for artifact in &bound {
            let object = ObjectRef { sha256: artifact.sha256.clone(), length: artifact.length };
            self.adopt(&submission.actor, &record.id, &self.engine.root.join(&artifact.path), &object)?;
        }
        Ok(Some(HeldMemo {
            actor: submission.actor.clone(),
            computation: record.memo.computation.clone(),
            record: raw,
            objects: bound.iter().map(|a| a.sha256.clone()).collect(),
        }))
    }

    /// A file this machine produced for the signer (a child run's result) as its object, so a
    /// later run of that signer may be handed it.
    pub(crate) fn adopt(&self, actor: &str, parent: &str, file: &Path, object: &ObjectRef) -> io::Result<()> {
        let _custody = self.guard();
        if !self.store().reuse(object).map_err(storage::io_error)? {
            storage::put(self.store(), File::open(file)?, Some(object))?;
        }
        storage::keep(self.store(), object)?;
        self.engine
            .with_journal(|j| j.adopt_object(actor, parent, object))?;
        self.engine.root_objects(std::slice::from_ref(object))
    }

    fn part(&self, actor: &str, hex: &str) -> PathBuf {
        let dir = self.root.join(&sha256::hex_digest(actor.as_bytes())[..32]);
        dir.join(format!("{hex}.part"))
    }

    /// The stored object this signer wrote, its roots renewed so a run naming it soon finds it;
    /// called with `guard` held.
    fn renew(&self, actor: &str, digest: &str) -> io::Result<Option<u64>> {
        let Some((_, length)) = self.path(actor, digest)? else {
            return Ok(None);
        };
        let object = ObjectRef {
            sha256: digest["sha256:".len()..].to_string(),
            length,
        };
        self.retain(std::slice::from_ref(&object))?;
        self.engine.with_journal(|j| j.bind_object(actor, &object))?;
        Ok(Some(length))
    }

    /// The ranges of `digest` (`sha256:<hex>`) this signer holds: all of it once stored, else
    /// what its upload has landed. An upload whose ranges cover it is settled first, so whole
    /// always means stored; bytes that differ from the digest are dropped, and held no more.
    pub(crate) fn held(&self, actor: &str, digest: &str) -> Result<Ranges, Refused> {
        let hex = valid(digest)?;
        let part = self.part(actor, &hex);
        // Under custody a settle of this part has not begun or has stored the object (`upload`).
        let upload = {
            let _custody = self.guard();
            if let Some(length) = self.renew(actor, digest)? {
                return Ok(whole(length));
            }
            self.uploads.lock().unwrap().get(&part).and_then(Weak::upgrade)
        };
        if let Some(upload) = upload {
            return match upload.held() {
                held if held != whole(upload.object.length) => Ok(held),
                _ => match self.settle(&upload) {
                    Err(refusal) if refusal.code == "object_digest_mismatch" => Ok(vec![]),
                    settled => settled,
                },
            };
        }
        Ok(read_held(&part)?.map(|(_, held)| held).unwrap_or_default())
    }

    /// The upload of `digest`, `length` bytes, for this signer: joined while under way, else
    /// resumed from the ranges an earlier one synced. None once the object is stored.
    pub(crate) fn upload(&self, actor: &str, digest: &str, length: u64) -> Result<Option<Arc<Upload>>, Refused> {
        match self.open_upload(actor, digest, length, false) {
            // Short: what the machine can free goes first (no lock held: a sweep takes custody),
            // and then it may use the reserve, since this is what the caller asked to put here.
            Err(refusal) if refusal.code == "machine_disk_full" => {
                if let Err(error) = self.store().maintain(length) {
                    tracing::warn!("making room for an upload: {error}");
                }
                self.open_upload(actor, digest, length, true)
            }
            opened => opened,
        }
    }

    fn open_upload(&self, actor: &str, digest: &str, length: u64, reserve: bool) -> Result<Option<Arc<Upload>>, Refused> {
        let hex = valid(digest)?;
        // Custody from the stored check through the part's open: a settle takes the part under
        // custody and keeps it until the object is stored and bound, so the part is never
        // reopened (and truncated) while a settle reads it.
        let _custody = self.guard();
        if self.renew(actor, digest)? == Some(length) {
            return Ok(None);
        }
        let part = self.part(actor, &hex);
        let mut uploads = self.uploads.lock().unwrap();
        uploads.retain(|_, upload| upload.strong_count() > 0);
        if let Some(upload) = uploads.get(&part).and_then(Weak::upgrade) {
            if upload.object.length != length {
                return Err(refused(
                    "invalid_request",
                    format!("{digest} is being written as {} bytes", upload.object.length),
                ));
            }
            return Ok(Some(upload));
        }
        fs::create_dir_all(part.parent().expect("a signer's directory"))?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&part)?;
        let held = match read_held(&part)? {
            Some((was, held)) if was == length => held,
            _ => {
                file.set_len(0)?;
                vec![]
            }
        };
        file.set_len(length)?;
        let missing = length - held.iter().map(|(s, e)| e - s).sum::<u64>();
        let disk = Disk::measure(&self.engine.root)?;
        if missing > if reserve { disk.available } else { disk.free() } {
            return Err(refused("machine_disk_full", format!("no room for the object: {missing} bytes do not fit, {} are free{}", disk.available, crate::reclaim::advice(&self.engine.root, missing))));
        }
        if missing > disk.free() {
            tracing::warn!("an upload of {missing} bytes takes this machine's disk into its reserve: {} are free", disk.available);
        }
        let landed = tokio::sync::watch::Sender::new(length - missing);
        let upload = Arc::new(Upload {
            actor: actor.to_string(),
            object: ObjectRef { sha256: hex, length },
            part: part.clone(),
            file,
            held: Mutex::new(held),
            landed,
            settling: Mutex::default(),
            ended: AtomicBool::new(false),
        });
        uploads.insert(part, Arc::downgrade(&upload));
        Ok(Some(upload))
    }

    /// Records what `upload` landed, durably. Once it holds every byte, the object is verified
    /// against its digest and stored for its signer; bytes that differ are dropped. Answers the
    /// ranges held.
    pub(crate) fn settle(&self, upload: &Upload) -> Result<Ranges, Refused> {
        let digest = format!("sha256:{}", upload.object.sha256);
        let length = upload.object.length;
        let _settling = upload.settling.lock().unwrap();
        if upload.ended.load(Ordering::Acquire) {
            return self.held(&upload.actor, &digest);
        }
        let held = upload.held();
        upload.file.sync_data()?;
        if held != whole(length) {
            write_held(&upload.part, length, &held)?;
            return Ok(held);
        }
        let _custody = self.guard();
        upload.ended.store(true, Ordering::Release);
        self.uploads.lock().unwrap().remove(&upload.part);
        let stored = File::open(&upload.part).map_err(Refused::from).and_then(|part| {
            storage::put(self.store(), part, Some(&upload.object)).and_then(|_| storage::keep(self.store(), &upload.object)).map_err(|e| {
                let code = match e.kind() {
                    io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => "machine_disk_full",
                    io::ErrorKind::InvalidData => "object_digest_mismatch",
                    _ => "object_storage_failed",
                };
                refused(code, e.to_string())
            })
        });
        let _ = fs::remove_file(&upload.part);
        let _ = fs::remove_file(held_path(&upload.part));
        stored?;
        self.engine
            .with_journal(|j| j.bind_object(&upload.actor, &upload.object))?;
        self.engine.root_objects(std::slice::from_ref(&upload.object))?;
        Ok(held)
    }
}

fn valid(digest: &str) -> Result<String, Refused> {
    hex(digest).ok_or_else(|| {
        refused(
            "invalid_request",
            "an object is named by sha256:<64 lowercase hex>",
        )
    })
}

fn hex(digest: &str) -> Option<String> {
    digest
        .strip_prefix("sha256:")
        .filter(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .map(String::from)
}

/// A typed refusal of the run layer (`runs`, `objects`) as the error its code names.
pub(crate) fn refusal(code: &str, message: impl Into<String>) -> api::error::Error {
    match code {
        "run_id_conflict" => api::error::Error::conflict(code, message),
        "machine_disk_full" => api::error::Error::capacity(code, message),
        "object_storage_failed" | "machine_released" | "machine_updating" => {
            api::error::Error::unavailable(code, message)
        }
        c if c.starts_with("invalid") || c.starts_with("object_") => api::error::Error::validation(code, message),
        _ => api::error::Error::conflict(code, message),
    }
}

impl From<Refused> for api::error::Error {
    fn from(refused: Refused) -> Self {
        refusal(refused.code, refused.message)
    }
}
