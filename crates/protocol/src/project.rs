//! Calcifer's project API (xs-151, cl-758): the projects this computer knows, a project folder
//! opened by its path, its assets as they stand and their versions (a video's are takes), the runs
//! that make them, what happens as they go, and its files and cuts. The CLI's `cozy project` verbs
//! are one client of it.
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

/// `POST /v1/projects/{id}/runs`: what the ask found. `/events` opens with the runs already going.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub id: String,
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
}

/// One thing an ask queued, and the names ahead of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedNow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ahead: Vec<String>,
}

/// One thing a project has to do: running, with its run's number once it has one, else queued.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

/// `GET /v1/projects/{id}`: its assets in order, and what reading the project said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub path: PathBuf,
    pub assets: Vec<Asset>,
    /// Identity and links, no work: they make nothing and hold no version.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<Entity>,
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
    /// Whether a push or pull is going.
    #[serde(default)]
    pub moving: bool,
    /// How many files project.lock names are not on this computer (`POST /v1/projects/{id}/pull`).
    #[serde(default)]
    pub absent: u64,
    /// How many files `cozy run`s left here, that belong to no asset.
    #[serde(default)]
    pub run_files: u64,
    /// What its `cozy/cut` assets made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<Cut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stale {
    pub name: String,
    pub why: String,
}

/// An asset project.lock keeps versions of that project.yaml no longer names: the bytes `reclaim`
/// would free of it (none that anything else names, and each file once across the orphans), and,
/// when project.yaml has exactly one new asset of its kind with none, that asset: renamed, it would
/// be this (`POST /v1/projects/{id}/rename`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
pub struct File {
    pub path: String,
    pub sha256: String,
}

/// `POST /v1/projects/{id}/imports`: the file at `source` (an absolute path, or an `http(s)` URL) kept in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Import {
    pub source: String,
    /// Runs `birefnet/remove_background` on it as a run of the project, whose output is the one file kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_background: bool,
    /// The machine that run goes to; absent: this computer if it can run models, else a machine you have up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<crate::calcifer::On>,
}

/// What an import kept, its hash prefix, and the line to paste into project.yaml.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    pub path: String,
    pub sha256: String,
    pub prefix: String,
    /// `image: ab12cd34ef56`, or `video:` or `audio:`, by what the file is.
    pub line: String,
    /// Its bytes were in `files/` already, so nothing was copied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
    /// The run removing its background, to follow: its output is the file kept and the line to paste, and the fields above are empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Box<crate::calcifer::Run>>,
    /// Why its background wasn't removed: a plain copy stands, and `line` is its.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A version's number and the hash prefix project.yaml can name its file by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionPrefix {
    pub number: u32,
    pub prefix: String,
    /// What it keeps of another take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Kept>,
}

/// The start of another take that a take keeps (`render --take --in`): which, and up to where. What follows is new.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub take: u32,
    pub until_s: f64,
}

impl Kept {
    /// `clash: take 3 is take 2 to 15.08 s, then new`.
    pub fn said(&self, name: &str, number: u32) -> String {
        format!("{name}: take {number} is take {} to {:.2} s, then new", self.take, self.until_s)
    }
}

/// A `cozy/cut` asset's film at the project's root (`GET /v1/projects/{id}/cuts/{cut}`; its timeline
/// beside it, `<name>.otio`): its file, when it was last cut and how many frames long at `fps`
/// (None: not yet), the takes it retimed to that rate (`<asset> (<fps> fps)`), and what it cuts that
/// has nothing made yet, which it skips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
}

/// How an asset stands, for a client that shows it rather than says it (`status` says it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub standing: Standing,
    /// The runs making a version of it now (`GET /v1/runs/{id}` says how far).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub making: Vec<u64>,
    /// Free text from project.yaml.
    pub kind: String,
    /// What its alternatives are called: `take` (one that holds a video) or `version`.
    pub noun: String,
    /// The function that makes it, as project.yaml or its defaults name it. None for a file pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
    /// How many it has.
    pub versions: u32,
    /// Each one's hash prefix, to name it in project.yaml.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefixes: Vec<VersionPrefix>,
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
    /// The function that made its state, `org/package/function@release`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<String>,
    /// For people, in its own noun: current, no take yet, out of date and why, making (run N), …
    pub status: String,
}

/// `GET /v1/projects/{id}/assets/{name}`: an asset and each of its versions, 1 first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetVersions {
    pub asset: Asset,
    pub versions: Vec<Version>,
}

/// One version (a take, if it holds a video).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// What it keeps of another take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Kept>,
}

/// `POST /v1/projects/{id}/runs`: for each of `names`, or `all`, and what it reads, if it has no state, run its operation.
/// Nothing else runs. `new`, or `n` over 1, also draws `n` new seeds for those named. It joins the project's walk, which sends
/// each once what it reads is settled; `on` lists the machines they may go to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// One asset's new take keeps the start of this take, not the one in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<u32>,
    /// One asset's new take keeps that take up to here, in seconds, and regenerates what follows: not from its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_s: Option<f64>,
}

fn one() -> u32 {
    1
}

/// An entity, with its links (field to asset) and sub-entities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub kind: String,
    /// What it is, as the file says; a sub-entity with none has its parent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subs: Vec<Entity>,
}

/// `POST /v1/projects/{id}/approve`: `name` pinned in project.yaml to its version `version` (1 first), or to `file`, a hash prefix
/// of any file in the project's `files/`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Approve {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
}

/// `POST /v1/projects/{id}/unpin`: `name`'s approval gone, so its state follows its inputs again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unpin {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unpinned {
    pub name: String,
    /// The version it had pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// What this put out of date, itself too.
    pub stale: Vec<String>,
}

/// `POST /v1/projects/{id}/rename`: what project.lock keeps of `from` kept as `to`, an asset project.yaml names now with no
/// versions. Nothing on disk moves; people rename the asset, and its `<mentions>`, in project.yaml.
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
    /// A render is going: it happens as that ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
}

/// `POST /v1/projects/{id}/reclaim`: lists what it would delete: orphans' versions, and any file nothing else names. With
/// `yes` it deletes them. Never automatic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reclaim {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yes: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reclaimed {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<Orphan>,
    /// The files nothing but the orphans names, or nothing at all, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// Whether they are gone (`yes`), or only listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

/// What a push or pull moved (`POST /v1/projects/{id}/push`, `/pull`). A push sends every file
/// project.lock names that the Hub lacks; a pull fetches every one `files/` lacks.
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
    /// `name`'s version `number` (its `noun`) was made: its files, and the one people mean by it.
    Landed {
        name: String,
        noun: String,
        number: u32,
        files: BTreeMap<String, File>,
        path: String,
        /// Its run's execution time.
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
    /// A cut (`edit.mp4`, and its `.otio`) was made here, in `took_s`: `frames` long at `fps`, the takes
    /// in `retimed` retimed to it. `unchanged`: what it holds did not change, so it was left.
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
    /// The walk is over: nothing it was asked is left to send or follow. `error` is the first thing that
    /// failed (`Failed` said it already, unless the walk itself stopped), or when more than that was
    /// asked, `render_incomplete`: how far it got, that failure, and the command for the rest.
    Ended {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Error>,
        /// What this walk landed, if it landed anything.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rendered: Option<Rendered>,
    },
}
