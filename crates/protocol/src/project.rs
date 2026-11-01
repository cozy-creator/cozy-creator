//! Calcifer's project API (xs-136, xs-139): a project folder opened by its path, its nodes as they stand,
//! the runs that make their versions, and what happens as they go. The CLI's `cozy project` verbs
//! and the web UI are its clients.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{calcifer::On, error::Error};

/// `POST /v1/projects`: the project in `path`, made first when `create`. The same path is always
/// the same project.
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

/// `GET /v1/projects/{id}`: its nodes in order, and what reading the project said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub path: PathBuf,
    pub nodes: Vec<Node>,
    /// Whether its walk is going: runs being made, or asked and not yet sent.
    pub busy: bool,
    pub edit: Edit,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The edit, cut here: where it is, when it was last cut (None: not yet), and the shots it cuts that
/// have no version yet, which it skips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skips: Vec<String>,
}

/// A node: an entity (character, prop, environment), a setup, a world state, a shot or a shot's
/// upscale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub kind: String,
    pub versions: u32,
    /// The file it reads as now, relative to the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Which version is approved, 1 first; None for a file attached by hand, or none yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<u32>,
    /// The function that made that version, `org/package/function@release`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<String>,
    /// For people: current, no version yet, out of date and why, making (run N), …
    pub status: String,
}

/// `POST /v1/projects/{id}/runs`: `n` new versions of each of `names`, or with `all` every node
/// brought up to date, each new version approved, then the edit cut. It joins the project's walk,
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
    pub file: String,
    /// What this put out of date.
    pub stale: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
    /// `file` became one of `name`'s versions.
    Landed {
        name: String,
        file: String,
    },
    Said {
        message: String,
    },
    Warning {
        message: String,
    },
    /// The edit, cut here.
    Cut {
        path: PathBuf,
        seconds: f64,
    },
    /// The walk is over: nothing it was asked is left to send or follow; `error` is the first thing
    /// that failed.
    Ended {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Error>,
    },
}
