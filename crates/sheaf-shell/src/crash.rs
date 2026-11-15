//! A last-resort handler for panics (DOC-AGENT-RULES rule 4, SEC-LOG).
//!
//! The app is built not to panic across its boundaries, but if something does,
//! the person should see a plain message instead of a window that vanishes,
//! and nobody should be left guessing. The note written to disk names the
//! version, the thread and the place in our own source where it happened,
//! never the panic message: those can carry file names or text from a document.

#![allow(unsafe_code)]

use sheaf_ui::i18n::I18n;
use std::path::PathBuf;

fn note_path() -> PathBuf {
    sheaf_store::config_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("crash.txt")
}

/// What goes into the note: no payload, no paths of the user's files.
pub fn note_text(version: &str, thread: &str, location: Option<(&str, u32)>) -> String {
    let at = location.map_or("unknown".to_string(), |(file, line)| format!("{file}:{line}"));
    format!("Sheaf {version}\nthread: {thread}\nat: {at}\n")
}

pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let text = note_text(
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("unnamed"),
            info.location().map(|l| (l.file(), l.line())),
        );
        let path = note_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, text);
        let lang = sheaf_store::config_dir()
            .map(|d| sheaf_store::Settings::load(&d).language)
            .unwrap_or_else(|_| "en".into());
        let t = I18n::new(&lang);
        tell(
            &t.t("crash.title"),
            &t.tv("crash.body", &[("path", &path.display().to_string())]),
        );
    }));
}

#[cfg(windows)]
fn tell(title: &str, body: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (t, b) = (wide(title), wide(body));
    // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the call.
    unsafe {
        MessageBoxW(None, PCWSTR(b.as_ptr()), PCWSTR(t.as_ptr()), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
fn tell(title: &str, body: &str) {
    crate::say_err(&format!("{title}\n{body}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_has_a_place_and_no_message() {
        let t = note_text("0.0.0", "main", Some(("crates/sheaf-ui/src/tray.rs", 42)));
        assert_eq!(t, "Sheaf 0.0.0\nthread: main\nat: crates/sheaf-ui/src/tray.rs:42\n");
        assert!(note_text("1", "t", None).contains("at: unknown"));
    }
}
