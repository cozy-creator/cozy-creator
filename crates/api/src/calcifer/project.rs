//! Calcifer's project API (xs-151, cl-758): the projects this computer knows, a project folder
//! opened by its path, its assets as they stand and their versions (a video's are takes), the runs
//! that make them, what happens as they go, and its files and cuts. The CLI's asset verbs
//! are one client of it.
use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{calcifer::On, error::Error};

/// `POST /v1/projects`: a new project called `name`, in Calcifer's home folder (`~/<name>`, the next free name when that is
/// taken) or at `path` (absolute, on Calcifer's computer), with the starter film when `starter`. A project is its id, kept in
/// its `project.db`: the folder is only where it lives. An existing one is named in a route (`NAMED_BY`) and needs no call here.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Open {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub starter: bool,
    /// Explicitly create ordinary local storage instead of the default encrypted project.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unencrypted: bool,
    /// New encrypted projects require a user-supplied passphrase. Never cache this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

/// The project `POST /v1/projects` made: its id, folder and name.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Opened {
    pub id: String,
    /// Its stable, nine-character public URL code.
    #[serde(default)]
    pub code: String,
    /// Where it lives on Calcifer's computer.
    pub path: PathBuf,
    /// Its name, unique among the projects Calcifer knows: what `-p` takes.
    pub name: String,
    /// Commands to run next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// What opening it did to it, said once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub said: Vec<String>,
    /// Disclosed once for a new encrypted project; never persist this response in a request cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_secret: Option<String>,
}

/// How a client names a project: its id, public code or name, or its folder on Calcifer's computer by its absolute path (cl-819). The
/// CLI project commands resolve their current directory and send its absolute path.
pub const NAMED_BY: &str = "a project's name, id or public code, or its folder on Calcifer's computer by its absolute path";

/// A request in no project, which needs one.
pub fn unnamed() -> Error {
    Error::new(crate::error::Kind::Usage, "not_in_a_project", "run this command from an existing project directory. Use cozy new to create one")
        .next("cozy new")
}

/// Where a card sits on a project's canvas: one row of `project.canvas.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Card {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
}

/// `POST /v1/projects/{id}/runs`: what the ask found. `GET /v1/events` tells its board's changes from here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Asked {
    pub id: String,
    /// This render, which the `ended` that names it ends. None when it joined one going and added nothing: that one's end is its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
    pub path: PathBuf,
    /// A render was going already: this ask joined it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub joined: bool,
    /// What it added to the walk; 0 with `joined` is no new work.
    #[serde(default)]
    pub added: u32,
    /// Pinned assets it was asked of by no name of their own: left as they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// What it added, each with what is ahead of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued: Vec<QueuedNow>,
    /// What the ask found of what it names, each line once: what it skips and why, a flag it ignores.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub said: Vec<Said>,
    /// Commands to run next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// The render as this ask left it: `board.updated` and `job.updated` on `GET /v1/events` say each change from here.
    pub board: Board,
}

/// One thing an ask queued, and the names ahead of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct QueuedNow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ahead: Vec<String>,
}

/// One thing a project has to do: running, with its run's number once it has one, else queued.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Queued {
    pub name: String,
    #[serde(default)]
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
}

/// `POST /v1/projects/{id}/cancel`: these assets' queued work dropped and their runs canceled, or `all`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cancel {
    pub names: Vec<String>,
}

/// What a cancel did: the names dropped from the queue, and the runs asked to stop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cancelled {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canceled: Vec<u64>,
}

/// `GET /v1/projects`: the projects this computer's Calcifer has opened, last used first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Projects {
    pub projects: Vec<Known>,
    /// How many more are older than this page.
    #[serde(default)]
    pub older: u64,
}

/// One project in the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "KnownProject"))]
pub struct Known {
    pub id: String,
    /// Its stable, nine-character public URL code.
    #[serde(default)]
    pub code: String,
    /// Its name: what `-p` takes.
    pub name: String,
    pub path: PathBuf,
    pub used_at_ms: i64,
    /// Its runs are being made, or asked and not yet sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub busy: bool,
}

/// `GET /v1/projects/{id}/address`: project identity without evaluating its assets; accepts a containing directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ProjectAddress"))]
pub struct Address {
    pub id: String,
    pub code: String,
}

