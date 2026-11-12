//! Optional local encrypted project sessions. A registered path never becomes a plaintext fallback mount.
use std::{collections::{BTreeMap, BTreeSet}, fs, io::Read as _, path::{Path as FsPath, PathBuf}, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, SystemTime}};
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
/// What wakes the vault clock (`start`) beside its next deadline: a session began or is closing, its mount ended, work on a project began
/// or ended (`stir`).
static CLOCK: LazyLock<Option<platform::alarm::Alarm>> = LazyLock::new(|| {
    platform::alarm::Alarm::new().inspect_err(|e| tracing::warn!("vaults have no clock, so none locks itself: {e}")).ok()
});
/// While a session is closing, how soon its close is tried again: programs may hold its view.
const CLOSE_AGAIN: Duration = Duration::from_secs(1);

pub(crate) fn stir() {
    if let Some(clock) = &*CLOCK { clock.wake(); }
}


struct Session {
    identity: MountIdentity,
    mount: Mounted,
    /// The mount's folder under `vaults/`: it goes once the mount is gone.
    runtime: PathBuf,
    secret: ProjectSecret,
    keybag: Keybag,
    idle: u64,
    maximum: u64,
    timer: SessionTimer,
    active_ms: i64,
    began_ms: i64,
    closing: bool,
    readable: Arc<AtomicBool>,
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
        if super::own_folder(&project.path) || recover_conversion(&project.path).is_err() || encrypted(&project.path)
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

fn exclusive(root: &FsPath) -> Result<tokio::sync::OwnedRwLockWriteGuard<()>, Error> {
    access(root).try_write_owned().map_err(|_| Error::new(Kind::Conflict, "project_busy", "a project request is still running; retry after it finishes"))
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

/// An already accepted walker may finish while viewing is closing, but only on its own mount.
pub(super) async fn admit_work(project: &super::Project) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let guard = access(&project.root).read_owned().await;
    if project.mount_id != mount_identity(&project.root) || resolve_path(&project.root)? != project.folder.dir {
        return Err(Error::new(Kind::Conflict, "project_view_changed", "the project's mounted view changed; reopen this project"));
    }
    Ok(guard)
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
        if let Some(root) = path.ancestors().find(|root| encrypted(root)) {
            super::refuse_own_folder(root)?;
            return Ok(root.to_owned());
        }
        return super::named(api, named);
    }
    Ok(super::known(api, named)?.path)
}

fn status(api: &Api, root: &FsPath) -> VaultStatus {
    let mut state = if encrypted(root) { "locked" } else { "plain" };
    let (mut view, mut expires, mut idle) = (None, None, DEFAULT_IDLE);
    // Work in progress holds the clocks, as the vault clock holds them: the idle one starts when it ends, and the maximum waits for it.
    let working = held(root).is_some() && working(api, root);
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
            let active = if working { super::now_ms() } else { session.active_ms };
            let idle_at = (session.idle > 0).then(|| active.saturating_add(milliseconds(session.idle)));
            let max_at = (session.maximum > 0 && !working).then(|| session.began_ms.saturating_add(milliseconds(session.maximum)));
            expires = match (idle_at, max_at) { (Some(a), Some(b)) => Some(a.min(b)), (a,b) => a.or(b) };
        }
    }
    let next = match state { "locked" => vec!["cozy vault unlock".into()], "locking" => vec!["close applications using the project, then lock again".into()], _ => vec![] };
    VaultStatus { encrypted: encrypted(root), state: state.into(), path: root.to_owned(), view, expires_at_ms: expires, idle_timeout_s: idle, next }
}

fn milliseconds(seconds: u64) -> i64 { i64::try_from(seconds).unwrap_or(i64::MAX).saturating_mul(1000) }

fn expired(session: &Session) -> bool { session.timer.expired() }

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

