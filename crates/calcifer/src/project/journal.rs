//! Project-owned controller records, on the admitted project's existing SQLite connection.
use std::{path::PathBuf, sync::Arc};
use api::error::Error;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use tokio::sync::OwnedRwLockReadGuard;
use crate::{api::Api, store::{self, Answer, StoreError}};
use super::{Project, Projects, vault};

/// The sole durable outside reference. Activity and display data belong to project.db.
#[derive(Clone, Debug)]
pub(crate) struct ProjectRef {
    pub id: String,
    pub path: PathBuf,
    pub code: String,
}

/// A bounded operation on one verified mounted owner. Never retain across a worker wait.
pub(crate) struct ProjectStore {
    project: Arc<Project>,
    _view: OwnedRwLockReadGuard<()>,
}

impl Projects {
    pub(crate) async fn store(&self, api: &Arc<Api>, named: &str) -> Result<ProjectStore, Error> {
        let view = vault::admit_project(api, named).await?;
        let project = self.get(api, named).map_err(|error| error.0)?;
        project.require_view()?;
        Ok(ProjectStore { project, _view: view })
    }

    /// Currently accessible owners only; no locked project is opened for this snapshot.
    pub(crate) fn accessible(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.open.lock().expect("projects lock").values().filter_map(|project| {
            let project = project.upgrade()?;
            project.require_view().ok()?;
            Some(project.id.clone())
        }).collect();
        ids.sort();
        ids
    }
}

impl ProjectStore {
    pub(crate) fn project_id(&self) -> &str { &self.project.id }

    pub(crate) fn read<T>(&self, read: impl FnOnce(&Connection) -> store::Result<T>) -> store::Result<T> {
        self.project.require_view().map_err(project_error)?;
        self.project.folder.journal_read(read)
    }

    pub(crate) fn write<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.project.require_view().map_err(project_error)?;
        self.project.folder.journal_write(write)
    }

    pub(crate) fn answer(&self, key: &str) -> store::Result<Option<Answer>> {
        self.read(|conn| {
            Ok(conn.query_row("SELECT digest, status, content_type, body FROM request_keys WHERE key = ?1 AND status IS NOT NULL", [key], |row| {
                Ok(Answer { digest: row.get(0)?, status: row.get::<_, i64>(1)? as u16, content_type: row.get(2)?, body: row.get(3)? })
            }).optional()?)
        })
    }

    pub(crate) fn keep_answer(&self, key: &str, answer: &Answer) -> store::Result<()> {
        self.project.require_view().map_err(project_error)?;
        self.project.folder.keep_answer(key, &answer.digest, answer.status, &answer.content_type, &answer.body).map_err(project_error)
    }

    pub(crate) fn append_diagnostic(&self, code: &'static str, detail: &str) -> store::Result<()> {
        self.write(|tx| {
            tx.execute("INSERT INTO project_diagnostics (at_ms, code, detail) VALUES (?1, ?2, ?3)", params![store::now_ms(), code, detail])?;
            Ok(())
        })
    }
}

fn project_error(error: Error) -> StoreError {
    StoreError::Protected(error.code.to_string())
}

