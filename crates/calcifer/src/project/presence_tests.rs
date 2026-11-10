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
    let app = crate::api::router(api.clone()).route("/fixture/open/{id}", axum::routing::get(move |Path(id): Path<String>| {
        let observing = observing.clone();
        async move { Json(serde_json::json!({"open": observing.projects.open_now(&id).is_some()})) }
    })).into_make_service_with_connect_info::<std::net::SocketAddr>();
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
        let response = client.post(format!("{base}/v1/projects")).bearer_auth("presence-fixture")
            .json(&serde_json::json!({"name": if read_first { "cold" } else { "leased" }})).send().await.unwrap();
        let opened: serde_json::Value = response.json().await.unwrap();
        let id = opened["id"].as_str().expect("project created");
        last = id.to_owned();
        // Creation itself owes an initial whole-project event. Do not mistake it for the
        // later asset write, or let it warm the comparison baseline between the barriers.
        let created = changed(&mut events, &mut buffered, id).await;
        println!("{}", serde_json::json!({"creation": created}));
        if read_first {
            let state: serde_json::Value = client.get(format!("{base}/v1/projects/{id}")).bearer_auth("presence-fixture")
                .send().await.unwrap().json().await.unwrap();
            assert!(state["assets"].as_array().unwrap().is_empty());
            assert!(api.projects.open_now(id).is_none(), "GET's unleased project did not close");
        }
        let shown = client.put(format!("{base}/v1/events/{stream}")).bearer_auth("presence-fixture")
            .json(&serde_json::json!({"projects": [id]})).send().await.unwrap();
        assert!(shown.status().is_success());
        let _ = shown.bytes().await.unwrap();
        assert!(api.projects.open_now(id).is_some(), "show did not install Reader");
        if !read_first {
            let state: serde_json::Value = client.get(format!("{base}/v1/projects/{id}")).bearer_auth("presence-fixture")
                .send().await.unwrap().json().await.unwrap();
            assert!(state["assets"].as_array().unwrap().is_empty());
        }
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
    if let Some(script) = std::env::var_os("COZY_PRESENCE_CLIENT_FIXTURE") {
        let url = base.clone();
        let output = tokio::task::spawn_blocking(move || Command::new("node").arg(script)
            .env("COZY_PRESENCE_URL", url).env("COZY_PRESENCE_TOKEN", "presence-fixture").output().unwrap()).await.unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "actual client fixture: {}", output.status);
    }
    drop(events);
    api.down().await;
    server.abort();
    let _ = server.await;
    calcifer.close();
}
