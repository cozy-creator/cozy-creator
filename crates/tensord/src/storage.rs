//! The machine's one TensorFS store, opened with TensorFS's own cache policy: it evicts on its
//! own and never takes an open checkpoint's bytes. A model TensorD keeps, it holds open.
//!
//! - A checkpoint's id is its cozytensors header's object (`sha256:<hex>`).
//! - Local models are `local/<name>` repos, never evicted: one commit, its newest.
//! - A Hub model this machine pulled is a cached ref naming its commit (`held_models::Pulled`).
//! - An adapter view (`adapter::compose`) lives only in this process's memory ([`keep_view`]):
//!   its id resolves here like a stored header's, and its executor is sent its bytes at Load.
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write as _},
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub use tensorfs::store::Store;
use tensorfs::format::{ObjectRef, Repo, Tree};

/// Opens (or creates) the machine store at `root`; one in an older format is refused.
pub fn open(root: &Path) -> io::Result<Arc<Store>> {
    Store::open(root).map(Arc::new).map_err(io_error)
}

/// The machine's claim on its store: `owner.lock` at the store's real path, held exclusively
/// (without waiting) for as long as the returned file lives; the OS releases it when the machine
/// exits. A second machine on the same store, by any path, is refused. Readers (`tfs`, the
/// library) never take it.
pub fn claim(root: &Path) -> io::Result<std::fs::File> {
    let real = std::fs::canonicalize(root)?;
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(real.join(OWNER_LOCK))?;
    match platform::lock::try_claim(&file)? {
        true => Ok(file),
        false => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("STORE_BUSY: this TensorFS store already has an owner ({})", real.display()),
        )),
    }
}

/// The machine's claim on its store (`claim`).
const OWNER_LOCK: &str = "owner.lock";

/// Past this the transport log rotates to `transport.log.1`, so it holds at most twice this.
const TRANSPORT_ROTATE: u64 = 4 << 20;

/// One line of the store's transport log (`cozy machine logs <m> tensorfs`): what a download or
/// import did, or why it failed: `<unix seconds> <kind> <subject> <detail>`, one per completed
/// fetch, never per object. Never fatal.
pub fn transport(store: &Store, kind: &str, subject: &str, detail: &str) {
    let dir = store.root().join("logs");
    let path = dir.join("transport.log");
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > TRANSPORT_ROTATE) {
        let _ = std::fs::rename(&path, dir.join("transport.log.1"));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH);
    let at = now.map_or(0.0, |since| since.as_secs_f64());
    let line = format!("{at:.3} {kind} {subject} {detail}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn io_error(error: tensorfs::store::Error) -> io::Error {
    let kind = match &error {
        tensorfs::store::Error::Full { .. } => io::ErrorKind::StorageFull,
        tensorfs::store::Error::Absent(_) => io::ErrorKind::NotFound,
        tensorfs::store::Error::Io { source, .. } => source.kind(),
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, error.to_string())
}

/// A local model: its commit (tree), that commit's lane and header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub tree: ObjectRef,
    pub lane: String,
    pub header: ObjectRef,
}

/// The local model `repo` (`local/<name>`): its newest commit; None when the name holds nothing.
pub fn local(store: &Store, repo: &str) -> io::Result<Option<Local>> {
    let Some(tree) = store.get_ref(repo).map_err(io_error)?.and_then(|named| named.commits.last().cloned()) else {
        return Ok(None);
    };
    let parsed = store.tree(&tree).map_err(io_error)?;
    let header = parsed.header().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?.1.clone();
    Ok(Some(Local { tree, lane: parsed.lane, header }))
}

/// Names `header` (with its source's repo `files`, in lane `lane`) the local model
/// `local/<name>`: a new commit, replacing what the name held.
pub fn set_local(store: &Store, name: &str, header: &ObjectRef, files: &BTreeMap<String, ObjectRef>, lane: &str) -> io::Result<ObjectRef> {
    let tree = Tree::of(lane, header.clone(), files).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let tree = store.put_tree(&tree).map_err(io_error)?;
    let repo = format!("local/{name}");
    store
        .update_ref(&repo, |_| Some(Repo { commits: vec![tree.clone()], ..Repo::new(&repo) }))
        .map_err(io_error)?;
    Ok(tree)
}

