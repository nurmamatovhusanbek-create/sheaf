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
    pdf(&(1..=n)
        .map(|i| PageDef::text(format!("Page {i}")))
        .collect::<Vec<_>>())
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
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + 2 * i))
        .collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(
        format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            pages.len(),
            kids.join(" ")
        )
        .into_bytes(),
    );
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for (i, page) in pages.iter().enumerate() {
        let (w, h) = page.size;
        let content = match &page.text {
            Some(t) => {
                let escaped = t
                    .replace('\\', "\\\\")
                    .replace('(', "\\(")
                    .replace(')', "\\)");
                format!(
                    "BT /F1 24 Tf 72 {} Td ({escaped}) Tj ET",
                    h.saturating_sub(100)
                )
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
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .into_bytes(),
        );
    }
    if locked.is_some() {
        let key = "ab".repeat(32);
        objects.push(
            format!("<< /Filter /Standard /V 1 /R 2 /O <{key}> /U <{key}> /P -4 >>").into_bytes(),
        );
    }

    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    let encrypt = match locked {
        Some(()) => format!(
            " /Encrypt {} 0 R /ID [<{id}> <{id}>]",
            objects.len(),
            id = "0123456789abcdef".repeat(2)
        ),
        None => String::new(),
    };
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R{encrypt} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
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
    let platform = if cfg!(windows) {
        "win-x64"
    } else {
        "linux-x64"
    };
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
        Scratch(dir.canonicalize().unwrap_or(dir))
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
