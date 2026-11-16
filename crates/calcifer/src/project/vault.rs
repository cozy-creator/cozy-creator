//! Optional local encrypted project sessions. A registered path never becomes a plaintext fallback mount.
use std::{collections::{BTreeMap, BTreeSet}, fs, io::Read as _, path::{Path as FsPath, PathBuf}, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant, SystemTime}};
use axum::{body::Bytes, extract::{Path, State, rejection::BytesRejection}, http::{HeaderValue, header}, response::{IntoResponse, Response}};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultStatus}, error::{Error, Kind}};
use project_mount::{MountOptions, Mounted, Provider};
use project_vault::{Credential, Hold, Keybag, ProjectSecret, SecretString, SessionPolicy, SessionTimer, KEYBAG_FILE};
use sha2::{Digest as _, Sha256};
use crate::api::{Api, Refusal, parse};
use super::vault_operation::{self, Progress};

#[cfg(test)]
#[path = "vault_decrypt_tests.rs"]
mod decrypt_tests;

pub(super) const BACKING: &str = "project.vault.data";
const RUNTIME_DIR: &str = ".vault-runtime";
const RUNTIME_MARKER: &[u8] = b"cozy-project-mount-runtime/1\n";
const TRANSITION_RECEIPT: &str = ".vault-transition.json";
const TRANSITION_VERIFY: &str = ".vault-transition.verify.json";
const DEFAULT_IDLE: u64 = api::calcifer::project::DEFAULT_VAULT_IDLE_SECONDS;
type Held = Arc<Mutex<Session>>;

/// In-memory identity of one physical mount. It survives reauthorization of that mount,
/// but a new unlock can never inherit caches from an older mount at the same OS path.
#[derive(Clone)]
pub(crate) struct MountIdentity(Arc<()>);
impl PartialEq for MountIdentity { fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) } }
impl Eq for MountIdentity {}
impl std::hash::Hash for MountIdentity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&Arc::as_ptr(&self.0), state); }
}
static SESSIONS: LazyLock<Mutex<BTreeMap<PathBuf, Held>>> = LazyLock::new(Mutex::default);
static ROOTS: LazyLock<Mutex<BTreeMap<PathBuf, PathBuf>>> = LazyLock::new(Mutex::default);
static OPERATIONS: LazyLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);
static ACCESS: LazyLock<Mutex<BTreeMap<PathBuf, Arc<tokio::sync::RwLock<()>>>>>= LazyLock::new(Mutex::default);
static CONVERTING: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);
/// What wakes the vault clock (`start`) before its next deadline: a session began or is closing, its mount ended, work on its project
/// began or ended (`stir`). A wall-clock alarm, which a suspend or a clock change doesn't put off.
static CLOCK: LazyLock<Option<platform::alarm::Alarm>> = LazyLock::new(|| {
    platform::alarm::Alarm::new().inspect_err(|e| tracing::warn!("vaults have no alarm, so the vault clock looks each second: {e}")).ok()
});
/// How soon the clock looks at a vault again while what holds it or puts off its lock says nothing when it ends: a cut or a file
/// move, a reader, an operation, a close a busy view or an unsaved page puts off.
const AGAIN: Duration = Duration::from_secs(1);
/// A session closed: a shutdown waiting on the views to close tries again.
pub(crate) static CLOSED: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub(crate) fn stir() {
    if let Some(clock) = &*CLOCK { clock.wake(); }
}

/// Work on the project at `root` ended (a request answered, a tool's process gone): its vault's clocks go on.
pub(crate) fn work_ended(root: &FsPath) {
    if SESSIONS.lock().expect("vault sessions").contains_key(root) { stir(); }
}

struct Session {
    identity: MountIdentity,
    mount: Mounted,
    secret: ProjectSecret,
    keybag: Keybag,
    idle: u64,
    maximum: u64,
    timer: SessionTimer,
    active_ms: i64,
    began_ms: i64,
    closing: bool,
    readable: Arc<AtomicBool>,
    runtime: PathBuf,
}

fn error(code: &'static str, why: impl std::fmt::Display) -> Error { Error::new(Kind::Unavailable, code, why.to_string()) }
fn io(e: std::io::Error) -> Error { error("vault_io", e) }
fn provider(e: impl std::fmt::Display) -> Error { error("vault_provider", e) }
/// The provider found and checked: what it needs that this computer lacks is a refusal naming its install command, never a 503
/// (cl-1000).
fn unusable(e: project_mount::Error) -> Error {
    match e {
        project_mount::Error::DependencyMissing { ref install, .. } => {
            install.iter().cloned().fold(Error::new(Kind::Conflict, "vault_dependency_missing", e.to_string()), Error::next)
        }
        project_mount::Error::DriverMissing(_) | project_mount::Error::HelperMissing => Error::new(Kind::Conflict, "vault_dependency_missing", e.to_string()),
        other => provider(other),
    }
}

/// `unusable`, said to one creating a project: an unencrypted one still works here.
fn unusable_for_creation(e: project_mount::Error) -> Error {
    let refused = unusable(e);
    match refused.code == "vault_dependency_missing" {
        true => Error { message: format!("{}; or create the project unencrypted", refused.message), ..refused }.next("cozy new --unencrypted"),
        false => refused,
    }
}
/// A provider's create or mount failure: its cause on one line for the person, all it said in the log (cl-1049). One whose
/// supervisor this cozy can't start, as an update replaced it, names the restart (cl-981).
fn mounting(e: project_mount::Error) -> Error {
    if let Some(detail) = e.provider_detail() {
        tracing::warn!(error = %e, provider_said = detail, "encrypted filesystem provider failed");
    }
    match e {
        project_mount::Error::Platform(e @ platform::Error::Updated { .. }) => crate::cozy::updated(e),
        other => provider(other),
    }
}
fn key(e: project_vault::Error) -> Error {
    match e {
        rejected @ (project_vault::Error::Unlock | project_vault::Error::Recovery) => Error::new(Kind::Credential, "vault_unlock_failed", rejected.to_string()),
        other => error("vault_key", other),
    }
}
fn held(root: &FsPath) -> Option<Held> { SESSIONS.lock().expect("vault sessions").get(root).cloned() }
pub(crate) fn mount_identity(root: &FsPath) -> Option<MountIdentity> { held(root).map(|session| session.lock().expect("vault session").identity.clone()) }
/// Whether `view` is an unencrypted project's own folder: no keybag behind it and no mounted view's place.
pub(crate) fn ordinary_storage(view: &FsPath) -> bool {
    !encrypted(&logical(view)) && !view.components().any(|part| part.as_os_str() == RUNTIME_DIR)
}

pub(crate) fn encrypted(root: &FsPath) -> bool { root.join(KEYBAG_FILE).is_file() || SESSIONS.lock().expect("vault sessions").contains_key(root) }

/// An encrypted project with no open view: Calcifer keeps only its folder.
pub(crate) fn locked_now(root: &FsPath) -> bool { encrypted(root) && held(root).is_none() }

/// The canonical id is available without opening the project's content database.
pub(super) fn project_id(root: &FsPath) -> Option<String> {
    if let Some(session) = held(root) {
        return Some(session.lock().expect("vault session").secret.project_id().to_owned());
    }
    Keybag::load(&root.join(KEYBAG_FILE)).ok().map(|keybag| keybag.info().project_id)
}

/// A locked project's id and public code, from its own keybag (cl-984): the index keeps neither.
pub(crate) fn kept_in_keybag(root: &FsPath) -> Option<(String, String)> {
    Keybag::load(&root.join(KEYBAG_FILE)).ok().map(|keybag| { let info = keybag.info(); (info.project_id, info.code) })
}

/// An unlocked project's public code, kept in its keybag beside its id so a compact link finds it once locked (cl-984).
pub(crate) fn keep_code(root: &FsPath, code: &str) -> Result<(), Error> {
    let Some(session) = held(root) else { return Ok(()) };
    let mut session = session.lock().expect("vault session");
    if session.keybag.info().code == code { return Ok(()); }
    let mut keybag = session.keybag.clone();
    keybag.set_code(code).map_err(key)?;
    keybag.save(&root.join(KEYBAG_FILE)).map_err(key)?;
    session.keybag = keybag;
    Ok(())
}

pub(crate) fn plain_project_id(root: &FsPath) -> Option<String> {
    if encrypted(root) { return None; }
    let metadata = fs::symlink_metadata(root.join(super::db::DB)).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() { return None; }
    super::kept_identity(root).map(|(id, _)| id)
}

/// Rehydrate only the existing location index's native-ID-validated retained roots.
/// A moved, missing or stale copy never prevents ordinary controller startup.
pub(crate) fn restore_plain_roots(store: &crate::store::Store) {
    let Ok(projects) = store.known_projects() else { return };
    for project in projects {
        if recover_conversion(&project.path).is_err() || encrypted(&project.path)
            || !project.path.join(api::machine::LOCAL_PRIVATE_MARKER).exists() { continue; }
        let _ = register_plain_root(&project.path, &project.id);
    }
}

/// Identity is already committed before a retained plaintext namespace can be granted.
pub(crate) fn register_plain_root(root: &FsPath, id: &str) -> Result<(), Error> {
    if encrypted(root) { return Ok(()); }
    KEY_ROOTS.lock().expect("vault project roots").retain(|_, known| known != root);
    if !root.join(api::machine::LOCAL_PRIVATE_MARKER).exists() { return Ok(()); }
    if plain_project_id(root).as_deref() != Some(id) { return Ok(()); }
    if let Some(scope) = crate::private_io::retained_scope(root, id)? {
        KEY_ROOTS.lock().expect("vault project roots").insert(id.to_owned(), scope.storage.view_root);
    }
    Ok(())
}

/// Logical registered path of an unlocked mount, including any relative subpath.
/// Whether `path` is inside an encrypted project: what no log outside it may name a run of (cl-1060).
pub(crate) fn encrypted_within(path: &FsPath) -> bool {
    logical(path).ancestors().any(encrypted)
}

pub(crate) fn logical(path: &FsPath) -> PathBuf {
    for (view, root) in ROOTS.lock().expect("vault roots").iter() {
        if let Ok(rest) = path.strip_prefix(view) { return root.join(rest); }
    }
    path.to_owned()
}

/// The private view for a registered encrypted project. No keybag means an ordinary project.
pub(super) fn view(root: &FsPath) -> Result<PathBuf, Error> {
    recover_conversion(root)?;
    if CONVERTING.lock().expect("vault conversions").contains(root) { return Err(error("vault_busy", "the project's storage is being converted")); }
    if !encrypted(root) {
        if exists(&receipt_path(root))? { return Err(error("vault_recovery", "the project's storage transition needs recovery; retry after active requests finish")); }
        return Ok(root.to_owned());
    }
    let Some(session) = held(root) else { return Err(locked(root)); };
    let mut session = session.lock().expect("vault session");
    if session.mount.reconcile().is_err() { mark_closing(&mut session); }
    if session.closing || expired(&session) || !session.mount.is_mounted().map_err(provider)? { return Err(locked(root)); }
    platform::durable::canonical(session.mount.view()).map_err(provider)
}

/// The conversion flag and filesystem classification are one observation. In
/// particular listing cannot retire the index while its directory is renamed.
pub(super) fn keeps_index(root: &FsPath) -> bool {
    let converting = CONVERTING.lock().expect("vault conversions");
    converting.contains(root) || receipt_path(root).is_file() || super::Folder::is_project(root) || encrypted(root)
}

/// A key-holding work lease can drain already-authorized work while the viewing session is locking.
fn work_view(root: &FsPath) -> Result<PathBuf, Error> {
    let Some(session) = held(root) else { return Err(locked(root)); };
    let mut session = session.lock().expect("vault session");
    session.mount.reconcile().map_err(provider)?;
    if !session.mount.is_mounted().map_err(provider)? { return Err(locked(root)); }
    // The provider guards its underlying path against plaintext fallback; lost mounts fail I/O.
    platform::durable::canonical(session.mount.view()).map_err(provider)
}

fn operation(root: &FsPath) -> Arc<Mutex<()>> {
    OPERATIONS.lock().expect("vault operations").entry(root.to_owned()).or_default().clone()
}

fn access(root: &FsPath) -> Arc<tokio::sync::RwLock<()>> {
    ACCESS.lock().expect("vault access").entry(root.to_owned()).or_default().clone()
}

/// Exclusive authority for this exact project's storage transition. Internal final
/// journal writes may borrow it while public work/read admission is already closed.
pub(super) struct ProjectTransition {
    root: PathBuf,
    _access: tokio::sync::OwnedRwLockWriteGuard<()>,
}

impl ProjectTransition {
    pub(super) fn root(&self) -> &FsPath { &self.root }

    pub(super) fn validate(&self, project: &super::Project) -> Result<(), Error> {
        validate_authority(&self.root, project)
    }
}

/// Read authority over this exact project's journal, beside its other readers: what the vault clock looks at a project's work under
/// (`working`). It took the transition (`exclusive`) once, and a tool starting then was refused: its run failed `lock_failed` (cl-1066).
pub(super) struct ProjectInspection {
    root: PathBuf,
    _access: tokio::sync::OwnedRwLockReadGuard<()>,
}

impl ProjectInspection {
    pub(super) fn validate(&self, project: &super::Project) -> Result<(), Error> {
        validate_authority(&self.root, project)
    }
}

/// `project` is `root`'s, on the mount and view it was opened on.
fn validate_authority(root: &FsPath, project: &super::Project) -> Result<(), Error> {
    if root != project.root || project.mount_id != mount_identity(root) {
        return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
    }
    let view = if encrypted(root) { work_view(root)? }
        else { platform::durable::canonical(root).map_err(provider)? };
    if view != project.folder.dir {
        return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
    }
    Ok(())
}

/// [`ProjectInspection`], now: none while a transition holds the project or waits for it.
fn inspection(root: &FsPath) -> Result<ProjectInspection, Error> {
    let guard = access(root).try_read_owned().map_err(|_| Error::new(Kind::Conflict, "project_busy", "the project's storage is changing"))?;
    Ok(ProjectInspection { root: root.to_owned(), _access: guard })
}

fn exclusive(root: &FsPath) -> Result<ProjectTransition, Error> {
    let guard = access(root).try_write_owned().map_err(|_| Error::new(Kind::Conflict, "project_busy", "a project request is still running; retry after it finishes"))?;
    Ok(ProjectTransition { root: root.to_owned(), _access: guard })
}

/// [`exclusive`], queued: what asks for the project after it waits behind it, so it has the project once the requests already
/// holding it end, or `project_busy` after `within`. Only from a blocking thread (`block_in_place`, `spawn_blocking`) holding no
/// read of the project: it would wait for itself.
fn exclusive_within(api: &Api, root: &FsPath, within: Duration) -> Result<ProjectTransition, Error> {
    let queued = tokio::time::timeout(within, access(root).write_owned());
    let Ok(guard) = tokio::runtime::Handle::current().block_on(queued) else {
        let holder = if api.projects.active_requests(root) { "a project request" }
            else if crate::local_code::private_process_active(root) { "a project tool" }
            else { "a project read (an upload, a thumbnail or a measurement)" };
        return Err(Error::new(Kind::Conflict, "project_busy", format!("{holder} is still running; retry after it finishes")));
    };
    Ok(ProjectTransition { root: root.to_owned(), _access: guard })
}

/// Upload routes are outside the project router, but must hold the same storage transition
/// lease through their body and commit. A delayed upload cannot write into a reused drive.
pub(crate) async fn admit_project(api: &Api, named: &str) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let root = location(api, named)?;
    admit_root(&root).await
}

/// [`admit_project`], with the project's root: an upload that lets go of the gate while a lock waits asks for it again by it.
pub(crate) async fn admit_project_root(api: &Api, named: &str) -> Result<(PathBuf, tokio::sync::OwnedRwLockReadGuard<()>), Error> {
    let root = location(api, named)?;
    let held = admit_root(&root).await?;
    Ok((root, held))
}

pub(crate) async fn admit_root(root: &FsPath) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let guard = access(root).read_owned().await;
    view(root)?;
    Ok(guard)
}

/// Synchronous tool admission holds the transition gate only until its active
/// owner has been registered. The caller releases it before waiting for work.
pub(crate) fn try_admit_root(root: &FsPath) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let guard = access(root).try_read_owned().map_err(|_| Error::new(Kind::Conflict, "project_busy", "the project's storage is changing; retry after it finishes"))?;
    view(root)?;
    Ok(guard)
}

