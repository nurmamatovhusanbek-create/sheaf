//! MOD-PDF basics against the vendored PDFium: probe, render, build.
//! Needs `bash tools/fetch-vendor` to have run.

use fixture_gen::{PageDef, Scratch};
use sheaf_core::{Anchor, CompressLevel, ImageFit, NormRect, Password, Protect, Stamp};
use sheaf_core::{ErrorCode, Rot};
use sheaf_pdf::{lo, Engine, Src};

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
    let probe = engine.probe(Src::plain(&file)).expect("probe");
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
        engine.probe(Src::plain(&corrupt)).expect_err("corrupt").code,
        ErrorCode::ParseError
    );
    let empty = dir.file("empty.pdf", b"");
    assert_eq!(
        engine.probe(Src::plain(&empty)).expect_err("empty").code,
        ErrorCode::ParseError
    );
    assert!(engine.probe(Src::plain(&dir.path().join("missing.pdf"))).is_err());

    // A locked file reports that it is locked and reveals nothing else.
    let locked = dir.file("locked.pdf", &fixture_gen::locked_pdf());
    let probe = engine
        .probe(Src::plain(&locked))
        .expect("probe of a locked file succeeds");
    assert!(probe.encrypted);
    assert_eq!(probe.page_count, 0);
    assert_eq!(
        engine
            .render(Src::plain(&locked), 0, (100, 100))
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

    let thumb = engine.render(Src::plain(&file), 0, (256, 256)).expect("render");
    assert_eq!(thumb.dimensions(), (181, 256));
    let center = thumb.get_pixel(90, 128);
    let corner = thumb.get_pixel(5, 5);
    assert!(center[0] < 100, "the dark box is in the middle: {center:?}");
    assert!(corner[0] > 240, "the paper is white: {corner:?}");

    let wide = engine.render(Src::plain(&file), 1, (256, 256)).expect("render");
    assert_eq!(wide.dimensions(), (256, 181));

    // An oversized request is capped at 300 dpi, not honored blindly.
    let full = engine.render(Src::plain(&file), 0, (100_000, 100_000)).expect("render");
    assert_eq!(full.dimensions(), (2479, 3508));

    assert_eq!(
        engine
            .render(Src::plain(&file), 2, (10, 10))
            .expect_err("past the end")
            .code,
        ErrorCode::InvalidArg
    );
}

fn build_copies_reorders_and_rotates(engine: &Engine) {
    // TEST-PAGEOPS at the file level: page sequence and /Rotate.
    let dir = Scratch::new("pdf-build");
    let a = dir.file("a.pdf", &fixture_gen::text_pdf(5));
    let b = dir.file("b.pdf", &fixture_gen::pdf(&[PageDef::text("Other").rotated(90)]));

    let mut builder = engine.builder().expect("builder");
    builder.copy_page(Src::plain(&a), 4, Rot::R0, None, None).expect("copy");
    builder
        .copy_page(Src::plain(&b), 0, Rot::R90, None, None)
        .expect("copy"); // 90 already + 90 more
    builder
        .copy_page(Src::plain(&a), 0, Rot::R270, None, None)
        .expect("copy");
    builder.blank_page(300.0, 400.0, Rot::R0).expect("blank");
    builder
        .copy_page(Src::plain(&a), 0, Rot::R0, None, None)
        .expect("same page twice");
    assert_eq!(
        builder
            .copy_page(Src::plain(&a), 5, Rot::R0, None, None)
            .expect_err("past the end")
            .code,
        ErrorCode::InvalidArg
    );
    let out = dir.path().join("out.pdf");
    let stats = builder.save(&out).expect("save");
    assert_eq!((stats.pages, stats.copied, stats.rasterized), (5, 4, 0));

    let probe = engine.probe(Src::plain(&out)).expect("probe output");
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
    assert!(text.contains("/Rotate 180"), "second page carries /Rotate 180");
    assert!(text.contains("/Rotate 270"), "third page carries /Rotate 270");
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
    builder
        .image_page(&img, 300, Rot::R0, None, ImageFit::Fit)
        .expect("image page");
    builder
        .image_page(&img, 150, Rot::R90, None, ImageFit::Fit)
        .expect("image page");
    let out = dir.path().join("scan.pdf");
    let stats = builder.save(&out).expect("save");
    assert_eq!((stats.pages, stats.copied, stats.rasterized), (2, 0, 2));

    let probe = engine.probe(Src::plain(&out)).expect("probe");
    assert_eq!(probe.has_text, [false, false]);
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(rounded, [(144, 216), (432, 288)]);

    let back = engine.render(Src::plain(&out), 0, (600, 900)).expect("render");
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
        engine.builder().expect("builder").save(&out).expect_err("empty").code,
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

/// A photo-like image: smooth gradients with texture, which JPEG handles well
/// and flate does not.
fn photo(w: u32, h: u32) -> sheaf_image::RgbaImage {
    sheaf_image::RgbaImage::from_fn(w, h, |x, y| {
        let wave = ((x as f32 / 37.0).sin() * 40.0 + (y as f32 / 53.0).cos() * 40.0) as i32;
        let noise = ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 13 & 15) as i32;
        let v = |base: i32| (base + wave + noise).clamp(0, 255) as u8;
        image::Rgba([v(120 + (x * 100 / w) as i32), v(110), v(90 + (y * 120 / h) as i32), 255])
    })
}

