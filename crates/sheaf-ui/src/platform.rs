//! The few things that are different per operating system: native file
//! dialogs and showing a file in Explorer. The app ships for Windows only
//! (NG-XPLAT); elsewhere these do nothing so the crate still builds for tests.

use sheaf_core::AppError;
use std::path::{Path, PathBuf};

const DOCUMENTS: &[&str] = &["pdf", "jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

/// The Open dialog. Blocks; call from a helper thread.
#[cfg(windows)]
pub fn pick_files() -> Vec<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("PDF, images", DOCUMENTS)
        .pick_files()
        .unwrap_or_default()
}

#[cfg(not(windows))]
pub fn pick_files() -> Vec<PathBuf> {
    let _ = DOCUMENTS;
    Vec::new()
}

#[cfg(windows)]
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[cfg(not(windows))]
pub fn pick_folder() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
pub fn pick_image() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("PNG, JPEG", &["png", "jpg", "jpeg"])
        .pick_file()
}

#[cfg(not(windows))]
pub fn pick_image() -> Option<PathBuf> {
    None
}

/// Show a file this app produced in Explorer (CMD-REVEAL; the caller only
/// passes paths a job reported).
#[cfg(windows)]
pub fn reveal(path: &Path) -> Result<(), AppError> {
    use std::os::windows::process::CommandExt;
    // Explorer wants `/select,"<path>"` as one argument with its own quotes;
    // a Windows path cannot contain a quote.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::io("start Explorer", &e))
}

#[cfg(not(windows))]
pub fn reveal(_path: &Path) -> Result<(), AppError> {
    Err(AppError::new(
        sheaf_core::ErrorCode::Unsupported,
        "Reveal needs Windows Explorer",
    ))
}
