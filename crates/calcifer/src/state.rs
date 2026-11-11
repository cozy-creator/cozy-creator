//! Calcifer's own state beside its runs, as tables of `state.db` (cl-818): the project folders it opened, browser sessions, your tool
//! list, Render here, what each model header said, and an ask's step after its run. Each change is one transaction; nothing is
//! rewritten whole. The JSON files an earlier Calcifer kept these in are imported once, at open, and deleted ([`import`]).
use std::{
    collections::{BTreeMap, HashMap},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use api::calcifer::tool::ToolList;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use tensorfs::family::Facts;
use tracing::{info, warn};

use crate::store::{Result, Store, StoreError};

/// Made at open when absent, at the store's version: every statement names its columns.
pub(crate) const TABLES: &str = "
    CREATE TABLE IF NOT EXISTS projects (
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL,
        used_at_ms INTEGER NOT NULL,
        walking INTEGER NOT NULL DEFAULT 0,
        name TEXT NOT NULL DEFAULT '',
        code TEXT NOT NULL DEFAULT ''
    ) STRICT;
    CREATE TABLE IF NOT EXISTS sessions (
        token_sha256 TEXT PRIMARY KEY,
        media_sha256 TEXT NOT NULL UNIQUE,
        used_at_ms INTEGER NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS session_codes (
        code_sha256 TEXT PRIMARY KEY,
        expires_ms INTEGER NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS tool_list (
        ord INTEGER PRIMARY KEY,
        package TEXT NOT NULL UNIQUE
    ) STRICT;
    CREATE TABLE IF NOT EXISTS tools_off (
        ord INTEGER PRIMARY KEY,
        tool TEXT NOT NULL UNIQUE
    ) STRICT;
    CREATE TABLE IF NOT EXISTS rendered_here (
        tool TEXT NOT NULL,
        machine TEXT NOT NULL,
        bytes INTEGER NOT NULL,
        PRIMARY KEY (tool, machine)
    ) STRICT, WITHOUT ROWID;
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
    ) STRICT;";

/// A project Calcifer opened, by its id: its name, and where it last was.
#[derive(Clone, Debug)]
pub(crate) struct KnownProject {
    pub id: String,
    pub code: String,
    pub name: String,
    pub path: PathBuf,
    pub used_at_ms: i64,
    /// Its walk was going: the next start walks it again.
    pub walking: bool,
}

/// A browser session, by its secrets' digests.
#[derive(Clone, Debug)]
pub(crate) struct SessionRow {
    pub token: String,
    pub media: String,
    pub used_at_ms: i64,
}

fn known(row: &rusqlite::Row) -> rusqlite::Result<KnownProject> {
    let path = PathBuf::from(row.get::<_, String>(1)?);
    let name = Some(row.get::<_, String>(4)?).filter(|name| !name.is_empty());
    let name = name.unwrap_or_else(|| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    Ok(KnownProject { id: row.get(0)?, code: row.get(5)?, name, path, used_at_ms: row.get(2)?, walking: row.get(3)? })
}

/// A store from before cl-819 names no project: its column, added.
pub(crate) fn project_names(conn: &Connection) -> rusqlite::Result<()> {
    let named = conn.prepare("SELECT name FROM projects LIMIT 0").is_ok();
    match named {
        true => Ok(()),
        false => conn.execute_batch("ALTER TABLE projects ADD COLUMN name TEXT NOT NULL DEFAULT ''"),
    }
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

/// Your tool list as it's kept; none until it's changed.
fn tool_list_in(conn: &Connection) -> Result<Option<ToolList>> {
    let packages = strings(conn, "SELECT package FROM tool_list ORDER BY ord")?;
    if packages.is_empty() {
        return Ok(None);
    }
    Ok(Some(ToolList { packages, without: strings(conn, "SELECT tool FROM tools_off ORDER BY ord")? }))
}

/// An ask's step is kept by its key's digest, as its file was named.
fn digest(key: &str) -> String {
    hex::encode(Sha256::digest(key))
}

impl Store {
    pub(crate) fn known_project(&self, id: &str) -> Result<Option<KnownProject>> {
        self.read(|conn| Ok(conn.query_row("SELECT id, path, used_at_ms, walking, name, code FROM projects WHERE id = ?1", [id], known).optional()?))
    }

    /// Every project opened here, last used first.
    pub(crate) fn known_projects(&self) -> Result<Vec<KnownProject>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT id, path, used_at_ms, walking, name, code FROM projects ORDER BY used_at_ms DESC, id")?;
            Ok(statement.query_map([], known)?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// These left the index: their folders are gone.
    pub(crate) fn projects_gone(&self, ids: &[String]) -> Result<()> {
        self.write(|tx| {
            let mut statement = tx.prepare("DELETE FROM projects WHERE id = ?1")?;
            for id in ids {
                statement.execute([id])?;
            }
            Ok(())
        })
    }

    /// `id`, called `name`, in `path`, opened at `used_at_ms` (none: when it was last, or now if it's new here); whether it was walking stands.
    pub(crate) fn project_opened(&self, id: &str, name: &str, path: &Path, used_at_ms: Option<i64>, preferred: Option<&str>) -> Result<String> {
        self.write(|tx| {
            // A folder holds one project: another the index had there is gone.
            tx.execute("DELETE FROM projects WHERE path = ?2 AND id <> ?1", params![id, path.to_string_lossy()])?;
            let code = project_code(tx, id, preferred, crate::project::code::generate)?;
            tx.execute(
                "INSERT INTO projects (id, path, used_at_ms, name, code) VALUES (?1, ?2, COALESCE(?4, CAST(strftime('%s', 'now') AS INTEGER) * 1000), ?3, ?5)
                 ON CONFLICT (id) DO UPDATE SET path = excluded.path, name = excluded.name, code = excluded.code, used_at_ms = COALESCE(?4, projects.used_at_ms)",
                params![id, path.to_string_lossy(), name, used_at_ms, code],
            )?;
            Ok(code)
        })
    }

    /// Whether each project is being walked, which the next start walks again.
    pub(crate) fn projects_walking(&self, walking: &[(String, bool)]) -> Result<()> {
        self.write(|tx| {
            let mut statement = tx.prepare("UPDATE projects SET walking = ?2 WHERE id = ?1 AND walking <> ?2")?;
            for (id, walking) in walking {
                statement.execute(params![id, walking])?;
            }
            Ok(())
        })
    }

    /// The sessions used after `since_ms`; the rest have ended, and go.
    pub(crate) fn sessions(&self, since_ms: i64) -> Result<Vec<SessionRow>> {
        self.write(|tx| {
            tx.execute("DELETE FROM sessions WHERE used_at_ms <= ?1", [since_ms])?;
            let mut statement = tx.prepare("SELECT token_sha256, media_sha256, used_at_ms FROM sessions")?;
            let rows = statement.query_map([], |r| Ok(SessionRow { token: r.get(0)?, media: r.get(1)?, used_at_ms: r.get(2)? }))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Keep only the digest of an unredeemed browser sign-in code.
    pub(crate) fn session_code(&self, code: &str, now: i64, expires_ms: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute("DELETE FROM session_codes WHERE expires_ms <= ?1", [now])?;
            tx.execute("INSERT INTO session_codes (code_sha256, expires_ms) VALUES (?1, ?2)", params![code, expires_ms])?;
            Ok(())
        })
    }

    /// Redeem a valid code and save its session in the same transaction.
    pub(crate) fn session_started(&self, code: &str, session: &SessionRow, now: i64, since_ms: i64) -> Result<bool> {
        self.write(|tx| {
            tx.execute("DELETE FROM session_codes WHERE expires_ms <= ?1", [now])?;
            if tx.execute("DELETE FROM session_codes WHERE code_sha256 = ?1", [code])? == 0 {
                return Ok(false);
            }
            tx.execute("DELETE FROM sessions WHERE used_at_ms <= ?1", [since_ms])?;
            tx.execute(
                "INSERT INTO sessions (token_sha256, media_sha256, used_at_ms) VALUES (?1, ?2, ?3)",
                params![session.token, session.media, session.used_at_ms],
            )?;
            Ok(true)
        })
    }

    pub(crate) fn session_used(&self, token: &str, at_ms: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE sessions SET used_at_ms = ?2 WHERE token_sha256 = ?1", params![token, at_ms])?;
            Ok(())
        })
    }

    pub(crate) fn session_ended(&self, token: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute("DELETE FROM sessions WHERE token_sha256 = ?1", [token])?;
            Ok(())
        })
    }

    /// Your tool list; none until it's changed.
    pub(crate) fn tool_list(&self) -> Result<Option<ToolList>> {
        self.read(tool_list_in)
    }

    /// One change of your tool list, `defaults` until its first: `add`, a package and its tools' prefix, on it with all its tools, and
    /// `off` taken off.
    pub(crate) fn change_tool_list(&self, defaults: &[&str], add: Option<(&str, &str)>, off: &[String]) -> Result<ToolList> {
        self.write(|tx| {
            if tool_list_in(tx)?.is_none() {
                for package in defaults {
                    tx.execute("INSERT OR IGNORE INTO tool_list (package) VALUES (?1)", [package])?;
                }
            }
            if let Some((package, prefix)) = add {
                tx.execute("DELETE FROM tools_off WHERE substr(tool, 1, length(?1)) = ?1", [prefix])?;
                tx.execute("INSERT OR IGNORE INTO tool_list (package) VALUES (?1)", [package])?;
            }
            for tool in off {
                tx.execute("INSERT OR IGNORE INTO tools_off (tool) VALUES (?1)", [tool])?;
            }
            Ok(tool_list_in(tx)?.unwrap_or_default())
        })
    }

    /// Each tool kept as rendered on `machine`, by the weights it was said for.
    pub(crate) fn rendered_here(&self, machine: &str) -> Result<HashMap<String, u64>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT tool, bytes FROM rendered_here WHERE machine = ?1")?;
            let rows = statement.query_map([machine], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Each of `tools` kept as rendered on `machine`, for the weights it has now.
    pub(crate) fn keep_rendered_here(&self, machine: &str, tools: &[(String, u64)]) -> Result<()> {
        self.write(|tx| {
            let mut statement = tx.prepare("INSERT OR REPLACE INTO rendered_here (tool, machine, bytes) VALUES (?1, ?2, ?3)")?;
            for (tool, bytes) in tools {
                statement.execute(params![tool, machine, *bytes as i64])?;
            }
            Ok(())
        })
    }

    /// What each header read here said, by Hub and sha256, while TensorFS's families are `families`: what others said goes.
    pub(crate) fn header_facts(&self, families: &str) -> Result<HashMap<(String, String), Facts>> {
        self.write(|tx| {
            tx.execute("DELETE FROM headers WHERE families <> ?1", [families])?;
            let mut statement = tx.prepare("SELECT hub, sha256, lora, family FROM headers")?;
            let rows = statement.query_map([], |r| Ok(((r.get(0)?, r.get(1)?), Facts { lora: r.get(2)?, family: r.get(3)? })))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What headers said, by Hub and sha256, read under `families`.
    pub(crate) fn keep_header_facts(&self, families: &str, read: &[((String, String), Facts)]) -> Result<()> {
        self.write(|tx| {
            let mut statement = tx.prepare("INSERT OR REPLACE INTO headers (hub, sha256, families, lora, family) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for ((hub, sha256), facts) in read {
                statement.execute(params![hub, sha256, families, facts.lora, facts.family])?;
            }
            Ok(())
        })
    }

    /// Keeps `step` (JSON) for the ask under `key`, before its run is submitted.
    pub(crate) fn plan_after(&self, key: &str, step: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute("INSERT OR REPLACE INTO after_steps (key_sha256, step) VALUES (?1, ?2)", params![digest(key), step])?;
            Ok(())
        })
    }

    pub(crate) fn after_step(&self, key: &str) -> Result<Option<String>> {
        self.read(|conn| Ok(conn.query_row("SELECT step FROM after_steps WHERE key_sha256 = ?1", [digest(key)], |r| r.get(0)).optional()?))
    }

    pub(crate) fn forget_after(&self, key: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute("DELETE FROM after_steps WHERE key_sha256 = ?1", [digest(key)])?;
            Ok(())
        })
    }
}

#[derive(Deserialize)]
struct OldIndex {
    #[serde(default)]
    projects: BTreeMap<String, OldProject>,
}

#[derive(Deserialize)]
struct OldProject {
    path: PathBuf,
    used_at_ms: i64,
    #[serde(default)]
    walking: bool,
}

#[derive(Deserialize)]
struct OldSession {
    token: String,
    media: String,
    used_at_ms: i64,
}

#[derive(Deserialize)]
struct OldHere {
    #[serde(default)]
    here: Vec<OldRenderedHere>,
}

#[derive(Deserialize)]
struct OldRenderedHere {
    tool: String,
    machine: String,
    #[serde(default)]
    bytes: u64,
}

#[derive(Deserialize)]
struct OldHeaders {
    families: String,
    facts: HashMap<String, Facts>,
}

/// What an old file holds: none when it isn't there, or can't be read now (it stays for a later start); `Some(None)` when it can't
/// be parsed, and goes, as the next write of it would have replaced it.
fn old<T: DeserializeOwned>(path: &Path) -> Option<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            warn!("{}: {e}; imported at a later start", path.display());
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(read) => Some(Some(read)),
        Err(e) => {
            warn!("{}: {e}; not imported", path.display());
            Some(None)
        }
    }
}

