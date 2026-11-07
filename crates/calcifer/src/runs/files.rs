//! `PUT`/`HEAD /v1/files/sha256:<hex>`: the one way a client's files reach Calcifer, by content, for a run to name (`sha256:<hex>` at
//! an input, or under a path in its `paths`), a project to import, a model to upload or a machine to update, as a machine takes
//! objects: byte ranges in any order, several side by side, resuming from the ranges held. Bytes wait in `parts/` and are named only
//! once every one is held and they hash to the name. Calcifer reads no client's disk. Held under Calcifer's home; a file or part
//! unused for a day goes the next time an upload starts, and a `HEAD` that finds one held counts as a use.
use std::{
    collections::HashMap,
    fs::{self, File as Disk},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path as UrlPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use futures_util::StreamExt as _;
use platform::durable::{self, FileAt as _};
use protocol::{
    calcifer::File,
    error::{Error, Kind},
    ids::Digest,
    machine::{HELD, spell_held},
};
use sha2::{Digest as _, Sha256};

use crate::{
    Calcifer,
    api::{Api, Refusal},
};

/// A file or part unused this long goes the next time an upload starts.
const UNUSED: Duration = Duration::from_secs(24 * 60 * 60);

/// Byte ranges `[start, end)`, sorted and apart.
type Ranges = Vec<(u64, u64)>;

/// Uploads under way, by part file.
static UPLOADS: LazyLock<Mutex<HashMap<PathBuf, Weak<Upload>>>> = LazyLock::new(Mutex::default);

/// One file's upload, shared by the writes that carry its ranges.
struct Upload {
    digest: Digest,
    length: u64,
    /// Where it goes once whole.
    held_at: PathBuf,
    part: PathBuf,
    file: Disk,
    held: Mutex<Ranges>,
    /// One settle at a time records what landed.
    settling: Mutex<()>,
    /// Named or dropped: later writes land nowhere, and a new upload starts afresh.
    ended: AtomicBool,
}

/// How an upload stands once what landed is recorded.
enum Settled {
    Partly(Ranges),
    /// Named now.
    Whole,
    /// Another write named it first.
    Held,
}

impl Upload {
    /// Lands `data` at `offset`.
    fn write_at(&self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        if self.ended.load(Ordering::Acquire) {
            return Ok(());
        }
        self.file.write_all_at(data, offset)?;
        insert(&mut self.held.lock().expect("held lock"), (offset, offset + data.len() as u64));
        Ok(())
    }

    fn held(&self) -> Ranges {
        self.held.lock().expect("held lock").clone()
    }

    /// Records what landed, synced; whole, its bytes are verified and it is named.
    fn settle(&self) -> Result<Settled, Refusal> {
        let _settling = self.settling.lock().expect("settling lock");
        if self.ended.load(Ordering::Acquire) {
            return Ok(Settled::Held);
        }
        let failed = |e: std::io::Error| unwritable(&self.part, e);
        let held = self.held();
        self.file.sync_data().map_err(failed)?;
        if held != whole(self.length) {
            write_held(&self.part, self.length, &held).map_err(failed)?;
            return Ok(Settled::Partly(held));
        }
        self.ended.store(true, Ordering::Release);
        UPLOADS.lock().expect("uploads lock").remove(&self.part);
        let got = hashed(&self.part).map_err(failed)?;
        let _ = fs::remove_file(held_path(&self.part));
        if got != self.digest {
            let _ = fs::remove_file(&self.part);
            return Err(mismatch(got, self.digest));
        }
        fs::rename(&self.part, &self.held_at).map_err(failed)?;
        let _ = durable::sync_dir(self.held_at.parent().expect("a file's folder"));
        Ok(Settled::Whole)
    }
}

/// Adds `[start, end)` to `ranges`, merging what it touches.
fn insert(ranges: &mut Ranges, (mut start, mut end): (u64, u64)) {
    if start >= end {
        return;
    }
    ranges.retain(|&(s, e)| {
        let apart = e < start || s > end;
        if !apart {
            (start, end) = (start.min(s), end.max(e));
        }
        apart
    });
    let at = ranges.partition_point(|&(s, _)| s < start);
    ranges.insert(at, (start, end));
}

fn whole(length: u64) -> Ranges {
    match length {
        0 => vec![],
        length => vec![(0, length)],
    }
}

/// The record of what a part holds, its file's length and the ranges synced.
fn held_path(part: &Path) -> PathBuf {
    part.with_extension("held")
}

fn read_held(part: &Path) -> Option<(u64, Ranges)> {
    serde_json::from_slice(&fs::read(held_path(part)).ok()?).ok()
}

fn write_held(part: &Path, length: u64, held: &Ranges) -> std::io::Result<()> {
    let path = held_path(part);
    let next = path.with_extension("held.next");
    fs::write(&next, serde_json::to_vec(&(length, held))?)?;
    fs::rename(next, path)
}

fn hashed(path: &Path) -> std::io::Result<Digest> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut Disk::open(path)?, &mut hasher)?;
    Ok(Digest::from_bytes(hasher.finalize().into()))
}

