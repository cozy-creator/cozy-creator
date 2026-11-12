//! Actual native database/receipt recovery. Driver, media and inference roundtrips
//! are qualified separately by the installed cross-platform CLI fixture.
use super::*;

#[test]
fn rejected_vault_credentials_have_a_specific_error() {
    let rejected = key(project_vault::Error::Unlock);
    assert_eq!(rejected.kind, Kind::Credential);
    assert_eq!(rejected.code, "vault_unlock_failed");
}
use super::super::{db, written};
use base64::{Engine as _, engine::general_purpose::STANDARD};

const FIXTURE: &str = "project::vault::decrypt_tests::native_plain_recovery_fixture";
const RESTART: &str = "project::vault::decrypt_tests::native_plain_journal_restart_fixture";

#[test]
fn vault_inventory_matches_copied_file_and_byte_totals() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("files")).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(source.join("files/first"), b"retained").unwrap();
    fs::write(source.join("files/second"), b"media").unwrap();
    fs::write(source.join("empty"), []).unwrap();
    fs::write(source.join("project.db-wal"), [1u8; 100]).unwrap();
    fs::write(source.join("project.db-shm"), [2u8; 100]).unwrap();
    let mut progress = Progress::quiet();
    progress.begin_inventory();
    walk_tree(&source, None, FsPath::new(""), false, false, &mut progress).unwrap();
    assert_eq!(progress.counts(), (0, 3, 0, 13));
    walk_tree(&source, Some(&target), FsPath::new(""), false, false, &mut progress).unwrap();
    assert_eq!(progress.counts(), (3, 3, 13, 13));
    assert!(!target.join("project.db-wal").exists());
    assert!(!target.join("project.db-shm").exists());
}

#[cfg(unix)]
#[test]
fn legacy_missing_media_is_skipped_but_live_external_link_is_refused() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("files")).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(source.join("files/retained.jpg"), b"retained original bytes").unwrap();
    symlink(temp.path().join("missing.jpg"), source.join("files/missing.jpg")).unwrap();
    let mut progress = Progress::quiet();
    walk_tree(&source, None, FsPath::new(""), false, false, &mut progress).unwrap();
    walk_tree(&source, Some(&target), FsPath::new(""), false, false, &mut progress).unwrap();
    assert_eq!(progress.skipped(), vec![PathBuf::from("files/missing.jpg")]);
    assert_eq!(fs::read(target.join("files/retained.jpg")).unwrap(), b"retained original bytes");
    assert!(fs::symlink_metadata(target.join("files/missing.jpg")).is_err());
    assert!(fs::symlink_metadata(source.join("files/missing.jpg")).unwrap().file_type().is_symlink());

    let outside = temp.path().join("outside.jpg");
    fs::write(&outside, b"outside the project").unwrap();
    symlink(outside, source.join("files/external.jpg")).unwrap();
    let error = walk_tree(&source, None, FsPath::new(""), false, false, &mut Progress::quiet()).unwrap_err();
    assert_eq!(error.code, "vault_symlink");
    assert!(error.message.contains("files/external.jpg"));

    let absent_directory = temp.path().join("absent-directory");
    let invalid_root = temp.path().join("invalid-root");
    fs::create_dir(&invalid_root).unwrap();
    symlink(absent_directory, invalid_root.join("files")).unwrap();
    assert_eq!(walk_tree(&invalid_root, None, FsPath::new(""), false, false, &mut Progress::quiet()).unwrap_err().code, "vault_symlink");
}

#[test]
fn decrypt_recovery_and_plain_retention() {
    let owned = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", FIXTURE, "--nocapture"])
        .env("COZY_DECRYPT_FIXTURE", owned.path())
        .env("TENSORFS_HOME", owned.path().join("tensorfs"))
        .output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "native decrypt fixture: {}", output.status);
    owned.close().expect("native decrypt fixture cleanup");
}

