use sheaf_core::{OcrResult, Result, Script};
use std::path::{Path, PathBuf};

/// Orientation and script detection (OCR-PIPELINE step 4).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Osd {
    /// Clockwise degrees to turn the image so the text reads upright;
    /// 0 when detection was not confident.
    pub rotate: u16,
    pub script: Script,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecOpts {
    pub langs: Vec<String>,
    pub dpi: u32,
    /// Where to write a text-only PDF of the page (invisible text, no image),
    /// if the engine can produce one.
    pub text_pdf: Option<PathBuf>,
    /// Where to keep the engine's raw word table (OCR-CACHE), if wanted.
    pub tsv: Option<PathBuf>,
}

/// ADR-TESS-SIDECAR: engines sit behind this trait so another one can be
/// added without touching the pipeline (OCR-ENGINES).
pub trait OcrEngine {
    fn id(&self) -> &str;
    /// Part of the cache key: a new engine version invalidates old results.
    fn version(&self) -> String;
    fn detect(&self, img: &Path) -> Result<Osd>;
    fn recognize(&self, img: &Path, opts: &RecOpts) -> Result<OcrResult>;
}
