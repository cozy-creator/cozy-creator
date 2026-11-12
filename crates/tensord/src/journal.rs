//! Durable acceptance and transitions. Observation has no lifecycle side effects.
use platform::process::Birth;
use crate::private_journal;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

// Status observation cursor allocation, not a duration or liveness policy.
const REVISION_WINDOW: u64 = 1 << 32;
/// A root run's log keeps package log lines while it holds fewer lines than this and its
/// records fit in the bytes (`Journal::append_package_log`).
pub(crate) const PACKAGE_LOG_LINES: u64 = 512;
pub(crate) const PACKAGE_LOG_BYTES: u64 = 256 << 10;
const PACKAGE_LOG_DROPPED: &[u8] = br#"{"level":"warning","text":"later package log lines were dropped: a run's log keeps 512 lines and 256 KiB"}"#;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(crate) struct Invocation {
    pub package: String,
    pub generation: String,
    pub module: String,
    pub entrypoint: String,
    pub input: Value,
    /// Verified local encrypted workspace. It is immutable for this invocation and its children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_private: Option<api::machine::LocalPrivateStorage>,
    /// The caller's attention-kernel pin, passed to the executor verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_kernel: String,
    /// Dev-only: the Runtime's attention capture spec, passed to the executor verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_capture: String,
    /// An explicit run GPU group; zero selects the package's widest supported group.
    #[serde(default)]
    pub gpus: u32,
    /// The main model's lane; empty: the Hub's pick for the hardware.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
    /// File inputs the caller imported, verified at acceptance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputFile>,
    /// GPUs its plan runs on: 0 for a deviceless one (`accelerator=False`, or a machine
    /// without GPUs). Stamped once prepared; it schedules and fences restarts.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub degree: u32,
    /// A root's widths on this machine, smallest first, of which it takes one as it starts
    /// (cl-816). Stamped once prepared; none: it holds no GPU.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub widths: Vec<u32>,
    /// It prepares only (warm, remove): it never takes a GPU.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prepares: bool,
    /// A child run's parent (its execution id): children end with it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent: String,
    /// A child run's root: its outermost ancestor, whose acceptance orders its GPU work.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root: String,
    /// A child run's outermost ancestor on a GPU, whose GPU its calls share.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub family: String,
}

/// One file input: the declared field path, its exact bytes (sha256 and length, held in the
/// store by the caller's retention) and media type, in the caller's order.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct InputFile {
    pub input_id: String,
    pub digest: String,
    pub length: u64,
    pub media_type: String,
    pub order: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct SubmissionContext {
    pub actor: String,
    pub request_id: String,
    pub submission_id: String,
    pub expected_workspace_id: String,
    pub capture_digest: String,
    pub invocation_digest: String,
    pub payload_digest: String,
    pub preparation_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Preparation {
    pub actor: String,
    pub id: String,
    pub installation: String,
    pub document: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Installation {
    pub actor: String,
    pub alias: String,
    pub generation: String,
    pub package: String,
    pub release: String,
    pub interface: Vec<u8>,
    /// The Hub origin a published release came from; empty for local code.
    #[serde(default)]
    pub hub: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    BindingConflict,
}
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::BindingConflict => {
                "request or submission is already bound to different authored semantics"
            }
        })
    }
}
impl std::error::Error for AdmissionError {}

fn admission(error: AdmissionError) -> io::Error {
    let kind = match error {
        AdmissionError::BindingConflict => io::ErrorKind::AlreadyExists,
    };
    io::Error::new(kind, error)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Queued,
    Starting,
    Running,
    Completed,
    Failed,
    Canceled,
    /// Written by a newer machine: listed, never dispatched, settled or overwritten.
    #[serde(other)]
    Unknown,
}

impl State {
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }
    fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ExecutorFacts {
    pub pid: u32,
    pub runtime_version: String,
    pub tensorfs_version: String,
    #[serde(default)]
    pub tensord_version: String,
    #[serde(default)]
    pub tensord_commit: Option<String>,
}

/// The software that actually served one attempt, not the machine's current installation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SoftwareFacts {
    pub attempt_ordinal: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tensord: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tensord_commit: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tensorfs: String,
}

impl ExecutorFacts {
    pub(crate) fn observed(pid: u32, runtime_version: String, tensorfs_version: String) -> Self {
        Self {
            pid, runtime_version, tensorfs_version,
            tensord_version: env!("CARGO_PKG_VERSION").into(),
            tensord_commit: Some(api::COMMIT.to_owned()),
        }
    }

    pub(crate) fn software(&self, attempt_ordinal: u32) -> SoftwareFacts {
        SoftwareFacts {
            attempt_ordinal, tensord: self.tensord_version.clone(),
            tensord_commit: self.tensord_commit.clone(),
            runtime: self.runtime_version.clone(), tensorfs: self.tensorfs_version.clone(),
        }
    }
}

/// Memoized results each signer keeps, its most recently used first.
pub(crate) const MAX_HELD_MEMOS: i64 = 4096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Artifact {
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub length: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct ResultRecord {
    pub value: Value,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub asset_bindings: Vec<AssetBinding>,
    /// How its executor served the request other than as sent; a memo answer repeats them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<Adjustment>,
}

/// One request field its executor served other than as sent (the invoke reply's row).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Adjustment {
    /// The request field's path: `steps`, `assets.2.asset`.
    pub field: String,
    pub kind: AdjustmentKind,
    pub requested: String,
    pub applied: String,
    pub reason: String,
    /// `handler`, or what the Runtime applied: `policy`, `checkpoint:…`, `adapter:…`.
    pub source: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdjustmentKind {
    Adjusted,
    Clamp,
    Warn,
    /// An input the function has no use for, left out (cl-944).
    Ignored,
}

impl AdjustmentKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Adjusted => "adjusted",
            Self::Clamp => "clamp",
            Self::Warn => "warn",
            Self::Ignored => "ignored",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct AssetBinding {
    pub relative_path: String,
    pub asset_ref: String,
    pub media_type: String,
    pub length: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Execution {
    pub id: String,
    pub idempotency_key: String,
    pub invocation: Invocation,
    #[serde(default)]
    pub submission: Option<SubmissionContext>,
    pub state: State,
    pub revision: u64,
    #[serde(default)]
    pub accepted_at_ms: u64,
    #[serde(default)]
    pub finished_at_ms: u64,
    #[serde(default)]
    pub acceptance_boot_id: String,
    #[serde(default)]
    pub collected: bool,
    /// Reserved observation cursor ceiling. Older stored records default to no reservation.
    #[serde(default)]
    pub revision_ceiling: u64,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub waiting_reason: Option<String>,
    /// Why a queued run waits, for people (another run's install, its place in the queue): an
    /// observation the engine overlays, never journaled.
    #[serde(skip)]
    pub waiting: Option<String>,
    /// Live process ownership only; cleared after a proven return. `executor` retains history.
    pub process: Option<Birth>,
    pub cancel_actor: Option<String>,
    /// Explicit cancellation reserved this actor/ID before any Run spec was accepted.
    #[serde(default)]
    pub canceled_before_acceptance: bool,
    pub completed_units: u64,
    pub progress: Option<String>,
    /// When its progress last moved (Unix ms): a run in progress shows how long it has not.
    #[serde(default)]
    pub progress_at_ms: u64,
    /// The cursor at which the attempt started running: its `running` event.
    #[serde(default)]
    pub running_revision: u64,
    /// When the attempt was claimed to start: its executor's start, or one kept, runs until `started_at_ms`.
    #[serde(default)]
    pub starting_at_ms: u64,
    /// When the attempt started running (wall clock), and on which executor.
    #[serde(default)]
    pub started_at_ms: u64,
    /// The run's own time running its callable, every ended attempt summed.
    #[serde(default)]
    pub executed_ms: u64,
    /// The GPUs it took as it started: its plan's degree, or a deviceless root's lane.
    #[serde(default)]
    pub width: u32,
    /// When it waited for its weights to download (Unix ms), as its meter saw them arrive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched: Option<(u64, u64)>,
    #[serde(default)]
    pub executor: Option<ExecutorFacts>,
    pub result: Option<ResultRecord>,
    /// Private durable custody, withheld from the public result until idle or exact exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_completion: Option<PendingCompletion>,
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "CallMemo::is_empty")]
    pub memo: CallMemo,
}

