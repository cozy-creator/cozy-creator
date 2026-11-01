//! A project folder (Paul, 11-01): `project.yaml` (people's; cozy writes it only at `cozy project
//! new`), `project.lock` (cozy's: each node's versions, what made each, which is approved, the runs
//! still making one, and what its walk still owes), and the cut, `edit.mp4` and `edit.otio`. What cozy
//! makes is kept in one folder per node, its files named for people: a shot's or upscale's takes in
//! `renders/` (`renders/fight/fight_take002.mp4`, `fight_take002.latents`), every other node's
//! versions in `references/` beside people's own files there (`references/theo/theo_v001.png`,
//! `theo_v001_clean.png`). No file says which is approved: project.lock does. `plates/` holds only
//! people's footage. cozy writes and removes only the files it names, and never another.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

use platform::durable;
use protocol::{
    calcifer::On,
    error::{Error, Kind},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::graph::name_key;

pub const PROJECT: &str = "project.yaml";
pub const LOCK: &str = "project.lock";
/// The folders `cozy project new` makes: references/ (people's, and cozy's versions), plates/
/// (people's footage) and renders/ (cozy's takes). Where people keep a file is a convention, never
/// enforced.
pub const FOLDERS: [&str; 3] = ["references", "plates", "renders"];
/// A version's primary asset, the one its node reads as: its video, else its clean (a cleaned
/// picture's), else its image.
const PRIMARY: [&str; 3] = ["video", "clean", "image"];

/// A file cozy made: its path in the project (`renders/<node>/<file>`, `references/<node>/<file>`)
/// and its sha256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub sha256: String,
}

/// One version of a node (a take, for a shot or its upscale): its number, its assets, what made it
/// and what it was made from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Made {
    pub node: String,
    /// 1 first, never reused; 0 while nothing of it has landed.
    #[serde(default)]
    pub number: u32,
    /// By the run output's name (`video`, `image`, `latents`; a cleaned picture's `clean`). Empty
    /// while its run is pending.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, Asset>,
    /// The function, models, LoRAs and options that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<MadeBy>,
    /// What it was made from; None for the clean of an attached picture, which is never out of date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<Vec<Upstream>>,
    /// The frames it wrote for world states, by state.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub writes: BTreeMap<String, Asset>,
    /// The runs that made it, in order: one per call of a shot made in consecutive calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<u64>,
    /// A picture's clean, which is never cleaned again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cleaned: bool,
    /// The digest of the picture attached in project.yaml that this cleans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_of: Option<String>,
    /// A video's width and height, which decide whether its shot is upscaled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u64; 2]>,
}

/// What made a version: `with` as `org/package/function@release`, and its run's model choices and
/// options as its render block gave them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MadeBy {
    pub with: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub models: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub lora: Value,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
}

impl Made {
    /// The asset its node reads as (`PRIMARY`).
    pub fn primary(&self) -> Option<&Asset> {
        PRIMARY.iter().find_map(|name| self.assets.get(*name))
    }

    /// Its assets' and writes' paths.
    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.assets.values().chain(self.writes.values()).map(|asset| &asset.path)
    }
}

/// What a version read: a node's asset (a state's revision, with `state`), or a file of the
/// project's that is no node's (`<node>.<field>`), by its sha256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub asset: String,
}

/// A run making a version, recorded before it is sent (under its idempotency key) and followed
/// until it lands: whatever stops the walk, the next one follows it and never sends it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    /// What it makes.
    pub made: Made,
    /// Its run's number, once sent.
    #[serde(default)]
    pub run: u64,
    /// The Hub it was sent for, which a picture's clean goes to too.
    #[serde(default)]
    pub hub: String,
    /// The result field holding the version (`image`, `video`).
    pub field: String,
    /// The machines it may go to, which a shot's next call goes to too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<On>,
    /// For a shot: which of its consecutive calls this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<Call>,
    pub then: Then,
}

/// One of a shot's consecutive calls (one when it reads nothing it wrote itself): which, where each
/// ends on the shot's timeline, the states whose frames this one's video gives (at seconds into the
/// shot), and the video and latents of the call before it, which this one continues.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Call {
    pub index: usize,
    pub ends: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<(String, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<(Asset, Option<Asset>)>,
}

/// What becomes of a run's file once it lands.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Then {
    /// Listed as a version; approved when `approve`, or when the node has nothing else.
    List { approve: bool },
    /// Cleaned first, and its clean listed.
    Clean { approve: bool },
    /// The clean of a picture attached in project.yaml: listed, and used while that picture is.
    Attached,
}

/// A node's versions by number, the approved one's primary asset's path, and the last number
/// given (numbers are never given again).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Node {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<String>,
    #[serde(default)]
    pub versions: Vec<Made>,
    #[serde(default)]
    pub numbered: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Lock {
    /// By name key.
    #[serde(default)]
    pub nodes: BTreeMap<String, Node>,
    /// By idempotency key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default, skip_serializing_if = "Owed::is_empty")]
    pub walk: Owed,
}

