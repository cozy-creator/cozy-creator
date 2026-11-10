//! `GET|HEAD /v1/projects/{id}/thumbs/{sha256}?w=&strip=&preview=`: a small picture of a project's file, by Calcifer's own ffmpeg: a
//! picture with its alpha, a video's poster, a strip of its frames side by side or its preview, a sound's waveform. Named by hash, size and
//! strip, so it never changes, and kept under the cache while it is used. Each is made once however many ask, from a task of its own that a
//! request going away leaves to finish, at most a few at a time, so none of it holds up another request. A video's preview is made as it
//! lands, too.
use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime},
};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use api::error::{Error, Kind};
use serde::Deserialize;
use tokio::sync::{Notify, Semaphore};
use tracing::warn;

use super::{folder, host::Host, media_type, tools};
use crate::{
    api::{Api, Refusal},
    tools::Media,
};

/// The ffmpegs a project's files may have going at once, for this and for measuring: a canvas of hundreds asks for all of them at once.
pub(super) static FFMPEGS: Semaphore = Semaphore::const_new(2);
/// Who is making each thumbnail, by its name.
static MAKING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Mutex::default);
/// The videos that landed whose previews are still to make, the first first.
static LANDED: LazyLock<(Mutex<VecDeque<(String, Drawing)>>, Notify)> = LazyLock::new(Default::default);

/// A thumbnail unused this long goes the next time one is made.
const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A partial thumbnail or a failure marker left by an older version is discarded sooner.
const TEMP_KEPT_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const WIDTH: u32 = 480;
const WIDEST: u32 = 2048;
const MOST_FRAMES: u32 = 32;
const WAVE_COLOR: &str = "#9aa4b2";
/// Where a poster may come from, through a video, as media-worker samples them.
const POSTER_AT: [f64; 5] = [0.2, 0.35, 0.5, 0.65, 0.8];
/// The spread of a frame's luma, 0 to 255, under which it is black or flat (media-worker's).
const FLAT: f64 = 12.0;
/// How long a preview lasts, and its longer side.
const PREVIEW_S: f64 = 3.0;
const PREVIEW_SIDE: u32 = 480;

#[derive(Deserialize)]
pub(super) struct Asked {
    /// How wide it is, a strip's frames each: never wider than the file. Held to 16 to 2048.
    w: Option<u32>,
    /// How many frames of a video, side by side. Held to 1 to 32.
    strip: Option<u32>,
    /// A video's preview.
    #[serde(default)]
    preview: bool,
}

/// What to draw of one file.
struct Drawing {
    source: PathBuf,
    field: &'static str,
    /// How long it lasts, when that is measured.
    seconds: f64,
    width: u32,
    frames: u32,
    preview: bool,
}

impl Drawing {
    /// A poster by its rule's name, so one cached by an earlier rule is made again.
    fn name(&self, hex: &str) -> String {
        match (self.preview, self.field, self.frames) {
            (true, ..) => format!("{hex}-preview"),
            (false, "video", 0) => format!("{hex}-{}-poster", self.width),
            _ => format!("{hex}-{}-{}", self.width, self.frames),
        }
    }
}

pub(super) async fn thumb(
    State(api): State<Arc<Api>>,
    Path((id, named)): Path<(String, String)>,
    Query(asked): Query<Asked>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let hex = folder::hex_of(&named).to_ascii_lowercase();
    let lock = folder.lock().map_err(Refusal)?;
    let file = lock.files.get(&format!("sha256:{hex}")).filter(|file| folder.path(&file.path).is_file());
    let Some(file) = file else {
        return Err(Refusal(Error::new(Kind::NotFound, "file_missing", format!("the project has no file {named}"))));
    };
    let field = file.field();
    let preview = asked.preview && field == "video";
    let frames = asked.strip.filter(|_| field == "video" && !preview).map_or(0, |n| n.clamp(1, MOST_FRAMES));
    let drawing = Drawing {
        source: folder.path(&file.path),
        field,
        seconds: file.lasts_ms() as f64 / 1000.0,
        width: asked.w.unwrap_or(WIDTH).clamp(16, WIDEST),
        frames,
        preview,
    };
    let name = drawing.name(&hex);
    let dir = api.machines.hubs.home().thumbs();
    let at = match kept(&dir, &name) {
        Some(at) => at,
        None => made(Host::new(&api), drawing, dir, name.clone()).await?,
    };
    // Shown now: a use.
    let _ = std::fs::File::options().write(true).open(&at).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    let unreadable = |e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", at.display())));
    let opened = std::fs::File::open(&at).map_err(unreadable)?;
    let len = opened.metadata().map_err(unreadable)?.len();
    let mut response = crate::files::send(opened, len, media_type(&at), &format!("\"{name}\""), &headers).map_err(unreadable)?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(crate::files::IMMUTABLE));
    Ok(response)
}

