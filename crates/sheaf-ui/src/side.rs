//! UI-DRAWER (the tools grid and one small form per tool) and UI-OCRPANEL.
//! Every tool ends in one command: a page edit, a tool run, or the export dialog.

use crate::app::SheafApp;
use eframe::egui::{self, vec2, RichText};
use serde_json::{json, Value};
use sheaf_app::api::{OcrPolicyDto, PickDto};
use sheaf_core::{Anchor, Stamp};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Tools,
    Ocr,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Merge,
    Split,
    Extract,
    Remove,
    Organize,
    Rotate,
    Crop,
    Compress,
    Img2Pdf,
    Pdf2Img,
    Stamp,
    Lock,
    Ocr,
    Scan,
}

const TOOLS: [(Tool, &str); 14] = [
    (Tool::Merge, "merge"),
    (Tool::Split, "split"),
    (Tool::Extract, "extract"),
    (Tool::Remove, "remove"),
    (Tool::Organize, "organize"),
    (Tool::Rotate, "rotate"),
    (Tool::Crop, "crop"),
    (Tool::Compress, "compress"),
    (Tool::Img2Pdf, "img2pdf"),
    (Tool::Pdf2Img, "pdf2img"),
    (Tool::Stamp, "stamp"),
    (Tool::Lock, "lock"),
    (Tool::Ocr, "ocr"),
    (Tool::Scan, "scan"),
];

const ANCHORS: [(Anchor, &str); 7] = [
    (Anchor::TopLeft, "TopLeft"),
    (Anchor::TopCenter, "TopCenter"),
    (Anchor::TopRight, "TopRight"),
    (Anchor::Center, "Center"),
    (Anchor::BottomLeft, "BottomLeft"),
    (Anchor::BottomCenter, "BottomCenter"),
    (Anchor::BottomRight, "BottomRight"),
];

const OCR_LANGS: [(&str, &str); 4] = [
    ("eng", "English"),
    ("rus", "Русский"),
    ("uzb", "Oʻzbekcha (Latn)"),
    ("uzb_cyrl", "Ўзбекча (Cyrl)"),
];

pub struct SideState {
    pub tab: Tab,
    pub form: Option<Tool>,
    query: String,
    split_rule: usize,
    split_n: u32,
    split_text: String,
    split_ranges: String,
    split_name: String,
    compress_level: usize,
    pdf2img_extract: bool,
    pdf2img_png: bool,
    pdf2img_dpi: u32,
    stamp_kind: usize,
    stamp_text: String,
    stamp_size: f32,
    stamp_opacity: f32,
    stamp_rotate: f32,
    stamp_anchor: usize,
    stamp_margin: f32,
    stamp_color: [f32; 3],
    stamp_from: u32,
    stamp_to: String,
    stamp_first: i32,
    picked: Option<PickDto>,
    lock_user: String,
    lock_again: String,
    lock_owner: String,
    lock_print: bool,
    lock_copy: bool,
    lock_edit: bool,
    error: Option<String>,
    ocr_all: bool,
    ocr_mode: usize,
    ocr_auto: bool,
    ocr_langs: [bool; 4],
}

impl Default for SideState {
    fn default() -> Self {
        SideState {
            tab: Tab::Tools,
            form: None,
            query: String::new(),
            split_rule: 0,
            split_n: 2,
            split_text: String::new(),
            split_ranges: String::new(),
            split_name: "{source}-{counter}".into(),
            compress_level: 1,
            pdf2img_extract: false,
            pdf2img_png: true,
            pdf2img_dpi: 150,
            stamp_kind: 0,
            stamp_text: "DRAFT".into(),
            stamp_size: 64.0,
            stamp_opacity: 25.0,
            stamp_rotate: 45.0,
            stamp_anchor: 3,
            stamp_margin: 0.0,
            stamp_color: [0.5, 0.5, 0.5],
            stamp_from: 1,
            stamp_to: String::new(),
            stamp_first: 1,
            picked: None,
            lock_user: String::new(),
            lock_again: String::new(),
            lock_owner: String::new(),
            lock_print: true,
            lock_copy: true,
            lock_edit: false,
            error: None,
            ocr_all: false,
            ocr_mode: 0,
            ocr_auto: true,
            ocr_langs: [false; 4],
        }
    }
}

