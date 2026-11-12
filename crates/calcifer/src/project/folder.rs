//! A project folder (cl-803): `project.db`, which holds everything a project says and has made (`db`, `docs/PROJECT-DB.md`), `files/`,
//! where every file it uses is kept by its hash (`files/<sha256>.<ext>`, flat), its cuts at its root, and `.runs/`, where a run's outputs
//! land on their way in. Its boxes name a file only by a hex prefix of its hash (`Lock::matching`), so nothing in it names this computer.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use infer::MatcherType;
use platform::{durable, paths::Home};
use api::{
    calcifer::On,
    error::{Error, Kind},
    calcifer::project::Unheld,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use tensorfs::err::{Code, Refusal};

use super::{
    db::{self, CardRow, Extra, Stored},
    graph::name_key,
    written::Written,
};

/// The board the last render ended on.
const BOARD: &str = "board";
/// The machines the project's runs go to (cl-873), as a JSON list of names.
const MACHINES: &str = "machines";

/// Authoring interchange filenames, also recognized to refuse retired file-only folders.
pub(crate) const PROJECT: &str = "project.yaml";
pub(crate) const LOCK: &str = "project.lock";
/// Where every file the project uses is kept, by content: `files/<sha256>.<ext>`.
pub(crate) const FILES: &str = "files";
/// Where a run's outputs land before they are kept in `files/`.
pub(crate) const RUNS: &str = ".runs";
/// What marks a row whose name the file has lost: no name can hold it.
pub(super) const TOMBSTONE: char = '#';
/// The lock's format, as `project.lock` wrote it.
pub(super) const FORMAT: u32 = 3;

/// The most a URL may give: far past any picture, and a shared project can't fill a disk with it.
const URL_MOST: usize = 64 << 20;
/// How long a URL may take. Measured progress can't judge this one: a stranger's server can drip a
/// byte at a time for ever, so this is the last resort, and it leaves room for `URL_MOST` over a slow line.
const URL_LONGEST: Duration = Duration::from_secs(300);
/// What an import kept: its file, and the field that names one of its type (`image`, `video`, `audio`).
pub(crate) struct Kept {
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
pub(crate) fn media_of(extension: &str) -> Option<(&'static str, &'static str)> {
    let extension = extension.to_ascii_lowercase();
    MEDIA.iter().find(|(known, ..)| *known == extension).map(|(_, media, field)| (*media, *field))
}

/// A file the project uses: its path in the project (`files/<sha256>.<ext>`), its sha256, and what is measured of it: a picture's or video's
/// size, a video's rate and frames, a picture's alpha (the share of it fully transparent). Its row in project.db holds only what can't be
/// read again from a file here (`Folder::fill`, `Lock::keeps_row`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct File {
    /// It was in `files/` when this was read, listed once for all of them: a stat each is seconds, on Windows, across a project's thousands.
    #[serde(skip)]
    pub here: bool,
    /// project.db holds its row (`lock_files`).
    #[serde(skip)]
    pub row: bool,
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

/// What is measured of a file, kept apart from its row, as ffprobe can always say it again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measured {
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

impl Measured {
    fn of(file: &File) -> Self {
        Self { width: file.width, height: file.height, fps: file.fps, frames: file.frames, alpha: file.alpha, duration_ms: file.duration_ms }
    }
}

impl File {
    /// What `measured` says is measured of these bytes, on this row.
    fn measured_as(&mut self, measured: &File) {
        (self.width, self.height, self.fps, self.frames, self.alpha, self.duration_ms) =
            (measured.width, measured.height, measured.fps, measured.frames, measured.alpha, measured.duration_ms);
    }

    fn extension(&self) -> String {
        Path::new(&self.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
    }

    /// What it is, `image`, `video` or `audio`, if its name says.
    pub(crate) fn media(&self) -> Option<&'static str> {
        media_of(&self.extension()).map(|(_, field)| field)
    }

    /// The role that pins it, by what it is.
    pub(crate) fn field(&self) -> &'static str {
        media_of(&self.extension()).map_or(if self.fps > 0.0 { "video" } else { "image" }, |(_, field)| field)
    }

    /// Whether a machine was left keeping it.
    pub(crate) fn stays(&self) -> bool {
        !self.machine.is_empty()
    }

    /// Its row would be empty: nothing is kept of where else it is, or what its name ends in.
    fn says_nothing(&self) -> bool {
        self.parts.is_empty() && self.ext.is_empty() && self.machine.is_empty() && self.length == 0
    }

    /// How long it lasts, if it does and that is measured.
    pub(crate) fn lasts_ms(&self) -> u64 {
        match self.fps > 0.0 {
            true => (self.frames as f64 / self.fps * 1000.0).round() as u64,
            false => self.duration_ms,
        }
    }

    /// Not measured yet, and one that can be.
    pub(crate) fn unmeasured(&self) -> bool {
        match media_of(&self.extension()) {
            Some((_, "audio")) => self.duration_ms == 0,
            Some(_) => self.width == 0,
            None => false,
        }
    }
}

/// A version's files: by output name, each one's sha256.
pub(crate) type Files = BTreeMap<String, String>;
/// A file's metadata, by its output name as in `Files`: what its function declared, or a person set (`orbit`).
pub(crate) type Metadata = BTreeMap<String, serde_json::Map<String, serde_json::Value>>;

/// One result of a tool: its number, its files and the key it was made under.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Version {
    /// Stable identity independent of the local display number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// 1 first, never reused.
    #[serde(default)]
    pub number: u32,
    /// Its key, 16 hex digits: what says it is out of date. Empty for one whose inputs can't be known: it never reads as current.
    #[serde(default)]
    pub inputs: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: Files,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: Metadata,
    /// The seed Calcifer drew. None: its function takes no seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The last run that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
    /// The globally stable run identity; the number remains a local display label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<api::ids::RunId>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub execution_ms: u64,
    /// What made it, `org/package/function@release`, as the asset's tool can change after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The take it picked up from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<api::calcifer::project::Pickup>,
    /// An Edit's: the clips that made it, each with its file, and their rate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<api::calcifer::project::Clip>,
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
    /// The actual structured generation request, retained independently of its run's history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Version {
    /// The file people mean by it: its video, else its picture, else its sound, else its first.
    pub(crate) fn main(&self) -> Option<&String> {
        main_of(&self.files)
    }

    /// The output name of that file.
    pub(crate) fn main_output(&self) -> Option<&String> {
        let main = self.main()?;
        self.files.iter().find(|(_, sha256)| *sha256 == main).map(|(output, _)| output)
    }

    /// What the file `sha256` of it says of itself; empty when nothing.
    pub(crate) fn metadata_of(&self, sha256: &str) -> serde_json::Map<String, serde_json::Value> {
        let output = self.files.iter().find(|(_, held)| *held == sha256).map(|(output, _)| output);
        output.and_then(|output| self.metadata.get(output)).cloned().unwrap_or_default()
    }
}

/// The file people mean by a set of files.
pub(crate) fn main_of(files: &Files) -> Option<&String> {
    ["video", "image", "audio"].iter().find_map(|name| files.get(*name)).or_else(|| files.values().next())
}

/// A piece a cut was made without, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Without {
    pub piece: String,
    pub why: String,
    /// Its file wasn't on this computer: once it is, that version isn't its cut's state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
}

impl Without {
    /// As a film says it: `segment_3: no version yet`.
    pub(crate) fn said(&self) -> String {
        format!("{}: {}", self.piece, self.why)
    }
}

/// A run making a version, recorded before it is sent (under its idempotency key) and followed
/// until it lands: whatever stops the walk, the next one follows it and never sends it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    /// What its asset is, as its version is listed when it lands.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The job it is of: the key of its first call, when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job: String,
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Asset {
    /// Stable identity shared with the written box; a rename preserves it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
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
    /// When a delete set it aside: Undo brings it back until the window ends, then its files leave the project (cl-919, `purge`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_ms: Option<i64>,
    /// The file its box showed of its own when it was deleted, which no version holds: an imported picture's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
}

/// What stopped the last try at it, on `inputs`: kept until a version lands or it's tried again, so a reader who wasn't watching hears it too.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Failed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
    pub error: Error,
}

/// The seed last sent for `inputs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tried {
    pub inputs: String,
    pub seed: u64,
}

impl Asset {
    pub(crate) fn version(&self, number: u32) -> Option<&Version> {
        self.versions.iter().find(|version| version.number == number)
    }

    /// Its versions and those discarded.
    pub(crate) fn on_record(&self) -> impl Iterator<Item = &Version> {
        self.versions.iter().chain(&self.discarded)
    }

    /// Every file it holds: its versions', discarded too, and the one its box showed of its own.
    pub(crate) fn files(&self) -> impl Iterator<Item = &String> {
        self.on_record().flat_map(|version| version.files.values()).chain(&self.held)
    }

    /// A delete set it aside and its undo window ended by `now`: nothing brings it back.
    pub(crate) fn lapsed(&self, now: i64) -> bool {
        self.deleted_ms.is_some_and(|at| at + super::purge::window_ms() <= now)
    }

