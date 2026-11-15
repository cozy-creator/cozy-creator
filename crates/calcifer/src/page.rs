//! The app's page (`web/app`), embedded by `build.rs`, on Calcifer's own port. It holds no secret, so it's below the
//! guard and outside `admit`. Client routes serve the same page so direct navigation and reload work.
mod development;
pub(crate) use development::HMR_PATH;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Request, State},
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
        .route("/__cozy_hmr", get(development::upgrade))
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

async fn index(State(api): State<Arc<Api>>, request: Request) -> Result<Response, Refusal> {
    if let Some(address) = api.development_web.get() { return development::http(&api, *address, request, true).await; }
    let (_, body, media_type) = FILES.iter().find(|(name, ..)| *name == "index.html").ok_or_else(not_built)?;
    Ok(answer(body, media_type))
}

/// A file Vite names by its hash.
async fn asset(State(api): State<Arc<Api>>, Path(file): Path<String>, request: Request) -> Result<Response, Refusal> {
    if let Some(address) = api.development_web.get() { return development::http(&api, *address, request, false).await; }
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

/// Only development web resources reach the selected Vite backend. Every API path stays local.
pub(crate) async fn fallback(State(api): State<Arc<Api>>, request: Request) -> Response {
    if !request.uri().path().starts_with("/v1") && matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        if let Some(address) = api.development_web.get() {
            return match development::http(&api, *address, request, false).await { Ok(response) => response, Err(error) => error.into_response() };
        }
    }
    crate::api::unknown_route(request.method().clone(), request.uri().clone()).await.into_response()
}
