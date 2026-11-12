//! Waking when directories' files change: inotify on Linux, kqueue on macOS, change
//! notifications on Windows. A wake says only that something changed; the caller rereads.
//!
//! Every OS wakes on an entry created, renamed in or out, or removed. Writes wake only for the
//! files [`Writes`] names: Linux and macOS on a plain write; Windows once the cache flushes
//! (measured: not within 10 s), so a follower there rereads on `most` too.
use crate::Result;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

/// Which files' writes in a watched directory wake it, beside its entries changing.
#[derive(Clone, Copy, Debug)]
pub enum Writes {
    None,
    Named(&'static [&'static str]),
    All,
}

impl Writes {
    #[cfg_attr(windows, allow(dead_code))]
    fn counts(self, name: &[u8]) -> bool {
        match self {
            Writes::None => false,
            Writes::Named(names) => names.iter().any(|n| n.as_bytes() == name),
            Writes::All => true,
        }
    }
}

/// A directory watched: a missing one is watched once it appears, as another's entry.
struct Dir {
    path: PathBuf,
    writes: Writes,
}

/// Directories whose changes wake [`DirWatch::wait`]; so does its [`Waker`].
pub(crate) struct DirWatch(imp::Watch);

/// Wakes a [`DirWatch`]'s wait from any thread.
#[derive(Clone)]
pub(crate) struct Waker(Arc<imp::Wake>);

impl DirWatch {
    pub(crate) fn new(dir: &Path) -> Result<Self> {
        Self::of(&[(dir, Writes::All)])
    }

    pub(crate) fn of(dirs: &[(&Path, Writes)]) -> Result<Self> {
        let dirs = dirs.iter().map(|&(path, writes)| Dir { path: path.to_owned(), writes }).collect();
        imp::Watch::new(dirs).map(Self)
    }

    /// Waits until a file changes, it is woken, or `most` passes; true when one changed.
    pub(crate) fn wait(&mut self, most: Option<Duration>) -> Result<bool> {
        self.0.wait(most)
    }

    pub(crate) fn waker(&self) -> Waker {
        Waker(self.0.wake())
    }
}

impl Waker {
    pub(crate) fn wake(&self) {
        self.0.wake();
    }
}

/// Calls `changed` on a thread of its own each time something in its directories changes, until dropped.
pub struct Watcher {
    waker: Waker,
    ended: Arc<AtomicBool>,
}

impl Watcher {
    pub fn new(name: &str, dirs: &[(&Path, Writes)], mut changed: impl FnMut() + Send + 'static) -> Result<Self> {
        let mut watch = DirWatch::of(dirs)?;
        let ended = Arc::new(AtomicBool::new(false));
        let watcher = Self { waker: watch.waker(), ended: ended.clone() };
        let named = name.to_owned();
        std::thread::Builder::new()
            .name(named.clone())
            .spawn(move || {
                while !ended.load(Ordering::Acquire) {
                    match watch.wait(None) {
                        Ok(true) if !ended.load(Ordering::Acquire) => changed(),
                        Ok(_) => {}
                        Err(error) => return tracing::warn!("{named}: watching ends: {error}"),
                    }
                }
            })
            .map_err(crate::Error::os("spawn"))?;
        Ok(watcher)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Release);
        self.waker.wake();
    }
}
