//! Calcifer's store: one SQLite file only Calcifer writes. A run's state lives in its row's
//! columns; what the CLI's event stream replays is derived from them (`events_after`). Durable
//! here: transitions, outputs at their newest revision, and log lines. Never progress.
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::project::vault;
use api::{
    calcifer::{Agreed, Call, Event, ItemStatus, Kind, Machine, MachineKind, MachineState, Memo, OutputItem, Reason, Run, Status, Warning},
    config::Origin,
    ids::{Digest, RunId},
    machine as v1,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

#[cfg(test)]
#[path = "store_listing_tests.rs"]
mod listing_tests;

#[cfg(test)]
#[path = "store_journal_tests.rs"]
mod journal_tests;

/// The store's version (`PRAGMA user_version`), and the schema a new store is made with. A store
/// at another version is not read.
pub(crate) const VERSION: i64 = 7;
const SCHEMA: &str = "
    CREATE TABLE machines (
        name TEXT PRIMARY KEY,
        kind TEXT NOT NULL CHECK (kind IN ('local', 'rental', 'added')),
        hub TEXT NOT NULL DEFAULT '',
        rental_id TEXT,
        worker_id TEXT,
        address TEXT,
        pin BLOB,
        key TEXT NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('booting', 'ready', 'ended')),
        served INTEGER NOT NULL DEFAULT 0,
        reason TEXT,
        created_at INTEGER NOT NULL,
        alias TEXT
    ) STRICT;
    CREATE UNIQUE INDEX machines_rental ON machines (rental_id) WHERE rental_id IS NOT NULL;
    CREATE UNIQUE INDEX machines_alias ON machines (alias) WHERE alias IS NOT NULL;
    CREATE TABLE runs (
        id TEXT PRIMARY KEY,
        number INTEGER NOT NULL UNIQUE,
        idempotency_key TEXT NOT NULL UNIQUE,
        digest TEXT NOT NULL,
        hub TEXT NOT NULL,
        kind TEXT NOT NULL,
        target TEXT NOT NULL,
        machine TEXT REFERENCES machines (name),
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
    ) STRICT;";

/// A store made before a run said what of it stays gains that column, empty: what it made comes here.
fn stays_column(writer: &mut Connection) -> Result<()> {
    let has: bool = writer.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('runs') WHERE name = 'stays')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        writer.execute_batch("ALTER TABLE runs ADD COLUMN stays TEXT NOT NULL DEFAULT '[]'")?;
    }
    Ok(())
}

/// A store made before runs were sent in batches gains that column, empty: each run its own (cl-816).
fn batch_column(writer: &mut Connection) -> Result<()> {
    let has: bool = writer.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('runs') WHERE name = 'batch')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        writer.execute_batch("ALTER TABLE runs ADD COLUMN batch TEXT NOT NULL DEFAULT ''")?;
    }
    Ok(())
}

/// A store made before Calcifer's own runs kept their job gains that column, empty: such a run the next Calcifer can't carry
/// out again fails `calcifer_stopped` (cl-866).
fn job_column(writer: &mut Connection) -> Result<()> {
    let has: bool = writer.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('runs') WHERE name = 'job')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        writer.execute_batch("ALTER TABLE runs ADD COLUMN job BLOB")?;
    }
    Ok(())
}

/// What each project write's `Idempotency-Key` answered (cl-812), kept a day: a store made before it gains the table, at the same
/// version, which a Calcifer that predates it never reads.
const ANSWERS: &str = "
    CREATE TABLE IF NOT EXISTS answers (
        key TEXT PRIMARY KEY,
        digest TEXT NOT NULL,
        status INTEGER NOT NULL,
        content_type TEXT NOT NULL,
        body BLOB NOT NULL,
        at_ms INTEGER NOT NULL
    ) STRICT;";
/// How long an answer is kept for its write's retries.
const ANSWER_KEPT_MS: i64 = 24 * 60 * 60 * 1000;

/// A store made before outputs were measured gains their columns. Its version stands: every statement names its columns, so a
/// Calcifer that predates them goes on reading it.
fn measured_columns(writer: &mut Connection) -> Result<()> {
    let has: bool = writer.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('outputs') WHERE name = 'measured_ms')",
        [],
        |r| r.get(0),
    )?;
    if has {
        return Ok(());
    }
    let tx = writer.transaction()?;
    for column in ["width", "height", "measured_ms"] {
        tx.execute_batch(&format!("ALTER TABLE outputs ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0"))?;
    }
    tx.commit()?;
    Ok(())
}

/// What is measured of an output: 0 for what doesn't apply or couldn't be read.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Measured {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
    #[error("Calcifer's store: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Calcifer's store is at version {0}: this cozy reads version {current}", current = VERSION)]
    Version(i64),
    #[error("idempotency key {0:?} already names another submission")]
    Conflict(String),
    #[error("no machine named {0:?}")]
    UnknownMachine(String),
    #[error("Calcifer's store holds a row it cannot read: {0}")]
    Corrupt(String),
    #[error("project is locked: {0}")]
    Locked(String),
    #[error("private journal: {0}")]
    Protected(String),
    #[error("{0}")]
    ProjectCode(String),
    #[error("Calcifer's store could not flush its journal: {0}")]
    Flush(std::io::Error),
}

pub(crate) type Result<T> = std::result::Result<T, StoreError>;

fn unlocked_rows<T>(rows: Vec<Result<T>>) -> Result<Vec<T>> {
    let mut open = Vec::new();
    for row in rows {
        match row {
            Ok(row) => open.push(row),
            Err(StoreError::Locked(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(open)
}
fn run_context(id: &str, field: &str) -> String {
    format!("run:{id}:{field}")
}
fn output_context(id: &str, name: &str, index: u32, rev: u64, field: &str) -> String {
    serde_json::to_string(&("output", id, name, index, rev, field)).expect("context serializes")
}
fn log_context(id: &str, seq: u64) -> String {
    format!("run:{id}:log:{seq}")
}
fn private_error(error: api::error::Error) -> StoreError {
    StoreError::Protected(format!("{}: {}", error.code, error.message))
}
fn open_bytes(context: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    vault::open_payload(context, bytes)
        .map_err(private_error)?
        .ok_or_else(|| StoreError::Locked("journal data".into()))
}
fn open_text(context: &str, text: &str) -> Result<String> {
    String::from_utf8(open_bytes(context, text.as_bytes())?).map_err(|_| corrupt("a private text field"))
}
fn optional_text(context: &str, text: Option<String>) -> Result<Option<String>> {
    text.map(|text| open_text(context, &text)).transpose()
}
fn sql_private(column: usize, error: StoreError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}
fn sql_text(row: &Row<'_>, column: usize, context: &str) -> rusqlite::Result<String> {
    let text: String = row.get(column)?;
    open_text(context, &text).map_err(|error| sql_private(column, error))
}
fn sql_optional_text(row: &Row<'_>, column: usize, context: &str) -> rusqlite::Result<Option<String>> {
    let text: Option<String> = row.get(column)?;
    optional_text(context, text).map_err(|error| sql_private(column, error))
}
fn sql_optional_path(row: &Row<'_>, column: usize, context: &str) -> rusqlite::Result<Option<PathBuf>> {
    let encoded: Option<String> = row.get(column)?;
    encoded.map(|value| {
        let decoded = open_text(context, &value).map_err(|error| sql_private(column, error))?;
        vault::payload_path(value.as_bytes(), Path::new(&decoded)).map_err(|error| sql_private(column, private_error(error)))
    }).transpose()
}

fn seal_at(path: &Path, context: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    vault::seal_payload(path, context, bytes).map_err(private_error)
}
fn as_text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| corrupt("a protected text encoding"))
}
fn run_scope(conn: &Connection, id: &str) -> Result<(PathBuf, bool)> {
    let (path, spec): (String, Vec<u8>) = conn.query_row("SELECT out_dir, spec FROM runs WHERE id = ?1", [id], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    Ok((PathBuf::from(path), vault::is_payload(&spec)))
}
fn seal_run(conn: &Connection, id: &str, context: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let (path, protected) = run_scope(conn, id)?;
    let sealed = seal_at(&path, context, bytes)?;
    if protected && !vault::is_payload(&sealed) {
        return Err(StoreError::Locked("journal destination".into()));
    }
    Ok(sealed)
}
fn seal_run_text(conn: &Connection, id: &str, context: &str, text: &str) -> Result<String> {
    as_text(seal_run(conn, id, context, text.as_bytes())?)
}
fn answer_path(body: &[u8]) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| Path::new(path).is_absolute())
        .map(PathBuf::from)
}

/// Where a run is between its submission and its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// In the outbox: never sent to its current machine.
    Queued,
    /// Crossed the send boundary: its machine may hold it.
    Sent,
    /// Its machine's first `RunState` was recorded.
    Accepted,
    Settled,
}

#[derive(Clone, Debug)]
pub(crate) struct RunRow {
    pub id: RunId,
    pub number: u64,
    pub kind: Kind,
    pub target: String,
    /// The run's Hub; none for local code.
    pub hub: Option<Origin>,
    pub machine: Option<String>,
    /// The caller named the machine: the run is never placed elsewhere.
    pub pinned: bool,
    /// The spec as it is sent.
    pub spec: v1::RunSpec,
    pub out_dir: PathBuf,
    pub state: State,
    pub cancel_requested: bool,
    /// The last machine sequence whose durable effect is recorded here.
    pub cursor: u64,
    pub machine_state: Option<String>,
    pub status: Option<Status>,
    pub reason: Option<Reason>,
    pub state_seq: u64,
    pub created_at_ms: i64,
    pub accepted_at_ms: Option<i64>,
    /// When its machine began executing it: its time counts from here.
    pub started_at_ms: Option<i64>,
    pub settled_at_ms: Option<i64>,
    pub warnings: Vec<Warning>,
    /// The staged objects its machine is written before the send.
    pub inputs: Vec<(Digest, u64)>,
    /// Its machine's outcome, once settled by one.
    pub outcome: Option<v1::Outcome>,
    /// Where an unplaced run goes: `any`, or `new[:sku]` to rent one.
    pub wants: String,
    /// The quote a `new` run's renter agreed to.
    pub agreed: Option<Agreed>,
}

/// Which runs a listing answers, newest first.
#[derive(Debug, Default)]
pub(crate) struct RunFilter {
    pub status: Option<Status>,
    /// A prefix of the run's target (`org/pkg`).
    pub target: Option<String>,
    pub machine: Option<String>,
    /// Only the runs landing here: a project's.
    pub out_dir: Option<String>,
    /// Only runs numbered below this.
    pub before: Option<u64>,
    pub limit: u64,
}

/// A new run, recorded under its idempotency key.
pub(crate) struct NewRun {
    pub id: RunId,
    pub kind: Kind,
    pub target: String,
    pub hub: Option<Origin>,
    pub machine: Option<String>,
    pub pinned: bool,
    pub spec: v1::RunSpec,
    pub out_dir: PathBuf,
    pub warnings: Vec<Warning>,
    pub inputs: Vec<(Digest, u64)>,
    /// Where an unpinned run may go: `any`, `new` or `new:<sku>`.
    pub wants: Option<String>,
    /// The quote a `new` run's renter agreed to.
    pub agreed: Option<Agreed>,
    /// Its outputs that stay on its machine, by name.
    pub stays: Vec<String>,
    /// The runs submitted with it: those on its machine go there in one admission (cl-816). Empty: alone.
    pub batch: String,
    /// Calcifer's own run (`Kind::Here`): its job's ask, which a next Calcifer carries out again (cl-866).
    pub job: Option<Vec<u8>>,
}

/// How to reach a machine: the id its caps name, its address and its TLS leaf's sha256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub worker_id: String,
    pub address: String,
    pub pin: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRow {
    pub name: String,
    pub kind: MachineKind,
    pub hub: String,
    pub rental_id: Option<String>,
    pub endpoint: Option<Endpoint>,
    /// The signer key file; the machine scopes runs to the key that sent them.
    pub key: PathBuf,
    pub state: MachineState,
    /// It had an endpoint once: runs it never confirmed may be placed elsewhere.
    pub served: bool,
    pub reason: Option<String>,
}

/// Durable effects of a run of machine frames, recorded in one transaction.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    /// The highest durable sequence among the frames.
    pub cursor: u64,
    pub logs: Vec<(i64, String, String)>,
    pub products: Vec<v1::Product>,
    pub machine_state: Option<String>,
    /// When the machine says it began executing the run.
    pub started_at_ms: Option<i64>,
}

impl Batch {
    pub(crate) fn is_empty(&self) -> bool {
        self.cursor == 0 && self.logs.is_empty() && self.products.is_empty() && self.machine_state.is_none() && self.started_at_ms.is_none()
    }
}

