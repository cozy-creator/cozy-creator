use super::*;

#[test]
fn public_codes_preserve_legacy_identity_and_lookup() {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "project::code_tests::public_code_fixture", "--nocapture"])
        .env("COZY_PROJECT_CODE_ROOT", root.path())
        .env("TENSORFS_HOME", root.path().join("tensorfs"))
        .output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_code_fixture() {
    let Some(root) = std::env::var_os("COZY_PROJECT_CODE_ROOT").map(PathBuf::from) else { return };
    let home = platform::paths::Home::under(&root.join("cozy"));
    home.open().unwrap();
    std::fs::create_dir_all(home.calcifer()).unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let config = api::config::Config { hub_url: api::config::Origin::parse("http://127.0.0.1:9").unwrap(), ..Default::default() };
    let machines = crate::machines::Machines::start(calcifer.clone(), home, &root, &config);
    let api = crate::api::Api::new(calcifer, machines, "project-code-fixture");
    let dir = root.join("legacy");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join(db::DB);
    db::create(&path, &db::Stored { written: written::Written::blank().into(), ..Default::default() }, &[], &[]).unwrap();
    let mut conn = db::open(&path).unwrap();
    let id = folder_id(&dir);
    db::set_identity(&mut conn, &path, &id, "Legacy", None).unwrap();
    let unsupported = root.join("unsupported");
    std::fs::create_dir(&unsupported).unwrap();
    let unsupported_id = folder_id(&unsupported);
    rusqlite::Connection::open(unsupported.join(db::DB)).unwrap()
        .pragma_update(None, "user_version", 9999).unwrap();
    api.calcifer.store().write(|tx| {
        tx.execute("INSERT INTO projects (id, name, path, used_at_ms) VALUES (?1, 'Legacy', ?2, 42)", rusqlite::params![id, dir.to_string_lossy()])?;
        tx.execute("INSERT INTO projects (id, name, path, used_at_ms) VALUES (?1, 'Unsupported', ?2, 43)", rusqlite::params![unsupported_id, unsupported.to_string_lossy()])?;
        Ok(())
    }).unwrap();

    // Listing assigns the readable project's code even when a different project's DB is unsupported.
    let listed = indexed(&api).unwrap();
    assert_eq!(listed.len(), 2);
    let legacy = listed.iter().find(|entry| entry.id == id).unwrap();
    let code = legacy.code.clone();
    assert_eq!(code::parse(&code), Some(code.clone()));
    assert_eq!(legacy.used_at_ms, 42);
    assert!(listed.iter().find(|entry| entry.id == unsupported_id).unwrap().code.is_empty());
    assert_eq!(known(&api, &unsupported_id).unwrap().path, unsupported);
    let refusal = api.projects.get(&api, &unsupported_id).err().expect("unsupported DB is refused when opened");
    assert_eq!(refusal.0.code, "project_db_version");
    assert_eq!(db::identity(&conn), Some((id.clone(), "Legacy".into())));
    assert_eq!(db::meta(&conn, "code"), Some(code.clone()));
    assert_eq!(identify(&api, &dir).unwrap(), (id.clone(), "Legacy".into(), code.clone()));
    assert_eq!(identify(&api, &dir).unwrap().2, code);
    db::set_meta(&conn, &path, "code", None).unwrap();
    assert_eq!(identify(&api, &dir).unwrap().2, code);
    assert_eq!(db::meta(&conn, "code"), Some(code.clone()));
    assert_eq!(known(&api, &code.to_ascii_lowercase()).unwrap().id, id);
    assert_eq!(known(&api, "legacy").unwrap().id, id);
    assert_eq!(named(&api, dir.to_str().unwrap()).unwrap(), dir);

    // A code remains authoritative when another project's display name is identical.
    let other = root.join("other");
    std::fs::create_dir(&other).unwrap();
    made_new(&api, &other, &code, false).unwrap();
    let (other_id, _, other_code) = identify(&api, &other).unwrap();
    assert_ne!(other_code, code);
    assert_eq!(known(&api, &code).unwrap().id, id);
    assert_eq!(known(&api, &other_id).unwrap().id, other_id);

    // All API records carry the public code while their id stays canonical.
    assert_eq!(opened(id.clone(), code.clone(), &dir).code, code);
    let project = api.projects.get(&api, &code).unwrap();
    let state = model::of(&api, &project).await.unwrap().state(&project, true);
    assert_eq!(state.id, id);
    assert_eq!(state.code, code);
    let Json(listed) = list(State(api.clone()), Query(Page { limit: None, before: None })).await.unwrap();
    assert_eq!(listed.projects.iter().find(|entry| entry.id == id).unwrap().code, code);
    drop(project);

    // A copied database gets a new identity and code without changing the source.
    let copy = root.join("copy");
    std::fs::create_dir(&copy).unwrap();
    let copy_path = copy.join(db::DB);
    db::create(&copy_path, &db::Stored { written: written::Written::blank().into(), ..Default::default() }, &[], &[]).unwrap();
    db::set_identity(&mut db::open(&copy_path).unwrap(), &copy_path, &id, "Legacy", Some(&code)).unwrap();
    let (copy_id, _, copy_code) = identify(&api, &copy).unwrap();
    assert_ne!(copy_id, id);
    assert_ne!(copy_code, code);
    assert_eq!(db::meta(&conn, "code"), Some(code));
}
