use crate::analyze::{estimate_skew, gray_of, otsu_threshold};
use image::{imageops, Rgba, RgbaImage};
use sheaf_core::{ColorMode, Deskew, Filter, NormRect, Rot, Threshold};

const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

/// Rotate by clockwise quarter turns. Lossless.
pub fn rotate_quarter(img: RgbaImage, rot: Rot) -> RgbaImage {
    match rot {
        Rot::R0 => img,
        Rot::R90 => imageops::rotate90(&img),
        Rot::R180 => imageops::rotate180(&img),
        Rot::R270 => imageops::rotate270(&img),
    }
}

/// Keep only the pixels inside `rect`; the rest is gone, not hidden (TOOL-CROP).
pub fn crop_norm(img: &RgbaImage, rect: NormRect) -> RgbaImage {
    let (w, h) = img.dimensions();
    let px = |v: f32, side: u32| ((v.clamp(0.0, 1.0) * side as f32).round() as u32).min(side);
    let (x0, y0) = (px(rect.x0, w).min(w - 1), px(rect.y0, h).min(h - 1));
    let (x1, y1) = (px(rect.x1, w).max(x0 + 1), px(rect.y1, h).max(y0 + 1));
    imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// Rotate by a small angle about the centre, clockwise for positive
/// `degrees`, same canvas, bilinear, white where nothing maps.
pub fn rotate_small(img: &RgbaImage, degrees: f32) -> RgbaImage {
    if degrees == 0.0 {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let (cx, cy) = ((w as f32 - 1.0) / 2.0, (h as f32 - 1.0) / 2.0);
    let (s, c) = degrees.to_radians().sin_cos();
    let mut out = RgbaImage::from_pixel(w, h, WHITE);
    for (x, y, dst) in out.enumerate_pixels_mut() {
        // Inverse map: where in the source does this output pixel come from?
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let (sx, sy) = (cx + dx * c + dy * s, cy - dx * s + dy * c);
        if sx < 0.0 || sy < 0.0 || sx > (w - 1) as f32 || sy > (h - 1) as f32 {
            continue;
        }
        let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
        let (p00, p10, p01, p11) = (
            img.get_pixel(x0, y0),
            img.get_pixel(x1, y0),
            img.get_pixel(x0, y1),
            img.get_pixel(x1, y1),
        );
        for ch in 0..4 {
            let top = f32::from(p00[ch]) * (1.0 - fx) + f32::from(p10[ch]) * fx;
            let bottom = f32::from(p01[ch]) * (1.0 - fx) + f32::from(p11[ch]) * fx;
            dst[ch] = (top * (1.0 - fy) + bottom * fy).round() as u8;
        }
    }
    out
}

/// 3×3 median per channel: removes salt-and-pepper specks, keeps strokes.
pub fn despeckle(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return img.clone();
    }
    let mut out = img.clone();
    let mut window = [0u8; 9];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            for ch in 0..3 {
                let mut i = 0;
                for dy in 0..3 {
                    for dx in 0..3 {
                        window[i] = img.get_pixel(x + dx - 1, y + dy - 1)[ch];
                        i += 1;
                    }
                }
                window.sort_unstable();
                out.get_pixel_mut(x, y)[ch] = window[4];
            }
        }
    }
    out
}

/// Scale so the image fits inside `max_w` × `max_h`. Never enlarges.
pub fn fit_within(img: RgbaImage, max_w: u32, max_h: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max_w && h <= max_h {
        return img;
    }
    let scale = (f64::from(max_w) / f64::from(w)).min(f64::from(max_h) / f64::from(h));
    let (nw, nh) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    imageops::resize(&img, nw, nh, imageops::FilterType::Triangle)
}

/// Resample from `src_dpi` to `target_dpi`. Never upscales.
pub fn downsample(img: RgbaImage, target_dpi: u32, src_dpi: u32) -> RgbaImage {
    if target_dpi == 0 || src_dpi == 0 || target_dpi >= src_dpi {
        return img;
    }
    let (t, d) = (u64::from(target_dpi), u64::from(src_dpi));
    let scale = |v: u32| ((u64::from(v) * t + d / 2) / d).max(1) as u32;
    let (w, h) = (scale(img.width()), scale(img.height()));
    fit_within(img, w, h)
}

/// Apply a DATA-FILTER: deskew, then color mode, then contrast, then despeckle.
pub fn apply_filter(img: RgbaImage, f: &Filter) -> RgbaImage {
    if f.is_identity() {
        return img;
    }
    let mut img = match f.deskew {
        Deskew::Off => img,
        Deskew::Auto => {
            let angle = estimate_skew(&gray_of(&img));
            rotate_small(&img, -angle)
        }
        Deskew::Angle(a) => rotate_small(&img, -a),
    };
    if f.contrast != 0 {
        // -128..127 maps to a slope of about 0.33..3 around mid-gray.
        let slope = 3f32.powf(f32::from(f.contrast) / 127.0);
        for p in img.pixels_mut() {
            for ch in 0..3 {
                p[ch] = ((f32::from(p[ch]) - 127.5) * slope + 127.5).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    match f.mode {
        ColorMode::Color => {}
        ColorMode::Gray | ColorMode::BW => {
            let gray = gray_of(&img);
            let cut = match (f.mode, f.threshold) {
                (ColorMode::BW, Threshold::Auto) => Some(otsu_threshold(&gray)),
                (ColorMode::BW, Threshold::Fixed(t)) => Some(t),
                _ => None,
            };
            for (dst, g) in img.pixels_mut().zip(gray.pixels()) {
                let v = match cut {
                    Some(t) => {
                        if g[0] <= t {
                            0
                        } else {
                            255
                        }
                    }
                    None => g[0],
                };
                *dst = Rgba([v, v, v, 255]);
            }
        }
    }
    if f.despeckle {
        img = despeckle(&img);
    }
    img
}

/// A white page of the given size.
pub fn white(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_pixel(width.max(1), height.max(1), WHITE)
}