pub(super) const SCHEMA: &str = "    CREATE TABLE runs (
        id TEXT PRIMARY KEY,
        number INTEGER NOT NULL UNIQUE,
        idempotency_key TEXT NOT NULL UNIQUE,
        digest TEXT NOT NULL,
        hub TEXT NOT NULL,
        kind TEXT NOT NULL,
        target TEXT NOT NULL,
        machine TEXT,
        pinned INTEGER NOT NULL,
        spec BLOB NOT NULL,
        out_dir TEXT NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('queued', 'sent', 'accepted', 'settled')),
        cancel_requested INTEGER NOT NULL DEFAULT 0,
        cursor INTEGER NOT NULL DEFAULT 0,
        machine_number INTEGER,
        machine_state TEXT,
        status TEXT CHECK (status IN ('completed', 'failed', 'canceled')),
        reason_code TEXT,
        reason TEXT,
        outcome BLOB,
        seq INTEGER NOT NULL DEFAULT 1,
        state_seq INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL,
        accepted_at INTEGER,
        settled_at INTEGER,
        warnings TEXT NOT NULL DEFAULT '[]',
        inputs TEXT NOT NULL DEFAULT '[]',
        wants TEXT,
        agreed TEXT,
        started_at INTEGER,
        stays TEXT NOT NULL DEFAULT '[]',
        batch TEXT NOT NULL DEFAULT '',
        job BLOB
    ) STRICT;
    CREATE INDEX runs_owed ON runs (machine) WHERE state <> 'settled';
    CREATE TABLE outputs (
        run TEXT NOT NULL REFERENCES runs (id),
        name TEXT NOT NULL,
        idx INTEGER NOT NULL,
        rev INTEGER NOT NULL,
        length INTEGER NOT NULL,
        digest TEXT NOT NULL,
        media_type TEXT NOT NULL,
        label TEXT NOT NULL DEFAULT '',
        duration_us INTEGER NOT NULL DEFAULT 0,
        width INTEGER NOT NULL DEFAULT 0,
        height INTEGER NOT NULL DEFAULT 0,
        measured_ms INTEGER NOT NULL DEFAULT 0,
        done INTEGER NOT NULL DEFAULT 0,
        path TEXT,
        exported_rev INTEGER,
        created_seq INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        failed_rev INTEGER,
        export_error TEXT,
        checkpoint TEXT,
        PRIMARY KEY (run, name, idx)
    ) STRICT, WITHOUT ROWID;
    CREATE INDEX outputs_unexported ON outputs (run) WHERE exported_rev IS NOT rev;
    CREATE TABLE run_logs (
        run TEXT NOT NULL REFERENCES runs (id),
        seq INTEGER NOT NULL,
        at_ms INTEGER NOT NULL,
        level TEXT NOT NULL,
        text TEXT NOT NULL,
        PRIMARY KEY (run, seq)
    ) STRICT, WITHOUT ROWID;
    CREATE TABLE asks (
        key TEXT PRIMARY KEY,
        hub TEXT NOT NULL,
        digest TEXT NOT NULL,
        body BLOB NOT NULL,
        machine TEXT,
        state TEXT NOT NULL CHECK (state IN ('pending', 'done', 'failed')),
        reason TEXT,
        created_at INTEGER NOT NULL,
        kind TEXT NOT NULL DEFAULT 'rent' CHECK (kind IN ('rent', 'end')),
        run TEXT REFERENCES runs (id)
    ) STRICT;
    CREATE TABLE project_view (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), document TEXT NOT NULL) STRICT;
    CREATE TABLE project_diagnostics (id INTEGER PRIMARY KEY, at_ms INTEGER NOT NULL, code TEXT NOT NULL, detail TEXT NOT NULL) STRICT;
    CREATE TABLE journal_migrations (source TEXT PRIMARY KEY, digest TEXT NOT NULL, record TEXT NOT NULL) STRICT;
    CREATE TABLE project_after_steps (key_sha256 TEXT PRIMARY KEY, step TEXT NOT NULL) STRICT;
    CREATE TABLE project_headers (hub TEXT NOT NULL, sha256 TEXT NOT NULL, families TEXT NOT NULL, lora INTEGER NOT NULL, family TEXT, PRIMARY KEY (hub, sha256)) STRICT, WITHOUT ROWID;
    CREATE TABLE project_rendered_here (tool TEXT NOT NULL, machine TEXT NOT NULL, bytes INTEGER NOT NULL, PRIMARY KEY (tool, machine)) STRICT, WITHOUT ROWID;
    CREATE TABLE project_vault_operations (id TEXT PRIMARY KEY, binding TEXT NOT NULL, state TEXT NOT NULL) STRICT;
";
