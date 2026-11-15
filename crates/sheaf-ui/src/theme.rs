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
    ctx.all_styles_mut(|style| {
        style.visuals.selection.bg_fill = if style.visuals.dark_mode {
            Color32::from_rgb(0x4a, 0x7d, 0xe8)
        } else {
            Color32::from_rgb(0x1f, 0x5f, 0xd6)
        };
        style.visuals.selection.stroke.color = Color32::WHITE;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.interaction.tooltip_delay = 0.4;
    });
}
