//! The repo documents: `cozytensors.tree/1`, a commit (one lane, one cozytensors file and the repo
//! files beside it), and `cozytensors.repo/1`, a model's releases and commits. Both parse through a
//! `Wire` twin that carries the `format` tag.

use std::collections::BTreeMap;
use std::str::FromStr;

use pep440_rs::Version;
use serde::{Deserialize, Serialize};

use super::doc::{path, segment};
use super::{check_format, invalid, is_header, json, Error, ObjectRef, Result};
use super::{HEADER_PATH, REPO_FORMAT, TREE_FORMAT};

/// The folder that holds a plain folder's chunks and assets: no repo file lives under it.
pub const CHUNKS_DIR: &str = "chunks";

/// `cozytensors.tree/1`: a commit, one immutable state of a model. Its `lane` is the label its
/// author gave it (`bf16`, `fp8`, `int8-convrot`, …), read only by a consumer asking for that
/// label. Its one `.cozytensors` file, at any path, is the header; every other file (configs, a
/// tokenizer, README) is a repo file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "TreeWire", into = "TreeWire")]
pub struct Tree {
    pub lane: String,
    pub files: BTreeMap<String, ObjectRef>,
}

#[derive(Serialize, Deserialize)]
struct TreeWire {
    format: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    lane: String,
    files: BTreeMap<String, ObjectRef>,
}

impl TryFrom<TreeWire> for Tree {
    type Error = Error;
    fn try_from(w: TreeWire) -> Result<Self> {
        check_format(&w.format, TREE_FORMAT)?;
        for p in w.files.keys() {
            check_file(p)?;
        }
        Ok(Tree { lane: w.lane, files: w.files })
    }
}

impl From<Tree> for TreeWire {
    fn from(t: Tree) -> Self {
        TreeWire { format: TREE_FORMAT.into(), lane: t.lane, files: t.files }
    }
}

/// A repo file path: plain segments, not under [`CHUNKS_DIR`].
pub(crate) fn check_file(p: &str) -> Result<()> {
    if !path(p) || p.split('/').next() == Some(CHUNKS_DIR) {
        return invalid(format!("file path {p:?} is not a repo file path"));
    }
    Ok(())
}

/// A lane or release label: alphanumeric first, then at most 63 of `A-Za-z0-9._+!-`.
pub fn label(text: &str) -> bool {
    let b = text.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._+!-".contains(c))
}

impl Tree {
    /// A commit of `header` at [`HEADER_PATH`] in lane `lane` (empty: unlabelled, a machine's
    /// own), beside `files` (their own `.cozytensors` files dropped: a commit holds one).
    pub fn of(lane: &str, header: ObjectRef, files: &BTreeMap<String, ObjectRef>) -> Result<Tree> {
        if !lane.is_empty() && !label(lane) {
            return invalid(format!("lane {lane:?}: start alphanumeric, then at most 63 of letters, digits, . _ + ! -"));
        }
        let mut files: BTreeMap<String, ObjectRef> = files.iter().filter(|(p, _)| !is_header(p)).map(|(p, r)| (p.clone(), r.clone())).collect();
        files.insert(HEADER_PATH.into(), header);
        Ok(Tree { lane: lane.into(), files })
    }

    pub fn from_json(bytes: &[u8]) -> Result<Tree> {
        json::read(bytes, TREE_FORMAT)
    }
    pub fn to_json(&self) -> Result<Vec<u8>> {
        json::write(self)
    }

    /// The commit's one cozytensors file: its path and header.
    pub fn header(&self) -> Result<(&str, &ObjectRef)> {
        let mut headers = self.files.iter().filter(|(p, _)| is_header(p));
        match (headers.next(), headers.next()) {
            (Some((path, header)), None) => Ok((path, header)),
            (None, _) => invalid("the commit holds no .cozytensors file"),
            (Some(_), Some(_)) => invalid("the commit holds several .cozytensors files; a commit holds one"),
        }
    }

    /// The repo files: everything but the header.
    pub fn repo_files(&self) -> impl Iterator<Item = (&str, &ObjectRef)> {
        (self.files.iter()).filter(|(p, _)| !is_header(p)).map(|(p, r)| (p.as_str(), r))
    }
}

/// `cozytensors.repo/1`: a model's releases, oldest first, each naming one commit per lane, and
/// its other commits (private until a release names one). Every commit it names is a root.
/// A store's record of a model, read and written only by its `get_ref` and `update_ref`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "RepoWire", into = "RepoWire")]
pub struct Repo {
    pub name: String,
    pub releases: Vec<Release>,
    pub commits: Vec<ObjectRef>,
}

/// A release: a version, higher than every earlier one, and its commit of each lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub lanes: BTreeMap<String, ObjectRef>,
}

#[derive(Serialize, Deserialize)]
struct RepoWire {
    format: String,
    name: String,
    #[serde(default)]
    releases: Vec<Release>,
    #[serde(default)]
    commits: Vec<ObjectRef>,
}

impl TryFrom<RepoWire> for Repo {
    type Error = Error;
    fn try_from(w: RepoWire) -> Result<Self> {
        check_format(&w.format, REPO_FORMAT)?;
        let repo = Repo { name: w.name, releases: w.releases, commits: w.commits };
        repo.check()?;
        Ok(repo)
    }
}

