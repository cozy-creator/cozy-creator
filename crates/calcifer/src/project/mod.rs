//! Projects (xs-151, cl-758): a folder of assets, each a stable name holding a set of files. Calcifer opens a project by its path
//! (and keeps every folder it opened in its index) and walks it: an operation makes an asset only when it has no state, and is an
//! ordinary run, sent as soon as what it reads has settled, so independent ones run at once and the machines' schedulers place
//! them. Its function is called with them, and what it makes lands as one version of the asset, its file in `files/` and its row in
//! `project.db` (cl-803), which holds all a project says and has made, one transaction per action. The reserved package `cozy` runs
//! here, free. One walker per project takes every render asked of it, and what it owes is kept in `project.db` too, so a restarted
//! Calcifer goes on with it. Its clients ask and watch; the runs go on without them.
mod apply;
mod assets;
mod canvas;
pub(crate) mod code;
mod db;
mod crdt;
pub(crate) mod fit;
mod folder;
mod format;
pub(crate) mod graph;
mod history;
pub(crate) mod purge;

#[cfg(test)]
#[path = "retired_storage_tests.rs"]
mod retired_storage_tests;
#[cfg(test)]
mod web_seed_tests;
pub(crate) mod host;
mod jobs;
pub(crate) mod journal;
pub(crate) use journal::{OpenProject, ProjectRef, ProjectStore, RunOwner};
mod keyed;
pub(crate) mod legacy_worker;
mod view;
pub(crate) mod model;
mod nodes;
mod remote;
mod thumbs;
mod binder;
pub(crate) use binder::Inline;
mod tools;
mod unseen;
mod walker;
mod written;
pub(crate) mod vault;
pub(crate) mod vault_operation;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::{delete, get, post, put},
};
use futures_util::{FutureExt as _, StreamExt as _, future::BoxFuture, stream::FuturesUnordered};
use api::{
    calcifer::{Kind as RunKind, NoteLevel, On, Run, StagedInput, Status as RunStatus, SubmitRun, Warning},
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    ids::{Digest, RunId},
    machine as v1,
    calcifer::project::{
        Approve, Approved, Asked, Asset as Row, Cancel, Cancelled, Cut, File as Listing, Import, Imported, Job, JobState, Known, Machines as ProjectMachines, MainFile, Open, Opened,
        Orphan, Projects as Listed, Queued, QueuedNow, Reclaim, Reclaimed, Rename, Renamed, Rendered, Runs, Said, Stale, Standing as RowStanding, State as ProjectState, Unpin, Unpinned,
        TakeNow, Unused, VersionPrefix, Warning as ProjectWarning,
    },
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast};
use tracing::{info, warn};

use crate::{
    api::{Api, Refusal, parse},
    placement::{self, Offer},
    plan,
};
use folder::{Ask, Coming, Failed, Files, Folder, Lock, Pending, Take, Target, Tried, Version};
use format::{ALL, Doc, Standing, State as Stand, Tool};
use graph::name_key;
use host::{Host, Package, Renderer};
use jobs::{Going, Jobs, Told};
use walker::{End, Frozen, Graph, Kind as JobKind, Named, Node, Turn};
use tools::Canvas;

pub(crate) use thumbs::previews;

/// The projects this Calcifer knows: those open now, by id, which each keeps in its `project.db` (cl-819). Every one it opened is
/// in `state.db`'s `projects`, by id, with its name and where it last was. A project is open while anything reads it: a call, a
/// stream, its walk (which goes until nothing is owed), a page showing it. The last of them to let go closes it (cl-823).
#[derive(Default)]
pub(crate) struct Projects {
    open: Mutex<HashMap<String, Weak<Project>>>,
    /// What was last heard of each published package projects name, by `with`.
    heard: Mutex<HashMap<String, Heard>>,
    /// How many times that changed: what a read said of them is good until then.
    heard_changes: AtomicU64,
    /// Each project made by name since Calcifer started, by the `Idempotency-Key` that asked: a retry makes no `_2`.
    made: Mutex<HashMap<String, PathBuf>>,
    requests:Mutex<HashMap<PathBuf,usize>>,
}

pub(super) struct RequestActivity {api:Arc<Api>,root:PathBuf}
impl Drop for RequestActivity {
    fn drop(&mut self) {
        let mut requests=self.api.projects.requests.lock().expect("project requests");
        if let Some(count)=requests.get_mut(&self.root) { *count-=1; if *count==0 {requests.remove(&self.root);} }
    }
}
impl Projects {
    pub(crate) fn active_requests(&self,root:&FsPath)->bool {
        self.requests.lock().expect("project requests").get(&vault::logical(root)).is_some_and(|count|*count>0)
    }
    fn track_request(&self,api:&Arc<Api>,root:&FsPath)->RequestActivity {
        let root=vault::logical(root);
        *self.requests.lock().expect("project requests").entry(root.clone()).or_default()+=1;
        RequestActivity {api:api.clone(),root}
    }
}

/// A package as a project's read knows it: when it was last asked of, and what that found.
#[derive(Default)]
struct Heard {
    asked: Option<Instant>,
    package: Option<Arc<Package>>,
}

pub(crate) struct Project {
    id: String,
    code: String,
    /// Immutable owner. A recycled physical mount path must never choose another project's root.
    root: PathBuf,
    /// Cache ownership follows the physical mount, not a replaceable viewer authorization.
    mount_id: Option<vault::MountIdentity>,
    folder: Folder,
    /// What its files say, as last read.
    model: tokio::sync::Mutex<Option<Arc<model::Model>>>,
    /// What streams were last told of it.
    told: Mutex<Option<model::Told>>,
    /// What its render is doing, asset by asset.
    pub(crate) jobs: Jobs,
    /// What its walker needn't read again: the jobs, and each text.
    walker: Mutex<Walker>,
    /// Whether its one walker is going. Admission and the terminal board share this lock.
    walking: Mutex<bool>,
    /// Tells the walker something was asked.
    asked: Notify,
    /// Held while a cut is made.
    cutting: Arc<Mutex<()>>,
    /// How far each Edit being rendered here is, 0 to 1.
    rendering: Arc<Mutex<BTreeMap<String, f64>>>,
    /// Serializes materialization of files already stored at the Hub.
    moving: tokio::sync::Mutex<()>,
    /// What opening it did to it, for the first to read it.
    said: Mutex<Vec<String>>,
    /// What kept its last render from reaching a machine (`State::unsent`).
    unsent: Mutex<Vec<Error>>,
    /// The versions this person hasn't looked at, by box id (cl-928): `state.db`'s, held here.
    unseen: Mutex<unseen::Unseen>,
    /// Its folder watched while it's open, a plain one's (`Projects::look`).
    _watch: Option<platform::watch::Watcher>,
}

impl Drop for Project {
    /// Nothing reads it now: what Calcifer kept of it goes with it, its folder's `project.db` connection, parts and history once no
    /// `Folder` names it. What streams were told of it goes too, and a reopen reads it anew.
    fn drop(&mut self) {
        let mut identities = IDENTITIES.lock().expect("identities lock");
        if identities.get(&self.folder.dir).is_some_and(|(id, _)| *id == self.id) {
            identities.remove(&self.folder.dir);
        }
        drop(identities);
        crate::events::project_closed(&self.id);
    }
}

/// A project held open, by a stream that shows it: it closes once nothing holds it.
pub(crate) struct Reader(Arc<Project>);

impl Reader {
    pub(crate) fn id(&self) -> &str { &self.0.id }
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod presence_tests;

#[cfg(test)]
mod code_tests;

pub(crate) fn routes(api: &Arc<Api>) -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/projects", post(open).get(list))
        .route("/v1/projects/{id}", get(state))
        .route("/v1/projects/{id}/imports", post(import))
        .route("/v1/projects/{id}/canvas", get(canvas::read).patch(canvas::patch))
        .route("/v1/projects/{id}/view", get(view::read).patch(view::patch))
        .route("/v1/projects/{id}/thumbs/{sha256}", get(thumbs::thumb))
        .route("/v1/projects/{id}/runs", post(runs).get(crate::api::project_list))
        .route("/v1/projects/{id}/runs/submit", post(crate::runs::submit_project))
        .route("/v1/projects/{id}/runs/{run}", get(crate::api::show))
        .route("/v1/projects/{id}/runs/{run}/cancel", post(crate::api::cancel))
        .route("/v1/projects/{id}/runs/{run}/events", get(crate::api::events))
        .route("/v1/projects/{id}/runs/{run}/outputs/{name}", get(crate::outputs::serve))
        .route("/v1/projects/{id}/runs/{run}/play", get(crate::play::play))
        .route("/v1/projects/{id}/runs/{run}/preview", get(crate::previews::serve))
        .route("/v1/projects/{id}/fit", post(fit::fit))
        .route("/v1/projects/{id}/fit/here", post(fit::here))
        .route("/v1/projects/{id}/cancel", post(cancel))
        .route("/v1/projects/{id}/approve", post(approve))
        .route("/v1/projects/{id}/unpin", post(unpin))
        .route("/v1/projects/{id}/rename", post(rename))
        .route("/v1/projects/{id}/machines", put(machines))
        .route("/v1/projects/{id}/reclaim", post(reclaim))
        .route("/v1/projects/{id}/assets", post(assets::create))
        .route("/v1/projects/{id}/copies", post(assets::copy))
        .route("/v1/projects/{id}/assets/{name}", get(assets::read).patch(assets::patch).delete(assets::delete))
        .route("/v1/projects/{id}/assets/{name}/versions", post(assets::join))
        .route("/v1/projects/{id}/assets/{name}/versions/{number}", delete(assets::discard).patch(assets::patch_version))
        .route("/v1/projects/{id}/assets/{name}/versions/{number}/seen", post(unseen::seen))
        .route("/v1/projects/{id}/files/{file}", get(file))
        .route("/v1/projects/{id}/cuts/{cut}", get(cut))
        .route("/v1/projects/{id}/nodes", get(nodes::read))
        .route("/v1/projects/{id}/apply", post(apply::apply))
        .route("/v1/projects/{id}/integrity", get(integrity))
        .route("/v1/projects/{id}/history", get(history::list))
        .route("/v1/projects/{id}/history/undo", post(history::undo))
        .route("/v1/projects/{id}/history/redo", post(history::redo))
        .route_layer(axum::middleware::from_fn(history::record))
        // Outside the history's: a write answered again is no edit of its own.
        .route_layer(axum::middleware::from_fn_with_state(api.clone(), keyed::answered))
        .route_layer(axum::middleware::from_fn_with_state(api.clone(), vault::privacy))
        .route("/v1/projects/{id}/vault", get(vault::read).post(vault::write))
        .route("/v1/projects/{id}/vault/operations/{operation}", axum::routing::put(vault_operation::write).delete(vault_operation::acknowledge))
        .route("/v1/projects/{id}/address", get(address))
}

/// Watches the plain project `id`'s folder `dir`, for what changes with no write of Calcifer's: a file into or out of `files/` (one
/// leaving is on another computer), another program's commit to project.db.
fn watch(id: &str, dir: &FsPath) -> Option<platform::watch::Watcher> {
    use platform::watch::{Watcher, Writes};
    let (stirred, files) = (id.to_owned(), dir.join(folder::FILES));
    let dirs = [(dir, Writes::Named(&[db::DB, db::WAL])), (files.as_path(), Writes::None)];
    Watcher::new("project-watch", &dirs, move || crate::events::project_stirred(&stirred))
        .inspect_err(|error| warn!("{}: what other programs change in it isn't seen: {error}", dir.display()))
        .ok()
}

impl Projects {
    /// Each of the `stirred` open projects told to the streams if its folder's change moved its revision (`revision`). Its revision is read
    /// through its own folder, and told only while it's still open: a forget takes it out under `open` before letting go of what Calcifer
    /// kept of it (cl-822), which a look mustn't make again.
    pub(crate) fn look(&self, stirred: &std::collections::BTreeSet<String>) {
        for project in self.now_open().into_iter().filter(|project| stirred.contains(&project.id)) {
            let id = &project.id;
            let dir = &project.folder.dir;
            if vault::encrypted(&project.root) { continue; }
            if project.require_view().is_err() {
                // Its folder went: a page showing it reads it again, and says so where it stands (cl-980). Held while it's told, as below.
                let still = self.open.lock().expect("projects lock");
                if !dir.exists() && still.get(id).is_some_and(|open| open.strong_count() > 0) {
                    crate::events::project_looked(dir, "gone".to_owned());
                }
                continue;
            }
            let rev = format!("{}.{}", project.folder.stamp(), stamp(&dir.join(folder::FILES)));
            // Held while it's told: a forget waits for it.
            let still = self.open.lock().expect("projects lock");
            if still.get(id).is_some_and(|open| open.strong_count() > 0) {
                crate::events::project_looked(dir, rev);
            }
        }
    }

    /// Each project open now, held for as long as the caller holds it.
    fn now_open(&self) -> Vec<Arc<Project>> {
        self.open.lock().expect("projects lock").values().filter_map(Weak::upgrade).collect()
    }

    /// Project `id` as it's open now, if it is.
    fn open_now(&self, id: &str) -> Option<Arc<Project>> {
        let project = self.open.lock().expect("projects lock").get(id).and_then(Weak::upgrade)?;
        project.require_view().ok()?;
        Some(project)
    }

    /// The project a route's `{id}` names (`api::calcifer::project::NAMED_BY`): its id, public code or name, or its folder on this computer. Open,
    /// else opened from the index. It stays open while the answer is held.
    fn get(&self, api: &Arc<Api>, named: &str) -> Result<Arc<Project>, Refusal> {
        if let Some(project) = self.open_now(named) {
            return Ok(project);
        }
        if api::target::written_as_path(named.trim()) {
            return self.open(api, &self::named(api, named).map_err(Refusal)?).map_err(Refusal);
        }
        let entry = known(api, named.trim()).map_err(Refusal)?;
        let project = self.open(api, &entry.path).map_err(Refusal)?;
        if project.id != entry.id {
            let said = format!("{named} was in {}, which holds another project now", entry.path.display());
            return Err(Refusal(Error::new(Kind::NotFound, "project_unknown", said).next("cozy open")));
        }
        // Opened for a call: used now, which the index lists by.
        api.calcifer.store().project_opened(&project.id, &identity(&project.folder.dir).1, &project.root, Some(now_ms()), Some(&project.code))?;
        Ok(project)
    }

    /// Project `id` held open by a stream that shows it, until the stream lets it go.
    pub(crate) fn reader(&self, api: &Arc<Api>, id: &str) -> Result<Reader, Refusal> {
        self.get(api, id).map(Reader)
    }

    /// The project in `dir` (canonical), opened if it isn't, given its id and name and recorded in the index; its walk goes on when
    /// one is owed. It stays open while the answer is held.
    fn open(&self, api: &Arc<Api>, dir: &FsPath) -> Result<Arc<Project>, Error> {
        let root = vault::logical(dir);
        let mount_id = vault::mount_identity(&root);
        let physical = current_view(&root)?;
        let dir = physical.as_path();
        let (folder, (id, name, code)) = tokio::task::block_in_place(|| Folder::open_with(dir, api.machines.hubs.home(), |folder| {
            let (id, name, code) = identify_loaded(api, dir, folder)?;
            // Validate a move before inherited local work can be unbound or resumed. A
            // rendering project keeps its original physical folder until its runs land.
            let moved = self.open.lock().expect("projects lock").get(&id).and_then(Weak::upgrade)
                .filter(|open| open.root != root || open.mount_id != mount_id || open.folder.dir != dir);
            if let Some(open) = moved {
                if *open.walking.lock().expect("walking lock") {
                    let said = format!("{id} moved from {} while it renders: move it back, or once its runs land", open.folder.dir.display());
                    return Err(Error::new(Kind::Conflict, "project_busy", said));
                }
                self.open.lock().expect("projects lock").remove(&id);
            }
            Ok((id, name, code))
        }))?;
        IDENTITIES.lock().expect("identities lock").insert(dir.to_owned(), (id.clone(), name));
        crate::events::project_opened(dir);
        let mut open = self.open.lock().expect("projects lock");
        let project = match open.get(&id).and_then(Weak::upgrade)
            .filter(|project| project.root == root && project.mount_id == mount_id && project.folder.dir == dir) {
            Some(project) => project,
            None => {
                let unseen=unseen::read(&folder,&id)?;
                let watch = (!vault::encrypted(&root)).then(|| watch(&id, dir)).flatten();
                let project = Arc::new(Project {
                    id: id.clone(),
                    code,
                    root,
                    mount_id,
                    folder,
                    jobs: Jobs::new(id.clone()),
                    model: tokio::sync::Mutex::default(),
                    told: Mutex::default(),
                    walker: Mutex::default(),
                    walking: Mutex::new(false),
                    asked: Notify::new(),
                    cutting: Arc::default(),
                    rendering: Arc::default(),
                    moving: tokio::sync::Mutex::default(),
                    said: Mutex::default(),
                    unsent: Mutex::default(),
                    unseen: Mutex::new(unseen),
                    _watch: watch,
                });
                // What closed since the last open goes from the list.
                open.retain(|_, held| held.strong_count() > 0);
                open.insert(id, Arc::downgrade(&project));
                if let Some(board) = project.folder.last_board() {
                    project.jobs.ended_on(board);
                }
                project
            }
        };
        drop(open);
        // Calcifer finds its journal while anything holds it open.
        api.calcifer.note_open(RunOwner::of(&project, &api.calcifer));
        // Its deleted boxes looked at as it opens: one set aside before cl-919 counts from now.
        purge::due(&project.id, now_ms());
        // Told until a reader hears it: a project closes when nothing holds it, so this open may not be the one read.
        if let Some(said) = copy_note(&project.root) {
            let mut told = project.said.lock().expect("said lock");
            if !told.contains(&said) { told.push(said); }
        }
        project.require_view()?;
        Ok(project)
    }

}

/// A run that a person canceled: no version, and no failure of its asset.
const CANCELED: &str = "version_canceled";
/// How long what was heard of a package stands before a read asks again, in the background.
const HEARD_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Where a run of the project in `dir` lands its outputs.
pub(crate) fn landing(dir: &FsPath) -> PathBuf {
    vault::logical(dir).join(folder::RUNS)
}

/// Project `id` opened again for Calcifer's work on it, as any request opens it: none for a locked or unknown one.
pub(crate) fn reopen(api: &Arc<Api>, id: &str) -> Option<RunOwner> {
    api.projects.get(api, id).ok().map(|project| RunOwner::of(&project, &api.calcifer))
}

/// The project a run's landing folder is in, if it's one.
pub(crate) fn of_landing(out_dir: &FsPath) -> Option<&FsPath> {
    out_dir.file_name().is_some_and(|name| name == folder::RUNS).then(|| out_dir.parent()).flatten()
}

/// The folder of the project a request names (`api::calcifer::project::NAMED_BY`): its id, public code or name in the index, or the project holding
/// a folder of this computer's, by its absolute path. Never a client's folder: a relative path names none.
pub(crate) fn named(api: &Api, text: &str) -> Result<PathBuf, Error> {
    let text = text.trim();
    let path = FsPath::new(text);
    if api::target::written_as_path(text) {
        if !path.is_absolute() {
            let said = format!("{text} is relative, and Calcifer has no folder of a client's to start it from");
            return Err(Error::new(Kind::Usage, "project_path", said).remedy(format!("name {}", api::calcifer::project::NAMED_BY)));
        }
        vault::recover_conversion(path)?;
        let root = match path.ancestors().find(|root| vault::encrypted(root)) {
            Some(root) => root.to_owned(),
            None => holding(path)?,
        };
        refuse_own_folder(&root)?;
        return Ok(root);
    }
    Ok(known(api, text)?.path)
}

/// The index, last used first: each project opened here whose folder still holds it. One whose folder is gone (deleted, moved, on a
/// drive that isn't there) leaves the index as it's found so, and its name is free. Its `project.db` keeps its id and name: opened
/// again by its path it is back as it was, or as `<name>_2` if another took the name meanwhile.
fn indexed(api: &Api) -> Result<Vec<crate::state::KnownProject>, Error> {
    let store = api.calcifer.store();
    let failed = |e: crate::store::StoreError| Error::internal("calcifer_store", e.to_string());
    let known = store.known_projects().map_err(failed)?;
    for entry in &known { vault::recover_conversion(&entry.path)?; }
    let (mut here, gone): (Vec<_>, Vec<_>) = known.into_iter().partition(|entry| vault::keeps_index(&entry.path));
    if !gone.is_empty() {
        store.projects_gone(&gone.into_iter().map(|entry| entry.id).collect::<Vec<_>>()).map_err(failed)?;
    }
    // An earlier Calcifer may have opened one of its own folders: its row stays, unlisted and unopened.
    here.retain(|entry| !own_folder(&entry.path));
    for entry in here.iter_mut().filter(|entry| entry.code.is_empty()) {
        // A locked project still has its canonical id. Do not mint a public
        // code until an opened view can recover the code stored in its DB.
        if vault::encrypted(&entry.path) { continue; }
        match backfill_code(store, entry) {
            Ok(code) => entry.code = code,
            Err(error) => warn!("keeping the existing project link for {}: {}", entry.path.display(), error.message),
        }
    }
    Ok(here)
}

/// An older project keeps its existing link if its public code cannot be recorded.
fn backfill_code(store: &crate::store::Store, entry: &crate::state::KnownProject) -> Result<String, Error> {
    let path = entry.path.join(db::DB);
    if !path.is_file() { return Err(Folder::not_a_project(&entry.path)); }
    let conn = db::open(&path)?;
    if db::identity(&conn).is_none() {
        return Err(Error::new(Kind::Validation, "project_identity", format!("{} has no native CRDT project identity", path.display())));
    }
    let kept = db::meta(&conn, "code");
    let code = reserve_code(store, &entry.id, &entry.name, &entry.path, kept.as_deref())?;
    db::set_meta(&conn, &path, "code", Some(&code))?;
    Ok(code)
}

/// The project the index knows by `text`, its id, public code or its name (in any case).
fn known(api: &Api, text: &str) -> Result<crate::state::KnownProject, Error> {
    let mut indexed = indexed(api)?;
    if let Some(at) = indexed.iter().position(|entry| entry.id == text) {
        return Ok(indexed.swap_remove(at));
    }
    if let Some(code) = code::parse(text) {
        if let Some(at) = indexed.iter().position(|entry| entry.code == code) {
            return Ok(indexed.swap_remove(at));
        }
    }
    let mut named: Vec<_> = indexed.into_iter().filter(|entry| entry.name.eq_ignore_ascii_case(text)).collect();
    match named.len() {
        0 => {
            let code = code::parse(text).unwrap_or_default();
            let gone = api.calcifer.store().project_gone(text, &code).map_err(|e| Error::internal("calcifer_store", e.to_string()))?;
            Err(match gone {
                Some(path) => Error::new(Kind::NotFound, "project_folder_gone", format!("this project's folder is gone: {}", path.display()))
                    .remedy("move the folder back, or open another project")
                    .next("cozy new <folder>"),
                None => Error::new(Kind::NotFound, "project_unknown", format!("this computer knows no project {text}")).next("cozy open"),
            })
        }
        1 => Ok(named.remove(0)),
        _ => {
            let ids: Vec<String> = named.iter().map(|entry| format!("{} ({})", entry.id, entry.path.display())).collect();
            let said = format!("{text} names {} projects: {}", ids.len(), ids.join(", "));
            Err(Error::new(Kind::Conflict, "project_ambiguous", said).remedy("name it by its id"))
        }
    }
}

/// The files a run is `given`, each `(path, sha256)`, kept in its project unless they're there. What Calcifer holds in `uploads` is
/// named by its content and linked; anything else is a person's file and is copied. Says what it couldn't keep.
/// An encrypted project's transport uploads stay inside its mounted storage, never the shared plaintext CAS.
pub(crate) fn private_upload_root(api: &Arc<Api>, named: &str) -> Result<Option<PathBuf>, Error> {
    let project = api.projects.get(api, named).map_err(|e| e.0)?;
    project.require_view()?;
    Ok(Some(project.folder.dir.clone()))
}

pub(crate) fn keep_given(project: &FsPath, given: &[(PathBuf, String)], uploads: &FsPath) -> Result<Vec<String>, Error> {
    let physical = vault::resolve_path(project)?;
    let folder = Folder::at(&physical);
    let unkept = given.iter().filter_map(|(path, sha256)| {
        let why = folder.keep_given(path, sha256, path.starts_with(uploads)).err()?;
        Some(format!("{} isn't kept in its project, so asking this run again needs it given again: {}", path.display(), why.message))
    });
    Ok(unkept.collect())
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
        Self { calcifer, dir: vault::logical(&dir) }
    }

    /// The file of this project `written` names by its hash, what stays on a machine fetched now that it's named. None: it names
    /// none of them.
    pub(crate) async fn file(&self, written: &str) -> Result<Option<PathBuf>, Error> {
        let Some(hex) = written_hash(written) else { return Ok(None) };
        let physical = vault::resolve_path(&self.dir)?;
        let folder = Folder::at(&physical);
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

    /// `cozy run`'s written fields bound as Render binds a box's: mentions read this project's boxes, then the run's own references
    /// (`inline`, each with its file here), ranges are cut here, and `pickup.` terms are the gesture. Answers the call, the files it
    /// names (the project's fetched here if a machine keeps them), what was said, and what it left out.
    pub(crate) async fn bound(&self, target: &str, package: &str, callable: &api::interface::Callable, written: Map<String, Value>, clips: &crate::runs::clip::Clips, inline: &[(binder::Inline, PathBuf)]) -> Result<(Map<String, Value>, Vec<crate::runs::bind::InputFile>, Vec<String>, Vec<Unused>), Error> {
        let _access = vault::admit_root(&self.dir).await?;
        let physical = vault::resolve_path(&self.dir)?;
        let folder = Folder::at(&physical);
        let doc = Doc::load(&folder)?;
        let lock = folder.lock()?;
        let stand = doc.stand(&lock);
        drop(_access);
        let mut fields = written;
        let picked = callable.request.fields.iter().find(|field| matches!(field.ty.plain(), api::interface::Type::Pickup(_))).and_then(|field| fields.remove(&field.name));
        let pickup = match picked {
            None => None,
            Some(value) => Some(serde_json::from_value::<api::calcifer::project::Pickup>(value).map_err(|why| Error::new(Kind::Usage, "pickup_malformed", format!("a pickup is pickup.take=<box.N> and pickup.out_frame=<frame>: {why}")))?),
        };
        let reserved = callable.tool.as_ref().map(|tool| tool.reserved.clone()).unwrap_or_default();
        doc.worded(&mut fields, &reserved);
        let own: Vec<binder::Inline> = inline.iter().map(|(named, _)| named.clone()).collect();
        let call = binder::Call { name: target, tool: target, package, callable, fields: &fields, reserved: &reserved, doc: &doc, own: None, lock: &lock, stand: &stand, gone: &BTreeSet::new(), pickup: pickup.as_ref(), inline: &own };
        let mut bound = binder::bind(&call)?;
        // The run's own files go as a cut clip does: by their digest, from where they are here.
        let mut cut: HashMap<String, PathBuf> = inline.iter().map(|(named, path)| (named.file.clone(), path.clone())).collect();
        let mut digests = vec![];
        for range in &bound.cuts {
            let Some(path) = self.file(&range.source).await? else { continue };
            let fps = lock.files.get(&range.source).map_or(0.0, |file| file.fps);
            if fps <= 0.0 {
                return Err(Error::new(Kind::Unavailable, "range_unmeasured", format!("{}: its rate isn't measured, so no range of it can be cut", folder::prefix_of(&range.source))).next("cozy status"));
            }
            let mut notes = vec![];
            let access = vault::admit_root(&self.dir).await?;
            let made = clips.cut(&range.range.to_string(), &path, range.range, fps, &mut notes, access).await?;
            bound.warnings.extend(notes.into_iter().filter(|note| note.level == api::calcifer::NoteLevel::Warning).map(|note| note.message));
            let clip = made.clip.clone();
            let access = vault::admit_root(&self.dir).await?;
            let sha256 = tokio::task::spawn_blocking(move || {
                let _access = access;
                folder::hash(&clip)
            }).await.map_err(|e| Error::new(Kind::Internal, "clip_hash", e.to_string()))??;
            cut.insert(sha256.clone(), made.clip);
            digests.push(sha256);
        }
        let mut payload = Value::Object(std::mem::take(&mut bound.payload));
        binder::cuts_filled(&mut payload, &digests);
        let mut files = vec![];
        for (field, _, sha256) in tools::files_in(&payload, &folder, &lock, &cut) {
            let path = match cut.get(&sha256) {
                Some(clip) => clip.clone(),
                None => self.file(&sha256).await?.unwrap_or_else(|| folder.path(&lock.files[&sha256].path)),
            };
            let sniffed = crate::runs::bind::sniff(&path)?;
            let digest = sha256.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?;
            files.push(crate::runs::bind::InputFile { order: order_of(&field), shown: sha256.clone(), field, path, digest, length: sniffed.length, media_type: sniffed.media_type });
        }
        Ok((payload.as_object().cloned().unwrap_or_default(), files, bound.warnings, bound.unused))
    }
}

/// What a project's run landed: which run, of what, its bytes' digest, and what's measured of them.
pub(crate) struct Landed<'a> {
    pub run_id: &'a api::ids::RunId,
    pub run: u64,
    pub target: &'a str,
    pub output: &'a str,
    pub index: u32,
    pub digest: &'a str,
    pub measured: Option<&'a Facts>,
}

/// What's measured of a file: a picture's or video's size, a video's rate and frames, a picture's alpha, a sound's length.
pub(crate) use folder::File as Facts;