/// Whether a lock waits for this project, or holds it: what holds a read of it for long (an upload's body) lets go then, and asks again.
pub(crate) fn lock_waiting(root: &FsPath) -> bool {
    access(root).try_read_owned().is_err()
}

/// An already accepted walker may finish while viewing is closing, but only on its own mount.
pub(super) async fn admit_work(project: &super::Project) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let guard = access(&project.root).read_owned().await;
    validate_work(project)?;
    Ok(guard)
}

/// Bounded access for an internal active-run callback. It must not wait behind a
/// closing transition or turn an unavailable project into global maintenance state.
pub(super) fn try_admit_work(project: &super::Project) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let guard = access(&project.root).try_read_owned().map_err(|_| Error::new(Kind::Conflict, "project_busy", "the project's storage is changing; retry after it finishes"))?;
    validate_work(project)?;
    Ok(guard)
}

fn validate_work(project: &super::Project) -> Result<(), Error> {
    if project.mount_id != mount_identity(&project.root) || resolve_path(&project.root)? != project.folder.dir {
        return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
    }
    Ok(())
}

/// Clients following a run of each project's (`following`), by root: the project is in use until they let go.
static FOLLOWING: LazyLock<Mutex<BTreeMap<PathBuf, usize>>> = LazyLock::new(Mutex::default);

/// A client follows a run whose outputs land under `path`: while it does, the run's encrypted project is in use, as its run is.
pub(crate) struct Following(PathBuf);

pub(crate) fn following(path: &FsPath) -> Option<Following> {
    let path = logical(path);
    let root = path.ancestors().find(|root| encrypted(root))?.to_owned();
    *FOLLOWING.lock().expect("vault followers").entry(root.clone()).or_default() += 1;
    stir();
    Some(Following(root))
}

impl Drop for Following {
    fn drop(&mut self) {
        let mut following = FOLLOWING.lock().expect("vault followers");
        if let Some(count) = following.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 { following.remove(&self.0); }
        }
        drop(following);
        stir();
    }
}

pub(crate) fn readable_token(path: &FsPath) -> Option<Arc<AtomicBool>> {
    let path = logical(path);
    let root = path.ancestors().find(|root| encrypted(root))?;
    held(root).map(|session| session.lock().expect("vault session").readable.clone())
}

pub(crate) fn resolve_path(path: &FsPath) -> Result<PathBuf, Error> {
    let path = logical(path);
    if let Some(root) = path.ancestors().find(|root| encrypted(root)) {
        return Ok(work_view(root)?.join(path.strip_prefix(root).expect("ancestor")));
    }
    Ok(path)
}

pub(crate) fn require_path(path: &FsPath) -> Result<(), Error> {
    let path = logical(path);
    if let Some(root) = path.ancestors().find(|root| encrypted(root)) { view(root)?; }
    Ok(())
}

fn locked(_root: &FsPath) -> Error {
    Error::new(Kind::Conflict, "project_locked", "the project is locked")
        .next("cozy vault unlock")
}

pub(super) fn location(api: &Api, named: &str) -> Result<PathBuf, Error> {
    if api::target::written_as_path(named) {
        let path = FsPath::new(named);
        if !path.is_absolute() { return Err(Error::new(Kind::Usage, "project_path", "a project path must be absolute")); }
        // Match the index's user-spelled canonical path (not Windows' verbatim prefix).
        // Resolve the parent first so an interrupted rename can recover even with root absent.
        let candidate = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => platform::durable::canonical(parent).map_err(provider)?.join(name),
            _ => path.to_owned(),
        };
        recover_conversion(&candidate)?;
        let path = match platform::durable::canonical(&candidate) {
            Ok(path) => logical(&path),
            // Its folder gone (cl-1124): a session still held for it ends through its operation, which says so.
            Err(_) if !candidate.exists() => {
                let path = logical(&candidate);
                if held(&path).is_some() { return Ok(path); }
                return Err(Error::new(Kind::NotFound, "project_gone", format!("There's no project at {}: its folder is gone.", path.display())));
            }
            Err(e) => return Err(provider(e)),
        };
        if let Some(root) = path.ancestors().find(|root| encrypted(root)) { return Ok(root.to_owned()); }
        return super::named(api, named);
    }
    Ok(super::known(api, named)?.path)
}

/// An idle lock that failed: retried by the standard backoff, each attempt said in the log, and
/// its next commands shown by status until a lock succeeds.
struct LockRetry { backoff: tensorfs::fetch::Backoff, at: std::time::Instant, next: Vec<String> }
static LOCK_RETRIES: LazyLock<Mutex<BTreeMap<PathBuf, LockRetry>>> = LazyLock::new(Mutex::default);

fn idle_lock_failed(root: &FsPath, failure: &Error) {
    let mut retries = LOCK_RETRIES.lock().expect("lock retries");
    let retry = retries.entry(root.to_owned()).or_insert_with(|| LockRetry { backoff: tensorfs::fetch::Backoff::background(), at: std::time::Instant::now(), next: vec![] });
    // Its code alone, with the folder: what refused it can name a run, which no log outside the project carries (cl-1060).
    let why = format!("{} did not lock at its deadline: {}", root.display(), failure.code);
    let (wait, said) = retry.backoff.again(&why, false, None).unwrap_or_else(|| {
        // Never left unlocked quietly: say the spent round and start another.
        let spent = retry.backoff.spent(&why);
        retry.backoff = tensorfs::fetch::Backoff::background();
        (Duration::from_secs(16), spent)
    });
    tracing::warn!("{said}");
    retry.at = std::time::Instant::now() + wait;
    retry.next = failure.next.iter().cloned().chain(["cozy vault lock".to_owned()]).collect();
}

fn status(root: &FsPath) -> VaultStatus {
    let mut state = if encrypted(root) { "locked" } else { "plain" };
    let (mut view, mut expires, mut idle) = (None, None, DEFAULT_IDLE);
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        if session.mount.reconcile().is_err() { mark_closing(&mut session); }
        idle = session.idle;
        // Only removing the fully closed session acknowledges lock. A native volume can
        // lose its path while its exact image/key or an old handle still needs cleanup.
        state = "locking";
        if session.mount.is_mounted().unwrap_or(false) {
            state = if session.closing || expired(&session) { "locking" } else { "unlocked" };
            if state == "unlocked" { view = Some(session.mount.view().to_owned()); }
            let idle_at = (session.idle > 0).then(|| session.active_ms.saturating_add(milliseconds(session.idle)));
            // Work in progress defers the maximum until it ends.
            let max_at = (session.maximum > 0 && session.timer.held() != Hold::Working).then(|| session.began_ms.saturating_add(milliseconds(session.maximum)));
            expires = match (idle_at, max_at) { (Some(a), Some(b)) => Some(a.min(b)), (a,b) => a.or(b) };
        }
    }
    if receipt_path(root).exists() {
        state = "storage_update_required";
        view = None;
        expires = None;
    }
    let next = match state {
        "locked" | "storage_update_required" => vec!["cozy vault unlock".into()],
        "locking" => LOCK_RETRIES.lock().expect("lock retries").get(root).map(|retry| retry.next.clone())
            .unwrap_or_else(|| vec!["close applications using the project, then lock again".into()]),
        _ => vec![],
    };
    VaultStatus { encrypted: encrypted(root), state: state.into(), path: root.to_owned(), view, expires_at_ms: expires, idle_timeout_s: idle, next }
}

fn milliseconds(seconds: u64) -> i64 { i64::try_from(seconds).unwrap_or(i64::MAX).saturating_mul(1000) }

fn expired(session: &Session) -> bool { session.timer.expired() }

/// Its view can no longer be opened: a lock revoked it, its clocks ran out, or its mount is gone. The sweeper locks it.
fn lapsed(session: &Session) -> bool { session.closing || expired(session) || !mounted(session) }

/// Whether its view is still mounted. A mount state it cannot read is mounted: a live session never lapses on a failed
/// read (a lock then ended its work and refused, e2e 43700377280). The health record says it happened, at most once a minute.
fn mounted(session: &Session) -> bool {
    session.mount.is_mounted().unwrap_or_else(|_| {
        static SAID: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let mut said = SAID.lock().expect("mount reads");
        if !said.is_some_and(|at| at.elapsed() < Duration::from_secs(60)) {
            *said = Some(std::time::Instant::now());
            crate::health::service_health(crate::health::HealthEvent::MountUnread);
        }
        true
    })
}

fn timer(idle: u64, maximum: u64) -> SessionTimer {
    SessionTimer::new(SessionPolicy { idle_seconds: (idle > 0).then_some(idle), maximum_seconds: (maximum > 0).then_some(maximum) })
}

fn credential(ask: &mut VaultAsk) -> Result<Credential, Error> {
    match (ask.passphrase.take(), ask.recovery_secret.take()) {
        (Some(pass), None) if !pass.is_empty() => Ok(Credential::Passphrase(SecretString::from(pass))),
        (None, Some(recovery)) if !recovery.is_empty() => Ok(Credential::Recovery(SecretString::from(recovery))),
        _ => Err(Error::new(Kind::Usage, "vault_credential", "supply exactly one passphrase or recovery secret")),
    }
}

fn new_passphrase(ask: &mut VaultAsk) -> Result<SecretString, Error> {
    ask.passphrase.take().filter(|pass| !pass.is_empty()).map(SecretString::from)
        .ok_or_else(|| Error::new(Kind::Usage, "vault_credential", "a new passphrase is required"))
}

/// Mount bookkeeping is part of this project, not a second project index under
/// Calcifer's home. The marker prevents adopting an unrelated user directory.
fn project_runtime(root: &FsPath) -> Result<PathBuf, Error> {
    let base = root.join(RUNTIME_DIR);
    match fs::symlink_metadata(&base) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if fs::read(base.join("owner")).map_err(io)? != RUNTIME_MARKER {
                return Err(error("vault_runtime", "the reserved project runtime directory belongs to another writer"));
            }
        }
        Ok(_) => return Err(error("vault_runtime", "the project runtime must be an owned directory")),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&base).map_err(io)?;
            platform::durable::set_mode(&base, 0o700).map_err(provider)?;
            platform::durable::replace(&base.join("owner"), RUNTIME_MARKER, 0o600).map_err(provider)?;
        }
        Err(why) => return Err(io(why)),
    }
    let runtime = base.join(fresh());
    fs::create_dir(&runtime).map_err(io)?;
    platform::durable::set_mode(&runtime, 0o700).map_err(provider)?;
    Ok(runtime)
}

fn retire_runtime(runtime: &FsPath) -> Result<(), Error> {
    #[cfg(not(windows))]
    match fs::remove_dir(runtime.join("view")) {
        Ok(()) => {},
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => {},
        Err(why) => return Err(io(why)),
    }
    match fs::remove_dir(runtime) {
        Ok(()) => Ok(()),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(why) => Err(io(why)),
    }
}

/// A project with no session holds no key or live view: a dead Calcifer's mount supervisor
/// unmounted and ended its provider when that exact process ended (`project_mount::supervise`).
/// Detach any view that outlived it and retire its runtime folder.
fn retire_orphaned_views(root: &FsPath) -> Result<(), Error> {
    let base = root.join(RUNTIME_DIR);
    match fs::symlink_metadata(&base) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if fs::read(base.join("owner")).map_err(io)? != RUNTIME_MARKER {
                return Err(error("vault_runtime", "the reserved project runtime directory belongs to another writer"));
            }
        }
        Ok(_) => return Err(error("vault_runtime", "the project runtime must be an owned directory")),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(why) => return Err(io(why)),
    }
    for entry in fs::read_dir(&base).map_err(io)? {
        let entry = entry.map_err(io)?;
        if !entry.file_type().map_err(io)?.is_dir() { continue; }
        project_mount::detach_stale(&project_mount::view_path(&entry.path()).map_err(provider)?);
        retire_runtime(&entry.path())?;
    }
    Ok(())
}

/// Retire only empty, detached runtime folders from earlier Calcifer versions.
/// Current mounts live inside their own project, so this never touches an active owned view.
pub(crate) fn sweep_runtime(calcifer: &FsPath) -> bool {
    fn retire(path: &FsPath) -> bool {
        let Ok(view) = project_mount::view_path(path) else { return false };
        project_mount::detach_stale(&view);
        let _ = fs::remove_dir(&view);
        let _ = fs::remove_dir(path);
        !path.exists()
    }
    let base = calcifer.join("vaults");
    let Ok(entries) = fs::read_dir(&base) else { return !base.exists() };
    let mut clean = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
            if let Ok(children) = fs::read_dir(&path) {
                for child in children.flatten().map(|entry| entry.path()) {
                    if child.file_name().is_some_and(|name| name == "view") { continue; }
                    if fs::symlink_metadata(&child).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) { clean &= retire(&child); }
                }
            }
            clean &= retire(&path);
        } else { clean = false; }
    }
    let _ = fs::remove_dir(&base);
    clean
}

fn mount(api: &Api, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk) -> Result<(), Error> {
    if let Some(known) = api.calcifer.maintenance_store().known_project(&keybag.info().project_id).map_err(|e| error("vault_journal", e))? {
        if known.path != root && Keybag::load(&known.path.join(KEYBAG_FILE)).is_ok_and(|other| other.info().project_id == keybag.info().project_id) {
            return Err(Error::new(Kind::Conflict, "vault_project_copy", "this encrypted project's original location still exists; move it away before restoring a copy"));
        }
    }
    if KEY_ROOTS.lock().expect("vault project roots").get(&keybag.info().project_id).is_some_and(|known| known != root && held(known).is_some()) {
        return Err(Error::new(Kind::Conflict, "vault_project_copy", "another location of this encrypted project is unlocked"));
    }
    let runtime = project_runtime(root)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(unusable)?;
    let took = crate::took::Took::new("vault mount");
    let mounted = secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(mounting)?;
    drop(took);
    let canonical_view = platform::durable::canonical(mounted.view()).map_err(provider)?;
    ROOTS.lock().expect("vault roots").insert(mounted.view().to_owned(), root.to_owned());
    ROOTS.lock().expect("vault roots").insert(canonical_view.clone(), root.to_owned());
    crate::private_io::descriptor_for_mounted(&canonical_view, &keybag.info().project_id, keybag.info().generation)?;
    TRUSTED_KEYS.lock().expect("trusted vault recipients").insert(root.to_owned(), keybag.clone());
    KEY_ROOTS.lock().expect("vault project roots").insert(keybag.info().project_id.clone(), root.to_owned());
    let idle = ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE);
    let maximum = ask.max_duration_s.unwrap_or(0);
    let session = Session { identity: MountIdentity(Arc::new(())), mount: mounted, secret, keybag, idle, maximum, timer: timer(idle, maximum), active_ms: super::now_ms(), began_ms: super::now_ms(), closing: false, readable: Arc::new(AtomicBool::new(true)), runtime };
    // Its supervisor's exit is its mount's end: a lock, its provider dying, an unmount from outside.
    let supervisor = session.mount.supervisor();
    SESSIONS.lock().expect("vault sessions").insert(root.to_owned(), Arc::new(Mutex::new(session)));
    match supervisor {
        Ok(supervisor) => {
            let _ = std::thread::Builder::new().name("vault-mount".into()).spawn(move || {
                let _ = supervisor.wait();
                stir();
            });
        }
        Err(e) => tracing::warn!("{}: its mount's end is seen at its next deadline: {e}", root.display()),
    }
    stir();
    Ok(())
}

fn forget(api: &Arc<Api>, root: &FsPath, transition: &ProjectTransition) -> Result<(), Error> {
    forget_with(api, root, transition, true)
}

