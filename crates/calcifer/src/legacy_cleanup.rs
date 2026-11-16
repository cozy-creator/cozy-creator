//! The legacy hard cut's files (cl-1003): what older Calcifers kept beside the store and nothing reads now. Copies of the old
//! store (`state.db.pre-*`), old JSON state and project index files, raw service captures, old browser redirects, the old digest
//! memo, copies of code no current source admitted, and clips and cuts of projects' files kept outside them. Each is deleted by its known name; anything else found
//! there stays, untouched. One-time code: it goes in the release after.
use std::{collections::HashSet, fs, path::Path};

fn remove(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else { return };
    let _ = if metadata.is_dir() && !metadata.file_type().is_symlink() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
}

fn raw_log(name: &str) -> bool {
    name == "calcifer.log" || name.strip_prefix("calcifer.log.").is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A copy of the store an older Calcifer, its migrations or a hand made beside it before changing it (`state.db.pre-<why>`, with
/// its `-wal` and `-shm`): the whole old history, encrypted projects' included, in plaintext.
fn backup(name: &str) -> bool { name.strip_prefix("state.db.pre-").is_some_and(|why| !why.is_empty()) }

fn redirect(name: &str) -> bool {
    name.strip_prefix("open-").and_then(|name| name.strip_suffix(".html"))
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// Run once the old tables are gone. `admitted`: the trees of independent source the store admitted (`sha256:<hex>`).
pub(crate) fn retire(calcifer: &Path, admitted: &HashSet<String>) {
    for name in ["projects.json", "sessions.json", "tools.json", "here.json", "checkpoints.json", "after", "machine-logs", "state.db.cut"] {
        remove(&calcifer.join(name));
    }
    for entry in fs::read_dir(calcifer).into_iter().flatten().flatten() {
        if entry.file_name().to_str().is_some_and(|name| raw_log(name) || backup(name)) { remove(&entry.path()); }
    }
    // A copy of a project's code kept here before projects kept their own (`.vault-calcifer`): never admitted, never read.
    for entry in fs::read_dir(calcifer.join("code")).into_iter().flatten().flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()) && !admitted.contains(&format!("sha256:{name}")) {
            remove(&entry.path());
        }
    }
    // A file an old Calcifer kept of each project it opened, which nothing reads.
    let index = calcifer.join("projects");
    let old_index = fs::read_dir(&index).into_iter().flatten().flatten()
        .all(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with("project-") && name.ends_with(".json")));
    if old_index { remove(&index); }
    let Some(home) = calcifer.parent() else { return };
    remove(&home.join("cache").join("digests"));
    // Clips and cuts of projects' files made before each project kept its own (`.vault-calcifer`): made again as asked.
    for dir in [Some(home.join("cache").join("clips")), platform::paths::cache().map(|cache| cache.join("cuts"))].into_iter().flatten() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() { remove(&entry.path()); }
    }
    for entry in fs::read_dir(home.join("cache")).into_iter().flatten().flatten() {
        if entry.file_name().to_str().is_some_and(redirect) { remove(&entry.path()); }
    }
}
