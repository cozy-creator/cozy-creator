use super::*;
use axum::{Router, routing};
use futures_util::Stream;
use std::{
    pin::Pin,
    process::Command,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

const FIXTURE: &str = "runs::files::http_tests::held_put_wire_fixture";

/// Observes the real incoming HTTP body; a socket barrier replaces any timing assumption.
struct Observed {
    body: Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>,
    first: Option<tokio::sync::oneshot::Sender<&'static str>>,
}

impl Stream for Observed {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let next = self.body.as_mut().poll_next(cx);
        if matches!(&next, Poll::Ready(Some(Ok(bytes))) if !bytes.is_empty())
            && let Some(first) = self.first.take()
        {
            let _ = first.send("read");
        }
        next
    }
}

impl Drop for Observed {
    fn drop(&mut self) {
        if let Some(first) = self.first.take() {
            let _ = first.send("dropped");
        }
    }
}

struct Answer {
    head: String,
    body: serde_json::Value,
}

async fn answer(socket: &mut BufReader<TcpStream>, head_only: bool) -> Answer {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(
            socket.read_line(&mut line).await.unwrap(),
            0,
            "connection ended before the next reply: {head}"
        );
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    let mut bytes = vec![0; if head_only { 0 } else { length }];
    socket.read_exact(&mut bytes).await.unwrap();
    Answer {
        head,
        body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    }
}

async fn put_request(
    address: std::net::SocketAddr,
    digest: &Digest,
    range: Option<String>,
    bytes: &[u8],
) -> Answer {
    let mut socket = BufReader::new(TcpStream::connect(address).await.unwrap());
    let range = range.map_or_else(String::new, |range| format!("Content-Range: {range}\r\n"));
    let headers = format!(
        "PUT /v1/files/{digest} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n{range}\r\n",
        bytes.len()
    );
    socket
        .get_mut()
        .write_all(headers.as_bytes())
        .await
        .unwrap();
    socket.get_mut().write_all(bytes).await.unwrap();
    answer(&mut socket, false).await
}

#[test]
fn held_put_drains_fragmented_body_and_preserves_keepalive() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", FIXTURE, "--nocapture"])
        .env("COZY_HELD_PUT_WIRE_FIXTURE", "1")
        .env("TENSORFS_HOME", home.path())
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "real held-PUT HTTP fixture failed: {}",
        output.status
    );
}

#[tokio::test]
async fn held_put_wire_fixture() {
    if std::env::var_os("COZY_HELD_PUT_WIRE_FIXTURE").is_none() {
        return;
    }
    let bytes = vec![71u8; 4 * 1024 * 1024];
    let digest = hold(&bytes).unwrap();
    let path = held(&digest).unwrap();
    // TensorFS deliberately touches mtime on reuse; payload identity is the file ID and bytes.
    let before = platform::durable::FileId::at(&path).unwrap();
    assert!(
        store()
            .unwrap()
            .root()
            .starts_with(std::env::var_os("TENSORFS_HOME").unwrap())
    );
    let (first, observed) = tokio::sync::oneshot::channel();
    let first = Arc::new(Mutex::new(Some(first)));
    let app = Router::new().route(
        "/v1/files/{name}",
        routing::put(
            move |name: UrlPath<String>, headers: HeaderMap, body: Body| {
                let first = first.lock().unwrap().take();
                async move {
                    let body = Body::from_stream(Observed {
                        body: Box::pin(body.into_data_stream()),
                        first,
                    });
                    put(name, headers, body).await
                }
            },
        )
        .head(head),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut socket = BufReader::new(TcpStream::connect(address).await.unwrap());
    let headers = format!(
        "PUT /v1/files/{digest} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
        bytes.len()
    );
    socket
        .get_mut()
        .write_all(headers.as_bytes())
        .await
        .unwrap();
    socket.get_mut().write_all(&bytes[..1024]).await.unwrap();
    let first = observed.await.unwrap();
    if first == "dropped" {
        let early = answer(&mut socket, false).await;
        let write = socket.get_mut().write_all(&bytes[1024..]).await;
        println!(
            "{}",
            serde_json::json!({"accepted_before_body": true, "sent": 1024, "declared": bytes.len(), "response": early.head, "tail_write_error": write.err().map(|e| e.to_string())})
        );
        panic!(
            "held PUT succeeded while its fragmented body was unread; the HTTP connection cannot safely be reused"
        );
    }
    for chunk in bytes[1024..].chunks(16 * 1024) {
        socket.get_mut().write_all(chunk).await.unwrap();
    }
    let put = answer(&mut socket, false).await;
    assert!(put.head.starts_with("HTTP/1.1 200 "), "{}", put.head);
    assert_eq!(put.body["digest"], digest.to_string());
    assert_eq!(put.body["length"], bytes.len());
    socket
        .get_mut()
        .write_all(
            format!("HEAD /v1/files/{digest} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let head = answer(&mut socket, true).await;
    assert!(head.head.starts_with("HTTP/1.1 200 "), "{}", head.head);
    let part = put_request(
        address,
        &digest,
        Some(format!("bytes 100-199/{}", bytes.len())),
        &bytes[100..200],
    )
    .await;
    assert!(part.head.starts_with("HTTP/1.1 200 "), "{}", part.head);
    assert_eq!(part.body["length"], bytes.len());
    for payload in [&b"x"[..], &b"xxx"[..]] {
        let invalid = put_request(
            address,
            &digest,
            Some(format!("bytes 0-1/{}", bytes.len())),
            payload,
        )
        .await;
        assert_eq!(
            invalid.body["error"]["code"], "invalid_range",
            "{}",
            invalid.body
        );
    }
    let conflict = put_request(
        address,
        &digest,
        Some(format!("bytes 0-0/{}", bytes.len() + 1)),
        b"x",
    )
    .await;
    assert_eq!(conflict.body["error"]["code"], "file_length_conflict");
    let empty = hold(b"").unwrap();
    let zero = put_request(address, &empty, None, b"").await;
    assert!(zero.head.starts_with("HTTP/1.1 200 "), "{}", zero.head);
    assert_eq!(zero.body["length"], 0);
    let fresh = b"a fresh real upload";
    let fresh_digest = Digest::of(fresh);
    let created = put_request(address, &fresh_digest, None, fresh).await;
    assert!(
        created.head.starts_with("HTTP/1.1 201 "),
        "{}",
        created.head
    );
    assert_eq!(created.body["digest"], fresh_digest.to_string());
    let invalid = put_request(
        address,
        &Digest::of(b"a different fresh upload"),
        None,
        b"wrong bytes",
    )
    .await;
    assert_eq!(invalid.body["error"]["code"], "file_digest_mismatch");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        platform::durable::FileId::at(&path).unwrap(),
        before,
        "held object was rewritten"
    );
    println!(
        "{}",
        serde_json::json!({"accepted_after_body": true, "bytes": bytes.len(), "same_connection_head": head.head, "store_path": path, "object_unchanged": true})
    );
    server.abort();
    let _ = server.await;
}
