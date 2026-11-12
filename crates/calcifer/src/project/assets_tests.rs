//! Real asset handlers and native project storage; no provider, model or browser is needed.
use super::*;
use super::super::{db, made_new};
use serde_json::json;

#[test]
fn typed_blank_media_persists_and_tool_removal_keeps_its_words() {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "project::assets::tests::typed_nodes_fixture", "--nocapture"])
        .env("COZY_TYPED_NODE_ROOT", root.path())
        .env("TENSORFS_HOME", root.path().join("tensorfs"))
        .output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

async fn make(api: &Arc<Api>, id: &str, value: Value) -> String {
    let (status, Json(made)) = create(State(api.clone()), Path(id.to_owned()), Ok(Bytes::from(value.to_string()))).await.unwrap();
    assert_eq!(status, StatusCode::CREATED);
    made.name
}

async fn edit(api: &Arc<Api>, id: &str, name: &str, value: Value) {
    let _ = patch(State(api.clone()), Path((id.to_owned(), name.to_owned())), Ok(Bytes::from(value.to_string()))).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_nodes_fixture() {
    let Some(root) = std::env::var_os("COZY_TYPED_NODE_ROOT").map(std::path::PathBuf::from) else { return };
    let home = platform::paths::Home::under(&root.join("cozy"));
    home.open().unwrap();
    std::fs::create_dir_all(home.calcifer()).unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let config = api::config::Config { hub_url: api::config::Origin::parse("http://127.0.0.1:9").unwrap(), ..Default::default() };
    let machines = crate::machines::Machines::start(calcifer.clone(), home, &root, &config);
    let api = crate::api::Api::new(calcifer, machines, "typed-node-fixture");
    let directory = root.join("native-project");
    std::fs::create_dir(&directory).unwrap();
    made_new(&api, &directory, "Typed nodes", false).unwrap();
    let project = api.projects.open(&api, &directory).unwrap();
    let id = project.id.clone();

    for kind in ["image", "video", "audio"] {
        let name = make(&api, &id, json!({"name":kind,"exact":true,"kind":kind,"fields":{}})).await;
        let Json(read) = read(State(api.clone()), Path((id.clone(), name.clone())), Query(Asked { call: false })).await.unwrap();
        assert_eq!(read.asset.kind, kind);
        assert_eq!(read.asset.fields.get(format::NODE_KIND), Some(&json!(kind)));
        assert!(!read.asset.fields.contains_key(format::TEXT));
        let written = project.folder.written().unwrap();
        let held = written.get(&name).unwrap();
        assert!(held.get(format::TEXT).is_none());
        assert!(held.get("tool").is_none());
    }
    make(&api, &id, json!({"name":"note","kind":"text","fields":{"text":"Some words."}})).await;
    let name = make(&api, &id, json!({"name":"edit","tool":"cozy/edit","kind":"image","fields":{"node_kind":"image","fps":24}})).await;
    let doc = Doc::load(&project.folder).unwrap();
    let i = doc.find(&name).unwrap();
    assert_eq!(doc.assets[i].kind, "video");
    assert_eq!(doc.assets[i].node_kind.as_deref(), Some("video"));
    assert!(doc.tool(i) == Some(Tool::Cut));
    assert!(!doc.inputs_of(i).contains_key(format::NODE_KIND));
    assert!(!doc.assets[i].own.contains(format::NODE_KIND));
    assert_eq!(doc.flat_fields(i).get(format::NODE_KIND), Some(&json!("video")));
    edit(&api, &id, "edit", json!({"fields":{"node_kind":"image"}})).await;
    assert_eq!(project.folder.written().unwrap().get("edit").unwrap().get(format::NODE_KIND), Some(&json!("video")));
    edit(&api, &id, "edit", json!({"tool":"","fields":{}})).await;
    assert_eq!(project.folder.written().unwrap().get("edit").unwrap().get(format::NODE_KIND), Some(&json!("video")));
    edit(&api, &id, "edit", json!({"tool":"cozy/edit","fields":{}})).await;
    assert_eq!(project.folder.written().unwrap().get("edit").unwrap().get("tracks"), Some(&json!([{"clips":[]}])));
    let note = doc.find("note").unwrap();
    assert_eq!(doc.assets[note].kind, "text");
    assert_eq!(doc.assets[note].text.as_deref(), Some("Some words."));

    let name = make(&api, &id, json!({"name":"picture","tool":"fixture/image","kind":"image","fields":{"instructions":"Keep these words.","steps":17}})).await;
    let stable_id = project.folder.written().unwrap().get(&name).unwrap().id.clone();
    let doc = Doc::load(&project.folder).unwrap();
    let i = doc.find(&name).unwrap();
    assert!(!doc.inputs_of(i).contains_key(format::NODE_KIND));
    assert!(!doc.assets[i].own.contains(format::NODE_KIND));
    edit(&api, &id, &name, json!({"tool":"","fields":{}})).await;
    let written = project.folder.written().unwrap();
    let held = written.get(&name).unwrap();
    assert_eq!(held.id, stable_id);
    assert_eq!(held.get(format::NODE_KIND), Some(&json!("image")));
    assert_eq!(held.get("instructions"), Some(&json!("Keep these words.")));
    assert!(held.get("steps").is_none());
    assert!(held.get(format::TEXT).is_none());
    let doc = Doc::load(&project.folder).unwrap();
    let i = doc.find(&name).unwrap();
    assert_eq!(doc.assets[i].kind, "image");
    assert!(doc.assets[i].text.is_none());

    // Current projects without this hint retain their old inference until an explicit edit.
    edit(&api, &id, &name, json!({"tool":"fixture/image","fields":{"node_kind":"image","instructions":"Still image words."}})).await;
    edit(&api, &id, &name, json!({"fields":{"node_kind":null}})).await;
    assert!(project.folder.written().unwrap().get(&name).unwrap().get(format::NODE_KIND).is_none());
    let doc = Doc::load(&project.folder).unwrap();
    assert_eq!(doc.assets[doc.find(&name).unwrap()].kind, "image");
    assert!(project.folder.written().unwrap().get(&name).unwrap().get(format::NODE_KIND).is_none(), "reading must not rewrite a current project");
    edit(&api, &id, &name, json!({"tool":"","fields":{}})).await;
    assert_eq!(project.folder.written().unwrap().get(&name).unwrap().get(format::NODE_KIND), Some(&json!("image")));
    assert!(project.folder.written().unwrap().get(&name).unwrap().get(format::TEXT).is_none());

    make(&api, &id, json!({"name":"known_video","tool":"fixture/video","kind":"video","fields":{}})).await;
    make(&api, &id, json!({"name":"known_text","tool":"fixture/text","kind":"text","fields":{}})).await;
    edit(&api, &id, "note", json!({"tool":"fixture/text","fields":{}})).await;
    let doc = Doc::load(&project.folder).unwrap();
    let note = &doc.assets[doc.find("note").unwrap()];
    assert_eq!(note.kind, "text");
    assert_eq!(note.node_kind.as_deref(), Some("text"));
    assert_eq!(note.tool.as_deref(), Some("fixture/text"));
    assert_eq!(note.inputs.get("instructions"), Some(&json!("Some words.")));
    for (name, value) in [
        ("image", json!({"tool":"fixture/video","fields":{}})),
        ("image", json!({"tool":"fixture/video","fields":{"node_kind":"image"}})),
        ("image", json!({"tool":"fixture/text","fields":{}})),
        ("video", json!({"tool":"fixture/text","fields":{}})),
        ("audio", json!({"tool":"fixture/text","fields":{}})),
        ("note", json!({"tool":"fixture/video","fields":{}})),
        ("image", json!({"tool":"cozy/edit","fields":{}})),
        ("audio", json!({"tool":"cozy/edit","fields":{}})),
    ] {
        let before = project.folder.written().unwrap();
        let seq = project.folder.seq();
        let refusal = patch(State(api.clone()), Path((id.clone(), name.to_owned())), Ok(Bytes::from(value.to_string()))).await.err().unwrap();
        assert_eq!(refusal.0.code, "asset_tool_kind");
        assert_eq!(project.folder.written().unwrap(), before, "a rejected model must not change type, tool or words");
        assert_eq!(project.folder.seq(), seq);
    }
    edit(&api, &id, "note", json!({"tool":"","fields":{}})).await;
    let doc = Doc::load(&project.folder).unwrap();
    let note = &doc.assets[doc.find("note").unwrap()];
    assert_eq!(note.kind, "text");
    assert_eq!(note.text.as_deref(), Some("Some words."));
    // A model without cached output metadata still cannot implicitly change the container.
    edit(&api, &id, "image", json!({"tool":"uncached/model","fields":{}})).await;
    let doc = Doc::load(&project.folder).unwrap();
    let image = &doc.assets[doc.find("image").unwrap()];
    assert_eq!(image.kind, "image");
    assert_eq!(image.node_kind.as_deref(), Some("image"));
    // Explicit API conversion is distinct from ordinary model assignment.
    make(&api, &id, json!({"name":"convertible","kind":"image","fields":{}})).await;
    edit(&api, &id, "convertible", json!({"tool":"fixture/video","fields":{"node_kind":"video"}})).await;
    let doc = Doc::load(&project.folder).unwrap();
    assert_eq!(doc.assets[doc.find("convertible").unwrap()].kind, "video");
    make(&api, &id, json!({"name":"convert_edit","tool":"cozy/edit","kind":"video","fields":{}})).await;
    edit(&api, &id, "convert_edit", json!({"tool":"fixture/video","fields":{"node_kind":"video"}})).await;
    let doc = Doc::load(&project.folder).unwrap();
    let converted = &doc.assets[doc.find("convert_edit").unwrap()];
    assert_eq!(converted.node_kind.as_deref(), Some("video"));
    assert_eq!(converted.tool.as_deref(), Some("fixture/video"));

    let before = project.folder.seq();
    let refused = create(State(api.clone()), Path(id.clone()), Ok(Bytes::from(json!({"name":"bad","kind":"image","fields":{"node_kind":"unknown"}}).to_string()))).await.err().unwrap();
    assert_eq!(refused.0.code, "node_kind");
    assert_eq!(project.folder.seq(), before);
    let conn = db::open(&directory.join(db::DB)).unwrap();
    let stored = db::load(&conn, &directory.join(db::DB)).unwrap();
    assert_eq!(stored.written.get("video").unwrap().get(format::NODE_KIND), Some(&json!("video")));
    assert_eq!(stored.written.get("edit").unwrap().get(format::NODE_KIND), Some(&json!("video")));
    assert_eq!(stored.written.get("picture").unwrap().id, stable_id);

    // A real imported file is v1 immediately. New results keep its approval,
    // even when the two versions have identical bytes.
    use base64::Engine as _;
    use sha2::{Digest as _, Sha256};
    let directory = root.join("version-project");
    std::fs::create_dir(&directory).unwrap();
    made_new(&api, &directory, "Version selection", false).unwrap();
    let series = api.projects.open(&api, &directory).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==").unwrap();
    let path = root.join("original.png");
    std::fs::write(&path, &bytes).unwrap();
    let hash = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    let kept = series.folder.take_held(&path, &hash).unwrap();
    let original = super::super::element(&series, &series.id, "Hei", None, None, kept, true, BTreeMap::new(), &BTreeMap::new()).unwrap();
    let doc = Doc::load(&series.folder).unwrap();
    let at = doc.find(&original.name).unwrap();
    let lock = series.folder.lock().unwrap();
    assert_eq!(lock.asset(&original.name).unwrap().versions.len(), 1);
    assert_eq!(doc.stand(&lock)[at].version(), Some(1));
    assert!(!doc.inputs_of(at).contains_key(format::SELECTED_VERSION));
    assert!(!doc.assets[at].own.contains(format::SELECTED_VERSION));
    drop(lock);
    let landing = super::super::Walk::new(&api, &series);
    let original_fields = serde_json::from_value(json!({
        "summary":"The original summary",
        "beats":[{"description":"The original beat","duration":12}],
        "models":{"model":{"ref":"author/model@1"}}
    })).unwrap();
    landing.record(None, &original.name, "image", folder::Version {
        files: folder::Files::from([("image".to_owned(), hash.clone())]),
        fields: Some(original_fields),
        tool: Some("fixture/image".to_owned()), ..Default::default()
    }, "version", false, |_| {}).unwrap();
    let doc = Doc::load(&series.folder).unwrap();
    assert_eq!(doc.stand(&series.folder.lock().unwrap())[at].version(), Some(1));
    series.folder.update_lock(|lock| assert_eq!(lock.number(&original.name, "image"), 3)).unwrap();
    landing.record_free(&original.name, "image", folder::Version {
        number: 3, files: folder::Files::from([("image".to_owned(), hash.clone())]), ..Default::default()
    }, "version", false).unwrap();
    assert_eq!(Doc::load(&series.folder).unwrap().stand(&series.folder.lock().unwrap())[at].version(), Some(1));
    landing.record(None, &original.name, "image", folder::Version {
        files: folder::Files::from([("image".to_owned(), hash.clone())]), ..Default::default()
    }, "version", true, |_| {}).unwrap();
    assert_eq!(Doc::load(&series.folder).unwrap().stand(&series.folder.lock().unwrap())[at].version(), Some(4));
    let _ = super::super::unpin(State(api.clone()), Path(series.id.clone()), Ok(Bytes::from(json!({"name":original.name}).to_string()))).await.unwrap();
    let unpinned = series.folder.written().unwrap();
    assert!(unpinned.get(&original.name).unwrap().get("approved").is_none());
    assert!(unpinned.get(&original.name).unwrap().get(format::SELECTED_VERSION).is_none());
    let Json(selected) = super::super::approve(State(api.clone()), Path(series.id.clone()), Ok(Bytes::from(json!({"name":original.name,"version":"1"}).to_string()))).await.unwrap();
    assert_eq!(selected.number, 1);
    let doc = Doc::load(&series.folder).unwrap();
    let lock = series.folder.lock().unwrap();
    assert_eq!(doc.stand(&lock)[at].version(), Some(1));
    assert_eq!(doc.version_files(at, 2, &lock).unwrap().get("image"), Some(&hash));
    drop(lock);
    let conn = db::open(&directory.join(db::DB)).unwrap();
    let stored = db::load(&conn, &directory.join(db::DB)).unwrap();
    assert_eq!(stored.written.get(&original.name).unwrap().get(format::SELECTED_VERSION), Some(&json!(1)));
    let Json(joined) = join(State(api.clone()), Path((series.id.clone(), original.name.clone())), Ok(Bytes::from(json!({"source":hash}).to_string()))).await.unwrap();
    assert_eq!(joined.stands, Some(1), "joining another version preserves the approved reference");

    let first = make(&api, &series.id, json!({"name":"first-output","kind":"image","fields":{}})).await;
    landing.record(None, &first, "image", folder::Version {
        files: folder::Files::from([("image".to_owned(), hash.clone())]), ..Default::default()
    }, "version", false, |_| {}).unwrap();
    let first_doc = Doc::load(&series.folder).unwrap();
    assert_eq!(first_doc.stand(&series.folder.lock().unwrap())[first_doc.find(&first).unwrap()].version(), Some(1));
    assert!(first_doc.assets[first_doc.find(&first).unwrap()].pin.is_some());

    // A current image remains renderable, while a reader's completed selected
    // input is independent of another generation of that image.
    edit(&api, &series.id, &original.name, json!({"tool":"fixture/image","fields":{"instructions":"edit the current image"}})).await;
    let Json(history) = read(State(api.clone()), Path((series.id.clone(), original.name.clone())), Query(Asked { call: false })).await.unwrap();
    let original_request = history.versions.iter().find(|version| version.number == 2).unwrap().fields.as_ref().unwrap();
    assert_eq!(original_request.get("summary"), Some(&json!("The original summary")));
    assert_eq!(original_request.get("beats"), Some(&json!([{"description":"The original beat","duration":12}])));
    let conn = db::open(&directory.join(db::DB)).unwrap();
    let stored = db::load(&conn, &directory.join(db::DB)).unwrap();
    assert_eq!(stored.lock.asset(&original.name).unwrap().version(2).unwrap().fields.as_ref(), Some(original_request));
    assert!(history.versions.iter().find(|version| version.number == 1).unwrap().fields.is_none(), "an import has no historical authoring request");
    let source_prompt = format!("Keep <{}.1> exactly", original.name);
    edit(&api, &series.id, &original.name, json!({"fields":{"summary":source_prompt}})).await;
    let doc = Doc::load(&series.folder).unwrap();
    let lock = series.folder.lock().unwrap();
    let source_at = doc.find(&original.name).unwrap();
    let authored = doc.authoring_fields(source_at);
    let callable: api::interface::Callable = serde_json::from_value(json!({
        "name":"image", "request":{"fields":[{"name":"summary","type":{"prompt":["image"]}}]}
    })).unwrap();
    let bound = super::super::binder::bind(&super::super::binder::Call {
        name: &original.name, tool: "fixture/image", package: "fixture", callable: &callable,
        fields: &authored, reserved: &[], doc: &doc, own: Some(source_at), lock: &lock,
        stand: &doc.stand(&lock), gone: &BTreeSet::new(), pickup: None, inline: &[],
    }).unwrap();
    assert_eq!(authored.get("summary"), Some(&json!(source_prompt)));
    assert_eq!(bound.payload["summary"]["text"], json!(source_prompt));
    assert!(bound.payload["summary"]["references"].is_array(), "binding genuinely produces a structured Prompt");
    drop(lock);
    let reader = make(&api, &series.id, json!({"name":"reader","tool":"fixture/video","kind":"video","fields":{"summary":format!("<{}>", original.name)}})).await;
    {
        use super::super::{AskedText, Graph, Named, Now, Turn, walker};
        let lock = series.folder.lock().unwrap();
        let mut reading = AskedText::new(Doc::load(&series.folder).unwrap());
        let mut graph = Graph::default();
        let background = folder::Ask { id: "background-image".into(), ..Default::default() };
        let jobs = graph.add(&background.id, &mut Now { reading: &mut reading, lock: &lock, ask: &background }, &walker::Asked {
            takes: vec![Named { key: "image-next".into(), asset: original.name.clone(), approve: true }], ..Default::default()
        });
        graph.go(jobs[0]);
        let video = folder::Ask { id: "named-video".into(), ..Default::default() };
        let jobs = graph.add(&video.id, &mut Now { reading: &mut reading, lock: &lock, ask: &video }, &walker::Asked {
            takes: vec![Named { key: "video-next".into(), asset: reader.clone(), approve: true }], ..Default::default()
        });
        assert!(graph.jobs[jobs[0]].needs.is_empty(), "a selected image does not wait for a background generation");
        assert_eq!(graph.turn(jobs[0], &mut Now { reading: &mut reading, lock: &lock, ask: &video }), Turn::Go);
    }

    // Landing a same-byte replacement must not even change the frozen version
    // number. Explicit --outdated is the opt-in that refreshes that snapshot.
    let text = series.folder.text().unwrap();
    let digest = hex::encode(Sha256::digest(text.as_bytes()));
    series.folder.keep_text(&digest, &text).unwrap();
    let snapshot = folder::Ask { id: "selected-snapshot".into(), digest, ..Default::default() };
    series.folder.update_walk(|walk| walk.walk.asks.push(snapshot.clone())).unwrap();
    let frozen_version = || {
        let walk = series.folder.walk_now().unwrap();
        let ask = walk.walk.asks.iter().find(|ask| ask.id == snapshot.id).unwrap();
        let doc = Doc::read(&series.folder, &series.folder.text_of(ask).unwrap()).unwrap();
        doc.stand(&series.folder.lock().unwrap())[doc.find(&original.name).unwrap()].version()
    };
    let walk = super::super::Walk::new(&api, &series);
    walk.select_asked(&snapshot.id, &original.name, 2, &hash).unwrap();
    assert_eq!(frozen_version(), Some(1));
    series.folder.update_walk(|walk| walk.walk.asks.iter_mut().find(|ask| ask.id == snapshot.id).unwrap().outdated = true).unwrap();
    walk.select_asked(&snapshot.id, &original.name, 2, &hash).unwrap();
    assert_eq!(frozen_version(), Some(2));
    series.folder.update_walk(|walk| walk.walk.asks.retain(|ask| ask.id != snapshot.id)).unwrap();

    let empty = make(&api, &series.id, json!({"name":"unfinished","tool":"fixture/image","kind":"image","fields":{"instructions":"its first image"}})).await;
    let waiting = make(&api, &series.id, json!({"name":"waiting","tool":"fixture/video","kind":"video","fields":{"summary":format!("<{}>", empty)}})).await;
    let (_, Json(asked)) = super::super::runs(State(api.clone()), Path(series.id.clone()), Ok(Bytes::from(json!({"names":["all"],"on":["local"]}).to_string()))).await.unwrap();
    assert_eq!(asked.added, 4);
    assert!(!asked.skipped.contains(&original.name));
    let reader_job = asked.board.jobs.iter().find(|job| job.asset == reader).unwrap();
    assert!(reader_job.needs.is_empty(), "{reader_job:?}");
    let waiting_job = asked.board.jobs.iter().find(|job| job.asset == waiting).unwrap();
    assert_eq!(waiting_job.needs, vec![empty], "{waiting_job:?}");
}
