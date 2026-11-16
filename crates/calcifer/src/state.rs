//! Calcifer's own state beside its runs, as tables of `state.db` (cl-818): the project folders it opened, your tool
//! list, Render here, model header caches and independent asks. Project activity belongs to project.db. Each change is one
//! transaction; nothing is rewritten whole.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use api::calcifer::tool::ToolList;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest as _, Sha256};
use tensorfs::family::Facts;

use crate::store::{Result, Store, StoreError};

/// Made at open when absent, at the store's version: every statement names its columns.
pub(crate) const TABLES: &str = "
    CREATE TABLE IF NOT EXISTS projects (
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL,
        code TEXT NOT NULL DEFAULT '',
        used_at_ms INTEGER NOT NULL DEFAULT 0
    ) STRICT;
    CREATE TABLE IF NOT EXISTS project_keys (
        key TEXT PRIMARY KEY,
        id TEXT NOT NULL,
        current INTEGER NOT NULL DEFAULT 0
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS project_keys_current ON project_keys (id) WHERE current = 1;
    CREATE TABLE IF NOT EXISTS project_handles (
        handle TEXT PRIMARY KEY,
        path TEXT NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS projects_gone (
        id TEXT PRIMARY KEY,
        code TEXT NOT NULL,
        path TEXT NOT NULL
    ) STRICT;
    DROP TABLE IF EXISTS sessions;
    DROP TABLE IF EXISTS session_codes;
    CREATE TABLE IF NOT EXISTS purge_due (
        project TEXT PRIMARY KEY,
        at_ms INTEGER NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS steps_done (step TEXT PRIMARY KEY) STRICT;
    CREATE TABLE IF NOT EXISTS tool_list (
        ord INTEGER PRIMARY KEY,
        package TEXT NOT NULL UNIQUE
    ) STRICT;
    CREATE TABLE IF NOT EXISTS tools_off (
        ord INTEGER PRIMARY KEY,
        tool TEXT NOT NULL UNIQUE
    ) STRICT;
    CREATE TABLE IF NOT EXISTS headers (
        hub TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        families TEXT NOT NULL,
        lora INTEGER NOT NULL,
        family TEXT,
        PRIMARY KEY (hub, sha256)
    ) STRICT, WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS after_steps (
        key_sha256 TEXT PRIMARY KEY,
        step TEXT NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS machine_warm (
        machine TEXT NOT NULL,
        item TEXT NOT NULL,
        PRIMARY KEY (machine, item)
    ) STRICT, WITHOUT ROWID;
    -- Calcifer's jobs once released (cl-1028), by machine ('' for its own), job type and outcome: what keeps the newest of each.
    CREATE TABLE IF NOT EXISTS ended_jobs (
        machine TEXT NOT NULL,
        task TEXT NOT NULL,
        status TEXT NOT NULL,
        number INTEGER NOT NULL,
        run TEXT NOT NULL UNIQUE,
        ended_at INTEGER NOT NULL,
        PRIMARY KEY (machine, task, status, number)
    ) STRICT, WITHOUT ROWID;";

/// A project Calcifer opened, by its id: its name, and where it last was.
#[derive(Clone, Debug)]
pub(crate) struct KnownProject {
    pub id: String,
    pub code: String,
    /// What its runs are named by here (cl-987); empty until it opens.
    pub key: String,
    pub name: String,
    pub path: PathBuf,
    pub used_at_ms: i64,
}

fn known(row: &rusqlite::Row) -> rusqlite::Result<KnownProject> {
    let path = PathBuf::from(row.get::<_, String>(1)?);
    let name = Some(row.get::<_, String>(4)?).filter(|name| !name.is_empty());
    let name = name.unwrap_or_else(|| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    Ok(KnownProject { id: row.get(0)?, code: row.get(5)?, key: row.get::<_, Option<String>>(6)?.unwrap_or_default(), name, path, used_at_ms: row.get(2)? })
}

/// `known`'s columns, from `projects`.
const KNOWN: &str = "SELECT id, path, used_at_ms, 0, '', code, (SELECT key FROM project_keys WHERE project_keys.id = projects.id AND current = 1) FROM projects";

/// Whether project `id` may take `key` here: no reserved key, and none another project this index knows has, now or before (cl-987).
/// A key of a project the index let go of is free.
fn key_free(conn: &Connection, id: &str, key: &str) -> rusqlite::Result<bool> {
    if api::calcifer::project::RESERVED_KEYS.contains(&key) { return Ok(false); }
    let taken: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM project_keys JOIN projects ON projects.id = project_keys.id
        WHERE key = ?1 AND project_keys.id <> ?2)", [key, id], |row| row.get(0))?;
    Ok(!taken)
}

/// `key` project `id`'s here: its key now when `current`.
fn key_kept(conn: &Connection, id: &str, key: &str, current: bool) -> rusqlite::Result<()> {
    if current {
        conn.execute("UPDATE project_keys SET current = 0 WHERE id = ?1 AND key <> ?2", [id, key])?;
    }
    conn.execute("INSERT INTO project_keys (key, id, current) VALUES (?1, ?2, ?3)
        ON CONFLICT (key) DO UPDATE SET id = excluded.id, current = MAX(excluded.current, CASE WHEN project_keys.id = excluded.id THEN project_keys.current ELSE 0 END)",
        params![key, id, current])?;
    Ok(())
}

/// A store from before cl-819 names no project: its column, added.
pub(crate) fn project_names(_conn: &Connection) -> rusqlite::Result<()> { Ok(()) }

/// When each project was last used, which the index lists by (Paul, 11-16: most recently used first): a column older indexes gain.
pub(crate) fn project_used(conn: &Connection) -> rusqlite::Result<()> {
    if conn.prepare("SELECT used_at_ms FROM projects LIMIT 0").is_err() {
        conn.execute_batch("ALTER TABLE projects ADD COLUMN used_at_ms INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

/// Older indexes gain public codes without replacing their project identities or rows.
pub(crate) fn project_codes(conn: &Connection) -> rusqlite::Result<()> {
    if conn.prepare("SELECT code FROM projects LIMIT 0").is_err() {
        conn.execute_batch("ALTER TABLE projects ADD COLUMN code TEXT NOT NULL DEFAULT ''")?;
    }
    conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS projects_code ON projects (code) WHERE code <> ''")
}

fn project_code(conn: &Connection, id: &str, preferred: Option<&str>, mut generate: impl FnMut() -> String) -> Result<String> {
    let existing = conn.query_row("SELECT code FROM projects WHERE id = ?1 AND code <> ''", [id], |row| row.get::<_, String>(0)).optional()?;
    if let Some(code) = preferred {
        if crate::project::code::parse(code).as_deref() != Some(code) {
            return Err(StoreError::ProjectCode(format!("project {id} has an invalid public code {code:?}")));
        }
        if existing.as_deref().is_some_and(|existing| existing != code) {
            return Err(StoreError::ProjectCode(format!("project {id} already has another public code")));
        }
    }
    if let Some(code) = existing {
        return Ok(code);
    }
    for _ in 0..8 {
        let code = preferred.map(str::to_owned).unwrap_or_else(&mut generate);
        let taken: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE code = ?1)", [&code], |row| row.get(0))?;
        if !taken {
            return Ok(code);
        }
        if preferred.is_some() {
            return Err(StoreError::ProjectCode(format!("public code {code} already belongs to another project")));
        }
    }
    Err(StoreError::ProjectCode("could not assign a unique public project code after 8 attempts".into()))
}

#[cfg(test)]
mod project_code_tests {
    use super::*;

    #[test]
    fn collision_retries_and_kept_codes_are_checked() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(TABLES).unwrap();
        project_codes(&conn).unwrap();
        conn.execute("INSERT INTO projects (id, path, used_at_ms, code) VALUES ('old', '/old', 1, 'ABCD12345')", []).unwrap();
        let mut candidates = ["ABCD12345", "EFGH12345"].into_iter();
        assert_eq!(project_code(&conn, "new", None, || candidates.next().unwrap().into()).unwrap(), "EFGH12345");
        assert!(project_code(&conn, "new", Some("ABCD12345"), || unreachable!()).is_err());
        assert!(project_code(&conn, "old", Some("EFGH12345"), || unreachable!()).is_err());
        assert_eq!(project_code(&conn, "old", None, || unreachable!()).unwrap(), "ABCD12345");
        assert!(project_code(&conn, "new", None, || "ABCD12345".into()).is_err());
    }

    #[test]
    fn legacy_index_gains_code_column_and_unique_reservation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let store = Store::open(&path).unwrap();
        store.write(|tx| {
            tx.execute_batch("DROP INDEX projects_code; ALTER TABLE projects DROP COLUMN code;")?;
            tx.execute("INSERT INTO projects (id, name, path, used_at_ms) VALUES ('old', 'Old', '/old', 42)", [])?;
            Ok(())
        }).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.known_project("old").unwrap().unwrap().code, "");
        let code = store.project_opened("old", "Old", Path::new("/old"), None, None).unwrap();
        assert_eq!(store.known_project("old").unwrap().unwrap().used_at_ms, 42);
        assert_eq!(store.project_opened("old", "Renamed", Path::new("/moved"), None, None).unwrap(), code);
        assert!(store.project_opened("new", "New", Path::new("/new"), None, Some(&code)).is_err());
        assert!(store.known_project("new").unwrap().is_none());
        drop(store);
        assert_eq!(Store::open(&path).unwrap().known_project("old").unwrap().unwrap().code, code);
    }
}

fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    Ok(statement.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Your tool list as it's kept: the packages you added, and the tools you took off.
fn tool_list_in(conn: &Connection) -> Result<ToolList> {
    Ok(ToolList { packages: strings(conn, "SELECT package FROM tool_list ORDER BY ord")?, without: strings(conn, "SELECT tool FROM tools_off ORDER BY ord")? })
}

/// An ask's step is kept by its key's digest, as its file was named.
fn digest(key: &str) -> String {
    hex::encode(Sha256::digest(key))
}


/// A folder as the index keys it: its components, so `/a/b/` and `/a/b` are one folder to its string compares (cl-1090: a project
/// registered with a trailing slash kept its locked handle's row beside it, and its handle named nothing).
fn folder(path: &Path) -> String {
    path.components().collect::<PathBuf>().to_string_lossy().into_owned()
}

impl Store {
    pub(crate) fn known_project(&self, id: &str) -> Result<Option<KnownProject>> {
        self.read(|conn| Ok(conn.query_row(&format!("{KNOWN} WHERE id = ?1"), [id], known).optional()?))
    }

    /// When each project's deleted boxes are next due (`project::purge`), kept so a start opens only those: a project with none has
    /// nothing to purge until a box of it is deleted, or it opens (cl-1010).
    pub(crate) fn purge_due(&self) -> Result<Vec<(String, i64)>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT project, at_ms FROM purge_due")?;
            Ok(statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// `project`'s next due time as the purger keeps it: the time, or none to take it out.
    pub(crate) fn keep_purge_due(&self, project: &str, at_ms: Option<i64>) -> Result<()> {
        self.write(|tx| {
            match at_ms {
                Some(at) => tx.execute("INSERT INTO purge_due (project, at_ms) VALUES (?1, ?2) ON CONFLICT (project) DO UPDATE SET at_ms = ?2", params![project, at])?,
                None => tx.execute("DELETE FROM purge_due WHERE project = ?1", [project])?,
            };
            Ok(())
        })
    }

    /// Every project opened here, last used first.
    pub(crate) fn known_projects(&self) -> Result<Vec<KnownProject>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!("{KNOWN} ORDER BY used_at_ms DESC, id"))?;
            Ok(statement.query_map([], known)?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// These left the index: their folders are gone (cl-1031). Only where each was stays, as a locked project's folder does: not its
    /// id, code or KEY-N names, their pages overwritten and the WAL emptied. Opened again from its folder, it registers again from its
    /// own project.db.
    pub(crate) fn projects_gone(&self, ids: &[String]) -> Result<()> {
        self.write(|tx| {
            tx.pragma_update(None, "secure_delete", "ON")?;
            let mut paths = tx.prepare("SELECT path FROM projects WHERE id = ?1")?;
            for id in ids {
                let Some(path) = paths.query_row([id], |row| row.get::<_, String>(0)).optional()? else { continue };
                let mut bytes = [0u8; 16];
                getrandom::fill(&mut bytes).expect("the OS gives random bytes");
                tx.execute("DELETE FROM projects_gone WHERE path = ?1", [&path])?;
                tx.execute("INSERT INTO projects_gone (id, code, path) VALUES (?1, '', ?2)", params![format!("gone-{}", hex::encode(bytes)), path])?;
                tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
                tx.execute("DELETE FROM project_keys WHERE id = ?1", [id])?;
                tx.execute("DELETE FROM purge_due WHERE project = ?1", [id])?;
                tx.execute("DELETE FROM project_handles WHERE path = ?1", [&path])?;
            }
            Ok(())
        })?;
        // A reader's snapshot keeps the old pages a moment: emptied by the next write that empties it, never a refused listing.
        if let Err(e) = self.empty_wal() { tracing::warn!("state.db: {e}"); }
        Ok(())
    }

    /// `id`, called `name`, in `path`, opened at `used_at_ms` (none: when it was last, or now if it's new here); whether it was walking stands.
    pub(crate) fn project_opened(&self, id: &str, _name: &str, path: &Path, used_at_ms: Option<i64>, preferred: Option<&str>) -> Result<String> {
        let path = folder(path);
        // Already so, as at nearly every open: read, the writer untouched (cl-1010: each open wrote it, and waited on whatever held the writer).
        let kept = self.read(|conn| {
            let kept = conn.query_row(
                "SELECT code FROM projects WHERE id = ?1 AND path = ?2 AND code <> ''
                   AND NOT EXISTS (SELECT 1 FROM projects WHERE path = ?2 AND id <> ?1)
                   AND NOT EXISTS (SELECT 1 FROM projects_gone WHERE id = ?1 OR path = ?2)",
                params![id, path],
                |row| row.get::<_, String>(0),
            );
            Ok(kept.optional()?)
        })?;
        // Its use, when it's one, is `project_used`'s, off the open's path.
        if let Some(code) = kept.filter(|code| preferred.is_none_or(|preferred| preferred == code)) {
            return Ok(code);
        }
        self.write(|tx| {
            // What it retires leaves no page behind (cl-984): a locked project's handle row among them.
            tx.pragma_update(None, "secure_delete", "ON")?;
            // The handle it had while locked names it still, until its next lock (cl-1090): a page that opened it from the list goes on.
            tx.execute("INSERT OR IGNORE INTO project_handles (handle, path) SELECT id, path FROM projects WHERE path = ?1 AND id LIKE 'locked-%'", [&path])?;
            // Its locked handle there was this project: its last use stays.
            let was: i64 = tx.query_row("SELECT COALESCE(MAX(used_at_ms), 0) FROM projects WHERE id = ?1 OR path = ?2", params![id, path], |row| row.get(0))?;
            tx.execute("DELETE FROM projects WHERE path = ?2 AND id <> ?1", params![id, path])?;
            tx.execute("DELETE FROM projects_gone WHERE id = ?1 OR path = ?2", params![id, path])?;
            let code = project_code(tx, id, preferred, crate::project::code::generate)?;
            tx.execute("INSERT INTO projects (id, path, code, used_at_ms) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(id) DO UPDATE SET path = excluded.path, code = excluded.code, used_at_ms = excluded.used_at_ms",
                params![id, path, code, used_at_ms.unwrap_or(was).max(was)])?;
            // A handle whose folder no longer holds a project here (it moved, its folder went, it never locked again) goes.
            tx.execute("DELETE FROM project_handles WHERE path NOT IN (SELECT path FROM projects)", [])?;
            Ok(code)
        })
    }

    /// Project `id` used now: a page shows it.
    pub(crate) fn project_used(&self, id: &str, at_ms: i64) -> Result<()> {
        self.write(|tx| Ok(tx.execute("UPDATE projects SET used_at_ms = MAX(used_at_ms, ?2) WHERE id = ?1", params![id, at_ms]).map(drop)?))
    }

    /// The key project `id`, in `path`, goes by here (cl-987), given `held`, the keys its project.db keeps, its key now last: that one
    /// when no other project here has it, else the one this index gave it, else the first free of `proposed` that no other folder here
    /// proposes first by its name, a locked one's too (`key::name_key`): a run's `KEY-N` never reads as another project's (cl-1075).
    /// `review`: a key it holds that another folder's name proposes first, and none of `own` (its own name's and folder's), goes, with
    /// every key it had: true. Each other held key free here names it too.
    pub(crate) fn project_keyed(&self, id: &str, path: &Path, held: &[String], own: &[String], review: bool, mut proposed: impl Iterator<Item = String>) -> Result<(String, bool)> {
        // Already so, as at nearly every open (one not under review): its key now is its project.db's, and each other key it keeps names
        // it or another project here. Read, the writer untouched (cl-1010).
        let kept = if review { None } else {
            self.read(|conn| {
                let Some(last) = held.last() else { return Ok(None) };
                let ours = |key: &str, current: bool| {
                    conn.query_row("SELECT EXISTS (SELECT 1 FROM project_keys WHERE key = ?1 AND id = ?2 AND current >= ?3)", params![key, id, current], |row| row.get::<_, bool>(0))
                };
                for key in held.iter().filter(|key| api::calcifer::project::key(key).as_ref() == Some(*key)) {
                    if !ours(key, false)? && key_free(conn, id, key)? { return Ok(None); }
                }
                Ok(ours(last, true)?.then(|| last.clone()))
            })?
        };
        if let Some(key) = kept {
            return Ok((key, false));
        }
        self.write(|tx| {
            let mut statement = tx.prepare("SELECT path FROM projects WHERE id <> ?1")?;
            let paths = statement.query_map([id], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            let others: std::collections::HashSet<String> = paths.iter().map(Path::new).filter(|other| *other != path)
                .filter_map(Path::file_name).map(|folder| crate::project::key::name_key(&folder.to_string_lossy()))
                .filter(|key| !own.contains(key)).collect();
            let goes = |key: &String| review && others.contains(key);
            for key in held.iter().filter(|key| api::calcifer::project::key(key).as_ref() == Some(*key) && !goes(*key)) {
                if key_free(tx, id, key)? { key_kept(tx, id, key, false)?; }
            }
            let mine = |key: &String| tx.query_row("SELECT EXISTS (SELECT 1 FROM project_keys WHERE key = ?1 AND id = ?2)", [key, id], |row| row.get::<_, bool>(0));
            let wanted = match held.last() {
                Some(key) if mine(key)? => Some(key.clone()),
                _ => tx.query_row("SELECT key FROM project_keys WHERE id = ?1 AND current = 1", [id], |row| row.get(0)).optional()?
                    .filter(|key: &String| key_free(tx, id, key).unwrap_or(false)),
            };
            let rekeyed = wanted.as_ref().is_some_and(goes) || held.last().is_some_and(goes);
            if rekeyed {
                tx.execute("DELETE FROM project_keys WHERE id = ?1", [id])?;
            }
            let key = match wanted.filter(|_| !rekeyed) {
                Some(key) => key,
                None => loop {
                    let key = proposed.next().ok_or_else(|| StoreError::ProjectKey("no free project key".into()))?;
                    if !others.contains(&key) && key_free(tx, id, &key)? { break key; }
                },
            };
            key_kept(tx, id, &key, true)?;
            Ok((key, rekeyed))
        })
    }

    /// Project `id`'s key is `key` now (cl-987); the one it had names it still. Refused when another project here has or had it.
    pub(crate) fn project_key_changed(&self, id: &str, key: &str) -> Result<()> {
        self.write(|tx| {
            if api::calcifer::project::RESERVED_KEYS.contains(&key) {
                return Err(StoreError::ProjectKey(format!("{key} is Calcifer's own: its runs are {key}-N")));
            }
            if !key_free(tx, id, key)? {
                return Err(StoreError::ProjectKey(format!("another project on this computer has the key {key}")));
            }
            Ok(key_kept(tx, id, key, true)?)
        })
    }

    /// The folder a locked project's `handle` named before it was opened (cl-1090), until its next lock.
    pub(crate) fn handle_path(&self, handle: &str) -> Result<Option<PathBuf>> {
        self.read(|conn| Ok(conn.query_row("SELECT path FROM project_handles WHERE handle = ?1", [handle], |row| row.get::<_, String>(0)).optional()?.map(PathBuf::from)))
    }

    /// The project `key` names here, now or before (cl-987).
    pub(crate) fn key_owner(&self, key: &str) -> Result<Option<String>> {
        self.read(|conn| Ok(conn.query_row("SELECT project_keys.id FROM project_keys JOIN projects ON projects.id = project_keys.id WHERE key = ?1",
            [key], |row| row.get(0)).optional()?))
    }

    /// The keys project `id` had here, which name it still: oldest first.
    pub(crate) fn former_keys(&self, id: &str) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT key FROM project_keys WHERE id = ?1 AND current = 0 ORDER BY rowid")?;
            Ok(statement.query_map([id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// A locked encrypted project keeps only where it is here (cl-984): an opaque handle and its folder, so a list offers Unlock.
    /// Its id, code and anything else go, their pages overwritten and the WAL emptied; unlocking opens it, which registers it again
    /// from its own project.db.
    pub(crate) fn project_locked(&self, id: &str, path: &Path) -> Result<()> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS gives random bytes");
        let handle = format!("locked-{}", hex::encode(bytes));
        let path = folder(path);
        self.write(|tx| {
            tx.pragma_update(None, "secure_delete", "ON")?;
            // When it was last used stays, so the list keeps its place.
            let used: i64 = tx.query_row("SELECT COALESCE(MAX(used_at_ms), 0) FROM projects WHERE id = ?1 OR path = ?2", params![id, path], |row| row.get(0))?;
            tx.execute("DELETE FROM projects WHERE id = ?1 OR path = ?2", params![id, path])?;
            tx.execute("DELETE FROM projects_gone WHERE id = ?1 OR path = ?2", params![id, path])?;
            // Its keys too (cl-987): its project.db keeps them, its next open registers them again, and its KEY-N is unknown here meanwhile.
            tx.execute("DELETE FROM project_keys WHERE id = ?1", [id])?;
            // The handles of its last lock go: each lock's is new.
            tx.execute("DELETE FROM project_handles WHERE path = ?1", [&path])?;
            tx.execute("INSERT INTO projects (id, path, code, used_at_ms) VALUES (?1, ?2, '', ?3)", params![handle, path, used])?;
            Ok(())
        })?;
        self.empty_wal()
    }

    /// The WAL checkpointed into state.db and emptied, retried while a reader holds an older snapshot: no deleted row's page outlives
    /// this there.
    fn empty_wal(&self) -> Result<()> {
        for _ in 0..40 {
            let busy: i64 = self.read(|conn| Ok(conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?))?;
            if busy == 0 { return Ok(()); }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err(StoreError::Protected("a reader holds the index's older pages of a project it let go".into()))
    }

    /// Whether each project is being walked, which the next start walks again.

    /// Your tool list.
    pub(crate) fn tool_list(&self) -> Result<ToolList> {
        self.read(tool_list_in)
    }

    /// One change of your tool list: `add`, a package and its tools' prefix, on it with all its tools, and `off` taken off.
    pub(crate) fn change_tool_list(&self, add: Option<(&str, &str)>, off: &[String]) -> Result<ToolList> {
        self.write(|tx| {
            if let Some((package, prefix)) = add {
                tx.execute("DELETE FROM tools_off WHERE substr(tool, 1, length(?1)) = ?1", [prefix])?;
                tx.execute("INSERT OR IGNORE INTO tool_list (package) VALUES (?1)", [package])?;
            }
            for tool in off {
                tx.execute("INSERT OR IGNORE INTO tools_off (tool) VALUES (?1)", [tool])?;
            }
            tool_list_in(tx)
        })
    }

    /// What each header read here said, by Hub and sha256, while TensorFS's families are `families`: what others said goes.
    pub(crate) fn header_facts(&self, _families: &str) -> Result<HashMap<(String,String),Facts>> { Ok(HashMap::new()) }
    pub(crate) fn keep_header_facts(&self, _families: &str, _read: &[((String,String),Facts)]) -> Result<()> { Ok(()) }


}