/// What recording a machine's acceptance found.
#[derive(Debug, PartialEq)]
pub(crate) enum Acceptance {
    /// Recorded now.
    Fresh(Vec<Event>),
    /// Recorded before (a reattach).
    Again,
    /// The run has moved on from this machine (or ended): the machine must not run it.
    Stale,
}

/// An output whose newest revision has not landed on this computer, which Calcifer still owes.
#[derive(Clone, Debug)]
pub(crate) struct OwedExport {
    pub run: RunId,
    pub number: u64,
    pub name: String,
    pub index: u32,
    pub length: u64,
}

/// One output item at its newest revision, as its export sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OutputRow {
    pub name: String,
    pub index: u32,
    pub rev: u64,
    pub length: u64,
    /// `sha256:<hex>` of this revision's bytes.
    pub digest: String,
    pub media_type: String,
    pub done: bool,
    /// Its file, holding revision `exported_rev`.
    pub path: Option<PathBuf>,
    pub exported_rev: Option<u64>,
    /// Why this revision will not be exported.
    pub export_error: Option<String>,
    pub checkpoint: Option<v1::CheckpointRef>,
}

/// The store: writes on one connection, one at a time, each durable before it returns; reads on
/// their own, which WAL lets go on while a write waits on the disk.
pub(crate) struct Store {
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
    durable: Durable,
}

/// Group commit. A commit only writes the WAL, under the writer lock; one flush of the WAL then
/// makes every commit before it durable. Writes waiting together share a flush, so on a disk
/// whose flush takes seconds a write waits one or two of them, not one per write queued ahead.
struct Durable {
    /// None: SQLite syncs each commit itself (not in WAL mode).
    wal: Option<PathBuf>,
    committed: AtomicU64,
    /// The newest commit a flush covers; held while flushing, so one flush runs at a time.
    flushed: Mutex<u64>,
}

impl Durable {
    fn sqlite() -> Self {
        Self { wal: None, committed: AtomicU64::new(0), flushed: Mutex::new(0) }
    }

