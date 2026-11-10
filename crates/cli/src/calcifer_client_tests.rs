use super::*;
use std::{collections::BTreeMap, io::Read as _, net::TcpListener};

struct Captured {
    request: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

async fn put(body: &[u8], start: u64, total: u64, complete: bool) -> Captured {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let want = body.len();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut raw = Vec::new();
        let mut buf = [0; 1024];
        let end = loop {
            let n = socket.read(&mut buf).unwrap();
            assert_ne!(n, 0);
            raw.extend_from_slice(&buf[..n]);
            if let Some(end) = raw.windows(4).position(|part| part == b"\r\n\r\n") {
                break end + 4;
            }
            assert!(raw.len() < 64 << 10);
        };
        let head = String::from_utf8(raw[..end].to_vec()).unwrap();
        let mut lines = head.lines();
        let request = lines.next().unwrap().to_owned();
        let headers = lines.filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned())).collect();
        while raw.len() - end < want {
            let n = socket.read(&mut buf).unwrap();
            assert_ne!(n, 0);
            raw.extend_from_slice(&buf[..n]);
        }
        let status = if complete { "201 Created" } else { "204 No Content" };
        write!(socket, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        Captured { request, headers, body: raw[end..].to_vec() }
    });
    let dir = tempfile::tempdir().unwrap();
    let client = Client(Arc::new(Inner {
        http: tensorfs::fetch::http(&[]).unwrap().no_proxy().build().unwrap(),
        home: Home::under(dir.path()),
        user_home: dir.path().to_owned(),
        json: true,
        holder: Mutex::new(Some(Holder {
            address, token: "fixture-token".into(), pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(), started_at_ms: 0,
        })),
    }));
    let digest = Digest::of(if total == 0 { b"" } else { b"hello" });
    assert_eq!(client.put_part(&digest, bytes::Bytes::copy_from_slice(body), start, total).await.unwrap(), complete);
    let captured = server.join().unwrap();
    assert_eq!(captured.request, format!("PUT /v1/files/{digest} HTTP/1.1"));
    assert_eq!(captured.body, body);
    assert_eq!(captured.headers.get("content-type").map(String::as_str), Some("application/octet-stream"));
    assert!(!captured.headers.contains_key("transfer-encoding"));
    captured
}

#[tokio::test]
async fn xs159_empty_file_put_names_its_zero_length() {
    let sent = put(b"", 0, 0, true).await;
    assert_eq!(sent.headers.get("content-length").map(String::as_str), Some("0"));
    assert!(!sent.headers.contains_key("content-range"));
}

#[tokio::test]
async fn xs159_nonempty_file_put_keeps_its_whole_length() {
    let sent = put(b"hello", 0, 5, true).await;
    assert_eq!(sent.headers.get("content-length").map(String::as_str), Some("5"));
    assert!(!sent.headers.contains_key("content-range"));
}

#[tokio::test]
async fn xs159_resumed_file_put_keeps_its_range_and_part_length() {
    let sent = put(b"lo", 3, 5, false).await;
    assert_eq!(sent.headers.get("content-length").map(String::as_str), Some("2"));
    assert_eq!(sent.headers.get("content-range").map(String::as_str), Some("bytes 3-4/5"));
}