fn mount(api: &Api, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk) -> Result<(), Error> {
    if let Some(known) = api.calcifer.store().known_project(&keybag.info().project_id).map_err(|e| error("vault_journal", e))? {
        if known.path != root && Keybag::load(&known.path.join(KEYBAG_FILE)).is_ok_and(|other| other.info().project_id == keybag.info().project_id) {
            return Err(Error::new(Kind::Conflict, "vault_project_copy", "this encrypted project's original location still exists; move it away before restoring a copy"));
        }
    }
    if KEY_ROOTS.lock().expect("vault project roots").get(&keybag.info().project_id).is_some_and(|known| known != root && held(known).is_some()) {
        return Err(Error::new(Kind::Conflict, "vault_project_copy", "another location of this encrypted project is unlocked"));
    }
    // IDs are document data, not filesystem path components (legacy/imported IDs may be arbitrary).
    let runtime_id = hex::encode(Sha256::digest(keybag.info().project_id.as_bytes()));
    let runtime = Runtime::new(vaults(api).join(runtime_id).join(fresh()))?;
    let view = runtime.view()?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    let mounted = secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
    let canonical_view = platform::durable::canonical(mounted.view()).map_err(provider)?;
    ROOTS.lock().expect("vault roots").insert(mounted.view().to_owned(), root.to_owned());
    ROOTS.lock().expect("vault roots").insert(canonical_view.clone(), root.to_owned());
    crate::private_io::descriptor_for_mounted(&canonical_view, &keybag.info().project_id, keybag.info().generation)?;
    TRUSTED_KEYS.lock().expect("trusted vault recipients").insert(root.to_owned(), keybag.clone());
    KEY_ROOTS.lock().expect("vault project roots").insert(keybag.info().project_id.clone(), root.to_owned());
    let project_id = keybag.info().project_id;
    let idle = ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE);
    let maximum = ask.max_duration_s.unwrap_or(0);
    let session = Session { identity: MountIdentity(Arc::new(())), mount: mounted, runtime: runtime.keep(), secret, keybag, idle, maximum, timer: timer(idle, maximum), active_ms: super::now_ms(), began_ms: super::now_ms(), closing: false, readable: Arc::new(AtomicBool::new(true)) };
    // Its supervisor's exit is its mount's end: a provider dying, an unmount from outside.
    let supervisor = session.mount.supervisor().map_err(provider)?;
    SESSIONS.lock().expect("vault sessions").insert(root.to_owned(), Arc::new(Mutex::new(session)));
    let _ = std::thread::Builder::new().name("vault-mount".into()).spawn(move || {
        let _ = supervisor.wait();
        stir();
    });
    stir();
    crate::events::project_vault(&project_id, "unlocked");
    Ok(())
}

fn forget(api: &Api, root: &FsPath) -> Result<(), Error> {
    let available = !encrypted(root) || held(root).is_some_and(|session| session.lock().expect("vault session").mount.is_mounted().unwrap_or(false));
    let landing = super::landing(root);
    let active = api.calcifer.store().read(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE out_dir = ?1 AND state <> 'settled')", [landing.to_string_lossy().as_ref()], |row| row.get::<_, bool>(0))?))
        .map_err(|e| error("vault_journal", e))?;
    if active { return Err(Error::new(Kind::Conflict, "project_busy", "The project has active work; finish or cancel it before changing storage.").next("cozy run ls").next("cozy run cancel <run>")); }
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
    session.readable.store(false, Ordering::Release);
    if changed {
        crate::events::project_vault(session.secret.project_id(), "locking");
        stir();
    }
}

fn close(api: &Arc<Api>, root: &FsPath) -> Result<(), Error> {
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        mark_closing(&mut session);
    }
    vault_operation::revoke(root);
    // Existing accepted work may drain while the key is retained, but no new viewing stream can read it.
    forget(api, root)?;
    tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?;
    crate::runs::files::release_private(root)?;
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        session.mount.try_unmount().map_err(|why| match why {
            project_mount::Error::Busy(_) => Error::new(Kind::Conflict, "project_busy", "close programs using the project, then retry the storage change"),
            other => provider(other),
        })?;
        retire_runtime(&session.runtime);
        drop(session);
        SESSIONS.lock().expect("vault sessions").remove(root);
        ROOTS.lock().expect("vault roots").retain(|_, logical| logical != root);
        if let Ok(keybag) = Keybag::load(&root.join(KEYBAG_FILE)) { crate::events::project_vault(&keybag.info().project_id, "locked"); }
    }
    Ok(())
}

pub(crate) fn close_all(api: &Arc<Api>) -> Result<(), Error> {
    let roots: Vec<_> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
    for root in roots {
        let op = operation(&root);
        let _operation = op.lock().expect("vault operation");
        if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
        let _access = exclusive(&root)?;
        close(api, &root)?;
    }
    Ok(())
}

