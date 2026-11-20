//! Calcifer's store: one SQLite file only Calcifer writes. A run's state lives in its row's
//! columns; what the CLI's event stream replays is derived from them (`events_after`). Durable
//! here: transitions, outputs at their newest revision, and log lines. Never progress.
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::project::vault;
use api::{
    calcifer::{Agreed, Call, Event, ItemStatus, Kind, Machine, MachineKind, MachineState, Memo, OutputItem, Reason, Retry, Run, Status, Warning},
    config::Origin,
    ids::{Digest, RunId},
    machine as v1,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

#[cfg(test)]
#[path = "store_listing_tests.rs"]
mod listing_tests;


/// The store's version (`PRAGMA user_version`), and the schema a new store is made with. A store
/// at another version is not read.
pub(crate) const VERSION: i64 = 9;
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
        alias TEXT,
        held TEXT,
        ended_at INTEGER
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
        job BLOB,
        owner TEXT NOT NULL DEFAULT '',
        attempt INTEGER NOT NULL DEFAULT 1,
        task TEXT NOT NULL DEFAULT ''
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

/// A job's type (cl-1028): a store from before has none on its rows, which no job's end reads.
fn task_column(writer: &mut Connection) -> Result<()> {
    let has: bool = writer.query_row("SELECT EXISTS (SELECT 1 FROM pragma_table_info('runs') WHERE name = 'task')", [], |r| r.get(0))?;
    if !has {
        writer.execute_batch("ALTER TABLE runs ADD COLUMN task TEXT NOT NULL DEFAULT ''")?;
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
/// How long an ended machine's row is kept (Paul 11-15: "keep other rental data for like 90 days or so, so that users can go back
/// and see what they did").
const KEPT_FOR_MS: i64 = 90 * 24 * 3600 * 1000;
/// How many a machine's end takes out: more than end meanwhile, few enough to hold the writer briefly.
const PRUNED_EACH: i64 = 16;
/// How long an ended job is kept before a newer one of its machine, job type and outcome takes it (cl-1028): whoever asked for it
/// has read its end by then.
const ENDED_KEPT_MS: i64 = 60_000;
/// A run this much older than the measurements kept, still not settled, never will be: `drop_measurements` goes past it.
const GIVEN_UP_MS: i64 = 30 * 24 * 3600 * 1000;
/// The newest run number whose measurements `drop_measurements` has gone past.
const MEASUREMENTS_DROPPED: &str = "CREATE TABLE IF NOT EXISTS measurements_dropped (through INTEGER NOT NULL) STRICT;";
/// Each name a rental had before its current one (cl-1125): a project's run keeps the name it was sent by, and an order replanned onto
/// another host is named again.
const MACHINE_NAMES: &str =
    "CREATE TABLE IF NOT EXISTS machine_names (name TEXT NOT NULL, rental_id TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY (name, rental_id)) STRICT, WITHOUT ROWID;";
/// Projects whose files left on a machine were gone before the legacy cut could copy them in (cl-1003), said by `cozy doctor`
/// and the project's page.
pub(crate) const LOST: &str = "CREATE TABLE IF NOT EXISTS lost_before_upgrade (path TEXT PRIMARY KEY, files INTEGER NOT NULL) STRICT;";
/// How long an answer is kept for its write's retries.
const ANSWER_KEPT_MS: i64 = 24 * 60 * 60 * 1000;


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
    #[error("Calcifer's store is at version {0}, written by a newer cozy (this one reads {current}): run `cozy update` to go forward", current = VERSION)]
    Version(i64),
    #[error("Calcifer's store is at version {0}, from a cozy before 0.26, which this one doesn't read: move it aside to start a new one")]
    Older(i64),
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
    #[error("{0}")]
    ProjectKey(String),
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
fn sql_optional_path(row: &Row<'_>, column: usize, context: &str, owner:Option<&Path>) -> rusqlite::Result<Option<PathBuf>> {
    let encoded: Option<String> = row.get(column)?;
    encoded.map(|value| {
        let decoded = open_text(context, &value).map_err(|error| sql_private(column, error))?;
        let path=vault::payload_path(value.as_bytes(), Path::new(&decoded)).map_err(|error| sql_private(column, private_error(error)))?;
        if let Some(root)=owner { if path.is_relative() { if path.components().any(|part|!matches!(part,std::path::Component::Normal(_))) { return Err(sql_private(column,corrupt("owned output path"))); } return Ok(root.join(path)); } }
        Ok(path)
    }).transpose()
}

fn seal_at(path: &Path, context: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    vault::seal_payload(path, context, bytes).map_err(private_error)
}
fn as_text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| corrupt("a protected text encoding"))
}
pub(crate) fn register_owner(conn:&Connection, root:Option<PathBuf>)->rusqlite::Result<()> {
    conn.create_scalar_function("cozy_project_root",0,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move |_| Ok(root.as_ref().map(|root|vault::logical(root).to_string_lossy().into_owned())))
}
fn stored_output_path(conn:&Connection,path:&Path)->Result<String> {
    let root=conn.query_row("SELECT cozy_project_root()",[],|row|row.get::<_,Option<String>>(0))?;
    let path=vault::logical(path);
    match root {
        None=>Ok(path.to_string_lossy().into_owned()),
        Some(root)=>path.strip_prefix(Path::new(&root)).map(|relative|relative.to_string_lossy().into_owned()).map_err(|_|StoreError::Protected("project_output_scope".into())),
    }
}

fn run_scope(conn: &Connection, id: &str) -> Result<(PathBuf, bool)> {
    let (path,spec,root):(String,Vec<u8>,Option<String>)=conn.query_row("SELECT out_dir,spec,cozy_project_root() FROM runs WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    let path=root.map(|root|crate::project::landing(Path::new(&root))).unwrap_or_else(||PathBuf::from(path));
    Ok((path,vault::is_payload(&spec)))
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
    pub project_root: Option<PathBuf>,
    pub owner: String,
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
    /// Which time it is sent: a machine's failure under it sends it again (cl-935).
    pub attempt: u32,
}

impl RunRow {
    /// The id its machine knows it by: its own, then `<id>~<attempt>` for each time it is sent again (moved, retried).
    pub(crate) fn sent_id(&self) -> Sent {
        Sent(match self.attempt {
            0 | 1 => self.id.to_string(),
            attempt => format!("{}~{attempt}", self.id),
        })
    }
}

/// A run as its machine knows it, made only by `RunRow::sent_id`: every word Calcifer says to a machine about a run (send, events,
/// cancel, preview, play, output reads) names it so. A bare run id names no run there once it was sent again (cl-1116: a moved H3
/// take's output read as `run_not_found` and was lost).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Sent(String);

impl Sent {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Sent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The run a machine's id names (`RunRow::sent_id`).
pub(crate) fn calcifer_id(sent: &str) -> &str {
    sent.rsplit_once('~').filter(|(_, n)| n.parse::<u32>().is_ok()).map_or(sent, |(id, _)| id)
}

/// How many times a run is sent in all when its machine's GPU fails under it (cl-935).
const ATTEMPTS: u32 = 3;

/// A failure that is the machine's GPU's, not the run's: a driver fault on its GPU, or its GPU's
/// memory lost to a killed process stuck in the kernel. It is sent again; a model or package
/// failure never is.
fn machine_failure(outcome: &v1::Outcome) -> bool {
    outcome.status == "failed" && outcome.reason.as_ref().is_some_and(|r| matches!(r.code.as_str(), "gpu_fault" | "gpu_lost"))
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
    pub project: Option<String>,
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
    /// What one of Calcifer's jobs does: kept on its state.db row (cl-1028).
    pub task: Task,
}

/// What one of Calcifer's jobs does (cl-1028): its machine keeps the newest ended of each, by outcome. `None`: a project's run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Task {
    #[default]
    None,
    Install,
    Download,
    Upload,
    Quantize,
    Warm,
    Remove,
    Update,
    Lock,
    Publish,
}

impl Task {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Task::None => "",
            Task::Install => "install",
            Task::Download => "download",
            Task::Upload => "upload",
            Task::Quantize => "quantize",
            Task::Warm => "warm",
            Task::Remove => "remove",
            Task::Update => "update",
            Task::Lock => "lock",
            Task::Publish => "publish",
        }
    }

    /// The job a reserved target of the package `cozy` asks (`api::calcifer::hub::job`).
    pub(crate) fn of_job(target: &str) -> Self {
        use ::api::calcifer::hub::job;
        match target {
            job::MODEL_DOWNLOAD => Task::Download,
            job::MODEL_UPLOAD => Task::Upload,
            job::MODEL_QUANTIZE => Task::Quantize,
            job::PACKAGE_INSTALL => Task::Install,
            job::PACKAGE_REMOVE => Task::Remove,
            job::PACKAGE_LOCK => Task::Lock,
            job::PACKAGE_PUBLISH => Task::Publish,
            job::COZY_UPDATE => Task::Update,
            _ => Task::None,
        }
    }
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
    /// What the machine says of the run itself, by code (cl-913): kept among its warnings, each code once.
    pub warnings: Vec<Warning>,
}

impl Batch {
    pub(crate) fn is_empty(&self) -> bool {
        self.cursor == 0 && self.logs.is_empty() && self.products.is_empty() && self.machine_state.is_none() && self.started_at_ms.is_none()
            && self.warnings.is_empty()
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
    readers: Readers,
    durable: Durable,
    /// state.db's own path: who holds its WAL open is asked of it (`empty_wal`).
    path: std::path::PathBuf,
}

/// How many reads go on at once: a slow one (a scan on a cold, busy disk) holds up only itself (cl-1010).
const READERS: usize = 8;

/// state.db's WAL, emptied once a deletion takes what must leave no trace (cl-984, cl-1031).
static WAL: platform::durable::wal::Wal = platform::durable::wal::Wal::new("state.db");

/// The store's read connections. WAL lets each read while the writer commits.
struct Readers {
    idle: Mutex<Vec<Connection>>,
    returned: std::sync::Condvar,
}

impl Readers {
    fn open(path: &Path) -> Result<Self> {
        let conns = (0..READERS).map(|_| {
            let reader = Connection::open(path)?;
            register_owner(&reader, None)?;
            reader.pragma_update(None, "query_only", "ON")?;
            reader.pragma_update(None, "temp_store", "MEMORY")?;
            Ok(reader)
        }).collect::<Result<_>>()?;
        Ok(Self { idle: Mutex::new(conns), returned: std::sync::Condvar::new() })
    }

