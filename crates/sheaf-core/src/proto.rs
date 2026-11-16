//! DATA-WORKERPROTO: newline-delimited JSON between MOD-JOBS and MOD-WORKER.
//!
//! No binary on the pipe. Paths that the worker *writes* are relative to the
//! session dir; inputs are absolute paths the main process has already passed
//! through the path policy (SEC-PATHS).

use crate::error::{AppError, ErrorCode, Result};
use crate::model::{Filter, NormRect, Rot, SourceKind};
use crate::tools::{CompressLevel, ImageFit, Password, Protect, Stamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: String,
    #[serde(default)]
    pub args: Value,
}

/// One line from the worker. Exactly one of `progress` / `ok` is present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Progress(f32),
    Done(Value),
    Failed(AppError),
}

impl Response {
    pub fn progress(id: u64, p: f32) -> Self {
        Self {
            id,
            progress: Some(p),
            ok: None,
            result: None,
            error: None,
        }
    }
    pub fn done(id: u64, result: Value) -> Self {
        Self {
            id,
            progress: None,
            ok: Some(true),
            result: Some(result),
            error: None,
        }
    }
    pub fn failed(id: u64, error: AppError) -> Self {
        Self {
            id,
            progress: None,
            ok: Some(false),
            result: None,
            error: Some(error),
        }
    }
    pub fn outcome(self) -> Outcome {
        match (self.ok, self.progress) {
            (Some(true), _) => Outcome::Done(self.result.unwrap_or(Value::Null)),
            (Some(false), _) => Outcome::Failed(
                self.error
                    .unwrap_or_else(|| AppError::new(ErrorCode::Internal, "worker sent no error body")),
            ),
            (None, Some(p)) => Outcome::Progress(p.clamp(0.0, 1.0)),
            (None, None) => Outcome::Failed(AppError::new(ErrorCode::Internal, "malformed worker line")),
        }
    }
}

/// Worker op names.
pub mod ops {
    pub const PING: &str = "ping";
    pub const IMPORT_PROBE: &str = "import_probe";
    pub const THUMB: &str = "thumb";
    pub const RENDER: &str = "render";
    pub const BUILD: &str = "build";
    pub const OCR_PAGE: &str = "ocr_page";
    pub const PAGE_TEXT: &str = "page_text";
    pub const PAGE_FACTS: &str = "page_facts";
    pub const EXPORT_IMAGES: &str = "export_images";
    pub const EXTRACT_IMAGES: &str = "extract_images";
    pub const REPAIR: &str = "repair";
    pub const METADATA: &str = "metadata";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Absolute, already validated by the main process.
    pub path: String,
    pub kind: SourceKind,
    /// Opens a locked PDF (CMD-UNLOCK-PW). Sent once per request, never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Password>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeArgs {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Password>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub kind: SourceKind,
    pub page_count: u32,
    pub encrypted: bool,
    /// The file opens without a password but an owner password restricts
    /// printing, copying or editing.
    #[serde(default)]
    pub restricted: bool,
    /// The document has fillable form fields. They do not survive page
    /// import as fields; see `PageSpec::flatten_forms` (RISK-FORMS-MERGE).
    #[serde(default)]
    pub has_forms: bool,
    /// Per page: does it carry a text layer? Empty when the file is locked.
    pub has_text: Vec<bool>,
    /// Per page: width and height in PDF points, as displayed.
    pub sizes_pt: Vec<(f32, f32)>,
}

/// What to draw: a source page (or a blank) plus the page's edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSpec {
    /// `None` is a blank page of `blank_pt` points.
    pub source: Option<SourceRef>,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub blank_pt: Option<(f32, f32)>,
    #[serde(default)]
    pub rot: Rot,
    #[serde(default)]
    pub crop: Option<NormRect>,
    #[serde(default)]
    pub filter: Filter,
    /// OCR cache key whose text layer is laid over the page at build time (OCR-TEXTLAYER).
    #[serde(default)]
    pub text_layer: Option<String>,
    /// Rasterize even an untouched PDF page. Used when existing text is
    /// replaced: the old text layer must not survive under the new one.
    #[serde(default)]
    pub rasterize: bool,
    /// Apply `crop` as a CropBox on an untouched PDF page instead of
    /// rasterizing: the rest of the page is hidden, not removed (TOOL-CROP).
    #[serde(default)]
    pub crop_hide_only: bool,
    /// Draw form fields into the page so their values stay visible after import.
    #[serde(default)]
    pub flatten_forms: bool,
}

/// `thumb` and `render`: draw one page into a PNG inside the session dir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderArgs {
    pub page: PageSpec,
    /// The image is scaled to fit inside this box, never enlarged past 300 dpi.
    pub max_px: (u32, u32),
    /// Session-relative output path.
    pub out: String,
}

