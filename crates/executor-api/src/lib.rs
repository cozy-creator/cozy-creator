//! TensorD ↔ executor seam: the commands TensorD sends a Runtime executor, the replies, progress
//! and requests the executor sends back, and TensorD's answers. One definition for both sides.
//!
//! The Runtime keeps no copy: it reads and builds these through the tensorfs wheel's
//! `tensorfs.executor` (the `python` feature). Declared optional fields take their defaults;
//! required fields fail by name and an unknown command is refused.
//!
//! A crate of its own so the tensorfs wheel builds it alone: as `api::executor` the wheel took 20 more crates and
//! built 31% longer, for a library 0.7% larger (cl-829).
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The result document's spool name.
pub const RESULT_DOCUMENT: &str = "result.canonical";

// --------------------------------------------------------------------------- commands

/// One command to an executor, tagged by `cmd`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Hello,
    Start(Start),
    Load(Box<Load>),
    PrepareRequest(PrepareRequest),
    /// Discard a queued preparation; consumed out of band, without a reply.
    Cancel(Cancel),
    Invoke(Box<Invoke>),
    /// Warming: the loaded construction's models at `level`, nothing run.
    Map(Map),
    /// Fork this import-only executor into a new one on its own seam (`fork/1`).
    Fork(Fork),
    /// A client watches a call's previews (`author.preview`); consumed out of band, without a
    /// reply. Sent only to an executor whose hello says `previews`.
    Preview(Preview),
}

impl Command {
    /// Every `cmd` TensorD sends.
    pub const NAMES: [&'static str; 9] =
        ["hello", "start", "load", "prepare_request", "cancel", "invoke", "map", "fork", "preview"];

    /// One command from its wire bytes. An unknown member is ignored and a missing one takes its
    /// default; the error names a needed member's path (`spool`, `bits`) or an unknown `cmd`.
    pub fn decode(data: &[u8]) -> Result<Command, String> {
        fn one<T: serde::de::DeserializeOwned>(fields: Value) -> Result<T, String> {
            serde_path_to_error::deserialize(fields).map_err(|e| e.to_string())
        }
        let fields: Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        let cmd = fields.get("cmd").and_then(Value::as_str).unwrap_or_default().to_owned();
        Ok(match cmd.as_str() {
            "hello" => Command::Hello,
            "start" => Command::Start(one(fields)?),
            "load" => Command::Load(Box::new(one(fields)?)),
            "prepare_request" => Command::PrepareRequest(one(fields)?),
            "cancel" => Command::Cancel(one(fields)?),
            "invoke" => Command::Invoke(Box::new(one(fields)?)),
            "map" => Command::Map(one(fields)?),
            "fork" => Command::Fork(one(fields)?),
            "preview" => Command::Preview(one(fields)?),
            other => return Err(format!("unknown command `{other}`")),
        })
    }

    /// Its `cmd`: what its reply names.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::Start(_) => "start",
            Self::Load(_) => "load",
            Self::PrepareRequest(_) => "prepare_request",
            Self::Cancel(_) => "cancel",
            Self::Invoke(_) => "invoke",
            Self::Map(_) => "map",
            Self::Fork(_) => "fork",
            Self::Preview(_) => "preview",
        }
    }
}

/// Bring the executor up before any Load: its GPUs, its package and what its attempts may call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Start {
    /// The session's GPUs; empty runs on the CPU and never opens CUDA.
    #[serde(default)]
    pub devices: String,
    #[serde(default)]
    pub application: String,
    #[serde(default)]
    pub package_interface: PathBuf,
    #[serde(default = "one")]
    pub sequence_parallel_degree: u32,
    /// Import torch, the Runtime and the package and stop: no device is touched.
    #[serde(default, skip_serializing_if = "is_false")]
    pub import_only: bool,
    /// What its attempts may call: the Runtime installs their callers before it imports the
    /// package and brokers each attempt's calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_interfaces: Vec<CallInterface>,
    /// Each constructed slot's checkpoint, so the Runtime schedules fetches before it imports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constructions: Vec<Construction>,
    /// Its stage memo tiers, when the machine memoizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<MemoSettings>,
    /// Development only (the machine's `gpu_memory_limit`, never a user setting): each GPU is a
    /// card of this many bytes, the executor's memory budget, to test constrained VRAM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_memory_limit: Option<u64>,
    /// The machine's mode: idle memory yields to foreign programs too. Peers donate either way.
    #[serde(default)]
    pub shared: bool,
    /// Each GPU's limit while it drives this computer's display, in the session's device order:
    /// its total less what the display keeps free at the driver (0: it drives none; empty: none
    /// does). The development `gpu_memory_limit` holds where it is lower.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_limit: Vec<u64>,
    /// A GPU of the session drives this computer's display (a shared machine): its calls pause
    /// for the desktop to draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_pace: Option<DisplayPace>,
}