    /// Every reader, once none reads: held, no read starts until they're let go.
    fn all(&self) -> std::sync::MutexGuard<'_, Vec<Connection>> {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        while idle.len() < READERS {
            idle = self.returned.wait(idle).unwrap_or_else(|e| e.into_inner());
        }
        idle
    }

    /// An idle reader, waited for while every one reads.
    fn take(&self) -> Reader<'_> {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(conn) = idle.pop() {
                return Reader { readers: self, conn: Some(conn) };
            }
            idle = self.returned.wait(idle).unwrap_or_else(|e| e.into_inner());
        }
    }
}

struct Reader<'a> {
    readers: &'a Readers,
    conn: Option<Connection>,
}

impl std::ops::Deref for Reader<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("held until dropped")
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        self.readers.idle.lock().unwrap_or_else(|e| e.into_inner()).push(conn);
        self.readers.returned.notify_all();
    }
}

/// Group commit. A commit only writes the WAL, under the writer lock; one flush of the WAL then
/// makes every commit before it durable. Writes waiting together share a flush, so on a disk
/// whose flush takes seconds a write waits one or two of them, not one per write queued ahead.
/// The purger's due times are kept from the first start with `purge_due` on: that once, every known plaintext project is due, as each
/// was at every start before; an encrypted one's never is (`project::purge`). Marked done by its own row, in the transaction that makes
/// them due.
fn due_once(writer: &mut Connection) -> Result<()> {
    const STEP: &str = "purge_due";
    if writer.query_row("SELECT EXISTS (SELECT 1 FROM steps_done WHERE step = ?1)", [STEP], |row| row.get::<_, bool>(0))? {
        return Ok(());
    }
    let tx = writer.transaction()?;
    let known: Vec<(String, String)> =
        tx.prepare("SELECT id, path FROM projects")?.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    for (id, _) in known.iter().filter(|(_, path)| !vault::encrypted(Path::new(path))) {
        tx.execute("INSERT OR IGNORE INTO purge_due (project, at_ms) VALUES (?1, 0)", [id])?;
    }
    tx.execute("INSERT INTO steps_done (step) VALUES (?1)", [STEP])?;
    tx.commit()?;
    Ok(())
}

/// The WAL's pages that start a checkpoint beside the writer: SQLite's own autocheckpoint size.
const CHECKPOINT_PAGES: std::ffi::c_int = 1000;

