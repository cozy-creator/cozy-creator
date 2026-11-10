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

tokio::task_local! { static PROJECT: Option<Scope>; }

pub(crate) fn current() -> Option<Scope> {
    PROJECT.try_with(Clone::clone).ok().flatten()
}

pub(crate) fn of_path(path: &Path) -> Result<Option<Scope>, Error> {
    let logical = crate::project::vault::logical(path);
    let Some(root) = logical.ancestors().find(|root| crate::project::vault::encrypted(root)) else {
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

#[derive(Clone)]
pub(crate) struct Scope {
    pub storage: LocalPrivateStorage,
    token: String,
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

pub(crate) fn of_output(out_dir: &Path) -> Result<Option<Scope>, Error> {
    let Some(project) = crate::project::of_landing(out_dir) else {
        return Ok(None);
    };
    let logical = crate::project::vault::logical(project);
    if !crate::project::vault::encrypted(&logical) {
        return Ok(None);
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

/// No machine is started merely to lock a project. Root calls this before unmount,
/// without holding the vault-session mutex; the server refuses live work and reaps only
/// this namespace's idle executors before releasing private store handles.
pub(crate) async fn close_private_project(
    api: &Arc<crate::api::Api>,
    root: &Path,
) -> Result<(), Error> {
    if !api.machines.local.running() {
        return Ok(());
    }
    let scope = SCOPES.lock().unwrap().get(root).cloned();
    let Some(scope) = scope else { return Ok(()) };
    if !scope.storage.view_root.join(".vault-machine").exists() {
        return Ok(());
    }
    let Some(link) = api
        .calcifer
        .link("local")
        .map_err(|_| error("the local execution service is unavailable"))?
    else {
        return Err(error(
            "the local execution service is not ready to close its private namespace",
        ));
    };
    link.with_private(scope)
        .close_private()
        .await
        .map_err(|_| {
            error("private execution is still active; wait or cancel its work before locking")
        })?;
    SCOPES.lock().unwrap().remove(root);
    Ok(())
}
