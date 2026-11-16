//! Hover intent for a menu whose submenu opens when the pointer rests on an item (the "safe
//! triangle", also called menu-aim: the trick behind Amazon's drop-down menu).
//!
//! The trouble it solves: when the pointer travels from the item that opened a submenu toward the
//! submenu, it crosses other items on the way, and a naive menu would swap to them and close the
//! submenu under the pointer. The remedy: while the pointer stays inside the triangle from the point
//! where it left the item to the two near corners of the submenu, it is taken to be aiming at the
//! submenu and the submenu stays open. Aiming is only tolerated while the pointer keeps moving and
//! for a short time, so a pointer that stops or wanders gets the item under it as usual.
//!
//! Windows' own cascading menus (Explorer's "Sheaf" entry) do this themselves; this is for our own
//! menus: the action wheel's flyouts. The module is pure so that it can be tested without a screen.

use eframe::egui::Pos2;

/// Longest the pointer may be aiming before the menu follows it again, in seconds.
pub const AIM_MAX: f64 = 0.8;
/// How long the pointer may stand still inside the triangle before the menu follows it, in seconds.
pub const STILL_MAX: f64 = 0.25;

/// Is `p` inside (or on the edge of) the triangle `a`, `b`, `c`?
pub fn in_triangle(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let side = |p: Pos2, q: Pos2, r: Pos2| (p.x - r.x) * (q.y - r.y) - (q.x - r.x) * (p.y - r.y);
    let (d1, d2, d3) = (side(p, a, b), side(p, b, c), side(p, c, a));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// What the menu knows about this frame.
pub struct Frame<'a> {
    /// Seconds, from any steady clock.
    pub time: f64,
    /// `None` when the pointer is not over the menu's window at all.
    pub pointer: Option<Pos2>,
    /// The item under the pointer, if any.
    pub item: Option<usize>,
    /// The pointer is over a part of the open submenu.
    pub over_submenu: bool,
    /// Whether an item has a submenu to open.
    pub has_submenu: &'a dyn Fn(usize) -> bool,
    /// The edge of the open submenu that faces the menu: its two corners.
    pub near_edge: Option<(Pos2, Pos2)>,
}

#[derive(Debug, Default)]
pub struct Aim {
    open: Option<usize>,
    /// Where the pointer last was on the item that owns the open submenu: the triangle's tip.
    apex: Option<Pos2>,
    last: Option<Pos2>,
    last_move: f64,
    aiming_since: Option<f64>,
}

impl Aim {
    /// The item whose submenu is open.
    pub fn open(&self) -> Option<usize> {
        self.open
    }

    /// Is the pointer being given the benefit of the doubt right now (in the triangle, not yet on
    /// the submenu)?
    pub fn aiming(&self) -> bool {
        self.aiming_since.is_some()
    }

    /// Decide which submenu is open for this frame.
    pub fn step(&mut self, f: &Frame) -> Option<usize> {
        if self
            .last
            .is_none_or(|l| f.pointer.is_none_or(|p| (p - l).length() > 0.5))
        {
            self.last_move = f.time;
        }
        let next = self.decide(f);
        self.last = f.pointer;
        if next != self.open {
            self.aiming_since = None;
        }
        if next.is_none() {
            self.apex = None;
        }
        self.open = next;
        next
    }

    fn decide(&mut self, f: &Frame) -> Option<usize> {
        let p = f.pointer?;
        if let Some(open) = self.open {
            if f.over_submenu {
                self.aiming_since = None;
                return Some(open);
            }
            if f.item == Some(open) {
                self.apex = Some(p);
                self.aiming_since = None;
                return Some(open);
            }
            if let (Some(apex), Some((a, b))) = (self.apex, f.near_edge) {
                if in_triangle(p, apex, a, b) {
                    let since = *self.aiming_since.get_or_insert(f.time);
                    if f.time - since < AIM_MAX && f.time - self.last_move < STILL_MAX {
                        return Some(open);
                    }
                }
            }
        }
        self.aiming_since = None;
        let item = f.item.filter(|i| (f.has_submenu)(*i));
        if item.is_some() && item != self.open {
            self.apex = Some(p);
        }
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    #[test]
    fn the_triangle_includes_its_inside_and_edges_and_nothing_else() {
        let (a, b, c) = (pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(0.0, 10.0));
        for p in [pos2(1.0, 1.0), pos2(0.0, 0.0), pos2(5.0, 5.0), pos2(5.0, 0.0)] {
            assert!(in_triangle(p, a, b, c), "{p:?}");
        }
        for p in [pos2(-1.0, 1.0), pos2(6.0, 6.0), pos2(11.0, 0.0), pos2(1.0, -0.1)] {
            assert!(!in_triangle(p, a, b, c), "{p:?}");
        }
        // The same shape the other way round.
        assert!(in_triangle(pos2(1.0, 1.0), c, b, a));
    }

    // A menu with items 0..3 stacked down the left, item 1 owning a submenu to its right whose
    // near edge runs from (100, 20) to (100, 80). Items are 30 high, 0..100 wide.
    fn item_at(p: Pos2) -> Option<usize> {
        (p.x >= 0.0 && p.x < 100.0 && p.y >= 0.0 && p.y < 90.0).then(|| (p.y / 30.0) as usize)
    }
    const NEAR: (Pos2, Pos2) = (Pos2::new(100.0, 20.0), Pos2::new(100.0, 80.0));
    fn over_sub(p: Pos2) -> bool {
        p.x >= 100.0 && p.x < 220.0 && p.y >= 20.0 && p.y < 80.0
    }

    fn frame(t: f64, p: Option<Pos2>, open: bool) -> Frame<'static> {
        Frame {
            time: t,
            pointer: p,
            item: p.and_then(item_at),
            over_submenu: p.is_some_and(over_sub),
            has_submenu: &|i| i == 1,
            near_edge: open.then_some(NEAR),
        }
    }

