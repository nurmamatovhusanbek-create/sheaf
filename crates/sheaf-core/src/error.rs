//! One error type for the whole app. Every failure maps to a code and an
//! i18n key (EVT-ERROR, FLOW-ERROR); messages never carry document content.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    /// A command argument failed validation (SEC-IPC).
    InvalidArg,
    /// An id does not name anything in the workspace.
    NotFound,
    /// A path was rejected by the path policy (SEC-PATHS).
    PathRejected,
    /// A resource limit was exceeded (SEC-LIMITS).
    LimitExceeded,
    /// The file could not be parsed as the format it claims.
    ParseError,
    /// The file is encrypted and no (or a wrong) password was given.
    NeedsPassword,
    /// The file type is not supported.
    Unsupported,
    Io,
    /// The worker process died while running the job.
    WorkerCrashed,
    Timeout,
    Cancelled,
    /// A bundled component (PDFium, tesseract) is missing or failed its check.
    EngineMissing,
    /// A scanner (or its driver) reported a problem: offline, cover open, jam, driver error.
    DeviceError,
    /// The scanner is in use by something else (SCAN-FAILURES).
    DeviceBusy,
    /// The scanner's driver crashed, hung or broke the protocol; the helper process that
    /// hosts it was shut down and the app is unaffected (SCAN-ISOLATE).
    DeviceCrashed,
    /// The feeder is empty (at the start, or, mid-batch, simply the end).
    NoPaper,
    Internal,
}

impl ErrorCode {
    /// Key looked up in `ui/i18n/*.json`.
    pub fn i18n_key(self) -> &'static str {
        match self {
            ErrorCode::InvalidArg => "error.invalid_arg",
            ErrorCode::NotFound => "error.not_found",
            ErrorCode::PathRejected => "error.path_rejected",
            ErrorCode::LimitExceeded => "error.limit_exceeded",
            ErrorCode::ParseError => "error.parse",
            ErrorCode::NeedsPassword => "error.needs_password",
            ErrorCode::Unsupported => "error.unsupported",
            ErrorCode::Io => "error.io",
            ErrorCode::WorkerCrashed => "error.worker_crashed",
            ErrorCode::Timeout => "error.timeout",
            ErrorCode::Cancelled => "error.cancelled",
            ErrorCode::EngineMissing => "error.engine_missing",
            ErrorCode::DeviceError => "error.device",
            ErrorCode::DeviceBusy => "error.device_busy",
            ErrorCode::DeviceCrashed => "error.device_crashed",
            ErrorCode::NoPaper => "error.no_paper",
            ErrorCode::Internal => "error.internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppError {
    pub code: ErrorCode,
    /// Developer-facing detail. Must not contain document text or passwords (SEC-LOG).
    pub msg: String,
}

impl AppError {
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self { code, msg: msg.into() }
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArg, msg)
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, msg)
    }
    pub fn limit(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::LimitExceeded, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, msg)
    }
    /// I/O error without the path: full paths stay out of messages (SEC-LOG).
    pub fn io(what: &str, e: &std::io::Error) -> Self {
        Self::new(ErrorCode::Io, format!("{what}: {:?}", e.kind()))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.msg)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
