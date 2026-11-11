//! Calcifer's local API: what the CLI sends Calcifer over loopback HTTP, the records it answers,
//! and the Responses-style events of one run (SSE, and `--json` NDJSON on the CLI).
//!
//! Durable events carry a `sequence_number`, per run and monotonic; a reconnect with
//! `starting_after=N` gets the run's current lifecycle event, each output item changed after N at
//! its newest revision, and every log line after N. Live events (`run.progress`, `run.waiting`)
//! carry none and are never replayed, but the latest of each is sent on connect.
//!
//! An output's bytes land on this computer after its revision is recorded: `output_item.landed`
//! carries the item with its `path` once its file holds the item's current revision (an item
//! that will not land is `output_item.done` again, carrying `export_error`). A reconnect replays
//! a landed item as `output_item.landed`, the whole item. The terminal events `run.completed`,
//! `run.failed` and `run.canceled` end the run; its stream ends after the terminal once each of
//! its outputs has landed or carries an `export_error`, but for what stays on its machine.
use std::{collections::BTreeMap, fmt, net::SocketAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::{Digest, RunId};

pub mod hub;
pub mod project;
pub mod report;
pub mod tool;

/// What a run does on its machine (`RunSpec.kind`), or that Calcifer carries it out itself, on no machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "RunKind"))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Call,
    Warm,
    Update,
    Remove,
    /// Calcifer's own work (`hub::job`): a lock, a publication, a commit recorded at the Hub, a removal. It has no machine,
    /// and one a Calcifer stopped under fails `calcifer_stopped`: asking again is safe.
    Here,
}

/// Where a run goes: `local`, a machine's name, `any`, or `new[:sku]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(type = "string"))]
#[serde(from = "String", into = "String")]
pub enum On {
    Machine(String),
    Any,
    New(Option<String>),
}

/// `POST /v1/runs`, with an `Idempotency-Key` header: one key names one submission forever.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitRun {
    pub on: On,
    /// The Tensorhub origin the run belongs to.
    pub hub: String,
    pub kind: Kind,
    /// What it runs, for people (`org/pkg/fn@release`).
    pub target: String,
    /// The spec as it is sent to the machine, frozen at submit.
    pub spec: crate::machine::RunSpec,
    /// Local files the run reads, copied once at submit and written to its machine before the
    /// send: what `spec.inputs` names and any other object the spec refers to
    /// (`object://sha256:<hex>/<file>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<StagedInput>,
    /// Those of `spec.inputs` its machine already holds, which nothing here writes to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<Digest>,
    /// Where its outputs land: its project's `.runs`, from which each is kept in `files/`. Absent for what is no person's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<PathBuf>,
    /// What the CLI noticed while binding the run (fields it dropped, …), kept with the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    /// The quote a `new` run's renter agreed to (`POST /v1/run-quotes`): Calcifer refuses a `new`
    /// run without one, and a dearer rental rents nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<Agreed>,
    /// Its outputs that stay on its machine, by name: the result's `Latents` fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stays: Vec<String>,
}

/// `POST /v1/run-quotes`: what a `new` run would rent, and its price; when the Hub placed it, why,
/// and two other machines it would rent instead. `null` for a run that rents nothing: one Calcifer carries out itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RunQuote {
    pub sku: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<u32>,
    pub quote: Quote,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Alternative>,
    /// What the Hub's placement wants its asker to know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Another machine a run could rent, against the pick: its time as a multiple of the pick's, and
/// the Hub's one price an hour. `label` says which way it trades.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Alternative {
    pub sku: String,
    pub gpus: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub time: f64,
    pub usd_micros_per_hour: i64,
}

/// Something noticed about a run that did not stop it: a stable code and a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// A run that has not moved: since when, on which machine, and what that machine costs an hour
/// (none: not a rental, or its Hub has not said).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Stalled {
    pub since_ms: i64,
    pub machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_micros_per_hour: Option<i64>,
}

/// A local file a run reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedInput {
    pub path: PathBuf,
    pub digest: Digest,
    pub length: u64,
}

/// `POST /v1/runs` and `POST /v1/run-quotes`: a run as people ask for it, which Calcifer resolves and
/// binds against its function's interface. Under one `Idempotency-Key` it is one run, whose integer
/// `seed`, when the request omits one, derives from the key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RunRequest {
    /// `[org/]pkg/fn[@release]`; with `code`, the function of it (empty: none named). Or one of Calcifer's own jobs
    /// (`hub::job`, `cozy/…`), its fields in `input`.
    pub target: String,
    /// Local code, sent by content (cl-819): Calcifer reads no client's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<crate::code::LocalCode>,
    /// `local`, a machine's name, `any`, or `new[:sku]`. Absent, Calcifer's to say: a call goes where it finishes soonest, and a
    /// job that makes something present on a machine to this computer's, else any rental it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<On>,
    /// The Tensorhub origin; empty: Calcifer's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hub: String,
    /// The payload as an object; `terms` override it. A file field holds an uploaded file's
    /// `sha256:<hex>`, a hash prefix of a file of its project's, or a key of `paths`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, serde_json::Value>>,
    /// The payload as JSON or YAML text, its files named as `input`'s are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_text: Option<String>,
    /// `key=value` (typed as the field is declared), `key:=<json>`, `model.<slot>=<ref>`, one
    /// positional value, and input files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<String>,
    /// Attached files, each into a named input, a labelled occurrence of the declared files, or
    /// those files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileTerm>,
    /// `label-or-index=auto|low|medium|high`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fidelities: Vec<String>,
    /// `[slot=]ref[,strength]`, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loras: Vec<String>,
    /// The client's files and folders the request names by a path, each as written there, sent
    /// first by content (`PUT /v1/files`). Calcifer reads no client's disk: a path not here names no file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub paths: BTreeMap<String, Sent>,
    /// At most this many GPUs; absent: every GPU its slots support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<u32>,
    /// The main model's lane; absent: the Hub's pick for the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// `KERNEL` or `[SLOT/]COMPONENT=KERNEL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_kernel: Option<String>,
    /// Compute it afresh: no machine memo answers or keeps it (dev-only `--no-memo`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_memo: bool,
    /// Keep the function at this level instead of running it (`off` takes it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<crate::machine::WarmLevel>,
    /// The project it belongs to: its id or name, or its folder on Calcifer's computer by its absolute path. A run needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Calls a package's `quantize`, which `cozy run` refuses: `cozy model quantize` runs it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recipes: bool,
    /// The quote a `new` run's renter agreed to (`POST /v1/run-quotes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<Agreed>,
}

impl RunRequest {
    /// One of Calcifer's own jobs (`hub::job`): `target` with `input` as its fields.
    pub fn job(target: &str, input: &impl Serialize, on: Option<On>, hub: String, code: Option<crate::code::LocalCode>) -> Self {
        let input = match serde_json::to_value(input) {
            Ok(serde_json::Value::Object(input)) => Some(input),
            _ => None,
        };
        Self { target: target.to_owned(), input, on, hub, code, ..Self::default() }
    }
}

/// One attached file: `name` is an input's field path (`image`, `frames.0`) or a label, absent for
/// one of the declared files; `file` is an uploaded file's `sha256:<hex>`, a hash prefix of a file
/// of its project's, or a key of the request's `paths`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FileTerm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub file: String,
}

