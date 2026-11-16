//! The few shared pieces every dialog and form is built from, so they stop being a column of equal
//! labels and equal buttons: a label that sits *above* its control and is quieter than it, a segmented
//! choice for two to four options, and a footer that puts the one primary action on the right.

use crate::theme::Palette;
use eframe::egui::{self, vec2, Color32, RichText};

/// A caption for the control below it: small and quiet, with a little air above to set it apart from
/// the control before.
pub fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).small().color(Palette::of(ui).weak));
}

/// One of a few options, shown side by side in a shared inset: the chosen one is washed with the
/// accent. `options` are `(value, label, enabled)`.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, String, bool)]) {
    let p = Palette::of(ui);
    egui::Frame::new()
        .fill(p.inset)
        .corner_radius(7)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (v, label, enabled) in options {
                    ui.add_enabled_ui(*enabled, |ui| {
                        if crate::icons::chip(ui, label, *value == *v).clicked() {
                            *value = *v;
                        }
                    });
                }
            });
        });
}

/// What was pressed in a [`footer`].
#[derive(Default)]
pub struct Pressed {
    pub primary: bool,
    /// Index into the secondary actions that were given.
    pub secondary: Option<usize>,
}

/// The row at the foot of a dialog: a hairline, then the secondary actions and, last and filled, the
/// one primary action, all on the right. `secondary` is `(label, enabled)` in reading order.
pub fn footer(ui: &mut egui::Ui, primary: &str, primary_enabled: bool, secondary: &[(String, bool)]) -> Pressed {
    let p = Palette::of(ui);
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    let mut pressed = Pressed::default();
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let button = egui::Button::new(RichText::new(primary).color(p.on_accent))
            .fill(p.accent)
            .min_size(vec2(84.0, 28.0));
        pressed.primary = ui.add_enabled(primary_enabled, button).clicked();
        // Right to left, so the last of the list is laid out first and the order on screen is the given one.
        for (i, (label, enabled)) in secondary.iter().enumerate().rev() {
            if ui
                .add_enabled(*enabled, egui::Button::new(label.as_str()).min_size(vec2(72.0, 28.0)))
                .clicked()
            {
                pressed.secondary = Some(i);
            }
        }
    });
    pressed
}

/// The one action of a form, as wide as the form: filled, with room above it.
pub fn primary_wide(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    let p = Palette::of(ui);
    ui.add_space(10.0);
    let button = egui::Button::new(RichText::new(label).color(p.on_accent))
        .fill(p.accent)
        .min_size(vec2(ui.available_width(), 30.0));
    ui.add_enabled(enabled, button).clicked()
}

/// "‹ Back" as a quiet link, not a button: leaving a form is not an action.
pub fn back_link(ui: &mut egui::Ui, label: &str) -> bool {
    let p = Palette::of(ui);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, Color32::PLACEHOLDER);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 24.0), egui::Sense::click());
    let colour = if response.hovered() { p.text } else { p.weak };
    crate::icons::paint(
        ui.painter(),
        egui::Rect::from_min_size(rect.min + vec2(0.0, 5.0), vec2(14.0, 14.0)),
        crate::icons::Icon::Prev,
        colour,
    );
    ui.painter().galley(
        egui::pos2(rect.min.x + 18.0, rect.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// How a job's bar is drawn.
pub enum Track {
    /// A known fraction, 0.0 to 1.0.
    Fraction(f32),
    /// Length unknown (a feeder does not say how many sheets it holds): a segment that keeps moving.
    Moving,
}

/// A slim progress track: a hairline-thin groove with a rounded fill, not a 28-point slab. The
/// percentage is the caller's to place beside it (in a fixed-width figure, so rows line up).
pub fn track(ui: &mut egui::Ui, kind: Track, fill: Color32, width: f32) {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 16.0), egui::Sense::hover());
    let groove = egui::Rect::from_center_size(rect.center(), vec2(width, 6.0));
    ui.painter().rect_filled(groove, 3.0, p.pressed);
    match kind {
        Track::Fraction(f) => {
            let f = f.clamp(0.0, 1.0);
            if f > 0.0 {
                // Never thinner than the groove is tall, so 1 % is still a visible dot, not nothing.
                let w = (groove.width() * f).max(groove.height());
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(groove.min, vec2(w, groove.height())),
                    3.0,
                    fill,
                );
            }
        }
        Track::Moving => {
            let t = ui.input(|i| i.time) as f32;
            let seg = groove.width() * 0.35;
            let x = ((t * 0.8) % 1.35 - 0.35) * groove.width();
            let r = egui::Rect::from_min_size(groove.min + vec2(x, 0.0), vec2(seg, groove.height())).intersect(groove);
            if r.width() > 0.0 {
                ui.painter().rect_filled(r, 3.0, fill);
            }
            ui.ctx().request_repaint();
        }
    }
}
