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
pub(crate) async fn serve(
    State(api): State<Arc<Api>>,
    Route(run): Route<String>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let none = || Ok(StatusCode::NO_CONTENT.into_response());
    let row = api::find(&api, &run)?;
    crate::project::vault::require_path(&row.out_dir).map_err(Refusal)?;
    let Some(machine) = row.machine.clone().filter(|_| row.state != RunState::Settled) else { return none() };
    let Some(link) = api.calcifer.link(&machine).map_err(|e| api::refuse(Kind::Unavailable, "machine_unreachable", e))? else {
        return none();
    };
    let private = match machine.as_str() {
        "local" => crate::private_io::of_output(&row.out_dir).map_err(Refusal)?.map(|scope| link.with_private(scope)),
        _ => None,
    };
    let link = private.as_ref().unwrap_or(&link);
    let preview = match link.preview(&row.sent_id()).await {
        Ok(Some(preview)) => preview,
        // Not on the machine (yet), or a machine older than previews.
        Ok(None) | Err(LinkError::NoRun | LinkError::NotFound) => return none(),
        // Previews are lossy: one that can't be had now is skipped, and the next ask may have it.
        Err(e) => {
            tracing::debug!(run = %row.id, machine = %machine, "no preview: {e}");
            return none();
        }
    };
    let tag = HeaderValue::from_str(&preview.tag).map_err(|_| api::refuse(Kind::Internal, "preview_unreadable", "its tag"))?;
    let held = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    if held.is_some_and(|held| held.split(',').any(|t| t.trim() == preview.tag)) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, tag)]).into_response());
    }
    let text = |value: &str| HeaderValue::from_str(value).unwrap_or(HeaderValue::from_static(""));
    let mut response = (StatusCode::OK, preview.body).into_response();
    let set = response.headers_mut();
    set.insert(header::CONTENT_TYPE, text(&preview.media_type));
    set.insert(header::ETAG, tag);
    set.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    set.insert(v1::PREVIEW_STEP, text(&preview.step));
    Ok(response)
}
