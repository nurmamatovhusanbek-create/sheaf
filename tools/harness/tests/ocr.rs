//! TEST-OCR: character error rate per language, skip logic, cache behaviour,
//! language chooser, orientation fix, and the searchable PDF verified by
//! text extraction, including where on the page the text sits.
//!
//! Fixtures are synthetic: text set in Noto Sans, rendered at 300 dpi.
//! Thresholds are provisional until measured on real scans (RISK-OCR-ACCURACY).
//! Needs `bash tools/fetch-vendor`, Tesseract, and the `sheaf-worker` binary.

use fixture_gen::{char_error_rate, Scratch};
use harness::{App, OcrOutcome};
use sheaf_core::{ErrorCode, NormRect, OcrState, Op, PageId, Rot};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ENG: &[&str] = &[
    "The supplier shall deliver the goods within thirty",
    "calendar days from the date of advance payment.",
    "Payment is made by bank transfer to the account",
    "stated in section nine of this agreement.",
    "Any dispute is settled by negotiation between",
    "the parties, and failing that by the court.",
];
const RUS: &[&str] = &[
    "Поставщик обязуется передать товар покупателю",
    "в течение тридцати календарных дней с момента",
    "получения предварительной оплаты по договору.",
    "Оплата производится банковским переводом на",
    "расчётный счёт, указанный в разделе девять.",
    "Споры решаются путём переговоров между сторонами.",
];
const UZB: &[&str] = &[
    "Yetkazib beruvchi tovarni oldindan toʻlov olingan",
    "kundan boshlab oʻttiz kalendar kuni ichida",
    "xaridorga topshirish majburiyatini oladi.",
    "Toʻlov shartnomaning toʻqqizinchi boʻlimida",
    "koʻrsatilgan hisob raqamiga bank orqali amalga",
    "oshiriladi. Nizolar muzokaralar yoʻli bilan hal qilinadi.",
];
const UZB_CYRL: &[&str] = &[
    "Етказиб берувчи товарни олдиндан тўлов олинган",
    "кундан бошлаб ўттиз календарь куни ичида",
    "харидорга топшириш мажбуриятини олади.",
    "Тўлов шартноманинг тўққизинчи бўлимида",
    "кўрсатилган ҳисоб рақамига банк орқали амалга",
    "оширилади. Низолар музокаралар йўли билан ҳал қилинади.",
];

/// Upright page image size: A4 at 300 dpi.
const PAGE_PX: (f32, f32) = (2479.0, 3508.0);

struct Fixtures {
    dir: Scratch,
}

impl Fixtures {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// Built once per test binary. PDFium lives in this process only while the
/// fixtures are made, on one thread (ADR-WORKER-PROCS).
fn fixtures() -> &'static Fixtures {
    static FIXTURES: OnceLock<Fixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let dir = Scratch::new("ocr-fixtures");
        let engine = sheaf_pdf::Engine::load(&fixture_gen::vendored_pdfium_dir())
            .expect("PDFium loads (run `bash tools/fetch-vendor` first)");
        let font = fixture_gen::noto_font();
        for (name, lines) in [("eng", ENG), ("rus", RUS), ("uzb", UZB), ("uzb_cyrl", UZB_CYRL)] {
            let pdf = dir.path().join(format!("{name}.pdf"));
            let mut builder = engine.builder().expect("builder");
            builder.text_page(lines, &font, 13.0).expect("text page");
            builder.save(&pdf).expect("save text pdf");
            let img = engine
                .render(sheaf_pdf::Src::plain(&pdf), 0, (10_000, 10_000))
                .expect("render at 300 dpi");
            dir.file(&format!("{name}.png"), &sheaf_image::encode_png(&img).expect("png"));
            if name == "eng" {
                // The same page scanned sideways: content turned 90° clockwise.
                let sideways = sheaf_image::rotate_quarter(img.clone(), Rot::R90);
                dir.file("eng-sideways.png", &sheaf_image::encode_png(&sideways).expect("png"));
            }
            if name == "rus" {
                // A scan inside a PDF whose page carries its own /Rotate, so it displays upside down.
                let mut builder = engine.builder().expect("builder");
                builder
                    .image_page(&img, 300, Rot::R180, None, Default::default())
                    .expect("image page");
                builder
                    .save(&dir.path().join("rus-upside-down.pdf"))
                    .expect("save scan pdf");
            }
        }
        Fixtures { dir }
    })
}

