//! Verified local encrypted-workspace selection. No peer can make an ordinary machine
//! endpoint silently write private bytes to global storage, and a missing/locked mount
//! never becomes a plaintext fallback directory.
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, File},
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use crate::storage::Store;
use api::machine::{LOCAL_PRIVATE_MARKER, LocalPrivateMarker, LocalPrivateStorage};
use sha2::{Digest, Sha256};
use tracing::instrument::WithSubscriber as _;

pub(crate) const AREA: &str = ".vault-machine";

/// Trusted parents stop dumping before proof/project bytes; unsupported hosts refuse early.
pub(crate) fn require_project_execution() -> io::Result<()> {
    platform::private_exec::disable_private_core_dumps().map_err(io::Error::from)
}

struct Verified {
    descriptor: LocalPrivateStorage,
    actor: String,
    token_hash: [u8; 32],
    store: Mutex<Option<Arc<Store>>>,
    area: Mutex<Option<File>>,
    roots: Mutex<crate::storage::Roots>,
    execution: Mutex<Option<Arc<ProjectExecution>>>,
    sealed: std::sync::atomic::AtomicBool,
    transport_prepared: std::sync::atomic::AtomicBool,
    activated: std::sync::atomic::AtomicBool,
    process_scopes: Mutex<Vec<Arc<platform::contain::Scope>>>,
    process_namespace: Mutex<Option<String>>,
}

/// Full execution ownership, including the existing Engine/Journal, belongs to the view.
struct ProjectExecution { runs: Arc<crate::runs::Runs>, leases: Arc<()> }
#[derive(Clone, Debug)]
pub(crate) struct ProjectExecutionLease { _owner: Arc<()> }

pub(crate) fn lease() -> io::Result<Option<ProjectExecutionLease>> {
    let Some(scope) = current() else { return Ok(None) };
    let verified = verified_scope(&scope)?;
    let execution = verified.execution.lock().unwrap();
    let execution = execution.as_ref().ok_or_else(denied)?;
    Ok(Some(ProjectExecutionLease { _owner: execution.leases.clone() }))
}

/// Project routes select a complete Runs/Service/Engine; the ordinary engine never sees a row.
pub(crate) fn open_execution(scope: &LocalPrivateStorage, ordinary: &Arc<crate::runs::Runs>) -> io::Result<Arc<crate::runs::Runs>> {
    let verified = verified_scope(scope)?;
    let mut execution = verified.execution.lock().unwrap();
    if verified.sealed.load(std::sync::atomic::Ordering::Acquire) { return Err(denied()); }
    if let Some(execution) = execution.as_ref() { return Ok(execution.runs.clone()); }
    let root = root(scope)?;

    let store = store(scope)?;
    let service = crate::service::Service::open_project(&root, store.clone(), &ordinary.service, scope.clone())?;
    let publisher = ordinary.publisher.for_project(&root, store)?;
    service.configure_publisher(publisher.clone());
    let objects = Arc::new(crate::objects::Objects::new(service.engine.clone()));
    service.engine.hold_memos(&objects);
    let runs = Arc::new(crate::runs::Runs { service: service.clone(), objects: objects.clone(),
        local: Arc::new(crate::local_source::LocalSources::new(objects, publisher.clone(), &root.join("generations"))),
        publisher: publisher.clone(), own_hub: ordinary.own_hub.clone(), contexts: Mutex::default(), projection: Mutex::default() });
    service.engine.on_settled(Box::new({ let runs = Arc::downgrade(&runs); move |record| { if let Some(runs) = runs.upgrade() { runs.settled(record); } } }));
    crate::calls::Calls::configure(&service, publisher.origin_machine(), Some(&runs))?;
    let owned = Arc::new(ProjectExecution { runs: runs.clone(), leases: Arc::new(()) });
    *execution = Some(owned);
    drop(execution);
    // Paused until migration/inventory and strict health prove admission. No old run/warm replay here.
    Ok(runs)
}

