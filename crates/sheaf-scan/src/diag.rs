//! SCAN-TIMING: where a scan spends its time.
//!
//! Scanning speed is decided by things only the real device shows: which transfer format its
//! driver accepts, how long it takes to hand over a page, how much of that is the app. So each scan
//! keeps a short log of timed notes (no document content, nothing but sizes and times), which the
//! helper sends to the app and the app writes to `scan-timing.txt` in its settings folder after
//! every scan. When a scan is slow, that file says why.

use std::sync::Mutex;
use std::time::Instant;

static START: Mutex<Option<Instant>> = Mutex::new(None);
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Start a new log; `header` says what is being scanned.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn begin(header: impl Into<String>) {
    *lock(&START) = Some(Instant::now());
    let mut notes = lock(&NOTES);
    notes.clear();
    notes.push(header.into());
}

/// Add a line, stamped with the milliseconds since [`begin`].
pub(crate) fn note(line: impl AsRef<str>) {
    let ms = lock(&START).map_or(0, |t| t.elapsed().as_millis());
    lock(&NOTES).push(format!("{ms:>7} ms  {}", line.as_ref()));
}

/// Take the log, leaving it empty.
pub(crate) fn take() -> Vec<String> {
    std::mem::take(&mut *lock(&NOTES))
}