fn size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).expect("stat").len()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn protect_and_unlock(engine: &Engine) {
    // TEST-PROTECT.
    let dir = Scratch::new("pdf-protect");
    let plain = dir.file("plain.pdf", &fixture_gen::text_pdf(2));
    let locked = dir.path().join("locked.pdf");
    let protect = Protect {
        user_pw: Password::new("s3cret ўқғҳ"),
        owner_pw: Password::default(),
        allow_print: true,
        allow_copy: false,
        allow_edit: false,
    };
    assert_eq!(lo::encrypt(&plain, &locked, &protect).expect("encrypt"), "AES-256");
    let bytes = std::fs::read(&locked).expect("read");
    assert!(
        contains(&bytes, b"/Encrypt") && contains(&bytes, b"/AESV3"),
        "reported strength is what is in the file"
    );
    assert!(!contains(&bytes, b"(Page 1)"), "content is not readable in the file");
    // Without an open password the file is readable by anyone: say so (SEC-HONESTY).
    let open = dir.path().join("restricted.pdf");
    let restricted = Protect {
        user_pw: Password::default(),
        ..protect.clone()
    };
    let label = lo::encrypt(&plain, &open, &restricted).expect("encrypt without open password");
    assert!(label.contains("no open password"), "{label}");
    assert!(!contains(&bytes, "s3cret".as_bytes()));

    // For checking the file by hand in other readers: SHEAF_TEST_KEEP=<dir> keeps a copy.
    if let Some(keep) = std::env::var_os("SHEAF_TEST_KEEP") {
        let _ = std::fs::copy(&locked, std::path::Path::new(&keep).join("locked.pdf"));
    }
    // Opens only with the password; a wrong one fails cleanly and reveals nothing.
    let no_pw = engine.probe(Src::plain(&locked)).expect("probe");
    assert!(no_pw.encrypted && no_pw.page_count == 0 && no_pw.has_text.is_empty());
    let wrong = Src {
        path: &locked,
        password: Some("guess"),
    };
    assert_eq!(
        engine.probe(wrong).expect_err("wrong password").code,
        ErrorCode::NeedsPassword
    );
    assert_eq!(
        engine.render(wrong, 0, (50, 50)).expect_err("wrong password").code,
        ErrorCode::NeedsPassword
    );
    let right = Src {
        path: &locked,
        password: Some("s3cret ўқғҳ"),
    };
    let opened = engine.probe(right).expect("opens with the password");
    assert_eq!(
        (opened.page_count, opened.encrypted, opened.has_text.clone()),
        (2, false, vec![true, true])
    );
    assert!(engine.page_text(right, 1, None).expect("text").contains("Page 2"));

    // TOOL-UNLOCK: with the password, the pages copy into a file that needs none.
    let mut builder = engine.builder().expect("builder");
    builder.copy_page(right, 0, Rot::R0, None, None).expect("copy");
    builder.copy_page(right, 1, Rot::R0, None, None).expect("copy");
    let unlocked = dir.path().join("unlocked.pdf");
    builder.save(&unlocked).expect("save");
    let probe = engine.probe(Src::plain(&unlocked)).expect("probe");
    assert_eq!((probe.page_count, probe.encrypted, probe.restricted), (2, false, false));
    let mut builder = engine.builder().expect("builder");
    assert_eq!(
        builder
            .copy_page(wrong, 0, Rot::R0, None, None)
            .expect_err("wrong")
            .code,
        ErrorCode::NeedsPassword
    );

    // Owner-only protection: opens for anyone, is flagged as restricted, and unlocks by copying.
    let restricted = dir.path().join("restricted.pdf");
    let owner_only = Protect {
        user_pw: Password::default(),
        owner_pw: Password::new("boss"),
        ..protect.clone()
    };
    lo::encrypt(&plain, &restricted, &owner_only).expect("encrypt");
    let probe = engine.probe(Src::plain(&restricted)).expect("opens without a password");
    assert_eq!((probe.page_count, probe.encrypted, probe.restricted), (2, false, true));

    // Protecting twice is refused rather than layered.
    let twice = lo::encrypt(&locked, &dir.path().join("twice.pdf"), &protect);
    assert_eq!(twice.expect_err("already protected").code, ErrorCode::InvalidArg);
}