/// The vault clock: each open vault's idle and maximum clocks and its mount, looked at on its next deadline (a wall-clock alarm, which a
/// suspend doesn't put off) or a stir, and closed once it's done. With no vault open it waits for one.
pub(crate) fn start(api: &Arc<Api>) {
    let calcifer = api.machines.hubs.home().calcifer();
    let mut runs = api.calcifer.subscribe();
    // A run beginning or settling holds or frees its project's vault.
    tokio::spawn(async move {
        loop {
            match runs.recv().await {
                Ok((_, api::calcifer::Event::Created { .. } | api::calcifer::Event::Completed { .. } | api::calcifer::Event::Failed { .. } | api::calcifer::Event::Canceled { .. }))
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => stir(),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    let runtime = tokio::runtime::Handle::current();
    let api = Arc::downgrade(api);
    let clock = std::thread::Builder::new().name("vault-clock".into()).spawn(move || {
        let Some(clock) = &*CLOCK else { return };
        // A provider an earlier Calcifer left can take a moment to let its view go: swept each second, at most 120 times.
        let mut sweeping = 120;
        loop {
            let Some(api) = api.upgrade() else { return };
            if sweeping > 0 { sweeping = if sweep_runtime(&calcifer) { 0 } else { sweeping - 1 }; }
            let mut next = (sweeping > 0).then(|| SystemTime::now() + Duration::from_secs(1));
            let roots: Vec<PathBuf> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
            for root in roots {
                if let Some(at) = keep(&api, &runtime, root) { next = Some(next.map_or(at, |next| next.min(at))); }
            }
            drop(api);
            if let Err(e) = clock.wait(next) {
                return tracing::warn!("the vault clock stops: {e}");
            }
        }
    });
    if let Err(e) = clock { tracing::warn!("no vault clock, so no vault locks itself: {e}"); }
}

/// One open vault's clocks and mount: when to look at it again, or None once it's closing and its close is going.
fn keep(api: &Arc<Api>, runtime: &tokio::runtime::Handle, root: PathBuf) -> Option<SystemTime> {
    let op = operation(&root);
    // An operation going may change it: looked at again once it's let go.
    let Ok(guard) = op.try_lock() else { return Some(SystemTime::now() + CLOSE_AGAIN) };
    let work = work(api, &root);
    let working = work.is_some();
    let (needs, next) = held(&root).map_or((false, None), |session| {
        let mut session = session.lock().expect("vault session");
        if session.mount.reconcile().is_err() { mark_closing(&mut session); }
        // Work in progress holds it open, its maximum included: the clocks go on when the last of it ends. Only an explicit lock ends
        // work (owner 11-11: "runs count as activity").
        let was = session.timer.held();
        session.timer.hold(working);
        if (working || was) && !session.closing { session.active_ms = super::now_ms(); }
        (session.closing || expired(&session) || !session.mount.is_mounted().unwrap_or(false), session.timer.deadline())
    });
    if !needs { return if work == Some(false) { Some(SystemTime::now() + CLOSE_AGAIN) } else { next }; }
    drop(guard);
    let api = api.clone();
    // One slow export/unmount must not delay another project's deadline.
    runtime.spawn_blocking(move || {
        let op = operation(&root);
        let Ok(_operation) = op.try_lock() else { return };
        if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
        if let Ok(_access) = exclusive(&root) && close(&api, &root).is_ok() { stir(); }
    });
    // Tried again while it stays: programs may hold its view.
    Some(SystemTime::now() + CLOSE_AGAIN)
}

pub(super) async fn read(State(api): State<Arc<Api>>, Path(named): Path<String>) -> Result<Response, Refusal> {
    let root = location(&api, &named).map_err(Refusal)?;
    let _access = access(&root).read_owned().await;
    Ok(private(axum::Json(status(&api, &root)).into_response()))
}

pub(super) async fn write(State(api): State<Arc<Api>>, Path(named): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let mut ask: VaultAsk = parse(&body?)?;
    let root = location(&api, &named).map_err(Refusal)?;
    let changed = tokio::task::block_in_place(|| operate(&api, &root, &mut ask, &mut Progress::quiet())).map_err(Refusal)?;
    Ok(private(axum::Json(changed).into_response()))
}

pub(super) fn operate(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk, progress: &mut Progress) -> Result<VaultChanged, Error> {
    progress.phase("waiting");
    let op = operation(&root); let _guard = op.lock().expect("vault operation");
    let mut ended = vec![];
    if ask.action == "lock" {
        if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
        // An explicit lock ends the project's work first, so nothing goes on billing behind a locked vault.
        ended = end_work(&api, &root)?;
    }
    let _access = exclusive(&root)?;
    progress.phase("preparing");
    let mut changed = match change(&api, &root, ask, progress) {
        // What it ended is still settling: the session is closing, and the sweeper locks it once it has.
        Err(busy) if !ended.is_empty() && busy.code == "project_busy" => VaultChanged { vault: status(api, &root), recovery_secret: None, ended: vec![], skipped_files: vec![] },
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
        let runtime = Runtime::new(vaults(api).join(format!("new-{}", fresh())))?;
        let view = runtime.view()?;
        let mut target = created.secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes,
            MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
        let initialized = super::new_database(target.view(), id, name, written);
        target.try_unmount().map_err(provider)?;
        drop((target, runtime));
        initialized?;
        platform::durable::sync_dir(root).map_err(provider)?;
        let ask = VaultAsk { action: "encrypt".into(), passphrase: None, recovery_secret: None, idle_timeout_s: None, max_duration_s: None };
        mount(api, root, created.keybag, created.secret, &ask)
    })();
    outcome.map_err(|why| {
        let next = if root.join(KEYBAG_FILE).is_file() { "Keep its backing, keybag and passphrase. Setup is unfinished; inspect the retained storage before retrying in a new empty directory." }
            else { "No project state was written. The unfinished backing is preserved; inspect it before retrying in a new empty directory." };
        error("vault_creation", format!("Encrypted project setup at {} could not finish: {}. {next}", root.display(), why.message))
    })?;
    Ok(recovery)
}

fn change(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk, progress: &mut Progress) -> Result<VaultChanged, Error> {
    recover_under_guards(root)?;
    require_transition_ready(root, &ask.action)?;
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
            convert(api, root, created.keybag, created.secret, ask, progress)?;
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
                    session.readable.store(false, Ordering::Release);
                    session.readable = Arc::new(AtomicBool::new(true));
                    session.began_ms = super::now_ms(); session.active_ms = session.began_ms;
                    session.idle = ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE); session.maximum = ask.max_duration_s.unwrap_or(0);
                    session.timer = timer(session.idle, session.maximum);
                    stir();
                } else {
                    session.mount.try_unmount().map_err(provider)?;
                    retire_runtime(&session.runtime);
                    drop(session);
                    SESSIONS.lock().expect("vault sessions").remove(root);
                    mount(api, root, keybag, secret, ask)?;
                }
            } else { mount(api, root, keybag, secret, ask)?; }
            progress.phase("protecting_history");
            api.calcifer.store().protect_project_journal(root).map_err(|e| error("vault_journal", format!("the project is unlocked, but protecting its history failed: {e}")))?;
            let completing = receipt_path(root).is_file();
            if completing || journal_cleanup_pending(root)? {
                progress.phase("compacting_history");
                api.calcifer.store().cleanup_private_journal().map_err(|e| error("vault_journal_cleanup", format!("the project is unlocked, but history cleanup is still pending: {e}")))?;
                progress.phase("cleaning_up");
                journal_cleanup_done(root)?;
                if completing { finish_conversion(root)?; }
            }
            if let Some(session) = held(root) { crate::events::project_vault(session.lock().expect("vault session").secret.project_id(), "unlocked"); }
        }
        "lock" => {
            if !encrypted(root) { return Err(Error::new(Kind::Usage, "project_plain", "unencrypted projects cannot be locked")); }
            close(api, root)?;
        }
        "activity" => {
            if let Some(session) = held(root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { if session.timer.activity() { session.active_ms = super::now_ms(); } } }
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
        "decrypt" => decrypt(api, root, progress)?,
        "rotate" => {
            let session = held(root).ok_or_else(|| locked(root))?;
            let created = session.lock().expect("vault session").keybag.new_generation(new_passphrase(ask)?).map_err(key)?;
            recovery_secret = Some(created.recovery_secret.expose().to_owned());
            convert(api, root, created.keybag, created.secret, ask, progress)?;
        }
        _ => return Err(Error::new(Kind::Usage, "vault_action", "unknown local vault action")),
    }
    Ok(VaultChanged { vault: status(api, root), recovery_secret, ended: vec![], skipped_files: progress.skipped() })
}

