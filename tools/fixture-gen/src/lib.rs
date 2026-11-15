//! Synthetic fixtures (TEST-FIXTURES). Everything is generated in code so no
//! real document ever lands in the repository. Dev-dependency only.

use std::path::PathBuf;

/// One page of a generated PDF.
#[derive(Debug, Clone)]
pub struct PageDef {
    /// Text drawn on the page; `None` draws a filled box instead, which gives
    /// a page without a text layer (stands in for a scan).
    pub text: Option<String>,
    /// MediaBox width and height in points.
    pub size: (u32, u32),
    /// The page's own `/Rotate` entry.
    pub rotate: u16,
}

impl PageDef {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            size: (595, 842),
            rotate: 0,
        }
    }
    pub fn no_text() -> Self {
        Self {
            text: None,
            size: (595, 842),
            rotate: 0,
        }
    }
    pub fn sized(mut self, w: u32, h: u32) -> Self {
        self.size = (w, h);
        self
    }
    pub fn rotated(mut self, degrees: u16) -> Self {
        self.rotate = degrees;
        self
    }
}

/// A born-digital PDF with `n` A4 pages reading "Page 1" … "Page n".
pub fn text_pdf(n: u32) -> Vec<u8> {
    pdf(&(1..=n).map(|i| PageDef::text(format!("Page {i}"))).collect::<Vec<_>>())
}

/// A minimal, valid PDF 1.4 file built by hand.
pub fn pdf(pages: &[PageDef]) -> Vec<u8> {
    build(pages, None)
}

/// A PDF whose trailer declares standard encryption with keys that match no
/// password: opening it must fail with a password error. (Fixtures that open
/// with a known password arrive with TOOL-PROTECT in PH-3.)
pub fn locked_pdf() -> Vec<u8> {
    build(&[PageDef::text("secret")], Some(()))
}

fn build(pages: &[PageDef], locked: Option<()>) -> Vec<u8> {
    // Object numbers: 1 catalog, 2 page tree, 3 font, then (page, content) pairs.
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(format!("<< /Type /Pages /Count {} /Kids [{}] >>", pages.len(), kids.join(" ")).into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for (i, page) in pages.iter().enumerate() {
        let (w, h) = page.size;
        let content = match &page.text {
            Some(t) => {
                let escaped = t.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
                format!("BT /F1 24 Tf 72 {} Td ({escaped}) Tj ET", h.saturating_sub(100))
            }
            None => format!("0.2 g {} {} {} {} re f", w / 4, h / 4, w / 2, h / 2),
        };
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Rotate {} \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                page.rotate,
                5 + 2 * i
            )
            .into_bytes(),
        );
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes());
    }
    if locked.is_some() {
        let key = "ab".repeat(32);
        objects.push(format!("<< /Filter /Standard /V 1 /R 2 /O <{key}> /U <{key}> /P -4 >>").into_bytes());
    }

    let encrypt = match locked {
        Some(()) => {
            format!(
                " /Encrypt {} 0 R /ID [<{id}> <{id}>]",
                objects.len(),
                id = "0123456789abcdef".repeat(2)
            )
        }
        None => String::new(),
    };
    write_objects(&objects, &encrypt)
}

/// Serialize numbered objects (1-based, object 1 is the catalog) with a classic xref table.
fn write_objects(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R{trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A one-page PDF with a filled-in text field (AcroForm): the page itself
/// says "Application form", the field shows "Filled value" through its
/// appearance stream.
pub fn form_pdf() -> Vec<u8> {
    let stream = |dict: &str, content: &str| {
        format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes()
    };
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /DA (/F1 12 Tf 0 g) /DR << /Font << /F1 3 0 R >> >> >> >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [4 0 R] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R /Annots [6 0 R] >>".to_vec(),
        stream("", "BT /F1 24 Tf 72 742 Td (Application form) Tj ET"),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (applicant) /V (Filled value) /Rect [72 600 300 630] /P 4 0 R /F 4 /DA (/F1 12 Tf 0 g) /AP << /N 7 0 R >> >>".to_vec(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 228 30] /Resources << /Font << /F1 3 0 R >> >>",
            "BT /F1 12 Tf 2 10 Td (Filled value) Tj ET",
        ),
    ];
    write_objects(&objects, "")
}

