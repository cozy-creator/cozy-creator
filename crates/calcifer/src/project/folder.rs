//! A project folder: `project.yaml` (people's), `versions.lock` (cozy's: what each version was
//! made from, by file digest, the runs still making one, and what its walk still owes) and
//! `files/<sha256>.<ext>`.
use std::{
    cell::RefCell,
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
use saphyr::{LoadableYamlNode, MappingOwned, ScalarOwned, YamlOwned};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::graph::name_key;

pub const PROJECT: &str = "project.yaml";
const LOCK: &str = "versions.lock";
const FILES: &str = "files";

/// What one version was made from, under its file's digest.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Made {
    pub entity: String,
    /// The upstream files it was made from; None for the clean of a picture attached by hand,
    /// which is never out of date.
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    pub entity: String,
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
    /// Listed as a version; chosen when `choose`, or when the entity has none.
    List { choose: bool },
    /// Cleaned first, and its clean listed.
    Clean { choose: bool },
    /// Chosen, not listed: the clean of a picture attached by hand.
    Choose,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Lock {
    /// By entity, then file digest: two entities may make the same file.
    #[serde(default)]
    pub versions: BTreeMap<String, BTreeMap<String, Made>>,
    /// By idempotency key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default, skip_serializing_if = "Owed::is_empty")]
    pub walk: Owed,
}

/// What `cozy render` asked that the project's walk has not sent yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Owed {
    /// `render all`: every entity up to date, then the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Target>,
    /// What `all` sent this walk, made or failed, by name key: each is sent once.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sent: BTreeSet<String>,
    /// New versions of named entities, one each.
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

/// One new version of `entity`, sent under `key`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Take {
    pub key: String,
    pub entity: String,
    #[serde(flatten)]
    pub target: Target,
}

impl Lock {
    /// Whether its walk owes anything: asked and not sent, or sent and not landed.
    pub fn owes(&self) -> bool {
        self.walk.all.is_some() || !self.walk.takes.is_empty() || !self.pending.is_empty()
    }

    /// `entity`'s version whose file has `digest`.
    pub fn made(&self, entity: &str, digest: &str) -> Option<&Made> {
        self.versions.get(&name_key(entity))?.get(digest)
    }

    /// Records `made` as its entity's version whose file has `digest`.
    pub fn record(&mut self, digest: String, made: Made) {
        self.versions.entry(name_key(&made.entity)).or_default().insert(digest, made);
    }
}

