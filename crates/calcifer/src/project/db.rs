//! `project.db` (cl-803, `docs/PROJECT-DB.md`): one SQLite file in a project's folder, WAL, written by this Calcifer alone. The Automerge
//! document is canonical; `commit` saves its changes and, in the same transaction, the rows its boxes, head, canvas, heads and
//! alternatives project to, the document they stand at and their digest (`rows_kept`). `load` reads those rows, so a read never decodes the
//! document (cl-885: 600 ms of a 5,000-box project's every open); it's decoded only to write, to merge, for history (`document`), or when
//! the rows don't stand at it. `integrity` holds the rows to the document.
use std::{
    collections::{BTreeMap, HashMap},
    fs::OpenOptions,
    path::Path,
    sync::Arc,
};

use api::error::{Error, Kind};
use rusqlite::{Connection, OptionalExtension, ToSql, Transaction, params, types::Value as Sql};
use sha2::{Digest as _, Sha256};

use super::{
    crdt,
    folder::{Asset, File, Lock, Measured, Version, WalkFile},
    graph::name_key,
    written::{Boxed, Written},
};

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;

pub(crate) const DB: &str = "project.db";
/// Its write-ahead log, where a commit lands first.
pub(crate) const WAL: &str = "project.db-wal";
/// This schema's `user_version`: 3's, with each box's stable id on its row, so the rows alone say what the document does (cl-885). A
/// database of another isn't read; an older one from 2 is migrated (`migrate`). A new schema is this number and its step in `migrate`.
pub(super) const VERSION: i64 = 4;
const SCHEMA: &str = "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
    CREATE TABLE head (key TEXT PRIMARY KEY, ord INTEGER NOT NULL, value TEXT NOT NULL) STRICT;
    CREATE TABLE boxes (key TEXT PRIMARY KEY, name TEXT NOT NULL, ord INTEGER NOT NULL, id TEXT NOT NULL DEFAULT '') STRICT;
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
    CREATE TABLE lock_runs (ord INTEGER PRIMARY KEY, record TEXT NOT NULL) STRICT;
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
    CREATE TABLE crdt_document (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        format INTEGER NOT NULL,
        snapshot BLOB NOT NULL,
        heads TEXT NOT NULL
    ) STRICT;
    CREATE TABLE crdt_changes (
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
    /// The canonical document, once something decoded it: a write, a merge or history. A read takes the rows (`load`).
    pub(super) crdt: Option<Arc<crdt::Document>>,
    /// The document's heads the rest stands at, kept on its row: what words typed on it were typed on.
    pub heads: Arc<Vec<String>>,
    /// The concurrent alternatives the document keeps, as its last commit found them.
    pub conflicts: Arc<Vec<String>>,
    /// Whether project.db's rows are what this holds, their digest checked as they were read or kept as they were written: a write then
    /// writes only what it changed of them.
    pub(super) rows_held: bool,
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
    /// A request's answer, said in its act, under its key (cl-885).
    pub answer: Option<(String, Said)>,
}

/// A request's answer as its act says it: its status, and its JSON made at the heads the act's commit leaves the document at.
pub(crate) struct Said {
    pub status: u16,
    pub body: Box<dyn Fn(&[String]) -> Vec<u8> + Send>,
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

/// A connection to `path` that waits `WAIT` on another's lock, as every one to a project.db does (cl-885: rusqlite's own 5 s wait
/// refused a run while the project's last connection checkpointed its WAL as it closed).
pub(crate) fn connect(path: &Path, read_only: bool) -> Result<Connection, Error> {
    let flags = match read_only {
        true => rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        false => rusqlite::OpenFlags::default() & !rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
    };
    let conn = Connection::open_with_flags(path, flags).map_err(|error| sql(path, error))?;
    conn.busy_timeout(WAIT).map_err(|error| sql(path, error))?;
    Ok(conn)
}

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

/// Open an existing current-format project. Read-only validation precedes every writable pragma.
pub(crate) fn open(path: &Path) -> Result<Connection, Error> {
    migrate(path)?;
    let checked = connect(path, true)?;
    require_current(&checked, path)?;
    drop(checked);
    let conn = connect(path, false)?;
    require_current(&conn, path)?;
    configure(&conn, path)?;
    Ok(conn)
}

/// Schema 2 kept a project's files, versions and runs in its Automerge document as well as in rows. Schema 3 keeps them in rows
/// alone (cl-904, Paul 11-12: the CRDT is for node state, never files): the document's are written to the rows once, which then
/// stand, and the document's next change leaves them out. Schema 4 reads a project from its rows (cl-885): it adds each box's id to its
/// row, and `rows_at` and `conflicts` to `meta`, and changes no row 3 has. One transaction from any older schema; the project as it
/// was is kept beside it once, `project.db.pre-schema4`, never read.
fn migrate(path: &Path) -> Result<(), Error> {
    let e = |error| sql(path, error);
    let version = |conn: &Connection| conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0));
    let older = |version: i64| (2..VERSION).contains(&version);
    let checked = connect(path, true)?;
    if !older(version(&checked).map_err(e)?) {
        return Ok(());
    }
    let kept = path.with_file_name(format!("{DB}.pre-schema{VERSION}"));
    if !kept.exists() {
        let part = path.with_file_name(format!("{DB}.pre-schema{VERSION}.part"));
        checked.backup(rusqlite::MAIN_DB, &part, rusqlite::backup::NO_PROGRESS).map_err(e)?;
        std::fs::rename(&part, &kept).map_err(|error| err(path, error))?;
        tracing::info!("project.db before schema {VERSION} kept at {}", kept.display());
    }
    drop(checked);
    let mut conn = connect(path, false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
    // Another process may have migrated it while this waited.
    let from = version(&tx).map_err(e)?;
    if !older(from) {
        return Ok(());
    }
    let doc = read_document(&tx, path)?;
    if from == 2 {
        let lock = doc.legacy_lock()?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS lock_runs (ord INTEGER PRIMARY KEY, record TEXT NOT NULL) STRICT;
            DELETE FROM lock_assets; DELETE FROM versions; DELETE FROM lock_files; DELETE FROM run_files; DELETE FROM lock_runs; DELETE FROM facts;").map_err(e)?;
        lock_written(&tx, &Lock::default(), &lock).map_err(e)?;
    }
    rows_of(&tx, path, doc)?;
    tx.pragma_update(None, "user_version", VERSION).map_err(e)?;
    tx.commit().map_err(e)
}