/// Opens only a paused project Engine. Metadata migration never executes a Runtime or replays
/// an old run/warm declaration. An ambiguous outside artifact remains a typed pending result.
pub(crate) fn migrate_execution(scope: &LocalPrivateStorage, ordinary: &Arc<crate::runs::Runs>, logs: &std::path::Path) -> io::Result<Arc<crate::runs::Runs>> {
    migrate_execution_bindings(scope, ordinary, logs, &[])
}
pub(crate) fn migrate_execution_bindings(scope: &LocalPrivateStorage, ordinary: &Arc<crate::runs::Runs>, logs: &std::path::Path, bindings: &[api::machine::ProjectExecutionMigrationBinding]) -> io::Result<Arc<crate::runs::Runs>> {
    let verified = verified_scope(scope)?;
    let runs = open_execution(scope, ordinary)?;
    if !bindings.is_empty() || runs.service.engine.needs_project_migration_from(&ordinary.service.engine, &scope.project_id)? {
        runs.service.pause_project();
        runs.service.engine.assert_private_idle(&scope.project_id)?;
        let first = runs.service.engine.migrate_project_from(&ordinary.service.engine, scope, logs, &verified.actor, bindings);
        if first.as_ref().is_err_and(|error| matches!(error.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::ResourceBusy)) {
            // Canonical rows have already moved only when the protected receipt says so.
            // Release stale in-memory roots/idle cache holders outside both journal locks,
            // then retry once. Actual independent readers remain an explicit pending result.
            let generations=runs.service.engine.retired_migration_generations(&ordinary.service.engine)?;
            within(None,|| -> io::Result<()> {
                ordinary.objects.sweep()?;
                if let Some(pool)=ordinary.service.gpu() {
                    pool.drop_members(&verified.actor,&generations);
                    for generation in &generations {pool.end_generation(generation);}
                }
                Ok(())
            })?;
            runs.service.engine.migrate_project_from(&ordinary.service.engine,scope,logs,&verified.actor,bindings)?;
        } else { first?; }
    }
    Ok(runs)
}

pub(crate) fn activate_execution(scope: &LocalPrivateStorage, runs: &Arc<crate::runs::Runs>) -> io::Result<()> {
    let verified = verified_scope(scope)?;
    if !verified.activated.swap(true, std::sync::atomic::Ordering::AcqRel) {
        if let Err(error) = crate::runs::resume_private(runs, &scope.project_id) {
            verified.activated.store(false, std::sync::atomic::Ordering::Release);
            return Err(error);
        }
    }
    runs.service.admit_project();
    Ok(())
}

/// Machine lifecycle consumes only aggregate busy/idle facts, never project/run identities.
pub(crate) fn machine_jobs(since: i64) -> io::Result<(bool, i64)> {
    let scopes: Vec<_> = VERIFIED.lock().unwrap().values().cloned().collect();
    let mut total = (false, 0);
    for scope in scopes {
        let execution = scope.execution.lock().unwrap();
        if let Some(project) = execution.as_ref() {
            let (working, ended) = project.runs.service.engine.jobs(since)?;
            total.0 |= working;
            total.1 = total.1.max(ended);
        }
    }
    Ok(total)
}

static VERIFIED: LazyLock<Mutex<HashMap<String, Arc<Verified>>>> = LazyLock::new(Mutex::default);
tokio::task_local! { static REQUEST: LocalPrivateStorage; }
thread_local! {
    static CURRENT: RefCell<Option<LocalPrivateStorage>> = const { RefCell::new(None) };
    static HTTP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "the local private workspace is locked or not authorized",
    )
}