/// Finds an entity's mapping in a project document.
pub type Locate = for<'a> fn(&'a mut YamlOwned, &str) -> Option<&'a mut MappingOwned>;

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

    /// Applies `change` to versions.lock as it is now.
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

    /// Applies `change` to entity `id`'s mapping in project.yaml, merged into the file as it is then
    /// (`write`): what the merge said, or None when the file holds no such entity (nothing written).
    pub fn edit(
        &self,
        locate: Locate,
        id: &str,
        change: impl Fn(&mut MappingOwned),
    ) -> Result<Option<Vec<String>>, Error> {
        let base = document(&self.text()?)?;
        let mut ours = base.clone();
        let Some(map) = locate(&mut ours, id) else { return Ok(None) };
        change(map);
        self.write(&base, &ours).map(Some)
    }

    /// The one writer of project.yaml: `ours`, a writer's document made from `base` (what it read),
    /// merged three ways into the file as it is now, and spliced into its text (`splice`): only the
    /// values that change move, and comments and style stay. A field changed both ways takes
    /// `ours`, the newer write; that, and a change the splice cannot place, is said.
    pub fn write(&self, base: &YamlOwned, ours: &YamlOwned) -> Result<Vec<String>, Error> {
        let said = RefCell::new(vec![]);
        self.rewrite(&self.project(), |now| {
            let theirs = document(now)?;
            let mut clashes = vec![];
            let merged = merge(Some(base), Some(ours), Some(&theirs), "", &mut clashes);
            let merged = merged.unwrap_or(YamlOwned::Mapping(MappingOwned::new()));
            let mut lines: Vec<String> = clashes
                .iter()
                .map(|path| format!("{PROJECT}: {path} changed here and in the file; this write is kept"))
                .collect();
            if merged == theirs {
                *said.borrow_mut() = lines;
                return Ok(now.to_owned());
            }
            let invalid = |why: String| Error::new(Kind::Internal, "project_splice", format!("{PROJECT}: {why}"));
            let (text, unplaced) = super::splice::splice(now, &theirs, &merged).map_err(invalid)?;
            // Never a file that does not read back.
            document(&text).map_err(|e| invalid(e.message))?;
            lines.extend(
                unplaced.iter().map(|path| format!("{PROJECT}: {path} was not written: the file changed it too")),
            );
            *said.borrow_mut() = lines;
            Ok(text)
        })?;
        Ok(said.into_inner())
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

    /// Deletes each file under `files/` no version names (`listed`) and none of their records
    /// does, and the records of versions no longer listed.
    pub fn collect_garbage(&self, listed: &BTreeSet<String>) -> Result<(), Error> {
        self.update_lock(|lock| {
            lock.versions.values_mut().for_each(|made| made.retain(|digest, _| listed.contains(digest)));
            lock.versions.retain(|_, made| !made.is_empty());
        })?;
        let lock = self.lock()?;
        let mut keep = listed.clone();
        keep.extend(lock.versions.values().flat_map(BTreeMap::values).flat_map(|made| made.outputs.values().cloned()));
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

/// One YAML document, owned.
pub fn document(text: &str) -> Result<YamlOwned, Error> {
    let docs = YamlOwned::load_from_str(text)
        .map_err(|e| Error::new(Kind::Validation, "project_yaml", format!("{PROJECT}: {e}")))?;
    match <[YamlOwned; 1]>::try_from(docs) {
        Ok([doc]) => Ok(doc),
        Err(_) => Err(Error::new(Kind::Validation, "project_yaml", format!("{PROJECT} holds one YAML document"))),
    }
}

/// Three-way merge of `ours` (made from `base`) into `theirs`: mappings key by key, lists of
/// named mappings name by name, other lists by what each side added and removed. A value changed
/// both ways takes `ours`, and its path joins `clashes`.
fn merge(
    base: Option<&YamlOwned>,
    ours: Option<&YamlOwned>,
    theirs: Option<&YamlOwned>,
    path: &str,
    clashes: &mut Vec<String>,
) -> Option<YamlOwned> {
    if ours == base {
        return theirs.cloned();
    }
    if theirs == base || theirs == ours {
        return ours.cloned();
    }
    let at = |key: &str| if path.is_empty() { key.to_owned() } else { format!("{path}.{key}") };
    match (ours, theirs) {
        (Some(YamlOwned::Mapping(o)), Some(YamlOwned::Mapping(t))) => {
            let b = base.and_then(YamlOwned::as_mapping);
            let keys = t.keys().chain(o.keys().filter(|k| !t.contains_key(*k)));
            let merged = keys.filter_map(|k| {
                let name = k.as_str().map_or_else(|| format!("{k:?}"), str::to_owned);
                merge(b.and_then(|b| b.get(k)), o.get(k), t.get(k), &at(&name), clashes).map(|v| (k.clone(), v))
            });
            Some(YamlOwned::Mapping(merged.collect()))
        }
        (Some(YamlOwned::Sequence(o)), Some(YamlOwned::Sequence(t))) => {
            let b = base.and_then(YamlOwned::as_vec).cloned().unwrap_or_default();
            let name = |item: &YamlOwned| item.as_mapping_get("name").and_then(YamlOwned::as_str).map(str::to_owned);
            if o.iter().chain(t).all(|item| name(item).is_some()) {
                let find =
                    |list: &[YamlOwned], n: &str| list.iter().find(|item| name(item).as_deref() == Some(n)).cloned();
                let mut names: Vec<String> = t.iter().filter_map(name).collect();
                names.extend(o.iter().filter_map(name).filter(|n| find(t, n).is_none()));
                let merged = names.iter().filter_map(|n| {
                    merge(find(&b, n).as_ref(), find(o, n).as_ref(), find(t, n).as_ref(), &at(n), clashes)
                });
                return Some(YamlOwned::Sequence(merged.collect()));
            }
            let removed = |item: &YamlOwned| b.contains(item) && !o.contains(item);
            let added = o.iter().filter(|item| !b.contains(item) && !t.contains(item));
            Some(YamlOwned::Sequence(t.iter().filter(|item| !removed(item)).chain(added).cloned().collect()))
        }
        _ => {
            clashes.push(path.to_owned());
            ours.cloned()
        }
    }
}

/// `key: value` in `map`, where the key was, else last.
pub fn put(map: &mut MappingOwned, key: &str, value: YamlOwned) {
    let key = YamlOwned::Value(ScalarOwned::String(key.to_owned()));
    match map.get_mut(&key) {
        Some(slot) => *slot = value,
        None => {
            map.insert(key, value);
        }
    }
}

pub fn text(value: &str) -> YamlOwned {
    YamlOwned::Value(ScalarOwned::String(value.to_owned()))
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
