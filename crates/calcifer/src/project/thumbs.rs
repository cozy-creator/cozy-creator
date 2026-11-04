//! `GET|HEAD /v1/projects/{id}/thumbs/{sha256}?w=&strip=`: a small picture of a project's file, by Calcifer's own ffmpeg: a picture with
//! its alpha, a video's poster or a strip of its frames side by side, a sound's waveform. Named by hash, size and strip, so it never
//! changes, and kept under the cache while it is used. Each is made once however many ask, from a task of its own that a request going
//! away leaves to finish, at most a few at a time, so none of it holds up another request.
use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    process::Command,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime},
};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use protocol::error::{Error, Kind};
use serde::Deserialize;
use tokio::sync::Semaphore;

use super::{folder, host::Host, media_type, tools};
use crate::api::{Api, Refusal};

/// The ffmpegs a project's files may have going at once, for this and for measuring: a canvas of hundreds asks for all of them at once.
pub(super) static FFMPEGS: Semaphore = Semaphore::const_new(2);
/// Who is making each thumbnail, by its name.
static MAKING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Mutex::default);

/// A thumbnail unused this long goes the next time one is made.
const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A file ffmpeg can't read isn't tried again for this long: a page asks each time it shows one.
const FAILED_KEPT_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const WIDTH: u32 = 480;
const WIDEST: u32 = 2048;
const MOST_FRAMES: u32 = 32;
const WAVE_COLOR: &str = "#9aa4b2";

#[derive(Deserialize)]
pub(super) struct Asked {
    /// How wide it is, a strip's frames each: never wider than the file. Held to 16 to 2048.
    w: Option<u32>,
    /// How many frames of a video, side by side. Held to 1 to 32.
    strip: Option<u32>,
}

/// What to draw of one file.
struct Drawing {
    source: PathBuf,
    field: &'static str,
    /// How long it lasts, when that is measured.
    seconds: f64,
    width: u32,
    frames: u32,
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
    let frames = asked.strip.filter(|_| field == "video").map_or(0, |n| n.clamp(1, MOST_FRAMES));
    let drawing = Drawing {
        source: folder.path(&file.path),
        field,
        seconds: file.lasts_ms() as f64 / 1000.0,
        width: asked.w.unwrap_or(WIDTH).clamp(16, WIDEST),
        frames,
    };
    let name = format!("{hex}-{}-{frames}", drawing.width);
    let dir = api.machines.hubs.home().thumbs();
    let at = match kept(&dir, &name) {
        Some(at) => at,
        None => made(Host::new(&api), drawing, dir, name.clone()).await?,
    };
    crate::runs::files::touch(&at);
    let unreadable = |e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", at.display())));
    let opened = std::fs::File::open(&at).map_err(unreadable)?;
    let len = opened.metadata().map_err(unreadable)?.len();
    let mut response = crate::files::send(opened, len, media_type(&at), &format!("\"{name}\""), &headers).map_err(unreadable)?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(crate::files::IMMUTABLE));
    Ok(response)
}

/// The thumbnail called `name`, if it is there.
fn kept(dir: &FsPath, name: &str) -> Option<PathBuf> {
    ["png", "jpg"].iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|at| at.is_file())
}

/// The thumbnail called `name`, made now if no one has: the wait for it is all a request holds.
async fn made(host: Host, drawing: Drawing, dir: PathBuf, name: String) -> Result<PathBuf, Refusal> {
    let turn = MAKING.lock().expect("making lock").entry(name.clone()).or_default().clone();
    let making = tokio::spawn(async move {
        let answer = {
            let _turn = turn.lock().await;
            make(host, drawing, dir, name.clone()).await
        };
        MAKING.lock().expect("making lock").remove(&name);
        answer
    });
    let answer = making.await.map_err(|e| Refusal(Error::new(Kind::Internal, "thumb_failed", e.to_string())))?;
    answer.map_err(Refusal)
}

