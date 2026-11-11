//! The app's page (`web/app`), embedded by `build.rs`, on Calcifer's own port. It holds no secret, so it's below the
//! guard and outside `admit`. Client routes serve the same page so direct navigation and reload work.
use std::sync::Arc;

use axum::{
    Router,
    extract::Path,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use api::error::{Error, Kind};

use crate::{
    api::{Api, Refusal, refuse},
    files::IMMUTABLE,
};

include!(concat!(env!("OUT_DIR"), "/page_files.rs"));

/// This origin's, and what's in memory. Script in this origin is code execution as you, so nothing else runs here. Uploads hash in
/// WebAssembly, which `'wasm-unsafe-eval'` allows without allowing `eval`.
pub(crate) const POLICY: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub(crate) fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/", get(index))
        .route("/assets/{file}", get(asset))
        .route("/p/{code}", get(index))
        .route("/p/{code}/{*rest}", get(index))
        .route("/projects", get(index))
        .route("/projects/{id}", get(index))
        .route("/projects/{id}/{*rest}", get(index))
        .route("/runs", get(index))
        .route("/runs/{number}", get(index))
        .route("/machines", get(index))
        .route("/machines/{name}", get(index))
        .route("/packages", get(index))
        .route("/packages/{name}", get(index))
        .route("/models", get(index))
        .route("/models/{name}", get(index))
        .route("/account", get(index))
}

fn not_built() -> Refusal {
    let error = Error::new(Kind::Unavailable, "page_not_built", "this cozy was built without its page")
        .remedy("cd web/app && pnpm install --frozen-lockfile && pnpm build, then build cozy again");
    Refusal(error)
}

fn answer(body: &'static [u8], media_type: &'static str) -> Response {
    let headers = [
        (header::CONTENT_TYPE, media_type),
        (header::CONTENT_SECURITY_POLICY, POLICY),
        (header::REFERRER_POLICY, "no-referrer"),
    ];
    (headers, body).into_response()
}

async fn index() -> Result<Response, Refusal> {
    let (_, body, media_type) = FILES.iter().find(|(name, ..)| *name == "index.html").ok_or_else(not_built)?;
    Ok(answer(body, media_type))
}

/// A file Vite names by its hash.
async fn asset(Path(file): Path<String>) -> Result<Response, Refusal> {
    if FILES.is_empty() {
        return Err(not_built());
    }
    let name = format!("assets/{file}");
    let found = FILES.iter().find(|(known, ..)| *known == name);
    let (_, body, media_type) = found.ok_or_else(|| refuse(Kind::NotFound, "asset_not_found", format!("the page has no {name}")))?;
    let mut response = answer(body, media_type);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    Ok(response)
}
