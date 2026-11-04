//! Projects (xs-151, cl-758): a folder of assets, each a stable name holding a set of files. Calcifer opens a project by its path
//! (and keeps every folder it opened in its index) and walks it: an operation makes an asset only when it has no state, and is an
//! ordinary run, sent as soon as what it reads has settled, so independent ones run at once and the machines' schedulers place
//! them. Its package's adapter for its kind turns it into calls, and they land as one version of the asset, in `files/` and
//! project.lock (people write project.yaml; cozy writes one line of it, at `approve` and `unpin`). The reserved package `cozy` runs
//! here, free. One walker per project takes every render asked of it, and what it owes is kept in
//! project.lock, so a restarted Calcifer goes on with it. Its clients ask and watch; the runs go on without them.
mod folder;
mod format;
mod graph;
pub(crate) mod host;
mod remote;
mod tools;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    convert::Infallible,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse as _, Response,
        sse::{self, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::{Stream, StreamExt as _, stream::FuturesUnordered};
use protocol::{
    calcifer::{FileTerm, Kind as RunKind, On, Run, RunRequest, StagedInput, Status as RunStatus, SubmitRun, Warning},
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    project::{
        Approve, Approved, Asked, Asset as Row, AssetVersions, Cancel, Cancelled, Cut, Event, File as Listing, Import, Imported, Kept, Known, Open, Opened,
        Orphan, Projects as Listed, Queued, QueuedNow, Reclaim, Reclaimed, Rename, Renamed, Rendered, Runs, Stale, Standing as RowStanding, State as ProjectState, Unpin, Unpinned,
        Version as VersionRow, VersionPrefix,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast};
use tracing::warn;

use crate::api::{Api, Refusal, parse};
use folder::{Ask, Files, Folder, Invocation, Keeps, Lock, MadeBy, Pending, ReadFrom, Take, Target, Tried, Version, Written};
use format::{ALL, Doc, Standing, State as Stand, Tool};
use graph::name_key;
use host::{Host, Package, Renderer};
use tools::Canvas;

/// The projects this Calcifer knows: those open since it started, by id (one folder is always one
/// project), and its index of every folder it opened, kept in `projects.json`.
#[derive(Default)]
pub struct Projects {
    open: Mutex<HashMap<String, Arc<Project>>>,
    /// Read on first use.
    index: Mutex<Option<Index>>,
}

/// Every project folder Calcifer opened, by id.
#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    projects: BTreeMap<String, Entry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    used_at_ms: i64,
    /// Its walk was going: the next start walks it again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    walking: bool,
}

struct Project {
    folder: Folder,
    events: broadcast::Sender<Event>,
    /// Whether its one walker is going; it goes until nothing is owed.
    walking: Mutex<bool>,
    /// Tells the walker something was asked.
    asked: Notify,
    /// Held while a cut is made.
    cutting: Arc<Mutex<()>>,
    /// Held while its files move to or from its Hub.
    moving: tokio::sync::Mutex<()>,
    /// Each `Idempotency-Key` a render was asked under since Calcifer started, and the ask's digest.
    keys: Mutex<HashMap<String, String>>,
}

pub fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/projects", post(open).get(list))
        .route("/v1/projects/{id}", get(state).delete(forget))
        .route("/v1/projects/{id}/imports", post(import))
        .route("/v1/projects/{id}/runs", post(runs))
        .route("/v1/projects/{id}/cancel", post(cancel))
        .route("/v1/projects/{id}/approve", post(approve))
        .route("/v1/projects/{id}/unpin", post(unpin))
        .route("/v1/projects/{id}/rename", post(rename))
        .route("/v1/projects/{id}/reclaim", post(reclaim))
        .route("/v1/projects/{id}/push", post(remote::pushed))
        .route("/v1/projects/{id}/pull", post(remote::pulled))
        .route("/v1/projects/{id}/events", get(events))
        .route("/v1/projects/{id}/yaml", get(yaml).put(save))
        .route("/v1/projects/{id}/assets/{name}", get(asset))
        .route("/v1/projects/{id}/files/{file}", get(file))
        .route("/v1/projects/{id}/cuts/{cut}", get(cut))
}

impl Projects {
    /// Project `id`: open, else opened from the index.
    fn get(&self, api: &Arc<Api>, id: &str) -> Result<Arc<Project>, Refusal> {
        if let Some(project) = self.open.lock().expect("projects lock").get(id).cloned() {
            return Ok(project);
        }
        let entry = indexed(api, false, |index| index.projects.get(id).cloned());
        let Some(entry) = entry else {
            let said = format!("this computer knows no project {id}");
            return Err(Refusal(Error::new(Kind::NotFound, "project_unknown", said).next("cozy project ls")));
        };
        self.open(api, &entry.path).map_err(Refusal)?;
        Ok(self.open.lock().expect("projects lock").get(id).cloned().expect("just opened"))
    }

    /// The project in `dir` (canonical), opened once; its walk goes on when project.lock owes one.
    fn open(&self, api: &Arc<Api>, dir: &FsPath) -> Result<String, Error> {
        let folder = Folder::open(dir, api.machines.hubs.home())?;
        crate::events::project_opened(dir);
        let id = project_id(dir);
        let project = (self.open.lock().expect("projects lock").entry(id.clone()))
            .or_insert_with(|| {
                let (events, asked) = (broadcast::channel(256).0, Notify::new());
                Arc::new(Project {
                    folder,
                    events,
                    walking: Mutex::new(false),
                    asked,
                    cutting: Arc::default(),
                    moving: tokio::sync::Mutex::default(),
                    keys: Mutex::default(),
                })
            })
            .clone();
        if project.folder.lock()?.owes() {
            walk(api, &project);
        }
        Ok(id)
    }

    /// Finds edits made outside Calcifer to the projects open here.
    pub(crate) fn look(&self) {
        let open = self.open.lock().expect("projects lock");
        let dirs: Vec<PathBuf> = open.values().map(|project| project.folder.dir.clone()).collect();
        drop(open);
        for dir in dirs {
            crate::events::project_changed(&dir);
        }
    }
}

/// A picture at least this share transparent has its background gone already.
const OPAQUE_BELOW: f64 = 0.05;

/// The words a run hears outside a project, as `git` says it of a repository.
pub(crate) fn not_in_one() -> Error {
    Error::new(Kind::Usage, "not_in_a_project", "not in a project: cd into one, or cozy project new <name>")
}

/// Where a run of the project in `dir` lands its outputs.
pub(crate) fn landing(dir: &FsPath) -> PathBuf {
    dir.join(folder::RUNS)
}

/// The project a run's landing folder is in, if it's one.
pub(crate) fn of_landing(out_dir: &FsPath) -> Option<&FsPath> {
    out_dir.file_name().is_some_and(|name| name == folder::RUNS).then(|| out_dir.parent()).flatten()
}

/// The project holding `path`, if one does.
pub(crate) fn holding_folder(path: &FsPath) -> Option<PathBuf> {
    holding(path).ok()
}

/// The landing folder of the project holding `path`, as a run's record has it.
pub(crate) fn landing_within(path: &FsPath) -> Option<String> {
    Some(landing(&holding_folder(path)?).to_string_lossy().into_owned())
}

/// `--file`'s term in `project`: six or more hex digits that start one of its files' hash are that file, what stays on a machine
/// fetched now that it's named.
pub(crate) async fn file_term(calcifer: &crate::Calcifer, project: Option<&FsPath>, term: &FileTerm) -> Result<FileTerm, Error> {
    let written = term.file.as_str();
    let hashlike = written.len() >= 6 && written.bytes().all(|byte| byte.is_ascii_hexdigit());
    let Some(project) = project.filter(|_| hashlike) else { return Ok(term.clone()) };
    let folder = Folder::at(project);
    let lock = folder.lock()?;
    match lock.matching(&written.to_ascii_lowercase()).as_slice() {
        [] => Ok(term.clone()),
        [file] => {
            let at = folder.path(&file.path);
            if file.stays() && !at.is_file() {
                let fetched = calcifer.bring(&file.sha256, &file.machine).await;
                fetched.map_err(|why| Error::new(Kind::Unavailable, "file_on_a_machine", format!("{written} stays on {}, and can't be had: {why}", file.machine)))?;
            }
            Ok(FileTerm { name: term.name.clone(), file: at.to_string_lossy().into_owned() })
        }
        several => Err(Error::new(Kind::Usage, "file_ambiguous", format!("{written} fits {} files of this project: write more of its hash", several.len()))),
    }
}

/// What a project's run landed: which run, of what, and its bytes' digest.
pub(crate) struct Landed<'a> {
    pub run: u64,
    pub target: &'a str,
    pub output: &'a str,
    pub index: u32,
    pub digest: &'a str,
}

/// A run's output, landed at `dest`, kept in its project's `files/` and listed there. An operation's run takes its files off the
/// list when it lands them on its version. Answers the file's path.
pub(crate) fn landed(project: &FsPath, made: &Landed, dest: &FsPath) -> Result<PathBuf, String> {
    let output = match made.index {
        0 => made.output.to_owned(),
        n => format!("{}-{n}", made.output),
    };
    let row = folder::RunFile { run: made.run, function: made.target.to_owned(), output, sha256: made.digest.to_owned() };
    let kept = Folder::at(project).adopt(dest, made.digest, row).map_err(|e| e.message)?;
    // The file is in `files/` now. An output that grows reads its next revision whole, with nothing here to extend.
    let _ = std::fs::remove_file(dest);
    Ok(kept)
}

/// An output its machine keeps, in its project's lock.
pub(crate) fn stays(project: &FsPath, sha256: &str, machine: &str, length: u64) -> Result<(), String> {
    Folder::at(project).stays(sha256, machine, length).map_err(|e| e.message)
}

/// One folder's id, the same for as long as it is there.
pub(crate) fn project_id(dir: &FsPath) -> String {
    format!("project-{}", &hex::encode(Sha256::digest(dir.to_string_lossy().as_bytes()))[..16])
}

/// What `project.changed` says of the project in `dir`: another text whenever project.yaml or project.lock is another file
/// than it was. Read from their stats alone, so a look at it costs nothing: every write replaces the file, so its inode tells
/// two writes in one clock tick apart.
pub(crate) fn revision(dir: &FsPath) -> String {
    let stamp = |name: &str| {
        let Ok(meta) = std::fs::metadata(dir.join(name)) else { return "none".to_owned() };
        let modified = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
        #[cfg(unix)]
        let file = std::os::unix::fs::MetadataExt::ino(&meta);
        #[cfg(not(unix))]
        let file = 0;
        format!("{file}-{}-{}", modified.map_or(0, |since| since.as_nanos()), meta.len())
    };
    format!("{}.{}", stamp(folder::PROJECT), stamp(folder::LOCK))
}

/// Walks again each project an earlier Calcifer was walking.
pub fn resume(api: &Arc<Api>) {
    let walking: Vec<PathBuf> = indexed(api, false, |index| {
        index.projects.values().filter(|entry| entry.walking).map(|entry| entry.path.clone()).collect()
    });
    for dir in walking {
        if let Err(error) = api.projects.open(api, &dir) {
            warn!("walking {} again: {}", dir.display(), error.message);
        }
    }
}

/// `change` applied to the index (read from `projects.json` on first use), which is written back
/// when `keep`.
fn indexed<R>(api: &Api, keep: bool, change: impl FnOnce(&mut Index) -> R) -> R {
    let path = api.machines.hubs.home().calcifer().join("projects.json");
    let mut held = api.projects.index.lock().expect("index lock");
    let index = held.get_or_insert_with(|| {
        std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    });
    let answer = change(index);
    if keep {
        let bytes = serde_json::to_vec_pretty(index).expect("an index serializes");
        if let Err(e) = platform::durable::replace(&path, &bytes, 0o644) {
            warn!("keeping the project index: {e}");
        }
    }
    answer
}

/// Starts the project's walker, or tells the one going that something was asked.
fn walk(api: &Arc<Api>, project: &Arc<Project>) {
    if std::mem::replace(&mut *project.walking.lock().expect("walking lock"), true) {
        return project.asked.notify_one();
    }
    let walk = Walk::new(api, project);
    let api = api.clone();
    tokio::spawn(async move {
        remember(&api);
        let error = match walk.go().await {
            Ok(first) => first,
            Err(error) => {
                *walk.project.walking.lock().expect("walking lock") = false;
                Some(error)
            }
        };
        remember(&api);
        walk.say(Event::Ended { error, rendered: walk.rendered() });
    });
}

/// Records in the index which open projects are being walked, which the next start walks again.
fn remember(api: &Api) {
    let walking: Vec<(String, bool)> = (api.projects.open.lock().expect("projects lock").iter())
        .map(|(id, project)| (id.clone(), *project.walking.lock().expect("walking lock")))
        .collect();
    indexed(api, true, |index| {
        for (id, walking) in walking {
            if let Some(entry) = index.projects.get_mut(&id) {
                entry.walking = walking;
            }
        }
    });
}

/// Opens the project holding `path` (the nearest folder up from it with a project.yaml), or makes
/// one there (`create`: the folder and its project.yaml); either way it is recorded in the index as
/// used now.
async fn open(State(api): State<Arc<Api>>, body: Result<Bytes, BytesRejection>) -> Result<Json<Opened>, Refusal> {
    let open: Open = parse(&body?)?;
    if !open.path.is_absolute() {
        return Err(Refusal(Error::new(Kind::Usage, "project_path", "a project is opened by its absolute path")));
    }
    let io = |e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", open.path.display())));
    let dir = match open.create {
        true => {
            let project = open.path.join(folder::PROJECT);
            if project.exists() {
                let said = format!("{} is a project already", open.path.display());
                return Err(Refusal(Error::new(Kind::Conflict, "project_exists", said)));
            }
            std::fs::create_dir_all(&open.path).map_err(io)?;
            platform::durable::replace(&project, format::starter().as_bytes(), 0o644)
                .map_err(|e| io(std::io::Error::other(e)))?;
            let dir = std::fs::canonicalize(&open.path).map_err(io)?;
            crate::events::project_changed(&dir);
            dir
        }
        false => holding(&open.path).map_err(Refusal)?,
    };
    // Indexed first: a walk this open starts records itself there.
    let (id, used_at_ms) = (project_id(&dir), now_ms());
    indexed(&api, true, |index| {
        let walking = index.projects.get(&id).is_some_and(|known| known.walking);
        index.projects.insert(id.clone(), Entry { path: dir.clone(), used_at_ms, walking });
    });
    api.projects.open(&api, &dir).map_err(Refusal)?;
    Ok(Json(Opened { id, path: dir }))
}