impl FileTerm {
    /// `--file`'s grammar, `[NAME=]FILE`: an explicit path is never split on `=`.
    pub fn parse(raw: &str) -> Self {
        let bare = || Self { name: None, file: raw.to_owned() };
        if crate::target::written_as_path(raw) {
            return bare();
        }
        match raw.split_once('=') {
            Some((name, file)) => Self { name: Some(name.trim().to_owned()), file: file.trim().to_owned() },
            None => bare(),
        }
    }
}

/// A file or folder a client sent by content (`PUT /v1/files`): a file's digest, or a folder's
/// tree document's (`api::tree`), sent after each file it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Sent {
    File(Digest),
    Tree(Digest),
}

/// What a path is sent under in a request's `paths`: as written, without the `@` a file may be written with.
pub fn path_key(written: &str) -> &str {
    let written = written.trim();
    written.strip_prefix('@').unwrap_or(written).trim()
}

/// `PUT /v1/files/sha256:<hex>`: a file Calcifer holds for runs to name, by its content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "HeldFile"))]
pub struct File {
    pub digest: Digest,
    pub length: u64,
    pub media_type: String,
}

/// `GET /v1/targets?target=`: what a run of `target` calls, resolved as `POST /v1/runs` resolves it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Target {
    /// `org/pkg`, or local code's `local/<name>`.
    pub package: String,
    /// The function, as the interface spells it; absent: the target names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The release, or local code's source digest.
    pub release: String,
    pub release_from: ReleaseFrom,
    /// What the package does, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The package's interface document (`cozy.package.interface/1`).
    pub interface: serde_json::Value,
    /// What a person runs of it, in the order offered: no internal function or recipe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<TargetFunction>,
    /// The named function's contract as people read it (`cozy run … --describe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    /// The named function's request schema (`--describe --json`), with the release and why when it is a published one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
    /// Commands to run next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
}

/// One function a person runs of a package, and what it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TargetFunction {
    pub function: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Why a run binds and runs the release it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ReleaseFrom {
    /// The target named it.
    Asked,
    /// The newest release its machine holds that isn't yanked.
    Installed,
    /// The Hub's newest: its machine holds none, so it installs this.
    Latest,
    /// Local code: its source as it is now.
    Local,
}

impl fmt::Display for ReleaseFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ReleaseFrom::Asked => "asked",
            ReleaseFrom::Installed => "installed",
            ReleaseFrom::Latest => "the Hub's latest",
            ReleaseFrom::Local => "local",
        })
    }
}

/// The answer to a submit: 202 when this call recorded the run, 200 (`replay`) when the key had.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Submitted {
    pub run: Run,
    pub replay: bool,
    /// What resolving and binding the request said, in order (dropped fields, cut clips, a machine
    /// without a GPU), for its client to show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// `run` makes the target ready (local code's install): follow it, then send the same request
    /// under the same key again for the call.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preparing: bool,
}

/// Something a request's handling said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Note {
    pub level: NoteLevel,
    pub message: String,
}

/// A note's weight: information or a warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NoteLevel {
    Info,
    Warning,
}

/// Where a run is: queued, in progress, or completed, failed or canceled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "RunStatus"))]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Not yet accepted by a machine (still in the outbox, or sent and unconfirmed).
    Queued,
    InProgress,
    Completed,
    Failed,
    Canceled,
}

/// One run as Calcifer holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Run {
    /// Its id; `<machine>/<number>` for one another client of this account sent.
    pub id: RunId,
    /// This controller's number for it; 0 for one another client sent.
    pub number: u64,
    /// One another client of this account sent, as the machine it runs on numbers it: read there,
    /// never held here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_number: Option<u64>,
    pub kind: Kind,
    pub target: String,
    pub hub: String,
    pub machine: Option<String>,
    /// The name of the project its outputs land in; none for what is no person's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// That project's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub status: Status,
    #[serde(default)]
    pub cancel_requested: bool,
    /// The machine's own word for the run: `preparing`, `queued`, `running`, ….
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_state: Option<String>,
    /// Why a run that did not succeed ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(default)]
    pub output: Vec<OutputItem>,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at_ms: Option<i64>,
    /// When its machine began executing it; its time counts from here, never its queue's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at_ms: Option<i64>,
    /// Live: why a queued run waits, from Calcifer's memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    /// Live: the machine's latest progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// A run waiting for its turn: its place in its machine's line (1 is next: only the work
    /// running is ahead of it), or among the runs waiting for a machine to be chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    /// How its machine served the request other than as sent, once settled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<Adjustment>,
    /// The result's canonical JSON, once settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_ms: Option<u64>,
    /// A memoizable run's computation, once settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<Memo>,
    /// With `?detail=full`: what the machine measured (stages, steps, GPUs, attention).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurements: Option<serde_json::Value>,
    /// It has made no progress for a while. Shown and said once, never acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<Stalled>,
    /// With `?detail=full`: the run's settled calls, in the order they ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<Call>,
    /// With `?detail=full`: its measurements and its calls', worked out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::calcifer::report::Report>,
    /// With `?detail=full`: what was asked of it, to show and to ask again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RunAsked>,
    /// The GPU it ran on, by name, once settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

impl Run {
    /// What names it to a person: its number, or `<machine>/<number>` for one another client sent.
    pub fn name(&self) -> String {
        match self.machine_number {
            Some(_) => self.id.to_string(),
            None => self.number.to_string(),
        }
    }
}

/// What a run was asked, as it was bound. Read-only: asking it again is a new `POST /v1/runs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RunAsked {
    pub target: String,
    /// Its payload, each file by its `sha256:`, and `models` for the models and LoRAs it chose: what `input` takes.
    pub input: serde_json::Map<String, serde_json::Value>,
    /// Its integer seed, given or derived from its key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
}

/// One request field a run's executor served other than as sent (`Outcome.adjustments`).
pub use crate::machine::Adjustment;

/// A memoizable run's or call's computation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Memo {
    /// `sha256:<hex>`.
    pub computation: String,
    /// Answered from a result its machine held: nothing ran.
    pub memoized: bool,
    /// What keyed its code (`machine::Outcome::memo_basis`); empty from a machine that predates it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub basis: String,
}

/// One settled call of a run (the run's own function or a child call).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Call {
    pub run: String,
    pub index: u32,
    pub function: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub status: String,
    pub called_at_ms: i64,
    pub finished_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurements: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(default)]
    pub memoized: bool,
    /// What keyed a memoizable call's code (`Memo::basis`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memo_basis: String,
}

/// Why a run or call did not succeed, as its machine said it (with its traceback) or as
/// Calcifer did.
pub use crate::machine::Reason;

/// Where an output is: still arriving, complete, or final in a run that did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Incomplete,
}