/// [`forget`], `snapshot` when what's next reads its files as they are (a conversion, a lock): project.db's WAL checkpointed into it.
/// A folder moved whole (cl-1097) takes its WAL with it, and waits on no reader for that.
fn forget_with(api: &Arc<Api>, root: &FsPath, transition: &ProjectTransition, snapshot: bool) -> Result<(), Error> {
    let available = !encrypted(root) || held(root).is_some_and(|session| session.lock().expect("vault session").mount.is_mounted().unwrap_or(false));
    if api.projects.active_requests(root) || crate::local_code::private_process_active(root) {
        return Err(Error::new(Kind::Conflict, "project_busy", "a project request or tool is still finishing; retry after it stops"));
    }
    for project in api.projects.now_open().iter().filter(|project| project.root == root) {
        let store = api.projects.transition_store(api, &project.id, transition)?;
        if !store.unsettled().map_err(|_| error("project_journal", "the project journal could not be checked"))?.is_empty()
            || !store.unsettled_here().map_err(|_| error("project_journal", "the project journal could not be checked"))?.is_empty()
            // A finished call's outputs still being saved into it (cl-1060).
            || api.calcifer.writing_into(&project.id).is_some()
        {
            return Err(Error::new(Kind::Conflict, "project_busy", "the project's work is still stopping; retry after it settles"));
        }
    }
    if available { crate::local_code::retire_private_processes(root)?; }
    let projects = api.projects.now_open();
    for project in projects {
        if project.root != root { continue; }
        if *project.walking.lock().expect("walking lock") || project.moving.try_lock().is_err() || project.cutting.try_lock().is_err() {
            return Err(Error::new(Kind::Conflict, "project_busy", "The project is rendering, cutting or transferring files.").remedy("Wait for project work to finish, then retry.").next("cozy open"));
        }
        if let Ok(mut model) = project.model.try_lock() { *model = None; }
        *project.told.lock().expect("project told") = None;
        crate::events::release_project_readers(&project.id);
        crate::local_code::forget_readings(&project.id);
        project.folder.close_connections(available && snapshot)?;
        api.projects.open.lock().expect("projects lock").remove(&project.id);
        crate::events::project_closed(&project.id);
    }
    if available && snapshot {
        // A vault may have no retained Project/Folder reader at all. Still checkpoint it before
        // a read-only source mount: immutable SQLite must never ignore committed WAL frames.
        let source = if encrypted(root) { work_view(root)? } else { root.to_owned() };
        let conn = rusqlite::Connection::open_with_flags(source.join("project.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| error("vault_database", format!("open project before storage transition: {e}")))?;
        let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .map_err(|e| error("vault_database", format!("checkpoint project before storage transition: {e}")))?;
        if busy != 0 { return Err(Error::new(Kind::Conflict, "project_busy", "a database reader is preventing its verified storage snapshot")); }
    }
    Ok(())
}

fn mark_closing(session: &mut Session) {
    let changed = !session.closing;
    session.closing = true;
    revoke(&session.readable);
    if changed {
        crate::events::project_vault(session.secret.project_id(), "locking");
        stir();
    }
}

/// The pages showing the project at `root` send what they typed of it, before it closes: how many didn't answer `within`.
fn save_first(root: &FsPath, within: Duration) -> usize {
    let Some(id) = project_id(root).or_else(|| plain_project_id(root)) else { return 0 };
    tokio::runtime::Handle::current().block_on(crate::events::save_first(&[id], within))
}

/// A run still writing into the project, as a deadline's or a stop's lock waits on it (cl-1060). It names no run and no project:
/// this refusal is logged, outside the project.
fn saving(api: &Arc<Api>, root: &FsPath) -> Option<Error> {
    let projects = api.projects.now_open();
    let project = projects.iter().find(|project| project.root == root)?;
    api.calcifer.writing_into(&project.id)?;
    Some(Error::new(Kind::Conflict, "project_busy", "a run is still writing into the project; it locks once that's done"))
}

/// Before a lock closes the machine's side of the project (cl-1060), which forgets what the machine still holds for it: every
/// output its runs still owe lands first, judged by progress, never a timer. The lock waits while those saves move bytes; a save
/// silent for `EXPORT_SILENCE` (its machine not answering, its exporter unable to go on) is cut: its exporter ends, its half-written
/// files go. A local run's outputs, which the machine's close then forgets, are recorded not saved and its run reads failed, saying
/// why. A remote machine's run keeps its outputs across a lock (the lock closes only this computer's machine,
/// `private_io::close_private_project`), so its save stays owed and resumes when that machine answers. The runs it cut and the runs
/// it left owed, by name, for the lock's answer to its own client and nothing else (no log). One rule for every lock.
fn drain_saves(api: &Arc<Api>, root: &FsPath, mut saving: impl FnMut()) -> Result<(Vec<String>, Vec<String>), Error> {
    let journal = |_: crate::store::StoreError| error("project_journal", "the project journal could not be checked");
    let projects = api.projects.now_open();
    let Some(project) = projects.iter().find(|project| project.root == root).cloned() else { return Ok((vec![], vec![])) };
    let owner = super::RunOwner::of(&project, &api.calcifer);
    let (mut said, mut busy) = (false, None::<Instant>);
    loop {
        let owed = match owner.store() {
            Ok(store) => { busy = None; store.exports_owed().map_err(journal)? }
            // Another lock's turn ahead of it on the project: looked at again shortly, no longer than a save may go silent.
            Err(crate::store::StoreError::Locked(code)) if code == "project_busy" => {
                if busy.get_or_insert_with(Instant::now).elapsed() >= crate::outputs::EXPORT_SILENCE {
                    return Err(Error::new(Kind::Conflict, "project_busy", "the project stayed busy; lock again in a moment"));
                }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(journal(e)),
        };
        if owed.is_empty() { return Ok((vec![], vec![])); }
        // An exporter missing is started; one going is left alone: waking it is no progress, and cuts its pause.
        for (id, _) in &owed { if !api.calcifer.exporting(id) { api.calcifer.export(id); } }
        if !said { saving(); said = true; }
        if owed.iter().any(|(id, _)| api.calcifer.export_moving(id)) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        // Silent past `EXPORT_SILENCE`: cut, said so on the run and its items.
        let ids: Vec<crate::RunId> = owed.iter().map(|(id, _)| id.clone()).collect();
        api.calcifer.end_exports_of(&ids);
        let store = owner.store().map_err(journal)?;
        let (mut cut, mut deferred) = (vec![], vec![]);
        for (id, number) in owed {
            let Ok(Some(row)) = store.run(id.as_str()) else { continue };
            let items = store.outputs_to_export(id.as_str()).unwrap_or_default();
            crate::outputs::drop_partials(&row, &items);
            // Its machine keeps the outputs: owed, saved when it answers (a rental's paid work is never thrown away).
            if row.machine.as_deref() != Some("local") {
                deferred.push(format!("{}-{number}", project.key()));
                continue;
            }
            let why = "not saved: its save stopped moving before the project locked";
            for item in &items {
                if let Ok(events) = store.export_failed(id.as_str(), &item.name, item.index, item.rev, why) { api.calcifer.publish(&id, events); }
            }
            let reason = ::api::calcifer::Reason { code: "output_not_saved".into(), message: "the project locked while its outputs' save had stopped moving: they were not saved".into(), ..Default::default() };
            if let Ok(events) = store.outputs_not_saved(id.as_str(), &reason) { api.calcifer.publish(&id, events); }
            cut.push(format!("{}-{number}", project.key()));
        }
        return Ok((cut, deferred));
    }
}

/// Every unlocked vault's pages send what they typed, before a stop closes them all.
pub(crate) async fn save_open() {
    // `project_id` takes SESSIONS itself: the roots are read first.
    let roots: Vec<PathBuf> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
    let ids: Vec<String> = roots.iter().filter_map(|root| project_id(root)).collect();
    let unanswered = crate::events::save_first(&ids, LOCK_DRAIN).await;
    if unanswered > 0 { tracing::warn!("stopping: {unanswered} page(s) didn't say they sent what was typed in {} s", LOCK_DRAIN.as_secs()); }
}

/// What a lock says when `local` didn't answer it (cl-1064).
const OWED_LINE: &str = "locked; local didn't answer, so its clean-up for this project runs when it next answers";
/// The projects whose last lock said it, for the lock's answer.
static OWED_SAID: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

fn unanswered_seal() -> Error {
    Error::new(Kind::Unavailable, "private_storage_unavailable", "local didn't answer to close the project's execution storage; retry once it answers")
        .next("cozy machine show local")
}

/// What `root`'s last lock says of a machine that didn't answer it, once.
fn owed_said(root: &FsPath) -> Option<String> {
    OWED_SAID.lock().expect("owed said").remove(root).then(|| OWED_LINE.to_owned())
}

fn close(api: &Arc<Api>, root: &FsPath, transition: &ProjectTransition) -> Result<(), Error> {
    // Under the project's turn and before anything is revoked: a run that still writes into it (one `drain_saves` didn't see
    // start) holds it (cl-1060).
    if let Some(error) = saving(api, root) { return Err(error); }
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        mark_closing(&mut session);
    }
    vault_operation::revoke(root);
    // A lock never waits on a machine that doesn't answer (cl-1064): the project's runs on local settle here as the lock ends them,
    // and what local ran in its view is owed a clean-up for when it answers.
    let unanswered = match project_id(root).or_else(|| plain_project_id(root)) {
        Some(id) if encrypted(root)
            && api.calcifer.unsettled_on(&id, "local").map_err(|_| error("run_cancel", "the project's work could not be read"))?
            && !tokio::runtime::Handle::current().block_on(api.machines.answers("local")) =>
        {
            api.calcifer.cancel_unanswered_for_lock(&id, "local").map_err(|_| error("run_cancel", "the project's work could not be cancelled"))?;
            true
        }
        _ => false,
    };
    // Existing accepted work may drain while the key is retained, but no new viewing stream can read it.
    forget(api, root, transition)?;
    let sealed = match unanswered {
        true => crate::private_io::let_go(root),
        false => tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?,
    };
    // Only an encrypted project's view ends without its machine: a plain project's folder stays, and its machine is asked again.
    if !encrypted(root) && matches!(sealed, crate::private_io::Sealed::Owed(_)) {
        return Err(unanswered_seal());
    }
    crate::runs::files::release_private(root)?;
    if let Some(id) = project_id(root).or_else(|| plain_project_id(root)) { api.calcifer.retire_project(&id); }
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        match &sealed {
            crate::private_io::Sealed::Closed => session.mount.try_unmount().map_err(|why| match why {
                project_mount::Error::Busy(_) => Error::new(Kind::Conflict, "project_busy", "close programs using the project, then retry the storage change"),
                other => provider(other),
            })?,
            // Owed before the view ends, so a Calcifer stopped between them still tells the machine; let go if it doesn't end.
            crate::private_io::Sealed::Owed(view) => {
                api.machines.owe(view, root).map_err(|e| error("vault_index", e.to_string()))?;
                if let Err(why) = session.mount.end() {
                    let _ = api.calcifer.store().paid("local", view);
                    return Err(provider(why));
                }
                OWED_SAID.lock().expect("owed said").insert(root.to_owned());
                say(root, "owed", || OWED_LINE.into());
            }
        }
        retire_runtime(&session.runtime)?;
        drop(session);
        SESSIONS.lock().expect("vault sessions").remove(root);
        LOCK_RETRIES.lock().expect("lock retries").remove(root);
        CLOSED.notify_waiters();
        ROOTS.lock().expect("vault roots").retain(|_, logical| logical != root);
        if let Ok(keybag) = Keybag::load(&root.join(KEYBAG_FILE)) {
            let state = status(root);
            crate::events::project_vault(&keybag.info().project_id, &state.state);
            let converting = CONVERTING.lock().expect("vault conversions").contains(root);
            if state.state != "locked" && !converting {
                return Err(error("project_storage_pending", "the project is closed; its storage change finishes at its next unlock"));
            }
            // Locked, nothing of it stays here but its folder (cl-984): not its id, code or keys, so its KEY-N is unknown here (cl-987).
            if !converting {
                api.calcifer.store().project_locked(&keybag.info().project_id, root).map_err(|e| error("vault_index", e.to_string()))?;
            }
        }
    }
    Ok(())
}

/// A session whose folder is gone, removed from outside (cl-1124: on Windows its view dies with its backing): it ends at once, said
/// once, nothing of it left open here, and its view let go on a thread of its own (a provider under a removed backing never ends on
/// its own; `Mounted`'s drop is bounded). Whether it ended one. Under the project's vault operation.
fn end_if_gone(api: &Arc<Api>, root: &FsPath) -> bool {
    // Gone only when both its keys and its encrypted data are reported missing: a keybag alone missing leaves its open session
    // holding the only copy of the keys, and an unreadable folder is not a gone one.
    let missing = |name: &str| fs::symlink_metadata(root.join(name)).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    if !(missing(KEYBAG_FILE) && missing(BACKING)) {
        return false;
    }
    let Some(session) = SESSIONS.lock().expect("vault sessions").remove(root) else { return false };
    tracing::warn!("{}: its folder is gone, so its encrypted view ended", root.display());
    let id = session.lock().expect("vault session").secret.project_id().to_owned();
    for project in api.projects.now_open().iter().filter(|project| project.root == root) {
        api.projects.open.lock().expect("projects lock").remove(&project.id);
        crate::events::project_closed(&project.id);
    }
    // Its index row and local's records of it go at once, as for any project whose folder is gone (cl-1031).
    if let Err(e) = super::let_go(api, std::slice::from_ref(&id)) {
        tracing::warn!("{}: a project whose folder is gone stays in the index until the next start: {e}", root.display());
    }
    LOCK_RETRIES.lock().expect("lock retries").remove(root);
    DUE.lock().expect("vault deadlines").remove(root);
    ROOTS.lock().expect("vault roots").retain(|_, logical| logical != root);
    crate::events::project_vault(&id, "locked");
    CLOSED.notify_waiters();
    let _ = std::thread::Builder::new().name("vault-let-go".into()).spawn(move || drop(session));
    true
}

pub(crate) fn close_all(api: &Arc<Api>) -> Result<(), Error> {
    let roots: Vec<_> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
    for root in roots {
        let op = operation(&root);
        let _operation = op.lock().expect("vault operation");
        if end_if_gone(api, &root) { continue; }
        if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
        drain_saves(api, &root, || {})?;
        let transition = exclusive(&root)?;
        close(api, &root, &transition)?;
    }
    Ok(())
}

/// The vault clock: each open vault's idle and maximum clocks and its mount, looked at on its next deadline or a stir, never each
/// second. With no vault open it waits for one.
pub(crate) fn start(api: &Arc<Api>) {
    // A run created or settling holds or frees its project's vault.
    let mut runs = api.calcifer.subscribe();
    tokio::spawn(async move {
        loop {
            match runs.recv().await {
                Ok((_, api::calcifer::Event::Created { .. } | api::calcifer::Event::InProgress { .. } | api::calcifer::Event::Completed { .. } | api::calcifer::Event::Failed { .. } | api::calcifer::Event::Canceled { .. }))
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => stir(),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    let runtime = tokio::runtime::Handle::current();
    let api = Arc::downgrade(api);
    let clock = std::thread::Builder::new().name("vault-clock".into()).spawn(move || {
        let _runtime = runtime.enter();
        loop {
            let Some(api) = api.upgrade() else { return };
            let roots: Vec<PathBuf> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
            let next = roots.into_iter().filter_map(|root| look(&api, &runtime, root)).min();
            drop(api);
            wait(next);
        }
    });
    if let Err(e) = clock { tracing::warn!("no vault clock, so no vault locks itself: {e}"); }
}

/// Until `at` passes or a stir. With no alarm, each second at most, so a vault still locks at its deadline.
fn wait(at: Option<SystemTime>) {
    let Some(clock) = &*CLOCK else {
        return std::thread::sleep(at.map_or(AGAIN, |at| at.duration_since(SystemTime::now()).unwrap_or_default().min(AGAIN)));
    };
    if let Err(e) = clock.wait(at) {
        tracing::warn!("the vault clock's alarm failed, so it looks again in a second: {e}");
        std::thread::sleep(AGAIN);
    }
}

/// How long an explicit lock, its view revoked, waits for what still holds its project to finish before it refuses.
const LOCK_DRAIN: Duration = Duration::from_secs(10);
/// How long past an idle or maximum deadline a lock that failed waits out its retry backoff: after it, each look tries again, as the
/// deadline is a security one.
const SAVE_GRACE: Duration = Duration::from_secs(30);
/// When each open vault was first seen past its deadline and not yet locked.
static DUE: LazyLock<Mutex<BTreeMap<PathBuf, std::time::Instant>>> = LazyLock::new(Mutex::default);

/// When the clock last asked whether the machine of runs holding a vault past its deadline answers (cl-1064), and how often it asks.
static ASKED: LazyLock<Mutex<BTreeMap<PathBuf, Instant>>> = LazyLock::new(Mutex::default);
const ASKED_AGAIN: Duration = Duration::from_secs(10);

/// Whether `root`'s runs on `local` hold it while local doesn't answer (cl-1064).
fn unanswered(api: &Arc<Api>, root: &FsPath) -> bool {
    let Some(id) = project_id(root).or_else(|| plain_project_id(root)) else { return false };
    encrypted(root)
        && api.calcifer.unsettled_on(&id, "local").unwrap_or(false)
        && !tokio::runtime::Handle::current().block_on(api.machines.answers("local"))
}

/// Vaults whose lock is going on a thread of its own.
static LOCKING: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

/// One open vault's clocks and mount: when to look at it again (none: on a stir alone). Past its deadline, its lock begins on a thread
/// of its own, as one slow close must not delay another vault's deadline.
fn look(api: &Arc<Api>, runtime: &tokio::runtime::Handle, root: PathBuf) -> Option<SystemTime> {
    let again = || Some(SystemTime::now() + AGAIN);
    if LOCKING.lock().expect("vault locks").contains(&root) { return again(); }
    let op = operation(&root);
    let Ok(operation) = op.try_lock() else { say(&root, "operation", || "its clocks wait for a vault operation".into()); return again(); };
    // Its folder gone: it ends now, not at its deadline (cl-1124).
    if end_if_gone(api, &root) { return None; }
    let work = working(api, &root);
    let (needs, deadline, past) = held(&root).map_or((false, None, false), |session| {
        let mut session = session.lock().expect("vault session");
        if session.mount.reconcile().is_err() { mark_closing(&mut session); }
        // Work going holds it open, its maximum included: the clocks go on when the last of it ends. Only an explicit lock ends
        // it (owner 11-11: "runs count as activity"). A run its machine hasn't begun holds the idle clock alone (cl-1082).
        session.timer.hold(work.map_or(Hold::Nothing, |work| work.held));
        if let Some(work) = work.filter(|_| !session.closing) {
            session.active_ms = super::now_ms();
            if session.idle > 0 { say(&root, "held", || format!("its idle clock is held by {}", work.why)); }
        }
        (lapsed(&session), session.timer.deadline(), session.timer.held_past_limit())
    });
    drop(operation);
    // Past its idle limit or maximum, held only by its runs: its lock waits on their machine, and one that doesn't answer never keeps
    // the vault open (cl-1064). Asked at most every `ASKED_AGAIN` meanwhile, never otherwise.
    if !needs && past && work.as_ref().is_some_and(|work| matches!(work.why, "runs not settled" | "a run not yet settled")) {
        let asked = ASKED.lock().expect("vault asks").get(&root).is_some_and(|at| at.elapsed() < ASKED_AGAIN);
        if !asked {
            ASKED.lock().expect("vault asks").insert(root.clone(), Instant::now());
            LOCKING.lock().expect("vault locks").insert(root.clone());
            let api = api.clone();
            let root = root.clone();
            runtime.spawn_blocking(move || {
                if unanswered(&api, &root) {
                    if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
                    lock_lapsed(&api, &root, Instant::now(), false);
                }
                LOCKING.lock().expect("vault locks").remove(&root);
                ASKED.lock().expect("vault asks").remove(&root);
                stir();
            });
        }
        return Some(SystemTime::now() + ASKED_AGAIN);
    }
    if !needs {
        DUE.lock().expect("vault deadlines").remove(&root);
        return match work { Some(Work { told: false, .. }) => again(), _ => deadline };
    }
    let due = *DUE.lock().expect("vault deadlines").entry(root.clone()).or_insert_with(Instant::now);
    let forced = due.elapsed() >= SAVE_GRACE;
    let retry = LOCK_RETRIES.lock().expect("lock retries").get(&root).map(|retry| retry.at).filter(|at| Instant::now() < *at);
    if let Some(at) = retry.filter(|_| !forced) { return Some(SystemTime::now() + at.saturating_duration_since(Instant::now())); }
    LOCKING.lock().expect("vault locks").insert(root.clone());
    let api = api.clone();
    runtime.spawn_blocking(move || {
        lock_lapsed(&api, &root);
        LOCKING.lock().expect("vault locks").remove(&root);
    });
    // Looked at again while it stays: a busy view or an unsaved page puts its lock off.
    again()
}

/// A vault past its deadline locks once what holds it has ended or drained. Its pages aren't asked to send what they typed: a lapsed
/// view takes no write.
fn lock_lapsed(api: &Arc<Api>, root: &FsPath) {
    let op = operation(root);
    let Ok(_operation) = op.try_lock() else { say(root, "operation", || "its lock waits for a vault operation".into()); return; };
    let still_needs = held(root).is_some_and(|session| {
        let session = session.lock().expect("vault session");
        lapsed(&session)
    });
    if !still_needs { return; }
    // A run its machine hasn't begun doesn't keep it open past its deadline (cl-1082): it ends before anything else, before a queued
    // run can begin. An explicit lock (closing) has ended the project's runs its own way.
    let clocked = held(root).is_some_and(|session| { let session = session.lock().expect("vault session"); !session.closing && expired(&session) });
    if let Some(id) = project_id(root).filter(|_| clocked) {
        if let Err(failure) = api.calcifer.end_unstarted(&id).map_err(|_| error("run_cancel", "the project's runs not begun could not be ended")) {
            idle_lock_failed(root, &failure);
            return;
        }
    }
    // No page's save is waited for: the view has lapsed, so a page can't save into it, and a wait only lets queued work begin past the
    // deadline (cl-1082 review). Its pages find it locked.
    // What its runs still owe lands first, judged by progress (cl-1060); then the project's turn, and `close` revokes nothing while
    // a writer is still active.
    if let Err(failure) = drain_saves(api, root, || {}) { idle_lock_failed(root, &failure); return; }
    let Ok(transition) = exclusive(root) else {
        say(root, "gate", || "locking waits for a request still holding the project".into());
        return;
    };
    match close(api, root, &transition) {
        Ok(_) => { DUE.lock().expect("vault deadlines").remove(root); }
        Err(failure) => idle_lock_failed(root, &failure),
    }
}

pub(super) async fn read(State(api): State<Arc<Api>>, Path(named): Path<String>) -> Result<Response, Refusal> {
    let root = location(&api, &named).map_err(Refusal)?;
    let _access = access(&root).read_owned().await;
    Ok(private(axum::Json(status(&root)).into_response()))
}

pub(super) async fn write(State(api): State<Arc<Api>>, Path(named): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let mut ask: VaultAsk = parse(&body?)?;
    let root = location(&api, &named).map_err(Refusal)?;
    let changed = tokio::task::block_in_place(|| operate(&api, &root, &mut ask, &mut Progress::quiet())).map_err(Refusal)?;
    Ok(private(axum::Json(changed).into_response()))
}

pub(super) fn operate(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk, progress: &mut Progress) -> Result<VaultChanged, Error> {
    // A page's activity only restarts the idle clock, under the session's own lock: it never waits for or refuses on the project's
    // gate, which any request in flight holds (that request restarted the clock itself).
    if ask.action == "activity" {
        if let Some(session) = held(root) {
            let mut session = session.lock().expect("vault session");
            if !session.closing && !expired(&session) && session.timer.activity() {
                session.active_ms = super::now_ms();
                say(root, "activity op", || "its idle clock restarts on a page's activity".into());
            }
        }
        return Ok(VaultChanged { vault: status(root), recovery_secret: None, ended: vec![], skipped_files: vec![], unsaved: vec![], deferred: vec![], owed: None });
    }
    if ask.action == "lock" && !encrypted(root) {
        return Err(Error::new(Kind::Usage, "project_plain", "unencrypted projects cannot be locked"));
    }
    progress.phase("waiting");
    // What its pages typed goes in first, while edits are still admitted.
    if matches!(ask.action.as_str(), "lock" | "encrypt" | "decrypt" | "rotate") { save_first(root, LOCK_DRAIN); }
    let op = operation(&root); let _guard = op.lock().expect("vault operation");
    // Its folder gone (removed from outside, cl-1124): nothing is left to lock or change; its view ends now.
    if end_if_gone(api, root) || (ask.action == "lock" && held(root).is_none() && !root.exists()) {
        return Err(Error::new(Kind::NotFound, "project_gone", "Its folder is gone, so its encrypted view ended: nothing of it stays unlocked."));
    }
    let (mut ended, mut cut, mut deferred) = (vec![], vec![], vec![]);
    if ask.action == "lock" {
        // A lock that found the project busy already revoked its view: this one finishes it, and admits no receipt into
        // a view that can no longer be opened.
        let revoked = held(root).is_some_and(|session| lapsed(&session.lock().expect("vault session")));
        // A read that holds it a moment is no reason to refuse: the lock waits for it, as it does below for what ends.
        let busy = match held(root).filter(|_| !revoked).map(|_| exclusive_within(api, root, LOCK_DRAIN)) {
            Some(Ok(transition)) => {
                let id = project_id(root).ok_or_else(|| error("vault_identity", "the project has no verified identity"))?;
                let store = api.projects.transition_store(api, &id, &transition)?;
                progress.admit_receipt(&store)?;
                None
            }
            Some(Err(busy)) => Some(busy),
            None => None,
        };
        if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
        // An explicit lock ends the project's work first, so nothing goes on billing behind a locked vault.
        ended = end_work(&api, &root)?;
        // What the project's runs still owe lands before the machine's side closes, judged by progress (cl-1060).
        (cut, deferred) = drain_saves(api, root, || progress.phase("saving its outputs first"))?;
        // A request still holds the project: its view is revoked, and the sweeper locks once it ends.
        if let Some(busy) = busy {
            if ended.is_empty() { return Err(busy); }
            progress.readable = readable_token(root);
            return Ok(VaultChanged { vault: status(&root), recovery_secret: None, ended, skipped_files: vec![], unsaved: vec![], deferred: vec![], owed: None });
        }
    }
    // A lock waits for the project as the drain above does: for the requests let in before its view was revoked (each now
    // refused), and for what it ended, still settling (its runs' last writes hold the project a moment), else it answers
    // locking with what it ended, and the sweeper locks once it has settled.
    // A decrypt reads the view it converts after its own close revoked it (a retried close revokes it first): the view must be
    // open when it's asked.
    if ask.action == "decrypt" && held(root).is_some() { view(root)?; }
    let settling = !ended.is_empty();
    // A storage change closes the machine's side as a lock does: what the project's runs still owe lands first (cl-1060).
    if matches!(ask.action.as_str(), "encrypt" | "decrypt" | "rotate") {
        let (more_cut, more_deferred) = drain_saves(api, root, || progress.phase("saving its outputs first"))?;
        cut.extend(more_cut);
        deferred.extend(more_deferred);
    }
    let gate = if settling || matches!(ask.action.as_str(), "lock" | "decrypt") { exclusive_within(api, &root, LOCK_DRAIN) } else { exclusive(&root) };
    let mut transition = match gate {
        Ok(transition) => transition,
        Err(busy) if settling && busy.code == "project_busy" => {
            progress.readable = readable_token(root);
            return Ok(VaultChanged { vault: status(&root), recovery_secret: None, ended, skipped_files: vec![], unsaved: vec![], deferred: vec![], owed: None });
        }
        Err(error) => return Err(error),
    };
    progress.phase("preparing");
    let draining = std::time::Instant::now() + LOCK_DRAIN;
    let (drains, decrypting) = (matches!(ask.action.as_str(), "lock" | "decrypt"), ask.action == "decrypt");
    let failed = |error: Error| if decrypting { decrypt_failed(root, error) } else { error };
    let mut changed = loop {
        match change(&api, &root, ask, progress, &transition) {
            // What it ended is still settling: the session is closing, and the sweeper locks it once it has.
            Err(busy) if !ended.is_empty() && busy.code == "project_busy" => break VaultChanged { vault: status(&root), recovery_secret: None, ended: vec![], skipped_files: vec![], unsaved: vec![], deferred: vec![], owed: None },
            // Its view is revoked, so nothing new opens it: what still holds it (a request, a journal read, a measurement) is
            // finishing. A lock or a decrypt waits for that a moment rather than refuse; its pages sent what they typed already.
            Err(busy) if drains && busy.code == "project_busy" && std::time::Instant::now() < draining => {
                say(root, "drain", || format!("its {} waits: {}", ask.action, busy.message));
                drop(transition);
                // What refused it says nothing when it ends: a moment, then the project again, queued.
                std::thread::sleep(Duration::from_millis(200));
                transition = exclusive_within(api, &root, draining.saturating_duration_since(std::time::Instant::now())).map_err(failed)?;
            }
            changed => break changed.map_err(failed)?,
        }
    };
    changed.ended = ended;
    // Capture this exact mount's revocation token while the operation still
    // holds the transition guard, so a later lock cannot revive its reply.
    progress.readable = readable_token(root);
    (changed.unsaved, changed.deferred) = (cut, deferred);
    Ok(changed)
}

pub(super) fn private(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// Refuse missing providers/drivers before creating a new project's directory or reserved storage.
pub(super) fn preflight_creation(api: &Api) -> Result<Provider, Error> {
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(unusable_for_creation)?;
    engine.check_driver().map_err(unusable_for_creation)?;
    Ok(engine)
}

/// Initialize the first database through encrypted storage. There is no plaintext source to convert or preserve.
pub(super) fn create_new(api: &Api, root: &FsPath, id: &str, name: &str, written: super::written::Written, passphrase: SecretString, engine: &Provider) -> Result<String, Error> {
    let operation = operation(root);
    let _operation = operation.lock().expect("vault operation");
    let _access = exclusive(root)?;
    let took = crate::took::Took::new("vault keys");
    let created = Keybag::create(id, passphrase).map_err(key)?;
    drop(took);
    let recovery = created.recovery_secret.expose().to_owned();
    let outcome = (|| {
        let took = crate::took::Took::new("vault filesystem");
        created.secret.with_bytes(|bytes| engine.create(&root.join(BACKING), bytes)).map_err(mounting)?;
        drop(took);
        // Persist the wrapping slots before writing any project state: the user's known passphrase
        // can reopen this backing even if setup or delivery of the recovery response is interrupted.
        created.keybag.save(&root.join(KEYBAG_FILE)).map_err(key)?;
        let runtime = project_runtime(root)?;
        let view = project_mount::view_path(&runtime).map_err(provider)?;
        let took = crate::took::Took::new("vault mount");
        let mut target = created.secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes,
            MountOptions { idle_timeout: None, read_only: false })).map_err(mounting)?;
        drop(took);
        let took = crate::took::Took::new("vault project database");
        let initialized = super::new_database(target.view(), id, name, written);
        drop(took);
        target.try_unmount().map_err(provider)?;
        retire_runtime(&runtime)?;
        initialized?;
        platform::durable::sync_dir(root).map_err(provider)?;
        let ask = VaultAsk { action: "encrypt".into(), passphrase: None, recovery_secret: None,
            idle_timeout_s: None, max_duration_s: None };
        mount(api, root, created.keybag, created.secret, &ask)
    })();
    outcome.map_err(|why| {
        let next = if root.join(KEYBAG_FILE).is_file() { "Keep its backing, keybag and passphrase. Setup is unfinished; inspect the retained storage before retrying in a new empty directory." }
            else { "No project state was written. The unfinished backing is preserved; inspect it before retrying in a new empty directory." };
        error("vault_creation", format!("Encrypted project setup at {} could not finish: {}. {next}", root.display(), why.message))
    })?;
    Ok(recovery)
}

fn change(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk, progress: &mut Progress, transition: &ProjectTransition) -> Result<VaultChanged, Error> {
    recover_under_guards(root)?;
    require_transition_ready(root, &ask.action)?;
    if ask.action == "encrypt" && encrypted(root) {
        return Err(Error::new(Kind::Conflict, "project_encrypted", "the project is already encrypted"));
    }
    if matches!(ask.action.as_str(), "decrypt" | "rotate" | "passphrase" | "recovery") && !encrypted(root) {
        return Err(Error::new(Kind::Usage, "project_plain", "this project is not encrypted"));
    }
    // Each needs its unlocked view. Locked, the index holds nothing of it to look up (cl-984): it is refused as locked first.
    if matches!(ask.action.as_str(), "decrypt" | "rotate" | "passphrase" | "recovery") && held(root).is_none() {
        return Err(locked(root));
    }
    // Once an operation: a decrypt's retry finds its view revoked, which opens no project to admit a receipt into.
    if !progress.prepared && matches!(ask.action.as_str(), "encrypt" | "decrypt" | "rotate" | "passphrase" | "recovery") {
        let id = project_id(root).or_else(|| plain_project_id(root)).ok_or_else(|| error("vault_identity", "the project has no verified identity"))?;
        let store = api.projects.transition_store(api, &id, transition)?;
        progress.admit_receipt(&store)?;
        progress.prepared = true;
    }
    let mut recovery_secret = None;
    match ask.action.as_str() {
        "encrypt" => {
            if encrypted(root) { return Err(Error::new(Kind::Conflict, "project_encrypted", "the project is already encrypted")); }
            let project = api.projects.open(api, root)?;
            let id = project.id.clone();
            // Keep the folder registered until convert closes its connections. Its
            // ordinary final drop closes SQLite asynchronously, which can retain
            // Windows file handles across the source-directory rename.
            let generation = plain_generation_for_encrypt(root, &id)?;
            let created = Keybag::create_generation(&id, generation, new_passphrase(ask)?).map_err(key)?;
            recovery_secret = Some(created.recovery_secret.expose().to_owned());
            convert(api, root, created.keybag, created.secret, ask, progress, transition)?;
            drop(project);
        }
        "unlock" => {
            progress.phase("opening");
            if !encrypted(root) { return Err(Error::new(Kind::Usage, "project_plain", "this project is not encrypted")); }
            let keybag = Keybag::load(&root.join(KEYBAG_FILE)).map_err(key)?;
            let took = crate::took::Took::new("vault keys");
            let secret = keybag.unlock(credential(ask)?).map_err(key)?;
            drop(took);
            if let Some(session) = held(root) {
                let mut session = session.lock().expect("vault session");
                if session.mount.is_mounted().map_err(provider)? {
                    if session.keybag.info().key_id != keybag.info().key_id { return Err(error("vault_key", "the active keybag was replaced; close this mount before recovery")); }
                    session.secret = secret;
                    session.keybag = keybag;
                    session.closing = false;
                    revoke(&session.readable);
                    session.readable = Arc::new(AtomicBool::new(true));
                    session.began_ms = super::now_ms(); session.active_ms = session.began_ms;
                    session.idle = ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE); session.maximum = ask.max_duration_s.unwrap_or(0);
                    session.timer = timer(session.idle, session.maximum);
                    stir();
                } else {
                    session.mount.try_unmount().map_err(provider)?;
                    drop(session);
                    SESSIONS.lock().expect("vault sessions").remove(root);
                    mount(api, root, keybag, secret, ask)?;
                }
            } else { mount(api, root, keybag, secret, ask)?; }
            // Verify the durable cutover before opening/migrating its SQLite
            // contents, whose legitimate writes would change the copied hash.
            if conversion(root)?.is_some_and(|record| record.internal && record.plain.is_none()) {
                progress.phase("finishing_storage_change");
                finish_conversion(root)?;
            }
            let id = project_id(root).ok_or_else(|| error("vault_identity", "the unlocked project has no verified identity"))?;
            // Open it at this root first: a moved folder is gone from the index by its old path.
            let opened = api.projects.open(api, root)?;
            if opened.id != id { return Err(error("vault_identity", "the unlocked project does not match its keybag")); }
            let store = api.projects.transition_store(api, &id, transition)?;
            drop(opened);
            progress.admit_receipt(&store)?;
            if receipt_path(root).is_file() { finish_conversion(root)?; }
            let _ = fs::remove_file(root.join(JOURNAL_CLEANUP_FILE));
            if let Some(session) = held(root) { crate::events::project_vault(session.lock().expect("vault session").secret.project_id(), "unlocked"); }
            progress.finish_receipt(&store, "completed")?;
        }
        "lock" => {
            if !encrypted(root) { return Err(Error::new(Kind::Usage, "project_plain", "unencrypted projects cannot be locked")); }
            // Locking a locked project is a no-op, after retiring what a dead Calcifer's views left.
            if held(root).is_some() { close(api, root, transition)?; } else { retire_orphaned_views(root)?; }
        }
        "passphrase" | "recovery" => {
            let session = held(root).ok_or_else(|| locked(root))?;
            let mut session = session.lock().expect("vault session");
            if session.closing || expired(&session) { return Err(locked(root)); }
            let mut keybag = session.keybag.clone();
            if ask.action == "passphrase" { keybag.change_passphrase(&session.secret, new_passphrase(ask)?).map_err(key)?; }
            else { recovery_secret = Some(keybag.replace_recovery(&session.secret).map_err(key)?.expose().to_owned()); }
            keybag.save(&root.join(KEYBAG_FILE)).map_err(key)?;
            session.keybag = keybag;
            if session.timer.activity() { session.active_ms = super::now_ms(); }
        }
        "decrypt" => decrypt(api, root, progress, transition)?,
        "rotate" => {
            let session = held(root).ok_or_else(|| locked(root))?;
            let created = session.lock().expect("vault session").keybag.new_generation(new_passphrase(ask)?).map_err(key)?;
            recovery_secret = Some(created.recovery_secret.expose().to_owned());
            convert(api, root, created.keybag, created.secret, ask, progress, transition)?;
        }
        _ => return Err(Error::new(Kind::Usage, "vault_action", "unknown local vault action")),
    }
    if matches!(ask.action.as_str(), "encrypt" | "decrypt" | "rotate" | "passphrase" | "recovery") {
        let id = project_id(root).or_else(|| plain_project_id(root)).ok_or_else(|| error("vault_identity", "the changed project has no verified identity"))?;
        // A conversion replaced the view its runs were owned under: the next open owns them again.
        if matches!(ask.action.as_str(), "encrypt" | "decrypt" | "rotate") { api.calcifer.retire_project(&id); }
        let store = api.projects.transition_store(api, &id, transition)?;
        progress.finish_receipt(&store, "completed")?;
    }
    Ok(VaultChanged { vault: status(root), recovery_secret, ended: vec![], skipped_files: progress.skipped(), unsaved: vec![], deferred: vec![], owed: owed_said(root) })
}

/// What holds an unlocked vault open: why, whether its end stirs the vault clock (one that doesn't is looked at again shortly), and how
/// (cl-1082): work going holds both clocks; a run its machine hasn't begun, its driver or a client following it, only the idle one.
#[derive(Clone, Copy)]
struct Work { why: &'static str, told: bool, held: Hold }

/// What holds an unlocked vault open now, if anything: a request, a private process, a run saving its outputs, a render, cut or file
/// move, a reader, a run not settled or its driver, a client following a run, a run's submission.
fn working(api: &Arc<Api>, root: &FsPath) -> Option<Work> {
    let going = |why, told| Some(Work { why, told, held: Hold::Working });
    let waiting = Some(Work { why: "a run its machine hasn't begun", told: true, held: Hold::Waiting });
    if api.projects.active_requests(root) { return going("a project request being answered", true); }
    if crate::local_code::private_process_active(root) { return going("a private process", true); }
    let following = FOLLOWING.lock().expect("vault followers").contains_key(root);
    let projects = api.projects.now_open();
    let submitting = api.projects.submitting(root);
    let Some(project) = projects.iter().find(|project| project.root == root) else {
        return if following { going("a client following a run", true) } else if submitting { waiting } else { None };
    };
    if api.calcifer.exporting_into(&project.id).is_some() { return going("a run saving its outputs", false); }
    if *project.walking.lock().expect("walking lock") { return going("a render", true); }
    if project.moving.try_lock().is_err() || project.cutting.try_lock().is_err() { return going("a move or cut", false); }
    let Ok(inspection) = inspection(root) else { return going("a storage change", false) };
    let Ok(store) = api.projects.inspection_store(api, &project.id, &inspection) else { return going("a journal it cannot read", false) };
    // The inspection authority can read already-accepted work even if an idle
    // deadline just expired. Public viewing admission is deliberately stricter.
    let (Ok(sent), Ok(here), Ok(unstarted)) = (store.unsettled(), store.unsettled_here(), store.unsettled_unstarted()) else {
        return going("a journal it cannot read", false);
    };
    // A run its machine began holds both clocks (owner 11-11: "runs count as activity"), and so does a driver whose run did: one
    // settling or handing on its outputs (cl-1060).
    if !here.is_empty() || sent.len() > unstarted.len() { return going("runs not settled", true); }
    let driven = api.calcifer.driven_into(&project.id);
    if driven.iter().any(|id| !unstarted.contains(id)) { return going("a run not yet settled", false); }
    if !unstarted.is_empty() || !driven.is_empty() || following || submitting { waiting } else { None }
}

/// A sweeper's reason, said at most every 10 seconds per project: a test's diagnostics name what kept a vault open.
fn say(root: &FsPath, what: &'static str, said: impl FnOnce() -> String) {
    static SAID: LazyLock<Mutex<BTreeMap<(PathBuf, &'static str), std::time::Instant>>> = LazyLock::new(Mutex::default);
    let now = std::time::Instant::now();
    let mut times = SAID.lock().expect("vault sayings");
    if times.get(&(root.to_owned(), what)).is_some_and(|at| now.duration_since(*at) < Duration::from_secs(10)) { return; }
    times.insert((root.to_owned(), what), now);
    tracing::warn!("{}: {}", root.display(), said());
}

/// Request cancellation in the owning journal before the exclusive close gate,
/// so existing drivers can still settle their bounded work transactions.
fn end_work(api: &Arc<Api>, root: &FsPath) -> Result<Vec<u64>, Error> {
    crate::local_code::cancel_private_processes(root);
    let mut ended = BTreeSet::new();
    let all = [super::format::ALL.to_owned()];
    for project in api.projects.now_open().iter().filter(|project| project.root == root) {
        let cancelled = super::cancelled(api, project, &all, &[], &|name| format!("{name}: the project was locked")).map_err(|refused| refused.0)?;
        ended.extend(cancelled.canceled);
    }
    if let Some(id) = project_id(root).or_else(|| plain_project_id(root)) {
        ended.extend(api.calcifer.cancel_for_lock(&id).map_err(|_| error("run_cancel", "the project's work could not be cancelled"))?);
    }
    Ok(ended.into_iter().collect())
}

/// Where a project's folder goes (cl-1097): renamed where it is, or to the OS trash.
pub(super) enum Relocation { Rename(PathBuf), Trash }

/// Project `root`'s folder moved as `to` says, once nothing of it is open (cl-1097), as a person would move it by hand: what the index
/// keeps of it follows (a rename) or goes (the trash, as a folder found gone does). An unlocked encrypted project locks first, which
/// needs no secret: its mounted view is bound to the folder. The trash ends its runs first; a rename ends none, and waits on no render:
/// its results land by the folder's path. Where the trash keeps it, when the OS says.
pub(super) fn relocate(api: &Arc<Api>, root: &FsPath, to: &Relocation) -> Result<Option<PathBuf>, Error> {
    let busy = |why: &str| Error::new(Kind::Conflict, "project_busy", format!("It's {why}. Try again once that's done."));
    if let Relocation::Rename(_) = to {
        let projects = api.projects.now_open();
        if let Some(project) = projects.iter().find(|project| project.root == root) {
            if *project.walking.lock().expect("walking lock") { return Err(busy("rendering")); }
            if api.calcifer.exporting_into(&project.id).is_some() || api.calcifer.writing_into(&project.id).is_some() { return Err(busy("saving a run's results")); }
        }
    }
    if held(root).is_some() {
        if let Relocation::Rename(_) = to { if let Some(work) = working(api, root).filter(|work| work.told) { return Err(busy(work.why)); } }
        let mut ask = VaultAsk { action: "lock".into(), passphrase: None, recovery_secret: None, idle_timeout_s: None, max_duration_s: None };
        operate(api, root, &mut ask, &mut Progress::quiet())?;
        let due = Instant::now() + LOCK_DRAIN;
        while held(root).is_some() {
            if Instant::now() >= due { return Err(busy("still locking")); }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let op = operation(root);
    let _guard = op.lock().expect("vault operation");
    // From here no new request opens it (`view` answers `vault_busy`), and the index keeps it while it moves: a listing never takes the
    // old path for a folder gone.
    if matches!(to, Relocation::Trash) && !encrypted(root) { end_work(api, root)?; }
    CONVERTING.lock().expect("vault conversions").insert(root.to_owned());
    let _converting = Converting(root.to_owned());
    let due = Instant::now() + LOCK_DRAIN;
    let transition = loop {
        let transition = exclusive_within(api, root, due.saturating_duration_since(Instant::now()))?;
        match forget_with(api, root, &transition, false) {
            Ok(()) => break transition,
            Err(held) if held.code == "project_busy" && Instant::now() < due => {
                drop(transition);
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(held) => return Err(held),
        }
    };
    let store = api.calcifer.store();
    let moved = match to {
        Relocation::Rename(next) => {
            // Its machine side closes as a lock closes it: nothing there holds the old place (an encrypted one's closed as it locked).
            if !encrypted(root) {
                tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?;
                crate::runs::files::release_private(root)?;
                if let Some(id) = plain_project_id(root) { api.calcifer.retire_project(&id); }
            }
            fs::rename(root, next).map_err(|e| Error::new(Kind::Conflict, "project_rename", format!("Couldn't rename {}: {e}", root.display())))?;
            store.project_moved(root, next).map_err(|e| error("vault_index", e.to_string()))?;
            if !encrypted(next) { crate::private_io::moved(root, next)?; }
            if let Some(id) = plain_project_id(next).or_else(|| project_id(next)) {
                if let Err(e) = register_plain_root(next, &id) { tracing::warn!("{}: {}", next.display(), e.message); }
            }
            None
        }
        Relocation::Trash => platform::trash::put(root).map_err(|e| Error::new(Kind::Unavailable, "project_trash", format!("Couldn't move {} to the trash: {e}", root.display())))?,
    };
    drop(transition);
    Ok(moved)
}

fn require_transition_ready(root: &FsPath, action: &str) -> Result<(), Error> {
    if matches!(action, "encrypt" | "rotate" | "decrypt") && conversion(root)?.is_some() {
        return Err(Error::new(Kind::Conflict, "vault_cleanup_pending", "finish the previous storage conversion before starting another")
            .remedy("unlock the project again to complete its pending journal and source cleanup")
            .next("cozy vault unlock"));
    }
    Ok(())
}

struct Converting(PathBuf);
impl Drop for Converting { fn drop(&mut self) { CONVERTING.lock().expect("vault conversions").remove(&self.0); } }

/// Only paths created by this conversion are ever recovered or retired. This receipt is durable
/// before either rename, so losing Calcifer between them cannot lose the registered project path.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Conversion {
    root: PathBuf, staged: PathBuf, saved: PathBuf, key_id: String,
    #[serde(default = "remove_legacy_source")]
    remove_source: bool,
    #[serde(default)]
    source_id: Option<[u64; 2]>,
    #[serde(default)]
    root_id: Option<[u64; 2]>,
    #[serde(default)]
    staged_id: Option<[u64; 2]>,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    internal: bool,
    #[serde(default)]
    source_key: Option<String>,
    #[serde(default)]
    backing_id: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plain: Option<PlainConversion>,
}

fn remove_legacy_source() -> bool { true }

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PlainConversion {
    project_id: String,
    generation: u64,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    source_id: [u64; 2],
    #[serde(default)]
    staged_id: [u64; 2],
    #[serde(default)]
    cleanup_started: bool,
}

fn directory_id(path: &FsPath) -> Result<[u64; 2], Error> {
    let metadata = fs::symlink_metadata(path).map_err(io)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err(error("vault_recovery", "a conversion directory was replaced")); }
    let id = platform::durable::FileId::at(path).map_err(provider)?;
    Ok([id.dev, id.ino])
}

fn save_conversion(record: &Conversion) -> Result<(), Error> {
    let path = if record.internal { record.root.join(TRANSITION_RECEIPT) } else { legacy_receipt_path(&record.root) };
    platform::durable::replace(&path, &serde_json::to_vec(record).map_err(|e| error("vault_recovery", e))?, 0o600).map_err(provider)
}

fn receipt_path(root: &FsPath) -> PathBuf {
    let contained = root.join(TRANSITION_RECEIPT);
    if contained.exists() { return contained; }
    legacy_receipt_path(root)
}

fn legacy_receipt_path(root: &FsPath) -> PathBuf {
    let name = hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()));
    root.parent().unwrap_or(root).join(format!(".cozy-vault-{name}.transaction.json"))
}

/// An opaque transition receipt is public control metadata; filenames and file
/// hashes used for verification stay in this manifest inside the target cipher.
#[derive(serde::Serialize, serde::Deserialize)]
struct VerifiedSnapshot { files: Vec<(PathBuf, [u8; 32])> }

fn snapshot_files(root: &FsPath, at: &FsPath, files: &mut Vec<(PathBuf, [u8; 32])>) -> Result<(), Error> {
    for entry in fs::read_dir(at).map_err(io)? {
        let entry = entry.map_err(io)?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| error("vault_verify", "a snapshot path escaped its project"))?;
        if relative == FsPath::new(TRANSITION_VERIFY) { continue; }
        if relative.components().count() == 1 && matches!(entry.file_name().to_str(), Some(".fseventsd" | ".Spotlight-V100")) { continue; }
        let kind = entry.file_type().map_err(io)?;
        if kind.is_dir() { snapshot_files(root, &path, files)?; }
        else if kind.is_file() { files.push((relative.to_owned(), hash(&path)?)); }
        else { return Err(error("vault_verify", "a storage snapshot contains an unsupported file")); }
    }
    Ok(())
}

fn save_snapshot(view: &FsPath) -> Result<(), Error> {
    let mut files = Vec::new();
    snapshot_files(view, view, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let bytes = serde_json::to_vec(&VerifiedSnapshot { files }).map_err(|why| error("vault_verify", why))?;
    platform::durable::replace(&view.join(TRANSITION_VERIFY), &bytes, 0o600).map_err(provider)
}

fn verify_snapshot(view: &FsPath) -> Result<(), Error> {
    let bytes = fs::read(view.join(TRANSITION_VERIFY)).map_err(io)?;
    let snapshot: VerifiedSnapshot = serde_json::from_slice(&bytes).map_err(|_| error("vault_verify", "the private storage verification manifest is invalid"))?;
    for (relative, expected) in snapshot.files {
        if relative.is_absolute() || relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
            return Err(error("vault_verify", "a verification path escaped its project"));
        }
        let path = view.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(io)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || hash(&path)? != expected {
            return Err(error("vault_verify", "the replacement project snapshot did not verify"));
        }
    }
    Ok(())
}

fn move_children(source: &FsPath, target: &FsPath, skip: &[PathBuf]) -> Result<(), Error> {
    let mut entries = fs::read_dir(source).map_err(io)?.collect::<Result<Vec<_>, _>>().map_err(io)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let from = entry.path();
        if skip.iter().any(|path| path == &from) { continue; }
        let to = target.join(entry.file_name());
        if exists(&to)? { return Err(error("vault_recovery", "a storage destination already exists; both copies were preserved")); }
        fs::rename(&from, &to).map_err(io)?;
        platform::durable::sync_dir(source).map_err(provider)?;
        platform::durable::sync_dir(target).map_err(provider)?;
    }
    Ok(())
}

fn root_control_paths(record: &Conversion) -> Vec<PathBuf> {
    vec![record.staged.clone(), record.saved.clone(), record.root.join(RUNTIME_DIR), record.root.join(TRANSITION_RECEIPT)]
}

/// Only exact recorded directories may be moved or retired, including recovery after restart.
fn verify_owned_directories(record: &Conversion) -> Result<(), Error> {
    for (path, expected) in [(&record.staged, record.staged_id.or_else(|| record.plain.as_ref().map(|plain| plain.staged_id))),
        (&record.saved, record.source_id.or_else(|| if record.internal { None } else { record.plain.as_ref().map(|plain| plain.source_id) }))] {
        if exists(path)? && expected != Some(directory_id(path)?) {
            return Err(error("vault_recovery", "a recorded storage directory changed or has no verified identity; all copies were preserved"));
        }
    }
    if record.internal && record.root_id != Some(directory_id(&record.root)?) {
        return Err(error("vault_recovery", "the project directory changed during storage conversion; all copies were preserved"));
    }
    Ok(())
}

fn internal_target(record: &Conversion) -> Result<bool, Error> {
    Ok(Keybag::load(&record.root.join(KEYBAG_FILE)).is_ok_and(|bag| bag.info().key_id == record.key_id)
        && record.backing_id.is_some_and(|expected| directory_id(&record.root.join(BACKING)).ok() == Some(expected)))
}

/// Project-contained transitions never rename the project root. A crash can
/// leave members in either owned subdirectory; it cannot strand another project
/// beside the directory the user copies or deletes.
fn recover_internal(record: &Conversion) -> Result<(), Error> {
    verify_owned_directories(record)?;
    if record.plain.is_none() && !record.ready {
        if record.root_id != Some(directory_id(&record.root)?)
            || fs::read_dir(&record.saved).map_err(io)?.next().is_some() {
            return Err(error("vault_recovery", "the original project changed before storage cutover; all copies were preserved"));
        }
        remove_stage(&record.staged)?;
        if record.saved.exists() { fs::remove_dir(&record.saved).map_err(io)?; }
        return clear_conversion(record);
    }
    if let Some(plain) = &record.plain {
        if !plain.ready {
            if !original_plain_source(record, &record.root)? {
                return Err(error("vault_recovery", "the original encrypted source could not be verified; all copies were preserved"));
            }
            remove_stage(&record.staged)?;
            if record.saved.exists() { fs::remove_dir(&record.saved).map_err(io)?; }
            return clear_conversion(record);
        }
        if !record.saved.exists() { fs::create_dir(&record.saved).map_err(io)?; }
        for name in [BACKING, KEYBAG_FILE] {
            let source = record.root.join(name);
            if !exists(&source)? { continue; }
            let target = record.saved.join(name);
            if exists(&target)? { return Err(error("vault_recovery", "both encrypted source copies exist; inspect them before retrying")); }
            fs::rename(source, target).map_err(io)?;
            platform::durable::sync_dir(&record.root).map_err(provider)?;
            platform::durable::sync_dir(&record.saved).map_err(provider)?;
        }
        if record.staged.exists() {
            move_children(&record.staged, &record.root, &[record.staged.join(RUNTIME_DIR)])?;
        }
        if plain_project_id(&record.root).as_deref() != Some(plain.project_id.as_str()) {
            return Err(error("vault_recovery", "the ordinary replacement project identity did not verify"));
        }
        verify_snapshot(&record.root)?;
        match crate::private_io::retained_scope(&record.root, &plain.project_id)? {
            Some(scope) if scope.storage.generation == plain.generation => {},
            Some(_) => return Err(error("vault_recovery", "the plaintext execution generation changed")),
            None => { crate::private_io::descriptor_for_mounted(&record.root, &plain.project_id, plain.generation)?; },
        }
        register_plain_root(&record.root, &plain.project_id)?;
        if record.saved.exists() { fs::remove_dir_all(&record.saved).map_err(io)?; }
        remove_stage(&record.staged)?;
        clear_conversion(record)?;
        let _ = fs::remove_file(record.root.join(TRANSITION_VERIFY));
        return Ok(());
    }

    if internal_target(record)? { return Ok(()); } // Authorized unlock verifies and retires the retained source.
    let target = Keybag::load(&record.staged.join(KEYBAG_FILE))
        .map_err(|_| error("vault_recovery", "the replacement keybag is unavailable; all project copies were preserved"))?;
    if target.info().key_id != record.key_id {
        return Err(error("vault_recovery", "the replacement keybag changed; all copies were preserved"));
    }
    // The backing is moved before its keybag is committed. Put that exact owned
    // backing back in the stage before rolling the untouched source members back.
    if !exists(&record.staged.join(BACKING))? && exists(&record.root.join(BACKING))? {
        if record.backing_id != Some(directory_id(&record.root.join(BACKING))?) {
            return Err(error("vault_recovery", "the replacement backing changed; all copies were preserved"));
        }
        fs::rename(record.root.join(BACKING), record.staged.join(BACKING)).map_err(io)?;
    }
    if record.saved.exists() { move_children(&record.saved, &record.root, &[])?; }
    let restored = match &record.source_key {
        Some(key_id) => Keybag::load(&record.root.join(KEYBAG_FILE)).is_ok_and(|bag| &bag.info().key_id == key_id),
        None => plain_project_id(&record.root).as_deref() == Some(target.info().project_id.as_str()),
    };
    if !restored { return Err(error("vault_recovery", "the original project identity could not be restored; all copies were preserved")); }
    remove_stage(&record.staged)?;
    if record.saved.exists() { fs::remove_dir(&record.saved).map_err(io)?; }
    clear_conversion(record)
}

fn conversion(root: &FsPath) -> Result<Option<Conversion>, Error> {
    let path = receipt_path(root);
    if !exists(&path)? { return Ok(None); }
    if fs::symlink_metadata(&path).map_err(io)?.file_type().is_symlink() { return Err(error("vault_recovery", "conversion receipt cannot be a symlink")); }
    let bytes = fs::read(path).map_err(io)?;
    let mut record: Conversion = serde_json::from_slice(&bytes).map_err(|_| error("vault_recovery", "invalid conversion receipt"))?;
    // RC 0.25's earlier transition receipt put these identities on the outer record.
    if let Some(plain) = &mut record.plain {
        plain.ready |= record.ready;
        if plain.source_id == [0, 0] { plain.source_id = record.source_id.unwrap_or([0, 0]); }
        if plain.staged_id == [0, 0] { plain.staged_id = record.staged_id.unwrap_or([0, 0]); }
    }
    let parent = if record.internal { Some(root) } else { root.parent() };
    if record.root != root || record.staged.parent() != parent || record.saved.parent() != parent
        || !record.staged.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".cozy-vault-stage-"))
        || !record.saved.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".cozy-project-source-")) {
        return Err(error("vault_recovery", "conversion receipt does not describe this project"));
    }
    Ok(Some(record))
}

