//! A project folder: `project.yaml` (people's; cozy writes it at `cozy project new`, and one line of it at `approve` and `unpin`),
//! `project.lock` (cozy's: each file, each asset's versions, what made each, the runs still making one, and what
//! its walk still owes), `files/`, and the cuts at its root. Every file the project uses is a real file in
//! `files/<sha256>.<ext>`, flat. project.yaml names one only by a hex prefix of its hash (`Lock::matching`),
//! so the lock names nothing of this computer and travels with project.yaml.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use infer::MatcherType;
use platform::{durable, paths::Home};
use protocol::{
    calcifer::On,
    error::{Error, Kind},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tensorfs::err::{Code, Refusal};

use super::graph::name_key;

pub const PROJECT: &str = "project.yaml";
pub const LOCK: &str = "project.lock";
/// Where every file the project uses is kept, by content: `files/<sha256>.<ext>`.
pub const FILES: &str = "files";
/// Where a run's outputs land before they are kept in `files/`.
pub const RUNS: &str = ".runs";
/// The lock's format.
const FORMAT: u32 = 2;

/// The most a URL may give: far past any picture, and a shared project can't fill a disk with it.
const URL_MOST: usize = 64 << 20;
/// How long a URL may take. Measured progress can't judge this one: a stranger's server can drip a
/// byte at a time for ever, so this is the last resort, and it leaves room for `URL_MOST` over a slow line.
const URL_LONGEST: Duration = Duration::from_secs(300);
/// What an import kept: its file, and the field that names one of its type (`image`, `video`, `audio`).
pub struct Kept {
    pub file: File,
    pub field: &'static str,
    /// Its bytes were in `files/` already, so nothing was copied.
    pub already: bool,
}

/// How many hex characters of its hash cozy prints for a file, for project.yaml.
const PREFIX: usize = 12;

/// A file the project uses: its path in the project (`files/<sha256>.<ext>`), its sha256, and what is
/// measured of it once: a picture's or video's size, a video's rate and frames, a picture's alpha (the
/// share of it fully transparent).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: u64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub fps: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub frames: u64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub alpha: f64,
    /// A file too big for one object at the Hub: the objects it goes as, in order. Its first push lists them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<tensorfs::format::ObjectRef>,
}

impl File {
    /// What an operation is given of it: no path, since nothing of this computer goes.
    pub fn record(&self) -> Value {
        serde_json::json!({"sha256": self.sha256, "width": self.width, "height": self.height, "fps": self.fps, "frames": self.frames, "alpha": self.alpha})
    }

    fn extension(&self) -> String {
        Path::new(&self.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
    }

    pub fn is_video(&self) -> bool {
        VIDEOS.contains(&self.extension().as_str())
    }

    /// The role that pins it, by what it is.
    pub fn field(&self) -> &'static str {
        match self.extension().as_str() {
            _ if self.is_video() => "video",
            "wav" | "mp3" | "flac" | "ogg" | "m4a" | "aac" => "audio",
            _ => "image",
        }
    }

    /// Not measured yet, and one that can be.
    fn unmeasured(&self) -> bool {
        self.width == 0 && (self.is_video() || PICTURES.contains(&self.extension().as_str()))
    }
}

/// The extensions of the files that are measured.
const VIDEOS: [&str; 5] = ["mp4", "mov", "m4v", "webm", "mkv"];
const PICTURES: [&str; 5] = ["png", "jpg", "jpeg", "webp", "avif"];

/// A version's files: by output name, each one's sha256.
pub type Files = BTreeMap<String, String>;

