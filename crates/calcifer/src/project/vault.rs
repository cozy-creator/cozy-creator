//! Optional local encrypted project sessions. A registered path never becomes a plaintext fallback mount.
use std::{collections::{BTreeMap, BTreeSet}, fs, io::Read as _, path::{Path as FsPath, PathBuf}, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::Duration};
use axum::{body::Bytes, extract::{Path, State, rejection::BytesRejection}, http::{HeaderValue, header}, response::{IntoResponse, Response}};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultStatus}, error::{Error, Kind}};
use project_mount::{MountOptions, Mounted, Provider};
use project_vault::{Credential, Keybag, ProjectSecret, SecretString, SessionPolicy, SessionTimer, KEYBAG_FILE};
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
/// A session began: its clocks and mount are watched from now on.
static HELD: tokio::sync::Notify = tokio::sync::Notify::const_new();

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
fn key(e: project_vault::Error) -> Error {
    match e {
        rejected @ project_vault::Error::Unlock => Error::new(Kind::Credential, "vault_unlock_failed", rejected.to_string()),
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

/// The canonical id is available without opening the project's content database.
pub(super) fn project_id(root: &FsPath) -> Option<String> {
    if let Some(session) = held(root) {
        return Some(session.lock().expect("vault session").secret.project_id().to_owned());
    }
    Keybag::load(&root.join(KEYBAG_FILE)).ok().map(|keybag| keybag.info().project_id)
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
        if self.root != project.root || project.mount_id != mount_identity(&self.root) {
            return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
        }
        let view = if encrypted(&self.root) { work_view(&self.root)? }
            else { platform::durable::canonical(&self.root).map_err(provider)? };
        if view != project.folder.dir {
            return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
        }
        Ok(())
    }
}

fn exclusive(root: &FsPath) -> Result<ProjectTransition, Error> {
    let guard = access(root).try_write_owned().map_err(|_| Error::new(Kind::Conflict, "project_busy", "a project request is still running; retry after it finishes"))?;
    Ok(ProjectTransition { root: root.to_owned(), _access: guard })
}

/// Upload routes are outside the project router, but must hold the same storage transition
/// lease through their body and commit. A delayed upload cannot write into a reused drive.
pub(crate) async fn admit_project(api: &Api, named: &str) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let root = location(api, named)?;
    admit_root(&root).await
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
        let path = logical(&platform::durable::canonical(&candidate).map_err(provider)?);
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
    let why = format!("{} did not lock at its deadline: {}", root.display(), failure.message);
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
            let max_at = (session.maximum > 0 && !session.timer.held()).then(|| session.began_ms.saturating_add(milliseconds(session.maximum)));
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

/// Whether the legacy source still holds this project's own history; another project's never holds it.
fn own_migration_pending(api: &Api, root: &FsPath) -> Result<bool, Error> {
    let Some(id) = project_id(root).or_else(|| plain_project_id(root)) else { return Ok(false) };
    api.calcifer.maintenance_store().project_migration_required(root, &id)
        .map_err(|_| error("project_migration_pending", "legacy project storage could not be checked"))
}

fn reported_status(api: &Api, root: &FsPath) -> VaultStatus {
    let mut state = status(root);
    if state.state == "locked" && own_migration_pending(api, root).unwrap_or(true) {
        state.state = "storage_update_required".into();
        state.next = vec!["cozy vault unlock".into()];
    }
    state
}

fn milliseconds(seconds: u64) -> i64 { i64::try_from(seconds).unwrap_or(i64::MAX).saturating_mul(1000) }

fn expired(session: &Session) -> bool { session.timer.expired() }

/// Its view can no longer be opened: a lock revoked it, its clocks ran out, or its mount is gone. The sweeper locks it.
fn lapsed(session: &Session) -> bool { session.closing || expired(session) || !session.mount.is_mounted().unwrap_or(false) }

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
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    let mounted = secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
    let canonical_view = platform::durable::canonical(mounted.view()).map_err(provider)?;
    ROOTS.lock().expect("vault roots").insert(mounted.view().to_owned(), root.to_owned());
    ROOTS.lock().expect("vault roots").insert(canonical_view.clone(), root.to_owned());
    crate::private_io::descriptor_for_mounted(&canonical_view, &keybag.info().project_id, keybag.info().generation)?;
    TRUSTED_KEYS.lock().expect("trusted vault recipients").insert(root.to_owned(), keybag.clone());
    KEY_ROOTS.lock().expect("vault project roots").insert(keybag.info().project_id.clone(), root.to_owned());
    let idle = ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE);
    let maximum = ask.max_duration_s.unwrap_or(0);
    let session = Session { identity: MountIdentity(Arc::new(())), mount: mounted, secret, keybag, idle, maximum, timer: timer(idle, maximum), active_ms: super::now_ms(), began_ms: super::now_ms(), closing: false, readable: Arc::new(AtomicBool::new(true)), runtime };
    SESSIONS.lock().expect("vault sessions").insert(root.to_owned(), Arc::new(Mutex::new(session)));
    // A session began: the sweep below watches its clocks and mount.
    HELD.notify_one();
    Ok(())
}

