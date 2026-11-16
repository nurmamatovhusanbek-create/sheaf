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
use sheaf_core::consts::{MAX_PAGE_PIXELS, MAX_PAGE_SIDE};
use sheaf_core::proto::{MetaEdit, MetaFields, MetadataInfo};
use sheaf_core::{AppError, CompressLevel, ErrorCode, Protect, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zeroize::Zeroize;

/// Largest decoded image handled while recompressing or extracting.
const MAX_IMAGE_BYTES: usize = 600 << 20;
/// Most image files written for one page: a hostile page can reference any number of them.
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
    /// Images that were not touched because they are in a form that cannot be read back faithfully.
    pub images_skipped: u32,
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

/// How an image's samples are to be read, from its `/ColorSpace`.
#[derive(Debug, Clone)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// A palette: each sample is an index into `palette`, which holds `base` bytes per entry.
    Indexed {
        base: usize,
        palette: Vec<u8>,
    },
}

impl Space {
    fn channels(&self) -> usize {
        match self {
            Space::Gray | Space::Indexed { .. } => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }
}

/// Resolve a reference (to any depth that is sane) to the object it names.
fn deref<'a>(doc: &'a Document, mut obj: &'a Object) -> Option<&'a Object> {
    for _ in 0..8 {
        match obj {
            Object::Reference(id) => obj = doc.get_object(*id).ok()?,
            other => return Some(other),
        }
    }
    None
}

/// Read a `/ColorSpace` entry. Returns how to read the samples and, when the colour space can be kept
/// as written for the recompressed image (the plain device and calibrated spaces and ICC profiles of
/// 1 or 3 components), that entry, so colours do not shift.
fn read_space(doc: &Document, entry: &Object, depth: u32) -> Option<(Space, Option<Object>)> {
    if depth > 3 {
        return None;
    }
    let obj = deref(doc, entry)?;
    match obj {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Some((Space::Gray, Some(Object::Name(b"DeviceGray".to_vec())))),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Some((Space::Rgb, Some(Object::Name(b"DeviceRGB".to_vec())))),
            b"DeviceCMYK" | b"CMYK" => Some((Space::Cmyk, None)),
            _ => None,
        },
        Object::Array(items) => {
            let family = deref(doc, items.first()?)?.as_name().ok()?;
            match family {
                b"CalGray" => Some((Space::Gray, Some(obj.clone()))),
                b"CalRGB" => Some((Space::Rgb, Some(obj.clone()))),
                b"ICCBased" => {
                    let profile = deref(doc, items.get(1)?)?.as_stream().ok()?;
                    match profile.dict.get(b"N").ok()?.as_i64().ok()? {
                        1 => Some((Space::Gray, Some(obj.clone()))),
                        3 => Some((Space::Rgb, Some(obj.clone()))),
                        4 => Some((Space::Cmyk, None)),
                        _ => None,
                    }
                }
                b"Indexed" | b"I" => {
                    let (base, _) = read_space(doc, items.get(1)?, depth + 1)?;
                    let base_channels = match base {
                        Space::Gray => 1,
                        Space::Rgb => 3,
                        _ => return None,
                    };
                    let hival = usize::try_from(deref(doc, items.get(2)?)?.as_i64().ok()?).ok()?;
                    let palette = match deref(doc, items.get(3)?)? {
                        Object::String(bytes, _) => bytes.clone(),
                        Object::Stream(st) => st.decompressed_content().ok()?,
                        _ => return None,
                    };
                    if hival > 255 || palette.len() < (hival + 1) * base_channels {
                        return None;
                    }
                    Some((
                        Space::Indexed {
                            base: base_channels,
                            palette,
                        },
                        None,
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// An image read back as 8-bit gray or RGB, and the colour space to write it with when that can stay as it was.
struct Decoded {
    img: image::DynamicImage,
    keep_space: Option<Object>,
}

/// Samples to 8 bits: 8-bit ones as they are, 16-bit ones by their high byte.
fn eight_bit(raw: Vec<u8>, bpc: i64, samples: usize) -> Option<Vec<u8>> {
    match bpc {
        8 => (raw.len() == samples).then_some(raw),
        16 => (raw.len() == samples * 2).then(|| raw.as_chunks::<2>().0.iter().map(|c| c[0]).collect()),
        _ => None,
    }
}

/// CMYK to RGB with the plain device formula (no profile): good enough for a copy that is about
/// to be stored as a smaller JPEG.
fn cmyk_to_rgb(cmyk: &[u8], inverted: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(cmyk.len() / 4 * 3);
    for px in cmyk.as_chunks::<4>().0 {
        let v = |i: usize| u32::from(if inverted { 255 - px[i] } else { px[i] });
        let k = 255 - v(3);
        for c in 0..3 {
            out.push((((255 - v(c)) * k) / 255) as u8);
        }
    }
    out
}

/// Decode an image stream to 8-bit gray or RGB. Understood: JPEG (RGB, gray, CMYK), and flate or
/// raw samples of 8 or 16 bits in the device, calibrated, ICC-profiled, CMYK and palette colour
/// spaces. Left alone, because they are already small or cannot be read faithfully: stencil masks,
/// colour-key masks, odd `/Decode` arrays, CCITT, JBIG2, JPEG 2000, LZW and the rest.
fn decode(doc: &Document, id: ObjectId) -> Option<Decoded> {
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    let dict = &stream.dict;
    let has = |key: &[u8]| dict.get(key).is_ok();
    if has(b"Mask") || dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) {
        return None;
    }
    let (w, h) = (
        dict.get(b"Width").ok().and_then(|o| deref(doc, o))?.as_i64().ok()?,
        dict.get(b"Height").ok().and_then(|o| deref(doc, o))?.as_i64().ok()?,
    );
    if w <= 0 || h <= 0 || (w as u128) * (h as u128) * 3 > MAX_IMAGE_BYTES as u128 {
        return None;
    }
    let (space, keep) = dict.get(b"ColorSpace").ok().and_then(|cs| read_space(doc, cs, 0))?;
    // A `/Decode` array that flips samples is only understood for CMYK, where it is how Photoshop
    // writes inverted JPEGs.
    let inverted = match dict.get(b"Decode").ok().and_then(|d| deref(doc, d)) {
        None => false,
        Some(Object::Array(a)) => {
            let nums: Vec<f32> = a.iter().filter_map(|o| o.as_float().ok()).collect();
            let flips = nums.chunks(2).all(|p| p == [1.0, 0.0]);
            let identity = nums.chunks(2).all(|p| p == [0.0, 1.0]);
            if flips && space.channels() == 4 && nums.len() == 8 {
                true
            } else if identity {
                false
            } else {
                return None;
            }
        }
        Some(_) => return None,
    };
    let bpc = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(8);
    let filters = stream.filters().unwrap_or_default();
    let (width, height) = (w as u32, h as u32);
    let img = match filters.as_slice() {
        [b"DCTDecode"] => {
            // The dictionary's Width and Height are the file's claim; the JPEG header says what a
            // decoder will actually allocate. Check the header first, against the same pixel budget
            // as everything else, and drop images whose two disagree (SEC-LIMITS).
            use image::ImageDecoder;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_PAGE_SIDE);
            limits.max_image_height = Some(MAX_PAGE_SIDE);
            limits.max_alloc = Some(MAX_IMAGE_BYTES as u64);
            let mut reader =
                image::ImageReader::with_format(std::io::Cursor::new(&stream.content), image::ImageFormat::Jpeg);
            reader.limits(limits);
            let decoder = reader.into_decoder().ok()?;
            let (jw, jh) = decoder.dimensions();
            if i64::from(jw) != w || i64::from(jh) != h || u64::from(jw) * u64::from(jh) > MAX_PAGE_PIXELS {
                return None;
            }
            // CMYK JPEGs are left alone: the decoder hands back RGB made with its own idea of how
            // such files are inverted, which does not always match the file's (an inverted picture
            // is worse than a large one).
            if space.channels() == 4 {
                return None;
            }
            match image::DynamicImage::from_decoder(decoder).ok()? {
                rgb @ image::DynamicImage::ImageRgb8(_) => rgb,
                gray @ image::DynamicImage::ImageLuma8(_) => gray,
                _ => return None,
            }
        }
        [] | [b"FlateDecode"] => {
            let raw = match filters.is_empty() {
                true => stream.content.clone(),
                false => stream.decompressed_content_with_limit(MAX_IMAGE_BYTES).ok()?,
            };
            let pixels = width as usize * height as usize;
            match &space {
                Space::Gray => {
                    let data = eight_bit(raw, bpc, pixels)?;
                    image::GrayImage::from_raw(width, height, data).map(image::DynamicImage::ImageLuma8)?
                }
                Space::Rgb => {
                    let data = eight_bit(raw, bpc, pixels * 3)?;
                    image::RgbImage::from_raw(width, height, data).map(image::DynamicImage::ImageRgb8)?
                }
                Space::Cmyk => {
                    let data = eight_bit(raw, bpc, pixels * 4)?;
                    image::RgbImage::from_raw(width, height, cmyk_to_rgb(&data, inverted))
                        .map(image::DynamicImage::ImageRgb8)?
                }
                Space::Indexed { base, palette } => {
                    let indices = eight_bit(raw, bpc.min(8), pixels)?;
                    let mut data = Vec::with_capacity(pixels * base);
                    for &i in &indices {
                        let at = usize::from(i) * base;
                        data.extend_from_slice(palette.get(at..at + base)?);
                    }
                    if *base == 3 {
                        image::RgbImage::from_raw(width, height, data).map(image::DynamicImage::ImageRgb8)?
                    } else {
                        image::GrayImage::from_raw(width, height, data).map(image::DynamicImage::ImageLuma8)?
                    }
                }
            }
        }
        _ => return None,
    };
    // Only keep the original colour space when what we write has the channels it describes.
    let keep_space = keep.filter(|_| {
        matches!(
            (&space, &img),
            (Space::Gray, image::DynamicImage::ImageLuma8(_)) | (Space::Rgb, image::DynamicImage::ImageRgb8(_))
        )
    });
    Some(Decoded { img, keep_space })
}

/// Every image XObject a page draws, directly or through form XObjects (a form is how many programs
/// wrap a scan), each once.
fn page_image_ids(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    fn walk(
        doc: &Document,
        resources: &Dictionary,
        depth: u32,
        forms: &mut BTreeSet<ObjectId>,
        out: &mut Vec<ObjectId>,
    ) {
        let Some(Object::Dictionary(xobjects)) = resources.get(b"XObject").ok().and_then(|o| deref(doc, o)) else {
            return;
        };
        for entry in xobjects.iter().map(|(_, v)| v) {
            let Ok(id) = entry.as_reference() else { continue };
            let Some(stream) = doc.get_object(id).ok().and_then(|o| o.as_stream().ok()) else {
                continue;
            };
            match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                Ok(b"Image") => {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
                Ok(b"Form") if depth < 8 && forms.insert(id) => {
                    if let Some(Object::Dictionary(inner)) =
                        stream.dict.get(b"Resources").ok().and_then(|o| deref(doc, o))
                    {
                        walk(doc, inner, depth + 1, forms, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    let Ok((inline, ids)) = doc.get_page_resources(page_id) else {
        return out;
    };
    let resources = inline
        .cloned()
        .or_else(|| ids.iter().find_map(|id| doc.get_dictionary(*id).ok().cloned()));
    if let Some(resources) = resources {
        walk(doc, &resources, 0, &mut BTreeSet::new(), &mut out);
    }
    out
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
    let mut skipped = 0u32;

    if let Some((target_dpi, quality)) = level.image_target() {
        // An image may sit on several pages: size it for the page where it is sharpest.
        let mut candidates: BTreeMap<ObjectId, Candidate> = BTreeMap::new();
        for page_id in doc.page_iter().collect::<Vec<_>>() {
            let width_in = page_width_pt(&doc, page_id) / 72.0;
            for id in page_image_ids(&doc, page_id) {
                let Some(width) = doc
                    .get_object(id)
                    .ok()
                    .and_then(|o| o.as_stream().ok())
                    .and_then(|st| st.dict.get(b"Width").ok())
                    .and_then(|o| deref(&doc, o))
                    .and_then(|o| o.as_i64().ok())
                else {
                    continue;
                };
                let dpi = width as f32 / width_in;
                let entry = candidates.entry(id).or_insert(Candidate { id, dpi });
                entry.dpi = entry.dpi.min(dpi);
            }
        }
        for cand in candidates.into_values() {
            let Some(Decoded { img, keep_space }) = decode(&doc, cand.id) else {
                skipped += 1;
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
                keep_space.unwrap_or_else(|| {
                    Object::Name(if gray {
                        b"DeviceGray".to_vec()
                    } else {
                        b"DeviceRGB".to_vec()
                    })
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
        images_skipped: skipped,
    })
}

/// The protection [`encrypt`] applies, as shown to the user (SEC-HONESTY).
pub const ENCRYPTION_LABEL: &str = "AES-256";
/// With no open password anybody can read the file; the permission flags are a request that
/// well-behaved programs honour, not protection (SEC-HONESTY).
pub const ENCRYPTION_LABEL_NO_OPEN_PASSWORD: &str = "AES-256, no open password: restrictions only";

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
    Ok(match protect.user_pw.is_empty() {
        true => ENCRYPTION_LABEL_NO_OPEN_PASSWORD,
        false => ENCRYPTION_LABEL,
    }
    .to_string())
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
    let ids: Vec<ObjectId> = page_image_ids(&doc, page_id);
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
            let Some(Decoded { img, .. }) = decode(&doc, id) else {
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