fn marker(descriptor: &LocalPrivateStorage) -> io::Result<LocalPrivateMarker> {
    if descriptor.project_id.is_empty()
        || descriptor.project_id.len() > 256
        || descriptor.project_id.chars().any(char::is_control)
        || descriptor.generation == 0
        || !descriptor.view_root.is_absolute()
    {
        return Err(denied());
    }
    let real = platform::durable::canonical(&descriptor.view_root).map_err(|_| denied())?;
    if real != descriptor.view_root {
        return Err(denied());
    }
    let path = real.join(LOCAL_PRIVATE_MARKER);
    if fs::symlink_metadata(&path)
        .map_err(|_| denied())?
        .file_type()
        .is_symlink()
    {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| denied())?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| denied())?;
    if bytes.len() > 8192 {
        return Err(denied());
    }
    let marker: LocalPrivateMarker = serde_json::from_slice(&bytes).map_err(|_| denied())?;
    if marker.format_version != 1
        || marker.storage != *descriptor
        || marker.token.len() != 64
        || !marker.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(denied());
    }
    verify_project_identity(descriptor)?;
    Ok(marker)
}

/// The token does not authorize an arbitrary directory: its canonical native project document
/// must independently name the requested project. Projections/meta rows are not authoritative.
fn verify_project_identity(descriptor: &LocalPrivateStorage) -> io::Result<()> {
    use automerge::ReadDoc as _;
    let db = descriptor.view_root.join("project.db");
    let metadata = fs::symlink_metadata(&db).map_err(|_| denied())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() { return Err(denied()); }
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = rusqlite::Connection::open_with_flags(db, flags).map_err(|_| denied())?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(|_| denied())?;
    if version != 2 { return Err(denied()); }
    let (bytes, heads): (Vec<u8>, String) = connection.query_row("SELECT snapshot, heads FROM crdt_document WHERE id=1", [], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| denied())?;
    let mut doc = automerge::AutoCommit::load(&bytes).map_err(|_| denied())?;
    let expected: Vec<String> = serde_json::from_str(&heads).map_err(|_| denied())?;
    let expected: std::collections::BTreeSet<_> = expected.into_iter().collect();
    let actual: std::collections::BTreeSet<_> = doc.get_heads().into_iter().map(|head| head.to_string()).collect();
    if actual != expected { return Err(denied()); }
    let (_, identity) = doc.get(automerge::ROOT, "identity").map_err(|_| denied())?.ok_or_else(denied)?;
    let (id, _) = doc.get(&identity, "id").map_err(|_| denied())?.ok_or_else(denied)?;
    if id.as_str() != Some(descriptor.project_id.as_str()) { return Err(denied()); }
    Ok(())
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn verified_scope(descriptor: &LocalPrivateStorage) -> io::Result<Arc<Verified>> {
    let verified = VERIFIED
        .lock()
        .unwrap()
        .get(&descriptor.project_id)
        .cloned()
        .ok_or_else(denied)?;
    if verified.sealed.load(std::sync::atomic::Ordering::Acquire) { return Err(denied()); }
    // Historical records retain their original generation/context. Full project rekey
    // copies their retained blobs into the new encrypted backing before cutover.
    if descriptor.generation == 0 || descriptor.generation > verified.descriptor.generation {
        return Err(denied());
    }
    let current = marker(&verified.descriptor)?;
    if digest(&current.token) != verified.token_hash {
        return Err(denied());
    }
    Ok(verified)
}

/// Called only after machine/controller authentication and the local-mode check. New
/// admissions must prove the EXACT currently mounted generation. The marker's token is
/// never retained in a global journal; only its process-local hash is kept here.
pub(crate) fn register(
    descriptor: &LocalPrivateStorage,
    token: &str,
    actor: &str,
) -> io::Result<bool> {
    let present = marker(descriptor)?;
    let token_hash = digest(token);
    if digest(&present.token) != token_hash {
        return Err(denied());
    }
    let mut all = VERIFIED.lock().unwrap();
    if let Some(old) = all.get(&descriptor.project_id) {
        if old.actor != actor {
            return Err(denied());
        }
        if old.descriptor == *descriptor && old.token_hash == token_hash {
            return Ok(false);
        }
        if !old.sealed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "close the previous private execution scope before replacing it",
            ));
        }
    }
    all.insert(
        descriptor.project_id.clone(),
        Arc::new(Verified {
            descriptor: descriptor.clone(),
            actor: actor.to_owned(),
            token_hash,
            store: Mutex::new(None),
            area: Mutex::new(None),
            roots: Mutex::default(),
            execution: Mutex::default(),
            sealed: std::sync::atomic::AtomicBool::new(false),
            transport_prepared: std::sync::atomic::AtomicBool::new(false),
            activated: std::sync::atomic::AtomicBool::new(false),
            process_scopes: Mutex::default(),
            process_namespace: Mutex::default(),
        }),
    );
    Ok(true)
}

