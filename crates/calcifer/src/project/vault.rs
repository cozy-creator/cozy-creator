//! Optional local encrypted project sessions. A registered path never becomes a plaintext fallback mount.
use std::{collections::{BTreeMap, BTreeSet}, fs, io::Read as _, path::{Path as FsPath, PathBuf}, sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use axum::{body::Bytes, extract::{Path, State, rejection::BytesRejection}, http::{HeaderValue, header}, response::{IntoResponse as _, Response}};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultStatus}, error::{Error, Kind}};
use project_mount::{MountOptions, Mounted, Provider};
use project_vault::{Credential, Keybag, ProjectSecret, SecretString, KEYBAG_FILE};
use sha2::{Digest as _, Sha256};
use crate::api::{Api, Refusal, parse};

pub(super) const BACKING: &str = "project.vault.data";
const DEFAULT_IDLE: u64 = 900;
type Held = Arc<Mutex<Session>>;
static SESSIONS: LazyLock<Mutex<BTreeMap<PathBuf, Held>>> = LazyLock::new(Mutex::default);
static ROOTS: LazyLock<Mutex<BTreeMap<PathBuf, PathBuf>>> = LazyLock::new(Mutex::default);
static OPERATIONS: LazyLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);
static CONVERTING: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

struct Session {
    root: PathBuf,
    mount: Mounted,
    secret: ProjectSecret,
    keybag: Keybag,
    idle: u64,
    maximum: u64,
    began: Instant,
    active: Instant,
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
pub(crate) fn encrypted(root: &FsPath) -> bool { root.join(KEYBAG_FILE).is_file() }

/// Logical registered path of an unlocked mount, including any relative subpath.
pub(crate) fn logical(path: &FsPath) -> PathBuf {
    for (view, root) in ROOTS.lock().expect("vault roots").iter() {
        if let Ok(rest) = path.strip_prefix(view) { return root.join(rest); }
    }
    path.to_owned()
}

/// The private view for a registered encrypted project. No keybag means an ordinary project.
pub(super) fn view(root: &FsPath) -> Result<PathBuf, Error> {
    if CONVERTING.lock().expect("vault conversions").contains(root) { return Err(error("vault_busy", "the project's storage is being converted")); }
    if !encrypted(root) { return Ok(root.to_owned()); }
    let Some(session) = held(root) else { return Err(locked(root)); };
    let session = session.lock().expect("vault session");
    if session.closing || expired(&session) || !session.mount.is_mounted().map_err(provider)? { return Err(locked(root)); }
    Ok(session.mount.view().to_owned())
}

/// A key-holding work lease can drain already-authorized work while the viewing session is locking.
fn work_view(root: &FsPath) -> Result<PathBuf, Error> {
    let Some(session) = held(root) else { return Err(locked(root)); };
    let session = session.lock().expect("vault session");
    // The provider guards its underlying path against plaintext fallback; lost mounts fail I/O.
    Ok(session.mount.view().to_owned())
}

fn operation(root: &FsPath) -> Arc<Mutex<()>> {
    OPERATIONS.lock().expect("vault operations").entry(root.to_owned()).or_default().clone()
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
        let path = path.canonicalize().map_err(io)?;
        if let Some(root) = path.ancestors().find(|root| encrypted(root)) { return Ok(root.to_owned()); }
        return super::named(api, named);
    }
    Ok(super::known(api, named)?.path)
}

