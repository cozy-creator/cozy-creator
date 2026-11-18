//! The machine API (design/machine-http-api.md): HTTP + JSON on the machine's one pinned-TLS port,
//! every request carrying `authorization: Cozy-Cap <token>`. Live state and run events are SSE;
//! files move by ranged `PUT`/`GET`. Refusals are [`crate::error::Envelope`]. Unknown fields
//! are ignored, and clients skip SSE events they do not know.
pub use executor_api::TensorFSOptions;
use executor_api::base64_bytes;
use serde::{Deserialize, Serialize};

pub mod capability;

/// The one DNS name every machine's self-signed leaf carries; clients pin the leaf itself.
pub const SERVER_NAME: &str = "cozy-worker";

/// An authenticated local controller's protected execution storage. Only the NEW
/// `/v1/local-private/{project_id}` namespace interprets this descriptor; it is never
/// an ignored optional privacy switch on the ordinary machine endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPrivateStorage {
    pub project_id: String,
    pub generation: u64,
    pub view_root: std::path::PathBuf,
    /// An unencrypted project's storage: its executors take the ordinary transport, as any
    /// Runtime speaks it. Absent, an encrypted view's executors need the protected one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ordinary: bool,
}

pub const LOCAL_PRIVATE_PREFIX: &str = "/v1/local-private";
pub const LOCAL_PRIVATE_DESCRIPTOR: &str = "cozy-local-private-storage";
pub const LOCAL_PRIVATE_TOKEN: &str = "cozy-local-private-token";
pub const LOCAL_PRIVATE_MARKER: &str = ".vault-local-execution.json";

/// A project's machine area in its own storage (`DIR`): TensorD's state for the project's runs.
/// Its journal, results and objects are the project's. What `rebuildable` names TensorD makes
/// again on next use, so a storage conversion leaves it out.
pub mod project_area {
    pub const DIR: &str = ".vault-machine";
    /// Package environments, by identity.
    pub const GENERATIONS: &str = "generations";
    /// Local sources' editable environments, beside `GENERATIONS`.
    pub const EDITABLE: &str = "editable";
    /// Each executor's working root.
    pub const EXECUTORS: &str = "executors";
    /// A launch's home, compiled kernels and JIT scope.
    pub const HOME: &str = "home";
    pub const KERNELS: &str = "kernels";
    pub const JIT: &str = "jit";
    /// Where child tools are pointed: temporary files, uv's cache, tools and Pythons, XDG folders, bytecode.
    pub const TMP: &str = "tmp";
    pub const UV_CACHE: &str = "uv-cache";
    pub const UV_TOOLS: &str = "uv-tools";
    pub const PYTHON: &str = "python";
    pub const CONFIG: &str = "config";
    pub const DATA: &str = "data";
    pub const STATE: &str = "state";
    pub const CACHE: &str = "cache";
    pub const BYTECODE: &str = "bytecode";
    pub const MATPLOTLIB: &str = "matplotlib";
    /// The project's own PyTorch base, `torch-<version>`, staged in `TORCH_STAGING`.
    pub const TORCH_BASE: &str = "torch-";
    pub const TORCH_STAGING: &str = ".torch.partial";

    const REBUILT: [&str; 16] = [GENERATIONS, EDITABLE, EXECUTORS, HOME, KERNELS, JIT, TMP, UV_CACHE, UV_TOOLS, PYTHON, CONFIG, DATA, STATE, CACHE, BYTECODE, MATPLOTLIB];

    /// Whether `name`, an entry of the area, is one TensorD recreates when it is gone.
    pub fn rebuildable(name: &str) -> bool {
        REBUILT.contains(&name) || name == TORCH_STAGING || name.strip_prefix(TORCH_BASE).is_some_and(|version| !version.is_empty())
    }

    /// The `torch-<version>` base folder name.
    pub fn torch_base(version: &str) -> String {
        format!("{TORCH_BASE}{version}")
    }
}

/// The strict project-owned execution contract. Older partial-private endpoints are not substitutes.
pub const PROJECT_EXECUTION_PREFIX: &str = "/v1/project-execution";
pub type ProjectExecutionScope = LocalPrivateStorage;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectExecutionCapabilities {
    pub protected_journal: bool,
    pub protected_objects_and_scratch: bool,
    /// Reserved broader sandbox capability; managed-storage readiness does not require it.
    #[serde(default)]
    pub filesystem_write_confinement: bool,
    pub quiescent_seal: bool,
}

impl ProjectExecutionCapabilities {
    pub fn ready(&self) -> bool {
        self.protected_journal && self.protected_objects_and_scratch && self.quiescent_seal
    }
}

/// A proof file written only INSIDE the live encrypted mount. Its random token is sent
/// as a private authentication header, never as RunSpec, URL, log or global journal data.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPrivateMarker {
    pub format_version: u32,
    pub storage: LocalPrivateStorage,
    pub token: String,
}

impl std::fmt::Debug for LocalPrivateMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPrivateMarker")
            .field("format_version", &self.format_version)
            .field("storage", &self.storage)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// Routes, as axum templates.
pub mod path {
    /// Verified local encrypted-project namespace; forwards GET/POST/PUT to existing data routes.
    pub const LOCAL_PRIVATE: &str = "/v1/local-private/{project_id}/{*rest}";
    pub const PROJECT_EXECUTION: &str = "/v1/project-execution/{project_id}/{*rest}";
    /// Close only this project's idle local execution namespace before unmount.
    pub const PRIVATE_CLOSE: &str = "/v1/project-execution/{project_id}/close";
    /// `GET` with no credential: 204 while the machine serves.
    pub const HEALTH: &str = "/v1/health";
    /// `GET`: one JSON [`super::StatusFrame`]; SSE `status` frames with `Accept: text/event-stream`.
    pub const STATUS: &str = "/v1/status";
    /// `POST`: renews a rental's idle deadline; answers the frame.
    pub const KEEPALIVE: &str = "/v1/keepalive";
    /// `POST` by this computer's controller: a project whose folder is gone (cl-1031). Every record of it here goes, as a lock's
    /// does: 204 once nothing of it is here, 409 while a folder it ran in still holds it or its runs are active.
    pub const PROJECT_GONE: &str = "/v1/project-gone/{project_id}";
    /// `POST` a [`super::ViewGone`] by this computer's controller: what a lock that couldn't reach this machine owes it (cl-1064).
    /// Every project it ran in that vault view has its runs cancelled and its records retired, as a lock's close would have. 204.
    pub const PROJECT_GONE_AT: &str = "/v1/project-gone";
    /// `POST` a [`super::RunRequest`] of [`super::RunKind::Update`], the one request a controller of
    /// another release sends; `GET` a [`super::RunList`], `?before=<number>&limit=`.
    pub const RUNS: &str = "/v1/runs";
    /// `POST` a [`super::RunBatch`]: work, admitted as one.
    pub const RUNS_BATCH: &str = "/v1/runs/batch";
    /// `GET`: SSE of the run's [`super::RunEvent`]s after `Last-Event-ID`.
    pub const EVENTS: &str = "/v1/runs/{id}/events";
    /// `POST`: cancels the run; answers its [`super::RunState`].
    pub const CANCEL: &str = "/v1/runs/{id}/cancel";
    /// `GET` with `Range` and `If-Match`; `?index=&member=`.
    pub const OUTPUT: &str = "/v1/runs/{id}/outputs/{output}";
    /// `GET`: the run's newest preview (cr-282), `ETag: "p<n>"`, its step in `Cozy-Preview-Step`; 204
    /// while it has none. Each ask makes the run's executor make them for a while.
    pub const PREVIEW: &str = "/v1/runs/{id}/preview";
    /// `GET` with `?tail=`.
    pub const LOG: &str = "/v1/logs/{name}";
    /// `PUT` with `Content-Range`; `GET` as SSE, what its upload holds; `{digest}` is `sha256:<hex>`.
    pub const OBJECT: &str = "/v1/objects/{digest}";
    /// `POST` a [`super::HeldRequest`]: what the machine holds of many objects, in one request.
    pub const OBJECTS_HELD: &str = "/v1/objects/held";
    /// `PUT` an [`super::Upcoming`]: the jobs the caller expects to send here, nearest first (cl-967).
    pub const UPCOMING: &str = "/v1/upcoming";
    /// `PUT` a [`super::Prepare`]: what the caller expects demand for here, with no job yet (cl-967).
    pub const PREPARE: &str = "/v1/prepare";
    /// Remove one repository's locally cached model copies; never changes its origin.
    pub const MODELS_REMOVE: &str = "/v1/models/remove";
    /// `GET` as SSE: `gpus` events, a [`super::GpuUse`] each second while the caller stays, its GPUs read only then (cl-1100).
    pub const GPUS: &str = "/v1/gpus";
}