/// One output of a run at its newest revision, rewritten in place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OutputItem {
    /// `<number>/<name>`, or `<number>/<name>/<i>` for a list's item `i`.
    pub id: String,
    pub name: String,
    /// A list item's 1-based index; 0 for a single output.
    pub index: u32,
    pub rev: u64,
    pub length: u64,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub status: ItemStatus,
    /// The final bytes' digest, once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Digest>,
    /// Calcifer's own: the file holding this revision's bytes once they landed. Never on the wire: a client fetches `url`.
    #[serde(skip)]
    pub path: Option<PathBuf>,
    /// Where Calcifer serves those bytes once they landed, an API path; a folder's is its tree document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A picture's or video's size, measured as this revision landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A video's or sound's length: measured as this revision landed, else its machine's word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Why this revision's bytes will not land (its machine ended, or holds them no more).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_error: Option<String>,
    /// A checkpoint already written to TensorFS; it has no output bytes to export here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<crate::machine::CheckpointRef>,
    /// Untyped bytes, such as a model's latents, of a run that made something else: its machine keeps them, they aren't
    /// brought here unless something needs them, and nothing waits for them. A person isn't told of them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stays: bool,
}

impl OutputItem {
    /// A checkpoint reference needs no local byte export.
    pub fn is_checkpoint(&self) -> bool {
        self.checkpoint.is_some()
    }

    /// Still owed to Calcifer: its last revision neither landed nor failed.
    pub fn owed(&self) -> bool {
        !self.is_checkpoint() && !self.stays && self.url.is_none() && self.export_error.is_none()
    }
}

/// An export filename with complete output name/index identity, shared by Calcifer and client downloads.
/// Sanitizing or truncating the readable part cannot merge distinct slots; TensorFS objects stay content-addressed.
pub fn output_file_name(number: u64, name: &str, index: u32, media_type: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let mut identity = Sha256::new();
    identity.update((name.len() as u64).to_le_bytes());
    identity.update(name.as_bytes());
    identity.update(index.to_le_bytes());
    let identity = hex::encode(identity.finalize());
    let name: String = (name.chars().take(128))
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    let stem = match index {
        0 => format!("{number}-{name}"),
        i => format!("{number}-{name}-{i}"),
    };
    let media_type = media_type.split(';').next().unwrap_or("").trim();
    let extension = match media_type.to_ascii_lowercase().as_str() {
        "image/png" => ".png",
        "image/jpeg" | "image/jpg" => ".jpg",
        "image/webp" => ".webp",
        "image/gif" => ".gif",
        "image/avif" => ".avif",
        "video/mp4" => ".mp4",
        "video/webm" => ".webm",
        "audio/wav" | "audio/x-wav" | "audio/wave" => ".wav",
        "audio/mpeg" => ".mp3",
        "audio/flac" => ".flac",
        "audio/ogg" => ".ogg",
        "application/json" => ".json",
        "text/plain" => ".txt",
        _ => "",
    };
    format!("{stem}-{identity}{extension}")
}

/// The machine's progress report, as it last sent it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Progress {
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// How far this stage is, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_fraction: Option<f64>,
    #[serde(default)]
    pub completed: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub bytes_total: u64,
    #[serde(default)]
    pub step_ms: f64,
    /// The stage's steady step time: the median of its last five steps after the first; 0 before
    /// its second.
    #[serde(default)]
    pub step_steady_ms: f64,
    /// When the machine last saw it move (Unix ms; 0: unknown): how long a run has not.
    #[serde(default)]
    pub at_ms: i64,
    /// How far the run is, as Calcifer reads it: its own fraction, else its stage's, else its
    /// steps', else its bytes'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_fraction: Option<f64>,
    /// The stage's bytes a second over its last 10 s, once a second of them is seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_per_s: Option<f64>,
    /// The seconds the run has left, all of it, as Calcifer estimates them (CALCIFER-API.md "Time
    /// left"): what runs like it took before, and its own steps as they come. None: no basis yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_s: Option<i64>,
    /// How far to trust `eta_s`, 0 to 1: higher with closer history and more of the run done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_confidence: Option<f64>,
    /// What `eta_s` stands on; `first_run` with none: nothing like it ran on this machine or its GPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_basis: Option<EtaBasis>,
}

/// What a time left stands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum EtaBasis {
    /// Runs like it on this machine, and its own steps as they come.
    Machine,
    /// Runs like it on other machines of this GPU, and its own steps.
    Gpu,
    /// Its own steps alone: nothing like it ran here.
    Steps,
    /// Nothing yet: no run like it here or on this GPU, and fewer than two of its own steps.
    FirstRun,
}

/// `POST /v1/run-estimates` (cl-816): when each machine a run may go to would finish it, as each
/// says now. Nothing is sent or reserved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunEstimate {
    /// One per machine that answered, for each copy in turn, the soonest finish first.
    pub machines: Vec<MachineEstimate>,
    /// Where the run would go: the soonest finish, a rental before `local` on a tie.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soonest: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// Local code's install, still running: follow it, then ask again under the same key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preparing: Option<Box<Run>>,
}

/// One machine's quote for a run, or for one of `count` copies of it. Times are ms from the answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MachineEstimate {
    pub machine: String,
    /// Which copy, 0 first.
    pub job: u32,
    /// It can run there at all; one that can't says why (`refusal`) and has no times.
    pub fits: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Reason>,
    /// The GPUs it would run on; 0: none.
    pub width: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_ms: Option<u64>,
    /// Its code's install and, its weights not there, their download and load: part of `start_ms`.
    pub prepare_ms: u64,
    /// How many runs start there before it.
    pub queue_position: u32,
    pub basis: crate::machine::Basis,
}

/// One event of a run's stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "RunEvent"))]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "run.created")]
    Created { sequence_number: u64, run: Run },
    #[serde(rename = "run.queued")]
    Queued { sequence_number: u64, run: Run },
    #[serde(rename = "run.in_progress")]
    InProgress { sequence_number: u64, run: Run },
    #[serde(rename = "run.completed")]
    Completed { sequence_number: u64, run: Run },
    #[serde(rename = "run.failed")]
    Failed { sequence_number: u64, run: Run },
    #[serde(rename = "run.canceled")]
    Canceled { sequence_number: u64, run: Run },
    #[serde(rename = "output_item.added")]
    OutputAdded {
        sequence_number: u64,
        output_index: u32,
        item: OutputItem,
    },
    #[serde(rename = "output_item.delta")]
    OutputDelta {
        sequence_number: u64,
        output_index: u32,
        item_id: String,
        rev: u64,
        length: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        appended_from: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<crate::machine::CheckpointRef>,
    },
    #[serde(rename = "output_item.done")]
    OutputDone {
        sequence_number: u64,
        output_index: u32,
        item: OutputItem,
    },
    /// The item landed in Calcifer at its current revision (`item.url`).
    #[serde(rename = "output_item.landed")]
    OutputLanded {
        sequence_number: u64,
        output_index: u32,
        item: OutputItem,
    },
    #[serde(rename = "run.log")]
    Log {
        sequence_number: u64,
        at_ms: i64,
        level: String,
        text: String,
    },
    /// A settled call of the run (its own function's or a child's).
    #[serde(rename = "run.call")]
    Call { sequence_number: u64, call: Call },
    #[serde(rename = "run.progress")]
    Progress { progress: Progress },
    /// Why the run waits now; an empty reason: it waits no longer.
    #[serde(rename = "run.waiting")]
    Waiting { reason: String },
}

impl Event {
    /// The lifecycle event that says `run` as it is now; `created` for its first.
    pub fn lifecycle(sequence_number: u64, run: Run, created: bool) -> Self {
        match run.status {
            Status::Queued if created => Event::Created {
                sequence_number,
                run,
            },
            Status::Queued => Event::Queued {
                sequence_number,
                run,
            },
            Status::InProgress => Event::InProgress {
                sequence_number,
                run,
            },
            Status::Completed => Event::Completed {
                sequence_number,
                run,
            },
            Status::Failed => Event::Failed {
                sequence_number,
                run,
            },
            Status::Canceled => Event::Canceled {
                sequence_number,
                run,
            },
        }
    }

