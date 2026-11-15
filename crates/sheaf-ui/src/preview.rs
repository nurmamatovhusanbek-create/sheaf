//! UI-PREVIEW: the current page at a chosen zoom, with rotate, filters, crop
//! handles and the recognized-text overlay. The picture is rendered by a
//! worker; the page is never parsed here.

use crate::app::SheafApp;
use crate::geometry::{conf_class, display_to_page, drag_rect, normalize, word_box, Conf, Handle, Rect01};
use crate::textures::Pic;
use eframe::egui::{self, vec2, Color32, Rect, Sense, Stroke, StrokeKind, TextureId};
use sheaf_app::api::{ErrorDto, OcrResultDto, UiOp};
use sheaf_core::{ColorMode, Deskew, NormRect};

const ZOOMS: [f32; 8] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];
const DEFAULT_CROP: Rect01 = Rect01 {
    x0: 0.05,
    y0: 0.05,
    x1: 0.95,
    y1: 0.95,
};

pub struct PreviewState {
    pub zoom: f32,
    pub crop: Option<Rect01>,
    crop_drag: Option<(Handle, Rect01, egui::Pos2)>,
    pub show_text: bool,
    pub words: Option<(u64, OcrResultDto)>,
    asked_words_for: Option<(u64, &'static str)>,
    pub doubtful: Option<usize>,
    /// The last picture shown, kept while a sharper one loads so the page never flickers.
    last: Option<(u64, TextureId, [usize; 2])>,
}

impl Default for PreviewState {
    fn default() -> Self {
        PreviewState {
            zoom: 1.0,
            crop: None,
            crop_drag: None,
            show_text: false,
            words: None,
            asked_words_for: None,
            doubtful: None,
            last: None,
        }
    }
}

impl PreviewState {
    pub fn on_ocr_result(&mut self, page: u64, result: Result<OcrResultDto, ErrorDto>) {
        if let Ok(r) = result {
            self.words = Some((page, r));
        }
    }

    pub fn doubtful_indexes(&self) -> Vec<usize> {
        self.words
            .as_ref()
            .map(|(_, r)| {
                r.words
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.conf < 70.0)
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Jump to the next word with doubtful confidence (OCR-CONF-UI).
    pub fn next_doubtful(&mut self) {
        let list = self.doubtful_indexes();
        if list.is_empty() {
            return;
        }
        let next = match self.doubtful {
            Some(cur) => list.iter().copied().find(|i| *i > cur).unwrap_or(list[0]),
            None => list[0],
        };
        self.doubtful = Some(next);
        self.show_text = true;
    }

    fn step_zoom(&mut self, dir: i32) {
        let at = ZOOMS
            .iter()
            .position(|z| *z >= self.zoom - 1e-4)
            .unwrap_or(ZOOMS.len() - 1) as i32;
        self.zoom = ZOOMS[(at + dir).clamp(0, ZOOMS.len() as i32 - 1) as usize];
    }
}

impl SheafApp {
    /// Fetch the recognized words of the current page once its OCR is done.
    fn ensure_words(&mut self) {
        let Some(p) = self.current_page() else { return };
        let (id, state) = (p.id, p.ocr.state);
        if state != "done" {
            if self.preview.words.as_ref().is_some_and(|(w, _)| *w == id) {
                self.preview.words = None;
            }
            self.preview.asked_words_for = None;
            return;
        }
        let have = self.preview.words.as_ref().is_some_and(|(w, _)| *w == id);
        if have || self.preview.asked_words_for == Some((id, state)) {
            return;
        }
        self.preview.asked_words_for = Some((id, state));
        self.spawn(move |b| crate::app::Msg::OcrResult {
            page: id,
            result: b.ocr_result(id).map_err(|e| ErrorDto::from(&e)),
        });
    }

    pub(crate) fn preview_ui(&mut self, ui: &mut egui::Ui) {
        if self.pages.is_empty() {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new(self.t.t("preview.empty")).weak()));
            return;
        }
        let Some(page) = self.current_page().cloned() else {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new(self.t.t("preview.none_selected")).weak()));
            return;
        };
        self.ensure_words();
        if self.preview.last.is_some_and(|(id, _, _)| id != page.id) {
            self.preview.crop = None;
            self.preview.doubtful = None;
        }
        self.preview_bar(ui, &page);
        ui.separator();

