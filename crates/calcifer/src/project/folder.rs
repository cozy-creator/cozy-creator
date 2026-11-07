//! A project folder: `project.yaml` (what a person or the page says, which Calcifer writes whole, `assetfile`),
//! `project.lock` (cozy's: each file, each asset's versions, what made each, the runs still making one, and what
//! its walk still owes), `files/`, and the cuts at its root. Every file the project uses is a real file in
//! `files/<sha256>.<ext>`, flat. project.yaml names one only by a hex prefix of its hash (`Lock::matching`),
//! so the lock names nothing of this computer and travels with project.yaml.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Read,
    path::{Path, PathBuf},
    cell::Cell,
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use infer::MatcherType;
use platform::{durable, paths::Home};
use protocol::{
    calcifer::On,
    error::{Error, Kind},
};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use tensorfs::err::{Code, Refusal};


use super::graph::name_key;

pub const PROJECT: &str = "project.yaml";
pub const LOCK: &str = "project.lock";
/// Where the elements sit: only the page reads it, and nothing in it is an input.
pub const CANVAS: &str = "project.canvas.yaml";
/// Where every file the project uses is kept, by content: `files/<sha256>.<ext>`.
pub const FILES: &str = "files";
/// Where a run's outputs land before they are kept in `files/`.
pub const RUNS: &str = ".runs";
/// What a walk owes, in `.runs`: no one commits it, and it moves without touching what the project is.
pub const WALK: &str = ".runs/walk.json";
/// What `Idempotency-Key`s answered, for their retries. Not in project.lock, which a key would rewrite whole and which goes with the project, and
/// not in `.runs`, which `reclaim` empties.
const KEYS: &str = ".keys.json";
/// How long a key is kept: as long as a held file waits.
const KEY_KEPT_MS: i64 = 24 * 60 * 60 * 1000;
/// A cache of what is measured of each file, which a read of the file can always make again.
pub(super) const MEASURED: &str = ".measured.json";
/// Where a project.lock that can't be read goes.
const BROKEN: &str = "project.lock.broken";
const UNREADABLE: &str = "lock_unreadable";
/// What marks a row whose name the file has lost: no name can hold it.
pub(super) const TOMBSTONE: char = '#';
/// The lock's format.
pub(super) const FORMAT: u32 = 3;

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

/// Each kind of file a project keeps by what it is: its extension, its media type, and the field that names one.
const MEDIA: [(&str, &str, &str); 40] = [
    ("png", "image/png", "image"),
    ("jpg", "image/jpeg", "image"),
    ("jpeg", "image/jpeg", "image"),
    ("webp", "image/webp", "image"),
    ("avif", "image/avif", "image"),
    ("gif", "image/gif", "image"),
    ("bmp", "image/bmp", "image"),
    ("tif", "image/tiff", "image"),
    ("tiff", "image/tiff", "image"),
    ("heif", "image/heif", "image"),
    ("jxl", "image/jxl", "image"),
    ("jp2", "image/jp2", "image"),
    ("jxr", "image/vnd.ms-photo", "image"),
    ("ico", "image/vnd.microsoft.icon", "image"),
    ("psd", "image/vnd.adobe.photoshop", "image"),
    ("cr2", "image/x-canon-cr2", "image"),
    ("ora", "image/openraster", "image"),
    ("djvu", "image/vnd.djvu", "image"),
    ("dwg", "image/vnd.dwg", "image"),
    ("mp4", "video/mp4", "video"),
    ("m4v", "video/x-m4v", "video"),
    ("mov", "video/quicktime", "video"),
    ("webm", "video/webm", "video"),
    ("mkv", "video/x-matroska", "video"),
    ("avi", "video/x-msvideo", "video"),
    ("wmv", "video/x-ms-wmv", "video"),
    ("mpg", "video/mpeg", "video"),
    ("flv", "video/x-flv", "video"),
    ("wav", "audio/wav", "audio"),
    ("mp3", "audio/mpeg", "audio"),
    ("flac", "audio/flac", "audio"),
    ("ogg", "audio/ogg", "audio"),
    ("opus", "audio/ogg", "audio"),
    ("m4a", "audio/mp4", "audio"),
    ("aac", "audio/aac", "audio"),
    ("aiff", "audio/aiff", "audio"),
    ("midi", "audio/midi", "audio"),
    ("amr", "audio/amr", "audio"),
    ("dsf", "audio/x-dsf", "audio"),
    ("ape", "audio/x-ape", "audio"),
];
/// The media type and field of an extension, in any case, if it is a picture, video or sound.
pub fn media_of(extension: &str) -> Option<(&'static str, &'static str)> {
    let extension = extension.to_ascii_lowercase();
    MEDIA.iter().find(|(known, ..)| *known == extension).map(|(_, media, field)| (*media, *field))
}

/// A file the project uses: its path in the project (`files/<sha256>.<ext>`), its sha256, and what is measured of it: a picture's or video's
/// size, a video's rate and frames, a picture's alpha (the share of it fully transparent). Project.lock holds only what can't be read again
/// from a file here (`Folder::fill`): where it stays, and its parts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct File {
    /// It was in `files/` when this was read, listed once for all of them: a stat each is seconds, on Windows, across a project's thousands.
    #[serde(skip)]
    pub here: bool,
    #[serde(skip)]
    pub path: String,
    #[serde(skip)]
    pub sha256: String,
    #[serde(skip)]
    pub width: u64,
    #[serde(skip)]
    pub height: u64,
    #[serde(skip)]
    pub fps: f64,
    #[serde(skip)]
    pub frames: u64,
    #[serde(skip)]
    pub alpha: f64,
    /// A file too big for one object at the Hub: the objects it goes as, in order. Its first push lists them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<tensorfs::format::ObjectRef>,
    /// What its name will end in, while it isn't here to say.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ext: String,
    /// The machine that keeps it, an output not brought here (`OutputItem::stays`): nothing is in `files/` until something needs it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine: String,
    /// Its bytes, when it isn't here to measure.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub length: u64,
    /// A sound's length; a video's comes of its rate and frames.
    #[serde(skip)]
    pub duration_ms: u64,
}

/// What is measured of a file, kept apart from project.lock, as ffprobe can always say it again.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Measured {
    #[serde(default, skip_serializing_if = "is_zero")]
    width: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    height: u64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    fps: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    frames: u64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    alpha: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    duration_ms: u64,
}

impl File {
    fn extension(&self) -> String {
        Path::new(&self.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
    }

    /// What it is, `image`, `video` or `audio`, if its name says.
    pub fn media(&self) -> Option<&'static str> {
        media_of(&self.extension()).map(|(_, field)| field)
    }

