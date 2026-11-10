//! `project.db` (cl-803, `docs/PROJECT-DB.md`): one SQLite file in a project's folder, WAL, written by this Calcifer alone. `load` reads
//! canonical Automerge state into what `Folder` holds. Relational graph/history rows are disposable query projections; local
//! execution bookkeeping stays SQL. `commit` saves original CRDT changes and a recoverable snapshot in the same transaction.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
};

use api::error::{Error, Kind};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{
    crdt,
    folder::{Asset, File, Lock, Measured, RunFile, Version, WalkFile},
    graph::name_key,
    written::{Boxed, Written},
};

pub(crate) const DB: &str = "project.db";
/// This schema's `user_version`. A database of another isn't read.
const VERSION: i64 = 2;
const SCHEMA: &str = "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
    CREATE TABLE head (key TEXT PRIMARY KEY, ord INTEGER NOT NULL, value TEXT NOT NULL) STRICT;
    CREATE TABLE boxes (key TEXT PRIMARY KEY, name TEXT NOT NULL, ord INTEGER NOT NULL) STRICT;
    CREATE TABLE fields (
        box TEXT NOT NULL,
        ord INTEGER NOT NULL,
        field TEXT NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (box, field)
    ) STRICT, WITHOUT ROWID;
    CREATE TABLE canvas (key TEXT PRIMARY KEY, name TEXT NOT NULL, x REAL NOT NULL, y REAL NOT NULL, w REAL, seq INTEGER NOT NULL, by TEXT NOT NULL) STRICT;
    CREATE TABLE lock_assets (key TEXT PRIMARY KEY, record TEXT NOT NULL) STRICT;
    CREATE TABLE versions (
        key TEXT NOT NULL,
        number INTEGER NOT NULL,
        discarded INTEGER NOT NULL,
        record TEXT NOT NULL,
        PRIMARY KEY (key, number)
    ) STRICT, WITHOUT ROWID;
    CREATE TABLE lock_files (sha256 TEXT PRIMARY KEY, record TEXT NOT NULL) STRICT;
    CREATE TABLE run_files (ord INTEGER PRIMARY KEY, run INTEGER NOT NULL, function TEXT NOT NULL, output TEXT NOT NULL, sha256 TEXT NOT NULL) STRICT;
    CREATE TABLE facts (kind TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (kind, name)) STRICT, WITHOUT ROWID;
    CREATE TABLE walk (id INTEGER PRIMARY KEY CHECK (id = 1), record TEXT NOT NULL) STRICT;
    CREATE TABLE asked (digest TEXT PRIMARY KEY, text TEXT NOT NULL) STRICT;
    CREATE TABLE measured (sha256 TEXT PRIMARY KEY, record TEXT NOT NULL) STRICT;
    CREATE TABLE request_keys (key TEXT PRIMARY KEY, digest TEXT NOT NULL, status INTEGER, content_type TEXT, body BLOB, at_ms INTEGER NOT NULL) STRICT;
    CREATE TABLE ops (
        id INTEGER PRIMARY KEY,
        seq INTEGER NOT NULL,
        at_ms INTEGER NOT NULL,
        by TEXT NOT NULL,
        words TEXT NOT NULL,
        edits TEXT NOT NULL,
        changes TEXT NOT NULL,
        undone TEXT
    ) STRICT;";
const CRDT_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS crdt_document (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        format INTEGER NOT NULL,
        snapshot BLOB NOT NULL,
        heads TEXT NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS crdt_changes (
        hash TEXT PRIMARY KEY,
        seq INTEGER NOT NULL,
        actor TEXT NOT NULL,
        bytes BLOB NOT NULL
    ) STRICT;";

/// How long a request's answer is kept for its retries.
const KEY_KEPT_MS: i64 = 24 * 60 * 60 * 1000;

/// A box's row on the canvas, and the transaction and client that last wrote it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CardRow {
    pub name: String,
    pub card: api::calcifer::project::Card,
    pub seq: u64,
    pub by: String,
}

/// Everything `project.db` holds but its history, keys and asked texts: what `Folder` keeps in memory. Each part is shared, so a
/// transaction copies only what it writes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stored {
    /// The canonical document already decoded while this local project is held open.
    pub(super) crdt: Option<Arc<crdt::Document>>,
    pub seq: u64,
    pub written: Arc<Written>,
    /// By name key.
    pub canvas: Arc<BTreeMap<String, CardRow>>,
    /// Its files, assets, runs and facts: what the walk owes is in `walk`.
    pub lock: Arc<Lock>,
    pub walk: Arc<WalkFile>,
    pub measured: Arc<BTreeMap<String, Measured>>,
}