/// Checkpoints run beside the writer, never in a commit: once `writer`'s commit leaves CHECKPOINT_PAGES in the WAL, a passive one of
/// `path` runs on a thread and a connection of its own, one at a time (the hook replaces SQLite's autocheckpoint). SQLite's own ran in
/// whichever commit filled the WAL, holding the writer: one after a prune's deletions held it 1.4 s, and every request that wrote waited
/// (cl-1010).
fn checkpoint_beside(writer: &mut Connection, path: &Path) -> rusqlite::Result<()> {
    let (path, going) = (path.to_owned(), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
    writer.wal_hook(Some(move |wal: &mut rusqlite::hooks::Wal, pages: std::ffi::c_int| {
        if pages >= CHECKPOINT_PAGES && !going.swap(true, Ordering::AcqRel) {
            let (path, beside) = (path.clone(), going.clone());
            let started = std::thread::Builder::new().name("state-checkpoint".into()).spawn(move || {
                let done = Connection::open(&path).and_then(|conn| {
                    register_owner(&conn, None)?;
                    conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
                });
                if let Err(e) = done {
                    tracing::warn!("state.db: a checkpoint beside the writer: {e}");
                }
                beside.store(false, Ordering::Release);
            });
            // No thread to run it on: in the commit, as SQLite's own was, never not at all (the WAL would grow without end).
            if started.is_err() {
                going.store(false, Ordering::Release);
                wal.checkpoint()?;
            }
        }
        Ok(())
    }))
}

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

/// The runs `which` selects (a condition on `runs`), newest first.
fn jobs_where(conn: &Connection, which: &str, values: impl rusqlite::Params) -> rusqlite::Result<Vec<Result<RunRow>>> {
    let mut statement = conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE {which} ORDER BY number DESC"))?;
    statement.query_map(values, run_row)?.collect()
}

/// Deletes the jobs `which` selects (a condition on `runs`) with what hangs off each: its log, outputs, placement, kept step, a
/// rental ask's link to it and its `ended_jobs` row (cl-1028). At most a hundred at once; how many went.
fn forget_jobs(tx: &Transaction<'_>, which: &str, values: impl rusqlite::Params) -> Result<usize> {
    let jobs: Vec<(String, String)> = {
        let mut statement = tx.prepare(&format!("SELECT id, idempotency_key FROM runs WHERE {which} LIMIT 100"))?;
        statement.query_map(values, |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for (id, key) in &jobs {
        for sql in ["DELETE FROM run_logs WHERE run = ?1", "DELETE FROM outputs WHERE run = ?1", "DELETE FROM run_placements WHERE run = ?1", "DELETE FROM run_retries WHERE run = ?1",
                    "UPDATE asks SET run = NULL WHERE run = ?1", "DELETE FROM ended_jobs WHERE run = ?1"] {
            tx.execute(sql, [id])?;
        }
        tx.execute("DELETE FROM after_steps WHERE key_sha256 = ?1", [key_digest(key)])?;
        tx.execute("DELETE FROM runs WHERE id = ?1", [id])?;
    }
    Ok(jobs.len())
}

/// A warm set job `row` asked, now it completed: the machine's own, kept beside it (cl-1028), so the next set it is sent still has it.
fn keep_warm_set(tx: &Transaction<'_>, row: &RunRow) -> Result<()> {
    let (Kind::Warm, Some(machine), Some(set)) = (row.kind, &row.machine, &row.spec.set) else { return Ok(()) };
    for item in &set.items {
        let item = serde_json::to_string(&v1::WarmItem { holds: String::new(), held_back: String::new(), ..item.clone() })
            .map_err(|e| StoreError::Corrupt(e.to_string()))?;
        tx.execute("INSERT OR IGNORE INTO machine_warm (machine, item) VALUES (?1, ?2)", params![machine, item])?;
    }
    Ok(())
}

pub(crate) fn key_digest(key: &str) -> String {
    use sha2::{Digest as _, Sha256};
    hex::encode(Sha256::digest(key.as_bytes()))
}

macro_rules! journal_methods {
    () => {
    /// Unsettled function runs by machine, how busy its GPUs are (Paul 11-11, cl-889): how many it accepted, and how many wait to
    /// be sent or accepted. An install, a download, an update or a removal is no job; neither is a copied run.
    pub(crate) fn machine_load(&self) -> Result<std::collections::HashMap<String, (u32, u32)>> {
        self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT machine, SUM(state = 'accepted'), SUM(state <> 'accepted') FROM runs
                 WHERE state <> 'settled' AND machine IS NOT NULL AND kind = 'call' AND (?1 IS NULL OR owner=?1) GROUP BY machine",
            )?;
            let rows = statement.query_map([self.owner_id()], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
    /// How many runs `machine` has been sent and hasn't settled, an update's aside: its work going.
    pub(crate) fn working(&self, machine: &str) -> Result<u32> {
        self.read(|conn| {
            let going = conn.query_row(
                // `state <> 'settled'` reads `runs_owed`, not every run.
                "SELECT COUNT(*) FROM runs WHERE machine = ?1 AND state <> 'settled' AND kind <> 'update' AND state IN ('sent', 'accepted') AND (?2 IS NULL OR owner=?2)",
                params![machine,self.owner_id()],
                |r| r.get(0),
            )?;
            Ok(going)
        })
    }

    /// How many runs wait for `machine`, are being sent or go there, an update's aside: whether it is idle (cl-1201).
    pub(crate) fn busy(&self, machine: &str) -> Result<u32> {
        self.read(|conn| {
            let busy = conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE machine = ?1 AND state <> 'settled' AND kind <> 'update' AND (?2 IS NULL OR owner=?2)",
                params![machine, self.owner_id()],
                |r| r.get(0),
            )?;
            Ok(busy)
        })
    }

    /// Whether this journal is at rest: no run going or waiting but an update, and none ended in the last `quiet_ms` (xs-195:
    /// `local` moves to the RC only then, never between a render's steps).
    pub(crate) fn at_rest(&self, quiet_ms: i64) -> Result<bool> {
        if now_ms() - LAST_ENDED_MS.load(Ordering::Relaxed).max(*STARTED_MS) < quiet_ms {
            return Ok(false);
        }
        self.read(|conn| {
            let busy: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM runs WHERE state <> 'settled' AND kind <> 'update' AND (?1 IS NULL OR owner=?1))",
                params![self.owner_id()],
                |r| r.get(0),
            )?;
            Ok(!busy)
        })
    }

    /// Where the plan sent run `id`, and its offer there (cl-967): the first said stands.
    pub(crate) fn record_placement(&self, id: &RunId, placed: &::api::calcifer::Placed) -> Result<()> {
        let record = serde_json::to_string(placed).expect("a placement serializes");
        self.write(|tx| { tx.execute("INSERT OR IGNORE INTO run_placements(run,placed) VALUES(?1,?2)", params![id.as_str(), record])?; Ok(()) })
    }
    pub(crate) fn plan_after(&self, key: &str, step: &str) -> Result<()> {
        self.write(|tx| { tx.execute("INSERT OR REPLACE INTO after_steps(key_sha256,step) VALUES(?1,?2)", params![key_digest(key),step])?; Ok(()) })
    }
    pub(crate) fn after_step(&self, key: &str) -> Result<Option<String>> {
        self.read(|conn| Ok(conn.query_row("SELECT step FROM after_steps WHERE key_sha256=?1", [key_digest(key)], |row| row.get(0)).optional()?))
    }
    pub(crate) fn forget_after(&self, key: &str) -> Result<()> {
        self.write(|tx| { tx.execute("DELETE FROM after_steps WHERE key_sha256=?1", [key_digest(key)])?; Ok(()) })
    }
    /// `machine`'s settled updates that completed, newest first, each with the key it was asked under (cl-922).
    pub(crate) fn updates(&self, machine: &str, limit: u32) -> Result<Vec<(RunRow, String)>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {RUN_COLUMNS}, idempotency_key FROM runs WHERE machine = ?1 AND kind = 'update' AND state = 'settled'
                     AND status = 'completed' AND (?3 IS NULL OR owner=?3) ORDER BY number DESC LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![machine, limit, self.owner_id()], |r| {
                let key: String = r.get(r.as_ref().column_count() - 1)?;
                run_row(r).map(|row| row.map(|row| (row, key)))
            })?;
            rows.map(|row| row?).collect()
        })
    }

    /// Unsettled runs no machine was chosen for, oldest first, with where each may go.
    pub(crate) fn unplaced(&self) -> Result<Vec<RunRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE state = 'queued' AND machine IS NULL AND (?1 IS NULL OR owner=?1) ORDER BY number"
            ))?;
            let rows = statement.query_map([self.owner_id()], run_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
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
    /// Notes why a pending ask is not done yet, without settling it: `reason` while it stays pending.
    pub(crate) fn note_ask(&self, key: &str, reason: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE asks SET reason = ?2 WHERE key = ?1 AND state = 'pending'", params![key, reason])?;
            Ok(())
        })
    }

    /// The end asks that failed: Calcifer asks each again, paced, while its machine stands (cl-977).
    pub(crate) fn failed_end_asks(&self) -> Result<Vec<Ask>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {ASK_COLUMNS} FROM asks WHERE state = 'failed' AND kind = 'end' ORDER BY created_at"
            ))?;
            let rows = statement.query_map([], ask_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            unlocked_rows(rows)
        })
    }

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
        if let Ok(Some(machine)) = outcome { self.keep_machine(machine)?; }
        self.write(|tx| {
            match outcome {
                Ok(machine) => {
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
        self.admit_submission(&new)?;
        if let Some(machine) = &new.machine { self.require_machine(machine)?; }
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
            let machine = new.machine.clone();
            // Calcifer's own run is no machine's to accept: it is going from the row that records it.
            let (state, began) = match new.kind {
                Kind::Here => ("accepted", Some(now_ms())),
                _ => ("queued", None),
            };
            tx.execute(
                "INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec,
                     out_dir, state, created_at, warnings, inputs, wants, agreed, stays, batch, accepted_at, started_at, job, owner)
                 VALUES (?1, (SELECT COALESCE(MAX(number), 0) + 1 FROM runs), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     ?10, ?18, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?19, ?19, ?20, ?21)",
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
                    new.project.as_deref().unwrap_or_default(),
                ],
            )?;
            if self.owner_id().is_none() && new.task != Task::None {
                tx.execute("UPDATE runs SET task = ?2 WHERE id = ?1", params![new.id.as_str(), new.task.as_str()])?;
            }
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
                // The rest by `runs_owed`, never every run.
                "SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1 OR (state <> 'settled' AND machine = (SELECT machine FROM runs WHERE id = ?1)
                     AND state IN ('queued', 'sent') AND batch <> '' AND batch = (SELECT batch FROM runs WHERE id = ?1)) ORDER BY number"
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
            // `+number`: ordered after reading `runs_owed`, never by walking every run in number order.
            let mut statement = conn.prepare("SELECT id FROM runs WHERE state <> 'settled' AND kind <> 'here' AND (?1 IS NULL OR owner=?1) ORDER BY +number")?;
            let ids = statement
                .query_map([self.owner_id()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter()
                .map(|id| RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into())))
                .collect()
        })
    }

    /// The machines' runs not yet settled that their machines haven't begun (cl-1082): a vault's maximum doesn't wait for them.
    pub(crate) fn unsettled_unstarted(&self) -> Result<Vec<RunId>> {
        self.read(|conn| {
            // A retried attempt (re-queued with `started_at` cleared) counts as begun: its earlier attempt may have.
            let mut statement = conn.prepare("SELECT id FROM runs WHERE state <> 'settled' AND kind <> 'here' AND started_at IS NULL AND attempt <= 1 AND (?1 IS NULL OR owner=?1) ORDER BY +number")?;
            let ids = statement
                .query_map([self.owner_id()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter()
                .map(|id| RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into())))
                .collect()
        })
    }

    /// Calcifer's own runs not yet settled: those an earlier Calcifer stopped under, or that are going now.
    pub(crate) fn unsettled_here(&self) -> Result<Vec<RunId>> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT id FROM runs WHERE state <> 'settled' AND kind = 'here' AND (?1 IS NULL OR owner=?1) ORDER BY +number")?;
            let ids = statement
                .query_map([self.owner_id()], |r| r.get::<_, String>(0))?
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


    /// Places an unplaced run on `machine`.
    pub(crate) fn place(&self, id: &str, machine: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE runs SET machine = ?2 WHERE id = ?1 AND machine IS NULL AND state = 'queued'",
                params![id, machine],
            )? == 1)
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
            let mut warnings = row.warnings.clone();
            for warning in batch.warnings {
                if !warnings.iter().any(|had| had.code == warning.code) {
                    warnings.push(warning);
                }
            }
            let warned = warnings.len() > row.warnings.len();
            if warned {
                let text = serde_json::to_string(&warnings).expect("serializes");
                tx.execute("UPDATE runs SET warnings = ?2 WHERE id = ?1", params![id, seal_run_text(tx, id, &run_context(id, "warnings"), &text)?])?;
            }
            match batch.machine_state.filter(|s| row.machine_state.as_ref() != Some(s)) {
                Some(state) => {
                    tx.execute("UPDATE runs SET machine_state = ?2 WHERE id = ?1", params![id, state])?;
                    events.push(lifecycle(tx, id)?);
                }
                None if warned => events.push(lifecycle(tx, id)?),
                None => {}
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

    /// Ends an accepted run with its machine's outcome; one its machine's GPU failed under goes
    /// back to the outbox instead, to be sent again (`machine_failure`).
    pub(crate) fn settle(&self, id: &str, outcome: &v1::Outcome, cursor: u64) -> Result<Vec<Event>> {
        self.write(|tx| {
            let Some(row) = run_in(tx, id)?.filter(|row| row.state == State::Accepted) else {
                return Ok(vec![]);
            };
            if machine_failure(outcome) && !row.cancel_requested && row.attempt < ATTEMPTS {
                let rental = match row.machine.as_deref() { Some(name) => self.rental(tx, name)?, None => false };
                let reason = outcome.reason.clone().unwrap_or_default();
                let machine = row.machine.clone().unwrap_or_default();
                // A run asked on its machine (`--on`) is sent again there only, never placed elsewhere (cl-1316).
                let elsewhere = reason.code == "gpu_lost" && rental && !row.pinned;
                let whither = match elsewhere {
                    true => "to a rental with GPUs in service".to_owned(),
                    false => format!("to {machine}, on GPUs in service"),
                };
                let said = format!("failed on {machine}, the machine's fault ({}: {}): sent again {whither}", reason.code, reason.message);
                return again(tx, &row, elsewhere, &said, &format!("{machine}'s GPU failed ({})", reason.code));
            }
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
            if self.owner_id().is_none() && status == Status::Completed {
                keep_warm_set(tx, &row)?;
            }
            Ok(events)
        })
    }

    /// Points the unsettled run `id`, recorded under `from`, at `to`: its machine's name now (cl-1089, `Calcifer::machine_now`).
    pub(crate) fn repoint(&self, id: &str, from: &str, to: &str) -> Result<Vec<Event>> {
        self.write(|tx| {
            let moved = tx.execute("UPDATE runs SET machine = ?3 WHERE id = ?1 AND machine = ?2 AND state <> 'settled'", params![id, from, to])?;
            Ok(if moved == 1 { vec![lifecycle(tx, id)?] } else { vec![] })
        })
    }

    /// Returns an accepted run its machine refused for its Runtime to the outbox (cl-1069, `Machines::refloored`), saying `said`:
    /// to its machine once that moved, or (`elsewhere`) an `any` run to be placed again. Nothing past its attempts or once canceled:
    /// it settles as refused.
    pub(crate) fn resend(&self, id: &str, elsewhere: bool, said: &str, why: &str) -> Result<Vec<Event>> {
        self.write(|tx| match run_in(tx, id)?.filter(|row| row.state == State::Accepted && !row.cancel_requested && row.attempt < ATTEMPTS) {
            Some(row) => again(tx, &row, elsewhere, said, why),
            None => Ok(vec![]),
        })
    }

    /// Sends run `id` to `machine`, which finishes it sooner (cl-1073): one not sent yet goes there; one accepted, once the machine that
    /// held it canceled it, is sent again there, the same run, its next attempt (one that started as it was told is sent again whole).
    /// None: it can't go (sent meanwhile, no longer accepted, or canceled by its asker), and settles as its machine said.
    pub(crate) fn moved(&self, id: &str, machine: &str) -> Result<Option<Vec<Event>>> {
        self.write(|tx| {
            let Some(row) = run_in(tx, id)?.filter(|row| matches!(row.state, State::Queued | State::Accepted) && !row.cancel_requested) else {
                return Ok(None);
            };
            let text = format!("moved from {} to {machine}, which finishes it sooner", row.machine.unwrap_or_default());
            match row.state {
                State::Queued => tx.execute("UPDATE runs SET machine = ?2 WHERE id = ?1", params![id, machine])?,
                _ => {
                    retried(tx, id, &text)?;
                    tx.execute(
                        "UPDATE runs SET state = 'queued', attempt = ?2, cursor = 0, machine_state = NULL, accepted_at = NULL, started_at = NULL, machine = ?3
                         WHERE id = ?1",
                        params![id, row.attempt + 1, machine],
                    )?
                }
            };
            let (seq, at_ms) = (next_seq(tx, id)?, now_ms());
            tx.execute(
                "INSERT INTO run_logs (run, seq, at_ms, level, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, seq as i64, at_ms, "info", seal_run_text(tx, id, &log_context(id, seq), &text)?],
            )?;
            Ok(Some(vec![log_event(seq, at_ms, "info".to_owned(), text), lifecycle(tx, id)?]))
        })
    }

    /// Returns run `id` to Calcifer's queue, its machine lost, saying `why` (`requeue_in`, cl-1132). `unsent`: only one never sent,
    /// as a machine that doesn't answer may hold one it was sent.
    pub(crate) fn requeue(&self, id: &str, why: &str, unsent: bool) -> Result<Vec<Event>> {
        self.write(|tx| match run_in(tx, id)? {
            Some(row) if row.state != State::Settled && (!unsent || row.state == State::Queued) => requeue_in(tx, &row, why),
            _ => Ok(vec![]),
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

    /// Ends run `id` as its vault locks past its deadline (cl-1082), only if no attempt of it has begun and no cancel is asked: one never
    /// sent fails `vault_locked`; one sent or accepted is asked to cancel, through its machine as an explicit lock does, so none runs on
    /// there orphaned. Each a guarded statement in this one transaction: a run its machine began meanwhile is left alone.
    pub(crate) fn end_unstarted(&self, id: &str) -> Result<Vec<Event>> {
        self.write(|tx| {
            const UNBEGUN: &str = "started_at IS NULL AND attempt <= 1 AND cancel_requested = 0";
            if tx.execute(&format!("UPDATE runs SET cancel_requested = 1 WHERE id = ?1 AND state = 'queued' AND {UNBEGUN}"), [id])? == 1 {
                let reason = Reason {
                    code: "vault_locked".into(),
                    message: "the project locked at its deadline before the run's machine began it".into(),
                    next: vec!["cozy vault unlock".into()],
                    ..Default::default()
                };
                return end(tx, id, Status::Failed, Some(reason));
            }
            if tx.execute(&format!("UPDATE runs SET cancel_requested = 1 WHERE id = ?1 AND state IN ('sent', 'accepted') AND {UNBEGUN}"), [id])? == 1 {
                return Ok(vec![lifecycle(tx, id)?]);
            }
            Ok(vec![])
        })
    }

    /// Records the intent to cancel (`cancel_in`).
    pub(crate) fn request_cancel(&self, id: &str) -> Result<Vec<Event>> {
        self.write(|tx| cancel_in(tx, id))
    }

    /// Returns every unsettled run on `machine` to Calcifer's queue, saying `why` (`requeue_in`, cl-1132): it ended, or is no machine
    /// here. Answers the events, by run.
    pub(crate) fn requeue_machine(&self, machine: &str, why: &str) -> Result<Vec<(RunId, Event)>> {
        self.write(|tx| {
            let mut statement = tx.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE machine = ?1 AND state <> 'settled' AND (?2 IS NULL OR owner=?2) ORDER BY number"
            ))?;
            let rows = statement.query_map(params![machine,self.owner_id()], run_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut events = vec![];
            for row in rows {
                let row = row?;
                events.extend(requeue_in(tx, &row, why)?.into_iter().map(|event| (row.id.clone(), event)));
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
                // From `outputs_unexported`: a run's row is read only when it owes one (cl-1010).
                "SELECT runs.id, runs.number FROM runs WHERE runs.id IN (SELECT outputs.run FROM outputs WHERE {OWED})
                 AND (?1 IS NULL OR runs.owner=?1) ORDER BY runs.number"
            ))?;
            let rows = statement
                .query_map([self.owner_id()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?
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
                 FROM outputs CROSS JOIN runs ON runs.id = outputs.run
                 WHERE runs.machine = ?1 AND {OWED} AND (?2 IS NULL OR runs.owner=?2)
                 ORDER BY runs.number, outputs.created_seq"
            ))?;
            let rows = statement
                .query_map(params![machine,self.owner_id()], |r| {
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

    /// Whether a run here owes Calcifer a file of these bytes: their output's file neither landed nor given up (cl-1108).
    pub(crate) fn owes_file(&self, digest: &str) -> Result<bool> {
        self.read(|conn| {
            Ok(conn.query_row(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM outputs WHERE outputs.digest = ?1 AND {OWED}
                     AND outputs.run IN (SELECT id FROM runs WHERE ?2 IS NULL OR owner = ?2))"
                ),
                params![digest, self.owner_id()],
                |r| r.get(0),
            )?)
        })
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
                params![run, name, index, rev as i64, seal_run_text(tx, run, &output_context(run, name, index, rev, "path"), &stored_output_path(tx,path)?)?, measured.width, measured.height, measured.duration_ms as i64],
            )?;
            export_settled(tx, run, name, index, rev, true)
        })
    }

    /// Records why revision `rev` of the output will not be exported; while that is its current
    /// revision, the run's next event is the item done again, carrying why.
    /// A settled run with an output that wasn't saved (cl-1060): a lock cut its save, or its export failed for good. It reads failed
    /// with `reason`, typed by why, never completed with no file; its items keep their own export errors.
    pub(crate) fn outputs_not_saved(&self, run: &str, reason: &Reason) -> Result<Vec<Event>> {
        self.write(|tx| {
            let changed = tx.execute(
                "UPDATE runs SET status = 'failed', reason_code = ?2, reason = ?3 WHERE id = ?1 AND state = 'settled' AND status = 'completed'",
                params![
                    run,
                    seal_run_text(tx, run, &run_context(run, "reason_code"), &reason.code)?,
                    seal_run_text(tx, run, &run_context(run, "reason"), &reason.message)?,
                ],
            )?;
            Ok(if changed == 0 { vec![] } else { vec![lifecycle(tx, run)?] })
        })
    }

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
    };
}