/// An unlabelled commit of `header` alone.
pub fn tree_of(store: &Store, header: &ObjectRef) -> io::Result<ObjectRef> {
    let tree = Tree::of("", header.clone(), &BTreeMap::new()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    store.put_tree(&tree).map_err(io_error)
}

/// The held object `object` read by its format: a tree (a commit), else a cozytensors header
/// (`None`); refused when it is neither.
pub fn tree_or_header(store: &Store, object: &ObjectRef) -> io::Result<Option<Tree>> {
    let bytes = store.read(object).map_err(io_error)?;
    if let Ok(tree) = Tree::from_json(&bytes) {
        return Ok(Some(tree));
    }
    tensorfs::format::Header::from_cbor(&bytes).map(|_| None).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{} is neither a tree nor a cozytensors header: {e}", object.id()))
    })
}

/// A header composed in memory, the objects it cites that no store holds (its graph), and what
/// composes it again.
#[derive(Debug)]
pub struct View {
    pub header: Vec<u8>,
    pub own: Vec<Vec<u8>>,
    pub recipe: crate::adapter::Recipe,
}

/// The views' bytes this process keeps; past them the oldest goes, and a run that names it again
/// composes it again (an H3 view is about 1 MB).
const VIEW_BYTES: usize = 256 << 20;

fn views() -> &'static Mutex<VecDeque<(String, Arc<View>)>> {
    static VIEWS: Mutex<VecDeque<(String, Arc<View>)>> = Mutex::new(VecDeque::new());
    &VIEWS
}

/// Keeps `header` (and `own`) as a view in memory, open over `store`; nothing is written.
pub fn keep_view(store: &Store, header: Vec<u8>, own: Vec<Vec<u8>>, recipe: crate::adapter::Recipe) -> io::Result<tensorfs::store::Checkpoint> {
    let checkpoint = store.view(&header, &own).map_err(io_error)?;
    let sha256 = checkpoint.id().sha256.clone();
    let mut views = views().lock().unwrap();
    views.retain(|(id, _)| *id != sha256);
    views.push_back((sha256, Arc::new(View { header, own, recipe })));
    let size = |views: &VecDeque<(String, Arc<View>)>| views.iter().map(|(_, v)| v.header.len() + v.own.iter().map(Vec::len).sum::<usize>()).sum::<usize>();
    while views.len() > 1 && size(&views) > VIEW_BYTES {
        views.pop_front();
    }
    Ok(checkpoint)
}

/// The view `id` (`sha256:<hex>` or bare hex) this process keeps.
pub fn view(id: &str) -> Option<Arc<View>> {
    let sha256 = id.trim_start_matches("sha256:");
    views().lock().unwrap().iter().find(|(id, _)| id == sha256).map(|(_, v)| v.clone())
}

/// The object `id` (`sha256:<hex>` or bare hex) as the store holds it, its length read off disk;
/// a view by its header's length.
pub fn held(store: &Store, id: &str) -> io::Result<ObjectRef> {
    let sha256 = id.trim_start_matches("sha256:").to_string();
    if let Some(view) = view(&sha256) {
        return Ok(ObjectRef { sha256, length: view.header.len() as u64 });
    }
    let path = store.path(&ObjectRef { sha256: sha256.clone(), length: 0 }).map_err(io_error)?;
    let length = std::fs::metadata(path)?.len();
    Ok(ObjectRef { sha256, length })
}

/// The checkpoint `id` names, held here (its structure; chunks may be absent): a header, a
/// commit's (a run's checkpoint output names its commit), or a view this process keeps.
pub fn checkpoint(store: &Store, id: &str) -> io::Result<tensorfs::store::Checkpoint> {
    if let Some(view) = view(id) {
        return store.view(&view.header, &view.own).map_err(io_error);
    }
    let object = held(store, id)?;
    match store.tree(&object) {
        Ok(_) => store.at_commit(&object),
        Err(_) => store.checkpoint(&object),
    }
    .map_err(io_error)
}

/// The chunks and assets `components` read (all components when empty), each once; none when
/// it names a component the checkpoint lacks.
pub fn chunks(checkpoint: &tensorfs::store::Checkpoint, components: &[String]) -> Vec<ObjectRef> {
    tensorfs::fetch::wanted(checkpoint.header(), (!components.is_empty()).then_some(components)).unwrap_or_default()
}

/// `text` is 64 lowercase hex digits (a bare sha256), or `sha256:` and them when `prefixed`.
pub fn check_digest(what: &str, text: &str, prefixed: bool) -> io::Result<()> {
    let hex = match prefixed {
        true => text.strip_prefix("sha256:"),
        false => Some(text),
    };
    match hex.is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))) {
        true => Ok(()),
        false => Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{what}: {text:?} is not a sha256 digest"))),
    }
}
