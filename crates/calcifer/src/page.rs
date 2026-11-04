//! A project's web page (`web/project`), embedded, on Calcifer's own port. It is one more client of the HTTP API, holding display
//! and YAML editing and no project logic. No build step: HTML and ES modules, as written. `web/player`'s client plays what is only
//! on a machine.
use std::sync::Arc;

use axum::{
    Router,
    extract::Path,
    http::{HeaderName, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::api::Api;

const JS: &str = "text/javascript; charset=utf-8";
/// Everything is its own origin's, and a file a run made or a machine sent. WebRTC is not `connect-src`'s to limit.
const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub fn routes() -> Router<Arc<Api>> {
    Router::new().route("/projects/{id}/", get(index)).route("/page/{file}", get(file))
}

fn answer(media_type: &'static str, body: &'static str) -> Response {
    let security = HeaderName::from_static("content-security-policy");
    ([(header::CONTENT_TYPE, media_type), (security, POLICY)], body).into_response()
}

async fn index() -> Response {
    answer("text/html; charset=utf-8", include_str!("../../../web/project/index.html"))
}

async fn file(Path(file): Path<String>) -> Response {
    match file.as_str() {
        "app.mjs" => answer(JS, include_str!("../../../web/project/app.mjs")),
        "lines.mjs" => answer(JS, include_str!("../../../web/project/lines.mjs")),
        "style.css" => answer("text/css; charset=utf-8", include_str!("../../../web/project/style.css")),
        "cozy-webrtc.js" => answer(JS, include_str!("../../../web/player/cozy-webrtc.js")),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
