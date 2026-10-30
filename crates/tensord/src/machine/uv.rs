//! uv as the machine runs it, one way: an image's uv, else the pinned one this user's cozy tools
//! hold (the CLI's too, `platform::tools`), fetched on first need; and the environment, cache and
//! Pythons every uv command gets, whatever it builds (a package's environment, the update's
//! resolution, the toolchain).
use platform::{paths::Home, tools};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, PoisonError},
};

#[derive(Clone, Debug)]
pub struct Uv {
    path: PathBuf,
    cache: Cache,
    pythons: PathBuf,
}

/// The cache uv keeps wheels and environments' files in.
#[derive(Clone, Debug)]
enum Cache {
    /// An image's baked one, linked so overlayfs never copies its payload (Torch) into each
    /// environment; nothing in it is ever removed.
    Seeded(PathBuf),
    /// On a local machine its owner's, as the owner's own uv resolves it, so a package's Torch
    /// stack is fetched and kept once; uv and its owner manage it.
    Owner(PathBuf),
    /// The machine's own, which `reclaim` manages.
    Machine(PathBuf),
}

impl Uv {
    /// The machine at `root` with its engine directory `engine`.
    pub fn of(root: &Path, engine: &Path, rental: bool) -> Self {
        let image = root.join("usr/local/bin").join(platform::paths::exe("uv"));
        let home = std::env::home_dir().unwrap_or_else(|| root.into());
        let path = match rental && image.is_file() {
            true => image,
            false => tools::uv_dir(&Home::under(&home).tools()).join(platform::paths::exe("uv")),
        };
        let (seed, own) = (root.join("opt/cozy/dependency-seed/uv-cache"), engine.join("published"));
        let owner = owner(protocol::config::environment()).filter(|_| !rental);
        let cache = match (seed.is_dir(), &owner) {
            (true, _) => Cache::Seeded(seed),
            (false, Some((cache, _))) => Cache::Owner(cache.clone()),
            (false, None) => Cache::Machine(own.join("uv-cache")),
        };
        let pythons = owner.map_or_else(|| own.join("python"), |(_, pythons)| pythons);
        Self { path, cache, pythons }
    }

    /// A uv command: the machine's environment without credentials or names the executor seal
    /// owns (a proxy and its certificates hold), over this machine's cache and Pythons.
    pub fn command(&self) -> io::Result<Command> {
        self.fetched()?;
        let mut command = Command::new(&self.path);
        command.env_clear().envs(crate::launch_identity::inherited()).env("UV_PYTHON_INSTALL_DIR", &self.pythons);
        match &self.cache {
            Cache::Seeded(cache) => command.env("UV_CACHE_DIR", cache).env("UV_LINK_MODE", "symlink"),
            Cache::Owner(cache) | Cache::Machine(cache) => command.env("UV_CACHE_DIR", cache),
        };
        Ok(command)
    }

    /// The cache the machine itself manages; none when an image's or the owner's serves.
    pub fn machine_cache(&self) -> Option<&Path> {
        match &self.cache {
            Cache::Machine(cache) => Some(cache),
            _ => None,
        }
    }

    /// Whether installs link into an image's seeded cache.
    pub fn seeded(&self) -> bool {
        matches!(self.cache, Cache::Seeded(_))
    }

    /// The unpacked wheels environments link their files from, where this machine may write
    /// beside them: an image's seeded cache or its own (an owner's cache is the owner's uv's).
    /// uv links an archive's bytecode into each later environment with its sources.
    pub fn archives(&self) -> Option<PathBuf> {
        match &self.cache {
            Cache::Seeded(cache) | Cache::Machine(cache) => Some(cache.join("archive-v0")),
            Cache::Owner(_) => None,
        }
    }

    /// The pinned uv in place: fetched when absent, held to its sha256, its directory put in
    /// place whole.
    fn fetched(&self) -> io::Result<()> {
        static FETCHING: Mutex<()> = Mutex::new(());
        if self.path.exists() {
            return Ok(());
        }
        let _fetching = FETCHING.lock().unwrap_or_else(PoisonError::into_inner);
        if self.path.exists() {
            return Ok(());
        }
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        let archive = tools::uv_build().ok_or_else(|| io::Error::other(format!("uv {} has no build for {os}/{arch}", tools::UV_VERSION)))?;
        let dir = self.path.parent().expect("uv in its directory");
        tracing::info!("fetching uv {} to {}", tools::UV_VERSION, dir.display());
        // Its own thread and runtime: callers may be on a runtime's blocking threads.
        let fetched = std::thread::scope(|scope| {
            scope
                .spawn(|| -> io::Result<Vec<u8>> {
                    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                    runtime.block_on(async {
                        let response = reqwest::get(archive.url).await.and_then(|r| r.error_for_status());
                        let bytes = response.map_err(io::Error::other)?.bytes().await.map_err(io::Error::other)?;
                        Ok(bytes.to_vec())
                    })
                })
                .join()
                .map_err(|_| io::Error::other("the uv fetch panicked"))?
        })?;
        if format!("{:x}", Sha256::digest(&fetched)) != archive.sha256 {
            return Err(io::Error::other(format!("{} does not match its pinned sha256", archive.url)));
        }
        let staging = dir.with_file_name(format!(".{}.fetching", tools::UV_VERSION));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        tools::extract(&archive, &mut io::Cursor::new(fetched), &staging)?;
        settle(&staging.join(platform::paths::exe("uv")))?;
        match fs::rename(&staging, dir) {
            // Another fetch put it in place meanwhile: theirs is as good.
            Err(_) if self.path.exists() => {
                let _ = fs::remove_dir_all(&staging);
                Ok(())
            }
            moved => moved,
        }
    }
}

/// Where uv keeps its cache and managed Pythons for the owner whose environment `env` is: as
/// uv itself resolves them (its variables, then XDG, then the home directory's defaults).
fn owner(env: &BTreeMap<String, String>) -> Option<(PathBuf, PathBuf)> {
    let path = |name: &str| env.get(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let home = path("HOME");
    let base = |xdg: &str, default: &str| path(xdg).or_else(|| home.as_ref().map(|h| h.join(default)));
    Some((
        path("UV_CACHE_DIR").or_else(|| Some(base("XDG_CACHE_HOME", ".cache")?.join("uv")))?,
        path("UV_PYTHON_INSTALL_DIR").or_else(|| Some(base("XDG_DATA_HOME", ".local/share")?.join("uv/python")))?,
    ))
}

/// Waits until the executable just written at `path` runs. A child another thread forked while
/// its file was open for writing holds that descriptor until the child execs, and any exec of
/// the file meanwhile fails "Text file busy" (ETXTBSY). Once it runs, no writer is left, so `uv`
/// appears under its name only then and no later exec of it meets that.
fn settle(path: &Path) -> io::Result<()> {
    loop {
        match Command::new(path).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }
            Err(error) => return Err(error),
            Ok(_) => return Ok(()),
        }
    }
}
