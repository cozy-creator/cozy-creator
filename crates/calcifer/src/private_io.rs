//! A local execution's proof of access to the already-unlocked encrypted project.
//! Tokens live only in its encrypted view and outgoing authentication headers.
use api::{
    error::{Error, Kind},
    machine::{LOCAL_PRIVATE_MARKER, LocalPrivateMarker, LocalPrivateStorage},
};
use project_vault::{KEYBAG_FILE, Keybag};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

static SCOPES: LazyLock<Mutex<HashMap<PathBuf, Scope>>> = LazyLock::new(Mutex::default);
/// Each project proof a local worker proved it protects, by project, proof token and worker (`execution_link`).
static PROVED: LazyLock<Mutex<std::collections::HashSet<(String, String, String)>>> = LazyLock::new(Mutex::default);

tokio::task_local! { static PROJECT: Option<Scope>; }

pub(crate) fn current() -> Option<Scope> {
    PROJECT.try_with(Clone::clone).ok().flatten()
}

pub(crate) fn of_path(path: &Path) -> Result<Option<Scope>, Error> {
    let logical = crate::project::vault::logical(path);
    let Some(root) = logical.ancestors().find(|root| crate::project::vault::encrypted(root) || root.join(LOCAL_PRIVATE_MARKER).is_file() || root.join("project.db").is_file()) else {
        return Ok(None);
    };
    of_output(&crate::project::landing(root))
}

pub(crate) async fn context<T>(scope: Option<Scope>, work: impl std::future::Future<Output = T>) -> T {
    PROJECT.scope(scope, work).await
}

/// A local-code cache belongs to its project even when it is only being described.
/// Revalidate the live view before any cache hit; a locked project never falls back.
pub(crate) fn cache(scope: Option<&Scope>, ordinary: PathBuf, name: &str) -> Result<PathBuf, Error> {
    let Some(scope) = scope else { return Ok(ordinary) };
    crate::project::vault::require_path(&scope.storage.view_root)?;
    let dir = scope.storage.view_root.join(".vault-calcifer").join(name);
    fs::create_dir_all(&dir).map_err(|_| error("cannot open private code staging"))?;
    platform::durable::set_mode(&dir, 0o700).map_err(|_| error("cannot protect private code staging"))?;
    Ok(dir)
}

/// Tests only (`calcifer.diagnostics`): `line`, timed, at the end of its project's own log, `.vault-calcifer/diagnostics/walk.log` under
/// `storage`, its folder's `dir` (a plain project's folder, an encrypted one's readable view). Never Calcifer's log, where a locked
/// project leaves nothing of itself; a locked view takes nothing.
pub(crate) fn diagnose(storage: &Path, line: impl FnOnce() -> String) {
    use std::io::Write as _;
    if !crate::health::diagnostics() || crate::project::vault::require_path(storage).is_err() {
        return;
    }
    let dir = storage.join(".vault-calcifer").join("diagnostics");
    let line = format!("{:.3} {}\n", jiff::Timestamp::now(), line());
    let _ = fs::create_dir_all(&dir)
        .and_then(|()| fs::OpenOptions::new().create(true).append(true).open(dir.join("walk.log")))
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

#[derive(Clone)]
pub(crate) struct Scope {
    pub storage: LocalPrivateStorage,
    token: String,
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope").field("project_id", &self.storage.project_id).finish_non_exhaustive()
    }
}

impl Scope {
    pub(crate) fn descriptor_header(&self) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&self.storage).expect("private descriptor serializes"))
    }
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

fn error(detail: &'static str) -> Error {
    Error::new(Kind::Conflict, "private_storage_unavailable", detail)
}

/// Validate the local canonical owner before placement. Choosing a remote machine
/// keeps the existing remote working-copy flow; its provider storage is external.
pub(crate) fn check_project_placement(
    project_id: &str,
    project_root: &Path,
    on: &[api::calcifer::On],
) -> Result<Vec<api::calcifer::On>, Error> {
    if project_id.is_empty() || !project_root.is_absolute() {
        return Err(error("This project isn't open right now. Open it, then try again."));
    }
    Ok(on.to_vec())
}

/// Native project identity is checked before creating or reusing its owner proof. Preparation,
/// source planning and uploads use this same scope, before any placement is accepted.
pub(crate) fn scope_for_project(project_id: &str, project_root: &Path) -> Result<Scope, Error> {
    let logical = crate::project::vault::logical(project_root);
    let scope = if crate::project::vault::encrypted(&logical) {
        of_output(&crate::project::landing(&logical))?.ok_or_else(|| error("This encrypted project's files aren't available. Unlock it, then try again."))?
    } else {
        let view = platform::durable::canonical(project_root).map_err(|_| error("This project's files aren't available right now. Try again in a moment."))?;
        if crate::project::vault::plain_project_id(&view).as_deref() != Some(project_id) {
            return Err(error("The project changed while the run was being sent. Try again."));
        }
        match retained_scope(&view, project_id)? {
            Some(scope) => scope,
            None => descriptor_for_mounted(&view, project_id, 1)?,
        }
    };
    if scope.storage.project_id != project_id { return Err(error("This run's files belong to another project.")); }
    Ok(scope)
}

