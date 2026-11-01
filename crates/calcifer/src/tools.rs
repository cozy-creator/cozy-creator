//! The tools cozy runs and never bundles that it fetches itself on first need, pinned builds held
//! to their sha256, into `~/.cozy/tools`: ffmpeg (cuts, clips) and uv (local code, authoring).
//! Calcifer fetches what its own work needs; the CLI calls the same functions until its work moves
//! behind the API (cl-716).
use std::{
    fs::{File, OpenOptions},
    io::{self, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use platform::{
    paths::Home,
    tools::{self, Archive, Found, Install, Tool},
};
use protocol::error::{Error, Kind};
use sha2::{Digest as _, Sha256};
use tensorfs::err::{Code, Refusal};

/// What a fetch says as it goes ("fetching ffmpeg: <url>"): the CLI prints it, Calcifer logs it.
pub type Say<'a> = &'a (dyn Fn(&str) + Sync);

/// ffmpeg and ffprobe, with the build that answers (`ffmpeg -version`'s first line).
#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub build: String,
}

pub fn path() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// An ffmpeg that encodes libx264 here, the pinned build first.
fn usable_ffmpeg(home: &Home) -> Option<Ffmpeg> {
    let (found, ffprobe) = tools::ffmpeg(&home.tools(), &path())?;
    tools::ffmpeg_encodes(&found.path, "libx264").then_some(Ffmpeg { ffmpeg: found.path, ffprobe, build: found.version })
}

/// ffmpeg for work that needs it: the pinned build, else PATH's when it encodes libx264, else the
/// pinned build fetched now, on this first need.
pub async fn ffmpeg(home: &Home, say: Say<'_>) -> Result<Ffmpeg, Error> {
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
pub async fn uv(home: &Home, say: Say<'_>) -> Result<PathBuf, Error> {
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

/// The words that install `tool` here.
pub fn install_hint(tool: &Tool, path: &[PathBuf]) -> String {
    match tool.install {
        Install::Fetched => "cozy fetches it when a feature first needs it".into(),
        Install::System(package) => tools::system_install(package, path)
            .unwrap_or_else(|| format!("install {package} with this computer's package manager")),
        Install::Note(text) => format!("install {text}"),
    }
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
        let (mut download, got) = download(archive.url, &part, &mut |_| {}).await.map_err(failed)?;
        if got != archive.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(Error::new(
                Kind::Validation,
                format!("{name}_digest_mismatch"),
                format!("{} is sha256:{got}, not the pinned sha256:{}", archive.url, archive.sha256),
            ));
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

/// Downloads `url` into `part` from the bytes it holds (what an earlier fetch reached resumes),
/// telling `moved` each byte position reached, and answers the file and its sha256 for the caller
/// to hold to its pin. A transfer that stalls or fails is asked again from the byte reached, by
/// TensorFS's one rule; once nothing moves for its stall rule it is refused, its bytes kept for
/// the next fetch. Another fetch of the same file holds it: this one is refused.
pub async fn download(url: &str, part: &Path, moved: &mut (dyn FnMut(u64) + Send)) -> Result<(File, String), String> {
    let failed = |e: io::Error| format!("{}: {e}", part.display());
    let mut file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(part).map_err(failed)?;
    if !platform::lock::try_claim(&file).map_err(|e| e.to_string())? {
        return Err(format!("another cozy command is fetching {url} now; run this again once it has"));
    }
    let (from, mut at) = (file.metadata().map_err(failed)?.len(), 0);
    moved(from);
    let mut sink = |start: u64, frame: &[u8]| -> tensorfs::err::Result<()> {
        let written = (|| {
            if start != at {
                file.set_len(start)?;
                file.seek(SeekFrom::Start(start))?;
            }
            file.write_all(frame)
        })();
        written.map_err(|e| Refusal { code: Code::IO_FAILED, detail: failed(e) })?;
        at = start + frame.len() as u64;
        moved(at);
        Ok(())
    };
    tensorfs::fetch::get_pinned(url, from, &mut sink).await.map_err(|e| e.to_string())?;
    file.rewind().map_err(failed)?;
    let mut hash = Sha256::new();
    io::copy(&mut file, &mut hash).map_err(failed)?;
    file.rewind().map_err(failed)?;
    Ok((file, hex::encode(hash.finalize())))
}