/// One result of an operation: its number, its files, the key it was made under, and what made it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Version {
    /// 1 first, never reused.
    #[serde(default)]
    pub number: u32,
    /// The memo key's inputs. Empty for one whose inputs can't be known: it never reads as current.
    #[serde(default)]
    pub inputs: String,
    /// The key's sample: the seed Calcifer drew. None: its function takes no seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// And the start of another take that it keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<protocol::project::Kept>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: Files,
    /// The frames it wrote, by the name key of the image each is.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub writes: BTreeMap<String, Files>,
    /// A digest per field of its record, to say which changed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digests: BTreeMap<String, String>,
    /// What it read: for people, never for a decision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<ReadFrom>,
    /// The function, models, LoRAs and options that made it. Outside the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<MadeBy>,
    /// The runs that made it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub execution_ms: u64,
    /// What each of those runs was asked: a function and its exact input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invocations: Vec<Value>,
    /// What its function said of it, and what was left out of its request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// An asset a version read, and the files that were.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadFrom {
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: Files,
}

/// What made a version: `with` as `org/package/function@release`, and its run's model choices and
/// options as the file gave them.
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

impl Version {
    /// The file people mean by it: its video, else its picture, else its sound, else its first.
    pub fn main(&self) -> Option<&String> {
        main_of(&self.files)
    }
}

/// The file people mean by a set of files.
pub fn main_of(files: &Files) -> Option<&String> {
    ["video", "image", "audio"].iter().find_map(|name| files.get(*name)).or_else(|| files.values().next())
}

/// A run making a version, recorded before it is sent (under its idempotency key) and followed
/// until it lands: whatever stops the walk, the next one follows it and never sends it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    /// The asset it makes, as the file names it.
    pub asset: String,
    /// What it makes.
    pub version: Version,
    /// Its run's number, once sent.
    #[serde(default)]
    pub run: u64,
    /// Pinned in project.yaml when it lands (`--approve`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approve: bool,
    /// The images it was asked to write, in order, each with where: its result's `written` in that order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<Written>,
    /// Where it goes, and each call after this one of the operation's list.
    pub target: Target,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rest: Vec<Invocation>,
    /// What each call before this one saved, for a later one that reads it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<Files>,
}

/// One call an adapter answered: a function of its package, and its exact input.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Invocation {
    pub function: String,
    #[serde(default)]
    pub input: Map<String, Value>,
}

/// An image a run writes: its name, and where in the video if the function doesn't.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub name: String,
    pub at_s: f64,
}

/// An asset's versions by number, the last number given (numbers are never given again), and the seed of a run that failed,
/// kept for the next try on those inputs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Asset {
    /// Its name as the file wrote it when it was made.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub versions: Vec<Version>,
    #[serde(default)]
    pub numbered: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tried: Option<Tried>,
}

/// The seed last sent for `inputs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tried {
    pub inputs: String,
    pub seed: u64,
}

impl Asset {
    pub fn version(&self, number: u32) -> Option<&Version> {
        self.versions.iter().find(|version| version.number == number)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Lock {
    pub format: u32,
    /// Every file, by sha256.
    #[serde(default)]
    pub files: BTreeMap<String, File>,
    /// By name key. An asset project.yaml no longer names is kept, its versions with it, until it is
    /// renamed or reclaimed.
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
    /// By idempotency key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    /// What `cozy run`s left here: they belong to no asset, and no memo holds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<RunFile>,
    /// Each package's own tags, by the name its memo knows it by, as last read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reserved: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Owed::is_empty")]
    pub walk: Owed,
}

/// One file a `cozy run` left in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunFile {
    pub run: u64,
    pub function: String,
    pub output: String,
    pub sha256: String,
}

impl Default for Lock {
    fn default() -> Self {
        Self { format: FORMAT, files: BTreeMap::new(), assets: BTreeMap::new(), pending: BTreeMap::new(), runs: vec![], reserved: BTreeMap::new(), walk: Owed::default() }
    }
}

/// What a render asked that the project's walk has not sent yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Owed {
    /// Where what it brings up goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// `render all`: every asset with no state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    /// What it sent or tried this walk, made or failed, by name key: each is tried once.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sent: BTreeSet<String>,
    /// New versions of named assets, one each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes: Vec<Take>,
}

