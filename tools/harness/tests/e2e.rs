//! PH-1 end to end: import, page ops with undo, thumbnails and export through
//! the real worker pool. Covers TEST-PAGEOPS at the file level and the
//! "session dir removed on exit" item of TEST-SEC.
//! Needs `bash tools/fetch-vendor` and the `sheaf-worker` binary (built by
//! `cargo test --workspace` / `cargo build --workspace`).

use fixture_gen::{PageDef, Scratch};
use harness::App;
use sheaf_core::{ErrorCode, Op, PageId, Rot};

fn app(base: &Scratch) -> App {
    let worker =
        harness::sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)");
    App::start(
        &worker,
        &fixture_gen::vendored_pdfium_dir(),
        &base.path().join("sessions"),
        2,
    )
    .expect("app starts")
}

/// Pages told apart by width: page `i` is `300 + 10 * i` points wide.
fn numbered_pdf(first: u32, count: u32) -> Vec<u8> {
    let pages: Vec<PageDef> = (first..first + count)
        .map(|i| PageDef::text(format!("p{i}")).sized(300 + 10 * i, 500))
        .collect();
    fixture_gen::pdf(&pages)
}

#[test]
fn import_edit_undo_export() {
    let base = Scratch::new("e2e");
    let out_dir = Scratch::new("e2e-out");
    let a = base.file("a.pdf", &numbered_pdf(0, 4));
    let b = base.file("b.pdf", &numbered_pdf(10, 2));
    let mut app = app(&base);
    let session_dir = app.session_dir().to_path_buf();

    let ra = app.import(&a).expect("import a");
    let rb = app.import(&b).expect("import b");
    assert_eq!((ra.pages.len(), rb.pages.len(), app.ws.len()), (4, 2, 6));
    assert!(app.ws.all_pages().all(|p| p.has_text == Some(true)));

    // Thumbnails are PNG files inside the session dir.
    let thumbs = app.thumbs(&ra.pages).expect("thumbnails");
    assert_eq!(thumbs.len(), 4);
    for t in &thumbs {
        assert!(t.starts_with(&session_dir));
        assert_eq!(&std::fs::read(t).expect("thumbnail exists")[1..4], b"PNG");
    }

    // TOOL-REMOVE, TOOL-ORGANIZE, TOOL-ROTATE, then one undo.
    app.ws
        .apply(Op::Delete {
            ids: vec![ra.pages[1]],
        })
        .expect("delete");
    app.ws
        .apply(Op::Move {
            ids: vec![rb.pages[0], rb.pages[1]],
            to: 0,
        })
        .expect("move");
    app.ws
        .apply(Op::Rotate {
            ids: vec![ra.pages[0]],
            delta: Rot::R90,
        })
        .expect("rotate");
    app.ws
        .apply(Op::Delete {
            ids: vec![ra.pages[3]],
        })
        .expect("delete");
    app.ws.undo().expect("undo the last delete");
    let order: Vec<PageId> = app.ws.all_pages().map(|p| p.id).collect();
    assert_eq!(
        order,
        [
            rb.pages[0],
            rb.pages[1],
            ra.pages[0],
            ra.pages[2],
            ra.pages[3]
        ]
    );

    let (out, built) = app.export(out_dir.path(), "merged.pdf").expect("export");
    assert_eq!(out, out_dir.path().join("merged.pdf"));
    assert_eq!(
        (built.page_count, built.copied, built.rasterized),
        (5, 5, 0),
        "pure page ops never re-encode"
    );

    // Read the result back: sequence (by width) and rotation (a0 is now landscape).
    let report = app.import(&out).expect("re-import output");
    assert_eq!(report.pages.len(), 5);
    assert!(report
        .pages
        .iter()
        .all(|id| app.ws.find(*id).is_some_and(|p| p.has_text == Some(true))));
    let dims: Vec<(u32, u32)> = app
        .thumbs(&report.pages)
        .expect("thumbnails of the output")
        .iter()
        .map(|t| {
            let bytes = std::fs::read(t).expect("thumbnail");
            let be =
                |o: usize| u32::from_be_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
            (be(16), be(20)) // PNG IHDR width, height
        })
        .collect();
    let expect = |w_pt: u32, h_pt: u32| {
        let scale = 256.0 / w_pt.max(h_pt) as f64;
        (
            (w_pt as f64 * scale).round() as u32,
            (h_pt as f64 * scale).round() as u32,
        )
    };
    assert_eq!(
        dims,
        [
            expect(400, 500),
            expect(410, 500),
            expect(500, 300),
            expect(320, 500),
            expect(330, 500)
        ],
        "b0, b1, a0 turned 90°, a2, a3"
    );

    // ADR-NO-OVERWRITE: the same name again gets a suffix; the first file is untouched.
    let first_bytes = std::fs::read(&out).expect("read first export");
    app.ws
        .apply(Op::Delete { ids: report.pages })
        .expect("drop the re-imported pages");
    let (second, _) = app
        .export(out_dir.path(), "merged.pdf")
        .expect("second export");
    assert_eq!(second, out_dir.path().join("merged-2.pdf"));
    assert_eq!(
        std::fs::read(&out).expect("first export still there"),
        first_bytes
    );
    // The inputs were never written to.
    assert_eq!(std::fs::read(&a).expect("input a"), numbered_pdf(0, 4));

    app.close().expect("close");
    assert!(!session_dir.exists(), "session dir is deleted on exit");
}

