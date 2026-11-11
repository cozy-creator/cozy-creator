//! Real SQLite selection/transaction checks with a deterministic payload transform.
//! Native vault lifecycle tests separately exercise the actual encrypted object writer.
use super::*;

fn database() -> (tempfile::TempDir, Store, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("selected");
    std::fs::create_dir(&root).unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    (dir, store, root)
}

fn insert_run(store: &Store, number: i64, out: &Path, spec: &[u8]) {
    let id = format!("run-{number}");
    store.write(|tx| {
        tx.execute(
            "INSERT INTO runs (id,number,idempotency_key,digest,hub,kind,target,pinned,spec,out_dir,state,status,created_at)
             VALUES (?1,?2,?1,'','','call','fixture/target',0,?3,?4,'settled','completed',0)",
            params![id, number, spec, out.to_string_lossy()],
        )?;
        Ok(())
    }).unwrap();
}

fn insert_answer(store: &Store, key: &str, body: &[u8]) {
    store.write(|tx| {
        tx.execute("INSERT INTO answers (key,digest,status,content_type,body,at_ms) VALUES (?1,'',200,'application/json',?2,0)", params![key, body])?;
        Ok(())
    }).unwrap();
}

fn pointer(root: &Path) -> Vec<u8> {
    let mut value = vault::PAYLOAD_PREFIX.to_vec();
    value.extend(serde_json::to_vec(&serde_json::json!({
        "project_id":"fixture-private", "root":root, "context":"fixture",
        "object":"00000000000000000000000000000000"
    })).unwrap());
    value
}

fn rewritten(value: &[u8]) -> Vec<u8> {
    [b"rewritten:".as_slice(), value].concat()
}

fn run_spec(store: &Store, id: &str) -> Vec<u8> {
    store.read(|conn| Ok(conn.query_row("SELECT spec FROM runs WHERE id=?1", [id], |row| row.get(0))?)).unwrap()
}

fn answer_body(store: &Store, key: &str) -> Vec<u8> {
    store.read(|conn| Ok(conn.query_row("SELECT body FROM answers WHERE key=?1", [key], |row| row.get(0))?)).unwrap()
}

