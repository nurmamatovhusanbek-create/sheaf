//! CMD-SCAN-DEVICES and CMD-SCAN-START (cancelled through CMD-JOB-CANCEL):
//! FLOW-SCAN-LIVE. A scan is an app job. The scanner runs on its own thread
//! and sends pages down a channel; each page is written to the session folder
//! and imported like any other file, so it appears in the tray at once, and
//! (when asked) its recognition is queued while the next page is still
//! scanning. Scanning never waits for recognition. Pages that arrived before a
//! cancel or a failure are kept (SCAN-FAILURES).

use super::*;
use sheaf_scan::{CancelToken, ScanBackend, ScanColor, ScanPaper, ScanProfile, ScanSource, ScannedPage};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// Most pages one scan job keeps (SEC-LIMITS); the scan stops with a limit error after that.
pub const MAX_SCAN_PAGES: usize = 500;

/// The resolutions the Scan dialog offers.
pub const SCAN_DPIS: [u32; 4] = sheaf_scan::DPIS;

/// One scanner in CMD-SCAN-DEVICES.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDeviceDto {
    pub id: String,
    pub name: String,
}

/// CMD-SCAN-START: DATA-SCANPROFILE as the dialog sends it. Words, not
/// numbers, for the closed sets (SEC-IPC); anything else is refused.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScanProfileDto {
    pub device_id: String,
    /// `flatbed` or `feeder`.
    pub source: String,
    pub duplex: bool,
    /// 150, 200, 300 or 600.
    pub dpi: u32,
    /// `color`, `gray` or `bw`.
    pub color: String,
    /// `a4`, `a3`, `letter` or `auto`.
    pub paper: String,
    /// Queue recognition for each page as it arrives. `None` follows the OCR setting (Auto).
    pub ocr: Option<bool>,
}

impl Default for ScanProfileDto {
    fn default() -> Self {
        ScanProfileDto {
            device_id: String::new(),
            source: "flatbed".into(),
            duplex: false,
            dpi: 300,
            color: "color".into(),
            paper: "a4".into(),
            ocr: None,
        }
    }
}

impl ScanProfileDto {
    pub fn to_profile(&self) -> Result<ScanProfile> {
        let source = match self.source.as_str() {
            "flatbed" => ScanSource::Flatbed,
            "feeder" => ScanSource::Feeder,
            _ => return Err(AppError::invalid("unknown scan source")),
        };
        let color = match self.color.as_str() {
            "color" => ScanColor::Color,
            "gray" => ScanColor::Gray,
            "bw" => ScanColor::Bw,
            _ => return Err(AppError::invalid("unknown scan colour mode")),
        };
        let paper = match self.paper.as_str() {
            "a4" => ScanPaper::A4,
            "a3" => ScanPaper::A3,
            "letter" => ScanPaper::Letter,
            "auto" => ScanPaper::Auto,
            _ => return Err(AppError::invalid("unknown paper size")),
        };
        let profile = ScanProfile {
            device_id: self.device_id.clone(),
            source,
            duplex: self.duplex,
            dpi: self.dpi,
            color,
            paper,
        };
        profile.validate()?;
        Ok(profile)
    }
}

/// The scanner a fresh backend starts with: WIA on Windows. Elsewhere there
/// is none, except a fake one when `SHEAF_FAKE_SCANNER` is set, which lets the
/// dialog be tried on a development machine.
pub(super) fn default_scanner() -> Option<Arc<dyn ScanBackend>> {
    #[cfg(windows)]
    {
        Some(Arc::new(sheaf_scan::WiaBackend))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("SHEAF_FAKE_SCANNER")
            .filter(|v| !v.is_empty())
            .map(|_| Arc::new(sheaf_scan::FakeBackend::new()) as Arc<dyn ScanBackend>)
    }
}

impl Backend {
    /// Replace the scanner (tests, or `None` to turn scanning off).
    pub fn set_scanner(&self, scanner: Option<Arc<dyn ScanBackend>>) {
        *lock(&self.scanner) = scanner;
    }

    /// Whether this machine has any scanner support at all.
    pub fn scan_supported(&self) -> bool {
        lock(&self.scanner).is_some()
    }

    fn scanner(&self) -> Result<Arc<dyn ScanBackend>> {
        lock(&self.scanner)
            .clone()
            .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "scanning is not available here"))
    }

    /// CMD-SCAN-DEVICES. Asks the system, which can take a few seconds: call it off the UI thread.
    pub fn scan_devices(&self) -> Result<Vec<ScanDeviceDto>> {
        Ok(self
            .scanner()?
            .devices()?
            .into_iter()
            .map(|d| ScanDeviceDto { id: d.id, name: d.name })
            .collect())
    }

    /// CMD-SCAN-START → the job id; progress is the job's `detail.pages`.
    pub fn scan_start(self: &Arc<Self>, dto: ScanProfileDto) -> Result<u64> {
        self.ready()?;
        let profile = dto.to_profile()?;
        let scanner = self.scanner()?;
        let recognize = match dto.ocr {
            Some(on) => on,
            None => lock(&self.settings).ocr_mode == "Auto",
        } && self.tesseract;
        let dir = self.session_dir().join("scans");
        std::fs::create_dir_all(&dir).map_err(|e| AppError::io("create the scan folder", &e))?;
        let me = Arc::clone(self);
        Ok(self.spawn_job("scan", move |ctx| {
            let cancel = CancelToken::new();
            let (tx, rx) = mpsc::channel::<ScannedPage>();
            let scanning = {
                let cancel = cancel.clone();
                std::thread::Builder::new()
                    .name("scan".into())
                    .spawn(move || scanner.scan(&profile, tx, cancel))
                    .map_err(|e| AppError::io("start the scanner", &e))?
            };
            let mut kept = 0usize;
            let mut stopped: Option<AppError> = None;
            loop {
                if ctx.cancelled() || me.closing.is_set() {
                    cancel.cancel();
                }
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(page) => {
                        // After our own failure the scanner is being stopped; anything still queued is dropped.
                        if stopped.is_some() {
                            continue;
                        }
                        match me.keep_scanned_page(ctx.entry.id, &dir, &page, recognize) {
                            Ok(()) => {
                                kept += 1;
                                ctx.set_detail(json!({ "pages": kept }));
                                if kept >= MAX_SCAN_PAGES {
                                    cancel.cancel();
                                    stopped = Some(AppError::limit("too many pages in one scan"));
                                }
                            }
                            Err(e) => {
                                cancel.cancel();
                                stopped = Some(e);
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            let scanned = scanning
                .join()
                .unwrap_or_else(|_| Err(AppError::internal("the scanner stopped unexpectedly")));
            if let Some(e) = stopped {
                return Err(e);
            }
            scanned?;
            Ok(JobOutput {
                outputs: Vec::new(),
                detail: json!({ "pages": kept }),
            })
        }))
    }

    /// Write one scanned page into the session folder and bring it in.
    fn keep_scanned_page(self: &Arc<Self>, job: u64, dir: &Path, page: &ScannedPage, recognize: bool) -> Result<()> {
        let path = dir.join(format!("scan-{job}-{:03}.{}", page.number, page.ext));
        std::fs::write(&path, &page.data).map_err(|e| AppError::io("save a scanned page", &e))?;
        let report = self.import_one(&path)?;
        if recognize {
            // FLOW-SCAN-LIVE: queued now, while the next page is still being scanned.
            if let Err(e) = self.ocr_run(Some(report.pages.clone()), OcrPolicyDto::default()) {
                self.emit_error(&e);
            }
        }
        Ok(())
    }
}