pub(super) fn recover_conversion(root: &FsPath) -> Result<(), Error> {
    if !exists(&receipt_path(root))? { return Ok(()); }
    let op = operation(root);
    let attempt = op.try_lock();
    if matches!(attempt, Err(std::sync::TryLockError::WouldBlock)) { return Ok(()); }
    let _operation = attempt.expect("vault operation");
    let Ok(_access) = exclusive(root) else { return Ok(()) };
    recover_under_guards(root)
}

/// Called only while this root's operation mutex and exclusive ACCESS are owned.
/// Reloading under those guards prevents a delayed recovery from retiring a newer receipt.
fn recover_under_guards(root: &FsPath) -> Result<(), Error> {
    let Some(record) = conversion(root)? else { return Ok(()); };
    if !CONVERTING.lock().expect("vault conversions").insert(root.to_owned()) {
        return Err(error("vault_busy", "the project's storage transition is already active"));
    }
    let _converting = Converting(root.to_owned());
    if record.internal {
        // No session: any view of this root is a dead Calcifer's.
        if held(root).is_none() { retire_orphaned_views(root)?; }
        return recover_internal(&record);
    }
    verify_owned_directories(&record)?;
    if record.plain.is_some() { return recover_plain(&record); }
    let target = |path: &FsPath| Keybag::load(&path.join(KEYBAG_FILE)).is_ok_and(|bag| bag.info().key_id == record.key_id);
    if target(root) { return Ok(()); } // Unlock completes the private journal transaction and cleanup.
    if root.exists() && !record.saved.exists() && target(&record.staged) {
        // The first rename never happened: the original remains authoritative.
        fs::remove_dir_all(&record.staged).map_err(io)?;
        fs::remove_file(receipt_path(root)).map_err(io)?;
    } else if !root.exists() && record.saved.is_dir() && target(&record.staged) {
        fs::rename(&record.staged, root).map_err(io)?;
    } else { return Err(error("vault_recovery", format!("conversion needs recovery; preserved source: {}", record.saved.display()))); }
    platform::durable::sync_dir(root.parent().ok_or_else(|| error("vault_root", "project has no parent"))?).map_err(provider)
}

