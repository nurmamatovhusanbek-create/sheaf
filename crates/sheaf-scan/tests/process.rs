//! TEST-SCAN: the scanner runs in a helper process, and whatever the helper
//! does (crash, abort, hang, babble) the caller gets pages and an error, never
//! a dead process of its own.

use sheaf_core::ErrorCode;
use sheaf_scan::{CancelToken, ProcessBackend, ScanBackend, ScanProfile, ScanSource, ScannedPage};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

fn backend(mode: &str) -> ProcessBackend {
    ProcessBackend::new(env!("CARGO_BIN_EXE_sheaf-fake-scan-helper"), vec![]).with_env("SHEAF_FAKE_SCAN", mode)
}

fn feeder() -> ScanProfile {
    let mut p = ScanProfile::new("fake:1");
    p.source = ScanSource::Feeder;
    p
}

fn collect(rx: &Receiver<ScannedPage>) -> Vec<usize> {
    rx.try_iter().map(|p| p.number).collect()
}

#[test]
fn devices_and_a_full_feeder_come_through() {
    let b = backend("ok");
    let devices = b.devices().expect("devices");
    assert_eq!((devices.len(), devices[0].id.as_str()), (1, "fake:1"));

    let (tx, rx) = channel();
    b.scan(&feeder(), tx, CancelToken::new()).expect("scan");
    let pages: Vec<_> = rx.try_iter().collect();
    assert_eq!(pages.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    assert!(pages.iter().all(|p| p.data.starts_with(b"\x89PNG") && p.ext == "png"));
}

#[test]
fn a_reported_failure_keeps_its_kind_and_the_pages_before_it() {
    let (tx, rx) = channel();
    let err = backend("jam").scan(&feeder(), tx, CancelToken::new()).expect_err("jam");
    assert_eq!(err.code, ErrorCode::DeviceError);
    assert_eq!(collect(&rx), [1, 2]);
}

#[test]
fn a_driver_that_kills_its_process_is_an_error_not_the_end_of_the_app() {
    // 0xC0000374 is what Windows reports for a corrupted heap.
    let (tx, rx) = channel();
    let err = backend("crash")
        .scan(&feeder(), tx, CancelToken::new())
        .expect_err("crash");
    assert_eq!(err.code, ErrorCode::DeviceCrashed);
    assert!(
        err.msg.contains("exit code"),
        "the exit code is in the details: {}",
        err.msg
    );
    // Windows keeps all 32 bits of it; other systems keep 8.
    #[cfg(windows)]
    assert!(err.msg.contains("0xc0000374"), "{}", err.msg);
    assert_eq!(collect(&rx), [1, 2], "pages that arrived are kept");

    let (tx, rx) = channel();
    let err = backend("abort")
        .scan(&feeder(), tx, CancelToken::new())
        .expect_err("abort");
    assert_eq!(err.code, ErrorCode::DeviceCrashed);
    assert_eq!(collect(&rx), [1]);

    // Listing the scanners is isolated too.
    assert_eq!(
        backend("crash").devices().expect_err("crash").code,
        ErrorCode::DeviceCrashed
    );
}

#[test]
fn a_helper_that_babbles_is_cut_off() {
    let (tx, rx) = channel();
    let started = Instant::now();
    let err = backend("junk")
        .scan(&feeder(), tx, CancelToken::new())
        .expect_err("junk");
    assert_eq!(err.code, ErrorCode::DeviceCrashed);
    assert!(err.msg.contains("protocol"), "{}", err.msg);
    assert_eq!(collect(&rx), [1], "the page before the noise is kept");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn cancelling_stops_the_helper_even_when_it_hangs() {
    let cancel = CancelToken::new();
    let (tx, rx) = channel();
    let stopper = cancel.clone();
    let started = Instant::now();
    let watcher = std::thread::spawn(move || {
        // Cancel as soon as the first page is in.
        while rx.recv_timeout(Duration::from_secs(20)).is_err() {}
        stopper.cancel();
        rx
    });
    let err = backend("hang").scan(&feeder(), tx, cancel).expect_err("cancelled");
    assert_eq!(err.code, ErrorCode::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "killed after the grace period"
    );
    drop(watcher.join().expect("watcher"));
}

#[test]
fn a_polite_cancel_ends_a_healthy_scan_quickly() {
    let cancel = CancelToken::new();
    let (tx, rx) = channel();
    let stopper = cancel.clone();
    let watcher = std::thread::spawn(move || {
        let _ = rx.recv_timeout(Duration::from_secs(20));
        stopper.cancel();
        rx
    });
    let started = Instant::now();
    let err = backend("ok").scan(&feeder(), tx, cancel).expect_err("cancelled");
    assert_eq!(err.code, ErrorCode::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "no need to kill a helper that listens"
    );
    drop(watcher.join().expect("watcher"));
}

#[test]
fn a_caller_that_stops_listening_counts_as_a_cancel() {
    let (tx, rx) = channel();
    drop(rx);
    let err = backend("ok")
        .scan(&feeder(), tx, CancelToken::new())
        .expect_err("nobody listens");
    assert_eq!(err.code, ErrorCode::Cancelled);
}

#[test]
fn a_missing_helper_is_a_device_error() {
    let b = ProcessBackend::new("/nonexistent/sheaf-helper", vec![]);
    assert_eq!(b.devices().expect_err("missing").code, ErrorCode::DeviceError);
    let (tx, _rx) = channel();
    assert_eq!(
        b.scan(&feeder(), tx, CancelToken::new()).expect_err("missing").code,
        ErrorCode::DeviceError
    );
}
