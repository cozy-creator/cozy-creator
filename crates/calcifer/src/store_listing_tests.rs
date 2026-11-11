use super::*;

fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    (dir, store)
}

fn insert(store: &Store, number: i64, target: &str, spec: &[u8], out: &Path) {
    let id = format!("run-{number}");
    store.write(|tx| {
        tx.execute(
            "INSERT INTO runs (id,number,idempotency_key,digest,hub,kind,target,pinned,spec,out_dir,state,status,created_at)
             VALUES (?1,?2,?1,'','','call',?3,0,?4,?5,'settled','completed',0)",
            params![id, number, target, spec, out.to_string_lossy()],
        )?;
        Ok(())
    }).unwrap();
}

// A valid stored pointer to a locked journal: no mounted vault or secret is needed to
// prove that a list cannot reveal its target or legacy plaintext prefix.
fn locked(number: i64, field: &str, root: &Path) -> Vec<u8> {
    let mut bytes = vault::PAYLOAD_PREFIX.to_vec();
    bytes.extend(serde_json::to_vec(&serde_json::json!({
        "project_id": format!("private-{number}"),
        "root": root,
        "context": format!("run:run-{number}:{field}"),
        "object": "00000000000000000000000000000000",
    })).unwrap());
    bytes
}

fn predicate() -> String {
    let prefix = hex::encode(vault::PAYLOAD_PREFIX);
    let length = vault::PAYLOAD_PREFIX.len();
    format!("(substr(CAST(target AS BLOB),1,{length})=X'{prefix}' OR substr(spec,1,{length})=X'{prefix}')")
}

fn plan(store: &Store, sql: &str, values: &[&dyn rusqlite::ToSql]) -> Vec<String> {
    store.read(|conn| {
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        Ok(statement.query_map(values, |row| row.get(3))?.collect::<rusqlite::Result<_>>()?)
    }).unwrap()
}

#[test]
fn list_queries_use_private_and_covering_indexes_after_reopen() {
    let (dir, store) = database();
    insert(&store, 1, "public/run", b"{}", dir.path());
    // Existing stores acquire the additive indexes too, without a schema-version change.
    store.write(|tx| {
        tx.execute_batch("DROP INDEX IF EXISTS runs_private_targets; DROP INDEX IF EXISTS runs_list_metadata;")?;
        Ok(())
    }).unwrap();
    drop(store);
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let private = format!("SELECT EXISTS(SELECT 1 {RUN_LIST_SCOPE} AND {})", predicate());
    let none: Option<&str> = None;
    let params: &[&dyn rusqlite::ToSql] = &[&i64::MAX, &none, &none, &none];
    assert!(plan(&store, &private, params).iter().any(|line| line.contains("runs_private_targets")));
    let count = "SELECT COUNT(*) FROM runs WHERE number < ?1
        AND target NOT LIKE '%/project-adapter-%'
        AND (?2 IS NULL OR CASE state WHEN 'accepted' THEN 'in_progress' WHEN 'settled' THEN status ELSE 'queued' END = ?2)
        AND (?3 IS NULL OR substr(target,1,length(?3))=?3)
        AND (?4 IS NULL OR machine=?4) AND (?5 IS NULL OR out_dir=?5)";
    let params: &[&dyn rusqlite::ToSql] = &[&i64::MAX, &none, &none, &none, &none];
    assert!(plan(&store, count, params).iter().any(|line| line.contains("COVERING INDEX runs_list_metadata")));
    // The public half of a mixed list must read the indexed predicate, not load spec blobs.
    let public = format!("SELECT COUNT(*) {RUN_LIST_SCOPE} AND {} = 0 AND target NOT LIKE '%/project-adapter-%'", predicate());
    store.read(|conn| {
        let root: i64 = conn.query_row("SELECT rootpage FROM sqlite_schema WHERE name='runs'", [], |row| row.get(0))?;
        let mut statement = conn.prepare(&format!("EXPLAIN {public}"))?;
        let ops: Vec<(String, i64, i64)> = statement.query_map(&params[..4], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        let table: Vec<i64> = ops.iter().filter(|(op, _, page)| op == "OpenRead" && *page == root).map(|(_, cursor, _)| *cursor).collect();
        assert!(!ops.iter().any(|(op, cursor, _)| op == "Column" && table.contains(cursor)), "spec read from the runs table: {ops:?}");
        Ok(())
    }).unwrap();
}

#[test]
fn empty_specs_and_unreadable_rows_keep_exact_older_counts() {
    let (dir, store) = database();
    insert(&store, 1, "public/old", b"{}", dir.path());
    insert(&store, 2, "public/legacy", b"", dir.path());
    insert(&store, 3, "public/broken", b"{", dir.path());
    insert(&store, 4, "public/project-adapter-hidden", b"{}", dir.path());
    let (rows, older, unreadable) = store.runs(&RunFilter { limit: 1, ..Default::default() }).unwrap();
    assert!(rows.is_empty());
    assert_eq!(older, 2);
    assert_eq!(unreadable.len(), 1);
    assert!(unreadable[0].starts_with("run 3:"));
    let (rows, older, unreadable) = store.runs(&RunFilter { before: Some(3), limit: 10, ..Default::default() }).unwrap();
    assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(older, 0);
    assert!(unreadable.is_empty());
}

#[test]
fn locked_targets_and_legacy_specs_keep_scope_and_prefix_privacy() {
    let (dir, store) = database();
    let public = dir.path().join("public/.runs");
    let private_root = dir.path().join("private");
    let private = private_root.join(".runs");
    insert(&store, 1, "public/old", b"{}", &public);
    insert(&store, 2, "public/legacy", b"", &public);
    insert(&store, 3, "public/broken", b"{", &public);
    insert(&store, 4, "public/project-adapter-hidden", b"{}", &public);
    let target = String::from_utf8(locked(5, "target", &private_root)).unwrap();
    insert(&store, 5, &target, &locked(5, "spec", &private_root), &private);
    insert(&store, 6, "secret/legacy", &locked(6, "spec", &private_root), &private);
    let (rows, older, unreadable) = store.runs(&RunFilter { limit: 2, ..Default::default() }).unwrap();
    assert!(rows.is_empty());
    assert_eq!(older, 2);
    assert_eq!(unreadable.len(), 2);
    assert!(unreadable[0].starts_with("run 6:"));
    assert!(unreadable[1].starts_with("run 5:"));
    let (rows, older, unreadable) = store.runs(&RunFilter { target: Some("secret/".into()), limit: 10, ..Default::default() }).unwrap();
    assert!(rows.is_empty() && unreadable.is_empty());
    assert_eq!(older, 0);
    let (rows, older, unreadable) = store.runs(&RunFilter { out_dir: Some(public.to_string_lossy().into_owned()), limit: 10, ..Default::default() }).unwrap();
    assert_eq!(rows.iter().map(|row| row.number).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(older, 0);
    assert_eq!(unreadable.len(), 1);
}
