use super::*;
use std::{pin::Pin, process::Command};

const FIXTURE: &str = "project::presence_tests::native_presence_fixture";
type Events = Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;

async fn next_event(events: &mut Events, buffered: &mut String) -> serde_json::Value {
    loop {
        if let Some(end) = buffered.find("\n\n") {
            let block: String = buffered.drain(..end + 2).collect();
            let data = block.lines().filter_map(|line| line.strip_prefix("data: ")).collect::<Vec<_>>().join("\n");
            if !data.is_empty() { return serde_json::from_str(&data).unwrap() }
        }
        let chunk = events.next().await.expect("events ended").unwrap();
        buffered.push_str(&String::from_utf8(chunk.to_vec()).unwrap().replace("\r\n", "\n"));
    }
}

async fn changed(events: &mut Events, buffered: &mut String, id: &str) -> serde_json::Value {
    loop {
        let event = next_event(events, buffered).await;
        if event["type"] == "project.changed" && event["id"] == id { return event }
    }
}

/// A real request reaches Calcifer while the preceding walk owns admission.
/// Seed the preceding walk at its terminal boundary so the schedule needs no machine.
async fn board_handoff(api: &Arc<Api>, client: &reqwest::Client, base: &str, stream: &str, events: &mut Events, buffered: &mut String, arrived: &Notify) {
    let within = std::time::Duration::from_secs(10);
    let opened: Opened = client.post(format!("{base}/v1/projects")).bearer_auth("presence-fixture")
        .json(&serde_json::json!({"path": api.machines.user_home.join("board handoff"), "unencrypted": true})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let id = opened.id;
    let shown: api::calcifer::ProjectShowReply = client.put(format!("{base}/v1/events/{stream}")).bearer_auth("presence-fixture")
        .json(&serde_json::json!({"projects": [&id]})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(shown.held, [id.clone()]);
    let project = api.projects.open_now(&id).expect("shown project");
    let (ready, held) = tokio::sync::oneshot::channel();
    let (finish, wait) = std::sync::mpsc::channel();
    let prior = tokio::task::spawn_blocking({
        let api = api.clone();
        move || {
            let mut walking = project.walking.lock().expect("walking lock");
            assert!(!*walking);
            *walking = true;
            let active = project.jobs.begin();
            let walk = Walk::new(&api, &project);
            walk.ended.lock().expect("ended lock").push("fixture-prior".into());
            let following = following(&api, &project, active);
            ready.send(()).unwrap();
            wait.recv_timeout(within).unwrap();
            walk.end(&mut walking, None, &following);
            let terminal = project.jobs.board();
            assert_eq!(project.folder.last_board(), Some(terminal.clone()));
            project.jobs.sync(active, vec!["stale".into()], vec![], vec![], |_| None);
            assert_eq!(project.jobs.board(), terminal);
            terminal.seq
        }
    });
    tokio::time::timeout(within, held).await.unwrap().unwrap();
    let posted = tokio::spawn({
        let client = client.clone();
        let url = format!("{base}/v1/projects/{id}/runs");
        async move {
            client.post(url).bearer_auth("presence-fixture")
                .json(&serde_json::json!({"names": ["all"], "on": ["local"]})).send().await.unwrap().error_for_status().unwrap().json::<Asked>().await.unwrap()
        }
    });
    tokio::time::timeout(within, arrived.notified()).await.expect("next request did not reach Calcifer");
    assert!(!posted.is_finished(), "next ask answered before the prior terminal board");
    finish.send(()).unwrap();
    let terminal = prior.await.unwrap();
    let next = tokio::time::timeout(within, posted).await.unwrap().unwrap();
    assert!(!next.joined && next.ask.is_some() && next.board.seq > terminal, "{next:?}");
    let mut boards = vec![];
    while boards.len() < 3 {
        let event = tokio::time::timeout(within, next_event(events, buffered)).await.expect("missing handoff frame");
        if event["type"] == "board.updated" && event["project"] == id {
            boards.push(event["board"].clone());
        }
    }
    assert!(boards[0]["ended"].is_null(), "{boards:?}");
    assert_eq!(boards[1]["seq"], terminal);
    assert_eq!(boards[1]["ended"]["asks"], serde_json::json!(["fixture-prior"]));
    assert!(boards[2]["ended"].is_null() && boards[2]["seq"].as_u64().unwrap() > terminal, "{boards:?}");

    // An old snapshot may also arrive after the next board has begun.
    let jobs = Jobs::new(id.clone());
    let stale = jobs.begin();
    jobs.end(0, api::calcifer::project::Ended::default());
    jobs.begin();
    let current = jobs.board();
    jobs.sync(stale, vec!["stale".into()], vec![], vec![], |_| None);
    assert_eq!(jobs.board(), current);
    println!("{}", serde_json::json!({"prior_end_before_next_board": true, "terminal_board_persisted": true, "stale_sync_refused": true}));
}

#[test]
fn native_presence_orders_and_client_contract() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", FIXTURE, "--nocapture"])
        .env("COZY_PRESENCE_ROOT", root.path())
        .env("TENSORFS_HOME", root.path().join("tensorfs"))
        .output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    let directory = root.path().to_owned();
    root.close().expect("owned native fixture profile did not close");
    assert!(!directory.exists());
    println!("{}", serde_json::json!({"native_profile_removed": directory}));
    assert!(output.status.success(), "native presence fixture: {}", output.status);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_presence_fixture() {
    let Some(root) = std::env::var_os("COZY_PRESENCE_ROOT").map(PathBuf::from) else { return };
    let home = platform::paths::Home::under(&root.join("cozy"));
    home.open().unwrap();
    std::fs::create_dir_all(home.calcifer()).unwrap();
    bind_calcifer(&home.calcifer()).unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let config = api::config::Config { hub_url: api::config::Origin::parse("http://127.0.0.1:9").unwrap(), ..Default::default() };
    let machines = crate::machines::Machines::start(calcifer.clone(), home, &root, &config);
    let api = crate::api::Api::new(calcifer.clone(), machines, "presence-fixture");
    crate::events::tell(api.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let observing = api.clone();
    let arrived = Arc::new(Notify::new());
    let incoming = arrived.clone();
    let app = crate::api::router(api.clone()).route("/fixture/open/{id}", axum::routing::get(move |Path(id): Path<String>| {
        let observing = observing.clone();
        async move { Json(serde_json::json!({"open": observing.projects.open_now(&id).is_some()})) }
    })).layer(axum::middleware::from_fn(move |request: axum::extract::Request, next: axum::middleware::Next| {
        let incoming = incoming.clone();
        async move {
            if request.method() == axum::http::Method::POST && request.uri().path().ends_with("/runs") {
                incoming.notify_one();
            }
            next.run(request).await
        }
    })).into_make_service_with_connect_info::<crate::api::Peer>();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = tensorfs::fetch::http(&[]).unwrap().no_proxy().build().unwrap();
    let base = format!("http://{address}");
    let response = client.get(format!("{base}/v1/events")).bearer_auth("presence-fixture").send().await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let mut events: Events = Box::pin(response.bytes_stream());
    let mut buffered = String::new();
    let hello = next_event(&mut events, &mut buffered).await;
    assert_eq!(hello["type"], "hello");
    let stream = hello["stream"].as_str().unwrap();
    let mut last = String::new();
    for read_first in [true, false] {
        let before = crate::events::snapshot_cursor().unwrap();
        let response = client.post(format!("{base}/v1/projects")).bearer_auth("presence-fixture")
            .json(&serde_json::json!({"name": if read_first { "cold" } else { "leased" }, "unencrypted": true})).send().await.unwrap();
        let opened: serde_json::Value = response.json().await.unwrap();
        let id = opened["id"].as_str().expect("project created");
        last = id.to_owned();
        // Creation is published before interest, so it cannot be used as an SSE barrier.
        // Observe that publication directly before exercising the two read/show orders.
        let created = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let cursor = crate::events::snapshot_cursor().unwrap();
                if cursor.sequence > before.sequence { break cursor; }
                tokio::task::yield_now().await;
            }
        }).await.expect("creation was not published");
        println!("{}", serde_json::json!({"creation": created}));
        let full;
        if read_first {
            let state: serde_json::Value = client.get(format!("{base}/v1/projects/{id}")).bearer_auth("presence-fixture")
                .send().await.unwrap().json().await.unwrap();
            assert!(state["assets"].as_array().unwrap().is_empty());
            full = Some(state);
            assert!(api.projects.open_now(id).is_none(), "GET's unleased project did not close");
        } else {
            full = None;
        }
        let shown = client.put(format!("{base}/v1/events/{stream}")).bearer_auth("presence-fixture")
            .json(&serde_json::json!({"projects": [id]})).send().await.unwrap();
        assert!(shown.status().is_success());
        let _ = shown.bytes().await.unwrap();
        assert!(api.projects.open_now(id).is_some(), "show did not install Reader");
        let full = if !read_first {
            let state: serde_json::Value = client.get(format!("{base}/v1/projects/{id}")).bearer_auth("presence-fixture")
                .send().await.unwrap().json().await.unwrap();
            assert!(state["assets"].as_array().unwrap().is_empty());
            state
        } else { full.unwrap() };
        assert_eq!(full["covered"]["generation"], created.generation);
        assert!(full["covered"]["sequence"].as_u64().unwrap() >= created.sequence);
        let response = client.post(format!("{base}/v1/projects/{id}/assets")).bearer_auth("presence-fixture")
            .json(&serde_json::json!({"name": "words", "tool": "", "fields": {"text": "hello"}})).send().await.unwrap();
        assert!(response.status().is_success(), "{}", response.text().await.unwrap());
        let _ = response.bytes().await.unwrap();
        let wanted = revision(FsPath::new(opened["path"].as_str().unwrap()));
        let event = loop {
            let event = changed(&mut events, &mut buffered, id).await;
            if event["rev"].as_str().is_some_and(|rev| rev.starts_with(&wanted)) { break event }
        };
        if read_first { assert!(event["assets"].is_null(), "{event}") }
        else { assert!(event["assets"].as_array().unwrap().iter().any(|name| name == "words"), "{event}") }
        assert!(event["cursor"]["sequence"].as_u64().unwrap() > full["covered"]["sequence"].as_u64().unwrap(), "snapshot covered a later write");
        println!("{}", serde_json::json!({"schedule": if read_first { "GET-close-show-write" } else { "show-GET-write" }, "event": event}));
    }
    let mut many: Vec<String> = (0..64).map(|i| format!("absent-{i:02}")).collect();
    many.push(last.clone());
    let capped = client.put(format!("{base}/v1/events/{stream}")).bearer_auth("presence-fixture")
        .json(&serde_json::json!({"projects": many})).send().await.unwrap();
    let capped: serde_json::Value = capped.json().await.unwrap();
    assert!(capped["held"].as_array().unwrap().is_empty());
    assert_eq!(capped["refused"].as_object().unwrap().len(), 64);
    assert!(api.projects.open_now(&last).is_none(), "omitted ID kept a Reader");
    println!("{}", serde_json::json!({"bounded_attempts": 64, "omitted_id_not_held": true}));
    board_handoff(&api, &client, &base, stream, &mut events, &mut buffered, &arrived).await;
    if let Some(script) = std::env::var_os("COZY_PRESENCE_CLIENT_FIXTURE") {
        let url = base.clone();
        let output = tokio::task::spawn_blocking(move || Command::new("node").arg(script)
            .env("COZY_PRESENCE_URL", url).env("COZY_PRESENCE_TOKEN", "presence-fixture").output().unwrap()).await.unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "actual client fixture: {}", output.status);
        let close: Vec<String> = String::from_utf8_lossy(&output.stdout).lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|value| value.get("close_on_exit").cloned())
            .flat_map(|value| serde_json::from_value::<Vec<String>>(value).unwrap()).collect();
        if !close.is_empty() {
            // The owned Node realm ends with live SSE streams, like closing its pages. This
            // fixture protocol observes the native Reader drops; it never kills a real run.
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while close.iter().any(|id| api.projects.open_now(id).is_some()) { tokio::task::yield_now().await; }
            }).await.expect("closed page cohorts did not release their native Readers");
            println!("{}", serde_json::json!({"closed_cohort_readers_released": close}));
        }
    }
    drop(events);
    api.down().await;
    server.abort();
    let _ = server.await;
    calcifer.close();
}
