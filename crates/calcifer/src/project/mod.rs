//! Projects (xs-136, xs-139, cl-716): a folder of versioned nodes that packages render. Calcifer
//! opens a project by its path (and keeps every folder it opened in its index) and walks it: each
//! version is an ordinary run, sent through its own submit as soon as what it reads is settled, so
//! independent ones run at once and the machines' schedulers place them. Each lands in assets/ and
//! project.lock (people write project.yaml; cozy never does), and the cuts are made here. One walker
//! per project takes every render asked of it, and what it owes is kept in project.lock, so a
//! restarted Calcifer goes on with it. Its clients ask and watch; the runs go on without them.
mod folder;
mod graph;
pub(crate) mod host;
mod video;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status as RunStatus, SubmitRun, Warning},
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    project::{
        Approve, Approved, Cut, Event, Known, Node as Row, NodeVersions, Open, Opened, Projects as Listed, Runs,
        Stale, State as ProjectState, Version,
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
use host::{Host, Install};
use video::{ALL, Canvas, Render, Video};

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
    /// Each `Idempotency-Key` a render was asked under since Calcifer started, and the ask's digest.
    keys: Mutex<HashMap<String, String>>,
}

pub fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/projects", post(open).get(list))
        .route("/v1/projects/{id}", get(state).delete(forget))
        .route("/v1/projects/{id}/runs", post(runs))
        .route("/v1/projects/{id}/approve", post(approve))
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
        let folder = Folder::open(dir)?;
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
                    keys: Mutex::default(),
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
    let walk = Walk { host: Host::new(api), project: project.clone() };
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
        walk.say(Event::Ended { error });
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
/// one there (`create`: the folder, its project.yaml, assets/, references/ and plates/); either way
/// it is recorded in the index as used now.
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
            for made in std::iter::once(folder::ASSETS).chain(folder::INPUTS) {
                std::fs::create_dir_all(open.path.join(made)).map_err(io)?;
            }
            platform::durable::replace(&project, video::skeleton().as_bytes(), 0o644)
                .map_err(|e| io(std::io::Error::other(e)))?;
            std::fs::canonicalize(&open.path).map_err(io)?
        }
        false => holding(&open.path).map_err(Refusal)?,
    };
    let id = api.projects.open(&api, &dir).map_err(Refusal)?;
    let entry = Entry { path: dir.clone(), used_at_ms: now_ms(), walking: false };
    indexed(&api, true, |index| {
        let walking = index.projects.get(&id).is_some_and(|known| known.walking);
        index.projects.insert(id.clone(), Entry { walking, ..entry });
    });
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
    let (beatless, mut warnings) = match beatless(&Host::new(&api), "", &video, &project.folder.dir, &[]).await {
        Ok(beatless) => (beatless, video.warnings.clone()),
        Err(error) => (
            vec![],
            [video.warnings.clone(), vec![format!("shot lengths are not checked: {}", error.message)]].concat(),
        ),
    };
    warnings.dedup();
    let (mut missing, mut stale) = (vec![], vec![]);
    let nodes = (video.graph.order.iter())
        .map(|&i| {
            let (row, standing) = row(&video, &lock, i, &beatless);
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
        let skips = match video.cuts(&project.folder, preview) {
            Ok((_, skips)) => skips,
            Err(error) => {
                warnings.push(error.message);
                vec![]
            }
        };
        cuts.push(Cut { cut: name.into(), path, cut_at_ms, skips });
    }
    warnings.dedup();
    let busy = *project.walking.lock().expect("walking lock");
    Ok(Json(ProjectState { id, path: project.folder.dir.clone(), nodes, missing, stale, busy, cuts, warnings }))
}

/// Where a node stands, for the lists under a project's table.
enum Standing {
    /// Nothing made yet that the project needs, or a picture not cleaned yet.
    Missing,
    Stale(String),
    Other,
}

