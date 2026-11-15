//! TOOL-METADATA through the real worker: reading, changed copies next to the original,
//! never overwriting, refused input.

use fixture_gen::{locked_pdf, noto_font, tessdata_dir, tesseract_exe, text_pdf, vendored_pdfium_dir, Scratch};
use sheaf_app::metadata::{edit, read};
use sheaf_app::plan::NoCtl;
use sheaf_app::{sibling_worker, App, Engines};
use sheaf_core::proto::{MetaEdit, MetaFields};
use sheaf_core::ErrorCode;

fn app(base: &Scratch) -> App {
    let engines = Engines {
        worker: sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)"),
        pdfium_dir: vendored_pdfium_dir(),
        tesseract: Some((tesseract_exe(), tessdata_dir())),
        font: Some(noto_font()),
    };
    App::start(&engines, &base.path().join("sessions"), 2).expect("app starts")
}

fn fields() -> MetaFields {
    MetaFields {
        title: "Договор № 7".into(),
        author: "Sheaf".into(),
        subject: String::new(),
        keywords: "scan".into(),
        creator: String::new(),
        producer: String::new(),
    }
}

#[test]
fn a_changed_copy_goes_next_to_the_original_and_the_original_is_untouched() {
    let base = Scratch::new("meta-base");
    let work = Scratch::new("meta-work");
    let app = app(&base);
    let shared = app.shared();
    let doc = work.file("doc.pdf", &text_pdf(2));
    let before = std::fs::read(&doc).expect("read");
    assert_eq!(read(&shared, &doc, &NoCtl).expect("read").fields, MetaFields::default());

    let (edited, info) = edit(&shared, &doc, &MetaEdit::Set(fields()), None, &NoCtl).expect("set");
    assert_eq!(edited.file_name().expect("name"), "doc.edited.pdf");
    assert_eq!(info.fields, fields());
    assert!(info.has_xmp);
    assert_eq!(
        read(&shared, &edited, &NoCtl).expect("read back"),
        info,
        "the file says what the call said"
    );
    assert!(!edited.to_string_lossy().starts_with(r"\\?\"), "plain path");

    let (clean, stripped) = edit(&shared, &edited, &MetaEdit::Strip, None, &NoCtl).expect("strip");
    assert_eq!(clean.file_name().expect("name"), "doc.edited.clean.pdf");
    assert_eq!(stripped.fields, MetaFields::default());
    assert!(!stripped.has_xmp);

    // A second strip never replaces the first.
    let (again, _) = edit(&shared, &edited, &MetaEdit::Strip, None, &NoCtl).expect("again");
    assert_eq!(again.file_name().expect("name"), "doc.edited.clean-2.pdf");
    assert_eq!(
        std::fs::read(&doc).expect("read"),
        before,
        "the original is not touched"
    );
}

#[test]
fn the_folder_can_be_chosen_and_bad_input_is_refused() {
    let base = Scratch::new("meta-base2");
    let work = Scratch::new("meta-work2");
    let out = Scratch::new("meta-out2");
    let app = app(&base);
    let shared = app.shared();
    let doc = work.file("doc.pdf", &text_pdf(1));
    let (path, _) = edit(&shared, &doc, &MetaEdit::Strip, Some(out.path()), &NoCtl).expect("strip");
    assert_eq!(path.parent(), Some(out.path()));

    let too_long = MetaFields {
        title: "x".repeat(2_000),
        ..MetaFields::default()
    };
    assert_eq!(
        edit(&shared, &doc, &MetaEdit::Set(too_long), None, &NoCtl)
            .expect_err("refused")
            .code,
        ErrorCode::InvalidArg
    );
    let locked = work.file("locked.pdf", &locked_pdf());
    assert_eq!(
        read(&shared, &locked, &NoCtl).expect_err("locked").code,
        ErrorCode::NeedsPassword
    );
    let png = work.file("pic.png", &{
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("png");
        bytes
    });
    assert_eq!(
        read(&shared, &png, &NoCtl).expect_err("not a pdf").code,
        ErrorCode::Unsupported
    );
    assert!(read(&shared, &work.path().join("missing.pdf"), &NoCtl).is_err());
}