fn forget(api: &Arc<Api>, root: &FsPath, transition: &ProjectTransition) -> Result<(), Error> {
    let available = !encrypted(root) || held(root).is_some_and(|session| session.lock().expect("vault session").mount.is_mounted().unwrap_or(false));
    if api.projects.active_requests(root) || crate::local_code::private_process_active(root) {
        return Err(Error::new(Kind::Conflict, "project_busy", "a project request or tool is still finishing; retry after it stops"));
    }
    for project in api.projects.now_open().iter().filter(|project| project.root == root) {
        let store = api.projects.transition_store(api, &project.id, transition)?;
        if !store.unsettled().map_err(|_| error("project_journal", "the project journal could not be checked"))?.is_empty()
            || !store.unsettled_here().map_err(|_| error("project_journal", "the project journal could not be checked"))?.is_empty()
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
        project.folder.close_connections(available)?;
        api.projects.open.lock().expect("projects lock").remove(&project.id);
        crate::events::project_closed(&project.id);
    }
    if available {
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
    if changed { crate::events::project_vault(session.secret.project_id(), "locking"); }
}

fn prepare_browser(api: &Arc<Api>) -> Result<Option<crate::browser::PreparedBrowserClose>, Error> {
    tokio::runtime::Handle::current().block_on(api.browser.prepare_close()).map(Some).map_err(|error| error.0)
}

fn retire_browser(api: &Arc<Api>, browser: &mut Option<crate::browser::PreparedBrowserClose>) -> Result<(), Error> {
    if let Some(prepared) = browser.take() {
        tokio::runtime::Handle::current().block_on(api.browser.close(prepared)).map_err(|error| error.0)?;
    }
    Ok(())
}

fn close(api: &Arc<Api>, root: &FsPath, transition: &ProjectTransition, browser: &mut Option<crate::browser::PreparedBrowserClose>) -> Result<(), Error> {
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        mark_closing(&mut session);
    }
    vault_operation::revoke(root);
    // Existing accepted work may drain while the key is retained, but no new viewing stream can read it.
    forget(api, root, transition)?;
    tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?;
    crate::runs::files::release_private(root)?;
    if let Some(id) = project_id(root).or_else(|| plain_project_id(root)) { api.calcifer.retire_project(&id); }
    let pending = api.calcifer.maintenance_store().migration_required()
        .map_err(|_| error("project_migration_pending", "legacy storage could not be checked before closing"))?;
    if !pending {
        api.calcifer.maintenance_store().cleanup_private_journal()
            .map_err(|_| error("project_migration_pending", "legacy database pages could not be retired before closing"))?;
    }
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        session.mount.try_unmount().map_err(|why| match why {
            project_mount::Error::Busy(_) => Error::new(Kind::Conflict, "project_busy", "close programs using the project, then retry the storage change"),
            other => provider(other),
        })?;
        retire_runtime(&session.runtime)?;
        drop(session);
        // Only after the view is gone: a busy refusal above leaves the owned pages as they were.
        retire_browser(api, browser)?;
        SESSIONS.lock().expect("vault sessions").remove(root);
        LOCK_RETRIES.lock().expect("lock retries").remove(root);
        ROOTS.lock().expect("vault roots").retain(|_, logical| logical != root);
        if let Ok(keybag) = Keybag::load(&root.join(KEYBAG_FILE)) {
            let state = reported_status(api, root);
            crate::events::project_vault(&keybag.info().project_id, &state.state);
            if state.state != "locked" && !CONVERTING.lock().expect("vault conversions").contains(root) {
                return Err(error("project_migration_pending", "the project is closed, but legacy data outside it still needs a verified storage update"));
            }
        }
    }
    retire_browser(api, browser)?;
    Ok(())
}

