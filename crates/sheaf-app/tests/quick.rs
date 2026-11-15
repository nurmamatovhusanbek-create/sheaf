//! The headless quick actions behind the action wheel and `sheaf run`, through
//! the real worker: what comes out, where, and that nothing is overwritten.

use fixture_gen::{noto_font, tessdata_dir, tesseract_exe, text_pdf, vendored_pdfium_dir, Scratch};
use sheaf_app::quick::run_quick;
use sheaf_app::{sibling_worker, App, Engines};
use sheaf_core::{ErrorCode, QuickAction, QuickRequest};
use std::path::{Path, PathBuf};

fn app(base: &Scratch) -> App {
    let engines = Engines {
        worker: sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)"),
        pdfium_dir: vendored_pdfium_dir(),
        tesseract: Some((tesseract_exe(), tessdata_dir())),
        font: Some(noto_font()),
    };
    App::start(&engines, &base.path().join("sessions"), 2).expect("app starts")
}

fn png(dir: &Scratch, name: &str, shade: u8) -> PathBuf {
    let img = image::RgbImage::from_fn(60, 80, |x, y| image::Rgb([shade, (x * 4) as u8, (y * 3) as u8]));
    let path = dir.path().join(name);
    img.save(&path).expect("write png");
    path
}

fn names(report: &sheaf_app::quick::QuickReport) -> Vec<String> {
    report
        .outputs
        .iter()
        .map(|p| p.file_name().expect("name").to_string_lossy().into_owned())
        .collect()
}

fn pages_of(app: &App, path: &Path) -> u32 {
    app.inspect(path, None).expect("inspect").page_count
}

