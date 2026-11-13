//! `GET /v1/runs/{id|number}/preview` (cr-282): a running run's newest preview, fetched from its
//! machine as asked. Asking is what keeps the machine making them: a run no client asks about
//! makes none.
use std::sync::Arc;

use axum::{
    extract::{Path as Route, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use ::api::{error::Kind, machine as v1};

use crate::{
    api::{self, Api, Refusal},
    link::LinkError,
    store::State as RunState,
};

/// The run's newest preview, `ETag: "p<n>"` and its step in `Cozy-Preview-Step`; 204 while there
/// is none: not placed yet, ended, or its machine makes none.
#[derive(serde::Deserialize)]
pub(crate) struct PreviewPath { id: String, #[serde(default)] run: Option<String> }

fn private(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

pub(crate) async fn serve(
    State(api): State<Arc<Api>>,
    Route(path): Route<PreviewPath>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let none = || Ok(private(StatusCode::NO_CONTENT.into_response()));
    let project = path.run.as_ref().map(|_| path.id.as_str());
    let run = path.run.as_deref().unwrap_or(&path.id);
    let row = api::find_scoped(&api, project, run).await?;
    crate::project::vault::require_path(&row.out_dir).map_err(Refusal)?;
    let Some(machine) = row.machine.clone().filter(|_| row.state != RunState::Settled) else { return none() };
    let Some(link) = api.calcifer.link(&machine).map_err(|e| api::refuse(Kind::Unavailable, "machine_unreachable", e))? else {
        return none();
    };
    let link = match project {
        Some(project) => {
            let owner = api.projects.store(&api, project).await.map_err(Refusal)?;
            let (project_id, root) = owner.execution_scope();
            crate::private_io::check_project_placement(project_id, root, &[::api::calcifer::On::Machine(machine)])
                .map_err(Refusal)?;
            let scope = crate::private_io::scope_for_project(project_id, root).map_err(Refusal)?;
            link.with_private(scope)
        }
        None => link,
    };
    let preview = match link.preview(&row.sent_id()).await {
        Ok(Some(preview)) => preview,
        // Not on the machine (yet), or a machine older than previews.
        Ok(None) | Err(LinkError::NoRun | LinkError::NotFound) => return none(),
        // Previews are lossy: one that can't be had now is skipped, and the next ask may have it.
        Err(_) => return none(),
    };
    let tag = HeaderValue::from_str(&preview.tag).map_err(|_| api::refuse(Kind::Internal, "preview_unreadable", "its tag"))?;
    let held = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    if project.is_none() && held.is_some_and(|held| held.split(',').any(|t| t.trim() == preview.tag)) {
        return Ok(private((StatusCode::NOT_MODIFIED, [(header::ETAG, tag)]).into_response()));
    }
    // A preview is a picture: whatever else a package's executor sends is none (the guard also sandboxes every /v1 answer).
    if !preview.media_type.starts_with("image/") {
        return none();
    }
    let text = |value: &str| HeaderValue::from_str(value).unwrap_or(HeaderValue::from_static(""));
    let mut response = (StatusCode::OK, preview.body).into_response();
    let set = response.headers_mut();
    set.insert(header::CONTENT_TYPE, text(&preview.media_type));
    set.insert(header::ETAG, tag);
    set.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    set.insert(v1::PREVIEW_STEP, text(&preview.step));
    Ok(private(response))
}