/// One history entry as its row holds it, each part JSON.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct OpRow {
    /// Stable history identity; `id` is only this replica's display number.
    #[serde(default)]
    pub uid: String,
    pub id: u64,
    pub at_ms: i64,
    pub by: String,
    pub words: String,
    pub edits: String,
    pub changes: String,
    pub undone: Option<String>,
}

/// What a transaction writes beside the rows it changed.
#[derive(Default)]
pub(crate) struct Extra {
    pub asked: Vec<(String, Option<String>)>,
    /// Every asked text gone.
    pub asked_cleared: bool,
    /// History rows written whole, or by id gone.
    pub ops: Vec<OpRow>,
    pub ops_gone: Vec<(String, u64)>,
    /// A request's key, with the digest of its ask: done, its answer kept after.
    pub key: Option<(String, String)>,
}

/// A kept request key: its ask's digest and, once kept, its answer.
pub(crate) struct Keyed {
    pub digest: String,
    pub answer: Option<(u16, String, Vec<u8>)>,
}

fn err(at: &Path, e: impl std::fmt::Display) -> Error {
    Error::new(Kind::Unavailable, "project_db", format!("{}: {e}", at.display()))
}

/// A SQLite error at `at`. Its write lock held past the busy timeout is another program's open transaction: `file_held`, which the page says
/// and waits out, and a landing is owed.
fn sql(at: &Path, e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => held(at),
        _ => err(at, e),
    }
}

/// `at` held by another program: its write lock past the busy timeout.
pub(crate) fn held(at: &Path) -> Error {
    let name = at.file_name().map_or_else(|| at.display().to_string(), |name| name.to_string_lossy().into_owned());
    Error::new(Kind::Unavailable, "file_held", format!("{name} is open in another program")).remedy("close it, and save again")
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("a project row serializes")
}

/// How long a write waits on another program's write lock before it's `file_held`.
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether another program holds the write lock this moment, asked with no wait: `conn` takes it, and lets it go at once.
pub(crate) fn locked_elsewhere(conn: &Connection) -> bool {
    let busy = |e: rusqlite::Error| matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked));
    if conn.busy_timeout(std::time::Duration::ZERO).is_err() {
        return false;
    }
    let held = conn.execute_batch("BEGIN IMMEDIATE; ROLLBACK").is_err_and(busy);
    let _ = conn.busy_timeout(WAIT);
    held
}

/// `project.db` in `dir` opened, made with this schema if it's new.
pub(crate) fn open(path: &Path) -> Result<Connection, Error> {
    let e = |error: rusqlite::Error| sql(path, error);
    let mut conn = Connection::open(path).map_err(e)?;
    conn.busy_timeout(WAIT).map_err(e)?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
    conn.pragma_update(None, "synchronous", "FULL").map_err(e)?;
    // Checkpoints run beside the writes (`checkpoint`), never in a commit: on a busy disk one's fsync takes a second.
    conn.pragma_update(None, "wal_autocheckpoint", 0).map_err(e)?;
    match conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).map_err(e)? {
        0 => {
            conn.execute_batch(&format!("BEGIN IMMEDIATE; {SCHEMA}; {CRDT_SCHEMA}; PRAGMA user_version = 1; INSERT INTO meta VALUES ('seq', '0'); COMMIT;")).map_err(e)?;
            initialize_crdt(&mut conn, path)?;
        }
        1 => {
            // VACUUM includes committed WAL pages and leaves a usable original before migration.
            // Never overwrite a user's earlier recovery copy.
            let backup = path.with_file_name(format!("{}.v1", path.file_name().unwrap_or_default().to_string_lossy()));
            if !backup.exists() {
                conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()]).map_err(e)?;
            }
            verify_v1_backup(&conn, &backup)?;
            initialize_crdt(&mut conn, path)?;
        }
        VERSION => {}
        other => return Err(Error::new(Kind::Validation, "project_db_version", format!("{} is of schema {other}, and this cozy reads {VERSION}", path.display())).next("cozy update")),
    }
    Ok(conn)
}

