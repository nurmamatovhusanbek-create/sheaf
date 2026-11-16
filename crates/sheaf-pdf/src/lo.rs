//! Object-level PDF work with lopdf: compression (TOOL-COMPRESS), encryption
//! (TOOL-PROTECT), embedded-image extraction (TOOL-PDF2IMG).
//!
//! These run on files this app has just built with PDFium, inside the
//! session dir, or on a user's file for image extraction. Like everything in
//! this crate, only inside a worker process.

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions};
use sheaf_core::{AppError, CompressLevel, ErrorCode, Protect, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zeroize::Zeroize;

/// Largest decoded image handled while recompressing or extracting.
const MAX_IMAGE_BYTES: usize = 600 << 20;
/// Most image files written for one page: a hostile page can reference any
/// number of them, and each one is a file and a line of the job's result.
const MAX_EXTRACTED_IMAGES: usize = 2_000;

fn map_err(e: lopdf::Error) -> AppError {
    use lopdf::Error as E;
    match e {
        E::IO(io) => AppError::io("pdf i/o", &io),
        E::Decryption(_) | E::InvalidPassword | E::Unimplemented(_) if is_password(&e) => {
            AppError::new(ErrorCode::NeedsPassword, "document is password protected")
        }
        // Variant names only: messages could quote document content (SEC-LOG).
        other => {
            let dbg = format!("{other:?}");
            let name = dbg.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("error");
            AppError::new(ErrorCode::ParseError, format!("pdf objects: {name}"))
        }
    }
}

fn is_password(e: &lopdf::Error) -> bool {
    matches!(e, lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_))
}

fn load(path: &Path, password: Option<&str>) -> Result<Document> {
    match password {
        Some(pw) => Document::load_with_password(path, pw),
        None => Document::load(path),
    }
    .map_err(map_err)
}

/// What [`compress`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// The result was not smaller, so `out` is a copy of the input.
    pub kept_original: bool,
    pub images_recompressed: u32,
}

/// An image XObject that can be decoded and written back as JPEG.
struct Candidate {
    id: ObjectId,
    /// Resolution the image has where it is used, taking the page it covers as its size.
    dpi: f32,
}

fn page_width_pt(doc: &Document, page_id: ObjectId) -> f32 {
    // MediaBox may be inherited from the page tree.
    let mut id = page_id;
    for _ in 0..32 {
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        if let Ok(Object::Array(mb)) = dict.get(b"MediaBox") {
            let num = |o: &Object| o.as_float().ok().or_else(|| o.as_i64().ok().map(|v| v as f32));
            if let (Some(x0), Some(x1)) = (mb.first().and_then(num), mb.get(2).and_then(num)) {
                return (x1 - x0).abs().max(1.0);
            }
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => id = parent,
            Err(_) => break,
        }
    }
    612.0
}

/// Decode an image stream to 8-bit gray or RGB, if it is one of the plain
/// kinds: JPEG, or flate/raw with 8 bits per component. Masks, indexed and
/// CMYK images, CCITT and JBIG2 are left alone.
fn decode(doc: &Document, id: ObjectId) -> Option<image::DynamicImage> {
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    let dict = &stream.dict;
    let has = |key: &[u8]| dict.get(key).is_ok();
    if has(b"Mask") || has(b"Decode") || dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) {
        return None;
    }
    let (w, h) = (
        dict.get(b"Width").ok()?.as_i64().ok()?,
        dict.get(b"Height").ok()?.as_i64().ok()?,
    );
    if w <= 0 || h <= 0 || (w as u128) * (h as u128) * 3 > MAX_IMAGE_BYTES as u128 {
        return None;
    }
    let filters = stream.filters().unwrap_or_default();
    match filters.as_slice() {
        [b"DCTDecode"] => {
            let img = image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg).ok()?;
            // CMYK and other exotic JPEGs come back in forms we do not rewrite.
            matches!(
                img,
                image::DynamicImage::ImageRgb8(_) | image::DynamicImage::ImageLuma8(_)
            )
            .then_some(img)
        }
        [] | [b"FlateDecode"] => {
            if dict.get(b"BitsPerComponent").and_then(Object::as_i64).ok()? != 8 {
                return None;
            }
            let channels = match dict.get(b"ColorSpace").and_then(Object::as_name).ok()? {
                b"DeviceRGB" => 3usize,
                b"DeviceGray" => 1,
                _ => return None,
            };
            let raw = match filters.is_empty() {
                true => stream.content.clone(),
                false => stream.decompressed_content_with_limit(MAX_IMAGE_BYTES).ok()?,
            };
            if raw.len() != w as usize * h as usize * channels {
                return None;
            }
            match channels {
                3 => image::RgbImage::from_raw(w as u32, h as u32, raw).map(image::DynamicImage::ImageRgb8),
                _ => image::GrayImage::from_raw(w as u32, h as u32, raw).map(image::DynamicImage::ImageLuma8),
            }
        }
        _ => None,
    }
}

fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    img.write_with_encoder(encoder).ok()?;
    Some(out)
}

/// TOOL-COMPRESS. `Low` only re-packs (object streams, unused objects
/// dropped, streams deflated). The other levels also downsample images above
/// the level's resolution and store them as JPEG. Images are never enlarged,
/// an image whose new encoding is not smaller keeps its old one, and if the
/// whole file does not shrink, `out` is a copy of `input`.
pub fn compress(input: &Path, out: &Path, level: CompressLevel) -> Result<CompressReport> {
    let bytes_before = std::fs::metadata(input)
        .map_err(|e| AppError::io("stat input", &e))?
        .len();
    let mut doc = load(input, None)?;
    let mut recompressed = 0u32;

    if let Some((target_dpi, quality)) = level.image_target() {
        // An image may sit on several pages: size it for the page where it is sharpest.
        let mut candidates: BTreeMap<ObjectId, Candidate> = BTreeMap::new();
        for page_id in doc.page_iter().collect::<Vec<_>>() {
            let width_in = page_width_pt(&doc, page_id) / 72.0;
            for img in doc.get_page_images(page_id).unwrap_or_default() {
                let dpi = img.width as f32 / width_in;
                let entry = candidates.entry(img.id).or_insert(Candidate { id: img.id, dpi });
                entry.dpi = entry.dpi.min(dpi);
            }
        }
        for cand in candidates.into_values() {
            let Some(img) = decode(&doc, cand.id) else {
                continue;
            };
            let scale = (target_dpi as f32 / cand.dpi).min(1.0);
            let (w, h) = (
                ((img.width() as f32 * scale).round() as u32).max(1),
                ((img.height() as f32 * scale).round() as u32).max(1),
            );
            let resized = match scale < 1.0 {
                true => img.resize_exact(w, h, image::imageops::FilterType::Triangle),
                false => img,
            };
            let Some(jpeg) = encode_jpeg(&resized, quality) else {
                continue;
            };
            let Ok(Object::Stream(stream)) = doc.get_object_mut(cand.id) else {
                continue;
            };
            if jpeg.len() >= stream.content.len() {
                continue; // already small: leave it exactly as it was
            }
            let gray = matches!(resized, image::DynamicImage::ImageLuma8(_));
            stream.dict.set("Width", i64::from(resized.width()));
            stream.dict.set("Height", i64::from(resized.height()));
            stream.dict.set(
                "ColorSpace",
                Object::Name(if gray {
                    b"DeviceGray".to_vec()
                } else {
                    b"DeviceRGB".to_vec()
                }),
            );
            stream.dict.set("BitsPerComponent", 8i64);
            stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            stream.dict.remove(b"DecodeParms");
            stream.set_content(jpeg);
            recompressed += 1;
        }
    }

    doc.prune_objects();
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_modern(&mut bytes)
        .map_err(|e| AppError::io("write compressed", &e))?;
    let kept_original = bytes.len() as u64 >= bytes_before;
    if kept_original {
        std::fs::copy(input, out).map_err(|e| AppError::io("keep original", &e))?;
    } else {
        std::fs::write(out, &bytes).map_err(|e| AppError::io("write compressed", &e))?;
    }
    Ok(CompressReport {
        bytes_before,
        bytes_after: if kept_original {
            bytes_before
        } else {
            bytes.len() as u64
        },
        kept_original,
        images_recompressed: if kept_original { 0 } else { recompressed },
    })
}

/// The protection [`encrypt`] applies, as shown to the user (SEC-HONESTY).
pub const ENCRYPTION_LABEL: &str = "AES-256";

