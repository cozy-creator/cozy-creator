use super::*;
use axum::response::IntoResponse;
use futures_util::StreamExt;

fn decoded(bytes: &[u8]) -> serde_json::Value {
    let text = std::str::from_utf8(bytes).unwrap();
    let data = text.lines().find_map(|line| line.strip_prefix("data: ")).expect("SSE data frame");
    serde_json::from_str(data).unwrap()
}

#[test]
fn scope_defaults_to_all_and_rejects_unknown_values() {
    let all = Query::<Subscription>::try_from_uri(&"/v1/events".parse().unwrap()).unwrap();
    assert!(all.scope == Scope::All);
    let runs = Query::<Subscription>::try_from_uri(&"/v1/events?scope=runs".parse().unwrap()).unwrap();
    assert!(runs.scope == Scope::Runs);
    assert!(Query::<Subscription>::try_from_uri(&"/v1/events?scope=unknown".parse().unwrap()).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_scope_skips_other_subscriptions_and_streams_run_updates() {
    let root = tempfile::tempdir().unwrap();
    let home = platform::paths::Home::under(root.path());
    home.open().unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let config = api::config::Config::default();
    let machines = Machines::start(calcifer.clone(), home, root.path(), &config);
    // Only the temporary controller and its event stream are under test, not machine startup.
    machines.close();
    let api = Api::new(calcifer.clone(), machines, "event-scope-test");

    let (locked, taken) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let locked_store = calcifer.clone();
    let holder = std::thread::spawn(move || {
        locked_store.store().read(|_| {
            locked.send(()).unwrap();
            // A regression must fail without leaving a locked fixture behind.
            let _ = released.recv_timeout(Duration::from_secs(2));
            Ok(())
        }).unwrap();
    });
    taken.recv().unwrap();
    let began = Instant::now();
    let runs = subscribe(api.clone(), Scope::Runs).await;
    assert!(runs.said.is_none());
    assert!(runs.machines_changed.is_none());
    assert!(runs.showing.is_none());
    assert!(runs.shown.is_empty());
    let response = Sse::new(futures_util::stream::unfold(runs, Watch::next)).into_response();
    let mut body = response.into_body().into_data_stream();
    let hello = decoded(&body.next().await.unwrap().unwrap());
    let first_frame_was_independent = began.elapsed() < Duration::from_secs(1);
    let _ = release.send(());
    holder.join().unwrap();
    assert!(first_frame_was_independent, "run events waited for the unrelated store read lock");
    assert_eq!(hello["type"], "hello");
    assert!(!SHOWN.lock().unwrap().contains_key(hello["stream"].as_str().unwrap()));

    let mut all = subscribe(api.clone(), Scope::All).await;
    assert!(all.said.is_some());
    assert!(all.machines_changed.is_some());
    assert!(all.showing.is_some());
    PROJECTS.said.send(StreamEvent::ProjectVault { id: "test-project".into(), state: "locked".into() }).unwrap();
    let submitted = calcifer.here(None, "scope-test", "scope-test", "test/echo".into(), &config.hub_url, b"{}".to_vec(),
        |_| async { Ok(serde_json::json!({"done": true})) }).await.unwrap();
    let update = decoded(&tokio::time::timeout(Duration::from_secs(2), body.next()).await.unwrap().unwrap().unwrap());
    assert_eq!(update["type"], "run.updated");
    assert_eq!(update["run"]["id"], submitted.run.id.to_string());
    all.arrived().unwrap();
    assert!(all.machines_stale, "default streams still refresh machine counts on run changes");
    assert!(all.pending.iter().any(|event| matches!(event, StreamEvent::ProjectVault { .. })));
    drop(all);
    api.down().await.unwrap();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(2), body.next()).await.unwrap() {
        assert_eq!(decoded(&chunk.unwrap())["type"], "run.updated");
    }
    api.machines.close();
    calcifer.close();
}