/// A kill can leave VACUUM's output partly written. Never treat its mere existence as a
/// recovery copy, and make its bytes/name durable before changing the intact v1 primary.
fn verify_v1_backup(primary: &Connection, at: &Path) -> Result<(), Error> {
    let checked = || -> Result<(), Error> {
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let copy = Connection::open_with_flags(at, flags).map_err(|e| sql(at, e))?;
        let version: i64 = copy.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(|e| sql(at, e))?;
        let integrity: String = copy.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(|e| sql(at, e))?;
        if version != 1 || integrity != "ok" {
            return Err(err(at, "the migration recovery copy is incomplete or is not schema 1"));
        }
        if meta(primary, "id") != meta(&copy, "id") {
            return Err(err(at, "the migration recovery copy belongs to a different project"));
        }
        legacy_load(&copy, at)?;
        legacy_ops(&copy, at)?;
        for table in ["asked", "request_keys"] {
            copy.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).map_err(|e| sql(at, e))?;
        }
        drop(copy);
        std::fs::OpenOptions::new().read(true).write(true).open(at).and_then(|file| file.sync_all()).map_err(|e| err(at, e))?;
        platform::durable::sync_dir(at.parent().unwrap_or_else(|| Path::new("."))).map_err(|e| err(at, e))
    };
    checked().map_err(|error| {
        Error::new(Kind::Unavailable, "project_recovery_backup", format!("{}; the original project remains schema 1", error.message))
            .remedy("preserve the recovery copy and move it aside before retrying migration")
    })
}

/// The project's id and name, as `meta` keeps them (cl-819); none before it was given one.
pub(crate) fn identity(conn: &Connection) -> Option<(String, String)> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).ok()?;
    if version == VERSION {
        let (format, bytes, heads): (i64, Vec<u8>, String) = conn.query_row("SELECT format, snapshot, heads FROM crdt_document WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).ok()?;
        return (format == crdt::FORMAT).then(|| crdt::identity_bytes(&bytes, &heads)).flatten();
    }
    if version != 1 { return None; }
    // Only the first import of a v1 database reads identity from the relational store.
    let value = |key: &str| conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get::<_, String>(0)).optional().ok().flatten();
    Some((value("id")?, value("name").unwrap_or_default()))
}

/// Keeps the project's id and name, in one transaction.
pub(crate) fn set_identity(conn: &mut Connection, at: &Path, id: &str, name: &str) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| sql(at, e))?;
    for (key, value) in [("id", id), ("name", name)] {
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", [key, value]).map_err(|e| sql(at, e))?;
    }
    let mut doc = read_document(&tx, at)?;
    let old_heads = doc.doc.get_heads();
    doc.set_identity(id, name)?;
    let seq = meta(&tx, "seq").and_then(|value| value.parse().ok()).unwrap_or(0);
    save_document(&tx, at, &mut doc, seq, Some(&old_heads))?;
    tx.commit().map_err(|e| sql(at, e))
}

/// `meta`'s `key`, if it has one.
pub(crate) fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get::<_, String>(0)).optional().ok().flatten()
}

/// Keeps `value` as `meta`'s `key`, in a transaction of its own: no part of the project changes, and `seq` doesn't move.
pub(crate) fn set_meta(conn: &Connection, at: &Path, key: &str, value: Option<&str>) -> Result<(), Error> {
    let done = match value {
        Some(value) => conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", [key, value]),
        None => conn.execute("DELETE FROM meta WHERE key = ?1", [key]),
    };
    done.map(drop).map_err(|e| sql(at, e))
}

/// How many of its transactions changed it, read alone: for a project nothing holds open.
pub(crate) fn seq_at(path: &Path) -> Option<u64> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).ok()?;
    conn.query_row("SELECT value FROM meta WHERE key = 'seq'", [], |r| r.get::<_, String>(0)).optional().ok().flatten()?.parse().ok()
}

/// What another connection's commit changes: `Folder` reads again when this moves.
pub(crate) fn data_version(conn: &Connection) -> i64 {
    conn.pragma_query_value(None, "data_version", |r| r.get(0)).unwrap_or(0)
}

/// `load` in one read transaction, and the `data_version` it's of.
pub(crate) fn read(conn: &Connection, at: &Path) -> Result<(i64, Stored), Error> {
    let tx = conn.unchecked_transaction().map_err(|e| sql(at, e))?;
    let stored = load(&tx, at)?;
    Ok((data_version(&tx), stored))
}