/// How a call shares a GPU with the display it drives (`machine.display_pace`): it pauses
/// `pause_ms` (a 60 Hz frame) after each `work_ms` of work, keeping about work / (work + pause)
/// of its speed. A `pause_ms` of 0 never pauses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct DisplayPace {
    pub work_ms: u32,
    pub pause_ms: u32,
}
impl Default for DisplayPace {
    fn default() -> Self {
        Self { work_ms: 80, pause_ms: 16 }
    }
}

/// Construct a construction's models and register their weights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Load {
    pub construction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tensorfs: Option<TensorFSOptions>,
    /// The request's signer: the Runtime keeps its bearer per signer and endpoint.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    #[serde(default)]
    pub devices: String,
    #[serde(default = "one")]
    pub sequence_parallel_degree: u32,
    /// Its model slots, one or several (H3 turbo: base + refiner). A call that constructs nothing
    /// sends one weightless binding.
    pub models: Vec<Binding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_pin: String,
}

/// Resolve one request before entry and keep it for its Invoke; no device is touched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct PrepareRequest {
    pub request_id: String,
    pub construction: String,
    pub entrypoint: String,
    pub payload: Json,
    /// Checked before entry, so a bad pin is the request's refusal, not a fault.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_kernel: String,
    /// The request's weight precision (16, 8 or 4; 0: the rules choose).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bits: u32,
    /// File inputs by field path (identity only; the bytes arrive with Invoke).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_metadata: BTreeMap<String, InputMetadata>,
}

/// A client watches call `request_id`'s previews for the next `for_ms`: its executor makes them
/// until then (cr-282).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Preview {
    pub request_id: String,
    pub for_ms: u64,
}

/// The queued preparation to discard, by its request id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Cancel {
    pub request_id: String,
}

/// Run one prepared request in its own spool, with its inputs, checkpoints and output allowances.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Invoke {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tensorfs: Option<TensorFSOptions>,
    /// Retained asset capacity derived from the installed result schema.
    #[serde(default = "max_output_bytes")]
    pub max_output_bytes: u64,
    pub request_id: String,
    pub construction: String,
    pub entrypoint: String,
    pub spool: PathBuf,
    #[serde(default)]
    pub deadline_s: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_kernel: String,
    /// Dev-only: what the Runtime's attention capture records (a JSON spec); empty: nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_capture: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bits: u32,
    /// File inputs by field path: read-only copies in this call's spool.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputFile>,
    /// Input trees by reference: (read-only directory, manifest digest).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub trees: BTreeMap<String, (PathBuf, String)>,
    /// Each `Weights[M]` parameter's checkpoint (`sha256:<hex>`, its header), which the Runtime
    /// opens from the store itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, String>,
    /// Each on-demand model parameter that bound no checkpoint, and what reading it fails with:
    /// a code, and one line naming the slot, its model and why (xs-186).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unbound: BTreeMap<String, (String, String)>,
    /// The weights outputs this attempt may write, and each one's granted new bytes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub weights: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub destination: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation: String,
    /// The family whose GPUs its calls share: while it waits on a child call it yields under
    /// this name, so the child is admitted beside it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub family: String,
    /// Start's `display_limit` as the display uses it now, read again for each request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_limit: Vec<u64>,
}

/// Warm a loaded construction at `level`; with a `payload`, also prepare its entrypoint's program.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Map {
    pub construction: String,
    #[serde(default)]
    pub level: Level,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Json>,
}

/// What a warmed construction's models hold: `gpu` is warm and fills free room now, `ram`
/// leaves its resident pages idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Gpu,
    Ram,
}

/// Where the forked executor runs: its seam's socket, its root directory and its environment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Fork {
    pub socket: PathBuf,
    pub root: PathBuf,
    /// The child's whole environment, as the machine seals a spawned executor's.
    pub environment: BTreeMap<String, String>,
}

/// A checkpoint whose header lives only in TensorD's memory (an adapter view composed by
/// reference): the executor opens it from these bytes (`tensorfs.view`), never from the store.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct View {
    /// The cozytensors header, base64.
    pub header: String,
    /// The objects it cites that no store holds (its graph), base64.
    pub own: Vec<String>,
}

/// One model's construction: its class, where its weights are, and how to fill them. A
/// weightless binding names only its release and an empty `model_class`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct Binding {
    pub application: String,
    pub package_interface: String,
    pub model_class: String,
    /// `<fn>.models.<param>`
    pub model_binding_path: String,
    pub model_parameter_name: String,
    /// The components it loads, in its declared order.
    pub components: Vec<String>,
    pub store: String,
    /// Its checkpoint (`sha256:<hex>`, its header), which holds every component.
    pub snapshot: String,
    pub release: String,
    pub package: String,
    pub model: String,
    /// `snapshot` as a view, when it is one: set only on the Load sent, never planned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<View>,
    /// A group's placeable components and the GPU that keeps each, an index within the group
    /// (0: GPU 0, the serial scope's; g: GPU g keeps it resident for the group), decided by
    /// TensorD at Load.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub homes: BTreeMap<String, u32>,
}

