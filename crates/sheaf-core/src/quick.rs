//! Quick actions: what the action wheel and the command line can do to a set
//! of files with no window (ADR-WHEEL, CLI-GRAMMAR). Pure: no I/O.

use crate::tools::CompressLevel;
use std::path::{Path, PathBuf};

/// A quick action on files, as the command line or the wheel asks for it.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickRequest {
    pub action: QuickAction,
    pub files: Vec<PathBuf>,
    /// Where outputs go; `None` is beside the first input (ADR-NO-OVERWRITE).
    pub out: Option<PathBuf>,
    pub level: CompressLevel,
    /// OCR languages; `None` detects them (OCR-LANGS).
    pub langs: Option<Vec<String>>,
}

impl QuickRequest {
    pub fn new(action: QuickAction, files: Vec<PathBuf>) -> Self {
        QuickRequest {
            action,
            files,
            out: None,
            level: CompressLevel::Recommended,
            langs: None,
        }
    }
}

/// What a file is, judged by its extension alone. The file is not opened here
/// (the main process never parses documents, SEC-ISOLATE); a wrong guess just
/// makes the worker refuse it later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Pdf,
    Image,
    Other,
}

pub fn classify(path: &Path) -> FileKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => FileKind::Pdf,
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp" | "webp" => FileKind::Image,
        _ => FileKind::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickAction {
    Merge,
    Compress,
    Ocr,
    Split,
    PdfToImages,
    ImagesToPdf,
    Rotate,
}

impl QuickAction {
    pub const ALL: [QuickAction; 7] = [
        QuickAction::Merge,
        QuickAction::Compress,
        QuickAction::Ocr,
        QuickAction::Split,
        QuickAction::PdfToImages,
        QuickAction::ImagesToPdf,
        QuickAction::Rotate,
    ];

    /// The word used on the command line and as the i18n key suffix.
    pub fn id(self) -> &'static str {
        match self {
            QuickAction::Merge => "merge",
            QuickAction::Compress => "compress",
            QuickAction::Ocr => "ocr",
            QuickAction::Split => "split",
            QuickAction::PdfToImages => "pdf2img",
            QuickAction::ImagesToPdf => "img2pdf",
            QuickAction::Rotate => "rotate",
        }
    }

    pub fn parse(word: &str) -> Option<QuickAction> {
        QuickAction::ALL.into_iter().find(|a| a.id() == word)
    }

    /// Does the action make sense for exactly these files?
    pub fn applies_to(self, kinds: &[FileKind]) -> bool {
        use FileKind::*;
        let n = kinds.len();
        let all = |ok: &[FileKind]| n > 0 && kinds.iter().all(|k| ok.contains(k));
        match self {
            QuickAction::Merge => n >= 2 && all(&[Pdf, Image]),
            QuickAction::Compress | QuickAction::Split | QuickAction::PdfToImages => all(&[Pdf]),
            QuickAction::Ocr | QuickAction::Rotate => all(&[Pdf, Image]),
            QuickAction::ImagesToPdf => all(&[Image]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extensions_decide_the_kind_case_blind() {
        let k = |s: &str| classify(&PathBuf::from(s));
        assert_eq!(k("a.PDF"), FileKind::Pdf);
        assert_eq!(k("C:\\x\\scan.JpEg"), FileKind::Image);
        assert_eq!(k("notes.txt"), FileKind::Other);
        assert_eq!(k("noext"), FileKind::Other);
        assert_eq!(k("dir.pdf.exe"), FileKind::Other);
    }

    #[test]
    fn actions_apply_only_where_they_make_sense() {
        use FileKind::*;
        let ok = |a: QuickAction, k: &[FileKind]| a.applies_to(k);
        assert!(!ok(QuickAction::Merge, &[Pdf]), "merge needs two");
        assert!(ok(QuickAction::Merge, &[Pdf, Image]));
        assert!(ok(QuickAction::Compress, &[Pdf, Pdf]));
        assert!(!ok(QuickAction::Compress, &[Pdf, Image]));
        assert!(ok(QuickAction::ImagesToPdf, &[Image]));
        assert!(!ok(QuickAction::ImagesToPdf, &[Image, Pdf]));
        assert!(ok(QuickAction::Ocr, &[Image, Pdf]));
        assert!(!ok(QuickAction::Ocr, &[Other]));
        for a in QuickAction::ALL {
            assert!(!a.applies_to(&[]), "{a:?} needs at least one file");
            assert_eq!(QuickAction::parse(a.id()), Some(a));
        }
        assert_eq!(QuickAction::parse("shred"), None);
    }
}
