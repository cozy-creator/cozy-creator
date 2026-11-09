//! The tools cozy runs and never bundles that it fetches itself on first need, pinned builds held
//! to their sha256, into `~/.cozy/tools`: ffmpeg (cuts, clips) and uv (local code, authoring), as
//! Calcifer's own work first needs them.
use std::path::{Path, PathBuf};

use api::error::{Error, Kind};
use platform::{
    paths::Home,
    tools::{self, Archive, Found, Install, Tool, install_hint},
};

/// What a fetch says as it goes ("fetching ffmpeg: <url>"): the CLI prints it, Calcifer logs it.
pub(crate) type Say<'a> = &'a (dyn Fn(&str) + Sync);

/// ffmpeg and ffprobe, with the build that answers (`ffmpeg -version`'s first line).
#[derive(Clone, Debug)]
pub(crate) struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub build: String,
}

pub(crate) fn path() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// Held by this process's one fetch of ffmpeg.
static FETCHING_FFMPEG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// An ffmpeg that encodes libx264 here, the pinned build first.
fn usable_ffmpeg(home: &Home) -> Option<Ffmpeg> {
    let (found, ffprobe) = tools::ffmpeg(&home.tools(), &path())?;
    tools::ffmpeg_encodes(&found.path, "libx264").then_some(Ffmpeg { ffmpeg: found.path, ffprobe, build: found.version })
}

/// ffmpeg for work that needs it: the pinned build, else PATH's when it encodes libx264, else the
/// pinned build fetched now, on this first need.
pub(crate) async fn ffmpeg(home: &Home, say: Say<'_>) -> Result<Ffmpeg, Error> {
    if let Some(found) = usable_ffmpeg(home) {
        return Ok(found);
    }
    // Several things want it first at once: one fetches, and the rest find what it fetched.
    let _one = FETCHING_FFMPEG.lock().await;
    if let Some(found) = usable_ffmpeg(home) {
        return Ok(found);
    }
    let archives = tools::ffmpeg_build();
    if archives.is_empty() {
        return Err(unpinned("ffmpeg"));
    }
    let dir = tools::ffmpeg_dir(&home.tools());
    fetch(say, "ffmpeg", archives, &dir).await?;
    usable_ffmpeg(home).ok_or_else(|| {
        Error::new(Kind::Internal, "ffmpeg_unavailable", format!("the pinned ffmpeg in {} does not answer", dir.display()))
    })
}

/// uv: the pinned build, else PATH's, else the pinned build fetched now.
pub(crate) async fn uv(home: &Home, say: Say<'_>) -> Result<PathBuf, Error> {
    if let Some(found) = tools::uv(&home.tools(), &path()) {
        return Ok(found.path);
    }
    let Some(archive) = tools::uv_build() else {
        return Err(unpinned("uv"));
    };
    let dir = tools::uv_dir(&home.tools());
    fetch(say, "uv", std::slice::from_ref(&archive), &dir).await?;
    let found: Option<Found> = tools::uv(&home.tools(), &[]);
    found.map(|found| found.path).ok_or_else(|| {
        Error::new(Kind::Internal, "uv_unavailable", format!("the pinned uv in {} does not answer", dir.display()))
    })
}

/// No build is pinned for this OS/CPU: the OS's own command installs it.
fn unpinned(name: &'static str) -> Error {
    let tool = tools::tool(name);
    Error::new(
        Kind::Unavailable,
        format!("{name}_unavailable"),
        format!("cozy pins no {name} build for {} {}", std::env::consts::OS, std::env::consts::ARCH),
    )
    .next(install_hint(&Tool { install: Install::System(name), ..*tool }, &path()))
    .next("cozy doctor")
}

/// Downloads `archives`, each held to its sha256, and puts their members into `dir` whole: a
/// staging directory renamed into place, so `dir` is either complete or absent.
async fn fetch(say: Say<'_>, name: &str, archives: &[Archive], dir: &Path) -> Result<(), Error> {
    let failed = |e: String| {
        Error::new(Kind::Unavailable, format!("{name}_unavailable"), e)
            .remedy("the next command that needs it fetches it again")
            .next("cozy doctor")
    };
    let parent = dir.parent().expect("a tool's directory has a parent");
    std::fs::create_dir_all(parent).map_err(|e| failed(format!("{}: {e}", parent.display())))?;
    let staging = tempfile::tempdir_in(parent).map_err(|e| failed(e.to_string()))?;
    for archive in archives {
        say(&format!("fetching {name}: {}", archive.url));
        let part = parent.join(format!(".{name}-{}.part", &archive.sha256[..12]));
        let (mut download, got) = tensorfs::fetch::get_pinned_into(archive.url, &part, &mut |_| {}).await.map_err(failed)?;
        if got != archive.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(Error::new(
                Kind::Validation,
                format!("{name}_digest_mismatch"),
                format!("{} is sha256:{got}, not the pinned sha256:{}", archive.url, archive.sha256),
            )
        .next("cozy doctor"));
        }
        tools::extract(archive, &mut download, staging.path()).map_err(|e| failed(e.to_string()))?;
        let _ = std::fs::remove_file(&part);
    }
    let staged = staging.keep();
    match std::fs::rename(&staged, dir) {
        Ok(()) => Ok(()),
        // Another command installed it meanwhile: theirs is as good.
        Err(_) if dir.is_dir() => {
            let _ = std::fs::remove_dir_all(&staged);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staged);
            Err(failed(format!("{}: {e}", dir.display())))
        }
    }
}
