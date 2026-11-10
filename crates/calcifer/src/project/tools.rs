//! What Calcifer runs itself, free, with no machine, the reserved package `cozy`: an Edit, a film at the project's root and its
//! timeline, of videos and stills, and an `image`, one frame of a video. And what is measured of a file. Every in and out point is an
//! exact frame.
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use protocol::{
    error::{Error, Kind},
    project::{Clip, ClipNow, Timeline, TimelineNow, TrackNow},
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{self, Folder, Lock},
    format::ReadNow,
    graph::{mentions_in, name_key},
};

/// A video's width and height, frame rate and frame count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub width: u64,
    pub height: u64,
    pub fps: f64,
    pub frames: u64,
}

/// How long a still lasts when nothing says.
const STILL_S: f64 = 3.0;

/// A piece of a cut: the asset it reads, its file (here, its digest, its path in the project,
/// `files/<sha256>.<ext>`, and its shape as project.lock records it), and its frames from `start`
/// to `end` (exclusive) at the cut's rate. A still is one picture, held from 0 to `end`.
pub struct Piece {
    pub asset: String,
    pub path: PathBuf,
    pub digest: String,
    pub file: String,
    pub shape: Option<Shape>,
    pub still: bool,
    pub start: u64,
    pub end: u64,
}

impl Piece {
    /// Its frames at `fps` within its take, `end` at most the take's last; None before its take is
    /// probed.
    fn frames(&self, fps: f64) -> Option<(u64, u64)> {
        if self.still {
            return Some((0, self.end));
        }
        let frames = available(&self.shape?, fps);
        Some((self.start.min(frames), self.end.min(frames)))
    }
}

/// Whether a take of `shape` is retimed to `fps`: it runs at another rate.
fn retimed(shape: &Shape, fps: f64) -> bool {
    (shape.fps - fps).abs() > 0.01
}

/// How many frames a take of `shape` has at `fps`: its own, else as many as retiming makes.
fn available(shape: &Shape, fps: f64) -> u64 {
    match retimed(shape, fps) {
        false => shape.frames,
        true => (shape.frames as f64 * fps / shape.fps).floor() as u64,
    }
}

/// A cut: its length in frames, and the takes it retimes to the project's rate (`<asset> (<fps> fps)`).
pub struct Cutted {
    pub frames: u64,
    pub retimed: Vec<String>,
}

/// Takes from `pieces` what starts past its take's end, a take being what its function made however
/// long it was asked for, and says each.
pub fn past_end(pieces: &mut Vec<Piece>, fps: f64) -> Vec<String> {
    let mut said = vec![];
    pieces.retain(|piece| {
        let past = piece.frames(fps).is_some_and(|(start, end)| start >= end);
        if past {
            said.push(format!("the cut takes {} from frame {}, past its take's end: that cut is left out", piece.asset, piece.start));
        }
        !past
    });
    said
}

/// Why the film `files` holds isn't what a cut of these fields makes: its frames, measured, against what its pieces add up to, one a
/// piece being rounding. A film short of a piece isn't that cut's state, whatever made it. None until each is measured.
pub fn film_off(fields: &Map<String, Value>, reads: &[ReadNow], lock: &Lock, version: &folder::Version) -> Option<String> {
    let film = lock.files.get(version.files.get("video")?)?;
    if film.frames == 0 {
        return None;
    }
    let fps = Canvas::of(fields, &mut vec![]).fps;
    let (mut pieces, _) = pieces(fields, reads, &Folder::at(Path::new("")), lock);
    // What it says it went without isn't why it's short.
    pieces.retain(|piece| !version.without.iter().any(|left| name_key(&left.piece) == name_key(&piece.asset)));
    past_end(&mut pieces, fps);
    let made = measure(&pieces, fps)?.frames;
    (film.frames.abs_diff(made) > pieces.len() as u64).then(|| format!("its film holds {} frames, and its pieces add up to {made}", film.frames))
}

/// ffprobe's answer when the file at `path` isn't a video it reads.
pub fn unread(ffprobe: &Path, path: &Path) -> Option<String> {
    probe(ffprobe, path, false).err().map(|error| error.message)
}

/// A clip a cut goes without: why, in the words its closing line and `status` use, and what was said of it, for its warning.
pub struct LeftOut {
    pub asset: String,
    pub why: String,
    pub answer: String,
}

impl LeftOut {
    fn new(piece: &Piece, why: &str, answer: String) -> Self {
        Self { asset: piece.asset.clone(), why: why.to_owned(), answer }
    }
}

const FFPROBE_CANT: &str = "ffprobe can't read it";

/// Why a cut can't take each of `pieces` that measuring left with no shape: asked of ffprobe now, so it says what is true.
pub fn unreadable(ffprobe: &Path, pieces: &[Piece]) -> Result<Vec<LeftOut>, Error> {
    let mut left = vec![];
    for piece in pieces.iter().filter(|piece| !piece.still && piece.shape.is_none()) {
        left.push(match probe(ffprobe, &piece.path, false) {
            Ok(_) => LeftOut::new(piece, "it's no video", format!("{} is no video a cut takes: those are mp4, mov, m4v, webm and mkv", piece.path.display())),
            Err(error) if error.code == TOOL_WONT_START => return Err(error),
            Err(error) => LeftOut::new(piece, FFPROBE_CANT, error.message),
        });
    }
    Ok(left)
}

/// What is wrong with `piece` alone, found by decoding and encoding it as a whole encode does with nothing written, so a full disk is
/// no clip's doing. None: nothing. Err: the tool itself won't start.
fn fails_alone(ffmpeg: &Path, ffprobe: &Path, piece: &Piece, canvas: &Canvas) -> Result<Option<LeftOut>, Error> {
    let probed = match probe(ffprobe, &piece.path, false) {
        Ok(probed) => probed,
        Err(error) if error.code == TOOL_WONT_START => return Err(error),
        Err(error) => return Ok(Some(LeftOut::new(piece, FFPROBE_CANT, error.message))),
    };
    let fps = canvas.fps;
    let (start, end) = piece.frames(fps).unwrap_or((0, probed.frames));
    let (width, height) = canvas.edit_size(probed.size);
    let native = piece.shape.is_some_and(|shape| !retimed(&shape, fps));
    let video = whole_filter(piece.still, native, fps, (start, end), (width, height));
    let sound = sound_filter((start, end), fps);
    let mut command = std::process::Command::new(ffmpeg);
    command.args(["-y", "-v", "error"]);
    if piece.still {
        command.args(["-loop", "1", "-framerate", &fps.to_string(), "-t", &format!("{:.6}", (end + 1) as f64 / fps)]);
    }
    let done = command
        .args(crate::media::options(&piece.path)?)
        .arg("-i")
        .arg(&piece.path)
        .args(["-map", "0:v:0", "-map", "0:a:0?", "-vf", &video, "-af", &sound])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-f", "null", "-"])
        .output()
        .map_err(|e| wont_start("ffmpeg", &e))?;
    let said = String::from_utf8_lossy(&done.stderr);
    Ok((!done.status.success()).then(|| LeftOut::new(piece, "ffmpeg can't cut it", format!("ffmpeg: {}", said.trim().lines().last().unwrap_or("it failed")))))
}