/// Whether the project has work in progress: a run not settled, or a render, cut or file move going.
fn working(api: &Api, root: &FsPath) -> bool {
    work(api, root).is_some()
}

/// The project's work in progress: true when its end stirs the vault clock (a run settling, a walk ending), false for a cut or a file
/// move alone, which is looked at again shortly.
fn work(api: &Api, root: &FsPath) -> Option<bool> {
    let landing = super::landing(root);
    let runs = api.calcifer.store().read(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE out_dir = ?1 AND state <> 'settled')", [landing.to_string_lossy().as_ref()], |row| row.get::<_, bool>(0))?));
    // A journal that can't be read holds the vault open rather than lock a render.
    let open = api.projects.now_open();
    let open = open.iter().filter(|project| project.root == root);
    if runs.unwrap_or(true) || open.clone().any(|project| *project.walking.lock().expect("walking lock")) {
        return Some(true);
    }
    open.into_iter().any(|project| project.moving.try_lock().is_err() || project.cutting.try_lock().is_err()).then_some(false)
}

/// An explicit lock ends the project's work in progress: what its renders still owe dropped, and each of its runs not
/// settled canceled. The runs it canceled, by number.
fn end_work(api: &Arc<Api>, root: &FsPath) -> Result<Vec<u64>, Error> {
    let mut ended = BTreeSet::new();
    let all = [super::format::ALL.to_owned()];
    for project in api.projects.now_open().iter().filter(|project| project.root == root) {
        let cancelled = super::cancelled(api, project, &all, &[], &|name| format!("{name}: the project was locked")).map_err(|refused| refused.0)?;
        ended.extend(cancelled.canceled);
    }
    let landing = super::landing(root);
    let store = api.calcifer.store();
    let ids = store.read(|conn| {
        let mut unsettled = conn.prepare("SELECT id FROM runs WHERE out_dir = ?1 AND state <> 'settled'")?;
        let ids = unsettled.query_map([landing.to_string_lossy().as_ref()], |row| row.get::<_, String>(0))?;
        Ok(ids.collect::<Result<Vec<_>, _>>()?)
    }).map_err(|e| error("vault_journal", e))?;
    for id in ids {
        let Some(row) = store.run(&id).map_err(|e| error("vault_journal", e))? else { continue };
        api.calcifer.cancel(&row.id).map_err(|e| Error::new(Kind::Internal, "run_cancel", e.to_string()))?;
        ended.insert(row.number);
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

/// Only paths created by this conversion are ever recovered or retired. The receipt is durable before
/// its stage exists and before either rename, so whatever a failure or crash leaves, recovery owns.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Conversion {
    root: PathBuf, staged: PathBuf, saved: PathBuf, key_id: String,
    /// The project folder the cutover moves to `saved`.
    source_id: [u64; 2],
    /// Recorded as soon as the stage exists; until then the stage can only be empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staged_id: Option<[u64; 2]>,
    /// The stage is whole and on disk for good: the cutover may begin.
    #[serde(default)]
    ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plain: Option<PlainConversion>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PlainConversion {
    project_id: String,
    generation: u64,
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
    platform::durable::replace(&receipt_path(&record.root), &serde_json::to_vec(record).map_err(|e| error("vault_recovery", e))?, 0o600).map_err(provider)
}

fn receipt_path(root: &FsPath) -> PathBuf {
    let name = hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()));
    root.parent().unwrap_or(root).join(format!(".cozy-vault-{name}.transaction.json"))
}