/// A video at `source` that just landed, measured: its preview is made soon, after the thumbnails asked before it.
pub(super) fn landed(source: PathBuf, sha256: &str, seconds: f64) {
    let drawing = Drawing { source, field: "video", seconds, width: 0, frames: 0, preview: true };
    LANDED.0.lock().expect("landed lock").push_back((drawing.name(&folder::hex_of(sha256).to_ascii_lowercase()), drawing));
    LANDED.1.notify_one();
}

/// Makes the preview of each video that lands, one at a time, so a canvas's thumbnails take their turns between them.
pub(crate) fn previews(api: Arc<Api>) {
    tokio::spawn(async move {
        loop {
            let next = LANDED.0.lock().expect("landed lock").pop_front();
            match next {
                Some((name, drawing)) => drop(made(Host::new(&api), drawing, api.machines.hubs.home().thumbs(), name).await),
                None => LANDED.1.notified().await,
            }
        }
    });
}

/// The thumbnail called `name`, if it is there.
fn kept(dir: &FsPath, name: &str) -> Option<PathBuf> {
    ["png", "jpg", "mp4"].iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|at| at.is_file())
}

/// The thumbnail called `name`, made now if no one has: the wait for it is all a request holds.
async fn made(host: Host, drawing: Drawing, dir: PathBuf, name: String) -> Result<PathBuf, Refusal> {
    let turn = MAKING.lock().expect("making lock").entry(name.clone()).or_default().clone();
    let making = tokio::spawn(async move {
        let answer = {
            let _turn = turn.lock().await;
            make(host, drawing, dir, name.clone()).await
        };
        // Keep the same turn for requests already waiting to draw this name.
        let mut making = MAKING.lock().expect("making lock");
        drop(turn);
        if making.get(&name).is_some_and(|turn| Arc::strong_count(turn) == 1) {
            making.remove(&name);
        }
        answer
    });
    let answer = making.await.map_err(|e| Refusal(Error::new(Kind::Internal, "thumb_failed", e.to_string())))?;
    answer.map_err(Refusal)
}

/// What the one making the thumbnail called `name` does, if one before it did not succeed.
async fn make(host: Host, drawing: Drawing, dir: PathBuf, name: String) -> Result<PathBuf, Error> {
    if let Some(at) = kept(&dir, &name) {
        return Ok(at);
    }
    // Its turn first: finding ffmpeg runs it, and blocks, so that too is a few at a time.
    let permit = FFMPEGS.acquire().await.expect("ffmpeg's turns never close");
    let (ffmpeg, ffprobe) = host.ffmpeg().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        draw(&ffmpeg, &ffprobe, &drawing, &dir, &name).map_err(|why| {
            // The source path can disappear while waiting for FFmpeg. Only a successful draw is cacheable.
            warn!("thumbnail {name} can't be drawn: {why}");
            Error::new(Kind::Validation, "thumb_failed", format!("ffmpeg can't draw this file: {why}"))
        })
    })
    .await
    .map_err(|e| Error::new(Kind::Internal, "thumb_failed", e.to_string()))?
}

/// Runs ffmpeg writing its one picture over `out`. Err: what it said.
fn run(ffmpeg: &FsPath, before: &[OsString], after: &[OsString], out: &FsPath) -> Result<(), String> {
    ran(ffmpeg, &[before, after, &strs(&["-frames:v", "1", "-update", "1"])].concat(), out)
}

/// Runs ffmpeg with `args` writing over `out`. Err: what it said.
fn ran(ffmpeg: &FsPath, args: &[OsString], out: &FsPath) -> Result<(), String> {
    let done = Media::new(ffmpeg).args(["-y", "-v", "error", "-nostdin"]).args(args).arg(out).output();
    let done = done.map_err(|e| e.to_string())?;
    let written = std::fs::metadata(out).is_ok_and(|meta| meta.len() > 0);
    match done.status.success() && written {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&done.stderr).lines().next().unwrap_or("it wrote nothing").to_owned()),
    }
}