/// A preview's step, `<position>/<total>` (`path::PREVIEW`).
pub const PREVIEW_STEP: &str = "cozy-preview-step";

/// A vault view a lock ended while this machine didn't answer (cl-1064): the path it was mounted at, new at each unlock.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ViewGone {
    pub view: std::path::PathBuf,
}

/// Remove all locally cached releases and lanes of one model repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemoveModel {
    pub model: String,
}

/// Local removal leaves active readers and content shared with other models intact.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemovedModel {
    pub model: String,
    pub removed: bool,
    /// Bytes removed from cache object paths, not a measurement of freed filesystem blocks.
    pub cache_bytes_removed: u64,
    /// Candidate bytes kept because they are in use, shared, or could not safely be removed.
    pub retained_bytes: u64,
}

/// The header naming the byte ranges of an object the machine holds: `0-1048575,2097152-4194303`
/// (inclusive, as `Content-Range` counts); empty when it holds none.
pub const HELD: &str = "cozy-held";

/// Byte ranges `[start, end)`, sorted and apart.
pub type Ranges = Vec<(u64, u64)>;

/// `ranges` as [`HELD`] spells them.
pub fn spell_held(ranges: &[(u64, u64)]) -> String {
    let spelled: Vec<_> = ranges
        .iter()
        .filter(|(start, end)| end > start)
        .map(|(start, end)| format!("{start}-{}", end - 1))
        .collect();
    spelled.join(",")
}

/// A [`HELD`] value as ranges, or None when it is malformed.
pub fn parse_held(value: &str) -> Option<Ranges> {
    let mut ranges: Ranges = vec![];
    for range in value.split(',').map(str::trim).filter(|r| !r.is_empty()) {
        let (first, last) = range.split_once('-')?;
        let (first, last): (u64, u64) = (first.parse().ok()?, last.parse().ok()?);
        if last < first || ranges.last().is_some_and(|(_, end)| first <= *end) {
            return None;
        }
        ranges.push((first, last + 1));
    }
    Some(ranges)
}

/// The bytes `ranges` cover.
pub fn held_bytes(ranges: &[(u64, u64)]) -> u64 {
    ranges
        .iter()
        .map(|(start, end)| end.saturating_sub(*start))
        .sum()
}

/// What of `[0, length)` `ranges` does not cover.
pub fn missing(ranges: &[(u64, u64)], length: u64) -> Ranges {
    let mut gaps = vec![];
    let mut at = 0;
    for &(start, end) in ranges {
        if start > at {
            gaps.push((at, start.min(length)));
        }
        at = at.max(end);
    }
    if at < length {
        gaps.push((at, length));
    }
    gaps.retain(|(start, end)| end > start);
    gaps
}

/// `POST /v1/objects/held`: which of these objects (`sha256:<hex>`) the machine holds for this
/// signer. Calcifer's `POST /v1/files/held` and `/v1/project-files/held` take it too.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct HeldRequest {
    pub digests: Vec<String>,
}

/// What the machine holds of each object asked, in the order asked; a stored object is held
/// whole, and asking renews it, as a `HEAD` does.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct HeldAnswer {
    pub held: Vec<HeldObject>,
}

/// One object asked: the byte ranges of `digest` the machine holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct HeldObject {
    pub digest: String,
    /// Byte ranges `[start, end)`; none: nothing held.
    pub ranges: Ranges,
}

/// What an upload holds, as `GET /v1/objects/sha256:<hex>?length=N` streams it (`event: held`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldBytes {
    /// Bytes landed, synced or not; the last event says all N.
    pub held: u64,
}

/// Whole lines of a machine log, as `GET /v1/logs/{name}` streams them (`event: lines`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogLines {
    /// The place after them, also the event's id: a follow resumed there goes on with the next line.
    pub after: String,
    pub text: String,
}

/// Raw JSON, kept as its exact text: canonical bytes and 64-bit integers survive.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Json(pub Box<serde_json::value::RawValue>);

impl Json {
    /// `text` if it is JSON.
    pub fn parse(text: impl Into<String>) -> Result<Self, serde_json::Error> {
        serde_json::value::RawValue::from_string(text.into()).map(Self)
    }
    pub fn get(&self) -> &str {
        self.0.get()
    }
}

impl PartialEq for Json {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

// ---- Run ------------------------------------------------------------------------------------

/// `POST /v1/runs`: submits when `id` is new (idempotent on id per signer); the same id with
/// another spec is `run_id_conflict`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunRequest {
    /// The caller's run id, 1-256 bytes.
    pub id: String,
    pub spec: RunSpec,
}

/// `POST /v1/runs/batch` (cl-816): work, admitted as one. Each new run is accepted with the others
/// before any prepares, so the machine packs them together, as its estimate quoted them. An update
/// is not work: it goes to `POST /v1/runs`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunBatch {
    pub runs: Vec<RunRequest>,
    /// The id of the run of `runs` whose events the answer goes on with, asked as `text/event-stream` (cl-1172): the answer is
    /// then its [`ADMITTED`] event, and the run's events follow it as `GET /v1/runs/{id}/events` sends them from its start, so one
    /// round trip admits a run and follows it. Empty, or a machine that doesn't stream it: the answer alone, as JSON.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub follow: String,
    /// Small objects its runs read, whole (cl-1172): written here as `PUT /v1/objects/{digest}` writes them, before the runs are
    /// admitted, so a run's small inputs take no round trip of their own. Sent only to a machine whose Status says it takes
    /// them (`StatusFrame::carried_objects`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<CarriedObject>,
}

/// One small object a batch carries (`RunBatch::objects`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CarriedObject {
    /// `sha256:<hex>` of `data`.
    pub digest: String,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

/// The SSE event that carries a batch's [`RunBatchAnswer`] ahead of its followed run's events (`RunBatch::follow`).
pub const ADMITTED: &str = "admitted";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunBatchAnswer {
    /// One per run sent, in its order.
    pub runs: Vec<Admitted>,
}

/// One run of a batch, as `POST /v1/runs` would answer it alone: its state, or its refusal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Admitted {
    /// The machine accepted it now; false: its id and spec already named it.
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<RunState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<crate::error::Error>,
}

/// What a run does: a call, a warm, a software update or a removal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// Every function is a call.
    #[default]
    Call,
    /// Prepare only: install and download, then succeed.
    Warm,
    /// Stage, verify and activate a software cohort at idle.
    Update,
    /// The caller's installations of the source leave the machine: an installation by
    /// its name, or a release's package (its release, or every release when empty) from the run's
    /// Hub (local/ packages from none). Their environments are deleted once no process and no
    /// other installation holds them.
    Remove,
}

/// How far a function is kept ready; each level includes the ones before it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WarmLevel {
    /// Not kept: a warm set's item at this level leaves it.
    #[default]
    Off,
    /// The package environment.
    Installed,
    /// The function's bound weights in the store.
    Downloaded,
    /// A process with its imports done: no device, no VRAM.
    Imported,
    /// An executor with the model constructed, weights in the host tier.
    Host,
    /// Its weights on the GPU.
    Gpu,
}