/// Called with the admitted project's identity/root after its bounded DB operation has ended.
pub(crate) async fn execution_link(
    project_id: &str,
    project_root: &Path,
    machine: &str,
    base: &crate::link::Link,
) -> Result<crate::link::Link, Error> {
    check_project_placement(project_id, project_root, &[api::calcifer::On::Machine(machine.into())])?;
    let scope = scope_for_project(project_id, project_root)?;
    // Never send this computer's mounted path or proof token to a remote worker.
    // Canonical inputs, history and downloaded results remain in this project.
    if machine != "local" { return Ok(base.clone()); }
    // Proved once for this worker and this proof, not before each call: every protected request is checked there anyway, and an export
    // that waited on two more round trips landed after its take was listed.
    let proved = (project_id.to_owned(), scope.token.clone(), base.worker());
    let selected = base.with_private(scope);
    if PROVED.lock().unwrap().contains(&proved) { return Ok(selected); }
    selected.probe_private().await.map_err(|failure| match failure {
        crate::link::LinkError::Refused { code, message, remedy, next } => Error {
            remedy, next, ..Error::new(Kind::Conflict, code, message)
        },
        crate::link::LinkError::NotFound => Error::new(Kind::Conflict, "project_worker_upgrade_required",
            "This machine is too old to run a project's work. Update it, then try again.").next("cozy doctor"),
        _ => Error::new(Kind::Unavailable, "project_worker_unavailable",
            "This computer's machine couldn't confirm it can keep the project's files private, so Cozy didn't send them.").next("cozy doctor"),
    })?;
    PROVED.lock().unwrap().insert(proved);
    Ok(selected)
}

/// Root calls this after a successful mount/unlock and after rekey cutover. Replacing
/// an old owned marker renews its proof without exposing the filesystem encryption key.
pub(crate) fn descriptor_for_mounted(
    view: &Path,
    project_id: &str,
    generation: u64,
) -> Result<Scope, Error> {
    let view = platform::durable::canonical(view)
        .map_err(|_| error("the private project view is unavailable"))?;
    let storage = LocalPrivateStorage {
        project_id: project_id.to_owned(),
        generation,
        ordinary: crate::project::vault::ordinary_storage(&view),
        view_root: view.clone(),
    };
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| error("operating-system randomness is unavailable"))?;
    let marker = LocalPrivateMarker {
        format_version: 1,
        storage: storage.clone(),
        token: hex::encode(bytes),
    };
    let path = view.join(LOCAL_PRIVATE_MARKER);
    if path.exists() {
        let old = read_marker(&view)?;
        if old.storage.project_id != project_id {
            return Err(error(
                "the reserved private execution marker belongs to another project",
            ));
        }
    }
    let encoded = serde_json::to_vec(&marker)
        .map_err(|_| error("cannot encode the private execution marker"))?;
    platform::durable::replace(&path, &encoded, 0o600)
        .map_err(|_| error("cannot retain the private execution marker"))?;
    let scope = Scope {
        storage,
        token: marker.token,
    };
    SCOPES
        .lock()
        .unwrap()
        .insert(crate::project::vault::logical(&view), scope.clone());
    Ok(scope)
}

/// A plain project's folder renamed (cl-1097): its marker names where it is now, its token and namespace kept. Its machine side was
/// closed before the move, so nothing holds the old place.
pub(crate) fn moved(from: &Path, to: &Path) -> Result<(), Error> {
    SCOPES.lock().unwrap().remove(from);
    if !to.join(LOCAL_PRIVATE_MARKER).exists() { return Ok(()); }
    let mut marker = read_marker(to)?;
    marker.storage.view_root = platform::durable::canonical(to).map_err(|_| error("the renamed project is unavailable"))?;
    let encoded = serde_json::to_vec(&marker).map_err(|_| error("cannot encode the private execution marker"))?;
    platform::durable::replace(&to.join(LOCAL_PRIVATE_MARKER), &encoded, 0o600).map_err(|_| error("cannot retain the private execution marker"))
}