/// `GET /v1/projects/{id}`: its assets in order, and what reading the project said.
/// Optional `covered` describes only a successful full snapshot: project-event emissions
/// before its read began. Head-only and partial/error-derived states do not claim coverage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ProjectState"))]
pub struct State {
    pub id: String,
    /// Its stable, nine-character public URL code.
    #[serde(default)]
    pub code: String,
    pub path: PathBuf,
    pub assets: Vec<Asset>,
    /// What is running and what is queued, in the order it goes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<Queued>,
    /// Assets with no state and no version yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Assets out of date, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<Stale>,
    /// Assets project.lock keeps versions of that project.yaml no longer names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<Orphan>,
    /// Whether its walk is going: runs being made, or asked and not yet sent.
    pub busy: bool,
    /// How many files project.lock names are not on this computer.
    #[serde(default)]
    pub absent: u64,
    /// How many files `cozy run`s left here, that belong to no asset.
    #[serde(default)]
    pub run_files: u64,
    /// What its `cozy/cut` assets made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<Cut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    /// Commands to run next, given what it holds and lacks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// Its render going, or its last since Calcifer started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<Board>,
    /// What kept its last render from reaching a machine, each once, until another is asked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsent: Vec<Error>,
    /// The machines its runs go to, each to the one of them that finishes it soonest; none: any machine (`ProjectMachines`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered: Option<super::ProjectEventCursor>,
}

/// An asset out of date, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Stale {
    pub name: String,
    pub why: String,
}

/// What reading a project said: of the asset or entity `name`, or of the file as a whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ProjectWarning"))]
pub struct Warning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
}

impl Warning {
    /// A warning about the asset or entity `name`.
    pub fn of(name: &str, message: String) -> Self {
        Self { name: Some(name.to_owned()), message }
    }

    /// A warning about the project file as a whole.
    pub fn of_file(message: String) -> Self {
        Self { name: None, message }
    }
}

/// A `<mention>` an asset's text makes that names something: an asset, an entity, a sub-entity, or a link of one, as the file names it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Read {
    pub name: String,
    /// The one version of it the text fixes: `<name.2>`, `<self.2>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Where its text says so: `summary`, `beats.0.description`, `tracks.0.clips.1`, `style` for what defaults gave it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<String>,
    /// What it sends now, as twelve hex digits: its version's files and its box's description, or a text box's words. It moves as what's read
    /// does, and only then. None while it has no state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// An asset project.lock keeps versions of that project.yaml no longer names: the bytes `reclaim`
/// would free of it (none that anything else names, and each file once across the orphans), and,
/// when project.yaml has exactly one new asset of its kind with none, that asset: renamed, it would
/// be this (`POST /v1/projects/{id}/rename`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Orphan {
    pub name: String,
    pub kind: String,
    pub versions: u32,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_to: Option<String>,
}

/// A file the project uses: its path in the project (`files/<sha256>.<ext>`, `GET
/// /v1/projects/{id}/files/{sha256}.{ext}`) and its sha256.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ProjectFile"))]
pub struct File {
    pub path: String,
    pub sha256: String,
    /// Its bytes aren't on this computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// The machine that keeps them, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// `POST /v1/projects/{id}/imports`, under an `Idempotency-Key`: the file at `source` kept in the project, as one constant more of
/// project.yaml, with `at` a card on its canvas. `source` is a held file (`sha256:<hex>`, moved rather than copied), a file of the project's
/// by its hash or the start of it, or an `http(s)` URL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Import {
    pub source: String,
    /// What to call it, through `as_name` with `.` as `_`; a taken name gets `_2`. Absent: the stem of a path or URL, lower-cased, else `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Card>,
    /// The words for whatever mentions it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A person typed `name`: refused if the project has it, not numbered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
    /// What its drop pushed aside, written with it, each only if no other client wrote it after the revision in `placed`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub moves: BTreeMap<String, Card>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placed: BTreeMap<String, u64>,
}

/// What an import made: the element's name, its file's hash, the field that names one of its type (`image`, `video`, `audio`), and its path in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Imported {
    pub name: String,
    pub sha256: String,
    pub field: String,
    pub path: String,
    /// Its bytes were in `files/` already, so nothing was copied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
    /// Each of its `moves` another wrote since, left as it is: its row now (null: none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kept: BTreeMap<String, Option<Card>>,
}

/// `GET /v1/projects/{id}/canvas`: each element's row, by name. A name with no row isn't listed, and the page places it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Canvas {
    /// The project's revision these rows are of.
    pub seq: u64,
    pub cards: BTreeMap<String, Card>,
}