/// What a run runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Release(Release),
    /// An installation this machine already holds for the signer.
    Installation(String),
    Local(LocalSource),
}

/// What a warm set's member keeps ready.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmSource {
    Release(Release),
    Installation(String),
}

/// One member of a caller's warm set: a function the machine keeps ready up to `level`. It is
/// the last idle thing to give its memory to a request, and comes back when room returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WarmItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<WarmSource>,
    pub entrypoint: String,
    /// None: the slots' defaults, as for a run.
    pub models: Vec<ModelChoice>,
    pub level: WarmLevel,
    /// Status only: the level it holds now (installed, downloaded, imported, host, gpu).
    pub holds: String,
    /// Status only: why that is lower than `level`.
    pub held_back: String,
    /// As [`RunSpec::lane`].
    pub lane: String,
    /// Optional typed inputs for a known GPU program. None warms weights only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Json>,
}

/// The functions a caller keeps ready; [`RunSpec::set`] replaces it whole.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WarmSet {
    pub items: Vec<WarmItem>,
}

/// What a run runs and how: its kind, source, function, payload, inputs, models, Hub and GPUs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunSpec {
    pub kind: RunKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    pub entrypoint: String,
    /// The function's parameters, a JSON object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Json>,
    /// Objects uploaded with `PUT /v1/objects`, bound to payload fields.
    pub inputs: Vec<InputFile>,
    pub models: Vec<ModelChoice>,
    pub attention_kernel: String,
    /// Dev-only: the Runtime's attention capture spec (JSON); empty: none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attention_capture: String,
    /// org/name: the requester's repository for the call's weights outputs, or a warm run's
    /// source model.
    pub weights_destination: String,
    /// The Hub the run reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubAccess>,
    /// Ordinary TensorFS connection settings supplied by the caller, forwarded to Runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tensorfs: Option<TensorFSOptions>,
    /// The account that org-relative references of local code name.
    pub owner: String,
    /// Provider tokens for source models, kept in memory only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<ProviderAccess>,
    /// The caller's whole warm set, replacing its previous one; no items clears it.
    /// The run installs and downloads every member, and its result lists each one's `level` and
    /// `held_back`. Absent: the set is left as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<WarmSet>,
    /// 0: widest group every model slot supports on this machine. Positive: at most this many
    /// GPUs, for the run and its children.
    pub gpus: u32,
    /// The main model's lane, a label (`bf16`, `fp8`, …). Empty, and every other slot: the Hub
    /// picks for the machine's hardware. For a model-only warm run with a weights destination,
    /// the lane its commit is labelled. The package's own children inherit it; another
    /// package's take none.
    pub lane: String,
    /// A real call that is the request a list item predicted (cl-1123): its key. One the machine finds identical to that item's
    /// (its run identity, `sha256` of the spec as sent, access aside) adopts what the item made ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted: Option<String>,
}

/// The caller's provider tokens, for models a run makes from a provider source.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderAccess {
    pub huggingface: String,
    pub civitai: String,
}

/// The tokens are never printed.
impl std::fmt::Debug for ProviderAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderAccess").finish_non_exhaustive()
    }
}

/// A published release by name (th-245): a machine that holds no installation of it reads its
/// card and locked requirements at the Hub by name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Release {
    /// org/name
    pub package: String,
    /// Empty: the newest its signer holds here, else the Hub's newest.
    pub release: String,
    /// The release digest its files must have (`hub::types::release_digest`): a project's lock, or the card this computer
    /// keeps (xs-196). Empty: none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest: String,
    /// The release's install card from its controller, which holds it (cl-1127): `sha256:<hex>` of a JSON object written with `PUT
    /// /v1/objects`, `{interface, python, pyproject, lock, wheels: [{url, sha256, name, length}], digest}`, each wheel (and each its
    /// lock takes from a Hub index) written too. The machine installs from it and reads nothing of the release at its Hub. Empty:
    /// it reads the Hub's install plan.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub card: String,
}

/// Local code: `manifest` names a JSON object written with `PUT /v1/objects`:
/// `{package: "local/<dist>", release, python_requires, python_version, callees: {distribution:
/// package}}` and its code, by content. The project is installed editable from `files: [{name,
/// digest, length, executable}]`: the project and the path dependencies its lock names, each file
/// under their common directory, with `project` the project's directory there. The machine keeps a
/// tree per manifest and an environment per signer, package and what it is built from (lock,
/// pyproject, path dependencies, wheels): an edit to the code alone reuses it. `wheels: [{name,
/// digest, length}]` are locked dependencies the caller carries because the machine cannot fetch
/// them. Members are objects the same signer wrote. No machine reads a host path: a manifest naming
/// `path` is refused, `local_source_path_refused`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSource {
    /// `sha256:<hex>` of the manifest object.
    pub manifest: String,
}

/// An uploaded object bound to a payload field.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputFile {
    /// The payload field path, list index included.
    pub field: String,
    /// `sha256:<hex>`
    pub digest: String,
    pub length: u64,
    pub media_type: String,
    pub order: u32,
}

/// The parameter of a model-only upload's base (`cozy model upload --base`): `base` keeps the
/// base's components the uploaded model lacks and takes its own in place of the base's;
/// `base:<component>[,…]` replaces only those: the uploaded model's of each name, or its one
/// component with tensors under every name. Of Hub checkpoints the machine holds only headers and
/// trees: their components stay the Hub's.
pub const BASE: &str = "base";
/// A weights upload's choice `add:<component>/<path>`: a file it adds to the checkpoint as that component's asset.
pub const ADD: &str = "add:";

/// One model slot's choice, by name (th-245: the machine resolves it at the Hub). A Hub model is
/// `repository` at `release` (empty: the newest) in `lane`, else in the lane the Hub picks for
/// the machine's hardware. `manifest` names a commit (or a header) by hash: one of the
/// repository's at the Hub (with `repository`), or one this machine holds (without). A slot with
/// no choice takes its declared default release, a callee's included (`<package>/<path>`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelChoice {
    /// A root slot parameter or full slot path; in a model-only upload, [`BASE`] names the model
    /// it starts from.
    pub parameter: String,
    // The supported machine protocol still names this field repository.
    #[serde(rename = "repository")]
    pub model: String,
    pub release: String,
    pub lane: String,
    /// `sha256:<hex>`
    pub manifest: String,
    pub manifest_length: u64,
    /// Each by name, a hash of the owner's, or a provider source.
    pub adapters: Vec<Adapter>,
    /// A provider source (`hf://<org>/<repo>@<rev>[/member]`, `civitai://<version>[/file]`) the
    /// machine makes the slot's model from, under these reviewed `profiles` (none: the one its
    /// headers select); never with repository or manifest. A model-only warm upload also accepts
    /// `input://<field>` naming its retained tree input: a plain CozyTensors folder, without
    /// selectors or conversion profiles. Missing folder payload must already be at the endpoint. A
    /// file reaches a machine only as `object://`: a path on it is refused, `local_model_path_refused`.
    pub source: String,
    pub profiles: Vec<String>,
    /// The base component a LoRA source was trained on, as its uploader says (`--trained-on`);
    /// empty: what its producer's fields declare, or nothing.
    pub trained_on: String,
}

/// An adapter (a LoRA) on one component of a slot's model, at `scale`: a Hub model or a provider source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adapter {
    pub component: String,
    pub model: String,
    pub release: String,
    pub lane: String,
    /// `sha256:<hex>`, a commit of `model` by hash; else `model` at `release`, `lane`.
    pub manifest: String,
    /// Canonical decimal; empty means 1. Calcifer gives a LoRA it was given without a strength its
    /// recommended one first.
    pub scale: String,
    /// A provider source (`civitai://<version>`, `hf://…`) made here and normalized at ingest,
    /// under these reviewed `profiles` (none: the one its headers select); never with `model`.
    pub source: String,
    pub profiles: Vec<String>,
}

