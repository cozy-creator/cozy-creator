//! `project.db` (cl-803, `docs/PROJECT-DB.md`): one SQLite file in a project's folder, WAL, written by this Calcifer alone. `load` reads
//! all of it into what `Folder` holds; `commit` writes what one transaction changed of it, row by row, with the history and the keys
//! that go with it.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
};

use protocol::error::{Error, Kind};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{
    folder::{Asset, File, Lock, Measured, RunFile, Version, WalkFile},
    graph::name_key,
    written::{Boxed, Written},
};

pub const DB: &str = "project.db";
/// This schema's `user_version`. A database of another isn't read.
const VERSION: i64 = 1;
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
/// How long a request's answer is kept for its retries.
const KEY_KEPT_MS: i64 = 24 * 60 * 60 * 1000;

/// A box's row on the canvas, and the transaction and client that last wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct CardRow {
    pub name: String,
    pub card: protocol::project::Card,
    pub seq: u64,
    pub by: String,
}

/// Everything `project.db` holds but its history, keys and asked texts: what `Folder` keeps in memory. Each part is shared, so a
/// transaction copies only what it writes.
#[derive(Clone, Debug, Default)]
pub struct Stored {
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
#[derive(Clone, Debug)]
pub struct OpRow {
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
pub struct Extra {
    pub asked: Vec<(String, Option<String>)>,
    /// Every asked text gone.
    pub asked_cleared: bool,
    /// History rows written whole, or by id gone.
    pub ops: Vec<OpRow>,
    pub ops_gone: Vec<u64>,
    /// A request's key, with the digest of its ask: done, its answer kept after.
    pub key: Option<(String, String)>,
}

/// A kept request key: its ask's digest and, once kept, its answer.
pub struct Keyed {
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
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            let name = at.file_name().map_or_else(|| at.display().to_string(), |name| name.to_string_lossy().into_owned());
            Error::new(Kind::Unavailable, "file_held", format!("{name} is open in another program")).remedy("close it, and save again")
        }
        _ => err(at, e),
    }
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("a project row serializes")
}

/// `project.db` in `dir` opened, made with this schema if it's new.
pub fn open(path: &Path) -> Result<Connection, Error> {
    let e = |error: rusqlite::Error| sql(path, error);
    let conn = Connection::open(path).map_err(e)?;
    conn.busy_timeout(std::time::Duration::from_secs(10)).map_err(e)?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
    conn.pragma_update(None, "synchronous", "NORMAL").map_err(e)?;
    // Checkpoints run beside the writes (`checkpoint`), never in a commit: on a busy disk one's fsync takes a second.
    conn.pragma_update(None, "wal_autocheckpoint", 0).map_err(e)?;
    match conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).map_err(e)? {
        0 => {
            conn.execute_batch(&format!("BEGIN IMMEDIATE; {SCHEMA}; PRAGMA user_version = {VERSION}; INSERT INTO meta VALUES ('seq', '0'); COMMIT;")).map_err(e)?;
        }
        VERSION => {}
        other => return Err(Error::new(Kind::Validation, "project_db_version", format!("{} is of schema {other}, and this cozy reads {VERSION}", path.display())).next("cozy update")),
    }
    Ok(conn)
}

/// What another connection's commit changes: `Folder` reads again when this moves.
pub fn data_version(conn: &Connection) -> i64 {
    conn.pragma_query_value(None, "data_version", |r| r.get(0)).unwrap_or(0)
}

