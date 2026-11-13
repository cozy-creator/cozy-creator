//! This running program as a file (cl-981). An update renames a new `cozy` over the one a process started from, which keeps
//! running: `current_exe()` then names `<path> (deleted)` on Linux and the newer build elsewhere. A process that starts itself
//! again (Calcifer's filesystem supervisor, TensorD's re-exec) or reads its own bytes goes through [`running`].
use crate::{Error, Result, durable::FileId};
use std::{path::PathBuf, sync::OnceLock};

/// Where this process started from, and that file's identity then.
pub struct Launch {
    /// The path it started from (`<home>/bin/cozy`): where an update puts the next build.
    pub path: PathBuf,
    /// That path's file then, through any link (Homebrew's `Cellar/<formula>/<version>/bin/cozy`).
    file: PathBuf,
    id: FileId,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

/// Records the launch, first thing in `main`, before anything can rename another build over it. Again: the one recorded.
pub fn record() -> Result<&'static Launch> {
    if let Some(launch) = LAUNCH.get() {
        return Ok(launch);
    }
    let path = std::env::current_exe().map_err(Error::os("find this program"))?;
    // On Windows, canonicalize answers a `\\?\` path, and no link leads to cozy.exe.
    let file = match cfg!(windows) {
        true => path.clone(),
        false => path.canonicalize().map_err(Error::io("resolve", &path))?,
    };
    let id = FileId::at(&file)?;
    Ok(LAUNCH.get_or_init(|| Launch { path, file, id }))
}

/// The launch, once recorded; `None` in a process `main` never recorded (tests).
pub fn launch() -> Option<&'static Launch> {
    LAUNCH.get()
}

impl Launch {
    /// Whether the launch path holds another file now: an update replaced it.
    pub fn swapped(&self) -> bool {
        FileId::at(&self.path).is_ok_and(|id| id != self.id)
    }
}

/// A path that runs this very build, to start it again or read it: on Linux `/proc/self/exe`, the running file however it was
/// replaced; elsewhere the file it started from while that is still this build. [`Error::Updated`] once an update replaced it
/// there: a child of another build would not be this one's.
pub fn running() -> Result<PathBuf> {
    if cfg!(target_os = "linux") {
        return Ok(PathBuf::from("/proc/self/exe"));
    }
    let Some(launch) = launch() else {
        return std::env::current_exe().map_err(Error::os("find this program"));
    };
    match FileId::at(&launch.file) {
        Ok(id) if id == launch.id => Ok(launch.file.clone()),
        _ => Err(Error::Updated { path: launch.path.clone() }),
    }
}
