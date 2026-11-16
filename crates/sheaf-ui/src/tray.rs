//! UI-TRAY: the page list. Only the rows in view are drawn (PERF-LAZY).
//! Selection by click, Shift and Ctrl (and Ctrl+A); reorder by dragging with
//! the pointer; a context menu for the common edits.

use crate::app::SheafApp;
use crate::textures::Pic;
use eframe::egui::{self, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
use sheaf_app::api::{PageDto, UiOp};
use std::collections::HashSet;

pub const ROW_H: f32 = 142.0;
const THUMB_PX: u32 = 256;

#[derive(Default)]
pub struct TrayState {
    pub drag: Option<Drag>,
    /// Locked files the person chose not to unlock now; not asked again this run.
    pub skipped_locked: HashSet<u32>,
    pub scroll_to: Option<usize>,
    scroll_y: f32,
}

pub struct Drag {
    pub ids: Vec<u64>,
    pub slot: usize,
}

/// Clicks with modifiers: plain selects one, Ctrl toggles, Shift extends from the anchor.
fn click_select(app: &mut SheafApp, index: usize, shift: bool, ctrl: bool) {
    let Some(page) = app.pages.get(index) else { return };
    let id = page.id;
    if shift && app.sel.anchor.is_some() {
        let a = app.sel.anchor.unwrap_or(index);
        let (lo, hi) = (a.min(index), a.max(index));
        let range: HashSet<u64> = app.pages[lo..=hi].iter().map(|p| p.id).collect();
        app.sel.ids = if ctrl {
            app.sel.ids.union(&range).copied().collect()
        } else {
            range
        };
        app.sel.focus = Some(index);
    } else if ctrl {
        if !app.sel.ids.remove(&id) {
            app.sel.ids.insert(id);
        }
        app.sel.anchor = Some(index);
        app.sel.focus = Some(index);
    } else {
        app.select_only(index);
    }
}

fn badges(app: &SheafApp, page: &PageDto, p: &crate::theme::Palette) -> Vec<(String, Color32)> {
    let t = &app.t;
    let (ok, warn, bad, plain) = (p.ok, p.warn, p.bad, p.weak);
    let mut out = Vec::new();
    match page.ocr.state {
        "queued" => out.push((t.t("badge.ocr_queued"), plain)),
        "running" => out.push((t.t("badge.ocr_running"), plain)),
        "done" => out.push((t.t("badge.ocr_done"), ok)),
        "failed" => out.push((t.t("badge.ocr_failed"), bad)),
        _ => {}
    }
    // A text layer is the normal case and says nothing. A page WITHOUT one is the thing worth a badge:
    // it is what recognition can fix.
    if page.has_text == Some(false)
        && page.ocr.state != "done"
        && page.ocr.state != "running"
        && page.ocr.state != "queued"
    {
        out.push((t.t("badge.no_text"), warn));
    }
    if page.crop.is_some() {
        out.push((t.t("badge.crop"), plain));
    }
    let f = &page.filter;
    if f.mode != sheaf_core::ColorMode::Color || f.deskew != sheaf_core::Deskew::Off || f.despeckle || f.contrast != 0 {
        out.push((t.t("badge.filter"), plain));
    }
    out
}

impl SheafApp {
    pub(crate) fn tray_ui(&mut self, ui: &mut egui::Ui) {
        self.tex.new_frame();
        let total = self.pages.len();
        let mut head = self.t.tn("tray.count", total as i64, &[]);
        if self.sel.ids.len() > 1 {
            head = format!(
                "{head} · {}",
                self.t.tn("tray.selected", self.sel.ids.len() as i64, &[])
            );
        }
        ui.label(
            egui::RichText::new(if total == 0 { String::new() } else { head })
                .small()
                .weak(),
        );
        ui.add_space(2.0);
        if total == 0 {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(egui::RichText::new(self.t.t("tray.empty_title")).strong());
                ui.label(egui::RichText::new(self.t.t("tray.empty_hint")).weak());
                ui.add_space(8.0);
                if ui.button(self.t.t("toolbar.open")).clicked() {
                    self.open_dialog();
                }
            });
            return;
        }
        self.tray_keys(ui.ctx());
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(index) = self.tray.scroll_to.take() {
            area = area.vertical_scroll_offset((index as f32 * (ROW_H + ui.spacing().item_spacing.y) - 40.0).max(0.0));
        } else if self.tray.drag.is_some() {
            area = area.vertical_scroll_offset(self.tray.scroll_y);
        }
        let output = area.show_rows(ui, ROW_H, total, |ui, range| {
            for i in range {
                self.tray_row(ui, i);
            }
        });
        self.tray.scroll_y = output.state.clamped_offset().y;
        self.tray_drag(ui, output.inner_rect, output.state.clamped_offset().y);
    }

    fn tray_row(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(page) = self.pages.get(index).cloned() else {
            return;
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
        let selected = self.sel.ids.contains(&page.id);
        let visuals = ui.visuals().clone();
        let pal = crate::theme::Palette::of(ui);
        let painter = ui.painter_at(rect);
        if selected {
            // A wash of the accent and a ring: quiet enough that eight selected pages are not a wall of blue.
            painter.rect_filled(
                rect,
                7.0,
                crate::theme::tint(pal.accent, if pal.dark { 46 } else { 30 }),
            );
            painter.rect_stroke(rect, 7.0, Stroke::new(1.5, pal.accent), StrokeKind::Inside);
        } else if resp.hovered() {
            painter.rect_filled(rect, 7.0, pal.hover);
        }
        if self.sel.focus == Some(index) && !selected {
            painter.rect_stroke(
                rect.shrink(1.0),
                6.0,
                Stroke::new(1.0, pal.line_strong),
                StrokeKind::Inside,
            );
        }
        let thumb = Rect::from_min_max(rect.min + vec2(14.0, 8.0), rect.max - vec2(14.0, 30.0));
        match self.picture(page.id, &page.ver, THUMB_PX) {
            Pic::Ready(id, [w, h]) | Pic::Stale(id, [w, h]) => {
                let scale = (thumb.width() / w as f32).min(thumb.height() / h as f32);
                let img = Rect::from_center_size(thumb.center(), vec2(w as f32 * scale, h as f32 * scale));
                crate::theme::paper_shadow(&painter, img, visuals.dark_mode);
                painter.image(
                    id,
                    img,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.rect_stroke(img, 0.0, Stroke::new(1.0, pal.line), StrokeKind::Outside);
            }
            Pic::Loading => {
                painter.rect_filled(
                    Rect::from_center_size(thumb.center(), vec2(thumb.width() * 0.45, thumb.height() * 0.7)),
                    2.0,
                    visuals.faint_bg_color,
                );
            }
            Pic::Failed => {
                painter.text(
                    thumb.center(),
                    Align2::CENTER_CENTER,
                    "!",
                    FontId::proportional(22.0),
                    visuals.error_fg_color,
                );
            }
        }
        let mut x = thumb.left() + 4.0;
        for (text, color) in badges(self, &page, &pal) {
            let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), Color32::WHITE);
            let r = Rect::from_min_size(
                egui::pos2(x, thumb.bottom() - galley.size().y - 8.0),
                galley.size() + vec2(8.0, 4.0),
            );
            painter.rect_filled(r, 3.0, color);
            painter.galley(r.min + vec2(4.0, 2.0), galley, Color32::WHITE);
            x = r.right() + 3.0;
        }
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 14.0),
            Align2::CENTER_CENTER,
            self.t.tv("tray.page", &[("n", &(index + 1).to_string())]),
            FontId::proportional(12.0),
            if selected { pal.text } else { pal.weak },
        );

        let m = ui.input(|i| i.modifiers);
        if resp.clicked() {
            click_select(self, index, m.shift, m.command);
        }
        if resp.drag_started() {
            if !selected {
                self.select_only(index);
            }
            let ids: Vec<u64> = self
                .pages
                .iter()
                .filter(|p| self.sel.ids.contains(&p.id))
                .map(|p| p.id)
                .collect();
            self.tray.drag = Some(Drag { ids, slot: index });
        }
        if resp.secondary_clicked() && !selected {
            self.select_only(index);
        }
        resp.context_menu(|ui| self.tray_menu(ui));
    }

    fn tray_menu(&mut self, ui: &mut egui::Ui) {
        let n = self.targets().len() as i64;
        let t = &self.t;
        let (cw, ccw, dup, blank, ocr, extract, del) = (
            t.t("menu.rotate_cw"),
            t.t("menu.rotate_ccw"),
            t.t("menu.duplicate"),
            t.t("menu.insert_blank"),
            t.tn("menu.ocr", n, &[]),
            t.tn("menu.extract", n, &[]),
            t.tn("menu.delete", n, &[]),
        );
        if ui.button(format!("{cw}   R")).clicked() {
            self.rotate(90);
            ui.close();
        }
        if ui.button(format!("{ccw}   Shift+R")).clicked() {
            self.rotate(-90);
            ui.close();
        }
        if ui.button(format!("{dup}   Ctrl+D")).clicked() {
            let ids = self.targets();
            self.command(move |b| b.apply(UiOp::Duplicate { ids }).map(|_| ()));
            ui.close();
        }
        if ui.button(blank).clicked() {
            let at = self
                .targets()
                .iter()
                .filter_map(|id| self.pages.iter().position(|p| p.id == *id))
                .max()
                .map_or(self.pages.len(), |i| i + 1);
            self.command(move |b| b.apply(UiOp::InsertBlank { at, size: None }).map(|_| ()));
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{ocr}   F")).clicked() {
            self.run_ocr_selection();
            ui.close();
        }
        if ui.button(extract).clicked() {
            self.open_export(true);
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{del}   Del")).clicked() {
            self.delete_selected();
            ui.close();
        }
    }

    /// While a drag is on: the insertion line, edge scrolling, and the drop.
    fn tray_drag(&mut self, ui: &mut egui::Ui, view: Rect, offset: f32) {
        let Some(drag) = &self.tray.drag else { return };
        let pos = ui.input(|i| i.pointer.latest_pos());
        let released = ui.input(|i| i.pointer.any_released());
        let Some(pos) = pos else { return };
        let row = ROW_H + ui.spacing().item_spacing.y;
        let total = self.pages.len();
        let slot = (((pos.y - view.top() + offset) / row).round().max(0.0) as usize).min(total);
        if released {
            let ids = drag.ids.clone();
            self.tray.drag = None;
            // The target index counts only the pages that stay: those before the slot that are not moved.
            let moved: HashSet<u64> = ids.iter().copied().collect();
            let to = self.pages[..slot.min(total)]
                .iter()
                .filter(|p| !moved.contains(&p.id))
                .count();
            if !ids.is_empty() {
                self.command(move |b| b.apply(UiOp::Move { ids, to }).map(|_| ()));
            }
            return;
        }
        if let Some(d) = &mut self.tray.drag {
            d.slot = slot;
        }
        let y = view.top() + slot as f32 * row - offset;
        if y >= view.top() - 2.0 && y <= view.bottom() + 2.0 {
            ui.painter().hline(
                view.x_range().shrink(8.0),
                y,
                Stroke::new(3.0, crate::theme::Palette::of(ui).accent),
            );
        }
        let speed = if pos.y < view.top() + 36.0 {
            -14.0
        } else if pos.y > view.bottom() - 36.0 {
            14.0
        } else {
            0.0
        };
        if speed != 0.0 {
            self.tray.scroll_y = (self.tray.scroll_y + speed).max(0.0);
            ui.ctx().request_repaint();
        }
        ui.ctx().request_repaint();
    }

    /// Arrow keys, Home/End, Page keys, Ctrl+A and Escape on the page list.
    pub(crate) fn tray_keys(&mut self, ctx: &egui::Context) {
        if self.modal.is_some() || ctx.egui_wants_keyboard_input() || self.pages.is_empty() {
            return;
        }
        let total = self.pages.len();
        let page = 3usize;
        let (mut to, shift): (Option<usize>, bool) = ctx.input(|i| {
            let from = self.sel.focus.unwrap_or(0);
            let first = self.sel.focus.is_none();
            let key = |k| i.key_pressed(k);
            let to = if key(egui::Key::ArrowDown) {
                Some(if first { 0 } else { from + 1 })
            } else if key(egui::Key::ArrowUp) {
                Some(if first { 0 } else { from.saturating_sub(1) })
            } else if key(egui::Key::PageDown) {
                Some(from + page)
            } else if key(egui::Key::PageUp) {
                Some(from.saturating_sub(page))
            } else if key(egui::Key::Home) {
                Some(0)
            } else if key(egui::Key::End) {
                Some(total - 1)
            } else {
                None
            };
            (to, i.modifiers.shift)
        });
        if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::A))) {
            self.sel.ids = self.pages.iter().map(|p| p.id).collect();
            self.sel.anchor.get_or_insert(0);
            self.sel.focus.get_or_insert(0);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sel.ids.clear();
        }
        if let Some(i) = to.take() {
            let i = i.min(total - 1);
            click_select(self, i, shift, false);
            self.tray.scroll_to = Some(i);
        }
    }
}