fn app(base: &Scratch) -> App {
    let engines = harness::vendored().expect("sheaf-worker is built (run `cargo build --workspace`)");
    App::start(&engines, &base.path().join("sessions"), 2).expect("app starts")
}

fn import_one(app: &mut App, path: &Path) -> PageId {
    let report = app.import(path).expect("import");
    assert_eq!(report.pages.len(), 1);
    report.pages[0]
}

fn ocr_one(app: &mut App, id: PageId, langs: Option<&[&str]>, force: bool) -> OcrOutcome {
    let langs = langs.map(|l| l.iter().map(|s| (*s).to_string()).collect());
    let mut outcomes = app.ocr(&[id], langs, force).expect("ocr call");
    assert_eq!(outcomes.len(), 1);
    outcomes.remove(0).1
}

fn done(outcome: OcrOutcome) -> sheaf_core::proto::OcrPageResult {
    match outcome {
        OcrOutcome::Done(r) => r,
        other => panic!("expected a recognized page, got {other:?}"),
    }
}

fn truth(lines: &[&str]) -> String {
    lines.join("\n")
}

/// The text layer sits where the words are: for a few recognized words, the
/// text extracted from that spot of the exported page is that word, and an
/// empty part of the page yields nothing. `upright_px` is the size of the
/// image the engine read, after any orientation fix.
fn assert_text_layer_aligned(app: &App, pdf: &Path, result: &sheaf_core::OcrResult, upright_px: (f32, f32)) {
    let mut checked = 0;
    for word in result
        .words
        .iter()
        .filter(|w| w.text.chars().count() >= 6 && w.conf > 80.0)
        .step_by(5)
    {
        let [left, top, width, height] = word.bbox.map(|v| v as f32);
        // Vertically the middle band of the word; horizontally a little slack.
        let rect = NormRect {
            x0: ((left - 12.0) / upright_px.0).max(0.0),
            y0: (top + height * 0.25) / upright_px.1,
            x1: ((left + width + 12.0) / upright_px.0).min(1.0),
            y1: (top + height * 0.75) / upright_px.1,
        };
        let found = app.file_text(pdf, 0, Some(rect)).expect("text in region");
        let clean = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>();
        assert_eq!(
            clean(found.trim()),
            clean(&word.text),
            "text at the position of {:?}",
            word.text
        );
        checked += 1;
    }
    assert!(checked >= 3, "enough words checked ({checked})");
    let empty = NormRect {
        x0: 0.1,
        y0: 0.8,
        x1: 0.9,
        y1: 0.95,
    };
    assert_eq!(
        app.file_text(pdf, 0, Some(empty)).expect("text in region").trim(),
        "",
        "nothing in the empty part"
    );
}