    /// The role that pins it, by what it is.
    pub fn field(&self) -> &'static str {
        media_of(&self.extension()).map_or(if self.fps > 0.0 { "video" } else { "image" }, |(_, field)| field)
    }

    /// Whether a machine was left keeping it.
    pub fn stays(&self) -> bool {
        !self.machine.is_empty()
    }

    /// How long it lasts, if it does and that is measured.
    pub fn lasts_ms(&self) -> u64 {
        match self.fps > 0.0 {
            true => (self.frames as f64 / self.fps * 1000.0).round() as u64,
            false => self.duration_ms,
        }
    }

    /// Not measured yet, and one that can be.
    pub fn unmeasured(&self) -> bool {
        match media_of(&self.extension()) {
            Some((_, "audio")) => self.duration_ms == 0,
            Some(_) => self.width == 0,
            None => false,
        }
    }
}

/// A version's files: by output name, each one's sha256.
pub type Files = BTreeMap<String, String>;

/// One result of a tool: its number, its files and the key it was made under.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Version {
    /// 1 first, never reused.
    #[serde(default)]
    pub number: u32,
    /// Its key, 16 hex digits: what says it is out of date. Empty for one whose inputs can't be known: it never reads as current.
    #[serde(default)]
    pub inputs: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: Files,
    /// The seed Calcifer drew. None: its function takes no seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The last run that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub execution_ms: u64,
    /// What made it, `org/package/function@release`, as the asset's tool can change after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The take it picked up from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<protocol::project::Pickup>,
    /// An Edit's: the clips that made it, each with its file, and their rate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<protocol::project::Clip>,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub fps: f64,
    /// A cut's: the pieces it was cut without.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<Without>,
    /// What its function said of it, and what was left out of its request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The words it was made from, only where they aren't the version before it's (`Asset::text_of`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
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

/// A piece a cut was made without, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Without {
    pub piece: String,
    pub why: String,
    /// Its file wasn't on this computer: once it is, that version isn't its cut's state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
}

impl Without {
    /// As a film says it: `segment_3: no version yet`.
    pub fn said(&self) -> String {
        format!("{}: {}", self.piece, self.why)
    }
}

/// A run making a version, recorded before it is sent (under its idempotency key) and followed
/// until it lands: whatever stops the walk, the next one follows it and never sends it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    /// What its asset is, as its version is listed when it lands.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The job it is of: the key of its first call, when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job: String,
    /// How many GPUs it was given; 0: as wide as the machine goes.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gpus: u32,
    /// The render it is of, whose text it lands and goes on with.
    #[serde(default)]
    pub ask: String,
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
    /// Its pin, by the field holding it, goes when it lands (`--outdated`): if it never does, the pin stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpin: Option<String>,
    /// Where it goes.
    pub target: Target,
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
    /// Versions set aside: off the list, on record until `reclaim`, each number still given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discarded: Vec<Version>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tried: Option<Tried>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failed>,
}

/// What stopped the last try at it, on `inputs`: kept until a version lands or it's tried again, so a reader who wasn't watching hears it too.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
    pub error: Error,
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

    /// Its versions and those discarded.
    pub fn on_record(&self) -> impl Iterator<Item = &Version> {
        self.versions.iter().chain(&self.discarded)
    }

    /// The words version `number` was made from: its own, else those of the latest before it that holds any.
    pub fn text_of(&self, number: u32) -> Option<&str> {
        self.on_record().filter(|version| version.number <= number && version.text.is_some()).max_by_key(|version| version.number)?.text.as_deref()
    }

    /// Each version's words, by number.
    fn texts(&self) -> BTreeMap<u32, String> {
        self.on_record().filter_map(|version| Some((version.number, self.text_of(version.number)?.to_owned()))).collect()
    }

    /// `texts` written back, each only where it isn't the one before it's.
    fn keep_texts(&mut self, mut texts: BTreeMap<u32, String>) {
        let mut all: Vec<&mut Version> = self.versions.iter_mut().chain(&mut self.discarded).collect();
        all.sort_by_key(|version| version.number);
        let mut last = None;
        for version in all {
            let text = texts.remove(&version.number);
            version.text = text.clone().filter(|text| Some(text) != last.as_ref());
            last = text.or(last);
        }
    }

    /// `version` on the list in its place, in place of one of that number.
    pub fn list(&mut self, mut version: Version) {
        let mut texts = self.texts();
        texts.extend(version.text.take().map(|text| (version.number, text)));
        self.versions.retain(|held| held.number != version.number);
        self.versions.push(version);
        self.versions.sort_by_key(|version| version.number);
        self.keep_texts(texts);
    }

    /// What's discarded gone for good but `read`, which a text names: what comes after it keeps its words.
    pub fn clear_discarded(&mut self, read: Option<&BTreeSet<u32>>) {
        let texts = self.texts();
        self.discarded.retain(|version| read.is_some_and(|read| read.contains(&version.number)));
        self.keep_texts(texts);
    }

    /// Only `read` left of what's on record: what a text still names of a deleted asset.
    pub fn keep_only(&mut self, read: &BTreeSet<u32>) {
        let texts = self.texts();
        self.versions.retain(|version| read.contains(&version.number));
        self.discarded.retain(|version| read.contains(&version.number));
        self.keep_texts(texts);
    }

    /// Version `number` off the list and on record.
    pub fn discard(&mut self, number: u32) {
        self.numbered = self.numbered.max(number);
        if let Some(at) = self.versions.iter().position(|version| version.number == number) {
            self.discarded.push(self.versions.remove(at));
            self.discarded.sort_by_key(|version| version.number);
        }
    }

    /// What `discard` set aside of `number`, back in its place.
    pub fn undiscard(&mut self, number: u32) {
        if let Some(at) = self.discarded.iter().position(|version| version.number == number) {
            self.versions.push(self.discarded.remove(at));
            self.versions.sort_by_key(|version| version.number);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lock {
    pub format: u32,
    /// Every file, by sha256.
    #[serde(default)]
    pub files: BTreeMap<String, File>,
    /// By name key. An asset project.yaml no longer names is kept, its versions with it, until it is
    /// renamed or reclaimed.
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
    /// By idempotency key. Never in project.lock, which a copy or a `push` carries: `Folder::lock` reads it from `WALK`.
    #[serde(skip)]
    pub pending: BTreeMap<String, Pending>,
    /// What `cozy run`s left here: they belong to no asset, and no memo holds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<RunFile>,
    /// Each package's own tags, by the name its memo knows it by, as last read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reserved: BTreeMap<String, Vec<String>>,
    /// What each tool outputs, `image`, `video` or `audio`, as its interface last said, for what `defaults` fill before it has a version.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub makes: BTreeMap<String, String>,
    /// As `pending`.
    #[serde(skip)]
    pub walk: Owed,
}

/// What an `Idempotency-Key` answered, and the digest of what asked it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Keyed {
    pub digest: String,
    pub at_ms: i64,
    /// What a new asset was called.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// An import's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<protocol::project::Imported>,
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
        Self { format: FORMAT, files: BTreeMap::new(), assets: BTreeMap::new(), pending: BTreeMap::new(), runs: vec![], reserved: BTreeMap::new(), makes: BTreeMap::new(), walk: Owed::default() }
    }
}

