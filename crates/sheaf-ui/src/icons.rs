//! Vector icons drawn with the painter on a 24 by 24 grid: crisp at any scale
//! and in any theme, and nothing to ship or to go missing from a font.

use eframe::egui::{self, pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

/// The icon set. Not every icon is used yet; they are cheap and the next screens will want them.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Open,
    Scan,
    Ocr,
    Export,
    Undo,
    Redo,
    Search,
    Settings,
    RotateCw,
    RotateCcw,
    ZoomIn,
    ZoomOut,
    Fit,
    Crop,
    Text,
    Trash,
    Copy,
    Blank,
    Lock,
    Unlock,
    Stamp,
    Split,
    Merge,
    Compress,
    Image,
    Extract,
    Presets,
    Close,
    Check,
    Prev,
    Next,
    Folder,
    Filter,
    Bell,
    Help,
    Alert,
    Update,
}

enum Seg {
    Line(&'static [(f32, f32)]),
    Closed(&'static [(f32, f32)]),
    Circle((f32, f32), f32),
    /// centre, radius, start and end angle in degrees, clockwise from the top
    Arc((f32, f32), f32, f32, f32),
}

use Seg::*;

fn segs(icon: Icon) -> &'static [Seg] {
    match icon {
        Icon::Open | Icon::Folder => &[Closed(&[
            (3.0, 6.0),
            (9.0, 6.0),
            (11.0, 9.0),
            (21.0, 9.0),
            (21.0, 19.0),
            (3.0, 19.0),
        ])],
        Icon::Scan => &[
            Line(&[(6.0, 14.0), (6.0, 5.0), (18.0, 5.0), (18.0, 14.0)]),
            Closed(&[(3.0, 14.0), (21.0, 14.0), (21.0, 19.0), (3.0, 19.0)]),
        ],
        Icon::Ocr => &[
            Line(&[(4.0, 8.0), (4.0, 4.0), (8.0, 4.0)]),
            Line(&[(16.0, 4.0), (20.0, 4.0), (20.0, 8.0)]),
            Line(&[(4.0, 16.0), (4.0, 20.0), (8.0, 20.0)]),
            Line(&[(16.0, 20.0), (20.0, 20.0), (20.0, 16.0)]),
            Line(&[(8.0, 10.0), (16.0, 10.0)]),
            Line(&[(8.0, 14.0), (13.0, 14.0)]),
        ],
        Icon::Export => &[
            Line(&[(12.0, 4.0), (12.0, 15.0)]),
            Line(&[(7.0, 10.5), (12.0, 15.5), (17.0, 10.5)]),
            Line(&[(4.0, 20.0), (20.0, 20.0)]),
        ],
        Icon::Undo => &[
            Line(&[(9.0, 5.0), (4.0, 10.0), (9.0, 15.0)]),
            Line(&[(4.0, 10.0), (14.0, 10.0), (19.0, 12.0), (20.0, 17.0)]),
        ],
        Icon::Redo => &[
            Line(&[(15.0, 5.0), (20.0, 10.0), (15.0, 15.0)]),
            Line(&[(20.0, 10.0), (10.0, 10.0), (5.0, 12.0), (4.0, 17.0)]),
        ],
        Icon::Search => &[Circle((10.0, 10.0), 6.0), Line(&[(14.5, 14.5), (20.0, 20.0)])],
        Icon::Settings => &[
            Line(&[(4.0, 7.0), (20.0, 7.0)]),
            Line(&[(4.0, 12.0), (20.0, 12.0)]),
            Line(&[(4.0, 17.0), (20.0, 17.0)]),
            Circle((9.0, 7.0), 2.2),
            Circle((15.0, 12.0), 2.2),
            Circle((8.0, 17.0), 2.2),
        ],
        Icon::RotateCw => &[
            Arc((12.0, 12.0), 7.5, 20.0, 330.0),
            Line(&[(18.5, 2.5), (19.5, 6.5), (15.5, 7.5)]),
        ],
        Icon::RotateCcw => &[
            Arc((12.0, 12.0), 7.5, 30.0, 340.0),
            Line(&[(5.5, 2.5), (4.5, 6.5), (8.5, 7.5)]),
        ],
        Icon::ZoomIn => &[
            Circle((10.0, 10.0), 6.0),
            Line(&[(14.5, 14.5), (20.0, 20.0)]),
            Line(&[(7.5, 10.0), (12.5, 10.0)]),
            Line(&[(10.0, 7.5), (10.0, 12.5)]),
        ],
        Icon::ZoomOut => &[
            Circle((10.0, 10.0), 6.0),
            Line(&[(14.5, 14.5), (20.0, 20.0)]),
            Line(&[(7.5, 10.0), (12.5, 10.0)]),
        ],
        Icon::Fit => &[
            Line(&[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)]),
            Line(&[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)]),
            Line(&[(4.0, 15.0), (4.0, 20.0), (9.0, 20.0)]),
            Line(&[(15.0, 20.0), (20.0, 20.0), (20.0, 15.0)]),
        ],
        Icon::Crop => &[
            Line(&[(7.0, 3.0), (7.0, 17.0), (21.0, 17.0)]),
            Line(&[(3.0, 7.0), (17.0, 7.0), (17.0, 21.0)]),
        ],
        Icon::Text => &[
            Line(&[(5.0, 6.0), (19.0, 6.0)]),
            Line(&[(12.0, 6.0), (12.0, 19.0)]),
            Line(&[(9.0, 19.0), (15.0, 19.0)]),
        ],
        Icon::Trash => &[
            Line(&[(4.0, 7.0), (20.0, 7.0)]),
            Line(&[(9.0, 7.0), (9.0, 4.0), (15.0, 4.0), (15.0, 7.0)]),
            Line(&[(6.5, 7.0), (7.5, 20.0), (16.5, 20.0), (17.5, 7.0)]),
            Line(&[(10.0, 11.0), (10.0, 16.0)]),
            Line(&[(14.0, 11.0), (14.0, 16.0)]),
        ],
        Icon::Copy => &[
            Closed(&[(9.0, 9.0), (20.0, 9.0), (20.0, 20.0), (9.0, 20.0)]),
            Line(&[(5.0, 15.0), (4.0, 15.0), (4.0, 4.0), (15.0, 4.0), (15.0, 5.0)]),
        ],
        Icon::Blank => &[
            Line(&[
                (6.0, 3.0),
                (14.0, 3.0),
                (18.0, 7.0),
                (18.0, 21.0),
                (6.0, 21.0),
                (6.0, 3.0),
            ]),
            Line(&[(12.0, 11.0), (12.0, 17.0)]),
            Line(&[(9.0, 14.0), (15.0, 14.0)]),
        ],
        Icon::Lock => &[
            Closed(&[(5.0, 11.0), (19.0, 11.0), (19.0, 21.0), (5.0, 21.0)]),
            Arc((12.0, 11.0), 4.5, 270.0, 90.0 + 360.0),
        ],
        Icon::Unlock => &[
            Closed(&[(5.0, 11.0), (19.0, 11.0), (19.0, 21.0), (5.0, 21.0)]),
            Arc((12.0, 9.0), 4.5, 270.0, 90.0 + 360.0 - 90.0),
        ],
        Icon::Stamp => &[
            Line(&[(4.0, 20.0), (20.0, 20.0)]),
            Closed(&[
                (6.0, 15.0),
                (18.0, 15.0),
                (18.0, 12.0),
                (14.0, 11.0),
                (14.0, 8.0),
                (10.0, 8.0),
                (10.0, 11.0),
                (6.0, 12.0),
            ]),
        ],
        Icon::Split => &[
            Line(&[(12.0, 3.0), (12.0, 21.0)]),
            Line(&[(7.0, 8.0), (3.0, 12.0), (7.0, 16.0)]),
            Line(&[(17.0, 8.0), (21.0, 12.0), (17.0, 16.0)]),
        ],
        Icon::Merge => &[
            Line(&[(6.0, 4.0), (6.0, 10.0), (12.0, 16.0)]),
            Line(&[(18.0, 4.0), (18.0, 10.0), (12.0, 16.0)]),
            Line(&[(12.0, 16.0), (12.0, 21.0)]),
        ],
        Icon::Compress => &[
            Line(&[(4.0, 4.0), (10.0, 10.0)]),
            Line(&[(10.0, 5.0), (10.0, 10.0), (5.0, 10.0)]),
            Line(&[(20.0, 4.0), (14.0, 10.0)]),
            Line(&[(14.0, 5.0), (14.0, 10.0), (19.0, 10.0)]),
            Line(&[(4.0, 20.0), (10.0, 14.0)]),
            Line(&[(10.0, 19.0), (10.0, 14.0), (5.0, 14.0)]),
            Line(&[(20.0, 20.0), (14.0, 14.0)]),
            Line(&[(14.0, 19.0), (14.0, 14.0), (19.0, 14.0)]),
        ],
        Icon::Image => &[
            Closed(&[(4.0, 5.0), (20.0, 5.0), (20.0, 19.0), (4.0, 19.0)]),
            Line(&[(4.0, 16.0), (9.0, 11.0), (13.0, 15.0), (16.0, 12.0), (20.0, 16.0)]),
            Circle((9.0, 9.0), 1.2),
        ],
        Icon::Extract => &[
            Line(&[
                (5.0, 3.0),
                (15.0, 3.0),
                (19.0, 7.0),
                (19.0, 21.0),
                (5.0, 21.0),
                (5.0, 3.0),
            ]),
            Line(&[(9.0, 14.0), (15.0, 14.0)]),
            Line(&[(12.0, 11.0), (12.0, 17.0)]),
        ],
        Icon::Presets => &[
            Closed(&[(4.0, 4.0), (11.0, 4.0), (11.0, 11.0), (4.0, 11.0)]),
            Closed(&[(13.0, 4.0), (20.0, 4.0), (20.0, 11.0), (13.0, 11.0)]),
            Closed(&[(4.0, 13.0), (11.0, 13.0), (11.0, 20.0), (4.0, 20.0)]),
            Closed(&[(13.0, 13.0), (20.0, 13.0), (20.0, 20.0), (13.0, 20.0)]),
        ],
        Icon::Close => &[Line(&[(6.0, 6.0), (18.0, 18.0)]), Line(&[(18.0, 6.0), (6.0, 18.0)])],
        Icon::Check => &[Line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)])],
        Icon::Prev => &[Line(&[(15.0, 5.0), (8.0, 12.0), (15.0, 19.0)])],
        Icon::Next => &[Line(&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)])],
        Icon::Bell => &[
            Closed(&[
                (6.0, 17.0),
                (6.0, 11.0),
                (8.0, 6.5),
                (12.0, 4.5),
                (16.0, 6.5),
                (18.0, 11.0),
                (18.0, 17.0),
            ]),
            Line(&[(4.0, 17.0), (20.0, 17.0)]),
            Line(&[(10.0, 20.5), (14.0, 20.5)]),
        ],
        Icon::Help => &[
            Circle((12.0, 12.0), 9.0),
            Arc((12.0, 9.5), 2.8, 270.0, 450.0),
            Line(&[(14.8, 9.5), (12.0, 13.0), (12.0, 14.8)]),
            Circle((12.0, 17.6), 0.5),
        ],
        Icon::Update => &[
            Circle((12.0, 12.0), 9.0),
            Line(&[(12.0, 16.5), (12.0, 7.5)]),
            Line(&[(8.0, 11.5), (12.0, 7.5), (16.0, 11.5)]),
        ],
        Icon::Alert => &[
            Closed(&[(12.0, 3.5), (21.0, 19.5), (3.0, 19.5)]),
            Line(&[(12.0, 9.5), (12.0, 14.0)]),
            Circle((12.0, 16.8), 0.5),
        ],
        Icon::Filter => &[Closed(&[
            (4.0, 5.0),
            (20.0, 5.0),
            (14.0, 13.0),
            (14.0, 19.0),
            (10.0, 17.0),
            (10.0, 13.0),
        ])],
    }
}