/// `pieces` cut, less each clip that can't be (Paul, 2026-11-04: "why would it ever fail the cut?"): when a cut fails, each clip is tried
/// alone, and those that fail are left out and the rest cut again. If every clip passes alone, a whole encode takes the fast join's place.
/// If that fails too it is no clip's doing (a full disk, say), and that is the Err. The cut is None once no clip is left.
#[allow(clippy::too_many_arguments)]
pub fn cut_around(
    ffmpeg: &Path,
    ffprobe: &Path,
    folder: &Folder,
    mut pieces: Vec<Piece>,
    canvas: &Canvas,
    name: &str,
    cache: &Path,
    told: &(dyn Fn(f64) + Send + Sync),
) -> Result<(Option<(Cutted, PathBuf)>, Vec<LeftOut>), Error> {
    let (mut left, mut fast) = (vec![], true);
    while !pieces.is_empty() {
        let error = match cut(ffmpeg, ffprobe, folder, &pieces, canvas, name, cache, told, fast) {
            Ok(made) => return Ok((Some(made), left)),
            Err(error) if error.code == TOOL_WONT_START => return Err(error),
            Err(error) => error,
        };
        let mut failing = vec![];
        for piece in &pieces {
            failing.extend(fails_alone(ffmpeg, ffprobe, piece, canvas)?);
        }
        if failing.is_empty() {
            if !fast {
                return Err(error);
            }
            fast = false;
        }
        pieces.retain(|piece| !failing.iter().any(|out: &LeftOut| out.asset == piece.asset));
        left.extend(failing);
    }
    Ok((None, left))
}

/// A piece's frames as a whole encode takes them: retimed unless it runs at the cut's rate, a still's transparency going to black, then
/// cut by frame and filling the canvas.
fn whole_filter(still: bool, native: bool, fps: f64, (start, end): (u64, u64), (width, height): (u64, u64)) -> String {
    // A take at another rate is retimed first (each frame its source's nearest in time), then cut by frame like any other.
    let retime = match (still, native) {
        (true, _) => "premultiply=inplace=1,".to_owned(),
        (false, true) => String::new(),
        (false, false) => format!("fps={fps},"),
    };
    format!(
        "{retime}trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS,\
         scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1,format=yuv420p"
    )
}

/// A piece's sound at its frames' times.
fn sound_filter((start, end): (u64, u64), fps: f64) -> String {
    format!("atrim=start={}:end={},asetpts=PTS-STARTPTS", start as f64 / fps, end as f64 / fps)
}

/// The cut `pieces` make at `fps`, as project.lock records their takes; None before each is probed.
pub fn measure(pieces: &[Piece], fps: f64) -> Option<Cutted> {
    let frames = pieces.iter().map(|piece| piece.frames(fps).map(|(start, end)| end - start)).sum::<Option<u64>>()?;
    let mut takes: Vec<String> = vec![];
    for piece in pieces {
        let Some(shape) = piece.shape.filter(|shape| retimed(shape, fps)) else { continue };
        let take = format!("{} ({} fps)", piece.asset, shape.fps);
        if !takes.contains(&take) {
            takes.push(take);
        }
    }
    Some(Cutted { frames, retimed: takes })
}

/// How far a take's aspect may be from the canvas's and still be it: models round their sizes to
/// multiples, which moves it this much at most (832x480 is 2.5% from 16:9).
const ROUNDED: f64 = 0.03;

/// The canvas a cut fills: its aspect (None: its first take's), its short edge when `resolution` sets one, and its frame rate.
#[derive(Clone, Debug)]
pub struct Canvas {
    pub ratio: Option<f64>,
    pub short: Option<u64>,
    /// Frames per second: the cut's rate, which its positions count in.
    pub fps: f64,
}

impl Canvas {
    /// What a cut's fields say: `fps`, `aspect_ratio`, `resolution`. What can't be read is said, and left at its default.
    pub fn of(fields: &Map<String, Value>, said: &mut Vec<String>) -> Self {
        let fps = match fields.get("fps").and_then(Value::as_f64) {
            Some(fps) if fps.is_finite() && fps > 0.0 && fps <= 240.0 => fps,
            Some(fps) => {
                said.push(format!("fps {fps} is not a frame rate such as 24, 25 or 30: it cuts at 24"));
                24.0
            }
            None => 24.0,
        };
        let short = fields.get("resolution").map(text).and_then(|written| {
            let short = written.strip_suffix('p').and_then(|n| n.parse::<u64>().ok()).filter(|n| *n >= 16);
            if short.is_none() {
                said.push(format!("resolution {written} is not a short edge such as 1080p or 1440p: it is left out"));
            }
            short
        });
        let side = |text: &str| text.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0);
        let ratio = fields.get("aspect_ratio").map(text).and_then(|written| {
            let ratio = written.split_once(':').and_then(|(w, h)| Some(side(w)? / side(h)?));
            if ratio.is_none() {
                said.push(format!("aspect_ratio {written} is not W:H, such as 16:9 or 2.39:1: it is left out"));
            }
            ratio
        });
        Self { ratio, short, fps }
    }

    /// The cut's size: `resolution`'s, else the first take's own (`first`) when its aspect is this one
    /// but for how models round their sizes and its sides are even (H3's 1344x768 at 16:9), so takes of
    /// that size join with no encode; else this aspect at that take's short edge.
    pub fn edit_size(&self, first: (u64, u64)) -> (u64, u64) {
        let ratio = self.ratio.unwrap_or(first.0 as f64 / first.1 as f64);
        if let Some(short) = self.short {
            return size(ratio, short);
        }
        let rounded = (first.0 as f64 / first.1 as f64 / ratio - 1.0).abs() <= ROUNDED;
        match rounded && first.0 % 2 == 0 && first.1 % 2 == 0 {
            true => first,
            false => size(ratio, first.0.min(first.1)),
        }
    }
}

