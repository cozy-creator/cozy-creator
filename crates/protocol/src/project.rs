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
/// /v1/projects/{id}/assets/{sha256}.{ext}`), its sha256, and for a file project.yaml names, the path
/// written there (its asset is a link to it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
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
    /// The approved one's number (1 first, never reused); None when it reads a file attached by
    /// hand, or has none yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<u32>,
    /// What it reads as now, by output name (`video`, `image`, `clean`, `latents`): the approved
    /// one's assets, else the file attached in project.yaml.
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
}

/// `POST /v1/projects/{id}/runs`: `n` new versions of each of `names`, or with `all` every node
/// brought up to date, each new version approved, then the cuts. It joins the project's walk,
/// which sends each version once what it reads is settled; `on` lists the machines they may go to.
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
    },
    Said {
        message: String,
    },
    Warning {
        message: String,
    },
    /// A cut (`edit.mp4`, `preview.mp4`, and its `.otio`) was made again here, in `took_s`: `frames`
    /// long at `fps`, the takes in `retimed` retimed to it.
    Cut {
        cut: String,
        frames: u64,
        fps: f64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        retimed: Vec<String>,
        took_s: f64,
    },
    /// The walk is over: nothing it was asked is left to send or follow; `error` is the first thing
    /// that failed.
    Ended {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Error>,
    },
}