/// An executor's stage memo tiers (`memo: stage/1`): its in-memory tier (-1: its own default;
/// 0: off) and the largest entry the machine keeps (0: that tier off).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct MemoSettings {
    pub process_bytes: i64,
    pub entry_bytes: u64,
}
impl Default for MemoSettings {
    fn default() -> Self {
        Self { process_bytes: -1, entry_bytes: 0 }
    }
}

/// One constructed model slot of a session: its checkpoint and the components it uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Construction {
    /// `<fn>.models.<param>`
    pub path: String,
    /// `sha256:<hex>`
    pub manifest: String,
    /// The slot's `components=`; empty is every component.
    #[serde(default)]
    pub components: Vec<String>,
}

/// One callable an attempt may call: its own package's, or another App's of its environment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct CallInterface {
    pub module: String,
    pub export: String,
    #[serde(default, rename = "self")]
    pub self_call: bool,
    /// A Runtime built-in surface (`operations`): the Runtime binds it from its own code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
}

/// One file input's identity, before its bytes are granted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct InputMetadata {
    pub input_id: String,
    pub media_type: String,
    pub digest: String,
    pub length: u64,
    #[serde(default)]
    pub order: u64,
}

/// One verified input file in the attempt's own spool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct InputFile {
    pub local: PathBuf,
    pub media_type: String,
    pub digest: String,
    pub length: u64,
    #[serde(default)]
    pub order: u64,
}

// --------------------------------------------------------------------------- the executor's frames

/// A frame the executor sends unasked: `progress` or `request`; a reply has none.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Progress,
    Request,
    #[serde(other)]
    Unknown,
}

/// What a `progress` frame carries: a progress update, a retained `log` observation or a preview.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Progress,
    /// A `progress` event carrying a retained `log` observation (`Telemetry.log`).
    Log,
    /// A `progress` event carrying a preview (`author.preview`): its `preview`.
    Preview,
    #[default]
    #[serde(other)]
    Unknown,
}

/// An executor's `hello` reply: who it is, its versions and its seal.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub transport_challenge: String,
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub runtime_version: String,
    pub tensorfs_version: String,
    pub sealed: BTreeMap<String, String>,
    /// It takes `preview` (cr-282): 1; 0 for an executor that makes none.
    pub previews: u32,
}

/// One preview of a call's latents (`author.preview`): the step it shows, and its picture.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PreviewFrame {
    /// The step it shows, of `total` (0 of 0: before the first).
    pub position: u64,
    pub total: u64,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    /// 1 for a picture; a video's, its frames.
    pub frames: u32,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

/// The result document the invoke wrote to its spool (`RESULT_DOCUMENT`): its digest and length.
#[derive(Clone, Debug, Deserialize)]
pub struct ResultRef {
    pub digest: String,
    pub length: u64,
}

/// An invocation's typed outcome: its terminal, origin, code and message.
#[derive(Clone, Debug, Deserialize)]
pub struct Outcome {
    pub terminal: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    /// The raising traceback of a failed invocation (diagnostics, never a terminal field).
    #[serde(default)]
    pub traceback: String,
}

/// One file of the invoke's result, by field path (`output_id`): its spool handle, kind and size.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Output {
    pub output_id: String,
    pub asset_ref: String,
    pub kind: String,
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// A saved output TensorD encodes after the reply: its raw spool snapshot, codec and facts.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostFrame {
    pub handle: String,
    pub codec: String,
    pub raw: String,
    pub raw_bytes: u64,
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub facts: BTreeMap<String, Value>,
}

/// An invoke reply's `metrics`: its timings, memory peaks and end RSS.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Metrics {
    pub handler_ms: Option<f64>,
    pub device_lease_ms: Option<f64>,
    pub d2h_wait_ms: Option<f64>,
    pub peak_vram_bytes: Option<i64>,
    pub peak_reserved_bytes: Option<i64>,
    pub activation_peak_bytes: Option<i64>,
    pub activation_peaks: BTreeMap<String, i64>,
    pub allocated_at_start_bytes: Option<i64>,
    pub working_peak_vram_bytes: Option<i64>,
    pub rss_at_end_bytes: Option<i64>,
    pub started_unix: Option<f64>,
    pub gpu_count: Option<u32>,
    pub shape_cell: Option<String>,
}

/// A load reply's `facts`: its timings and the device and host memory after it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LoadFacts {
    pub cuda_init_ms: Option<f64>,
    pub prepare_ms: Option<f64>,
    pub stages: Vec<(String, f64)>,
    pub warm_ms: Option<f64>,
    pub filled_bytes: Option<u64>,
    pub device_free_bytes: Option<i64>,
    pub device_total_bytes: Option<i64>,
    pub allocator_bytes: Option<i64>,
    pub reserved_bytes: Option<i64>,
    pub rss_bytes: Option<i64>,
    /// The attention kernel each component's sites were given, and what was skipped and why.
    pub attention: Option<Value>,
}

