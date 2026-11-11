use super::*;
use std::{collections::BTreeMap, io::Read as _, net::{TcpListener, TcpStream}, sync::atomic::{AtomicBool, AtomicUsize, Ordering}};

#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read(socket: &mut TcpStream) -> Request {
    socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let end = loop {
        let n = socket.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") { break at + 4; }
    };
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let mut lines = head.lines();
    let mut target = lines.next().unwrap().split_whitespace();
    let method = target.next().unwrap().to_owned();
    let path = target.next().unwrap().to_owned();
    let headers: BTreeMap<String, String> = lines.filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned())).collect();
    let length = headers.get("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
    while bytes.len() - end < length {
        let n = socket.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    Request { method, path, headers, body: bytes[end..end + length].to_vec() }
}

struct Reply {
    status: &'static str,
    body: String,
    content_type: &'static str,
    headers: Vec<(&'static str, &'static str)>,
}

impl Reply {
    fn json(body: serde_json::Value) -> Self {
        Self { status: "200 OK", body: body.to_string(), content_type: "application/json", headers: vec![] }
    }

    fn restarting() -> Self {
        Self { status: "503 Service Unavailable", body: "restarting".into(), content_type: "text/plain", headers: vec![("Retry-After", "0")] }
    }

    fn event(body: serde_json::Value) -> Self {
        Self { status: "200 OK", body: format!("data: {body}\n\n"), content_type: "text/event-stream", headers: vec![] }
    }
}

struct Fixture {
    client: Client,
    seen: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
    _claim: Arc<platform::lock::Held>,
    _home: tempfile::TempDir,
}

impl Fixture {
    fn new(mut answer: impl FnMut(&Request, &platform::lock::Held, &Holder) -> Option<Reply> + Send + 'static) -> Self {
        let home = tempfile::tempdir().unwrap();
        let paths = Home::under(home.path());
        paths.open().unwrap();
        std::fs::create_dir_all(paths.calcifer()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let holder = Holder { address: listener.local_addr().unwrap(), token: "first-token".into(), pid: std::process::id(), version: env!("CARGO_PKG_VERSION").into(), started_at_ms: 1 };
        let claim = Arc::new(LockFile::new(paths.calcifer_lock()).lock(LockKind::Exclusive).unwrap());
        claim.publish(&serde_json::to_vec(&holder).unwrap()).unwrap();
        let client = Client(Arc::new(Inner {
            http: tensorfs::fetch::http(&[]).unwrap().no_proxy().build().unwrap(),
            home: paths, user_home: home.path().into(), json: true, holder: Mutex::new(Some(holder.clone())),
        }));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (recorded, stopping, claimed) = (seen.clone(), stop.clone(), claim.clone());
        let server = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                let request = read(&mut socket);
                let reply = if request.path == "/v1/status" {
                    Some(Reply::json(serde_json::json!({})))
                } else { answer(&request, &claimed, &holder) };
                recorded.lock().unwrap().push(request);
                if let Some(reply) = reply {
                    let headers: String = reply.headers.iter().map(|(key, value)| format!("{key}: {value}\r\n")).collect();
                    let _ = write!(socket, "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: {}\r\n{headers}Connection: close\r\n\r\n{}", reply.status, reply.body.len(), reply.content_type, reply.body);
                }
            }
        });
        Self { client, seen, stop, server: Some(server), _claim: claim, _home: home }
    }

    fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().iter().filter(|request| request.path != "/v1/status").cloned().collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server) = self.server.take() { let _ = server.join(); }
    }
}