/// All of it but the history, keys and asked texts.
fn legacy_load(conn: &Connection, at: &Path) -> Result<Stored, Error> {
    let e = |error: rusqlite::Error| sql(at, error);
    let bad = |what: &str, why: serde_json::Error| err(at, format!("{what}: {why}"));
    let (mut written, mut canvas, mut lock, mut walk, mut measured) = (Written::default(), BTreeMap::new(), Lock::default(), WalkFile::default(), BTreeMap::new());
    let seq = conn.query_row("SELECT value FROM meta WHERE key = 'seq'", [], |r| r.get::<_, String>(0)).optional().map_err(e)?.and_then(|seq| seq.parse().ok()).unwrap_or(0);
    let mut rows = conn.prepare("SELECT key, value FROM head ORDER BY ord").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(e)? {
        let (key, value) = row.map_err(e)?;
        written.head.push((key, serde_json::from_str(&value).map_err(|why| bad("head", why))?));
    }
    let mut boxes: BTreeMap<String, (i64, Boxed)> = BTreeMap::new();
    let mut rows = conn.prepare("SELECT key, name, ord FROM boxes").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))).map_err(e)? {
        let (key, name, ord) = row.map_err(e)?;
        boxes.insert(key, (ord, Boxed { name, ..Default::default() }));
    }
    let mut rows = conn.prepare("SELECT box, field, value FROM fields ORDER BY box, ord").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).map_err(e)? {
        let (key, field, value) = row.map_err(e)?;
        if let Some((_, held)) = boxes.get_mut(&key) {
            held.fields.push((field, serde_json::from_str(&value).map_err(|why| bad("fields", why))?));
        }
    }
    let mut boxes: Vec<(i64, Boxed)> = boxes.into_values().collect();
    boxes.sort_by_key(|(ord, _)| *ord);
    written.boxes = boxes.into_iter().map(|(_, held)| held).collect();
    let mut rows = conn.prepare("SELECT key, name, x, y, w, seq, by FROM canvas").map_err(e)?;
    for row in rows.query_map([], |r| {
        let card = api::calcifer::project::Card { x: r.get(2)?, y: r.get(3)?, w: r.get(4)? };
        Ok((r.get::<_, String>(0)?, CardRow { name: r.get(1)?, card, seq: r.get::<_, i64>(5)? as u64, by: r.get(6)? }))
    }).map_err(e)? {
        let (key, row) = row.map_err(e)?;
        canvas.insert(key, row);
    }
    let mut rows = conn.prepare("SELECT key, record FROM lock_assets").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(e)? {
        let (key, record) = row.map_err(e)?;
        lock.assets.insert(key, serde_json::from_str(&record).map_err(|why| bad("lock_assets", why))?);
    }
    let mut rows = conn.prepare("SELECT key, discarded, record FROM versions ORDER BY key, number").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?))).map_err(e)? {
        let (key, discarded, record) = row.map_err(e)?;
        let version: Version = serde_json::from_str(&record).map_err(|why| bad("versions", why))?;
        let held = lock.assets.entry(key).or_default();
        if discarded { held.discarded.push(version) } else { held.versions.push(version) }
    }
    let mut rows = conn.prepare("SELECT sha256, record FROM lock_files").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(e)? {
        let (sha256, record) = row.map_err(e)?;
        let file = File { row: true, ..serde_json::from_str(&record).map_err(|why| bad("lock_files", why))? };
        lock.files.insert(sha256, file);
    }
    let mut rows = conn.prepare("SELECT run, function, output, sha256 FROM run_files ORDER BY ord").map_err(e)?;
    for row in rows.query_map([], |r| Ok(RunFile { run: r.get::<_, i64>(0)? as u64, function: r.get(1)?, output: r.get(2)?, sha256: r.get(3)?, ..Default::default() })).map_err(e)? {
        lock.runs.push(row.map_err(e)?);
    }
    let mut rows = conn.prepare("SELECT kind, name, value FROM facts").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).map_err(e)? {
        let (kind, name, value) = row.map_err(e)?;
        match kind.as_str() {
            "reserved" => drop(lock.reserved.insert(name, serde_json::from_str(&value).map_err(|why| bad("facts", why))?)),
            "makes" => drop(lock.makes.insert(name, value)),
            _ => {}
        }
    }
    if let Some(record) = conn.query_row("SELECT record FROM walk WHERE id = 1", [], |r| r.get::<_, String>(0)).optional().map_err(e)? {
        walk = serde_json::from_str(&record).map_err(|why| bad("walk", why))?;
    }
    let mut rows = conn.prepare("SELECT sha256, record FROM measured").map_err(e)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(e)? {
        let (sha256, record) = row.map_err(e)?;
        measured.insert(sha256, serde_json::from_str(&record).map_err(|why| bad("measured", why))?);
    }
    Ok(Stored { seq, written: Arc::new(written), canvas: Arc::new(canvas), lock: Arc::new(lock), walk: Arc::new(walk), measured: Arc::new(measured), crdt: None })
}