fn mismatch(got: Digest, digest: Digest) -> Refusal {
    Refusal(Error::new(Kind::Validation, "file_digest_mismatch", format!("the bytes sent hash to {got}, not {digest}")))
}

fn unwritable(path: &Path, e: std::io::Error) -> Refusal {
    Refusal(Error::new(Kind::Internal, "file_unwritable", format!("{}: {e}", path.display())))
}

/// Where uploaded files are held, by their digest's hex.
pub(crate) fn dir(calcifer: &Calcifer) -> PathBuf {
    calcifer.home().join("files")
}

/// The file Calcifer holds under `digest`, if whole.
pub(crate) fn held(calcifer: &Calcifer, digest: &Digest) -> Option<PathBuf> {
    Some(dir(calcifer).join(hex::encode(digest.bytes()))).filter(|path| path.is_file())
}

/// `name`'s upload as `length` bytes: going, or begun from what its record says landed.
fn open(calcifer: &Calcifer, digest: Digest, length: u64) -> Result<Arc<Upload>, Refusal> {
    let dir = dir(calcifer);
    let parts = dir.join("parts");
    let part = parts.join(format!("{}.part", hex::encode(digest.bytes())));
    let failed = |e: std::io::Error| unwritable(&part, e);
    let mut uploads = UPLOADS.lock().expect("uploads lock");
    uploads.retain(|_, upload| upload.strong_count() > 0);
    if let Some(upload) = uploads.get(&part).and_then(Weak::upgrade) {
        if upload.length != length {
            let said = format!("{digest} is being written as {} bytes, not {length}", upload.length);
            return Err(Refusal(Error::new(Kind::Conflict, "file_length_conflict", said)));
        }
        return Ok(upload);
    }
    fs::create_dir_all(&parts).map_err(failed)?;
    sweep(&dir);
    sweep(&parts);
    let file = Disk::options().read(true).write(true).create(true).truncate(false).open(&part).map_err(failed)?;
    let held = match read_held(&part) {
        Some((was, held)) if was == length => held,
        _ => {
            file.set_len(0).map_err(failed)?;
            vec![]
        }
    };
    file.set_len(length).map_err(failed)?;
    let upload = Arc::new(Upload {
        digest,
        length,
        held_at: dir.join(hex::encode(digest.bytes())),
        part: part.clone(),
        file,
        held: Mutex::new(held),
        settling: Mutex::default(),
        ended: AtomicBool::new(false),
    });
    uploads.insert(part, Arc::downgrade(&upload));
    Ok(upload)
}

/// A PUT's `[start, end)` of a file of `length` bytes: its `Content-Range`, else the whole file its `Content-Length` measures.
fn span(headers: &HeaderMap) -> Result<(u64, u64, u64), Refusal> {
    let text = |name| headers.get(name).and_then(|value: &HeaderValue| value.to_str().ok());
    let invalid = || Refusal(Error::new(Kind::Usage, "invalid_range", "a part is `Content-Range: bytes a-b/length`"));
    let Some(range) = text(header::CONTENT_RANGE) else {
        let length = text(header::CONTENT_LENGTH).and_then(|length| length.parse().ok());
        let length = length.ok_or_else(|| Refusal(Error::new(Kind::Usage, "file_length_required", "a whole file's PUT carries its Content-Length").remedy("or send it as parts, with `Content-Range: bytes a-b/length`")))?;
        return Ok((0, length, length));
    };
    let (span, length) = range.trim().strip_prefix("bytes ").and_then(|range| range.split_once('/')).ok_or_else(invalid)?;
    let (first, last) = span.split_once('-').ok_or_else(invalid)?;
    let number = |n: &str| n.trim().parse::<u64>().map_err(|_| invalid());
    let (start, end, length) = (number(first)?, number(last)?.saturating_add(1), number(length)?);
    match start < end && end <= length {
        true => Ok((start, end, length)),
        false => Err(invalid()),
    }
}

fn held_header(ranges: &[(u64, u64)]) -> (axum::http::HeaderName, HeaderValue) {
    (axum::http::HeaderName::from_static(HELD), HeaderValue::from_str(&spell_held(ranges)).expect("ranges are ASCII"))
}