    /// Numbers a commit just made, under the writer lock.
    fn committed(&self) -> u64 {
        self.committed.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Returns once `commit` is on disk: covered by a flush already made, or by one made now of
    /// every commit numbered so far.
    fn wait(&self, commit: u64) -> Result<()> {
        let Some(wal) = &self.wal else { return Ok(()) };
        let mut flushed = self.flushed.lock().unwrap_or_else(|e| e.into_inner());
        if *flushed >= commit {
            return Ok(());
        }
        let covers = self.committed.load(Ordering::Acquire);
        match std::fs::OpenOptions::new().write(true).open(wal) {
            Ok(file) => file.sync_data().map_err(StoreError::Flush)?,
            // No WAL: a checkpoint moved every commit into the database and synced it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::Flush(e)),
        }
        *flushed = covers;
        Ok(())
    }
}

impl Store {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        }
        let mut writer = Connection::open(path)?;
        let mode: String = writer.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        let wal = mode.eq_ignore_ascii_case("wal");
        // In WAL mode a commit doesn't sync: `write` flushes for it (`Durable`).
        writer.pragma_update(None, "synchronous", if wal { "NORMAL" } else { "FULL" })?;
        writer.pragma_update(None, "secure_delete", "ON")?;
        writer.pragma_update(None, "foreign_keys", "ON")?;
        match writer.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? {
            0 => {
                let tx = writer.transaction()?;
                tx.execute_batch(SCHEMA)?;
                tx.pragma_update(None, "user_version", VERSION)?;
                tx.commit()?;
            }
            VERSION => {
                measured_columns(&mut writer)?;
                stays_column(&mut writer)?;
                batch_column(&mut writer)?;
                job_column(&mut writer)?;
            }
            other => return Err(StoreError::Version(other)),
        }
        run_list_indexes(&writer)?;
        writer.execute_batch(ANSWERS)?;
        writer.execute_batch(crate::project::vault_operation::TABLES)?;
        writer.execute_batch(crate::state::TABLES)?;
        crate::state::project_names(&writer)?;
        crate::state::project_codes(&writer)?;
        private(path);
        let reader = Connection::open(path)?;
        reader.pragma_update(None, "query_only", "ON")?;
        let mut journal = path.as_os_str().to_owned();
        journal.push("-wal");
        Ok(Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            durable: Durable { wal: wal.then(|| journal.into()), ..Durable::sqlite() },
        })
    }

    /// Waiting for another reader, loading rows and opening private records may block on I/O.
    /// Hand the async worker back before taking the lock, as writes do before their commit.
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let read = || f(&self.reader.lock().unwrap_or_else(|e| e.into_inner()));
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(read),
            _ => read(),
        }
    }

    /// One transaction, durable when it returns. Its flush can wait seconds on a busy disk's
    /// write-back: on a multi-thread runtime the worker says so first, so its other tasks (the
    /// API's among them) move on. The flush runs outside the writer lock: others commit meanwhile.
    pub(crate) fn write<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let commit = || {
            let (value, commit) = {
                let mut conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let value = f(&tx)?;
                tx.commit()?;
                (value, self.durable.committed())
            };
            self.durable.wait(commit)?;
            Ok(value)
        };
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(commit),
            _ => commit(),
        }
    }

    /// Protect already-recorded private content when an ordinary project opts into a vault.
    /// The caller installs the keybag/public recipient before this atomic journal transaction.
    pub(crate) fn protect_project_journal(&self, root: &Path) -> Result<()> {
        self.rewrite_project_journal(root, |context, value, protected| {
            if protected {
                // Validate the existing slot; a prefix alone is not proof that it is valid ciphertext.
                open_bytes(context, value)?;
                return Ok(value.to_vec());
            }
            // A frame may have been sealed after the physical cutover but before this transaction.
            // Only a verified same-root/context envelope is already protected. A legacy plaintext
            // log that merely begins with our prefix is still data and must be encrypted.
            if vault::is_payload(value)
                && vault::payload_root(value).ok().flatten().as_deref() == Some(root)
                && matches!(vault::open_payload(context, value), Ok(Some(_)))
            {
                return Ok(value.to_vec());
            }
            let sealed = seal_at(root, context, value)?;
            if !vault::is_payload(&sealed) {
                return Err(StoreError::Protected(
                    "project encryption is not ready for journal conversion".into(),
                ));
            }
            Ok(sealed)
        })
        .map(drop)
    }

    /// Rewrite every retained protected payload using the owner's verified replacement generation.
    pub(crate) fn rekey_project_journal(&self, root: &Path, old: &project_vault::ProjectSecret, new: &project_vault::Keybag) -> Result<()> {
        self.rewrite_project_journal(root, |context, value, protected| {
            if protected {
                return vault::rekey_payload(context, value, old, new).map_err(private_error);
            }
            // A newly protected slot (target on an older sealed-spec row) can still be
            // plaintext. Put it in the current protected view rather than carrying it on.
            let sealed = seal_at(root, context, value)?;
            if !vault::is_payload(&sealed) {
                return Err(StoreError::Protected("project encryption is not ready for journal rekey".into()));
            }
            Ok(sealed)
        })
        .map(drop)
    }

    fn rewrite_project_journal(&self, root: &Path, mut transform: impl FnMut(&str, &[u8], bool) -> Result<Vec<u8>>) -> Result<bool> {
        self.write(|tx| {
            let all: Vec<(String, String, Vec<u8>)> = {
                // Keep only the prefix in Rust until the path or an envelope may belong to
                // this project. Other projects' plaintext specs can be large.
                let mut rows = tx.prepare("SELECT id, out_dir, substr(spec, 1, ?1) AS spec FROM runs NOT INDEXED")?;
                rows.query_map([vault::PAYLOAD_PREFIX.len() as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut changed = false;
            for (id, directory, probe) in all {
                let path = vault::logical(Path::new(&directory));
                if !path.starts_with(root) && !vault::is_payload(&probe) {
                    continue;
                }
                // Every protected prefix is still fully parsed, including malformed or
                // relocated envelopes whose out_dir no longer identifies their owner.
                let spec: Vec<u8> = tx.query_row("SELECT spec FROM runs WHERE id = ?1", [&id], |row| row.get(0))?;
                let protected = vault::is_payload(&spec);
                let scope = vault::payload_root(&spec).map_err(private_error)?;
                if !path.starts_with(root) && scope.as_deref() != Some(root) {
                    continue;
                }
                if protected { tx.execute("UPDATE runs SET out_dir = ?2 WHERE id = ?1", params![&id, crate::project::landing(root).to_string_lossy()])?; }

                for column in ["spec", "outcome"] {
                    let value: Option<Vec<u8>> =
                        tx.query_row(&format!("SELECT {column} FROM runs WHERE id = ?1"), [&id], |row| row.get(0))?;
                    if let Some(value) = value {
                        let sealed = transform(&run_context(&id, column), &value, protected)?;
                        if sealed != value {
                            tx.execute(&format!("UPDATE runs SET {column} = ?2 WHERE id = ?1"), params![id, sealed])?;
                            changed = true;
                        }
                    }
                }
                for column in ["target", "warnings", "inputs", "reason_code", "reason"] {
                    let value: Option<String> =
                        tx.query_row(&format!("SELECT {column} FROM runs WHERE id = ?1"), [&id], |row| row.get(0))?;
                    if let Some(value) = value {
                        // Earlier private records already sealed their spec, before target
                        // became a private slot. Its own envelope decides whether to retain
                        // or seal it; the spec's marker alone is not sufficient.
                        let protected = protected && (column != "target" || vault::is_payload(value.as_bytes()));
                        let sealed = as_text(transform(&run_context(&id, column), value.as_bytes(), protected)?)?;
                        if sealed != value {
                            tx.execute(&format!("UPDATE runs SET {column} = ?2 WHERE id = ?1"), params![id, sealed])?;
                            changed = true;
                        }
                    }
                }
                let outputs: Vec<(String, u32, u64, Option<u64>, Option<u64>)> = {
                    let mut rows = tx.prepare("SELECT name, idx, rev, exported_rev, failed_rev FROM outputs WHERE run = ?1")?;
                    rows.query_map([&id], |row| {
                        Ok((
                            row.get(0)?,
                            row.get::<_, i64>(1)? as u32,
                            row.get::<_, i64>(2)? as u64,
                            row.get::<_, Option<i64>>(3)?.map(|n| n as u64),
                            row.get::<_, Option<i64>>(4)?.map(|n| n as u64),
                        ))
                    })?
                    .collect::<rusqlite::Result<_>>()?
                };
                for (name, index, rev, exported, failed) in outputs {
                    for (column, revision) in [
                        ("label", rev),
                        ("checkpoint", rev),
                        ("path", exported.unwrap_or(rev)),
                        ("export_error", failed.unwrap_or(rev)),
                    ] {
                        let value: Option<String> = tx.query_row(
                            &format!("SELECT {column} FROM outputs WHERE run = ?1 AND name = ?2 AND idx = ?3"),
                            params![id, name, index],
                            |row| row.get(0),
                        )?;
                        if let Some(value) = value {
                            let sealed = as_text(transform(
                                &output_context(&id, &name, index, revision, column),
                                value.as_bytes(),
                                protected,
                            )?)?;
                            if sealed != value {
                                tx.execute(
                                    &format!("UPDATE outputs SET {column} = ?4 WHERE run = ?1 AND name = ?2 AND idx = ?3"),
                                    params![id, name, index, sealed],
                                )?;
                                changed = true;
                            }
                        }
                    }
                }
                let logs: Vec<(i64, String)> = {
                    let mut rows = tx.prepare("SELECT seq, text FROM run_logs WHERE run = ?1")?;
                    rows.query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for (seq, text) in logs {
                    let sealed = as_text(transform(&log_context(&id, seq as u64), text.as_bytes(), protected)?)?;
                    if sealed != text {
                        tx.execute(
                            "UPDATE run_logs SET text = ?3 WHERE run = ?1 AND seq = ?2",
                            params![id, seq, sealed],
                        )?;
                        changed = true;
                    }
                }
                let asks: Vec<(String, Vec<u8>, Option<String>)> = {
                    let mut rows = tx.prepare("SELECT key, body, reason FROM asks WHERE run = ?1")?;
                    rows.query_map([&id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for (key, body, reason) in asks {
                    let sealed = transform(&format!("ask:{key}:body"), &body, protected)?;
                    let sealed_reason = reason
                        .as_ref()
                        .map(|reason| transform(&format!("ask:{key}:reason"), reason.as_bytes(), protected).and_then(as_text))
                        .transpose()?;
                    if sealed != body || sealed_reason != reason {
                        tx.execute(
                            "UPDATE asks SET body = ?2, reason = ?3 WHERE key = ?1",
                            params![key, sealed, sealed_reason],
                        )?;
                        changed = true;
                    }
                }
            }
            let answers: Vec<String> = {
                // Preserve the old table-scan order rather than using the key index.
                let mut rows = tx.prepare("SELECT key FROM answers NOT INDEXED")?;
                rows.query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for key in answers {
                let body: Vec<u8> = tx.query_row("SELECT body FROM answers WHERE key = ?1", [&key], |row| row.get(0))?;
                let scope = vault::payload_root(&body).map_err(private_error)?.or_else(|| answer_path(&body));
                if scope.as_deref().is_none_or(|path| !vault::logical(path).starts_with(root)) {
                    continue;
                }
                let sealed = transform(&format!("answer:{key}:body"), &body, vault::is_payload(&body))?;
                if sealed != body {
                    tx.execute("UPDATE answers SET body = ?2 WHERE key = ?1", params![key, sealed])?;
                    changed = true;
                }
            }
            if changed { vault::mark_journal_cleanup(root).map_err(private_error)?; }
            Ok(changed)
        })
    }

    /// Old plaintext must not remain in our live SQLite freelist or WAL after conversion.
    /// This cannot erase historical external copies or SSD remnants.
    pub(crate) fn cleanup_private_journal(&self) -> Result<()> {
        let writer = self.writer.lock().expect("store writer");
        let _reader = self.reader.lock().expect("store reader");
        writer.pragma_update(None, "secure_delete", "ON")?;
        let checkpoint = |conn: &Connection| -> Result<()> {
            let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
            if busy != 0 {
                return Err(StoreError::Protected("journal cleanup is waiting for SQLite readers".into()));
            }
            Ok(())
        };
        checkpoint(&writer)?;
        writer.execute_batch("VACUUM;")?;
        checkpoint(&writer)
    }

    /// What the write under `key` answered, if it was kept.
    pub(crate) fn answer(&self, key: &str) -> Result<Option<Answer>> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT digest, status, content_type, body FROM answers WHERE key = ?1",
                    [key],
                    |r| {
                        Ok(Answer {
                            digest: r.get(0)?,
                            status: r.get(1)?,
                            content_type: r.get(2)?,
                            body: r.get(3)?,
                        })
                    },
                )
                .optional()?;
            row.map(|mut answer| {
                answer.body = open_bytes(&format!("answer:{key}:body"), &answer.body)?;
                Ok(answer)
            })
            .transpose()
        })
    }

    /// Keeps what the write under `key` answered; answers older than a day go.
    pub(crate) fn keep_answer(&self, key: &str, answer: &Answer) -> Result<()> {
        let now = now_ms();
        let body = match answer_path(&answer.body) {
            Some(path) => seal_at(&path, &format!("answer:{key}:body"), &answer.body)?,
            None => answer.body.clone(),
        };
        self.write(|tx| {
            tx.execute("DELETE FROM answers WHERE at_ms < ?1", [now - ANSWER_KEPT_MS])?;
            tx.execute(
                "INSERT OR REPLACE INTO answers (key, digest, status, content_type, body, at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key, answer.digest, answer.status, answer.content_type, body, now],
            )?;
            Ok(())
        })
    }

    pub(crate) fn put_machine(&self, machine: &MachineRow) -> Result<()> {
        self.write(|tx| put_machine_in(tx, machine))
    }

    pub(crate) fn machine(&self, name: &str) -> Result<Option<MachineRow>> {
        self.read(|conn| {
            conn.query_row(
                &format!("SELECT {MACHINE_COLUMNS} FROM machines WHERE name = ?1"),
                [name],
                machine_row,
            )
            .optional()?
            .transpose()
        })
    }

    pub(crate) fn machines(&self) -> Result<Vec<MachineRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!("SELECT {MACHINE_COLUMNS} FROM machines ORDER BY name"))?;
            let rows = statement.query_map([], machine_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().collect()
        })
    }

    /// The machines a name, a former name (alias) or a rental id names, in any state; one with
    /// that very name first.
    pub(crate) fn machines_named(&self, text: &str) -> Result<Vec<MachineRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {MACHINE_COLUMNS} FROM machines
                 WHERE name = ?1 COLLATE NOCASE OR alias = ?1 COLLATE NOCASE OR rental_id = ?1 COLLATE NOCASE
                 ORDER BY name = ?1 COLLATE NOCASE DESC, rowid DESC"
            ))?;
            let rows = statement.query_map([text], machine_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().collect()
        })
    }

    /// The name a renamed machine had before.
    pub(crate) fn alias(&self, name: &str) -> Result<Option<String>> {
        self.read(|conn| {
            Ok(conn
                .query_row("SELECT alias FROM machines WHERE name = ?1", [name], |r| r.get(0))
                .optional()?
                .flatten())
        })
    }

    /// Renames a machine that has not served (its Hub named it): its runs move with it, and the
    /// old name stays its alias. An ended machine that held the name gives it up: its history
    /// keeps it as `<name>@<when it was recorded>`.
    pub(crate) fn rename_machine(&self, old: &str, new: &str) -> Result<bool> {
        self.write(|tx| {
            let renamable: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM machines WHERE name = ?1 AND served = 0 AND state <> 'ended')
                     AND NOT EXISTS (SELECT 1 FROM machines WHERE alias = ?2 AND name <> ?1)",
                params![old, new],
                |r| r.get(0),
            )?;
            let holder: Option<(String, i64)> = tx
                .query_row("SELECT state, created_at FROM machines WHERE name = ?1", [new], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            match holder {
                _ if !renamable => return Ok(false),
                Some((state, _)) if state != "ended" => return Ok(false),
                Some((_, at)) => move_machine(tx, new, &format!("{new}@{at}"), None)?,
                None => {}
            }
            move_machine(tx, old, new, Some(old))?;
            Ok(true)
        })
    }

    /// Records a rental another client of the account ordered, under its name; an ended machine that
    /// held the name gives it up (as in `rename_machine`), and a live one keeps it, so this one is
    /// named by its rental id. The row as recorded; none when its rental is recorded already.
    pub(crate) fn adopt(&self, machine: &MachineRow) -> Result<Option<MachineRow>> {
        self.write(|tx| {
            let known: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM machines WHERE rental_id = ?1)",
                [&machine.rental_id],
                |r| r.get(0),
            )?;
            if known {
                return Ok(None);
            }
            let mut row = machine.clone();
            let holder: Option<(String, i64)> = tx
                .query_row("SELECT state, created_at FROM machines WHERE name = ?1", [&row.name], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            match holder {
                Some((state, at)) if state == "ended" => move_machine(tx, &row.name, &format!("{}@{at}", row.name), None)?,
                Some(_) => row.name = row.rental_id.clone().unwrap_or_default().to_ascii_lowercase(),
                None => {}
            }
            put_machine_in(tx, &row)?;
            Ok(Some(row))
        })
    }

    /// Unsettled runs by machine: how many it accepted, and how many wait to be sent or accepted.
    pub(crate) fn machine_load(&self) -> Result<std::collections::HashMap<String, (u32, u32)>> {
        self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT machine, SUM(state = 'accepted'), SUM(state <> 'accepted') FROM runs
                 WHERE state <> 'settled' AND machine IS NOT NULL GROUP BY machine",
            )?;
            let rows = statement.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// How many runs `machine` has been sent and hasn't settled, an update's aside: its work going.
    pub(crate) fn working(&self, machine: &str) -> Result<u32> {
        self.read(|conn| {
            let going = conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE machine = ?1 AND kind <> 'update' AND state IN ('sent', 'accepted')",
                [machine],
                |r| r.get(0),
            )?;
            Ok(going)
        })
    }

    /// Whether an update of `machine` is yet to end: what's sent to it waits for that.
    pub(crate) fn updating(&self, machine: &str) -> Result<bool> {
        self.read(|conn| {
            let held = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM runs WHERE machine = ?1 AND kind = 'update' AND state <> 'settled')",
                [machine],
                |r| r.get(0),
            )?;
            Ok(held)
        })
    }

    /// Unsettled runs no machine was chosen for, oldest first, with where each may go.
    pub(crate) fn unplaced(&self) -> Result<Vec<RunRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE state = 'queued' AND machine IS NULL ORDER BY number"
            ))?;
            let rows = statement.query_map([], run_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            unlocked_rows(rows)
        })
    }

    /// Records an ask of a Hub under its key before it is made; the same key again answers the
    /// recorded ask (`false`), and with another intent (`digest`) is a conflict.
    pub(crate) fn begin_ask(&self, ask: &Ask) -> Result<(Ask, bool)> {
        self.write(|tx| {
            if let Some(recorded) = ask_in(tx, &ask.key)? {
                return if recorded.digest == ask.digest {
                    Ok((recorded, false))
                } else {
                    Err(StoreError::Conflict(ask.key.clone()))
                };
            }
            let body = match &ask.run {
                Some(run) => seal_run(tx, run.as_str(), &format!("ask:{}:body", ask.key), &ask.body)?,
                None => ask.body.clone(),
            };
            tx.execute(
                "INSERT INTO asks (key, hub, digest, body, machine, state, reason, created_at, kind, run)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, ?6, ?7, ?8)",
                params![
                    ask.key,
                    ask.hub,
                    ask.digest,
                    body,
                    ask.machine,
                    now_ms(),
                    ask.kind.as_str(),
                    ask.run.as_ref().map(RunId::as_str),
                ],
            )?;
            Ok((ask.clone(), true))
        })
    }

    /// The rent ask that made the machine.
    pub(crate) fn rent_ask(&self, machine: &str) -> Result<Option<Ask>> {
        self.read(|conn| {
            conn.query_row(
                &format!("SELECT {ASK_COLUMNS} FROM asks WHERE machine = ?1 AND kind = 'rent' AND state = 'done'"),
                [machine],
                ask_row,
            )
            .optional()?
            .transpose()
        })
    }

    /// Asks a failed ask again.
    pub(crate) fn reopen_ask(&self, key: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE asks SET state = 'pending', reason = NULL WHERE key = ?1 AND state = 'failed'",
                [key],
            )?;
            Ok(())
        })
    }

    pub(crate) fn ask(&self, key: &str) -> Result<Option<Ask>> {
        self.read(|conn| ask_in(conn, key))
    }

    /// When the ask under `key` was first made.
    pub(crate) fn ask_made_at(&self, key: &str) -> Result<Option<i64>> {
        self.read(|conn| {
            Ok(conn
                .query_row("SELECT created_at FROM asks WHERE key = ?1", [key], |r| r.get(0))
                .optional()?)
        })
    }

    /// Replaces a pending ask's body: what it is to do, changed by its asker while it waits.
    pub(crate) fn rewrite_ask(&self, key: &str, body: &[u8]) -> Result<()> {
        self.write(|tx| {
            let run: Option<String> = tx
                .query_row("SELECT run FROM asks WHERE key = ?1", [key], |row| row.get(0))
                .optional()?
                .flatten();
            let body = match run {
                Some(run) => seal_run(tx, &run, &format!("ask:{key}:body"), body)?,
                None => body.to_vec(),
            };
            tx.execute("UPDATE asks SET body = ?2 WHERE key = ?1 AND state = 'pending'", params![key, body])?;
            Ok(())
        })
    }

    /// Asks not yet answered: what a starting Calcifer asks again.
    pub(crate) fn pending_asks(&self) -> Result<Vec<Ask>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {ASK_COLUMNS} FROM asks WHERE state = 'pending' ORDER BY created_at"
            ))?;
            let rows = statement.query_map([], ask_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            unlocked_rows(rows)
        })
    }

    /// Ends an ask: `done` with the machine it made, or `failed` with why. A rent ask that is
    /// done records its machine in the same transaction.
    pub(crate) fn settle_ask(&self, key: &str, outcome: std::result::Result<Option<&MachineRow>, &str>) -> Result<()> {
        self.write(|tx| {
            match outcome {
                Ok(machine) => {
                    if let Some(machine) = machine {
                        put_machine_in(tx, machine)?;
                    }
                    tx.execute(
                        "UPDATE asks SET state = 'done', machine = COALESCE(?2, machine) WHERE key = ?1",
                        params![key, machine.map(|m| &m.name)],
                    )?;
                }
                Err(reason) => {
                    let run: Option<String> = tx.query_row("SELECT run FROM asks WHERE key = ?1", [key], |row| row.get(0))?;
                    let reason = match run {
                        Some(run) => seal_run_text(tx, &run, &format!("ask:{key}:reason"), reason)?,
                        None => reason.to_owned(),
                    };
                    tx.execute("UPDATE asks SET state = 'failed', reason = ?2 WHERE key = ?1", params![key, reason])?;
                }
            }
            Ok(())
        })
    }

    /// Records `new` under `key`, once: the same key again answers the recorded run (`false`), and
    /// with another submission (`digest`) is a conflict.
    /// The run recorded under `key`, if one is.
    pub(crate) fn keyed(&self, key: &str) -> Result<Option<RunRow>> {
        self.read(|db| {
            let id = db.query_row("SELECT id FROM runs WHERE idempotency_key = ?1", [key], |r| r.get::<_, String>(0));
            id.optional()?.map_or(Ok(None), |id| run_in(db, &id))
        })
    }

    /// How many runs wait ahead of run `number` for `machine` (none: for any machine to be chosen):
    /// in the outbox, or in that machine's line.
    pub(crate) fn queued_ahead(&self, machine: Option<&str>, number: u64) -> Result<u32> {
        self.read(|db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM runs WHERE machine IS ?1 AND state <> 'settled' AND number < ?2
                     AND (state = 'queued' OR (state = 'accepted' AND machine_state = 'queued'))",
                params![machine, number as i64],
                |r| r.get(0),
            )?)
        })
    }

    /// The run recorded under `key`, and the digest of what it asked.
    pub(crate) fn recorded(&self, key: &str) -> Result<Option<(RunRow, String)>> {
        self.read(|db| {
            let found = db
                .query_row("SELECT id, digest FROM runs WHERE idempotency_key = ?1", [key], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .optional()?;
            let Some((id, digest)) = found else { return Ok(None) };
            Ok(run_in(db, &id)?.map(|row| (row, digest)))
        })
    }

    /// The job's ask Calcifer's own run `id` carries out, when it kept one.
    pub(crate) fn job(&self, id: &str) -> Result<Option<Vec<u8>>> {
        self.read(|db| {
            let job: Option<Vec<u8>> = db.query_row("SELECT job FROM runs WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
            job.map(|job| open_bytes(&run_context(id, "job"), &job)).transpose()
        })
    }

    /// The idempotency key run `id` was submitted under.
    pub(crate) fn key(&self, id: &str) -> Result<Option<String>> {
        self.read(|db| {
            let key = db.query_row("SELECT idempotency_key FROM runs WHERE id = ?1", [id], |r| r.get(0));
            Ok(key.optional()?)
        })
    }

    pub(crate) fn submit(&self, key: &str, digest: &str, new: NewRun) -> Result<(RunRow, bool)> {
        let (id, fresh) = self.write(|tx| {
            let recorded: Option<(String, String)> = tx
                .query_row("SELECT id, digest FROM runs WHERE idempotency_key = ?1", [key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if let Some((id, recorded)) = recorded {
                return if recorded == digest {
                    Ok((id, false))
                } else {
                    Err(StoreError::Conflict(key.into()))
                };
            }
            // The machine's name, as `Machines::named` resolved what the run named.
            let machine = match &new.machine {
                None => None,
                Some(name) => Some(
                    tx.query_row("SELECT name FROM machines WHERE name = ?1", [name], |r| r.get::<_, String>(0))
                        .optional()?
                        .ok_or_else(|| StoreError::UnknownMachine(name.clone()))?,
                ),
            };
            // Calcifer's own run is no machine's to accept: it is going from the row that records it.
            let (state, began) = match new.kind {
                Kind::Here => ("accepted", Some(now_ms())),
                _ => ("queued", None),
            };
            tx.execute(
                "INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec,
                     out_dir, state, created_at, warnings, inputs, wants, agreed, stays, batch, accepted_at, started_at, job)
                 VALUES (?1, (SELECT COALESCE(MAX(number), 0) + 1 FROM runs), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     ?10, ?18, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?19, ?19, ?20)",
                params![
                    new.id.as_str(),
                    key,
                    digest,
                    new.hub.as_deref().unwrap_or_default(),
                    new.kind.as_str(),
                    as_text(seal_at(&new.out_dir, &run_context(new.id.as_str(), "target"), new.target.as_bytes())?)?,
                    machine,
                    new.pinned,
                    seal_at(
                        &new.out_dir,
                        &run_context(new.id.as_str(), "spec"),
                        &serde_json::to_vec(&new.spec).expect("serializes")
                    )?,
                    new.out_dir.to_string_lossy(),
                    now_ms(),
                    as_text(seal_at(
                        &new.out_dir,
                        &run_context(new.id.as_str(), "warnings"),
                        &serde_json::to_vec(&new.warnings).expect("serializes")
                    )?)?,
                    as_text(seal_at(
                        &new.out_dir,
                        &run_context(new.id.as_str(), "inputs"),
                        &serde_json::to_vec(&new.inputs).expect("serializes")
                    )?)?,
                    new.wants,
                    new.agreed.map(|agreed| serde_json::to_string(&agreed).expect("serializes")),
                    serde_json::to_string(&new.stays).expect("serializes"),
                    new.batch,
                    state,
                    began,
                    new.job.as_deref().map(|job| seal_at(&new.out_dir, &run_context(new.id.as_str(), "job"), job)).transpose()?,
                ],
            )?;
            Ok((new.id.to_string(), true))
        })?;
        let row = self.run(&id)?.ok_or_else(|| StoreError::Corrupt(format!("run {id} vanished")))?;
        Ok((row, fresh))
    }

    pub(crate) fn run(&self, id: &str) -> Result<Option<RunRow>> {
        self.read(|conn| run_in(conn, id))
    }

    /// Run `id` and the others of its batch its machine hasn't accepted (queued or sent there), in their order.
    pub(crate) fn batched(&self, id: &str) -> Result<Vec<RunRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1 OR (state IN ('queued', 'sent') AND batch <> ''
                     AND (batch, machine) = (SELECT batch, machine FROM runs WHERE id = ?1)) ORDER BY number"
            ))?;
            let rows = statement.query_map([id], run_row)?;
            rows.map(|row| row?).collect()
        })
    }

    pub(crate) fn run_by_number(&self, number: u64) -> Result<Option<RunRow>> {
        self.read(|conn| {
            conn.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE number = ?1"),
                [number as i64],
                run_row,
            )
            .optional()?
            .transpose()
        })
    }

    /// The runs `filter` names, newest first; how many more match beyond its limit; and each
    /// matching run it cannot read, as `run N: why`, left out.
    pub(crate) fn runs(&self, filter: &RunFilter) -> Result<(Vec<RunRow>, u64, Vec<String>)> {
        // A run of a project adapter, from when there were any, is no one's to read in a list, though `run watch` has it.
        const MATCHING: &str = "FROM runs WHERE number < ?1
            AND target NOT LIKE '%/project-adapter-%'
            AND (?2 IS NULL OR CASE state WHEN 'accepted' THEN 'in_progress' WHEN 'settled' THEN status
                ELSE 'queued' END = ?2)
            AND (?3 IS NULL OR substr(target, 1, length(?3)) = ?3)
            AND (?4 IS NULL OR machine = ?4)
            AND (?5 IS NULL OR out_dir = ?5)";
        self.read(|conn| {
            // Keep classification, counts and the combined page on one read snapshot.
            let snapshot = conn.unchecked_transaction()?;
            let conn = &snapshot;
            let status = filter.status.map(|status| match status {
                Status::Queued => "queued",
                Status::InProgress => "in_progress",
                Status::Completed => "completed",
                Status::Failed => "failed",
                Status::Canceled => "canceled",
            });
            let before = filter.before.map_or(i64::MAX, |n| n as i64);
            let private_target = private_run_target();
            let private: bool = conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 {RUN_LIST_SCOPE} AND {private_target})"),
                params![before, status, filter.machine, filter.out_dir],
                |row| row.get(0),
            )?;
            if private {
                return runs_with_private_targets(conn, filter, before, status, &private_target);
            }
            let matching = params![before, status, filter.target, filter.machine, filter.out_dir];
            let total: i64 = conn.query_row(&format!("SELECT COUNT(*) {MATCHING}"), matching, |r| r.get(0))?;
            let mut statement = conn.prepare(&format!("SELECT {RUN_COLUMNS} {MATCHING} ORDER BY number DESC LIMIT ?6"))?;
            let read = statement.query_map(
                params![before, status, filter.target, filter.machine, filter.out_dir, filter.limit as i64],
                |r| Ok((r.get::<_, i64>(1)?, run_row(r))),
            )?;
            let (mut rows, mut unreadable) = (vec![], vec![]);
            for item in read {
                let (number, row) = item?;
                match row.map_err(StoreError::from).and_then(|row| row) {
                    Ok(row) => rows.push(row),
                    Err(e) => unreadable.push(format!("run {number}: {e}")),
                }
            }
            let older = (total as u64).saturating_sub((rows.len() + unreadable.len()) as u64);
            Ok((rows, older, unreadable))
        })
    }

    /// The machines' runs not yet settled, oldest first: what a starting Calcifer drives.
    pub(crate) fn unsettled(&self) -> Result<Vec<RunId>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT id FROM runs WHERE state <> 'settled' AND kind <> 'here' ORDER BY number")?;
            let ids = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter()
                .map(|id| RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into())))
                .collect()
        })
    }

    /// Calcifer's own runs not yet settled: those an earlier Calcifer stopped under, or that are going now.
    pub(crate) fn unsettled_here(&self) -> Result<Vec<RunId>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT id FROM runs WHERE state <> 'settled' AND kind = 'here' ORDER BY number")?;
            let ids = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter()
                .map(|id| RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into())))
                .collect()
        })
    }

    /// Whether any unsettled run is written the staged object `digest`.
    pub(crate) fn input_in_use(&self, digest: &Digest) -> Result<bool> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT id, inputs FROM runs WHERE state <> 'settled'")?;
            let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, text) = row?;
                let plain = match open_text(&run_context(&id, "inputs"), &text) {
                    Ok(plain) => plain,
                    Err(StoreError::Locked(_)) => return Ok(true),
                    Err(error) => return Err(error),
                };
                let inputs: Vec<(Digest, u64)> = serde_json::from_str(&plain).map_err(|_| corrupt("a run's inputs"))?;
                if inputs.iter().any(|(held, _)| held == digest) {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// The run with what `?detail=full` adds: its measurements and its settled calls.
    pub(crate) fn detail(&self, row: &RunRow) -> Result<Run> {
        self.read(|conn| {
            let mut run = record(conn, row)?;
            run.measurements = row
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.measurements.as_ref())
                .and_then(|measurements| serde_json::from_str(measurements.get()).ok());
            let mut statement = conn.prepare("SELECT seq, text FROM run_logs WHERE run = ?1 AND level = 'call' ORDER BY seq")?;
            let calls = statement.query_map([row.id.as_str()], |r| {
                sql_text(r, 1, &log_context(row.id.as_str(), r.get::<_, i64>(0)? as u64))
            })?;
            for call in calls {
                run.calls.extend(serde_json::from_str::<Call>(&call?).ok());
            }
            Ok(run)
        })
    }

    /// Drops the measurements of the next runs past number `after` that settled before `before_ms`,
    /// in one short write. The number to go on from; None: no run past `after` is that old.
    pub(crate) fn drop_measurements(&self, after: i64, before_ms: i64) -> Result<Option<i64>> {
        self.write(|tx| {
            let mut next = tx.prepare("SELECT number, created_at FROM runs WHERE number > ?1 ORDER BY number LIMIT 64")?;
            let next = next
                .query_map([after], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            // A run that settled before then was created before then, and numbers go up with time.
            let Some(through) = next.iter().take_while(|(_, created)| *created < before_ms).last().map(|(n, _)| *n) else {
                return Ok(None);
            };
            let ids: Vec<String> = {
                let mut rows = tx.prepare("SELECT id FROM runs WHERE number > ?1 AND number <= ?2 AND settled_at < ?3")?;
                rows.query_map(params![after, through, before_ms], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for id in ids {
                let outcome: Option<Vec<u8>> = tx.query_row("SELECT outcome FROM runs WHERE id = ?1", [&id], |row| row.get(0))?;
                if let Some(bytes) = outcome {
                    if let Some(plain) = vault::open_payload(&run_context(&id, "outcome"), &bytes).map_err(private_error)? {
                        if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&plain) {
                            if value.as_object_mut().is_some_and(|value| value.remove("measurements").is_some()) {
                                let bytes = seal_run(
                                    tx,
                                    &id,
                                    &run_context(&id, "outcome"),
                                    &serde_json::to_vec(&value).expect("serializes"),
                                )?;
                                tx.execute("UPDATE runs SET outcome = ?2 WHERE id = ?1", params![id, bytes])?;
                            }
                        }
                    }
                }
                let calls: Vec<(i64, String)> = {
                    let mut rows = tx.prepare("SELECT seq, text FROM run_logs WHERE run = ?1 AND level = 'call'")?;
                    rows.query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for (seq, text) in calls {
                    let context = log_context(&id, seq as u64);
                    if let Some(plain) = vault::open_payload(&context, text.as_bytes()).map_err(private_error)? {
                        if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&plain) {
                            if value.as_object_mut().is_some_and(|value| value.remove("measurements").is_some()) {
                                let text = seal_run_text(tx, &id, &context, &value.to_string())?;
                                tx.execute("UPDATE run_logs SET text = ?3 WHERE run = ?1 AND seq = ?2", params![id, seq, text])?;
                            }
                        }
                    }
                }
            }
            Ok(Some(through))
        })
    }

    /// Places an unplaced run on `machine`.
    pub(crate) fn place(&self, id: &str, machine: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE runs SET machine = ?2 WHERE id = ?1 AND machine IS NULL AND state = 'queued'",
                params![id, machine],
            )? == 1)
        })
    }

    /// Returns an unsent `any` run to the outbox, to be placed again.
    pub(crate) fn unplace(&self, id: &str) -> Result<Vec<Event>> {
        self.write(|tx| {
            let unplaced = tx.execute(
                "UPDATE runs SET machine = NULL WHERE id = ?1 AND state = 'queued' AND wants = 'any'",
                [id],
            )?;
            Ok(match unplaced {
                1 => vec![lifecycle(tx, id)?],
                _ => vec![],
            })
        })
    }

    /// The send boundary: the run may reach `machine` only if this wins against a cancel.
    pub(crate) fn mark_sent(&self, id: &str, machine: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE runs SET state = 'sent'
                 WHERE id = ?1 AND state = 'queued' AND machine = ?2 AND cancel_requested = 0",
                params![id, machine],
            )? == 1)
        })
    }

    /// Records `machine`'s first `RunState` of the run.
    pub(crate) fn accept(&self, id: &str, machine: &str, state: &v1::RunState) -> Result<Acceptance> {
        self.write(|tx| {
            let row = run_in(tx, id)?.ok_or_else(|| StoreError::Corrupt(format!("no run {id}")))?;
            if row.machine.as_deref() != Some(machine) || row.state == State::Queued {
                return Ok(Acceptance::Stale);
            }
            if row.state != State::Sent {
                return Ok(if row.state == State::Accepted {
                    Acceptance::Again
                } else {
                    Acceptance::Stale
                });
            }
            tx.execute(
                "UPDATE runs SET state = 'accepted', machine_number = ?2, machine_state = ?3, accepted_at = ?4,
                 started_at = ?5 WHERE id = ?1",
                params![
                    id,
                    state.number as i64,
                    state.state,
                    now_ms(),
                    (state.started_at_ms > 0).then_some(state.started_at_ms as i64)
                ],
            )?;
            Ok(Acceptance::Fresh(vec![lifecycle(tx, id)?]))
        })
    }

    /// Records a batch of an accepted run's frames and moves its cursor past them.
    pub(crate) fn observe(&self, id: &str, batch: Batch) -> Result<Vec<Event>> {
        self.write(|tx| {
            let Some(row) = run_in(tx, id)?.filter(|row| row.state == State::Accepted) else {
                return Ok(vec![]);
            };
            let mut events = vec![];
            for (at_ms, level, text) in batch.logs {
                let seq = next_seq(tx, id)?;
                tx.execute(
                    "INSERT INTO run_logs (run, seq, at_ms, level, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, seq as i64, at_ms, level, seal_run_text(tx, id, &log_context(id, seq), &text)?],
                )?;
                events.push(log_event(seq, at_ms, level, text));
            }
            for product in &batch.products {
                events.extend(put_output(tx, &row, product)?);
            }
            if let Some(at) = batch.started_at_ms.filter(|_| row.started_at_ms.is_none()) {
                tx.execute("UPDATE runs SET started_at = ?2 WHERE id = ?1", params![id, at])?;
            }
            if let Some(state) = batch.machine_state.filter(|s| row.machine_state.as_ref() != Some(s)) {
                tx.execute("UPDATE runs SET machine_state = ?2 WHERE id = ?1", params![id, state])?;
                events.push(lifecycle(tx, id)?);
            }
            tx.execute(
                "UPDATE runs SET cursor = MAX(cursor, ?2) WHERE id = ?1",
                params![id, batch.cursor as i64],
            )?;
            events.sort_by_key(Event::sequence_number);
            Ok(events)
        })
    }

    /// A line of Calcifer's own in the run's log, whatever its machine has said yet (the inputs it
    /// reused); none once it settled.
    pub(crate) fn note(&self, id: &str, level: &str, text: &str) -> Result<Vec<Event>> {
        self.write(|tx| {
            if run_in(tx, id)?.is_none_or(|row| row.state == State::Settled) {
                return Ok(vec![]);
            }
            let (seq, at_ms) = (next_seq(tx, id)?, now_ms());
            tx.execute(
                "INSERT INTO run_logs (run, seq, at_ms, level, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, seq as i64, at_ms, level, seal_run_text(tx, id, &log_context(id, seq), &text)?],
            )?;
            Ok(vec![log_event(seq, at_ms, level.to_owned(), text.to_owned())])
        })
    }

    /// Ends an accepted run with its machine's outcome.
    pub(crate) fn settle(&self, id: &str, outcome: &v1::Outcome, cursor: u64) -> Result<Vec<Event>> {
        self.write(|tx| {
            let Some(row) = run_in(tx, id)?.filter(|row| row.state == State::Accepted) else {
                return Ok(vec![]);
            };
            let mut events = vec![];
            for product in &outcome.outputs {
                events.extend(put_output(tx, &row, product)?);
            }
            let status = match outcome.status.as_str() {
                "succeeded" => Status::Completed,
                "canceled" => Status::Canceled,
                _ => Status::Failed,
            };
            let reason = outcome.reason.as_ref().filter(|_| status != Status::Completed).cloned();
            tx.execute(
                "UPDATE runs SET cursor = MAX(cursor, ?2), outcome = ?3 WHERE id = ?1",
                params![
                    id,
                    cursor as i64,
                    seal_run(
                        tx,
                        id,
                        &run_context(id, "outcome"),
                        &serde_json::to_vec(outcome).expect("serializes")
                    )?
                ],
            )?;
            events.extend(end(tx, id, status, reason)?);
            Ok(events)
        })
    }

    /// Settles a run its machine answered it has no record of: canceled when that was asked, else
    /// failed. It is never sent again, since the machine may have run part of it.
    pub(crate) fn lost(&self, id: &str, machine: &str) -> Result<Vec<Event>> {
        self.write(|tx| match run_in(tx, id)? {
            Some(row) if row.state == State::Settled => Ok(vec![]),
            Some(row) if row.cancel_requested => {
                let message = format!("canceled; {machine} has no record of the run");
                end(
                    tx,
                    id,
                    Status::Canceled,
                    Some(Reason {
                        code: "canceled".into(),
                        message,
                        ..Default::default()
                    }),
                )
            }
            Some(_) => {
                let message = format!("{machine} has no record of the run, and the run was not retried");
                end(
                    tx,
                    id,
                    Status::Failed,
                    Some(Reason {
                        code: "machine_lost_run".into(),
                        message,
                        ..Default::default()
                    }),
                )
            }
            None => Ok(vec![]),
        })
    }

    /// Settles a canceled run its machine keeps answering an internal failure about. Anything else stays unsettled: only its
    /// machine's outcome settles it.
    pub(crate) fn cancel_unanswered(&self, id: &str, machine: &str, code: &str, message: &str) -> Result<Vec<Event>> {
        self.write(|tx| match run_in(tx, id)? {
            Some(row) if row.state != State::Settled && row.cancel_requested => {
                let message = format!("canceled, though {machine} wouldn't answer: {code}: {message}");
                end(
                    tx,
                    id,
                    Status::Canceled,
                    Some(Reason {
                        code: "canceled".into(),
                        message,
                        ..Default::default()
                    }),
                )
            }
            _ => Ok(vec![]),
        })
    }

    /// Ends a run its machine never accepted, with `reason`.
    pub(crate) fn fail(&self, id: &str, reason: Reason) -> Result<Vec<Event>> {
        self.write(|tx| match run_in(tx, id)? {
            Some(row) if matches!(row.state, State::Queued | State::Sent) => end(tx, id, Status::Failed, Some(reason)),
            _ => Ok(vec![]),
        })
    }

    /// Adds a warning to an unsettled run, answered as its lifecycle event.
    pub(crate) fn warn(&self, id: &str, warning: &Warning) -> Result<Vec<Event>> {
        self.warning(id, warning, false)
    }

    /// `warn`, of a run that may have ended: what comes of its outputs lands after it does.
    pub(crate) fn warned(&self, id: &str, warning: &Warning) -> Result<Vec<Event>> {
        self.warning(id, warning, true)
    }

    fn warning(&self, id: &str, warning: &Warning, settled_too: bool) -> Result<Vec<Event>> {
        self.write(|tx| {
            let Some(mut row) = run_in(tx, id)?.filter(|row| settled_too || row.state != State::Settled) else {
                return Ok(vec![]);
            };
            row.warnings.push(warning.clone());
            let warnings = serde_json::to_string(&row.warnings).expect("serializes");
            tx.execute(
                "UPDATE runs SET warnings = ?2 WHERE id = ?1",
                params![id, seal_run_text(tx, id, &run_context(id, "warnings"), &warnings)?],
            )?;
            Ok(vec![lifecycle(tx, id)?])
        })
    }

    /// Records the intent to cancel. Unsent work ends canceled here, in the transaction that the
    /// send boundary races; sent work keeps the intent until its machine's outcome arrives.
    pub(crate) fn request_cancel(&self, id: &str) -> Result<Vec<Event>> {
        self.write(|tx| {
            let Some(row) = run_in(tx, id)? else {
                return Ok(vec![]);
            };
            match row.state {
                State::Settled => Ok(vec![]),
                _ if row.cancel_requested => Ok(vec![]),
                State::Queued => {
                    tx.execute("UPDATE runs SET cancel_requested = 1 WHERE id = ?1", [id])?;
                    let reason = Reason {
                        code: "canceled".into(),
                        message: "canceled before it was sent".into(),
                        ..Default::default()
                    };
                    end(tx, id, Status::Canceled, Some(reason))
                }
                State::Sent | State::Accepted => {
                    tx.execute("UPDATE runs SET cancel_requested = 1 WHERE id = ?1", [id])?;
                    Ok(vec![lifecycle(tx, id)?])
                }
            }
        })
    }

    /// Records `machine` as gone for good and settles what it owed: a run it accepted fails (or
    /// ends canceled when that was asked) and is never run again; one it never confirmed goes back
    /// to the outbox under the same id when Calcifer placed it and the machine had served, and
    /// otherwise fails. Answers the events, by run.
    pub(crate) fn machine_ended(&self, machine: &str, reason: &str) -> Result<Vec<(RunId, Event)>> {
        self.write(|tx| {
            let served: Option<bool> = tx
                .query_row("SELECT served FROM machines WHERE name = ?1", [machine], |r| r.get(0))
                .optional()?;
            let Some(served) = served else { return Err(StoreError::UnknownMachine(machine.into())) };
            tx.execute("UPDATE machines SET state = 'ended', reason = ?2 WHERE name = ?1", params![machine, reason])?;
            let mut statement = tx.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE machine = ?1 AND state <> 'settled' ORDER BY number"
            ))?;
            let rows = statement.query_map([machine], run_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut events = vec![];
            for row in rows {
                let row = row?;
                let id = row.id.to_string();
                let ended = if row.cancel_requested {
                    let reason = Reason { code: "canceled".into(), message: format!("canceled; {reason}"), ..Default::default() };
                    end(tx, &id, Status::Canceled, Some(reason))?
                } else if row.state != State::Accepted && !row.pinned && served {
                    tx.execute(
                        "UPDATE runs SET machine = NULL, state = 'queued', cursor = 0, machine_state = NULL
                         WHERE id = ?1",
                        [&id],
                    )?;
                    vec![lifecycle(tx, &id)?]
                } else {
                    let message = match row.state {
                        State::Accepted if reason == "released_by_pod" => format!("{machine} accepted the run, but its result never reached Calcifer, and {machine} released itself after 15 idle minutes: the result is gone"),
                        State::Accepted => format!("its machine {machine} ended mid-run, and the run was not retried: {reason}"),
                        _ => format!("its machine {machine} ended before it accepted the run: {reason}"),
                    };
                    end(tx, &id, Status::Failed, Some(Reason { code: "machine_lost".into(), message, ..Default::default() }))?
                };
                events.extend(ended.into_iter().map(|event| (row.id.clone(), event)));
            }
            Ok(events)
        })
    }

    /// The run as the API answers it.
    pub(crate) fn record(&self, row: &RunRow) -> Result<Run> {
        self.read(|conn| record(conn, row))
    }

    /// The run's durable events after `after`, as a reconnecting stream gets them: its current
    /// lifecycle event, each output item changed since at its newest revision, and its log lines.
    pub(crate) fn events_after(&self, id: &str, after: u64) -> Result<Vec<Event>> {
        self.read(|conn| {
            let Some(row) = run_in(conn, id)? else { return Ok(vec![]) };
            let run = record(conn, &row)?;
            let mut events = vec![];
            for (index, item) in run.output.iter().enumerate() {
                let (created, seq): (i64, i64) = conn.query_row(
                    "SELECT created_seq, seq FROM outputs WHERE run = ?1 AND name = ?2 AND idx = ?3",
                    params![id, item.name, item.index],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let (created, seq, output_index) = (created as u64, seq as u64, index as u32);
                if seq <= after {
                    continue;
                }
                events.push(if item.path.is_some() {
                    Event::OutputLanded {
                        sequence_number: seq,
                        output_index,
                        item: item.clone(),
                    }
                } else if item.status != ItemStatus::InProgress {
                    Event::OutputDone {
                        sequence_number: seq,
                        output_index,
                        item: item.clone(),
                    }
                } else if created > after {
                    Event::OutputAdded {
                        sequence_number: seq,
                        output_index,
                        item: item.clone(),
                    }
                } else {
                    Event::OutputDelta {
                        sequence_number: seq,
                        output_index,
                        item_id: item.id.clone(),
                        rev: item.rev,
                        length: item.length,
                        appended_from: None,
                        checkpoint: item.checkpoint.clone(),
                    }
                });
            }
            let mut statement = conn.prepare("SELECT seq, at_ms, level, text FROM run_logs WHERE run = ?1 AND seq > ?2 ORDER BY seq")?;
            let logs = statement.query_map(params![id, after as i64], |r| {
                Ok(log_event(
                    r.get::<_, i64>(0)? as u64,
                    r.get(1)?,
                    r.get(2)?,
                    sql_text(r, 3, &log_context(id, r.get::<_, i64>(0)? as u64))?,
                ))
            })?;
            for log in logs {
                events.push(log?);
            }
            if row.state_seq > after {
                events.push(Event::lifecycle(row.state_seq, run, row.state_seq == 1));
            }
            events.sort_by_key(Event::sequence_number);
            Ok(events)
        })
    }

    /// The runs, by id and number, with an output whose file lacks its newest revision that
    /// Calcifer has not given up.
    pub(crate) fn exports_owed(&self) -> Result<Vec<(RunId, u64)>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT DISTINCT runs.id, runs.number FROM outputs JOIN runs ON runs.id = outputs.run
                 WHERE {OWED} ORDER BY runs.number"
            ))?;
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(id, number)| Ok((RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into()))?, number)))
                .collect()
        })
    }

    /// The outputs of runs on `machine` whose file lacks its newest revision and that Calcifer has
    /// not given up, by run and appearance.
    pub(crate) fn exports_owed_on(&self, machine: &str) -> Result<Vec<OwedExport>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT runs.id, runs.number, outputs.name, outputs.idx, outputs.length
                 FROM outputs JOIN runs ON runs.id = outputs.run
                 WHERE runs.machine = ?1 AND {OWED}
                 ORDER BY runs.number, outputs.created_seq"
            ))?;
            let rows = statement
                .query_map([machine], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)? as u64,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)? as u32,
                        r.get::<_, i64>(4)? as u64,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(id, number, name, index, length)| {
                    let run = RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into()))?;
                    Ok(OwedExport {
                        run,
                        number,
                        name,
                        index,
                        length,
                    })
                })
                .collect()
        })
    }

    /// The run's outputs to export, in the order they first appeared.
    pub(crate) fn outputs_to_export(&self, run: &str) -> Result<Vec<OutputRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {OUTPUT_COLUMNS} FROM outputs WHERE run = ?1 AND {OWED} ORDER BY created_seq"
            ))?;
            Ok(statement.query_map([run], output_row)?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The newest run on `machine` that made these bytes, with its output: what can read them from there.
    pub(crate) fn making(&self, digest: &str, machine: &str) -> Result<Option<(RunRow, OutputRow)>> {
        let found: Option<(String, String, i64)> = self.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT outputs.run, outputs.name, outputs.idx FROM outputs JOIN runs ON runs.id = outputs.run
                     WHERE outputs.digest = ?1 AND outputs.done AND runs.machine = ?2 ORDER BY runs.number DESC LIMIT 1",
                    params![digest, machine],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?)
        })?;
        let Some((run, name, index)) = found else { return Ok(None) };
        Ok(self.run(&run)?.zip(self.output(&run, &name, index as u32)?))
    }

    pub(crate) fn output(&self, run: &str, name: &str, index: u32) -> Result<Option<OutputRow>> {
        self.read(|conn| {
            Ok(conn
                .query_row(
                    &format!("SELECT {OUTPUT_COLUMNS} FROM outputs WHERE run = ?1 AND name = ?2 AND idx = ?3"),
                    params![run, name, index],
                    output_row,
                )
                .optional()?)
        })
    }

    /// Records the output's file at `path` as holding revision `rev`, and what was measured of it; while that is its current
    /// revision, the run's next event says it landed.
    pub(crate) fn exported(&self, run: &str, name: &str, index: u32, rev: u64, path: &Path, measured: Measured) -> Result<Vec<Event>> {
        self.write(|tx| {
            tx.execute(
                "UPDATE outputs SET exported_rev = ?4, path = ?5, width = ?6, height = ?7, measured_ms = ?8 WHERE run = ?1 AND name = ?2 AND idx = ?3",
                params![run, name, index, rev as i64, seal_run_text(tx, run, &output_context(run, name, index, rev, "path"), &vault::logical(path).to_string_lossy())?, measured.width, measured.height, measured.duration_ms as i64],
            )?;
            export_settled(tx, run, name, index, rev, true)
        })
    }

    /// Records why revision `rev` of the output will not be exported; while that is its current
    /// revision, the run's next event is the item done again, carrying why.
    pub(crate) fn export_failed(&self, run: &str, name: &str, index: u32, rev: u64, reason: &str) -> Result<Vec<Event>> {
        self.write(|tx| {
            tx.execute(
                "UPDATE outputs SET failed_rev = ?4, export_error = ?5 WHERE run = ?1 AND name = ?2 AND idx = ?3",
                params![
                    run,
                    name,
                    index,
                    rev as i64,
                    seal_run_text(tx, run, &output_context(run, name, index, rev, "export_error"), reason)?
                ],
            )?;
            export_settled(tx, run, name, index, rev, false)
        })
    }
}