/// What's measured of the picture, video or sound at `path`, as a project measures its files, with ffprobe (and ffmpeg, for a picture's
/// alpha) from `tools`, else PATH's. None of anything else, or with no ffprobe here.
pub(crate) fn measured(tools: &FsPath, path: &FsPath, media_type: &str) -> Option<Facts> {
    let field = ["image", "video", "audio"].into_iter().find(|field| media_type.strip_prefix(field).is_some_and(|rest| rest.starts_with('/')))?;
    let (found, ffprobe) = platform::tools::ffmpeg(tools, &crate::tools::path())?;
    tools::measured(&found.path, &ffprobe, path, field)
}

/// A run's output, landed at `dest`, kept in its project's `files/` and listed there. An operation's run takes its files off the
/// list when it lands them on its version. Answers the file's path, and what to say if its project couldn't list it.
pub(crate) fn landed(project: &FsPath, made: &Landed, dest: &FsPath) -> Result<(PathBuf, Option<String>), String> {
    let physical = vault::resolve_path(project).map_err(|error| error.message)?;
    let project = physical.as_path();
    let output = match made.index {
        0 => made.output.to_owned(),
        n => format!("{}-{n}", made.output),
    };
    let row = folder::RunFile { run: made.run, run_id: Some(made.run_id.clone()), function: made.target.to_owned(), output, sha256: made.digest.to_owned(), ..Default::default() };
    let folder = Folder::at(project);
    let (kept, unlisted) = folder.adopt(dest, made.digest, row, made.measured).map_err(|e| e.message)?;
    // The file is in `files/` now. An output that grows reads its next revision whole, with nothing here to extend.
    let _ = std::fs::remove_file(dest);
    let said = unlisted.map(|why| match why.next.first() {
        Some(next) => format!("{} is in files/, and its project can't list it: {}: {next}", made.output, why.message),
        None => format!("{} is in files/, and its project can't list it: {}", made.output, why.message),
    });
    Ok((kept, said))
}

/// An output its machine keeps, in its project's lock.
pub(crate) fn stays(project: &FsPath, sha256: &str, machine: &str, length: u64) -> Result<(), String> {
    let physical = vault::resolve_path(project).map_err(|error| error.message)?;
    Folder::at(&physical).stays(sha256, machine, length).map_err(|e| e.message)
}

/// Each open project's id and name by its folder, as its `project.db` keeps them, read as it opens.
static IDENTITIES: std::sync::LazyLock<Mutex<HashMap<PathBuf, (String, String)>>> = std::sync::LazyLock::new(Mutex::default);

/// The id of the project in `dir`, which its `project.db` keeps (cl-819).
pub(crate) fn project_id(dir: &FsPath) -> String {
    identity(dir).0
}

/// Best-effort identity for event correlation. Opening a native project separately requires
/// its persisted canonical identity; this fallback never repairs or initializes a project.
pub(crate) fn identity(dir: &FsPath) -> (String, String) {
    if let Some(known) = IDENTITIES.lock().expect("identities lock").get(dir) {
        return known.clone();
    }
    let root = vault::logical(dir);
    if vault::encrypted(&root) && let Some(id) = vault::project_id(&root) {
        let name = vault::resolve_path(&root).ok().and_then(|view| {
            IDENTITIES.lock().expect("identities lock").get(&view)
                .filter(|(held, _)| held == &id).map(|(_, name)| name.clone())
        }).unwrap_or_else(|| folder_name(&root));
        return (id, name);
    }
    kept_identity(dir).unwrap_or_else(|| (folder_id(dir), folder_name(dir)))
}

fn kept_identity(dir: &FsPath) -> Option<(String, String)> {
    db::identity(&db::connect(&dir.join(db::DB), true).ok()?)
}

/// Best-effort event correlation for a path without a loaded identity. This is never persisted
/// as the identity of a native project.
fn folder_id(dir: &FsPath) -> String {
    let dir = vault::logical(dir);
    format!("project-{}", &hex::encode(Sha256::digest(dir.to_string_lossy().as_bytes()))[..16])
}

/// A new project's id: 128 random bits.
fn fresh_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS gives random bytes");
    format!("project-{}", hex::encode(bytes))
}

fn folder_name(dir: &FsPath) -> String {
    dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into())
}

/// The current native project in `dir`, its canonical identity recorded in the index. A copy
/// another folder still holds takes a new id; a name another project has takes `_2`.
fn identify(api: &Api, dir: &FsPath) -> Result<(String, String, String), Error> {
    let path = dir.join(db::DB);
    if !path.is_file() {
        return Err(Folder::not_a_project(dir));
    }
    let mut conn = db::open(&path)?;
    let kept = db::identity(&conn);
    let code = db::meta(&conn, "code");
    identify_with(api, dir, kept, code, |id, name, code| db::set_identity(&mut conn, &path, id, name, Some(code)))
}

fn identify_loaded(api: &Api, dir: &FsPath, folder: &Folder) -> Result<(String, String, String), Error> {
    identify_with(api, dir, folder.canonical_identity()?, folder.public_code()?, |id, name, code| folder.set_identity(id, name, code))
}

fn identify_with(api: &Api, dir: &FsPath, kept: Option<(String, String)>, kept_code: Option<String>, keep: impl FnOnce(&str, &str, &str) -> Result<(), Error>) -> Result<(String, String, String), Error> {
    let store = api.calcifer.store();
    let logical = vault::logical(dir);
    refuse_own_folder(&logical)?;
    let encrypted = vault::encrypted(&logical);
    // The project another folder in the index still holds by this id.
    let original = |id: &str| store.known_project(id).ok().flatten().filter(|known| known.path != logical).filter(|known| {
        if vault::encrypted(&known.path) {
            project_vault::Keybag::load(&known.path.join(project_vault::KEYBAG_FILE))
                .is_ok_and(|keybag| keybag.info().project_id == id)
        } else { kept_identity(&known.path).is_some_and(|(held, _)| held == id) }
    });
    let copied = |id: &str| original(id).is_some();
    if encrypted {
        let keybag = project_vault::Keybag::load(&logical.join(project_vault::KEYBAG_FILE))
            .map_err(|_| Error::new(Kind::Conflict, "vault_identity", "the encrypted project keybag is unreadable"))?;
        if kept.as_ref().is_none_or(|(id, _)| id != &keybag.info().project_id) {
            return Err(Error::new(Kind::Conflict, "vault_identity", "the encrypted project identity does not match its keybag"));
        }
        if copied(&keybag.info().project_id) {
            return Err(Error::new(Kind::Conflict, "vault_copy", "another folder still holds this encrypted project; open that project or move its folder instead"));
        }
    }
    let (id, name, copy_of) = match &kept {
        Some((id, name)) => match original(id) {
            None => (id.clone(), name.clone(), None),
            Some(original) => (fresh_id(), name.clone(), Some(original)),
        },
        None => return Err(Error::new(Kind::Validation, "project_identity", format!("{} has no native CRDT project identity", dir.join(db::DB).display()))),
    };
    let name = unique_name(api, &id, dir, if name.is_empty() { folder_name(dir) } else { name })?;
    if let Some(original) = copy_of {
        let said = format!("{} is a copy of {} ({}), so it is a project of its own now: {name}", logical.display(), original.name, original.path.display());
        tracing::info!("{said}");
        COPIES.lock().expect("copies lock").insert(logical.clone(), said);
    }
    let kept_code = kept.as_ref().filter(|(held, _)| held == &id).and(kept_code);
    let code = reserve_code(store, &id, &name, &logical, kept_code.as_deref())?;
    if kept.as_ref() != Some(&(id.clone(), name.clone())) || kept_code.as_deref() != Some(&code) {
        keep(&id, &name, &code)?;
    }
    vault::register_plain_root(&logical, &id)?;
    Ok((id, name, code))
}

/// What the first open of a folder found to be a copy tells, once: its new identity, and the project it was a copy of.
static COPIES: std::sync::LazyLock<Mutex<HashMap<PathBuf, String>>> = std::sync::LazyLock::new(Mutex::default);

fn copy_told(dir: &FsPath) -> Option<String> {
    COPIES.lock().expect("copies lock").remove(&vault::logical(dir))
}

/// What a copy has to tell, still untold.
fn copy_note(dir: &FsPath) -> Option<String> {
    COPIES.lock().expect("copies lock").get(&vault::logical(dir)).cloned()
}

/// Calcifer's own folders beside a project, a conversion's stage and the original it moves aside, are never projects.
pub(crate) fn own_folder(dir: &FsPath) -> bool {
    dir.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".cozy-"))
}

fn refuse_own_folder(dir: &FsPath) -> Result<(), Error> {
    if !own_folder(dir) { return Ok(()); }
    Err(Error::new(Kind::Conflict, "project_own_folder", format!("{} is a folder Calcifer keeps while it converts a project's storage, not a project", dir.display()))
        .remedy("open the project beside it"))
}

/// The index reserves the code atomically. If recording it in project.db fails, the next open
/// recovers the index's assignment before answering with a public URL.
fn reserve_code(store: &crate::store::Store, id: &str, name: &str, dir: &FsPath, preferred: Option<&str>) -> Result<String, Error> {
    store.project_opened(id, name, dir, None, preferred).map_err(|error| match error {
        crate::store::StoreError::ProjectCode(message) => Error::new(Kind::Conflict, "project_code_conflict", message),
        other => Error::internal("calcifer_store", other.to_string()),
    })
}

/// `wanted`, or the first of `wanted_2`, `wanted_3`, … no other project in the index has, in any case: what `-p` names it by. The
/// project is `id`, in `dir`: one the index still has there was deleted and this made in its place, and holds no name.
fn unique_name(api: &Api, id: &str, dir: &FsPath, wanted: String) -> Result<String, Error> {
    let known = indexed(api)?;
    let taken = |name: &str| known.iter().any(|entry| entry.id != id && entry.path != dir && entry.name.eq_ignore_ascii_case(name));
    let names = std::iter::once(wanted.clone()).chain((2..).map(|n| format!("{wanted}_{n}")));
    Ok(names.into_iter().find(|name| !taken(name)).expect("names go on"))
}

/// What `project.changed` says of the project in `dir`: its revision in `project.db`, which moves with each commit, and how `files/` stands,
/// as a pull's files come with no commit of their own.
pub(crate) fn revision(dir: &FsPath) -> String {
    format!("{}.{}", Folder::stamp_at(dir), stamp(&dir.join(folder::FILES)))
}

/// What a file's stats say of it: another text whenever it's another file than it was.
fn stamp(path: &FsPath) -> String {
    let Ok(meta) = std::fs::metadata(path) else { return "none".to_owned() };
    let modified = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
    #[cfg(unix)]
    let file = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let file = 0;
    format!("{file}-{}-{}", modified.map_or(0, |since| since.as_nanos()), meta.len())
}

/// Walks again each project an earlier Calcifer was walking.
pub(crate) fn resume(api: &Arc<Api>) {
    let known = api.calcifer.store().known_projects().unwrap_or_else(|e| {
        warn!("the projects being walked: {e}");
        vec![]
    });
    // Each stays open while its walk goes.
    for dir in known.into_iter().filter(|entry| entry.walking).map(|entry| entry.path) {
        if let Err(error) = api.projects.open(api, &dir) {
            warn!("walking {} again: {}", dir.display(), error.message);
        }
    }
}

/// Starts the project's walker, or tells the one going that something was asked. Its board begins once the last walk's end is told.
fn walk(api: &Arc<Api>, project: &Arc<Project>) {
    let mut walking = project.walking.lock().expect("walking lock");
    walk_under(api, project, &mut walking);
}

fn walk_under(api: &Arc<Api>, project: &Arc<Project>, walking: &mut bool) {
    if std::mem::replace(walking, true) {
        return project.asked.notify_one();
    }
    let active = project.jobs.begin();
    let walk = Walk::new(api, project);
    let api = api.clone();
    tokio::spawn(async move {
        let access = match vault::admit_work(&walk.project).await {
            Ok(access) => access,
            Err(error) => {
                let mut walking = walk.project.walking.lock().expect("walking lock");
                walk.project.unsent.lock().expect("unsent lock").push(error.clone());
                let ended = api::calcifer::project::Ended { error: Some(error), rendered: None, asks: vec![] };
                walk.project.jobs.end(walk.began.elapsed().as_millis() as u64, ended);
                *walking = false;
                drop(walking);
                remember(&api);
                vault::stir();
                return;
            }
        };
        drop(access);
        let following = following(&api, &walk.project, active);
        remember(&api);
        if let Err(error) = walk.go(&following).await {
            walk.measure_at_end();
            let mut walking = walk.project.walking.lock().expect("walking lock");
            walk.project.unsent.lock().expect("unsent lock").push(error.clone());
            // The stopped walk reports every accepted ask it still owes. Their
            // durable rows remain for a later walk to resume.
            if let Ok(owed) = walk.folder().walk_now() {
                walk.ended.lock().expect("ended lock").extend(owed.walk.asks.into_iter().map(|ask| ask.id));
            }
            walk.end(&mut walking, Some(error), &following);
        }
        following.abort();
        let _ = following.await;
        remember(&api);
        crate::events::project_settled(&walk.project.folder.dir);
        // Its work ended: its vault's clocks go on.
        vault::stir();
    });
}

/// Keeps the board as the project's runs go, with the warnings their logs give, each once, as its asset's.
fn following(api: &Arc<Api>, project: &Arc<Project>, active: u64) -> tokio::task::JoinHandle<()> {
    let mut tail = api.calcifer.subscribe();
    let project = project.clone();
    tokio::spawn(async move {
        loop {
            let (id, event) = match tail.recv().await {
                Ok(heard) => heard,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            project.jobs.ran(active, &id, &event, &project.folder.dir);
        }
    })
}

/// The board as the graph and what is on its way stand now. `run` reads a run by its number.
fn sync_jobs(project: &Project, run: impl Fn(u64) -> Option<Run>) -> Result<(), Error> {
    let Some(active) = project.jobs.active() else { return Ok(()) };
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
            ends_at_ms: walker.sent.get(id).map(|offer| offer.finish_at_ms),
            gpus: walker.sent.get(id).map(|offer| offer.width).filter(|width| *width > 0),
        }
    });
    let going = going.collect();
    let sent: HashSet<&str> = walk.pending.iter().map(|(key, pending)| pending.job_of(key)).collect();
    let now = now_ms();
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
            let placed = walker.placed.get(&job.key);
            // Its turn come, held for a machine on its way that finishes it sooner (cl-915): `waits for pluto, ready in about 2 min`.
            let held = placed.filter(|_| waits.is_empty()).and_then(|placed| Some((placed.soonest.waits_for(now)?, placed.soonest.machine.to_string())));
            let row = Job {
                id: job.key.clone(),
                asset: job.asset.clone(),
                noun: noun(Some(job)).into(),
                needs: waits.iter().map(|name| (*name).to_owned()).collect(),
                why: Some(held.as_ref().map_or(why, |(said, _)| said.clone())),
                machine: held.map(|(_, machine)| machine),
                gpus: placed.map(|placed| placed.soonest.width).filter(|width| *width > 0),
                starts_at_ms: placed.map(|placed| placed.soonest.start_at_ms),
                ends_at_ms: placed.map(|placed| placed.soonest.finish_at_ms),
                ..Job::default()
            };
            (row, job.shot)
        })
        .collect();
    drop(walker);
    let on = walk.walk.asks.iter().map(|ask| &ask.target).chain(walk.pending.values().map(|pending| &pending.target));
    let on = on.flat_map(|target| target.on.iter().map(ToString::to_string)).collect();
    project.jobs.sync(active, on, going, coming, run);
    Ok(())
}

/// A job not sent yet, as a plan reads it: its id, key, render, target, function and choices, the jobs it reads, and whether it's ready.
struct Unsent {
    id: usize,
    key: String,
    ask: String,
    target: Target,
    with: String,
    made: Map<String, Value>,
    needs: Vec<usize>,
    ready: bool,
}

/// Where a job whose turn is next goes: what each of its machines would do with it, and where the plan placed it.
#[derive(Clone)]
struct Placed {
    offers: Vec<Offer>,
    soonest: Offer,
}

/// What a project's walker needn't read again.
#[derive(Default)]
struct Walker {
    graph: Graph,
    /// By text's digest: asks of one text read it once.
    readings: HashMap<String, AskedText>,
    /// The asks in the graph.
    joined: HashSet<String>,
    /// Asks joined that it hasn't said anything of yet.
    greet: Vec<String>,
    /// Asks whose text doesn't read, and why.
    unread: Vec<(String, Error)>,
    /// Jobs whose sending found out what their package makes: they have their turn again, read knowing that.
    again: HashSet<String>,
    /// The assets the project names now, as of its stamp.
    names: Option<(String, Arc<HashSet<String>>)>,
    /// Where each job not ready yet will most likely go, by its key, as the last plan placed it (cl-967).
    foreseen: HashMap<String, Option<On>>,
    /// Where each job whose turn is next goes, by its key, as the last plan placed it (cl-967), with its other machines' offers.
    placed: HashMap<String, Placed>,
    /// Why the last plan placed no machine for each job whose turn is next, by its key, as its line said.
    unplanned: HashMap<String, String>,
    /// What each job not sent asks of a machine, by its key, once its package and its models' cards said.
    quotables: HashMap<String, placement::Job>,
    /// Each job's planned start and the machines that can run it, by its key, as the last plan said.
    starts: HashMap<String, (i64, Vec<String>)>,
    /// What each machine is told to have ready for each job, by machine and job key: its Hub, item and function; none: nothing.
    items: HashMap<(String, String), Option<(String, v1::Ahead, String)>>,
    /// Items being made.
    making: HashSet<(String, String)>,
    /// The last turn held a job for a busy machine.
    held: bool,
    /// What the machine each job went to quoted, by its key.
    sent: HashMap<String, Offer>,
    /// What's on its way may still say an old name: what each is called now, by name key.
    renamed: HashMap<String, String>,
}

impl Walker {
    /// A new version of `name`: what stands by it is worked out again, under what it's called now.
    fn moved(&mut self, name: &str) {
        let name = self.renamed.get(&name_key(name)).map_or(name, String::as_str);
        self.readings.values_mut().for_each(|reading| reading.moved(name));
    }
}

/// One text a render was asked with, read once, and what stands by it: each asset worked out when asked of, and again once what it reads moved.
struct AskedText {
    doc: Arc<Doc>,
    nodes: Vec<Node>,
    done: Vec<Standing>,
    known: Vec<bool>,
    /// What reads each.
    readers: Vec<Vec<usize>>,
}

impl AskedText {
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
    let said = serde_json::to_string(&(&asset.target, &asset.made, &asset.inputs)).unwrap_or_default();
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

/// A frozen render as the graph reads it, by what the lock holds now.
struct Now<'a> {
    reading: &'a mut AskedText,
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
        // Of `all`, every pin; else only those its names read.
        if !self.ask.outdated || (!self.ask.whole && !self.ask.judges.contains(&name_key(&self.reading.doc.assets[i].name))) {
            return false;
        }
        self.reading.ensure(i, self.lock);
        judged_pin(&self.reading.doc, &self.reading.done, self.ask, i)
    }
}

fn current_view(root: &FsPath) -> Result<PathBuf, Error> {
    platform::durable::canonical(&vault::view(root)?)
        .map_err(|why| Error::new(Kind::Conflict, "project_view_unavailable", why.to_string()))
}

impl Project {
    /// Old references are bound to their original physical view, never a recycled drive.
    fn require_view(&self) -> Result<(), Error> {
        if self.mount_id != vault::mount_identity(&self.root) || current_view(&self.root)? != self.folder.dir {
            return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
        }
        Ok(())
    }

    /// The renders going say `to` for `from` from now on, as the lock does: their snapshots, what they plan on those, and the board. What landed under `from` stands, what lands
    /// is `to`'s, and what reads it reads on. Each snapshot keeps its place in the walk, under its new digest. The old reading stays for this walk: a turn may be holding its digest.
    fn rename_walk(&self, from: &str, to: &str) -> Result<(), Error> {
        let walk = self.folder.walk_now()?;
        let mut snapshots: HashMap<String, (String, Doc)> = HashMap::new();
        for ask in walk.walk.asks.iter().filter(|ask| !ask.digest.is_empty()) {
            if snapshots.contains_key(&ask.digest) {
                continue;
            }
            // Asked before the project named it, or with another of the new name since gone: what it plans on goes on finding the row under its old name.
            let moved = self.folder.asked(ask).and_then(|mut written| {
                written.renamed(from, to)?;
                let doc = Doc::asked(&self.folder, &written)?;
                Ok((written, doc))
            });
            let Ok((written, doc)) = moved else { continue };
            let (digest, snapshot) = written.snapshot();
            self.folder.keep_snapshot(&digest, &snapshot)?;
            snapshots.insert(ask.digest.clone(), (digest, doc));
        }
        let mut walker = self.walker.lock().expect("walker lock");
        walker.graph.renamed(from, to);
        walker.renamed.insert(name_key(from), to.to_owned());
        let mut digests = HashMap::new();
        for (was, (digest, doc)) in snapshots {
            let reading = AskedText::new(doc);
            walker.graph.read_again(|job_ask| walk.walk.asks.iter().any(|ask| ask.id == job_ask && ask.digest == was), &reading.nodes);
            walker.readings.insert(digest.clone(), reading);
            digests.insert(was, digest);
        }
        self.folder.update_walk(|walk| {
            for ask in &mut walk.walk.asks {
                if let Some(digest) = digests.get(&ask.digest) {
                    ask.digest.clone_from(digest);
                }
            }
        })?;
        drop(walker);
        self.jobs.renamed(from, to);
        self.asked.notify_one();
        Ok(())
    }

    /// Each render `lock` holds that the graph doesn't yet: its jobs made, what it left going taken in.
    fn join(&self, lock: &Lock) {
        let mut walker = self.walker.lock().expect("walker lock");
        let Walker { graph, readings, joined, greet, unread, .. } = &mut *walker;
        for ask in &lock.walk.asks {
            if !joined.insert(ask.id.clone()) {
                continue;
            }
            if !readings.contains_key(&ask.digest) {
                let read = self.folder.asked(ask).and_then(|written| Doc::asked(&self.folder, &written));
                match read {
                    Ok(doc) => drop(readings.insert(ask.digest.clone(), AskedText::new(doc))),
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
                takes: ask.takes.iter().map(|take| Named { key: take.key.clone(), asset: take.asset.clone(), approve: take.approve, covers: take.covers }).collect(),
            };
            graph.add(&ask.id, &mut Now { reading, lock, ask }, &asked);
        }
    }
}

/// Records which open projects are being walked, which the next start walks again.
fn remember(api: &Api) {
    let walking: Vec<(String, bool)> = (api.projects.now_open().iter()).map(|project| (project.id.clone(), *project.walking.lock().expect("walking lock"))).collect();
    if let Err(e) = api.calcifer.store().projects_walking(&walking) {
        warn!("recording the projects being walked: {e}");
    }
}

/// What a project and its render say while what landed can't go in.
pub(super) fn not_saved(why: &str) -> String {
    format!("Not saved yet: {why}, and what landed goes in as soon as it can")
}

/// Makes a new project called `name`: in this person's home folder, or at `path`, a folder of this computer's, recorded in the index as
/// used now.
async fn open(State(api): State<Arc<Api>>, headers: HeaderMap, retry:Option<axum::Extension<keyed::CreationRetry>>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let mut open: Open = parse(&body?)?;
    let _ownership=crate::maintenance_source::begin_project_creation().map_err(Refusal)?;
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok());
    if open.unencrypted {
        if open.passphrase.is_some() {
            return Err(Refusal(Error::new(Kind::Usage, "vault_credential", "unencrypted creation cannot include a passphrase")));
        }
    } else {
        if !open.passphrase.as_ref().is_some_and(|pass| !pass.is_empty() && pass.len() <= 4096) {
            return Err(Refusal(Error::new(Kind::Usage, "vault_credential", "new projects are encrypted: supply a passphrase, or explicitly choose unencrypted storage")
                .next("cozy new --passphrase-file <file>").next("cozy new --unencrypted")));
        }
        if key.is_some() {
            return Err(Refusal(Error::new(Kind::Usage, "vault_idempotency", "encrypted creation cannot use an Idempotency-Key: its recovery response must not be cached")));
        }
    }
    let engine = if open.unencrypted { None } else { Some(tokio::task::block_in_place(|| vault::preflight_creation(&api)).map_err(Refusal)?) };
    let io = |at: &FsPath, e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", at.display())));
    let mut moved_on = None;
    let (dir, recovery_secret) = if let Some(axum::Extension(retry))=retry {(retry.0,None)} else { match &open.path {
        Some(path) if !path.is_absolute() => {
            let said = format!("{} is relative: a project is made at a folder of Calcifer's computer by its absolute path", path.display());
            return Err(Refusal(Error::new(Kind::Usage, "project_path", said)));
        }
        Some(path) => {
            // First use from separate CLI processes can reach this path together.
            let _creation = api.projects.made.lock().expect("made lock");
            refuse_own_folder(path).map_err(Refusal)?;
            refuse_encrypted_parent(path).map_err(Refusal)?;
            if Folder::is_project(path) || vault::encrypted(path) {
                let said = format!("{} is a project already", path.display());
                return Err(Refusal(Error::new(Kind::Conflict, "project_exists", said).next("cozy status")));
            }
            std::fs::create_dir_all(path).map_err(|e| io(path, e))?;
            let name = open.name.clone().unwrap_or_else(|| folder_name(path));
            tokio::task::block_in_place(|| create_new(&api, path, &name, open.starter, open.passphrase.take(), engine.as_ref())).map_err(Refusal)?
        }
        None => {
            let Some(name) = &open.name else {
                return Err(Refusal(Error::new(Kind::Usage, "project_path", "a new project needs a `name`, or a `path`")));
            };
            match key.and_then(|key| api.projects.made.lock().expect("made lock").get(key).cloned()) {
                Some(dir) => (dir, None),
                None => {
                    let (dir, why, recovery) = tokio::task::block_in_place(|| named_project(&api, name, open.starter, open.passphrase.take(), engine.as_ref())).map_err(Refusal)?;
                    moved_on = why;
                    if let Some(key) = key {
                        api.projects.made.lock().expect("made lock").insert(key.to_owned(), dir.clone());
                    }
                    (dir, recovery)
                }
            }
        }
    }};
    // Named, a project nothing reads is only identified: the call that reads it opens it. One open, maybe where it was before it moved,
    // is opened here.
    let unread = match dir.join(db::DB).is_file() {
        true => {
            let (id, _, code) = tokio::task::block_in_place(|| identify(&api, &dir)).map_err(Refusal)?;
            api.projects.open_now(&id).is_none().then_some((id, code))
        },
        false => None,
    };
    let (id, code, said) = match unread {
        Some((id, code)) => (id, code, copy_told(&dir).into_iter().collect()),
        None => {
            let project = api.projects.open(&api, &dir).map_err(Refusal)?;
            let said = std::mem::take(&mut *project.said.lock().expect("said lock"));
            copy_told(&project.root);
            (project.id.clone(), project.code.clone(), said)
        }
    };
    let opened = opened(id, code, &dir);
    api.calcifer.store().project_opened(&opened.id, &opened.name, &dir, Some(now_ms()), Some(&opened.code))?;
    let next = vec!["cozy open".into()];
    let said = moved_on.into_iter().chain(said).collect();
    let response = Json(Opened { next, said, recovery_secret, ..opened }).into_response();
    Ok(if open.unencrypted { response } else { vault::private(response) })
}

/// The project `id`, in `dir`, as an answer gives it.
fn opened(id: String, code: String, dir: &FsPath) -> Opened {
    Opened { id, code, path: dir.to_owned(), name: identity(dir).1, next: vec![], said: vec![], recovery_secret: None }
}

/// A new project called `name` in this person's home folder, or the next free name: what it could be called, `_2`, `_3`, …. Whatever is
/// there already stays as it is, a folder of anything or another project, and it says so: nothing is made in a folder it didn't make.
fn named_project(api: &Api, name: &str, starter: bool, passphrase: Option<String>, engine: Option<&project_mount::Provider>) -> Result<(PathBuf, Option<String>, Option<String>), Error> {
    let home = &api.machines.user_home;
    refuse_encrypted_parent(home)?;
    let io = |e: std::io::Error| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", home.display()));
    let wanted = format::plain_name(name).unwrap_or_else(|| "untitled".to_owned());
    let names = std::iter::once(wanted.clone()).chain((2..).map(|n| format!("{wanted}_{n}")));
    for name in names {
        let dir = home.join(&name);
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                let (dir, recovery) = create_new(api, &dir, &name, starter, passphrase, engine)?;
                let why = (name != wanted).then(|| format!("{} is taken, so this is {name}", home.join(&wanted).display()));
                return Ok((dir, why, recovery));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e)),
        }
    }
    unreachable!("names go on")
}

/// A new project called `name` in `dir`, with a new id, holding the starter film or nothing: its `project.db`, made whole and then
/// named. Its canonical folder.
fn made_new(api: &Api, dir: &FsPath, name: &str, starter: bool) -> Result<PathBuf, Error> {
    refuse_encrypted_parent(dir)?;
    let io = |e: platform::Error| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", dir.display()));
    let written = if starter { format::starter() } else { written::Written::default() };
    let canonical = platform::durable::canonical(dir).map_err(io)?;
    let (id, name) = (fresh_id(), unique_name(api, "", &canonical, name.to_owned())?);
    new_database(&canonical, &id, &name, written)?;
    crate::events::project_changed(&canonical);
    Ok(canonical)
}