/// `PATCH /v1/projects/{id}/canvas`: these rows set, a null one gone. Last move wins, but for a name in `placed`, the client's own working:
/// its row is set only if no other client wrote it after the revision it was worked out from, so it can't clobber a newer one (cl-800).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CanvasPatch {
    pub cards: BTreeMap<String, Option<Card>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placed: BTreeMap<String, u64>,
}

/// What a canvas patch left: the new revision and the `placed` rows it kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CanvasPatched {
    pub seq: u64,
    /// Each of `placed` another wrote since, left as it is: its row now (null: none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kept: BTreeMap<String, Option<Card>>,
}

/// An Edit's `fields.timeline`: its tracks, each of clips. One for now, V1; a higher one takes priority, later.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    #[serde(default)]
    pub tracks: Vec<Track>,
}

/// One track of an Edit's timeline: its clips in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub clips: Vec<Clip>,
}

/// A clip: an asset, and the part of it. Its place in the list is its place on the track, with no gaps. Points are frames at the Edit's rate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Clip {
    /// `<name>`.
    pub clip: String,
    /// A video's in point, from 0.
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub first: Option<u64>,
    /// A video's out point, exclusive. None: its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<u64>,
    /// A still's duration. None: 3 s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    /// On a version's clip: the file it cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// An Edit's clips as they read now, or as they read when a version of it was rendered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TimelineNow {
    pub fps: f64,
    pub tracks: Vec<TrackNow>,
}

/// One track's clips as they read now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrackNow {
    pub clips: Vec<ClipNow>,
}

/// A clip and what it reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ClipNow {
    #[serde(flatten)]
    pub clip: Clip,
    pub name: String,
    /// `video` or `image`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How many frames a video holds at the Edit's rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    /// Why a render goes on without it: it has no file to read here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
}

/// A version's number and the hash prefix project.yaml can name its file by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VersionPrefix {
    pub number: u32,
    pub prefix: String,
    /// The take it picked up from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<Pickup>,
    /// The machine keeping its latents, when one does: the box warns before a pickup if that machine is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latents: Option<String>,
    /// How long its runs took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_ms: Option<u64>,
    /// The last run that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
    /// Its main file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<MainFile>,
    /// A cut's: what it was cut from, `<asset> <version>` each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<String>,
    /// A cut's: what it was cut without, `<piece>: <why>` each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
    /// An Edit's: the clips that made this version, which an edit can carry on from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<TimelineNow>,
}

/// The file people mean by a version, and what is measured of it so far.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MainFile {
    /// Its output's name in the version, which `GET /v1/runs/{id}/play` takes.
    pub output: String,
    pub path: String,
    pub sha256: String,
    /// The field that names one of its type: `image`, `video`, `audio`.
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// A video's frames per second, for timecode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Part of it is fully transparent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alpha: bool,
    /// Its bytes aren't on this computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// The machine that keeps them, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// Pick up from here: a take, by mention, and the frame its new part starts at. Never written to project.yaml; the take it makes records it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Pickup {
    /// `<shot_1.3>`: one of the box's own takes, or another box's.
    pub take: String,
    pub out_frame: u32,
    /// Seconds into the box's beats where the new part starts. Absent: the tool's choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats_from: Option<f64>,
}

impl Pickup {
    /// `clash: take 3 picks up <clash.2> at frame 243`.
    pub fn said(&self, name: &str, number: u32) -> String {
        format!("{name}: take {number} picks up {} at frame {}", self.take, self.out_frame)
    }
}

/// A `cozy/cut` asset's film at the project's root (`GET /v1/projects/{id}/cuts/{cut}`; its timeline
/// beside it, `<name>.otio`): its file, when it was last cut and how many frames long at `fps`
/// (None: not yet), the takes it retimed to that rate (`<asset> (<fps> fps)`), and what it cuts that
/// has nothing made yet, which it skips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Cut {
    pub cut: String,
    pub path: PathBuf,
    pub fps: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retimed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skips: Vec<String>,
    /// The film that stands was cut without these: `<piece>: <why>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
}

/// How an asset stands, for a client that shows it rather than says it (`status` says it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// It has a state: a pin, or a version with these inputs.
    Current,
    /// It has versions, none with these inputs.
    OutOfDate,
    /// A run is making a version of it now.
    Making,
    /// It has no state and no version to show for it. Also what a reader shows of a standing it doesn't know.
    #[default]
    #[serde(other)]
    Missing,
}