/// What a render asked that the project's walk has not sent yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Owed {
    /// `render all`: every node up to date, then the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Target>,
    /// `render all --preview`: no upscales, and only the native cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
    /// What `all` sent this walk, made or failed, by name key: each is sent once.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sent: BTreeSet<String>,
    /// New versions of named nodes, one each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes: Vec<Take>,
}

impl Owed {
    pub fn is_empty(&self) -> bool {
        self.all.is_none() && self.sent.is_empty() && self.takes.is_empty()
    }
}

/// The machines a version may be made on, and the Hub its renderer is read from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub on: Vec<On>,
    pub hub: String,
}

/// One new version of `node`, sent under `key`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Take {
    pub key: String,
    pub node: String,
    #[serde(flatten)]
    pub target: Target,
    /// Approved when it lands.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approve: bool,
}

impl Lock {
    /// Whether its walk owes anything: asked and not sent, or sent and not landed.
    pub fn owes(&self) -> bool {
        self.walk.all.is_some() || !self.walk.takes.is_empty() || !self.pending.is_empty()
    }

    pub fn node(&self, node: &str) -> Option<&Node> {
        self.nodes.get(&name_key(node))
    }

    /// `node`'s version whose primary asset is at `path`.
    pub fn made(&self, node: &str, path: &str) -> Option<&Made> {
        self.node(node)?.versions.iter().find(|made| made.primary().is_some_and(|asset| asset.path == path))
    }

    /// `node`'s next number, given now: one past every number it gave.
    pub fn number(&mut self, node: &str) -> u32 {
        let node = self.nodes.entry(name_key(node)).or_default();
        node.numbered = node.numbered.max(node.versions.iter().map(|made| made.number).max().unwrap_or(0)) + 1;
        node.numbered
    }

    /// Lists `made` (its assets kept, its number given) as its node's newest version, approved when
    /// `approve`.
    pub fn record(&mut self, made: Made, approve: bool) {
        let node = self.nodes.entry(name_key(&made.node)).or_default();
        if approve {
            node.approved = made.primary().map(|asset| asset.path.clone());
        }
        node.versions.retain(|version| version.number != made.number);
        node.versions.push(made);
        node.versions.sort_by_key(|made| made.number);
    }
}

/// Where version `number` of `node` keeps an asset made from the file `from`: in renders/ for a
/// take (`noun`), else in references/, in the node's folder, named `<node>_take###` or `<node>_v###`
/// and then `part`: `""` for the primary asset, `_clean` for a cleaned picture's clean, or
/// `.<name>` for any other (`.latents`, a state's frame `.wall`, a call's `.call1`). A picture, video
/// or sound keeps its extension; another file is named by its part alone (`fight_take001.latents`).
pub fn asset_path(node: &str, noun: &str, number: u32, part: &str, from: &Path) -> String {
    let tag = match noun {
        "take" => format!("take{number:03}"),
        _ => format!("v{number:03}"),
    };
    let ext = from.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let media = MEDIA.contains(&ext.as_str());
    let ext = if part.starts_with('.') && !media || ext.is_empty() { String::new() } else { format!(".{ext}") };
    format!("{}/{}_{tag}{part}{ext}", folder_of(node, noun), node_dir(node))
}

/// The extensions a picture, video or sound keeps.
const MEDIA: [&str; 11] = ["png", "jpg", "jpeg", "webp", "mp4", "webm", "mov", "wav", "mp3", "flac", "m4a"];

/// A node's folder: `renders/<node>` for a node with takes, else `references/<node>`.
fn folder_of(node: &str, noun: &str) -> String {
    let base = if noun == "take" { "renders" } else { "references" };
    format!("{base}/{}", node_dir(node))
}

/// A node's folder's name: its name, without path separators.
fn node_dir(node: &str) -> String {
    node.replace(['/', '\\'], "_")
}

pub struct Folder {
    pub dir: PathBuf,
    /// Held across each write's read and swap, so no two of this Calcifer's writes lose one.
    writing: Mutex<()>,
}

impl Folder {
    /// The project in `dir`.
    pub fn open(dir: &Path) -> Result<Self, Error> {
        if !dir.join(PROJECT).is_file() {
            return Err(Error::new(
                Kind::NotFound,
                "project_not_found",
                format!("{} holds no {PROJECT}", dir.display()),
            )
            .next("cozy project new <path>"));
        }
        Ok(Self { dir: dir.to_owned(), writing: Mutex::default() })
    }

    pub fn project(&self) -> PathBuf {
        self.dir.join(PROJECT)
    }

    pub fn text(&self) -> Result<String, Error> {
        read(&self.project())
    }

    pub fn lock(&self) -> Result<Lock, Error> {
        let path = self.dir.join(LOCK);
        if !path.exists() {
            return Ok(Lock::default());
        }
        serde_json::from_str(&read(&path)?)
            .map_err(|e| Error::new(Kind::Validation, "lock_unreadable", format!("{}: {e}", path.display())))
    }