    /// The event's place in its run's durable stream; `None` for a live event.
    pub fn sequence_number(&self) -> Option<u64> {
        match self {
            Event::Created {
                sequence_number, ..
            }
            | Event::Queued {
                sequence_number, ..
            }
            | Event::InProgress {
                sequence_number, ..
            }
            | Event::Completed {
                sequence_number, ..
            }
            | Event::Failed {
                sequence_number, ..
            }
            | Event::Canceled {
                sequence_number, ..
            }
            | Event::OutputAdded {
                sequence_number, ..
            }
            | Event::OutputDelta {
                sequence_number, ..
            }
            | Event::OutputDone {
                sequence_number, ..
            }
            | Event::OutputLanded {
                sequence_number, ..
            }
            | Event::Log {
                sequence_number, ..
            }
            | Event::Call {
                sequence_number, ..
            } => Some(*sequence_number),
            Event::Progress { .. } | Event::Waiting { .. } => None,
        }
    }

    /// Whether the run ends here: nothing about it changes after it but its outputs landing.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Event::Completed { .. } | Event::Failed { .. } | Event::Canceled { .. }
        )
    }
}

/// `GET /v1/runs`, newest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RunList {
    pub runs: Vec<Run>,
    /// How many more runs match beyond the limit.
    #[serde(default)]
    pub older: u64,
    /// The machine `?machine=` named, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Matching runs the store cannot read, each as `run N: why`, left out of `runs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    /// With `?machine=`, newest first: its runs other clients of this account sent, as it reports
    /// them now (`Run::machine_number`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<Run>,
}

/// `PUT /v1/events/{stream}`: the projects a page on that stream shows, which Calcifer holds open
/// while it lasts or until it shows others.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    pub projects: Vec<String>,
}

/// The Readers installed by `PUT /v1/events/{stream}`. IDs not in `held` are not acknowledged;
/// at most 64 IDs are attempted, and attempted IDs that could not open retain their refusal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectShowReply {
    pub held: Vec<String>,
    pub refused: BTreeMap<String, ProjectShowRefusal>,
}

/// One requested project's refusal, independent of the other projects on the same stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectShowRefusal {
    pub status: u16,
    pub error: crate::error::Error,
}

/// A project event's position in one controller process. Generations never compare across
/// restarts; a full snapshot covers only positions captured before its read began.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectEventCursor {
    pub generation: String,
    pub sequence: u64,
}

/// `GET /v1/events`: what a client keeps on screen, on one stream, each thing whole as its route
/// answers it. No backlog: a client subscribes, then reads what it holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type")]
pub enum StreamEvent {
    /// First on every stream. `stream` names it for `PUT /v1/events/{stream}`.
    #[serde(rename = "hello")]
    Hello { version: String, stream: String },
    #[serde(rename = "run.updated")]
    RunUpdated { run: Box<Run> },
    #[serde(rename = "machine.updated")]
    MachineUpdated { machine: Box<Machine> },
    /// It left the list.
    #[serde(rename = "machine.removed")]
    MachineRemoved { name: String },
    /// `rev` is opaque, and changes whenever project.yaml or project.lock does. Optional
    /// `cursor` is its emission position, comparable only with the same controller generation.
    #[serde(rename = "project.changed")]
    ProjectChanged {
        id: String,
        rev: String,
        /// The assets whose row is another now, each to read by `GET /v1/projects/{id}/assets/{name}`. Absent: read the whole project.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assets: Option<Vec<String>>,
        /// Those gone from it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        gone: Vec<String>,
        /// What's said of the project besides its assets changed: `GET /v1/projects/{id}?assets=false`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        head: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<ProjectEventCursor>,
    },
    /// The rows of a project's canvas that changed, at revision `seq`; none is a row gone.
    #[serde(rename = "canvas")]
    Canvas { id: String, seq: u64, cards: BTreeMap<String, Option<crate::calcifer::project::Card>> },
    /// One job of a project's render as it stands now, whole; `GET /v1/projects/{id}` has them all under `board`.
    #[serde(rename = "job.updated")]
    JobUpdated { project: String, job: Box<crate::calcifer::project::Job>, seq: u64 },
    /// What a machine is fetching ahead of a project's queued jobs, as it stands now.
    #[serde(rename = "ahead.updated")]
    AheadUpdated { project: String, ahead: Box<crate::calcifer::project::Ahead> },
    /// A project's history moved: an edit, an undo or a redo. `undo` and `redo` are what each would take now.
    #[serde(rename = "history")]
    History {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undo: Option<Box<crate::calcifer::project::HistoryEntry>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redo: Option<Box<crate::calcifer::project::HistoryEntry>>,
    },
    /// This stream fell behind: read again what you hold.
    #[serde(rename = "resync")]
    Resync,
}

/// What a machine is to Calcifer: this computer's, a rental, or another computer's added by address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    /// This computer's machine.
    Local,
    /// A machine rented through a Hub.
    Rental,
    /// Another computer's machine, added by address.
    Added,
}

/// Where a machine is in its life, as Calcifer sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    /// No address yet: runs to it wait in the outbox.
    Booting,
    Ready,
    /// Its end is asked of its Hub, which has not yet affirmed it.
    Ending,
    Ended,
    /// `local` is not running: it starts when something uses it.
    Stopped,
    /// A state this reader does not know (a newer Calcifer's): shown as unknown, never refused.
    #[serde(other)]
    Unknown,
}

/// One machine as Calcifer knows it: its kind, state, address, rental, latest status and runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Machine {
    pub name: String,
    pub kind: MachineKind,
    pub state: MachineState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rental_id: Option<String>,
    /// The name it had before its Hub named it; it keeps resolving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The sha256 of its pinned TLS leaf: what `cozy machine add … --fingerprint` takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Digest>,
    /// Why it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why a ready machine is offline, by its Hub's word (its provider stopped it, or its Hub is
    /// ending it), while it does not answer: runs to it are refused. Its state stays `ready`,
    /// which is what a reader that predates this field shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<String>,
    /// The Hub's word, for a rental.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rental: Option<Rental>,
    /// How to log in, for a rental rented with an SSH key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<Ssh>,
    /// The machine's own latest Status, once it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MachineStatus>,
    /// Why its Status cannot be read now (`status` is then the last it read), and since when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable_since_ms: Option<i64>,
    /// A booting rental's phase, as its Hub says it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<String>,
    /// A booting rental's acquisition attempt, as its Hub says it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Attempt>,
    /// Calcifer's runs on it.
    #[serde(default)]
    pub running: u32,
    #[serde(default)]
    pub queued: u32,
    /// False: a rental another client of this account ordered. This controller reaches it as its
    /// own (the Hub grants it every device the account signed in on), but places no run on it unasked.
    #[serde(default = "yes")]
    pub ordered: bool,
    /// Its Hub did not answer the last listing.
    #[serde(default)]
    pub unverified: bool,
    /// On `local`: this controller's public key, what another computer's `cozy machine authorize`
    /// takes to let this one use its machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_key: Option<String>,
    /// On `local`: where other computers reach it, what their `cozy machine add` takes (a pod's
    /// public address; this computer's LAN address while it listens for authorized controllers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<String>,
    /// What an ending machine waits for before its end is carried out: its runs' outputs to land
    /// here. None once its end is under way, or when nothing holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<String>,
}