fn run(app: &mut App, action: QuickAction, files: Vec<PathBuf>) -> sheaf_app::quick::QuickReport {
    let report = run_quick(app, &QuickRequest::new(action, files), &|_, _| {}).expect("quick run");
    // Paths are shown to people and passed to scripts: never in the `\\?\` form.
    assert!(
        report.outputs.iter().all(|p| !p.to_string_lossy().starts_with(r"\\?\")),
        "{:?}",
        report.outputs
    );
    report
}

#[test]
fn merge_joins_pdfs_and_images_in_the_order_given() {
    let base = Scratch::new("quick-base");
    let work = Scratch::new("quick-work");
    let mut app = app(&base);
    let a = work.file("a.pdf", &text_pdf(2));
    let b = work.file("b.pdf", &text_pdf(3));
    let img = png(&work, "c.png", 200);
    let report = run(&mut app, QuickAction::Merge, vec![b.clone(), img, a.clone()]);
    assert_eq!(report.exit_code(), 0, "{:?}", report.failures);
    assert_eq!(names(&report), ["b.merged.pdf"], "named after the first input");
    assert_eq!(pages_of(&app, &report.outputs[0]), 6);
    assert_eq!(report.outputs[0].parent(), b.parent(), "beside the first input");
    // The inputs are untouched.
    assert_eq!(pages_of(&app, &a), 2);
    // Running it again does not replace the first result.
    let again = run(&mut app, QuickAction::Merge, vec![b, a]);
    assert_eq!(names(&again), ["b.merged-2.pdf"]);
}

#[test]
fn compress_split_rotate_and_pdf_to_images_make_one_result_per_input() {
    let base = Scratch::new("quick-base2");
    let work = Scratch::new("quick-work2");
    let out = Scratch::new("quick-out2");
    let mut app = app(&base);
    let a = work.file("a.pdf", &text_pdf(3));
    let b = work.file("b.pdf", &text_pdf(1));

    let mut req = QuickRequest::new(QuickAction::Compress, vec![a.clone(), b.clone()]);
    req.out = Some(out.path().to_path_buf());
    let r = run_quick(&mut app, &req, &|_, _| {}).expect("compress");
    assert_eq!(names(&r), ["a.compressed.pdf", "b.compressed.pdf"]);
    assert!(r.bytes_before > 0 && r.bytes_after > 0);
    assert!(r.outputs.iter().all(|p| p.starts_with(out.path())));

    let r = run(&mut app, QuickAction::Split, vec![a.clone()]);
    assert_eq!(names(&r), ["a-001.pdf", "a-002.pdf", "a-003.pdf"]);
    assert!(r.outputs.iter().all(|p| pages_of(&app, p) == 1));

    let r = run(&mut app, QuickAction::Rotate, vec![b.clone()]);
    assert_eq!(names(&r), ["b.rotated.pdf"]);

    let r = run(&mut app, QuickAction::PdfToImages, vec![a]);
    assert_eq!(r.outputs.len(), 1, "one folder");
    assert!(r.outputs[0].is_dir() && r.outputs[0].ends_with("a-images"));
    assert_eq!(std::fs::read_dir(&r.outputs[0]).expect("ls").count(), 3);
}

#[test]
fn images_to_pdf_makes_one_file_with_a_page_per_image() {
    let base = Scratch::new("quick-base3");
    let work = Scratch::new("quick-work3");
    let mut app = app(&base);
    let r = run(
        &mut app,
        QuickAction::ImagesToPdf,
        vec![
            png(&work, "scan1.png", 10),
            png(&work, "scan2.png", 90),
            png(&work, "scan3.png", 170),
        ],
    );
    assert_eq!(r.exit_code(), 0, "{:?}", r.failures);
    assert_eq!(names(&r), ["scan1.pdf"]);
    assert_eq!(pages_of(&app, &r.outputs[0]), 3);
}

#[test]
fn ocr_makes_a_searchable_copy_and_says_so_when_it_cannot() {
    let base = Scratch::new("quick-base4");
    let work = Scratch::new("quick-work4");
    let mut app = app(&base);
    let a = work.file("born-digital.pdf", &text_pdf(2));
    let r = run(&mut app, QuickAction::Ocr, vec![a]);
    assert_eq!(r.exit_code(), 0, "{:?}", r.failures);
    assert_eq!(names(&r), ["born-digital.ocr.pdf"]);
    let text = app.file_text(&r.outputs[0], 0, None).expect("text");
    assert!(text.contains("Page 1"), "{text}");
}

#[test]
fn a_bad_input_is_reported_and_the_others_still_run() {
    let base = Scratch::new("quick-base5");
    let work = Scratch::new("quick-work5");
    let mut app = app(&base);
    let good = work.file("good.pdf", &text_pdf(1));
    let junk = work.file("junk.pdf", b"not a pdf at all");
    let missing = work.path().join("missing.pdf");
    let r = run(
        &mut app,
        QuickAction::Compress,
        vec![junk.clone(), good, missing.clone()],
    );
    assert_eq!(names(&r), ["good.compressed.pdf"]);
    assert_eq!(r.failures.len(), 2);
    assert_eq!(r.failures[0].0, junk);
    assert_eq!(r.failures[1].1.code, ErrorCode::NotFound);
    assert_eq!(r.exit_code(), 2, "partly done");
    // Nothing at all done is a plain failure.
    let r = run(&mut app, QuickAction::Compress, vec![missing]);
    assert_eq!(r.exit_code(), 1);
}

#[test]
fn a_locked_file_is_reported_not_guessed() {
    let base = Scratch::new("quick-base6");
    let work = Scratch::new("quick-work6");
    let mut app = app(&base);
    let a = work.file("plain.pdf", &text_pdf(1));
    let report = {
        use sheaf_core::{Password, Protect};
        app.import(&a).expect("import");
        let spec = sheaf_app::ExportSpec {
            protect: Some(Protect {
                user_pw: Password::new("pw"),
                owner_pw: Password::default(),
                allow_print: true,
                allow_copy: true,
                allow_edit: false,
            }),
            ..sheaf_app::ExportSpec::named("locked")
        };
        app.export_pdf(work.path(), &spec).expect("lock").remove(0)
    };
    let r = run(&mut app, QuickAction::Compress, vec![report.path]);
    assert!(r.outputs.is_empty());
    assert_eq!(r.failures[0].1.code, ErrorCode::NeedsPassword);
}

#[test]
fn an_output_folder_that_does_not_exist_is_refused() {
    let base = Scratch::new("quick-base7");
    let work = Scratch::new("quick-work7");
    let mut app = app(&base);
    let a = work.file("a.pdf", &text_pdf(1));
    let mut req = QuickRequest::new(QuickAction::Compress, vec![a]);
    req.out = Some(work.path().join("nope"));
    assert!(run_quick(&mut app, &req, &|_, _| {}).is_err());
}