/// Width and height on this aspect for `short` as its short edge, each even.
fn size(ratio: f64, short: u64) -> (u64, u64) {
    let long = |s: f64| ((s / 2.0).round() as u64) * 2;
    match ratio >= 1.0 {
        true => (long(short as f64 * ratio), short),
        false => (short, long(short as f64 / ratio)),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The clips of an edit's `tracks`, the first for now: `<a>`, or a mapping holding a mention with `in` and `out` frames, or a still's `frames`.
pub fn clips_of(tracks: &Value) -> Vec<Clip> {
    let clip = |item: &Value| {
        let name = mentions_in(item).into_iter().next()?;
        Some(Clip { clip: format!("<{name}>"), first: item["in"].as_u64().filter(|first| *first > 0), out: item["out"].as_u64(), frames: item["frames"].as_u64(), sha256: None })
    };
    tracks[0]["clips"].as_array().into_iter().flatten().filter_map(clip).collect()
}

/// An edit's clips: its `tracks`, or with none, all it reads.
pub fn clips(fields: &Map<String, Value>, reads: &[ReadNow]) -> Vec<Clip> {
    match fields.get("tracks") {
        Some(tracks) => clips_of(tracks),
        None => reads.iter().map(|read| Clip { clip: format!("<{}>", read.name), ..Default::default() }).collect(),
    }
}

/// `fields` as the file writes them, the wire's `timeline` as `tracks`. What can be fixed is, and said.
pub fn in_the_file(fields: &Map<String, Value>) -> Result<(Map<String, Value>, Vec<String>), Error> {
    let mut written = fields.clone();
    let mut said = vec![];
    let Some(value) = written.remove("timeline") else { return Ok((written, said)) };
    if value.is_null() {
        written.insert("tracks".to_owned(), Value::Null);
        return Ok((written, said));
    }
    let timeline: Timeline = serde_json::from_value(value).map_err(|why| Error::new(Kind::Validation, "timeline_invalid", format!("timeline: {why}")))?;
    if timeline.tracks.len() > 1 {
        said.push(format!("an Edit has one track for now: the other {} are left out", timeline.tracks.len() - 1));
    }
    let clips: Vec<Value> = timeline.tracks.first().into_iter().flat_map(|track| &track.clips).filter_map(clip_in_the_file).collect();
    written.insert("tracks".to_owned(), json!([{"clips": clips}]));
    Ok((written, said))
}

/// A clip as `tracks` writes it. A clip's points can't cross: its out point is at least a frame past its in point.
fn clip_in_the_file(clip: &Clip) -> Option<Value> {
    let name = mentions_in(&json!(clip.clip)).into_iter().next().or_else(|| Some(clip.clip.trim().to_owned()).filter(|name| !name.is_empty()))?;
    let name = format!("<{name}>");
    if let Some(frames) = clip.frames {
        return Some(json!({"image": name, "frames": frames.max(1)}));
    }
    let first = clip.first.unwrap_or(0);
    Some(match (first, clip.out) {
        (0, None) => json!(name),
        (first, None) => json!({"video": name, "in": first}),
        (0, Some(out)) => json!({"video": name, "out": out.max(1)}),
        (first, Some(out)) => json!({"video": name, "in": first, "out": out.max(first + 1)}),
    })
}

/// `clips` and what each reads, at `fps`: `files` answers a name's files, or why it has none.
pub fn timeline_now(clips: Vec<Clip>, fps: f64, files: impl Fn(&str) -> Result<folder::Files, String>, lock: &Lock) -> TimelineNow {
    let now = |clip: Clip| {
        let name = mentions_in(&json!(clip.clip)).into_iter().next().unwrap_or_else(|| clip.clip.clone());
        let file = files(&name).and_then(|files| folder::main_of(&files).and_then(|sha256| lock.files.get(sha256)).cloned().ok_or_else(|| "no version yet".to_owned()));
        let mut row = ClipNow { clip, name, field: None, path: None, holds: None, width: None, height: None, missing: None };
        match file {
            Err(why) => row.missing = Some(why),
            Ok(file) => {
                let shape = (file.fps > 0.0).then_some(Shape { width: file.width, height: file.height, fps: file.fps, frames: file.frames });
                row.holds = shape.map(|shape| available(&shape, fps));
                row.width = (file.width > 0).then_some(file.width);
                row.height = (file.height > 0).then_some(file.height);
                row.field = Some(file.field().to_owned());
                row.missing = (!file.here).then(|| "not on this computer".to_owned());
                row.path = Some(file.path);
                row.clip.sha256 = Some(file.sha256);
            }
        }
        row
    };
    TimelineNow { fps, tracks: vec![TrackNow { clips: clips.into_iter().map(now).collect() }] }
}

/// A cut's pieces, and what its clips name with nothing made yet, skipped. A whole take is what its function made.
pub fn pieces(fields: &Map<String, Value>, reads: &[ReadNow], folder: &Folder, lock: &Lock) -> (Vec<Piece>, Vec<String>) {
    let fps = Canvas::of(fields, &mut vec![]).fps;
    let (mut pieces, mut skipped) = (vec![], vec![]);
    for clip in clips(fields, reads) {
        let mention = mentions_in(&json!(clip.clip)).into_iter().next().unwrap_or_default();
        let read = reads.iter().find(|read| name_key(&read.name) == name_key(&mention));
        let Some((read, file)) = read.and_then(|read| Some((read, lock.files.get(folder::main_of(&read.files)?)?))) else {
            if !skipped.contains(&mention) {
                skipped.push(mention);
            }
            continue;
        };
        let still = file.field() == "image";
        let shape = (file.fps > 0.0).then_some(Shape { width: file.width, height: file.height, fps: file.fps, frames: file.frames });
        pieces.push(Piece {
            asset: read.name.clone(),
            path: folder.path(&file.path),
            digest: file.sha256.clone(),
            file: file.path.clone(),
            shape,
            still,
            start: if still { 0 } else { clip.first.unwrap_or(0) },
            end: match still {
                true => clip.frames.unwrap_or((STILL_S * fps).round() as u64).max(1),
                false => clip.out.unwrap_or(u64::MAX),
            },
        });
    }
    (pieces, skipped)
}

/// Cuts `pieces` into a file beside `<project>/<name>`, answered with what it is, with cozy's ffmpeg, here, on `canvas` (its short
/// edge, or the first piece's) at its frame rate, and writes the same edit as OpenTimelineIO at the root
/// (`<name>.otio`, `otio`). Every in and out point is an exact frame. Every piece fills the canvas
/// and is center-cropped, never letterboxed or stretched; a still's transparency goes to black. By fast join when every piece is a video
/// already that size and rate: each piece's frames between keyframes are copied, only the GOPs at its ends
/// are encoded (kept in `cache`, cozy's cache folder, by take, frames and encoding, so a re-cut
/// encodes only what changed; `prune` keeps it small), and the sound is encoded once. Pieces that
/// differ in codec, size or frame rate are encoded whole, a take at another rate retimed to the
/// project's. Nothing is encoded above or below its sources: an edge piece at its own take's video
/// bitrate, a whole encode and the sound at the highest of the takes cut in (bitrate, sample rate,
/// channels). Sound comes with each clip, silence with one that has none. `told` hears how far it is, 0 to 1.
#[allow(clippy::too_many_arguments)]
fn cut(
    ffmpeg: &Path,
    ffprobe: &Path,
    folder: &Folder,
    pieces: &[Piece],
    canvas: &Canvas,
    name: &str,
    cache: &Path,
    told: &(dyn Fn(f64) + Send + Sync),
    fast: bool,
) -> Result<(Cutted, PathBuf), Error> {
    let probes = pieces.iter().map(|piece| probe(ffprobe, &piece.path, true)).collect::<Result<Vec<_>, _>>()?;
    let (fps, rate) = (canvas.fps, canvas.fps.to_string());
    let (width, height) = canvas.edit_size(probes[0].size);
    // Each piece's frames at the project's rate, within its take, as project.lock records it.
    let Some(made) = measure(pieces, fps) else {
        let unprobed: Vec<&str> = pieces.iter().filter(|piece| piece.shape.is_none()).map(|piece| piece.asset.as_str()).collect();
        return Err(failed(format!("no frame rate is recorded for {}'s file", unprobed.join(", "))));
    };
    let ranges: Vec<(u64, u64)> = pieces.iter().filter_map(|piece| piece.frames(fps)).collect();
    let native = |piece: &Piece| piece.shape.is_some_and(|shape| !retimed(&shape, fps));
    let io = |e: std::io::Error| failed(format!("{}: {e}", cache.display()));
    std::fs::create_dir_all(cache).map_err(io)?;
    let work = tempfile::tempdir_in(cache).map_err(io)?;
    let video = work.path().join("video.mp4");
    // A picture's bitrate says nothing of a video's.
    let highest = pieces.iter().zip(&probes).filter(|(piece, _)| !piece.still).filter_map(|(_, probe)| probe.video_bit_rate).max();
    let share = |frames: u64| 0.9 * frames as f64 / made.frames.max(1) as f64;
    let mut done = 0;
    let os = |args: &[&str]| args.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    let fast = fast
        && (pieces.iter().zip(&probes))
            .all(|(piece, p)| p.video == probes[0].video && p.video.starts_with("h264 ") && p.size == (width, height) && native(piece));
    if fast {
        let mut list = String::new();
        for (k, ((piece, probe), &(start, end))) in pieces.iter().zip(&probes).zip(&ranges).enumerate() {
            for (n, (from, to, copy)) in spans(start, end, probe.frames, &probe.keys).into_iter().enumerate() {
                let count = (to - from).to_string();
                // An edge piece is the same file for the same take, frames and encoding, whatever cuts it.
                let encoding = format!("libx264 veryfast {} {} {}", bit_rate(probe.video_bit_rate).join(" "), probe.pix_fmt, probe.rate_text);
                let encoding = &hex::encode(Sha256::digest(encoding.as_bytes()))[..8];
                let take = &piece.digest.trim_start_matches("sha256:")[..16];
                let part = match copy {
                    true => work.path().join(format!("{k}-{n}.ts")),
                    false => cache.join(format!("{take}-f{from}-f{to}-{encoding}.ts")),
                };
                if copy {
                    // From the keyframe at `from`: half a frame past it, so the seek lands on it.
                    let at = format!("{:.6}", (from as f64 + 0.5) / probe.rate);
                    let mut args = os(&["-ss", &at]);
                    args.extend(crate::media::options(&piece.path)?.into_iter().map(std::ffi::OsString::from));
                    args.push("-i".into());
                    args.push(piece.path.clone().into_os_string());
                    args.extend(os(&["-frames:v", &count, "-map", "0:v:0", "-c", "copy", "-f", "mpegts"]));
                    args.push(part.clone().into_os_string());
                    run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
                } else if !part.exists() {
                    // From the keyframe at or before `from`, its frames counted to `from`: no timestamp
                    // decides which frame comes first.
                    let staged = work.path().join(format!("{k}-{n}.ts"));
                    let key = probe.keys.iter().copied().filter(|&key| key <= from).max().unwrap_or(0);
                    let at = format!("{:.6}", (key as f64 + 0.5) / probe.rate);
                    let mut args = os(&["-noaccurate_seek", "-ss", &at]);
                    args.extend(crate::media::options(&piece.path)?.into_iter().map(std::ffi::OsString::from));
                    args.push("-i".into());
                    args.push(piece.path.clone().into_os_string());
                    let trim = format!("trim=start_frame={},setpts=PTS-STARTPTS", from - key);
                    args.extend(os(&["-map", "0:v:0", "-vf", &trim, "-frames:v", &count, "-c:v", "libx264", "-preset", "veryfast"]));
                    args.extend(bit_rate(probe.video_bit_rate).iter().map(Into::into));
                    args.extend(os(&["-pix_fmt", &probe.pix_fmt, "-r", &probe.rate_text, "-f", "mpegts"]));
                    args.push(staged.clone().into_os_string());
                    run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
                    std::fs::rename(&staged, &part).map_err(io)?;
                } else {
                    // Used now: its age starts again.
                    let _ = std::fs::File::options().write(true).open(&part).and_then(|file| file.set_modified(std::time::SystemTime::now()));
                }
                let _ = writeln!(list, "file '{}'", part.display().to_string().replace('\'', r"'\''"));
            }
            done += end - start;
            told(share(done));
        }
        let listed = work.path().join("pieces.txt");
        std::fs::write(&listed, list).map_err(io)?;
        run(
            ffmpeg,
            &[
                "-f".as_ref(),
                "concat".as_ref(),
                "-safe".as_ref(),
                "0".as_ref(),
                "-i".as_ref(),
                listed.as_os_str(),
                "-c".as_ref(),
                "copy".as_ref(),
                "-f".as_ref(),
                "mp4".as_ref(),
                video.as_os_str(),
            ],
        )?;
    } else {
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            if piece.still {
                let held = format!("{:.6}", (end + 1) as f64 / fps);
                inputs.extend(["-loop", "1", "-framerate", &rate, "-t", &held].map(Into::into));
            }
            inputs.extend(crate::media::options(&piece.path)?.into_iter().map(std::ffi::OsString::from));
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            let _ = write!(filter, "[{k}:v]{}[v{k}];", whole_filter(piece.still, native(piece), fps, (*start, *end), (width, height)));
            let _ = write!(joined, "[v{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=1:a=0[v]", pieces.len());
        let mut args: Vec<std::ffi::OsString> = inputs;
        let encode = ["-filter_complex", &filter, "-map", "[v]", "-c:v", "libx264", "-preset", "veryfast"];
        args.extend(encode.into_iter().chain(bit_rate(highest).iter().map(String::as_str)).map(Into::into));
        args.extend(["-pix_fmt", "yuv420p", "-r", &rate, "-f", "mp4"].map(Into::into));
        args.push(video.clone().into_os_string());
        run_counting(ffmpeg, &args, &|frame| told(share(frame)))?;
    }
    let mut mux: Vec<std::ffi::OsString> = vec!["-i".into(), video.into_os_string()];
    let sounds: Vec<&Sound> = probes.iter().filter_map(|probe| probe.sound.as_ref()).collect();
    if !sounds.is_empty() {
        // The sound once, over the whole edit, each piece's at its frames' times, however short it runs, and silence for one with none.
        let most = |of: fn(&Sound) -> Option<u64>| sounds.iter().filter_map(|sound| of(sound)).max();
        let silence = format!("anullsrc=r={}:cl={}", most(|s| s.sample_rate).unwrap_or(48_000), if most(|s| s.channels) == Some(1) { "mono" } else { "stereo" });
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, ((piece, probe), (start, end))) in pieces.iter().zip(&probes).zip(&ranges).enumerate() {
            let lasts = (*end - *start) as f64 / fps;
            match probe.sound {
                Some(_) => {
                    inputs.extend(crate::media::options(&piece.path)?.into_iter().map(std::ffi::OsString::from));
                    inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
                    let _ = write!(filter, "[{k}:a]{},apad=whole_dur={lasts}[a{k}];", sound_filter((*start, *end), fps));
                }
                None => {
                    inputs.extend(["-f", "lavfi", "-t", &format!("{lasts:.6}"), "-i", &silence].map(Into::into));
                    let _ = write!(filter, "[{k}:a]asetpts=PTS-STARTPTS[a{k}];");
                }
            }
            let _ = write!(joined, "[a{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=0:a=1[a]", pieces.len());
        let sound = work.path().join("sound.m4a");
        let mut args: Vec<std::ffi::OsString> = inputs;
        let mut encode = vec!["-filter_complex".to_owned(), filter, "-map".into(), "[a]".into(), "-c:a".into(), "aac".into()];
        for (flag, value) in [("-b:a", most(|s| s.bit_rate)), ("-ar", most(|s| s.sample_rate)), ("-ac", most(|s| s.channels))] {
            encode.extend(value.map(|value| [flag.to_owned(), value.to_string()]).into_iter().flatten());
        }
        args.extend(encode.into_iter().chain(["-f".into(), "mp4".into()]).map(Into::into));
        args.push(sound.clone().into_os_string());
        run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
        mux.extend(["-i".into(), sound.into_os_string(), "-map".into(), "1:a".into()]);
    }
    // Made beside the cut, then named it: a reader never meets half a file.
    let part = folder.dir.join(format!(".{name}.part.mp4"));
    mux.extend(["-map", "0:v", "-c", "copy", "-movflags", "+faststart", "-f", "mp4"].map(Into::into));
    mux.push(part.clone().into_os_string());
    run(ffmpeg, &mux.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    write_otio(folder, name, pieces, &probes, &ranges, fps)?;
    prune(cache);
    Ok((made, part))
}

fn write_otio(folder: &Folder, name: &str, pieces: &[Piece], probes: &[Probe], ranges: &[(u64, u64)], fps: f64) -> Result<(), Error> {
    let timeline = otio(name, pieces, probes, ranges, fps);
    let otio_path = folder.dir.join(Path::new(name).with_extension("otio"));
    let text = serde_json::to_string_pretty(&timeline).expect("a timeline serializes") + "\n";
    platform::durable::replace(&otio_path, text.as_bytes(), 0o644).map_err(|e| failed(format!("{}: {e}", otio_path.display())))
}

/// The timeline of a cut that stands, when its `.otio` is gone: the same pieces, so nothing is cut.
pub fn timeline(ffprobe: &Path, folder: &Folder, pieces: &[Piece], fps: f64, name: &str) -> Result<(), Error> {
    let probes = pieces.iter().map(|piece| probe(ffprobe, &piece.path, false)).collect::<Result<Vec<_>, _>>()?;
    let ranges: Vec<(u64, u64)> = pieces.iter().filter_map(|piece| piece.frames(fps)).collect();
    write_otio(folder, name, pieces, &probes, &ranges, fps)
}

/// The edit as OpenTimelineIO's own JSON (`Timeline.1`, as OpenTimelineIO 0.18 writes it), written
/// by hand: the project's rate, a video track and an audio track, one item per piece, each clip's
/// source range frame-exact at that rate and its reference that take's file (`files/<sha256>.<ext>`,
/// never whichever is approved, so the timeline keeps naming what it cut). A piece without sound is
/// a gap on the audio track. A take retimed to the project's rate says so in its clips' metadata.
fn otio(name: &str, pieces: &[Piece], probes: &[Probe], ranges: &[(u64, u64)], fps: f64) -> Value {
    let time = |value: u64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": fps, "value": value as f64});
    let range = |start: u64, frames: u64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": time(start), "duration": time(frames)});
    let item = |schema: &str, name: &str, metadata: Value, source: Value| {
        json!({
            "OTIO_SCHEMA": schema,
            "metadata": metadata,
            "name": name,
            "source_range": source,
            "effects": [],
            "markers": [],
            "enabled": true,
            "color": null,
        })
    };
    let track = |kind: &str, sound: bool| {
        let children = (pieces.iter().zip(probes).zip(ranges)).map(|((piece, probe), &(start, end))| {
            if sound && probe.sound.is_none() {
                return item("Gap.1", "", json!({}), range(0, end - start));
            }
            // A still can be held as long as the cut says.
            let (metadata, holds) = match piece.shape.filter(|_| !piece.still) {
                None => (json!({"cozy": {"still": true}}), end),
                Some(shape) if retimed(&shape, fps) => (json!({"cozy": {"retimed_from_fps": shape.fps}}), available(&shape, fps)),
                Some(shape) => (json!({}), available(&shape, fps)),
            };
            let mut clip = item("Clip.2", &piece.asset, metadata, range(start, end - start));
            clip["media_references"] = json!({"DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "metadata": {},
                "name": piece.asset,
                "available_range": range(0, holds),
                "available_image_bounds": null,
                "target_url": piece.file,
            }});
            clip["active_media_reference_key"] = json!("DEFAULT_MEDIA");
            clip
        });
        let mut track = item("Track.1", kind, json!({}), Value::Null);
        track["children"] = children.collect();
        track["kind"] = json!(kind);
        track
    };
    let mut tracks = item("Stack.1", "tracks", json!({}), Value::Null);
    tracks["children"] = json!([track("Video", false), track("Audio", true)]);
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "metadata": {"cozy": {"cut": name, "fps": fps}},
        "name": Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        "global_start_time": time(0),
        "tracks": tracks,
    })
}

