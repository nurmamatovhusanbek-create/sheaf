//! UI-ABOUT and UI-WELCOME: the two windows that make the program feel finished.
//!
//! About says what this is, which version, whether its components are in order, where it keeps things
//! and where the licences are. Welcome is shown once, on a first start with no file to open, and
//! offers the two things that are not obvious: the action wheel and the right-click menu.

use crate::app::{Msg, SheafApp};
use crate::icons::{paint, Icon};
use eframe::egui::{self, vec2, RichText};
use std::path::PathBuf;

/// Marker in the settings folder: the welcome window has been seen.
const SEEN: &str = "welcome-seen";

/// "ctrl+alt+space" as people write it: "Ctrl+Alt+Space".
pub fn pretty_keys(keys: &str) -> String {
    keys.split('+')
        .filter(|k| !k.is_empty())
        .map(|k| {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Should the welcome window open now?
pub fn welcome_due(config_dir: Option<&std::path::Path>, opening_files: bool) -> bool {
    !opening_files && config_dir.is_some_and(|d| !d.join(SEEN).exists())
}

pub fn mark_welcome_seen(config_dir: Option<&std::path::Path>) {
    if let Some(dir) = config_dir {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(SEEN), b"");
    }
}

/// The licence file the installer puts next to the program (a one-file build unpacks it with its engines).
pub fn licence_file() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.join("THIRD_PARTY_LICENSES");
    let unpacked = sheaf_app::vendor_dir_override().map(|dir| dir.join("THIRD_PARTY_LICENSES"));
    std::iter::once(beside).chain(unpacked).find(|f| f.is_file())
}

/// What a helper thread tells the window.
pub enum Say {
    WheelStarted(bool),
    MenuAdded(bool),
}

impl SheafApp {
    pub(crate) fn open_about(&mut self) {
        self.modal = Some(crate::app::Modal::About);
    }

    pub(crate) fn on_say(&mut self, say: Say) {
        let (ok, key) = match say {
            Say::WheelStarted(ok) => (ok, "welcome.wheel_started"),
            Say::MenuAdded(ok) => (ok, "welcome.menu_added"),
        };
        if ok {
            let text = self.t.t(key);
            self.success(text, "");
        } else {
            let text = self.t.t("error.internal");
            self.toast(text, "", true);
        }
    }

    /// Run a copy of this program with `args` off the UI thread and report how it went.
    fn run_self(&self, args: &'static [&'static str], wait: bool, make: fn(bool) -> Say) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let ok = crate::platform::run_self(args, wait);
            let _ = tx.send(Msg::Say(make(ok)));
            ctx.request_repaint();
        });
    }

    pub(crate) fn about_dialog(&mut self, ctx: &egui::Context) -> bool {
        let (mut close, mut licences, mut help, mut report) = (false, false, false, false);
        let licence_present = licence_file().is_some();
        let verified = match self.engines.verified {
            Some(true) => self.t.t("settings.verified"),
            Some(false) => self.t.t("settings.verify_failed"),
            None => self.t.t("settings.verifying"),
        };
        let places = [
            (
                self.t.t("about.settings"),
                self.config_dir.as_ref().map(|d| d.display().to_string()),
            ),
            (self.t.t("about.scratch"), Some(self.base_dir.display().to_string())),
        ];
        crate::theme::modal(ctx, "about").show(ctx, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                paint(ui.painter(), r, Icon::Presets, ui.visuals().selection.bg_fill);
                ui.vertical(|ui| {
                    ui.heading(self.t.t("about.title"));
                    ui.label(RichText::new(self.t.tv("about.version", &[("version", &self.version)])).weak());
                });
            });
            ui.add_space(6.0);
            ui.label(self.t.t("about.tagline"));
            ui.add_space(10.0);
            ui.label(RichText::new(self.t.t("about.components")).strong());
            ui.label(verified);
            ui.label(if self.engines.tesseract {
                self.t.t("settings.tesseract_ok")
            } else {
                self.t.t("settings.tesseract_missing")
            });
            ui.label(if self.engines.font {
                self.t.t("settings.font_ok")
            } else {
                self.t.t("settings.font_missing")
            });
            ui.add_space(10.0);
            ui.label(RichText::new(self.t.t("about.places")).strong());
            for (what, where_) in &places {
                if let Some(p) = where_ {
                    ui.label(RichText::new(what).small().weak());
                    ui.label(RichText::new(p).monospace().small());
                }
            }
            ui.add_space(10.0);
            ui.label(RichText::new(self.t.t("about.licences_hint")).small().weak());
            if !licence_present {
                ui.label(
                    RichText::new(self.t.t("about.licences_missing"))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(licence_present, |ui| {
                    if ui.button(self.t.t("about.licences")).clicked() {
                        licences = true;
                    }
                });
                if ui.button(self.t.t("about.help")).clicked() {
                    help = true;
                }
                if ui.button(self.t.t("report.open")).clicked() {
                    report = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(self.t.t("common.close")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if licences {
            if let Some(file) = licence_file() {
                let _ = crate::platform::open_text_file(&file);
            }
        }
        if help {
            self.open_help("start");
            return false;
        }
        if report {
            self.open_report(None);
            return false;
        }
        !(close || crate::dialogs::ui_escape(ctx))
    }

    pub(crate) fn welcome_dialog(&mut self, ctx: &egui::Context) -> bool {
        let (mut done, mut wheel, mut menu, mut help) = (false, false, false, false);
        let keys = pretty_keys(&self.settings.wheel_hotkey);
        let has_wheel = !keys.is_empty();
        crate::theme::modal(ctx, "welcome").show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(self.t.t("welcome.title"));
            ui.add_space(4.0);
            ui.label(self.t.t("welcome.intro"));
            ui.add_space(10.0);
            card(
                ui,
                Icon::Open,
                &self.t.t("welcome.open_title"),
                &self.t.t("welcome.open_body"),
                None,
            );
            if has_wheel {
                let body = self.t.tv("welcome.wheel_body", &[("keys", &keys)]);
                if card(
                    ui,
                    Icon::Merge,
                    &self.t.t("welcome.wheel_title"),
                    &body,
                    Some(&self.t.t("welcome.wheel_start")),
                ) {
                    wheel = true;
                }
            }
            if card(
                ui,
                Icon::Presets,
                &self.t.t("welcome.menu_title"),
                &self.t.t("welcome.menu_body"),
                Some(&self.t.t("welcome.menu_add")),
            ) {
                menu = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(self.t.t("welcome.help")).clicked() {
                    help = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(self.t.t("welcome.start")).clicked() {
                        done = true;
                    }
                });
            });
        });
        if wheel {
            self.run_self(&["tray"], false, Say::WheelStarted);
        }
        if menu {
            self.run_self(&["install-verbs"], true, Say::MenuAdded);
        }
        if help {
            self.open_help("start");
            done = true;
        }
        if done || crate::dialogs::ui_escape(ctx) {
            mark_welcome_seen(self.config_dir.as_deref());
            return false;
        }
        true
    }
}