    /// The words version `number` was made from: its own, else those of the latest before it that holds any.
    pub(crate) fn text_of(&self, number: u32) -> Option<&str> {
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
    pub(crate) fn list(&mut self, mut version: Version) {
        if version.id.is_empty() {
            version.id = self.version(version.number).filter(|held| !held.id.is_empty()).map_or_else(super::new_key, |held| held.id.clone());
        }
        let mut texts = self.texts();
        texts.extend(version.text.take().map(|text| (version.number, text)));
        self.versions.retain(|held| held.number != version.number);
        self.versions.push(version);
        self.versions.sort_by_key(|version| version.number);
        self.keep_texts(texts);
    }

    /// What's discarded gone for good but `read`, which a text names: what comes after it keeps its words.
    pub(crate) fn clear_discarded(&mut self, read: Option<&BTreeSet<u32>>) {
        let texts = self.texts();
        self.discarded.retain(|version| read.is_some_and(|read| read.contains(&version.number)));
        self.keep_texts(texts);
    }

    /// Only `read` left of what's on record: what a text still names of a deleted asset.
    pub(crate) fn keep_only(&mut self, read: &BTreeSet<u32>) {
        let texts = self.texts();
        self.versions.retain(|version| read.contains(&version.number));
        self.discarded.retain(|version| read.contains(&version.number));
        self.keep_texts(texts);
    }

    /// Version `number` off the list and on record.
    pub(crate) fn discard(&mut self, number: u32) {
        self.numbered = self.numbered.max(number);
        if let Some(at) = self.versions.iter().position(|version| version.number == number) {
            self.discarded.push(self.versions.remove(at));
            self.discarded.sort_by_key(|version| version.number);
        }
    }

    /// What `discard` set aside of `number`, back in its place.
    pub(crate) fn undiscard(&mut self, number: u32) {
        if let Some(at) = self.discarded.iter().position(|version| version.number == number) {
            self.versions.push(self.discarded.remove(at));
            self.versions.sort_by_key(|version| version.number);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Lock {
    pub format: u32,
    /// Every file, by sha256.
    #[serde(default)]
    pub files: BTreeMap<String, File>,
    /// By name key. An asset project.yaml no longer names is kept, its versions with it, until it is
    /// renamed or reclaimed.
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
    /// By idempotency key. Never in project.lock, which a project copy carries: `Folder::lock` reads it from `WALK`.
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

/// One file a `cozy run` left in the project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunFile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<api::ids::RunId>,
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
pub(crate) struct Owed {
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
pub(crate) struct WalkFile {
    /// The project and Calcifer it is of: a copy of the project, which has its own id, or another Calcifer, doesn't go on with paid
    /// work. A moved folder is the same project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<Bound>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default)]
    pub walk: Owed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Bound {
    /// The project's id; empty in a walk from before projects kept one, which goes on where it is.
    #[serde(default)]
    pub project: String,
    pub calcifer: String,
}

impl Bound {
    /// Of another project than `here` (a copy, which has its own id) or another Calcifer's: what it was doing isn't `here`'s.
    fn elsewhere(&self, here: &Bound) -> bool {
        self.calcifer != here.calcifer || (!self.project.is_empty() && self.project != here.project)
    }
}

/// The board a render ended on, with the project and Calcifer whose render it was.
#[derive(Serialize, Deserialize)]
struct KeptBoard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound: Option<Bound>,
    board: api::calcifer::project::Board,
}

/// This Calcifer's id, the same across its restarts, once it has said it.
static CALCIFER: std::sync::OnceLock<String> = std::sync::OnceLock::new();


/// Reads this Calcifer's id from `dir`, or makes it. Before any project is read.
pub(crate) fn bind_calcifer(dir: &Path) -> Result<(), Error> {
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
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.walk.is_empty()
    }
}

impl Pending {
    /// The job it is of, under the key `key` it is sent with.
    pub(crate) fn job_of<'a>(&'a self, key: &'a str) -> &'a str {
        if self.job.is_empty() { key } else { &self.job }
    }
}

/// One render: project.yaml as it was when asked, and what it owes. It plans on that text alone, so an edit after it touches nothing
/// of it, and a restart resumes it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ask {
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
    pub(crate) fn is_empty(&self) -> bool {
        self.asks.is_empty() && self.asked.is_empty() && self.renames.is_empty() && self.runs.is_empty()
    }
}

/// A version listed before its files have landed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Coming {
    pub name: String,
    pub number: u32,
    pub take: bool,
}

/// The machines a version may be made on, and the Hub its function is read from.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Target {
    pub on: Vec<On>,
    pub hub: String,
}

/// One new version of `asset`, sent under `key`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Take {
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
    pub pickup: Option<api::calcifer::project::Pickup>,
}

impl Lock {
    /// The files whose hash starts with `prefix`.
    pub(crate) fn matching(&self, prefix: &str) -> Vec<&File> {
        let from = format!("sha256:{prefix}");
        self.files.range(from.clone()..).take_while(|(sha256, _)| sha256.starts_with(&from)).map(|(_, file)| file).collect()
    }

    /// Whether its walk owes anything: asked and not sent, or sent and not landed.
    pub(crate) fn owes(&self) -> bool {
        !self.walk.asks.is_empty() || !self.pending.is_empty() || !self.walk.renames.is_empty() || !self.walk.runs.is_empty()
    }

    /// The number the next version of `name` would take, as far as what's listed says.
    pub(crate) fn listed_number(&self, name: &str) -> u32 {
        self.asset(name).map_or(0, |held| held.numbered.max(held.versions.iter().map(|version| version.number).max().unwrap_or(0)))
    }

