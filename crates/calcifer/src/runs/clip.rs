//! A part of a video a mention names (`<take3:48-336>`, an orbit's `<cat:120deg>`): cut once with ffmpeg into
//! its project's clip cache, frame-accurate, and sent in its source's place. Calcifer cuts it, with
//! cozy's ffmpeg (`crate::tools::ffmpeg`, fetched on first need).
use std::{
    fmt,
    fs::{self, FileTimes},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use api::{
    calcifer::{Note, NoteLevel},
    error::{Error, Kind},
};
use sha2::{Digest as _, Sha256};
use tokio::sync::OwnedRwLockReadGuard;

use crate::tools::Media;

const THREADS: &str = "4";
/// Re-encoded at the source's size and rate. Part of each clip's key.
#[rustfmt::skip]
const ENCODE: &[&str] = &[
    "-map_metadata", "-1", "-map_chapters", "-1",
    "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p",
    "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-threads", THREADS, "-f", "mp4",
];
/// A frame, as a picture. Part of each picture's key.
#[rustfmt::skip]
const STILL: &[&str] = &["-map", "0:v:0", "-frames:v", "1", "-map_metadata", "-1", "-c:v", "png", "-f", "image2", "-update", "1"];
/// A clip not sent for this long goes the next time one is cut.
const UNUSED: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Where clips are kept, and the cozy home whose ffmpeg cuts them.
#[derive(Debug)]
pub(crate) struct Clips {
    pub dir: PathBuf,
    pub home: platform::paths::Home,
}

/// What's cut of a video, in its own frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Range {
    /// Frames `from` to `to` (not counting it), or to its end, as a clip.
    Frames { from: u32, to: Option<u32> },
    /// Frames `from` to its end and on from its first to `to`, as one clip: an orbit's turn across 0°. It has no sound.
    Wrapped { from: u32, to: u32 },
    /// One frame, as a picture.
    Frame(u32),
}

/// A clip sent in its source's place.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cut {
    pub field: String,
    pub source: PathBuf,
    pub range: Range,
    pub clip: PathBuf,
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Frames { from, to } => write!(f, "frames {from}-{}", to.map_or_else(String::new, |to| to.to_string())),
            Self::Wrapped { from, to } => write!(f, "frames {from}- and 0-{to}"),
            Self::Frame(at) => write!(f, "frame {at}"),
        }
    }
}

impl Range {
    /// Where ffmpeg starts reading at `fps`, half a frame before the first, so that one is the first it keeps.
    fn start(from: u32, fps: f64) -> f64 {
        (f64::from(from) - 0.5).max(0.0) / fps
    }

    /// Where it stops reading before frame `to`, half a frame before it, so the frame before it is the last it keeps.
    fn end(to: u32, fps: f64) -> f64 {
        (f64::from(to) - 0.5) / fps
    }

    /// When it is in its source, at `fps`.
    fn timed(self, fps: f64) -> String {
        let at = |frame: u32| clock(f64::from(frame) / fps);
        match self {
            Self::Frames { from, to } => format!("{} to {}", at(from), to.map_or_else(|| "the end".into(), at)),
            Self::Wrapped { from, to } => format!("{} to the end and 0:00 to {}", at(from), at(to)),
            Self::Frame(frame) => at(frame),
        }
    }
}

impl fmt::Display for Cut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is {} of {}, sent as {}",
            self.field,
            self.range,
            self.source.display(),
            self.clip.display()
        )
    }
}