fn strs(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// `source` as ffmpeg's input, which may read nothing else: what a machine made can be a playlist in a picture's name, and a canvas
/// draws each file unasked.
fn input(source: &str) -> Vec<OsString> {
    Media::reading(source)
}

/// Draws `drawing` as the thumbnail called `name`, whole or not there at all.
fn draw(ffmpeg: &FsPath, ffprobe: &FsPath, drawing: &Drawing, dir: &FsPath, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    prune(dir);
    let source = drawing.source.to_string_lossy().into_owned();
    let width = drawing.width;
    let scale = format!("scale='min(iw,{width})':-2");
    let part = |ext: &str| tempfile::Builder::new().prefix(".making").suffix(&format!(".{ext}")).tempfile_in(dir).map_err(|e| e.to_string());
    let wave = || -> Result<(tempfile::NamedTempFile, &str), String> {
        let out = part("png")?;
        let picture = format!("showwavespic=s={width}x{}:colors={WAVE_COLOR}", (width / 4).max(16));
        run(ffmpeg, &input(&source), &strs(&["-filter_complex", &picture]), out.path())?;
        Ok((out, "png"))
    };
    let (out, ext) = match drawing.field {
        "audio" => wave()?,
        "video" if drawing.preview => {
            let out = part("mp4")?;
            let seconds = seconds(ffprobe, drawing);
            preview(ffmpeg, &source, seconds, poster_at(ffmpeg, &source, seconds), out.path())?;
            (out, "mp4")
        }
        "video" => {
            let out = part("jpg")?;
            let drawn = match drawing.frames {
                0 => poster(ffmpeg, &source, poster_at(ffmpeg, &source, seconds(ffprobe, drawing)), &scale, out.path()),
                frames => strip(ffmpeg, ffprobe, drawing, &source, frames, &scale, out.path()),
            };
            match drawn {
                Ok(()) => (out, "jpg"),
                // No picture in it: a sound in a video's clothes.
                Err(why) => wave().map_err(|_| why)?,
            }
        }
        _ => {
            let ext = if tools::has_alpha(ffprobe, &drawing.source) { "png" } else { "jpg" };
            let out = part(ext)?;
            run(ffmpeg, &input(&source), &strs(&["-vf", &scale, "-q:v", "3"]), out.path())?;
            (out, ext)
        }
    };
    let at = dir.join(format!("{name}.{ext}"));
    out.persist(&at).map_err(|e| e.error.to_string())?;
    Ok(at)
}

/// How long a video lasts: as measured, else as ffprobe says now.
fn seconds(ffprobe: &FsPath, drawing: &Drawing) -> f64 {
    match drawing.seconds > 0.0 {
        true => drawing.seconds,
        false => tools::seconds_of(ffprobe, &drawing.source).unwrap_or(0.0),
    }
}

/// Where a video's poster is, as media-worker picks one: the first of `POSTER_AT` that isn't black or flat, else the busiest, as the first
/// frame is often black or a cut. Each is kept off the end, past the last frame's start.
fn poster_at(ffmpeg: &FsPath, source: &str, seconds: f64) -> f64 {
    let mut busiest = (-1.0, 0.0);
    for fraction in POSTER_AT.iter().filter(|_| seconds > 0.0) {
        let at = ((seconds * fraction).min(seconds - 0.25).max(0.0) * 1000.0).round() / 1000.0;
        let Some(spread) = spread(ffmpeg, source, at) else { continue };
        if spread >= FLAT {
            return at;
        }
        if spread > busiest.0 {
            busiest = (spread, at);
        }
    }
    busiest.1
}

/// How far the luma of the frame at `at` spreads, 0 to 255, measured on 64×36 of it: near 0 for black and flat frames.
fn spread(ffmpeg: &FsPath, source: &str, at: f64) -> Option<f64> {
    let before = strs(&["-v", "error", "-nostdin", "-threads", "1", "-ss", &format!("{at:.3}")]);
    let after = strs(&["-map", "0:v:0", "-frames:v", "1", "-vf", "scale=64:36,format=gray", "-f", "rawvideo", "-"]);
    let done = Media::new(ffmpeg).args(before).input(source).args(after).output().ok()?;
    let grey: Vec<f64> = done.stdout.iter().map(|&luma| f64::from(luma)).collect();
    if !done.status.success() || grey.is_empty() {
        return None;
    }
    let mean = grey.iter().sum::<f64>() / grey.len() as f64;
    Some((grey.iter().map(|luma| (luma - mean).powi(2)).sum::<f64>() / grey.len() as f64).sqrt())
}

/// The frame at `at`; the first if that fails, as a length can mislead.
fn poster(ffmpeg: &FsPath, source: &str, at: f64, scale: &str, out: &FsPath) -> Result<(), String> {
    let after = strs(&["-vf", scale, "-q:v", "3"]);
    let opening = input(source);
    if at > 0.0 && run(ffmpeg, &[strs(&["-ss", &format!("{at:.3}")]), opening.clone()].concat(), &after, out).is_ok() {
        return Ok(());
    }
    run(ffmpeg, &opening, &after, out)
}

/// `PREVIEW_S` of the video from its poster's frame on, or to its end: small, silent, and quick to start, for a page to loop while the pointer
/// is on it. Its first frame is the poster, so nothing jumps when it starts.
fn preview(ffmpeg: &FsPath, source: &str, seconds: f64, at: f64, out: &FsPath) -> Result<(), String> {
    let lasts = if seconds > at { (seconds - at).min(PREVIEW_S) } else { PREVIEW_S };
    let fit = format!("scale='min({PREVIEW_SIDE},iw)':'min({PREVIEW_SIDE},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,format=yuv420p");
    let before = strs(&["-threads", "1", "-ss", &format!("{at:.3}")]);
    let encode = strs(&["-t", &format!("{lasts:.3}"), "-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &fit, "-c:v", "libx264", "-preset", "veryfast", "-crf", "28"]);
    let container = strs(&["-threads", "1", "-movflags", "+faststart", "-f", "mp4"]);
    ran(ffmpeg, &[before, input(source), encode, container].concat(), out)
}

/// `frames` evenly through the video, side by side: a seek to each, which leaves a long take cheap. A seek past its last frame gives
/// nothing, and ffmpeg then writes nothing and says nothing: a clip of few frames, or a length that outruns the video. So those are
/// decoded once, whatever they hold, and a length that outran it leaves the rest of the row black.
fn strip(ffmpeg: &FsPath, ffprobe: &FsPath, drawing: &Drawing, source: &str, frames: u32, scale: &str, out: &FsPath) -> Result<(), String> {
    let seconds = seconds(ffprobe, drawing);
    let at = |n: u32| seconds * (f64::from(n) + 0.5) / f64::from(frames);
    let inputs: Vec<OsString> = (0..frames).flat_map(|n| [strs(&["-threads", "1", "-ss", &format!("{:.3}", at(n))]), input(source)].concat()).collect();
    let scaled: String = (0..frames).map(|n| format!("[{n}:v:0]{scale},setsar=1[s{n}];")).collect();
    let joined: String = (0..frames).map(|n| format!("[s{n}]")).collect();
    let graph = format!("{scaled}{joined}hstack=inputs={frames}[out]");
    let graph = if frames == 1 { format!("[0:v:0]{scale},setsar=1[out]") } else { graph };
    let after = |graph: &str| strs(&["-filter_complex", graph, "-map", "[out]", "-q:v", "3"]);
    run(ffmpeg, &inputs, &after(&graph), out).or_else(|_| {
        let rate = if seconds > 0.0 { f64::from(frames) / seconds } else { f64::from(frames) };
        run(ffmpeg, &input(source), &after(&format!("[0:v:0]fps={rate:.6},{scale},setsar=1,tile={frames}x1[out]")), out)
    })
}

/// Old thumbnails, partial draws and legacy failure markers are discarded.
fn prune(dir: &FsPath) {
    let now = SystemTime::now();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let kept_for = match path.extension().is_some_and(|ext| ext == "failed") || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".making")) {
            true => TEMP_KEPT_FOR,
            false => KEPT_FOR,
        };
        let old = entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|used| now.duration_since(used).is_ok_and(|age| age > kept_for));
        if old {
            let _ = std::fs::remove_file(path);
        }
    }
}
