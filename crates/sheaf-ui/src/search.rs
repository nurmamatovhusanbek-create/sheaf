//! UI-SEARCH: full text over recognized pages (CMD-SEARCH). A result jumps to its page.

use crate::app::SheafApp;
use eframe::egui;
use sheaf_app::api::{ErrorDto, HitDto};

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub hits: Option<Vec<HitDto>>,
    pub focus_next: bool,
    asked: String,
}

impl SearchState {
    pub fn on_result(&mut self, r: Result<Vec<HitDto>, ErrorDto>) {
        self.hits = r.ok();
    }
}

impl SheafApp {
    pub(crate) fn search_box(&mut self, ui: &mut egui::Ui) {
        let hint = self.t.t("search.placeholder");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.search.query)
                .hint_text(hint)
                .desired_width(200.0),
        );
        if std::mem::take(&mut self.search.focus_next) {
            resp.request_focus();
        }
        let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = resp.changed();
        if (submit || changed) && self.search.query.trim() != self.search.asked {
            self.search.asked = self.search.query.trim().to_string();
            if self.search.asked.is_empty() {
                self.search.hits = None;
            } else {
                let q = self.search.asked.clone();
                self.spawn(move |b| crate::app::Msg::Search(b.search(&q).map_err(|e| ErrorDto::from(&e))));
            }
        }
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.query.clear();
            self.search.asked.clear();
            self.search.hits = None;
        }
        let Some(hits) = self.search.hits.clone() else { return };
        if !resp.has_focus() && !ui.ctx().is_pointer_over_egui() && self.search.query.is_empty() {
            return;
        }
        let mut jump = None;
        egui::Area::new(egui::Id::unique("search-results"))
            .order(egui::Order::Foreground)
            .fixed_pos(resp.rect.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(320.0);
                    if hits.is_empty() {
                        ui.label(egui::RichText::new(self.t.t("search.none")).weak());
                    }
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for hit in hits.iter().take(50) {
                            let at = self.pages.iter().position(|p| p.id == hit.page);
                            let label = format!(
                                "{}  {}",
                                at.map_or("…".into(), |i| self
                                    .t
                                    .tv("search.page", &[("n", &(i + 1).to_string())])),
                                hit.snippet
                            );
                            if ui.selectable_label(false, label).clicked() {
                                jump = at;
                            }
                        }
                    });
                });
            });
        if let Some(i) = jump {
            self.select_only(i);
            self.tray.scroll_to = Some(i);
            self.search.hits = None;
        }
    }
}