fn new_database(dir: &FsPath, id: &str, name: &str, written: written::Written) -> Result<(), Error> {
    let part = dir.join(format!("{}.new", db::DB));
    let creation=keyed::claimed();
    db::create_with_key(&part, &db::Stored { written: written.into(), ..Default::default() }, &[], &[],creation.as_ref())?;
    db::set_identity(&mut db::open(&part)?, &part, id, name, None)?;
    std::fs::rename(&part, dir.join(db::DB)).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", dir.display())))?;
    platform::durable::sync_dir(dir).map_err(|e| Error::new(Kind::Unavailable, "project_io", e.to_string()))
}

/// Fresh encrypted projects write their first database through the mount, never through a plaintext source folder.
fn create_new(api: &Api, dir: &FsPath, name: &str, starter: bool, passphrase: Option<String>, engine: Option<&project_mount::Provider>) -> Result<(PathBuf, Option<String>), Error> {
    let Some(passphrase) = passphrase else { return made_new(api, dir, name, starter).map(|dir| (dir, None)); };
    refuse_encrypted_parent(dir)?;
    if std::fs::read_dir(dir).map_err(|e| Error::new(Kind::Unavailable, "project_io", e.to_string()))?.next().is_some() {
        return Err(Error::new(Kind::Conflict, "project_not_empty", "create an encrypted project in an empty directory; existing files are preserved")
            .next("cozy new <new-directory>").next("cozy new --unencrypted"));
    }
    let canonical = platform::durable::canonical(dir).map_err(|e| Error::new(Kind::Unavailable, "project_io", e.to_string()))?;
    let (id, name) = (fresh_id(), unique_name(api, "", &canonical, name.to_owned())?);
    let written = if starter { format::starter() } else { written::Written::default() };
    let engine = engine.ok_or_else(|| Error::internal("vault_provider", "encrypted creation requires the prepared provider"))?;
    let recovery = vault::create_new(api, &canonical, &id, &name, written, project_vault::SecretString::from(passphrase), engine)?;
    crate::events::project_changed(&canonical);
    Ok((canonical, Some(recovery)))
}

fn refuse_encrypted_parent(dir: &FsPath) -> Result<(), Error> {
    if vault::logical(dir).ancestors().any(vault::encrypted) {
        return Err(Error::new(Kind::Conflict, "vault_nested_project", "create a separate project outside the encrypted project's storage"));
    }
    Ok(())
}

/// The project folder holding a source staged by path: the nearest, up from it, that is one. A retired or incomplete project's
/// files there are never global maintenance data either.
pub(crate) fn source_project(path: &FsPath) -> Result<Option<PathBuf>, Error> {
    match owning(path, true) {
        Ok(project) => Ok(Some(project)),
        Err(error) if error.code == "project_not_found" => Ok(None),
        Err(error) => Err(error),
    }
}

/// The project folder a request names by a path: the nearest, up from it, that is one. A folder of old files (a
/// `project.canvas.yaml`) or an interrupted creation's `project.db.new` is no project (cl-930).
fn holding(path: &FsPath) -> Result<PathBuf, Error> {
    owning(path, false)
}

fn owning(path: &FsPath, source: bool) -> Result<PathBuf, Error> {
    let unavailable = || Error::new(Kind::Unavailable, "project_ownership_unavailable", "the source's project ownership cannot be verified");
    // A client can name a path only it sees (its own copy under a project's folder here): the nearest
    // ancestor that exists here decides the owner, as for any path.
    let existing = path.ancestors().find(|dir| dir.exists()).ok_or_else(unavailable)?;
    let start = platform::durable::canonical(existing).map_err(|_| unavailable())?;
    let metadata=std::fs::metadata(&start).map_err(|_|unavailable())?;
    let start=if metadata.is_file() {start.parent().ok_or_else(unavailable)?.to_owned()} else if metadata.is_dir() {start} else {return Err(unavailable());};
    for dir in start.ancestors() {
        let logical = vault::logical(dir);
        if logical != dir || vault::encrypted(dir) { return Ok(logical); }
        for marker in [project_vault::KEYBAG_FILE, db::DB, "project.canvas.yaml", "project.db.new"] {
            match std::fs::symlink_metadata(dir.join(marker)) {
                Ok(_) if marker == project_vault::KEYBAG_FILE || marker == db::DB => return Ok(dir.to_owned()),
                Ok(_) if source => return Err(Error::new(Kind::Validation, "project_format_unsupported", "the source belongs to a retired or incomplete project; do not treat it as global maintenance")),
                Ok(_) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(_) => return Err(unavailable()),
            }
        }
    }
    if source { return Err(Error::new(Kind::NotFound, "project_not_found", "the verified source is outside a project").next("cozy new")); }
    Err(Error::new(Kind::NotFound, "project_not_found", format!("{} is in no project: no {} there or in a folder above it", path.display(), db::DB))
        .remedy("run from an existing project directory, or create one with cozy new")
        .next("cozy new"))
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

/// The index, last used first: each project's name, where it is and its last use, and whether it is rendering.
async fn list(State(api): State<Arc<Api>>, Query(page): Query<Page>) -> Result<Json<Listed>, Refusal> {
    let mut known = indexed(&api).map_err(Refusal)?;
    if let Some(before) = &page.before {
        let at = known.iter().position(|entry| &entry.id == before).map_or(known.len(), |at| at + 1);
        known.drain(..at);
    }
    let limit = page.limit.unwrap_or(50).clamp(1, 1000);
    let older = known.len().saturating_sub(limit) as u64;
    let open: HashMap<String, Arc<Project>> = api.projects.now_open().into_iter().map(|project| (project.id.clone(), project)).collect();
    let projects = (known.into_iter().take(limit))
        .map(|entry| Known {
            name: entry.name,
            busy: open.get(&entry.id).is_some_and(|project| *project.walking.lock().expect("walking lock")),
            path: entry.path,
            used_at_ms: entry.used_at_ms,
            id: entry.id,
            code: entry.code,
        })
        .collect();
    Ok(Json(Listed { projects, older }))
}

/// A compact link can reach the unlock UI using only the index, while content stays locked.
async fn address(State(api): State<Arc<Api>>, Path(reference): Path<String>) -> Result<Json<api::calcifer::project::Address>, Refusal> {
    if api::target::written_as_path(reference.trim()) {
        let project = api.projects.get(&api, &reference)?;
        return Ok(Json(api::calcifer::project::Address { id: project.id.clone(), code: project.code.clone() }));
    }
    let entry = known(&api, &reference).map_err(Refusal)?;
    Ok(Json(api::calcifer::project::Address { id: entry.id, code: entry.code }))
}

/// Where its rows differ from its document (cl-885): nowhere, always. What e2e checks after each write.
async fn integrity(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let differences = project.folder.integrity().map_err(Refusal)?;
    Ok(Json(serde_json::json!({ "differences": differences })))
}

/// Its assets as they stand.
async fn state(State(api): State<Arc<Api>>, Path(id): Path<String>, Query(asked): Query<Read>) -> Result<Json<ProjectState>, Refusal> {
    let covered = asked.assets.then(crate::events::snapshot_cursor).flatten();
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    let folder = &project.folder;
    let model = match model::of(&api, &project).await {
        Ok(model) => model,
        Err(error) => {
            let lock = folder.lock().map_err(Refusal)?;
            if !lock.owes() {
                return Err(Refusal(error));
            }
            // A render going has its own text: what it owes, and what's wrong with the file, are the answer.
            let queue = queue(&project, &folder.walk_now().map_err(Refusal)?);
            let busy = *project.walking.lock().expect("walking lock");
            let warning = format!("{}: a render going uses the text it was asked with, and a new one is refused until this reads", error.message);
            return Ok(Json(ProjectState { covered: None, id, code: project.code.clone(), path: folder.dir.clone(), assets: vec![], heads: vec![], queue, missing: vec![], stale: vec![], orphans: vec![], busy, absent: 0, run_files: 0, cuts: vec![], warnings: vec![ProjectWarning { name: None, message: warning }], notes: vec![], next: vec![], board: board_of(&project), unsent: vec![], machines: chosen(&api, &project.folder) }));
        }
    };
    // What a reader now holds is what any change from here is told against.
    project.told.lock().expect("told lock").get_or_insert_with(|| model::Told::now(&project, model.clone()));
    let mut state = model.state(&project, asked.assets);
    state.machines = chosen(&api, &project.folder);
    state.covered = covered;
    state.unsent = project.unsent.lock().expect("unsent lock").clone();
    state.warnings.extend(project.said.lock().expect("said lock").drain(..).map(ProjectWarning::of_project));
    copy_told(&project.root);
    Ok(Json(state))
}

/// `GET /v1/projects/{id}`.
#[derive(Deserialize)]
struct Read {
    /// Without them, what's said of the project besides its assets, whatever it holds.
    #[serde(default = "yes")]
    assets: bool,
}

fn yes() -> bool {
    true
}

/// Open the browser when the project needs an asset edit, transfer or render.
fn next_steps(orphans: &[Orphan], absent: u64, owed: bool) -> Vec<String> {
    (!orphans.is_empty() || absent > 0 || owed).then(|| "cozy open".to_owned()).into_iter().collect()
}

/// Its render going, or its last since Calcifer started.
fn board_of(project: &Project) -> Option<api::calcifer::project::Board> {
    Some(project.jobs.board()).filter(|board| board.seq > 0)
}

/// The local code `id`'s assets name, each once: its path as they write it, and where that is. Not a checkout's first-party
/// package, which is listed by its name (`toolbox::from_source`).
pub(crate) fn local_code_named(api: &Arc<Api>, id: &str) -> Result<Vec<(String, PathBuf)>, Refusal> {
    let project = api.projects.get(api, id)?;
    let folder = &project.folder;
    let doc = Doc::load(folder).map_err(Refusal)?;
    let home = &api.machines.user_home;
    let mut named = BTreeMap::new();
    for with in (0..doc.assets.len()).map(|i| doc.target(i)).filter(|with| !with.is_empty() && crate::checkout::named(with).is_none()) {
        if let Some(Ok((path, function))) = crate::local_code::target(&with, Some(&folder.dir), home) {
            let written = match function {
                Some(function) => with.trim().trim_end_matches(['/', '\\']).strip_suffix(function.as_str()).unwrap_or(&with).trim_end_matches(['/', '\\']).to_owned(),
                None => with.trim().trim_end_matches(['/', '\\']).to_owned(),
            };
            named.insert(path, written);
        }
    }
    Ok(named.into_iter().map(|(path, written)| (written, path)).collect())
}

/// What can be said of each asset whose published package has been heard of: what makes it, its tags. A read waits on no
/// Hub and writes nothing, so what isn't heard of yet is said by a later read, or a render. Local code is described only on a machine, so a render says it.
fn unmade(doc: &Doc, known: &HeardOf, lock: &Lock) -> Vec<ProjectWarning> {
    let mut said = vec![];
    for i in (0..doc.assets.len()).filter(|&i| doc.makes(i) && doc.tool(i).is_none()) {
        let Some(heard) = known.get(&doc.target(i)).filter(|heard| !heard.local) else { continue };
        if doc.tags(i).is_none() {
            said.extend(doc.said_once_read(i, &heard.tags, lock));
        }
        if doc.assets[i].pin.is_none() && !heard.makes {
            said.push(ProjectWarning::of(&doc.assets[i].name, no_function(&doc.assets[i].name, &heard.name)));
        }
    }
    said
}

/// What one read says of a package its assets name, worked out once for all of them.
struct HeardPackage {
    name: String,
    tags: Vec<String>,
    makes: bool,
    /// The function its tool names: what its boxes' text is bound by.
    callable: Option<api::interface::Callable>,
    /// Local code, read only for `callable`: its tags and function are said once a render reads it.
    local: bool,
}

/// Each package a read's assets name that's been heard of, by what they call it: published, or local code as it was last described.
type HeardOf = HashMap<String, HeardPackage>;

fn known_packages(api: &Arc<Api>, doc: &Doc, dir: &FsPath) -> HeardOf {
    let targets: BTreeSet<String> = (0..doc.assets.len()).filter(|&i| doc.makes(i) && doc.tool(i).is_none()).map(|i| doc.target(i)).collect();
    (targets.into_iter().filter(|with| !with.is_empty()))
        .filter_map(|with| {
            let package = heard_of(api, &with, dir)?;
            let callable = package.named.as_deref().and_then(|function| package.callable(function));
            // A checkout's first-party package is said as its release would be (xs-195).
            let local = is_local(&with) && crate::checkout::named(&with).is_none();
            let heard = HeardPackage { name: package.name.clone(), tags: package.reserved(), makes: matches!(package.maker(), Ok(Some(_))), callable, local };
            Some((with, heard))
        })
        .collect()
}

/// What asset `i`'s text mentions that its tool can't use, as its binder finds it: none until its package is heard of.
fn unused_of(reading: &Reading, i: usize) -> Vec<Unused> {
    let Reading { doc, lock, stand, known, .. } = *reading;
    let target = doc.target(i);
    let Some((heard, callable)) = known.get(&target).filter(|_| doc.makes(i) && doc.tool(i).is_none()).and_then(|heard| Some((heard, heard.callable.as_ref()?))) else {
        return vec![];
    };
    let fields = doc.inputs_of(i);
    let reserved = tags_now(doc, known, i).unwrap_or(heard.tags.as_slice());
    let call = binder::Call { name: &doc.assets[i].name, tool: &target, package: &heard.name, callable, fields: &fields, reserved, doc, own: Some(i), lock, stand, gone: &BTreeSet::new(), pickup: None, inline: &[] };
    binder::unused(&call)
}

/// The tags of what makes asset `i`, this moment: none of what Calcifer makes itself. None: they aren't known yet, and its names held back.
fn tags_now<'a>(doc: &'a Doc, known: &'a HeardOf, i: usize) -> Option<&'a [String]> {
    if doc.tool(i).is_some() {
        return Some(&[]);
    }
    if let Some(tags) = doc.tags(i) {
        return Some(tags);
    }
    known.get(&doc.target(i)).filter(|heard| !heard.local).map(|heard| heard.tags.as_slice())
}

/// Whether `with` names local code.
fn is_local(with: &str) -> bool {
    api::target::written_as_path(with) || with.ends_with(".py") || crate::checkout::named(with).is_some()
}

/// What was last heard of the package `with`, this moment: published, or local code from `dir` as described on this computer's machine.
/// If that isn't recent, it is asked again in the background.
fn heard_of(api: &Arc<Api>, with: &str, dir: &FsPath) -> Option<Arc<Package>> {
    let local = is_local(with);
    // Local code by where it is: two projects' `./tool.py` are two packages, and one absolute path is one however many projects name it.
    let key = match local && !with.starts_with('~') && !FsPath::new(with).is_absolute() {
        true => dir.join(with).display().to_string(),
        false => with.to_owned(),
    };
    let mut heard = api.projects.heard.lock().expect("heard lock");
    let entry = heard.entry(key.clone()).or_default();
    if entry.asked.is_none_or(|asked| asked.elapsed() > HEARD_FOR) {
        entry.asked = Some(Instant::now());
        let (api, with, dir) = (api.clone(), with.to_owned(), dir.to_path_buf());
        tokio::spawn(async move {
            let found = match local {
                true => Host::new(&api).described(&with, &dir).await,
                false => Host::new(&api).published("", &with, None).await,
            };
            if let Ok(package) = found {
                api.projects.heard.lock().expect("heard lock").entry(key).or_default().package = Some(Arc::new(package));
                api.projects.heard_changes.fetch_add(1, Ordering::Relaxed);
                api.projects.now_open().iter().for_each(|project| crate::events::project_settled(&project.folder.dir));
            }
        });
    }
    entry.package.clone()
}

/// Keeps `package`'s tags, and what its function outputs, in the lock for what reads the project. Whether that says what asset `i` is for the first time.
fn read_tags(doc: &Doc, folder: &Folder, lock: &Lock, i: usize, package: &Package) -> bool {
    let (key, tags, makes) = (doc.assets[i].tool.clone().unwrap_or_default(), package.reserved(), package.named_kind());
    let same = lock.reserved.get(&key) == Some(&tags) && (makes.is_none() || lock.makes.get(&key) == makes.as_ref());
    if !same
        && let Err(error) = folder.update_lock(|lock| {
            lock.reserved.insert(key.clone(), tags.clone());
            lock.makes.extend(makes.clone().map(|makes| (key.clone(), makes)));
        })
    {
        warn!("{}: {error}", key);
    }
    !doc.assets[i].tool_makes && makes.is_some()
}

/// The line that says an asset's tool names a package and no function.
fn no_function(name: &str, package: &str) -> String {
    format!("{name}: a tool is a function, and {package} has no one function that makes it, so name one: `{package}/<function>`")
}

/// The version of cut `asset` that cutting it again from `inputs` would only make again: those inputs, nothing left out, and its film
/// here and as long as its pieces.
fn same_cut<'a>(folder: &Folder, lock: &'a Lock, asset: &format::Asset, reads: &[format::ReadNow], inputs: &str) -> Option<&'a Version> {
    let film_stands = |old: &Version| old.files.get("video").and_then(|sha256| lock.files.get(sha256)).is_some_and(|film| folder.path(&film.path).is_file());
    lock.asset(&asset.name)?.versions.iter().rev()
        .find(|old| old.inputs == inputs && old.without.is_empty() && film_stands(old) && tools::film_off(&asset.inputs, reads, lock, old).is_none())
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
    let canvas = Canvas::of(&asset.inputs, &mut vec![]);
    let (mut pieces, skips) = tools::pieces(&asset.inputs, &stand[i].reads, folder, lock);
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

/// What a cut's version was cut from: each clip's asset, once, and its version when it was one.
fn cut_from(lock: &Lock, version: &Version) -> Vec<String> {
    let mut from: Vec<String> = vec![];
    for clip in &version.clips {
        let name = graph::mentions(&clip.clip).first().map_or_else(|| clip.clip.clone(), |name| (*name).to_owned());
        let number = clip.sha256.as_ref().and_then(|sha256| lock.asset(&name)?.versions.iter().find(|held| held.main() == Some(sha256)));
        let said = number.map_or_else(|| name.clone(), |held| format!("{name} {}", held.number));
        if !from.iter().any(|there| there.split(' ').next() == Some(name.as_str())) {
            from.push(said);
        }
    }
    from
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
fn listed(lock: &Lock, files: &Files) -> BTreeMap<String, Listing> {
    (files.iter())
        .filter_map(|(name, sha256)| {
            let file = lock.files.get(sha256)?;
            let (absent, machine) = whereabouts(file);
            Some((name.clone(), Listing { path: file.path.clone(), sha256: sha256.clone(), absent, machine, metadata: Map::new() }))
        })
        .collect()
}

/// Whether a file isn't on this computer, and the machine that keeps it if one does.
fn whereabouts(file: &folder::File) -> (bool, Option<String>) {
    let absent = !file.here;
    (absent, (absent && file.stays()).then(|| file.machine.clone()))
}

/// The file people mean by `files`, as a version lists it.
fn main_file(lock: &Lock, version: &Version) -> Option<MainFile> {
    let sha256 = version.main()?;
    let file = lock.files.get(sha256)?;
    let output = version.main_output()?.clone();
    let metadata = version.metadata.get(&output).cloned().unwrap_or_default();
    let (absent, machine) = whereabouts(file);
    let measured = |number: u64| (number > 0).then_some(number);
    Some(MainFile {
        output,
        path: file.path.clone(),
        sha256: sha256.clone(),
        field: file.field().to_owned(),
        width: measured(file.width),
        height: measured(file.height),
        duration_ms: measured(file.lasts_ms()),
        fps: (file.fps > 0.0).then_some(file.fps),
        alpha: file.alpha > 0.0,
        absent,
        machine,
        frames: measured(file.frames),
        metadata,
    })
}

/// The most metadata a file keeps, as its Runtime bounds it.
pub(super) const METADATA_MAX: usize = 4096;

/// What a run's `result` says of the file it returned at output `name` (`pair.thumb`, `takes[2].video`), a list's item `index` (1 first):
/// the `metadata` its function declared on it (`File.with_metadata`).
fn declared(result: Option<&Value>, name: &str, index: u32) -> Result<Map<String, Value>, String> {
    let mut at = result;
    for part in name.split('.') {
        let (field, item) = part.split_once('[').map_or((part, None), |(field, rest)| (field, rest.trim_end_matches(']').parse::<usize>().ok()));
        at = at.and_then(|value| value.get(field));
        if let Some(item) = item {
            at = at.and_then(|value| value.get(item));
        }
    }
    if index > 0 && at.is_some_and(Value::is_array) {
        at = at.and_then(|value| value.get(index as usize - 1));
    }
    let Some(said) = at.and_then(|node| node.get("metadata")) else { return Ok(Map::new()) };
    let said = said.as_object().ok_or("is not a map")?;
    let length = serde_json::to_vec(said).map_or(usize::MAX, |bytes| bytes.len());
    if length > METADATA_MAX {
        return Err(format!("is {length} bytes, over {METADATA_MAX}"));
    }
    Ok(said.clone())
}

/// The path of the file people mean by `files`.
fn main_path(lock: &Lock, files: &Files) -> Option<String> {
    folder::main_of(files).and_then(|sha256| lock.files.get(sha256)).map(|file| file.path.clone())
}

/// What a read of a project works out once, for all its rows.
#[derive(Clone, Copy)]
struct Reading<'a> {
    doc: &'a Doc,
    folder: &'a Folder,
    lock: &'a Lock,
    stand: &'a [Standing],
    known: &'a HeardOf,
}

/// The runs making each asset, by name key.
fn making_now(walk: &folder::WalkFile) -> HashMap<String, Vec<u64>> {
    let mut making: HashMap<String, Vec<u64>> = HashMap::new();
    for pending in walk.pending.values().filter(|pending| pending.run > 0) {
        making.entry(name_key(&pending.asset)).or_default().push(pending.run);
    }
    making
}

/// Each asset's takes asked and not landed, by name key, in the order they go (cl-945), by id: each open job of the walk, what an ask holds
/// that the walk hasn't taken in yet, and what was sent. Sent, with its run once that has a number.
fn takes_now(project: &Project, walk: &folder::WalkFile) -> HashMap<String, Vec<TakeNow>> {
    let runs: HashMap<&str, u64> = walk.pending.iter().map(|(key, pending)| (pending.job_of(key), pending.run)).collect();
    let mut takes: HashMap<String, Vec<TakeNow>> = HashMap::new();
    let mut listed = HashSet::new();
    let mut list = |asset: &str, id: &str| {
        if listed.insert(id.to_owned()) {
            let run = runs.get(id).copied();
            takes.entry(name_key(asset)).or_default().push(TakeNow { id: id.to_owned(), queued: run.is_none(), run: run.filter(|run| *run > 0), failure: None });
        }
    };
    {
        let walker = project.walker.lock().expect("walker lock");
        let graph = &walker.graph;
        graph.open_jobs().into_iter().for_each(|id| list(&graph.jobs[id].asset, &graph.jobs[id].key));
    }
    walk.walk.asks.iter().flat_map(|ask| &ask.takes).for_each(|take| list(&take.asset, &take.key));
    walk.pending.iter().for_each(|(key, pending)| list(&pending.asset, pending.job_of(key)));
    takes
}

/// Asset `i` as `GET /v1/projects/{id}` lists it.
fn row(reading: &Reading, i: usize) -> Row {
    let Reading { doc, folder, lock, stand, known } = *reading;
    let (asset, standing) = (&doc.assets[i], &stand[i]);
    let noun = noun(doc, lock, i);
    let held = lock.asset(&asset.name);
    let files = standing.files.clone().unwrap_or_default();
    let gone: Vec<&str> = (files.values())
        .filter(|sha256| lock.files.get(*sha256).is_none_or(|file| folder.missing(file)))
        .map(|sha256| folder::prefix_of(sha256))
        .collect();
    let status = match &standing.state {
        _ if !gone.is_empty() => format!("not on this computer: {}; download it in the browser opened by cozy open", gone.join(", ")),
        Stand::Pinned { version: None, .. } => "approved".into(),
        Stand::Pinned { version: Some(version), changed } if changed.is_empty() => format!("approved: {noun} {version}"),
        Stand::Pinned { version: Some(version), changed } => format!("approved: {noun} {version}; its inputs changed: {}", changed.join(", ")),
        Stand::Current(_) => "current".into(),
        Stand::Words => "only words".into(),
        Stand::Waits => "waits for what it reads".into(),
        Stand::Stale(why) => format!("out of date: {why}"),
        Stand::Missing if asset.hollow => "it has nothing to make".into(),
        Stand::Missing => format!("no {noun} yet"),
    };
    let made = standing.version().and_then(|number| held?.version(number));
    let main = folder::main_of(&files).and_then(|sha256| lock.files.get(sha256));
    let measured = |number: fn(&folder::File) -> u64| main.map(number).filter(|&number| number > 0);
    let (absent, machine) = main.map_or((false, None), whereabouts);
    let failure = held.and_then(|held| held.failed.as_ref()).filter(|failed| failed.inputs == standing.inputs).map(|failed| failed.error.clone());
    let changed = match &standing.state {
        Stand::Pinned { changed, .. } => changed.clone(),
        _ => vec![],
    };
    let row_standing = match &standing.state {
        Stand::Stale(_) => RowStanding::OutOfDate,
        Stand::Pinned { .. } if !changed.is_empty() => RowStanding::OutOfDate,
        Stand::Missing | Stand::Waits => RowStanding::Missing,
        _ => RowStanding::Current,
    };
    Row {
        name: asset.name.clone(),
        standing: row_standing,
        making: vec![],
        // Its takes that failed, each with why (cl-954); what's on its way goes over them (`overlaid`).
        takes: (held.into_iter().flat_map(|held| held.ended.iter().rev()))
            .map(|ended| TakeNow { id: ended.id.clone(), queued: false, run: ended.run, failure: Some(ended.error.clone()) })
            .collect(),
        kind: asset.kind.clone(),
        noun: noun.into(),
        tool: doc.makes(i).then(|| doc.target(i)),
        versions: held.map_or(0, |held| held.versions.len() as u32),
        prefixes: (held.into_iter().flat_map(|held| &held.versions))
            .filter_map(|version| {
                Some(VersionPrefix {
                    number: version.number,
                    id: version.id.clone(),
                    prefix: folder::prefix_of(version.main()?).to_owned(),
                    pickup: version.pickup.clone(),
                    latents: version.files.get("latents").and_then(|sha256| lock.files.get(sha256)).filter(|file| file.stays()).map(|file| file.machine.clone()),
                    execution_ms: (version.execution_ms > 0).then_some(version.execution_ms),
                    run: version.run,
                    main: main_file(lock, version),
                    from: cut_from(lock, version),
                    without: version.without.iter().map(folder::Without::said).collect(),
                    timeline: (!version.clips.is_empty()).then(|| {
                        let files = |name: &str| {
                            let cut = version.clips.iter().find(|clip| clip.sha256.is_some() && graph::mentions(&clip.clip).first().is_some_and(|mention| name_key(mention) == name_key(name)));
                            let why = || version.without.iter().find(|left| name_key(&left.piece) == name_key(name)).map_or_else(|| "no longer in the project".to_owned(), |left| left.why.clone());
                            cut.and_then(|clip| Some(Files::from([("file".to_owned(), clip.sha256.clone()?)]))).ok_or_else(why)
                        };
                        tools::timeline_now(version.clips.clone(), version.fps, files, lock)
                    }),
                })
            })
            .collect(),
        version: standing.version(),
        approved: asset.pin.as_ref().and(standing.version()),
        pinned: asset.pin.is_some(),
        starred: asset.starred,
        inputs: standing.inputs.clone(),
        path: main_path(lock, &files),
        files: listed(lock, &files),
        sha256: main.map(|file| file.sha256.clone()),
        field: main.map(|file| file.field().to_owned()),
        width: measured(|file| file.width),
        height: measured(|file| file.height),
        duration_ms: measured(folder::File::lasts_ms),
        alpha: main.is_some_and(|file| file.alpha > 0.0),
        absent,
        machine,
        made_by: made.map(|made| made.tool.clone().unwrap_or_else(|| doc.target(i))).filter(|made_by| !made_by.is_empty()),
        status,
        reads: doc.reads_of(i, &standing.reads),
        unused: unused_of(reading, i),
        fields: doc.flat_fields(i),
        etag: None,
        failure,
        changed,
        missing: tags_now(doc, known, i).map_or_else(|| doc.versions_missing(i, lock), |tags| doc.missing_names(i, tags, lock)),
        timeline: (doc.tool(i) == Some(Tool::Cut)).then(|| {
            let files = |name: &str| {
                let read = standing.reads.iter().find(|read| name_key(&read.name) == name_key(name));
                let why = || if doc.find(name).is_some() { "no version yet" } else { "no longer in the project" }.to_owned();
                read.map(|read| read.files.clone()).ok_or_else(why)
            };
            tools::timeline_now(tools::clips(&asset.inputs, &standing.reads), Canvas::of(&asset.inputs, &mut vec![]).fps, files, lock)
        }),
        rendering: None,
        unseen: vec![],
    }
}