/// What a read takes, added beside the rows a project already has: each box's id on its row, and its identity and alternatives in
/// `meta` where they're missing. No row there is changed. The rows stand (`rows_at`) when they're what the document says; else a read
/// takes the document and the next write writes them whole.
fn rows_of(tx: &Transaction, at: &Path, doc: crdt::Document) -> Result<(), Error> {
    let e = |error| sql(at, error);
    let has_id: i64 = tx.query_row("SELECT COUNT(*) FROM pragma_table_info('boxes') WHERE name = 'id'", [], |row| row.get(0)).map_err(e)?;
    if has_id == 0 {
        tx.execute_batch("ALTER TABLE boxes ADD COLUMN id TEXT NOT NULL DEFAULT ''").map_err(e)?;
    }
    let identity = doc.identity();
    let mut projected = from_document(doc)?;
    Arc::make_mut(&mut projected.written).in_stored_order();
    // A row holds the box the document selects for its name, its last of that name.
    for held in projected.written.boxes.iter() {
        tx.execute("UPDATE boxes SET id = ?1 WHERE key = ?2 AND name = ?3", params![held.id, name_key(&held.name), held.name]).map_err(e)?;
    }
    for (key, value) in identity.into_iter().flat_map(|(id, name)| [("id", id), ("name", name)]).chain([(CONFLICTS, json(&*projected.conflicts))]) {
        tx.execute("INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)", params![key, value]).map_err(e)?;
    }
    let (rows, digest) = read_rows(tx, at)?;
    if rows_differ(&rows, &projected).is_empty() {
        rows_written(tx, at, &projected.written, digest)?;
    }
    Ok(())
}

/// The identity a read takes, beside the document's.
fn identity_rows(conn: &Connection, id: &str, name: &str) -> rusqlite::Result<()> {
    for (key, value) in [("id", id), ("name", name)] {
        conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", [key, value])?;
    }
    Ok(())
}

/// A project's files, versions, runs and what its tools make, from its rows: they're no part of its Automerge document.
fn read_lock(conn: &Connection, at: &Path) -> Result<Lock, Error> {
    let e = |error| sql(at, error);
    fn decode<T: serde::de::DeserializeOwned>(at: &Path, record: String) -> Result<T, Error> {
        serde_json::from_str(&record).map_err(|error| err(at, error))
    }
    let mut lock = Lock::default();
    let mut rows = conn.prepare("SELECT key, record FROM lock_assets").map_err(e)?;
    for row in rows.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(e)? {
        let (key, record) = row.map_err(e)?;
        lock.assets.insert(key, decode(at, record)?);
    }
    let mut rows = conn.prepare("SELECT key, discarded, record FROM versions ORDER BY key, number").map_err(e)?;
    for row in rows.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, String>(2)?))).map_err(e)? {
        let (key, discarded, record) = row.map_err(e)?;
        let version: Version = decode(at, record)?;
        let Some(held) = lock.assets.get_mut(&key) else { continue };
        if discarded { held.discarded.push(version) } else { held.versions.push(version) }
    }
    let mut rows = conn.prepare("SELECT sha256, record FROM lock_files").map_err(e)?;
    for row in rows.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(e)? {
        let (sha256, record) = row.map_err(e)?;
        lock.files.insert(sha256, File { row: true, ..decode(at, record)? });
    }
    let mut rows = conn.prepare("SELECT record FROM lock_runs ORDER BY ord").map_err(e)?;
    for row in rows.query_map([], |row| row.get::<_, String>(0)).map_err(e)? {
        lock.runs.push(decode(at, row.map_err(e)?)?);
    }
    let mut rows = conn.prepare("SELECT kind, name, value FROM facts").map_err(e)?;
    for row in rows.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))).map_err(e)? {
        match row.map_err(e)? {
            (kind, name, value) if kind == "reserved" => drop(lock.reserved.insert(name, decode(at, value)?)),
            (kind, name, value) if kind == "makes" => drop(lock.makes.insert(name, value)),
            _ => {}
        }
    }
    Ok(lock)
}