/// The project folder holding `path`: the nearest, up from it, with a project.yaml.
fn holding(path: &FsPath) -> Result<PathBuf, Error> {
    let not_found = || {
        let said = format!("{} is in no project: no {} there or in a folder above it", path.display(), folder::PROJECT);
        Error::new(Kind::NotFound, "project_not_found", said)
            .remedy("run it inside a project's folder, or name one with -p <path>")
            .next("cozy project new <path>")
    };
    let start = std::fs::canonicalize(path).map_err(|_| not_found())?;
    (start.ancestors()).find(|dir| dir.join(folder::PROJECT).is_file()).map(FsPath::to_owned).ok_or_else(not_found)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

#[derive(Deserialize)]
struct Page {
    limit: Option<usize>,
    /// The last id seen.
    before: Option<String>,
}

/// The index, last used first: each project's folder name, path and last use, whether its folder
/// is gone, and whether it is rendering.
async fn list(State(api): State<Arc<Api>>, Query(page): Query<Page>) -> Result<Json<Listed>, Refusal> {
    let mut known: Vec<(String, Entry)> =
        indexed(&api, false, |index| index.projects.iter().map(|(id, entry)| (id.clone(), entry.clone())).collect());
    known.sort_by_key(|(id, entry)| (std::cmp::Reverse(entry.used_at_ms), id.clone()));
    if let Some(before) = &page.before {
        let at = known.iter().position(|(id, _)| id == before).map_or(known.len(), |at| at + 1);
        known.drain(..at);
    }
    let limit = page.limit.unwrap_or(50).clamp(1, 1000);
    let older = known.len().saturating_sub(limit) as u64;
    let open = api.projects.open.lock().expect("projects lock");
    let projects = (known.into_iter().take(limit))
        .map(|(id, entry)| Known {
            name: entry.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
            missing: !entry.path.join(folder::PROJECT).is_file(),
            busy: open.get(&id).is_some_and(|project| *project.walking.lock().expect("walking lock")),
            path: entry.path,
            used_at_ms: entry.used_at_ms,
            id,
        })
        .collect();
    Ok(Json(Listed { projects, older }))
}

/// Forgets project `id`: out of the index, never its folder. Refused while it renders.
async fn forget(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<StatusCode, Refusal> {
    let mut open = api.projects.open.lock().expect("projects lock");
    if open.get(&id).is_some_and(|project| *project.walking.lock().expect("walking lock")) {
        let said = format!("{id} is rendering: it is forgotten once its runs land");
        return Err(Refusal(Error::new(Kind::Conflict, "project_busy", said).next("cozy project status")));
    }
    open.remove(&id);
    drop(open);
    match indexed(&api, true, |index| index.projects.remove(&id)) {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err(Refusal(Error::new(Kind::NotFound, "project_unknown", format!("this computer knows no project {id}")))),
    }
}

/// Its assets as they stand.
async fn state(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<ProjectState>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let lock = folder.lock().map_err(Refusal)?;
    let doc = match Doc::load(folder) {
        Ok(doc) => doc,
        Err(error) if !lock.owes() => return Err(Refusal(error)),
        // A render going has its own text: what it owes, and what's wrong with the file, are the answer.
        Err(error) => {
            let queue = queue(folder, &lock).into_iter().map(|(item, _)| item).collect();
            let busy = *project.walking.lock().expect("walking lock");
            let moving = project.moving.try_lock().is_err();
            let warning = format!("{}: a render going uses the text it was asked with, and a new one is refused until this reads", error.message);
            return Ok(Json(ProjectState { id, path: folder.dir.clone(), assets: vec![], entities: vec![], queue, missing: vec![], stale: vec![], orphans: vec![], busy, moving, absent: 0, run_files: 0, cuts: vec![], warnings: vec![warning] }));
        }
    };
    let stand = doc.stand(&lock);
    let mut warnings = doc.warnings.clone();
    warnings.extend(unadapted(&api, &doc, folder).await);
    for (i, standing) in stand.iter().enumerate() {
        let held = lock.asset(&doc.assets[i].name);
        let made = standing.version().and_then(|number| held?.version(number));
        warnings.extend(made.into_iter().flat_map(|made| made.warnings.iter().cloned()));
    }
    warnings.dedup();
    let (mut missing, mut stale) = (vec![], vec![]);
    let entities = (0..doc.assets.len()).filter(|&i| doc.assets[i].links.as_ref().is_some_and(|links| links.parent.is_none())).map(|i| entity_row(&doc, i)).collect();
    let assets = (0..doc.assets.len())
        .filter(|&i| doc.assets[i].links.is_none())
        .map(|i| {
            let row = row(&doc, folder, &lock, &stand, i);
            match &stand[i].state {
                Stand::Stale(why) => stale.push(Stale { name: row.name.clone(), why: why.clone() }),
                Stand::Missing | Stand::Waits if doc.makes(i) => missing.push(row.name.clone()),
                _ => {}
            }
            row
        })
        .collect();
    let cuts = (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)).map(|i| cut_row(&doc, folder, &lock, &stand, i)).collect();
    let busy = *project.walking.lock().expect("walking lock");
    let moving = project.moving.try_lock().is_err();
    let absent = lock.files.values().filter(|file| folder.missing(file)).count() as u64;
    let orphans = orphans(&doc, folder, &lock);
    let kept = kept_files(&doc, &lock);
    let run_files = lock.runs.iter().filter(|row| !kept.contains(&row.sha256)).count() as u64;
    let queued = queue(folder, &lock).into_iter().map(|(item, _)| item).collect();
    Ok(Json(ProjectState { id, path: folder.dir.clone(), assets, queue: queued, missing, stale, orphans, busy, moving, absent, run_files, entities, cuts, warnings }))
}

/// Entity `i` as `GET /v1/projects/{id}` lists it, with its links and sub-entities.
fn entity_row(doc: &Doc, i: usize) -> protocol::project::Entity {
    let asset = &doc.assets[i];
    let links = asset.links.as_ref();
    let subs = links.into_iter().flat_map(|links| &links.subs).filter_map(|sub| doc.find(sub));
    protocol::project::Entity {
        name: asset.name.clone(),
        kind: asset.kind.clone(),
        description: asset.fields.get("description").and_then(Value::as_str).map(str::to_owned),
        links: links.map(|links| links.own.clone()).unwrap_or_default(),
        subs: subs.map(|sub| entity_row(doc, sub)).collect(),
    }
}

/// Each asset whose published package, as this computer knows it or can ask at once, has no adapter for its kind. Local code is
/// described only on a machine, so a render says it.
async fn unadapted(api: &Arc<Api>, doc: &Doc, folder: &Folder) -> Vec<String> {
    let host = Host::new(api);
    let (mut said, mut known) = (vec![], HashMap::new());
    for i in (0..doc.assets.len()).filter(|&i| doc.makes(i) && doc.tool(i).is_none()) {
        let with = doc.target(i);
        if protocol::target::written_as_path(&with) || with.ends_with(".py") {
            continue;
        }
        if !known.contains_key(&with) {
            let asked = tokio::time::timeout(std::time::Duration::from_secs(3), host.published("", &with, None)).await;
            known.insert(with.clone(), asked.ok().and_then(Result::ok));
        }
        let Some(package) = &known[&with] else { continue };
        said.extend(read_tags(doc, folder, i, package));
        if doc.assets[i].pin.is_none() && package.adapter(&doc.assets[i].kind).is_none() {
            said.push(no_adapter(&doc.assets[i].name, &package.name, &doc.assets[i].kind));
        }
    }
    said
}

/// Keeps `package`'s tags in project.lock for what reads the file, and answers what asset `i` couldn't be told until now.
fn read_tags(doc: &Doc, folder: &Folder, i: usize, package: &Package) -> Vec<String> {
    let (key, tags) = (format::plain_with(&doc.target(i)), package.reserved());
    let same = folder.lock().is_ok_and(|lock| lock.reserved.get(&key) == Some(&tags));
    if !same && let Err(error) = folder.update_lock(|lock| drop(lock.reserved.insert(key.clone(), tags.clone()))) {
        warn!("{}: {error}", key);
    }
    doc.said_once_read(i, &tags)
}

/// The line that says an asset can't be made by what names it.
fn no_adapter(name: &str, package: &str, kind: &str) -> String {
    format!("{name}: {package} has no adapter for kind {kind}: it can't be made")
}

/// What a cut of asset `i` is at the root: `<name>.mp4`.
fn cut_name(name: &str) -> String {
    format!("{}.mp4", name.replace(['/', '\\'], "_"))
}

fn cut_row(doc: &Doc, folder: &Folder, lock: &Lock, stand: &[Standing], i: usize) -> Cut {
    let asset = &doc.assets[i];
    let name = cut_name(&asset.name);
    let path = folder.dir.join(&name);
    let cut_at_ms = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .map(|at| at.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64));
    let canvas = Canvas::of(&asset.fields, &mut vec![]);
    let (mut pieces, skips) = tools::pieces(&asset.fields, &stand[i].reads, folder, lock);
    tools::past_end(&mut pieces, canvas.fps);
    let made = tools::measure(&pieces, canvas.fps);
    let (frames, retimed) = (made.as_ref().map(|made| made.frames), made.map(|made| made.retimed).unwrap_or_default());
    Cut { cut: name, path, fps: canvas.fps, cut_at_ms, frames, retimed, skips }
}

/// What its versions are called.
fn noun(doc: &Doc, lock: &Lock, i: usize) -> &'static str {
    let asset = &doc.assets[i];
    let video = lock.asset(&asset.name).is_some_and(|held| held.versions.iter().any(|version| version.files.contains_key("video")));
    match video || asset.kind == "video" {
        true => "take",
        false => "version",
    }
}

/// A set of files as the API lists them.
fn listed(lock: &Lock, files: &Files) -> BTreeMap<String, Listing> {
    (files.iter())
        .filter_map(|(name, sha256)| Some((name.clone(), Listing { path: lock.files.get(sha256)?.path.clone(), sha256: sha256.clone() })))
        .collect()
}

/// The path of the file people mean by `files`.
fn main_path(lock: &Lock, files: &Files) -> Option<String> {
    folder::main_of(files).and_then(|sha256| lock.files.get(sha256)).map(|file| file.path.clone())
}

/// Asset `i` as `GET /v1/projects/{id}` lists it.
fn row(doc: &Doc, folder: &Folder, lock: &Lock, stand: &[Standing], i: usize) -> Row {
    let (asset, standing) = (&doc.assets[i], &stand[i]);
    let noun = noun(doc, lock, i);
    let held = lock.asset(&asset.name);
    let making: Vec<u64> = (lock.pending.values())
        .filter(|pending| name_key(&pending.asset) == name_key(&asset.name) && pending.run > 0)
        .map(|pending| pending.run)
        .collect();
    let files = standing.files.clone().unwrap_or_default();
    let gone: Vec<&str> = (files.values())
        .filter(|sha256| lock.files.get(*sha256).is_none_or(|file| folder.missing(file)))
        .map(|sha256| folder::prefix_of(sha256))
        .collect();
    let status = match (making.as_slice(), &standing.state) {
        ([run], _) => format!("making (run {run})"),
        (runs @ [_, ..], _) => format!("making (runs {})", runs.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")),
        ([], _) if !gone.is_empty() => format!("not on this computer: {}: cozy project pull", gone.join(", ")),
        ([], Stand::Pinned { version: None, .. }) => "pinned".into(),
        ([], Stand::Pinned { version: Some(version), changed }) if changed.is_empty() => format!("pinned: {noun} {version}"),
        ([], Stand::Pinned { version: Some(version), changed }) => format!("pinned: {noun} {version}; its inputs changed: {}", changed.join(", ")),
        ([], Stand::Current(_)) => "current".into(),
        ([], Stand::Linked) => "linked".into(),
        ([], Stand::Written { by, version }) => format!("written by {by} {version}"),
        ([], Stand::Unwritten { by }) => format!("{by} has not written it"),
        ([], Stand::Waits) => "waits for what it reads".into(),
        ([], Stand::Stale(why)) => format!("out of date: {why}"),
        ([], Stand::Missing) if asset.hollow => "it has nothing to make".into(),
        ([], Stand::Missing) => format!("no {noun} yet"),
    };
    let made = standing.version().and_then(|number| held?.version(number));
    let row_standing = match (making.is_empty(), &standing.state) {
        (false, _) => RowStanding::Making,
        (true, Stand::Stale(_)) => RowStanding::OutOfDate,
        (true, Stand::Missing | Stand::Waits | Stand::Unwritten { .. }) => RowStanding::Missing,
        (true, _) => RowStanding::Current,
    };
    Row {
        name: asset.name.clone(),
        standing: row_standing,
        making,
        kind: asset.kind.clone(),
        noun: noun.into(),
        with: doc.makes(i).then(|| doc.target(i)),
        versions: held.map_or(0, |held| held.versions.len() as u32),
        prefixes: (held.into_iter().flat_map(|held| &held.versions))
            .filter_map(|version| Some(VersionPrefix { number: version.number, prefix: folder::prefix_of(version.main()?).to_owned(), kept: version.kept }))
            .collect(),
        approved: asset.pin.as_ref().and(standing.version()),
        pinned: asset.pin.is_some(),
        inputs: standing.inputs.clone(),
        path: main_path(lock, &files),
        files: listed(lock, &files),
        made_by: made.and_then(|made| made.made_by.as_ref()).map(|by| by.with.clone()),
        status,
    }
}

/// `GET /v1/projects/{id}/assets/{name}`: the asset and each of its versions.
async fn asset(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Json<AssetVersions>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &name).map_err(Refusal)?;
    let row = row(&doc, folder, &lock, &doc.stand(&lock), i);
    let held = lock.asset(&row.name);
    let pinned = row.approved;
    let versions = (held.into_iter().flat_map(|held| &held.versions))
        .map(|version| VersionRow {
            number: version.number,
            files: listed(&lock, &version.files),
            made_by: version.made_by.as_ref().map(|by| by.with.clone()),
            seed: version.seed,
            runs: version.runs.clone(),
            approved: Some(version.number) == pinned,
            warnings: version.warnings.clone(),
            kept: version.kept,
        })
        .collect();
    Ok(Json(AssetVersions { asset: row, versions }))
}