#[test]
fn character_error_rate_and_language_choice() {
    let fx = fixtures();
    let base = Scratch::new("ocr-cer");
    let mut app = app(&base);
    // (fixture, reference, expected languages, CER threshold)
    let cases: [(&str, &[&str], &[&str], f64); 4] = [
        ("eng", ENG, &["uzb", "eng"], 0.02),
        ("uzb", UZB, &["uzb", "eng"], 0.04),
        ("rus", RUS, &["uzb_cyrl", "rus"], 0.02),
        ("uzb_cyrl", UZB_CYRL, &["uzb_cyrl", "rus"], 0.04),
    ];
    for (name, lines, langs, max_cer) in cases {
        let id = import_one(&mut app, &fx.path(&format!("{name}.png")));
        assert_eq!(app.ws.find(id).and_then(|p| p.has_text), Some(false));
        let r = done(ocr_one(&mut app, id, None, false));
        assert!(
            !r.cached && r.rotate == 0 && r.engine.starts_with("tesseract"),
            "{name}: {r:?}"
        );
        assert_eq!(r.langs, langs, "{name}: script detection picks the language pair");
        let result = app.ocr_result(id).expect("result from cache");
        let cer = char_error_rate(&truth(lines), &result.text());
        eprintln!(
            "CER {name}: {:.2}% (mean confidence {:.0})",
            cer * 100.0,
            result.mean_conf
        );
        assert!(
            cer <= max_cer,
            "{name}: CER {:.2}% over {:.0}%\n{}",
            cer * 100.0,
            max_cer * 100.0,
            result.text()
        );
        assert!(result.mean_conf > 70.0, "{name}: mean confidence {}", result.mean_conf);
        assert!(matches!(app.ws.find(id).map(|p| &p.ocr), Some(OcrState::Done { langs: l, .. }) if l == langs));
    }

    // CMD-SEARCH over what was just recognized.
    let pages: Vec<PageId> = app.ws.all_pages().map(|p| p.id).collect();
    assert_eq!(
        app.search("банковским переводом")
            .iter()
            .map(|h| h.page)
            .collect::<Vec<_>>(),
        [pages[2]]
    );
    assert_eq!(
        app.search("to'lov").first().map(|h| h.page),
        Some(pages[1]),
        "apostrophe variants match"
    );
    assert_eq!(app.search("тўққизинчи").first().map(|h| h.page), Some(pages[3]));
    assert!(app.search("nonexistentword").is_empty());

    // Fixed languages skip detection and are used as given.
    let fixed = done(ocr_one(&mut app, pages[2], Some(&["rus"]), false));
    assert_eq!(fixed.langs, ["rus"]);
    assert!(!fixed.cached, "a different language setting is a different cache entry");

    // "Extract text only": one text file, pages separated by form feeds.
    let txt = app.export_text(base.path(), "all.ocr.txt").expect("text export");
    let text = std::fs::read_to_string(&txt).expect("read text export");
    assert_eq!(text.matches('\u{c}').count(), 4);
    assert!(text.contains("bank transfer") && text.contains("банк") && !text.contains("Toshkent"));
}

#[test]
fn pages_with_text_are_skipped_unless_redone() {
    let fx = fixtures();
    let base = Scratch::new("ocr-skip");
    let mut app = app(&base);
    let id = import_one(&mut app, &fx.path("eng.pdf"));
    assert_eq!(app.ws.find(id).and_then(|p| p.has_text), Some(true));

    // Add missing text (default): a born-digital page is left alone and stays lossless.
    assert_eq!(ocr_one(&mut app, id, None, false), OcrOutcome::SkippedHasText);
    assert_eq!(app.ws.find(id).map(|p| p.ocr.clone()), Some(OcrState::SkippedHasText));
    assert_eq!(
        app.ocr_result(id).expect_err("nothing recognized").code,
        ErrorCode::NotFound
    );
    let (kept, built) = app.export(base.path(), "kept.pdf").expect("export");
    assert_eq!((built.copied, built.rasterized, built.text_layers), (1, 0, 0));
    assert!(app.file_text(&kept, 0, None).expect("text").contains("bank transfer"));

    // Redo all: recognized anyway, and the old text does not survive under the new layer.
    let r = done(ocr_one(&mut app, id, None, true));
    assert!(r.words > 30);
    let (redone, built) = app.export(base.path(), "redone.pdf").expect("export");
    assert_eq!((built.copied, built.rasterized, built.text_layers), (0, 1, 1));
    let text = app.file_text(&redone, 0, None).expect("text");
    assert_eq!(
        text.matches("negotiation").count(),
        1,
        "exactly one text layer:\n{text}"
    );
    assert!(char_error_rate(&truth(ENG), &text) < 0.03);
}

#[test]
fn cache_follows_the_pixels() {
    let fx = fixtures();
    let base = Scratch::new("ocr-cache");
    let mut app = app(&base);
    let id = import_one(&mut app, &fx.path("eng.png"));

    let first = done(ocr_one(&mut app, id, None, false));
    assert!(!first.cached);
    let again = done(ocr_one(&mut app, id, None, false));
    assert!(again.cached, "the same pixels are not recognized twice");
    assert_eq!(again.key, first.key);

    // Edits that leave the pixels alone keep the result.
    app.ws
        .apply(Op::Rotate {
            ids: vec![id],
            delta: Rot::R180,
        })
        .expect("rotate");
    app.ws.apply(Op::Duplicate { ids: vec![id] }).expect("duplicate");
    let copy = app.ws.pages(1..2)[0].id;
    assert!(done(ocr_one(&mut app, id, None, false)).cached);
    assert_eq!(
        done(ocr_one(&mut app, copy, None, false)).key,
        first.key,
        "a duplicate shares the entry"
    );

    // Edits that change pixels do not.
    let top_half = NormRect {
        x0: 0.0,
        y0: 0.0,
        x1: 1.0,
        y1: 0.5,
    };
    app.ws
        .apply(Op::Crop {
            ids: vec![id],
            rect: Some(top_half),
        })
        .expect("crop");
    let cropped = done(ocr_one(&mut app, id, None, false));
    assert!(!cropped.cached);
    assert_ne!(cropped.key, first.key);
    // Undo brings the old pixels back, and with them the cached result.
    app.ws.undo().expect("undo crop");
    assert!(done(ocr_one(&mut app, id, None, false)).cached);
}