/// Resolve a verified current mount for a current or retained historical descriptor.
/// Stored physical view paths are not authoritative across mount/recovery changes.
pub(crate) fn resolve(descriptor: &LocalPrivateStorage) -> io::Result<LocalPrivateStorage> {
    Ok(verified_scope(descriptor)?.descriptor.clone())
}

/// Conservative metadata classification for generation/import/JIT probes. It grants
/// no access: actual I/O still resolves and revalidates the mounted proof each time.
pub(crate) fn is_private_path(path: &std::path::Path) -> bool {
    VERIFIED
        .lock()
        .unwrap()
        .values()
        .any(|scope| path.starts_with(&scope.descriptor.view_root))
}

pub(crate) fn root(descriptor: &LocalPrivateStorage) -> io::Result<PathBuf> {
    let scope = verified_scope(descriptor)?;
    let root = scope.descriptor.view_root.join(AREA);
    match fs::create_dir(&root) { Ok(()) => (), Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (), Err(error) => return Err(error) }
    let metadata = fs::symlink_metadata(&root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || platform::durable::canonical(&root)? != root { return Err(denied()); }
    let file = platform::durable::open_nofollow(&root, &fs::OpenOptions::new().read(true))?;
    platform::durable::set_file_mode(&file, 0o700)?;
    let mut pinned = scope.area.lock().unwrap();
    if let Some(known) = pinned.as_ref() {
        if platform::durable::FileId::of(known)? != platform::durable::FileId::of(&file)? { return Err(denied()); }
    } else { *pinned = Some(file); }
    Ok(root)
}
fn owned_directory(path: &std::path::Path) -> io::Result<()> {
    match fs::create_dir(path) { Ok(()) => (), Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (), Err(error) => return Err(error) }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || platform::durable::canonical(path)? != path { return Err(denied()); }
    let directory = platform::durable::open_nofollow(path, &fs::OpenOptions::new().read(true))?;
    platform::durable::set_file_mode(&directory, 0o700)?;
    Ok(())
}

/// Project objects, provider imports and derived views use this owned store. A source is
/// never treated as public merely because it is already cached or a provider accepted a token.
pub(crate) fn store(descriptor: &LocalPrivateStorage) -> io::Result<Arc<Store>> {
    let scope = verified_scope(descriptor)?;
    let path = root(&scope.descriptor)?.join("objects");
    owned_directory(&path)?;
    let mut held = scope.store.lock().unwrap();
    if let Some(store) = held.as_ref() {
        return Ok(store.clone());
    }
    let store = Arc::new(Store::open(path).map_err(io::Error::other)?);
    *held = Some(store.clone());
    Ok(store)
}

pub(crate) fn retain(
    descriptor: &LocalPrivateStorage,
    objects: &[tensorfs::format::ObjectRef],
) -> io::Result<()> {
    let scope = verified_scope(descriptor)?;
    let storage = store(&scope.descriptor)?;
    let retained = scope.roots.lock().unwrap().add(&storage, objects);
    retained
}

/// Root first refuses active jobs and reaps only this scope's idle executors. Then the
/// cache can be dropped so no retained upload/store handle silently keeps FUSE busy.
pub(crate) fn close(descriptor: &LocalPrivateStorage, actor: &str) -> io::Result<()> {
    let Some(scope) = VERIFIED.lock().unwrap().get(&descriptor.project_id).cloned() else { return Ok(()); };
    if scope.actor != actor || scope.descriptor.generation != descriptor.generation {
        return Err(denied());
    }
    if scope.sealed.load(std::sync::atomic::Ordering::Acquire) { return Ok(()); }
    if !platform::contain::stuck().is_empty() { return Err(io::Error::new(io::ErrorKind::ResourceBusy, "a previous kernel scope has not quiesced")); }
    let mut execution = scope.execution.lock().unwrap();
    crate::storage::close_project_views(&descriptor.project_id);
    if let Some(project) = execution.as_ref() {
        if Arc::strong_count(&project.runs) > 1 || Arc::strong_count(&project.leases) > 1 {
            return Err(io::Error::new(io::ErrorKind::ResourceBusy, "project requests or native checkpoints remain open"));
        }
        project.runs.service.engine.assert_private_idle(&descriptor.project_id)?;
        if let Some(pool) = project.runs.service.pool() { pool.close_private_namespace(descriptor)?; pool.forget_project_measurements(&project.runs.service.engine); }
        project.runs.service.engine.close_private_storage(&descriptor.project_id)?;
        project.runs.service.seal_project()?;
    }
    for process_scope in scope.process_scopes.lock().unwrap().iter() {
            if let Err(error) = process_scope.end() {
                let error: io::Error = error.into();
                if error.kind() != io::ErrorKind::NotFound { return Err(error); }
            }
            if !platform::private_exec::project_scope_quiescent(process_scope)? { return Err(io::Error::new(io::ErrorKind::ResourceBusy, "a protected descendant has not exited")); }
        }
    scope.process_scopes.lock().unwrap().clear();
    crate::access::forget_project(&root(descriptor)?.join("objects"));
    *execution = None;
    drop(execution);
    crate::storage::close_project_views(&descriptor.project_id);
    *scope.roots.lock().unwrap() = crate::storage::Roots::default();
    let mut store = scope.store.lock().unwrap();
    if store
        .as_ref()
        .is_some_and(|store| Arc::strong_count(store) > 1)
    {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            "private input/output handles are still in use",
        ));
    }
    *scope.roots.lock().unwrap() = crate::storage::Roots::default();
    *store = None;
    drop(store);
    *scope.area.lock().unwrap() = None;
    scope.sealed.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// HTTP blocking work carries the verified scope. Background work reads its immutable