/// Project `id`'s folder moved to `root` (cl-1066): this computer's machine, if it's running, is proved it there and its executors from
/// where it was end, as a rename closes them (they work in the folder), once nothing of it runs there. Its next run starts them here.
pub(crate) async fn relocated(api: Arc<crate::api::Api>, id: String, root: PathBuf) {
    PROVED.lock().unwrap().retain(|(project, _, _)| project != &id);
    if !api.machines.local.running() { return; }
    let Ok(Some(base)) = api.calcifer.link("local") else { return };
    if let Err(error) = execution_link(&id, &root, "local", &base).await {
        return tracing::warn!("{}: this computer's machine still reads it where it was: {}", root.display(), error.message);
    }
    if let Err(error) = close_private_project(&api, &root).await {
        tracing::warn!("{}: this computer's machine keeps its executors from where it was: {}", root.display(), error.message);
    }
}

fn read_marker(view: &Path) -> Result<LocalPrivateMarker, Error> {
    let path = view.join(LOCAL_PRIVATE_MARKER);
    if fs::symlink_metadata(&path)
        .map_err(|_| error("the private execution marker is unavailable"))?
        .file_type()
        .is_symlink()
    {
        return Err(error(
            "the private execution marker is not a regular owned file",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| error("the private execution marker is unavailable"))?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| error("the private execution marker is unreadable"))?;
    if bytes.len() > 8192 {
        return Err(error("the private execution marker is invalid"));
    }
    serde_json::from_slice(&bytes).map_err(|_| error("the private execution marker is invalid"))
}

/// A decrypted project retains its execution closure and namespace. The caller first
/// obtains `project_id` from its canonical document, never from the marker itself.
pub(crate) fn retained_scope(root: &Path, project_id: &str) -> Result<Option<Scope>, Error> {
    if !root.join(LOCAL_PRIVATE_MARKER).exists() { return Ok(None); }
    let mut marker = read_marker(root)?;
    if marker.storage.project_id != project_id { return Ok(None); }
    validate_retained(&marker, project_id)?;
    let view = platform::durable::canonical(root).map_err(|_| error("the retained project is unavailable"))?;
    if marker.storage.view_root != view {
        // Moved, by hand or another program (cl-1066): where it was holds it no more, so its marker names where it is, its token and
        // namespace kept, and this computer's machine is proved it again (`reprove`).
        if crate::project::vault::plain_project_id(&marker.storage.view_root).as_deref() == Some(project_id) {
            return Err(error("Another folder holds this project's files on this computer's machine. Open that one."));
        }
        marker.storage.view_root = view.clone();
        let encoded = serde_json::to_vec(&marker).map_err(|_| error("cannot encode the private execution marker"))?;
        platform::durable::replace(&root.join(LOCAL_PRIVATE_MARKER), &encoded, 0o600).map_err(|_| error("cannot retain the private execution marker"))?;
        PROVED.lock().unwrap().retain(|(project, _, _)| project != project_id);
    }
    let scope = Scope { storage: marker.storage, token: marker.token };
    SCOPES.lock().unwrap().insert(view, scope.clone());
    Ok(Some(scope))
}

fn validate_retained(marker: &LocalPrivateMarker, project_id: &str) -> Result<(), Error> {
    if project_id.is_empty() || project_id.len() > 256 || project_id.chars().any(char::is_control)
        || marker.format_version != 1 || marker.storage.project_id != project_id || !marker.storage.view_root.is_absolute()
        || marker.storage.generation == 0 || marker.token.len() != 64
        || !marker.token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(error("the retained execution marker does not match this project"));
    }
    Ok(())
}

/// Classification only: a copied framework marker may identify retained files
/// and disposable caches without granting its original execution namespace.
pub(crate) fn retained_copy_storage(root: &Path) -> Result<Option<LocalPrivateStorage>, Error> {
    match fs::symlink_metadata(root.join(LOCAL_PRIVATE_MARKER)) {
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(error("the retained execution marker is unavailable")),
        Ok(_) => {},
    }
    let marker = read_marker(root)?;
    validate_retained(&marker, &marker.storage.project_id)?;
    Ok(Some(marker.storage))
}

/// Journal reads use an existing native-ID-validated root binding; they do not
/// reload the entire Automerge document for each private journal column.
pub(crate) fn retained_view(root: &Path, project_id: &str) -> Result<PathBuf, Error> {
    let marker = read_marker(root)?;
    validate_retained(&marker, project_id)?;
    let view = platform::durable::canonical(root).map_err(|_| error("the retained project is unavailable"))?;
    if marker.storage.view_root != view { return Err(error("the retained execution marker names another project location")); }
    Ok(view)
}