/// The assets the lock keeps files of that the project no longer names, deleted boxes whose undo window hasn't ended among them, each
/// with the one new asset of its kind with no versions, if there is exactly one: what a rename left. Each file's bytes count
/// once, and only if nothing else names it.
fn orphans(doc: &Doc, folder: &Folder, lock: &Lock) -> Vec<Orphan> {
    let kept = kept_files(doc, lock);
    let empty = |kind: &str| -> Vec<&str> {
        (0..doc.assets.len())
            .filter(|&i| doc.makes(i) && doc.assets[i].kind == kind && lock.asset(&doc.assets[i].name).is_none_or(|held| held.versions.is_empty()))
            .map(|i| doc.assets[i].name.as_str())
            .collect()
    };
    let mut counted = BTreeSet::new();
    let now = now_ms();
    (lock.assets.iter())
        .filter(|(key, held)| doc.find(key).is_none() && held.files().next().is_some() && !held.lapsed(now))
        .map(|(key, held)| {
            let freed: Vec<&String> = held.files().filter(|sha256| !kept.contains(*sha256) && counted.insert((*sha256).clone())).collect();
            Orphan {
                name: if held.name.is_empty() { key.clone() } else { held.name.clone() },
                kind: held.kind.clone(),
                versions: held.on_record().count() as u32,
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
    let mut kept: BTreeSet<String> = lock.named(|key| doc.find(key).is_some()).into_iter().map(str::to_owned).collect();
    kept.extend(doc.assets.iter().filter_map(|asset| asset.pin.clone()));
    let clips = lock.assets.iter().filter(|(key, _)| doc.find(key).is_some()).flat_map(|(_, held)| &held.versions).flat_map(|version| &version.clips);
    kept.extend(clips.filter_map(|clip| clip.sha256.clone()));
    for (key, numbers) in doc.versions_named() {
        let read = lock.rows_of(&key).flat_map(|(_, held)| held.on_record()).filter(|version| numbers.contains(&version.number));
        kept.extend(read.flat_map(|version| version.files.values().cloned()));
    }
    kept
}

/// `POST /v1/projects/{id}/reclaim`: what it would free, and with `yes` frees it: orphans' versions, and any file nothing else
/// names. What a run left in `.runs/` that `files/` doesn't hold stays, listed. Refused while the project renders. Never automatic.
async fn reclaim(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Reclaimed>, Refusal> {
    let asked: Reclaim = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    let folder = &project.folder;
    let filter = crate::store::RunFilter {
        status: None,
        target: None,
        machine: None,
        out_dir: Some(landing(&folder.dir).to_string_lossy().into_owned()),
        before: None,
        limit: 100,
    };
    let going = (api.calcifer.project_store(&project.id).and_then(|store| store.runs(&filter)).map(|(rows, ..)| rows).unwrap_or_default().iter()).any(|row| row.state != crate::store::State::Settled);
    if asked.yes && (going || *project.walking.lock().expect("walking lock")) {
        let said = "the project is rendering: reclaim once its runs land";
        return Err(Refusal(Error::new(Kind::Conflict, "project_busy", said).next("cozy status")));
    }
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let (orphans, kept) = (orphans(&doc, folder, &lock), kept_files(&doc, &lock));
    let lost = folder.history_lost(&lock, &kept);
    let recorded = lock.recorded();
    if let (true, Some(why)) = (asked.yes, &lost) {
        let remedy = format!("put {} back from a backup, or delete what you don't want from files/ by hand", db::DB);
        return Err(Refusal(Error::new(Kind::Conflict, "project_history_lost", why.clone()).remedy(remedy).next("cozy status")));
    }
    if asked.yes {
        // An orphaned cut's film at the root goes with it.
        for held in lock.assets.iter().filter(|(key, _)| doc.find(key).is_none()).map(|(_, held)| held) {
            let cut = held.versions.iter().any(|version| !version.clips.is_empty());
            if cut {
                let name = cut_name(&held.name);
                let _ = std::fs::remove_file(folder.dir.join(&name));
                let _ = std::fs::remove_file(folder.dir.join(FsPath::new(&name).with_extension("otio")));
            }
        }
        // What a text names by number stays, on any row.
        let named = doc.versions_named();
        let row_named: BTreeMap<String, BTreeSet<u32>> = named.iter().flat_map(|(key, numbers)| lock.rows_of(key).map(move |(row, _)| (row.clone(), numbers.clone()))).collect();
        folder
            .update_lock(|lock| {
                lock.assets.retain(|key, _| doc.find(key).is_some() || row_named.contains_key(key));
                for (key, held) in &mut lock.assets {
                    match row_named.get(key).filter(|_| doc.find(key).is_none()) {
                        Some(numbers) => held.keep_only(numbers),
                        None => held.clear_discarded(row_named.get(key)),
                    }
                }
            })
            .map_err(Refusal)?;
    }
    let (files, bytes, unrecorded, unheld) = folder.sweep(&kept, &recorded, asked.yes).map_err(Refusal)?;
    if asked.yes {
        canvas::prune(folder, &id, |name| doc.find(name).is_some()).map_err(Refusal)?;
    }
    let next = (!asked.yes && lost.is_none() && (files > unrecorded || !orphans.is_empty())).then(|| "cozy open".to_owned()).into_iter().collect();
    Ok(Json(Reclaimed { orphans, files, bytes, unrecorded, deleted: asked.yes, next, lost, unheld }))
}

/// Renames asset `from` to `to`, at once, whatever is going, and nothing else about it changes: the lock's row and all it keeps, its pin and what is pending or owed of
/// it, what renders going say of it, its row on the canvas, and its box's name and every `<from>`, unless the project has moved to `to` already. A name is no input, so
/// what stood on the text that said `from` stands on the text that says `to`. The lock goes before the text, and the old name finds its row until the text says so: a read
/// between is never of an empty box. Nothing on disk moves.
async fn rename(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Renamed>, Refusal> {
    let asked: Rename = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    rename_now(&project, &id, asked).map(Json)
}

/// `PUT /v1/projects/{id}/machines` (cl-873): the machines its runs go to, each by its name or rental id as `cozy machine ls` lists it;
/// none: any machine. One offline stays: a run none of them can take says why. Every reader of the project hears it.
async fn machines(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ProjectMachines>, Refusal> {
    let asked: ProjectMachines = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let mut kept: Vec<String> = vec![];
    for written in asked.machines.iter().filter(|name| !name.trim().is_empty()) {
        let name = match On::from(written.clone()) {
            On::Machine(name) if name == "local" => {
                api::calcifer::local_usable(&name).map_err(Refusal)?;
                name
            }
            On::Machine(name) => api.machines.named_now(&name).await.map_err(Refusal)?.name,
            other => {
                let said = format!("{other} is no machine: a project's runs go to machines by name, or with none to any");
                return Err(Refusal(Error::new(Kind::Usage, "project_machines", said).next("cozy machine ls")));
            }
        };
        if !kept.contains(&name) {
            kept.push(name);
        }
    }
    project.folder.set_machines(&kept).map_err(Refusal)?;
    crate::events::project_settled(&project.folder.dir);
    Ok(Json(ProjectMachines { machines: kept }))
}

/// Asset `from` called `to`, as `POST rename` has it, in one transaction.
fn rename_now(project: &Arc<Project>, id: &str, asked: Rename) -> Result<Renamed, Refusal> {
    // Under `walking`, as a walk's end is: it is over, or it follows this. Taken before the transaction, as every writer takes it.
    let walking = project.walking.lock().expect("walking lock");
    let done = renamed_under(project, id, asked, *walking);
    drop(walking);
    done
}

/// `rename_now` with `walking` held by the caller, whose walk `follows` is going.
fn renamed_under(project: &Arc<Project>, id: &str, asked: Rename, follows: bool) -> Result<Renamed, Refusal> {
    format::named(&asked.to).map_err(Refusal)?;
    let folder = &project.folder;
    let done = folder.act(|| {
        let refuse = |kind, code, said: String| Error::new(kind, code, said);
        let written = folder.written()?;
        let lock = folder.lock()?;
        let was = Doc::of(folder, &written, &lock)?;
        let same = name_key(&asked.to) == name_key(&asked.from);
        if was.find(&asked.to).is_none() && !same {
            format::new_name(&asked.to)?;
        }
        if was.find(&asked.from).is_some_and(|i| was.assets[i].text.is_some()) {
            format::refuse_tag(&lock, &asked.to)?;
        }
        let free = was.find(&asked.to).is_none() || same;
        let writes = was.find(&asked.from).is_some() && free;
        let said = match writes {
            true => {
                let mut said = written.clone();
                said.renamed(&asked.from, &asked.to)?;
                said
            }
            false => written,
        };
        let doc = if writes { Doc::of(folder, &said, &lock)? } else { was };
        let Some(i) = doc.find(&asked.to) else {
            return Err(refuse(Kind::NotFound, "asset_unknown", format!("the project names no {}", asked.to)));
        };
        let mut back = lock.clone();
        back.untombstone(&asked.from);
        let old = back.asset(&asked.from).filter(|held| held.on_record().next().is_some());
        if old.is_none() && !free && doc.find(&asked.from).is_some() {
            return Err(refuse(Kind::Conflict, "rename_taken", format!("{} is in the project already", asked.to)));
        }
        if old.is_none() && !free && canvas::row(folder, &asked.from)?.is_none() {
            let said = format!("the project keeps no version of {}", asked.from);
            return Err(Error::new(Kind::NotFound, "asset_unknown", said).next("cozy status"));
        }
        let to = doc.assets[i].name.clone();
        if old.is_some() && !same && lock.asset(&to).is_some_and(|held| held.on_record().next().is_some()) {
            return Err(refuse(Kind::Conflict, "rename_taken", format!("{to} has versions of its own already")));
        }
        let (from, versions) = (asked.from.clone(), old.map_or(0, |old| old.versions.len() as u32));
        let saved_reads = lock.assets.values().flat_map(|held| held.on_record())
            .chain(lock.pending.values().map(|pending| &pending.version)).filter_map(|version| version.fields.as_ref())
            .any(|fields| fields.values().any(|value| graph::mentions_in(value).iter().any(|name| graph::mention_moved(name, &from, &to).is_some())));
        let stamps = restamps(folder, &doc, &back, &from, &to, &said);
        canvas::moved(folder, id, &from, &to)?;
        if follows || old.is_some() || !stamps.is_empty() || saved_reads {
            folder.update_lock(|lock| {
                renamed(lock, &from, &to);
                lock.called(&from, &to);
                stamps.iter().for_each(|stamp| stamp.written(lock));
            })?;
        }
        if writes {
            folder.edit(|written| written.renamed(&from, &to))?;
        }
        match follows {
            true => project.rename_walk(&from, &to)?,
            false => folder.update_lock(|lock| lock.walk.renames.retain(|(was, _)| name_key(was) != name_key(&from)))?,
        }
        if doc.tool(i) == Some(Tool::Cut) {
            let (dir, from, to) = (folder.dir.clone(), from.clone(), to.clone());
            folder.then(move |_| film_renamed(&Folder::at(&dir), &from, &to));
        }
        history::renamed(folder, &from, &to);
        let renamed = Renamed { from, to, versions };
        // Its answer is kept with it, in its transaction (cl-885): an undo that renames back says its own after.
        keyed::answering(StatusCode::OK, &renamed);
        Ok(renamed)
    });
    done.map_err(Refusal)
}

/// One asset's key moved by a rename, its inputs the same: what stood on `was` stands on `is`.
struct Restamp {
    key: String,
    was: String,
    is: String,
    /// The version that was its state, on whatever key.
    stood: Option<u32>,
}

impl Restamp {
    fn written(&self, lock: &mut Lock) {
        lock.pending.values_mut().filter(|pending| name_key(&pending.asset) == self.key && pending.version.inputs == self.was).for_each(|pending| pending.version.inputs.clone_from(&self.is));
        let Some(held) = lock.assets.get_mut(&self.key) else { return };
        held.tried.iter_mut().filter(|tried| tried.inputs == self.was).for_each(|tried| tried.inputs.clone_from(&self.is));
        held.failed.iter_mut().filter(|failed| failed.inputs.as_ref() == Some(&self.was)).for_each(|failed| failed.inputs = Some(self.is.clone()));
        let stood = |version: &Version| version.inputs == self.was || Some(version.number) == self.stood;
        held.versions.iter_mut().chain(&mut held.discarded).filter(|version| stood(version)).for_each(|version| version.inputs.clone_from(&self.is));
    }
}

/// What renaming `from` to `to` moves of each key, `doc` being `written`, which says `to`, and `lock` what stands of `from`. It's read from that
/// said back, the same whether this call renames it or an import did. What was out of date stays so.
fn restamps(folder: &Folder, doc: &Doc, lock: &Lock, from: &str, to: &str, written: &written::Written) -> Vec<Restamp> {
    let mut back = written.clone();
    let then = back.renamed(to, from).and_then(|()| Doc::of(folder, &back, lock));
    let Ok(then) = then else { return vec![] };
    let mut after = lock.clone();
    renamed(&mut after, from, to);
    let (was, is) = (then.stand(lock), doc.stand(&after));
    let moved = |i: usize| {
        let name = &doc.assets[i].name;
        let j = then.find(if name_key(name) == name_key(to) { from } else { name })?;
        let (old, new) = (was[j].inputs.clone()?, is[i].inputs.clone()?);
        let stood = was[j].version().filter(|_| !matches!(&was[j].state, Stand::Pinned { changed, .. } if !changed.is_empty()));
        (old != new).then(|| Restamp { key: name_key(name), was: old, is: new, stood })
    };
    (0..doc.assets.len()).filter_map(moved).collect()
}

/// A cut's film and timeline at the root go by its new name. A file already there under it is someone's, and stays: the next walk sets it aside.
fn film_renamed(folder: &Folder, from: &str, to: &str) {
    let (from, to) = (folder.dir.join(cut_name(from)), folder.dir.join(cut_name(to)));
    for (from, to) in [(from.with_extension("otio"), to.with_extension("otio")), (from, to)] {
        if !to.exists()
            && let Err(error) = std::fs::rename(&from, &to)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!("{}: {error}", from.display());
        }
    }
}

/// The lock with asset `from` called `to`: its row, what is pending or owed of it, and what a cut's versions say of it.
pub(super) fn renamed(lock: &mut Lock, from: &str, to: &str) {
    lock.untombstone(from);
    let key = name_key(from);
    let move_snapshot = |version: &mut Version| {
        if let Some(fields) = &mut version.fields {
            fields.values_mut().for_each(|value| written::mentions_renamed(value, &|name| graph::mention_moved(name, from, to)));
        }
    };
    for version in lock.assets.values_mut().flat_map(|held| held.versions.iter_mut().chain(&mut held.discarded)) {
        move_snapshot(version);
        for clip in &mut version.clips {
            if let Some(moved) = graph::mentions(&clip.clip).first().and_then(|name| graph::mention_moved(name, from, to)) {
                clip.clip = format!("<{moved}>");
            }
        }
        version.without.iter_mut().filter(|left| name_key(&left.piece) == key).for_each(|left| left.piece = to.to_owned());
    }
    if let Some(mut held) = lock.assets.remove(&key) {
        held.name = to.to_owned();
        lock.assets.insert(name_key(to), held);
    }
    for pending in lock.pending.values_mut() {
        move_snapshot(&mut pending.version);
        if name_key(&pending.asset) == key {
            pending.asset = to.into();
        }
    }
    for ask in &mut lock.walk.asks {
        for take in ask.takes.iter_mut().filter(|take| name_key(&take.asset) == key) {
            take.asset = to.into();
        }
        if ask.sent.remove(&key) {
            ask.sent.insert(name_key(to));
        }
    }
    if let Some(number) = lock.walk.numbered.remove(&key) {
        lock.walk.numbered.insert(name_key(to), number);
    }
    lock.walk.runs.values_mut().flatten().filter(|coming| name_key(&coming.name) == key).for_each(|coming| to.clone_into(&mut coming.name));
}

/// Adds what `Runs` asks to the project's walk, answered at once (`202`; `200` when its
/// `Idempotency-Key` asked it already); its board on `GET /v1/events` says how it goes.
async fn runs(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Asked>), Refusal> {
    let body = body?;
    let mut asked: Runs = parse(&body)?;
    api::calcifer::machine_set(&asked.on).map_err(Refusal)?;
    if asked.on.iter().any(|on| matches!(on, On::New(_))) {
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
    api.calcifer.project_store(&project.id)?.admit_new_work()?;
    asked.on=crate::private_io::check_project_placement(&project.id,&project.root,&asked.on).map_err(Refusal)?;
    let id = project.id.clone();
    // None named: where `cozy run` naming none goes (cl-894).
    let on = match asked.on.is_empty() {
        true => crate::runs::unnamed_set(&api, Some(&id)).await.map_err(Refusal)?,
        false => asked.on,
    };
    project.unsent.lock().expect("unsent lock").clear();
    // What this render uses: the project as it is now, its snapshot kept by digest until the render ends.
    let written = project.folder.written().map_err(Refusal)?;
    let doc = Doc::asked(&project.folder, &written).map_err(Refusal)?;
    let (digest, snapshot) = written.snapshot();
    let target = Target { on, hub };
    let all = matches!(asked.names.as_slice(), [only] if name_key(only) == name_key(ALL));
    let lock = project.folder.lock().map_err(Refusal)?;
    let stand = doc.stand(&lock);
    // What the ask finds is its answer's: the walk it joins or begins says only what it does.
    let lines = std::cell::RefCell::new(Vec::<Said>::new());
    let say = |message: String| lines.borrow_mut().push(Said { level: NoteLevel::Warning, message, names: vec![] });
    let said = |name: &str, message: String| lines.borrow_mut().push(Said { level: NoteLevel::Info, message, names: vec![name.to_owned()] });
    // Several are for a node you name: of everything it's more than anyone wants, so one of each.
    let n = if all { 1 } else { asked.n.max(1) };
    if all && asked.n > 1 {
        say(format!("-n is for an asset you name: `all` makes one of each, not {}", asked.n));
    }
    // Every render makes new versions, unless it asks only to bring what it names up to date (`cozy render`): then what has its state
    // stands, and `-n` is how many takes of each named node that renders.
    let missing = asked.missing;
    let outdated = asked.outdated;
    let picking = asked.pickup.is_some();
    if picking && (all || asked.names.len() != 1) {
        say(format!("a pickup is for one box: with {} it's ignored", if all { "all" } else { "several names" }));
    }
    if asked.approve && (all || asked.names.len() != 1) {
        say("--approve is for one asset: it is ignored".into());
    }
    let mut named = vec![];
    let mut first = None;
    // Selected media is its current state, not a request to exclude its generator
    // from Render all. Timeline can likewise make the video's next version.
    let mut skipped = vec![];
    let mut covering = |i: usize, named: &mut Vec<usize>| {
        if doc.tool(i) == Some(Tool::Cut) {
            named.push(i);
            return;
        }
        match doc.assets[i].pin.is_some() {
            // What this asks of it is judged at its turn.
            true if outdated && matches!(stand[i].state, Stand::Pinned { version: Some(_), .. }) => {}
            true if format::KINDS.contains(&doc.assets[i].kind.as_str()) => named.push(i),
            true if !doc.assets[i].hollow && !missing => skipped.push(doc.assets[i].name.clone()),
            true => {}
            false => named.push(i),
        }
    };
    if all {
        if !missing {
            for &i in doc.order.iter().filter(|&&i| doc.makes(i) && (doc.tool(i).is_none() || format::KINDS.contains(&doc.assets[i].kind.as_str()))) {
                covering(i, &mut named);
            }
        }
    } else {
        for name in &asked.names {
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
    let asked_of: HashSet<usize> = named.iter().copied().collect();
    // Whether anything `i` reads, all the way up, is asked of here too: then what it reads is about to change.
    let reads_asked = |i: usize| {
        let (mut up, mut seen) = (doc.upstream[i].clone(), HashSet::new());
        while let Some(j) = up.pop() {
            if seen.insert(j) {
                up.extend(&doc.upstream[j]);
            }
        }
        !seen.is_disjoint(&asked_of)
    };
    // Already on its way: a run of it with these inputs, or a job of a render of the project as its text says now, which reads what
    // stands when its turn comes.
    let now: HashSet<&str> = lock.walk.asks.iter().filter(|ask| ask.digest == digest).map(|ask| ask.id.as_str()).collect();
    let open: Vec<String> = {
        let walker = project.walker.lock().expect("walker lock");
        walker.graph.open_jobs().into_iter().map(|id| &walker.graph.jobs[id]).filter(|job| now.contains(job.ask.as_str())).map(|job| name_key(&job.asset)).collect()
    };
    let being_made = |i: usize| {
        let key = name_key(&doc.assets[i].name);
        lock.pending.values().any(|pending| name_key(&pending.asset) == key && Some(&pending.version.inputs) == stand[i].inputs.as_ref())
            || lock.walk.asks.iter().any(|ask| ask.digest == digest && ask.takes.iter().any(|take| name_key(&take.asset) == key))
            || open.contains(&key)
    };
    // Everything `i` reads, all the way up.
    let above = |i: usize| {
        let (mut up, mut seen) = (doc.upstream[i].clone(), HashSet::new());
        while let Some(j) = up.pop() {
            if seen.insert(j) {
                up.extend(&doc.upstream[j]);
            }
        }
        seen
    };
    // `--outdated` of names: each approved node out of date among them and what they read, re-run at its turn.
    let judges: BTreeSet<String> = match outdated && !all {
        false => BTreeSet::new(),
        true => (named.iter().flat_map(|&i| above(i).into_iter().chain([i])))
            .filter(|&j| doc.makes(j) && doc.tool(j).is_none() && matches!(stand[j].state, Stand::Pinned { version: Some(_), .. }) && pin_changed(&stand, j) && !being_made(j))
            .map(|j| name_key(&doc.assets[j].name))
            .collect(),
    };
    let judged = |j: usize| judges.contains(&name_key(&doc.assets[j].name));
    // A node named that reads, all the way up, something out of date or with no take isn't brought up to date by standing: a cut reads
    // an out-of-date clip's newest take, so it can stand on one its words no longer make. What it reads renders first, then it. A read
    // of one take (`<shot_2.1>`) is that take, whatever its box's words say now.
    let behind = |i: usize| {
        if all || !matches!(stand[i].state, Stand::Current(_)) {
            return false;
        }
        let (mut up, mut seen) = (vec![i], HashSet::new());
        while let Some(k) = up.pop() {
            for j in doc.assets[k].reads.iter().filter(|read| read.number.is_none()).map(|read| read.asset) {
                if !seen.insert(j) {
                    continue;
                }
                if doc.makes(j) && matches!(stand[j].state, Stand::Stale(_) | Stand::Missing) {
                    return true;
                }
                up.push(j);
            }
        }
        false
    };
    for i in named {
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        if asset.pin.is_some() && asset.hollow {
            said(&name, format!("{name} is a constant, one file: nothing is made of it"));
            continue;
        }
        // Brought up to date: what is on its way is joined, and what has its state stands, unless something it reads is re-run.
        if missing && !(one && asked.pickup.is_some()) && !(asset.hollow && stand[i].files.is_none()) {
            if judged(i) {
                said(&name, format!("{name} is approved and out of date: it renders again"));
                continue;
            }
            if being_made(i) {
                said(&name, format!("{name} is rendering: joined"));
                continue;
            }
            if stand[i].files.is_some() && !above(i).into_iter().any(judged) && !behind(i) {
                said(&name, match &stand[i].state {
                    Stand::Pinned { version: Some(version), changed } if !changed.is_empty() => {
                        format!("{name} take {version} is approved, though {} changed (--outdated renders it again)", changed.join(", "))
                    }
                    Stand::Pinned { version: Some(version), .. } => format!("{name} take {version} is approved (--new renders it again)"),
                    Stand::Current(version) => format!("{name} take {version} is current (--new renders it again)"),
                    _ => format!("{name} is current (--new renders it again)"),
                });
                continue;
            }
            // A cut is cut once; what it and the rest read with no state renders first.
            let count = if doc.tool(i).is_some() { 1 } else { n };
            let take = |_| Take { key: new_key(), asset: name.clone(), target: target.clone(), approve, new: count > 1, pickup: None, covers: true };
            takes.extend((0..count).map(take));
            continue;
        }
        // A cut that stands on what it reads now would only be cut the same again: answered now, with nothing queued (cl-910).
        if doc.tool(i) == Some(Tool::Cut) && !reads_asked(i)
            && let Some(version) = stand[i].inputs.as_deref().and_then(|inputs| same_cut(&project.folder, &lock, asset, &stand[i].reads, inputs))
        {
            said(&name, format!("{name} is current, version {}: what it cuts hasn't changed, so cutting it again makes the same film", version.number));
            continue;
        }
        if doc.tool(i).is_some() {
            said(&name, format!("{name} is made here, free, when what it reads settles"));
            // Named, it is made again, as anything named is.
            takes.push(Take { key: new_key(), asset: name.clone(), target: target.clone(), approve, new: true, pickup: None, covers: false });
            continue;
        }
        if asset.hollow {
            say(doc.nothing_to_make(i));
            continue;
        }
        let pickup = asked.pickup.clone().filter(|_| one);
        let take = |_| Take { key: new_key(), asset: name.clone(), target: target.clone(), approve, new: true, pickup: pickup.clone(), covers: missing };
        takes.extend((0..n).map(take));
    }
    if let (Some(error), true, false) = (first, takes.is_empty(), all) {
        return Err(Refusal(error));
    }
    // The ids its asker made, one a take (cl-945): asked again with one it holds, that take isn't asked twice.
    let resent = !asked.takes.is_empty() && !takes.is_empty() && {
        if !one || asked.takes.len() != takes.len() {
            let said = format!("{} take ids for {} takes: ids are for one box named, one a take", asked.takes.len(), takes.len());
            return Err(Refusal(Error::new(Kind::Usage, "take_ids", said)));
        }
        if let Some(bad) = asked.takes.iter().find(|id| !is_key(id)) {
            return Err(Refusal(Error::new(Kind::Usage, "take_id", format!("{bad} is no take id: `project-` and 32 hex digits"))));
        }
        let held = takes_held(&lock);
        takes.iter_mut().zip(&asked.takes).for_each(|(take, id)| id.clone_into(&mut take.key));
        takes.retain(|take| !held.contains(take.key.as_str()));
        takes.is_empty()
    };
    // Hold admission through the commit and its board. The previous walk
    // publishes its end under this same lock before a new one can begin.
    let mut walking = project.walking.lock().expect("walking lock");
    let joined = *walking;
    let new_all = all && missing && !lock.walk.asks.iter().any(|ask| ask.all && ask.digest == digest);
    let new_outdated = match all {
        true => outdated && !lock.walk.asks.iter().any(|ask| ask.outdated && ask.digest == digest),
        false => !judges.is_empty(),
    };
    let added = if resent { 0 } else { takes.len() as u32 + u32::from(new_all) + u32::from(new_outdated) };
    let outdated = outdated && (all || !judges.is_empty());
    let ask = Ask { id: new_key(), digest: digest.clone(), target: target.clone(), all: all && missing, outdated, whole: all, judges, sent: BTreeSet::new(), takes };
    // One transaction: its snapshot, the ask, and its key, so a retry however late runs nothing twice. What the project has lost is set aside,
    // and what's asked again fails no more.
    let lost = std::cell::Cell::new(false);
    let begun = ((!joined || added > 0) && !resent).then(|| ask.id.clone());
    (project.folder.act(|| {
        project.folder.keep_snapshot(&digest, &snapshot)?;
        project.folder.update_walk(|walk| {
            if begun.is_some() {
                walk.walk.asks.push(ask.clone());
            }
        })?;
        project.folder.update_lock(|lock| {
            lost.set(lock.tombstone(|key| doc.find(key).is_some()));
            for take in &ask.takes {
                if let Some(held) = lock.assets.get_mut(&name_key(&take.asset)) {
                    held.failed = None;
                }
            }
        })?;
        // Its answer is kept with it, in its transaction (cl-885), its board as the transaction knows it: what's going, and each take it
        // asks, next. What it queued behind, and the board the walk makes of it, its answer after says.
        let mut board = project.jobs.board();
        let next: Vec<Job> = (ask.takes.iter().filter(|take| !board.jobs.iter().any(|job| job.id == take.key)))
            .map(|take| {
                let noun = doc.find(&take.asset).map_or("version", |i| noun(&doc, &lock, i));
                Job { id: take.key.clone(), asset: take.asset.clone(), noun: noun.into(), why: Some("next".into()), ..Job::default() }
            })
            .collect();
        board.jobs.extend(next);
        let path = project.folder.dir.clone();
        let next = vec!["cozy status".into()];
        let (skipped, said) = (skipped.clone(), lines.borrow().clone());
        keyed::answering(StatusCode::ACCEPTED, &Asked { id: id.clone(), ask: begun.clone(), path, joined, added, skipped, queued: vec![], said, next, board });
        Ok(())
    }))
    .map_err(Refusal)?;
    if lost.get() {
        purge::due(&project.id, now_ms() + purge::window_ms());
    }
    project.join(&project.folder.lock().map_err(Refusal)?);
    let queued = queued_now(&project, &ask.id);
    walk_under(&api, &project, &mut walking);
    sync_jobs(&project, numbered(&api, &project.id)).map_err(Refusal)?;
    let board = project.jobs.board();
    drop(walking);
    let said = lines.into_inner();
    Ok((StatusCode::ACCEPTED, Json(Asked { id, ask: begun, path: project.folder.dir.clone(), joined, added, skipped, queued, said, next: vec!["cozy status".into()], board })))
}

/// A run by its number, as its record says now.
fn numbered<'a>(api: &'a Api, project: &'a str) -> impl Fn(u64) -> Option<Run> + 'a {
    move |number| {
        let calcifer = &api.calcifer;
        calcifer.record(&calcifer.project_store(project).ok()?.run_by_number(number).ok()??).ok()
    }
}

/// What the project has to do, in the order it goes: what is running, then what each render has queued.
fn queue(project: &Project, walk: &folder::WalkFile) -> Vec<Queued> {
    let unjoined = {
        let walker = project.walker.lock().expect("walker lock");
        walk.walk.asks.iter().any(|ask| !walker.joined.contains(&ask.id))
    };
    if unjoined && let Ok(lock) = project.folder.lock() {
        project.join(&lock);
    }
    let runs: HashMap<&str, u64> = walk.pending.iter().map(|(key, pending)| (pending.job_of(key), pending.run)).collect();
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

/// What `ask` queued, each with the names ahead of it. By its ask: the walk, or a read of the queue, may have taken it in before its answer.
fn queued_now(project: &Project, ask: &str) -> Vec<QueuedNow> {
    let walker = project.walker.lock().expect("walker lock");
    let graph = &walker.graph;
    let open = graph.open_jobs();
    (open.iter().enumerate())
        .filter(|&(_, &id)| graph.jobs[id].ask == ask)
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
    Ok(Json(cancelled(&api, &project, &asked.names, &asked.takes, &|_| "canceled".to_owned())?))
}

/// `names`' queued work dropped, its rows saying `why`, and their runs canceled, or with `all` everything's.
fn cancelled(api: &Arc<Api>, project: &Arc<Project>, names: &[String], takes: &[String], why: &dyn Fn(&str) -> String) -> Result<Cancelled, Refusal> {
    let folder = &project.folder;
    let lock = folder.lock().map_err(Refusal)?;
    let all = matches!(names, [only] if name_key(only) == name_key(ALL));
    let keys: BTreeSet<String> = names.iter().map(|name| name_key(name)).collect();
    // A box's work, or one take of it by its id (cl-945).
    let wanted = |name: &str, take: &str| all || keys.contains(&name_key(name)) || takes.iter().any(|id| id == take);
    project.join(&lock);
    let (dropped, canceled) = stop_jobs(api, project, &lock, &wanted, all, why).map_err(Refusal)?;
    // A take that failed, named: dismissed (cl-954).
    if !takes.is_empty() && lock.assets.values().any(|held| held.ended.iter().any(|ended| takes.contains(&ended.id))) {
        folder.update_lock(|lock| lock.assets.values_mut().for_each(|held| held.ended.retain(|ended| !takes.contains(&ended.id)))).map_err(Refusal)?;
    }
    project.asked.notify_one();
    sync_jobs(project, numbered(api, &project.id)).map_err(Refusal)?;
    // `all` stops what its machines have ready ahead for it too; the walker drops what fewer leave unneeded from their lists.
    if all {
        api.machines.lists.set(&api.machines, &folder.dir.to_string_lossy(), HashMap::new());
    }
    Ok(Cancelled { dropped, canceled })
}

/// Why a job of asset `name`, deleted, made nothing.
fn deleted_why(name: &str) -> String {
    format!("{name} was deleted, so it isn't made")
}

/// Stops what `wanted` names of a project's jobs: what waits leaves, with a row saying `why`; what is being sent isn't sent; a run is canceled. None of it
/// is asked again, a restart too. Answers what left and the runs canceled.
fn stop_jobs(api: &Api, project: &Project, lock: &Lock, wanted: &dyn Fn(&str, &str) -> bool, all: bool, why: &dyn Fn(&str) -> String) -> Result<(Vec<String>, Vec<u64>), Error> {
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
        project.jobs.ended(Job { id: job.key.clone(), asset: job.asset.clone(), state: JobState::Canceled, why: Some(why(&job.asset)), ..Job::default() });
    }
    let owed: Vec<String> = dropped.iter().map(|job| name_key(&job.asset)).chain(unsent).collect();
    project.folder.update_walk(|walk| {
        for ask in &mut walk.walk.asks {
            ask.takes.retain(|take| !wanted(&take.asset, &take.key));
            ask.all &= !all;
            ask.outdated &= !all;
            ask.sent.extend(owed.iter().cloned());
        }
    })?;
    let store = api.calcifer.project_store(&project.id).map_err(|error|Error::new(Kind::Conflict,"project_journal",error.to_string()))?;
    let mut canceled = vec![];
    for pending in lock.pending.iter().filter(|(key, pending)| pending.run > 0 && wanted(&pending.asset, pending.job_of(key))).map(|(_, pending)| pending) {
        let Ok(Some(row)) = store.run_by_number(pending.run) else { continue };
        if row.state != crate::store::State::Settled {
            // Inside a delete's act, `cancel` tells the machine now and keeps the request once the act lets the writer go.
            api.calcifer.cancel(&row.id).map_err(|e| Error::new(Kind::Internal, "run_cancel", e.to_string()))?;
            canceled.push(pending.run);
        }
    }
    Ok((dropped.into_iter().map(|job| job.asset).collect(), canceled))
}

/// Box `name`'s runs not yet ended, each told to stop at its machine now: what deletes it waits for the project's writer, which a
/// run's own progress or another program may hold, and a held commit never keeps a render going.
fn stop_at_machines(api: &Arc<Api>, project: &Project, name: &str) {
    let (Ok(lock), Ok(store)) = (project.folder.lock(), api.calcifer.project_store(&project.id)) else { return };
    let key = name_key(name);
    for pending in lock.pending.values().filter(|pending| pending.run > 0 && name_key(&pending.asset) == key) {
        if let Ok(Some(row)) = store.run_by_number(pending.run) && row.state != crate::store::State::Settled {
            api.calcifer.stop_at_machine(&row.id);
        }
    }
}

/// `source` kept as a file of the project, in the lock.
async fn keep_file(folder: &Folder, source: &str) -> Result<folder::Kept, Refusal> {
    // A file of the project's by the start of its hash, as `status` shows it: what lifting a take out of its asset names.
    let whole = written_hash(source).and_then(|hex| match folder.lock().ok()?.matching(&hex).as_slice() {
        [file] => Some(file.sha256.clone()),
        _ => None,
    });
    let source = whole.as_deref().unwrap_or(source);
    let Some(digest) = held_name(source)? else {
        return folder.import(source).await.map_err(|why| import_failed(source, why));
    };
    match crate::runs::files::held_for(&folder.dir, &digest) {
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
    Refusal(Error::new(Kind::Validation, "import_failed", said).remedy("import a picture, video or sound"))
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

/// The element a kept file makes, in one transaction: one constant box more, under a name nothing has, with `at` its row, and what its drop pushed aside.
#[allow(clippy::too_many_arguments)]
fn element(project: &Project, id: &str, wanted: &str, at: Option<api::calcifer::project::Card>, description: Option<&str>, file: folder::Kept, exact: bool, moves: BTreeMap<String, api::calcifer::project::Card>, placed: &BTreeMap<String, u64>) -> Result<Imported, Refusal> {
    let folder = &project.folder;
    let made = folder.act(|| {
        let lock = folder.lock()?;
        let doc = Doc::load(folder)?;
        let name = format::free_name(&doc, &lock, wanted);
        if exact && name != wanted {
            return Err(format::name_taken(&doc, wanted));
        }
        // Its row with its box: a page that reads the name has somewhere to put it, and places nothing of its own.
        if let Some(card) = at {
            canvas::set(folder, id, BTreeMap::from([(name.clone(), Some(card))]))?;
        }
        let kept = canvas::pushed(folder, id, moves, placed)?;
        folder.edit(|written| {
            written.constant(&name, &file.file.sha256)?;
            if let Some(words) = description {
                written.set(&name, &Map::from_iter([("description".to_owned(), json!(words))]))?;
            }
            Ok(())
        })?;
        // Imported media starts at v1. Its file and numbered version are committed with the box.
        let number = std::cell::Cell::new(0);
        folder.update_lock(|lock| {
            let row = lock.files.entry(file.file.sha256.clone()).or_default();
            (row.sha256, row.path, row.here) = (file.file.sha256.clone(), file.file.path.clone(), true);
            number.set(lock.joined(&name, file.field, Files::from([(file.field.to_owned(), file.file.sha256.clone())])).0);
        })?;
        folder.edit(|written| {
            written.selected(&name, number.get(), &file.file.sha256)?;
            if api::calcifer::tool::MEDIA.contains(&file.field) {
                written.set(&name, &Map::from_iter([(format::NODE_KIND.to_owned(), json!(file.field))]))?;
            }
            Ok(())
        })?;
        history::take(folder, &name, number.get(), if file.field == "video" { "take" } else { "version" }, history::Took::Absent, history::Took::Listed);
        Ok((name, kept))
    });
    let (name, kept) = made.map_err(Refusal)?;
    Ok(Imported { name, sha256: file.file.sha256, field: file.field.to_owned(), path: file.file.path, already: file.already, kept })
}

/// What a project's files are measured as, once its ffprobe is here, from a task of its own: a read waits for neither.
fn measure_soon(api: &Arc<Api>, project: &Arc<Project>) {
    let (host, project) = (Host::new(api), project.clone());
    tokio::spawn(async move {
        let Ok(access) = vault::admit_root(&project.root).await else { return; };
        if project.require_view().is_err() { return; }
        if project.folder.lock().map_or(true, |lock| !lock.files.values().any(folder::File::unmeasured)) {
            return;
        }
        let permit = thumbs::FFMPEGS.acquire().await.expect("ffmpeg's turns never close");
        if let Err(error) = host.ffmpeg().await {
            return warn!("measuring a project's files: {}", error.message);
        }
        let dir = project.folder.dir.clone();
        let measuring = tokio::task::spawn_blocking(move || {
            // This task can outlive its importer and must drain before storage is replaced.
            let _access = access;
            let _permit = permit;
            let measured = project.folder.lock().and_then(|mut lock| project.folder.measure(&mut lock));
            // What was read before this finished said nothing of how long a clip is.
            crate::events::project_settled(&dir);
            measured
        });
        if let Ok(Err(error)) = measuring.await {
            warn!("measuring a project's files: {}", error.message);
        }
    });
}

/// `POST /v1/projects/{id}/imports`: `source` kept in the project as an element, in one call.
async fn import(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Imported>, Refusal> {
    let asked: Import = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    let folder = &project.folder;
    let wanted = element_name(asked.name.as_deref(), &asked.source);
    if asked.exact {
        // As it's written, a space an `_`: what a name can't be is a version's or `self`, which is said, not changed.
        if let Some(name) = asked.name.as_deref().and_then(format::plain_name) {
            format::new_name(&name).map_err(Refusal)?;
        }
        let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
        if format::free_name(&doc, &lock, &wanted) != wanted {
            return Err(Refusal(format::name_taken(&doc, &wanted)));
        }
    }
    let kept = keep_file(folder, &asked.source).await?;
    // Its answer is kept with it, in its transaction (cl-885).
    let answer = folder.act(|| {
        let answer = element(&project, &id, &wanted, asked.at, asked.description.as_deref(), kept, asked.exact, asked.moves, &asked.placed).map_err(|Refusal(why)| why)?;
        keyed::answering(StatusCode::OK, &answer);
        Ok(answer)
    }).map_err(Refusal)?;
    measure_soon(&api, &project);
    Ok(Json(answer))
}

/// `POST /v1/projects/{id}/approve`: the version's main file, by hash, as the asset's `approved`: its whole file set is its state until `unpin`.
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
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy status")));
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
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy status")));
            };
            sha256.clone()
        }
    };
    let field = lock.files.get(&chosen).map_or_else(|| asset.kind.clone(), |file| file.field().to_owned());
    let version = wanted.and_then(|number| versions.iter().copied().find(|version| version.number == number))
        .or_else(|| versions.iter().copied().rev().find(|version| version.main() == Some(&chosen)));
    let before = stale_names(&doc, &doc.stand(&lock));
    let name = asset.name.clone();
    let files = version.map_or_else(|| Files::from([(field.clone(), chosen.clone())]), |version| version.files.clone());
    let (number, path) = (version.map_or(0, |version| version.number), main_path(&lock, &files).unwrap_or_default());
    let files = listed(&lock, &files);
    // Its answer is kept with it, in its transaction (cl-885): what it approved. What that put out of date, and what settling found, its
    // answer after says.
    let approved = folder.act(|| {
        folder.edit(|written| match version {
            Some(version) => written.selected(&name, version.number, &chosen),
            None => written.approved(&name, &chosen),
        })?;
        let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag))?;
        let approved = Approved { name: name.clone(), noun: noun.into(), number, files, path, stale: vec![], warnings: vec![], etag };
        keyed::answering(StatusCode::OK, &approved);
        Ok(approved)
    }).map_err(Refusal)?;
    let warnings = settle(&api, &project).await;
    let stale = newly_stale(folder, &before).map_err(Refusal)?;
    let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag)).map_err(Refusal)?;
    Ok(Json(Approved { stale, warnings, etag, ..approved }))
}

/// `POST /v1/projects/{id}/unpin`: `approved` gone from its box, so the asset's state follows its inputs again. A constant,
/// nothing but its file, keeps it: it is all it is.
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
    if asset.pin.is_some() && asset.hollow && lock.asset(&name).is_none_or(|held| held.versions.is_empty()) {
        let said = format!("{name} is a constant, nothing but its file: to be rid of it, delete it");
        return Err(Refusal(Error::new(Kind::Usage, "project_edit", said).next("cozy open")));
    }
    // Its answer is kept with it, in its transaction (cl-885), as approve's is.
    folder.act(|| {
        if asset.pin.is_some() {
            folder.edit(|written| written.unapproved(&name))?;
        }
        let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag))?;
        keyed::answering(StatusCode::OK, &Unpinned { name: name.clone(), version, stale: vec![], warnings: vec![], etag });
        Ok(())
    }).map_err(Refusal)?;
    let warnings = settle(&api, &project).await;
    let stale = newly_stale(folder, &before).map_err(Refusal)?;
    let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag)).map_err(Refusal)?;
    Ok(Json(Unpinned { name, version, stale, warnings, etag }))
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

