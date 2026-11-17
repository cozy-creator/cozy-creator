//! A still picture (PNG, JPEG, WebP, GIF's first frame) read in Calcifer itself: its thumbnail and its measure with no ffmpeg started.
//! On a Mac runner each start of the 100 MB static ffmpeg and ffprobe cost a picture about a second, two at a time: three tabs opening a
//! project of 50 pictures with none stored drew 4 and 17 of them in 15 s (opens, e2e 44011938933). Any other picture, and every video
//! and sound, is ffmpeg's; a still that won't decode here is too.
use std::{
    io::{BufWriter, Write as _},
    path::Path,
};

use image::{
    DynamicImage, ImageFormat, ImageReader,
    codecs::{jpeg::JpegEncoder, png::{CompressionType, FilterType as PngFilter, PngEncoder}},
    imageops::FilterType,
    metadata::Orientation,
};

use super::folder;

const READ: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
/// What ffmpeg's `-q:v 3` gave a thumbnail.
const QUALITY: u8 = 90;

/// Whether `path` is named as a still read here.
pub(super) fn named(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| READ.iter().any(|read| ext.eq_ignore_ascii_case(read)))
}

/// `path` decoded, turned by its EXIF as a browser draws it. None: not a still read here, or it won't decode.
fn decoded(path: &Path) -> Option<DynamicImage> {
    if !named(path) {
        return None;
    }
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif)) {
        return None;
    }
    let mut image = reader.decode().ok()?;
    if let Some(turn) = Orientation::from_exif(super::tools::orientation(path)) {
        image.apply_orientation(turn);
    }
    Some(image)
}

/// What measuring finds of the still at `path`: its size as it's drawn and the share of it fully transparent. None: ffmpeg's to measure.
pub(super) fn measured(path: &Path) -> Option<folder::File> {
    let image = decoded(path)?;
    let alpha = match image.color().has_alpha() {
        true => {
            let pixels = image.to_rgba8();
            pixels.pixels().filter(|pixel| pixel[3] == 0).count() as f64 / (u64::from(image.width()) * u64::from(image.height())).max(1) as f64
        }
        false => 0.0,
    };
    Some(folder::File { width: u64::from(image.width()), height: u64::from(image.height()), alpha, ..Default::default() })
}

/// The still at `path` drawn at most `width` wide into the file `part` makes, as ffmpeg drew it (`scale='min(iw,W)':-2`): a PNG with
/// its alpha, else a JPEG. None: not a still read here.
pub(super) fn draw(
    path: &Path,
    width: u32,
    part: impl Fn(&str) -> Result<tempfile::NamedTempFile, String>,
) -> Option<Result<(tempfile::NamedTempFile, &'static str), String>> {
    let image = decoded(path)?;
    let wide = image.width().min(width).max(1);
    let high = ((f64::from(image.height()) * f64::from(wide) / f64::from(image.width().max(1)) / 2.0).round() as u32 * 2).max(2);
    let image = match (wide, high) == (image.width(), image.height()) {
        true => image,
        false => image.resize_exact(wide, high, FilterType::CatmullRom),
    };
    Some(encoded(&image, part))
}

/// `image` written into the file `part` makes: a PNG with its alpha, else a JPEG.
fn encoded(image: &DynamicImage, part: impl Fn(&str) -> Result<tempfile::NamedTempFile, String>) -> Result<(tempfile::NamedTempFile, &'static str), String> {
    let ext = if image.color().has_alpha() { "png" } else { "jpg" };
    let out = part(ext)?;
    let mut file = BufWriter::new(out.as_file());
    let written = match ext {
        "png" => image.to_rgba8().write_with_encoder(PngEncoder::new_with_quality(&mut file, CompressionType::Fast, PngFilter::Adaptive)),
        _ => image.to_rgb8().write_with_encoder(JpegEncoder::new_with_quality(&mut file, QUALITY)),
    };
    written.map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    Ok((out, ext))
}