/// Bytes that claim to be a PDF and are not.
pub fn corrupt_pdf() -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend((0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
    bytes
}

/// Directory holding the vendored PDFium library for this platform
/// (`bash tools/fetch-vendor` puts it there).
pub fn vendored_pdfium_dir() -> PathBuf {
    let platform = if cfg!(windows) { "win-x64" } else { "linux-x64" };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/pdfium")
        .join(platform)
}

/// A fresh, empty directory under the system temp dir; removed on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sheaf-test-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        // Canonical, but in the plain form on Windows (`C:\x`, not `\\?\C:\x`), the way the app writes paths.
        let canon = dir.canonicalize().unwrap_or(dir);
        let plain = canon
            .to_str()
            .and_then(|s| s.strip_prefix(r"\\?\"))
            .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
            .map(std::path::PathBuf::from);
        Scratch(plain.unwrap_or(canon))
    }
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
    /// Write `bytes` to `name` inside the directory and return the path.
    pub fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
        p
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The Tesseract executable to test with: the vendored build on Windows, the
/// system one elsewhere (CI installs the distribution package).
pub fn tesseract_exe() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/tesseract/win-x64/tesseract.exe")
    } else {
        PathBuf::from("tesseract")
    }
}

/// Vendored language models (`eng`, `rus`, `uzb`, `uzb_cyrl`, `osd`) and `pdf.ttf`.
pub fn tessdata_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/tessdata")
}

/// Vendored Noto Sans Regular: covers Latin, Cyrillic and the Uzbek letters.
pub fn noto_font() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/fonts/NotoSans-Regular.ttf")
}

/// Text as compared for OCR accuracy: whitespace runs collapsed, the
/// apostrophe variants used for Uzbek oʻ/gʻ unified.
pub fn normalize_text(s: &str) -> String {
    let unified: String = s.chars().map(|c| if "ʻʼ’‘`´".contains(c) { '\'' } else { c }).collect();
    unified.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Character error rate: edit distance between the normalized texts, divided
/// by the length of the reference.
pub fn char_error_rate(reference: &str, recognized: &str) -> f64 {
    let a: Vec<char> = normalize_text(reference).chars().collect();
    let b: Vec<char> = normalize_text(recognized).chars().collect();
    if a.is_empty() {
        return if b.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != cb);
            row.push(substitute.min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()] as f64 / a.len() as f64
}

/// Insert an EXIF segment carrying only an orientation tag (1 to 8) into a JPEG.
pub fn jpeg_with_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let tiff: [u8; 26] = [
        b'M',
        b'M',
        0,
        42,
        0,
        0,
        0,
        8, // big-endian TIFF header, IFD at 8
        0,
        1, // one entry
        0x01,
        0x12,
        0,
        3,
        0,
        0,
        0,
        1,
        0,
        orientation,
        0,
        0, // Orientation, SHORT, count 1
        0,
        0,
        0,
        0, // no next IFD
    ];
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cer_counts_edits_against_the_reference() {
        assert_eq!(char_error_rate("abcd", "abcd"), 0.0);
        assert_eq!(char_error_rate("abcd", "abxd"), 0.25);
        assert_eq!(char_error_rate("abcd", "abd"), 0.25);
        assert_eq!(char_error_rate("abcd", ""), 1.0);
        assert_eq!(char_error_rate("a  b\n c", "a b c"), 0.0);
        assert_eq!(char_error_rate("oʻzbek", "o'zbek"), 0.0);
        assert_eq!(char_error_rate("", ""), 0.0);
    }
}
