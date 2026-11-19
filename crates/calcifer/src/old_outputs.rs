//! The old landing folder of runs outside a project, `~/.cozy/outputs` (cl-1036; Paul 11-16: "Delete both"); nothing since 0.26
//! writes there. At each start while it is there, before anything is served: what each registered project shows of it is held in
//! this computer's TensorFS store, beside what the legacy cut holds, and copied into the project at its next open
//! (`project::keep_left_on_local`). A project that can't be read then (locked, or its lock unreadable) is written down as waiting,
//! and the folder is set aside; it goes once no project waits, each read at its next open. A registration whose folder is gone
//! (deleted, or moved to a place not known) shows nothing and waits for nothing. A stop anywhere leaves
//! the folder where the next start finds it again. One-time code: it goes in the release after.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Mutex,
};

use tensorfs::format::{Header, ObjectRef};

/// One reader and writer of what waits at a time: a start's set-aside, an open's read, the deletion.
static WAITING: Mutex<()> = Mutex::new(());

/// The folder, and where it waits once set aside.
pub(crate) fn dirs(calcifer: &Path) -> [PathBuf; 2] {
    let cozy = calcifer.parent().unwrap_or(calcifer);
    [cozy.join("outputs"), cozy.join("outputs.cut")]
}

/// The projects it waits for, beside the holds: deleting `kept/` lets it go too, as `cozy doctor` says. Each by its id and its folder:
/// a locked project's index id isn't the one its open finds inside it, and a moved one opens under its id elsewhere.
fn list(calcifer: &Path) -> PathBuf {
    calcifer.join("kept").join("outputs.waiting")
}

type Waiting = BTreeSet<(String, String)>;

/// Where a project's files left on `local` are held in this computer's TensorFS store until its next open copies them in
/// (`project::keep_left_on_local`): a header of just those objects, beside the store in `kept/`, which TensorFS roots while the
/// file is there, whatever its uses' ages. Named by the project's folder, so nothing else records which project it is.
pub(crate) fn kept_path(calcifer: &Path, root: &Path) -> PathBuf {
    let folder = root.components().collect::<PathBuf>().to_string_lossy().into_owned();
    calcifer.join("kept").join(format!("{}.cozytensors", crate::store::key_digest(&format!("cozy: files a locked project left on local\n{folder}"))))
}

/// `objects` held by TensorFS under `at` (`kept_path`) until that file is removed. Err: why it couldn't be, in a person's words.
fn hold(store: &tensorfs::store::Store, at: &Path, objects: Vec<ObjectRef>) -> Result<(), &'static str> {
    let header = Header::new(objects, None, vec![], Default::default()).map_err(|_| "its record couldn't be made")?;
    let bytes = header.to_cbor().map_err(|_| "its record couldn't be made")?;
    let dir = at.parent().expect("kept/");
    platform::durable::private_dir(dir).map_err(|error| crate::project::write_cause(&error))?;
    platform::durable::replace(at, &bytes, 0o600).map_err(|error| crate::project::write_cause(&error))?;
    store.root_file(at, &ObjectRef::of(&bytes)).map_err(|error| crate::project::write_cause(&error))
}

/// A folder as one key, however its path was written (`lab` and `lab/`).
fn key(root: &Path) -> String {
    crate::project::vault::logical(root).components().collect::<PathBuf>().to_string_lossy().into_owned()
}

fn read(calcifer: &Path) -> Waiting {
    let text = std::fs::read_to_string(list(calcifer)).unwrap_or_default();
    text.lines().filter_map(|line| line.split_once('\t')).map(|(id, folder)| (id.to_owned(), folder.to_owned())).collect()
}

fn write(calcifer: &Path, waiting: &Waiting) -> Result<(), &'static str> {
    let path = list(calcifer);
    platform::durable::private_dir(path.parent().expect("kept/")).map_err(|error| crate::project::write_cause(&error))?;
    let text: String = waiting.iter().map(|(id, folder)| format!("{id}\t{folder}\n")).collect();
    platform::durable::replace(&path, text.as_bytes(), 0o600).map_err(|error| crate::project::write_cause(&error))
}

/// How many projects the set-aside folder waits for (`cozy doctor`), with it; none once it is gone.
pub(crate) fn waiting(calcifer: &Path) -> (u64, PathBuf) {
    let [_, aside] = dirs(calcifer);
    let _waiting = WAITING.lock().expect("old outputs lock");
    (if aside.exists() { read(calcifer).len() as u64 } else { 0 }, aside)
}