#[tokio::test]
async fn download_progress_reports_written_bytes_and_total_including_empty_files() {
    let fixture = Fixture::new(|request, _, _| Some(Reply {
        status: "200 OK",
        body: if request.path == "/v1/empty" { String::new() } else { "abcdé".into() },
        content_type: "application/octet-stream",
        headers: vec![],
    }));
    for (path, expected) in [("/v1/file", "abcdé".as_bytes()), ("/v1/empty", b"".as_slice())] {
        let destination = fixture._home.path().join("download");
        let observed = Mutex::new(Vec::new());
        let written = fixture.client.download_progress(path, &destination, &|done, total| {
            observed.lock().unwrap().push((done, total));
        }).await.unwrap();
        let count = expected.len() as u64;
        assert_eq!(written, count);
        assert_eq!(std::fs::read(destination).unwrap(), expected);
        let observed = observed.into_inner().unwrap();
        assert_eq!(observed.first(), Some(&(0, Some(count))));
        assert_eq!(observed.last(), Some(&(count, Some(count))));
        assert!(observed.iter().all(|(_, total)| *total == Some(count)));
        assert!(observed.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }
}

#[tokio::test]
async fn raw_503_retries_and_reads_the_rotated_holder_token() {
    let mut attempt = 0;
    let fixture = Fixture::new(move |_, claim, original| {
        attempt += 1;
        if attempt == 1 {
            claim.publish(&serde_json::to_vec(&Holder { token: "new-token".into(), started_at_ms: 2, ..original.clone() }).unwrap()).unwrap();
            Some(Reply::restarting())
        } else { Some(Reply::json(serde_json::json!({"ok": true}))) }
    });
    let result: serde_json::Value = fixture.client.get("/v1/test").await.unwrap();
    assert_eq!(result["ok"], true);
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].headers["authorization"], "Bearer first-token");
    assert_eq!(sent[1].headers["authorization"], "Bearer new-token");
}

struct CountedBody(Arc<AtomicUsize>);
impl Serialize for CountedBody {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = self.0.fetch_add(1, Ordering::Relaxed);
        serde_json::json!({"value": value}).serialize(serializer)
    }
}

#[tokio::test]
async fn accepted_lost_reply_replays_the_same_key_and_body() {
    let mut attempt = 0;
    let fixture = Fixture::new(move |_, _, _| {
        attempt += 1;
        (attempt > 1).then(|| Reply::json(serde_json::json!({"run": "same-run"})))
    });
    let encoded = Arc::new(AtomicUsize::new(0));
    let result: serde_json::Value = fixture.client.post("/v1/runs", &CountedBody(encoded.clone()), Some("same-key")).await.unwrap();
    assert_eq!(result["run"], "same-run");
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(encoded.load(Ordering::Relaxed), 1);
    assert_eq!(sent[0].body, sent[1].body);
    assert_eq!(sent[0].headers["idempotency-key"], "same-key");
    assert_eq!(sent[1].headers["idempotency-key"], "same-key");
}

#[tokio::test]
async fn permanent_refusals_and_unsafe_posts_are_not_retried() {
    for (status, kind, code) in [("403 Forbidden", Kind::Forbidden, "denied"), ("503 Service Unavailable", Kind::Capacity, "no_capacity")] {
        let fixture = Fixture::new(move |_, _, _| Some(Reply {
            status, body: serde_json::to_string(&Envelope { error: Error::new(kind, code, "permanent refusal") }).unwrap(),
            content_type: "application/json", headers: vec![],
        }));
        let error = fixture.client.get::<serde_json::Value>("/v1/test").await.unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(fixture.requests().len(), 1);
    }
    let fixture = Fixture::new(|_, _, _| Some(Reply::restarting()));
    let error = fixture.client.post::<_, serde_json::Value>("/v1/unsafe", &serde_json::json!({}), None).await.unwrap_err();
    assert!(error.remedy.as_deref().unwrap().contains("check whether the operation completed"));
    assert_eq!(fixture.requests().len(), 1);
    let fixture = Fixture::new(|_, _, _| None);
    assert!(fixture.client.post::<_, serde_json::Value>("/v1/unsafe", &serde_json::json!({}), None).await.is_err());
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn persistent_503_stops_after_five_attempts() {
    let fixture = Fixture::new(|_, _, _| Some(Reply::restarting()));
    let error = fixture.client.get::<serde_json::Value>("/v1/test").await.unwrap_err();
    assert_eq!(error.code, "calcifer_unavailable");
    assert!(error.message.contains("after 5 attempts"));
    assert_eq!(fixture.requests().len(), 5);
}

#[tokio::test]
async fn vault_private_creation_omits_the_response_cache_key() {
    let fixture = Fixture::new(|_, _, _| Some(Reply::json(serde_json::json!({"created":true}))));
    let result: serde_json::Value = fixture.client.private_post("/v1/projects", &serde_json::json!({"passphrase":"synthetic"})).await.unwrap();
    assert_eq!(result["created"], true);
    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("idempotency-key"));
}

