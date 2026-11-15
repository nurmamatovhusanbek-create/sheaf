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
    if width > MAX_PAGE_SIDE || height > MAX_PAGE_SIDE || u64::from(width) * u64::from(height) > MAX_PAGE_PIXELS {
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
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Tiff | ImageFormat::Bmp | ImageFormat::WebP) => {
            Ok(reader)
        }
        _ => Err(AppError::new(ErrorCode::Unsupported, "image format not supported")),
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
    let mut decoder = reader(std::io::BufReader::new(file))?.into_decoder().map_err(map_err)?;
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
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    rgb.write_with_encoder(encoder).map_err(map_err)?;
    Ok(out)
}

/// Binary PPM (P6): uncompressed, so writing a full-page bitmap costs almost
/// nothing. Used for the image handed to the OCR engine. Alpha is flattened
/// onto white.
pub fn encode_ppm(img: &RgbaImage) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", img.width(), img.height()).into_bytes();
    out.reserve(img.width() as usize * img.height() as usize * 3);
    for p in img.pixels() {
        let a = u32::from(p[3]);
        for c in 0..3 {
            out.push(((u32::from(p[c]) * a + 255 * (255 - a)) / 255) as u8);
        }
    }
    out
}

fn has_magic(path: &Path, magics: &[&[u8]]) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| magics.iter().any(|m| head.starts_with(m)))
}

pub fn is_jpeg(path: &Path) -> bool {
    has_magic(path, &[&[0xFF, 0xD8, 0xFF]])
}

fn is_tiff(path: &Path) -> bool {
    has_magic(path, &[b"II*\0", b"MM\0*"])
}

/// For embedding a JPEG without re-encoding (TOOL-IMG2PDF): the pixel size as
/// stored, and the clockwise turn its EXIF orientation asks for. `None` when
/// the file is not a JPEG that can be passed through: not a JPEG at all, or
/// mirrored, which a page rotation cannot express.
pub fn jpeg_passthrough(path: &Path) -> Option<((u32, u32), sheaf_core::Rot)> {
    use image::metadata::Orientation as O;
    use sheaf_core::Rot;
    if !is_jpeg(path) {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = reader(std::io::BufReader::new(file)).ok()?.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    check_dimensions(w, h).ok()?;
    // Only plain 8-bit gray or RGB: PDF readers disagree about the rest (CMYK, 12-bit).
    if !matches!(decoder.color_type(), image::ColorType::Rgb8 | image::ColorType::L8) {
        return None;
    }
    let rot = match decoder.orientation().unwrap_or(O::NoTransforms) {
        O::NoTransforms => Rot::R0,
        O::Rotate90 => Rot::R90,
        O::Rotate180 => Rot::R180,
        O::Rotate270 => Rot::R270,
        _ => return None,
    };
    Some(((w, h), rot))
}

/// Most pages read from one TIFF.
const MAX_TIFF_PAGES: u32 = 5_000;

fn tiff_decoder(path: &Path) -> Result<tiff::decoder::Decoder<std::io::BufReader<std::fs::File>>> {
    let file = std::fs::File::open(path).map_err(|e| AppError::io("open image", &e))?;
    let mut limits = tiff::decoder::Limits::default();
    limits.decoding_buffer_size = (MAX_PAGE_PIXELS * 4) as usize;
    tiff::decoder::Decoder::new(std::io::BufReader::new(file))
        .map(|d| d.with_limits(limits))
        .map_err(|_| AppError::new(ErrorCode::ParseError, "image could not be decoded"))
}

/// Pages in an image file: more than one only for a multipage TIFF.
pub fn page_count(path: &Path) -> Result<u32> {
    if !is_tiff(path) {
        return Ok(1);
    }
    let mut decoder = tiff_decoder(path)?;
    let mut pages = 1;
    while decoder.more_images() && pages < MAX_TIFF_PAGES {
        if decoder.next_image().is_err() {
            break;
        }
        pages += 1;
    }
    Ok(pages)
}

/// Decode one page of an image file. Page 0 of anything goes through the
/// general decoder; later pages exist only in TIFF files.
pub fn decode_page(path: &Path, index: u32) -> Result<RgbaImage> {
    if index == 0 {
        return decode_file(path);
    }
    if !is_tiff(path) || index >= MAX_TIFF_PAGES {
        return Err(AppError::invalid("page index out of range"));
    }
    let bad = || AppError::new(ErrorCode::ParseError, "image could not be decoded");
    let mut decoder = tiff_decoder(path)?;
    decoder
        .seek_to_image(index as usize)
        .map_err(|_| AppError::invalid("page index out of range"))?;
    let (w, h) = decoder.dimensions().map_err(|_| bad())?;
    check_dimensions(w, h)?;
    let color = decoder.colortype().map_err(|_| bad())?;
    let tiff::decoder::DecodingResult::U8(data) = decoder.read_image().map_err(|_| bad())? else {
        return Err(AppError::new(
            ErrorCode::Unsupported,
            "TIFF sample format not supported",
        ));
    };
    let pixels = w as usize * h as usize;
    let channels = match color {
        tiff::ColorType::Gray(8) => 1,
        tiff::ColorType::GrayA(8) => 2,
        tiff::ColorType::RGB(8) => 3,
        tiff::ColorType::RGBA(8) => 4,
        _ => {
            return Err(AppError::new(
                ErrorCode::Unsupported,
                "TIFF colour format not supported",
            ))
        }
    };
    if data.len() != pixels * channels {
        return Err(bad());
    }
    let mut out = RgbaImage::new(w, h);
    for (dst, src) in out.pixels_mut().zip(data.chunks_exact(channels)) {
        dst.0 = match channels {
            1 => [src[0], src[0], src[0], 255],
            2 => [src[0], src[0], src[0], src[1]],
            3 => [src[0], src[1], src[2], 255],
            _ => [src[0], src[1], src[2], src[3]],
        };
    }
    Ok(out)
}