/// The Runtime's residency report on an invoke reply: its notes and, per model, its graph
/// regions' counters (captures, replays, pool bytes, eager gaps). Relayed whole: fields this
/// side does not know are kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Residency {
    pub notes: serde_json::Map<String, Value>,
    pub graphs: BTreeMap<String, serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

/// One part of a composite product: a spool file and the media time it adds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct PublishPart {
    pub local: String,
    pub duration_us: u64,
}

/// One decode per frame of whatever the executor sends (a reply, progress or a request); only
/// consumed business fields are represented, and every one is optional.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Frame {
    pub event: Option<Event>,
    /// A request or progress frame whose fields did not decode: why. A request is refused alone
    /// under its own sequence number and a progress frame is dropped; the executor goes on.
    #[serde(skip)]
    pub malformed: String,
    /// A `request` frame's request, decoded from the same members (`Request::decode`).
    #[serde(skip)]
    pub request: Option<Request>,
    pub reply: String,
    pub ok: bool,
    pub code: String,
    pub detail: String,
    /// A refusal's own terminal (`refused`/`failed`) and origin (`request`/`author`/`runtime`).
    pub terminal: String,
    pub origin: String,
    /// A traced command refusal's raising traceback.
    pub traceback: String,
    #[serde(flatten)]
    pub hello: Hello,
    pub outcome: Option<Outcome>,
    pub quiescent: bool,
    pub poisoned: String,
    pub reused: bool,
    /// Map: this explicit program was prepared now. None is weight-only warming.
    pub prepared: Option<bool>,
    /// `start` of a group (degree > 1): the follower ranks' pids, rank 1 first.
    pub follower_pids: Vec<u32>,
    /// `start` on a CUDA device: why its Runtime serves it none of its native kernels (the pure wheel, or kernels that don't
    /// load); empty: it does (cl-913).
    pub kernels_absent: String,
    pub result_ref: Option<ResultRef>,
    pub outputs: Vec<Output>,
    pub frames: Vec<HostFrame>,
    pub max_output_bytes: Option<u64>,
    pub request_id: String,
    pub seq: u64,
    pub kind: Kind,
    pub descriptor: bool,
    pub name: String,
    /// `load` and `invoke` telemetry: one of a shape this side does not read is absent, never the
    /// reply's failure.
    #[serde(deserialize_with = "lenient")]
    pub facts: Option<LoadFacts>,
    #[serde(deserialize_with = "lenient")]
    pub metrics: Option<Metrics>,
    /// `invoke`: varena's record of the request (`Stats`), kept whole; another frame's `memory`
    /// of another shape reads as absent.
    #[serde(deserialize_with = "lenient")]
    pub memory: Option<serde_json::Map<String, Value>>,
    /// `invoke`: the Runtime's residency report, relayed into the run's measurements; one of
    /// another shape reads as absent.
    #[serde(deserialize_with = "lenient")]
    pub residency: Option<Residency>,
    pub stage: String,
    pub position: Option<u64>,
    pub total: Option<u64>,
    /// What `position` counts: empty for steps, `tokens` for a decode's, whose `total` is only its cap.
    pub unit: String,
    pub stage_fraction: Option<f64>,
    pub step_ms: Option<f64>,
    pub advance: u64,
    /// `progress`: the child call whose stage a job relays.
    pub call_request: Option<String>,
    /// A `preview` progress frame's picture.
    #[serde(deserialize_with = "lenient")]
    pub preview: Option<PreviewFrame>,
    /// A retained observation (`kind` log): `name` is its message, `value` its level and `fields`
    /// its scalars.
    pub value: Value,
    pub fields: Value,
    /// PrepareRequest: the request's normalized features (its shape).
    pub features: BTreeMap<String, Value>,
    /// `start`: the executor's legs, `[[name, ms], ...]`; read leniently.
    pub stages: Value,
    /// `invoke`: the executor's stage and step tracks, its ranks' execution records and its
    /// observation tail; read leniently, kept as the run's measurements.
    pub attribution: Value,
    pub execution: Value,
    pub observations: Value,
    /// `invoke`: some attention site of the call applied the request's pin.
    pub attention_applied: bool,
    /// `invoke`: the rows of how the request was served other than as sent; read row by row.
    pub adjustments: Value,
}

impl Frame {
    /// A `log` observation (`Telemetry.log`) as its log line: the level (debug, info, warning or
    /// error) and the message followed by ` key=value` for each field.
    pub fn log(&self) -> Option<(&'static str, String)> {
        if self.kind != Kind::Log {
            return None;
        }
        let level = match self.value.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("warning" | "warn") => "warning",
            Some("error" | "critical" | "fatal") => "error",
            Some("debug") => "debug",
            _ => "info",
        };
        let mut text = self.name.clone();
        for (key, value) in self.fields.as_object().into_iter().flatten() {
            match value {
                Value::String(s) if !s.is_empty() && !s.contains(char::is_whitespace) => {
                    text.push_str(&format!(" {key}={s}"))
                }
                other => text.push_str(&format!(" {key}={other}")),
            }
        }
        Some((level, text))
    }
}

