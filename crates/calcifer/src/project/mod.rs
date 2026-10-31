//! Projects (xs-136): a folder of versioned entities that packages render. Calcifer opens a project
//! by its path and walks it: each version is an ordinary run, sent through its own submit as soon
//! as what it reads is settled, so independent ones run at once and the machines' schedulers place
//! them. Each lands in the folder through the one merge, and the edit is cut here. One walker per
//! project takes every `cozy render` asked of it, and what it owes is kept in versions.lock, so a
//! restarted Calcifer goes on with it. Its clients (`cozy render`, `cozy project`, the web UI) ask
//! and watch; the runs go on without them.
mod folder;
mod graph;
mod host;
mod splice;
mod video;

use std::{
    collections::{HashMap, HashSet},
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
    project::{Approve, Approved, Edit, Entity as Row, Event, Open, Opened, Runs, State as ProjectState},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast};
use tracing::warn;

use crate::api::{Api, Refusal, parse};
use folder::{Folder, Lock, Made, Pending, Take, Target, Then};
use graph::{Status, name_key};
use host::Host;
use video::{ALL, Video};

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

    /// The project in `dir` (canonical), opened once; its walk goes on when versions.lock owes one.
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

/// Its entities as they stand.
async fn state(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<ProjectState>, Refusal> {
    let project = api.projects.get(&id)?;
    let video = Video::load(&project.folder).map_err(Refusal)?;
    let lock = project.folder.lock().map_err(Refusal)?;
    let status = video.graph.status(&lock);
    let (beatless, mut warnings) = match beatless(&Host::new(&api), "", &video).await {
        Ok(beatless) => (beatless, video.warnings.clone()),
        Err(error) => (
            vec![],
            [video.warnings.clone(), vec![format!("shot lengths are not checked: {}", error.message)]].concat(),
        ),
    };
    warnings.dedup();
    let making = |id: &str| -> Vec<String> {
        (lock.pending.values())
            .filter(|p| name_key(&p.made.entity) == name_key(id) && p.made.run > 0)
            .map(|p| p.made.run.to_string())
            .collect()
    };
    let entities = (video.graph.order.iter())
        .map(|&i| {
            let entity = &video.graph.entities[i];
            let file = entity.file.as_ref().map(|(file, _)| file.clone());
            let chosen =
                file.as_ref().and_then(|file| entity.versions.iter().position(|v| v == file)).map(|n| n as u32 + 1);
            let flagged = beatless.iter().find(|line| line.starts_with(&format!("{} ", entity.id)));
            let status = match (making(&entity.id).as_slice(), flagged, &status[i]) {
                ([run], ..) => format!("making (run {run})"),
                (runs @ [_, ..], ..) => format!("making (runs {})", runs.join(", ")),
                ([], Some(line), _) => line.clone(),
                ([], None, Status::Current) if video.unclean(&lock, i).is_some() => "picture not cleaned yet".into(),
                ([], None, Status::Empty) if !video.wanted(i) => "not used by the edit (edit_uses: native)".into(),
                ([], None, status) => shown(status),
            };
            Row {
                name: entity.id.clone(),
                kind: entity.kind.into(),
                versions: entity.versions.len() as u32,
                file,
                chosen,
                status,
            }
        })
        .collect();
    let path = project.folder.dir.join("edit.mp4");
    let cut_at_ms = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .map(|at| at.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64));
    let skips = match video.cuts(&project.folder) {
        Ok((_, skips)) => skips,
        Err(error) => {
            warnings.push(error.message);
            vec![]
        }
    };
    let edit = Edit { path, cut_at_ms, skips };
    let busy = *project.walking.lock().expect("walking lock");
    Ok(Json(ProjectState { id, path: project.folder.dir.clone(), entities, busy, edit, warnings }))
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
        match beatless(&Host::new(&api), &target.hub, &video).await {
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
        let entity = video.graph.entities[find(&video, name).map_err(Refusal)?].id.clone();
        let take = || Take { key: new_key(), entity: entity.clone(), target: target.clone() };
        takes.extend((0..asked.n.max(1)).map(|_| take()));
    }
    (project.folder.update_lock(|lock| {
        if all {
            lock.walk.all = Some(target.clone());
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
    let entity = &video.graph.entities[i];
    let file =
        (asked.version.checked_sub(1).and_then(|n| entity.versions.get(n as usize))).cloned().ok_or_else(|| {
            Refusal(Error::new(
                Kind::Usage,
                "version_unknown",
                format!("{} has {} versions", entity.id, entity.versions.len()),
            ))
        })?;
    let before = video.graph.by_id(&video.graph.status(&lock));
    let field = video.field(i);
    let warnings =
        folder.edit(video::locate, &entity.id, |map| folder::put(map, field, folder::text(&file))).map_err(Refusal)?;
    let after = Video::load(folder).map_err(Refusal)?;
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
    Ok(Json(Approved { file, stale, warnings: warnings.unwrap_or_default() }))
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
        made: Made,
        picture: PathBuf,
        choose: bool,
        hub: String,
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
            for take in lock.walk.takes.iter().filter(|take| video.graph.find(&take.entity).is_none()) {
                let said = format!("{} is no longer in {}: its version is not made", take.entity, folder::PROJECT);
                self.failed(Error::new(Kind::NotFound, "entity_unknown", said), &mut first);
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
        let id = video.graph.entities[i].id.clone();
        let key = next.take.as_ref().map_or_else(new_key, |take| take.key.clone());
        let settle = |lock: &mut Lock| match &next.take {
            None => {
                lock.walk.sent.insert(name_key(&id));
            }
            Some(_) => lock.walk.takes.retain(|take| take.key != key),
        };
        let status = &video.graph.status(lock)[i];
        let (target, remake, choose) = match &next.take {
            None => (lock.walk.all.clone().expect("all is owed"), *status != Status::Current, true),
            Some(take) => (take.target.clone(), video.unclean(lock, i).is_none(), false),
        };
        if next.take.is_none() {
            let said = if remake { shown(status) } else { "cleaning its picture".into() };
            self.say(Event::Said { message: format!("{id}: {said}") });
        }
        let sent = async {
            if let (false, Some((file, digest))) = (remake, video.unclean(lock, i)) {
                let made =
                    lock.made(&id, &digest).cloned().unwrap_or(Made { entity: id.clone(), ..Default::default() });
                let picture = (self.folder().path(&file), digest);
                return self
                    .clean(&key, &target.hub, video.clean_renderer(), picture, made, Then::Choose, &settle)
                    .await;
            }
            let request = video.request(self.folder(), lock, i)?;
            let then = if video.cleans(i) { Then::Clean { choose } } else { Then::List { choose } };
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
            let (made, field, writes, hub) = (request.made, request.field.into(), request.writes, target.hub.clone());
            let pending = Pending { made, hub, field, writes, then };
            self.submit(&key, &payload, files, video.renderer(i), &target.on, pending, &settle).await
        };
        let sent = sent.await;
        if sent.is_err() {
            self.folder().update_lock(settle)?;
        }
        sent
    }

    /// The edit cut again here from the chosen shots' files (fast join: only what changed is
    /// encoded); shots with no file yet are skipped, and said.
    async fn recut(&self) -> Result<(), Error> {
        let video = Video::load(self.folder())?;
        let (pieces, skipped) = video.cuts(self.folder())?;
        if !skipped.is_empty() {
            self.say(Event::Said { message: format!("the edit skips {}: no version yet", skipped.join(", ")) });
        }
        if pieces.is_empty() {
            return Ok(());
        }
        let (ffmpeg, ffprobe) = self.host.ffmpeg()?;
        let (folder, cutting, began) =
            (Folder::open(&self.folder().dir)?, self.project.cutting.clone(), Instant::now());
        let path = tokio::task::spawn_blocking(move || {
            let _held = cutting.lock().expect("cutting lock");
            video::cut(&ffmpeg, &ffprobe, &folder, &pieces)
        })
        .await
        .map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))??;
        self.say(Event::Cut { path, seconds: began.elapsed().as_secs_f64() });
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
            let entity = self.folder().lock()?.pending.get(&key).map(|pending| pending.made.entity.clone());
            let Landed::Clean { made, picture, choose, hub } = self.land(&key, &run)? else { return Ok(entity) };
            let cleaner = Video::load(self.folder())?.clean_renderer().to_owned();
            let picture = (picture.clone(), folder::hash(&picture)?);
            let settle = |lock: &mut Lock| drop(lock.pending.remove(&key));
            self.clean(&new_key(), &hub, &cleaner, picture, made, Then::List { choose }, &settle).await?;
            Ok(None)
        };
        let landed = landed.await;
        if let Err(error) = self.folder().update_lock(|lock| drop(lock.pending.remove(&key))) {
            return (key, Err(error));
        }
        let cut = async {
            let video = Video::load(self.folder())?;
            match landed?.and_then(|entity| video.graph.find(&entity)).is_some_and(|i| video.cut_reads(i)) {
                true => self.recut().await,
                false => Ok(()),
            }
        };
        let cut = cut.await;
        (key, Ok(cut))
    }

    /// The clean of a picture (its path and digest), keeping the record `made` of what it was made
    /// from: one run under `key` on this computer's machine, landed as `then` says and marked as a
    /// clean.
    #[allow(clippy::too_many_arguments)]
    async fn clean(
        &self,
        key: &str,
        hub: &str,
        cleaner: &str,
        picture: (PathBuf, String),
        made: Made,
        then: Then,
        settle: &impl Fn(&mut Lock),
    ) -> Result<(), Error> {
        let made = Made { cleaned: true, outputs: Default::default(), ..made };
        let pending = Pending { made, hub: hub.into(), field: "image".into(), writes: vec![], then };
        let payload = serde_json::json!({ "image": picture.1 });
        let files = vec![("image".into(), picture.0, picture.1)];
        self.submit(key, &payload, files, cleaner, &[On::Machine("local".into())], pending, settle).await
    }

    /// Sends `payload` (its `files` as field, path and digest) to `target` on one of `on`'s machines
    /// as an ordinary run under `key`, recorded as pending (with `settle`: what it settles of the
    /// walk) in one write before it is sent. Its `models:` and `lora:` (a shot's `settings:`) are the run's
    /// model choices, as `cozy run --input` takes them.
    #[allow(clippy::too_many_arguments)]
    async fn submit(
        &self,
        key: &str,
        payload: &Value,
        files: Vec<(String, PathBuf, String)>,
        target: &str,
        on: &[On],
        pending: Pending,
        settle: &impl Fn(&mut Lock),
    ) -> Result<(), Error> {
        let renderer = self.host.renderer(&pending.hub, target).await?;
        let mut payload = payload.clone();
        let mut choices = Choices::new(target, &renderer.function, &renderer.slots, renderer.calls.iter().collect());
        let entity = &pending.made.entity;
        let named = |e: Error| Error { message: format!("{entity}'s settings: {}", e.message), ..e };
        payload.as_object_mut().map_or(Ok(()), |fields| choices.take(fields)).map_err(named)?;
        let warnings = match choices.dropped.is_empty() {
            true => vec![],
            false => vec![Warning {
                code: "request_fields_ignored".into(),
                message: format!("{entity}'s settings: ignored {}", choices.dropped.join(", ")),
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
        let spec = v1::RunSpec {
            kind: v1::RunKind::Call,
            source: Some(v1::Source::Release(v1::Release {
                package: renderer.package.clone(),
                release: renderer.release.clone(),
            })),
            entrypoint: renderer.function.clone(),
            payload: Some(v1::Json::parse(payload.to_string()).expect("a payload is JSON")),
            inputs,
            models,
            ..Default::default()
        };
        let submit = SubmitRun {
            on: self.host.place(on, &renderer),
            hub: pending.hub.clone(),
            kind: RunKind::Call,
            target: format!("{}/{}@{}", renderer.package, renderer.function, renderer.release),
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
        let name = pending.made.entity.clone();
        self.say(Event::Started { name, run: run.id.to_string(), number: run.number });
        Ok(())
    }

    /// The run sent under `key`, once it ended, into the project: its file kept under files/, its
    /// record moved from pending to the file's digest, and placed on its entity as its `then` says.
    /// A run that did not complete is dropped with its reason and never run again.
    fn land(&self, key: &str, run: &Run) -> Result<Landed, Error> {
        let folder = self.folder();
        let Some(pending) = folder.lock()?.pending.remove(key) else { return Ok(Landed::Done) };
        if run.status != RunStatus::Completed {
            let why =
                run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |r| r.message.clone());
            let entity = &pending.made.entity;
            let said = format!("{entity}'s run {} did not complete: {why}; it is not run again", run.number);
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
        if let Then::Clean { choose } = pending.then {
            // The render itself is never listed: only its clean is.
            return Ok(Landed::Clean { made: pending.made, picture, choose, hub: pending.hub });
        }
        let (file, digest) = folder.keep(&picture)?;
        let mut made = pending.made.clone();
        for (k, state) in pending.writes.iter().enumerate() {
            made.outputs.insert(state.clone(), folder.keep(&saved("frames", k + 1)?)?.1);
        }
        folder.update_lock(|lock| {
            lock.pending.remove(key);
            lock.record(digest.clone(), made.clone());
        })?;
        let (entity, field, then) = (&made.entity, &pending.field, pending.then);
        let placed = folder.edit(video::locate, entity, |map| match then {
            Then::Choose => folder::put(map, field, folder::text(&file)),
            Then::List { choose } | Then::Clean { choose } => video::add_version(map, field, &file, choose),
        })?;
        match placed {
            Some(warnings) => {
                warnings.into_iter().for_each(|message| self.say(Event::Warning { message }));
                self.say(Event::Landed { name: entity.clone(), file });
            }
            None => self.say(Event::Warning {
                message: format!("{entity} is no longer in {}: {file} is not placed", folder::PROJECT),
            }),
        }
        Ok(Landed::Done)
    }

    /// The frame of `video` at `at` seconds, as a PNG beside the project's files.
    async fn frame(&self, id: &str, video: &FsPath, at: f64) -> Result<tempfile::NamedTempFile, Error> {
        let (ffmpeg, _) = self.host.ffmpeg()?;
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
/// however far, still owed or being made. A take once nothing upstream is still owed or being
/// made; the take of a picture not cleaned yet is its clean, one at a time.
fn plan(video: &Video, lock: &Lock) -> Vec<Next> {
    let (graph, walk) = (&video.graph, &lock.walk);
    let status = graph.status(lock);
    let clean = |i: usize| video.unclean(lock, i).is_none();
    let mut unsettled = vec![false; graph.entities.len()];
    let mut next = vec![];
    for &i in &graph.order {
        let id = name_key(&graph.entities[i].id);
        let busy = lock.pending.values().any(|pending| name_key(&pending.made.entity) == id);
        let owed = walk.all.is_some()
            && video.wanted(i)
            && (status[i] != Status::Current || !clean(i))
            && !walk.sent.contains(&id);
        let up = graph.upstream[i].iter().any(|edge| unsettled[edge.entity]);
        unsettled[i] = busy || owed || up;
        if up {
            continue;
        }
        let made = graph.upstream[i].iter().all(|edge| status[edge.entity] == Status::Current && clean(edge.entity));
        if owed && !busy && made {
            next.push(Next { i, take: None });
        }
        let takes = walk.takes.iter().filter(|take| name_key(&take.entity) == id);
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

/// Shots longer than their renderer's pieces with no beats, as lines naming each.
async fn beatless(host: &Host, hub: &str, video: &Video) -> Result<Vec<String>, Error> {
    let Some(target) = video.shot_renderer() else { return Ok(vec![]) };
    let Some(max) = host.renderer(hub, target).await?.max_piece_s else { return Ok(vec![]) };
    Ok((video.beatless(max).into_iter())
        .map(|(shot, seconds)| {
            format!("{shot} needs beats: {seconds} s, and {target} renders pieces of at most {max} s")
        })
        .collect())
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

/// Deletes the files and records no entity lists any more.
fn garbage(folder: &Folder) -> Result<(), Error> {
    folder.collect_garbage(&Video::load(folder)?.listed(folder)?)
}

fn find(video: &Video, name: &str) -> Result<usize, Error> {
    video.graph.find(name).ok_or_else(|| {
        Error::new(Kind::NotFound, "entity_unknown", format!("the project has nothing named {name}"))
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