    /// Move the pointer along a path, 10 ms per step, and report whether the submenu is open at the end.
    fn walk(aim: &mut Aim, start: f64, path: &[Pos2]) -> (f64, Option<usize>) {
        let mut t = start;
        let mut open = aim.open().is_some();
        for p in path {
            t += 0.01;
            let r = aim.step(&frame(t, Some(*p), open));
            open = r.is_some();
        }
        (t, aim.open())
    }

    fn line(from: Pos2, to: Pos2, n: usize) -> Vec<Pos2> {
        (1..=n).map(|i| from + (to - from) * (i as f32 / n as f32)).collect()
    }

    #[test]
    fn resting_on_an_item_with_a_submenu_opens_it() {
        let mut aim = Aim::default();
        let (_, open) = walk(&mut aim, 0.0, &[pos2(50.0, 45.0)]);
        assert_eq!(open, Some(1));
        let mut aim = Aim::default();
        let (_, open) = walk(&mut aim, 0.0, &[pos2(50.0, 15.0)]);
        assert_eq!(open, None, "item 0 has no submenu");
    }

    #[test]
    fn a_diagonal_path_across_another_item_toward_the_submenu_keeps_it_open() {
        let mut aim = Aim::default();
        let (t, _) = walk(&mut aim, 0.0, &[pos2(60.0, 35.0)]);
        assert_eq!(aim.open(), Some(1));
        // From the middle of item 1 down and right, across item 2, to the lower part of the submenu's edge:
        // without the triangle the pointer would be "on item 2" and the submenu would close.
        let path = line(pos2(60.0, 35.0), pos2(99.0, 78.0), 8);
        let crosses_item_2 = path.iter().any(|p| item_at(*p) == Some(2));
        assert!(crosses_item_2, "the test path does cross another item");
        let (t, open) = walk(&mut aim, t, &path);
        assert_eq!(open, Some(1), "still open: the pointer is aiming at the submenu");
        let (_, open) = walk(&mut aim, t, &[pos2(110.0, 70.0), pos2(150.0, 70.0)]);
        assert_eq!(open, Some(1), "and it is open when the pointer arrives");
    }

    #[test]
    fn a_path_away_from_the_submenu_follows_the_pointer_at_once() {
        let mut aim = Aim::default();
        let (t, _) = walk(&mut aim, 0.0, &[pos2(60.0, 35.0)]);
        let (_, open) = walk(&mut aim, t, &line(pos2(60.0, 35.0), pos2(20.0, 80.0), 6));
        assert_eq!(open, None, "heading down-left is not aiming at the submenu");
    }

    #[test]
    fn a_pointer_that_stops_in_the_triangle_or_aims_too_long_is_followed() {
        let mut aim = Aim::default();
        let (t, _) = walk(&mut aim, 0.0, &[pos2(60.0, 35.0)]);
        // Into the triangle, onto item 2, then standing still there.
        let (t, open) = walk(&mut aim, t, &[pos2(85.0, 62.0)]);
        assert_eq!(open, Some(1));
        let mut t = t;
        let mut open = Some(1);
        for _ in 0..40 {
            t += 0.01;
            open = aim.step(&frame(t, Some(pos2(85.0, 62.0)), true));
        }
        assert_eq!(
            open, None,
            "after standing still for a moment, item 2 is what the pointer is on"
        );

        // Moving the whole time, but for longer than the tolerance: also followed.
        let mut aim = Aim::default();
        let (mut t, _) = walk(&mut aim, 0.0, &[pos2(60.0, 35.0)]);
        let mut x = 60.0;
        let mut still_open = true;
        for i in 0..120 {
            t += 0.01;
            // Wobble inside the triangle without ever reaching the submenu.
            x = if i % 2 == 0 { 84.0 } else { 86.0 };
            still_open = aim.step(&frame(t, Some(pos2(x, 63.0)), true)).is_some();
        }
        let _ = x;
        assert!(!still_open, "aiming is tolerated for a limited time");
    }

    #[test]
    fn leaving_the_window_closes_and_moving_to_another_submenu_item_switches() {
        let mut aim = Aim::default();
        let (t, _) = walk(&mut aim, 0.0, &[pos2(60.0, 35.0)]);
        assert_eq!(aim.step(&frame(t + 0.01, None, true)), None);
        // A menu with two items that both have submenus switches directly.
        let both = |t: f64, p: Pos2, open: bool| Frame {
            time: t,
            pointer: Some(p),
            item: item_at(p),
            over_submenu: over_sub(p),
            has_submenu: &|i| i <= 1,
            near_edge: open.then_some(NEAR),
        };
        let mut aim = Aim::default();
        assert_eq!(aim.step(&both(0.0, pos2(50.0, 45.0), false)), Some(1));
        // Straight up, away from the triangle, onto item 0.
        assert_eq!(aim.step(&both(0.5, pos2(50.0, 10.0), true)), Some(0));
    }
}