    pub(crate) fn asset(&self, name: &str) -> Option<&Asset> {
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
    pub(crate) fn called(&mut self, from: &str, to: &str) {
        let (key, new) = (name_key(from), name_key(to));
        if key == new {
            return;
        }
        self.walk.renames.retain(|(was, _)| name_key(was) != new);
        self.walk.renames.iter_mut().filter(|(_, now)| name_key(now) == key).for_each(|(_, now)| to.clone_into(now));
        self.walk.renames.push((from.to_owned(), to.to_owned()));
    }

    pub(crate) fn asset_mut(&mut self, name: &str, kind: &str) -> &mut Asset {
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
    pub(crate) fn number(&mut self, name: &str, kind: &str) -> u32 {
        let asset = self.asset_mut(name, kind);
        asset.numbered = asset.numbered.max(asset.versions.iter().map(|version| version.number).max().unwrap_or(0)) + 1;
        asset.numbered
    }

    /// `files` as `name`'s next version, unless a version holds that file already: its number, and whether it did. It made none of it, so it never reads as
    /// current, and it says no words.
    pub(crate) fn joined(&mut self, name: &str, kind: &str, files: Files) -> (u32, bool) {
        if let Some(held) = self.asset(name).and_then(|held| held.versions.iter().find(|version| version.main() == main_of(&files))) {
            return (held.number, true);
        }
        let number = self.number(name, kind);
        self.asset_mut(name, kind).list(Version { number, files, text: Some(String::new()), ..Default::default() });
        (number, false)
    }

    /// The rows of a name key, and those `tombstone` set aside of it.
    pub(crate) fn rows_of<'a>(&'a self, key: &str) -> impl Iterator<Item = (&'a String, &'a Asset)> {
        let (key, buried) = (key.to_owned(), format!("{key}{TOMBSTONE}"));
        self.assets.iter().filter(move |(at, _)| **at == key || at.starts_with(&buried))
    }

    /// The lock as it's written, each file's row only where it `keeps_row`.
    pub(crate) fn written(&self) -> String {
        let recorded = self.recorded();
        let mut value = serde_json::to_value(self).expect("a lock serializes");
        if let Some(files) = value["files"].as_object_mut() {
            files.retain(|sha256, _| self.files.get(sha256).is_some_and(|file| Self::keeps_row(sha256, file, &recorded)));
        }
        api::yaml::from_json(&value)
    }

    /// Whether a file has a row: for what can't be read again from a file here (where it stays, its parts), or the whole hash of one no
    /// version or run records, which a box names only the start of (`approved`, a pin), so it's known once the file is gone from `files/`.
    fn keeps_row(sha256: &str, file: &File, recorded: &BTreeSet<String>) -> bool {
        !file.says_nothing() || !recorded.contains(sha256)
    }

    /// Each file's `row` as this lock is written: what project.db keeps once it commits (`db::commit`).
    pub(crate) fn mark_rows(&mut self) {
        let recorded = self.recorded();
        for (sha256, file) in &mut self.files {
            file.row = Self::keeps_row(sha256, file, &recorded);
        }
    }

    /// Each asset whose name the file has lost and with nothing on its way, set aside under a key no name can be, so that a later asset of that
    /// name starts with no versions. It stays an orphan until its undo window ends (`purge`), and renaming it in the browser still finds it.
    pub(crate) fn tombstone(&mut self, named: impl Fn(&str) -> bool) -> bool {
        let going: BTreeSet<String> = (self.pending.values().map(|pending| name_key(&pending.asset)))
            .chain(self.walk.asks.iter().flat_map(|ask| ask.takes.iter().map(|take| name_key(&take.asset))))
            .collect();
        let lost: Vec<String> = self.assets.keys().filter(|key| !key.contains(TOMBSTONE) && !named(key) && !going.contains(*key)).cloned().collect();
        lost.iter().for_each(|key| self.bury(key));
        !lost.is_empty()
    }

    /// The row under `key` set aside, deleted now: what it holds stays, for Undo, until its window ends (`purge`), and that name starts with none.
    pub(crate) fn bury(&mut self, key: &str) {
        let Some(mut held) = self.assets.remove(key).filter(|held| held.files().next().is_some()) else { return };
        if held.name.is_empty() {
            held.name = key.to_owned();
        }
        held.deleted_ms = Some(super::now_ms());
        let free = (1..).map(|n| format!("{key}{TOMBSTONE}{n}")).find(|at| !self.assets.contains_key(at)).expect("a free key");
        self.assets.insert(free, held);
    }

    /// Every sha256 a version or a run has on record here, of any asset, discarded too: all `reclaim` may delete, once nothing uses it.
    pub(crate) fn recorded(&self) -> BTreeSet<String> {
        let discarded = self.assets.values().flat_map(|held| &held.discarded).flat_map(|version| version.files.values()).map(String::as_str);
        let held = self.assets.values().filter_map(|row| row.held.as_deref());
        self.named(|_| true).into_iter().chain(discarded).chain(held).chain(self.runs.iter().map(|row| row.sha256.as_str())).map(str::to_owned).collect()
    }

    /// The latest row `tombstone` set aside of `name` that Undo can still bring back, back under its name, if nothing holds that. Whether one came back.
    pub(crate) fn untombstone(&mut self, name: &str) -> bool {
        self.restore(name, "")
    }

    /// Box `id`'s row a delete set aside (any of `name`'s with no id), back under `name` as it was before, if its window hasn't ended and
    /// nothing holds that name. Whether one came back.
    pub(crate) fn restore(&mut self, name: &str, id: &str) -> bool {
        let key = name_key(name);
        if self.assets.contains_key(&key) {
            return false;
        }
        let now = super::now_ms();
        let prefix = format!("{key}{TOMBSTONE}");
        let at = self.assets.iter()
            .filter(|(at, held)| at.starts_with(&prefix) && (id.is_empty() || held.id == id) && !held.lapsed(now) && held.files().next().is_some())
            .max_by_key(|(at, _)| at[prefix.len()..].parse::<u32>().unwrap_or(0))
            .map(|(at, _)| at.clone());
        let Some(mut held) = at.and_then(|at| self.assets.remove(&at)) else { return false };
        (held.deleted_ms, held.held) = (None, None);
        if held.on_record().next().is_some() {
            self.assets.insert(key, held);
        }
        true
    }

    /// Box `name` deleted now: its row and `pin`, the file it showed of its own, set aside for Undo under `id`, its box's.
    pub(crate) fn set_aside(&mut self, name: &str, id: &str, pin: Option<String>) {
        let key = self.row_of(name);
        if pin.is_some() || self.assets.contains_key(&key) {
            let row = self.asset_mut(name, "");
            if row.id.is_empty() {
                id.clone_into(&mut row.id);
            }
            row.held = pin.filter(|pin| !row.on_record().any(|version| version.files.values().any(|file| file == pin)));
        }
        self.bury(&key);
    }

    /// The tombstone row a delete of box `id` (else of `name`) set aside, the latest.
    pub(crate) fn deleted_box(&self, name: &str, id: &str) -> Option<&Asset> {
        let prefix = format!("{}{TOMBSTONE}", name_key(name));
        (self.assets.iter())
            .filter(|(at, held)| at.starts_with(&prefix) && held.deleted_ms.is_some() && (id.is_empty() || held.id == id))
            .max_by_key(|(at, _)| at[prefix.len()..].parse::<u32>().unwrap_or(0))
            .map(|(_, held)| held)
    }

    /// Whether `name` is a box deleted from the project that Undo can still bring back.
    pub(crate) fn was_deleted(&self, name: &str, now: i64) -> bool {
        self.deleted_box(name, "").is_some_and(|held| !held.lapsed(now) && held.files().next().is_some())
    }

    /// `sha256` is on `machine`, and not here: for good, or with `name`, on its way here as the file that makes.
    pub(crate) fn away(&mut self, sha256: &str, machine: &str, length: u64, name: Option<&str>) {
        let path = file_of(sha256, Path::new(name.unwrap_or_default()));
        let ext = path.rsplit_once('.').map(|(_, ext)| format!(".{ext}")).unwrap_or_default();
        let held = self.files.entry(sha256.to_owned()).or_default();
        if name.is_none() || !held.stays() {
            (held.machine, held.length) = (machine.to_owned(), length);
            if name.is_some() {
                held.ext = ext;
            }
        }
        // Whole as `fill` makes a row: a reader bound before the next fill finds its hash and where it lands (cl-815).
        held.sha256 = sha256.to_owned();
        if !held.here {
            held.path = format!("{FILES}/{}{}", hex_of(sha256), held.ext);
        }
    }

    /// Every sha256 a version or pending run of a kept asset names; `keep` says which assets are kept, by key.
    pub(crate) fn named(&self, keep: impl Fn(&str) -> bool) -> BTreeSet<&str> {
        let versions = (self.assets.iter().filter(|(key, _)| keep(key))).flat_map(|(_, asset)| &asset.versions);
        let pending = self.pending.values().map(|pending| &pending.version);
        versions.chain(pending).flat_map(|version| version.files.values()).map(String::as_str).collect()
    }
}

/// What this Calcifer holds of one project folder, whichever `Folder` names it, for as long as one does: the last to go closes it (cl-823).
struct Shared {
    /// Captured logical ownership and canonical physical view; a reused drive is another owner.
    key: (PathBuf, PathBuf, Option<super::vault::MountIdentity>),
    /// One writer at a time, with the connection that writes: an act, or a write of no act's, holds it from what it changes to its commit,
    /// SQLite's wait on another program's write lock too. Nothing that only reads waits on it (cl-837).
    writing: Mutex<Option<rusqlite::Connection>>,
    /// Held while a file is kept and while files are deleted, so one isn't deleted between.
    collecting: Mutex<()>,
    /// What `project.db` holds, read once and kept as each commit leaves it. Held only to look at or swap, never through a wait on SQLite.
    held: Mutex<Option<Held>>,
    /// A connection that only reads: what `held` doesn't keep, and whether another connection committed. In WAL a read waits on no writer.
    reading: Mutex<Option<rusqlite::Connection>>,
    /// A checkpoint is going.
    checkpointing: std::sync::atomic::AtomicBool,
    /// Its history, read from project.db once.
    book: Mutex<Option<Arc<super::history::Book>>>,
    /// What ffprobe last said of each film at its root.
    films: Mutex<HashMap<PathBuf, Frames>>,
}

/// How many frames a film holds, as ffprobe said of it at its size and time.
type Frames = ((u64, std::time::SystemTime), Option<u64>);

impl Drop for Shared {
    /// Nothing names the folder now: project.db's connections close, beside whatever dropped it, as closing them may checkpoint its
    /// WAL. What the project held is freed there too, and its memory given back to the OS (cl-870): without that, Calcifer stays as
    /// large as the project made it.
    fn drop(&mut self) {
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if shared.get(&self.key).is_some_and(|held| held.strong_count() == 0) {
            shared.remove(&self.key);
        }
        drop(shared);
        let writing = self.writing.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        let reading = self.reading.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        let held = self.held.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        let book = self.book.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        let films = std::mem::take(self.films.get_mut().unwrap_or_else(|e| e.into_inner()));
        if writing.is_none() && reading.is_none() && held.is_none() {
            return;
        }
        let dir = self.key.1.clone();
        std::thread::spawn(move || {
            let resident = || platform::contain::host_memory::process(std::process::id()).map_or(0, |memory| memory.rss) >> 20;
            let before = (resident(), platform::heap::committed() >> 20);
            drop((writing, reading, held, book, films));
            platform::heap::give_back();
            tracing::info!(
                "{} closed: resident {} → {} MiB, allocator committed {} → {} MiB, in use {} MiB",
                dir.display(),
                before.0,
                resident(),
                before.1,
                platform::heap::committed() >> 20,
                platform::heap::held() >> 20,
            );
        });
    }
}

struct Held {
    /// `PRAGMA data_version` of `reading` as `stored` was read or committed: another connection's commit moves it, and it's read again.
    seen: i64,
    /// How many times it was read again for another connection's commit.
    loads: u64,
    stored: Stored,
    /// How `files/` stood when the lock's table was filled from it.
    files: String,
    /// Commits since the last checkpoint began.
    commits: u32,
    /// Acts that changed what a read shows and nothing project.db keeps (a file here, what's measured on its row): with each, the revision
    /// moves too, or a read worked out once per revision would go on showing what was.
    shown: u64,
}

/// Each project folder some `Folder` names now, by its canonical path: one `Shared` while any does.
static SHARED: LazyLock<Mutex<HashMap<(PathBuf, PathBuf, Option<super::vault::MountIdentity>), std::sync::Weak<Shared>>>> = LazyLock::new(Mutex::default);

fn shared(dir: &Path) -> Arc<Shared> {
    let dir = durable::canonical(dir).unwrap_or_else(|_| dir.to_owned());
    let owner = super::vault::logical(&dir);
    let identity = super::vault::mount_identity(&owner);
    let key = (owner, dir, identity);
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(held) = shared.get(&key).and_then(std::sync::Weak::upgrade) {
        return held;
    }
    let held = Arc::new(Shared {
        key: key.clone(),
        writing: Mutex::default(),
        collecting: Mutex::default(),
        held: Mutex::default(),
        reading: Mutex::default(),
        checkpointing: std::sync::atomic::AtomicBool::default(),
        book: Mutex::default(),
        films: Mutex::default(),
    });
    shared.insert(key, Arc::downgrade(&held));
    held
}

/// A write that took this long, waiting and held, is said, with how long each took.
const SLOW_MS: f64 = 5.0;
/// How many commits go to the WAL before a checkpoint moves them into the database, beside the writes.
const CHECKPOINT_EVERY: u32 = 200;

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// One act: what it has changed so far of each part, over what was committed when it began. A part it hasn't touched is read from `base`.
struct Acting {
    id: usize,
    base: Stored,
    written: RefCell<Option<Written>>,
    canvas: RefCell<Option<BTreeMap<String, CardRow>>>,
    lock: RefCell<Option<Lock>>,
    walk: RefCell<Option<WalkFile>>,
    measured: RefCell<Option<BTreeMap<String, Measured>>>,
    extra: RefCell<Extra>,
    made: RefCell<Vec<Made>>,
    after: RefCell<Vec<Box<dyn FnOnce(u64)>>>,
}

thread_local! {
    /// The acts this thread is in, innermost last.
    static ACTING: RefCell<Vec<Rc<Acting>>> = const { RefCell::new(Vec::new()) };
}

/// This thread in an act until it's dropped, however the act ends.
struct In(Rc<Acting>);

impl In {
    fn enter(act: Rc<Acting>) -> Self {
        ACTING.with(|acting| acting.borrow_mut().push(act.clone()));
        Self(act)
    }
}

impl Drop for In {
    fn drop(&mut self) {
        ACTING.with(|acting| acting.borrow_mut().retain(|held| !Rc::ptr_eq(held, &self.0)));
    }
}

/// The act on `id` this thread is in, if one.
fn acting(id: usize) -> Option<Rc<Acting>> {
    ACTING.with(|acting| acting.borrow().iter().rev().find(|held| held.id == id).cloned())
}

/// One of a project's parts, as an act reads and writes it.
macro_rules! part {
    ($read:ident, $write:ident, $field:ident, $ty:ty) => {
        #[allow(dead_code)]
        fn $read<R>(&self, f: impl FnOnce(&$ty) -> R) -> Result<R, Error> {
            if let Some(act) = acting(self.id()) {
                let held = act.$field.borrow();
                return Ok(f(held.as_ref().unwrap_or(&*act.base.$field)));
            }
            let held = self.held()?;
            Ok(f(&*held.as_ref().expect("held").stored.$field))
        }

        fn $write<R>(&self, f: impl FnOnce(&mut $ty) -> Result<R, Error>) -> Result<R, Error> {
            self.act(|| {
                let act = acting(self.id()).expect("in its act");
                let mut held = act.$field.borrow_mut();
                f(held.get_or_insert_with(|| (*act.base.$field).clone()))
            })
        }
    };
}

/// A history change a write made, in the order made.
pub(super) enum Made {
    Written(Written, Written),
    Change(super::history::Change),
}

pub(crate) struct Folder {
    pub dir: PathBuf,
    /// cozy's tools folder, where its ffprobe is (else PATH's).
    pub tools: PathBuf,
    shared: Arc<Shared>,
    /// Why what's landing can't go in just now. It's owed in the walk, and said as a warning until it has.
    unsaved: Mutex<Option<String>>,
}

impl Folder {
    /// Release cached connections before unmounting or moving the storage. The caller has
    /// stopped admissions and checked background work; an in-flight DB operation refuses it.
    pub(crate) fn close_connections(&self, checkpoint: bool) -> Result<(), Error> {
        let busy = || Error::new(api::error::Kind::Conflict, "project_busy", "a project database operation is still running");
        if self.shared.checkpointing.load(std::sync::atomic::Ordering::Acquire) { return Err(busy()); }
        let mut writer = self.shared.writing.try_lock().map_err(|_| busy())?;
        let mut reader = self.shared.reading.try_lock().map_err(|_| busy())?;
        let reading = reader.take();
        drop(reading);
        if checkpoint {
            let extra;
            let conn = match writer.as_ref() {
                Some(conn) => conn,
                None => {
                    extra = rusqlite::Connection::open_with_flags(self.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                        .map_err(|e| Error::new(api::error::Kind::Unavailable, "vault_database", format!("open project checkpoint: {e}")))?;
                    &extra
                }
            };
            let occupied: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
                .map_err(|e| Error::new(api::error::Kind::Unavailable, "vault_database", format!("checkpoint project before closing: {e}")))?;
            if occupied != 0 { return Err(busy()); }
        }
        let writing = writer.take();
        *self.shared.held.lock().expect("held lock") = None;
        drop(reader);
        drop(writer);
        drop(writing);
        *self.shared.book.lock().expect("history cache") = None;
        self.shared.films.lock().expect("film cache").clear();
        Ok(())
    }

    /// `stamp` of the project in `dir` while anything holds it. Of one nothing does, its transactions' count, read with nothing opened:
    /// another than any it had open, and it moves with each commit.
    pub(crate) fn stamp_at(dir: &Path) -> String {
        let canonical = durable::canonical(dir).unwrap_or_else(|_| dir.to_owned());
        let owner = super::vault::logical(&canonical);
        let identity = super::vault::mount_identity(&owner);
        let key = (owner, canonical, identity);
        let held = SHARED.lock().unwrap_or_else(|e| e.into_inner()).get(&key).and_then(std::sync::Weak::upgrade);
        match held {
            Some(shared) => Self { dir: dir.to_owned(), tools: PathBuf::new(), shared, unsaved: Mutex::default() }.stamp(),
            None => db::seq_at(&dir.join(db::DB)).map_or_else(String::new, |seq| seq.to_string()),
        }
    }

    /// The project in `dir` as Calcifer holds it, else opened for as long as this lives: for what lands from outside a walk.
    pub(crate) fn at(dir: &Path) -> Self {
        Self { dir: dir.to_owned(), tools: PathBuf::new(), shared: shared(dir), unsaved: Mutex::default() }
    }

    /// The current native project in `dir`, measured with ffprobe from `home`'s tools (else PATH).
    pub(crate) fn open(dir: &Path, home: &Home) -> Result<Self, Error> {
        Self::open_with(dir, home, |_| Ok(())).map(|(folder, ())| folder)
    }

    /// Inspect the loaded canonical state before resuming any local work. In particular a
    /// copy must receive its own identity before deciding whether its inherited walk belongs here.
    pub(crate) fn open_with<T>(dir: &Path, home: &Home, inspect: impl FnOnce(&Self) -> Result<T, Error>) -> Result<(Self, T), Error> {
        let folder = Self { dir: dir.to_owned(), tools: home.tools(), shared: shared(dir), unsaved: Mutex::default() };
        drop(folder.held()?);
        let inspected = inspect(&folder)?;
        folder.unbind_elsewhere()?;
        Ok((folder, inspected))
    }

    /// Identity from the same native document used by this open, never a query projection
    /// or another independent decode of its Automerge snapshot.
    pub(crate) fn canonical_identity(&self) -> Result<Option<(String, String)>, Error> {
        let held = self.held()?;
        Ok(held.as_ref().and_then(|held| held.stored.crdt.as_ref()).and_then(|doc| doc.identity()))
    }

    /// The public route code is local metadata, separate from the native project id.
    pub(crate) fn public_code(&self) -> Result<Option<String>, Error> {
        self.reading_db(|conn| db::meta(conn, "code"))
    }

    /// Rare identity changes (first name or a copied project) invalidate the held native view.
    pub(crate) fn set_identity(&self, id: &str, name: &str, code: &str) -> Result<(), Error> {
        let mut turn = self.shared.writing.lock().unwrap_or_else(|e| e.into_inner());
        db::set_identity(self.writer(&mut turn)?, &self.db(), id, name, Some(code))?;
        *self.shared.held.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    /// The board the last render here ended on, as project.db keeps it. A copy's (its original's render) or another Calcifer's is none:
    /// its runs aren't this project's, as what its walk owed isn't (`unbind_elsewhere`).
    pub(crate) fn last_board(&self) -> Option<api::calcifer::project::Board> {
        let kept: KeptBoard = self.reading_db(|conn| db::meta(conn, BOARD)).ok().flatten().and_then(|kept| serde_json::from_str(&kept).ok())?;
        let theirs = kept.bound.as_ref().zip(self.bound()).is_some_and(|(bound, here)| bound.elsewhere(&here));
        (!theirs).then_some(kept.board)
    }

    /// Keeps the board a render ended on, for a read after this project has closed and opened again.
    pub(crate) fn keep_board(&self, board: &api::calcifer::project::Board) -> Result<(), Error> {
        let kept = KeptBoard { bound: self.bound(), board: board.clone() };
        self.write_past(|conn| db::set_meta(conn, &self.db(), BOARD, Some(&serde_json::to_string(&kept).expect("a board serializes"))))
    }

    /// The machines its runs go to (cl-873), each by name; none: any machine. A local setting, as its route code is: not the
    /// project's document, so no history step or export carries it.
    pub(crate) fn machines(&self) -> Vec<String> {
        let kept = self.reading_db(|conn| db::meta(conn, MACHINES)).ok().flatten();
        kept.and_then(|kept| serde_json::from_str(&kept).ok()).unwrap_or_default()
    }

    pub(crate) fn set_machines(&self, machines: &[String]) -> Result<(), Error> {
        let kept = (!machines.is_empty()).then(|| serde_json::to_string(machines).expect("names serialize"));
        self.write_past(|conn| db::set_meta(conn, &self.db(), MACHINES, kept.as_deref()))
    }

    /// Whether another program holds project.db's write lock, asked this moment with no wait. None while a write of Calcifer's has the
    /// turn, which finds out.
    pub(crate) fn held_elsewhere(&self) -> Option<bool> {
        let mut turn = self.shared.writing.try_lock().ok()?;
        self.writer(&mut turn).ok().map(|conn| db::locked_elsewhere(conn))
    }

    /// Recognize native storage or retired markers so a legacy folder receives a precise
    /// unsupported-format refusal rather than looking like an ordinary non-project directory.
    pub(crate) fn is_project(dir: &Path) -> bool {
        dir.join(db::DB).is_file() || Self::legacy_files(dir)
    }

    pub(crate) fn legacy_files(dir: &Path) -> bool {
        [PROJECT, LOCK, "project.canvas.yaml", "project.db.new"].iter().any(|name| dir.join(name).exists())
            || dir.join(".cozy-legacy-v1").exists()
    }

    pub(crate) fn missing_native(dir: &Path) -> Error {
        if Self::legacy_files(dir) {
            Error::new(Kind::Validation, "project_format_unsupported", format!("{} holds retired or incomplete project files; a current native CRDT project.db is required", dir.display()))
                .remedy("Create a new project directory; import current-format authoring YAML explicitly into that native project")
        } else {
            Error::new(Kind::NotFound, "project_not_found", format!("{} holds no {}", dir.display(), db::DB)).next("cozy new")
        }
    }

    fn id(&self) -> usize {
        Arc::as_ptr(&self.shared) as usize
    }

    /// Its history, as `read` first reads it, for as long as anything holds the project.
    pub(super) fn book(&self, read: impl FnOnce() -> super::history::Book) -> Arc<super::history::Book> {
        self.shared.book.lock().expect("history cache").get_or_insert_with(|| Arc::new(read())).clone()
    }

    pub(crate) fn db(&self) -> PathBuf {
        self.dir.join(db::DB)
    }

    /// What's held of `project.db`, read now if it wasn't or another connection wrote it, `files/` filled in again if it moved. It
    /// waits on no writer: a commit holds this only to swap in what it wrote (cl-837).
    fn held(&self) -> Result<MutexGuard<'_, Option<Held>>, Error> {
        if !self.db().is_file() { return Err(Self::missing_native(&self.dir)); }
        let mut held = self.shared.held.lock().unwrap_or_else(|e| e.into_inner());
        let mut reading = self.shared.reading.lock().unwrap_or_else(|e| e.into_inner());
        let conn = match &mut *reading {
            Some(conn) => conn,
            none => none.insert(db::open(&self.db())?),
        };
        if held.as_ref().is_none_or(|held| db::data_version(conn) != held.seen) {
            let (seen, mut stored) = db::read(conn, &self.db())?;
            let files = super::stamp(&self.dir.join(FILES));
            self.fill(Arc::make_mut(&mut stored.lock), &stored.measured);
            let loads = held.as_ref().map_or(0, |held| held.loads + 1);
            *held = Some(Held { seen, loads, stored, files, commits: 0, shown: 0 });
        }
        drop(reading);
        let now = super::stamp(&self.dir.join(FILES));
        if let Some(held) = held.as_mut().filter(|held| held.files != now) {
            let measured = held.stored.measured.clone();
            self.fill(Arc::make_mut(&mut held.stored.lock), &measured);
            held.files = now;
        }
        Ok(held)
    }

    /// Native CRDT conflicts are kept visible even though an ordinary value read chooses one.
    pub(crate) fn crdt_conflicts(&self) -> Result<Vec<String>, Error> {
        let held = self.held()?;
        Ok(held.as_ref().and_then(|held| held.stored.crdt.as_ref()).map_or_else(Vec::new, |doc| doc.conflicts()))
    }

    /// The revision: one more with each commit.
    pub(crate) fn seq(&self) -> u64 {
        self.held().ok().and_then(|held| held.as_ref().map(|held| held.stored.seq)).unwrap_or(0)
    }

    /// What moves with each commit, this Calcifer's or another connection's that it read again, and with each act that changed what a
    /// read shows and committed nothing.
    pub(crate) fn stamp(&self) -> String {
        self.held().ok().and_then(|held| held.as_ref().map(|held| format!("{}.{}.{}", held.stored.seq, held.loads, held.shown))).unwrap_or_default()
    }

    /// `change` as one transaction: every write it makes to this project, its history entry and its request's key commit together, or
    /// nothing does. Inside another act of this project, it is part of that one.
    pub(crate) fn act<R>(&self, change: impl FnOnce() -> Result<R, Error>) -> Result<R, Error> {
        let id = self.id();
        if acting(id).is_some() {
            return change();
        }
        let asked = Instant::now();
        let mut turn = self.shared.writing.lock().unwrap_or_else(|e| e.into_inner());
        let (began, wait_ms) = (Instant::now(), ms(asked));
        let (base, files, seen) = {
            let held = self.held()?;
            let held = held.as_ref().expect("held");
            (held.stored.clone(), held.files.clone(), held.seen)
        };
        let act = Rc::new(Acting {
            id,
            base,
            written: RefCell::default(),
            canvas: RefCell::default(),
            lock: RefCell::default(),
            walk: RefCell::default(),
            measured: RefCell::default(),
            extra: RefCell::default(),
            made: RefCell::default(),
            after: RefCell::default(),
        });
        let done = {
            let _in = In::enter(act.clone());
            change()
        };
        let answer = done?;
        let act = Rc::try_unwrap(act).unwrap_or_else(|_| panic!("an act outlives its turn"));
        let mut after = act.base.clone();
        let changed = [act.written.borrow().is_some(), act.canvas.borrow().is_some(), act.lock.borrow().is_some(), act.walk.borrow().is_some(), act.measured.borrow().is_some()];
        if let Some(written) = act.written.into_inner() {
            after.written = Arc::new(written);
        }
        if let Some(mut canvas) = act.canvas.into_inner() {
            let seq = act.base.seq + 1;
            canvas.iter_mut().filter(|(key, row)| act.base.canvas.get(*key) != Some(row)).for_each(|(_, row)| row.seq = seq);
            after.canvas = Arc::new(canvas);
        }
        if let Some(lock) = act.lock.into_inner() {
            after.lock = Arc::new(lock);
        }
        if let Some(walk) = act.walk.into_inner() {
            after.walk = Arc::new(walk);
        }
        if let Some(measured) = act.measured.into_inner() {
            after.measured = Arc::new(measured);
        }
        let mut extra = act.extra.into_inner();
        let entry = super::history::composed(self, act.made.into_inner(), &mut extra);
        if let Some(key) = super::keyed::claimed() {
            extra.key = Some(key);
        }
        if !changed.contains(&true) && extra.ops.is_empty() && extra.ops_gone.is_empty() && extra.asked.is_empty() && !extra.asked_cleared && extra.key.is_none() {
            return Ok(answer);
        }
        // SQLite's wait on another program's write lock holds the turn alone: every read goes on meanwhile (cl-837).
        let conn = self.writer(&mut turn)?;
        let mut current = false;
        // Readers retain the old view throughout preparation and an outside writer's
        // BEGIN wait. Only COMMIT and publication share the held guard: a reader can
        // never see our committed data_version with the cache still on the old view.
        let mut publication = None;
        let seq = db::commit(conn, &self.db(), &act.base, &mut after, &extra, || current = self.version() == seen,
            || publication = Some(self.shared.held.lock().unwrap_or_else(|e| e.into_inner())))?;
        let (seq, shown) = {
            // A no-op rollback does not call before_commit; local-only file facts
            // still use the existing short publication path in that case.
            let mut held = publication.unwrap_or_else(|| self.shared.held.lock().unwrap_or_else(|e| e.into_inner()));
            let held = held.as_mut().expect("held");
            after.seq = seq.unwrap_or(act.base.seq);
            let shown = seq.is_none() && changed[2] && after.lock.files != act.base.lock.files;
            held.shown += u64::from(shown);
            held.stored = after;
            held.files = files;
            // What it began from was all there was, and nothing read again since: this commit is the only change. Else the next read
            // reads it all, with what another connection committed.
            if current && held.seen == seen {
                held.seen = self.version();
            }
            held.commits += u32::from(seq.is_some());
            if held.commits >= CHECKPOINT_EVERY && !self.shared.checkpointing.swap(true, std::sync::atomic::Ordering::AcqRel) {
                held.commits = 0;
                let (shared, path) = (self.shared.clone(), self.db());
                std::thread::spawn(move || {
                    db::checkpoint(&path);
                    shared.checkpointing.store(false, std::sync::atomic::Ordering::Release);
                });
            }
            (seq, shown)
        };
        super::history::committed(self, entry);
        let now = seq.unwrap_or(act.base.seq);
        for told in act.after.into_inner() {
            told(now);
        }
        if (seq.is_some() || shown) && changed.contains(&true) {
            crate::events::project_changed(&self.dir);
        }
        let held_ms = ms(began);
        if wait_ms + held_ms >= SLOW_MS {
            info!(phase = "project_write", seq = seq.unwrap_or_default(), wait_ms, held_ms);
        }
        Ok(answer)
    }

    part!(reading_written, writing_written, written, Written);
    part!(reading_canvas, writing_canvas, canvas, BTreeMap<String, CardRow>);
    part!(reading_bare_lock, writing_bare_lock, lock, Lock);
    part!(reading_walk, writing_walk, walk, WalkFile);
    part!(reading_measured, writing_measured, measured, BTreeMap<String, Measured>);

    /// What a write of this act says in its history, kept for the act's end.
    pub(super) fn made(&self, made: impl FnOnce() -> Made) {
        if let Some(act) = acting(self.id()) {
            act.made.borrow_mut().push(made());
        }
    }

    /// Done once the act this is in commits, given the revision it made: what a stream is told.
    pub(super) fn then(&self, told: impl FnOnce(u64) + 'static) {
        match acting(self.id()) {
            Some(act) => act.after.borrow_mut().push(Box::new(told)),
            None => told(self.seq()),
        }
    }

    /// History rows this act writes, and those it deletes.
    pub(super) fn write_ops(&self, ops: Vec<db::OpRow>, gone: Vec<(String, u64)>) -> Result<(), Error> {
        self.act(|| {
            let act = acting(self.id()).expect("in its act");
            let mut extra = act.extra.borrow_mut();
            extra.ops.extend(ops);
            extra.ops_gone.extend(gone);
            Ok(())
        })
    }

    /// The connection that writes, opened at the first write, once the project's first open has made `project.db`.
    fn writer<'t>(&self, turn: &'t mut Option<rusqlite::Connection>) -> Result<&'t mut rusqlite::Connection, Error> {
        if turn.is_none() {
            drop(self.held()?);
            *turn = Some(db::open(&self.db())?);
        }
        Ok(turn.as_mut().expect("opened"))
    }

