use super::*;
use super::super::written::Boxed;
use automerge::transaction::Transactable;

fn current(path: &Path) -> Connection {
    let mut written = Written::blank();
    written.boxes.push(Boxed { name: "art".into(), fields: vec![("instructions".into(), serde_json::json!("initial"))], ..Default::default() });
    create(path, &Stored { written: Arc::new(written), ..Default::default() }, &[], &[]).unwrap();
    let mut conn = open(path).unwrap();
    set_identity(&mut conn, path, "project-current", "Current", Some("12345678A")).unwrap();
    conn
}

fn changed(before: &Stored, words: &str) -> Stored {
    let mut after = before.clone();
    Arc::make_mut(&mut after.written).boxes[0].fields[0].1 = serde_json::json!(words);
    after
}

#[test]
fn open_never_creates_and_create_never_replaces_existing_paths() {
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join(DB);
    assert!(open(&absent).is_err());
    assert!(!absent.exists());
    let empty = temp.path().join("project.db.new");
    std::fs::write(&empty, []).unwrap();
    assert_eq!(open(&empty).err().unwrap().code, "project_db_version");
    assert_eq!(create(&empty, &Stored::default(), &[], &[]).unwrap_err().code, "project_db_exists");
    assert_eq!(std::fs::metadata(empty).unwrap().len(), 0);
}

#[test]
fn unsupported_schema_refuses_before_writable_setup_or_backup() {
    for version in [0, 1, 3] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES('id','preserve-me');").unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        assert_eq!(open(&path).err().unwrap().code, "project_db_version");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!temp.path().join("project.db.v1").exists());
        assert!(!temp.path().join("project.db.new").exists());
        assert!(!temp.path().join("project.db-wal").exists());
    }
}

#[test]
fn unsupported_native_format_preserves_existing_backup_and_stage() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DB);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE crdt_document(id INTEGER PRIMARY KEY,format INTEGER,snapshot BLOB,heads TEXT); INSERT INTO crdt_document VALUES(1,999,X'00','[]'); PRAGMA user_version=2;").unwrap();
    drop(conn);
    let backup = temp.path().join("project.db.v1");
    let stage = temp.path().join("project.db.new");
    std::fs::write(&backup, b"owner's existing backup").unwrap();
    std::fs::write(&stage, b"owner's existing stage").unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(open(&path).err().unwrap().code, "project_db_version");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(backup).unwrap(), b"owner's existing backup");
    assert_eq!(std::fs::read(stage).unwrap(), b"owner's existing stage");
    assert!(!temp.path().join("project.db-wal").exists());
}

#[test]
fn unsupported_committed_wal_version_does_not_change_database_or_wal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DB);
    let writer = current(&path);
    writer.pragma_update(None, "user_version", 1).unwrap();
    let wal = temp.path().join("project.db-wal");
    let before = (std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap());
    assert_eq!(open(&path).err().unwrap().code, "project_db_version");
    assert_eq!((std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap()), before);
    assert!(!temp.path().join("project.db.v1").exists());
}

#[test]
fn current_native_append_and_committed_wal_reload_remain_authoritative() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DB);
    let mut writer = current(&path);
    let before = load(&writer, &path).unwrap();
    let old: Vec<u8> = writer.query_row("SELECT snapshot FROM crdt_document WHERE id=1", [], |row| row.get(0)).unwrap();
    let mut after = changed(&before, "current native words");
    assert!(commit(&mut writer, &path, &before, &mut after, &Extra::default(), || (), || ()).unwrap().is_some());
    let appended: Vec<u8> = writer.query_row("SELECT snapshot FROM crdt_document WHERE id=1", [], |row| row.get(0)).unwrap();
    assert!(appended.len() > old.len() && appended.starts_with(&old));
    writer.execute("UPDATE fields SET value='\"projection only\"'", []).unwrap();
    let reader = open(&path).unwrap();
    let loaded = load(&reader, &path).unwrap();
    assert_eq!(loaded.written.boxes[0].fields[0].1, serde_json::json!("current native words"));
    assert_eq!(identity(&reader), Some(("project-current".into(), "Current".into())));
    assert_eq!(reader.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), VERSION);
}

#[test]
fn stale_native_commit_rolls_back_projection_and_canonical_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DB);
    let mut first = current(&path);
    let mut second = open(&path).unwrap();
    let before = load(&first, &path).unwrap();
    let stale = load(&second, &path).unwrap();
    let mut winning = changed(&before, "winner");
    commit(&mut first, &path, &before, &mut winning, &Extra::default(), || (), || ()).unwrap();
    let mut losing = changed(&stale, "stale");
    assert_eq!(commit(&mut second, &path, &stale, &mut losing, &Extra::default(), || (), || ()).unwrap_err().code, "project_changed");
    assert_eq!(load(&second, &path).unwrap().written.boxes[0].fields[0].1, serde_json::json!("winner"));
    let projection: String = second.query_row("SELECT value FROM fields WHERE field='instructions'", [], |row| row.get(0)).unwrap();
    assert_eq!(projection, "\"winner\"");
}

#[test]
fn native_corruption_and_missing_identity_never_fall_back_to_sql() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DB);
    let conn = current(&path);
    conn.execute("UPDATE crdt_document SET snapshot=X'00' WHERE id=1", []).unwrap();
    assert!(load(&conn, &path).is_err());
    assert!(identity(&conn).is_none());
    let fresh = temp.path().join("unidentified.db");
    create(&fresh, &Stored::default(), &[], &[]).unwrap();
    let fresh_conn = open(&fresh).unwrap();
    assert!(identity(&fresh_conn).is_some(), "fresh initialization supplies a native identity");
    let mut native = read_document(&fresh_conn, &fresh).unwrap();
    native.doc.delete(automerge::ROOT, "identity").unwrap();
    let tx = fresh_conn.unchecked_transaction().unwrap();
    save_document(&tx, &fresh, &mut native, 0, None).unwrap();
    tx.commit().unwrap();
    set_meta(&fresh_conn, &fresh, "id", Some("sql-only")).unwrap();
    assert!(load(&fresh_conn, &fresh).is_ok(), "the native stream and its recorded heads still match");
    assert!(identity(&fresh_conn).is_none());
}