#[test]
fn searchable_pdf_has_the_text_where_the_words_are() {
    let fx = fixtures();
    let base = Scratch::new("ocr-layer");
    let mut app = app(&base);

    // An image scan: the page is rebuilt around the image, the text laid over it.
    let id = import_one(&mut app, &fx.path("rus.png"));
    done(ocr_one(&mut app, id, None, false));
    let result = app.ocr_result(id).expect("result");
    let (out, built) = app.export(base.path(), "rus.ocr.pdf").expect("export");
    assert_eq!((built.page_count, built.text_layers), (1, 1));
    let text = app.file_text(&out, 0, None).expect("text of the searchable PDF");
    let cer = char_error_rate(&truth(RUS), &text);
    assert!(
        cer < 0.03,
        "extracted text matches the page (CER {:.2}%):\n{text}",
        cer * 100.0
    );
    assert_text_layer_aligned(&app, &out, &result, PAGE_PX);

    // The searchable PDF is itself a document with text now: importing it skips OCR.
    let reimported = import_one(&mut app, &out);
    assert_eq!(app.ws.find(reimported).and_then(|p| p.has_text), Some(true));
    assert_eq!(ocr_one(&mut app, reimported, None, false), OcrOutcome::SkippedHasText);
}

#[test]
fn sideways_and_upside_down_pages_are_turned_and_stay_aligned() {
    let fx = fixtures();
    let base = Scratch::new("ocr-orient");
    let mut app = app(&base);

    // A sideways image scan.
    let id = import_one(&mut app, &fx.path("eng-sideways.png"));
    let r = done(ocr_one(&mut app, id, None, false));
    assert_eq!(
        r.rotate, 270,
        "content turned 90° clockwise needs 270° more to be upright"
    );
    assert_eq!(
        app.ws.find(id).map(|p| p.rot),
        Some(Rot::R270),
        "the fix is recorded on the page"
    );
    let result = app.ocr_result(id).expect("result");
    assert!(char_error_rate(&truth(ENG), &result.text()) < 0.03, "{}", result.text());
    let (out, _) = app.export(base.path(), "sideways.ocr.pdf").expect("export");
    assert!(char_error_rate(&truth(ENG), &app.file_text(&out, 0, None).expect("text")) < 0.03);
    assert_text_layer_aligned(&app, &out, &result, PAGE_PX);
    // The fix is an ordinary op: it can be undone.
    app.ws.undo().expect("undo the rotation");
    assert_eq!(app.ws.find(id).map(|p| p.rot), Some(Rot::R0));
    app.ws.apply(Op::Delete { ids: vec![id] }).expect("remove");

    // A scan inside a PDF whose page is stored with /Rotate and shows upside
    // down. The page is copied losslessly; the text layer has to go through
    // both the page's own rotation and the orientation fix.
    let id = import_one(&mut app, &fx.path("rus-upside-down.pdf"));
    let r = done(ocr_one(&mut app, id, None, false));
    assert_eq!(r.rotate, 180);
    let result = app.ocr_result(id).expect("result");
    let (out, built) = app.export(base.path(), "upside-down.ocr.pdf").expect("export");
    assert_eq!(
        (built.copied, built.rasterized, built.text_layers),
        (1, 0, 1),
        "the scan itself is not re-encoded"
    );
    assert!(char_error_rate(&truth(RUS), &app.file_text(&out, 0, None).expect("text")) < 0.03);
    assert_text_layer_aligned(&app, &out, &result, PAGE_PX);
}