fn arc_points(c: (f32, f32), r: f32, a0: f32, a1: f32) -> Vec<(f32, f32)> {
    let steps = (((a1 - a0).abs() / 12.0).ceil() as usize).max(2);
    (0..=steps)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / steps as f32).to_radians();
            (c.0 + r * a.sin(), c.1 - r * a.cos())
        })
        .collect()
}

/// Draw `icon` into `rect` (kept square, centred) in `color`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    let scale = side / 24.0;
    let origin = rect.center() - vec2(side, side) / 2.0;
    let map = |p: (f32, f32)| origin + vec2(p.0, p.1) * scale;
    let stroke = Stroke::new((1.7 * scale).clamp(1.2, 2.6), color);
    for seg in segs(icon) {
        match seg {
            Line(points) => painter.add(Shape::line(
                points.iter().map(|p| map(*p)).collect::<Vec<Pos2>>(),
                stroke,
            )),
            Closed(points) => painter.add(Shape::closed_line(
                points.iter().map(|p| map(*p)).collect::<Vec<Pos2>>(),
                stroke,
            )),
            Circle(c, r) => painter.circle_stroke(map(*c), r * scale, stroke),
            Arc(c, r, a0, a1) => painter.add(Shape::line(
                arc_points(*c, *r, *a0, *a1).into_iter().map(map).collect::<Vec<Pos2>>(),
                stroke,
            )),
        };
    }
}