/// A Hub a run reads. Public content is read anonymously.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HubAccess {
    pub origin: String,
    /// An origin's private CA (DER, base64), when it has one.
    #[serde(with = "base64_bytes", skip_serializing_if = "Vec::is_empty")]
    pub ca_der: Vec<u8>,
    /// The run capability its computer signed for this machine's workload key (cl-675): what
    /// the signer's runs here read and write at this Hub, redeemed by the machine alone (AuthKit's
    /// jwt-bearer grant). Empty for runs that read only public releases.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub capability: String,
}

impl std::fmt::Debug for HubAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubAccess")
            .field("origin", &self.origin)
            .field("capability", &(!self.capability.is_empty()).then_some("<redacted>"))
            .finish_non_exhaustive()
    }
}

/// One entry of a run's log, as `GET /v1/runs/{id}/events` sends it: SSE `event: <variant>`,
/// `id: <sequence>` for durable entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    /// 0: a frame that is not in the log (the snapshot, progress before acceptance, live state).
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub at_ms: i64,
    pub event: Event,
}

/// What a run's log entry carries; its variant is the SSE event name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    State(RunState),
    Progress(Progress),
    Product(Product),
    Log(LogLine),
    Outcome(Outcome),
    Call(Call),
    Gpu(GpuAllocation),
    /// Live, never in its log: its SSE data is the Responses API's event object alone.
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(OutputTextDelta),
}

impl Event {
    /// Its SSE event name.
    pub fn name(&self) -> &'static str {
        match self {
            Event::State(_) => "state",
            Event::Progress(_) => "progress",
            Event::Product(_) => "product",
            Event::Log(_) => "log",
            Event::Outcome(_) => "outcome",
            Event::Call(_) => "call",
            Event::Gpu(_) => "gpu",
            Event::OutputTextDelta(_) => OUTPUT_TEXT_DELTA,
        }
    }
    /// The SSE event names a run's stream carries.
    pub const NAMES: [&'static str; 8] = [
        "state", "progress", "product", "log", "outcome", "call", "gpu", OUTPUT_TEXT_DELTA,
    ];
}

/// The Responses API's event for words a text output made (xs-209).
pub const OUTPUT_TEXT_DELTA: &str = "response.output_text.delta";

/// Words a run's text output made since its follower last heard (xs-209), as the Responses API's
/// `response.output_text.delta` says them: live, held only in memory while the run goes, and each
/// follower's from its own place (one attaching mid-run hears the words so far first). `item_id`
/// is the output's name, `<name>/<n>` for a list's item `n`; `output_index` that item's place, from 0.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputTextDelta {
    #[serde(rename = "type")]
    pub kind: String,
    pub item_id: String,
    pub output_index: u32,
    pub content_index: u32,
    pub delta: String,
    /// Its place on its follower's stream, from 0.
    pub sequence_number: u64,
}

impl OutputTextDelta {
    /// `delta` of output `output`'s item `index` (0: a single output; from 1, a list's).
    pub fn new(output: &str, index: u32, delta: String, sequence_number: u64) -> Self {
        let item_id = if index == 0 { output.to_owned() } else { format!("{output}/{index}") };
        Self { kind: OUTPUT_TEXT_DELTA.into(), item_id, output_index: index.saturating_sub(1), content_index: 0, delta, sequence_number }
    }

    /// The output and item it is of: `(name, 0)` for a single output, `(name, n)` for a list's item `n`.
    pub fn output(&self) -> (&str, u32) {
        match self.item_id.split_once('/') {
            Some((name, n)) => (name, n.parse().unwrap_or(0)),
            None => (self.item_id.as_str(), 0),
        }
    }
}

/// A root run's GPU allocation changed: `grant` binds its lane, `wait` queues it behind `behind`
/// (a run id), `release` ends its lane (`Context.release_gpus()` or its exit).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuAllocation {
    pub state: String,
    /// The lane, as machine GPU indices.
    pub gpus: Vec<u32>,
    pub width: u32,
    pub behind: String,
    /// `wait`: about how long until it gets them, ms, as the machine's packing times the runs holding them (cl-1187); 0: not known,
    /// as none of them has been timed there.
    #[serde(skip_serializing_if = "is_zero")]
    pub left_ms: u64,
}

/// A run as its machine tracks it: number, state, log head, what it waits for and its times.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunState {
    pub id: String,
    /// This machine's run number.
    pub number: u64,
    /// queued, preparing, needs_access (a restart interrupted its preparation: the same id and spec
    /// sent again bring its access), starting (out of its queue, its executor starting: `waiting` says
    /// what it does, `importing <package>`, `starting the executor`), running, succeeded, failed, canceled
    pub state: String,
    /// The log's head.
    pub sequence: u64,
    pub attempt: u32,
    /// Why a queued run waits, for people. A run another of this machine's clients sent is named by its id there
    /// (`run-<hex>`), which its client names as people know it.
    pub waiting: String,
    /// Its place in this machine's line while it waits there, 1 next.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place: Option<u32>,
    /// The runs it waits for, by id (an update waits for those in flight).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting_for: Vec<String>,
    /// When it began executing (`running`), Unix ms: its time counts from here, never its queue's.
    /// 0 before it does.
    pub started_at_ms: u64,
    /// What it runs, `<package>/<function>`; empty for an update.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// When the machine accepted it and when it ended, Unix ms; 0 until then.
    pub accepted_at_ms: u64,
    pub finished_at_ms: u64,
    /// A list item's (`StatusFrame::ahead`): how ready its machine is for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness: Option<Readiness>,
    /// Why a list's item failed (`StatusFrame::ahead`, cl-1104): what its caller decides by whether to send it again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
}

/// How ready a machine is for a predicted request (cl-1123): `planning` (its request not planned yet), `installing`, `downloading`,
/// `ready` (its weights here), `loading`
/// (its executor starting and its weights going onto a GPU), `loaded` (they are there: its first step loads nothing), `evicted`
/// (a run took that GPU memory since), `running`, `done`. `done`/`total` count what the stage counts, in bytes; `reason` says why
/// it stops where it is (`refused`: the machine won't make it ready, and why).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Readiness {
    pub stage: String,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// `GET /v1/runs`: the runs the caller reads, newest first; on a rental its owner reads every
/// one, whichever of its clients sent it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunList {
    pub runs: Vec<RunState>,
}

/// How far a run's current stage is: counts, bytes and step times.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub stage: String,
    /// How far the run is: its time running over its machine's estimate of its whole, short of 1;
    /// -1: no estimate.
    pub fraction: f64,
    pub completed: u64,
    pub total: u64,
    /// What `completed` counts: empty for steps, `tokens` for a decode's (`calcifer::Progress::unit`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// The latest step's time, while a stage steps.
    pub step_ms: f64,
    /// The median of its last five steps after the first, which carries the stage's setup: its
    /// steady pace, robust to a slow step; 0 before its second.
    pub step_steady_ms: f64,
    /// How far this stage is, when it says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_fraction: Option<f64>,
    /// The number of the run's newest preview (`path::PREVIEW`), from 1; 0: none.
    #[serde(skip_serializing_if = "no_preview")]
    pub preview: u64,
}

fn no_preview(n: &u64) -> bool {
    *n == 0
}

/// A weights output's commit: `revision` (`sha256:<tree>`) of `repo_id`, in `lane`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct CheckpointRef {
    #[serde(rename = "repo_id")]
    pub model_id: String,
    pub revision: String,
    pub lane: String,
}

/// One revision of an output: a single output is replaced by each, a list output grows. Byte
/// outputs use `GET /v1/runs/{id}/outputs/{output}`; checkpoints are immutable references.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Product {
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointRef>,
    /// A list item's 1-based index; 0 for a single output.
    pub index: u32,
    pub rev: u64,
    pub length: u64,
    /// `sha256:<hex>` of these bytes.
    pub digest: String,
    pub media_type: String,
    pub label: String,
    /// Media time, for a growing video.
    pub duration_us: u64,
    /// These bytes extend the previous revision's, from here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appended_from: Option<u64>,
}