/// scope from Invocation; it must never inherit another request's thread-local scope.
pub(crate) fn current() -> Option<LocalPrivateStorage> {
    CURRENT
        .with(|value| value.borrow().clone())
        .or_else(|| REQUEST.try_with(Clone::clone).ok())
}

pub(crate) fn within<T>(scope: Option<LocalPrivateStorage>, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<LocalPrivateStorage>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|current| *current.borrow_mut() = self.0.take());
        }
    }
    let private = scope.is_some();
    let restore = Restore(CURRENT.with(|current| current.replace(scope)));
    let value = if private { tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), work) } else { work() };
    drop(restore);
    value
}

/// A private HTTP future keeps its scope across awaits without leaking thread-local state
/// into an unrelated request polled by the same Tokio worker.
pub(crate) async fn request<T>(
    scope: LocalPrivateStorage,
    work: impl std::future::Future<Output = T>,
) -> T {
    REQUEST.scope(scope, work.with_subscriber(tracing::subscriber::NoSubscriber::default())).await
}

/// Distinguish an authenticated HTTP boundary (public or private) from trusted internal
/// engine work. Public HTTP must not discover private records through a global endpoint.
pub(crate) fn http_scope() -> Option<Option<LocalPrivateStorage>> {
    HTTP.with(|active| active.get().then(current))
}

pub(crate) fn within_http<T>(scope: Option<LocalPrivateStorage>, work: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            HTTP.with(|active| active.set(self.0));
        }
    }
    let reset = Reset(HTTP.with(|active| active.replace(true)));
    let result = within(scope, work);
    drop(reset);
    result
}