    /// `data_version` of the connection that reads: any other connection's commit moves it, the one that writes too.
    fn version(&self) -> i64 {
        self.shared.reading.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, db::data_version)
    }

    /// `read` of what `held` doesn't keep, on the connection that only reads: past every writer.
    fn reading_db<R>(&self, read: impl FnOnce(&rusqlite::Connection) -> R) -> Result<R, Error> {
        drop(self.held()?);
        let reading = self.shared.reading.lock().unwrap_or_else(|e| e.into_inner());
        Ok(read(reading.as_ref().expect("opened")))
    }

    /// `write` in the writing turn, of what `held` doesn't keep: no part of the project moves, so what it holds stays current but for
    /// another connection's commit.
    fn write_past<R>(&self, write: impl FnOnce(&rusqlite::Connection) -> Result<R, Error>) -> Result<R, Error> {
        let mut turn = self.shared.writing.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.writer(&mut turn)?;
        let seen = self.held()?.as_ref().map(|held| held.seen);
        let done = write(conn);
        let mut held = self.shared.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = held.as_mut().filter(|held| Some(held.seen) == seen) {
            held.seen = self.version();
        }
        done
    }

    /// Every history row, oldest first.
    pub(super) fn ops(&self) -> Result<Vec<db::OpRow>, Error> {
        let held = self.held()?;
        held.as_ref().and_then(|held| held.stored.crdt.as_ref())
            .ok_or_else(|| Error::new(Kind::Validation, "project_crdt", "the canonical project document is missing"))?.ops()
    }