/// Canonical project state plus machine-local bookkeeping. SQL projection damage never becomes a
/// new authoritative project edit on read.
pub(crate) fn load(conn: &Connection, at: &Path) -> Result<Stored, Error> {
    let doc = read_document(conn, at)?;
    let mut stored = doc.stored()?;
    stored.seq = meta(conn, "seq").and_then(|seq| seq.parse().ok()).unwrap_or(0);
    if let Some(record) = conn.query_row("SELECT record FROM walk WHERE id = 1", [], |row| row.get::<_, String>(0)).optional().map_err(|e| sql(at, e))? {
        stored.walk = Arc::new(serde_json::from_str(&record).map_err(|e| err(at, e))?);
    }
    let mut measured = BTreeMap::new();
    let mut rows = conn.prepare("SELECT sha256, record FROM measured").map_err(|e| sql(at, e))?;
    for row in rows.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(|e| sql(at, e))? {
        let (hash, record) = row.map_err(|e| sql(at, e))?;
        measured.insert(hash, serde_json::from_str(&record).map_err(|e| err(at, e))?);
    }
    stored.measured = Arc::new(measured);
    Ok(stored)
}

fn read_document(conn: &Connection, at: &Path) -> Result<crdt::Document, Error> {
    let (format, bytes, heads): (i64, Vec<u8>, String) = conn.query_row("SELECT format, snapshot, heads FROM crdt_document WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(|e| sql(at, e))?;
    if format != crdt::FORMAT {
        return Err(err(at, format!("unknown CRDT storage format {format}; update cozy")));
    }
    crdt::Document::load(&bytes, &heads, at)
}

fn save_document(tx: &Transaction<'_>, at: &Path, doc: &mut crdt::Document, seq: u64, base: Option<&[automerge::ChangeHash]>) -> Result<(), Error> {
    if let Some(change) = doc.doc.get_last_local_change() {
        tx.execute("INSERT OR IGNORE INTO crdt_changes (hash, seq, actor, bytes) VALUES (?1, ?2, ?3, ?4)", params![change.hash().to_string(), seq as i64, change.actor_id().to_hex_string(), change.raw_bytes()]).map_err(|e| sql(at, e))?;
    }
    let heads: Vec<String> = doc.doc.get_heads().iter().map(ToString::to_string).collect();
    match base {
        None => {
            let bytes = doc.doc.save();
            tx.execute("INSERT OR REPLACE INTO crdt_document (id, format, snapshot, heads) VALUES (1, ?1, ?2, ?3)", params![crdt::FORMAT, bytes, json(&heads)]).map_err(|e| sql(at, e))?;
        }
        Some(base) => {
            let changed = || Error::new(Kind::Conflict, "project_changed", "the project changed while this edit was being committed; read it again and retry");
            let token: String = tx.query_row("SELECT heads FROM crdt_document WHERE id = 1", [], |row| row.get(0)).map_err(|e| sql(at, e))?;
            if crdt::head_set(&token)? != base.iter().copied().collect() { return Err(changed()); }
            // Explicit heads avoid AutoCommit::load's empty save cursor. The result is
            // supported native Change chunks, appended to the complete native stream.
            let bytes = doc.doc.save_after(base);
            let wrote = tx.execute("UPDATE crdt_document SET snapshot = CAST(snapshot || ?1 AS BLOB), heads = ?2 WHERE id = 1 AND heads = ?3", params![bytes, json(&heads), token]).map_err(|e| sql(at, e))?;
            if wrote != 1 { return Err(changed()); }
        }
    }
    Ok(())
}

/// Convert a relational v1 once, preserving a recoverable v1 SQLite snapshot alongside it. The new
/// canonical snapshot, its immutable change bytes and normalized query projections commit together.
fn initialize_crdt(conn: &mut Connection, at: &Path) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| sql(at, e))?;
    tx.execute_batch(CRDT_SCHEMA).map_err(|e| sql(at, e))?;
    let before = legacy_load(&tx, at)?;
    let mut after = before.clone();
    crdt::identify(&before, &mut after);
    let mut doc = crdt::Document::new(at);
    let mut ops = legacy_ops(&tx, at)?;
    crdt::identify_history(&after, &mut ops);
    // Before stored IDs, indexed projects were identified by their canonical folder path.
    // Preserve that ID on legacy import/restore; new-project creation explicitly replaces
    // it with its freshly generated UUID after this initializer completes.
    let inherited = identity(&tx).unwrap_or_else(|| {
        (super::folder_id(at.parent().unwrap_or_else(|| Path::new("."))), String::new())
    });
    doc.update(crdt::value(&after, &ops, &serde_json::Value::Null, Some(inherited)))?;
    write(&tx, &before, &after, &Extra { ops:ops.clone(), ..Default::default() }).map_err(|e| sql(at, e))?;
    save_document(&tx, at, &mut doc, after.seq, None)?;
    tx.pragma_update(None, "user_version", VERSION).map_err(|e| sql(at, e))?;
    tx.commit().map_err(|e| sql(at, e))
}

