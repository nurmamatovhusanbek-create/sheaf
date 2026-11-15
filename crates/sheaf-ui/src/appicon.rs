//! The app's icon, drawn here at any size so no image file has to ship: a wheel of eight
//! sectors with a gap between them, the shape of the action wheel (ADR-WHEEL). The window,
//! the taskbar and the tray all use it.

/// Straight RGBA, `n` by `n` pixels, with edges smoothed by coverage.
pub fn rgba(n: u32) -> Vec<u8> {
    let n = n.clamp(16, 512) as usize;
    let scale = n as f32 / 32.0;
    let centre = (n as f32 - 1.0) / 2.0;
    let (inner, outer) = (8.0 * scale, 15.0 * scale);
    let sector = std::f32::consts::TAU / 8.0;
    let mut out = vec![0u8; n * n * 4];
    // 4x4 samples per pixel give the edges their softness.
    const SS: usize = 4;
    for y in 0..n {
        for x in 0..n {
            let (mut covered, mut r_sum, mut g_sum, mut b_sum) = (0usize, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32 - 0.5;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32 - 0.5;
                    let (dx, dy) = (px - centre, py - centre);
                    let r = (dx * dx + dy * dy).sqrt();
                    if !(inner..=outer).contains(&r) {
                        continue;
                    }
                    let angle = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
                    // A thin gap between sectors, as wide as one pixel at 32.
                    if (angle % sector) * r < scale {
                        continue;
                    }
                    let shade = if ((angle / sector) as usize).is_multiple_of(2) {
                        [0x1f, 0x5f, 0xd6]
                    } else {
                        [0x4a, 0x7d, 0xe8]
                    };
                    covered += 1;
                    r_sum += shade[0] as f32;
                    g_sum += shade[1] as f32;
                    b_sum += shade[2] as f32;
                }
            }
            if covered > 0 {
                let i = (y * n + x) * 4;
                let c = covered as f32;
                out[i] = (r_sum / c).round() as u8;
                out[i + 1] = (g_sum / c).round() as u8;
                out[i + 2] = (b_sum / c).round() as u8;
                out[i + 3] = ((covered * 255) / (SS * SS)) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_ring_with_a_clear_middle_and_corners() {
        for n in [16u32, 32, 48, 256] {
            let px = rgba(n);
            assert_eq!(px.len(), (n * n * 4) as usize);
            let alpha = |x: u32, y: u32| px[((y * n + x) * 4 + 3) as usize];
            assert_eq!(alpha(0, 0), 0, "corner at {n}");
            assert_eq!(alpha(n / 2, n / 2), 0, "hole at {n}");
            // Somewhere on the ring, at its mid radius, it is solid.
            let ring_y = (n as f32 / 2.0 - 11.5 * n as f32 / 32.0).round() as u32;
            let solid = (0..n).any(|x| alpha(x, ring_y) == 255);
            assert!(solid, "ring at {n}");
        }
    }
}
