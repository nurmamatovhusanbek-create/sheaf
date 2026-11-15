//! Headless harness: drives MOD-APP without a window, for tests, the command
//! line and the perf bench. The application core itself lives in `sheaf-app`;
//! this crate only adds what needs the development checkout (the vendored
//! engines next to the sources).

pub use sheaf_app::*;

/// The vendored engines of a development checkout, with the worker binary
/// that sits beside the running executable.
pub fn vendored() -> sheaf_core::Result<Engines> {
    Ok(Engines {
        worker: sibling_worker()?,
        pdfium_dir: fixture_gen::vendored_pdfium_dir(),
        tesseract: Some((fixture_gen::tesseract_exe(), fixture_gen::tessdata_dir())),
        font: Some(fixture_gen::noto_font()),
    })
}
