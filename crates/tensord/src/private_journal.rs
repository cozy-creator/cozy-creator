//! Immutable private journal blobs. Only verified local encrypted views receive payload bytes;
//! the project-owned SQLite stores their references and scheduling projection. A new blob is
//! fsynced before its reference can participate in a SQLite commit. Never overwrite an old blob.
use std::{
    fs::{self, OpenOptions},
    io::{self, Read as _, Write as _},
    path::Path,
};

use api::machine::LocalPrivateStorage;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

const PREFIX: &[u8] = b"cozy-private-blob:1:";
const DIRECTORY: &str = "journal";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    format: u32,
    storage: LocalPrivateStorage,
    context: String,
    file: String,
    length: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    format: u32,
    project_id: String,
    generation: u64,
    context: String,
    payload: String,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "private journal reference or content is invalid",
    )
}
fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(io::Error::other)
}

pub(crate) fn scope(value: &[u8]) -> io::Result<Option<LocalPrivateStorage>> {
    let Some(encoded) = value.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    let reference: Reference = serde_json::from_slice(encoded).map_err(|_| invalid())?;
    if reference.format != 1 {
        return Err(invalid());
    }
    Ok(Some(reference.storage))
}

pub(crate) fn read_scoped(context: &str, value: &[u8]) -> io::Result<Vec<u8>> {
    if let Some(scope) = scope(value)? {
        if crate::private_io::current()
            .as_ref()
            .is_none_or(|current| current.project_id != scope.project_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private journal is outside this request's workspace",
            ));
        }
    }
    read(context, value, false)
}

pub(crate) fn write(
    scope: Option<&LocalPrivateStorage>,
    context: &str,
    value: &[u8],
) -> io::Result<Vec<u8>> {
    let Some(scope) = scope else {
        return Ok(value.to_vec());
    };
    let root = crate::private_io::root(scope)?;
    let directory = root.join(DIRECTORY);
    fs::create_dir_all(&directory)?;
    let metadata = fs::symlink_metadata(&directory)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid());
    }
    let file = format!("{}.blob", crate::uuid::simple());
    let path = directory.join(&file);
    let body = encode(&Blob {
        format: 1,
        project_id: scope.project_id.clone(),
        generation: scope.generation,
        context: context.into(),
        payload: STANDARD.encode(value),
    })?;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        platform::durable::Mode::mode(&mut options, 0o600);
        let mut output = platform::durable::open_nofollow(&path, &mut options)?;
        output.write_all(&body)?;
        output.sync_all()?;
        platform::durable::sync_dir(&directory)?;
        let reference = Reference {
            format: 1,
            storage: scope.clone(),
            context: context.into(),
            file,
            length: body.len() as u64,
        };
        let mut encoded = PREFIX.to_vec();
        encoded.extend(encode(&reference)?);
        Ok(encoded)
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

