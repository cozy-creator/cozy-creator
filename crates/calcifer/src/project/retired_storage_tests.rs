//! Retired folder storage is recognized only to refuse it; explicit current authoring text
//! remains supported after the caller has created native storage.
use super::*;

fn entries(dir: &FsPath) -> BTreeSet<String> {
    std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn yaml_only_folders_are_refused_without_import_backup_or_stage() {
    for version in [2, 3, 4] {
        let directory = tempfile::tempdir().unwrap();
        let yaml = format!("version: {version}\nassets: {{}}\n");
        let lock = format!("{{\"format\":{version},\"assets\":{{}},\"files\":{{}}}}");
        std::fs::write(directory.path().join(folder::PROJECT), &yaml).unwrap();
        std::fs::write(directory.path().join(folder::LOCK), &lock).unwrap();
        let before = entries(directory.path());
        assert!(Folder::is_project(directory.path()), "old markers must reach the precise refusal");
        let error = Folder::at(directory.path()).text().unwrap_err();
        assert_eq!(error.code, "project_format_unsupported");
        assert_eq!(entries(directory.path()), before);
        assert_eq!(std::fs::read_to_string(directory.path().join(folder::PROJECT)).unwrap(), yaml);
        assert_eq!(std::fs::read_to_string(directory.path().join(folder::LOCK)).unwrap(), lock);
    }
}

#[test]
fn old_staged_import_is_not_resumed_or_archived() {
    let directory = tempfile::tempdir().unwrap();
    let staged = directory.path().join("project.db.new");
    std::fs::write(&staged, b"unfinished retired import").unwrap();
    let archive = directory.path().join(".cozy-legacy-v1");
    std::fs::create_dir(&archive).unwrap();
    std::fs::write(archive.join(folder::PROJECT), b"version: 3\nassets: {}\n").unwrap();
    let before = entries(directory.path());
    let error = Folder::at(directory.path()).text().unwrap_err();
    assert_eq!(error.code, "project_format_unsupported");
    assert_eq!(entries(directory.path()), before);
    assert_eq!(std::fs::read(&staged).unwrap(), b"unfinished retired import");
    assert!(!directory.path().join(db::DB).exists());
}

#[test]
fn retired_folder_refuses_media_writes_before_changing_its_files() {
    let directory = tempfile::tempdir().unwrap();
    let old = directory.path().join("old-project");
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join(folder::PROJECT), b"version: 2\nassets: {}\n").unwrap();
    let incoming = directory.path().join("incoming.bin");
    std::fs::write(&incoming, b"generated output outside the retired folder").unwrap();
    let digest = folder::hash(&incoming).unwrap();
    let before = entries(&old);
    let folder = Folder::at(&old);
    assert_eq!(folder.keep(&incoming).unwrap_err().code, "project_format_unsupported");
    assert_eq!(folder.take(&incoming).unwrap_err().code, "project_format_unsupported");
    let made = folder::RunFile { run: 1, sha256: digest.clone(), ..Default::default() };
    assert_eq!(folder.adopt(&incoming, &digest, made, None).unwrap_err().code, "project_format_unsupported");
    assert_eq!(entries(&old), before);
    assert_eq!(std::fs::read(&incoming).unwrap(), b"generated output outside the retired folder");
}

#[test]
fn native_project_keeps_current_authoring_import_and_rejects_old_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(db::DB);
    db::create(&path, &db::Stored::default(), &[], &[]).unwrap();
    let mut connection = db::open(&path).unwrap();
    db::set_identity(&mut connection, &path, "project-current", "Current", None).unwrap();
    drop(connection);
    let folder = Folder::at(directory.path());
    let lock = folder.lock().unwrap();
    assert!(Doc::of_text(&folder, "version: 4\nassets:\n  note: {text: Current authoring.}\n", &lock).is_ok());
    for version in [2, 3] {
        let text = format!("version: {version}\nassets: {{}}\n");
        let error = Doc::of_text(&folder, &text, &lock).err().unwrap();
        assert_eq!(error.code, "project_format");
    }
    assert!(!directory.path().join(".cozy-legacy-v1").exists());
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
    for kind in ["yaml", "schema1", "corrupt", "current"] {
        let root = directory.path().join(kind);
        std::fs::create_dir(&root).unwrap();
        let database = root.join(db::DB);
        match kind {
            "yaml" => std::fs::write(root.join(folder::PROJECT), b"version: 4\nassets: {}\n").unwrap(),
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
            if kind == "yaml" { assert_eq!(error.code, "project_format_unsupported"); }
            if kind == "schema1" { assert_eq!(error.code, "project_db_version"); }
            if kind != "corrupt" { assert_eq!(entries(&root), before); }
        }
        assert!(!root.join("project.db.v1").exists());
        assert!(!root.join(".cozy-legacy-v1").exists());
    }
    calcifer.close();
}