fn is_zero<T: Default + PartialEq>(n: &T) -> bool {
    *n == T::default()
}

fn is_zero_f(n: &f64) -> bool {
    *n == 0.0
}

impl Owed {
    pub fn is_empty(&self) -> bool {
        !self.all && self.target.is_none() && self.sent.is_empty() && self.takes.is_empty()
    }
}

/// The machines a version may be made on, and the Hub its function is read from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub on: Vec<On>,
    pub hub: String,
}

/// One new version of `asset`, sent under `key`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Take {
    pub key: String,
    pub asset: String,
    #[serde(flatten)]
    pub target: Target,
    /// Pinned when it lands (`--approve`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approve: bool,
    /// A new seed even with a state (`--new`, `-n`): no such take is a run a state already answers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
    /// It keeps the start of an earlier take (`--take`, `--in`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keeps: Option<Keeps>,
}

/// The take a new take keeps the start of, and where it stops keeping it: None is its end.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keeps {
    pub take: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_s: Option<f64>,
}

impl Lock {
    /// The files whose hash starts with `prefix`.
    pub fn matching(&self, prefix: &str) -> Vec<&File> {
        self.files.values().filter(|file| hex_of(&file.sha256).starts_with(prefix)).collect()
    }

    /// Whether its walk owes anything: asked and not sent, or sent and not landed.
    pub fn owes(&self) -> bool {
        self.walk.all || !self.walk.takes.is_empty() || !self.pending.is_empty()
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.get(&name_key(name))
    }

    pub fn asset_mut(&mut self, name: &str, kind: &str) -> &mut Asset {
        let asset = self.assets.entry(name_key(name)).or_default();
        asset.name = name.to_owned();
        if !kind.is_empty() {
            asset.kind = kind.to_owned();
        }
        asset
    }

    /// `name`'s next number, given now: one past every number it gave.
    pub fn number(&mut self, name: &str, kind: &str) -> u32 {
        let asset = self.asset_mut(name, kind);
        asset.numbered = asset.numbered.max(asset.versions.iter().map(|version| version.number).max().unwrap_or(0)) + 1;
        asset.numbered
    }

    /// Every sha256 a version or pending run of a kept asset names; `keep` says which assets are kept, by key.
    pub fn named(&self, keep: impl Fn(&str) -> bool) -> BTreeSet<&str> {
        let versions = (self.assets.iter().filter(|(key, _)| keep(key))).flat_map(|(_, asset)| &asset.versions);
        let pending = self.pending.values().map(|pending| &pending.version);
        versions
            .chain(pending)
            .flat_map(|version| version.files.values().chain(version.writes.values().flat_map(BTreeMap::values)))
            .map(String::as_str)
            .collect()
    }
}

/// Held across each write's read and swap, so no two of this Calcifer's writes lose one.
static WRITING: Mutex<()> = Mutex::new(());
/// Held while a file is kept and while files are deleted, so one isn't deleted between.
static COLLECTING: Mutex<()> = Mutex::new(());

pub struct Folder {
    pub dir: PathBuf,
    /// cozy's tools folder, where its ffprobe is (else PATH's).
    pub tools: PathBuf,
}

impl Folder {
    /// The project in `dir`, whichever part of Calcifer has it open: for what lands from outside a walk.
    pub fn at(dir: &Path) -> Self {
        Self { dir: dir.to_owned(), tools: PathBuf::new() }
    }

    /// The project in `dir`, its files measured with the ffprobe in `home`'s tools (else PATH's).
    pub fn open(dir: &Path, home: &Home) -> Result<Self, Error> {
        if !dir.join(PROJECT).is_file() {
            return Err(Error::new(
                Kind::NotFound,
                "project_not_found",
                format!("{} holds no {PROJECT}", dir.display()),
            )
            .next("cozy project new <path>"));
        }
        Ok(Self { dir: dir.to_owned(), tools: home.tools() })
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
        parse_lock(&read(&path)?)
    }