pub(crate) fn close_all(api: &Arc<Api>) -> Result<(), Error> {
    let mut browser = prepare_browser(api)?;
    close_all_prepared(api, &mut browser)
}

pub(crate) fn close_all_prepared(api: &Arc<Api>, browser: &mut Option<crate::browser::PreparedBrowserClose>) -> Result<(), Error> {
    let roots: Vec<_> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
    for root in roots {
        let op = operation(&root);
        let _operation = op.lock().expect("vault operation");
        if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
        let transition = exclusive(&root)?;
        close(api, &root, &transition, browser)?;
    }
    retire_browser(api, browser)?;
    Ok(())
}

pub(crate) fn start(api: &Arc<Api>) {
    let api = Arc::downgrade(api);
    tokio::spawn(async move {
        loop {
            // Each second while a vault is open, its idle and maximum clocks and its mount; nothing at all with none.
            while SESSIONS.lock().expect("vault sessions").is_empty() {
                HELD.notified().await;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(api) = api.upgrade() else { return };
            vault_operation::expire();
            let roots: Vec<PathBuf> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
            for root in roots {
                let api = api.clone();
                // One slow export/unmount must not delay another project's deadline.
                tokio::task::spawn_blocking(move || idle_sweep(&api, &root));
            }
        }
    });
}

/// How long past an idle or maximum deadline a vault waits for Cozy's browser to confirm its edits are saved before it
/// locks anyway: the deadline is a security one.
const SAVE_GRACE: Duration = Duration::from_secs(30);
/// When each open vault was first seen past its deadline and not yet locked.
static DUE: LazyLock<Mutex<BTreeMap<PathBuf, std::time::Instant>>> = LazyLock::new(Mutex::default);

/// One second of a vault's clocks: it locks once its deadline has passed, in order (the browser's edits saved, its
/// requests drained) and, past `SAVE_GRACE`, whatever is unsaved.
fn idle_sweep(api: &Arc<Api>, root: &FsPath) {
    let op = operation(root);
    let Ok(_operation) = op.try_lock() else { say(root, "operation", || "its clocks wait for a vault operation".into()); return; };
    let working = working(api, root);
    let needs = held(root).is_some_and(|session| {
        let mut session = session.lock().expect("vault session");
        if session.mount.reconcile().is_err() { mark_closing(&mut session); }
        // Work in progress holds it open, its maximum included: the clocks go on when the last of it
        // ends. Only an explicit lock ends work (owner 11-11: "runs count as activity").
        session.timer.hold(working.is_some());
        if let Some(why) = working.filter(|_| !session.closing) {
            session.active_ms = super::now_ms();
            if session.idle > 0 { say(root, "held", || format!("its idle clock is held by {why}")); }
        }
        lapsed(&session)
    });
    if !needs { DUE.lock().expect("vault deadlines").remove(root); return; }
    let due = *DUE.lock().expect("vault deadlines").entry(root.to_owned()).or_insert_with(std::time::Instant::now);
    let forced = due.elapsed() >= SAVE_GRACE;
    if !forced && LOCK_RETRIES.lock().expect("lock retries").get(root).is_some_and(|retry| std::time::Instant::now() < retry.at) { return; }
    // The browser's save acknowledgements may include activity writes
    // under this operation mutex. Never wait for them while owning it.
    drop(_operation);
    let mut browser = match prepare_browser(api) {
        Ok(browser) => browser,
        Err(why) if forced => {
            let unsent = tokio::runtime::Handle::current().block_on(api.browser.unsent());
            let unsent = unsent.map_or("an unknown number of".to_owned(), |count| count.to_string());
            say(root, "forced", || format!("its deadline passed {} s ago and Cozy's browser has not confirmed its edits are saved ({}): locking anyway, {unsent} unsent edits left", due.elapsed().as_secs(), why.message));
            if let Some(id) = project_id(root) { crate::events::project_vault(&id, "locking"); }
            None
        }
        Err(why) => { say(root, "browser", || format!("its lock waits for Cozy's browser to save its edits: {}", why.message)); return; }
    };
    let Ok(_operation) = op.try_lock() else { say(root, "operation", || "its lock waits for a vault operation".into()); return; };
    let still_needs = held(root).is_some_and(|session| {
        let session = session.lock().expect("vault session");
        lapsed(&session)
    });
    if !still_needs { return; }
    if let Some(session) = held(root) { mark_closing(&mut session.lock().expect("vault session")); }
    let Ok(transition) = exclusive(root) else {
        say(root, "gate", || "locking waits for a request still holding the project".into());
        return;
    };
    match close(api, root, &transition, &mut browser) {
        Ok(()) => { DUE.lock().expect("vault deadlines").remove(root); }
        Err(failure) => idle_lock_failed(root, &failure),
    }
}

pub(super) async fn read(State(api): State<Arc<Api>>, Path(named): Path<String>) -> Result<Response, Refusal> {
    let root = location(&api, &named).map_err(Refusal)?;
    let _access = access(&root).read_owned().await;
    Ok(private(axum::Json(reported_status(&api, &root)).into_response()))
}

pub(super) async fn write(State(api): State<Arc<Api>>, Path(named): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let mut ask: VaultAsk = parse(&body?)?;
    let root = location(&api, &named).map_err(Refusal)?;
    let changed = tokio::task::block_in_place(|| operate(&api, &root, &mut ask, &mut Progress::quiet())).map_err(Refusal)?;
    Ok(private(axum::Json(changed).into_response()))
}

pub(super) fn operate(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk, progress: &mut Progress) -> Result<VaultChanged, Error> {
    if ask.action == "lock" && !encrypted(root) {
        return Err(Error::new(Kind::Usage, "project_plain", "unencrypted projects cannot be locked"));
    }
    progress.phase("waiting");
    let mut browser = if matches!(ask.action.as_str(), "lock" | "encrypt" | "decrypt" | "rotate") { prepare_browser(api)? } else { None };
    let op = operation(&root); let _guard = op.lock().expect("vault operation");
    let mut ended = vec![];
    if ask.action == "lock" {
        // A lock that found the project busy already revoked its view: this one finishes it, and admits no receipt into
        // a view that can no longer be opened.
        let revoked = held(root).is_some_and(|session| lapsed(&session.lock().expect("vault session")));
        let busy = match held(root).filter(|_| !revoked).map(|_| exclusive(root)) {
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
        // A request still holds the project: its view is revoked, and the sweeper locks once it ends.
        if let Some(busy) = busy {
            if ended.is_empty() { return Err(busy); }
            progress.readable = readable_token(root);
            return Ok(VaultChanged { vault: reported_status(api, &root), recovery_secret: None, ended, skipped_files: vec![] });
        }
    }
    let transition = exclusive(&root)?;
    progress.phase("preparing");
    let mut changed = match change(&api, &root, ask, progress, &transition, &mut browser) {
        // What it ended is still settling: the session is closing, and the sweeper locks it once it has.
        Err(busy) if !ended.is_empty() && busy.code == "project_busy" => VaultChanged { vault: reported_status(api, &root), recovery_secret: None, ended: vec![], skipped_files: vec![] },
        changed => changed?,
    };
    changed.ended = ended;
    // Capture this exact mount's revocation token while the operation still
    // holds the transition guard, so a later lock cannot revive its reply.
    progress.readable = readable_token(root);
    Ok(changed)
}

pub(super) fn private(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// Refuse missing providers/drivers before creating a new project's directory or reserved storage.
pub(super) fn preflight_creation(api: &Api) -> Result<Provider, Error> {
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    engine.check_driver().map_err(provider)?;
    Ok(engine)
}

/// Initialize the first database through encrypted storage. There is no plaintext source to convert or preserve.
pub(super) fn create_new(api: &Api, root: &FsPath, id: &str, name: &str, written: super::written::Written, passphrase: SecretString, engine: &Provider) -> Result<String, Error> {
    let operation = operation(root);
    let _operation = operation.lock().expect("vault operation");
    let _access = exclusive(root)?;
    let created = Keybag::create(id, passphrase).map_err(key)?;
    let recovery = created.recovery_secret.expose().to_owned();
    let outcome = (|| {
        created.secret.with_bytes(|bytes| engine.create(&root.join(BACKING), bytes)).map_err(provider)?;
        // Persist the wrapping slots before writing any project state: the user's known passphrase
        // can reopen this backing even if setup or delivery of the recovery response is interrupted.
        created.keybag.save(&root.join(KEYBAG_FILE)).map_err(key)?;
        let runtime = project_runtime(root)?;
        let view = project_mount::view_path(&runtime).map_err(provider)?;
        let mut target = created.secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes,
            MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
        let initialized = super::new_database(target.view(), id, name, written);
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

fn change(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk, progress: &mut Progress, transition: &ProjectTransition, browser: &mut Option<crate::browser::PreparedBrowserClose>) -> Result<VaultChanged, Error> {
    recover_under_guards(root)?;
    require_transition_ready(root, &ask.action)?;
    if ask.action == "encrypt" && encrypted(root) {
        return Err(Error::new(Kind::Conflict, "project_encrypted", "the project is already encrypted"));
    }
    if matches!(ask.action.as_str(), "decrypt" | "rotate" | "passphrase" | "recovery") && !encrypted(root) {
        return Err(Error::new(Kind::Usage, "project_plain", "this project is not encrypted"));
    }
    if matches!(ask.action.as_str(), "encrypt" | "decrypt" | "rotate" | "passphrase" | "recovery") {
        let id = project_id(root).or_else(|| plain_project_id(root)).ok_or_else(|| error("vault_identity", "the project has no verified identity"))?;
        let store = api.projects.transition_store(api, &id, transition)?;
        progress.admit_receipt(&store)?;
        api.calcifer.maintenance_store().migrate_project_sealing(&store, &api.calcifer.inputs)
            .map_err(|_| error("project_migration_pending", "legacy project history must be verified before changing its storage"))?;
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
            convert(api, root, created.keybag, created.secret, ask, progress, transition, browser)?;
            drop(project);
        }
        "unlock" => {
            progress.phase("opening");
            if !encrypted(root) { return Err(Error::new(Kind::Usage, "project_plain", "this project is not encrypted")); }
            let keybag = Keybag::load(&root.join(KEYBAG_FILE)).map_err(key)?;
            let secret = keybag.unlock(credential(ask)?).map_err(key)?;
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
            if api.calcifer.maintenance_store().migration_required().map_err(|_| error("project_migration_pending", "legacy project storage could not be checked"))? {
                progress.phase("updating_project_storage");
                api.calcifer.maintenance_store().migrate_project_sealing(&store, &api.calcifer.inputs)
                    .map_err(|_| error("project_migration_pending", "the project is open, but its legacy history still needs a verified storage update"))?;
            }
            let completing = receipt_path(root).is_file();
            if completing || journal_cleanup_pending(root)? {
                if own_migration_pending(api, root)? {
                    return Err(error("project_migration_pending", "the project is open; verified legacy ownership is still required before storage cleanup"));
                }
                // Compaction copies the whole store: it waits while any project's history is still pending there.
                if !api.calcifer.maintenance_store().migration_required().map_err(|_| error("project_migration_pending", "legacy project storage could not be checked"))? {
                    progress.phase("compacting_history");
                    api.calcifer.maintenance_store().cleanup_private_journal().map_err(|_| error("project_migration_pending", "the project is open, but legacy journal cleanup is still pending"))?;
                    progress.phase("cleaning_up");
                    journal_cleanup_done(root)?;
                }
                if completing { finish_conversion(root)?; }
            }
            if let Some(session) = held(root) { crate::events::project_vault(session.lock().expect("vault session").secret.project_id(), "unlocked"); }
            progress.finish_receipt(&store, "completed")?;
        }
        "lock" => {
            if !encrypted(root) { return Err(Error::new(Kind::Usage, "project_plain", "unencrypted projects cannot be locked")); }
            // Locking a locked project is a no-op, after retiring what a dead Calcifer's views left.
            if held(root).is_some() { close(api, root, transition, browser)?; } else { retire_orphaned_views(root)?; }
        }
        "activity" => {
            if let Some(session) = held(root) {
                let mut session = session.lock().expect("vault session");
                if !session.closing && !expired(&session) && session.timer.activity() {
                    session.active_ms = super::now_ms();
                    say(root, "activity op", || "its idle clock restarts on a page's activity".into());
                }
            }
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
        "decrypt" => decrypt(api, root, progress, transition, browser)?,
        "rotate" => {
            let session = held(root).ok_or_else(|| locked(root))?;
            let created = session.lock().expect("vault session").keybag.new_generation(new_passphrase(ask)?).map_err(key)?;
            recovery_secret = Some(created.recovery_secret.expose().to_owned());
            convert(api, root, created.keybag, created.secret, ask, progress, transition, browser)?;
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
    Ok(VaultChanged { vault: reported_status(api, root), recovery_secret, ended: vec![], skipped_files: progress.skipped() })
}

/// Whether the project has work in progress: a run not settled, or a render, cut or file move going.
/// What holds an unlocked vault open now, if anything.
fn working(api: &Arc<Api>, root: &FsPath) -> Option<&'static str> {
    if api.projects.active_requests(root) { return Some("a project request being answered"); }
    if crate::local_code::private_process_active(root) { return Some("a private process"); }
    let projects = api.projects.now_open();
    let project = projects.iter().find(|project| project.root == root)?;
    if *project.walking.lock().expect("walking lock") || project.moving.try_lock().is_err() || project.cutting.try_lock().is_err() { return Some("a render, move or cut"); }
    let Ok(transition) = exclusive(root) else { return Some("a reader holding the project") };
    let Ok(store) = api.projects.transition_store(api, &project.id, &transition) else { return Some("a journal it cannot read") };
    // The transition authority can inspect already-accepted work even if an idle
    // deadline just expired. Public viewing admission is deliberately stricter.
    (store.unsettled().map_or(true, |runs| !runs.is_empty()) || store.unsettled_here().map_or(true, |runs| !runs.is_empty()))
        .then_some("runs not settled")
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

fn decrypt(api: &Arc<Api>, root: &FsPath, progress: &mut Progress, transition: &ProjectTransition, browser: &mut Option<crate::browser::PreparedBrowserClose>) -> Result<(), Error> {
    // This makes a retry after a completed or recovered switch harmless.
    if !encrypted(root) { return Ok(()); }
    let published = view(root)?;
    let session = held(root).ok_or_else(|| locked(root))?;
    let (secret, info) = {
        let session = session.lock().expect("vault session");
        (session.secret.duplicate(), session.keybag.info())
    };
    if super::kept_identity(&published).as_ref().is_none_or(|(id, _)| id != &info.project_id) {
        return Err(error("vault_identity", "the unlocked project does not match its keybag"));
    }
    no_nested(api, root)?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    if !CONVERTING.lock().expect("vault conversions").insert(root.to_owned()) { return Err(error("vault_busy", "the project is already being converted")); }
    let converting = Converting(root.to_owned());
    close(api, root, transition, browser)?;
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
            MountOptions { idle_timeout: None, read_only: !cfg!(windows) })).map_err(provider)?;
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

fn no_nested(api: &Api, root: &FsPath) -> Result<(), Error> {
    let nested = api.calcifer.maintenance_store().known_projects().map_err(|e| error("vault_journal", e))?
        .into_iter().any(|project| project.path != root && project.path.starts_with(root) && project.path.exists());
    if nested || api.projects.now_open().iter().any(|project| project.root != root && project.root.starts_with(root)) {
        return Err(Error::new(Kind::Conflict, "vault_nested_project", "move nested projects outside this project before converting its storage"));
    }
    Ok(())
}

fn convert(api: &Arc<Api>, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk, progress: &mut Progress, transition: &ProjectTransition, browser: &mut Option<crate::browser::PreparedBrowserClose>) -> Result<PathBuf, Error> {
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
    retire_browser(api, browser)?;
    if old_secret.is_none() {
        tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?;
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
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    secret.with_bytes(|bytes| engine.create(&staged.join(BACKING), bytes)).map_err(provider)?;
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
        close(api, root, transition, browser)?;
        let runtime = project_runtime(root)?;
        let view = project_mount::view_path(&runtime).map_err(provider)?;
        let mounted = old.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: !cfg!(windows) })).map_err(provider)?;
        let path = mounted.view().to_owned();
        source_mount = Some(mounted);
        source_runtime = Some(runtime);
        path
    } else { source };
    let runtime = project_runtime(&staged)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
    progress.phase("mounting");
    let mut target = secret.with_bytes(|bytes| engine.mount(&staged.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
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
const JOURNAL_CLEANUP_FILE: &str = "project.vault.journal-pending";
const JOURNAL_CLEANUP_MARKER: &[u8] = b"cozy project journal cleanup 1\n";
static KEY_ROOTS: LazyLock<Mutex<BTreeMap<String, PathBuf>>> = LazyLock::new(Mutex::default);
static TRUSTED_KEYS: LazyLock<Mutex<BTreeMap<PathBuf, Keybag>>> = LazyLock::new(Mutex::default);

fn journal_cleanup_pending(root: &FsPath) -> Result<bool, Error> {
    let path = root.join(JOURNAL_CLEANUP_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(why) => return Err(io(why)),
    };
    let mut bytes = Vec::new();
    if metadata.file_type().is_file() {
        fs::File::open(&path).map_err(io)?.take(JOURNAL_CLEANUP_MARKER.len() as u64 + 1).read_to_end(&mut bytes).map_err(io)?;
    }
    if bytes != JOURNAL_CLEANUP_MARKER {
        return Err(error("vault_journal_cleanup", format!("unexpected {JOURNAL_CLEANUP_FILE}; preserve this file and resolve the conflict before retrying")));
    }
    Ok(true)
}

/// Durable before the global SQL rewrite commits; contains no private data or key material.
pub(crate) fn mark_journal_cleanup(root: &FsPath) -> Result<(), Error> {
    if !journal_cleanup_pending(root)? {
        platform::durable::replace(&root.join(JOURNAL_CLEANUP_FILE), JOURNAL_CLEANUP_MARKER, 0o600).map_err(provider)?;
    }
    Ok(())
}

fn journal_cleanup_done(root: &FsPath) -> Result<(), Error> {
    if journal_cleanup_pending(root)? {
        fs::remove_file(root.join(JOURNAL_CLEANUP_FILE)).map_err(io)?;
        platform::durable::sync_dir(root).map_err(provider)?;
    }
    Ok(())
}

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

/// Full filesystem rekey already rewrites these blob bytes. Preserve their logical pointer identity.
pub(crate) fn rekey_payload(context: &str, value: &[u8], old: &ProjectSecret, new: &Keybag) -> Result<Vec<u8>, Error> {
    let Some(reference) = payload(value)? else { return Err(error("vault_journal", "expected a private journal pointer")) };
    if reference.context != context || reference.project_id != old.project_id() || reference.project_id != new.info().project_id { return Err(error("vault_journal", "private journal rekey context mismatch")); }
    if open_payload(context, value)?.is_none() { return Err(locked(&current_root(&reference))); }
    Ok(value.to_vec())
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
                say(&root, "activity", || format!("its idle clock restarts on {} {}", request.method(), request.uri().path()));
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