/// The event of an output's export at `rev` settling, if that is its current revision.
fn export_settled(tx: &Transaction, run: &str, name: &str, index: u32, rev: u64, landed: bool) -> Result<Vec<Event>> {
    let current: Option<i64> = tx
        .query_row(
            "SELECT rev FROM outputs WHERE run = ?1 AND name = ?2 AND idx = ?3",
            params![run, name, index],
            |r| r.get(0),
        )
        .optional()?;
    if current != Some(rev as i64) {
        return Ok(vec![]);
    }
    let seq = next_seq(tx, run)?;
    tx.execute(
        "UPDATE outputs SET seq = ?4 WHERE run = ?1 AND name = ?2 AND idx = ?3",
        params![run, name, index, seq as i64],
    )?;
    let row = run_in(tx, run)?.ok_or_else(|| StoreError::Corrupt(format!("no run {run}")))?;
    let items = record(tx, &row)?.output.into_iter().enumerate();
    let Some((output_index, item)) = items.into_iter().find(|(_, i)| i.name == name && i.index == index) else {
        return Ok(vec![]);
    };
    let (sequence_number, output_index) = (seq, output_index as u32);
    Ok(vec![match landed {
        true => Event::OutputLanded {
            sequence_number,
            output_index,
            item,
        },
        false => Event::OutputDone {
            sequence_number,
            output_index,
            item,
        },
    }])
}