impl From<Repo> for RepoWire {
    fn from(r: Repo) -> Self {
        RepoWire { format: REPO_FORMAT.into(), name: r.name, releases: r.releases, commits: r.commits }
    }
}

/// The root namespace's org (Tensorhub's first-party models): a name with no org, `sdxl`, is
/// `_/sdxl` in a reference, a repo key and a URL.
pub const ROOT_ORG: &str = "_";

/// A repository name: `<org>/<name>`, two segments.
pub(crate) fn repo_name(name: &str) -> bool {
    name.split_once('/').is_some_and(|(org, name)| segment(org) && segment(name))
}

impl Repo {
    pub fn new(name: &str) -> Repo {
        Repo { name: name.into(), ..Repo::default() }
    }
    pub fn from_json(bytes: &[u8]) -> Result<Repo> {
        json::read(bytes, REPO_FORMAT)
    }
    /// The canonical bytes; refuses a document a reader would refuse.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        self.check()?;
        json::write(self)
    }
    fn check(&self) -> Result<()> {
        if !repo_name(&self.name) {
            return invalid(format!("repo name {:?} is not <org>/<name>", self.name));
        }
        for release in &self.releases {
            if !label(&release.version) || release.lanes.keys().any(|lane| !label(lane)) {
                return invalid(format!("release {:?} or one of its lanes is not a label", release.version));
            }
        }
        Ok(())
    }

    /// Release `version`.
    pub fn release(&self, version: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.version == version)
    }

    /// Records `commit` as `version`'s lane `lane`, the release kept in version order.
    pub fn set_lane(&mut self, version: &str, lane: &str, commit: ObjectRef) {
        match self.releases.iter_mut().find(|r| r.version == version) {
            Some(release) => drop(release.lanes.insert(lane.into(), commit)),
            None => {
                let release = Release { version: version.into(), lanes: BTreeMap::from([(lane.into(), commit)]) };
                let after = |r: &Release| order(&r.version) > order(version);
                let at = self.releases.iter().position(after).unwrap_or(self.releases.len());
                self.releases.insert(at, release);
            }
        }
    }

    /// Records `commit` as one of the repo's, the newest.
    pub fn add_commit(&mut self, commit: ObjectRef) {
        self.commits.retain(|c| *c != commit);
        self.commits.push(commit);
    }

    /// The commit `release` (none: the newest) names in `lane` (none: its only lane), as
    /// (release, lane, commit). A repo without releases (a machine's own model) answers its newest
    /// commit, in no release. Refused naming what the repo has.
    pub fn resolve(&self, release: Option<&str>, lane: Option<&str>) -> Result<(String, Option<String>, ObjectRef)> {
        if self.releases.is_empty() && release.is_none() {
            return match self.commits.last() {
                Some(commit) => Ok((String::new(), lane.map(str::to_owned), commit.clone())),
                None => invalid(format!("{} has no commit", self.name)),
            };
        }
        let found = match release {
            Some(version) => self.release(version),
            None => self.releases.last(),
        };
        let Some(found) = found else {
            let has: Vec<&str> = self.releases.iter().map(|r| r.version.as_str()).collect();
            return invalid(format!("{} has no release {} (has {has:?})", self.name, release.unwrap_or("at all")));
        };
        let picked = match lane {
            Some(lane) => found.lanes.get_key_value(lane),
            None if found.lanes.len() == 1 => found.lanes.iter().next(),
            None => None,
        };
        match picked {
            Some((lane, commit)) => Ok((found.version.clone(), Some(lane.clone()), commit.clone())),
            None => {
                let has: Vec<&str> = found.lanes.keys().map(String::as_str).collect();
                let wanted = lane.map_or_else(|| "name a lane".to_owned(), |lane| format!("has no lane {lane:?}"));
                invalid(format!("{}@{}: {wanted} (lanes {has:?})", self.name, found.version))
            }
        }
    }

    /// Every commit the repo keeps alive, each once.
    pub fn roots(&self) -> Vec<&ObjectRef> {
        let mut roots: Vec<&ObjectRef> = self.releases.iter().flat_map(|r| r.lanes.values()).chain(&self.commits).collect();
        roots.sort();
        roots.dedup();
        roots
    }
}

/// The one "newest" ordering: of `items`, the one whose `version` is the highest final PEP 440
/// version (a post-release is final), else the highest pre-release; of equal versions (`1.0`,
/// `1.0.0`), the last given. Items whose version does not parse are skipped.
pub fn newest_by<T>(items: impl IntoIterator<Item = T>, version: impl Fn(&T) -> &str) -> Option<T> {
    let versioned = items.into_iter().filter_map(|item| Some((Version::from_str(version(&item)).ok()?, item)));
    let order = |(a, _): &(Version, T), (b, _): &(Version, T)| a.is_stable().cmp(&b.is_stable()).then_with(|| a.cmp(b));
    versioned.max_by(order).map(|(_, item)| item)
}

/// A version's order (spec §7): PEP 440, an unparsable one first.
fn order(version: &str) -> Option<Version> {
    Version::from_str(version).ok()
}