// --------------------------------------------------------------------------- the executor's requests

/// One request an executor makes of TensorD during a call, tagged by `kind`, sent as
/// `{"event": "request", "seq": N, **request}` and answered under its `seq`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// The socket TensorD nudges when one of this attempt's calls moves (an fd follows the answer).
    ChildEvents(ChildEvents),
    ChildCall(ChildCall),
    ChildPoll(ChildPoll),
    ChildCancel(ChildCancel),
    ChildForget(ChildForget),
    /// A deviceless root's GPUs end (`Context.release_gpus()`).
    GpuRelease(GpuRelease),
    /// A hint about a future call: TensorD may prepare its weights.
    ModelPrefetch(ModelPrefetch),
    /// Show one asset to the run's owner now, as a product of a declared output.
    Publish(Publish),
    /// A weights output's TensorFS reference, which the Runtime committed or uploaded.
    CheckpointItem(CheckpointItem),
    /// A memoized method's key, before its scope opens.
    StageMemoLookup(StageMemoLookup),
    /// The outcome of a looked-up miss.
    StageMemoStore(StageMemoStore),
}

/// Ask for the child-call nudge socket; it has no fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ChildEvents {}

/// A file the caller saved in this attempt and passes to its child: its spool handle, and the
/// digest `payload` names it by, which its bytes must have.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct SavedFile {
    pub asset_ref: String,
    pub digest: String,
    pub media_type: String,
}

/// Start a child call: its callee, its request and the files passed to it, once per `call_index`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ChildCall {
    pub call_index: u64,
    pub module: String,
    pub export: String,
    /// The callee's canonical request document.
    pub payload: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub progress_label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved: Vec<SavedFile>,
}

/// Ask a child call's state (its answer's `CallState`), by index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ChildPoll {
    pub call_index: u64,
}

/// Cancel a child call, by index; its answer is `pending`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ChildCancel {
    pub call_index: u64,
}

/// Drop a child call's record, by index, once it has ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ChildForget {
    pub call_index: u64,
}

/// A deviceless root giving up its GPUs; it has no fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct GpuRelease {}

/// A future call's callee and its model choices, for TensorD to prepare.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ModelPrefetch {
    pub module: String,
    pub export: String,
    /// The canonical `{slot: ModelChoice | None}` document.
    #[serde(default)]
    pub payload: String,
}

/// One product to show now: the result file field it belongs to, its spool file or `parts`, its kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct Publish {
    pub output: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub asset_ref: String,
    pub asset_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub digest: String,
    /// A composite product: its bytes are these parts concatenated in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PublishPart>,
}

/// A weights output's commit: its repository, revision and the tag moved to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct CheckpointItem {
    pub output: String,
    /// Historical executor frame spelling is retained during the supported protocol cohort.
    #[serde(rename = "repo_id", alias = "model_id")]
    pub model_id: String,
    /// The commit (its tree id).
    pub revision: String,
    /// The private tag the Runtime moved there.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tag: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variant: String,
}

/// A memoized method's key and the scope (`stage`, `numerics`) it is looked up in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct StageMemoLookup {
    pub key: String,
    pub stage: String,
    pub numerics: String,
}

/// `local` names the entry's spool file; empty means nothing is stored and `reason` says why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct StageMemoStore {
    pub key: String,
    pub stage: String,
    pub numerics: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub length: u64,
    #[serde(default, skip_serializing_if = "is_zero_ms")]
    pub cost_ms: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl Request {
    /// One request from a request frame's members (`event` and `seq` beside them are ignored);
    /// the error names a needed member's path or an unknown `kind`.
    pub fn decode(fields: Value) -> Result<Request, String> {
        fn one<T: serde::de::DeserializeOwned>(fields: Value) -> Result<T, String> {
            serde_path_to_error::deserialize(fields).map_err(|e| e.to_string())
        }
        let kind = fields.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned();
        Ok(match kind.as_str() {
            "child_events" => Request::ChildEvents(one(fields)?),
            "child_call" => Request::ChildCall(one(fields)?),
            "child_poll" => Request::ChildPoll(one(fields)?),
            "child_cancel" => Request::ChildCancel(one(fields)?),
            "child_forget" => Request::ChildForget(one(fields)?),
            "gpu_release" => Request::GpuRelease(one(fields)?),
            "model_prefetch" => Request::ModelPrefetch(one(fields)?),
            "publish" => Request::Publish(one(fields)?),
            "checkpoint_item" => Request::CheckpointItem(one(fields)?),
            "stage_memo_lookup" => Request::StageMemoLookup(one(fields)?),
            "stage_memo_store" => Request::StageMemoStore(one(fields)?),
            other => return Err(format!("unknown request `{other}`")),
        })
    }
}

// --------------------------------------------------------------------------- TensorD's answers

