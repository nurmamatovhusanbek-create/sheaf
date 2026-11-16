//! SCAN-SPEED: pages are converted off the thread the driver calls back on.
//!
//! A scanner driver hands over each page from inside a callback, and does not start the next page
//! until that callback returns. Whatever is done in it, decoding a bitmap and writing it as PNG
//! for one, is time the scanner spends standing still. So the callback only copies the bytes and
//! passes them on; this thread does [`normalize_for`], in order, while the scanner carries on with the
//! next sheet. A few pages may wait in the queue; beyond that the callback waits, which bounds
//! memory (a raw A4 page at 300 dpi is about 26 MB).

use crate::{diag, normalize_for, CancelToken, ScanColor, ScannedPage};
use sheaf_core::{AppError, ErrorCode, Result};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::thread::JoinHandle;

/// Raw pages that may wait for conversion.
const QUEUE: usize = 3;

/// The queue to put raw page bytes into, and the thread that converts them and sends the results on
/// as numbered pages. Dropping the queue's sender ends the thread; joining it gives the first error
/// (and cancels `cancel` so the driver stops). Pages converted before an error are kept.
pub(crate) fn start(
    pages: Sender<ScannedPage>,
    cancel: CancelToken,
    color: ScanColor,
) -> (SyncSender<Vec<u8>>, JoinHandle<Result<()>>) {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(QUEUE);
    let handle = std::thread::Builder::new().name("scan-convert".into()).spawn(move || {
        let mut number = 0usize;
        for raw in rx {
            if cancel.is_cancelled() {
                continue; // drain what is queued without doing the work
            }
            let (began, raw_len) = (std::time::Instant::now(), raw.len());
            let outcome = normalize_for(raw, color).and_then(|(data, ext)| {
                number += 1;
                diag::note(format!(
                    "page {number} converted in {} ms: {raw_len} -> {} bytes ({ext})",
                    began.elapsed().as_millis(),
                    data.len()
                ));
                pages
                    .send(ScannedPage { number, data, ext })
                    .map_err(|_| AppError::new(ErrorCode::Cancelled, "nobody is listening"))
            });
            if let Err(e) = outcome {
                cancel.cancel();
                return Err(e);
            }
        }
        Ok(())
    });
    match handle {
        Ok(handle) => (tx, handle),
        // No thread, no conversion: a queue nobody reads would stall the driver, so hand back a closed one.
        Err(e) => {
            let (dead, _) = mpsc::sync_channel(0);
            (
                dead,
                std::thread::spawn(move || Err(AppError::io("start the page converter", &e))),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn png() -> Vec<u8> {
        sheaf_image::encode_png(&sheaf_image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))).expect("png")
    }

    fn bmp() -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(6, 6, image::Rgba([9, 9, 9, 255])))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Bmp)
            .expect("bmp");
        out
    }

    #[test]
    fn pages_come_out_in_order_numbered_and_converted() {
        let (out, rx) = channel();
        let (queue, converter) = start(out, CancelToken::new(), ScanColor::Bw);
        queue.send(png()).expect("queue");
        queue.send(bmp()).expect("queue");
        queue.send(png()).expect("queue");
        drop(queue);
        converter.join().expect("thread").expect("no error");
        let pages: Vec<_> = rx.try_iter().collect();
        assert_eq!(pages.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(
            pages.iter().all(|p| p.data.starts_with(b"\x89PNG")),
            "the bitmap became a PNG"
        );
    }

    #[test]
    fn the_driver_is_not_held_up_by_conversion() {
        // The queue takes a few pages at once: handing them over does not wait for the first to be converted.
        let (out, _rx) = channel();
        let (queue, converter) = start(out, CancelToken::new(), ScanColor::Bw);
        let started = std::time::Instant::now();
        for _ in 0..QUEUE {
            queue.send(bmp()).expect("queue");
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        drop(queue);
        converter.join().expect("thread").expect("ok");
    }

    #[test]
    fn a_bad_page_stops_the_scan_with_its_error_and_keeps_the_pages_before_it() {
        let (out, rx) = channel();
        let cancel = CancelToken::new();
        let (queue, converter) = start(out, cancel.clone(), ScanColor::Bw);
        queue.send(png()).expect("queue");
        queue.send(b"not an image".to_vec()).expect("queue");
        let _ = queue.send(png()); // may or may not be taken: the converter has stopped
        drop(queue);
        let err = converter.join().expect("thread").expect_err("bad page");
        assert_eq!(err.code, ErrorCode::DeviceError);
        assert!(cancel.is_cancelled(), "the driver is told to stop");
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn a_listener_that_went_away_counts_as_a_cancel() {
        let (out, rx) = channel();
        drop(rx);
        let (queue, converter) = start(out, CancelToken::new(), ScanColor::Bw);
        queue.send(png()).expect("queue");
        drop(queue);
        assert_eq!(
            converter.join().expect("thread").expect_err("nobody listens").code,
            ErrorCode::Cancelled
        );
    }
}