fn require_current(conn: &Connection, path: &Path) -> Result<(), Error> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(|error| sql(path, error))?;
    if version != VERSION {
        return Err(Error::new(Kind::Validation, "project_db_version", format!("{} has schema {version}; only current CRDT schema {VERSION} is supported", path.display()))
            .remedy("Preserve this database. Create a new current-format project in a separate empty directory"));
    }
    let format: i64 = conn.query_row("SELECT format FROM crdt_document WHERE id = 1", [], |row| row.get(0)).map_err(|error| sql(path, error))?;
    if format != crdt::FORMAT {
        return Err(Error::new(Kind::Validation, "project_db_version", format!("{} has CRDT format {format}; only format {} is supported", path.display(), crdt::FORMAT)));
    }
    Ok(())
}

fn configure(conn: &Connection, path: &Path) -> Result<(), Error> {
    let e = |error| sql(path, error);
    conn.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
    conn.pragma_update(None, "synchronous", "FULL").map_err(e)?;
    // Checkpoints run beside writes, never in a commit.
    conn.pragma_update(None, "wal_autocheckpoint", 0).map_err(e)
}

/// Its identity, as its rows keep it beside its document; one from before schema 4 is the document's.
pub(crate) fn identity(conn: &Connection) -> Option<(String, String)> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).ok()?;
    if !(2..VERSION).contains(&version) {
        require_current(conn, Path::new(DB)).ok()?;
        // Never a project with no native document: its row stands beside what it keeps.
        document_at(conn, Path::new(DB)).ok()?;
        if let Some(kept) = meta(conn, "id").zip(meta(conn, "name")) {
            return Some(kept);
        }
    }
    let (bytes, heads): (Vec<u8>, String) = conn.query_row("SELECT snapshot, heads FROM crdt_document WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?))).ok()?;
    crdt::identity_bytes(&bytes, &heads)
}

/// Keeps the project's identity and optional public code in one transaction.
pub(crate) fn set_identity(conn: &mut Connection, at: &Path, id: &str, name: &str, code: Option<&str>) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| sql(at, e))?;
    identity_rows(&tx, id, name).map_err(|e| sql(at, e))?;
    if let Some(code) = code {
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('code', ?1)", [code]).map_err(|e| sql(at, e))?;
    }
    let kept = rows_kept(&tx, at)?;
    let mut doc = read_document(&tx, at)?;
    let old_heads = doc.doc.get_heads();
    doc.set_identity(id, name)?;
    let seq = meta(&tx, "seq").and_then(|value| value.parse().ok()).unwrap_or(0);
    save_document(&tx, at, &mut doc, seq, Some(&old_heads))?;
    // Its identity is no row of a box: rows that stood at the document stand at it still.
    if let Some(digest) = kept {
        keep_rows_at(&tx, at, digest)?;
    }
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
    let conn = connect(path, true).ok()?;
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