pub(crate) async fn request_optional<T>(
    scope: Option<LocalPrivateStorage>,
    work: impl std::future::Future<Output = T>,
) -> T {
    match scope {
        Some(scope) => request(scope, work).await,
        None => work.await,
    }
}

pub(crate) fn executor_prefix(scope: &LocalPrivateStorage) -> String {
    let identity = tensorfs::sha256::hex_digest(&crate::boundary_json::exact(&serde_json::json!([scope.project_id, scope.generation])));
    format!("private-{identity}:")
}

/// Namespace a metadata/cache resource without changing its wire identity or signer.
/// Logical project identity stays stable across physical key-generation rotation.
pub(crate) fn resource_key(key: &str) -> String {
    match current() {
        Some(scope) => {
            let prefix = format!(
                "local-private:{}:{}:",
                scope.project_id.len(),
                scope.project_id
            );
            if key.starts_with(&prefix) {
                key.to_owned()
            } else {
                format!("{prefix}{key}")
            }
        }
        None => key.to_owned(),
    }
}

/// Return the actual signer/wire identifier from this request's resource namespace.
pub(crate) fn unscoped_key(key: &str) -> String {
    match current() {
        Some(scope) => key
            .strip_prefix(&format!(
                "local-private:{}:{}:",
                scope.project_id.len(),
                scope.project_id
            ))
            .unwrap_or(key)
            .to_owned(),
        None => key.to_owned(),
    }
}

/// A cache produced while reading private source is private too. The public object/model
/// store never uses this selector.
pub(crate) fn cache_root(ordinary: PathBuf, name: &str) -> io::Result<PathBuf> {
    let Some(scope) = current() else { return Ok(ordinary) };
    let path = root(&scope)?.join(name);
    owned_directory(&path)?;
    Ok(path)
}

/// Private generations are resolved only from verified live mounts. Background engine
/// reads have no HTTP task scope, so they may locate an already-authorized generation;
/// admissions and metadata authorization still require the explicit request namespace.
pub(crate) fn generation_path(identity: &str) -> io::Result<Option<PathBuf>> {
    if let Some(scope) = current() {
        let path = root(&scope)?.join("generations").join(identity);
        return Ok(path.is_dir().then_some(path));
    }
    let scopes: Vec<_> = VERIFIED.lock().unwrap().values().map(|value| value.descriptor.clone()).collect();
    let mut found = None;
    for scope in scopes {
        let Ok(root) = root(&scope) else { continue };
        let path = root.join("generations").join(identity);
        if path.is_dir() {
            if found.is_some() { return Err(io::Error::new(io::ErrorKind::InvalidData, "private generation identity is ambiguous")); }
            found = Some(path);
        }
    }
    Ok(found)
}

/// Child tools can copy private sources while building/editing packages. Pin their
/// cwd, temporary files, uv cache and bytecode to the same protected mount.
pub(crate) fn protect_command(command: &mut std::process::Command) -> io::Result<bool> {
    let Some(scope) = current() else { return Ok(false) };
    let root = root(&scope)?;
    let temp = root.join("tmp");
    fs::create_dir_all(&temp)?;
    command.current_dir(&root)
        .env("UV_CACHE_DIR", root.join("uv-cache"))
        .env("UV_TOOL_DIR", root.join("uv-tools"))
        .env("UV_PYTHON_INSTALL_DIR", root.join("python"))
        .env("UV_LINK_MODE", "copy")
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("PYTHONPYCACHEPREFIX", root.join("bytecode"))
        .env("MPLCONFIGDIR", root.join("matplotlib"))
        .env("TMPDIR", &temp).env("TEMP", &temp).env("TMP", &temp)
        .env("PYTHONDONTWRITEBYTECODE", "1");
    Ok(true)
}

/// Protected warnings never enter the global subscriber, including their run identity.
pub(crate) fn warning(engine: &crate::execution::Engine, run: &str, detail: impl std::fmt::Display) {
    if current().is_none() && matches!(engine.private_storage(run), Ok(None)) {
        tracing::warn!(run, "{detail}");
    } else {
        let _ = detail;
    }
}