/// The assets project.lock keeps versions of that project.yaml no longer names, each with the one new asset of its kind with no
/// versions, if there is exactly one: what a rename in project.yaml left. Each file's bytes count once, and only if nothing else names it.
fn orphans(doc: &Doc, folder: &Folder, lock: &Lock) -> Vec<Orphan> {
    let kept = kept_files(doc, lock);
    let empty = |kind: &str| -> Vec<&str> {
        (0..doc.assets.len())
            .filter(|&i| doc.makes(i) && doc.assets[i].kind == kind && lock.asset(&doc.assets[i].name).is_none_or(|held| held.versions.is_empty()))
            .map(|i| doc.assets[i].name.as_str())
            .collect()
    };
    let mut counted = BTreeSet::new();
    (lock.assets.iter())
        .filter(|(key, held)| doc.holds(key).is_none() && !held.versions.is_empty())
        .map(|(key, held)| {
            let files = held.versions.iter().flat_map(|version| version.files.values().chain(version.writes.values().flat_map(BTreeMap::values)));
            let freed: Vec<&String> = files.filter(|sha256| !kept.contains(*sha256) && counted.insert((*sha256).clone())).collect();
            Orphan {
                name: if held.name.is_empty() { key.clone() } else { held.name.clone() },
                kind: held.kind.clone(),
                versions: held.versions.len() as u32,
                bytes: freed.iter().map(|sha256| folder.size(lock, sha256)).sum(),
                renamed_to: match empty(&held.kind).as_slice() {
                    [only] => Some((*only).to_owned()),
                    _ => None,
                },
            }
        })
        .collect()
}

/// Every file something the file names uses: its assets' versions, what is on its way, and its file pins.
fn kept_files(doc: &Doc, lock: &Lock) -> BTreeSet<String> {
    let mut kept: BTreeSet<String> = lock.named(|key| doc.holds(key).is_some()).into_iter().map(str::to_owned).collect();
    kept.extend(doc.assets.iter().filter_map(|asset| Some(asset.pin.as_ref()?.1.clone())));
    kept
}