/// Canonical project state plus machine-local bookkeeping: its rows, when they stand at its document as it is and are as its last commit
/// left them (`rows_kept`); else the document, decoded, as when another writer set a document in its place or changed a row, or it holds
/// what its rows can't (two boxes of one name).
pub(crate) fn load(conn: &Connection, at: &Path) -> Result<Stored, Error> {
    let mut stored = match rows_kept(conn, at)? {
        Some(kept) => match read_rows(conn, at)? {
            (rows, read) if read == kept => rows,
            _ => {
                tracing::warn!("{}: its rows aren't as its last commit left them; it's read from its document, and its next write writes them again", at.display());
                from_document(read_document(conn, at)?)?
            }
        },
        None => from_document(read_document(conn, at)?)?,
    };
    stored.lock = Arc::new(read_lock(conn, at)?);
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

/// What a commit's rows say the document stood at as they were written (its heads, and the length of its stream), and their digest.
const ROWS_AT: &str = "rows_at";

#[derive(serde::Serialize, serde::Deserialize)]
struct RowsAt {
    heads: std::collections::BTreeSet<String>,
    bytes: i64,
    rows: String,
}

/// The rows a read takes, as one value: each row's hash, XORed, so a commit moves it by the rows it takes out and puts in, and a read
/// checks it for the cost of a hash of what it reads. Rows changed past Calcifer don't match it.
#[derive(Clone, Copy, Default, PartialEq)]
struct Digest([u8; 32]);

impl Digest {
    fn flip(&mut self, table: &str, row: &[Sql]) {
        let mut hash = Sha256::new();
        hash.update(table.as_bytes());
        for value in row {
            match value {
                Sql::Null => hash.update([0u8]),
                Sql::Integer(n) => {
                    hash.update([1u8]);
                    hash.update(n.to_le_bytes());
                }
                Sql::Real(x) => {
                    hash.update([2u8]);
                    hash.update(x.to_bits().to_le_bytes());
                }
                Sql::Text(text) => {
                    hash.update([3u8]);
                    hash.update((text.len() as u64).to_le_bytes());
                    hash.update(text.as_bytes());
                }
                Sql::Blob(bytes) => {
                    hash.update([4u8]);
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(bytes);
                }
            }
        }
        self.0.iter_mut().zip(hash.finalize()).for_each(|(at, by)| *at ^= by);
    }

    fn hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// Each row in a read's order and a commit's alike.
const HEAD: &str = "head";
const BOXES: &str = "boxes";
const FIELDS: &str = "fields";
const CANVAS: &str = "canvas";
const CONFLICTS: &str = "conflicts";

/// Rows taken out by `statement` (a `DELETE … RETURNING` of every column), each flipped out of `digest` as it stood.
fn gone(tx: &Transaction, digest: &mut Digest, table: &str, statement: &str, key: &[&dyn ToSql]) -> rusqlite::Result<()> {
    let mut statement = tx.prepare(statement)?;
    let columns = statement.column_count();
    let mut rows = statement.query(key)?;
    while let Some(row) = rows.next()? {
        digest.flip(table, &(0..columns).map(|at| row.get::<_, Sql>(at)).collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok(())
}

/// A row put in by `statement`, flipped into `digest`.
fn put(tx: &Transaction, digest: &mut Digest, table: &str, statement: &str, row: &[Sql]) -> rusqlite::Result<()> {
    tx.prepare(statement)?.execute(rusqlite::params_from_iter(row))?;
    digest.flip(table, row);
    Ok(())
}

/// The document as its row says it stands: its heads, and its stream's length (SQLite reads it from the row's header).
fn document_at(conn: &Connection, at: &Path) -> Result<(std::collections::BTreeSet<String>, i64), Error> {
    let (heads, bytes): (String, i64) = conn.query_row("SELECT heads, length(snapshot) FROM crdt_document WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|e| sql(at, e))?;
    let heads: Vec<String> = serde_json::from_str(&heads).map_err(|error| err(at, error))?;
    Ok((heads.into_iter().collect(), bytes))
}

/// The digest its rows were written with, if they were written at the document as it stands: a document set in its place by another
/// writer, cut short or put back to an older one, isn't what they say.
fn rows_kept(conn: &Connection, at: &Path) -> Result<Option<Digest>, Error> {
    let (heads, bytes) = document_at(conn, at)?;
    let kept = meta(conn, ROWS_AT).and_then(|kept| serde_json::from_str::<RowsAt>(&kept).ok()).filter(|kept| kept.heads == heads && kept.bytes == bytes);
    Ok(kept.and_then(|kept| hex::decode(kept.rows).ok()?.try_into().ok()).map(Digest))
}

/// The rows stand at the document as this transaction leaves it, with `digest`.
fn keep_rows_at(tx: &Transaction, at: &Path, digest: Digest) -> Result<(), Error> {
    let (heads, bytes) = document_at(tx, at)?;
    tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![ROWS_AT, json(&RowsAt { heads, bytes, rows: digest.hex() })]).map(drop).map_err(|e| sql(at, e))
}

/// Its rows stand at the document as this transaction leaves it, when they can say all of it: rows hold one box a name, so a document
/// with two (a merge of two replicas') is read from itself. Whether they stand.
fn rows_written(tx: &Transaction, at: &Path, written: &Written, digest: Digest) -> Result<bool, Error> {
    let mut names = std::collections::HashSet::new();
    if written.boxes.iter().all(|held| names.insert(name_key(&held.name))) {
        keep_rows_at(tx, at, digest)?;
        return Ok(true);
    }
    tx.execute("DELETE FROM meta WHERE key = ?1", [ROWS_AT]).map_err(|e| sql(at, e))?;
    Ok(false)
}

/// The project as its document says, decoded: what its rows say when they stand.
fn from_document(doc: crdt::Document) -> Result<Stored, Error> {
    let (heads, conflicts) = (doc.heads(), doc.conflicts());
    let mut stored = doc.into_stored()?;
    (stored.heads, stored.conflicts) = (Arc::new(heads), Arc::new(conflicts));
    Ok(stored)
}

/// Every row a read takes of what `stored` holds, written whole: the boxes with their ids, head, canvas and alternatives. Their digest.
fn all_rows(tx: &Transaction, at: &Path, stored: &Stored) -> Result<Digest, Error> {
    let e = |error| sql(at, error);
    let mut digest = Digest::default();
    tx.execute_batch("DELETE FROM head; DELETE FROM fields; DELETE FROM boxes; DELETE FROM canvas;").map_err(e)?;
    written_rows(tx, &Written::default(), &stored.written, &mut digest).map_err(e)?;
    for (key, row) in stored.canvas.iter() {
        put(tx, &mut digest, CANVAS, CANVAS_IN, &canvas_row(key, row)).map_err(e)?;
    }
    // Written whole: what the row said before was never in this digest.
    conflicts_row(tx, &mut digest, &stored.conflicts, false).map_err(e)?;
    Ok(digest)
}

const CANVAS_IN: &str = "INSERT INTO canvas (key, name, x, y, w, seq, by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

fn canvas_row(key: &str, row: &CardRow) -> [Sql; 7] {
    [Sql::Text(key.to_owned()), Sql::Text(row.name.clone()), Sql::Real(row.card.x), Sql::Real(row.card.y), row.card.w.map_or(Sql::Null, Sql::Real),
        Sql::Integer(row.seq as i64), Sql::Text(row.by.clone())]
}

/// The alternatives' row, written over what it was, which `counted` says `digest` holds.
fn conflicts_row(tx: &Transaction, digest: &mut Digest, conflicts: &[String], counted: bool) -> rusqlite::Result<()> {
    if let Some(was) = meta(tx, CONFLICTS).filter(|_| counted) {
        digest.flip(CONFLICTS, &[Sql::Text(was)]);
    }
    let now = json(&conflicts);
    tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", [CONFLICTS, now.as_str()])?;
    digest.flip(CONFLICTS, &[Sql::Text(now)]);
    Ok(())
}

/// The boxes, head, canvas, heads and alternatives the document projects to, from the rows each commit writes with it, and their digest.
fn read_rows(conn: &Connection, at: &Path) -> Result<(Stored, Digest), Error> {
    let e = |error| sql(at, error);
    let mut digest = Digest::default();
    let text = |value: &Sql| match value {
        Sql::Text(text) => Ok(text.clone()),
        other => Err(err(at, format!("a row holds {other:?} where it holds text"))),
    };
    let mut read = |table: &str, query: &str| -> Result<Vec<Vec<Sql>>, Error> {
        let mut statement = conn.prepare(query).map_err(e)?;
        let columns = statement.column_count();
        let mut rows = statement.query([]).map_err(e)?;
        let mut found = Vec::new();
        while let Some(row) = rows.next().map_err(e)? {
            let values = (0..columns).map(|at| row.get::<_, Sql>(at)).collect::<rusqlite::Result<Vec<_>>>().map_err(e)?;
            digest.flip(table, &values);
            found.push(values);
        }
        Ok(found)
    };
    let value = |sql: &Sql| serde_json::from_str::<serde_json::Value>(&text(sql)?).map_err(|error| err(at, error));
    let mut head = Vec::new();
    for row in read(HEAD, "SELECT key, ord, value FROM head ORDER BY ord")? {
        head.push((text(&row[0])?, value(&row[2])?));
    }
    let mut fields: HashMap<String, Vec<(String, serde_json::Value)>> = HashMap::new();
    for row in read(FIELDS, "SELECT box, ord, field, value FROM fields ORDER BY box, ord")? {
        fields.entry(text(&row[0])?).or_default().push((text(&row[2])?, value(&row[3])?));
    }
    let mut boxes = Vec::new();
    for row in read(BOXES, "SELECT key, name, ord, id FROM boxes ORDER BY ord")? {
        boxes.push(Boxed { id: text(&row[3])?, name: text(&row[1])?, fields: fields.remove(&text(&row[0])?).unwrap_or_default(), at: None });
    }
    let mut written = Written { head, boxes };
    // As the document reads them back: each mapping in key order.
    written.in_stored_order();
    let mut canvas = BTreeMap::new();
    let number = |value: &Sql| match value {
        Sql::Real(x) => Ok(*x),
        other => Err(err(at, format!("a canvas row holds {other:?} where it holds a number"))),
    };
    for row in read(CANVAS, "SELECT key, name, x, y, w, seq, by FROM canvas")? {
        let w = match &row[4] { Sql::Null => None, w => Some(number(w)?) };
        let seq = match row[5] { Sql::Integer(seq) => seq as u64, _ => 0 };
        let card = api::calcifer::project::Card { x: number(&row[2])?, y: number(&row[3])?, w };
        canvas.insert(text(&row[0])?, CardRow { name: text(&row[1])?, card, seq, by: text(&row[6])? });
    }
    let kept = meta(conn, CONFLICTS);
    if let Some(kept) = &kept {
        digest.flip(CONFLICTS, &[Sql::Text(kept.clone())]);
    }
    let conflicts: Vec<String> = kept.map(|text| serde_json::from_str(&text).map_err(|error| err(at, error))).transpose()?.unwrap_or_default();
    // In the order the document gives them: a change hash orders as its hex.
    let heads: Vec<String> = document_at(conn, at)?.0.into_iter().collect();
    let stored = Stored { written: Arc::new(written), canvas: Arc::new(canvas), heads: Arc::new(heads), conflicts: Arc::new(conflicts), rows_held: true, ..Default::default() };
    Ok((stored, digest))
}

/// The canonical document, decoded: what a write, a merge or history needs, and what `integrity` holds the rows to.
pub(super) fn document(conn: &Connection, at: &Path) -> Result<crdt::Document, Error> {
    read_document(conn, at)
}

/// Where the rows a read takes differ from the document they were written with: none, always (each commit writes both). A difference
/// is a fault, said in full; nothing reads around it.
pub(crate) fn integrity(conn: &Connection, at: &Path) -> Result<Vec<String>, Error> {
    let tx = conn.unchecked_transaction().map_err(|e| sql(at, e))?;
    let digest = rows_kept(&tx, at)?;
    let (rows, read) = read_rows(&tx, at)?;
    let doc = read_document(&tx, at)?;
    let (heads, identity) = (doc.heads(), doc.identity());
    let mut projected = from_document(doc)?;
    Arc::make_mut(&mut projected.written).in_stored_order();
    let mut found = Vec::new();
    let kept = meta(&tx, "id").zip(meta(&tx, "name"));
    if kept != identity {
        found.push(format!("identity: rows {kept:?} | document {identity:?}"));
    }
    match digest {
        None => {
            let mut names = std::collections::HashSet::new();
            if projected.written.boxes.iter().all(|held| names.insert(name_key(&held.name))) {
                found.push(format!("rows written at {:?}, the document at {:?}", meta(&tx, ROWS_AT), document_at(&tx, at).map(|now| json(&now)).ok()));
                found.extend(rows_differ(&rows, &projected));
            }
            // A read takes the document: the rows aren't read.
            return Ok(found);
        }
        // A read takes the document, and the rows are said as they differ.
        Some(kept) if kept != read => found.push(format!("rows changed past Calcifer: their digest is {}, their last commit's {}", read.hex(), kept.hex())),
        Some(_) => {}
    }
    found.extend(rows_differ(&rows, &projected));
    let set = |list: &[String]| list.iter().cloned().collect::<std::collections::BTreeSet<_>>();
    if set(&rows.heads) != set(&heads) {
        found.push(format!("heads: rows {:?} | document {:?}", rows.heads, heads));
    }
    Ok(found)
}

/// Where `rows` (as a read takes them) and `document` (as it projects, in stored order) say otherwise: boxes, head, canvas, alternatives.
fn rows_differ(rows: &Stored, document: &Stored) -> Vec<String> {
    let mut found = Vec::new();
    if serde_json::to_string(&rows.written.head).ok() != serde_json::to_string(&document.written.head).ok() {
        found.push(format!("head: rows {} | document {}", json(&rows.written.head), json(&document.written.head)));
    }
    let boxes = |stored: &Stored| stored.written.boxes.iter().map(|held| (held.id.clone(), held.name.clone(), json(&held.fields))).collect::<Vec<_>>();
    let (row_boxes, doc_boxes) = (boxes(rows), boxes(document));
    if row_boxes != doc_boxes {
        let first = row_boxes.iter().zip(&doc_boxes).position(|(a, b)| a != b).unwrap_or(row_boxes.len().min(doc_boxes.len()));
        found.push(format!("boxes: {} rows, {} in the document; first differing at {first}: rows {:?} | document {:?}", row_boxes.len(), doc_boxes.len(), row_boxes.get(first), doc_boxes.get(first)));
    }
    let cards = |canvas: &BTreeMap<String, CardRow>| canvas.iter().map(|(key, row)| (key.clone(), row.name.clone(), json(&row.card), row.seq, row.by.clone())).collect::<Vec<_>>();
    if cards(&rows.canvas) != cards(&document.canvas) {
        found.push(format!("canvas: rows {:?} | document {:?}", cards(&rows.canvas), cards(&document.canvas)));
    }
    let set = |list: &[String]| list.iter().cloned().collect::<std::collections::BTreeSet<_>>();
    if set(&rows.conflicts) != set(&document.conflicts) {
        found.push(format!("conflicts: rows {:?} | document {:?}", rows.conflicts, document.conflicts));
    }
    found
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

/// The initial current-format schema and native baseline commit atomically for a newly reserved file.
fn initialize_current(conn: &mut Connection, at: &Path) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|error| sql(at, error))?;
    tx.execute_batch(SCHEMA).map_err(|error| sql(at, error))?;
    tx.execute_batch(CRDT_SCHEMA).map_err(|error| sql(at, error))?;
    tx.execute("INSERT INTO meta VALUES ('seq', '0')", []).map_err(|error| sql(at, error))?;
    let mut doc = crdt::Document::new(at);
    doc.initialize(crdt::value(&Stored::default(), &[], &serde_json::Value::Null, None))?;
    save_document(&tx, at, &mut doc, 0, None)?;
    if let Some((id, name)) = doc.identity() {
        identity_rows(&tx, &id, &name).map_err(|error| sql(at, error))?;
    }
    rows_written(&tx, at, &Written::default(), Digest::default())?;
    tx.pragma_update(None, "user_version", VERSION).map_err(|error| sql(at, error))?;
    tx.commit().map_err(|error| sql(at, error))
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
    // What the rows a read takes moved by.
    let mut moved = Digest::default();
    write(&tx, before, after, extra, &mut moved).map_err(e)?;
    if tx.total_changes() == was && extra.answer.is_none() {
        tx.rollback().map_err(e)?;
        return Ok(None);
    }
    let seq = before.seq + 1;
    // Its files, runs, walk and what's measured are rows alone: a change only to them leaves the document as it was, undecoded if it was.
    let owners = |lock: &Lock| lock.assets.iter().map(|(key, held)| (key.clone(), held.id.clone())).collect::<Vec<_>>();
    let documented = !Arc::ptr_eq(&before.written, &after.written) || !Arc::ptr_eq(&before.canvas, &after.canvas)
        || !extra.ops.is_empty() || !extra.ops_gone.is_empty()
        || (!Arc::ptr_eq(&before.lock, &after.lock) && owners(&before.lock) != owners(&after.lock));
    if !documented {
        (after.crdt, after.heads, after.conflicts, after.rows_held) = (before.crdt.clone(), before.heads.clone(), before.conflicts.clone(), before.rows_held);
        answered(&tx, extra, &after.heads).map_err(e)?;
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('seq', ?1)", [seq.to_string()]).map_err(e)?;
        before_commit();
        tx.commit().map_err(e)?;
        return Ok(Some(seq));
    }
    let mut doc = match &before.crdt {
        Some(doc) => (**doc).clone(),
        None => {
            // Read from its rows, its document is decoded now, as it stands in this transaction: at the heads `before` was read at, or
            // another commit came between and this one's view is stale.
            let doc = read_document(&tx, at)?;
            let stands: std::collections::BTreeSet<String> = doc.heads().into_iter().collect();
            if !before.heads.is_empty() && stands != before.heads.iter().cloned().collect::<std::collections::BTreeSet<_>>() {
                return Err(Error::new(Kind::Conflict, "project_changed", "the project changed while this edit was being committed; read it again and retry"));
            }
            doc
        }
    };
    let old_heads = doc.doc.get_heads();
    // Rows that weren't what `before` holds (its read decoded the document) are written whole with it, not by what changed.
    let kept = rows_kept(&tx, at)?.filter(|_| before.rows_held);
    let mut ops = doc.ops()?;
    ops.retain(|row| !extra.ops_gone.iter().any(|(uid, _)| uid == &row.uid) && !extra.ops.iter().any(|next| next.uid == row.uid));
    ops.extend(extra.ops.iter().cloned());
    doc.update_stored(before, after, &ops)?;
    if doc.doc.get_heads() != old_heads {
        // A fresh staged store's first populated document is its full baseline, not
        // a large change tail on the empty initializer. Normal edits stay incremental.
        let base = (before.crdt.is_some() || !before.heads.is_empty()).then_some(old_heads.as_slice());
        save_document(&tx, at, &mut doc, seq, base)?;
    }
    // What a read takes besides the rows `write` wrote: the heads they stand at, and the alternatives the document keeps.
    after.heads = Arc::new(doc.heads());
    after.conflicts = Arc::new(doc.conflicts());
    answered(&tx, extra, &after.heads).map_err(e)?;
    let digest = match kept {
        Some(mut digest) => {
            if after.conflicts != before.conflicts {
                conflicts_row(&tx, &mut digest, &after.conflicts, true).map_err(e)?;
            }
            digest.0.iter_mut().zip(moved.0).for_each(|(at, by)| *at ^= by);
            digest
        }
        None => all_rows(&tx, at, after)?,
    };
    after.rows_held = rows_written(&tx, at, &after.written, digest)?;
    after.crdt = Some(Arc::new(doc));
    tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('seq', ?1)", [seq.to_string()]).map_err(e)?;
    before_commit();
    tx.commit().map_err(e)?;
    Ok(Some(seq))
}

fn write(tx: &Transaction, before: &Stored, after: &Stored, extra: &Extra, digest: &mut Digest) -> rusqlite::Result<()> {
    let seq = before.seq + 1;
    if !Arc::ptr_eq(&before.written, &after.written) {
        written_rows(tx, &before.written, &after.written, digest)?;
    }
    if !Arc::ptr_eq(&before.canvas, &after.canvas) {
        const OUT: &str = "DELETE FROM canvas WHERE key = ?1 RETURNING key, name, x, y, w, seq, by";
        for key in before.canvas.keys().filter(|key| !after.canvas.contains_key(*key)) {
            gone(tx, digest, CANVAS, OUT, params![key])?;
        }
        for (key, row) in after.canvas.iter().filter(|(key, row)| before.canvas.get(*key) != Some(row)) {
            gone(tx, digest, CANVAS, OUT, params![key])?;
            put(tx, digest, CANVAS, CANVAS_IN, &canvas_row(key, row))?;
        }
    }
    if !Arc::ptr_eq(&before.lock, &after.lock) {
        lock_written(tx, &before.lock, &after.lock)?;
    }
    if !Arc::ptr_eq(&before.walk, &after.walk) {
        let walk = json(&*after.walk);
        if json(&*before.walk) != walk {
            match after.walk.is_empty() && after.walk.bound.is_none() {
                true => drop(tx.execute("DELETE FROM walk", [])?),
                false => drop(tx.execute("INSERT OR REPLACE INTO walk (id, record) VALUES (1, ?1)", [walk])?),
            }
        }
    }
    if !Arc::ptr_eq(&before.measured, &after.measured) {
        for sha256 in before.measured.keys().filter(|sha256| !after.measured.contains_key(*sha256)) {
            tx.execute("DELETE FROM measured WHERE sha256 = ?1", [sha256])?;
        }
        for (sha256, held) in after.measured.iter().filter(|(sha256, held)| before.measured.get(*sha256) != Some(held)) {
            tx.execute("INSERT OR REPLACE INTO measured (sha256, record) VALUES (?1, ?2)", params![sha256, json(held)])?;
        }
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

/// The answer its act said, on its key's row, made at `heads`: where the commit leaves the document.
fn answered(tx: &Transaction, extra: &Extra, heads: &[String]) -> rusqlite::Result<()> {
    let Some((key, said)) = &extra.answer else { return Ok(()) };
    tx.execute("UPDATE request_keys SET status = ?2, content_type = 'application/json', body = ?3 WHERE key = ?1", params![key, said.status as i64, (said.body)(heads)]).map(drop)
}

/// Project the selected box for each normalized name, with each side's own ordinal.
fn written_rows(tx: &Transaction, before: &Written, after: &Written, digest: &mut Digest) -> rusqlite::Result<()> {
    if before.head != after.head {
        gone(tx, digest, HEAD, "DELETE FROM head RETURNING key, ord, value", params![])?;
        for (ord, (key, value)) in after.head.iter().enumerate() {
            put(tx, digest, HEAD, "INSERT INTO head (key, ord, value) VALUES (?1, ?2, ?3)", &[Sql::Text(key.clone()), Sql::Integer(ord as i64), Sql::Text(json(value))])?;
        }
    }
    const FIELDS_OUT: &str = "DELETE FROM fields WHERE box = ?1 RETURNING box, ord, field, value";
    const BOX_OUT: &str = "DELETE FROM boxes WHERE key = ?1 RETURNING key, name, ord, id";
    const BOX_IN: &str = "INSERT INTO boxes (key, name, ord, id) VALUES (?1, ?2, ?3, ?4)";
    fn boxed(key: &str, held: &Boxed, ord: usize) -> [Sql; 4] {
        [Sql::Text(key.to_owned()), Sql::Text(held.name.clone()), Sql::Integer(ord as i64), Sql::Text(held.id.clone())]
    }
    let common = before.boxes.iter().zip(&after.boxes)
        .take_while(|(old, new)| old.id == new.id && old.name == new.name)
        .count();
    let mut rows = BTreeMap::new();
    for (ord, (old, new)) in before.boxes.iter().zip(&after.boxes).take(common).enumerate() {
        rows.insert(name_key(&old.name), (Some((ord, old)), Some((ord, new))));
    }
    // Each side retains its last occurrence independently, including when a suffix
    // collides with a shared-prefix name after normalization.
    for (ord, held) in before.boxes.iter().enumerate().skip(common) {
        rows.entry(name_key(&held.name)).or_default().0 = Some((ord, held));
    }
    for (ord, held) in after.boxes.iter().enumerate().skip(common) {
        rows.entry(name_key(&held.name)).or_default().1 = Some((ord, held));
    }
    for (key, (old, new)) in &rows {
        if old.is_some() && new.is_none() {
            gone(tx, digest, FIELDS, FIELDS_OUT, params![key])?;
            gone(tx, digest, BOXES, BOX_OUT, params![key])?;
        }
    }
    for (key, (old, new)) in &rows {
        let Some((ord, held)) = new else { continue };
        match old {
            Some((was_ord, was_held)) if was_held == held && was_held.id == held.id => {
                if was_ord != ord {
                    gone(tx, digest, BOXES, BOX_OUT, params![key])?;
                    put(tx, digest, BOXES, BOX_IN, &boxed(key, held, *ord))?;
                }
            }
            _ => {
                gone(tx, digest, BOXES, BOX_OUT, params![key])?;
                put(tx, digest, BOXES, BOX_IN, &boxed(key, held, *ord))?;
                gone(tx, digest, FIELDS, FIELDS_OUT, params![key])?;
                for (n, (field, value)) in held.fields.iter().enumerate() {
                    put(tx, digest, FIELDS, "INSERT INTO fields (box, ord, field, value) VALUES (?1, ?2, ?3, ?4)", &[Sql::Text(key.clone()), Sql::Integer(n as i64), Sql::Text(field.clone()), Sql::Text(json(value))])?;
                }
            }
        }
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
        tx.execute("DELETE FROM lock_runs", [])?;
        for (ord, row) in after.runs.iter().enumerate() {
            tx.execute("INSERT INTO run_files (ord, run, function, output, sha256) VALUES (?1, ?2, ?3, ?4, ?5)", params![ord as i64, row.run as i64, row.function, row.output, row.sha256])?;
            tx.execute("INSERT INTO lock_runs (ord, record) VALUES (?1, ?2)", params![ord as i64, json(row)])?;
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

/// Exclusively create a fresh current database. Existing paths, including incomplete stages, are preserved.
pub(crate) fn create(path: &Path, stored: &Stored, ops: &[OpRow], asked: &[(String, String)]) -> Result<(), Error> {
    OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error::new(Kind::Conflict, "project_db_exists", format!("{} already exists; preserve it and choose a fresh database path", path.display()))
        } else { err(path, error) }
    })?;
    let mut conn = connect(path, false)?;
    configure(&conn, path)?;
    initialize_current(&mut conn, path)?;
    let mut fresh = stored.clone();
    crdt::identify(&Stored::default(), &mut fresh);
    let mut ops = ops.to_vec();
    crdt::identify_history(&fresh, &mut ops);
    let extra = Extra { asked: asked.iter().map(|(digest, text)| (digest.clone(), Some(text.clone()))).collect(), ops, ..Default::default() };
    fresh.seq = 0;
    commit(&mut conn, path, &Stored::default(), &mut fresh, &extra, || (), || ())?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").map_err(|e| err(path, e))?;
    drop(conn);
    Ok(())
}

/// What the WAL holds moved into the database, from a connection of its own, as far as readers let it: writes go on meanwhile.
pub(crate) fn checkpoint(path: &Path) {
    let done = open(path).and_then(|conn| conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())).map_err(|error| sql(path, error)));
    if let Err(error) = done {
        tracing::warn!("{}: checkpoint: {error}", path.display());
    }
}
