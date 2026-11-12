//! How Calcifer answers with a file (CALCIFER-API.md, Files): GET and HEAD, `Accept-Ranges`, one
//! `Range: bytes=…` as `206` (`416` when unsatisfiable, the whole when `If-Range` names another
//! revision), an `ETag` and `If-None-Match` as `304`, so a `<video>` seeks it and a browser caches
//! it. What a run wrote is never script in Calcifer's origin, where script is code execution as you:
//! each file is sandboxed, and only a picture, video or sound opens in place.
use std::fmt::Write as _;
use std::io::{Seek as _, SeekFrom};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use tokio::io::AsyncReadExt as _;

/// `Cache-Control` for what's named by its own hash: a browser never needs to ask again. Everything
/// else is the guard's `no-store`.
pub(crate) const IMMUTABLE: &str = "private, max-age=31536000, immutable";

/// `file` (`len` bytes of `media_type`, its revision `etag`, quoted) as `headers` ask for it.
pub(crate) fn send(
    mut file: std::fs::File,
    len: u64,
    media_type: &str,
    etag: &str,
    headers: &HeaderMap,
) -> std::io::Result<Response> {
    let octets = HeaderValue::from_static("application/octet-stream");
    let disposition = if shown_in_place(media_type) { "inline" } else { "attachment" };
    let media_type = HeaderValue::from_str(media_type).ok().filter(|v| !v.is_empty());
    let response = Response::builder()
        .header(header::CONTENT_TYPE, media_type.unwrap_or(octets))
        .header(header::CONTENT_SECURITY_POLICY, "sandbox")
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::ETAG, etag)
        .header(header::ACCEPT_RANGES, "bytes");
    let matched = (headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()))
        .is_some_and(|tags| tags.split(',').any(|tag| matches!(tag.trim(), "*") || tag.trim() == etag));
    if matched {
        return Ok(response.status(StatusCode::NOT_MODIFIED).body(Body::empty()).expect("a response"));
    }
    // A range of another revision would splice two files: the whole of this one instead.
    let same_revision = headers.get(header::IF_RANGE).is_none_or(|tag| tag.as_bytes() == etag.as_bytes());
    let range = headers.get(header::RANGE).filter(|_| same_revision);
    let (response, status, first, count) = match ranged(range, len) {
        Ok(None) => (response, StatusCode::OK, 0, len),
        Ok(Some((first, last))) => {
            let response = response.header(header::CONTENT_RANGE, format!("bytes {first}-{last}/{len}"));
            (response, StatusCode::PARTIAL_CONTENT, first, last - first + 1)
        }
        Err(()) => {
            return Ok(response
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .expect("a response"));
        }
    };
    file.seek(SeekFrom::Start(first))?;
    let bytes = tokio::fs::File::from_std(file).take(count);
    Ok(response
        .status(status)
        .header(header::CONTENT_LENGTH, count)
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(bytes)))
        .expect("a response"))
}

/// `?download=<name>` on a file route: a browser saves it as `name`, not shows it (cl-891).
#[derive(serde::Deserialize)]
pub(crate) struct Download {
    pub(crate) download: Option<String>,
}

/// `response` saved by a browser as `name`, with the file's extension `ext` when it ends otherwise: `hei.2` is `hei.2.png`.
pub(crate) fn save_as(response: &mut Response, name: &str, ext: &str) {
    let kept: String = name.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let mut name = kept.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_owned();
    if name.is_empty() {
        name.push_str("file");
    }
    if !ext.is_empty() && !name.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) {
        name = format!("{name}.{ext}");
    }
    let ascii: String = name.chars().map(|c| if c.is_ascii() { c } else { '_' }).collect();
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(char::from(b));
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    let value = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().insert(header::CONTENT_DISPOSITION, value);
    }
}

fn shown_in_place(media_type: &str) -> bool {
    let kind = media_type.split('/').next().unwrap_or_default();
    ["image", "video", "audio"].iter().any(|shown| kind.eq_ignore_ascii_case(shown))
}

/// The one range a `Range: bytes=…` header asks of `len` bytes, as (first, last): none for the
/// whole (no header, or another unit), `Err` for several ranges or one not satisfiable.
fn ranged(header: Option<&HeaderValue>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let asked = header.and_then(|h| h.to_str().ok());
    let Some(spec) = asked.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Ok(None);
    };
    let (first, last) = spec.split_once('-').ok_or(())?;
    let number = |text: &str| text.trim().parse::<u64>().map_err(|_| ());
    let end = len.checked_sub(1).ok_or(())?;
    let (first, last) = match (first.trim(), last.trim()) {
        ("", suffix) => (len.saturating_sub(number(suffix)?), end),
        (first, "") => (number(first)?, end),
        (first, last) => (number(first)?, number(last)?.min(end)),
    };
    if first > last {
        return Err(());
    }
    Ok(Some((first, last)))
}