fn finish_conversion(root: &FsPath) -> Result<(), Error> {
    let Some(record) = conversion(root)? else { return Ok(()); };
    if record.plain.is_some() { return Err(error("vault_recovery", "a plaintext conversion must finish its own recorded cleanup; retry decrypt")); }
    if Keybag::load(&root.join(KEYBAG_FILE)).map_err(key)?.info().key_id != record.key_id { return Err(error("vault_recovery", "replacement keybag does not match the verified conversion")); }
    verify_owned_directories(&record)?;
    if record.internal {
        let view = work_view(root)?;
        verify_snapshot(&view)?;
    }
    if !record.remove_source && record.saved.exists() {
        return Err(error("vault_cleanup_pending", "an older conversion kept a separate plaintext copy; resolve that copy before completing the storage update"));
    }
    if record.remove_source && record.saved.exists() {
        fs::remove_dir_all(&record.saved).map_err(|e| error("vault_cleanup_pending", format!("replacement verified, but old source cleanup is pending at {}: {e}", record.saved.display())))?;
    }
    if record.internal { remove_stage(&record.staged)?; }
    let receipt = receipt_path(root);
    fs::remove_file(&receipt).map_err(io)?;
    platform::durable::sync_dir(receipt.parent().ok_or_else(|| error("vault_root", "the transition receipt has no parent"))?).map_err(provider)?;
    if record.internal {
        // This private, already-verified manifest is harmless if a crash leaves
        // it behind. Retire the public intent first so recovery cannot require
        // a manifest that was removed just before its receipt.
        let _ = fs::remove_file(work_view(root)?.join(TRANSITION_VERIFY));
    }
    platform::durable::sync_dir(root.parent().ok_or_else(|| error("vault_root", "project has no parent"))?).map_err(provider)
}