/// What the page's filters did to this picture, so the window can say so instead of leaving a person
/// to squint at a result that may be unchanged because there was nothing to change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct FilterNote {
    /// Degrees the page was rotated to level the text; `Some(0.0)` when it checked and it was level.
    /// `None` when straightening is off.
    #[serde(default)]
    pub skew_deg: Option<f32>,
    /// Specks painted out; `Some(0)` when it looked and found none. `None` when cleaning is off.
    #[serde(default)]
    pub specks: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderResult {
    pub out: String,
    pub width: u32,
    pub height: u32,
    /// What the filters did, for the picture just made.
    #[serde(default)]
    pub filter: Option<FilterNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildArgs {
    pub pages: Vec<PageSpec>,
    /// Session-relative output path; the main process publishes it afterwards.
    pub out: String,
    #[serde(default)]
    pub stamps: Vec<Stamp>,
    #[serde(default)]
    pub image_fit: ImageFit,
    #[serde(default)]
    pub compress: Option<CompressLevel>,
    #[serde(default)]
    pub protect: Option<Protect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    pub out: String,
    pub page_count: u32,
    /// Pages copied from their source without re-encoding (PERF-LOSSLESS).
    pub copied: u32,
    /// Pages that had to be rasterized.
    pub rasterized: u32,
    /// Pages that received an OCR text layer.
    #[serde(default)]
    pub text_layers: u32,
    /// JPEG files embedded byte for byte.
    #[serde(default)]
    pub jpeg_passthrough: u32,
    #[serde(default)]
    pub stamps: u32,
    /// Pages whose form fields were drawn into the page.
    #[serde(default)]
    pub flattened: u32,
    /// Size of the finished file.
    #[serde(default)]
    pub bytes: u64,
    /// Size before compression, when compression was asked for.
    #[serde(default)]
    pub bytes_before_compress: Option<u64>,
    /// Compression could not make the file smaller, so the uncompressed
    /// result was kept (TOOL-COMPRESS: never return a bigger file).
    #[serde(default)]
    pub compress_kept_original: bool,
    /// What protection was actually applied, e.g. `AES-256` (SEC-HONESTY).
    #[serde(default)]
    pub encryption: Option<String>,
}

/// `ocr_page`: OCR-PIPELINE for one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrArgs {
    pub page: PageSpec,
    /// `None` detects script and picks languages (OCR-LANGS); a fixed list skips detection.
    #[serde(default)]
    pub langs: Option<Vec<String>>,
    /// Recognize even if the page already has a text layer ("redo all").
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrPageResult {
    /// The page already has text and `force` was off: nothing was done.
    pub skipped_has_text: bool,
    /// Cache key (DATA-CACHEKEY); `None` when skipped.
    pub key: Option<String>,
    /// The result came from the cache; the engine did not run.
    pub cached: bool,
    pub engine: String,
    pub langs: Vec<String>,
    pub mean_conf: f32,
    pub words: u32,
    /// Clockwise degrees the page should be turned to read upright; the main
    /// process records it as a Rotate op (OCR-PIPELINE step 4).
    pub rotate: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageTextArgs {
    pub path: String,
    pub index: u32,
    /// Only text inside this part of the page, as displayed.
    #[serde(default)]
    pub within: Option<NormRect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageTextResult {
    pub text: String,
}

/// `page_facts`: what the split planner needs to know about a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactsArgs {
    pub page: PageSpec,
    pub blank: bool,
    pub text: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FactsResult {
    pub blank: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpg,
}

/// `export_images` (TOOL-PDF2IMG): one image file per page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportImagesArgs {
    pub pages: Vec<PageSpec>,
    pub format: ImageFormat,
    pub dpi: u32,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    /// Session-relative directory, created by the worker.
    pub out_dir: String,
}

/// `extract_images` (TOOL-PDF2IMG, alternative mode): the images embedded in
/// one PDF page, unchanged where the format allows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractImagesArgs {
    pub source: SourceRef,
    pub index: u32,
    pub out_dir: String,
}

/// The document information fields people look at (TOOL-METADATA), as plain text.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetaFields {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
}

/// Longest value of one field, in characters.
pub const MAX_META_CHARS: usize = 1_000;

impl MetaFields {
    /// SEC-IPC: these come from a form (or a command line) and end up inside a file.
    pub fn validate(&self) -> Result<()> {
        for (what, v) in [
            ("title", &self.title),
            ("author", &self.author),
            ("subject", &self.subject),
            ("keywords", &self.keywords),
            ("creator", &self.creator),
            ("producer", &self.producer),
        ] {
            if v.chars().count() > MAX_META_CHARS || v.chars().any(char::is_control) {
                return Err(AppError::invalid(format!(
                    "the {what} is too long or has control characters"
                )));
            }
        }
        Ok(())
    }
}

/// What a file says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MetadataInfo {
    pub fields: MetaFields,
    /// The creation and modification dates exactly as the file spells them (`D:20260101120000Z`).
    pub created: String,
    pub modified: String,
    /// The file carries an XMP packet (a second copy of the same facts, and more).
    pub has_xmp: bool,
    pub pages: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetaEdit {
    /// Remove the information fields and the XMP packet.
    Strip,
    /// Write these fields to both places; an empty field is left out.
    Set(MetaFields),
}

/// `metadata` (TOOL-METADATA): read a PDF's information, or write a changed copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataArgs {
    /// Absolute, already validated by the main process.
    pub path: String,
    /// With `edit`: the session-relative file to write.
    #[serde(default)]
    pub out: Option<String>,
    #[serde(default)]
    pub edit: Option<MetaEdit>,
}

/// What the file says: the input when nothing was written, the new file when something was
/// (read back from it, so it is what is really in the file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataResult {
    pub info: MetadataInfo,
    pub out: Option<String>,
}

/// `repair` (TOOL-REPAIR): rebuild a damaged PDF's structure into a new file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepairArgs {
    /// Absolute, already validated by the main process.
    pub path: String,
    /// Session-relative output.
    pub out: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepairResult {
    pub out: String,
    /// Pages the repaired file has when opened again by the renderer: the honest count.
    pub pages: u32,
    /// The original did not open cleanly (or opened with a different number of pages).
    pub was_damaged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilesResult {
    /// Session-relative paths, in page order.
    pub files: Vec<String>,
    /// Images that could not be written in a common format and were left out.
    #[serde(default)]
    pub skipped: u32,
}
