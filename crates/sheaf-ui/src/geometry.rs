//! Pure geometry for crop and OCR overlays. A rectangle is in fractions of the
//! page, origin top-left.

pub const MIN_SIDE: f32 = 0.01; // matches NormRect::MIN_SIDE in sheaf-core

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect01 {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

fn corner(x: f32, y: f32, rot: i32, to_page: bool) -> (f32, f32) {
    match (rot.rem_euclid(360), to_page) {
        (90, true) => (y, 1.0 - x),
        (90, false) => (1.0 - y, x),
        (180, _) => (1.0 - x, 1.0 - y),
        (270, true) => (1.0 - y, x),
        (270, false) => (y, 1.0 - x),
        _ => (x, y),
    }
}

fn map(r: Rect01, rot: i32, to_page: bool) -> Rect01 {
    let (ax, ay) = corner(r.x0, r.y0, rot, to_page);
    let (bx, by) = corner(r.x1, r.y1, rot, to_page);
    Rect01 {
        x0: ax.min(bx),
        y0: ay.min(by),
        x1: ax.max(bx),
        y1: ay.max(by),
    }
}

/// Crop rectangles are stored in the page's frame *before* its rotation (DATA-PAGE).
pub fn display_to_page(r: Rect01, rot: i32) -> Rect01 {
    map(r, rot, true)
}

/// Where a page-frame rectangle appears once the page is turned.
pub fn page_to_display(r: Rect01, rot: i32) -> Rect01 {
    map(r, rot, false)
}

/// Keep a rectangle inside the page and at least [`MIN_SIDE`] wide and high.
pub fn normalize(r: Rect01) -> Rect01 {
    let (mut x0, mut x1) = (r.x0.min(r.x1), r.x0.max(r.x1));
    let (mut y0, mut y1) = (r.y0.min(r.y1), r.y0.max(r.y1));
    x0 = x0.clamp(0.0, 1.0 - MIN_SIDE);
    y0 = y0.clamp(0.0, 1.0 - MIN_SIDE);
    x1 = x1.clamp(x0 + MIN_SIDE, 1.0);
    y1 = y1.clamp(y0 + MIN_SIDE, 1.0);
    Rect01 { x0, y0, x1, y1 }
}

/// Which part of the crop frame is being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

/// Move a handle by a fractional delta; the result stays on the page.
pub fn drag_rect(r: Rect01, h: Handle, dx: f32, dy: f32) -> Rect01 {
    if h == Handle::Move {
        let (w, hgt) = (r.x1 - r.x0, r.y1 - r.y0);
        let x0 = (r.x0 + dx).clamp(0.0, 1.0 - w);
        let y0 = (r.y0 + dy).clamp(0.0, 1.0 - hgt);
        return Rect01 {
            x0,
            y0,
            x1: x0 + w,
            y1: y0 + hgt,
        };
    }
    let mut o = r;
    if matches!(h, Handle::W | Handle::NW | Handle::SW) {
        o.x0 += dx;
    }
    if matches!(h, Handle::E | Handle::NE | Handle::SE) {
        o.x1 += dx;
    }
    if matches!(h, Handle::N | Handle::NW | Handle::NE) {
        o.y0 += dy;
    }
    if matches!(h, Handle::S | Handle::SW | Handle::SE) {
        o.y1 += dy;
    }
    normalize(o)
}

/// Confidence class of an OCR word: `Low` under 60, `Mid` under 80, otherwise `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conf {
    Low,
    Mid,
    Ok,
}

pub fn conf_class(conf: f32) -> Conf {
    if conf < 60.0 {
        Conf::Low
    } else if conf < 80.0 {
        Conf::Mid
    } else {
        Conf::Ok
    }
}

/// An OCR word box (`left, top, width, height` in image pixels) as fractions of its image.
pub fn word_box(bbox: [u32; 4], image_px: (u32, u32)) -> Option<Rect01> {
    let (w, h) = (image_px.0 as f32, image_px.1 as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(Rect01 {
        x0: bbox[0] as f32 / w,
        y0: bbox[1] as f32 / h,
        x1: (bbox[0] + bbox[2]) as f32 / w,
        y1: (bbox[1] + bbox[3]) as f32 / h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Rect01, b: Rect01) -> bool {
        [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)]
            .iter()
            .all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn crop_rectangles_round_trip_through_every_rotation() {
        let r = Rect01 {
            x0: 0.1,
            y0: 0.2,
            x1: 0.6,
            y1: 0.9,
        };
        for rot in [0, 90, 180, 270, -90, 360] {
            assert!(near(display_to_page(page_to_display(r, rot), rot), r), "rot {rot}");
        }
    }

    #[test]
    fn a_page_turned_clockwise_shows_its_top_left_at_the_top_right() {
        let shown = page_to_display(
            Rect01 {
                x0: 0.0,
                y0: 0.0,
                x1: 0.25,
                y1: 0.5,
            },
            90,
        );
        assert!(
            near(
                shown,
                Rect01 {
                    x0: 0.5,
                    y0: 0.0,
                    x1: 1.0,
                    y1: 0.25
                }
            ),
            "{shown:?}"
        );
        let page = display_to_page(
            Rect01 {
                x0: 0.5,
                y0: 0.0,
                x1: 1.0,
                y1: 0.25,
            },
            90,
        );
        assert!(
            near(
                page,
                Rect01 {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 0.25,
                    y1: 0.5
                }
            ),
            "{page:?}"
        );
    }

    #[test]
    fn rectangles_stay_on_the_page_and_not_thinner_than_the_core_allows() {
        let r = normalize(Rect01 {
            x0: 0.9,
            y0: -1.0,
            x1: 0.2,
            y1: 5.0,
        });
        assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= 1.0 && r.y1 <= 1.0);
        let thin = normalize(Rect01 {
            x0: 0.5,
            y0: 0.5,
            x1: 0.5,
            y1: 0.5,
        });
        assert!(thin.x1 - thin.x0 >= MIN_SIDE - 1e-6 && thin.y1 - thin.y0 >= MIN_SIDE - 1e-6);
    }

    #[test]
    fn dragging_moves_resizes_and_never_leaves_the_page() {
        let r = Rect01 {
            x0: 0.2,
            y0: 0.2,
            x1: 0.6,
            y1: 0.6,
        };
        let moved = drag_rect(r, Handle::Move, 0.9, -0.9);
        assert!(((moved.x1 - moved.x0) - 0.4).abs() < 1e-5 && moved.x1 <= 1.0 && moved.y0 >= 0.0);
        let grown = drag_rect(r, Handle::SE, 0.1, 0.1);
        assert!((grown.x1 - 0.7).abs() < 1e-5 && (grown.y1 - 0.7).abs() < 1e-5 && (grown.x0 - 0.2).abs() < 1e-5);
        let past = drag_rect(r, Handle::W, 2.0, 0.0);
        assert!(past.x1 - past.x0 >= MIN_SIDE - 1e-6 && past.x1 <= 1.0);
    }

    #[test]
    fn ocr_boxes_become_fractions_of_their_image() {
        let b = word_box([100, 50, 200, 25], (1000, 500)).expect("box");
        assert!(near(
            b,
            Rect01 {
                x0: 0.1,
                y0: 0.1,
                x1: 0.3,
                y1: 0.15
            }
        ));
        assert!(word_box([1, 2, 3, 4], (0, 0)).is_none());
        assert_eq!(
            [conf_class(30.0), conf_class(70.0), conf_class(95.0)],
            [Conf::Low, Conf::Mid, Conf::Ok]
        );
    }
}
