//! MOD-PDF basics against the vendored PDFium: probe, render, build.
//! Needs `bash tools/fetch-vendor` to have run.

use fixture_gen::{PageDef, Scratch};
use sheaf_core::{ErrorCode, Rot};
use sheaf_pdf::Engine;

fn probe_reports_pages_text_and_sizes(engine: &Engine) {
    let dir = Scratch::new("pdf-probe");
    let file = dir.file(
        "mixed.pdf",
        &fixture_gen::pdf(&[
            PageDef::text("Hello"),
            PageDef::no_text(),
            PageDef::text("Wide").sized(842, 595),
            PageDef::text("Turned").rotated(90),
        ]),
    );
    let probe = engine.probe(&file).expect("probe");
    assert_eq!(probe.page_count, 4);
    assert!(!probe.encrypted);
    assert_eq!(probe.has_text, [true, false, true, true]);
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(
        rounded,
        [(595, 842), (595, 842), (842, 595), (842, 595)],
        "sizes are as displayed"
    );
}

fn hostile_files_fail_cleanly(engine: &Engine) {
    let dir = Scratch::new("pdf-bad");
    let corrupt = dir.file("corrupt.pdf", &fixture_gen::corrupt_pdf());
    assert_eq!(
        engine.probe(&corrupt).expect_err("corrupt").code,
        ErrorCode::ParseError
    );
    let empty = dir.file("empty.pdf", b"");
    assert_eq!(
        engine.probe(&empty).expect_err("empty").code,
        ErrorCode::ParseError
    );
    assert!(engine.probe(&dir.path().join("missing.pdf")).is_err());

    // A locked file reports that it is locked and reveals nothing else.
    let locked = dir.file("locked.pdf", &fixture_gen::locked_pdf());
    let probe = engine
        .probe(&locked)
        .expect("probe of a locked file succeeds");
    assert!(probe.encrypted);
    assert_eq!(probe.page_count, 0);
    assert_eq!(
        engine
            .render(&locked, 0, (100, 100))
            .expect_err("locked")
            .code,
        ErrorCode::NeedsPassword
    );
}

fn render_fits_the_box_and_draws_the_page(engine: &Engine) {
    let dir = Scratch::new("pdf-render");
    let file = dir.file(
        "doc.pdf",
        &fixture_gen::pdf(&[PageDef::no_text(), PageDef::text("x").sized(842, 595)]),
    );

    let thumb = engine.render(&file, 0, (256, 256)).expect("render");
    assert_eq!(thumb.dimensions(), (181, 256));
    let center = thumb.get_pixel(90, 128);
    let corner = thumb.get_pixel(5, 5);
    assert!(center[0] < 100, "the dark box is in the middle: {center:?}");
    assert!(corner[0] > 240, "the paper is white: {corner:?}");

    let wide = engine.render(&file, 1, (256, 256)).expect("render");
    assert_eq!(wide.dimensions(), (256, 181));

    // An oversized request is capped at 300 dpi, not honored blindly.
    let full = engine.render(&file, 0, (100_000, 100_000)).expect("render");
    assert_eq!(full.dimensions(), (2479, 3508));

    assert_eq!(
        engine
            .render(&file, 2, (10, 10))
            .expect_err("past the end")
            .code,
        ErrorCode::InvalidArg
    );
}

