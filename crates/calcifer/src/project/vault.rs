//! Optional local encrypted project sessions. A registered path never becomes a plaintext fallback mount.
use std::{collections::{BTreeMap, BTreeSet}, fs, io::Read as _, path::{Path as FsPath, PathBuf}, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::Duration};
use axum::{body::Bytes, extract::{Path, State, rejection::BytesRejection}, http::{HeaderValue, header}, response::{IntoResponse as _, Response}};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultStatus}, error::{Error, Kind}};
use project_mount::{MountOptions, Mounted, Provider};
use project_vault::{Credential, Keybag, ProjectSecret, SecretString, SessionPolicy, SessionTimer, KEYBAG_FILE};
use sha2::{Digest as _, Sha256};
use crate::api::{Api, Refusal, parse};

pub(super) const BACKING: &str = "project.vault.data";
const DEFAULT_IDLE: u64 = 900;
type Held = Arc<Mutex<Session>>;
static SESSIONS: LazyLock<Mutex<BTreeMap<PathBuf, Held>>> = LazyLock::new(Mutex::default);
static ROOTS: LazyLock<Mutex<BTreeMap<PathBuf, PathBuf>>> = LazyLock::new(Mutex::default);
static OPERATIONS: LazyLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);
static ACCESS: LazyLock<Mutex<BTreeMap<PathBuf, Arc<tokio::sync::RwLock<()>>>>>= LazyLock::new(Mutex::default);
static CONVERTING: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

struct Session {
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
}

fn error(code: &'static str, why: impl std::fmt::Display) -> Error { Error::new(Kind::Unavailable, code, why.to_string()) }
fn io(e: std::io::Error) -> Error { error("vault_io", e) }
fn provider(e: impl std::fmt::Display) -> Error { error("vault_provider", e) }
fn key(e: project_vault::Error) -> Error { error("vault_key", e) }
fn held(root: &FsPath) -> Option<Held> { SESSIONS.lock().expect("vault sessions").get(root).cloned() }
pub(crate) fn encrypted(root: &FsPath) -> bool { root.join(KEYBAG_FILE).is_file() || SESSIONS.lock().expect("vault sessions").contains_key(root) }

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
    if !encrypted(root) { return Ok(root.to_owned()); }
    let Some(session) = held(root) else { return Err(locked(root)); };
    let mut session = session.lock().expect("vault session");
    if session.mount.reconcile().is_err() { mark_closing(&mut session); }
    if session.closing || expired(&session) || !session.mount.is_mounted().map_err(provider)? { return Err(locked(root)); }
    platform::durable::canonical(session.mount.view()).map_err(provider)
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
    let guard = access(&root).read_owned().await;
    view(&root)?;
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

fn locked(root: &FsPath) -> Error {
    Error::new(Kind::Conflict, "project_locked", "the project is locked")
        .next(format!("cozy project -p {} unlock", root.display()))
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

fn status(root: &FsPath) -> VaultStatus {
    let mut state = if encrypted(root) { "locked" } else { "plain" };
    let (mut view, mut expires, mut idle) = (None, None, 0);
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        if session.mount.reconcile().is_err() { mark_closing(&mut session); }
        idle = session.idle;
        if session.mount.is_mounted().unwrap_or(false) {
            state = if session.closing || expired(&session) { "locking" } else { "unlocked" };
            if state == "unlocked" { view = Some(session.mount.view().to_owned()); }
            let idle_at = (session.idle > 0).then(|| session.active_ms.saturating_add(milliseconds(session.idle)));
            let max_at = (session.maximum > 0).then(|| session.began_ms.saturating_add(milliseconds(session.maximum)));
            expires = match (idle_at, max_at) { (Some(a), Some(b)) => Some(a.min(b)), (a,b) => a.or(b) };
        }
    }
    let next = match state { "locked" => vec![format!("cozy project -p {} unlock", root.display())], "locking" => vec!["close applications using the project, then lock again".into()], _ => vec![] };
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
    let runtime = api.machines.hubs.home().calcifer().join("vaults").join(runtime_id).join(fresh());
    fs::create_dir_all(&runtime).map_err(io)?;
    platform::durable::set_mode(&runtime, 0o700).map_err(provider)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
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
    let session = Session { mount: mounted, secret, keybag, idle, maximum, timer: timer(idle, maximum), active_ms: super::now_ms(), began_ms: super::now_ms(), closing: false, readable: Arc::new(AtomicBool::new(true)) };
    SESSIONS.lock().expect("vault sessions").insert(root.to_owned(), Arc::new(Mutex::new(session)));
    crate::events::project_vault(&project_id, "unlocked");
    Ok(())
}

fn forget(api: &Api, root: &FsPath) -> Result<(), Error> {
    let available = !encrypted(root) || held(root).is_some_and(|session| session.lock().expect("vault session").mount.is_mounted().unwrap_or(false));
    let landing = super::landing(root);
    let active = api.calcifer.store().read(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE out_dir = ?1 AND state <> 'settled')", [landing.to_string_lossy().as_ref()], |row| row.get::<_, bool>(0))?))
        .map_err(|e| error("vault_journal", e))?;
    if active { return Err(Error::new(Kind::Conflict, "project_busy", "the project has active work; finish or cancel it before locking or converting")); }
    let projects = api.projects.now_open();
    for project in projects {
        if logical(&project.folder.dir) != root { continue; }
        if *project.walking.lock().expect("walking lock") || project.moving.try_lock().is_err() || project.cutting.try_lock().is_err() {
            return Err(Error::new(Kind::Conflict, "project_busy", "the project is rendering, cutting or transferring; finish or cancel it before changing its storage").next("cozy project cancel all"));
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
    if changed { crate::events::project_vault(session.secret.project_id(), "locking"); }
}

fn close(api: &Arc<Api>, root: &FsPath) -> Result<(), Error> {
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        mark_closing(&mut session);
    }
    // Existing accepted work may drain while the key is retained, but no new viewing stream can read it.
    forget(api, root)?;
    tokio::runtime::Handle::current().block_on(crate::private_io::close_private_project(api, root))?;
    crate::runs::files::release_private(root)?;
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        session.mount.try_unmount().map_err(provider)?;
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

pub(crate) fn start(api: &Arc<Api>) {
    let api = Arc::downgrade(api);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(api) = api.upgrade() else { return };
            let roots: Vec<PathBuf> = SESSIONS.lock().expect("vault sessions").keys().cloned().collect();
            for root in roots {
                let needs = tokio::task::block_in_place(|| held(&root).is_some_and(|session| {
                    let mut session = session.lock().expect("vault session");
                    if session.mount.reconcile().is_err() { mark_closing(&mut session); }
                    session.closing || expired(&session) || !session.mount.is_mounted().unwrap_or(false)
                }));
                if needs { let _ = tokio::task::block_in_place(|| {
                    let op = operation(&root); let _guard = op.lock().expect("vault operation");
                    let still = held(&root).is_some_and(|session| { let session = session.lock().expect("vault session"); session.closing || expired(&session) || !session.mount.is_mounted().unwrap_or(false) });
                    if !still { return Ok(()); }
                    if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); }
                    let _access = exclusive(&root)?;
                    close(&api, &root)
                }); }
            }
        }
    });
}

