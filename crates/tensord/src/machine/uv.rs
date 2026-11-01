//! uv as the machine runs it, one way: an image's uv, else the pinned one this user's cozy tools
//! hold (the CLI's too, `platform::tools`), fetched on first need; and the environment, cache and
//! Pythons every uv command gets, whatever it builds (a package's environment, the update's
//! resolution, the toolchain).
use platform::{paths::Home, tools};
use sha2::{Digest, Sha256};
use std::{
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

/// The cache uv keeps wheels and environments' files in, which `reclaim` manages: the machine's
/// own, never its owner's general one (shared with their other projects), so what a dropped
/// environment or PyTorch base alone used goes with it.
#[derive(Clone, Debug)]
enum Cache {
    /// An image's baked one, linked so overlayfs never copies its payload (Torch) into each
    /// environment; the image's own entries, which its interpreter hard-links, stay.
    Seeded(PathBuf),
    /// Under the machine's engine, on its filesystem: environments hard-link it.
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
        let cache = match seed.is_dir() {
            true => Cache::Seeded(seed),
            false => Cache::Machine(own.join("uv-cache")),
        };
        Self { path, cache, pythons: own.join("python") }
    }

    /// A uv command: the machine's environment without credentials or names the executor seal
    /// owns (a proxy and its certificates hold), over this machine's cache and Pythons.
    pub fn command(&self) -> io::Result<Command> {
        self.fetched()?;
        let mut command = Command::new(&self.path);
        command.env_clear().envs(crate::launch_identity::inherited()).env("UV_PYTHON_INSTALL_DIR", &self.pythons);
        match &self.cache {
            Cache::Seeded(cache) => command.env("UV_CACHE_DIR", cache).env("UV_LINK_MODE", "symlink"),
            Cache::Machine(cache) => command.env("UV_CACHE_DIR", cache),
        };
        Ok(command)
    }

    /// The cache the machine manages: its own or an image's.
    pub fn cache(&self) -> &Path {
        match &self.cache {
            Cache::Machine(cache) | Cache::Seeded(cache) => cache,
        }
    }

    /// Whether installs link into an image's seeded cache.
    pub fn seeded(&self) -> bool {
        matches!(self.cache, Cache::Seeded(_))
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
