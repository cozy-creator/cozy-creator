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
mod benchmarks;
pub(crate) mod fit;
mod folder;
mod format;
mod guess;
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
pub(crate) mod key;
pub(crate) use journal::{OpenProject, ProjectRef, ProjectStore, RunOwner};
mod keyed;
mod view;
pub(crate) mod model;
mod nodes;
pub(crate) mod numbers;
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
        Approve, Approved, Asked, Asset as Row, Cancel, Cancelled, Cut, File as Listing, Import, Imported, Job, JobState, Known, Machines as ProjectMachines, AppearanceKept as ProjectAppearance, Moved as ProjectMoved, Name as ProjectName, MainFile, Open, Opened,
        NoGpu, NoRoom, Orphan, Projects as Listed, Queued, QueuedNow, Reclaim, Reclaimed, Rename, Renamed, Rendered, Runs, Said, Standing as RowStanding, State as ProjectState, Unpin, Unpinned,
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
/// stream, its walk (which goes until nothing is owed), a page showing it, and a plaintext one `LINGER` after a request. The last of them
/// to let go closes it (cl-823).
#[derive(Default)]
pub(crate) struct Projects {
    open: Mutex<HashMap<String, Weak<Project>>>,
    /// Plaintext projects a request used, held until `LINGER` after the last, by id, with their project.db's file as it was then.
    lingering: Mutex<HashMap<String, Lingering>>,
    /// What was last heard of each published package projects name, by `with`.
    heard: Mutex<HashMap<String, Heard>>,
    /// How many times that changed: what a read said of them is good until then.
    heard_changes: AtomicU64,
    /// Held while a project is made: two first uses of one folder (two `cozy new`s, a page and the CLI) make one.
    making: Mutex<()>,
    requests:Mutex<HashMap<PathBuf,usize>>,
    /// Run submissions being answered, apart from `requests`: one waits for its run's machine, which a vault's maximum doesn't (cl-1082).
    submits:Mutex<HashMap<PathBuf,usize>>,
}

