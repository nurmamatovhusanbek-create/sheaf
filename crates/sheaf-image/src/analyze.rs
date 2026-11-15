use image::{GrayImage, Luma, RgbaImage};

/// Otsu's threshold: the gray level that best separates two classes.
/// Pixels `<=` the result are ink.
pub fn otsu_threshold(gray: &GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in gray.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total = gray.as_raw().len() as f64;
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum();
    let (mut w_bg, mut sum_bg, mut best, mut best_t) = (0f64, 0f64, -1f64, 127u8);
    for (t, &n) in hist.iter().enumerate() {
        w_bg += n as f64;
        if w_bg == 0.0 {
            continue;
        }
        let w_fg = total - w_bg;
        if w_fg == 0.0 {
            break;
        }
        sum_bg += t as f64 * n as f64;
        let diff = sum_bg / w_bg - (sum_all - sum_bg) / w_fg;
        let between = w_bg * w_fg * diff * diff;
        if between > best {
            best = between;
            best_t = t as u8;
        }
    }
    best_t
}

/// Sauvola local thresholding for unevenly lit pages. `window` is the side of
/// the neighbourhood in pixels, `k` the usual sensitivity (0.2 to 0.5).
pub fn binarize_sauvola(gray: &GrayImage, window: u32, k: f32) -> GrayImage {
    let (w, h) = gray.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    // Integral images of values and squared values, with a zero border.
    let stride = wu + 1;
    let mut sum = vec![0f64; stride * (hu + 1)];
    let mut sq = vec![0f64; stride * (hu + 1)];
    for y in 0..hu {
        let (mut row, mut row_sq) = (0f64, 0f64);
        for x in 0..wu {
            let v = f64::from(gray.as_raw()[y * wu + x]);
            row += v;
            row_sq += v * v;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
            sq[(y + 1) * stride + x + 1] = sq[y * stride + x + 1] + row_sq;
        }
    }
    let r = (window.max(3) / 2) as usize;
    let mut out = GrayImage::new(w, h);
    for y in 0..hu {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(hu));
        for x in 0..wu {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(wu));
            let area = ((y1 - y0) * (x1 - x0)) as f64;
            let rect =
                |t: &[f64]| t[y1 * stride + x1] - t[y0 * stride + x1] - t[y1 * stride + x0] + t[y0 * stride + x0];
            let mean = rect(&sum) / area;
            let var = (rect(&sq) / area - mean * mean).max(0.0);
            let threshold = mean * (1.0 + f64::from(k) * (var.sqrt() / 128.0 - 1.0));
            let v = f64::from(gray.as_raw()[y * wu + x]);
            out.put_pixel(x as u32, y as u32, Luma([if v <= threshold { 0 } else { 255 }]));
        }
    }
    out
}

/// Shrink to at most `max_side` on the long edge by box averaging.
fn shrink(gray: &GrayImage, max_side: u32) -> GrayImage {
    let (w, h) = gray.dimensions();
    let long = w.max(h);
    if long <= max_side {
        return gray.clone();
    }
    let f = long.div_ceil(max_side);
    let (nw, nh) = ((w / f).max(1), (h / f).max(1));
    let mut out = GrayImage::new(nw, nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = 0u32;
            let mut n = 0u32;
            for dy in 0..f {
                for dx in 0..f {
                    let (sx, sy) = (x * f + dx, y * f + dy);
                    if sx < w && sy < h {
                        acc += u32::from(gray.get_pixel(sx, sy)[0]);
                        n += 1;
                    }
                }
            }
            out.put_pixel(x, y, Luma([(acc / n.max(1)) as u8]));
        }
    }
    out
}

fn to_gray(img: &RgbaImage) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for (dst, src) in out.pixels_mut().zip(img.pixels()) {
        // Transparent counts as paper.
        let a = u32::from(src[3]);
        let luma = (u32::from(src[0]) * 299 + u32::from(src[1]) * 587 + u32::from(src[2]) * 114) / 1000;
        dst[0] = ((luma * a + 255 * (255 - a)) / 255) as u8;
    }
    out
}

