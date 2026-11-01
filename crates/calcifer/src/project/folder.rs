//! A project folder: `project.yaml` (people's; cozy writes it only at `cozy project new`),
//! `project.lock` (cozy's: each node's versions, what made each, which is approved, the runs still
//! making one, and what its walk still owes), `assets/` (everything cozy makes: kept by content in
//! `assets/.store/<sha256>.<ext>`, and shown as a tree of links named for people, `links`), and
//! people's inputs, which cozy never writes: `references/` (what guides) and `plates/` (footage to
//! edit). `edit.mp4` is the film; `.cuts/` holds the pieces a re-cut reuses.
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
pub const ASSETS: &str = "assets";
/// Where assets are kept, by content; project.lock names them here.
const STORE: &str = "assets/.store";
/// The folders `cozy project new` makes for people's own files: a convention, never enforced.
pub const INPUTS: [&str; 2] = ["references", "plates"];
/// A version's primary asset, the one its node reads as: its video, else its clean (a cleaned
/// picture's), else its image.
const PRIMARY: [&str; 3] = ["video", "clean", "image"];

/// One version of a node (a take, for a shot or its upscale): its assets, what made it and what it
/// was made from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Made {
    pub node: String,
    /// By the run output's name (`video`, `image`, `latents`; a cleaned picture's `clean`):
    /// `assets/.store/<sha256>.<ext>`. Empty while its run is pending.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, String>,
    /// The function, models, LoRAs and options that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<MadeBy>,
    /// What it was made from; None for the clean of an attached picture, which is never out of date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<Vec<Upstream>>,
    /// The frames it wrote for world states, by state: `assets/.store/…`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub writes: BTreeMap<String, String>,
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
    pub fn primary(&self) -> Option<&String> {
        PRIMARY.iter().find_map(|name| self.assets.get(*name))
    }
}

/// What a version read: a node's asset (a state's revision, with `state`), or a file of the
/// project's that is no node's (`<node>.<field>`), by digest.
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
/// shot), and the video and latents (`assets/…`) of the call before it, which this one continues.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Call {
    pub index: usize,
    pub ends: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<(String, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<(String, Option<String>)>,
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

/// A node's versions in the order they were made, and the approved one's primary asset.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Node {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<String>,
    #[serde(default)]
    pub versions: Vec<Made>,
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

    /// `node`'s version whose primary asset has `digest`.
    pub fn made(&self, node: &str, digest: &str) -> Option<&Made> {
        self.node(node)?.versions.iter().find(|made| made.primary().is_some_and(|asset| digest_of(asset) == digest))
    }

    /// Lists `made` (its assets kept) as its node's newest version, approved when `approve`: its
    /// number, 1 first.
    pub fn record(&mut self, made: Made, approve: bool) -> u32 {
        let node = self.nodes.entry(name_key(&made.node)).or_default();
        if approve {
            node.approved = made.primary().cloned();
        }
        node.versions.retain(|version| version.primary() != made.primary());
        node.versions.push(made);
        node.versions.len() as u32
    }
}