#[test]
fn journal_scope_keeps_large_unrelated_values_and_stale_path_envelopes() {
    let (dir, store, root) = database();
    // A string-prefix path comparison would accidentally include this sibling.
    let other = dir.path().join("selected-sibling");
    let large = vec![b'x'; 1024 * 1024];
    let plain = br#"{"fixture":"selected"}"#;
    let moved = pointer(&root);
    let foreign = pointer(&other);
    let mut lookalike = vault::PAYLOAD_PREFIX[..vault::PAYLOAD_PREFIX.len() - 1].to_vec();
    lookalike.extend_from_slice(b"not-an-envelope");
    insert_run(&store, 30, &other, &large);
    insert_run(&store, 20, &root.join(".runs"), plain);
    insert_run(&store, 10, &dir.path().join("old-location"), &moved);
    insert_run(&store, 40, &other, &foreign);
    insert_run(&store, 50, &other, &lookalike);
    store.write(|tx| {
        tx.execute("UPDATE runs SET outcome=?1,warnings='warning',inputs='input',reason_code='code',reason='reason' WHERE id='run-20'", [b"outcome".as_slice()])?;
        tx.execute("INSERT INTO outputs (run,name,idx,rev,length,digest,media_type,label,path,checkpoint,export_error,exported_rev,failed_rev,created_seq,seq)
                    VALUES ('run-20','clip',2,7,0,'','text/plain','label','path','checkpoint','export-error',5,6,1,2)", [])?;
        tx.execute("INSERT INTO run_logs (run,seq,at_ms,level,text) VALUES ('run-20',2,0,'info','second'),('run-20',1,0,'info','first')", [])?;
        tx.execute("INSERT INTO asks (key,hub,digest,body,state,reason,created_at,run) VALUES ('fixture-ask','','',?1,'done','ask-reason',0,'run-20')", [b"ask-body".as_slice()])?;
        Ok(())
    }).unwrap();
    // Insert answers in the opposite order from their key index.
    let answer = serde_json::to_vec(&serde_json::json!({"path":root.join("child")})).unwrap();
    let unrelated_answer = serde_json::to_vec(&serde_json::json!({"path":other,"padding":"y".repeat(1024 * 1024)})).unwrap();
    insert_answer(&store, "z-selected", &answer);
    insert_answer(&store, "a-protected", &moved);
    insert_answer(&store, "middle-unrelated", &unrelated_answer);
    let mut calls = Vec::new();
    assert!(store.rewrite_project_journal(&root, |context, value, protected| {
        calls.push((context.to_owned(), protected));
        Ok(rewritten(value))
    }).unwrap());
    let expected = [
        ("run:run-20:spec", false), ("run:run-20:outcome", false),
        ("run:run-20:target", false), ("run:run-20:warnings", false),
        ("run:run-20:inputs", false), ("run:run-20:reason_code", false), ("run:run-20:reason", false),
        (r#"["output","run-20","clip",2,7,"label"]"#, false),
        (r#"["output","run-20","clip",2,7,"checkpoint"]"#, false),
        (r#"["output","run-20","clip",2,5,"path"]"#, false),
        (r#"["output","run-20","clip",2,6,"export_error"]"#, false),
        ("run:run-20:log:1", false), ("run:run-20:log:2", false),
        ("ask:fixture-ask:body", false), ("ask:fixture-ask:reason", false),
        ("run:run-10:spec", true), ("run:run-10:target", false),
        ("run:run-10:warnings", true), ("run:run-10:inputs", true),
        ("answer:z-selected:body", false), ("answer:a-protected:body", true),
    ];
    assert_eq!(calls, expected.map(|(context, protected)| (context.to_owned(), protected)));
    assert_eq!(run_spec(&store, "run-20"), rewritten(plain));
    assert_eq!(run_spec(&store, "run-10"), rewritten(&moved));
    assert_eq!(run_spec(&store, "run-30"), large);
    assert_eq!(run_spec(&store, "run-40"), foreign);
    assert_eq!(run_spec(&store, "run-50"), lookalike);
    assert_eq!(answer_body(&store, "z-selected"), rewritten(&answer));
    assert_eq!(answer_body(&store, "a-protected"), rewritten(&moved));
    assert_eq!(answer_body(&store, "middle-unrelated"), unrelated_answer);
    let moved_out: String = store.read(|conn| Ok(conn.query_row("SELECT out_dir FROM runs WHERE id='run-10'", [], |row| row.get(0))?)).unwrap();
    assert_eq!(PathBuf::from(moved_out), crate::project::landing(&root));
    assert!(root.join("project.vault.journal-pending").is_file());
}

#[test]
fn journal_unrelated_malformed_protected_spec_still_aborts_atomically() {
    for malformed in [
        [vault::PAYLOAD_PREFIX, b"not-json"].concat(),
        [vault::PAYLOAD_PREFIX, br#"{"project_id":"x","root":"/other","context":"x","object":"bad"}"#].concat(),
    ] {
        let (dir, store, root) = database();
        let original = b"selected spec";
        insert_run(&store, 20, &root, original);
        insert_run(&store, 10, &dir.path().join("unrelated"), &malformed);
        let mut transformed = false;
        let error = store.rewrite_project_journal(&root, |_, value, _| {
            transformed = true;
            Ok(rewritten(value))
        }).unwrap_err();
        assert!(matches!(error, StoreError::Protected(_)));
        assert!(transformed, "the earlier selected row must be visited before the malformed row");
        assert_eq!(run_spec(&store, "run-20"), original);
        assert_eq!(run_spec(&store, "run-10"), malformed);
        assert!(!root.join("project.vault.journal-pending").exists());
    }
}

#[test]
fn journal_answer_order_and_malformed_envelopes_keep_rollback() {
    let (_dir, store, root) = database();
    let body = serde_json::to_vec(&serde_json::json!({"path":root})).unwrap();
    insert_answer(&store, "z-selected", &body);
    insert_answer(&store, "a-malformed", &[vault::PAYLOAD_PREFIX, b"broken"].concat());
    let mut calls = Vec::new();
    let error = store.rewrite_project_journal(&root, |context, value, _| {
        calls.push(context.to_owned());
        Ok(rewritten(value))
    }).unwrap_err();
    assert!(matches!(error, StoreError::Protected(_)));
    assert_eq!(calls, ["answer:z-selected:body"]);
    assert_eq!(answer_body(&store, "z-selected"), body);
    assert!(!root.join("project.vault.journal-pending").exists());
}
