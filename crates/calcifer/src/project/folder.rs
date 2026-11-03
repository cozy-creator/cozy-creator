//! A project folder (Paul, 11-01): `project.yaml` (people's; cozy writes it only at `cozy project
//! new`), `project.lock` (cozy's: each node's versions, what made each, which is approved, the files
//! imported, the runs still making one, and what its walk still owes), `assets/`, and the cut
//! (`edit.mp4`). Every file the project uses is a real file in `assets/<sha256>.<ext>`, flat: each take
//! and version, clean and latents cozy makes, and each file `cozy project import` brought in
//! (`Folder::import`). project.yaml names one only by a hex prefix of its hash (`Lock::matching`), so
//! the lock names nothing of this computer and travels with project.yaml.
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
/// Where every file the project uses is kept, by content: `assets/<sha256>.<ext>`.
pub const ASSETS: &str = "assets";
/// A version's primary asset, the one its node reads as: its video, else its clean (a cleaned
/// picture's), else its image.
const PRIMARY: [&str; 3] = ["video", "clean", "image"];

/// The most a URL may give: far past any picture, and a shared project can't fill a disk with it.
const URL_MOST: usize = 64 << 20;
/// How long a URL may take. Measured progress can't judge this one: a stranger's server can drip a
/// byte at a time for ever, so this is the last resort, and it leaves room for `URL_MOST` over a slow line.
const URL_LONGEST: Duration = Duration::from_secs(300);
/// What an import kept: its asset, and the field that names one of its type (`image`, `video`, `audio`).
pub struct Kept {
    pub asset: Asset,
    pub field: &'static str,
    /// Its bytes were in `assets/` already, so nothing was copied.
    pub already: bool,
    /// It was marked for a clean before, and this import did not mark it.
    pub unmarked: bool,
}

/// How many hex characters of its hash cozy prints for an asset, for project.yaml.
const PREFIX: usize = 12;

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
    /// What it was made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<Vec<Upstream>>,
    /// The frames it wrote for world states, by state.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub writes: BTreeMap<String, Asset>,
    /// The runs that made it, in order: one per call of a shot made in consecutive calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<u64>,
    /// What each of those runs was asked: a function and its exact input. Never read to say what is out of date.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invocations: Vec<Value>,
    /// What its function said of it, and what was left out of its request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// A picture's clean, which is never cleaned again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cleaned: bool,
    /// The hash of the imported picture this run cleans.
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

    /// Its assets and writes.
    fn files(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values().chain(self.writes.values())
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
    /// The clean of an imported picture: from then on what its hash resolves to.
    Imported,
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
    /// The files `cozy project import` brought in, by sha256.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub imports: BTreeMap<String, Import>,
    /// By idempotency key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default, skip_serializing_if = "Owed::is_empty")]
    pub walk: Owed,
}

/// An imported file: its asset, and for one `--character` or `--prop` marked (`mark`), the clean that its
/// hash resolves to for what reads it once made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Import {
    #[serde(flatten)]
    pub asset: Asset,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mark: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<Asset>,
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
    /// Every file it names: each import and its clean, each version's assets and writes, what each
    /// pending run makes and continues.
    fn files(&self) -> impl Iterator<Item = &Asset> {
        let imports = self.imports.values().flat_map(|import| std::iter::once(&import.asset).chain(&import.clean));
        let pending = self.pending.values().map(|pending| &pending.made);
        let versions = self.nodes.values().flat_map(|node| &node.versions).chain(pending).flat_map(Made::files);
        let from = self.pending.values().filter_map(|pending| pending.call.as_ref()?.from.as_ref());
        let continued = from.flat_map(|(video, latents)| std::iter::once(video).chain(latents));
        imports.chain(versions).chain(continued)
    }

    /// Every file it names, by path, with its sha256.
    pub fn assets(&self) -> BTreeMap<&str, &str> {
        self.files().map(|asset| (asset.path.as_str(), asset.sha256.as_str())).collect()
    }

    /// The assets whose hash starts with `prefix`, imported or made, one per hash.
    pub fn matching(&self, prefix: &str) -> Vec<&Asset> {
        let mut found: BTreeMap<&str, &Asset> = BTreeMap::new();
        for asset in self.files().filter(|asset| hex_of(&asset.sha256).starts_with(prefix)) {
            let seen = found.entry(&asset.sha256).or_insert(asset);
            if seen.video.is_none() {
                *seen = asset;
            }
        }
        found.into_values().collect()
    }

    /// Whether a run cleaning the imported `sha256` is on its way.
    pub fn cleaning(&self, sha256: &str) -> Option<u64> {
        self.pending.values().find(|pending| pending.made.clean_of.as_deref() == Some(sha256)).map(|pending| pending.run)
    }

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
    /// A version whose primary asset one already has (a render that came back the same, such as a fixed
    /// seed's) is that version: its number is the one given back, and no second version is listed.
    pub fn record(&mut self, made: Made, kind: &str, approve: bool) -> u32 {
        let node = self.nodes.entry(name_key(&made.node)).or_default();
        node.kind = kind.into();
        if approve {
            node.approved = made.primary().map(|asset| asset.path.clone());
        }
        // The same content (its path is its sha256), whatever has been probed of it yet.
        let path = |made: &Made| made.primary().map(|asset| asset.path.clone());
        let same = node.versions.iter().find(|version| version.number != made.number && path(version) == path(&made));
        if let Some(same) = same {
            return same.number;
        }
        let number = made.number;
        node.versions.retain(|version| version.number != number);
        node.versions.push(made);
        node.versions.sort_by_key(|made| made.number);
        number
    }
}