/// An icon, a title, a line, and optionally a button; true when the button was clicked.
fn card(ui: &mut egui::Ui, icon: Icon, title: &str, body: &str, button: Option<&str>) -> bool {
    let mut clicked = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), egui::Sense::hover());
            paint(ui.painter(), r, icon, ui.visuals().selection.bg_fill);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(body);
                if let Some(b) = button {
                    if ui.button(b).clicked() {
                        clicked = true;
                    }
                }
            });
        });
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_are_shown_the_way_people_write_them() {
        assert_eq!(pretty_keys("ctrl+alt+space"), "Ctrl+Alt+Space");
        assert_eq!(pretty_keys("ctrl+shift+f9"), "Ctrl+Shift+F9");
        assert_eq!(pretty_keys(""), "");
    }

    #[test]
    fn the_welcome_window_shows_once_and_not_when_files_are_being_opened() {
        let dir = std::env::temp_dir().join(format!("sheaf-welcome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(welcome_due(Some(&dir), false), "first start");
        assert!(!welcome_due(Some(&dir), true), "a file was passed on the command line");
        assert!(
            !welcome_due(None, false),
            "nowhere to remember it, so it would show every time"
        );
        mark_welcome_seen(Some(&dir));
        assert!(!welcome_due(Some(&dir), false), "seen");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
