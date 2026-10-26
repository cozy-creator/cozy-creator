//! The Hub as this machine's executors reach it (cl-675): a loopback endpoint that forwards their
//! reads (GET, HEAD) to their signer's Hub under its run capability's token and a fresh DPoP proof of
//! the workload key, which never leaves TensorD. An executor presents its signer's secret here; the
//! Hub's answers pass through as they are, its redirects to storage too, so the bytes go straight to
//! the executor.
use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderName, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use std::{net::Ipv4Addr, sync::Mutex};

/// The gateway's origin, serving from its first use.
pub fn url() -> std::io::Result<String> {
    static URL: Mutex<Option<String>> = Mutex::new(None);
    let mut url = URL.lock().unwrap();
    if let Some(url) = url.as_ref() {
        return Ok(url.clone());
    }
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let origin = format!("http://{}", listener.local_addr()?);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("hub-gateway").enable_all().build()?;
    std::thread::Builder::new().name("hub-gateway".into()).spawn(move || {
        runtime.block_on(async {
            let served = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => axum::serve(listener, Router::new().fallback(forward)).await,
                Err(error) => Err(error),
            };
            if let Err(error) = served {
                tracing::error!("the Hub gateway stopped: {error}");
            }
        })
    })?;
    Ok(url.insert(origin).clone())
}

/// Headers a hop answers for itself (RFC 9110 §7.6.1), and the credentials each side presents.
fn own(name: &HeaderName) -> bool {
    matches!(name.as_str(), "authorization" | "dpop" | "host" | "connection" | "keep-alive" | "proxy-connection"
        | "te" | "trailer" | "transfer-encoding" | "upgrade" | "content-length")
}

async fn forward(request: Request) -> Response {
    let refuse = |status: StatusCode, why: String| (status, why).into_response();
    let method = request.method().clone();
    if method != Method::GET && method != Method::HEAD {
        return refuse(StatusCode::METHOD_NOT_ALLOWED, "the gateway forwards reads only".into());
    }
    let secret = request.headers().get(header::AUTHORIZATION).and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ")).unwrap_or_default();
    let Some(capability) = crate::hub::Capability::by_secret(secret) else {
        return refuse(StatusCode::UNAUTHORIZED, "present the secret TensorD gave this executor".into());
    };
    let path = request.uri().path_and_query().map_or("/", |path| path.as_str());
    let target = format!("{}{path}", capability.origin());
    let client = match crate::hub::gateway_http(capability.ca_der()) {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::BAD_GATEWAY, error.0),
    };
    let mut asked = false;
    loop {
        let authorized = capability.authorize(method.as_str(), &target).await;
        let mut upstream = client.request(method.clone(), &target);
        for (name, value) in request.headers().iter().filter(|(name, _)| !own(name)) {
            upstream = upstream.header(name, value);
        }
        for (name, value) in &authorized {
            upstream = upstream.header(name, value);
        }
        let answer = match upstream.send().await {
            Ok(answer) => answer,
            Err(error) => return refuse(StatusCode::BAD_GATEWAY, format!("{}: {error}", capability.origin())),
        };
        // A token renewed while this one was out is presented once more.
        let presented = authorized.first().map_or("", |(_, value)| value.as_str());
        if !asked && capability.renewed_since(answer.status().as_u16(), presented) {
            asked = true;
            continue;
        }
        tracing::info!(origin = %capability.origin(), %method, path = request.uri().path(), status = answer.status().as_u16(), "an executor's read, through the gateway");
        let mut response = Response::builder().status(answer.status().as_u16());
        for (name, value) in answer.headers().iter().filter(|(name, _)| !own(name) || *name == header::CONTENT_LENGTH) {
            response = response.header(name, value);
        }
        return response.body(Body::from_stream(answer.bytes_stream())).unwrap_or_else(|error| refuse(StatusCode::BAD_GATEWAY, error.to_string()));
    }
}