    /// What the request under `key` asked and answered here, if it was kept.
    pub(super) fn keyed(&self, key: &str) -> Option<db::Keyed> {
        self.reading_db(|conn| db::keyed(conn, key)).ok().flatten()
    }

    /// The answer the request under `key` gave, kept on its row.
    pub(super) fn keep_answer(&self, key: &str, digest: &str, status: u16, content_type: &str, body: &[u8]) -> Result<(), Error> {
        self.write_past(|conn| db::keep_answer(conn, key, digest, status, content_type, body).map_err(|e| Error::new(Kind::Unavailable, "project_db", e.to_string())))
    }

    /// What the project says: its boxes, as rows.
    pub(crate) fn written(&self) -> Result<Written, Error> {
        self.reading_written(Written::clone)
    }

    /// `seen` of its boxes, with no copy made.
    pub(crate) fn reading_boxes<R>(&self, seen: impl FnOnce(&Written) -> R) -> Result<R, Error> {
        if acting(self.id()).is_some() {
            return self.reading_written(seen);
        }
        let written = {
            let held = self.held()?;
            held.as_ref().expect("held").stored.written.clone()
        };
        // A bounded immutable snapshot, not a cache: building a model from it must
        // not retain the held mutex and delay publication of an independent edit.
        Ok(seen(&written))
    }

