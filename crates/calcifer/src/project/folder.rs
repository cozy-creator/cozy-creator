//! A project folder (Paul, 11-01): `project.yaml` (people's; cozy writes it only at `cozy project
//! new`), `project.lock` (cozy's: each node's versions, what made each, which is approved, the files
//! the project reads, the runs still making one, and what its walk still owes), `assets/`, and the
//! cut (`edit.mp4`). Every file the project uses is in `assets/<sha256>.<ext>`, flat: each take and
//! version, clean and latents cozy makes (real files), and each file project.yaml names, taken in on
//! first use: a copy-on-write clone where the filesystem has them, else a link to the original
//! (`Method`). Names live only in project.yaml and the lock.
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
/// Where every file the project uses is kept, by content: `assets/<sha256>.<ext>`.
pub const ASSETS: &str = "assets";
/// A version's primary asset, the one its node reads as: its video, else its clean (a cleaned
/// picture's), else its image.
const PRIMARY: [&str; 3] = ["video", "clean", "image"];

/// A file the project uses: its path in the project (`assets/<sha256>.<ext>`), its sha256, and for a
/// video its shape, probed once (`Folder::shape_videos`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<Shape>,
}

/// A video's width and height (which decide whether its shot is upscaled), frame rate (a take at
/// another rate than the project's is retimed when cut) and frame count.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub width: u64,
    pub height: u64,
    pub fps: f64,
    pub frames: u64,
}

/// The extensions of the files whose shape is probed.
const VIDEOS: [&str; 5] = ["mp4", "mov", "m4v", "webm", "mkv"];

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

/// A node's kind, its versions by number, the approved one's primary asset's path, and the last
/// number given (numbers are never given again).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<String>,
    #[serde(default)]
    pub versions: Vec<Made>,
    #[serde(default)]
    pub numbered: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Lock {
    /// By name key. A node project.yaml no longer names is kept, its versions with it, until it is
    /// renamed (`cozy project rename`).
    #[serde(default)]
    pub nodes: BTreeMap<String, Node>,
    /// The files project.yaml names, by the path written there: each one's asset as last read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, Input>,
    /// By idempotency key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default, skip_serializing_if = "Owed::is_empty")]
    pub walk: Owed,
}

/// A file project.yaml names, as last taken in: its asset, how it was made, where the original is
/// (absolute), and the size and time it had then, which spare hashing it again while they hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    #[serde(flatten)]
    pub asset: Asset,
    pub method: Method,
    pub original: PathBuf,
    pub size: u64,
    pub modified_ms: i64,
}

/// How a file project.yaml names became an asset, tried in this order (Paul 11-01): a reflink (a
/// copy-on-write clone: its own file, no space, whatever becomes of the original), else a symlink to
/// the original, else (Windows without the right) a hard link on the same volume, else a copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Reflink,
    Symlink,
    Hardlink,
    Copy,
}

impl Method {
    /// Whether the asset is the original itself, so it goes when the original moves and changes when
    /// it does: a link's. A reflink or copy is the project's own.
    pub fn linked(self) -> bool {
        matches!(self, Method::Symlink | Method::Hardlink)
    }
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

    /// Lists `made` (its assets kept, its number given) as its node's (of `kind`) newest version,
    /// approved when `approve`.
    pub fn record(&mut self, made: Made, kind: &str, approve: bool) {
        let node = self.nodes.entry(name_key(&made.node)).or_default();
        node.kind = kind.into();
        if approve {
            node.approved = made.primary().map(|asset| asset.path.clone());
        }
        node.versions.retain(|version| version.number != made.number);
        node.versions.push(made);
        node.versions.sort_by_key(|made| made.number);
    }
}

pub struct Folder {
    pub dir: PathBuf,
    /// cozy's tools folder, where its ffprobe is (else PATH's).
    pub tools: PathBuf,
    /// Held across each write's read and swap, so no two of this Calcifer's writes lose one.
    writing: Mutex<()>,
}