/// A line the run logged, at its level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogLine {
    pub level: String,
    pub text: String,
    /// A warning the machine says of the run itself, by its stable code (`gpu_throttled`, `runtime_without_kernels`): its
    /// controller keeps it among the run's warnings (cl-913). Empty: a line only.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
}

/// How a run ended: status, reason, result and outputs, with its measurements and memo facts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Outcome {
    /// succeeded, failed, canceled
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// The result, canonical JSON, assets as digests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Json>,
    pub outputs: Vec<Product>,
    /// What its executor measured, canonical JSON: `attribution` (stage and step tracks),
    /// `execution` (degree, ranks with the GPU and attention each served, executor boot,
    /// construction load and warm) and `observations` (attention rows). None for a run that ran
    /// on no device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurements: Option<Json>,
    /// Its own time running its callable, every attempt summed; 0: unknown.
    pub execution_ms: u64,
    /// A memoizable callable's computation (`sha256:<hex>`), whether it ran or was answered.
    pub computation_digest: String,
    /// Answered from a result this machine held: nothing ran.
    pub memoized: bool,
    /// Why it was neither looked up nor held (its author's condition unmet, e.g. "seed not given").
    pub unmemoized: String,
    /// What keyed a memoizable callable's code: `closure` (its module and the modules of its
    /// package it imports), `package` (its package's whole source) or `identity` (another
    /// package's declared operation identity). Empty from a machine that predates it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub memo_basis: String,
    /// How its executor served the request other than as sent, in the executor's order. Advisory.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<Adjustment>,
    /// The GPU it ran on, by name (`NVIDIA GeForce RTX 4090`), as its controller recorded it: what
    /// a later run's time left is learned from on another machine of that GPU. Empty: unknown.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gpu: String,
}

/// One request field served other than as sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Adjustment {
    /// Its path in the request: `steps`, `assets.2.asset`.
    pub field: String,
    /// `adjusted`, `clamp`, `warn` or `ignored` (an input the function has no use for).
    pub kind: String,
    pub requested: String,
    pub applied: String,
    pub reason: String,
    /// `handler`, or what the Runtime applied: `policy`, `checkpoint:…`, `adapter:…`.
    pub source: String,
}

/// Why a run or call did not succeed: a stable code, message and origin, with detail and next commands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Reason {
    pub code: String,
    pub message: String,
    /// request, runtime, author, machine
    pub origin: String,
    /// A failed call's traceback, else its executor's stderr: the last lines, secrets masked.
    /// It explains the failure after its machine is gone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The exact commands to run next, when a refusal leaves one; at most two.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
}

/// One settled call the run made: a child run, or an effect such as its weights publication.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Call {
    /// The child run's id.
    pub run: String,
    /// Its index among the run's calls.
    pub index: u32,
    pub function: String,
    /// The author's label for it.
    pub label: String,
    /// succeeded, failed, canceled
    pub status: String,
    pub called_at_ms: i64,
    pub finished_at_ms: i64,
    /// Its executor's, as [`Outcome::measurements`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurements: Option<Json>,
    /// Why it did not succeed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// Answered from a result this machine held; nothing ran.
    pub memoized: bool,
    /// A memoized call's computation (`sha256:<hex>`), ran or answered.
    pub computation_digest: String,
    /// Why a memoizable call was neither looked up nor held ("seed not given").
    pub unmemoized: String,
    /// As [`Outcome::memo_basis`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub memo_basis: String,
}

// ---- Predicted requests ---------------------------------------------------------------------

/// `PUT /v1/upcoming` (cl-967, Paul 11-12): the jobs the caller will send here, nearest first, its whole list, replacing its last.
/// Each is made ready behind every run, in its order; one the list drops stops. A run later sent for it finds what it made ready.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Upcoming {
    pub jobs: Vec<Predicted>,
}

/// `PUT /v1/prepare` (cl-967): what the caller expects demand for here with no job yet, its whole set, replacing its last: its
/// guesses (cl-1104: what a render of a box would run, from the moment a tool is on it), and many shots of one function queued
/// elsewhere with this machine new. Each behind every upcoming job and every run's guesses, and no job: it holds no rental, gives
/// the install line and the disk to any run, and what only it brought is the first to go. One the set drops stops once what the
/// set replaced it with is in the download line, so what both read never stops; what landed stays.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prepare {
    pub items: Vec<Predicted>,
}

/// A request the caller predicts it may send here (cl-1123; Paul, 11-17: "calcifer only has to worry about what it knows, i.e., it
/// predicts what the user MAY want in the future, and the remote machine only needs to worry about what it's told, what a request MAY
/// look like, to ensure it's ready for it"): the exact spec a run of it would carry, its payload bound and its seed fixed. The machine
/// plans it as it plans that run (its function's needs rules on its payload, its lane for this machine's GPUs, its store) and makes
/// it ready: its code installed, what it needs downloaded. Nothing of components, lanes or downloads is said. Its readiness shows in
/// Status (`ahead`), its downloads in `StatusFrame::activity` with its id among their `runs`: `cozy:upcoming:<key>`, or
/// `cozy:prepare:<key>`. A run later sent naming the key (`RunSpec::predicted`) with the same spec adopts it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Predicted {
    /// The caller's name for it: an upcoming job's key, the one its run is sent under.
    pub key: String,
    pub spec: RunSpec,
}

// ---- Estimate -------------------------------------------------------------------------------

/// A job the estimator quotes a machine for (`api::estimate::quote`, cl-967): Calcifer's, from what the machine pushes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EstimateJob {
    /// The caller's name for it, answered back.
    pub key: String,
    /// `org/package/function@release` (no release: any held here), or `sha256:<hex>/function`:
    /// local code by its manifest's digest.
    pub function: String,
    /// The checkpoints it reads.
    pub models: Vec<ModelRef>,
    pub size: JobSize,
    /// The user's GPU count; 0: the machine chooses.
    pub gpus: u32,
    /// It builds a model on a GPU, itself or a call it makes: a machine with none can't run it.
    pub gpu: bool,
    /// The caller's time for it on one GPU of this kind, from runs elsewhere; 0: none. Used when
    /// this machine has measured none of the function.
    pub prior_ms: u64,
    /// How many of this machine's own runs `prior_ms` is, which its frame doesn't time (a project's private runs, cl-1047): its quote
    /// is then Measured. 0: runs elsewhere.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub prior_runs: u32,
    /// The soonest it can start, ms from now: when what it reads is made.
    #[serde(skip_serializing_if = "is_zero")]
    pub after_ms: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// A checkpoint a job reads, as the catalog resolved it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelRef {
    /// `sha256:<hex>`: a checkpoint tree or header.
    #[serde(rename = "commit")]
    pub checkpoint: String,
    /// Its weights' bytes as the catalog lists them: what a machine that holds none of it fetches.
    pub bytes: u64,
    /// Its repository (`org/name`): a queued run fetching it (`Queued::models`) brings it for this job too.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// What a run of the job's function read of it on another machine (`Read::bytes`): what one that holds none of it fetches. Zero:
    /// unknown.
    #[serde(skip_serializing_if = "is_zero")]
    pub reads: u64,
    /// The components its slot reads (cl-1104): of what a machine's runs of the function read of the checkpoint, those this job
    /// fetches. None: all it reads of it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
}

/// What a job makes, in the units a function's time scales with; each unknown one is absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobSize {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub megapixels: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// Its media's seconds: every `duration` in its input, summed (a shot's beats).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// The pictures and videos it reads, where its caller counted them (a box's mentions and slots): no unit of its time, but which
    /// measured request it is most like (`estimate::References`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refs: Option<u32>,
}