/// Skew of the text lines in degrees, by projection-profile search over
/// ±`MAX_SKEW`. Positive means the content is rotated clockwise on screen;
/// `rotate_small(img, -angle)` straightens it. Returns 0 for pages with too
/// little ink to tell.
pub fn estimate_skew(gray: &GrayImage) -> f32 {
    const MAX_SKEW: f32 = 10.0;
    let small = shrink(gray, 800);
    let (w, h) = small.dimensions();
    let t = otsu_threshold(&small);
    let ink: Vec<(f32, f32)> = small
        .enumerate_pixels()
        .filter(|(_, _, p)| p[0] <= t)
        .map(|(x, y, _)| (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0))
        .collect();
    // No contrast (Otsu splits noise) or almost no ink: nothing to measure.
    if ink.len() < 50 || ink.len() * 2 > (w * h) as usize {
        return 0.0;
    }
    // Sharpness of the row histogram once the page is counter-rotated by `deg`.
    let bins = (h as usize) * 2 + 1;
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut hist = vec![0u32; bins];
        for &(x, y) in &ink {
            let row = (y * c - x * s + h as f32).round();
            if row >= 0.0 && (row as usize) < bins {
                hist[row as usize] += 1;
            }
        }
        hist.iter().map(|&n| f64::from(n) * f64::from(n)).sum()
    };
    let search = |centre: f32, span: f32, step: f32| -> f32 {
        let steps = (2.0 * span / step).round() as i32;
        (0..=steps)
            .map(|i| centre - span + i as f32 * step)
            .map(|a| (a, score(a)))
            // Near-ties go to the angle closest to zero, so an upright page reads 0.
            .fold((centre, f64::MIN), |best, cur| {
                let clearly_better = cur.1 > best.1 * 1.002;
                let tie_but_smaller = cur.1 >= best.1 * 0.998 && cur.0.abs() < best.0.abs();
                if clearly_better || tie_but_smaller {
                    cur
                } else {
                    best
                }
            })
            .0
    };
    let coarse = search(0.0, MAX_SKEW, 0.5);
    let fine = search(coarse, 0.5, 0.05);
    if fine.abs() < 0.05 {
        0.0
    } else {
        fine
    }
}

/// Fraction of the page covered by ink, ignoring a 3% margin (scanner edge
/// shadows, punch holes) and isolated specks (dust).
pub fn ink_coverage(img: &RgbaImage) -> f32 {
    let small = shrink(&to_gray(img), 400);
    let (w, h) = small.dimensions();
    let (mx, my) = ((w * 3).div_ceil(100), (h * 3).div_ceil(100));
    if w <= 2 * mx + 2 || h <= 2 * my + 2 {
        return 0.0;
    }
    // Paper tone: the median gray level. Ink is clearly darker than paper.
    let mut hist = [0u32; 256];
    for p in small.pixels() {
        hist[p[0] as usize] += 1;
    }
    let half = (w * h) / 2;
    let mut seen = 0u32;
    let paper = hist.iter().position(|&n| {
        seen += n;
        seen > half
    });
    let cutoff = paper.unwrap_or(255).saturating_sub(64) as u8;
    let dark = |x: u32, y: u32| small.get_pixel(x, y)[0] < cutoff;
    let mut ink = 0u32;
    for y in my..h - my {
        for x in mx..w - mx {
            if !dark(x, y) {
                continue;
            }
            // A speck has no dark neighbour; real strokes do.
            let neighbours = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)];
            if neighbours.iter().any(|&(nx, ny)| dark(nx, ny)) {
                ink += 1;
            }
        }
    }
    ink as f32 / ((w - 2 * mx) * (h - 2 * my)) as f32
}

/// Blank-page detection: ink coverage under `sensitivity`
/// (see [`crate::BLANK_SENSITIVITY`]).
pub fn is_blank(img: &RgbaImage, sensitivity: f32) -> bool {
    ink_coverage(img) < sensitivity
}

pub(crate) fn gray_of(img: &RgbaImage) -> GrayImage {
    to_gray(img)
}