fn native_project(path: &FsPath, id: &str) {
    fs::create_dir_all(path).unwrap();
    let at = path.join(db::DB);
    db::create(&at, &db::Stored { written: written::Written::default().into(), ..Default::default() }, &[], &[]).unwrap();
    let mut conn = db::open(&at).unwrap();
    db::set_identity(&mut conn, &at, id, "Retained project", Some("Proof123")).unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").unwrap();
}

#[test]
fn encrypted_conversion_cleanup() {
    // Real keybags, native SQLite and durable directory receipts. Installed CLI
    // tests separately exercise the actual mount/copy/verify provider path.
    let temp = tempfile::tempdir().unwrap();
    let id = format!("source-policy-{}", fresh());
    let created = Keybag::create(&id, SecretString::from("synthetic source policy".to_owned())).unwrap();
    // After the cutover: the original goes only once an unlock finishes the conversion.
    let root = temp.path().join("switched");
    let saved = temp.path().join(".cozy-project-source-switched");
    native_project(&saved, &id);
    fs::write(saved.join("retained.txt"), b"sole good original").unwrap();
    fs::create_dir(&root).unwrap();
    created.keybag.save(&root.join(KEYBAG_FILE)).unwrap();
    fs::create_dir(root.join(BACKING)).unwrap();
    let record = Conversion { root: root.clone(), staged: temp.path().join(".cozy-vault-stage-switched"), saved: saved.clone(),
        key_id: created.keybag.info().key_id, source_id: directory_id(&saved).unwrap(), staged_id: None, ready: true, plain: None };
    let mut wrong = record.clone();
    wrong.key_id.push_str("-mismatch");
    save_conversion(&wrong).unwrap();
    assert!(finish_conversion(&root).is_err());
    assert!(recover_conversion(&root).is_err());
    assert_eq!(fs::read(saved.join("retained.txt")).unwrap(), b"sole good original");
    save_conversion(&record).unwrap();
    // Restart sees a replacement, but may not delete the original before an
    // authorized reopen proves a credential opens it.
    recover_conversion(&root).unwrap();
    assert!(Keybag::load(&root.join(KEYBAG_FILE)).unwrap()
        .unlock(Credential::Passphrase(SecretString::from("incorrect synthetic passphrase".to_owned()))).is_err());
    assert!(saved.join("retained.txt").is_file() && receipt_path(&root).is_file());
    finish_conversion(&root).unwrap();
    assert!(!saved.exists() && !receipt_path(&root).exists());

    // Before the cutover: an interrupted stage goes, whole or half-made, and the original stays as it was.
    for (name, filled) in [("empty", false), ("copied", true)] {
        let root = temp.path().join(format!("interrupted-{name}"));
        native_project(&root, &id);
        let before = hash(&root.join(db::DB)).unwrap();
        let staged = temp.path().join(format!(".cozy-vault-stage-{name}"));
        let mut record = Conversion { root: root.clone(), staged: staged.clone(), saved: temp.path().join(format!(".cozy-project-source-{name}")),
            key_id: created.keybag.info().key_id, source_id: directory_id(&root).unwrap(), staged_id: None, ready: false, plain: None };
        fs::create_dir(&staged).unwrap();
        if filled {
            record.staged_id = Some(directory_id(&staged).unwrap());
            created.keybag.save(&staged.join(KEYBAG_FILE)).unwrap();
            fs::create_dir(staged.join(BACKING)).unwrap();
            fs::write(staged.join(BACKING).join("half"), b"half-made ciphertext").unwrap();
        }
        save_conversion(&record).unwrap();
        recover_conversion(&root).unwrap();
        assert_eq!(hash(&root.join(db::DB)).unwrap(), before);
        assert!(!staged.exists() && !receipt_path(&root).exists());
    }

    // A stage whose identity was never recorded can only be removed while it is empty.
    let root = temp.path().join("foreign");
    native_project(&root, &id);
    let staged = temp.path().join(".cozy-vault-stage-foreign");
    fs::create_dir(&staged).unwrap();
    fs::write(staged.join("unrelated.txt"), b"not the conversion's").unwrap();
    save_conversion(&Conversion { root: root.clone(), staged: staged.clone(), saved: temp.path().join(".cozy-project-source-foreign"),
        key_id: created.keybag.info().key_id, source_id: directory_id(&root).unwrap(), staged_id: None, ready: false, plain: None }).unwrap();
    assert!(recover_conversion(&root).is_err());
    assert!(staged.join("unrelated.txt").is_file() && receipt_path(&root).is_file());
}

