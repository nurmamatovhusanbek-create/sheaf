//! TEST-REPAIR: damaged files come back as new files with the page count the renderer
//! really sees; what cannot be recovered is reported, not faked; nothing is overwritten.

use fixture_gen::{
    corrupt_pdf, locked_pdf, noto_font, tessdata_dir, tesseract_exe, text_pdf, vendored_pdfium_dir, Scratch,
};
use sheaf_app::plan::NoCtl;
use sheaf_app::repair::repair_files;
use sheaf_app::{sibling_worker, App, Engines};
use sheaf_core::ErrorCode;
use std::path::PathBuf;

fn app(base: &Scratch) -> App {
    let engines = Engines {
        worker: sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)"),
        pdfium_dir: vendored_pdfium_dir(),
        tesseract: Some((tesseract_exe(), tessdata_dir())),
        font: Some(noto_font()),
    };
    App::start(&engines, &base.path().join("sessions"), 2).expect("app starts")
}

fn run(app: &App, files: &[PathBuf]) -> sheaf_app::repair::RepairReport {
    repair_files(&app.shared(), files, None, &|_, _| {}, &NoCtl).expect("repair run")
}

#[test]
fn a_file_with_its_tail_cut_off_is_rebuilt_with_all_its_pages() {
    let base = Scratch::new("rep-base");
    let work = Scratch::new("rep-work");
    let app = app(&base);
    let good = text_pdf(5);
    let cut = work.file("cut.pdf", &good[..good.len() - 80]);
    assert!(
        app.inspect(&cut, None).is_err(),
        "the damaged file really does not open"
    );

    let r = run(&app, std::slice::from_ref(&cut));
    assert_eq!(r.quick.exit_code(), 0, "{:?}", r.quick.failures);
    assert_eq!(r.repaired.len(), 1);
    assert_eq!(r.repaired[0].pages, 5);
    assert!(r.repaired[0].was_damaged);
    assert_eq!(r.repaired[0].output.file_name().expect("name"), "cut.repaired.pdf");
    assert_eq!(
        app.inspect(&r.repaired[0].output, None).expect("opens now").page_count,
        5
    );
    assert!(
        !r.repaired[0].output.to_string_lossy().starts_with(r"\\?\"),
        "paths are in the plain form"
    );

    // A second run never replaces the first result.
    let again = run(&app, &[cut]);
    assert_eq!(
        again.repaired[0].output.file_name().expect("name"),
        "cut.repaired-2.pdf"
    );
}

#[test]
fn a_wrong_cross_reference_table_is_a_repair_too_and_a_healthy_file_is_not_called_damaged() {
    let base = Scratch::new("rep-base2");
    let work = Scratch::new("rep-work2");
    let app = app(&base);
    let good = text_pdf(3);
    let start = good.windows(9).rposition(|w| w == b"startxref").expect("startxref");
    let mut wrong = good[..start].to_vec();
    wrong.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    let wrong = work.file("wrong.pdf", &wrong);
    let healthy = work.file("healthy.pdf", &good);

    let r = run(&app, &[wrong, healthy]);
    assert_eq!(r.quick.exit_code(), 0, "{:?}", r.quick.failures);
    assert_eq!(r.repaired.iter().map(|x| x.pages).collect::<Vec<_>>(), [3, 3]);
    assert!(!r.repaired[1].was_damaged, "an intact file is reported as intact");
}

#[test]
fn what_cannot_be_recovered_is_reported_and_the_others_still_run() {
    let base = Scratch::new("rep-base3");
    let work = Scratch::new("rep-work3");
    let app = app(&base);
    let junk = work.file("junk.pdf", &corrupt_pdf());
    let locked = work.file("locked.pdf", &locked_pdf());
    let text = work.file("notes.txt", b"just text");
    let missing = work.path().join("missing.pdf");
    let good = text_pdf(2);
    let ok = work.file("ok.pdf", &good[..good.len() - 80]);

    let r = run(&app, &[junk.clone(), locked.clone(), text, missing, ok]);
    assert_eq!(r.quick.exit_code(), 2, "partly done");
    assert_eq!(r.repaired.len(), 1);
    let why = |p: &PathBuf| r.quick.failures.iter().find(|(f, _)| f == p).map(|(_, e)| e.code);
    assert_eq!(
        why(&junk),
        Some(ErrorCode::ParseError),
        "nothing readable is an error, not an empty file"
    );
    assert_eq!(why(&locked), Some(ErrorCode::NeedsPassword));
    assert_eq!(r.quick.failures.len(), 4);
    assert!(
        !work.path().join("junk.repaired.pdf").exists(),
        "no file is made up for garbage"
    );
}

#[test]
fn the_output_folder_can_be_chosen_and_a_cancel_stops_the_run() {
    let base = Scratch::new("rep-base4");
    let work = Scratch::new("rep-work4");
    let out = Scratch::new("rep-out4");
    let app = app(&base);
    let good = text_pdf(2);
    let cut = work.file("a.pdf", &good[..good.len() - 80]);
    let r = repair_files(
        &app.shared(),
        std::slice::from_ref(&cut),
        Some(out.path()),
        &|_, _| {},
        &NoCtl,
    )
    .expect("run");
    assert_eq!(r.repaired[0].output.parent(), Some(out.path()));

    struct Cancelled;
    impl sheaf_app::plan::JobCtl for Cancelled {
        fn cancelled(&self) -> bool {
            true
        }
        fn progress(&self, _: f32) {}
    }
    let err = repair_files(&app.shared(), &[cut], None, &|_, _| {}, &Cancelled).expect_err("cancelled");
    assert_eq!(err.code, ErrorCode::Cancelled);
    assert_eq!(
        repair_files(&app.shared(), &[], None, &|_, _| {}, &NoCtl)
            .expect_err("none")
            .code,
        ErrorCode::InvalidArg
    );
}
