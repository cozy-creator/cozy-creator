//! Projects (xs-136): a folder of versioned entities that packages render. Calcifer opens a project
//! by its path, makes each version as an ordinary run through its own submit, lands it in the
//! folder, writes project.yaml through the one merge, and cuts the edit here. Its clients (`cozy
//! run --project`, `cozy project`, the web UI) ask and watch; the runs go on without them.
mod folder;
mod graph;
mod splice;
mod video;

use std::{
    collections::{BTreeSet, HashMap},
    convert::Infallible,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
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
use futures_util::Stream;
use protocol::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status as RunStatus, SubmitRun, Warning},
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    ids::{Digest, RunId},
    interface::{self, Slot},
    machine as v1,
    project::{Approve, Approved, Edit, Entity as Row, Event, Open, Opened, Runs, State as ProjectState},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::broadcast;

use crate::api::{Api, Refusal, parse};
use folder::{Folder, Made, Pending, Then};
use graph::{Status, name_key};
use video::{ALL, Video};

/// The projects opened since Calcifer started, by id; one path is always one project.
#[derive(Default)]
pub struct Projects {
    open: Mutex<HashMap<String, Arc<Project>>>,
}

struct Project {
    folder: Folder,
    events: broadcast::Sender<Event>,
    /// Held while its runs are being made.
    busy: Arc<tokio::sync::Mutex<()>>,
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
    let folder = Folder::open(&dir).map_err(Refusal)?;
    let id = format!("project-{}", &hex::encode(Sha256::digest(dir.to_string_lossy().as_bytes()))[..16]);
    let mut projects = api.projects.open.lock().expect("projects lock");
    projects.entry(id.clone()).or_insert_with(|| {
        let events = broadcast::channel(256).0;
        Arc::new(Project { folder, events, busy: Arc::default(), cutting: Arc::default() })
    });
    Ok(Json(Opened { id, path: dir }))
}