pub(super) struct RequestActivity {api:Arc<Api>,root:PathBuf,submit:bool}
impl Drop for RequestActivity {
    fn drop(&mut self) {
        let projects=&self.api.projects;
        let mut requests=if self.submit { projects.submits.lock() } else { projects.requests.lock() }.expect("project requests");
        let Some(count)=requests.get_mut(&self.root) else { return };
        *count-=1;
        if *count==0 { requests.remove(&self.root); drop(requests); vault::work_ended(&self.root); }
    }
}
impl Projects {
    pub(crate) fn active_requests(&self,root:&FsPath)->bool {
        self.requests.lock().expect("project requests").get(&vault::logical(root)).is_some_and(|count|*count>0)
    }
    /// Whether a run submission to the project is being answered (`track_submit`).
    pub(crate) fn submitting(&self,root:&FsPath)->bool {
        self.submits.lock().expect("project requests").get(&vault::logical(root)).is_some_and(|count|*count>0)
    }
    fn track_request(&self,api:&Arc<Api>,root:&FsPath)->RequestActivity {
        let root=vault::logical(root);
        *self.requests.lock().expect("project requests").entry(root.clone()).or_default()+=1;
        RequestActivity {api:api.clone(),root,submit:false}
    }
    /// A run submission being answered: it may wait for its run's machine, so it's no request a vault's lock waits for (cl-1082).
    fn track_submit(&self,api:&Arc<Api>,root:&FsPath)->RequestActivity {
        let root=vault::logical(root);
        *self.submits.lock().expect("project requests").entry(root.clone()).or_default()+=1;
        RequestActivity {api:api.clone(),root,submit:true}
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
    /// What its runs are named by, `KEY-N` (cl-987).
    key: Mutex<String>,
    /// Immutable owner. A recycled physical mount path must never choose another project's root.
    root: PathBuf,
    /// Cache ownership follows the physical mount, not a replaceable viewer authorization.
    mount_id: Option<vault::MountIdentity>,
    folder: Folder,
    /// Which file its project.db was as it opened (`db_file`): another there is another project's, or a restored copy, never this one.
    db_file: Option<(u64, u64)>,
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
    /// Whether its files are being measured (`measure_soon`), and if so whether more landed meanwhile, measured once it ends.
    measuring: Mutex<Option<bool>>,
    /// The installs its walk's jobs wait on (`host::Installs`).
    installs: Arc<host::Installs>,
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
    /// What its boxes would run is guessed while it's open (`guess::keep`), since a page first showed it.
    guessing: std::sync::atomic::AtomicBool,
    /// Each box's seed drawn for its draft (cl-1123), by name key: the one its predicted request carries, which its next render takes.
    drafts: Mutex<HashMap<String, u64>>,
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
        guess::closed();
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
        .route("/v1/projects/{id}/runs/{run}/retry", post(retry))
        .route("/v1/projects/{id}/runs/{run}/events", get(crate::api::events))
        .route("/v1/projects/{id}/runs/{run}/outputs/{name}", get(crate::outputs::serve))
        .route("/v1/projects/{id}/runs/{run}/play", get(crate::play::play))
        .route("/v1/projects/{id}/runs/{run}/preview", get(crate::previews::serve))
        .route("/v1/projects/{id}/fit", post(fit::fit))
        .route("/v1/projects/{id}/benchmarks", get(benchmarks::benchmarks))
        .route("/v1/projects/{id}/cancel", post(cancel))
        .route("/v1/projects/{id}/approve", post(approve))
        .route("/v1/projects/{id}/unpin", post(unpin))
        .route("/v1/projects/{id}/rename", post(rename))
        .route("/v1/projects/{id}/machines", put(machines))
        .route("/v1/projects/{id}/appearance", put(appearance))
        .route("/v1/projects/{id}/reclaim", post(reclaim))
        .route("/v1/projects/{id}/assets", post(assets::create))
        .route("/v1/projects/{id}/copies", post(assets::copy))
        .route("/v1/projects/{id}/assets/{name}", get(assets::read).patch(assets::patch).delete(assets::delete))
        .route("/v1/projects/{id}/read", post(assets::read_many))
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
        // A project's folder renamed or trashed (cl-1097): outside the layers, as nothing of it may stay open, and a locked one moves too.
        .route("/v1/projects/{id}/name", put(name))
        .route("/v1/projects/{id}", delete(delete_project))
        .route("/v1/projects/{id}/key", get(key::read).put(key::change))
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

    /// The machine `old` named has its Hub's name now, a rental ordered under its handle (cl-1106): each open project that chose it
    /// chooses it by that name, its renders asked of it go there, and every page showing one reads it again. A closed project takes the
    /// name at its next read (`chosen`) and its next walk (`renders_follow`).
    pub(crate) fn machine_renamed(&self, api: &Api, old: &str) {
        for project in self.now_open() {
            let folder = &project.folder;
            let chose = folder.machines().iter().any(|name| name.eq_ignore_ascii_case(old));
            if chose {
                chosen(api, folder);
            }
            let asked = renders_follow(api, folder).unwrap_or_else(|e| {
                warn!("{}: its renders asked of {old} keep that name: {}", folder.dir.display(), e.message);
                false
            });
            if chose || asked {
                crate::events::project_settled(&folder.dir);
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
    /// else opened from the index. It stays open while the answer is held, and a plaintext one `LINGER` after.
    fn get(&self, api: &Arc<Api>, named: &str) -> Result<Arc<Project>, Refusal> {
        let project = self.found(api, named)?;
        self.linger(api, &project);
        Ok(project)
    }

    fn found(&self, api: &Arc<Api>, named: &str) -> Result<Arc<Project>, Refusal> {
        if let Some(project) = self.open_now(named) {
            return Ok(project);
        }
        if api::target::written_as_path(named.trim()) {
            return self.open(api, &self::named(api, named).map_err(Refusal)?).map_err(Refusal);
        }
        let entry = known(api, named.trim()).map_err(Refusal)?;
        let project = self.open(api, &entry.path).map_err(Refusal)?;
        if project.id != entry.id {
            let said = format!("{named} was in {}, but that folder holds a different project now.", entry.path.display());
            return Err(Refusal(Error::new(Kind::NotFound, "project_unknown", said).next("cozy open")));
        }
        // Opened for a call: used now, which the index lists by.
        api.calcifer.store().project_opened(&project.id, &identity(&project.folder.dir).1, &project.root, Some(now_ms()), Some(&project.code))?;
        used_now(api, &project.id);
        Ok(project)
    }

    /// A project lingering in `dir` whose project.db isn't the file it was (its folder deleted and made again), let go: what opens there
    /// now reads the new file, never the old one's connections and document a lingering project would keep shared.
    fn let_go_replaced(&self, dir: &FsPath) {
        let now = db_file(dir);
        let mut lingering = self.lingering.lock().expect("lingering lock");
        let stale: Vec<String> = lingering.iter().filter(|(_, held)| held.project.folder.dir == dir && held.project.db_file != now).map(|(id, _)| id.clone()).collect();
        let gone: Vec<Lingering> = stale.iter().filter_map(|id| lingering.remove(id)).collect();
        drop(lingering);
        drop(gone);
    }

    /// Every project held after its last request let go, its connections closed and its WAL emptied first: Calcifer stops. Each waits
    /// for its database's uses in flight to end, up to the stop's deadline `until`, then is left with its WAL.
    pub(crate) fn close_lingering(&self, until: Instant) {
        let lingering: Vec<Lingering> = self.lingering.lock().expect("lingering lock").drain().map(|(_, held)| held).collect();
        for held in lingering {
            if let Err(error) = held.project.folder.close_connections(true, until) {
                warn!(code = %error.code, "a project held after its request didn't close as Calcifer stopped");
            }
        }
    }

    /// `project`, a plaintext one a request used, held open until `LINGER` after this, its last use: one task a project lets it go.
    fn linger(&self, api: &Arc<Api>, project: &Arc<Project>) {
        if vault::encrypted(&project.root) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let held = Lingering { project: project.clone(), until: Instant::now() + LINGER };
        let fresh = self.lingering.lock().expect("lingering lock").insert(project.id.clone(), held).is_none();
        if !fresh {
            return;
        }
        let (api, id) = (api.clone(), project.id.clone());
        runtime.spawn(async move {
            loop {
                let Some(until) = api.projects.lingering.lock().expect("lingering lock").get(&id).map(|held| held.until) else { return };
                tokio::time::sleep_until(until.into()).await;
                let mut lingering = api.projects.lingering.lock().expect("lingering lock");
                if lingering.get(&id).is_some_and(|held| held.until <= Instant::now()) {
                    let gone = lingering.remove(&id);
                    drop(lingering);
                    drop(gone);
                    return;
                }
            }
        });
    }

    /// Project `id` held open by a stream that shows it, until the stream lets it go.
    pub(crate) fn reader(&self, api: &Arc<Api>, id: &str) -> Result<Reader, Refusal> {
        let project = self.get(api, id)?;
        // A page shows it: what its boxes would run is guessed, and their machines made ready, while it stays open (cl-1104).
        guess::keep(api, &project);
        Ok(Reader(project))
    }

    /// The project in `dir` (canonical), opened if it isn't, given its id and name and recorded in the index; its walk goes on when
    /// one is owed. It stays open while the answer is held.
    fn open(&self, api: &Arc<Api>, dir: &FsPath) -> Result<Arc<Project>, Error> {
        let root = vault::logical(dir);
        let mount_id = vault::mount_identity(&root);
        let physical = current_view(&root)?;
        let dir = physical.as_path();
        self.let_go_replaced(dir);
        let (folder, Identified { id, name, code, key }) = tokio::task::block_in_place(|| Folder::open_with(dir, api.machines.hubs.home(), |folder| {
            let identified = identify_loaded(api, dir, folder)?;
            let id = &identified.id;
            // Validate a move before inherited local work can be unbound or resumed. A
            // rendering project keeps its original physical folder until its runs land.
            let moved = self.open.lock().expect("projects lock").get(id).and_then(Weak::upgrade)
                .filter(|open| open.root != root || open.mount_id != mount_id || open.folder.dir != dir);
            if let Some(open) = moved {
                if *open.walking.lock().expect("walking lock") {
                    let said = format!("Project {id} was moved out of {} while it was rendering. Move it back, or wait until its renders finish before moving it.", open.folder.dir.display());
                    return Err(Error::new(Kind::Conflict, "project_busy", said));
                }
                self.open.lock().expect("projects lock").remove(id);
            }
            Ok(identified)
        }))?;
        IDENTITIES.lock().expect("identities lock").insert(dir.to_owned(), (id.clone(), name));
        crate::events::project_opened(dir);
        let mut open = self.open.lock().expect("projects lock");
        let file = db_file(dir);
        let project = match open.get(&id).and_then(Weak::upgrade)
            .filter(|project| project.root == root && project.mount_id == mount_id && project.folder.dir == dir && project.db_file == file) {
            Some(project) => {
                *project.key.lock().expect("key lock") = key;
                project
            }
            None => {
                let unseen=unseen::read(&folder,&id)?;
                let watch = (!vault::encrypted(&root)).then(|| watch(&id, dir)).flatten();
                let project = Arc::new(Project {
                    id: id.clone(),
                    code,
                    key: Mutex::new(key),
                    root,
                    mount_id,
                    folder,
                    db_file: file,
                    jobs: Jobs::new(id.clone()),
                    model: tokio::sync::Mutex::default(),
                    told: Mutex::default(),
                    walker: Mutex::default(),
                    walking: Mutex::new(false),
                    measuring: Mutex::new(None),
                    installs: Arc::default(),
                    asked: Notify::new(),
                    cutting: Arc::default(),
                    rendering: Arc::default(),
                    moving: tokio::sync::Mutex::default(),
                    said: Mutex::default(),
                    unsent: Mutex::default(),
                    unseen: Mutex::new(unseen),
                    _watch: watch,
                    guessing: Default::default(),
                    drafts: Mutex::default(),
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
        // Its deleted boxes looked at as it opens, if it has any.
        purge::opened(&project.id, &project.folder);
        // What it shows of the old landing folder, if that waits for it to be read (cl-1036), held first.
        crate::old_outputs::opened(&api.machines.hubs.home().calcifer(), &project.id, &project.root);
        // What the legacy cut held for it in this computer's TensorFS store, copied in now it can be read (cl-1003).
        if crate::store::kept_path(&api.machines.hubs.home().calcifer(), &project.root).exists() {
            match keep_left_on_local(&project.root, &api.machines.hubs.home().calcifer()) {
                Ok(Some(0)) | Ok(None) => {}
                Ok(Some(gone)) => { let _ = api.calcifer.store().note_lost(&project.root, gone as u64); }
                // Its cause and nothing of the project outside it (an encrypted project's leave no trace there); said on its page too.
                Err(cause) => {
                    tracing::warn!(target: "cozy.service_health", code = "legacy_copy_failed", cause, "a project's old files couldn't be copied into it; they stay held until its next open");
                    project.said.lock().expect("said lock").push(format!("this project's old files kept on this computer couldn't be copied into it ({cause}): they stay kept, and its next open copies them"));
                }
            }
        }
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

/// How long a plaintext project a request opened stays open after it: asked again meanwhile (a page's next read, a client's next
/// call), it isn't loaded again (cl-1010: each run list of a 3,000-box project loaded its whole document, 50–60 ms, 200 four at once).
/// An encrypted one closes with its request, as every project did.
const LINGER: std::time::Duration = std::time::Duration::from_secs(2);

/// A project held open after a request (`Projects::linger`).
struct Lingering {
    project: Arc<Project>,
    until: Instant,
}

/// Which file `dir`'s project.db is: its device and inode. Elsewhere none: Windows can't delete a database a process has open.
fn db_file(dir: &FsPath) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(dir.join("project.db")).ok().map(|meta| (meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

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
            let said = format!("{text} is a relative path, and Cozy can't tell which folder it starts from.");
            return Err(Error::new(Kind::Usage, "project_path", said).remedy("Use the project's name, id or code, or the absolute path of its folder."));
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

/// Whether a reader of project `id` hears its names now (runs, boxes, local packages): one this computer knows, never a locked one.
/// The one test for its events and for machines' pictures (cl-1230).
pub(crate) fn readable(api: &Api, id: &str) -> bool {
    api.calcifer.store().known_project(id).ok().flatten().is_some_and(|known| vault::require_path(&known.path).is_ok())
}

/// The index, last used first: each project opened here whose folder still holds it. One whose folder is gone (deleted, moved, on a
/// drive that isn't there) leaves the index as it's found so, and its name is free. Its `project.db` keeps its id and name: opened
/// again by its path it is back as it was, or as `<name>_2` if another took the name meanwhile.
/// Where project `id` was, if its folder is gone: it leaves the index as `indexed` lets it go. None while it's there, or can't be told.
pub(crate) fn gone(api: &Api, id: &str) -> Option<PathBuf> {
    match api.calcifer.store().known_project(id) {
        Ok(Some(entry)) if !vault::keeps_index(&entry.path) => let_go(api, std::slice::from_ref(&entry.id)).ok().map(|()| entry.path),
        _ => None,
    }
}

/// Projects whose folders are gone (deleted, moved, on a drive that isn't there) leave this computer (cl-1031): Calcifer keeps only
/// where each was, its runs' routes go, and the local machine retires every record of it, now or, when it isn't running, as it
/// starts. Everything of value is in the folder: opened again from it, a project registers again from its own project.db.
fn let_go(api: &Api, ids: &[String]) -> Result<(), Error> {
    api.calcifer.store().projects_gone(ids).map_err(|e| Error::internal("calcifer_store", e.to_string()))?;
    for id in ids { api.calcifer.retire_project(id); }
    if let (Ok(Some(link)), Ok(runtime)) = (api.calcifer.link("local"), tokio::runtime::Handle::try_current()) {
        let ids = ids.to_vec();
        runtime.spawn(async move {
            for id in ids {
                if let Err(error) = link.project_gone(&id).await { warn!("a project whose folder is gone keeps its records on the local machine: {error}"); }
            }
        });
    }
    Ok(())
}

fn indexed(api: &Api) -> Result<Vec<crate::state::KnownProject>, Error> {
    let store = api.calcifer.store();
    let failed = |e: crate::store::StoreError| Error::internal("calcifer_store", e.to_string());
    let known = store.known_projects().map_err(failed)?;
    for entry in &known { vault::recover_conversion(&entry.path)?; }
    let (mut here, gone): (Vec<_>, Vec<_>) = known.into_iter().partition(|entry| vault::keeps_index(&entry.path));
    if !gone.is_empty() {
        let_go(api, &gone.into_iter().map(|entry| entry.id).collect::<Vec<_>>())?;
    }
    // An earlier Calcifer may have opened one of its own folders: its row stays, unlisted and unopened.
    here.retain(|entry| !own_folder(&entry.path));
    // Every project has a code, its only address (cl-1171): a plain one with none in the index gets one now; an encrypted one's is its
    // keybag's (`as_named`).
    for entry in here.iter_mut().filter(|entry| entry.code.is_empty() && !vault::encrypted(&entry.path)) {
        entry.code = backfill_code(store, entry)?;
    }
    Ok(here)
}

/// A plain project's code, for an index from before codes: its `project.db`'s when that's free here, else a new one, kept in the index and,
/// when it opens, in its `project.db`. Even one that can't be opened has one: its address says why.
fn backfill_code(store: &crate::store::Store, entry: &crate::state::KnownProject) -> Result<String, Error> {
    let path = entry.path.join(db::DB);
    let conn = path.is_file().then(|| db::open(&path).ok()).flatten()
        .filter(|conn| db::identity(conn).is_some_and(|(id, _)| id == entry.id));
    let kept = conn.as_ref().and_then(|conn| db::meta(conn, "code"));
    let code = match reserve_code(store, &entry.id, &entry.name, &entry.path, kept.as_deref()) {
        Err(error) if error.code == "project_code_conflict" => reserve_code(store, &entry.id, &entry.name, &entry.path, None)?,
        reserved => reserved?,
    };
    if let Some(conn) = conn.filter(|_| kept.as_deref() != Some(&code)) {
        if let Err(error) = db::set_meta(&conn, &path, "code", Some(&code)) { warn!("{}: its code is kept at its next open: {}", path.display(), error.message); }
    }
    Ok(code)
}

/// A locked encrypted project as the index's row for it says it outside Calcifer: by its keybag's id and code, never the row's
/// `locked-` handle, which names only its folder here (cl-984, cl-1171). One whose keybag can't be read is named by its handle, and its
/// address says why it can't open. Any other row as it is.
fn as_named(entry: crate::state::KnownProject) -> crate::state::KnownProject {
    if !(entry.id.starts_with(LOCKED) || entry.code.is_empty() && vault::encrypted(&entry.path)) { return entry; }
    match vault::kept_in_keybag(&entry.path) {
        Some((id, code)) if entry.id.starts_with(LOCKED) => crate::state::KnownProject { id, code, ..entry },
        Some((_, code)) => crate::state::KnownProject { code, ..entry },
        None => crate::state::KnownProject { code: code::derived(&entry.id), ..entry },
    }
}

/// What the index's row of a locked encrypted project is keyed by (`Store::project_locked`).
const LOCKED: &str = "locked-";

/// The project the index knows by `text`, its id, public code or its name (in any case). A locked one by its keybag's (`as_named`).
fn known(api: &Api, text: &str) -> Result<crate::state::KnownProject, Error> {
    let mut indexed: Vec<_> = indexed(api)?.into_iter().map(as_named).collect();
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
        // One whose folder went is known by its folder alone (cl-1031): opened from it again, it is back as it was.
        0 => Err(Error::new(Kind::NotFound, "project_unknown", format!("There's no project called {text} on this computer."))
            .remedy("If its folder moved or its drive was disconnected, open the folder again where it is now.")
            .next("cozy open <folder>")),
        1 => Ok(named.remove(0)),
        _ => {
            let ids: Vec<String> = named.iter().map(|entry| format!("{} ({})", entry.id, entry.path.display())).collect();
            let said = format!("{} projects are called {text}: {}.", ids.len(), ids.join(", "));
            Err(Error::new(Kind::Conflict, "project_ambiguous", said).remedy("Name the one you mean by its id."))
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

/// A message set inside another sentence: without its closing period.
pub(crate) fn inner(said: &str) -> &str {
    said.trim_end_matches('.')
}

pub(crate) fn keep_given(project: &FsPath, given: &[(PathBuf, String)], uploads: &FsPath) -> Result<Vec<String>, Error> {
    let physical = vault::resolve_path(project)?;
    let folder = Folder::at(&physical);
    let unkept = given.iter().filter_map(|(path, sha256)| {
        let why = folder.keep_given(path, sha256, path.starts_with(uploads)).err()?;
        Some(format!("Cozy couldn't keep a copy of {} in the project ({}), so to run this again you'll need to give the file again.", path.display(), inner(&why.message)))
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
                    fetched.map_err(|why| Error::new(Kind::Unavailable, "file_on_a_machine", format!("{written} is on {}, and Cozy couldn't download it: {}.", file.machine, inner(&why.to_string()))))?;
                }
                Ok(Some(at))
            }
            several => Err(Error::new(Kind::Usage, "file_ambiguous", format!("{written} matches {} files in this project. Type more of its hash.", several.len()))),
        }
    }

    /// `cozy run`'s written fields bound as Render binds a box's: mentions read this project's boxes, then the run's own references
    /// (`inline`, each with its file here), ranges are cut here, and `pickup.` terms are the gesture. Answers the call, the files it
    /// names, each where the call reads it (a box's `video.file`, cl-1183): the run's own (`given`, as attached), the project's (fetched
    /// here if a machine keeps them); what was said, and what it left out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn bound(&self, target: &str, package: &str, title: &str, callable: &api::interface::Callable, written: Map<String, Value>, clips: &crate::runs::clip::Clips, inline: &[(binder::Inline, PathBuf)], given: &[crate::runs::bind::InputFile]) -> Result<(Map<String, Value>, Vec<crate::runs::bind::InputFile>, Vec<String>, Vec<Unused>), Error> {
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
            Some(value) => Some(serde_json::from_value::<api::calcifer::project::Pickup>(value).map_err(|why| Error::new(Kind::Usage, "pickup_malformed", format!("Couldn't read the pickup ({why}). Write it as pickup.take=<box.N> and pickup.out_frame=<frame>.")))?),
        };
        let reserved = callable.tool.as_ref().map(|tool| tool.reserved.clone()).unwrap_or_default();
        doc.worded(&mut fields, &reserved);
        let own: Vec<binder::Inline> = inline.iter().map(|(named, _)| named.clone()).collect();
        let call = binder::Call { name: target, tool: target, package, title, callable, fields: &fields, reserved: &reserved, doc: &doc, own: None, lock: &lock, stand: &stand, gone: &BTreeSet::new(), pickup: pickup.as_ref(), inline: &own };
        let mut bound = binder::bind(&call)?;
        // The run's own files go as a cut clip does: by their digest, from where they are here.
        let mut cut: HashMap<String, PathBuf> = inline.iter().map(|(named, path)| (named.file.clone(), path.clone())).collect();
        cut.extend(given.iter().map(|file| (file.digest.to_string(), file.path.clone())));
        let mut digests = vec![];
        for range in &bound.cuts {
            let Some(path) = self.file(&range.source).await? else { continue };
            let fps = lock.files.get(&range.source).map_or(0.0, |file| file.fps);
            if fps <= 0.0 {
                return Err(Error::new(Kind::Unavailable, "range_unmeasured", format!("Cozy can't cut a range of {} because its frame rate hasn't been measured yet.", folder::prefix_of(&range.source))).next("cozy status"));
            }
            let mut notes = vec![];
            let ffmpeg = clips.ffmpeg().await?;
            let access = vault::admit_root(&self.dir).await?;
            let made = clips.cut(&range.range.to_string(), &path, range.range, fps, &mut notes, ffmpeg, access).await?;
            bound.warnings.extend(notes.into_iter().filter(|note| note.level == api::calcifer::NoteLevel::Warning).map(|note| note.message));
            let clip = made.clip.clone();
            let access = vault::admit_root(&self.dir).await?;
            let sha256 = tokio::task::spawn_blocking(move || {
                // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
                let _access = access.here();
                folder::hash(&clip)
            }).await.map_err(|e| Error::new(Kind::Internal, "clip_hash", e.to_string()))??;
            cut.insert(sha256.clone(), made.clip);
            digests.push(sha256);
        }
        let mut payload = Value::Object(std::mem::take(&mut bound.payload));
        binder::cuts_filled(&mut payload, &digests);
        let mut files = vec![];
        for (field, _, sha256) in tools::files_in(&payload, &folder, &lock, &cut) {
            if let Some(file) = given.iter().find(|file| file.digest.to_string() == sha256) {
                files.push(crate::runs::bind::InputFile { order: order_of(&field), field, ..file.clone() });
                continue;
            }
            let path = match cut.get(&sha256) {
                Some(clip) => clip.clone(),
                None => self.file(&sha256).await?.unwrap_or_else(|| folder.path(&lock.files[&sha256].path)),
            };
            let sniffed = crate::runs::bind::sniff(&path)?;
            let digest = sha256.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?;
            files.push(crate::runs::bind::InputFile { order: order_of(&field), shown: sha256.clone(), field, path, digest, length: sniffed.length, media_type: sniffed.media_type });
        }
        files.sort_by(|a, b| a.field.cmp(&b.field));
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
    let take = folder.take_run(made.run_id);
    let (kept, unlisted) = folder.adopt(dest, made.digest, row, made.measured, take).map_err(|e| e.message)?;
    // The file is in `files/` now. An output that grows reads its next revision whole, with nothing here to extend.
    let _ = std::fs::remove_file(dest);
    let said = unlisted.map(|why| format!("{} is saved in files/, but Cozy couldn't add it to the project: {}.", made.output, inner(&why.message)));
    Ok((kept, said))
}

/// An output its machine keeps, in its project's lock.
pub(crate) fn stays(project: &FsPath, sha256: &str, machine: &str, length: u64) -> Result<(), String> {
    let physical = vault::resolve_path(project).map_err(|error| error.message)?;
    Folder::at(&physical).stays(sha256, machine, length).map_err(|e| e.message)
}

/// The files the lock of the project at `root` says stay on `local` and its folder lacks (cl-1003): what the legacy cut holds in this
/// computer's TensorFS store for it before any copy lets go of anything. None when its lock can't be read here.
pub(crate) fn left_on_local(root: &FsPath) -> Option<Vec<tensorfs::format::ObjectRef>> {
    let physical = vault::resolve_path(root).ok()?;
    let folder = Folder::at(&physical);
    let lock = folder.lock().ok()?;
    Some(lock.files.values().filter(|file| file.machine == "local" && !folder.path(&file.path).is_file())
        .filter_map(|file| Some(tensorfs::format::ObjectRef { sha256: file.sha256.strip_prefix("sha256:")?.to_owned(), length: file.length }))
        .collect())
}

/// What the project at `root` shows that only the old landing folder holds (`outputs`: `~/.cozy/outputs`, then where it is set aside;
/// cl-1036, `old_outputs`): each file of its `files/` that links into that folder (an older cozy's media link), and each file its lock
/// names that `files/` lacks and whose bytes are there (found by length, then hash). The files there that hold them: none for a
/// folder no longer there (deleted, or moved to a place not known), which shows nothing; not known when it can't be read now (locked,
/// or its lock unreadable), when nothing it shows can be proven not to be there.
pub(crate) fn kept_in_outputs(root: &FsPath, outputs: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let landing = outputs.first()?;
    if !outputs.iter().any(|dir| dir.is_dir()) || !root.is_dir() { return Some(vec![]); }
    if vault::locked_now(root) { return None; }
    let physical = vault::resolve_path(root).ok()?;
    let files = physical.join(folder::FILES);
    let canonical = std::fs::canonicalize(landing).ok();
    let mut found = vec![];
    for entry in std::fs::read_dir(&files).into_iter().flatten().flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else { continue };
        let target = if target.is_absolute() { target } else { files.join(target) };
        let tail = target.strip_prefix(landing).ok().map(FsPath::to_path_buf)
            .or_else(|| canonical.as_ref().and_then(|landing| target.strip_prefix(landing).ok().map(FsPath::to_path_buf)));
        if let Some(source) = tail.and_then(|tail| outputs.iter().map(|dir| dir.join(&tail)).find(|source| source.is_file())) { found.push(source); }
    }
    let folder = Folder::at(&physical);
    let lock = folder.lock().ok()?;
    let mut missing: HashMap<u64, Vec<String>> = HashMap::new();
    for file in lock.files.values().filter(|file| file.machine.is_empty() && std::fs::symlink_metadata(folder.path(&file.path)).is_err()) {
        if let Some(hex) = file.sha256.strip_prefix("sha256:") { missing.entry(file.length).or_default().push(hex.to_owned()); }
    }
    let mut dirs: Vec<PathBuf> = if missing.is_empty() { vec![] } else { outputs.iter().filter(|dir| dir.is_dir()).cloned().collect() };
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() { dirs.push(entry.path()); continue; }
            if !kind.is_file() { continue; }
            let Some(wanted) = entry.metadata().ok().and_then(|meta| missing.get_mut(&meta.len())) else { continue };
            let Ok(hex) = folder::hash(&entry.path()) else { continue };
            let hex = hex.trim_start_matches("sha256:").to_owned();
            if let Some(at) = wanted.iter().position(|sha| *sha == hex) {
                wanted.swap_remove(at);
                found.push(entry.path());
            }
        }
    }
    Some(found)
}

/// Projects an open is copying the legacy cut's held files into now: a second open of one leaves it to the first.
static COPYING: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Mutex::default);

/// Why a write failed, in a person's words (cl-1003): the disk read-only, no permission there, the disk full, or else.
pub(crate) fn write_cause(error: &(dyn std::error::Error + 'static)) -> &'static str {
    let mut at = Some(error);
    while let Some(error) = at {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return match io.kind() {
                std::io::ErrorKind::ReadOnlyFilesystem => "the disk is read-only",
                std::io::ErrorKind::PermissionDenied => "cozy may not write there",
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => "the disk is full",
                _ => "it couldn't be written",
            };
        }
        at = error.source();
    }
    "it couldn't be written"
}

/// The legacy cut's hold of the project at `root`, at its open (cl-1003). Each file its lock says stays on `local` and only this
/// computer's TensorFS store holds (made before projects kept their own machine objects) is copied into its `files/`: written under a
/// hidden name of this open's own, synced, read back and checked against its hash, then put in place. One a local run made since
/// containment is in the project's own `.vault-machine/objects` already. Then its hold goes, and what the copies brought in is let go
/// of in the store; what another project's hold still names stays there for a later start (`release_kept`). Answers how many files
/// the store no longer held whole (gone before now: nothing brings them back); None when its lock can't be read here or another open
/// is copying it. Err: why one couldn't be copied (`write_cause`); its hold stays for its next open.
pub(crate) fn keep_left_on_local(root: &FsPath, calcifer: &FsPath) -> Result<Option<usize>, &'static str> {
    use std::io::Read as _;
    struct Copying(PathBuf);
    impl Drop for Copying { fn drop(&mut self) { COPYING.lock().expect("copying lock").remove(&self.0); } }
    let logical = vault::logical(root);
    if !COPYING.lock().expect("copying lock").insert(logical.clone()) { return Ok(None); }
    let _copying = Copying(logical);
    let Ok(physical) = vault::resolve_path(root) else { return Ok(None) };
    let folder = Folder::at(&physical);
    let Ok(lock) = folder.lock() else { return Ok(None) };
    let store = crate::runs::files::store().map_err(|_| "this computer's file store can't be opened")?;
    // The project's own machine store, looked at by path only: opening it could write its layout under a running TensorD.
    let own = physical.join(".vault-machine").join("objects").join("blobs");
    let record = crate::store::kept_path(calcifer, root).with_extension("release");
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|_| "it couldn't be written")?;
    let nonce = hex::encode(nonce);
    // What a stopped copy left: one open copies at a time, so none of these is being written.
    for entry in std::fs::read_dir(physical.join("files")).into_iter().flatten().flatten() {
        if entry.file_name().to_str().is_some_and(|name| name.starts_with('.') && name.ends_with(".cut-part")) { let _ = std::fs::remove_file(entry.path()); }
    }
    let mut gone = 0;
    for file in lock.files.values() {
        let at = folder.path(&file.path);
        let Some(hex) = file.sha256.strip_prefix("sha256:") else { continue };
        // Here, itself or by a link whose bytes are still there. A link left dangling (into the old landing folder, set aside; cl-1036)
        // is as missing as a file left on `local`: its bytes were held for it. One kept nowhere is copied in when the store has it (a
        // file whose bytes only that folder had), and is no loss of this upgrade's when it hasn't.
        if at.is_file() { continue; }
        let left = file.machine == "local" || std::fs::symlink_metadata(&at).is_ok_and(|meta| meta.file_type().is_symlink());
        if !left && !file.machine.is_empty() { continue; }
        let ours = hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) && own.join(&hex[..2]).join(&hex[2..4]).join(hex).is_file();
        if ours { continue; }
        // A link's entry has no length in the lock (`Folder::lock`): the held copy's is the file's.
        let held = tensorfs::format::ObjectRef { sha256: hex.to_owned(), length: file.length };
        let Some((from, length)) = store.path(&held).ok().and_then(|from| Some((from.clone(), std::fs::metadata(&from).ok().filter(|meta| meta.is_file())?.len()))) else {
            if left { gone += 1; }
            continue;
        };
        let object = tensorfs::format::ObjectRef { length, ..held };
        // Hidden while it's written: a name that starts with its hash would be taken for the file itself (`Folder::lock`).
        let part = at.with_file_name(format!(".{}.{nonce}.cut-part", at.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()));
        let done = (|| -> Result<bool, &'static str> {
            let failed = |error: std::io::Error| write_cause(&error);
            if let Some(dir) = part.parent() { std::fs::create_dir_all(dir).map_err(failed)?; }
            // A file of its own (the store's are read-only), synced, then read back and checked against its hash.
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut written = options.open(&part).map_err(failed)?;
            std::io::copy(&mut std::fs::File::open(&from).map_err(failed)?, &mut written).map_err(failed)?;
            written.sync_all().map_err(failed)?;
            drop(written);
            let (mut read, mut hash, mut buffer) = (std::fs::File::open(&part).map_err(failed)?, Sha256::new(), vec![0; 1 << 20]);
            loop {
                let n = read.read(&mut buffer).map_err(failed)?;
                if n == 0 { break; }
                hash.update(&buffer[..n]);
            }
            if hex::encode(hash.finalize()) != object.sha256 { return Ok(false); }
            std::fs::rename(&part, &at).map_err(failed)?;
            if let Some(dir) = at.parent() { platform::durable::sync_dir(dir).map_err(|error| write_cause(&error))?; }
            Ok(true)
        })();
        match done {
            Ok(true) => {
                let mut noted = released(&record);
                noted.push(object);
                note(&record, &noted);
            }
            // The store's copy isn't what the lock names: as lost as one gone, and never let go of here.
            Ok(false) => { let _ = std::fs::remove_file(&part); gone += 1; }
            Err(cause) => { let _ = std::fs::remove_file(&part); return Err(cause); }
        }
    }
    release_left(root, calcifer);
    Ok(Some(gone))
}

/// What the legacy cut held for the project at `root` in this computer's TensorFS store let go of, and what its copies brought in
/// (noted by `keep_left_on_local`) let go of there too; what the store keeps yet (another project's hold, a reader) is let go of at
/// a later start (`release_kept`).
fn release_left(root: &FsPath, calcifer: &FsPath) {
    let kept = crate::store::kept_path(calcifer, root);
    if kept.exists() && std::fs::remove_file(&kept).is_err() {
        tracing::warn!(target: "cozy.service_health", code = "legacy_hold_kept", "a hold of the legacy cut stays until the next open");
        return;
    }
    let Ok(store) = crate::runs::files::store() else { return };
    let record = kept.with_extension("release");
    release(store, &record, released(&record));
}

/// The legacy cut's holds not let go of yet (each a project not opened since at the folder it had then), and the bytes they keep in
/// this computer's TensorFS store.
pub(crate) fn held(calcifer: &FsPath) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(calcifer.join("kept")) else { return (0, 0) };
    let store = crate::runs::files::store().ok();
    let (mut projects, mut bytes) = (0, 0);
    for entry in entries.flatten().filter(|entry| entry.path().extension().is_some_and(|e| e == "cozytensors")) {
        projects += 1;
        let Some(header) = std::fs::read(entry.path()).ok().and_then(|read| tensorfs::format::Header::from_cbor(&read).ok()) else { continue };
        bytes += header.objects().filter(|object| store.is_some_and(|store| store.path(object).is_ok_and(|at| at.is_file()))).map(|object| object.length).sum::<u64>();
    }
    (projects, bytes)
}

/// What a project's copies left in this computer's TensorFS store and couldn't let go of yet (`release`).
fn released(record: &FsPath) -> Vec<tensorfs::format::ObjectRef> {
    std::fs::read(record).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Lets go of `objects` in this computer's TensorFS store, each copied into a project and checked there, with the uses named by
/// them. What the store keeps (another project's hold, a reader, a delete Windows defers) is left in `record` for the next start.
fn release(store: &tensorfs::store::Store, record: &FsPath, objects: Vec<tensorfs::format::ObjectRef>) {
    let named: Vec<String> = objects.iter().map(|object| object.sha256.clone()).collect();
    let kept = match objects.is_empty() {
        true => vec![],
        false => match store.retire_objects(&objects, &named, &[]) {
            Ok(retired) => retired.retained,
            Err(_) => {
                tracing::warn!(target: "cozy.service_health", code = "legacy_release_failed", "files copied into a project stay in this computer's store until the next start");
                objects
            }
        },
    };
    if kept.is_empty() {
        let _ = std::fs::remove_file(record);
        return;
    }
    tracing::info!(target: "cozy.service_health", code = "legacy_release_kept", kept = kept.len(), "files copied into a project stay in this computer's store until nothing else holds them");
    note(record, &kept);
}

/// `objects` noted in `record`, to let go of in this computer's TensorFS store.
fn note(record: &FsPath, objects: &[tensorfs::format::ObjectRef]) {
    let written = record.parent().map_or(Ok(()), platform::durable::private_dir)
        .and_then(|()| platform::durable::replace(record, &serde_json::to_vec(objects).expect("serializes"), 0o600));
    if written.is_err() { tracing::warn!(target: "cozy.service_health", code = "legacy_release_unnoted", "what a project's copies brought in couldn't be noted for letting go"); }
}

/// What a project's copies couldn't let go of in this computer's TensorFS store before, let go of again (cl-1003): at each start.
pub(crate) fn release_kept(calcifer: &FsPath) {
    let Ok(entries) = std::fs::read_dir(calcifer.join("kept")) else { return };
    let records: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e == "release")).collect();
    if records.is_empty() { return; }
    let Ok(store) = crate::runs::files::store() else { return };
    for record in records {
        release(store, &record, released(&record));
    }
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
/// A project's folder identified: its id, name, public code and key.
struct Identified {
    id: String,
    name: String,
    code: String,
    key: String,
}

/// What identifying a folder keeps in its project.db: its identity, or its key (`fresh`: the keys it held go, a copy's original's).
enum Keep<'a> {
    Identity { id: &'a str, name: &'a str, code: &'a str },
    Key { key: &'a str, fresh: bool },
}

fn identify(api: &Api, dir: &FsPath) -> Result<Identified, Error> {
    let path = dir.join(db::DB);
    if !path.is_file() {
        return Err(Folder::not_a_project(dir));
    }
    let mut conn = db::open(&path)?;
    let kept = db::identity(&conn);
    let code = db::meta(&conn, "code");
    let keys = db::keys(&conn);
    let review = db::key_review(&conn);
    identify_with(api, dir, kept, code, keys, review, |keep| match keep {
        Keep::Identity { id, name, code } => db::set_identity(&mut conn, &path, id, name, Some(code)),
        Keep::Key { key, fresh } => db::set_key_now(&conn, &path, key, fresh).map(drop),
    })
}

fn identify_loaded(api: &Api, dir: &FsPath, folder: &Folder) -> Result<Identified, Error> {
    identify_with(api, dir, folder.canonical_identity()?, folder.public_code()?, folder.keys()?, folder.key_review()?, |keep| match keep {
        Keep::Identity { id, name, code } => folder.set_identity(id, name, code),
        Keep::Key { key, fresh } => folder.set_key_now(key, fresh).map(drop),
    })
}

fn identify_with(api: &Api, dir: &FsPath, kept: Option<(String, String)>, kept_code: Option<String>, kept_keys: Vec<String>, review: bool, mut keep: impl FnMut(Keep<'_>) -> Result<(), Error>) -> Result<Identified, Error> {
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
    // An encrypted project's keybag's code: empty for one kept before codes were.
    let mut sealed = None;
    if encrypted {
        let keybag = project_vault::Keybag::load(&logical.join(project_vault::KEYBAG_FILE))
            .map_err(|_| Error::new(Kind::Conflict, "vault_identity", "Cozy can't read this locked project's key file."))?;
        sealed = Some(keybag.info().code);
        if kept.as_ref().is_none_or(|(id, _)| id != &keybag.info().project_id) {
            return Err(Error::new(Kind::Conflict, "vault_identity", "This locked project's id doesn't match its key file."));
        }
        if copied(&keybag.info().project_id) {
            return Err(Error::new(Kind::Conflict, "vault_copy", "Another folder already holds this locked project. Open that one, or move its folder here instead."));
        }
    }
    let (id, copy_of) = match &kept {
        Some((id, _)) => match original(id) {
            None => (id.clone(), None),
            Some(original) => (fresh_id(), Some(original)),
        },
        None => return Err(Error::new(Kind::Validation, "project_identity", format!("{} has no project id, so Cozy can't open it as a project.", dir.join(db::DB).display()))),
    };
    // Its name is its folder's (cl-1097): the index lists it so, and a folder renamed, here or by hand, renames it.
    let name = unique_name(api, &id, dir, folder_name(&logical))?;
    if let Some(original) = copy_of {
        let said = format!("{} is a copy of {} ({}), so Cozy opened it as a separate project called {name}.", logical.display(), original.name, original.path.display());
        tracing::info!("{said}");
        COPIES.lock().expect("copies lock").insert(logical.clone(), said);
    }
    let kept_code = kept.as_ref().filter(|(held, _)| held == &id).and(kept_code);
    // An encrypted project's code is the one its links say (`as_named`, cl-1171): the index's while it has one, else its keybag's, which
    // it was listed by locked, or for a keybag that holds none, the one derived from its id.
    let wanted = match sealed {
        Some(sealed) => Some(match store.known_project(&id).map_err(|e| Error::internal("calcifer_store", e.to_string()))? {
            Some(known) if !known.code.is_empty() => known.code,
            _ if !sealed.is_empty() => sealed,
            _ => code::derived(&id),
        }),
        None => kept_code.clone(),
    };
    let code = reserve_code(store, &id, &name, &logical, wanted.as_deref())?;
    if encrypted { vault::keep_code(&logical, &code)?; }
    if kept.as_ref() != Some(&(id.clone(), name.clone())) || kept_code.as_deref() != Some(&code) {
        keep(Keep::Identity { id: &id, name: &name, code: &code })?;
    }
    // A copy's keys are its original's.
    let kept_keys = if kept.as_ref().is_some_and(|(held, _)| held == &id) { kept_keys } else { vec![] };
    let (key, rekeyed) = key::keyed(api, &id, &name, &logical, &kept_keys, review)?;
    if kept_keys.last() != Some(&key) || review {
        keep(Keep::Key { key: &key, fresh: kept_keys.is_empty() || rekeyed })?;
    }
    vault::register_plain_root(&logical, &id)?;
    Ok(Identified { id, name, code, key })
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
    Err(Error::new(Kind::Conflict, "project_own_folder", format!("{} isn't a project. It's a folder Cozy uses while it converts a project's storage.", dir.display()))
        .remedy("Open the project next to it."))
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

/// Takes up what an earlier Calcifer left in each plaintext project: runs not settled, outputs not exported, a walk that owes.
/// Each project.db is read alone first, and only one with work left opens, which drives it. Nothing of it is kept outside the
/// project. An encrypted project is locked after a restart: its runs go on on their machines and are taken up as it unlocks.
pub(crate) fn resume(api: &Arc<Api>) {
    let known = api.calcifer.store().known_projects().unwrap_or_else(|e| {
        warn!("the projects to resume: {e}");
        vec![]
    });
    let api = api.clone();
    tokio::spawn(async move {
        for entry in known {
            if vault::encrypted(&entry.path) || !tokio::task::block_in_place(|| owes_work(&entry.path)) { continue; }
            if let Err(error) = api.projects.store(&api, &entry.id).await {
                warn!("resuming {}: {}", entry.path.display(), error.message);
            }
        }
    });
}

/// Whether the project.db in `root` holds work an earlier Calcifer left: a run not settled, an output not exported, or a walk.
fn owes_work(root: &FsPath) -> bool {
    let read = || -> rusqlite::Result<bool> {
        let conn = rusqlite::Connection::open_with_flags(root.join(db::DB), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.query_row(
            &format!("SELECT EXISTS (SELECT 1 FROM runs WHERE state <> 'settled') OR EXISTS (SELECT 1 FROM outputs WHERE {}) OR EXISTS (SELECT 1 FROM walk)", crate::store::OWED),
            [],
            |row| row.get(0),
        )
    };
    read().unwrap_or(false)
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
    project.installs.clear();
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
                vault::stir();
                return;
            }
        };
        drop(access);
        let following = following(&api, &walk.project, active);
        if let Err(error) = walk.go(&following).await {
            walk.measure_at_end();
            // The gate, then `walking`, then the writer: its read first, so the board it writes under `walking` is granted beside it.
            // Refused (a lock closing the project), the end goes on, its writes refused at once.
            let _read = vault::read_gate(&walk.project.root).await.ok();
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
            // Held for a machine that can run it (cl-1175, cl-1279): what its machines lack, said as its box says it.
            let unable = walker.unable.get(&job.key).filter(|_| waits.is_empty());
            let held = held.or_else(|| unable.map(|offer| (placement::unable_line(offer, "render"), offer.machine.to_string())));
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
    /// Its box.
    asset: String,
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
    /// Ready jobs whose quote is being read off the walk's turn (`Done::Quoted`), by key: the walk never waits on one (xs-199).
    quoting: HashSet<String>,
    /// Jobs whose quote couldn't be had, by key: why, and whether its reads gave up (the job fails at its turn) or it goes unplanned
    /// to the first of its machines that takes it, as one its package can't say.
    unquoted: HashMap<String, Unquoted>,
    /// Ready jobs every machine of which doesn't answer its frame while each is asked again, by key: they wait for one to.
    waiting: HashSet<String>,
    /// Jobs none of whose machines can run them, by key, with the offer that says why (`placement::unable`): no room on its disk for
    /// their weights (cl-1175), or no GPU (cl-1279). They wait for one that can or a rental, nothing of them sent or downloaded.
    unable: HashMap<String, Offer>,
    /// What the walk log last said of each machine not answering, by name: said again only when it changes.
    silent: HashMap<String, String>,
    /// The runs whose version's files were on their way that have arrived, by number: each arrives once, by whichever looks first.
    arrived: HashSet<u64>,
    /// Jobs waiting for another's output whose quote reads their package statically, off the plan (`Done::Foreseen`), by key: asked
    /// once while they wait, however long that takes; one that failed is asked again at its turn.
    foreseeing: HashSet<String>,
    /// Each job's planned start and the machines that can run it, by its key, as the last plan said.
    starts: HashMap<String, (i64, Vec<String>)>,
    /// What each machine is told to have ready for each job, by machine and job key: its Hub, item and function; none: nothing.
    items: HashMap<(String, String), Option<(String, v1::Predicted, String)>>,
    /// Items being made.
    making: HashSet<(String, String)>,
    /// The last turn held a job for a busy machine.
    held: bool,
    /// A run sent waits on its machine, which another may take sooner (`moves`).
    waits: bool,
    /// The machines each job's run moved away from, by its key: it never goes back to one (no flapping).
    left: HashMap<String, Vec<On>>,
    /// What `moves` last quoted its runs on (`moves_key`).
    moves_seen: u64,
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
}

fn current_view(root: &FsPath) -> Result<PathBuf, Error> {
    platform::durable::canonical(&vault::view(root)?)
        .map_err(|why| Error::new(Kind::Conflict, "project_view_unavailable", why.to_string()))
}

impl Project {
    /// Its id, as its views and runs name it.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    /// Its folder, as the index and a lock's debts name it (cl-1064).
    pub(crate) fn folder_root(&self) -> &std::path::Path { &self.root }

    /// Old references are bound to their original physical view, never a recycled drive.
    fn require_view(&self) -> Result<(), Error> {
        if self.mount_id != vault::mount_identity(&self.root) || current_view(&self.root)? != self.folder.dir || db_file(&self.folder.dir) != self.db_file {
            return Err(Error::new(Kind::Conflict, "project_view_changed", "This project's folder changed while it was open. Open the project again."));
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
                takes: ask.takes.iter().map(|take| Named { key: take.key.clone(), asset: take.asset.clone(), approve: take.approve }).collect(),
            };
            graph.add(&ask.id, &mut Now { reading, lock }, &asked);
        }
    }
}

/// What a project and its render say while what landed can't go in.
pub(super) fn not_saved(why: &str) -> String {
    format!("Not saved yet: {}. Cozy will save the results as soon as it can.", inner(why))
}

/// Makes a new project at `path`, a folder of this computer's by its absolute path, named for its folder and recorded in the index as
/// used now: `cozy new` and the page's New alike (cl-980). A retry under its `Idempotency-Key` is answered once (`keyed`).
async fn open(State(api): State<Arc<Api>>, headers: HeaderMap, retry:Option<axum::Extension<keyed::CreationRetry>>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let mut open: Open = parse(&body?)?;
    let _ownership=crate::maintenance_source::begin_project_creation().await;
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok());
    if open.unencrypted {
        if open.passphrase.is_some() {
            return Err(Refusal(Error::new(Kind::Usage, "vault_credential", "A project without a lock can't have a passphrase. Leave the passphrase out, or make a locked project.")));
        }
    } else {
        if !open.passphrase.as_ref().is_some_and(|pass| !pass.is_empty() && pass.len() <= 4096) {
            return Err(Refusal(Error::new(Kind::Usage, "vault_credential", "New projects are locked with a passphrase. Give one, or choose to make the project without a lock.")
                .next("cozy new <directory> --passphrase-file <file>").next("cozy new --unencrypted <directory>")));
        }
        if key.is_some() {
            return Err(Refusal(Error::new(Kind::Usage, "vault_idempotency", "A locked project can't be made with an Idempotency-Key, because its recovery secret must never be stored for a retry.")));
        }
    }
    let path = open.path.clone();
    if !path.is_absolute() {
        let said = format!("{} is a relative path. Give the absolute path of a folder on Cozy's computer.", path.display());
        return Err(Refusal(Error::new(Kind::Usage, "project_path", said)));
    }
    let engine = if open.unencrypted { None } else { Some(tokio::task::block_in_place(|| vault::preflight_creation(&api)).map_err(Refusal)?) };
    let (dir, recovery_secret) = if let Some(axum::Extension(retry)) = retry { (retry.0, None) } else {
        let _making = api.projects.making.lock().expect("making lock");
        refuse_own_folder(&path).map_err(Refusal)?;
        refuse_encrypted_parent(&path).map_err(Refusal)?;
        if Folder::is_project(&path) || vault::encrypted(&path) {
            let said = format!("{} is already a project.", path.display());
            return Err(Refusal(Error::new(Kind::Conflict, "project_exists", said).next("cozy status")));
        }
        let made = !path.exists();
        std::fs::create_dir_all(&path).map_err(|e| Refusal(unmakeable(&path, e)))?;
        // A folder Calcifer makes is its account's alone: a plain project is otherwise open to other accounts straight from disk.
        if made { platform::durable::make_private_dir(&path).map_err(|e| Refusal(Error::new(Kind::Internal, "project_folder_private", e.to_string())))?; }
        let name = folder_name(&path);
        tokio::task::block_in_place(|| create_new(&api, &path, &name, open.starter, open.passphrase.take(), engine.as_ref())).map_err(Refusal)?
    };
    // A project nothing reads is only identified: the call that reads it opens it. One open, maybe where it was before it moved, is
    // opened here.
    let unread = match dir.join(db::DB).is_file() {
        true => {
            let identified = tokio::task::block_in_place(|| identify(&api, &dir)).map_err(Refusal)?;
            api.projects.open_now(&identified.id).is_none().then_some(identified)
        },
        false => None,
    };
    let (id, code, key, said) = match unread {
        Some(Identified { id, code, key, .. }) => (id, code, key, copy_told(&dir).into_iter().collect()),
        None => {
            let project = api.projects.open(&api, &dir).map_err(Refusal)?;
            let said = std::mem::take(&mut *project.said.lock().expect("said lock"));
            copy_told(&project.root);
            (project.id.clone(), project.code.clone(), project.key(), said)
        }
    };
    let opened = opened(id, code, key, &dir);
    api.calcifer.store().project_opened(&opened.id, &opened.name, &dir, Some(now_ms()), Some(&opened.code))?;
    used_now(&api, &opened.id);
    let next = vec!["cozy open".into()];
    let response = Json(Opened { next, said, recovery_secret, ..opened }).into_response();
    Ok(if open.unencrypted { response } else { vault::private(response) })
}

/// A project folder other accounts on this computer may open, shut to them (fix, don't warn; cl-1066): what's said only when that
/// fails, in one line. On Windows, where an access list reaches into all the folder holds, it's done once per folder on a thread of its
/// own, off this read's path; a failure is said at the next read.
pub(crate) fn loosened(dir: &FsPath) -> Option<String> {
    if !platform::durable::open_to_others(dir).ok()? {
        return None;
    }
    let said = |e: &dyn std::fmt::Display| format!("Other accounts on this computer can open {}, and Cozy couldn't shut them out: {e}", dir.display());
    if !cfg!(windows) {
        return platform::durable::shut_to_others(dir).err().map(|e| said(&e));
    }
    // A folder being shut: None; one that couldn't be: why, said once, then tried again. One shut leaves, so it's shut again if reopened.
    static SHUTTING: std::sync::LazyLock<Mutex<HashMap<PathBuf, Option<String>>>> = std::sync::LazyLock::new(Mutex::default);
    let mut shutting = SHUTTING.lock().expect("shutting lock");
    match shutting.get(dir) {
        Some(None) => return None,
        Some(Some(e)) => {
            let line = said(e);
            shutting.remove(dir);
            return Some(line);
        }
        None => {}
    }
    shutting.insert(dir.to_owned(), None);
    let dir = dir.to_owned();
    std::thread::spawn(move || {
        let done = platform::durable::shut_to_others(&dir).err().map(|e| e.to_string());
        let mut shutting = SHUTTING.lock().expect("shutting lock");
        match done {
            Some(e) => shutting.insert(dir, Some(e)),
            None => shutting.remove(&dir),
        };
    });
    None
}

/// A folder that can't be made, in a person's words: no permission there, or why not.
fn unmakeable(path: &FsPath, e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => Error::new(Kind::Forbidden, "project_permission", format!("Cozy doesn't have permission to make a folder at {}.", path.display())),
        _ => Error::new(Kind::Unavailable, "project_io", format!("Cozy couldn't make a folder at {}: {e}.", path.display())),
    }
}

/// The project `id`, in `dir`, as an answer gives it.
fn opened(id: String, code: String, key: String, dir: &FsPath) -> Opened {
    Opened { id, code, key, path: dir.to_owned(), name: identity(dir).1, next: vec![], said: vec![], recovery_secret: None }
}

/// A new project called `name` in `dir`, with a new id, holding the starter film or nothing: its `project.db`, made whole and then
/// named. Its canonical folder.
fn made_new(api: &Api, dir: &FsPath, name: &str, starter: bool) -> Result<PathBuf, Error> {
    refuse_encrypted_parent(dir)?;
    let io = |e: platform::Error| Error::new(Kind::Unavailable, "project_io", format!("Cozy couldn't set up a project in {}: {e}.", dir.display()));
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
    std::fs::rename(&part, dir.join(db::DB)).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("Cozy couldn't set up a project in {}: {e}.", dir.display())))?;
    platform::durable::sync_dir(dir).map_err(|e| Error::new(Kind::Unavailable, "project_io", e.to_string()))
}

/// Fresh encrypted projects write their first database through the mount, never through a plaintext source folder.
fn create_new(api: &Api, dir: &FsPath, name: &str, starter: bool, passphrase: Option<String>, engine: Option<&project_mount::Provider>) -> Result<(PathBuf, Option<String>), Error> {
    let Some(passphrase) = passphrase else { return made_new(api, dir, name, starter).map(|dir| (dir, None)); };
    refuse_encrypted_parent(dir)?;
    if std::fs::read_dir(dir).map_err(|e| Error::new(Kind::Unavailable, "project_io", e.to_string()))?.next().is_some() {
        return Err(api::calcifer::project::folder_not_empty(dir));
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
        return Err(Error::new(Kind::Conflict, "vault_nested_project", "This folder is inside a locked project. Make the new project somewhere outside it."));
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
    let unavailable = || Error::new(Kind::Unavailable, "project_ownership_unavailable", "Cozy couldn't tell which project this source folder belongs to.");
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
                Ok(_) if source => return Err(Error::new(Kind::Validation, "project_format_unsupported", "This source is inside an old or half-made project, so Cozy won't use it for maintenance.")),
                Ok(_) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(_) => return Err(unavailable()),
            }
        }
    }
    if source { return Err(Error::new(Kind::NotFound, "project_not_found", "This source isn't inside a project.").next("cozy new <directory>")); }
    Err(Error::new(Kind::NotFound, "project_not_found", format!("{} isn't in a project: there's no {} in it or in any folder above it.", path.display(), db::DB))
        .remedy("Run this from a project's folder, or make a new project.")
        .next("cozy new <directory>"))
}

