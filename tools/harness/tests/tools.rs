//! PH-3 tools end to end, through the real worker pool: TEST-MERGE,
//! TEST-SPLIT, TEST-PAGEOPS (extract), TEST-CROP, TEST-COMPRESS,
//! TEST-IMG2PDF, TEST-PDF2IMG, TEST-STAMP, TEST-PROTECT.
//! Needs `bash tools/fetch-vendor` and the `sheaf-worker` binary.

use fixture_gen::{PageDef, Scratch};
use harness::{App, ExportSpec};
use sheaf_core::proto::ImageFormat;
use sheaf_core::{
    ColorMode, CompressLevel, ErrorCode, Filter, ImageFit, NormRect, Op, PageId, Password, Protect, SplitRule, Stamp,
};
use std::path::{Path, PathBuf};

fn app(base: &Scratch) -> App {
    let engines = harness::vendored().expect("sheaf-worker is built (run `cargo build --workspace`)");
    App::start(&engines, &base.path().join("sessions"), 2).expect("app starts")
}

/// Pages told apart by width: page `i` is `300 + 10 * i` points wide.
fn numbered_pdf(first: u32, count: u32) -> Vec<u8> {
    let pages: Vec<PageDef> = (first..first + count)
        .map(|i| PageDef::text(format!("p{i}")).sized(300 + 10 * i, 500))
        .collect();
    fixture_gen::pdf(&pages)
}

/// Page numbers (by width) found in a PDF.
fn sequence(app: &App, pdf: &Path) -> Vec<u32> {
    let probe = app.inspect(pdf, None).expect("inspect output");
    probe
        .sizes_pt
        .iter()
        .map(|s| ((s.0.round() as u32).saturating_sub(300)) / 10)
        .collect()
}

fn names(files: &[harness::Exported]) -> Vec<String> {
    files
        .iter()
        .map(|f| f.path.file_name().expect("name").to_string_lossy().into_owned())
        .collect()
}

fn photo(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let wave = ((x as f32 / 37.0).sin() * 40.0 + (y as f32 / 53.0).cos() * 40.0) as i32;
        let v = |base: i32| (base + wave).clamp(0, 255) as u8;
        image::Rgba([v(120 + (x * 100 / w) as i32), v(110), v(90 + (y * 120 / h) as i32), 255])
    })
}

