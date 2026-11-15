//! This running program as a file (cl-981). An update renames a new `cozy` over the one a process started from, which keeps
//! running: `current_exe()` then names `<path> (deleted)` on Linux and the newer build elsewhere. A process that starts itself
//! again (Calcifer's filesystem supervisor, TensorD's re-exec) or reads its own bytes goes through [`running`].
use crate::{Error, Result, durable::FileId};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

/// Where this process started from, and that file's identity then.
pub struct Launch {
    /// The path it started from (`<home>/bin/cozy`): where an update puts the next build.
    pub path: PathBuf,
    /// That path's file then, through any link (Homebrew's `Cellar/<formula>/<version>/bin/cozy`).
    file: PathBuf,
    id: FileId,
    /// This build's version (`api::VERSION`), as `cozy --version` says it.
    version: &'static str,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

/// Records the launch of this build (`version`), first thing in `main`, before anything can rename another build over it.
/// Again: the one recorded.
pub fn record(version: &'static str) -> Result<&'static Launch> {
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
    Ok(LAUNCH.get_or_init(|| Launch { path, file, id, version }))
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

    /// Whether the program at `file` is this build: it says this version.
    fn says_this_build(&self, file: &Path) -> bool {
        let said = Command::new(file).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output();
        said.is_ok_and(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == format!("cozy {}", self.version))
    }
}

/// A path that runs this very build, to start it again or read it: on Linux `/proc/self/exe`, the running file however it was
/// replaced; elsewhere the file it started from while that is still this build, or, replaced, while the file there says this
/// build's version (the same release installed again). [`Error::Updated`] once another build is there: a child of it would not
/// be this one's.
pub fn running() -> Result<PathBuf> {
    if cfg!(target_os = "linux") {
        return Ok(PathBuf::from("/proc/self/exe"));
    }
    let Some(launch) = launch() else {
        return std::env::current_exe().map_err(Error::os("find this program"));
    };
    if FileId::at(&launch.file).is_ok_and(|id| id == launch.id) {
        return Ok(launch.file.clone());
    }
    match [&launch.file, &launch.path].into_iter().find(|file| launch.says_this_build(file)) {
        Some(file) => Ok(file.clone()),
        None => Err(Error::Updated { path: launch.path.clone() }),
    }
}