/// Imports the JSON files an earlier Calcifer kept its own state in, in `dir` beside `state.db`, in one transaction, then deletes them.
/// A row already here wins: killed between the commit and the deletes, the next start imports the same files over what they wrote.
pub(crate) fn import(store: &Store, dir: &Path) {
    let file = |name: &str| dir.join(name);
    let index = old::<OldIndex>(&file("projects.json"));
    let sessions = old::<Vec<OldSession>>(&file("sessions.json"));
    let tools = old::<ToolList>(&file("tools.json"));
    let here = old::<OldHere>(&file("here.json"));
    let headers = old::<OldHeaders>(&file("checkpoints.json"));
    let after_dir = dir.join("after");
    let mut steps = vec![];
    if let Ok(entries) = std::fs::read_dir(&after_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(key) = (path.extension().is_some_and(|e| e == "json")).then(|| path.file_stem()).flatten() else { continue };
            let Some(step) = old::<serde_json::Value>(&path) else { continue };
            steps.push((key.to_string_lossy().into_owned(), step.map(|step| step.to_string()), path));
        }
    }
    let families = tensorfs::family::families_digest();
    let none = index.is_none() && sessions.is_none() && tools.is_none() && here.is_none() && headers.is_none() && steps.is_empty();
    if none {
        let _ = std::fs::remove_dir(&after_dir);
        return;
    }
    let mut counts = BTreeMap::new();
    let imported = store.write(|tx| {
        for (id, project) in index.iter().flatten().flat_map(|index| &index.projects) {
            let sql = "INSERT OR IGNORE INTO projects (id, path, used_at_ms, walking) VALUES (?1, ?2, ?3, ?4)";
            *counts.entry("projects").or_insert(0) += tx.execute(sql, params![id, project.path.to_string_lossy(), project.used_at_ms, project.walking])?;
        }
        for session in sessions.iter().flatten().flatten() {
            let sql = "INSERT OR IGNORE INTO sessions (token_sha256, media_sha256, used_at_ms) VALUES (?1, ?2, ?3)";
            *counts.entry("sessions").or_insert(0) += tx.execute(sql, params![session.token, session.media, session.used_at_ms])?;
        }
        if let Some(Some(list)) = &tools
            && tool_list_in(tx)?.is_none()
        {
            for package in &list.packages {
                *counts.entry("tool_list").or_insert(0) += tx.execute("INSERT OR IGNORE INTO tool_list (package) VALUES (?1)", [package])?;
            }
            for tool in &list.without {
                *counts.entry("tools_off").or_insert(0) += tx.execute("INSERT OR IGNORE INTO tools_off (tool) VALUES (?1)", [tool])?;
            }
        }
        for held in here.iter().flatten().flat_map(|here| &here.here) {
            let sql = "INSERT OR IGNORE INTO rendered_here (tool, machine, bytes) VALUES (?1, ?2, ?3)";
            *counts.entry("rendered_here").or_insert(0) += tx.execute(sql, params![held.tool, held.machine, held.bytes as i64])?;
        }
        // What other families said is read again.
        for (key, facts) in headers.iter().flatten().filter(|held| held.families == families).flat_map(|held| &held.facts) {
            let Some((hub, sha256)) = key.rsplit_once(' ') else { continue };
            let sql = "INSERT OR IGNORE INTO headers (hub, sha256, families, lora, family) VALUES (?1, ?2, ?3, ?4, ?5)";
            *counts.entry("headers").or_insert(0) += tx.execute(sql, params![hub, sha256, families, facts.lora, facts.family])?;
        }
        for (key, step, _) in &steps {
            let Some(step) = step else { continue };
            let sql = "INSERT OR IGNORE INTO after_steps (key_sha256, step) VALUES (?1, ?2)";
            *counts.entry("after_steps").or_insert(0) += tx.execute(sql, params![key, step])?;
        }
        Ok(())
    });
    if let Err(e) = imported {
        warn!("importing Calcifer's files into state.db: {e}; they stay for the next start");
        return;
    }
    info!("imported into state.db: {counts:?}");
    let read = [("projects.json", index.is_some()), ("sessions.json", sessions.is_some()), ("tools.json", tools.is_some()), ("here.json", here.is_some()), ("checkpoints.json", headers.is_some())];
    let gone = read.into_iter().filter(|(_, read)| *read).map(|(name, _)| file(name)).chain(steps.into_iter().map(|(_, _, path)| path));
    for path in gone {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("{}: {e}", path.display());
        }
    }
    let _ = std::fs::remove_dir(&after_dir);
}
