//! Derived weights (a LoRA merged into a model's blocks, a converted lane) kept on disk so a later process reads them
//! instead of making them again: in the OS's temporary storage, never a store or `~/.cozy` (cl-1318; Paul, 11-19: "bake
//! the LoRA and write it to disc in a temporary location (which can be reclaimed later)"; "you can persist derived weights
//! to disc, so long as they are in a temporary folder and it's easy to reclaim that space as needed").
//!
//! - Where: `/var/tmp` on Linux, which the FHS keeps on disk where `/tmp` is often tmpfs (RAM); elsewhere the OS's
//!   temporary directory. One that is RAM (tmpfs, ramfs) keeps nothing. `cozy-derived-<uid>/<boot>/<slot>/<page>`:
//!   only this boot's directory is read, and the first process of a boot removes the others, so a reboot clears it.
//! - How much: a hard cap of an eighth of that filesystem, never into the store's reserve (2% of the disk, 1 to 10 GiB).
//!   A page past it displaces the least recently used pages this process hasn't used; else it isn't kept, and that block
//!   is made again next time. So a model larger than the cap keeps its first blocks, never trading them for its last.
//! - Given back first: a store on the same filesystem that is short takes these before any model (`give_back`).
//! - A page is written under a `.part` name its writer holds locked, then renamed whole; one whose writer died (`kill -9`)
//!   is removed at the next look. Nothing syncs: a crash of the machine is a reboot, which discards the boot's directory.
//!   Nothing sweeps: what is kept is what fits the cap.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use crate::format::is_digest;

/// The cache's share of its filesystem.
const SHARE: u64 = 8;

/// A page found: its key and its file, open.
pub struct Found {
    pub page: String,
    pub file: File,
    pub len: u64,
}

struct Cache {
    /// `cozy-derived-<uid>`, which holds the lock.
    dir: PathBuf,
    /// This boot's directory in it.
    root: PathBuf,
}

fn cache() -> Option<&'static Cache> {
    static CACHE: OnceLock<Option<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(|| match open() {
            Ok(cache) => cache,
            Err(error) => {
                tracing::warn!("tensorfs: derived weights stay in memory: {error}");
                None
            }
        })
        .as_ref()
}

fn base() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let candidates = [PathBuf::from("/var/tmp"), std::env::temp_dir()];
    #[cfg(not(target_os = "linux"))]
    let candidates = [std::env::temp_dir()];
    candidates.into_iter().find(|path| path.is_dir() && !matches!(platform::durable::filesystem(path).kind, "tmpfs" | "ramfs"))
}

fn open() -> io::Result<Option<Cache>> {
    began();
    let Some(base) = base() else { return Ok(None) };
    #[cfg(unix)]
    let dir = base.join(format!("cozy-derived-{}", rustix::process::geteuid().as_raw()));
    #[cfg(not(unix))]
    let dir = base.join("cozy-derived");
    private(&dir)?;
    let boot: String = platform::process::boot_id()
        .map_err(|error| io::Error::other(error.to_string()))?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if boot.is_empty() {
        return Ok(None);
    }
    let cache = Cache { root: dir.join(&boot), dir };
    let _held = cache.lock()?;
    for entry in fs::read_dir(&cache.dir)? {
        let path = entry?.path();
        if path != cache.root && path.file_name().is_some_and(|name| name != ".lock") {
            let gone = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
            if let Err(error) = gone {
                tracing::warn!("tensorfs: an earlier boot's derived weights stay at {}: {error}", path.display());
            }
        }
    }
    fs::create_dir_all(&cache.root)?;
    Ok(Some(cache))
}

/// `dir` made this user's alone, or refused: another user may have made it first in a shared temporary directory.
fn private(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(io::Error::other(format!("{} is not this user's directory", dir.display())));
        }
        if meta.mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)
}

struct Page {
    path: PathBuf,
    len: u64,
    used: SystemTime,
}

/// When this process began, as far as the cache can tell: a page used since then is this process's.
fn began() -> SystemTime {
    static BEGAN: OnceLock<SystemTime> = OnceLock::new();
    *BEGAN.get_or_init(SystemTime::now)
}