/// What a render asked that the project's walk has not sent yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Owed {
    /// Each render not done, in the order asked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<Ask>,
    /// The `Idempotency-Key`s this walk was asked under, and each ask's digest: a retry across a restart adds nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub asked: BTreeMap<String, String>,
    /// What renders going may still call an asset, (from, to): a run on its way, a job being sent. Its row is under the new name; these find it until the walk ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<(String, String)>,
    /// The last number each asset's jobs took, by name key: taking one rewrites nothing of the project. One that failed stays taken in project.lock.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub numbered: BTreeMap<String, u32>,
    /// This walk's runs not over: what they land is their version's. With the version listed whose file is on its way, which a restart follows.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runs: BTreeMap<u64, Option<Coming>>,
    /// What those landed so far, each output's newest: what its next revision takes the place of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landing: Vec<RunFile>,
}

/// What a walk owes: what it sent and what its renders asked. Kept apart from the project, so it changes as often as it likes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WalkFile {
    /// The folder and Calcifer it is of: a copy of the folder, or another Calcifer, doesn't go on with paid work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<Bound>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default)]
    pub walk: Owed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bound {
    pub path: PathBuf,
    pub calcifer: String,
}

/// This Calcifer's id, the same across its restarts, once it has said it.
static CALCIFER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Reads this Calcifer's id from `dir`, or makes it. Before any project is read.
pub fn bind_calcifer(dir: &Path) -> Result<(), Error> {
    let path = dir.join("id");
    let id = match std::fs::read_to_string(&path) {
        Ok(id) if !id.trim().is_empty() => id.trim().to_owned(),
        _ => {
            let id = super::new_key();
            durable::replace(&path, id.as_bytes(), 0o600).map_err(io)?;
            id
        }
    };
    let _ = CALCIFER.set(id);
    Ok(())
}

impl WalkFile {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.walk.is_empty()
    }
}

impl Pending {
    /// The job it is of, under the key `key` it is sent with.
    pub fn job_of<'a>(&'a self, key: &'a str) -> &'a str {
        if self.job.is_empty() { key } else { &self.job }
    }
}

/// One render: project.yaml as it was when asked, and what it owes. It plans on that text alone, so an edit after it touches nothing
/// of it, and a restart resumes it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub id: String,
    /// Only in a walk from before its text was kept apart (`Folder::text_of`).
    #[serde(default, skip_serializing)]
    pub text: String,
    /// `text`'s sha256: asks of the same text make the same inputs.
    pub digest: String,
    /// Where what it brings up goes.
    pub target: Target,
    /// `render all --missing`: every asset with no state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    /// `render all --outdated`: each pin out of date when its turn comes, as that is judged then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
    /// Asked of `all`, with or without `--missing`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole: bool,
    /// What it sent or tried, made or failed, by name key: each is tried once.
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
        self.asks.is_empty() && self.asked.is_empty() && self.renames.is_empty() && self.runs.is_empty()
    }
}

/// A version listed before its files have landed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coming {
    pub name: String,
    pub number: u32,
    pub take: bool,
}

/// The machines a version may be made on, and the Hub its function is read from.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// A new seed even with a state, as every render but `--missing` asks: no such take is a run a state already answers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
    /// Pick up from here: the take its new part goes on from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<protocol::project::Pickup>,
}

impl Lock {
    /// The files whose hash starts with `prefix`.
    pub fn matching(&self, prefix: &str) -> Vec<&File> {
        let from = format!("sha256:{prefix}");
        self.files.range(from.clone()..).take_while(|(sha256, _)| sha256.starts_with(&from)).map(|(_, file)| file).collect()
    }

    /// Whether its walk owes anything: asked and not sent, or sent and not landed.
    pub fn owes(&self) -> bool {
        !self.walk.asks.is_empty() || !self.pending.is_empty() || !self.walk.renames.is_empty() || !self.walk.runs.is_empty()
    }

    /// The number the next version of `name` would take, as far as what's listed says.
    pub fn listed_number(&self, name: &str) -> u32 {
        self.asset(name).map_or(0, |held| held.numbered.max(held.versions.iter().map(|version| version.number).max().unwrap_or(0)))
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.get(&self.row_of(name))
    }

    /// Where `name`'s row is: under its name, else what a rename made it.
    fn row_of(&self, name: &str) -> String {
        let key = name_key(name);
        match self.assets.contains_key(&key) {
            true => key,
            false => self.walk.renames.iter().find(|(from, _)| name_key(from) == key).map_or(key, |(_, to)| name_key(to)),
        }
    }

    /// `from` is called `to` now, and what still says `from` finds it. A name taken again is what took it.
    pub fn called(&mut self, from: &str, to: &str) {
        let (key, new) = (name_key(from), name_key(to));
        if key == new {
            return;
        }
        self.walk.renames.retain(|(was, _)| name_key(was) != new);
        self.walk.renames.iter_mut().filter(|(_, now)| name_key(now) == key).for_each(|(_, now)| to.clone_into(now));
        self.walk.renames.push((from.to_owned(), to.to_owned()));
    }

    pub fn asset_mut(&mut self, name: &str, kind: &str) -> &mut Asset {
        let key = self.row_of(name);
        let own = key == name_key(name);
        let asset = self.assets.entry(key).or_default();
        if own {
            asset.name = name.to_owned();
        }
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

    /// `files` as `name`'s next version, unless a version holds that file already: its number, and whether it did. It made none of it, so it never reads as
    /// current, and it says no words.
    pub fn joined(&mut self, name: &str, kind: &str, files: Files) -> (u32, bool) {
        if let Some(held) = self.asset(name).and_then(|held| held.versions.iter().find(|version| version.main() == main_of(&files))) {
            return (held.number, true);
        }
        let number = self.number(name, kind);
        self.asset_mut(name, kind).list(Version { number, files, text: Some(String::new()), ..Default::default() });
        (number, false)
    }

    /// The rows of a name key, and those `tombstone` set aside of it.
    pub fn rows_of<'a>(&'a self, key: &str) -> impl Iterator<Item = (&'a String, &'a Asset)> {
        let (key, buried) = (key.to_owned(), format!("{key}{TOMBSTONE}"));
        self.assets.iter().filter(move |(at, _)| **at == key || at.starts_with(&buried))
    }