    /// The project as YAML, format 4: its export, and what a render keeps of what it was asked.
    pub(crate) fn text(&self) -> Result<String, Error> {
        self.reading_written(Written::to_yaml)
    }

    /// One change to its boxes. The history hears what it was and is.
    pub(crate) fn edit<R>(&self, change: impl FnOnce(&mut Written) -> Result<R, Error>) -> Result<R, Error> {
        self.act(|| {
            let before = self.written()?;
            let answer = self.writing_written(|written| {
                let answer = change(written)?;
                written.in_stored_order();
                Ok(answer)
            })?;
            let after = self.written()?;
            if before != after {
                self.made(|| Made::Written(before, after));
            }
            Ok(answer)
        })
    }

    /// Its canvas rows, by name key.
    pub(crate) fn canvas(&self) -> Result<BTreeMap<String, CardRow>, Error> {
        self.reading_canvas(BTreeMap::clone)
    }

    /// Its canvas rows and the revision they're of, read at once.
    pub(crate) fn canvas_at(&self) -> Result<(u64, BTreeMap<String, CardRow>), Error> {
        let held = self.held()?;
        let stored = &held.as_ref().expect("held").stored;
        Ok((stored.seq, (*stored.canvas).clone()))
    }

    pub(crate) fn edit_canvas<R>(&self, change: impl FnOnce(&mut BTreeMap<String, CardRow>) -> Result<R, Error>) -> Result<R, Error> {
        self.writing_canvas(change)
    }

    /// The history: this and `update_lock` are the one way to it, whatever holds it. What the walk owes is in it.
    pub(crate) fn lock(&self) -> Result<Lock, Error> {
        let mut lock = self.reading_bare_lock(Lock::clone)?;
        let walk = self.reading_walk(WalkFile::clone)?;
        (lock.pending, lock.walk) = (walk.pending, walk.walk);
        Ok(lock)
    }

    /// `seen` of the lock, with no copy made. What the walk owes isn't in it.
    pub(crate) fn reading_lock<R>(&self, seen: impl FnOnce(&Lock) -> R) -> Result<R, Error> {
        self.reading_bare_lock(seen)
    }

    /// Applies `change` to the lock as it is now, what the walk owes among it.
    pub(crate) fn update_lock(&self, change: impl FnOnce(&mut Lock)) -> Result<(), Error> {
        self.act(|| {
            let walk = self.reading_walk(WalkFile::clone)?;
            let moved = self.writing_bare_lock(|lock| {
                (lock.pending, lock.walk) = (walk.pending.clone(), walk.walk.clone());
                change(lock);
                let owed = (std::mem::take(&mut lock.pending), std::mem::take(&mut lock.walk));
                Ok(owed)
            })?;
            let (pending, owed) = moved;
            if serde_json::to_string(&pending).ok() != serde_json::to_string(&walk.pending).ok() || serde_json::to_string(&owed).ok() != serde_json::to_string(&walk.walk).ok() {
                let bound = self.bound();
                self.writing_walk(|walk| {
                    (walk.pending, walk.walk) = (pending, owed);
                    walk.bound = (!walk.is_empty()).then_some(bound).flatten();
                    Ok(())
                })?;
            }
            Ok(())
        })
    }

    /// What the walk owes, with no need to read the rest.
    pub(crate) fn walk_now(&self) -> Result<WalkFile, Error> {
        self.reading_walk(WalkFile::clone)
    }

    /// Applies `change` to what the walk owes, touching nothing of what the project is.
    pub(crate) fn update_walk(&self, change: impl FnOnce(&mut WalkFile)) -> Result<(), Error> {
        let bound = self.bound();
        self.writing_walk(|walk| {
            change(walk);
            walk.bound = (!walk.is_empty()).then_some(bound).flatten();
            Ok(())
        })
    }

    /// Who may go on with this project's walk: this project, under this Calcifer. None where no Calcifer runs, which judges nothing.
    fn bound(&self) -> Option<Bound> {
        let calcifer = CALCIFER.get()?;
        let (project, _) = self.canonical_identity().ok()??;
        Some(Bound { project, calcifer: calcifer.clone() })
    }

    /// A walk of another project (this is a copy) or another Calcifer's doesn't go on here: what it owes is dropped.
    fn unbind_elsewhere(&self) -> Result<(), Error> {
        let Some(here) = self.bound() else { return Ok(()) };
        let elsewhere = self.reading_walk(|walk| walk.bound.as_ref().is_some_and(|bound| bound.elsewhere(&here)))?;
        if !elsewhere {
            return Ok(());
        }
        warn!("{}: its walk is of another project or Calcifer, so it doesn't go on here", self.dir.display());
        self.act(|| {
            self.writing_walk(|walk| {
                *walk = WalkFile::default();
                Ok(())
            })?;
            self.drop_texts();
            Ok(())
        })
    }