/// One of the project's files by its name in files/ (`<sha256>.<ext>`): content-addressed, so its sha256 is its ETag.
async fn file(
    State(api): State<Arc<Api>>,
    Path((id, file)): Path<(String, String)>,
    Query(saved): Query<crate::files::Download>,
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
    if let Some(name) = saved.download {
        crate::files::save_as(&mut response, &name, ext);
    }
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
        Refusal(Error::new(Kind::NotFound, "file_missing", said).next("cozy open"))
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
    /// The films at the root it couldn't replace, which it has said.
    stays: Mutex<BTreeSet<String>>,
    /// Runs being sent, by key: recorded as owed before they go, and nothing to follow until they have gone.
    on_the_way: Mutex<HashSet<String>>,
    /// What it did not make, for its last word.
    missed: Mutex<Missed>,
    /// How many runs failed each way on each machine: (machine, code, message).
    faults: Mutex<HashMap<(String, String, String), u32>>,
    /// The machines it stopped sending to (`fault`), with the renders that stops: one asked later tries again.
    stopped: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// The renders whose machines were all gone, which it said go where they finish soonest (cl-920).
    fell_back: Mutex<HashSet<String>>,
    /// How many times each job was placed again after its machine went as it was sent (cl-920).
    replaced: Mutex<HashMap<String, u32>>,
    /// That this computer's first need of ffprobe was met: it measures what a function reads of a file.
    measuring: AtomicBool,
    /// The ranges of videos cut on this computer, by hash, where each is: sent from there, never kept in the project.
    clips: Mutex<HashMap<String, PathBuf>>,
    /// The asks it finished, which its end names.
    ended: Mutex<Vec<String>>,
}

/// How often what's landing tries a project that won't take it.
const HELD_UP: std::time::Duration = std::time::Duration::from_secs(1);
/// What a send refused for a machine that went meanwhile says (`Machines::reach`): the job is placed again (cl-920), this many times.
pub(crate) const GONE: [&str; 3] = ["machine_ended", "machine_offline", "machine_ending"];
const PLACED_AGAIN: u32 = 3;
/// How long a job's package may take to say what it calls, and its models' cards what they weigh, before its machines are asked without.
const QUOTE_LATE: std::time::Duration = std::time::Duration::from_secs(3);

/// Why the plan placed no machine for ready job `key`, said when that changes: it goes to the first of its machines that takes it.
fn unplanned(walker: &mut Walker, dir: &str, key: &str, why: String) {
    if walker.unplanned.get(key) != Some(&why) {
        warn!("{dir}: {key} isn't planned: {why}; it goes to the first of its machines that takes it");
        walker.unplanned.insert(key.to_owned(), why);
    }
}

/// Something a walker started came to its end.
enum Done {
    Sent { id: usize, key: String, ask: Arc<Ask>, name: String, kind: String, inputs: Option<String>, result: Result<(), Error> },
    Made { id: usize, ask: String, name: String, kind: String, inputs: Option<String>, result: Result<(), Error> },
    Landed { job: String, landed: Followed },
    /// All that was on its way here from a run came, or won't.
    Arrived { name: Option<String>, result: Result<(), Error> },
    Ahead,
}

/// What a queued job's machine can fetch before its turn: a function as its text names it, on the machine the plan placed it on.
#[derive(Clone)]
struct Wanted {
    machine: On,
    hub: String,
    with: String,
    kind: String,
    made: Map<String, Value>,
}


/// A run being sent, until it has gone or hasn't.
struct OnTheWay<'a>(&'a Mutex<HashSet<String>>, String);

impl<'a> OnTheWay<'a> {
    fn new(held: &'a Mutex<HashSet<String>>, key: &str) -> Self {
        held.lock().expect("on the way lock").insert(key.to_owned());
        Self(held, key.to_owned())
    }
}

impl Drop for OnTheWay<'_> {
    fn drop(&mut self) {
        self.0.lock().expect("on the way lock").remove(&self.1);
    }
}

/// What a job's turn came to, done once the graph is let go.
enum Act {
    Stands { id: usize, ask: String, key: String, asset: String, noun: &'static str, number: Option<u32> },
    Unread { id: usize, ask: String, on: Vec<On>, key: String, asset: String, kind: String, inputs: Option<String>, message: String },
    Kept { id: usize, ask: String, key: String, asset: String, said: String },
    Send { id: usize, key: String, ask: Arc<Ask>, doc: Arc<Doc>, stand: Arc<Vec<Standing>>, lock: Arc<Lock>, next: Next },
    /// Its machines are all busy: it waits here for one.
    Held { id: usize },
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

/// How often a walk with no network looks again.
const OFFLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// A run followed to its end: its key, the asset, inputs and number it was a try at if it was still owed, and what landing it came to.
struct Followed {
    key: String,
    name: Option<String>,
    inputs: Option<String>,
    run: Option<u64>,
    /// When it ended, what is on its way here from it.
    landed: Result<Result<Option<(RunId, Option<Coming>)>, Error>, Error>,
}

impl Walk {
    fn new(api: &Arc<Api>, project: &Arc<Project>) -> Self {
        Self {
            host: Host::for_project(api, &project.id),
            project: project.clone(),
            began: Instant::now(),
            tally: Mutex::default(),
            cut: Mutex::default(),
            stays: Mutex::default(),
            on_the_way: Mutex::default(),
            missed: Mutex::default(),
            faults: Mutex::default(),
            stopped: Mutex::default(),
            fell_back: Mutex::default(),
            replaced: Mutex::default(),
            measuring: AtomicBool::new(false),
            clips: Mutex::new(HashMap::new()),
            ended: Mutex::default(),
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
            true if missed.outdated => "all missing and outdated assets".to_owned(),
            true => "all missing assets".to_owned(),
            false => missed.names.iter().cloned().collect::<Vec<_>>().join(" "),
        };
        let on = missed.on.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let mut error = Error::new(first.kind, "render_incomplete", said).cause(first.code)
            .remedy(format!("retry rendering {what} on {on} in the browser opened by cozy open")).next("cozy open");
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
            self.say(Told::Warning { message: format!("{machine}: {seen} runs failed the same way, so this render sends it nothing more: {why}"), name: None });
        }
        for pending in lock.pending.values().filter(|pending| pending.run > 0 && asks.contains(&pending.ask)) {
            self.host.cancel_unstarted(pending.run, machine);
        }
        Ok(())
    }

    /// Which of `on` render `ask` may still send to.
    fn sending(&self, ask: &str, on: &[On]) -> Vec<On> {
        let on = self.live(ask, on);
        let on = on.as_slice();
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

    /// Of `on`, the machines that take work now (cl-920, Paul 11-12: "if those machines are ended then either try a different machine or
    /// tell me to rent a new one"): with every one gone (ended, ending, offline), where a render naming none goes, said once a render.
    fn live(&self, ask: &str, on: &[On]) -> Vec<On> {
        let api = self.host.api();
        let gone: Vec<String> = (on.iter())
            .filter_map(|on| match on {
                On::Machine(name) => api.machines.gone(name),
                _ => None,
            })
            .collect();
        if gone.is_empty() || gone.len() < on.len() {
            return on.iter().filter(|on| !matches!(on, On::Machine(name) if api.machines.gone(name).is_some())).cloned().collect();
        }
        // The set a render naming none takes, under this project's storage policy. `any` alone is a rental, which never comes uninvited.
        let automatic = crate::private_io::check_project_placement(&self.project.id, &self.project.root, &api.machines.automatic()).unwrap_or_default();
        if self.fell_back.lock().expect("fell back lock").insert(ask.to_owned()) {
            let set = automatic.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            self.say(Told::Warning { message: format!("{}: it goes where it finishes soonest of {set}", gone.join("; ")), name: None });
        }
        automatic
    }

    /// Sizes a function reads are ffprobe's: with files to measure and none here, this computer's first need fetches it behind the walk, which
    /// waits for no download here, or says what goes without.
    fn measuring(&self, lock: &Lock) {
        if !self.folder().wants_ffprobe(lock) || self.measuring.swap(true, Ordering::Relaxed) {
            return;
        }
        self.say(Told::Said { message: "fetching ffmpeg and ffprobe, which measure this project's files".into(), name: None });
        let (api, project) = (self.host.api(), self.project.clone());
        tokio::spawn(async move {
            match Host::new(&api).ffmpeg().await {
                Ok(_) => project.asked.notify_one(),
                Err(error) => project.jobs.told(Told::Warning { message: format!("functions see no sizes for this project's files: {}: cozy doctor", error.message), name: None }),
            }
        });
    }

    /// What it landed and how long it took, if it landed anything.
    fn rendered(&self) -> Option<Rendered> {
        let tally = self.tally.lock().expect("tally lock").clone();
        (tally.takes + tally.versions > 0).then(|| Rendered { elapsed_ms: self.began.elapsed().as_millis() as u64, ..tally })
    }

    fn folder(&self) -> &Folder {
        &self.project.folder
    }

    /// What has landed, measured: a cut and a function read those sizes, and a film is judged by its frames.
    fn measure(&self) -> Result<(), Error> {
        self.folder().measure(&mut self.folder().lock()?)
    }

    /// Measure the files the walk made here, or that landed before ffprobe was available.
    fn measure_at_end(&self) {
        if let Err(error) = self.measure() {
            warn!("measuring a project's files: {}", error.message);
        }
    }

    /// Publish and keep the old board before another request can admit a new walk.
    fn end(&self, walking: &mut bool, error: Option<Error>, following: &tokio::task::JoinHandle<()>) {
        following.abort();
        let asks = std::mem::take(&mut *self.ended.lock().expect("ended lock"));
        let ended = api::calcifer::project::Ended { error, rendered: self.rendered(), asks };
        let board = self.project.jobs.end(self.began.elapsed().as_millis() as u64, ended);
        if let Err(error) = self.folder().keep_board(&board) {
            warn!("keeping a render's board: {}", error.message);
        }
        *walking = false;
    }

    /// A render has nothing more that can go: each cut it ends on is said, and it is done.
    async fn finish(&self, id: &str) -> Result<(), Error> {
        {
            let walker = self.project.walker.lock().expect("walker lock");
            let open: Vec<&str> = walker.graph.open_jobs().into_iter().map(|job| &walker.graph.jobs[job]).filter(|job| job.ask == id).map(|job| job.asset.as_str()).collect();
            if !open.is_empty() {
                warn!("a render finishes with {open:?} still open");
            }
        }
        let asks = self.folder().walk_now()?.walk.asks;
        if let Some(ask) = asks.iter().find(|ask| ask.id == id)
            && let Ok(doc) = self.folder().asked(ask).and_then(|written| Doc::asked(self.folder(), &written))
            && (0..doc.assets.len()).any(|i| doc.tool(i) == Some(Tool::Cut))
        {
            self.measure()?;
            let lock = self.folder().lock()?;
            let stand = doc.stand(&lock);
            self.say_cuts(&doc, &stand).await?;
        }
        // Under `walking`, as an ask keeps its words and joins: one asked now of the same words keeps them.
        let _walking = self.project.walking.lock().expect("walking lock");
        let asks = self.folder().walk_now()?.walk.asks;
        if let Some(digest) = asks.iter().find(|ask| ask.id == id).map(|ask| &ask.digest) {
            if !asks.iter().any(|ask| ask.id != id && &ask.digest == digest) {
                self.project.walker.lock().expect("walker lock").readings.remove(digest);
                self.folder().drop_snapshot(digest);
            }
        }
        self.folder().update_walk(|walk| {
            walk.walk.asks.retain(|ask| ask.id != id);
            if walk.walk.asks.is_empty() {
                walk.walk.asked.clear();
            }
        })?;
        let mut ended = self.ended.lock().expect("ended lock");
        if !ended.iter().any(|done| done == id) {
            ended.push(id.to_owned());
        }
        Ok(())
    }

    /// `run` sent under `key`. A machine that answers no connect has recorded nothing, so it is asked
    /// again under that key; one its Hub says stopped or ended is refused at once.
    async fn send_run(&self, key: &str, mut run: SubmitRun) -> Result<Run, Error> {
        run.project = Some(self.project.id.clone());
        run.out = Some(landing(&self.project.root));
        self.again(|| self.host.submit(key, run.clone())).await
    }

    /// `ask`, and again while its machine answers no connect or the Hub doesn't answer, each attempt said. Everything asked this way
    /// is a read, or goes under a key.
    async fn again<T, F: Future<Output = Result<T, Error>>>(&self, ask: impl Fn() -> F) -> Result<T, Error> {
        let (mut backoff, mut said) = (tensorfs::fetch::Backoff::call(), false);
        loop {
            let e = match ask().await {
                Err(e) if e.transient() => e,
                answer => return answer,
            };
            if let Some((wait, message)) = backoff.again(&e.message, false, e.retry_after_s.map(std::time::Duration::from_secs)) {
                self.say(Told::Warning { message, name: None });
                tokio::time::sleep(wait).await;
                continue;
            }
            // This computer with no network tells nothing of its machine, and its job is never failed for it: it goes when the network does.
            if !(self.elsewhere() && !self.host.online().await) {
                return Err(Error { message: backoff.spent(&e.message), ..e });
            }
            if !std::mem::replace(&mut said, true) {
                self.say(Told::Warning { message: format!("{}; this computer reaches nothing: it goes on once it does", e.message), name: None });
            }
            tokio::time::sleep(OFFLINE).await;
        }
    }

    fn say(&self, told: Told) {
        self.project.jobs.told(told);
    }

    /// The board as the lock stands now, with the times last quoted.
    fn sync(&self) {
        if let Err(error) = sync_jobs(&self.project, |number| self.host.numbered(number)) {
            warn!("the board of {}: {}", self.folder().dir.display(), error.message);
        }
        // What's queued is in neither file, so no write says a job left it.
        if std::mem::take(&mut self.project.walker.lock().expect("walker lock").graph.ended) {
            crate::events::project_settled(&self.folder().dir);
        }
    }

    /// `name` stands as it is: no new version comes of its render.
    fn stands(&self, name: &str, noun: &str, number: u32, why: String) {
        let path = self.folder().lock().ok().and_then(|lock| main_path(&lock, &lock.asset(name)?.version(number)?.files));
        self.project.jobs.ended(Job { asset: name.to_owned(), state: JobState::Done, noun: noun.to_owned(), number: Some(number), why: Some(why), path, ..Job::default() });
    }

    /// Gives each ready job its turn as it comes up, each sending, making or following on its own so none holds up another, and goes on
    /// as each ends or something is asked, until nothing is owed and its terminal board is kept.
    async fn go(&self, following: &tokio::task::JoinHandle<()>) -> Result<(), Error> {
        let (mut first, mut followed) = (None, HashSet::new());
        let mut work: FuturesUnordered<BoxFuture<'_, Done>> = FuturesUnordered::new();
        // A project's revision moved (this Calcifer's commit, or another program's its folder watch saw): what was deleted stops.
        let mut edits = crate::events::edits();
        // A machine's Status moved (a run started or ended, bytes came, a machine came or went): a job held for a machine is placed again.
        let mut machines = self.host.api().calcifer.machines_changed();
        // A machine became ready or ended: what isn't sent yet is placed again.
        let mut arrivals = self.host.api().machines.arrivals.subscribe();
        // A walk before this one's versions whose files are on their way: it isn't over until they're here, or never come.
        let walk = self.folder().walk_now()?;
        for (number, coming) in walk.walk.runs {
            if walk.pending.values().any(|pending| pending.run == number) {
                continue;
            }
            let Some(run) = self.host.numbered(number) else { continue };
            let name = coming.as_ref().map(|coming| coming.name.clone());
            work.push(async move { Done::Arrived { name, result: self.arrived_owed(&run.id, coming).await } }.boxed());
        }
        loop {
            self.deletions()?;
            self.joined(&mut first).await?;
            while self.turns(&mut work, &mut first).await? {}
            for (key, pending) in self.folder().walk_now()?.pending {
                if !self.on_the_way.lock().expect("on the way lock").contains(&key) && followed.insert(key.clone()) {
                    let job = pending.job_of(&key).to_owned();
                    work.push(async move { Done::Landed { job, landed: self.follow(key).await } }.boxed());
                }
            }
            // What waits for another's output is placed too, so its machine fetches ahead for it.
            if !self.project.walker.lock().expect("walker lock").graph.has_ready() {
                self.plan(false).await;
            }
            // What the jobs not sent will run with goes to the machines they're planned on, as each one's upcoming list (cl-967).
            let ahead = self.ahead_jobs();
            for (key, _, wanted) in &ahead {
                let On::Machine(name) = &wanted.machine else { continue };
                let at = (name.clone(), key.clone());
                {
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    if walker.items.contains_key(&at) || !walker.making.insert(at.clone()) {
                        continue;
                    }
                }
                let (key, wanted) = (key.clone(), wanted.clone());
                work.push(async move {
                    let item = match self.item(&key, &wanted).await {
                        Ok(item) => item,
                        Err(error) => {
                            warn!("{}: what {key} needs ahead on {}: {}", self.folder().dir.display(), wanted.machine, error.message);
                            None
                        }
                    };
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    walker.making.remove(&at);
                    walker.items.insert(at, item.map(|(item, function)| (wanted.hub.clone(), item, function)));
                    Done::Ahead
                }.boxed());
            }
            self.list_ahead(&ahead);
            self.finish_asks().await?;
            self.sync();
            if work.is_empty() {
                // What is going with nothing following it has ended. What this walker took in is done; an ask that came since is
                // the next turn's, so `over` finds it and the walk goes on.
                let done = {
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    let going: Vec<usize> = (0..walker.graph.jobs.len()).filter(|&id| walker.graph.jobs[id].state == walker::State::Going).collect();
                    going.into_iter().for_each(|id| {
                        warn!("{}: it was going with nothing following it", walker.graph.jobs[id].asset);
                        walker.graph.end(id, End::Done);
                    });
                    if walker.graph.has_ready() {
                        continue;
                    }
                    for id in walker.graph.open_jobs() {
                        warn!("{}: nothing is going, and it still waits for {:?}", walker.graph.jobs[id].asset, walker.graph.waits_for(id));
                    }
                    walker.joined.clone()
                };
                for ask in self.folder().walk_now()?.walk.asks.into_iter().filter(|ask| done.contains(&ask.id)) {
                    self.finish(&ask.id).await?;
                }
                if self.over(&mut first, following)? {
                    return Ok(());
                }
                continue;
            }
            loop {
                tokio::select! {
                    Some(done) = work.next() => {
                        self.done(done, &mut work, &mut followed, &mut first)?;
                        break;
                    }
                    () = self.project.asked.notified() => break,
                    Ok(()) = machines.changed() => {
                        if self.project.walker.lock().expect("walker lock").held {
                            break;
                        }
                    }
                    Ok(()) = arrivals.changed() => break,
                    Ok(()) = edits.changed() => {
                        if self.deletions()? {
                            break;
                        }
                    }
                }
            }
            while let Some(Some(done)) = work.next().now_or_never() {
                self.done(done, &mut work, &mut followed, &mut first)?;
            }
        }
    }

    /// Each job not sent, with the machine it will most likely go to: its one, else where the last plan placed it; by its key and box.
    fn ahead_jobs(&self) -> Vec<(String, String, Wanted)> {
        let Ok(walk) = self.folder().walk_now() else { return vec![] };
        let walker = self.project.walker.lock().expect("walker lock");
        let Walker { graph, readings, placed, foreseen, .. } = &*walker;
        let mut jobs = vec![];
        for id in graph.open_jobs() {
            let job = &graph.jobs[id];
            if job.node == usize::MAX || job.state != walker::State::Waiting || matches!(job.kind, JobKind::Free { .. }) {
                continue;
            }
            let Some(ask) = walk.walk.asks.iter().find(|ask| ask.id == job.ask) else { continue };
            let Some(reading) = readings.get(&ask.digest) else { continue };
            let target = ask.takes.iter().find(|take| take.key == job.key).map_or(&ask.target, |take| &take.target);
            let asset = &reading.doc.assets[job.node];
            let machine = match target.on.as_slice() {
                [only] => Some(only.clone()),
                _ => placed.get(&job.key).map(|placed| placed.soonest.machine.clone()).or_else(|| foreseen.get(&job.key).cloned().flatten()),
            };
            // With `any`, or none placed yet, where isn't known.
            let Some(machine @ On::Machine(_)) = machine else { continue };
            let wanted = Wanted { machine, hub: target.hub.clone(), with: reading.doc.target(job.node), kind: asset.kind.clone(), made: asset.made.clone() };
            jobs.push((job.key.clone(), job.asset.clone(), wanted));
        }
        jobs
    }

    /// Each machine's upcoming jobs of this walk, as far as their items are made, in the order they start (`ahead::Lists`); their rows
    /// on the board, and only theirs.
    fn list_ahead(&self, jobs: &[(String, String, Wanted)]) {
        let api = self.host.api();
        let mut by_machine: HashMap<String, Vec<crate::ahead::Listed>> = HashMap::new();
        {
            let walker = self.project.walker.lock().expect("walker lock");
            for (key, asset, wanted) in jobs {
                let On::Machine(name) = &wanted.machine else { continue };
                let Some(Some((hub, item, function))) = walker.items.get(&(name.clone(), key.clone())) else { continue };
                let (start_at_ms, fits) = walker.starts.get(key).cloned().unwrap_or((i64::MAX, vec![]));
                let listed = crate::ahead::Listed {
                    project: self.project.clone(),
                    asset: asset.clone(),
                    start_at_ms,
                    hub: hub.clone(),
                    item: item.clone(),
                    function: function.clone(),
                    fits,
                };
                by_machine.entry(name.clone()).or_default().push(listed);
            }
        }
        let keys: HashSet<String> = by_machine.values().flatten().map(|listed| listed.item.key.clone()).collect();
        api.machines.lists.set(&api.machines, &self.folder().dir.to_string_lossy(), by_machine);
        self.project.jobs.upcoming_kept(&keys);
    }

    /// What `wanted`'s machine has ready for job `key` (`PUT /v1/upcoming`): the function its text names, else the first of its package
    /// that makes this kind, with its model choices; and the function, as its row says it. None: its package makes nothing of the kind.
    /// Local code is installed there first, as a call's is.
    async fn item(&self, key: &str, wanted: &Wanted) -> Result<Option<(v1::Ahead, String)>, Error> {
        // A machine-wide list is no project's: what a project's job will read comes with its own run, in its own storage.
        if crate::private_io::of_path(&self.folder().dir)?.is_some() {
            return Ok(None);
        }
        let Wanted { machine, hub, with, kind, made } = wanted;
        let package = self.host.package(hub, with, &self.folder().dir, std::slice::from_ref(machine)).await?;
        let Some(renderer) = package.first_making(kind) else { return Ok(None) };
        let mut payload = Map::new();
        for field in ["models", "lora"] {
            if let Some(value) = made.get(field).filter(|value| !value.is_null()) {
                payload.insert(field.into(), value.clone());
            }
        }
        let called = renderer.target();
        let mut choices = Choices::new(&called, &renderer.function, &renderer.slots, renderer.call_slots());
        choices.take(&mut payload)?;
        let models = choices::wire(&choices.into_choices())?;
        let source = match &renderer.local {
            None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() }),
            Some(path) => {
                // A private project's code goes with the run that calls it.
                if crate::private_io::of_path(&self.folder().dir)?.is_some() {
                    return Ok(None);
                }
                v1::Source::Local(self.host.ready(path, machine, hub).await?.source)
            }
        };
        let item = v1::Ahead { key: key.to_owned(), source: Some(source), entrypoint: renderer.function.clone(), models, ..Default::default() };
        Ok(Some((item, package.name.clone() + "/" + &renderer.function)))
    }