/// An asset: a stable name holding a set of files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub standing: Standing,
    /// The runs making a version of it now (`GET /v1/runs/{id}` says how far).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub making: Vec<u64>,
    /// Its file's type: `image`, `video` or `audio`.
    pub kind: String,
    /// What its alternatives are called: `take` (one that holds a video) or `version`.
    pub noun: String,
    /// The tool that makes it. None for a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// How many it has.
    pub versions: u32,
    /// Each one's hash prefix, to name it in project.yaml.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefixes: Vec<VersionPrefix>,
    /// The version that is its state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// The number (1 first, never reused) of the version `approve` or `--approve` pinned. Its state until `unpin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<u32>,
    /// Its state is a file project.yaml names: nothing is made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Its memo key's inputs now; None while something it reads has no state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
    /// Its state by output name (`video`, `image`, `latents`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, File>,
    /// The file people mean by it, for people who want it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The hash of that file, the field that names one of its type, and what is measured of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Part of that file is fully transparent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alpha: bool,
    /// That file's bytes aren't on this computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// The machine that keeps them, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The function that made its state, `org/package/function@release`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<String>,
    /// For people, in its own noun: current, no take yet, out of date and why, making (run N), …
    pub status: String,
    /// What it reads by a mention, in the order its text says them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<Read>,
    /// Its inputs, flat, each by a control's `field`: what `PATCH /v1/projects/{id}/assets/{name}` sets, as it applies now.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    /// Names what a write of its fields read: `If-Match` on `PATCH`. An edit of another node doesn't move it. None for what has no lines of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// What stopped its last try: until a version lands, it's tried again, or its inputs change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Error>,
    /// A pin's: what changed since the version it holds was made. It stands, out of date, until it is run or moved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// The names its text reads that are nothing in the project, as far as its package's tags are known: it can't run until they are.
    /// A pin lists them too, for its text: it stands, and only fails if it is run again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// An Edit's: its clips as they read now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<TimelineNow>,
    /// An Edit being rendered here: how far, 0 to 1. It runs on no machine, so it has no run to ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendering: Option<f64>,
}

/// `GET /v1/projects/{id}/assets/{name}`: an asset and each of its versions, 1 first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssetVersions {
    pub asset: Asset,
    pub versions: Vec<Version>,
    /// The numbers discarded that `PUT` can still bring back: `reclaim` ends that.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discarded: Vec<u32>,
    /// With `?call=true`: the call its binder would send now, as `cozy run --input` takes it, and what binding it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_warnings: Vec<String>,
}

/// `POST /v1/projects/{id}/assets`: a tool dropped on the canvas. Under an `Idempotency-Key`, a retry makes no `_2`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NewAsset {
    /// A tool's `id`, from `GET /v1/tools`. Empty: a text asset, its words in `fields.text`.
    #[serde(default)]
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Card>,
    /// What to call it, through `as_name` with `.` as `_`; a taken name gets `_2`. Absent: the tool's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    /// A person typed `name`: refused if the project has it, not numbered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
    /// What the tool makes, as its `GET /v1/tools` row said. Absent: Calcifer works it out behind the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// What its drop pushed aside, written with it, each only if no other client wrote it after the revision in `placed`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub moves: BTreeMap<String, Card>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placed: BTreeMap<String, u64>,
}

/// What a write of an asset answers: its name, and what `If-Match` names from here on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssetMade {
    pub name: String,
    pub etag: String,
    /// Each of its `moves` another wrote since, left as it is: its row now (null: none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kept: BTreeMap<String, Option<Card>>,
}

/// `PATCH /v1/projects/{id}/assets/{name}`: these inputs set, each by a control's `field`, and nothing else moved. A null clears one.
/// `tool` changes what makes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssetPatch {
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// What a `PATCH` answers. `was` is the ETag it wrote over: equal to what a window read means nobody else wrote since.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssetPatched {
    pub name: String,
    pub etag: String,
    pub was: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// What `DELETE /v1/projects/{id}/assets/{name}` took, and where its box sat: undo puts it back, its versions with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssetDeleted {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Card>,
}