/// How the machine memo (`call_memo`) met a memoizable call.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct CallMemo {
    /// `sha256:<hex>`: what answered it, or what its success is held under.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub computation: String,
    /// Answered from a result this machine held: nothing ran.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub memoized: bool,
    /// Why it was neither looked up nor held: its author's condition unmet ("seed not given").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unmemoized: String,
    /// What keyed its code (`call_memo::Basis`): `closure`, `package` or `identity`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub basis: String,
}
impl CallMemo {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

impl Execution {
    /// Its root run's acceptance order: GPU work queues by it.
    pub(crate) fn root_seq(&self) -> u64 {
        let root = if self.invocation.root.is_empty() { &self.id } else { &self.invocation.root };
        root.parse().unwrap_or(u64::MAX)
    }
    /// The run whose GPU its calls share: its outermost ancestor on a GPU, itself included.
    pub(crate) fn family(&self) -> Option<&str> {
        match (self.invocation.family.as_str(), self.invocation.degree > 0) {
            ("", true) => Some(&self.id),
            ("", false) => None,
            (family, _) => Some(family),
        }
    }
    /// Its progress is observed: an attempt running, or a run preparing inside itself.
    /// A running or queued run carries volatile observations (progress, what it waits for).
    pub(crate) fn observed(&self) -> bool {
        matches!(self.state, State::Running | State::Queued)
    }
}

/// What one completed root took: its width, its size as its input said, its compute, and what it
/// spent putting weights on its GPUs (`Ran`).
#[derive(Clone, Debug)]
pub(crate) struct RunTime {
    pub width: u32,
    pub size: api::machine::JobSize,
    pub ms: f64,
    pub load_ms: f64,
}

/// What a completed root did with its time, by its own and its calls' records and measurements
/// (cl-816, release owner 11-09). `compute_ms`: its time running less what it spent, while no call
/// computed, waiting for a GPU lane (its `gpu` log), for its calls' weights to download (`fetched`),
/// on their executors' start (claimed to running), and building weights onto the GPUs. What recurs
/// warm (a call's own preparation, its dispatch, a kept executor) is compute. `load_ms`: its own and
/// its calls' executor starts and builds, which a machine without its weights quotes again with their
/// download.
struct Ran {
    compute_ms: u64,
    load_ms: u64,
}

impl Ran {
    fn of(tx: &rusqlite::Transaction<'_>, root: &Execution) -> io::Result<Self> {
        let mut statement = tx
            .prepare("SELECT e.record,m.measurements FROM executions e LEFT JOIN run_measurements m ON m.execution=e.id WHERE json_extract(e.invocation,'$.parent')=?1")
            .map_err(db_error)?;
        let calls = statement
            .query_map([&root.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let own: Option<Vec<u8>> = tx
            .query_row("SELECT measurements FROM run_measurements WHERE execution=?1", [&root.id], |row| row.get(0))
            .optional()
            .map_err(db_error)?;
        let (mut idle, mut running, mut builds, mut loads) = (lane_waits(tx, &root.id)?, vec![], vec![], vec![]);
        let mut spent = |record: &Execution, measured: Option<&[u8]>, idle: &mut Vec<Span>| {
            let start = (record.starting_at_ms > 0).then_some((record.starting_at_ms, record.started_at_ms));
            let build = built(measured);
            builds.extend(build);
            idle.extend(start.into_iter().chain(build));
            loads.extend(start.into_iter().chain(build));
        };
        spent(root, own.as_deref(), &mut idle);
        for (call, measured) in &calls {
            let call = decoded_execution(call)?;
            // A call answered without running (memoized, refused) took no time of its own.
            if call.started_at_ms == 0 {
                continue;
            }
            running.push((call.started_at_ms, call.finished_at_ms.max(call.started_at_ms)));
            idle.extend(call.fetched);
            spent(&call, measured.as_deref(), &mut idle);
        }
        let within = |spans: &[Span], (from, to): Span| -> Vec<Span> {
            spans.iter().map(|&(a, b)| (a.max(from), b.min(to))).collect()
        };
        let window = (root.started_at_ms, root.finished_at_ms);
        let (idle, running, builds) = (within(&idle, window), within(&running, window), within(&builds, window));
        // What of `idle` no call spent computing: builds count as idle even inside a call's run.
        let lost = covered(&[&idle[..], &running[..]].concat()) - covered(&[&running[..], &builds[..]].concat()) + covered(&builds);
        Ok(Ran {
            compute_ms: root.executed_ms.saturating_sub(lost),
            load_ms: covered(&within(&loads, (root.accepted_at_ms, root.finished_at_ms))),
        })
    }
}

/// When root `id` waited for a GPU lane, as its `gpu` log says: from each `wait` to the `grant` after it.
fn lane_waits(tx: &rusqlite::Transaction<'_>, id: &str) -> io::Result<Vec<Span>> {
    let mut statement = tx.prepare("SELECT at_ms,record FROM run_logs WHERE execution=?1 ORDER BY sequence").map_err(db_error)?;
    let rows = statement
        .query_map([id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
        .map_err(db_error)?;
    let (mut waits, mut since) = (vec![], None);
    for row in rows {
        let (at, record) = row.map_err(db_error)?;
        let Ok(entry) = serde_json::from_slice::<Value>(&record) else { continue };
        match entry["gpu"]["state"].as_str() {
            Some("wait") => since = since.or(Some(at.max(0) as u64)),
            Some("grant") => waits.extend(since.take().map(|from| (from, at.max(0) as u64))),
            _ => {}
        }
    }
    Ok(waits)
}

/// A construction's build onto the GPUs, as its measurements say (`execution.construction`), Unix ms.
/// An executor serving several calls carries its build: a caller keeps what falls in its own time.
fn built(measured: Option<&[u8]>) -> Option<Span> {
    let measured: Value = serde_json::from_slice(measured?).ok()?;
    let construction = &measured["execution"]["construction"];
    match (construction["prepare_ms"].as_f64(), construction["prepared_unix_ms"].as_f64()) {
        (Some(ms), Some(end)) if ms > 0.0 => Some(((end - ms).max(0.0) as u64, end as u64)),
        _ => None,
    }
}

/// From, to: Unix ms.
type Span = (u64, u64);

/// How long `spans` cover, each instant once.
fn covered(spans: &[Span]) -> u64 {
    let mut spans: Vec<Span> = spans.iter().copied().filter(|(a, b)| b > a).collect();
    spans.sort_unstable();
    let (mut total, mut reach) = (0, 0);
    for (a, b) in spans {
        total += b.saturating_sub(a.max(reach));
        reach = reach.max(b);
    }
    total
}

/// A function's key in `root_times`: `<package>/<entrypoint>`, any release.
pub(crate) fn function_key(package: &str, entrypoint: &str) -> String {
    format!("{package}/{entrypoint}")
}

/// One journaled product of an execution's output log (`stored::RunProduct` bytes).
#[derive(Clone, Debug)]
pub(crate) struct StoredProduct {
    pub sequence: u64,
    pub at_ms: u64,
    pub product: Vec<u8>,
}

/// Coalesced observation; persisted only as part of an authoritative transition.
#[derive(Clone, Debug)]
pub(crate) struct ProgressSnapshot {
    pub completed_units: u64,
    pub detail: String,
    /// What the run waits for, while it waits (`Execution::waiting`).
    pub waiting: Option<String>,
    /// The weights it waits on while they arrive, shown in `detail` over its own detail, kept here.
    pub weights: Option<(Weights, String)>,
    pub revision: u64,
    /// When it last moved (Unix ms).
    pub at_ms: u64,
}

/// Weights arriving for a run (`access::meter`): their repositories, and the bytes here of their
/// total.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Weights {
    pub models: String,
    pub done: u64,
    pub total: u64,
}

impl Weights {
    /// The progress detail they show over the run's own `detail`: its enclosing scope (what the
    /// stage is part of) and the download, never a step its weights have not reached.
    pub(crate) fn over(&self, detail: &str) -> String {
        let own: Value = serde_json::from_str(detail).unwrap_or(Value::Null);
        let label = format!("downloading {} weights", self.models);
        let stage = match own["stage"].as_str().and_then(|stage| stage.rsplit_once(" / ")) {
            Some((scope, _)) => format!("{scope} / {label}"),
            None => label,
        };
        serde_json::json!({"stage": stage, "bytes_done": self.done, "bytes_total": self.total, "step_ms": 0.0}).to_string()
    }
}

impl ProgressSnapshot {
    pub(crate) fn overlay(&self, record: &mut Execution) -> bool {
        if self.revision <= record.revision || self.completed_units < record.completed_units {
            return false;
        }
        record.completed_units = self.completed_units;
        if !self.detail.is_empty() {
            record.progress = Some(self.detail.clone());
        }
        record.waiting = self.waiting.clone();
        record.revision = record.revision.max(self.revision);
        record.progress_at_ms = self.at_ms;
        true
    }
}

pub(crate) struct Journal {
    connection: Connection,
    workspace_id: String,
}

/// The record layout this machine writes. A newer journal still opens: unknown fields are
/// ignored and rows it cannot read are skipped, never a refusal of the whole journal.
pub(crate) const JOURNAL_FORMAT: u32 = 2;
const FILE: &str = "executions.sqlite3";
/// One unreadable row (written by a newer machine, or damaged) must not hide every other run.
fn readable(rows: Vec<String>) -> Vec<Execution> {
    rows.into_iter()
        .filter_map(|row| match decoded_execution(&row) {
            Ok(record) => Some(record),
            Err(error) => {
                tracing::warn!("journal: skipping an unreadable execution row: {error}");
                None
            }
        })
        .collect()
}

fn db_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
fn encoded(value: &impl Serialize) -> io::Result<String> {
    serde_json::to_string(value).map_err(db_error)
}
fn invocation_index(invocation: &Invocation) -> io::Result<String> {
    if invocation.local_private.is_none() { return encoded(invocation); }
    encoded(&serde_json::json!({
        "generation": invocation.generation, "degree": invocation.degree, "widths": invocation.widths,
        "parent": invocation.parent, "root": invocation.root, "family": invocation.family,
        "prepares": invocation.prepares, "local_private": invocation.local_private,
    }))
}
fn execution_context(id: &str) -> String { format!("execution:{id}") }
fn payload_context(id: &str, slot: &str) -> String { format!("execution:{id}:{slot}") }
fn encoded_execution(record: &Execution) -> io::Result<String> {
    let Some(scope) = &record.invocation.local_private else { return encoded(record); };
    let pointer = private_journal::write(Some(scope), &execution_context(&record.id), encoded(record)?.as_bytes())?;
    // All fields read by global SQL admission/recovery indexes remain scalars here. This is
    // not a dispatchable Execution and must go through decoded_execution before use.
    encoded(&serde_json::json!({
        "id": record.id, "state": record.state, "invocation": serde_json::from_str::<Value>(&invocation_index(&record.invocation)?).map_err(db_error)?,
        "waiting_reason": record.waiting_reason, "process": record.process,
        "finished_at_ms": record.finished_at_ms,
        "_private_payload": String::from_utf8(pointer).map_err(db_error)?,
    }))
}
fn decoded_execution(value: &str) -> io::Result<Execution> {
    let projection: Value = serde_json::from_str(value).map_err(db_error)?;
    if let Some(pointer) = projection.get("_private_payload") {
        let id = projection.get("id").and_then(Value::as_str).ok_or_else(|| db_error("private execution index has no id"))?;
        let pointer = pointer.as_str().ok_or_else(|| db_error("private execution reference is invalid"))?;
        let bytes = private_journal::read(&execution_context(id), pointer.as_bytes(), true)?;
        let record: Execution = serde_json::from_slice(&bytes).map_err(|_| db_error("private execution payload cannot be decoded"))?;
        let indexed: Option<api::machine::LocalPrivateStorage> = serde_json::from_value(projection["invocation"]["local_private"].clone()).map_err(|_| db_error("private execution scope index is invalid"))?;
        if record.id != id || record.invocation.local_private.is_none() || record.invocation.local_private != indexed {
            return Err(db_error("private execution scope or identity differs from its index"));
        }
        return Ok(record);
    }
    let record: Execution = serde_json::from_value(projection).map_err(db_error)?;
    if record.invocation.local_private.is_some() {
        return Err(db_error("a private execution has an unprotected global record"));
    }
    Ok(record)
}
fn private_scope(conn: &Connection, id: &str) -> io::Result<Option<api::machine::LocalPrivateStorage>> {
    let invocation: String = conn.query_row("SELECT invocation FROM executions WHERE id=?1", [id], |row| row.get(0)).map_err(db_error)?;
    let value: Value = serde_json::from_str(&invocation).map_err(db_error)?;
    value.get("local_private").cloned().map(|scope| serde_json::from_value(scope).map_err(db_error)).transpose()
}
fn write_payload(conn: &Connection, id: &str, slot: &str, body: &[u8]) -> io::Result<Vec<u8>> {
    private_journal::write(private_scope(conn, id)?.as_ref(), &payload_context(id, slot), body)
}
fn read_payload(conn: &Connection, id: &str, slot: &str, body: &[u8]) -> io::Result<Vec<u8>> {
    private_journal::read(&payload_context(id, slot), body, private_scope(conn, id)?.is_some())
}

fn metadata_actor(actor: &str) -> String {
    crate::private_io::resource_key(actor)
}
fn metadata_name(name: &str) -> String {
    if crate::private_io::current().is_some() {
        metadata_actor(&tensorfs::sha256::hex_digest(name.as_bytes()))
    } else { name.to_owned() }
}
fn encoded_metadata(value: &impl Serialize, scope: Option<&api::machine::LocalPrivateStorage>, context: &str, mut index: Value) -> io::Result<String> {
    let Some(scope) = scope else { return encoded(value) };
    let pointer = private_journal::write(Some(scope), context, encoded(value)?.as_bytes())?;
    index["_private_payload"] = String::from_utf8(pointer).map_err(db_error)?.into();
    encoded(&index)
}
fn decoded_metadata<T: serde::de::DeserializeOwned>(value: &str, context: &str) -> io::Result<T> {
    let index: Value = serde_json::from_str(value).map_err(db_error)?;
    match index.get("_private_payload") {
        Some(pointer) => {
            let pointer = pointer.as_str().ok_or_else(|| db_error("invalid private metadata reference"))?;
            let bytes = private_journal::read_scoped(context, pointer.as_bytes())?;
            serde_json::from_slice(&bytes).map_err(|_| db_error("private metadata cannot be decoded"))
        }
        None => serde_json::from_value(index).map_err(db_error),
    }
}

fn timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

impl Journal {
    pub(crate) fn open(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let connection = Connection::open(root.join(FILE)).map_err(db_error)?;
        // It holds runs' Hub capabilities (`run_specs`); its WAL and index take this mode too.
        platform::durable::set_mode(&root.join(FILE), 0o600)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS executions (
              id INTEGER PRIMARY KEY AUTOINCREMENT, idempotency_key TEXT NOT NULL UNIQUE,
              invocation TEXT NOT NULL, record TEXT NOT NULL, state TEXT NOT NULL,
              updated_ms INTEGER NOT NULL, actor TEXT, request_id TEXT, submission_id TEXT);
            CREATE INDEX IF NOT EXISTS executions_nonterminal ON executions(id)
              WHERE state IN ('queued','starting','running');
            CREATE INDEX IF NOT EXISTS executions_active ON executions(id)
              WHERE state IN ('starting','running');
            CREATE INDEX IF NOT EXISTS executions_ready ON executions(id)
              WHERE state='queued' AND json_extract(record,'$.waiting_reason') IS NULL;
            CREATE INDEX IF NOT EXISTS executions_updated ON executions(updated_ms);",
            )
            .map_err(db_error)?;
        connection.execute_batch("BEGIN; CREATE TABLE IF NOT EXISTS machine_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS update_run_ids(actor TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(actor,id));
            CREATE TABLE IF NOT EXISTS installations(actor TEXT NOT NULL,alias TEXT NOT NULL,record TEXT NOT NULL,PRIMARY KEY(actor,alias));
            CREATE TABLE IF NOT EXISTS preparations(actor TEXT NOT NULL,id TEXT NOT NULL,record TEXT NOT NULL,PRIMARY KEY(actor,id));
            CREATE TABLE IF NOT EXISTS warm_sets(actor TEXT NOT NULL,position INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(actor,position));
            CREATE TABLE IF NOT EXISTS public_terminals(execution INTEGER PRIMARY KEY REFERENCES executions(id),outcome BLOB NOT NULL,events BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS run_products(execution INTEGER NOT NULL REFERENCES executions(id),sequence INTEGER NOT NULL,at_ms INTEGER NOT NULL,product BLOB NOT NULL,PRIMARY KEY(execution,sequence));
            CREATE TABLE IF NOT EXISTS run_measurements(execution INTEGER PRIMARY KEY REFERENCES executions(id),measurements BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS run_logs(execution INTEGER NOT NULL REFERENCES executions(id),sequence INTEGER NOT NULL,at_ms INTEGER NOT NULL,record BLOB NOT NULL,PRIMARY KEY(execution,sequence));
            CREATE TABLE IF NOT EXISTS attention_applied(execution INTEGER PRIMARY KEY REFERENCES executions(id));
            CREATE TABLE IF NOT EXISTS run_log_lengths(execution INTEGER NOT NULL,sequence INTEGER NOT NULL,length INTEGER NOT NULL,PRIMARY KEY(execution,sequence));
            CREATE TABLE IF NOT EXISTS package_log_dropped(execution INTEGER PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS run_calls(execution INTEGER NOT NULL REFERENCES executions(id),sequence INTEGER NOT NULL,at_ms INTEGER NOT NULL,call TEXT NOT NULL,record BLOB NOT NULL,PRIMARY KEY(execution,sequence),UNIQUE(execution,call));
            CREATE TABLE IF NOT EXISTS native_outputs(actor TEXT NOT NULL,owner TEXT NOT NULL,source BLOB NOT NULL,PRIMARY KEY(actor,owner));
            CREATE TABLE IF NOT EXISTS triage(execution INTEGER PRIMARY KEY REFERENCES executions(id),record TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS resolutions(actor TEXT NOT NULL,key TEXT NOT NULL,package TEXT NOT NULL,preparation TEXT NOT NULL,PRIMARY KEY(actor,key));
            DROP TABLE IF EXISTS names;
            CREATE TABLE IF NOT EXISTS held_names(name TEXT PRIMARY KEY,named TEXT NOT NULL,read_ms INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS objects(actor TEXT NOT NULL,sha256 TEXT NOT NULL,length INTEGER NOT NULL,PRIMARY KEY(actor,sha256));
            CREATE TABLE IF NOT EXISTS object_uses(sha256 TEXT PRIMARY KEY,length INTEGER NOT NULL,used_ms INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS private_object_uses(project TEXT NOT NULL,sha256 TEXT NOT NULL,length INTEGER NOT NULL,used_ms INTEGER NOT NULL,PRIMARY KEY(project,sha256));
            CREATE TABLE IF NOT EXISTS run_objects(execution INTEGER NOT NULL REFERENCES executions(id),sha256 TEXT NOT NULL,length INTEGER NOT NULL,PRIMARY KEY(execution,sha256));
            CREATE INDEX IF NOT EXISTS run_objects_digest ON run_objects(sha256);
            CREATE TABLE IF NOT EXISTS run_specs(execution INTEGER PRIMARY KEY REFERENCES executions(id),spec BLOB NOT NULL);
            DROP TABLE IF EXISTS run_times;
            CREATE TABLE IF NOT EXISTS root_times(execution INTEGER PRIMARY KEY REFERENCES executions(id),function TEXT NOT NULL,width INTEGER NOT NULL,size TEXT NOT NULL,ms INTEGER NOT NULL,load_ms INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS root_times_function ON root_times(function,execution);
            CREATE TABLE IF NOT EXISTS held_memos(actor TEXT NOT NULL,computation TEXT NOT NULL,record BLOB NOT NULL,used_ms INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(actor,computation));
            CREATE TABLE IF NOT EXISTS held_memo_objects(actor TEXT NOT NULL,computation TEXT NOT NULL,sha256 TEXT NOT NULL,PRIMARY KEY(actor,computation,sha256));
            CREATE INDEX IF NOT EXISTS held_memo_objects_digest ON held_memo_objects(sha256);
            CREATE UNIQUE INDEX IF NOT EXISTS executions_actor_request ON executions(actor,request_id) WHERE actor IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS executions_actor_submission ON executions(actor,submission_id) WHERE actor IS NOT NULL;
            CREATE INDEX IF NOT EXISTS executions_actor_order ON executions(actor,id) WHERE actor IS NOT NULL; COMMIT;").map_err(db_error)?;
        let existing: Option<String> = connection
            .query_row(
                "SELECT value FROM machine_metadata WHERE key='workspace_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if existing.is_none() {
            connection
                .execute(
                    "INSERT OR IGNORE INTO machine_metadata(key,value) VALUES('workspace_id',?1)",
                    [crate::uuid::hyphenated()],
                )
                .map_err(db_error)?;
        }
        connection
            .execute(
                "INSERT OR IGNORE INTO machine_metadata(key,value) VALUES('journal_format',?1)",
                [JOURNAL_FORMAT.to_string()],
            )
            .map_err(db_error)?;
        let format: String = connection
            .query_row(
                "SELECT value FROM machine_metadata WHERE key='journal_format'",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if format
            .parse::<u32>()
            .map_or(true, |format| format > JOURNAL_FORMAT)
        {
            tracing::warn!(
                "journal: format {format} is newer than {JOURNAL_FORMAT}; reading what it can"
            );
        }
        let workspace_id = connection
            .query_row(
                "SELECT value FROM machine_metadata WHERE key='workspace_id'",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        platform::durable::sync_dir(root)?;
        Ok(Self {
            connection,
            workspace_id,
        })
    }

    pub(crate) fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Public scheduler metadata only: locking does not need to deserialize private requests.
    pub(crate) fn private_descriptor(&self, id: &str) -> io::Result<Option<api::machine::LocalPrivateStorage>> {
        private_scope(&self.connection, id)
    }

    pub(crate) fn private_active(&self, project_id: &str) -> io::Result<bool> {
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM executions WHERE state IN ('queued','starting','running') AND json_extract(invocation,'$.local_private.project_id')=?1)", [project_id], |row| row.get(0)).map_err(db_error)
    }

    pub(crate) fn installation(&self, actor: &str, alias: &str) -> io::Result<Option<Installation>> {
        let scoped = metadata_actor(actor);
        if scoped != actor {
            let local: Option<String> = self.connection.query_row("SELECT record FROM installations WHERE actor=?1 AND alias=?2", params![scoped, alias], |row| row.get(0)).optional().map_err(db_error)?;
            if let Some(local) = local { return decoded_metadata(&local, &format!("installation:{actor}:{alias}")).map(Some); }
        }
        let body: Option<String> = self.connection.query_row("SELECT record FROM installations WHERE actor=?1 AND alias=?2", params![actor, alias], |row| row.get(0)).optional().map_err(db_error)?;
        let body: Option<Installation> = body.map(|body| decoded_metadata(&body, &format!("installation:{actor}:{alias}"))).transpose()?;
        Ok(body.filter(|body| scoped == actor || !body.package.starts_with("local/")))
    }
    pub(crate) fn installation_for_generation(&self, actor: &str, generation: &str) -> io::Result<Option<Installation>> {
        let scoped = metadata_actor(actor);
        let found: Option<(String,String)> = self.connection.query_row("SELECT alias,record FROM installations WHERE actor IN (?1,?2) AND json_extract(record,'$.generation')=?3 ORDER BY CASE actor WHEN ?2 THEN 0 ELSE 1 END LIMIT 1", params![actor, scoped, generation], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(db_error)?;
        let Some((alias, body)) = found else { return Ok(None) };
        let body: Installation = decoded_metadata(&body, &format!("installation:{actor}:{alias}"))?;
        Ok((body.generation == generation && (scoped == actor || !body.package.starts_with("local/") || self.connection.query_row("SELECT EXISTS(SELECT 1 FROM installations WHERE actor=?1 AND alias=?2)",params![scoped,alias],|row|row.get::<_,bool>(0)).map_err(db_error)?)).then_some(body))
    }

    /// Every generation an installation or an unfinished run names: never evicted meanwhile.
    pub(crate) fn bound_generations(&self) -> io::Result<std::collections::HashSet<String>> {
        let mut bound = self.running_generations()?;
        bound.extend(self.generations("SELECT json_extract(record,'$.generation') FROM installations")?);
        Ok(bound)
    }
    /// Every generation an unfinished run names.
    pub(crate) fn running_generations(&self) -> io::Result<std::collections::HashSet<String>> {
        self.generations(
            "SELECT json_extract(invocation,'$.generation') FROM executions
             WHERE state NOT IN ('completed','failed','canceled')",
        )
    }
    /// Every generation a run still needs its installation for: an unfinished run's, and a finished
    /// one's whose outcome has not been read, since it is made at its first read from that
    /// installation's interface. Taken away first, the read fails for good (cl-773).
    pub(crate) fn unsettled_generations(&self) -> io::Result<std::collections::HashSet<String>> {
        self.generations(
            "SELECT json_extract(invocation,'$.generation') FROM executions
             WHERE state NOT IN ('completed','failed','canceled')
                OR (COALESCE(json_extract(invocation,'$.parent'),'')=''
                    AND id NOT IN (SELECT execution FROM public_terminals))",
        )
    }
    fn generations(&self, query: &str) -> io::Result<std::collections::HashSet<String>> {
        let mut statement = self.connection.prepare(query).map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .map_err(db_error)?;
        rows.filter_map(|row| row.map_err(db_error).transpose())
            .collect()
    }

    pub(crate) fn installations(&self, actor: &str) -> io::Result<Vec<Installation>> {
        let scoped = metadata_actor(actor);
        let mut statement = self.connection.prepare("SELECT actor,alias,record FROM installations WHERE actor IN (?1,?2) ORDER BY alias").map_err(db_error)?;
        let rows = statement.query_map(params![actor, scoped], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))).map_err(db_error)?;
        let mut found = std::collections::BTreeMap::new();
        for row in rows {
            let (owner,alias,body) = row.map_err(db_error)?;
            let body: Installation = decoded_metadata(&body,&format!("installation:{actor}:{alias}"))?;
            if scoped != actor && owner == actor && body.package.starts_with("local/") { continue; }
            found.insert(alias,body);
        }
        Ok(found.into_values().collect())
    }
    pub(crate) fn bind_installation(&mut self, record: Installation) -> io::Result<Installation> {
        validate_scope(&record.actor,&record.alias,&record.generation)?;
        let scope = crate::private_io::current().filter(|_| record.package.starts_with("local/"));
        let actor = if scope.is_some() { metadata_actor(&record.actor) } else { record.actor.clone() };
        let prior: Option<String> = self.connection.query_row("SELECT record FROM installations WHERE actor=?1 AND alias=?2",params![actor,record.alias],|row|row.get(0)).optional().map_err(db_error)?;
        if let Some(prior) = prior {
            let prior: Installation = decoded_metadata(&prior,&format!("installation:{}:{}",record.actor,record.alias))?;
            return if prior == record { Ok(prior) } else { Err(admission(AdmissionError::BindingConflict)) };
        }
        let body = encoded_metadata(&record,scope.as_ref(),&format!("installation:{}:{}",record.actor,record.alias),serde_json::json!({"generation":record.generation}))?;
        self.connection.execute("INSERT INTO installations(actor,alias,record) VALUES(?1,?2,?3)",params![actor,record.alias,body]).map_err(db_error)?;
        Ok(record)
    }

    /// Drop `actor`'s installation `alias` with its preparations, the resolutions naming them
    /// and its warm set members. Answers the installation removed, if there was one.
    pub(crate) fn remove_installation(&mut self, actor: &str, alias: &str) -> io::Result<Option<Installation>> {
        let Some(installed) = self.installation(actor, alias)? else {
            return Ok(None);
        };
        let owned_actor = if installed.package.starts_with("local/") { metadata_actor(actor) } else { actor.to_owned() };
        let actor = owned_actor.as_str();
        let tx = self.connection.transaction().map_err(db_error)?;
        for statement in [
            "DELETE FROM resolutions WHERE actor=?1 AND preparation IN
                (SELECT id FROM preparations WHERE actor=?1 AND json_extract(record,'$.installation')=?2)",
            "DELETE FROM preparations WHERE actor=?1 AND json_extract(record,'$.installation')=?2",
            "DELETE FROM warm_sets WHERE actor=?1 AND json_extract(record,'$.installation')=?2",
            "DELETE FROM installations WHERE actor=?1 AND alias=?2",
        ] {
            tx.execute(statement, params![actor, alias]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(Some(installed))
    }

    pub(crate) fn resolution(&self, actor: &str, key: &str) -> io::Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT preparation FROM resolutions WHERE actor=?1 AND key=?2",
                params![metadata_actor(actor), key],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)
    }
    pub(crate) fn bind_resolution(
        &mut self,
        actor: &str,
        key: &str,
        package: &str,
        preparation: &str,
    ) -> io::Result<()> {
        let scope = crate::private_io::current();
        let package = private_journal::write(scope.as_ref(), &format!("resolution:{actor}:{key}:package"), package.as_bytes())?;
        let package = String::from_utf8(package).map_err(db_error)?;
        self.connection
            .execute("INSERT OR REPLACE INTO resolutions(actor,key,package,preparation) VALUES(?1,?2,?3,?4)", params![metadata_actor(actor), key, package, preparation])
            .map_err(db_error)?;
        Ok(())
    }
    /// Private preparation warnings share this cache with public Hub names, so both the
    /// private value and its lookup label must stay outside the global plaintext journal.
    pub(crate) fn named(&self, name: &str) -> io::Result<Option<(String, i64)>> {
        let name = metadata_name(name);
        let row: Option<(String, i64)> = self.connection
            .query_row("SELECT named,read_ms FROM held_names WHERE name=?1", [&name], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional().map_err(db_error)?;
        row.map(|(body, read_ms)| {
            let body = private_journal::read_scoped(&format!("named:{name}"), body.as_bytes())?;
            Ok((String::from_utf8(body).map_err(db_error)?, read_ms))
        }).transpose()
    }
    /// `name` as read now.
    pub(crate) fn bind_named(&mut self, name: &str, named: &str) -> io::Result<()> {
        let name = metadata_name(name);
        let body = private_journal::write(crate::private_io::current().as_ref(), &format!("named:{name}"), named.as_bytes())?;
        let body = String::from_utf8(body).map_err(db_error)?;
        self.connection
            .execute("INSERT OR REPLACE INTO held_names(name,named,read_ms) VALUES(?1,?2,?3)", params![name, body, timestamp()])
            .map_err(db_error)?;
        Ok(())
    }
    /// This machine's catalog: how many times a held name has resolved anew. Held preparations
    /// are kept per catalog.
    pub(crate) fn catalog(&self) -> io::Result<u64> {
        let catalog: Option<String> = self.connection
            .query_row("SELECT value FROM machine_metadata WHERE key='catalog'", [], |r| r.get(0))
            .optional()
            .map_err(db_error)?;
        Ok(catalog.and_then(|c| c.parse().ok()).unwrap_or(0))
    }
    pub(crate) fn move_catalog(&mut self) -> io::Result<()> {
        let next = (self.catalog()? + 1).to_string();
        self.connection
            .execute("INSERT OR REPLACE INTO machine_metadata(key,value) VALUES('catalog',?1)", [next])
            .map_err(db_error)?;
        Ok(())
    }
    pub(crate) fn held_memo(&self, actor: &str, computation: &str) -> io::Result<Option<Vec<u8>>> {
        let record: Option<Vec<u8>> = self.connection
            .query_row("SELECT record FROM held_memos WHERE actor=?1 AND computation=?2",
                params![actor, computation], |r| r.get(0))
            .optional()
            .map_err(db_error)?;
        if record.is_some() {
            self.connection.execute("UPDATE held_memos SET used_ms=?3 WHERE actor=?1 AND computation=?2",
                params![actor, computation, timestamp()]).map_err(db_error)?;
        }
        record.map(|body| private_journal::read_scoped(&format!("memo:{actor}:{computation}"), &body)).transpose()
    }
    /// Every held memo naming `sha256` goes with it (its file is no longer this machine's).
    pub(crate) fn drop_held_memos(&mut self, sha256: &str) -> io::Result<usize> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_error)?;
        let scope = crate::private_io::current();
        let prefix = scope.as_ref().map(|scope| format!("local-private:{}:{}:", scope.project_id.len(), scope.project_id));
        let dropped = tx.execute("DELETE FROM held_memos WHERE (actor,computation) IN
            (SELECT actor,computation FROM held_memo_objects WHERE sha256=?1
             AND ((?2 IS NULL AND substr(actor,1,14)!='local-private:') OR substr(actor,1,length(?2))=?2))", params![sha256, prefix]).map_err(db_error)?;
        tx.execute("DELETE FROM held_memo_objects WHERE (actor,computation) NOT IN
            (SELECT actor,computation FROM held_memos)", []).map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(dropped)
    }
    pub(crate) fn object(&self, actor: &str, sha256: &str) -> io::Result<Option<u64>> {
        self.connection
            .query_row(
                "SELECT length FROM objects WHERE actor=?1 AND sha256=?2",
                params![actor, sha256],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)
    }
    pub(crate) fn bind_object(
        &mut self,
        actor: &str,
        object: &tensorfs::format::ObjectRef,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute(
            "INSERT OR REPLACE INTO objects(actor,sha256,length) VALUES(?1,?2,?3)",
            params![actor, object.sha256, object.length],
        )
        .map_err(db_error)?;
        touch_object(&tx, object)?;
        tx.commit().map_err(db_error)?;
        Ok(())
    }

    /// A child's result becomes its parent's dependency before the parent is handed it.
    pub(crate) fn adopt_object(
        &mut self,
        actor: &str,
        parent: &str,
        object: &tensorfs::format::ObjectRef,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let scope = private_scope(&tx, parent)?;
        let actor = crate::private_io::within(scope, || metadata_actor(actor));
        tx.execute(
            "INSERT OR REPLACE INTO objects(actor,sha256,length) VALUES(?1,?2,?3)",
            params![actor, object.sha256, object.length],
        )
        .map_err(db_error)?;
        reference_objects(&tx, parent, std::slice::from_ref(object))?;
        tx.commit().map_err(db_error)
    }

    /// The objects something needs now: used (a Write, a new run) since `since_ms`, or named by
    /// a run that is unfinished or ended since.
    pub(crate) fn needed_objects(&self, since_ms: i64) -> io::Result<Vec<tensorfs::format::ObjectRef>> {
        let scope = crate::private_io::current().map(|scope| scope.project_id);
        let mut statement = self.connection.prepare("SELECT sha256,length FROM object_uses WHERE ?2 IS NULL AND used_ms>=?1
            UNION SELECT sha256,length FROM private_object_uses WHERE project=?2 AND used_ms>=?1
            UNION SELECT r.sha256,r.length FROM run_objects r JOIN executions e ON e.id=r.execution
              WHERE ((?2 IS NULL AND json_extract(e.invocation,'$.local_private.project_id') IS NULL) OR json_extract(e.invocation,'$.local_private.project_id')=?2)
                AND (e.updated_ms>=?1 OR e.state NOT IN ('completed','failed','canceled'))").map_err(db_error)?;
        let rows = statement.query_map(params![since_ms, scope], |row| Ok(tensorfs::format::ObjectRef { sha256: row.get(0)?, length: row.get(1)? })).map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }

    /// Every retained output's record (`bind_native_output`).
    pub(crate) fn native_outputs(&self) -> io::Result<Vec<Vec<u8>>> {
        let mut statement = self.connection.prepare("SELECT actor,owner,source FROM native_outputs").map_err(db_error)?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?))).map_err(db_error)?;
        let current = crate::private_io::current();
        let mut outputs = Vec::new();
        for row in rows {
            let (actor, owner, source) = row.map_err(db_error)?;
            let scope = private_journal::scope(&source)?;
            if scope.as_ref().map(|scope| &scope.project_id) != current.as_ref().map(|scope| &scope.project_id) { continue; }
            outputs.push(private_journal::read_scoped(&format!("native:{actor}:{owner}"), &source)?);
        }
        Ok(outputs)
    }

    /// Whether nothing needs this object's root: no use (a Write, a new run) since
    /// `before_ms`, and no run naming it that is unfinished (any state but the three
    /// terminal ones) or ended since then.
    pub(crate) fn object_releasable(&self, sha256: &str, before_ms: i64) -> io::Result<bool> {
        let scope = crate::private_io::current().map(|scope| scope.project_id);
        self.connection.query_row("SELECT
          NOT EXISTS(SELECT 1 FROM object_uses WHERE ?3 IS NULL AND sha256=?1 AND used_ms>=?2)
          AND NOT EXISTS(SELECT 1 FROM private_object_uses WHERE project=?3 AND sha256=?1 AND used_ms>=?2)
          AND NOT EXISTS(SELECT 1 FROM run_objects r JOIN executions e ON e.id=r.execution WHERE r.sha256=?1
            AND ((?3 IS NULL AND json_extract(e.invocation,'$.local_private.project_id') IS NULL) OR json_extract(e.invocation,'$.local_private.project_id')=?3)
            AND (e.updated_ms>=?2 OR e.state NOT IN ('completed','failed','canceled')))", params![sha256, before_ms, scope], |row| row.get(0)).map_err(db_error)
    }

    /// The failed attempt's triage bundle reference; written before the run settles.
    pub(crate) fn bind_triage(&mut self, id: &str, triage: &crate::triage::TriageRef) -> io::Result<()> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO triage(execution,record) VALUES(?1,?2)",
                params![id, String::from_utf8(write_payload(&self.connection, id, "triage", encoded(triage)?.as_bytes())?).map_err(db_error)?],
            )
            .map_err(db_error)?;
        Ok(())
    }
    pub(crate) fn triage(&self, id: &str) -> io::Result<Option<crate::triage::TriageRef>> {
        let body: Option<String> = self.connection.query_row("SELECT record FROM triage WHERE execution=?1", [id], |row| row.get(0)).optional().map_err(db_error)?;
        body.map(|body| {
            let bytes = read_payload(&self.connection, id, "triage", body.as_bytes())?;
            serde_json::from_slice(&bytes).map_err(|_| db_error("triage reference cannot be decoded"))
        }).transpose()
    }

    pub(crate) fn preparation(&self, actor: &str, id: &str) -> io::Result<Option<Preparation>> {
        let body: Option<String> = self.connection.query_row("SELECT record FROM preparations WHERE actor=?1 AND id=?2",params![metadata_actor(actor),id],|row|row.get(0)).optional().map_err(db_error)?;
        body.map(|body| decoded_metadata(&body,&format!("preparation:{actor}:{id}"))).transpose()
    }

    /// Replace `actor`'s warm set with `members` (each one's record, in its order).
    pub(crate) fn replace_warm_set(&mut self, actor: &str, members: &[String]) -> io::Result<()> {
        let owned_actor = metadata_actor(actor);
        let scope = crate::private_io::current();
        let transaction = self.connection.transaction().map_err(db_error)?;
        transaction.execute("DELETE FROM warm_sets WHERE actor=?1", [&owned_actor]).map_err(db_error)?;
        for (position, record) in members.iter().enumerate() {
            let body = if scope.is_some() {
                let value: Value = serde_json::from_str(record).map_err(db_error)?;
                encoded_metadata(&value,scope.as_ref(),&format!("warm:{actor}:{position}"),serde_json::json!({"installation":value["installation"]}))?
            } else { record.clone() };
            transaction.execute("INSERT INTO warm_sets(actor,position,record) VALUES(?1,?2,?3)",params![owned_actor,position as i64,body]).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }
    pub(crate) fn warm_sets(&self) -> io::Result<Vec<(String,String)>> {
        let scope=crate::private_io::current();
        let prefix=scope.as_ref().map(|scope|format!("local-private:{}:{}:",scope.project_id.len(),scope.project_id));
        let mut statement=self.connection.prepare("SELECT actor,position,record FROM warm_sets ORDER BY actor,position").map_err(db_error)?;
        let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?))).map_err(db_error)?;
        let mut found=Vec::new();
        for row in rows {
            let(actor,position,body)=row.map_err(db_error)?;
            if actor.starts_with("local-private:") {
                let Some(real)=prefix.as_ref().and_then(|prefix|actor.strip_prefix(prefix)) else {continue};
                let value:Value=decoded_metadata(&body,&format!("warm:{real}:{position}"))?;
                found.push((real.to_owned(),value.to_string()));
            } else {found.push((actor,body));}
        }
        Ok(found)
    }

    /// Every preparation `actor` bound to `installation`.
    pub(crate) fn preparations_of(&self, actor: &str, installation: &str) -> io::Result<Vec<Preparation>> {
        let mut statement = self.connection.prepare("SELECT id,record FROM preparations WHERE actor=?1 AND json_extract(record,'$.installation')=?2").map_err(db_error)?;
        let rows = statement.query_map(params![metadata_actor(actor),installation],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).map_err(db_error)?;
        rows.map(|row|{let(id,body)=row.map_err(db_error)?;decoded_metadata(&body,&format!("preparation:{actor}:{id}"))}).collect()
    }
    pub(crate) fn bind_preparation(&mut self, record: Preparation) -> io::Result<Preparation> {
        validate_scope(&record.actor,&record.id,&record.installation)?;
        if self.installation(&record.actor,&record.installation)?.is_none() { return Err(admission(AdmissionError::BindingConflict)); }
        if let Some(prior)=self.preparation(&record.actor,&record.id)? {
            return if prior==record {Ok(prior)} else {Err(admission(AdmissionError::BindingConflict))};
        }
        let body=encoded_metadata(&record,crate::private_io::current().as_ref(),&format!("preparation:{}:{}",record.actor,record.id),serde_json::json!({"installation":record.installation}))?;
        self.connection.execute("INSERT INTO preparations(actor,id,record) VALUES(?1,?2,?3)",params![metadata_actor(&record.actor),record.id,body]).map_err(db_error)?;
        Ok(record)
    }

    pub(crate) fn public_terminal(&self, id: &str) -> io::Result<Option<Vec<u8>>> {
        let body: Option<Vec<u8>> = self.connection.query_row("SELECT events FROM public_terminals WHERE execution=?1", [id], |row| row.get(0)).optional().map_err(db_error)?;
        body.map(|body| read_payload(&self.connection, id, "terminal", &body)).transpose()
    }

    pub(crate) fn native_output(&self, actor: &str, owner: &str) -> io::Result<Option<Vec<u8>>> {
        let (actor,owner)=(metadata_actor(actor),metadata_actor(owner));
        let body: Option<Vec<u8>> = self.connection.query_row("SELECT source FROM native_outputs WHERE actor=?1 AND owner=?2", params![actor, owner], |row| row.get(0)).optional().map_err(db_error)?;
        body.map(|body| private_journal::read_scoped(&format!("native:{actor}:{owner}"), &body)).transpose()
    }
    pub(crate) fn native_owner(&self, owner: &str) -> io::Result<Option<String>> {
        let actor: Option<String> = self.connection.query_row("SELECT actor FROM native_outputs WHERE owner=?1 LIMIT 1",[metadata_actor(owner)],|row|row.get(0)).optional().map_err(db_error)?;
        Ok(actor.map(|actor| crate::private_io::unscoped_key(&actor)))
    }
    pub(crate) fn bind_native_output(&mut self, actor: &str, owner: &str, source: &[u8]) -> io::Result<()> {
        let (actor,owner)=(metadata_actor(actor),metadata_actor(owner));
        if self.native_owner(&owner)?.is_some_and(|held|metadata_actor(&held)!=actor) {return Err(io::Error::new(io::ErrorKind::PermissionDenied,"native retention is already owned by another actor"));}
        if let Some(prior)=self.native_output(&actor,&owner)? {
            return if prior==source {Ok(())} else {Err(admission(AdmissionError::BindingConflict))};
        }
        let body=private_journal::write(crate::private_io::current().as_ref(),&format!("native:{actor}:{owner}"),source)?;
        self.connection.execute("INSERT INTO native_outputs(actor,owner,source) VALUES(?1,?2,?3)",params![actor,owner,body]).map_err(db_error)?;
        Ok(())
    }

    pub(crate) fn commit_public_terminal(
        &mut self,
        id: &str,
        events: Vec<u8>,
    ) -> io::Result<Vec<u8>> {
        if !self.get(id)?.state.terminal() {
            return Err(db_error(
                "terminal projection requires durable terminal custody",
            ));
        }
        if let Some(prior) = self.public_terminal(id)? {
            return Ok(prior);
        }
        self.connection
            .execute(
                "INSERT INTO public_terminals(execution,outcome,events) VALUES(?1,x'',?2)",
                params![id, write_payload(&self.connection, id, "terminal", &events)?],
            )
            .map_err(db_error)?;
        Ok(events)
    }

    /// `actor`'s runs accepted as one admission, in one transaction (cl-816): each referencing its
    /// `objects` (tree members, a local source) and keeping its `spec` (empty: none) until its
    /// preparation ends, for a restart to prepare it again. Each answers the run, and whether it
    /// is new; an id that names another spec or an update, or is invalid, is refused alone.
    pub(crate) fn accept_runs(&mut self, actor: &str, runs: Vec<Accepting<'_>>) -> io::Result<Vec<io::Result<(Execution, bool)>>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut accepted = vec![];
        for run in runs {
            accepted.push(match accepting(&tx, actor, run) {
                Err(error) if matches!(error.kind(), io::ErrorKind::AlreadyExists | io::ErrorKind::InvalidInput) => Err(error),
                accepted => Ok(accepted?),
            });
        }
        tx.commit().map_err(db_error)?;
        Ok(accepted)
    }

    /// Reserves an absent run as canceled in the same transaction namespace as acceptance.
    /// An accepted run is returned for normal cancellation; its spec/outcome is never replaced.
    pub(crate) fn reserve_run_cancellation(&mut self, actor: &str, id: &str) -> io::Result<Execution> {
        validate_scope(actor, id, id)?;
        let key = format!("run:{}", encoded(&(actor, id))?);
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if update_run_reserved(&tx, actor, id)? {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "software update runs do not support Control"));
        }
        if let Some(prior) = public_prior(&tx, actor, id, id)? {
            return decoded_execution(&prior);
        }
        let context = SubmissionContext {
            actor: actor.into(), request_id: id.into(), submission_id: id.into(),
            ..Default::default()
        };
        let mut record = insert(&tx, &key, Invocation { local_private: crate::private_io::current(), ..Default::default() }, Some(context), "", None)?;
        record.state = State::Canceled;
        record.cancel_actor = Some(actor.into());
        record.canceled_before_acceptance = true;
        record.finished_at_ms = timestamp().max(0) as u64;
        tx.execute("UPDATE executions SET record=?1,state='canceled',updated_ms=?2 WHERE id=?3",
            params![encoded_execution(&record)?, timestamp(), record.id]).map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(record)
    }

    /// Updates keep their payload journal, but share actor/ID ownership with Run and CANCEL.
    pub(crate) fn reserve_update_run(&mut self, actor: &str, id: &str) -> io::Result<()> {
        validate_scope(actor, id, id)?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if public_prior(&tx, actor, id, id)?.is_some() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "this run id already names an ordinary run"));
        }
        tx.execute("INSERT OR IGNORE INTO update_run_ids(actor,id) VALUES(?1,?2)", params![actor, id])
            .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub(crate) fn update_run_reserved(&self, actor: &str, id: &str) -> io::Result<bool> {
        update_run_reserved(&self.connection, actor, id)
    }

    /// A preparing run's code and models are ready: it names them and becomes dispatchable.
    /// The prepared run names its call; a memoized one answered (`answer`) completes with it.
    pub(crate) fn bind_prepared(
        &mut self,
        id: &str,
        invocation: Invocation,
        preparation: &str,
        memo: CallMemo,
        answer: Option<ResultRecord>,
    ) -> io::Result<Execution> {
        self.update_with(
            id,
            None,
            |record| {
                if record.state != State::Queued
                    || record.waiting_reason.as_deref() != Some(PREPARING)
                {
                    return Ok(false);
                }
                let (root, family) = (std::mem::take(&mut record.invocation.root), std::mem::take(&mut record.invocation.family));
                let local_private = record.invocation.local_private.clone();
                if invocation.local_private.as_ref().is_some_and(|scope| local_private.as_ref().is_none_or(|parent| parent.project_id != scope.project_id)) {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "preparation cannot change its private workspace"));
                }
                record.invocation = Invocation { root, family, local_private, ..invocation };
                if let Some(submission) = record.submission.as_mut() {
                    submission.preparation_id = preparation.into();
                }
                record.waiting_reason = None;
                record.memo = memo;
                if let Some(result) = answer {
                    record.state = State::Completed;
                    record.result = Some(result);
                }
                Ok(true)
            },
            |tx, record| {
                tx.execute(
                    "UPDATE executions SET invocation=?1 WHERE id=?2",
                    params![invocation_index(&record.invocation)?, record.id],
                )
                .map_err(db_error)?;
                Ok(())
            },
        )
    }