/// `POST /v1/projects/{id}/reclaim`: what it would free, and with `yes` frees it: orphans' versions, and any file nothing else
/// names. Refused while the project renders. Never automatic.
async fn reclaim(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Reclaimed>, Refusal> {
    let asked: Reclaim = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let filter = crate::store::RunFilter {
        status: None,
        target: None,
        machine: None,
        out_dir: Some(landing(&folder.dir).to_string_lossy().into_owned()),
        before: None,
        limit: 100,
    };
    let going = (api.calcifer.store().runs(&filter).map(|(rows, ..)| rows).unwrap_or_default().iter()).any(|row| row.state != crate::store::State::Settled);
    if asked.yes && (going || *project.walking.lock().expect("walking lock")) {
        let said = "the project is rendering: reclaim once its runs land";
        return Err(Refusal(Error::new(Kind::Conflict, "project_busy", said).next("cozy project status")));
    }
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let (orphans, kept) = (orphans(&doc, folder, &lock), kept_files(&doc, &lock));
    if asked.yes {
        // An orphaned cut's film at the root goes with it.
        for held in lock.assets.iter().filter(|(key, _)| doc.holds(key).is_none()).map(|(_, held)| held) {
            let cut = held.versions.iter().any(|version| version.made_by.as_ref().is_some_and(|by| by.with == "cozy/cut"));
            if cut {
                let name = cut_name(&held.name);
                let _ = std::fs::remove_file(folder.dir.join(&name));
                let _ = std::fs::remove_file(folder.dir.join(FsPath::new(&name).with_extension("otio")));
            }
        }
        folder.update_lock(|lock| lock.assets.retain(|key, _| doc.holds(key).is_some())).map_err(Refusal)?;
    }
    let (files, bytes) = folder.sweep(&kept, asked.yes).map_err(Refusal)?;
    Ok(Json(Reclaimed { orphans, files, bytes, deleted: asked.yes }))
}

/// Keeps what project.lock has of `from` as `to`, an asset project.yaml names now with no versions: its versions and pin, and what
/// is pending or owed of it. Its readers' text changed, so they go out of date unless pinned. Refused while the project renders.
/// Nothing on disk moves.
async fn rename(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Renamed>, Refusal> {
    let asked: Rename = parse(&body?)?;
    format::named(&asked.to).map_err(Refusal)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let refuse = |kind, code, said: String| Refusal(Error::new(kind, code, said));
    if *project.walking.lock().expect("walking lock") {
        return Err(refuse(Kind::Conflict, "project_busy", "the project is rendering: rename once its runs land".into()));
    }
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let Some(old) = lock.asset(&asked.from).filter(|held| !held.versions.is_empty()) else {
        return Err(refuse(Kind::NotFound, "asset_unknown", format!("project.lock keeps no version of {}", asked.from)));
    };
    let Some(i) = doc.holds(&asked.to) else {
        let said = format!("{} names no {}: rename the asset (and its <mentions>) there first", folder::PROJECT, asked.to);
        return Err(refuse(Kind::NotFound, "asset_unknown", said));
    };
    let to = doc.assets[i].name.clone();
    if lock.asset(&to).is_some_and(|held| !held.versions.is_empty()) {
        return Err(refuse(Kind::Conflict, "rename_taken", format!("{to} has versions of its own already")));
    }
    let (from, versions) = (asked.from.clone(), old.versions.len() as u32);
    folder.update_lock(|lock| renamed(lock, &from, &to)).map_err(Refusal)?;
    Ok(Json(Renamed { from, to, versions }))
}

/// project.lock with asset `from` called `to`: its row, and what is pending or owed of it.
fn renamed(lock: &mut Lock, from: &str, to: &str) {
    let key = name_key(from);
    if let Some(mut held) = lock.assets.remove(&key) {
        held.name = to.to_owned();
        lock.assets.insert(name_key(to), held);
    }
    for pending in lock.pending.values_mut().filter(|pending| name_key(&pending.asset) == key) {
        pending.asset = to.into();
    }
    for ask in &mut lock.walk.asks {
        for take in ask.takes.iter_mut().filter(|take| name_key(&take.asset) == key) {
            take.asset = to.into();
        }
        if ask.sent.remove(&key) {
            ask.sent.insert(name_key(to));
        }
    }
}

/// Adds what `Runs` asks to the project's walk, answered at once (`202`; `200` when its
/// `Idempotency-Key` asked it already); `/events` says how it goes.
async fn runs(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Asked>), Refusal> {
    let body = body?;
    let asked: Runs = parse(&body)?;
    if asked.on.is_empty() || asked.on.iter().any(|on| matches!(on, On::New(_))) {
        let said = "a project's runs go to machines: rent one first (`cozy machine rent`)";
        return Err(Refusal(Error::new(Kind::Usage, "project_on_new", said)));
    }
    if asked.approve && asked.n > 1 {
        let said = format!("--approve approves the one new version; -n {} makes {}", asked.n, asked.n);
        return Err(Refusal(Error::new(Kind::Usage, "approve_one", said).remedy("drop --approve, or -n")));
    }
    let hub = match asked.hub.is_empty() {
        true => api.machines.hubs.hub.to_string(),
        false => asked.hub.clone(),
    };
    Origin::parse(&hub).map_err(Refusal)?;
    let project = api.projects.get(&api, &id)?;
    let idempotency = headers.get("idempotency-key").and_then(|key| key.to_str().ok()).map(|key| (key.to_owned(), hex::encode(Sha256::digest(&body))));
    if let Some((key, digest)) = &idempotency {
        // Kept in the lock too while the walk goes: the answer to a retry must outlive a restart, now each ask makes new versions.
        let kept = project.folder.lock().map_err(Refusal)?.walk.asked.get(key).cloned();
        let mut keys = project.keys.lock().expect("keys lock");
        match keys.get(key).or(kept.as_ref()) {
            Some(seen) if seen == digest => {
                return Ok((StatusCode::OK, Json(Asked { id, path: project.folder.dir.clone(), joined: false, added: 0, skipped: vec![], queued: vec![] })));
            }
            Some(_) => {
                let said = format!("Idempotency-Key {key} asked another render already");
                return Err(Refusal(Error::new(Kind::Conflict, "idempotency_conflict", said)));
            }
            None => drop(keys.insert(key.clone(), digest.clone())),
        }
    }
    // What this render uses: the file as it is now. One that doesn't read refuses it, and only it.
    let text = project.folder.text().map_err(Refusal)?;
    let doc = Doc::read(&project.folder, &text).map_err(Refusal)?;
    let digest = hex::encode(Sha256::digest(text.as_bytes()));
    let target = Target { on: asked.on, hub };
    let all = matches!(asked.names.as_slice(), [only] if name_key(only) == name_key(ALL));
    let lock = project.folder.lock().map_err(Refusal)?;
    let stand = doc.stand(&lock);
    let say = |message: String| drop(project.events.send(Event::Warning { message }));
    let said = |message: String| drop(project.events.send(Event::Said { message }));
    let n = asked.n.max(1);
    // Every render makes new versions, unless it asks only for what has no state.
    let missing = asked.missing;
    if missing && n > 1 {
        say("-n makes new versions, and --missing only what has no state: it is ignored".into());
    }
    let keeping = asked.take.is_some() || asked.in_s.is_some();
    if keeping && (all || asked.names.len() != 1) {
        say(format!("--take and --in keep the start of one asset: with {} they are ignored", if all { "all" } else { "several names" }));
    }
    if asked.approve && (all || asked.names.len() != 1) {
        say("--approve is for one asset: it is ignored".into());
    }
    let mut named = vec![];
    let mut first = None;
    // Pinned, and not named: what `all` or an entity's name covers leaves it be. Naming one is what re-renders it.
    let mut skipped = vec![];
    let mut covering = |i: usize, named: &mut Vec<usize>| match doc.assets[i].pin.is_some() {
        true if !doc.assets[i].hollow && !missing => skipped.push(doc.assets[i].name.clone()),
        true => {}
        false => named.push(i),
    };
    if all {
        if !missing {
            for &i in doc.order.iter().filter(|&&i| doc.makes(i) && doc.tool(i).is_none()) {
                covering(i, &mut named);
            }
        }
    } else {
        for name in &asked.names {
            if let Some(i) = doc.find(name).filter(|&i| doc.assets[i].links.is_some()) {
                doc.linked(i).into_iter().for_each(|j| covering(j, &mut named));
                continue;
            }
            match find(&doc, name) {
                Ok(i) => named.push(i),
                Err(error) => {
                    say(error.message.clone());
                    first.get_or_insert(error);
                }
            }
        }
    }
    let mut takes = vec![];
    let one = !all && asked.names.len() == 1 && named.len() == 1;
    let approve = asked.approve && one;
    for i in named {
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        if asset.pin.is_some() && asset.hollow {
            said(format!("{name} is a constant, one file: nothing is made of it"));
            continue;
        }
        if let Some(writer) = asset.writer {
            said(format!("{name} comes with {}'s take", doc.assets[writer].name));
            continue;
        }
        if doc.tool(i).is_some() {
            match missing {
                true => said(format!("{name} is made here, free, when what it reads settles")),
                // Named, it is made again, as anything named is.
                false => takes.push(Take { key: new_key(), asset: name.clone(), target: target.clone(), approve: false, new: true, keeps: None }),
            }
            if approve {
                say(format!("--approve pins what a run lands: `cozy project approve {name}` pins this once it is made"));
            }
            continue;
        }
        if asset.hollow {
            say(doc.nothing_to_make(i));
            continue;
        }
        let keeps = match keeping && one {
            false => None,
            true => keeps_of(&doc, &lock, &stand, i, asked.take, asked.in_s).map_err(Refusal)?,
        };
        if keeping && keeps.is_none() && !all {
            say(format!("{name} has no take to keep the start of: --take and --in are ignored"));
        }
        let noun = noun(&doc, &lock, i);
        // Already on its way, the same inputs: another's version answers this.
        let making = lock.pending.values().any(|pending| name_key(&pending.asset) == name_key(&name) && Some(&pending.version.inputs) == stand[i].inputs.as_ref());
        let asked_of = lock.walk.asks.iter().any(|ask| ask.digest == digest && ask.takes.iter().any(|take| name_key(&take.asset) == name_key(&name)));
        if missing && keeps.is_none() && (making || asked_of) {
            said(format!("{name} is being made"));
            continue;
        }
        if missing && keeps.is_none() && stand[i].files.is_some() {
            said(match &stand[i].state {
                Stand::Pinned { changed, .. } if !changed.is_empty() => format!("{name} is pinned, though {} changed: unpin to make it again", changed.join(", ")),
                Stand::Pinned { .. } => format!("{name} is pinned: without --missing, naming it makes another {noun} to look at"),
                Stand::Current(version) => format!("{name} is current, {noun} {version}; without --missing it makes another"),
                _ => format!("{name} has its state; without --missing it makes another {noun}"),
            });
            continue;
        }
        let makes_new = !missing || keeps.is_some();
        let take = |_| Take { key: new_key(), asset: name.clone(), target: target.clone(), approve, new: makes_new, keeps };
        takes.extend((0..if makes_new { n } else { 1 }).map(take));
    }
    if let (Some(error), true, false) = (first, takes.is_empty(), all) {
        return Err(Refusal(error));
    }
    let joined = *project.walking.lock().expect("walking lock");
    let new_all = all && missing && !lock.walk.asks.iter().any(|ask| ask.all && ask.digest == digest);
    let added = takes.len() as u32 + u32::from(new_all);
    let before = queue(&project.folder, &lock);
    let ask = Ask { id: new_key(), text, digest, target: target.clone(), all: all && missing, sent: BTreeSet::new(), takes };
    (project.folder.update_lock(|lock| {
        if !joined || added > 0 {
            lock.walk.asks.push(ask.clone());
        }
        lock.walk.asked.extend(idempotency.iter().cloned());
    }))
    .map_err(Refusal)?;
    let after = queue(&project.folder, &project.folder.lock().map_err(Refusal)?);
    let queued = (after.iter().enumerate())
        .filter(|(_, item)| !before.contains(item))
        .map(|(at, (item, _))| QueuedNow { name: item.name.clone(), ahead: after[..at].iter().map(|(ahead, _)| ahead.name.clone()).collect() })
        .collect();
    walk(&api, &project);
    Ok((StatusCode::ACCEPTED, Json(Asked { id, path: project.folder.dir.clone(), joined, added, skipped, queued })))
}

/// What the project has to do, in the order it goes: what is running, then what each render has queued, each with its take's key if
/// it is one. Each render's by its own text.
fn queue(folder: &Folder, lock: &Lock) -> Vec<(Queued, Option<String>)> {
    let item = |name: &str| Queued { name: name.to_owned(), running: false, run: None };
    let mut items: Vec<(Queued, Option<String>)> = (lock.pending.iter())
        .map(|(key, pending)| (Queued { name: pending.asset.clone(), running: true, run: (pending.run > 0).then_some(pending.run) }, Some(key.clone())))
        .collect();
    for ask in &lock.walk.asks {
        let waiting = |take: &&Take| !lock.pending.contains_key(&take.key);
        let Ok(doc) = Doc::read(folder, &ask.text) else {
            items.extend(ask.takes.iter().filter(waiting).map(|take| (item(&take.asset), Some(take.key.clone()))));
            continue;
        };
        let stand = doc.stand(lock);
        let covered = covered(&doc, &stand, ask);
        for &i in &doc.order {
            let (name, key) = (&doc.assets[i].name, name_key(&doc.assets[i].name));
            let takes: Vec<&Take> = ask.takes.iter().filter(waiting).filter(|take| name_key(&take.asset) == key).collect();
            items.extend(takes.iter().map(|take| (item(name), Some(take.key.clone()))));
            let owed = covered[i] && stand[i].files.is_none() && doc.makes(i) && doc.tool(i).is_none() && !ask.sent.contains(&key);
            let there = items.iter().any(|(there, _)| name_key(&there.name) == key);
            if takes.is_empty() && owed && !there {
                items.push((item(name), None));
            }
        }
    }
    items
}

/// `POST /v1/projects/{id}/cancel`: the names' queued work dropped and their runs canceled, as `cozy run cancel` does. `all` is
/// everything, and what the walk owes for it. What is dropped isn't sent again: what the walk owes beyond its takes is marked sent.
async fn cancel(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Cancelled>, Refusal> {
    let asked: Cancel = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let lock = folder.lock().map_err(Refusal)?;
    let all = matches!(asked.names.as_slice(), [only] if name_key(only) == name_key(ALL));
    let mut keys = BTreeSet::new();
    if !all {
        // Only an entity's name needs a file: this one, or if it doesn't read, the newest render's.
        let doc = Doc::load(folder).ok().or_else(|| lock.walk.asks.last().and_then(|ask| Doc::read(folder, &ask.text).ok()));
        for name in &asked.names {
            match doc.as_ref().and_then(|doc| Some((doc, doc.find(name).filter(|&i| doc.assets[i].links.is_some())?))) {
                Some((doc, i)) => keys.extend(doc.linked(i).into_iter().map(|j| name_key(&doc.assets[j].name))),
                None => drop(keys.insert(name_key(name))),
            }
        }
    }
    let wanted = |name: &str| all || keys.contains(&name_key(name));
    let (mut dropped, mut owed) = (vec![], vec![]);
    for (item, take) in queue(folder, &lock).into_iter().filter(|(item, _)| !item.running && wanted(&item.name)) {
        if take.is_none() {
            owed.push(name_key(&item.name));
        }
        dropped.push(item.name);
    }
    folder
        .update_lock(|lock| {
            for ask in &mut lock.walk.asks {
                ask.takes.retain(|take| !wanted(&take.asset));
                ask.all &= !all;
                ask.sent.extend(owed.iter().cloned());
            }
        })
        .map_err(Refusal)?;
    let store = api.calcifer.store();
    let mut canceled = vec![];
    for pending in lock.pending.values().filter(|pending| pending.run > 0 && wanted(&pending.asset)) {
        let Ok(Some(row)) = store.run_by_number(pending.run) else { continue };
        if row.state != crate::store::State::Settled {
            api.calcifer.cancel(&row.id).map_err(|e| Refusal(Error::new(Kind::Internal, "run_cancel", e.to_string())))?;
            canceled.push(pending.run);
        }
    }
    project.asked.notify_one();
    Ok(Json(Cancelled { dropped, canceled }))
}

/// What `--take` and `--in` ask of asset `i`: the take a new one keeps the start of, the one that is its state, else its newest, if
/// not named. A named take it doesn't have is an error.
fn keeps_of(doc: &Doc, lock: &Lock, stand: &[Standing], i: usize, take: Option<u32>, in_s: Option<f64>) -> Result<Option<Keeps>, Error> {
    let name = &doc.assets[i].name;
    let versions = lock.asset(name).map(|held| held.versions.as_slice()).unwrap_or_default();
    match take {
        Some(take) if versions.iter().any(|version| version.number == take) => Ok(Some(Keeps { take, in_s })),
        Some(take) if versions.is_empty() => Err(Error::new(Kind::Usage, "take_unknown", format!("{name} has no take {take}: it has none yet"))),
        Some(take) => {
            let numbers: Vec<String> = versions.iter().map(|version| version.number.to_string()).collect();
            Err(Error::new(Kind::Usage, "take_unknown", format!("{name} has no take {take}: it has {}", numbers.join(", "))))
        }
        None => Ok(stand[i].version().or(versions.last().map(|version| version.number)).map(|take| Keeps { take, in_s })),
    }
}

/// `source` kept as a file of the project, in project.lock, with the line to paste.
async fn kept(folder: &Folder, source: &str) -> Result<Imported, String> {
    let folder::Kept { file, field, already } = folder.import(source).await?;
    let prefix = folder::prefix_of(&file.sha256).to_owned();
    let line = format!("{field}: {}", format::yaml_scalar(&prefix));
    Ok(Imported { path: file.path, sha256: file.sha256, prefix, line, already, run: None, note: None })
}

/// A picture's background removed by a run of the project, which lands the one file kept. With nothing to remove, or no way to run
/// it, a plain copy stands instead, and says why. The one place Calcifer names this package.
async fn remove_background(api: &Arc<Api>, folder: &Folder, at: PathBuf, field: &str, on: Option<On>) -> Result<Imported, String> {
    let note = if field != "image" {
        "--remove-background takes a picture: this is kept as it is".to_owned()
    } else if folder.transparent(&at) >= OPAQUE_BELOW {
        "already transparent, so it is kept as it is".to_owned()
    } else {
        let image = FileTerm { name: Some("image".into()), file: at.to_string_lossy().into_owned() };
        let ask = RunRequest { target: "birefnet/remove_background".into(), on, files: vec![image], project: Some(folder.dir.clone()), ..Default::default() };
        match crate::runs::request(api, &new_key(), ask).await {
            Ok(submitted) => {
                let run = Some(Box::new(submitted.run));
                return Ok(Imported { path: String::new(), sha256: String::new(), prefix: String::new(), line: String::new(), already: false, run, note: None });
            }
            Err(error) if error.code == "local_unavailable" => format!("{}: rent one with `cozy machine rent`, or add `--on <machine>`; a plain copy is kept", error.message),
            Err(error) => format!("{}; a plain copy is kept", error.message),
        }
    };
    let imported = kept(folder, &at.to_string_lossy()).await?;
    // A URL's bytes were only here to look at.
    if at.starts_with(folder.dir.join(folder::RUNS)) {
        let _ = std::fs::remove_file(&at);
    }
    Ok(Imported { note: Some(note), ..imported })
}

/// Keeps the file at `source` as a file of the project, in project.lock, and answers the line to paste.
async fn import(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Imported>, Refusal> {
    let asked: Import = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let failed = |why: String| {
        let said = format!("{}: {why}", asked.source);
        Refusal(Error::new(Kind::Validation, "import_failed", said).remedy("cozy project import takes a picture, video or sound, by path or http(s) URL"))
    };
    let imported = match asked.remove_background {
        false => kept(folder, &asked.source).await,
        true => {
            let (at, field) = folder.source(&asked.source).await.map_err(failed)?;
            remove_background(&api, folder, at, field, asked.on.clone()).await
        }
    };
    Ok(Json(imported.map_err(failed)?))
}

/// `name` pinned to a file in project.yaml: one line, in the field of its kind, written over what the file holds this moment. An
/// approved asset is the same as any pinned one.
fn pin_in_file(folder: &Folder, name: &str, field: &str, sha256: &str) -> Result<(), Error> {
    let value = format::yaml_scalar(folder::prefix_of(sha256));
    folder.edit_project(|text| format::pinned(text, name, field, &value))
}

/// `POST /v1/projects/{id}/approve`: the version's main file, by hash, in the asset's line of project.yaml: its whole file set is the
/// asset's state until `unpin`.
async fn approve(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Approved>, Refusal> {
    let asked: Approve = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &asked.name).map_err(Refusal)?;
    let asset = &doc.assets[i];
    if let Some(writer) = asset.writer {
        let said = format!("{} comes with {}'s take: approve that take", asset.name, doc.assets[writer].name);
        return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said)));
    }
    let noun = noun(&doc, &lock, i);
    let versions: Vec<&Version> = lock.asset(&asset.name).into_iter().flat_map(|held| &held.versions).collect();
    // A file of the project's, by hash: any picture can be what this asset is.
    let chosen = match (&asked.file, asked.version) {
        (Some(prefix), _) => match lock.matching(&prefix.to_ascii_lowercase()).as_slice() {
            [file] => file.sha256.clone(),
            [] => {
                let said = format!("no file of this project's starts with {prefix}");
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy project status")));
            }
            several => {
                let said = format!("{prefix} fits {} files of this project: write more of its hash", several.len());
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said)));
            }
        },
        (None, wanted) => {
            let Some(sha256) = versions.iter().find(|version| Some(version.number) == wanted).and_then(|version| version.main()) else {
                let numbers: Vec<String> = versions.iter().map(|version| version.number.to_string()).collect();
                let said = match numbers.as_slice() {
                    [] => format!("{} has no {noun} yet", asset.name),
                    numbers => format!("{} has no {noun} {}: it has {}", asset.name, wanted.unwrap_or_default(), numbers.join(", ")),
                };
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy project status")));
            };
            sha256.clone()
        }
    };
    let field = asset.kind.as_str();
    if let Some(is) = lock.files.get(&chosen).map(folder::File::field).filter(|is| asked.file.is_some() && *is != field) {
        let said = format!("{} makes one {field}, and that file is {is}", asset.name);
        return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).remedy(format!("if it should be {is}, write `kind: {is}` on it"))));
    }
    let version = versions.iter().find(|version| version.main() == Some(&chosen));
    if !doc.declared(i) && asset.line.is_none() {
        let said = format!("{} is a frame its writer's beat names, with no lines of its own in {}", asset.name, folder::PROJECT);
        return Err(Refusal(Error::new(Kind::Usage, "project_edit", said).remedy(format!("add it under assets: with `{field}: {}`", format::yaml_scalar(folder::prefix_of(&chosen))))));
    }
    let before = stale_names(&doc, &doc.stand(&lock));
    let name = asset.name.clone();
    // A hash written in an entity's link is rewritten there.
    let (owner, written_in) = asset.line.as_ref().map_or((name.as_str(), field), |(owner, link)| (owner.as_str(), link.as_str()));
    pin_in_file(folder, owner, written_in, &chosen).map_err(Refusal)?;
    let warnings = settle(&api, &project).await;
    let files = version.map_or_else(|| Files::from([(field.to_owned(), chosen.clone())]), |version| version.files.clone());
    let (number, path) = (version.map_or(0, |version| version.number), main_path(&lock, &files).unwrap_or_default());
    let files = listed(&lock, &files);
    let stale = newly_stale(folder, &before).map_err(Refusal)?;
    Ok(Json(Approved { name, noun: noun.into(), number, files, path, stale, warnings }))
}

/// `POST /v1/projects/{id}/unpin`: the pin's line gone from project.yaml, so the asset's state follows its inputs again. A constant,
/// nothing but its file, keeps it: its line is all it is.
async fn unpin(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Unpinned>, Refusal> {
    let asked: Unpin = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &asked.name).map_err(Refusal)?;
    let asset = &doc.assets[i];
    let name = asset.name.clone();
    let stand = doc.stand(&lock);
    let version = stand[i].version().filter(|_| asset.pin.is_some());
    let before = stale_names(&doc, &stand);
    if let Some((field, _)) = &asset.pin {
        if asset.hollow {
            let said = format!("{name} is a constant, nothing but its {field}: to be rid of it, delete it from {}", folder::PROJECT);
            return Err(Refusal(Error::new(Kind::Usage, "project_edit", said)));
        }
        folder.edit_project(|text| format::unpinned(text, &name, field)).map_err(Refusal)?;
    }
    for message in settle(&api, &project).await {
        let _ = project.events.send(Event::Warning { message });
    }
    let stale = newly_stale(folder, &before).map_err(Refusal)?;
    Ok(Json(Unpinned { name, version, stale }))
}

fn stale_names(doc: &Doc, stand: &[Standing]) -> BTreeSet<String> {
    (0..doc.assets.len()).filter(|&i| matches!(stand[i].state, Stand::Stale(_))).map(|i| doc.assets[i].name.clone()).collect()
}

