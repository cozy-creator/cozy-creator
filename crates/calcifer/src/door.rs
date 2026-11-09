//! `ANY /v1/machines/{name}/calcifer/{*path}`: another machine's Calcifer, through its TensorD's
//! door (`Link::forward`), with both bodies streaming. It serves a pod's answers on this computer's
//! origin, where script is code execution as you, and a pod's provider has root and any package on
//! it can answer for it. So nothing of this computer's goes out with a request, and only what can't
//! run or stick comes back with an answer: a named few headers, always sandboxed, and only JSON, an
//! event stream or media shown in place. Anything else comes back as octets, to save.
use std::{io, sync::Arc};

use axum::{
    body::{Body, Bytes, HttpBody as _},
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, header},
    response::Response,
};
use futures_util::{Stream, StreamExt as _};
use api::error::{Error, Kind};
use tokio::sync::watch;

use crate::api::{Api, Refusal, refuse};

/// Whether `path` is this route: `/v1/machines/<name>/calcifer/<path>`.
pub(crate) fn is_door(path: &str) -> bool {
    let mut segments = path.split('/').skip(2);
    segments.next() == Some("machines") && segments.nth(1) == Some("calcifer") && segments.next().is_some()
}

pub(crate) async fn forward(State(api): State<Arc<Api>>, request: Request) -> Result<Response, Refusal> {
    let (parts, body) = request.into_parts();
    // Raw, as TensorD will judge it: `/v1/machines/<name>/calcifer/<path>`.
    let mut segments = parts.uri.path().splitn(6, '/').skip(3);
    let name = segments.next().unwrap_or_default();
    let path = format!("/{}", segments.nth(1).unwrap_or_default());
    if name.eq_ignore_ascii_case("local") {
        return Err(refuse(Kind::Usage, "machine_is_here", "`local`'s Calcifer is this one: ask it directly"));
    }
    if api::machine::calcifer_path(&path).is_none() {
        return Err(refuse(Kind::Forbidden, "calcifer_path_refused", format!("{path:?} is no route of Calcifer's API")));
    }
    let (row, link) = api.machines.linked(name).map_err(Refusal)?;
    // This computer's media token is this computer's.
    let query = parts.uri.query().map(|query| {
        let kept: Vec<_> = query.split('&').filter(|pair| pair.split('=').next() != Some("media")).collect();
        kept.join("&")
    });
    let target = match query.filter(|query| !query.is_empty()) {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let body = (!body.is_end_stream()).then(|| reqwest::Body::wrap_stream(body.into_data_stream()));
    let answer = link
        .forward(parts.method, &target, carried(&parts.headers), body)
        .await
        .map_err(|e| Refusal(crate::machines::refusal(&row.name, e)))?;
    let status = answer.status();
    if status.is_informational() {
        let said = format!("{} answered {status}", row.name);
        return Err(Refusal(Error::new(Kind::Unavailable, "machine_unanswered", said)));
    }
    let headers = returned(answer.headers());
    let body = until_stopped(answer.bytes_stream(), api.stop_signal());
    let mut response = Response::new(Body::from_stream(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

/// What a request carries to the pod: never `Authorization` or a cookie, which are this computer's.
fn carried(asked: &HeaderMap) -> HeaderMap {
    let named = |name: &HeaderName| {
        let name = name.as_str();
        name.starts_with("cozy-")
            || matches!(
                name,
                "accept"
                    | "content-type"
                    | "content-length"
                    | "content-range"
                    | "range"
                    | "if-match"
                    | "if-none-match"
                    | "if-range"
                    | "last-event-id"
                    | "idempotency-key"
            )
    };
    let mut carried = HeaderMap::new();
    for (name, value) in asked.iter().filter(|(name, _)| named(name)) {
        carried.append(name.clone(), value.clone());
    }
    carried
}

/// What an answer brings back. Everything else a pod says is dropped: `Set-Cookie`, `Location`,
/// its own CSP or `Content-Disposition`, `Clear-Site-Data`, `Link`, `WWW-Authenticate`.
fn returned(answer: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in [header::CONTENT_LENGTH, header::CONTENT_RANGE, header::ACCEPT_RANGES, header::ETAG] {
        if let Some(value) = answer.get(&name) {
            headers.insert(name, value.clone());
        }
    }
    for (name, value) in answer.iter().filter(|(name, _)| name.as_str().starts_with("cozy-")) {
        headers.append(name.clone(), value.clone());
    }
    // `script-src 'self'` passes this origin, so a type that can run as a module (a package's component) never comes back from a pod.
    let kind = media_type(answer).filter(|kind| in_place(kind));
    if answer.contains_key(header::CONTENT_TYPE) {
        let spelled = kind.as_deref().unwrap_or("application/octet-stream");
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(spelled).expect("checked to be tokens"));
    }
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if kind.is_none() {
        headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    }
    headers
}

/// An answer's one media type, lowercase and without parameters. Several, or anything but
/// `type/subtype`, is none: a browser may read `application/json, text/html` as HTML.
fn media_type(answer: &HeaderMap) -> Option<String> {
    let mut said = answer.get_all(header::CONTENT_TYPE).iter();
    let first = said.next()?.to_str().ok()?;
    if said.next().is_some() {
        return None;
    }
    let essence = first.split(';').next()?.trim().to_ascii_lowercase();
    let token = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b));
    let (kind, subtype) = essence.split_once('/')?;
    (token(kind) && token(subtype)).then_some(essence)
}

/// What a browser may open in place. An SVG is a document that can hold script, sandbox or not.
fn in_place(kind: &str) -> bool {
    matches!(kind, "application/json" | "text/event-stream") || (crate::files::shown_in_place(kind) && kind != "image/svg+xml")
}

/// `body`, failing once Calcifer is stopping: a stream may run forever, and `down` waits for none.
fn until_stopped(
    body: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    stop: watch::Receiver<bool>,
) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    futures_util::stream::unfold((Box::pin(body), stop), |(mut body, mut stop)| async move {
        let next = tokio::select! {
            next = body.next() => next.map(|next| next.map_err(io::Error::other)),
            _ = stop.wait_for(|stopping| *stopping) => Some(Err(io::Error::other("Calcifer is stopping"))),
        };
        next.map(|next| (next, (body, stop)))
    })
}
