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
    began_ms: i64,
    closing: bool,
    readable: Arc<AtomicBool>,
}

fn error(code: &'static str, why: impl std::fmt::Display) -> Error { Error::new(Kind::Unavailable, code, why.to_string()) }
fn io(e: std::io::Error) -> Error { error("vault_io", e) }
fn provider(e: impl std::fmt::Display) -> Error { error("vault_provider", e) }
fn key(e: project_vault::Error) -> Error { error("vault_key", e) }
fn held(root: &FsPath) -> Option<Held> { SESSIONS.lock().expect("vault sessions").get(root).cloned() }
pub(super) fn encrypted(root: &FsPath) -> bool { root.join(KEYBAG_FILE).is_file() }

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

pub(crate) fn resolve_path(path: &FsPath) -> Result<PathBuf, Error> {
    let path = logical(path);
    if let Some(root) = path.ancestors().find(|root| encrypted(root)) {
        return Ok(view(root)?.join(path.strip_prefix(root).expect("ancestor")));
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
            let idle_at = (session.idle > 0).then(|| super::now_ms() + session.idle.saturating_sub(session.active.elapsed().as_secs()) as i64 * 1000);
            let max_at = (session.maximum > 0).then(|| session.began_ms.saturating_add(session.maximum as i64 * 1000));
            expires = match (idle_at, max_at) { (Some(a), Some(b)) => Some(a.min(b)), (a,b) => a.or(b) };
        }
    }
    let next = match state { "locked" => vec![format!("cozy project -p {} unlock", root.display())], "locking" => vec!["close applications using the project, then lock again".into()], _ => vec![] };
    VaultStatus { encrypted: encrypted(root), state: state.into(), path: root.to_owned(), view, expires_at_ms: expires, idle_timeout_s: idle, next }
}

fn expired(session: &Session) -> bool {
    (session.idle > 0 && session.active.elapsed() >= Duration::from_secs(session.idle))
        || (session.maximum > 0 && (session.began.elapsed() >= Duration::from_secs(session.maximum)
            || super::now_ms().saturating_sub(session.began_ms) >= (session.maximum as i64).saturating_mul(1000)))
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
    let now = Instant::now();
    let session = Session { root: root.to_owned(), mount: mounted, secret, keybag, idle: ask.idle_timeout_s.unwrap_or(DEFAULT_IDLE), maximum: ask.max_duration_s.unwrap_or(0), began: now, active: now, began_ms: super::now_ms(), closing: false, readable: Arc::new(AtomicBool::new(true)) };
    SESSIONS.lock().expect("vault sessions").insert(root.to_owned(), Arc::new(Mutex::new(session)));
    Ok(())
}

fn forget(api: &Api, root: &FsPath) -> Result<(), Error> {
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
    forget(api, root)?;
    if let Some(session) = held(root) {
        let mut session = session.lock().expect("vault session");
        session.closing = true;
        session.readable.store(false, Ordering::Release);
        // Busy remains a truthful locking state. Never report locked while a readable mount survives.
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
                if needs { let _ = tokio::task::block_in_place(|| close(&api, &root)); }
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
    let changed = tokio::task::block_in_place(|| change(&api, &root, &mut ask)).map_err(Refusal)?;
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
            if let Some(session) = held(root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { session.active = Instant::now(); } }
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
            session.active = Instant::now();
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

const PAYLOAD_PREFIX: &[u8] = b"cozy:vault:journal:1:";
static TRUSTED_KEYS: LazyLock<Mutex<BTreeMap<PathBuf, Keybag>>> = LazyLock::new(Mutex::default);

#[derive(serde::Serialize, serde::Deserialize)]
struct ProtectedPayload {
    root: PathBuf,
    context: String,
    ciphertext: String,
}

pub(crate) fn is_payload(value: &[u8]) -> bool { value.starts_with(PAYLOAD_PREFIX) }

/// Seal private global journal fields BEFORE binding them into SQLite. Ordinary project values pass through.
pub(crate) fn seal_payload(path: &FsPath, context: &str, value: &[u8]) -> Result<Vec<u8>, Error> {
    use base64::Engine as _;
    if is_payload(value) { return Ok(value.to_vec()); }
    let path = logical(path);
    let Some(root) = path.ancestors().find(|root| encrypted(root)) else { return Ok(value.to_vec()) };
    let keybag = TRUSTED_KEYS.lock().expect("trusted vault recipients").get(root).cloned().ok_or_else(|| locked(root))?;
    let ciphertext = keybag.seal(context, value).map_err(key)?;
    let payload = ProtectedPayload { root: root.to_owned(), context: context.into(), ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext) };
    let mut encoded = PAYLOAD_PREFIX.to_vec();
    encoded.extend(serde_json::to_vec(&payload).map_err(|e| error("vault_journal", e))?);
    Ok(encoded)
}

/// None is an intentionally locked field, never a failed parse to be treated as plaintext.
pub(crate) fn open_payload(context: &str, value: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    use base64::Engine as _;
    let Some(encoded) = value.strip_prefix(PAYLOAD_PREFIX) else { return Ok(Some(value.to_vec())) };
    let payload: ProtectedPayload = serde_json::from_slice(encoded).map_err(|_| error("vault_journal", "invalid protected journal envelope"))?;
    if payload.context != context { return Err(error("vault_journal", "protected journal context mismatch")); }
    let Some(session) = held(&payload.root) else { return Ok(None) };
    let session = session.lock().expect("vault session");
    if session.closing || expired(&session) { return Ok(None); }
    let ciphertext = base64::engine::general_purpose::STANDARD.decode(&payload.ciphertext).map_err(|_| error("vault_journal", "invalid protected journal ciphertext"))?;
    Ok(Some(session.secret.open(context, &ciphertext).map_err(key)?.to_vec()))
}

/// Rewrite a protected field during full local rekey. The caller commits all replacements atomically.
pub(crate) fn rekey_payload(context: &str, value: &[u8], old: &ProjectSecret, new: &Keybag) -> Result<Vec<u8>, Error> {
    use base64::Engine as _;
    let Some(encoded) = value.strip_prefix(PAYLOAD_PREFIX) else { return Err(error("vault_journal", "expected a protected field during rekey")) };
    let mut payload: ProtectedPayload = serde_json::from_slice(encoded).map_err(|_| error("vault_journal", "invalid protected journal envelope"))?;
    if payload.context != context { return Err(error("vault_journal", "protected journal context mismatch")); }
    let ciphertext = base64::engine::general_purpose::STANDARD.decode(&payload.ciphertext).map_err(|_| error("vault_journal", "invalid protected journal ciphertext"))?;
    let plain = old.open(context, &ciphertext).map_err(key)?;
    payload.ciphertext = base64::engine::general_purpose::STANDARD.encode(new.seal(context, &plain).map_err(key)?);
    let mut out = PAYLOAD_PREFIX.to_vec();
    out.extend(serde_json::to_vec(&payload).map_err(|e| error("vault_journal", e))?);
    Ok(out)
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
        if let Some(session) = held(&root) { let mut session = session.lock().expect("vault session"); if !session.closing && !expired(&session) { session.active = Instant::now(); } }
    }
    let response = private(next.run(request).await);
    let Some(token) = token else { return response };
    let (parts, body) = response.into_parts();
    let body = body.into_data_stream().take_while(move |_| futures_util::future::ready(token.load(Ordering::Acquire)));
    Response::from_parts(parts, axum::body::Body::from_stream(body))
}