impl Folder {
    /// The project in `dir`, its videos probed with the ffprobe in `tools` (else PATH's).
    pub fn open(dir: &Path, tools: PathBuf) -> Result<Self, Error> {
        if !dir.join(PROJECT).is_file() {
            return Err(Error::new(
                Kind::NotFound,
                "project_not_found",
                format!("{} holds no {PROJECT}", dir.display()),
            )
            .next("cozy project new <path>"));
        }
        Ok(Self { dir: dir.to_owned(), tools, writing: Mutex::default() })
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

    /// `source`, a file a run saved, kept as an asset: copied whole before it is named, once per
    /// content.
    pub fn keep(&self, source: &Path) -> Result<Asset, Error> {
        let sha256 = hash(source)?;
        let path = asset_of(&sha256, source);
        self.store(&path, |part| std::fs::copy(source, part).map(drop))?;
        Ok(Asset { path, sha256, video: None })
    }

    /// The file project.yaml names as `file` (relative to the folder, absolute, or `~/…`), taken in as
    /// an asset `assets/<sha256>.<ext>` (`Method`): hashed again unless `known`, as last taken in,
    /// still has its size and time. None when the original is not there and the asset was a link to
    /// it; a reflink or a copy is the project's own and stays.
    pub fn take_in(&self, file: &str, known: Option<&Input>) -> Result<Option<Input>, Error> {
        let original = std::path::absolute(self.path(file)).map_err(|e| io_at(&self.path(file), e))?;
        let Ok(meta) = std::fs::metadata(&original) else {
            let owned = known.filter(|known| !known.method.linked() && self.dir.join(&known.asset.path).is_file());
            return Ok(owned.cloned());
        };
        let modified_ms = (meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()))
            .map_or(0, |at| at.as_millis() as i64);
        let same = known.filter(|known| known.original == original && known.size == meta.len() && known.modified_ms == modified_ms);
        if let Some(known) = same.filter(|known| self.dir.join(&known.asset.path).is_file()) {
            return Ok(Some(known.clone()));
        }
        let sha256 = hash(&original)?;
        let path = asset_of(&sha256, &original);
        let at = self.dir.join(&path);
        let kept = known.filter(|known| known.asset.sha256 == sha256 && known.original == original && at.is_file());
        let method = match kept {
            Some(known) => known.method,
            None => {
                let dir = self.dir.join(ASSETS);
                std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
                let part = dir.join(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
                let _ = std::fs::remove_file(&part);
                let made = (durable::reflink(&original, &part).map(|()| Method::Reflink))
                    .or_else(|_| durable::symlink(&original, &part).map(|()| Method::Symlink).map_err(std::io::Error::other))
                    .or_else(|_| std::fs::hard_link(&original, &part).map(|()| Method::Hardlink))
                    .or_else(|_| std::fs::copy(&original, &part).map(|_| Method::Copy));
                made.and_then(|method| std::fs::rename(&part, &at).map(|()| method)).map_err(|e| io_at(&at, e))?
            }
        };
        Ok(Some(Input { asset: Asset { path, sha256, video: None }, method, original, size: meta.len(), modified_ms }))
    }

    /// `path` (an asset's) made by `write` at a part beside it and then named, unless it is there.
    fn store(&self, path: &str, write: impl Fn(&Path) -> std::io::Result<()>) -> Result<(), Error> {
        let at = self.dir.join(path);
        if at.is_file() {
            return Ok(());
        }
        let dir = self.dir.join(ASSETS);
        std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
        let part = dir.join(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
        let _ = std::fs::remove_file(&part);
        write(&part).and_then(|()| std::fs::rename(&part, &at)).map_err(|e| io_at(&at, e))
    }

    /// A file as project.yaml or the lock writes it: relative to the folder, absolute, or `~/…`.
    pub fn path(&self, file: &str) -> PathBuf {
        match file.strip_prefix("~/").zip(platform::paths::login_home()) {
            Some((rest, home)) => home.join(rest),
            None => self.dir.join(file),
        }
    }

    /// Every video `lock` records (a version's asset, a file project.yaml names) with no shape yet,
    /// probed once and recorded, in `lock` and in project.lock. With no ffprobe here yet (the walk
    /// fetches one before it cuts), they wait for the next read.
    pub fn shape_videos(&self, lock: &mut Lock) -> Result<(), Error> {
        let unshaped = |asset: &Asset| {
            let ext = Path::new(&asset.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            asset.video.is_none() && ext.is_some_and(|ext| VIDEOS.contains(&ext.as_str()))
        };
        let versions = lock.nodes.values().flat_map(|node| &node.versions).flat_map(|made| made.assets.values());
        let wanted: BTreeSet<&String> =
            lock.inputs.values().map(|input| &input.asset).chain(versions).filter(|a| unshaped(a)).map(|a| &a.path).collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let Some((_, ffprobe)) = platform::tools::ffmpeg(&self.tools, &crate::tools::path()) else { return Ok(()) };
        let shapes: BTreeMap<String, Shape> = (wanted.into_iter())
            .filter_map(|path| Some((path.clone(), super::video::shape(&ffprobe, &self.dir.join(path))?)))
            .collect();
        let fill = |lock: &mut Lock| {
            let versions = lock.nodes.values_mut().flat_map(|node| &mut node.versions).flat_map(|made| made.assets.values_mut());
            for asset in lock.inputs.values_mut().map(|input| &mut input.asset).chain(versions) {
                asset.video = asset.video.or_else(|| shapes.get(&asset.path).copied());
            }
        };
        if shapes.is_empty() {
            return Ok(());
        }
        fill(lock);
        self.update_lock(fill)
    }

    /// Deletes each asset nothing in project.lock names: no version (of a node project.yaml names or
    /// not), no pending run or call, no input.
    pub fn collect_garbage(&self) -> Result<(), Error> {
        let lock = self.lock()?;
        let pending = lock.pending.values().map(|pending| &pending.made);
        let mut named: BTreeSet<&String> = lock.inputs.values().map(|input| &input.asset.path).collect();
        for made in lock.nodes.values().flat_map(|node| &node.versions).chain(pending) {
            named.extend(made.paths());
        }
        for (video, latents) in lock.pending.values().filter_map(|pending| pending.call.as_ref()?.from.as_ref()) {
            named.extend(std::iter::once(&video.path).chain(latents.iter().map(|asset| &asset.path)));
        }
        for entry in std::fs::read_dir(self.dir.join(ASSETS)).into_iter().flatten().flatten() {
            let path = format!("{ASSETS}/{}", entry.file_name().to_string_lossy());
            let ours = entry.file_type().is_ok_and(|kind| kind.is_file() || kind.is_symlink());
            if ours && !path.contains("/.") && !named.contains(&path) {
                std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
            }
        }
        Ok(())
    }
}

/// An asset's path: `assets/<hex of sha256>.<the source's extension>`.
fn asset_of(sha256: &str, source: &Path) -> String {
    let ext = source.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
    format!("{ASSETS}/{}{ext}", sha256.trim_start_matches("sha256:"))
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
