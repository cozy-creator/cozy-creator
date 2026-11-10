//! Inputs to desktop ffmpeg/ffprobe: one media file, selected by its bytes. A remote output
//! may be a playlist with a media filename; neither auto-detection nor image-sequence paths
//! may make it read another file. Internal edit concatenation uses its own generated list.
use std::path::Path;

use protocol::error::{Error, Kind};

/// Input options, before `-i` (ffmpeg) or the input path (ffprobe). Fixed image pipe demuxers
/// read a single file without interpreting a filename as an image-sequence pattern.
pub(crate) fn options(path: &Path) -> Result<Vec<&'static str>, Error> {
    let kind = crate::runs::bind::sniff(path)?.media_type;
    let format = match kind.as_str() {
        "image/jpeg" => "jpeg_pipe",
        "image/jp2" => "j2k_pipe",
        "image/png" => "png_pipe",
        "image/gif" => "gif",
        "image/webp" => "webp_pipe",
        "image/tiff" | "image/x-canon-cr2" => "tiff_pipe",
        "image/bmp" => "bmp_pipe",
        "image/vnd.adobe.photoshop" => "psd_pipe",
        "image/vnd.microsoft.icon" => "ico",
        "image/jxl" => "jpegxl_pipe",
        "image/avif" | "image/heif" | "video/mp4" | "video/x-m4v" | "video/quicktime"
        | "audio/m4a" => "mov",
        "video/x-matroska" | "video/webm" => "matroska",
        "video/x-msvideo" => "avi",
        "video/x-ms-wmv" => "asf",
        "video/mpeg" => "mpeg",
        "video/x-flv" => "flv",
        "audio/mpeg" => "mp3",
        "audio/opus" | "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" => "wav",
        "audio/amr" => "amr",
        "audio/aac" => "aac",
        "audio/x-aiff" => "aiff",
        "audio/x-dsf" => "dsf",
        "audio/x-ape" => "ape",
        _ => {
            return Err(Error::new(
                Kind::Validation,
                "media_format_unsupported",
                format!(
                    "{} is not a supported single media file ({kind})",
                    path.display()
                ),
            ));
        }
    };
    let mut options = vec!["-protocol_whitelist", "file", "-f", format];
    if format == "mov" {
        options.extend(["-enable_drefs", "0", "-use_absolute_path", "0"]);
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_bytes_choose_the_demuxer_and_playlists_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("video.mp4.part");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        assert_eq!(
            options(&path).unwrap(),
            ["-protocol_whitelist", "file", "-f", "png_pipe"]
        );
        for bytes in [
            b"ffconcat version 1.0\nfile marker.mp4\n".as_slice(),
            b"#EXTM3U\nmarker.mp4\n".as_slice(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(options(&path).unwrap_err().code, "media_format_unsupported");
        }
    }
}
