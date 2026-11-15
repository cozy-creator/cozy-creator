//! A project as `project.db` says, worked out once per revision of it, so a read of it or of one asset costs what the answer costs, and what
//! changed since streams were last told, by asset, so a client reads only that.
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
    time::Instant,
};

use api::{
    calcifer::StreamEvent,
    error::{Error, Kind},
    calcifer::project::{Asset as Row, Cut, Standing as RowStanding, State as ProjectState, TakeNow},
};

use super::{
    Project, Reading, Stale, board_of, cut_row,
    folder::{Folder, Lock},
    format::{Doc, Standing, State as Stand, Tool},
    graph::name_key,
    kept_files, known_packages, making_now, next_steps, orphans, queue, revision, row, takes_now, unmade,
    unseen::{self, Unseen},
};
use crate::api::Api;
use tracing::info;

/// More assets than this changing at once is told as "read it whole".
const MOST_TOLD: usize = 256;

pub(super) struct Model {
    rev: String,
    heard: u64,
    pub doc: Doc,
    pub lock: Lock,
    /// All `GET /v1/projects/{id}` says but its assets and what's going this moment.
    head: ProjectState,
    /// In the doc's order, so an asset's index in it is its index here.
    rows: Vec<Row>,
    pub stand: Vec<Standing>,
    /// Each box's id, by its name key: what's unseen of it is kept by that.
    ids: HashMap<String, String>,
}

/// What streams were last told of a project.
#[derive(Clone)]
pub(super) struct Told {
    model: Arc<Model>,
    rendering: BTreeMap<String, f64>,
    making: HashMap<String, Vec<u64>>,
    takes: HashMap<String, Vec<TakeNow>>,
    queue: Vec<api::calcifer::project::Queued>,
    busy: bool,
    unseen: Unseen,
}

impl Told {
    pub(super) fn now(project: &Project, model: Arc<Model>) -> Self {
        let rendering = project.rendering.lock().expect("rendering lock").clone();
        let busy = *project.walking.lock().expect("walking lock");
        let walk = project.folder.walk_now().unwrap_or_default();
        let unseen = project.unseen.lock().expect("unseen lock").clone();
        Self { model, rendering, making: making_now(&walk), takes: takes_now(project, &walk), queue: queue(project, &walk), busy, unseen }
    }

    /// What of `row` hasn't been looked at, as this was told.
    fn unseen_of(&self, row: &Row) -> Vec<u32> {
        unseen_in(row, self.model.ids.get(&name_key(&row.name)).and_then(|id| self.unseen.get(id)))
    }
}

/// Of `numbers`, those `row` still has.
fn unseen_in(row: &Row, numbers: Option<&std::collections::BTreeSet<u32>>) -> Vec<u32> {
    numbers.into_iter().flatten().copied().filter(|number| row.prefixes.iter().any(|held| held.number == *number)).collect()
}

/// `project`'s model as its files are this moment. One read at a time, and any other waits and takes it.
pub(super) async fn of(api: &Arc<Api>, project: &Arc<Project>) -> Result<Arc<Model>, Error> {
    let access = Arc::new(super::vault::admit_root(&project.root).await?);
    project.require_view()?;
    let asked = Instant::now();
    let mut held = project.model.lock().await;
    project.require_view()?;
    let wait_ms = asked.elapsed().as_secs_f64() * 1000.0;
    let rev = revision(&project.folder.dir);
    let heard = api.projects.heard_changes.load(std::sync::atomic::Ordering::Relaxed);
    if let Some(model) = held.as_ref().filter(|model| model.rev == rev && model.heard == heard) {
        project.require_view()?;
        return Ok(model.clone());
    }
    let (api, read, reading, began) = (api.clone(), project.clone(), access.clone(), Instant::now());
    let answer = tokio::task::spawn_blocking(move || {
        // Dropping a canceled request's JoinHandle cannot stop a blocking worker.
        // Its own clone keeps the mounted view leased until this read ends.
        let _reading = reading;
        read.require_view()?;
        read_model(&api, &read, rev, heard)
    }).await;
    let model = Arc::new(answer.map_err(|why| Error::new(Kind::Internal, "project_read", why.to_string()))??);
    let read_ms = began.elapsed().as_secs_f64() * 1000.0;
    if wait_ms + read_ms >= 5.0 {
        info!(phase = "project_read", wait_ms, read_ms);
    }
    project.require_view()?;
    *held = Some(model.clone());
    Ok(model)
}