fn compress_shrinks_or_keeps(engine: &Engine) {
    // TEST-COMPRESS.
    let dir = Scratch::new("pdf-compress");
    let text = dir.file("text.pdf", &fixture_gen::text_pdf(3));
    let scan = dir.path().join("scan.pdf");
    let mut builder = engine.builder().expect("builder");
    builder
        .image_page(&photo(1240, 1754), 300, Rot::R0, None, ImageFit::Fit)
        .expect("image page");
    builder
        .copy_page(Src::plain(&text), 0, Rot::R0, None, None)
        .expect("copy");
    builder.save(&scan).expect("save");
    let before = size(&scan);

    let mut last = before;
    for (level, name, max_ratio) in [
        (CompressLevel::Low, "low.pdf", 1.0),
        (CompressLevel::Recommended, "rec.pdf", 0.5),
        (CompressLevel::Extreme, "ext.pdf", 0.25),
    ] {
        let out = dir.path().join(name);
        let report = lo::compress(&scan, &out, level).expect("compress");
        assert_eq!((report.bytes_before, report.bytes_after), (before, size(&out)));
        assert!(report.bytes_after <= before, "{level:?}: never bigger");
        assert!(
            (report.bytes_after as f64) <= before as f64 * max_ratio,
            "{level:?}: {} of {before}",
            report.bytes_after
        );
        assert!(
            report.bytes_after <= last,
            "{level:?}: each level is at least as small as the one before"
        );
        last = report.bytes_after;
        assert_eq!(report.images_recompressed, u32::from(level != CompressLevel::Low));

        // Still the same document: page sizes, text, and a picture that looks like the picture.
        let probe = engine.probe(Src::plain(&out)).expect("probe");
        assert_eq!(probe.page_count, 2);
        assert_eq!(probe.has_text, [false, true], "{level:?}: text is still extractable");
        assert!(engine
            .page_text(Src::plain(&out), 1, None)
            .expect("text")
            .contains("Page 1"));
        let rendered = engine.render(Src::plain(&out), 0, (310, 439)).expect("render");
        let reference = engine.render(Src::plain(&scan), 0, (310, 439)).expect("render");
        let diff: u64 = rendered
            .pixels()
            .zip(reference.pixels())
            .map(|(a, b)| u64::from(a[0].abs_diff(b[0])))
            .sum();
        let mean = diff as f64 / f64::from(rendered.width() * rendered.height());
        assert!(mean < 12.0, "{level:?}: quality floor, mean error {mean:.1}");
    }

    // Resolution: the page shows the 1240 px image at 300 dpi, so Recommended
    // halves it to 150 dpi and Extreme takes it to 96 dpi.
    let dims = |name: &str| {
        let out = Scratch::new("pdf-compress-dims");
        let (files, _) = lo::extract_images(&dir.path().join(name), None, 0, out.path(), "img").expect("extract");
        image::image_dimensions(&files[0]).expect("dimensions")
    };
    assert_eq!(dims("low.pdf"), (1240, 1754), "lossless level leaves pixels alone");
    assert_eq!(dims("rec.pdf"), (620, 877));
    assert_eq!(dims("ext.pdf"), (397, 561));
    // An image already at or below the target is never enlarged.
    let coarse = dir.path().join("coarse.pdf");
    let mut builder = engine.builder().expect("builder");
    builder
        .image_page(&photo(400, 500), 100, Rot::R0, None, ImageFit::Fit)
        .expect("image page");
    builder.save(&coarse).expect("save");
    lo::compress(&coarse, &dir.path().join("coarse-rec.pdf"), CompressLevel::Recommended).expect("compress");
    assert_eq!(dims("coarse-rec.pdf"), (400, 500));

    // A file that cannot shrink comes back unchanged, and says so.
    let small = dir.path().join("small.pdf");
    let once = lo::compress(&text, &small, CompressLevel::Extreme).expect("compress");
    let again_path = dir.path().join("again.pdf");
    let again = lo::compress(&small, &again_path, CompressLevel::Extreme).expect("compress again");
    assert!(once.bytes_after <= size(&text));
    assert!(again.kept_original && again.bytes_after == again.bytes_before);
    assert_eq!(
        std::fs::read(&again_path).expect("read"),
        std::fs::read(&small).expect("read")
    );
    assert!(lo::compress(
        &dir.file("bad.pdf", &fixture_gen::corrupt_pdf()),
        &again_path,
        CompressLevel::Low
    )
    .is_err());
}

