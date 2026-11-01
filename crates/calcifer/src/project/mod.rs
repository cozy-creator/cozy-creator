//! Projects (xs-136, xs-139): a folder of versioned nodes that packages render. Calcifer opens a
//! project by its path and walks it: each version is an ordinary run, sent through its own submit as
//! soon as what it reads is settled, so independent ones run at once and the machines' schedulers
//! place them. Each lands in files/ and project.lock (people write project.yaml; cozy never does),
//! and the edit is cut here. One walker per project takes every `cozy render` asked of it, and what
//! it owes is kept in project.lock, so a restarted Calcifer goes on with it. Its clients (`cozy render`, `cozy project`, the web UI) ask
//! and watch; the runs go on without them.
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
    extract::{Path, State, rejection::BytesRejection},
    http::{StatusCode, header},
    response::{
        IntoResponse, Response,
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
    project::{Approve, Approved, Edit, Event, Node as Row, Open, Opened, Runs, State as ProjectState},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast};
use tracing::warn;

use crate::api::{Api, Refusal, parse};
use folder::{Call, Folder, Lock, Made, MadeBy, Pending, Take, Target, Then};
use graph::{Status, name_key};
use host::{Host, Install};
use video::{ALL, Canvas, Render, Video};

/// The projects opened since Calcifer started, by id; one path is always one project.
#[derive(Default)]
pub struct Projects {
    open: Mutex<HashMap<String, Arc<Project>>>,
}

struct Project {
    folder: Folder,
    events: broadcast::Sender<Event>,
    /// Whether its one walker is going; it goes until nothing is owed.
    walking: Mutex<bool>,
    /// Tells the walker something was asked.
    asked: Notify,
    /// Held while its edit is cut.
    cutting: Arc<Mutex<()>>,
}

pub fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/projects", post(open))
        .route("/v1/projects/{id}", get(state))
        .route("/v1/projects/{id}/runs", post(runs))
        .route("/v1/projects/{id}/approve", post(approve))
        .route("/v1/projects/{id}/events", get(events))
        .route("/v1/projects/{id}/files/{file}", get(file))
}

impl Projects {
    fn get(&self, id: &str) -> Result<Arc<Project>, Refusal> {
        let open = self.open.lock().expect("projects lock");
        open.get(id)
            .cloned()
            .ok_or_else(|| Refusal(Error::new(Kind::NotFound, "project_not_open", format!("no project {id} is open"))))
    }

    /// The project in `dir` (canonical), opened once; its walk goes on when project.lock owes one.
    fn open(&self, api: &Arc<Api>, dir: &FsPath) -> Result<String, Error> {
        let folder = Folder::open(dir)?;
        let id = format!("project-{}", &hex::encode(Sha256::digest(dir.to_string_lossy().as_bytes()))[..16]);
        let project = (self.open.lock().expect("projects lock").entry(id.clone()))
            .or_insert_with(|| {
                let (events, asked) = (broadcast::channel(256).0, Notify::new());
                Arc::new(Project { folder, events, walking: Mutex::new(false), asked, cutting: Arc::default() })
            })
            .clone();
        if project.folder.lock()?.owes() {
            walk(api, &project);
        }
        Ok(id)
    }
}

