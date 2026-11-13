//! `GET|HEAD /v1/projects/{id}/thumbs/{sha256}?w=&strip=&preview=&wave=`: a small picture of a project's file, by Calcifer's own ffmpeg: a
//! picture with its alpha, a video's poster, a strip of its frames side by side or its preview, a sound's waveform or a video's. Each is kept
//! in its project, in `.thumbs/` beside `files/` (an encrypted project's inside its encrypted store), named by hash, size and strip, so it
//! never changes. What the canvas draws of a file (a picture, a video's poster and its preview) is made as the file lands (cl-982; Paul,
//! 11-13: "all thumbnails should be generated ahead of time, and stored in the project"). A request only reads: one not made yet is drawn
//! next, in the background, and answered `202 thumb_pending` at once, and the `thumb` event tells the project's pages when it's made. So no
//! request, and none of a browser's six connections to Calcifer, waits on ffmpeg.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
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

/// The ffmpegs a project's files may have going at once, for this and for measuring.
pub(super) static FFMPEGS: Semaphore = Semaphore::const_new(2);
/// What's to be drawn: what a page waits on first, then what landed.
static QUEUE: LazyLock<(Mutex<Queue>, Notify)> = LazyLock::new(Default::default);

/// Where a project keeps its thumbnails, beside `files/`.
pub(super) const STORE: &str = ".thumbs";
/// What the canvas draws of a picture or a video at rest, and what every list of them does: a box (240 CSS px) at 100% on a screen of
/// twice the density. Drawn wider, a picture is its own file.
const PICTURE: u32 = 512;
const WIDEST: u32 = 2048;
const MOST_FRAMES: u32 = 32;
/// How many are drawn at once, each from a task of its own.
const DRAWERS: usize = 2;
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
    /// How wide it is, a strip's frames each: never wider than the file. Held to 16 to 2048; none, what the canvas draws.
    w: Option<u32>,
    /// How many frames of a video, side by side. Held to 1 to 32.
    strip: Option<u32>,
    /// A video's preview.
    #[serde(default)]
    preview: bool,
    /// A video's sound as a waveform: none if it has no sound.
    #[serde(default)]
    wave: bool,
}

/// What to draw of one file.
#[derive(Clone)]
struct Drawing {
    /// Its project, as registered: an encrypted one's root, never the view it's read through now, which a lock and an unlock replace.
    root: PathBuf,
    /// The file, under `root`.
    source: PathBuf,
    sha256: String,
    private: bool,
    field: &'static str,
    /// How long it lasts, when that is measured.
    seconds: f64,
    width: u32,
    frames: u32,
    preview: bool,
    wave: bool,
}

impl Drawing {
    fn name(&self) -> String {
        let hex = folder::hex_of(&self.sha256).to_ascii_lowercase();
        match (self.preview, self.field, self.frames) {
            (true, ..) => format!("{hex}-preview"),
            _ if self.wave => format!("{hex}-{}-wave", self.width),
            (false, "video", 0) => format!("{hex}-{}-poster", self.width),
            _ => format!("{hex}-{}-{}", self.width, self.frames),
        }
    }

    /// Its store and name: what it's queued under.
    fn key(&self) -> String {
        format!("{}\0{}", self.root.display(), self.name())
    }
}