fn status(root: &FsPath) -> VaultStatus {
    let mut state = if encrypted(root) { "locked" } else { "plain" };
    let (mut view, mut expires, mut idle) = (None, None, 0);
    if let Some(session) = held(root) {
        let session = session.lock().expect("vault session");
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

fn expired(session: &Session) -> bool {
    (session.idle > 0 && (session.active.elapsed() >= Duration::from_secs(session.idle) || super::now_ms().saturating_sub(session.active_ms) >= milliseconds(session.idle)))
        || (session.maximum > 0 && (session.began.elapsed() >= Duration::from_secs(session.maximum)
            || super::now_ms().saturating_sub(session.began_ms) >= milliseconds(session.maximum)))
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
    let runtime = api.machines.hubs.home().calcifer().join("vaults").join(&keybag.info().project_id);
    fs::create_dir_all(&runtime).map_err(io)?;
    platform::durable::set_mode(&runtime, 0o700).map_err(provider)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
    let engine = Provider::discover(&api.machines.hubs.home().tools()).map_err(provider)?;
    let mounted = secret.with_bytes(|bytes| engine.mount(&root.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
    ROOTS.lock().expect("vault roots").insert(mounted.view().to_owned(), root.to_owned());
    TRUSTED_KEYS.lock().expect("trusted vault recipients").insert(root.to_owned(), keybag.clone());
    KEY_ROOTS.lock().expect("vault project roots").insert(keybag.info().project_id.clone(), root.to_owned());
    let now = Instant::now();
    let session = Session { root: root.to_owned(), mount: mounted, secret, keybag, idle: ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE), maximum: ask.max_duration_s.unwrap_or(0), began: now, active: now, active_ms: super::now_ms(), began_ms: super::now_ms(), closing: false, readable: Arc::new(AtomicBool::new(true)) };
    SESSIONS.lock().expect("vault sessions").insert(root.to_owned(), Arc::new(Mutex::new(session)));
    Ok(())
}

fn forget(api: &Api, root: &FsPath) -> Result<(), Error> {
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
        api.projects.open.lock().expect("projects lock").remove(&project.id);
        crate::events::project_closed(&project.id);
    }
    Ok(())
}

fn close(api: &Api, root: &FsPath) -> Result<(), Error> {
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        session.closing = true;
        session.readable.store(false, Ordering::Release);
    }
    // Existing accepted work may drain while the key is retained, but no new viewing stream can read it.
    forget(api, root)?;
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        if session.mount.is_mounted().map_err(provider)? { session.mount.try_unmount().map_err(provider)?; }
        drop(session);
        SESSIONS.lock().expect("vault sessions").remove(root);
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
                let needs = held(&root).is_some_and(|session| { let session = session.lock().expect("vault session"); session.closing || expired(&session) || !session.mount.is_mounted().unwrap_or(false) });
                if needs { let _ = tokio::task::block_in_place(|| {
                    let op = operation(&root); let _guard = op.lock().expect("vault operation");
                    let still = held(&root).is_some_and(|session| { let session = session.lock().expect("vault session"); session.closing || expired(&session) || !session.mount.is_mounted().unwrap_or(false) });
                    if still { close(&api, &root) } else { Ok(()) }
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
                if let Some(source) = &preserved_source { fs::remove_dir_all(source).map_err(io)?; }
                preserved_source = None;
            }
        }
        "unlock" => {
            if !encrypted(root) { return Err(Error::new(Kind::Usage, "project_plain", "this project is not encrypted")); }
            if held(root).is_some() { return Err(Error::new(Kind::Conflict, "vault_session_exists", "the project already has an unlock session; lock it before changing its policy")); }
            let keybag = Keybag::load(&root.join(KEYBAG_FILE)).map_err(key)?;
            let secret = keybag.unlock(credential(ask)?).map_err(key)?;
            mount(api, root, keybag, secret, ask)?;
        }
        "lock" => { close(api, root)?; }
        "activity" => {
            if let Some(session) = held(root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { session.active = Instant::now(); session.active_ms = super::now_ms(); } }
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
            session.active = Instant::now(); session.active_ms = super::now_ms();
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
            preserved_source = Some(convert(api, root, created.keybag, created.secret, ask)?);
        }
        _ => return Err(Error::new(Kind::Usage, "vault_action", "unknown local vault action")),
    }
    Ok(VaultChanged { vault: status(root), recovery_secret, preserved_source })
}

struct Converting(PathBuf);
impl Drop for Converting { fn drop(&mut self) { CONVERTING.lock().expect("vault conversions").remove(&self.0); } }

fn convert(api: &Arc<Api>, root: &FsPath, keybag: Keybag, secret: ProjectSecret, ask: &VaultAsk) -> Result<PathBuf, Error> {
    let source = view(root)?;
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
    let runtime = api.machines.hubs.home().calcifer().join("vaults").join(format!("stage-{suffix}"));
    fs::create_dir_all(&runtime).map_err(io)?;
    platform::durable::set_mode(&runtime, 0o700).map_err(provider)?;
    let view = project_mount::view_path(&runtime).map_err(provider)?;
    let mut target = secret.with_bytes(|bytes| engine.mount(&staged.join(BACKING), &view, bytes, MountOptions { idle_timeout: None, read_only: false })).map_err(provider)?;
    copy_tree(&source, target.view())?;
    target.try_unmount().map_err(provider)?;
    if encrypted(root) { close(api, root)?; }
    fs::rename(root, &saved).map_err(io)?;
    if let Err(e) = fs::rename(&staged, root) { let _ = fs::rename(&saved, root); return Err(io(e)); }
    // The source is still intact if reopening fails. It is never deleted before the verified switch.
    mount(api, root, keybag, secret, ask)?;
    Ok(saved)
}

fn copy_tree(source: &FsPath, target: &FsPath) -> Result<(), Error> {
    for entry in fs::read_dir(source).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if kind.is_symlink() { return Err(Error::new(Kind::Usage, "vault_symlink", "conversion/export refuses symlinks; replace them with project-owned files")); }
        if kind.is_dir() { fs::create_dir(&to).map_err(io)?; copy_tree(&from, &to)?; }
        else if kind.is_file() {
            if entry.file_name() == "project.db-wal" || entry.file_name() == "project.db-shm" { continue; }
            if entry.file_name() == "project.db" {
                let conn = rusqlite::Connection::open_with_flags(&from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| error("vault_database", e))?;
                conn.backup(rusqlite::MAIN_DB, &to, rusqlite::backup::NO_PROGRESS).map_err(|e| error("vault_database", e))?;
                let copied = rusqlite::Connection::open_with_flags(&to, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| error("vault_database", e))?;
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
    if !dir.canonicalize().map_err(io)?.starts_with(view.canonicalize().map_err(io)?) { return Err(error("vault_journal", "private journal escapes its mounted project")); }
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

pub(crate) fn open_payload(context: &str, value: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    use base64::Engine as _;
    let Some(reference) = payload(value)? else { return Ok(Some(value.to_vec())) };
    if reference.context != context { return Err(error("vault_journal", "private journal context mismatch")); }
    let root = current_root(&reference);
    let Some(session) = held(&root) else { return Ok(None) };
    let session = session.lock().expect("vault session");
    if session.secret.project_id() != reference.project_id { return Err(error("vault_journal", "private journal project mismatch")); }
    let view = session.mount.view().to_owned();
    drop(session);
    let at = view.join(JOURNAL_DIR).join(&reference.object);
    if fs::symlink_metadata(&at).map_err(io)?.file_type().is_symlink() { return Err(error("vault_journal", "private journal objects cannot be symlinks")); }
    if !at.canonicalize().map_err(io)?.starts_with(view.canonicalize().map_err(io)?) { return Err(error("vault_journal", "private journal object escapes its mounted project")); }
    let mut bytes = Vec::new();
    fs::File::open(&at).map_err(io)?.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES { return Err(error("vault_journal", "private journal record is too large")); }
    let private: PrivateRecord = serde_json::from_slice(&bytes).map_err(|_| error("vault_journal", "invalid private journal record"))?;
    if private.project_id != reference.project_id || private.context != context { return Err(error("vault_journal", "private journal ownership mismatch")); }
    let plain = base64::engine::general_purpose::STANDARD.decode(private.data).map_err(|_| error("vault_journal", "invalid private journal value"))?;
    if reference.root != root { ROOTS.lock().expect("vault roots").insert(reference.root, root); }
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
    let root = params.get("id").and_then(|id| location(&api, id).ok()).filter(|root| encrypted(root));
    let Some(root) = root else { return next.run(request).await };
    if let Err(error) = view(&root) { return private(Refusal(error).into_response()); }
    let token = held(&root).map(|session| session.lock().expect("vault session").readable.clone());
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        if let Some(session) = held(&root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { session.active = Instant::now(); session.active_ms = super::now_ms(); } }
    }
    let response = private(next.run(request).await);
    let Some(token) = token else { return response };
    let (parts, body) = response.into_parts();
    let body = body.into_data_stream().take_while(move |_| futures_util::future::ready(token.load(Ordering::Acquire)));
    Response::from_parts(parts, axum::body::Body::from_stream(body))
}