fn read_model(api: &Arc<Api>, project: &Project, rev: String, heard: u64) -> Result<Model, Error> {
    let folder = &project.folder;
    let lock = folder.lock()?;
    let (doc, etags, ids, heads) = folder.reading_document(|written, heads| {
        let etags: HashMap<String, String> = written.boxes.iter().map(|held| (name_key(&held.name), held.etag())).collect();
        let ids: HashMap<String, String> = written.boxes.iter().map(|held| (name_key(&held.name), unseen::id_of(held))).collect();
        Doc::of(folder, written, &lock).map(|doc| (doc, etags, ids, heads))
    })??;
    let stand = doc.stand(&lock);
    let mut warnings = doc.warnings.clone();
    warnings.extend(folder.crdt_conflicts()?.into_iter().map(api::calcifer::project::Warning::of_project));
    warnings.extend(doc.said_of_known(&lock));
    let known = known_packages(api, &doc, &folder.dir);
    warnings.extend(unmade(&doc, &known, &lock));
    for (i, standing) in stand.iter().enumerate() {
        let held = lock.asset(&doc.assets[i].name);
        let made = standing.version().and_then(|number| held?.version(number));
        warnings.extend(made.into_iter().flat_map(|made| made.warnings.iter().map(|message| api::calcifer::project::Warning::of(&doc.assets[i].name, message.clone()))));
    }
    warnings.dedup();
    let (mut missing, mut stale) = (vec![], vec![]);
    let reading = Reading { doc: &doc, folder, lock: &lock, stand: &stand, known: &known };
    let rows: Vec<Row> = (0..doc.assets.len())
        .map(|i| {
            let mut row = row(&reading, i);
            row.etag = etags.get(&name_key(&row.name)).cloned();
            match &stand[i].state {
                Stand::Stale(why) => stale.push(Stale { name: row.name.clone(), why: why.clone() }),
                Stand::Missing | Stand::Waits if doc.makes(i) => missing.push(row.name.clone()),
                _ => {}
            }
            row
        })
        .collect();
    let cuts = cuts_of(&doc, folder, &lock, &stand);
    let absent = lock.files.values().filter(|file| folder.missing(file)).count() as u64;
    let orphans = orphans(&doc, folder, &lock);
    let kept = kept_files(&doc, &lock);
    let run_files = lock.runs.iter().filter(|row| !kept.contains(&row.sha256)).count() as u64;
    let next = next_steps(&orphans, absent, !missing.is_empty() || !stale.is_empty());
    let failed: std::collections::BTreeSet<&str> = rows.iter().filter_map(|row| Some(row.failure.as_ref()?.message.as_str())).collect();
    warnings.retain(|warning| !failed.contains(warning.message.as_str()));
    let notes = rows.iter().flat_map(|row| row.unused.iter().map(|unused| api::calcifer::project::Warning::of(&row.name, unused.line(&row.name)))).collect();
    let head = ProjectState { covered: None, id: project.id.clone(), code: project.code.clone(), path: folder.dir.clone(), assets: vec![], heads, queue: vec![], missing, stale, orphans, busy: false, absent, run_files, cuts, warnings, notes, next, board: None, unsent: vec![], machines: vec![] };
    Ok(Model { rev, heard, doc, lock, head, rows, stand, ids })
}

fn cuts_of(doc: &Doc, folder: &Folder, lock: &Lock, stand: &[Standing]) -> Vec<Cut> {
    (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)).map(|i| cut_row(doc, folder, lock, stand, i)).collect()
}