impl Clips {
    /// `range` of `source`, whose rate is `fps`, as a clip or a picture: the one cut before for this version of the file, else
    /// cut now. What it did is said in `notes`.
    pub(crate) async fn cut(&self, field: &str, source: &Path, range: Range, fps: f64, notes: &mut Vec<Note>, access: OwnedRwLockReadGuard<()>) -> Result<Cut, Error> {
        fn say(notes: &mut Vec<Note>, message: String) {
            notes.push(Note { level: NoteLevel::Info, message });
        }
        let key = key(source, range, fps).ok_or_else(|| {
            Error::new(
                Kind::NotFound,
                "input_file_unreadable",
                format!("input file {}", source.display()),
            )
        })?;
        let extension = if matches!(range, Range::Frame(_)) { "png" } else { "mp4" };
        let clip = self.dir.join(format!("{}.{key}.{extension}", stem(source)));
        let cut = Cut {
            field: field.into(),
            source: source.into(),
            range,
            clip: clip.clone(),
        };
        if clip.is_file() {
            let _ = fs::File::options()
                .write(true)
                .open(&clip)
                .and_then(|f| f.set_times(FileTimes::new().set_accessed(SystemTime::now())));
            say(notes, format!("{field}: {} of {}, cut before: {}", range.timed(fps), source.display(), clip.display()));
            return Ok(cut);
        }
        let ffmpeg = crate::tools::ffmpeg(&self.home, &|line| tracing::info!("{line}")).await?;
        let past = within(&ffmpeg, field, source, range, fps)?;
        if let Some(end) = past {
            notes.push(Note { level: NoteLevel::Warning, message: format!("{field}: {} runs past the end of {}: cut to its end, {}", range.timed(fps), source.display(), clock(end)) });
        }
        say(notes, format!("{field}: cutting {} of {}", range.timed(fps), source.display()));
        let began = Instant::now();
        let (source, dir) = (source.to_owned(), self.dir.clone());
        let (encoded, _access) = tokio::task::spawn_blocking(move || {
            // The encoder can finish after its async caller has been canceled.
            (encode(&ffmpeg, &source, range, fps, past.is_some(), &dir, &clip), access)
        })
        .await
        .map_err(|e| Error::new(Kind::Internal, "clip_cut", e.to_string()))?;
        encoded?;
        say(notes, format!("{field}: cut in {:.1} s: {}", began.elapsed().as_secs_f64(), cut.clip.display()));
        Ok(cut)
    }
}

/// Whether `range` starts inside the source; Some(its duration) when it runs past its end.
fn within(ffmpeg: &crate::tools::Ffmpeg, at: &str, source: &Path, range: Range, fps: f64) -> Result<Option<f64>, Error> {
    let duration = probe(&ffmpeg.ffprobe, source)?;
    let (first, last) = match range {
        Range::Frames { from, to } => (from, to),
        Range::Wrapped { from, to } => (from.max(to), None),
        Range::Frame(at) => (at, None),
    };
    let start = f64::from(first) / fps;
    if start >= duration {
        return Err(invalid(format!("{at}: frame {first} at {} is past the end of {} ({})", clock(start), source.display(), clock(duration))));
    }
    Ok(last.filter(|&to| Range::end(to, fps) > duration + 0.0005).map(|_| duration))
}

