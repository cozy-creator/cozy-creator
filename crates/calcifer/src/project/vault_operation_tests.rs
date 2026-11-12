//! These checks run on cloud runners; they never inspect a user's project or credentials.
use super::*;

fn ask(secret: &str) -> VaultAsk {
    VaultAsk { action: "encrypt".into(), passphrase: Some(secret.into()), recovery_secret: None,
        idle_timeout_s: Some(900), max_duration_s: None }
}

#[test]
fn retry_binding_excludes_credentials_but_binds_project_and_options() {
    assert_eq!(binding("/project", &ask("one")), binding("/project", &ask("another")));
    assert_ne!(binding("/project", &ask("one")), binding("/different", &ask("one")));
    let mut other = ask("one");
    other.idle_timeout_s = Some(0);
    assert_ne!(binding("/project", &ask("one")), binding("/project", &other));
}

#[test]
fn revoked_result_cannot_reveal_recovery_or_old_view() {
    let (events, _) = watch::channel(VaultEvent::Accepted { action: "encrypt".into() });
    let token = Arc::new(AtomicBool::new(true));
    let operation = Operation { named: "/project".into(), action: "encrypt".into(), root: Mutex::new(None), binding: String::new(), events,
        finished: Mutex::new(None), readable: Mutex::new(Some(token.clone())), available: AtomicBool::new(true), locked: AtomicBool::new(false) };
    let mut event = VaultEvent::Completed { changed: VaultChanged {
        vault: api::calcifer::project::VaultStatus { encrypted: true, state: "unlocked".into(), path: "/project".into(),
            view: Some("/readable".into()), expires_at_ms: None, idle_timeout_s: 900, next: vec![] },
        recovery_secret: Some("synthetic recovery".into()), ended: vec![], skipped_files: vec![],
    }};
    token.store(false, Ordering::Release);
    let mut locked = event.clone();
    sanitize(&mut locked, &operation);
    assert!(matches!(locked, VaultEvent::Failed { error } if error.code == "project_locked"));

    // A reader that already cloned a reply must also honor acknowledgment/expiry,
    // even while the project remains unlocked and its mount token stays valid.
    token.store(true, Ordering::Release);
    operation.events.send_replace(event.clone());
    retire(&operation);
    sanitize(&mut event, &operation);
    assert!(matches!(event, VaultEvent::Failed { error } if error.code == "vault_operation_result_expired"));
    assert!(matches!(&*operation.events.borrow(), VaultEvent::Failed { error } if error.code == "vault_operation_result_expired"));
}

#[test]
fn a_busy_transition_keeps_its_error_code_without_private_details() {
    let (events, _) = watch::channel(VaultEvent::Accepted { action: "decrypt".into() });
    let operation = Operation { named: "/project".into(), action: "decrypt".into(), root: Mutex::new(None), binding: String::new(), events,
        finished: Mutex::new(None), readable: Mutex::new(None), available: AtomicBool::new(true), locked: AtomicBool::new(true) };
    let mut error = Error::new(Kind::Conflict, "project_busy", "private-file.mp4 is held open");
    error.remedy = Some("close private-file.mp4".into());
    error.metadata = Some(serde_json::json!({"file":"private-file.mp4"}));
    let mut event = VaultEvent::Failed { error };
    sanitize(&mut event, &operation);
    assert!(!serde_json::to_string(&event).unwrap().contains("private-file.mp4"));
    assert!(matches!(event, VaultEvent::Failed { error } if error.kind == Kind::Conflict && error.code == "project_busy"));
}