    /// Job `key` as its machines quote it: the function its tool names, with `made`'s choices; else why not. `late`: how long its package
    /// may take to say what it calls, and its models' cards what they weigh (none: as long as they take).
    async fn quotable(&self, key: String, on: Vec<On>, hub: &str, with: &str, made: Map<String, Value>, late: Option<std::time::Duration>) -> Result<placement::Job, String> {
        let api = self.host.api();
        let limit = late.unwrap_or(std::time::Duration::MAX);
        let package = match tokio::time::timeout(limit, self.host.package(hub, with, &self.folder().dir, &on)).await {
            Err(_) => return Err(format!("{with} didn't say within {} s what it calls", limit.as_secs())),
            Ok(Err(e)) => return Err(format!("{with}: {}", e.message)),
            Ok(Ok(package)) => package,
        };
        let renderer = match package.maker() {
            Ok(Some(renderer)) => renderer,
            Ok(None) => return Err(format!("{with} names no function")),
            Err(e) => return Err(format!("{with}: {}", e.message)),
        };
        let models = tokio::time::timeout(limit, fit::checkpoints(&api, hub, &package, &renderer.function, &made)).await.unwrap_or_default();
        let size = v1::JobSize::of(&Value::Object(made));
        let gpu = api::interface::Interface::parse(package.document()).ok().is_some_and(|interface| {
            interface.callables.iter().find(|callable| callable.name == renderer.function).is_some_and(|callable| interface.needs_gpu(callable))
        });
        Ok(placement::Job { key, target: renderer.target(), function: package.estimated(&renderer.function), models, size, gpus: 0, gpu, on })
    }

    /// Places the jobs not sent yet, the nearest `plan::PLANNED` of them (cl-967): every machine pictured from the Status it pushed, each
    /// job beginning once what it reads ends, ahead of it on each machine the runs sent there that it hasn't accepted yet. A ready job's
    /// place is where it goes at its turn; one waiting for another's output, where its weights are fetched ahead. A ready job waits for its
    /// package to say what it calls (its send would anyway); one waiting, `QUOTE_LATE` at most. Each ready job's line says where it goes
    /// and why, or why no machine was planned: then it goes to the first of its machines that takes it. `update`: as `placement::pictures`.
    async fn plan(&self, update: bool) {
        let Ok(walk) = self.folder().walk_now() else { return };
        let dir = self.folder().dir.display().to_string();
        let (unsent, ends, foreseen) = {
            let walker = self.project.walker.lock().expect("walker lock");
            let graph = &walker.graph;
            let ready: HashSet<usize> = graph.ready(usize::MAX).into_iter().collect();
            let unsent: Vec<Unsent> = (graph.open_jobs().into_iter())
                .filter(|&id| graph.jobs[id].state == walker::State::Waiting)
                .filter_map(|id| {
                    let job = &graph.jobs[id];
                    if job.node == usize::MAX || matches!(job.kind, JobKind::Free { .. }) {
                        return None;
                    }
                    let ask = walk.walk.asks.iter().find(|ask| ask.id == job.ask)?;
                    let target = ask.takes.iter().find(|take| take.key == job.key).map_or(&ask.target, |take| &take.target);
                    let reading = walker.readings.get(&ask.digest)?;
                    let asset = reading.doc.assets.get(job.node)?;
                    let (with, made) = (reading.doc.target(job.node), asset.made.clone());
                    Some(Unsent { id, key: job.key.clone(), ask: ask.id.clone(), target: target.clone(), with, made, needs: job.needs.clone(), ready: ready.contains(&id) })
                })
                .take(plan::PLANNED)
                .collect();
            // When the jobs they read that went already end, as their machines were quoted.
            let ends: HashMap<usize, i64> = (unsent.iter().flat_map(|unsent| unsent.needs.iter().copied()))
                .filter_map(|need| Some((need, walker.sent.get(&graph.jobs[need].key)?.finish_at_ms)))
                .collect();
            (unsent, ends, walker.foreseen.clone())
        };
        if unsent.is_empty() {
            return;
        }
        // What each asks of a machine, read once.
        let known: HashSet<String> = self.project.walker.lock().expect("walker lock").quotables.keys().cloned().collect();
        let read = unsent.iter().filter(|unsent| !known.contains(&unsent.key)).map(|unsent| async move {
            let late = (!unsent.ready).then_some(QUOTE_LATE);
            (unsent.key.clone(), self.quotable(unsent.key.clone(), vec![], &unsent.target.hub, &unsent.with, unsent.made.clone(), late).await)
        });
        let mut unread: HashMap<String, String> = HashMap::new();
        let quotables = {
            let read = futures_util::future::join_all(read).await;
            let mut walker = self.project.walker.lock().expect("walker lock");
            for (key, quotable) in read {
                match quotable {
                    Ok(job) => drop(walker.quotables.insert(key, job)),
                    Err(why) => drop(unread.insert(key, why)),
                }
            }
            let pending: HashSet<&str> = walk.pending.iter().map(|(key, pending)| pending.job_of(key)).collect();
            walker.quotables.retain(|key, _| pending.contains(key.as_str()) || unsent.iter().any(|unsent| unsent.key == *key));
            walker.quotables.clone()
        };
        let jobs: Vec<(usize, placement::Job)> = (unsent.iter().enumerate())
            .filter_map(|(at, unsent)| {
                let job = quotables.get(&unsent.key)?;
                Some((at, placement::Job { on: self.sending(&unsent.ask, &unsent.target.on), ..job.clone() }))
            })
            .collect();
        // Runs sent that their machines haven't accepted yet wait there ahead of these.
        let ahead: Vec<(On, placement::Job)> = (walk.pending.iter())
            .filter(|(_, pending)| pending.run > 0)
            .filter_map(|(key, pending)| {
                let run = self.host.numbered(pending.run)?;
                if run.accepted_at_ms.is_some() || run.settled_at_ms.is_some() {
                    return None;
                }
                Some((On::Machine(run.machine?), quotables.get(pending.job_of(key))?.clone()))
            })
            .collect();
        let api = self.host.api();
        let now = now_ms();
        let only: Vec<placement::Job> = jobs.iter().map(|(_, job)| job.clone()).collect();
        let mut machines = placement::machines_of(&only);
        for (on, _) in &ahead {
            if !machines.contains(on) {
                machines.push(on.clone());
            }
        }
        let mut pictures = vec![];
        for (on, pictured) in machines.iter().zip(placement::pictures(&api, &machines, update, now).await) {
            match pictured {
                Ok(picture) => pictures.push(picture),
                Err(why) => warn!("{dir}: {on} isn't planned on: {why}"),
            }
        }
        for (on, job) in &ahead {
            if let Some(m) = plan::index(&pictures, on) {
                let prior = placement::prior(&api, &pictures[m], job).await;
                pictures[m].ahead.push(placement::asked(job, prior));
            }
        }
        let position: HashMap<usize, usize> = jobs.iter().enumerate().map(|(at, (unsent_at, _))| (unsent[*unsent_at].id, at)).collect();
        let mut planned = vec![];
        for (at, job) in &jobs {
            let unsent = &unsent[*at];
            let machines: Vec<usize> = job.on.iter().filter_map(|on| plan::index(&pictures, on)).collect();
            let mut priors = vec![];
            for &m in &machines {
                priors.push(placement::prior(&api, &pictures[m], job).await);
            }
            planned.push(plan::Job {
                ask: placement::asked(job, 0),
                priors,
                machines,
                needs: unsent.needs.iter().filter_map(|need| position.get(need).copied()).collect(),
                after_at_ms: unsent.needs.iter().filter_map(|need| ends.get(need).copied()).max(),
                stay: foreseen.get(&unsent.key).cloned().flatten().and_then(|on| plan::index(&pictures, &on)),
            });
        }
        // What each job's function read of its models on any machine: what it fetches on one that holds none of them (cl-967).
        let frames: Vec<&api::machine::StatusFrame> = pictures.iter().map(|picture| &picture.frame).collect();
        for job in &mut planned {
            api::estimate::learn_reads(&frames, &mut job.ask);
        }
        let placed = plan::plan(&pictures, &planned, now);
        let mut walker = self.project.walker.lock().expect("walker lock");
        for ((at, _), placement) in jobs.iter().zip(placed) {
            let unsent = &unsent[*at];
            let Some((_, chosen)) = placement.chosen else {
                walker.placed.remove(&unsent.key);
                walker.foreseen.remove(&unsent.key);
                walker.starts.remove(&unsent.key);
                if unsent.ready {
                    let why = match placement.offers.is_empty() {
                        true => "none of its machines answered".to_owned(),
                        false => placement.offers.iter().map(|offer| placement::said(offer, now)).collect::<Vec<_>>().join("; "),
                    };
                    unplanned(&mut walker, &dir, &unsent.key, why);
                }
                continue;
            };
            let fits = placement.offers.iter().filter(|offer| offer.fits).map(|offer| offer.machine.to_string()).collect();
            walker.starts.insert(unsent.key.clone(), (chosen.start_at_ms, fits));
            if !unsent.ready {
                walker.foreseen.insert(unsent.key.clone(), Some(chosen.machine.clone()));
                continue;
            }
            // Where each job goes, said when that changes: what placement weighed, for anyone asking why a job went where it did.
            if walker.placed.get(&unsent.key).is_none_or(|was| was.soonest.machine != chosen.machine) {
                let quoted: Vec<String> = placement.offers.iter().map(|offer| placement::said(offer, now)).collect();
                info!("{dir}: {} goes to {}: {}", unsent.key, chosen.machine, quoted.join("; "));
            }
            walker.unplanned.remove(&unsent.key);
            walker.placed.insert(unsent.key.clone(), Placed { offers: placement.offers, soonest: chosen });
        }
        for unsent in unsent.iter().filter(|unsent| unsent.ready) {
            if let Some(why) = unread.remove(&unsent.key) {
                walker.placed.remove(&unsent.key);
                unplanned(&mut walker, &dir, &unsent.key, why);
            }
        }
        walker.placed.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.unplanned.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.foreseen.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.starts.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.items.retain(|(_, key), _| unsent.iter().any(|unsent| unsent.key == *key));
    }