/// Whether an output stays on its machine (`OutputItem::stays`): its run's result declares it `Latents`, by its name.
const STAYS: &str =
    "EXISTS (SELECT 1 FROM runs held, json_each(held.stays) WHERE held.id = outputs.run AND json_each.value = outputs.name)";

/// An output owed to this computer: its newest revision neither landed nor given up, no checkpoint, and none that stays.
const OWED: &str = "outputs.exported_rev IS NOT outputs.rev AND outputs.failed_rev IS NOT outputs.rev AND outputs.checkpoint IS NULL
    AND NOT EXISTS (SELECT 1 FROM runs held, json_each(held.stays) WHERE held.id = outputs.run AND json_each.value = outputs.name)";

const OUTPUT_COLUMNS: &str = "name, idx, rev, length, digest, media_type, done, path, exported_rev,
    CASE WHEN failed_rev = rev THEN export_error END, checkpoint, run";

fn checkpoint_ref(row: &Row, column: usize, context: &str) -> rusqlite::Result<Option<v1::CheckpointRef>> {
    sql_optional_text(row, column, context)?
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error)))
        })
        .transpose()
}

fn output_row(r: &Row) -> rusqlite::Result<OutputRow> {
    let (run, name, index, rev): (String, String, u32, u64) =
        (r.get(11)?, r.get(0)?, r.get::<_, i64>(1)? as u32, r.get::<_, i64>(2)? as u64);
    let exported = r.get::<_, Option<i64>>(8)?.map(|rev| rev as u64);
    let path = sql_optional_path(r, 7, &output_context(&run, &name, index, exported.unwrap_or(rev), "path"))?;
    let export_error = sql_optional_text(r, 9, &output_context(&run, &name, index, rev, "export_error"))?;
    let checkpoint = checkpoint_ref(r, 10, &output_context(&run, &name, index, rev, "checkpoint"))?;
    Ok(OutputRow {
        name,
        index,
        rev,
        length: r.get::<_, i64>(3)? as u64,
        digest: r.get(4)?,
        media_type: r.get(5)?,
        done: r.get(6)?,
        path,
        exported_rev: exported,
        export_error,
        checkpoint,
    })
}

