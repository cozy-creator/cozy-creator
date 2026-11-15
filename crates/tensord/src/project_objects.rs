//! Object bookkeeping (cl-984): the objects a signer wrote, their uses, each run's objects and its held memos. The
//! machine journal keeps an unscoped run's; a project's own are in `objects.sqlite3` in its machine area, opened only
//! under its verified scope, so no digest of a project's inputs, sources or outputs is the machine's.
use api::machine::LocalPrivateStorage;
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::HashSet, io};
use tensorfs::format::ObjectRef;

use crate::journal::HeldMemo;

fn failed(error: rusqlite::Error) -> io::Error {
    io::Error::other(format!("object bookkeeping: {error}"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

/// The project's bookkeeping. A rollback journal, not WAL: an encrypted view may not map a shared-memory file.
pub(crate) fn open(scope: &LocalPrivateStorage) -> io::Result<Connection> {
    let connection = Connection::open(crate::private_io::root(scope)?.join("objects.sqlite3")).map_err(failed)?;
    connection.busy_timeout(std::time::Duration::from_secs(30)).map_err(failed)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE;
        CREATE TABLE IF NOT EXISTS objects(actor TEXT NOT NULL,sha256 TEXT NOT NULL,length INTEGER NOT NULL,PRIMARY KEY(actor,sha256));
        CREATE TABLE IF NOT EXISTS object_uses(sha256 TEXT PRIMARY KEY,length INTEGER NOT NULL,used_ms INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS run_objects(execution INTEGER NOT NULL,sha256 TEXT NOT NULL,length INTEGER NOT NULL,PRIMARY KEY(execution,sha256));
        CREATE INDEX IF NOT EXISTS run_objects_digest ON run_objects(sha256);
        CREATE TABLE IF NOT EXISTS held_memos(actor TEXT NOT NULL,computation TEXT NOT NULL,record BLOB NOT NULL,used_ms INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(actor,computation));
        CREATE TABLE IF NOT EXISTS held_memo_objects(actor TEXT NOT NULL,computation TEXT NOT NULL,sha256 TEXT NOT NULL,PRIMARY KEY(actor,computation,sha256));
        CREATE INDEX IF NOT EXISTS held_memo_objects_digest ON held_memo_objects(sha256);").map_err(failed)?;
    Ok(connection)
}

/// The length of `sha256` as `actor` wrote it, if it did.
pub(crate) fn object(conn: &Connection, actor: &str, sha256: &str) -> io::Result<Option<u64>> {
    conn.query_row("SELECT length FROM objects WHERE actor=?1 AND sha256=?2", params![actor, sha256], |row| row.get(0)).optional().map_err(failed)
}

/// `actor` wrote `object`.
pub(crate) fn bind(conn: &Connection, actor: &str, object: &ObjectRef) -> io::Result<()> {
    conn.execute("INSERT OR REPLACE INTO objects(actor,sha256,length) VALUES(?1,?2,?3)", params![actor, object.sha256, object.length]).map_err(failed)?;
    Ok(())
}

pub(crate) fn touch(conn: &Connection, object: &ObjectRef) -> io::Result<()> {
    crate::storage::check_digest("run object", &object.sha256, false)?;
    conn.execute("INSERT INTO object_uses(sha256,length,used_ms) VALUES(?1,?2,?3) ON CONFLICT(sha256) DO UPDATE SET used_ms=excluded.used_ms",
        params![object.sha256, object.length, now_ms()]).map_err(failed)?;
    Ok(())
}

/// Execution `execution` (its machine journal id) names `objects`.
pub(crate) fn reference(conn: &Connection, execution: &str, objects: &[ObjectRef]) -> io::Result<()> {
    for object in objects {
        touch(conn, object)?;
        conn.execute("INSERT OR IGNORE INTO run_objects(execution,sha256,length) VALUES(?1,?2,?3)", params![execution, object.sha256, object.length]).map_err(failed)?;
    }
    Ok(())
}

/// Holds `held`, each signer keeping its `keep` most recently used.
pub(crate) fn hold(conn: &Connection, held: &HeldMemo, keep: i64) -> io::Result<()> {
    let HeldMemo { actor, computation, record, objects } = held;
    conn.execute("INSERT OR REPLACE INTO held_memos(actor,computation,record,used_ms) VALUES(?1,?2,?3,?4)", params![actor, computation, record, now_ms()]).map_err(failed)?;
    conn.execute("DELETE FROM held_memos WHERE actor=?1 AND computation NOT IN (SELECT computation FROM held_memos WHERE actor=?1 ORDER BY used_ms DESC LIMIT ?2)",
        params![actor, keep]).map_err(failed)?;
    conn.execute("DELETE FROM held_memo_objects WHERE actor=?1 AND computation NOT IN (SELECT computation FROM held_memos WHERE actor=?1)", params![actor]).map_err(failed)?;
    conn.execute("DELETE FROM held_memo_objects WHERE actor=?1 AND computation=?2", params![actor, computation]).map_err(failed)?;
    for sha256 in objects {
        conn.execute("INSERT OR IGNORE INTO held_memo_objects(actor,computation,sha256) VALUES(?1,?2,?3)", params![actor, computation, sha256]).map_err(failed)?;
    }
    Ok(())
}

/// The record held for `actor`'s `computation`, now used.
pub(crate) fn held(conn: &Connection, actor: &str, computation: &str) -> io::Result<Option<Vec<u8>>> {
    let record: Option<Vec<u8>> = conn.query_row("SELECT record FROM held_memos WHERE actor=?1 AND computation=?2", params![actor, computation], |row| row.get(0))
        .optional().map_err(failed)?;
    if record.is_some() {
        conn.execute("UPDATE held_memos SET used_ms=?3 WHERE actor=?1 AND computation=?2", params![actor, computation, now_ms()]).map_err(failed)?;
    }
    Ok(record)
}

/// Every held memo naming `sha256` goes; how many.
pub(crate) fn drop_naming(conn: &Connection, sha256: &str) -> io::Result<usize> {
    let dropped = conn.execute("DELETE FROM held_memos WHERE (actor,computation) IN (SELECT actor,computation FROM held_memo_objects WHERE sha256=?1)", [sha256])
        .map_err(failed)?;
    conn.execute("DELETE FROM held_memo_objects WHERE (actor,computation) NOT IN (SELECT actor,computation FROM held_memos)", []).map_err(failed)?;
    Ok(dropped)
}

/// What the project needs now: objects used since `since_ms`, and those its `live` runs name.
pub(crate) fn needed(conn: &Connection, since_ms: i64, live: &HashSet<i64>) -> io::Result<Vec<ObjectRef>> {
    let mut needed: Vec<ObjectRef> = {
        let mut statement = conn.prepare("SELECT sha256,length FROM object_uses WHERE used_ms>=?1").map_err(failed)?;
        let rows = statement.query_map([since_ms], |row| Ok(ObjectRef { sha256: row.get(0)?, length: row.get(1)? })).map_err(failed)?;
        rows.collect::<Result<_, _>>().map_err(failed)?
    };
    let mut statement = conn.prepare("SELECT execution,sha256,length FROM run_objects").map_err(failed)?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, ObjectRef { sha256: row.get(1)?, length: row.get(2)? }))).map_err(failed)?;
    for row in rows {
        let (execution, object) = row.map_err(failed)?;
        if live.contains(&execution) && !needed.contains(&object) {
            needed.push(object);
        }
    }
    Ok(needed)
}

/// Whether nothing of the project needs `sha256`: no use since `since_ms` and none of its `live` runs names it.
pub(crate) fn releasable(conn: &Connection, sha256: &str, since_ms: i64, live: &HashSet<i64>) -> io::Result<bool> {
    let used: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM object_uses WHERE sha256=?1 AND used_ms>=?2)", params![sha256, since_ms], |row| row.get(0))
        .map_err(failed)?;
    if used {
        return Ok(false);
    }
    let mut statement = conn.prepare("SELECT execution FROM run_objects WHERE sha256=?1").map_err(failed)?;
    let rows = statement.query_map([sha256], |row| row.get::<_, i64>(0)).map_err(failed)?;
    for row in rows {
        if live.contains(&row.map_err(failed)?) {
            return Ok(false);
        }
    }
    Ok(true)
}