/// `DELETE /v1/projects/{id}/assets/{name}/versions/{number}` and `POST …/versions`: a version discarded, or joined.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VersionMoved {
    pub name: String,
    pub number: u32,
    /// `take` or `version`.
    pub noun: String,
    /// It was the approved one: `approved:` went with it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approved: bool,
    /// What the asset stands on now, else its newest. None: it has none left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stands: Option<u32>,
    /// What is out of date now and wasn't.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    /// What names this version in its text: `<name.2>`. Gone, a render of them fails until it's back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_by: Vec<String>,
    /// A version held that file already: it's that one, and nothing changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
}

/// `POST /v1/projects/{id}/assets/{name}/versions`: a picture joins it. `source` as an import's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewVersion {
    pub source: String,
}

/// One version (a take, if it holds a video).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Version {
    pub number: u32,
    /// By output name.
    pub files: BTreeMap<String, File>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<String>,
    /// The seed Calcifer drew for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The runs that made it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approved: bool,
    /// What its function said of it, and what was left out of its request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The take it picked up from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<Pickup>,
    /// How long its runs took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_ms: Option<u64>,
    /// The words it was made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `POST /v1/projects/{id}/runs`: `n` new versions of each of `names`, with new seeds, a pinned one's to look at, and of what it reads
/// only if that has no state. With `all`, of everything not pinned. It joins the project's walk, which sends each once what it reads
/// is settled; `on` lists the machines they may go to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Runs {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default = "one")]
    pub n: u32,
    pub on: Vec<On>,
    /// The Hub its functions are read from and its runs belong to. Absent: Calcifer's configured Hub.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hub: String,
    /// The new version is approved when it lands; only with `n` 1.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approve: bool,
    /// Make only what has no state, rather than a new version.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// With `all`: also make a new version of each pin that is out of date when its turn comes in reading order, which goes when that
    /// lands. The version it held stays, to be approved again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
    /// Pick up from here: one box's new take goes on from this take's frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<Pickup>,
}

fn one() -> u32 {
    1
}

/// `POST /v1/projects/{id}/fit` and `…/fit/here`: what a render of `names` on this computer (`local`) asks of it, `all` and `outdated` as
/// a render reads them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FitAsk {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
}

/// How this computer would do what a render asks: each tool it would do poorly, and the GPU to rent instead. Empty: well enough, or
/// nothing says otherwise.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Fit {
    pub tools: Vec<ToolFit>,
    /// What the Hub places them all on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rent: Option<RentFor>,
}

/// One tool this computer would run poorly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolFit {
    pub tool: String,
    pub label: String,
    /// The assets of it the render makes.
    pub names: Vec<String>,
    /// How it would go here, in a line.
    pub line: String,
    /// Its weights' bytes, as its machine would load them; 0: unknown.
    #[serde(default)]
    pub bytes: u64,
    /// Render here was said of it on this computer, for weights as big: nothing asks again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub here: bool,
}

/// A GPU to rent for it, as `POST /v1/machines` `{kind: rent}` names one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RentFor {
    pub sku: String,
    pub gpus: u32,
    /// The card, in a person's words: `RTX 4090`.
    pub card: String,
    pub vram_gb: i64,
    pub usd_micros_per_hour: i64,
    /// What renting it does for these, in a line.
    pub line: String,
}

/// `POST /v1/projects/{id}/approve`: `name` pinned in project.yaml to `version`: a short run of digits is its version (1 first),
/// anything else a hash prefix of any file in the project's `files/`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Approve {
    pub name: String,
    pub version: String,
}

/// What an approve pinned: the asset's version and files, and what that put out of date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Approved {
    pub name: String,
    pub noun: String,
    /// The version it is; 0 for a file that is none of its versions.
    pub number: u32,
    pub files: BTreeMap<String, File>,
    /// The file people mean by it.
    pub path: String,
    /// What this put out of date.
    pub stale: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// What `If-Match` names from here on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// `POST /v1/projects/{id}/unpin`: `name`'s approval gone, so its state follows its inputs again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Unpin {
    pub name: String,
}

/// What an unpin left: the version it had pinned and what that put out of date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Unpinned {
    pub name: String,
    /// The version it had pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// What this put out of date, itself too.
    pub stale: Vec<String>,
    /// What making the tools that read it now said.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// What `If-Match` names from here on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// `PUT /v1/projects/{id}/machines` (cl-873): the machines the project's runs and renders go to, each to the one of them that finishes
/// it soonest, unless a run names its own; none: any machine. Answered as kept: each by its name, once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ProjectMachines"))]
pub struct Machines {
    pub machines: Vec<String>,
}