fn put_machine_in(tx: &Transaction, machine: &MachineRow) -> Result<()> {
    let (worker, address, pin) = match &machine.endpoint {
        Some(e) => (Some(&e.worker_id), Some(&e.address), Some(e.pin.to_vec())),
        None => (None, None, None),
    };
    tx.execute(
        "INSERT INTO machines (name, kind, hub, rental_id, worker_id, address, pin, key, state,
             served, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (name) DO UPDATE SET kind = ?2, hub = ?3, rental_id = ?4, worker_id = ?5,
             address = ?6, pin = ?7, key = ?8, state = ?9, served = served OR ?10, reason = ?11",
        params![
            machine.name,
            machine_kind(machine.kind),
            machine.hub,
            machine.rental_id,
            worker,
            address,
            pin,
            machine.key.to_string_lossy(),
            machine_state(machine.state),
            machine.served || machine.endpoint.is_some(),
            machine.reason,
            now_ms(),
        ],
    )?;
    Ok(())
}

const RUN_COLUMNS: &str = "id, number, kind, target, hub, machine, pinned, spec, out_dir, state, cancel_requested,
    cursor, machine_state, status, reason_code, reason, state_seq, created_at, accepted_at, settled_at, warnings,
    inputs, outcome, wants, agreed, started_at";

const RUN_LIST_SCOPE: &str = "FROM runs WHERE number < ?1
    AND (?2 IS NULL OR CASE state WHEN 'accepted' THEN 'in_progress' WHEN 'settled' THEN status ELSE 'queued' END = ?2)
    AND (?3 IS NULL OR machine = ?3) AND (?4 IS NULL OR out_dir = ?4)";
// Spec also classifies the old sealed-spec/plaintext-target representation. Otherwise a
// locked legacy row could reveal its target through prefix-filter membership.
// Literals let SQLite match the expression and partial indexes; bind parameters cannot.
fn private_run_target() -> String {
    let length = vault::PAYLOAD_PREFIX.len();
    let prefix = hex::encode(vault::PAYLOAD_PREFIX);
    format!("(substr(CAST(target AS BLOB),1,{length})=X'{prefix}' OR substr(spec,1,{length})=X'{prefix}')")
}