    /// A preparing run ends without dispatch: a warm run's success or a preparation failure.
    pub(crate) fn end_preparation(&mut self, id: &str, outcome: Outcome) -> io::Result<Execution> {
        self.update(id, |record| {
            if record.state != State::Queued || record.waiting_reason.as_deref() != Some(PREPARING)
            {
                return Ok(false);
            }
            match outcome {
                Outcome::Completed(result) => {
                    record.state = State::Completed;
                    record.result = Some(result);
                }
                Outcome::Failed(reason) => {
                    record.state = State::Failed;
                    record.failure = Some(reason.encode());
                }
                Outcome::Canceled => record.state = State::Canceled,
            }
            record.waiting_reason = None;
            Ok(true)
        })
    }

    /// A memoizable run that called something a replay of it would skip: its success is not held.
    /// Answers whether it was going to be, and its parent, which is in the same case.
    pub(crate) fn unmemoize(&mut self, id: &str, why: &str) -> io::Result<(bool, String)> {
        let mut held = false;
        let record = self.update(id, |record| {
            held = !record.memo.computation.is_empty();
            if held {
                record.memo = CallMemo { unmemoized: why.into(), ..Default::default() };
            }
            Ok(held)
        })?;
        Ok((held, record.invocation.parent))
    }

    /// A restarted machine holds no preparation. A preparing run it kept the spec of prepares
    /// again, or waits for its access (`interrupted`). A child keeps none: its parent ended with
    /// this restart, and it ends FAILED with its parent.
    pub(crate) fn interrupt_preparations(&mut self) -> io::Result<()> {
        let preparing: Vec<i64> = {
            let mut statement = self
                .connection
                .prepare("SELECT id FROM executions WHERE state='queued' AND json_extract(record,'$.waiting_reason')=?1 AND id NOT IN (SELECT execution FROM run_specs)")
                .map_err(db_error)?;
            let rows = statement.query_map([PREPARING], |row| row.get(0)).map_err(db_error)?;
            rows.collect::<Result<_, _>>().map_err(db_error)?
        };
        for id in preparing {
            let failure = Failure {
                status: 3,
                cause: 7,
                origin: 3,
                code: "preparation_interrupted".into(),
                message: "the machine restarted while this run was preparing; run it again".into(),
            };
            self.end_preparation(&id.to_string(), Outcome::Failed(failure))?;
        }
        Ok(())
    }