pub(super) async fn read(State(api): State<Arc<Api>>, Path(named): Path<String>) -> Result<Response, Refusal> {
    let root = location(&api, &named).map_err(Refusal)?;
    Ok(private(axum::Json(status(&root)).into_response()))
}

pub(super) async fn write(State(api): State<Arc<Api>>, Path(named): Path<String>, body: Result<Bytes, BytesRejection>) -> Result<Response, Refusal> {
    let mut ask: VaultAsk = parse(&body?)?;
    let root = location(&api, &named).map_err(Refusal)?;
    let changed = tokio::task::block_in_place(|| {
        let op = operation(&root); let _guard = op.lock().expect("vault operation");
        if ask.action == "lock" { if let Some(session) = held(&root) { mark_closing(&mut session.lock().expect("vault session")); } }
        let _access = exclusive(&root)?;
        change(&api, &root, &mut ask)
    }).map_err(Refusal)?;
    Ok(private(axum::Json(changed).into_response()))
}

fn private(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn change(api: &Arc<Api>, root: &FsPath, ask: &mut VaultAsk) -> Result<VaultChanged, Error> {
    let (mut recovery_secret, mut preserved_source) = (None, None);
    match ask.action.as_str() {
        "encrypt" => {
            if encrypted(root) { return Err(Error::new(Kind::Conflict, "project_encrypted", "the project is already encrypted")); }
            let project = api.projects.open(api, root)?;
            let id = project.id.clone();
            drop(project);
            let created = Keybag::create(&id, new_passphrase(ask)?).map_err(key)?;
            recovery_secret = Some(created.recovery_secret.expose().to_owned());
            preserved_source = Some(convert(api, root, created.keybag, created.secret, ask)?);
            if ask.remove_plaintext {
                preserved_source = None;
            }
        }
        "unlock" => {
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
                    session.readable = Arc::new(AtomicBool::new(true));
                    session.began_ms = super::now_ms(); session.active_ms = session.began_ms;
                    session.idle = ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE); session.maximum = ask.max_duration_s.unwrap_or(0);
                    session.timer = timer(session.idle, session.maximum);
                } else {
                    drop(session);
                    SESSIONS.lock().expect("vault sessions").remove(root);
                    mount(api, root, keybag, secret, ask)?;
                }
            } else { mount(api, root, keybag, secret, ask)?; }
            api.calcifer.store().protect_project_journal(root).map_err(|e| error("vault_journal", e))?;
            if receipt_path(root).is_file() {
                api.calcifer.store().cleanup_private_journal().map_err(|e| error("vault_journal_cleanup", e))?;
                finish_conversion(root)?;
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
        "export" => {
            let source = view(root)?;
            let output = ask.output.as_ref().filter(|path| path.is_absolute()).ok_or_else(|| Error::new(Kind::Usage, "vault_export_path", "plaintext export needs a new absolute output directory"))?;
            if output.exists() { return Err(Error::new(Kind::Conflict, "vault_export_exists", "plaintext export destination already exists")); }
            fs::create_dir(output).map_err(io)?;
            copy_tree(&source, output)?;
        }
        "rotate" => {
            let session = held(root).ok_or_else(|| locked(root))?;
            let created = session.lock().expect("vault session").keybag.new_generation(new_passphrase(ask)?).map_err(key)?;
            recovery_secret = Some(created.recovery_secret.expose().to_owned());
            convert(api, root, created.keybag, created.secret, ask)?;
        }
        _ => return Err(Error::new(Kind::Usage, "vault_action", "unknown local vault action")),
    }
    Ok(VaultChanged { vault: status(root), recovery_secret, preserved_source })
}