    /// The lock as it's written. A file holds a row only for what can't be read again from a file here: where it stays, its parts, or, with nothing
    /// else naming it, its whole hash, which `project.yaml` holds only the start of.
    pub fn written(&self) -> String {
        let named = self.recorded();
        let mut value = serde_json::to_value(self).expect("a lock serializes");
        if let Some(files) = value["files"].as_object_mut() {
            files.retain(|sha256, row| !row.as_object().is_some_and(Map::is_empty) || !named.contains(sha256.as_str()));
        }
        protocol::yaml::from_json(&value)
    }

    /// Each asset whose name the file has lost and with nothing on its way, set aside under a key no name can be, so that a later asset of that
    /// name starts with no versions. It stays an orphan until `reclaim`, and `cozy project rename` still finds it.
    pub fn tombstone(&mut self, named: impl Fn(&str) -> bool) {
        let going: BTreeSet<String> = (self.pending.values().map(|pending| name_key(&pending.asset)))
            .chain(self.walk.asks.iter().flat_map(|ask| ask.takes.iter().map(|take| name_key(&take.asset))))
            .collect();
        let lost: Vec<String> = self.assets.keys().filter(|key| !key.contains(TOMBSTONE) && !named(key) && !going.contains(*key)).cloned().collect();
        lost.iter().for_each(|key| self.bury(key));
    }

    /// The row under `key` set aside: its versions stay, an orphan's, and that name starts with none.
    pub fn bury(&mut self, key: &str) {
        let Some(mut held) = self.assets.remove(key).filter(|held| held.on_record().next().is_some()) else { return };
        if held.name.is_empty() {
            held.name = key.to_owned();
        }
        let free = (1..).map(|n| format!("{key}{TOMBSTONE}{n}")).find(|at| !self.assets.contains_key(at)).expect("a free key");
        self.assets.insert(free, held);
    }

    /// Every sha256 a version or a run has on record here, of any asset, discarded too: all `reclaim` may delete, once nothing uses it.
    pub fn recorded(&self) -> BTreeSet<String> {
        let discarded = self.assets.values().flat_map(|held| &held.discarded).flat_map(|version| version.files.values()).map(String::as_str);
        self.named(|_| true).into_iter().chain(discarded).chain(self.runs.iter().map(|row| row.sha256.as_str())).map(str::to_owned).collect()
    }

    /// The latest row `tombstone` set aside of `name`, back under its name, if nothing holds that. Whether one came back.
    pub fn untombstone(&mut self, name: &str) -> bool {
        let key = name_key(name);
        if self.assets.contains_key(&key) {
            return false;
        }
        let prefix = format!("{key}{TOMBSTONE}");
        let latest = self.assets.keys().filter(|at| at.starts_with(&prefix)).max_by_key(|at| at[prefix.len()..].parse::<u32>().unwrap_or(0)).cloned();
        let Some(held) = latest.and_then(|at| self.assets.remove(&at)) else { return false };
        self.assets.insert(key, held);
        true
    }

    /// `sha256` is on `machine`, and not here: for good, or with `name`, on its way here as the file that makes.
    pub fn away(&mut self, sha256: &str, machine: &str, length: u64, name: Option<&str>) {
        let path = file_of(sha256, Path::new(name.unwrap_or_default()));
        let ext = path.rsplit_once('.').map(|(_, ext)| format!(".{ext}")).unwrap_or_default();
        let held = self.files.entry(sha256.to_owned()).or_default();
        if name.is_none() || !held.stays() {
            (held.machine, held.length) = (machine.to_owned(), length);
            if name.is_some() {
                held.ext = ext;
            }
        }
    }

    /// Every sha256 a version or pending run of a kept asset names; `keep` says which assets are kept, by key.
    pub fn named(&self, keep: impl Fn(&str) -> bool) -> BTreeSet<&str> {
        let versions = (self.assets.iter().filter(|(key, _)| keep(key))).flat_map(|(_, asset)| &asset.versions);
        let pending = self.pending.values().map(|pending| &pending.version);
        versions.chain(pending).flat_map(|version| version.files.values()).map(String::as_str).collect()
    }
}

/// What this Calcifer holds of one project folder, whichever `Folder` names it.
#[derive(Default)]
struct Shared {
    /// Held across each write's read and swap, so no two of this Calcifer's writes to it lose one. Nothing under it waits on the
    /// disk's write-back, and nothing that reads takes it.
    writing: Mutex<()>,
    /// Held while a file is kept and while files are deleted, so one isn't deleted between.
    collecting: Mutex<()>,
    /// project.lock as last read, and how what it was read from stood then, so it is read again only when that changes.
    held: Mutex<Option<(Stamp, Lock)>>,
}

static SHARED: LazyLock<Mutex<HashMap<PathBuf, Arc<Shared>>>> = LazyLock::new(Mutex::default);

fn shared(dir: &Path) -> Arc<Shared> {
    let dir = durable::canonical(dir).unwrap_or_else(|_| dir.to_owned());
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).entry(dir).or_default().clone()
}

/// A write that took this long, waiting and held, is said, with how long each took.
const SLOW_MS: f64 = 5.0;

thread_local! {
    /// What this thread's writes spent replacing their files, since its turn began.
    static REPLACE_MS: Cell<f64> = const { Cell::new(0.0) };
}

/// One write's turn at its project's `writing`.
struct Turn<'a> {
    _held: MutexGuard<'a, ()>,
    file: &'static str,
    began: Instant,
    wait_ms: f64,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let (wait_ms, held_ms, replace_ms) = (self.wait_ms, ms(self.began), REPLACE_MS.get());
        if wait_ms + held_ms >= SLOW_MS {
            info!(file = self.file, phase = "project_write", wait_ms, held_ms, replace_ms);
        }
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Replaced at once, and durable soon after: no write of a project waits on anyone else's bytes reaching the disk. A crash of the
/// machine in between keeps the file before, whole, and the order of these: what it can lose is the last moment of edits.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let began = Instant::now();
    let done = durable::replace_soon(path, bytes, 0o644).map_err(io);
    REPLACE_MS.set(REPLACE_MS.get() + ms(began));
    done
}
pub struct Folder {
    pub dir: PathBuf,
    /// cozy's tools folder, where its ffprobe is (else PATH's).
    pub tools: PathBuf,
    shared: Arc<Shared>,
    /// Why what's landing can't go in just now. It's owed in the walk's file, and said as a warning until it has.
    unsaved: Mutex<Option<String>>,
}

/// How each of project.lock, the measurements and `files/` stood.
type Stamp = [String; 3];

