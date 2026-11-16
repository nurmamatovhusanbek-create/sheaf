//! SEC-LIMITS and SEC-ISOLATE against hostile input: mutated and crafted files
//! go through the real import and render path. Whatever they do, the app must
//! answer (a result or an error, within a time bound), never crash or hang, and
//! the next, ordinary file must still open: the worker pool recovers.

use fixture_gen::{pdf, text_pdf, PageDef, Scratch};
use harness::App;
use std::path::Path;
use std::time::{Duration, Instant};

fn app(base: &Scratch) -> App {
    App::start(&harness::vendored().expect("engines"), &base.path().join("s"), 2).expect("app")
}

/// xorshift: the same inputs on every run, so a failure can be reproduced.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    let mut tail = kind.to_vec();
    tail.extend_from_slice(body);
    out.extend_from_slice(&tail);
    out.extend_from_slice(&crc32(&tail).to_be_bytes());
    out
}

/// A PNG whose header claims `w` by `h` pixels but whose data is a few bytes:
/// the decompression-bomb shape.
fn bomb_png(w: u32, h: u32) -> Vec<u8> {
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(b"IDAT", &[0x78, 0x9C, 0x63, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01]));
    out.extend(chunk(b"IEND", &[]));
    out
}

/// Replace the first occurrence of `from` in `bytes` with `to`.
fn patch(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = bytes
        .windows(from.len())
        .position(|w| w == from)
        .expect("pattern present");
    let mut out = bytes[..at].to_vec();
    out.extend_from_slice(to);
    out.extend_from_slice(&bytes[at + from.len()..]);
    out
}

/// Open `path` and, if it opened, draw every page. Returns how long that took.
fn try_open(app: &mut App, path: &Path) -> Duration {
    let started = Instant::now();
    if let Ok(report) = app.import(path) {
        if !report.pages.is_empty() {
            let _ = app.thumbs(&report.pages[..report.pages.len().min(3)]);
        }
    }
    started.elapsed()
}

#[test]
fn mutated_pdfs_never_take_the_app_down() {
    let base = Scratch::new("hostile-base");
    let dir = Scratch::new("hostile-files");
    let mut app = app(&base);
    let good = text_pdf(3);
    let mut rng = Rng(0x5EAF_0001);
    let mut slowest = Duration::ZERO;
    for i in 0..48 {
        let mut bytes = good.clone();
        match i % 4 {
            0 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(bytes.len());
                    bytes[at] = rng.next() as u8;
                }
            }
            1 => bytes.truncate(rng.below(bytes.len())),
            2 => {
                let (a, b) = (rng.below(bytes.len()), rng.below(bytes.len()));
                let chunk: Vec<u8> = bytes[a.min(b)..a.max(b)].to_vec();
                let at = rng.below(bytes.len());
                bytes.splice(at..at, chunk);
            }
            _ => {
                let at = rng.below(bytes.len());
                bytes[at..].fill(0);
            }
        }
        let path = dir.file(&format!("m{i}.pdf"), &bytes);
        slowest = slowest.max(try_open(&mut app, &path));
    }
    assert!(slowest < Duration::from_secs(30), "slowest input took {slowest:?}");
    // The pool is still healthy.
    let ok = dir.file("ok.pdf", &good);
    assert_eq!(app.import(&ok).expect("ordinary file after the storm").pages.len(), 3);
}

#[test]
fn crafted_pdfs_are_refused_or_bounded() {
    let base = Scratch::new("hostile-base2");
    let dir = Scratch::new("hostile-files2");
    let mut app = app(&base);
    let good = text_pdf(2);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        // Page sizes far past anything printable.
        (
            "huge-mediabox",
            patch(&good, b"/MediaBox [0 0 595 842]", b"/MediaBox [0 0 99999999 99999999]"),
        ),
        // A page tree that claims a billion pages.
        ("page-count-lie", patch(&good, b"/Count 2", b"/Count 999999999")),
        // A page tree that points back at itself.
        (
            "cyclic-kids",
            patch(&good, b"/Kids [4 0 R 6 0 R]", b"/Kids [2 0 R 2 0 R]"),
        ),
        (
            "negative-size",
            patch(&good, b"/MediaBox [0 0 595 842]", b"/MediaBox [0 0 -595 -842]"),
        ),
        ("empty", Vec::new()),
        ("header-only", b"%PDF-1.7\n".to_vec()),
        (
            "not-a-pdf",
            b"MZ\x90\x00 this is an executable, not a document".to_vec(),
        ),
    ];
    for (name, bytes) in cases {
        let path = dir.file(&format!("{name}.pdf"), &bytes);
        let took = try_open(&mut app, &path);
        assert!(took < Duration::from_secs(30), "{name} took {took:?}");
    }
    let ok = dir.file("ok.pdf", &good);
    assert_eq!(
        app.import(&ok).expect("ordinary file after crafted ones").pages.len(),
        2
    );
}

#[test]
fn image_bombs_are_refused_before_they_are_decoded() {
    let base = Scratch::new("hostile-base3");
    let dir = Scratch::new("hostile-files3");
    let mut app = app(&base);
    // 60000 by 60000 is 3.6 billion pixels from a 70-byte file.
    for (name, bytes) in [
        ("bomb-60000.png", bomb_png(60_000, 60_000)),
        ("bomb-wide.png", bomb_png(u32::MAX, 1)),
        ("bomb-zero.png", bomb_png(0, 0)),
    ] {
        let path = dir.file(name, &bytes);
        let started = Instant::now();
        let result = app.import(&path).and_then(|r| {
            app.thumbs(&r.pages)?;
            Ok(r)
        });
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{name} took {:?}",
            started.elapsed()
        );
        assert!(result.is_err(), "{name} must not be rendered");
    }
    // And a modest page still works afterwards.
    let ok = dir.file("ok.pdf", &pdf(&[PageDef::text("fine")]));
    assert_eq!(app.import(&ok).expect("ordinary file").pages.len(), 1);
}