impl SideState {
    pub fn on_pick(&mut self, p: Option<PickDto>) {
        if p.is_some() {
            self.picked = p;
        }
    }

    fn stamp_preset(&mut self) {
        match self.stamp_kind {
            0 => {
                (
                    self.stamp_text,
                    self.stamp_size,
                    self.stamp_opacity,
                    self.stamp_rotate,
                    self.stamp_anchor,
                    self.stamp_margin,
                    self.stamp_color,
                ) = ("DRAFT".into(), 64.0, 25.0, 45.0, 3, 0.0, [0.5; 3])
            }
            1 => {
                (
                    self.stamp_text,
                    self.stamp_size,
                    self.stamp_opacity,
                    self.stamp_rotate,
                    self.stamp_anchor,
                    self.stamp_margin,
                    self.stamp_color,
                ) = ("{n} / {N}".into(), 10.0, 100.0, 0.0, 5, 24.0, [0.0; 3])
            }
            _ => {
                (
                    self.stamp_size,
                    self.stamp_opacity,
                    self.stamp_rotate,
                    self.stamp_anchor,
                    self.stamp_margin,
                ) = (160.0, 60.0, 0.0, 6, 24.0)
            }
        }
    }
}

impl SheafApp {
    pub(crate) fn side_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (a, b) = (self.t.t("side.tools"), self.t.t("side.ocr"));
            ui.selectable_value(&mut self.side.tab, Tab::Tools, a);
            ui.selectable_value(&mut self.side.tab, Tab::Ocr, b);
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.side.tab {
                Tab::Tools => match self.side.form {
                    None => self.tool_grid(ui),
                    Some(tool) => self.tool_form(ui, tool),
                },
                Tab::Ocr => self.ocr_panel(ui),
            });
    }

    fn scope_note(&self, ui: &mut egui::Ui) {
        let n = self.targets().len();
        let text = if n > 0 {
            self.t.tn("scope.selected", n as i64, &[])
        } else {
            self.t.tn("scope.all", self.pages.len() as i64, &[])
        };
        ui.label(RichText::new(text).weak());
    }

    /// Selected pages, or `None` for all of them.
    fn pages_param(&self) -> Option<Vec<u64>> {
        let ids = self.targets();
        (!self.sel.ids.is_empty() && !ids.is_empty()).then_some(ids)
    }

    pub(crate) fn run_tool(&mut self, tool: &'static str, params: Value) {
        let pages = self.pages_param();
        self.start_job(move |b| b.tool_run(tool, params, pages));
        self.side.form = None;
    }

    fn tool_grid(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.side.query)
                .hint_text(self.t.t("side.search_tools"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        let q = self.side.query.trim().to_lowercase();
        let entries: Vec<(Tool, String, String)> = TOOLS
            .iter()
            .map(|(tool, id)| {
                (
                    *tool,
                    self.t.t(&format!("tool.{id}.name")),
                    if *tool == Tool::Scan {
                        self.t.t("tool.soon")
                    } else {
                        self.t.t(&format!("tool.{id}.hint"))
                    },
                )
            })
            .filter(|(_, name, hint)| q.is_empty() || format!("{name} {hint}").to_lowercase().contains(&q))
            .collect();
        if entries.is_empty() {
            ui.label(RichText::new(self.t.t("side.no_tools")).weak());
            return;
        }
        let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        for pair in entries.chunks(2) {
            ui.horizontal(|ui| {
                for (tool, name, hint) in pair {
                    let enabled = *tool != Tool::Scan;
                    let text = egui::WidgetText::from(RichText::new(name).strong());
                    let button = egui::Button::new(text).min_size(vec2(width, 62.0)).wrap();
                    let resp = ui.add_enabled(enabled, button).on_hover_text(hint);
                    if resp.clicked() {
                        self.open_tool(*tool);
                    }
                }
            });
        }
    }

    fn open_tool(&mut self, tool: Tool) {
        match tool {
            Tool::Merge | Tool::Img2Pdf => self.open_dialog(),
            Tool::Extract => self.open_export(true),
            Tool::Remove => self.delete_selected(),
            Tool::Organize => {
                let at = self
                    .targets()
                    .iter()
                    .filter_map(|id| self.pages.iter().position(|p| p.id == *id))
                    .max()
                    .map_or(self.pages.len(), |i| i + 1);
                self.command(move |b| {
                    b.apply(sheaf_app::api::UiOp::InsertBlank { at, size: None })
                        .map(|_| ())
                });
            }
            Tool::Rotate => self.rotate(90),
            Tool::Crop => {
                self.preview.crop = Some(crate::geometry::Rect01 {
                    x0: 0.05,
                    y0: 0.05,
                    x1: 0.95,
                    y1: 0.95,
                })
            }
            Tool::Ocr => self.side.tab = Tab::Ocr,
            Tool::Scan => {}
            Tool::Split | Tool::Compress | Tool::Pdf2Img | Tool::Stamp | Tool::Lock => {
                self.side.error = None;
                self.side.form = Some(tool);
            }
        }
    }

    fn form_header(&mut self, ui: &mut egui::Ui, tool: Tool) {
        if ui.button(format!("< {}", self.t.t("common.back"))).clicked() {
            self.side.form = None;
        }
        let id = TOOLS.iter().find(|(t, _)| *t == tool).map_or("", |(_, id)| *id);
        ui.heading(self.t.t(&format!("tool.{id}.name")));
        self.scope_note(ui);
    }

    fn tool_form(&mut self, ui: &mut egui::Ui, tool: Tool) {
        self.form_header(ui, tool);
        match tool {
            Tool::Split => self.split_form(ui),
            Tool::Compress => self.compress_form(ui),
            Tool::Pdf2Img => self.pdf2img_form(ui),
            Tool::Stamp => self.stamp_form(ui),
            Tool::Lock => self.lock_form(ui),
            _ => {}
        }
    }

    fn split_form(&mut self, ui: &mut egui::Ui) {
        let t = &self.t;
        let rules = [
            t.t("split.every_n"),
            t.t("split.blank"),
            t.t("split.text"),
            t.t("split.ranges"),
        ];
        let (rule_l, n_l, regex_l, regex_h, ranges_l, ranges_h, name_l, name_h, run) = (
            t.t("split.rule"),
            t.t("split.n"),
            t.t("split.regex"),
            t.t("split.regex_hint"),
            t.t("split.ranges_label"),
            t.t("split.ranges_hint"),
            t.t("export.name"),
            t.t("export.name_hint"),
            t.t("common.run"),
        );
        let s = &mut self.side;
        ui.label(rule_l);
        egui::ComboBox::from_id_salt("split-rule")
            .selected_text(&rules[s.split_rule])
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, r) in rules.iter().enumerate() {
                    ui.selectable_value(&mut s.split_rule, i, r);
                }
            });
        match s.split_rule {
            0 => {
                ui.label(n_l);
                ui.add(egui::DragValue::new(&mut s.split_n).range(1..=10_000));
            }
            2 => {
                ui.label(regex_l);
                ui.text_edit_singleline(&mut s.split_text);
                ui.label(RichText::new(regex_h).weak().small());
            }
            3 => {
                ui.label(ranges_l);
                ui.text_edit_singleline(&mut s.split_ranges);
                ui.label(RichText::new(ranges_h).weak().small());
            }
            _ => {}
        }
        ui.label(name_l);
        ui.text_edit_singleline(&mut s.split_name);
        ui.label(RichText::new(name_h).weak().small());
        if ui.button(run).clicked() {
            let rule = match s.split_rule {
                0 => json!({ "EveryN": s.split_n.max(1) }),
                1 => json!("BlankSheet"),
                2 => json!({ "TextMatch": s.split_text }),
                _ => json!({ "Ranges": s.split_ranges }),
            };
            let name = s.split_name.clone();
            self.run_tool("split", json!({ "rule": rule, "name": name }));
        }
    }

    fn compress_form(&mut self, ui: &mut egui::Ui) {
        let levels = [
            ("Low", "compress.low"),
            ("Recommended", "compress.recommended"),
            ("Extreme", "compress.extreme"),
        ];
        for (i, (_, key)) in levels.iter().enumerate() {
            let label = self.t.t(key);
            let hint = self.t.t(&format!("{key}_hint"));
            ui.radio_value(&mut self.side.compress_level, i, RichText::new(label).strong());
            ui.label(RichText::new(hint).weak().small());
        }
        ui.label(RichText::new(self.t.t("compress.report")).weak());
        if ui.button(self.t.t("common.run")).clicked() {
            let level = levels[self.side.compress_level].0;
            self.run_tool("compress", json!({ "level": level }));
        }
    }

    fn pdf2img_form(&mut self, ui: &mut egui::Ui) {
        let (render, extract, mode_l, fmt_l, dpi_l, note) = (
            self.t.t("pdf2img.render"),
            self.t.t("pdf2img.extract"),
            self.t.t("pdf2img.mode"),
            self.t.t("pdf2img.format"),
            self.t.t("pdf2img.dpi"),
            self.t.t("pdf2img.note"),
        );
        let s = &mut self.side;
        ui.label(mode_l);
        ui.radio_value(&mut s.pdf2img_extract, false, render);
        ui.radio_value(&mut s.pdf2img_extract, true, extract);
        if !s.pdf2img_extract {
            ui.label(fmt_l);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.pdf2img_png, true, "PNG");
                ui.selectable_value(&mut s.pdf2img_png, false, "JPG");
            });
            ui.label(dpi_l);
            egui::ComboBox::from_id_salt("dpi")
                .selected_text(format!("{} dpi", s.pdf2img_dpi))
                .show_ui(ui, |ui| {
                    for d in [72, 150, 200, 300, 600] {
                        ui.selectable_value(&mut s.pdf2img_dpi, d, format!("{d} dpi"));
                    }
                });
        }
        ui.label(RichText::new(note).weak());
        if ui.button(self.t.t("common.run")).clicked() {
            let s = &self.side;
            let params = json!({ "mode": if s.pdf2img_extract { "extract" } else { "render" }, "format": if s.pdf2img_png { "png" } else { "jpg" }, "dpi": s.pdf2img_dpi });
            self.run_tool("pdf2img", params);
        }
    }

    fn stamp_form(&mut self, ui: &mut egui::Ui) {
        let kinds = [
            self.t.t("stamp.watermark"),
            self.t.t("stamp.pagenum"),
            self.t.t("stamp.image"),
        ];
        let before = self.side.stamp_kind;
        ui.label(self.t.t("stamp.kind"));
        egui::ComboBox::from_id_salt("stamp-kind")
            .selected_text(&kinds[self.side.stamp_kind])
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, k) in kinds.iter().enumerate() {
                    ui.selectable_value(&mut self.side.stamp_kind, i, k);
                }
            });
        if before != self.side.stamp_kind {
            self.side.stamp_preset();
        }
        if self.side.stamp_kind == 2 {
            let name = self
                .side
                .picked
                .as_ref()
                .map_or_else(|| self.t.t("stamp.no_image"), |p| p.name.clone());
            ui.horizontal(|ui| {
                if ui.button(self.t.t("stamp.choose_image")).clicked() {
                    self.spawn(|b| {
                        let picked = crate::platform::pick_image();
                        crate::app::Msg::Pick(match picked {
                            Some(p) => b
                                .register_pick(&p)
                                .map(Some)
                                .map_err(|e| sheaf_app::api::ErrorDto::from(&e)),
                            None => Ok(None),
                        })
                    });
                }
                ui.label(RichText::new(name).weak());
            });
        } else {
            ui.label(self.t.t("stamp.text"));
            ui.text_edit_singleline(&mut self.side.stamp_text);
            ui.label(RichText::new(self.t.t("stamp.text_hint")).weak().small());
        }
        let (size_l, op_l, rot_l, pos_l, margin_l, color_l, from_l, to_l, first_l, note) = (
            self.t.t("stamp.size"),
            self.t.t("stamp.opacity"),
            self.t.t("stamp.rotation"),
            self.t.t("stamp.position"),
            self.t.t("stamp.margin"),
            self.t.t("stamp.color"),
            self.t.t("stamp.from"),
            self.t.t("stamp.to"),
            self.t.t("stamp.first_number"),
            self.t.t("stamp.page_range_note"),
        );
        let anchor_names: Vec<String> = ANCHORS
            .iter()
            .map(|(_, id)| self.t.t(&format!("anchor.{id}")))
            .collect();
        let s = &mut self.side;
        egui::Grid::new("stamp-grid").num_columns(2).show(ui, |ui| {
            ui.label(size_l);
            ui.add(egui::DragValue::new(&mut s.stamp_size).range(1.0..=2000.0));
            ui.end_row();
            ui.label(op_l);
            ui.add(egui::DragValue::new(&mut s.stamp_opacity).range(0.0..=100.0));
            ui.end_row();
            ui.label(rot_l);
            ui.add(egui::DragValue::new(&mut s.stamp_rotate).range(-180.0..=180.0));
            ui.end_row();
            ui.label(pos_l);
            egui::ComboBox::from_id_salt("anchor")
                .selected_text(&anchor_names[s.stamp_anchor])
                .show_ui(ui, |ui| {
                    for (i, n) in anchor_names.iter().enumerate() {
                        ui.selectable_value(&mut s.stamp_anchor, i, n);
                    }
                });
            ui.end_row();
            ui.label(margin_l);
            ui.add(egui::DragValue::new(&mut s.stamp_margin).range(0.0..=2000.0));
            ui.end_row();
            ui.label(color_l);
            ui.color_edit_button_rgb(&mut s.stamp_color);
            ui.end_row();
            ui.label(from_l);
            ui.add(egui::DragValue::new(&mut s.stamp_from).range(1..=100_000));
            ui.end_row();
            ui.label(to_l);
            ui.add(egui::TextEdit::singleline(&mut s.stamp_to).desired_width(60.0));
            ui.end_row();
            if s.stamp_kind == 1 {
                ui.label(first_l);
                ui.add(egui::DragValue::new(&mut s.stamp_first));
                ui.end_row();
            }
        });
        ui.label(RichText::new(note).weak().small());
        if ui.button(self.t.t("common.run")).clicked() {
            let s = &self.side;
            if s.stamp_kind == 2 && s.picked.is_none() {
                let text = self.t.t("stamp.need_image");
                self.toast(text, "", true);
                return;
            }
            let stamp = Stamp {
                text: if s.stamp_kind == 2 {
                    String::new()
                } else {
                    s.stamp_text.clone()
                },
                image: (s.stamp_kind == 2)
                    .then(|| s.picked.as_ref().map(|p| p.token.clone()))
                    .flatten(),
                size_pt: s.stamp_size,
                opacity: s.stamp_opacity / 100.0,
                rotate_deg: s.stamp_rotate,
                anchor: ANCHORS[s.stamp_anchor].0,
                margin_pt: s.stamp_margin,
                color: s.stamp_color.map(|c| (c * 255.0).round() as u8),
                from_page: s.stamp_from.max(1),
                to_page: s.stamp_to.trim().parse().ok(),
                first_number: s.stamp_first,
            };
            let params = json!({ "stamps": [stamp] });
            self.run_tool("stamp", params);
        }
    }

    fn lock_form(&mut self, ui: &mut egui::Ui) {
        let (u_l, u_h, r_l, o_l, o_h, allow, print, copy, edit, strength) = (
            self.t.t("lock.user_pw"),
            self.t.t("lock.user_pw_hint"),
            self.t.t("lock.repeat"),
            self.t.t("lock.owner_pw"),
            self.t.t("lock.owner_pw_hint"),
            self.t.t("lock.allow"),
            self.t.t("lock.print"),
            self.t.t("lock.copy"),
            self.t.t("lock.edit"),
            self.t.t("lock.strength"),
        );
        let s = &mut self.side;
        ui.label(u_l);
        ui.add(
            egui::TextEdit::singleline(&mut s.lock_user)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        ui.label(RichText::new(u_h).weak().small());
        ui.label(r_l);
        ui.add(
            egui::TextEdit::singleline(&mut s.lock_again)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        ui.label(o_l);
        ui.add(
            egui::TextEdit::singleline(&mut s.lock_owner)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        ui.label(RichText::new(o_h).weak().small());
        ui.label(allow);
        ui.checkbox(&mut s.lock_print, print);
        ui.checkbox(&mut s.lock_copy, copy);
        ui.checkbox(&mut s.lock_edit, edit);
        ui.label(RichText::new(strength).weak().small());
        if let Some(e) = &s.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if ui.button(self.t.t("lock.protect_run")).clicked() {
            let s = &mut self.side;
            s.error = None;
            if s.lock_user.is_empty() && s.lock_owner.is_empty() {
                s.error = Some(self.t.t("lock.need_password"));
            } else if s.lock_user != s.lock_again {
                s.error = Some(self.t.t("lock.mismatch"));
            } else {
                let params = json!({ "protect": { "user_pw": s.lock_user, "owner_pw": s.lock_owner, "allow_print": s.lock_print, "allow_copy": s.lock_copy, "allow_edit": s.lock_edit } });
                s.lock_user.clear();
                s.lock_again.clear();
                s.lock_owner.clear();
                self.run_tool("protect", params);
            }
        }
        ui.separator();
        ui.heading(self.t.t("lock.unlock_title"));
        let locked: Vec<_> = self
            .sources
            .iter()
            .filter(|s| s.encrypted && !s.unlocked)
            .cloned()
            .collect();
        if locked.is_empty() {
            ui.label(RichText::new(self.t.t("lock.none_locked")).weak());
        }
        for src in locked {
            ui.horizontal(|ui| {
                ui.label(&src.name);
                if ui.button(self.t.t("lock.unlock")).clicked() {
                    self.tray.skipped_locked.remove(&src.id);
                    self.queue_password(src.clone());
                }
            });
        }
        ui.label(RichText::new(self.t.t("lock.remove_note")).weak().small());
        if ui.button(self.t.t("lock.remove_run")).clicked() {
            self.run_tool("unlock", json!({}));
        }
    }

    fn ocr_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t.t("ocr.title"));
        let n = self.targets().len();
        let (sel_l, all_l) = (
            self.t.tv("ocr.scope_selected", &[("n", &n.to_string())]),
            self.t.tv("ocr.scope_all", &[("n", &self.pages.len().to_string())]),
        );
        ui.label(self.t.t("ocr.scope"));
        ui.radio_value(&mut self.side.ocr_all, false, sel_l);
        ui.radio_value(&mut self.side.ocr_all, true, all_l);
        ui.label(self.t.t("ocr.mode"));
        for (i, id) in ["add", "redo", "text"].iter().enumerate() {
            let l = self.t.t(&format!("ocr.mode_{id}"));
            ui.radio_value(&mut self.side.ocr_mode, i, l);
        }
        let mode_id = ["add", "redo", "text"][self.side.ocr_mode];
        ui.label(
            RichText::new(self.t.t(&format!("ocr.mode_{mode_id}_hint")))
                .weak()
                .small(),
        );
        ui.label(self.t.t("ocr.langs"));
        ui.checkbox(&mut self.side.ocr_auto, self.t.t("ocr.langs_auto"));
        ui.add_enabled_ui(!self.side.ocr_auto, |ui| {
            for (i, (_, name)) in OCR_LANGS.iter().enumerate() {
                ui.checkbox(&mut self.side.ocr_langs[i], *name);
            }
        });
        let no_engine = !self.engines.tesseract;
        if no_engine {
            ui.colored_label(ui.visuals().error_fg_color, self.t.t("ocr.no_engine"));
        }
        let off = self.settings.ocr_mode == "Off";
        if ui
            .add_enabled(
                !no_engine && !off,
                egui::Button::new(self.t.t("ocr.run")).fill(ui.visuals().selection.bg_fill),
            )
            .clicked()
        {
            let langs: Option<Vec<String>> = if self.side.ocr_auto {
                None
            } else {
                Some(
                    OCR_LANGS
                        .iter()
                        .zip(self.side.ocr_langs)
                        .filter(|(_, on)| *on)
                        .map(|((c, _), _)| c.to_string())
                        .collect(),
                )
            };
            if langs.as_ref().is_some_and(|l| l.is_empty()) {
                let text = self.t.t("ocr.pick_lang");
                self.toast(text, "", true);
            } else {
                let ids = if self.side.ocr_all { None } else { Some(self.targets()) };
                if ids.as_ref().is_some_and(Vec::is_empty) {
                    let text = self.t.t("ocr.nothing_selected");
                    self.toast(text, "", false);
                } else {
                    let policy = OcrPolicyDto {
                        mode: mode_id.into(),
                        langs,
                    };
                    self.start_job(move |b| b.ocr_run(ids, policy));
                }
            }
        }
        ui.separator();
        self.ocr_current(ui);
    }

    fn ocr_current(&mut self, ui: &mut egui::Ui) {
        let Some(page) = self.current_page().cloned() else {
            return;
        };
        ui.heading(self.t.t("ocr.current"));
        let reason = page.ocr.reason.clone().unwrap_or_default();
        ui.label(
            RichText::new(
                self.t
                    .tv(&format!("ocr.state_{}", page.ocr.state), &[("reason", &reason)]),
            )
            .weak(),
        );
        let Some((_, result)) = self.preview.words.clone().filter(|(id, _)| *id == page.id) else {
            return;
        };
        let stat = |ui: &mut egui::Ui, k: String, v: String| {
            ui.horizontal(|ui| {
                ui.label(k);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(v).strong())
                });
            });
        };
        stat(
            ui,
            self.t.t("ocr.confidence"),
            format!("{}%", result.mean_conf.round() as i32),
        );
        stat(ui, self.t.t("ocr.language"), result.langs.join(" + "));
        stat(ui, self.t.t("ocr.words"), result.words.len().to_string());
        let doubtful = self.preview.doubtful_indexes();
        if doubtful.is_empty() {
            ui.label(RichText::new(self.t.t("ocr.none_doubtful")).weak());
        } else {
            ui.label(self.t.tn("ocr.doubtful", doubtful.len() as i64, &[]));
            if ui.button(self.t.t("ocr.next_doubtful")).clicked() {
                self.preview.next_doubtful();
            }
            ui.horizontal_wrapped(|ui| {
                for i in doubtful.iter().take(60) {
                    let w = &result.words[*i];
                    if ui
                        .small_button(format!("{} · {}%", w.text, w.conf.round() as i32))
                        .clicked()
                    {
                        self.preview.doubtful = Some(*i);
                        self.preview.show_text = true;
                    }
                }
            });
        }
        ui.label(self.t.t("ocr.text"));
        let mut text = result.text.clone();
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .desired_width(f32::INFINITY)
                .desired_rows(8)
                .interactive(true)
                .font(egui::TextStyle::Monospace),
        );
    }
}