/// How long an edge piece is kept unused, and how much the pieces may hold together.
const PIECES_KEPT_FOR: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 3600);
const PIECES_CAP: u64 = 10 << 30;

/// The edge pieces in `cache` kept while used: one unused for `PIECES_KEPT_FOR` goes, and past
/// `PIECES_CAP` the least recently used go first.
fn prune(cache: &Path) {
    let now = std::time::SystemTime::now();
    let mut pieces: Vec<(std::time::SystemTime, u64, PathBuf)> = (std::fs::read_dir(cache).into_iter().flatten().flatten())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ts"))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    pieces.sort();
    let mut held: u64 = pieces.iter().map(|(_, len, _)| len).sum();
    for (used, len, path) in pieces {
        let old = now.duration_since(used).is_ok_and(|age| age > PIECES_KEPT_FOR);
        if (old || held > PIECES_CAP) && std::fs::remove_file(&path).is_ok() {
            held -= len;
        }
    }
}

/// A piece's frames from `start` to `end` (exclusive) of a take of `frames`: copied between its first
/// and last keyframe inside them, the take's end counting as one, encoded before and after.
fn spans(start: u64, end: u64, frames: u64, keys: &[u64]) -> Vec<(u64, u64, bool)> {
    let ended = (end >= frames).then_some(end);
    let inside: Vec<u64> = keys.iter().copied().filter(|&key| key >= start && key <= end).chain(ended).collect();
    match (inside.first(), inside.last()) {
        (Some(&first), Some(&last)) if last > first => {
            let mut spans = vec![];
            if first > start {
                spans.push((start, first, false));
            }
            spans.push((first, last, true));
            if end > last {
                spans.push((last, end, false));
            }
            spans
        }
        _ => vec![(start, end, false)],
    }
}