/// At a start, with the registered projects (id, folder): the folder held and set aside while it is there, the projects no longer
/// registered waiting for nothing, and the folder deleted off the start's path once none waits.
pub(crate) fn start(calcifer: &Path, registered: &[(String, PathBuf)]) {
    let _waiting = WAITING.lock().expect("old outputs lock");
    let [outputs, aside] = dirs(calcifer);
    if outputs.is_dir() {
        if aside.exists() {
            tracing::warn!(target: "cozy.service_health", code = "old_outputs_kept", "the old landing folder ~/.cozy/outputs stays until the one set aside before it is gone");
        } else if let Err(cause) = set_aside(calcifer, registered, &outputs, &aside) {
            tracing::warn!(target: "cozy.service_health", code = "old_outputs_kept", cause, "the old landing folder ~/.cozy/outputs stays until the next start");
        }
    }
    if !aside.exists() { return; }
    let ids: BTreeSet<&str> = registered.iter().map(|(id, _)| id.as_str()).collect();
    let folders: BTreeSet<String> = registered.iter().map(|(_, root)| key(root)).collect();
    let mut waiting = read(calcifer);
    let before = waiting.len();
    waiting.retain(|(id, folder)| ids.contains(id.as_str()) || folders.contains(folder));
    if waiting.len() != before && write(calcifer, &waiting).is_err() { return; }
    if waiting.is_empty() { drop_aside(aside); }
}

/// What each project shows of the folder held, those that can't be read written down, then the folder moved aside: a stop before the
/// move leaves it to the next start, which holds again (the same bytes are one object).
fn set_aside(calcifer: &Path, registered: &[(String, PathBuf)], outputs: &Path, aside: &Path) -> Result<(), &'static str> {
    let dirs = [outputs.to_owned(), aside.to_owned()];
    let mut waiting = BTreeSet::new();
    for (id, root) in registered {
        if !hold_shown(calcifer, root, &dirs)? { waiting.insert((id.clone(), key(root))); }
    }
    write(calcifer, &waiting)?;
    std::fs::rename(outputs, aside).map_err(|error| crate::project::write_cause(&error))?;
    if let Some(cozy) = outputs.parent() { let _ = platform::durable::sync_dir(cozy); }
    tracing::info!(target: "cozy.service_health", waiting = waiting.len(), "the old landing folder is set aside, what projects show of it held; it goes once no project waits to be read");
    Ok(())
}

/// At a project's open, before its holds are copied in: one the set-aside folder waits for is read now, what it shows held, and the
/// folder deleted once none waits.
pub(crate) fn opened(calcifer: &Path, id: &str, root: &Path) {
    let _waiting = WAITING.lock().expect("old outputs lock");
    let [outputs, aside] = dirs(calcifer);
    let mut waiting = read(calcifer);
    let folder = key(root);
    let this = |(waits, at): &(String, String)| waits == id || *at == folder;
    if !aside.exists() || !waiting.iter().any(this) { return; }
    match hold_shown(calcifer, root, &[outputs, aside.clone()]) {
        Ok(true) => {}
        Ok(false) => return,
        Err(cause) => return tracing::warn!(target: "cozy.service_health", code = "old_outputs_kept", cause, "the old landing folder stays until this project's next open reads it"),
    }
    waiting.retain(|entry| !this(entry));
    if write(calcifer, &waiting).is_ok() && waiting.is_empty() { drop_aside(aside); }
}

/// What the project at `root` shows of the folder (`project::kept_in_outputs`), put in this computer's TensorFS store and held for it
/// with what the legacy cut holds. False when it can't be read now. Put through an origin's handle: a cache's admission runs the
/// store's upkeep, which would let aged objects go that a hold not yet written keeps.
fn hold_shown(calcifer: &Path, root: &Path, dirs: &[PathBuf]) -> Result<bool, &'static str> {
    let Some(shown) = crate::project::kept_in_outputs(root, dirs) else { return Ok(false) };
    if shown.is_empty() { return Ok(true); }
    let cache = crate::runs::files::store().map_err(|_| "this computer's file store can't be opened")?;
    let origin = tensorfs::store::Store::open(cache.root()).map_err(|error| crate::project::write_cause(&error))?.origin();
    let held = kept_path(calcifer, root);
    let mut objects: Vec<ObjectRef> = match std::fs::read(&held) {
        Ok(bytes) => Header::from_cbor(&bytes).map_err(|_| "its hold couldn't be read")?.objects().cloned().collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(crate::project::write_cause(&error)),
    };
    for source in shown {
        let file = std::fs::File::open(&source).map_err(|error| crate::project::write_cause(&error))?;
        let object = origin.put(file, None).map_err(|error| crate::project::write_cause(&error))?;
        if !objects.contains(&object) { objects.push(object); }
    }
    hold(cache, &held, objects)?;
    Ok(true)
}

/// The set-aside folder deleted on a thread of its own (Paul's laptop's was 62 GB in 427,674 files); what is left goes at the next
/// start.
fn drop_aside(aside: PathBuf) {
    std::thread::spawn(move || {
        match std::fs::remove_dir_all(&aside) {
            Ok(()) => tracing::info!(target: "cozy.service_health", "the old landing folder is gone"),
            Err(_) => tracing::warn!(target: "cozy.service_health", code = "old_outputs_left", "what is left of the old landing folder goes at the next start"),
        }
    });
}
