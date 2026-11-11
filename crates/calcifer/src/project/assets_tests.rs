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
    patch(State(api.clone()), Path((id.to_owned(), name.to_owned())), HeaderMap::new(), Ok(Bytes::from(value.to_string()))).await.unwrap();
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
    assert_eq!(doc.assets[i].node_kind.as_deref(), Some("video_edit"));
    assert!(doc.tool(i) == Some(Tool::Cut));
    assert!(!doc.inputs_of(i).contains_key(format::NODE_KIND));
    assert!(!doc.assets[i].own.contains(format::NODE_KIND));
    assert_eq!(doc.flat_fields(i).get(format::NODE_KIND), Some(&json!("video_edit")));
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

    let before = project.folder.seq();
    let refused = create(State(api.clone()), Path(id.clone()), Ok(Bytes::from(json!({"name":"bad","kind":"image","fields":{"node_kind":"unknown"}}).to_string()))).await.err().unwrap();
    assert_eq!(refused.0.code, "node_kind");
    assert_eq!(project.folder.seq(), before);
    let conn = db::open(&directory.join(db::DB)).unwrap();
    let stored = db::load(&conn, &directory.join(db::DB)).unwrap();
    assert_eq!(stored.written.get("video").unwrap().get(format::NODE_KIND), Some(&json!("video")));
    assert_eq!(stored.written.get("edit").unwrap().get(format::NODE_KIND), Some(&json!("video_edit")));
    assert_eq!(stored.written.get("picture").unwrap().id, stable_id);
}
