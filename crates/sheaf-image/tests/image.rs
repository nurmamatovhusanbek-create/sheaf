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
    assert_eq!(
        estimate_skew(&gray(&page)),
        0.0,
        "an upright page reports no skew"
    );
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
    assert_eq!(
        dominant(&turned, 4, 4),
        "blue",
        "bottom-left moves to top-left"
    );
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
        decode_bytes(b"not an image at all")
            .expect_err("garbage")
            .code,
        ErrorCode::Unsupported
    );
    let png = encode_png(&corners()).expect("encode");
    assert_eq!(
        decode_bytes(&png[..png.len() / 2])
            .expect_err("truncated")
            .code,
        ErrorCode::ParseError
    );
    assert_eq!(
        decode_bytes(b"GIF89a\x01\0\x01\0\0\0\0;")
            .expect_err("gif")
            .code,
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
    assert_eq!(
        check_dimensions(0, 10).expect_err("empty").code,
        ErrorCode::ParseError
    );
    check_dimensions(9933, 14043).expect("A3 at 600 dpi is allowed");
}

#[test]
fn png_round_trip_is_lossless() {
    let img = corners();
    assert_eq!(
        decode_bytes(&encode_png(&img).expect("encode")).expect("decode"),
        img
    );
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
    assert_eq!(
        dominant(&turned, 28, 4),
        "red",
        "clockwise: top-left goes to top-right"
    );
    assert_eq!(
        rotate_quarter(rotate_quarter(img.clone(), Rot::R270), Rot::R90),
        img
    );
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
    assert_eq!(
        ink(&bw),
        ink(&page),
        "auto threshold keeps exactly the strokes"
    );

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
    for _ in 0..40 {
        dusty.put_pixel(1 + rng.next() % 398, 1 + rng.next() % 28, BLACK); // top margin, clear of text
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
        p[0] = if is_ink {
            (paper - 90).max(0) as u8
        } else {
            paper as u8
        };
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
    assert!(
        false_ink < hit / 10,
        "background stays clean: {false_ink} stray pixels"
    );
}