/// TensorD's answer to one executor request, under its `seq`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct Answer {
    pub event: String,
    pub seq: u64,
    pub ok: bool,
    pub code: String,
    pub detail: String,
    /// One fd follows the answer. Sent only when true.
    #[serde(skip_serializing_if = "is_false")]
    pub descriptor: bool,
    pub sha256: String,
    pub length: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub sequence: u64,
    /// `CallState` (a managed call): its state, child, canonical result and byte grants.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub child_request_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub result: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub byte_grants: Vec<ByteGrant>,
    /// Advisory: a progress of a shape this side does not read is absent, never the answer's
    /// failure.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub progress: Option<CallProgress>,
    /// A failed child call's raising traceback, whole (its triage's).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub traceback: String,
    /// A memo hit: the entry's verified copy in the run's spool, and what producing it cost.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub local: String,
    #[serde(skip_serializing_if = "is_zero_ms")]
    pub cost_ms: f64,
    /// The memo scope computed one key twice, differently: the executor stops reusing it.
    #[serde(skip_serializing_if = "is_false")]
    pub disputed: bool,
    /// A memo store: whether the entry was kept, else why not.
    #[serde(skip_serializing_if = "is_false")]
    pub stored: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl Default for Answer {
    fn default() -> Self {
        Self {
            event: "answer".into(),
            seq: 0,
            ok: false,
            code: String::new(),
            detail: String::new(),
            descriptor: false,
            sha256: String::new(),
            length: 0,
            digest: String::new(),
            sequence: 0,
            state: String::new(),
            child_request_id: String::new(),
            result: String::new(),
            byte_grants: Vec::new(),
            progress: None,
            traceback: String::new(),
            local: String::new(),
            cost_ms: 0.0,
            disputed: false,
            stored: false,
            reason: String::new(),
        }
    }
}

impl Answer {
    /// A refusal of a request this side does not serve (`capability_unavailable`).
    pub fn unavailable(seq: u64) -> Self {
        Self {
            seq,
            code: "capability_unavailable".into(),
            detail: "owner has not qualified this operation".into(),
            ..Self::default()
        }
    }
    /// A bare success under `seq`.
    pub fn ok(seq: u64) -> Self {
        Self { ok: true, code: String::new(), detail: String::new(), ..Self::unavailable(seq) }
    }
    /// A refusal under `seq` with its `code` and `detail`.
    pub fn refused(seq: u64, code: &str, detail: impl Into<String>) -> Self {
        Self { code: code.into(), detail: detail.into(), ..Self::unavailable(seq) }
    }
}

/// A child's result byte, granted into the caller's spool: what the caller checks against the
/// result's own record before it hands out a handle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct ByteGrant {
    pub output_id: String,
    pub kind: String,
    pub digest: String,
    pub local: PathBuf,
    pub length: u64,
    pub content_bytes: u64,
    #[serde(default)]
    pub media_type: String,
}

/// A managed call's newest progress: its sequence and the child's progress frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
pub struct CallProgress {
    pub sequence: u64,
    pub payload: Json,
}

// --------------------------------------------------------------------------- helpers

/// A field of the shape this side reads, else absent: never a reason to refuse the frame.
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<Option<T>, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).ok())
}
fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero_ms(value: &f64) -> bool {
    *value == 0.0
}
fn one() -> u32 {
    1
}
/// The Runtime's own default retained-output capacity (`MAX_OUTPUT_BYTES`).
fn max_output_bytes() -> u64 {
    256 << 20
}

/// A JSON document carried as it is (a request's payload, an interface): Python reads it as plain
/// dicts, lists and scalars.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Json(pub Value);