/// Its entities as they stand. Runs that ended while nothing was making them land first.
async fn state(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<ProjectState>, Refusal> {
    let project = api.projects.get(&id)?;
    let busy = project.busy.clone().try_lock_owned();
    if let Ok(_held) = &busy {
        Walk::new(&api, &project, On::Machine("local".into()), None).collect(false).await.map_err(Refusal)?;
    }
    let video = Video::load(&project.folder).map_err(Refusal)?;
    let lock = project.folder.lock().map_err(Refusal)?;
    let status = video.graph.status(&lock);
    let walk = Walk::new(&api, &project, On::Machine("local".into()), None);
    let (beatless, mut warnings) = match walk.beatless(&video).await {
        Ok(beatless) => (beatless, video.warnings.clone()),
        Err(error) => (
            vec![],
            [video.warnings.clone(), vec![format!("shot lengths are not checked: {}", error.message)]].concat(),
        ),
    };
    warnings.dedup();
    let making =
        |id: &str| lock.pending.values().find(|p| name_key(&p.made.entity) == name_key(id)).map(|p| p.made.run);
    let entities = (video.graph.order.iter())
        .map(|&i| {
            let entity = &video.graph.entities[i];
            let file = entity.file.as_ref().map(|(file, _)| file.clone());
            let chosen =
                file.as_ref().and_then(|file| entity.versions.iter().position(|v| v == file)).map(|n| n as u32 + 1);
            let flagged = beatless.iter().find(|line| line.starts_with(&format!("{} ", entity.id)));
            let status = match (making(&entity.id), flagged, &status[i]) {
                (Some(run), ..) => format!("making (run {run})"),
                (None, Some(line), _) => line.clone(),
                (None, None, Status::Current) if video.unclean(&lock, i).is_some() => "picture not cleaned yet".into(),
                (None, None, Status::Empty) if !video.wanted(i) => "not used by the edit (edit_uses: native)".into(),
                (None, None, status) => shown(status),
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
    Ok(Json(ProjectState { id, path: project.folder.dir.clone(), entities, busy: busy.is_err(), edit, warnings }))
}

/// Starts making what `Runs` asks, answered at once; `/events` says how it goes.
async fn runs(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
    let asked: Runs = parse(&body?)?;
    if matches!(asked.on, On::New(_)) {
        let said = "a project's runs go to a machine: rent one first (`cozy machine rent`)";
        return Err(Refusal(Error::new(Kind::Usage, "project_on_new", said)));
    }
    let hub = Origin::parse(&asked.hub).map_err(Refusal)?;
    let project = api.projects.get(&id)?;
    let held = project.busy.clone().try_lock_owned().map_err(|_| {
        Refusal(Error::new(
            Kind::Conflict,
            "project_busy",
            "runs are being made in this project; its events say how they go",
        ))
    })?;
    let walk = Walk::new(&api, &project, asked.on, Some(hub));
    tokio::spawn(async move {
        let _held = held;
        let error = walk.go(&asked.names, asked.n.max(1)).await.err();
        walk.say(Event::Ended { error });
    });
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
    garbage(folder).map_err(Refusal)?;
    if video.cut_reads(i) {
        Walk::new(&api, &project, On::Machine("local".into()), None).recut().await.map_err(Refusal)?;
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

/// A renderer as the Hub has it: its package's newest release, and the longest piece its
/// `video/shot` makes.
#[derive(Clone)]
struct Renderer {
    package: String,
    function: String,
    release: String,
    max_piece_s: Option<f64>,
    /// Its model slots and those of the calls it makes, for a request's `models:` and `lora:`.
    slots: Vec<Slot>,
    calls: Vec<Slot>,
}

/// What landing a run left to do.
enum Landed {
    Done,
    /// A picture to clean before it is listed, and its record.
    Clean {
        made: Made,
        picture: PathBuf,
        choose: bool,
    },
}

/// Makes a project's versions: one asked set of runs, or the collection of runs that ended.
struct Walk {
    api: Arc<Api>,
    project: Arc<Project>,
    on: On,
    hub: Option<Origin>,
    renderers: Mutex<HashMap<String, Renderer>>,
}

impl Walk {
    fn new(api: &Arc<Api>, project: &Arc<Project>, on: On, hub: Option<Origin>) -> Self {
        Self { api: api.clone(), project: project.clone(), on, hub, renderers: Mutex::default() }
    }

    fn folder(&self) -> &Folder {
        &self.project.folder
    }

    fn say(&self, event: Event) {
        let _ = self.project.events.send(event);
    }

    fn load(&self) -> Result<Video, Error> {
        let video = Video::load(self.folder())?;
        for message in &video.warnings {
            self.say(Event::Warning { message: message.clone() });
        }
        Ok(video)
    }

    /// `names`, each `n` times in order (a picture still to be cleaned is cleaned instead), or
    /// `all`: every entity out of date or empty in order, each new version chosen, then the edit.
    async fn go(&self, names: &[String], n: u32) -> Result<(), Error> {
        self.collect(true).await?;
        if let [only] = names
            && name_key(only) == name_key(ALL)
        {
            return self.all().await;
        }
        let video = self.load()?;
        let mut picked = BTreeSet::new();
        for name in names {
            picked.insert(find(&video, name)?);
        }
        let ids: Vec<String> = (video.graph.order.iter())
            .filter(|i| picked.contains(*i))
            .map(|&i| video.graph.entities[i].id.clone())
            .collect();
        for id in ids {
            for _ in 0..n {
                let video = Video::load(self.folder())?;
                let i = find(&video, &id)?;
                let remake = video.unclean(&self.folder().lock()?, i).is_none();
                self.step(&video, i, remake, false).await?;
            }
        }
        garbage(self.folder())
    }

    async fn all(&self) -> Result<(), Error> {
        match self.beatless(&self.load()?).await {
            Ok(beatless) if !beatless.is_empty() => {
                return Err(Error::new(Kind::Validation, "beats_needed", beatless[0].clone())
                    .remedy("give it beats: [{at, prompt}, …]; nothing was rendered"));
            }
            Ok(_) => {}
            Err(error) => {
                self.say(Event::Warning { message: format!("shot lengths are not checked: {}", error.message) })
            }
        }
        // Each entity at most once a step: a new version when it is out of date or empty, else its
        // picture's clean.
        let mut done = BTreeSet::new();
        loop {
            let video = Video::load(self.folder())?;
            let lock = self.folder().lock()?;
            let status = video.graph.status(&lock);
            let next = (video.graph.order.iter().copied()).filter(|&i| video.wanted(i)).find_map(|i| {
                let remake = status[i] != Status::Current;
                let key = (video.graph.entities[i].id.clone(), remake);
                ((remake || video.unclean(&lock, i).is_some()) && !done.contains(&key)).then_some((i, key))
            });
            let Some((i, key)) = next else { break };
            let said = if key.1 { shown(&status[i]) } else { "cleaning its picture".into() };
            self.say(Event::Said { message: format!("{}: {said}", key.0) });
            done.insert(key.clone());
            self.step(&video, i, key.1, true).await?;
        }
        garbage(self.folder())?;
        self.recut().await
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
        let (ffmpeg, ffprobe) = ffmpeg(&self.api.machines.hubs.home().tools())?;
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

    /// One step of entity `i`: with `remake`, a new version made `on` its machine (chosen when
    /// `choose`, or when the entity has none; a picture is cleaned before it is listed), else its
    /// chosen picture's clean.
    async fn step(&self, video: &Video, i: usize, remake: bool, choose: bool) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        let id = &video.graph.entities[i].id;
        if !remake {
            let Some((file, digest)) = video.unclean(&lock, i) else { return Ok(()) };
            let made = lock.made(id, &digest).cloned().unwrap_or(Made { entity: id.clone(), ..Default::default() });
            return self.clean(video.clean_renderer(), (self.folder().path(&file), digest), made, Then::Choose).await;
        }
        let request = video.request(self.folder(), &lock, i)?;
        let then = if video.cleans(i) { Then::Clean { choose } } else { Then::List { choose } };
        let (mut payload, mut files, mut frames) = (request.payload, vec![], vec![]);
        for input in request.files {
            // A frame of a video, taken out here: held until the run has been sent.
            let (path, digest) = match input.at {
                None => (input.path, input.digest),
                Some(at) => {
                    let frame = self.frame(id, &input.path, at).await?;
                    let digest = folder::hash(frame.path())?;
                    set(&mut payload, &input.field, Value::String(digest.clone()));
                    frames.push(frame);
                    (frames.last().expect("a frame").path().to_owned(), digest)
                }
            };
            files.push((input.field, path, digest));
        }
        let pending = Pending { made: request.made, field: request.field.into(), writes: request.writes, then };
        let (run_id, run) = self.send(&payload, files, video.renderer(i), &self.on, pending).await?;
        self.finish(video.clean_renderer(), &run_id, &run).await?;
        match video.cut_reads(i) {
            true => self.recut().await,
            false => Ok(()),
        }
    }

    /// A run's file into the project once the run ended, cleaned first when it is a picture its
    /// entity cleans.
    async fn finish(&self, cleaner: &str, id: &str, run: &Run) -> Result<(), Error> {
        match self.land(id, run)? {
            Landed::Done => Ok(()),
            Landed::Clean { made, picture, choose } => {
                let digest = folder::hash(&picture)?;
                self.clean(cleaner, (picture, digest), made, Then::List { choose }).await
            }
        }
    }

    /// The clean of a picture (its path and digest), keeping the record `made` of what it was made
    /// from: one run on this computer's machine, landed as `then` says and marked as a clean.
    async fn clean(&self, cleaner: &str, picture: (PathBuf, String), made: Made, then: Then) -> Result<(), Error> {
        let made = Made { cleaned: true, outputs: Default::default(), ..made };
        let pending = Pending { made, field: "image".into(), writes: vec![], then };
        let payload = serde_json::json!({ "image": picture.1 });
        let (id, run) = self
            .send(
                &payload,
                vec![("image".into(), picture.0, picture.1)],
                cleaner,
                &On::Machine("local".into()),
                pending,
            )
            .await?;
        self.land(&id, &run).map(drop)
    }

    /// Sends `payload` (its `files` as field, path and digest) to `target` `on` its machine as an
    /// ordinary run, records it as pending, and waits for it to end with its outputs here. Its
    /// `models:` and `lora:` (a shot's `settings:`) are the run's model choices, as `cozy run
    /// --input` takes them.
    async fn send(
        &self,
        payload: &Value,
        files: Vec<(String, PathBuf, String)>,
        target: &str,
        on: &On,
        mut pending: Pending,
    ) -> Result<(String, Run), Error> {
        let renderer = self.renderer(target).await?;
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
        let hub = self.hub.as_ref().map(ToString::to_string).unwrap_or_default();
        let submit = SubmitRun {
            on: on.clone(),
            hub,
            kind: RunKind::Call,
            target: format!("{}/{}@{}", renderer.package, renderer.function, renderer.release),
            spec,
            inputs: staged,
            out: None,
            warnings,
            agreed: None,
        };
        let mut key = [0u8; 16];
        getrandom::fill(&mut key).expect("the OS has randomness");
        let submitted =
            self.api.calcifer.submit(&format!("project-{}", hex::encode(key)), submit).await.map_err(refused)?;
        let id = submitted.run.id.to_string();
        pending.made.run = submitted.run.number;
        let name = pending.made.entity.clone();
        self.folder().update_lock(|lock| {
            lock.pending.insert(id.clone(), pending.clone());
        })?;
        self.say(Event::Started { name, run: id.clone(), number: submitted.run.number });
        Ok((id.clone(), self.wait(&submitted.run.id).await?))
    }

    /// The run once it ended and each output landed here or failed to.
    async fn wait(&self, id: &RunId) -> Result<Run, Error> {
        let mut tail = self.api.calcifer.subscribe();
        loop {
            let run = self.run(id.as_str())?;
            if settled(&run) && !owed(&run) {
                return Ok(run);
            }
            tokio::select! {
                _ = tail.recv() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }

    fn run(&self, id: &str) -> Result<Run, Error> {
        let calcifer = &self.api.calcifer;
        let row = calcifer.store().run(id).map_err(|e| refused(e.into()))?;
        let row =
            row.ok_or_else(|| Error::new(Kind::NotFound, "run_not_found", format!("Calcifer holds no run {id}")))?;
        calcifer.record(&row).map_err(refused)
    }

    /// Lands each pending run that ended; with `wait`, waits for those still going first, and
    /// cleans a landed picture (else its clean waits for a walk).
    async fn collect(&self, wait: bool) -> Result<(), Error> {
        let pending = self.folder().lock()?.pending;
        if pending.is_empty() {
            return Ok(());
        }
        let cleaner = Video::load(self.folder())?.clean_renderer().to_owned();
        let mut cut = false;
        for (id, pending) in pending {
            let run = match self.run(&id) {
                Ok(run) => run,
                Err(error) if error.kind == Kind::NotFound => {
                    self.folder().update_lock(|lock| {
                        lock.pending.remove(&id);
                    })?;
                    self.say(Event::Warning { message: format!("{error}; its version is lost") });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let run = match wait {
                true => self.wait(&run.id).await?,
                false if !settled(&run) || owed(&run) || matches!(pending.then, Then::Clean { .. }) => continue,
                false => run,
            };
            if let Err(error) = self.finish(&cleaner, &id, &run).await {
                self.say(Event::Warning { message: error.message });
            }
            let video = Video::load(self.folder())?;
            cut |= wait && video.graph.find(&pending.made.entity).is_some_and(|i| video.cut_reads(i));
        }
        garbage(self.folder())?;
        match cut {
            true => self.recut().await,
            false => Ok(()),
        }
    }

    /// A pending run's file into the project once the run ended: kept under files/, its record
    /// moved from pending to the file's digest, and placed on its entity as its `then` says. A run
    /// that did not complete is dropped with its reason and never run again.
    fn land(&self, id: &str, run: &Run) -> Result<Landed, Error> {
        let folder = self.folder();
        let Some(pending) = folder.lock()?.pending.remove(id) else { return Ok(Landed::Done) };
        let forget = || {
            folder.update_lock(|lock| {
                lock.pending.remove(id);
            })
        };
        if run.status != RunStatus::Completed {
            forget()?;
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
            forget()?;
            return Ok(Landed::Clean { made: pending.made, picture, choose });
        }
        let (file, digest) = folder.keep(&picture)?;
        let mut made = pending.made.clone();
        for (k, state) in pending.writes.iter().enumerate() {
            made.outputs.insert(state.clone(), folder.keep(&saved("frames", k + 1)?)?.1);
        }
        if let Ok(latents) = saved("latents", 0) {
            made.latents = Some(folder.keep(&latents)?.1);
        }
        folder.update_lock(|lock| {
            lock.pending.remove(id);
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
        let (ffmpeg, _) = ffmpeg(&self.api.machines.hubs.home().tools())?;
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

    /// `target` (`[org/]package/function`) as the Hub has it, read once a walk.
    async fn renderer(&self, target: &str) -> Result<Renderer, Error> {
        if let Some(found) = self.renderers.lock().expect("renderers lock").get(target) {
            return Ok(found.clone());
        }
        let malformed =
            || Error::new(Kind::Validation, "renderer_malformed", format!("{target} is not [org/]package/function"));
        let parts: Vec<&str> = target.split('/').collect();
        let (org, name, function) = match parts[..] {
            [name, function] => (protocol::names::ROOT, name, function),
            [org, name, function] => (org, name, function),
            _ => return Err(malformed()),
        };
        let hub = self.hub.as_ref().map_or_else(|| self.api.machines.hubs.default_origin(), ToString::to_string);
        let client = self.api.machines.hubs.reader(&hub)?;
        let (org, name) = (org.to_ascii_lowercase(), name.to_ascii_lowercase());
        let package = client.get_package_if_changed(&org, &name, None).await.map_err(hub_refusal)?;
        let release = (package.and_then(|(card, _)| card.package.latest_release)).ok_or_else(|| {
            Error::new(Kind::NotFound, "package_unreleased", format!("{org}/{name} has no release to run"))
        })?;
        let card = client.get_package_release(&org, &name, &release).await.map_err(hub_refusal)?;
        let card = serde_json::to_value(&card).expect("a card serializes");
        let entry = interface::entrypoints(&card["package_interface"])
            .find(|entry| interface::name(entry) == Some(function))
            .ok_or_else(|| {
                Error::new(
                    Kind::NotFound,
                    "callable_unknown",
                    format!("{org}/{name}@{release} has no function {function}"),
                )
            })?;
        let implements = &entry["implements"];
        let max_piece_s =
            (implements["interface"] == "video/shot").then(|| implements["max_piece_s"].as_f64()).flatten();
        let read = |slots: Vec<&Value>| slots.into_iter().filter_map(|slot| Slot::read(slot).ok()).collect();
        let slots = read(interface::models(entry).iter().collect());
        let calls = read(interface::call_slots(&card["package_interface"], entry));
        let found =
            Renderer { package: format!("{org}/{name}"), function: function.into(), release, max_piece_s, slots, calls };
        self.renderers.lock().expect("renderers lock").insert(target.into(), found.clone());
        Ok(found)
    }

    /// Shots longer than their renderer's pieces with no beats, as lines naming each.
    async fn beatless(&self, video: &Video) -> Result<Vec<String>, Error> {
        let Some(target) = video.shot_renderer() else { return Ok(vec![]) };
        let Some(max) = self.renderer(target).await?.max_piece_s else { return Ok(vec![]) };
        Ok((video.beatless(max).into_iter())
            .map(|(shot, seconds)| {
                format!("{shot} needs beats: {seconds} s, and {target} renders pieces of at most {max} s")
            })
            .collect())
    }
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

fn settled(run: &Run) -> bool {
    matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled)
}

/// An output still owed to this computer: its last revision neither landed nor failed.
fn owed(run: &Run) -> bool {
    run.output.iter().any(|item| !item.is_checkpoint() && item.path.is_none() && item.export_error.is_none())
}

/// cozy's ffmpeg and ffprobe: the pinned build `cozy` fetched, else PATH's.
fn ffmpeg(tools: &FsPath) -> Result<(PathBuf, PathBuf), Error> {
    let path: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let (found, ffprobe) = platform::tools::ffmpeg(tools, &path).ok_or_else(|| {
        Error::new(Kind::Unavailable, "ffmpeg_unavailable", "the edit is cut with ffmpeg, which this computer lacks")
            .next("cozy doctor")
    })?;
    Ok((found.path, ffprobe))
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

fn refused(error: crate::Error) -> Error {
    Refusal::from(error).0
}

fn hub_refusal(error: hub_client::Error) -> Error {
    error.refusal(|| "cozy login <email>".into())
}
