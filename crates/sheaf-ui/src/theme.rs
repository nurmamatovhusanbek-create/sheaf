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

/// `system`, `light` or `dark`.
pub fn apply(ctx: &egui::Context, theme: &str) {
    ctx.set_theme(match theme {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
    ctx.all_styles_mut(style_one);
}

/// The look of the app: calm neutrals, one blue accent, generous rounding and spacing.
fn style_one(style: &mut egui::Style) {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};
    let dark = style.visuals.dark_mode;
    let c = |r: u8, g: u8, b: u8| Color32::from_rgb(r, g, b);
    let (panel, surface, raised, hover, active, border, text, weak, accent) = if dark {
        (
            c(0x16, 0x19, 0x21),
            c(0x1c, 0x20, 0x2a),
            c(0x25, 0x2a, 0x37),
            c(0x2c, 0x32, 0x41),
            c(0x35, 0x3c, 0x4e),
            c(0x2d, 0x33, 0x42),
            c(0xe7, 0xea, 0xf2),
            c(0x8d, 0x96, 0xab),
            c(0x5b, 0x8d, 0xef),
        )
    } else {
        (
            c(0xf4, 0xf5, 0xf8),
            c(0xff, 0xff, 0xff),
            c(0xec, 0xee, 0xf3),
            c(0xe3, 0xe7, 0xef),
            c(0xd5, 0xdb, 0xe7),
            c(0xdf, 0xe2, 0xea),
            c(0x1b, 0x20, 0x30),
            c(0x6a, 0x72, 0x86),
            c(0x25, 0x63, 0xeb),
        )
    };
    let v = &mut style.visuals;
    v.panel_fill = panel;
    v.window_fill = surface;
    v.extreme_bg_color = if dark { c(0x12, 0x14, 0x1b) } else { c(0xff, 0xff, 0xff) };
    v.faint_bg_color = raised;
    v.code_bg_color = raised;
    v.window_stroke = Stroke::new(1.0, border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow.color = Color32::from_black_alpha(if dark { 120 } else { 45 });
    v.popup_shadow.color = Color32::from_black_alpha(if dark { 120 } else { 45 });
    v.selection.bg_fill = accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = accent;
    v.override_text_color = Some(text);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, weak);
    for (w, fill) in [
        (&mut v.widgets.inactive, raised),
        (&mut v.widgets.hovered, hover),
        (&mut v.widgets.active, active),
        (&mut v.widgets.open, hover),
    ] {
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
        w.corner_radius = CornerRadius::same(7);
        w.fg_stroke = Stroke::new(1.0, text);
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, border);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.striped = false;
    v.indent_has_left_vline = false;

    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
    style.interaction.tooltip_delay = 0.35;
    style.text_styles = [
        (TextStyle::Heading, FontId::new(19.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
}