/// Local arbitrary-path imports remain unavailable. Provider imports already target the
/// project Publisher Store and never acquire public-cache authority from a token.
pub(crate) fn check_model_source(source: &str) -> io::Result<()> {
    if current().is_none() || source.trim().is_empty() { return Ok(()); }
    if source.starts_with("object://") || source.starts_with("input://")
        || matches!(tensorfs::import::remote::parse(source), Ok(tensorfs::import::remote::Spec::Local(_)))
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported,
            "local model-weight imports are unavailable in encrypted project execution"));
    }
    Ok(())
}

pub(crate) fn bind_process_namespace(scope: &LocalPrivateStorage, namespace: &str) -> io::Result<()> {
    let verified = verified_scope(scope)?;
    let mut current = verified.process_namespace.lock().unwrap();
    if current.as_ref().is_some_and(|old| old != namespace) { return Err(denied()); }
    *current = Some(namespace.to_owned());
    Ok(())
}
pub(crate) fn new_process_scope() -> io::Result<Arc<platform::contain::Scope>> {
    let scope = current().ok_or_else(denied)?;
    let verified = verified_scope(&scope)?;
    let namespace = verified.process_namespace.lock().unwrap().clone().ok_or_else(denied)?;
    Ok(Arc::new(platform::contain::Scope::inherited(&namespace)?))
}

/// Known IPC endpoints exist before spawn, so the policy can review their native identity.
pub(crate) struct CommandIo {
    pub input: Option<File>,
    pub output: Option<File>,
    pub error: Option<File>,
    inherited: Vec<platform::private_exec::PrivateDescriptor>,
}
impl CommandIo {
    pub(crate) fn control(file: File) -> Self { Self { input: None, output: None, error: None, inherited: vec![platform::private_exec::PrivateDescriptor::KernelIpc(file)] } }
}
fn ipc_files() -> io::Result<(File, File)> {
    let (left, right) = platform::ipc::pair()?;
    #[cfg(unix)]
    { use std::os::fd::OwnedFd; Ok((File::from(OwnedFd::from(left)), File::from(OwnedFd::from(right)))) }
    #[cfg(windows)]
    { use std::os::windows::io::OwnedHandle; Ok((File::from(OwnedHandle::from(left)), File::from(OwnedHandle::from(right)))) }
}
pub(crate) fn command_io(command: &mut std::process::Command, input: bool, output: bool, error: bool) -> io::Result<Option<CommandIo>> {
    if current().is_none() { return Ok(None); }
    let mut pipes = CommandIo { input: None, output: None, error: None, inherited: vec![] };
    for (which, requested) in [(0, input), (1, output), (2, error)] {
        if !requested { continue; }
        let (parent, child) = ipc_files()?;
        let stdio = std::process::Stdio::from(child.try_clone()?);
        pipes.inherited.push(platform::private_exec::PrivateDescriptor::KernelIpc(child));
        match which { 0 => { command.stdin(stdio); pipes.input = Some(parent); }, 1 => { command.stdout(stdio); pipes.output = Some(parent); }, _ => { command.stderr(stdio); pipes.error = Some(parent); } }
    }
    Ok(Some(pipes))
}

/// Managed process setup only: descriptor inheritance and basic no-Cozy-core defense.
/// Deliberate custom Python exports and OS-owned core/swap are not an OS sandbox guarantee.
pub(crate) fn apply_native(command: &mut std::process::Command, io: Option<&mut CommandIo>, process_scope: Option<&Arc<platform::contain::Scope>>) -> io::Result<()> {
    if current().is_none() { return Ok(()); }
    let inherited = io.map(|pipes| std::mem::take(&mut pipes.inherited)).unwrap_or_default();
    if let Some(scope) = process_scope { command.envs(scope.environment()); }
    platform::private_exec::prepare_managed_child(command, inherited)?;
    if let Some(process_scope) = process_scope { let scope=current().ok_or_else(denied)?; verified_scope(&scope)?.process_scopes.lock().unwrap().push(process_scope.clone()); }
    Ok(())
}
/// Parent/process semantics only; no filesystem-ABI or piped-kernel-handler admission gate.
pub(crate) fn probe_native(_: &LocalPrivateStorage) -> io::Result<()> {
    platform::private_exec::disable_project_parent_dumps().map_err(io::Error::from)
}

