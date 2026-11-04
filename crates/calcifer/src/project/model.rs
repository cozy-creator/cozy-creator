//! A project as its files say, worked out once per revision of them, so a read of it or of one asset costs what the answer costs, and what
//! changed since streams were last told, by asset, so a client reads only that.
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};

use protocol::{
    calcifer::StreamEvent,
    error::{Error, Kind},
    project::{Asset as Row, Standing as RowStanding, State as ProjectState},
};

use super::{
    Project, Reading, Stale, assetfile, board_of, cut_row,
    folder::Lock,
    format::{Doc, State as Stand, Tool},
    graph::name_key,
    kept_files, known_packages, making_now, next_steps, orphans, queue, revision, row, unadapted,
};
use crate::api::Api;

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
}

/// What streams were last told of a project.
#[derive(Clone)]
pub(super) struct Told {
    model: Arc<Model>,
    rendering: BTreeMap<String, f64>,
    busy: bool,
    moving: bool,
}

impl Told {
    pub(super) fn now(project: &Project, model: Arc<Model>) -> Self {
        let rendering = project.rendering.lock().expect("rendering lock").clone();
        let busy = *project.walking.lock().expect("walking lock");
        Self { model, rendering, busy, moving: project.moving.try_lock().is_err() }
    }
}

/// `project`'s model as its files are this moment. One read at a time, and any other waits and takes it.
pub(super) async fn of(api: &Arc<Api>, project: &Arc<Project>) -> Result<Arc<Model>, Error> {
    let mut held = project.model.lock().await;
    let rev = revision(&project.folder.dir);
    let heard = api.projects.heard_changes.load(std::sync::atomic::Ordering::Relaxed);
    if let Some(model) = held.as_ref().filter(|model| model.rev == rev && model.heard == heard) {
        return Ok(model.clone());
    }
    let (api, read) = (api.clone(), project.clone());
    let answer = tokio::task::spawn_blocking(move || read_model(&api, &read, rev, heard)).await;
    let model = Arc::new(answer.map_err(|why| Error::new(Kind::Internal, "project_read", why.to_string()))??);
    *held = Some(model.clone());
    Ok(model)
}

fn read_model(api: &Arc<Api>, project: &Project, rev: String, heard: u64) -> Result<Model, Error> {
    let folder = &project.folder;
    let lock = folder.lock()?;
    let text = folder.text()?;
    let doc = Doc::of(folder, &text, &lock)?;
    let stand = doc.stand(&lock);
    let mut warnings = doc.warnings.clone();
    warnings.extend(doc.said_of_known());
    let known = known_packages(api, &doc);
    warnings.extend(unadapted(&doc, &known));
    for (i, standing) in stand.iter().enumerate() {
        let held = lock.asset(&doc.assets[i].name);
        let made = standing.version().and_then(|number| held?.version(number));
        warnings.extend(made.into_iter().flat_map(|made| made.warnings.iter().map(|message| protocol::project::Warning::of(&doc.assets[i].name, message.clone()))));
    }
    warnings.dedup();
    let (mut missing, mut stale) = (vec![], vec![]);
    let etags = assetfile::etags(&text);
    let making = making_now(&lock);
    let reading = Reading { doc: &doc, folder, lock: &lock, stand: &stand, known: &known, making: &making };
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
    let cuts = (0..doc.assets.len()).filter(|&i| doc.tool(i) == Some(Tool::Cut)).map(|i| cut_row(&doc, folder, &lock, &stand, i)).collect();
    let absent = lock.files.values().filter(|file| folder.missing(file)).count() as u64;
    let orphans = orphans(&doc, folder, &lock);
    let kept = kept_files(&doc, &lock);
    let run_files = lock.runs.iter().filter(|row| !kept.contains(&row.sha256)).count() as u64;
    let queued = queue(folder, &lock).into_iter().map(|(item, _)| item).collect();
    let next = next_steps(&orphans, absent, !missing.is_empty() || !stale.is_empty());
    let failed: std::collections::BTreeSet<&str> = rows.iter().filter_map(|row| Some(row.failure.as_ref()?.message.as_str())).collect();
    warnings.retain(|warning| !failed.contains(warning.message.as_str()));
    let head = ProjectState { id: super::project_id(&folder.dir), path: folder.dir.clone(), assets: vec![], queue: queued, missing, stale, orphans, busy: false, moving: false, absent, run_files, cuts, warnings, next, board: None };
    Ok(Model { rev, heard, doc, lock, head, rows })
}