impl Folder {
    /// The project in `dir`, whichever part of Calcifer has it open: for what lands from outside a walk.
    pub fn at(dir: &Path) -> Self {
        Self { dir: dir.to_owned(), tools: PathBuf::new(), shared: shared(dir), unsaved: Mutex::default() }
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
        Ok(Self { dir: dir.to_owned(), tools: home.tools(), shared: shared(dir), unsaved: Mutex::default() })
    }

    pub fn project(&self) -> PathBuf {
        self.dir.join(PROJECT)
    }

    pub fn text(&self) -> Result<String, Error> {
        read(&self.project())
    }

    /// The history: this and `update_lock` are the one way to it, whatever holds it.
    pub fn lock(&self) -> Result<Lock, Error> {
        let mut lock = self.lock_file()?;
        // What the walk owes is never held: the walk's file says it, every time, as it does after a crash.
        if let Some(walk) = self.walk_kept()? {
            (lock.pending, lock.walk) = (walk.pending, walk.walk);
        }
        Ok(lock)
    }

    /// project.lock as the file says, held while it is the same file.
    fn lock_file(&self) -> Result<Lock, Error> {
        self.reading_lock(Lock::clone)
    }

    /// `seen` of project.lock as the file says, with no copy of it made. What the walk owes isn't in it.
    pub fn reading_lock<R>(&self, seen: impl FnOnce(&Lock) -> R) -> Result<R, Error> {
        let stamp = self.stamp();
        let mut held = self.shared.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, lock)) = held.as_ref().filter(|(at, _)| *at == stamp) {
            return Ok(seen(lock));
        }
        let path = self.dir.join(LOCK);
        let mut lock = match path.exists() {
            true => match parse_lock(&read(&path)?) {
                Err(error) if error.code == UNREADABLE => self.set_aside(&error)?,
                parsed => parsed?,
            },
            false => Lock::default(),
        };
        self.fill(&mut lock);
        let answer = seen(&lock);
        *held = Some((stamp, lock));
        Ok(answer)
    }

    fn turn(&self, file: &'static str) -> Turn<'_> {
        let asked = Instant::now();
        let held = self.shared.writing.lock().unwrap_or_else(|e| e.into_inner());
        REPLACE_MS.set(0.0);
        Turn { _held: held, file, began: Instant::now(), wait_ms: ms(asked) }
    }

    fn stamp(&self) -> Stamp {
        [LOCK, MEASURED, FILES].map(|name| super::stamp(&self.dir.join(name)))
    }

    /// A project.lock that can't be read is kept, never written over: moved aside, and said by `Doc::read` for as long as it's there.
    fn set_aside(&self, why: &Error) -> Result<Lock, Error> {
        let aside = (1..).map(|n| self.dir.join(if n == 1 { BROKEN.to_owned() } else { format!("{BROKEN}-{n}") })).find(|path| !path.exists()).expect("a free name");
        std::fs::rename(self.dir.join(LOCK), &aside).map_err(|e| io_at(&aside, e))?;
        warn!("{}: {}: moved to {}", self.dir.display(), why.message, aside.display());
        Ok(Lock::default())
    }

    /// What says project.lock was set aside, if one was: its versions aren't read.
    pub fn unsaved(&self) -> Option<String> {
        self.unsaved.lock().expect("unsaved lock").clone()
    }

    pub fn set_unsaved(&self, why: Option<String>) {
        *self.unsaved.lock().expect("unsaved lock") = why;
    }

    pub fn lock_set_aside(&self) -> bool {
        self.dir.join(BROKEN).exists()
    }

    /// A lock for a project about to hold a file: `files/` with none reads as a project that lost its own.
    pub fn start_lock(&self) -> Result<(), Error> {
        match self.dir.join(LOCK).exists() {
            true => Ok(()),
            false => self.update_lock(|_| {}),
        }
    }

    /// Whether project.lock is gone from a project that has files.
    pub fn lock_gone(&self) -> bool {
        !self.dir.join(LOCK).exists() && std::fs::read_dir(self.dir.join(FILES)).is_ok_and(|mut files| files.next().is_some())
    }

    /// Why the files `named` leaves out may be all that's left of this project's takes, not what's done with: project.lock was lost, can't be
    /// read or lists nothing, so nothing here says what they were. `reclaim --yes` deletes none of them.
    pub fn history_lost(&self, lock: &Lock, named: &BTreeSet<String>) -> Option<String> {
        let unnamed = lock.files.values().filter(|file| !named.contains(&file.sha256) && self.path(&file.path).is_file()).count();
        let said = if unnamed == 0 {
            return None;
        } else if self.lock_set_aside() {
            format!("{LOCK} can't be read and is kept as {BROKEN}")
        } else if !self.dir.join(LOCK).exists() {
            format!("{LOCK} is gone")
        } else if lock.runs.is_empty() && lock.assets.values().all(|held| held.on_record().next().is_none()) {
            format!("{LOCK} lists no versions")
        } else {
            return None;
        };
        Some(format!("{said}, so the {unnamed} files nothing names in {FILES}/ may be all that's left of this project's takes"))
    }

    /// `lock`'s file table whole, from what needs no record: each file in `files/` names itself, what a version or run names and isn't here has
    /// its hash, and what is measured comes from a cache a read of the file can always make again.
    pub(super) fn fill(&self, lock: &mut Lock) {
        for (sha256, file) in &mut lock.files {
            file.sha256.clone_from(sha256);
            file.path = format!("{FILES}/{}{}", hex_of(sha256), file.ext);
        }
        for sha256 in lock.recorded() {
            let file = lock.files.entry(sha256.clone()).or_default();
            file.path = format!("{FILES}/{}{}", hex_of(&sha256), file.ext);
            file.sha256 = sha256;
        }
        for entry in std::fs::read_dir(self.dir.join(FILES)).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (hex, rest) = name.split_once('.').unwrap_or((&name, ""));
            // What is named more than its hash and its type is no file of the project's.
            if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) && !rest.contains('.') {
                let file = lock.files.entry(format!("sha256:{hex}")).or_default();
                file.path = format!("{FILES}/{name}");
                file.sha256 = format!("sha256:{hex}");
                file.here = true;
                // It's here: the machine that kept it keeps it no longer, and the next write says so, with no write of its own.
                (file.ext, file.machine, file.length) = Default::default();
            }
        }
        let measured: BTreeMap<String, Measured> = std::fs::read(self.dir.join(MEASURED)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        for (sha256, held) in measured {
            if let Some(file) = lock.files.get_mut(&sha256) {
                (file.width, file.height, file.fps, file.frames, file.alpha, file.duration_ms) = (held.width, held.height, held.fps, held.frames, held.alpha, held.duration_ms);
            }
        }
    }

    /// What the walk owes, with no need to read the rest. A lock from before holds its own until its next write.
    pub fn walk_now(&self) -> Result<WalkFile, Error> {
        Ok(self.walk_kept()?.unwrap_or_default())
    }

    /// Who may go on with this folder's walk: this path, under this Calcifer. None where no Calcifer runs, which judges nothing.
    fn bound(&self) -> Option<Bound> {
        let path = durable::canonical(&self.dir).unwrap_or_else(|_| self.dir.clone());
        CALCIFER.get().map(|calcifer| Bound { path, calcifer: calcifer.clone() })
    }

    fn walk_kept(&self) -> Result<Option<WalkFile>, Error> {
        let path = self.dir.join(WALK);
        if !path.exists() {
            return Ok(None);
        }
        let walk: WalkFile = serde_json::from_str(&read(&path)?).map_err(|e| Error::new(Kind::Validation, "walk_unreadable", format!("{WALK}: {e}")))?;
        if let Some(here) = self.bound().filter(|here| walk.bound.as_ref() != Some(here)) {
            warn!("{}: its walk is of {:?}, so it doesn't go on here", here.path.display(), walk.bound);
            let _ = std::fs::remove_file(&path);
            self.drop_texts();
            return Ok(None);
        }
        Ok(Some(walk))
    }

    fn keep_walk(&self, walk: &mut WalkFile) -> Result<(), Error> {
        let path = self.dir.join(WALK);
        if walk.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_at(&path, e)),
                _ => Ok(()),
            };
        }
        walk.bound = self.bound();
        std::fs::create_dir_all(self.dir.join(RUNS)).map_err(|e| io_at(&path, e))?;
        replace(&path, serde_json::to_string(walk).expect("a walk serializes").as_bytes())
    }

    /// The text render `ask` was asked with: kept apart, so what the walk owes stays small.
    pub fn text_of(&self, ask: &Ask) -> Result<String, Error> {
        match ask.text.is_empty() {
            true => read(&self.dir.join(RUNS).join("asked").join(&ask.digest)),
            false => Ok(ask.text.clone()),
        }
    }

    pub fn keep_text(&self, digest: &str, text: &str) -> Result<(), Error> {
        let dir = self.dir.join(RUNS).join("asked");
        std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
        replace(&dir.join(digest), text.as_bytes())
    }

    pub fn drop_text(&self, digest: &str) {
        let _ = std::fs::remove_file(self.dir.join(RUNS).join("asked").join(digest));
    }

    /// No render holds any.
    pub fn drop_texts(&self) {
        let _ = std::fs::remove_dir_all(self.dir.join(RUNS).join("asked"));
    }

    /// What `Idempotency-Key` `key` answered here.
    pub fn keyed(&self, key: &str) -> Option<Keyed> {
        self.keys().remove(key)
    }

    fn keys(&self) -> BTreeMap<String, Keyed> {
        std::fs::read(self.dir.join(KEYS)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// Keeps what `key` answered.
    pub fn keep_key(&self, key: &str, digest: &str, name: &str, answer: Option<protocol::project::Imported>) -> Result<(), Error> {
        let _turn = self.turn(KEYS);
        let mut keys = self.keys();
        let at_ms = super::now_ms();
        keys.retain(|_, row| at_ms - row.at_ms < KEY_KEPT_MS);
        keys.insert(key.to_owned(), Keyed { digest: digest.to_owned(), at_ms, name: name.to_owned(), answer });
        replace(&self.dir.join(KEYS), serde_json::to_string(&keys).expect("keys serialize").as_bytes())
    }

    /// Applies `change` to what the walk owes, touching nothing of what the project is.
    pub fn update_walk(&self, change: impl Fn(&mut WalkFile)) -> Result<(), Error> {
        let _turn = self.turn(WALK);
        let mut walk = self.walk_kept()?.unwrap_or_default();
        change(&mut walk);
        self.keep_walk(&mut walk)
    }

    /// Applies `change` to project.lock as it is now, what the walk owes among it.
    pub fn update_lock(&self, change: impl Fn(&mut Lock)) -> Result<(), Error> {
        let _turn = self.turn(LOCK);
        let moved = std::cell::RefCell::new(None);
        let wrote = std::cell::RefCell::new(None);
        // What this wrote it needn't read back.
        let held = || {
            if let Some(mut lock) = wrote.take() {
                self.fill(&mut lock);
                *self.shared.held.lock().unwrap_or_else(|e| e.into_inner()) = Some((self.stamp(), lock));
            }
        };
        self.rewrite_held(&self.dir.join(LOCK), |_| {
            let mut lock = self.lock_file()?;
            let kept = self.walk_kept()?;
            let before = kept.as_ref().map(|walk| (serde_json::to_string(&walk.pending).expect("a walk serializes"), serde_json::to_string(&walk.walk).expect("a walk serializes")));
            if let Some(walk) = kept {
                (lock.pending, lock.walk) = (walk.pending, walk.walk);
            }
            change(&mut lock);
            let walk = WalkFile { bound: None, pending: std::mem::take(&mut lock.pending), walk: std::mem::take(&mut lock.walk) };
            let after = (serde_json::to_string(&walk.pending).expect("a walk serializes"), serde_json::to_string(&walk.walk).expect("a walk serializes"));
            // Unchanged, it isn't written.
            *moved.borrow_mut() = (before.as_ref() != Some(&after) && (before.is_some() || !walk.is_empty())).then_some(walk);
            let text = lock.written();
            *wrote.borrow_mut() = Some(lock);
            Ok(text)
        }, held)?;
        // The project first. Killed between, a version is listed and the run that made it is still owed, so it lands again, the same. The
        // other way round, its take would be listed nowhere and its run owed by no one.
        match moved.into_inner() {
            Some(mut walk) => self.keep_walk(&mut walk),
            None => Ok(()),
        }
    }

    /// Writes one change to project.yaml: `change` is given what it holds this moment, and a save that lands first is read again,
    /// so that edit stands. The only write of it but `cozy project new` and `cozy project upgrade`.
    pub fn edit_project(&self, change: impl Fn(&str) -> Result<String, Error>) -> Result<String, Error> {
        self.rewrite(&self.project(), change)
    }

    /// What project.canvas.yaml holds, nothing if it isn't there.
    pub fn canvas_text(&self) -> Result<String, Error> {
        match self.dir.join(CANVAS).exists() {
            true => read(&self.dir.join(CANVAS)),
            false => Ok(String::new()),
        }
    }

    /// Writes one change to project.canvas.yaml, as `edit_project` does to project.yaml.
    pub fn edit_canvas(&self, change: impl Fn(&str) -> Result<String, Error>) -> Result<(), Error> {
        self.rewrite(&self.dir.join(CANVAS), change).map(drop)
    }

    /// `source`, a file a run saved, kept: copied whole before it is named, once per content, and in the lock's table.
    pub fn keep(&self, source: &Path) -> Result<String, Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let sha256 = hash(source)?;
        let path = file_of(&sha256, source);
        self.store(&path, |part| std::fs::copy(source, part).map(drop))?;
        Ok(sha256)
    }

    /// A file a run is given, of `sha256`, kept in `files/` unless it's here, and in the lock's table, under what its bytes say it is
    /// else its own extension: a hard link with `link`, else a copy.
    pub fn keep_given(&self, source: &Path, sha256: &str, link: bool) -> Result<(), Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        if self.lock()?.files.get(sha256).is_some_and(|file| self.path(&file.path).is_file()) {
            return Ok(());
        }
        let mut head = Vec::new();
        std::fs::File::open(source).and_then(|file| file.take(8192).read_to_end(&mut head)).map_err(|e| io_at(source, e))?;
        let path = infer::get(&head).map_or_else(|| file_of(sha256, source), |kind| file_named(sha256, kind.extension()));
        self.store(&path, |part| match link && std::fs::hard_link(source, part).is_ok() {
            true => Ok(()),
            false => std::fs::copy(source, part).map(drop),
        })?;
        Ok(())
    }

    /// `source`, a file Calcifer made beside the project, moved into `files/` under its hash, and in the lock's table.
    pub fn take(&self, source: &Path) -> Result<String, Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let sha256 = hash(source)?;
        let path = file_of(&sha256, source);
        if !self.store(&path, |part| std::fs::rename(source, part))? {
            let _ = std::fs::remove_file(source);
        }
        Ok(sha256)
    }

    /// What a run landed at `dest`, of `digest`, kept in `files/` by hash: a hard link where the filesystem allows. Listed as
    /// that run's, in place of what an earlier revision of that output was, which goes if nothing else names it. Answers its path, and why it
    /// isn't listed: paid-for bytes are kept whatever the project can say of them.
    pub fn adopt(&self, dest: &Path, digest: &str, made: RunFile) -> Result<(PathBuf, Option<Error>), Error> {
        let (path, old, unlisted) = {
            let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
            let path = file_of(digest, dest);
            let at = self.dir.join(&path);
            if !at.is_file() {
                let dir = self.dir.join(FILES);
                std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
                match std::fs::hard_link(dest, &at) {
                    Ok(()) => {
                        sealed(&at);
                        durable::soon(&at);
                    }
                    Err(_) => drop(self.store(&path, |part| std::fs::copy(dest, part).map(drop))?),
                }
            }
            let old = std::cell::RefCell::new(vec![]);
            let listed = |rows: &mut Vec<RunFile>| {
                let earlier = rows.iter().position(|row| row.run == made.run && row.output == made.output);
                match earlier {
                    Some(at) => *old.borrow_mut() = vec![std::mem::replace(&mut rows[at], made.clone()).sha256],
                    None => rows.push(made.clone()),
                }
            };
            // A walk's run lands on its version: nothing of the project moves until then.
            let listed = match self.walk_kept() {
                Ok(walk) if walk.as_ref().is_some_and(|walk| walk.walk.runs.contains_key(&made.run)) => self.update_walk(|walk| listed(&mut walk.walk.landing)),
                Ok(_) => self.update_lock(|lock| listed(&mut lock.runs)),
                Err(error) => Err(error),
            };
            (self.path(&path), old.into_inner(), listed.err())
        };
        if unlisted.is_none() {
            self.drop_unused(&old)?;
        }
        Ok((path, unlisted))
    }

    /// An output its machine keeps, in the lock's table: its hash, which machine, and no file.
    pub fn stays(&self, sha256: &str, machine: &str, length: u64) -> Result<(), Error> {
        self.update_lock(|lock| lock.away(sha256, machine, length, None))
    }

    /// The file `sha256` as the project has it, here or kept by a machine: nothing need come for it.
    pub fn own(&self, sha256: &str) -> Option<Kept> {
        let file = self.lock().ok()?.files.remove(sha256)?;
        (file.stays() || self.path(&file.path).is_file()).then(|| Kept { field: file.field(), file, already: true })
    }

    /// The held file at `at`, of `sha256`, moved into `files/` and recorded in project.lock. Err: why not.
    pub fn take_held(&self, at: &Path, sha256: &str) -> Result<Kept, String> {
        let mut head = Vec::new();
        std::fs::File::open(at).and_then(|file| file.take(8192).read_to_end(&mut head)).map_err(|e| e.to_string())?;
        let (kind, field) = media(infer::get(&head))?;
        let path = file_named(sha256, kind.extension());
        self.recorded(path, sha256.to_owned(), field, |part| match std::fs::rename(at, part) {
            Ok(()) => Ok(()),
            // Another disk.
            Err(_) => std::fs::copy(at, part).and_then(|_| std::fs::remove_file(at)),
        })
    }

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

    /// An import's bytes at `path`, written by `write` unless the project has them under any name, named in project.lock.
    fn recorded(
        &self,
        path: String,
        sha256: String,
        field: &'static str,
        write: impl Fn(&Path) -> std::io::Result<()>,
    ) -> Result<Kept, String> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(kept) = self.own(&sha256).filter(|kept| self.path(&kept.file.path).is_file()) {
            return Ok(kept);
        }
        let already = !self.store(&path, write).map_err(|e| e.message)?;
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
        sealed(&at);
        durable::soon(&at);
        Ok(true)
    }

    /// A file's path, as the lock writes it, on this computer.
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// Whether a file the lock names should be here and isn't, so a pull is what it needs: what stays on a machine isn't.
    pub fn missing(&self, file: &File) -> bool {
        !file.stays() && !file.here
    }

    /// Whether files wait to be measured with no ffprobe here to do it: a walk fetches one first.
    pub fn wants_ffprobe(&self, lock: &Lock) -> bool {
        lock.files.values().any(File::unmeasured) && platform::tools::ffmpeg(&self.tools, &crate::tools::path()).is_none()
    }

    /// How many frames the film at `path` holds, by its file; None with no file there, or no ffprobe to read it. Asked of ffprobe once for
    /// each size and time it has, since a read of a project asks this.
    pub fn film_frames(&self, path: &Path) -> Option<u64> {
        let meta = std::fs::metadata(path).ok()?;
        let held = (meta.len(), meta.modified().ok()?);
        if let Some((known, frames)) = FILMS.lock().expect("films lock").get(path)
            && *known == held
        {
            return *frames;
        }
        let (_, ffprobe) = platform::tools::ffmpeg(&self.tools, &crate::tools::path())?;
        let frames = super::tools::frames_of(&ffprobe, path);
        FILMS.lock().expect("films lock").insert(path.to_owned(), (held, frames));
        frames
    }

    /// Every file `lock` names and nothing has measured, measured once and recorded, in `lock` and in
    /// project.lock. With no ffprobe here yet they wait for the next read, or a cut, which fetches one first.
    pub fn measure(&self, lock: &mut Lock) -> Result<(), Error> {
        if !lock.files.values().any(File::unmeasured) {
            return Ok(());
        }
        let Some((found, ffprobe)) = platform::tools::ffmpeg(&self.tools, &crate::tools::path()) else { return Ok(()) };
        let measured: BTreeMap<String, File> = (lock.files.values().filter(|file| file.unmeasured()))
            .filter(|file| self.dir.join(&file.path).is_file())
            .filter_map(|file| Some((file.sha256.clone(), super::tools::measured(&found.path, &ffprobe, &self.dir.join(&file.path), file.field())?)))
            .collect();
        if measured.is_empty() {
            return Ok(());
        }
        for (sha256, probed) in &measured {
            if let Some(file) = lock.files.get_mut(sha256) {
                (file.width, file.height, file.fps, file.frames, file.alpha, file.duration_ms) = (probed.width, probed.height, probed.fps, probed.frames, probed.alpha, probed.duration_ms);
                // A video that landed: the page plays its preview on a hover.
                if file.field() == "video" && file.width > 0 {
                    super::thumbs::landed(self.dir.join(&file.path), sha256, file.lasts_ms() as f64 / 1000.0);
                }
            }
        }
        self.rewrite(&self.dir.join(MEASURED), |text| {
            let mut held: BTreeMap<String, Measured> = serde_json::from_str(text).unwrap_or_default();
            for (sha256, file) in &measured {
                held.insert(sha256.clone(), Measured { width: file.width, height: file.height, fps: file.fps, frames: file.frames, alpha: file.alpha, duration_ms: file.duration_ms });
            }
            Ok(serde_json::to_string(&held).expect("what is measured serializes"))
        })
        .map(drop)
    }

    /// The files in `files/` that nothing uses (`named` is every sha256 something does), whatever the lock's table says, their bytes, and how many
    /// of them nothing has a record of. With `delete` those on record go, from disk and from the table: what `reclaim --yes` frees. One with no
    /// record may be all that's left of a take, so it's said and it stays.
    pub fn sweep(&self, named: &BTreeSet<String>, recorded: &BTreeSet<String>, delete: bool) -> Result<(u64, u64, u64), Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let (mut files, mut bytes, mut unrecorded) = (0, 0, 0);
        for entry in std::fs::read_dir(self.dir.join(FILES)).into_iter().flatten().flatten() {
            let path = format!("{FILES}/{}", entry.file_name().to_string_lossy());
            let file = lock.files.values().find(|file| file.path == path);
            if path.contains("/.") || file.is_some_and(|file| named.contains(&file.sha256)) || entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if !file.is_some_and(|file| recorded.contains(&file.sha256)) {
                unrecorded += 1;
                if delete {
                    continue;
                }
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
        Ok((files, bytes, unrecorded))
    }

    /// Bytes on disk of `sha256`, if here.
    pub fn size(&self, lock: &Lock, sha256: &str) -> u64 {
        lock.files.get(sha256).and_then(|file| std::fs::metadata(self.path(&file.path)).ok()).map_or(0, |meta| meta.len())
    }

    /// Deletes those of `sha256s` nothing in the lock names any more: a tool's old version, free to make again.
    pub fn drop_unused(&self, sha256s: &[String]) -> Result<(), Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let named = lock.recorded();
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
    let unreadable = |why: String| Error::new(Kind::Validation, UNREADABLE, format!("{LOCK}: {why}"));
    let value = protocol::yaml::to_json(text).map_err(unreadable)?;
    match value["format"].as_u64() {
        Some(format) if format == u64::from(FORMAT) => serde_json::from_value(value).map_err(|e| unreadable(e.to_string())),
        Some(format) => Err(old_lock(&format.to_string())),
        None if value.is_object() => Err(old_lock("1")),
        None => Err(unreadable("it is no mapping".to_owned())),
    }
}