/// What is out of date now and wasn't in `before`.
fn newly_stale(folder: &Folder, before: &BTreeSet<String>) -> Result<Vec<String>, Error> {
    let doc = Doc::load(folder)?;
    let lock = folder.lock()?;
    Ok(stale_names(&doc, &doc.stand(&lock)).into_iter().filter(|name| !before.contains(name)).collect())
}

/// The tools whose reads have settled, made now, or by the walker if one is going. What fails is answered.
async fn settle(api: &Arc<Api>, project: &Arc<Project>) -> Vec<String> {
    let walker = Walk::new(api, project);
    // Of the file as it is now, whatever renders are going: they have their own text.
    let now = Ask::default();
    let mut tried: HashSet<String> = HashSet::new();
    loop {
        let ready = (|| -> Result<_, Error> {
            let (doc, lock) = (Doc::load(walker.folder())?, walker.folder().lock()?);
            let stand = doc.stand(&lock);
            let tools = plan(&doc, &stand, &lock, &now).tools;
            Ok((doc, lock, stand, tools))
        })();
        let (doc, lock, stand, tools) = match ready {
            Ok(ready) => ready,
            Err(error) => return vec![error.message],
        };
        let tools: Vec<usize> = tools.into_iter().filter(|&i| tried.insert(doc.assets[i].name.clone())).collect();
        if tools.is_empty() {
            return vec![];
        }
        for i in tools {
            if let Err(error) = walker.tool(&now, &doc, &stand, &lock, i).await {
                return vec![error.message];
            }
        }
    }
}

