//! MOD-SCAN: the scanner contract, a fake scanner for tests, and the WIA backend.
//!
//! Everything above the scanner (the app's scan job, the dialog) talks to the
//! [`ScanBackend`] trait only, so it is tested on any machine with
//! [`FakeBackend`]. [`WiaBackend`] (Windows) runs COM on its own
//! single-threaded-apartment thread (SCAN-THREADING). Pages leave a backend as
//! encoded PNG or JPEG bytes; the app writes them to the session folder and
//! imports them like any other file, so a hostile driver gets no further than
//! a hostile image file would (SEC-LIMITS). See `ARCHITECTURE.md`, anchor `mod-scan`.

mod fake;
#[cfg_attr(not(windows), allow(dead_code))]
mod pipeline;
mod proc;
#[cfg(windows)]
mod wia;

pub use fake::FakeBackend;
pub use proc::{run_helper, ProcessBackend, HELPER_FLAG};
#[cfg(windows)]
pub use wia::WiaBackend;

use sheaf_core::{AppError, ErrorCode, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Most bytes one page may have (SEC-LIMITS): a 600 dpi A3 colour page is about 140 MB raw.
pub const MAX_PAGE_BYTES: usize = 256 * 1024 * 1024;
/// Most devices listed; anything beyond is ignored.
pub const MAX_DEVICES: usize = 32;
/// The resolutions the dialog offers.
pub const DPIS: [u32; 4] = [150, 200, 300, 600];

/// One scanner as the system lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Opaque id the backend understands; passed back in the profile.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanSource {
    Flatbed,
    Feeder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanColor {
    Color,
    Gray,
    /// Black and white (a threshold picture).
    Bw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPaper {
    A4,
    A3,
    Letter,
    /// Whatever the device detects.
    Auto,
}

impl ScanPaper {
    /// Width and height in millimetres, `None` for [`ScanPaper::Auto`].
    pub fn size_mm(self) -> Option<(f32, f32)> {
        match self {
            ScanPaper::A4 => Some((210.0, 297.0)),
            ScanPaper::A3 => Some((297.0, 420.0)),
            ScanPaper::Letter => Some((215.9, 279.4)),
            ScanPaper::Auto => None,
        }
    }
}

/// DATA-SCANPROFILE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProfile {
    pub device_id: String,
    pub source: ScanSource,
    /// Both sides of each sheet; only meaningful with the feeder.
    pub duplex: bool,
    pub dpi: u32,
    pub color: ScanColor,
    pub paper: ScanPaper,
}

impl ScanProfile {
    pub fn new(device_id: impl Into<String>) -> ScanProfile {
        ScanProfile {
            device_id: device_id.into(),
            source: ScanSource::Flatbed,
            duplex: false,
            dpi: 300,
            color: ScanColor::Color,
            paper: ScanPaper::A4,
        }
    }

    /// SEC-IPC: the dialog's values are checked again here, because a profile
    /// may also come from a saved preset someday.
    pub fn validate(&self) -> Result<()> {
        let id_ok =
            !self.device_id.is_empty() && self.device_id.len() <= 512 && !self.device_id.chars().any(char::is_control);
        if !id_ok {
            return Err(AppError::invalid("bad scanner id"));
        }
        if !DPIS.contains(&self.dpi) {
            return Err(AppError::invalid("unsupported scan resolution"));
        }
        if self.duplex && self.source == ScanSource::Flatbed {
            return Err(AppError::invalid("two-sided scanning needs the feeder"));
        }
        Ok(())
    }

    /// Pixels one page should have at this resolution, when the paper is known.
    pub fn pixels(&self) -> Option<(u32, u32)> {
        let (w, h) = self.paper.size_mm()?;
        let px = |mm: f32| ((mm / 25.4) * self.dpi as f32).round() as u32;
        Some((px(w), px(h)))
    }
}

/// One scanned page, as an image file in memory.
#[derive(Debug, Clone)]
pub struct ScannedPage {
    /// 1-based position in this scan.
    pub number: usize,
    /// PNG or JPEG bytes (see [`normalize`]).
    pub data: Vec<u8>,
    /// `"png"` or `"jpg"`.
    pub ext: &'static str,
}

/// Stops a scan: checked between pages and, where the driver allows, during one.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Everything above the scanner goes through this (SCAN-BACKEND).
pub trait ScanBackend: Send + Sync {
    /// Local scanners only: the app opens no network connections (SEC-NONET).
    fn devices(&self) -> Result<Vec<Device>>;

    /// Scan until the feeder is empty (or one page from the flatbed), sending
    /// each page as it arrives. Returns `Ok` after the last page and
    /// `Err(Cancelled)` when `cancel` was set. Pages already sent stay sent.
    /// A receiver that has gone away counts as a cancel.
    fn scan(&self, profile: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()>;
}

/// Turn what a driver delivered into PNG or JPEG bytes, or refuse it. BMP and
/// TIFF (what drivers often give for memory transfers) are decoded and written
/// as PNG; a file that is none of these is an error, not something to pass on.
pub fn normalize(data: Vec<u8>) -> Result<(Vec<u8>, &'static str)> {
    if data.is_empty() || data.len() > MAX_PAGE_BYTES {
        return Err(AppError::new(
            ErrorCode::DeviceError,
            "the scanner sent a page of a bad size",
        ));
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok((data, "png"));
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok((data, "jpg"));
    }
    let img = sheaf_image::decode_bytes(&data)
        .map_err(|_| AppError::new(ErrorCode::DeviceError, "the scanner sent a page that is not an image"))?;
    Ok((sheaf_image::encode_png(&img)?, "png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_checked() {
        let mut p = ScanProfile::new("dev1");
        assert!(p.validate().is_ok());
        p.dpi = 123;
        assert!(p.validate().is_err());
        p.dpi = 300;
        p.duplex = true;
        assert!(p.validate().is_err(), "duplex needs the feeder");
        p.source = ScanSource::Feeder;
        assert!(p.validate().is_ok());
        p.device_id = "bad\nid".into();
        assert!(p.validate().is_err());
        p.device_id = String::new();
        assert!(p.validate().is_err());
    }

    #[test]
    fn page_sizes_follow_the_paper_and_the_resolution() {
        let mut p = ScanProfile::new("d");
        assert_eq!(p.pixels(), Some((2480, 3508)), "A4 at 300 dpi");
        p.dpi = 150;
        assert_eq!(p.pixels(), Some((1240, 1754)));
        p.paper = ScanPaper::Auto;
        assert_eq!(p.pixels(), None);
    }

    #[test]
    fn normalize_passes_png_and_jpeg_and_converts_or_refuses_the_rest() {
        let img = sheaf_image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        let png = sheaf_image::encode_png(&img).expect("png");
        assert_eq!(normalize(png.clone()).expect("png").1, "png");
        let jpg = sheaf_image::encode_jpeg(&img, 80).expect("jpg");
        assert_eq!(normalize(jpg).expect("jpg").1, "jpg");
        // A bitmap is decoded and re-encoded as PNG.
        let mut bmp = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .expect("bmp");
        let (out, ext) = normalize(bmp).expect("bmp");
        assert_eq!(ext, "png");
        assert!(out.starts_with(b"\x89PNG"));
        assert_eq!(
            normalize(b"not an image".to_vec()).expect_err("junk").code,
            ErrorCode::DeviceError
        );
        assert_eq!(normalize(Vec::new()).expect_err("empty").code, ErrorCode::DeviceError);
    }
}
