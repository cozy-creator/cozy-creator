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
//! its outputs has landed or carries an `export_error`.
use std::{fmt, net::SocketAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::{Digest, RunId};

/// What a run does on its machine (`RunSpec.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Call,
    Warm,
    Update,
    Remove,
}

/// Where a run goes: `local`, a machine's name, `any`, or `new[:sku]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Where outputs land; absent: `~/.cozy/outputs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<PathBuf>,
    /// What the CLI noticed while binding the run (fields it dropped, …), kept with the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    /// The quote a `new` run's renter agreed to (`POST /v1/run-quotes`): Calcifer refuses a `new`
    /// run without one, and a dearer rental rents nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<Agreed>,
}

/// `POST /v1/run-quotes`: what a `new` run would rent, and its price; when the Hub placed it, why,
/// and two other machines it would rent instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunQuote {
    pub sku: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<u32>,
    pub quote: Quote,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Alternative>,
}

/// Another machine a run could rent, against the pick: its time as a multiple of the pick's, and
/// the GPU's price an hour. `label` says which way it trades.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alternative {
    pub sku: String,
    pub gpus: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub time: f64,
    pub usd_micros_per_hour: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// A local file a run reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedInput {
    pub path: PathBuf,
    pub digest: Digest,
    pub length: u64,
}

/// The answer to a submit: 202 when this call recorded the run, 200 (`replay`) when the key had.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Submitted {
    pub run: Run,
    pub replay: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct Run {
    pub id: RunId,
    pub number: u64,
    pub kind: Kind,
    pub target: String,
    pub hub: String,
    pub machine: Option<String>,
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
    /// With `?detail=full`: the run's settled calls, in the order they ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<Call>,
}

/// One request field a run's executor served other than as sent (`Outcome.adjustments`).
pub use crate::machine::Adjustment;

/// A memoizable run's or call's computation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memo {
    /// `sha256:<hex>`.
    pub computation: String,
    /// Answered from a result its machine held: nothing ran.
    pub memoized: bool,
    /// What keyed its code (`machine::Outcome::memo_basis`); empty from a machine that predates it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub basis: String,
}

/// One settled call of a run (the run's own function or a child call).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Incomplete,
}

/// One output of a run at its newest revision, rewritten in place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The file on this computer that holds this revision's bytes, once they landed there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Why this revision's bytes will not land (its machine ended, or holds them no more).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_error: Option<String>,
    /// A checkpoint already written to TensorFS; it has no output bytes to export here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<crate::machine::CheckpointRef>,
}

impl OutputItem {
    /// A checkpoint reference needs no local byte export.
    pub fn is_checkpoint(&self) -> bool {
        self.checkpoint.is_some()
    }
}

/// The machine's progress report, as it last sent it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// How far this stage is, when it says (`tel.progress`).
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
    /// The stage's steady step time: the mean of its steps after the first; 0 before one.
    #[serde(default)]
    pub step_avg_ms: f64,
    /// When the machine last saw it move (Unix ms; 0: unknown): how long a run has not.
    #[serde(default)]
    pub at_ms: i64,
}

/// One event of a run's stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The item's file on this computer holds its current revision (`item.path`).
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
}

/// `GET /v1/runs/events`: every run's changes, each run whole as `GET /v1/runs/{id}` answers it. No
/// backlog: a client subscribes, then lists. A stream that falls behind ends; its client lists again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RunsEvent {
    #[serde(rename = "run.updated")]
    Updated { run: Run },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    /// This computer's machine.
    Local,
    /// A machine rented through a Hub.
    Rental,
    /// Another computer's machine, added by address.
    Added,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// False: a live rental of this account that this controller did not rent.
    #[serde(default = "yes")]
    pub recorded: bool,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replaced {
    pub number: u32,
    pub failure: String,
    pub ended_at_ms: i64,
}

/// A rental as its Hub says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// A machine's `Status`, as it last sent it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Its build toolchain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<crate::machine::Toolchain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kernels: Vec<crate::machine::KernelVerdict>,
    /// Killed processes stuck in the kernel there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stuck: Vec<crate::machine::StuckProcess>,
}

/// A run a machine holds for this controller, beside this controller's record of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct Model {
    pub repository: String,
    pub total_bytes: u64,
    pub unique_bytes: u64,
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    #[serde(default)]
    pub release: String,
    pub lane: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub precision: String,
    pub manifest: String,
}

/// A package environment the machine holds for this controller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub installation: String,
    pub package: String,
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub hub: String,
    /// What it holds now: `installed`, `downloaded`, `imported`, `host` or `gpu`.
    #[serde(default)]
    pub level: String,
    /// Why it does not run the machine's own Runtime; empty when it does.
    #[serde(default)]
    pub warning: String,
}

/// One member of this controller's warm set on the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Live rentals summed: how many, what they cost an hour now, and what they accrued so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
pub fn weakest(held: &str, next: &str, settled: &str) -> String {
    let rank = |basis: &str| match basis {
        "" => 0,
        b if b == settled => 1,
        "unmetered" => 2,
        _ => 3,
    };
    if rank(next) > rank(held) { next } else { held }.to_owned()
}

