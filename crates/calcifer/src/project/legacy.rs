//! A project as files, from before `project.db` (cl-803): `project.yaml`, `project.lock`, `project.canvas.yaml`, `.measured.json`,
//! `.history.json`, `.keys.json` and what its walk owed in `.runs/`. Read once, on its first open, into `project.db`, then deleted. A
//! format 2 or 3 file is upgraded to 4 first (`upgrade::on_touch`), as before.
use std::{collections::BTreeMap, path::Path};

use protocol::error::{Error, Kind};
use tracing::{info, warn};

use super::{
    canvas, db,
    folder::{Folder, LOCK, Lock, Measured, PROJECT, WalkFile, parse_lock},
    graph::name_key,
    history,
    written::Written,
};

pub(super) const CANVAS: &str = "project.canvas.yaml";
pub(super) const MEASURED: &str = ".measured.json";
const WALK: &str = ".runs/walk.json";
const ASKED: &str = ".runs/asked";
/// What an import writes, whole, before it's `project.db`.
const NEW: &str = "project.db.new";
/// What an import deletes once it holds them.
const IMPORTED: [&str; 7] = [PROJECT, LOCK, CANVAS, MEASURED, ".history.json", ".keys.json", WALK];

fn io(at: &Path, e: impl std::fmt::Display) -> Error {
    Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", at.display()))
}

/// Whether `dir` holds a whole import that a kill stopped before it was named `project.db`.
pub(super) fn pending(dir: &Path) -> bool {
    let at = dir.join(NEW);
    at.is_file() && rusqlite::Connection::open(&at).ok().and_then(|conn| conn.query_row("SELECT value FROM meta WHERE key = 'seq'", [], |r| r.get::<_, String>(0)).ok()).is_some_and(|seq| seq != "0")
}

/// The folder's files read into `project.db`, then deleted. Killed before the end, its files stand (or a whole import does, which this
/// finishes), and the next open does it again.
pub(super) fn import(folder: &Folder) -> Result<(), Error> {
    let dir = &folder.dir;
    let (new, db) = (dir.join(NEW), dir.join(db::DB));
    if !pending(dir) {
        let stored = read(folder)?;
        let ops = history::imported(dir);
        db::create(&new, &stored.0, &ops, &stored.1)?;
        info!("{}: imported {} boxes, {} rows and {} history entries into {}", dir.display(), stored.0.written.boxes.len(), stored.0.canvas.len(), ops.len(), db::DB);
    }
    for file in IMPORTED {
        match std::fs::remove_file(dir.join(file)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&dir.join(file), e)),
            _ => {}
        }
    }
    let _ = std::fs::remove_dir_all(dir.join(ASKED));
    std::fs::rename(&new, &db).map_err(|e| io(&db, e))
}

/// What a folder from after its import may still hold of one: an import that was never named.
pub(super) fn leftovers(dir: &Path) {
    if dir.join(db::DB).is_file() {
        let _ = std::fs::remove_file(dir.join(NEW));
    }
}

/// Everything the files hold, as `project.db` will, and the texts its renders were asked with.
fn read(folder: &Folder) -> Result<(db::Stored, Vec<(String, String)>), Error> {
    let dir = &folder.dir;
    let text = std::fs::read_to_string(dir.join(PROJECT)).map_err(|e| io(&dir.join(PROJECT), e))?;
    let written = Written::from_yaml(&text)?;
    // As the reader would read it: a file it refuses isn't imported, and stays as it is.
    let lock = lock_file(dir)?;
    super::format::Doc::of(folder, &written, &lock)?;
    let walk: WalkFile = match std::fs::read_to_string(dir.join(WALK)) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| Error::new(Kind::Validation, "walk_unreadable", format!("{WALK}: {e}")))?,
        Err(_) => WalkFile::default(),
    };
    let asked = std::fs::read_dir(dir.join(ASKED)).into_iter().flatten().flatten().filter_map(|entry| Some((entry.file_name().to_str()?.to_owned(), std::fs::read_to_string(entry.path()).ok()?))).collect();
    let canvas = match std::fs::read_to_string(dir.join(CANVAS)) {
        Ok(text) => canvas::legacy_rows(&text).into_iter().map(|(name, card)| (name_key(&name), db::CardRow { name, card, seq: 0, by: String::new() })).collect(),
        Err(_) => BTreeMap::new(),
    };
    let stored = db::Stored { seq: 0, written: written.into(), canvas: canvas.into(), lock: lock.into(), walk: walk.into(), measured: measured(dir).into() };
    Ok((stored, asked))
}

/// `project.lock` as it was written, none if it's gone. One that can't be read is kept beside as `project.lock.broken`, and what's
/// approved stands.
fn lock_file(dir: &Path) -> Result<Lock, Error> {
    let at = dir.join(LOCK);
    let Ok(text) = std::fs::read_to_string(&at) else { return Ok(Lock::default()) };
    match parse_lock(&text) {
        Err(error) if error.code == "lock_unreadable" => {
            let aside = (1..).map(|n| dir.join(if n == 1 { format!("{LOCK}.broken") } else { format!("{LOCK}.broken-{n}") })).find(|path| !path.exists()).expect("a free name");
            std::fs::rename(&at, &aside).map_err(|e| io(&aside, e))?;
            warn!("{}: {}: moved to {}", dir.display(), error.message, aside.display());
            Ok(Lock::default())
        }
        read => read,
    }
}

fn measured(dir: &Path) -> BTreeMap<String, Measured> {
    std::fs::read(dir.join(MEASURED)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// project.yaml and project.lock as files, filled from `files/`, what the walk owed in it, for the upgrade of a format 2 or 3 folder
/// before its import.
pub(super) fn files_of(folder: &Folder) -> Result<(String, Lock), Error> {
    let dir = &folder.dir;
    let text = std::fs::read_to_string(dir.join(PROJECT)).map_err(|e| io(&dir.join(PROJECT), e))?;
    let mut lock = lock_file(dir)?;
    folder.fill(&mut lock, &measured(dir));
    if let Some(walk) = std::fs::read_to_string(dir.join(WALK)).ok().and_then(|text| serde_json::from_str::<WalkFile>(&text).ok()) {
        (lock.pending, lock.walk) = (walk.pending, walk.walk);
    }
    Ok((text, lock))
}

/// `project.canvas.yaml`'s rows under the names `renamed` gives, as an upgrade moves them before the import.
pub(super) fn canvas_renamed(dir: &Path, renamed: &dyn Fn(&str) -> Option<String>) -> Result<(), Error> {
    let at = dir.join(CANVAS);
    let Ok(text) = std::fs::read_to_string(&at) else { return Ok(()) };
    let rows: BTreeMap<String, protocol::project::Card> = canvas::legacy_rows(&text).into_iter().map(|(name, card)| (renamed(&name).unwrap_or(name), card)).collect();
    let mut out = "version: 1\ncards:\n".to_owned();
    for (name, card) in rows {
        let width = card.w.map_or(String::new(), |w| format!(", w: {w}"));
        out.push_str(&format!("  {}: {{x: {}, y: {}{width}}}\n", super::format::yaml_key(&name), card.x, card.y));
    }
    platform::durable::replace(&at, out.as_bytes(), 0o644).map_err(|e| io(&at, e))
}
