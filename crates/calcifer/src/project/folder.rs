//! A project folder: `project.yaml` (people's; cozy writes it only at `cozy project new`),
//! `project.lock` (cozy's: each node's versions, what made each, which is approved, the runs still
//! making one, and what its walk still owes) and `files/<sha256>.<ext>`.
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
const FILES: &str = "files";

/// One version of a node: its file, what made it and what it was made from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Made {
    pub node: String,
    /// `files/<sha256>.<ext>`; empty while its run is pending.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// The function, models, LoRAs and options that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<MadeBy>,
    /// The upstream files it was made from; None for the clean of an attached picture, which is
    /// never out of date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<Vec<Upstream>>,
    /// Files the version wrote besides itself, by the state each is a revision of.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, String>,
    /// The run that made it.
    #[serde(default)]
    pub run: u64,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub file: String,
}

/// A run making a version, recorded before it is sent (under its idempotency key) and followed
/// until it lands: whatever stops the walk, the next one follows it and never sends it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    /// What it makes, with its run's number once sent.
    pub made: Made,
    /// The Hub it was sent for, which a picture's clean goes to too.
    #[serde(default)]
    pub hub: String,
    /// The result field holding the version (`image`, `video`).
    pub field: String,
    /// The states its `frames` are revisions of, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    pub then: Then,
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

/// A node's versions in the order they were made, and the approved one's file.
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

/// What `cozy render` asked that the project's walk has not sent yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Owed {
    /// `render all`: every node up to date, then the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Target>,
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
}

impl Lock {
    /// Whether its walk owes anything: asked and not sent, or sent and not landed.
    pub fn owes(&self) -> bool {
        self.walk.all.is_some() || !self.walk.takes.is_empty() || !self.pending.is_empty()
    }

    pub fn node(&self, node: &str) -> Option<&Node> {
        self.nodes.get(&name_key(node))
    }

    /// `node`'s version whose file has `digest`.
    pub fn made(&self, node: &str, digest: &str) -> Option<&Made> {
        self.node(node)?.versions.iter().find(|made| digest_of(&made.file) == digest)
    }

    /// Lists `made` (its `file` set) as its node's newest version, approved when `approve`.
    pub fn record(&mut self, made: Made, approve: bool) {
        let node = self.nodes.entry(name_key(&made.node)).or_default();
        if approve {
            node.approved = Some(made.file.clone());
        }
        node.versions.retain(|version| version.file != made.file);
        node.versions.push(made);
    }
}

/// The digest a file under `files/` is named by.
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
            .next("cozy project new <name>"));
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

    /// `source` kept as `files/<sha256>.<ext>`: its path relative to the project, and its digest.
    pub fn keep(&self, source: &Path) -> Result<(String, String), Error> {
        let digest = hash(source)?;
        let ext = source.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let name = format!("{FILES}/{}{ext}", digest.trim_start_matches("sha256:"));
        let path = self.dir.join(&name);
        if !path.exists() {
            std::fs::create_dir_all(self.dir.join(FILES)).map_err(|e| io_at(&self.dir.join(FILES), e))?;
            let bytes = std::fs::read(source).map_err(|e| io_at(source, e))?;
            durable::replace(&path, &bytes, 0o644).map_err(io)?;
        }
        Ok((name, digest))
    }

    /// A version's digest: from its name under `files/`, else its bytes (a file attached by hand).
    pub fn digest(&self, file: &str) -> Result<String, Error> {
        let stem = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match Path::new(file).parent() == Some(Path::new(FILES))
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

    /// The file under `files/` with this digest, relative to the project.
    pub fn find(&self, digest: &str) -> Result<String, Error> {
        let hex = digest.trim_start_matches("sha256:");
        let entries = std::fs::read_dir(self.dir.join(FILES)).into_iter().flatten().flatten();
        (entries.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .find(|name| Path::new(name).file_stem().is_some_and(|stem| stem == hex))
            .map(|name| format!("{FILES}/{name}"))
            .ok_or_else(|| Error::new(Kind::NotFound, "file_missing", format!("{FILES}/ holds no {digest}")))
    }

    /// Forgets the lock's nodes the project no longer has (`nodes`, by name key), and deletes each
    /// file under `files/` that neither a version nor what one wrote nor an attached file
    /// (`attached`, digests) names.
    pub fn collect_garbage(&self, nodes: &BTreeSet<String>, attached: &BTreeSet<String>) -> Result<(), Error> {
        self.update_lock(|lock| lock.nodes.retain(|key, _| nodes.contains(key)))?;
        let lock = self.lock()?;
        let mut keep = attached.clone();
        for made in lock.nodes.values().flat_map(|node| &node.versions) {
            keep.insert(digest_of(&made.file));
            keep.extend(made.outputs.values().cloned());
        }
        let entries = std::fs::read_dir(self.dir.join(FILES)).into_iter().flatten().flatten();
        for entry in entries {
            let name = format!("{FILES}/{}", entry.file_name().to_string_lossy());
            if entry.path().is_file() && !keep.contains(&self.digest(&name)?) {
                std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
            }
        }
        Ok(())
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