fn write_png(dir: &Scratch, name: &str, img: &image::RgbaImage) -> PathBuf {
    dir.file(name, &sheaf_image::encode_png(img).expect("png"))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn merge_keeps_order_and_copies_pages() {
    // TEST-MERGE and TOOL-EXTRACT.
    let base = Scratch::new("tools-merge");
    let mut app = app(&base);
    let a = app.import(&base.file("a.pdf", &numbered_pdf(0, 3))).expect("import a");
    let b = app.import(&base.file("b.pdf", &numbered_pdf(10, 2))).expect("import b");
    let scan = app
        .import(&write_png(&base, "scan.png", &photo(900, 1500)))
        .expect("import image");

    let (merged, built) = app.export(base.path(), "merged.pdf").expect("merge");
    assert_eq!(
        sequence(&app, &merged)[..5],
        [0, 1, 2, 10, 11],
        "sources in workspace order"
    );
    assert_eq!(
        (built.page_count, built.copied, built.rasterized),
        (6, 5, 1),
        "PDF pages are copied, not re-encoded"
    );
    let probe = app.inspect(&merged, None).expect("inspect");
    assert_eq!(probe.has_text, [true, true, true, true, true, false]);

    // Extract: any subset, written in workspace order however it was picked.
    let picked = ExportSpec {
        pages: Some(vec![b.pages[1], a.pages[2], a.pages[0]]),
        ..ExportSpec::named("picked")
    };
    let out = app.export_pdf(base.path(), &picked).expect("extract");
    assert_eq!(sequence(&app, &out[0].path), [0, 2, 11]);
    assert_eq!(out[0].built.copied, 3);
    let missing = ExportSpec {
        pages: Some(vec![PageId(9_999)]),
        ..ExportSpec::named("x")
    };
    assert_eq!(
        app.export_pdf(base.path(), &missing).expect_err("unknown page").code,
        ErrorCode::NotFound
    );
    let _ = scan;
}

#[test]
fn forms_are_flagged_and_can_be_flattened() {
    // RISK-FORMS-MERGE: what happens to a filled-in form when its page is imported.
    let base = Scratch::new("tools-forms");
    let mut app = app(&base);
    let form = app
        .import(&base.file("form.pdf", &fixture_gen::form_pdf()))
        .expect("import form");
    assert!(form.has_forms, "the import report says so, so the UI can warn");
    let plain = app
        .import(&base.file("plain.pdf", &numbered_pdf(0, 1)))
        .expect("import plain");
    assert!(!plain.has_forms);

    // As is: the page is copied, but the result is no longer a form.
    let (kept, _) = app.export(base.path(), "kept.pdf").expect("export");
    let probe = app.inspect(&kept, None).expect("inspect");
    assert!(!probe.has_forms, "fields do not survive page import as fields");
    assert!(app
        .file_text(&kept, 0, None)
        .expect("text")
        .contains("Application form"));

    // Flattened: the value becomes part of the page, visible in every reader.
    let flat = ExportSpec {
        flatten_forms: true,
        ..ExportSpec::named("flat")
    };
    let out = app.export_pdf(base.path(), &flat).expect("export flattened");
    assert_eq!(out[0].built.flattened, 2);
    let text = app.file_text(&out[0].path, 0, None).expect("text");
    assert!(
        text.contains("Application form") && text.contains("Filled value"),
        "{text}"
    );
    assert!(!app.inspect(&out[0].path, None).expect("inspect").has_forms);
}

#[test]
fn split_rules_and_names() {
    // TEST-SPLIT.
    let base = Scratch::new("tools-split");
    let out = Scratch::new("tools-split-out");
    let mut app = app(&base);
    app.import(&base.file("batch.pdf", &numbered_pdf(0, 5)))
        .expect("import");

    let every = ExportSpec {
        split: SplitRule::EveryN(2),
        ..ExportSpec::named("{source}_{counter}")
    };
    let files = app.export_pdf(out.path(), &every).expect("split every 2");
    assert_eq!(names(&files), ["batch_001.pdf", "batch_002.pdf", "batch_003.pdf"]);
    let pages: Vec<Vec<u32>> = files.iter().map(|f| sequence(&app, &f.path)).collect();
    assert_eq!(pages, [vec![0, 1], vec![2, 3], vec![4]], "no page lost or duplicated");

    let ranges = ExportSpec {
        split: SplitRule::Ranges("4-,1".into()),
        ..ExportSpec::named("part-{n}")
    };
    let files = app.export_pdf(out.path(), &ranges).expect("split by ranges");
    assert_eq!(names(&files), ["part-1.pdf", "part-2.pdf"]);
    assert_eq!(
        files.iter().map(|f| sequence(&app, &f.path)).collect::<Vec<_>>(),
        [vec![3, 4], vec![0]]
    );

    // The same name twice never overwrites: the second file gets a suffix.
    let same = ExportSpec {
        split: SplitRule::EveryN(3),
        ..ExportSpec::named("same")
    };
    assert_eq!(
        names(&app.export_pdf(out.path(), &same).expect("split")),
        ["same.pdf", "same-2.pdf"]
    );
    assert!(names(&app.export_pdf(out.path(), &ExportSpec::named("{date}")).expect("dated"))[0].starts_with("20"));

    for bad in [
        SplitRule::EveryN(0),
        SplitRule::Ranges("9".into()),
        SplitRule::TextMatch("(".into()),
    ] {
        let spec = ExportSpec {
            split: bad.clone(),
            ..ExportSpec::named("bad")
        };
        assert_eq!(
            app.export_pdf(out.path(), &spec).expect_err("bad rule").code,
            ErrorCode::InvalidArg,
            "{bad:?}"
        );
    }
    assert!(app.export_pdf(out.path(), &ExportSpec::named("{nope}")).is_err());
    assert!(!out.path().join("bad.pdf").exists(), "a rejected export writes nothing");
}

#[test]
fn split_on_blank_sheets_and_on_text() {
    // TEST-SPLIT: the rules that have to look at the pages.
    let base = Scratch::new("tools-split2");
    let out = Scratch::new("tools-split2-out");
    let mut app = app(&base);
    let blank = || PageDef::text("").sized(420, 500);
    let page = |i: u32, text: &str| PageDef::text(text).sized(300 + 10 * i, 500);
    let batch = fixture_gen::pdf(&[
        page(0, "Shartnoma No 45"),
        page(1, "second page"),
        blank(),
        page(2, "Shartnoma No 46"),
        blank(),
        blank(),
        page(3, "Shartnoma No 47"),
        page(4, "appendix"),
    ]);
    app.import(&base.file("scan batch.pdf", &batch)).expect("import");

    // Blank separator sheets split the batch and are dropped; doubled ones make no empty file.
    let by_blank = ExportSpec {
        split: SplitRule::BlankSheet,
        ..ExportSpec::named(r"contract_{match:No\s*(\d+)}")
    };
    let files = app.export_pdf(out.path(), &by_blank).expect("split on blanks");
    assert_eq!(names(&files), ["contract_45.pdf", "contract_46.pdf", "contract_47.pdf"]);
    let pages: Vec<Vec<u32>> = files.iter().map(|f| sequence(&app, &f.path)).collect();
    assert_eq!(
        pages,
        [vec![0, 1], vec![2], vec![3, 4]],
        "separators gone, everything else kept once"
    );

    // A page whose text matches starts a new file; here the blanks stay where they are.
    let by_text = ExportSpec {
        split: SplitRule::TextMatch(r"Shartnoma\s+No".into()),
        ..ExportSpec::named("{source}-{n}")
    };
    let files = app.export_pdf(out.path(), &by_text).expect("split on text");
    assert_eq!(
        names(&files),
        ["scan batch-1.pdf", "scan batch-2.pdf", "scan batch-3.pdf"]
    );
    assert_eq!(files.iter().map(|f| f.built.page_count).collect::<Vec<_>>(), [3, 3, 2]);

    // No match on the first page: the token expands to nothing, the file still gets a name.
    let unmatched = ExportSpec {
        split: SplitRule::EveryN(8),
        ..ExportSpec::named(r"x{match:Invoice (\d+)}")
    };
    assert_eq!(
        names(&app.export_pdf(out.path(), &unmatched).expect("export")),
        ["x.pdf"]
    );
}

#[test]
fn cropping_removes_pixels_unless_told_to_hide() {
    // TEST-CROP.
    let base = Scratch::new("tools-crop");
    let mut app = app(&base);
    // A scan whose right half holds something that must not leave the building.
    let mut scan = image::RgbaImage::from_pixel(800, 600, image::Rgba([250, 250, 250, 255]));
    for y in 100..500 {
        for x in 450..750 {
            scan.put_pixel(x, y, image::Rgba([10, 10, 10, 255]));
        }
    }
    let img = app.import(&write_png(&base, "scan.png", &scan)).expect("import").pages[0];
    let left_half = NormRect {
        x0: 0.0,
        y0: 0.0,
        x1: 0.5,
        y1: 1.0,
    };
    app.ws
        .apply(Op::Crop {
            ids: vec![img],
            rect: Some(left_half),
        })
        .expect("crop");
    let (out, _) = app.export(base.path(), "cropped.pdf").expect("export");

    // What is in the file is the kept half and nothing else.
    let imported = app.import(&out).expect("re-import").pages[0];
    let (files, skipped) = app.extract_images(base.path(), "inside", imported).expect("extract");
    assert_eq!((files.len(), skipped), (1, 0));
    let inside = image::open(&files[0]).expect("embedded image").into_rgb8();
    assert_eq!(
        inside.dimensions(),
        (400, 600),
        "the cropped-away half is not in the file"
    );
    assert!(inside.pixels().all(|p| p[0] > 200), "none of its pixels survive");
    app.ws
        .apply(Op::Delete {
            ids: vec![imported, img],
        })
        .expect("clear");

    // A born-digital page: cropping rasterizes by default, so the text outside is really gone …
    let page = app
        .import(&base.file("doc.pdf", &fixture_gen::pdf(&[PageDef::text("Top secret heading")])))
        .expect("import")
        .pages[0];
    let lower_half = NormRect {
        x0: 0.0,
        y0: 0.5,
        x1: 1.0,
        y1: 1.0,
    };
    app.ws
        .apply(Op::Crop {
            ids: vec![page],
            rect: Some(lower_half),
        })
        .expect("crop");
    let (removed, built) = app.export(base.path(), "removed.pdf").expect("export");
    assert_eq!((built.copied, built.rasterized), (0, 1));
    assert_eq!(app.file_text(&removed, 0, None).expect("text").trim(), "");

    // … and "hide only" keeps the page lossless but leaves the content in the file, as its name says.
    let hide = ExportSpec {
        crop_hide_only: true,
        ..ExportSpec::named("hidden")
    };
    let hidden = app.export_pdf(base.path(), &hide).expect("export hidden");
    assert_eq!((hidden[0].built.copied, hidden[0].built.rasterized), (1, 0));
    assert!(app
        .file_text(&hidden[0].path, 0, None)
        .expect("text")
        .contains("Top secret heading"));
    let shown = app.inspect(&hidden[0].path, None).expect("inspect").sizes_pt[0];
    assert_eq!((shown.0.round() as u32, shown.1.round() as u32), (595, 421));
}

#[test]
fn compression_reports_honestly() {
    // TEST-COMPRESS, through the export path.
    let base = Scratch::new("tools-compress");
    let mut app = app(&base);
    app.import(&write_png(&base, "scan.png", &photo(1240, 1754)))
        .expect("import");
    app.import(&base.file("text.pdf", &fixture_gen::text_pdf(1)))
        .expect("import");

    let plain = app
        .export_pdf(base.path(), &ExportSpec::named("plain"))
        .expect("export");
    assert_eq!(plain[0].built.bytes_before_compress, None);
    let size = |p: &Path| std::fs::metadata(p).expect("stat").len();
    assert_eq!(plain[0].built.bytes, size(&plain[0].path));

    let small = ExportSpec {
        compress: Some(CompressLevel::Recommended),
        ..ExportSpec::named("small")
    };
    let small = &app.export_pdf(base.path(), &small).expect("export compressed")[0];
    assert_eq!(
        small.built.bytes_before_compress,
        Some(plain[0].built.bytes),
        "before/after sizes are reported"
    );
    assert_eq!(small.built.bytes, size(&small.path));
    assert!(small.built.bytes * 2 < plain[0].built.bytes && !small.built.compress_kept_original);
    assert!(
        app.file_text(&small.path, 1, None).expect("text").contains("Page 1"),
        "text still extractable"
    );
    assert_eq!(app.inspect(&small.path, None).expect("inspect").page_count, 2);
}

#[test]
fn images_become_pdf_pages() {
    // TEST-IMG2PDF.
    let base = Scratch::new("tools-img2pdf");
    let mut app = app(&base);
    let jpeg = sheaf_image::encode_jpeg(&photo(600, 400), 85).expect("jpeg");
    let plain = app.import(&base.file("photo.jpg", &jpeg)).expect("import jpeg").pages[0];
    let sideways = base.file("sideways.jpg", &fixture_gen::jpeg_with_orientation(&jpeg, 6));
    let turned = app.import(&sideways).expect("import turned jpeg").pages[0];

    // A multipage TIFF brings one page per image.
    let tiff_path = base.path().join("pages.tif");
    {
        use tiff::encoder::{colortype, TiffEncoder};
        let mut enc = TiffEncoder::new(std::fs::File::create(&tiff_path).expect("create")).expect("encoder");
        for shade in [40u8, 120, 220] {
            enc.write_image::<colortype::RGB8>(300, 200, &vec![shade; 300 * 200 * 3])
                .expect("tiff page");
        }
    }
    let tiff = app.import(&tiff_path).expect("import tiff");
    assert_eq!(tiff.pages.len(), 3);

    let out = &app
        .export_pdf(base.path(), &ExportSpec::named("album"))
        .expect("export")[0];
    assert_eq!(
        (out.built.page_count, out.built.jpeg_passthrough, out.built.rasterized),
        (5, 2, 3)
    );
    let bytes = std::fs::read(&out.path).expect("read");
    assert!(contains(&bytes, &jpeg), "the JPEG is embedded byte for byte");
    let probe = app.inspect(&out.path, None).expect("inspect");
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(
        rounded,
        [(144, 96), (96, 144), (72, 48), (72, 48), (72, 48)],
        "EXIF orientation turns the page"
    );

    // TIFF pages came through in order: dark, mid, light.
    let folder = app
        .export_images(base.path(), "check", Some(tiff.pages.clone()), ImageFormat::Png, 300)
        .expect("images");
    let shades: Vec<u8> = folder
        .iter()
        .map(|f| image::open(f).expect("png").into_rgb8().get_pixel(10, 10)[0])
        .collect();
    assert_eq!(shades, [40, 120, 220]);

    // Any cleanup means the pixels change, so the JPEG is re-encoded rather than passed through.
    let gray = Filter {
        mode: ColorMode::Gray,
        ..Default::default()
    };
    app.ws
        .apply(Op::SetFilter {
            ids: vec![plain],
            filter: gray,
        })
        .expect("filter");
    let on_a4 = ExportSpec {
        pages: Some(vec![plain, turned]),
        image_fit: ImageFit::A4 { margin_pt: 36.0 },
        ..ExportSpec::named("a4")
    };
    let a4 = &app.export_pdf(base.path(), &on_a4).expect("export a4")[0];
    assert_eq!((a4.built.jpeg_passthrough, a4.built.rasterized), (1, 1));
    let probe = app.inspect(&a4.path, None).expect("inspect");
    let rounded: Vec<(u32, u32)> = probe
        .sizes_pt
        .iter()
        .map(|s| (s.0.round() as u32, s.1.round() as u32))
        .collect();
    assert_eq!(
        rounded,
        [(842, 595), (595, 842)],
        "each image gets a sheet in its own orientation"
    );
}

#[test]
fn pages_become_images() {
    // TEST-PDF2IMG.
    let base = Scratch::new("tools-pdf2img");
    let out = Scratch::new("tools-pdf2img-out");
    let mut app = app(&base);
    let doc = app.import(&base.file("doc.pdf", &numbered_pdf(0, 3))).expect("import");
    app.ws
        .apply(Op::Rotate {
            ids: vec![doc.pages[2]],
            delta: sheaf_core::Rot::R90,
        })
        .expect("rotate");

    let files = app
        .export_images(out.path(), "doc pages", None, ImageFormat::Png, 150)
        .expect("export images");
    let names: Vec<String> = files
        .iter()
        .map(|f| f.file_name().expect("name").to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["page-001.png", "page-002.png", "page-003.png"]);
    assert!(
        files
            .iter()
            .all(|f| f.parent() == Some(out.path().join("doc pages").as_path())),
        "one new folder"
    );
    let dims: Vec<(u32, u32)> = files.iter().map(|f| image::image_dimensions(f).expect("png")).collect();
    assert_eq!(
        dims,
        [(625, 1042), (646, 1042), (1042, 667)],
        "150 dpi, as displayed (the third page is turned)"
    );

    // The folder is never reused, and a page range in JPEG works the same way.
    let again = app
        .export_images(out.path(), "doc pages", Some(vec![doc.pages[1]]), ImageFormat::Jpg, 72)
        .expect("again");
    assert_eq!(again.len(), 1);
    assert_eq!(again[0], out.path().join("doc pages-2").join("page-001.jpg"));
    assert_eq!(image::image_dimensions(&again[0]).expect("jpg"), (310, 500));
    assert_eq!(
        std::fs::read_dir(out.path().join("doc pages")).expect("list").count(),
        3,
        "first folder untouched"
    );

    assert!(
        app.export_images(out.path(), "bad", None, ImageFormat::Png, 5).is_err(),
        "resolution out of range"
    );
    assert!(app
        .export_images(out.path(), "none", Some(vec![]), ImageFormat::Png, 150)
        .is_err());
}

#[test]
fn stamps_through_export() {
    // TEST-STAMP, through the export path.
    let base = Scratch::new("tools-stamp");
    let mut app = app(&base);
    app.import(&base.file("doc.pdf", &numbered_pdf(0, 3))).expect("import");
    let logo = write_png(
        &base,
        "logo.png",
        &image::RgbaImage::from_pixel(60, 30, image::Rgba([200, 0, 0, 255])),
    );

    let mut numbers = Stamp::page_numbers();
    numbers.text = "{n}-bet / {N}".into();
    let mut image = Stamp {
        image: Some(logo.to_string_lossy().into_owned()),
        size_pt: 60.0,
        ..Stamp::page_numbers()
    };
    image.anchor = sheaf_core::Anchor::TopLeft;
    let spec = ExportSpec {
        stamps: vec![numbers, Stamp::watermark("НУСХА"), image],
        ..ExportSpec::named("stamped")
    };
    let out = &app.export_pdf(base.path(), &spec).expect("export")[0];
    assert_eq!(
        (out.built.stamps, out.built.copied),
        (9, 3),
        "stamping does not rasterize the pages"
    );
    for page in 0..3 {
        let text = app.file_text(&out.path, page, None).expect("text");
        assert!(
            text.contains(&format!("{}-bet / 3", page + 1)) && text.contains("НУСХА"),
            "page {}: {text}",
            page + 1
        );
        assert!(text.contains(&format!("p{page}")), "the page's own text is still there");
    }
    let (images, _) = {
        let id = app.import(&out.path).expect("re-import").pages[0];
        app.extract_images(base.path(), "stamp-img", id).expect("extract")
    };
    assert_eq!(images.len(), 1, "the logo is on the page");

    // A stamp image is input like any other file.
    let missing = Stamp {
        image: Some(base.path().join("nope.png").to_string_lossy().into_owned()),
        ..Stamp::page_numbers()
    };
    let spec = ExportSpec {
        stamps: vec![missing],
        ..ExportSpec::named("bad")
    };
    assert_eq!(
        app.export_pdf(base.path(), &spec).expect_err("no such image").code,
        ErrorCode::NotFound
    );
    let invalid = ExportSpec {
        stamps: vec![Stamp {
            opacity: 7.0,
            ..Stamp::page_numbers()
        }],
        ..ExportSpec::named("bad")
    };
    assert_eq!(
        app.export_pdf(base.path(), &invalid).expect_err("bad stamp").code,
        ErrorCode::InvalidArg
    );

    // Without the bundled font, stamping fails cleanly; plain export still works.
    let mut engines = harness::vendored().expect("engines");
    engines.font = None;
    let mut bare = App::start(&engines, &base.path().join("sessions-nofont"), 1).expect("app");
    bare.import(&base.path().join("doc.pdf")).expect("import");
    let spec = ExportSpec {
        stamps: vec![Stamp::page_numbers()],
        ..ExportSpec::named("nofont")
    };
    assert_eq!(
        bare.export_pdf(base.path(), &spec).expect_err("no font").code,
        ErrorCode::EngineMissing
    );
    bare.export(base.path(), "plain.pdf").expect("plain export");
}

/// Every file under `dir`, read whole.
fn all_bytes(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(all_bytes(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((path, bytes));
        }
    }
    out
}

#[test]
fn protect_then_unlock() {
    // TEST-PROTECT.
    let base = Scratch::new("tools-protect");
    let out = Scratch::new("tools-protect-out");
    let mut app = app(&base);
    app.import(&base.file("report.pdf", &numbered_pdf(0, 2)))
        .expect("import");
    let secret = "Tr0ub4dor-қулф";
    let protect = Protect {
        user_pw: Password::new(secret),
        owner_pw: Password::default(),
        allow_print: true,
        allow_copy: false,
        allow_edit: false,
    };
    let spec = ExportSpec {
        protect: Some(protect.clone()),
        compress: Some(CompressLevel::Low),
        ..ExportSpec::named("locked")
    };
    let locked = app.export_pdf(out.path(), &spec).expect("export protected").remove(0);
    assert_eq!(
        locked.built.encryption.as_deref(),
        Some("AES-256"),
        "the strength reported is the one applied"
    );
    assert!(!contains(&std::fs::read(&locked.path).expect("read"), b"(p0)"));

    // The password is nowhere on disk: not in the output folder, not in the session dir.
    for (path, bytes) in all_bytes(out.path()).into_iter().chain(all_bytes(app.session_dir())) {
        assert!(
            !contains(&bytes, secret.as_bytes()),
            "password found in {}",
            path.display()
        );
    }

    // Opening it: locked, nothing revealed; a wrong password fails cleanly; the right one brings the pages in.
    let report = app.import(&locked.path).expect("import locked file");
    assert!(report.encrypted && report.pages.is_empty());
    let before = app.ws.len();
    assert_eq!(
        app.unlock(report.source, Password::new("wrong"))
            .expect_err("wrong")
            .code,
        ErrorCode::NeedsPassword
    );
    assert_eq!(
        app.unlock(report.source, Password::default()).expect_err("empty").code,
        ErrorCode::NeedsPassword
    );
    assert_eq!(app.ws.len(), before, "a failed unlock adds nothing");
    assert!(app.ws.source(report.source).is_some_and(|s| !s.unlocked));
    let pages = app.unlock(report.source, Password::new(secret)).expect("unlock");
    assert_eq!(pages.len(), 2);
    assert!(
        app.unlock(report.source, Password::new(secret)).is_err(),
        "already unlocked"
    );
    assert_eq!(app.thumbs(&pages).expect("thumbnails of unlocked pages").len(), 2);

    // TOOL-UNLOCK: exported again without protection, it opens for anyone.
    let spec = ExportSpec {
        pages: Some(pages),
        ..ExportSpec::named("unlocked")
    };
    let unlocked = app.export_pdf(out.path(), &spec).expect("export unlocked").remove(0);
    let probe = app.inspect(&unlocked.path, None).expect("inspect");
    assert_eq!((probe.page_count, probe.encrypted, probe.restricted), (2, false, false));
    assert_eq!(sequence(&app, &unlocked.path), [0, 1]);
    assert_eq!(unlocked.built.encryption, None);
    for (path, bytes) in all_bytes(app.session_dir()) {
        assert!(
            !contains(&bytes, secret.as_bytes()),
            "password found in {}",
            path.display()
        );
    }

    // Owner-only protection: opens freely, is flagged, and exporting lifts the restrictions.
    let owner_only = Protect {
        user_pw: Password::default(),
        owner_pw: Password::new("boss"),
        ..protect
    };
    let spec = ExportSpec {
        protect: Some(owner_only),
        ..ExportSpec::named("restricted")
    };
    let restricted = app.export_pdf(out.path(), &spec).expect("export restricted").remove(0);
    let mut fresh = App::start(
        &harness::vendored().expect("engines"),
        &base.path().join("sessions2"),
        1,
    )
    .expect("app");
    let report = fresh.import(&restricted.path).expect("import restricted file");
    assert!(!report.encrypted && report.restricted && !report.pages.is_empty());
    let (free, _) = fresh.export(out.path(), "free.pdf").expect("export");
    assert!(!fresh.inspect(&free, None).expect("inspect").restricted);
}