async fn events(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    // Heard from before what is going is read, so nothing falls between: a run can be told twice, never not at all.
    let tail = project.events.subscribe();
    let going = going(&api, &project.folder).map_err(Refusal)?;
    let told = going.into_iter().map(|event| Ok(sse::Event::default().json_data(&event).expect("an event serializes")));
    let stop = api.stop_signal();
    let stream = futures_util::stream::iter(told).chain(futures_util::stream::unfold((tail, stop), |(mut tail, mut stop)| async move {
        loop {
            let event = tokio::select! {
                event = tail.recv() => event,
                _ = stop.wait_for(|stopping| *stopping) => return None,
            };
            match event {
                Ok(event) => {
                    let sent = sse::Event::default().json_data(&event).expect("an event serializes");
                    return Some((Ok(sent), (tail, stop)));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// The runs going in the project now, as each was told when it began, for whoever comes after: one more render, or a page.
fn going(api: &Api, folder: &Folder) -> Result<Vec<Event>, Error> {
    let lock = folder.lock()?;
    let store = api.calcifer.store();
    let running = lock.pending.values().filter(|pending| pending.run > 0).filter_map(|pending| {
        let row = store.run_by_number(pending.run).ok()??;
        (row.state != crate::store::State::Settled).then(|| Event::Started { name: pending.asset.clone(), run: row.id.to_string(), number: pending.run })
    });
    Ok(running.collect())
}

/// project.yaml's ETag, as `text` holds it.
fn yaml_etag(text: &str) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(text)))
}

/// `GET /v1/projects/{id}/yaml`: project.yaml as it is this moment, and the ETag a `PUT` names.
async fn yaml(State(api): State<Arc<Api>>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let etag = yaml_etag(&project.folder.text().map_err(Refusal)?);
    send(&project.folder.project(), &etag, &headers)
}

/// `PUT /v1/projects/{id}/yaml`: these bytes are project.yaml, if it still holds what `If-Match` read, so a save made since stands.
/// A text that doesn't read as a project is refused, and the file left as it is. Nothing is rendered, and a render going keeps the
/// text it was asked with: what a save puts out of date waits for the next.
async fn save(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Refusal> {
    let Some(read) = headers.get(header::IF_MATCH).and_then(|value| value.to_str().ok()) else {
        let said = "a save names what it read: `If-Match` with the ETag of `GET /v1/projects/{id}/yaml`";
        return Err(Refusal(Error::new(Kind::Usage, "if_match_required", said)));
    };
    let body = body?;
    let text = std::str::from_utf8(&body).map_err(|e| Refusal(Error::new(Kind::Validation, "project_yaml", format!("{}: {e}", folder::PROJECT))))?;
    let project = api.projects.get(&api, &id)?;
    Doc::read(&project.folder, text).map_err(Refusal)?;
    (project.folder.edit_project(|now| match yaml_etag(now) == read {
        true => Ok(text.to_owned()),
        false => Err(Error::new(Kind::Conflict, "project_changed", format!("{} changed since it was read", folder::PROJECT))
            .remedy("read it again and make the edit on what it holds")),
    }))
    .map_err(Refusal)?;
    Ok((StatusCode::NO_CONTENT, [(header::ETAG, yaml_etag(text))]).into_response())
}

/// One of the project's files by its name in files/ (`<sha256>.<ext>`): content-addressed, so its sha256 is its ETag.
async fn file(
    State(api): State<Arc<Api>>,
    Path((id, file)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let (hex, ext) = file.split_once('.').unwrap_or((&file, ""));
    let named = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    if !named {
        let said = format!("{file} is no file of a project: they are named <sha256>.<ext>");
        return Err(Refusal(Error::new(Kind::Usage, "file_name", said)));
    }
    send(&project.folder.path(&format!("{}/{file}", folder::FILES)), &format!("\"{hex}\""), &headers)
}

/// One of the project's cuts (`edit.mp4`) or its timeline (`edit.otio`), as last made: its ETag names that cut.
async fn cut(
    State(api): State<Arc<Api>>,
    Path((id, cut)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let doc = Doc::load(&project.folder).map_err(Refusal)?;
    let made: Vec<String> = (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)).map(|i| cut_name(&doc.assets[i].name)).collect();
    let named = |name: &String| *name == cut || FsPath::new(name).with_extension("otio") == FsPath::new(&cut);
    if !made.iter().any(named) {
        let said = format!("the project makes no cut {cut}: it makes {}, each with its .otio", made.join(" and "));
        return Err(Refusal(Error::new(Kind::NotFound, "cut_unknown", said)));
    }
    let path = project.folder.dir.join(&cut);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let at = modified.and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |at| at.as_nanos());
    send(&path, &format!("\"{at:x}\""), &headers)
}

/// The file at `path` as `headers` ask for it (whole, or one range), or not found.
fn send(path: &FsPath, etag: &str, headers: &HeaderMap) -> Result<Response, Refusal> {
    let missing = |e: std::io::Error| {
        let said = format!("{} is not there: {e}", path.display());
        Refusal(Error::new(Kind::NotFound, "file_missing", said).next("cozy project render all"))
    };
    let file = std::fs::File::open(path).map_err(missing)?;
    let len = file.metadata().map_err(missing)?.len();
    crate::files::send(file, len, media_type(path), etag, headers)
        .map_err(|e| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display()))))
}

/// A version to send now: asset `i`'s because it has no state (take None), or a take's.
struct Next {
    i: usize,
    take: Option<Take>,
}

/// Where a run can read a file from.
enum Reach {
    /// In `files/`, or nowhere it knows of, which a pull answers.
    Here,
    /// That machine, one it may go to, holds it.
    There(On),
    /// The machine that keeps it is there to fetch it from.
    Fetch,
    /// With the machine that kept it.
    Gone,
}

/// A project's one walker: what project.lock owes, each operation sent once what it reads has settled, each run followed until it lands.
struct Walk {
    host: Host,
    project: Arc<Project>,
    began: Instant,
    /// What it landed, for its last word.
    tally: Mutex<Rendered>,
    /// The cuts it made, which it has said.
    cut: Mutex<BTreeSet<String>>,
}

impl Walk {
    fn new(api: &Arc<Api>, project: &Arc<Project>) -> Self {
        Self { host: Host::new(api), project: project.clone(), began: Instant::now(), tally: Mutex::default(), cut: Mutex::default() }
    }

    /// What it landed and how long it took, if it landed anything.
    fn rendered(&self) -> Option<Rendered> {
        let tally = self.tally.lock().expect("tally lock").clone();
        (tally.takes + tally.versions > 0).then(|| Rendered { elapsed_ms: self.began.elapsed().as_millis() as u64, ..tally })
    }

    fn folder(&self) -> &Folder {
        &self.project.folder
    }

    /// The project as the render `ask` was asked of it; one from no render in the lock, as the file reads now.
    fn doc_of(&self, ask: &str) -> Result<Doc, Error> {
        let text = self.folder().lock()?.walk.asks.into_iter().find(|mine| mine.id == ask).map(|mine| mine.text);
        match text {
            Some(text) => Doc::read(self.folder(), &text),
            None => Doc::load(self.folder()),
        }
    }

    /// What a render is left with, when nothing more of it can go, said; then it is done.
    async fn finish(&self, id: &str, first: &mut Option<Error>) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        if let Some(ask) = lock.walk.asks.iter().find(|ask| ask.id == id)
            && let Ok(doc) = Doc::read(self.folder(), &ask.text)
        {
            let stand = doc.stand(&lock);
            for message in stuck(&doc, &stand, &lock, ask) {
                self.failed(Error::new(Kind::NotFound, "file_missing", message), first);
            }
            self.say_cuts(&doc, &stand).await?;
        }
        self.folder().update_lock(|lock| {
            lock.walk.asks.retain(|ask| ask.id != id);
            if lock.walk.asks.is_empty() {
                lock.walk.asked.clear();
            }
        })
    }

    /// `run` sent under `key`. A machine that answers no connect has recorded nothing, so it is asked
    /// again under that key, each attempt said; one its Hub says stopped or ended is refused at once.
    async fn send_run(&self, key: &str, run: SubmitRun) -> Result<Run, Error> {
        const WAITS: [u64; 4] = [3, 6, 12, 24];
        for (attempt, wait) in WAITS.iter().enumerate() {
            match self.host.submit(key, run.clone()).await {
                Err(e) if e.code == "machine_unreachable" => {
                    let message = format!("{}; attempt {}/{} in {wait} s", e.message, attempt + 2, WAITS.len() + 1);
                    self.say(Event::Warning { message });
                    tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
                }
                answer => return answer,
            }
        }
        self.host.submit(key, run).await
    }

    fn say(&self, event: Event) {
        let _ = self.project.events.send(event);
    }

    /// Sends whatever is ready, makes each tool whose reads have settled, follows every pending run (what it sent, what an earlier
    /// walker sent), and plans again as each lands or something is asked, until nothing is owed: then the first thing that failed, if one did.
    async fn go(&self) -> Result<Option<Error>, Error> {
        let (mut first, mut following, mut followed) = (None, FuturesUnordered::new(), HashSet::new());
        let mut greeted = HashSet::new();
        loop {
            let mut moved = false;
            // Each render on its own text, all going at once: what is independent goes together.
            let asks: Vec<String> = self.folder().lock()?.walk.asks.into_iter().map(|ask| ask.id).collect();
            for id in asks {
                let Some(text) = self.folder().lock()?.walk.asks.into_iter().find(|ask| ask.id == id).map(|ask| ask.text) else { continue };
                let doc = match Doc::read(self.folder(), &text) {
                    Ok(doc) => doc,
                    Err(error) => {
                        self.failed(error, &mut first);
                        self.finish(&id, &mut first).await?;
                        continue;
                    }
                };
                // The lock after the doc, whose read measures what has landed: a cut and an adapter read those sizes.
                let lock = self.folder().lock()?;
                let Some(ask) = lock.walk.asks.iter().find(|ask| ask.id == id) else { continue };
                if greeted.insert(id.clone()) {
                    doc.warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
                }
                let stand = doc.stand(&lock);
                let planned = plan(&doc, &stand, &lock, ask);
                let idle = planned.send.is_empty() && planned.tools.is_empty();
                moved |= !idle;
                for next in planned.send {
                    if let Err(error) = self.send(ask, &doc, &stand, &lock, next).await {
                        self.failed(error, &mut first);
                    }
                }
                for i in planned.tools {
                    let made = self.tool(ask, &doc, &stand, &lock, i).await;
                    self.folder().update_lock(|lock| {
                        for mine in lock.walk.asks.iter_mut().filter(|mine| mine.id == id) {
                            mine.takes.retain(|take| name_key(&take.asset) != name_key(&doc.assets[i].name));
                        }
                    })?;
                    if let Err(error) = made {
                        self.failed(error, &mut first);
                        self.tried(&id, &doc.assets[i].name)?;
                    }
                }
                if idle && !planned.open && !lock.pending.values().any(|pending| pending.ask == id) {
                    self.finish(&id, &mut first).await?;
                }
            }
            for key in self.folder().lock()?.pending.into_keys() {
                if followed.insert(key.clone()) {
                    following.push(self.follow(key));
                }
            }
            if moved {
                continue;
            }
            if following.is_empty() {
                // Nothing is on its way: what is left can't be made.
                for ask in self.folder().lock()?.walk.asks {
                    self.finish(&ask.id, &mut first).await?;
                }
                if self.over()? {
                    return Ok(first);
                }
                continue;
            }
            tokio::select! {
                Some((key, landed)) = following.next() => {
                    followed.remove(&key);
                    if let Err(error) = landed? {
                        self.failed(error, &mut first);
                    }
                }
                () = self.project.asked.notified() => {}
            }
        }
    }

    /// Whether the walk is over, nothing owed or followed. Decided under `walking`, so an ask finds
    /// this walker going or starts the next.
    fn over(&self) -> Result<bool, Error> {
        let mut walking = self.project.walking.lock().expect("walking lock");
        *walking = self.folder().lock()?.owes();
        Ok(!*walking)
    }

    /// What failed: said now, never sent again this walk; what does not read it goes on.
    fn failed(&self, error: Error, first: &mut Option<Error>) {
        self.say(Event::Failed { error: error.clone() });
        first.get_or_insert(error);
    }

    /// Each current cut at the walk's end, what a render ends on, its film back at the root if someone moved it.
    async fn say_cuts(&self, doc: &Doc, stand: &[Standing]) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        for i in (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)) {
            let name = cut_name(&doc.assets[i].name);
            let Some(file) = stand[i].files.as_ref().and_then(|files| files.get("video")).and_then(|sha256| lock.files.get(sha256)) else { continue };
            let at = self.folder().dir.join(&name);
            if !at.is_file() {
                let _ = tools::publish(&self.folder().path(&file.path), &at);
            }
            if !at.with_extension("otio").is_file() {
                self.timeline_again(&doc.assets[i], &stand[i], &lock, &name).await;
            }
            if self.cut.lock().expect("cut lock").contains(&name) {
                continue;
            }
            let row = cut_row(doc, self.folder(), &lock, stand, i);
            self.say(Event::Cut { cut: name, frames: row.frames.unwrap_or(file.frames), fps: row.fps, retimed: row.retimed, took_s: 0.0, unchanged: true });
        }
        Ok(())
    }

    /// A standing cut's `.otio`, written again from its pieces; one that can't be says so and the walk goes on.
    async fn timeline_again(&self, asset: &format::Asset, standing: &Standing, lock: &Lock, name: &str) {
        let canvas = Canvas::of(&asset.fields, &mut vec![]);
        let (mut pieces, _) = tools::pieces(&asset.fields, &standing.reads, self.folder(), lock);
        tools::past_end(&mut pieces, canvas.fps);
        let project = self.project.clone();
        let (cut, fps) = (name.to_owned(), canvas.fps);
        let written = match self.host.ffmpeg().await {
            Ok((_, ffprobe)) => tokio::task::spawn_blocking(move || tools::timeline(&ffprobe, &project.folder, &pieces, fps, &cut))
                .await
                .map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))
                .and_then(|written| written),
            Err(error) => Err(error),
        };
        if let Err(error) = written {
            self.say(Event::Warning { message: format!("{name}'s timeline is gone and isn't written again: {}", error.message) });
        }
    }

    /// Sends `next` under its key: a new version of its asset on its target's machines, recorded as pending together with what
    /// it settles of the walk, before the run is sent; one that fails is settled all the same. An adapter that answers no run
    /// lands it at once, the asset's files being what it reads.
    async fn send(&self, ask: &Ask, doc: &Doc, stand: &[Standing], lock: &Lock, next: Next) -> Result<(), Error> {
        let i = next.i;
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        let key = next.take.as_ref().map_or_else(new_key, |take| take.key.clone());
        let settle = |lock: &mut Lock| {
            let Some(mine) = lock.walk.asks.iter_mut().find(|mine| mine.id == ask.id) else { return };
            match &next.take {
                None => drop(mine.sent.insert(name_key(&name))),
                Some(_) => mine.takes.retain(|take| take.key != key),
            }
        };
        let (target, approve, new) = match &next.take {
            None => (ask.target.clone(), false, false),
            Some(take) => (take.target.clone(), take.approve, take.new),
        };
        let standing = &stand[i];
        let noun = noun(doc, lock, i);
        if !new && standing.files.is_some() {
            return self.folder().update_lock(settle);
        }
        match &standing.state {
            Stand::Stale(why) if !new => self.say(Event::Said { message: format!("{name}: out of date: {why}") }),
            Stand::Missing if !new => self.say(Event::Said { message: format!("{name}: no {noun} yet") }),
            _ => {}
        }
        let sent = async {
            let inputs = standing.inputs.clone().ok_or_else(|| cannot(&name, "what it reads has no state".into()))?;
            for read in &standing.reads {
                let gone: Vec<&str> = (read.files.values())
                    .filter(|sha256| lock.files.get(*sha256).is_none_or(|file| self.folder().missing(file)))
                    .map(|sha256| folder::prefix_of(sha256))
                    .collect();
                if !gone.is_empty() {
                    return Err(cannot(&name, format!("{} of {} is not on this computer: cozy project pull", gone.join(", "), read.name)));
                }
            }
            let package = self.host.package(&target.hub, &doc.target(i), &self.folder().dir, &target.on).await?;
            if let Some(moved) = &package.moved {
                self.say(Event::Warning { message: moved.clone() });
            }
            for message in read_tags(doc, self.folder(), i, &package) {
                self.say(Event::Warning { message });
            }
            let Some(adapter) = package.adapter(&asset.kind) else {
                self.say(Event::Warning { message: no_adapter(&name, &package.name, &asset.kind) });
                return self.folder().update_lock(settle);
            };
            let now = self.folder().lock()?;
            let holds = now.asset(&name).and_then(|held| held.versions.iter().rev().find(|version| version.inputs == inputs)).map(|version| version.number);
            let going = now.pending.values().any(|pending| name_key(&pending.asset) == name_key(&name) && pending.version.inputs == inputs);
            let keeps = next.take.as_ref().and_then(|take| take.keeps);
            let keep = keeps.and_then(|keeps| doc.keep(i, lock, keeps));
            if keeps.is_some() && keep.is_none() {
                self.say(Event::Warning { message: format!("{name}: that take holds no video to keep: the whole {noun} renders") });
            }
            if let Some(keep) = &keep
                && let Some(file) = keep["latents"]["sha256"].as_str().and_then(|sha256| lock.files.get(sha256))
                && matches!(self.reach(file, &target.on).await, Reach::Gone)
            {
                let said = format!("{name}: take {}'s latents went with its machine: `cozy project render {name}` renders the whole shot", keep["take"]);
                return Err(Error::new(Kind::NotFound, "take_latents_gone", said));
            }
            // What stays on a machine that's gone isn't read: an adapter does without it.
            let mut reads = standing.reads.clone();
            for read in &mut reads {
                for (field, sha256) in read.files.clone() {
                    if let Some(file) = lock.files.get(&sha256)
                        && matches!(self.reach(file, &target.on).await, Reach::Gone)
                    {
                        read.files.remove(&field);
                    }
                }
            }
            let mut warnings = vec![];
            if let Some(signature) = &adapter.signature {
                let unknown = (asset.own.iter()).filter(|field| asset.fields.contains_key(*field) && !signature.contains(*field));
                warnings.extend(unknown.map(|field| format!("{name}: {} reads no field {field}", adapter.target())));
            }
            let operation = doc.operation(i, &reads, lock, keep.clone(), stand);
            let (calls, said) = self.adapt(&adapter, &operation, &target.hub, &target.on).await?;
            warnings.extend(said);
            // No call: this asset's files are what it reads.
            if calls.is_empty() {
                warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
                let Some(number) = holds else {
                    let mut version = self.version(doc, standing, i, warnings, inputs.clone());
                    version.made_by = version.made_by.map(|by| MadeBy { with: adapter.target(), ..by });
                    return self.land_read(&name, &asset.kind, version, standing, noun, approve, settle);
                };
                self.say(Event::Said { message: format!("{name}: {} has nothing to make: {noun} {number} stands", package.name) });
                return self.folder().update_lock(settle);
            }
            let renderers = calls.iter().map(|call| package.function(&call.function)).collect::<Result<Vec<_>, _>>()?;
            // A function with no seed has one result per inputs.
            // An adapter that names a seed has fixed it.
            let fixed = calls.iter().any(|call| call.input.contains_key("seed"));
            let seeded = !fixed && renderers.iter().any(|renderer| renderer.accepts.contains("seed"));
            if !seeded && (holds.is_some() || going) {
                let stands = holds.map_or_else(|| "one is being made".to_owned(), |number| format!("{noun} {number} stands"));
                let why = if fixed { "fixes its seed" } else { "takes no seed" };
                self.say(Event::Said { message: format!("{name}: {} {why}, so it has one result per inputs: {stands}", package.name) });
                return self.folder().update_lock(settle);
            }
            if !asset.writes.is_empty() && !renderers.iter().any(|renderer| renderer.accepts.contains("writes")) {
                warnings.push(format!("{} takes no writes: {name}'s frames are cut from its take, and it reads what it started with", package.name));
            }
            warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
            let mut version = self.version(doc, standing, i, warnings, inputs.clone());
            if seeded {
                // A failed run's seed is kept for the next try on these inputs, so finished segments aren't paid twice.
                let tried = now.asset(&name).and_then(|held| held.tried.as_ref()).filter(|tried| !new && tried.inputs == inputs).map(|tried| tried.seed);
                version.seed = Some(tried.unwrap_or_else(new_seed));
            }
            version.kept = keep.and_then(|keep| Some(Kept { take: keep["take"].as_u64()? as u32, until_s: keep["at_s"].as_f64().or(keep["ends_s"].as_f64())? }));
            // Its number now, so the first of several sent is the lowest, whichever lands first.
            let number = std::cell::Cell::new(0);
            self.folder().update_lock(|lock| number.set(lock.number(&name, &asset.kind)))?;
            version.number = number.get();
            let writes = asset.writes.iter().map(|write| Written { name: write.name.clone(), at_s: write.at_s }).collect();
            let mut calls = calls.into_iter();
            let first = calls.next().expect("at least one");
            let seed = version.seed;
            let pending = Pending { ask: ask.id.clone(), asset: name.clone(), version, run: 0, approve, writes, target: target.clone(), rest: calls.collect(), done: vec![] };
            let settling = |lock: &mut Lock| {
                settle(lock);
                if let Some(seed) = seed {
                    lock.asset_mut(&name, &asset.kind).tried = Some(Tried { inputs: inputs.clone(), seed });
                }
            };
            // Canceled while its adapter ran: not sent.
            let now = self.folder().lock()?;
            let mine = now.walk.asks.iter().find(|mine| mine.id == ask.id);
            let stands = mine.is_some_and(|mine| match &next.take {
                Some(take) => mine.takes.iter().any(|still| still.key == take.key),
                None => !mine.sent.contains(&name_key(&name)),
            });
            if !stands {
                return Ok(());
            }
            self.submit(&key, first, &package, asset, pending, &settling).await
        };
        let sent = sent.await;
        if sent.is_err() {
            self.folder().update_lock(settle)?;
        }
        sent
    }

    /// Where a run going `on` can read `file` from.
    async fn reach(&self, file: &folder::File, on: &[On]) -> Reach {
        if !file.stays() || self.folder().path(&file.path).is_file() {
            return Reach::Here;
        }
        if let Some(machine) = self.host.holding(&file.sha256, file.length, on).await {
            return Reach::There(machine);
        }
        match self.host.fetchable(&file.sha256, &file.machine) {
            true => Reach::Fetch,
            false => Reach::Gone,
        }
    }

    /// A version of asset `i` as it stands now, its files not yet known.
    fn version(&self, doc: &Doc, standing: &Standing, i: usize, warnings: Vec<String>, inputs: String) -> Version {
        let asset = &doc.assets[i];
        let reads = standing.reads.iter().map(|read| ReadFrom { name: read.name.clone(), files: read.files.clone() }).collect();
        let body = |field: &str| asset.body.get(field).cloned().unwrap_or(Value::Null);
        let options = asset.body.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
        let made_by = MadeBy { with: doc.target(i), models: body("models"), lora: body("lora"), options };
        Version { inputs, digests: standing.digests.clone(), reads, made_by: Some(made_by), warnings, ..Default::default() }
    }

    /// An adapter's no run: this asset's files are what it reads.
    #[allow(clippy::too_many_arguments)]
    fn land_read(&self, name: &str, kind: &str, mut version: Version, standing: &Standing, noun: &str, approve: bool, settle: impl Fn(&mut Lock)) -> Result<(), Error> {
        let Some(read) = standing.reads.first() else {
            return Err(cannot(name, "its function answered no run, and it reads nothing".into()));
        };
        version.files = read.files.clone();
        self.record(None, name, kind, version, noun, approve, settle)
    }

    /// A package's project adapter on `operation`, an ordinary run on the machine the render goes to: the calls it answers, in order,
    /// and what it said. None is no run. It holds no files: a file is its digest.
    async fn adapt(&self, adapter: &Renderer, operation: &Value, hub: &str, on: &[On]) -> Result<(Vec<Invocation>, Vec<String>), Error> {
        let failed = |why: String| Error::new(Kind::Unavailable, "project_adapter", format!("{}'s adapter for {}: {why}", adapter.package, operation["kind"].as_str().unwrap_or_default()));
        let placed = self.host.place(on, adapter);
        let (source, inputs) = match &adapter.local {
            None => (v1::Source::Release(v1::Release { package: adapter.package.clone(), release: adapter.release.clone() }), vec![]),
            Some(path) => {
                let prepared = self.host.ready(path, &placed, hub).await?;
                (v1::Source::Local(prepared.source), prepared.files)
            }
        };
        let spec = v1::RunSpec {
            kind: v1::RunKind::Call,
            source: Some(source),
            entrypoint: adapter.function.clone(),
            payload: Some(v1::Json::parse(operation.to_string()).expect("an operation is JSON")),
            ..Default::default()
        };
        let out = Some(landing(&self.folder().dir));
        let submit = SubmitRun { on: placed, hub: hub.to_owned(), kind: RunKind::Call, target: adapter.target(), spec, inputs, held: vec![], out, warnings: vec![], agreed: None };
        let run = self.send_run(&new_key(), submit).await?;
        let run = self.host.wait(&run.id).await?;
        if run.status != RunStatus::Completed {
            return Err(failed(run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |r| r.message.clone())));
        }
        Ok(answered(run.result.as_ref()))
    }

    /// Sends `call` to its function of `package` on one of the pending's machines, as an ordinary run under `key`, recorded as pending
    /// (with `settle`: what it settles of the walk) in one write before it is sent. The seed, and on the first call the asset's
    /// `models` and `lora`, as `cozy run --input` takes them, go on top. Its `options` are the adapter's, not added here.
    async fn submit(&self, key: &str, call: Invocation, package: &Package, asset: &format::Asset, mut pending: Pending, settle: &impl Fn(&mut Lock)) -> Result<(), Error> {
        let renderer = package.function(&call.function)?;
        let (on, hub) = (pending.target.on.clone(), pending.target.hub.clone());
        let first = pending.done.is_empty();
        let name = asset.name.clone();
        let named = |e: Error| Error { message: format!("{name}'s render: {}", e.message), ..e };
        pending.version.invocations.push(json!({"function": call.function, "input": call.input}));
        let mut payload = call.input;
        if let Some(seed) = pending.version.seed.filter(|_| renderer.accepts.contains("seed")) {
            payload.insert("seed".into(), json!(seed));
        }
        for field in ["models", "lora"] {
            if let Some(value) = asset.body.get(field).filter(|value| first && !value.is_null()) {
                payload.insert(field.into(), value.clone());
            }
        }
        let called = renderer.target();
        let mut choices = Choices::new(&called, &renderer.function, &renderer.slots, renderer.calls.iter().collect());
        choices.take(&mut payload).map_err(named)?;
        if first {
            pending.version.made_by = Some(MadeBy { with: renderer.target(), ..pending.version.made_by.clone().unwrap_or_default() });
        }
        let mut warnings = match choices.dropped.is_empty() {
            true => vec![],
            false => vec![Warning {
                code: "request_fields_ignored".into(),
                message: format!("{name}'s render: ignored {}", choices.dropped.join(", ")),
            }],
        };
        warnings.extend(choices.replaced.iter().map(|replaced| Warning {
            code: "model_slot_replaced".into(),
            message: format!("{name}'s render: {replaced}"),
        }));
        let models = choices::wire(&choices.into_choices()).map_err(named)?;
        let payload = Value::Object(payload);
        let lock = self.folder().lock()?;
        let (mut inputs, mut staged, mut held, mut holder) = (vec![], vec![], vec![], None);
        for (field, path, digest) in tools::files_in(&payload, self.folder(), &lock) {
            let file = &lock.files[&digest];
            // What stays on a machine: read there by its hash, else fetched now that a run needs it.
            let mut on_machine = false;
            match self.reach(file, holder.as_ref().map_or(on.as_slice(), std::slice::from_ref)).await {
                Reach::Here => {}
                Reach::There(machine) => {
                    self.say(Event::Said { message: format!("{name}: {field} is read on {machine}, which holds it") });
                    holder = Some(machine);
                    on_machine = true;
                }
                Reach::Fetch => {
                    self.say(Event::Said { message: format!("{name}: fetching {field} from {}", file.machine) });
                    let fetched = self.host.fetch(&digest, &file.machine).await;
                    fetched.map_err(|why| cannot(&name, format!("{field} is on {}, and can't be had: {why}", file.machine)))?;
                }
                Reach::Gone => return Err(cannot(&name, format!("{field} went with its machine {}", file.machine))),
            }
            let length = match on_machine {
                true => file.length,
                false => {
                    std::fs::metadata(&path)
                        .map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())).next("cozy project pull"))?
                        .len()
                }
            };
            let order = field.split('.').filter_map(|part| part.parse().ok()).next_back().unwrap_or(0);
            let media_type = media_type(&path).to_owned();
            inputs.push(v1::InputFile { field, digest: digest.clone(), length, media_type, order });
            let digest: Digest = digest.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?;
            match on_machine {
                true => held.push(digest),
                false => staged.push(StagedInput { path, digest, length }),
            }
        }
        let placed = holder.unwrap_or_else(|| self.host.place(&on, &renderer));
        // Local code is made ready on the machine the call goes to, its files and manifest sent
        // with every call: a machine that dropped them installs them in the call.
        let source = match &renderer.local {
            None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() }),
            Some(path) => {
                let prepared = self.host.ready(path, &placed, &hub).await.map_err(named)?;
                staged.extend(prepared.files);
                v1::Source::Local(prepared.source)
            }
        };
        let spec = v1::RunSpec {
            kind: v1::RunKind::Call,
            source: Some(source),
            entrypoint: renderer.function.clone(),
            payload: Some(v1::Json::parse(payload.to_string()).expect("a payload is JSON")),
            inputs,
            models,
            ..Default::default()
        };
        let submit = SubmitRun {
            on: placed,
            hub,
            kind: RunKind::Call,
            target: renderer.target(),
            spec,
            inputs: staged,
            held,
            out: Some(landing(&self.folder().dir)),
            warnings,
            agreed: None,
        };
        self.folder().update_lock(|lock| {
            lock.pending.insert(key.into(), pending.clone());
            settle(lock);
        })?;
        let run = match self.send_run(key, submit).await {
            Ok(run) => run,
            Err(error) => {
                self.folder().update_lock(|lock| {
                    lock.pending.remove(key);
                })?;
                return Err(error);
            }
        };
        self.folder().update_lock(|lock| {
            if let Some(pending) = lock.pending.get_mut(key) {
                pending.run = run.number;
            }
        })?;
        self.say(Event::Started { name, run: run.id.to_string(), number: run.number });
        Ok(())
    }

    /// Follows the run sent under `key` to its end and lands it, its pending entry gone however it ends (a lock that cannot be
    /// written ends the walk).
    async fn follow(&self, key: String) -> (String, Result<Result<(), Error>, Error>) {
        let landed = async {
            let Some(run) = self.host.sent(&key)? else { return Ok(()) };
            let run = self.host.wait(&run.id).await?;
            self.land(&key, &run).await
        };
        let landed = landed.await;
        if let Err(error) = self.folder().update_lock(|lock| drop(lock.pending.remove(&key))) {
            return (key, Err(error));
        }
        (key, Ok(landed))
    }

    /// The run sent under `key`, once it ended, into the project: its outputs kept as files, and listed as its asset's newest
    /// version in project.lock. A run that did not complete is no version, with its reason, and is never run again this walk.
    async fn land(&self, key: &str, run: &Run) -> Result<(), Error> {
        let folder = self.folder();
        let Some(pending) = folder.lock()?.pending.remove(key) else { return Ok(()) };
        let name = pending.asset.clone();
        // What this run landed is its version's, not a loose file of the project's.
        folder.update_lock(|lock| lock.runs.retain(|row| row.run != run.number))?;
        if run.status != RunStatus::Completed {
            let status = format!("{:?}", run.status).to_lowercase();
            let said = match &run.reason {
                Some(reason) => format!("{name}: {status}: {}", reason.message),
                None => format!("{name}: {status}"),
            };
            return Err(Error::new(Kind::Unavailable, "version_failed", said).next(format!("cozy run show {}", run.number)));
        }
        let mut version = pending.version.clone();
        // Where its function kept the other take up to, exactly: the time asked lands on its own grid.
        if let (Some(kept), Some(until_s)) = (&mut version.kept, run.result.as_ref().and_then(|result| result["kept_s"].as_f64())) {
            kept.until_s = until_s;
        }
        version.runs.push(run.number);
        version.execution_ms += run.execution_ms.unwrap_or(0);
        for said in said_by(run) {
            let message = format!("{name}: {said}");
            self.say(Event::Warning { message: message.clone() });
            if !version.warnings.contains(&message) {
                version.warnings.push(message);
            }
        }
        // Every file the run saved, by its output's name (a list's items as `<name>-<n>`). `written` is the frames it was asked for, in order.
        let (mut saved, mut stayed) = (Files::new(), Files::new());
        for output in &run.output {
            // The first of a list goes by its name, so a reader that takes one takes it.
            let file = match output.index {
                0 | 1 => output.name.clone(),
                n => format!("{}-{n}", output.name),
            };
            // What stays on its machine is listed with who holds it, and no file.
            if output.stays && output.path.as_ref().is_none_or(|path| !path.is_file()) {
                if let (Some(sha256), Some(machine)) = (&output.sha256, &run.machine) {
                    folder.stays(&sha256.to_string(), machine, output.length)?;
                    stayed.insert(file, sha256.to_string());
                }
                continue;
            }
            let Some(path) = output.path.as_ref().filter(|path| path.is_file()) else { continue };
            if output.name == "written" {
                if let Some(write) = pending.writes.get(output.index as usize) {
                    version.writes.insert(name_key(&write.name), Files::from([("image".to_owned(), folder.keep(path)?)]));
                }
                continue;
            }
            saved.insert(file, folder.keep(path)?);
        }
        if saved.is_empty() && pending.rest.is_empty() && version.files.is_empty() {
            let said = format!("{name}: run {} saved no file", run.number);
            return Err(Error::new(Kind::Unavailable, "version_unsaved", said).next(format!("cozy run show {}", run.number)));
        }
        saved.append(&mut stayed);
        version.files.extend(saved.clone());
        // Its next call, if the adapter answered several: one version, whichever call makes what.
        if !pending.rest.is_empty() {
            let doc = self.doc_of(&pending.ask)?;
            let Some(i) = doc.find(&name) else {
                return Err(cannot(&name, format!("it is no longer in {}", folder::PROJECT)));
            };
            let mut rest = pending.rest.clone();
            let mut next = rest.remove(0);
            let mut done = pending.done.clone();
            done.push(saved);
            next.input = earlier(next.input, &done).map_err(|why| cannot(&name, why))?;
            let package = self.host.package(&pending.target.hub, &doc.target(i), &folder.dir, &pending.target.on).await?;
            let carried = Pending { version, rest, done, run: 0, ..pending.clone() };
            let settle = |lock: &mut Lock| drop(lock.pending.remove(key));
            return self.submit(&new_key(), next, &package, &doc.assets[i], carried, &settle).await;
        }
        for write in &pending.writes {
            if !version.writes.contains_key(&name_key(&write.name)) {
                let frame = self.cut_write(&name, &version, write).await?;
                version.writes.insert(name_key(&write.name), Files::from([("image".to_owned(), frame)]));
            }
        }
        let kind = folder.lock()?.asset(&name).map(|held| held.kind.clone()).unwrap_or_default();
        let noun = if version.files.contains_key("video") { "take" } else { "version" };
        let key = key.to_owned();
        self.record(Some(&key), &name, &kind, version, noun, pending.approve, |_: &mut Lock| {})
    }

    /// An image its function didn't write, cut from its landed video.
    async fn cut_write(&self, name: &str, version: &Version, write: &Written) -> Result<String, Error> {
        let lock = self.folder().lock()?;
        let video = version.files.get("video").and_then(|sha256| lock.files.get(sha256)).map(|file| self.folder().path(&file.path));
        let Some(video) = video else {
            return Err(cannot(name, format!("it saved no video to take {} from", write.name)));
        };
        self.frame(name, &video, Some(write.at_s)).await
    }

    /// The frame of `video` at `at` seconds, half a frame early so a write at its end is its last frame, kept as a file of the
    /// project. One past its end is its last, which is said.
    async fn frame(&self, name: &str, video: &FsPath, at: Option<f64>) -> Result<String, Error> {
        let (ffmpeg, _) = self.host.ffmpeg().await?;
        let (dir, path) = (self.folder().dir.clone(), video.to_owned());
        let early = at.map(|at| (at - 1.0 / 48.0).max(0.0));
        let grabbed = tokio::task::spawn_blocking(move || tools::frame(&ffmpeg, &path, early, &dir))
            .await
            .map_err(|e| Error::new(Kind::Internal, "frame", e.to_string()))?;
        let (frame, past) = grabbed?;
        if let (true, Some(at)) = (past, at) {
            self.say(Event::Warning { message: format!("{name}: its video ends before {at:.2} s: its last frame stands in") });
        }
        self.folder().keep(frame.path())
    }

    /// Makes tool `i` here, from what it reads now, and lists what it made. A tool keeps one version, free to make again, and its pinned one.
    async fn tool(&self, ask: &Ask, doc: &Doc, stand: &[Standing], lock: &Lock, i: usize) -> Result<(), Error> {
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        let standing = &stand[i];
        let Some(inputs) = standing.inputs.clone() else { return Ok(()) };
        for read in &standing.reads {
            if let Some(number) = read.newest {
                let noun = noun(doc, lock, read.asset);
                self.say(Event::Warning { message: format!("{name} reads {}'s newest {noun}, {number}, which is out of date", read.name) });
            }
        }
        let mut version = self.version(doc, standing, i, vec![], inputs);
        version.made_by = Some(MadeBy { with: match doc.tool(i) {
            Some(Tool::Cut) => "cozy/cut".into(),
            _ => "cozy/frame".into(),
        }, ..Default::default() });
        match doc.tool(i).expect("a tool") {
            Tool::Cut => {
                // Once a walk: what it cuts stands, or says why not.
                self.tried(&ask.id, &name)?;
                let Some(files) = self.cut_tool(asset, standing, &mut version).await? else { return Ok(()) };
                version.files = files;
            }
            Tool::Frame => {
                let Some(read) = standing.reads.first() else { return Ok(()) };
                let file = folder::main_of(&read.files).and_then(|sha256| lock.files.get(sha256));
                let Some(file) = file else { return Err(cannot(&name, format!("{} has no file", read.name))) };
                let at = asset.fields.get("at").and_then(Value::as_f64);
                let frame = self.frame(&name, &self.folder().path(&file.path), at).await?;
                version.files = Files::from([("image".to_owned(), frame)]);
            }
        }
        let number = std::cell::Cell::new(0);
        self.folder().update_lock(|lock| number.set(lock.number(&name, &asset.kind)))?;
        version.number = number.get();
        // One cut missing a file here matches no key, so the next walk makes it again; this one doesn't.
        if version.inputs.is_empty() {
            self.tried(&ask.id, &name)?;
        }
        self.record_free(doc, &name, &asset.kind, version, noun(doc, lock, i))
    }

    /// `name` is tried this walk: it is not tried again until the next.
    fn tried(&self, ask: &str, name: &str) -> Result<(), Error> {
        let key = name_key(name);
        self.folder().update_lock(|lock| {
            for mine in lock.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
                mine.sent.insert(key.clone());
            }
        })
    }

    /// Cuts `asset`'s pieces, its film and timeline at the root. None: nothing to cut. The film stays at the root however its
    /// version goes; a cut missing a file here is recorded under no key, so it is made again. A piece here that it can't read
    /// fails it: nothing is cut, and no film stands short of a piece.
    async fn cut_tool(&self, asset: &format::Asset, standing: &Standing, version: &mut Version) -> Result<Option<Files>, Error> {
        let folder = self.folder();
        let mut said = vec![];
        let canvas = Canvas::of(&asset.fields, &mut said);
        // ffprobe first, as this may be its first need here, then every file measured.
        let (ffmpeg, ffprobe) = self.host.ffmpeg().await?;
        let mut lock = folder.lock()?;
        folder.measure(&mut lock)?;
        let (mut pieces, skips) = tools::pieces(&asset.fields, &standing.reads, folder, &lock);
        let name = cut_name(&asset.name);
        if !skips.is_empty() {
            self.say(Event::Said { message: format!("{name} skips {}: no version yet", skips.join(", ")) });
        }
        let absent = pieces.iter().any(|piece| !piece.path.is_file());
        said.extend(tools::absent(&mut pieces));
        let unreadable = tools::unreadable(&ffprobe, &pieces);
        if !unreadable.is_empty() {
            return Err(Error::new(Kind::Validation, "piece_unreadable", format!("{} cannot render: {}", asset.name, unreadable.join("; "))));
        }
        said.extend(tools::past_end(&mut pieces, canvas.fps));
        said.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
        if pieces.is_empty() {
            return Ok(None);
        }
        let (project, cutting, began) = (self.project.clone(), self.project.cutting.clone(), Instant::now());
        let cache = self.host.cache().join("cuts");
        let cutting_name = name.clone();
        let fps = canvas.fps;
        let (made, part) = tokio::task::spawn_blocking(move || {
            let _held = cutting.lock().expect("cutting lock");
            tools::cut(&ffmpeg, &ffprobe, &project.folder, &pieces, &canvas, &cutting_name, &cache)
        })
        .await
        .map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))??;
        let before = folder.lock()?.asset(&asset.name).and_then(|held| held.versions.last()?.files.get("video").cloned());
        let sha256 = folder.take(&part)?;
        let path = folder.lock()?.files.get(&sha256).map(|file| folder.path(&file.path));
        if let Some(path) = path {
            tools::publish(&path, &folder.dir.join(&name)).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{name}: {e}")))?;
        }
        if absent {
            version.inputs.clear();
        }
        let (frames, retimed, took_s) = (made.frames, made.retimed, began.elapsed().as_secs_f64());
        self.cut.lock().expect("cut lock").insert(name.clone());
        self.say(Event::Cut { cut: name, frames, fps, retimed, took_s, unchanged: before.as_ref() == Some(&sha256) });
        Ok(Some(Files::from([("video".to_owned(), sha256)])))
    }

    /// Lists a tool's version, its older ones gone unless pinned, and whatever files only they named.
    fn record_free(&self, doc: &Doc, name: &str, kind: &str, version: Version, noun: &str) -> Result<(), Error> {
        let number = version.number;
        let old = std::cell::RefCell::new(vec![]);
        // What is pinned now, as a save may have moved it, else as this render read it.
        let now = Doc::load(self.folder());
        let doc = now.as_ref().unwrap_or(doc);
        let pin = doc.find(name).and_then(|i| Some(doc.assets[i].pin.as_ref()?.1.clone()));
        self.folder().update_lock(|lock| {
            let held = lock.asset_mut(name, kind);
            let (keep, going): (Vec<Version>, Vec<Version>) = held.versions.drain(..).partition(|version| version.main() == pin.as_ref());
            held.versions = keep;
            held.versions.push(version.clone());
            held.versions.sort_by_key(|version| version.number);
            *old.borrow_mut() = going.iter().flat_map(|version| version.files.values().cloned()).collect();
        })?;
        self.folder().drop_unused(&old.into_inner())?;
        self.say_landed(name, &version, noun, number)
    }

    /// Lists `version` as its asset's newest (dropping the pending run under `key` in the same write), pinned when `approve`, and
    /// says it landed.
    #[allow(clippy::too_many_arguments)]
    fn record(&self, key: Option<&str>, name: &str, kind: &str, mut version: Version, noun: &str, approve: bool, settle: impl Fn(&mut Lock)) -> Result<(), Error> {
        let number = std::cell::Cell::new(version.number);
        self.folder().update_lock(|lock| {
            if let Some(key) = key {
                lock.pending.remove(key);
            }
            settle(lock);
            let given = if version.number == 0 { lock.number(name, kind) } else { version.number };
            number.set(given);
            let held = lock.asset_mut(name, kind);
            let mut listed = version.clone();
            listed.number = given;
            held.versions.retain(|version| version.number != given);
            held.versions.push(listed);
            held.versions.sort_by_key(|version| version.number);
            held.tried = None;
        })?;
        version.number = number.get();
        if approve
            && let Some(sha256) = version.main()
            && let Err(error) = pin_in_file(self.folder(), name, kind, sha256)
        {
            self.say(Event::Warning { message: error.message });
        }
        {
            let mut tally = self.tally.lock().expect("tally lock");
            match noun {
                "take" => tally.takes += 1,
                _ => tally.versions += 1,
            }
            tally.execution_ms += version.execution_ms;
        }
        self.say_landed(name, &version, noun, number.get())
    }

    fn say_landed(&self, name: &str, version: &Version, noun: &str, number: u32) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        let path = main_path(&lock, &version.files).unwrap_or_default();
        let execution_ms = (version.execution_ms > 0).then_some(version.execution_ms);
        self.say(Event::Landed { name: name.into(), noun: noun.into(), number, files: listed(&lock, &version.files), path, execution_ms });
        if let Some(kept) = version.kept {
            self.say(Event::Said { message: kept.said(name, number) });
        }
        Ok(())
    }
}