/// Walks again each project an earlier Calcifer was walking.
pub fn resume(api: &Arc<Api>) {
    let dirs: Vec<PathBuf> =
        std::fs::read(walks(api)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    for dir in dirs {
        if let Err(error) = api.projects.open(api, &dir) {
            warn!("walking {} again: {}", dir.display(), error.message);
        }
    }
}

/// Where Calcifer keeps the folders of the projects it walks, which its next start walks again.
fn walks(api: &Api) -> PathBuf {
    api.machines.hubs.home().calcifer().join("projects.json")
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

/// Writes the folders of the projects being walked where the next start reads them.
fn remember(api: &Api) {
    let open = api.projects.open.lock().expect("projects lock");
    let dirs: Vec<&PathBuf> =
        (open.values()).filter(|p| *p.walking.lock().expect("walking lock")).map(|p| &p.folder.dir).collect();
    let bytes = serde_json::to_vec(&dirs).expect("paths serialize");
    if let Err(e) = platform::durable::replace(&walks(api), &bytes, 0o644) {
        warn!("keeping the projects being walked: {e}");
    }
}

async fn open(State(api): State<Arc<Api>>, body: Result<Bytes, BytesRejection>) -> Result<Json<Opened>, Refusal> {
    let open: Open = parse(&body?)?;
    if !open.path.is_absolute() {
        return Err(Refusal(Error::new(Kind::Usage, "project_path", "a project is opened by its absolute path")));
    }
    if open.create {
        let project = open.path.join(folder::PROJECT);
        if project.exists() {
            let said = format!("{} is a project already", open.path.display());
            return Err(Refusal(Error::new(Kind::Conflict, "project_exists", said)));
        }
        std::fs::create_dir_all(open.path.join("files"))
            .and_then(|()| {
                platform::durable::replace(&project, video::skeleton().as_bytes(), 0o644).map_err(std::io::Error::other)
            })
            .map_err(|e| {
                Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", open.path.display())))
            })?;
    }
    let dir = std::fs::canonicalize(&open.path).unwrap_or(open.path);
    let id = api.projects.open(&api, &dir).map_err(Refusal)?;
    Ok(Json(Opened { id, path: dir }))
}

/// Its nodes as they stand.
async fn state(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<ProjectState>, Refusal> {
    let project = api.projects.get(&id)?;
    let video = Video::load(&project.folder).map_err(Refusal)?;
    let lock = project.folder.lock().map_err(Refusal)?;
    let status = video.graph.status(&lock);
    let (beatless, mut warnings) = match beatless(&Host::new(&api), "", &video, &project.folder.dir, &[]).await {
        Ok(beatless) => (beatless, video.warnings.clone()),
        Err(error) => (
            vec![],
            [video.warnings.clone(), vec![format!("shot lengths are not checked: {}", error.message)]].concat(),
        ),
    };
    warnings.dedup();
    let making = |id: &str| -> Vec<String> {
        (lock.pending.values())
            .filter(|p| name_key(&p.made.node) == name_key(id) && p.made.run > 0)
            .map(|p| p.made.run.to_string())
            .collect()
    };
    let nodes = (video.graph.order.iter())
        .map(|&i| {
            let node = &video.graph.nodes[i];
            let file = node.file.as_ref().map(|(file, _)| file.clone());
            // The version project.lock approves; a picture attached by hand (or its clean) is none.
            let approved = lock.node(&node.id).and_then(|n| n.approved.as_ref());
            let chosen = approved.and_then(|file| node.versions.iter().position(|v| v == file)).map(|n| n as u32 + 1);
            let flagged = beatless.iter().find(|line| line.starts_with(&format!("{} ", node.id)));
            let made_by = (file.as_ref())
                .and_then(|file| lock.made(&node.id, &folder::digest_of(file)))
                .and_then(|made| made.made_by.as_ref())
                .map(|made_by| made_by.with.clone());
            let status = match (making(&node.id).as_slice(), flagged, &status[i]) {
                ([run], ..) => format!("making (run {run})"),
                (runs @ [_, ..], ..) => format!("making (runs {})", runs.join(", ")),
                ([], Some(line), _) => line.clone(),
                ([], None, Status::Current) if video.unclean(&lock, i).is_some() => "picture not cleaned yet".into(),
                ([], None, Status::Empty) if !video.wanted(i) => "not needed: no resolution, or its shot meets it".into(),
                ([], None, status) => shown(status),
            };
            Row {
                name: node.id.clone(),
                kind: node.kind.into(),
                versions: node.versions.len() as u32,
                file,
                chosen,
                made_by,
                status,
            }
        })
        .collect();
    let path = project.folder.dir.join("edit.mp4");
    let cut_at_ms = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .map(|at| at.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64));
    let skips = match video.cuts(&project.folder, false) {
        Ok((_, skips)) => skips,
        Err(error) => {
            warnings.push(error.message);
            vec![]
        }
    };
    let edit = Edit { path, cut_at_ms, skips };
    let busy = *project.walking.lock().expect("walking lock");
    Ok(Json(ProjectState { id, path: project.folder.dir.clone(), nodes, busy, edit, warnings }))
}

/// Adds what `Runs` asks to the project's walk, answered at once; `/events` says how it goes.
async fn runs(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
    let asked: Runs = parse(&body?)?;
    if asked.on.is_empty() || asked.on.iter().any(|on| matches!(on, On::New(_))) {
        let said = "a project's runs go to machines: rent one first (`cozy machine rent`)";
        return Err(Refusal(Error::new(Kind::Usage, "project_on_new", said)));
    }
    Origin::parse(&asked.hub).map_err(Refusal)?;
    let project = api.projects.get(&id)?;
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
        let take = || Take { key: new_key(), node: node.clone(), target: target.clone() };
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
    let project = api.projects.get(&id)?;
    let folder = &project.folder;
    let (video, lock) = (Video::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&video, &asked.name).map_err(Refusal)?;
    let node = &video.graph.nodes[i];
    let file = (asked.version.checked_sub(1).and_then(|n| node.versions.get(n as usize))).cloned().ok_or_else(|| {
        Refusal(Error::new(Kind::Usage, "version_unknown", format!("{} has {} versions", node.id, node.versions.len())))
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
    Ok(Json(Approved { file, stale, warnings: vec![] }))
}

async fn events(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, Refusal> {
    let tail = api.projects.get(&id)?.events.subscribe();
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

/// One of the project's files by its digest's hex (`files/<hex>.<ext>`).
async fn file(State(api): State<Arc<Api>>, Path((id, file)): Path<(String, String)>) -> Result<Response, Refusal> {
    let project = api.projects.get(&id)?;
    let hex = file.split('.').next().unwrap_or_default();
    let found = project.folder.find(&format!("sha256:{hex}")).map_err(Refusal)?;
    let path = project.folder.path(&found);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display()))))?;
    Ok(([(header::CONTENT_TYPE, media_type(&path))], bytes).into_response())
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
    /// One of a shot's calls, its video kept: its write frames to cut, then its next call or the
    /// shot's version.
    Call {
        made: Box<Made>,
        call: Call,
        latents: Option<String>,
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
            Some(take) => (take.target.clone(), video.unclean(lock, i).is_none(), false),
        };
        if next.take.is_none() {
            let said = if remake { shown(status) } else { "cleaning its picture".into() };
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
            let pending = Pending { made, hub, field, on: vec![], call, then };
            self.submit(&key, &payload, files, &request.render, &target.on, pending, &settle).await
        };
        let sent = sent.await;
        if sent.is_err() {
            self.folder().update_lock(settle)?;
        }
        sent
    }

    /// The cuts again here from the approved files (fast join: only what changed is encoded), each
    /// filling the project's canvas: with `resolution:`, `preview.mp4` from the shots and, unless a
    /// preview walk is going, `edit.mp4` from their upscales; without it, `edit.mp4` from the shots.
    /// What has no file yet is skipped, and said.
    async fn recut(&self) -> Result<(), Error> {
        let video = Video::load(self.folder())?;
        let mut cuts = vec![];
        if video.canvas.short.is_some() {
            cuts.push(("preview.mp4", true));
        }
        if video.canvas.short.is_none() || !self.folder().lock()?.walk.preview {
            cuts.push(("edit.mp4", video.canvas.short.is_none()));
        }
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
            let path = tokio::task::spawn_blocking(move || {
                let _held = cutting.lock().expect("cutting lock");
                video::cut(&ffmpeg, &ffprobe, &folder, &pieces, &canvas, name)
            })
            .await
            .map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))??;
            self.say(Event::Cut { path, seconds: began.elapsed().as_secs_f64() });
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
                Landed::Call { made, call, latents, approve, hub, on } => {
                    return self.called(&key, *made, call, latents, approve, &hub, &on).await;
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

    /// A shot's call that landed (its video kept as `made.file`): the frames its writes take from
    /// it, cut here; then its next call, which continues it, or (its last) the shot's version, with
    /// its latents and every call's run. What the walk follows is pending under the next call's key in
    /// the same write that drops this one's.
    #[allow(clippy::too_many_arguments)]
    async fn called(
        &self,
        key: &str,
        mut made: Made,
        call: Call,
        latents: Option<String>,
        approve: bool,
        hub: &str,
        on: &[On],
    ) -> Result<Option<String>, Error> {
        let folder = self.folder();
        let node = made.node.clone();
        for (state, at) in &call.writes {
            // Half a frame early: a write at the shot's end is its last frame.
            let frame = self.frame(&node, &folder.path(&made.file), (at - 1.0 / 48.0).max(0.0)).await?;
            made.outputs.insert(state.clone(), folder.keep(frame.path())?.1);
        }
        let video = Video::load(folder)?;
        let i = find(&video, &node)?;
        if call.index + 1 < call.ends.len() {
            made.runs.push(made.run);
            let next = Call {
                index: call.index + 1,
                ends: call.ends.clone(),
                writes: vec![],
                from: Some((made.file.clone(), latents)),
            };
            let lock = folder.lock()?;
            let renderer = self.host.renderer(hub, video.render(i).target(), &folder.dir, on, Install::Wait).await?;
            let own = (made.outputs.iter())
                .map(|(state, digest)| Ok((state.clone(), folder.find(digest)?)))
                .collect::<Result<BTreeMap<_, _>, Error>>()?;
            let request = video.next_call(folder, &lock, i, &renderer, next, &own)?;
            request.warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone() }));
            let carried = Made {
                node: node.clone(),
                upstream: made.upstream.clone(),
                outputs: made.outputs.clone(),
                runs: made.runs.clone(),
                ..Default::default()
            };
            let then = Then::List { approve };
            let pending =
                Pending { made: carried, hub: hub.into(), field: "video".into(), on: vec![], call: request.call, then };
            let files = request.files.into_iter().map(|input| (input.field, input.path, input.digest)).collect();
            let settle = |lock: &mut Lock| drop(lock.pending.remove(key));
            self.submit(&new_key(), &request.payload, files, &request.render, on, pending, &settle).await?;
            return Ok(None);
        }
        made.latents = latents;
        let approve = approve || video.graph.nodes[i].file.is_none();
        folder.update_lock(|lock| {
            lock.pending.remove(key);
            lock.record(made.clone(), approve);
        })?;
        self.say(Event::Landed { name: node.clone(), file: made.file.clone() });
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
        let made = Made { cleaned: true, outputs: Default::default(), ..made };
        let pending = Pending { made, hub: hub.into(), field: "image".into(), on: vec![], call: None, then };
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
                pending.made.run = run.number;
            }
        })?;
        let name = pending.made.node.clone();
        self.say(Event::Started { name, run: run.id.to_string(), number: run.number });
        Ok(())
    }

    /// The run sent under `key`, once it ended, into the project: its file kept under files/ and
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
        let (file, _) = folder.keep(&picture)?;
        let mut made = Made { file: file.clone(), run: run.number, ..pending.made.clone() };
        if pending.field == "video" {
            let (_, ffprobe) = self.host.ffmpeg().await?;
            made.size = video::size(&ffprobe, &folder.path(&file));
        }
        let approve = matches!(pending.then, Then::List { approve: true } | Then::Clean { approve: true });
        if let Some(call) = pending.call.clone() {
            let latents = saved("latents", 0).ok().map(|path| folder.keep(&path)).transpose()?.map(|(file, _)| file);
            let (hub, on) = (pending.hub.clone(), pending.on.clone());
            return Ok(Landed::Call { made: Box::new(made), call, latents, approve, hub, on });
        }
        let approve = match pending.then {
            Then::List { approve } | Then::Clean { approve } => approve || video.graph.nodes[i].file.is_none(),
            Then::Attached => false,
        };
        folder.update_lock(|lock| {
            lock.pending.remove(key);
            lock.record(made.clone(), approve);
        })?;
        self.say(Event::Landed { name: node, file });
        Ok(Landed::Done)
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

fn shown(status: &Status) -> String {
    match status {
        Status::Current => "current".into(),
        Status::Empty => "no version yet".into(),
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