/// A row with what's going this moment on it, and what of it this person hasn't looked at. A render moves in the walk's file, which this is
/// never read again for.
fn overlaid(row: &mut Row, rendering: &BTreeMap<String, f64>, making: &HashMap<String, Vec<u64>>, takes: &HashMap<String, Vec<TakeNow>>, ids: &HashMap<String, String>, unseen: &Unseen) {
    row.unseen = unseen_in(row, ids.get(&name_key(&row.name)).and_then(|id| unseen.get(id)));
    row.rendering = rendering.get(&row.name).copied();
    if row.rendering.is_some() {
        row.standing = RowStanding::Making;
    }
    // Sent and its run not numbered yet, as while it binds, installs or fetches weights, it's being made all the same (cl-945).
    if let Some(takes) = takes.get(&name_key(&row.name)) {
        // What's on its way, then what failed of it (the row's own), each once: one just failed is the failed one.
        let failed: Vec<TakeNow> = std::mem::take(&mut row.takes);
        let ended: std::collections::HashSet<String> = failed.iter().map(|take| take.id.clone()).collect();
        row.takes = takes.iter().filter(|take| !ended.contains(&take.id)).cloned().chain(failed).collect();
        if takes.iter().any(|take| !take.queued && !row.takes.iter().any(|held| held.id == take.id && held.failure.is_some())) {
            row.standing = RowStanding::Making;
        }
    }
    if let Some(runs) = making.get(&name_key(&row.name)) {
        row.making.clone_from(runs);
        row.standing = RowStanding::Making;
        row.failure = None;
        row.status = match runs.as_slice() {
            [run] => format!("making (run {run})"),
            runs => format!("making (runs {})", runs.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")),
        };
    }
}

impl Model {
    /// `GET /v1/projects/{id}`, with or without its assets.
    pub(super) fn state(&self, project: &Project, assets: bool) -> ProjectState {
        let mut state = self.head.clone();
        // As it is this moment: it moves no revision, so the model worked out once per revision can't hold it (cl-837).
        if let Some(why) = project.folder.unsaved() {
            state.warnings.insert(0, api::calcifer::project::Warning::of_project(super::not_saved(&why)));
        }
        state.busy = *project.walking.lock().expect("walking lock");
        state.machines = project.folder.machines();
        state.board = board_of(project);
        let walk = project.folder.walk_now().unwrap_or_default();
        state.queue = queue(project, &walk);
        // A film at the root is a person's as much as ours, and no revision of the project's files says it moved.
        state.cuts = cuts_of(&self.doc, &project.folder, &self.lock, &self.stand);
        if assets {
            let rendering = project.rendering.lock().expect("rendering lock");
            let (making, takes) = (making_now(&walk), takes_now(project, &walk));
            let unseen = project.unseen.lock().expect("unseen lock");
            state.assets = self.rows.clone();
            state.assets.iter_mut().for_each(|row| overlaid(row, &rendering, &making, &takes, &self.ids, &unseen));
        }
        state
    }

    /// Asset `i` of the doc, as it stands this moment.
    /// The project document's heads it was read at.
    pub(super) fn heads(&self) -> Vec<String> {
        self.head.heads.clone()
    }

    pub(super) fn row(&self, project: &Project, i: usize) -> Row {
        let mut row = self.rows[i].clone();
        let walk = project.folder.walk_now().unwrap_or_default();
        let (making, takes) = (making_now(&walk), takes_now(project, &walk));
        overlaid(&mut row, &project.rendering.lock().expect("rendering lock"), &making, &takes, &self.ids, &project.unseen.lock().expect("unseen lock"));
        row
    }
}

/// `project.changed`, for the project in `dir`: what changed since streams were last told, by asset; `project.gone` once its folder is. Anything it can't say by asset, as a file
/// that doesn't read, it says as no list: read it whole, and that read says why.
pub(crate) async fn told(api: &Arc<Api>, id: String, dir: &Path, settled: bool) -> StreamEvent {
    let rev = match settled {
        // A new text, though neither file changed, so a client that keeps the last hears it.
        true => format!("{}+{}", revision(dir), crate::store::now_ms()),
        false => revision(dir),
    };
    // Its folder gone, that's all there is to tell: nothing of it reads (cl-980).
    if let Some(path) = super::gone(api, &id) {
        return StreamEvent::ProjectGone { id, path: path.to_string_lossy().into_owned() };
    }
    let project = api.projects.open_now(&id);
    let whole = |id, rev| StreamEvent::ProjectChanged { id, rev, cursor: None, assets: None, gone: vec![], head: false };
    let Some(project) = project else { return whole(id, rev) };
    let Ok(model) = of(api, &project).await else {
        *project.told.lock().expect("told lock") = None;
        return whole(id, rev);
    };
    let now = Told::now(&project, model);
    let was = project.told.lock().expect("told lock").replace(now.clone());
    let Some(was) = was else { return whole(id, rev) };
    let (assets, gone) = changed(&was, &now);
    let (from, to) = (&was.model.head, &now.model.head);
    // Told with no file moved, it's what the head says that moved, even if it went there and back since the last telling and a reader saw it there.
    let head = settled || was.busy != now.busy || was.queue != now.queue || (&from.missing, &from.stale, &from.orphans, &from.absent, &from.run_files, &from.cuts, &from.warnings, &from.notes, &from.next) != (&to.missing, &to.stale, &to.orphans, &to.absent, &to.run_files, &to.cuts, &to.warnings, &to.notes, &to.next);
    match assets.len() + gone.len() > MOST_TOLD {
        true => whole(id, rev),
        false => StreamEvent::ProjectChanged { id, rev, cursor: None, assets: Some(assets), gone, head },
    }
}

/// No one's listening to `id`: whoever next does reads it whole.
pub(crate) fn forget(api: &Arc<Api>, id: &str) {
    if let Some(project) = api.projects.open_now(id) {
        *project.told.lock().expect("told lock") = None;
    }
}

/// The assets whose row is another now, and those gone. It takes what each costs to compare, once per change, whatever a client then reads.
fn changed(was: &Told, now: &Told) -> (Vec<String>, Vec<String>) {
    let before: HashMap<&str, &Row> = was.model.rows.iter().map(|row| (row.name.as_str(), row)).collect();
    let after: std::collections::HashSet<&str> = now.model.rows.iter().map(|row| row.name.as_str()).collect();
    let moved = |row: &&Row| before.get(row.name.as_str()).is_none_or(|old| old != row) || was.rendering.get(&row.name) != now.rendering.get(&row.name)
        || was.making.get(&name_key(&row.name)) != now.making.get(&name_key(&row.name))
        || was.takes.get(&name_key(&row.name)) != now.takes.get(&name_key(&row.name))
        || before.get(row.name.as_str()).is_some_and(|old| was.unseen_of(old) != now.unseen_of(row));
    let assets = now.model.rows.iter().filter(moved).map(|row| row.name.clone()).collect();
    let gone = was.model.rows.iter().filter(|row| !after.contains(row.name.as_str())).map(|row| row.name.clone()).collect();
    (assets, gone)
}
