//! A vault mutation is received once and observed over a reconnectable private SSE stream.
//! The durable receipt holds only an opaque ID, a binding digest and state. Credentials,
//! filenames, progress and recovery secrets never enter that receipt or the global stream.
use std::{collections::HashMap, convert::Infallible, path::PathBuf, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultEvent}, error::{Error, Kind}};
use axum::{Extension, body::Bytes, extract::{Path, State, rejection::BytesRejection}, http::{HeaderValue, StatusCode, header}, response::{IntoResponse, Response, sse::{Event, KeepAlive, Sse}}};
use futures_util::StreamExt as _;
use rusqlite::OptionalExtension as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use crate::api::{Api, Refusal, parse};

#[cfg(test)]
#[path = "vault_operation_tests.rs"]
mod tests;

pub(crate) const TABLES: &str = "CREATE TABLE IF NOT EXISTS vault_operations (
    id TEXT PRIMARY KEY, binding TEXT NOT NULL, state TEXT NOT NULL
) STRICT;";

// A client acknowledges its terminal reply. A lost client cannot retain a recovery
// secret indefinitely; normal vault locking also removes it immediately.
const RESULT_LIFETIME: Duration = Duration::from_secs(15 * 60);
static OPERATIONS: LazyLock<Mutex<HashMap<String, Arc<Operation>>>> = LazyLock::new(Mutex::default);

struct Operation {
    named: String,
    action: String,
    root: Mutex<Option<PathBuf>>,
    binding: String,
    events: watch::Sender<VaultEvent>,
    finished: Mutex<Option<Instant>>,
    readable: Mutex<Option<Arc<AtomicBool>>>,
    available: AtomicBool,
    locked: AtomicBool,
}

fn terminal(event: &VaultEvent) -> bool {
    matches!(event, VaultEvent::Completed { .. } | VaultEvent::Failed { .. })
}