/// The asset row of the lock, without its versions, which have rows of their own.
fn asset_record(held: &Asset) -> String {
    json(&Asset { versions: vec![], discarded: vec![], ..held.clone() })
}

/// A file's row: only what the file can't say of itself.
fn file_record(file: &File) -> String {
    json(file)
}

/// `after` written over `before`, row by row, with `extra`, as one transaction: the new revision, or none when nothing differed.
/// `locked` runs once it holds the write lock. `before_commit` runs only at the final
/// SQLite commit, allowing the caller to publish its held view without a reload gap.
pub(crate) fn commit(conn: &mut Connection, at: &Path, before: &Stored, after: &mut Stored, extra: &Extra, locked: impl FnOnce(), before_commit: impl FnOnce()) -> Result<Option<u64>, Error> {
    let e = |error: rusqlite::Error| sql(at, error);
    crdt::identify(before, after);
    if !Arc::ptr_eq(&before.lock, &after.lock) {
        Arc::make_mut(&mut after.lock).mark_rows();
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
    locked();
    let was = tx.total_changes();
    write(&tx, before, after, extra).map_err(e)?;
    if tx.total_changes() == was {
        tx.rollback().map_err(e)?;
        return Ok(None);
    }
    let seq = before.seq + 1;
    let mut doc = match &before.crdt {
        Some(doc) => (**doc).clone(),
        None => read_document(&tx, at)?,
    };
    let old_heads = doc.doc.get_heads();
    let identity = doc.identity();
    let mut ops = doc.ops()?;
    ops.retain(|row| !extra.ops_gone.iter().any(|(uid, _)| uid == &row.uid) && !extra.ops.iter().any(|next| next.uid == row.uid));
    ops.extend(extra.ops.iter().cloned());
    let value = crdt::changed_value(before, after, &ops, &doc.value, identity);
    doc.update(value)?;
    if doc.doc.get_heads() != old_heads {
        // A fresh staged store's first populated document is its full baseline, not
        // a large change tail on the empty initializer. Normal edits stay incremental.
        let base = before.crdt.as_ref().map(|_| old_heads.as_slice());
        save_document(&tx, at, &mut doc, seq, base)?;
    }
    after.crdt = Some(Arc::new(doc));
    tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('seq', ?1)", [seq.to_string()]).map_err(e)?;
    before_commit();
    tx.commit().map_err(e)?;
    Ok(Some(seq))
}

fn write(tx: &Transaction, before: &Stored, after: &Stored, extra: &Extra) -> rusqlite::Result<()> {
    let seq = before.seq + 1;
    if before.written.head != after.written.head {
        tx.execute("DELETE FROM head", [])?;
        for (ord, (key, value)) in after.written.head.iter().enumerate() {
            tx.execute("INSERT INTO head (key, ord, value) VALUES (?1, ?2, ?3)", params![key, ord as i64, json(value)])?;
        }
    }
    let was: BTreeMap<String, (usize, &Boxed)> = before.written.boxes.iter().enumerate().map(|(ord, held)| (name_key(&held.name), (ord, held))).collect();
    let now: BTreeMap<String, (usize, &Boxed)> = after.written.boxes.iter().enumerate().map(|(ord, held)| (name_key(&held.name), (ord, held))).collect();
    for key in was.keys().filter(|key| !now.contains_key(*key)) {
        tx.execute("DELETE FROM fields WHERE box = ?1", [key])?;
        tx.execute("DELETE FROM boxes WHERE key = ?1", [key])?;
    }
    for (key, (ord, held)) in &now {
        match was.get(key) {
            Some((was_ord, was_held)) if was_held == held => {
                if was_ord != ord {
                    tx.execute("UPDATE boxes SET ord = ?2 WHERE key = ?1", params![key, *ord as i64])?;
                }
            }
            _ => {
                tx.execute("INSERT OR REPLACE INTO boxes (key, name, ord) VALUES (?1, ?2, ?3)", params![key, held.name, *ord as i64])?;
                tx.execute("DELETE FROM fields WHERE box = ?1", [key])?;
                for (n, (field, value)) in held.fields.iter().enumerate() {
                    tx.execute("INSERT INTO fields (box, ord, field, value) VALUES (?1, ?2, ?3, ?4)", params![key, n as i64, field, json(value)])?;
                }
            }
        }
    }
    for key in before.canvas.keys().filter(|key| !after.canvas.contains_key(*key)) {
        tx.execute("DELETE FROM canvas WHERE key = ?1", [key])?;
    }
    for (key, row) in after.canvas.iter().filter(|(key, row)| before.canvas.get(*key) != Some(row)) {
        tx.execute("INSERT OR REPLACE INTO canvas (key, name, x, y, w, seq, by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![key, row.name, row.card.x, row.card.y, row.card.w, row.seq as i64, row.by])?;
    }
    lock_written(tx, &before.lock, &after.lock)?;
    let walk = json(&*after.walk);
    if json(&*before.walk) != walk {
        match after.walk.is_empty() && after.walk.bound.is_none() {
            true => drop(tx.execute("DELETE FROM walk", [])?),
            false => drop(tx.execute("INSERT OR REPLACE INTO walk (id, record) VALUES (1, ?1)", [walk])?),
        }
    }
    for sha256 in before.measured.keys().filter(|sha256| !after.measured.contains_key(*sha256)) {
        tx.execute("DELETE FROM measured WHERE sha256 = ?1", [sha256])?;
    }
    for (sha256, held) in after.measured.iter().filter(|(sha256, held)| before.measured.get(*sha256) != Some(held)) {
        tx.execute("INSERT OR REPLACE INTO measured (sha256, record) VALUES (?1, ?2)", params![sha256, json(held)])?;
    }
    if extra.asked_cleared {
        tx.execute("DELETE FROM asked", [])?;
    }
    for (digest, text) in &extra.asked {
        match text {
            Some(text) => drop(tx.execute("INSERT OR REPLACE INTO asked (digest, text) VALUES (?1, ?2)", params![digest, text])?),
            None => drop(tx.execute("DELETE FROM asked WHERE digest = ?1", [digest])?),
        }
    }
    for (_, id) in &extra.ops_gone {
        tx.execute("DELETE FROM ops WHERE id = ?1", [*id as i64])?;
    }
    for op in &extra.ops {
        tx.execute(
            "INSERT OR REPLACE INTO ops (id, seq, at_ms, by, words, edits, changes, undone) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![op.id as i64, seq as i64, op.at_ms, op.by, op.words, op.edits, op.changes, op.undone],
        )?;
    }
    if let Some((key, digest)) = &extra.key {
        let now = super::now_ms();
        tx.execute("DELETE FROM request_keys WHERE at_ms < ?1", [now - KEY_KEPT_MS])?;
        tx.execute("INSERT OR REPLACE INTO request_keys (key, digest, at_ms) VALUES (?1, ?2, ?3)", params![key, digest, now])?;
    }
    Ok(())
}

/// The lock's rows that differ: an asset whose record or versions moved, a file whose row did, the runs and the facts.
fn lock_written(tx: &Transaction, before: &Lock, after: &Lock) -> rusqlite::Result<()> {
    for key in before.assets.keys().filter(|key| !after.assets.contains_key(*key)) {
        tx.execute("DELETE FROM versions WHERE key = ?1", [key])?;
        tx.execute("DELETE FROM lock_assets WHERE key = ?1", [key])?;
    }
    for (key, held) in &after.assets {
        let was = before.assets.get(key);
        if was == Some(held) {
            continue;
        }
        let record = asset_record(held);
        if was.is_none_or(|was| asset_record(was) != record) {
            tx.execute("INSERT OR REPLACE INTO lock_assets (key, record) VALUES (?1, ?2)", params![key, record])?;
        }
        fn numbered(held: Option<&Asset>) -> BTreeMap<u32, (bool, &Version)> {
            held.into_iter().flat_map(|held| held.versions.iter().map(|v| (v.number, (false, v))).chain(held.discarded.iter().map(|v| (v.number, (true, v))))).collect()
        }
        let (old, new) = (numbered(was), numbered(Some(held)));
        for number in old.keys().filter(|number| !new.contains_key(*number)) {
            tx.execute("DELETE FROM versions WHERE key = ?1 AND number = ?2", params![key, *number as i64])?;
        }
        for (number, (discarded, version)) in new.iter().filter(|(number, row)| old.get(*number) != Some(row)) {
            tx.execute("INSERT OR REPLACE INTO versions (key, number, discarded, record) VALUES (?1, ?2, ?3, ?4)", params![key, *number as i64, discarded, json(*version)])?;
        }
    }
    let row = |lock: &Lock, sha256: &str| lock.files.get(sha256).filter(|file| file.row).map(file_record);
    for (sha256, _) in before.files.iter().filter(|(sha256, file)| file.row && row(after, sha256).is_none()) {
        tx.execute("DELETE FROM lock_files WHERE sha256 = ?1", [sha256])?;
    }
    for (sha256, file) in after.files.iter().filter(|(_, file)| file.row) {
        let record = file_record(file);
        if row(before, sha256).is_none_or(|was| was != record) {
            tx.execute("INSERT OR REPLACE INTO lock_files (sha256, record) VALUES (?1, ?2)", params![sha256, record])?;
        }
    }
    if before.runs != after.runs {
        tx.execute("DELETE FROM run_files", [])?;
        for (ord, row) in after.runs.iter().enumerate() {
            tx.execute("INSERT INTO run_files (ord, run, function, output, sha256) VALUES (?1, ?2, ?3, ?4, ?5)", params![ord as i64, row.run as i64, row.function, row.output, row.sha256])?;
        }
    }
    if before.reserved != after.reserved || before.makes != after.makes {
        tx.execute("DELETE FROM facts", [])?;
        for (name, tags) in &after.reserved {
            tx.execute("INSERT INTO facts (kind, name, value) VALUES ('reserved', ?1, ?2)", params![name, json(tags)])?;
        }
        for (tool, kind) in &after.makes {
            tx.execute("INSERT INTO facts (kind, name, value) VALUES ('makes', ?1, ?2)", params![tool, kind])?;
        }
    }
    Ok(())
}

/// The text a render was asked with, by its digest.
pub(crate) fn asked(conn: &Connection, digest: &str) -> Option<String> {
    conn.query_row("SELECT text FROM asked WHERE digest = ?1", [digest], |r| r.get(0)).optional().ok().flatten()
}

/// Every history row, oldest first.
fn legacy_ops(conn: &Connection, at: &Path) -> Result<Vec<OpRow>, Error> {
    let e = |error: rusqlite::Error| sql(at, error);
    let mut rows = conn.prepare("SELECT id, at_ms, by, words, edits, changes, undone FROM ops ORDER BY id").map_err(e)?;
    let found = rows
        .query_map([], |r| Ok(OpRow { uid: String::new(), id: r.get::<_, i64>(0)? as u64, at_ms: r.get(1)?, by: r.get(2)?, words: r.get(3)?, edits: r.get(4)?, changes: r.get(5)?, undone: r.get(6)? }))
        .map_err(e)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(e)?;
    Ok(found)
}

/// What the request under `key` asked and answered, if it was kept.
pub(crate) fn keyed(conn: &Connection, key: &str) -> Option<Keyed> {
    conn.query_row("SELECT digest, status, content_type, body FROM request_keys WHERE key = ?1", [key], |r| {
        let answer = match r.get::<_, Option<i64>>(1)? {
            Some(status) => Some((status as u16, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default())),
            None => None,
        };
        Ok(Keyed { digest: r.get(0)?, answer })
    })
    .optional()
    .ok()
    .flatten()
}

/// The answer a key's request gave, kept on its row: written after the transaction that did it, so the effect never waits on its words.
pub(crate) fn keep_answer(conn: &Connection, key: &str, digest: &str, status: u16, content_type: &str, body: &[u8]) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO request_keys (key, digest, status, content_type, body, at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (key) DO UPDATE SET status = excluded.status, content_type = excluded.content_type, body = excluded.body",
        params![key, digest, status as i64, content_type, body, super::now_ms()],
    )
    .map(drop)
}

/// A fresh database at `path` holding `stored`, its history and its asked texts: what a first open imports.
pub(crate) fn create(path: &Path, stored: &Stored, ops: &[OpRow], asked: &[(String, String)]) -> Result<(), Error> {
    let _ = std::fs::remove_file(path);
    let mut conn = open(path)?;
    let mut fresh = stored.clone();
    crdt::identify(&Stored::default(), &mut fresh);
    let mut ops = ops.to_vec();
    crdt::identify_history(&fresh, &mut ops);
    let extra = Extra { asked: asked.iter().map(|(digest, text)| (digest.clone(), Some(text.clone()))).collect(), ops, ..Default::default() };
    fresh.seq = 0;
    commit(&mut conn, path, &Stored::default(), &mut fresh, &extra, || (), || ())?;
    set_meta(&conn, path, "created_complete", Some("1"))?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").map_err(|e| err(path, e))?;
    drop(conn);
    Ok(())
}

/// What the WAL holds moved into the database, from a connection of its own, as far as readers let it: writes go on meanwhile.
pub(crate) fn checkpoint(path: &Path) {
    let done = Connection::open(path).and_then(|conn| conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())));
    if let Err(error) = done {
        tracing::warn!("{}: checkpoint: {error}", path.display());
    }
}