/// `range` of `source`, encoded into `clip` whole or not at all: to its end when it runs `past` it.
fn encode(ffmpeg: &crate::tools::Ffmpeg, source: &Path, range: Range, fps: f64, past: bool, dir: &Path, clip: &Path) -> Result<(), Error> {
    let failed = |e: std::io::Error| {
        Error::new(
            Kind::Internal,
            "clip_cut",
            format!("{}: {e}", dir.display()),
        )
    };
    fs::create_dir_all(dir).map_err(failed)?;
    sweep(dir);
    let suffix = clip.extension().map_or_else(String::new, |extension| format!(".{}", extension.to_string_lossy()));
    let part = tempfile::Builder::new()
        .prefix(".cut-")
        .suffix(&suffix)
        .tempfile_in(dir)
        .map_err(failed)?;
    let mut command = Media::niced(&ffmpeg.ffmpeg);
    command.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-threads", THREADS]);
    // Each read starts and stops half a frame from a frame, so a time rounded to the millisecond can't take a frame more or
    // less, and a count holds the clip to its frames.
    match range {
        Range::Frames { from, to } => {
            command.args(["-ss", &decimal(Range::start(from, fps))]);
            if let Some(to) = to.filter(|_| !past) {
                command.args(["-to", &decimal(Range::end(to, fps))]);
            }
            command.input(source).args(["-map", "0:v:0", "-map", "0:a:0?"]);
            if let Some(to) = to.filter(|_| !past) {
                command.args(["-frames:v", &(to - from).to_string()]);
            }
            command.args(ENCODE);
        }
        Range::Wrapped { from, to } => {
            command.args(["-ss", &decimal(Range::start(from, fps))]).input(source);
            command.args(["-to", &decimal(Range::end(to, fps))]).input(source);
            command.args(["-filter_complex", "[0:v:0][1:v:0]concat=n=2:v=1:a=0[v]", "-map", "[v]"]).args(ENCODE);
        }
        Range::Frame(at) => {
            command.args(["-ss", &decimal(Range::start(at, fps))]).input(source).args(STILL);
        }
    }
    let out = command.arg(part.path()).output().map_err(failed)?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = said.lines().collect();
        let tail = &lines[lines.len().saturating_sub(3)..];
        return Err(Error::new(
            Kind::Validation,
            "clip_cut",
            format!(
                "ffmpeg could not cut {}: {}",
                source.display(),
                tail.join(" | ")
            ),
        ));
    }
    if part.as_file().metadata().map_err(failed)?.len() == 0 {
        return Err(invalid(format!("{range} of {} holds no frame", source.display())));
    }
    part.persist(clip).map_err(|e| failed(e.error))?;
    Ok(())
}

/// The container's duration, in seconds.
fn probe(ffprobe: &Path, source: &Path) -> Result<f64, Error> {
    let out = Media::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .input(source)
        .output()
        .map_err(|e| {
            Error::new(
                Kind::Internal,
                "clip_probe",
                format!("{}: {e}", ffprobe.display()),
            )
        })?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| {
            invalid(format!(
                "ffprobe reads no duration from {}",
                source.display()
            ))
        })
}

/// Names one version of the source (path, length, modification time), the range at its rate and the encode.
fn key(source: &Path, range: Range, fps: f64) -> Option<String> {
    let path = fs::canonicalize(source).ok()?;
    let meta = fs::metadata(&path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let (cut, encode) = match range {
        Range::Frames { from, to } => (format!("{from}\0{}", to.map_or_else(|| "end".into(), |to| to.to_string())), ENCODE),
        Range::Wrapped { from, to } => (format!("{from}\0wraps\0{to}"), ENCODE),
        Range::Frame(at) => (format!("{at}\0frame"), STILL),
    };
    let named = format!("{}\0{}\0{modified}\0{cut}\0{fps}\0{}", path.display(), meta.len(), encode.join(" "));
    Some(hex::encode(Sha256::digest(named.as_bytes()))[..16].to_owned())
}

/// The source's name, as a clip's file name starts.
fn stem(source: &Path) -> String {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let kept: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if kept.is_empty() { "clip".into() } else { kept }
}

/// Removes clips not sent for `UNUSED`.
fn sweep(dir: &Path) {
    let before = SystemTime::now() - UNUSED;
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let unused = entry
            .metadata()
            .and_then(|m| m.accessed())
            .is_ok_and(|at| at < before);
        if unused {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// `m:ss[.fff]`, or `h:mm:ss[.fff]` from an hour.
fn clock(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    let (h, m, s, frac) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    let frac = match frac {
        0 => String::new(),
        f => format!(".{f:03}").trim_end_matches('0').to_owned(),
    };
    match h {
        0 => format!("{m}:{s:02}{frac}"),
        h => format!("{h}:{m:02}:{s:02}{frac}"),
    }
}

/// Seconds to the millisecond, as ffmpeg reads them.
fn decimal(seconds: f64) -> String {
    format!("{:.3}", (seconds * 1000.0).round() / 1000.0)
}

fn invalid(message: String) -> Error {
    Error::new(Kind::Validation, "input_segment_invalid", message)
}