fn conversion(root: &FsPath) -> Result<Option<Conversion>, Error> {
    let path = receipt_path(root);
    if !exists(&path)? { return Ok(None); }
    if fs::symlink_metadata(&path).map_err(io)?.file_type().is_symlink() { return Err(error("vault_recovery", "conversion receipt cannot be a symlink")); }
    let bytes = fs::read(path).map_err(io)?;
    let record: Conversion = serde_json::from_slice(&bytes).map_err(|_| error("vault_recovery", "invalid conversion receipt"))?;
    let parent = root.parent();
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
    if record.plain.is_some() { return recover_plain(&record); }
    recover_sealed(&record)
}

/// Encrypt and rekey. Until the cutover the original is authoritative: an unfinished stage goes with its receipt.
/// After it the replacement is: a missing second rename completes, and the source goes once an unlock proves a
/// credential opens the replacement (`finish_conversion`).
fn recover_sealed(record: &Conversion) -> Result<(), Error> {
    let target = |path: &FsPath| Keybag::load(&path.join(KEYBAG_FILE)).is_ok_and(|bag| bag.info().key_id == record.key_id);
    if target(&record.root) { return Ok(()); }
    if exists(&record.root)? && !exists(&record.saved)? {
        remove_stage(record)?;
        return clear_conversion(record);
    }
    if !exists(&record.root)? && exists(&record.saved)? && record.ready && target(&record.staged) {
        fs::rename(&record.staged, &record.root).map_err(io)?;
        return sync_parent(record);
    }
    Err(error("vault_recovery", format!("conversion needs recovery; the original is at {}", record.saved.display())))
}

/// A sealed conversion that failed before its cutover leaves the original as it was, and nothing beside it.
fn abandon(root: &FsPath, mut why: Error) -> Error {
    let cleaned = conversion(root).and_then(|record| record.map_or(Ok(()), |record| recover_sealed(&record)));
    if let Err(cleanup) = cleaned { why.message = format!("{}; {}", why.message, cleanup.message); }
    why
}

fn finish_conversion(root: &FsPath) -> Result<(), Error> {
    let Some(record) = conversion(root)? else { return Ok(()); };
    if record.plain.is_some() { return Err(error("vault_recovery", "a plaintext conversion must finish its own recorded cleanup; retry decrypt")); }
    if Keybag::load(&root.join(KEYBAG_FILE)).map_err(key)?.info().key_id != record.key_id { return Err(error("vault_recovery", "replacement keybag does not match the verified conversion")); }
    remove_source(&record)?;
    clear_conversion(&record)
}

/// The stage goes whole once its identity is recorded; before that, only while it is still empty.
fn remove_stage(record: &Conversion) -> Result<(), Error> {
    if !exists(&record.staged)? { return Ok(()); }
    let removed = match record.staged_id {
        Some(id) if directory_id(&record.staged)? == id => remove_tree(&record.staged),
        Some(_) => return Err(error("vault_recovery", format!("the conversion stage {} was replaced; preserve it before retrying", record.staged.display()))),
        None => fs::remove_dir(&record.staged),
    };
    removed.map_err(|why| error("vault_cleanup_pending", format!("cleanup of the unfinished stage {} is pending: {why}", record.staged.display())))?;
    sync_parent(record)
}

/// The source the cutover moved aside goes durably, before its receipt can.
fn remove_source(record: &Conversion) -> Result<(), Error> {
    if !exists(&record.saved)? { return Ok(()); }
    if directory_id(&record.saved)? != record.source_id { return Err(error("vault_recovery", "the moved-aside source was replaced; cleanup refused")); }
    remove_tree(&record.saved).map_err(|why| error("vault_cleanup_pending", format!("the replacement verified, but cleanup of the old source {} is pending: {why}", record.saved.display())))?;
    sync_parent(record)
}

/// A folder a conversion owns is removed whole: one left read-only is made writable first.
fn remove_tree(path: &FsPath) -> std::io::Result<()> {
    if fs::remove_dir_all(path).is_ok() || fs::symlink_metadata(path).is_err_and(|why| why.kind() == std::io::ErrorKind::NotFound) { return Ok(()); }
    writable(path);
    fs::remove_dir_all(path)
}

