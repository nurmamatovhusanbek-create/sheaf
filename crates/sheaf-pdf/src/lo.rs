//! Object-level PDF work with lopdf: compression (TOOL-COMPRESS), encryption
//! (TOOL-PROTECT), embedded-image extraction (TOOL-PDF2IMG).
//!
//! These run on files this app has just built with PDFium, inside the
//! session dir, or on a user's file for image extraction. Like everything in
//! this crate, only inside a worker process.

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions, Stream, StringFormat,
};
use sheaf_core::proto::{MetaEdit, MetaFields, MetadataInfo};
use sheaf_core::{AppError, CompressLevel, ErrorCode, Protect, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zeroize::Zeroize;

/// Largest decoded image handled while recompressing or extracting.
const MAX_IMAGE_BYTES: usize = 600 << 20;

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

/// TOOL-REPAIR: read a damaged PDF the tolerant way (a missing or wrong cross-reference table, a
/// lost trailer, a cut-off end are rebuilt by scanning for the objects themselves) and write the
/// whole structure out again. Returns the number of pages found in the page tree; the caller opens
/// the result with the renderer and reports that count, which is the honest one. A locked file is
/// not repaired (it would need its password), and nothing readable is an error, not an empty file.
pub fn repair(path: &Path, out: &Path) -> Result<u32> {
    let size = std::fs::metadata(path)
        .map_err(|e| AppError::io("read file", &e))?
        .len();
    if size > sheaf_core::consts::MAX_FILE_BYTES {
        return Err(AppError::limit("file is too large"));
    }
    let bytes = std::fs::read(path).map_err(|e| AppError::io("read file", &e))?;
    let mut doc = Document::load_mem(&bytes).map_err(map_err)?;
    drop(bytes);
    if doc.is_encrypted() {
        return Err(AppError::new(
            ErrorCode::NeedsPassword,
            "document is password protected",
        ));
    }
    let pages = doc.get_pages().len() as u32;
    if pages == 0 {
        return Err(AppError::new(ErrorCode::ParseError, "no pages could be recovered"));
    }
    doc.save(out).map_err(|e| AppError::io("write repaired file", &e))?;
    Ok(pages)
}

// -- TOOL-METADATA ---------------------------------------------------------

/// A PDF text string: UTF-16BE with a byte order mark, UTF-8 with a mark, or PDFDocEncoding
/// (which for the characters people type is Latin-1).
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (pairs, _) = rest.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|c| u16::from_be_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Plain ASCII goes in as is; anything else as UTF-16BE with its mark, which every reader understands.
fn encode_text(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut out = vec![0xFE, 0xFF];
        for unit in s.encode_utf16() {
            out.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(out, StringFormat::Hexadecimal)
    }
}

fn text_of(doc: &Document, object: &Object) -> String {
    match object {
        Object::String(bytes, _) => decode_text(bytes),
        Object::Reference(id) => doc.get_object(*id).map(|o| text_of(doc, o)).unwrap_or_default(),
        _ => String::new(),
    }
}

fn info_id(doc: &Document) -> Option<ObjectId> {
    doc.trailer.get(b"Info").ok()?.as_reference().ok()
}

fn read_info(doc: &Document) -> MetadataInfo {
    let field = |key: &[u8]| -> String {
        info_id(doc)
            .and_then(|id| doc.get_dictionary(id).ok())
            .and_then(|d| d.get(key).ok())
            .map(|o| text_of(doc, o))
            .unwrap_or_default()
    };
    MetadataInfo {
        fields: MetaFields {
            title: field(b"Title"),
            author: field(b"Author"),
            subject: field(b"Subject"),
            keywords: field(b"Keywords"),
            creator: field(b"Creator"),
            producer: field(b"Producer"),
        },
        created: field(b"CreationDate"),
        modified: field(b"ModDate"),
        has_xmp: doc.catalog().is_ok_and(|c| c.has(b"Metadata")),
        pages: doc.get_pages().len() as u32,
    }
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// The XMP packet that says the same as the information fields (Dublin Core and the PDF schema).
fn xmp_packet(f: &MetaFields) -> String {
    let mut out = String::from(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n",
    );
    let alt = |tag: &str, v: &str| {
        format!(
            "<dc:{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:{tag}>\n",
            xml_escape(v)
        )
    };
    if !f.title.is_empty() {
        out.push_str(&alt("title", &f.title));
    }
    if !f.subject.is_empty() {
        out.push_str(&alt("description", &f.subject));
    }
    if !f.author.is_empty() {
        out.push_str(&format!(
            "<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n",
            xml_escape(&f.author)
        ));
    }
    if !f.keywords.is_empty() {
        out.push_str(&format!("<pdf:Keywords>{}</pdf:Keywords>\n", xml_escape(&f.keywords)));
    }
    if !f.producer.is_empty() {
        out.push_str(&format!("<pdf:Producer>{}</pdf:Producer>\n", xml_escape(&f.producer)));
    }
    if !f.creator.is_empty() {
        out.push_str(&format!(
            "<xmp:CreatorTool>{}</xmp:CreatorTool>\n",
            xml_escape(&f.creator)
        ));
    }
    out.push_str("</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    out
}

fn drop_object(doc: &mut Document, id: ObjectId) {
    doc.objects.remove(&id);
}

/// Remove the information fields and the XMP packet.
fn strip(doc: &mut Document) {
    if let Some(id) = info_id(doc) {
        drop_object(doc, id);
    }
    doc.trailer.remove(b"Info");
    let xmp = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Metadata").ok())
        .and_then(|o| o.as_reference().ok());
    if let Some(id) = xmp {
        drop_object(doc, id);
    }
    if let Ok(catalog) = doc.catalog_mut() {
        catalog.remove(b"Metadata");
    }
}

/// Write the fields to the information dictionary and to a fresh XMP packet; the old packet goes,
/// so no stale value survives in it. An empty field is left out.
fn set(doc: &mut Document, f: &MetaFields) {
    let old_xmp = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Metadata").ok())
        .and_then(|o| o.as_reference().ok());
    let id = info_id(doc)
        .filter(|id| doc.get_dictionary(*id).is_ok())
        .unwrap_or_else(|| {
            let id = doc.add_object(Dictionary::new());
            doc.trailer.set("Info", Object::Reference(id));
            id
        });
    if let Ok(info) = doc.get_dictionary_mut(id) {
        for (key, value) in [
            ("Title", &f.title),
            ("Author", &f.author),
            ("Subject", &f.subject),
            ("Keywords", &f.keywords),
            ("Creator", &f.creator),
            ("Producer", &f.producer),
        ] {
            if value.is_empty() {
                info.remove(key.as_bytes());
            } else {
                info.set(key, encode_text(value));
            }
        }
    }
    if let Some(id) = old_xmp {
        drop_object(doc, id);
    }
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Metadata".to_vec()));
    dict.set("Subtype", Object::Name(b"XML".to_vec()));
    let mut stream = Stream::new(dict, xmp_packet(f).into_bytes());
    // Readers look for the packet in plain text.
    stream.allows_compression = false;
    let xmp = doc.add_object(stream);
    if let Ok(catalog) = doc.catalog_mut() {
        catalog.set("Metadata", Object::Reference(xmp));
    }
}

fn load_for_metadata(path: &Path) -> Result<Document> {
    let size = std::fs::metadata(path)
        .map_err(|e| AppError::io("read file", &e))?
        .len();
    if size > sheaf_core::consts::MAX_FILE_BYTES {
        return Err(AppError::limit("file is too large"));
    }
    let doc = Document::load(path).map_err(map_err)?;
    if doc.is_encrypted() {
        return Err(AppError::new(
            ErrorCode::NeedsPassword,
            "document is password protected",
        ));
    }
    Ok(doc)
}

/// TOOL-METADATA: what a PDF says about itself.
pub fn read_metadata(path: &Path) -> Result<MetadataInfo> {
    Ok(read_info(&load_for_metadata(path)?))
}

/// TOOL-METADATA: write a changed copy and return what is really in it (read back from the file).
pub fn edit_metadata(path: &Path, out: &Path, edit: &MetaEdit) -> Result<MetadataInfo> {
    let mut doc = load_for_metadata(path)?;
    match edit {
        MetaEdit::Strip => strip(&mut doc),
        MetaEdit::Set(fields) => {
            fields.validate()?;
            set(&mut doc, fields);
        }
    }
    doc.save(out).map_err(|e| AppError::io("write file", &e))?;
    read_metadata(out)
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