/// `POST /v1/projects/{id}/rename`: what project.lock keeps of `from` kept as `to`, an asset project.yaml names now with no
/// versions. Nothing on disk moves; people rename the asset, and its `<mentions>`, in project.yaml.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Rename {
    pub from: String,
    pub to: String,
}

/// A rename done: `from`, `to`, and how many versions it kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Renamed {
    pub from: String,
    pub to: String,
    pub versions: u32,
}

/// `POST /v1/projects/{id}/reclaim`: lists what it would delete: orphans' versions, and any file nothing else names. With
/// `yes` it deletes them. Never automatic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reclaim {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yes: bool,
}

/// What a reclaim listed or deleted: orphans' versions, files nothing names, and their bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reclaimed {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<Orphan>,
    /// The files nothing but the orphans names, or nothing at all, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// Of what nothing names, what nothing has a record of: it may be all that's left of a take, so `yes` leaves it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unrecorded: u64,
    /// Whether they are gone (`yes`), or only listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
    /// Commands to run next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// Why `yes` would be refused: its lock was lost, so these files may be all that's left of its takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<String>,
    /// What runs left in `.runs/` that `files/` doesn't hold by its bytes, which `yes` leaves: it may be an output's only copy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unheld: Vec<Unheld>,
}

/// A file or folder a run left in `.runs/`, where its run's record names it, with bytes `files/` doesn't hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unheld {
    /// From the project's folder: `.runs/<name>`.
    pub path: String,
    /// A file's; a folder has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub bytes: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What a walk landed: how many takes and versions, how long it took, and its runs' execution time summed, which is more
/// than that when they ran at once.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Rendered {
    pub takes: u32,
    pub versions: u32,
    pub elapsed_ms: u64,
    pub execution_ms: u64,
}

/// A render as a view draws it: everything it covers, what has ended first, then what is going, then what waits. It is
/// the whole of what a client needs to show how a render goes, so a client follows no run of its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Board {
    /// Goes up with each change, across a Calcifer restart: a client keeps the highest it heard.
    pub seq: u64,
    /// The machines it may go to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
    pub began_at_ms: i64,
    /// How long it took, once its walk ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// How many shots it will make, what's queued too: what a job's `shot` counts up to. It grows only as another render joins.
    #[serde(default)]
    pub shots: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<Job>,
    /// What machines are fetching ahead of what's queued, one per function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ahead: Vec<Ahead>,
    /// What it said as it went, each line once, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub said: Vec<Said>,
    /// What failed of no one asset, each once; what does not read it goes on. A job's failure is its own `error`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Error>,
    /// How its walk ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// A line a render said, once however often it came: of the assets `names` (a warning's are those whose runs gave it), or of the
/// render.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Said {
    pub level: crate::calcifer::NoteLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// How a render's walk ended: nothing it was asked is left to send or follow, and every take it made is whole here. `error` is the
/// first thing that failed, or when more than that was asked, `render_incomplete`: how far it got, that failure, and the command for
/// the rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "RenderEnded"))]
pub struct Ended {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
    /// What it landed, if it landed anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<Rendered>,
    /// The renders it ended, by `Asked::ask`: an ask made as another walk ends is the next one's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<String>,
}

/// How the target of a run fetching ahead starts.
pub const AHEAD: &str = "ahead ";

/// One function's code and weights a machine is fetching ahead of jobs that will need them. It is no job of anyone's: wrong, it costs
/// bytes, and its job's run fetches what it needs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Ahead {
    /// `org/package/function`.
    pub function: String,
    pub machine: String,
    /// Its run, kept out of the list people read.
    pub run: u64,
    /// `installing it`, `downloading its weights`, …; none once it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub total: u64,
    /// It's all there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ready: bool,
    /// Why it stopped short: the disk has no room, or it was canceled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_back: Option<String>,
}

/// How a job stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done,
    Failed,
    Canceled,
    /// Its machines stopped taking work, so it was never sent.
    NotSent,
    /// It waits its turn, here or in its machine's line. Also what a reader shows of a state it doesn't know.
    #[default]
    #[serde(other)]
    Waiting,
}