#[derive(Default)]
struct Queue {
    /// Each in its turn; one asked again by a page goes first again, and its later place is passed over.
    jobs: VecDeque<Drawing>,
    /// What's queued, by key: whether it's being drawn.
    held: HashMap<String, bool>,
    /// What ffmpeg couldn't draw, for as long as this Calcifer runs: asked again, it's refused, not drawn again.
    failed: HashSet<String>,
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
    let (field, seconds, source) = {
        let lock = folder.lock().map_err(Refusal)?;
        let file = lock.files.get(&format!("sha256:{hex}")).filter(|file| folder.path(&file.path).is_file());
        let Some(file) = file else {
            return Err(Refusal(Error::new(Kind::NotFound, "file_missing", format!("the project has no file {named}"))));
        };
        (file.field(), file.lasts_ms() as f64 / 1000.0, folder.path(&file.path))
    };
    let preview = asked.preview && field == "video";
    let wave = asked.wave && field == "video" && !preview;
    let frames = asked.strip.filter(|_| field == "video" && !preview && !wave).map_or(0, |n| n.clamp(1, MOST_FRAMES));
    let private = super::vault::encrypted(&project.root);
    let source = super::vault::logical(&source);
    let width = asked.w.unwrap_or(PICTURE).clamp(16, WIDEST);
    let drawing = Drawing { root: project.root.clone(), source, sha256: format!("sha256:{hex}"), private, field, seconds, width, frames, preview, wave };
    let name = drawing.name();
    let Some(at) = kept(&folder.dir.join(STORE), &name) else {
        // Drawn next: `202 thumb_pending` now, which is no picture and no error, and the page hears `thumb` when it's there.
        let mut response = match ask(drawing, true) {
            true => {
                let mut pending = Refusal(Error::new(Kind::NotFound, "thumb_pending", "this thumbnail is being made; the thumb event says when it's there")).into_response();
                *pending.status_mut() = StatusCode::ACCEPTED;
                pending
            }
            false => Refusal(Error::new(Kind::Validation, "thumb_failed", "ffmpeg can't draw this file")).into_response(),
        };
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(response);
    };
    let unreadable = |e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", at.display())));
    let opened = std::fs::File::open(&at).map_err(unreadable)?;
    let len = opened.metadata().map_err(unreadable)?.len();
    let mut response = crate::files::send(opened, len, media_type(&at), &format!("\"{name}\""), &headers).map_err(unreadable)?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(if private { "private, no-store" } else { crate::files::IMMUTABLE }));
    Ok(response)
}

/// A file that just landed at `source` (`<project>/files/<hex>.<ext>`), measured as a `field`: what the canvas draws of it is made soon,
/// after what pages wait on.
pub(super) fn landed(source: PathBuf, sha256: &str, field: &str, seconds: f64) {
    let source = super::vault::logical(&source);
    let Some(root) = source.parent().and_then(FsPath::parent).map(FsPath::to_owned) else { return };
    let field = match field {
        "image" => "image",
        "video" => "video",
        _ => return,
    };
    let private = super::vault::encrypted(&root);
    let sha256 = format!("sha256:{}", folder::hex_of(sha256).to_ascii_lowercase());
    let picture = Drawing { root, source, sha256, private, field, seconds, width: PICTURE, frames: 0, preview: false, wave: false };
    let preview = (field == "video").then(|| Drawing { width: 0, preview: true, ..picture.clone() });
    ask(picture, false);
    if let Some(preview) = preview {
        ask(preview, false);
    }
}

/// `drawing` queued, `first` when a page waits on it. False: ffmpeg couldn't draw it.
fn ask(drawing: Drawing, first: bool) -> bool {
    let key = drawing.key();
    let mut queue = QUEUE.0.lock().expect("thumbnail queue");
    if queue.failed.contains(&key) {
        return false;
    }
    match queue.held.get(&key) {
        Some(true) => return true,
        Some(false) if !first => return true,
        _ => {}
    }
    queue.held.insert(key, false);
    match first {
        true => queue.jobs.push_front(drawing),
        false => queue.jobs.push_back(drawing),
    }
    drop(queue);
    QUEUE.1.notify_one();
    true
}

/// The next to draw, marked as being drawn.
fn next() -> Option<Drawing> {
    let mut queue = QUEUE.0.lock().expect("thumbnail queue");
    while let Some(drawing) = queue.jobs.pop_front() {
        let key = drawing.key();
        if queue.held.get(&key) == Some(&false) {
            queue.held.insert(key, true);
            return Some(drawing);
        }
    }
    None
}

