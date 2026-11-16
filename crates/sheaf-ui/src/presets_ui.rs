//! UI-PRESETS: pick, edit, save and delete presets, and run one on files
//! (CMD-PRESETS, PRESET-FORMAT). A run works on the chosen files in a
//! session of its own; the open document is not touched. Folders and files
//! are chosen in native dialogs, never typed (SEC-PATHS).

use crate::app::{Modal, SheafApp};
use eframe::egui::{self, RichText};
use sheaf_app::api::ErrorDto;
use sheaf_core::Preset;
use std::path::PathBuf;

/// What the background work tells the dialog.
pub enum PresetMsg {
    Names(Vec<String>),
    Loaded(Result<Preset, ErrorDto>),
    Saved(Result<Preset, ErrorDto>),
    Deleted(Result<(), ErrorDto>),
    Folder(PathBuf),
    Files(Vec<PathBuf>),
}

pub struct PresetsForm {
    /// `None` while the list is being read.
    names: Option<Vec<String>>,
    /// The saved preset the working copy came from, when there is one.
    selected: Option<String>,
    edit: Preset,
}

impl PresetsForm {
    pub fn new() -> PresetsForm {
        PresetsForm {
            names: None,
            selected: None,
            edit: fresh(),
        }
    }
}

fn fresh() -> Preset {
    Preset::default()
}

const MODES: [&str; 4] = ["", "color", "gray", "bw"];
const SPLITS: [&str; 5] = ["None", "EveryN", "BlankSheet", "TextMatch", "Ranges"];
const COMPRESS: [&str; 4] = ["", "Low", "Recommended", "Extreme"];

impl SheafApp {
    pub(crate) fn open_presets(&mut self) {
        self.modal = Some(Modal::Presets(PresetsForm::new()));
        self.reload_presets();
    }

    fn reload_presets(&self) {
        self.spawn(|b| crate::app::Msg::Preset(PresetMsg::Names(b.presets_list())));
    }

    /// Messages from the background work, while the dialog is open.
    pub(crate) fn on_preset_msg(&mut self, msg: PresetMsg) {
        match msg {
            PresetMsg::Names(names) => {
                if let Some(Modal::Presets(f)) = &mut self.modal {
                    f.names = Some(names);
                }
            }
            PresetMsg::Loaded(Ok(preset)) => {
                if let Some(Modal::Presets(f)) = &mut self.modal {
                    f.selected = Some(preset.name.clone());
                    f.edit = preset;
                }
            }
            PresetMsg::Saved(Ok(preset)) => {
                if let Some(Modal::Presets(f)) = &mut self.modal {
                    f.selected = Some(preset.name.clone());
                    f.edit = preset.clone();
                }
                let text = self.t.tv("presets.saved", &[("name", &preset.name)]);
                self.toast(text, "", false);
                self.reload_presets();
            }
            PresetMsg::Deleted(Ok(())) => {
                if let Some(Modal::Presets(f)) = &mut self.modal {
                    f.selected = None;
                    f.edit = fresh();
                }
                self.reload_presets();
            }
            PresetMsg::Loaded(Err(e)) | PresetMsg::Saved(Err(e)) | PresetMsg::Deleted(Err(e)) => self.show_error(&e),
            PresetMsg::Folder(dir) => {
                if let Some(Modal::Presets(f)) = &mut self.modal {
                    f.edit.out_dir = Some(dir.to_string_lossy().into_owned());
                }
            }
            PresetMsg::Files(files) => {
                // The run uses the saved preset, so what is on screen is saved first.
                let preset = match &self.modal {
                    Some(Modal::Presets(f)) => f.edit.clone(),
                    _ => return,
                };
                self.start_job(move |b| {
                    let saved = b.preset_save(preset)?;
                    b.preset_run(&saved.name, files)
                });
            }
        }
    }