fn build_copies_reorders_and_rotates(engine: &Engine) {
    // TEST-PAGEOPS at the file level: page sequence and /Rotate.
    let dir = Scratch::new("pdf-build");
    let a = dir.file("a.pdf", &fixture_gen::text_pdf(5));
    let b = dir.file(
        "b.pdf",
        &fixture_gen::pdf(&[PageDef::text("Other").rotated(90)]),
    );

    let mut builder = engine.builder().expect("builder");
    builder.copy_page(&a, 4, Rot::R0).expect("copy");
    builder.copy_page(&b, 0, Rot::R90).expect("copy"); // 90 already + 90 more
    builder.copy_page(&a, 0, Rot::R270).expect("copy");
    builder.blank_page(300.0, 400.0, Rot::R0).expect("blank");
    builder.copy_page(&a, 0, Rot::R0).expect("same page twice");
    assert_eq!(
        builder
            .copy_page(&a, 5, Rot::R0)
            .expect_err("past the end")
            .code,
        ErrorCode::InvalidArg
    );
    let out = dir.path().join("out.pdf");
    let stats = builder.save(&out).expect("save");
    assert_eq!((stats.pages, stats.copied, stats.rasterized), (5, 4, 0));

    let probe = engine.probe(&out).expect("probe output");
    assert_eq!(probe.page_count, 5);
    assert_eq!(
        probe.has_text,
        [true, true, true, false, true],
        "copied pages keep their text layer"
    );
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(
        rounded,
        [(595, 842), (595, 842), (842, 595), (300, 400), (595, 842)],
        "b's page is now at 180°, a's first page at 270°"
    );

    // Rotation is stored as /Rotate, not baked into re-encoded content.
    let bytes = std::fs::read(&out).expect("read output");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("/Rotate 180"),
        "second page carries /Rotate 180"
    );
    assert!(
        text.contains("/Rotate 270"),
        "third page carries /Rotate 270"
    );
    assert!(!text.contains("/Image"), "nothing was rasterized");
}

fn image_pages_get_their_size_from_the_dpi(engine: &Engine) {
    let dir = Scratch::new("pdf-image");
    let mut img = sheaf_image::RgbaImage::from_pixel(600, 900, image::Rgba([255, 255, 255, 255]));
    for y in 0..450 {
        for x in 0..600 {
            img.put_pixel(x, y, image::Rgba([200, 30, 30, 255]));
        }
    }
    let mut builder = engine.builder().expect("builder");
    builder.image_page(&img, 300, Rot::R0).expect("image page");
    builder.image_page(&img, 150, Rot::R90).expect("image page");
    let out = dir.path().join("scan.pdf");
    let stats = builder.save(&out).expect("save");
    assert_eq!((stats.pages, stats.copied, stats.rasterized), (2, 0, 2));

    let probe = engine.probe(&out).expect("probe");
    assert_eq!(probe.has_text, [false, false]);
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(rounded, [(144, 216), (432, 288)]);

    let back = engine.render(&out, 0, (600, 900)).expect("render");
    assert_eq!(back.dimensions(), (600, 900));
    assert!(
        back.get_pixel(300, 100)[0] > 150 && back.get_pixel(300, 100)[1] < 80,
        "top half is red"
    );
    assert!(back.get_pixel(300, 800)[1] > 240, "bottom half is white");
}

fn an_empty_build_is_refused(engine: &Engine) {
    let dir = Scratch::new("pdf-empty");
    let out = dir.path().join("out.pdf");
    assert_eq!(
        engine
            .builder()
            .expect("builder")
            .save(&out)
            .expect_err("empty")
            .code,
        ErrorCode::InvalidArg
    );
    assert!(!out.exists());
}

fn missing_library_is_reported_as_such(engine: &Engine) {
    let dir = Scratch::new("pdf-nolib");
    let _ = engine;
    assert_eq!(
        Engine::load(dir.path()).err().map(|e| e.code),
        Some(ErrorCode::EngineMissing)
    );
}

/// One test, one thread, one engine: PDFium is not thread-safe, and dropping
/// an engine tears the library down for every other user in the process
/// (ADR-WORKER-PROCS). The harness would otherwise run these in parallel.
#[test]
fn pdf_suite() {
    let engine = Engine::load(&fixture_gen::vendored_pdfium_dir())
        .expect("PDFium loads (run `bash tools/fetch-vendor` first)");
    probe_reports_pages_text_and_sizes(&engine);
    hostile_files_fail_cleanly(&engine);
    render_fits_the_box_and_draws_the_page(&engine);
    build_copies_reorders_and_rotates(&engine);
    image_pages_get_their_size_from_the_dpi(&engine);
    an_empty_build_is_refused(&engine);
    missing_library_is_reported_as_such(&engine);
}
