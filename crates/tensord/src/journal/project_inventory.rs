//! Read-only inventory of possible legacy project traces. A match is a retirement blocker,
//! never ownership authority. Reports belong only in the verified project's receipt.
use std::{io, path::Path};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct LegacyFootprint {
    pub(super) execution_ids: Vec<String>,
    pub(super) request_ids: Vec<String>,
    pub(super) generations: Vec<String>,
    pub(super) constructions: Vec<String>,
    pub(super) manifests: Vec<String>,
    pub(super) provider_models: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct InventoryReport {
    pub(super) pending: Vec<String>,
    pub(super) inspected: Vec<String>,
}

pub(super) fn inspect(
    scope: &api::machine::LocalPrivateStorage,
    machine_logs: &Path,
    engine_root: &Path,
    store: &crate::storage::Store,
    footprint: &LegacyFootprint,
) -> io::Result<InventoryReport> {
    if scope.project_id.is_empty() || scope.generation == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "legacy inventory requires a validated project scope"));
    }
    #[cfg(target_os = "linux")]
    { linux::inspect(scope, machine_logs, engine_root, store.root(), footprint) }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (machine_logs, engine_root, store, footprint);
        Ok(InventoryReport { pending: vec!["inventory_platform_unqualified".into()], inspected: vec![] })
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{collections::BTreeSet, ffi::OsString, fs::File, io::Read as _, os::{fd::OwnedFd, unix::{ffi::{OsStrExt as _, OsStringExt as _}, fs::MetadataExt as _}}, path::{Component, PathBuf}};
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};

    const MAX_ENTRIES: usize = 4096;
    const MAX_FILE_BYTES: usize = 2 << 20;
    const MAX_TOTAL_BYTES: usize = 64 << 20;
    const MAX_TERMS: usize = 2048;
    const MAX_TERM_BYTES: usize = 8192;

    struct Inventory {
        report: InventoryReport,
        terms: Vec<Vec<u8>>,
        entries: usize,
        bytes: usize,
        legacy_executions: bool,
    }
    struct Directory { fd: OwnedFd, path: PathBuf }

    impl Inventory {
        fn pending(&mut self, category: &str, path: &Path) {
            self.report.pending.push(format!("{category}:{}", path.display()));
        }
        fn inspected(&mut self, category: &str, path: &Path) {
            self.report.inspected.push(format!("{category}:{}", path.display()));
        }
        fn matched(&self, bytes: &[u8]) -> bool {
            let named = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/');
            self.terms.iter().any(|term| bytes.windows(term.len()).enumerate().any(|(at, found)| {
                found == term.as_slice() && (at == 0 || !named(bytes[at - 1]))
                    && (at + term.len() == bytes.len() || !named(bytes[at + term.len()]) || term.starts_with(b"/") && bytes[at + term.len()] == b'/')
            }))
        }
        fn open_root(&mut self, path: &Path) -> Option<Directory> {
            if !path.is_absolute() { self.pending("relative_managed_root", path); return None; }
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let mut fd = match rustix::fs::open("/", flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(_) => { self.pending("unreadable_managed_root", path); return None; }
            };
            for component in path.components() {
                match component {
                    Component::RootDir => (),
                    Component::Normal(name) => match rustix::fs::openat(&fd, name, flags, Mode::empty()) {
                        Ok(next) => fd = next,
                        Err(rustix::io::Errno::NOENT) => return None,
                        Err(_) => { self.pending("unsafe_or_unreadable_managed_root", path); return None; }
                    },
                    _ => { self.pending("untrusted_managed_root_component", path); return None; }
                }
            }
            self.inspected("managed_root", path);
            Some(Directory { fd, path: path.to_owned() })
        }
        fn list(&mut self, directory: &Directory) -> Vec<(OsString, FileType)> {
            let read = match Dir::read_from(&directory.fd) {
                Ok(read) => read,
                Err(_) => { self.pending("unreadable_managed_directory", &directory.path); return vec![]; }
            };
            let mut entries = vec![];
            for entry in read {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => { self.pending("unreadable_managed_entry", &directory.path); break; }
                };
                let name = entry.file_name().to_bytes();
                if name == b"." || name == b".." { continue; }
                if self.entries >= MAX_ENTRIES { self.pending("inventory_entry_limit", &directory.path); break; }
                self.entries += 1;
                let name = OsString::from_vec(name.to_vec());
                // Inspect through the pinned parent, not a path that could follow an ancestor link.
                let kind = match rustix::fs::statat(&directory.fd, &name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(found) => FileType::from_raw_mode(found.st_mode),
                    Err(rustix::io::Errno::NOENT) => continue,
                    Err(_) => { self.pending("unreadable_managed_entry", &directory.path.join(&name)); continue; }
                };
                entries.push((name, kind));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
        }
        fn directory(&mut self, parent: &Directory, name: &std::ffi::OsStr) -> Option<Directory> {
            let path = parent.path.join(name);
            match rustix::fs::openat(&parent.fd, name, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
                Ok(fd) => Some(Directory { fd, path }),
                Err(rustix::io::Errno::NOENT) => None,
                Err(_) => { self.pending("unsafe_or_unreadable_managed_directory", &path); None }
            }
        }
        fn read(&mut self, parent: &Directory, name: &std::ffi::OsStr) -> Option<Vec<u8>> {
            let path = parent.path.join(name);
            let fd = match rustix::fs::openat(&parent.fd, name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => return None,
                Err(_) => { self.pending("unsafe_or_unreadable_managed_file", &path); return None; }
            };
            let mut file = File::from(fd);
            let before = match file.metadata() {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => { self.pending("unknown_managed_file_type", &path); return None; }
            };
            if self.bytes >= MAX_TOTAL_BYTES { self.pending("inventory_byte_limit", &path); return None; }
            let limit = MAX_FILE_BYTES.min(MAX_TOTAL_BYTES - self.bytes);
            let mut bytes = Vec::new();
            if file.by_ref().take(limit as u64).read_to_end(&mut bytes).is_err() {
                self.pending("unreadable_managed_file", &path); return None;
            }
            self.bytes += bytes.len();
            if before.len() > bytes.len() as u64 { self.pending("bounded_file_read_incomplete", &path); }
            match file.metadata() {
                Ok(after) if before.dev() == after.dev() && before.ino() == after.ino() && before.len() == after.len() && before.modified().ok() == after.modified().ok() => (),
                _ => self.pending("managed_file_changed_during_inventory", &path),
            }
            self.inspected("managed_file", &path);
            Some(bytes)
        }
        fn file(&mut self, parent: &Directory, name: &std::ffi::OsStr, category: &str, mixed: bool) {
            if let Some(bytes) = self.read(parent, name) {
                let path = parent.path.join(name);
                if self.matched(&bytes) || self.matched(name.as_bytes()) { self.pending(category, &path); }
                else if mixed && self.legacy_executions && !bytes.is_empty() { self.pending("mixed_legacy_log_possible_overlap", &path); }
            }
        }
        fn flat(&mut self, root: &Path, category: &str, mixed: bool, daily: bool) {
            let Some(directory) = self.open_root(root) else { return };
            for (name, kind) in self.list(&directory) {
                if daily && !daily_name(&name) { continue; }
                match kind {
                    FileType::RegularFile => self.file(&directory, &name, category, mixed),
                    _ => self.pending("unsafe_or_unknown_managed_entry", &directory.path.join(&name)),
                }
            }
        }
    }

    fn daily_name(name: &std::ffi::OsStr) -> bool {
        let value = name.to_string_lossy();
        matches!(value.as_ref(), "tensord.log" | "stdout.log" | "stderr.log") || value.starts_with("tensord.") && value.ends_with(".log")
    }
    fn digest(text: &str) -> Option<String> {
        let value = text.strip_prefix("sha256:").unwrap_or(text);
        (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| value.to_ascii_lowercase())
    }

    pub(super) fn inspect(scope: &api::machine::LocalPrivateStorage, logs: &Path, engine: &Path, store: &Path, footprint: &LegacyFootprint) -> io::Result<InventoryReport> {
        let mut terms = BTreeSet::new();
        terms.insert(scope.project_id.as_bytes().to_vec());
        terms.insert(scope.view_root.as_os_str().as_bytes().to_vec());
        let fields = [&footprint.execution_ids, &footprint.request_ids, &footprint.generations, &footprint.constructions, &footprint.manifests, &footprint.provider_models];
        let mut limited = fields.iter().fold(0usize, |total, values| total.saturating_add(values.len())) > MAX_TERMS;
        for value in footprint.request_ids.iter().chain(&footprint.generations).chain(&footprint.constructions).chain(&footprint.manifests).chain(&footprint.provider_models).take(MAX_TERMS) {
            if value.len() > MAX_TERM_BYTES { limited = true; continue; }
            if !value.is_empty() && !value.bytes().all(|byte| byte.is_ascii_digit()) { terms.insert(value.as_bytes().to_vec()); }
        }
        // A bare legacy execution number would match unrelated JSON version/size fields.
        // Keep such numbers only in explicit run/execution field spellings, never as ownership.
        for value in footprint.execution_ids.iter().take(MAX_TERMS) {
            if value.len() > MAX_TERM_BYTES { limited = true; continue; }
            if value.is_empty() { continue; }
            if value.bytes().all(|byte| byte.is_ascii_digit()) {
                for key in ["run", "execution", "run_id", "execution_id"] {
                    for term in [format!("{key}={value}"), format!("\"{key}\":{value}"), format!("\"{key}\":\"{value}\"")] { terms.insert(term.into_bytes()); }
                }
            } else { terms.insert(value.as_bytes().to_vec()); }
        }
        for value in footprint.manifests.iter().take(MAX_TERMS) { if let Some(value) = digest(value) { terms.insert(value.into_bytes()); } }
        for value in footprint.generations.iter().take(MAX_TERMS) { if value.len() > MAX_TERM_BYTES { limited = true; continue; } terms.insert(tensorfs::sha256::hex_digest(value.as_bytes())[..32].as_bytes().to_vec()); }
        limited |= terms.len() > MAX_TERMS || terms.iter().any(|term| term.len() > MAX_TERM_BYTES);
        let mut inventory = Inventory { report: InventoryReport::default(), terms: terms.into_iter().filter(|term| !term.is_empty() && term.len() <= MAX_TERM_BYTES).take(MAX_TERMS).collect(), entries: 0, bytes: 0, legacy_executions: !footprint.execution_ids.is_empty() || !footprint.request_ids.is_empty() };
        if limited { inventory.pending("inventory_footprint_limit", store); }
        inventory.flat(logs, "project_term_in_machine_log", true, true);
        if let Some(directory) = inventory.open_root(&store.join("logs")) {
            for name in ["memory.log", "memory.log.1", "transport.log", "transport.log.1"] { inventory.file(&directory, name.as_ref(), "project_term_in_store_log", true); }
        }
        if let Some(directory) = inventory.open_root(&engine.join("gpu/peaks")) {
            for (name, kind) in inventory.list(&directory) {
                let path = directory.path.join(&name);
                if kind != FileType::RegularFile { inventory.pending("unsafe_shared_peak_entry", &path); continue; }
                let Some(bytes) = inventory.read(&directory, &name) else { continue };
                if inventory.matched(&bytes) { inventory.pending("shared_gpu_peak_candidate", &path); }
                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(value) => if let Some(rows) = value.as_array() {
                        for row in rows {
                            let Some(key) = row["key"].as_str() else { inventory.pending("unparsed_shared_peak_record", &path); continue };
                            let Some((construction, _shape)) = key.split_once('|') else { inventory.pending("unparsed_shared_peak_record", &path); continue };
                            if footprint.constructions.iter().take(MAX_TERMS).any(|known| construction == known.as_str() || construction.strip_suffix(known.as_str()).is_some_and(|prefix| prefix.ends_with(':'))) {
                                inventory.pending("shared_gpu_peak_construction_overlap", &path);
                            }
                        }
                    } else { inventory.pending("unparsed_shared_peak_record", &path); },
                    Err(_) => inventory.pending("unparsed_shared_peak_record", &path),
                }
            }
        }
        // These paths are shared by UID/incarnation/generation, never intrinsically project-owned.
        for base in ["gpu/home", "gpu/kernels", "gpu/jit"] {
            let Some(directory) = inventory.open_root(&engine.join(base)) else { continue };
            for (name, kind) in inventory.list(&directory) {
                let path = directory.path.join(&name);
                if base == "gpu/kernels" && kind == FileType::RegularFile && name.to_str().is_some_and(|name|name.strip_prefix(".u").and_then(|name|name.strip_suffix(".hold")).is_some_and(|uid|!uid.is_empty()&&uid.bytes().all(|b|b.is_ascii_digit()))) {
                    if inventory.read(&directory,&name).is_some_and(|bytes|bytes.is_empty()) { inventory.inspected("system_kernel_cache_lock",&path); }
                    else { inventory.pending("unparsed_kernel_cache_lock",&path); }
                    continue;
                }
                if kind != FileType::Directory { inventory.pending("unsafe_shared_engine_entry", &path); continue; }
                let Some(child) = inventory.directory(&directory, &name) else { continue };
                for (leaf, kind) in inventory.list(&child) {
                    let path = child.path.join(&leaf);
                    let generation = leaf.as_bytes().strip_prefix(b"torch-kernels.").unwrap_or(leaf.as_bytes());
                    if inventory.matched(leaf.as_bytes()) || inventory.terms.iter().any(|term| generation == term.as_slice()) { inventory.pending("shared_engine_generation_candidate", &path); }
                    if !matches!(kind, FileType::Directory | FileType::RegularFile) { inventory.pending("unsafe_shared_engine_entry", &path); }
                }
                if inventory.legacy_executions { inventory.pending("shared_engine_namespace_provenance_unknown", &path); }
                inventory.inspected("shared_engine_namespace", &path);
            }
        }
        if let Some(directory) = inventory.open_root(&store.join("tmp")) {
            for (name, kind) in inventory.list(&directory) {
                if !name.as_bytes().starts_with(b".tensorfs-view-") { continue; }
                let path = directory.path.join(&name);
                if kind != FileType::RegularFile { inventory.pending("unsafe_derived_view_candidate", &path); continue; }
                inventory.file(&directory, &name, "derived_view_project_term", false);
                if inventory.legacy_executions { inventory.pending("derived_view_provenance_unknown", &path); }
            }
        }
        if let Some(directory) = inventory.open_root(&store.join("snapshots")) {
            for (name, kind) in inventory.list(&directory) {
                let path = directory.path.join(&name);
                if inventory.matched(name.as_bytes()) { inventory.pending("global_snapshot_manifest_overlap", &path); }
                if kind != FileType::Directory { inventory.pending("unsafe_snapshot_entry", &path); continue; }
                let Some(snapshot) = inventory.directory(&directory, &name) else { continue };
                for (leaf, kind) in inventory.list(&snapshot) {
                    if kind == FileType::RegularFile && (leaf.as_bytes().ends_with(b".cozytensors") || leaf.as_bytes().ends_with(b".json")) {
                        inventory.file(&snapshot, &leaf, "global_snapshot_metadata_overlap", false);
                    } else if !matches!(kind, FileType::RegularFile | FileType::Directory) { inventory.pending("unsafe_snapshot_entry", &snapshot.path.join(&leaf)); }
                    else if kind == FileType::Directory && inventory.legacy_executions { inventory.pending("nested_snapshot_closure_uninspected", &snapshot.path.join(&leaf)); }
                }
            }
        }
        for area in ["holds", "files", "uses"] {
            let Some(directory) = inventory.open_root(&store.join(area)) else { continue };
            for (name, kind) in inventory.list(&directory) {
                if area == "holds" && name == "gc.lock" { continue; }
                if kind != FileType::RegularFile { inventory.pending("unsafe_store_retention_entry", &directory.path.join(&name)); continue; }
                let Some(bytes) = inventory.read(&directory, &name) else { continue };
                let path = directory.path.join(&name);
                if inventory.matched(&bytes) || inventory.matched(name.as_bytes()) { inventory.pending("global_retention_possible_overlap", &path); }
                if area == "files" {
                    match serde_json::from_slice::<serde_json::Value>(&bytes) {
                        Ok(value) => if value["path"].as_str().is_some_and(|path| Path::new(path).starts_with(&scope.view_root)) { inventory.pending("global_header_root_for_project", &path); },
                        Err(_) => inventory.pending("unparsed_global_header_root", &path),
                    }
                }
            }
        }
        // A referenced provider/local model remains pending even if it looks published or its
        // ref has gone. Neither a familiar model name nor a shared digest proves independence.
        for model in footprint.provider_models.iter().take(MAX_TERMS) { if !model.is_empty() { inventory.report.pending.push(format!("referenced_global_provider_model:{model}")); } }
        if let Some(directory) = inventory.open_root(&store.join("refs")) {
            for (org, kind) in inventory.list(&directory) {
                if kind != FileType::Directory { if org != ".lock" { inventory.pending("unsafe_global_model_ref_entry", &directory.path.join(&org)); } continue; }
                let Some(org) = inventory.directory(&directory, &org) else { continue };
                for (name, kind) in inventory.list(&org) {
                    let path = org.path.join(&name);
                    if kind != FileType::RegularFile { inventory.pending("unsafe_global_model_ref_entry", &path); continue; }
                    let Some(bytes) = inventory.read(&org, &name) else { continue };
                    if inventory.matched(&bytes) { inventory.pending("global_model_ref_possible_overlap", &path); }
                    match serde_json::from_slice::<serde_json::Value>(&bytes) {
                        Ok(value) => if value["name"].as_str().is_some_and(|name| footprint.provider_models.iter().take(MAX_TERMS).any(|model| model.eq_ignore_ascii_case(name))) { inventory.pending("referenced_global_model_record", &path); },
                        Err(_) => inventory.pending("unparsed_global_model_ref", &path),
                    }
                }
            }
        }
        for manifest in footprint.manifests.iter().take(MAX_TERMS) {
            let Some(digest) = digest(manifest) else { inventory.report.pending.push("unparsed_legacy_manifest_reference".into()); continue };
            let Some(blobs) = inventory.open_root(&store.join("blobs")) else { continue };
            let Some(first) = inventory.directory(&blobs, digest[..2].as_ref()) else { continue };
            let Some(second) = inventory.directory(&first, digest[2..4].as_ref()) else { continue };
            let path = second.path.join(&digest);
            match rustix::fs::statat(&second.fd, digest.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(found) if FileType::from_raw_mode(found.st_mode) == FileType::RegularFile => { inventory.inspected("referenced_manifest_candidate", &path); inventory.pending("referenced_global_manifest_object", &path); },
                Ok(_) => inventory.pending("unsafe_referenced_manifest_object", &path),
                Err(rustix::io::Errno::NOENT) => (),
                Err(_) => inventory.pending("unreadable_referenced_manifest_object", &path),
            }
        }
        inventory.report.pending.sort(); inventory.report.pending.dedup();
        inventory.report.inspected.sort(); inventory.report.inspected.dedup();
        Ok(inventory.report)
    }
}