fn run_list_indexes(conn: &Connection) -> Result<()> {
    let private = private_run_target();
    conn.execute_batch(&format!(
        "CREATE INDEX IF NOT EXISTS runs_private_targets ON runs(number) WHERE {private};
         CREATE INDEX IF NOT EXISTS runs_list_metadata ON runs(number,target,state,status,machine,out_dir,id,{private});"
    ))?;
    Ok(())
}

/// Only private target slots need opening to choose a page. Ordinary history keeps its
/// SQL prefix/count/limit path; full specs are decoded only for the requested merged page.
fn runs_with_private_targets(conn: &Connection, filter: &RunFilter, before: i64, status: Option<&str>, private_target: &str) -> Result<(Vec<RunRow>, u64, Vec<String>)> {
    let prefix = vault::PAYLOAD_PREFIX;
    let length = prefix.len() as i64;
    let limit = filter.limit.min(i64::MAX as u64) as i64;
    // = 0 preserves NOT's NULL result for an empty legacy spec, while SQLite can read
    // the indexed OR expression instead of rewriting NOT into two blob reads.
    let public = format!("{RUN_LIST_SCOPE} AND {private_target} = 0
        AND target NOT LIKE '%/project-adapter-%' AND (?5 IS NULL OR substr(target, 1, length(?5)) = ?5)");
    let public_count: u64 = conn.query_row(
        &format!("SELECT COUNT(*) {public}"),
        params![before, status, filter.machine, filter.out_dir, filter.target],
        |row| Ok(row.get::<_, i64>(0)? as u64),
    )?;
    let mut candidates: Vec<(u64, String)> = {
        let mut statement = conn.prepare(&format!("SELECT number, id {public} ORDER BY number DESC LIMIT ?6"))?;
        statement.query_map(params![before, status, filter.machine, filter.out_dir, filter.target, limit], |row| {
            Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
        })?.collect::<rusqlite::Result<_>>()?
    };
    let mut private_count = 0u64;
    let mut private_page = 0i64;
    let mut statement = conn.prepare(&format!(
        "SELECT number, id, target, CASE WHEN substr(CAST(target AS BLOB), 1, ?5) = ?6 THEN NULL ELSE spec END
         {RUN_LIST_SCOPE} AND {private_target} ORDER BY number DESC"
    ))?;
    let targets = statement.query_map(params![before, status, filter.machine, filter.out_dir, length, prefix], |row| {
        Ok((row.get::<_, i64>(0)? as u64, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<Vec<u8>>>(3)?))
    })?;
    for target in targets {
        let (number, id, target, legacy_spec) = target?;
        let opened = (|| {
            if let Some(spec) = legacy_spec {
                // A legacy target is plaintext, but it still belongs to the protected spec.
                // Do not use its bytes for matching unless that journal is accessible.
                open_bytes(&run_context(&id, "spec"), &spec)?;
            }
            open_text(&run_context(&id, "target"), &target)
        })();
        let matching = match opened {
            Ok(target) => !target.to_ascii_lowercase().contains("/project-adapter-") && filter.target.as_ref().is_none_or(|prefix| target.starts_with(prefix)),
            // An unfiltered list may report opaque unreadable run numbers, as before. A
            // target-filtered list must reveal no matching/nonmatching fact while locked.
            Err(_) => filter.target.is_none(),
        };
        if matching {
            private_count += 1;
            if private_page < limit {
                candidates.push((number, id));
                private_page += 1;
            }
        }
    }
    candidates.sort_unstable_by_key(|(number, _)| std::cmp::Reverse(*number));
    candidates.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let older = public_count.saturating_add(private_count).saturating_sub(candidates.len() as u64);
    let (mut rows, mut unreadable) = (Vec::new(), Vec::new());
    for (number, id) in candidates {
        match run_in(conn, &id) {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => return Err(corrupt("a listed run vanished from its read snapshot")),
            Err(error) => unreadable.push(format!("run {number}: {error}")),
        }
    }
    Ok((rows, older, unreadable))
}

fn run_row(r: &Row) -> rusqlite::Result<Result<RunRow>> {
    let id: String = r.get(0)?;
    let target: String = r.get(3)?;
    let kind: String = r.get(2)?;
    let hub: String = r.get(4)?;
    let state: String = r.get(9)?;
    let status: Option<String> = r.get(13)?;
    let reason_code: Option<String> = r.get(14)?;
    let reason: Option<String> = r.get(15)?;
    let out_dir: String = r.get(8)?;
    let warnings: String = r.get(20)?;
    let inputs: String = r.get(21)?;
    let spec: Vec<u8> = r.get(7)?;
    let outcome: Option<Vec<u8>> = r.get(22)?;
    let wants: Option<String> = r.get(23)?;
    let agreed: Option<String> = r.get(24)?;
    Ok((|| {
        let out_dir = match vault::payload_root(&spec).map_err(private_error)? { Some(root) => crate::project::landing(&root), None => PathBuf::from(out_dir) };
        let spec = open_bytes(&run_context(&id, "spec"), &spec)?;
        let target = open_text(&run_context(&id, "target"), &target)?;
        let warnings = open_text(&run_context(&id, "warnings"), &warnings)?;
        let inputs = open_text(&run_context(&id, "inputs"), &inputs)?;
        let reason_code = optional_text(&run_context(&id, "reason_code"), reason_code)?;
        let reason = optional_text(&run_context(&id, "reason"), reason)?;
        let outcome = outcome.map(|bytes| open_bytes(&run_context(&id, "outcome"), &bytes)).transpose()?;
        Ok(RunRow {
            id: RunId::try_from(id).map_err(|_| corrupt("a run id"))?,
            number: r.get::<_, i64>(1)? as u64,
            kind: Kind::parse(&kind).ok_or_else(|| corrupt("a run kind"))?,
            target,
            // One spelling, whatever an older Calcifer recorded (`https://tensorhub.com:443`).
            hub: (!hub.is_empty())
                .then(|| Origin::parse(&hub))
                .transpose()
                .map_err(|_| corrupt("a run's Hub"))?,
            machine: r.get(5)?,
            pinned: r.get(6)?,
            // A run imported from the Go CLI was recorded without one.
            spec: match spec.is_empty() {
                true => v1::RunSpec::default(),
                false => serde_json::from_slice(&spec).map_err(|_| corrupt("a run's spec"))?,
            },
            out_dir,
            state: match state.as_str() {
                "queued" => State::Queued,
                "sent" => State::Sent,
                "accepted" => State::Accepted,
                "settled" => State::Settled,
                _ => return Err(corrupt("a run state")),
            },
            cancel_requested: r.get(10)?,
            cursor: r.get::<_, i64>(11)? as u64,
            machine_state: r.get(12)?,
            status: match status.as_deref() {
                None => None,
                Some("completed") => Some(Status::Completed),
                Some("failed") => Some(Status::Failed),
                Some("canceled") => Some(Status::Canceled),
                Some(_) => return Err(corrupt("a run status")),
            },
            reason: reason_code.zip(reason).map(|(code, message)| Reason {
                code,
                message,
                ..Default::default()
            }),
            state_seq: r.get::<_, i64>(16)? as u64,
            created_at_ms: r.get(17)?,
            accepted_at_ms: r.get(18)?,
            started_at_ms: r.get(25)?,
            settled_at_ms: r.get(19)?,
            warnings: serde_json::from_str(&warnings).map_err(|_| corrupt("a run's warnings"))?,
            inputs: serde_json::from_str(&inputs).map_err(|_| corrupt("a run's inputs"))?,
            outcome: match outcome {
                Some(bytes) => Some(serde_json::from_slice(&bytes).map_err(|_| corrupt("an outcome"))?),
                None => None,
            },
            wants: wants.unwrap_or_else(|| "any".into()),
            agreed: (agreed.as_deref().map(serde_json::from_str).transpose()).map_err(|_| corrupt("a run's agreed price"))?,
        })
    })())
}

fn run_in(conn: &Connection, id: &str) -> Result<Option<RunRow>> {
    conn.query_row(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"), [id], run_row)
        .optional()?
        .transpose()
}

const MACHINE_COLUMNS: &str = "name, kind, hub, rental_id, worker_id, address, pin, key, state, served, reason";

fn machine_row(r: &Row) -> rusqlite::Result<Result<MachineRow>> {
    let kind: String = r.get(1)?;
    let worker: Option<String> = r.get(4)?;
    let address: Option<String> = r.get(5)?;
    let pin: Option<Vec<u8>> = r.get(6)?;
    let key: String = r.get(7)?;
    let state: String = r.get(8)?;
    let endpoint = match (worker, address, pin.map(<[u8; 32]>::try_from)) {
        (Some(worker_id), Some(address), Some(Ok(pin))) => Some(Endpoint { worker_id, address, pin }),
        (None, None, None) => None,
        _ => return Ok(Err(StoreError::Corrupt("a machine endpoint".into()))),
    };
    Ok(Ok(MachineRow {
        name: r.get(0)?,
        kind: match kind.as_str() {
            "local" => MachineKind::Local,
            "rental" => MachineKind::Rental,
            _ => MachineKind::Added,
        },
        hub: r.get(2)?,
        rental_id: r.get(3)?,
        endpoint,
        key: PathBuf::from(key),
        state: match state.as_str() {
            "booting" => MachineState::Booting,
            "ready" => MachineState::Ready,
            _ => MachineState::Ended,
        },
        served: r.get(9)?,
        reason: r.get(10)?,
    }))
}

/// Moves a machine to another name, with its runs and asks; `alias` is its former name.
fn move_machine(tx: &Transaction, from: &str, to: &str, alias: Option<&str>) -> Result<()> {
    // Its unique rental id and alias move with it.
    let (rental, held): (Option<String>, Option<String>) =
        tx.query_row("SELECT rental_id, alias FROM machines WHERE name = ?1", [from], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    tx.execute("UPDATE machines SET rental_id = NULL, alias = NULL WHERE name = ?1", [from])?;
    tx.execute(
        "INSERT INTO machines (name, kind, hub, rental_id, worker_id, address, pin, key, state, served,
             reason, created_at, alias)
         SELECT ?2, kind, hub, ?3, worker_id, address, pin, key, state, served, reason, created_at, ?4
         FROM machines WHERE name = ?1",
        params![from, to, rental, held.as_deref().or(alias)],
    )?;
    tx.execute("UPDATE runs SET machine = ?2 WHERE machine = ?1", params![from, to])?;
    tx.execute("UPDATE asks SET machine = ?2 WHERE machine = ?1", params![from, to])?;
    tx.execute("DELETE FROM machines WHERE name = ?1", [from])?;
    Ok(())
}

/// What a Hub is asked to do, recorded before it is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AskKind {
    Rent,
    End,
}

impl AskKind {
    fn as_str(self) -> &'static str {
        match self {
            AskKind::Rent => "rent",
            AskKind::End => "end",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AskState {
    Pending,
    Done,
    Failed,
}

/// What a project write answered under its `Idempotency-Key`, and the digest of the ask (method, path and body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub digest: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// One ask of a Hub: its key is the idempotency key the Hub gets, and `body` the exact bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    pub key: String,
    pub kind: AskKind,
    pub hub: String,
    /// What was asked for, which the same key must ask again.
    pub digest: String,
    pub body: Vec<u8>,
    pub machine: Option<String>,
    /// The run it was made for.
    pub run: Option<RunId>,
    pub state: AskState,
    pub reason: Option<String>,
}

const ASK_COLUMNS: &str = "key, kind, hub, digest, body, machine, run, state, reason";

fn ask_row(r: &Row) -> rusqlite::Result<Result<Ask>> {
    let key: String = r.get(0)?;
    let body: Vec<u8> = r.get(4)?;
    let reason: Option<String> = r.get(8)?;
    let kind: String = r.get(1)?;
    let run: Option<String> = r.get(6)?;
    let state: String = r.get(7)?;
    Ok((|| {
        let body = open_bytes(&format!("ask:{key}:body"), &body)?;
        let reason = optional_text(&format!("ask:{key}:reason"), reason)?;
        Ok(Ask {
            key,
            kind: if kind == "end" { AskKind::End } else { AskKind::Rent },
            hub: r.get(2)?,
            digest: r.get(3)?,
            body,
            machine: r.get(5)?,
            run: run
                .map(RunId::try_from)
                .transpose()
                .map_err(|_| StoreError::Corrupt("an ask's run".into()))?,
            state: match state.as_str() {
                "pending" => AskState::Pending,
                "done" => AskState::Done,
                _ => AskState::Failed,
            },
            reason,
        })
    })())
}

fn ask_in(conn: &Connection, key: &str) -> Result<Option<Ask>> {
    conn.query_row(&format!("SELECT {ASK_COLUMNS} FROM asks WHERE key = ?1"), [key], ask_row)
        .optional()?
        .transpose()
}

fn machine_kind(kind: MachineKind) -> &'static str {
    match kind {
        MachineKind::Local => "local",
        MachineKind::Rental => "rental",
        MachineKind::Added => "added",
    }
}

fn machine_state(state: MachineState) -> &'static str {
    match state {
        MachineState::Booting => "booting",
        // Ending is the API's word for a machine whose end its Hub has not affirmed; nothing
        // records a state it does not know.
        MachineState::Ready | MachineState::Ending | MachineState::Stopped | MachineState::Unknown => "ready",
        MachineState::Ended => "ended",
    }
}