    /// Of `on`, where job `job` goes, as the last plan placed it (`pick`).
    async fn chosen(&self, job: &str, on: &[On], hub: &str, package: &Package, renderer: &Renderer, asset: &format::Asset) -> Result<(On, Option<Offer>), Error> {
        let placed = self.project.walker.lock().expect("walker lock").placed.get(job).cloned();
        let planned = placed.as_ref().map(|placed| placed.soonest.clone()).filter(|offer| on.contains(&offer.machine));
        let offers: Vec<Offer> = placed.map(|placed| placed.offers).unwrap_or_default().into_iter().filter(|offer| on.contains(&offer.machine)).collect();
        pick(&self.host.api(), on, &offers, planned.as_ref(), hub, Some(package), &renderer.function, &asset.made).await
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
                self.say(Told::Warning { message: warning.message.clone(), name: warning.name.clone() });
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

    /// The asset names the project holds now, read again only when it changed. None: it doesn't read, so nothing is known gone.
    fn names_now(&self, held: &mut Option<(String, Arc<HashSet<String>>)>) -> Option<Arc<HashSet<String>>> {
        let stamp = self.folder().seq().to_string();
        if held.as_ref().is_none_or(|(was, _)| *was != stamp) {
            let names = self.folder().reading_boxes(written::Written::names).ok()?;
            *held = Some((stamp, Arc::new(names)));
        }
        held.as_ref().map(|(_, names)| names.clone())
    }

    /// Whether `name` is deleted from the file: a rename isn't, and what still says it is the new name's.
    fn gone(&self, name: &str) -> bool {
        let key = name_key(name);
        let names = {
            let mut walker = self.project.walker.lock().expect("walker lock");
            self.names_now(&mut walker.names)
        };
        names.is_some_and(|names| !names.contains(&key)) && !self.folder().walk_now().is_ok_and(|walk| walk.walk.renames.iter().any(|(from, _)| name_key(from) == key))
    }

    /// What the file no longer has stops: what waits isn't made, and a run is canceled, so billing stops, and lands nothing. What reads it says so at
    /// its turn. Whether anything left, so that those can have it. Each wake, as a job being sent has a run to cancel only once it's sent.
    fn deletions(&self) -> Result<bool, Error> {
        let open: Vec<String> = {
            let mut walker = self.project.walker.lock().expect("walker lock");
            let Walker { graph, names, .. } = &mut *walker;
            let Some(names) = self.names_now(names) else { return Ok(false) };
            graph.open_names().filter(|name| !names.contains(*name)).cloned().collect()
        };
        if open.is_empty() {
            return Ok(false);
        }
        let lock = self.folder().lock()?;
        let gone: HashSet<String> = open.into_iter().filter(|name| !lock.walk.renames.iter().any(|(from, _)| name_key(from) == *name)).collect();
        if gone.is_empty() {
            return Ok(false);
        }
        let (dropped, _) = stop_jobs(&self.host.api(), &self.project, &lock, &|name, _| gone.contains(&name_key(name)), false, &deleted_why)?;
        for name in &dropped {
            self.say(Told::Said { message: deleted_why(name), name: Some(name.clone()) });
        }
        Ok(!dropped.is_empty())
    }

    /// A job's asset was deleted, and it ended with nothing made: its row, and a line, say so.
    fn deleted(&self, job: &str, name: &str) {
        self.say(Told::Said { message: deleted_why(name), name: Some(name.to_owned()) });
        self.project.jobs.ended(Job { id: job.to_owned(), asset: name.to_owned(), state: JobState::Canceled, why: Some(deleted_why(name)), ..Job::default() });
    }

    /// The ready jobs, as many as there is room for, each given its turn; whether there were any.
    async fn turns<'a>(&'a self, work: &mut FuturesUnordered<BoxFuture<'a, Done>>, first: &mut Option<Error>) -> Result<bool, Error> {
        if !self.project.walker.lock().expect("walker lock").graph.has_ready() {
            return Ok(false);
        }
        self.plan(true).await;
        let held = self.folder().lock()?;
        self.measuring(&held);
        self.measure()?;
        let lock = Arc::new(self.folder().lock()?);
        let mut asks: HashMap<String, Arc<Ask>> = HashMap::new();
        let mut acts = vec![];
        {
            let mut walker = self.project.walker.lock().expect("walker lock");
            let Walker { graph, readings, names, placed, .. } = &mut *walker;
            let names = self.names_now(names);
            for id in graph.take_ready(usize::MAX) {
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
                    warn!("{asset}: its render {ask_id} is gone from the walk, so it isn't made");
                    graph.end(id, End::Dropped);
                    continue;
                };
                let Some(reading) = readings.get_mut(&ask.digest) else {
                    warn!("{asset}: its render's text is no longer held, so it isn't made");
                    graph.end(id, End::Dropped);
                    continue;
                };
                // Read with tags it didn't know: a package has been read since.
                if !reading.doc.knows_tags_of(&lock)
                    && let Ok(doc) = self.folder().asked(&ask).and_then(|written| Doc::asked(self.folder(), &written))
                {
                    *reading = AskedText::new(doc);
                    graph.read_again(|job_ask| lock.walk.asks.iter().any(|other| other.id == job_ask && other.digest == ask.digest), &reading.nodes);
                }
                let gone = |name: &str| names.as_ref().is_some_and(|names| !names.contains(&name_key(name)));
                // With several machines it stays here while its planned machine has no GPU for it, so nothing commits early to the wrong
                // queue; it is placed again as their Status moves.
                if ask.target.on.len() > 1 && !matches!(kind, JobKind::Free { .. }) && placed.get(&key).is_some_and(|placed| placed.soonest.waits_ms(now_ms()) > 0) {
                    acts.push(Act::Held { id });
                    continue;
                }
                let turn = graph.turn(id, &mut Now { reading, lock: &lock, ask: &ask });
                let noun = if reading.nodes[node].video { "take" } else { "version" };
                let row_of = |reading: &mut AskedText| {
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
                            // Asked for by name with nothing it reads ever made: it fails and says why, or the press does nothing at all.
                            JobKind::Free { named: true } if standing.inputs.is_none() => {
                                let message = format!("{asset} cannot render: {}", unread(&doc, &stand, &lock, &ask, node));
                                let kind = doc.assets[node].kind.clone();
                                acts.push(Act::Unread { id, ask: ask_id, on: ask.target.on.clone(), key, asset, kind, inputs: None, message });
                            }
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
                                if outdated_now(&doc, &stand, &ask, node, &lock) {
                                    let next = Next { i: node, key: key.clone(), take: None, unpin: true };
                                    acts.push(Act::Send { id, key, ask, doc, stand, lock: lock.clone(), next });
                                } else if pin_changed(&stand, node) {
                                    let why = doc.tags(node).and_then(|tags| doc.missing_inputs(node, tags, &lock)).unwrap_or_default();
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
        // Only what moved counts: a job held for a machine that isn't free yet waits for the next wake, not for another turn now, which
        // would come round again at once, forever, with nothing it sent ever followed.
        let moved = acts.iter().any(|act| !matches!(act, Act::Held { .. }));
        self.project.walker.lock().expect("walker lock").held = acts.iter().any(|act| matches!(act, Act::Held { .. }));
        // What goes to one machine goes together, as its quote packed it (cl-816).
        let mut together = HashMap::new();
        for act in acts {
            self.act(act, &mut together, work, first)?;
        }
        Ok(moved)
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

    fn act<'a>(&'a self, act: Act, together: &mut HashMap<String, Arc<host::Together>>, work: &mut FuturesUnordered<BoxFuture<'a, Done>>, first: &mut Option<Error>) -> Result<(), Error> {
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
                self.say(Told::Said { message: said, name: Some(asset.clone()) });
                self.settled(&ask, &asset, &key)?;
                self.project.jobs.ended(Job { id: key, asset, state: JobState::Done, noun: "version".into(), why: Some("stays pinned".into()), ..Job::default() });
                end(id, End::Stands);
            }
            Act::Unread { id, ask, on, key, asset, kind, inputs, message } => {
                self.settled(&ask, &asset, &key)?;
                match self.sending(&ask, &on).is_empty() {
                    true => self.not_sent(&asset),
                    false => self.failed_of(&asset, &kind, inputs, None, None, Error::new(Kind::NotFound, "file_missing", message), first),
                }
                end(id, End::Failed);
            }
            Act::Held { id } => self.project.walker.lock().expect("walker lock").graph.put_back(id),
            Act::Send { id, key, ask, doc, stand, lock, next } => {
                let (name, kind, inputs) = (doc.assets[next.i].name.clone(), doc.assets[next.i].kind.clone(), stand[next.i].inputs.clone());
                // Where its quote sent it, else its one machine.
                let target = next.take.as_ref().map_or(&ask.target, |take| &take.target);
                let goes = match (self.project.walker.lock().expect("walker lock").placed.get(&key), target.on.as_slice()) {
                    (Some(placed), _) => placed.soonest.machine.clone(),
                    (None, [only]) => only.clone(),
                    _ => On::Any,
                };
                let member = together.entry(goes.to_string()).or_insert_with(|| self.host.together(goes)).member();
                work.push(async move {
                    let result = self.send(&ask, &doc, &stand, &lock, next, member).await;
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
    fn done<'a>(&'a self, done: Done, work: &mut FuturesUnordered<BoxFuture<'a, Done>>, followed: &mut HashSet<String>, first: &mut Option<Error>) -> Result<(), Error> {
        let ended = |id: usize, how: End, name: &str| {
            let mut walker = self.project.walker.lock().expect("walker lock");
            walker.graph.end(id, how);
            walker.moved(name);
        };
        match done {
            Done::Ahead => {}
            Done::Sent { id, key, ask, name, kind, inputs, result } => match result {
                Ok(()) => {
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    if walker.again.remove(&key) {
                        walker.graph.put_back(id);
                        return Ok(());
                    }
                    drop(walker);
                    // No run came of it: it stands, it had no adapter, or it was canceled while being sent.
                    if !self.folder().walk_now()?.pending.iter().any(|(sent, pending)| pending.job_of(sent) == key) {
                        ended(id, End::Done, &name);
                        match self.gone(&name) {
                            true => self.deleted(&key, &name),
                            false => self.project.jobs.ended_if_open(&key, &name),
                        }
                    }
                }
                // Its machine went as it was sent (a pod releasing itself that second, cl-920): placed again, never failed.
                Err(error) if GONE.contains(&error.code.as_str()) && self.place_again(&key) => {
                    self.say(Told::Said { message: format!("{name}: {}; placing it again", error.message), name: Some(name.clone()) });
                    self.project.walker.lock().expect("walker lock").graph.put_back(id);
                }
                Err(error) => {
                    self.send_failed(&ask, &name, &error)?;
                    self.failed_of(&name, &kind, inputs, None, Some(&key), error, first);
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
                        self.failed_of(&name, &kind, inputs, None, Some(&job), error, first);
                        self.tried(&ask, &name)?;
                        ended(id, End::Failed, &name);
                    }
                }
            }
            Done::Arrived { name, result } => {
                if let Err(error) = result {
                    match &name {
                        Some(name) => {
                            self.project.walker.lock().expect("walker lock").moved(name);
                            self.failed_of(name, "", None, None, None, error, first);
                        }
                        None => self.failed(error, first),
                    }
                }
            }
            Done::Landed { job, landed } => {
                let failed = landed.landed.as_ref().is_ok_and(Result::is_err);
                let asset = landed.name.clone();
                self.project.walker.lock().expect("walker lock").sent.remove(&job);
                if let Some((id, version)) = self.landed(landed, &job, followed, first)? {
                    let name = asset.clone();
                    work.push(async move { Done::Arrived { name, result: self.arrived_owed(&id, version).await } }.boxed());
                }
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
    fn landed(&self, landed: Followed, job: &str, followed: &mut HashSet<String>, first: &mut Option<Error>) -> Result<Option<(RunId, Option<Coming>)>, Error> {
        followed.remove(&landed.key);
        match landed.landed? {
            Ok(behind) => Ok(behind),
            Err(error) => {
                match &landed.name {
                    Some(name) => self.failed_of(name, "", landed.inputs, landed.run, Some(job), error, first),
                    None => self.failed(error, first),
                }
                Ok(None)
            }
        }
    }

    /// Whether job `key`, whose machine went as it was sent, is placed again: a few times, then it fails as sent.
    fn place_again(&self, key: &str) -> bool {
        let mut replaced = self.replaced.lock().expect("replaced lock");
        let times = replaced.entry(key.to_owned()).or_default();
        *times += 1;
        *times <= PLACED_AGAIN
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

    /// Measure before taking the admission lock. An ask can join meanwhile,
    /// so check what is owed again before closing the walk under the lock.
    fn over(&self, first: &mut Option<Error>, following: &tokio::task::JoinHandle<()>) -> Result<bool, Error> {
        let owes = |walk: &folder::WalkFile| !walk.walk.asks.is_empty() || !walk.pending.is_empty();
        if owes(&self.folder().walk_now()?) {
            return Ok(false);
        }
        self.measure_at_end();
        let mut walking = self.project.walking.lock().expect("walking lock");
        let walk = self.folder().walk_now()?;
        if owes(&walk) {
            return Ok(false);
        }
        if !walk.walk.renames.is_empty() {
            self.folder().update_lock(|lock| {
                for (from, to) in std::mem::take(&mut lock.walk.renames) {
                    // Still under its old name: a rename an older Calcifer left for this moment.
                    if lock.assets.contains_key(&name_key(&from)) && lock.asset(&to).is_none_or(|held| held.on_record().next().is_none()) {
                        renamed(lock, &from, &to);
                        if lock.asset(&to).is_some_and(|held| held.versions.iter().any(|version| !version.clips.is_empty())) {
                            film_renamed(self.folder(), &from, &to);
                        }
                    }
                }
            })?;
        }
        self.folder().update_walk(|walk| {
            walk.walk.runs.clear();
            walk.walk.landing.clear();
            walk.walk.numbered.clear();
        })?;
        *self.project.walker.lock().expect("walker lock") = Walker::default();
        self.folder().drop_snapshots();
        self.end(&mut walking, first.take().map(|error| self.last_word(error)), following);
        Ok(true)
    }

    /// What failed, of no one asset: said now; what does not read it goes on.
    fn failed(&self, error: Error, first: &mut Option<Error>) {
        self.missed.lock().expect("missed lock").failed += 1;
        self.project.unsent.lock().expect("unsent lock").push(error.clone());
        self.say(Told::Failed { error: error.clone(), name: None, run: None });
        first.get_or_insert(error);
    }

    /// What failed of `name`, by `run` if it is of one: said now, kept for whoever reads it later, never sent again this walk; what does not read
    /// it goes on. Canceling is said, and isn't a failure of the asset.
    /// Of take `take`, by its id, when it's one: kept on its asset as that take's, listed until it's dismissed (cl-954).
    fn failed_of(&self, name: &str, kind: &str, inputs: Option<String>, run: Option<u64>, take: Option<&str>, error: Error, first: &mut Option<Error>) {
        {
            let mut missed = self.missed.lock().expect("missed lock");
            missed.failed += 1;
            missed.names.insert(name.to_owned());
        }
        if error.code != CANCELED {
            let failed = Failed { inputs, error: error.clone() };
            let ended = take.filter(|take| !take.is_empty()).map(|id| folder::Ended { id: id.to_owned(), run, error: error.clone() });
            let kept = self.folder().update_lock(|lock| {
                let held = lock.asset_mut(name, kind);
                held.failed = Some(failed.clone());
                if let Some(ended) = &ended {
                    held.ended.retain(|was| was.id != ended.id);
                    held.ended.push(ended.clone());
                    let over = held.ended.len().saturating_sub(folder::ENDED_KEPT);
                    held.ended.drain(..over);
                }
            });
            if let Err(error) = kept {
                warn!("{name}: {error}");
            }
        }
        self.say(Told::Failed { error: error.clone(), name: Some(name.to_owned()), run });
        first.get_or_insert(error);
    }

    /// Each current cut at the walk's end, what a render ends on, its film back at the root if someone moved it.
    async fn say_cuts(&self, doc: &Doc, stand: &[Standing]) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        for i in (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)) {
            self.say_cut(doc, stand, &lock, i).await;
        }
        Ok(())
    }

    /// Cut `i` stands, as it did: its film back at the root if someone moved it, and said, once a walk.
    async fn say_cut(&self, doc: &Doc, stand: &[Standing], lock: &Lock, i: usize) {
        let name = cut_name(&doc.assets[i].name);
        let Some(file) = stand[i].files.as_ref().and_then(|files| files.get("video")).and_then(|sha256| lock.files.get(sha256)) else { return };
        self.film_follows(doc, stand, lock, i).await;
        if !self.cut.lock().expect("cut lock").insert(name.clone()) {
            return;
        }
        let row = cut_row(doc, self.folder(), lock, stand, i);
        self.say(Told::Cut { name: doc.assets[i].name.clone(), cut: name, frames: row.frames.unwrap_or(file.frames), fps: row.fps, retimed: row.retimed, took_s: 0.0, unchanged: true, without: row.without });
    }

    /// A standing cut's `.otio`, written again from its pieces; one that can't be says so and the walk goes on.
    async fn timeline_again(&self, asset: &format::Asset, standing: &Standing, lock: &Lock, name: &str) {
        let canvas = Canvas::of(&asset.inputs, &mut vec![]);
        let (mut pieces, _) = tools::pieces(&asset.inputs, &standing.reads, self.folder(), lock);
        if let Some(version) = standing.version().and_then(|number| lock.asset(&asset.name)?.version(number)) {
            pieces.retain(|piece| !version.without.iter().any(|left| name_key(&left.piece) == name_key(&piece.asset)));
        }
        tools::past_end(&mut pieces, canvas.fps);
        let project = self.project.clone();
        let (cut, fps) = (name.to_owned(), canvas.fps);
        let written = async {
            let (_, ffprobe) = self.host.ffmpeg().await?;
            let access = vault::admit_work(&project).await?;
            tokio::task::spawn_blocking(move || {
                let _access = access;
                tools::timeline(&ffprobe, &project.folder, &pieces, fps, &cut)
            }).await.map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))?
        }.await;
        if let Err(error) = written {
            self.say(Told::Warning { message: format!("{name}'s timeline is gone and isn't written again: {}", error.message), name: None });
        }
    }

    /// Sends `next` under its key: a new version of its asset on its target's machines, recorded as pending together with what
    /// it settles of the walk, before the run is sent; one that fails is settled all the same.
    /// `member`: its place among the runs this turn sends to the machine its quote chose, which go together.
    async fn send(&self, ask: &Ask, doc: &Doc, stand: &[Standing], lock: &Lock, next: Next, member: host::Member) -> Result<(), Error> {
        let i = next.i;
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        let key = next.key.clone();
        let settle = |owed: &mut folder::Owed| {
            let Some(mine) = owed.asks.iter_mut().find(|mine| mine.id == ask.id) else { return };
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
            return self.folder().update_walk(|walk| settle(&mut walk.walk));
        }
        let standing = &stand[i];
        let noun = noun(doc, lock, i);
        if !new && standing.files.is_some() {
            return self.folder().update_walk(|walk| settle(&mut walk.walk));
        }
        match &standing.state {
            Stand::Stale(why) if !new => self.say(Told::Said { message: format!("{name}: out of date: {why}"), name: Some(name.clone()) }),
            Stand::Missing if !new => self.say(Told::Said { message: format!("{name}: no {noun} yet"), name: Some(name.clone()) }),
            Stand::Pinned { changed, .. } if next.unpin => self.say(Told::Said { message: format!("{name}: pinned, and {} changed: a new {noun} takes its place, and the one it held stays", changed.join(", ")), name: Some(name.clone()) }),
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
                    return Err(cannot(&name, format!("{} of {} is not on this computer; download it in the browser opened by cozy open", gone.join(", "), read.name)));
                }
            }
            let with = doc.target(i);
            let package = self.again(|| self.host.package(&target.hub, &with, &self.folder().dir, &target.on)).await?;
            if let Some(moved) = &package.moved {
                self.say(match moved.level {
                    api::calcifer::NoteLevel::Warning => Told::Warning { message: moved.message.clone(), name: None },
                    api::calcifer::NoteLevel::Info => Told::Said { message: moved.message.clone(), name: None },
                });
            }
            // What it outputs says what `defaults` fill, and so its key: planned again knowing that.
            if read_tags(doc, self.folder(), lock, i, &package) {
                self.project.walker.lock().expect("walker lock").again.insert(key.clone());
                return Ok(());
            }
            // Before anything that could cost.
            if let Some(why) = doc.missing_inputs(i, &package.reserved(), lock) {
                return Err(Error::new(Kind::Validation, "input_missing", why).remedy(format!("bring it back, or edit {name}'s text")));
            }
            let Some(maker) = package.maker()? else {
                self.say(Told::Warning { message: no_function(&name, &package.name), name: None });
                return self.folder().update_walk(|walk| settle(&mut walk.walk));
            };
            let now = self.folder().lock()?;
            let holds = now.asset(&name).and_then(|held| held.versions.iter().rev().find(|version| version.inputs == inputs)).map(|version| version.number);
            let going = now.pending.values().any(|pending| name_key(&pending.asset) == name_key(&name) && pending.version.inputs == inputs);
            let pickup = next.take.as_ref().and_then(|take| take.pickup.clone());
            // What stays on a machine that's gone isn't read: a function does without it.
            let mut gone = BTreeSet::new();
            let taken = pickup.iter().flat_map(|pickup| self.take_files(doc, lock, stand, &pickup.take));
            for sha256 in standing.reads.iter().flat_map(|read| read.files.values().cloned()).chain(taken) {
                if let Some(file) = lock.files.get(&sha256)
                    && matches!(self.reach(file, &target.on).await, Reach::Gone)
                {
                    gone.insert(sha256);
                }
            }
            let unknown = || Error::new(Kind::NotFound, "callable_unknown", format!("{} has no function {}", package.name, maker.function));
            let callable = package.callable(&maker.function).ok_or_else(unknown)?;
            let fields = doc.inputs_of(i);
            let reserved = package.reserved();
            let call = binder::Call { name: &name, tool: &doc.target(i), package: &package.name, callable: &callable, fields: &fields, reserved: &reserved, doc, own: Some(i), lock, stand, gone: &gone, pickup: pickup.as_ref(), inline: &[] };
            let mut bound = binder::bind(&call)?;
            self.cut(&mut bound, lock).await.map_err(|why| cannot(&name, why))?;
            let (input, mut warnings, unused) = (bound.payload, bound.warnings, bound.unused);
            warnings.dedup();
            // A function with no seed has one result per inputs, as has an asset that names one.
            let fixed = input.contains_key("seed");
            let seeded = !fixed && maker.accepts.contains("seed");
            if !seeded && pickup.is_none() && (holds.is_some() || going) {
                let stands = holds.map_or_else(|| "one is being made".to_owned(), |number| format!("{noun} {number} stands"));
                let why = if fixed { "fixes its seed" } else { "takes no seed" };
                match holds {
                    Some(number) => self.stands(&name, noun, number, format!("{} {why}, so it has one result per inputs", package.name)),
                    None => self.say(Told::Said { message: format!("{name}: {} {why}, so it has one result per inputs: {stands}", package.name), name: Some(name.clone()) }),
                }
                return self.folder().update_walk(|walk| settle(&mut walk.walk));
            }
            warnings.iter().for_each(|message| self.say(Told::Warning { message: message.clone(), name: None }));
            // What it can't use is struck through on its box, and said once here: a note, not a warning.
            unused.iter().for_each(|unused| self.say(Told::Said { message: unused.line(&name), name: Some(name.clone()) }));
            // Its id is its take's, from the render that asked it (cl-945).
            let mut version = Version { id: key.clone(), inputs: inputs.clone(), warnings, text: doc.text(i), fields: Some(doc.authoring_fields(i)), ..Default::default() };
            if seeded {
                // A failed run's seed is kept for the next try on these inputs, so finished segments aren't paid twice.
                // Try again is a render of what failed on these inputs: the same seed, so its finished calls come from the memo.
                let tried = now.asset(&name).and_then(|held| held.tried.as_ref()).filter(|tried| tried.inputs == inputs).map(|tried| tried.seed);
                version.seed = Some(tried.unwrap_or_else(new_seed));
            }
            version.pickup = pickup;
            // Its number now, whichever of several lands first.
            let listed = now.listed_number(&name);
            let number = std::cell::Cell::new(0);
            self.folder().update_walk(|walk| {
                let last = walk.walk.numbered.entry(name_key(&name)).or_default();
                *last = (*last).max(listed) + 1;
                number.set(*last);
            })?;
            version.number = number.get();
            let pending = Pending { job: key.clone(), kind: asset.kind.clone(), ask: ask.id.clone(), asset: name.clone(), version, run: 0, approve, unpin: next.unpin.then(|| asset.kind.clone()), target: target.clone() };
            let settling = |walk: &mut folder::WalkFile| settle(&mut walk.walk);
            if now.asset(&name).is_some_and(|held| held.failed.is_some()) {
                self.folder().update_lock(|lock| {
                    if let Some(held) = lock.assets.get_mut(&name_key(&name)) {
                        held.failed = None;
                    }
                })?;
            }
            // Canceled meanwhile: not sent.
            let now = self.folder().lock()?;
            let mine = now.walk.asks.iter().find(|mine| mine.id == ask.id);
            let stands = mine.is_some_and(|mine| match &next.take {
                Some(take) => mine.takes.iter().any(|still| still.key == take.key),
                None => !mine.sent.contains(&name_key(&name)),
            });
            if !stands {
                return Ok(());
            }
            // Renamed meanwhile: what this read says what was, so its turn comes again.
            if mine.is_some_and(|mine| mine.digest != ask.digest) {
                self.project.walker.lock().expect("walker lock").again.insert(key.clone());
                return Ok(());
            }
            // Its machines stopped while it was on its way.
            let mut pending = pending;
            pending.target.on = self.sending(&ask.id, &pending.target.on);
            if pending.target.on.is_empty() {
                self.not_sent(&name);
                return self.folder().update_walk(|walk| settle(&mut walk.walk));
            }
            self.submit(&key, input, &package, asset, pending, &settling, member, &unused).await
        };
        let sent = sent.await;
        if sent.is_err() {
            self.folder().update_walk(|walk| settle(&mut walk.walk))?;
        }
        sent
    }

    /// Whether this walk sends anywhere but this computer's own machine, which needs no network.
    fn elsewhere(&self) -> bool {
        let Ok(lock) = self.folder().lock() else { return false };
        let on = lock.walk.asks.iter().map(|ask| &ask.target).chain(lock.pending.values().map(|pending| &pending.target));
        on.flat_map(|target| &target.on).any(|on| !matches!(on, On::Machine(name) if name.eq_ignore_ascii_case("local")))
    }

    /// `sha256` from `machine`, here. This computer with no network says nothing of whether it can be had: that waits for the network.
    async fn fetching(&self, sha256: &str, machine: &str) -> Result<(), String> {
        loop {
            match self.host.fetch(sha256, machine).await {
                Err(_) if self.elsewhere() && !self.host.online().await => tokio::time::sleep(OFFLINE).await,
                fetched => return fetched,
            }
        }
    }

    /// The files of the take a pickup names, `<shot_1.3>`.
    fn take_files(&self, doc: &Doc, lock: &Lock, stand: &[Standing], take: &str) -> Vec<String> {
        let Some(mention) = graph::mentions(take).first().and_then(|inner| graph::mention(inner)) else { return vec![] };
        let Some(j) = doc.find(mention.name) else { return vec![] };
        let files = match mention.version {
            Some(number) => doc.version_files(j, number, lock),
            None => stand[j].files.clone(),
        };
        files.into_iter().flat_map(|files| files.into_values()).collect()
    }

    /// Each range `bound` names cut on this computer with cozy's ffmpeg, and named in its payload by the clip's hash.
    async fn cut(&self, bound: &mut binder::Bound, lock: &Lock) -> Result<(), String> {
        if bound.cuts.is_empty() {
            return Ok(());
        }
        let home = self.host.home();
        let scope = crate::private_io::of_path(&self.project.folder.dir).map_err(|e| e.message)?;
        let dir = crate::private_io::cache(scope.as_ref(), home.root().join("cache").join("clips"), "clips").map_err(|e| e.message)?;
        let clips = crate::runs::clip::Clips { dir, home: home.clone() };
        let mut cut = vec![];
        for range in &bound.cuts {
            let file = lock.files.get(&range.source).ok_or_else(|| format!("{} isn't one of the project's files", folder::prefix_of(&range.source)))?;
            let path = self.folder().path(&file.path);
            if !path.is_file() || file.fps <= 0.0 {
                return Err(format!("{} must be on this computer, and measured, to cut a range of it; download it in the browser opened by cozy open", folder::prefix_of(&range.source)));
            }
            let mut notes = vec![];
            let access = vault::admit_work(&self.project).await.map_err(|e| e.message)?;
            let made = clips.cut(&range.range.to_string(), &path, range.range, file.fps, &mut notes, access).await.map_err(|e| e.message)?;
            notes.into_iter().filter(|note| note.level == api::calcifer::NoteLevel::Warning).for_each(|note| self.say(Told::Warning { message: note.message, name: None }));
            let clip = made.clip.clone();
            let access = vault::admit_work(&self.project).await.map_err(|e| e.message)?;
            let sha256 = tokio::task::spawn_blocking(move || {
                let _access = access;
                folder::hash(&clip)
            }).await.map_err(|e| e.to_string())?.map_err(|e| e.message)?;
            self.clips.lock().expect("clips lock").insert(sha256.clone(), made.clip);
            cut.push(sha256);
        }
        let mut payload = Value::Object(std::mem::take(&mut bound.payload));
        binder::cuts_filled(&mut payload, &cut);
        bound.payload = payload.as_object().cloned().unwrap_or_default();
        Ok(())
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

    /// Sends `input` to the function `package` names on one of the pending's machines, as an ordinary run under `key`, recorded as pending
    /// (with `settle`: what it settles of the walk) in one write before it is sent. The seed and the asset's `models` and `lora`, as
    /// `cozy run --input` takes them, go on top.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    async fn submit(&self, key: &str, mut payload: Map<String, Value>, package: &Package, asset: &format::Asset, mut pending: Pending, settle: &impl Fn(&mut folder::WalkFile), member: host::Member, unused: &[Unused]) -> Result<(), Error> {
        let renderer = package.maker()?.ok_or_else(|| cannot(&asset.name, "its tool names no function".into()))?;
        let (on, hub) = (pending.target.on.clone(), pending.target.hub.clone());
        let name = asset.name.clone();
        let named = |e: Error| Error { message: format!("{name}'s render: {}", e.message), ..e };
        if let Some(seed) = pending.version.seed.filter(|_| renderer.accepts.contains("seed")) {
            payload.insert("seed".into(), json!(seed));
        }
        for field in ["models", "lora"] {
            if let Some(value) = asset.made.get(field) {
                payload.insert(field.into(), value.clone());
            }
        }
        let called = renderer.target();
        let mut choices = Choices::new(&called, &renderer.function, &renderer.slots, renderer.call_slots());
        choices.take(&mut payload).map_err(named)?;
        pending.version.tool = Some(renderer.target());
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
        warnings.extend(unused.iter().map(|unused| Warning { code: api::calcifer::MENTION_UNUSED.into(), message: unused.line(&name) }));
        let models = choices::wire(&choices.into_choices()).map_err(named)?;
        let payload = Value::Object(payload);
        let lock = self.folder().lock()?;
        let (mut inputs, mut staged, mut held, mut holder) = (vec![], vec![], vec![], None);
        let clips = self.clips.lock().expect("clips lock").clone();
        for (field, path, digest) in tools::files_in(&payload, self.folder(), &lock, &clips) {
            // What stays on a machine: read there by its hash, else fetched now that a run needs it.
            let mut on_machine = false;
            let Some(file) = lock.files.get(&digest) else {
                let length = std::fs::metadata(&path).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())))?.len();
                inputs.push(v1::InputFile { field: field.clone(), digest: digest.clone(), length, media_type: media_type(&path).to_owned(), order: order_of(&field) });
                staged.push(StagedInput { path, digest: digest.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?, length });
                continue;
            };
            match self.reach(file, holder.as_ref().map_or(on.as_slice(), std::slice::from_ref)).await {
                Reach::Here => {}
                Reach::There(machine) => {
                    self.say(Told::Said { message: format!("{name}: {field} is read on {machine}, which holds it"), name: Some(name.clone()) });
                    holder = Some(machine);
                    on_machine = true;
                }
                Reach::Fetch => {
                    self.say(Told::Said { message: format!("{name}: fetching {field} from {}", file.machine), name: Some(name.clone()) });
                    let fetched = self.fetching(&digest, &file.machine).await;
                    fetched.map_err(|why| cannot(&name, format!("{field} is on {}, and can't be had: {why}", file.machine)))?;
                }
                Reach::Gone => return Err(cannot(&name, format!("{field} is gone from its machine {}", file.machine))),
            }
            let length = match on_machine {
                true => file.length,
                false => {
                    std::fs::metadata(&path)
                        .map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())).next("cozy open"))?
                        .len()
                }
            };
            let (order, media_type) = (order_of(&field), media_type(&path).to_owned());
            inputs.push(v1::InputFile { field, digest: digest.clone(), length, media_type, order });
            let digest: Digest = digest.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?;
            match on_machine {
                true => held.push(digest),
                false => staged.push(StagedInput { path, digest, length }),
            }
        }
        // Where it finishes soonest, as its machines quoted, which has its weights coming; a machine holding a file it reads, else.
        let job = pending.job_of(key).to_owned();
        let (soonest, mut offer) = self.chosen(&job, &on, &hub, package, &renderer, asset).await.map_err(named)?;
        let placed = match holder {
            Some(holder) if holder != soonest => {
                let placed = self.project.walker.lock().expect("walker lock").placed.get(&job).cloned();
                offer = placed.and_then(|placed| placed.offers.into_iter().find(|offer| offer.machine == holder));
                holder
            }
            _ => soonest,
        };
        // Sent here alone, as `cozy render --on local`, which asks nothing: said as a box asks it, unless Render here was said (cl-897).
        if placed.is_local() && on.len() == 1 {
            let tool = asset.tool.clone().unwrap_or_else(|| asset.target.clone());
            if let Some(line) = fit::told_here(&self.host.api(), &hub, package, &renderer.function, &asset.made, &tool) {
                self.say(Told::Said { message: format!("{name}: {line}"), name: Some(name.clone()) });
            }
        }
        // Local code is made ready on the machine the call goes to, its files and manifest sent
        // with every call: a machine that dropped them installs them in the call.
        let source = match &renderer.local {
            None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() }),
            Some(path) => {
                let prepared = self.again(|| async {
                    let scope = crate::private_io::of_path(&self.folder().dir)?;
                    crate::private_io::context(scope, self.host.ready(path, &placed, &hub)).await
                }).await.map_err(named)?;
                staged.extend(prepared.files);
                warnings.extend(prepared.from.map(|from| Warning { code: api::calcifer::FROM_SOURCE.into(), message: from.line() }));
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
        let submit = SubmitRun { project: None,
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
            stays: package.callable(&renderer.function).map(|callable| api::calcifer::tool::stays(&callable)).unwrap_or_default(),
        };
        let _on_the_way = OnTheWay::new(&self.on_the_way, key);
        self.folder().update_walk(|walk| {
            walk.pending.insert(key.into(), pending.clone());
            settle(walk);
        })?;
        let sent = match member.on() == Some(&submit.on) {
            true => match member.submit(key, submit.clone()).await {
                Err(_) => self.send_run(key, submit).await,
                sent => sent,
            },
            false => {
                drop(member);
                self.send_run(key, submit).await
            }
        };
        let run = match sent {
            Ok(run) => run,
            Err(error) => {
                self.folder().update_walk(|walk| {
                    walk.pending.remove(key);
                })?;
                return Err(error);
            }
        };
        self.folder().update_walk(|walk| {
            if let Some(pending) = walk.pending.get_mut(key) {
                pending.run = run.number;
            }
            walk.walk.runs.insert(run.number, None);
        })?;
        if let Some(offer) = offer {
            self.host.quoted(&run.id, offer.quoted());
            self.project.walker.lock().expect("walker lock").sent.insert(job, offer);
        }
        self.sync();
        crate::events::project_settled(&self.folder().dir);
        Ok(())
    }

    /// Whether `error` is the project refusing a write it may take later: a run is paid for and its bytes are here, so what's landing
    /// stays owed in `.runs/walk.json`, says so, and nothing runs again. A folder that's gone isn't that.
    fn held_up(&self, error: &Error) -> bool {
        if !matches!(error.code.as_str(), "file_held" | "project_io") || !self.folder().dir.is_dir() {
            return false;
        }
        if self.folder().unsaved().is_none() {
            self.say(Told::Warning { message: not_saved(&error.message), name: None });
            self.folder().set_unsaved(Some(error.message.clone()));
            crate::events::project_settled(&self.folder().dir);
        }
        true
    }

    /// The project took what was held up.
    fn taken(&self) {
        if self.folder().unsaved().is_some() {
            self.folder().set_unsaved(None);
            crate::events::project_settled(&self.folder().dir);
        }
    }

    /// `land`, however long the project takes to take it. Each try waits on the project's turn and SQLite's lock off the async workers,
    /// which answer every read meanwhile (cl-837).
    async fn land_owed(&self, key: &str, run: &Run) -> Result<Option<Coming>, Error> {
        loop {
            match tokio::task::block_in_place(|| self.land(key, run)) {
                Err(error) if self.held_up(&error) => self.until_free().await,
                done => {
                    self.taken();
                    return done;
                }
            }
        }
    }

    /// Run `id`'s files, once they're all here, onto its version, however long the project takes to take that.
    async fn arrived_owed(&self, id: &RunId, version: Option<Coming>) -> Result<(), Error> {
        let run = self.host.wait(id).await?;
        loop {
            match tokio::task::block_in_place(|| self.arrived(&run, version.clone())) {
                Err(error) if self.held_up(&error) => self.until_free().await,
                done => {
                    self.taken();
                    return done;
                }
            }
        }
    }

    /// Back once project.db takes a write again: asked each `HELD_UP` with no wait on its lock, so no try sits in SQLite's wait
    /// with the project's turn while another program holds it, and a write of anyone's goes first.
    async fn until_free(&self) {
        loop {
            tokio::time::sleep(HELD_UP).await;
            if self.folder().held_elsewhere() == Some(false) {
                return;
            }
        }
    }

    /// Follows the run sent under `key` to its end and lands it, its pending entry gone however it ends, and kept while the project
    /// won't take it (`owed`).
    async fn follow(&self, key: String) -> Followed {
        let owed = self.folder().walk_now().ok().and_then(|walk| walk.pending.get(&key).map(|pending| (pending.asset.clone(), pending.version.inputs.clone(), pending.run)));
        let (name, inputs, run) = match owed {
            Some((name, inputs, run)) => (Some(name), Some(inputs), Some(run)),
            None => (None, None, None),
        };
        let landed = async {
            let Some(run) = self.host.sent(&key)? else { return Ok(None) };
            let ended = self.host.ended(&run.id).await?;
            // What reads it goes now, on its machine. The walk isn't over until the bytes are here.
            Ok(Some((run.id, self.land_owed(&key, &ended).await?)))
        };
        let landed = landed.await;
        let landed = match self.folder().update_walk(|walk| drop(walk.pending.remove(&key))) {
            Ok(()) => Ok(landed),
            Err(error) => Err(error),
        };
        crate::events::project_settled(&self.folder().dir);
        Followed { key, name, inputs, run: run.filter(|run| *run > 0), landed }
    }

    /// Run `run` has nothing more on its way here. What landed after its version was listed is that version's, no loose file of the
    /// project's. One of its files that never came means no version: there's nothing of it to see, and a render makes it again.
    fn arrived(&self, run: &Run, version: Option<Coming>) -> Result<(), Error> {
        let folder = self.folder();
        let lost = run.output.iter().find(|output| !output.stays && output.export_error.is_some());
        // One write, the project first: killed between, what never came is gone and a restart hears of this run again, which changes nothing.
        folder.update_lock(|lock| {
            lock.runs.retain(|row| row.run != run.number);
            lock.walk.runs.remove(&run.number);
            lock.walk.landing.retain(|row| row.run != run.number);
            if let (Some(coming), true) = (&version, lost.is_some()) {
                let held = lock.asset_mut(&coming.name, "");
                held.versions.retain(|version| version.number != coming.number);
                held.discarded.retain(|version| version.number != coming.number);
            }
        })?;
        let Some(Coming { name, number, take }) = version else { return Ok(()) };
        self.project.jobs.arrived(&name, number, lost.is_none());
        let Some(output) = lost else { return Ok(()) };
        let mut tally = self.tally.lock().expect("tally lock");
        match take {
            true => tally.takes = tally.takes.saturating_sub(1),
            false => tally.versions = tally.versions.saturating_sub(1),
        }
        let said = format!("{name}: run {} made its file, and it can't be brought here: {}", run.number, output.export_error.clone().unwrap_or_default());
        Err(Error::new(Kind::Unavailable, "version_unsaved", said).next(format!("cozy run watch {}", run.number)))
    }

    /// The run sent under `key`, once it ended on its machine, into the project: its outputs listed as its asset's newest version in
    /// the lock, kept as files once they land, and read on its machine by their hashes until then. A run that did not complete is
    /// no version, with its reason, and is never run again this walk. What it listed, if its files are on their way.
    fn land(&self, key: &str, run: &Run) -> Result<Option<Coming>, Error> {
        let folder = self.folder();
        let Some(pending) = folder.walk_now()?.pending.remove(key) else { return Ok(None) };
        let name = pending.asset.clone();
        // Nothing lands under a name that's gone: a later asset of that name would inherit it.
        if self.gone(&name) {
            self.deleted(pending.job_of(key), &name);
            return Ok(None);
        }
        if run.status != RunStatus::Completed {
            // Canceled because its machine stopped: not sent, as far as anyone paid for it.
            if run.status == RunStatus::Canceled && run.started_at_ms.is_none() && run.machine.as_deref().is_some_and(|machine| self.stopped.lock().expect("stopped lock").contains_key(machine)) {
                self.not_sent(&name);
                return Ok(None);
            }
            self.missed.lock().expect("missed lock").names.insert(name.clone());
            // Made again with what it asked, its seed too; and its number stays taken, so the next try is the next.
            let tried = pending.version.seed.map(|seed| Tried { inputs: pending.version.inputs.clone(), seed });
            let remains = folder.act(|| {
                // A delete can stage its cancellation while its commit waits on SQLite.
                // Readers still see the old name then. Recheck under the same project
                // transaction as the mutation, so a late canceled result cannot create an
                // empty live row that hides the deleted asset's retained versions from undo.
                if self.gone(&name) {
                    return Ok(false);
                }
                folder.update_lock(|lock| {
                    let held = lock.asset_mut(&name, &pending.kind);
                    held.numbered = held.numbered.max(pending.version.number);
                    if tried.is_some() {
                        held.tried.clone_from(&tried);
                    }
                })?;
                Ok(true)
            })?;
            if !remains {
                self.deleted(pending.job_of(key), &name);
                return Ok(None);
            }
            if let (Some(machine), Some(reason), RunStatus::Failed) = (&run.machine, &run.reason, run.status) {
                self.fault(machine, &reason.code, &reason.message)?;
            }
            let status = format!("{:?}", run.status).to_lowercase();
            let said = match &run.reason {
                Some(reason) => format!("{name}: {status}: {}", reason.message),
                None => format!("{name}: {status}"),
            };
            let code = if run.status == RunStatus::Canceled { CANCELED } else { "version_failed" };
            let next = run.reason.iter().flat_map(|reason| reason.next.iter().cloned()).chain([format!("cozy run watch {}", run.number)]);
            return Err(next.fold(Error::new(Kind::Unavailable, code, said), Error::next));
        }
        let mut version = pending.version.clone();
        version.run = Some(run.number);
        version.run_id = Some(run.id.clone());
        version.execution_ms += run.execution_ms.unwrap_or(0);
        for said in said_by(run) {
            let message = format!("{name}: {said}");
            if !self.project.jobs.warned(&name, &said) {
                self.say(Told::Warning { message: said, name: Some(name.clone()) });
            }
            if !version.warnings.contains(&message) {
                version.warnings.push(message);
            }
        }
        // Every file the run saved, by its output's name (a list's items as `<name>-<n>`).
        let (mut saved, mut stayed, mut coming, mut away) = (Files::new(), Files::new(), false, vec![]);
        let mut metadata = folder::Metadata::new();
        for output in &run.output {
            // The first of a list goes by its name, so a reader that takes one takes it.
            let file = match output.index {
                0 | 1 => output.name.clone(),
                n => format!("{}-{n}", output.name),
            };
            match declared(run.result.as_ref(), &output.name, output.index) {
                Ok(said) if said.is_empty() => {}
                Ok(said) => {
                    metadata.insert(file.clone(), said);
                }
                Err(why) => version.warnings.push(format!("{name}: {file}'s metadata {why}, so it was left off")),
            }
            // What stays on its machine is listed with who holds it, and no file.
            if output.stays && output.path.as_ref().is_none_or(|path| !path.is_file()) {
                if let (Some(sha256), Some(machine)) = (&output.sha256, &run.machine) {
                    away.push((sha256.to_string(), machine.clone(), output.length, None));
                    stayed.insert(file, sha256.to_string());
                }
                continue;
            }
            let sha256 = match output.path.as_ref().filter(|path| path.is_file()) {
                Some(path) => folder.keep(path)?,
                // On its way here: its machine holds it, and whatever reads it there needn't wait for this computer.
                None if output.owed() && output.media_type != api::tree::MEDIA => {
                    let (Some(sha256), Some(machine)) = (&output.sha256, &run.machine) else { continue };
                    let name = crate::outputs::file_name(run.number, &output.name, output.index, &output.media_type);
                    away.push((sha256.to_string(), machine.clone(), output.length, Some(name)));
                    coming = true;
                    sha256.to_string()
                }
                None => continue,
            };
            saved.insert(file, sha256);
        }
        if saved.is_empty() && version.files.is_empty() {
            let said = format!("{name}: run {} saved no file", run.number);
            return Err(Error::new(Kind::Unavailable, "version_unsaved", said).next(format!("cozy run watch {}", run.number)));
        }
        saved.append(&mut stayed);
        version.files.extend(saved.clone());
        metadata.retain(|file, _| saved.contains_key(file));
        version.metadata.extend(metadata);
        // On its way, its version is listed: a restart follows it from here.
        let on_its_way = coming.then(|| Coming { name: name.clone(), number: version.number, take: version.files.contains_key("video") });
        let list_away = |lock: &mut Lock| {
            away.iter().for_each(|(sha256, machine, length, name)| lock.away(sha256, machine, *length, name.as_deref()));
            if let (Some(row), Some(coming)) = (lock.walk.runs.get_mut(&run.number), &on_its_way) {
                *row = Some(coming.clone());
            }
        };
        let kind = pending.kind.clone();
        let noun = if version.files.contains_key("video") { "take" } else { "version" };
        let key = key.to_owned();
        let number = version.number;
        let selected = format::KINDS.contains(&kind.as_str()).then(|| version.main().cloned()).flatten();
        self.record(Some(&key), &name, &kind, version, noun, pending.approve, list_away)?;
        if coming {
            self.project.jobs.why(&name, number, "its file is on its way here".into());
        }
        if let Some(sha256) = selected {
            self.select_asked(&pending.ask, &name, number, &sha256)?;
        } else if pending.unpin.is_some() {
            self.unpin_landed(&pending.ask, &name);
        }
        Ok(coming.then_some(Coming { name, number, take: noun == "take" }))
    }

    /// `name`'s re-run landed, so what it approved goes: from the project, and from the snapshot its render plans on, so what reads it reads this.
    fn unpin_landed(&self, ask: &str, name: &str) {
        if let Err(error) = self.folder().edit(|written| written.unapproved(name)) {
            self.say(Told::Warning { message: error.message, name: None });
        }
        if let Err(error) = self.unpin_asked(ask, name) {
            warn!("{name}: {error}");
        }
    }

    /// Render `ask`'s own snapshot holds the pin no more: what reads it there reads the new version.
    fn unpin_asked(&self, ask: &str, name: &str) -> Result<(), Error> {
        let walk = self.folder().walk_now()?;
        let Some(mine) = walk.walk.asks.iter().find(|mine| mine.id == ask) else { return Ok(()) };
        let mut written = self.folder().asked(mine)?;
        written.unapproved(name)?;
        let (digest, snapshot) = written.snapshot();
        self.folder().keep_snapshot(&digest, &snapshot)?;
        let doc = Doc::asked(self.folder(), &written)?;
        self.project.walker.lock().expect("walker lock").readings.entry(digest.clone()).or_insert_with(|| AskedText::new(doc));
        self.folder().update_walk(|walk| {
            for mine in walk.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
                mine.digest.clone_from(&digest);
            }
        })
    }

    /// An initially empty source gains the media this render made. A completed
    /// selected source stays the ask's input snapshot unless --outdated refreshes it.
    fn select_asked(&self, ask: &str, name: &str, number: u32, sha256: &str) -> Result<(), Error> {
        let walk = self.folder().walk_now()?;
        let Some(mine) = walk.walk.asks.iter().find(|mine| mine.id == ask) else { return Ok(()) };
        let mut written = self.folder().asked(mine)?;
        if !mine.outdated {
            let doc = Doc::asked(self.folder(), &written)?;
            if doc.find(name).is_some_and(|i| doc.assets[i].pin.is_some()) {
                return Ok(());
            }
        }
        written.selected(name, number, sha256)?;
        let (digest, snapshot) = written.snapshot();
        self.folder().keep_snapshot(&digest, &snapshot)?;
        let doc = Doc::asked(self.folder(), &written)?;
        self.project.walker.lock().expect("walker lock").readings.entry(digest.clone()).or_insert_with(|| AskedText::new(doc));
        self.folder().update_walk(|walk| {
            for mine in walk.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
                mine.digest.clone_from(&digest);
            }
        })
    }

    /// The frame of `video` at `at` seconds, half a frame early so one at its end is its last frame, kept as a file of the
    /// project. One past its end is its last, which is said.
    async fn frame(&self, name: &str, video: &FsPath, at: Option<f64>) -> Result<String, Error> {
        let (ffmpeg, _) = self.host.ffmpeg().await?;
        let (dir, path) = (self.folder().dir.clone(), video.to_owned());
        let early = at.map(|at| (at - 1.0 / 48.0).max(0.0));
        let access = vault::admit_work(&self.project).await?;
        // The abandoned result must drop its temporary frame before releasing the view.
        let (grabbed, _access) = tokio::task::spawn_blocking(move || {
            (tools::frame(&ffmpeg, &path, early, &dir), access)
        }).await.map_err(|e| Error::new(Kind::Internal, "frame", e.to_string()))?;
        let (frame, past) = grabbed?;
        if let (true, Some(at)) = (past, at) {
            self.say(Told::Warning { message: format!("{name}: its video ends before {at:.2} s: its last frame stands in"), name: None });
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
                self.say(Told::Warning { message: format!("{name} reads {}'s newest {noun}, {number}, which is out of date", read.name), name: None });
            }
        }
        let id = ask.takes.iter().find(|take| name_key(&take.asset) == name_key(&doc.assets[i].name)).map(|take| take.key.clone()).unwrap_or_default();
        let mut version = Version { id, inputs, fields: Some(doc.authoring_fields(i)), ..Default::default() };
        match doc.tool(i).expect("a tool") {
            Tool::Cut => {
                // Once a walk: what it cuts stands, or says why not.
                self.tried(&ask.id, &name)?;
                let Some((files, without)) = self.cut_tool(doc, asset, standing, &ask.target, &version.inputs).await? else {
                    self.say_cut(doc, stand, &self.folder().lock()?, i).await;
                    return Ok(());
                };
                version.without = without;
                version.clips = tools::clips(&asset.inputs, &standing.reads);
                for clip in &mut version.clips {
                    let name = graph::mentions(&clip.clip).first().map(|name| name_key(name));
                    let read = standing.reads.iter().find(|read| Some(name_key(&read.name)) == name);
                    clip.sha256 = read.and_then(|read| folder::main_of(&read.files).cloned());
                }
                version.fps = Canvas::of(&asset.inputs, &mut vec![]).fps;
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
                if file.stays() && !self.folder().path(&file.path).is_file() {
                    let brought = self.host.fetch(&file.sha256, &file.machine).await;
                    brought.map_err(|why| cannot(&name, format!("{} can't be had: {why}", read.name)))?;
                }
                let at = asset.inputs.get("at").and_then(Value::as_f64);
                let frame = self.frame(&name, &self.folder().path(&file.path), at).await?;
                version.files = Files::from([("image".to_owned(), frame)]);
            }
        }
        let number = std::cell::Cell::new(0);
        self.folder().update_lock(|lock| number.set(lock.number(&name, &asset.kind)))?;
        version.number = number.get();
        let selected = version.main().cloned();
        let approve = ask.takes.iter().any(|take| name_key(&take.asset) == name_key(&name) && take.approve);
        self.record_free(&name, &asset.kind, version, noun(doc, lock, i), approve)?;
        if let Some(sha256) = selected {
            self.select_asked(&ask.id, &name, number.get(), &sha256)?;
        }
        Ok(())
    }

    /// `name` is tried this walk: it is not tried again until the next.
    fn tried(&self, ask: &str, name: &str) -> Result<(), Error> {
        let key = name_key(name);
        self.folder().update_walk(|walk| {
            for mine in walk.walk.asks.iter_mut().filter(|mine| mine.id == ask) {
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
        let canvas = Canvas::of(&asset.inputs, &mut said);
        // ffprobe first, as this may be its first need here, then every file measured.
        let (ffmpeg, ffprobe) = self.host.ffmpeg().await?;
        let lock = folder.lock()?;
        let (pieces, _) = tools::pieces(&asset.inputs, &standing.reads, folder, &lock);
        let name = cut_name(&asset.name);
        let on = target.on.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let mut without = self.bring_pieces(asset, &pieces, &lock, target, &ffprobe, &on).await?;
        let mut lock = folder.lock()?;
        folder.measure(&mut lock)?;
        let (mut pieces, skips) = tools::pieces(&asset.inputs, &standing.reads, folder, &lock);
        pieces.retain(|piece| !without.iter().any(|left| name_key(&left.piece) == name_key(&piece.asset)));
        for piece in skips {
            let (why, said) = match doc.find(&piece) {
                Some(_) => ("no version yet", format!("which has no version yet; render {piece} on {on} in the browser opened by cozy open")),
                None => ("no longer in the project", "which is no longer in the project".to_owned()),
            };
            self.say(Told::Warning { message: format!("{name} is cut without {piece}, {said}"), name: None });
            without.push(folder::Without { piece, why: why.into(), absent: false });
        }
        // The same edit, with everything it took here and its film right, isn't rendered again.
        if without.is_empty() && same_cut(folder, &lock, asset, &standing.reads, inputs).is_some() {
            return Ok(None);
        }
        self.leave_out(&name, &on, tools::unreadable(&ffprobe, &pieces)?, &mut without);
        pieces.retain(|piece| piece.still || piece.shape.is_some());
        said.extend(tools::past_end(&mut pieces, canvas.fps));
        said.iter().for_each(|message| self.say(Told::Warning { message: message.clone(), name: None }));
        let (project, cutting, began) = (self.project.clone(), self.project.cutting.clone(), Instant::now());
        let scope = crate::private_io::of_path(&self.project.folder.dir)?;
        let cache = crate::private_io::cache(scope.as_ref(), self.host.cache().join("cuts"), "cuts")?;
        let cutting_name = name.clone();
        let fps = canvas.fps;
        let (rendering, rendered) = (self.project.rendering.clone(), asset.name.clone());
        let access = vault::admit_work(&self.project).await?;
        rendering.lock().expect("rendering lock").insert(rendered.clone(), 0.0);
        crate::events::project_settled(&folder.dir);
        let cutting_result = tokio::task::spawn_blocking(move || {
            let _access = access;
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
        let (made, left) = cutting_result.map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))?.map_err(|e| e.next("cozy open"))?;
        self.leave_out(&name, &on, left, &mut without);
        let Some((made, part)) = made else {
            let stands = if folder.dir.join(&name).is_file() { ": the film that stands stays" } else { "" };
            self.say(Told::Warning { message: format!("{name} isn't cut, and no version is made: none of its clips can be used{stands}"), name: None });
            return Ok(None);
        };
        let now = folder.lock()?;
        let held = now.asset(&asset.name);
        let before = held.and_then(|held| held.versions.last()?.files.get("video").cloned());
        let sha256 = folder.take(&part)?;
        let path = folder.lock()?.files.get(&sha256).map(|file| folder.path(&file.path));
        let mut film = folder.dir.join(&name);
        if let Some(path) = path {
            // The version is in `files/`: a film at the root that can't be replaced costs it nothing.
            if let Err(e) = tools::publish(&path, &film) {
                self.film_stays(&name, &e);
                film = path;
            }
        }
        // The timeline it's the film of, kept with it: a version chosen later brings its own.
        let timeline = folder.keep(&folder.dir.join(&name).with_extension("otio"))?;
        // What the film is, not what its pieces add up to.
        let frames = folder.film_frames(&film).unwrap_or(made.frames);
        if frames.abs_diff(made.frames) > 1 + standing.reads.len() as u64 {
            self.say(Told::Warning { message: format!("{name} holds {frames} frames, and its pieces add up to {}", made.frames), name: None });
        }
        let (retimed, took_s) = (made.retimed, began.elapsed().as_secs_f64());
        self.cut.lock().expect("cut lock").insert(name.clone());
        let said = without.iter().map(folder::Without::said).collect();
        self.say(Told::Cut { name: asset.name.clone(), cut: name, frames, fps, retimed, took_s, unchanged: before.as_ref() == Some(&sha256), without: said });
        Ok(Some((Files::from([("video".to_owned(), sha256), ("timeline".to_owned(), timeline)]), without)))
    }

    /// Each clip a cut goes without, warned of with what was said of it, and recorded.
    fn leave_out(&self, name: &str, on: &str, left: Vec<tools::LeftOut>, without: &mut Vec<folder::Without>) {
        for tools::LeftOut { asset: piece, why, answer } in left {
            if without.iter().any(|held| name_key(&held.piece) == name_key(&piece)) {
                continue;
            }
            self.say(Told::Warning { message: format!("{name} is cut without {piece}: {answer}; render {piece} on {on} in the browser opened by cozy open"), name: None });
            without.push(folder::Without { piece, why, absent: false });
        }
    }

    /// Said once of a film at the root that couldn't be replaced: its version stands, and `film_follows` puts it there the next time it can.
    fn film_stays(&self, name: &str, error: &std::io::Error) {
        if !self.stays.lock().expect("stays lock").insert(name.to_owned()) {
            return;
        }
        let message = match platform::durable::held(error) {
            true => format!("{name} is open in another program, so it wasn't updated: close it, and the next render updates it"),
            false => format!("{name}: {error}"),
        };
        self.say(Told::Warning { message, name: None });
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
        if self.stays.lock().expect("stays lock").contains(&cut_name(&doc.assets[i].name)) {
            return;
        }
        let same = !tools::shares_bytes(&at) && tokio::task::block_in_place(|| folder::hash(&at)).is_ok_and(|got| got == file.sha256);
        if !same {
            if let Err(e) = tools::publish(&from, &at) {
                self.film_stays(&cut_name(&doc.assets[i].name), &e);
            }
        }
        let timeline = at.with_extension("otio");
        match files.get("timeline").and_then(|sha256| lock.files.get(sha256)) {
            Some(kept) if !same || !timeline.is_file() => {
                if let Err(e) = tools::publish(&self.folder().path(&kept.path), &timeline) {
                    self.say(Told::Warning { message: format!("{}: {e}", timeline.display()), name: None });
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
            let changed = if here {
                let access = vault::admit_work(&self.project).await?;
                tokio::task::spawn_blocking(move || {
                    let _access = access;
                    folder::hash(&path).is_ok_and(|got| got != digest)
                }).await.unwrap_or(false)
            } else {
                false
            };
            if here && !changed {
                continue;
            }
            let why = if changed { "its bytes changed" } else { "not on this computer" };
            self.say(Told::Said { message: format!("{}: {} is {why}: fetching it", asset.name, piece.asset), name: Some(asset.name.clone()) });
            let brought = match file.stays() && self.host.fetchable(&file.sha256, &file.machine).await {
                true => self.host.fetch(&file.sha256, &file.machine).await,
                false => self.host.pull_one(&self.project, &target.hub, file.clone()).await,
            };
            let Err(reason) = brought else { continue };
            let answer = changed.then(|| tools::unread(ffprobe, &piece.path)).flatten().map_or(String::new(), |answer| format!(" ({answer})"));
            let piece = piece.asset.clone();
            self.say(Told::Warning { message: format!("{} is cut without {piece}: {why}{answer}, and it can't be had: {reason}; render {piece} on {on} or download its files in the browser opened by cozy open", cut_name(&asset.name)), name: None });
            without.push(folder::Without { piece, why: why.into(), absent: !here });
        }
        Ok(without)
    }

    /// Lists the built-in tool's next version, retaining its earlier media states, approved only when `approve` (cl-908).
    fn record_free(&self, name: &str, kind: &str, version: Version, noun: &str, approve: bool) -> Result<(), Error> {
        let number = version.number;
        self.folder().act(|| {
            self.folder().update_lock(|lock| {
                let held = lock.asset_mut(name, kind);
                held.versions.push(version.clone());
                held.versions.sort_by_key(|version| version.number);
                held.failed = None;
            })?;
            if approve && let Some(sha256) = version.main() {
                self.folder().edit(|written| written.selected(name, number, sha256))?;
            }
            Ok(())
        })?;
        self.landed_unseen(name, number, approve);
        self.say_landed(name, &version, noun, number)
    }

    /// Lists `version` as its asset's newest (dropping the pending run under `key` in the same write), and says it landed. Only
    /// `approve` (`--approve`) approves it: a new take never changes what's approved, and a box with none approved stays so (cl-908).
    #[allow(clippy::too_many_arguments)]
    fn record(&self, key: Option<&str>, name: &str, kind: &str, mut version: Version, noun: &str, approve: bool, settle: impl Fn(&mut Lock)) -> Result<(), Error> {
        let number = std::cell::Cell::new(version.number);
        self.folder().act(|| {
            self.folder().update_lock(|lock| {
                if let Some(key) = key {
                    lock.pending.remove(key);
                }
                settle(lock);
                let given = if version.number == 0 { lock.number(name, kind) } else { version.number };
                number.set(given);
                let held = lock.asset_mut(name, kind);
                held.numbered = held.numbered.max(given);
                let mut listed = version.clone();
                listed.number = given;
                held.list(listed);
                held.tried = None;
                held.failed = None;
            })?;
            if approve && let Some(sha256) = version.main() {
                self.folder().edit(|written| written.selected(name, number.get(), sha256))?;
            }
            Ok(())
        })?;
        version.number = number.get();
        {
            let mut tally = self.tally.lock().expect("tally lock");
            match noun {
                "take" => tally.takes += 1,
                _ => tally.versions += 1,
            }
            tally.execution_ms += version.execution_ms;
        }
        self.landed_unseen(name, number.get(), approve);
        self.say_landed(name, &version, noun, number.get())
    }

    /// A version that lands beside an approved one, which the box goes on showing, is unseen until it's looked at (cl-928).
    fn landed_unseen(&self, name: &str, number: u32, approve: bool) {
        if !approve {
            unseen::landed(&self.project, name, number);
        }
    }

    fn say_landed(&self, name: &str, version: &Version, noun: &str, number: u32) -> Result<(), Error> {
        let lock = self.folder().lock()?;
        let path = main_path(&lock, &version.files).unwrap_or_default();
        let execution_ms = (version.execution_ms > 0).then_some(version.execution_ms);
        self.say(Told::Landed { name: name.into(), noun: noun.into(), number, path, execution_ms });
        if let Some(pickup) = &version.pickup {
            self.say(Told::Said { message: pickup.said(name, number), name: Some(name.to_owned()) });
            self.project.jobs.why(name, number, format!("picks up {} at frame {}", pickup.take, pickup.out_frame));
        }
        Ok(())
    }
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
fn outdated_now(doc: &Doc, stand: &[Standing], ask: &Ask, i: usize, lock: &Lock) -> bool {
    ask.outdated && judged_pin(doc, stand, ask, i) && pin_changed(stand, i) && doc.tags(i).is_none_or(|tags| doc.missing_names(i, tags, lock).is_empty())
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

/// Why asset `i` can't read what it reads: the first read with no state.
fn unread(doc: &Doc, stand: &[Standing], lock: &Lock, ask: &Ask, i: usize) -> String {
    let Some(&j) = doc.upstream[i].iter().find(|&&j| stand[j].files.is_none()) else { return "what it reads has no state".into() };
    let name = &doc.assets[j].name;
    match &stand[j].state {
        _ if ask.sent.contains(&name_key(name)) => format!("{name} failed in this render"),
        _ if doc.assets[j].hollow => format!("{name} has nothing to make"),
        Stand::Stale(why) => format!("{name} is out of date: {why}"),
        _ if lock.asset(name).is_some_and(|held| !held.versions.is_empty()) => format!("{name} has no version with these inputs"),
        _ => format!("{name} has no version"),
    }
}

/// Whether `key` is one `new_key` makes: what a take's id is, whoever made it.
fn is_key(key: &str) -> bool {
    key.strip_prefix("project-").is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
}

/// Every take id the project holds: asked, sent, and landed as a version.
fn takes_held(lock: &Lock) -> HashSet<&str> {
    let asked = lock.walk.asks.iter().flat_map(|ask| &ask.takes).map(|take| take.key.as_str());
    let sent = lock.pending.iter().map(|(key, pending)| pending.job_of(key));
    let landed = lock.assets.values().flat_map(|held| held.on_record()).map(|version| version.id.as_str());
    asked.chain(sent).chain(landed).filter(|id| !id.is_empty()).collect()
}

/// A fresh idempotency key: one key names one run forever.
fn new_key() -> String {
    let mut key = [0u8; 16];
    getrandom::fill(&mut key).expect("the OS has randomness");
    format!("project-{}", hex::encode(key))
}

/// This Calcifer's id, from its home's `dir`, which binds what a walk owes to it. Before any project is read.
pub(crate) fn bind_calcifer(dir: &FsPath) -> Result<(), Error> {
    folder::bind_calcifer(dir)
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
    doc.find(name).ok_or_else(|| {
        Error::new(Kind::NotFound, "asset_unknown", format!("the project has nothing named {name}")).next("cozy status")
    })
}

/// Of `on`, where a job goes (cl-816): the soonest finish its machines quoted (`offers`), a rental before this computer on a tie, and not
/// this computer when it would run `function` poorly (`fit`) and no quote of its own runs says it ends it first (Paul, 11-05: a rented
/// GPU is for using), nor when its GPU draws the screen, which a render makes lag (cl-897). With no quote that fits, the first of `on`
/// that can take it (`placement::takes`).
#[allow(clippy::too_many_arguments)]
async fn pick(api: &Arc<Api>, on: &[On], offers: &[Offer], planned: Option<&Offer>, hub: &str, package: Option<&Package>, function: &str, made: &Map<String, Value>) -> Result<(On, Option<Offer>), Error> {
    let now = now_ms();
    let Some(soonest) = planned.filter(|offer| offer.fits).or_else(|| placement::soonest(offers, now)) else {
        return Ok((placement::takes(api, on, offers).await?, None));
    };
    let poorly = soonest.machine.is_local()
        && (fit::draws_screen(api) || (soonest.basis != v1::Basis::Measured && package.is_some_and(|package| fit::poorly_now(api, hub, package, function, made))));
    let others: Vec<Offer> = offers.iter().filter(|offer| !offer.machine.is_local()).cloned().collect();
    let chosen = match poorly {
        true => placement::soonest(&others, now).unwrap_or(soonest),
        false => soonest,
    };
    Ok((chosen.machine.clone(), Some(chosen.clone())))
}

/// Where `cozy run` of `function` goes among `set` (Paul, 11-06): as a render's job does (`pick`), each machine asked once, for `count`
/// copies of it as if sent at once. Also each copy's offers, and a line for each machine that didn't answer.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_on(api: &Arc<Api>, set: &[On], hub: &str, package: &str, release: &str, function: &str, size: v1::JobSize, gpus: u32, gpu: bool, count: u32, update: bool) -> (Result<On, Error>, Vec<Vec<Offer>>, Vec<String>) {
    let host = Host::new(api);
    let read = host.published(hub, &format!("{package}@{release}"), set.first()).await.ok();
    let models = match &read {
        Some(read) => fit::checkpoints(api, hub, read, function, &Map::new()).await,
        None => vec![],
    };
    let named = format!("{package}/{function}@{release}");
    let jobs: Vec<placement::Job> = (0..count.max(1))
        .map(|copy| placement::Job { key: copy.to_string(), target: named.clone(), function: named.clone(), models: models.clone(), size, gpus, gpu, on: set.to_vec() })
        .collect();
    let (offers, said) = placement::offers(api, &jobs, update).await;
    let chosen = pick(api, set, offers.first().map_or(&[][..], Vec::as_slice), None, hub, read.as_ref(), function, &Map::new()).await;
    (chosen.map(|(chosen, _)| chosen), offers, said)
}

/// The machines the project `named` pins its runs to (cl-873), those that ended left out (cl-920); none: wherever they finish soonest.
/// With every one of them gone now (ending, or offline by its Hub's word), wherever they finish soonest too.
pub(crate) fn machines_of(api: &Arc<Api>, named: &str) -> Result<Vec<On>, Error> {
    let project = api.projects.get(api, named).map_err(|Refusal(e)| e)?;
    let pinned = chosen(api, &project.folder);
    match pinned.iter().all(|name| api.machines.gone(name).is_some()) {
        true => Ok(vec![]),
        false => Ok(pinned.into_iter().map(On::Machine).collect()),
    }
}

/// A project's machines (cl-873) without those that ended or are no machines here (cl-920): a machine that ends leaves every project's
/// list, kept so at its next read.
pub(crate) fn chosen(api: &Api, folder: &Folder) -> Vec<String> {
    let kept = folder.machines();
    let live: Vec<String> = kept.iter().filter(|name| !api.machines.ended_here(name)).cloned().collect();
    if live.len() != kept.len()
        && let Err(e) = folder.set_machines(&live)
    {
        warn!("{}: leaving its ended machines out: {}", folder.dir.display(), e.message);
    }
    live
}

/// Where a file stands in its list: its place in the innermost list holding it, as Runtime orders it (`references.1.file` is 1).
pub(crate) fn order_of(field: &str) -> u32 {
    field.rsplit('.').find_map(|part| part.parse().ok()).unwrap_or(0)
}

fn media_type(path: &FsPath) -> &'static str {
    let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "otio" => "application/json",
        extension => folder::media_of(extension).map_or("application/octet-stream", |(media, _)| media),
    }
}