/// Project `id` used now, which the index lists by (cl-1096): written off the caller's path, as a write waits on whatever holds the
/// writer (cl-1010).
pub(crate) fn used_now(api: &Arc<Api>, id: &str) {
    let (api, id, at) = (api.clone(), id.to_owned(), now_ms());
    let write = move || if let Err(e) = api.calcifer.store().project_used(&id, at) { warn!("state.db: {e}"); };
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => drop(runtime.spawn_blocking(write)),
        Err(_) => write(),
    }
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

/// The index, last used first: each project's name, where it is and its last use, and whether it is rendering. A locked one by its
/// keybag's id and code (`as_named`).
async fn list(State(api): State<Arc<Api>>, Query(page): Query<Page>) -> Result<Json<Listed>, Refusal> {
    let mut known: Vec<_> = indexed(&api).map_err(Refusal)?.into_iter().map(as_named).collect();
    if let Some(before) = &page.before {
        let at = known.iter().position(|entry| &entry.id == before).map_or(known.len(), |at| at + 1);
        known.drain(..at);
    }
    let limit = page.limit.unwrap_or(50).clamp(1, 1000);
    let older = known.len().saturating_sub(limit) as u64;
    let open: HashMap<String, Arc<Project>> = api.projects.now_open().into_iter().map(|project| (project.id.clone(), project)).collect();
    let projects = (known.into_iter().take(limit))
        .map(|entry| Known {
            key: readable_key(&entry.path, entry.key),
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
        return Ok(Json(api::calcifer::project::Address { id: project.id.clone(), code: project.code.clone(), key: project.key(), gone: None }));
    }
    let entry = known(&api, &reference).map_err(Refusal)?;
    Ok(Json(api::calcifer::project::Address { id: entry.id, code: entry.code, key: readable_key(&entry.path, entry.key), gone: None }))
}

/// A project's key (its `KEY-N`) from the index, said only while the project can be read: a locking one's is its project.db's, as a
/// locked one's is (cl-1130).
fn readable_key(path: &FsPath, key: String) -> String {
    if !key.is_empty() && vault::encrypted(path) && vault::hidden(path) { String::new() } else { key }
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
            let warning = format!("{}. The render that's going keeps the text it started with, and Cozy can't start a new one until the project reads again.", inner(&error.message));
            return Ok(Json(ProjectState { covered: None, id, code: project.code.clone(), key: project.key(), path: folder.dir.clone(), assets: vec![], heads: vec![], queue, missing: vec![], orphans: vec![], busy, absent: 0, run_files: 0, cuts: vec![], warnings: vec![ProjectWarning { name: None, message: warning }], notes: vec![], next: vec![], board: board_of(&project), unsent: vec![], machines: chosen(&api, &project.folder), appearance: project.folder.appearance() }));
        }
    };
    // What a reader now holds is what any change from here is told against.
    project.told.lock().expect("told lock").get_or_insert_with(|| model::Told::now(&project, model.clone()));
    let mut state = model.state(&project, asked.assets);
    state.machines = chosen(&api, &project.folder);
    state.appearance = project.folder.appearance();
    state.covered = covered;
    state.unsent = project.unsent.lock().expect("unsent lock").clone();
    state.warnings.extend(project.said.lock().expect("said lock").drain(..).map(ProjectWarning::of_project));
    state.warnings.extend(loosened(&project.root).map(ProjectWarning::of_project));
    let lost = api.calcifer.store().lost_before_upgrade().unwrap_or_default().into_iter().find(|(path, _)| vault::logical(path) == vault::logical(&project.root));
    state.warnings.extend(lost.map(|(_, files)| ProjectWarning::of_project(format!("{files} files of this project were lost before this upgrade: the machine that kept them no longer had them"))));
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
    // With no render yet, what its machines have ready for its boxes is still its board's to say (`Jobs::guessed`).
    Some(project.jobs.board()).filter(|board| board.seq > 0 || !board.ahead.is_empty())
}

/// The local code `id`'s assets name, each once: its path as they write it, and where that is. Not a checkout's first-party
/// package, which is listed by its name (`source_tools::listed`).
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

/// The published tools its boxes name, as a tool's `id` is written (`[org/]package/function`, no release): the list holds each, whatever
/// the Hub lists or you took off it, as the local code they name (`local_code_named`).
pub(crate) fn tools_named(api: &Arc<Api>, id: &str) -> Result<BTreeSet<String>, Refusal> {
    let project = api.projects.get(api, id)?;
    let folder = &project.folder;
    let doc = Doc::load(folder).map_err(Refusal)?;
    let home = &api.machines.user_home;
    let local = |with: &str| is_local(with) || crate::local_code::target(with, Some(&folder.dir), home).is_some();
    Ok((0..doc.assets.len()).map(|i| doc.target(i))
        .filter(|with| !with.trim().is_empty() && !local(with))
        .filter_map(|with| {
            let id = with.trim().split('@').next().unwrap_or_default().to_owned();
            api::target::Published::parse(&id).ok().filter(|published| published.function.is_some()).map(|_| id)
        })
        .collect())
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
    /// It in words (`Package::title`).
    title: String,
    tags: Vec<String>,
    makes: bool,
    /// The function its tool names: what its boxes' text is bound by.
    callable: Option<api::interface::Callable>,
    /// Local code, read only for `callable`: its tags and function are said once a render reads it.
    local: bool,
    /// Its interface, whose needs rules choose a box's models (`chosen_of`).
    package: Arc<Package>,
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
            let heard = HeardPackage { name: package.name.clone(), title: package.title(), tags: package.reserved(), makes: matches!(package.maker(), Ok(Some(_))), callable, local, package };
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
    let call = binder::Call { name: &doc.assets[i].name, tool: &target, package: &heard.name, title: &heard.title, callable, fields: &fields, reserved, doc, own: Some(i), lock, stand, gone: &BTreeSet::new(), pickup: None, inline: &[] };
    binder::unused(&call)
}

/// The models asset `i`'s tool chose for it by its own rules (cl-1281): each slot of its function whose needs rule holds for the request a
/// render of it would send (`binder::foreseen`, as a guess reads it), by what the box holds there, else the slot's default. None until its
/// package is heard of.
fn chosen_of(reading: &Reading, i: usize) -> Option<Vec<api::calcifer::project::Chosen>> {
    let Reading { doc, lock, stand, known, .. } = *reading;
    let target = doc.target(i);
    let heard = known.get(&target).filter(|_| doc.makes(i) && doc.tool(i).is_none())?;
    let callable = heard.callable.as_ref()?;
    let entry = api::interface::entrypoint(heard.package.document(), &callable.name)?;
    let slots = api::interface::models(entry);
    let ruled = |slot: &Value| slot["needs"].is_string();
    if !slots.iter().any(ruled) {
        return Some(vec![]);
    }
    let fields = doc.inputs_of(i);
    let reserved = tags_now(doc, known, i).unwrap_or(heard.tags.as_slice());
    let call = binder::Call { name: &doc.assets[i].name, tool: &target, package: &heard.name, title: &heard.title, callable, fields: &fields, reserved, doc, own: Some(i), lock, stand, gone: &BTreeSet::new(), pickup: None, inline: &[] };
    let answers = ::needs::answers(entry, &Value::Object(binder::foreseen(&call)));
    let held = doc.assets[i].made.get("models").unwrap_or(&Value::Null);
    let chosen = slots.iter().filter(|slot| ruled(slot)).filter_map(|slot| {
        let slot_name = api::interface::slot_path(slot)?.rsplit_once(".models.")?.1;
        let model = held[slot_name]["ref"].as_str().or(slot["default"].as_str()).filter(|_| answers.needed.contains(slot_name))?;
        Some(api::calcifer::project::Chosen { slot: slot_name.to_owned(), model: model.to_owned() })
    });
    Some(chosen.collect())
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
    format!("{name}: {package} has no single function that makes this box, so Cozy can't tell which to use. Choose one of its functions as the box's tool.")
}

/// Whether cutting `asset` again from `inputs` would only make version `old` again: those inputs, nothing left out, and its film here and
/// as long as its pieces.
fn cuts_same(folder: &Folder, lock: &Lock, asset: &format::Asset, reads: &[format::ReadNow], inputs: &str, old: &Version) -> bool {
    let film_stands = old.files.get("video").and_then(|sha256| lock.files.get(sha256)).is_some_and(|film| folder.path(&film.path).is_file());
    old.inputs == inputs && old.without.is_empty() && film_stands && tools::film_off(&asset.inputs, reads, lock, old).is_none()
}

/// The newest version of cut `asset` that cutting it again from `inputs` would only make again.
fn same_cut<'a>(folder: &Folder, lock: &'a Lock, asset: &format::Asset, reads: &[format::ReadNow], inputs: &str) -> Option<&'a Version> {
    lock.asset(&asset.name)?.versions.iter().rev().find(|old| cuts_same(folder, lock, asset, reads, inputs, old))
}

/// The version cut `asset` shows (`standing`: its pinned one, else its newest), when cutting it again would only make that again: then a
/// render makes nothing (cl-910). One only an older version holds is landed again as its newest, as a cut lands (cl-1310).
fn shown_cut<'a>(folder: &Folder, lock: &'a Lock, asset: &format::Asset, standing: &Standing, inputs: &str) -> Option<&'a Version> {
    let shown = lock.asset(&asset.name)?.version(standing.version()?)?;
    cuts_same(folder, lock, asset, &standing.reads, inputs, shown).then_some(shown)
}

/// What a cut of asset `i` is at the root: `<name>.mp4`.
fn cut_name(name: &str) -> String {
    format!("{}.mp4", name.replace(['/', '\\'], "_"))
}

/// What to do about a piece a cut went without: render it on the cut's machines, then cut again.
fn render_first(piece: &str, on: &str) -> String {
    match on.is_empty() {
        true => format!("Render {piece} first, then cut again."),
        false => format!("Render {piece} on {on} first, then cut again."),
    }
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

/// The most of a box of words' version a read of it says: the rest is in its file.
const WORDS_MAX: usize = 64 * 1024;

/// The file people mean by `files`, as a version lists it.
fn main_file(folder: &Folder, lock: &Lock, version: &Version) -> Option<MainFile> {
    let sha256 = version.main()?;
    let file = lock.files.get(sha256)?;
    let text = (file.field() == format::TEXT && file.here).then(|| {
        let words = std::fs::read(folder.path(&file.path)).ok()?;
        let cut = String::from_utf8_lossy(&words[..words.len().min(WORDS_MAX)]).into_owned();
        Some(cut)
    }).flatten();
    let output = version.main_output()?.clone();
    let metadata = version.metadata.get(&output).cloned().unwrap_or_default();
    let (absent, machine) = whereabouts(file);
    let measured = |number: u64| (number > 0).then_some(number);
    Some(MainFile {
        output,
        path: file.path.clone(),
        sha256: sha256.clone(),
        field: file.field().to_owned(),
        text,
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
    let said = said.as_object().ok_or("isn't a map of names to values")?;
    let length = serde_json::to_vec(said).map_or(usize::MAX, |bytes| bytes.len());
    if length > METADATA_MAX {
        return Err(format!("is {length} bytes, more than the {METADATA_MAX} allowed"));
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
            takes.entry(name_key(asset)).or_default().push(TakeNow { id: id.to_owned(), queued: run.is_none(), run: run.filter(|run| *run > 0) });
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
        _ if !gone.is_empty() => format!("{} isn't on this computer; open the project in the browser to download it", gone.join(", ")),
        Stand::Pinned { version: None } => "approved".into(),
        Stand::Pinned { version: Some(version) } => format!("approved {noun} {version}"),
        Stand::Current(version) => format!("{noun} {version}"),
        Stand::Words => "text only".into(),
        Stand::Missing if asset.hollow => "nothing to make".into(),
        Stand::Missing => format!("no {noun} yet"),
    };
    let made = standing.version().and_then(|number| held?.version(number));
    let main = folder::main_of(&files).and_then(|sha256| lock.files.get(sha256));
    let measured = |number: fn(&folder::File) -> u64| main.map(number).filter(|&number| number > 0);
    let (absent, machine) = main.map_or((false, None), whereabouts);
    let failure = held.and_then(|held| held.failed.as_ref()).filter(|failed| failed.inputs == standing.inputs).map(|failed| failed.error.clone());
    let row_standing = match &standing.state {
        Stand::Missing => RowStanding::Missing,
        _ => RowStanding::Current,
    };
    Row {
        name: asset.name.clone(),
        standing: row_standing,
        making: vec![],
        takes: vec![],
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
                    main: main_file(folder, lock, version),
                    aspect_ratio: version.fields.as_ref().and_then(|fields| fields.get("aspect_ratio")?.as_str()).map(str::to_owned),
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
        chosen: chosen_of(reading, i),
        fields: doc.flat_fields(i),
        etag: None,
        failure,
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
        let said = "The project is still rendering. Free up space once its renders finish.";
        return Err(Refusal(Error::new(Kind::Conflict, "project_busy", said).next("cozy status")));
    }
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let (orphans, kept) = (orphans(&doc, folder, &lock), kept_files(&doc, &lock));
    let lost = folder.history_lost(&lock, &kept);
    let recorded = lock.recorded();
    if let (true, Some(why)) = (asked.yes, &lost) {
        let remedy = format!("Restore {} from a backup, or delete the files you don't want from files/ yourself.", db::DB);
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
    }
    if !asked.yes {
        let (files, bytes, unrecorded, unheld) = folder.sweep(&kept, &recorded, false).map_err(Refusal)?;
        let next = (lost.is_none() && (files > unrecorded || !orphans.is_empty())).then(|| "cozy open".to_owned()).into_iter().collect();
        return Ok(Json(Reclaimed { orphans, files, bytes, unrecorded, deleted: false, next, lost, unheld }));
    }
    // What a text names by number stays, on any row.
    let named = doc.versions_named();
    let row_named: BTreeMap<String, BTreeSet<u32>> = named.iter().flat_map(|(key, numbers)| lock.rows_of(key).map(move |(row, _)| (row.clone(), numbers.clone()))).collect();
    // One transaction: the lock and the canvas without what goes, and its answer with its key (cl-885). Its files go inside it, before it
    // commits, with what's kept as files arrive held off (`collecting`, taken before the transaction as every keeper takes it): a resend
    // of one that never committed finds them gone, and says so.
    let _collecting = folder.collecting();
    let reclaimed = folder.act(|| {
        folder.update_lock(|lock| {
            lock.assets.retain(|key, _| doc.find(key).is_some() || row_named.contains_key(key));
            for (key, held) in &mut lock.assets {
                match row_named.get(key).filter(|_| doc.find(key).is_none()) {
                    Some(numbers) => held.keep_only(numbers),
                    None => held.clear_discarded(row_named.get(key)),
                }
            }
        })?;
        let (files, bytes, unrecorded, unheld) = folder.swept(&kept, &recorded, true)?;
        canvas::prune(folder, &id, |name| doc.find(name).is_some())?;
        let reclaimed = Reclaimed { orphans, files, bytes, unrecorded, deleted: true, next: vec![], lost, unheld };
        keyed::answering(StatusCode::OK, &reclaimed);
        Ok(reclaimed)
    });
    Ok(Json(reclaimed.map_err(Refusal)?))
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
/// none: any machine. One ending stays: a run none of them can take says why. Every reader of the project hears it.
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
                let said = format!("{other} isn't a machine. Name the machines this project renders on, or leave the list empty to use any.");
                return Err(Refusal(Error::new(Kind::Usage, "project_machines", said).next("cozy machine ls")));
            }
        };
        if !kept.contains(&name) {
            kept.push(name);
        }
    }
    // Its answer is kept with it, in its transaction (cl-885).
    project.folder.act(|| {
        project.folder.set_machines(&kept)?;
        keyed::answering(StatusCode::OK, &ProjectMachines { machines: kept.clone() });
        Ok(())
    }).map_err(Refusal)?;
    crate::events::project_settled(&project.folder.dir);
    Ok(Json(ProjectMachines { machines: kept }))
}