fn jpeg_files_pass_through_untouched(engine: &Engine) {
    // TEST-IMG2PDF: byte-identical stream, orientation, page fitting.
    let dir = Scratch::new("pdf-jpeg");
    let jpeg = sheaf_image::encode_jpeg(&photo(600, 400), 85).expect("jpeg");
    let file = dir.file("photo.jpg", &jpeg);

    let out = dir.path().join("fit.pdf");
    let mut builder = engine.builder().expect("builder");
    builder
        .jpeg_page(&file, (600, 400), 300, Rot::R0, Rot::R0, None, ImageFit::Fit)
        .expect("jpeg page");
    // The same file, stored sideways (EXIF orientation 6): shown turned, pixels untouched.
    builder
        .jpeg_page(&file, (600, 400), 300, Rot::R90, Rot::R0, None, ImageFit::Fit)
        .expect("jpeg page");
    // On A4 with a margin: centred, never enlarged.
    builder
        .jpeg_page(
            &file,
            (600, 400),
            300,
            Rot::R0,
            Rot::R0,
            None,
            ImageFit::A4 { margin_pt: 36.0 },
        )
        .expect("jpeg page");
    builder
        .image_page(
            &photo(600, 400),
            300,
            Rot::R0,
            None,
            ImageFit::Letter { margin_pt: 36.0 },
        )
        .expect("image page");
    let stats = builder.save(&out).expect("save");
    assert_eq!((stats.pages, stats.jpeg_passthrough, stats.rasterized), (4, 3, 1));
    assert!(
        contains(&std::fs::read(&out).expect("read"), &jpeg),
        "the JPEG bytes are in the PDF as they were"
    );

    let probe = engine.probe(Src::plain(&out)).expect("probe");
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(
        rounded,
        [(144, 96), (96, 144), (842, 595), (792, 612)],
        "landscape images get landscape sheets"
    );

    // Turned page: what was the left edge (reddish-dark) of the photo is now at the top.
    let upright = engine.render(Src::plain(&out), 0, (144, 96)).expect("render");
    let turned = engine.render(Src::plain(&out), 1, (96, 144)).expect("render");
    assert!(upright.get_pixel(5, 48)[0].abs_diff(turned.get_pixel(48, 5)[0]) < 30);
    assert!(upright.get_pixel(138, 48)[0].abs_diff(turned.get_pixel(48, 138)[0]) < 30);
    // A4: white margin, picture in the middle.
    let a4 = engine.render(Src::plain(&out), 2, (842, 595)).expect("render");
    assert!(a4.get_pixel(10, 10)[0] > 250 && a4.get_pixel(421, 297)[2] < 240);
    assert!(
        a4.get_pixel(421 - 80, 297)[0] > 250,
        "144 pt wide image is not stretched to the sheet"
    );

    // The embedded images come back out: the JPEG byte for byte, the bitmap as a PNG.
    let extracted = Scratch::new("pdf-jpeg-out");
    let (files, skipped) = lo::extract_images(&out, None, 0, extracted.path(), "p1").expect("extract");
    assert_eq!((files.len(), skipped), (1, 0));
    assert_eq!(std::fs::read(&files[0]).expect("read"), jpeg);
    let (files, _) = lo::extract_images(&out, None, 3, extracted.path(), "p4").expect("extract");
    assert_eq!(image::image_dimensions(&files[0]).expect("png"), (600, 400));
    assert!(lo::extract_images(&out, None, 9, extracted.path(), "x").is_err());
}