impl Cache {
    fn lock(&self) -> io::Result<File> {
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join(".lock"))?;
        file.lock()?;
        Ok(file)
    }

    /// Every page, least recently used first, and the bytes of writes in flight; a part whose writer is gone is
    /// removed. Under the lock.
    fn pages(&self) -> io::Result<(Vec<Page>, u64)> {
        let (mut found, mut writing) = (Vec::new(), 0);
        for slot in list(&self.root)? {
            found.extend(pages(&slot)?);
            for part in list(&slot)?.into_iter().filter(|path| is_part(path)) {
                match File::open(&part).map(|file| file.try_lock().is_ok()) {
                    Ok(true) => {
                        let _ = fs::remove_file(&part);
                    }
                    _ => writing += fs::metadata(&part).map_or(0, |meta| meta.len()),
                }
            }
        }
        found.sort_by_key(|page| page.used);
        Ok((found, writing))
    }

    /// Room for `len` bytes: within the cap and beside the store's reserve, pages this process hasn't used given up
    /// least recently used first. False: it isn't kept. Under the lock.
    fn room(&self, len: u64) -> io::Result<bool> {
        let space = platform::durable::space(&self.root).map_err(|error| io::Error::other(error.to_string()))?;
        let (cap, reserve) = (space.total / SHARE, (space.total / 50).clamp(1 << 30, 10 << 30));
        let (pages, writing) = self.pages()?;
        let mut used: u64 = writing + pages.iter().map(|p| p.len).sum::<u64>();
        let mut free = space.free;
        let mut older = pages.iter().filter(|p| p.used < began());
        while used.saturating_add(len) > cap || free < len.saturating_add(reserve) {
            let Some(page) = older.next() else { return Ok(false) };
            if fs::remove_file(&page.path).is_ok() {
                used -= page.len;
                free += page.len;
            }
        }
        Ok(true)
    }
}

fn list(dir: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(entries) => entries.map(|entry| entry.map(|e| e.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn is_part(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.') && n.ends_with(".part"))
}

fn pages(slot: &Path) -> io::Result<Vec<Page>> {
    let mut found = Vec::new();
    for path in list(slot)? {
        if !path.file_name().and_then(|n| n.to_str()).is_some_and(is_digest) {
            continue;
        }
        if let Ok(meta) = fs::metadata(&path) {
            found.push(Page { len: meta.len(), used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), path });
        }
    }
    Ok(found)
}

fn named(slot: &str, page: Option<&str>) -> io::Result<()> {
    if is_digest(slot) && page.is_none_or(is_digest) {
        return Ok(());
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, "a derived page and its slot are named by digests"))
}

/// Page `page` of `slot`, or with none the one of that slot used last; it is then young.
pub fn find(slot: &str, page: Option<&str>) -> io::Result<Option<Found>> {
    named(slot, page)?;
    let Some(cache) = cache() else { return Ok(None) };
    let dir = cache.root.join(slot);
    let path = match page {
        Some(page) => dir.join(page),
        None => match pages(&dir)?.into_iter().max_by_key(|p| p.used) {
            Some(newest) => newest.path,
            None => return Ok(None),
        },
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let len = file.metadata()?.len();
    let _ = file.set_modified(SystemTime::now());
    let page = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    Ok(Some(Found { page, file, len }))
}

/// Keeps `bytes` as page `page` of `slot`. False: no cache, or no room for it (`Cache::room`).
pub fn keep(slot: &str, page: &str, bytes: &[u8]) -> io::Result<bool> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    named(slot, Some(page))?;
    let Some(cache) = cache() else { return Ok(false) };
    let len = bytes.len() as u64;
    let dir = cache.root.join(slot);
    let (path, mut file) = {
        let _held = cache.lock()?;
        if !cache.room(len)? {
            return Ok(false);
        }
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!(".{page}.{}.{}.part", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
        let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
        file.lock()?;
        // Counted from here on (`Cache::pages`), before the lock lets another writer judge the room.
        file.set_len(len)?;
        (path, file)
    };
    let written = file.write_all(bytes).and_then(|()| fs::rename(&path, dir.join(page)));
    if written.is_err() {
        let _ = fs::remove_file(&path);
    }
    written.map(|()| true)
}

/// Whether the cache's filesystem is `path`'s.
fn shares(cache: &Cache, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((fs::metadata(&cache.root), fs::metadata(path)), (Ok(a), Ok(b)) if a.dev() == b.dev())
    }
    #[cfg(not(unix))]
    {
        let volume = |p: &Path| fs::canonicalize(p).ok().and_then(|p| p.components().next().map(|c| c.as_os_str().to_owned()));
        matches!((volume(&cache.root), volume(path)), (Some(a), Some(b)) if a == b)
    }
}

/// What the cache holds on `path`'s filesystem: what it could give back for it.
pub fn spare(path: &Path) -> u64 {
    let Some(cache) = cache().filter(|cache| shares(cache, path)) else { return 0 };
    let Ok(_held) = cache.lock() else { return 0 };
    cache.pages().map_or(0, |(pages, _)| pages.iter().map(|p| p.len).sum())
}

/// Gives back at least `need` bytes of `path`'s filesystem if the cache holds them there, least recently used first;
/// what it freed. A store short of room calls it before giving up any model.
pub fn give_back(path: &Path, need: u64) -> u64 {
    let Some(cache) = cache().filter(|cache| need > 0 && shares(cache, path)) else { return 0 };
    let Ok(_held) = cache.lock() else { return 0 };
    let mut freed = 0;
    for page in cache.pages().map(|(pages, _)| pages).unwrap_or_default() {
        if freed >= need {
            break;
        }
        if fs::remove_file(&page.path).is_ok() {
            freed += page.len;
        }
    }
    if freed > 0 {
        tracing::info!("tensorfs: freed {} MiB of derived pages, least recently used first", freed >> 20);
    }
    freed
}
