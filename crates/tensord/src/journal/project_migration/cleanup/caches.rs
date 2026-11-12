//! Existing managed generation/build caches, never arbitrary authoring trees.
use super::*;
use std::path::{Component, PathBuf};

fn hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| b.is_ascii_hexdigit())
}
fn absent(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}
fn owned_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || platform::durable::canonical(path)? != path
    {
        return Err(db_error("legacy managed cache is not a native directory"));
    }
    Ok(())
}
fn named_elsewhere(generations: &Path, candidate: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(generations) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for (count, entry) in entries.enumerate() {
        if count >= 4096 {
            return Err(db_error("legacy generation inventory limit"));
        }
        let entry = entry?;
        // Build locks carry no environment reference. Unknown generation entries do.
        if entry.file_name() == ".locks" {
            continue;
        }
        if !entry.file_type()?.is_dir() {
            return Err(db_error("unknown legacy generation entry"));
        }
        for name in ["env", "source"] {
            let path = entry.path().join(name);
            match fs::read_link(&path) {
                Ok(target) => {
                    let target = if target.is_absolute() {
                        target
                    } else {
                        entry.path().join(target)
                    };
                    let target = platform::durable::canonical(&target)?;
                    if target.starts_with(candidate) || candidate.starts_with(&target) {
                        return Ok(true);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound && name == "source" => (),
                Err(error) if error.kind() == io::ErrorKind::InvalidInput && path.is_dir() => (),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(false)
}
fn source_copied(scope: &LocalPrivateStorage, manifest: &str) -> io::Result<()> {
    let private = crate::private_io::store(scope)?;
    let path = private
        .root()
        .join("blobs")
        .join(&manifest[..2])
        .join(&manifest[2..4])
        .join(manifest);
    let file = platform::durable::open_nofollow(&path, fs::OpenOptions::new().read(true))?;
    let object = ObjectRef {
        sha256: manifest.to_owned(),
        length: file.metadata()?.len(),
    };
    verify_object(&private, &object)?;
    let bytes = private.read(&object).map_err(crate::storage::io_error)?;
    let manifest: crate::local_source::Manifest =
        serde_json::from_slice(&bytes).map_err(db_error)?;
    for member in manifest.files.iter().chain(&manifest.wheels) {
        let object = ObjectRef {
            sha256: member.digest.trim_start_matches("sha256:").to_owned(),
            length: member.length,
        };
        if !private.holds(&object) {
            return Err(db_error("legacy source member is not protected"));
        }
        verify_object(&private, &object)?;
    }
    Ok(())
}

pub(super) fn retire(
    source: &Connection,
    target: &Connection,
    receipt: &mut Receipt,
    scope: &LocalPrivateStorage,
    engine: &Path,
) -> io::Result<Vec<String>> {
    let generations = engine.join("generations");
    let records = accepted(receipt, scope)?;
    let mut pending = vec![];
    for identity in receipt.cleanup.generations.clone() {
        if !hex(&identity, 32) {
            pending.push("legacy_generation_identity_invalid".into());
            continue;
        }
        let used:bool=source.query_row("SELECT EXISTS(SELECT 1 FROM executions WHERE json_extract(invocation,'$.generation')=?1 UNION SELECT 1 FROM installations WHERE json_extract(record,'$.generation')=?1)",[&identity],|row|row.get(0)).map_err(db_error)?;
        if used {
            continue;
        } // An authoritative independent source owner remains.
        let directory = generations.join(&identity);
        if absent(&directory)? {
            continue;
        }
        owned_directory(&directory)?;
        let file = platform::durable::open_nofollow(
            &directory.join("generation.json"),
            fs::OpenOptions::new().read(true),
        )?;
        if file.metadata()?.len() > 4 << 20 {
            return Err(db_error("legacy generation document limit"));
        }
        let mut bytes = vec![];
        use std::io::Read as _;
        file.take((4 << 20) + 1).read_to_end(&mut bytes)?;
        let generation: crate::catalog::Generation =
            serde_json::from_slice(&bytes).map_err(db_error)?;
        if generation.identity != identity
            || generation.python != platform::paths::venv_python(&directory.join("env"))
        {
            return Err(db_error(
                "legacy generation identity differs from actual manifest",
            ));
        }
        // Published software is an independent installation, not project-authored cache.
        if !generation.package.starts_with("local/") {
            continue;
        }
        if let Some(previous) = receipt.cleanup.generation_records.get(&identity) {
            if previous != &bytes {
                return Err(db_error("legacy generation changed after receipt"));
            }
        } else {
            receipt
                .cleanup
                .generation_records
                .insert(identity.clone(), bytes);
        }
        let owners = records
            .iter()
            .filter(|record| record.invocation.generation == identity)
            .filter_map(|record| {
                record
                    .submission
                    .as_ref()
                    .map(|submission| submission.actor.as_str())
            })
            .collect::<BTreeSet<_>>();
        if owners.len() != 1 {
            pending.push(format!("legacy_generation_owner_ambiguous:{identity}"));
            continue;
        }
        let actor = owners.into_iter().next().unwrap();
        let package =
            crate::catalog::normalized(generation.package.strip_prefix("local/").unwrap());
        if package.is_empty() || package.contains('/') {
            return Err(db_error("legacy local package is invalid"));
        }
        let home = engine
            .join("editable")
            .join(&tensorfs::sha256::hex_digest(actor.as_bytes())[..16])
            .join(package);
        for (name, prefix, length) in [("env", "env-", 16), ("source", "source-", 64)] {
            let link = fs::read_link(directory.join(name))?;
            let link = if link.is_absolute() {
                link
            } else {
                directory.join(link)
            };
            let candidate = if name == "source" {
                if link.file_name() != Some(std::ffi::OsStr::new("tree")) {
                    return Err(db_error("unrecognized editable source link"));
                }
                link.parent()
                    .ok_or_else(|| db_error("editable source has no parent"))?
                    .to_path_buf()
            } else {
                link
            };
            let leaf = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| db_error("invalid editable cache name"))?;
            let key = leaf
                .strip_prefix(prefix)
                .filter(|key| hex(key, length))
                .ok_or_else(|| db_error("unknown editable cache layout"))?;
            if candidate.parent() != Some(home.as_path()) {
                return Err(db_error(
                    "editable cache differs from actual signer/package home",
                ));
            }
            owned_directory(&candidate)?;
            let proof = if name == "source" {
                source_copied(scope, key)?;
                key.to_owned()
            } else {
                let ready = fs::read(candidate.join(".ready"))?;
                serde_json::from_slice::<crate::published::environment::Built>(&ready)
                    .map_err(db_error)?;
                String::new()
            };
            receipt
                .cleanup
                .editable_caches
                .insert(candidate.to_string_lossy().into_owned(), proof);
        }
        // The exact full generation document and cache plan are protected before removal.
        save(target, receipt)?;
        checkpoint(target)?;
        if crate::reclaim::retire_generation(&generations, &identity)?.is_none() {
            pending.push(format!("legacy_generation_reader:{identity}"));
        }
    }
    for (path, manifest) in receipt.cleanup.editable_caches.clone() {
        let candidate = PathBuf::from(path);
        if !candidate.starts_with(engine.join("editable"))
            || candidate
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(db_error("receipt editable path is outside managed cache"));
        }
        if absent(&candidate)? {
            continue;
        }
        owned_directory(&candidate)?;
        if named_elsewhere(&generations, &candidate)? {
            continue;
        }
        if !manifest.is_empty() {
            source_copied(scope, &manifest)?;
            let destination =
                crate::private_io::cache_root(PathBuf::new(), "legacy-source")?.join(&manifest);
            copy_run_directory(&candidate, &destination)?;
            save(target, receipt)?;
            checkpoint(target)?;
            retire_run_directory(&candidate)?;
        } else {
            // Known built environments are regenerable; source and generation metadata are
            // already durable. `remove_dir_all` removes links, never their external targets.
            serde_json::from_slice::<crate::published::environment::Built>(&fs::read(
                candidate.join(".ready"),
            )?)
            .map_err(db_error)?;
            fs::remove_dir_all(&candidate)?;
            platform::durable::sync_dir(candidate.parent().unwrap())?;
        }
    }
    if pending.is_empty() {
        // Only the machine's own cache: never a user cache or the image dependency seed.
        let mut environments = vec![
            generations,
            engine.join("editable"),
            crate::machine::toolchain::dir(engine),
            engine.join("sdk-runtime-probe"),
        ];
        environments.extend(
            crate::published::layer::bases(engine)
                .into_iter()
                .map(|(path, _)| path),
        );
        pending.extend(crate::reclaim::retire_uv_cache(
            &engine.join("published/uv-cache"),
            &environments,
        )?);
    }
    Ok(pending)
}