impl JobSize {
    /// A run's size as its input says: `megapixels`, else `width` by `height`; `frames`, `steps`,
    /// `tokens`, and every `duration` summed. The one reading, for callers and the machine's history alike.
    pub fn of(input: &serde_json::Value) -> Self {
        fn durations(value: &serde_json::Value) -> f64 {
            match value {
                serde_json::Value::Object(map) => map
                    .iter()
                    .map(|(key, value)| match (key.as_str(), value.as_f64()) {
                        ("duration", Some(seconds)) => seconds.max(0.0),
                        _ => durations(value),
                    })
                    .sum(),
                serde_json::Value::Array(items) => items.iter().map(durations).sum(),
                _ => 0.0,
            }
        }
        let number = |key: &str| input.get(key).and_then(serde_json::Value::as_f64).filter(|n| *n > 0.0);
        let count = |key: &str| input.get(key).and_then(serde_json::Value::as_u64).filter(|n| *n > 0);
        let seconds = durations(input);
        JobSize {
            megapixels: number("megapixels").or_else(|| Some(number("width")? * number("height")? / 1e6)),
            frames: count("frames"),
            steps: count("steps"),
            tokens: count("tokens"),
            seconds: (seconds > 0.0).then_some(seconds),
            refs: None,
        }
    }

    /// How much longer `self` takes than `other`: the product of each unit both know.
    pub fn ratio(&self, other: &JobSize) -> f64 {
        let each = [
            self.megapixels.zip(other.megapixels),
            self.frames.zip(other.frames).map(|(a, b)| (a as f64, b as f64)),
            self.steps.zip(other.steps).map(|(a, b)| (a as f64, b as f64)),
            self.tokens.zip(other.tokens).map(|(a, b)| (a as f64, b as f64)),
            self.seconds.zip(other.seconds),
        ];
        each.into_iter().flatten().filter(|(_, b)| *b > 0.0).map(|(a, b)| a / b).product()
    }
}

/// When one job would run here. Times are milliseconds from the answer, so machines' clocks never
/// enter a comparison of them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quote {
    pub key: String,
    /// It can run here at all; one that can't has no times.
    pub fits: bool,
    /// Why it can't: `gpu_lost`, `machine_disk_full` (its weights, the disk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Reason>,
    /// With `machine_disk_full`, the bytes its disk lacks for them, after all the store could free (cl-1175).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub short_bytes: u64,
    /// The GPUs it would run on; 0: none.
    pub width: u32,
    pub start_ms: u64,
    pub finish_ms: u64,
    /// Its code's install and, its weights not here, their download and load onto the GPUs: part of `start_ms`.
    pub prepare_ms: u64,
    /// How many runs and jobs start here before it.
    pub queue_position: u32,
    pub basis: Basis,
    /// Its time is the function's warm one there: the model held, its plan compiled.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub warm: bool,
    /// Its weights' bytes it downloads there first (cl-967).
    #[serde(skip_serializing_if = "is_zero")]
    pub fetch_bytes: u64,
}

/// What a quote's run time stands on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Runs of the function on this machine.
    Measured,
    /// Runs of it elsewhere on this card: the caller's `prior_ms`, else another of the caller's machines' (cl-1107).
    Prior,
    /// Its release's benchmark of this card (`estimate::References`, cl-1024), scaled to its size.
    Reference,
    /// Estimated: another of the caller's machines' runs of it on another card (cl-1107), else its release's benchmark of another
    /// card, scaled to this one by how the two compare across the functions both were measured on, else by the Hub's spec speeds
    /// (`References::time`; Paul 11-15, "a rough guess"). Never counted as measured.
    Scaled,
    /// Nothing: a minute is assumed.
    #[default]
    None,
}

// ---- Status ---------------------------------------------------------------------------------

/// The machine's picture. Without a machine-scope cap, `GET /v1/status` answers identity,
/// version and the readiness `receipt` only. With one it answers all of it,
/// and as SSE (`Accept: text/event-stream`) sends the whole frame again whenever it changes. An
/// open stream is not activity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusFrame {
    pub worker_id: String,
    pub boot_id: String,
    /// This machine's release.
    pub version: String,
    /// The commit its `cozy` binary was built from.
    pub commit: String,
    /// Its binary arrived by a dev update (`cozy machine update --tensord <path>`), not a release.
    pub dev: bool,
    /// The RFC 7638 thumbprint of this TensorD's workload key, made at its start and held in memory
    /// only: what its runs' capabilities bind (cl-675).
    pub workload_key: String,
    /// This boot's readiness facts (JSON, base64), which a Hub binds to the leaf it pinned at the
    /// provider's address; empty until measured.
    #[serde(with = "base64_bytes")]
    pub receipt: Vec<u8>,
    /// An idle rental's release, 15 min after its last job; 0: none (a job is queued or running,
    /// or it never releases).
    pub idle_deadline_unix_ms: i64,
    /// The caller's live runs.
    pub runs: Vec<RunState>,
    /// An update in progress here; none without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<RunState>,
    /// booting, ready, releasing
    pub phase: String,
    /// os/arch
    pub platform: String,
    /// Its distribution and kernel major.minor (`Ubuntu 24.04 (6.8)`); empty: unread.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub os_release: String,
    /// Its host's memory; 0: unread.
    #[serde(skip_serializing_if = "is_zero")]
    pub memory_bytes: u64,
    pub started_at_unix_ms: i64,
    /// Measured at boot.
    pub gpus: Vec<Gpu>,
    pub hubs: Vec<Hub>,
    /// The executors' cozy-runtime.
    pub runtime: String,
    pub tensorfs: String,
    /// The PyTorch its packages use (`2.15.0+cu132`): its image's, else its own base's; empty while a machine with no
    /// image holds none.
    pub torch: String,
    /// The caller's held package installations.
    pub environments: Vec<Environment>,
    /// The filesystem holding the machine's store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<Disk>,
    /// The models the machine's store holds.
    pub models: Vec<Model>,
    /// Their bytes together, shared content counted once.
    pub models_bytes: u64,
    /// The direct player endpoint, when the machine listens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webrtc: Option<WebRtc>,
    /// The caller's warm set, each with what it holds now.
    pub warm: Vec<WarmItem>,
    /// `shared` or `dedicated`: how it shares its computer (`machine.mode`); empty from an older one.
    #[serde(default)]
    pub mode: String,
    /// The oldest NVIDIA driver this machine's kernels run on; empty: no GPU stack.
    pub driver_floor: String,
    /// Why a GPU of this computer is not among `gpus`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gpu_problems: Vec<String>,
    /// What builds kernels here when no prebuilt one serves the card.
    pub toolchain: Toolchain,
    /// Its batch admission writes the small objects a batch carries (`RunBatch::objects`, cl-1172); false from an older one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub carried_objects: bool,
    /// Each native kernel's verdict on this machine's cards, as the Runtime keeps it.
    pub kernels: Vec<KernelVerdict>,
    /// The TensorD service's process (not its stable parent, a pod's PID 1).
    pub tensord_pid: u32,
    /// While the machine starts: the step it is on, answered before anything else, so a
    /// watcher tells a slow start from none. Absent once it serves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub booting: Option<Booting>,
    /// Killed processes stuck in the kernel: nothing waits on them, and what each holds stays
    /// held until it exits.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stuck: Vec<StuckProcess>,
    /// Why nothing can run here: it takes no work, and a rental releases itself once nothing works there (cl-977).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broken: Option<Broken>,
    /// What it does for the caller's runs besides their own work, each once however many wait on it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<Operation>,
    /// Its GPUs the driver held back for heat most of their last loaded run (cl-913): the machine is hot, and its runs slow.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub heat: Vec<GpuHeat>,
    /// Why the executors' Runtime serves its GPUs none of its native kernels (`this cozy-runtime wheel carries no kernels
    /// (the pure wheel)`), as its latest executor said; empty: it does, or none has started.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub runtime_kernels: String,
    /// Every root here not ended, every caller's, in acceptance order: what a job sent now waits behind (cl-967). On a stream,
    /// absent: as its last frame said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<Queued>>,
    /// Each function's newest runs here, newest first: what its time here stands on (cl-967). On a stream, absent: as its
    /// last frame said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub times: Option<Vec<Timed>>,
    /// Idle executors' loaded models: a job of them loads nothing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resident: Vec<Resident>,
    /// Weights' arrival, bytes a second, once 1 GiB came; 0 before.
    pub link_bytes_per_s: f64,
    /// The most GPUs side by side in service: the widest a root runs at.
    pub serving_gpus: u32,
    /// Its GPUs talk over NVLink.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub nvlink: bool,
    /// The caller's lists' items (`PUT /v1/upcoming`, `/v1/prepare`), each as its run stands: ready once it succeeded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ahead: Vec<RunState>,
    /// The newest lines of its `machine.log` (at most [`MACHINE_LOG_LINES`] and [`MACHINE_LOG_BYTES`]): its boot, software,
    /// service, GPUs, disk, Hub lease and end, never a run or a project's content. Calcifer keeps them after the machine is
    /// gone (cl-1012). On a stream, absent: as its last frame said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_log: Option<String>,
}

