use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use sheaf_core::consts::{MAX_PAGE_PIXELS, MAX_PAGE_SIDE};
use sheaf_core::{AppError, ErrorCode, Result};
use std::io::{BufRead, Cursor, Seek};
use std::path::Path;

/// Reject bitmaps that would not fit the memory budget (SEC-LIMITS). Called
/// with the size from the file header, before any pixel is decoded.
pub fn check_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(AppError::new(ErrorCode::ParseError, "image has no pixels"));
    }
    if width > MAX_PAGE_SIDE
        || height > MAX_PAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_PAGE_PIXELS
    {
        return Err(AppError::limit("image dimensions exceed the page limit"));
    }
    Ok(())
}

fn map_err(e: image::ImageError) -> AppError {
    use image::ImageError as E;
    match e {
        E::Unsupported(_) => AppError::new(ErrorCode::Unsupported, "image format not supported"),
        E::Limits(_) => AppError::limit("image exceeds decoder limits"),
        E::IoError(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
            AppError::new(ErrorCode::ParseError, "image data is truncated")
        }
        E::IoError(io) => AppError::io("read image", &io),
        // Decoder messages can quote file content; keep them out (SEC-LOG).
        _ => AppError::new(ErrorCode::ParseError, "image could not be decoded"),
    }
}

fn reader<R: BufRead + Seek>(r: R) -> Result<ImageReader<R>> {
    let reader = ImageReader::new(r)
        .with_guessed_format()
        .map_err(|e| AppError::io("read image", &e))?;
    match reader.format() {
        Some(
            ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::Tiff
            | ImageFormat::Bmp
            | ImageFormat::WebP,
        ) => Ok(reader),
        _ => Err(AppError::new(
            ErrorCode::Unsupported,
            "image format not supported",
        )),
    }
}

fn decode<R: BufRead + Seek>(r: R) -> Result<RgbaImage> {
    let mut reader = reader(r)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PAGE_SIDE);
    limits.max_image_height = Some(MAX_PAGE_SIDE);
    limits.max_alloc = Some(MAX_PAGE_PIXELS * 4 + (64 << 20));
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(map_err)?;
    let (w, h) = decoder.dimensions();
    check_dimensions(w, h)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(map_err)?;
    img.apply_orientation(orientation);
    Ok(img.into_rgba8())
}

/// Decode JPEG, PNG, TIFF, BMP or WebP, upright (EXIF orientation applied).
pub fn decode_file(path: &Path) -> Result<RgbaImage> {
    let file = std::fs::File::open(path).map_err(|e| AppError::io("open image", &e))?;
    decode(std::io::BufReader::new(file))
}

pub fn decode_bytes(bytes: &[u8]) -> Result<RgbaImage> {
    decode(Cursor::new(bytes))
}

/// Width and height as displayed (EXIF orientation applied), from the header only.
pub fn dimensions(path: &Path) -> Result<(u32, u32)> {
    let file = std::fs::File::open(path).map_err(|e| AppError::io("open image", &e))?;
    let mut decoder = reader(std::io::BufReader::new(file))?
        .into_decoder()
        .map_err(map_err)?;
    let (w, h) = decoder.dimensions();
    check_dimensions(w, h)?;
    use image::metadata::Orientation as O;
    let swapped = matches!(
        decoder.orientation().unwrap_or(O::NoTransforms),
        O::Rotate90 | O::Rotate270 | O::Rotate90FlipH | O::Rotate270FlipH
    );
    Ok(if swapped { (h, w) } else { (w, h) })
}

pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).map_err(map_err)?;
    Ok(out.into_inner())
}

/// JPEG has no alpha: the image is flattened onto white first.
pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>> {
    let mut rgb = image::RgbImage::new(img.width(), img.height());
    for (dst, src) in rgb.pixels_mut().zip(img.pixels()) {
        let a = u32::from(src[3]);
        for c in 0..3 {
            dst[c] = ((u32::from(src[c]) * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    let mut out = Vec::new();
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    rgb.write_with_encoder(encoder).map_err(map_err)?;
    Ok(out)
}