fn private(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn valid_id(id: &str) -> bool {
    (16..=128).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn binding(named: &str, ask: &VaultAsk) -> String {
    // Do not hash a low-entropy passphrase into a durable offline guessing target.
    let input = serde_json::to_vec(&(named, &ask.action, ask.idle_timeout_s, ask.max_duration_s, ask.remove_plaintext))
        .expect("vault operation metadata serializes");
    hex::encode(Sha256::digest(input))
}

/// Drop completed replies after acknowledgment, expiry or lock; durable admission
/// tombstones remain so a delayed retry can never perform the mutation twice.
pub(super) fn expire() {
    OPERATIONS.lock().expect("vault operations").retain(|_, operation| {
        let keep = operation.finished.lock().expect("vault result age").is_none_or(|at| at.elapsed() < RESULT_LIFETIME);
        if !keep { retire(operation); }
        keep
    });
}

fn expired_result() -> VaultEvent {
    VaultEvent::Failed { error: Error::new(Kind::Conflict, "vault_operation_result_expired",
        "this private operation reply is no longer available; check the vault status before starting another operation")
        .next("cozy vault status") }
}

fn retire(operation: &Operation) {
    operation.available.store(false, Ordering::Release);
    // Attached readers can outlive the registry entry. Remove the stored secret
    // and revoke their emission authority before dropping that entry.
    operation.events.send_replace(expired_result());
}

pub(super) fn revoke(root: &std::path::Path) {
    for operation in OPERATIONS.lock().expect("vault operations").values().filter(|operation| operation.root.lock().expect("vault operation root").as_deref() == Some(root)) {
        operation.locked.store(true, Ordering::Release);
        let completed = terminal(&operation.events.borrow());
        if completed { retire(operation); }
    }
}

pub(super) async fn acknowledge(Path((named, id)): Path<(String, String)>) -> Result<Response, Refusal> {
    let mut operations = OPERATIONS.lock().expect("vault operations");
    if let Some(operation) = operations.get(&id) {
        if operation.named != named { return Err(Refusal(Error::new(Kind::Conflict, "vault_operation_conflict", "the operation belongs to another project"))); }
        if operation.finished.lock().expect("vault result age").is_some() {
            retire(operation);
            operations.remove(&id);
        }
    }
    Ok(private(StatusCode::NO_CONTENT.into_response()))
}

pub(super) async fn write(State(api): State<Arc<Api>>, Path((named, id)): Path<(String, String)>, Extension(lease): Extension<crate::api::MutationLease>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    if !valid_id(&id) { return Err(Refusal(Error::new(Kind::Usage, "vault_operation_id", "use a unique operation ID of 16 to 128 letters, numbers, dashes or underscores"))); }
    let ask: VaultAsk = parse(&body?)?;
    if !matches!(ask.action.as_str(), "encrypt" | "unlock" | "lock" | "decrypt" | "passphrase" | "recovery" | "rotate") {
        return Err(Refusal(Error::new(Kind::Usage, "vault_action", "unknown local vault action")));
    }
    // Path resolution and even admission fsync can block on storage. Acknowledge
    // receipt first; the worker records admission durably before any mutation.
    let binding = binding(&named, &ask);
    expire();
    let accepted = VaultEvent::Accepted { action: ask.action.clone() };
    let (operation, fresh) = {
        let mut operations = OPERATIONS.lock().expect("vault operations");
        match operations.get(&id) {
            Some(operation) if operation.binding != binding => return Err(Refusal(Error::new(Kind::Conflict, "vault_operation_conflict", "this operation ID already names another request"))),
            Some(operation) => (operation.clone(), false),
            None => {
                let (events, _) = watch::channel(accepted.clone());
                let operation = Arc::new(Operation { named, action: ask.action.clone(), root: Mutex::new(None), binding, events, finished: Mutex::new(None), readable: Mutex::new(None), available: AtomicBool::new(true), locked: AtomicBool::new(false) });
                operations.insert(id.clone(), operation.clone());
                (operation, true)
            }
        }
    };
    if fresh {
        let worker = operation.clone();
        tokio::spawn(async move {
            // The HTTP handler can return its ACK now; shutdown must still wait
            // for this exact admitted mutation, including final receipt storage.
            let _lease = lease;
            let completed = worker.clone();
            let result = tokio::task::spawn_blocking(move || perform(&api, &id, &worker, ask)).await;
            let mut event = match result {
                Ok(Ok(changed)) => VaultEvent::Completed { changed },
                Ok(Err(error)) => VaultEvent::Failed { error },
                Err(_) => VaultEvent::Failed { error: Error::new(Kind::Internal, "vault_operation_failed", "the vault operation stopped unexpectedly; check its status before retrying") },
            };
            // Publish under the same short critical section as lock/expiry/ACK.
            // A late worker must not reinstall a secret after it was revoked.
            let _registry = OPERATIONS.lock().expect("vault operations");
            sanitize(&mut event, &completed);
            *completed.finished.lock().expect("vault result age") = Some(Instant::now());
            completed.events.send_replace(event);
        });
    }
    let mut receiver = operation.events.subscribe();
    receiver.mark_changed();
    let updates = futures_util::stream::unfold((receiver, false, operation), |(mut receiver, ended, operation)| async move {
        if ended { return None; }
        loop {
            receiver.changed().await.ok()?;
            let mut event = receiver.borrow_and_update().clone();
            if matches!(event, VaultEvent::Accepted { .. }) { continue; }
            let ended = terminal(&event);
            sanitize(&mut event, &operation);
            return Some((Ok::<_, Infallible>(frame(&event)), (receiver, ended, operation)));
        }
    });
    let stream = futures_util::stream::once(async move { Ok::<_, Infallible>(frame(&accepted)) }).chain(updates);
    Ok(private(Sse::new(stream).keep_alive(KeepAlive::default()).into_response()))
}

fn frame(event: &VaultEvent) -> Event {
    Event::default().data(serde_json::to_string(event).expect("vault event serializes"))
}

fn sanitize(event: &mut VaultEvent, operation: &Operation) {
    if !operation.available.load(Ordering::Acquire) {
        *event = expired_result();
        return;
    }
    if let VaultEvent::Failed { error } = event
        && operation.locked.load(Ordering::Acquire)
    {
        // A storage transition can revoke its own old view before discovering
        // that a native reader is still using it. Preserve the public error
        // category (especially Busy); redact only its private details.
        error.message = match error.code.as_str() {
            "project_busy" => "Close applications using the project, then retry the storage change.",
            "project_locked" => "The project is locked.",
            _ => "The vault operation could not finish after project access changed; check its protection state before retrying.",
        }.into();
        error.remedy = None;
        error.next = vec!["cozy vault status".into()];
        error.cause = None;
        error.metadata = None;
        return;
    }
    if let VaultEvent::Completed { changed } = event
        && changed.vault.encrypted
        && !operation.readable.lock().expect("vault result authority").as_ref().is_some_and(|token| token.load(Ordering::Acquire))
    {
        if operation.action == "lock" {
            changed.recovery_secret = None;
            changed.vault.view = None;
            changed.skipped_files.clear();
            changed.preserved_source = None;
        } else {
            *event = VaultEvent::Failed { error: Error::new(Kind::Conflict, "project_locked", "the project was locked before its private result was read").next("cozy vault unlock") };
        }
    }
}

fn perform(api: &Arc<Api>, id: &str, operation: &Operation, mut ask: VaultAsk) -> Result<VaultChanged, Error> {
    let previous = api.calcifer.store().write(|transaction| {
        let previous: Option<(String, String)> = transaction.query_row("SELECT binding, state FROM vault_operations WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if previous.is_none() {
            transaction.execute("INSERT INTO vault_operations (id,binding,state) VALUES (?1,?2,'accepted')", rusqlite::params![id, operation.binding])?;
        }
        Ok(previous)
    }).map_err(|error| Error::new(Kind::Unavailable, "vault_operation_receipt", error.to_string()))?;
    if let Some((binding, state)) = previous {
        if binding != operation.binding { return Err(Error::new(Kind::Conflict, "vault_operation_conflict", "this operation ID already names another request")); }
        let (code, message) = if state == "completed" {
            ("vault_operation_result_expired", "the operation completed, but its private reply is no longer held; check the vault status and replace the recovery secret if needed")
        } else {
            ("vault_operation_interrupted", "Calcifer stopped before this operation's result was received; check the vault status before starting a new operation")
        };
        return Err(Error::new(Kind::Conflict, code, message).next("cozy vault status"));
    }
    let root = super::vault::location(api, &operation.named)?;
    *operation.root.lock().expect("vault operation root") = Some(root.clone());
    let mut progress = Progress::new(operation.events.clone());
    let result = super::vault::operate(api, &root, &mut ask, &mut progress);
    *operation.readable.lock().expect("vault result authority") = progress.readable;
    let state = if result.is_ok() { "completed" } else { "failed" };
    api.calcifer.store().write(|transaction| {
        transaction.execute("UPDATE vault_operations SET state=?2 WHERE id=?1", rusqlite::params![id, state])?;
        Ok(())
    }).map_err(|error| Error::new(Kind::Unavailable, "vault_operation_receipt", format!("the operation ended but its receipt could not be saved: {error}; check the vault status before retrying")))?;
    result
}

/// Progress stays local to the worker and sends a bounded latest snapshot. It
/// never blocks filesystem work because a terminal or browser is reading slowly.
pub(super) struct Progress {
    events: Option<watch::Sender<VaultEvent>>,
    phase: &'static str,
    files: u64,
    bytes: u64,
    files_total: u64,
    bytes_total: u64,
    last: Instant,
    skipped: Vec<PathBuf>,
    pub(super) readable: Option<Arc<AtomicBool>>,
}

impl Progress {
    fn new(events: watch::Sender<VaultEvent>) -> Self { Self { events: Some(events), ..Self::quiet() } }
    pub(super) fn quiet() -> Self { Self { events: None, phase: "preparing", files: 0, bytes: 0, files_total: 0, bytes_total: 0, last: Instant::now(), skipped: vec![], readable: None } }
    pub(super) fn begin_inventory(&mut self) { self.files_total = 0; self.bytes_total = 0; }
    pub(super) fn plan_file(&mut self, bytes: u64) { self.files_total += 1; self.bytes_total = self.bytes_total.saturating_add(bytes); }
    pub(super) fn inventory_done(&mut self) { self.report(true); }
    #[cfg(test)]
    pub(super) fn counts(&self) -> (u64, u64, u64, u64) { (self.files, self.files_total, self.bytes, self.bytes_total) }
    pub(super) fn skip(&mut self, path: &std::path::Path) { if !self.skipped.iter().any(|known| known == path) { self.skipped.push(path.to_owned()); } }
    pub(super) fn skipped(&self) -> Vec<PathBuf> { self.skipped.clone() }
    pub(super) fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
        self.report(true);
    }
    pub(super) fn copied(&mut self, bytes: u64) { self.bytes = self.bytes.saturating_add(bytes); self.report(false); }
    pub(super) fn file_done(&mut self) { self.files += 1; self.report(false); }
    fn report(&mut self, force: bool) {
        if force || self.last.elapsed() >= Duration::from_millis(250) {
            if let Some(events) = &self.events {
                events.send_replace(VaultEvent::Progress { phase: self.phase.into(), files_done: self.files, bytes_done: self.bytes, files_total: self.files_total, bytes_total: self.bytes_total });
            }
            self.last = Instant::now();
        }
    }
}