/// What an adapter answered: its `invocations`, in order, and what it said of them. None is no run.
fn answered(result: Option<&Value>) -> (Vec<Invocation>, Vec<String>) {
    let calls = result.and_then(|result| result["invocations"].as_array()).map(Vec::as_slice).unwrap_or_default();
    let said = calls.iter().flat_map(|call| call["warnings"].as_array().into_iter().flatten()).filter_map(|said| said.as_str().map(str::to_owned)).collect();
    let calls = calls.iter().filter_map(|call| {
        Some(Invocation { function: call["function"].as_str()?.to_owned(), input: call["input"].as_object().cloned().unwrap_or_default() })
    });
    (calls.collect(), said)
}

/// `input` with each string that is exactly `$0.video` the digest of that field of that call's result. Err: which it can't.
fn earlier(input: Map<String, Value>, done: &[Files]) -> Result<Map<String, Value>, String> {
    fn walk(value: Value, done: &[Files]) -> Result<Value, String> {
        Ok(match value {
            Value::String(text) => match text.strip_prefix('$').and_then(|rest| rest.split_once('.')).and_then(|(n, field)| Some((n.parse::<usize>().ok()?, field))) {
                Some((n, field)) => json!(done.get(n).and_then(|files| files.get(field)).ok_or_else(|| format!("{text} is no file an earlier call saved"))?),
                None => Value::String(text),
            },
            Value::Object(fields) => Value::Object(fields.into_iter().map(|(key, value)| Ok((key, walk(value, done)?))).collect::<Result<_, String>>()?),
            Value::Array(items) => Value::Array(items.into_iter().map(|item| walk(item, done)).collect::<Result<_, _>>()?),
            other => other,
        })
    }
    input.into_iter().map(|(key, value)| Ok((key, walk(value, done)?))).collect()
}