/// What `cozy machine logs` prints by default: what Calcifer keeps of a machine's own lines (`StatusFrame::machine_log`).
pub const MACHINE_LOG_LINES: u64 = 200;
/// Its kernel-boot lines are about 2 KB.
pub const MACHINE_LOG_BYTES: usize = 64 << 10;

/// The newest `lines` whole lines of `text` (0: all), within `bytes`.
pub fn log_tail(text: &str, lines: u64, bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 {
        start = text[start..].find('\n').map_or(text.len(), |i| start + i + 1);
    }
    let kept = &text[start..];
    if lines == 0 {
        return kept;
    }
    let body = kept.strip_suffix('\n').unwrap_or(kept);
    match body.rmatch_indices('\n').nth(lines as usize - 1) {
        Some((i, _)) => &kept[i + 1..],
        None => kept,
    }
}

/// A root not ended (`StatusFrame::queue`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queued {
    /// The caller's id for it; empty: another caller's.
    pub run: String,
    /// `<package>/<function>`, as `Timed::function` names it; empty: a private run's.
    pub function: String,
    pub size: JobSize,
    /// The widths it may run at, smallest first; none: no GPU.
    pub widths: Vec<u32>,
    /// It gives the machine its width: no count asked, and more than one width.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub chooses: bool,
    /// It left its queue: its executor starts or it runs.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
    /// The GPUs it holds, running.
    pub width: u32,
    /// When it began executing, this machine's clock; 0 before.
    pub started_at_unix_ms: i64,
    /// When its work took its GPUs, this machine's clock: its own claim of them, or, for a root on no GPU of its own (an
    /// orchestrator), its first call's (cl-1107). 0: not yet, or a machine that doesn't say.
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub gpu_at_unix_ms: i64,
    /// It runs on its CPU alone while its GPU work waits to begin: an orchestrator whose calls wait for their weights or a GPU
    /// (cl-1107). All its work is still ahead of it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gpu_waits: bool,
    /// The repositories it waits to download, its calls' included.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Its weights' bytes still to come, its calls' included.
    pub bytes_left: u64,
}

fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

/// A function's newest runs here (`StatusFrame::times`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timed {
    /// `<package>/<function>`, any release.
    pub function: String,
    pub runs: Vec<TimedRun>,
    /// What its newest run here read of each model (cl-967): what a job of it fetches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<Read>,
}

/// Of one model, the components a function's runs read: their bytes, and of them those this machine lacks now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Read {
    /// Its repository (`org/name`).
    pub model: String,
    /// `sha256:<hex>` of the header read.
    pub checkpoint: String,
    pub bytes: u64,
    pub missing: u64,
    /// Each component, as runs of the function read it (cl-1104): two boxes of one function may read different ones of a checkpoint,
    /// and a quote of each counts its own (`ModelRef::components`). None: its header isn't here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ReadComponent>,
}

/// One component of a checkpoint a function reads: its bytes, and of them those this machine lacks now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadComponent {
    pub name: String,
    pub bytes: u64,
    pub missing: u64,
}

impl Read {
    /// Of what it names, the bytes of `components` and of them those missing; None when it doesn't name each of them. None asked: all.
    pub fn of(&self, components: &[String]) -> Option<(u64, u64)> {
        if components.is_empty() || self.components.is_empty() {
            return Some((self.bytes, self.missing));
        }
        let parts: Vec<&ReadComponent> = self.components.iter().filter(|part| components.contains(&part.name)).collect();
        (parts.len() == components.len()).then(|| (parts.iter().map(|part| part.bytes).sum(), parts.iter().map(|part| part.missing).sum()))
    }
}

/// What one completed root took: its width, its size as its input said, its compute, and what it spent putting weights on its
/// GPUs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimedRun {
    pub width: u32,
    pub size: JobSize,
    pub ms: f64,
    pub load_ms: f64,
    /// The function's first call on the machine: its plan compiled as it ran (its cold time).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cold: bool,
}

/// An idle executor's loaded models (`StatusFrame::resident`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Resident {
    /// Their headers, `sha256:<hex>`.
    pub checkpoints: Vec<String>,
    pub gpus: Vec<u32>,
    /// `host` or `gpu`.
    pub level: String,
}

/// A GPU the driver held back for most of a run that loaded it, for heat (thermal or hardware slowdown, cl-913) or at a power
/// limit its host set under the card's default (cl-1161): what that run's readings measured, until a later loaded run there runs
/// at full speed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct GpuHeat {
    pub gpu: u32,
    /// The share of its busy readings under that limit, 0 to 1.
    pub share: f32,
    /// Its busy time those readings cover.
    pub busy_ms: u64,
    pub sm_mhz_mean: u32,
    pub sm_mhz_limit: u32,
    pub temperature_max_c: u32,
    /// The driver's limits it saw (`hw_thermal`, `sw_thermal`, `hw_slowdown`, `power_cap`, ...).
    pub throttles: Vec<String>,
    /// The run it was measured in, by this machine's id.
    pub run: String,
    pub at_unix_ms: i64,
    /// Its readings were recorded ones a test replays (`machine.gpu_samples`), not this GPU's own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
    /// Held at a power limit, not heat: the limit the driver enforced and the card's default, in W; 0: heat.
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub power_limit_w: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub power_default_w: u32,
}

/// Weights arriving or a package's environment built for runs: one per repository or package, whichever runs wait on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Operation {
    /// The same for as long as it lasts: its kind and subject.
    pub id: String,
    /// `download` or `install`.
    pub kind: String,
    /// The model's repository (or provider source), or the package.
    pub subject: String,
    /// Where it is, in words; empty when only its bytes say.
    pub stage: String,
    /// Bytes fetched of those it has to fetch (0: unknown), never what was here already.
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// What of it was here already (shared with another model, or pulled before).
    pub bytes_here: u64,
    /// Its bytes a second over its last 10 s, once a second of them is seen.
    pub bytes_per_s: f64,
    pub started_at_unix_ms: i64,
    /// The runs waiting on it, by the caller's ids.
    pub runs: Vec<String>,
}

/// Why nothing can run on a machine (`StatusFrame::broken`, cl-977). `code` is the state: `stuck_in_driver`, the TensorD
/// before this one can't exit (it is in `stuck`) and a fresh process can't open its GPUs; Calcifer's own, for a rental that
/// hasn't answered since an update restarted it, `silent_after_update`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Broken {
    pub code: String,
    pub message: String,
}

/// `StuckProcess::command` of the TensorD before a machine's own that can't exit (cl-977): a word of TensorD's, not a process's, so
/// it is shown where commands aren't.
pub const STUCK_TENSORD: &str = "the TensorD before this one";