fn writable(path: &FsPath) {
    let Ok(metadata) = fs::symlink_metadata(path) else { return };
    if metadata.file_type().is_symlink() { return; }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt as _; permissions.set_mode(permissions.mode() | 0o700); }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    let _ = fs::set_permissions(path, permissions);
    if metadata.is_dir() && let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() { writable(&entry.path()); }
    }
}

fn sync_parent(record: &Conversion) -> Result<(), Error> {
    platform::durable::sync_dir(record.root.parent().ok_or_else(|| error("vault_root", "project has no parent"))?).map_err(provider)
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
    if directory_id(path)? != record.source_id { return Ok(false); }
    let bag = Keybag::load(&path.join(KEYBAG_FILE)).map_err(key)?.info();
    Ok(bag.key_id == record.key_id && bag.project_id == plain.project_id && bag.generation == plain.generation)
}

fn plain_target(record: &Conversion, path: &FsPath) -> Result<bool, Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if !exists(path)? { return Ok(false); }
    Ok(record.staged_id == Some(directory_id(path)?) && !exists(&path.join(KEYBAG_FILE))?
        && plain_project_id(path).as_deref() == Some(&plain.project_id))
}

fn clear_conversion(record: &Conversion) -> Result<(), Error> {
    fs::remove_file(receipt_path(&record.root)).map_err(io)?;
    sync_parent(record)
}

/// The original encrypted directory remains authoritative until both renames commit.
/// A durable copying intent also owns an incomplete stage, so ordinary failures can
/// immediately remove partial plaintext rather than leaving it until the next launch.
fn recover_plain(record: &Conversion) -> Result<(), Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if plain.generation == 0 || plain.project_id.is_empty() { return Err(error("vault_recovery", "invalid plaintext conversion identity")); }
    if original_plain_source(record, &record.root)? && !exists(&record.saved)? {
        remove_stage(record)?;
        return clear_conversion(record);
    }
    if record.ready && !exists(&record.root)? && original_plain_source(record, &record.saved)? && plain_target(record, &record.staged)? {
        fs::rename(&record.staged, &record.root).map_err(io)?;
        sync_parent(record)?;
    }
    if !record.ready || !plain_target(record, &record.root)? {
        return Err(error("vault_recovery", format!("plaintext conversion needs recovery; preserved source: {}; stage: {}", record.saved.display(), record.staged.display())));
    }
    finish_plain(record)
}

fn recover_pending_plain(root: &FsPath) -> Result<(), Error> {
    let record = conversion(root)?.filter(|record| record.plain.is_some())
        .ok_or_else(|| error("vault_recovery", "the durable plaintext conversion receipt is unavailable"))?;
    recover_plain(&record)
}

fn finish_plain(record: &Conversion) -> Result<(), Error> {
    let plain = record.plain.as_ref().expect("plaintext conversion");
    if !record.ready || !plain_target(record, &record.root)? { return Err(error("vault_recovery", "the plaintext replacement did not verify")); }
    if exists(&record.saved)? {
        if directory_id(&record.saved)? != record.source_id
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
        remove_source(record)?;
    }
    clear_conversion(record)
}

/// A mount's folder under `vaults/`. It is never removed recursively: a view still mounted refuses.
struct Runtime(PathBuf);

/// The runtime folders this process made and has not retired: a sweep never touches them, or what holds them.
static OWNED: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

impl Runtime {
    fn new(path: PathBuf) -> Result<Self, Error> {
        OWNED.lock().expect("vault runtime folders").insert(path.clone());
        let runtime = Self(path);
        fs::create_dir_all(&runtime.0).map_err(io)?;
        platform::durable::set_mode(&runtime.0, 0o700).map_err(provider)?;
        Ok(runtime)
    }
    fn view(&self) -> Result<PathBuf, Error> { project_mount::view_path(&self.0).map_err(provider) }
    /// A session's mount outlives this call; its close removes the folder.
    fn keep(mut self) -> PathBuf { std::mem::take(&mut self.0) }
}

impl Drop for Runtime {
    fn drop(&mut self) { if !self.0.as_os_str().is_empty() { retire_runtime(&self.0); } }
}

fn vaults(api: &Api) -> PathBuf { api.machines.hubs.home().calcifer().join("vaults") }

/// Removes an unmounted runtime folder, and its project's folder once that is empty.
fn retire_runtime(runtime: &FsPath) {
    let _ = fs::remove_dir(runtime.join("view"));
    let _ = fs::remove_dir(runtime);
    if let Some(parent) = runtime.parent().filter(|parent| parent.file_name().is_some_and(|name| name != "vaults")) {
        let _ = fs::remove_dir(parent);
    }
    if !runtime.exists() { OWNED.lock().expect("vault runtime folders").remove(runtime); }
}

/// An earlier process's runtime folder, its view detached first if that process's provider left it mounted.
fn retire_stale(runtime: &FsPath) {
    if let Ok(view) = project_mount::view_path(runtime) { project_mount::detach_stale(&view); }
    retire_runtime(runtime);
}