/// One version of one asset being made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Job {
    /// The same for as long as the render goes, whatever it does: what `job.updated` names it by.
    #[serde(default)]
    pub id: String,
    pub asset: String,
    pub state: JobState,
    /// What its alternatives are called: `take` or `version`.
    pub noun: String,
    /// Its take or version, once one is reserved for it. None for a file that comes with another's take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Which of the render's shots it makes, 1 first, for the whole render: a `kind: video` its package makes. None for a cut or a still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot: Option<u32>,
    /// The assets it waits for, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// How many GPUs it is planned on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<u32>,
    /// When it should start and end, as the walker works it out from what has run here: to the nearest 10 s, as a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at_ms: Option<i64>,
    /// A sentence: what it waits for (`after a, b`, `comes with c`, `waiting for a GPU, held by d`), or what came of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The run making it now, or that last did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// When its machine began it. Its time counts from here, never its queue's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    /// How long its runs executed, once they ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took_ms: Option<u64>,
    /// Where it is, in words: `segment 2/2, denoise`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// How far through that: `14/30`, `12 GiB/52 GiB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How far through the whole run, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// It has stopped moving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<crate::calcifer::Stalled>,
    /// What it has shown so far while it goes on: never the take or version itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Preview>,
    /// Its file in the project, once done: `files/<sha256>.<ext>`, or a cut's film.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A cut's length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// Why it failed, and what to run next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

/// A file a run watched before it ended: its package's word for it, in a sentence (`segment 1/2`), and its path in the project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Preview {
    pub label: String,
    pub path: String,
}

/// Who made a change to a project: the page (`client` names its tab), the CLI, a hand in project.yaml, or another client of the API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct By {
    /// `page`, `cli`, `file` or `api`.
    pub via: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

/// One edit of a project as its history keeps it: what it did in plain words, and whether it's undone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HistoryEntry {
    pub id: u64,
    pub at_ms: i64,
    pub by: By,
    pub words: String,
    /// The assets it touched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// The page's own ids for the edit (`Cozy-Edit`), which undo and redo take as well as `id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone: Option<Undone>,
}

/// When a history entry was undone, and by whom.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Undone {
    pub at_ms: i64,
    pub by: By,
}

/// `GET /v1/projects/{id}/history`: newest first, and the entries undo and redo would take now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct History {
    pub entries: Vec<HistoryEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redo: Option<u64>,
}

/// `POST /v1/projects/{id}/history/undo` and `…/redo`: one entry by its `id` or the page's `edit`; with `to`, every entry back to it (undo)
/// or up to it (redo); with nothing, the next.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HistoryStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
}

/// What a step undid or redid, in the order it did them; `already` when the entry asked was so already.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HistoryStepped {
    pub entries: Vec<HistoryEntry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
    /// What refused the next of a `to`, short of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<Error>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redo: Option<u64>,
}

/// Local project encryption. Plain projects never need a mount provider or credentials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VaultStatus {
    pub encrypted: bool,
    /// plain, locked, unlocked, or locking (a native application still holds the mount).
    pub state: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    #[serde(default = "default_vault_idle_seconds")]
    pub idle_timeout_s: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
}

/// The default local project unlock window; explicit zero still disables idle expiry.
pub const DEFAULT_VAULT_IDLE_SECONDS: u64 = 1800;
fn default_vault_idle_seconds() -> u64 { DEFAULT_VAULT_IDLE_SECONDS }

/// POST /v1/projects/{id}/vault. Credentials are deliberately not Debug; neither the request nor its response is cached in history.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VaultAsk {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_s: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_s: Option<u64>,
    /// Legacy removal flag. Verified source removal is now the default; false does not preserve it.
    #[serde(default)]
    pub remove_plaintext: bool,
    /// Explicitly keep the original plaintext after a verified encrypt conversion. Not valid for rekey/decrypt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preserve_plaintext: bool,
}

/// A vault operation can disclose a new recovery secret exactly to the caller. Do not persist this response in request/history caches.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VaultChanged {
    pub vault: VaultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserved_source: Option<PathBuf>,
    /// The project's runs a lock canceled first, by number.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ended: Vec<u64>,
    /// Already-missing retained media skipped during conversion. Their project records remain missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_files: Vec<PathBuf>,
}

/// A private vault-operation stream. Progress counts copied bytes and completed files, not an estimated percentage.
/// Only the terminal reply can contain a recovery secret; never record these events in history or shared event streams.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VaultEvent {
    Accepted { action: String },
    Progress {
        phase: String,
        files_done: u64,
        bytes_done: u64,
        #[serde(default)]
        files_total: u64,
        #[serde(default)]
        bytes_total: u64,
    },
    Completed { changed: VaultChanged },
    Failed { error: Error },
}