#[cfg(feature = "python")]
mod python {
    use pyo3::{
        exceptions::PyValueError,
        prelude::*,
        types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple},
        IntoPyObjectExt,
    };
    use serde_json::Value;

    use super::*;

    /// A JSON value as plain Python dicts, lists and scalars.
    pub(crate) fn value<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
        Ok(match value {
            Value::Null => py.None().into_bound(py),
            Value::Bool(b) => b.into_bound_py_any(py)?,
            Value::Number(n) => match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => i.into_bound_py_any(py)?,
                (_, Some(u)) => u.into_bound_py_any(py)?,
                _ => n.as_f64().unwrap_or(f64::NAN).into_bound_py_any(py)?,
            },
            Value::String(s) => s.into_bound_py_any(py)?,
            Value::Array(items) => {
                let list = PyList::empty(py);
                for item in items {
                    list.append(self::value(py, item)?)?;
                }
                list.into_any()
            }
            Value::Object(fields) => {
                let dict = PyDict::new(py);
                for (key, item) in fields {
                    dict.set_item(key, self::value(py, item)?)?;
                }
                dict.into_any()
            }
        })
    }

    /// A Python value as JSON: a class of this module as its own wire JSON, a path as its text,
    /// else dicts, lists and scalars.
    fn json_of(value: &Bound<'_, PyAny>) -> PyResult<Value> {
        let invalid = |what: &str| PyValueError::new_err(format!("{what} has no JSON form"));
        Ok(if value.is_none() {
            Value::Null
        } else if let Ok(b) = value.cast::<PyBool>() {
            Value::Bool(b.is_true())
        } else if value.is_instance_of::<PyInt>() {
            match value.extract::<i64>() {
                Ok(i) => i.into(),
                Err(_) => value.extract::<u64>()?.into(),
            }
        } else if value.is_instance_of::<PyFloat>() {
            serde_json::Number::from_f64(value.extract::<f64>()?).map(Value::Number).ok_or_else(|| invalid("a non-finite float"))?
        } else if let Ok(s) = value.cast::<PyString>() {
            Value::String(s.to_cow()?.into_owned())
        } else if let Ok(d) = value.cast::<PyDict>() {
            let mut fields = serde_json::Map::new();
            for (key, item) in d.iter() {
                fields.insert(key.extract::<String>()?, json_of(&item)?);
            }
            Value::Object(fields)
        } else if value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>() {
            Value::Array(value.try_iter()?.map(|item| json_of(&item?)).collect::<PyResult<_>>()?)
        } else if value.hasattr("__fspath__")? {
            Value::String(value.call_method0("__fspath__")?.extract::<String>()?)
        } else if value.hasattr("json")? && value.get_type().module()?.to_cow()? == "tensorfs.executor" {
            serde_json::from_str(&value.call_method0("json")?.extract::<String>()?).map_err(|e| PyValueError::new_err(e.to_string()))?
        } else {
            return Err(invalid(&value.get_type().name()?.to_cow()?));
        })
    }

    /// `this` with `changes` in place of its fields, through serde.
    fn replaced<T: Serialize + serde::de::DeserializeOwned>(this: &T, changes: Option<&Bound<'_, PyDict>>) -> PyResult<T> {
        match serde_json::to_value(this).map_err(|e| PyValueError::new_err(e.to_string()))? {
            Value::Object(fields) => merged(fields, changes),
            _ => Err(PyValueError::new_err("not a record")),
        }
    }

    /// The record `fields` and `changes` make, through serde: a change of the wrong shape is
    /// refused by its path, one naming no field of the record by its name, and a needed field
    /// left out by its name.
    fn merged<T: Serialize + serde::de::DeserializeOwned>(
        mut fields: serde_json::Map<String, Value>,
        changes: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<T> {
        let error = |e: &dyn std::fmt::Display| PyValueError::new_err(e.to_string());
        // The canonical Python name crosses the unchanged supported executor protocol.
        // Remove this keyword bridge with the TensorFS 0.25 naming cut.
        if let Some(changes) = changes {
            if changes.contains("model_id")? && changes.contains("repo_id")? {
                return Err(PyValueError::new_err("use model_id, not both model_id and repo_id"));
            }
        }
        let mut given = vec![];
        for (key, value) in changes.into_iter().flat_map(|d| d.iter()) {
            let (key, value) = (key.extract::<String>()?, json_of(&value)?);
            let key = if key == "model_id" { "repo_id".to_owned() } else { key };
            fields.insert(key.clone(), value.clone());
            given.push((key, value));
        }
        let result: T = serde_path_to_error::deserialize(Value::Object(fields)).map_err(|e| error(&e))?;
        let after = serde_json::to_value(&result).map_err(|e| error(&e))?;
        // A field serde leaves out when empty reads back absent; any other change must read back.
        let empty = |v: &Value| match v {
            Value::Null => true,
            Value::Bool(b) => !b,
            Value::Number(n) => n.as_f64() == Some(0.0),
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
        };
        if let Some((key, _)) = given.iter().find(|(key, value)| after.get(key).is_none() && !empty(value)) {
            return Err(PyValueError::new_err(format!("no field `{key}`")));
        }
        Ok(result)
    }

    impl<'py> IntoPyObject<'py> for &Json {
        type Target = PyAny;
        type Output = Bound<'py, PyAny>;
        type Error = PyErr;
        fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
            value(py, &self.0)
        }
    }

    impl<'py> IntoPyObject<'py> for Json {
        type Target = PyAny;
        type Output = Bound<'py, PyAny>;
        type Error = PyErr;
        fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
            value(py, &self.0)
        }
    }

    /// A level reads as its wire name, `gpu` or `ram`.
    impl<'py> IntoPyObject<'py> for &Level {
        type Target = PyAny;
        type Output = Bound<'py, PyAny>;
        type Error = PyErr;
        fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
            match self {
                Level::Gpu => "gpu",
                Level::Ram => "ram",
            }
            .into_bound_py_any(py)
        }
    }

    macro_rules! methods {
        ($($t:ty),*) => {$(
            #[pymethods]
            impl $t {
                fn __repr__(&self) -> String {
                    format!("{self:?}")
                }
                /// Its wire JSON.
                fn json(&self) -> PyResult<String> {
                    serde_json::to_string(self).map_err(|e| PyValueError::new_err(e.to_string()))
                }
                /// A copy with `changes` in place of its fields (`msgspec.structs.replace`).
                #[pyo3(signature = (**changes))]
                fn replace(&self, changes: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
                    replaced(self, changes)
                }
                /// One built from `fields` by name: a field left out takes its default; a needed one
                /// left out is refused by its name.
                #[new]
                #[pyo3(signature = (**fields))]
                fn new(fields: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
                    merged(serde_json::Map::new(), fields)
                }
            }
        )*};
    }
    methods!(
        Binding, MemoSettings, Construction, CallInterface, InputMetadata, InputFile,
        ByteGrant, CallProgress, Answer, PublishPart, SavedFile
    );

    macro_rules! request_methods {
        ($($t:ident),*) => {$(
            #[pymethods]
            impl $t {
                fn __repr__(&self) -> String {
                    format!("{self:?}")
                }
                /// Its request's members, `kind` included.
                fn json(&self) -> PyResult<String> {
                    serde_json::to_string(&Request::$t(self.clone())).map_err(|e| PyValueError::new_err(e.to_string()))
                }
                /// A copy with `changes` in place of its fields.
                #[pyo3(signature = (**changes))]
                fn replace(&self, changes: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
                    replaced(self, changes)
                }
                /// One built from `fields` by name: a field left out takes its default; a needed one
                /// left out is refused by its name.
                #[new]
                #[pyo3(signature = (**fields))]
                fn new(fields: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
                    merged(serde_json::Map::new(), fields)
                }
            }
        )*};
    }
    request_methods!(
        ChildEvents, ChildCall, ChildPoll, ChildCancel, ChildForget, GpuRelease, ModelPrefetch, Publish,
        CheckpointItem, StageMemoLookup, StageMemoStore
    );

    macro_rules! command_methods {
        ($($t:ident => $wrap:expr),*) => {$(
            #[pymethods]
            impl $t {
                fn __repr__(&self) -> String {
                    format!("{self:?}")
                }
                /// Its frame, `cmd` included.
                fn json(&self) -> PyResult<String> {
                    let wrap: fn($t) -> Command = $wrap;
                    serde_json::to_string(&wrap(self.clone())).map_err(|e| PyValueError::new_err(e.to_string()))
                }
                /// A copy with `changes` in place of its fields (`msgspec.structs.replace`).
                #[pyo3(signature = (**changes))]
                fn replace(&self, changes: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
                    replaced(self, changes)
                }
                /// One built from `fields` by name: a field left out takes its default; a needed one
                /// left out is refused by its name.
                #[new]
                #[pyo3(signature = (**fields))]
                fn new(fields: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
                    merged(serde_json::Map::new(), fields)
                }
            }
        )*};
    }
    command_methods!(
        Start => Command::Start,
        Load => |c| Command::Load(Box::new(c)),
        PrepareRequest => Command::PrepareRequest,
        Cancel => Command::Cancel,
        Preview => Command::Preview,
        Invoke => |c| Command::Invoke(Box::new(c)),
        Map => Command::Map,
        Fork => Command::Fork
    );
}