    /// The text render `ask` was asked with: kept apart, so what the walk owes stays small.
    pub(crate) fn text_of(&self, ask: &Ask) -> Result<String, Error> {
        if !ask.text.is_empty() {
            return Ok(ask.text.clone());
        }
        if let Some(act) = acting(self.id())
            && let Some((_, Some(text))) = act.extra.borrow().asked.iter().rev().find(|(digest, _)| *digest == ask.digest)
        {
            return Ok(text.clone());
        }
        self.reading_db(|conn| db::asked(conn, &ask.digest))?.ok_or_else(|| Error::new(Kind::NotFound, "ask_text_missing", format!("the text render {} was asked with is gone", ask.id)))
    }

    pub(crate) fn keep_text(&self, digest: &str, text: &str) -> Result<(), Error> {
        self.extra(|extra| extra.asked.push((digest.to_owned(), Some(text.to_owned()))))
    }

    pub(crate) fn drop_text(&self, digest: &str) {
        let _ = self.extra(|extra| extra.asked.push((digest.to_owned(), None)));
    }

    /// No render holds any.
    pub(crate) fn drop_texts(&self) {
        let _ = self.extra(|extra| {
            extra.asked.clear();
            extra.asked_cleared = true;
        });
    }

    fn extra(&self, change: impl FnOnce(&mut Extra)) -> Result<(), Error> {
        self.act(|| {
            change(&mut acting(self.id()).expect("in its act").extra.borrow_mut());
            Ok(())
        })
    }

    /// What says a landing couldn't be listed yet.
    pub(crate) fn unsaved(&self) -> Option<String> {
        self.unsaved.lock().expect("unsaved lock").clone()
    }

    pub(crate) fn set_unsaved(&self, why: Option<String>) {
        *self.unsaved.lock().expect("unsaved lock") = why;
    }

    /// Why the files `named` leaves out may be all that's left of this project's takes, not what's done with: the lock lists no version or
    /// run while `files/` holds what nothing names. `reclaim --yes` deletes none of them.
    pub(crate) fn history_lost(&self, lock: &Lock, named: &BTreeSet<String>) -> Option<String> {
        let unnamed = lock.files.values().filter(|file| !named.contains(&file.sha256) && self.path(&file.path).is_file()).count();
        if unnamed == 0 || !lock.runs.is_empty() || lock.assets.values().any(|held| held.on_record().next().is_some()) {
            return None;
        }
        Some(format!("the project lists no versions, so the {unnamed} files nothing names in {FILES}/ may be all that's left of its takes"))
    }

    /// `lock`'s file table whole, from what needs no record: each file in `files/` names itself, what a version or run names and isn't here has
    /// its hash, and what is measured comes from `measured`, which a read of the file can always make again.
    pub(super) fn fill(&self, lock: &mut Lock, measured: &BTreeMap<String, Measured>) {
        for (sha256, file) in &mut lock.files {
            file.sha256.clone_from(sha256);
            file.path = format!("{FILES}/{}{}", hex_of(sha256), file.ext);
            file.here = false;
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
        for (sha256, held) in measured {
            if let Some(file) = lock.files.get_mut(sha256) {
                (file.width, file.height, file.fps, file.frames, file.alpha, file.duration_ms) = (held.width, held.height, held.fps, held.frames, held.alpha, held.duration_ms);
            }
        }
    }
    /// `source`, a file a run saved, kept: copied whole before it is named, once per content, and in the lock's table.
    pub(crate) fn keep(&self, source: &Path) -> Result<String, Error> {
        drop(self.held()?);
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let sha256 = hash(source)?;
        let path = file_of(&sha256, source);
        self.store(&path, |part| std::fs::copy(source, part).map(drop))?;
        Ok(sha256)
    }

    /// A file a run is given, of `sha256`, kept in `files/` unless it's here, and in the lock's table, under what its bytes say it is
    /// else its own extension: a hard link with `link`, else a copy.
    pub(crate) fn keep_given(&self, source: &Path, sha256: &str, link: bool) -> Result<(), Error> {
        drop(self.held()?);
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
    pub(crate) fn take(&self, source: &Path) -> Result<String, Error> {
        drop(self.held()?);
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let sha256 = hash(source)?;
        let path = file_of(&sha256, source);
        if !self.store(&path, |part| std::fs::rename(source, part))? {
            let _ = std::fs::remove_file(source);
        }
        Ok(sha256)
    }

    /// What a run landed at `dest`, of `digest`, kept in `files/` by hash: a hard link where the filesystem allows. In this order: what's
    /// `measured` of it in project.db, its file in `files/`, then its row as here and its run's listing, in place of what an earlier revision
    /// of that output was, which goes if nothing else names it. So a file here is a file measured.
    /// Unsupported folders are refused before their files are changed; a current project's write
    /// contention can still leave a measured file awaiting its listing.
    pub(crate) fn adopt(&self, dest: &Path, digest: &str, made: RunFile, measured: Option<&File>) -> Result<(PathBuf, Option<Error>), Error> {
        drop(self.held()?);
        let (path, old, unlisted) = {
            let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
            let path = file_of(digest, dest);
            let at = self.dir.join(&path);
            // Held by another program, it's said at once and not waited out: the bytes are kept, and a walk's landing lists them (cl-837).
            let held = (self.held_elsewhere() == Some(true)).then(|| db::held(&self.db()));
            let unmeasured = held.or_else(|| measured.and_then(|measured| self.record_measured(&BTreeMap::from([(digest.to_owned(), measured.clone())])).err()));
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
                if let Some(measured) = measured.filter(|measured| measured.fps > 0.0 && measured.width > 0) {
                    super::thumbs::landed(at.clone(), digest, measured.lasts_ms() as f64 / 1000.0);
                }
            }
            let old = std::cell::RefCell::new(vec![]);
            let listed = |rows: &mut Vec<RunFile>| {
                let earlier = rows.iter().position(|row| row.output == made.output && match (&row.run_id, &made.run_id) {
                    (Some(held), Some(made)) => held == made,
                    (None, None) => row.run == made.run,
                    _ => false,
                });
                match earlier {
                    Some(at) => {
                        let next = RunFile { id: rows[at].id.clone(), ..made.clone() };
                        *old.borrow_mut() = vec![std::mem::replace(&mut rows[at], next).sha256];
                    }
                    None => rows.push(made.clone()),
                }
            };
            // Here as `fill` finds a file in `files/`, with no wait for the next read to list it.
            let here = |lock: &mut Lock| {
                let row = lock.files.entry(digest.to_owned()).or_default();
                (row.sha256, row.path, row.here) = (digest.to_owned(), path.clone(), true);
                (row.ext, row.machine, row.length) = Default::default();
                if let Some(measured) = measured {
                    row.measured_as(measured);
                }
            };
            // A walk's run lands on its version: nothing of the project moves until then. No row goes in without what's measured of it.
            let listed = match unmeasured {
                Some(why) => Err(why),
                None => self.act(|| match self.walk_now()? {
                    walk if walk.walk.runs.contains_key(&made.run) => {
                        self.update_lock(here)?;
                        self.update_walk(|walk| listed(&mut walk.walk.landing))
                    }
                    _ => self.update_lock(|lock| {
                        here(lock);
                        listed(&mut lock.runs);
                    }),
                }),
            };
            (self.path(&path), old.into_inner(), listed.err())
        };
        if unlisted.is_none() {
            self.drop_unused(&old)?;
        }
        Ok((path, unlisted))
    }

    /// An output its machine keeps, in the lock's table: its hash, which machine, and no file.
    pub(crate) fn stays(&self, sha256: &str, machine: &str, length: u64) -> Result<(), Error> {
        self.update_lock(|lock| lock.away(sha256, machine, length, None))
    }

    /// The file `sha256` as the project has it, here or kept by a machine: nothing need come for it.
    pub(crate) fn own(&self, sha256: &str) -> Option<Kept> {
        let file = self.lock().ok()?.files.remove(sha256)?;
        (file.stays() || self.path(&file.path).is_file()).then(|| Kept { field: file.field(), file, already: true })
    }

    /// The held file at `at`, of `sha256`, in `files/` and recorded in project.lock: a hard link to the store's object, which stays
    /// the store's, else a copy. Err: why not.
    pub(crate) fn take_held(&self, at: &Path, sha256: &str) -> Result<Kept, String> {
        let mut head = Vec::new();
        std::fs::File::open(at).and_then(|file| file.take(8192).read_to_end(&mut head)).map_err(|e| e.to_string())?;
        let (kind, field) = media(infer::get(&head))?;
        let path = file_named(sha256, kind.extension());
        self.recorded(path, sha256.to_owned(), field, |part| match std::fs::hard_link(at, part) {
            Ok(()) => Ok(()),
            // Another disk.
            Err(_) => std::fs::copy(at, part).map(drop),
        })
    }

    /// The `http(s)` URL `source`, fetched into `files/<sha256>.<ext>`, its type read from its bytes, and recorded in project.lock.
    /// Err: why not. A client's own file comes held (`PUT /v1/files`), never by its path.
    pub(crate) async fn import(&self, source: &str) -> Result<Kept, String> {
        if !is_url(source) {
            return Err("it is neither a held file's sha256:<hex>, a file of the project's nor an http(s) URL".into());
        }
        let bytes = fetch_url(source).await?;
        let (kind, field) = media(infer::get(&bytes))?;
        let sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        let path = file_named(&sha256, kind.extension());
        self.recorded(path, sha256, field, |part| std::fs::write(part, &bytes))
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
    pub(crate) fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// Whether a file the lock names should be here and isn't, so a pull is what it needs: what stays on a machine isn't.
    pub(crate) fn missing(&self, file: &File) -> bool {
        !file.stays() && !file.here
    }

    /// Whether files wait to be measured with no ffprobe here to do it: a walk fetches one first.
    pub(crate) fn wants_ffprobe(&self, lock: &Lock) -> bool {
        lock.files.values().any(File::unmeasured) && platform::tools::ffmpeg(&self.tools, &crate::tools::path()).is_none()
    }

    /// How many frames the film at `path` holds, by its file; None with no file there, or no ffprobe to read it. Asked of ffprobe once for
    /// each size and time it has, since a read of a project asks this.
    pub(crate) fn film_frames(&self, path: &Path) -> Option<u64> {
        let meta = std::fs::metadata(path).ok()?;
        let held = (meta.len(), meta.modified().ok()?);
        if let Some((known, frames)) = self.shared.films.lock().expect("films lock").get(path)
            && *known == held
        {
            return *frames;
        }
        let (_, ffprobe) = platform::tools::ffmpeg(&self.tools, &crate::tools::path())?;
        let frames = super::tools::frames_of(&ffprobe, path);
        self.shared.films.lock().expect("films lock").insert(path.to_owned(), (held, frames));
        frames
    }

    /// Every file `lock` names and nothing has measured, measured once and recorded, in `lock` and in
    /// project.lock. With no ffprobe here yet they wait for the next read, or a cut, which fetches one first.
    pub(crate) fn measure(&self, lock: &mut Lock) -> Result<(), Error> {
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
                file.measured_as(probed);
                // A video that landed: the page plays its preview on a hover.
                if file.field() == "video" && file.width > 0 {
                    super::thumbs::landed(self.dir.join(&file.path), sha256, file.lasts_ms() as f64 / 1000.0);
                }
            }
        }
        self.record_measured(&measured)
    }

