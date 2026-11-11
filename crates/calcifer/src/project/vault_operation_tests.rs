//! These checks run on cloud runners; they never inspect a user's project or credentials.
use super::*;

fn ask(secret: &str) -> VaultAsk {
    VaultAsk { action: "encrypt".into(), passphrase: Some(secret.into()), recovery_secret: None,
        idle_timeout_s: Some(900), max_duration_s: None, remove_plaintext: false }
}

#[test]
fn retry_binding_excludes_credentials_but_binds_project_and_options() {
    assert_eq!(binding("/project", &ask("one")), binding("/project", &ask("another")));
    assert_ne!(binding("/project", &ask("one")), binding("/different", &ask("one")));
    let mut other = ask("one");
    other.remove_plaintext = true;
    assert_ne!(binding("/project", &ask("one")), binding("/project", &other));
}

#[test]
fn revoked_result_cannot_reveal_recovery_or_old_view() {
    let (events, _) = watch::channel(VaultEvent::Accepted { action: "encrypt".into() });
    let token = Arc::new(AtomicBool::new(true));
    let operation = Operation { named: "/project".into(), root: Mutex::new(None), binding: String::new(), events,
        finished: Mutex::new(None), readable: Mutex::new(Some(token.clone())), available: AtomicBool::new(true) };
    let mut event = VaultEvent::Completed { changed: VaultChanged {
        vault: api::calcifer::project::VaultStatus { encrypted: true, state: "unlocked".into(), path: "/project".into(),
            view: Some("/readable".into()), expires_at_ms: None, idle_timeout_s: 900, next: vec![] },
        recovery_secret: Some("synthetic recovery".into()), preserved_source: None, ended: vec![], skipped_files: vec![],
    }};
    token.store(false, Ordering::Release);
    sanitize(&mut event, &operation);
    let VaultEvent::Completed { mut changed } = event else { panic!("expected completion") };
    assert!(changed.recovery_secret.is_none() && changed.vault.view.is_none());
    assert_eq!(changed.vault.state, "locked");

    // A reader that already cloned a reply must also honor acknowledgment/expiry,
    // even while the project remains unlocked and its mount token stays valid.
    token.store(true, Ordering::Release);
    changed.recovery_secret = Some("synthetic held recovery".into());
    let mut copied = VaultEvent::Completed { changed };
    operation.events.send_replace(copied.clone());
    retire(&operation);
    sanitize(&mut copied, &operation);
    assert!(matches!(copied, VaultEvent::Failed { error } if error.code == "vault_operation_result_expired"));
    assert!(matches!(&*operation.events.borrow(), VaultEvent::Failed { error } if error.code == "vault_operation_result_expired"));
}