/// What the one making the thumbnail called `name` does: nothing if one before it did, or found ffmpeg can't.
async fn make(host: Host, drawing: Drawing, dir: PathBuf, name: String) -> Result<PathBuf, Error> {
    if let Some(at) = kept(&dir, &name) {
        return Ok(at);
    }
    let marked = dir.join(format!("{name}.failed"));
    if let Ok(why) = std::fs::read_to_string(&marked) {
        return Err(unreadable_file(why));
    }
    let (ffmpeg, ffprobe) = host.ffmpeg().await?;
    let permit = FFMPEGS.acquire().await.expect("ffmpeg's turns never close");
    let drawn = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        draw(&ffmpeg, &ffprobe, &drawing, &dir, &name)
    })
    .await
    .map_err(|e| Error::new(Kind::Internal, "thumb_failed", e.to_string()))?;
    drawn.map_err(|why| {
        let _ = std::fs::write(&marked, &why);
        unreadable_file(why)
    })
}

fn unreadable_file(why: String) -> Error {
    Error::new(Kind::Validation, "thumb_failed", format!("ffmpeg can't draw this file: {why}"))
}

/// Runs ffmpeg writing its one picture over `out`. Err: what it said.
fn run(ffmpeg: &FsPath, before: &[String], after: &[String], out: &FsPath) -> Result<(), String> {
    let done = Command::new(ffmpeg).args(["-y", "-v", "error", "-nostdin"]).args(before).args(after).args(["-frames:v", "1", "-update", "1"]).arg(out).output();
    let done = done.map_err(|e| e.to_string())?;
    let written = std::fs::metadata(out).is_ok_and(|meta| meta.len() > 0);
    match done.status.success() && written {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&done.stderr).lines().next().unwrap_or("it wrote nothing").to_owned()),
    }
}

fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// `source` as ffmpeg's input, which may read nothing else: what a machine made can be a playlist in a picture's name, and a canvas
/// draws each file unasked.
fn input(source: &str) -> Vec<String> {
    strs(&["-protocol_whitelist", "file", "-i", source])
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
        "video" => {
            let out = part("jpg")?;
            let drawn = match drawing.frames {
                0 => poster(ffmpeg, &source, drawing.seconds, &scale, out.path()),
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

/// A frame a little way in, as the first is often black; the first if that fails, as a length can mislead.
fn poster(ffmpeg: &FsPath, source: &str, seconds: f64, scale: &str, out: &FsPath) -> Result<(), String> {
    let after = strs(&["-vf", scale, "-q:v", "3"]);
    let at = (seconds * 0.1).min(3.0);
    let opening = input(source);
    if at > 0.0 && run(ffmpeg, &[strs(&["-ss", &format!("{at:.3}")]), opening.clone()].concat(), &after, out).is_ok() {
        return Ok(());
    }
    run(ffmpeg, &opening, &after, out)
}

/// `frames` evenly through the video, side by side.
fn strip(ffmpeg: &FsPath, ffprobe: &FsPath, drawing: &Drawing, source: &str, frames: u32, scale: &str, out: &FsPath) -> Result<(), String> {
    let seconds = match drawing.seconds > 0.0 {
        true => drawing.seconds,
        false => tools::seconds_of(ffprobe, &drawing.source).unwrap_or(0.0),
    };
    let at = |n: u32| seconds * (f64::from(n) + 0.5) / f64::from(frames);
    let inputs: Vec<String> = (0..frames).flat_map(|n| [strs(&["-threads", "1", "-ss", &format!("{:.3}", at(n))]), input(source)].concat()).collect();
    let scaled: String = (0..frames).map(|n| format!("[{n}:v:0]{scale},setsar=1[s{n}];")).collect();
    let joined: String = (0..frames).map(|n| format!("[s{n}]")).collect();
    let graph = format!("{scaled}{joined}hstack=inputs={frames}[out]");
    let graph = if frames == 1 { format!("[0:v:0]{scale},setsar=1[out]") } else { graph };
    run(ffmpeg, &inputs, &strs(&["-filter_complex", &graph, "-map", "[out]", "-q:v", "3"]), out)
}

/// What hasn't been used in a while goes, what ffmpeg couldn't read sooner, and any part a stop left behind.
fn prune(dir: &FsPath) {
    let now = SystemTime::now();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let kept_for = match path.extension().is_some_and(|ext| ext == "failed") || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".making")) {
            true => FAILED_KEPT_FOR,
            false => KEPT_FOR,
        };
        let old = entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|used| now.duration_since(used).is_ok_and(|age| age > kept_for));
        if old {
            let _ = std::fs::remove_file(path);
        }
    }
}
