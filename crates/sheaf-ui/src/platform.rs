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

/// Save `text` as a file the person names (Save dialog). Returns where it went. Blocks; call from a
/// helper thread. Without Windows there is no dialog: the file goes to the temporary folder, which is
/// what the development checks use.
#[cfg(windows)]
pub fn save_text(name: &str, text: &str) -> Option<PathBuf> {
    let path = rfd::FileDialog::new()
        .set_file_name(name)
        .add_filter("Text", &["txt"])
        .save_file()?;
    std::fs::write(&path, text).ok()?;
    Some(path)
}

#[cfg(not(windows))]
pub fn save_text(name: &str, text: &str) -> Option<PathBuf> {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// Show a text file (the licences) in Notepad.
#[cfg(windows)]
pub fn open_text_file(path: &Path) -> Result<(), AppError> {
    std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::io("start Notepad", &e))
}

#[cfg(not(windows))]
pub fn open_text_file(_path: &Path) -> Result<(), AppError> {
    Err(AppError::new(sheaf_core::ErrorCode::Unsupported, "needs Windows"))
}

/// Start another copy of this program with `args` (`tray`, `install-verbs`). With `wait` it returns
/// whether that copy succeeded; without, whether it started.
#[cfg(windows)]
pub fn run_self(args: &[&str], wait: bool) -> bool {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut cmd = std::process::Command::new(exe);
    // CREATE_NO_WINDOW | DETACHED_PROCESS are not combinable; a helper that lives on is detached.
    cmd.args(args)
        .creation_flags(if wait { 0x0800_0000 } else { 0x0000_0008 });
    if wait {
        cmd.status().is_ok_and(|s| s.success())
    } else {
        cmd.spawn().is_ok()
    }
}

#[cfg(not(windows))]
pub fn run_self(_args: &[&str], _wait: bool) -> bool {
    false
}