impl Store {
    journal_methods!();

    fn owner_id(&self)->Option<&str> { None }

    /// Calcifer's own journal only: its table is in state.db. Drops the measurements of the next runs that settled before `before_ms`,
    /// past the last one done (kept, so a start never reads the old ones again: cl-1010), in one short write. False: none past it is
    /// that old.
    pub(crate) fn drop_measurements(&self, before_ms: i64) -> Result<bool> {
        // What changes is worked out on a reader, the writer held only to put it: one batch read and rewrote under the writer for
        // 5.5 s on Paul's laptop (outcomes of megabytes), and every write waited (cl-1010). Only this walk moves its place.
        type Found = (i64, Vec<(String, Vec<u8>)>, Vec<(String, i64, String)>);
        let found: Option<Found> = self.read(|conn| {
            let after: i64 = conn.query_row("SELECT coalesce(max(through), 0) FROM measurements_dropped", [], |row| row.get(0))?;
            let mut next = conn.prepare("SELECT number, created_at, state = 'settled' AND settled_at < ?2 FROM runs WHERE number > ?1 ORDER BY number LIMIT 16")?;
            let next = next
                .query_map([after, before_ms], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            // A run that settled before then was created before then, and numbers go up with time. The walk keeps its place before a
            // run still going (or settled since), to drop its measurements once it's old too; one a month older still never will be.
            let past = |(_, created, done): &&(i64, i64, bool)| *created < before_ms && (*done || *created < before_ms - GIVEN_UP_MS);
            let Some(through) = next.iter().take_while(past).last().map(|(n, ..)| *n) else {
                return Ok(None);
            };
            let ids: Vec<String> = {
                let mut rows = conn.prepare("SELECT id FROM runs WHERE number > ?1 AND number <= ?2 AND settled_at < ?3")?;
                rows.query_map(params![after, through, before_ms], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let (mut outcomes, mut calls) = (vec![], vec![]);
            for id in ids {
                let outcome: Option<Vec<u8>> = conn.query_row("SELECT outcome FROM runs WHERE id = ?1", [&id], |row| row.get(0))?;
                if let Some(bytes) = outcome
                    && let Some(plain) = vault::open_payload(&run_context(&id, "outcome"), &bytes).map_err(private_error)?
                    && let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&plain)
                    && value.as_object_mut().is_some_and(|value| value.remove("measurements").is_some())
                {
                    let sealed = seal_run(conn, &id, &run_context(&id, "outcome"), &serde_json::to_vec(&value).expect("serializes"))?;
                    outcomes.push((id.clone(), sealed));
                }
                let lines: Vec<(i64, String)> = {
                    let mut rows = conn.prepare("SELECT seq, text FROM run_logs WHERE run = ?1 AND level = 'call'")?;
                    rows.query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for (seq, text) in lines {
                    let context = log_context(&id, seq as u64);
                    if let Some(plain) = vault::open_payload(&context, text.as_bytes()).map_err(private_error)?
                        && let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&plain)
                        && value.as_object_mut().is_some_and(|value| value.remove("measurements").is_some())
                    {
                        calls.push((id.clone(), seq, seal_run_text(conn, &id, &context, &value.to_string())?));
                    }
                }
            }
            Ok(Some((through, outcomes, calls)))
        })?;
        let Some((through, outcomes, calls)) = found else { return Ok(false) };
        // A run's rewrite a write of its own: a megabyte outcome holds the writer for itself alone. Dropping is the same done twice.
        let ids: std::collections::BTreeSet<&String> = outcomes.iter().map(|(id, _)| id).chain(calls.iter().map(|(id, ..)| id)).collect();
        for id in ids {
            self.write(|tx| {
                for (_, bytes) in outcomes.iter().filter(|(of, _)| of == id) {
                    tx.execute("UPDATE runs SET outcome = ?2 WHERE id = ?1", params![id, bytes])?;
                }
                for (_, seq, text) in calls.iter().filter(|(of, ..)| of == id) {
                    tx.execute("UPDATE run_logs SET text = ?3 WHERE run = ?1 AND seq = ?2", params![id, seq, text])?;
                }
                Ok(())
            })?;
        }
        self.write(|tx| {
            tx.execute("DELETE FROM measurements_dropped", [])?;
            tx.execute("INSERT INTO measurements_dropped (through) VALUES (?1)", [through])?;
            Ok(true)
        })
    }
    fn admit_submission(&self, new: &NewRun) -> Result<()> {
        if new.project.is_some() { return Err(StoreError::Protected("project_store_required".into())); }
        Ok(())
    }
    fn require_machine(&self, name: &str) -> Result<()> {
        self.machine(name)?.ok_or_else(|| StoreError::UnknownMachine(name.into())).map(drop)
    }
    fn keep_machine(&self, machine: &MachineRow) -> Result<()> { self.put_machine(machine) }
    fn rental(&self, tx: &Transaction, name: &str) -> Result<bool> {
        Ok(tx.query_row("SELECT kind FROM machines WHERE name = ?1", [name], |r| r.get::<_, String>(0)).optional()?.is_some_and(|kind| kind == "rental"))
    }

    pub(crate) fn machine_ended(&self, machine: &str, reason: &str) -> Result<Vec<(RunId, Event)>> {
        self.machine(machine)?.ok_or_else(|| StoreError::UnknownMachine(machine.into()))?;
        self.write(|tx| {
            tx.execute("UPDATE machines SET state = 'ended', reason = ?2, ended_at = coalesce(ended_at, ?3) WHERE name = ?1", params![machine, reason, now_ms()])?;
            // Its warm set goes; the jobs it ran stay with it, what it did, until it goes (cl-1028).
            tx.execute("DELETE FROM machine_warm WHERE machine = ?1", [machine])?;
            Ok(())
        })?;
        let ended = self.requeue_machine(machine, &ended_why(machine, reason));
        // A machine that ends takes out a few ended past 90 days.
        if let Err(e) = self.prune_machines(PRUNED_EACH) { tracing::warn!("ended machines past 90 days: {e}"); }
        ended
    }

    /// Returns once no write is going: what `Calcifer::close` waits out (a prune's batch).
    pub(crate) fn settled_writes(&self) {
        drop(self.writer.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Takes out ended machines older than [`KEPT_FOR_MS`] that no kept run names but their ended jobs, at most `limit`, with their logs
    /// and those jobs (cl-1028), and gives the pages back (a new store vacuums incrementally): the store keeps 90 days of them and of
    /// what they did, with no timer (Paul 11-15). Its own check stands for the foreign key's, which with no index on runs(machine) (one
    /// would take the unsettled-run plans) read every run for each machine: off for this transaction only. How many went.
    pub(crate) fn prune_machines(&self, limit: i64) -> Result<usize> {
        let before = now_ms() - KEPT_FOR_MS;
        let (pruned, commit) = {
            let mut conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            conn.pragma_update(None, "foreign_keys", "OFF")?;
            let pruned = (|| -> Result<usize> {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS pruned_machines (name TEXT PRIMARY KEY); DELETE FROM temp.pruned_machines;")?;
                tx.execute(
                    "INSERT INTO temp.pruned_machines SELECT name FROM machines WHERE state = 'ended' AND ended_at < ?1
                         AND name NOT IN (SELECT machine FROM runs WHERE machine IS NOT NULL AND id NOT IN (SELECT run FROM ended_jobs)) LIMIT ?2",
                    params![before, limit],
                )?;
                while forget_jobs(&tx, "id IN (SELECT run FROM ended_jobs WHERE machine IN (SELECT name FROM temp.pruned_machines))", [])? > 0 {}
                tx.execute("DELETE FROM machine_logs WHERE machine IN (SELECT name FROM temp.pruned_machines)", [])?;
                let pruned = tx.execute("DELETE FROM machines WHERE name IN (SELECT name FROM temp.pruned_machines)", [])?;
                tx.execute("DELETE FROM machine_names WHERE rental_id NOT IN (SELECT rental_id FROM machines WHERE rental_id IS NOT NULL)", [])?;
                if pruned > 0 { tx.execute_batch("PRAGMA incremental_vacuum(256)")?; }
                tx.commit()?;
                Ok(pruned)
            })();
            conn.pragma_update(None, "foreign_keys", "ON")?;
            (pruned?, self.durable.committed())
        };
        self.durable.wait(commit)?;
        Ok(pruned)
    }

    pub(crate) fn open(path: &Path) -> Result<Self> {
        std::sync::LazyLock::force(&STARTED_MS);
        let began = std::time::Instant::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        }
        let mut writer = Connection::open(path)?;
        register_owner(&writer,None)?;
        let mode: String = writer.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        let wal = mode.eq_ignore_ascii_case("wal");
        // In WAL mode a commit doesn't sync: `write` flushes for it (`Durable`).
        writer.pragma_update(None, "synchronous", "FULL")?;
        writer.pragma_update(None, "secure_delete", "ON")?;
        writer.pragma_update(None, "foreign_keys", "ON")?;
        writer.pragma_update(None, "temp_store", "MEMORY")?;
        match writer.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? {
            0 => {
                // Pages a prune frees go back to the disk (`prune`).
                writer.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
                let tx = writer.transaction()?;
                tx.execute_batch(SCHEMA)?;
                tx.pragma_update(None, "user_version", VERSION)?;
                tx.commit()?;
            }
            // One an RC build cut before cl-1028 has no job type on its runs.
            VERSION => task_column(&mut writer)?,
            older if older < VERSION => return Err(StoreError::Older(older)),
            newer => return Err(StoreError::Version(newer)),
        }
        run_list_indexes(&writer)?;
        writer.execute_batch("CREATE INDEX IF NOT EXISTS machines_live ON machines (name) WHERE state <> 'ended';")?;
        writer.execute_batch(ANSWERS)?;
        writer.execute_batch(LOST)?;
        writer.execute_batch(MEASUREMENTS_DROPPED)?;
        writer.execute_batch(MACHINE_NAMES)?;
        writer.execute_batch(crate::project::journal::PLACEMENTS)?;
        writer.execute_batch(crate::project::journal::RETRIES)?;
        writer.execute_batch(crate::maintenance_source::TABLES)?;
        writer.execute_batch(crate::project::vault_operation::TABLES)?;
        writer.execute_batch(crate::state::TABLES)?;
        due_once(&mut writer)?;
        writer.execute_batch(MACHINE_LOGS)?;
        // The store step's parts, in the closed health log: a slow start names its slow part (cl-1010).
        let said = |part: &'static str, since: std::time::Instant| tracing::info!(target: "cozy.service_health", step = part, ms = since.elapsed().as_millis() as u64, "started");
        said("store_schema", began);
        let rest = std::time::Instant::now();
        crate::state::project_names(&writer)?;
        crate::state::project_codes(&writer)?;
        crate::state::project_used(&writer)?;
        writer.pragma_update(None,"synchronous",if wal {"NORMAL"} else {"FULL"})?;
        private(path);
        let readers = Readers::open(path)?;
        said("store_readers", rest);
        let mut journal = path.as_os_str().to_owned();
        journal.push("-wal");
        if wal { checkpoint_beside(&mut writer, path)?; }
        // What an earlier run owed its WAL (a reader outside Calcifer held a deletion's pages) goes now, before any read here.
        WAL.empty(&writer);
        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            durable: Durable { wal: wal.then(|| journal.into()), ..Durable::sqlite() },
            path: path.to_path_buf(),
        })
    }

    /// The warm set Calcifer's own jobs asked of `machine`: what ended ones left (`machine_warm`, cl-1028) and what those going ask.
    pub(crate) fn warm_items(&self, machine: &str) -> Result<Vec<v1::WarmItem>> {
        let (kept, going) = self.read(|conn| {
            let mut kept = conn.prepare("SELECT item FROM machine_warm WHERE machine = ?1")?;
            let kept = kept.query_map([machine], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut going = conn.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE machine = ?1 AND state <> 'settled' AND kind = 'warm' AND instr(spec, CAST('\"set\":{{' AS BLOB)) > 0"
            ))?;
            let going = going.query_map([machine], run_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((kept, going))
        })?;
        let mut items: Vec<v1::WarmItem> = kept.iter().filter_map(|item| serde_json::from_str(item).ok()).collect();
        for item in going.into_iter().filter_map(|row| row.ok()).flat_map(|row| row.spec.set.map(|set| set.items).unwrap_or_default()) {
            let item = v1::WarmItem { holds: String::new(), held_back: String::new(), ..item };
            if !items.contains(&item) {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Calcifer's jobs on `machine` (cl-1028): those going and the newest ended of each job type and outcome, newest first; with
    /// `local`, also its own, which go to no machine (`''` in `ended_jobs`).
    pub(crate) fn jobs_on(&self, machine: &str, local: bool) -> Result<Vec<RunRow>> {
        let rows = self.read(|conn| {
            // Each by an index: those going by `runs_owed`, the ended by `ended_jobs`' key.
            let mut rows = jobs_where(conn, "machine = ?1 AND state <> 'settled'", [machine])?;
            if local { rows.extend(jobs_where(conn, "machine IS NULL AND state <> 'settled'", [])?); }
            rows.extend(jobs_where(
                conn,
                "id IN (SELECT run FROM ended_jobs e WHERE machine IN (?1, ?2)
                    AND number = (SELECT max(number) FROM ended_jobs WHERE machine = e.machine AND task = e.task AND status = e.status))",
                params![machine, if local { "" } else { machine }],
            )?);
            Ok(rows)
        })?;
        Ok(rows.into_iter().filter_map(|row| row.ok()).collect())
    }

    /// Job `row` ended, owes nothing and was released (cl-1028): it joins `ended_jobs`, and those of its machine, job type and
    /// outcome released before it with lower numbers go, with their logs, eight at most, once their end is [`ENDED_KEPT_MS`] old (so
    /// whoever asked for one has read it). So each (machine, job type, outcome) keeps its newest ended job, and those that ended in the
    /// minute before it or after it, until its next. One going or not yet released is never in `ended_jobs`, nor any from before
    /// cl-1028 (the hard cut, cl-1003, takes those). It holds the writer milliseconds: 100 ms is said slow.
    pub(crate) fn job_ended(&self, row: &RunRow) -> Result<()> {
        let Some(status) = row.status else { return Ok(()) };
        let status = match status {
            Status::Completed => "completed",
            Status::Canceled => "canceled",
            _ => "failed",
        };
        let machine = row.machine.as_deref().unwrap_or("");
        let now = now_ms();
        self.write_held(Some(("job_ended", std::time::Duration::from_millis(100))), |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO ended_jobs (machine, task, status, number, run, ended_at)
                 SELECT ?1, task, ?2, number, id, coalesce(settled_at, ?4) FROM runs WHERE id = ?3 AND state = 'settled'",
                params![machine, status, row.id.as_str(), now],
            )?;
            forget_jobs(
                tx,
                "id IN (SELECT run FROM ended_jobs WHERE machine = ?1 AND task = (SELECT task FROM runs WHERE id = ?5) AND status = ?2
                    AND number < ?3 AND ended_at <= ?4 ORDER BY number DESC LIMIT 8)",
                params![machine, status, row.number as i64, now - ENDED_KEPT_MS, row.id.as_str()],
            )?;
            Ok(())
        })
    }
    /// Projects whose files left on a machine were gone before the legacy cut could copy them in, and how many (cl-1003).
    pub(crate) fn lost_before_upgrade(&self) -> Result<Vec<(PathBuf, u64)>> {
        self.read(|conn| {
            let mut stmt = conn.prepare("SELECT path, files FROM lost_before_upgrade ORDER BY path")?;
            Ok(stmt.query_map([], |row| Ok((PathBuf::from(row.get::<_, String>(0)?), row.get::<_, i64>(1)?.max(0) as u64)))?.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub(crate) fn note_lost(&self, root: &Path, files: u64) -> Result<()> {
        self.write(|tx| { tx.execute("INSERT OR REPLACE INTO lost_before_upgrade (path, files) VALUES (?1, ?2)", params![root.to_string_lossy(), files as i64])?; Ok(()) })
    }

    /// Waiting for a reader, loading rows and opening private records may block on I/O.
    /// Hand the async worker back before taking one, as writes do before their commit.
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let read = || f(&self.readers.take());
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(read),
            _ => read(),
        }
    }

    /// The WAL emptied once a deletion took what must leave no trace (cl-984, cl-1031): on a reader's connection, every reader
    /// held so none reads meanwhile. A reader outside Calcifer that holds an older snapshot, or a write in flight, leaves it owed,
    /// said once, and the next write empties it (`write`), or the next start. When another program holds it, what a person is
    /// told: what still holds the deleted rows' pages, and until when.
    pub(crate) fn empty_wal(&self) -> Option<String> {
        let readers = self.readers.all();
        let emptied = readers.first().is_none_or(|reader| WAL.empty(reader));
        drop(readers);
        if emptied {
            return None;
        }
        // Only a holder the OS names: with none (or off Linux), the emptying may wait on a write of Calcifer's own, which retries it.
        let (pid, name) = platform::durable::wal::holder(&self.path)?;
        let who = if name.is_empty() { format!(" (pid {pid})") } else { format!(" ({name}, pid {pid})") };
        Some(format!("another program{who} has this computer's project index open, so the project's id stays in the index's write log until that program closes"))
    }

    /// One transaction, durable when it returns. Its flush can wait seconds on a busy disk's
    /// write-back: on a multi-thread runtime the worker says so first, so its other tasks (the
    /// API's among them) move on. The flush runs outside the writer lock: others commit meanwhile.
    pub(crate) fn write<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        self.write_held(None, f)
    }

    /// `write`, said slow (`crate::took`) when it held the writer `held`'s time or more, its commit included.
    fn write_held<T>(&self, held: Option<(&'static str, std::time::Duration)>, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let commit = || {
            let (value, commit) = {
                let mut conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
                let _took = held.map(|(task, slow)| crate::took::Took::over(task, slow));
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let value = f(&tx)?;
                tx.commit()?;
                (value, self.durable.committed())
            };
            self.durable.wait(commit)?;
            // An emptying the WAL owes is tried now the writer let go (`empty_wal`): by the first write after it became owed, then
            // at most once a second.
            if WAL.retry() {
                self.empty_wal();
            }
            Ok(value)
        };
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(commit),
            _ => commit(),
        }
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

    /// Keeps `text` as the newest of the machine's own lines, in place of those kept before (cl-1012).
    pub(crate) fn keep_machine_log(&self, machine: &str, text: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO machine_logs (machine, text) SELECT name, ?2 FROM machines WHERE name = ?1
                 ON CONFLICT (machine) DO UPDATE SET text = excluded.text",
                params![machine, text],
            )?;
            Ok(())
        })
    }

    /// The machine's own lines kept, if any.
    pub(crate) fn machine_log(&self, machine: &str) -> Result<Option<String>> {
        self.read(|conn| Ok(conn.query_row("SELECT text FROM machine_logs WHERE machine = ?1", [machine], |r| r.get(0)).optional()?))
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

    /// The machines not ended: what a listing shows, never every rental this computer ever had (cl-1010).
    pub(crate) fn live_machines(&self) -> Result<Vec<MachineRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!("SELECT {MACHINE_COLUMNS} FROM machines WHERE state <> 'ended' ORDER BY name"))?;
            let rows = statement.query_map([], machine_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().collect()
        })
    }

    /// The machines a name, a former name (its order's handle, or a host its Hub named before) or a rental id names, in any state;
    /// one with that very name first.
    pub(crate) fn machines_named(&self, text: &str) -> Result<Vec<MachineRow>> {
        self.read(|conn| {
            let mut statement = conn.prepare(&format!(
                "SELECT {MACHINE_COLUMNS} FROM machines
                 WHERE name = ?1 COLLATE NOCASE OR alias = ?1 COLLATE NOCASE OR rental_id = ?1 COLLATE NOCASE
                    OR rental_id IN (SELECT rental_id FROM machine_names WHERE name = ?1 COLLATE NOCASE)
                 ORDER BY name = ?1 COLLATE NOCASE DESC, rowid DESC"
            ))?;
            let rows = statement.query_map([text], machine_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().collect()
        })
    }

    /// The update by hand that holds `name` on its software, in its words (cl-952).
    pub(crate) fn held(&self, name: &str) -> Result<Option<String>> {
        self.read(|conn| {
            Ok(conn
                .query_row("SELECT held FROM machines WHERE name = ?1", [name], |r| r.get(0))
                .optional()?
                .flatten())
        })
    }

    /// Holds `name` on its software by hand (`words`, the update's), or lets it go (none).
    pub(crate) fn hold(&self, name: &str, words: Option<&str>) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE machines SET held = ?2 WHERE name = ?1", params![name, words])?;
            Ok(())
        })
    }

    /// Cancels `machine`'s updates yet to end whose key `superseded` names (cl-1196): an unsent one ends here saying `why`, a sent one
    /// keeps the intent until its machine's outcome, as any cancel. Each one's events.
    pub(crate) fn supersede(&self, machine: &str, superseded: impl Fn(&str) -> bool, why: &str) -> Result<Vec<(RunId, Vec<Event>)>> {
        self.write(|tx| {
            let ids: Vec<(String, String)> = tx
                .prepare(
                    "SELECT id, idempotency_key FROM runs WHERE machine = ?1 AND kind = 'update' AND state <> 'settled'
                         AND (?2 IS NULL OR owner = ?2) ORDER BY number",
                )?
                .query_map(params![machine, self.owner_id()], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut ended = vec![];
            for (id, _) in ids.into_iter().filter(|(_, key)| superseded(key)) {
                let events = cancel_saying(tx, &id, why)?;
                ended.push((RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into()))?, events));
            }
            Ok(ended)
        })
    }

    /// Ends `machine`'s moves to the open RC not sent yet, saying `why` (cl-1201): work came for it, which goes first, and the follower
    /// moves it again once it is idle. One sent is under way and stays: its send (`mark_sent`) and this are one journal's writes, so a
    /// move either yields or was sent. Each one ended, with its events.
    pub(crate) fn yield_moves(&self, machine: &str, why: &str) -> Result<Vec<(RunId, Vec<Event>)>> {
        self.write(|tx| {
            let ids: Vec<String> = tx
                .prepare(
                    "SELECT id FROM runs WHERE machine = ?1 AND kind = 'update' AND state = 'queued' AND cancel_requested = 0
                         AND idempotency_key LIKE ?2 AND (?3 IS NULL OR owner = ?3) ORDER BY number",
                )?
                .query_map(params![machine, format!("{}%", crate::rc::MOVE), self.owner_id()], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut ended = vec![];
            for id in ids {
                tx.execute("UPDATE runs SET cancel_requested = 1 WHERE id = ?1", [&id])?;
                let reason = Reason { code: crate::rc::YIELDED.into(), message: why.into(), ..Default::default() };
                let events = end(tx, &id, Status::Canceled, Some(reason))?;
                ended.push((RunId::try_from(id).map_err(|e| StoreError::Corrupt(e.into()))?, events));
            }
            Ok(ended)
        })
    }

    /// `machine`'s update yet to end, whoever asked it, if one is.
    pub(crate) fn update_going(&self, machine: &str) -> Result<Option<RunRow>> {
        self.read(|db| {
            let id = db.query_row(
                "SELECT id FROM runs WHERE machine = ?1 AND kind = 'update' AND state <> 'settled' ORDER BY number LIMIT 1",
                params![machine],
                |r| r.get::<_, String>(0),
            );
            id.optional()?.map_or(Ok(None), |id| run_in(db, &id))
        })
    }

    /// Whether an update of `machine` after run `number` has ended: what that run moved may be undone.
    pub(crate) fn updated_since(&self, machine: &str, number: u64) -> Result<bool> {
        self.read(|db| {
            let sql = "SELECT EXISTS(SELECT 1 FROM runs WHERE machine = ?1 AND kind = 'update' AND state = 'settled' AND number > ?2)";
            Ok(db.query_row(sql, params![machine, number], |r| r.get::<_, bool>(0))?)
        })
    }

    /// `machine`'s move to the open RC yet to end, if one is.
    pub(crate) fn moving(&self, machine: &str) -> Result<Option<RunRow>> {
        self.read(|db| {
            let id = db.query_row(
                "SELECT id FROM runs WHERE machine = ?1 AND kind = 'update' AND state <> 'settled' AND idempotency_key LIKE ?2 ORDER BY number LIMIT 1",
                params![machine, format!("{}%", crate::rc::MOVE)],
                |r| r.get::<_, String>(0),
            );
            id.optional()?.map_or(Ok(None), |id| run_in(db, &id))
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

    /// Renames a machine that has not served (its Hub named it): its runs move with it, the
    /// old name stays its alias, and every name it had finds it. An ended machine that held the name gives it up: its history
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
            // Its alias keeps only the first: a host named after a replan (cl-1125) leaves a project's runs under the one before.
            tx.execute(
                "INSERT OR REPLACE INTO machine_names (name, rental_id, at) SELECT ?1, rental_id, ?3 FROM machines WHERE name = ?2 AND rental_id IS NOT NULL",
                params![old, new, now_ms()],
            )?;
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


/// An output owed to this computer: its newest revision neither landed nor given up, no checkpoint, and none that stays on another
/// machine. A local run's latents land in its project like any output (cl-1046): an ordinary file of its take, there as long as the
/// take is, which purge takes with it; still unlisted to a person (`STAYS`).
pub(super) const OWED: &str = "outputs.exported_rev IS NOT outputs.rev AND outputs.failed_rev IS NOT outputs.rev AND outputs.checkpoint IS NULL
    AND NOT EXISTS (SELECT 1 FROM runs held, json_each(held.stays) WHERE held.id = outputs.run AND json_each.value = outputs.name
        AND held.machine IS NOT 'local')";

const OUTPUT_COLUMNS: &str = "name, idx, rev, length, digest, media_type, done, path, exported_rev,
    CASE WHEN failed_rev = rev THEN export_error END, checkpoint, run, cozy_project_root()";

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
    let root=r.get::<_,Option<String>>(12)?.map(PathBuf::from);
    let path = sql_optional_path(r, 7, &output_context(&run, &name, index, exported.unwrap_or(rev), "path"),root.as_deref())?;
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

/// The newest of each remote machine's own lines (`StatusFrame::machine_log`), so an ended one's still read (cl-1012). They go
/// with the machine's row.
const MACHINE_LOGS: &str = "
    CREATE TABLE IF NOT EXISTS machine_logs (
        machine TEXT PRIMARY KEY REFERENCES machines (name) ON DELETE CASCADE,
        text TEXT NOT NULL
    ) STRICT;";

fn put_machine_in(tx: &Transaction, machine: &MachineRow) -> Result<()> {
    let (worker, address, pin) = match &machine.endpoint {
        Some(e) => (Some(&e.worker_id), Some(&e.address), Some(e.pin.to_vec())),
        None => (None, None, None),
    };
    // A new machine under an ended one's name keeps none of its lines.
    if machine.state != MachineState::Ended {
        tx.execute(
            "DELETE FROM machine_logs WHERE machine = ?1 AND EXISTS (SELECT 1 FROM machines WHERE name = ?1 AND state = 'ended')",
            [&machine.name],
        )?;
    }
    tx.execute(
        "INSERT INTO machines (name, kind, hub, rental_id, worker_id, address, pin, key, state,
             served, reason, created_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, CASE WHEN ?9 = 'ended' THEN ?12 END)
         ON CONFLICT (name) DO UPDATE SET kind = ?2, hub = ?3, rental_id = ?4, worker_id = ?5,
             address = ?6, pin = ?7, key = ?8, state = ?9, served = served OR ?10, reason = ?11,
             held = CASE WHEN rental_id IS ?4 THEN held END, ended_at = CASE WHEN ?9 = 'ended' THEN coalesce(ended_at, ?12) END",
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
    inputs, outcome, wants, agreed, started_at, cozy_project_root(), owner, attempt";

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
         CREATE INDEX IF NOT EXISTS runs_list_metadata ON runs(number,target,state,status,machine,out_dir,id,{private});
         DROP INDEX IF EXISTS runs_settled_at;
         CREATE INDEX IF NOT EXISTS runs_warm ON runs(machine) WHERE kind = 'warm';
         DROP INDEX IF EXISTS runs_machine;
         CREATE INDEX IF NOT EXISTS runs_warm_set ON runs(machine) WHERE kind = 'warm' AND instr(spec, CAST('\"set\":{{' AS BLOB)) > 0;"
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
    let project_root=r.get::<_,Option<String>>(26)?.map(PathBuf::from);
    let warnings: String = r.get(20)?;
    let inputs: String = r.get(21)?;
    let spec: Vec<u8> = r.get(7)?;
    let outcome: Option<Vec<u8>> = r.get(22)?;
    let wants: Option<String> = r.get(23)?;
    let agreed: Option<String> = r.get(24)?;
    Ok((|| {
        let out_dir = match project_root.as_ref() {Some(root)=>crate::project::landing(root),None=>match vault::payload_root(&spec).map_err(private_error)? { Some(root)=>crate::project::landing(&root),None=>PathBuf::from(out_dir) }};
        let spec = open_bytes(&run_context(&id, "spec"), &spec)?;
        let target = open_text(&run_context(&id, "target"), &target)?;
        let warnings = open_text(&run_context(&id, "warnings"), &warnings)?;
        let inputs = open_text(&run_context(&id, "inputs"), &inputs)?;
        let reason_code = optional_text(&run_context(&id, "reason_code"), reason_code)?;
        let reason = optional_text(&run_context(&id, "reason"), reason)?;
        let outcome = outcome.map(|bytes| open_bytes(&run_context(&id, "outcome"), &bytes)).transpose()?;
        Ok(RunRow {
            project_root,
            owner:r.get(27)?,
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
            attempt: r.get::<_, i64>(28)?.max(1) as u32,
        })
    })())
}

/// Records the intent to cancel run `id` in `tx`. Unsent work ends canceled here, in the transaction that the send boundary races;
/// sent work keeps the intent until its machine's outcome arrives.
pub(crate) fn cancel_in(tx: &Transaction, id: &str) -> Result<Vec<Event>> {
    cancel_saying(tx, id, "canceled before it was sent")
}

/// `cancel_in`, an unsent run's reason saying `why`.
fn cancel_saying(tx: &Transaction, id: &str, why: &str) -> Result<Vec<Event>> {
    let Some(row) = run_in(tx, id)? else {
        return Ok(vec![]);
    };
    match row.state {
        State::Settled => Ok(vec![]),
        _ if row.cancel_requested => Ok(vec![]),
        State::Queued => {
            tx.execute("UPDATE runs SET cancel_requested = 1 WHERE id = ?1", [id])?;
            let reason = Reason { code: "canceled".into(), message: why.into(), ..Default::default() };
            end(tx, id, Status::Canceled, Some(reason))
        }
        State::Sent | State::Accepted => {
            tx.execute("UPDATE runs SET cancel_requested = 1 WHERE id = ?1", [id])?;
            Ok(vec![lifecycle(tx, id)?])
        }
    }
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
    tx.execute("UPDATE machine_logs SET machine = ?2 WHERE machine = ?1", params![from, to])?;
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
            placeholder: None,
            kind: row.kind,
            state: row.state,
            hub: row.hub.clone(),
            rental_id: row.rental_id.clone(),
            alias: None,
            address: row.endpoint.as_ref().map(|e| e.address.clone()),
            fingerprint: row.endpoint.as_ref().map(|e| Digest::from_bytes(e.pin)),
            reason: row.reason.clone(),
            broken: None,
            rental: None,
            status: None,
            unreachable: None,
            unreachable_since_ms: None,
            boot: None,
            attempt: None,
            running: 0,
            queued: 0,
            jobs: vec![],
            unverified: false,
            controller_key: None,
            reach: None,
            ending: None,
            end_failed: None,
            end_unconfirmed: None,
            rc: None,
            held: None,
            unheld: None,
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
/// Sends `row` again, its attempt's outputs not kept, saying why (`said`): after its machine's GPU failed under it, or once its
/// machine moved to a Runtime its package needs (cl-1069). `elsewhere`: an `any` run placed again (`Machines::place_unplaced`)
/// rather than to its machine: one its machine can't serve (`gpu_lost` on a rental, or a Runtime it can't move to).
fn again(tx: &Transaction, row: &RunRow, elsewhere: bool, said: &str, why: &str) -> Result<Vec<Event>> {
    let id = row.id.as_str();
    retried(tx, id, why)?;
    tx.execute(
        "UPDATE runs SET state = 'queued', attempt = ?2, cursor = 0, machine_state = NULL, accepted_at = NULL, started_at = NULL,
            machine = CASE WHEN ?3 THEN NULL ELSE machine END, wants = CASE WHEN ?3 THEN 'any' ELSE wants END,
            pinned = CASE WHEN ?3 THEN 0 ELSE pinned END
         WHERE id = ?1",
        params![id, row.attempt + 1, elsewhere],
    )?;
    let text = format!("attempt {} {said}, attempt {} of {ATTEMPTS}", row.attempt, row.attempt + 1);
    let (seq, at_ms) = (next_seq(tx, id)?, now_ms());
    tx.execute(
        "INSERT INTO run_logs (run, seq, at_ms, level, text) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, seq as i64, at_ms, "warning", seal_run_text(tx, id, &log_context(id, seq), &text)?],
    )?;
    Ok(vec![log_event(seq, at_ms, "warning".to_owned(), text), lifecycle(tx, id)?])
}

/// Why a machine's runs leave it once it ended (`reason`, its end's): said in each one's log.
pub(crate) fn ended_why(machine: &str, reason: &str) -> String {
    match reason {
        "released_by_pod" => format!("{machine} ended itself after 15 idle minutes"),
        _ => format!("{machine} shut down"),
    }
}

/// Why run `id`'s attempt before this one stopped, in one line: what its record's `retry` says (cl-1173).
fn retried(tx: &Transaction, id: &str, why: &str) -> Result<()> {
    tx.execute("INSERT OR REPLACE INTO run_retries (run, why) VALUES (?1, ?2)", params![id, seal_run_text(tx, id, &run_context(id, "retry"), why)?])?;
    Ok(())
}

/// Returns `row` to Calcifer's queue, its machine lost (cl-1132, Paul 11-17: "If that machine dies or disappears, it re-assigns that
/// queued job to another machine"), saying `why` in its log. A call goes back to be placed wherever it finishes soonest
/// (`Machines::place_unplaced`), never renting on its own; one its machine took, as its next attempt. Past its last attempt, a
/// machine's own job (an install, a warm set, an update: it was for that machine, and what asked for it places its work again), or a
/// call its caller pinned to the machine (`--on <machine>`: it measures that machine, and another may be another session's: cl-1316),
/// it fails in one line. A pin is to a machine: a rental whose order never filled (it never served) was none yet, so a call asked of
/// it is placed again, as `--on new`'s is (Paul 11-19). One its asker canceled ends canceled.
fn requeue_in(tx: &Transaction, row: &RunRow, why: &str) -> Result<Vec<Event>> {
    let id = row.id.as_str();
    if row.cancel_requested {
        return end(tx, id, Status::Canceled, Some(Reason { code: "canceled".into(), message: format!("canceled; {why}"), ..Default::default() }));
    }
    let run = match run_name(tx, row) {
        name if name.is_empty() => row.target.clone(),
        name => name,
    };
    let taken = row.state == State::Accepted;
    let lost = |message: String| end(tx, id, Status::Failed, Some(Reason { code: api::calcifer::MACHINE_LOST.into(), message, ..Default::default() }));
    if row.kind != Kind::Call {
        return lost(match taken {
            true => format!("{why}, so {run} stopped"),
            false => format!("{why} before it took {run}"),
        });
    }
    let served = |name: &str| tx.query_row("SELECT served FROM machines WHERE name = ?1", [name], |r| r.get::<_, bool>(0)).optional();
    if row.pinned && row.machine.as_deref().map(served).transpose()?.flatten().unwrap_or(false) {
        let machine = row.machine.as_deref().unwrap_or("its machine");
        return lost(match taken {
            true => format!("{why}, so {run} stopped: it was asked on {machine} (--on), so it runs nowhere else"),
            false => format!("{why} before it took {run}: it was asked on {machine} (--on), so it runs nowhere else"),
        });
    }
    if taken && row.attempt >= ATTEMPTS {
        return lost(format!("{why}, so {run} stopped on its last attempt ({ATTEMPTS} of {ATTEMPTS})"));
    }
    let attempt = row.attempt.max(1) + u32::from(taken);
    if taken {
        retried(tx, id, why)?;
    }
    tx.execute(
        "UPDATE runs SET state = 'queued', attempt = ?2, cursor = 0, machine_state = NULL, accepted_at = NULL, started_at = NULL,
            machine = NULL, wants = 'any', pinned = 0
         WHERE id = ?1",
        params![id, attempt],
    )?;
    let text = match taken {
        true => format!("attempt {} stopped: {why}; attempt {attempt} of {ATTEMPTS} goes wherever it finishes soonest", row.attempt.max(1)),
        false => format!("{why} before it took {run}; Cozy places it again, wherever it finishes soonest"),
    };
    let (seq, at_ms) = (next_seq(tx, id)?, now_ms());
    tx.execute(
        "INSERT INTO run_logs (run, seq, at_ms, level, text) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, seq as i64, at_ms, "warning", seal_run_text(tx, id, &log_context(id, seq), &text)?],
    )?;
    Ok(vec![log_event(seq, at_ms, "warning".to_owned(), text), lifecycle(tx, id)?])
}

fn lifecycle(tx: &Transaction, id: &str) -> Result<Event> {
    let seq = next_seq(tx, id)?;
    tx.execute("UPDATE runs SET state_seq = ?2 WHERE id = ?1", params![id, seq as i64])?;
    let row = run_in(tx, id)?.ok_or_else(|| StoreError::Corrupt(format!("no run {id}")))?;
    Ok(Event::lifecycle(seq, record(tx, &row)?, false))
}

/// When a run other than an update last ended here, in any journal (`at_rest`); a Calcifer counts its start as one, as a run may
/// have ended just before it.
static LAST_ENDED_MS: AtomicI64 = AtomicI64::new(0);
static STARTED_MS: std::sync::LazyLock<i64> = std::sync::LazyLock::new(now_ms);

/// Settles the run: its outputs can no longer change, and its terminal event is the last.
fn end(tx: &Transaction, id: &str, status: Status, reason: Option<Reason>) -> Result<Vec<Event>> {
    if tx.query_row("SELECT kind <> 'update' FROM runs WHERE id = ?1", [id], |r| r.get::<_, bool>(0)).optional()?.unwrap_or(true) {
        LAST_ENDED_MS.fetch_max(now_ms(), Ordering::Relaxed);
    }
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
fn output_url(row:&RunRow, name: &str, index: u32) -> String {
    let mut url = url::Url::parse("http://calcifer/").expect("a base URL");
    if let Some(root)=&row.project_root {
        url.path_segments_mut().expect("a base URL").extend(["v1","projects",&crate::project::project_id(root),"runs",row.id.as_str(),"outputs",name]);
    } else {
        url.path_segments_mut().expect("a base URL").extend(["v1","runs",&row.number.to_string(),"outputs",name]);
    }
    match index {
        0 => url.path().to_owned(),
        index => format!("{}?index={index}", url.path()),
    }
}

/// A run row as the API answers it, with its output items in the order they first appeared.
/// What names `row` to a person (cl-987): `KEY-N`, the key its project.db keeps now (a project's journal is its project.db) and its
/// number there. Calcifer's jobs are no one's runs and have no name (cl-1028): its id names one.
fn run_name(conn: &Connection, row: &RunRow) -> String {
    if row.project_root.is_none() { return String::new(); }
    let key = conn.query_row("SELECT key FROM project_keys ORDER BY ord DESC LIMIT 1", [], |r| r.get::<_, String>(0)).ok();
    key.map_or_else(|| row.number.to_string(), |key| api::calcifer::project::run_label(&key, row.number))
}

fn record(conn: &Connection, row: &RunRow) -> Result<Run> {
    let mut statement = conn.prepare(&format!(
        "SELECT name, idx, rev, length, digest, media_type, label, done,
             CASE WHEN exported_rev = rev THEN path END, CASE WHEN failed_rev = rev THEN export_error END,
             checkpoint, 0, width, height, measured_ms, duration_us
         FROM outputs WHERE run = ?1 ORDER BY created_seq"
    ))?;
    let settled_ok = row.status == Some(Status::Completed);
    // Its outputs that stay on its machine, read once: STAYS read the run's row again for each output, through its outcome (cl-1010).
    let stays: Vec<String> = conn.query_row("SELECT stays FROM runs WHERE id = ?1", [row.id.as_str()], |r| r.get::<_, String>(0)).optional()?
        .and_then(|stays| serde_json::from_str(&stays).ok()).unwrap_or_default();
    let items = statement.query_map([row.id.as_str()], |r| {
        let name: String = r.get(0)?;
        let index = r.get::<_, i64>(1)? as u32;
        let digest: String = r.get(4)?;
        let done: bool = r.get(7)?;
        let rev = r.get::<_, i64>(2)? as u64;
        let path = sql_optional_path(r, 8, &output_context(row.id.as_str(), &name, index, rev, "path"),row.project_root.as_deref())?;
        let checkpoint = checkpoint_ref(r, 10, &output_context(row.id.as_str(), &name, index, rev, "checkpoint"))?;
        let media_type: String = r.get(5)?;
        let url = (path.is_some() && checkpoint.is_none()).then(|| output_url(row, &name, index));
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
        let stays = stays.contains(&name);
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
            stays,
            checkpoint,
        })
    })?;
    let placed = (conn.query_row("SELECT placed FROM run_placements WHERE run = ?1", [row.id.as_str()], |r| r.get::<_, String>(0)).optional())
        .ok()
        .flatten()
        .and_then(|placed| serde_json::from_str(&placed).ok());
    // Past its first attempt, why the one before stopped: a store or project.db not yet given the table has none (cl-1173).
    let retry = (row.attempt > 1).then(|| Retry {
        attempt: row.attempt,
        attempts: ATTEMPTS.max(row.attempt),
        why: (conn.query_row("SELECT why FROM run_retries WHERE run = ?1", [row.id.as_str()], |r| r.get::<_, String>(0)).optional())
            .ok()
            .flatten()
            .and_then(|why| open_text(&run_context(row.id.as_str(), "retry"), &why).ok())
            .unwrap_or_default(),
    });
    Ok(Run {
        retry,
        placed,
        id: row.id.clone(),
        number: row.number,
        name: run_name(conn, row),
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
        text: vec![],
        video_progress: None,
        arriving: None,
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

impl crate::project::ProjectStore {
    journal_methods!();
}

pub(crate) enum RunStore<'a> {
    Maintenance(&'a Store),
    Project(crate::project::ProjectStore),
}
impl RunStore<'_> {
    journal_methods!();
    fn owner_id(&self)->Option<&str> { match self {Self::Maintenance(_)=>None,Self::Project(store)=>Some(store.project_id())} }
    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        match self { Self::Maintenance(store) => store.read(f), Self::Project(store) => store.read(f) }
    }
    fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        match self { Self::Maintenance(store) => store.write(f), Self::Project(store) => store.write(f) }
    }
    fn admit_submission(&self, new: &NewRun) -> Result<()> {
        match self { Self::Maintenance(store) => store.admit_submission(new), Self::Project(store) => store.admit_submission(new) }
    }
    fn require_machine(&self, name: &str) -> Result<()> {
        match self { Self::Maintenance(store) => store.require_machine(name), Self::Project(store) => store.require_machine(name) }
    }
    fn keep_machine(&self, machine: &MachineRow) -> Result<()> {
        match self { Self::Maintenance(store) => store.keep_machine(machine), Self::Project(store) => store.keep_machine(machine) }
    }
    fn rental(&self, tx: &Transaction, name: &str) -> Result<bool> {
        match self { Self::Maintenance(store) => store.rental(tx, name), Self::Project(store) => store.rental(tx, name) }
    }
}

impl crate::project::journal::SealingStore<'_> {
    journal_methods!();
}
