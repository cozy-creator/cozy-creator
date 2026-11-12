//! Canvas view preferences stay in the project's own database, under its read lease.
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
};
use api::{calcifer::project::{ProjectView, ProjectViewPatch}, error::{Error, Kind}};
use rusqlite::{Connection, OptionalExtension};

use crate::{api::{Api, Refusal, parse}, store::{self, StoreError}};

fn stored(connection: &Connection) -> store::Result<ProjectView> {
    let document: Option<String> = connection
        .query_row("SELECT document FROM project_view WHERE singleton=1", [], |row| row.get(0))
        .optional()?;
    match document {
        Some(document) => serde_json::from_str(&document).map_err(|error| StoreError::Corrupt(error.to_string())),
        None => Ok(ProjectView::default()),
    }
}

pub(super) async fn read(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<ProjectView>, Refusal> {
    let store = api.projects.store(&api, &id).await.map_err(Refusal)?;
    Ok(Json(store.read(stored)?))
}

pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ProjectView>, Refusal> {
    let patch: ProjectViewPatch = parse(&body?)?;
    if let Some(viewport) = patch.viewport
        && (!viewport.x.is_finite() || !viewport.y.is_finite() || !viewport.zoom.is_finite() || viewport.zoom <= 0.0)
    {
        return Err(Refusal(Error::new(Kind::Validation, "project_view", "Canvas coordinates must be finite and zoom must be positive")));
    }
    let store = api.projects.store(&api, &id).await.map_err(Refusal)?;
    Ok(Json(store.write(|transaction| {
        let mut view = stored(transaction)?;
        if let Some(viewport) = patch.viewport { view.viewport = Some(viewport); }
        if let Some(selection) = patch.selection { view.selection = selection; }
        if let Some(more) = patch.more { view.more = more; }
        let document = serde_json::to_string(&view).map_err(|error| StoreError::Corrupt(error.to_string()))?;
        transaction.execute("INSERT INTO project_view(singleton,document) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET document=excluded.document", [&document])?;
        Ok(view)
    })?))
}
