//! TEST-IMAGE: deskew within tolerance on rotated fixtures, blank detection
//! precision/recall, EXIF orientation. Fixtures are synthetic (TEST-FIXTURES).

use sheaf_core::{ColorMode, Deskew, ErrorCode, Filter, NormRect, Rot, Threshold};
use sheaf_image::*;

const WHITE: image::Rgba<u8> = image::Rgba([255, 255, 255, 255]);
const BLACK: image::Rgba<u8> = image::Rgba([20, 20, 20, 255]);

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

/// A page of "text": rows of word-like bars of varying length.
fn text_page(w: u32, h: u32, lines: u32, seed: u64) -> RgbaImage {
    let mut rng = Rng(seed | 1);
    let mut img = RgbaImage::from_pixel(w, h, WHITE);
    let (margin, pitch) = (w / 10, (h * 8 / 10) / lines.max(1));
    let bar = (pitch / 3).max(2);
    for line in 0..lines {
        let y0 = h / 10 + line * pitch;
        let mut x = margin;
        while x < w - margin {
            let word = 20 + rng.next() % 60;
            for yy in y0..(y0 + bar).min(h) {
                for xx in x..(x + word).min(w - margin) {
                    img.put_pixel(xx, yy, BLACK);
                }
            }
            x += word + 12 + rng.next() % 10;
        }
    }
    img
}

fn gray(img: &RgbaImage) -> GrayImage {
    image::DynamicImage::ImageRgba8(img.clone()).into_luma8()
}

#[test]
fn skew_is_estimated_within_tolerance() {
    let page = text_page(1000, 1400, 30, 7);
    assert_eq!(estimate_skew(&gray(&page)), 0.0, "an upright page reports no skew");
    for angle in [-7.0f32, -3.5, -1.0, -0.4, 0.5, 1.5, 2.75, 6.0] {
        let skewed = rotate_small(&page, angle);
        let got = estimate_skew(&gray(&skewed));
        assert!((got - angle).abs() <= 0.15, "skew {angle}: estimated {got}");
    }
}

#[test]
fn auto_deskew_straightens_the_page() {
    let page = text_page(1000, 1400, 30, 11);
    let skewed = rotate_small(&page, 3.2);
    let fixed = apply_filter(
        skewed,
        &Filter {
            deskew: Deskew::Auto,
            ..Default::default()
        },
    );
    let residual = estimate_skew(&gray(&fixed));
    assert!(residual.abs() <= 0.15, "residual skew {residual}");
}

#[test]
fn skew_of_an_empty_page_is_zero() {
    let blank = RgbaImage::from_pixel(600, 800, WHITE);
    assert_eq!(estimate_skew(&gray(&blank)), 0.0);
}

/// Paper with scanner noise: uneven tone, dust specks, a dark edge shadow.
fn noisy_blank(seed: u64, tone: u8) -> RgbaImage {
    let mut rng = Rng(seed | 1);
    let (w, h) = (1240, 1754);
    let mut img = RgbaImage::new(w, h);
    for p in img.pixels_mut() {
        let v = tone.saturating_sub((rng.next() % 12) as u8);
        *p = image::Rgba([v, v, v, 255]);
    }
    for _ in 0..150 {
        img.put_pixel(rng.next() % w, rng.next() % h, BLACK);
    }
    for y in 0..h {
        for x in 0..12 {
            img.put_pixel(x, y, image::Rgba([60, 60, 60, 255]));
        }
    }
    img
}

#[test]
fn blank_detection_precision_and_recall() {
    // Recall: every blank sheet is found, whatever the paper tone.
    for (seed, tone) in [(1, 255), (2, 240), (3, 225), (4, 205), (5, 250), (6, 215)] {
        let page = noisy_blank(seed, tone);
        assert!(
            is_blank(&page, BLANK_SENSITIVITY),
            "blank seed {seed} tone {tone}: {}",
            ink_coverage(&page)
        );
    }
    // Precision: pages with any real content are kept, down to a single line.
    for (lines, seed) in [(1, 1), (2, 2), (5, 3), (30, 4), (60, 5)] {
        let page = text_page(1240, 1754, lines, seed);
        assert!(
            !is_blank(&page, BLANK_SENSITIVITY),
            "{lines} lines: {}",
            ink_coverage(&page)
        );
    }
    // A one-line page on noisy gray paper is still content.
    let mut mixed = noisy_blank(9, 230);
    for y in 300..312 {
        for x in 200..1000 {
            mixed.put_pixel(x, y, BLACK);
        }
    }
    assert!(!is_blank(&mixed, BLANK_SENSITIVITY));
}