/// `PUT /v1/projects/{id}/appearance` (cl-1097): how the page looks with it open; none, as everywhere. No history step: every reader
/// hears it, as for its machines.
async fn appearance(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ProjectAppearance>, Refusal> {
    let asked: ProjectAppearance = parse(&body?)?;
    if let Some(Err(why)) = asked.appearance.as_ref().map(api::calcifer::Appearance::check) {
        return Err(Refusal(Error::new(Kind::Usage, "appearance_invalid", why)));
    }
    let project = api.projects.get(&api, &id)?;
    project.folder.act(|| {
        project.folder.set_appearance(asked.appearance.as_ref())?;
        keyed::answering(StatusCode::OK, &asked);
        Ok(())
    }).map_err(Refusal)?;
    crate::events::project_settled(&project.folder.dir);
    Ok(Json(asked))
}

/// What a project's folder may be called (cl-1097): a name the folder beside it takes, never a path, nor one of Cozy's own.
fn folder_named(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    let bad = |why: &str| Err(Error::new(Kind::Usage, "project_name", format!("{why}. Pick another name.")));
    if name.is_empty() { return bad("A project needs a name"); }
    if name.contains(['/', '\\', '\0']) { return bad("A project's name can't have / or \\ in it"); }
    if matches!(name, "." | "..") || name.starts_with(".cozy-") { return bad(&format!("{name} is a name Cozy keeps for itself")); }
    if name.len() > 255 { return bad("That name is too long"); }
    Ok(name)
}

/// `PUT /v1/projects/{id}/name` (cl-1097): the project called `name`. A project's name is its folder's (the index lists each by its
/// folder, and a locked one has no other), so its folder is renamed where it is; its project.db takes the name as it opens next. Locked,
/// it's renamed with no secret; unlocked and encrypted, it locks first. Every page hears the index change.
async fn name(State(api): State<Arc<Api>>, Path(named): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Json<ProjectMoved>, Refusal> {
    let asked: ProjectName = parse(&body?)?;
    let wanted = folder_named(&asked.name).map_err(Refusal)?.to_owned();
    let root = vault::location(&api, &named).map_err(Refusal)?;
    let id = vault::project_id(&root).or_else(|| vault::plain_project_id(&root)).unwrap_or_else(|| project_id(&root));
    let next = root.with_file_name(&wanted);
    if next == root {
        return Ok(Json(ProjectMoved { id, name: wanted, path: root, trashed: None }));
    }
    // Another folder there is never taken: case alone differing is the same name on most disks, and that's this folder.
    if std::fs::symlink_metadata(&next).is_ok() && !next.to_string_lossy().eq_ignore_ascii_case(&root.to_string_lossy()) {
        let said = format!("There's already a folder called {wanted} beside this project's. Pick another name.");
        return Err(Refusal(Error::new(Kind::Conflict, "project_name_taken", said)));
    }
    let streams = crate::events::showing(&id);
    tokio::task::block_in_place(|| vault::relocate(&api, &root, &vault::Relocation::Rename(next.clone()))).map_err(Refusal)?;
    // A plain one opens again at once, which keeps its new name in its project.db, and what showed it shows it again.
    if !vault::encrypted(&next) {
        crate::events::show_again(&api, &streams, &id);
        match api.projects.get(&api, &id) {
            Ok(project) => crate::events::project_settled(&project.folder.dir),
            Err(e) => warn!("{}: {}", next.display(), e.0.message),
        }
    }
    crate::events::projects_changed();
    Ok(Json(ProjectMoved { id, name: wanted, path: next, trashed: None }))
}

/// `DELETE /v1/projects/{id}` (cl-1097): its folder moved to the OS trash, whole, never deleted: what's in it, encrypted or not, isn't
/// read, so a locked project needs no secret. Its runs are canceled, it leaves the index as a folder found gone does, and every page
/// hears it gone.
async fn delete_project(State(api): State<Arc<Api>>, Path(named): Path<String>) -> Result<Json<ProjectMoved>, Refusal> {
    let root = vault::location(&api, &named).map_err(Refusal)?;
    let id = vault::project_id(&root).or_else(|| vault::plain_project_id(&root)).unwrap_or_else(|| project_id(&root));
    let trashed = tokio::task::block_in_place(|| vault::relocate(&api, &root, &vault::Relocation::Trash)).map_err(Refusal)?;
    // The index lets it go as for any folder gone: its row, its routes, and the local machine's records of it.
    indexed(&api).map_err(Refusal)?;
    crate::events::project_deleted(&id, &root);
    crate::events::projects_changed();
    Ok(Json(ProjectMoved { id, name: folder_name(&root), path: root, trashed }))
}

/// Asset `from` called `to`, as `POST rename` has it, in one transaction.
fn rename_now(project: &Arc<Project>, id: &str, asked: Rename) -> Result<Renamed, Refusal> {
    // Under `walking`, as a walk's end is: it is over, or it follows this. Taken before the transaction, as every writer takes it.
    // The gate, then `walking`, then the writer: its read first, so the writes under `walking` are granted beside it and never queue.
    let _read = vault::read_gate_blocking(&project.root).map_err(Refusal)?;
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
            return Err(refuse(Kind::NotFound, "asset_unknown", format!("There's no box called {} in this project.", asked.to)));
        };
        let mut back = lock.clone();
        back.untombstone(&asked.from);
        let old = back.asset(&asked.from).filter(|held| held.on_record().next().is_some());
        if old.is_none() && !free && doc.find(&asked.from).is_some() {
            return Err(refuse(Kind::Conflict, "rename_taken", format!("There's already a box called {} in this project.", asked.to)));
        }
        if old.is_none() && !free && canvas::row(folder, &asked.from)?.is_none() {
            let said = format!("This project has no versions of {}.", asked.from);
            return Err(Error::new(Kind::NotFound, "asset_unknown", said).next("cozy status"));
        }
        let to = doc.assets[i].name.clone();
        if old.is_some() && !same && lock.asset(&to).is_some_and(|held| held.on_record().next().is_some()) {
            return Err(refuse(Kind::Conflict, "rename_taken", format!("{to} already has versions of its own.")));
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
/// said back, the same whether this call renames it or an import did.
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
        let stood = was[j].version();
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
    let asked: Runs = parse(&body?)?;
    Ok((StatusCode::ACCEPTED, Json(render(&api, &id, asked).await?)))
}

/// `asked` of project `id`: what `POST /v1/projects/{id}/runs` and a retry of a box's run (`retry`, cl-1085) add to its walk.
async fn render(api: &Arc<Api>, id: &str, mut asked: Runs) -> Result<Asked, Refusal> {
    let (api, id) = (api.clone(), id.to_owned());
    api::calcifer::machine_set(&asked.on).map_err(Refusal)?;
    if asked.on.iter().any(|on| matches!(on, On::New(_))) {
        let said = "A project renders on machines you already have. Rent one first, then name it with --on.";
        return Err(Refusal(Error::new(Kind::Usage, "project_on_new", said).next("cozy machine rent")));
    }
    if asked.approve && asked.n > 1 {
        let said = format!("--approve approves one new version, but -n {} makes {}.", asked.n, asked.n);
        return Err(Refusal(Error::new(Kind::Usage, "approve_one", said).remedy("Leave out --approve or -n.")));
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
    // None named: where `cozy run` naming none goes (cl-894). A rental named by its order's handle, by its Hub's name for it (cl-1106).
    let any = asked.on.is_empty();
    let on = match asked.on.is_empty() {
        true => crate::runs::unnamed_set(&api, Some(&id)).await.map_err(Refusal)?,
        false => asked.on.into_iter().map(|on| match on {
            On::Machine(name) => On::Machine(api.calcifer.machine_name_now(&name)),
            other => other,
        }).collect(),
    };
    project.unsent.lock().expect("unsent lock").clear();
    // What this render uses: the project as it is now, its snapshot kept by digest until the render ends.
    let written = project.folder.written().map_err(Refusal)?;
    let doc = Doc::asked(&project.folder, &written).map_err(Refusal)?;
    let (digest, snapshot) = written.snapshot();
    let target = Target { on, hub, any };
    let lock = project.folder.lock().map_err(Refusal)?;
    let stand = doc.stand(&lock);
    // What the ask finds is its answer's: the walk it joins or begins says only what it does.
    let lines = std::cell::RefCell::new(Vec::<Said>::new());
    let say = |message: String| lines.borrow_mut().push(Said { level: NoteLevel::Warning, message, names: vec![] });
    let said = |name: &str, message: String| lines.borrow_mut().push(Said { level: NoteLevel::Info, message, names: vec![name.to_owned()] });
    let n = asked.n.max(1);
    // Every render makes new versions of what it names, and nothing else: what they read is read as it is (cl-1192, Paul 11-18).
    if asked.pickup.is_some() && asked.names.len() != 1 {
        say("A pickup works only for one box, so Cozy ignored it for several boxes.".into());
    }
    if asked.approve && asked.names.len() != 1 {
        say("--approve works only for one box, so Cozy ignored it.".into());
    }
    let mut named = vec![];
    let mut first = None;
    for name in &asked.names {
        match find(&doc, name) {
            Ok(i) => named.push(i),
            Err(error) => {
                say(error.message.clone());
                first.get_or_insert(error);
            }
        }
    }
    let mut takes = vec![];
    let one = asked.names.len() == 1 && named.len() == 1;
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
    for i in named {
        let asset = &doc.assets[i];
        let name = asset.name.clone();
        if asset.pin.is_some() && asset.hollow {
            said(&name, format!("{name} is just a file, so there's nothing to render"));
            continue;
        }
        // A cut whose shown version already cuts what it reads now would only be cut the same again: answered now, with nothing queued
        // (cl-910). One only an older version cuts is queued, and lands that film as its newest (cl-1310).
        if doc.tool(i) == Some(Tool::Cut) && !reads_asked(i)
            && let Some(version) = stand[i].inputs.as_deref().and_then(|inputs| shown_cut(&project.folder, &lock, asset, &stand[i], inputs))
        {
            said(&name, format!("{name} version {} already cuts what it reads now, so cutting it again would make the same film", version.number));
            continue;
        }
        if doc.tool(i).is_some() {
            said(&name, format!("{name} is made on this computer, for free"));
            takes.push(Take { key: new_key(), asset: name.clone(), target: target.clone(), approve, pickup: None });
            continue;
        }
        if asset.hollow {
            say(doc.nothing_to_make(i));
            continue;
        }
        let pickup = asked.pickup.clone().filter(|_| one);
        let take = |_| Take { key: new_key(), asset: name.clone(), target: target.clone(), approve, pickup: pickup.clone() };
        takes.extend((0..n).map(take));
    }
    if let (Some(error), true) = (first, takes.is_empty()) {
        return Err(Refusal(error));
    }
    // The ids its asker made, one a take (cl-945): asked again with one it holds, that take isn't asked twice.
    let resent = !asked.takes.is_empty() && !takes.is_empty() && {
        if !one || asked.takes.len() != takes.len() {
            let said = format!("You gave {} take ids for {} takes. Take ids work only when you name one box, with one id per take.", asked.takes.len(), takes.len());
            return Err(Refusal(Error::new(Kind::Usage, "take_ids", said)));
        }
        if let Some(bad) = asked.takes.iter().find(|id| !is_key(id)) {
            return Err(Refusal(Error::new(Kind::Usage, "take_id", format!("{bad} isn't a take id. A take id is `project-` followed by 32 hex digits."))));
        }
        let held = takes_held(&lock);
        takes.iter_mut().zip(&asked.takes).for_each(|(take, id)| id.clone_into(&mut take.key));
        takes.retain(|take| !held.contains(take.key.as_str()));
        takes.is_empty()
    };
    // Hold admission through the commit and its board. The previous walk
    // publishes its end under this same lock before a new one can begin.
    // The gate, then `walking`, then the writer: its read first, so the writes under `walking` are granted beside it and never queue.
    let _read = vault::read_gate_blocking(&project.root).map_err(Refusal)?;
    let mut walking = project.walking.lock().expect("walking lock");
    let joined = *walking;
    let added = if resent { 0 } else { takes.len() as u32 };
    let ask = Ask { id: new_key(), digest: digest.clone(), target: target.clone(), sent: BTreeSet::new(), takes };
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
        let said = lines.borrow().clone();
        keyed::answering(StatusCode::ACCEPTED, &Asked { id: id.clone(), ask: begun.clone(), path, joined, added, queued: vec![], said, next, board });
        Ok(())
    }))
    .map_err(Refusal)?;
    if lost.get() {
        purge::deleted(&project.id, now_ms() + purge::window_ms());
    }
    project.join(&project.folder.lock().map_err(Refusal)?);
    let queued = queued_now(&project, &ask.id);
    walk_under(&api, &project, &mut walking);
    sync_jobs(&project, numbered(&api, &project.id)).map_err(Refusal)?;
    let board = project.jobs.board();
    drop(walking);
    let said = lines.into_inner();
    Ok(Asked { id, ask: begun, path: project.folder.dir.clone(), joined, added, queued, said, next: vec!["cozy status".into()], board })
}

/// `POST /v1/projects/{id}/runs/{run}/retry` (cl-1085): a run that ended without its result asked again, placed as a run naming no machine
/// is (the project's machines, else where it finishes soonest), so never on one that ended. A box's take renders again, on its inputs and
/// the seed it failed with (`tried`), answered as a render is; any other call is sent again with its own input and seed.
async fn retry(
    State(api): State<Arc<Api>>,
    Path((id, run)): Path<(String, String)>,
    crate::api::IdempotencyKey(key): crate::api::IdempotencyKey,
) -> Result<Response, Refusal> {
    let row = crate::api::find_scoped(&api, Some(&id), &run).await?;
    let name = api.calcifer.run_name(&row);
    if row.state != crate::store::State::Settled {
        return Err(Refusal(Error::new(Kind::Conflict, "run_going", format!("{name} hasn't ended: nothing to retry")).next(format!("cozy run watch {name}"))));
    }
    if row.status == Some(RunStatus::Completed) {
        return Err(Refusal(Error::new(Kind::Conflict, "run_completed", format!("{name} completed: nothing to retry"))));
    }
    let project = api.projects.get(&api, &id)?;
    let made = project.folder.lock().map_err(Refusal)?.assets.values().find(|held| held.failed.as_ref().and_then(|failed| failed.run) == Some(row.number)).map(|held| held.name.clone());
    if let Some(made) = made {
        let asked = Runs { names: vec![made], n: 1, on: vec![], hub: String::new(), approve: false, pickup: None, takes: vec![] };
        return Ok((StatusCode::ACCEPTED, Json(render(&api, &id, asked).await?)).into_response());
    }
    if row.kind != RunKind::Call {
        return Err(Refusal(Error::new(Kind::Usage, "retry_call_only", format!("{name} is no call of a function: nothing to retry"))));
    }
    let (Some(hub), Some(asked)) = (row.hub.as_ref(), crate::runs::asked(&row)) else {
        let said = format!("{name} ran local code, which goes from its folder");
        return Err(Refusal(Error::new(Kind::Usage, "retry_local_code", said).next(format!("cozy run {} …", row.target))));
    };
    let ask = api::calcifer::RunRequest { target: asked.target, input: Some(asked.input), hub: hub.to_string(), project: Some(project.id.clone()), ..Default::default() };
    vault::project_reply(&api, Some(&id), async {
        let submitted = crate::runs::request(&api, &key, ask).await.map_err(Refusal)?;
        let status = if submitted.replay { StatusCode::OK } else { StatusCode::ACCEPTED };
        Ok((status, Json(submitted)))
    }).await
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
        let held = walker.unable.get(&job.key);
        let no_room = held.filter(|offer| !offer.no_gpu()).map(|short| NoRoom { line: placement::no_room_line(short, "render"), short: crate::bytes(short.short_bytes as f64) });
        let no_gpu = held.filter(|offer| offer.no_gpu()).map(|offer| NoGpu { line: placement::no_gpu_line(offer, "render") });
        Queued { name: job.asset.clone(), running: run.is_some(), run: run.filter(|run| *run > 0), no_room, no_gpu }
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
    // One transaction: what the walk owes dropped, and its answer with its key (cl-885). Its runs are asked to end inside it, before it
    // commits: a resend of one that never committed asks them again, which ends nothing twice.
    let done = project.folder.act(|| {
        let done = cancelled(&api, &project, &asked.names, &asked.takes, &|_| "canceled".to_owned()).map_err(|Refusal(why)| why)?;
        keyed::answering(StatusCode::OK, &done);
        Ok(done)
    });
    Ok(Json(done.map_err(Refusal)?))
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
    let (dropped, canceled) = stop_jobs(api, project, &wanted, why).map_err(Refusal)?;
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
    format!("{name} was deleted, so Cozy won't render it")
}

/// Stops what `wanted` names of a project's jobs: what waits leaves, with a row saying `why`; what is being sent isn't sent, and a run
/// it is making the sender cancels as it has it (`Walk::submit`); a run is canceled. None of it is asked again, a restart too. Answers
/// what left and the runs canceled.
fn stop_jobs(api: &Api, project: &Project, wanted: &dyn Fn(&str, &str) -> bool, why: &dyn Fn(&str) -> String) -> Result<(Vec<String>, Vec<u64>), Error> {
    let (waiting, going) = {
        let mut walker = project.walker.lock().expect("walker lock");
        let (dropped, going) = walker.graph.cancel(wanted);
        let rows = |ids: &[usize]| ids.iter().map(|&id| walker.graph.jobs[id].clone()).collect::<Vec<walker::Job>>();
        (rows(&dropped), rows(&going))
    };
    // In the walk's own write, so a send can't come between: one being sent that has no run yet sends nothing, its run being made is
    // the sender's to cancel, and what has a run is canceled here.
    let (mut dropped, mut running) = (waiting, vec![]);
    project.folder.update_walk(|walk| {
        for job in going {
            let on_its_way = walk.pending.iter().find(|(key, pending)| pending.job_of(key) == job.key).map(|(key, pending)| (key.clone(), pending.run));
            match on_its_way {
                Some((_, run)) if run > 0 => continue,
                Some((key, _)) => drop(walk.pending.remove(&key)),
                None => {}
            }
            dropped.push(job);
        }
        let owed: Vec<String> = dropped.iter().map(|job| name_key(&job.asset)).collect();
        for ask in &mut walk.walk.asks {
            ask.takes.retain(|take| !wanted(&take.asset, &take.key));
            ask.sent.extend(owed.iter().cloned());
        }
        running = walk.pending.iter().filter(|(key, pending)| pending.run > 0 && wanted(&pending.asset, pending.job_of(key))).map(|(_, pending)| pending.run).collect();
    })?;
    for job in &dropped {
        project.jobs.ended(Job { id: job.key.clone(), asset: job.asset.clone(), state: JobState::Canceled, why: Some(why(&job.asset)), ..Job::default() });
    }
    // What they waited on to be made ready ends with them, unless something else waits on it.
    project.installs.cancel(api, &dropped.iter().map(|job| job.key.clone()).collect::<Vec<_>>());
    let store = api.calcifer.project_store(&project.id).map_err(|error|Error::new(Kind::Conflict,"project_journal",error.to_string()))?;
    let mut canceled = vec![];
    for run in running {
        let Ok(Some(row)) = store.run_by_number(run) else { continue };
        if row.state != crate::store::State::Settled {
            // Inside a delete's act, `cancel` tells the machine now and keeps the request once the act lets the writer go.
            api.calcifer.cancel(&row.id).map_err(|e| Error::new(Kind::Internal, "run_cancel", e.to_string()))?;
            canceled.push(run);
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
    let mut kept = match held_name(source)? {
        None => folder.import(source).await.map_err(|why| import_failed(source, why))?,
        Some(digest) => match crate::runs::files::held_for(&folder.dir, &digest) {
            Some(at) => tokio::task::block_in_place(|| folder.take_held(&at, &digest.to_string())).map_err(|why| import_failed(source, why))?,
            None => folder.own(&digest.to_string()).ok_or_else(|| not_held(source))?,
        },
    };
    // A picture's size from its header now, so its box is drawn at its shape from the first read (`Folder::keep_shapes`).
    if kept.field == "image" && kept.file.width == 0 && let Some((width, height)) = tools::shape_of(&folder.path(&kept.file.path)) {
        (kept.file.width, kept.file.height, kept.file.shape_only) = (width, height, true);
    }
    Ok(kept)
}

/// The digest `source` names, if it names one.
fn held_name(source: &str) -> Result<Option<Digest>, Refusal> {
    if !source.starts_with("sha256:") {
        return Ok(None);
    }
    let digest = source.parse().map_err(|why: &str| Refusal(Error::new(Kind::Usage, "file_name_invalid", format!("{source:?} isn't a file hash: {why}.")).remedy("Name the file as sha256:<hex>.")))?;
    Ok(Some(digest))
}

fn import_failed(source: &str, why: String) -> Refusal {
    // A hash is no one's to read: a person dropped a file, and is told of it by its name.
    // A link is shown without its query, which a presigned one's credential is.
    let named = if source.starts_with("sha256:") { "This file".to_owned() } else { tensorfs::fetch::shown(source).to_string() };
    let why = inner(&why);
    let said = match (why.strip_prefix("it's "), why.strip_prefix("it ")) {
        (Some(rest), _) => format!("{named} is {rest}."),
        (None, Some(rest)) => format!("{named} {rest}."),
        (None, None) => format!("{named}: {why}."),
    };
    Refusal(Error::new(Kind::Validation, "import_failed", said).remedy("Import an image, video or sound file."))
}

fn not_held(source: &str) -> Refusal {
    Refusal(Error::new(Kind::NotFound, "file_not_held", format!("Neither Cozy nor this project has the file {source}.")).remedy(format!("Upload it first with PUT /v1/files/{source}.")))
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
        folder.keep_shapes([&file])?;
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

/// What measures project files, one ffprobe at a time beside the thumbnails' own turns (`thumbs::FFMPEGS`): what a page shows never
/// waits behind a bulk import's measuring (cl-1054: 100 pictures imported on a Mac drew none until every import had ended).
static MEASURES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// What a project's files are measured as, once its ffprobe is here, from a task of its own: a read waits for neither. One task a project:
/// each import asks, and what lands while it measures is measured once it ends, never by a task of each import's measuring them all again.
fn measure_soon(api: &Arc<Api>, project: &Arc<Project>) {
    {
        let mut measuring = project.measuring.lock().expect("measuring lock");
        if let Some(again) = measuring.as_mut() {
            *again = true;
            return;
        }
        *measuring = Some(false);
    }
    let (host, project) = (Host::new(api), project.clone());
    tokio::spawn(async move {
        loop {
            measure_once(&host, &project).await;
            let mut measuring = project.measuring.lock().expect("measuring lock");
            match *measuring {
                Some(true) => *measuring = Some(false),
                _ => {
                    *measuring = None;
                    return;
                }
            }
        }
    });
}

/// `project`'s files measured that aren't yet.
async fn measure_once(host: &Host, project: &Arc<Project>) {
    let unmeasured = || project.require_view().is_ok() && project.folder.lock().is_ok_and(|lock| lock.files.values().any(folder::File::unmeasured));
    {
        let Ok(_access) = vault::admit_root(&project.root).await else { return; };
        if !unmeasured() { return; }
    }
    // ffmpeg first, holding nothing, then its turn, holding nothing of the project, then the project, holding no turn: a lock queued
    // on it waits for none of them, and while it waits no other project's thumbnails or measures wait for it (cl-1025).
    if let Err(error) = host.ffmpeg().await {
        return warn!("measuring a project's files: {}", error.message);
    }
    // Its turn, then the project now or not at all: waiting for the project holding a turn would hold up every other project's
    // measures behind one project's lock (cl-1025). Behind one, it waits its turn on the project holding none, then asks again.
    let (access, permit) = loop {
        let permit = MEASURES.acquire().await.expect("measuring's turn never closes");
        match vault::try_admit_root(&project.root) {
            Ok(Some(access)) => break (access, permit),
            Ok(None) => {}
            Err(_) => return,
        }
        drop(permit);
        let Ok(_access) = vault::admit_root(&project.root).await else { return; };
    };
    if !unmeasured() { return; }
    let (dir, project) = (project.folder.dir.clone(), project.clone());
    let measuring = tokio::task::spawn_blocking(move || {
        // This task can outlive its importer and must drain before storage is replaced.
        // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
        let _access = access.here();
        let _permit = permit;
        let measured = crate::tools::quietly(|| project.folder.lock().and_then(|mut lock| project.folder.measure(&mut lock)));
        // What was read before this finished said nothing of how long a clip is.
        crate::events::project_settled(&dir);
        measured
    });
    if let Ok(Err(error)) = measuring.await {
        warn!("measuring a project's files: {}", error.message);
    }
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
    let landing = (folder.path(&kept.file.path), kept.file.sha256.clone(), kept.field);
    // Its answer is kept with it, in its transaction (cl-885).
    let answer = folder.act(|| {
        let answer = element(&project, &id, &wanted, asked.at, asked.description.as_deref(), kept, asked.exact, asked.moves, &asked.placed).map_err(|Refusal(why)| why)?;
        keyed::answering(StatusCode::OK, &answer);
        Ok(answer)
    }).map_err(Refusal)?;
    // What the canvas draws of it, made now (cl-982): its page asks for it in a moment.
    thumbs::landed(landing.0, &landing.1, landing.2, 0.0, false);
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
                let said = format!("No file in this project starts with {}.", asked.version);
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy status")));
            }
            several => {
                let said = format!("{} matches {} files in this project. Type more of its hash.", asked.version, several.len());
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said)));
            }
        },
        Some(wanted) => {
            let Some(sha256) = versions.iter().find(|version| version.number == wanted).and_then(|version| version.main()) else {
                let numbers: Vec<String> = versions.iter().map(|version| version.number.to_string()).collect();
                let said = match numbers.as_slice() {
                    [] => format!("{} has no {noun} yet.", asset.name),
                    numbers => format!("{} has no {noun} {wanted}. It has {}.", asset.name, numbers.join(", ")),
                };
                return Err(Refusal(Error::new(Kind::Usage, "version_unknown", said).next("cozy status")));
            };
            sha256.clone()
        }
    };
    let field = lock.files.get(&chosen).map_or_else(|| asset.kind.clone(), |file| file.field().to_owned());
    let version = wanted.and_then(|number| versions.iter().copied().find(|version| version.number == number))
        .or_else(|| versions.iter().copied().rev().find(|version| version.main() == Some(&chosen)));
    let name = asset.name.clone();
    let files = version.map_or_else(|| Files::from([(field.clone(), chosen.clone())]), |version| version.files.clone());
    let (number, path) = (version.map_or(0, |version| version.number), main_path(&lock, &files).unwrap_or_default());
    let files = listed(&lock, &files);
    // Its answer is kept with it, in its transaction (cl-885): what it approved. What settling found, its answer after says.
    let approved = folder.act(|| {
        folder.edit(|written| match version {
            Some(version) => written.selected(&name, version.number, &chosen),
            None => written.approved(&name, &chosen),
        })?;
        let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag))?;
        let approved = Approved { name: name.clone(), noun: noun.into(), number, files, path, warnings: vec![], etag };
        keyed::answering(StatusCode::OK, &approved);
        Ok(approved)
    }).map_err(Refusal)?;
    let warnings = settle(&api, &project).await;
    let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag)).map_err(Refusal)?;
    Ok(Json(Approved { warnings, etag, ..approved }))
}