    /// The runs started here and not ended, by their callers' ids: what a restart would end.
    pub(crate) fn started(&self) -> io::Result<Vec<String>> {
        let mut statement = self.connection.prepare("SELECT COALESCE(request_id, CAST(id AS TEXT)) FROM executions WHERE state IN ('starting','running') ORDER BY id").map_err(db_error)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }

    /// A preparing run waits for its access, kept with its spec.
    pub(crate) fn park(&mut self, id: &str) -> io::Result<Execution> {
        self.update(id, |record| {
            let preparing = record.state == State::Queued && record.waiting_reason.as_deref() == Some(PREPARING);
            if preparing {
                record.waiting_reason = Some(NEEDS_ACCESS.into());
            }
            Ok(preparing)
        })
    }

    /// A run waiting for its access prepares again: whether it was waiting.
    pub(crate) fn unpark(&mut self, id: &str) -> io::Result<bool> {
        let mut parked = false;
        self.update(id, |record| {
            parked = record.state == State::Queued && record.waiting_reason.as_deref() == Some(NEEDS_ACCESS);
            if parked {
                record.waiting_reason = Some(PREPARING.into());
            }
            Ok(parked)
        })?;
        Ok(parked)
    }

    /// The preparing runs a restart interrupted, each with its signer and kept spec.
    pub(crate) fn interrupted_for(&self, project_id: Option<&str>) -> io::Result<Vec<(String, String, Vec<u8>)>> {
        let mut statement = self.connection.prepare("SELECT id, actor, spec FROM executions JOIN run_specs ON execution=id WHERE state='queued' AND json_extract(record,'$.waiting_reason')=?1
            AND ((?2 IS NULL AND json_extract(invocation,'$.local_private.project_id') IS NULL) OR json_extract(invocation,'$.local_private.project_id')=?2) ORDER BY id").map_err(db_error)?;
        let rows = statement.query_map(params![PREPARING, project_id], |row| Ok((row.get::<_, i64>(0)?.to_string(), row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?))).map_err(db_error)?;
        let mut ready = Vec::new();
        for row in rows {
            let (id, actor, body) = row.map_err(db_error)?;
            match read_payload(&self.connection, &id, "spec", &body) {
                Ok(body) => ready.push((id, actor, body)),
                Err(error) => tracing::warn!(run = %id, "private or unreadable preparation remains deferred: {error}"),
            }
        }
        Ok(ready)
    }

}

/// A memoized call's success, reusable while this machine holds every file it names: its
/// result record, keyed by its signer and computation. Each signer keeps its `MAX_HELD_MEMOS`
/// most recently used.
#[derive(Clone, Debug)]
pub(crate) struct HeldMemo {
    pub actor: String,
    pub computation: String,
    pub record: Vec<u8>,
    pub objects: Vec<String>,
}
fn hold_memo(tx: &rusqlite::Transaction<'_>, held: &HeldMemo) -> io::Result<()> {
    let HeldMemo { actor, computation, record, objects } = held;
    tx.execute("INSERT OR REPLACE INTO held_memos(actor,computation,record,used_ms) VALUES(?1,?2,?3,?4)",
        params![actor, computation, record, timestamp()]).map_err(db_error)?;
    tx.execute("DELETE FROM held_memos WHERE actor=?1 AND computation NOT IN
        (SELECT computation FROM held_memos WHERE actor=?1 ORDER BY used_ms DESC LIMIT ?2)",
        params![actor, MAX_HELD_MEMOS]).map_err(db_error)?;
    tx.execute("DELETE FROM held_memo_objects WHERE actor=?1 AND computation NOT IN
        (SELECT computation FROM held_memos WHERE actor=?1)", params![actor]).map_err(db_error)?;
    tx.execute("DELETE FROM held_memo_objects WHERE actor=?1 AND computation=?2", params![actor, computation])
        .map_err(db_error)?;
    for sha256 in objects {
        tx.execute("INSERT OR IGNORE INTO held_memo_objects(actor,computation,sha256) VALUES(?1,?2,?3)",
            params![actor, computation, sha256]).map_err(db_error)?;
    }
    Ok(())
}

fn touch_object(tx: &rusqlite::Transaction<'_>, object: &tensorfs::format::ObjectRef) -> io::Result<()> {
    touch_object_scoped(tx, object, crate::private_io::current().as_ref())
}
fn touch_object_scoped(tx: &rusqlite::Transaction<'_>, object: &tensorfs::format::ObjectRef, scope: Option<&api::machine::LocalPrivateStorage>) -> io::Result<()> {
    crate::storage::check_digest("run object", &object.sha256, false)?;
    match scope {
        Some(scope) => tx.execute("INSERT INTO private_object_uses(project,sha256,length,used_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(project,sha256) DO UPDATE SET used_ms=excluded.used_ms", params![scope.project_id, object.sha256, object.length, timestamp()]),
        None => tx.execute("INSERT INTO object_uses(sha256,length,used_ms) VALUES(?1,?2,?3) ON CONFLICT(sha256) DO UPDATE SET used_ms=excluded.used_ms", params![object.sha256, object.length, timestamp()]),
    }.map(drop).map_err(db_error)
}

fn reference_objects(
    tx: &rusqlite::Transaction<'_>,
    execution: &str,
    objects: &[tensorfs::format::ObjectRef],
) -> io::Result<()> {
    let scope = private_scope(tx, execution)?;
    for object in objects {
        touch_object_scoped(tx, object, scope.as_ref())?;
        tx.execute(
            "INSERT OR IGNORE INTO run_objects(execution,sha256,length) VALUES(?1,?2,?3)",
            params![execution, object.sha256, object.length],
        )
        .map_err(db_error)?;
    }
    Ok(())
}

/// The waiting reason of a run accepted before its preparation completes.
pub(crate) const PREPARING: &str = "preparing";
/// The waiting reason of a preparing run a restart interrupted, until its sender sends it again
/// with the access that stays off disk.
pub(crate) const NEEDS_ACCESS: &str = "needs_access";

/// Not yet prepared: its spec is kept.
fn unprepared(record: &Execution) -> bool {
    record.state == State::Queued && matches!(record.waiting_reason.as_deref(), Some(PREPARING | NEEDS_ACCESS))
}

/// One run of an admission (`Journal::accept_runs`).
pub(crate) struct Accepting<'a> {
    pub id: &'a str,
    pub digest: &'a str,
    pub invocation: Invocation,
    pub objects: &'a [tensorfs::format::ObjectRef],
    pub spec: &'a [u8],
}

fn accepting(tx: &rusqlite::Transaction<'_>, actor: &str, run: Accepting<'_>) -> io::Result<(Execution, bool)> {
    let Accepting { id, digest, invocation, objects, spec } = run;
    validate_scope(actor, id, id)?;
    let key = format!("run:{}", encoded(&(actor, id))?);
    if update_run_reserved(tx, actor, id)? {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "this run id names a software update"));
    }
    let prior: Option<String> = tx
        .query_row("SELECT record FROM executions WHERE idempotency_key=?1", [&key], |row| row.get(0))
        .optional()
        .map_err(db_error)?;
    if let Some(prior) = prior {
        let execution = decoded_execution(&prior)?;
        if execution.invocation.local_private.as_ref().map(|scope| &scope.project_id) != invocation.local_private.as_ref().map(|scope| &scope.project_id) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "this run belongs to another workspace"));
        }
        let canceled = execution.canceled_before_acceptance && execution.state == State::Canceled;
        if !canceled && execution.submission.as_ref().map(|s| s.invocation_digest.as_str()) != Some(digest) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "this run id already names another run spec"));
        }
        return Ok((execution, false));
    }
    let context = SubmissionContext {
        actor: actor.into(),
        request_id: id.into(),
        submission_id: id.into(),
        expected_workspace_id: String::new(),
        capture_digest: String::new(),
        invocation_digest: digest.into(),
        payload_digest: String::new(),
        preparation_id: String::new(),
    };
    let execution = insert(tx, &key, invocation, Some(context), "", Some(PREPARING))?;
    reference_objects(tx, &execution.id, objects)?;
    if !spec.is_empty() {
        tx.execute("INSERT INTO run_specs(execution,spec) VALUES(?1,?2)", params![execution.id, write_payload(tx, &execution.id, "spec", spec)?]).map_err(db_error)?;
    }
    Ok((execution, true))
}