pub(crate) fn transport_capabilities(bytes: &[u8]) -> io::Result<()> {
    let answer: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| io::Error::new(io::ErrorKind::Unsupported, "project_transport_unsupported: Runtime has no inherited transport capability"))?;
    if answer["inherited_local_stream"] != true || answer["transport_challenge"] != true {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "project_transport_unsupported: Runtime cannot prove protected inherited transport"));
    }
    Ok(())
}

/// The installed/pinned Runtime is checked without running .pth files or authored Start/Invoke.
pub(crate) fn probe_runtime(python: &std::path::Path) -> io::Result<()> {
    let process_scope = new_process_scope()?;
    let mut command = crate::launch_identity::module(python, "cozy_runtime.internal.executor")?;
    command.arg("--transport-capabilities").env_clear().stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    protect_command(&mut command)?;
    let mut pipes = command_io(&mut command, false, true, false)?;
    apply_native(&mut command, pipes.as_mut(), Some(&process_scope))?;
    let mut child = command.spawn()?;
    drop(command);
    let mut bytes = Vec::new();
    let mut output = pipes.as_mut().and_then(|pipes| pipes.output.take()).ok_or_else(|| io::Error::other("protected Runtime probe has no output channel"))?;
    output.by_ref().take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 { let _ = child.kill(); process_scope.end()?; return Err(io::Error::new(io::ErrorKind::InvalidData, "Runtime transport probe exceeded its protocol bound")); }
    let status = child.wait()?;
    process_scope.end()?;
    if !platform::private_exec::project_scope_quiescent(&process_scope)? { return Err(io::Error::new(io::ErrorKind::ResourceBusy, "Runtime probe descendants remain")); }
    if !status.success() { return Err(io::Error::new(io::ErrorKind::Unsupported, "project_transport_unsupported: selected Runtime refused the inherited transport probe")); }
    transport_capabilities(&bytes)
}

/// A real protected no-payload executor must complete the inherited-channel challenge before
/// the namespace reports readiness. No project invocation or source is sent by this probe.
pub(crate) fn probe_transport(scope: &LocalPrivateStorage, ordinary: &crate::published::Publisher, python: &std::path::Path) -> io::Result<()> {
    let verified = verified_scope(scope)?;
    if verified.transport_prepared.load(std::sync::atomic::Ordering::Acquire) { return Ok(()); }
    let root = root(scope)?;
    let probe_root = root.join("probes").join(crate::uuid::simple());
    let mut seal = crate::launch_identity::Seal::prepare(&root, &crate::uuid::simple(), "project-probe", "")?;
    seal.tensorfs_home = Some(store(scope)?.root().to_owned());
    let config = crate::device_executor::ExecutorConfig { private: true, python: python.to_owned(), root: probe_root,
        socket: PathBuf::new(), environment: std::collections::BTreeMap::new(), seal, generation_hold: None,
        cgroup_namespace: Some(ordinary.process_namespace().to_owned()) };
    let launcher = crate::child_launcher::ChildLauncher::new()?;
    let executor = crate::device_executor::DeviceExecutor::spawn_owned(config, &launcher, |_, _| Ok(()))?;
    executor.terminate()?;
    for process_scope in verified.process_scopes.lock().unwrap().iter() {
        if !platform::private_exec::project_scope_quiescent(process_scope)? { return Err(io::Error::new(io::ErrorKind::ResourceBusy, "protected transport probe has a live descendant")); }
    }
    verified.transport_prepared.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}
