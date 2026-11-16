use crate::analyze::{estimate_skew, gray_of, otsu_threshold};
use image::{imageops, Rgba, RgbaImage};
use sheaf_core::proto::FilterNote;
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

/// Paint out specks of dust: small, isolated dark blobs on a lighter page.
///
/// A median filter only removes dust one pixel across and eats the corners of the text it is meant to
/// keep; real scanner dust is a few pixels. This finds the page's dark pixels (Otsu), groups them into
/// connected blobs, and paints over every blob that is both small (about half a millimetre across,
/// scaled to the scan's resolution, which is taken from the page width) and alone (no other ink close
/// by) with the paper colour around it. Punctuation is never alone: a full stop, a comma or the dot of
/// an "i" sits next to the letters, and a hyphen is longer than a speck is wide; so they stay, as does
/// everything on a page that is mostly dark. Returns the picture and how many specks were painted out.
pub fn despeckle_report(img: &RgbaImage) -> (RgbaImage, u32) {
    let (w, h) = img.dimensions();
    if w < 16 || h < 16 {
        return (img.clone(), 0);
    }
    let gray = gray_of(img);
    let t = otsu_threshold(&gray);
    let ink = |i: usize| gray.as_raw()[i] <= t;
    let (lo, hi) = gray.pixels().fold((255u8, 0u8), |(l, h), p| (l.min(p[0]), h.max(p[0])));
    let dark = gray.pixels().filter(|p| p[0] <= t).count();
    // No contrast, or a page that is mostly dark (a negative): there is no "paper" to clean.
    if hi.saturating_sub(lo) < 40 || dark * 2 > (w as usize) * (h as usize) {
        return (img.clone(), 0);
    }
    // A page is about 8.3 inches on its short side (A4 and Letter, either way up). At 300 dpi a speck is up to about 30 pixels (5 or 6 across) and
    // "alone" means no other ink within 8 pixels (about 0.7 mm); both scale with the resolution.
    let dpi = (w.min(h) as f32 / 8.3).max(30.0);
    let scale = (dpi / 300.0).powi(2);
    let max_area = ((30.0 * scale).round() as usize).max(2);
    let max_side = ((7.0 * scale.sqrt()).round() as u32).max(2);
    let room = ((8.0 * scale.sqrt()).round() as u32).max(2);

    let (wu, hu) = (w as usize, h as usize);
    let mut seen = vec![false; wu * hu];
    let mut out = img.clone();
    let mut removed = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    let mut blob: Vec<usize> = Vec::new();
    for start in 0..wu * hu {
        if seen[start] || !ink(start) {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        blob.clear();
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        let mut size = 0usize;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i % wu) as u32, (i / wu) as u32);
            if size <= max_area {
                blob.push(i);
            }
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * wu + nx as usize;
                    if !seen[j] && ink(j) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if size > max_area || x1 - x0 + 1 > max_side || y1 - y0 + 1 > max_side {
            continue;
        }
        // Alone? Any other ink within `room` pixels means it belongs to something (a letter, a stop).
        let (ax0, ay0) = (x0.saturating_sub(room), y0.saturating_sub(room));
        let (ax1, ay1) = ((x1 + room).min(w - 1), (y1 + room).min(h - 1));
        let mut crowded = false;
        'look: for y in ay0..=ay1 {
            for x in ax0..=ax1 {
                let inside = x >= x0 && x <= x1 && y >= y0 && y <= y1;
                if !inside && ink(y as usize * wu + x as usize) {
                    crowded = true;
                    break 'look;
                }
            }
        }
        if crowded {
            continue;
        }
        // The paper around it: the average of the light pixels in a ring two pixels outside the blob.
        let (rx0, ry0) = (x0.saturating_sub(2), y0.saturating_sub(2));
        let (rx1, ry1) = ((x1 + 2).min(w - 1), (y1 + 2).min(h - 1));
        let (mut sum, mut n) = ([0u32; 3], 0u32);
        for y in ry0..=ry1 {
            for x in rx0..=rx1 {
                let i = y as usize * wu + x as usize;
                if !ink(i) {
                    let p = img.get_pixel(x, y);
                    for c in 0..3 {
                        sum[c] += u32::from(p[c]);
                    }
                    n += 1;
                }
            }
        }
        if n == 0 {
            continue;
        }
        let paper = Rgba([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]);
        // The blob and one pixel around it, which is where its soft edge is.
        for &i in &blob {
            let (bx, by) = ((i % wu) as i32, (i / wu) as i32);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (bx + dx, by + dy);
                    if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        out.put_pixel(nx as u32, ny as u32, paper);
                    }
                }
            }
        }
        removed += 1;
    }
    (out, removed)
}

/// [`despeckle_report`] without the count.
pub fn despeckle(img: &RgbaImage) -> RgbaImage {
    despeckle_report(img).0
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
    apply_filter_report(img, f).0
}

/// [`apply_filter`], and what the filters did: the angle the page was levelled by and the specks painted out.
pub fn apply_filter_report(img: RgbaImage, f: &Filter) -> (RgbaImage, FilterNote) {
    let mut note = FilterNote::default();
    if f.is_identity() {
        return (img, note);
    }
    let mut img = match f.deskew {
        Deskew::Off => img,
        Deskew::Auto => {
            let angle = estimate_skew(&gray_of(&img));
            note.skew_deg = Some(angle);
            rotate_small(&img, -angle)
        }
        Deskew::Angle(a) => {
            note.skew_deg = Some(a);
            rotate_small(&img, -a)
        }
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
        let (cleaned, specks) = despeckle_report(&img);
        img = cleaned;
        note.specks = Some(specks);
    }
    (img, note)
}

/// A white page of the given size.
pub fn white(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_pixel(width.max(1), height.max(1), WHITE)
}
