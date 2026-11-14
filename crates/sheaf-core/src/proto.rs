//! DATA-WORKERPROTO: newline-delimited JSON between MOD-JOBS and MOD-WORKER.
//!
//! No binary on the pipe. Paths that the worker *writes* are relative to the
//! session dir; inputs are absolute paths the main process has already passed
//! through the path policy (SEC-PATHS).

use crate::error::{AppError, ErrorCode};
use crate::model::{Filter, NormRect, Rot, SourceKind};
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
            (Some(false), _) => Outcome::Failed(self.error.unwrap_or_else(|| {
                AppError::new(ErrorCode::Internal, "worker sent no error body")
            })),
            (None, Some(p)) => Outcome::Progress(p.clamp(0.0, 1.0)),
            (None, None) => {
                Outcome::Failed(AppError::new(ErrorCode::Internal, "malformed worker line"))
            }
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Absolute, already validated by the main process.
    pub path: String,
    pub kind: SourceKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeArgs {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub kind: SourceKind,
    pub page_count: u32,
    pub encrypted: bool,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderResult {
    pub out: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildArgs {
    pub pages: Vec<PageSpec>,
    /// Session-relative output path; the main process publishes it afterwards.
    pub out: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    pub out: String,
    pub page_count: u32,
    /// Pages copied from their source without re-encoding (PERF-LOSSLESS).
    pub copied: u32,
    /// Pages that had to be rasterized.
    pub rasterized: u32,
}
