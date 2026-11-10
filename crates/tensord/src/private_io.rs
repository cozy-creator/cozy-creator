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

pub(crate) const AREA: &str = ".vault-machine";

struct Verified {
    descriptor: LocalPrivateStorage,
    actor: String,
    token_hash: [u8; 32],
    store: Mutex<Option<Arc<Store>>>,
    roots: Mutex<crate::storage::Roots>,
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
    let real = fs::canonicalize(&descriptor.view_root).map_err(|_| denied())?;
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
    Ok(marker)
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
        if marker(&old.descriptor).is_ok_and(|marker| digest(&marker.token) == old.token_hash) {
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
            roots: Mutex::default(),
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
    let root = resolve(descriptor)?.view_root.join(AREA);
    fs::create_dir_all(&root)?;
    platform::durable::set_mode(&root, 0o700)?;
    Ok(root)
}

/// Only private run media/trees/results use this store. Public model weights retain the
/// existing machine-wide store and are never copied here merely because a run is private.
pub(crate) fn store(descriptor: &LocalPrivateStorage) -> io::Result<Arc<Store>> {
    let scope = verified_scope(descriptor)?;
    let path = root(&scope.descriptor)?.join("objects");
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
    let mut all = VERIFIED.lock().unwrap();
    let Some(scope) = all.get(&descriptor.project_id) else {
        return Ok(());
    };
    if scope.actor != actor || scope.descriptor.generation != descriptor.generation {
        return Err(denied());
    }
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
    all.remove(&descriptor.project_id);
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
    let restore = Restore(CURRENT.with(|current| current.replace(scope)));
    let value = work();
    drop(restore);
    value
}

/// A private HTTP future keeps its scope across awaits without leaking thread-local state
/// into an unrelated request polled by the same Tokio worker.
pub(crate) async fn request<T>(
    scope: LocalPrivateStorage,
    work: impl std::future::Future<Output = T>,
) -> T {
    REQUEST.scope(scope, work).await
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
