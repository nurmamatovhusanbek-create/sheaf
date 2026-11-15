//! TEST-PRESET: presets run on files that exist already, through the real
//! worker: what comes out and where, blank pages, splitting, nothing
//! overwritten, one bad file not stopping the rest, cancel.

use fixture_gen::{
    locked_pdf, noto_font, pdf, tessdata_dir, tesseract_exe, text_pdf, vendored_pdfium_dir, PageDef, Scratch,
};
use sheaf_app::preset::run_preset;
use sheaf_app::{sibling_worker, App, Engines};
use sheaf_core::{ErrorCode, Preset};
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

fn run(app: &mut App, preset: &Preset, files: &[PathBuf], out: Option<&Path>) -> sheaf_app::quick::QuickReport {
    run_preset(app, preset, files, out, &|_, _| {}, &|| false).expect("preset run")
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

fn preset(f: impl FnOnce(&mut Preset)) -> Preset {
    let mut p = Preset {
        name: "test".into(),
        ..Preset::default()
    };
    f(&mut p);
    p
}

#[test]
fn outputs_go_beside_the_input_under_the_pattern_and_nothing_is_overwritten() {
    let base = Scratch::new("preset-base");
    let work = Scratch::new("preset-work");
    let mut app = app(&base);
    let doc = work.file("doc.pdf", &text_pdf(3));
    let p = preset(|p| p.compress = Some("Recommended".into()));

    let first = run(&mut app, &p, std::slice::from_ref(&doc), None);
    assert_eq!(names(&first), ["doc.processed.pdf"]);
    assert_eq!(first.exit_code(), 0);
    assert_eq!(pages_of(&app, &first.outputs[0]), 3);
    assert!(first.bytes_before > 0 && first.bytes_after > 0);
    let again = run(&mut app, &p, std::slice::from_ref(&doc), None);
    assert_eq!(names(&again), ["doc.processed-2.pdf"], "the first result is left alone");
    assert!(
        !first.outputs[0].to_string_lossy().starts_with(r"\\?\"),
        "paths are in the plain form"
    );
    assert_eq!(pages_of(&app, &doc), 3, "the input is untouched");
}

#[test]
fn blank_pages_are_dropped_and_a_blank_sheet_splits() {
    let base = Scratch::new("preset-base2");
    let work = Scratch::new("preset-work2");
    let mut app = app(&base);
    let doc = work.file(
        "scan.pdf",
        &pdf(&[
            PageDef::text("First"),
            PageDef::no_text(),
            PageDef::text("Second"),
            PageDef::no_text(),
        ]),
    );

    let dropped = run(
        &mut app,
        &preset(|p| p.cleanup.drop_blank = true),
        std::slice::from_ref(&doc),
        None,
    );
    assert_eq!(dropped.outputs.len(), 1);
    assert_eq!(pages_of(&app, &dropped.outputs[0]), 2, "the two blank pages are gone");

    let split = run(
        &mut app,
        &preset(|p| {
            p.split.rule = "BlankSheet".into();
            p.name_pattern = "{source}-{counter}".into();
        }),
        &[doc],
        None,
    );
    assert_eq!(names(&split), ["scan-001.pdf", "scan-002.pdf"]);
    assert!(split.outputs.iter().all(|o| pages_of(&app, o) == 1));
}

#[test]
fn the_output_folder_comes_from_the_command_line_then_the_preset() {
    let base = Scratch::new("preset-base3");
    let work = Scratch::new("preset-work3");
    let from_preset = Scratch::new("preset-out-a");
    let from_flag = Scratch::new("preset-out-b");
    let mut app = app(&base);
    let doc = work.file("a.pdf", &text_pdf(1));
    let p = preset(|p| p.out_dir = Some(from_preset.path().to_string_lossy().into_owned()));

    let r = run(&mut app, &p, std::slice::from_ref(&doc), None);
    assert_eq!(r.outputs[0].parent(), Some(from_preset.path()));
    let r = run(&mut app, &p, std::slice::from_ref(&doc), Some(from_flag.path()));
    assert_eq!(r.outputs[0].parent(), Some(from_flag.path()), "--out wins");
    assert!(
        !work.path().join("a.processed.pdf").exists(),
        "nothing beside the input"
    );

    // A folder that is not there is refused before anything runs.
    let missing = preset(|p| p.out_dir = Some(work.path().join("nope").to_string_lossy().into_owned()));
    let err = run_preset(&mut app, &missing, &[doc], None, &|_, _| {}, &|| false).expect_err("refused");
    assert!(matches!(err.code, ErrorCode::PathRejected | ErrorCode::Io), "{err}");
}

#[test]
fn a_bad_file_is_reported_and_the_others_still_run() {
    let base = Scratch::new("preset-base4");
    let work = Scratch::new("preset-work4");
    let mut app = app(&base);
    let good = work.file("good.pdf", &text_pdf(2));
    let locked = work.file("locked.pdf", &locked_pdf());
    let missing = work.path().join("missing.pdf");

    let r = run(&mut app, &preset(|_| {}), &[locked.clone(), missing, good], None);
    assert_eq!(names(&r), ["good.processed.pdf"]);
    assert_eq!(r.failures.len(), 2);
    assert_eq!(r.failures[0].0, locked);
    assert_eq!(r.failures[0].1.code, ErrorCode::NeedsPassword);
    assert_eq!(r.exit_code(), 2, "partly done");
}

#[test]
fn cleanup_and_recognition_stages_keep_the_pages() {
    let base = Scratch::new("preset-base5");
    let work = Scratch::new("preset-work5");
    let mut app = app(&base);
    let doc = work.file("form.pdf", &text_pdf(2));
    let p = preset(|p| {
        p.cleanup.mode = "gray".into();
        p.cleanup.deskew = true;
        p.cleanup.despeckle = true;
        p.ocr.mode = "Auto".into();
        p.compress = Some("Low".into());
    });
    let r = run(&mut app, &p, &[doc], None);
    assert_eq!(r.exit_code(), 0, "{:?}", r.failures);
    assert_eq!(pages_of(&app, &r.outputs[0]), 2);
}

#[test]
fn an_invalid_preset_or_a_cancel_stops_the_run() {
    let base = Scratch::new("preset-base6");
    let work = Scratch::new("preset-work6");
    let mut app = app(&base);
    let doc = work.file("a.pdf", &text_pdf(1));

    let bad = preset(|p| p.ocr.mode = "Maybe".into());
    let err = run_preset(&mut app, &bad, std::slice::from_ref(&doc), None, &|_, _| {}, &|| false).expect_err("invalid");
    assert_eq!(err.code, ErrorCode::InvalidArg);
    let err = run_preset(&mut app, &preset(|_| {}), &[], None, &|_, _| {}, &|| false).expect_err("no files");
    assert_eq!(err.code, ErrorCode::InvalidArg);

    let err = run_preset(&mut app, &preset(|_| {}), &[doc], None, &|_, _| {}, &|| true).expect_err("cancelled");
    assert_eq!(err.code, ErrorCode::Cancelled);
    assert!(
        std::fs::read_dir(work.path()).expect("dir").count() == 1,
        "a cancel before the first file leaves nothing behind"
    );
}