fn yes() -> bool {
    true
}

/// A duration as a clock reads: `41s`, `14m`, `1h12m`.
pub fn clock(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// A booting rental's acquisition attempt: the first is 1. `state` is the Hub's: obligated or
/// ambiguous while its create is answered, booting, or replanning between attempts. Times are
/// the Hub's clock (unix ms).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Attempt {
    pub number: u32,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    /// The attempt this one replaced: its number, why it failed, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<Replaced>,
}

/// The attempt a newer one replaced: its number, why it failed, and when it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Replaced {
    pub number: u32,
    pub failure: String,
    pub ended_at_ms: i64,
}

/// A rental as its Hub says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Rental {
    /// The product asked for, or the card when another client rented it.
    pub sku: String,
    pub accelerator: String,
    pub gpus: u32,
    pub usd_micros_per_hour: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_usd_micros: Option<i64>,
    /// `charged`, `unmetered` or `estimate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_basis: Option<String>,
    pub rented_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
    pub disk_gb: u32,
    /// The Hub's failure code, for a rental that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Why it runs off the requester's continent: nothing nearer fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_note: Option<String>,
    /// Since when the Hub cannot reach it, though its provider reports it running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable_since_ms: Option<i64>,
    /// The worker image it boots, by digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_worker_image_digest: Option<String>,
    /// `host:port` of its sshd, when it was rented with an SSH key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    /// Where it runs and whether its pod mounted the datacenter's NFS drive: a Hub admin's read, or
    /// a renter's who named the provider (dev-only `--provider`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacenter: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nfs_drive: bool,
}

/// A machine's `Status`, as it last sent it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MachineStatus {
    pub worker_id: String,
    pub version: String,
    /// The commit its `cozy` binary was built from.
    #[serde(default)]
    pub commit: String,
    /// Its binary arrived by a dev update, not a release.
    #[serde(default)]
    pub dev: bool,
    pub platform: String,
    /// `booting`, `ready` or `releasing`.
    pub phase: String,
    /// An update in progress there: what it waits for, else its state.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub update: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub tensorfs: String,
    /// The PyTorch its packages use; empty from an older machine (`v1::StatusFrame::torch`).
    #[serde(default)]
    pub torch: String,
    pub started_at_ms: i64,
    /// When a rental releases itself, or this computer's machine exits, if it stays idle; none
    /// while it works or is kept up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_deadline_ms: Option<i64>,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    #[serde(default)]
    pub disk_total_bytes: u64,
    #[serde(default)]
    pub disk_free_bytes: u64,
    /// What fills the disk, by kind (`machine::Disk`); 0 from an older machine.
    #[serde(default)]
    pub disk_models_bytes: u64,
    #[serde(default)]
    pub disk_environments_bytes: u64,
    #[serde(default)]
    pub disk_runtime_bytes: u64,
    #[serde(default)]
    pub disk_runs_bytes: u64,
    /// This controller's queued, preparing or running runs there, as the machine says.
    #[serde(default)]
    pub runs: Vec<LiveRun>,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(default)]
    pub models_bytes: u64,
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default)]
    pub warm: Vec<WarmItem>,
    /// `shared` or `dedicated` (`machine.mode`); empty from an older machine.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// The oldest NVIDIA driver its kernels run on; empty: no GPU stack.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver_floor: String,
    /// Why a GPU of that computer is not among `gpus`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_problems: Vec<String>,
    /// Its build toolchain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<crate::machine::Toolchain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kernels: Vec<crate::machine::KernelVerdict>,
    /// Killed processes stuck in the kernel there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stuck: Vec<crate::machine::StuckProcess>,
    /// What it does for this controller's runs besides their own work, each once however many wait on it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<Operation>,
}

/// Weights arriving or a package's environment built on a machine (`machine::Operation`), and the runs waiting on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Operation {
    /// The same for as long as it lasts.
    pub id: String,
    /// `download` or `install`.
    pub kind: String,
    /// The model's repository (or provider source), or the package.
    pub subject: String,
    /// Where it is, in words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stage: String,
    /// Bytes fetched of those it has to fetch (0: unknown), never what was here already.
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub bytes_total: u64,
    /// What of it was here already (shared with another model, or pulled before).
    #[serde(default)]
    pub bytes_here: u64,
    /// Its bytes a second over its last 10 s, once a second of them is seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_per_s: Option<f64>,
    /// The seconds its bytes still take at that rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_s: Option<i64>,
    pub started_at_ms: i64,
    /// The runs waiting on it, by number.
    #[serde(default)]
    pub runs: Vec<u64>,
}

/// A run a machine holds for this controller, beside this controller's record of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveRun {
    pub id: String,
    /// The machine's word for it: queued, preparing or running.
    pub state: String,
    /// Why it waits there.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub waiting: String,
    /// None when this controller keeps no record of it; a settled one was left behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
}

/// One of a machine's GPUs: its name, memory, driver and family.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Gpu {
    pub index: u32,
    pub name: String,
    pub memory_bytes: u64,
    #[serde(default)]
    pub driver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_class: Option<String>,
    /// The most of `memory_bytes` the machine uses; 0: all of it.
    #[serde(default, skip_serializing_if = "unlimited")]
    pub memory_limit_bytes: u64,
}

fn unlimited(bytes: &u64) -> bool {
    *bytes == 0
}

/// A model repository the machine holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Model {
    pub repository: String,
    pub total_bytes: u64,
    pub unique_bytes: u64,
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
}

/// One checkpoint of a held model: its release, lane, precision and header.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Checkpoint {
    #[serde(default)]
    pub release: String,
    pub lane: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub precision: String,
    pub manifest: String,
}

/// `ssh` into a rental, as root, trusting its host key the first time, in a known-hosts file of that rental's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Ssh {
    /// The command and its arguments, for what runs it.
    pub args: Vec<String>,
    /// The same as one line to paste.
    pub line: String,
}

/// `GET /v1/machines/{name}/packages`: what a machine holds installed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MachinePackages {
    pub machine: String,
    pub packages: Vec<HeldPackage>,
}

/// One package a machine holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HeldPackage {
    pub machine: String,
    pub package: String,
    pub release: String,
    /// The cozy-runtime in its environment.
    pub runtime: String,
    pub level: String,
    pub hub: String,
    /// `local`, `hub`, or another Hub's URL.
    pub source: String,
    /// What its runs are told of the Runtime pair it holds (one of the machine's it did not take,
    /// and why); empty for nothing.
    pub warning: String,
}

/// `GET /v1/machines/{name}/models`: the models a machine holds. Its store's total counts shared content once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MachineModels {
    pub machine: String,
    pub models: Vec<HeldModel>,
    /// It has reported no Status yet, so nothing is known of what it holds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub silent: bool,
    /// The repositories in its store, and their bytes.
    #[serde(default)]
    pub repositories: u64,
    #[serde(default)]
    pub bytes: u64,
    /// Everything its store takes on the disk, models or not (cut downloads, objects no model
    /// names any more); 0 from a machine that does not measure it.
    #[serde(default)]
    pub stored_bytes: u64,
}