/// A killed conversion's target view can outlive its provider as a dead mount inside the
/// stage: detach it first, or removing the stage fails on it.
fn remove_stage(staged: &FsPath) -> Result<(), Error> {
    if !exists(staged)? { return Ok(()); }
    retire_orphaned_views(staged)?;
    fs::remove_dir_all(staged).map_err(io)
}

fn exists(path: &FsPath) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(why) => Err(io(why)),
    }
}

fn original_plain_source(record: &Conversion, path: &FsPath) -> Result<bool, Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if !exists(path)? { return Ok(false); }
    if directory_id(path)? != plain.source_id { return Ok(false); }
    let bag = Keybag::load(&path.join(KEYBAG_FILE)).map_err(key)?.info();
    Ok(bag.key_id == record.key_id && bag.project_id == plain.project_id && bag.generation == plain.generation)
}

fn plain_target(record: &Conversion, path: &FsPath) -> Result<bool, Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if !exists(path)? { return Ok(false); }
    Ok(directory_id(path)? == plain.staged_id && !exists(&path.join(KEYBAG_FILE))?
        && plain_project_id(path).as_deref() == Some(&plain.project_id))
}

fn clear_conversion(record: &Conversion) -> Result<(), Error> {
    let receipt = receipt_path(&record.root);
    fs::remove_file(&receipt).map_err(io)?;
    platform::durable::sync_dir(receipt.parent().ok_or_else(|| error("vault_root", "the transition receipt has no parent"))?).map_err(provider)
}

/// The original encrypted directory remains authoritative until both renames commit.
/// A durable copying intent also owns an incomplete stage, so ordinary failures can
/// immediately remove partial plaintext rather than leaving it until the next launch.
fn recover_plain(record: &Conversion) -> Result<(), Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if plain.generation == 0 || plain.project_id.is_empty() { return Err(error("vault_recovery", "invalid plaintext conversion identity")); }
    if original_plain_source(record, &record.root)? && !exists(&record.saved)? {
        if exists(&record.staged)? {
            if directory_id(&record.staged)? != plain.staged_id { return Err(error("vault_recovery", "the plaintext stage was replaced; preserve it before retrying")); }
            fs::remove_dir_all(&record.staged).map_err(|why| error("vault_cleanup_pending", format!("partial plaintext cleanup is pending at {}: {why}; retry decrypt", record.staged.display())))?;
        }
        return clear_conversion(record);
    }
    if plain.ready && !exists(&record.root)? && original_plain_source(record, &record.saved)? && plain_target(record, &record.staged)? {
        fs::rename(&record.staged, &record.root).map_err(io)?;
        platform::durable::sync_dir(record.root.parent().expect("validated conversion parent")).map_err(provider)?;
    }
    if !plain.ready || !plain_target(record, &record.root)? {
        return Err(error("vault_recovery", format!("plaintext conversion needs recovery; preserved source: {}; stage: {}", record.saved.display(), record.staged.display())));
    }
    finish_plain(record)
}