fn insert(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    mut invocation: Invocation,
    context: Option<SubmissionContext>,
    boot: &str,
    waiting: Option<&str>,
) -> io::Result<Execution> {
    if !invocation.parent.is_empty() {
        let parent: String = tx
            .query_row("SELECT record FROM executions WHERE id=?1", [&invocation.parent], |row| row.get(0))
            .map_err(db_error)?;
        let parent = decoded_execution(&parent)?;
        if invocation.local_private.as_ref().is_some_and(|scope| parent.invocation.local_private.as_ref().is_none_or(|parent| parent.project_id != scope.project_id)) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "a child cannot change its private workspace"));
        }
        invocation.local_private = parent.invocation.local_private.clone();
        invocation.root = parent.root_seq().to_string();
        invocation.family = parent.family().unwrap_or_default().into();
    }
    let objects: Vec<_> = invocation
        .inputs
        .iter()
        .filter_map(|input| {
            Some(tensorfs::format::ObjectRef {
                sha256: input.digest.strip_prefix("sha256:")?.into(),
                length: input.length,
            })
        })
        .collect();
    tx.execute("INSERT INTO executions(idempotency_key,invocation,record,state,updated_ms,actor,request_id,submission_id) VALUES(?1,?2,'{}','queued',?3,?4,?5,?6)", params![key, invocation_index(&invocation)?, timestamp(),context.as_ref().map(|context|context.actor.as_str()),context.as_ref().map(|context|context.request_id.as_str()),context.as_ref().map(|context|context.submission_id.as_str())]).map_err(db_error)?;
    let execution = Execution {
        id: tx.last_insert_rowid().to_string(),
        idempotency_key: key.into(),
        invocation,
        submission: context,
        state: State::Queued,
        revision: 1,
        accepted_at_ms: timestamp().max(0) as u64,
        finished_at_ms: 0,
        acceptance_boot_id: boot.into(),
        collected: false,
        revision_ceiling: 1,
        attempt: 0,
        waiting_reason: waiting.map(String::from),
        waiting: None,
        process: None,
        cancel_actor: None,
        canceled_before_acceptance: false,
        completed_units: 0,
        progress: None,
        progress_at_ms: 0,
        running_revision: 0,
        starting_at_ms: 0,
        started_at_ms: 0,
        executed_ms: 0,
        width: 0,
        fetched: None,
        executor: None,
        result: None,
        pending_completion: None,
        failure: None,
        memo: CallMemo::default(),
    };
    tx.execute(
        "UPDATE executions SET record=?1 WHERE id=?2",
        params![encoded_execution(&execution)?, execution.id],
    )
    .map_err(db_error)?;
    reference_objects(tx, &execution.id, &objects)?;
    Ok(execution)
}