#[tokio::test]
async fn vault_private_creation_does_not_replay_a_lost_reply() {
    let fixture = Fixture::new(|_, _, _| None);
    assert!(fixture.client.private_post::<_, serde_json::Value>("/v1/projects", &serde_json::json!({"passphrase":"synthetic"})).await.is_err());
    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("idempotency-key"));
}

#[tokio::test]
async fn credentials_are_refreshed_only_when_the_holder_changed() {
    for rotate in [false, true] {
        let mut attempt = 0;
        let fixture = Fixture::new(move |_, claim, original| {
            attempt += 1;
            if rotate && attempt > 1 { return Some(Reply::json(serde_json::json!({"ok":true}))); }
            if rotate {
                claim.publish(&serde_json::to_vec(&Holder { token: "new-token".into(), started_at_ms: 2, ..original.clone() }).unwrap()).unwrap();
            }
            Some(Reply { status: "401 Unauthorized", body: serde_json::to_string(&Envelope {
                error: Error::new(Kind::Credential, "unauthenticated", "sign-in required"),
            }).unwrap(), content_type: "application/json", headers: vec![] })
        });
        let result = fixture.client.get::<serde_json::Value>("/v1/test").await;
        assert_eq!(result.is_ok(), rotate);
        let sent = fixture.requests();
        assert_eq!(sent.len(), if rotate { 2 } else { 1 });
        if rotate { assert_eq!(sent[1].headers["authorization"], "Bearer new-token"); }
    }
}

#[tokio::test]
async fn follower_reconnects_with_the_same_run_and_durable_cursor() {
    let mut attempt = 0;
    let fixture = Fixture::new(move |_, _, _| {
        attempt += 1;
        Some(match attempt {
            1 => Reply::event(serde_json::json!({"type":"run.log", "sequence_number":7, "at_ms":0, "level":"info", "text":"before restart"})),
            2 => Reply::restarting(),
            _ => Reply::event(serde_json::json!({"type":"run.completed", "sequence_number":8, "run":{
                "id":"same-run", "number":1, "kind":"call", "target":"test/echo", "hub":"https://example.test", "machine":null, "status":"completed", "created_at_ms":0
            }})),
        })
    });
    let mut events = fixture.client.events("same-run", 0);
    assert_eq!(events.next().await.unwrap().unwrap().sequence_number(), Some(7));
    assert!(matches!(events.next().await.unwrap().unwrap(), Event::Completed { sequence_number: 8, .. }));
    assert!(events.next().await.is_none());
    let paths: Vec<_> = fixture.requests().into_iter().map(|request| request.path).collect();
    assert_eq!(paths, ["/v1/runs/same-run/events?starting_after=0", "/v1/runs/same-run/events?starting_after=7", "/v1/runs/same-run/events?starting_after=7"]);
}

fn vault_ask() -> VaultAsk {
    VaultAsk {
        action: "encrypt".into(), passphrase: Some("request-secret".into()), recovery_secret: None,
        idle_timeout_s: None, max_duration_s: None, remove_plaintext: false,
    }
}

