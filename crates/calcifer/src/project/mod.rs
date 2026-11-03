//! Projects (xs-136, xs-139, cl-716): a folder of versioned nodes that packages render. Calcifer
//! opens a project by its path (and keeps every folder it opened in its index) and walks it: each
//! version is an ordinary run, sent through its own submit as soon as what it reads is settled, so
//! independent ones run at once and the machines' schedulers place them. Each lands in its node's
//! folder (`assets/`, `folder`) and project.lock (people write project.yaml; cozy never
//! does), and the cuts are made here. One walker
//! per project takes every render asked of it, and what it owes is kept in project.lock, so a
//! restarted Calcifer goes on with it. Its clients ask and watch; the runs go on without them.
mod folder;
mod graph;
pub(crate) mod host;
mod remote;
mod video;

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
    http::{HeaderMap, StatusCode},
    response::{
        Response,
        sse::{self, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::{Stream, StreamExt as _, stream::FuturesUnordered};
use protocol::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status as RunStatus, SubmitRun, Warning, local_usable},
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    project::{
        Approve, Approved, Asset as Listing, Clean, Cut, Event, Import, Imported, Known, Node as Row, NodeVersions, Open, Opened,
        Orphan, Projects as Listed, Rename, Renamed, Rendered, Runs, Stale, State as ProjectState, Version, VersionPrefix,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast};
use tracing::warn;

use crate::api::{Api, Refusal, parse};
use folder::{Call, Folder, Lock, Made, MadeBy, Pending, Take, Target, Then};
use graph::{Status, name_key};
use host::{Host, Renderer};
use video::{ALL, Canvas, Render, Request, Video};

/// A picture with at least this share of fully transparent pixels needs no clean (cl-726's sheets are 59-73%).
const TRANSPARENT: f64 = 0.05;

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
    /// project.yaml as the walk last read it, and whether it has said it can't read a later save.
    read: Mutex<Option<(String, bool)>>,
}

pub fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/projects", post(open).get(list))
        .route("/v1/projects/{id}", get(state).delete(forget))
        .route("/v1/projects/{id}/imports", post(import))
        .route("/v1/projects/{id}/runs", post(runs))
        .route("/v1/projects/{id}/approve", post(approve))
        .route("/v1/projects/{id}/rename", post(rename))
        .route("/v1/projects/{id}/push", post(remote::pushed))
        .route("/v1/projects/{id}/pull", post(remote::pulled))
        .route("/v1/projects/{id}/events", get(events))
        .route("/v1/projects/{id}/nodes/{name}", get(node))
        .route("/v1/projects/{id}/assets/{asset}", get(asset))
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
        let id = project_id(dir);
        if !self.open.lock().expect("projects lock").contains_key(&id)
            && let Err(error) = folder.real_files()
        {
            warn!("{}: {}", dir.display(), error.message);
        }
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
                    read: Mutex::default(),
                })
            })
            .clone();
        if project.folder.lock()?.owes() {
            walk(api, &project);
        }
        Ok(id)
    }
}

/// One folder's id, the same for as long as it is there.
fn project_id(dir: &FsPath) -> String {
    format!("project-{}", &hex::encode(Sha256::digest(dir.to_string_lossy().as_bytes()))[..16])
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
            platform::durable::replace(&project, video::skeleton().as_bytes(), 0o644)
                .map_err(|e| io(std::io::Error::other(e)))?;
            std::fs::canonicalize(&open.path).map_err(io)?
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

/// Its nodes as they stand.
async fn state(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<ProjectState>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let video = Video::load(&project.folder).map_err(Refusal)?;
    let lock = project.folder.lock().map_err(Refusal)?;
    let mut warnings = video.warnings.clone();
    for node in &video.graph.nodes {
        let made = node.file.as_ref().and_then(|(path, _)| lock.made(&node.id, path));
        warnings.extend(made.into_iter().flat_map(|made| made.warnings.iter().cloned()));
    }
    warnings.dedup();
    let (mut missing, mut stale, status) = (vec![], vec![], video.graph.status(&lock));
    let nodes = (video.graph.order.iter())
        .map(|&i| {
            let (row, standing) = row(&video, &lock, &status, i);
            match standing {
                Standing::Missing => missing.push(row.name.clone()),
                Standing::Stale(why) => stale.push(Stale { name: row.name.clone(), why }),
                Standing::Other => {}
            }
            row
        })
        .collect();
    let mut cuts = vec![];
    for (name, preview) in made_cuts(&video) {
        let path = project.folder.dir.join(name);
        let cut_at_ms = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .map(|at| at.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64));
        let (mut pieces, skips) = match video.cuts(&project.folder, preview) {
            Ok(cut) => cut,
            Err(error) => {
                warnings.push(error.message);
                (vec![], vec![])
            }
        };
        warnings.extend(video::past_end(&mut pieces, video.canvas.fps));
        // What the cut is as project.lock records its takes: frames, and the takes retimed.
        let made = video::measure(&pieces, video.canvas.fps);
        let (frames, retimed) = (made.as_ref().map(|made| made.frames), made.map(|made| made.retimed).unwrap_or_default());
        cuts.push(Cut { cut: name.into(), path, cut_at_ms, frames, retimed, skips });
    }
    warnings.dedup();
    let busy = *project.walking.lock().expect("walking lock");
    let moving = project.moving.try_lock().is_err();
    let absent = lock.assets().keys().filter(|path| !project.folder.dir.join(path).is_file()).count() as u64;
    let orphans = orphans(&video, &lock);
    let (path, fps) = (project.folder.dir.clone(), video.canvas.fps);
    Ok(Json(ProjectState { id, path, fps, nodes, missing, stale, orphans, busy, moving, absent, cuts, warnings }))
}

/// Where a node stands, for the lists under a project's table.
enum Standing {
    /// Nothing made yet that the project needs, or a picture not cleaned yet.
    Missing,
    Stale(String),
    Other,
}

/// Node `i` as `GET /v1/projects/{id}` lists it: its approved version, assets and status in its own
/// noun (`status`: every node's).
fn row(video: &Video, lock: &Lock, status: &[Status], i: usize) -> (Row, Standing) {
    let (node, status) = (&video.graph.nodes[i], &status[i]);
    let making: Vec<String> = (lock.pending.values())
        .filter(|p| name_key(&p.made.node) == name_key(&node.id) && p.run > 0)
        .map(|p| p.run.to_string())
        .collect();
    let made = node.file.as_ref().and_then(|(path, _)| lock.made(&node.id, path));
    // The version chosen by `approve` or `--approve`, if one was.
    let approved = (lock.node(&node.id).and_then(|n| n.approved.as_ref()))
        .and_then(|path| lock.made(&node.id, path))
        .map(|made| made.number);
    let assets = match (made, &node.file) {
        (Some(made), _) => listed(made),
        (None, Some((path, sha256))) => BTreeMap::from([(video.field(i).to_owned(), Listing { path: path.clone(), sha256: sha256.clone() })]),
        (None, None) => BTreeMap::new(),
    };
    let gone = video.missing(i);
    let (status, standing) = match (making.as_slice(), status) {
        ([run], _) => (format!("making (run {run})"), Standing::Other),
        ([], _) if !gone.is_empty() => (format!("not on this computer: {}: cozy project pull", gone.join(", ")), Standing::Missing),
        (runs @ [_, ..], _) => (format!("making (runs {})", runs.join(", ")), Standing::Other),
        ([], Status::Current) if video.own_unclean(i) => ("picture not cleaned yet".into(), Standing::Missing),
        ([], Status::Current) if node.pinned => ("pinned".into(), Standing::Other),
        ([], Status::Empty) if !video.wanted(i) => {
            ("not needed: no resolution, or its shot meets it".into(), Standing::Other)
        }
        ([], Status::Empty) => (shown(status, node.noun), Standing::Missing),
        ([], Status::Stale(why)) => (shown(status, node.noun), Standing::Stale(why.clone())),
        ([], Status::Current) => (shown(status, node.noun), Standing::Other),
    };
    let row = Row {
        name: node.id.clone(),
        kind: node.kind.into(),
        noun: node.noun.into(),
        versions: node.versions.len() as u32,
        prefixes: (lock.node(&node.id).into_iter().flat_map(|n| &n.versions))
            .filter_map(|made| Some(VersionPrefix { number: made.number, prefix: folder::prefix_of(&made.primary()?.sha256).to_owned() }))
            .collect(),
        approved,
        pinned: node.pinned,
        assets,
        path: node.file.as_ref().map(|(path, _)| path.clone()),
        made_by: made.and_then(|made| made.made_by.as_ref()).map(|made_by| made_by.with.clone()),
        status,
    };
    (row, standing)
}

