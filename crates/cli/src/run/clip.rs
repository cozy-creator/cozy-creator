//! A segment of a local video, named by a media fragment on its path (`clip.mp4#t=3:19,3:32`):
//! cut once with ffmpeg into `~/.cozy/cache/clips`, frame-accurate, and sent in its source's place.
use std::{
    fmt,
    fs::{self, FileTimes},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use protocol::error::{Error, Kind};
use sha2::{Digest as _, Sha256};

use crate::{Context, output, tools::Ffmpeg};

/// What follows a path to name a segment of it (W3C media fragments' temporal dimension).
const FRAGMENT: &str = "#t=";
const MIN_SECONDS: f64 = 2.0;
const THREADS: &str = "4";
/// Re-encoded at the source's size and rate. Part of each clip's key.
#[rustfmt::skip]
const ENCODE: &[&str] = &[
    "-map", "0:v:0", "-map", "0:a:0?", "-map_metadata", "-1", "-map_chapters", "-1",
    "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p",
    "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-threads", THREADS, "-f", "mp4",
];
/// A clip not sent for this long goes the next time one is cut.
const UNUSED: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Where clips are kept, and the command whose ffmpeg cuts them.
#[derive(Debug)]
pub struct Clips<'a> {
    pub ctx: &'a Context,
    pub dir: PathBuf,
}

/// Seconds into the source; no end: the source's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub start: f64,
    pub end: Option<f64>,
}

/// A clip sent in its source's place.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub field: String,
    pub source: PathBuf,
    pub range: Range,
    pub clip: PathBuf,
}

/// `source` without its `#t=start,end` fragment, and the range it names, if any.
pub fn fragment(source: &Path, at: &str) -> Result<(PathBuf, Option<Range>), Error> {
    let text = source.to_string_lossy();
    let Some(split) = text.rfind(FRAGMENT) else {
        return Ok((source.into(), None));
    };
    let (path, times) = (&text[..split], &text[split + FRAGMENT.len()..]);
    let malformed = || {
        invalid(format!("{at}: {text:?} names no range after {FRAGMENT}"))
            .remedy("path#t=<start>,<end> in whole seconds, m:ss or h:mm:ss; either may be left out (#t=5, #t=,0:12)")
    };
    let (start, end) = times.split_once(',').unwrap_or((times, ""));
    if path.is_empty() || (start.is_empty() && end.is_empty()) {
        return Err(malformed());
    }
    let time = |t: &str| -> Result<Option<f64>, Error> {
        if t.is_empty() {
            return Ok(None);
        }
        seconds(t).map(|s| Some(s as f64)).ok_or_else(|| {
            let fraction = t.contains('.');
            invalid(match fraction {
                true => format!("{at}: {t} has a fraction of a second; a range is whole seconds"),
                false => format!("{at}: {t} is not a time"),
            })
            .remedy("whole seconds (199), m:ss (3:19) or h:mm:ss (1:03:19)")
        })
    };
    let range = Range {
        start: time(start.strip_prefix("npt:").unwrap_or(start))?.unwrap_or(0.0),
        end: time(end)?,
    };
    if let Some(end) = range.end {
        range.long_enough(end, at)?;
    }
    Ok((path.into(), Some(range)))
}

