//! Dev mode (`dev = true` and `[calcifer] development_web`): the page's code and its hot reload come from a local Vite, fetched by
//! Calcifer and served as its own page, so a browser only ever talks to Calcifer and Vite forwards nothing. Only Calcifer's own OS
//! account is answered here (the code is this checkout's), and API bodies, credentials, project client-route paths/queries and the
//! browser's Referer never go to Vite.
use std::{net::SocketAddr, sync::Arc};
use axum::{body::Body, extract::{ConnectInfo, Request, State}, http::{HeaderMap, HeaderValue, Method, StatusCode, header}, response::Response};
use hyper_util::rt::TokioIo;
use crate::{api::{self, Api, Refusal}, loopback::Peer};

pub(super) const HMR_PATH: &str = "/__cozy_hmr";
const NONCE: &str = "__COZY_DEVELOPMENT_NONCE__";
const MAX_HTML: usize = 2 << 20;

fn unavailable() -> Refusal { api::refuse(::api::error::Kind::Unavailable, "development_web_unavailable", "The configured Vite development server is not answering") }
fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
}
fn resource(path: &str) -> bool {
    matches!(path, "/@react-refresh" | "/assets/NOTICES.txt")
        || ["/src/", "/node_modules/", "/@vite/", "/@id/", "/@fs/"].iter().any(|prefix| path.starts_with(prefix))
}

fn query(query: Option<&str>) -> String {
    let mut kept = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        let valid = match key.as_ref() {
            "t" => !value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit()),
            "v" => !value.is_empty() && value.len() <= 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "import" | "direct" | "raw" | "url" | "worker" | "sharedworker" => value.is_empty(),
            _ => false,
        };
        if valid { kept.append_pair(&key, &value); }
    }
    let result = kept.finish();
    if result.is_empty() { result } else { format!("?{result}") }
}

pub(super) async fn http(api: &Arc<Api>, address: SocketAddr, request: Request, client_route: bool) -> Result<Response, Refusal> {
    let peer = request.extensions().get::<ConnectInfo<Peer>>().map(|info| info.0.clone()).ok_or_else(unavailable)?;
    crate::loopback::own_user(&peer).await?;
    if request.uri().path().starts_with("/v1") || !matches!(*request.method(), Method::GET | Method::HEAD) || !client_route && !resource(request.uri().path()) {
        return Err(api::refuse(::api::error::Kind::NotFound, "unknown_route", "Calcifer does not serve this development resource"));
    }
    let target = if client_route { "/".to_owned() } else { format!("{}{}", request.uri().path(), query(request.uri().query())) };
    let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().map_err(|_| unavailable())?;
    // Start with no caller headers. In particular no bearer, media/session secret, cookie,
    // project header, Origin or Referer can enter this development-code stream.
    let css = request.headers().get(header::ACCEPT).and_then(|value| value.to_str().ok()).is_some_and(|value| value.contains("text/css"));
    let outgoing = client.request(request.method().clone(), format!("http://{address}{target}"))
        .header(header::ACCEPT, if client_route { "text/html" } else if css { "text/css" } else { "*/*" });
    let mut upstream = outgoing.send().await.map_err(|_| unavailable())?;
    if upstream.status().is_redirection() { return Err(unavailable()); }
    let status = upstream.status();
    let media = upstream.headers().get(header::CONTENT_TYPE).cloned().unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let html = media.to_str().is_ok_and(|value| value.starts_with("text/html")) && *request.method() != Method::HEAD;
    let mut response = if html {
        let mut bytes = Vec::new();
        while let Some(chunk) = upstream.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > MAX_HTML { return Err(unavailable()); }
            bytes.extend_from_slice(&chunk);
        }
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).map_err(|_| unavailable())?;
        let nonce = hex::encode(random);
        let body = String::from_utf8(bytes).map_err(|_| unavailable())?.replace(NONCE, &nonce);
        let listener = api.browser_address.get().ok_or_else(unavailable)?;
        let policy = format!("default-src 'none'; script-src 'self' 'wasm-unsafe-eval' 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self' ws://{listener}; base-uri 'none'; form-action 'none'; frame-ancestors 'none'");
        let mut response = Response::new(Body::from(body));
        response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&policy).map_err(|_| unavailable())?);
        response
    } else { Response::new(Body::from_stream(upstream.bytes_stream())) };
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, media);
    no_store(response.headers_mut());
    Ok(response)
}

/// The hot reload socket, by `/v1`'s rule: Calcifer's account and its own page, whose exact `Origin` the guard required.
pub(super) async fn upgrade(State(api): State<Arc<Api>>, ConnectInfo(peer): ConnectInfo<Peer>, mut request: Request) -> Result<Response, Refusal> {
    crate::loopback::admitted(&api, &peer, request.method(), request.headers()).await?;
    let address = api.development_web.get().ok_or_else(unavailable)?;
    let protocol = request.headers().get(header::SEC_WEBSOCKET_PROTOCOL).and_then(|value| value.to_str().ok()).filter(|value| matches!(*value, "vite-hmr" | "vite-ping")).ok_or_else(unavailable)?;
    if request.headers().get(header::UPGRADE).and_then(|value| value.to_str().ok()).is_none_or(|value| !value.eq_ignore_ascii_case("websocket")) { return Err(unavailable()); }
    let mut builder = axum::http::Request::builder().method(Method::GET).uri(HMR_PATH)
        .header(header::HOST, address.to_string()).header(header::CONNECTION, "Upgrade").header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_PROTOCOL, protocol);
    for key in [header::SEC_WEBSOCKET_KEY, header::SEC_WEBSOCKET_VERSION] {
        builder = builder.header(&key, request.headers().get(&key).ok_or_else(unavailable)?);
    }
    // The proxy is a native client of the fixed Vite backend: omit all browser headers and
    // queries, including Vite's own non-Cozy token. Incoming Host/Origin already passed guard.
    let upstream = builder.body(Body::empty()).map_err(|_| unavailable())?;
    let socket = tokio::net::TcpStream::connect(address).await.map_err(|_| unavailable())?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(socket)).await.map_err(|_| unavailable())?;
    tokio::spawn(async move { let _ = connection.with_upgrades().await; });
    let mut response = sender.send_request(upstream).await.map_err(|_| unavailable())?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS { return Err(unavailable()); }
    let downstream = hyper::upgrade::on(&mut request);
    let upstream = hyper::upgrade::on(&mut response);
    tokio::spawn(async move {
        if let (Ok(client), Ok(server)) = tokio::join!(downstream, upstream) {
            let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(client), &mut TokioIo::new(server)).await;
        }
    });
    let mut answer = Response::new(Body::empty());
    *answer.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    for key in [header::CONNECTION, header::UPGRADE, header::SEC_WEBSOCKET_ACCEPT, header::SEC_WEBSOCKET_PROTOCOL] {
        if let Some(value) = response.headers().get(&key) { answer.headers_mut().insert(key, value.clone()); }
    }
    no_store(answer.headers_mut());
    Ok(answer)
}