fn staged(root: &FsPath, name: &str, bag: &Keybag) -> Conversion {
    let directory = root.join(name);
    fs::create_dir(&directory).unwrap();
    bag.save(&directory.join(KEYBAG_FILE)).unwrap();
    // No provider is mocked or invoked: this is the durable layout after the
    // original encrypted provider has closed, identified by its real keybag.
    fs::create_dir(directory.join(BACKING)).unwrap();
    let suffix = fresh();
    let stage = root.join(format!(".cozy-vault-stage-{suffix}"));
    native_project(&stage, &bag.info().project_id);
    fs::write(stage.join("retained.txt"), b"unchanged retained bytes").unwrap();
    Conversion {
        root: directory.clone(), staged: stage.clone(), saved: root.join(format!(".cozy-project-source-{suffix}")),
        key_id: bag.info().key_id, source_id: directory_id(&directory).unwrap(), staged_id: Some(directory_id(&stage).unwrap()), ready: false,
        plain: Some(PlainConversion { project_id: bag.info().project_id, generation: bag.info().generation, cleanup_started: false }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_plain_recovery_fixture() {
    let Some(root) = std::env::var_os("COZY_DECRYPT_FIXTURE").map(PathBuf::from) else { return };
    let id = format!("decrypt-{}", fresh());
    let created = Keybag::create_generation(&id, 2, SecretString::from("native fixture passphrase".to_owned())).unwrap();

    // A killed copy (including a partial plaintext file) leaves the encrypted
    // original authoritative. Only the recorded stage is removed on restart.
    let copying = staged(&root, "copying", &created.keybag);
    fs::write(copying.staged.join("partial"), b"partial copy").unwrap();
    save_conversion(&copying).unwrap();
    assert!(finish_conversion(&copying.root).is_err());
    assert!(receipt_path(&copying.root).is_file() && copying.staged.join("partial").is_file());
    recover_conversion(&copying.root).unwrap();
    assert!(copying.root.join(KEYBAG_FILE).is_file());
    assert!(!copying.staged.exists() && !receipt_path(&copying.root).exists());

    // Replacing an owned stage or redirecting its receipt must never delete
    // either that replacement or another directory.
    let mut changed = staged(&root, "replaced", &created.keybag);
    let actual = changed.staged_id;
    changed.staged_id.as_mut().unwrap()[1] ^= 1;
    save_conversion(&changed).unwrap();
    assert!(recover_conversion(&changed.root).is_err());
    assert!(changed.root.join(KEYBAG_FILE).is_file() && changed.staged.join("retained.txt").is_file());
    changed.staged_id = actual;
    let true_stage = changed.staged.clone();
    let elsewhere = root.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    changed.staged = elsewhere.join(true_stage.file_name().unwrap());
    fs::create_dir(&changed.staged).unwrap();
    fs::write(changed.staged.join("unrelated"), b"preserve me").unwrap();
    save_conversion(&changed).unwrap();
    assert!(recover_conversion(&changed.root).is_err());
    assert_eq!(fs::read(changed.staged.join("unrelated")).unwrap(), b"preserve me");
    changed.staged = true_stage;
    save_conversion(&changed).unwrap();
    recover_conversion(&changed.root).unwrap();

    // Crash after the first directory rename, before installation. The native
    // database, immutable bytes and generation survive the real second rename.
    let mut ready = staged(&root, "current", &created.keybag);
    let readonly = ready.staged.join("readonly.txt");
    fs::write(&readonly, b"retained readonly bytes").unwrap();
    let mut permissions = fs::metadata(&readonly).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&readonly, permissions).unwrap();
    let object = fresh();
    fs::create_dir(ready.staged.join(JOURNAL_DIR)).unwrap();
    fs::write(ready.staged.join(JOURNAL_DIR).join(&object), serde_json::to_vec(&PrivateRecord {
        project_id: id.clone(), context: "retained-history".into(), data: STANDARD.encode(b"old journal bytes"),
    }).unwrap()).unwrap();
    fs::create_dir_all(ready.staged.join(".vault-machine/objects")).unwrap();
    fs::write(ready.staged.join(".vault-machine/objects/source"), b"original immutable source").unwrap();
    fs::create_dir_all(ready.staged.join(".vault-machine/generations")).unwrap();
    fs::write(ready.staged.join(".vault-machine/generations/rebuildable"), b"old physical environment").unwrap();
    sync_tree(&ready.staged).unwrap();
    ready.ready = true;
    save_conversion(&ready).unwrap();
    fs::rename(&ready.root, &ready.saved).unwrap();
    assert!(keeps_index(&ready.root), "the durable receipt must retain a temporarily missing project after interruption");
    recover_conversion(&ready.root).unwrap();
    assert!(!ready.saved.exists() && !ready.staged.exists() && !receipt_path(&ready.root).exists());
    assert_eq!(plain_project_id(&ready.root).as_deref(), Some(id.as_str()));
    assert_eq!(fs::read(ready.root.join("readonly.txt")).unwrap(), b"retained readonly bytes");
    assert!(fs::metadata(ready.root.join("readonly.txt")).unwrap().permissions().readonly());
    assert_eq!(fs::read(ready.root.join("retained.txt")).unwrap(), b"unchanged retained bytes");
    assert_eq!(crate::private_io::retained_scope(&ready.root, &id).unwrap().unwrap().storage.generation, 2);

    // A future re-encryption copies retained closure but rebuilds environments
    // whose absolute physical mount paths are no longer valid.
    let next = root.join("next-copy"); fs::create_dir(&next).unwrap();
    copy_tree(&ready.root, &next).unwrap();
    assert_eq!(fs::read(next.join(".vault-machine/objects/source")).unwrap(), b"original immutable source");
    assert!(!next.join(".vault-machine/generations").exists());
    assert!(!next.join(api::machine::LOCAL_PRIVATE_MARKER).exists());

    // Crash after the second rename, with the source still retained, is also
    // recoverable; tampering with its FileId refuses cleanup first.
    let mut installed = staged(&root, "installed", &created.keybag);
    installed.ready = true;
    save_conversion(&installed).unwrap();
    fs::rename(&installed.root, &installed.saved).unwrap();
    fs::rename(&installed.staged, &installed.root).unwrap();
    installed.source_id[1] ^= 1;
    save_conversion(&installed).unwrap();
    assert!(recover_conversion(&installed.root).is_err());
    assert!(installed.saved.join(KEYBAG_FILE).is_file() && installed.root.join("retained.txt").is_file());
    installed.source_id[1] ^= 1;
    save_conversion(&installed).unwrap(); recover_conversion(&installed.root).unwrap();

    // Deletion may have removed the old keybag before a crash. Its durable
    // cleanup authorization and original directory ID make this retry safe.
    let mut cleanup = staged(&root, "partial-cleanup", &created.keybag);
    cleanup.ready = true;
    save_conversion(&cleanup).unwrap();
    fs::rename(&cleanup.root, &cleanup.saved).unwrap();
    fs::rename(&cleanup.staged, &cleanup.root).unwrap();
    cleanup.plain.as_mut().unwrap().cleanup_started = true;
    save_conversion(&cleanup).unwrap();
    fs::remove_file(cleanup.saved.join(KEYBAG_FILE)).unwrap();
    assert!(cleanup.saved.join(BACKING).exists());
    recover_pending_plain(&cleanup.root).unwrap();
    assert!(cleanup.root.join("retained.txt").is_file() && !cleanup.saved.exists());
    assert!(!receipt_path(&cleanup.root).exists());
    assert!(recover_pending_plain(&cleanup.root).is_err(), "a missing receipt must not mask an earlier durability error");

    // An overlapping public recovery skips an owned operation instead of
    // capturing its old receipt. A second intent at the SAME path survives.
    let serial = staged(&root, "serialized", &created.keybag); save_conversion(&serial).unwrap();
    let op = operation(&serial.root); let operation_guard = op.lock().unwrap();
    let writer = exclusive(&serial.root).unwrap();
    let before = fs::read(receipt_path(&serial.root)).unwrap();
    let contender = std::thread::spawn({ let path = serial.root.clone(); move || recover_conversion(&path) });
    contender.join().unwrap().unwrap();
    assert_eq!(fs::read(receipt_path(&serial.root)).unwrap(), before);
    assert!(serial.staged.join("retained.txt").exists());
    recover_under_guards(&serial.root).unwrap();
    assert!(!receipt_path(&serial.root).exists());
    let mut next_intent = serial.clone(); let suffix = fresh();
    next_intent.staged = root.join(format!(".cozy-vault-stage-{suffix}"));
    next_intent.saved = root.join(format!(".cozy-project-source-{suffix}"));
    fs::create_dir(&next_intent.staged).unwrap();
    next_intent.staged_id = Some(directory_id(&next_intent.staged).unwrap());
    save_conversion(&next_intent).unwrap();
    let next_bytes = fs::read(receipt_path(&serial.root)).unwrap();
    drop(operation_guard);
    let contender = std::thread::spawn({ let path = serial.root.clone(); move || recover_conversion(&path) });
    contender.join().unwrap().unwrap();
    assert_eq!(fs::read(receipt_path(&serial.root)).unwrap(), next_bytes);
    drop(writer);
    recover_conversion(&serial.root).unwrap();
    assert!(!next_intent.staged.exists() && serial.root.join(KEYBAG_FILE).exists());

    // A read lease prevents recovery writes. Pending plaintext never becomes
    // an ordinary fallback, including when the receipt path is a directory.
    let mut read_held = staged(&root, "read-held", &created.keybag);
    read_held.ready = true; save_conversion(&read_held).unwrap();
    fs::rename(&read_held.root, &read_held.saved).unwrap();
    fs::rename(&read_held.staged, &read_held.root).unwrap();
    let reader = access(&read_held.root).read_owned().await;
    recover_conversion(&read_held.root).unwrap();
    assert!(receipt_path(&read_held.root).is_file());
    assert!(view(&read_held.root).is_err());
    let bytes = fs::read(receipt_path(&read_held.root)).unwrap();
    fs::remove_file(receipt_path(&read_held.root)).unwrap();
    fs::create_dir(receipt_path(&read_held.root)).unwrap();
    assert!(view(&read_held.root).is_err());
    fs::remove_dir(receipt_path(&read_held.root)).unwrap();
    fs::write(receipt_path(&read_held.root), bytes).unwrap();
    drop(reader);
    recover_conversion(&read_held.root).unwrap();
    assert_eq!(view(&read_held.root).unwrap(), read_held.root);

    // An installed encrypted transaction still awaits unlock's journal/source
    // cleanup. A new storage action cannot overwrite that durable intent.
    let mut unfinished = staged(&root, "unfinished-encrypted", &created.keybag);
    unfinished.plain = None; save_conversion(&unfinished).unwrap();
    let op = operation(&unfinished.root); let operation_guard = op.lock().unwrap();
    let custody = exclusive(&unfinished.root).unwrap();
    recover_under_guards(&unfinished.root).unwrap();
    let before = fs::read(receipt_path(&unfinished.root)).unwrap();
    for action in ["encrypt", "rotate", "decrypt"] {
        assert_eq!(require_transition_ready(&unfinished.root, action).unwrap_err().code, "vault_cleanup_pending");
        assert_eq!(fs::read(receipt_path(&unfinished.root)).unwrap(), before);
    }
    for action in ["lock", "activity", "passphrase", "recovery"] { require_transition_ready(&unfinished.root, action).unwrap(); }
    drop(custody); drop(operation_guard);

    // Rebinding an older pre-move pointer is reconstructed from the real index
    // and native ID in a FRESH process, not by keeping this test's static map.
    let mut pointer = PAYLOAD_PREFIX.to_vec();
    pointer.extend(serde_json::to_vec(&ProtectedPayload { project_id: id.clone(), root: root.join("before-move"), context: "retained-history".into(), object }).unwrap());
    fs::write(root.join("pointer"), &pointer).unwrap();
    fs::write(root.join("current-root"), ready.root.to_string_lossy().as_bytes()).unwrap();
    let controller = root.join("controller");
    let store = crate::store::Store::open(&controller.join("state.db")).unwrap();
    store.project_opened(&id, "Retained", &ready.root, None, None).unwrap();
    store.project_opened("missing", "Missing", &root.join("missing"), None, None).unwrap();
    drop(store);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", RESTART, "--nocapture"]).env("COZY_DECRYPT_FIXTURE", &root).output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "plaintext restart fixture");

    // A normal copied document receives its own ID. Its inherited old marker
    // cannot grant the original namespace, but its ordinary identity stays usable.
    let copy = root.join("foreign-copy"); fs::create_dir(&copy).unwrap();
    copy_tree_at(&ready.root, &copy, FsPath::new(""), false, false).unwrap();
    {
        let at = copy.join(db::DB); let mut conn = db::open(&at).unwrap();
        db::set_identity(&mut conn, &at, "new-copy-id", "Copy", Some("Copy1234")).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").unwrap();
    }
    KEY_ROOTS.lock().unwrap().insert(id.clone(), copy.clone());
    register_plain_root(&copy, "new-copy-id").unwrap();
    assert_ne!(KEY_ROOTS.lock().unwrap().get(&id), Some(&copy));
    assert!(crate::private_io::retained_scope(&copy, "new-copy-id").unwrap().is_none());
    assert!(crate::private_io::retained_view(&copy, &id).is_err());
    assert_eq!(plain_project_id(&copy).as_deref(), Some("new-copy-id"));
    let copied_db = hash(&copy.join(db::DB)).unwrap();
    assert_eq!(plain_generation_for_encrypt(&copy, "new-copy-id").unwrap(), 1);
    assert_eq!(hash(&copy.join(db::DB)).unwrap(), copied_db);
    assert!(!copy.join(KEYBAG_FILE).exists());
    let fresh_stage = root.join("fresh-copy-stage"); fs::create_dir(&fresh_stage).unwrap();
    copy_tree(&copy, &fresh_stage).unwrap();
    assert_eq!(plain_project_id(&fresh_stage).as_deref(), Some("new-copy-id"));
    assert_eq!(fs::read(fresh_stage.join(".vault-machine/objects/source")).unwrap(), b"original immutable source");
    assert_eq!(fs::read_dir(fresh_stage.join(JOURNAL_DIR)).unwrap().count(), 1);
    assert!(!fresh_stage.join(api::machine::LOCAL_PRIVATE_MARKER).exists());
    assert!(!fresh_stage.join(".vault-machine/generations").exists());
    let fresh_scope = crate::private_io::descriptor_for_mounted(&fresh_stage, "new-copy-id", 1).unwrap();
    assert_eq!(fresh_scope.storage.project_id, "new-copy-id");
    assert_eq!(fresh_scope.storage.generation, 1);
    assert_ne!(KEY_ROOTS.lock().unwrap().get(&id), Some(&fresh_stage));
    let marker_path = copy.join(api::machine::LOCAL_PRIVATE_MARKER);
    let marker_bytes = fs::read(&marker_path).unwrap(); fs::write(&marker_path, b"{}").unwrap();
    assert!(plain_generation_for_encrypt(&copy, "new-copy-id").is_err());
    assert_eq!(fs::read(&marker_path).unwrap(), b"{}");
    assert_eq!(hash(&copy.join(db::DB)).unwrap(), copied_db);
    fs::write(&marker_path, marker_bytes).unwrap();

    // The original list predicate is demonstrably false between the renames.
    // The real indexed/named functions must retain the ID throughout that gap,
    // and the actual GET handler must remain Pending while the writer owns ACCESS.
    let home = platform::paths::Home::under(&root.join("api-home")); home.open().unwrap();
    fs::create_dir_all(home.calcifer()).unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let config = api::config::Config { hub_url: api::config::Origin::parse("http://127.0.0.1:9").unwrap(), ..Default::default() };
    let machines = crate::machines::Machines::start(calcifer.clone(), home, &root, &config);
    let api = crate::api::Api::new(calcifer, machines, "native-decrypt-fixture");
    api.calcifer.store().project_opened(&id, "Gap", &copying.root, None, None).unwrap();
    let writer = exclusive(&copying.root).unwrap();
    CONVERTING.lock().unwrap().insert(copying.root.clone());
    let converting = Converting(copying.root.clone());
    let gap = root.join("gap-encrypted-source"); fs::rename(&copying.root, &gap).unwrap();
    assert!(!super::super::Folder::is_project(&copying.root) && !encrypted(&copying.root));
    assert!(super::super::indexed(&api).unwrap().iter().any(|project| project.id == id));
    assert_eq!(super::super::named(&api, &id).unwrap(), copying.root);
    let mut reading = Box::pin(read(State(api.clone()), Path(id.clone())));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::future::Future::poll(reading.as_mut(), &mut context).is_pending(), "GET answered a transient plaintext status during the rename gap");
    fs::rename(&gap, &copying.root).unwrap();
    drop(converting); drop(writer);
    let response = reading.await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let observed: VaultStatus = serde_json::from_slice(&bytes).unwrap();
    assert!(observed.encrypted && observed.state == "locked");
    api.down().await.unwrap();

    #[cfg(windows)]
    for directory in [&ready.root, &next, &copy, &fresh_stage] {
        let path = directory.join("readonly.txt");
        let mut permissions = fs::metadata(&path).unwrap().permissions(); permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
    }
    println!("{}", serde_json::json!({"copying_rollback":true,"ready_rename_recovery":true,"installed_recovery":true,
        "changed_file_ids_refused":true,"escaped_path_preserved":true,"readonly_bytes_and_permissions":true,
        "retained_source_preserved_caches_rebuilt":true,"foreign_copy_marker_denied":true,
        "foreign_copy_fresh_staging_preserves_retained_bytes":true,"malformed_marker_refused_without_mutation":true,
        "partial_cipher_cleanup_retry":true,"encryption_finisher_preserves_plain_intent":true,
        "contended_recovery_skips_without_old_snapshot":true,"replacement_receipt_survives_contention":true,
        "own_read_pending_plain_refused":true,"non_file_receipt_fails_closed":true,"unfinished_encrypted_intent_preserved":true,
        "original_gap_predicate_false":true,"index_and_named_survive_rename_gap":true,"status_waits_for_transition":true}));
}

#[test]
fn native_plain_journal_restart_fixture() {
    let Some(root) = std::env::var_os("COZY_DECRYPT_FIXTURE").map(PathBuf::from) else { return };
    let pointer = fs::read(root.join("pointer")).unwrap();
    assert!(open_payload("retained-history", &pointer).unwrap().is_none());
    let controller = root.join("controller");
    let calcifer = crate::Calcifer::open(&controller, &root.join("outputs")).unwrap();
    assert_eq!(open_payload("retained-history", &pointer).unwrap().unwrap(), b"old journal bytes");
    assert!(open_payload("other-context", &pointer).is_err());
    let current = PathBuf::from(fs::read_to_string(root.join("current-root")).unwrap());
    assert_eq!(payload_root(&pointer).unwrap(), Some(current));
    drop(calcifer);
    println!("{}", serde_json::json!({"fresh_process_index_rehydrates_moved_plain_journal":true,"journal_context_preserved":true,"stale_index_skipped":true}));
}