fn stamps_land_where_asked(engine: &Engine) {
    // TEST-STAMP.
    let dir = Scratch::new("pdf-stamp");
    let src = dir.file(
        "src.pdf",
        &fixture_gen::pdf(&[PageDef::no_text(), PageDef::no_text().rotated(90), PageDef::no_text()]),
    );
    let font = fixture_gen::noto_font();
    let logo = sheaf_image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255]));
    let image_of = |_: &str| -> sheaf_core::Result<sheaf_image::RgbaImage> { Ok(logo.clone()) };

    let mut numbers = Stamp::page_numbers();
    numbers.from_page = 2; // skip the first page, numbering starts there
    let mark = Stamp::watermark("Нусха ўқғҳ Oʻzbek");
    let image = Stamp {
        image: Some("logo.png".into()),
        size_pt: 80.0,
        opacity: 0.5,
        rotate_deg: 0.0,
        anchor: Anchor::TopRight,
        margin_pt: 20.0,
        to_page: Some(1),
        ..Stamp::page_numbers()
    };
    let mut builder = engine.builder().expect("builder");
    for i in 0..3 {
        builder
            .copy_page(Src::plain(&src), i, Rot::R0, None, None)
            .expect("copy");
    }
    builder.stamp(&[numbers, mark, image], &font, &image_of).expect("stamp");
    let out = dir.path().join("stamped.pdf");
    let stats = builder.save(&out).expect("save");
    assert_eq!(stats.stamps, 2 + 3 + 1);

    let text = |page: u32, within: Option<NormRect>| engine.page_text(Src::plain(&out), page, within).expect("text");
    let bottom = NormRect {
        x0: 0.3,
        y0: 0.93,
        x1: 0.7,
        y1: 1.0,
    };
    let middle = NormRect {
        x0: 0.0,
        y0: 0.2,
        x1: 1.0,
        y1: 0.8,
    };
    // Numbers: none on page 1, "1 / 3" on page 2, bottom centre as displayed, even on the turned page.
    assert_eq!(text(0, Some(bottom)).trim(), "");
    assert_eq!(
        text(1, Some(bottom)).trim(),
        "1 / 3",
        "page 2 is stored with /Rotate 90"
    );
    assert_eq!(text(2, Some(bottom)).trim(), "2 / 3");
    // Watermark: on every page, across the middle, Cyrillic and Uzbek letters intact.
    for page in 0..3 {
        assert_eq!(
            text(page, Some(middle)).trim(),
            "Нусха ўқғҳ Oʻzbek",
            "page {}",
            page + 1
        );
    }
    // Image stamp: top right of page 1 only, blended at half opacity over white.
    let page1 = engine.render(Src::plain(&out), 0, (595, 842)).expect("render");
    let p = page1.get_pixel(595 - 20 - 40, 20 + 20);
    assert!(
        p[0] > 240 && (100..160).contains(&p[1]) && (100..160).contains(&p[2]),
        "half-transparent red: {p:?}"
    );
    assert!(page1.get_pixel(30, 30)[1] > 240, "nothing in the other corner");
    let page3 = engine.render(Src::plain(&out), 2, (595, 842)).expect("render");
    assert!(
        page3.get_pixel(595 - 20 - 40, 20 + 20)[1] > 240,
        "the image stamp stops after page 1"
    );

    let mut builder = engine.builder().expect("builder");
    builder.blank_page(300.0, 300.0, Rot::R0).expect("blank");
    let bad = Stamp {
        opacity: 2.0,
        ..Stamp::page_numbers()
    };
    assert_eq!(
        builder.stamp(&[bad], &font, &image_of).expect_err("bad stamp").code,
        ErrorCode::InvalidArg
    );
    let missing_font = builder.stamp(&[Stamp::page_numbers()], &dir.path().join("none.ttf"), &image_of);
    assert_eq!(missing_font.expect_err("no font").code, ErrorCode::EngineMissing);
}