/// One checkpoint a machine holds, and its repository's bytes there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HeldModel {
    pub machine: String,
    pub model: String,
    pub release: String,
    pub lane: String,
    /// Its header's precision lane (`int8`, …).
    pub precision: String,
    pub checkpoint: String,
    pub bytes: u64,
    /// The repository's bytes other held models share.
    pub shared_bytes: u64,
}

/// Where an installation came from, for a client of `hub`: `local` code, the Hub, or another Hub by its URL.
pub fn package_source(installed_from: &str, hub: &str) -> String {
    let other = |origin: &str| crate::config::Origin::parse(origin).is_ok_and(|origin| origin.to_string() != hub);
    match installed_from {
        "" => "local".into(),
        other_hub if other(other_hub) => other_hub.into(),
        _ => "hub".into(),
    }
}

impl Machine {
    /// The packages it holds, read by a client of `hub`.
    pub fn packages(&self, hub: &str) -> MachinePackages {
        let held = |env: &Environment| HeldPackage {
            machine: self.name.clone(),
            package: env.package.clone(),
            release: env.release.clone(),
            runtime: env.runtime.clone(),
            level: env.level.clone(),
            hub: env.hub.clone(),
            source: package_source(&env.hub, hub),
            warning: env.warning.clone(),
        };
        MachinePackages { machine: self.name.clone(), packages: self.status.iter().flat_map(|s| &s.environments).map(held).collect() }
    }

    /// The models it holds.
    pub fn models(&self) -> MachineModels {
        let mut held = MachineModels { machine: self.name.clone(), silent: self.status.is_none(), ..Default::default() };
        let Some(status) = &self.status else { return held };
        (held.repositories, held.bytes, held.stored_bytes) = (status.models.len() as u64, status.models_bytes, status.disk_models_bytes);
        for model in &status.models {
            let row = |release: &str, lane: &str, precision: &str, checkpoint: &str| HeldModel {
                machine: self.name.clone(),
                model: model.repository.clone(),
                release: release.into(),
                lane: lane.into(),
                precision: precision.into(),
                checkpoint: checkpoint.into(),
                bytes: model.total_bytes,
                shared_bytes: model.total_bytes.saturating_sub(model.unique_bytes),
            };
            if model.checkpoints.is_empty() {
                held.models.push(row("", "", "", ""));
            }
            held.models.extend(model.checkpoints.iter().map(|c| row(&c.release, &c.lane, &c.precision, &c.manifest)));
        }
        held
    }
}

/// A package environment the machine holds for this controller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Environment {
    pub installation: String,
    pub package: String,
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub hub: String,
    /// The cozy-runtime installed in it: the machine's, or a newer one its package needs.
    #[serde(default)]
    pub runtime: String,
    /// What it holds now: `installed`, `downloaded`, `imported`, `host` or `gpu`.
    #[serde(default)]
    pub level: String,
    /// What its runs are told of the Runtime pair it holds (one of the machine's it did not take,
    /// and why); empty for nothing.
    #[serde(default)]
    pub warning: String,
}

/// One member of this controller's warm set on the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WarmItem {
    /// `org/name[@release]`, or an installation id.
    pub source: String,
    #[serde(default)]
    pub entrypoint: String,
    /// The level asked for, and the one it holds now.
    pub level: String,
    #[serde(default)]
    pub holds: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub held_back: String,
}

/// `GET /v1/machines` (`?refresh=true` lists every Hub again first).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MachineList {
    pub machines: Vec<Machine>,
    /// When each Hub last answered a listing.
    #[serde(default)]
    pub hubs: Vec<HubListing>,
    /// Hubs whose last listing failed: their rows are `unverified`.
    #[serde(default)]
    pub unanswered: Vec<HubProblem>,
    /// Hubs this computer holds no login for.
    #[serde(default)]
    pub signed_out: Vec<String>,
    /// The live rentals listed, summed.
    pub rentals: LiveRentals,
}

impl MachineList {
    /// Where a render goes with no machine named, the plan placing each job on whichever ends it first (Paul, 11-05): every rental of this
    /// controller up or on its way, any other computer's that's up, then this one, the last choice, which starts when used. With none of them,
    /// `local`, which says why it can't where it can't.
    pub fn render_set(&self) -> Vec<On> {
        let held = |machine: &&Machine| machine.ordered && machine.offline.is_none();
        let rented = self.machines.iter().filter(held).filter(|machine| machine.kind == MachineKind::Rental && matches!(machine.state, MachineState::Ready | MachineState::Booting));
        let added = self.machines.iter().filter(held).filter(|machine| machine.kind == MachineKind::Added && machine.state == MachineState::Ready && machine.unreachable.is_none());
        let mut set: Vec<On> = rented.chain(added).map(|machine| On::Machine(machine.name.clone())).collect();
        if set.is_empty() || platform::Os::CURRENT.local_inference() {
            set.push(On::Machine("local".into()));
        }
        set
    }
}

/// Live rentals summed: how many, what they cost an hour now, and what they accrued so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveRentals {
    pub count: u32,
    pub usd_micros_per_hour: i64,
    /// Over the rentals whose Hub said what they cost; none when none did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accrued_usd_micros: Option<i64>,
    /// The weakest basis among them: `charged`, `unmetered`, or `estimate` (also when a rental's
    /// Hub said nothing, and the sum is then `accrued_floor`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accrued_basis: Option<String>,
    /// Some rental's Hub said nothing of its cost: the sum is at least this.
    #[serde(default)]
    pub accrued_floor: bool,
}

impl LiveRentals {
    /// Sums `rentals`.
    pub fn of<'a>(rentals: impl IntoIterator<Item = &'a Rental>) -> Self {
        let mut live = Self::default();
        let mut known = 0;
        for r in rentals {
            live.count += 1;
            live.usd_micros_per_hour += r.usd_micros_per_hour;
            if let Some(micros) = r.spend_usd_micros {
                known += 1;
                *live.accrued_usd_micros.get_or_insert(0) += micros;
                let basis = r.spend_basis.clone().unwrap_or_else(|| "estimate".into());
                live.accrued_basis = Some(weakest(live.accrued_basis.as_deref().unwrap_or(""), &basis, "charged"));
            }
        }
        if live.accrued_usd_micros.is_some() && known < live.count {
            live.accrued_floor = true;
            live.accrued_basis = Some("estimate".into());
        }
        live
    }
}

/// The weaker of two cost bases (`settled` is the known one: `charged`, or `provider_billed`): an
/// estimate is weaker than anything known, unmetered than charged.
pub(crate) fn weakest(held: &str, next: &str, settled: &str) -> String {
    let rank = |basis: &str| match basis {
        "" => 0,
        b if b == settled => 1,
        "unmetered" => 2,
        _ => 3,
    };
    if rank(next) > rank(held) { next } else { held }.to_owned()
}

/// When a Hub last answered a machine listing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubListing {
    pub hub: String,
    pub listed_at_ms: i64,
}

/// A Hub whose last machine listing failed, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubProblem {
    pub hub: String,
    pub code: String,
    pub message: String,
}

