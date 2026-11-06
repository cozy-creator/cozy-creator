//! Durability off the caller's path. A busy disk can hold an fsync for seconds: ext4's journal commits every
//! process's data at once, so one waits on everyone's. What asks `soon` returns at once, and one writer per
//! process syncs each path in the order asked, many at a time.
use std::{
    fmt::Display,
    mem,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, PoisonError},
};

use tracing::warn;

struct Owed {
    paths: Vec<PathBuf>,
    writing: bool,
}

static OWED: Mutex<Owed> = Mutex::new(Owed { paths: Vec::new(), writing: false });
static ASKED: Condvar = Condvar::new();

/// Makes `path` durable soon: a file's bytes and its name in its directory, or a directory's entries.
pub fn soon(path: &Path) {
    let mut owed = OWED.lock().unwrap_or_else(PoisonError::into_inner);
    if !owed.paths.iter().any(|held| held == path) {
        owed.paths.push(path.to_owned());
    }
    if mem::replace(&mut owed.writing, true) {
        return ASKED.notify_one();
    }
    drop(owed);
    if std::thread::Builder::new().name("durable".into()).spawn(write).is_err() {
        // No thread to spare: this caller waits, as every write did before.
        let mut owed = OWED.lock().unwrap_or_else(PoisonError::into_inner);
        let paths = mem::take(&mut owed.paths);
        owed.writing = false;
        drop(owed);
        sync(&paths);
    }
}

fn write() {
    loop {
        let paths = {
            let mut owed = OWED.lock().unwrap_or_else(PoisonError::into_inner);
            while owed.paths.is_empty() {
                owed = ASKED.wait(owed).unwrap_or_else(PoisonError::into_inner);
            }
            mem::take(&mut owed.paths)
        };
        sync(&paths);
    }
}

/// Each file's bytes in order, then each directory once: one journal commit covers most of them.
fn sync(paths: &[PathBuf]) {
    let mut dirs: Vec<&Path> = vec![];
    for path in paths {
        let dir = match path.is_dir() {
            true => Some(path.as_path()),
            false => {
                said(path, super::sync_file(path));
                path.parent().filter(|dir| !dir.as_os_str().is_empty())
            }
        };
        if let Some(dir) = dir.filter(|dir| !dirs.contains(dir)) {
            dirs.push(dir);
        }
    }
    for dir in dirs {
        said(dir, super::sync_dir(dir));
    }
}

/// A file replaced or removed since it was asked of needs nothing.
fn said(path: &Path, synced: Result<(), impl Display>) {
    if let Err(error) = synced
        && path.exists()
    {
        warn!("{}: not made durable: {error}", path.display());
    }
}
