//! Fonts and colors.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily};
use std::path::Path;
use std::sync::Arc;

/// Put the bundled Noto Sans first so Cyrillic and the Uzbek letters
/// (ў қ ғ ҳ) draw; egui's own fonts stay behind it for symbols.
pub fn install_fonts(ctx: &egui::Context, noto: Option<&Path>) {
    let Some(bytes) = noto.and_then(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("noto".into(), Arc::new(FontData::from_owned(bytes)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().insert(0, "noto".into());
    }
    ctx.set_fonts(fonts);
}

/// A dialog with room around its contents. egui's default leaves six points between a dialog's edge
/// and its title; every dialog of the app comes from here so they all breathe the same. The
/// content stays where it was (a dialog is centred, so growing the margin only grows the frame).
pub fn modal(ctx: &egui::Context, name: &'static str) -> egui::Modal {
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(18));
    egui::Modal::new(egui::Id::unique(name)).frame(frame)
}

/// `system`, `light` or `dark`.
pub fn apply(ctx: &egui::Context, theme: &str) {
    ctx.set_theme(match theme {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
    ctx.all_styles_mut(style_one);
}

/// The colours of the app, named for the world it lives in: paper on a desk, written on in ink.
///
/// One warm hue family for every surface (only lightness changes), one accent, and three words for
/// state. The page itself is the only thing that casts a shadow; everything else is told apart by
/// tone and a hairline, so the eye goes to the paper.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// What the page lies on: the preview's background. Deliberately the darkest (light theme) or
    /// the deepest (dark theme) surface, so a white page stands off it.
    pub desk: Color32,
    /// Toolbar, tray and side panel: one tone, so they read as one frame around the desk.
    pub panel: Color32,
    /// Dialogs and menus: one step above the panel.
    pub surface: Color32,
    /// Fields that receive text: a step *below* what is around them.
    pub inset: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    /// A hairline: findable, never loud.
    pub line: Color32,
    pub line_strong: Color32,
    pub text: Color32,
    /// Supporting text: labels, hints.
    pub weak: Color32,
    /// Disabled.
    pub faint: Color32,
    /// Fountain-pen blue: the primary action, the selection, the link. Nothing else.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
}

pub fn palette(dark: bool) -> Palette {
    let c = |r: u8, g: u8, b: u8| Color32::from_rgb(r, g, b);
    if dark {
        Palette {
            dark,
            desk: c(0x14, 0x13, 0x11),
            panel: c(0x1d, 0x1c, 0x19),
            surface: c(0x26, 0x25, 0x21),
            inset: c(0x16, 0x15, 0x13),
            hover: c(0x2d, 0x2b, 0x27),
            pressed: c(0x37, 0x35, 0x30),
            line: Color32::from_rgba_unmultiplied(255, 244, 222, 22),
            line_strong: Color32::from_rgba_unmultiplied(255, 244, 222, 46),
            text: c(0xec, 0xe8, 0xde),
            weak: c(0xa6, 0x9f, 0x90),
            faint: c(0x63, 0x5e, 0x54),
            accent: c(0x6f, 0x8b, 0xe8),
            accent_hover: c(0x85, 0x9e, 0xee),
            on_accent: c(0x0d, 0x12, 0x2c),
            ok: c(0x5b, 0xb3, 0x81),
            warn: c(0xd9, 0xa4, 0x41),
            bad: c(0xe5, 0x76, 0x6a),
        }
    } else {
        Palette {
            dark,
            desk: c(0xe3, 0xdf, 0xd4),
            panel: c(0xf5, 0xf2, 0xeb),
            surface: c(0xfc, 0xfa, 0xf6),
            inset: c(0xeb, 0xe7, 0xdc),
            hover: c(0xe9, 0xe5, 0xda),
            pressed: c(0xde, 0xd9, 0xcb),
            line: Color32::from_rgba_unmultiplied(60, 45, 10, 30),
            line_strong: Color32::from_rgba_unmultiplied(60, 45, 10, 62),
            text: c(0x25, 0x22, 0x1b),
            weak: c(0x62, 0x5c, 0x4f),
            faint: c(0xa9, 0xa1, 0x91),
            accent: c(0x2a, 0x49, 0x9c),
            accent_hover: c(0x37, 0x5a, 0xb4),
            on_accent: Color32::WHITE,
            ok: c(0x2f, 0x7d, 0x52),
            warn: c(0xa8, 0x6a, 0x10),
            bad: c(0xb8, 0x3a, 0x28),
        }
    }
}

impl Palette {
    pub fn of(ui: &egui::Ui) -> Palette {
        palette(ui.visuals().dark_mode)
    }
    pub fn of_ctx(ctx: &egui::Context) -> Palette {
        palette(ctx.global_style().visuals.dark_mode)
    }
}

/// `colour` at a given opacity (0-255), not darkened: the "on" state of a chip is the accent, washed.
pub fn tint(colour: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
}

/// A sheet of paper lying on the desk: a soft, short shadow under it. The only shadow in the app.
pub fn paper_shadow(painter: &egui::Painter, rect: egui::Rect, dark: bool) {
    let shadow = egui::Shadow {
        offset: [0, 2],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 150 } else { 52 }),
    };
    painter.add(shadow.as_shape(rect, 1.0));
    let contact = egui::Shadow {
        offset: [0, 1],
        blur: 2,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 120 } else { 34 }),
    };
    painter.add(contact.as_shape(rect, 1.0));
}