/// `POST /v1/machines` with `Rent`: 202, or 200 when the key had asked already (`replay`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Rented {
    pub machine: Machine,
    /// The Hub's quote the order was placed at.
    pub quote: Quote,
    pub replay: bool,
}

/// The Hub's price for the exact rental asked (`POST /v1/rental-quotes`): one price an hour, the
/// machine with its disk and traffic, and what an hour plus its download is expected to cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Quote {
    /// What a person is shown and charged.
    pub usd_micros_per_hour: i64,
    pub disk_gb: u32,
    pub expected_hours: f64,
    pub expected_download_gb: u32,
    pub expected_total_usd_micros: i64,
    /// The order's cap, which a renter sends back as it came: the Hub rents nothing dearer.
    pub agreed: Agreed,
    /// What the Hub changed of the ask to quote it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Where the quoted offer is, for an ask that named its provider or a Hub admin's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacenter: Option<String>,
}

/// The most a renter agreed to pay an hour: the order's `max_usd_micros_per_hour` at the Hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Agreed {
    pub usd_micros_per_hour: i64,
}

/// `DELETE /v1/machines/{name}[?force=true]`: `changed` is false when it had ended already. While
/// its runs' outputs have not landed it is refused `outputs_exporting`, unless `force`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Ended {
    pub machine: Machine,
    pub changed: bool,
    /// Outputs of its runs that had not landed when `force` ended it, given up: `run 12 big`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub abandoned: Vec<String>,
}

/// `GET /v1/runs/{id}/play?output=<name[/i]>&expires_s=N`: how a browser plays one run output
/// straight from its machine over WebRTC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Play {
    /// `host:port` the browser dials.
    pub address: String,
    /// SHA-256 hex of the certificate the machine's DTLS presents.
    pub fingerprint: String,
    /// A Cozy-Cap for that run output alone.
    pub token: String,
    /// The hosted player's link, with all of the above in its fragment.
    pub url: String,
    /// How long `token` and `url` work, from now.
    pub expires_s: u64,
}

/// `POST /v1/machines/{name}/keepalive`, once the machine acknowledged it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Keepalive {
    pub acknowledged_at_ms: i64,
    /// None while a job is queued or running there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_deadline_ms: Option<i64>,
}

/// `POST /v1/machines`: rent one (with an `Idempotency-Key`), add another computer's, or record
/// this computer's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewMachine {
    Rent {
        sku: String,
        /// Empty: Calcifer's configured Hub.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hub: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gpus: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk_gb: Option<u32>,
        /// A Hub admin's worker image, `sha256:<hex>`, in place of the machine kind's current
        /// one; the Hub refuses anyone else.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        /// The one provider to buy from (`runpod`, `vast`): dev-only `--provider`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// The quote the renter agreed to (`POST /v1/rental-quotes`): a dearer one rents nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agreed: Option<Agreed>,
        /// An OpenSSH public key root logs in with (`cozy machine ssh`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ssh_key: Option<String>,
    },
    Add {
        name: String,
        address: String,
        /// `sha256:<hex>` of the machine's TLS leaf.
        fingerprint: Digest,
    },
}

/// `GET /v1/machines/{name}/logs/{log}`: the logs a machine keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MachineLog {
    /// TensorD's own log.
    Tensord,
    Tensorfs,
    Memory,
}

/// `GET /v1/machines/{name}/logs/{log}` with `Accept: text/event-stream`: each batch of whole lines
/// as the machine appends it; `after` (also the event's `id:`) resumes the follow after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LogEvent {
    #[serde(rename = "lines")]
    Lines { after: String, text: String },
}

/// `POST /v1/machines/{name}/update` (with an `Idempotency-Key`): the machine's Runtime, TensorFS
/// and TensorD (its `cozy` binary) in one update run and one restart. Each is a release, or a file
/// on this computer (a dev build: a wheel, or a `cozy` binary). With none named, each moves to its
/// Hub's target.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MachineUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Component>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tensorfs: Option<Component>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tensord: Option<Component>,
    /// Development only: the most of each GPU's memory the machine uses, in bytes; 0 lifts it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_memory_limit: Option<u64>,
    /// Shared/Dedicated category, applied after the ordinary drain and restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<crate::config::MachineMode>,
}

/// One component of a `MachineUpdate`: `{"version": "0.33.1"}`, or a dev build sent first
/// (`PUT /v1/files`): `{"file": {sha256, name, version?}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Version(String),
    File(DevBuild),
}

/// A dev build by content: a wheel named `<distribution>-<version>-….whl`, or a `cozy` binary and
/// the release it says it is (`cozy --version`), when it says one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DevBuild {
    pub sha256: Digest,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// The answer to `POST /v1/machines/{name}/update`: `202` with its update run, followed like any
/// run; `200` with none when the machine runs its Hub's target already. An update run that
/// completes without the machine running what it asked for fails `update_unapplied`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Updating {
    pub machine: Machine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
    /// What it asks for, in words: `runtime 0.33.1 (prerelease), cozy 0.16.2 (installed)`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `PUT` or `DELETE /v1/machines/local/authorized-keys/{key}`: whether the file changed, and
/// `local` as it then is: once a key is added, where other computers add it (`reach`), unless it
/// is busy and moves there once idle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Authorized {
    pub key: String,
    pub file: PathBuf,
    pub changed: bool,
    pub machine: Machine,
    /// It runs as a worker image's machine, whose renter names where it is reached.
    pub image: bool,
}

/// `GET /v1/rental-skus[?hub=]`: what is for rent now, cheapest first by its fewest GPUs, as the
/// Hub offers it. A product the Hub cannot sell now, for whatever reason, is not listed.
#[derive(Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RentalSkus {
    pub rental_skus: Vec<RentalSku>,
}

/// One product (a GPU card, or the CPU machine) with the widths buyable now.
#[derive(Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RentalSku {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub accelerator_model: String,
    #[serde(default)]
    pub base_worker_profile: String,
    /// major.minor; empty for the CPU product.
    #[serde(default)]
    pub compute_capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_class: Option<String>,
    #[serde(default)]
    pub vram_gb: i64,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub python_provisionable_minors: Vec<String>,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub python_interpreters: Vec<RentalPythonInterpreter>,
    /// The GPU counts buyable now, ascending.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub widths: Vec<RentalSkuWidth>,
}

/// A Python interpreter a product offers, as the Hub lists it: its version and ABI.
#[derive(Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct RentalPythonInterpreter {
    pub version: String,
    pub abi: String,
}

/// One GPU count of a product and its one price an hour, the machine with its disk and traffic.
#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RentalSkuWidth {
    #[serde(default)]
    pub accelerator_count: i64,
    #[serde(default)]
    pub price_usd_micros_per_hour: i64,
    #[serde(default)]
    pub container_disk_gb: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcpu_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<i64>,
    #[serde(default)]
    pub expected_price: RentalPriceEstimate,
}

/// What a rental is expected to cost at its prices, for the expected hours plus its download.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct RentalPriceEstimate {
    pub hours: f64,
    pub download_gb: i64,
    pub gpu_usd_micros: i64,
    pub disk_usd_micros: i64,
    pub download_usd_micros: i64,
    pub total_usd_micros: i64,
}