    /// Applies `change` to project.lock as it is now.
    pub fn update_lock(&self, change: impl Fn(&mut Lock)) -> Result<(), Error> {
        self.rewrite(&self.dir.join(LOCK), |text| {
            let mut lock: Lock = match text {
                "" => Lock::default(),
                text => parse_lock(text)?,
            };
            change(&mut lock);
            Ok(serde_json::to_string_pretty(&lock).expect("a lock serializes") + "\n")
        })
    }

    /// Writes one change to project.yaml: `change` is given what it holds this moment, and a save that lands first is read again,
    /// so that edit stands. The only write of it but `cozy project new`.
    pub fn edit_project(&self, change: impl Fn(&str) -> Result<String, Error>) -> Result<(), Error> {
        self.rewrite(&self.project(), change)
    }

    /// `source`, a file a run saved, kept: copied whole before it is named, once per content, and in the lock's table.
    pub fn keep(&self, source: &Path) -> Result<String, Error> {
        let _held = COLLECTING.lock().unwrap_or_else(|e| e.into_inner());
        let sha256 = hash(source)?;
        let path = file_of(&sha256, source);
        self.store(&path, |part| std::fs::copy(source, part).map(drop))?;
        self.tabled(path, &sha256)?;
        Ok(sha256)
    }

    /// `source`, a file Calcifer made beside the project, moved into `files/` under its hash, and in the lock's table.
    pub fn take(&self, source: &Path) -> Result<String, Error> {
        let _held = COLLECTING.lock().unwrap_or_else(|e| e.into_inner());
        let sha256 = hash(source)?;
        let path = file_of(&sha256, source);
        if !self.store(&path, |part| std::fs::rename(source, part))? {
            let _ = std::fs::remove_file(source);
        }
        self.tabled(path, &sha256)?;
        Ok(sha256)
    }

    /// What a run landed at `dest`, of `digest`, kept in `files/` by hash: a hard link where the filesystem allows. Listed as
    /// that run's, in place of what an earlier revision of that output was, which goes if nothing else names it. Answers its path.
    pub fn adopt(&self, dest: &Path, digest: &str, made: RunFile) -> Result<PathBuf, Error> {
        let (path, old) = {
            let _held = COLLECTING.lock().unwrap_or_else(|e| e.into_inner());
            let path = file_of(digest, dest);
            let at = self.dir.join(&path);
            if !at.is_file() {
                let dir = self.dir.join(FILES);
                std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
                if std::fs::hard_link(dest, &at).is_err() {
                    self.store(&path, |part| std::fs::copy(dest, part).map(drop))?;
                }
            }
            self.tabled(path.clone(), digest)?;
            let old = std::cell::RefCell::new(vec![]);
            self.update_lock(|lock| {
                let earlier = lock.runs.iter().position(|row| row.run == made.run && row.output == made.output);
                match earlier {
                    Some(at) => *old.borrow_mut() = vec![std::mem::replace(&mut lock.runs[at], made.clone()).sha256],
                    None => lock.runs.push(made.clone()),
                }
            })?;
            (self.path(&path), old.into_inner())
        };
        self.drop_unused(&old)?;
        Ok(path)
    }

    /// The file at `path` in the lock's table, unless it is there.
    fn tabled(&self, path: String, sha256: &str) -> Result<(), Error> {
        let file = File { path, sha256: sha256.to_owned(), ..Default::default() };
        self.update_lock(|lock| {
            lock.files.entry(sha256.to_owned()).or_insert_with(|| file.clone());
        })
    }