pub(crate) fn read(context: &str, value: &[u8], required: bool) -> io::Result<Vec<u8>> {
    let Some(encoded) = value.strip_prefix(PREFIX) else {
        return if required {
            Err(invalid())
        } else {
            Ok(value.to_vec())
        };
    };
    let reference: Reference = serde_json::from_slice(encoded).map_err(|_| invalid())?;
    if reference.format != 1 || reference.context != context || reference.storage.generation == 0 {
        return Err(invalid());
    }
    let stem = reference.file.strip_suffix(".blob").ok_or_else(invalid)?;
    if stem.len() != 32 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let root = crate::private_io::root(&reference.storage)?;
    let mut source =
        crate::execution::open_artifact(&root, &Path::new(DIRECTORY).join(&reference.file))?;
    if source.metadata()?.len() != reference.length {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut source)
        .take(reference.length.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != reference.length {
        return Err(invalid());
    }
    let blob: Blob = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if blob.format != 1
        || blob.project_id != reference.storage.project_id
        || blob.generation != reference.storage.generation
        || blob.context != context
    {
        return Err(invalid());
    }
    STANDARD.decode(blob.payload).map_err(|_| invalid())
}

/// Re-home only framework references/descriptors, never arbitrary authored path strings.
/// Each old immutable blob is validated through the current verified view before a new
/// immutable reference is written there. The original bytes remain protected history.
pub(crate) fn rebase_for_migration(
    bytes: &[u8],
    scope: &LocalPrivateStorage,
    references: &mut std::collections::BTreeMap<String, String>,
) -> io::Result<Vec<u8>> {
    if bytes.starts_with(PREFIX) {
        let key = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        if let Some(rebased) = references.get(key) {
            return Ok(rebased.as_bytes().to_vec());
        }
        let reference: Reference =
            serde_json::from_slice(&bytes[PREFIX.len()..]).map_err(|_| invalid())?;
        if reference.storage.project_id != scope.project_id
            || reference.storage.generation == 0
            || reference.storage.generation > scope.generation
        {
            return Err(invalid());
        }
        let payload = read(&reference.context, bytes, true)?;
        let mut payload = payload;
        // Execution and run-spec framework scopes are top-level fields. Input/value text
        // remains verbatim, even if it happens to spell an old physical path.
        if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&payload) {
            let descriptor = if reference.context.split(':').count() == 2
                && reference.context.starts_with("execution:")
            {
                value
                    .get_mut("invocation")
                    .and_then(|invocation| invocation.get_mut("local_private"))
            } else if reference.context.ends_with(":spec") {
                value.get_mut("local_private")
            } else {
                None
            };
            if let Some(descriptor) = descriptor {
                if !descriptor.is_null() {
                    let mut old: LocalPrivateStorage =
                        serde_json::from_value(descriptor.clone()).map_err(|_| invalid())?;
                    if old.project_id != scope.project_id
                        || old.generation == 0
                        || old.generation > scope.generation
                    {
                        return Err(invalid());
                    }
                    old.view_root = scope.view_root.clone();
                    *descriptor = serde_json::to_value(old).map_err(|_| invalid())?;
                    payload = encode(&value)?;
                }
            }
        }
        let mut owned = reference.storage;
        owned.view_root = scope.view_root.clone();
        let rebased = write(Some(&owned), &reference.context, &payload)?;
        references.insert(
            key.to_owned(),
            String::from_utf8(rebased.clone()).map_err(|_| invalid())?,
        );
        return Ok(rebased);
    }
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Ok(bytes.to_vec());
    };
    let mut changed = false;
    if let Some(pointer) = value.get_mut("_private_payload") {
        let pointer = pointer.as_str().ok_or_else(invalid)?;
        let rebased = rebase_for_migration(pointer.as_bytes(), scope, references)?;
        value["_private_payload"] = String::from_utf8(rebased).map_err(|_| invalid())?.into();
        changed = true;
        if let Some(descriptor) = value
            .get_mut("invocation")
            .and_then(|invocation| invocation.get_mut("local_private"))
        {
            let mut owned: LocalPrivateStorage =
                serde_json::from_value(descriptor.clone()).map_err(|_| invalid())?;
            if owned.project_id != scope.project_id
                || owned.generation == 0
                || owned.generation > scope.generation
            {
                return Err(invalid());
            }
            owned.view_root = scope.view_root.clone();
            *descriptor = serde_json::to_value(owned).map_err(|_| invalid())?;
        }
    } else if let Some(descriptor) = value.get_mut("local_private") {
        let mut owned: LocalPrivateStorage =
            serde_json::from_value(descriptor.clone()).map_err(|_| invalid())?;
        if owned.project_id != scope.project_id
            || owned.generation == 0
            || owned.generation > scope.generation
        {
            return Err(invalid());
        }
        owned.view_root = scope.view_root.clone();
        *descriptor = serde_json::to_value(owned).map_err(|_| invalid())?;
        changed = true;
    }
    if changed {
        encode(&value)
    } else {
        Ok(bytes.to_vec())
    }
}

/// Decode only a framework journal reference/index for inside-project migration inventory.
pub(crate) fn inspect_for_migration(
    bytes: &[u8],
    scope: &LocalPrivateStorage,
) -> io::Result<Vec<u8>> {
    if let Some(encoded) = bytes.strip_prefix(PREFIX) {
        let reference: Reference = serde_json::from_slice(encoded).map_err(|_| invalid())?;
        if reference.storage.project_id != scope.project_id
            || reference.storage.generation == 0
            || reference.storage.generation > scope.generation
        {
            return Err(invalid());
        }
        return read(&reference.context, bytes, true);
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if let Some(pointer) = value
            .get("_private_payload")
            .and_then(serde_json::Value::as_str)
        {
            return inspect_for_migration(pointer.as_bytes(), scope);
        }
    }
    Ok(bytes.to_vec())
}