/// What the edit needs of a file: its video (`<codec> <W>x<H> <pix_fmt> <rate>`), size, pixel
/// format, frame rate and frames, whether it has sound, and its keyframes' frames.
struct Probe {
    video: String,
    size: (u64, u64),
    pix_fmt: String,
    rate: f64,
    rate_text: String,
    frames: u64,
    /// Its video's bitrate (bit/s): its stream's, else its whole file's.
    video_bit_rate: Option<u64>,
    sound: Option<Sound>,
    keys: Vec<u64>,
}

/// A file's sound as ffprobe reads it.
struct Sound {
    bit_rate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
}

/// The encoder's bitrate flags for a source's `bit_rate`: the same average, capped at it over a
/// two-second buffer; without one, the encoder's own quality.
fn bit_rate(bit_rate: Option<u64>) -> Vec<String> {
    match bit_rate {
        Some(rate) => ["-b:v", &rate.to_string(), "-maxrate", &rate.to_string(), "-bufsize", &(2 * rate).to_string()]
            .map(str::to_owned)
            .into(),
        None => vec!["-crf".into(), "18".into()],
    }
}

/// What is measured of a file of `field`, when ffprobe reads it: a video's shape, a picture's size and its share fully transparent, a sound's length.
pub fn measured(ffmpeg: &Path, ffprobe: &Path, path: &Path, field: &str) -> Option<folder::File> {
    if field == "audio" {
        let duration_ms = (seconds_of(ffprobe, path)? * 1000.0).round() as u64;
        return (duration_ms > 0).then(|| folder::File { duration_ms, ..Default::default() });
    }
    let probe = probe(ffprobe, path, false).ok()?;
    let (width, height) = probe.size;
    Some(match field {
        "video" => folder::File { width, height, fps: probe.rate, frames: probe.frames, ..Default::default() },
        _ => folder::File { width, height, alpha: transparent_share(ffmpeg, path), ..Default::default() },
    })
}