    /// The file at `source` (an absolute path, or an `http(s)` URL) kept as a real file
    /// `files/<sha256>.<ext>`, its type read from its bytes, and recorded in project.lock. Err: why not.
    pub async fn import(&self, source: &str) -> Result<Kept, String> {
        if is_url(source) {
            let bytes = fetch_url(source).await?;
            let (kind, field) = media(infer::get(&bytes))?;
            let sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
            let path = file_named(&sha256, kind.extension());
            return self.recorded(path, sha256, field, |part| std::fs::write(part, &bytes));
        }
        let path = Path::new(source);
        if !path.is_absolute() {
            return Err("it is neither an absolute path nor an http(s) URL".into());
        }
        // A file can be large: this worker's other tasks move on while it is copied.
        tokio::task::block_in_place(|| {
            let mut head = Vec::new();
            std::fs::File::open(path).and_then(|file| file.take(8192).read_to_end(&mut head)).map_err(|e| e.to_string())?;
            let (kind, field) = media(infer::get(&head))?;
            let sha256 = hash(path).map_err(|e| e.message)?;
            let at = file_named(&sha256, kind.extension());
            self.recorded(at, sha256, field, |part| std::fs::copy(path, part).map(drop))
        })
    }

    /// A source for a run to read, kept nowhere: a file where it is, or a URL's bytes where runs land. With the field that names it.
    pub async fn source(&self, source: &str) -> Result<(PathBuf, &'static str), String> {
        if is_url(source) {
            let bytes = fetch_url(source).await?;
            let (kind, field) = media(infer::get(&bytes))?;
            let dir = self.dir.join(RUNS);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let at = dir.join(format!("{}.{}", hex::encode(Sha256::digest(&bytes)), kind.extension()));
            std::fs::write(&at, &bytes).map_err(|e| e.to_string())?;
            return Ok((at, field));
        }
        let path = Path::new(source);
        if !path.is_absolute() {
            return Err("it is neither an absolute path nor an http(s) URL".into());
        }
        let mut head = Vec::new();
        std::fs::File::open(path).and_then(|file| file.take(8192).read_to_end(&mut head)).map_err(|e| e.to_string())?;
        let (_, field) = media(infer::get(&head))?;
        Ok((path.to_owned(), field))
    }

    /// The share of the picture at `path` that is fully transparent; 0 with no ffmpeg to say.
    pub fn transparent(&self, path: &Path) -> f64 {
        let Some((found, ffprobe)) = platform::tools::ffmpeg(&self.tools, &crate::tools::path()) else { return 0.0 };
        super::tools::measured(&found.path, &ffprobe, path, false).map_or(0.0, |file| file.alpha)
    }

    /// An import's bytes at `path`, written by `write` unless they are there, named in project.lock.
    fn recorded(
        &self,
        path: String,
        sha256: String,
        field: &'static str,
        write: impl Fn(&Path) -> std::io::Result<()>,
    ) -> Result<Kept, String> {
        let _held = COLLECTING.lock().unwrap_or_else(|e| e.into_inner());
        let already = !self.store(&path, write).map_err(|e| e.message)?;
        self.tabled(path.clone(), &sha256).map_err(|e| e.message)?;
        Ok(Kept { file: File { path, sha256, ..Default::default() }, field, already })
    }