/// A 3×2 image with a distinct colour in each corner.
fn corners() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(48, 32, image::Rgba([128, 128, 128, 255]));
    let mut paint = |x0: u32, y0: u32, c: [u8; 3]| {
        for y in y0..y0 + 16 {
            for x in x0..x0 + 16 {
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    };
    paint(0, 0, [255, 0, 0]); // top-left red
    paint(32, 0, [0, 255, 0]); // top-right green
    paint(0, 16, [0, 0, 255]); // bottom-left blue
    paint(32, 16, [255, 255, 0]); // bottom-right yellow
    img
}

/// Insert an EXIF APP1 segment carrying only an orientation tag after SOI.
fn with_exif_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let tiff: [u8; 26] = [
        b'M',
        b'M',
        0,
        42,
        0,
        0,
        0,
        8, // big-endian TIFF header, IFD at 8
        0,
        1, // one entry
        0x01,
        0x12,
        0,
        3,
        0,
        0,
        0,
        1,
        0,
        orientation,
        0,
        0, // Orientation, SHORT, count 1
        0,
        0,
        0,
        0, // no next IFD
    ];
    let mut out = jpeg[..2].to_vec();
    let len = (2 + 6 + tiff.len()) as u16;
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn dominant(img: &RgbaImage, x: u32, y: u32) -> &'static str {
    let p = img.get_pixel(x, y);
    match (p[0] > 128, p[1] > 128, p[2] > 128) {
        (true, false, false) => "red",
        (false, true, false) => "green",
        (false, false, true) => "blue",
        (true, true, false) => "yellow",
        _ => "other",
    }
}

