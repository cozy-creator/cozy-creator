//! Storage that isn't the current native `project.db` is refused, and nothing writes sidecopies.
use super::*;

fn entries(dir: &FsPath) -> BTreeSet<String> {
    std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn root_export_replacement_keeps_retained_media_without_edited_sidecopies() {
    let directory = tempfile::tempdir().unwrap();
    let retained = directory.path().join("files");
    std::fs::create_dir(&retained).unwrap();
    let version = retained.join("selected.mp4");
    std::fs::write(&version, b"retained selected version").unwrap();
    let root = directory.path().join("edit.mp4");
    std::fs::write(&root, b"external root edit").unwrap();
    tools::publish(&version, &root).unwrap();
    assert_eq!(std::fs::read(&root).unwrap(), b"retained selected version");
    assert_eq!(std::fs::read(&version).unwrap(), b"retained selected version");
    assert_eq!(entries(directory.path()), BTreeSet::from(["edit.mp4".to_owned(), "files".to_owned()]));
    assert_eq!(entries(&retained), BTreeSet::from(["selected.mp4".to_owned()]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_calls_require_readable_current_native_storage() {
    let directory = tempfile::tempdir().unwrap();
    let home = platform::paths::Home::under(&directory.path().join("controller"));
    home.open().unwrap();
    std::fs::create_dir_all(home.calcifer()).unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let clips = crate::runs::clip::Clips { dir: directory.path().join("clips"), home };
    let callable: api::interface::Callable = serde_json::from_value(json!({"name": "call", "request": {}})).unwrap();
    for kind in ["schema1", "corrupt", "current"] {
        let root = directory.path().join(kind);
        std::fs::create_dir(&root).unwrap();
        let database = root.join(db::DB);
        match kind {
            "schema1" => rusqlite::Connection::open(&database).unwrap().pragma_update(None, "user_version", 1).unwrap(),
            _ => {
                db::create(&database, &db::Stored::default(), &[], &[]).unwrap();
                if kind == "corrupt" {
                    let connection = db::open(&database).unwrap();
                    connection.execute("UPDATE crdt_document SET snapshot = ?1", [b"invalid native bytes".as_slice()]).unwrap();
                }
            }
        }
        let before = entries(&root);
        let result = Holding::new(calcifer.clone(), root.clone())
            .bound("fixture/call", "fixture", &callable, Map::new(), &clips, &[]).await;
        if kind == "current" {
            let (fields, files, _) = result.unwrap();
            assert!(fields.is_empty() && files.is_empty());
        } else {
            let error = result.unwrap_err();
            if kind == "schema1" { assert_eq!(error.code, "project_db_version"); }
            if kind != "corrupt" { assert_eq!(entries(&root), before); }
        }
        assert!(!root.join("project.db.v1").exists());
    }
    calcifer.close();
}