    /// `path` (a file's) made by `write` at a part beside it and then named, unless it is there. Whether it wrote it.
    fn store(&self, path: &str, write: impl Fn(&Path) -> std::io::Result<()>) -> Result<bool, Error> {
        let at = self.dir.join(path);
        if at.is_file() {
            return Ok(false);
        }
        let dir = self.dir.join(FILES);
        std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
        let part = dir.join(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
        let _ = std::fs::remove_file(&part);
        write(&part).and_then(|()| std::fs::rename(&part, &at)).map_err(|e| io_at(&at, e))?;
        Ok(true)
    }

    /// A file's path, as the lock writes it, on this computer.
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// Every file `lock` names and nothing has measured, measured once and recorded, in `lock` and in
    /// project.lock. With no ffprobe here yet (the walk fetches one first), they wait for the next read.
    pub fn measure(&self, lock: &mut Lock) -> Result<(), Error> {
        if !lock.files.values().any(File::unmeasured) {
            return Ok(());
        }
        let Some((found, ffprobe)) = platform::tools::ffmpeg(&self.tools, &crate::tools::path()) else { return Ok(()) };
        let measured: BTreeMap<String, File> = (lock.files.values().filter(|file| file.unmeasured()))
            .filter(|file| self.dir.join(&file.path).is_file())
            .filter_map(|file| {
                let at = self.dir.join(&file.path);
                let probed = super::tools::measured(&found.path, &ffprobe, &at, file.is_video())?;
                Some((file.sha256.clone(), File { path: file.path.clone(), sha256: file.sha256.clone(), parts: file.parts.clone(), ..probed }))
            })
            .collect();
        if measured.is_empty() {
            return Ok(());
        }
        let fill = |lock: &mut Lock| {
            for (sha256, file) in &measured {
                lock.files.insert(sha256.clone(), file.clone());
            }
        };
        fill(lock);
        self.update_lock(fill)
    }

    /// The files in `files/` that nothing uses (`named` is every sha256 something does), whatever the lock's table says, and
    /// their bytes. With `delete` they go, from disk and from the table: what `reclaim --yes` frees.
    pub fn sweep(&self, named: &BTreeSet<String>, delete: bool) -> Result<(u64, u64), Error> {
        let _held = COLLECTING.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let (mut files, mut bytes) = (0, 0);
        for entry in std::fs::read_dir(self.dir.join(FILES)).into_iter().flatten().flatten() {
            let path = format!("{FILES}/{}", entry.file_name().to_string_lossy());
            let used = lock.files.values().any(|file| file.path == path && named.contains(&file.sha256));
            if path.contains("/.") || used || entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            bytes += entry.metadata().map_or(0, |meta| meta.len());
            files += 1;
            if delete {
                std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
            }
        }
        // What runs left where they land: a folder they made, or a file still there.
        let left = walked(&self.dir.join(RUNS));
        files += left.0;
        bytes += left.1;
        if delete {
            self.update_lock(|lock| {
                lock.files.retain(|sha256, _| named.contains(sha256));
                lock.runs.retain(|row| named.contains(&row.sha256));
            })?;
            let _ = std::fs::remove_dir_all(self.dir.join(RUNS));
        }
        Ok((files, bytes))
    }

    /// Bytes on disk of `sha256`, if here.
    pub fn size(&self, lock: &Lock, sha256: &str) -> u64 {
        lock.files.get(sha256).and_then(|file| std::fs::metadata(self.path(&file.path)).ok()).map_or(0, |meta| meta.len())
    }

    /// Deletes those of `sha256s` nothing in the lock names any more: a tool's old version, free to make again.
    pub fn drop_unused(&self, sha256s: &[String]) -> Result<(), Error> {
        let _held = COLLECTING.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let mut named = lock.named(|_| true);
        named.extend(lock.runs.iter().map(|row| row.sha256.as_str()));
        for sha256 in sha256s.iter().filter(|sha256| !named.contains(sha256.as_str())) {
            let Some(file) = lock.files.get(sha256) else { continue };
            let at = self.path(&file.path);
            let _ = std::fs::remove_file(&at);
        }
        let unused: Vec<&String> = sha256s.iter().filter(|sha256| !named.contains(sha256.as_str())).collect();
        self.update_lock(|lock| lock.files.retain(|sha256, _| !unused.contains(&sha256)))
    }
}

/// `text` as a lock of this format. A lock of another isn't read, so nothing writes over it.
fn parse_lock(text: &str) -> Result<Lock, Error> {
    let lock: Result<Lock, _> = serde_json::from_str(text);
    match lock {
        Ok(lock) if lock.format == FORMAT => Ok(lock),
        Ok(lock) => Err(old_lock(&lock.format.to_string())),
        Err(e) if e.to_string().contains("missing field `format`") => Err(old_lock("1")),
        Err(e) => Err(Error::new(Kind::Validation, "lock_unreadable", format!("{LOCK}: {e}"))),
    }
}

fn old_lock(format: &str) -> Error {
    let said = format!("{LOCK} is format {format} and cozy reads format {FORMAT}: it is left as it is");
    Error::new(Kind::Validation, "lock_format", said).remedy("migrate the project once with a cozy that reads it (design/project-graph)")
}

/// The file at `url`, public addresses only at every hop, up to `URL_MOST`. Err: why not.
async fn fetch_url(url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut sink = |at: u64, frame: &[u8]| {
        bytes.truncate(at as usize);
        if bytes.len() + frame.len() > URL_MOST {
            let detail = format!("it is over {} MiB, the most a URL may give: download it and import its path", URL_MOST >> 20);
            return Err(Refusal { code: Code::SOURCE_NOT_ALLOWED, detail });
        }
        bytes.extend_from_slice(frame);
        Ok(())
    };
    match tokio::time::timeout(URL_LONGEST, tensorfs::fetch::get_link(url, &mut sink)).await {
        Err(_) => Err(format!("it did not finish in {} s", URL_LONGEST.as_secs())),
        // A refusal's first sentence says it; the rest is for whoever reads the policy.
        Ok(Err(refused)) => {
            let why = refused.detail.split(". ").next().unwrap_or_default();
            Err(why.strip_prefix(&format!("{url}: ")).unwrap_or(why).to_owned())
        }
        Ok(Ok(())) => Ok(bytes),
    }
}

/// Whether `source` is a URL, not a path.
pub fn is_url(source: &str) -> bool {
    source.split_once("://").is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
}

/// `sha256` with no scheme.
pub fn hex_of(sha256: &str) -> &str {
    sha256.trim_start_matches("sha256:")
}

/// What cozy prints of `sha256` for project.yaml.
pub fn prefix_of(sha256: &str) -> &str {
    let hex = hex_of(sha256);
    &hex[..PREFIX.min(hex.len())]
}

/// `kind` when it is a picture, video or sound, with the field that names one. Err: why not.
fn media(kind: Option<infer::Type>) -> Result<(infer::Type, &'static str), String> {
    let field = |kind: &infer::Type| match kind.matcher_type() {
        MatcherType::Image => Some("image"),
        MatcherType::Video => Some("video"),
        MatcherType::Audio => Some("audio"),
        _ => None,
    };
    let kind = kind.ok_or("it is no picture, video or sound")?;
    let field = field(&kind).ok_or("it is no picture, video or sound")?;
    Ok((kind, field))
}

/// A file's path, given its extension.
fn file_named(sha256: &str, ext: &str) -> String {
    format!("{FILES}/{}.{ext}", hex_of(sha256))
}

/// A file's path: `files/<hex of sha256>.<the source's extension>`.
fn file_of(sha256: &str, source: &Path) -> String {
    let ext = source.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
    format!("{FILES}/{}{ext}", hex_of(sha256))
}

impl Folder {
    /// Writes `change(what path holds)` over it: compare and swap on its bytes. A write that lands
    /// between the read and the swap (a person saving) is read again and the change applied to it,
    /// so that edit stands.
    fn rewrite(&self, path: &Path, change: impl Fn(&str) -> Result<String, Error>) -> Result<(), Error> {
        let _held = WRITING.lock().unwrap_or_else(|e| e.into_inner());
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

/// How many files are under `dir`, and their bytes.
fn walked(dir: &Path) -> (u64, u64) {
    let (mut files, mut bytes) = (0, 0);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => {
                let below = walked(&entry.path());
                files += below.0;
                bytes += below.1;
            }
            Ok(meta) => {
                files += 1;
                bytes += meta.len();
            }
            Err(_) => {}
        }
    }
    (files, bytes)
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