/// `GET /v1/machines/events`: a machine as `GET /v1/machines` lists it, each time its row changes,
/// and the name of one that left the list. No backlog: a client subscribes, then lists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MachinesEvent {
    #[serde(rename = "machine.updated")]
    Updated { machine: Box<Machine> },
    #[serde(rename = "machine.removed")]
    Removed { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubListing {
    pub hub: String,
    pub listed_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubProblem {
    pub hub: String,
    pub code: String,
    pub message: String,
}

/// `POST /v1/machines` with `Rent`: 202, or 200 when the key had asked already (`replay`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rented {
    pub machine: Machine,
    /// The price the rental was asked at: an acceptance above it is released.
    pub quote: Quote,
    pub replay: bool,
}

/// The Hub's price for the exact rental asked (`POST /v1/rental-quotes`): the machine and its disk an
/// hour, traffic a GB, and what an hour plus its download is expected to cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub gpu_usd_micros_per_hour: i64,
    pub storage_usd_micros_per_hour: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_usd_micros_per_gb: Option<i64>,
    pub disk_gb: u32,
    pub expected_hours: f64,
    pub expected_download_gb: u32,
    pub expected_total_usd_micros: i64,
}

impl Quote {
    /// The order's terms the Hub holds it to: the machine and its disk an hour, and traffic. A
    /// user is shown only the GPU's price (`gpu_usd_micros_per_hour`): that is what we charge.
    pub fn agreed(&self) -> Agreed {
        Agreed {
            usd_micros_per_hour: self.gpu_usd_micros_per_hour + self.storage_usd_micros_per_hour,
            usd_micros_per_gb: self.bandwidth_usd_micros_per_gb.unwrap_or_default(),
        }
    }
}

/// The most a renter agreed to pay: the machine and its disk an hour, and traffic a GB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreed {
    pub usd_micros_per_hour: i64,
    #[serde(default)]
    pub usd_micros_per_gb: i64,
}

/// `DELETE /v1/machines/{name}[?force=true]`: `changed` is false when it had ended already. While
/// its runs' outputs have not landed it is refused `outputs_exporting`, unless `force`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
pub struct Play {
    /// `host:port` the browser dials.
    pub address: String,
    /// SHA-256 hex of the certificate the machine's DTLS presents.
    pub fingerprint: String,
    /// A Cozy-Cap for that run output alone.
    pub token: String,
}

/// `POST /v1/machines/{name}/keepalive`, once the machine acknowledged it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keepalive {
    pub acknowledged_at_ms: i64,
    /// None while a job is queued or running there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_deadline_ms: Option<i64>,
}

/// `POST /v1/machines`: rent one (with an `Idempotency-Key`), add another computer's, or record
/// this computer's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewMachine {
    Rent {
        sku: String,
        hub: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gpus: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk_gb: Option<u32>,
        /// A Hub admin's worker image, `sha256:<hex>`, in place of the machine kind's current
        /// one; the Hub refuses anyone else.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        /// A Hub admin's provider (`runpod`, `vast`), in place of every provider; the Hub refuses
        /// anyone else.
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
}

/// One component of a `MachineUpdate`: `{"version": "0.33.1"}` or `{"file": "/abs/path"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Version(String),
    File(PathBuf),
}

/// The answer to `POST /v1/machines/{name}/update`: `202` with its update run, followed like any
/// run; `200` with none when the machine runs its Hub's target already. An update run that
/// completes without the machine running what it asked for fails `update_unapplied`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Updating {
    pub machine: Machine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
    /// What it asks for, in words: `runtime 0.33.1 (prerelease), cozy 0.16.2 (installed)`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `GET /v1/machines/local/authorized-keys`: the controllers' public keys this computer's machine
/// admits, and the file that holds them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedKeys {
    pub file: PathBuf,
    pub keys: Vec<String>,
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
pub struct RentalSkus {
    pub rental_skus: Vec<RentalSku>,
}

/// One product (a GPU card, or the CPU machine) with the widths buyable now.
#[derive(Debug, Default, Deserialize, Serialize)]
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

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RentalPythonInterpreter {
    pub version: String,
    pub abi: String,
}

/// One GPU count of a product: what we charge for its GPUs an hour, and its disk and traffic.
#[derive(Debug, Deserialize, Serialize)]
pub struct RentalSkuWidth {
    #[serde(default)]
    pub accelerator_count: i64,
    #[serde(default)]
    pub price_usd_micros_per_hour: i64,
    #[serde(default)]
    pub storage_usd_micros_per_hour: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_usd_micros_per_gb: Option<i64>,
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
#[serde(default)]
pub struct RentalPriceEstimate {
    pub hours: f64,
    pub download_gb: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_mbps: Option<i64>,
    pub download_hours: f64,
    pub gpu_usd_micros: i64,
    pub disk_usd_micros: i64,
    pub download_usd_micros: i64,
    pub total_usd_micros: i64,
}

/// A JSON `null` read as the type's empty value.
pub fn null_is_empty<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(
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
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Kind::Call, Kind::Warm, Kind::Update, Kind::Remove]
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

impl From<String> for On {
    fn from(text: String) -> Self {
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