impl Journal {
    pub(crate) fn get(&self, id: &str) -> io::Result<Execution> {
        let value: Option<String> = self
            .connection
            .query_row("SELECT record FROM executions WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)?;
        let value = value
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "execution does not exist"))?;
        decoded_execution(&value)
    }

    /// The run `actor` sent as `request_id`; with no actor, the one any client sent.
    pub(crate) fn get_public(&self, actor: Option<&str>, request_id: &str) -> io::Result<Execution> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT record FROM executions WHERE (actor=?1 OR ?1 IS NULL AND actor IS NOT NULL) AND request_id=?2 LIMIT 1",
                params![actor, request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        decoded_execution(&value.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "public execution does not exist for this actor",
            )
        })?)
    }

    /// Closure is a durable acceptance tombstone, never implicit execution cancellation. With no
    /// actor, every client's runs.
    pub(crate) fn actor_page(
        &self,
        actor: Option<&str>,
        after: u64,
        before: u64,
        newest: bool,
        states: &[String],
        limit: usize,
    ) -> io::Result<Vec<Execution>> {
        let states: Vec<&str> = states
            .iter()
            .map(|s| {
                if s == "succeeded" {
                    "completed"
                } else {
                    s.as_str()
                }
            })
            .collect();
        let sql=format!("SELECT record FROM executions WHERE (actor=?1 OR ?1 IS NULL AND actor IS NOT NULL) AND id>?2 AND (?3=0 OR id<?3) AND (?4='[]' OR state IN (SELECT value FROM json_each(?4))) ORDER BY id {} LIMIT ?5",if newest{"DESC"}else{"ASC"});
        let mut statement = self.connection.prepare(&sql).map_err(db_error)?;
        let records = statement
            .query_map(
                params![
                    actor,
                    after.min(i64::MAX as u64) as i64,
                    before.min(i64::MAX as u64) as i64,
                    encoded(&states)?,
                    limit.min(256) as i64
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(db_error)?;
        records
            .map(|record| record.map_err(db_error))
            .collect::<io::Result<Vec<_>>>()
            .map(readable)
    }

    /// Accepted history remains evidence of use even after every execution is terminal.
    /// Whether a job (any run asked of this machine: a call, a warm-up, an upload) is queued or
    /// running, and when the last one ended: `since` or later. One waiting for its sender's access
    /// (`NEEDS_ACCESS`) is none: a sender that never comes back leaves the machine idle.
    /// A record changed since `since` is the only one that can have stopped after it, so each
    /// sample reads only what changed.
    pub(crate) fn jobs(&self, since: i64) -> io::Result<(bool, i64)> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM executions WHERE state IN ('queued','starting','running')
                   AND COALESCE(json_extract(record,'$.waiting_reason'),'')<>?2),
                 (SELECT MAX(json_extract(record,'$.finished_at_ms')) FROM executions
                   WHERE updated_ms>=?1 AND state IN ('completed','failed','canceled'))",
                params![since, NEEDS_ACCESS],
                |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(since))),
            )
            .map_err(db_error)
    }

    pub(crate) fn nonterminal(&self, limit: usize) -> io::Result<Vec<Execution>> {
        self.selected("SELECT record FROM executions WHERE state IN ('queued','starting','running') ORDER BY id LIMIT ?1", limit)
    }

    pub(crate) fn active(&self, limit: usize) -> io::Result<Vec<Execution>> {
        self.selected("SELECT record FROM executions WHERE state IN ('starting','running') ORDER BY id LIMIT ?1", limit)
    }

    /// Every run ready to dispatch, by its root's acceptance, then its own.
    pub(crate) fn ready(&self) -> io::Result<Vec<Execution>> {
        let mut statement = self.connection.prepare("SELECT record FROM executions WHERE state='queued' AND json_extract(record,'$.waiting_reason') IS NULL ORDER BY CAST(COALESCE(NULLIF(json_extract(record,'$.invocation.root'),''),id) AS INTEGER), id").map_err(db_error)?;
        let records = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        records
            .map(|record| record.map_err(db_error))
            .collect::<io::Result<Vec<_>>>()
            .map(readable)
    }

    /// Includes terminal requests: their retained executor may still own a context.
    /// Read only birth metadata in bounded pages, never materialize old results.
    pub(crate) fn gpu_births_after(
        &self,
        after: u64,
        limit: usize,
    ) -> io::Result<Vec<(u64, Birth)>> {
        let mut statement = self.connection.prepare("SELECT id,json_extract(record,'$.process') FROM executions WHERE id>?1 AND (json_extract(record,'$.invocation.degree')>0) AND json_extract(record,'$.process') IS NOT NULL ORDER BY id LIMIT ?2").map_err(db_error)?;
        let rows = statement
            .query_map(
                params![
                    after.min(i64::MAX as u64) as i64,
                    limit.min(i64::MAX as usize) as i64
                ],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(db_error)?;
        rows.map(|row| {
            let (id, birth) = row.map_err(db_error)?;
            Ok((id, serde_json::from_str(&birth).map_err(db_error)?))
        })
        .collect()
    }

    /// What a completed root took here, for the times of later ones (`estimate`): none for a
    /// child, which its root's time holds, or for one answered without running.
    fn timed(tx: &rusqlite::Transaction<'_>, record: &Execution) -> io::Result<()> {
        let invocation = &record.invocation;
        if invocation.local_private.is_some() || !invocation.parent.is_empty() || record.executed_ms == 0 || record.memo.memoized {
            return Ok(());
        }
        let ran = Ran::of(tx, record)?;
        let size = encoded(&api::machine::JobSize::of(&invocation.input))?;
        tx.execute(
            "INSERT OR REPLACE INTO root_times(execution,function,width,size,ms,load_ms) VALUES(?1,?2,?3,?4,?5,?6)",
            params![record.id, function_key(&invocation.package, &invocation.entrypoint), record.width, size, ran.compute_ms as i64, ran.load_ms as i64],
        )
        .map_err(db_error)?;
        Ok(())
    }

    /// The newest `limit` times of `function` here (`function_key`), newest first.
    pub(crate) fn run_times(&self, function: &str, limit: usize) -> io::Result<Vec<RunTime>> {
        let mut statement = self.connection
            .prepare("SELECT width,size,ms,load_ms FROM root_times WHERE function=?1 ORDER BY execution DESC LIMIT ?2")
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![function, limit.min(i64::MAX as usize) as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?))
            })
            .map_err(db_error)?;
        rows.map(|row| {
            let (width, size, ms, load_ms) = row.map_err(db_error)?;
            Ok(RunTime { width: width as u32, size: serde_json::from_str(&size).unwrap_or_default(), ms: ms as f64, load_ms: load_ms as f64 })
        })
        .collect()
    }

    fn selected(&self, query: &str, limit: usize) -> io::Result<Vec<Execution>> {
        let mut statement = self.connection.prepare(query).map_err(db_error)?;
        let records = statement
            .query_map([limit.min(i64::MAX as usize) as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(db_error)?;
        records
            .map(|record| record.map_err(db_error))
            .collect::<io::Result<Vec<_>>>()
            .map(readable)
    }

    fn update(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Execution) -> io::Result<bool>,
    ) -> io::Result<Execution> {
        self.update_observed(id, None, change)
    }

    fn update_observed(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
        change: impl FnOnce(&mut Execution) -> io::Result<bool>,
    ) -> io::Result<Execution> {
        self.update_with(id, progress, change, |_, _| Ok(()))
    }

    /// One durable transition; `also` writes in the same transaction, after the record has
    /// its new revision, only when the transition changed it.
    fn update_with(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
        change: impl FnOnce(&mut Execution) -> io::Result<bool>,
        also: impl FnOnce(&rusqlite::Transaction<'_>, &Execution) -> io::Result<()>,
    ) -> io::Result<Execution> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let record: Option<String> = tx
            .query_row("SELECT record FROM executions WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)?;
        let mut record = decoded_execution(&record.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "execution does not exist")
        })?)?;
        if record.state == State::Unknown {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "execution state was written by a newer machine: `cozy machine update <machine>` reads it",
            ));
        }
        let was_terminal = record.state.terminal();
        let was_unprepared = unprepared(&record);
        let running_since = (record.state == State::Running).then_some(record.started_at_ms);
        let changed = change(&mut record)?;
        if let Some(since) = running_since.filter(|&s| s > 0 && record.state != State::Running) {
            let now = timestamp().max(0) as u64;
            record.executed_ms += now.saturating_sub(since);
        }
        let observed =
            !was_terminal && progress.is_some_and(|progress| progress.overlay(&mut record));
        if changed || observed {
            if record.state.terminal() && record.finished_at_ms == 0 {
                record.finished_at_ms = timestamp().max(0) as u64;
            }
            // Every durable mutation jumps past all volatile cursors the former owner
            // could have published, even when its progress snapshot was lost.
            record.revision = record
                .revision
                .max(record.revision_ceiling)
                .checked_add(1)
                .ok_or_else(|| db_error("execution observation cursor exhausted"))?;
            record.revision_ceiling = if record.observed() {
                record
                    .revision
                    .checked_add(REVISION_WINDOW)
                    .ok_or_else(|| db_error("execution observation cursor exhausted"))?
            } else {
                record.revision
            };
            tx.execute(
                "UPDATE executions SET record=?1,state=?2,updated_ms=?3 WHERE id=?4",
                params![encoded_execution(&record)?, record.state.name(), timestamp(), id],
            )
            .map_err(db_error)?;
            if was_unprepared && !unprepared(&record) {
                tx.execute("DELETE FROM run_specs WHERE execution=?1", [id]).map_err(db_error)?;
            }
            // Its time is for later quotes: one that can't be kept never fails the run.
            if !was_terminal && record.state == State::Completed {
                if let Err(error) = Self::timed(&tx, &record) {
                    tracing::warn!(run = %record.id, "its time for later estimates isn't kept: {error}");
                }
            }
            also(&tx, &record)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(record)
    }

    /// Journal one product on a started execution's output log. Its event sequence is the
    /// transition's revision, so products interleave with every other observed change.
    pub(crate) fn append_product(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
        product: &[u8],
    ) -> io::Result<u64> {
        let record = self.update_with(
            id,
            progress,
            |record| {
                if record.state.terminal() || record.state == State::Queued {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "only a running execution publishes products",
                    ));
                }
                Ok(true)
            },
            |tx, record| {
                tx.execute(
                    "INSERT INTO run_products(execution,sequence,at_ms,product) VALUES(?1,?2,?3,?4)",
                    params![id, record.revision as i64, timestamp(), write_payload(tx, id, &format!("product:{}", record.revision), product)?],
                )
                .map_err(db_error)?;
                Ok(())
            },
        )?;
        Ok(record.revision)
    }

    /// One line of the run's log the machine itself writes (a `log` event: a warning about
    /// its request), at the next sequence.
    pub(crate) fn append_log(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
        line: &[u8],
    ) -> io::Result<u64> {
        self.append_entry("INSERT INTO run_logs(execution,sequence,at_ms,record) VALUES(?1,?2,?3,?4)", id, progress, line)
    }
    /// A package's log line on a root run's log while the log holds fewer than
    /// `PACKAGE_LOG_LINES` lines and the line fits in `PACKAGE_LOG_BYTES`; past that, one
    /// `PACKAGE_LOG_DROPPED` notice and nothing more. True when a line was written.
    pub(crate) fn append_package_log(
        &mut self,
        root: &str,
        progress: Option<&ProgressSnapshot>,
        line: &[u8],
    ) -> io::Result<bool> {
        let (lines, bytes): (u64, u64) = self
            .connection
            .query_row(
                "SELECT COUNT(*),COALESCE(SUM(COALESCE(n.length,LENGTH(l.record))),0) FROM run_logs l LEFT JOIN run_log_lengths n ON n.execution=l.execution AND n.sequence=l.sequence WHERE l.execution=?1",
                [root],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_error)?;
        if lines < PACKAGE_LOG_LINES && bytes + line.len() as u64 <= PACKAGE_LOG_BYTES {
            self.append_log(root, progress, line)?;
            return Ok(true);
        }
        let noticed: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM package_log_dropped WHERE execution=?1) OR EXISTS(SELECT 1 FROM run_logs WHERE execution=?1 AND record=?2)",
                params![root, PACKAGE_LOG_DROPPED],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if noticed {
            return Ok(false);
        }
        self.append_log(root, progress, PACKAGE_LOG_DROPPED)?;
        Ok(true)
    }
    /// A call of the run applied its attention pin: the pin is the root run's, whichever call
    /// holds the attention sites.
    pub(crate) fn apply_attention(&mut self, id: &str) -> io::Result<()> {
        let root = self.get(id)?.root_seq().to_string();
        self.connection
            .execute(
                "INSERT OR IGNORE INTO attention_applied(execution) VALUES(?1)",
                [&root],
            )
            .map_err(db_error)?;
        Ok(())
    }
    /// A root run completing with a pin none of its calls applied says so in its log: an
    /// unused request field warns, never fails.
    pub(crate) fn warn_unapplied_attention(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
    ) -> io::Result<()> {
        let record = self.get(id)?;
        let pin = &record.invocation.attention_kernel;
        if pin.is_empty() || !record.invocation.parent.is_empty() || record.state.terminal() {
            return Ok(());
        }
        let applied: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM attention_applied WHERE execution=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if applied {
            return Ok(());
        }
        let text = format!("attention pin {pin} was never applied");
        let line = serde_json::to_vec(&serde_json::json!({"level": "warning", "text": text}))?;
        self.append_log(id, progress, &line).map(drop)
    }
    fn append_entry(
        &mut self,
        insert: &str,
        id: &str,
        progress: Option<&ProgressSnapshot>,
        entry: &[u8],
    ) -> io::Result<u64> {
        let record = self.update_with(
            id,
            progress,
            |record| Ok(!record.state.terminal()),
            |tx, record| {
                tx.execute(insert, params![id, record.revision as i64, timestamp(), write_payload(tx, id, &format!("log:{}", record.revision), entry)?])
                    .map_err(db_error)?;
                tx.execute("INSERT OR REPLACE INTO run_log_lengths(execution,sequence,length) VALUES(?1,?2,?3)", params![id, record.revision as i64, entry.len() as i64]).map_err(db_error)?;
                if entry == PACKAGE_LOG_DROPPED { tx.execute("INSERT OR IGNORE INTO package_log_dropped(execution) VALUES(?1)", [id]).map_err(db_error)?; }
                Ok(())
            },
        )?;
        Ok(record.revision)
    }
    /// One settled call of the run (a `call` event: a child run, or an effect such as its
    /// weights publication) at the next sequence, once per `call`; None when it is already
    /// in the log or the run has ended.
    pub(crate) fn append_call(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
        call: &str,
        body: &[u8],
    ) -> io::Result<Option<u64>> {
        let known: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM run_calls WHERE execution=?1 AND call=?2)",
                params![id, call],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if known {
            return Ok(None);
        }
        let mut appended = false;
        let record = self.update_with(
            id,
            progress,
            |record| Ok(!record.state.terminal()),
            |tx, record| {
                appended = true;
                tx.execute(
                    "INSERT INTO run_calls(execution,sequence,at_ms,call,record) VALUES(?1,?2,?3,?4,?5)",
                    params![id, record.revision as i64, timestamp(), call, write_payload(tx, id, &format!("call:{}", record.revision), body)?],
                )
                .map_err(db_error)?;
                Ok(())
            },
        )?;
        Ok(appended.then_some(record.revision))
    }
    pub(crate) fn calls(&self, id: &str) -> io::Result<Vec<StoredProduct>> {
        self.stored(
            "SELECT sequence,at_ms,record FROM run_calls WHERE execution=?1 ORDER BY sequence",
            id,
        )
    }
    pub(crate) fn logs(&self, id: &str) -> io::Result<Vec<StoredProduct>> {
        self.stored(
            "SELECT sequence,at_ms,record FROM run_logs WHERE execution=?1 ORDER BY sequence",
            id,
        )
    }

    /// What the execution's executor measured of its latest attempt (`run watch`).
    pub(crate) fn record_measurements(&mut self, id: &str, measurements: &[u8]) -> io::Result<()> {
        self.connection
            .execute(
                "INSERT INTO run_measurements(execution,measurements) VALUES(?1,?2)
                 ON CONFLICT(execution) DO UPDATE SET measurements=excluded.measurements",
                params![id, write_payload(&self.connection, id, "measurements", measurements)?],
            )
            .map(drop)
            .map_err(db_error)
    }
    pub(crate) fn measurements(&self, id: &str) -> io::Result<Option<Vec<u8>>> {
        let body: Option<Vec<u8>> = self.connection.query_row("SELECT measurements FROM run_measurements WHERE execution=?1", [id], |row| row.get(0)).optional().map_err(db_error)?;
        body.map(|body| read_payload(&self.connection, id, "measurements", &body)).transpose()
    }

    /// The execution's output log, oldest first.
    pub(crate) fn products(&self, id: &str) -> io::Result<Vec<StoredProduct>> {
        self.stored(
            "SELECT sequence,at_ms,product FROM run_products WHERE execution=?1 ORDER BY sequence",
            id,
        )
    }
    fn stored(&self, query: &str, id: &str) -> io::Result<Vec<StoredProduct>> {
        let mut statement = self.connection.prepare(query).map_err(db_error)?;
        let kind = if query.contains("run_calls") { "call" } else if query.contains("run_logs") { "log" } else { "product" };
        let rows = statement.query_map([id], |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64, row.get::<_, Vec<u8>>(2)?))).map_err(db_error)?;
        rows.map(|row| {
            let (sequence, at_ms, body) = row.map_err(db_error)?;
            Ok(StoredProduct { sequence, at_ms, product: read_payload(&self.connection, id, &format!("{kind}:{sequence}"), &body)? })
        }).collect()
    }

    /// Only one dispatcher can claim a never-started attempt, on `width` GPUs.
    pub(crate) fn claim(&mut self, id: &str, width: u32) -> io::Result<bool> {
        let mut claimed = false;
        self.update(id, |record| {
            if record.state != State::Queued {
                return Ok(false);
            }
            record.state = State::Starting;
            record.starting_at_ms = timestamp().max(0) as u64;
            record.width = width;
            record.attempt += 1;
            record.waiting_reason = None;
            claimed = true;
            Ok(true)
        })?;
        Ok(claimed)
    }

    /// A never-authorized attempt the machine's own restart interrupted is dispatched again at
    /// once: nothing of it ran, and the restart was not its failure. Called after exact
    /// termination.
    pub(crate) fn requeue_unstarted(&mut self, id: &str) -> io::Result<Execution> {
        self.update(id, |record| {
            if record.state != State::Starting || record.pending_completion.is_some() {
                return Err(db_error(
                    "only a never-authorized attempt without an outcome may return to queued",
                ));
            }
            record.process = None;
            record.waiting_reason = None;
            record.state = if record.cancel_actor.is_some() {
                State::Canceled
            } else {
                State::Queued
            };
            Ok(true)
        })
    }

    /// A job that cannot start ends FAILED with its reason, never parked to wait or queued to
    /// fail again: every job ends completed or failed, and the user resubmits.
    pub(crate) fn fail_unstarted(&mut self, id: &str, failure: Failure) -> io::Result<Execution> {
        self.update(id, |record| {
            if !matches!(record.state, State::Queued | State::Starting) {
                return Ok(false);
            }
            record.process = None;
            record.waiting_reason = None;
            record.state = if record.cancel_actor.is_some() {
                State::Canceled
            } else {
                record.failure = Some(failure.clone().encode());
                State::Failed
            };
            Ok(true)
        })
    }

    /// Authorized, but the request never reached a handler: a retained executor's channel was
    /// gone before Invoke (PrepareRequest enters none). After its exact termination the attempt
    /// stays claimed, back to awaiting its first executor.
    pub(crate) fn redeliver(&mut self, id: &str) -> io::Result<Execution> {
        self.update(id, |record| {
            if !matches!(record.state, State::Starting | State::Running) || record.pending_completion.is_some() {
                return Err(db_error("only an active attempt without an outcome is redelivered"));
            }
            record.process = None;
            record.executor = None;
            record.starting_at_ms = timestamp().max(0) as u64;
            record.started_at_ms = 0;
            record.state = if record.cancel_actor.is_some() {
                State::Canceled
            } else {
                State::Starting
            };
            Ok(true)
        })
    }

    /// Its weights downloaded from `from` to `to` (Unix ms) while it waited on them (`access::meter`).
    pub(crate) fn fetched(&mut self, id: &str, from: u64, to: u64) -> io::Result<()> {
        self.update(id, |record| {
            if record.state.terminal() {
                return Ok(false);
            }
            record.fetched = Some((from, to));
            Ok(true)
        })
        .map(drop)
    }

    /// Called once the attempt's executor exists, before its Invoke.
    pub(crate) fn register_process(&mut self, id: &str, process: Birth) -> io::Result<Execution> {
        self.update(id, |record| {
            if record.state != State::Starting || record.process.is_some() {
                return Err(db_error("attempt is not awaiting its first executor"));
            }
            record.process = Some(process);
            Ok(true)
        })
    }

    /// Reserve a log cursor before the transition's snapshot cursor. A log cannot hide a
    /// terminal/status snapshot at the same cursor; both are written in the same transaction.
    fn phase_revision(record: &mut Execution) -> io::Result<()> {
        record.revision_ceiling = record.revision.max(record.revision_ceiling)
            .checked_add(1).ok_or_else(|| db_error("execution observation cursor exhausted"))?;
        Ok(())
    }
    fn phase_log(tx: &rusqlite::Transaction<'_>, record: &Execution, mut phase: serde_json::Value) -> io::Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        phase["at_ns"] = (now.as_nanos().min(u64::MAX as u128) as u64).into();
        let line = serde_json::to_vec(&serde_json::json!({"level": "info", "text": phase.to_string()}))?;
        let sequence = record.revision.checked_sub(1).ok_or_else(|| db_error("phase cursor absent"))?;
        tx.execute("INSERT INTO run_logs(execution,sequence,at_ms,record) VALUES(?1,?2,?3,?4)",
            params![record.id, sequence as i64, timestamp(), write_payload(tx, &record.id, &format!("log:{sequence}"), &line)?]).map_err(db_error)?;
        tx.execute("INSERT OR REPLACE INTO run_log_lengths(execution,sequence,length) VALUES(?1,?2,?3)", params![record.id, sequence as i64, line.len() as i64]).map_err(db_error)?;
        Ok(())
    }

    /// The owner has proved completion and revoked this invocation's cancellation delivery.
    /// Keep historical executor facts, but no live process obligation can follow its next use.
    pub(crate) fn returned_process(&mut self, id: &str, birth: &Birth, progress: Option<&ProgressSnapshot>) -> io::Result<Execution> {
        self.update_with(id, progress, |record| {
            if !matches!(record.state, State::Starting | State::Running) || record.process.as_ref() != Some(birth) {
                return Err(db_error("returned executor does not own this active invocation"));
            }
            record.process = None;
            if let Some(pending) = &mut record.pending_completion { pending.processes.clear(); }
            Self::phase_revision(record)?;
            Ok(true)
        }, |tx, record| {
            let phase = if record.invocation.degree > 0 { "gpu_returned" } else { "executor_returned" };
            Self::phase_log(tx, record, serde_json::json!({"phase": phase, "pid": birth.pid}))
        })
    }

    pub(crate) fn running(&mut self, id: &str, executor: Option<ExecutorFacts>) -> io::Result<Execution> {
        self.update(id, |record| {
            if record.state != State::Starting || record.process.is_none() {
                return Err(db_error("attempt has no registered executor"));
            }
            if record.cancel_actor.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "stopped before package start authorization",
                ));
            }
            record.state = State::Running;
            record.running_revision = record.revision.max(record.revision_ceiling) + 1;
            record.started_at_ms = timestamp().max(0) as u64;
            record.executor = executor;
            Ok(true)
        })
    }

    /// Reserve another cursor window only when the previous one is exhausted.
    /// This rare allocation commit is not a per-event telemetry write.
    pub(crate) fn reserve_observations(
        &mut self,
        id: &str,
        progress: Option<&ProgressSnapshot>,
    ) -> io::Result<Execution> {
        self.update_observed(id, progress, |record| {
            if !record.observed() {
                return Err(db_error(
                    "only running or queued executions reserve observation cursors",
                ));
            }
            Ok(true)
        })
    }

    pub(crate) fn cancel_observed(
        &mut self,
        id: &str,
        actor: &str,
        progress: Option<&ProgressSnapshot>,
    ) -> io::Result<Execution> {
        if actor.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cancellation requires an actor",
            ));
        }
        let output = std::cell::Cell::new(false);
        self.update_with(id, progress, |record| {
            if record.state.terminal() || record.cancel_actor.is_some() {
                return Ok(false);
            }
            output.set(record.state == State::Running && record.process.is_none());
            if output.get() { Self::phase_revision(record)?; }
            record.cancel_actor = Some(actor.into());
            if record.state == State::Queued {
                record.state = State::Canceled;
                record.finished_at_ms = timestamp().max(0) as u64;
            }
            Ok(true)
        }, |tx, record| {
            if output.get() { Self::phase_log(tx, record, serde_json::json!({"phase": "output_cancelled"})) }
            else { Ok(()) }
        })
    }

    /// A run's children that have not finished.
    pub(crate) fn children(&self, parent: &str) -> io::Result<Vec<Execution>> {
        let mut statement = self
            .connection
            .prepare("SELECT record FROM executions WHERE state IN ('queued','starting','running') AND json_extract(invocation,'$.parent')=?1 ORDER BY id")
            .map_err(db_error)?;
        let records = statement
            .query_map([parent], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        records
            .map(|record| record.map_err(db_error))
            .collect::<io::Result<Vec<_>>>()
            .map(readable)
    }

    /// Hold a known outcome in the existing record, without releasing its reservation.
    pub(crate) fn defer_completion(
        &mut self, id: &str, outcome: Outcome, processes: Vec<Birth>,
        progress: Option<&ProgressSnapshot>,
    ) -> io::Result<Execution> {
        self.update_observed(id, progress, |record| {
            if record.state.terminal() || record.pending_completion.is_some() {
                return Ok(false);
            }
            if !matches!(record.state, State::Starting | State::Running) {
                return Err(db_error("only an active attempt can hold an outcome"));
            }
            if record.process.as_ref().is_none_or(|birth| !processes.contains(birth)) {
                return Err(db_error("a held outcome must name its exact executor birth"));
            }
            record.pending_completion = Some(PendingCompletion { outcome, processes });
            Ok(true)
        })
    }

    /// Caller must prove process termination and durable artifact custody first.
    pub(crate) fn finish(&mut self, id: &str, outcome: Outcome) -> io::Result<Execution> {
        self.finish_observed(id, outcome, None, None)
    }

    /// The run ends; a memoizable success's `held` memo is held in the same transaction.
    pub(crate) fn finish_observed(
        &mut self,
        id: &str,
        outcome: Outcome,
        progress: Option<&ProgressSnapshot>,
        held: Option<HeldMemo>,
    ) -> io::Result<Execution> {
        let output = std::cell::Cell::new(false);
        let hold = |tx: &rusqlite::Transaction<'_>, record: &Execution| {
            if let Some(mut held) = held.filter(|_| record.state == State::Completed) {
                held.record = private_journal::write(record.invocation.local_private.as_ref(), &format!("memo:{}:{}", held.actor, held.computation), &held.record)?;
                hold_memo(tx, &held)?;
            }
            if output.get() { Self::phase_log(tx, record, serde_json::json!({"phase": "output_finished"}))?; }
            Ok(())
        };
        self.update_with(id, progress, |record| {
            if record.state.terminal() {
                return Ok(false);
            }
            if record.state == State::Queued {
                return Err(db_error("unstarted attempt cannot finish"));
            }
            output.set(record.state == State::Running && record.process.is_none());
            if output.get() { Self::phase_revision(record)?; }
            // This is the final serialized decision, after custody: an accepted cancellation
            // cannot be replaced by success from the earlier, now detached invocation.
            let outcome = if output.get() && record.cancel_actor.is_some() { Outcome::Canceled } else { outcome };
            match outcome {
                Outcome::Completed(result) => {
                    record.state = State::Completed;
                    record.result = Some(result);
                }
                Outcome::Failed(reason) => {
                    record.state = State::Failed;
                    record.failure = Some(reason.encode());
                }
                Outcome::Canceled => {
                    if record.cancel_actor.is_none() {
                        return Err(db_error("an attempt cannot cancel without durable authority"));
                    }
                    record.state = State::Canceled;
                }
            }
            record.pending_completion = None;
            record.finished_at_ms = timestamp().max(0) as u64;
            Ok(true)
        }, hold)
    }
}