/// Node `i` as `GET /v1/projects/{id}` lists it: its approved version, assets and status in its own
/// noun (`beatless`: the lines naming shots that need beats).
fn row(video: &Video, lock: &Lock, i: usize, beatless: &[String]) -> (Row, Standing) {
    let node = &video.graph.nodes[i];
    let status = &video.graph.status(lock)[i];
    let making: Vec<String> = (lock.pending.values())
        .filter(|p| name_key(&p.made.node) == name_key(&node.id) && p.run > 0)
        .map(|p| p.run.to_string())
        .collect();
    let flagged = beatless.iter().find(|line| line.starts_with(&format!("{} ", node.id)));
    let made = node.file.as_ref().and_then(|(_, digest)| lock.made(&node.id, digest));
    // The version project.lock approves; a file attached by hand (or its clean) is none.
    let approved = (lock.node(&node.id).and_then(|n| n.approved.as_ref()))
        .and_then(|file| node.versions.iter().position(|v| v == file))
        .map(|n| n as u32 + 1);
    let assets = match (made, &node.file) {
        (Some(made), _) => made.assets.clone(),
        (None, Some((file, _))) => BTreeMap::from([(video.field(i).to_owned(), file.clone())]),
        (None, None) => BTreeMap::new(),
    };
    let (status, standing) = match (making.as_slice(), flagged, status) {
        ([run], ..) => (format!("making (run {run})"), Standing::Other),
        (runs @ [_, ..], ..) => (format!("making (runs {})", runs.join(", ")), Standing::Other),
        ([], Some(line), _) => (line.clone(), Standing::Other),
        ([], None, Status::Current) if video.unclean(lock, i).is_some() => {
            ("picture not cleaned yet".into(), Standing::Missing)
        }
        ([], None, Status::Empty) if !video.wanted(i) => {
            ("not needed: no resolution, or its shot meets it".into(), Standing::Other)
        }
        ([], None, Status::Empty) => (shown(status, node.noun), Standing::Missing),
        ([], None, Status::Stale(why)) => (shown(status, node.noun), Standing::Stale(why.clone())),
        ([], None, Status::Current) => (shown(status, node.noun), Standing::Other),
    };
    let row = Row {
        name: node.id.clone(),
        kind: node.kind.into(),
        noun: node.noun.into(),
        versions: node.versions.len() as u32,
        approved,
        assets,
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
    let (node, _) = row(&video, &lock, i, &[]);
    let approved = lock.node(&node.name).and_then(|n| n.approved.clone());
    let versions = (lock.node(&node.name).into_iter().flat_map(|n| &n.versions).enumerate())
        .map(|(n, made)| Version {
            number: n as u32 + 1,
            assets: made.assets.clone(),
            made_by: made.made_by.as_ref().map(|by| by.with.clone()),
            runs: made.runs.clone(),
            approved: approved.is_some() && made.primary() == approved.as_ref(),
        })
        .collect();
    Ok(Json(NodeVersions { node, versions }))
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
    let mut takes = vec![];
    if all {
        match beatless(&Host::new(&api), &target.hub, &video, &project.folder.dir, &target.on).await {
            Ok(beatless) if !beatless.is_empty() => {
                return Err(Refusal(
                    Error::new(Kind::Validation, "beats_needed", beatless[0].clone())
                        .remedy("give it beats: [{at, prompt}, …]; nothing was rendered"),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                let message = format!("shot lengths are not checked: {}", error.message);
                let _ = project.events.send(Event::Warning { message });
            }
        }
    }
    for name in asked.names.iter().filter(|_| !all) {
        let node = video.graph.nodes[find(&video, name).map_err(Refusal)?].id.clone();
        let take = || Take { key: new_key(), node: node.clone(), target: target.clone(), approve: asked.approve };
        takes.extend((0..asked.n.max(1)).map(|_| take()));
    }
    (project.folder.update_lock(|lock| {
        if all {
            lock.walk.all = Some(target.clone());
            lock.walk.preview = asked.preview;
        }
        lock.walk.takes.extend(takes.iter().cloned());
    }))
    .map_err(Refusal)?;
    walk(&api, &project);
    Ok((StatusCode::ACCEPTED, Json(Opened { id, path: project.folder.dir.clone() })))
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
    let file = (asked.version.checked_sub(1).and_then(|n| node.versions.get(n as usize))).cloned().ok_or_else(|| {
        let said = format!("{} has {} {}s", node.id, node.versions.len(), node.noun);
        Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy project status"))
    })?;
    let before = video.graph.by_id(&video.graph.status(&lock));
    let key = name_key(&node.id);
    folder.update_lock(|lock| lock.nodes.entry(key.clone()).or_default().approved = Some(file.clone())).map_err(Refusal)?;
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
        Walk { host: Host::new(&api), project: project.clone() }.recut().await.map_err(Refusal)?;
    }
    let assets = lock.made(&node.id, &folder::digest_of(&file)).map(|made| made.assets.clone()).unwrap_or_default();
    let (name, noun) = (node.id.clone(), node.noun.to_owned());
    Ok(Json(Approved { name, noun, number: asked.version, assets, stale, warnings: vec![] }))
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

/// One of the project's assets, by its name under assets/ (`<sha256>.<ext>`, or the hex alone):
/// content-addressed, so its digest is its ETag.
async fn asset(
    State(api): State<Arc<Api>>,
    Path((id, asset)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let hex = asset.split('.').next().unwrap_or_default();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let said = format!("{asset} is no asset: assets are named <sha256>.<ext>");
        return Err(Refusal(Error::new(Kind::Usage, "asset_name", said)));
    }
    let found = project.folder.find(&format!("sha256:{hex}")).map_err(Refusal)?;
    send(&project.folder.path(&found), &format!("\"{hex}\""), &headers)
}

/// One of the project's cuts (`edit.mp4`, `preview.mp4`), as last made: its ETag names that cut.
async fn cut(
    State(api): State<Arc<Api>>,
    Path((id, cut)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let video = Video::load(&project.folder).map_err(Refusal)?;
    if !made_cuts(&video).iter().any(|(name, _)| *name == cut) {
        let made: Vec<&str> = made_cuts(&video).into_iter().map(|(name, _)| name).collect();
        let said = format!("the project makes no cut {cut}: it makes {}", made.join(" and "));
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
    /// A picture to clean before it is listed, its record, and the Hub its clean goes to.
    Clean {
        made: Box<Made>,
        picture: PathBuf,
        approve: bool,
        hub: String,
    },
    /// One of a shot's calls, its assets kept: its write frames to cut, then its next call or the
    /// shot's take.
    Call {
        made: Box<Made>,
        call: Call,
        approve: bool,
        hub: String,
        on: Vec<On>,
    },
}

/// A version to send now: entity `i`'s for `all` (take None), or a take's.
struct Next {
    i: usize,
    take: Option<Take>,
}

/// A project's one walker: what versions.lock owes, each version sent once what it reads is
/// settled, each run followed until it lands.
struct Walk {
    host: Host,
    project: Arc<Project>,
}

impl Walk {
    fn folder(&self) -> &Folder {
        &self.project.folder
    }

    fn say(&self, event: Event) {
        let _ = self.project.events.send(event);
    }

    /// Sends whatever is ready, follows every pending run (what it sent, a picture's clean, what an
    /// earlier walker sent), and plans again as each lands or something is asked, until nothing is
    /// owed: then the first thing that failed, if one did. What `all` asked ends with the edit, cut
    /// once nothing more of it can be made.
    async fn go(&self) -> Result<Option<Error>, Error> {
        for message in Video::load(self.folder())?.warnings {
            self.say(Event::Warning { message });
        }
        let (mut first, mut following, mut followed) = (None, FuturesUnordered::new(), HashSet::new());
        loop {
            let (video, lock) = (Video::load(self.folder())?, self.folder().lock()?);
            for take in lock.walk.takes.iter().filter(|take| video.graph.find(&take.node).is_none()) {
                let said = format!("{} is no longer in {}: its version is not made", take.node, folder::PROJECT);
                self.failed(Error::new(Kind::NotFound, "node_unknown", said), &mut first);
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
                if self.folder().lock()?.walk.all.is_some() {
                    garbage(self.folder())?;
                    if let Err(error) = self.recut().await {
                        self.failed(error, &mut first);
                    }
                    self.folder().update_lock(|lock| {
                        lock.walk.all = None;
                        lock.walk.preview = false;
                        lock.walk.sent.clear();
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
        self.say(Event::Warning { message: error.message.clone() });
        first.get_or_insert(error);
    }

    /// Sends `next` under its key: a new version made `on` its target's machines (chosen for
    /// `all`, or when the entity has none; a picture is cleaned before it is listed), else its
    /// chosen picture's clean. Recorded as pending together with what it settles of the walk, before
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
        let (target, remake, approve) = match &next.take {
            None => (lock.walk.all.clone().expect("all is owed"), *status != Status::Current, true),
            Some(take) => (take.target.clone(), video.unclean(lock, i).is_none(), take.approve),
        };
        if next.take.is_none() {
            let said = if remake { shown(status, video.graph.nodes[i].noun) } else { "cleaning its picture".into() };
            self.say(Event::Said { message: format!("{id}: {said}") });
        }
        let sent = async {
            if let (false, Some((file, digest))) = (remake, video.unclean(lock, i)) {
                let made = Made { node: id.clone(), clean_of: Some(digest.clone()), ..Default::default() };
                let picture = (self.folder().path(&file), digest);
                return self.clean(&key, &target.hub, &video.clean_render(), picture, made, Then::Attached, &settle).await;
            }
            let renderer = (self.host)
                .renderer(&target.hub, video.render(i).target(), &self.folder().dir, &target.on, Install::Wait)
                .await?;
            let request = video.request(self.folder(), lock, i, &renderer)?;
            request.warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
            let then = if video.cleans(i) { Then::Clean { approve } } else { Then::List { approve } };
            let (mut payload, mut files, mut frames) = (request.payload, vec![], vec![]);
            for input in request.files {
                // A frame of a video, taken out here: held until the run has been sent.
                let (path, digest) = match input.at {
                    None => (input.path, input.digest),
                    Some(at) => {
                        let frame = self.frame(&id, &input.path, at).await?;
                        let digest = folder::hash(frame.path())?;
                        set(&mut payload, &input.field, Value::String(digest.clone()));
                        frames.push(frame);
                        (frames.last().expect("a frame").path().to_owned(), digest)
                    }
                };
                files.push((input.field, path, digest));
            }
            let (made, field, call, hub) =
                (request.made.clone(), request.field.into(), request.call.clone(), target.hub.clone());
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
        let video = Video::load(self.folder())?;
        let preview_walk = self.folder().lock()?.walk.preview;
        let cuts = made_cuts(&video).into_iter().filter(|(_, native)| *native || !preview_walk);
        for (name, preview) in cuts {
            let (pieces, skipped) = video.cuts(self.folder(), preview)?;
            if !skipped.is_empty() {
                self.say(Event::Said { message: format!("{name} skips {}: no version yet", skipped.join(", ")) });
            }
            if pieces.is_empty() {
                continue;
            }
            let (ffmpeg, ffprobe) = self.host.ffmpeg().await?;
            let (folder, cutting, began) =
                (Folder::open(&self.folder().dir)?, self.project.cutting.clone(), Instant::now());
            let canvas = Canvas { short: if preview { None } else { video.canvas.short }, ..video.canvas.clone() };
            tokio::task::spawn_blocking(move || {
                let _held = cutting.lock().expect("cutting lock");
                video::cut(&ffmpeg, &ffprobe, &folder, &pieces, &canvas, name)
            })
            .await
            .map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))??;
            self.say(Event::Cut { cut: name.into(), seconds: began.elapsed().as_secs_f64() });
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
            let (made, picture, approve, hub) = match self.land(&key, &run).await? {
                Landed::Done => return Ok(node),
                Landed::Call { made, call, approve, hub, on } => {
                    return self.called(&key, *made, call, approve, &hub, &on).await;
                }
                Landed::Clean { made, picture, approve, hub } => (made, picture, approve, hub),
            };
            let cleaner = Video::load(self.folder())?.clean_render();
            let picture = (picture.clone(), folder::hash(&picture)?);
            let settle = |lock: &mut Lock| drop(lock.pending.remove(&key));
            self.clean(&new_key(), &hub, &cleaner, picture, *made, Then::List { approve }, &settle).await?;
            Ok(None)
        };
        let landed = landed.await;
        if let Err(error) = self.folder().update_lock(|lock| drop(lock.pending.remove(&key))) {
            return (key, Err(error));
        }
        let cut = async {
            let video = Video::load(self.folder())?;
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
        approve: bool,
        hub: &str,
        on: &[On],
    ) -> Result<Option<String>, Error> {
        let folder = self.folder();
        let node = made.node.clone();
        let video_asset = made.assets.get("video").cloned().unwrap_or_default();
        for (state, at) in &call.writes {
            // Half a frame early: a write at the shot's end is its last frame.
            let frame = self.frame(&node, &folder.path(&video_asset), (at - 1.0 / 48.0).max(0.0)).await?;
            made.writes.insert(state.clone(), folder.keep(frame.path())?.0);
        }
        let video = Video::load(folder)?;
        let i = find(&video, &node)?;
        if call.index + 1 < call.ends.len() {
            let next = Call {
                index: call.index + 1,
                ends: call.ends.clone(),
                writes: vec![],
                from: Some((video_asset, made.assets.get("latents").cloned())),
            };
            let lock = folder.lock()?;
            let renderer = self.host.renderer(hub, video.render(i).target(), &folder.dir, on, Install::Wait).await?;
            let request = video.next_call(folder, &lock, i, &renderer, next, &made.writes)?;
            request.warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
            let carried = Made {
                node: node.clone(),
                upstream: made.upstream.clone(),
                writes: made.writes.clone(),
                runs: made.runs.clone(),
                ..Default::default()
            };
            let then = Then::List { approve };
            let field = "video".into();
            let pending = Pending { made: carried, run: 0, hub: hub.into(), field, on: vec![], call: request.call, then };
            let files = request.files.into_iter().map(|input| (input.field, input.path, input.digest)).collect();
            let settle = |lock: &mut Lock| drop(lock.pending.remove(key));
            self.submit(&new_key(), &request.payload, files, &request.render, on, pending, &settle).await?;
            return Ok(None);
        }
        let approve = approve || video.graph.nodes[i].file.is_none();
        self.record(Some(key), &made, approve, video.graph.nodes[i].noun)?;
        Ok(Some(node))
    }

    /// The clean of a picture (its path and digest), keeping the record `made` of what it was made
    /// from: one run under `key` on this computer's machine, landed as `then` says and marked as a
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
        settle: &impl Fn(&mut Lock),
    ) -> Result<(), Error> {
        let made = Made { cleaned: true, assets: Default::default(), writes: Default::default(), ..made };
        let pending = Pending { made, run: 0, hub: hub.into(), field: "image".into(), on: vec![], call: None, then };
        let payload = serde_json::json!({ "image": picture.1 });
        let files = vec![("image".into(), picture.0, picture.1)];
        self.submit(key, &payload, files, cleaner, &[On::Machine("local".into())], pending, settle).await
    }

    /// Sends `payload` (its `files` as field, path and digest) to `render`'s function on one of `on`'s
    /// machines as an ordinary run under `key`, recorded as pending (with `settle`: what it settles of
    /// the walk) in one write before it is sent. The render block's `models` and `lora` are the run's
    /// model choices, as `cozy run --input` takes them, and its `options` join the request; an option
    /// may not set a field the project compiles. The version records them, and the release, as what
    /// made it.
    #[allow(clippy::too_many_arguments)]
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
        let renderer = self.host.renderer(&pending.hub, target, &self.folder().dir, on, Install::Wait).await?;
        let node = pending.made.node.clone();
        let named = |e: Error| Error { message: format!("{node}'s render: {}", e.message), ..e };
        let mut payload = payload.clone();
        let fields = payload.as_object_mut().expect("a request is an object");
        if let Some(field) = render.options.keys().find(|key| matches!(key.as_str(), "resolution" | "aspect_ratio")) {
            let said = format!("{node}'s render options set `{field}`: {field} is set by the project");
            return Err(Error::new(Kind::Validation, "option_project", said).remedy(format!("set {field}: in project.yaml")));
        }
        if let Some(field) = render.options.keys().find(|key| fields.contains_key(*key) || matches!(key.as_str(), "models" | "lora")) {
            let said = format!("{node}'s render options set `{field}`, which the project sends");
            return Err(Error::new(Kind::Validation, "option_compiled", said).remedy("remove it from options"));
        }
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
        let warnings = match choices.dropped.is_empty() {
            true => vec![],
            false => vec![Warning {
                code: "request_fields_ignored".into(),
                message: format!("{node}'s render: ignored {}", choices.dropped.join(", ")),
            }],
        };
        let models = choices::wire(&choices.into_choices()).map_err(named)?;
        let mut inputs = vec![];
        let mut staged = vec![];
        for (field, path, digest) in files {
            let length = std::fs::metadata(&path)
                .map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())))?
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
        let run = match self.host.submit(key, submit).await {
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
    /// listed as its node's newest version in project.lock, approved as its `then` says (or when the
    /// node has nothing else). A run that did not complete is dropped with its reason and never run
    /// again.
    async fn land(&self, key: &str, run: &Run) -> Result<Landed, Error> {
        let folder = self.folder();
        let Some(pending) = folder.lock()?.pending.remove(key) else { return Ok(Landed::Done) };
        let node = pending.made.node.clone();
        if run.status != RunStatus::Completed {
            let why =
                run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |r| r.message.clone());
            let said = format!("{node}'s run {} did not complete: {why}; it is not run again", run.number);
            return Err(
                Error::new(Kind::Unavailable, "version_failed", said).next(format!("cozy run show {}", run.number))
            );
        }
        // A single output is index 0; a list's items count from 1.
        let saved = |name: &str, index: usize| {
            (run.output.iter())
                .find(|o| o.name == name && o.index as usize == index)
                .and_then(|o| o.path.clone())
                .ok_or_else(|| {
                    Error::new(Kind::Unavailable, "version_unsaved", format!("run {} saved no {name}", run.number))
                        .next(format!("cozy run show {}", run.number))
                })
        };
        let picture = saved(&pending.field, 0)?;
        if let Then::Clean { approve } = pending.then {
            // The render itself is never listed: only its clean is.
            return Ok(Landed::Clean { made: Box::new(pending.made), picture, approve, hub: pending.hub });
        }
        let video = Video::load(folder)?;
        let Some(i) = video.graph.find(&node) else {
            folder.update_lock(|lock| drop(lock.pending.remove(key)))?;
            let message = format!("{node} is no longer in {}: run {}'s file is not kept", folder::PROJECT, run.number);
            self.say(Event::Warning { message });
            return Ok(Landed::Done);
        };
        let mut made = pending.made.clone();
        made.runs.push(run.number);
        // Every file the run saved, by its output's name (a list's items as `<name>.<n>`).
        for output in &run.output {
            let Some(path) = output.path.as_ref().filter(|path| path.is_file()) else { continue };
            let name = match output.index {
                0 => output.name.clone(),
                n => format!("{}.{n}", output.name),
            };
            made.assets.insert(name, folder.keep(path)?.0);
        }
        made.assets.insert(pending.field.clone(), folder.keep(&picture)?.0);
        if pending.field == "video" {
            let (_, ffprobe) = self.host.ffmpeg().await?;
            made.size = video::size(&ffprobe, &folder.path(&made.assets[&pending.field]));
        }
        let approve = matches!(pending.then, Then::List { approve: true } | Then::Clean { approve: true });
        if let Some(call) = pending.call.clone() {
            let (hub, on) = (pending.hub.clone(), pending.on.clone());
            return Ok(Landed::Call { made: Box::new(made), call, approve, hub, on });
        }
        let approve = match pending.then {
            Then::List { approve } | Then::Clean { approve } => approve || video.graph.nodes[i].file.is_none(),
            Then::Attached => false,
        };
        self.record(Some(key), &made, approve, video.graph.nodes[i].noun)?;
        Ok(Landed::Done)
    }

    /// Lists `made` as its node's newest version (dropping the pending run under `key` in the same
    /// write), approved when `approve`, and says it landed.
    fn record(&self, key: Option<&str>, made: &Made, approve: bool, noun: &str) -> Result<(), Error> {
        let number = std::cell::Cell::new(0);
        self.folder().update_lock(|lock| {
            if let Some(key) = key {
                lock.pending.remove(key);
            }
            number.set(lock.record(made.clone(), approve));
        })?;
        let (name, assets) = (made.node.clone(), made.assets.clone());
        self.say(Event::Landed { name, noun: noun.into(), number: number.get(), assets });
        Ok(())
    }

    /// The frame of `video` at `at` seconds, as a PNG beside the project's files.
    async fn frame(&self, id: &str, video: &FsPath, at: f64) -> Result<tempfile::NamedTempFile, Error> {
        let (ffmpeg, _) = self.host.ffmpeg().await?;
        let io = |e: std::io::Error| {
            Error::new(Kind::Unavailable, "project_io", format!("a frame of {}: {e}", video.display()))
        };
        let frame = tempfile::Builder::new().suffix(".png").tempfile_in(&self.folder().dir).map_err(io)?;
        let (video_path, out) = (video.to_owned(), frame.path().to_owned());
        let done = tokio::task::spawn_blocking(move || {
            std::process::Command::new(ffmpeg)
                .args(["-y", "-v", "error", "-ss", &format!("{at}"), "-i"])
                .arg(video_path)
                .args(["-frames:v", "1", "-update", "1"])
                .arg(out)
                .output()
        })
        .await
        .map_err(|e| Error::new(Kind::Internal, "frame", e.to_string()))?
        .map_err(io)?;
        if !done.status.success() || std::fs::metadata(frame.path()).map_or(0, |m| m.len()) == 0 {
            let said = String::from_utf8_lossy(&done.stderr);
            let said = format!("{id}'s frame: its shot's video has no frame at {at} s ({})", said.trim());
            return Err(
                Error::new(Kind::Validation, "frame_missing", said).remedy("move `at:` inside the shot's length")
            );
        }
        Ok(frame)
    }
}

/// What can be sent now, in project order. What `all` owes (a version out of date or missing, or
/// a picture's clean) once everything it reads is made, cleaned and settled: nothing upstream,
/// however far, still owed or being made into its chosen file (a take that will not be chosen
/// holds nothing up). A take once nothing upstream is unsettled; the take of a picture not cleaned
/// yet is its clean, one at a time.
fn plan(video: &Video, lock: &Lock) -> Vec<Next> {
    let (graph, walk) = (&video.graph, &lock.walk);
    let status = graph.status(lock);
    let clean = |i: usize| video.unclean(lock, i).is_none();
    let mut unsettled = vec![false; graph.nodes.len()];
    let mut next = vec![];
    for &i in &graph.order {
        let id = name_key(&graph.nodes[i].id);
        let busy = lock.pending.values().any(|pending| {
            let chosen = match pending.then {
                Then::Attached => true,
                Then::List { approve } | Then::Clean { approve } => approve,
            };
            name_key(&pending.made.node) == id && (chosen || graph.nodes[i].file.is_none())
        });
        let owed = walk.all.is_some()
            && video.wanted(i)
            && !(walk.preview && video.is_upscale(i))
            && (status[i] != Status::Current || !clean(i))
            && !walk.sent.contains(&id);
        let up = graph.upstream[i].iter().any(|edge| unsettled[edge.node]);
        unsettled[i] = busy || owed || up;
        if up {
            continue;
        }
        let made = graph.upstream[i].iter().all(|edge| status[edge.node] == Status::Current && clean(edge.node));
        if owed && !busy && made {
            next.push(Next { i, take: None });
        }
        let takes = walk.takes.iter().filter(|take| name_key(&take.node) == id);
        let room = if clean(i) { usize::MAX } else { usize::from(!busy && !owed) };
        next.extend(takes.take(room).map(|take| Next { i, take: Some(take.clone()) }));
    }
    next
}

/// A fresh idempotency key: one key names one run forever.
fn new_key() -> String {
    let mut key = [0u8; 16];
    getrandom::fill(&mut key).expect("the OS has randomness");
    format!("project-{}", hex::encode(key))
}

/// Shots longer than their renderer's pieces with no beats, as lines naming each: each shot's own
/// renderer states its longest piece (`max_piece_s`).
async fn beatless(host: &Host, hub: &str, video: &Video, folder: &FsPath, on: &[On]) -> Result<Vec<String>, Error> {
    let mut lines = vec![];
    for (i, shot, seconds) in video.beatless() {
        let render = video.render(i);
        let target = render.target();
        let renderer = host.renderer(hub, target, folder, on, Install::Never).await?;
        if let Some(max) = renderer.accepts.max_piece_s.filter(|max| seconds > *max) {
            lines.push(format!("{shot} needs beats: {seconds} s, and {target} renders pieces of at most {max} s"));
        }
    }
    Ok(lines)
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

/// Forgets the versions of nodes the project no longer has, and deletes the files nothing names.
fn garbage(folder: &Folder) -> Result<(), Error> {
    let video = Video::load(folder)?;
    folder.collect_garbage(&video.keys(), &video.attached(folder)?)
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
        _ => "application/octet-stream",
    }
}
