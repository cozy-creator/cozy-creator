//! `ANY /v1/calcifer/{*path}`: the door to this machine's own Calcifer, which binds only loopback.
//! One function, whatever carried the request. It holds a machine-scope key to the same power it
//! already has here, and no more: X never gets a device key, and the door never stops Calcifer.
//!
//! Bytes of files moving through it are use, as an output read's are (`Lifecycle::transferred`). A
//! page looking is not, however long it stays or however often it asks: JSON, text and event streams
//! count for nothing, so a rental with a page open and no job still releases itself, and nothing
//! here takes an `Admission` or a `Follow`.
use super::{
    auth::StreamAuthority,
    http::Caller,
    v1::{self, path},
};
use crate::machine::{calcifer::Calcifer, lifecycle::Lifecycle};
use axum::{
    body::{Body, Bytes, HttpBody as _},
    extract::Request,
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use api::error::{Envelope, Error};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio_stream::{Stream, StreamExt as _};

const VIA: HeaderName = HeaderName::from_static("cozy-via");

type Chunks = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>;

/// `request`, its path under this door, to this machine's Calcifer and its answer back, both bodies
/// streaming with backpressure and ending when `authority` lapses.
pub(super) async fn forward(
    calcifer: Option<&Calcifer>,
    lifecycle: Option<&Arc<Lifecycle>>,
    caller: &Caller,
    authority: &StreamAuthority,
    request: Request,
) -> Result<Response, Error> {
    // `caller()` admits a play link's run-scope cap: it never reaches the holder's token.
    caller.machine()?;
    let (parts, body) = request.into_parts();
    let asked = parts.uri.path().strip_prefix(path::CALCIFER.trim_end_matches("/{*path}")).unwrap_or_default();
    judged(&parts.method, asked)?;
    let target = match parts.uri.query() {
        Some(query) => format!("{asked}?{query}"),
        None => asked.to_owned(),
    };
    let calcifer = calcifer.ok_or_else(down)?;
    let moved = lifecycle.filter(|_| moves_files(&parts.headers)).cloned();
    let mut headers = parts.headers;
    strip(&mut headers);
    for name in [header::AUTHORIZATION, header::HOST, header::ORIGIN, header::COOKIE] {
        headers.remove(name);
    }
    let fetch: Vec<_> = headers.keys().filter(|name| name.as_str().starts_with("sec-fetch-")).cloned().collect();
    fetch.into_iter().for_each(|name| drop(headers.remove(name)));
    let via = URL_SAFE_NO_PAD.encode(caller.actor.public_key);
    headers.insert(VIA, HeaderValue::from_str(&via).expect("base64url is ASCII"));
    let unread = (!body.is_end_stream()).then(|| {
        let chunks = body.into_data_stream().map(move |chunk| match chunk {
            Ok(bytes) => {
                if let Some(lifecycle) = &moved {
                    lifecycle.transferred();
                }
                Ok(bytes)
            }
            Err(error) => Err(Error::validation("request_body_failed", error.to_string())),
        });
        Unread(Arc::new(Mutex::new(Some(authority.wrap(Box::pin(chunks))))))
    });
    for attempt in 0..2 {
        // A body can't be sent twice, so it goes to the record as it stands now.
        let Some(holder) = calcifer.holder(attempt > 0 || unread.is_some()).await else { break };
        if unread.as_ref().is_some_and(|unread| unread.0.lock().unwrap().is_none()) {
            break;
        }
        let url = format!("http://{}{target}", holder.address);
        let mut send = calcifer.http.request(parts.method.clone(), url).headers(headers.clone()).bearer_auth(&holder.token);
        if let Some(unread) = &unread {
            send = send.body(reqwest::Body::wrap_stream(Reading(unread.clone(), None)));
        }
        let Ok(answer) = send.send().await else { continue };
        if answer.status() != StatusCode::UNAUTHORIZED {
            let (status, headers) = (answer.status(), returning(answer.headers()));
            let moved = lifecycle.filter(|_| moves_files(answer.headers())).cloned();
            let chunks = answer.bytes_stream().map(move |chunk| match chunk {
                Ok(bytes) => {
                    if let Some(lifecycle) = &moved {
                        lifecycle.transferred();
                    }
                    Ok(bytes)
                }
                Err(error) => Err(Error::unavailable("calcifer_down", error.to_string())),
            });
            return Ok(answered(status, headers, Body::from_stream(authority.wrap(Box::pin(chunks)))));
        }
        // A restart minted a new token; any other 401 is Calcifer's own answer, from the Hub, say.
        let headers = returning(answer.headers());
        let Ok(text) = answer.bytes().await else { continue };
        let stale = serde_json::from_slice::<Envelope>(&text).is_ok_and(|envelope| envelope.error.code == "unauthenticated");
        if !stale {
            return Ok(answered(StatusCode::UNAUTHORIZED, headers, Body::from(text)));
        }
    }
    Err(down())
}

fn down() -> Error {
    Error::unavailable("calcifer_down", "this machine's Calcifer is not answering")
        .remedy("ask again in a moment: it starts again by itself, and an open stream has to be opened again")
}

fn answered(status: StatusCode, headers: HeaderMap, body: Body) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// What the door never carries: outside `/v1/`, `POST /v1/down`, and a write to the account, which
/// would enrol this machine's Calcifer as a device of someone's account.
fn judged(method: &Method, asked: &str) -> Result<(), Error> {
    let refused = |said: String| Error::forbidden("calcifer_path_refused", said);
    let Some(decoded) = v1::calcifer_path(asked) else {
        return Err(refused(format!("{asked:?} is no route of Calcifer's API")));
    };
    if decoded == "/v1/down" {
        return Err(refused("this door does not stop Calcifer: `cozy machine end` ends the machine".into()));
    }
    if decoded.starts_with("/v1/account") && !matches!(*method, Method::GET | Method::HEAD) {
        return Err(refused("a machine reached through this door does not sign in: sign in on the computer in front of you".into()));
    }
    Ok(())
}

/// Whether a body is files moving rather than a page looking, by what it says it is. An upload may
/// say nothing, so saying nothing counts.
fn moves_files(headers: &HeaderMap) -> bool {
    let kind = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default();
    let kind = kind.trim_start().to_ascii_lowercase();
    !(kind.starts_with("text/") || kind.starts_with("application/json") || kind.contains("+json"))
}

/// Hop-by-hop headers: those named as such, and those a `Connection` names.
fn strip(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = (headers.get_all(header::CONNECTION).iter())
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    let hop_by_hop = [
        header::CONNECTION,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
        header::EXPECT,
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("proxy-connection"),
    ];
    for name in named.into_iter().chain(hop_by_hop) {
        headers.remove(name);
    }
}

fn returning(answer: &HeaderMap) -> HeaderMap {
    let mut headers = answer.clone();
    strip(&mut headers);
    headers.remove(header::SET_COOKIE);
    headers
}

/// A request's body: an attempt that reads any of it takes it, so another may go only while it is
/// still here.
#[derive(Clone)]
struct Unread(Arc<Mutex<Option<Chunks>>>);

struct Reading(Unread, Option<Chunks>);

impl Stream for Reading {
    type Item = Result<Bytes, Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let reading = self.get_mut();
        if reading.1.is_none() {
            reading.1 = reading.0 .0.lock().unwrap().take();
        }
        match &mut reading.1 {
            Some(chunks) => chunks.as_mut().poll_next(cx),
            None => Poll::Ready(None),
        }
    }
}
