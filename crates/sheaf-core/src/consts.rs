//! Constants shared by every module. Limits implement SEC-LIMITS.

/// Product name. "Sheaf" is a placeholder; this is the one place to rename it
/// (the bundle identifier in MOD-SHELL is the other).
pub const APP_NAME: &str = "Sheaf";

/// Undo and redo depth (DATA-WORKSPACE).
pub const MAX_UNDO: usize = 100;

/// Largest input file accepted (SEC-LIMITS).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Most pages a single source may contribute (SEC-LIMITS).
pub const MAX_SOURCE_PAGES: u32 = 20_000;
/// Most pages a workspace may hold (SEC-LIMITS).
pub const MAX_WORKSPACE_PAGES: usize = 50_000;
/// Largest decoded bitmap, in pixels, for one page (SEC-LIMITS, RISK-LARGE-SCANS).
/// A3 at 600 dpi is about 70 MP; this leaves headroom without admitting bombs.
pub const MAX_PAGE_PIXELS: u64 = 150_000_000;
/// Largest single side of a bitmap, in pixels (SEC-LIMITS).
pub const MAX_PAGE_SIDE: u32 = 30_000;

/// Default per-job timeout (SEC-LIMITS).
pub const DEFAULT_JOB_TIMEOUT_MS: u64 = 120_000;
/// Default per-worker resident-memory cap (SEC-LIMITS).
pub const DEFAULT_WORKER_RSS_MB: u64 = 2048;

/// Resolution used when a PDF page has to be rasterized (pixel edits, OCR input).
pub const RASTER_DPI: u32 = 300;
/// Longest side of a thumbnail, in pixels.
pub const THUMB_PX: u32 = 256;