/// A killed process in uninterruptible sleep (`D`: a driver after an Xid, a hung network
/// filesystem): it exits once that wait ends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct StuckProcess {
    pub pid: u32,
    pub command: String,
    /// The kernel function it sleeps in; empty when the kernel hides it.
    pub wchan: String,
    /// When it was killed.
    pub since_unix_ms: i64,
    /// Its resident host memory then.
    pub resident_bytes: u64,
    /// The GPUs (by index) out of service while it holds their memory, nothing placed on them until it exits (cl-935).
    pub gpus: Vec<u32>,
    /// What the driver showed it holding on them.
    pub gpu_bytes: u64,
}

/// A starting machine's step and a counter that advances as it moves.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Booting {
    pub step: String,
    pub progress: u64,
    /// How long it has been on `step`: a start whose step stops moving is told from a slow one (cl-1066). None: an older machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_step_ms: Option<u64>,
}

/// The machine's build toolchain: CUDA's compiler and a host C/C++ compiler.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Toolchain {
    /// ready, absent, installing, failed
    pub state: String,
    /// `nvcc --version`'s release line; empty: none found.
    pub nvcc: String,
    /// The host compiler's first version line; empty: none found.
    pub host_compiler: String,
    /// What it lacks: `nvcc`, `cc`, `ninja`.
    pub missing: Vec<String>,
    /// The OS's own command for a missing host compiler; empty when none is missing or known.
    pub install: String,
    /// Why the last install failed.
    pub detail: String,
}

/// One kernel's verification on this machine (the Runtime's `verify_op`), keyed by its
/// artifact, the card's architecture and the driver.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct KernelVerdict {
    pub kernel: String,
    pub artifact: String,
    pub sm: u32,
    pub driver: String,
    pub ok: bool,
    /// What was measured, or why it failed.
    pub detail: String,
}

/// The direct player endpoint: its port, the addresses to dial and its DTLS fingerprint.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebRtc {
    /// The listener; a persistent machine keeps it across restarts.
    pub port: u32,
    /// host:port to dial: own interfaces, or the provider's mapping.
    pub addresses: Vec<String>,
    /// SHA-256 hex of the certificate its DTLS presents.
    pub fingerprint: String,
    /// Why no address is reported (a malformed mapping).
    pub unavailable_reason: String,
}

/// A repository the store holds: its bytes and checkpoints.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    /// org/name; org "local" is a local alias.
    // The supported machine protocol still names this field repository.
    #[serde(rename = "repository")]
    pub model: String,
    /// Everything this repository reaches.
    pub total_bytes: u64,
    /// Reached by no other held repository.
    pub unique_bytes: u64,
    pub checkpoints: Vec<Checkpoint>,
}

/// One of a held model's checkpoints: its release, lane, precision and header.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checkpoint {
    /// Empty for a local alias or a commit by hash.
    pub release: String,
    /// Its commit's lane label.
    pub lane: String,
    /// Its header's precision family (`int8`, `bf16`, …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub precision: String,
    /// `sha256:<hex>` of its header.
    pub manifest: String,
    /// `model`, `lora`, or `unknown`; empty from older machines.
    pub role: String,
    /// Whether every object in the header's closure is cached. None from older machines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
    /// `sha256:<hex>` of its tree, as a Hub card names the checkpoint.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tree: String,
    /// Its closure's bytes, and of them those held.
    pub bytes: u64,
    pub held_bytes: u64,
}

/// One GPU, as measured at boot: identity, memory, driver and family.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gpu {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub pci_bus_id: String,
    pub memory_bytes: u64,
    pub driver: String,
    /// The tensor-core row by compute capability ("sm90"); empty: unread.
    pub family: String,
    /// discrete, coherent (GH200, GB200), unified (GB10); empty: unread.
    pub memory_class: String,
    /// The most of `memory_bytes` this machine uses (`machine.gpu_memory_limit`); 0: all of it.
    pub memory_limit_bytes: u64,
    /// It drives a display now: a run on it slows the screen (cl-796, cl-897).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub display: bool,
    /// TensorD's page accounting of its memory manager on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<GpuMemory>,
    /// Why nothing is placed on it now (cl-935): its memory held by a process stuck in the kernel, or in use at the
    /// machine's start by processes it doesn't run. Empty: in service.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub out_of_service: String,
}

/// What a machine's NVIDIA GPUs are doing now (`path::GPUS`, cl-1100): one reading each; none from a machine without one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct GpuUse {
    pub at_unix_ms: i64,
    pub gpus: Vec<GpuReading>,
}

/// One GPU's reading: the share of its last sample period a kernel ran (`nvidia-smi`'s GPU-Util), and its memory in use, every
/// process's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct GpuReading {
    pub index: u32,
    pub busy_percent: u32,
    pub memory_used_bytes: u64,
}

/// A GPU's page accounting, as its memory manager holds it now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuMemory {
    /// What it may allocate: the card or its limit, less its own context and the reserved pool.
    pub capacity_bytes: u64,
    /// `capacity_bytes` less what it allocated.
    pub free_bytes: u64,
    /// Bytes no cozy process holds (another app's), as its last reading measured them.
    pub foreign_bytes: u64,
    /// Allocated to live processes.
    pub allocated_bytes: u64,
    pub processes: Vec<ProcessMemory>,
}

/// One process's allocated memory on a GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessMemory {
    pub pid: u32,
    /// Its executor group (plan and lane).
    pub executor: String,
    pub bytes: u64,
}

/// A Hub this machine is registered with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hub {
    pub origin: String,
    /// This machine's worker id there.
    pub machine_id: String,
}

/// A held package installation: package, release, functions, Runtime and level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Environment {
    /// [`Source::Installation`] names it.
    pub installation: String,
    pub package: String,
    pub release: String,
    pub entrypoints: Vec<String>,
    /// The cozy-runtime installed in it.
    pub runtime: String,
    /// What it holds now (as [`WarmItem::holds`]), in a warm set or not.
    pub level: String,
    /// What its runs are told of the Runtime pair it holds (one of the machine's it did not take,
    /// and why); empty for nothing.
    pub warning: String,
    /// The Hub origin a published installation came from; empty for local code.
    pub hub: String,
    /// Local code's manifest (`sha256:<hex>`) it was installed from; empty for a release.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub manifest: String,
    /// Each of its functions as the machine times and packs it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<HeldFunction>,
}

/// One function of an installation (`Environment::functions`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldFunction {
    pub entrypoint: String,
    /// Its runs' key in `StatusFrame::times`.
    pub timed_as: String,
    /// Its widths with no GPU count asked, smallest first; none: it holds no GPU.
    pub widths: Vec<u32>,
}

/// The filesystem holding the store: its size, free bytes and what fills it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// What fills it, as the machine's last sweep measured (0 before one, or from an older
    /// machine): its TensorFS store; its package environments with what they added to uv's
    /// cache; its Runtime pair, kernels and toolchain; its runs' files, inputs and outputs. The rest is other.
    pub models_bytes: u64,
    pub environments_bytes: u64,
    pub runtime_bytes: u64,
    pub runs_bytes: u64,
    /// What TensorFS keeps free and no download writes into (cl-1175); 0 from an older machine.
    pub reserve_bytes: u64,
    /// What its store could free for a download: models no process holds and nothing queued or running uses, never one an
    /// executor has open (cl-1175); 0 from an older machine.
    pub reclaimable_bytes: u64,
}

impl Disk {
    /// The bytes a download may take here (cl-1175): free past the reserve, with what the store could free.
    pub fn room(&self) -> u64 {
        (self.free_bytes + self.reclaimable_bytes).saturating_sub(self.reserve_bytes)
    }

    /// The room for downloads its usual models need, when it has less (cl-1175, cl-1100): a model the size of its `biggest`, and a tenth
    /// of the disk. None: it has room, or doesn't say.
    pub fn short_of(&self, biggest: u64) -> Option<u64> {
        let needs = biggest.max(self.total_bytes / 10);
        (self.total_bytes > 0 && self.room() < needs).then_some(needs)
    }
}