/// All of it but the history, keys and asked texts.
pub fn load(conn: &Connection, at: &Path) -> Result<Stored, Error> {
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
        boxes.insert(key, (ord, Boxed { name, fields: vec![], at: None }));
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
        let card = protocol::project::Card { x: r.get(2)?, y: r.get(3)?, w: r.get(4)? };
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
        lock.files.insert(sha256, serde_json::from_str(&record).map_err(|why| bad("lock_files", why))?);
    }
    let mut rows = conn.prepare("SELECT run, function, output, sha256 FROM run_files ORDER BY ord").map_err(e)?;
    for row in rows.query_map([], |r| Ok(RunFile { run: r.get::<_, i64>(0)? as u64, function: r.get(1)?, output: r.get(2)?, sha256: r.get(3)? })).map_err(e)? {
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
    Ok(Stored { seq, written: Arc::new(written), canvas: Arc::new(canvas), lock: Arc::new(lock), walk: Arc::new(walk), measured: Arc::new(measured) })
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
pub fn commit(conn: &mut Connection, at: &Path, before: &Stored, after: &Stored, extra: &Extra) -> Result<Option<u64>, Error> {
    let e = |error: rusqlite::Error| sql(at, error);
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
    let was = tx.total_changes();
    write(&tx, before, after, extra).map_err(e)?;
    if tx.total_changes() == was {
        tx.rollback().map_err(e)?;
        return Ok(None);
    }
    let seq = before.seq + 1;
    tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('seq', ?1)", [seq.to_string()]).map_err(e)?;
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
    for id in &extra.ops_gone {
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
    for sha256 in before.files.keys().filter(|sha256| !after.files.contains_key(*sha256)) {
        tx.execute("DELETE FROM lock_files WHERE sha256 = ?1", [sha256])?;
    }
    for (sha256, file) in &after.files {
        let record = file_record(file);
        if before.files.get(sha256).is_none_or(|was| file_record(was) != record) {
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
pub fn asked(conn: &Connection, digest: &str) -> Option<String> {
    conn.query_row("SELECT text FROM asked WHERE digest = ?1", [digest], |r| r.get(0)).optional().ok().flatten()
}

/// Every history row, oldest first.
pub fn ops(conn: &Connection, at: &Path) -> Result<Vec<OpRow>, Error> {
    let e = |error: rusqlite::Error| sql(at, error);
    let mut rows = conn.prepare("SELECT id, at_ms, by, words, edits, changes, undone FROM ops ORDER BY id").map_err(e)?;
    let found = rows
        .query_map([], |r| Ok(OpRow { id: r.get::<_, i64>(0)? as u64, at_ms: r.get(1)?, by: r.get(2)?, words: r.get(3)?, edits: r.get(4)?, changes: r.get(5)?, undone: r.get(6)? }))
        .map_err(e)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(e)?;
    Ok(found)
}

/// What the request under `key` asked and answered, if it was kept.
pub fn keyed(conn: &Connection, key: &str) -> Option<Keyed> {
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
pub fn keep_answer(conn: &Connection, key: &str, digest: &str, status: u16, content_type: &str, body: &[u8]) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO request_keys (key, digest, status, content_type, body, at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (key) DO UPDATE SET status = excluded.status, content_type = excluded.content_type, body = excluded.body",
        params![key, digest, status as i64, content_type, body, super::now_ms()],
    )
    .map(drop)
}

/// A fresh database at `path` holding `stored`, its history and its asked texts: what a first open imports.
pub fn create(path: &Path, stored: &Stored, ops: &[OpRow], asked: &[(String, String)]) -> Result<(), Error> {
    let _ = std::fs::remove_file(path);
    let mut conn = open(path)?;
    let extra = Extra { asked: asked.iter().map(|(digest, text)| (digest.clone(), Some(text.clone()))).collect(), ops: ops.to_vec(), ..Default::default() };
    let mut fresh = stored.clone();
    fresh.seq = 0;
    commit(&mut conn, path, &Stored::default(), &fresh, &extra)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").map_err(|e| err(path, e))?;
    drop(conn);
    Ok(())
}

/// What the WAL holds moved into the database, from a connection of its own, as far as readers let it: writes go on meanwhile.
pub fn checkpoint(path: &Path) {
    let done = Connection::open(path).and_then(|conn| conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())));
    if let Err(error) = done {
        tracing::warn!("{}: checkpoint: {error}", path.display());
    }
}

/// A copy of the database as it stands, whole in one file: what an archive carries.
pub fn snapshot(conn: &Connection, to: &Path, at: &Path) -> Result<(), Error> {
    let _ = std::fs::remove_file(to);
    conn.execute("VACUUM INTO ?1", [to.to_string_lossy()]).map(drop).map_err(|e| sql(at, e))
}