/// A square icon-only button.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> egui::Response {
    let size = vec2(30.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, 6.0, visuals.bg_fill);
    }
    let color = if ui.is_enabled() {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(18.0, 18.0)),
        icon,
        color,
    );
    response.on_hover_text(tooltip)
}

/// An icon with a label, as a flat button; `primary` fills it with the accent.
pub fn labeled_button(ui: &mut egui::Ui, icon: Icon, label: &str, primary: bool, tooltip: &str) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, Color32::PLACEHOLDER);
    let size = vec2(12.0 + 18.0 + 6.0 + galley.size().x + 12.0, 32.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let enabled = ui.is_enabled();
    let accent = ui.visuals().selection.bg_fill;
    let (fill, fg) = if primary {
        let f = if response.hovered() {
            accent.gamma_multiply(1.12)
        } else {
            accent
        };
        (Some(f), Color32::WHITE)
    } else {
        let f = response.hovered().then(|| ui.visuals().widgets.hovered.bg_fill);
        (f, ui.visuals().text_color())
    };
    let fg = if enabled { fg } else { fg.gamma_multiply(0.45) };
    if let Some(f) = fill {
        ui.painter()
            .rect_filled(rect, 7.0, if enabled { f } else { f.gamma_multiply(0.5) });
    }
    paint(
        ui.painter(),
        Rect::from_min_size(rect.min + vec2(12.0, 7.0), vec2(18.0, 18.0)),
        icon,
        fg,
    );
    ui.painter().galley(
        pos2(rect.min.x + 12.0 + 18.0 + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    response.on_hover_text(tooltip)
}
