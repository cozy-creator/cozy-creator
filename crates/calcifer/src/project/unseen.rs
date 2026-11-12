//! What a person hasn't looked at (cl-928): a version that lands on a box showing an approved one stays beside it, unseen, until it's
//! looked at. Calcifer's own, in `state.db`, by box id so a rename keeps it; never the project's, so each person has theirs.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use api::error::{Error, Kind};
use tracing::warn;

use super::{Project, graph::name_key, written::Boxed};
use crate::{
    api::{Api, Refusal},
    store::Store,
};

/// By box id, the version numbers not looked at.
pub(super) type Unseen = BTreeMap<String, BTreeSet<u32>>;

/// A box's id: what stays through a rename. One imported before ids had none: its name, as the project calls it.
pub(super) fn id_of(held: &Boxed) -> String {
    match held.id.is_empty() {
        true => format!("name:{}", name_key(&held.name)),
        false => held.id.clone(),
    }
}

/// `project`'s, as `state.db` holds them: none, if it can't be read.
pub(super) fn read(store: &Store, project: &str) -> Unseen {
    store.unseen(project).unwrap_or_else(|e| {
        warn!("{e}: what {project} hasn't been looked at starts over");
        Unseen::new()
    })
}

/// Version `number` of `name` landed: unseen when its box shows an approved one, as it goes on doing.
pub(super) fn landed(store: &Store, project: &Project, name: &str, number: u32) {
    let held = project.folder.reading_boxes(|written| written.get(name).filter(|held| held.get("approved").is_some_and(|value| !value.is_null())).map(id_of));
    let Ok(Some(held)) = held else { return };
    if keep(store, project, &held, number, false) {
        crate::events::project_settled(&project.folder.dir);
    }
}

/// Number `number` of box `held` unseen, or with `seen`, seen: whether that moved anything.
fn keep(store: &Store, project: &Project, held: &str, number: u32, seen: bool) -> bool {
    let mut unseen = project.unseen.lock().expect("unseen lock");
    let numbers = unseen.entry(held.to_owned()).or_default();
    let moved = match seen {
        true => numbers.remove(&number),
        false => numbers.insert(number),
    };
    if numbers.is_empty() {
        unseen.remove(held);
    }
    drop(unseen);
    if moved && let Err(e) = store.keep_unseen(&project.id, held, number, seen) {
        warn!("{e}: {} of {held} is {} only until Calcifer restarts", number, if seen { "seen" } else { "unseen" });
    }
    moved
}

/// `POST /v1/projects/{id}/assets/{name}/versions/{number}/seen`: this person looked at it; its dot goes, for them alone.
pub(super) async fn seen(State(api): State<Arc<Api>>, Path((id, name, number)): Path<(String, String, String)>) -> Result<StatusCode, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let number: u32 = number.parse().map_err(|_| Refusal(Error::new(Kind::Validation, "version_unknown", format!("{number} is no version number"))))?;
    let held = project.folder.reading_boxes(|written| written.get(&name).map(id_of)).map_err(Refusal)?;
    let Some(held) = held else {
        return Err(Refusal(Error::new(Kind::NotFound, "asset_unknown", format!("the project has nothing named {name}")).next("cozy status")));
    };
    if keep(api.calcifer.store(), &project, &held, number, true) {
        crate::events::project_settled(&project.folder.dir);
    }
    Ok(StatusCode::NO_CONTENT)
}
