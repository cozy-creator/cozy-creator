//! Retire obsolete, disposable service captures and browser redirects. Canonical run
//! logs belong to the project journal; these mixed raw copies have no safe project owner.
use std::{fs, io, path::Path};

fn absent(error: &io::Error) -> bool { error.kind() == io::ErrorKind::NotFound }
fn raw_log(name: &str) -> bool {
    name == "calcifer.log" || name.strip_prefix("calcifer.log.")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
}
fn machine_capture(name: &str) -> bool {
    ["tensord", "memory", "tensorfs"].iter().any(|stem|
        [format!("{stem}.log"), format!("{stem}.log.1"), format!("{stem}.after")].iter().any(|known| known == name))
}
fn redirect(name: &str) -> bool {
    name.strip_prefix("open-").and_then(|name| name.strip_suffix(".html"))
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}
fn remove_file(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) { Ok(metadata) => metadata, Err(error) if absent(&error) => return Ok(()), Err(error) => return Err(error) };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::other("an obsolete capture has an unexpected file type"));
    }
    fs::remove_file(path)?;
    platform::durable::sync_dir(path.parent().ok_or_else(|| io::Error::other("capture has no parent"))?)
        .map_err(io::Error::other)
}
fn entries(path: &Path) -> io::Result<Vec<fs::DirEntry>> {
    match fs::symlink_metadata(path) {
        Err(error) if absent(&error) => return Ok(vec![]),
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => return Err(io::Error::other("an obsolete capture directory changed")),
        _ => {}
    }
    fs::read_dir(path)?.collect()
}

/// Called only by the new exclusive Calcifer holder. No old capture writer remains.
/// Unknown files and symlinks are retained and make completion fail, never traversed.
pub(crate) fn retire(calcifer: &Path) -> io::Result<()> {
    for entry in entries(calcifer)? {
        if entry.file_name().to_str().is_some_and(raw_log) { remove_file(&entry.path())?; }
    }
    let logs = calcifer.join("machine-logs");
    for machine in entries(&logs)? {
        for entry in entries(&machine.path())? {
            if !entry.file_name().to_str().is_some_and(machine_capture) {
                return Err(io::Error::other("an old machine capture needs an ownership decision"));
            }
            remove_file(&entry.path())?;
        }
        fs::remove_dir(machine.path())?;
        platform::durable::sync_dir(&logs).map_err(io::Error::other)?;
    }
    if logs.exists() { fs::remove_dir(&logs)?; platform::durable::sync_dir(calcifer).map_err(io::Error::other)?; }
    if let Some(home) = calcifer.parent() {
        for entry in entries(&home.join("cache"))? {
            if entry.file_name().to_str().is_some_and(redirect) { remove_file(&entry.path())?; }
        }
    }
    Ok(())
}

/// A read error or unretired capture blocks a clean-storage claim.
pub(crate) fn pending(calcifer: &Path) -> bool {
    let Ok(found) = entries(calcifer) else { return true };
    if found.iter().any(|entry| entry.file_name().to_str().is_some_and(raw_log)) { return true; }
    if fs::symlink_metadata(calcifer.join("machine-logs")).is_ok() { return true; }
    // Old hashed runtime folders are project traces even after their mounts close.
    match entries(&calcifer.join("vaults")) {
        Ok(found) if found.is_empty() => {},
        _ => return true,
    }
    let Some(home) = calcifer.parent() else { return true };
    match entries(&home.join("cache")) {
        Ok(found) => found.iter().any(|entry| entry.file_name().to_str().is_some_and(redirect)),
        Err(_) => true,
    }
}