#[test]
fn bad_inputs_fail_cleanly_and_the_app_carries_on() {
    let base = Scratch::new("e2e-bad");
    let mut app = app(&base);

    let corrupt = base.file("corrupt.pdf", &fixture_gen::corrupt_pdf());
    assert_eq!(
        app.import(&corrupt).expect_err("corrupt").code,
        ErrorCode::ParseError
    );
    let text = base.file("notes.txt", b"hello");
    assert_eq!(
        app.import(&text).expect_err("not a document").code,
        ErrorCode::Unsupported
    );
    assert_eq!(
        app.import(&base.path().join("missing.pdf"))
            .expect_err("missing")
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        app.import(base.path()).expect_err("directory").code,
        ErrorCode::PathRejected
    );
    assert!(app.ws.is_empty(), "failed imports add nothing");
    assert!(!app.ws.can_undo());

    // A locked file is reported as such and reveals no pages.
    let locked = base.file("locked.pdf", &fixture_gen::locked_pdf());
    let report = app.import(&locked).expect("import of a locked file");
    assert!(report.encrypted && report.pages.is_empty());
    assert!(app
        .ws
        .source(report.source)
        .is_some_and(|s| s.encrypted && !s.unlocked));

    assert_eq!(
        app.export(base.path(), "empty.pdf")
            .expect_err("nothing to export")
            .code,
        ErrorCode::InvalidArg
    );
    assert!(!base.path().join("empty.pdf").exists());

    // Still fully working.
    let good = base.file("good.pdf", &fixture_gen::text_pdf(2));
    assert_eq!(app.import(&good).expect("import").pages.len(), 2);
    let (out, _) = app.export(base.path(), "ok.pdf").expect("export");
    assert!(out.is_file());
}

#[test]
fn a_stale_session_is_swept_at_startup() {
    let base = Scratch::new("e2e-sweep");
    let sessions = base.path().join("sessions");
    let stale = sessions.join("session-1-crashed");
    std::fs::create_dir_all(stale.join("cache")).expect("mkdir");
    std::fs::write(stale.join(".lock"), b"").expect("lock");
    std::fs::write(stale.join("cache/page.png"), b"left behind by a crash").expect("leftover");
    let app = app(&base);
    assert!(
        !stale.exists(),
        "leftovers from a crashed run are removed at startup"
    );
    assert!(app.session_dir().exists());
}