/// How many frames the video at `path` holds, when ffprobe reads it.
pub fn frames_of(ffprobe: &Path, path: &Path) -> Option<u64> {
    probe(ffprobe, path, false).ok().map(|probe| probe.frames)
}

/// How long the video or sound at `path` lasts, as its container says.
pub fn seconds_of(ffprobe: &Path, path: &Path) -> Option<f64> {
    let args = ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"];
    let done = std::process::Command::new(ffprobe).args(crate::media::options(path).ok()?).args(args).arg(path).output().ok()?;
    String::from_utf8_lossy(&done.stdout).trim().parse().ok()
}

/// Whether the picture at `path` has an alpha channel, or a palette that may.
pub fn has_alpha(ffprobe: &Path, path: &Path) -> bool {
    let args = ["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=pix_fmt", "-of", "csv=p=0"];
    let Ok(done) = std::process::Command::new(ffprobe).args(match crate::media::options(path) { Ok(options) => options, Err(_) => return false }).args(args).arg(path).output() else { return false };
    let format = String::from_utf8_lossy(&done.stdout);
    ["rgba", "bgra", "argb", "abgr", "ya", "gbrap", "pal8"].iter().any(|alpha| format.contains(alpha))
}

/// What the edit needs of the video at `path`; its keyframes only with `keys`.
fn probe(ffprobe: &Path, path: &Path, keys: bool) -> Result<Probe, Error> {
    let read = |args: &[&str]| -> Result<Vec<u8>, Error> {
        let done = std::process::Command::new(ffprobe)
            .args(["-v", "error"])
            .args(crate::media::options(path)?)
            .args(args)
            .arg(path)
            .output()
            .map_err(|e| wont_start("ffprobe", &e))?;
        Ok(done.stdout)
    };
    let read_json: Value = serde_json::from_slice(&read(&[
        "-count_packets",
        "-show_entries",
        "stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate,nb_frames,nb_read_packets,bit_rate,sample_rate,channels:format=duration,bit_rate,start_time",
        "-of",
        "json",
    ])?)
    .unwrap_or_default();
    let streams = read_json["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").cloned().unwrap_or_default();
    let (Some(width), Some(height)) = (video["width"].as_u64(), video["height"].as_u64()) else {
        return Err(failed(format!("{} holds no video ffprobe reads", path.display())));
    };
    let rate_text = video["r_frame_rate"].as_str().unwrap_or("24/1").to_owned();
    let rate = match rate_text.split_once('/') {
        Some((n, d)) => n.parse::<f64>().unwrap_or(24.0) / d.parse::<f64>().unwrap_or(1.0).max(1.0),
        None => rate_text.parse().unwrap_or(24.0),
    };
    let pix_fmt = video["pix_fmt"].as_str().unwrap_or("yuv420p").to_owned();
    let codec = video["codec_name"].as_str().unwrap_or("");
    let seconds = read_json["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let start = read_json["format"]["start_time"].as_str().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
    let keys = match keys {
        true => read(&["-select_streams", "v:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])?,
        false => vec![],
    };
    let keys = (String::from_utf8_lossy(&keys).lines())
        .filter_map(|line| line.trim().trim_end_matches(',').parse::<f64>().ok())
        .map(|at| ((at - start) * rate).round().max(0.0) as u64)
        .collect();
    Ok(Probe {
        video: format!("{codec} {width}x{height} {pix_fmt} {rate_text}"),
        size: (width, height),
        pix_fmt,
        rate,
        rate_text,
        frames: (number(&video["nb_frames"]).or_else(|| number(&video["nb_read_packets"])))
            .unwrap_or_else(|| (seconds * rate).floor() as u64),
        video_bit_rate: number(&video["bit_rate"]).or_else(|| number(&read_json["format"]["bit_rate"])),
        sound: streams.iter().find(|s| s["codec_type"] == "audio").map(|audio| Sound {
            bit_rate: number(&audio["bit_rate"]),
            sample_rate: number(&audio["sample_rate"]),
            channels: number(&audio["channels"]),
        }),
        keys,
    })
}

/// A number ffprobe wrote, as a string or a number.
fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

/// One ffmpeg run, `frame` told each frame count it reports.
fn run_counting(ffmpeg: &Path, args: &[std::ffi::OsString], frame: &dyn Fn(u64)) -> Result<(), Error> {
    let started = std::process::Command::new(ffmpeg)
        .args(["-y", "-v", "error", "-nostats", "-progress", "pipe:1"])
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = started.map_err(|e| failed(format!("ffmpeg did not start: {e}")))?;
    let (stdout, stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    // What it says goes on two pipes, each read as it comes, so a full one stops nothing.
    let said = std::thread::spawn(move || std::io::read_to_string(stderr).unwrap_or_default());
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)).map_while(Result::ok) {
        if let Some(count) = line.strip_prefix("frame=").and_then(|count| count.trim().parse().ok()) {
            frame(count);
        }
    }
    let status = child.wait().map_err(|e| failed(format!("ffmpeg: {e}")))?;
    let said = said.join().unwrap_or_default();
    match status.success() {
        true => Ok(()),
        false => Err(failed(format!("ffmpeg could not cut the edit: {}", said.trim()))),
    }
}

/// One ffmpeg run, refused with what it said when it fails.
fn run(ffmpeg: &Path, args: &[&std::ffi::OsStr]) -> Result<(), Error> {
    let done = std::process::Command::new(ffmpeg)
        .args(["-y", "-v", "error"])
        .args(args)
        .output()
        .map_err(|e| wont_start("ffmpeg", &e))?;
    match done.status.success() {
        true => Ok(()),
        false => {
            Err(failed(format!("ffmpeg could not cut the edit: {}", String::from_utf8_lossy(&done.stderr).trim())))
        }
    }
}

fn failed(message: String) -> Error {
    Error::new(Kind::Unavailable, "edit_failed", message)
}

const TOOL_WONT_START: &str = "edit_tool_wont_start";

/// No clip's doing: every cut here would meet it.
fn wont_start(tool: &str, error: &std::io::Error) -> Error {
    Error::new(Kind::Unavailable, TOOL_WONT_START, format!("{tool} did not start: {error}")).next("cozy doctor")
}

/// Each file `value`, a request by its digests, names: its field, path and digest.
pub fn files_in(value: &Value, folder: &Folder, lock: &Lock) -> Vec<(String, PathBuf, String)> {
    fn walk(value: &Value, at: String, folder: &Folder, lock: &Lock, found: &mut Vec<(String, PathBuf, String)>) {
        let under = |key: String| if at.is_empty() { key } else { format!("{at}.{key}") };
        match value {
            Value::Object(fields) => fields.iter().for_each(|(key, value)| walk(value, under(key.clone()), folder, lock, found)),
            Value::Array(items) => items.iter().enumerate().for_each(|(n, value)| walk(value, under(n.to_string()), folder, lock, found)),
            Value::String(text) => {
                if let Some(file) = lock.files.get(text) {
                    found.push((at, folder.path(&file.path), text.clone()));
                }
            }
            _ => {}
        }
    }
    let mut found = vec![];
    walk(value, String::new(), folder, lock, &mut found);
    found
}

/// The share of `picture`'s pixels that are fully transparent; none if it can't be read.
fn transparent_share(ffmpeg: &Path, picture: &Path) -> f64 {
    let Ok(options) = crate::media::options(picture) else { return 0.0 };
    let read = std::process::Command::new(ffmpeg)
        .args(options)
        .args(["-v", "error", "-i"])
        .arg(picture)
        .args(["-frames:v", "1", "-vf", "format=rgba,alphaextract", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output();
    match read {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            out.stdout.iter().filter(|alpha| **alpha == 0).count() as f64 / out.stdout.len() as f64
        }
        _ => 0.0,
    }
}

/// The frame of `video` at `at` seconds (None: its last) as a PNG in `dir`. A take is what its function made, so one that ends
/// before that gives its last frame: the bool says so.
pub fn frame(ffmpeg: &Path, video: &Path, at: Option<f64>, dir: &Path) -> Result<(tempfile::NamedTempFile, bool), Error> {
    let io = |e: std::io::Error| Error::new(Kind::Unavailable, "project_io", format!("a frame of {}: {e}", video.display()));
    let frame = tempfile::Builder::new().suffix(".png").tempfile_in(dir).map_err(io)?;
    let options = crate::media::options(video)?;
    let grab = |seek: Vec<String>, once: bool| {
        let mut command = std::process::Command::new(ffmpeg);
        command.args(["-y", "-v", "error"]).args(seek).args(&options).arg("-i").arg(video);
        command.args(if once { &["-frames:v", "1", "-update", "1"][..] } else { &["-update", "1"][..] }).arg(frame.path());
        let done = command.output();
        let empty = std::fs::metadata(frame.path()).map_or(true, |meta| meta.len() == 0);
        (done, empty)
    };
    // Its last frame: the frames of its last second, each over the one before.
    let last = || grab(vec!["-sseof".into(), "-1".into()], false).0;
    let (done, past) = match at {
        None => (last(), false),
        Some(at) => match grab(vec!["-ss".into(), format!("{at}")], true) {
            (Ok(done), false) if done.status.success() => (Ok(done), false),
            _ => (last(), true),
        },
    };
    let done = done.map_err(io)?;
    if !done.status.success() || std::fs::metadata(frame.path()).map_or(0, |m| m.len()) == 0 {
        let said = format!("{} has no frame ({})", video.display(), String::from_utf8_lossy(&done.stderr).trim());
        return Err(Error::new(Kind::Validation, "frame_missing", said));
    }
    Ok((frame, past))
}

/// `from`, a file of the project's, as `to` at its root: a hard link, else a copy; a reader never meets half a file.
pub fn publish(from: &Path, to: &Path) -> std::io::Result<()> {
    let part = to.with_file_name(format!(".{}.part", to.file_name().expect("a name").to_string_lossy()));
    let _ = std::fs::remove_file(&part);
    // A real file of its own, whatever a person does to it; `copy` clones where the filesystem can.
    std::fs::copy(from, &part)?;
    writable(&part)?;
    platform::durable::rename_over(&part, to)
}

#[cfg(unix)]
fn writable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn writable(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)
}

/// Whether the film at `path` still shares its bytes with `files/`, as one a cozy before this published does: writing to it would write there.
#[cfg(unix)]
pub fn shares_bytes(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.nlink() > 1)
}

#[cfg(not(unix))]
pub fn shares_bytes(_: &Path) -> bool {
    false
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    #[ignore = "requires ffmpeg and ffprobe on PATH"]
    fn probes_and_cuts_read_single_media_files() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = Path::new("ffmpeg");
        let ffprobe = Path::new("ffprobe");
        let source = dir.path().join("marker.mp4");
        let made = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=32x32:r=4", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=8000", "-t", "1", "-c:v", "libx264", "-threads", "1", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&source).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        let shape = probe(ffprobe, &source, true).unwrap();
        assert_eq!(shape.size, (32, 32));
        assert_eq!(shape.frames, 4);
        let folder = Folder::at(dir.path());
        let canvas = Canvas { ratio: None, short: None, fps: 4.0 };
        let mut piece = Piece {
            asset: "marker".into(), path: source.clone(),
            digest: format!("sha256:{}", hex::encode(Sha256::digest(std::fs::read(&source).unwrap()))),
            file: "marker.mp4".into(), shape: Some(Shape { width: 32, height: 32, fps: 4.0, frames: 4 }),
            still: false, start: 0, end: 4,
        };
        for fast in [true, false] {
            let (_, film) = cut(ffmpeg, ffprobe, &folder, std::slice::from_ref(&piece), &canvas, "film", &dir.path().join("cache"), &|_| {}, fast).unwrap();
            assert_eq!(probe(ffprobe, &film, false).unwrap().frames, 4);
        }
        let picture = dir.path().join("marker.png");
        let made = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-i"]).arg(&source)
            .args(["-frames:v", "1", "-threads", "1"]).arg(&picture).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        piece.path = picture;
        piece.still = true;
        piece.shape = None;
        let (_, film) = cut(ffmpeg, ffprobe, &folder, std::slice::from_ref(&piece), &canvas, "still", &dir.path().join("cache"), &|_| {}, false).unwrap();
        assert_eq!(probe(ffprobe, &film, false).unwrap().frames, 4);
        assert!(frame(ffmpeg, &source, Some(0.0), dir.path()).is_ok());
        let disguised = dir.path().join("download.mp4.part");
        std::fs::write(&disguised, "ffconcat version 1.0\nfile marker.mp4\n").unwrap();
        assert_eq!(probe(ffprobe, &disguised, true).err().unwrap().code, "media_format_unsupported");
        piece.path = disguised.clone();
        let error = cut(ffmpeg, ffprobe, &folder, &[piece], &canvas, "refused", &dir.path().join("cache"), &|_| {}, true).err().unwrap();
        assert_eq!(error.code, "media_format_unsupported");
        assert!(!dir.path().join(".refused.part.mp4").exists());
        assert_eq!(frame(ffmpeg, &disguised, None, dir.path()).unwrap_err().code, "media_format_unsupported");
        assert!(seconds_of(ffprobe, &disguised).is_none());
    }
}