pub struct Folder {
    pub dir: PathBuf,
    /// cozy's tools folder, where its ffprobe is (else PATH's).
    pub tools: PathBuf,
    /// Held across each write's read and swap, so no two of this Calcifer's writes lose one.
    writing: Mutex<()>,
    /// Held while an import is named in the lock and while garbage is collected, so it is not collected between.
    collecting: Mutex<()>,
}

impl Folder {
    /// The project in `dir`, its videos probed with the ffprobe in `home`'s tools (else PATH's).
    pub fn open(dir: &Path, home: &Home) -> Result<Self, Error> {
        if !dir.join(PROJECT).is_file() {
            return Err(Error::new(
                Kind::NotFound,
                "project_not_found",
                format!("{} holds no {PROJECT}", dir.display()),
            )
            .next("cozy project new <path>"));
        }
        Ok(Self { dir: dir.to_owned(), tools: home.tools(), writing: Mutex::default(), collecting: Mutex::default() })
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

    /// The file at `source` (an absolute path, or an `http(s)` URL) kept as a real file
    /// `assets/<sha256>.<ext>`, its type read from its bytes, and recorded in project.lock with `mark` (none
    /// for what is no picture; a later import's word stands). Err: why not.
    pub async fn import(&self, source: &str, mark: bool) -> Result<Kept, String> {
        if is_url(source) {
            let bytes = fetch_url(source).await?;
            let (kind, field) = media(infer::get(&bytes))?;
            let sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
            let path = asset_named(&sha256, kind.extension());
            return self.recorded(path, sha256, field, mark, |part| std::fs::write(part, &bytes));
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
            let at = asset_named(&sha256, kind.extension());
            self.recorded(at, sha256, field, mark, |part| std::fs::copy(path, part).map(drop))
        })
    }

    /// An import's bytes at `path`, written by `write` unless they are there, named in project.lock.
    fn recorded(
        &self,
        path: String,
        sha256: String,
        field: &'static str,
        mark: bool,
        write: impl Fn(&Path) -> std::io::Result<()>,
    ) -> Result<Kept, String> {
        let _held = self.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let already = !self.store(&path, write).map_err(|e| e.message)?;
        let asset = Asset { path, sha256, video: None };
        let mark = mark && field == "image";
        let unmarked = !mark && self.lock().map_err(|e| e.message)?.imports.get(&asset.sha256).is_some_and(|import| import.mark);
        let new = || Import { asset: asset.clone(), mark, clean: None };
        let record = |lock: &mut Lock| {
            lock.imports.entry(asset.sha256.clone()).and_modify(|import| import.mark = mark).or_insert_with(new);
        };
        self.update_lock(record).map_err(|e| e.message)?;
        Ok(Kept { asset, field, already, unmarked })
    }

    /// `path` (an asset's) made by `write` at a part beside it and then named, unless it is there. Whether it wrote it.
    fn store(&self, path: &str, write: impl Fn(&Path) -> std::io::Result<()>) -> Result<bool, Error> {
        let at = self.dir.join(path);
        if at.is_file() {
            return Ok(false);
        }
        let dir = self.dir.join(ASSETS);
        std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
        let part = dir.join(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
        let _ = std::fs::remove_file(&part);
        write(&part).and_then(|()| std::fs::rename(&part, &at)).map_err(|e| io_at(&at, e))?;
        Ok(true)
    }

    /// An asset's path, as the lock writes it, on this computer.
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// Every video `lock` records (a version's asset, an imported file) with no shape yet,
    /// probed once and recorded, in `lock` and in project.lock. With no ffprobe here yet (the walk
    /// fetches one before it cuts), they wait for the next read.
    pub fn shape_videos(&self, lock: &mut Lock) -> Result<(), Error> {
        let unshaped = |asset: &Asset| {
            let ext = Path::new(&asset.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            asset.video.is_none() && ext.is_some_and(|ext| VIDEOS.contains(&ext.as_str()))
        };
        let versions = lock.nodes.values().flat_map(|node| &node.versions).flat_map(|made| made.assets.values());
        let imports = lock.imports.values().map(|import| &import.asset);
        let wanted: BTreeSet<&String> = imports.chain(versions).filter(|a| unshaped(a)).map(|a| &a.path).collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let Some((_, ffprobe)) = platform::tools::ffmpeg(&self.tools, &crate::tools::path()) else { return Ok(()) };
        let shapes: BTreeMap<String, Shape> = (wanted.into_iter())
            .filter_map(|path| Some((path.clone(), super::video::shape(&ffprobe, &self.dir.join(path))?)))
            .collect();
        let fill = |lock: &mut Lock| {
            let versions = lock.nodes.values_mut().flat_map(|node| &mut node.versions).flat_map(|made| made.assets.values_mut());
            for asset in lock.imports.values_mut().map(|import| &mut import.asset).chain(versions) {
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
    /// not), no pending run or call, no import.
    pub fn collect_garbage(&self) -> Result<(), Error> {
        let _held = self.collecting.lock().unwrap_or_else(|e| e.into_inner());
        let lock = self.lock()?;
        let named = lock.assets();
        for entry in std::fs::read_dir(self.dir.join(ASSETS)).into_iter().flatten().flatten() {
            let path = format!("{ASSETS}/{}", entry.file_name().to_string_lossy());
            let ours = entry.file_type().is_ok_and(|kind| kind.is_file() || kind.is_symlink());
            if ours && !path.contains("/.") && !named.contains_key(path.as_str()) {
                std::fs::remove_file(entry.path()).map_err(|e| io_at(&entry.path(), e))?;
            }
        }
        Ok(())
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

/// An asset's path, given its extension.
fn asset_named(sha256: &str, ext: &str) -> String {
    format!("{ASSETS}/{}.{ext}", hex_of(sha256))
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
