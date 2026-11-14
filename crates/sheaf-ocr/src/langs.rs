//! OCR-LANGS: which Tesseract models to use for a page.

use sheaf_core::{AppError, Result, Script};

/// Models bundled with the app, besides `osd`.
pub const BUNDLED: [&str; 4] = ["eng", "rus", "uzb", "uzb_cyrl"];

/// Default chooser. The mapping is a hypothesis to validate on real
/// documents (TEST-OCR).
pub fn choose_langs(script: Script) -> Vec<String> {
    let langs: &[&str] = match script {
        Script::Latin => &["uzb", "eng"],
        Script::Cyrillic => &["uzb_cyrl", "rus"],
        Script::Unknown => &BUNDLED,
    };
    langs.iter().map(|l| (*l).to_string()).collect()
}

/// Language names end up on a command line and in file names: accept only
/// what a Tesseract model name can look like.
pub fn validate(langs: &[String]) -> Result<()> {
    let ok = !langs.is_empty()
        && langs.len() <= 8
        && langs.iter().all(|l| {
            (2..=24).contains(&l.len())
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
    if ok {
        Ok(())
    } else {
        Err(AppError::invalid("invalid OCR language list"))
    }
}