/// Public TensorFS transport options. TensorD forwards a supplied token without acquiring one: at
/// the run's Hub, the run capability its computer minted, never an account's sign-in.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "python", pyo3::pyclass(module = "tensorfs.executor", frozen, eq, get_all, skip_from_py_object))]
#[serde(default)]
pub struct TensorFSOptions {
    pub endpoint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// When `token` expires; 0: unknown.
    #[serde(skip_serializing_if = "is_zero")]
    pub expires_unix_ms: i64,
    /// An endpoint's private CA, base64 DER on the wire.
    #[serde(with = "base64_bytes", skip_serializing_if = "Vec::is_empty")]
    pub ca_der: Vec<u8>,
}

impl std::fmt::Debug for TensorFSOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TensorFSOptions")
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Bytes as base64 text.
pub mod base64_bytes {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    /// The bytes as standard base64 text.
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    /// Bytes from standard base64 text; text that is not base64 is an error.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod checkpoint_name_tests {
    use super::*;

    #[test]
    fn checkpoint_identity_source_name_preserves_the_executor_frame() {
        let modern = serde_json::json!({"output": "model", "model_id": "local/derived-test", "revision": "sha256:abc"});
        let legacy = serde_json::json!({"output": "model", "repo_id": "local/derived-test", "revision": "sha256:abc"});
        let new: CheckpointItem = serde_json::from_value(modern).unwrap();
        let old: CheckpointItem = serde_json::from_value(legacy).unwrap();
        assert_eq!(new, old);
        assert_eq!(new.model_id, "local/derived-test");
        let frame = serde_json::to_value(Request::CheckpointItem(new)).unwrap();
        assert_eq!(frame["kind"], "checkpoint_item");
        assert_eq!(frame["repo_id"], "local/derived-test");
        assert!(frame.get("model_id").is_none());
        assert!(serde_json::from_value::<CheckpointItem>(serde_json::json!({
            "output": "model", "model_id": "one/model", "repo_id": "other/model", "revision": "sha256:abc"
        })).is_err());
    }
}