/// The digest an asset (`assets/.store/<sha256>.<ext>`) is named by.
pub fn digest_of(file: &str) -> String {
    let stem = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    format!("sha256:{stem}")
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

    /// `source` kept as `assets/.store/<sha256>.<ext>`: its path relative to the project, and its digest.
    pub fn keep(&self, source: &Path) -> Result<(String, String), Error> {
        let digest = hash(source)?;
        let ext = source.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let name = format!("{STORE}/{}{ext}", digest.trim_start_matches("sha256:"));
        let path = self.dir.join(&name);
        if !path.exists() {
            std::fs::create_dir_all(self.dir.join(STORE)).map_err(|e| io_at(&self.dir.join(STORE), e))?;
            let bytes = std::fs::read(source).map_err(|e| io_at(source, e))?;
            durable::replace(&path, &bytes, 0o644).map_err(io)?;
        }
        Ok((name, digest))
    }

    /// A file's digest: from its name when it is an asset, else its bytes (a file of people's).
    pub fn digest(&self, file: &str) -> Result<String, Error> {
        let stem = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match Path::new(file).parent() == Some(Path::new(STORE))
            && stem.len() == 64
            && stem.bytes().all(|b| b.is_ascii_hexdigit())
        {
            true => Ok(format!("sha256:{stem}")),
            false => hash(&self.dir.join(file)),
        }
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// The asset with this digest, relative to the project.
    pub fn find(&self, digest: &str) -> Result<String, Error> {
        let hex = digest.trim_start_matches("sha256:");
        let entries = std::fs::read_dir(self.dir.join(STORE)).into_iter().flatten().flatten();
        (entries.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .find(|name| Path::new(name).file_stem().is_some_and(|stem| stem == hex))
            .map(|name| format!("{STORE}/{name}"))
            .ok_or_else(|| Error::new(Kind::NotFound, "asset_missing", format!("{STORE}/ holds no {digest}")))
    }

    /// Forgets the lock's nodes the project no longer has (`nodes`, by name key), and deletes each
    /// asset that no version's assets or writes, no pending call and no attached file (`attached`,
    /// digests) names.
    pub fn collect_garbage(&self, nodes: &BTreeSet<String>, attached: &BTreeSet<String>) -> Result<(), Error> {
        self.update_lock(|lock| lock.nodes.retain(|key, _| nodes.contains(key)))?;
        let lock = self.lock()?;
        let mut keep = attached.clone();
        let pending = lock.pending.values().map(|pending| &pending.made);
        for made in lock.nodes.values().flat_map(|node| &node.versions).chain(pending) {
            keep.extend(made.assets.values().chain(made.writes.values()).map(|asset| digest_of(asset)));
        }
        for (video, latents) in lock.pending.values().filter_map(|pending| pending.call.as_ref()?.from.as_ref()) {
            keep.insert(digest_of(video));
            keep.extend(latents.iter().map(|asset| digest_of(asset)));
        }
        let entries = std::fs::read_dir(self.dir.join(STORE)).into_iter().flatten().flatten();
        for entry in entries {
            let name = format!("{STORE}/{}", entry.file_name().to_string_lossy());
            if entry.path().is_file() && !keep.contains(&self.digest(&name)?) {
                std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
            }
        }
        Ok(())
    }
}

/// The tree of links in `assets/` for people (Paul 11-01), from the store: one folder per node,
/// `<node>/<node>_take###.<ext>` (a shot's and an upscale's takes) or `<node>_v###.<ext>` (every other
/// node's versions) for each one's primary asset, a cleaned picture's clean beside it as
/// `<node>_v###_clean.<ext>`, and `<node>_approved.<ext>` for the approved one. Symlinks into
/// `.store`; on Windows, where a symlink needs a privilege, hard links (one volume).
pub struct Linked<'a> {
    /// Each node as project.yaml names it, and its noun (`take`, `version`).
    pub nodes: Vec<(&'a str, &'a str)>,
}

/// `node`'s version `number`'s links: (its path in the project, the store path it names).
pub fn links(node: &str, noun: &str, number: u32, made: &Made) -> Vec<(String, String)> {
    let dir = node.replace(['/', '\\'], "_");
    let tag = match noun {
        "take" => format!("take{number:03}"),
        _ => format!("v{number:03}"),
    };
    let named = |suffix: &str, asset: &String| {
        let ext = Path::new(asset).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (format!("{ASSETS}/{dir}/{dir}_{tag}{suffix}{ext}"), asset.clone())
    };
    match made.assets.get("clean") {
        Some(clean) => std::iter::once(named("_clean", clean)).chain(made.assets.get("image").map(|image| named("", image))).collect(),
        None => made.primary().map(|asset| named("", asset)).into_iter().collect(),
    }
}

impl Folder {
    /// The tree of links (`Linked`) made again from project.lock: each link it should hold made or
    /// pointed again, each of its own it should not removed; anything else in it is left alone.
    pub fn relink(&self, linked: &Linked) -> Result<(), Error> {
        let lock = self.lock()?;
        let mut wanted: BTreeMap<PathBuf, String> = BTreeMap::new();
        for (node, noun) in &linked.nodes {
            let Some(versions) = lock.node(node) else { continue };
            for (n, made) in versions.versions.iter().enumerate() {
                wanted.extend(links(node, noun, n as u32 + 1, made).into_iter().map(|(link, asset)| (link.into(), asset)));
            }
            if let Some(approved) = &versions.approved {
                let dir = node.replace(['/', '\\'], "_");
                let ext = Path::new(approved).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                wanted.insert(format!("{ASSETS}/{dir}/{dir}_approved{ext}").into(), approved.clone());
            }
        }
        let ours = |dir: &str, name: &str| {
            let rest = name.strip_prefix(&format!("{dir}_")).unwrap_or_default();
            let tag = rest.split('.').next().unwrap_or_default();
            let numbered = |tag: &str| tag.len() == 3 && tag.bytes().all(|b| b.is_ascii_digit());
            tag == "approved"
                || tag.strip_prefix("take").is_some_and(numbered)
                || tag.strip_prefix('v').map(|tag| tag.strip_suffix("_clean").unwrap_or(tag)).is_some_and(numbered)
        };
        for dir in std::fs::read_dir(self.dir.join(ASSETS)).into_iter().flatten().flatten() {
            let name = dir.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !dir.path().is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(dir.path()).into_iter().flatten().flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                let at = Path::new(ASSETS).join(&name).join(&file);
                if ours(&name, &file) && !wanted.contains_key(&at) {
                    std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
                }
            }
            let _ = std::fs::remove_dir(dir.path());
        }
        for (link, asset) in &wanted {
            let (at, store) = (self.dir.join(link), self.dir.join(asset));
            if same(&at, &store) {
                continue;
            }
            let parent = at.parent().expect("a link is in a node's folder");
            std::fs::create_dir_all(parent).map_err(|e| io_at(parent, e))?;
            let part = parent.join(format!(".{}.part", link.file_name().expect("a name").to_string_lossy()));
            let _ = std::fs::remove_file(&part);
            #[cfg(unix)]
            let made = std::os::unix::fs::symlink(Path::new("..").join(".store").join(store.file_name().expect("a name")), &part);
            #[cfg(windows)]
            let made = std::fs::hard_link(&store, &part);
            made.and_then(|()| std::fs::rename(&part, &at)).map_err(|e| io_at(&at, e))?;
        }
        Ok(())
    }
}

/// Whether the link at `at` already names `store`.
fn same(at: &Path, store: &Path) -> bool {
    #[cfg(unix)]
    {
        std::fs::read_link(at).ok().and_then(|target| at.parent().map(|dir| dir.join(target))).and_then(|t| t.canonicalize().ok())
            == store.canonicalize().ok()
    }
    // A hard link cannot be told from its file cheaply: it is made again.
    #[cfg(windows)]
    {
        let _ = (at, store);
        false
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