impl From<&MachineRow> for Machine {
    fn from(row: &MachineRow) -> Self {
        Machine {
            ssh: None,
            name: row.name.clone(),
            kind: row.kind,
            state: row.state,
            hub: row.hub.clone(),
            rental_id: row.rental_id.clone(),
            alias: None,
            address: row.endpoint.as_ref().map(|e| e.address.clone()),
            fingerprint: row.endpoint.as_ref().map(|e| Digest::from_bytes(e.pin)),
            reason: row.reason.clone(),
            offline: None,
            rental: None,
            status: None,
            unreachable: None,
            unreachable_since_ms: None,
            boot: None,
            attempt: None,
            running: 0,
            queued: 0,
            ordered: true,
            unverified: false,
            controller_key: None,
            reach: None,
            ending: None,
            end_failed: None,
        }
    }
}

/// A recorded log line as its event: a settled call is `run.call`, the rest `run.log`.
fn log_event(sequence_number: u64, at_ms: i64, level: String, text: String) -> Event {
    match serde_json::from_str::<Call>(&text) {
        Ok(call) if level == "call" => Event::Call { sequence_number, call },
        _ => Event::Log {
            sequence_number,
            at_ms,
            level,
            text,
        },
    }
}

/// The run's next event sequence number.
fn next_seq(tx: &Transaction, id: &str) -> Result<u64> {
    Ok(
        tx.query_row("UPDATE runs SET seq = seq + 1 WHERE id = ?1 RETURNING seq", [id], |r| {
            r.get::<_, i64>(0)
        })? as u64,
    )
}

/// A new lifecycle event for the run as it now stands.
fn lifecycle(tx: &Transaction, id: &str) -> Result<Event> {
    let seq = next_seq(tx, id)?;
    tx.execute("UPDATE runs SET state_seq = ?2 WHERE id = ?1", params![id, seq as i64])?;
    let row = run_in(tx, id)?.ok_or_else(|| StoreError::Corrupt(format!("no run {id}")))?;
    Ok(Event::lifecycle(seq, record(tx, &row)?, false))
}

/// Settles the run: its outputs can no longer change, and its terminal event is the last.
fn end(tx: &Transaction, id: &str, status: Status, reason: Option<Reason>) -> Result<Vec<Event>> {
    let mut events = vec![];
    let open: Vec<(String, i64)> = {
        let mut statement = tx.prepare("SELECT name, idx FROM outputs WHERE run = ?1 AND done = 0 ORDER BY created_seq")?;
        statement
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.execute(
        "UPDATE runs SET state = 'settled', status = ?2, reason_code = ?3, reason = ?4, settled_at = ?5 WHERE id = ?1",
        params![
            id,
            match status {
                Status::Completed => "completed",
                Status::Canceled => "canceled",
                _ => "failed",
            },
            reason
                .as_ref()
                .map(|r| seal_run_text(tx, id, &run_context(id, "reason_code"), &r.code))
                .transpose()?,
            reason
                .as_ref()
                .map(|r| seal_run_text(tx, id, &run_context(id, "reason"), &r.message))
                .transpose()?,
            now_ms(),
        ],
    )?;
    for (name, idx) in open {
        let seq = next_seq(tx, id)?;
        tx.execute(
            "UPDATE outputs SET done = 1, seq = ?4 WHERE run = ?1 AND name = ?2 AND idx = ?3",
            params![id, name, idx, seq as i64],
        )?;
        let row = run_in(tx, id)?.ok_or_else(|| StoreError::Corrupt(format!("no run {id}")))?;
        let run = record(tx, &row)?;
        if let Some((output_index, item)) = run
            .output
            .into_iter()
            .enumerate()
            .find(|(_, item)| item.name == name && i64::from(item.index) == idx)
        {
            events.push(Event::OutputDone {
                sequence_number: seq,
                output_index: output_index as u32,
                item,
            });
        }
    }
    events.push(lifecycle(tx, id)?);
    Ok(events)
}

/// Records one revision of an output, unless a newer one is recorded.
fn put_output(tx: &Transaction, row: &RunRow, product: &v1::Product) -> Result<Vec<Event>> {
    let id = row.id.as_str();
    let index = i64::from(product.index);
    let recorded: Option<(i64, bool)> = tx
        .query_row(
            "SELECT rev, done FROM outputs WHERE run = ?1 AND name = ?2 AND idx = ?3",
            params![id, product.output, index],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if recorded.is_some_and(|(rev, done)| done || rev as u64 >= product.rev) {
        return Ok(vec![]);
    }
    let seq = next_seq(tx, id)?;
    let checkpoint = product
        .checkpoint
        .as_ref()
        .map(|checkpoint| {
            let text = serde_json::to_string(checkpoint).expect("serializes");
            seal_run_text(
                tx,
                id,
                &output_context(id, &product.output, product.index, product.rev, "checkpoint"),
                &text,
            )
        })
        .transpose()?;
    let (length, digest, media_type, duration_us) = match product.checkpoint {
        Some(_) => (0, "", "", 0),
        None => (
            product.length,
            product.digest.as_str(),
            product.media_type.as_str(),
            product.duration_us,
        ),
    };
    tx.execute(
        "INSERT INTO outputs (run, name, idx, rev, length, digest, media_type, label, duration_us, created_seq, seq, checkpoint)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)
         ON CONFLICT (run, name, idx) DO UPDATE SET rev = ?4, length = ?5, digest = ?6, media_type = ?7,
             label = ?8, duration_us = ?9, seq = ?10, checkpoint = ?11",
        params![
            id,
            product.output,
            index,
            product.rev as i64,
            length as i64,
            digest,
            media_type,
            seal_run_text(
                tx,
                id,
                &output_context(id, &product.output, product.index, product.rev, "label"),
                &product.label
            )?,
            duration_us as i64,
            seq as i64,
            checkpoint,
        ],
    )?;
    let run = record(tx, row)?;
    let Some((output_index, item)) = run
        .output
        .into_iter()
        .enumerate()
        .find(|(_, item)| item.name == product.output && item.index == product.index)
    else {
        return Ok(vec![]);
    };
    let output_index = output_index as u32;
    Ok(vec![match recorded {
        None => Event::OutputAdded {
            sequence_number: seq,
            output_index,
            item,
        },
        Some(_) => Event::OutputDelta {
            sequence_number: seq,
            output_index,
            item_id: item.id,
            rev: product.rev,
            length: item.length,
            appended_from: product.appended_from.filter(|_| item.checkpoint.is_none()),
            checkpoint: item.checkpoint,
        },
    }])
}

/// The API path that serves an output.
fn output_url(number: u64, name: &str, index: u32) -> String {
    let mut url = url::Url::parse("http://calcifer/").expect("a base URL");
    url.path_segments_mut()
        .expect("a base URL")
        .extend(["v1", "runs", &number.to_string(), "outputs", name]);
    match index {
        0 => url.path().to_owned(),
        index => format!("{}?index={index}", url.path()),
    }
}

/// A run row as the API answers it, with its output items in the order they first appeared.
fn record(conn: &Connection, row: &RunRow) -> Result<Run> {
    let mut statement = conn.prepare(&format!(
        "SELECT name, idx, rev, length, digest, media_type, label, done,
             CASE WHEN exported_rev = rev THEN path END, CASE WHEN failed_rev = rev THEN export_error END,
             checkpoint, {STAYS}, width, height, measured_ms, duration_us
         FROM outputs WHERE run = ?1 ORDER BY created_seq"
    ))?;
    let settled_ok = row.status == Some(Status::Completed);
    let items = statement.query_map([row.id.as_str()], |r| {
        let name: String = r.get(0)?;
        let index = r.get::<_, i64>(1)? as u32;
        let digest: String = r.get(4)?;
        let done: bool = r.get(7)?;
        let rev = r.get::<_, i64>(2)? as u64;
        let path = sql_optional_path(r, 8, &output_context(row.id.as_str(), &name, index, rev, "path"))?;
        let checkpoint = checkpoint_ref(r, 10, &output_context(row.id.as_str(), &name, index, rev, "checkpoint"))?;
        let media_type: String = r.get(5)?;
        let url = (path.is_some() && checkpoint.is_none()).then(|| output_url(row.number, &name, index));
        let measured = |column| -> rusqlite::Result<Option<u32>> { Ok(Some(r.get::<_, i64>(column)? as u32).filter(|n| *n > 0)) };
        let (measured_ms, grown_us) = (r.get::<_, i64>(14)? as u64, r.get::<_, i64>(15)? as u64);
        let id = match index {
            0 => format!("{}/{name}", row.number),
            i => format!("{}/{name}/{i}", row.number),
        };
        let status = match (done, settled_ok) {
            (false, _) => ItemStatus::InProgress,
            (true, true) => ItemStatus::Completed,
            (true, false) => ItemStatus::Incomplete,
        };
        let label = sql_text(r, 6, &output_context(row.id.as_str(), &name, index, rev, "label"))?;
        let export_error = sql_optional_text(r, 9, &output_context(row.id.as_str(), &name, index, rev, "export_error"))?;
        Ok(OutputItem {
            id,
            name,
            index,
            url,
            width: measured(12)?,
            height: measured(13)?,
            duration_ms: Some(if measured_ms > 0 { measured_ms } else { grown_us / 1000 }).filter(|ms| *ms > 0),
            rev: r.get::<_, i64>(2)? as u64,
            length: r.get::<_, i64>(3)? as u64,
            media_type,
            label,
            sha256: (status == ItemStatus::Completed && checkpoint.is_none())
                .then(|| digest.parse().ok())
                .flatten(),
            status,
            path: path.filter(|_| checkpoint.is_none()),
            export_error: export_error.filter(|_| checkpoint.is_none()),
            checkpoint,
            stays: r.get(11)?,
        })
    })?;
    Ok(Run {
        id: row.id.clone(),
        number: row.number,
        machine_number: None,
        kind: row.kind,
        target: row.target.clone(),
        hub: row.hub.clone().map(String::from).unwrap_or_default(),
        project: None,
        project_id: None,
        request: None,
        machine: row.machine.clone(),
        status: match (row.state, row.status) {
            (State::Settled, Some(status)) => status,
            (State::Accepted, _) => Status::InProgress,
            _ => Status::Queued,
        },
        cancel_requested: row.cancel_requested,
        machine_state: row.machine_state.clone(),
        queue_position: None,
        // The machine's own reason, with its traceback, when it is the run's.
        reason: row.reason.clone().map(|reason| {
            (row.outcome.as_ref().and_then(|o| o.reason.clone()))
                .filter(|full| full.code == reason.code)
                .unwrap_or(reason)
        }),
        output: items.collect::<rusqlite::Result<_>>()?,
        created_at_ms: row.created_at_ms,
        accepted_at_ms: row.accepted_at_ms,
        started_at_ms: row.started_at_ms,
        settled_at_ms: row.settled_at_ms,
        waiting: None,
        progress: None,
        report: None,
        warnings: row.warnings.clone(),
        adjustments: row.outcome.iter().flat_map(|outcome| outcome.adjustments.iter().cloned()).collect(),
        result: row
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.result.as_ref())
            .and_then(|result| serde_json::from_str(result.get()).ok()),
        execution_ms: row.outcome.as_ref().map(|outcome| outcome.execution_ms).filter(|ms| *ms > 0),
        memo: (row.outcome.as_ref())
            .filter(|outcome| !outcome.computation_digest.is_empty())
            .map(|outcome| Memo {
                computation: outcome.computation_digest.clone(),
                memoized: outcome.memoized,
                basis: outcome.memo_basis.clone(),
            }),
        measurements: None,
        stalled: None,
        calls: vec![],
        gpu: row
            .outcome
            .as_ref()
            .map(|outcome| outcome.gpu.clone())
            .filter(|gpu| !gpu.is_empty()),
    })
}

/// The store and its WAL are this person's alone: they hold sessions' digests and machines' pins.
fn private(path: &Path) {
    #[cfg(unix)]
    for file in [path.to_owned(), path.with_extension("db-wal"), path.with_extension("db-shm")] {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("{}: {e}", file.display());
        }
    }
}

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(what.into())
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