#[test]
fn ocr_failures_stay_on_their_page() {
    let fx = fixtures();
    let base = Scratch::new("ocr-fail");
    let mut app = app(&base);
    let good = import_one(&mut app, &fx.path("eng.png"));
    let other = import_one(&mut app, &fx.path("rus.png"));

    // Language names are input like any other.
    for bad in [&["../../etc/passwd"][..], &["eng; rm -rf"], &[""], &["ENG"]] {
        match ocr_one(&mut app, good, Some(bad), false) {
            OcrOutcome::Failed(e) => assert_eq!(e.code, ErrorCode::InvalidArg, "{bad:?}"),
            o => panic!("{bad:?} should be rejected, got {o:?}"),
        }
    }
    // A well-formed name with no model behind it fails that page only.
    assert!(matches!(
        ocr_one(&mut app, good, Some(&["zzz"]), false),
        OcrOutcome::Failed(_)
    ));
    assert!(matches!(
        app.ws.find(good).map(|p| &p.ocr),
        Some(OcrState::Failed { .. })
    ));
    done(ocr_one(&mut app, other, None, false));
    done(ocr_one(&mut app, good, None, false));

    // A blank page has nothing to recognize.
    app.ws
        .apply(Op::InsertBlank {
            at: 0,
            size: (595.0, 842.0),
        })
        .expect("blank");
    let blank = app.ws.pages(0..1)[0].id;
    assert!(matches!(ocr_one(&mut app, blank, None, false), OcrOutcome::Failed(e) if e.code == ErrorCode::InvalidArg));
    // Export still works, with text layers on the recognized pages only.
    let (_, built) = app.export(base.path(), "mixed.pdf").expect("export");
    assert_eq!((built.page_count, built.text_layers), (3, 2));
}

#[test]
fn without_an_engine_ocr_fails_cleanly() {
    let fx = fixtures();
    let base = Scratch::new("ocr-noengine");
    let mut engines = harness::vendored().expect("engines");
    engines.tesseract = None;
    let mut app = App::start(&engines, &base.path().join("sessions"), 1).expect("app starts");
    let id = import_one(&mut app, &fx.path("eng.png"));
    assert!(matches!(ocr_one(&mut app, id, None, false), OcrOutcome::Failed(e) if e.code == ErrorCode::EngineMissing));

    engines.tesseract = Some(("/no/such/tesseract".into(), fixture_gen::tessdata_dir()));
    let mut app = App::start(&engines, &base.path().join("sessions2"), 1).expect("app starts");
    let id = import_one(&mut app, &fx.path("eng.png"));
    assert!(matches!(ocr_one(&mut app, id, None, false), OcrOutcome::Failed(e) if e.code == ErrorCode::EngineMissing));
    // Everything that does not need OCR still works.
    app.export(base.path(), "plain.pdf").expect("export without OCR");
}

#[test]
fn headless_ocr_command() {
    // FLOW-OCR-MANUAL step 5: `ocr file --headless` equivalent.
    let fx = fixtures();
    let out = Scratch::new("ocr-cli");
    let run = |args: &[&str], file: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_sheaf-harness"))
            .arg("ocr")
            .args(args)
            .arg("--out")
            .arg(out.path())
            .arg(fx.path(file))
            .output()
            .expect("harness runs")
    };
    let result = run(&[], "uzb.png");
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        out.path().join("uzb.ocr.pdf").to_string_lossy()
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("1 pages recognized, 0 already had text, 0 failed"));
    assert!(out.path().join("uzb.ocr.pdf").is_file());

    let result = run(&["--text-only", "--langs", "rus"], "rus.png");
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = std::fs::read_to_string(out.path().join("rus.ocr.txt")).expect("text file");
    assert!(
        char_error_rate(&truth(RUS), text.trim_end_matches(['\n', '\u{c}'])) < 0.03,
        "{text}"
    );

    // The inputs are never written to, and a second run does not overwrite the first output.
    let result = run(&[], "uzb.png");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        out.path().join("uzb.ocr-2.pdf").to_string_lossy()
    );
    // A page that fails makes the run "partial": exit code 2, output still written.
    let result = run(&["--langs", "zzz"], "eng.png");
    assert_eq!(result.status.code(), Some(2));
    assert!(out.path().join("eng.ocr.pdf").is_file());
    assert_eq!(run(&["--langs"], "eng.png").status.code(), Some(64), "usage error");
}
