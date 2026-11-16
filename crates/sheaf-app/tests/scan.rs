//! TEST-SCAN through the command layer: a fake feeder, page order, cancel
//! mid-batch, recognition overlapping the scan, and the error mapping. The
//! real scanner (WIA) is not involved (RISK-WIA).

use fixture_gen::{noto_font, tessdata_dir, tesseract_exe, vendored_pdfium_dir, Scratch};
use serde_json::Value;
use sheaf_app::api::{Backend, BackendConfig, Emit, Event, ScanProfileDto};
use sheaf_app::{sibling_worker, Engines};
use sheaf_core::ErrorCode;
use sheaf_scan::FakeBackend;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Collector(Mutex<Vec<Event>>);

impl Emit for Collector {
    fn emit(&self, event: Event) {
        self.0.lock().expect("events").push(event);
    }
}

struct Rig {
    backend: Arc<Backend>,
    events: Arc<Collector>,
    // Dropped last: the session lives in here.
    _base: Scratch,
}

fn rig(fake: FakeBackend) -> Rig {
    let base = Scratch::new("scan-base");
    let events = Arc::new(Collector::default());
    let backend = Backend::start(
        BackendConfig {
            engines: Engines {
                worker: sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)"),
                pdfium_dir: vendored_pdfium_dir(),
                tesseract: Some((tesseract_exe(), tessdata_dir())),
                font: Some(noto_font()),
            },
            base_dir: base.path().join("sessions"),
            config_dir: Some(base.path().join("config")),
            verify_vendor: false,
        },
        events.clone(),
    )
    .expect("backend starts");
    backend.set_scanner(Some(Arc::new(fake)));
    Rig {
        backend,
        events,
        _base: base,
    }
}

fn fast() -> FakeBackend {
    FakeBackend::new().with_delay(Duration::from_millis(5))
}

fn feeder(ocr: bool) -> ScanProfileDto {
    ScanProfileDto {
        device_id: "fake:1".into(),
        source: "feeder".into(),
        dpi: 150,
        ocr: Some(ocr),
        ..ScanProfileDto::default()
    }
}

impl Rig {
    fn job(&self, id: u64) -> Option<Value> {
        self.backend
            .jobs()
            .into_iter()
            .find(|j| j.id == id)
            .map(|j| serde_json::to_value(j).expect("view"))
    }

    fn wait(&self, id: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let view = self.job(id).expect("job exists");
            if matches!(view["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return view;
            }
            assert!(Instant::now() < deadline, "scan job did not finish: {view}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn page_count(&self) -> usize {
        self.backend.pages(0, 1_000).expect("pages").pages.len()
    }
}

#[test]
fn devices_are_listed_and_a_feeder_scan_adds_its_pages_in_order() {
    let rig = rig(fast().with_sheets(3));
    let devices = rig.backend.scan_devices().expect("devices");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, "fake:1");

    let job = rig.backend.scan_start(feeder(false)).expect("start");
    let view = rig.wait(job);
    assert_eq!(view["state"], "done", "{view}");
    assert_eq!(view["detail"]["pages"], 3);
    assert_eq!(rig.page_count(), 3);

    // One source per page, in the order they came out of the feeder.
    let names: Vec<String> = rig
        .backend
        .snapshot()
        .expect("snapshot")
        .sources
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(
        names[0].ends_with("001.png") && names[2].ends_with("003.png"),
        "{names:?}"
    );
    // The tray was told as pages arrived, not only at the end.
    let announced = rig
        .events
        .0
        .lock()
        .expect("events")
        .iter()
        .filter(|e| matches!(e, Event::Pages(p) if p.total >= 1))
        .count();
    assert!(announced >= 3, "{announced}");
}

#[test]
fn the_flatbed_scans_one_page_and_two_sided_feeding_doubles_them() {
    let rig = rig(fast().with_sheets(2));
    let flat = ScanProfileDto {
        device_id: "fake:1".into(),
        dpi: 150,
        ocr: Some(false),
        ..ScanProfileDto::default()
    };
    assert_eq!(
        rig.wait(rig.backend.scan_start(flat).expect("flatbed"))["detail"]["pages"],
        1
    );
    let mut duplex = feeder(false);
    duplex.duplex = true;
    assert_eq!(
        rig.wait(rig.backend.scan_start(duplex).expect("duplex"))["detail"]["pages"],
        4
    );
    assert_eq!(rig.page_count(), 5);
}

#[test]
fn recognition_is_queued_while_the_scan_is_still_running() {
    let rig = rig(FakeBackend::new().with_sheets(3).with_delay(Duration::from_millis(250)));
    let job = rig.backend.scan_start(feeder(true)).expect("start");
    let view = rig.wait(job);
    assert_eq!(view["state"], "done", "{view}");

    // An "ocr" job was announced before the scan job reported done.
    let events = rig.events.0.lock().expect("events");
    let kinds: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Job(v) => Some((v.kind.clone(), v.state.to_string())),
            _ => None,
        })
        .collect();
    let first_ocr = kinds
        .iter()
        .position(|(k, _)| k == "ocr")
        .expect("an ocr job was queued");
    let scan_done = kinds
        .iter()
        .position(|(k, s)| k == "scan" && s == "done")
        .expect("the scan finished");
    assert!(first_ocr < scan_done, "{kinds:?}");
}

