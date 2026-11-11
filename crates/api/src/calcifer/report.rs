//! What a run's machine measured, as Calcifer reads it for `GET /v1/runs/{id}?detail=full`
//! (`Run.report`): where each executor's time went, the models it ran, its kernels, and each GPU's
//! memory. The numbers are worked out once here; a client only formats them.
use serde::{Deserialize, Serialize};

/// What a run's machine measured: the attention kernels it pinned and each executor's measurements.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Report {
    /// The attention kernels the run pinned, as its executors recorded them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention_pins: Vec<String>,
    /// Each executor's measurements: the run's own, then each call's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measured: Vec<Measured>,
}

/// One executor's measurements.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Measured {
    /// `run`, or the call's index.
    pub by: String,
    /// Where its span's time went, in the order it ran; last, what no measurement covers.
    pub stages: Vec<Stage>,
    pub models: Vec<Model>,
    pub components: Vec<Component>,
    /// Every GPU's attention kernels (the one that served, the others and why not), then where each package
    /// Triton set it loaded came from (its `state`: shipped, compiled or store).
    pub kernels: Vec<Kernel>,
    /// Each GPU's whole request.
    pub memory: Vec<Memory>,
    /// Each GPU's plan steps.
    pub steps: Vec<Step>,
    /// Each make_room that had to take bytes.
    pub make_room: Vec<MakeRoom>,
}

/// Where part of an executor's span went: a stage's time, its longest legs and its steps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ReportStage"))]
#[serde(default)]
pub struct Stage {
    /// The stage, `executor start`, `construction`, `loading <slot>`, or `unaccounted`.
    pub name: String,
    pub ms: f64,
    /// An executor's or a construction's three longest legs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legs: Vec<Leg>,
    /// Its steps: how many, and each one's mean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Steps>,
    /// The span's time no measurement covers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unaccounted: bool,
}

/// One named leg of a stage, and its time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Leg {
    pub name: String,
    pub ms: f64,
}

/// How many steps a stage took, and each one's mean time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Steps {
    pub count: u64,
    pub each_ms: f64,
}

/// A model slot's checkpoint as its machine resolved it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ReportModel"))]
#[serde(default)]
pub struct Model {
    pub slot: String,
    pub repository: String,
    pub release: String,
    /// Its hash, for a checkpoint passed by value.
    pub manifest: String,
    pub lane: String,
    /// The header's precision family (`int8`, …).
    pub precision: String,
    pub adapters: Vec<Adapter>,
}

/// A LoRA on a model: its strength and the components it applies to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Adapter {
    pub model: String,
    pub strength: f64,
    pub components: Vec<String>,
}

/// A component's attention and GEMM kernels (a 16-bit lane's has no GEMM).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ReportComponent"))]
#[serde(default)]
pub struct Component {
    pub component: String,
    pub attention: Vec<String>,
    pub gemm: Vec<Gemm>,
}

/// A GEMM kernel; one that served below its lane says what it served, what it wanted and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Gemm {
    pub kernel: String,
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Fallback>,
}

/// A GEMM kernel that served below its lane: what served, what was wanted, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Fallback {
    /// What served instead, when it was not the kernel itself.
    pub served: String,
    pub wanted: String,
    pub reason: String,
}

/// One kernel on a GPU: an attention kernel's state, whether it served and its compile, or a package Triton
/// set's source (`shipped`, `compiled` with its compile, `store`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ReportKernel"))]
#[serde(default)]
pub struct Kernel {
    /// None: the executor did not say.
    pub gpu: Option<u32>,
    pub arch: String,
    pub kernel: String,
    pub state: String,
    pub served: bool,
    /// How far its compile is.
    pub progress: Option<f64>,
    pub compile_ms: f64,
    pub detail: String,
}

/// One GPU's memory over the request: bytes moved by source, stall, peaks, what make_room took and what was left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ReportMemory"))]
#[serde(default)]
pub struct Memory {
    pub gpu: u32,
    /// What its allocator figures cover: `request`, or the process.
    pub scope: String,
    /// Bytes kept from the manager's retention, leased and filled, read from the store, and
    /// refilled from a derived entry.
    pub kept: Moved,
    pub leased: Moved,
    pub store: Moved,
    pub derived: Moved,
    pub stall_ms: f64,
    /// The most it held: pages and torch's bytes.
    pub peak: Option<u64>,
    pub torch_peak: Option<u64>,
    pub compute: Option<u64>,
    /// What make_room took, by class.
    pub evicted: Vec<Taken>,
    /// What it still held at the end.
    pub leftover: Option<u64>,
}

/// Bytes moved and, when they took time, their rate and its share of the measured ceiling.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Moved {
    pub bytes: u64,
    pub gbps: Option<f64>,
    pub ceiling_share: Option<f64>,
}

/// Bytes make_room took of one class (F, O, S, N, D, W, P).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Taken {
    pub class: String,
    pub bytes: u64,
}

/// One plan step of a GPU: the free bytes at its start, the bytes its binds found mapped (hit),
/// adopted (kept), read (store) and refilled (derived), how many were prefetched, and what
/// make_room took in it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct Step {
    pub gpu: u32,
    pub step: u32,
    pub capacity: Option<u64>,
    pub hit: u64,
    pub kept: u64,
    pub store: u64,
    pub derived: u64,
    pub prefetched: u64,
    pub evicted: Option<Vec<Taken>>,
}

/// One make_room that took bytes: its step, what asked, what it needed and freed, and its waits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct MakeRoom {
    pub gpu: u32,
    pub step: u32,
    /// What asked.
    pub trigger: String,
    pub need: u64,
    pub freed: u64,
    /// The release events it waited on.
    pub waited: u32,
}