/// `POST /v1/projects/{id}/unpin`: `approved` gone from its box, so the asset stands on its newest version again. A constant,
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
    if asset.pin.is_some() && asset.hollow && lock.asset(&name).is_none_or(|held| held.versions.is_empty()) {
        let said = format!("{name} is just a file, so there's nothing to change. To get rid of it, delete the box.");
        return Err(Refusal(Error::new(Kind::Usage, "project_edit", said).next("cozy open")));
    }
    // Its answer is kept with it, in its transaction (cl-885), as approve's is.
    folder.act(|| {
        if asset.pin.is_some() {
            folder.edit(|written| written.unapproved(&name))?;
        }
        let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag))?;
        keyed::answering(StatusCode::OK, &Unpinned { name: name.clone(), version, warnings: vec![], etag });
        Ok(())
    }).map_err(Refusal)?;
    let warnings = settle(&api, &project).await;
    let etag = folder.reading_boxes(|written| written.get(&name).map(written::Boxed::etag)).map_err(Refusal)?;
    Ok(Json(Unpinned { name, version, warnings, etag }))
}

/// What an approval moves at the root: each cut's film and timeline, as it stands now. Nothing is made again (cl-1192): a box is rendered
/// by name. What fails is answered.
async fn settle(api: &Arc<Api>, project: &Arc<Project>) -> Vec<String> {
    let walker = Walk::new(api, project);
    let read = || -> Result<_, Error> {
        let (doc, lock) = (Doc::load(walker.folder())?, walker.folder().lock()?);
        let stand = doc.stand(&lock);
        Ok((doc, lock, stand))
    };
    let (doc, lock, stand) = match read() {
        Ok(read) => read,
        Err(error) => return vec![error.message],
    };
    for i in (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)) {
        walker.film_follows(&doc, &stand, &lock, i).await;
    }
    if lock.files.values().any(folder::File::unmeasured) {
        measure_soon(api, project);
    }
    vec![]
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
        let said = format!("{file} isn't a project file name. Project files are named <sha256>.<ext>.");
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
        let said = format!("This project has no cut called {cut}. Its cuts are {}, each with an .otio timeline.", made.join(" and "));
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
        let said = format!("{} is missing: {e}.", path.display());
        Refusal(Error::new(Kind::NotFound, "file_missing", said).next("cozy open"))
    };
    let file = std::fs::File::open(path).map_err(missing)?;
    let len = file.metadata().map_err(missing)?.len();
    crate::files::send(file, len, media_type(path), etag, headers)
        .map_err(|e| Refusal(Error::new(Kind::Unavailable, "project_io", format!("Cozy couldn't read {}: {e}.", path.display()))))
}

/// A version to send now: asset `i`'s because it has no state (take None), or a take's.
struct Next {
    i: usize,
    /// The job's key: its first run goes under it.
    key: String,
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
pub(crate) const GONE: [&str; 3] = ["machine_ended", "machine_name_ended", "machine_ending"];
const PLACED_AGAIN: u32 = 3;

/// Job `key` as its machines `on` quote it: the function its tool `with` names, with `made`'s choices, its package read with nothing
/// done on any machine for it (`Host::foreseen`, cl-1115: a quote never acts); else why not. The walk's quote, `fit`'s estimate and a
/// box's guess alike (cl-1004, cl-1104). `asked`: its box in its project, whose render's request its function's needs rules read (cl-1103).
#[allow(clippy::too_many_arguments)]
pub(super) async fn job_of(host: &Host, dir: &FsPath, key: String, on: Vec<On>, hub: &str, with: &str, made: Map<String, Value>, asked: Option<(&Asking, &str)>) -> Result<placement::Job, String> {
    job_and_package(host, dir, key, on, hub, with, made, asked).await.map(|(job, _)| job)
}

/// `job_of`, and the package it read for the first of `on`: a guess reads it once (cl-1123: twice, a local package's guess staged its
/// checkout twice, 80 ms each, 44204359393).
#[allow(clippy::too_many_arguments)]
pub(super) async fn job_and_package(host: &Host, dir: &FsPath, key: String, on: Vec<On>, hub: &str, with: &str, made: Map<String, Value>, asked: Option<(&Asking, &str)>) -> Result<(placement::Job, Package), String> {
    job_read(host, dir, key, on, hub, with, made, asked).await.map_err(|unread| match unread {
        Unquotable::Again(why) | Unquotable::Not(why) => why,
    })
}

/// Why a job has no quote: `Again`, a read that may answer if asked again (a Hub, PyPI or machine that didn't answer in time); `Not`,
/// one that won't.
enum Unquotable {
    Again(String),
    Not(String),
}

/// `job_of`, its reads timed as stages of the quote (`crate::stages`), and why not said as whether asking again may help. `asked`: its
/// box in its project, whose render's request its function's needs rules read (cl-1103); none, no request is known and no rule answers.
#[allow(clippy::too_many_arguments)]
async fn job_read(host: &Host, dir: &FsPath, key: String, on: Vec<On>, hub: &str, with: &str, made: Map<String, Value>, asked: Option<(&Asking, &str)>) -> Result<(placement::Job, Package), Unquotable> {
    let api = host.api();
    let package = match crate::stages::stage("package", host.foreseen(hub, with, dir, &on)).await {
        Err(e) if e.kind == Kind::Unavailable => return Err(Unquotable::Again(format!("{with}: {}", e.message))),
        Err(e) => return Err(Unquotable::Not(format!("{with}: {}", e.message))),
        Ok(package) => package,
    };
    let renderer = match package.maker() {
        Ok(Some(renderer)) => renderer,
        Ok(None) => return Err(Unquotable::Not(format!("{with} has no function to run"))),
        Err(e) => return Err(Unquotable::Not(format!("{with}: {}", e.message))),
    };
    let request = asked.and_then(|(asking, name)| asking.request(name, &package, &renderer.function));
    let models = crate::stages::stage("models", fit::checkpoints(&api, hub, &package, &renderer.function, &made, request.as_ref())).await;
    // Its size and cost as its function's estimate reads its render's request (cl-1286); its made fields guessed where none is known.
    let cost = (request.as_ref().zip(api::interface::entrypoint(package.document(), &renderer.function)))
        .map(|(request, entry)| Arc::new(::needs::Cost::of(entry, request)));
    let size = match cost.as_ref().filter(|cost| cost.sized()) {
        Some(cost) => cost.size.clone(),
        None => v1::JobSize::of(&Value::Object(made)),
    };
    let gpu = api::interface::Interface::parse(package.document()).ok().is_some_and(|interface| {
        interface.callables.iter().find(|callable| callable.name == renderer.function).is_some_and(|callable| interface.needs_gpu(callable))
    });
    let scope = crate::private_io::of_path(dir).ok().flatten();
    let source = package.local.clone().filter(|dir| dir.is_dir());
    let job = placement::Job { key, target: renderer.target(), function: package.estimated(&renderer.function), models, size, gpus: 0, gpu, on, scope, source, cost };
    Ok((job, package))
}

/// A project's boxes as their renders would ask their functions, read once (cl-1103): what each one's needs rules read, so a machine is
/// told to fetch only what that render needs, and its quote counts only that.
pub(super) struct Asking {
    doc: Doc,
    lock: Lock,
    stand: Vec<Standing>,
}

impl Asking {
    /// `folder` as it stands. None: it can't be read.
    pub(super) fn of(folder: &Folder) -> Option<Self> {
        let doc = Doc::load(folder).ok()?;
        let lock = folder.lock().ok()?;
        let stand = doc.stand(&lock);
        Some(Self { doc, lock, stand })
    }

    /// What a render of box `name` would send `function` of `package` (`binder::foreseen`): its fields bound, a mention of a box not
    /// rendered yet as its kind, and no pickup (only a gesture makes one). None: it has no such box, or `package` no such function.
    pub(super) fn request(&self, name: &str, package: &Package, function: &str) -> Option<Value> {
        let i = find(&self.doc, name).ok()?;
        let callable = package.callable(function)?;
        let (fields, reserved, title, tool) = (self.doc.inputs_of(i), package.reserved(), package.title(), self.doc.target(i));
        let gone = BTreeSet::new();
        let call = binder::Call {
            name, tool: &tool, package: &package.name, title: &title, callable: &callable, fields: &fields, reserved: &reserved, doc: &self.doc,
            own: Some(i), lock: &self.lock, stand: &self.stand, gone: &gone, pickup: None, inline: &[],
        };
        Some(Value::Object(binder::foreseen(&call)))
    }
}

impl Project {
    /// The seed box `name`'s draft is predicted with (cl-1123), drawn once and kept: its render sends it, so the request a machine
    /// made ready is the one it is sent, and nothing about the render changes the item it names.
    pub(super) fn draft_seed(&self, name: &str) -> u64 {
        *self.drafts.lock().expect("drafts lock").entry(name_key(name)).or_insert_with(new_seed)
    }