    /// What's `measured` of these files by hash, in project.db and on their rows, in one write: what ffprobe says of their bytes, here or not.
    /// Two would let a read between them work out the rows unmeasured, under the revision that stands after.
    fn record_measured(&self, measured: &BTreeMap<String, File>) -> Result<(), Error> {
        let kept = self.reading_measured(|held| measured.iter().all(|(sha256, file)| held.get(sha256) == Some(&Measured::of(file))))?;
        let rows = self.reading_bare_lock(|lock| measured.iter().all(|(sha256, file)| lock.files.get(sha256).is_none_or(|row| Measured::of(row) == Measured::of(file))))?;
        if kept && rows {
            return Ok(());
        }
        self.act(|| {
            if !kept {
                self.writing_measured(|held| {
                    for (sha256, file) in measured {
                        held.insert(sha256.clone(), Measured::of(file));
                    }
                    Ok(())
                })?;
            }
            self.update_lock(|held| {
                for (sha256, file) in measured {
                    if let Some(row) = held.files.get_mut(sha256) {
                        row.measured_as(file);
                    }
                }
            })
        })
    }

    /// The files in `files/` that nothing uses (`named` is every sha256 something does), whatever the lock's table says, their bytes, and how many
    /// of them nothing has a record of. With `delete` those on record go, from disk and from the table: what `reclaim --yes` frees. One with no
    /// record may be all that's left of a take, so it's said and it stays. What a run left in `.runs/` goes only if what stays in `files/`
    /// holds every byte of it, by sha256: else it may be an output's only copy (one `files/` couldn't take, or a folder), and stays, unheld.
    pub(crate) fn sweep(&self, named: &BTreeSet<String>, recorded: &BTreeSet<String>, delete: bool) -> Result<(u64, u64, u64, Vec<Unheld>), Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let (mut files, mut bytes, mut unrecorded) = (0, 0, 0);
        // What stays in `files/`, by the sha256 its name says, which its bytes must say too before anything goes for it.
        let mut held = BTreeMap::new();
        for entry in std::fs::read_dir(self.dir.join(FILES)).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{FILES}/{name}");
            let file = lock.files.values().find(|file| file.path == path);
            if path.contains("/.") || entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let hex = name.split('.').next().unwrap_or_default();
            if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                held.insert(format!("sha256:{hex}"), entry.path());
            }
            if file.is_some_and(|file| named.contains(&file.sha256)) {
                continue;
            }
            match file.is_some_and(|file| recorded.contains(&file.sha256)) {
                // Listed, and `delete` leaves it.
                false => {
                    unrecorded += 1;
                    if delete {
                        continue;
                    }
                }
                true => drop(held.remove(&format!("sha256:{hex}"))),
            }
            bytes += entry.metadata().map_or(0, |meta| meta.len());
            files += 1;
            if delete {
                std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
            }
        }
        let holds = |sha256: &Option<String>| sha256.as_ref().is_some_and(|sha256| held.get(sha256).is_some_and(|at| hash(at).is_ok_and(|bytes| bytes == *sha256)));
        // What runs left where they land, each a file still there or a folder one made.
        let mut unheld = vec![];
        for entry in std::fs::read_dir(self.dir.join(RUNS)).into_iter().flatten().flatten() {
            let (at, folder) = (entry.path(), entry.file_type().is_ok_and(|kind| kind.is_dir()));
            let within = hashed(&at);
            let length = within.iter().map(|(_, length)| length).sum();
            if !within.iter().all(|(sha256, _)| holds(sha256)) {
                let sha256 = within.into_iter().next().and_then(|(sha256, _)| sha256).filter(|_| !folder);
                unheld.push(Unheld { path: format!("{RUNS}/{}", entry.file_name().to_string_lossy()), sha256, bytes: length });
                continue;
            }
            files += within.len() as u64;
            bytes += length;
            if delete {
                let gone = if folder { std::fs::remove_dir_all(&at) } else { std::fs::remove_file(&at) };
                gone.map_err(|e| io_at(&at, e))?;
            }
        }
        if delete {
            self.update_lock(|lock| {
                lock.files.retain(|sha256, _| named.contains(sha256));
                lock.runs.retain(|row| named.contains(&row.sha256));
            })?;
            // Gone once nothing is left in it.
            let _ = std::fs::remove_dir(self.dir.join(RUNS));
        }
        Ok((files, bytes, unrecorded, unheld))
    }

    /// Held while files are deleted: a file isn't kept between.
    pub(super) fn collecting(&self) -> MutexGuard<'_, ()> {
        self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bytes on disk of `sha256`, if here.
    pub(crate) fn size(&self, lock: &Lock, sha256: &str) -> u64 {
        lock.files.get(sha256).and_then(|file| std::fs::metadata(self.path(&file.path)).ok()).map_or(0, |meta| meta.len())
    }

    /// Deletes those of `sha256s` nothing in the lock names any more: a tool's old version, free to make again.
    pub(crate) fn drop_unused(&self, sha256s: &[String]) -> Result<(), Error> {
        let _held = self.shared.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let named = lock.recorded();
        for sha256 in sha256s.iter().filter(|sha256| !named.contains(sha256.as_str())) {
            let Some(file) = lock.files.get(sha256) else { continue };
            let at = self.path(&file.path);
            let _ = std::fs::remove_file(&at);
        }
        let unused: Vec<&String> = sha256s.iter().filter(|sha256| !named.contains(sha256.as_str())).collect();
        // What was measured of an earlier revision goes with it.
        self.act(|| {
            if self.reading_measured(|held| unused.iter().any(|sha256| held.contains_key(*sha256)))? {
                self.writing_measured(|held| {
                    held.retain(|sha256, _| !unused.contains(&sha256));
                    Ok(())
                })?;
            }
            self.update_lock(|lock| lock.files.retain(|sha256, _| !unused.contains(&sha256)))
        })
    }
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
pub(crate) fn is_url(source: &str) -> bool {
    source.split_once("://").is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
}

/// `sha256` with no scheme.
pub(crate) fn hex_of(sha256: &str) -> &str {
    sha256.trim_start_matches("sha256:")
}

/// What cozy prints of `sha256` for project.yaml.
pub(crate) fn prefix_of(sha256: &str) -> &str {
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

/// Each file at or under `path`, its sha256 and length. One that can't be read has no sha256.
fn hashed(path: &Path) -> Vec<(Option<String>, u64)> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => match std::fs::read_dir(path) {
            Ok(entries) => entries.flat_map(|entry| entry.map_or_else(|_| vec![(None, 0)], |entry| hashed(&entry.path()))).collect(),
            Err(_) => vec![(None, 0)],
        },
        Ok(meta) => vec![(hash(path).ok(), meta.len())],
        Err(_) => vec![(None, 0)],
    }
}

/// A file in `files/` is named for its bytes: read-only, so that writing to it fails rather than make its name a lie.
pub(super) fn sealed(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut permissions = meta.permissions();
        permissions.set_readonly(true);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

pub(crate) fn hash(path: &Path) -> Result<String, Error> {
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