/// The cuts the project makes, and whether each is the native one: with `resolution:`,
/// `preview.mp4` from the shots and `edit.mp4` from their upscales; without it, `edit.mp4` from the
/// shots.
fn made_cuts(video: &Video) -> Vec<(&'static str, bool)> {
    match video.canvas.short {
        Some(_) => vec![("preview.mp4", true), ("edit.mp4", false)],
        None => vec![("edit.mp4", true)],
    }
}

/// `GET /v1/projects/{id}/nodes/{name}`: the node and each of its versions.
async fn node(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Json<NodeVersions>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let (video, lock) = (Video::load(&project.folder).map_err(Refusal)?, project.folder.lock().map_err(Refusal)?);
    let i = find(&video, &name).map_err(Refusal)?;
    let (node, _) = row(&video, &lock, &video.graph.status(&lock), i);
    let approved = lock.node(&node.name).and_then(|n| n.approved.clone());
    let versions = (lock.node(&node.name).into_iter().flat_map(|n| &n.versions))
        .map(|made| Version {
            number: made.number,
            assets: listed(made),
            made_by: made.made_by.as_ref().map(|by| by.with.clone()),
            runs: made.runs.clone(),
            approved: made.primary().is_some_and(|asset| Some(&asset.path) == approved.as_ref()),
            warnings: made.warnings.clone(),
        })
        .collect();
    Ok(Json(NodeVersions { node, versions }))
}

/// A version's assets as the API lists them: by output name, each one's path and sha256.
fn listed(made: &Made) -> BTreeMap<String, Listing> {
    (made.assets.iter())
        .map(|(name, asset)| (name.clone(), Listing { path: asset.path.clone(), sha256: asset.sha256.clone() }))
        .collect()
}

/// The nodes project.lock keeps versions of that project.yaml no longer names, each with the one new
/// node of its kind with no versions and no file attached, if there is exactly one: what a rename in
/// project.yaml left.
fn orphans(video: &Video, lock: &Lock) -> Vec<Orphan> {
    let named: Vec<String> = video.graph.nodes.iter().map(|node| name_key(&node.id)).collect();
    let empty = |kind: &str| -> Vec<&str> {
        (video.graph.nodes.iter())
            .filter(|node| node.kind == kind && node.file.is_none() && lock.node(&node.id).is_none_or(|n| n.versions.is_empty()))
            .map(|node| node.id.as_str())
            .collect()
    };
    (lock.nodes.iter())
        .filter(|(key, node)| !named.contains(key) && !node.versions.is_empty())
        .map(|(key, node)| Orphan {
            name: node.versions.first().map_or_else(|| key.clone(), |made| made.node.clone()),
            kind: node.kind.clone(),
            versions: node.versions.len() as u32,
            renamed_to: match empty(&node.kind).as_slice() {
                [only] => Some((*only).to_owned()),
                _ => None,
            },
        })
        .collect()
}