fn vault_changed() -> serde_json::Value {
    serde_json::json!({
        "vault": {"encrypted": true, "state": "unlocked", "path": "/project"},
        "recovery_secret": "terminal-secret"
    })
}

#[tokio::test]
async fn vault_stream_keeps_progress_separate_from_the_final_secret() {
    let fixture = Fixture::new(|_, _, _| Some(Reply {
        status: "200 OK", content_type: "text/event-stream; charset=utf-8", headers: vec![],
        body: format!("data: {{\"type\":\"accepted\",\"action\":\"encrypt\"}}\n\n: keepalive\n\ndata: {{\"type\":\"progress\",\"phase\":\"copying\",\"files_done\":2,\"bytes_done\":128}}\n\ndata: {{\"type\":\"progress\",\"phase\":\"copying\",\"files_done\":2,\"bytes_done\":128,\"files_total\":4,\"bytes_total\":1000000}}\n\ndata: {}\n\n",
            serde_json::json!({"type":"completed", "changed":vault_changed()})),
    }));
    let mut progress = Vec::new();
    let changed = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |event| {
        progress.push(match event {
            VaultProgress::Accepted { action } => format!("accepted {action}"),
            VaultProgress::Notice { message } => format!("notice {message}"),
            VaultProgress::Progress { phase, files_done, bytes_done, files_total, bytes_total } => format!("{phase} {files_done}/{files_total} {bytes_done}/{bytes_total}"),
        });
    }).await.unwrap();
    assert_eq!(progress, ["accepted encrypt", "copying 2/0 128/0", "copying 2/4 128/1000000"]);
    assert_eq!(changed.recovery_secret.as_deref(), Some("terminal-secret"));
    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PUT");
    assert!(requests[0].path.starts_with("/v1/projects/project/vault/operations/cli-"));
    assert_eq!(requests[0].headers["authorization"], "Bearer first-token");
    assert_eq!(requests[0].headers["accept"], "text/event-stream");
    assert!(!requests[0].headers.contains_key("idempotency-key"));
}

#[tokio::test]
async fn vault_unreadable_event_never_discloses_partial_data() {
    let fixture = Fixture::new(|_, _, _| Some(Reply {
        status: "200 OK", content_type: "text/event-stream", headers: vec![],
        body: "data: {\"type\":\"completed\",\"changed\":\"terminal-secret\"}\n\n".into(),
    }));
    let error = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| {}).await.err().unwrap();
    assert_eq!(error.code, "vault_stream_interrupted");
    assert!(error.remedy.as_deref().unwrap().contains("status"));
    assert!(!serde_json::to_string(&error).unwrap().contains("terminal-secret"));
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn vault_disconnect_reattaches_to_the_same_operation_and_admits_once() {
    let admitted = Arc::new(AtomicUsize::new(0));
    let count = admitted.clone();
    let mut operation = None;
    let fixture = Fixture::new(move |request, _, _| {
        if let Some(path) = &operation {
            assert_eq!(&request.path, path);
            return Some(Reply::event(serde_json::json!({"type":"completed", "changed":vault_changed()})));
        }
        count.fetch_add(1, Ordering::Relaxed);
        operation = Some(request.path.clone());
        Some(Reply::event(serde_json::json!({"type":"accepted", "action":"encrypt"})))
    });
    let changed = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| {}).await.unwrap();
    assert_eq!(changed.recovery_secret.as_deref(), Some("terminal-secret"));
    assert_eq!(admitted.load(Ordering::Relaxed), 1);
    let requests = fixture.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "PUT");
    assert_eq!(requests[0].path, requests[1].path);
    assert_eq!(requests[0].body, requests[1].body);
}

