//! MOD-CORE: workspace model, ops, undo, errors, consts (no I/O).
//!
//! Pure logic: no file access, no threads. See `ARCHITECTURE.md`, anchor `mod-core`.

pub mod consts;
pub mod error;
pub mod job;
pub mod model;
pub mod ocr;
pub mod op;
pub mod proto;
pub mod tools;
pub mod workspace;

pub use consts::APP_NAME;
pub use error::{AppError, ErrorCode, Result};
pub use job::{JobId, JobState, Limits, Priority};
pub use model::{
    ColorMode, Deskew, Filter, NormRect, OcrState, Page, PageContent, PageId, Rot, Source, SourceId, SourceKind,
    Threshold,
};
pub use ocr::{OcrResult, Script, Word};
pub use op::{NewPage, Op};
pub use tools::{Anchor, CompressLevel, ImageFit, NameContext, PageFacts, Password, Protect, SplitRule, Stamp};
pub use workspace::Workspace;