fn recover_pending_plain(root: &FsPath) -> Result<(), Error> {
    let record = conversion(root)?.filter(|record| record.plain.is_some())
        .ok_or_else(|| error("vault_recovery", "the durable plaintext conversion receipt is unavailable"))?;
    if record.internal { recover_internal(&record) } else { recover_plain(&record) }
}

fn finish_plain(record: &Conversion) -> Result<(), Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if !plain.ready || !plain_target(record, &record.root)? { return Err(error("vault_recovery", "the plaintext replacement did not verify")); }
    if exists(&record.saved)? {
        if directory_id(&record.saved)? != plain.source_id
            || (!plain.cleanup_started && !original_plain_source(record, &record.saved)?) {
            return Err(error("vault_recovery", "the preserved encrypted source was replaced; cleanup refused"));
        }
    }
    let scope = crate::private_io::retained_scope(&record.root, &plain.project_id)?;
    match scope {
        Some(scope) if scope.storage.generation == plain.generation => {},
        Some(_) => return Err(error("vault_recovery", "the plaintext execution generation changed")),
        None => { crate::private_io::descriptor_for_mounted(&record.root, &plain.project_id, plain.generation)?; },
    }
    register_plain_root(&record.root, &plain.project_id)?;
    if exists(&record.saved)? {
        if !plain.cleanup_started {
            let mut cleaning = record.clone();
            cleaning.plain.as_mut().expect("plaintext conversion").cleanup_started = true;
            save_conversion(&cleaning)?;
        }
        if directory_id(&record.saved)? != plain.source_id { return Err(error("vault_recovery", "the preserved source changed before cleanup")); }
        fs::remove_dir_all(&record.saved).map_err(|why| error("vault_cleanup_pending", format!("plaintext replacement verified, but old encrypted source cleanup is pending at {}: {why}; retry decrypt", record.saved.display())))?;
        platform::durable::sync_dir(record.root.parent().expect("validated conversion parent")).map_err(provider)?;
    }
    clear_conversion(record)
}

fn sync_tree(path: &FsPath) -> Result<(), Error> {
    for entry in fs::read_dir(path).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        if kind.is_dir() { sync_tree(&entry.path())?; }
        else if kind.is_file() { sync_plain_file(&entry.path())?; }
        else { return Err(error("vault_file_type", "plaintext staging accepts ordinary files and directories only")); }
    }
    platform::durable::sync_dir(path).map_err(provider)
}

fn sync_plain_file(path: &FsPath) -> Result<(), Error> {
    #[cfg(not(windows))]
    { fs::File::open(path).and_then(|file| file.sync_all()).map_err(io) }
    #[cfg(windows)]
    {
        // FlushFileBuffers needs a writable handle. Only the owned stage is changed;
        // restore its original attribute even if opening that handle fails.
        let permissions = fs::metadata(path).map_err(io)?.permissions();
        let readonly = permissions.readonly();
        if readonly {
            let mut writable = permissions.clone();
            writable.set_readonly(false);
            fs::set_permissions(path, writable).map_err(io)?;
        }
        let opened = fs::OpenOptions::new().read(true).write(true).open(path);
        let restored = if readonly { fs::set_permissions(path, permissions) } else { Ok(()) };
        let file = opened.map_err(io)?;
        restored.map_err(io)?;
        file.sync_all().map_err(io)
    }
}

fn decrypt(api: &Arc<Api>, root: &FsPath, progress: &mut Progress, transition: &ProjectTransition) -> Result<(), Error> {
    // This makes a retry after a completed or recovered switch harmless.
    if !encrypted(root) { return Ok(()); }
    // `operate` found its view open. A close this decrypt retries has revoked it since, so it reads the mount as work does.
    let published = work_view(root)?;
    let session = held(root).ok_or_else(|| locked(root))?;
    let (secret, info) = {
        let session = session.lock().expect("vault session");
        (session.secret.duplicate(), session.keybag.info())
    };
    if super::kept_identity(&published).as_ref().is_none_or(|(id, _)| id != &info.project_id) {
        return Err(error("vault_identity", "the unlocked project does not match its keybag"));
    }
    no_nested(api, root)?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(unusable)?;
    if !CONVERTING.lock().expect("vault conversions").insert(root.to_owned()) { return Err(error("vault_busy", "the project is already being converted")); }
    let converting = Converting(root.to_owned());
    close(api, root, transition)?;
    root.parent().ok_or_else(|| error("vault_root", "cannot decrypt a filesystem root"))?;
    let suffix = fresh();
    let staged = root.join(format!(".cozy-vault-stage-{suffix}"));
    fs::create_dir(&staged).map_err(io)?;
    platform::durable::set_mode(&staged, 0o700).map_err(provider)?;
    let saved = root.join(format!(".cozy-project-source-{suffix}"));
    fs::create_dir(&saved).map_err(io)?;
    platform::durable::set_mode(&saved, 0o700).map_err(provider)?;
    let mut record = Conversion {
        root: root.to_owned(), staged: staged.clone(), saved: saved.clone(),
        key_id: info.key_id, remove_source: true, source_id: Some(directory_id(&saved)?), root_id: Some(directory_id(root)?), staged_id: Some(directory_id(&staged)?), ready: false, internal: true, source_key: None, backing_id: None,
        plain: Some(PlainConversion { project_id: info.project_id.clone(), generation: info.generation, ready: false,
            source_id: directory_id(root)?, staged_id: directory_id(&staged)?, cleanup_started: false }),
    };
    if let Err(why) = save_conversion(&record) {
        return match fs::remove_dir(&staged) {
            Ok(()) => Err(why),
            Err(cleanup) => Err(error("vault_cleanup_pending", format!("{why}; remove the empty owned stage {} before retrying: {cleanup}", staged.display()))),
        };
    }
    let outcome = (|| {
        let runtime = project_runtime(root)?;
        let source_view = project_mount::view_path(&runtime).map_err(provider)?;
        progress.phase("mounting");
        let mut source = secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &source_view, bytes,
            MountOptions { idle_timeout: None, read_only: !cfg!(windows) })).map_err(mounting)?;
        progress.phase("validating");
        progress.begin_inventory();
        walk_tree(source.view(), None, FsPath::new(""), true, !cfg!(windows), progress)?;
        progress.inventory_done();
        progress.phase("copying");
        walk_tree(source.view(), Some(&staged), FsPath::new(""), true, !cfg!(windows), progress)?;
        progress.phase("verifying");
        save_snapshot(&staged)?;
        sync_tree(&staged)?;
        if !plain_target(&record, &staged)? { return Err(error("vault_identity", "the plaintext snapshot did not retain the project identity")); }
        source.try_unmount().map_err(provider)?;
        retire_runtime(&runtime)?;
        record.plain.as_mut().expect("plaintext conversion").ready = true;
        save_conversion(&record)?;
        if !original_plain_source(&record, root)? || fs::read_dir(&record.saved).map_err(io)?.next().is_some() { return Err(error("vault_recovery", "the encrypted source changed before cutover")); }
        progress.phase("committing");
        recover_internal(&record)
    })();
    if let Err(why) = outcome {
        // Keep the directory handoff active and reload its durable state: cleanup
        // may have advanced beyond our original in-memory record before failing.
        if let Err(recovery) = recover_pending_plain(root) {
            return Err(error("vault_recovery", format!("{why}; {recovery}; retry decrypt after resolving the preserved source/stage")));
        }
        if encrypted(root) { return Err(why); }
    }
    drop(converting);
    crate::events::release_project_readers(&info.project_id);
    crate::events::project_vault(&info.project_id, "plain");
    Ok(())
}

/// A decrypt that failed after its close revoked the view leaves the project locking (its custody was being sealed, so the
/// view isn't given back): it says so, and how to go on.
fn decrypt_failed(root: &FsPath, failed: Error) -> Error {
    let open = held(root).is_some_and(|session| !lapsed(&session.lock().expect("vault session")));
    if !encrypted(root) || open { return failed; }
    Error::new(failed.kind, failed.code.clone(), format!("{}; the project locked as decrypting closed it", failed.message))
        .remedy("unlock the project, then decrypt it again").next("cozy vault unlock")
}

fn no_nested(api: &Api, root: &FsPath) -> Result<(), Error> {
    let nested = api.calcifer.maintenance_store().known_projects().map_err(|e| error("vault_journal", e))?
        .into_iter().any(|project| project.path != root && project.path.starts_with(root) && project.path.exists());
    if nested || api.projects.now_open().iter().any(|project| project.root != root && project.root.starts_with(root)) {
        return Err(Error::new(Kind::Conflict, "vault_nested_project", "move nested projects outside this project before converting its storage"));
    }
    Ok(())
}

fn convert(api: &Arc<Api>, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk, progress: &mut Progress, transition: &ProjectTransition) -> Result<PathBuf, Error> {
    let source = view(root)?;
    // A child project has its own activity, connections and transition gate. Never move/delete
    // its live storage as an incidental part of converting its parent.
    no_nested(api, root)?;
    progress.phase("validating");
    let managed = managed_tree(&source)?;
    let old_secret = held(root).map(|session| session.lock().expect("vault session").secret.duplicate());
    if old_secret.is_none() && plain_generation_for_encrypt(root, &keybag.info().project_id)? != keybag.info().generation {
        return Err(error("vault_key", "the plaintext execution generation changed before encryption"));
    }
    let target_keybag = keybag.clone();
    if !CONVERTING.lock().expect("vault conversions").insert(root.to_owned()) { return Err(error("vault_busy", "the project is already being converted")); }
    let _converting = Converting(root.to_owned());
    forget(api, root, transition)?;
    if old_secret.is_none() {
        if let crate::private_io::Sealed::Owed(_) = tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))? {
            return Err(unanswered_seal());
        }
        crate::runs::files::release_private(root)?;
    }
    // Checkpoint before measuring: the source DB's file length must include
    // committed WAL pages, and the inventory must match what the copy retains.
    progress.begin_inventory();
    walk_tree(&source, None, FsPath::new(""), managed, false, progress)?;
    progress.inventory_done();
    root.parent().ok_or_else(|| Error::new(Kind::Usage, "vault_root", "cannot convert a filesystem root"))?;
    let suffix = fresh();
    let staged = root.join(format!(".cozy-vault-stage-{suffix}"));
    let saved = root.join(format!(".cozy-project-source-{suffix}"));
    fs::create_dir(&staged).map_err(io)?;
    platform::durable::set_mode(&staged, 0o700).map_err(provider)?;
    fs::create_dir(&saved).map_err(io)?;
    platform::durable::set_mode(&saved, 0o700).map_err(provider)?;
    let mut receipt = Conversion { root: root.to_owned(), staged: staged.clone(), saved: saved.clone(),
        key_id: target_keybag.info().key_id, remove_source: true, source_id: Some(directory_id(&saved)?), root_id: Some(directory_id(root)?),
        staged_id: Some(directory_id(&staged)?), ready: false, internal: true,
        source_key: Keybag::load(&root.join(KEYBAG_FILE)).ok().map(|bag| bag.info().key_id),
        backing_id: None, plain: None };
    save_conversion(&receipt)?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(unusable)?;
    secret.with_bytes(|bytes| engine.create(&staged.join(BACKING), bytes)).map_err(mounting)?;
    keybag.save(&staged.join(KEYBAG_FILE)).map_err(key)?;
    receipt.backing_id = Some(directory_id(&staged.join(BACKING))?);
    save_conversion(&receipt)?;
    // Retire the published old view BEFORE snapshotting an encrypted project. A native editor
    // must release its stale handles (Busy refuses). The replacement source mount is never
    // published to apps. Windows lacks a qualified read-only provider: do not browse conversion
    // drives while this runs; Linux/macOS enforce a read-only source volume.
    let mut source_mount = None;
    let mut source_runtime = None;
    let source = if let Some(old) = &old_secret {
        close(api, root, transition)?;
        let runtime = project_runtime(root)?;
        let view = project_mount::view_path(&runtime).map_err(provider)?;
        let mounted = old.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: !cfg!(windows) })).map_err(mounting)?;
        let path = mounted.view().to_owned();
        source_mount = Some(mounted);
        source_runtime = Some(runtime);
        path
    } else { source };
    let runtime = project_runtime(&staged)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
    progress.phase("mounting");
    let mut target = secret.with_bytes(|bytes| engine.mount(&staged.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(mounting)?;
    progress.phase("copying");
    walk_tree(&source, Some(target.view()), FsPath::new(""), managed, source_mount.is_some() && !cfg!(windows), progress)?;
    progress.phase("verifying");
    save_snapshot(target.view())?;
    sync_tree(target.view())?;
    target.try_unmount().map_err(provider)?;
    retire_runtime(&runtime)?;
    if let Some(mut source) = source_mount.take() { source.try_unmount().map_err(provider)?; }
    if let Some(runtime) = source_runtime { retire_runtime(&runtime)?; }
    receipt.ready = true;
    save_conversion(&receipt)?;
    verify_owned_directories(&receipt)?;
    progress.phase("committing");
    move_children(root, &saved, &root_control_paths(&receipt))?;
    move_children(&staged, root, &[staged.join(RUNTIME_DIR)])?;
    platform::durable::sync_dir(root).map_err(provider)?;
    // The source is still intact if reopening fails. It is never deleted before the verified switch.
    progress.phase("opening");
    mount(api, root, keybag, secret, ask)?;
    drop(_converting);
    progress.phase("cleaning_up");
    finish_conversion(root)?;
    Ok(saved)
}

fn plain_generation_for_encrypt(root: &FsPath, id: &str) -> Result<u64, Error> {
    match crate::private_io::retained_copy_storage(root)? {
        Some(storage) if storage.project_id == id => storage.generation.checked_add(1)
            .ok_or_else(|| error("vault_key", "project key generations are exhausted")),
        // A copied native document already has a new identity. Its old retained
        // records stay inert; the replacement mount creates a fresh namespace.
        _ => Ok(1),
    }
}

fn managed_tree(source: &FsPath) -> Result<bool, Error> {
    // Executable environments contain interpreter symlinks. They are reproducible caches,
    // not retained content. Only omit them for a verified managed view, never a plain user folder.
    let logical = logical(source);
    let identity = Keybag::load(&logical.join(KEYBAG_FILE)).ok().map(|bag| (bag.info().project_id, Some(bag.info().generation)))
        .or_else(|| plain_project_id(source).map(|id| (id, None)));
    let managed = identity.is_some_and(|(id, generation)| {
        fs::read(source.join(api::machine::LOCAL_PRIVATE_MARKER)).ok()
            .and_then(|bytes| serde_json::from_slice::<api::machine::LocalPrivateMarker>(&bytes).ok())
            .is_some_and(|marker| marker.format_version == 1 && marker.storage.project_id == id
                && marker.storage.generation > 0 && generation.is_none_or(|generation| marker.storage.generation == generation)
                && platform::durable::canonical(source).is_ok_and(|path| path == marker.storage.view_root))
    }) || (plain_project_id(source).is_some() && crate::private_io::retained_copy_storage(source)?.is_some());
    Ok(managed)
}

#[cfg(test)]
fn copy_tree(source: &FsPath, target: &FsPath) -> Result<(), Error> {
    walk_tree(source, Some(target), FsPath::new(""), managed_tree(source)?, false, &mut Progress::quiet())
}

#[cfg(test)]
fn copy_tree_at(source: &FsPath, target: &FsPath, relative: &FsPath, managed: bool, frozen: bool) -> Result<(), Error> {
    walk_tree(source, Some(target), relative, managed, frozen, &mut Progress::quiet())
}

fn walk_tree(source: &FsPath, target: Option<&FsPath>, relative: &FsPath, managed: bool, frozen: bool, progress: &mut Progress) -> Result<(), Error> {
    let transition = if relative.as_os_str().is_empty() { conversion(&logical(source))?.filter(|record| record.internal) } else { None };
    for entry in fs::read_dir(source).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        let from = entry.path();
        if transition.as_ref().is_some_and(|record| root_control_paths(record).iter().any(|path| path == &from)) { continue; }
        let relative = relative.join(entry.file_name());
        let top = relative.components().next().and_then(|part| part.as_os_str().to_str());
        if relative.components().count() == 1 && top == Some(RUNTIME_DIR)
            && kind.is_dir() && fs::read(from.join("owner")).is_ok_and(|bytes| bytes == RUNTIME_MARKER)
        {
            continue;
        }
        let apple_volume = managed && cfg!(target_os = "macos");
        // These are regenerated volume indexes, not project history. Fresh APFS volumes
        // already contain them. Other user data, including readable Trash/revision data,
        // is retained; only known OS auxiliary directories may merge with fresh OS dirs.
        if apple_volume && relative.components().count() == 1 && matches!(top, Some(".fseventsd" | ".Spotlight-V100")) { continue; }
        let merge_os_directory = apple_volume && matches!(top, Some(".Trashes" | ".TemporaryItems" | ".DocumentRevisions-V100"));
        if entry.file_name() == KEYBAG_FILE || (entry.file_name() == "project.db" && relative.parent().is_some_and(|parent| !parent.as_os_str().is_empty())) {
            return Err(Error::new(Kind::Conflict, "vault_nested_project", "storage conversion refuses a nested project's storage"));
        }
        if managed {
            let mut parts = relative.components();
            let first = parts.next().and_then(|part| part.as_os_str().to_str());
            let second = parts.next().and_then(|part| part.as_os_str().to_str());
            use api::machine::project_area;
            if first == Some(api::machine::LOCAL_PRIVATE_MARKER) || first == Some(".vault-calcifer")
                || (first == Some(project_area::DIR) && second.is_some_and(project_area::rebuildable)) { continue; }
        }
        if kind.is_symlink() {
            // Legacy media links with no bytes already represent missing media. Keep
            // that project record missing; following a live external link is never implied.
            if relative.strip_prefix(super::folder::FILES).is_ok_and(|tail| !tail.as_os_str().is_empty())
                && fs::metadata(&from).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
                progress.skip(&relative);
                continue;
            }
            return Err(Error::new(Kind::Usage, "vault_symlink", format!("storage conversion cannot copy symbolic link {}; import its contents into the project first", relative.display())));
        }
        if !kind.is_dir() && !kind.is_file() { return Err(Error::new(Kind::Usage, "vault_file_type", format!("storage conversion accepts ordinary files and directories only: {}", relative.display()))); }
        if kind.is_file() && (entry.file_name() == "project.db-wal" || entry.file_name() == "project.db-shm") { continue; }
        let Some(target) = target else {
            if kind.is_dir() { walk_tree(&from, None, &relative, managed, frozen, progress)?; }
            else { progress.plan_file(entry.metadata().map_err(io)?.len()); }
            continue;
        };
        let to = target.join(entry.file_name());
        if kind.is_dir() {
            match fs::create_dir(&to) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && merge_os_directory
                    && fs::symlink_metadata(&to).is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink()) => {},
                Err(e) => return Err(error("vault_copy", format!("create directory {}: {e}", relative.display()))),
            }
            walk_tree(&from, Some(&to), &relative, managed, frozen, progress)?;
        }
        else if kind.is_file() {
            if entry.file_name() == "project.db" {
                if frozen {
                    if fs::metadata(from.with_file_name("project.db-wal")).is_ok_and(|meta| meta.len() > 0) { return Err(error("vault_database", "read-only project snapshot was not fully checkpointed")); }
                    copy_file(&from, &to, progress)?;
                    if hash(&from)? != hash(&to)? { return Err(error("vault_database", "frozen project database copy did not verify")); }
                } else {
                    let conn = rusqlite::Connection::open_with_flags(&from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| error("vault_database", format!("open source snapshot: {e}")))?;
                    conn.backup(rusqlite::MAIN_DB, &to, rusqlite::backup::NO_PROGRESS).map_err(|e| error("vault_database", format!("backup project snapshot: {e}")))?;
                    progress.copied(fs::metadata(&to).map_err(io)?.len());
                }
                // Leave a self-contained database, not a WAL header that needs a writable SHM
                // beside it. Immutable verification is safe only after this explicit checkpoint.
                {
                    let copied = rusqlite::Connection::open_with_flags(&to, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|e| error("vault_database", format!("open replacement checkpoint: {e}")))?;
                    let busy: i64 = copied.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0)).map_err(|e| error("vault_database", format!("checkpoint replacement: {e}")))?;
                    let mode: String = copied.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0)).map_err(|e| error("vault_database", format!("finalize replacement journal: {e}")))?;
                    if busy != 0 || mode != "delete" { return Err(error("vault_database", "replacement database is still in use")); }
                }
                if fs::metadata(to.with_file_name("project.db-wal")).is_ok_and(|meta| meta.len() > 0) { return Err(error("vault_database", "replacement database still has uncheckpointed data")); }
                let mut uri = url::Url::from_file_path(&to).map_err(|_| error("vault_database", "database snapshot path is not an absolute file path"))?;
                uri.query_pairs_mut().append_pair("immutable", "1");
                let copied = rusqlite::Connection::open_with_flags(uri.as_str(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI).map_err(|e| error("vault_database", format!("open copied snapshot for verification: {e}")))?;
                let okay: String = copied.query_row("PRAGMA quick_check", [], |row| row.get(0)).map_err(|e| error("vault_database", e))?;
                if okay != "ok" { return Err(error("vault_database", "the copied project database did not verify")); }
            } else {
                copy_file(&from, &to, progress)?;
                if hash(&from)? != hash(&to)? { return Err(error("vault_copy_mismatch", "a copied project file changed or did not verify")); }
            }
            progress.file_done();
        } else { return Err(Error::new(Kind::Usage, "vault_file_type", "storage conversion accepts ordinary files and directories only")); }
    }
    Ok(())
}