    /// The seed a render of box `name` takes when its seed is unset: its draft's, so the request its machine made ready is the one it
    /// is sent (cl-1123), and taken, so its next draft, and its next render, draw a fresh one. An unset seed is a new seed every render
    /// (`new_seed`), whatever the machine answers or whether the memo serves it.
    pub(super) fn take_draft_seed(&self, name: &str) -> u64 {
        self.drafts.lock().expect("drafts lock").remove(&name_key(name)).unwrap_or_else(new_seed)
    }
}

/// The request a render of a box would send `renderer`'s function (cl-1123; Paul, 11-17: "calcifer only has to worry about what it
/// knows, i.e., it predicts what the user MAY want"): `request`, its fields bound as a render binds them (`Asking::request`), with
/// `seed` as a render puts it in, its model choices taken from its fields (`made`) as a render takes them, from `source`. The spec a
/// prepare or upcoming item carries: the machine plans it as it would plan that run, and nothing about its components, lanes or
/// downloads is said.
fn predicted(renderer: &Renderer, made: &Map<String, Value>, request: Option<Value>, seed: u64, source: v1::Source) -> Result<v1::RunSpec, Error> {
    let mut payload = match request {
        Some(Value::Object(payload)) => payload,
        _ => Map::new(),
    };
    if renderer.accepts.contains("seed") && !payload.contains_key("seed") {
        payload.insert("seed".into(), json!(seed));
    }
    for field in ["models", "lora"] {
        if let Some(value) = made.get(field) {
            payload.insert(field.into(), value.clone());
        }
    }
    let called = renderer.target();
    let mut choices = Choices::new(&called, &renderer.function, &renderer.slots, renderer.call_slots());
    choices.take(&mut payload)?;
    let models = choices::wire(&choices.into_choices())?;
    let payload = v1::Json::parse(Value::Object(payload).to_string()).map_err(|e| Error::new(Kind::Internal, "payload", e.to_string()))?;
    Ok(v1::RunSpec { kind: v1::RunKind::Call, source: Some(source), entrypoint: renderer.function.clone(), payload: Some(payload), models, ..Default::default() })
}

/// The files `spec`'s payload reads (`tools::files_in`) as a run of it lists them (`Walk::submit`), and as they're written to a machine
/// ahead of it (`Calcifer::ship`), so a predicted request is its render's to the byte (cl-1123). One not on this computer is left out.
pub(super) fn predicted_inputs(folder: &Folder, lock: &Lock, spec: &v1::RunSpec) -> (Vec<v1::InputFile>, Vec<StagedInput>) {
    let Some(payload) = spec.payload.as_ref().and_then(|payload| serde_json::from_str::<Value>(payload.get()).ok()) else { return (vec![], vec![]) };
    let (mut inputs, mut staged) = (vec![], vec![]);
    for (field, path, digest) in tools::files_in(&payload, folder, lock, &HashMap::new()) {
        let (Ok(length), Ok(parsed)) = (std::fs::metadata(&path).map(|held| held.len()), digest.parse::<Digest>()) else { continue };
        inputs.push(v1::InputFile { field: field.clone(), digest: digest.clone(), length, media_type: media_type(&path).to_owned(), order: order_of(&field) });
        staged.push(StagedInput { path, digest: parsed, length });
    }
    (inputs, staged)
}

/// What a call of `function` of `package` wants of its models, by slot (cl-1103): its needs rules evaluated on `request` (none: no rule
/// answers), walked over the functions it calls (`needs::wants`), the walk its machine makes of a request it is sent. A quote's alone
/// (cl-1123): what a machine fetches is its own to plan.
pub(super) fn wants(package: &Package, function: &str, request: Option<&Value>) -> ::needs::Wants {
    let document = package.document();
    let answers = match (request, api::interface::entrypoint(document, function)) {
        (Some(request), Some(entry)) => ::needs::answers(entry, request),
        _ => ::needs::Answers::default(),
    };
    // Another package's functions it calls are its machine's to walk, from the environment that holds them (cl-1123).
    ::needs::wants(document, &[], function, &answers)
}

/// Why the plan placed no machine for ready job `key`, said when that changes: it goes to the first of its machines that takes it.
fn unplanned(walker: &mut Walker, dir: &str, key: &str, why: String) {
    if walker.unplanned.get(key) != Some(&why) {
        warn!("{dir}: {key} isn't planned: {why}; it goes to the first of its machines that takes it");
        crate::private_io::diagnose(FsPath::new(dir), || format!("{key} isn't planned: {why}"));
        walker.unplanned.insert(key.to_owned(), why);
    }
}

/// What `moves` quotes its waiting `runs` on, hashed: each machine's runs ahead as its quotes read them, its boot, phase, update, link
/// (to 10 MB/s), installs and timings. Bytes still to come count by the GiB, so a download's progress isn't a change.
fn moves_key(runs: &[&str], pictures: &[placement::Picture]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    runs.hash(&mut hasher);
    for picture in pictures {
        let frame = &picture.frame;
        (picture.on.to_string(), &frame.boot_id, &frame.phase, frame.serving_gpus, (frame.link_bytes_per_s / 1e7) as u64, picture.ready_at_ms).hash(&mut hasher);
        (frame.update.as_ref().map(|update| &update.state), frame.environments.len(), frame.resident.len(), frame.models.len(), picture.ahead.len()).hash(&mut hasher);
        for root in frame.queue.iter().flatten() {
            (&root.run, &root.function, root.running, root.gpu_waits, root.gpu_at_unix_ms, root.width, root.bytes_left >> 30).hash(&mut hasher);
        }
        for timed in frame.times.iter().flatten() {
            (&timed.function, timed.runs.len()).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// How many times a quote's read that may answer if asked again is asked, backing off between (1, 2, 4, 8 s).
const QUOTE_ATTEMPTS: u32 = 5;
/// A quote going longer than this says so, and what it's waiting on, in the project's walk log.
const LONG_QUOTE: std::time::Duration = std::time::Duration::from_secs(60);

/// Why a job has no quote; `gave_up`: its reads didn't answer after every attempt, so the job fails at its turn, never waits on.
/// `silent`: the machines of its that didn't answer, which its failure names.
#[derive(Clone)]
struct Unquoted {
    why: String,
    gave_up: bool,
    silent: Vec<String>,
}

/// Something a walker started came to its end.
enum Done {
    Sent { id: usize, key: String, ask: Arc<Ask>, name: String, kind: String, inputs: Option<String>, result: Result<(), Error> },
    Made { id: usize, ask: String, name: String, kind: String, inputs: Option<String>, result: Result<(), Error> },
    Landed { job: String, landed: Followed },
    /// All that was on its way here from a run came, or won't.
    Arrived { name: Option<String>, result: Result<(), Error> },
    Ahead,
    /// A ready job's quote, read off the walk's turn (`Walker::quoting`).
    Quoted { key: String, quotable: Result<placement::Job, Unquoted> },
    /// A waiting job's quote (`Walker::foreseeing`).
    Foreseen { key: String, quotable: Result<placement::Job, Unquoted> },
}

/// What a queued job's machine can fetch before its turn: a function as its text names it, on the machine the plan placed it on.
#[derive(Clone)]
struct Wanted {
    machine: On,
    hub: String,
    with: String,
    kind: String,
    made: Map<String, Value>,
    /// Its box, whose render's request its function's needs rules read (cl-1103).
    asset: String,
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
    Send { id: usize, key: String, ask: Arc<Ask>, doc: Arc<Doc>, stand: Arc<Vec<Standing>>, lock: Arc<Lock>, next: Next },
    /// Its machines are all busy: it waits here for one.
    Held { id: usize },
    /// Its quote is being read: it has its turn once that is in (`Done::Quoted`).
    Quoting { id: usize },
    /// Its quote's reads didn't answer after every attempt: it fails, in one line, and never waits on them (xs-199).
    Unquoted { id: usize, ask: String, on: Vec<On>, key: String, asset: String, kind: String, inputs: Option<String>, message: String, silent: Vec<String> },
    Tool { id: usize, ask: Arc<Ask>, doc: Arc<Doc>, stand: Arc<Vec<Standing>>, lock: Arc<Lock>, i: usize },
}

/// What a walk did not make, and how to ask for it again.
#[derive(Default)]
struct Missed {
    failed: u32,
    unsent: u32,
    names: BTreeSet<String>,
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
            host: Host::for_project(api, &project.id, project.installs.clone()),
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
            n => format!(" and {n} weren't sent"),
        };
        let said = format!("Rendered {made} of {total}: {} failed{unsent}. {}", missed.failed, first.message);
        // What renders the rest again, from where it was asked.
        let what = missed.names.iter().cloned().collect::<Vec<_>>().join(" ");
        let on = missed.on.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let on = if on.is_empty() { String::new() } else { format!(" --on {on}") };
        let mut error = Error::new(first.kind, "render_incomplete", said).cause(first.code);
        if !what.is_empty() {
            error = error.next(format!("cozy render {what}{on}"));
        }
        error.next.extend(first.next.into_iter().take(1));
        error
    }

    /// Records what `ask` is, for the last word.
    fn asked_of(&self, ask: &Ask) {
        let mut missed = self.missed.lock().expect("missed lock");
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
            self.say(Told::Warning { message: format!("{seen} runs on {machine} failed the same way, so this render won't send it anything else: {}.", inner(why)), name: None });
        }
        for pending in lock.pending.values().filter(|pending| pending.run > 0 && asks.contains(&pending.ask)) {
            self.host.cancel_unstarted(pending.run, machine);
        }
        Ok(())
    }

    /// `target`'s machines (cl-1175): those it named; for one that named none, where a render naming none goes now beside where it went when
    /// asked, so a machine added or rented since takes it too: a render waiting for disk room goes to the computer added for it.
    fn wider(&self, target: &Target) -> Vec<On> {
        if !target.any {
            return target.on.clone();
        }
        let api = self.host.api();
        let now = match machines_of(&api, &self.project.id) {
            Ok(pinned) if !pinned.is_empty() => pinned,
            _ => api.machines.automatic(),
        };
        let mut on = target.on.clone();
        on.extend(now.into_iter().filter(|machine| !target.on.contains(machine)));
        on
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
    /// tell me to rent a new one"): with every one gone (ended or ending), where a render naming none goes, said once a render.
    fn live(&self, ask: &str, on: &[On]) -> Vec<On> {
        let api = self.host.api();
        // Each by its name now: a rental asked by a name it had before goes where its order went (cl-1089, cl-1125).
        let on: Vec<On> = (on.iter())
            .map(|on| match on {
                On::Machine(name) => On::Machine(api.calcifer.machine_name_now(name)),
                other => other.clone(),
            })
            .collect();
        let on = on.as_slice();
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
        let automatic = crate::private_io::check_project_placement(&self.project.id, &self.project.root, &api.machines.fallback()).unwrap_or_default();
        if self.fell_back.lock().expect("fell back lock").insert(ask.to_owned()) {
            let set = automatic.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let gone: Vec<&str> = gone.iter().map(|said| inner(said)).collect();
            self.say(Told::Warning { message: format!("{}. Cozy will send this render to whichever of {set} finishes soonest.", gone.join("; ")), name: None });
        }
        automatic
    }

    /// Sizes a function reads are ffprobe's: with files to measure and none here, the walk goes on while Calcifer's start installs it,
    /// or says what goes without and how to install it.
    fn measuring(&self, lock: &Lock) {
        if !self.folder().wants_ffprobe(lock) || self.measuring.swap(true, Ordering::Relaxed) {
            return;
        }
        if crate::tools::installing_ffmpeg() {
            self.say(Told::Said { message: "Installing ffmpeg and ffprobe, which Cozy uses to measure this project's files.".into(), name: None });
        }
        let (api, project) = (self.host.api(), self.project.clone());
        tokio::spawn(async move {
            match Host::new(&api).ffmpeg().await {
                Ok(_) => project.asked.notify_one(),
                Err(error) => project.jobs.told(Told::Warning { message: format!("Cozy has no ffprobe, so tools can't see the sizes of this project's files: {}. Run `cozy doctor --fix`.", inner(&error.message)), name: None }),
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

    /// The digest `renderer`'s release must install (xs-196): the one this project's lock recorded at its first run, else its card's,
    /// recorded now. A card that disagrees with the lock is refused: a published release never changes.
    fn pinned(&self, hub: &str, renderer: &Renderer) -> Result<String, Error> {
        let hub = crate::runs::hub(&self.host.api(), hub);
        let key = format!("{}@{} {hub}", renderer.package, renderer.release);
        let recorded = self.folder().reading_lock(|lock| lock.releases.get(&key).cloned())?;
        match (recorded, renderer.digest.as_str()) {
            (Some(recorded), card) if !card.is_empty() && card != recorded => {
                let said = format!(
                    "{}@{} at {hub} has different files from the release this project used: its digest is now {card}, but the project recorded {recorded}. A published release should never change.",
                    renderer.package, renderer.release
                );
                Err(Error::new(Kind::Conflict, "release_changed", said).remedy("Use another release of this tool, and let its publisher know."))
            }
            (Some(recorded), _) => Ok(recorded),
            (None, "") => Ok(String::new()),
            (None, card) => {
                self.folder().update_lock(|lock| drop(lock.releases.insert(key, card.to_owned())))?;
                Ok(card.to_owned())
            }
        }
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
        // What a delete set aside waited for its renders to read it: looked at now.
        purge::opened(&self.project.id, &self.project.folder);
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
        // The gate, then `walking`, then the writer: its read first, so the writes under `walking` are granted beside it and never queue.
        let _read = vault::read_gate(&self.project.root).await?;
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
                self.say(Told::Warning { message: format!("{}. This computer is offline, so the render will carry on once it's back online.", inner(&e.message)), name: None });
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
        // What it was asked of by a rental's handle, renamed while no walk went, goes to the Hub's name for it (cl-1106).
        if let Err(e) = renders_follow(&self.host.api(), self.folder()) {
            warn!("{}: its renders keep the names they were asked of: {}", self.folder().dir.display(), e.message);
        }
        let (mut first, mut followed) = (None, HashSet::new());
        let mut work: FuturesUnordered<BoxFuture<'_, Done>> = FuturesUnordered::new();
        // No quote is going yet: one an earlier walk left ended with it.
        {
            let mut walker = self.project.walker.lock().expect("walker lock");
            walker.quoting.clear();
            walker.foreseeing.clear();
        }
        // A project's revision moved (this Calcifer's commit, or another program's its folder watch saw): what was deleted stops.
        let mut edits = crate::events::edits();
        // A machine's Status moved (a run started or ended, bytes came, a machine came or went): a job held for a machine, or a run
        // waiting on one, is placed again (cl-1107).
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
            self.arrivals(&mut first)?;
            while self.turns(&mut work, &mut first).await? {}
            for (key, pending) in self.folder().walk_now()?.pending {
                if !self.on_the_way.lock().expect("on the way lock").contains(&key) && followed.insert(key.clone()) {
                    let job = pending.job_of(&key).to_owned();
                    work.push(async move { Done::Landed { job, landed: self.follow(key).await } }.boxed());
                }
            }
            // What waits for another's output is placed too, so its machine fetches ahead for it.
            if !self.project.walker.lock().expect("walker lock").graph.has_ready() {
                let unquoted = self.plan(false).await;
                self.quote_later(unquoted, &mut work);
            }
            self.moves().await;
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
                    let began = std::time::Instant::now();
                    let made = self.item(&key, &wanted).await;
                    crate::private_io::diagnose(&self.folder().dir, || {
                        let said = match &made {
                            Ok(Some((_, function))) => format!("{function} listed"),
                            Ok(None) => "nothing to list".into(),
                            Err(error) => format!("none: {}", error.message),
                        };
                        format!("{key} ahead on {} in {} ms: {said}", wanted.machine, began.elapsed().as_millis())
                    });
                    let item = match made {
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
                    // What's ready but held for a machine (its room, cl-1175; its answer) waits below for one to change, never another turn
                    // at once, which would come round again at once, forever.
                    match (walker.graph.has_ready(), walker.held) {
                        (true, true) => None,
                        (true, false) => continue,
                        (false, _) => {
                            for id in walker.graph.open_jobs() {
                                warn!("{}: nothing is going, and it still waits for {:?}", walker.graph.jobs[id].asset, walker.graph.waits_for(id));
                            }
                            Some(walker.joined.clone())
                        }
                    }
                };
                if let Some(done) = done {
                    for ask in self.folder().walk_now()?.walk.asks.into_iter().filter(|ask| done.contains(&ask.id)) {
                        self.finish(&ask.id).await?;
                    }
                    if self.over(&mut first, following)? {
                        return Ok(());
                    }
                    continue;
                }
            }
            loop {
                tokio::select! {
                    Some(done) = work.next() => {
                        self.done(done, &mut work, &mut followed, &mut first)?;
                        break;
                    }
                    () = self.project.asked.notified() => break,
                    Ok(()) = machines.changed() => {
                        let (held, waits) = {
                            let walker = self.project.walker.lock().expect("walker lock");
                            (walker.held, walker.waits)
                        };
                        if held || (waits && self.moves().await) {
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
        let Walker { graph, readings, placed, foreseen, unable, .. } = &*walker;
        let mut jobs = vec![];
        for id in graph.open_jobs() {
            let job = &graph.jobs[id];
            // One none of whose machines can run it fetches nothing ahead anywhere (cl-1175, cl-1279).
            if job.node == usize::MAX || job.state != walker::State::Waiting || matches!(job.kind, JobKind::Free) || unable.contains_key(&job.key) {
                continue;
            }
            let Some(ask) = walk.walk.asks.iter().find(|ask| ask.id == job.ask) else { continue };
            let Some(reading) = readings.get(&ask.digest) else { continue };
            let target = ask.takes.iter().find(|take| take.key == job.key).map_or(&ask.target, |take| &take.target);
            let asset = &reading.doc.assets[job.node];
            let machine = match target.on.as_slice() {
                [only] if !target.any => Some(only.clone()),
                _ => placed.get(&job.key).map(|placed| placed.soonest.machine.clone()).or_else(|| foreseen.get(&job.key).cloned().flatten()),
            };
            // With `any`, or none placed yet, where isn't known.
            let Some(machine @ On::Machine(_)) = machine else { continue };
            let wanted = Wanted { machine, hub: target.hub.clone(), with: reading.doc.target(job.node), kind: asset.kind.clone(), made: asset.made.clone(), asset: asset.name.clone() };
            jobs.push((job.key.clone(), job.asset.clone(), wanted));
        }
        jobs
    }

    /// Each machine's upcoming jobs of this walk, as far as their items are made, in the order they start (`prefetch::Lists`); their rows
    /// on the board, and only theirs.
    fn list_ahead(&self, jobs: &[(String, String, Wanted)]) {
        let api = self.host.api();
        let mut by_machine: HashMap<String, Vec<crate::prefetch::Listed>> = HashMap::new();
        {
            let walker = self.project.walker.lock().expect("walker lock");
            for (key, asset, wanted) in jobs {
                let On::Machine(name) = &wanted.machine else { continue };
                let Some(Some((hub, item, function))) = walker.items.get(&(name.clone(), key.clone())) else { continue };
                let (start_at_ms, fits) = walker.starts.get(key).cloned().unwrap_or((i64::MAX, vec![]));
                let listed = crate::prefetch::Listed {
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
    /// that makes this kind, with its model choices and what its needs rules say its box's render needs of its models (cl-1103); and
    /// the function, as its row says it. None: its package makes nothing of the kind.
    /// Local code is installed there first, as a call's is. A project's published code and its models are the machine's, as every
    /// project's are (Paul, 11-13); an encrypted project's local code goes with the run that calls it.
    async fn item(&self, key: &str, wanted: &Wanted) -> Result<Option<(v1::Predicted, String)>, Error> {
        let Wanted { machine, hub, with, kind, made, asset } = wanted;
        let package = self.host.package(hub, with, &self.folder().dir, std::slice::from_ref(machine), key, None).await?;
        let Some(renderer) = package.first_making(kind) else { return Ok(None) };
        let source = match &renderer.local {
            None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone(), digest: self.pinned(hub, &renderer)?, ..Default::default() }),
            Some(path) => {
                // Staged in its project's storage scope, as its call's are; an encrypted project's code goes with the run that calls it.
                let scope = crate::private_io::of_path(&self.folder().dir)?;
                if scope.as_ref().is_some_and(|scope| !scope.storage.ordinary) {
                    return Ok(None);
                }
                v1::Source::Local(crate::private_io::context(scope, self.host.ready(path, machine, hub, key, None)).await?.source)
            }
        };
        let asking = Asking::of(self.folder());
        let request = asking.as_ref().and_then(|asking| asking.request(asset, &package, &renderer.function));
        let mut spec = predicted(&renderer, made, request, self.project.draft_seed(asset), source)?;
        // Its LoRAs at the strengths its render's record gives them (cl-1281).
        crate::hub::models::recommended(&self.host.api().machines.hubs, hub, &mut spec.models).await;
        // Its files as its run lists them, which that run writes there: the item is its request to the byte (cl-1123).
        if let Some(asking) = &asking {
            spec.inputs = predicted_inputs(self.folder(), &asking.lock, &spec).0;
        }
        // Local code's interface as read here: its machine resolves its slots while it installs the code (cl-1123).
        let interface = renderer.local.as_ref().and_then(|_| v1::Json::parse(package.document().to_string()).ok());
        Ok(Some((v1::Predicted { key: key.to_owned(), spec, interface }, package.name.clone() + "/" + &renderer.function)))
    }

    /// Job `key` as its machines `on` quote it: the function its tool names, with `made`'s choices, its package read with nothing done on
    /// any machine for it (`job_of`); else why not. `asked`: its box, whose render's request its needs rules read (cl-1103).
    #[allow(clippy::too_many_arguments)]
    async fn quotable(&self, key: String, on: Vec<On>, hub: &str, with: &str, made: Map<String, Value>, asked: Option<(&Asking, &str)>) -> Result<placement::Job, Unquoted> {
        let (began, job, dir) = (std::time::Instant::now(), key.clone(), self.folder().dir.clone());
        let stages = Arc::new(Mutex::new(crate::stages::Stages::default()));
        // A read that may answer if asked again is, as any call (retry-standard-api-practice): backing off, each attempt said.
        let attempts = async {
            let mut attempt = 1;
            loop {
                match job_read(&self.host, &dir, key.clone(), on.clone(), hub, with, made.clone(), asked).await {
                    Err(Unquotable::Again(why)) if attempt < QUOTE_ATTEMPTS => {
                        let wait = std::time::Duration::from_secs(1 << (attempt - 1));
                        warn!("{}: quote of {job}, attempt {attempt}/{QUOTE_ATTEMPTS}: {why}; again in {} s", dir.display(), wait.as_secs());
                        crate::private_io::diagnose(&dir, || format!("quote {job} ({with}) attempt {attempt}/{QUOTE_ATTEMPTS}: {why}; again in {} s", wait.as_secs()));
                        attempt += 1;
                        tokio::time::sleep(wait).await;
                    }
                    Err(Unquotable::Again(why)) => break Err(Unquoted { why: format!("Cozy couldn't get an estimate from its machines after {QUOTE_ATTEMPTS} tries: {}", inner(&why)), gave_up: true, silent: vec![] }),
                    Err(Unquotable::Not(why)) => break Err(Unquoted { why, gave_up: false, silent: vec![] }),
                    Ok((job, _)) => break Ok(job),
                }
            }
        };
        let quoting = crate::stages::recorded(stages.clone(), attempts);
        tokio::pin!(quoting);
        let long = tokio::time::sleep(LONG_QUOTE);
        tokio::pin!(long);
        let mut said_long = false;
        let quoted = loop {
            tokio::select! {
                quoted = &mut quoting => break quoted,
                () = &mut long, if !said_long => {
                    said_long = true;
                    let going = stages.lock().expect("stages lock").said();
                    warn!("{}: quote of {job} still going after {} s: {going}", dir.display(), LONG_QUOTE.as_secs());
                    crate::private_io::diagnose(&dir, || format!("quote {job} ({with}) still going after {} s: {going}", LONG_QUOTE.as_secs()));
                }
            }
        };
        crate::private_io::diagnose(&dir, || {
            let said = match &quoted {
                Ok(quoted) => quoted.function.clone(),
                Err(unquoted) => format!("none: {}", unquoted.why),
            };
            let stages = stages.lock().expect("stages lock").said();
            format!("quote {job} ({with}) in {} ms: {said} ({stages})", began.elapsed().as_millis())
        });
        quoted
    }

    /// Its runs waiting for a machine, in its queue, held here while it updates, or started there on its CPU alone while their GPU work
    /// waits (cl-1107), each moved to another of its machines that would finish it clearly sooner (cl-1073, Paul 11-16: an LTX run
    /// waited 40 minutes in chartreux's queue behind runs still loading their weights while kamitsure could run it): the same run
    /// (`Calcifer::move_to`). Where it waits is quoted as if asked again with what holds that machine now; it leaves only for a finish
    /// clearly sooner (`plan::clearly_sooner`), what it fetched there given up, and never goes back to a machine it left. Whether one
    /// moved. While one waits, each machine's report asks again (`go`).
    async fn moves(&self) -> bool {
        let Ok(walk) = self.folder().walk_now() else { return false };
        let api = self.host.api();
        let quotables = self.project.walker.lock().expect("walker lock").quotables.clone();
        let mut sent_or_held = vec![];
        for (key, pending) in walk.pending.iter().filter(|(_, pending)| pending.run > 0 && pending.target.on.len() > 1) {
            let Some(run) = self.host.numbered(pending.run) else { continue };
            // Accepted by its machine, whose picture says whether it waits there; or held here for it (its machine updating).
            let sent = run.accepted_at_ms.is_some();
            let waits = match sent {
                true => matches!(run.machine_state.as_deref(), Some("queued" | "starting" | "running")),
                false => run.status == RunStatus::Queued && run.started_at_ms.is_none(),
            };
            let waits = waits && run.settled_at_ms.is_none() && !api.calcifer.moving(&run.id);
            let (true, Some(machine), Some(job)) = (waits, run.machine, quotables.get(pending.job_of(key))) else { continue };
            // Between rentals and added machines: a run for this computer is made for it (its project's private storage), and one
            // made for another isn't.
            let on: Vec<On> = self.sending(&pending.ask, &self.wider(&pending.target)).into_iter().filter(|on| !on.is_local()).collect();
            if machine == "local" || on.len() < 2 {
                continue;
            }
            sent_or_held.push((key.clone(), pending.job_of(key).to_owned(), run.id, On::Machine(machine), sent, placement::Job { on, ..job.clone() }));
        }
        let mut pictures = vec![];
        let now = now_ms();
        if !sent_or_held.is_empty() {
            let jobs: Vec<placement::Job> = sent_or_held.iter().map(|(.., job)| job.clone()).collect();
            let machines = placement::machines_of(&jobs);
            let scope = jobs.iter().find_map(|job| job.scope.clone());
            for picture in placement::pictures(&api, &machines, false, now).await.into_iter().flatten() {
                pictures.push(placement::scoped_picture(&api, &picture, scope.as_ref()).await);
            }
        }
        // Of those sent, the ones its machine's picture lists queued, or running on its CPU alone while their GPU work waits. Running
        // with it begun, or on a machine that doesn't say, it stays.
        let waiting: Vec<_> = (sent_or_held.into_iter())
            .filter_map(|(key, of, id, held, sent, job)| {
                let unbegun = match sent {
                    false => false,
                    true => {
                        let queue = pictures.iter().find(|picture| picture.on == held)?.frame.queue.as_ref()?;
                        let root = queue.iter().find(|root| crate::store::calcifer_id(&root.run) == id.as_str())?;
                        if root.running && !root.gpu_waits {
                            return None;
                        }
                        root.running
                    }
                };
                Some((key, of, id, held, sent, unbegun, job))
            })
            .collect();
        self.project.walker.lock().expect("walker lock").waits = !waiting.is_empty();
        if waiting.is_empty() {
            return false;
        }
        // Quoted again only when what a quote reads changed since: with no change, time alone moves no finish against another.
        let seen = moves_key(&waiting.iter().map(|(_, _, id, ..)| id.as_str()).collect::<Vec<_>>(), &pictures);
        if std::mem::replace(&mut self.project.walker.lock().expect("walker lock").moves_seen, seen) == seen {
            return false;
        }
        let jobs: Vec<placement::Job> = waiting.iter().map(|(.., job)| job.clone()).collect();
        placement::benched(&api, &jobs, pictures.iter_mut());
        let frames: Vec<&api::machine::StatusFrame> = pictures.iter().map(|picture| &picture.frame).collect();
        let mut moved = false;
        for (key, of, id, held, sent, unbegun, job) in waiting {
            let left: Vec<On> = self.project.walker.lock().expect("walker lock").left.get(&of).cloned().unwrap_or_default();
            let mut offers = vec![];
            for picture in pictures.iter().filter(|picture| job.on.contains(&picture.on) && (picture.on == held || !left.contains(&picture.on))) {
                let mut picture = picture.clone();
                if picture.on == held && sent {
                    picture.frame.queue.get_or_insert_default().retain(|root| crate::store::calcifer_id(&root.run) != id.as_str());
                }
                let mut ask = placement::asked(&job, placement::prior(&api, &picture, &job).await);
                api::estimate::learn_reads(&frames, &mut ask);
                offers.extend(placement::quote(&picture, &[ask], now));
            }
            let Some(here) = offers.iter().find(|offer| offer.machine == held && offer.fits) else { continue };
            let Some(sooner) = placement::soonest(&offers, now).filter(|offer| offer.machine != held && plan::clearly_sooner(offer, here, now)) else { continue };
            let to = sooner.machine.to_string();
            if !api.calcifer.move_to(&id, &to, unbegun) {
                continue;
            }
            moved = true;
            let said = offers.iter().map(|offer| placement::said(offer, now)).collect::<Vec<_>>().join("; ");
            info!("{}: {key} moves from {held} to {to}: {said}", self.folder().dir.display());
            crate::private_io::diagnose(&self.folder().dir, || format!("{key} moves from {held} to {to}: {said}"));
            self.host.placed(&self.project.id, &id, &sooner.placed());
            self.host.quoted(&id, sooner.quoted());
            {
                let mut walker = self.project.walker.lock().expect("walker lock");
                walker.left.entry(of.clone()).or_default().push(held);
                walker.sent.insert(of, sooner.clone());
            }
        }
        moved
    }

    /// Places the jobs not sent yet, the nearest `plan::PLANNED` of them (cl-967): every machine pictured from the Status it pushed, each
    /// job beginning once what it reads ends, ahead of it on each machine the runs sent there that it hasn't accepted yet. A ready job's
    /// place is where it goes at its turn; one waiting for another's output, where its weights are fetched ahead, else the first of its
    /// machines, as a ready one goes at its turn. A ready job waits for its package to say what it calls, but not for its install, which
    /// would hold every job of the turn (owner, cl-973). Returned to be quoted in the walk's work (`quote_later`): the ready jobs whose
    /// install is going, and the waiting jobs not quoted yet, whose package is read statically (cl-905: its install neither holds its
    /// quote nor waits for a wake to place it). Each ready job's line says where it goes and why, or why no machine was planned: then it
    /// goes to the first of its machines that takes it. `update`: as `placement::pictures`.
    async fn plan(&self, update: bool) -> Vec<Unsent> {
        let Ok(walk) = self.folder().walk_now() else { return vec![] };
        let dir = self.folder().dir.display().to_string();
        let (unsent, ends, foreseen) = {
            let walker = self.project.walker.lock().expect("walker lock");
            let graph = &walker.graph;
            let ready: HashSet<usize> = graph.ready(usize::MAX).into_iter().collect();
            let unsent: Vec<Unsent> = (graph.open_jobs().into_iter())
                .filter(|&id| graph.jobs[id].state == walker::State::Waiting)
                .filter_map(|id| {
                    let job = &graph.jobs[id];
                    if job.node == usize::MAX || matches!(job.kind, JobKind::Free) {
                        return None;
                    }
                    let ask = walk.walk.asks.iter().find(|ask| ask.id == job.ask)?;
                    let target = ask.takes.iter().find(|take| take.key == job.key).map_or(&ask.target, |take| &take.target);
                    let reading = walker.readings.get(&ask.digest)?;
                    let asset = reading.doc.assets.get(job.node)?;
                    let (with, made) = (reading.doc.target(job.node), asset.made.clone());
                    Some(Unsent { id, key: job.key.clone(), asset: job.asset.clone(), ask: ask.id.clone(), target: target.clone(), with, made, needs: job.needs.clone(), ready: ready.contains(&id) })
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
            return vec![];
        }
        // What each asks of a machine, read once, in the walk's work: a ready job's from its package now, a waiting one's statically.
        // None is read on this turn, which would hold every job, landing and arrival of the walk while one read waits (xs-199).
        let (known, foresee): (HashSet<String>, HashSet<String>) = {
            let walker = self.project.walker.lock().expect("walker lock");
            let known = walker.quotables.keys().chain(&walker.quoting).chain(walker.unquoted.keys()).cloned().collect();
            let foresee = (unsent.iter().filter(|unsent| !unsent.ready))
                .filter(|unsent| !walker.quotables.contains_key(&unsent.key) && !walker.foreseeing.contains(&unsent.key))
                .map(|unsent| unsent.key.clone())
                .collect();
            (known, foresee)
        };
        let quoting: HashSet<String> = unsent.iter().filter(|unsent| unsent.ready && !known.contains(&unsent.key)).map(|unsent| unsent.key.clone()).collect();
        let quotables = {
            let mut walker = self.project.walker.lock().expect("walker lock");
            let pending: HashSet<&str> = walk.pending.iter().map(|(key, pending)| pending.job_of(key)).collect();
            walker.quotables.retain(|key, _| pending.contains(key.as_str()) || unsent.iter().any(|unsent| unsent.key == *key));
            walker.quotables.clone()
        };
        let jobs: Vec<(usize, placement::Job)> = (unsent.iter().enumerate())
            .filter_map(|(at, unsent)| {
                let job = quotables.get(&unsent.key)?;
                Some((at, placement::Job { on: self.sending(&unsent.ask, &self.wider(&unsent.target)), ..job.clone() }))
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
        // `local` as this project sees it: what its own runs installed there, read in its scope, as the Render estimate does (cl-1047).
        let scope = only.iter().find_map(|job| job.scope.clone());
        for (on, pictured) in machines.iter().zip(placement::pictures(&api, &machines, update, now).await) {
            match pictured {
                Ok(picture) => pictures.push(placement::scoped_picture(&api, &picture, scope.as_ref()).await),
                Err(why) => {
                    warn!("{dir}: {on} isn't planned on: {why}");
                    let name = on.to_string();
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    if walker.silent.get(&name) != Some(&why) {
                        crate::private_io::diagnose(&self.folder().dir, || format!("{name} isn't planned on: {why}"));
                        walker.silent.insert(name, why);
                    }
                }
            }
        }
        for (on, job) in &ahead {
            if let Some(m) = plan::index(&pictures, on) {
                let prior = placement::prior(&api, &pictures[m], job).await;
                pictures[m].ahead.push(placement::asked(job, prior));
            }
        }
        // What each function never ran on a machine is timed by its release's benchmarks there (cl-1024).
        placement::benched(&api, jobs.iter().map(|(_, job)| job).chain(ahead.iter().map(|(_, job)| job)), pictures.iter_mut());
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
                ask: placement::asked(job, placement::Prior::default()),
                cost: job.cost.clone(),
                priors,
                machines,
                needs: unsent.needs.iter().filter_map(|need| position.get(need).copied()).collect(),
                after_at_ms: unsent.needs.iter().filter_map(|need| ends.get(need).copied()).max(),
                // Where its weights are fetched ahead: for this walk, else for its box's guess (cl-1104), so the machine a guess made
                // ready is where its render goes unless another finishes it clearly sooner.
                stay: (foreseen.get(&unsent.key).cloned().flatten())
                    .or_else(|| api.machines.lists.guessed_on(&self.folder().dir.to_string_lossy(), &unsent.asset).map(On::Machine))
                    .and_then(|on| plan::index(&pictures, &on)),
            });
        }
        // What each job's function read of its models on any machine: what it fetches on one that holds none of them (cl-967).
        let frames: Vec<&api::machine::StatusFrame> = pictures.iter().map(|picture| &picture.frame).collect();
        for job in &mut planned {
            api::estimate::learn_reads(&frames, &mut job.ask);
        }
        let placed = plan::plan(&pictures, &planned, now);
        let said = |offers: &[Offer]| offers.iter().map(|offer| placement::said(offer, now)).collect::<Vec<_>>().join("; ");
        // What a job none of whose machines can run it says, once each time it changes (cl-1175, cl-1279).
        let (mut roomless, mut rooms) = (vec![], false);
        let mut walker = self.project.walker.lock().expect("walker lock");
        for ((at, job), placement) in jobs.iter().zip(placed) {
            let unsent = &unsent[*at];
            // Where a waiting job is fetched ahead, said when that changes.
            let ahead = |walker: &mut Walker, on: &On, planned: bool| {
                if walker.foreseen.insert(unsent.key.clone(), Some(on.clone())).flatten().as_ref() != Some(on) {
                    let how = if planned { "planned" } else { "no machine planned" };
                    crate::private_io::diagnose(&self.folder().dir, || format!("{} waits; fetched ahead on {on}, {how}: {}", unsent.key, said(&placement.offers)));
                }
            };
            let Some((_, chosen)) = placement.chosen else {
                walker.placed.remove(&unsent.key);
                walker.starts.remove(&unsent.key);
                // A waiting job none of its machines was planned for goes where a ready one would at its turn: the first of them that can
                // run it, which fetches ahead for it.
                let able = |on: &&On| !placement.offers.iter().any(|offer| offer.unable() && offer.machine == **on);
                match job.on.iter().find(able).filter(|_| !unsent.ready) {
                    Some(first) => ahead(&mut walker, first, false),
                    None => drop(walker.foreseen.remove(&unsent.key)),
                }
                // One waiting for its inputs that none of its machines can run says so on its box, and nothing of it goes ahead.
                if let (false, None, Some(held)) = (unsent.ready, job.on.iter().find(able), placement::unable(&placement.offers)) {
                    rooms |= walker.unable.insert(unsent.key.clone(), held.clone()).is_none_or(|was| placement::unable_line(&was, "render") != placement::unable_line(held, "render"));
                    continue;
                }
                // None of its machines can run it: no room for its weights (cl-1175, Paul 11-17: "stop them from trying to download
                // models they cannot fit"), or no GPU where nothing measured says it runs without one (cl-1279). It waits here, nothing
                // of it sent, until one can, or a rental comes; said once.
                if let (true, Some(held)) = (unsent.ready, placement::unable(&placement.offers)) {
                    walker.waiting.remove(&unsent.key);
                    let line = placement::unable_line(held, "render");
                    match walker.unable.insert(unsent.key.clone(), held.clone()) {
                        None => {
                            info!("{dir}: {} waits for a machine that can run it: {}", unsent.key, said(&placement.offers));
                            roomless.push(Told::Warning { message: format!("{}: {line}", unsent.asset), name: Some(unsent.asset.clone()) });
                            rooms = true;
                        }
                        Some(was) => rooms |= placement::unable_line(&was, "render") != line,
                    }
                    continue;
                }
                if unsent.ready {
                    // Every one of its machines connected and not answering its frame: it waits while they're asked again, placed once one
                    // answers, and fails, naming them, once none has after every attempt (xs-199). Never sent to one that doesn't answer.
                    let silent: Vec<(String, String, bool)> = (job.on.iter())
                        .filter_map(|on| match on {
                            On::Machine(name) => api.machines.not_answering(name).map(|(why, dropped)| (name.clone(), why, dropped)),
                            _ => None,
                        })
                        .collect();
                    if !silent.is_empty() && silent.len() == job.on.len() {
                        let said = silent.iter().map(|(name, why, _)| format!("{name}: {why}")).collect::<Vec<_>>().join("; ");
                        match silent.iter().all(|(_, _, dropped)| *dropped) {
                            true => {
                                walker.waiting.remove(&unsent.key);
                                let silent = silent.into_iter().map(|(name, ..)| name).collect();
                                walker.unquoted.insert(unsent.key.clone(), Unquoted { why: format!("none of the machines it can use is answering ({said})"), gave_up: true, silent });
                            }
                            false => drop(walker.waiting.insert(unsent.key.clone())),
                        }
                        unplanned(&mut walker, &dir, &unsent.key, format!("none of its machines answers: {said}"));
                        continue;
                    }
                    walker.waiting.remove(&unsent.key);
                    let why = match placement.offers.is_empty() {
                        true => "none of its machines answered".to_owned(),
                        false => said(&placement.offers),
                    };
                    unplanned(&mut walker, &dir, &unsent.key, why);
                }
                continue;
            };
            walker.waiting.remove(&unsent.key);
            let was_held = walker.unable.remove(&unsent.key);
            rooms |= was_held.is_some();
            let fits = placement.offers.iter().filter(|offer| offer.fits).map(|offer| offer.machine.to_string()).collect();
            walker.starts.insert(unsent.key.clone(), (chosen.start_at_ms, fits));
            if !unsent.ready {
                ahead(&mut walker, &chosen.machine, true);
                continue;
            }
            // Where each job goes, said when that changes: what placement weighed, for anyone asking why a job went where it did.
            if walker.placed.get(&unsent.key).is_none_or(|was| was.soonest.machine != chosen.machine) {
                let quoted = said(&placement.offers);
                info!("{dir}: {} goes to {}: {quoted}", unsent.key, chosen.machine);
                crate::private_io::diagnose(&self.folder().dir, || format!("{} goes to {}: {quoted}", unsent.key, chosen.machine));
            }
            // Elsewhere for a disk without room for it, as its machines quote it now or as it waited for room: said in a line, once.
            let short = placement::roomless(&placement.offers).or(was_held.as_ref().filter(|held| held.short_bytes > 0)).filter(|short| short.machine != chosen.machine);
            if let Some(short) = short.filter(|_| was_held.is_some() || walker.placed.get(&unsent.key).is_none_or(|was| was.soonest.machine != chosen.machine)) {
                let message = format!("{}: {}", unsent.asset, placement::elsewhere_line(short, &chosen.machine, "render"));
                roomless.push(Told::Said { message, name: Some(unsent.asset.clone()) });
            }
            walker.unplanned.remove(&unsent.key);
            walker.placed.insert(unsent.key.clone(), Placed { offers: placement.offers, soonest: chosen });
        }
        for unsent in unsent.iter().filter(|unsent| unsent.ready) {
            if let Some(Unquoted { why, gave_up: false, .. }) = walker.unquoted.get(&unsent.key).cloned() {
                walker.placed.remove(&unsent.key);
                unplanned(&mut walker, &dir, &unsent.key, why);
            }
        }
        walker.placed.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.unplanned.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.foreseen.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.starts.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.items.retain(|(_, key), _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.foreseeing.retain(|key| unsent.iter().any(|unsent| unsent.key == *key));
        walker.unquoted.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        walker.waiting.retain(|key| unsent.iter().any(|unsent| unsent.key == *key));
        let before = walker.unable.len();
        walker.unable.retain(|key, _| unsent.iter().any(|unsent| unsent.key == *key));
        rooms |= walker.unable.len() != before;
        drop(walker);
        roomless.into_iter().for_each(|told| self.say(told));
        // The page shows a job waiting for a machine that can run it on its box, from the project's queue.
        if rooms {
            crate::events::project_settled(&self.folder().dir);
        }
        unsent.into_iter().filter(|unsent| if unsent.ready { quoting.contains(&unsent.key) } else { foresee.contains(&unsent.key) }).collect()
    }

    /// Quotes `unquoted` in the walk's work, each once (`plan`), on the machines its render may go to: a ready job's (`Walker::quoting`)
    /// and a waiting job's (`Walker::foreseeing`) alike, its package read with nothing done on any machine for it (cl-1115: quoting a
    /// box installed its local code on this computer though its project renders on a rental alone). The plan after each places it, and
    /// its call makes its code ready where it goes.
    fn quote_later<'a>(&'a self, unquoted: Vec<Unsent>, work: &mut FuturesUnordered<BoxFuture<'a, Done>>) {
        // What each box's render would send, which its function's needs rules read (cl-1103): the project as it stands, read once.
        let asking = (!unquoted.is_empty()).then(|| Asking::of(self.folder())).flatten().map(Arc::new);
        for unsent in unquoted {
            {
                let mut walker = self.project.walker.lock().expect("walker lock");
                let going = if unsent.ready { &mut walker.quoting } else { &mut walker.foreseeing };
                going.insert(unsent.key.clone());
            }
            let asking = asking.clone();
            work.push(async move {
                let (key, hub, with) = (&unsent.key, &unsent.target.hub, &unsent.with);
                let asked = asking.as_deref().map(|asking| (asking, unsent.asset.as_str()));
                let on = self.sending(&unsent.ask, &self.wider(&unsent.target));
                let quotable = self.quotable(key.clone(), on, hub, with, unsent.made.clone(), asked).await;
                match unsent.ready {
                    true => Done::Quoted { key: unsent.key, quotable },
                    false => Done::Foreseen { key: unsent.key, quotable },
                }
            }.boxed());
        }
    }

    /// Of `on`, where job `job` goes, as the last plan placed it (`pick`).
    async fn chosen(&self, job: &str, on: &[On], hub: &str, package: &Package, renderer: &Renderer, asset: &format::Asset) -> Result<(On, Option<Offer>), Error> {
        let (placed, quotable) = {
            let walker = self.project.walker.lock().expect("walker lock");
            (walker.placed.get(job).cloned(), walker.quotables.get(job).cloned())
        };
        let planned = placed.as_ref().map(|placed| placed.soonest.clone()).filter(|offer| on.contains(&offer.machine));
        let offers: Vec<Offer> = placed.map(|placed| placed.offers).unwrap_or_default().into_iter().filter(|offer| on.contains(&offer.machine)).collect();
        let (chosen, offer, said) = pick(&self.host.api(), on, &offers, planned.as_ref(), quotable.as_ref(), hub, Some(package), &renderer.function, &asset.made).await?;
        if let Some(message) = said {
            self.say(Told::Warning { message, name: Some(asset.name.clone()) });
        }
        Ok((chosen, offer))
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
        let (dropped, _) = stop_jobs(&self.host.api(), &self.project, &|name, _| gone.contains(&name_key(name)), &deleted_why)?;
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
        // Their quotes wait for their packages' installs off this turn: each has its turn once its quote is in (owner, cl-973). A
        // waiting job's is read statically meanwhile, so its machine fetches ahead (cl-905).
        let unquoted = self.plan(true).await;
        self.quote_later(unquoted, work);
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
                // With several machines it stays here while its planned machine has no GPU for it, or is still on its way (a rental
                // booting, whose boot counts as its preparation: no wait past it), so nothing commits early to the wrong queue or to a
                // machine that may never serve; it is placed again as their Status moves, or once that machine serves or ends.
                if ask.target.on.len() > 1 && !matches!(kind, JobKind::Free) && placed.get(&key).is_some_and(|placed| placed.soonest.ready_at_ms.is_some() || placed.soonest.waits_ms(now_ms()) > 0) {
                    acts.push(Act::Held { id });
                    continue;
                }
                let turn = graph.turn(id, &mut Now { reading, lock: &lock });
                let noun = if reading.nodes[node].video { "take" } else { "version" };
                let row_of = |reading: &mut AskedText| {
                    reading.ensure(node, &lock);
                    reading.done[node].clone()
                };
                match turn {
                    Turn::Wait => {}
                    Turn::Unread { read } => {
                        let standing = row_of(reading);
                        let doc = reading.doc.clone();
                        let stand = reading.cone(node, &lock);
                        let why = match gone(&reading.nodes[read].name) && !stand[read].files.is_some() {
                            true => format!("{} was deleted", reading.nodes[read].name),
                            false => unread(&doc, &stand, &ask, node),
                        };
                        let message = format!("{asset}: {}.", inner(&why));
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
                            JobKind::Free if standing.inputs.is_none() => {
                                let message = format!("{asset}: {}.", unread(&doc, &stand, &ask, node));
                                let kind = doc.assets[node].kind.clone();
                                acts.push(Act::Unread { id, ask: ask_id, on: ask.target.on.clone(), key, asset, kind, inputs: None, message });
                            }
                            JobKind::Free if doc.makes(node) => acts.push(Act::Tool { id, ask, doc, stand, lock: lock.clone(), i: node }),
                            JobKind::Free => {
                                let number = standing.version();
                                acts.push(Act::Stands { id, ask: ask_id, key, asset, noun, number });
                            }
                            JobKind::Take => {
                                let take = ask.takes.iter().find(|take| take.key == key).cloned();
                                let next = Next { i: node, key: key.clone(), take };
                                acts.push(Act::Send { id, key, ask, doc, stand, lock: lock.clone(), next });
                            }
                        }
                    }
                }
            }
        }
        // What can start now goes without a job whose quote is being read: that one goes once it's in, with what's ready then (owner,
        // cl-973). One whose quote couldn't be had after every attempt fails now.
        {
            let walker = self.project.walker.lock().expect("walker lock");
            for act in &mut acts {
                if let Act::Send { id, key, .. } = act
                    && walker.quoting.contains(key.as_str())
                {
                    let id = *id;
                    *act = Act::Quoting { id };
                } else if let Act::Send { id, key, .. } = act
                    && (walker.waiting.contains(key.as_str()) || walker.unable.contains_key(key.as_str()))
                {
                    let id = *id;
                    *act = Act::Held { id };
                } else if let Act::Send { id, key, ask, doc, stand, next, .. } = act
                    && let Some(Unquoted { why, gave_up: true, silent }) = walker.unquoted.get(key.as_str())
                {
                    let asset = doc.assets[next.i].name.clone();
                    let message = format!("{asset}: {}.", inner(why));
                    *act = Act::Unquoted {
                        id: *id,
                        ask: ask.id.clone(),
                        on: ask.target.on.clone(),
                        key: key.clone(),
                        kind: doc.assets[next.i].kind.clone(),
                        inputs: stand[next.i].inputs.clone(),
                        asset,
                        message,
                        silent: silent.clone(),
                    };
                }
            }
        }
        // Only what moved counts: a job held for a machine that isn't free yet waits for the next wake, not for another turn now, which
        // would come round again at once, forever, with nothing it sent ever followed.
        let moved = acts.iter().any(|act| !matches!(act, Act::Held { .. } | Act::Quoting { .. }));
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
            Act::Unread { id, ask, on, key, asset, kind, inputs, message } => {
                self.settled(&ask, &asset, &key)?;
                match self.sending(&ask, &on).is_empty() {
                    true => self.not_sent(&asset),
                    false => self.failed_of(&asset, &kind, inputs, None, Error::new(Kind::NotFound, "file_missing", message), first),
                }
                end(id, End::Failed);
            }
            Act::Unquoted { id, ask, on, key, asset, kind, inputs, message, silent } => {
                self.settled(&ask, &asset, &key)?;
                let mut error = Error::new(Kind::Unavailable, "quote_unavailable", message);
                if let Some(first) = silent.first() {
                    error = error.remedy(format!("Check what {first} is doing, end it and rent another machine, or render on other machines.")).next(format!("cozy machine show {first}"));
                }
                match self.sending(&ask, &on).is_empty() {
                    true => self.not_sent(&asset),
                    false => self.failed_of(&asset, &kind, inputs, None, error, first),
                }
                end(id, End::Failed);
            }
            Act::Held { id } | Act::Quoting { id } => self.project.walker.lock().expect("walker lock").graph.put_back(id),
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
            // Quoted: the next plan places it, and it has its turn. Not: that plan asks again, and says why.
            Done::Quoted { key, quotable } => {
                let mut walker = self.project.walker.lock().expect("walker lock");
                walker.quoting.remove(&key);
                match quotable {
                    Ok(job) => drop(walker.quotables.insert(key, job)),
                    Err(unquoted) => drop(walker.unquoted.insert(key, unquoted)),
                }
            }
            // Read: the next plan places it, and its machine fetches ahead; one its turn read already stands. Not: it isn't asked again
            // while it waits, and its turn reads it as any.
            Done::Foreseen { key, quotable } => match quotable {
                Ok(job) => {
                    self.project.walker.lock().expect("walker lock").quotables.entry(key).or_insert(job);
                }
                Err(Unquoted { why, .. }) => crate::private_io::diagnose(&self.folder().dir, || format!("{key} isn't fetched ahead: {why}")),
            },
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
                    self.say(Told::Said { message: format!("{name}: {}. Cozy is sending it to another machine.", inner(&error.message)), name: Some(name.clone()) });
                    self.project.walker.lock().expect("walker lock").graph.put_back(id);
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
            Done::Arrived { name, result } => {
                if let Err(error) = result {
                    match &name {
                        Some(name) => {
                            self.project.walker.lock().expect("walker lock").moved(name);
                            self.failed_of(name, "", None, None, error, first);
                        }
                        None => self.failed(error, first),
                    }
                }
            }
            Done::Landed { job, landed } => {
                let failed = landed.landed.as_ref().is_ok_and(Result::is_err);
                let asset = landed.name.clone();
                {
                    let mut walker = self.project.walker.lock().expect("walker lock");
                    walker.sent.remove(&job);
                    walker.left.remove(&job);
                }
                if let Some((id, version)) = self.landed(landed, followed, first)? {
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
    fn landed(&self, landed: Followed, followed: &mut HashSet<String>, first: &mut Option<Error>) -> Result<Option<(RunId, Option<Coming>)>, Error> {
        followed.remove(&landed.key);
        match landed.landed? {
            Ok(behind) => Ok(behind),
            Err(error) => {
                match &landed.name {
                    Some(name) => self.failed_of(name, "", landed.inputs, landed.run, error, first),
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
        // The gate, then `walking`, then the writer: its read first, so its end's writes are granted beside it. Refused (a lock closing
        // the project), it goes on, its writes refused at once.
        let _read = vault::read_gate_blocking(&self.project.root).ok();
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
    /// it goes on. Canceling is said, and isn't a failure of the asset. Its take is gone with it: nothing of a failed take is kept (cl-1070).
    fn failed_of(&self, name: &str, kind: &str, inputs: Option<String>, run: Option<u64>, error: Error, first: &mut Option<Error>) {
        {
            let mut missed = self.missed.lock().expect("missed lock");
            missed.failed += 1;
            missed.names.insert(name.to_owned());
        }
        if error.code != CANCELED {
            let failed = Failed { inputs, error: error.clone(), run };
            let kept = self.folder().update_lock(|lock| lock.asset_mut(name, kind).failed = Some(failed.clone()));
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
                // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
                let _access = access.here();
                tools::timeline(&ffprobe, &project.folder, &pieces, fps, &cut)
            }).await.map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))?
        }.await;
        if let Err(error) = written {
            self.say(Told::Warning { message: format!("{name}'s timeline file is gone, and Cozy couldn't write it again: {}.", inner(&error.message)), name: None });
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
        let (mut target, approve) = match &next.take {
            None => (ask.target.clone(), false),
            Some(take) => (take.target.clone(), take.approve),
        };
        target.on = self.sending(&ask.id, &self.wider(&target));
        if target.on.is_empty() {
            self.not_sent(&name);
            return self.folder().update_walk(|walk| settle(&mut walk.walk));
        }
        let standing = &stand[i];
        let noun = noun(doc, lock, i);
        let sent = async {
            let inputs = standing.inputs.clone().ok_or_else(|| cannot(&name, "something it reads isn't ready yet".into()))?;
            for read in &standing.reads {
                let gone: Vec<&str> = (read.files.values())
                    .filter(|sha256| lock.files.get(*sha256).is_none_or(|file| self.folder().missing(file)))
                    .map(|sha256| folder::prefix_of(sha256))
                    .collect();
                if !gone.is_empty() {
                    return Err(cannot(&name, format!("{} of {} isn't on this computer. Open the project in the browser to download it", gone.join(", "), read.name)));
                }
            }
            let with = doc.target(i);
            // Read for the machine its plan sends it to: its code is made ready there, never first on another of its machines (cl-1115).
            let planned = member.on().filter(|on| target.on.contains(on)).map_or_else(|| target.on.clone(), |on| vec![on]);
            let package = self.again(|| self.host.package(&target.hub, &with, &self.folder().dir, &planned, &key, Some(&member))).await?;
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
                return Err(Error::new(Kind::Validation, "input_missing", why).remedy(format!("Bring it back, or edit {name}'s text.")));
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
            let unknown = || Error::new(Kind::NotFound, "callable_unknown", format!("{} has no function called {}.", package.name, maker.function));
            let callable = package.callable(&maker.function).ok_or_else(unknown)?;
            let fields = doc.inputs_of(i);
            let (reserved, title) = (package.reserved(), package.title());
            let call = binder::Call { name: &name, tool: &doc.target(i), package: &package.name, title: &title, callable: &callable, fields: &fields, reserved: &reserved, doc, own: Some(i), lock, stand, gone: &gone, pickup: pickup.as_ref(), inline: &[] };
            let mut bound = binder::bind(&call)?;
            self.cut(&mut bound, lock).await.map_err(|why| cannot(&name, why))?;
            let (input, mut warnings, unused) = (bound.payload, bound.warnings, bound.unused);
            warnings.dedup();
            // A function with no seed has one result per inputs, as has an asset that names one.
            let fixed = input.contains_key("seed");
            let seeded = !fixed && maker.accepts.contains("seed");
            if !seeded && pickup.is_none() && (holds.is_some() || going) {
                let stands = holds.map_or_else(|| "one is already being made".to_owned(), |number| format!("{noun} {number} stays"));
                // A fixed seed is the box's (or its tool's), never the package's.
                let why = if fixed { "its seed is fixed".to_owned() } else { format!("{} takes no seed", package.name) };
                match holds {
                    Some(number) => self.stands(&name, noun, number, format!("{why}, so the same inputs always give the same result")),
                    None => self.say(Told::Said { message: format!("{name}: {why}, so the same inputs always give the same result, and {stands}."), name: Some(name.clone()) }),
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
                // Else the seed its draft was predicted with (cl-1123): the request its machine made ready is the one it is sent, and
                // that item stays listed for it, whatever the box is guessed as before it is sent.
                version.seed = Some(tried.unwrap_or_else(|| {
                    self.host.api().machines.lists.taking(&self.folder().dir.to_string_lossy(), &name);
                    self.project.take_draft_seed(&name)
                }));
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
            let words = (asset.kind == format::TEXT).then(|| api::calcifer::tool::text_field(&callable).map(str::to_owned)).flatten();
            let pending = Pending { job: key.clone(), kind: asset.kind.clone(), ask: ask.id.clone(), asset: name.clone(), version, run: 0, approve, words, target: target.clone() };
            let settling = |walk: &mut folder::WalkFile| settle(&mut walk.walk);
            if now.asset(&name).is_some_and(|held| held.failed.is_some()) {
                self.folder().update_lock(|lock| {
                    if let Some(held) = lock.assets.get_mut(&name_key(&name)) {
                        held.failed = None;
                    }
                })?;
            }
            // Canceled meanwhile: not sent. `submit` asks again as it records its run on its way, in the same write a cancel makes.
            let stands = |owed: &folder::Owed| owed.asks.iter().find(|mine| mine.id == ask.id).is_some_and(|mine| match &next.take {
                Some(take) => mine.takes.iter().any(|still| still.key == take.key),
                None => !mine.sent.contains(&name_key(&name)),
            });
            let now = self.folder().lock()?;
            let mine = now.walk.asks.iter().find(|mine| mine.id == ask.id);
            if !stands(&now.walk) {
                return Ok(());
            }
            // Renamed meanwhile: what this read says what was, so its turn comes again.
            if mine.is_some_and(|mine| mine.digest != ask.digest) {
                self.project.walker.lock().expect("walker lock").again.insert(key.clone());
                return Ok(());
            }
            // Its machines stopped while it was on its way.
            let mut pending = pending;
            pending.target.on = self.sending(&ask.id, &self.wider(&pending.target));
            if pending.target.on.is_empty() {
                self.not_sent(&name);
                return self.folder().update_walk(|walk| settle(&mut walk.walk));
            }
            self.submit(&key, input, &package, asset, pending, &settling, &stands, member, &unused).await
        };
        let sent = sent.await;
        match &sent {
            // Canceled while it waited for its install, which ended with it if nothing else waited on it: nothing sent, nothing failed.
            Err(e) if e.code == host::CANCELED => return Ok(()),
            Err(_) => self.folder().update_walk(|walk| settle(&mut walk.walk))?,
            Ok(()) => {}
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
            let file = lock.files.get(&range.source).ok_or_else(|| format!("{} isn't one of this project's files", folder::prefix_of(&range.source)))?;
            let path = self.folder().path(&file.path);
            if !path.is_file() || file.fps <= 0.0 {
                return Err(format!("to cut a range of {}, Cozy needs it on this computer and measured. Open the project in the browser to download it", folder::prefix_of(&range.source)));
            }
            let mut notes = vec![];
            let ffmpeg = clips.ffmpeg().await.map_err(|e| e.message)?;
            let access = vault::admit_work(&self.project).await.map_err(|e| e.message)?;
            let made = clips.cut(&range.range.to_string(), &path, range.range, file.fps, &mut notes, ffmpeg, access).await.map_err(|e| e.message)?;
            notes.into_iter().filter(|note| note.level == api::calcifer::NoteLevel::Warning).for_each(|note| self.say(Told::Warning { message: note.message, name: None }));
            let clip = made.clip.clone();
            let access = vault::admit_work(&self.project).await.map_err(|e| e.message)?;
            let sha256 = tokio::task::spawn_blocking(move || {
                // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
                let _access = access.here();
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
    async fn submit(&self, key: &str, mut payload: Map<String, Value>, package: &Package, asset: &format::Asset, mut pending: Pending, settle: &impl Fn(&mut folder::WalkFile), stands: &impl Fn(&folder::Owed) -> bool, member: host::Member, unused: &[Unused]) -> Result<(), Error> {
        let renderer = package.maker()?.ok_or_else(|| cannot(&asset.name, "its tool has no function to run".into()))?;
        let (on, hub) = (pending.target.on.clone(), pending.target.hub.clone());
        let name = asset.name.clone();
        let named = |e: Error| Error { message: format!("{name}: {}", e.message), ..e };
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
                message: format!("{name}: Cozy ignored {}, which this tool doesn't use.", choices.dropped.join(", ")),
            }],
        };
        warnings.extend(choices.replaced.iter().map(|replaced| Warning {
            code: "model_slot_replaced".into(),
            message: format!("{name}: {replaced}"),
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
                let length = std::fs::metadata(&path).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("Cozy couldn't read {}: {e}.", path.display())))?.len();
                inputs.push(v1::InputFile { field: field.clone(), digest: digest.clone(), length, media_type: media_type(&path).to_owned(), order: order_of(&field) });
                staged.push(StagedInput { path, digest: digest.parse().map_err(|why: &str| Error::new(Kind::Internal, "digest", why))?, length });
                continue;
            };
            match self.reach(file, holder.as_ref().map_or(on.as_slice(), std::slice::from_ref)).await {
                Reach::Here => {}
                Reach::There(machine) => {
                    self.say(Told::Said { message: format!("{name}: {field} is already on {machine}, so it renders there."), name: Some(name.clone()) });
                    holder = Some(machine);
                    on_machine = true;
                }
                Reach::Fetch => {
                    self.say(Told::Said { message: format!("{name}: downloading {field} from {}.", file.machine), name: Some(name.clone()) });
                    let began = std::time::Instant::now();
                    let fetched = self.fetching(&digest, &file.machine).await;
                    crate::private_io::diagnose(&self.folder().dir, || format!("{key} waited {} ms for {field} from {}", began.elapsed().as_millis(), file.machine));
                    fetched.map_err(|why| cannot(&name, format!("couldn't download {field} from {}: {}", file.machine, inner(&why))))?;
                }
                Reach::Gone => return Err(cannot(&name, format!("{field} was on {}, which no longer has it", file.machine))),
            }
            let length = match on_machine {
                true => file.length,
                false => {
                    std::fs::metadata(&path)
                        .map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("Cozy couldn't read {}: {e}.", path.display())).next("cozy open"))?
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
        // Sent here alone, which asks nothing (cl-1066): said once, as the box's render goes (cl-897).
        if placed.is_local() && on.len() == 1 {
            if let Some(line) = fit::told_here(&self.host.api(), &hub, package, &renderer.function, &asset.made) {
                self.say(Told::Said { message: format!("{name}: {line}"), name: Some(name.clone()) });
            }
        }
        // Local code is made ready on the machine the call goes to, its files and manifest sent
        // with every call: a machine that dropped them installs them in the call.
        let source = match &renderer.local {
            None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone(), digest: self.pinned(&hub, &renderer).map_err(named)?, ..Default::default() }),
            Some(path) => {
                let began = std::time::Instant::now();
                let prepared = self.again(|| async {
                    let scope = crate::private_io::of_path(&self.folder().dir)?;
                    crate::private_io::context(scope, self.host.ready(path, &placed, &hub, key, Some(&member))).await
                }).await;
                crate::private_io::diagnose(&self.folder().dir, || {
                    let how = prepared.as_ref().map_or_else(|error| format!("not: {}", error.code), |_| "ready".to_owned());
                    format!("{key}'s code on {placed}: {how} in {} ms", began.elapsed().as_millis())
                });
                let prepared = prepared.map_err(named)?;
                staged.extend(prepared.files);
                warnings.extend(prepared.from.map(|from| Warning { code: api::calcifer::FROM_SOURCE.into(), message: from.line() }));
                v1::Source::Local(prepared.source)
            }
        };
        let mut spec = v1::RunSpec {
            kind: v1::RunKind::Call,
            source: Some(source),
            entrypoint: renderer.function.clone(),
            payload: Some(v1::Json::parse(payload.to_string()).expect("a payload is JSON")),
            inputs,
            models,
            ..Default::default()
        };
        // The request its machine was told it may get, if it was (cl-1123): it names that item, which made it ready.
        spec.predicted = match &placed {
            On::Machine(machine) => self.host.api().machines.lists.predicted(machine, &self.folder().dir.to_string_lossy(), &name, key, &spec),
            _ => None,
        };
        let submit = SubmitRun { project: Some(self.project.id.clone()),
            on: placed,
            hub,
            kind: RunKind::Call,
            target: renderer.target(),
            spec,
            inputs: staged,
            held,
            out: Some(landing(&self.project.root)),
            warnings,
            agreed: None,
            stays: package.callable(&renderer.function).map(|callable| api::calcifer::tool::stays(&callable)).unwrap_or_default(),
        };
        let _on_the_way = OnTheWay::new(&self.on_the_way, key);
        // Canceled while it was made ready: nothing is sent.
        let mut stopped = false;
        self.folder().update_walk(|walk| match stands(&walk.walk) {
            true => {
                walk.pending.insert(key.into(), pending.clone());
                settle(walk);
            }
            false => stopped = true,
        })?;
        if stopped {
            return Ok(());
        }
        // With the others, unless it left them to wait for an install.
        let sent = match member.on().as_ref() == Some(&submit.on) {
            true => match member.submit(key, submit.clone()).await {
                // Its batch never reached Calcifer's record, or met a moment's outage: sent alone, under the same key, as often as that takes.
                Err(e) if e.transient() || e.code == host::UNSENT => {
                    warn!("{name}'s run is sent alone, as its batch wasn't: {}", e.message);
                    self.send_run(key, submit).await
                }
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
        crate::private_io::diagnose(&self.folder().dir, || format!("{key} ({name}) sent to {}: run {}", run.machine.as_deref().unwrap_or("-"), run.number));
        self.folder().update_walk(|walk| match walk.pending.get_mut(key) {
            Some(pending) => {
                pending.run = run.number;
                walk.walk.runs.insert(run.number, None);
            }
            // Canceled while its run was being made (`stop_jobs`): it ends at its machine, and nothing of it lands.
            None => stopped = true,
        })?;
        if stopped {
            self.host.api().calcifer.cancel(&run.id).map_err(|e| Error::new(Kind::Internal, "run_cancel", e.to_string()))?;
            return Ok(());
        }
        if let Some(offer) = offer {
            self.host.placed(&self.project.id, &run.id, &offer.placed());
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

    /// What has arrived already, from the walk's own record each turn (xs-199): a run whose version's files were on their way, once its
    /// record says each landed or failed, arrives now, however the follow-up waiting on it fares. A project that won't take the write now
    /// is asked again next turn.
    fn arrivals(&self, first: &mut Option<Error>) -> Result<(), Error> {
        let walk = self.folder().walk_now()?;
        for (number, coming) in &walk.walk.runs {
            let Some(coming) = coming else { continue };
            if walk.pending.values().any(|pending| pending.run == *number) || self.project.walker.lock().expect("walker lock").arrived.contains(number) {
                continue;
            }
            let Some(run) = self.host.numbered(*number) else { continue };
            if !matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Canceled) || run.output.iter().any(|output| output.owed()) {
                continue;
            }
            match tokio::task::block_in_place(|| self.arrived(&run, Some(coming.clone()))) {
                Err(error) if self.held_up(&error) => {}
                Err(error) => {
                    self.taken();
                    self.project.walker.lock().expect("walker lock").moved(&coming.name);
                    self.failed_of(&coming.name, "", None, None, error, first);
                }
                Ok(()) => self.taken(),
            }
        }
        Ok(())
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
            // Listed now, on any machine: until its bytes are here (`arrived_owed`) its take is pending, its file absent and unsized, never
            // done (Paul, 11-12). What reads it goes now, on its machine, and the walk isn't over until the bytes are here.
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
        // Once: the walk's own look (`arrivals`) and the follow-up that waited for it both get here.
        if version.is_some() && self.project.walker.lock().expect("walker lock").arrived.contains(&run.number) {
            return Ok(());
        }
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
        self.project.walker.lock().expect("walker lock").arrived.insert(run.number);
        self.project.jobs.arrived(&name, number, lost.is_none());
        let Some(output) = lost else { return Ok(()) };
        let mut tally = self.tally.lock().expect("tally lock");
        match take {
            true => tally.takes = tally.takes.saturating_sub(1),
            false => tally.versions = tally.versions.saturating_sub(1),
        }
        let said = format!("{name}: run {} made its file, but Cozy couldn't download it: {}.", run.number, inner(&output.export_error.clone().unwrap_or_default()));
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
            // Its machine shut down under it (cl-1085): said as that, which Retry renders again elsewhere. Not the machine's fault.
            let lost = run.status == RunStatus::Failed && run.reason.as_ref().is_some_and(|reason| reason.code == api::calcifer::MACHINE_LOST);
            if let (Some(machine), Some(reason), RunStatus::Failed, false) = (&run.machine, &run.reason, run.status, lost) {
                self.fault(machine, &reason.code, &reason.message)?;
            }
            let said = match (&run.reason, run.status) {
                (Some(reason), RunStatus::Canceled) => format!("{name}: the run was canceled. {}", reason.message),
                (Some(reason), _) => format!("{name}: {}", reason.message),
                (None, RunStatus::Canceled) => format!("{name}: the run was canceled."),
                (None, _) => format!("{name}: the run failed."),
            };
            let code = match run.status {
                RunStatus::Canceled => CANCELED,
                _ if lost => api::calcifer::MACHINE_LOST,
                _ => "version_failed",
            };
            let retry = lost.then(|| format!("cozy run retry {}", run.name));
            let next = retry.into_iter().chain(run.reason.iter().flat_map(|reason| reason.next.iter().cloned())).chain([format!("cozy run watch {}", run.number)]);
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
                Err(why) => version.warnings.push(format!("{name}: Cozy left off {file}'s metadata because it {why}.")),
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
                // No file at its path: on its way here, or landed already where its path only names it (an encrypted project's storage). Listed
                // by its hash either way, as its machine holds it, and `arrived_owed` takes it from there: whatever reads it needn't wait.
                None if !output.is_checkpoint() && !output.stays && output.export_error.is_none() && output.media_type != api::tree::MEDIA => {
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
        // A box of words keeps what its tool answered as its version's file, as a picture is kept (xs-204).
        if let Some(words) = pending.words.as_deref().zip(run.result.as_ref()).and_then(|(field, result)| api::calcifer::tool::text_in(result, field)) {
            saved.insert(format::TEXT.to_owned(), folder.keep_words(&words)?);
        }
        if saved.is_empty() && version.files.is_empty() {
            let said = format!("{name}: run {} finished without saving a file.", run.number);
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
        }
        Ok(coming.then_some(Coming { name, number, take: noun == "take" }))
    }

    /// An initially empty source gains the media this render made. A completed
    /// selected source stays the ask's input snapshot.
    fn select_asked(&self, ask: &str, name: &str, number: u32, sha256: &str) -> Result<(), Error> {
        let walk = self.folder().walk_now()?;
        let Some(mine) = walk.walk.asks.iter().find(|mine| mine.id == ask) else { return Ok(()) };
        let mut written = self.folder().asked(mine)?;
        let doc = Doc::asked(self.folder(), &written)?;
        if doc.find(name).is_some_and(|i| doc.assets[i].pin.is_some()) {
            return Ok(());
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
            // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
            let access = access.here();
            (tools::frame(&ffmpeg, &path, early, &dir), access)
        }).await.map_err(|e| Error::new(Kind::Internal, "frame", e.to_string()))?;
        let (frame, past) = grabbed?;
        if let (true, Some(at)) = (past, at) {
            self.say(Told::Warning { message: format!("{name}: the video ends before {at:.2}s, so Cozy used its last frame instead."), name: None });
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
                // Only the version it shows, cut again the same, makes none; the same as an older one lands as its newest (cl-1310).
                let now = self.folder().lock()?;
                let shown = standing.version().and_then(|number| now.asset(&name)?.version(number));
                let again = shown.is_some_and(|old| old.inputs == version.inputs && old.main() == folder::main_of(&files));
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
                Some(_) => ("no version yet", format!("which has no version yet. {}", render_first(&piece, &on))),
                None => ("no longer in the project", "which is no longer in the project.".to_owned()),
            };
            self.say(Told::Warning { message: format!("{name} was cut without {piece}, {said}"), name: None });
            without.push(folder::Without { piece, why: why.into(), absent: false });
        }
        // The same edit, with everything it took here and its film right, isn't cut again: shown, it stands (cl-910); an older version's
        // film and timeline land again as its newest, by hash (cl-1310).
        if without.is_empty() {
            if shown_cut(folder, &lock, asset, standing, inputs).is_some() {
                return Ok(None);
            }
            if let Some(held) = same_cut(folder, &lock, asset, &standing.reads, inputs) {
                // Said as any cut is, so its job names its film at the root.
                let sha256 = held.files.get("video");
                let newest = lock.asset(&asset.name).and_then(|held| held.versions.last()?.files.get("video"));
                let frames = sha256.and_then(|sha256| lock.files.get(sha256)).map_or(0, |film| film.frames);
                let retimed = tools::measure(&pieces, canvas.fps).map(|cut| cut.retimed).unwrap_or_default();
                self.cut.lock().expect("cut lock").insert(name.clone());
                self.say(Told::Cut { name: asset.name.clone(), cut: name, frames, fps: canvas.fps, retimed, took_s: 0.0, unchanged: newest == sha256, without: vec![] });
                return Ok(Some((held.files.clone(), vec![])));
            }
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
            // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
            let _access = access.here();
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
        // Its cut ended: a transition waiting on it looks again.
        vault::work_ended(&self.project.root);
        self.project.rendering.lock().expect("rendering lock").remove(&asset.name);
        crate::events::project_settled(&folder.dir);
        let (made, left) = cutting_result.map_err(|e| Error::new(Kind::Internal, "edit_cut", e.to_string()))?.map_err(|e| e.next("cozy open"))?;
        self.leave_out(&name, &on, left, &mut without);
        let Some((made, part)) = made else {
            let stands = if folder.dir.join(&name).is_file() { " The current film stays." } else { "" };
            self.say(Told::Warning { message: format!("Cozy couldn't cut {name} because none of its clips can be used, so there's no new version.{stands}"), name: None });
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
            self.say(Told::Warning { message: format!("{name} has {frames} frames, but its pieces add up to {}.", made.frames), name: None });
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
            self.say(Told::Warning { message: format!("{name} was cut without {piece}: {}. {}", inner(&answer), render_first(&piece, on)), name: None });
            without.push(folder::Without { piece, why, absent: false });
        }
    }

    /// Said once of a film at the root that couldn't be replaced: its version stands, and `film_follows` puts it there the next time it can.
    fn film_stays(&self, name: &str, error: &std::io::Error) {
        if !self.stays.lock().expect("stays lock").insert(name.to_owned()) {
            return;
        }
        let message = match platform::durable::held(error) {
            true => format!("{name} is open in another program, so Cozy couldn't update it. Close it, and the next render will."),
            false => format!("Cozy couldn't update {name}: {error}."),
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
                    self.say(Told::Warning { message: format!("Cozy couldn't update {}: {e}.", timeline.display()), name: None });
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
                    // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
                let _access = access.here();
                    folder::hash(&path).is_ok_and(|got| got != digest)
                }).await.unwrap_or(false)
            } else {
                false
            };
            if here && !changed {
                continue;
            }
            let why = if changed { "its bytes changed" } else { "not on this computer" };
            let which = if changed { "changed on disk" } else { "isn't on this computer" };
            self.say(Told::Said { message: format!("{}: downloading {}, which {which}.", asset.name, piece.asset), name: Some(asset.name.clone()) });
            let brought = match file.stays() && self.host.fetchable(&file.sha256, &file.machine).await {
                true => self.host.fetch(&file.sha256, &file.machine).await,
                false => self.host.pull_one(&self.project, &target.hub, file.clone()).await,
            };
            let Err(reason) = brought else { continue };
            let answer = changed.then(|| tools::unread(ffprobe, &piece.path)).flatten().map_or(String::new(), |answer| format!(" ({answer})"));
            let piece = piece.asset.clone();
            let render = if on.is_empty() { format!("Render {piece} again") } else { format!("Render {piece} on {on} again") };
            self.say(Told::Warning { message: format!("{} was cut without {piece}, which {which}{answer}, and Cozy couldn't download it: {}. {render}, or open the project in the browser to download its files.", cut_name(&asset.name), inner(&reason.to_string())), name: None });
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
        unseen::landed(&self.project, name, number);
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
        unseen::landed(&self.project, name, number.get());
        self.say_landed(name, &version, noun, number.get())
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

/// Why `name` can't render, as `{name}: {why}.`, the shape the page strips its box's name from.
fn cannot(name: &str, why: String) -> Error {
    Error::new(Kind::NotFound, "file_missing", format!("{name}: {}.", inner(&why)))
}

/// The tools with no state whose reads have settled: those nothing that's on its way goes into.
/// Why asset `i` can't read what it reads, in one line: the first read with nothing to read. Nothing renders it on `i`'s behalf (cl-1192).
fn unread(doc: &Doc, stand: &[Standing], ask: &Ask, i: usize) -> String {
    let Some(&j) = doc.upstream[i].iter().find(|&&j| stand[j].files.is_none()) else { return "something it reads isn't ready yet".into() };
    let name = &doc.assets[j].name;
    match doc.assets[j].kind.as_str() {
        _ if ask.sent.contains(&name_key(name)) => format!("{name} failed in this render"),
        _ if doc.assets[j].hollow => format!("{name} has nothing to make"),
        "image" => format!("Render {name} first: it has no picture yet"),
        "video" => format!("Render {name} first: it has no video yet"),
        "audio" => format!("Render {name} first: it has no sound yet"),
        _ => format!("Render {name} first: it has nothing yet"),
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
        Error::new(Kind::NotFound, "asset_unknown", format!("There's no box called {name} in this project.")).next("cozy status")
    })
}

/// Of `on`, where a job goes (cl-816): the soonest finish its machines quoted (`offers`), a rental before this computer on a tie, and not
/// this computer when it would run `function` poorly (`fit`) and no quote of its own runs says it ends it first (Paul, 11-05: a rented
/// GPU is for using), nor when its GPU draws the screen, which a render makes lag (cl-897). With no quote that fits, the first of `on`
/// that can take it, else where a job naming none goes, as that quotes `job`, said (`placement::takes`).
#[allow(clippy::too_many_arguments)]
async fn pick(api: &Arc<Api>, on: &[On], offers: &[Offer], planned: Option<&Offer>, job: Option<&placement::Job>, hub: &str, package: Option<&Package>, function: &str, made: &Map<String, Value>) -> Result<(On, Option<Offer>, Option<String>), Error> {
    let now = now_ms();
    let Some(soonest) = planned.filter(|offer| offer.fits).or_else(|| placement::soonest(offers, now)) else {
        let (taken, said) = placement::takes(api, on, offers, job).await?;
        return Ok((taken, None, said));
    };
    let chosen = well_placed(api, offers, soonest, hub, package, function, made, now);
    Ok((chosen.machine.clone(), Some(chosen), None))
}

/// `soonest` of `offers`, unless it's this computer and would run `function` poorly or draw the screen meanwhile (`pick`): then the
/// soonest elsewhere, if any.
#[allow(clippy::too_many_arguments)]
fn well_placed(api: &Arc<Api>, offers: &[Offer], soonest: &Offer, hub: &str, package: Option<&Package>, function: &str, made: &Map<String, Value>, now: i64) -> Offer {
    let poorly = soonest.machine.is_local()
        && (fit::draws_screen(api) || (soonest.basis != v1::Basis::Measured && package.is_some_and(|package| fit::poorly_now(api, hub, package, function, made))));
    let others: Vec<Offer> = offers.iter().filter(|offer| !offer.machine.is_local()).cloned().collect();
    match poorly {
        true => placement::soonest(&others, now).unwrap_or(soonest).clone(),
        false => soonest.clone(),
    }
}

/// A run's models as it chose them (`--model`), as a box names them: what it fetches and is timed by (cl-1279: a row is of a model).
fn chosen_models(chosen: &[v1::ModelChoice]) -> Map<String, Value> {
    let slot = |choice: &v1::ModelChoice| choice.parameter.rsplit(".models.").next().unwrap_or(&choice.parameter).to_owned();
    let written = |choice: &v1::ModelChoice| if choice.release.is_empty() { choice.model.clone() } else { format!("{}@{}", choice.model, choice.release) };
    let models: Map<String, Value> = chosen.iter().filter(|choice| !choice.model.is_empty()).map(|choice| (slot(choice), serde_json::json!({ "ref": written(choice) }))).collect();
    Map::from_iter([("models".to_owned(), Value::Object(models))])
}

/// Where a job of Calcifer's queue may go (cl-1132): its project's machines (`machines_of`), else where a run naming none goes.
pub(crate) async fn queue_set(api: &Arc<Api>, row: &crate::store::RunRow) -> Vec<On> {
    if row.project_root.is_some()
        && let Ok(set) = machines_of(api, &row.owner)
        && !set.is_empty()
    {
        return set;
    }
    crate::runs::unnamed_set(api, None).await.unwrap_or_else(|_| vec![On::Machine("local".into())])
}

/// Why a job of Calcifer's queue went nowhere (`queued_on`): it waits for what can change, or no machine will ever take it.
pub(crate) enum Unplaced {
    Waits(String),
    Never(v1::Reason),
}

/// Where a job of Calcifer's queue goes now (cl-1132, Paul 11-17: "it gives that queue of jobs to a machine. If that machine dies or
/// disappears, it re-assigns that queued job to another machine"): of `set`, the soonest finish, as a render's job goes (`pick`). Never
/// one that quoted no finish: a machine that doesn't answer is no candidate until it does. Else why none can take it now, or the
/// refusal no machine's answer will change (cl-1244).
pub(crate) async fn queued_on(api: &Arc<Api>, set: &[On], row: &crate::store::RunRow) -> Result<Offer, Unplaced> {
    let hub = row.hub.as_ref().map_or_else(|| api.machines.hubs.default_origin(), ToString::to_string);
    let function = row.spec.entrypoint.as_str();
    let payload: Value = row.spec.payload.as_ref().and_then(|payload| serde_json::from_str(payload.get()).ok()).unwrap_or_default();
    let (read, named, models, gpu) = match &row.spec.source {
        Some(v1::Source::Release(release)) => {
            // Read as a quote reads it: nothing started, moved or updated on the first of `set` (cl-1115).
            let first = set.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
            let read = Host::new(api).read_published(&hub, &format!("{}@{}", release.package, release.release), &first).await.ok();
            // What its request waits for, by its function's needs rules on its payload (cl-1103).
            let models = match &read {
                Some(read) => fit::checkpoints(api, &hub, read, function, &chosen_models(&row.spec.models), Some(&payload)).await,
                None => vec![],
            };
            let gpu = read.as_ref().and_then(|read| api::interface::Interface::parse(read.document()).ok()).is_some_and(|interface| {
                interface.callables.iter().find(|callable| callable.name == function).is_some_and(|callable| interface.needs_gpu(callable))
            });
            (read, format!("{}/{function}@{}", release.package, release.release), models, gpu)
        }
        // Local code: as its machine's estimate names it; one that builds models wants a GPU.
        Some(v1::Source::Local(local)) => (None, format!("{}/{function}", local.manifest), vec![], !row.spec.models.is_empty()),
        _ => (None, row.target.clone(), vec![], false),
    };
    let cost = (read.as_ref()).and_then(|read| Some(Arc::new(::needs::Cost::of(api::interface::entrypoint(read.document(), function)?, &payload))));
    let job = placement::Job {
        key: row.id.to_string(),
        target: row.target.clone(),
        function: named,
        models,
        size: cost.as_ref().map_or_else(|| v1::JobSize::of(&payload), |cost| cost.size.clone()),
        gpus: row.spec.gpus,
        gpu,
        on: set.to_vec(),
        scope: None,
        source: None,
        cost,
    };
    let (offers, said) = Box::pin(placement::offers(api, std::slice::from_ref(&job), true)).await;
    let offers = offers.into_iter().next().unwrap_or_default();
    let now = now_ms();
    match placement::soonest(&offers, now) {
        Some(soonest) => Ok(well_placed(api, &offers, soonest, &hub, read.as_ref(), function, &Map::new(), now)),
        // None of its machines can run it (cl-1175, cl-1279): why, with the rental.
        None if !offers.is_empty() && offers.iter().all(Offer::unable) => Err(Unplaced::Waits(placement::unable(&offers).map(|held| placement::unable_line(held, "run")).unwrap_or_default())),
        None => {
            // Every machine that answered refuses the request itself (`origin: request`): the same answer for good, so it fails with it.
            let request: Vec<&v1::Reason> = offers.iter().filter_map(|offer| offer.refusal.as_ref().filter(|reason| reason.origin == "request")).collect();
            if !request.is_empty() && request.len() == offers.len() {
                return Err(Unplaced::Never(request[0].clone()));
            }
            let why: Vec<String> = offers.iter().map(|offer| placement::said(offer, now)).chain(said).collect();
            Err(Unplaced::Waits(match why.is_empty() {
                true => "waiting for a machine that can take it".into(),
                false => format!("waiting for a machine that can take it: {}", why.join("; ")),
            }))
        }
    }
}

/// Where `cozy run` of `function` goes among `set` (Paul, 11-06): as a render's job does (`pick`), each machine asked once, for `count`
/// copies of it as if sent at once, each quoted what its needs rules say `request` (its bound payload) needs (cl-1103). Also each copy's
/// offers, and a line for each machine that didn't answer.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_on(api: &Arc<Api>, set: &[On], hub: &str, package: &str, release: &str, function: &str, request: Option<&Value>, chosen: &[v1::ModelChoice], size: v1::JobSize, gpus: u32, gpu: bool, count: u32, update: bool) -> (Result<(On, Option<String>), Error>, Vec<Vec<Offer>>, Vec<String>) {
    let host = Host::new(api);
    // Its models, for placement, read as a quote reads them: nothing started, moved or updated on the first of `set` (cl-1115), which
    // may not be where it goes; what its needs rules say `request` needs (cl-1103).
    let first = set.first().cloned().unwrap_or_else(|| On::Machine("local".into()));
    let read = host.read_published(hub, &format!("{package}@{release}"), &first).await.ok();
    let models = match &read {
        Some(read) => fit::checkpoints(api, hub, read, function, &chosen_models(chosen), request).await,
        None => vec![],
    };
    let named = format!("{package}/{function}@{release}");
    let cost = (read.as_ref().zip(request)).and_then(|(read, request)| Some(Arc::new(::needs::Cost::of(api::interface::entrypoint(read.document(), function)?, request))));
    let jobs: Vec<placement::Job> = (0..count.max(1))
        .map(|copy| placement::Job { key: copy.to_string(), target: named.clone(), function: named.clone(), models: models.clone(), size: size.clone(), gpus, gpu, on: set.to_vec(), scope: None, source: None, cost: cost.clone() })
        .collect();
    let (offers, said) = placement::offers(api, &jobs, update).await;
    let chosen = pick(api, set, offers.first().map_or(&[][..], Vec::as_slice), None, jobs.first(), hub, read.as_ref(), function, &Map::new()).await;
    (chosen.map(|(chosen, _, fell_back)| (chosen, fell_back)), offers, said)
}

/// The machines the project `named` pins its runs to (cl-873), those that ended left out (cl-920); none: wherever they finish soonest.
/// With every one of them gone now (ending, by this computer's or its Hub's word), this computer and this account's rentals: never an added
/// machine the project didn't name (cl-1066).
pub(crate) fn machines_of(api: &Arc<Api>, named: &str) -> Result<Vec<On>, Error> {
    let project = api.projects.get(api, named).map_err(|Refusal(e)| e)?;
    let pinned = chosen(api, &project.folder);
    match pinned.iter().all(|name| api.machines.gone(name).is_some()) {
        true if pinned.is_empty() => Ok(vec![]),
        true => Ok(api.machines.fallback()),
        false => Ok(pinned.into_iter().map(On::Machine).collect()),
    }
}

/// A project's machines (cl-873), each by its name now, without those that ended or are no machines here (cl-920): a rental chosen
/// under its order's handle is chosen under its Hub's name for the host (cl-1106), as its runs follow it (cl-1089), and a machine that
/// ends leaves every project's list, kept so at its next read.
pub(crate) fn chosen(api: &Api, folder: &Folder) -> Vec<String> {
    let kept = folder.machines();
    let mut live: Vec<String> = vec![];
    for name in kept.iter().map(|name| api.calcifer.machine_name_now(name)) {
        if !api.machines.ended_here(&name) && !live.contains(&name) {
            live.push(name);
        }
    }
    if live != kept
        && let Err(e) = folder.set_machines(&live)
    {
        warn!("{}: keeping its machines as they're named now: {}", folder.dir.display(), e.message);
    }
    live
}

/// Its renders' machines, each by its name now (cl-1106): what was asked of a rental by its order's handle goes to its Hub's name for the
/// host, as its runs do (cl-1089). True: some were renamed.
fn renders_follow(api: &Api, folder: &Folder) -> Result<bool, Error> {
    let now = |on: &On| match on {
        On::Machine(name) => On::Machine(api.calcifer.machine_name_now(name)),
        other => other.clone(),
    };
    let stale = |on: &[On]| on.iter().any(|one| now(one) != *one);
    let walk = folder.walk_now()?;
    if !walk.walk.asks.iter().any(|ask| stale(&ask.target.on) || ask.takes.iter().any(|take| stale(&take.target.on)))
        && !walk.pending.values().any(|pending| stale(&pending.target.on))
    {
        return Ok(false);
    }
    let follow = |on: &mut Vec<On>| {
        let mut kept: Vec<On> = vec![];
        for one in on.iter().map(now) {
            if !kept.contains(&one) {
                kept.push(one);
            }
        }
        *on = kept;
    };
    folder.update_walk(|walk| {
        for ask in &mut walk.walk.asks {
            follow(&mut ask.target.on);
            ask.takes.iter_mut().for_each(|take| follow(&mut take.target.on));
        }
        walk.pending.values_mut().for_each(|pending| follow(&mut pending.target.on));
    })?;
    Ok(true)
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