/// The look of the app: warm paper neutrals, one ink-blue accent, tight spacing on a 4-point grid,
/// and a radius scale of 5 (controls), 8 (rows, cards), 12 (dialogs).
fn style_one(style: &mut egui::Style) {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};
    let p = palette(style.visuals.dark_mode);
    let dark = p.dark;
    let v = &mut style.visuals;
    v.panel_fill = p.panel;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.inset;
    v.faint_bg_color = p.hover;
    v.code_bg_color = p.inset;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 150 } else { 46 }),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 140 } else { 40 }),
    };
    // "Selected" is a wash of the accent with accent-coloured text (as in a chip), not a solid slab:
    // the solid accent is kept for the one primary action.
    v.selection.bg_fill = tint(p.accent, if dark { 70 } else { 44 });
    v.selection.stroke = Stroke::new(1.0, if dark { p.accent_hover } else { p.accent });
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.bad;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.weak);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.weak);
    v.widgets.noninteractive.bg_fill = p.panel;
    for (w, fill) in [
        (&mut v.widgets.inactive, p.hover),
        (&mut v.widgets.hovered, p.pressed),
        (&mut v.widgets.active, p.pressed),
        (&mut v.widgets.open, p.pressed),
    ] {
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
        w.corner_radius = CornerRadius::same(5);
        w.fg_stroke = Stroke::new(1.0, p.text);
    }
    // Resting controls keep a hairline: without it an unchecked box or a flat button disappears into
    // the panel behind it.
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.line_strong);
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.striped = false;
    v.indent_has_left_vline = false;

    // A 4-point grid: 4 between related things, 8 between groups, 12/16 around blocks.
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
    style.interaction.tooltip_delay = 0.35;
    // Type scale 11.5 / 13 / 15 / 18 (about a minor third): weight is not available (one weight of the
    // font is bundled), so hierarchy comes from size and the colour ramp text / weak / faint.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    /// WCAG contrast ratio of two opaque colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `wash` laid over `ground`, as the eye sees a translucent fill.
    fn over(wash: Color32, ground: Color32) -> Color32 {
        let a = f32::from(wash.a()) / 255.0;
        let mix = |w: u8, g: u8| (f32::from(w) * a + f32::from(g) * (1.0 - a)).round() as u8;
        Color32::from_rgb(
            mix(wash.r(), ground.r()),
            mix(wash.g(), ground.g()),
            mix(wash.b(), ground.b()),
        )
    }

    #[test]
    fn text_is_readable_on_every_surface_in_both_themes() {
        for dark in [false, true] {
            let p = palette(dark);
            for (surface, name) in [
                (p.panel, "panel"),
                (p.surface, "surface"),
                (p.desk, "desk"),
                (p.inset, "inset"),
            ] {
                assert!(contrast(p.text, surface) >= 7.0, "text on {name} (dark={dark})");
                assert!(contrast(p.weak, surface) >= 4.5, "weak text on {name} (dark={dark})");
            }
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "the label of the primary button (dark={dark})"
            );
            assert!(
                contrast(p.accent, p.panel) >= 4.5,
                "accent as text or icon (dark={dark})"
            );
            for (status, name) in [(p.ok, "ok"), (p.warn, "warn"), (p.bad, "bad")] {
                assert!(contrast(status, p.surface) >= 3.0, "{name} on a dialog (dark={dark})");
                assert!(contrast(status, p.panel) >= 3.0, "{name} on the panel (dark={dark})");
            }
        }
    }

    #[test]
    fn a_chosen_chip_and_a_selected_row_keep_their_text_readable() {
        for dark in [false, true] {
            let p = palette(dark);
            // The washes used by chips (see icons::chip), selected rows (the global selection) and selected thumbnails.
            let chip = over(tint(p.accent, if dark { 56 } else { 34 }), p.surface);
            let text = if dark { p.accent_hover } else { p.accent };
            assert!(contrast(text, chip) >= 4.5, "chip text (dark={dark})");
            let row = over(tint(p.accent, if dark { 70 } else { 44 }), p.surface);
            assert!(contrast(text, row) >= 4.5, "selected row text (dark={dark})");
        }
    }

    #[test]
    fn the_page_stands_off_the_desk() {
        // A white page must read as an object on its surface, in both themes.
        for dark in [false, true] {
            let p = palette(dark);
            assert!(
                contrast(Color32::WHITE, p.desk) >= 1.25,
                "page against desk (dark={dark})"
            );
        }
    }
}
