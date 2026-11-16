//! Test double for the scanner helper (TEST-SCAN): the real helper loop over
//! the fake scanner, plus a way to die the way drivers do. `SHEAF_FAKE_SCAN`
//! picks the behaviour: `ok` (default), `jam`, `crash` (exit with a Windows
//! heap-corruption code after two pages), `abort` (a hard abort after one page),
//! `hang` (stop answering after one page), `junk` (garbage on the pipe).
//! Never bundled.

use sheaf_core::Result;
use sheaf_scan::{CancelToken, FakeBackend, ScanBackend, ScanProfile, ScannedPage};
use std::io::Write;
use std::sync::mpsc::Sender;
use std::time::Duration;

struct Misbehaving(FakeBackend, String);

impl ScanBackend for Misbehaving {
    fn devices(&self) -> Result<Vec<sheaf_scan::Device>> {
        if self.1 == "crash" {
            std::process::exit(0xC000_0374u32 as i32);
        }
        self.0.devices()
    }

    fn meter(&self) -> Option<std::sync::Arc<sheaf_scan::ScanMeter>> {
        self.0.meter()
    }

    fn scan(&self, profile: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = self.0.clone();
        let (p, c) = (profile.clone(), cancel.clone());
        let feeder = std::thread::spawn(move || inner.scan(&p, tx, c));
        let mut count = 0;
        for page in rx {
            count += 1;
            let _ = pages.send(page);
            std::thread::sleep(Duration::from_millis(60)); // let the app read it before we misbehave
            match (self.1.as_str(), count) {
                ("crash", 2) => std::process::exit(0xC000_0374u32 as i32),
                ("abort", 1) => std::process::abort(),
                ("hang", 1) => loop {
                    std::thread::sleep(Duration::from_secs(60));
                },
                _ => {}
            }
        }
        feeder
            .join()
            .unwrap_or_else(|_| Err(sheaf_core::AppError::internal("fake panicked")))
    }
}

fn main() {
    let mode = std::env::var("SHEAF_FAKE_SCAN").unwrap_or_else(|_| "ok".into());
    // `SHEAF_FAKE_DELAY_MS` makes a page slow enough to watch; `SHEAF_FAKE_SILENT` is a driver that never reports progress.
    let delay = std::env::var("SHEAF_FAKE_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let mut fake = FakeBackend::new()
        .with_sheets(5)
        .with_delay(Duration::from_millis(delay));
    if std::env::var_os("SHEAF_FAKE_SILENT").is_some() {
        fake = fake.without_progress();
    }
    if mode == "jam" {
        fake = fake.with_jam_after(2);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if mode == "junk" && args.first().is_some_and(|a| a == "scan") {
        // Not through the helper loop, which owns the pipe: a page, then noise.
        let mut out = std::io::stdout().lock();
        let png = sheaf_image::encode_png(&sheaf_image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 9, 9, 255])))
            .unwrap_or_default();
        let mut frame = vec![b'p'];
        frame.extend(png);
        let _ = out.write_all(b"P");
        let _ = out.write_all(&(frame.len() as u32).to_le_bytes());
        let _ = out.write_all(&frame);
        let _ = out.write_all(b"\x07garbage");
        let _ = out.flush();
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    std::process::exit(sheaf_scan::run_helper(&args, &Misbehaving(fake, mode)));
}