/// Keeps what project.lock has of `from` as `to`, a node project.yaml names now of the same kind with
/// no versions: its versions and approval, what other versions read of it, and what is owed of it.
/// Refused while the project renders. Nothing on disk moves.
async fn rename(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Renamed>, Refusal> {
    let asked: Rename = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let refuse = |kind, code, said: String| Refusal(Error::new(kind, code, said));
    if *project.walking.lock().expect("walking lock") {
        return Err(refuse(Kind::Conflict, "project_busy", "the project is rendering: rename once its runs land".into()));
    }
    let (video, lock) = (Video::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let Some(old) = lock.node(&asked.from).filter(|node| !node.versions.is_empty()) else {
        return Err(refuse(Kind::NotFound, "node_unknown", format!("project.lock keeps no version of {}", asked.from)));
    };
    let Some(i) = video.graph.find(&asked.to) else {
        let said = format!("{} names no {}: rename the node (and its <mentions>) there first", folder::PROJECT, asked.to);
        return Err(refuse(Kind::NotFound, "node_unknown", said));
    };
    let new = &video.graph.nodes[i];
    if lock.node(&new.id).is_some_and(|node| !node.versions.is_empty()) {
        let said = format!("{} has {}s of its own already", new.id, new.noun);
        return Err(refuse(Kind::Conflict, "rename_taken", said));
    }
    if !old.kind.is_empty() && old.kind != new.kind {
        let said = format!("{} is a {} and {} a {}: a rename keeps its kind", asked.from, old.kind, new.id, new.kind);
        return Err(refuse(Kind::Usage, "rename_kind", said));
    }
    let (from, to, versions) = (asked.from.clone(), new.id.clone(), old.versions.len() as u32);
    // A rename rewrites the mentions of it: that is no new text, so what reads it stays as it stood.
    let readers: Vec<(String, BTreeMap<String, String>)> = (0..video.graph.nodes.len())
        .filter(|&j| j == i || video.graph.upstream[j].iter().any(|edge| edge.node == i))
        .map(|j| (name_key(&video.graph.nodes[j].id), video.graph.nodes[j].text.clone()))
        .collect();
    folder
        .update_lock(|lock| {
            renamed(lock, &from, &to);
            for (key, text) in &readers {
                let Some(node) = lock.nodes.get_mut(key) else { continue };
                if let Some(at) = node.current_at() {
                    node.versions[at].text = Some(text.clone());
                }
            }
        })
        .map_err(Refusal)?;
    Ok(Json(Renamed { from, to, versions }))
}

/// project.lock with node `from` called `to`: its entry, its versions' names, what other versions
/// read of it (`<from>` and its files `<from>.<field>`), and what is pending or owed of it.
fn renamed(lock: &mut Lock, from: &str, to: &str) {
    let key = name_key(from);
    // `name` with `from` (or `<from>.<field>`) called `to`.
    let called = |name: &str| -> Option<String> {
        if name_key(name) == key {
            return Some(to.to_owned());
        }
        (name.match_indices('.')).find(|(at, _)| name_key(&name[..*at]) == key).map(|(at, _)| format!("{to}{}", &name[at..]))
    };
    if let Some(mut node) = lock.nodes.remove(&key) {
        node.versions.iter_mut().for_each(|made| made.node = to.into());
        lock.nodes.insert(name_key(to), node);
    }
    for made in lock.nodes.values_mut().flat_map(|node| &mut node.versions) {
        for read in made.upstream.iter_mut().flatten() {
            if let Some(name) = called(&read.node) {
                read.node = name;
            }
        }
    }
    for pending in lock.pending.values_mut().filter(|pending| name_key(&pending.made.node) == key) {
        pending.made.node = to.into();
    }
    for take in lock.walk.takes.iter_mut().filter(|take| name_key(&take.node) == key) {
        take.node = to.into();
    }
    if lock.walk.sent.remove(&key) {
        lock.walk.sent.insert(name_key(to));
    }
}

/// Adds what `Runs` asks to the project's walk, answered at once (`202`; `200` when its
/// `Idempotency-Key` asked it already); `/events` says how it goes.
async fn runs(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
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
    Origin::parse(&asked.hub).map_err(Refusal)?;
    let project = api.projects.get(&api, &id)?;
    if let Some(key) = headers.get("idempotency-key").and_then(|key| key.to_str().ok()) {
        let digest = hex::encode(Sha256::digest(&body));
        let mut keys = project.keys.lock().expect("keys lock");
        match keys.get(key) {
            Some(seen) if *seen == digest => {
                return Ok((StatusCode::OK, Json(Opened { id, path: project.folder.dir.clone() })));
            }
            Some(_) => {
                let said = format!("Idempotency-Key {key} asked another render already");
                return Err(Refusal(Error::new(Kind::Conflict, "idempotency_conflict", said)));
            }
            None => drop(keys.insert(key.to_owned(), digest)),
        }
    }
    let video = Video::load(&project.folder).map_err(Refusal)?;
    let target = Target { on: asked.on, hub: asked.hub };
    let all = matches!(asked.names.as_slice(), [only] if name_key(only) == name_key(ALL));
    let lock = project.folder.lock().map_err(Refusal)?;
    let say = |message: String| drop(project.events.send(Event::Warning { message }));
    let status = video.graph.status(&lock);
    let n = asked.n.max(1);
    let (mut takes, mut first, mut first_made, mut only) = (vec![], None, BTreeSet::new(), BTreeSet::new());
    for name in asked.names.iter().filter(|_| !all) {
        // A name that is nothing to render is said; the rest go on.
        let found = find(&video, name).and_then(|i| match video.is_asset(i) {
            true => Err(video::asset_made(&video.graph.nodes[i].id)),
            false => Ok(i),
        });
        let i = match found {
            Ok(i) => i,
            Err(error) => {
                say(error.message.clone());
                first.get_or_insert(error);
                continue;
            }
        };
        let node = video.graph.nodes[i].id.clone();
        let current = status[i] == Status::Current;
        if let Some(field) = video.pinned_by(i) {
            drop(project.events.send(Event::Said { message: format!("{node} is pinned by its {field}:; remove it to generate") }));
            if !video.unclean(i).is_empty() {
                only.extend(video.with_upstream(i));
            }
            continue;
        }
        // Up to date, with what it reads; a current one stays as it is.
        if !asked.new {
            if !current || !video.unclean(i).is_empty() {
                only.extend(video.with_upstream(i));
            } else {
                let (noun, another) = (video.graph.nodes[i].noun, if video.graph.nodes[i].versions.is_empty() { "a" } else { "another" });
                drop(project.events.send(Event::Said { message: format!("{node} is current; --new makes {another} {noun}") }));
            }
            continue;
        }
        let take = |_| Take { key: new_key(), node: node.clone(), target: target.clone(), approve: asked.approve };
        takes.extend((0..n).map(take));
        // What it reads with no version yet is made first, and stands as that node's.
        for j in unmade(&video, &lock, i) {
            let up = video.graph.nodes[j].id.clone();
            if first_made.insert(j) {
                say(format!("{node} reads {up}, which has no version yet: making it first"));
                takes.push(Take { key: new_key(), node: up, target: target.clone(), approve: false });
            }
        }
    }
    if let (Some(error), true, false) = (first, takes.is_empty(), all) {
        return Err(Refusal(error));
    }
    (project.folder.update_lock(|lock| {
        if all {
            lock.walk.all = Some(target.clone());
            lock.walk.preview = asked.preview;
            lock.walk.only.clear();
            lock.walk.new |= asked.new;
            lock.walk.n = lock.walk.n.max(n);
        } else if !only.is_empty() {
            // Every node asked already stays asked.
            if lock.walk.all.is_none() || !lock.walk.only.is_empty() {
                lock.walk.only.extend(only.iter().cloned());
            }
            lock.walk.all = Some(target.clone());
            lock.walk.n = lock.walk.n.max(n);
        }
        lock.walk.takes.extend(takes.iter().cloned());
    }))
    .map_err(Refusal)?;
    walk(&api, &project);
    Ok((StatusCode::ACCEPTED, Json(Opened { id, path: project.folder.dir.clone() })))
}

/// Keeps the file at `source` as an asset of the project, in project.lock with its mark (`Folder::import`).
/// A marked picture with no clean is cleaned now if this computer
/// runs models: the walk follows that run. Otherwise, or if it can't be sent, the first machine to render
/// something that reads it cleans it, as the mark travels with the lock.
async fn import(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Imported>, Refusal> {
    let asked: Import = parse(&body?)?;
    Origin::parse(&asked.hub).map_err(Refusal)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let folder::Kept { asset, field, already, unmarked } = folder.import(&asked.source, asked.clean).await.map_err(|why| {
        let said = format!("{}: {why}", asked.source);
        Refusal(Error::new(Kind::Validation, "import_failed", said).remedy("cozy project import takes a picture, video or sound, by path or http(s) URL"))
    })?;
    let mark = asked.clean && field == "image";
    let mut warnings = vec![];
    if asked.clean && !mark {
        warnings.push(format!("a{} {field} has no background to remove: it is imported as it is", if field == "audio" { "n" } else { "" }));
    }
    if unmarked {
        warnings.push("it was marked to have its background removed, and is used as it is from now on: import it with --character or --prop to mark it again".into());
    }
    let lock = folder.lock().map_err(Refusal)?;
    let cleaned = lock.imports.get(&asset.sha256).is_some_and(|import| import.clean.is_some());
    let clean = match (mark, cleaned, lock.cleaning(&asset.sha256)) {
        (false, ..) => Clean::Untouched,
        (true, true, _) => Clean::Removed,
        (true, false, Some(run)) if run > 0 => Clean::Removing { run: run.to_string() },
        _ if local_usable("local").is_err() => Clean::Later,
        _ => {
            let sender = Walk::new(&api, &project);
            let target = Target { on: vec![On::Machine("local".into())], hub: asked.hub.clone() };
            match sender.clean_import(&asset.sha256, &target).await {
                Ok(run) => {
                    walk(&api, &project);
                    Clean::Removing { run: run.id.to_string() }
                }
                Err(error) => {
                    warnings.push(format!("its background is not removed yet: {}", error.message));
                    Clean::Later
                }
            }
        }
    };
    let prefix = folder::prefix_of(&asset.sha256).to_owned();
    let line = format!("{field}: {}", video::yaml_scalar(&prefix));
    Ok(Json(Imported { path: asset.path, sha256: asset.sha256, prefix, line, already, clean, warnings }))
}

/// What node `i` reads, near or far, with no version yet and none on its way.
fn unmade(video: &Video, lock: &Lock, i: usize) -> Vec<usize> {
    let graph = &video.graph;
    let (mut found, mut seen, mut next) = (vec![], BTreeSet::new(), graph.upstream[i].iter().map(|edge| edge.node).collect::<Vec<_>>());
    while let Some(j) = next.pop() {
        if !seen.insert(j) {
            continue;
        }
        next.extend(graph.upstream[j].iter().map(|edge| edge.node));
        let on_its_way = (lock.pending.values().any(|pending| name_key(&pending.made.node) == name_key(&graph.nodes[j].id)))
            || lock.walk.takes.iter().any(|take| name_key(&take.node) == name_key(&graph.nodes[j].id));
        if graph.nodes[j].file.is_none() && !video.is_asset(j) && !on_its_way {
            found.push(j);
        }
    }
    found
}

async fn approve(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Approved>, Refusal> {
    let asked: Approve = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let (video, lock) = (Video::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&video, &asked.name).map_err(Refusal)?;
    let node = &video.graph.nodes[i];
    let versions: Vec<&Made> = lock.node(&node.id).into_iter().flat_map(|n| &n.versions).collect();
    let chosen = versions.iter().find(|made| made.number == asked.version).and_then(|made| made.primary());
    let file = chosen.map(|asset| asset.path.clone()).ok_or_else(|| {
        let numbers: Vec<String> = versions.iter().map(|made| made.number.to_string()).collect();
        let said = match numbers.as_slice() {
            [] => format!("{} has no {} yet", node.id, node.noun),
            numbers => format!("{} has no {} {}: it has {}", node.id, node.noun, asked.version, numbers.join(", ")),
        };
        Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy project status"))
    })?;
    let before = video.graph.by_id(&video.graph.status(&lock));
    let key = name_key(&node.id);
    // "This one": it is current until what it reads, or its text, changes from here.
    let (text, reading) = (node.text.clone(), video.reading(&lock, i));
    let chosen = |lock: &mut Lock| {
        let entry = lock.nodes.entry(key.clone()).or_default();
        entry.approved = Some(file.clone());
        if let Some(made) = entry.versions.iter_mut().find(|made| made.number == asked.version) {
            (made.text, made.upstream) = (Some(text.clone()), Some(reading.clone()));
        }
    };
    folder.update_lock(chosen).map_err(Refusal)?;
    let (after, lock) = (Video::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let stale = (after.graph.by_id(&after.graph.status(&lock)).into_iter())
        .filter(|(id, status)| matches!(status, Status::Stale(_)) && !matches!(before.get(id), Some(Status::Stale(_))))
        .map(|(id, _)| id)
        .collect();
    // A walk going collects at its end: what it is landing is not listed yet.
    if !*project.walking.lock().expect("walking lock") {
        garbage(folder).map_err(Refusal)?;
    }
    if video.cut_reads(i) {
        Walk::new(&api, &project).recut().await.map_err(Refusal)?;
    }
    let assets = lock.made(&node.id, &file).map(listed).unwrap_or_default();
    let (name, noun) = (node.id.clone(), node.noun.to_owned());
    let warnings = (video.pinned_by(i).into_iter())
        .map(|field| format!("{name} is pinned by its {field}:, which it reads until that is removed"))
        .collect();
    Ok(Json(Approved { name, noun, number: asked.version, assets, path: file, stale, warnings }))
}

async fn events(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, Refusal> {
    let tail = api.projects.get(&api, &id)?.events.subscribe();
    let stop = api.stop_signal();
    let stream = futures_util::stream::unfold((tail, stop), |(mut tail, mut stop)| async move {
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
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// One of the project's assets by its name in assets/ (`<sha256>.<ext>`): content-addressed, so its
/// sha256 is its ETag.
async fn asset(
    State(api): State<Arc<Api>>,
    Path((id, asset)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let (hex, ext) = asset.split_once('.').unwrap_or((&asset, ""));
    let named = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    if !named {
        let said = format!("{asset} is no asset: assets are named <sha256>.<ext>");
        return Err(Refusal(Error::new(Kind::Usage, "asset_name", said)));
    }
    send(&project.folder.path(&format!("{}/{asset}", folder::ASSETS)), &format!("\"{hex}\""), &headers)
}

/// One of the project's cuts (`edit.mp4`, `preview.mp4`) or its timeline (`edit.otio`), as last
/// made: its ETag names that cut.
async fn cut(
    State(api): State<Arc<Api>>,
    Path((id, cut)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let video = Video::load(&project.folder).map_err(Refusal)?;
    let named = |name: &str| name == cut || FsPath::new(name).with_extension("otio") == FsPath::new(&cut);
    if !made_cuts(&video).iter().any(|(name, _)| named(name)) {
        let made: Vec<&str> = made_cuts(&video).into_iter().map(|(name, _)| name).collect();
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

/// What landing a run left to do.
enum Landed {
    Done,
    /// A picture to clean before it is listed, its record, and the Hub and machines its clean goes to.
    Clean {
        made: Box<Made>,
        picture: PathBuf,
        approve: bool,
        clears: bool,
        hub: String,
        on: Vec<On>,
    },
    /// One of a shot's calls, its assets kept: its write frames to cut, then its next call or the
    /// shot's take.
    Call {
        made: Box<Made>,
        call: Call,
        approve: bool,
        clears: bool,
        hub: String,
        on: Vec<On>,
    },
}

/// A version to send now: node `i`'s for `all` (take None), the `nth` of its versions this walk, or a take's.
struct Next {
    i: usize,
    take: Option<Take>,
    nth: u32,
}

/// A project's one walker: what versions.lock owes, each version sent once what it reads is
/// settled, each run followed until it lands.
struct Walk {
    host: Host,
    project: Arc<Project>,
    began: Instant,
    /// What it landed, for its last word.
    tally: Mutex<Rendered>,
}

impl Walk {
    fn new(api: &Arc<Api>, project: &Arc<Project>) -> Self {
        Self { host: Host::new(api), project: project.clone(), began: Instant::now(), tally: Mutex::default() }
    }

    /// What it landed and how long it took, if it landed anything.
    fn rendered(&self) -> Option<Rendered> {
        let tally = self.tally.lock().expect("tally lock").clone();
        (tally.takes + tally.versions > 0).then(|| Rendered { elapsed_ms: self.began.elapsed().as_millis() as u64, ..tally })
    }

    fn folder(&self) -> &Folder {
        &self.project.folder
    }

    /// The project as project.yaml reads now. A save that doesn't read, say mid-edit, stops nothing:
    /// the walk goes on with what it last read, and says so once.
    fn video(&self) -> Result<Video, Error> {
        let text = self.folder().text()?;
        let mut read = self.project.read.lock().expect("read lock");
        match Video::read(self.folder(), &text) {
            Ok(video) => {
                *read = Some((text, false));
                Ok(video)
            }
            Err(e) if e.kind == Kind::Validation && read.is_some() => {
                let (last, said) = read.as_mut().expect("is some");
                if !std::mem::replace(said, true) {
                    self.say(Event::Warning { message: format!("{}: the render goes on with what it said before", e.message) });
                }
                Video::read(self.folder(), last)
            }
            Err(e) => Err(e),
        }
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

    /// Cleans the marked pictures what it owes reads, sends whatever is ready, follows every pending run
    /// (what it sent, a picture's clean, what an earlier walker sent), and plans again as each lands or something is asked, until nothing is
    /// owed: then the first thing that failed, if one did. What `all` asked ends with the edit, cut
    /// once nothing more of it can be made.
    async fn go(&self) -> Result<Option<Error>, Error> {
        for message in self.video()?.warnings {
            self.say(Event::Warning { message });
        }
        let (mut first, mut following, mut followed) = (None, FuturesUnordered::new(), HashSet::new());
        // The marked pictures this walk has sent to be cleaned, each once: what reads one that failed is
        // not made, and the next walk tries again.
        let mut cleaning = HashSet::new();
        loop {
            let (video, lock) = (self.video()?, self.folder().lock()?);
            for (sha256, target) in video.to_clean(&lock) {
                if cleaning.insert(sha256.clone())
                    && lock.cleaning(&sha256).is_none()
                    && let Err(error) = self.clean_import(&sha256, &target).await
                {
                    self.failed(error, &mut first);
                }
            }
            let lock = self.folder().lock()?;
            for take in lock.walk.takes.iter().filter(|take| video.graph.find(&take.node).is_none()) {
                let said = format!("{} is no longer in {}: its version is not made", take.node, folder::PROJECT);
                self.failed(Error::new(Kind::NotFound, "node_unknown", said), &mut first);
                self.folder().update_lock(|lock| lock.walk.takes.retain(|t| t.key != take.key))?;
            }
            for take in &lock.walk.takes {
                let Some(i) = video.graph.find(&take.node) else { continue };
                let (gone, unclean) = (video.missing(i), video.unclean(i));
                // It waits for a clean on its way.
                if gone.is_empty() && (unclean.is_empty() || unclean.iter().any(|sha256| lock.cleaning(sha256).is_some())) {
                    continue;
                }
                let why = match gone.is_empty() {
                    true => "its picture's background could not be removed".to_owned(),
                    false => format!("{} not on this computer: cozy project pull", gone.join(", ")),
                };
                self.failed(Error::new(Kind::NotFound, "file_missing", format!("{} cannot render: {why}", take.node)), &mut first);
                self.folder().update_lock(|lock| lock.walk.takes.retain(|t| t.key != take.key))?;
            }
            for next in plan(&video, &lock) {
                if let Err(error) = self.send(&video, &lock, next).await {
                    self.failed(error, &mut first);
                }
            }
            for key in self.folder().lock()?.pending.into_keys() {
                if followed.insert(key.clone()) {
                    following.push(self.follow(key));
                }
            }
            if following.is_empty() {
                let owed = self.folder().lock()?;
                if owed.walk.all.is_some() {
                    garbage(self.folder())?;
                    // A named node it could not make is said; the edit is cut only for the whole project, as a shot's landing cuts it.
                    if owed.walk.only.is_empty() {
                        if let Err(error) = self.recut().await {
                            self.failed(error, &mut first);
                        }
                    }
                    let video = self.video()?;
                    let status = video.graph.status(&owed);
                    for &i in video.graph.order.iter().filter(|&&i| status[i] != Status::Current) {
                        let (id, key) = (&video.graph.nodes[i].id, name_key(&video.graph.nodes[i].id));
                        if owed.walk.only.contains(&key) && !owed.walk.sent.contains(&key) {
                            let why = match (video.missing(i).as_slice(), video.unclean(i).is_empty()) {
                                ([], true) => "what it reads was not made".to_owned(),
                                ([], false) => "its picture's background could not be removed".to_owned(),
                                (gone, _) => format!("{} not on this computer: cozy project pull", gone.join(", ")),
                            };
                            self.failed(Error::new(Kind::NotFound, "file_missing", format!("{id} cannot render: {why}")), &mut first);
                        }
                    }
                    self.folder().update_lock(|lock| {
                        lock.walk.all = None;
                        lock.walk.preview = false;
                        lock.walk.sent.clear();
                        lock.walk.only.clear();
                        lock.walk.new = false;
                        lock.walk.n = 0;
                    })?;
                } else if self.over()? {
                    garbage(self.folder())?;
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

    /// A version that failed: said now, never sent again this walk; what does not read it goes on.
    fn failed(&self, error: Error, first: &mut Option<Error>) {
        self.say(Event::Failed { error: error.clone() });
        first.get_or_insert(error);
    }

    /// Sends `next` under its key: a new version made `on` its target's machines (chosen for
    /// `all`, or when the entity has none; a picture its renderer can't draw transparent is cleaned
    /// before it is listed). Recorded as pending together with what it settles of the walk, before
    /// the run is sent; one that fails is settled all the same.
    async fn send(&self, video: &Video, lock: &Lock, next: Next) -> Result<(), Error> {
        let i = next.i;
        let id = video.graph.nodes[i].id.clone();
        let key = next.take.as_ref().map_or_else(new_key, |take| take.key.clone());
        let settle = |lock: &mut Lock| match &next.take {
            None => {
                lock.walk.sent.insert(name_key(&id));
            }
            Some(_) => lock.walk.takes.retain(|take| take.key != key),
        };
        let status = &video.graph.status(lock)[i];
        let (target, approve) = match &next.take {
            None => (lock.walk.all.clone().expect("all is owed"), false),
            Some(take) => (take.target.clone(), take.approve),
        };
        // What brings an out-of-date node up to date is what it reads from now on.
        let clears = *status != Status::Current;
        if next.take.is_none() && next.nth == 0 {
            let noun = video.graph.nodes[i].noun;
            let said = if *status == Status::Current { format!("a new {noun}") } else { shown(status, noun) };
            self.say(Event::Said { message: format!("{id}: {said}") });
        }
        let sent = async {
            let renderer = (self.host)
                .renderer(&target.hub, video.render(i).target(), &self.folder().dir, &target.on)
                .await?;
            let mut request = video.request(self.folder(), lock, i, &renderer)?;
            request.warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
            let then = match video.cleans(i) {
                true => Then::Clean { approve, clears },
                false => Then::List { approve, clears },
            };
            let (mut frames, mut said) = (vec![], vec![]);
            for input in &mut request.files {
                // A frame of a video, taken out here: held until the run has been sent.
                let Some(at) = input.at else { continue };
                let (frame, past) = self.frame(&id, &input.path, at).await?;
                said.extend(past);
                input.digest = folder::hash(frame.path())?;
                input.path = frame.path().to_owned();
                set(&mut request.payload, &input.field, Value::String(input.digest.clone()));
                set(&mut request.node, &input.field, Value::String(input.digest.clone()));
                frames.push(frame);
            }
            let (payload, files) = self.invocation(&mut request, &renderer, &target.hub, &target.on).await?;
            let (mut made, field, call, hub) =
                (request.made.clone(), request.field.into(), request.call.clone(), target.hub.clone());
            made.warnings.extend(said);
            // Its number now, so the first of several sent is the lowest, whichever lands first.
            let number = std::cell::Cell::new(0);
            self.folder().update_lock(|lock| number.set(lock.number(&id)))?;
            made.number = number.get();
            let pending = Pending { made, run: 0, hub, field, on: vec![], call, then };
            self.submit(&key, &payload, files, &request.render, &target.on, pending, &settle).await
        };
        let sent = sent.await;
        if sent.is_err() {
            self.folder().update_lock(settle)?;
        }
        sent
    }

    /// The cuts again here from the approved assets (fast join: only what changed is encoded), each
    /// filling the project's canvas (`made_cuts`; a preview walk makes only the native one). What has
    /// nothing made yet is skipped, and said.
    async fn recut(&self) -> Result<(), Error> {
        let video = self.video()?;
        let preview_walk = self.folder().lock()?.walk.preview;
        let cuts = made_cuts(&video).into_iter().filter(|(_, native)| *native || !preview_walk);
        for (name, preview) in cuts {
            let (mut pieces, skipped) = video.cuts(self.folder(), preview)?;
            if !skipped.is_empty() {
                self.say(Event::Said { message: format!("{name} skips {}: no version yet", skipped.join(", ")) });
            }
            if pieces.is_empty() {
                continue;
            }
            let (ffmpeg, ffprobe) = self.host.ffmpeg().await?;
            // A file taken in before this computer had an ffprobe is probed now.
            if pieces.iter().any(|piece| piece.shape.is_none()) {
                pieces = self.video()?.cuts(self.folder(), preview)?.0;
            }
            for message in video::unreadable(&mut pieces).into_iter().chain(video::past_end(&mut pieces, video.canvas.fps)) {
                self.say(Event::Warning { message });
            }
            if pieces.is_empty() {
                continue;
            }
            let canvas = Canvas { short: if preview { None } else { video.canvas.short }, ..video.canvas.clone() };
            // What it holds, which unchanged and still there is not cut again.
            let holds = video::holds(&pieces, &canvas);
            let fps = video.canvas.fps;
            if self.folder().lock()?.cuts.get(name) == Some(&holds) && self.folder().dir.join(name).is_file()
                && let Some(made) = video::measure(&pieces, fps)
            {
                self.say(Event::Cut { cut: name.into(), frames: made.frames, fps, retimed: made.retimed, took_s: 0.0, unchanged: true });
                continue;
            }
            let (project, cutting, began) = (self.project.clone(), self.project.cutting.clone(), Instant::now());
            let cache = self.host.cache().join("cuts");
            let made = tokio::task::spawn_blocking(move || {
                let _held = cutting.lock().expect("cutting lock");
                video::cut(&ffmpeg, &ffprobe, &project.folder, &pieces, &canvas, name, &cache)
            })
            .await
            .map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))??;
            self.folder().update_lock(|lock| drop(lock.cuts.insert(name.to_owned(), holds.clone())))?;
            let (frames, retimed, took_s) = (made.frames, made.retimed, began.elapsed().as_secs_f64());
            self.say(Event::Cut { cut: name.into(), frames, fps, retimed, took_s, unchanged: false });
        }
        Ok(())
    }

    /// Follows the run sent under `key` to its end and lands it, its pending entry gone however it
    /// ends (a lock that cannot be written ends the walk). A picture its entity cleans is sent to its
    /// clean in the same write, which the walk follows next; the edit is cut again when it cuts the
    /// entity.
    async fn follow(&self, key: String) -> (String, Result<Result<(), Error>, Error>) {
        let landed = async {
            let Some(run) = self.host.sent(&key)? else { return Ok(None) };
            let run = self.host.wait(&run.id).await?;
            let node = self.folder().lock()?.pending.get(&key).map(|pending| pending.made.node.clone());
            let (made, picture, approve, clears, hub, on) = match self.land(&key, &run).await? {
                Landed::Done => return Ok(node),
                Landed::Call { made, call, approve, clears, hub, on } => {
                    return self.called(&key, *made, call, (approve, clears), &hub, &on).await;
                }
                Landed::Clean { made, picture, approve, clears, hub, on } => (made, picture, approve, clears, hub, on),
            };
            let cleaner = self.video()?.clean_render();
            let picture = (picture.clone(), folder::hash(&picture)?);
            let settle = |lock: &mut Lock| drop(lock.pending.remove(&key));
            self.clean(&new_key(), &hub, &cleaner, picture, *made, Then::List { approve, clears }, &on, &settle).await?;
            Ok(None)
        };
        let landed = landed.await;
        if let Err(error) = self.folder().update_lock(|lock| drop(lock.pending.remove(&key))) {
            return (key, Err(error));
        }
        let cut = async {
            let video = self.video()?;
            match landed?.and_then(|node| video.graph.find(&node)).is_some_and(|i| video.cut_reads(i)) {
                true => self.recut().await,
                false => Ok(()),
            }
        };
        let cut = cut.await;
        (key, Ok(cut))
    }

    /// A shot's call that landed (its assets kept in `made`): the frames its writes take from its
    /// video, cut here; then its next call, which continues it, or (its last) the shot's take, with
    /// its last call's assets and every call's run. What the walk follows is pending under the next
    /// call's key in the same write that drops this one's.
    async fn called(
        &self,
        key: &str,
        mut made: Made,
        call: Call,
        (approve, clears): (bool, bool),
        hub: &str,
        on: &[On],
    ) -> Result<Option<String>, Error> {
        let folder = self.folder();
        let node = made.node.clone();
        let video = self.video()?;
        let i = find(&video, &node)?;
        let noun = video.graph.nodes[i].noun;
        let Some(video_asset) = made.assets.get("video").cloned() else {
            return Err(Error::new(Kind::Unavailable, "version_unsaved", format!("{node}'s call saved no video")));
        };
        for (state, at) in &call.writes {
            // Half a frame early: a write at the shot's end is its last frame.
            let (frame, past) = self.frame(&node, &folder.path(&video_asset.path), (at - 1.0 / 48.0).max(0.0)).await?;
            made.warnings.extend(past);
            made.writes.insert(state.clone(), folder.keep(frame.path())?);
        }
        if call.index + 1 < call.ends.len() {
            let next = Call {
                index: call.index + 1,
                ends: call.ends.clone(),
                writes: vec![],
                from: Some((video_asset, made.assets.get("latents").cloned())),
            };
            let lock = folder.lock()?;
            let renderer = self.host.renderer(hub, video.render(i).target(), &folder.dir, on).await?;
            let mut request = video.next_call(folder, &lock, i, &renderer, next, &made.writes)?;
            request.warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
            let (payload, files) = self.invocation(&mut request, &renderer, hub, on).await?;
            let carried = Made {
                node: node.clone(),
                number: made.number,
                upstream: made.upstream.clone(),
                text: made.text.clone(),
                execution_ms: made.execution_ms,
                writes: made.writes.clone(),
                runs: made.runs.clone(),
                invocations: made.invocations.iter().chain(&request.made.invocations).cloned().collect(),
                warnings: made.warnings.clone(),
                ..Default::default()
            };
            let then = Then::List { approve, clears };
            let field = "video".into();
            let pending = Pending { made: carried, run: 0, hub: hub.into(), field, on: vec![], call: request.call, then };
            let settle = |lock: &mut Lock| drop(lock.pending.remove(key));
            self.submit(&new_key(), &payload, files, &request.render, on, pending, &settle).await?;
            return Ok(None);
        }
        self.record(Some(key), &made, (approve, clears), video.graph.nodes[i].kind, noun)?;
        Ok(Some(node))
    }

    /// The clean of the imported picture `sha256`, on one of `target`'s machines: the run, which the walk follows.
    async fn clean_import(&self, sha256: &str, target: &Target) -> Result<Run, Error> {
        let (video, lock, key) = (self.video()?, self.folder().lock()?, new_key());
        let path = (lock.imports.get(sha256).map(|import| self.folder().path(&import.asset.path)))
            .ok_or_else(|| Error::new(Kind::NotFound, "import_unknown", format!("project.lock names no import {sha256}")))?;
        let node = folder::prefix_of(sha256).to_owned();
        let made = Made { node, clean_of: Some(sha256.to_owned()), ..Default::default() };
        let picture = (path, sha256.to_owned());
        self.clean(&key, &target.hub, &video.clean_render(), picture, made, Then::Imported, &target.on, &|_: &mut Lock| {}).await?;
        let sent = self.host.sent(&key)?;
        sent.ok_or_else(|| Error::new(Kind::Internal, "clean_unsent", format!("the clean of {sha256} was not sent")))
    }

    /// The clean of a picture (its path and digest), keeping the record `made` of what it was made
    /// from: one run under `key` on one of `on`'s machines, landed as `then` says and marked as a
    /// clean.
    #[allow(clippy::too_many_arguments)]
    async fn clean(
        &self,
        key: &str,
        hub: &str,
        cleaner: &Render,
        picture: (PathBuf, String),
        made: Made,
        then: Then,
        on: &[On],
        settle: &impl Fn(&mut Lock),
    ) -> Result<(), Error> {
        let made = Made { cleaned: true, writes: Default::default(), ..made };
        let pending = Pending { made, run: 0, hub: hub.into(), field: "image".into(), on: vec![], call: None, then };
        let payload = serde_json::json!({ "image": picture.1 });
        let files = vec![("image".into(), picture.0, picture.1)];
        self.submit(key, &payload, files, cleaner, on, pending, settle).await
    }

    /// Sends `payload` (its `files` as field, path and digest) to `render`'s function on one of `on`'s
    /// machines as an ordinary run under `key`, recorded as pending (with `settle`: what it settles of
    /// the walk) in one write before it is sent. The render block's `models` and `lora` are the run's
    /// model choices, as `cozy run --input` takes them, and its `options` join the request; an option
    /// may not set a field the project compiles. The version records them, and the release, as what
    /// made it.
    #[allow(clippy::too_many_arguments)]
    /// The call `request` makes: its package's adapter's, or its flat request. The take records it; its files are found
    /// in it by digest.
    async fn invocation(
        &self,
        request: &mut Request,
        renderer: &Renderer,
        hub: &str,
        on: &[On],
    ) -> Result<(Value, Vec<(String, PathBuf, String)>), Error> {
        let (function, payload) = match request.adapter.clone() {
            None => (renderer.function.clone(), request.payload.clone()),
            Some(adapter) => self.adapt(renderer, &adapter, &request.node, hub, on).await?,
        };
        if function != renderer.function {
            let target = request.render.target();
            let (path, release) = target.split_once('@').map_or((target, None), |(path, release)| (path, Some(release)));
            let package = path.rsplit_once('/').map_or(path, |(package, _)| package);
            let with = release.map_or_else(|| format!("{package}/{function}"), |release| format!("{package}/{function}@{release}"));
            request.render.with = Some(with);
        }
        request.made.invocations.push(serde_json::json!({"function": function, "input": payload}));
        let files = video::files_in(&payload, &request.files);
        Ok((payload, files))
    }

    /// A package's project adapter on `node`, an ordinary run on the machine the render goes to: the function and input it
    /// answers. It holds no files: a file is its digest.
    async fn adapt(&self, renderer: &Renderer, adapter: &str, node: &Value, hub: &str, on: &[On]) -> Result<(String, Value), Error> {
        let failed = |why: String| Error::new(Kind::Unavailable, "project_adapter", format!("{}'s {adapter}: {why}", renderer.package));
        let placed = self.host.place(on, renderer);
        let (source, inputs) = match &renderer.local {
            None => (v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() }), vec![]),
            Some(path) => {
                let prepared = self.host.ready(path, &placed, hub).await?;
                (v1::Source::Local(prepared.source), prepared.files)
            }
        };
        let spec = v1::RunSpec {
            kind: v1::RunKind::Call,
            source: Some(source),
            entrypoint: adapter.to_owned(),
            payload: Some(v1::Json::parse(node.to_string()).expect("a node is JSON")),
            ..Default::default()
        };
        let asked = Renderer { function: adapter.to_owned(), ..renderer.clone() };
        let submit = SubmitRun { on: placed, hub: hub.to_owned(), kind: RunKind::Call, target: asked.target(), spec, inputs, out: None, warnings: vec![], agreed: None };
        let run = self.send_run(&new_key(), submit).await?;
        let run = self.host.wait(&run.id).await?;
        if run.status != RunStatus::Completed {
            return Err(failed(run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |r| r.message.clone())));
        }
        let result = run.result.as_ref().ok_or_else(|| failed("it returned nothing".into()))?;
        let function = result["function"].as_str().ok_or_else(|| failed("it named no function".into()))?;
        Ok((function.to_owned(), result["input"].clone()))
    }

    async fn submit(
        &self,
        key: &str,
        payload: &Value,
        files: Vec<(String, PathBuf, String)>,
        render: &Render,
        on: &[On],
        mut pending: Pending,
        settle: &impl Fn(&mut Lock),
    ) -> Result<(), Error> {
        let target = render.target();
        let renderer = self.host.renderer(&pending.hub, target, &self.folder().dir, on).await?;
        let node = pending.made.node.clone();
        let named = |e: Error| Error { message: format!("{node}'s render: {}", e.message), ..e };
        let mut payload = payload.clone();
        let fields = payload.as_object_mut().expect("a request is an object");
        // A person's options stand over what the project sends, and that is said.
        let over: Vec<String> = render.options.keys().filter(|key| fields.contains_key(*key)).cloned().collect();
        fields.extend(render.options.clone());
        for (key, value) in [("models", &render.models), ("lora", &render.lora)] {
            if !value.is_null() {
                fields.insert(key.into(), value.clone());
            }
        }
        let mut choices = Choices::new(target, &renderer.function, &renderer.slots, renderer.calls.iter().collect());
        choices.take(fields).map_err(named)?;
        pending.on = on.to_vec();
        // A clean keeps what made the picture it cleans.
        pending.made.made_by.get_or_insert(MadeBy {
            with: format!("{}/{}@{}", renderer.package, renderer.function, renderer.release),
            models: render.models.clone(),
            lora: render.lora.clone(),
            options: render.options.clone(),
        });
        let mut warnings = match choices.dropped.is_empty() {
            true => vec![],
            false => vec![Warning {
                code: "request_fields_ignored".into(),
                message: format!("{node}'s render: ignored {}", choices.dropped.join(", ")),
            }],
        };
        warnings.extend(choices.replaced.iter().map(|replaced| Warning {
            code: "model_slot_replaced".into(),
            message: format!("{node}'s render: {replaced}"),
        }));
        warnings.extend((!over.is_empty()).then(|| Warning {
            code: "render_options_over".into(),
            message: format!("{node}'s render options set {} over what the project sends", over.join(", ")),
        }));
        let models = choices::wire(&choices.into_choices()).map_err(named)?;
        let mut inputs = vec![];
        let mut staged = vec![];
        for (field, path, digest) in files {
            let length = std::fs::metadata(&path)
                .map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())).next("cozy project pull"))?
                .len();
            let order = field.split('.').filter_map(|part| part.parse().ok()).next_back().unwrap_or(0);
            let media_type = media_type(&path).to_owned();
            inputs.push(v1::InputFile { field, digest: digest.clone(), length, media_type, order });
            let digest: Digest = digest.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?;
            staged.push(StagedInput { path, digest, length });
        }
        let placed = self.host.place(on, &renderer);
        // Local code is made ready on the machine the call goes to, its files and manifest sent
        // with every call: a machine that dropped them installs them in the call.
        let source = match &renderer.local {
            None => v1::Source::Release(v1::Release {
                package: renderer.package.clone(),
                release: renderer.release.clone(),
            }),
            Some(path) => {
                let prepared = self.host.ready(path, &placed, &pending.hub).await.map_err(named)?;
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
            hub: pending.hub.clone(),
            kind: RunKind::Call,
            target: renderer.target(),
            spec,
            inputs: staged,
            out: None,
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
        let name = pending.made.node.clone();
        self.say(Event::Started { name, run: run.id.to_string(), number: run.number });
        Ok(())
    }

    /// The run sent under `key`, once it ended, into the project: its outputs kept as assets and
    /// listed as its node's newest version in project.lock, approved or its approval cleared as its `then` says. A run that did not complete is dropped with its reason and never run
    /// again.
    async fn land(&self, key: &str, run: &Run) -> Result<Landed, Error> {
        let folder = self.folder();
        let Some(pending) = folder.lock()?.pending.remove(key) else { return Ok(Landed::Done) };
        if let (Then::Imported, Some(of)) = (pending.then, pending.made.clean_of.as_deref()) {
            return self.land_clean(key, run, of);
        }
        let node = pending.made.node.clone();
        if run.status != RunStatus::Completed {
            let status = format!("{:?}", run.status).to_lowercase();
            let said = match &run.reason {
                Some(reason) => format!("{node}: {status}: {}", reason.message),
                None => format!("{node}: {status}"),
            };
            return Err(
                Error::new(Kind::Unavailable, "version_failed", said).next(format!("cozy run show {}", run.number))
            );
        }
        let picture = saved(run, &pending.field)?;
        let video = self.video()?;
        let Some(i) = video.graph.find(&node) else {
            folder.update_lock(|lock| drop(lock.pending.remove(key)))?;
            let message = format!("{node} is no longer in {}: run {}'s file is not kept", folder::PROJECT, run.number);
            self.say(Event::Warning { message });
            return Ok(Landed::Done);
        };
        let noun = video.graph.nodes[i].noun;
        // Its number, given at its first landing and kept with it, so a landing done again keeps it.
        let number = match pending.made.number {
            0 => {
                let given = std::cell::Cell::new(0);
                folder.update_lock(|lock| {
                    let number = lock.number(&node);
                    if let Some(pending) = lock.pending.get_mut(key) {
                        pending.made.number = number;
                    }
                    given.set(number);
                })?;
                given.get()
            }
            number => number,
        };
        let mut made = Made { number, ..pending.made.clone() };
        made.runs.push(run.number);
        made.execution_ms += run.execution_ms.unwrap_or(0);
        for said in said_by(run) {
            let message = format!("{node}: {said}");
            self.say(Event::Warning { message: message.clone() });
            if !made.warnings.contains(&message) {
                made.warnings.push(message);
            }
        }
        // A picture that came back with a real share of fully transparent pixels needs no clean.
        let opaque = match (&pending.then, self.host.ffmpeg().await) {
            (Then::Clean { .. }, Ok((ffmpeg, _))) => {
                let path = picture.clone();
                tokio::task::spawn_blocking(move || video::transparent_share(&ffmpeg, &path) < TRANSPARENT).await.unwrap_or(true)
            }
            _ => true,
        };
        if let (Then::Clean { approve, clears }, true) = (pending.then, opaque) {
            // Listed once cleaned, made by both runs: the render as its `image`, and its `clean`.
            made.assets.insert(pending.field.clone(), folder.keep(&picture)?);
            return Ok(Landed::Clean { made: Box::new(made), picture, approve, clears, hub: pending.hub, on: pending.on });
        }
        if made.cleaned {
            made.assets.insert("clean".into(), folder.keep(&picture)?);
        } else {
            // Every file the run saved, by its output's name (a list's items as `<name>-<n>`).
            for output in &run.output {
                let Some(path) = output.path.as_ref().filter(|path| path.is_file()) else { continue };
                let name = match output.index {
                    0 => output.name.clone(),
                    n => format!("{}-{n}", output.name),
                };
                made.assets.insert(name, folder.keep(path)?);
            }
            made.assets.insert(pending.field.clone(), folder.keep(&picture)?);
        }
        if let Some(asset) = made.assets.get_mut("video") {
            let (_, ffprobe) = self.host.ffmpeg().await?;
            asset.video = video::shape(&ffprobe, &folder.path(&asset.path));
        }
        let (approve, clears) = match pending.then {
            Then::List { approve, clears } | Then::Clean { approve, clears } => (approve, clears),
            Then::Imported => (false, false),
        };
        if let Some(call) = pending.call.clone() {
            let (hub, on) = (pending.hub.clone(), pending.on.clone());
            return Ok(Landed::Call { made: Box::new(made), call, approve, clears, hub, on });
        }
        self.record(Some(key), &made, (approve, clears), video.graph.nodes[i].kind, noun)?;
        Ok(Landed::Done)
    }

    /// The run cleaning the imported picture `of`, once it ended: its clean kept, and from now on what the
    /// hash resolves to. One that did not complete leaves the mark, and the next render that reads it tries again.
    fn land_clean(&self, key: &str, run: &Run, of: &str) -> Result<Landed, Error> {
        let folder = self.folder();
        let kept = match run.status {
            RunStatus::Completed => saved(run, "image").and_then(|picture| folder.keep(&picture)),
            _ => {
                let why = run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |r| r.message.clone());
                Err(Error::new(Kind::Unavailable, "clean_failed", why).next(format!("cozy run show {}", run.number)))
            }
        };
        let (sha256, name) = (of.to_owned(), folder::prefix_of(of));
        let asset = match kept {
            Ok(asset) => asset,
            Err(error) => {
                folder.update_lock(|lock| drop(lock.pending.remove(key)))?;
                let message = format!("{name}: its background was not removed: {}; the next render that reads it tries again", error.message);
                self.say(Event::Cleaned { sha256, error: Some(message.clone()) });
                return Err(Error { message, ..error });
            }
        };
        folder.update_lock(|lock| {
            lock.pending.remove(key);
            if let Some(import) = lock.imports.get_mut(of) {
                import.clean = Some(asset.clone());
            }
        })?;
        self.say(Event::Said { message: format!("{name}: background removed ({})", asset.path) });
        self.say(Event::Cleaned { sha256, error: None });
        Ok(Landed::Done)
    }

    /// Lists `made` as its node's newest version (dropping the pending run under `key` in the same
    /// write), approved when `approve`, and says it landed.
    fn record(&self, key: Option<&str>, made: &Made, (approve, clears): (bool, bool), kind: &str, noun: &str) -> Result<(), Error> {
        let number = std::cell::Cell::new(made.number);
        let kept = std::cell::Cell::new(None);
        self.folder().update_lock(|lock| {
            if let Some(key) = key {
                lock.pending.remove(key);
            }
            number.set(lock.record(made.clone(), kind, approve, clears));
            let node = lock.node(&made.node);
            kept.set(node.and_then(|node| node.versions.get(node.current_at()?)).map(|made| made.number));
        })?;
        let (name, number, assets) = (made.node.clone(), number.get(), listed(made));
        let path = made.primary().map(|asset| asset.path.clone()).unwrap_or_default();
        {
            let mut tally = self.tally.lock().expect("tally lock");
            match noun {
                "take" => tally.takes += 1,
                _ => tally.versions += 1,
            }
            tally.execution_ms += made.execution_ms;
        }
        let execution_ms = (made.execution_ms > 0).then_some(made.execution_ms);
        let kept = kept.get().filter(|kept| *kept != number && matches!(kind, "shot" | "upscale"));
        let keeps = kept.map(|kept| format!("{name}: the edit keeps {noun} {kept}, approved; cozy project approve {name} {number} uses this one"));
        self.say(Event::Landed { name, noun: noun.into(), number, assets, path, execution_ms });
        if let Some(message) = keeps {
            self.say(Event::Said { message });
        }
        Ok(())
    }

    /// The frame of `video` at `at` seconds as a PNG beside the project's files, or its last when it
    /// ends before that (a take is what its renderer made), which is said and returned.
    async fn frame(&self, id: &str, video: &FsPath, at: f64) -> Result<(tempfile::NamedTempFile, Option<String>), Error> {
        let (ffmpeg, _) = self.host.ffmpeg().await?;
        let io = |e: std::io::Error| {
            Error::new(Kind::Unavailable, "project_io", format!("a frame of {}: {e}", video.display()))
        };
        let frame = tempfile::Builder::new().suffix(".png").tempfile_in(&self.folder().dir).map_err(io)?;
        let (video_path, out) = (video.to_owned(), frame.path().to_owned());
        let grab = move |seek: Vec<String>, once: bool| {
            let mut command = std::process::Command::new(&ffmpeg);
            command.args(["-y", "-v", "error"]).args(seek).arg("-i").arg(&video_path);
            command.args(if once { &["-frames:v", "1", "-update", "1"][..] } else { &["-update", "1"][..] }).arg(&out);
            let done = command.output();
            let empty = std::fs::metadata(&out).map_or(true, |meta| meta.len() == 0);
            (done, empty)
        };
        let task = |e: tokio::task::JoinError| Error::new(Kind::Internal, "frame", e.to_string());
        let (done, past) = tokio::task::spawn_blocking(move || {
            let (done, empty) = grab(vec!["-ss".into(), format!("{at}")], true);
            match (done, empty) {
                (Ok(done), false) if done.status.success() => (Ok(done), false),
                // Its last frame: the frames of its last second, each over the one before.
                _ => (grab(vec!["-sseof".into(), "-1".into()], false).0, true),
            }
        })
        .await
        .map_err(task)?;
        let done = done.map_err(io)?;
        if !done.status.success() || std::fs::metadata(frame.path()).map_or(0, |m| m.len()) == 0 {
            let said = String::from_utf8_lossy(&done.stderr);
            let said = format!("{id}'s frame: its shot's video has no frame ({})", said.trim());
            return Err(Error::new(Kind::Validation, "frame_missing", said));
        }
        let past = past.then(|| format!("{id}: its video ends before {at:.2} s: its last frame stands in"));
        if let Some(message) = &past {
            self.say(Event::Warning { message: message.clone() });
        }
        Ok((frame, past))
    }
}

/// What can be sent now, in project order. What `all` owes (a version out of date or missing, of what it covers; with `new`,
/// any version it reads as) once everything it reads is made, cleaned and settled: nothing upstream, however far, still owed or being
/// made into its chosen file (a take that will not be chosen holds nothing up). A take once nothing
/// upstream is unsettled.
fn plan(video: &Video, lock: &Lock) -> Vec<Next> {
    let (graph, walk) = (&video.graph, &lock.walk);
    let status = graph.status(lock);
    let mut unsettled = vec![false; graph.nodes.len()];
    let mut next = vec![];
    for &i in &graph.order {
        let id = name_key(&graph.nodes[i].id);
        // What lands becomes what it reads, unless a version stays chosen over it.
        let adopts = lock.node(&graph.nodes[i].id).is_none_or(|node| node.approved.is_none());
        let busy = lock.pending.values().any(|pending| {
            let chosen = match pending.then {
                Then::Imported => false,
                Then::List { approve, clears } | Then::Clean { approve, clears } => approve || clears,
            };
            name_key(&pending.made.node) == id && (chosen || adopts)
        });
        let covered = if walk.only.is_empty() { video.wanted(i) } else { walk.only.contains(&id) };
        let owed = walk.all.is_some()
            && covered
            && !(walk.preview && video.is_upscale(i))
            && (status[i] != Status::Current || (walk.new && video.reads_made(lock, i)))
            && !walk.sent.contains(&id);
        let up = graph.upstream[i].iter().any(|edge| unsettled[edge.node]);
        // A node reading a file not here, or a picture waiting for its clean, is not made yet, nor is what reads it.
        let gone = !video.missing(i).is_empty() || !video.unclean(i).is_empty();
        // A take of a node with no version yet: what reads it waits for it.
        let taken = walk.takes.iter().any(|take| name_key(&take.node) == id && (adopts || take.approve));
        unsettled[i] = busy || owed || up || gone || taken;
        if up || gone {
            continue;
        }
        let made = graph.upstream[i].iter().all(|edge| status[edge.node] == Status::Current);
        if owed && !busy && made {
            next.extend((0..walk.n.max(1)).map(|nth| Next { i, take: None, nth }));
        }
        next.extend(walk.takes.iter().filter(|take| name_key(&take.node) == id).map(|take| Next { i, take: Some(take.clone()), nth: 0 }));
    }
    next
}

/// A fresh idempotency key: one key names one run forever.
fn new_key() -> String {
    let mut key = [0u8; 16];
    getrandom::fill(&mut key).expect("the OS has randomness");
    format!("project-{}", hex::encode(key))
}

/// The file a run saved as its output `name` (a single output is index 0).
fn saved(run: &Run, name: &str) -> Result<PathBuf, Error> {
    (run.output.iter()).find(|o| o.name == name && o.index == 0).and_then(|o| o.path.clone()).ok_or_else(|| {
        Error::new(Kind::Unavailable, "version_unsaved", format!("run {} saved no {name}", run.number))
            .next(format!("cozy run show {}", run.number))
    })
}

/// What a run's function said of its result (its `warnings`), and its machine's.
fn said_by(run: &Run) -> Vec<String> {
    let result = run.result.as_ref().and_then(|result| result["warnings"].as_array()).into_iter().flatten();
    let result = result.filter_map(|said| said.as_str().map(str::to_owned));
    run.warnings.iter().map(|warning| warning.message.clone()).chain(result).collect()
}

/// `value` at `field` (`assets.0.image`) in `payload`.
fn set(payload: &mut Value, field: &str, value: Value) {
    let mut at = payload;
    for part in field.split('.') {
        at = match part.parse::<usize>() {
            Ok(n) => &mut at[n],
            Err(_) => &mut at[part],
        };
    }
    *at = value;
}

/// Deletes the assets project.lock no longer names.
fn garbage(folder: &Folder) -> Result<(), Error> {
    folder.collect_garbage()
}

fn find(video: &Video, name: &str) -> Result<usize, Error> {
    video.graph.find(name).ok_or_else(|| {
        Error::new(Kind::NotFound, "node_unknown", format!("the project has nothing named {name}"))
            .next("cozy project status")
    })
}

fn shown(status: &Status, noun: &str) -> String {
    match status {
        Status::Current => "current".into(),
        Status::Empty => format!("no {noun} yet"),
        Status::Stale(cause) => format!("out of date: {cause}"),
    }
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
        _ => "application/octet-stream",
    }
}
