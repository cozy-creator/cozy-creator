//! What Calcifer runs itself, free, with no machine, the reserved package `cozy`: a `video`, a film at the project's root and
//! its timeline, and an `image`, one frame of a video. And what is measured of a file. Every in and out point is an exact frame.
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use protocol::error::{Error, Kind};
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

/// A piece of a cut: the asset it reads, its file (here, its digest, its path in the project,
/// `files/<sha256>.<ext>`, and its shape as project.lock records it), and its frames from `start`
/// to `end` (exclusive) at the cut's rate.
pub struct Piece {
    pub asset: String,
    pub path: PathBuf,
    pub digest: String,
    pub file: String,
    pub shape: Option<Shape>,
    pub start: u64,
    pub end: u64,
}

impl Piece {
    /// Its frames at `fps` within its take, `end` at most the take's last; None before its take is
    /// probed.
    fn frames(&self, fps: f64) -> Option<(u64, u64)> {
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

/// Why a cut can't take each of `pieces` that measuring left with no shape: asked of ffprobe now, so it says what is true.
pub fn unreadable(ffprobe: &Path, pieces: &[Piece]) -> Vec<String> {
    (pieces.iter().filter(|piece| piece.shape.is_none()))
        .map(|piece| match probe(ffprobe, &piece.path, false) {
            Ok(_) => format!("{}: {} is no video a cut takes: those are mp4, mov, m4v, webm and mkv", piece.asset, piece.path.display()),
            Err(error) => format!("{}: {}", piece.asset, error.message),
        })
        .collect()
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

/// A cut's pieces: each entry of its `pieces` (`<a>`, or a mapping holding a mention and `start` and `end` frames), or
/// with none, all it reads. And what it names with nothing made yet, skipped. A whole take is what its function made.
pub fn pieces(fields: &Map<String, Value>, reads: &[ReadNow], folder: &Folder, lock: &Lock) -> (Vec<Piece>, Vec<String>) {
    let entries: Vec<(String, Option<u64>, Option<u64>)> = match fields.get("pieces").and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(|item| {
                let mention = mentions_in(item).into_iter().next()?;
                Some((mention, item["start"].as_u64(), item["end"].as_u64()))
            })
            .collect(),
        None => reads.iter().map(|read| (read.name.clone(), None, None)).collect(),
    };
    let (mut pieces, mut skipped) = (vec![], vec![]);
    for (mention, start, end) in entries {
        let read = reads.iter().find(|read| name_key(&read.name) == name_key(&mention));
        let Some((read, file)) = read.and_then(|read| Some((read, lock.files.get(folder::main_of(&read.files)?)?))) else {
            if !skipped.contains(&mention) {
                skipped.push(mention);
            }
            continue;
        };
        let shape = (file.fps > 0.0).then_some(Shape { width: file.width, height: file.height, fps: file.fps, frames: file.frames });
        pieces.push(Piece {
            asset: read.name.clone(),
            path: folder.path(&file.path),
            digest: file.sha256.clone(),
            file: file.path.clone(),
            shape,
            start: start.unwrap_or(0),
            end: end.unwrap_or(u64::MAX),
        });
    }
    (pieces, skipped)
}

/// Cuts `pieces` into a file beside `<project>/<name>`, answered with what it is, with cozy's ffmpeg, here, on `canvas` (its short
/// edge, or the first piece's) at its frame rate, and writes the same edit as OpenTimelineIO at the root
/// (`<name>.otio`, `otio`). Every in and out point is an exact frame. Every piece fills the canvas
/// and is center-cropped, never letterboxed or stretched. By fast join when every piece is already
/// that size and rate: each piece's frames between keyframes are copied, only the GOPs at its ends
/// are encoded (kept in `cache`, cozy's cache folder, by take, frames and encoding, so a re-cut
/// encodes only what changed; `prune` keeps it small), and the sound is encoded once. Pieces that
/// differ in codec, size or frame rate are encoded whole, a take at another rate retimed to the
/// project's. Nothing is encoded above or below its sources: an edge piece at its own take's video
/// bitrate, a whole encode and the sound at the highest of the takes cut in (bitrate, sample rate,
/// channels).
pub fn cut(
    ffmpeg: &Path,
    ffprobe: &Path,
    folder: &Folder,
    pieces: &[Piece],
    canvas: &Canvas,
    name: &str,
    cache: &Path,
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
    let highest = probes.iter().filter_map(|probe| probe.video_bit_rate).max();
    let os = |args: &[&str]| args.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    let fast = (pieces.iter().zip(&probes))
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
                    let mut args = os(&["-ss", &at, "-i"]);
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
                    let mut args = os(&["-noaccurate_seek", "-ss", &at, "-i"]);
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
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            // A take at another rate is retimed first (each frame its source's nearest in time), then
            // cut by frame like any other.
            let retime = if native(piece) { String::new() } else { format!("fps={rate},") };
            let _ = write!(
                filter,
                "[{k}:v]{retime}trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS,\
                 scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1[v{k}];"
            );
            let _ = write!(joined, "[v{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=1:a=0[v]", pieces.len());
        let mut args: Vec<std::ffi::OsString> = inputs;
        let encode = ["-filter_complex", &filter, "-map", "[v]", "-c:v", "libx264", "-preset", "veryfast"];
        args.extend(encode.into_iter().chain(bit_rate(highest).iter().map(String::as_str)).map(Into::into));
        args.extend(["-pix_fmt", "yuv420p", "-r", &rate, "-f", "mp4"].map(Into::into));
        args.push(video.clone().into_os_string());
        run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    }
    let mut mux: Vec<std::ffi::OsString> = vec!["-i".into(), video.into_os_string()];
    let sounds: Vec<&Sound> = probes.iter().filter_map(|probe| probe.sound.as_ref()).collect();
    if sounds.len() == probes.len() {
        // The sound once, over the whole edit, each piece's at its frames' times.
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            let (start, end) = (*start as f64 / fps, *end as f64 / fps);
            let _ = write!(filter, "[{k}:a]atrim=start={start}:end={end},asetpts=PTS-STARTPTS[a{k}];");
            let _ = write!(joined, "[a{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=0:a=1[a]", pieces.len());
        let sound = work.path().join("sound.m4a");
        let mut args: Vec<std::ffi::OsString> = inputs;
        let most = |of: fn(&Sound) -> Option<u64>| sounds.iter().filter_map(|sound| of(sound)).max();
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
            let shape = piece.shape.expect("a piece cut is probed");
            let metadata = match retimed(&shape, fps) {
                true => json!({"cozy": {"retimed_from_fps": shape.fps}}),
                false => json!({}),
            };
            let mut clip = item("Clip.2", &piece.asset, metadata, range(start, end - start));
            clip["media_references"] = json!({"DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "metadata": {},
                "name": piece.asset,
                "available_range": range(0, available(&shape, fps)),
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

/// What is measured of a picture or video, when ffprobe reads it: a video's shape, a picture's size and its share fully transparent.
pub fn measured(ffmpeg: &Path, ffprobe: &Path, path: &Path, video: bool) -> Option<folder::File> {
    let probe = probe(ffprobe, path, false).ok()?;
    let (width, height) = probe.size;
    Some(match video {
        true => folder::File { width, height, fps: probe.rate, frames: probe.frames, ..Default::default() },
        false => folder::File { width, height, alpha: transparent_share(ffmpeg, path), ..Default::default() },
    })
}

/// How many frames the video at `path` holds, when ffprobe reads it.
pub fn frames_of(ffprobe: &Path, path: &Path) -> Option<u64> {
    probe(ffprobe, path, false).ok().map(|probe| probe.frames)
}

/// What the edit needs of the video at `path`; its keyframes only with `keys`.
fn probe(ffprobe: &Path, path: &Path, keys: bool) -> Result<Probe, Error> {
    let read = |args: &[&str]| -> Result<Vec<u8>, Error> {
        let done = std::process::Command::new(ffprobe)
            .args(["-v", "error"])
            .args(args)
            .arg(path)
            .output()
            .map_err(|e| failed(format!("ffprobe did not start: {e}")))?;
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

/// One ffmpeg run, refused with what it said when it fails.
fn run(ffmpeg: &Path, args: &[&std::ffi::OsStr]) -> Result<(), Error> {
    let done = std::process::Command::new(ffmpeg)
        .args(["-y", "-v", "error"])
        .args(args)
        .output()
        .map_err(|e| failed(format!("ffmpeg did not start: {e}")))?;
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
    let read = std::process::Command::new(ffmpeg)
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
    let grab = |seek: Vec<String>, once: bool| {
        let mut command = std::process::Command::new(ffmpeg);
        command.args(["-y", "-v", "error"]).args(seek).arg("-i").arg(video);
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
    let part = to.with_file_name(format!(".{}.link", to.file_name().expect("a name").to_string_lossy()));
    let _ = std::fs::remove_file(&part);
    if std::fs::hard_link(from, &part).is_err() {
        std::fs::copy(from, &part)?;
    }
    std::fs::rename(&part, to)
}
