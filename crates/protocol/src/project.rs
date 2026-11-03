//! Calcifer's project API (xs-136, xs-139, cl-716): the projects this computer knows, a project
//! folder opened by its path, its nodes as they stand and their versions (a shot's are takes), the
//! runs that make them, what happens as they go, and its assets and cuts. The CLI's `cozy project`
//! verbs are one client of it.
use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{calcifer::On, error::Error};

/// `POST /v1/projects`: the project holding `path` (the nearest folder up from it with a
/// project.yaml), or with `create` a new one made at `path`. The same folder is always the same
/// project, and opening it records it in the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Open {
    pub path: PathBuf,
    #[serde(default)]
    pub create: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    pub id: String,
    pub path: PathBuf,
}

/// `GET /v1/projects`: the projects this computer's Calcifer has opened, last used first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projects {
    pub projects: Vec<Known>,
    /// How many more are older than this page.
    #[serde(default)]
    pub older: u64,
}

/// One project in the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Known {
    pub id: String,
    /// Its folder's name.
    pub name: String,
    pub path: PathBuf,
    pub used_at_ms: i64,
    /// Its folder holds no project.yaml any more (moved or deleted); `DELETE` forgets it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// Its runs are being made, or asked and not yet sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub busy: bool,
}

/// `GET /v1/projects/{id}`: its nodes in order, and what reading the project said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub path: PathBuf,
    /// The edit's frame rate (project.yaml's `fps:`), which its positions count in.
    pub fps: f64,
    pub nodes: Vec<Node>,
    /// Nodes with nothing made yet that the project needs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Nodes out of date, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<Stale>,
    /// Nodes project.lock keeps versions of that project.yaml no longer names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<Orphan>,
    /// Whether its walk is going: runs being made, or asked and not yet sent.
    pub busy: bool,
    /// Whether a push or pull is going.
    #[serde(default)]
    pub moving: bool,
    /// How many files project.lock names are not on this computer (`POST /v1/projects/{id}/pull`).
    #[serde(default)]
    pub absent: u64,
    /// The cuts it makes: `edit.mp4`, and `preview.mp4` when it has a `resolution:`.
    pub cuts: Vec<Cut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stale {
    pub name: String,
    pub why: String,
}

/// A node project.lock keeps versions of that project.yaml no longer names, and, when project.yaml
/// has exactly one new node of its kind with none, that node: renamed, it would be this
/// (`POST /v1/projects/{id}/rename`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Orphan {
    pub name: String,
    pub kind: String,
    pub versions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_to: Option<String>,
}

/// A file the project uses: its path in the project (`assets/<sha256>.<ext>`, `GET
/// /v1/projects/{id}/assets/{sha256}.{ext}`) and its sha256.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub sha256: String,
}

/// `POST /v1/projects/{id}/imports`: the file at `source` (an absolute path, or an `http(s)` URL) kept as
/// an asset. With `clean` (`--character`, `--prop`) it needs its background removed; that function is
/// read from `hub`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Import {
    pub source: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clean: bool,
    pub hub: String,
}

/// What an import kept: its asset, the hash prefix and line to paste into project.yaml, and its clean.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    pub path: String,
    pub sha256: String,
    pub prefix: String,
    /// `image: ab12cd34ef56`, or `video:` or `audio:`, by what the file is.
    pub line: String,
    /// Its bytes were in `assets/` already, so nothing was copied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
    pub clean: Clean,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// An imported picture's background: not to be removed; removed by an earlier run, so none was sent; being removed by run `run` (which
/// `GET /v1/runs/{id}/events` follows; `cleaned` on the project's events says it landed); or to be removed
/// by the first machine that renders something that reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Clean {
    Untouched,
    Removed,
    Removing { run: String },
    Later,
}

/// A version's number and the hash prefix project.yaml can name its primary asset by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionPrefix {
    pub number: u32,
    pub prefix: String,
}

/// A cut, made here: its name (`GET /v1/projects/{id}/cuts/{cut}`; its timeline beside it,
/// `<name>.otio`), its file, when it was last cut and how many frames long (None: not yet), the
/// takes it retimed to the project's rate (`<node> (<fps> fps)`), and the shots it cuts that have
/// nothing made yet, which it skips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    pub cut: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retimed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skips: Vec<String>,
}

/// A node: an entity (character, prop, environment), a setup, a world state, a shot or a shot's
/// upscale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub kind: String,
    /// What its alternatives are called: `take` (a shot's, and its upscale's) or `version`.
    pub noun: String,
    /// How many it has.
    pub versions: u32,
    /// Each one's hash prefix, to name it in project.yaml.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefixes: Vec<VersionPrefix>,
    /// The number (1 first, never reused) of the one chosen by `approve` or `--approve`; None when none was, and it reads as
    /// the newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<u32>,
    /// Its own result is a file project.yaml names: that is what it reads, whatever is approved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// What it reads as now, by output name (`video`, `image`, `clean`, `latents`): the file project.yaml pins it to, else the
    /// approved or newest one's assets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, Asset>,
    /// The file it reads as now (its primary asset's path), for people who want it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The function that made the approved one, `org/package/function@release`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<String>,
    /// For people, in its own noun: current, no take yet, out of date and why, making (run N), …
    pub status: String,
}