struct Converting(PathBuf);
impl Drop for Converting { fn drop(&mut self) { CONVERTING.lock().expect("vault conversions").remove(&self.0); } }

/// Only paths created by this conversion are ever recovered or retired. This receipt is durable
/// before either rename, so losing Calcifer between them cannot lose the registered project path.
#[derive(serde::Serialize, serde::Deserialize)]
struct Conversion { root: PathBuf, staged: PathBuf, saved: PathBuf, key_id: String, remove_source: bool }

fn receipt_path(root: &FsPath) -> PathBuf {
    let name = hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()));
    root.parent().unwrap_or(root).join(format!(".cozy-vault-{name}.transaction.json"))
}

fn conversion(root: &FsPath) -> Result<Option<Conversion>, Error> {
    let path = receipt_path(root);
    if !path.exists() { return Ok(None); }
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
    if CONVERTING.lock().expect("vault conversions").contains(root) { return Ok(()); }
    let Some(record) = conversion(root)? else { return Ok(()); };
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
    if Keybag::load(&root.join(KEYBAG_FILE)).map_err(key)?.info().key_id != record.key_id { return Err(error("vault_recovery", "replacement keybag does not match the verified conversion")); }
    if record.remove_source && record.saved.exists() {
        fs::remove_dir_all(&record.saved).map_err(|e| error("vault_cleanup_pending", format!("replacement verified, but old source cleanup is pending at {}: {e}", record.saved.display())))?;
    }
    fs::remove_file(receipt_path(root)).map_err(io)?;
    platform::durable::sync_dir(root.parent().ok_or_else(|| error("vault_root", "project has no parent"))?).map_err(provider)
}