/// The file `name`: a whole body or one part (`Content-Range`). A part leaves it unnamed: `204`, with what is held in `Cozy-Held`.
/// The write that completes it answers `201` once its bytes hash to its name, and one that finds it held `200`. What landed is synced
/// and recorded when the body ends, however it ends.
pub(crate) async fn put(State(api): State<Arc<Api>>, UrlPath(name): UrlPath<String>, headers: HeaderMap, body: Body) -> Result<Response, Refusal> {
    let digest = named(&name)?;
    let (start, end, length) = span(&headers)?;
    let answer = |status: StatusCode, path: &Path| -> Result<Response, Refusal> {
        let file = file(digest, path)?;
        Ok((status, [held_header(&whole(file.length))], Json(file)).into_response())
    };
    if let Some(path) = held(&api.calcifer, &digest) {
        touch(&path);
        return answer(StatusCode::OK, &path);
    }
    let upload = open(&api.calcifer, digest, length)?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Bytes>(8);
    let landing = upload.clone();
    let writer = tokio::task::spawn_blocking(move || -> Result<Settled, Refusal> {
        let (mut at, mut wrote) = (start, Ok(()));
        while let Some(data) = receiver.blocking_recv() {
            if at + data.len() as u64 > end {
                wrote = Err(Refusal(Error::new(Kind::Usage, "invalid_range", "the body is longer than its Content-Range")));
                break;
            }
            if let Err(e) = landing.write_at(at, &data) {
                wrote = Err(unwritable(&landing.part, e));
                break;
            }
            at += data.len() as u64;
        }
        drop(receiver);
        let settled = landing.settle()?;
        wrote.map(|()| settled)
    });
    let (mut stream, mut unread) = (body.into_data_stream(), None);
    while let Some(frame) = stream.next().await {
        match frame {
            // A refused writer drops its receiver; its refusal is the answer.
            Ok(data) => {
                if sender.send(data).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                unread = Some(Refusal(Error::new(Kind::Validation, "body_unreadable", e.to_string())));
                break;
            }
        }
    }
    drop(sender);
    let settled = writer.await.map_err(|e| Refusal(Error::new(Kind::Internal, "file_unwritable", e.to_string())))??;
    if let Some(refusal) = unread {
        return Err(refusal);
    }
    match settled {
        Settled::Partly(held) => Ok((StatusCode::NO_CONTENT, [held_header(&held)]).into_response()),
        Settled::Whole => answer(StatusCode::CREATED, &upload.held_at),
        Settled::Held => answer(StatusCode::OK, &upload.held_at),
    }
}

/// Whether Calcifer holds `name`: `200` with its length and media type, else `404`. `Cozy-Held` says which bytes either way.
pub(crate) async fn head(State(api): State<Arc<Api>>, UrlPath(name): UrlPath<String>) -> Result<Response, Refusal> {
    let digest = named(&name)?;
    let upload = {
        let parts = dir(&api.calcifer).join("parts");
        let part = parts.join(format!("{}.part", hex::encode(digest.bytes())));
        (UPLOADS.lock().expect("uploads lock").get(&part).and_then(Weak::upgrade), part)
    };
    let ranges = match upload {
        (Some(upload), _) if upload.held() == whole(upload.length) => {
            // All landed: this waits for it to be verified, and says how that went.
            let settling = upload.clone();
            match tokio::task::spawn_blocking(move || settling.settle()).await.map_err(|e| Refusal(Error::new(Kind::Internal, "file_unwritable", e.to_string())))? {
                Ok(Settled::Partly(ranges)) => ranges,
                _ => vec![],
            }
        }
        (Some(upload), _) => upload.held(),
        (None, part) => read_held(&part).map(|(_, ranges)| ranges).unwrap_or_default(),
    };
    let Some(path) = held(&api.calcifer, &digest) else {
        let refusal = Refusal(Error::new(Kind::NotFound, "file_not_found", format!("Calcifer holds no file {digest}")));
        return Ok((StatusCode::NOT_FOUND, [held_header(&ranges)], refusal).into_response());
    };
    touch(&path);
    let held = file(digest, &path)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(held.length));
    if let Ok(media_type) = HeaderValue::from_str(&held.media_type) {
        headers.insert(header::CONTENT_TYPE, media_type);
    }
    let (name, value) = held_header(&whole(held.length));
    headers.insert(name, value);
    Ok(headers.into_response())
}

fn named(name: &str) -> Result<Digest, Refusal> {
    name.parse().map_err(|why: &str| {
        Refusal(Error::new(Kind::Usage, "file_name_invalid", format!("{name:?}: {why}")).remedy("name a file sha256:<hex>"))
    })
}

fn file(digest: Digest, path: &Path) -> Result<File, Refusal> {
    let sniffed = super::bind::sniff(path).map_err(Refusal)?;
    Ok(File { digest, length: sniffed.length, media_type: sniffed.media_type })
}

/// Marks a held file named now.
pub(crate) fn touch(path: &Path) {
    let _ = fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now()));
}

fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let before = SystemTime::now() - UNUSED;
    for entry in entries.flatten() {
        if entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at < before) {
            let _ = fs::remove_file(entry.path());
        }
    }
}