    pub(crate) fn presets_dialog(&mut self, ctx: &egui::Context, mut f: PresetsForm) -> Option<PresetsForm> {
        let mut close = false;
        let mut load: Option<String> = None;
        let mut new = false;
        let mut save = false;
        let mut delete = false;
        let mut run = false;
        let mut pick_folder = false;
        egui::Modal::new(egui::Id::unique("presets")).show(ctx, |ui| {
            ui.set_width(680.0);
            ui.heading(self.t.t("presets.title"));
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                // The saved presets.
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    ui.set_min_height(330.0);
                    egui::ScrollArea::vertical()
                        .max_height(290.0)
                        .id_salt("preset-list")
                        .show(ui, |ui| match &f.names {
                            None => {
                                ui.spinner();
                            }
                            Some(names) if names.is_empty() => {
                                ui.label(RichText::new(self.t.t("presets.none")).weak());
                            }
                            Some(names) => {
                                for name in names {
                                    let on = f.selected.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(on, name).clicked() && !on {
                                        load = Some(name.clone());
                                    }
                                }
                            }
                        });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        new = ui.button(self.t.t("presets.new")).clicked();
                        delete = ui
                            .add_enabled(f.selected.is_some(), egui::Button::new(self.t.t("presets.delete")))
                            .clicked();
                    });
                });
                ui.add_space(8.0);
                // The working copy.
                ui.vertical(|ui| {
                    let e = &mut f.edit;
                    egui::Grid::new("preset-grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(self.t.t("presets.name"));
                            ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(280.0));
                            ui.end_row();

                            ui.label(self.t.t("presets.cleanup"));
                            ui.vertical(|ui| {
                                let mode_text = |m: &str| match m {
                                    "" => self.t.t("presets.keep"),
                                    other => self.t.t(&format!("scan.color_{other}")),
                                };
                                egui::ComboBox::from_id_salt("preset-mode")
                                    .selected_text(mode_text(&e.cleanup.mode))
                                    .show_ui(ui, |ui| {
                                        for m in MODES {
                                            ui.selectable_value(&mut e.cleanup.mode, m.to_string(), mode_text(m));
                                        }
                                    });
                                ui.checkbox(&mut e.cleanup.deskew, self.t.t("presets.deskew"));
                                ui.checkbox(&mut e.cleanup.despeckle, self.t.t("presets.despeckle"));
                                ui.checkbox(&mut e.cleanup.drop_blank, self.t.t("presets.drop_blank"));
                            });
                            ui.end_row();

                            ui.label(self.t.t("presets.ocr"));
                            egui::ComboBox::from_id_salt("preset-ocr")
                                .selected_text(self.t.t(&format!("settings.ocr_{}", e.ocr.mode)))
                                .show_ui(ui, |ui| {
                                    for m in ["Off", "Auto"] {
                                        ui.selectable_value(
                                            &mut e.ocr.mode,
                                            m.to_string(),
                                            self.t.t(&format!("settings.ocr_{m}")),
                                        );
                                    }
                                });
                            ui.end_row();

                            ui.label(self.t.t("presets.split"));
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("preset-split")
                                    .selected_text(self.t.t(&format!("presets.split_{}", e.split.rule)))
                                    .show_ui(ui, |ui| {
                                        for r in SPLITS {
                                            ui.selectable_value(
                                                &mut e.split.rule,
                                                r.to_string(),
                                                self.t.t(&format!("presets.split_{r}")),
                                            );
                                        }
                                    });
                                match e.split.rule.as_str() {
                                    "EveryN" => {
                                        ui.add(egui::DragValue::new(&mut e.split.n).range(1..=10_000));
                                        ui.label(self.t.t("presets.split_pages"));
                                    }
                                    "TextMatch" => {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut e.split.pattern)
                                                .hint_text(self.t.t("presets.split_pattern"))
                                                .desired_width(150.0),
                                        );
                                    }
                                    "Ranges" => {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut e.split.ranges)
                                                .hint_text("1-3,5,8-")
                                                .desired_width(150.0),
                                        );
                                    }
                                    _ => {}
                                }
                            });
                            ui.end_row();

                            ui.label(self.t.t("presets.name_pattern"));
                            ui.add(egui::TextEdit::singleline(&mut e.name_pattern).desired_width(280.0));
                            ui.end_row();

                            ui.label(self.t.t("presets.compress"));
                            let mut level = e.compress.clone().unwrap_or_default();
                            let level_text = |l: &str| match l {
                                "" => self.t.t("presets.compress_none"),
                                other => self.t.t(&format!("presets.compress_{other}")),
                            };
                            egui::ComboBox::from_id_salt("preset-compress")
                                .selected_text(level_text(&level))
                                .show_ui(ui, |ui| {
                                    for l in COMPRESS {
                                        ui.selectable_value(&mut level, l.to_string(), level_text(l));
                                    }
                                });
                            e.compress = (!level.is_empty()).then_some(level);
                            ui.end_row();

                            ui.label(self.t.t("presets.folder"));
                            ui.horizontal(|ui| {
                                let shown = e.out_dir.clone().unwrap_or_else(|| self.t.t("presets.folder_here"));
                                ui.add(egui::Label::new(shown).truncate());
                                pick_folder = ui.button(self.t.t("presets.choose")).clicked();
                                if e.out_dir.is_some() && ui.button(self.t.t("presets.clear")).clicked() {
                                    e.out_dir = None;
                                }
                            });
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    ui.label(RichText::new(self.t.t("presets.hint")).weak().small());
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let named = !f.edit.name.trim().is_empty();
                save = ui
                    .add_enabled(
                        named,
                        egui::Button::new(self.t.t("common.save")).fill(ui.visuals().selection.bg_fill),
                    )
                    .clicked();
                run = ui
                    .add_enabled(named, egui::Button::new(self.t.t("presets.run")))
                    .clicked();
                if ui.button(self.t.t("common.close")).clicked() {
                    close = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if let Some(name) = load {
            self.spawn(move |b| {
                crate::app::Msg::Preset(PresetMsg::Loaded(b.preset_get(&name).map_err(|e| ErrorDto::from(&e))))
            });
        }
        if new {
            f.selected = None;
            f.edit = fresh();
        }
        if delete {
            if let Some(name) = f.selected.clone() {
                self.spawn(move |b| {
                    crate::app::Msg::Preset(PresetMsg::Deleted(
                        b.preset_delete(&name).map_err(|e| ErrorDto::from(&e)),
                    ))
                });
            }
        }
        if save {
            let preset = f.edit.clone();
            self.spawn(move |b| {
                crate::app::Msg::Preset(PresetMsg::Saved(b.preset_save(preset).map_err(|e| ErrorDto::from(&e))))
            });
        }
        if pick_folder {
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                if let Some(dir) = crate::platform::pick_folder() {
                    let _ = tx.send(crate::app::Msg::Preset(PresetMsg::Folder(dir)));
                    ctx.request_repaint();
                }
            });
        }
        if run {
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let files = crate::platform::pick_files();
                if !files.is_empty() {
                    let _ = tx.send(crate::app::Msg::Preset(PresetMsg::Files(files)));
                    ctx.request_repaint();
                }
            });
        }
        if close {
            return None;
        }
        Some(f)
    }
}