fn copy_file(from: &FsPath, to: &FsPath, progress: &mut Progress) -> Result<(), Error> {
    use std::io::Write as _;
    let mut source = fs::File::open(from).map_err(io)?;
    let mut destination = fs::File::create(to).map_err(io)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = source.read(&mut buffer).map_err(io)?;
        if count == 0 { break; }
        destination.write_all(&buffer[..count]).map_err(io)?;
        progress.copied(count as u64);
    }
    fs::set_permissions(to, source.metadata().map_err(io)?.permissions()).map_err(io)
}

fn hash(path: &FsPath) -> Result<[u8;32], Error> {
    let mut file = fs::File::open(path).map_err(io)?;
    let mut sha = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop { let n = file.read(&mut bytes).map_err(io)?; if n == 0 { break; } sha.update(&bytes[..n]); }
    Ok(sha.finalize().into())
}

fn fresh() -> String { let mut bytes = [0u8;16]; getrandom::fill(&mut bytes).expect("OS randomness"); hex::encode(bytes) }

pub(crate) const PAYLOAD_PREFIX: &[u8] = b"cozy:vault:journal:2:";
const JOURNAL_DIR: &str = ".vault-journal";
const MAX_JOURNAL_BYTES: u64 = 32 << 20;
/// The marker a 0.26 RC left beside an encrypted project while Calcifer's store still had to be compacted for it (cl-1003: none is now).
const JOURNAL_CLEANUP_FILE: &str = "project.vault.journal-pending";
static KEY_ROOTS: LazyLock<Mutex<BTreeMap<String, PathBuf>>> = LazyLock::new(Mutex::default);
static TRUSTED_KEYS: LazyLock<Mutex<BTreeMap<PathBuf, Keybag>>> = LazyLock::new(Mutex::default);

#[derive(serde::Serialize, serde::Deserialize)]
struct ProtectedPayload {
    project_id: String,
    root: PathBuf,
    context: String,
    object: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PrivateRecord { project_id: String, context: String, data: String }

pub(crate) fn is_payload(value: &[u8]) -> bool { value.starts_with(PAYLOAD_PREFIX) }

fn payload(value: &[u8]) -> Result<Option<ProtectedPayload>, Error> {
    let Some(encoded) = value.strip_prefix(PAYLOAD_PREFIX) else { return Ok(None) };
    let record: ProtectedPayload = serde_json::from_slice(encoded).map_err(|_| error("vault_journal", "invalid private journal pointer"))?;
    if record.object.len() != 32 || !record.object.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err(error("vault_journal", "invalid private journal object")); }
    Ok(Some(record))
}

fn current_root(record: &ProtectedPayload) -> PathBuf { KEY_ROOTS.lock().expect("vault project roots").get(&record.project_id).cloned().unwrap_or_else(|| record.root.clone()) }

/// Global SQL stores only public pointer metadata. Private bytes live under the SAME project filesystem cipher.
pub(crate) fn seal_payload(path: &FsPath, context: &str, value: &[u8]) -> Result<Vec<u8>, Error> {
    use base64::Engine as _;
    let path = logical(path);
    let Some(root) = path.ancestors().find(|root| encrypted(root)) else { return Ok(value.to_vec()) };
    let view = work_view(root)?;
    let keybag = TRUSTED_KEYS.lock().expect("trusted vault recipients").get(root).cloned().ok_or_else(|| locked(root))?;
    let project_id = keybag.info().project_id;
    let dir = view.join(JOURNAL_DIR);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {},
        Ok(_) => return Err(error("vault_journal", "the private journal is not a project-owned directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&dir).map_err(io)?,
        Err(e) => return Err(io(e)),
    }
    if !platform::durable::canonical(&dir).map_err(provider)?.starts_with(platform::durable::canonical(&view).map_err(provider)?) { return Err(error("vault_journal", "private journal escapes its mounted project")); }
    let object = fresh();
    let private = PrivateRecord { project_id: project_id.clone(), context: context.into(), data: base64::engine::general_purpose::STANDARD.encode(value) };
    let bytes = serde_json::to_vec(&private).map_err(|e| error("vault_journal", e))?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES { return Err(error("vault_journal", "private journal record is too large")); }
    // A fresh immutable revision is durable BEFORE the SQL pointer transaction commits.
    platform::durable::replace(&dir.join(&object), &bytes, 0o600).map_err(provider)?;
    let reference = ProtectedPayload { project_id, root: root.to_owned(), context: context.into(), object };
    let mut out = PAYLOAD_PREFIX.to_vec();
    out.extend(serde_json::to_vec(&reference).map_err(|e| error("vault_journal", e))?);
    Ok(out)
}

pub(crate) fn payload_root(value: &[u8]) -> Result<Option<PathBuf>, Error> { Ok(payload(value)?.map(|record| current_root(&record))) }



pub(crate) fn payload_path(value: &[u8], decoded: &FsPath) -> Result<PathBuf, Error> {
    let Some(reference) = payload(value)? else { return Ok(decoded.to_owned()) };
    Ok(match decoded.strip_prefix(&reference.root) { Ok(relative) => current_root(&reference).join(relative), Err(_) => decoded.to_owned() })
}


pub(crate) fn open_payload(context: &str, value: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    use base64::Engine as _;
    let Some(reference) = payload(value)? else { return Ok(Some(value.to_vec())) };
    if reference.context != context { return Err(error("vault_journal", "private journal context mismatch")); }
    let root = current_root(&reference);
    let view = if let Some(session) = held(&root) {
        let mut session = session.lock().expect("vault session");
        if session.secret.project_id() != reference.project_id { return Err(error("vault_journal", "private journal project mismatch")); }
        session.mount.reconcile().map_err(provider)?;
        if !session.mount.is_mounted().map_err(provider)? { return Err(locked(&root)); }
        session.mount.view().to_owned()
    } else {
        if encrypted(&root) || KEY_ROOTS.lock().expect("vault project roots").get(&reference.project_id) != Some(&root) { return Ok(None); }
        crate::private_io::retained_view(&root, &reference.project_id)?
    };
    let at = view.join(JOURNAL_DIR).join(&reference.object);
    if fs::symlink_metadata(&at).map_err(io)?.file_type().is_symlink() { return Err(error("vault_journal", "private journal objects cannot be symlinks")); }
    if !platform::durable::canonical(&at).map_err(provider)?.starts_with(platform::durable::canonical(&view).map_err(provider)?) { return Err(error("vault_journal", "private journal object escapes its mounted project")); }
    let mut bytes = Vec::new();
    fs::File::open(&at).map_err(io)?.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES { return Err(error("vault_journal", "private journal record is too large")); }
    let private: PrivateRecord = serde_json::from_slice(&bytes).map_err(|_| error("vault_journal", "invalid private journal record"))?;
    if private.project_id != reference.project_id || private.context != context { return Err(error("vault_journal", "private journal ownership mismatch")); }
    let plain = base64::engine::general_purpose::STANDARD.decode(private.data).map_err(|_| error("vault_journal", "invalid private journal value"))?;
    Ok(Some(plain))
}


/// Private responses never enter browser caches; an existing stream stops yielding new chunks once lock begins.
pub(super) async fn privacy(
    State(api): State<Arc<Api>>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(id) = params.get("id") else { return private(next.run(request).await) };
    let root = match location(&api, id) {
        Ok(root) => root,
        Err(error) => return private(Refusal(error).into_response()),
    };
    let token = {
        // Handler-owned filesystem/database operations take their own bounded
        // admission. A worker/network wait must never hold the transition gate.
        let _access = access(&root).read_owned().await;
        if let Err(error) = view(&root) { return private(Refusal(error).into_response()); }
        readable_token(&root)
    };
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        if let Some(session) = held(&root) {
            let mut session = session.lock().expect("vault session");
            if !session.closing && !expired(&session) && session.timer.activity() {
                session.active_ms = super::now_ms();
                // The method alone: a request's path names its project, which a locked project leaves nowhere (cl-1031).
                say(&root, "activity", || format!("its idle clock restarts on a {} request", request.method()));
            }
        }
    }
    protect_reply(next.run(request).await, token)
}

/// Project-scoped APIs outside the project router use the same response boundary.
pub(crate) async fn project_reply<T: IntoResponse>(
    api: &Api,
    named: Option<&str>,
    answer: impl std::future::Future<Output = Result<T, Refusal>>,
) -> Result<Response, Refusal> {
    let Some(named) = named else { return answer.await.map(IntoResponse::into_response).map(private) };
    let root = location(api, named).map_err(Refusal)?;
    let token = {
        let _access = admit_root(&root).await.map_err(Refusal)?;
        readable_token(&root)
    };
    let response = answer.await.into_response();
    Ok(protect_reply(response, token))
}

fn protect_reply(response: Response, token: Option<Arc<AtomicBool>>) -> Response {
    let response = private(response);
    let Some(token) = token else { return response };
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, revocable(body, token))
}

/// Told each time a view's readable token is revoked.
static REVOKED: LazyLock<tokio::sync::watch::Sender<u64>> = LazyLock::new(|| tokio::sync::watch::channel(0).0);

fn revoke(token: &AtomicBool) {
    token.store(false, Ordering::Release);
    REVOKED.send_modify(|told| *told = told.wrapping_add(1));
}

/// A private body that ends once its view's lease is revoked, even while its client reads nothing: what it reads from
/// (an open file in the view) is dropped at once, so the view can unmount.
pub(crate) fn revocable(body: axum::body::Body, token: Arc<AtomicBool>) -> axum::body::Body {
    use futures_util::StreamExt as _;
    struct Source { body: Mutex<Option<axum::body::BodyDataStream>>, reader: futures_util::task::AtomicWaker }
    let source = Arc::new(Source { body: Mutex::new(Some(body.into_data_stream())), reader: Default::default() });
    let (reading, ended) = tokio::sync::oneshot::channel::<()>();
    let (held, open, mut told) = (Arc::downgrade(&source), token.clone(), REVOKED.subscribe());
    tokio::spawn(async move {
        let revoked = async { while open.load(Ordering::Acquire) { if told.changed().await.is_err() { std::future::pending::<()>().await; } } };
        tokio::select! {
            _ = ended => {}
            () = revoked => if let Some(source) = held.upgrade() {
                source.body.lock().expect("private body").take();
                // Its reader waits on the dropped source: told now, it reads the end.
                source.reader.wake();
            },
        }
    });
    axum::body::Body::from_stream(futures_util::stream::poll_fn(move |cx| {
        let _reading = &reading;
        source.reader.register(cx.waker());
        let mut body = source.body.lock().expect("private body");
        if !token.load(Ordering::Acquire) { body.take(); }
        match body.as_mut() { Some(body) => body.poll_next_unpin(cx), None => std::task::Poll::Ready(None) }
    }))
}