#[test]
fn exif_orientation_is_applied() {
    let jpeg = encode_jpeg(&corners(), 95).expect("encode");
    let plain = decode_bytes(&jpeg).expect("decode");
    assert_eq!(plain.dimensions(), (48, 32));
    assert_eq!(dominant(&plain, 4, 4), "red");

    // Orientation 6: the stored image must be turned 90° clockwise to display.
    let turned = decode_bytes(&with_exif_orientation(&jpeg, 6)).expect("decode");
    assert_eq!(turned.dimensions(), (32, 48));
    assert_eq!(dominant(&turned, 4, 4), "blue", "bottom-left moves to top-left");
    assert_eq!(dominant(&turned, 27, 4), "red");
    assert_eq!(dominant(&turned, 27, 43), "green");

    // Orientation 3: upside down.
    let flipped = decode_bytes(&with_exif_orientation(&jpeg, 3)).expect("decode");
    assert_eq!(flipped.dimensions(), (48, 32));
    assert_eq!(dominant(&flipped, 4, 4), "yellow");

    // The header-only size agrees with the decoded one.
    let dir = std::env::temp_dir().join(format!("sheaf-image-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("o6.jpg");
    std::fs::write(&path, with_exif_orientation(&jpeg, 6)).expect("write");
    assert_eq!(dimensions(&path).expect("dimensions"), (32, 48));
    assert_eq!(decode_file(&path).expect("decode").dimensions(), (32, 48));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_and_unsupported_inputs_fail_cleanly() {
    assert_eq!(
        decode_bytes(b"not an image at all").expect_err("garbage").code,
        ErrorCode::Unsupported
    );
    let png = encode_png(&corners()).expect("encode");
    assert_eq!(
        decode_bytes(&png[..png.len() / 2]).expect_err("truncated").code,
        ErrorCode::ParseError
    );
    assert_eq!(
        decode_bytes(b"GIF89a\x01\0\x01\0\0\0\0;").expect_err("gif").code,
        ErrorCode::Unsupported
    );

    // A PNG header claiming 60000×60000 pixels is refused before decoding.
    let mut bomb = png.clone();
    bomb[16..20].copy_from_slice(&60_000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&60_000u32.to_be_bytes());
    let code = decode_bytes(&bomb).expect_err("bomb").code;
    assert!(
        matches!(code, ErrorCode::LimitExceeded | ErrorCode::ParseError),
        "{code:?}"
    );
    assert_eq!(
        check_dimensions(60_000, 10).expect_err("side").code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        check_dimensions(20_000, 20_000).expect_err("area").code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(check_dimensions(0, 10).expect_err("empty").code, ErrorCode::ParseError);
    check_dimensions(9933, 14043).expect("A3 at 600 dpi is allowed");
}

#[test]
fn png_round_trip_is_lossless() {
    let img = corners();
    assert_eq!(decode_bytes(&encode_png(&img).expect("encode")).expect("decode"), img);
}

#[test]
fn crop_and_rotate_move_the_right_pixels() {
    let img = corners();
    let right_half = crop_norm(
        &img,
        NormRect {
            x0: 0.5,
            y0: 0.0,
            x1: 1.0,
            y1: 1.0,
        },
    );
    assert_eq!(right_half.dimensions(), (24, 32));
    assert_eq!(dominant(&right_half, 20, 4), "green");
    assert_eq!(dominant(&right_half, 20, 28), "yellow");

    let turned = rotate_quarter(img.clone(), Rot::R90);
    assert_eq!(turned.dimensions(), (32, 48));
    assert_eq!(dominant(&turned, 28, 4), "red", "clockwise: top-left goes to top-right");
    assert_eq!(rotate_quarter(rotate_quarter(img.clone(), Rot::R270), Rot::R90), img);
    assert_eq!(rotate_quarter(img.clone(), Rot::R0), img);
}

#[test]
fn filters_do_what_they_say() {
    let img = corners();
    assert_eq!(
        apply_filter(img.clone(), &Filter::default()),
        img,
        "identity filter is a no-op"
    );

    let gray = apply_filter(
        img.clone(),
        &Filter {
            mode: ColorMode::Gray,
            ..Default::default()
        },
    );
    assert!(gray.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));

    let page = text_page(400, 300, 6, 3);
    let bw = apply_filter(
        page.clone(),
        &Filter {
            mode: ColorMode::BW,
            ..Default::default()
        },
    );
    assert!(bw.pixels().all(|p| p[0] == 0 || p[0] == 255));
    let ink = |i: &RgbaImage| i.pixels().filter(|p| p[0] < 128).count();
    assert_eq!(ink(&bw), ink(&page), "auto threshold keeps exactly the strokes");

    let all_white = apply_filter(
        page.clone(),
        &Filter {
            mode: ColorMode::BW,
            threshold: Threshold::Fixed(5),
            ..Default::default()
        },
    );
    assert_eq!(ink(&all_white), 0);

    // Despeckle removes dust but keeps the text.
    let mut dusty = page.clone();
    let mut rng = Rng(5);
    let _ = &mut rng;
    // Dust in the top margin, clear of the text and of each other (dust that touches the next speck is
    // left alone, as the two dots of a colon must be).
    for i in 0..20u32 {
        dusty.put_pixel(10 + i * 19, 3 + (i * 7) % 20, BLACK);
    }
    let clean = apply_filter(
        dusty,
        &Filter {
            despeckle: true,
            ..Default::default()
        },
    );
    let top_ink = (0..400)
        .flat_map(|x| (0..29).map(move |y| (x, y)))
        .filter(|&(x, y)| clean.get_pixel(x, y)[0] < 128)
        .count();
    assert_eq!(top_ink, 0);
    assert!(ink(&clean) * 10 >= ink(&page) * 9);
}

#[test]
fn resizing_never_enlarges() {
    let img = text_page(600, 800, 10, 1);
    assert_eq!(downsample(img.clone(), 150, 300).dimensions(), (300, 400));
    assert_eq!(downsample(img.clone(), 600, 300).dimensions(), (600, 800));
    assert_eq!(fit_within(img.clone(), 256, 256).dimensions(), (192, 256));
    assert_eq!(fit_within(img.clone(), 4000, 4000).dimensions(), (600, 800));
}

#[test]
fn sauvola_handles_uneven_lighting() {
    // Dark text on a background that fades from white to mid-gray: a global
    // threshold loses one side, a local one keeps the strokes everywhere.
    let page = text_page(600, 400, 8, 2);
    let mut shaded = GrayImage::new(600, 400);
    for (x, y, p) in shaded.enumerate_pixels_mut() {
        let paper = 250 - (x * 150 / 600) as i32;
        let is_ink = page.get_pixel(x, y)[0] < 128;
        p[0] = if is_ink { (paper - 90).max(0) as u8 } else { paper as u8 };
    }
    let local = binarize_sauvola(&shaded, 41, 0.3);
    let (mut hit, mut miss, mut false_ink) = (0u32, 0u32, 0u32);
    for (x, y, p) in local.enumerate_pixels() {
        match (page.get_pixel(x, y)[0] < 128, p[0] == 0) {
            (true, true) => hit += 1,
            (true, false) => miss += 1,
            (false, true) => false_ink += 1,
            _ => {}
        }
    }
    assert!(hit > miss * 4, "strokes kept: {hit} hit, {miss} missed");
    assert!(false_ink < hit / 10, "background stays clean: {false_ink} stray pixels");
}

#[test]
fn multipage_tiff_pages_decode_in_order() {
    // TEST-IMG2PDF: TIFF pages.
    use tiff::encoder::{colortype, TiffEncoder};
    let dir = std::env::temp_dir().join(format!("sheaf-image-tiff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("pages.tif");
    {
        let file = std::fs::File::create(&path).expect("create");
        let mut encoder = TiffEncoder::new(std::io::BufWriter::new(file)).expect("encoder");
        encoder
            .write_image::<colortype::RGB8>(4, 2, &[200u8; 24])
            .expect("page 1");
        encoder
            .write_image::<colortype::Gray8>(3, 5, &[90u8; 15])
            .expect("page 2");
        encoder
            .write_image::<colortype::RGB8>(2, 2, &[10u8, 20, 30].repeat(4))
            .expect("page 3");
    }
    assert_eq!(page_count(&path).expect("count"), 3);
    assert_eq!(decode_page(&path, 0).expect("page 1").dimensions(), (4, 2));
    let second = decode_page(&path, 1).expect("page 2");
    assert_eq!(
        (second.dimensions(), second.get_pixel(1, 1).0),
        ((3, 5), [90, 90, 90, 255])
    );
    assert_eq!(
        decode_page(&path, 2).expect("page 3").get_pixel(0, 0).0,
        [10, 20, 30, 255]
    );
    assert_eq!(
        decode_page(&path, 3).expect_err("past the end").code,
        ErrorCode::InvalidArg
    );

    // Other formats have exactly one page.
    let png = dir.join("one.png");
    std::fs::write(&png, encode_png(&corners()).expect("png")).expect("write");
    assert_eq!(page_count(&png).expect("count"), 1);
    assert_eq!(
        decode_page(&png, 1).expect_err("no second page").code,
        ErrorCode::InvalidArg
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn jpeg_passthrough_reports_stored_size_and_turn() {
    let dir = std::env::temp_dir().join(format!("sheaf-image-jpt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let jpeg = encode_jpeg(&corners(), 90).expect("jpeg");
    let write = |name: &str, bytes: &[u8]| {
        let p = dir.join(name);
        std::fs::write(&p, bytes).expect("write");
        p
    };
    assert_eq!(jpeg_passthrough(&write("plain.jpg", &jpeg)), Some(((48, 32), Rot::R0)));
    // Orientation 6 asks for a quarter turn clockwise; the stored size is unchanged.
    assert_eq!(
        jpeg_passthrough(&write("o6.jpg", &with_exif_orientation(&jpeg, 6))),
        Some(((48, 32), Rot::R90))
    );
    assert_eq!(
        jpeg_passthrough(&write("o8.jpg", &with_exif_orientation(&jpeg, 8))),
        Some(((48, 32), Rot::R270))
    );
    // Mirrored: a page rotation cannot express it, so it has to be re-encoded.
    assert_eq!(
        jpeg_passthrough(&write("o2.jpg", &with_exif_orientation(&jpeg, 2))),
        None
    );
    assert_eq!(
        jpeg_passthrough(&write("not.jpg", &encode_png(&corners()).expect("png"))),
        None
    );
    assert_eq!(jpeg_passthrough(&dir.join("missing.jpg")), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 300 dpi-sized page with dust a few pixels across on the paper (never on the text), and how many specks.
fn dusty_page(seed: u64) -> (RgbaImage, RgbaImage, u32) {
    let page = text_page(2480, 3508, 30, seed);
    let mut dusty = page.clone();
    let mut rng = Rng(seed * 31 + 7);
    let mut specks = 0;
    let mut taken: Vec<(u32, u32)> = Vec::new();
    while specks < 200 {
        let (x, y) = (rng.next() % 2400 + 20, rng.next() % 3400 + 20);
        let size = 2 + rng.next() % 3; // 2 to 4 pixels across
                                       // On clear paper (10 pixels all round: nearer than that, a blob belongs to the text beside it),
                                       // and away from the other specks so each stays one speck.
        let room = size + 20;
        let clear = (0..room).all(|d| (0..room).all(|e| page.get_pixel(x + d - 10, y + e - 10)[0] > 200))
            && taken.iter().all(|&(tx, ty)| tx.abs_diff(x) > 16 || ty.abs_diff(y) > 16);
        if !clear {
            continue;
        }
        for dy in 0..size {
            for dx in 0..size {
                dusty.put_pixel(x + dx, y + dy, BLACK);
            }
        }
        taken.push((x, y));
        specks += 1;
    }
    (page, dusty, specks)
}

fn ink_count(i: &RgbaImage) -> i64 {
    i.pixels().filter(|p| p[0] < 128).count() as i64
}

#[test]
fn clean_removes_dust_of_real_size_and_leaves_the_text_alone() {
    let (page, dusty, specks) = dusty_page(9);
    let (clean, note) = apply_filter_report(
        dusty.clone(),
        &Filter {
            despeckle: true,
            ..Default::default()
        },
    );
    // Every speck is gone: the ink is what the clean page had, to the pixel.
    assert_eq!(ink_count(&clean), ink_count(&page), "all dust removed, no text lost");
    assert_eq!(clean, page, "the page is the same as before the dust, pixel for pixel");
    // And it says how many, so a person can see that it did something.
    assert_eq!(note.specks, Some(specks), "reported {:?} for {specks}", note.specks);
    assert!(ink_count(&dusty) > ink_count(&page), "the test dust is really there");
}

#[test]
fn clean_keeps_full_stops_hyphens_and_the_dots_of_i() {
    // 300 dpi: a letter is 24 pixels tall; a full stop of 10-point type is about 5 across, set close to
    // its letter; the dot of an "i" floats a few pixels above its stem; a hyphen is about 9 by 2.
    let mut page = RgbaImage::from_pixel(2480, 3508, WHITE);
    let fill = |page: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32| {
        for dy in 0..h {
            for dx in 0..w {
                page.put_pixel(x + dx, y + dy, BLACK);
            }
        }
    };
    // "d." : a stem and its full stop 4 pixels to the right, on the baseline.
    fill(&mut page, 300, 300, 4, 24);
    fill(&mut page, 308, 319, 5, 5);
    // "i" : a stem and its dot 4 pixels above.
    fill(&mut page, 600, 310, 3, 14);
    fill(&mut page, 600, 302, 4, 4);
    // "a-b" : a hyphen between two stems, longer than a speck is wide.
    fill(&mut page, 900, 300, 4, 24);
    fill(&mut page, 906, 312, 9, 2);
    fill(&mut page, 918, 300, 4, 24);
    // Something big to be the page's text, so the page is not empty.
    fill(&mut page, 300, 1000, 1700, 40);
    let (clean, note) = apply_filter_report(
        page.clone(),
        &Filter {
            despeckle: true,
            ..Default::default()
        },
    );
    assert_eq!(clean, page, "nothing that is punctuation was removed");
    assert_eq!(note.specks, Some(0), "and it says it found no specks");
}

#[test]
fn clean_removes_a_blob_that_is_alone_and_keeps_one_of_the_same_size_beside_a_letter() {
    let mut page = RgbaImage::from_pixel(2480, 3508, WHITE);
    let fill = |page: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32| {
        for dy in 0..h {
            for dx in 0..w {
                page.put_pixel(x + dx, y + dy, BLACK);
            }
        }
    };
    fill(&mut page, 300, 1000, 1700, 40); // the text
    fill(&mut page, 1500, 2000, 5, 5); // alone in the paper: dust
    fill(&mut page, 300, 1045, 5, 5); // the same blob, 5 pixels under the text: it belongs to it, leave it
    let (clean, note) = apply_filter_report(
        page,
        &Filter {
            despeckle: true,
            ..Default::default()
        },
    );
    assert_eq!(note.specks, Some(1));
    assert_eq!(*clean.get_pixel(1502, 2002), WHITE, "the lone blob is gone");
    assert_eq!(*clean.get_pixel(302, 1047), BLACK, "the one near the text stays");
}

#[test]
fn clean_paints_with_the_colour_of_the_paper_not_white() {
    let cream = image::Rgba([240, 228, 196, 255]);
    let mut page = RgbaImage::from_pixel(1240, 1754, cream);
    for y in 500..530 {
        for x in 200..1000 {
            page.put_pixel(x, y, BLACK);
        }
    }
    for dy in 0..2 {
        for dx in 0..2 {
            page.put_pixel(600 + dx, 900 + dy, BLACK);
        }
    }
    let clean = despeckle(&page);
    assert_eq!(*clean.get_pixel(600, 900), cream, "the speck became paper");
    assert_eq!(*clean.get_pixel(601, 901), cream);
    assert_eq!(*clean.get_pixel(300, 510), BLACK, "the text stayed");
}

#[test]
fn clean_leaves_a_page_that_is_mostly_dark_alone() {
    // A negative (white text on black) has no "paper" to clean.
    let mut page = RgbaImage::from_pixel(1240, 1754, BLACK);
    for y in 500..530 {
        for x in 200..1000 {
            page.put_pixel(x, y, WHITE);
        }
    }
    let (clean, specks) = despeckle_report(&page);
    assert_eq!((clean == page, specks), (true, 0));
}

#[test]
fn the_filters_report_what_they_did() {
    let (_, dusty, _) = dusty_page(4);
    // Straight text and dust: levelled by (almost) nothing, specks removed.
    let (_, note) = apply_filter_report(
        dusty.clone(),
        &Filter {
            deskew: Deskew::Auto,
            despeckle: true,
            ..Default::default()
        },
    );
    assert!(note.skew_deg.is_some_and(|d| d.abs() < 0.2), "{note:?}");
    assert!(note.specks.is_some_and(|n| n > 100), "{note:?}");
    // Off means no claim either way.
    let (_, off) = apply_filter_report(
        dusty,
        &Filter {
            mode: ColorMode::Gray,
            ..Default::default()
        },
    );
    assert_eq!((off.skew_deg, off.specks), (None, None));
}

#[test]
fn a_tilted_page_is_levelled_and_the_report_gives_the_angle() {
    let straight = text_page(1240, 1754, 30, 3);
    let tilted = rotate_small(&straight, 2.0);
    let (level, note) = apply_filter_report(
        tilted,
        &Filter {
            deskew: Deskew::Auto,
            ..Default::default()
        },
    );
    let found = note.skew_deg.expect("a measurement");
    assert!((found.abs() - 2.0).abs() < 0.4, "measured {found}, expected about 2");
    assert!(estimate_skew(&gray(&level)).abs() < 0.4, "the result is level");
}

#[test]
fn a_straight_page_with_one_short_word_is_not_tilted() {
    // The shape of a page with "Page 1" at the top and nothing else: too little to measure a skew from.
    let mut page = RgbaImage::from_pixel(636, 900, WHITE);
    for (x0, y0, w, h) in [
        (40u32, 40u32, 8u32, 14u32),
        (52, 44, 7, 10),
        (62, 44, 7, 14),
        (73, 44, 7, 10),
        (84, 40, 2, 14),
    ] {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                page.put_pixel(x, y, BLACK);
            }
        }
    }
    assert_eq!(estimate_skew(&gray(&page)), 0.0);
    // Nor is a page whose only content is a short stack of lines in one corner.
    let mut corner = RgbaImage::from_pixel(1240, 1754, WHITE);
    for line in 0..6 {
        for y in 100 + line * 30..100 + line * 30 + 10 {
            for x in 100..300 {
                corner.put_pixel(x, y, BLACK);
            }
        }
    }
    assert_eq!(estimate_skew(&gray(&corner)), 0.0);
}

#[test]
#[ignore = "timing, run by hand in release"]
fn how_long_do_the_filters_take_on_a_full_a4_scan() {
    let (_, dusty, _) = dusty_page(3);
    let tilted = rotate_small(&dusty, 1.5);
    for (name, f) in [
        (
            "straighten",
            Filter {
                deskew: Deskew::Auto,
                ..Default::default()
            },
        ),
        (
            "clean",
            Filter {
                despeckle: true,
                ..Default::default()
            },
        ),
        (
            "gray",
            Filter {
                mode: ColorMode::Gray,
                ..Default::default()
            },
        ),
        (
            "b/w",
            Filter {
                mode: ColorMode::BW,
                ..Default::default()
            },
        ),
        (
            "all",
            Filter {
                deskew: Deskew::Auto,
                despeckle: true,
                mode: ColorMode::BW,
                ..Default::default()
            },
        ),
    ] {
        let began = std::time::Instant::now();
        let _ = apply_filter_report(tilted.clone(), &f);
        eprintln!("{name}: {} ms", began.elapsed().as_millis());
    }
}