/// A retained failure carries its stable code separately from its plain detail.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Failure {
    pub status: u8,
    pub cause: u8,
    pub origin: u8,
    pub code: String,
    pub message: String,
}
impl Failure {
    pub(crate) fn executor(terminal: &str, origin: &str, code: &str, message: &str) -> Self {
        let (status, cause, origin) = match (terminal, origin, code) {
            ("refused", ..) | (_, "request", _) => (2, 1, 6),
            (_, "runtime", _) | (_, _, "device_out_of_memory") => (3, 7, 2),
            (_, "author", _) => (3, 6, 1),
            _ => (3, 7, 3),
        };
        Self {
            status,
            cause,
            origin,
            code: code.to_string(),
            message: message.to_string(),
        }
    }
    pub(crate) fn machine(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: 3,
            cause: 7,
            origin: 3,
            code: code.into(),
            message: message.into(),
        }
    }
    pub(crate) fn abandoned(why: &str) -> Self {
        Self {
            status: 5,
            cause: 16,
            origin: 3,
            code: "executor_invalidated".into(),
            message: why.into(),
        }
    }
    pub(crate) fn custody(why: &str) -> Self {
        Self {
            status: 3,
            cause: 10,
            origin: 4,
            code: "result_custody_failed".into(),
            message: why.into(),
        }
    }
    pub(crate) fn encode(&self) -> String {
        serde_json::to_string(self).expect("failure fields are JSON strings and integers")
    }
    pub(crate) fn decode(text: &str) -> io::Result<Self> {
        serde_json::from_str(text).map_err(db_error)
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum Outcome {
    Completed(ResultRecord),
    Failed(Failure),
    Canceled,
}

/// Exact process identities retained with a known result, including observed followers.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PendingCompletion {
    pub outcome: Outcome,
    pub processes: Vec<Birth>,
}
impl PendingCompletion {
    pub(crate) fn exited(&self) -> io::Result<bool> {
        // Re-read groups as well: a member born after custody was recorded may outlive its
        // leader. Unknown identities never count as exit proof.
        for birth in &self.processes {
            if !birth.ended()? {
                return Ok(false);
            }
            for member in platform::process::group_members(birth) {
                if !member.ended()? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

fn validate_scope(actor: &str, request_id: &str, submission_id: &str) -> io::Result<()> {
    if [actor, request_id, submission_id]
        .iter()
        .any(|value| value.is_empty() || value.len() > 512)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "actor, request and submission identities must contain 1..512 bytes",
        ));
    }
    Ok(())
}

fn public_prior(
    tx: &rusqlite::Transaction<'_>,
    actor: &str,
    request_id: &str,
    submission_id: &str,
) -> io::Result<Option<String>> {
    let request: Option<String> = tx
        .query_row(
            "SELECT record FROM executions WHERE actor=?1 AND request_id=?2",
            params![actor, request_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if request.is_some() {
        return Ok(request);
    }
    tx.query_row(
        "SELECT record FROM executions WHERE actor=?1 AND submission_id=?2",
        params![actor, submission_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_error)
}

fn update_run_reserved(connection: &Connection, actor: &str, id: &str) -> io::Result<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM update_run_ids WHERE actor=?1 AND id=?2)",
        params![actor, id], |row| row.get(0)).map_err(db_error)
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