/// Draws what's queued, `DRAWERS` at a time, and tells each project's pages of each it drew, or couldn't. What Calcifer kept before
/// thumbnails were kept in their projects goes.
pub(crate) fn previews(api: Arc<Api>) {
    let old = api.machines.hubs.home().root().join("cache").join("thumbs");
    tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(old);
    });
    for _ in 0..DRAWERS {
        let api = api.clone();
        tokio::spawn(async move {
            loop {
                let Some(drawing) = next() else {
                    QUEUE.1.notified().await;
                    continue;
                };
                let (key, root, sha256) = (drawing.key(), drawing.root.clone(), drawing.sha256.clone());
                let made = make(Host::new(&api), drawing).await;
                {
                    let mut queue = QUEUE.0.lock().expect("thumbnail queue");
                    queue.held.remove(&key);
                    // What a lock or an unlock cut short is drawn when it's asked again; what else failed isn't tried again.
                    if made.as_ref().is_err_and(|error| !matches!(error.kind, Kind::Conflict)) {
                        queue.failed.insert(key);
                    }
                }
                crate::events::thumb(&super::project_id(&root), &sha256);
            }
        });
    }
}

/// The thumbnail called `name` in `dir`, if it is there.
fn kept(dir: &FsPath, name: &str) -> Option<PathBuf> {
    ["png", "jpg", "mp4"].iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|at| at.is_file())
}

/// `drawing` drawn into its project's store, unless it's there already.
async fn make(host: Host, mut drawing: Drawing) -> Result<PathBuf, Error> {
    let access = match drawing.private {
        true => Some(super::vault::admit_root(&drawing.root).await?),
        false => None,
    };
    let dir = super::vault::resolve_path(&drawing.root)?.join(STORE);
    drawing.source = super::vault::resolve_path(&drawing.source)?;
    let name = drawing.name();
    if let Some(at) = kept(&dir, &name) {
        return Ok(at);
    }
    // Its turn first: finding ffmpeg runs it, and blocks, so that too is a few at a time.
    let permit = FFMPEGS.acquire().await.expect("ffmpeg's turns never close");
    let (ffmpeg, ffprobe) = host.ffmpeg().await?;
    tokio::task::spawn_blocking(move || {
        let _access = access;
        let _permit = permit;
        draw(&ffmpeg, &ffprobe, &drawing, &dir, &name).map_err(|why| {
            // The source path can disappear while waiting for FFmpeg. Only a successful draw is kept.
            if !drawing.private { warn!("thumbnail {name} can't be drawn: {why}"); }
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
    let source = drawing.source.to_string_lossy().into_owned();
    let width = drawing.width;
    let scale = format!("scale='min(iw,{width})':-2");
    let part = |ext: &str| tempfile::Builder::new().prefix(".making").suffix(&format!(".{ext}")).tempfile_in(dir).map_err(|e| e.to_string());
    // A video's sound under its clip on a timeline: quiet sound drawn on a square-root scale, so it shows. A column a sample at least:
    // shorter sound, or none's flat line, is padded with silence, as showwavespic refuses fewer samples than its width.
    let picture = format!("[0:a:0]apad=whole_len={width},showwavespic=s={width}x{}:colors={WAVE_COLOR}{}", (width / 4).max(16), if drawing.wave { ":scale=sqrt" } else { "" });
    let wave = || -> Result<(tempfile::NamedTempFile, &str), String> {
        let out = part("png")?;
        run(ffmpeg, &input(&source), &strs(&["-filter_complex", &picture]), out.path())?;
        Ok((out, "png"))
    };
    let (out, ext) = match drawing.field {
        "audio" => wave()?,
        // No sound in it: a flat line, kept like any other, so it isn't asked of ffmpeg again.
        "video" if drawing.wave => match wave() {
            Ok(drawn) => drawn,
            Err(_) => {
                let out = part("png")?;
                run(ffmpeg, &strs(&["-f", "lavfi", "-t", "0.1", "-i", "anullsrc=r=8000:cl=mono"]), &strs(&["-filter_complex", &picture]), out.path())?;
                (out, "png")
            }
        },
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
            // Turned by its EXIF as a browser draws it, here, and by nothing of FFmpeg's own, whichever version it is.
            let upright = match tools::orientation(&drawing.source) {
                2 => "hflip,",
                3 => "hflip,vflip,",
                4 => "vflip,",
                5 => "transpose=cclock_flip,",
                6 => "transpose=clock,",
                7 => "transpose=clock_flip,",
                8 => "transpose=cclock,",
                _ => "",
            };
            run(ffmpeg, &[strs(&["-noautorotate"]), input(&source)].concat(), &strs(&["-vf", &format!("{upright}{scale}"), "-q:v", "3"]), out.path())?;
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