#[test]
fn cancelling_mid_batch_keeps_the_pages_already_scanned() {
    let rig = rig(FakeBackend::new()
        .with_sheets(200)
        .with_delay(Duration::from_millis(40)));
    let job = rig.backend.scan_start(feeder(false)).expect("start");
    let deadline = Instant::now() + Duration::from_secs(30);
    while rig.page_count() < 2 {
        assert!(Instant::now() < deadline, "no pages arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    rig.backend.job_cancel(job);
    let view = rig.wait(job);
    assert_eq!(view["state"], "cancelled", "{view}");
    let kept = rig.page_count();
    assert!((2..200).contains(&kept), "{kept}");
    assert_eq!(view["detail"]["pages"], kept, "the count shown is the count kept");
}

#[test]
fn failures_are_reported_with_their_codes_and_earlier_pages_stay() {
    let jam = rig(fast().with_sheets(5).with_jam_after(2));
    let view = jam.wait(jam.backend.scan_start(feeder(false)).expect("start"));
    assert_eq!(view["state"], "failed");
    assert_eq!(
        view["error"]["code"],
        serde_json::to_value(ErrorCode::DeviceError).expect("code")
    );
    assert_eq!(jam.page_count(), 2, "pages before the jam are kept");

    let empty = rig(fast().with_sheets(0));
    let view = empty.wait(empty.backend.scan_start(feeder(false)).expect("start"));
    assert_eq!(
        view["error"]["code"],
        serde_json::to_value(ErrorCode::NoPaper).expect("code")
    );
    assert_eq!(view["error"]["key"], "error.no_paper");

    let busy = rig(fast().with_busy(true));
    let view = busy.wait(busy.backend.scan_start(feeder(false)).expect("start"));
    assert_eq!(
        view["error"]["code"],
        serde_json::to_value(ErrorCode::DeviceBusy).expect("code")
    );
}

#[test]
fn a_bad_request_is_refused_before_anything_starts() {
    let rig = rig(fast());
    let refuse = |f: &dyn Fn(&mut ScanProfileDto)| {
        let mut p = feeder(false);
        f(&mut p);
        rig.backend.scan_start(p).expect_err("refused").code
    };
    assert_eq!(refuse(&|p| p.dpi = 77), ErrorCode::InvalidArg);
    assert_eq!(refuse(&|p| p.source = "tray".into()), ErrorCode::InvalidArg);
    assert_eq!(refuse(&|p| p.color = "pink".into()), ErrorCode::InvalidArg);
    assert_eq!(refuse(&|p| p.paper = "a0".into()), ErrorCode::InvalidArg);
    assert_eq!(refuse(&|p| p.device_id = String::new()), ErrorCode::InvalidArg);
    assert_eq!(
        refuse(&|p| {
            p.source = "flatbed".into();
            p.duplex = true;
        }),
        ErrorCode::InvalidArg
    );
    assert_eq!(rig.page_count(), 0);

    rig.backend.set_scanner(None);
    assert_eq!(
        rig.backend.scan_start(feeder(false)).expect_err("none").code,
        ErrorCode::Unsupported
    );
    assert!(!rig.backend.scan_supported());
}
