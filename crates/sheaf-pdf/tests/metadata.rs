//! TEST-METADATA: strip removes the information fields and the XMP packet; set writes both;
//! text outside ASCII survives; nothing else about the file changes.

use fixture_gen::{locked_pdf, text_pdf, Scratch};
use sheaf_core::proto::{MetaEdit, MetaFields};
use sheaf_core::ErrorCode;
use sheaf_pdf::lo::{edit_metadata, read_metadata};

fn fields() -> MetaFields {
    MetaFields {
        title: "Shartnoma № 12".into(),
        author: "Алишер Навоий".into(),
        subject: "Ёзув & <hisob>".into(),
        keywords: "pdf, scan, o‘zbek".into(),
        creator: "Sheaf test".into(),
        producer: "Sheaf".into(),
    }
}

#[test]
fn a_plain_file_has_nothing_to_say_and_set_writes_both_places() {
    let dir = Scratch::new("meta-set");
    let src = dir.file("plain.pdf", &text_pdf(3));
    let before = read_metadata(&src).expect("read");
    assert_eq!(before.fields, MetaFields::default());
    assert!(!before.has_xmp);
    assert_eq!(before.pages, 3);

    let out = dir.path().join("set.pdf");
    let after = edit_metadata(&src, &out, &MetaEdit::Set(fields())).expect("set");
    assert_eq!(
        after.fields,
        fields(),
        "what is read back is what was written, text outside ASCII included"
    );
    assert!(after.has_xmp);
    assert_eq!(after.pages, 3, "the pages are untouched");

    // The XMP packet really holds the values (escaped), as readable text.
    let bytes = std::fs::read(&out).expect("read file");
    let text = String::from_utf8_lossy(&bytes);
    for needle in [
        "Shartnoma № 12",
        "Алишер Навоий",
        "Ёзув &amp; &lt;hisob&gt;",
        "x:xmpmeta",
    ] {
        assert!(text.contains(needle), "XMP lacks {needle}");
    }
}

#[test]
fn strip_removes_the_fields_and_the_packet_and_the_values_are_gone_from_the_file() {
    let dir = Scratch::new("meta-strip");
    let src = dir.file("plain.pdf", &text_pdf(2));
    let set = dir.path().join("set.pdf");
    edit_metadata(&src, &set, &MetaEdit::Set(fields())).expect("set");

    let stripped = dir.path().join("clean.pdf");
    let info = edit_metadata(&set, &stripped, &MetaEdit::Strip).expect("strip");
    assert_eq!(info.fields, MetaFields::default());
    assert!(!info.has_xmp);
    assert_eq!(info.pages, 2);
    let bytes = std::fs::read(&stripped).expect("read file");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("xmpmeta") && !text.contains("Shartnoma"),
        "no packet left in the file"
    );
    // The UTF-16 form of the author is not there either.
    let utf16: Vec<u8> = "Алишер".encode_utf16().flat_map(u16::to_be_bytes).collect();
    assert!(
        !bytes.windows(utf16.len()).any(|w| w == utf16),
        "the encoded author is gone"
    );
}

#[test]
fn changing_one_field_replaces_the_packet_so_no_old_value_survives_in_it() {
    let dir = Scratch::new("meta-replace");
    let src = dir.file("plain.pdf", &text_pdf(1));
    let first = dir.path().join("first.pdf");
    edit_metadata(&src, &first, &MetaEdit::Set(fields())).expect("first");
    let second = dir.path().join("second.pdf");
    let changed = MetaFields {
        title: "Новый заголовок".into(),
        author: String::new(),
        ..fields()
    };
    let info = edit_metadata(&first, &second, &MetaEdit::Set(changed.clone())).expect("second");
    assert_eq!(info.fields, changed);
    let text = String::from_utf8_lossy(&std::fs::read(&second).expect("read")).into_owned();
    assert!(text.contains("Новый заголовок"));
    assert!(!text.contains("Shartnoma"), "the old title is not left in the packet");
    assert!(
        !text.contains("Алишер Навоий"),
        "an emptied field is left out of the packet"
    );
}

#[test]
fn bad_values_a_locked_file_and_junk_are_refused() {
    let dir = Scratch::new("meta-bad");
    let src = dir.file("plain.pdf", &text_pdf(1));
    let out = dir.path().join("out.pdf");
    for bad in [
        MetaFields {
            title: "x".repeat(1_001),
            ..MetaFields::default()
        },
        MetaFields {
            author: "line\nbreak".into(),
            ..MetaFields::default()
        },
    ] {
        let err = edit_metadata(&src, &out, &MetaEdit::Set(bad)).expect_err("refused");
        assert_eq!(err.code, ErrorCode::InvalidArg);
        assert!(!out.exists(), "nothing is written for a refused edit");
    }
    let locked = dir.file("locked.pdf", &locked_pdf());
    assert_eq!(
        read_metadata(&locked).expect_err("locked").code,
        ErrorCode::NeedsPassword
    );
    let junk = dir.file("junk.pdf", b"not a pdf at all");
    assert!(read_metadata(&junk).is_err());
}