/// Every runtime folder this process doesn't own is an earlier process's. True once none is left.
pub(crate) fn sweep_runtime(calcifer: &FsPath) -> bool {
    let owned = OWNED.lock().expect("vault runtime folders").clone();
    let holds = |path: &FsPath| owned.iter().any(|held| held.starts_with(path));
    let Ok(entries) = fs::read_dir(calcifer.join("vaults")) else { return true };
    let mut clean = true;
    for path in entries.flatten().map(|entry| entry.path()) {
        if let Ok(children) = fs::read_dir(&path) {
            for child in children.flatten().map(|child| child.path()) {
                if child.file_name().is_some_and(|name| name == "view") || holds(&child) { continue; }
                retire_stale(&child);
                clean &= !child.exists();
            }
        }
        if holds(&path) { continue; }
        retire_stale(&path);
        clean &= !path.exists();
    }
    clean
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

fn decrypt(api: &Arc<Api>, root: &FsPath, progress: &mut Progress) -> Result<(), Error> {
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
    close(api, root)?;
    let parent = root.parent().ok_or_else(|| error("vault_root", "cannot decrypt a filesystem root"))?;
    let suffix = fresh();
    let staged = parent.join(format!(".cozy-vault-stage-{suffix}"));
    let mut record = Conversion {
        root: root.to_owned(), staged: staged.clone(), saved: parent.join(format!(".cozy-project-source-{suffix}")),
        key_id: info.key_id, source_id: directory_id(root)?, staged_id: None, ready: false,
        plain: Some(PlainConversion { project_id: info.project_id.clone(), generation: info.generation, cleanup_started: false }),
    };
    save_conversion(&record)?;
    let outcome = (|| {
        fs::create_dir(&staged).map_err(io)?;
        record.staged_id = Some(directory_id(&staged)?);
        save_conversion(&record)?;
        platform::durable::set_mode(&staged, 0o700).map_err(provider)?;
        let runtime = Runtime::new(vaults(api).join(format!("decrypt-{suffix}")))?;
        let source_view = runtime.view()?;
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
        sync_tree(&staged)?;
        if !plain_target(&record, &staged)? { return Err(error("vault_identity", "the plaintext snapshot did not retain the project identity")); }
        source.try_unmount().map_err(provider)?;
        drop((source, runtime));
        record.ready = true;
        save_conversion(&record)?;
        if !original_plain_source(&record, root)? || exists(&record.saved)? { return Err(error("vault_recovery", "the encrypted source changed before cutover")); }
        progress.phase("committing");
        fs::rename(root, &record.saved).map_err(io)?;
        platform::durable::sync_dir(parent).map_err(provider)?;
        fs::rename(&staged, root).map_err(io)?;
        platform::durable::sync_dir(parent).map_err(provider)?;
        finish_plain(&record)
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
    let nested = api.calcifer.store().known_projects().map_err(|e| error("vault_journal", e))?
        .into_iter().any(|project| project.path != root && project.path.starts_with(root) && project.path.exists());
    if nested || api.projects.now_open().iter().any(|project| project.root != root && project.root.starts_with(root)) {
        return Err(Error::new(Kind::Conflict, "vault_nested_project", "move nested projects outside this project before converting its storage"));
    }
    Ok(())
}

fn convert(api: &Arc<Api>, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk, progress: &mut Progress) -> Result<(), Error> {
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
    let converting = Converting(root.to_owned());
    forget(api, root)?;
    if old_secret.is_none() {
        tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?;
        crate::runs::files::release_private(root)?;
    }
    // Checkpoint before measuring: the source DB's file length must include
    // committed WAL pages, and the inventory must match what the copy retains.
    progress.begin_inventory();
    walk_tree(&source, None, FsPath::new(""), managed, false, progress)?;
    progress.inventory_done();
    let parent = root.parent().ok_or_else(|| Error::new(Kind::Usage, "vault_root", "cannot convert a filesystem root"))?;
    let suffix = fresh();
    let staged = parent.join(format!(".cozy-vault-stage-{suffix}"));
    let saved = parent.join(format!(".cozy-project-source-{suffix}"));
    let mut record = Conversion { root: root.to_owned(), staged: staged.clone(), saved: saved.clone(), key_id: target_keybag.info().key_id,
        source_id: directory_id(root)?, staged_id: None, ready: false, plain: None };
    save_conversion(&record)?;
    let copied = (|| {
        fs::create_dir(&staged).map_err(io)?;
        record.staged_id = Some(directory_id(&staged)?);
        save_conversion(&record)?;
        platform::durable::set_mode(&staged, 0o700).map_err(provider)?;
        let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
        secret.with_bytes(|bytes| engine.create(&staged.join(BACKING), bytes)).map_err(provider)?;
        keybag.save(&staged.join(KEYBAG_FILE)).map_err(key)?;
        // Retire the published old view BEFORE snapshotting an encrypted project. A native editor
        // must release its stale handles (Busy refuses). The replacement source mount is never
        // published to apps. Windows lacks a qualified read-only provider: do not browse conversion
        // drives while this runs; Linux/macOS enforce a read-only source volume.
        let mut source_runtime = None;
        let mut source_mount = None;
        let source = if let Some(old) = &old_secret {
            close(api, root)?;
            let view = source_runtime.insert(Runtime::new(vaults(api).join(format!("source-{suffix}")))?).view()?;
            let mounted = old.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: !cfg!(windows) })).map_err(provider)?;
            let path = mounted.view().to_owned();
            source_mount = Some(mounted);
            path
        } else { source.clone() };
        let runtime = Runtime::new(vaults(api).join(format!("stage-{suffix}")))?;
        let view = runtime.view()?;
        progress.phase("mounting");
        let mut target = secret.with_bytes(|bytes| engine.mount(&staged.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
        progress.phase("copying");
        walk_tree(&source, Some(target.view()), FsPath::new(""), managed, source_mount.is_some() && !cfg!(windows), progress)?;
        progress.phase("verifying");
        target.try_unmount().map_err(provider)?;
        if let Some(mut source) = source_mount.take() { source.try_unmount().map_err(provider)?; }
        drop((target, runtime, source_runtime));
        // The original goes only after its replacement is on disk for good.
        sync_tree(&staged)?;
        record.ready = true;
        save_conversion(&record)
    })();
    copied.map_err(|why| abandon(root, why))?;
    progress.phase("committing");
    fs::rename(root, &saved).map_err(|why| abandon(root, error("vault_io", format!("move the project aside for its encrypted copy: {why}"))))?;
    platform::durable::sync_dir(parent).map_err(provider)?;
    if let Err(e) = fs::rename(&staged, root) { return Err(error("vault_recovery", format!("the original is at {}; reopen the project to complete conversion: {e}", saved.display()))); }
    platform::durable::sync_dir(parent).map_err(provider)?;
    // From here the replacement is the project. The original stays until it opens: here, or at the next unlock.
    progress.phase("opening");
    mount(api, root, keybag, secret, ask)?;
    drop(converting);
    progress.phase("protecting_history");
    let journal = match old_secret {
        Some(old) => api.calcifer.store().rekey_project_journal(root, &old, &target_keybag),
        None => api.calcifer.store().protect_project_journal(root),
    };
    journal.map_err(|e| error("vault_journal", format!("encrypted files are in place; the original goes at the next unlock, which completes the journal conversion: {e}")))?;
    progress.phase("compacting_history");
    api.calcifer.store().cleanup_private_journal().map_err(|e| error("vault_journal_cleanup", format!("encrypted data is in place; the original goes at the next unlock, which completes journal cleanup: {e}")))?;
    progress.phase("cleaning_up");
    journal_cleanup_done(root)?;
    finish_conversion(root)
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
    for entry in fs::read_dir(source).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        let from = entry.path();
        let relative = relative.join(entry.file_name());
        let top = relative.components().next().and_then(|part| part.as_os_str().to_str());
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
            if first == Some(api::machine::LOCAL_PRIVATE_MARKER) || first == Some(".vault-calcifer")
                || (first == Some(".vault-machine") && matches!(second,
                    Some("editable" | "generations" | "executors" | "home" | "jit" | "kernels" | "tmp" | "cache" | "matplotlib" | "uv-cache" | "uv-tools"))) { continue; }
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
    let root = params.get("id").and_then(|id| location(&api, id).ok());
    let Some(root) = root else { return next.run(request).await };
    let _access = access(&root).read_owned().await;
    if let Err(error) = view(&root) { return private(Refusal(error).into_response()); }
    if !encrypted(&root) { return next.run(request).await; }
    let token = held(&root).map(|session| session.lock().expect("vault session").readable.clone());
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        if let Some(session) = held(&root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { if session.timer.activity() { session.active_ms = super::now_ms(); } } }
    }
    protect_reply(next.run(request).await, token)
}

/// Project-scoped APIs outside the project router use the same response boundary.
pub(crate) async fn project_reply<T: IntoResponse>(
    api: &Api,
    named: Option<&str>,
    answer: impl std::future::Future<Output = Result<T, Refusal>>,
) -> Result<Response, Refusal> {
    let Some(named) = named else { return answer.await.map(IntoResponse::into_response) };
    let root = location(api, named).map_err(Refusal)?;
    let _access = admit_root(&root).await.map_err(Refusal)?;
    let encrypted = encrypted(&root);
    let token = readable_token(&root);
    let response = answer.await.into_response();
    Ok(if encrypted { protect_reply(response, token) } else { response })
}

fn protect_reply(response: Response, token: Option<Arc<AtomicBool>>) -> Response {
    use futures_util::StreamExt as _;
    let response = private(response);
    let Some(token) = token else { return response };
    let (parts, body) = response.into_parts();
    let body = body.into_data_stream().take_while(move |_| futures_util::future::ready(token.load(Ordering::Acquire)));
    Response::from_parts(parts, axum::body::Body::from_stream(body))
}