        let avail = ui.available_size();
        let ppp = ui.ctx().pixels_per_point();
        let zoom = self.preview.zoom;
        let longest = (avail.x.max(avail.y) * ppp * zoom).clamp(300.0, 4096.0);
        // Round to steps so a resize does not render a new picture for every pixel.
        let px = ((longest / 256.0).ceil() as u32 * 256).clamp(512, 4096);
        let pic = self.picture(page.id, &page.ver, px);
        let shown = match pic {
            Pic::Ready(id, size) => {
                self.preview.last = Some((page.id, id, size));
                Some((id, size))
            }
            _ => self
                .preview
                .last
                .filter(|(p, _, _)| *p == page.id)
                .map(|(_, id, size)| (id, size)),
        };
        let Some((tex, [w, h])) = shown else {
            ui.centered_and_justified(|ui| {
                if matches!(pic, Pic::Failed) {
                    ui.label(egui::RichText::new(self.t.t("tray.thumb_failed")).color(ui.visuals().error_fg_color));
                } else {
                    ui.spinner();
                }
            });
            return;
        };
        let aspect = w as f32 / h as f32;
        let pad = 16.0;
        let (aw, ah) = ((avail.x - 2.0 * pad).max(80.0), (avail.y - 2.0 * pad).max(80.0));
        let fit_w = if aspect >= aw / ah { aw } else { ah * aspect };
        let size = vec2(fit_w * zoom, fit_w * zoom / aspect);
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let outer = vec2(size.x + 2.0 * pad, size.y + 2.0 * pad).max(ui.available_size());
            let (area, _) = ui.allocate_exact_size(outer, Sense::hover());
            let rect = Rect::from_center_size(area.center(), size);
            let painter = ui.painter_at(area);
            painter.rect_filled(
                rect.expand(1.0),
                0.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            );
            painter.image(
                tex,
                rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            self.draw_words(&painter, rect);
            self.crop_overlay(ui, rect, &page);
        });
    }

    fn preview_bar(&mut self, ui: &mut egui::Ui, page: &sheaf_app::api::PageDto) {
        let at = self.pages.iter().position(|p| p.id == page.id).unwrap_or(0);
        let total = self.pages.len();
        let t = &self.t;
        let (cw, ccw, zin, zout, fit) = (
            t.t("menu.rotate_cw"),
            t.t("menu.rotate_ccw"),
            t.t("preview.zoom_in"),
            t.t("preview.zoom_out"),
            t.t("preview.fit"),
        );
        let label = t.tv(
            "preview.page_of",
            &[("n", &(at + 1).to_string()), ("total", &total.to_string())],
        );
        let crop_label = if page.crop.is_some() && self.preview.crop.is_none() {
            t.t("preview.crop_reset")
        } else {
            t.t("preview.crop")
        };
        let text_label = t.t("preview.text");
        let text_hint = if page.ocr.state == "done" {
            t.t("preview.text_title")
        } else {
            t.t("preview.text_needs_ocr")
        };
        let modes = [
            ("Color", t.t("filter.Color")),
            ("Gray", t.t("filter.Gray")),
            ("BW", t.t("filter.BW")),
        ];
        let (deskew_l, clean_l) = (t.t("filter.deskew"), t.t("filter.despeckle"));
        let f = page.filter;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(at > 0, egui::Button::new("<"))
                .on_hover_text(self.t.t("preview.prev"))
                .clicked()
            {
                self.select_only(at - 1);
                self.tray.scroll_to = Some(at - 1);
            }
            ui.label(label);
            if ui
                .add_enabled(at + 1 < total, egui::Button::new(">"))
                .on_hover_text(self.t.t("preview.next"))
                .clicked()
            {
                self.select_only(at + 1);
                self.tray.scroll_to = Some(at + 1);
            }
            ui.separator();
            if ui.button("-90°").on_hover_text(ccw).clicked() {
                self.rotate_pages(vec![page.id], -90);
            }
            if ui.button("+90°").on_hover_text(cw).clicked() {
                self.rotate_pages(vec![page.id], 90);
            }
            ui.separator();
            if ui.button("-").on_hover_text(zout).clicked() {
                self.preview.step_zoom(-1);
            }
            ui.label(format!("{}%", (self.preview.zoom * 100.0).round() as i32));
            if ui.button("+").on_hover_text(zin).clicked() {
                self.preview.step_zoom(1);
            }
            if ui.button(self.t.t("preview.fit")).on_hover_text(fit).clicked() {
                self.preview.zoom = 1.0;
            }
            ui.separator();
            for (id, name) in modes {
                let mode = match id {
                    "Gray" => ColorMode::Gray,
                    "BW" => ColorMode::BW,
                    _ => ColorMode::Color,
                };
                if ui.selectable_label(f.mode == mode, name).clicked() && f.mode != mode {
                    self.set_filter(sheaf_core::Filter { mode, ..f });
                }
            }
            if ui.selectable_label(f.deskew != Deskew::Off, deskew_l).clicked() {
                self.set_filter(sheaf_core::Filter {
                    deskew: if f.deskew == Deskew::Off {
                        Deskew::Auto
                    } else {
                        Deskew::Off
                    },
                    ..f
                });
            }
            if ui.selectable_label(f.despeckle, clean_l).clicked() {
                self.set_filter(sheaf_core::Filter {
                    despeckle: !f.despeckle,
                    ..f
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = page.ocr.state == "done";
                if ui
                    .add_enabled(enabled, egui::Button::new(text_label).selected(self.preview.show_text))
                    .on_hover_text(text_hint)
                    .clicked()
                {
                    self.preview.show_text = !self.preview.show_text;
                }
                if ui
                    .add(egui::Button::new(crop_label.clone()).selected(self.preview.crop.is_some()))
                    .clicked()
                {
                    if page.crop.is_some() {
                        let id = page.id;
                        self.command(move |b| {
                            b.apply(UiOp::Crop {
                                ids: vec![id],
                                rect: None,
                            })
                            .map(|_| ())
                        });
                    } else if self.preview.crop.is_some() {
                        self.preview.crop = None;
                    } else {
                        self.preview.crop = Some(DEFAULT_CROP);
                    }
                }
            });
        });
    }

    fn rotate_pages(&mut self, ids: Vec<u64>, deg: i32) {
        self.command(move |b| b.apply(UiOp::Rotate { ids, deg }).map(|_| ()));
    }

    fn set_filter(&mut self, filter: sheaf_core::Filter) {
        let ids = self.targets();
        if !ids.is_empty() {
            self.command(move |b| b.apply(UiOp::SetFilter { ids, filter }).map(|_| ()));
        }
    }

    /// Recognized words over the page: doubtful ones marked by outline and dash, not color alone.
    fn draw_words(&self, painter: &egui::Painter, rect: Rect) {
        let Some((_, result)) = self.preview.words.as_ref().filter(|_| self.preview.show_text) else {
            return;
        };
        let accent = painter.ctx().global_style().visuals.selection.bg_fill;
        for (i, w) in result.words.iter().enumerate() {
            let current = self.preview.doubtful == Some(i);
            let class = conf_class(w.conf);
            if class == Conf::Ok && !current {
                continue;
            }
            let Some(b) = word_box(w.bbox, result.image_px) else {
                continue;
            };
            let r = Rect::from_min_max(
                egui::pos2(rect.left() + b.x0 * rect.width(), rect.top() + b.y0 * rect.height()),
                egui::pos2(rect.left() + b.x1 * rect.width(), rect.top() + b.y1 * rect.height()),
            );
            let color = if current {
                accent
            } else if class == Conf::Low {
                Color32::from_rgb(0xd0, 0x3b, 0x32)
            } else {
                Color32::from_rgb(0xc9, 0x8a, 0x00)
            };
            painter.rect_filled(r, 2.0, color.gamma_multiply(0.25));
            painter.rect_stroke(
                r,
                2.0,
                Stroke::new(if current { 3.0 } else { 1.5 }, color),
                StrokeKind::Outside,
            );
            if class == Conf::Low {
                // A dashed underline: low confidence is not carried by color alone.
                let mut x = r.left();
                while x < r.right() {
                    painter.hline(x..=(x + 4.0).min(r.right()), r.bottom() + 2.0, Stroke::new(1.5, color));
                    x += 8.0;
                }
            }
        }
    }

    /// The crop frame: dragged by its handles, moved by its body, nudged by arrows.
    fn crop_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, page: &sheaf_app::api::PageDto) {
        let Some(mut crop) = self.preview.crop else { return };
        let painter = ui.painter_at(rect.expand(2.0));
        let frame = Rect::from_min_max(
            egui::pos2(
                rect.left() + crop.x0 * rect.width(),
                rect.top() + crop.y0 * rect.height(),
            ),
            egui::pos2(
                rect.left() + crop.x1 * rect.width(),
                rect.top() + crop.y1 * rect.height(),
            ),
        );
        // Dim what will be removed.
        let dim = Color32::from_black_alpha(120);
        painter.rect_filled(
            Rect::from_min_max(rect.min, egui::pos2(rect.right(), frame.top())),
            0.0,
            dim,
        );
        painter.rect_filled(
            Rect::from_min_max(egui::pos2(rect.left(), frame.bottom()), rect.max),
            0.0,
            dim,
        );
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(rect.left(), frame.top()),
                egui::pos2(frame.left(), frame.bottom()),
            ),
            0.0,
            dim,
        );
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(frame.right(), frame.top()),
                egui::pos2(rect.right(), frame.bottom()),
            ),
            0.0,
            dim,
        );
        let accent = ui.visuals().selection.bg_fill;
        painter.rect_stroke(frame, 0.0, Stroke::new(2.0, accent), StrokeKind::Outside);

        let handles = [
            (Handle::NW, frame.left_top(), egui::CursorIcon::ResizeNwSe),
            (Handle::N, frame.center_top(), egui::CursorIcon::ResizeVertical),
            (Handle::NE, frame.right_top(), egui::CursorIcon::ResizeNeSw),
            (Handle::E, frame.right_center(), egui::CursorIcon::ResizeHorizontal),
            (Handle::SE, frame.right_bottom(), egui::CursorIcon::ResizeNwSe),
            (Handle::S, frame.center_bottom(), egui::CursorIcon::ResizeVertical),
            (Handle::SW, frame.left_bottom(), egui::CursorIcon::ResizeNeSw),
            (Handle::W, frame.left_center(), egui::CursorIcon::ResizeHorizontal),
        ];
        let body = ui.interact(
            frame.shrink(8.0),
            ui.scope_id().with("crop-body"),
            Sense::click_and_drag(),
        );
        let mut grabbed: Option<(Handle, egui::Response)> = None;
        for (handle, center, cursor) in handles {
            let r = Rect::from_center_size(center, vec2(14.0, 14.0));
            painter.rect_filled(r, 3.0, accent);
            painter.rect_stroke(r, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
            let resp = ui
                .interact(
                    r.expand(4.0),
                    ui.scope_id().with(("crop-handle", handle as u8)),
                    Sense::drag(),
                )
                .on_hover_cursor(cursor);
            if resp.drag_started() {
                grabbed = Some((handle, resp));
            }
        }
        if let Some((handle, resp)) = grabbed {
            self.preview.crop_drag = Some((handle, crop, resp.interact_pointer_pos().unwrap_or_default()));
        } else if body.drag_started() {
            self.preview.crop_drag = Some((Handle::Move, crop, body.interact_pointer_pos().unwrap_or_default()));
        }
        if let Some((handle, start, from)) = self.preview.crop_drag {
            if ui.input(|i| i.pointer.primary_down()) {
                if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                    crop = drag_rect(
                        start,
                        handle,
                        (pos.x - from.x) / rect.width(),
                        (pos.y - from.y) / rect.height(),
                    );
                    self.preview.crop = Some(crop);
                }
            } else {
                self.preview.crop_drag = None;
            }
        }
        // Keyboard: arrows move the frame, Shift+arrows resize it.
        if !ui.ctx().egui_wants_keyboard_input() {
            let (dx, dy, shift) = ui.input(|i| {
                let f = |k| if i.key_pressed(k) { 1.0 } else { 0.0 };
                (
                    0.01 * (f(egui::Key::ArrowRight) - f(egui::Key::ArrowLeft)),
                    0.01 * (f(egui::Key::ArrowDown) - f(egui::Key::ArrowUp)),
                    i.modifiers.shift,
                )
            });
            if dx != 0.0 || dy != 0.0 {
                crop = drag_rect(crop, if shift { Handle::SE } else { Handle::Move }, dx, dy);
                self.preview.crop = Some(crop);
            }
        }

        // Apply / cancel.
        let multi = self.sel.ids.len() > 1 && self.sel.ids.contains(&page.id);
        let hint = if multi {
            self.t
                .tv("preview.crop_apply_n", &[("n", &self.sel.ids.len().to_string())])
        } else {
            self.t.t("preview.crop_hint")
        };
        let (apply_l, cancel_l) = (self.t.t("preview.crop_apply"), self.t.t("common.cancel"));
        let ids: Vec<u64> = if multi { self.targets() } else { vec![page.id] };
        let rot = page.rot as i32;
        egui::Area::new(ui.scope_id().with("crop-hint"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(rect.center().x - 190.0, rect.bottom() - 44.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(hint);
                        if ui.button(apply_l).clicked() {
                            let r = display_to_page(normalize(crop), rot);
                            let rect = NormRect {
                                x0: r.x0,
                                y0: r.y0,
                                x1: r.x1,
                                y1: r.y1,
                            };
                            self.preview.crop = None;
                            self.command(move |b| b.apply(UiOp::Crop { ids, rect: Some(rect) }).map(|_| ()));
                        }
                        if ui.button(cancel_l).clicked() {
                            self.preview.crop = None;
                        }
                    });
                });
            });
    }
}