#[tokio::test]
async fn vault_expired_operation_never_falls_back_to_a_new_mutation() {
    let mut attempts = 0;
    let fixture = Fixture::new(move |_, _, _| {
        attempts += 1;
        Some(if attempts == 1 {
            Reply::event(serde_json::json!({"type":"accepted", "action":"encrypt"}))
        } else {
            Reply {
                status: "404 Not Found", content_type: "application/json", headers: vec![],
                body: serde_json::json!({"error":Error::not_found("vault_operation_expired", "result no longer available")}).to_string(),
            }
        })
    });
    let error = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| {}).await.err().unwrap();
    assert_eq!(error.code, "vault_operation_expired");
    let requests = fixture.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.method == "PUT"));
    assert_eq!(requests[0].path, requests[1].path);
}

#[tokio::test]
async fn vault_failed_event_preserves_the_typed_refusal_without_retry() {
    let refusal = Error::conflict("project_busy", "a project request is still running");
    let sent = refusal.clone();
    let fixture = Fixture::new(move |_, _, _| Some(Reply::event(serde_json::json!({
        "type":"failed", "error":sent
    }))));
    let error = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| {}).await.err().unwrap();
    assert_eq!(error, refusal);
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn vault_old_backend_json_is_read_once_without_progress() {
    let fixture = Fixture::new(|request, _, _| Some(if request.path.contains("/operations/") {
        Reply {
            status: "404 Not Found", content_type: "application/json", headers: vec![],
            body: serde_json::json!({"error":Error::not_found("unknown_route", "Calcifer does not serve this route")}).to_string(),
        }
    } else { Reply::json(vault_changed()) }));
    let mut progress = 0;
    let changed = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| progress += 1).await.unwrap();
    assert_eq!(changed.recovery_secret.as_deref(), Some("terminal-secret"));
    assert_eq!(progress, 0);
    let requests = fixture.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "PUT");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/v1/projects/project/vault");
    assert!(!requests[1].headers.contains_key("idempotency-key"));
}

#[tokio::test]
async fn vault_legacy_disconnect_does_not_resend_the_post() {
    let fixture = Fixture::new(|request, _, _| {
        request.path.contains("/operations/").then(|| Reply {
            status: "404 Not Found", content_type: "application/json", headers: vec![],
            body: serde_json::json!({"error":Error::not_found("unknown_route", "old controller")}).to_string(),
        })
    });
    assert!(fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| {}).await.is_err());
    let requests = fixture.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "POST");
}

#[tokio::test]
async fn vault_lost_put_headers_never_turn_a_retry_into_a_legacy_mutation() {
    let mut attempts = 0;
    let fixture = Fixture::new(move |request, _, _| {
        attempts += 1;
        if attempts == 1 { return None; } // Admitted, but no response headers reached the client.
        Some(if request.path.contains("/operations/") {
            Reply {
                status: "404 Not Found", content_type: "application/json", headers: vec![],
                body: serde_json::json!({"error":Error::not_found("unknown_route", "old controller after restart")}).to_string(),
            }
        } else { Reply::json(vault_changed()) })
    });
    let error = fixture.client.vault("/v1/projects/project/vault", &vault_ask(), |_| {}).await.err().unwrap();
    assert_eq!(error.code, "unknown_route");
    let requests = fixture.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.method == "PUT"));
    assert_eq!(requests[0].path, requests[1].path);
}

#[test]
fn vault_sse_decoder_retains_fragmented_lines_and_ignores_keepalives() {
    let raw = b": alive\r\n\r\ndata: {\"type\":\"progress\",\r\ndata: \"phase\":\"copying\",\"files_done\":1,\"bytes_done\":9}\r\n\r\n";
    let mut sse = Sse::default();
    let mut decoded = Vec::new();
    for byte in raw {
        sse.push(&[*byte]);
        if let Some(data) = sse.next() { decoded.push(data); }
    }
    assert_eq!(decoded.len(), 1);
    assert!(matches!(serde_json::from_str::<VaultEvent>(&decoded[0]).unwrap(),
        VaultEvent::Progress { files_done: 1, bytes_done: 9, .. }));
}