/// A JSON `null` read as the type's empty value.
pub(crate) fn null_is_empty<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `GET /v1/rentals[?since_ms=&hub=]`: every rental of this account at the Hub, ended ones too and
/// rented from any computer, newest first, with what each cost and the sums per UTC day and in
/// all. Not paged: `since_ms` is the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RentalHistory {
    /// Rentals started at or after this instant; none: every one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    pub rentals: Vec<RentalRow>,
    /// Newest day first.
    pub days: Vec<RentalSum>,
    pub total: RentalSum,
}

/// One rental in the history: its product, state, times and what it cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RentalRow {
    pub rental_id: String,
    pub name: String,
    pub sku: String,
    pub gpus: u32,
    /// The Hub's word: `ready`, `released`, `failed`, …
    pub state: String,
    pub started_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
    /// From its order to its end, or to now while it lives.
    pub ran_s: i64,
    /// What it cost its renter, on its basis: `charged`, `unmetered` (priced, not charged) or
    /// `estimate`.
    pub spend_usd_micros: i64,
    pub spend_basis: String,
    /// What its provider billed Tensorhub, on its basis (`provider_billed` or `estimate`): the
    /// Hub tells an operator only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cost_usd_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cost_basis: Option<String>,
}

/// Rentals' costs summed, each on its weakest basis.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RentalSum {
    /// A UTC day, `2026-10-30`; none for the whole total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    pub rentals: u32,
    pub spend_usd_micros: i64,
    pub spend_basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cost_usd_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cost_basis: Option<String>,
}

impl RentalSum {
    pub fn add(&mut self, row: &RentalRow) {
        self.rentals += 1;
        self.spend_usd_micros += row.spend_usd_micros;
        self.spend_basis = weakest(&self.spend_basis, &row.spend_basis, "charged");
        if let (Some(micros), Some(basis)) = (row.provider_cost_usd_micros, &row.provider_cost_basis) {
            *self.provider_cost_usd_micros.get_or_insert(0) += micros;
            let held = self.provider_cost_basis.take().unwrap_or_default();
            self.provider_cost_basis = Some(weakest(&held, basis, "provider_billed"));
        }
    }
}

/// `POST /v1/down`: Calcifer stops; machines keep running what they accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Down {
    pub stopping: bool,
    /// What Calcifer still owes, which the next Calcifer resumes.
    pub owed: Vec<String>,
}

/// `GET /v1/status`: this Calcifer at a glance. It never waits on a Hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CalciferStatus {
    pub version: String,
    /// `linux`, `macos` or `windows`.
    pub os: String,
    /// Whether this computer runs models itself ([`local_usable`]).
    pub local_usable: bool,
    /// Whether this computer holds a device key for its Hub. `GET /v1/account` asks the Hub whether it still counts.
    pub signed_in: bool,
    /// Bytes Calcifer's code has allocated and not freed: what it holds, which its resident memory also counts with what its
    /// allocator keeps of what was freed.
    #[serde(default)]
    pub heap_bytes: u64,
}

/// `POST /v1/session-codes`: a code a browser trades once for a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCode {
    pub code: String,
    /// Calcifer's page, the code in its fragment.
    pub url: String,
    pub expires_ms: i64,
}

/// `POST /v1/session`'s body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemCode {
    pub code: String,
}

/// `POST /v1/session`: `token` for `Authorization`, `media` for the `?media=` of a file's URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BrowserSession {
    pub token: String,
    pub media: String,
}

/// The file in Calcifer's directory (`~/.cozy/calcifer`) that says, as an `error::Envelope`, why
/// the last start failed; a successful start deletes it.
pub const REFUSED: &str = "refused.json";

/// What a serving Calcifer publishes beside its lock (`calcifer.lock.holder`, 0600, one JSON
/// line): where its API listens and the bearer it wants, fresh at every start. Absent while the
/// lock is held: Calcifer is still opening its store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub address: SocketAddr,
    pub token: String,
    pub pid: u32,
    pub version: String,
    pub started_at_ms: i64,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Call => "call",
            Kind::Warm => "warm",
            Kind::Update => "update",
            Kind::Remove => "remove",
            Kind::Here => "here",
        }
    }

    /// The kind [`Self::as_str`] spells as `text`; none for any other.
    pub fn parse(text: &str) -> Option<Self> {
        [Kind::Call, Kind::Warm, Kind::Update, Kind::Remove, Kind::Here]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// Refuses `machine` when it is this computer's (`local`) on an OS whose local inference is off
/// ([`platform::Os::local_inference`]): every verb that would use it, and Calcifer, refuse alike.
pub fn local_usable(machine: &str) -> Result<(), crate::error::Error> {
    let os = platform::Os::CURRENT;
    if !machine.eq_ignore_ascii_case("local") || os.local_inference() {
        return Ok(());
    }
    let message = format!("local runs aren't available on {os} yet");
    Err(crate::error::Error::new(crate::error::Kind::Unavailable, "local_unavailable", message)
        .remedy("add `--on new` to rent a Linux GPU, or `--on <machine>` for one you have")
        .next("cozy machine rent"))
}

impl On {
    /// This computer's machine.
    pub fn is_local(&self) -> bool {
        matches!(self, On::Machine(name) if name == "local")
    }
}

impl From<String> for On {
    fn from(text: String) -> Self {
        // Machine names and products are never case-sensitive.
        let text = text.trim().to_ascii_lowercase();
        match text.as_str() {
            "any" => On::Any,
            "new" => On::New(None),
            _ => match text.strip_prefix("new:") {
                Some(sku) => On::New(Some(sku.to_owned())),
                None => On::Machine(text),
            },
        }
    }
}

impl From<On> for String {
    fn from(on: On) -> Self {
        on.to_string()
    }
}

impl fmt::Display for On {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            On::Machine(name) => f.write_str(name),
            On::Any => f.write_str("any"),
            On::New(None) => f.write_str("new"),
            On::New(Some(sku)) => write!(f, "new:{sku}"),
        }
    }
}

/// `gpus`: 1 or more. Where a run uses the CPU is its function's and its machine's to say.
pub fn gpu_count(text: &str) -> Result<u32, String> {
    match text.parse::<u32>() {
        Ok(0) => Err(
            "a function that needs no GPU runs on the CPU by itself, as does every run on a \
             machine without one: `--on new:cpu` rents a machine with none"
                .into(),
        ),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("{text:?} is not a number of GPUs")),
    }
}

/// `attention_kernel`, in the grammar the Runtime reads (`KERNEL` or `[SLOT/]COMPONENT=KERNEL`, no
/// whitespace); the Runtime judges the names.
pub fn attention_pin(pin: &str) -> Result<String, String> {
    let formed = match pin.split_once('=') {
        None => !pin.is_empty(),
        Some((scope, kernel)) => !scope.is_empty() && !kernel.is_empty() && !kernel.contains('='),
    };
    match formed && !pin.contains(char::is_whitespace) {
        true => Ok(pin.to_owned()),
        false => Err("use KERNEL or [SLOT/]COMPONENT=KERNEL, with no whitespace".into()),
    }
}

/// A file fidelity's form; which file it names is checked as the run binds.
pub fn fidelity(term: &str) -> Result<String, String> {
    match term.rsplit_once('=') {
        Some((asset, "auto" | "low" | "medium" | "high")) if !asset.is_empty() => Ok(term.into()),
        _ => Err("use LABEL-OR-INDEX=auto|low|medium|high".into()),
    }
}
