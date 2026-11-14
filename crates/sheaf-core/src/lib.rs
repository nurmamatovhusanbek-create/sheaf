//! MOD-CORE: workspace model, ops, undo, errors, consts (no I/O).
//!
//! Pure logic: no file access, no threads. See `ARCHITECTURE.md`, anchor `mod-core`.

pub mod consts;
pub mod error;
pub mod job;
pub mod model;
pub mod op;
pub mod proto;
pub mod workspace;

pub use consts::APP_NAME;
pub use error::{AppError, ErrorCode, Result};
pub use job::{JobId, JobState, Limits, Priority};
pub use model::{
    ColorMode, Deskew, Filter, NormRect, OcrState, Page, PageContent, PageId, Rot, Source,
    SourceId, SourceKind, Threshold,
};
pub use op::{NewPage, Op};
pub use workspace::Workspace;