fn hide_only_crop_keeps_the_content(engine: &Engine) {
    // TEST-CROP, the "hide only" half: a CropBox changes what is shown, not what is in the file.
    let dir = Scratch::new("pdf-hide");
    let src = dir.file("src.pdf", &fixture_gen::pdf(&[PageDef::text("Confidential heading")]));
    let lower_half = NormRect {
        x0: 0.0,
        y0: 0.5,
        x1: 1.0,
        y1: 1.0,
    };
    let mut builder = engine.builder().expect("builder");
    builder
        .copy_page(Src::plain(&src), 0, Rot::R0, None, Some(lower_half))
        .expect("copy");
    let out = dir.path().join("hidden.pdf");
    builder.save(&out).expect("save");

    let probe = engine.probe(Src::plain(&out)).expect("probe");
    let (w, h) = probe.sizes_pt[0];
    assert_eq!(
        (w.round() as u32, h.round() as u32),
        (595, 421),
        "only the lower half is shown"
    );
    let shown = engine.render(Src::plain(&out), 0, (595, 421)).expect("render");
    assert!(shown.pixels().all(|p| p[0] > 200), "the heading is out of view");
    assert!(
        engine
            .page_text(Src::plain(&out), 0, None)
            .expect("text")
            .contains("Confidential heading"),
        "but it is still in the file: this mode hides, it does not remove"
    );
}

/// One test, one thread, one engine: PDFium is not thread-safe, and dropping
/// an engine tears the library down for every other user in the process
/// (ADR-WORKER-PROCS). The harness would otherwise run these in parallel.
#[test]
fn pdf_suite() {
    let engine =
        Engine::load(&fixture_gen::vendored_pdfium_dir()).expect("PDFium loads (run `bash tools/fetch-vendor` first)");
    probe_reports_pages_text_and_sizes(&engine);
    hostile_files_fail_cleanly(&engine);
    render_fits_the_box_and_draws_the_page(&engine);
    build_copies_reorders_and_rotates(&engine);
    image_pages_get_their_size_from_the_dpi(&engine);
    protect_and_unlock(&engine);
    compress_shrinks_or_keeps(&engine);
    jpeg_files_pass_through_untouched(&engine);
    stamps_land_where_asked(&engine);
    hide_only_crop_keeps_the_content(&engine);
    an_empty_build_is_refused(&engine);
    missing_library_is_reported_as_such(&engine);
}

/// A one-page PDF whose only image is `jpeg`, declared as `declared` pixels.
fn jpeg_pdf(jpeg: &[u8], declared: (u32, u32)) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /XObject << /I1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
    ];
    let content = b"q 595 0 0 842 0 0 cm /I1 Do Q";
    objects.push(
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat(),
    );
    let head = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 \
         /Filter /DCTDecode /Length {} >>\nstream\n",
        declared.0,
        declared.1,
        jpeg.len()
    );
    objects.push([head.as_bytes(), jpeg, b"\nendstream"].concat());
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn compress_believes_the_jpeg_header_not_the_dictionary() {
    // SEC-LIMITS: an image that claims to be small and decodes large must be left alone instead
    // of being decoded at whatever size it really has.
    let dir = Scratch::new("pdf-compress-lie");
    let rgb = image::DynamicImage::ImageRgba8(photo(2400, 3000)).into_rgb8();
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100);
    rgb.write_with_encoder(encoder).expect("encode");

    let honest = dir.file("honest.pdf", &jpeg_pdf(&jpeg, (2400, 3000)));
    let report =
        lo::compress(&honest, &dir.path().join("honest-out.pdf"), CompressLevel::Recommended).expect("compress");
    assert_eq!(
        report.images_recompressed, 1,
        "control: a truthful image is recompressed"
    );

    let liar = dir.file("liar.pdf", &jpeg_pdf(&jpeg, (10, 10)));
    let report = lo::compress(&liar, &dir.path().join("liar-out.pdf"), CompressLevel::Recommended).expect("compress");
    assert_eq!(report.images_recompressed, 0, "a mislabelled image is left as it is");
}
