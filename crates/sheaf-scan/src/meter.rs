//! SCAN-PROGRESS: how far the page being transferred is, as the driver says.
//!
//! WIA reports a percentage for the page it is transferring (`lPercentComplete`); some drivers fill it
//! in and some never do. The meter keeps the last value and remembers whether the driver ever gave a
//! real one in this scan, so the window can show a true percentage when there is one and say "about"
//! (or nothing) when there is not. The scanner helper process forwards the meter to the app as small
//! frames; a backend that runs in-process (the fake one) shares it directly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Default)]
pub struct ScanMeter {
    page: AtomicU32,
    percent: AtomicU32,
    /// The driver has reported something above zero during this scan.
    reported: AtomicBool,
}

/// What the meter holds at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// The page being transferred, 1-based (0 before the first).
    pub page: u32,
    /// The driver's own percentage for that page; `None` when this driver does not report one.
    pub percent: Option<u8>,
}

impl ScanMeter {
    pub fn new() -> Arc<ScanMeter> {
        Arc::new(ScanMeter::default())
    }

    /// A new scan: forget the last one.
    pub fn reset(&self) {
        self.page.store(0, Ordering::SeqCst);
        self.percent.store(0, Ordering::SeqCst);
        self.reported.store(false, Ordering::SeqCst);
    }

    /// The driver says page `page` is `percent` % transferred.
    pub fn set(&self, page: u32, percent: u8) {
        let percent = u32::from(percent.min(100));
        self.page.store(page, Ordering::SeqCst);
        self.percent.store(percent, Ordering::SeqCst);
        if percent > 0 {
            self.reported.store(true, Ordering::SeqCst);
        }
    }

    /// Exactly what is stored, for forwarding (`reported` is rebuilt by [`ScanMeter::set`] on the other side).
    pub fn raw(&self) -> (u32, u8) {
        (
            self.page.load(Ordering::SeqCst),
            self.percent.load(Ordering::SeqCst) as u8,
        )
    }

    pub fn reading(&self) -> Reading {
        Reading {
            page: self.page.load(Ordering::SeqCst),
            percent: self
                .reported
                .load(Ordering::SeqCst)
                .then(|| self.percent.load(Ordering::SeqCst) as u8),
        }
    }
}

/// The meter of the real driver, which the callback (deep inside a COM object) can reach and the
/// helper's main loop can read.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn driver() -> &'static Arc<ScanMeter> {
    static METER: OnceLock<Arc<ScanMeter>> = OnceLock::new();
    METER.get_or_init(ScanMeter::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_driver_that_never_reports_has_no_percentage() {
        let m = ScanMeter::new();
        m.set(1, 0);
        m.set(1, 0);
        assert_eq!(m.reading(), Reading { page: 1, percent: None });
    }

    #[test]
    fn a_reported_percentage_is_shown_and_the_next_page_starts_again_at_zero() {
        let m = ScanMeter::new();
        m.set(1, 40);
        assert_eq!(
            m.reading(),
            Reading {
                page: 1,
                percent: Some(40)
            }
        );
        m.set(1, 100);
        m.set(2, 0);
        // The driver has shown it reports, so zero on a new page is a real zero, not "unknown".
        assert_eq!(
            m.reading(),
            Reading {
                page: 2,
                percent: Some(0)
            }
        );
        m.set(2, 250);
        assert_eq!(m.reading().percent, Some(100), "never above 100");
    }

    #[test]
    fn reset_forgets_the_last_scan() {
        let m = ScanMeter::new();
        m.set(3, 80);
        m.reset();
        assert_eq!(m.reading(), Reading { page: 0, percent: None });
    }
}
