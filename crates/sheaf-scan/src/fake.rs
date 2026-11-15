//! A scanner that feeds generated pages with delays (SCAN-BACKEND, TEST-SCAN).

use crate::{
    normalize, CancelToken, Device, ScanBackend, ScanColor, ScanProfile, ScanSource, ScannedPage, MAX_DEVICES,
};
use sheaf_core::{AppError, ErrorCode, Result};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// A scanner for tests and for running the app where there is none.
#[derive(Debug, Clone)]
pub struct FakeBackend {
    devices: Vec<Device>,
    /// Sheets in the feeder.
    sheets: usize,
    /// Time each page takes to "scan".
    delay: Duration,
    /// After this many pages the device fails (a paper jam).
    jam_after: Option<usize>,
    /// The device is in use by something else.
    busy: bool,
    /// Largest side of a generated page in pixels (the resolution is not simulated beyond this).
    max_side: u32,
}

impl Default for FakeBackend {
    fn default() -> Self {
        FakeBackend {
            devices: vec![Device {
                id: "fake:1".into(),
                name: "Sheaf test scanner".into(),
            }],
            sheets: 3,
            delay: Duration::from_millis(150),
            jam_after: None,
            busy: false,
            max_side: 900,
        }
    }
}

impl FakeBackend {
    pub fn new() -> FakeBackend {
        FakeBackend::default()
    }
    pub fn with_sheets(mut self, sheets: usize) -> FakeBackend {
        self.sheets = sheets;
        self
    }
    pub fn with_delay(mut self, delay: Duration) -> FakeBackend {
        self.delay = delay;
        self
    }
    pub fn with_jam_after(mut self, pages: usize) -> FakeBackend {
        self.jam_after = Some(pages);
        self
    }
    pub fn with_busy(mut self, busy: bool) -> FakeBackend {
        self.busy = busy;
        self
    }
    pub fn with_devices(mut self, devices: Vec<Device>) -> FakeBackend {
        self.devices = devices;
        self
    }

    /// Wait out the delay in small steps so a cancel is seen quickly.
    fn wait(&self, cancel: &CancelToken) -> bool {
        let mut left = self.delay;
        while !left.is_zero() {
            if cancel.is_cancelled() {
                return false;
            }
            let step = left.min(Duration::from_millis(10));
            std::thread::sleep(step);
            left -= step;
        }
        !cancel.is_cancelled()
    }

    /// A page with rows of "text" and its number in the corner, so pages differ
    /// and are not blank.
    fn page_image(&self, profile: &ScanProfile, number: usize) -> sheaf_image::RgbaImage {
        let (w, h) = profile.pixels().unwrap_or((1240, 1754));
        let scale = (self.max_side as f32 / w.max(h) as f32).min(1.0);
        let (w, h) = (((w as f32 * scale) as u32).max(64), ((h as f32 * scale) as u32).max(64));
        let mut img = sheaf_image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        let ink = match profile.color {
            ScanColor::Color => image::Rgba([20, 40, 120, 255]),
            _ => image::Rgba([0, 0, 0, 255]),
        };
        let line = (h / 40).max(2);
        let mut y = h / 10;
        let mut row = 0u32;
        while y + line < h - h / 10 {
            // Row lengths differ with the page number, a cheap fingerprint.
            let len = w / 2 + ((row * 37 + number as u32 * 101) % (w / 3).max(1));
            for yy in y..y + line {
                for x in w / 10..(w / 10 + len).min(w - w / 10) {
                    img.put_pixel(x, yy, ink);
                }
            }
            y += line * 2;
            row += 1;
        }
        // The page number as a block of that many squares.
        for n in 0..number.min(12) as u32 {
            for yy in 2..2 + line {
                for x in (4 + n * line * 2)..(4 + n * line * 2 + line) {
                    if x < w {
                        img.put_pixel(x, yy, ink);
                    }
                }
            }
        }
        img
    }
}

impl ScanBackend for FakeBackend {
    fn devices(&self) -> Result<Vec<Device>> {
        Ok(self.devices.iter().take(MAX_DEVICES).cloned().collect())
    }