/// `{name} cannot render: {why}`.
fn cannot(name: &str, why: String) -> Error {
    Error::new(Kind::NotFound, "file_missing", format!("{name} cannot render: {why}"))
}

/// What can be done now.
struct Plan {
    send: Vec<Next>,
    /// Tools whose reads have settled, with no version for what they read.
    tools: Vec<usize>,
    /// Something it covers is still on its way.
    open: bool,
}

/// What a walk covers: everything, for `all`; each take; and through each with no state or with a take, or a tool, what it reads.
fn covered(doc: &Doc, stand: &[Standing], ask: &Ask) -> Vec<bool> {
    let key = |i: usize| name_key(&doc.assets[i].name);
    let taken = |i: usize| ask.takes.iter().any(|take| name_key(&take.asset) == key(i));
    let mut covered = vec![ask.all; doc.assets.len()];
    for &i in doc.order.iter().rev() {
        covered[i] |= taken(i);
        let tool = doc.tool(i).is_some() && doc.assets[i].pin.is_none();
        if covered[i] && (stand[i].files.is_none() || taken(i) || tool) {
            for &read in &doc.upstream[i] {
                covered[read] = true;
            }
        }
    }
    covered
}

/// What can be sent now, in file order: what the walk covers with no state, and each take, once all it reads is settled and has a
/// state. What a walk covers: everything, for `all`; each take; and through each with no state or with a take, what it reads. A
/// tool has no state until made, whether asked or not, once what it reads has settled.
fn plan(doc: &Doc, stand: &[Standing], lock: &Lock, ask: &Ask) -> Plan {
    let key = |i: usize| name_key(&doc.assets[i].name);
    let takes = |i: usize| ask.takes.iter().filter(move |take| name_key(&take.asset) == key(i));
    let covered = covered(doc, stand, ask);
    let mut unsettled = vec![false; doc.assets.len()];
    let (mut send, mut tools) = (vec![], vec![]);
    for &i in &doc.order {
        let pinned = doc.assets[i].pin.is_some();
        let taken = takes(i).next().is_some();
        // A pin needs nothing it reads, unless a take of it does.
        let reads: &[usize] = if pinned && !taken { &[] } else { &doc.upstream[i] };
        let busy = lock.pending.values().any(|pending| name_key(&pending.asset) == key(i) && (pending.approve || Some(&pending.version.inputs) == stand[i].inputs.as_ref()));
        let waiting = reads.iter().any(|&read| unsettled[read]);
        let tool = doc.tool(i).is_some();
        // A tool reads what stands, or the newest: it is held up only by what is on its way.
        let blocked = !tool && reads.iter().any(|&read| !unsettled[read] && stand[read].files.is_none());
        let tried = ask.sent.contains(&key(i));
        let owed = covered[i] && stand[i].files.is_none() && doc.makes(i) && !tool && !tried;
        let approving = takes(i).any(|take| take.approve);
        unsettled[i] = waiting || approving || (!pinned && (busy || taken)) || (owed && !blocked);
        if waiting || blocked {
            continue;
        }
        if tool {
            // Named, it is made though it has a state.
            if doc.makes(i) && stand[i].inputs.is_some() && (taken || (stand[i].files.is_none() && !tried)) {
                unsettled[i] = true;
                tools.push(i);
            }
            continue;
        }
        if owed && !busy && !taken {
            send.push(Next { i, take: None });
        }
        // Under --missing, what is on its way with these inputs answers it. A plain render makes its own version.
        send.extend(takes(i).filter(|take| take.new || !busy).map(|take| Next { i, take: Some(take.clone()) }));
    }
    Plan { send, tools, open: unsettled.contains(&true) }
}

/// What a render owes that can't be made, and why: what it reads has no state and won't get one in this render.
fn stuck(doc: &Doc, stand: &[Standing], lock: &Lock, ask: &Ask) -> Vec<String> {
    let takes = ask.takes.iter().filter_map(|take| doc.find(&take.asset));
    let all = (0..doc.assets.len()).filter(|&i| ask.all && doc.makes(i) && doc.tool(i).is_none() && stand[i].files.is_none());
    let mut said: Vec<usize> = takes.chain(all).filter(|&i| !ask.sent.contains(&name_key(&doc.assets[i].name))).collect();
    said.sort_unstable();
    said.dedup();
    said.into_iter().map(|i| format!("{} cannot render: {}", doc.assets[i].name, unread(doc, stand, lock, ask, i))).collect()
}

/// Why asset `i` can't read what it reads: the first read with no state, through an entity to the asset it links.
fn unread(doc: &Doc, stand: &[Standing], lock: &Lock, ask: &Ask, i: usize) -> String {
    let Some(&j) = doc.upstream[i].iter().find(|&&j| stand[j].files.is_none()) else { return "what it reads has no state".into() };
    let name = &doc.assets[j].name;
    if doc.assets[j].links.is_some() {
        return unread(doc, stand, lock, ask, j);
    }
    match &stand[j].state {
        _ if ask.sent.contains(&name_key(name)) => format!("{name} failed in this render"),
        Stand::Unwritten { by } => format!("{name} comes with {by}'s take, and there is none"),
        Stand::Stale(why) => format!("{name} is out of date: {why}"),
        _ if doc.assets[j].hollow => format!("{name} has nothing to make"),
        _ if lock.asset(name).is_some_and(|held| !held.versions.is_empty()) => format!("{name} has no version with these inputs"),
        _ => format!("{name} has no version"),
    }
}

/// A fresh idempotency key: one key names one run forever.
fn new_key() -> String {
    let mut key = [0u8; 16];
    getrandom::fill(&mut key).expect("the OS has randomness");
    format!("project-{}", hex::encode(key))
}

/// A fresh seed, recorded on its version.
fn new_seed() -> u64 {
    let mut seed = [0u8; 4];
    getrandom::fill(&mut seed).expect("the OS has randomness");
    u64::from(u32::from_le_bytes(seed) >> 1)
}

/// What a run's function said of its result (its `warnings`), and its machine's.
fn said_by(run: &Run) -> Vec<String> {
    let result = run.result.as_ref().and_then(|result| result["warnings"].as_array()).into_iter().flatten();
    let result = result.filter_map(|said| said.as_str().map(str::to_owned));
    run.warnings.iter().map(|warning| warning.message.clone()).chain(result).collect()
}

fn find(doc: &Doc, name: &str) -> Result<usize, Error> {
    if let Some(links) = doc.find(name).and_then(|i| doc.assets[i].links.as_ref()) {
        let linked: Vec<&str> = links.own.values().chain(links.roles.get("")).map(String::as_str).collect();
        let said = format!("{name} is an entity, with no files of its own: it links {}", linked.join(", "));
        return Err(Error::new(Kind::Usage, "asset_is_entity", said).remedy("name an asset it links"));
    }
    doc.find(name).ok_or_else(|| {
        Error::new(Kind::NotFound, "asset_unknown", format!("the project has nothing named {name}")).next("cozy project status")
    })
}

fn media_type(path: &FsPath) -> &'static str {
    let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "otio" => "application/json",
        "yaml" => "text/yaml; charset=utf-8",
        _ => "application/octet-stream",
    }
}