fn convert(api: &Arc<Api>, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk) -> Result<PathBuf, Error> {
    let source = view(root)?;
    // A child project has its own activity, connections and transition gate. Never move/delete
    // its live storage as an incidental part of converting its parent.
    let nested = api.calcifer.store().known_projects().map_err(|e| error("vault_journal", e))?
        .into_iter().any(|project| project.path != root && project.path.starts_with(root) && project.path.exists());
    if nested || api.projects.now_open().iter().any(|project| {
        let path = logical(&project.folder.dir);
        path != root && path.starts_with(root)
    }) { return Err(Error::new(Kind::Conflict, "vault_nested_project", "move nested projects outside this project before converting its storage")); }
    let old_secret = held(root).map(|session| session.lock().expect("vault session").secret.duplicate());
    let target_keybag = keybag.clone();
    if !CONVERTING.lock().expect("vault conversions").insert(root.to_owned()) { return Err(error("vault_busy", "the project is already being converted")); }
    let _converting = Converting(root.to_owned());
    forget(api, root)?;
    let parent = root.parent().ok_or_else(|| Error::new(Kind::Usage, "vault_root", "cannot convert a filesystem root"))?;
    let suffix = fresh();
    let staged = parent.join(format!(".cozy-vault-stage-{suffix}"));
    let saved = parent.join(format!(".cozy-project-source-{suffix}"));
    fs::create_dir(&staged).map_err(io)?;
    platform::durable::set_mode(&staged, 0o700).map_err(provider)?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    secret.with_bytes(|bytes| engine.create(&staged.join(BACKING), bytes)).map_err(provider)?;
    keybag.save(&staged.join(KEYBAG_FILE)).map_err(key)?;
    // Retire the published old view BEFORE snapshotting an encrypted project. A native editor
    // must release its stale handles (Busy refuses). The replacement source mount is never
    // published to apps. Windows lacks a qualified read-only provider: do not browse conversion
    // drives while this runs; Linux/macOS enforce a read-only source volume.
    let mut source_mount = None;
    let source = if let Some(old) = &old_secret {
        close(api, root)?;
        let runtime = api.machines.hubs.home().calcifer().join("vaults").join(format!("source-{suffix}"));
        fs::create_dir_all(&runtime).map_err(io)?;
        platform::durable::set_mode(&runtime, 0o700).map_err(provider)?;
        let view = project_mount::view_path(&runtime).map_err(provider)?;
        let mounted = old.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: !cfg!(windows) })).map_err(provider)?;
        let path = mounted.view().to_owned();
        source_mount = Some(mounted);
        path
    } else { source };
    let runtime = api.machines.hubs.home().calcifer().join("vaults").join(format!("stage-{suffix}"));
    fs::create_dir_all(&runtime).map_err(io)?;
    platform::durable::set_mode(&runtime, 0o700).map_err(provider)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
    let mut target = secret.with_bytes(|bytes| engine.mount(&staged.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
    if source_mount.is_some() { copy_tree_at(&source, target.view(), FsPath::new(""), true, !cfg!(windows))?; }
    else { copy_tree(&source, target.view())?; }
    target.try_unmount().map_err(provider)?;
    if let Some(mut source) = source_mount.take() { source.try_unmount().map_err(provider)?; }
    let receipt = Conversion { root: root.to_owned(), staged: staged.clone(), saved: saved.clone(), key_id: target_keybag.info().key_id, remove_source: old_secret.is_some() || ask.remove_plaintext };
    platform::durable::replace(&receipt_path(root), &serde_json::to_vec(&receipt).map_err(|e| error("vault_recovery", e))?, 0o600).map_err(provider)?;
    fs::rename(root, &saved).map_err(io)?;
    platform::durable::sync_dir(parent).map_err(provider)?;
    if let Err(e) = fs::rename(&staged, root) { return Err(error("vault_recovery", format!("source preserved at {}; retry opening the project to complete conversion: {e}", saved.display()))); }
    platform::durable::sync_dir(parent).map_err(provider)?;
    // The source is still intact if reopening fails. It is never deleted before the verified switch.
    mount(api, root, keybag, secret, ask)?;
    drop(_converting);
    let journal = match old_secret {
        Some(old) => api.calcifer.store().rekey_project_journal(root, &old, &target_keybag),
        None => api.calcifer.store().protect_project_journal(root),
    };
    journal.map_err(|e| error("vault_journal", format!("encrypted files are in place; source preserved at {}; journal conversion needs recovery: {e}", saved.display())))?;
    api.calcifer.store().cleanup_private_journal().map_err(|e| error("vault_journal_cleanup", format!("encrypted data is in place; source preserved at {}; journal cleanup is pending: {e}", saved.display())))?;
    finish_conversion(root)?;
    Ok(saved)
}

fn copy_tree(source: &FsPath, target: &FsPath) -> Result<(), Error> {
    // Executable environments contain interpreter symlinks. They are reproducible caches,
    // not retained content. Only omit them for a verified managed view, never a plain user folder.
    let logical = logical(source);
    let managed = Keybag::load(&logical.join(KEYBAG_FILE)).ok().is_some_and(|bag| {
        fs::read(source.join(api::machine::LOCAL_PRIVATE_MARKER)).ok()
            .and_then(|bytes| serde_json::from_slice::<api::machine::LocalPrivateMarker>(&bytes).ok())
            .is_some_and(|marker| marker.format_version == 1 && marker.storage.project_id == bag.info().project_id
                && marker.storage.generation == bag.info().generation
                && platform::durable::canonical(source).is_ok_and(|path| path == marker.storage.view_root))
    });
    copy_tree_at(source, target, FsPath::new(""), managed, false)
}

fn copy_tree_at(source: &FsPath, target: &FsPath, relative: &FsPath, managed: bool, frozen: bool) -> Result<(), Error> {
    for entry in fs::read_dir(source).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let relative = relative.join(entry.file_name());
        if entry.file_name() == KEYBAG_FILE || (entry.file_name() == "project.db" && relative.parent().is_some_and(|parent| !parent.as_os_str().is_empty())) {
            return Err(Error::new(Kind::Conflict, "vault_nested_project", "conversion/export refuses a nested project's storage"));
        }
        if managed {
            let mut parts = relative.components();
            let first = parts.next().and_then(|part| part.as_os_str().to_str());
            let second = parts.next().and_then(|part| part.as_os_str().to_str());
            if first == Some(api::machine::LOCAL_PRIVATE_MARKER) || first == Some(".vault-calcifer")
                || (first == Some(".vault-machine") && matches!(second,
                    Some("editable" | "generations" | "executors" | "home" | "jit" | "kernels" | "tmp" | "cache" | "matplotlib" | "uv-cache" | "uv-tools"))) { continue; }
        }
        if kind.is_symlink() { return Err(Error::new(Kind::Usage, "vault_symlink", "conversion/export refuses symlinks; replace them with project-owned files")); }
        if kind.is_dir() { fs::create_dir(&to).map_err(io)?; copy_tree_at(&from, &to, &relative, managed, frozen)?; }
        else if kind.is_file() {
            if entry.file_name() == "project.db-wal" || entry.file_name() == "project.db-shm" { continue; }
            if entry.file_name() == "project.db" {
                if frozen {
                    if fs::metadata(from.with_file_name("project.db-wal")).is_ok_and(|meta| meta.len() > 0) { return Err(error("vault_database", "read-only project snapshot was not fully checkpointed")); }
                    fs::copy(&from, &to).map_err(io)?;
                    if hash(&from)? != hash(&to)? { return Err(error("vault_database", "frozen project database copy did not verify")); }
                } else {
                    let conn = rusqlite::Connection::open_with_flags(&from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| error("vault_database", format!("open source snapshot: {e}")))?;
                    conn.backup(rusqlite::MAIN_DB, &to, rusqlite::backup::NO_PROGRESS).map_err(|e| error("vault_database", format!("backup project snapshot: {e}")))?;
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
                fs::copy(&from, &to).map_err(io)?;
                if hash(&from)? != hash(&to)? { return Err(error("vault_copy_mismatch", "a copied project file changed or did not verify")); }
            }
        } else { return Err(Error::new(Kind::Usage, "vault_file_type", "conversion/export accepts ordinary files and directories only")); }
    }
    Ok(())
}

fn hash(path: &FsPath) -> Result<[u8;32], Error> {
    let mut file = fs::File::open(path).map_err(io)?;
    let mut sha = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop { let n = file.read(&mut bytes).map_err(io)?; if n == 0 { break; } sha.update(&bytes[..n]); }
    Ok(sha.finalize().into())
}

fn fresh() -> String { let mut bytes = [0u8;16]; getrandom::fill(&mut bytes).expect("OS randomness"); hex::encode(bytes) }

const PAYLOAD_PREFIX: &[u8] = b"cozy:vault:journal:2:";
const JOURNAL_DIR: &str = ".vault-journal";
const MAX_JOURNAL_BYTES: u64 = 32 << 20;
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
    let Some(session) = held(&root) else { return Ok(None) };
    let mut session = session.lock().expect("vault session");
    if session.secret.project_id() != reference.project_id { return Err(error("vault_journal", "private journal project mismatch")); }
    session.mount.reconcile().map_err(provider)?;
    if !session.mount.is_mounted().map_err(provider)? { return Err(locked(&root)); }
    let view = session.mount.view().to_owned();
    drop(session);
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
    use futures_util::StreamExt as _;
    let root = params.get("id").and_then(|id| location(&api, id).ok());
    let Some(root) = root else { return next.run(request).await };
    let _access = access(&root).read_owned().await;
    if let Err(error) = view(&root) { return private(Refusal(error).into_response()); }
    if !encrypted(&root) { return next.run(request).await; }
    let token = held(&root).map(|session| session.lock().expect("vault session").readable.clone());
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        if let Some(session) = held(&root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { if session.timer.activity() { session.active_ms = super::now_ms(); } } }
    }
    let response = private(next.run(request).await);
    let Some(token) = token else { return response };
    let (parts, body) = response.into_parts();
    let body = body.into_data_stream().take_while(move |_| futures_util::future::ready(token.load(Ordering::Acquire)));
    Response::from_parts(parts, axum::body::Body::from_stream(body))
}