    /// Applies `change` to project.lock as it is now.
    pub fn update_lock(&self, change: impl Fn(&mut Lock)) -> Result<(), Error> {
        self.rewrite(&self.dir.join(LOCK), |text| {
            let mut lock: Lock = match text {
                "" => Lock::default(),
                text => serde_json::from_str(text)
                    .map_err(|e| Error::new(Kind::Validation, "lock_unreadable", format!("{LOCK}: {e}")))?,
            };
            change(&mut lock);
            Ok(serde_json::to_string_pretty(&lock).expect("a lock serializes") + "\n")
        })
    }

    /// `source` kept at `path` (`asset_path`), copied there whole before it is named: its asset. A
    /// file already there is never overwritten: the same bytes are this asset kept before (a landing
    /// done again), and other bytes are people's, refused.
    pub fn keep(&self, source: &Path, path: String) -> Result<Asset, Error> {
        let (at, sha256) = (self.dir.join(&path), hash(source)?);
        if at.exists() {
            return match hash(&at)? == sha256 {
                true => Ok(Asset { path, sha256 }),
                false => Err(Error::new(Kind::Conflict, "asset_name_taken", format!("{path} is there and is not what cozy made"))
                    .remedy("rename or move that file: cozy names its own files <node>_v### and <node>_take###, and never overwrites another")),
            };
        }
        let dir = at.parent().expect("an asset is in its node's folder");
        std::fs::create_dir_all(dir).map_err(|e| io_at(dir, e))?;
        let part = dir.join(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
        std::fs::copy(source, &part).map_err(|e| io_at(source, e))?;
        std::fs::rename(&part, &at).map_err(|e| io_at(&at, e))?;
        Ok(Asset { path, sha256 })
    }

    /// Removes the files cozy made at `paths` that project.lock no longer names (and project.yaml
    /// does not attach, `attached`): a version forgotten, a call continued, a run that failed.
    pub fn drop_files<'a>(&self, paths: impl IntoIterator<Item = &'a String>, attached: &BTreeSet<String>) -> Result<(), Error> {
        let lock = self.lock()?;
        let pending = lock.pending.values().map(|pending| &pending.made);
        let mut named: BTreeSet<&String> = attached.iter().collect();
        for made in lock.nodes.values().flat_map(|node| &node.versions).chain(pending) {
            named.extend(made.paths());
        }
        for (video, latents) in lock.pending.values().filter_map(|pending| pending.call.as_ref()?.from.as_ref()) {
            named.extend(std::iter::once(&video.path).chain(latents.iter().map(|asset| &asset.path)));
        }
        for path in paths.into_iter().filter(|path| !named.contains(path)) {
            match std::fs::remove_file(self.dir.join(path)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_at(&self.dir.join(path), e)),
                _ => {}
            }
        }
        Ok(())
    }

    /// A file's sha256 (`sha256:<hex>`), from its bytes.
    pub fn digest(&self, file: &str) -> Result<String, Error> {
        hash(&self.dir.join(file))
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// Forgets the lock's nodes the project no longer has (`nodes`, by name key), removing the files
    /// their versions were.
    pub fn collect_garbage(&self, nodes: &BTreeSet<String>, attached: &BTreeSet<String>) -> Result<(), Error> {
        let lock = self.lock()?;
        let gone: Vec<String> = (lock.nodes.iter())
            .filter(|(key, _)| !nodes.contains(*key))
            .flat_map(|(_, node)| node.versions.iter().flat_map(Made::paths))
            .cloned()
            .collect();
        self.update_lock(|lock| lock.nodes.retain(|key, _| nodes.contains(key)))?;
        self.drop_files(&gone, attached)
    }
}

impl Folder {
    /// Writes `change(what path holds)` over it: compare and swap on its bytes. A write that lands
    /// between the read and the swap (a person saving) is read again and the change applied to it,
    /// so that edit stands.
    fn rewrite(&self, path: &Path, change: impl Fn(&str) -> Result<String, Error>) -> Result<(), Error> {
        let _held = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let before = match path.exists() {
                true => read(path)?,
                false => String::new(),
            };
            let after = change(&before)?;
            if after == before {
                return Ok(());
            }
            let now = match path.exists() {
                true => read(path)?,
                false => String::new(),
            };
            if now == before {
                return durable::replace(path, after.as_bytes(), 0o644).map_err(io);
            }
        }
    }
}

fn read(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|e| io_at(path, e))
}

pub fn hash(path: &Path) -> Result<String, Error> {
    let mut file = std::fs::File::open(path).map_err(|e| io_at(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        match file.read(&mut buffer).map_err(|e| io_at(path, e))? {
            0 => return Ok(format!("sha256:{}", hex::encode(hasher.finalize()))),
            n => hasher.update(&buffer[..n]),
        }
    }
}

fn io(error: platform::Error) -> Error {
    Error::new(Kind::Unavailable, "project_io", error.to_string())
}

fn io_at(path: &Path, error: std::io::Error) -> Error {
    Error::new(Kind::Unavailable, "project_io", format!("{}: {error}", path.display()))
}