impl Range {
    fn long_enough(self, end: f64, at: &str) -> Result<Self, Error> {
        if end <= self.start {
            return Err(invalid(format!(
                "{at}: end {} is not after start {}",
                clock(end),
                clock(self.start)
            )));
        }
        if end - self.start < MIN_SECONDS {
            return Err(invalid(format!(
                "{at}: {self} is {:.3} s; a segment is at least {MIN_SECONDS} s",
                end - self.start
            )));
        }
        Ok(self)
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let end = self.end.map_or_else(|| "the end".into(), clock);
        write!(f, "{} to {end}", clock(self.start))
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

impl Clips<'_> {
    /// `range` of `source` as a clip: the one cut before for this version of the file, else cut now.
    pub async fn cut(&self, field: &str, source: &Path, range: Range) -> Result<Cut, Error> {
        let key = key(source, range).ok_or_else(|| {
            Error::new(
                Kind::NotFound,
                "input_file_unreadable",
                format!("input file {}", source.display()),
            )
        })?;
        let clip = self.dir.join(format!("{}.{key}.mp4", stem(source)));
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
            output::say(
                self.ctx,
                &format!(
                    "{field}: {range} of {}, cut before: {}",
                    source.display(),
                    clip.display()
                ),
            );
            return Ok(cut);
        }
        let ffmpeg = crate::tools::ffmpeg(self.ctx).await?;
        let end = within(&ffmpeg, field, source, range)?;
        output::say(
            self.ctx,
            &format!("{field}: cutting {range} of {}", source.display()),
        );
        let began = Instant::now();
        let (source, dir) = (source.to_owned(), self.dir.clone());
        tokio::task::spawn_blocking(move || {
            encode(&ffmpeg, &source, range.start, end, &dir, &clip)
        })
        .await
        .map_err(|e| Error::new(Kind::Internal, "clip_cut", e.to_string()))??;
        output::say(
            self.ctx,
            &format!(
                "{field}: cut in {:.1} s: {}",
                began.elapsed().as_secs_f64(),
                cut.clip.display()
            ),
        );
        Ok(cut)
    }
}

/// The range's end, once it is checked against the source's duration.
fn within(ffmpeg: &Ffmpeg, at: &str, source: &Path, range: Range) -> Result<f64, Error> {
    let duration = probe(&ffmpeg.ffprobe, source)?;
    let past = |what: &str, t: f64| {
        invalid(format!(
            "{at}: {what} {} is past the end of {} ({})",
            clock(t),
            source.display(),
            clock(duration)
        ))
    };
    if range.start >= duration {
        return Err(past("start", range.start));
    }
    let end = range.end.unwrap_or(duration);
    if end > duration + 0.0005 {
        return Err(past("end", end));
    }
    range.long_enough(end, at)?;
    Ok(end)
}

/// `start..end` of `source`, encoded into `clip` whole or not at all.
fn encode(
    ffmpeg: &Ffmpeg,
    source: &Path,
    start: f64,
    end: f64,
    dir: &Path,
    clip: &Path,
) -> Result<(), Error> {
    let failed = |e: std::io::Error| {
        Error::new(
            Kind::Internal,
            "clip_cut",
            format!("{}: {e}", dir.display()),
        )
    };
    fs::create_dir_all(dir).map_err(failed)?;
    sweep(dir);
    let part = tempfile::Builder::new()
        .prefix(".cut-")
        .suffix(".mp4")
        .tempfile_in(dir)
        .map_err(failed)?;
    let out = niced(&ffmpeg.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-threads",
            THREADS,
        ])
        .args(["-ss", &decimal(start), "-to", &decimal(end), "-i"])
        .arg(source)
        .args(ENCODE)
        .arg(part.path())
        .output()
        .map_err(failed)?;
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
    part.persist(clip).map_err(|e| failed(e.error))?;
    Ok(())
}

/// The container's duration, in seconds.
fn probe(ffprobe: &Path, source: &Path) -> Result<f64, Error> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(source)
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

#[cfg(unix)]
fn niced(program: &Path) -> Command {
    let mut command = Command::new("nice");
    command.args(["-n", "19"]).arg(program);
    command
}

#[cfg(not(unix))]
fn niced(program: &Path) -> Command {
    Command::new(program)
}

/// Names one version of the source (path, length, modification time), the range and the encode.
fn key(source: &Path, range: Range) -> Option<String> {
    let path = fs::canonicalize(source).ok()?;
    let meta = fs::metadata(&path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let end = range.end.map_or_else(|| "end".into(), decimal);
    let named = format!(
        "{}\0{}\0{modified}\0{}\0{end}\0{}",
        path.display(),
        meta.len(),
        decimal(range.start),
        ENCODE.join(" ")
    );
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

/// Whole seconds written as `s`, `m:ss` or `h:mm:ss`.
fn seconds(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    parts.iter().enumerate().try_fold(0u64, |total, (i, part)| {
        let n: u64 = part.parse().ok().filter(|_| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))?;
        (i == 0 || n < 60).then_some(total * 60 + n)
    })
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
