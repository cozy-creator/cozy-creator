//! How Calcifer answers with a file (CALCIFER-API.md, Files): GET and HEAD, `Accept-Ranges`, one
//! `Range: bytes=…` as `206` (`416` when unsatisfiable), an `ETag` and `If-None-Match` as `304`, so a
//! `<video>` seeks it and a browser caches it.
use std::io::{Seek as _, SeekFrom};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use tokio::io::AsyncReadExt as _;

/// `file` (`len` bytes of `media_type`, its revision `etag`, quoted) as `headers` ask for it.
pub fn send(
    mut file: std::fs::File,
    len: u64,
    media_type: &str,
    etag: &str,
    headers: &HeaderMap,
) -> std::io::Result<Response> {
    let octets = HeaderValue::from_static("application/octet-stream");
    let media_type = HeaderValue::from_str(media_type).ok().filter(|v| !v.is_empty());
    let response = Response::builder()
        .header(header::CONTENT_TYPE, media_type.unwrap_or(octets))
        .header(header::ETAG, etag)
        .header(header::ACCEPT_RANGES, "bytes");
    let matched = (headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()))
        .is_some_and(|tags| tags.split(',').any(|tag| matches!(tag.trim(), "*") || tag.trim() == etag));
    if matched {
        return Ok(response.status(StatusCode::NOT_MODIFIED).body(Body::empty()).expect("a response"));
    }
    let (response, status, first, count) = match ranged(headers.get(header::RANGE), len) {
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
