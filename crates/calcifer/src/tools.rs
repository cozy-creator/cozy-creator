//! The tools cozy runs and never bundles that it fetches itself on first need, pinned builds held
//! to their sha256, into `~/.cozy/tools`: ffmpeg (cuts, clips) and uv (local code, authoring), as
//! Calcifer's own work first needs them.
use std::{
    ffi::{OsStr, OsString},
    io::Read as _,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

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

/// The demuxers a file is read with: pictures, videos and sounds, and none that reads what a file names (a playlist, a `concat` script).
/// `image2` is a sequence reader: even a `%02d` in a parent directory can make it open another file. Images use their file demuxers.
const DEMUXERS: &str = "mov,matroska,avi,asf,mpeg,mpegts,flv,png_pipe,apng,jpeg_pipe,webp_pipe,gif,bmp_pipe,tiff_pipe,jpegxl_pipe,jpegxl_anim,psd_pipe,ico,mp3,ogg,flac,wav,aac,aiff,amr,ape,dsf";

/// A picture's single-file demuxer, by what its first bytes say it is. FFmpeg's probe takes a picture's filename for an `image2`
/// sequence, which `DEMUXERS` refuses: by its extension it beats `jpeg_pipe` on any JPEG whose end isn't in the first 2 KiB read, so
/// nearly every photo had no thumbnail (cl-888), and with `*`, `?` or `{` in the path, every picture. None: not a picture, or one whose
/// own probe wins (a PNG's, which tells an APNG), and its container is detected.
fn picture(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 12];
    let read = std::fs::File::open(path).and_then(|mut file| file.read(&mut head)).unwrap_or(0);
    let by_content = match &head[..read] {
        [0xFF, 0xD8, 0xFF, ..] => Some("jpeg_pipe"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P'] => Some("webp_pipe"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        [b'B', b'M', _, _, _, _, 0, 0, 0, 0, ..] => Some("bmp_pipe"),
        [b'I', b'I', 0x2A, 0, ..] | [b'M', b'M', 0, 0x2A, ..] => Some("tiff_pipe"),
        [b'8', b'B', b'P', b'S', ..] => Some("psd_pipe"),
        _ => None,
    };
    if by_content.is_some() || !path.to_string_lossy().contains(['*', '?', '{']) {
        return by_content;
    }
    // FFmpeg pre-probes these filenames as image2 before opening them, including Windows' \\?\ device paths.
    path.extension().and_then(OsStr::to_str).and_then(|extension| match extension.to_ascii_lowercase().as_str() {
        "png" => Some("png_pipe"),
        "apng" => Some("apng"),
        "jxl" => Some("jpegxl_pipe"),
        "ico" => Some("ico"),
        _ => None,
    })
}

/// One ffmpeg or ffprobe run, and the only way Calcifer starts either. What it reads is named by `input`, `joined` or `made`
/// alone, so it reads that file and nothing the file names: a run's output is a package's bytes, and a playlist or a `concat`
/// script in a video's name would have it read other files here, or open a URL.
pub(crate) struct Media(Command);

impl Media {
    pub(crate) fn new(program: &Path) -> Self {
        Self(Command::new(program))
    }

    /// At the lowest priority, where the OS has one.
    pub(crate) fn niced(program: &Path) -> Self {
        if cfg!(unix) {
            let mut command = Command::new("nice");
            command.args(["-n", "19"]).arg(program);
            return Self(command);
        }
        Self::new(program)
    }

    pub(crate) fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.0.arg(arg);
        self
    }

    pub(crate) fn args(&mut self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> &mut Self {
        self.0.args(args);
        self
    }

    /// The file at `path`, read from this computer's disk with a demuxer of `DEMUXERS`.
    pub(crate) fn input(&mut self, path: impl AsRef<OsStr>) -> &mut Self {
        self.args(Self::reading(path))
    }

    /// `input`'s arguments, for a run whose arguments are put together before it.
    pub(crate) fn reading(path: impl AsRef<OsStr>) -> Vec<OsString> {
        let path = path.as_ref();
        let mut args: Vec<OsString> = ["-protocol_whitelist", "file", "-format_whitelist", DEMUXERS].into_iter().map(OsString::from).collect();
        if let Some(demuxer) = picture(Path::new(path)) {
            args.extend(["-f", demuxer].into_iter().map(OsString::from));
        }
        args.extend([OsString::from("-i"), path.to_owned()]);
        args
    }

    /// `list`, a `concat` script Calcifer wrote itself, of MPEG-TS pieces it cut.
    pub(crate) fn joined(&mut self, list: &Path) -> &mut Self {
        self.args(["-f", "concat", "-safe", "0", "-protocol_whitelist", "file", "-format_whitelist", "concat,mpegts", "-i"]).arg(list)
    }

    /// `seconds` of the filter source `graph`, which reads no file.
    pub(crate) fn made(&mut self, graph: &str, seconds: f64) -> &mut Self {
        self.args(["-f", "lavfi", "-t", &format!("{seconds:.6}"), "-i", graph])
    }

    pub(crate) fn output(&mut self) -> std::io::Result<Output> {
        self.0.output()
    }

    /// Started with what it says on two pipes.
    pub(crate) fn piped(&mut self) -> std::io::Result<Child> {
        self.0.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    }
}

pub(crate) fn path() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// Held by this process's one fetch of each tool: several things want it first at once, one fetches, and the rest wait and find
/// what it fetched (cl-911: a second local code was refused `uv_unavailable` while the first fetched uv).
static FETCHING_FFMPEG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static FETCHING_UV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _one = FETCHING_UV.lock().await;
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
        tracing::info!(target: "cozy.service_health", tool = name, "fetching a tool");
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

#[cfg(test)]
#[path = "tools_media_tests.rs"]
mod media_tests;