/// TOOL-PROTECT: AES-256 (PDF 2.0 security handler, revision 6). Returns the
/// label of what was applied, after reading it back from the written file.
pub fn encrypt(input: &Path, out: &Path, protect: &Protect) -> Result<String> {
    let mut doc = load(input, None)?;
    if doc.is_encrypted() {
        return Err(AppError::invalid("document is already encrypted"));
    }
    // Printing is on or off as a whole; accessibility extraction stays allowed.
    let mut permissions = Permissions::COPYABLE_FOR_ACCESSIBILITY;
    if protect.allow_print {
        permissions |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY;
    }
    if protect.allow_copy {
        permissions |= Permissions::COPYABLE;
    }
    if protect.allow_edit {
        permissions |=
            Permissions::MODIFIABLE | Permissions::ANNOTABLE | Permissions::FILLABLE | Permissions::ASSEMBLABLE;
    }
    let random = |buf: &mut [u8]| getrandom::fill(buf).map_err(|_| AppError::internal("no secure random source"));
    // An encrypted file must carry a file identifier; readers warn without one.
    if doc.trailer.get(b"ID").is_err() {
        let mut id = [0u8; 16];
        random(&mut id)?;
        let part = || Object::String(id.to_vec(), lopdf::StringFormat::Hexadecimal);
        doc.trailer.set("ID", Object::Array(vec![part(), part()]));
    }
    let mut key = [0u8; 32];
    random(&mut key)?;
    // No owner password given: use one nobody knows, so the restrictions
    // cannot be lifted with the password that merely opens the file.
    let mut owner = protect.owner_pw.expose().to_string();
    if owner.is_empty() {
        let mut raw = [0u8; 24];
        random(&mut raw)?;
        owner = raw.iter().map(|b| format!("{b:02x}")).collect();
    }
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: &owner,
        user_password: protect.user_pw.expose(),
        permissions,
    };
    let state = EncryptionState::try_from(version).map_err(map_err);
    let applied = state.and_then(|state| doc.encrypt(&state).map_err(map_err));
    key.zeroize();
    owner.zeroize();
    applied?;
    // Report what is in the document, not what was asked for.
    let handler = doc
        .trailer
        .get(b"Encrypt")
        .and_then(Object::as_reference)
        .and_then(|id| doc.get_dictionary(id))
        .ok()
        .and_then(|enc| {
            Some((
                enc.get(b"V").and_then(Object::as_i64).ok()?,
                enc.get(b"R").and_then(Object::as_i64).ok()?,
            ))
        });
    if handler != Some((5, 6)) {
        return Err(AppError::internal(
            "encryption did not produce the expected security handler",
        ));
    }
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| AppError::io("write protected file", &e))?;
    std::fs::write(out, &bytes).map_err(|e| AppError::io("write protected file", &e))?;
    Ok(ENCRYPTION_LABEL.to_string())
}

/// TOOL-PDF2IMG, alternative mode: write the images embedded in one page into
/// `out_dir` as `<stem>-<k>.jpg` (JPEG data, byte for byte) or `.png`
/// (decoded flate images). Returns the files written and how many images
/// were skipped because they are in a form with no common file equivalent.
pub fn extract_images(
    input: &Path,
    password: Option<&str>,
    page_index: u32,
    out_dir: &Path,
    stem: &str,
) -> Result<(Vec<PathBuf>, u32)> {
    let doc = load(input, password)?;
    let page_id = doc
        .page_iter()
        .nth(page_index as usize)
        .ok_or_else(|| AppError::invalid("page index out of range"))?;
    let ids: Vec<ObjectId> = doc
        .get_page_images(page_id)
        .map_err(map_err)?
        .iter()
        .map(|i| i.id)
        .collect();
    let (mut files, mut skipped) = (Vec::new(), 0u32);
    for id in ids {
        if files.len() >= MAX_EXTRACTED_IMAGES {
            skipped += 1;
            continue;
        }
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            skipped += 1;
            continue;
        };
        let k = files.len() + 1;
        let is_jpeg = matches!(stream.filters().unwrap_or_default().as_slice(), [b"DCTDecode"]);
        let (path, bytes) = if is_jpeg {
            (out_dir.join(format!("{stem}-{k}.jpg")), stream.content.clone())
        } else {
            let Some(img) = decode(&doc, id) else {
                skipped += 1;
                continue;
            };
            let mut png = std::io::Cursor::new(Vec::new());
            if img.write_to(&mut png, image::ImageFormat::Png).is_err() {
                skipped += 1;
                continue;
            }
            (out_dir.join(format!("{stem}-{k}.png")), png.into_inner())
        };
        std::fs::write(&path, bytes).map_err(|e| AppError::io("write extracted image", &e))?;
        files.push(path);
    }
    Ok((files, skipped))
}