pub(crate) fn of_output(out_dir: &Path) -> Result<Option<Scope>, Error> {
    let Some(project) = crate::project::of_landing(out_dir) else {
        return Ok(None);
    };
    let logical = crate::project::vault::logical(project);
    if !crate::project::vault::encrypted(&logical) {
        let id = crate::project::vault::plain_project_id(&logical).ok_or_else(|| error("This project isn't open right now. Open it, then try again."))?;
        return scope_for_project(&id, &logical).map(Some);
    }
    let view = crate::project::vault::resolve_path(&logical)?;
    let info = Keybag::load(&logical.join(KEYBAG_FILE))
        .map_err(|_| error("the private project keybag is unavailable"))?
        .info();
    let marker = match read_marker(&view) {
        Ok(marker) => marker,
        Err(_) if !view.join(LOCAL_PRIVATE_MARKER).exists() => {
            return descriptor_for_mounted(&view, &info.project_id, info.generation).map(Some);
        }
        Err(failure) => return Err(failure),
    };
    if marker.format_version != 1
        || marker.storage.project_id != info.project_id
        || marker.storage.generation != info.generation
        || marker.storage.view_root != view
        || marker.token.len() != 64
        || !marker.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(error(
            "the private execution marker does not match this mounted project",
        ));
    }
    let scope = Scope {
        storage: marker.storage,
        token: marker.token,
    };
    SCOPES.lock().unwrap().insert(logical, scope.clone());
    Ok(Some(scope))
}

/// Ordinary staging is unchanged. Private input copies/parts never reach the global CAS.
pub(crate) fn input_root(out_dir: &Path, ordinary: &Path) -> Result<PathBuf, Error> {
    let Some(scope) = of_output(out_dir)? else {
        return Ok(ordinary.to_owned());
    };
    let path = scope.storage.view_root.join(".vault-inputs");
    fs::create_dir_all(&path).map_err(|_| error("cannot open private input staging"))?;
    platform::durable::set_mode(&path, 0o700)
        .map_err(|_| error("cannot protect private input staging"))?;
    Ok(path)
}

/// Root calls this before unmount, without holding the vault-session mutex; the server
/// finishes any custody it still owes this project, refuses live work and reaps only this
/// namespace's idle executors before releasing private store handles. A project with worker
/// storage starts the local worker for this seal alone when none runs.
pub(crate) async fn close_private_project(
    api: &Arc<crate::api::Api>,
    root: &Path,
) -> Result<(), Error> {
    let view = crate::project::vault::resolve_path(root)?;
    if !view.join(".vault-machine").exists() { let mut scopes = SCOPES.lock().unwrap(); scopes.remove(root); scopes.remove(&view); return Ok(()); }
    if !api.machines.local.running() {
        api.machines.local_up().await.map_err(|why| Error::new(Kind::Unavailable, "project_seal_pending",
            format!("This computer's machine couldn't start, so Cozy couldn't secure the project's work: {}", why.message)).next("cozy doctor"))?;
    }
    let scope = match SCOPES.lock().unwrap().get(root).cloned() {
        Some(scope) => scope,
        None => {
            let marker = read_marker(&view)?;
            let logical = crate::project::vault::logical(root);
            let id = if crate::project::vault::encrypted(&logical) {
                let info = Keybag::load(&logical.join(KEYBAG_FILE)).map_err(|_| error("This project isn't open right now. Open it, then try again."))?.info();
                if marker.storage.generation != info.generation { return Err(error("The project's encryption changed while Cozy was locking it. Try again.")); }
                info.project_id
            } else { crate::project::vault::plain_project_id(&view).ok_or_else(|| error("This project isn't open right now. Open it, then try again."))? };
            validate_retained(&marker, &id)?;
            if marker.storage.view_root != view { return Err(error("The project moved while Cozy was locking it. Try again.")); }
            Scope { storage: marker.storage, token: marker.token }
        }
    };
    let Some(link) = api
        .calcifer
        .link("local")
        .map_err(|_| error("This computer's machine isn't running, so Cozy can't lock the project yet. Try again in a moment."))?
    else {
        return Err(error(
            "This computer's machine isn't ready to lock the project yet. Try again in a moment.",
        ));
    };
    link.with_private(scope.clone())
        .close_private()
        .await
        .map_err(|failure| match failure {
            crate::link::LinkError::Refused { code, message, remedy, next } => Error { remedy, next, ..Error::new(Kind::Conflict, code, message) },
            _ => error("This computer's machine couldn't confirm the project's work has stopped, so the project isn't locked yet. Try again in a moment."),
        })?;
    descriptor_for_mounted(&scope.storage.view_root, &scope.storage.project_id, scope.storage.generation)?;
    let mut scopes = SCOPES.lock().unwrap();
    scopes.remove(root);
    scopes.remove(&scope.storage.view_root);
    Ok(())
}