    fn scan(&self, profile: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()> {
        profile.validate()?;
        if !self.devices.iter().any(|d| d.id == profile.device_id) {
            return Err(AppError::new(ErrorCode::DeviceError, "the scanner is not connected"));
        }
        if self.busy {
            return Err(AppError::new(ErrorCode::DeviceBusy, "the scanner is in use"));
        }
        let count = match profile.source {
            ScanSource::Flatbed => 1,
            ScanSource::Feeder if self.sheets == 0 => {
                return Err(AppError::new(ErrorCode::NoPaper, "the feeder is empty"));
            }
            ScanSource::Feeder => self.sheets * if profile.duplex { 2 } else { 1 },
        };
        for number in 1..=count {
            if !self.wait(&cancel) {
                return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
            }
            if self.jam_after.is_some_and(|n| number > n) {
                return Err(AppError::new(ErrorCode::DeviceError, "paper jam"));
            }
            let png = sheaf_image::encode_png(&self.page_image(profile, number))?;
            let (data, ext) = normalize(png)?;
            if pages.send(ScannedPage { number, data, ext }).is_err() {
                return Err(AppError::new(ErrorCode::Cancelled, "nobody is listening"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn quick() -> FakeBackend {
        FakeBackend::new().with_delay(Duration::from_millis(1))
    }

    fn profile(source: ScanSource, duplex: bool) -> ScanProfile {
        ScanProfile {
            source,
            duplex,
            dpi: 150,
            ..ScanProfile::new("fake:1")
        }
    }

    #[test]
    fn pages_arrive_in_order_one_for_the_flatbed_and_all_sheets_for_the_feeder() {
        let (tx, rx) = channel();
        quick()
            .scan(&profile(ScanSource::Flatbed, false), tx, CancelToken::new())
            .expect("flatbed");
        assert_eq!(rx.iter().map(|p| p.number).collect::<Vec<_>>(), [1]);

        let (tx, rx) = channel();
        quick()
            .with_sheets(4)
            .scan(&profile(ScanSource::Feeder, false), tx, CancelToken::new())
            .expect("feeder");
        let got: Vec<ScannedPage> = rx.iter().collect();
        assert_eq!(got.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(got.iter().all(|p| p.ext == "png" && p.data.starts_with(b"\x89PNG")));
        assert_ne!(got[0].data, got[1].data, "pages differ");

        let (tx, rx) = channel();
        quick()
            .with_sheets(2)
            .scan(&profile(ScanSource::Feeder, true), tx, CancelToken::new())
            .expect("duplex");
        assert_eq!(rx.iter().count(), 4, "both sides of two sheets");
    }

    #[test]
    fn cancel_stops_the_batch_but_keeps_the_pages_already_sent() {
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let backend = FakeBackend::new().with_sheets(50).with_delay(Duration::from_millis(20));
        let c = cancel.clone();
        let worker = std::thread::spawn(move || backend.scan(&profile(ScanSource::Feeder, false), tx, c));
        rx.recv().expect("first page");
        cancel.cancel();
        let err = worker.join().expect("join").expect_err("cancelled");
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert!(rx.iter().count() < 5, "stopped long before the 50th page");
    }

    #[test]
    fn failures_map_to_their_codes_and_keep_earlier_pages() {
        let run = |b: FakeBackend, p: ScanProfile| {
            let (tx, rx) = channel();
            let r = b.scan(&p, tx, CancelToken::new());
            (r, rx.iter().count())
        };
        let (r, got) = run(
            quick().with_sheets(5).with_jam_after(2),
            profile(ScanSource::Feeder, false),
        );
        assert_eq!(r.expect_err("jam").code, ErrorCode::DeviceError);
        assert_eq!(got, 2, "pages before the jam are kept");
        let (r, got) = run(quick().with_sheets(0), profile(ScanSource::Feeder, false));
        assert_eq!(r.expect_err("empty").code, ErrorCode::NoPaper);
        assert_eq!(got, 0);
        let (r, _) = run(quick().with_busy(true), profile(ScanSource::Flatbed, false));
        assert_eq!(r.expect_err("busy").code, ErrorCode::DeviceBusy);
        let mut unknown = profile(ScanSource::Flatbed, false);
        unknown.device_id = "nope".into();
        let (r, _) = run(quick(), unknown);
        assert_eq!(r.expect_err("unknown").code, ErrorCode::DeviceError);
        let mut bad = profile(ScanSource::Flatbed, false);
        bad.dpi = 7;
        let (r, _) = run(quick(), bad);
        assert_eq!(r.expect_err("invalid").code, ErrorCode::InvalidArg);
    }

    #[test]
    fn a_receiver_that_went_away_ends_the_scan() {
        let (tx, rx) = channel();
        drop(rx);
        let err = quick()
            .scan(&profile(ScanSource::Flatbed, false), tx, CancelToken::new())
            .expect_err("no listener");
        assert_eq!(err.code, ErrorCode::Cancelled);
    }
}