/// A row with what's going this moment on it.
fn overlaid(row: &mut Row, rendering: &BTreeMap<String, f64>) {
    row.rendering = rendering.get(&row.name).copied();
    if row.rendering.is_some() {
        row.standing = RowStanding::Making;
    }
}

impl Model {
    /// `GET /v1/projects/{id}`, with or without its assets.
    pub(super) fn state(&self, project: &Project, assets: bool) -> ProjectState {
        let mut state = self.head.clone();
        state.busy = *project.walking.lock().expect("walking lock");
        state.moving = project.moving.try_lock().is_err();
        state.board = board_of(project);
        if assets {
            let rendering = project.rendering.lock().expect("rendering lock");
            state.assets = self.rows.clone();
            state.assets.iter_mut().for_each(|row| overlaid(row, &rendering));
        }
        state
    }

    /// Asset `i` of the doc, as it stands this moment.
    pub(super) fn row(&self, project: &Project, i: usize) -> Row {
        let mut row = self.rows[i].clone();
        overlaid(&mut row, &project.rendering.lock().expect("rendering lock"));
        row
    }
}

/// `project.changed`, for the project in `dir`: what changed since streams were last told, by asset. Anything it can't say by asset, as a file
/// that doesn't read, it says as no list: read it whole, and that read says why.
pub(crate) async fn told(api: &Arc<Api>, id: String, dir: &Path, settled: bool) -> StreamEvent {
    let rev = match settled {
        // A new text, though neither file changed, so a client that keeps the last hears it.
        true => format!("{}+{}", revision(dir), crate::store::now_ms()),
        false => revision(dir),
    };
    let project = api.projects.open.lock().expect("projects lock").get(&id).cloned();
    let whole = |id, rev| StreamEvent::ProjectChanged { id, rev, assets: None, gone: vec![], head: false };
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
    let head = was.busy != now.busy || was.moving != now.moving || (&from.queue, &from.orphans, &from.absent, &from.run_files, &from.cuts, &from.warnings, &from.next) != (&to.queue, &to.orphans, &to.absent, &to.run_files, &to.cuts, &to.warnings, &to.next);
    match assets.len() + gone.len() > MOST_TOLD {
        true => whole(id, rev),
        false => StreamEvent::ProjectChanged { id, rev, assets: Some(assets), gone, head },
    }
}

/// No one's listening to `id`: whoever next does reads it whole.
pub(crate) fn forget(api: &Arc<Api>, id: &str) {
    if let Some(project) = api.projects.open.lock().expect("projects lock").get(id) {
        *project.told.lock().expect("told lock") = None;
    }
}

/// The assets whose row is another now, and those gone. It takes what each costs to compare, once per change, whatever a client then reads.
fn changed(was: &Told, now: &Told) -> (Vec<String>, Vec<String>) {
    let before: HashMap<&str, &Row> = was.model.rows.iter().map(|row| (row.name.as_str(), row)).collect();
    let after: std::collections::HashSet<&str> = now.model.rows.iter().map(|row| row.name.as_str()).collect();
    let moved = |row: &&Row| before.get(row.name.as_str()).is_none_or(|old| old != row) || was.rendering.get(&row.name) != now.rendering.get(&row.name);
    let assets = now.model.rows.iter().filter(moved).map(|row| row.name.clone()).collect();
    let gone = was.model.rows.iter().filter(|row| !after.contains(row.name.as_str())).map(|row| row.name.clone()).collect();
    (assets, gone)
}
