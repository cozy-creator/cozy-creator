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
}

static VERIFIED: LazyLock<Mutex<HashMap<String, Arc<Verified>>>> = LazyLock::new(Mutex::default);
thread_local! { static CURRENT: RefCell<Option<LocalPrivateStorage>> = const { RefCell::new(None) }; }

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
) -> io::Result<()> {
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
            return Ok(());
        }
        if marker(&old.descriptor).is_ok() {
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
        }),
    );
    Ok(())
}

/// Resolve a verified current mount for a current or retained historical descriptor.
/// Stored physical view paths are not authoritative across mount/recovery changes.
pub(crate) fn resolve(descriptor: &LocalPrivateStorage) -> io::Result<LocalPrivateStorage> {
    Ok(verified_scope(descriptor)?.descriptor.clone())
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
    *store = None;
    drop(store);
    all.remove(&descriptor.project_id);
    Ok(())
}

/// HTTP blocking work carries the verified scope. Background work reads its immutable
/// scope from Invocation; it must never inherit another request's thread-local scope.
pub(crate) fn current() -> Option<LocalPrivateStorage> {
    CURRENT.with(|value| value.borrow().clone())
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