/// `GET /v1/projects/{id}/nodes/{name}`: a node and each of its versions, 1 first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeVersions {
    pub node: Node,
    pub versions: Vec<Version>,
}

/// One version (a take, for a shot or its upscale).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub number: u32,
    /// By output name.
    pub assets: BTreeMap<String, Asset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<String>,
    /// The runs that made it, in order (a shot in consecutive calls has one per call).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approved: bool,
    /// What its function said of it, and what was left out of its request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `POST /v1/projects/{id}/runs`: `names`, or `all`, brought up to date, with what they read: what is missing or out of date
/// gets `n` new versions, and any approval it had goes. With `new` what is current gets them too, its approval kept. A pinned
/// node gets none. It joins the project's walk, which sends each version once what it reads is settled; `on` lists the machines
/// they may go to. `all` ends with the cuts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Runs {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default = "one")]
    pub n: u32,
    pub on: Vec<On>,
    /// The Hub its renderers are read from and its runs belong to.
    pub hub: String,
    /// With `all`: no upscales, and only the native cut (`preview.mp4`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
    /// The new version is approved when it lands; only with `n` 1.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approve: bool,
    /// Render what is current too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
}

fn one() -> u32 {
    1
}

/// `POST /v1/projects/{id}/approve`: version `version` (1 first) approved for `name`, in project.lock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Approve {
    pub name: String,
    pub version: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Approved {
    pub name: String,
    pub noun: String,
    pub number: u32,
    pub assets: BTreeMap<String, Asset>,
    /// Its primary asset: the file the node reads as now.
    pub path: String,
    /// What this put out of date.
    pub stale: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `POST /v1/projects/{id}/rename`: what project.lock keeps of `from` (versions, approval, what read
/// it) kept as `to`, a node project.yaml names now (of the same kind, with no versions). Nothing on
/// disk moves; people rename the node, and its `<mentions>`, in project.yaml.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Renamed {
    pub from: String,
    pub to: String,
    pub versions: u32,
}

/// What a push or pull moved (`POST /v1/projects/{id}/push`, `/pull`). A push sends every file
/// project.lock names that the Hub lacks; a pull fetches every one `assets/` lacks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transferred {
    /// The files project.lock names.
    pub files: u64,
    /// Those sent or fetched now.
    pub moved: u64,
    pub bytes: u64,
    /// The files it could not move, which stopped nothing else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
    /// A push's: the private model and commit that keep them at the Hub, until deleted by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// A file a push or pull left out: its path in the project, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    pub why: String,
}

/// What a walk landed: how many takes and versions, how long it took, and its runs' execution time summed, which is more
/// than that when they ran at once.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rendered {
    pub takes: u32,
    pub versions: u32,
    pub elapsed_ms: u64,
    pub execution_ms: u64,
}

/// `GET /v1/projects/{id}/events`: what the project's runs do, as they do it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A run for `name` began: its id and number, which `GET /v1/runs/{id}/events` follows.
    Started {
        name: String,
        run: String,
        number: u64,
    },
    /// `name`'s version `number` (its `noun`) was made: its assets, and its primary one's path.
    Landed {
        name: String,
        noun: String,
        number: u32,
        assets: BTreeMap<String, Asset>,
        path: String,
        /// Its runs' execution time, its clean's too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_ms: Option<u64>,
    },
    Said {
        message: String,
    },
    Warning {
        message: String,
    },
    /// Something asked could not be made, said as it happens; the walk goes on with what does not
    /// read it.
    Failed {
        error: Error,
    },
    /// A push or pull landed `file` (None: it is under way), `bytes` moved so far.
    Moved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        bytes: u64,
    },
    /// A push or pull ended: what it moved, or what stopped it.
    Transferred {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        done: Option<Transferred>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Error>,
    },
    /// The run cleaning imported picture `sha256` ended: its clean is what the hash resolves to now, or
    /// `error` says why not, and the next render that reads it tries again.
    Cleaned {
        sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A cut (`edit.mp4`, `preview.mp4`, and its `.otio`) was made again here, in `took_s`: `frames`
    /// long at `fps`, the takes in `retimed` retimed to it. `unchanged`: what it holds did not change, so it was left.
    Cut {
        cut: String,
        frames: u64,
        fps: f64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        retimed: Vec<String>,
        took_s: f64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unchanged: bool,
    },
    /// The walk is over: nothing it was asked is left to send or follow; `error` is the first thing
    /// that failed (`Failed` said it already, unless the walk itself stopped).
    Ended {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Error>,
        /// What this walk landed, if it landed anything.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rendered: Option<Rendered>,
    },
}
