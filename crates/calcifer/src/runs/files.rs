//! `PUT`/`HEAD /v1/files/sha256:<hex>`: files a client gives Calcifer for its runs to name by content
//! (`sha256:<hex>` at an input), as a machine takes objects. A client on this computer may name its
//! files by path instead. Held under Calcifer's home; a file no run named for a day goes the next
//! time one is put.
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    Json,
    body::Body,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use futures_util::StreamExt as _;
use protocol::{calcifer::File, error::{Error, Kind}, ids::Digest};
use sha2::{Digest as _, Sha256};

use crate::{
    Calcifer,
    api::{Api, Refusal},
};

/// A file unnamed this long goes the next time one is put.
const UNUSED: Duration = Duration::from_secs(24 * 60 * 60);

/// Where uploaded files are held, by their digest's hex.
pub(crate) fn dir(calcifer: &Calcifer) -> PathBuf {
    calcifer.home().join("files")
}

/// The file `name` (`sha256:<hex>`), whole: 201 when it is new, 200 when it was held. Its bytes
/// must hash to its name.
pub(crate) async fn put(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    body: Body,
) -> Result<(StatusCode, Json<File>), Refusal> {
    let digest = named(&name)?;
    let dir = dir(&api.calcifer);
    let path = dir.join(hex::encode(digest.bytes()));
    if path.is_file() {
        touch(&path);
        return Ok((StatusCode::OK, Json(file(digest, &path)?)));
    }
    let failed = |e: std::io::Error| Refusal(Error::new(Kind::Internal, "file_unwritable", format!("{}: {e}", dir.display())));
    std::fs::create_dir_all(&dir).map_err(failed)?;
    sweep(&dir);
    let mut part = tempfile::NamedTempFile::new_in(&dir).map_err(failed)?;
    let (mut hasher, mut stream) = (Sha256::new(), body.into_data_stream());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Refusal(Error::new(Kind::Validation, "body_unreadable", e.to_string())))?;
        hasher.update(&chunk);
        part.write_all(&chunk).map_err(failed)?;
    }
    let got = Digest::from_bytes(hasher.finalize().into());
    if got != digest {
        return Err(Refusal(Error::new(
            Kind::Validation,
            "file_digest_mismatch",
            format!("the bytes sent hash to {got}, not {digest}"),
        )));
    }
    part.as_file().sync_all().map_err(failed)?;
    part.persist(&path).map_err(|e| failed(e.error))?;
    Ok((StatusCode::CREATED, Json(file(digest, &path)?)))
}

/// Whether Calcifer holds `name`: 200 with its length and media type, else 404.
pub(crate) async fn head(State(api): State<Arc<Api>>, UrlPath(name): UrlPath<String>) -> Result<HeaderMap, Refusal> {
    let digest = named(&name)?;
    let path = dir(&api.calcifer).join(hex::encode(digest.bytes()));
    if !path.is_file() {
        return Err(Refusal(Error::new(Kind::NotFound, "file_not_found", format!("Calcifer holds no file {digest}"))));
    }
    let held = file(digest, &path)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(held.length));
    if let Ok(media_type) = HeaderValue::from_str(&held.media_type) {
        headers.insert(header::CONTENT_TYPE, media_type);
    }
    Ok(headers)
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
    let _ = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now()));
}

fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let before = SystemTime::now() - UNUSED;
    for entry in entries.flatten() {
        if entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at < before) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
