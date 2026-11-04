//! Projects (xs-151, cl-758): a folder of assets, each a stable name holding a set of files. Calcifer opens a project by its path
//! (and keeps every folder it opened in its index) and walks it: an operation makes an asset only when it has no state, and is an
//! ordinary run, sent as soon as what it reads has settled, so independent ones run at once and the machines' schedulers place
//! them. Its package's adapter for its kind turns it into calls, and they land as one version of the asset, in `files/` and
//! project.lock (people write project.yaml; cozy writes one line of it at `approve`, `unpin`, `import` and `rename`). The reserved package `cozy` runs
//! here, free. One walker per project takes every render asked of it, and what it owes is kept in
//! `.runs/walk.json`, so a restarted Calcifer goes on with it. Its clients ask and watch; the runs go on without them.
mod archive;
mod assetfile;
mod assets;
mod canvas;
mod folder;
mod format;
mod graph;
pub(crate) mod host;
mod jobs;
mod remote;
mod thumbs;
mod caption;
mod direct;
mod tools;
mod walker;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    convert::Infallible,
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse as _, Response,
        sse::{self, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::{FutureExt as _, Stream, StreamExt as _, future::BoxFuture, stream::FuturesUnordered};
use protocol::{
    tool::EDIT,
    calcifer::{FileTerm, Kind as RunKind, On, Run, RunRequest, StagedInput, Status as RunStatus, SubmitRun, Warning},
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    project::{
        Approve, Approved, Asked, Asset as Row, Cancel, Cancelled, Cut, Event, File as Listing, Import, Imported, Job, JobState, Kept, Known, MainFile, Open, Opened,
        Orphan, Projects as Listed, Queued, QueuedNow, Reclaim, Reclaimed, Rename, Renamed, Rendered, Runs, Stale, Standing as RowStanding, State as ProjectState, Unpin, Unpinned,
        VersionPrefix, Warning as ProjectWarning,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast};
use tracing::warn;

use crate::api::{Api, Refusal, parse};
use folder::{Ask, Failed, Files, Folder, Invocation, Keeps, Lock, MadeBy, Pending, ReadFrom, Take, Target, Tried, Version};
use format::{ALL, Doc, Standing, State as Stand, Tool};
use graph::name_key;
use host::{Host, Maker, Package, Renderer};
use jobs::{Going, Jobs};
use walker::{End, Frozen, Graph, Kind as JobKind, Named, Node, Turn};
use tools::Canvas;

/// The projects this Calcifer knows: those open since it started, by id (one folder is always one
/// project), and its index of every folder it opened, kept in `projects.json`.
#[derive(Default)]
pub struct Projects {
    open: Mutex<HashMap<String, Arc<Project>>>,
    /// Read on first use.
    index: Mutex<Option<Index>>,
    /// What was last heard of each published package projects name, by `with`.
    heard: Mutex<HashMap<String, Heard>>,
    /// Each project made by name since Calcifer started, by the `Idempotency-Key` that asked: a retry makes no `_2`.
    made: Mutex<HashMap<String, PathBuf>>,
}

/// A package as a project's read knows it: when it was last asked of, and what that found.
#[derive(Default)]
struct Heard {
    asked: Option<Instant>,
    package: Option<Package>,
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
    /// What its render is doing, asset by asset.
    jobs: Jobs,
    /// What its walker needn't read again: the jobs, and each text.
    walker: Mutex<Walker>,
    /// Whether its one walker is going; it goes until nothing is owed.
    walking: Mutex<bool>,
    /// Tells the walker something was asked.
    asked: Notify,
    /// Held while a cut is made.
    cutting: Arc<Mutex<()>>,
    /// How far each Edit being rendered here is, 0 to 1.
    rendering: Arc<Mutex<BTreeMap<String, f64>>>,
    /// Held while its files move to or from its Hub.
    moving: tokio::sync::Mutex<()>,
    /// Held while an import chooses a name and writes it.
    importing: Mutex<()>,
    /// Each `Idempotency-Key` a render was asked under since Calcifer started, and the ask's digest.
    keys: Mutex<HashMap<String, String>>,
}

pub fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/projects", post(open).get(list))
        .route("/v1/projects/{id}", get(state).delete(forget))
        .route("/v1/projects/{id}/imports", post(import))
        .route("/v1/projects/{id}/assets/{name}/caption", post(caption::caption))
        .route("/v1/projects/{id}/canvas", get(canvas::read).patch(canvas::patch))
        .route("/v1/projects/{id}/thumbs/{sha256}", get(thumbs::thumb))
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
        .route("/v1/projects/{id}/assets", post(assets::create))
        .route("/v1/projects/{id}/assets/{name}", get(assets::read).patch(assets::patch).delete(assets::delete).put(assets::put_back))
        .route("/v1/projects/{id}/files/{file}", get(file))
        .route("/v1/projects/{id}/cuts/{cut}", get(cut))
        .route("/v1/projects/{id}/archive", get(archive::archive))
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

    /// The project in `dir` (canonical), opened once; its walk goes on when one is owed.
    fn open(&self, api: &Arc<Api>, dir: &FsPath) -> Result<String, Error> {
        let folder = Folder::open(dir, api.machines.hubs.home())?;
        crate::events::project_opened(dir);
        let id = project_id(dir);
        let project = (self.open.lock().expect("projects lock").entry(id.clone()))
            .or_insert_with(|| {
                let (events, asked) = (broadcast::channel::<Event>(256).0, Notify::new());
                Arc::new(Project {
                    folder,
                    jobs: Jobs::new(id.clone(), events.clone()),
                    events,
                    walker: Mutex::default(),
                    walking: Mutex::new(false),
                    asked,
                    cutting: Arc::default(),
                    rendering: Arc::default(),
                    moving: tokio::sync::Mutex::default(),
                    importing: Mutex::default(),
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

/// A run that a person canceled: no version, and no failure of its asset.
const CANCELED: &str = "version_canceled";
/// A picture at least this share transparent has its background gone already.
const OPAQUE_BELOW: f64 = 0.05;
/// How long what an import under an `Idempotency-Key` answered is kept for its retries: as long as a held file waits.
const IMPORT_KEPT_MS: i64 = 24 * 60 * 60 * 1000;
/// How long what was heard of a package stands before a read asks again, in the background.
const HEARD_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The words a run hears outside a project, as `git` says it of a repository.
pub(crate) fn not_in_one() -> Error {
    Error::new(Kind::Usage, "not_in_a_project", "not in a project: cd into one, or make one")
        .next("cozy project ls")
        .next("cozy project new my-project && cd my-project")
}

/// Where a run of the project in `dir` lands its outputs.
pub(crate) fn landing(dir: &FsPath) -> PathBuf {
    dir.join(folder::RUNS)
}

/// The project a run's landing folder is in, if it's one.
pub(crate) fn of_landing(out_dir: &FsPath) -> Option<&FsPath> {
    out_dir.file_name().is_some_and(|name| name == folder::RUNS).then(|| out_dir.parent()).flatten()
}

/// The folder of the project a request names: its id (`404 project_unknown` when this computer knows none), else the project
/// holding a path, if one does.
pub(crate) fn named(api: &Api, named: &FsPath) -> Result<Option<PathBuf>, Error> {
    let text = named.to_string_lossy();
    if !text.starts_with("project-") || text.contains(['/', '\\']) {
        return Ok(holding(named).ok());
    }
    match indexed(api, false, |index| index.projects.get(text.as_ref()).map(|entry| entry.path.clone())) {
        Some(path) => Ok(Some(path)),
        None => Err(Error::new(Kind::NotFound, "project_unknown", format!("this computer knows no project {text}")).next("cozy project ls")),
    }
}

/// The files a run is `given`, each `(path, sha256)`, kept in its project unless they're there. What Calcifer holds in `uploads` is
/// named by its content and linked; anything else is a person's file and is copied. Says what it couldn't keep.
pub(crate) fn keep_given(project: &FsPath, given: &[(PathBuf, String)], uploads: &FsPath) -> Vec<String> {
    let folder = Folder::at(project);
    let unkept = given.iter().filter_map(|(path, sha256)| {
        let why = folder.keep_given(path, sha256, path.starts_with(uploads)).err()?;
        Some(format!("{} isn't kept in its project, so asking this run again needs it given again: {}", path.display(), why.message))
    });
    unkept.collect()
}

/// The folder a project's runs land in, as a run's record has it.
pub(crate) fn landed_in(dir: &FsPath) -> String {
    landing(dir).to_string_lossy().into_owned()
}

/// A run's project, whose files its request may name by hash.
#[derive(Clone)]
pub(crate) struct Holding {
    calcifer: Arc<crate::Calcifer>,
    dir: PathBuf,
}

impl std::fmt::Debug for Holding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Holding({})", self.dir.display())
    }
}

/// The hex of a hash as a request writes it: `sha256:<hex>`, or six or more hex digits.
fn written_hash(written: &str) -> Option<String> {
    let hex = written.strip_prefix("sha256:").unwrap_or(written);
    (hex.len() >= 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
}

impl Holding {
    pub(crate) fn new(calcifer: Arc<crate::Calcifer>, dir: PathBuf) -> Self {
        Self { calcifer, dir }
    }

    /// Whether `written` starts the hash of a file of this project.
    pub(crate) fn names(&self, written: &str) -> bool {
        let Some(hex) = written_hash(written) else { return false };
        Folder::at(&self.dir).lock().is_ok_and(|lock| !lock.matching(&hex).is_empty())
    }

    /// The file of this project `written` names by its hash, what stays on a machine fetched now that it's named. None: it names
    /// none of them.
    pub(crate) async fn file(&self, written: &str) -> Result<Option<PathBuf>, Error> {
        let Some(hex) = written_hash(written) else { return Ok(None) };
        let folder = Folder::at(&self.dir);
        let lock = folder.lock()?;
        match lock.matching(&hex).as_slice() {
            [] => Ok(None),
            [file] => {
                let at = folder.path(&file.path);
                if file.stays() && !at.is_file() {
                    let fetched = self.calcifer.bring(&file.sha256, &file.machine).await;
                    fetched.map_err(|why| Error::new(Kind::Unavailable, "file_on_a_machine", format!("{written} stays on {}, and can't be had: {why}", file.machine)))?;
                }
                Ok(Some(at))
            }
            several => Err(Error::new(Kind::Usage, "file_ambiguous", format!("{written} fits {} files of this project: write more of its hash", several.len()))),
        }
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
/// than it was, or a file comes to or goes from `files/`, as a pull's do with no write to either. Read from their stats alone, so a look at it
/// costs nothing: every write replaces the file, so its inode tells two writes in one clock tick apart.
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
    format!("{}.{}.{}", stamp(folder::PROJECT), stamp(folder::LOCK), stamp(folder::FILES))
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
    project.jobs.begin();
    let walk = Walk::new(api, project);
    let following = following(api, project);
    let api = api.clone();
    tokio::spawn(async move {
        remember(&api);
        let error = match walk.go().await {
            Ok(first) => first.map(|first| walk.last_word(first)),
            Err(error) => {
                *walk.project.walking.lock().expect("walking lock") = false;
                Some(error)
            }
        };
        remember(&api);
        following.abort();
        walk.project.jobs.end(walk.began.elapsed().as_millis() as u64);
        walk.say(Event::Ended { error, rendered: walk.rendered() });
    });
}

/// Keeps the board as the project's runs go, and says the warnings their logs give, each once, as its asset's.
fn following(api: &Arc<Api>, project: &Arc<Project>) -> tokio::task::JoinHandle<()> {
    let mut tail = api.calcifer.subscribe();
    let project = project.clone();
    tokio::spawn(async move {
        loop {
            let (id, event) = match tail.recv().await {
                Ok(heard) => heard,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if let Some((name, message)) = project.jobs.ran(&id, &event, &project.folder.dir) {
                let _ = project.events.send(Event::Warning { message, name: Some(name) });
            }
        }
    })
}

/// The board as the graph and what is on its way stand now. `run` reads a run by its number.
fn sync_jobs(project: &Project, run: impl Fn(u64) -> Option<Run>) -> Result<(), Error> {
    let walk = project.folder.walk_now()?;
    let walker = project.walker.lock().expect("walker lock");
    let graph = &walker.graph;
    let noun = |job: Option<&walker::Job>| if job.is_some_and(|job| job.video) { "take" } else { "version" };
    let going = (walk.pending.iter()).map(|(key, pending)| {
        let id = pending.job_of(key);
        let job = graph.job(id);
        Going {
            job: id.to_owned(),
            asset: pending.asset.clone(),
            noun: noun(job).into(),
            number: pending.version.number,
            run: pending.run,
            earlier_ms: pending.version.execution_ms,
            shot: job.is_some_and(|job| job.shot),
        }
    });
    let going = going.collect();
    let sent: HashSet<&str> = walk.pending.iter().map(|(key, pending)| pending.job_of(key)).collect();
    let coming = (graph.open_jobs().into_iter())
        .filter(|&id| !sent.contains(graph.jobs[id].key.as_str()))
        .map(|id| {
            let job = &graph.jobs[id];
            let waits = graph.waits_for(id);
            let why = match waits.as_slice() {
                [] => "next".to_owned(),
                [a, b, c] => format!("after {a}, {b}, {c}"),
                [a, b, c, more @ ..] => format!("after {a}, {b}, {c} and {} more", more.len()),
                waits => format!("after {}", waits.join(", ")),
            };
            let row = Job { id: job.key.clone(), asset: job.asset.clone(), noun: noun(Some(job)).into(), needs: waits.iter().map(|name| (*name).to_owned()).collect(), why: Some(why), ..Job::default() };
            (row, job.shot)
        })
        .collect();
    drop(walker);
    let on = walk.walk.asks.iter().map(|ask| &ask.target).chain(walk.pending.values().map(|pending| &pending.target));
    let on = on.flat_map(|target| target.on.iter().map(ToString::to_string)).collect();
    project.jobs.sync(on, going, coming, run);
    Ok(())
}

/// What a project's walker needn't read again.
#[derive(Default)]
struct Walker {
    graph: Graph,
    /// By text's digest: asks of one text read it once.
    readings: HashMap<String, Reading>,
    /// The asks in the graph.
    joined: HashSet<String>,
    /// Asks joined that it hasn't said anything of yet.
    greet: Vec<String>,
    /// Asks whose text doesn't read, and why.
    unread: Vec<(String, Error)>,
    /// The assets project.yaml names now, as of its stamp.
    names: Option<(String, HashSet<String>)>,
    /// What machines were asked to fetch ahead: machine, function and its model choices.
    asked_ahead: HashSet<(String, String, String)>,
}

/// One text a render was asked with, read once, and what stands by it: each asset worked out when asked of, and again once what it reads moved.
struct Reading {
    doc: Arc<Doc>,
    nodes: Vec<Node>,
    done: Vec<Standing>,
    known: Vec<bool>,
    /// What reads each.
    readers: Vec<Vec<usize>>,
}

impl Reading {
    fn new(doc: Doc) -> Self {
        let n = doc.assets.len();
        let mut readers = vec![vec![]; n];
        for (i, reads) in doc.upstream.iter().enumerate() {
            reads.iter().for_each(|&read| readers[read].push(i));
        }
        let nodes = (0..n).map(|i| node_of(&doc, i)).collect();
        Self { nodes, done: vec![Standing::default(); n], known: vec![false; n], readers, doc: Arc::new(doc) }
    }

    fn ensure(&mut self, i: usize, lock: &Lock) {
        if self.known[i] {
            return;
        }
        for read in self.doc.upstream[i].clone() {
            self.ensure(read, lock);
        }
        self.done[i] = self.doc.standing(i, lock, &self.done);
        self.known[i] = true;
    }

    /// A new version of `name`: what stands by it is worked out again when next asked of.
    fn moved(&mut self, name: &str) {
        let Some(i) = self.doc.find(name) else { return };
        let mut stack = vec![i];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut self.known[i], false) {
                stack.extend(&self.readers[i]);
            }
        }
    }

    /// What stands of asset `i` and all it reads: what sending it looks at. The rest of the list holds nothing, so it costs nothing.
    fn cone(&mut self, i: usize, lock: &Lock) -> Vec<Standing> {
        self.ensure(i, lock);
        let mut out = vec![Standing::default(); self.done.len()];
        let (mut stack, mut seen) = (vec![i], HashSet::new());
        while let Some(i) = stack.pop() {
            if seen.insert(i) {
                out[i] = self.done[i].clone();
                stack.extend(&self.doc.upstream[i]);
            }
        }
        out
    }
}

fn node_of(doc: &Doc, i: usize) -> Node {
    let asset = &doc.assets[i];
    let said = serde_json::to_string(&(&asset.fields, &asset.body)).unwrap_or_default();
    let video = asset.kind == "video";
    Node {
        name: asset.name.clone(),
        spec: hex::encode(Sha256::digest(said)),
        reads: doc.upstream[i].clone(),
        makes: doc.makes(i),
        free: doc.tool(i).is_some(),
        pinned: asset.pin.is_some(),
        shot: video && doc.tool(i).is_none(),
        video,
    }
}

/// A frozen render as the graph reads it, by what project.lock holds now.
struct Now<'a> {
    reading: &'a mut Reading,
    lock: &'a Lock,
    ask: &'a Ask,
}

impl Frozen for Now<'_> {
    fn nodes(&self) -> &[Node] {
        &self.reading.nodes
    }

    fn order(&self) -> &[usize] {
        &self.reading.doc.order
    }

    fn holds(&mut self, i: usize) -> bool {
        self.reading.ensure(i, self.lock);
        self.reading.done[i].files.is_some()
    }

    fn judged(&mut self, i: usize) -> bool {
        if !self.ask.outdated {
            return false;
        }
        self.reading.ensure(i, self.lock);
        judged_pin(&self.reading.doc, &self.reading.done, self.ask, i)
    }
}

impl Project {
    /// Each render `lock` holds that the graph doesn't yet: its jobs made, what it left going taken in. Answers the jobs made.
    fn join(&self, lock: &Lock) -> Vec<usize> {
        let mut walker = self.walker.lock().expect("walker lock");
        let Walker { graph, readings, joined, greet, unread, .. } = &mut *walker;
        let mut made = vec![];
        for ask in &lock.walk.asks {
            if !joined.insert(ask.id.clone()) {
                continue;
            }
            if !readings.contains_key(&ask.digest) {
                let read = self.folder.text_of(ask).and_then(|text| Doc::read(&self.folder, &text));
                match read {
                    Ok(doc) => drop(readings.insert(ask.digest.clone(), Reading::new(doc))),
                    Err(error) => {
                        unread.push((ask.id.clone(), error));
                        continue;
                    }
                }
            }
            greet.push(ask.id.clone());
            let reading = readings.get_mut(&ask.digest).expect("just read");
            for (key, pending) in lock.pending.iter().filter(|(_, pending)| pending.ask == ask.id) {
                let job = pending.job_of(key);
                if graph.id(job).is_none() {
                    let node = reading.doc.find(&pending.asset).map(|i| reading.nodes[i].clone()).unwrap_or_default();
                    graph.adopt(&ask.id, job, &pending.asset, &node, pending.approve);
                }
            }
            let asked = walker::Asked {
                all: ask.all,
                tried: ask.sent.clone(),
                takes: ask.takes.iter().map(|take| Named { key: take.key.clone(), asset: take.asset.clone(), approve: take.approve }).collect(),
            };
            made.extend(graph.add(&ask.id, &mut Now { reading, lock, ask }, &asked));
        }
        made
    }
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

/// What to run while a project's render goes on.
fn status_next(folder: &Folder) -> Vec<String> {
    vec![format!("cozy project -p {} status", folder.dir.display())]
}

/// By `name`, makes a new empty project under `~/.cozy/projects`. By `path`, opens the project holding it (the nearest folder up from
/// it with a project.yaml), or makes one there (`create`: the folder and its project.yaml). Either way it is recorded in the index as
/// used now.
async fn open(State(api): State<Arc<Api>>, headers: HeaderMap, body: Result<Bytes, BytesRejection>) -> Result<Json<Opened>, Refusal> {
    let open: Open = parse(&body?)?;
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok());
    let io = |at: &FsPath, e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", at.display())));
    let dir = match (&open.name, &open.path) {
        (Some(name), _) => {
            let made = key.and_then(|key| api.projects.made.lock().expect("made lock").get(key).cloned());
            match made {
                Some(dir) => dir,
                None => {
                    let dir = named_project(&api, name).map_err(Refusal)?;
                    if let Some(key) = key {
                        api.projects.made.lock().expect("made lock").insert(key.to_owned(), dir.clone());
                    }
                    dir
                }
            }
        }
        (None, Some(path)) if !path.is_absolute() => {
            return Err(Refusal(Error::new(Kind::Usage, "project_path", "a project is opened by its absolute path")));
        }
        (None, Some(path)) if open.create => {
            let project = path.join(folder::PROJECT);
            if project.exists() {
                let said = format!("{} is a project already", path.display());
                return Err(Refusal(Error::new(Kind::Conflict, "project_exists", said).next(format!("cozy project -p {} status", path.display()))));
            }
            std::fs::create_dir_all(path).map_err(|e| io(path, e))?;
            platform::durable::replace(&project, format::starter().as_bytes(), 0o644).map_err(|e| io(path, std::io::Error::other(e)))?;
            let dir = platform::durable::canonical(path).map_err(|e| io(path, std::io::Error::other(e)))?;
            crate::events::project_changed(&dir);
            dir
        }
        (None, Some(path)) => holding(path).map_err(Refusal)?,
        (None, None) => {
            return Err(Refusal(Error::new(Kind::Usage, "project_path", "a project is made by its name, or opened by its absolute path")));
        }
    };
    // Indexed first: a walk this open starts records itself there.
    let (id, used_at_ms) = (project_id(&dir), now_ms());
    indexed(&api, true, |index| {
        let walking = index.projects.get(&id).is_some_and(|known| known.walking);
        index.projects.insert(id.clone(), Entry { path: dir.clone(), used_at_ms, walking });
    });
    api.projects.open(&api, &dir).map_err(Refusal)?;
    let next = open.create.then(|| format!("cd {} && cozy project render all --on <machine>", dir.display())).into_iter().collect();
    Ok(Json(Opened { next, ..opened(id, &dir) }))
}

/// The project `id`, in `dir`, as an answer gives it.
fn opened(id: String, dir: &FsPath) -> Opened {
    let name = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Opened { id, path: dir.to_owned(), name, next: vec![] }
}

/// A new empty project called `name` under `~/.cozy/projects`, or the next free name: what it could be called, `_2`, `_3`, ….
fn named_project(api: &Api, name: &str) -> Result<PathBuf, Error> {
    let home = api.machines.hubs.home().projects();
    let io = |e: std::io::Error| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", home.display()));
    std::fs::create_dir_all(&home).map_err(io)?;
    let wanted = format::plain_name(name).unwrap_or_else(|| "untitled".to_owned());
    let names = std::iter::once(wanted.clone()).chain((2..).map(|n| format!("{wanted}_{n}")));
    for name in names {
        let dir = home.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                let project = dir.join(folder::PROJECT);
                platform::durable::replace(&project, format::blank().as_bytes(), 0o644).map_err(|e| io(std::io::Error::other(e)))?;
                let dir = platform::durable::canonical(&dir).map_err(|e| io(std::io::Error::other(e)))?;
                crate::events::project_changed(&dir);
                return Ok(dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e)),
        }
    }
    unreachable!("names go on")
}

/// The project folder holding `path`: the nearest, up from it, with a project.yaml.
fn holding(path: &FsPath) -> Result<PathBuf, Error> {
    let not_found = || {
        let said = format!("{} is in no project: no {} there or in a folder above it", path.display(), folder::PROJECT);
        Error::new(Kind::NotFound, "project_not_found", said)
            .remedy("run it inside a project's folder, or name one with -p <path>")
            .next("cozy project new <path>")
    };
    let start = platform::durable::canonical(path).map_err(|_| not_found())?;
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
        let said = format!("{id} is rendering: forget it once its runs land");
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
    let text = folder.text().map_err(Refusal)?;
    let doc = match Doc::read(folder, &text) {
        Ok(doc) => doc,
        Err(error) if !lock.owes() => return Err(Refusal(error)),
        // A render going has its own text: what it owes, and what's wrong with the file, are the answer.
        Err(error) => {
            let queue = queue(&project, &lock);
            let busy = *project.walking.lock().expect("walking lock");
            let moving = project.moving.try_lock().is_err();
            let warning = format!("{}: a render going uses the text it was asked with, and a new one is refused until this reads", error.message);
            return Ok(Json(ProjectState { id, path: folder.dir.clone(), assets: vec![], entities: vec![], queue, missing: vec![], stale: vec![], orphans: vec![], busy, moving, absent: 0, run_files: 0, cuts: vec![], warnings: vec![ProjectWarning::of_file(warning)], next: vec![], board: board_of(&project) }));
        }
    };
    let stand = doc.stand(&lock);
    let mut warnings = doc.warnings.clone();
    warnings.extend(doc.said_of_known());
    warnings.extend(unadapted(&api, &doc));
    for (i, standing) in stand.iter().enumerate() {
        let held = lock.asset(&doc.assets[i].name);
        let made = standing.version().and_then(|number| held?.version(number));
        warnings.extend(made.into_iter().flat_map(|made| made.warnings.iter().map(|message| ProjectWarning::of(&doc.assets[i].name, message.clone()))));
    }
    warnings.dedup();
    let (mut missing, mut stale) = (vec![], vec![]);
    let entities = (0..doc.assets.len()).filter(|&i| doc.assets[i].links.as_ref().is_some_and(|links| links.parent.is_none())).map(|i| entity_row(&doc, i)).collect();
    let etags = assetfile::etags(&text);
    let assets: Vec<Row> = (0..doc.assets.len())
        .filter(|&i| doc.assets[i].links.is_none())
        .map(|i| {
            let mut row = row(&api, &doc, folder, &lock, &stand, i);
            row.etag = etags.get(&name_key(&row.name)).cloned();
            row.rendering = project.rendering.lock().expect("rendering lock").get(&row.name).copied();
            if row.rendering.is_some() {
                row.standing = RowStanding::Making;
            }
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
    let queued = queue(&project, &lock);
    let next = next_steps(&orphans, absent, !missing.is_empty() || !stale.is_empty());
    let kept: BTreeSet<&str> = assets.iter().filter_map(|row| Some(row.failure.as_ref()?.message.as_str())).collect();
    warnings.retain(|warning| !kept.contains(warning.message.as_str()));
    Ok(Json(ProjectState { id, path: folder.dir.clone(), assets, queue: queued, missing, stale, orphans, busy, moving, absent, run_files, entities, cuts, warnings, next, board: board_of(&project) }))
}

/// What to run next on a project: its orphans' renames and `reclaim`, then a pull of what isn't here, else a render of what is owed.
fn next_steps(orphans: &[Orphan], absent: u64, owed: bool) -> Vec<String> {
    let renames = orphans.iter().map(|orphan| format!("cozy project rename {} {}", orphan.name, orphan.renamed_to.as_deref().unwrap_or("<its new name>")));
    let mut next: Vec<String> = renames.collect();
    if !orphans.is_empty() {
        next.push("cozy project reclaim".into());
    }
    if absent > 0 {
        next.push("cozy project pull".into());
    } else if owed {
        next.push("cozy project render all --on <machine>".into());
    }
    next
}

/// Its render going, or its last since Calcifer started.
fn board_of(project: &Project) -> Option<protocol::project::Board> {
    Some(project.jobs.board()).filter(|board| board.seq > 0)
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

/// The local code `id`'s assets name, each once: its path as they write it, and where that is.
pub(crate) fn local_code_named(api: &Arc<Api>, id: &str) -> Result<Vec<(String, PathBuf)>, Refusal> {
    let project = api.projects.get(api, id)?;
    let folder = &project.folder;
    let doc = Doc::load(folder).map_err(Refusal)?;
    let home = &api.machines.user_home;
    let mut named = BTreeMap::new();
    for with in (0..doc.assets.len()).map(|i| doc.target(i)).filter(|with| !with.is_empty()) {
        if let Some(Ok((path, function))) = crate::local_code::target(&with, &folder.dir, home) {
            let written = match function {
                Some(function) => with.trim().trim_end_matches(['/', '\\']).strip_suffix(function.as_str()).unwrap_or(&with).trim_end_matches(['/', '\\']).to_owned(),
                None => with.trim().trim_end_matches(['/', '\\']).to_owned(),
            };
            named.insert(path, written);
        }
    }
    Ok(named.into_iter().map(|(path, written)| (written, path)).collect())
}

/// What can be said of each asset whose published package has been heard of: its adapter for its kind, its tags. A read waits on no
/// Hub and writes nothing, so what isn't heard of yet is said by a later read, or a render. Local code is described only on a machine, so a render says it.
fn unadapted(api: &Arc<Api>, doc: &Doc) -> Vec<ProjectWarning> {
    let mut said = vec![];
    for i in (0..doc.assets.len()).filter(|&i| doc.makes(i) && doc.tool(i).is_none()) {
        let with = doc.target(i);
        if protocol::target::written_as_path(&with) || with.ends_with(".py") {
            continue;
        }
        let Some(package) = heard_of(api, &with) else { continue };
        if doc.tags(i).is_none() {
            said.extend(doc.said_once_read(i, &package.reserved()));
        }
        if doc.assets[i].pin.is_none() && !matches!(package.maker(&doc.assets[i].kind), Ok(Some(_))) {
            said.push(ProjectWarning::of(&doc.assets[i].name, no_adapter(&doc.assets[i].name, &package.name, &doc.assets[i].kind)));
        }
    }
    said
}

/// The tags of what makes asset `i`, this moment: none of what Calcifer makes itself. None: they aren't known yet, and its names held back.
fn tags_now(api: &Arc<Api>, doc: &Doc, i: usize) -> Option<Vec<String>> {
    if doc.tool(i).is_some() {
        return Some(vec![]);
    }
    if let Some(tags) = doc.tags(i) {
        return Some(tags.clone());
    }
    let with = doc.target(i);
    if with.is_empty() || protocol::target::written_as_path(&with) || with.ends_with(".py") {
        return None;
    }
    Some(heard_of(api, &with)?.reserved())
}

/// What was last heard of the package `with`, this moment. If that isn't recent, it is asked again in the background.
fn heard_of(api: &Arc<Api>, with: &str) -> Option<Package> {
    let mut heard = api.projects.heard.lock().expect("heard lock");
    let entry = heard.entry(with.to_owned()).or_default();
    if entry.asked.is_none_or(|asked| asked.elapsed() > HEARD_FOR) {
        entry.asked = Some(Instant::now());
        let (api, with) = (api.clone(), with.to_owned());
        tokio::spawn(async move {
            if let Ok(package) = Host::new(&api).published("", &with, None).await {
                api.projects.heard.lock().expect("heard lock").entry(with).or_default().package = Some(package);
            }
        });
    }
    entry.package.clone()
}

/// Keeps `package`'s tags in project.lock for what reads the file.
fn read_tags(doc: &Doc, folder: &Folder, i: usize, package: &Package) {
    let (key, tags) = (doc.assets[i].tool.clone().unwrap_or_default(), package.reserved());
    let same = folder.lock().is_ok_and(|lock| lock.reserved.get(&key) == Some(&tags));
    if !same && let Err(error) = folder.update_lock(|lock| drop(lock.reserved.insert(key.clone(), tags.clone()))) {
        warn!("{}: {error}", key);
    }
}

/// The line that says an asset can't be made by what names it.
fn no_adapter(name: &str, package: &str, kind: &str) -> String {
    format!("{name}: {package} has no adapter for kind {kind}, and no one function that makes it, so name one: `{package}/<function>`")
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
    // The film's own length once one stands, whatever its pieces now add up to. A film the size of its version is that version, whose
    // frames are measured: a read runs no process.
    let len = |at: &FsPath| std::fs::metadata(at).ok().map(|meta| meta.len());
    let stands = stand[i].files.as_ref().and_then(|files| files.get("video")).and_then(|sha256| lock.files.get(sha256));
    let measured = stands.filter(|film| film.frames > 0 && len(&path).is_some() && len(&folder.path(&film.path)) == len(&path)).map(|film| film.frames);
    let frames = measured.or_else(|| folder.film_frames(&path)).or(made.as_ref().map(|made| made.frames));
    let retimed = made.map(|made| made.retimed).unwrap_or_default();
    let standing = stand[i].version().and_then(|number| lock.asset(&asset.name)?.version(number));
    let without = standing.map(|version| version.without.iter().map(folder::Without::said).collect()).unwrap_or_default();
    Cut { cut: name, path, fps: canvas.fps, cut_at_ms, frames, retimed, skips, without }
}

/// What a cut's version was cut from: each piece's asset, and its version when it was one.
fn cut_from(lock: &Lock, version: &Version) -> Vec<String> {
    if version.made_by.as_ref().is_none_or(|by| by.with != EDIT) {
        return vec![];
    }
    let took = |read: &ReadFrom| {
        let number = lock.asset(&read.name)?.versions.iter().find(|held| held.main() == folder::main_of(&read.files))?.number;
        Some(format!("{} {number}", read.name))
    };
    version.reads.iter().map(|read| took(read).unwrap_or_else(|| read.name.clone())).collect()
}

/// What its versions are called.
fn noun(doc: &Doc, lock: &Lock, i: usize) -> &'static str {
    let asset = &doc.assets[i];
    if doc.tool(i) == Some(Tool::Cut) {
        return "version";
    }
    let video = lock.asset(&asset.name).is_some_and(|held| held.versions.iter().any(|version| version.files.contains_key("video")));
    match video || asset.kind == "video" {
        true => "take",
        false => "version",
    }
}

/// A set of files as the API lists them.
fn listed(folder: &Folder, lock: &Lock, files: &Files) -> BTreeMap<String, Listing> {
    (files.iter())
        .filter_map(|(name, sha256)| {
            let file = lock.files.get(sha256)?;
            let (absent, machine) = whereabouts(folder, file);
            Some((name.clone(), Listing { path: file.path.clone(), sha256: sha256.clone(), absent, machine }))
        })
        .collect()
}

/// Whether a file isn't on this computer, and the machine that keeps it if one does.
fn whereabouts(folder: &Folder, file: &folder::File) -> (bool, Option<String>) {
    let absent = !folder.path(&file.path).is_file();
    (absent, (absent && file.stays()).then(|| file.machine.clone()))
}

/// The file people mean by `files`, as a version lists it.
fn main_file(folder: &Folder, lock: &Lock, files: &Files) -> Option<MainFile> {
    let sha256 = folder::main_of(files)?;
    let file = lock.files.get(sha256)?;
    let output = files.iter().find(|(_, held)| *held == sha256)?.0.clone();
    let (absent, machine) = whereabouts(folder, file);
    let measured = |number: u64| (number > 0).then_some(number);
    Some(MainFile {
        output,
        path: file.path.clone(),
        sha256: sha256.clone(),
        field: file.field().to_owned(),
        width: measured(file.width),
        height: measured(file.height),
        duration_ms: measured(file.lasts_ms()),
        alpha: file.alpha > 0.0,
        absent,
        machine,
    })
}

/// The path of the file people mean by `files`.
fn main_path(lock: &Lock, files: &Files) -> Option<String> {
    folder::main_of(files).and_then(|sha256| lock.files.get(sha256)).map(|file| file.path.clone())
}

/// Asset `i` as `GET /v1/projects/{id}` lists it.
fn row(api: &Arc<Api>, doc: &Doc, folder: &Folder, lock: &Lock, stand: &[Standing], i: usize) -> Row {
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
        ([], Stand::Waits) => "waits for what it reads".into(),
        ([], Stand::Stale(why)) => format!("out of date: {why}"),
        ([], Stand::Missing) if asset.hollow => "it has nothing to make".into(),
        ([], Stand::Missing) => format!("no {noun} yet"),
    };
    let made = standing.version().and_then(|number| held?.version(number));
    let main = folder::main_of(&files).and_then(|sha256| lock.files.get(sha256));
    let measured = |number: fn(&folder::File) -> u64| main.map(number).filter(|&number| number > 0);
    let (absent, machine) = main.map_or((false, None), |file| whereabouts(folder, file));
    let failure = held.and_then(|held| held.failed.as_ref()).filter(|failed| making.is_empty() && failed.inputs == standing.inputs).map(|failed| failed.error.clone());
    let changed = match &standing.state {
        Stand::Pinned { changed, .. } => changed.clone(),
        _ => vec![],
    };
    let row_standing = match (making.is_empty(), &standing.state) {
        (false, _) => RowStanding::Making,
        (true, Stand::Stale(_)) => RowStanding::OutOfDate,
        (true, Stand::Pinned { .. }) if !changed.is_empty() => RowStanding::OutOfDate,
        (true, Stand::Missing | Stand::Waits) => RowStanding::Missing,
        (true, _) => RowStanding::Current,
    };
    Row {
        name: asset.name.clone(),
        standing: row_standing,
        making,
        kind: asset.kind.clone(),
        noun: noun.into(),
        tool: doc.makes(i).then(|| doc.target(i)),
        versions: held.map_or(0, |held| held.versions.len() as u32),
        prefixes: (held.into_iter().flat_map(|held| &held.versions))
            .filter_map(|version| {
                Some(VersionPrefix {
                    number: version.number,
                    prefix: folder::prefix_of(version.main()?).to_owned(),
                    kept: version.kept,
                    execution_ms: (version.execution_ms > 0).then_some(version.execution_ms),
                    run: version.runs.last().copied(),
                    main: main_file(folder, lock, &version.files),
                    from: cut_from(lock, version),
                    without: version.without.iter().map(folder::Without::said).collect(),
                    timeline: (!version.clips.is_empty()).then(|| {
                        let files = |name: &str| {
                            let read = version.reads.iter().find(|read| name_key(&read.name) == name_key(name));
                            let why = || version.without.iter().find(|left| name_key(&left.piece) == name_key(name)).map_or_else(|| "no longer in the project".to_owned(), |left| left.why.clone());
                            read.map(|read| read.files.clone()).ok_or_else(why)
                        };
                        tools::timeline_now(version.clips.clone(), version.fps, files, folder, lock)
                    }),
                })
            })
            .collect(),
        version: standing.version(),
        approved: asset.pin.as_ref().and(standing.version()),
        pinned: asset.pin.is_some(),
        inputs: standing.inputs.clone(),
        path: main_path(lock, &files),
        files: listed(folder, lock, &files),
        sha256: main.map(|file| file.sha256.clone()),
        field: main.map(|file| file.field().to_owned()),
        width: measured(|file| file.width),
        height: measured(|file| file.height),
        duration_ms: measured(folder::File::lasts_ms),
        alpha: main.is_some_and(|file| file.alpha > 0.0),
        absent,
        machine,
        made_by: made.and_then(|made| made.made_by.as_ref()).map(|by| by.with.clone()),
        status,
        reads: doc.reads_of(i),
        fields: doc.flat_fields(i),
        etag: None,
        failure,
        changed,
        missing: tags_now(api, doc, i).map_or_else(Vec::new, |tags| doc.missing_names(i, &tags)),
        timeline: (doc.tool(i) == Some(Tool::Cut)).then(|| {
            let files = |name: &str| {
                let read = standing.reads.iter().find(|read| name_key(&read.name) == name_key(name));
                let why = || if doc.find(name).is_some() { "no version yet" } else { "no longer in the project" }.to_owned();
                read.map(|read| read.files.clone()).ok_or_else(why)
            };
            tools::timeline_now(tools::clips(&asset.fields, &standing.reads), Canvas::of(&asset.fields, &mut vec![]).fps, files, folder, lock)
        }),
        rendering: None,
    }
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
            let files = held.versions.iter().flat_map(|version| version.files.values());
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
            let cut = held.versions.iter().any(|version| version.made_by.as_ref().is_some_and(|by| by.with == EDIT));
            if cut {
                let name = cut_name(&held.name);
                let _ = std::fs::remove_file(folder.dir.join(&name));
                let _ = std::fs::remove_file(folder.dir.join(FsPath::new(&name).with_extension("otio")));
            }
        }
        folder.update_lock(|lock| lock.assets.retain(|key, _| doc.holds(key).is_some())).map_err(Refusal)?;
    }
    let (files, bytes) = folder.sweep(&kept, asked.yes).map_err(Refusal)?;
    if asked.yes {
        canvas::prune(folder, &id, |name| doc.find(name).is_some()).map_err(Refusal)?;
    }
    let next = (!asked.yes && (files > 0 || !orphans.is_empty())).then(|| "cozy project reclaim --yes".to_owned()).into_iter().collect();
    Ok(Json(Reclaimed { orphans, files, bytes, deleted: asked.yes, next }))
}

/// Renames asset `from` to `to`: in project.yaml its key and every `<from>`, unless the file has moved to `to` already, then what
/// project.lock keeps of it, its pin and what is pending or owed of it, and its row on the canvas. Its readers' text changed, so they
/// go out of date unless pinned. While the project renders, what it renders still says `from`, so project.lock's part happens as that
/// walk ends. Nothing on disk moves.
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
    let mut doc = Doc::load(folder).map_err(Refusal)?;
    let free = doc.find(&asked.to).is_none() || name_key(&asked.to) == name_key(&asked.from);
    if let Some(i) = doc.holds(&asked.from).filter(|_| free) {
        if !doc.declared(i) {
            let said = format!("{} is named by another's lines in {}: rename it there", asked.from, folder::PROJECT);
            return Err(refuse(Kind::Usage, "project_edit", said));
        }
        folder.edit_project(|text| format::renamed_in(text, &asked.from, &asked.to)).map_err(Refusal)?;
        doc = Doc::load(folder).map_err(Refusal)?;
    }
    let lock = folder.lock().map_err(Refusal)?;
    let Some(i) = doc.holds(&asked.to) else {
        let said = format!("{} names no {}: rename the asset (and its <mentions>) there first", folder::PROJECT, asked.to);
        return Err(refuse(Kind::NotFound, "asset_unknown", said));
    };
    let old = lock.asset(&asked.from).filter(|held| !held.versions.is_empty());
    let stands = !free && doc.holds(&asked.from).is_some();
    if old.is_none() && stands {
        return Err(refuse(Kind::Conflict, "rename_taken", format!("{} is in the project already", asked.to)));
    }
    if old.is_none() && !free && canvas::row(folder, &asked.from).map_err(Refusal)?.is_none() {
        let said = format!("project.lock keeps no version of {}", asked.from);
        return Err(Refusal(Error::new(Kind::NotFound, "asset_unknown", said).next("cozy project status")));
    }
    let to = doc.assets[i].name.clone();
    if old.is_some() && lock.asset(&to).is_some_and(|held| !held.versions.is_empty()) {
        return Err(refuse(Kind::Conflict, "rename_taken", format!("{to} has versions of its own already")));
    }
    let (from, versions) = (asked.from.clone(), old.map_or(0, |old| old.versions.len() as u32));
    // Under `walking`, as a walk's end is, so that it is done now or that walk does it.
    let walking = project.walking.lock().expect("walking lock");
    let queued = *walking;
    if queued || old.is_some() {
        folder
            .update_lock(|lock| match queued {
                true => lock.walk.renames.push((from.clone(), to.clone())),
                false => renamed(lock, &from, &to),
            })
            .map_err(Refusal)?;
    }
    drop(walking);
    canvas::moved(folder, &id, &from, &to).map_err(Refusal)?;
    Ok(Json(Renamed { from, to, versions, queued }))
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
        let said = "a project's runs go to machines: rent one first, then name it with --on";
        return Err(Refusal(Error::new(Kind::Usage, "project_on_new", said).next("cozy machine rent")));
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
                return Ok((StatusCode::OK, Json(Asked { id, path: project.folder.dir.clone(), joined: false, added: 0, skipped: vec![], queued: vec![], next: status_next(&project.folder), board: project.jobs.board() })));
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
    let say = |message: String| drop(project.events.send(Event::Warning { message, name: None }));
    let said = |name: &str, message: String| drop(project.events.send(Event::Said { message, name: Some(name.to_owned()) }));
    let n = asked.n.max(1);
    // Every render makes new versions, unless it asks only for what has no state.
    let missing = asked.missing;
    if missing && n > 1 {
        say("-n makes new versions, and --missing only what has no state: it is ignored".into());
    }
    let outdated = all && asked.outdated;
    if asked.outdated && !all {
        say("--outdated goes with all: naming a pin already makes a new version of it: it is ignored".into());
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
        // What this asks of it is judged at its turn.
        true if outdated && matches!(stand[i].state, Stand::Pinned { version: Some(_), .. }) => {}
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
            said(&name, format!("{name} is a constant, one file: nothing is made of it"));
            continue;
        }
        if doc.tool(i).is_some() {
            said(&name, format!("{name} is made here, free, when what it reads settles"));
            // Named, it is made again, as anything named is.
            if !missing {
                takes.push(Take { key: new_key(), asset: name.clone(), target: target.clone(), approve: false, new: true, keeps: None });
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
            said(&name, format!("{name} is being made"));
            continue;
        }
        if missing && keeps.is_none() && stand[i].files.is_some() {
            said(&name, match &stand[i].state {
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
    let new_outdated = outdated && !lock.walk.asks.iter().any(|ask| ask.outdated && ask.digest == digest);
    let added = takes.len() as u32 + u32::from(new_all) + u32::from(new_outdated);
    project.folder.keep_text(&digest, &text).map_err(Refusal)?;
    let ask = Ask { id: new_key(), text: String::new(), digest, target: target.clone(), all: all && missing, outdated, whole: all, sent: BTreeSet::new(), takes };
    (project.folder.update_lock(|lock| {
        if !joined || added > 0 {
            lock.walk.asks.push(ask.clone());
        }
        lock.walk.asked.extend(idempotency.iter().cloned());
        for take in &ask.takes {
            if let Some(held) = lock.assets.get_mut(&name_key(&take.asset)) {
                held.failed = None;
            }
        }
    }))
    .map_err(Refusal)?;
    let made = project.join(&project.folder.lock().map_err(Refusal)?);
    let queued = queued_now(&project, &made);
    walk(&api, &project);
    sync_jobs(&project, numbered(&api)).map_err(Refusal)?;
    Ok((StatusCode::ACCEPTED, Json(Asked { id, path: project.folder.dir.clone(), joined, added, skipped, queued, next: status_next(&project.folder), board: project.jobs.board() })))
}

/// A run by its number, as its record says now.
fn numbered(api: &Api) -> impl Fn(u64) -> Option<Run> + '_ {
    move |number| {
        let calcifer = &api.calcifer;
        calcifer.record(&calcifer.store().run_by_number(number).ok()??).ok()
    }
}

/// What the project has to do, in the order it goes: what is running, then what each render has queued.
fn queue(project: &Project, lock: &Lock) -> Vec<Queued> {
    project.join(lock);
    let runs: HashMap<&str, u64> = lock.pending.iter().map(|(key, pending)| (pending.job_of(key), pending.run)).collect();
    let walker = project.walker.lock().expect("walker lock");
    let graph = &walker.graph;
    let row = |id: usize| {
        let job = &graph.jobs[id];
        let run = runs.get(job.key.as_str()).copied();
        Queued { name: job.asset.clone(), running: run.is_some(), run: run.filter(|run| *run > 0) }
    };
    graph.open_jobs().into_iter().map(row).collect()
}

/// How many names a queued job says are ahead of it.
const AHEAD_SAID: usize = 8;

/// What `made` are, each with the names ahead of it.
fn queued_now(project: &Project, made: &[usize]) -> Vec<QueuedNow> {
    let walker = project.walker.lock().expect("walker lock");
    let graph = &walker.graph;
    let open = graph.open_jobs();
    (open.iter().enumerate())
        .filter(|(_, id)| made.contains(id))
        .map(|(at, &id)| QueuedNow { name: graph.jobs[id].asset.clone(), ahead: open[..at].iter().take(AHEAD_SAID).map(|&ahead| graph.jobs[ahead].asset.clone()).collect() })
        .collect()
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
        let doc = Doc::load(folder).ok().or_else(|| lock.walk.asks.last().and_then(|ask| Doc::read(folder, &folder.text_of(ask).ok()?).ok()));
        for name in &asked.names {
            match doc.as_ref().and_then(|doc| Some((doc, doc.find(name).filter(|&i| doc.assets[i].links.is_some())?))) {
                Some((doc, i)) => keys.extend(doc.linked(i).into_iter().map(|j| name_key(&doc.assets[j].name))),
                None => drop(keys.insert(name_key(name))),
            }
        }
    }
    let wanted = |name: &str| all || keys.contains(&name_key(name));
    project.join(&lock);
    let sent: HashSet<&str> = lock.pending.iter().map(|(key, pending)| pending.job_of(key)).collect();
    let (dropped, unsent) = {
        let mut walker = project.walker.lock().expect("walker lock");
        let (dropped, going) = walker.graph.cancel(wanted);
        let rows: Vec<walker::Job> = dropped.iter().map(|&id| walker.graph.jobs[id].clone()).collect();
        // What is being sent, and has no run yet, isn't sent.
        let unsent: Vec<String> = going.iter().map(|&id| &walker.graph.jobs[id]).filter(|job| !sent.contains(job.key.as_str())).map(|job| name_key(&job.asset)).collect();
        (rows, unsent)
    };
    for job in &dropped {
        project.jobs.ended(Job { id: job.key.clone(), asset: job.asset.clone(), state: JobState::Canceled, why: Some("canceled".into()), ..Job::default() });
    }
    let owed: Vec<String> = dropped.iter().map(|job| name_key(&job.asset)).chain(unsent).collect();
    let dropped: Vec<String> = dropped.into_iter().map(|job| job.asset).collect();
    folder
        .update_walk(|walk| {
            for ask in &mut walk.walk.asks {
                ask.takes.retain(|take| !wanted(&take.asset));
                ask.all &= !all;
                ask.outdated &= !all;
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
    sync_jobs(&project, numbered(&api)).map_err(Refusal)?;
    // `all` stops what is fetched ahead too; the walker stops what fewer leave unneeded.
    if all {
        for (_, _, run) in project.jobs.ahead_going() {
            let _ = api.calcifer.cancel(&run);
        }
    }
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

/// `source` kept as a file of the project, in project.lock.
async fn keep_file(api: &Api, folder: &Folder, source: &str) -> Result<folder::Kept, Refusal> {
    let Some(digest) = held_name(source)? else {
        return folder.import(source).await.map_err(|why| import_failed(source, why));
    };
    match crate::runs::files::held(&api.calcifer, &digest) {
        Some(at) => tokio::task::block_in_place(|| folder.take_held(&at, &digest.to_string())).map_err(|why| import_failed(source, why)),
        None => folder.own(&digest.to_string()).ok_or_else(|| not_held(source)),
    }
}

/// The digest `source` names, if it names one.
fn held_name(source: &str) -> Result<Option<Digest>, Refusal> {
    if !source.starts_with("sha256:") {
        return Ok(None);
    }
    let digest = source.parse().map_err(|why: &str| Refusal(Error::new(Kind::Usage, "file_name_invalid", format!("{source:?}: {why}")).remedy("name a file sha256:<hex>")))?;
    Ok(Some(digest))
}

fn import_failed(source: &str, why: String) -> Refusal {
    // A hash is no one's to read: a person dropped a file, and is told of it by its name.
    let said = if source.starts_with("sha256:") { why } else { format!("{source}: {why}") };
    Refusal(Error::new(Kind::Validation, "import_failed", said).remedy("cozy project import takes a picture, video or sound, by path or http(s) URL"))
}

fn not_held(source: &str) -> Refusal {
    Refusal(Error::new(Kind::NotFound, "file_not_held", format!("Calcifer holds no file {source}, and the project has none")).remedy(format!("PUT its bytes to /v1/files/{source} first")))
}

/// What a person's `name`, or `source`'s stem, calls an element: `file` when neither can be a name.
fn element_name(name: Option<&str>, source: &str) -> String {
    let stem = || {
        let path = match folder::is_url(source) {
            true => source.split_once("://")?.1.split_once('/')?.1.split(['?', '#']).next()?,
            false => source.strip_prefix("sha256:").map_or(Some(source), |_| None)?,
        };
        FsPath::new(path).file_stem()?.to_str().and_then(format::plain_name).map(|stem| stem.to_lowercase())
    };
    name.and_then(format::plain_name).or_else(stem).unwrap_or_else(|| "file".to_owned())
}

/// The run that removes the background of the picture at `at`. Err: why there is none, and a plain copy stands. The one place Calcifer names this package.
async fn removing_background(api: &Arc<Api>, folder: &Folder, at: &FsPath, field: &str, on: Option<On>, key: &str) -> Result<Run, String> {
    if field != "image" {
        return Err("--remove-background takes a picture: this is kept as it is".to_owned());
    }
    if folder.transparent(at) >= OPAQUE_BELOW {
        return Err("already transparent, so it is kept as it is".to_owned());
    }
    let image = FileTerm { name: Some("image".into()), file: at.to_string_lossy().into_owned() };
    let ask = RunRequest { target: "birefnet/remove_background".into(), on, files: vec![image], project: Some(folder.dir.clone()), ..Default::default() };
    match crate::runs::request_keeping_outputs(api, key, ask).await {
        Ok(submitted) => Ok(submitted.run),
        Err(error) if error.code == "local_unavailable" => Err(format!("{}: rent one with `cozy machine rent`, or add `--on <machine>`; a plain copy is kept", error.message)),
        Err(error) => Err(format!("{}; a plain copy is kept", error.message)),
    }
}

/// The element a kept file makes: one constant more in project.yaml, under a name nothing has, and with `at`, its row on the canvas.
/// Its answer is kept for its key.
fn element(project: &Project, id: &str, wanted: &str, at: Option<protocol::project::Card>, kept: folder::Kept, note: Option<String>, key: Option<&(String, String)>) -> Result<Imported, Refusal> {
    let folder = &project.folder;
    let _one = project.importing.lock().expect("importing lock");
    if let Some(done) = replayed(folder, key)? {
        return Ok(done);
    }
    let lock = folder.lock().map_err(Refusal)?;
    let value = format::yaml_scalar(folder::prefix_of(&kept.file.sha256));
    let name = std::cell::RefCell::new(String::new());
    (folder.edit_project(|text| {
        let free = format::free_name(&Doc::read(folder, text)?, &lock, wanted);
        let edited = format::constant(text, &free, kept.field, &value)?;
        *name.borrow_mut() = free;
        Ok(edited)
    }))
    .map_err(Refusal)?;
    let name = name.into_inner();
    let answer = Imported { name: name.clone(), sha256: kept.file.sha256, field: kept.field.to_owned(), path: kept.file.path, already: kept.already, run: None, note };
    if let Some((key, digest)) = key {
        let at_ms = now_ms();
        (folder.update_lock(|lock| {
            lock.imported.retain(|_, row| at_ms - row.at_ms < IMPORT_KEPT_MS);
            lock.imported.insert(key.clone(), folder::Keyed { digest: digest.clone(), at_ms, answer: answer.clone() });
        }))
        .map_err(Refusal)?;
    }
    if let Some(card) = at {
        canvas::set(folder, id, BTreeMap::from([(name, Some(card))])).map_err(Refusal)?;
    }
    Ok(answer)
}

/// What an import under this key answered, if one did: it answers the same. Another import under it is refused.
fn replayed(folder: &Folder, key: Option<&(String, String)>) -> Result<Option<Imported>, Refusal> {
    let Some((key, digest)) = key else { return Ok(None) };
    match folder.lock().map_err(Refusal)?.imported.remove(key) {
        Some(row) if row.digest == *digest => Ok(Some(row.answer)),
        Some(_) => Err(Refusal(Error::new(Kind::Conflict, "idempotency_conflict", format!("Idempotency-Key {key} asked another import already")))),
        None => Ok(None),
    }
}

/// What a project's files are measured as, once its ffprobe is here, from a task of its own: a read waits for neither.
fn measure_soon(api: &Arc<Api>, project: &Arc<Project>) {
    let (host, project) = (Host::new(api), project.clone());
    tokio::spawn(async move {
        if project.folder.lock().map_or(true, |lock| !lock.files.values().any(folder::File::unmeasured)) {
            return;
        }
        let permit = thumbs::FFMPEGS.acquire().await.expect("ffmpeg's turns never close");
        if let Err(error) = host.ffmpeg().await {
            return warn!("measuring a project's files: {}", error.message);
        }
        let measuring = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            project.folder.lock().and_then(|mut lock| project.folder.measure(&mut lock))
        });
        if let Ok(Err(error)) = measuring.await {
            warn!("measuring a project's files: {}", error.message);
        }
    });
}

/// `POST /v1/projects/{id}/imports`: `source` kept in the project as an element, in one call. With `remove_background`, a picture goes
/// to a run first; the element is what the client then imports of its output, under the `name` answered.
async fn import(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Imported>, Refusal> {
    let body = body?;
    let asked: Import = parse(&body)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok()).map(|key| (key.to_owned(), hex::encode(Sha256::digest(&body))));
    if let Some(done) = replayed(folder, key.as_ref())? {
        return Ok(Json(done));
    }
    let wanted = element_name(asked.name.as_deref(), &asked.source);
    let (mut source, mut note, mut looked_at) = (asked.source.clone(), None, None);
    if asked.remove_background {
        let readable = match held_name(&asked.source)? {
            Some(digest) => (crate::runs::files::held(&api.calcifer, &digest))
                .or_else(|| folder.own(&digest.to_string()).map(|kept| folder.path(&kept.file.path)))
                .ok_or_else(|| not_held(&asked.source))?
                .to_string_lossy()
                .into_owned(),
            None => asked.source.clone(),
        };
        let (at, field) = folder.source(&readable).await.map_err(|why| import_failed(&asked.source, why))?;
        let run_key = key.as_ref().map_or_else(new_key, |(key, _)| format!("import-{key}"));
        match removing_background(&api, folder, &at, field, asked.on.clone(), &run_key).await {
            Ok(run) => {
                let run = Some(Box::new(run));
                return Ok(Json(Imported { name: wanted, sha256: String::new(), field: String::new(), path: String::new(), already: false, run, note: None }));
            }
            Err(why) => note = Some(why),
        }
        // A URL's bytes are here now, only to look at.
        if folder::is_url(&asked.source) {
            source = at.to_string_lossy().into_owned();
            looked_at = Some(at);
        }
    }
    let kept = keep_file(&api, folder, &source).await?;
    if let Some(at) = looked_at {
        let _ = std::fs::remove_file(at);
    }
    let answer = element(&project, &id, &wanted, asked.at, kept, note, key.as_ref())?;
    measure_soon(&api, &project);
    Ok(Json(answer))
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
    let noun = noun(&doc, &lock, i);
    let versions: Vec<&Version> = lock.asset(&asset.name).into_iter().flat_map(|held| &held.versions).collect();
    // A short run of digits is a version. Anything else is a file of the project's, by hash: any picture can be what this asset is.
    let wanted = asked.version.parse::<u32>().ok().filter(|_| asked.version.len() <= 6);
    let chosen = match wanted {
        None => match lock.matching(&asked.version.to_ascii_lowercase()).as_slice() {
            [file] => file.sha256.clone(),
            [] => {
                let said = format!("no file of this project's starts with {}", asked.version);
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy project status")));
            }
            several => {
                let said = format!("{} fits {} files of this project: write more of its hash", asked.version, several.len());
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said)));
            }
        },
        Some(wanted) => {
            let Some(sha256) = versions.iter().find(|version| version.number == wanted).and_then(|version| version.main()) else {
                let numbers: Vec<String> = versions.iter().map(|version| version.number.to_string()).collect();
                let said = match numbers.as_slice() {
                    [] => format!("{} has no {noun} yet", asset.name),
                    numbers => format!("{} has no {noun} {wanted}: it has {}", asset.name, numbers.join(", ")),
                };
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy project status")));
            };
            sha256.clone()
        }
    };
    let field = asset.kind.as_str();
    if let Some(is) = lock.files.get(&chosen).map(folder::File::field).filter(|is| wanted.is_none() && *is != field) {
        let said = format!("{} makes one {field}, and that file is {is}", asset.name);
        return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).remedy(format!("if it should be {is}, write `kind: {is}` on it"))));
    }
    let version = versions.iter().find(|version| version.main() == Some(&chosen));
    let before = stale_names(&doc, &doc.stand(&lock));
    let name = asset.name.clone();
    // A hash written in an entity's link is rewritten there.
    let (owner, written_in) = asset.line.as_ref().map_or((name.as_str(), field), |(owner, link)| (owner.as_str(), link.as_str()));
    pin_in_file(folder, owner, written_in, &chosen).map_err(Refusal)?;
    let warnings = settle(&api, &project).await;
    let files = version.map_or_else(|| Files::from([(field.to_owned(), chosen.clone())]), |version| version.files.clone());
    let (number, path) = (version.map_or(0, |version| version.number), main_path(&lock, &files).unwrap_or_default());
    let files = listed(folder, &lock, &files);
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
        let _ = project.events.send(Event::Warning { message, name: None });
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
            walker.measure()?;
            let (doc, lock) = (Doc::load(walker.folder())?, walker.folder().lock()?);
            let stand = doc.stand(&lock);
            let tools = tools_ready(&doc, &stand, &lock);
            Ok((doc, lock, stand, tools))
        })();
        let (doc, lock, stand, tools) = match ready {
            Ok(ready) => ready,
            Err(error) => return vec![error.message],
        };
        let tools: Vec<usize> = tools.into_iter().filter(|&i| tried.insert(doc.assets[i].name.clone())).collect();
        if tools.is_empty() {
            for i in (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)) {
                walker.film_follows(&doc, &stand, &lock, i).await;
            }
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
    let mut boards = project.jobs.subscribe();
    let board = boards.borrow_and_update().clone();
    let mut going = going(&api, &project.folder).map_err(Refusal)?;
    if board.seq > 0 {
        going.push(Event::Board { board });
    }
    let told = going.into_iter().map(|event| Ok(sse::Event::default().json_data(&event).expect("an event serializes")));
    let stop = api.stop_signal();
    let stream = futures_util::stream::iter(told).chain(futures_util::stream::unfold((tail, boards, stop), |(mut tail, mut boards, mut stop)| async move {
        loop {
            // The board first: what the walk says last, it has already shown.
            let event = tokio::select! {
                biased;
                changed = boards.changed() => match changed {
                    Ok(()) => Ok(Event::Board { board: boards.borrow_and_update().clone() }),
                    Err(_) => return None,
                },
                event = tail.recv() => event,
                _ = stop.wait_for(|stopping| *stopping) => return None,
            };
            match event {
                Ok(event) => {
                    let sent = sse::Event::default().json_data(&event).expect("an event serializes");
                    return Some((Ok(sent), (tail, boards, stop)));
                }
                // It fell behind: what it missed is in the board.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let event = Event::Board { board: boards.borrow().clone() };
                    let sent = sse::Event::default().json_data(&event).expect("an event serializes");
                    return Some((Ok(sent), (tail, boards, stop)));
                }
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
    let mut response = send(&project.folder.path(&format!("{}/{file}", folder::FILES)), &format!("\"{hex}\""), &headers)?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(crate::files::IMMUTABLE));
    Ok(response)
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
    /// The job's key: its first run goes under it.
    key: String,
    take: Option<Take>,
    /// A pin out of date: a new version, and when it lands, no pin.
    unpin: bool,
}

/// Where a run can read a file from.
enum Reach {
    /// In `files/`, or nowhere it knows of, which a pull answers.
    Here,
    /// That machine, one it may go to, holds it.
    There(On),
    /// The machine that keeps it is there to fetch it from.
    Fetch,
    /// With the machine that kept it, or evicted by it.
    Gone,
}

/// A project's one walker: gives each job of its graph its turn as what it needs ends, and follows each run until it lands.
struct Walk {
    host: Host,
    project: Arc<Project>,
    began: Instant,
    /// What it landed, for its last word.
    tally: Mutex<Rendered>,
    /// The cuts it made, which it has said.
    cut: Mutex<BTreeSet<String>>,
    /// What it did not make, for its last word.
    missed: Mutex<Missed>,
    /// How many runs failed each way on each machine: (machine, code, message).
    faults: Mutex<HashMap<(String, String, String), u32>>,
    /// The machines it stopped sending to (`fault`), with the renders that stops: one asked later tries again.
    stopped: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// That this computer's first need of ffprobe was met: it measures what adapters read.
    measuring: AtomicBool,
    /// While that is fetched, nothing goes: an adapter reads those sizes.
    holding: AtomicBool,
}

/// How many jobs go at once, per machine they may go to: being sent, in its line or running. What's beyond waits here, so a pod's queue stays short and what is asked later or canceled costs nothing there.
const WINDOW: usize = 32;

/// Something a walker started came to its end.
enum Done {
    Measured,
    Sent { id: usize, key: String, ask: Arc<Ask>, name: String, kind: String, inputs: Option<String>, result: Result<(), Error> },
    Made { id: usize, ask: String, name: String, kind: String, inputs: Option<String>, result: Result<(), Error> },
    Landed { job: String, landed: Followed },
    Ahead,
}

/// What a queued job's machine can fetch before its turn: a function as its text names it, on one machine.
struct Wanted {
    machine: On,
    hub: String,
    with: String,
    kind: String,
    body: Map<String, Value>,
}

/// What a job's turn came to, done once the graph is let go.
enum Act {
    Stands { id: usize, ask: String, key: String, asset: String, noun: &'static str, number: Option<u32> },
    Unread { id: usize, ask: String, on: Vec<On>, key: String, asset: String, kind: String, inputs: Option<String>, message: String },
    Deleted { id: usize, ask: String, key: String, asset: String },
    Kept { id: usize, ask: String, key: String, asset: String, said: String },
    Send { id: usize, key: String, ask: Arc<Ask>, doc: Arc<Doc>, stand: Arc<Vec<Standing>>, lock: Arc<Lock>, next: Next },
    Tool { id: usize, ask: Arc<Ask>, doc: Arc<Doc>, stand: Arc<Vec<Standing>>, lock: Arc<Lock>, i: usize },
}

/// What a walk did not make, and how to ask for it again.
#[derive(Default)]
struct Missed {
    failed: u32,
    unsent: u32,
    names: BTreeSet<String>,
    /// A render of `all` was among them.
    whole: bool,
    /// A render that takes out-of-date pins too was among them.
    outdated: bool,
    on: Vec<On>,
}

/// A run followed to its end: its key, the asset, inputs and number it was a try at if it was still owed, and what landing it came to.
struct Followed {
    key: String,
    name: Option<String>,
    inputs: Option<String>,
    run: Option<u64>,
    landed: Result<Result<(), Error>, Error>,
}

impl Walk {
    fn new(api: &Arc<Api>, project: &Arc<Project>) -> Self {
        Self {
            host: Host::telling(api, project.events.clone()),
            project: project.clone(),
            began: Instant::now(),
            tally: Mutex::default(),
            cut: Mutex::default(),
            missed: Mutex::default(),
            faults: Mutex::default(),
            stopped: Mutex::default(),
            measuring: AtomicBool::new(false),
            holding: AtomicBool::new(false),
        }
    }

    /// The walk's first failure as its last word when it made or missed more than that one: how far it got, and the command for the rest.
    fn last_word(&self, first: Error) -> Error {
        let tally = self.tally.lock().expect("tally lock");
        let missed = self.missed.lock().expect("missed lock");
        let made = tally.takes + tally.versions;
        let total = made + missed.failed + missed.unsent;
        if total <= 1 {
            return first;
        }
        let unsent = match missed.unsent {
            0 => String::new(),
            n => format!(", {n} not sent"),
        };
        let said = format!("rendered {made} of {total}; {} failed{unsent}: {}", missed.failed, first.message);
        let what = match missed.whole || missed.names.is_empty() {
            true if missed.outdated => "all --missing --outdated".to_owned(),
            true => "all --missing".to_owned(),
            false => missed.names.iter().cloned().collect::<Vec<_>>().join(" "),
        };
        let on = missed.on.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let mut error = Error::new(first.kind, "render_incomplete", said).cause(first.code).next(format!("cozy project render {what} --on {on}"));
        error.next.extend(first.next.into_iter().take(1));
        error
    }

    /// Records what `ask` is, for the last word.
    fn asked_of(&self, ask: &Ask) {
        let mut missed = self.missed.lock().expect("missed lock");
        missed.whole |= ask.all || ask.whole;
        missed.outdated |= ask.outdated;
        for on in &ask.target.on {
            if !missed.on.contains(on) {
                missed.on.push(on.clone());
            }
        }
    }

    /// One thing it did not send, because every machine it could go to stopped.
    fn not_sent(&self, name: &str) {
        let mut missed = self.missed.lock().expect("missed lock");
        missed.unsent += 1;
        missed.names.insert(name.to_owned());
        self.project.jobs.ended(Job { asset: name.to_owned(), state: JobState::NotSent, ..Job::default() });
    }

    /// A run failed `code`, saying `why`, on `machine`. The second alike stops what this walk's renders send there and cancels
    /// what they sent that has not begun: whatever is wrong there, the next run would pay to find it again.
    fn fault(&self, machine: &str, code: &str, why: &str) -> Result<(), Error> {
        let seen = {
            let mut faults = self.faults.lock().expect("faults lock");
            let seen = faults.entry((machine.to_owned(), code.to_owned(), why.to_owned())).or_default();
            *seen += 1;
            *seen
        };
        if seen < 2 {
            return Ok(());
        }
        let lock = self.folder().lock()?;
        let asks: BTreeSet<String> = lock.walk.asks.iter().map(|ask| ask.id.clone()).collect();
        let fresh = {
            let mut stopped = self.stopped.lock().expect("stopped lock");
            let fresh = !stopped.contains_key(machine);
            stopped.entry(machine.to_owned()).or_default().extend(asks.iter().cloned());
            fresh
        };
        if fresh {
            self.say(Event::Warning { message: format!("{machine}: {seen} runs failed the same way, so this render sends it nothing more: {why}"), name: None });
        }
        for pending in lock.pending.values().filter(|pending| pending.run > 0 && asks.contains(&pending.ask)) {
            self.host.cancel_unstarted(pending.run, machine);
        }
        Ok(())
    }

    /// Which of `on` render `ask` may still send to.
    fn sending(&self, ask: &str, on: &[On]) -> Vec<On> {
        let stopped = self.stopped.lock().expect("stopped lock");
        if stopped.is_empty() {
            return on.to_vec();
        }
        let stops = |machine: &str| stopped.get(machine).is_some_and(|asks| asks.contains(ask));
        (on.iter())
            .filter(|on| match on {
                On::Machine(machine) => !stops(machine),
                // Whichever machine it would be may be the one.
                _ => !stopped.keys().any(|machine| stops(machine)),
            })
            .cloned()
            .collect()
    }

    /// Sizes an adapter reads are ffprobe's: with files to measure and none here, this computer's first need fetches it, or says what goes without.
    async fn measure_first(&self) {
        self.say(Event::Said { message: "fetching ffmpeg and ffprobe, which measure this project's files".into(), name: None });
        if let Err(error) = self.host.ffmpeg().await {
            self.say(Event::Warning { message: format!("adapters see no sizes for this project's files: {}: cozy doctor", error.message), name: None });
        }
    }

    /// What it landed and how long it took, if it landed anything.
    fn rendered(&self) -> Option<Rendered> {
        let tally = self.tally.lock().expect("tally lock").clone();
        (tally.takes + tally.versions > 0).then(|| Rendered { elapsed_ms: self.began.elapsed().as_millis() as u64, ..tally })
    }

    fn folder(&self) -> &Folder {
        &self.project.folder
    }

    /// What has landed, measured: a cut and an adapter read those sizes, and a film is judged by its frames.
    fn measure(&self) -> Result<(), Error> {
        self.folder().measure(&mut self.folder().lock()?)
    }

    /// The project as the render `ask` was asked of it; one from no render in the lock, as the file reads now.
    fn doc_of(&self, ask: &str) -> Result<Doc, Error> {
        self.measure()?;
        match self.folder().walk_now()?.walk.asks.iter().find(|mine| mine.id == ask) {
            Some(mine) => Doc::read(self.folder(), &self.folder().text_of(mine)?),
            None => Doc::load(self.folder()),
        }
    }

    /// A render has nothing more that can go: each cut it ends on is said, and it is done.
    async fn finish(&self, id: &str) -> Result<(), Error> {
        let asks = self.folder().walk_now()?.walk.asks;
        if let Some(ask) = asks.iter().find(|ask| ask.id == id)
            && let Ok(doc) = self.folder().text_of(ask).and_then(|text| Doc::read(self.folder(), &text))
            && (0..doc.assets.len()).any(|i| doc.tool(i) == Some(Tool::Cut))
        {
            self.measure()?;
            let lock = self.folder().lock()?;
            let stand = doc.stand(&lock);
            self.say_cuts(&doc, &stand).await?;
        }
        if let Some(digest) = asks.iter().find(|ask| ask.id == id).map(|ask| &ask.digest) {
            if !asks.iter().any(|ask| ask.id != id && &ask.digest == digest) {
                self.project.walker.lock().expect("walker lock").readings.remove(digest);
                self.folder().drop_text(digest);
            }
        }
        self.folder().update_walk(|walk| {
            walk.walk.asks.retain(|ask| ask.id != id);
            if walk.walk.asks.is_empty() {
                walk.walk.asked.clear();
            }
        })
    }

    /// `run` sent under `key`. A machine that answers no connect has recorded nothing, so it is asked
    /// again under that key; one its Hub says stopped or ended is refused at once.
    async fn send_run(&self, key: &str, run: SubmitRun) -> Result<Run, Error> {
        self.again(|| self.host.submit(key, run.clone())).await
    }

    /// `ask`, and again while its machine answers no connect or the Hub doesn't answer, each attempt said. Everything asked this way
    /// is a read, or goes under a key.
    async fn again<T, F: Future<Output = Result<T, Error>>>(&self, ask: impl Fn() -> F) -> Result<T, Error> {
        const WAITS: [u64; 4] = [3, 6, 12, 24];
        let again = |e: &Error| e.code == "machine_unreachable" || e.hub_transient();
        for (attempt, wait) in WAITS.iter().enumerate() {
            match ask().await {
                Err(e) if again(&e) => {
                    let wait = e.wait_or(std::time::Duration::from_secs(*wait));
                    let message = format!("{}; attempt {}/{} in {} s", e.message, attempt + 2, WAITS.len() + 1, wait.as_secs());
                    self.say(Event::Warning { message, name: None });
                    tokio::time::sleep(wait).await;
                }
                answer => return answer,
            }
        }
        ask().await.map_err(|e| match again(&e) {
            true => Error { message: format!("{}, after {} attempts", e.message, WAITS.len() + 1), ..e },
            false => e,
        })
    }

    fn say(&self, event: Event) {
        self.project.jobs.told(&event);
        if matches!(event, Event::Started { .. }) {
            self.sync();
        }
        let _ = self.project.events.send(event);
    }

    /// The board as project.lock stands now.
    fn sync(&self) {
        if let Err(error) = sync_jobs(&self.project, |number| self.host.numbered(number)) {
            warn!("the board of {}: {}", self.folder().dir.display(), error.message);
        }
    }

    /// `name` stands as it is: no new version comes of its render.
    fn stands(&self, name: &str, noun: &str, number: u32, why: String) {
        let path = self.folder().lock().ok().and_then(|lock| main_path(&lock, &lock.asset(name)?.version(number)?.files));
        self.project.jobs.ended(Job { asset: name.to_owned(), state: JobState::Done, noun: noun.to_owned(), number: Some(number), why: Some(why), path, ..Job::default() });
    }

    /// Gives each ready job its turn as it comes up, each sending, making or following on its own so none holds up another, and goes on
    /// as each ends or something is asked, until nothing is owed: then the first thing that failed, if one did.
    async fn go(&self) -> Result<Option<Error>, Error> {
        let (mut first, mut followed) = (None, HashSet::new());
        let mut work: FuturesUnordered<BoxFuture<'_, Done>> = FuturesUnordered::new();
        loop {
            self.joined(&mut first).await?;
            while self.turns(&mut work, &mut first)? {}
            for (key, pending) in self.folder().walk_now()?.pending {
                if followed.insert(key.clone()) {
                    let job = pending.job_of(&key).to_owned();
                    work.push(async move { Done::Landed { job, landed: self.follow(key).await } }.boxed());
                }
            }
            for wanted in self.wanted_ahead() {
                work.push(async move {
                    if let Err(error) = self.ask_ahead(wanted).await {
                        warn!("fetching ahead in {}: {}", self.folder().dir.display(), error.message);
                    }
                    Done::Ahead
                }.boxed());
            }
            self.finish_asks().await?;
            self.sync();
            self.ahead_unneeded();
            if work.is_empty() {
                // What is going with nothing following it has ended.
                {
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    let going: Vec<usize> = (0..walker.graph.jobs.len()).filter(|&id| walker.graph.jobs[id].state == walker::State::Going).collect();
                    going.into_iter().for_each(|id| walker.graph.end(id, End::Done));
                    if walker.graph.has_ready() {
                        continue;
                    }
                }
                for ask in self.folder().walk_now()?.walk.asks {
                    self.finish(&ask.id).await?;
                }
                if self.over()? {
                    return Ok(first);
                }
                continue;
            }
            tokio::select! {
                Some(done) = work.next() => self.done(done, &mut followed, &mut first)?,
                () = self.project.asked.notified() => {}
            }
            while let Some(Some(done)) = work.next().now_or_never() {
                self.done(done, &mut followed, &mut first)?;
            }
        }
    }

    /// What jobs waiting for their turn will need that no machine was asked for. Only where the machine is known: with several it is chosen when a job goes.
    fn wanted_ahead(&self) -> Vec<Wanted> {
        let Ok(walk) = self.folder().walk_now() else { return vec![] };
        let mut walker = self.project.walker.lock().expect("walker lock");
        let Walker { graph, readings, asked_ahead, .. } = &mut *walker;
        let mut wanted = vec![];
        for id in graph.open_jobs() {
            let job = &graph.jobs[id];
            if job.state != walker::State::Waiting || matches!(job.kind, JobKind::Free { .. }) {
                continue;
            }
            let Some(ask) = walk.walk.asks.iter().find(|ask| ask.id == job.ask) else { continue };
            let Some(reading) = readings.get(&ask.digest) else { continue };
            let [machine] = ask.target.on.as_slice() else { continue };
            let asset = &reading.doc.assets[job.node];
            let (with, kind) = (reading.doc.target(job.node), asset.kind.clone());
            let chosen = serde_json::to_string(&(asset.body.get("models"), asset.body.get("lora"))).unwrap_or_default();
            if asked_ahead.insert((machine.to_string(), with.clone(), chosen)) {
                wanted.push(Wanted { machine: machine.clone(), hub: ask.target.hub.clone(), with, kind, body: asset.body.clone() });
            }
        }
        wanted
    }

    /// Asks a machine for a function's code and weights, in the background: a warm run, which holds no GPU and starts no executor. The
    /// function is what the text names, else the first of its package that makes this kind. Nothing here can fail a render.
    async fn ask_ahead(&self, wanted: Wanted) -> Result<(), Error> {
        let Wanted { machine, hub, with, kind, body } = wanted;
        let on = [machine];
        let package = self.host.package(&hub, &with, &self.folder().dir, &on).await?;
        let Some(renderer) = package.first_making(&kind) else { return Ok(()) };
        let placed = self.host.place(&on, &renderer);
        let mut payload = Map::new();
        for field in ["models", "lora"] {
            if let Some(value) = body.get(field).filter(|value| !value.is_null()) {
                payload.insert(field.into(), value.clone());
            }
        }
        let called = renderer.target();
        let mut choices = Choices::new(&called, &renderer.function, &renderer.slots, renderer.calls.iter().collect());
        choices.take(&mut payload)?;
        let models = choices::wire(&choices.into_choices())?;
        let (source, inputs) = match &renderer.local {
            None => {
                if let On::Machine(name) = &placed {
                    self.host.release_ready(name, &renderer.package, &renderer.release, &hub).await?;
                }
                (v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() }), vec![])
            }
            Some(path) => {
                let prepared = self.host.ready(path, &placed, &hub).await?;
                (v1::Source::Local(prepared.source), prepared.files)
            }
        };
        let spec = v1::RunSpec { kind: v1::RunKind::Warm, source: Some(source), entrypoint: renderer.function.clone(), models, ..Default::default() };
        let target = format!("{}{called}", protocol::project::AHEAD);
        let shown = package.name.clone() + "/" + &renderer.function;
        let submit = SubmitRun { on: placed.clone(), hub, kind: RunKind::Warm, target, spec, inputs, held: vec![], out: None, warnings: vec![], agreed: None };
        let key = format!("project-ahead-{}", new_key());
        let run = self.send_run(&key, submit).await?;
        self.project.jobs.ahead(run.id.clone(), shown, placed.to_string(), run.number);
        Ok(())
    }

    /// What it fetches ahead for jobs that all ended: it needn't go on.
    fn ahead_unneeded(&self) {
        let needed: HashSet<(String, String)> = {
            let walk = self.folder().walk_now().unwrap_or_default();
            let walker = self.project.walker.lock().expect("walker lock");
            let graph = &walker.graph;
            (graph.open_jobs().into_iter().filter(|&id| graph.jobs[id].state == walker::State::Waiting))
                .filter_map(|id| {
                    let job = &graph.jobs[id];
                    let ask = walk.walk.asks.iter().find(|ask| ask.id == job.ask)?;
                    let doc = &walker.readings.get(&ask.digest)?.doc;
                    Some(ask.target.on.iter().map(|on| (on.to_string(), format::plain_with(&doc.target(job.node), self.folder()))).collect::<Vec<_>>())
                })
                .flatten()
                .collect()
        };
        for (function, machine, run) in self.project.jobs.ahead_going() {
            let package = function.rsplit_once('/').map_or(function.as_str(), |(package, _)| package);
            let wanted = needed.iter().any(|(on, with)| *on == machine && (with == &function || with == package || with.rsplit('/').next() == package.rsplit('/').next()));
            if !wanted {
                self.host.cancel_ahead(&run);
            }
        }
    }

    /// Takes in what was asked since: said once, and what can't be read ends there.
    async fn joined(&self, first: &mut Option<Error>) -> Result<(), Error> {
        let walk = self.folder().walk_now()?;
        let new = {
            let walker = self.project.walker.lock().expect("walker lock");
            walk.walk.asks.iter().any(|ask| !walker.joined.contains(&ask.id)) || !walker.greet.is_empty() || !walker.unread.is_empty()
        };
        if !new {
            return Ok(());
        }
        self.project.join(&self.folder().lock()?);
        let (greet, unread) = {
            let mut walker = self.project.walker.lock().expect("walker lock");
            (std::mem::take(&mut walker.greet), std::mem::take(&mut walker.unread))
        };
        for ask in walk.walk.asks.iter().filter(|ask| greet.contains(&ask.id)) {
            self.asked_of(ask);
            let warnings = {
                let walker = self.project.walker.lock().expect("walker lock");
                walker.readings.get(&ask.digest).map(|reading| reading.doc.clone())
            };
            for warning in warnings.iter().flat_map(|doc| &doc.warnings) {
                self.say(Event::Warning { message: warning.message.clone(), name: warning.name.clone() });
            }
        }
        for (ask, error) in unread {
            self.failed(error, first);
            self.finish(&ask).await?;
        }
        Ok(())
    }

    /// The asks with nothing left, done.
    async fn finish_asks(&self) -> Result<(), Error> {
        let walk = self.folder().walk_now()?;
        for ask in &walk.walk.asks {
            let done = {
                let walker = self.project.walker.lock().expect("walker lock");
                walker.joined.contains(&ask.id) && !walker.graph.owes(&ask.id)
            };
            if done && !walk.pending.values().any(|pending| pending.ask == ask.id) {
                self.finish(&ask.id).await?;
            }
        }
        Ok(())
    }

    /// The asset names project.yaml holds now, read again only when it changed. None: it doesn't read, so nothing is known gone.
    fn names_now(&self, held: &mut Option<(String, HashSet<String>)>) -> Option<HashSet<String>> {
        let path = self.folder().project();
        let meta = std::fs::metadata(&path).ok()?;
        let stamp = format!("{:?}-{}", meta.modified().ok(), meta.len());
        if held.as_ref().is_none_or(|(was, _)| *was != stamp) {
            let text = self.folder().text().ok()?;
            let names: HashSet<String> = protocol::yaml::keys(&text, &["assets"]).iter().map(|name| name_key(name)).collect();
            *held = Some((stamp, names));
        }
        held.as_ref().map(|(_, names)| names.clone()).filter(|names| !names.is_empty())
    }

    /// The ready jobs, as many as there is room for, each given its turn; whether there were any.
    fn turns<'a>(&'a self, work: &mut FuturesUnordered<BoxFuture<'a, Done>>, first: &mut Option<Error>) -> Result<bool, Error> {
        if self.holding.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let room = {
            let walker = self.project.walker.lock().expect("walker lock");
            if !walker.graph.has_ready() {
                return Ok(false);
            }
            WINDOW.saturating_sub(walker.graph.going())
        };
        if room == 0 {
            return Ok(false);
        }
        let held = self.folder().lock()?;
        if self.folder().wants_ffprobe(&held) && !self.measuring.swap(true, Ordering::Relaxed) {
            self.holding.store(true, Ordering::Relaxed);
            work.push(async {
                self.measure_first().await;
                Done::Measured
            }.boxed());
            return Ok(false);
        }
        self.measure()?;
        let lock = Arc::new(self.folder().lock()?);
        let mut asks: HashMap<String, Arc<Ask>> = HashMap::new();
        let mut acts = vec![];
        {
            let mut walker = self.project.walker.lock().expect("walker lock");
            let Walker { graph, readings, names, .. } = &mut *walker;
            let names = self.names_now(names);
            for id in graph.take_ready(room) {
                let (key, ask_id, node, kind, asset) = {
                    let job = &graph.jobs[id];
                    (job.key.clone(), job.ask.clone(), job.node, job.kind, job.asset.clone())
                };
                if !asks.contains_key(&ask_id)
                    && let Some(ask) = lock.walk.asks.iter().find(|ask| ask.id == ask_id)
                {
                    asks.insert(ask_id.clone(), Arc::new(ask.clone()));
                }
                let Some(ask) = asks.get(&ask_id).cloned() else {
                    graph.end(id, End::Dropped);
                    continue;
                };
                let Some(reading) = readings.get_mut(&ask.digest) else {
                    graph.end(id, End::Dropped);
                    continue;
                };
                // Read with tags it didn't know: a package has been read since.
                if !reading.doc.knows_tags_of(&lock)
                    && let Ok(doc) = self.folder().text_of(&ask).and_then(|text| Doc::read(self.folder(), &text))
                {
                    *reading = Reading::new(doc);
                }
                let gone = |name: &str| names.as_ref().is_some_and(|names| !names.contains(&name_key(name)));
                if gone(&asset) {
                    acts.push(Act::Deleted { id, ask: ask_id, key, asset });
                    continue;
                }
                let turn = graph.turn(id, &mut Now { reading, lock: &lock, ask: &ask });
                let noun = if reading.nodes[node].video { "take" } else { "version" };
                let row_of = |reading: &mut Reading| {
                    reading.ensure(node, &lock);
                    reading.done[node].clone()
                };
                match turn {
                    Turn::Wait => {}
                    Turn::Stands => {
                        let number = row_of(reading).version();
                        acts.push(Act::Stands { id, ask: ask_id, key, asset, noun, number });
                    }
                    Turn::Unread { read } => {
                        let standing = row_of(reading);
                        if kind == JobKind::Judged {
                            acts.push(Act::Stands { id, ask: ask_id, key, asset, noun, number: standing.version() });
                            continue;
                        }
                        let doc = reading.doc.clone();
                        let stand = reading.cone(node, &lock);
                        let why = match gone(&reading.nodes[read].name) && !stand[read].files.is_some() {
                            true => format!("{} was deleted", reading.nodes[read].name),
                            false => unread(&doc, &stand, &lock, &ask, node),
                        };
                        let message = format!("{asset} cannot render: {why}");
                        let kind = doc.assets[node].kind.clone();
                        acts.push(Act::Unread { id, ask: ask_id, on: ask.target.on.clone(), key, asset, kind, inputs: standing.inputs, message });
                    }
                    Turn::Go => {
                        graph.go(id);
                        let doc = reading.doc.clone();
                        let stand = Arc::new(reading.cone(node, &lock));
                        let standing = &stand[node];
                        match kind {
                            JobKind::Free { named } => {
                                let makes = doc.makes(node) && standing.inputs.is_some() && (named || standing.files.is_none());
                                match makes {
                                    true => acts.push(Act::Tool { id, ask, doc, stand, lock: lock.clone(), i: node }),
                                    false => {
                                        let number = standing.version();
                                        acts.push(Act::Stands { id, ask: ask_id, key, asset, noun, number });
                                    }
                                }
                            }
                            JobKind::Judged => {
                                if outdated_now(&doc, &stand, &ask, node) {
                                    let next = Next { i: node, key: key.clone(), take: None, unpin: true };
                                    acts.push(Act::Send { id, key, ask, doc, stand, lock: lock.clone(), next });
                                } else if pin_changed(&stand, node) {
                                    let why = doc.tags(node).and_then(|tags| doc.missing_inputs(node, tags)).unwrap_or_default();
                                    let why = why.strip_prefix(&format!("{asset}: ")).unwrap_or(&why);
                                    let changed = match &standing.state {
                                        Stand::Pinned { changed, .. } => changed.join(", "),
                                        _ => String::new(),
                                    };
                                    let said = format!("{asset} stays pinned, though {changed} changed: {why}");
                                    acts.push(Act::Kept { id, ask: ask_id, key, asset, said });
                                } else {
                                    let number = standing.version();
                                    acts.push(Act::Stands { id, ask: ask_id, key, asset, noun, number });
                                }
                            }
                            JobKind::Owed | JobKind::Take => {
                                let take = ask.takes.iter().find(|take| take.key == key).cloned();
                                let next = Next { i: node, key: key.clone(), take, unpin: false };
                                acts.push(Act::Send { id, key, ask, doc, stand, lock: lock.clone(), next });
                            }
                        }
                    }
                }
            }
        }
        for act in acts {
            self.act(act, work, first)?;
        }
        Ok(true)
    }

    /// `asset` was tried by render `ask`, whichever way it went: it isn't owed again, a restart too.
    fn settled(&self, ask: &str, asset: &str, key: &str) -> Result<(), Error> {
        let name = name_key(asset);
        self.folder().update_walk(|walk| {
            for mine in walk.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
                mine.sent.insert(name.clone());
                mine.takes.retain(|take| take.key != key);
            }
        })
    }

    fn act<'a>(&'a self, act: Act, work: &mut FuturesUnordered<BoxFuture<'a, Done>>, first: &mut Option<Error>) -> Result<(), Error> {
        let end = |id: usize, how: End| self.project.walker.lock().expect("walker lock").graph.end(id, how);
        match act {
            Act::Stands { id, ask, key, asset, noun, number } => {
                self.settled(&ask, &asset, &key)?;
                match number {
                    Some(number) => self.stands(&asset, noun, number, jobs::STANDS.into()),
                    None => self.project.jobs.ended(Job { id: key, asset, state: JobState::Done, noun: noun.into(), why: Some("nothing to do".into()), ..Job::default() }),
                }
                end(id, End::Stands);
            }
            Act::Kept { id, ask, key, asset, said } => {
                self.say(Event::Said { message: said, name: Some(asset.clone()) });
                self.settled(&ask, &asset, &key)?;
                self.project.jobs.ended(Job { id: key, asset, state: JobState::Done, noun: "version".into(), why: Some("stays pinned".into()), ..Job::default() });
                end(id, End::Stands);
            }
            Act::Deleted { id, ask, key, asset } => {
                self.settled(&ask, &asset, &key)?;
                let why = format!("{asset} was deleted while it waited, so it isn't made");
                self.say(Event::Said { message: why.clone(), name: Some(asset.clone()) });
                self.project.jobs.ended(Job { id: key, asset, state: JobState::Canceled, why: Some(why), ..Job::default() });
                end(id, End::Dropped);
            }
            Act::Unread { id, ask, on, key, asset, kind, inputs, message } => {
                self.settled(&ask, &asset, &key)?;
                match self.sending(&ask, &on).is_empty() {
                    true => self.not_sent(&asset),
                    false => self.failed_of(&asset, &kind, inputs, None, Error::new(Kind::NotFound, "file_missing", message), first),
                }
                end(id, End::Failed);
            }
            Act::Send { id, key, ask, doc, stand, lock, next } => {
                let (name, kind, inputs) = (doc.assets[next.i].name.clone(), doc.assets[next.i].kind.clone(), stand[next.i].inputs.clone());
                work.push(async move {
                    let result = self.send(&ask, &doc, &stand, &lock, next).await;
                    Done::Sent { id, key, ask, name, kind, inputs, result }
                }.boxed());
            }
            Act::Tool { id, ask, doc, stand, lock, i } => {
                let (name, kind, inputs) = (doc.assets[i].name.clone(), doc.assets[i].kind.clone(), stand[i].inputs.clone());
                work.push(async move {
                    let result = self.tool(&ask, &doc, &stand, &lock, i).await;
                    Done::Made { id, ask: ask.id.clone(), name, kind, inputs, result }
                }.boxed());
            }
        }
        Ok(())
    }

    /// Takes in what ended: what reads it can have its turn.
    fn done(&self, done: Done, followed: &mut HashSet<String>, first: &mut Option<Error>) -> Result<(), Error> {
        let ended = |id: usize, how: End, name: &str| {
            let mut walker = self.project.walker.lock().expect("walker lock");
            walker.graph.end(id, how);
            walker.readings.values_mut().for_each(|reading| reading.moved(name));
        };
        match done {
            Done::Measured => self.holding.store(false, Ordering::Relaxed),
            Done::Ahead => {}
            Done::Sent { id, key, ask, name, kind, inputs, result } => match result {
                Ok(()) => {
                    // No run came of it: it stands, it had no adapter, or it was canceled while being sent.
                    if !self.folder().walk_now()?.pending.iter().any(|(sent, pending)| pending.job_of(sent) == key) {
                        ended(id, End::Done, &name);
                        self.project.jobs.ended_if_open(&key, &name);
                    }
                }
                Err(error) => {
                    self.send_failed(&ask, &name, &error)?;
                    self.failed_of(&name, &kind, inputs, None, error, first);
                    ended(id, End::Failed, &name);
                }
            },
            Done::Made { id, ask, name, kind, inputs, result } => {
                let key = name_key(&name);
                self.folder().update_walk(|walk| {
                    for mine in walk.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
                        mine.takes.retain(|take| name_key(&take.asset) != key);
                    }
                })?;
                let job = self.project.walker.lock().expect("walker lock").graph.jobs[id].key.clone();
                match result {
                    Ok(()) => {
                        ended(id, End::Done, &name);
                        self.project.jobs.ended_if_open(&job, &name);
                    }
                    Err(error) => {
                        self.failed_of(&name, &kind, inputs, None, error, first);
                        self.tried(&ask, &name)?;
                        ended(id, End::Failed, &name);
                    }
                }
            }
            Done::Landed { job, landed } => {
                let failed = landed.landed.as_ref().is_ok_and(Result::is_err);
                let asset = landed.name.clone();
                self.landed(landed, followed, first)?;
                // Its next call, if its operation has one, goes on under its own key.
                let goes_on = self.folder().walk_now()?.pending.iter().any(|(sent, pending)| pending.job_of(sent) == job);
                if !goes_on {
                    let id = self.project.walker.lock().expect("walker lock").graph.id(&job);
                    if let Some(id) = id {
                        ended(id, if failed { End::Failed } else { End::Done }, asset.as_deref().unwrap_or_default());
                    }
                }
            }
        }
        Ok(())
    }

    /// A followed run came to its end: it is followed no more, and what failed is said as its asset's and kept on it.
    fn landed(&self, landed: Followed, followed: &mut HashSet<String>, first: &mut Option<Error>) -> Result<(), Error> {
        followed.remove(&landed.key);
        if let Err(error) = landed.landed? {
            match &landed.name {
                Some(name) => self.failed_of(name, "", landed.inputs, landed.run, error, first),
                None => self.failed(error, first),
            }
        }
        Ok(())
    }

    /// `name` could not be sent: when that is its machines' doing, not its own, it counts as what they failed.
    fn send_failed(&self, ask: &Ask, name: &str, error: &Error) -> Result<(), Error> {
        self.missed.lock().expect("missed lock").names.insert(name.to_owned());
        if error.kind != Kind::Unavailable {
            return Ok(());
        }
        let code = error.cause.as_deref().unwrap_or(&error.code);
        let why = error.message.strip_prefix(name).unwrap_or(&error.message);
        for on in &ask.target.on {
            if let On::Machine(machine) = on {
                self.fault(machine, code, why)?;
            }
        }
        Ok(())
    }

    /// Whether the walk is over, nothing owed or followed. Decided under `walking`, so an ask finds
    /// this walker going or starts the next.
    fn over(&self) -> Result<bool, Error> {
        let mut walking = self.project.walking.lock().expect("walking lock");
        let lock = self.folder().lock()?;
        *walking = !lock.walk.asks.is_empty() || !lock.pending.is_empty();
        if !*walking && !lock.walk.renames.is_empty() {
            self.folder().update_lock(|lock| {
                for (from, to) in std::mem::take(&mut lock.walk.renames) {
                    // A render of it since made versions of its own: nothing overwrites those.
                    match lock.asset(&to).is_some_and(|held| !held.versions.is_empty()) {
                        true => self.say(Event::Warning { message: format!("{from} stays {from}: {to} has versions of its own now"), name: None }),
                        false => renamed(lock, &from, &to),
                    }
                }
            })?;
        }
        if !*walking {
            *self.project.walker.lock().expect("walker lock") = Walker::default();
            self.folder().drop_texts();
        }
        Ok(!*walking)
    }

    /// What failed, of no one asset: said now; what does not read it goes on.
    fn failed(&self, error: Error, first: &mut Option<Error>) {
        self.missed.lock().expect("missed lock").failed += 1;
        self.say(Event::Failed { error: error.clone(), name: None, run: None });
        first.get_or_insert(error);
    }

    /// What failed of `name`, by `run` if it is of one: said now, kept for whoever reads it later, never sent again this walk; what does not read
    /// it goes on. Canceling is said, and isn't a failure of the asset.
    fn failed_of(&self, name: &str, kind: &str, inputs: Option<String>, run: Option<u64>, error: Error, first: &mut Option<Error>) {
        {
            let mut missed = self.missed.lock().expect("missed lock");
            missed.failed += 1;
            missed.names.insert(name.to_owned());
        }
        if error.code != CANCELED {
            let failed = Failed { inputs, error: error.clone() };
            if let Err(error) = self.folder().update_lock(|lock| lock.asset_mut(name, kind).failed = Some(failed.clone())) {
                warn!("{name}: {error}");
            }
        }
        self.say(Event::Failed { error: error.clone(), name: Some(name.to_owned()), run });
        first.get_or_insert(error);
    }

    /// Each current cut at the walk's end, what a render ends on, its film back at the root if someone moved it.
    async fn say_cuts(&self, doc: &Doc, stand: &[Standing]) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        for i in (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)) {
            let name = cut_name(&doc.assets[i].name);
            let Some(file) = stand[i].files.as_ref().and_then(|files| files.get("video")).and_then(|sha256| lock.files.get(sha256)) else { continue };
            self.film_follows(doc, stand, &lock, i).await;
            if self.cut.lock().expect("cut lock").contains(&name) {
                continue;
            }
            let row = cut_row(doc, self.folder(), &lock, stand, i);
            self.say(Event::Cut { name: doc.assets[i].name.clone(), cut: name, frames: row.frames.unwrap_or(file.frames), fps: row.fps, retimed: row.retimed, took_s: 0.0, unchanged: true, without: row.without });
        }
        Ok(())
    }

    /// A standing cut's `.otio`, written again from its pieces; one that can't be says so and the walk goes on.
    async fn timeline_again(&self, asset: &format::Asset, standing: &Standing, lock: &Lock, name: &str) {
        let canvas = Canvas::of(&asset.fields, &mut vec![]);
        let (mut pieces, _) = tools::pieces(&asset.fields, &standing.reads, self.folder(), lock);
        if let Some(version) = standing.version().and_then(|number| lock.asset(&asset.name)?.version(number)) {
            pieces.retain(|piece| !version.without.iter().any(|left| name_key(&left.piece) == name_key(&piece.asset)));
        }
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
            self.say(Event::Warning { message: format!("{name}'s timeline is gone and isn't written again: {}", error.message), name: None });
        }
    }

    /// Sends `next` under its key: a new version of its asset on its target's machines, recorded as pending together with what
    /// it settles of the walk, before the run is sent; one that fails is settled all the same. An adapter that answers no run
    /// lands it at once, the asset's files being what it reads.
    async fn send(&self, ask: &Ask, doc: &Doc, stand: &[Standing], lock: &Lock, next: Next) -> Result<(), Error> {
        let i = next.i;
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        let key = next.key.clone();
        let settle = |lock: &mut Lock| {
            let Some(mine) = lock.walk.asks.iter_mut().find(|mine| mine.id == ask.id) else { return };
            // Tried once either way: a take that fails isn't owed again for what reads it. Only a new render asks again.
            mine.sent.insert(name_key(&name));
            if next.take.is_some() {
                mine.takes.retain(|take| take.key != key);
            }
        };
        let (mut target, approve, new) = match &next.take {
            None => (ask.target.clone(), false, next.unpin),
            Some(take) => (take.target.clone(), take.approve, take.new),
        };
        target.on = self.sending(&ask.id, &target.on);
        if target.on.is_empty() {
            self.not_sent(&name);
            return self.folder().update_lock(settle);
        }
        let standing = &stand[i];
        let noun = noun(doc, lock, i);
        if !new && standing.files.is_some() {
            return self.folder().update_lock(settle);
        }
        match &standing.state {
            Stand::Stale(why) if !new => self.say(Event::Said { message: format!("{name}: out of date: {why}"), name: Some(name.clone()) }),
            Stand::Missing if !new => self.say(Event::Said { message: format!("{name}: no {noun} yet"), name: Some(name.clone()) }),
            Stand::Pinned { changed, .. } if next.unpin => self.say(Event::Said { message: format!("{name}: pinned, and {} changed: a new {noun} takes its place, and the one it held stays", changed.join(", ")), name: Some(name.clone()) }),
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
            let with = doc.target(i);
            let package = self.again(|| self.host.package(&target.hub, &with, &self.folder().dir, &target.on)).await?;
            if let Some(moved) = &package.moved {
                self.say(Event::Warning { message: moved.clone(), name: None });
            }
            read_tags(doc, self.folder(), i, &package);
            // Before anything that could cost: its adapter's run, or a function's.
            if let Some(why) = doc.missing_inputs(i, &package.reserved()) {
                return Err(Error::new(Kind::Validation, "input_missing", why).remedy(format!("bring it back, or edit {name}'s text")));
            }
            let Some(maker) = package.maker(&asset.kind)? else {
                self.say(Event::Warning { message: no_adapter(&name, &package.name, &asset.kind), name: None });
                return self.folder().update_lock(settle);
            };
            if let Some(made) = package.named_kind().filter(|made| *made != asset.kind) {
                let tool = doc.target(i);
                self.say(Event::Warning { message: format!("{name} is a {} and {tool} makes a {made}: write `kind: {made}`, and it can be made", asset.kind), name: None });
                return self.folder().update_lock(settle);
            }
            let now = self.folder().lock()?;
            let holds = now.asset(&name).and_then(|held| held.versions.iter().rev().find(|version| version.inputs == inputs)).map(|version| version.number);
            let going = now.pending.values().any(|pending| name_key(&pending.asset) == name_key(&name) && pending.version.inputs == inputs);
            let keeps = next.take.as_ref().and_then(|take| take.keeps);
            let keep = keeps.and_then(|keeps| doc.keep(i, lock, keeps));
            if keeps.is_some() && keep.is_none() {
                self.say(Event::Warning { message: format!("{name}: that take holds no video to keep: the whole {noun} renders"), name: None });
            }
            if let Some(keep) = &keep
                && let Some(file) = keep["latents"]["sha256"].as_str().and_then(|sha256| lock.files.get(sha256))
                && matches!(self.reach(file, &target.on).await, Reach::Gone)
            {
                let said = format!("{name}: take {}'s latents are gone from their machine: rendering again renders the whole shot", keep["take"]);
                let on = target.on.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
                return Err(Error::new(Kind::NotFound, "take_latents_gone", said).next(format!("cozy project render {name} --on {on}")));
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
            let (calls, said) = match &maker {
                Maker::Adapter(adapter) => {
                    if let Some(signature) = &adapter.signature {
                        // Every asset can have a description, for what mentions it, whatever makes it.
                        let unknown = (asset.own.iter()).filter(|field| *field != "description" && asset.fields.contains_key(*field) && !signature.contains(*field));
                        warnings.extend(unknown.map(|field| format!("{name}: {} reads no field {field}", adapter.target())));
                    }
                    let mut operation = doc.operation(i, package.named.as_deref(), &reads, lock, keep.clone(), stand);
                    let typed = package.callable(&adapter.function).and_then(|adapter| adapter.typed_fields().cloned());
                    if let (Some(typed), Some(fields)) = (typed, operation["fields"].as_object_mut()) {
                        warnings.extend(direct::conform(&name, &typed, fields));
                    }
                    self.adapt(adapter, &operation, &target.hub, &target.on).await?
                }
                Maker::Direct(renderer) => {
                    let unknown = || Error::new(Kind::NotFound, "callable_unknown", format!("{} has no function {}", package.name, renderer.function));
                    let callable = package.callable(&renderer.function).ok_or_else(unknown)?;
                    let (input, said) = direct::input(&name, &doc.target(i), &callable, &doc.node_fields(i), &reads, lock)?;
                    (vec![Invocation { function: renderer.function.clone(), input }], said)
                }
            };
            warnings.extend(said);
            // No call: this asset's files are what it reads.
            if calls.is_empty() {
                warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone(), name: None }));
                let Some(number) = holds else {
                    let mut version = self.version(doc, standing, i, warnings, inputs.clone());
                    if let Maker::Adapter(adapter) = &maker {
                        version.made_by = version.made_by.map(|by| MadeBy { with: adapter.target(), ..by });
                    }
                    self.land_read(&name, &asset.kind, version, standing, noun, approve, settle)?;
                    if next.unpin {
                        self.unpin_landed(&ask.id, &name, &asset.kind);
                    }
                    return Ok(());
                };
                self.stands(&name, noun, number, format!("{} has nothing to make", package.name));
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
                match holds {
                    Some(number) => self.stands(&name, noun, number, format!("{} {why}, so it has one result per inputs", package.name)),
                    None => self.say(Event::Said { message: format!("{name}: {} {why}, so it has one result per inputs: {stands}", package.name), name: Some(name.clone()) }),
                }
                return self.folder().update_lock(settle);
            }
            warnings.iter().for_each(|message| self.say(Event::Warning { message: message.clone(), name: None }));
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
            let mut calls = calls.into_iter();
            let first = calls.next().expect("at least one");
            let seed = version.seed;
            let pending = Pending { job: key.clone(), ask: ask.id.clone(), asset: name.clone(), version, run: 0, approve, unpin: next.unpin.then(|| asset.kind.clone()), target: target.clone(), rest: calls.collect(), done: vec![] };
            let settling = |lock: &mut Lock| {
                settle(lock);
                if let Some(seed) = seed {
                    lock.asset_mut(&name, &asset.kind).tried = Some(Tried { inputs: inputs.clone(), seed });
                }
                if let Some(held) = lock.assets.get_mut(&name_key(&name)) {
                    held.failed = None;
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
            // Its machines stopped while it was on its way.
            let mut pending = pending;
            pending.target.on = self.sending(&ask.id, &pending.target.on);
            if pending.target.on.is_empty() {
                self.not_sent(&name);
                return self.folder().update_lock(settle);
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
        match self.host.fetchable(&file.sha256, &file.machine).await {
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
                let prepared = self.again(|| self.host.ready(path, &placed, hub)).await?;
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
            let why = run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |r| r.message.clone());
            return Err(failed(why).next(format!("cozy run show {}", run.number)));
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
                    self.say(Event::Said { message: format!("{name}: {field} is read on {machine}, which holds it"), name: Some(name.clone()) });
                    holder = Some(machine);
                    on_machine = true;
                }
                Reach::Fetch => {
                    self.say(Event::Said { message: format!("{name}: fetching {field} from {}", file.machine), name: Some(name.clone()) });
                    let fetched = self.host.fetch(&digest, &file.machine).await;
                    fetched.map_err(|why| cannot(&name, format!("{field} is on {}, and can't be had: {why}", file.machine)))?;
                }
                Reach::Gone => return Err(cannot(&name, format!("{field} is gone from its machine {}", file.machine))),
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
            None => {
                if let On::Machine(machine) = &placed {
                    self.again(|| self.host.release_ready(machine, &renderer.package, &renderer.release, &hub)).await.map_err(named)?;
                }
                v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() })
            }
            Some(path) => {
                let prepared = self.again(|| self.host.ready(path, &placed, &hub)).await.map_err(named)?;
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
    async fn follow(&self, key: String) -> Followed {
        let owed = self.folder().lock().ok().and_then(|lock| lock.pending.get(&key).map(|pending| (pending.asset.clone(), pending.version.inputs.clone(), pending.run)));
        let (name, inputs, run) = match owed {
            Some((name, inputs, run)) => (Some(name), Some(inputs), Some(run)),
            None => (None, None, None),
        };
        let landed = async {
            let Some(run) = self.host.sent(&key)? else { return Ok(()) };
            let run = self.host.wait(&run.id).await?;
            self.land(&key, &run).await
        };
        let landed = landed.await;
        let landed = match self.folder().update_lock(|lock| drop(lock.pending.remove(&key))) {
            Ok(()) => Ok(landed),
            Err(error) => Err(error),
        };
        Followed { key, name, inputs, run: run.filter(|run| *run > 0), landed }
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
            // Canceled because its machine stopped: not sent, as far as anyone paid for it.
            if run.status == RunStatus::Canceled && run.started_at_ms.is_none() && run.machine.as_deref().is_some_and(|machine| self.stopped.lock().expect("stopped lock").contains_key(machine)) {
                self.not_sent(&name);
                return Ok(());
            }
            self.missed.lock().expect("missed lock").names.insert(name.clone());
            if let (Some(machine), Some(reason), RunStatus::Failed) = (&run.machine, &run.reason, run.status) {
                self.fault(machine, &reason.code, &reason.message)?;
            }
            let status = format!("{:?}", run.status).to_lowercase();
            let said = match &run.reason {
                Some(reason) => format!("{name}: {status}: {}", reason.message),
                None => format!("{name}: {status}"),
            };
            let code = if run.status == RunStatus::Canceled { CANCELED } else { "version_failed" };
            return Err(Error::new(Kind::Unavailable, code, said).next(format!("cozy run show {}", run.number)));
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
            if !self.project.jobs.warned(&name, &said) {
                self.say(Event::Warning { message: said, name: Some(name.clone()) });
            }
            if !version.warnings.contains(&message) {
                version.warnings.push(message);
            }
        }
        // Every file the run saved, by its output's name (a list's items as `<name>-<n>`).
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
            let with = doc.target(i);
            let package = self.again(|| self.host.package(&pending.target.hub, &with, &folder.dir, &pending.target.on)).await?;
            let carried = Pending { version, rest, done, run: 0, ..pending.clone() };
            let settle = |lock: &mut Lock| drop(lock.pending.remove(key));
            return self.submit(&new_key(), next, &package, &doc.assets[i], carried, &settle).await;
        }
        let kind = folder.lock()?.asset(&name).map(|held| held.kind.clone()).unwrap_or_default();
        let noun = if version.files.contains_key("video") { "take" } else { "version" };
        let key = key.to_owned();
        self.record(Some(&key), &name, &kind, version, noun, pending.approve, |_: &mut Lock| {})?;
        if let Some(field) = &pending.unpin {
            self.unpin_landed(&pending.ask, &name, field);
        }
        Ok(())
    }

    /// `name`'s re-run landed, so its pin goes: from project.yaml, and from the text its render plans on, so what reads it reads this.
    fn unpin_landed(&self, ask: &str, name: &str, field: &str) {
        if let Err(error) = self.folder().edit_project(|text| format::unpinned(text, name, field)) {
            self.say(Event::Warning { message: error.message, name: None });
        }
        let planned = self.folder().update_lock(|lock| {
            for mine in lock.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
                if let Ok(text) = format::unpinned(&mine.text, name, field) {
                    mine.digest = hex::encode(Sha256::digest(text.as_bytes()));
                    mine.text = text;
                }
            }
        });
        if let Err(error) = planned {
            warn!("{name}: {error}");
        }
    }

    /// The frame of `video` at `at` seconds, half a frame early so one at its end is its last frame, kept as a file of the
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
            self.say(Event::Warning { message: format!("{name}: its video ends before {at:.2} s: its last frame stands in"), name: None });
        }
        self.folder().keep(frame.path())
    }

    /// Makes tool `i` here, from what it reads now, and lists what it made. A frame keeps one version, free to make again, and its pinned one. A cut keeps
    /// every one with new bytes, as anything else does: the same pieces coming out the same make none.
    async fn tool(&self, ask: &Ask, doc: &Doc, stand: &[Standing], lock: &Lock, i: usize) -> Result<(), Error> {
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        let standing = &stand[i];
        let Some(inputs) = standing.inputs.clone() else { return Ok(()) };
        for read in &standing.reads {
            if let Some(number) = read.newest {
                let noun = noun(doc, lock, read.asset);
                self.say(Event::Warning { message: format!("{name} reads {}'s newest {noun}, {number}, which is out of date", read.name), name: None });
            }
        }
        let mut version = self.version(doc, standing, i, vec![], inputs);
        version.made_by = Some(MadeBy { with: match doc.tool(i) {
            Some(Tool::Cut) => EDIT.into(),
            _ => "cozy/frame".into(),
        }, ..Default::default() });
        match doc.tool(i).expect("a tool") {
            Tool::Cut => {
                // Once a walk: what it cuts stands, or says why not.
                self.tried(&ask.id, &name)?;
                let Some((files, without)) = self.cut_tool(doc, asset, standing, &ask.target, &version.inputs).await? else { return Ok(()) };
                version.without = without;
                version.clips = tools::clips(&asset.fields, &standing.reads);
                version.fps = Canvas::of(&asset.fields, &mut vec![]).fps;
                let now = self.folder().lock()?;
                let again = now.asset(&name).is_some_and(|held| held.versions.iter().any(|old| old.inputs == version.inputs && old.main() == folder::main_of(&files)));
                if again {
                    return Ok(());
                }
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

    /// Cuts `asset`'s pieces, its film and timeline at the root, and what it went without. A piece not here is fetched from the machine
    /// that keeps it, or the Hub. One that can't be had, has no version yet, or that ffprobe or ffmpeg can't take is left out, warned of
    /// and said with the film (Paul: "it'd generate the edit without them", "why would it ever fail the cut?"), and the film before it
    /// stays a version. None: the same edit stands, or no clip can be used, so there is no new version and the film that stands stays. Err is
    /// what isn't a clip's doing: ffmpeg that can't be had, or a full disk.
    async fn cut_tool(&self, doc: &Doc, asset: &format::Asset, standing: &Standing, target: &Target, inputs: &str) -> Result<Option<(Files, Vec<folder::Without>)>, Error> {
        let folder = self.folder();
        let mut said = vec![];
        let canvas = Canvas::of(&asset.fields, &mut said);
        // ffprobe first, as this may be its first need here, then every file measured.
        let (ffmpeg, ffprobe) = self.host.ffmpeg().await?;
        let lock = folder.lock()?;
        let (pieces, _) = tools::pieces(&asset.fields, &standing.reads, folder, &lock);
        let name = cut_name(&asset.name);
        let on = target.on.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let mut without = self.bring_pieces(asset, &pieces, &lock, target, &ffprobe, &on).await?;
        let mut lock = folder.lock()?;
        folder.measure(&mut lock)?;
        let (mut pieces, skips) = tools::pieces(&asset.fields, &standing.reads, folder, &lock);
        pieces.retain(|piece| !without.iter().any(|left| name_key(&left.piece) == name_key(&piece.asset)));
        for piece in skips {
            let (why, said) = match doc.find(&piece) {
                Some(_) => ("no version yet", format!("which has no version yet: cozy project render {piece} --on {on}")),
                None => ("no longer in the project", "which is no longer in the project".to_owned()),
            };
            self.say(Event::Warning { message: format!("{name} is cut without {piece}, {said}"), name: None });
            without.push(folder::Without { piece, why: why.into(), absent: false });
        }
        // The same edit, with everything it took here and its film right, isn't rendered again.
        let film_stands = |old: &Version| old.files.get("video").and_then(|sha256| lock.files.get(sha256)).is_some_and(|film| folder.path(&film.path).is_file());
        let again = |old: &Version| old.inputs == inputs && old.without.is_empty() && film_stands(old) && tools::film_off(&asset.fields, &standing.reads, &lock, old).is_none();
        if without.is_empty() && lock.asset(&asset.name).is_some_and(|held| held.versions.iter().any(again)) {
            return Ok(None);
        }
        self.leave_out(&name, &on, tools::unreadable(&ffprobe, &pieces)?, &mut without);
        pieces.retain(|piece| piece.still || piece.shape.is_some());
        said.extend(tools::past_end(&mut pieces, canvas.fps));
        said.iter().for_each(|message| self.say(Event::Warning { message: message.clone(), name: None }));
        let (project, cutting, began) = (self.project.clone(), self.project.cutting.clone(), Instant::now());
        let cache = self.host.cache().join("cuts");
        let cutting_name = name.clone();
        let fps = canvas.fps;
        let (rendering, rendered) = (self.project.rendering.clone(), asset.name.clone());
        rendering.lock().expect("rendering lock").insert(rendered.clone(), 0.0);
        crate::events::project_settled(&folder.dir);
        let cutting_result = tokio::task::spawn_blocking(move || {
            let _held = cutting.lock().expect("cutting lock");
            let dir = project.folder.dir.clone();
            // Each twentieth, so a page shows it moving and reads the project a few times, however long the film.
            let told = move |done: f64| {
                let mut now = rendering.lock().expect("rendering lock");
                if now.get(&rendered).is_none_or(|was| done - was >= 0.05) {
                    now.insert(rendered.clone(), done.min(1.0));
                    drop(now);
                    crate::events::project_settled(&dir);
                }
            };
            tools::cut_around(&ffmpeg, &ffprobe, &project.folder, pieces, &canvas, &cutting_name, &cache, &told)
        })
        .await;
        self.project.rendering.lock().expect("rendering lock").remove(&asset.name);
        crate::events::project_settled(&folder.dir);
        let (made, left) = cutting_result.map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))?.map_err(|e| e.next(format!("cozy project render {} --on {on}", asset.name)))?;
        self.leave_out(&name, &on, left, &mut without);
        let Some((made, part)) = made else {
            let stands = if folder.dir.join(&name).is_file() { ": the film that stands stays" } else { "" };
            self.say(Event::Warning { message: format!("{name} isn't cut, and no version is made: none of its clips can be used{stands}"), name: None });
            return Ok(None);
        };
        let now = folder.lock()?;
        let held = now.asset(&asset.name);
        let before = held.and_then(|held| held.versions.last()?.files.get("video").cloned());
        let sha256 = folder.take(&part)?;
        let path = folder.lock()?.files.get(&sha256).map(|file| folder.path(&file.path));
        if let Some(path) = path {
            self.keep_aside(&asset.name, &folder.dir.join(&name), &now);
            tools::publish(&path, &folder.dir.join(&name)).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{name}: {e}")))?;
        }
        // The timeline it's the film of, kept with it: a version chosen later brings its own.
        let timeline = folder.keep(&folder.dir.join(&name).with_extension("otio"))?;
        // What the film is, not what its pieces add up to.
        let frames = folder.film_frames(&folder.dir.join(&name)).unwrap_or(made.frames);
        if frames.abs_diff(made.frames) > 1 + standing.reads.len() as u64 {
            self.say(Event::Warning { message: format!("{name} holds {frames} frames, and its pieces add up to {}", made.frames), name: None });
        }
        let (retimed, took_s) = (made.retimed, began.elapsed().as_secs_f64());
        self.cut.lock().expect("cut lock").insert(name.clone());
        let said = without.iter().map(folder::Without::said).collect();
        self.say(Event::Cut { name: asset.name.clone(), cut: name, frames, fps, retimed, took_s, unchanged: before.as_ref() == Some(&sha256), without: said });
        Ok(Some((Files::from([("video".to_owned(), sha256), ("timeline".to_owned(), timeline)]), without)))
    }

    /// Each clip a cut goes without, warned of with what was said of it, and recorded.
    fn leave_out(&self, name: &str, on: &str, left: Vec<tools::LeftOut>, without: &mut Vec<folder::Without>) {
        for tools::LeftOut { asset: piece, why, answer } in left {
            if without.iter().any(|held| name_key(&held.piece) == name_key(&piece)) {
                continue;
            }
            self.say(Event::Warning { message: format!("{name} is cut without {piece}: {answer}: cozy project render {piece} --on {on}"), name: None });
            without.push(folder::Without { piece, why, absent: false });
        }
    }

    /// A file at `at`, where a film goes, that is no version of `asset`: someone made it there. It is kept beside it, and said.
    fn keep_aside(&self, asset: &str, at: &FsPath, lock: &Lock) {
        let Ok(got) = tokio::task::block_in_place(|| folder::hash(at)) else { return };
        if lock.asset(asset).is_some_and(|held| held.versions.iter().any(|version| version.main() == Some(&got))) {
            return;
        }
        let stem = at.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let aside = at.with_file_name(format!("{stem}.edited-{}.mp4", folder::prefix_of(&got)));
        if std::fs::rename(at, &aside).is_ok() {
            self.say(Event::Warning { message: format!("{} isn't a version of {asset}: it is kept as {}", at.display(), aside.display()), name: None });
        }
    }

    /// The film and timeline at the root as asset `i`'s state has them: a version chosen, a memo gone back to, one deleted, or one
    /// still linked into `files/` by a cozy before this.
    async fn film_follows(&self, doc: &Doc, stand: &[Standing], lock: &Lock, i: usize) {
        let Some(files) = stand[i].files.as_ref() else { return };
        let Some(file) = files.get("video").and_then(|sha256| lock.files.get(sha256)) else { return };
        let (at, from) = (self.folder().dir.join(cut_name(&doc.assets[i].name)), self.folder().path(&file.path));
        if !from.is_file() {
            return;
        }
        let same = !tools::shares_bytes(&at) && tokio::task::block_in_place(|| folder::hash(&at)).is_ok_and(|got| got == file.sha256);
        if !same {
            self.keep_aside(&doc.assets[i].name, &at, lock);
            if let Err(e) = tools::publish(&from, &at) {
                self.say(Event::Warning { message: format!("{}: {e}", at.display()), name: None });
            }
        }
        let timeline = at.with_extension("otio");
        match files.get("timeline").and_then(|sha256| lock.files.get(sha256)) {
            Some(kept) if !same || !timeline.is_file() => {
                if let Err(e) = tools::publish(&self.folder().path(&kept.path), &timeline) {
                    self.say(Event::Warning { message: format!("{}: {e}", timeline.display()), name: None });
                }
            }
            None if !timeline.is_file() => self.timeline_again(&doc.assets[i], &stand[i], lock, &cut_name(&doc.assets[i].name)).await,
            _ => {}
        }
    }

    /// Each of `pieces` not on this computer, or whose bytes aren't its hash, fetched: from the machine that keeps it while that lives, else
    /// the Hub. What can't be had, warned of with what to run, is what the cut goes without; one whose bytes changed says what ffprobe says of it.
    async fn bring_pieces(&self, asset: &format::Asset, pieces: &[tools::Piece], lock: &Lock, target: &Target, ffprobe: &FsPath, on: &str) -> Result<Vec<folder::Without>, Error> {
        let (mut without, mut seen) = (vec![], BTreeSet::new());
        for piece in pieces.iter().filter(|piece| seen.insert(name_key(&piece.asset))) {
            let Some(file) = lock.files.get(&piece.digest) else { continue };
            // A file is named for its bytes, and a cut reads them all anyway.
            let here = piece.path.is_file();
            let (path, digest) = (piece.path.clone(), piece.digest.clone());
            let changed = here && tokio::task::spawn_blocking(move || folder::hash(&path).is_ok_and(|got| got != digest)).await.unwrap_or(false);
            if here && !changed {
                continue;
            }
            let why = if changed { "its bytes changed" } else { "not on this computer" };
            self.say(Event::Said { message: format!("{}: {} is {why}: fetching it", asset.name, piece.asset), name: Some(asset.name.clone()) });
            let brought = match file.stays() && self.host.fetchable(&file.sha256, &file.machine).await {
                true => self.host.fetch(&file.sha256, &file.machine).await,
                false => self.host.pull_one(&self.project, &target.hub, file.clone()).await,
            };
            let Err(reason) = brought else { continue };
            let answer = changed.then(|| tools::unread(ffprobe, &piece.path)).flatten().map_or(String::new(), |answer| format!(" ({answer})"));
            let piece = piece.asset.clone();
            self.say(Event::Warning { message: format!("{} is cut without {piece}: {why}{answer}, and it can't be had: {reason}: cozy project render {piece} --on {on}, or cozy project pull", cut_name(&asset.name)), name: None });
            without.push(folder::Without { piece, why: why.into(), absent: !here });
        }
        Ok(without)
    }

    /// Lists a tool's version: a cut's join its others, and a frame's older ones go unless pinned, with whatever files only they named.
    fn record_free(&self, doc: &Doc, name: &str, kind: &str, version: Version, noun: &str) -> Result<(), Error> {
        let number = version.number;
        let old = std::cell::RefCell::new(vec![]);
        // What is pinned now, as a save may have moved it, else as this render read it.
        let now = Doc::load(self.folder());
        let doc = now.as_ref().unwrap_or(doc);
        let pin = doc.find(name).and_then(|i| Some(doc.assets[i].pin.as_ref()?.1.clone()));
        let cut = doc.find(name).is_some_and(|i| doc.tool(i) == Some(Tool::Cut));
        self.folder().update_lock(|lock| {
            let held = lock.asset_mut(name, kind);
            let (keep, going): (Vec<Version>, Vec<Version>) = held.versions.drain(..).partition(|version| cut || version.main() == pin.as_ref());
            held.versions = keep;
            held.versions.push(version.clone());
            held.versions.sort_by_key(|version| version.number);
            held.failed = None;
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
            held.failed = None;
        })?;
        version.number = number.get();
        if approve
            && let Some(sha256) = version.main()
            && let Err(error) = pin_in_file(self.folder(), name, kind, sha256)
        {
            self.say(Event::Warning { message: error.message, name: None });
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
        self.say(Event::Landed { name: name.into(), noun: noun.into(), number, files: listed(self.folder(), &lock, &version.files), path, execution_ms });
        if let Some(kept) = version.kept {
            self.say(Event::Said { message: kept.said(name, number), name: Some(name.to_owned()) });
            self.project.jobs.why(name, number, format!("keeps take {} to {:.2} s", kept.take, kept.until_s));
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

/// Whether asset `i` is a pin `ask` may yet re-run: one with a version behind it, and one it hasn't tried. A dropped file has nothing to run again.
fn judged_pin(doc: &Doc, stand: &[Standing], ask: &Ask, i: usize) -> bool {
    doc.makes(i) && doc.tool(i).is_none() && matches!(stand[i].state, Stand::Pinned { version: Some(_), .. }) && !ask.sent.contains(&name_key(&doc.assets[i].name))
}

fn pin_changed(stand: &[Standing], i: usize) -> bool {
    matches!(&stand[i].state, Stand::Pinned { changed, .. } if !changed.is_empty())
}

/// Whether `ask` has a re-run to make of asset `i` as things stand: not if a name it reads is gone, which only fails. With its package's
/// tags not known, that is for the run to find, which leaves the pin as it is.
fn outdated_now(doc: &Doc, stand: &[Standing], ask: &Ask, i: usize) -> bool {
    ask.outdated && judged_pin(doc, stand, ask, i) && pin_changed(stand, i) && doc.tags(i).is_none_or(|tags| doc.missing_names(i, tags).is_empty())
}

/// The tools with no state whose reads have settled: those nothing that's on its way goes into.
fn tools_ready(doc: &Doc, stand: &[Standing], lock: &Lock) -> Vec<usize> {
    let going: HashSet<String> = lock.pending.values().map(|pending| name_key(&pending.asset)).collect();
    let mut waiting = vec![false; doc.assets.len()];
    let mut ready = vec![];
    for &i in &doc.order {
        let pinned = doc.assets[i].pin.is_some();
        let reads_going = !pinned && doc.upstream[i].iter().any(|&read| waiting[read]);
        waiting[i] = reads_going || (!pinned && going.contains(&name_key(&doc.assets[i].name)));
        if !reads_going && doc.tool(i).is_some() && doc.makes(i) && stand[i].inputs.is_some() && stand[i].files.is_none() {
            waiting[i] = true;
            ready.push(i);
        }
    }
    ready
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
        _ if doc.assets[j].hollow => format!("{name} has nothing to make"),
        Stand::Stale(why) => format!("{name} is out of date: {why}"),
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
        "otio" => "application/json",
        "yaml" => "text/yaml; charset=utf-8",
        extension => folder::media_of(extension).map_or("application/octet-stream", |(media, _)| media),
    }
}