/// A lock older than its project.yaml, which `Projects::open` would have moved had the file been old too: nothing here can convert it.
fn old_lock(format: &str) -> Error {
    let said = format!("{LOCK} is format {format} and cozy reads format {FORMAT}: it is left as it is, and {PROJECT} is newer than it, so it can't be upgraded");
    Error::new(Kind::Validation, "lock_format", said).remedy(format!("put back the {LOCK} that goes with this {PROJECT}, from git or a backup"))
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
    fn rewrite(&self, path: &Path, change: impl Fn(&str) -> Result<String, Error>) -> Result<String, Error> {
        let _turn = self.turn(match path.file_name().and_then(|name| name.to_str()) {
            Some(PROJECT) => PROJECT,
            Some(CANVAS) => CANVAS,
            _ => MEASURED,
        });
        self.rewrite_held(path, change, || {})
    }

    /// As `rewrite`, its turn taken.
    fn rewrite_held(&self, path: &Path, change: impl Fn(&str) -> Result<String, Error>, wrote: impl FnOnce()) -> Result<String, Error> {
        loop {
            let before = match path.exists() {
                true => read(path)?,
                false => String::new(),
            };
            let after = change(&before)?;
            if after == before {
                return Ok(after);
            }
            let now = match path.exists() {
                true => read(path)?,
                false => String::new(),
            };
            if now == before {
                replace(path, after.as_bytes())?;
                wrote();
                crate::events::project_changed(&self.dir);
                return Ok(after);
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

/// What ffprobe last said of each film at a project's root, by its size and time.
static FILMS: LazyLock<Mutex<HashMap<PathBuf, ((u64, std::time::SystemTime), Option<u64>)>>> = LazyLock::new(Mutex::default);

/// A file in `files/` is named for its bytes: read-only, so that writing to it fails rather than make its name a lie.
pub(super) fn sealed(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut permissions = meta.permissions();
        permissions.set_readonly(true);
        let _ = std::fs::set_permissions(path, permissions);
    }
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
    if let platform::Error::Io { path, source, .. } = &error
        && durable::held(source)
    {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
        return Error::new(Kind::Unavailable, "file_held", format!("{name} is open in another program")).remedy("close it, and save again");
    }
    Error::new(Kind::Unavailable, "project_io", error.to_string())
}

fn io_at(path: &Path, error: std::io::Error) -> Error {
    Error::new(Kind::Unavailable, "project_io", format!("{}: {error}", path.display()))
}
