//! UI-EXPORTDLG, UI-SETTINGS and the password prompt (CMD-UNLOCK-PW). One
//! modal at a time, never nested (DOC-UI). The output folder is picked in a
//! native dialog; no path is ever typed (SEC-PATHS).

use crate::app::{Modal, Msg, SheafApp};
use crate::i18n::LANGUAGES;
use eframe::egui::{self, RichText};
use sheaf_app::api::{
    ErrorDto, ExportSpecDto, ScanDeviceDto, ScanProfileDto, SettingsDto, SettingsPatch, SourceDto, UnlockDto,
};
use sheaf_core::{CompressLevel, Protect, SplitRule};
use zeroize::Zeroizing;

pub struct ExportForm {
    selected: Vec<u64>,
    total: usize,
    scope_selected: bool,
    format: usize,
    name: String,
    split: usize,
    split_arg: String,
    compress: Option<usize>,
    protect: bool,
    user_pw: Zeroizing<String>,
    owner_pw: Zeroizing<String>,
    flatten: bool,
    hide_only: bool,
    image_png: bool,
    dpi: u32,
    error: Option<String>,
}

impl ExportForm {
    pub fn new(selected: Vec<u64>, total: usize, prefer_selected: bool) -> ExportForm {
        ExportForm {
            scope_selected: prefer_selected && !selected.is_empty(),
            selected,
            total,
            format: 0,
            name: String::new(),
            split: 0,
            split_arg: String::new(),
            compress: None,
            protect: false,
            user_pw: Zeroizing::default(),
            owner_pw: Zeroizing::default(),
            flatten: false,
            hide_only: false,
            image_png: true,
            dpi: 150,
            error: None,
        }
    }
}

pub struct SettingsForm {
    language: String,
    theme: String,
    ocr_mode: String,
    ocr_quality: String,
    workers: u32,
    wheel_hotkey: String,
    allow_network_paths: bool,
}

impl SettingsForm {
    pub fn new(s: &SettingsDto) -> SettingsForm {
        SettingsForm {
            language: s.language.clone(),
            theme: s.theme.clone(),
            ocr_mode: s.ocr_mode.clone(),
            ocr_quality: s.ocr_quality.clone(),
            workers: s.workers.unwrap_or(0) as u32,
            wheel_hotkey: s.wheel_hotkey.clone(),
            allow_network_paths: s.allow_network_paths,
        }
    }
}

pub struct PasswordForm {
    pub source: SourceDto,
    password: Zeroizing<String>,
    wrong: bool,
    busy: bool,
}

const SPLITS: [&str; 5] = ["None", "EveryN", "BlankSheet", "TextMatch", "Ranges"];

impl SheafApp {
    pub(crate) fn queue_password(&mut self, source: SourceDto) {
        if self.locked_queue.iter().any(|s| s.id == source.id)
            || matches!(&self.modal, Some(Modal::Password(f)) if f.source.id == source.id)
        {
            return;
        }
        self.locked_queue.push_back(source);
    }

    pub(crate) fn on_unlock(&mut self, source: u32, result: Result<UnlockDto, ErrorDto>) {
        match result {
            Ok(r) if r.ok => {
                let name = self
                    .sources
                    .iter()
                    .find(|s| s.id == source)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                let text = self.t.tv("password.unlocked", &[("name", &name)]);
                self.toast(text, "", false);
                self.modal = None;
            }
            Ok(_) => {
                if let Some(Modal::Password(f)) = &mut self.modal {
                    f.wrong = true;
                    f.busy = false;
                }
            }
            Err(e) => {
                if let Some(Modal::Password(f)) = &mut self.modal {
                    f.busy = false;
                }
                self.show_error(&e);
            }
        }
    }

    pub(crate) fn modal_ui(&mut self, ctx: &egui::Context) {
        if self.modal.is_none() {
            if let Some(source) = self.locked_queue.pop_front() {
                self.modal = Some(Modal::Password(PasswordForm {
                    source,
                    password: Zeroizing::default(),
                    wrong: false,
                    busy: false,
                }));
            }
        }
        let Some(modal) = self.modal.take() else { return };
        self.modal = match modal {
            Modal::Export(f) => self.export_dialog(ctx, f).map(Modal::Export),
            Modal::Settings(f) => self.settings_dialog(ctx, f).map(Modal::Settings),
            Modal::Password(f) => self.password_dialog(ctx, f).map(Modal::Password),
            Modal::Scan(f) => self.scan_dialog(ctx, f).map(Modal::Scan),
        };
    }

    fn password_dialog(&mut self, ctx: &egui::Context, mut f: PasswordForm) -> Option<PasswordForm> {
        let mut keep = true;
        let mut submit = false;
        egui::Modal::new(egui::Id::unique("password")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(self.t.t("password.title"));
            ui.label(self.t.tv("password.prompt", &[("name", &f.source.name)]));
            ui.label(RichText::new(self.t.t("password.label")).weak());
            let resp = ui.add_enabled(
                !f.busy,
                egui::TextEdit::singleline(&mut *f.password)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            if !resp.has_focus() && f.password.is_empty() && !f.wrong {
                resp.request_focus();
            }
            if f.wrong {
                ui.colored_label(ui.visuals().error_fg_color, self.t.t("password.wrong"));
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !f.busy,
                        egui::Button::new(self.t.t("password.unlock")).fill(ui.visuals().selection.bg_fill),
                    )
                    .clicked()
                {
                    submit = true;
                }
                if ui.button(self.t.t("password.skip")).clicked() {
                    keep = false;
                }
            });
        });
        if ui_escape(ctx) {
            keep = false;
        }
        if submit && !f.busy {
            f.busy = true;
            f.wrong = false;
            // The password leaves this form the moment it is sent (SEC-SECRETS).
            let pw = std::mem::take(&mut *f.password);
            let id = f.source.id;
            self.spawn(move |b| Msg::Unlock {
                source: id,
                result: b.unlock(id, pw).map_err(|e| ErrorDto::from(&e)),
            });
        }
        if !keep {
            self.tray.skipped_locked.insert(f.source.id);
            return None;
        }
        Some(f)
    }

    fn settings_dialog(&mut self, ctx: &egui::Context, mut f: SettingsForm) -> Option<SettingsForm> {
        let mut close = false;
        let mut save = false;
        egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(self.t.t("settings.title"));
            egui::Grid::new("settings-grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label(self.t.t("settings.language"));
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(LANGUAGES.iter().find(|(c, _)| *c == f.language).map_or("", |(_, n)| *n))
                        .show_ui(ui, |ui| {
                            for (code, name) in LANGUAGES {
                                ui.selectable_value(&mut f.language, code.to_string(), name);
                            }
                        });
                    ui.end_row();
                    ui.label(self.t.t("settings.theme"));
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(self.t.t(&format!("settings.theme_{}", f.theme)))
                        .show_ui(ui, |ui| {
                            for v in ["system", "light", "dark"] {
                                ui.selectable_value(
                                    &mut f.theme,
                                    v.to_string(),
                                    self.t.t(&format!("settings.theme_{v}")),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label(self.t.t("settings.ocr_mode"));
                    egui::ComboBox::from_id_salt("ocrmode")
                        .selected_text(self.t.t(&format!("settings.ocr_{}", f.ocr_mode)))
                        .show_ui(ui, |ui| {
                            for v in ["Manual", "Auto", "Off"] {
                                ui.selectable_value(
                                    &mut f.ocr_mode,
                                    v.to_string(),
                                    self.t.t(&format!("settings.ocr_{v}")),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label(self.t.t("settings.ocr_quality"));
                    egui::ComboBox::from_id_salt("quality")
                        .selected_text(self.t.t(&format!("settings.quality_{}", f.ocr_quality)))
                        .show_ui(ui, |ui| {
                            for v in ["Fast", "Accurate"] {
                                ui.selectable_value(
                                    &mut f.ocr_quality,
                                    v.to_string(),
                                    self.t.t(&format!("settings.quality_{v}")),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label(self.t.t("settings.workers"));
                    ui.add(egui::DragValue::new(&mut f.workers).range(0..=32));
                    ui.end_row();
                    ui.label(self.t.t("settings.wheel_hotkey"));
                    ui.add(egui::TextEdit::singleline(&mut f.wheel_hotkey).desired_width(160.0));
                    ui.end_row();
                });
            ui.label(RichText::new(self.t.t("settings.workers_hint")).weak().small());
            ui.checkbox(&mut f.allow_network_paths, self.t.t("settings.network_paths"));
            ui.label(RichText::new(self.t.t("settings.network_paths_hint")).weak().small());
            ui.label(RichText::new(self.t.t("settings.wheel_hotkey_hint")).weak().small());
            ui.separator();
            ui.label(self.t.t("settings.engines"));
            let e = &self.engines;
            ui.horizontal_wrapped(|ui| {
                let badge = |ui: &mut egui::Ui, ok: bool, yes: String, no: String| {
                    ui.label(
                        RichText::new(if ok { format!("[ok] {yes}") } else { format!("[!] {no}") }).color(if ok {
                            egui::Color32::from_rgb(0x2e, 0x9e, 0x5b)
                        } else {
                            ui.visuals().error_fg_color
                        }),
                    );
                };
                let verifying = if e.verified == Some(false) {
                    self.t.t("settings.verify_failed")
                } else {
                    self.t.t("settings.verifying")
                };
                badge(ui, e.verified == Some(true), self.t.t("settings.verified"), verifying);
                badge(
                    ui,
                    e.tesseract,
                    self.t.t("settings.tesseract_ok"),
                    self.t.t("settings.tesseract_missing"),
                );
                badge(
                    ui,
                    e.font,
                    self.t.t("settings.font_ok"),
                    self.t.t("settings.font_missing"),
                );
            });
            ui.label(
                RichText::new(self.t.tv("settings.about", &[("version", &self.version)]))
                    .weak()
                    .small(),
            );
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(self.t.t("common.save")).fill(ui.visuals().selection.bg_fill))
                    .clicked()
                {
                    save = true;
                }
                if ui.button(self.t.t("common.cancel")).clicked() {
                    close = true;
                }
            });
        });
        if ui_escape(ctx) {
            close = true;
        }
        if save {
            let patch = SettingsPatch {
                language: Some(f.language.clone()),
                theme: Some(f.theme.clone()),
                ocr_mode: Some(f.ocr_mode.clone()),
                ocr_quality: Some(f.ocr_quality.clone()),
                workers: Some(f.workers as usize),
                default_preset: None,
                wheel_hotkey: Some(f.wheel_hotkey.trim().to_ascii_lowercase()),
                allow_network_paths: Some(f.allow_network_paths),
            };
            self.spawn(move |b| Msg::Settings(b.set_settings(patch).map_err(|e| ErrorDto::from(&e))));
            return None;
        }
        (!close).then_some(f)
    }

    fn export_dialog(&mut self, ctx: &egui::Context, mut f: ExportForm) -> Option<ExportForm> {
        let mut close = false;
        let mut start = false;
        egui::Modal::new(egui::Id::unique("export")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(self.t.t("export.title"));
            egui::ScrollArea::vertical()
                .max_height(ctx.content_rect().height() - 180.0)
                .show(ui, |ui| {
                    ui.label(self.t.t("export.scope"));
                    ui.radio_value(
                        &mut f.scope_selected,
                        false,
                        self.t.tv("export.scope_all", &[("n", &f.total.to_string())]),
                    );
                    ui.add_enabled_ui(!f.selected.is_empty(), |ui| {
                        ui.radio_value(
                            &mut f.scope_selected,
                            true,
                            self.t
                                .tv("export.scope_selected", &[("n", &f.selected.len().to_string())]),
                        );
                    });
                    ui.label(self.t.t("export.format"));
                    ui.horizontal(|ui| {
                        for (i, k) in ["export.format_pdf", "export.format_images", "export.format_text"]
                            .iter()
                            .enumerate()
                        {
                            ui.selectable_value(&mut f.format, i, self.t.t(k));
                        }
                    });
                    ui.label(self.t.t("export.name"));
                    let placeholder = ["{source}.edited", "{source}-images", "{source}"][f.format];
                    ui.add(
                        egui::TextEdit::singleline(&mut f.name)
                            .hint_text(placeholder)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label(RichText::new(self.t.t("export.name_hint")).weak().small());
                    if f.format == 0 {
                        ui.label(self.t.t("export.split"));
                        egui::ComboBox::from_id_salt("exsplit")
                            .selected_text(self.t.t(&format!("export.split_{}", SPLITS[f.split])))
                            .width(ui.available_width())
                            .show_ui(ui, |ui| {
                                for (i, s) in SPLITS.iter().enumerate() {
                                    ui.selectable_value(&mut f.split, i, self.t.t(&format!("export.split_{s}")));
                                }
                            });
                        if matches!(SPLITS[f.split], "EveryN" | "TextMatch" | "Ranges") {
                            ui.label(self.t.t(&format!("export.split_arg_{}", SPLITS[f.split])));
                            ui.text_edit_singleline(&mut f.split_arg);
                        }
                        ui.label(self.t.t("export.compress"));
                        egui::ComboBox::from_id_salt("excompress")
                            .selected_text(match f.compress {
                                None => self.t.t("export.compress_none"),
                                Some(0) => self.t.t("compress.low"),
                                Some(1) => self.t.t("compress.recommended"),
                                _ => self.t.t("compress.extreme"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut f.compress, None, self.t.t("export.compress_none"));
                                ui.selectable_value(&mut f.compress, Some(0), self.t.t("compress.low"));
                                ui.selectable_value(&mut f.compress, Some(1), self.t.t("compress.recommended"));
                                ui.selectable_value(&mut f.compress, Some(2), self.t.t("compress.extreme"));
                            });
                        ui.checkbox(&mut f.protect, self.t.t("export.protect"));
                        if f.protect {
                            ui.label(self.t.t("lock.user_pw"));
                            ui.add(
                                egui::TextEdit::singleline(&mut *f.user_pw)
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.label(self.t.t("lock.owner_pw"));
                            ui.add(
                                egui::TextEdit::singleline(&mut *f.owner_pw)
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.label(RichText::new(self.t.t("lock.strength")).weak().small());
                        }
                        ui.checkbox(&mut f.flatten, self.t.t("export.flatten"));
                        ui.checkbox(&mut f.hide_only, self.t.t("export.hide_only"));
                        if f.hide_only {
                            // SEC-HONESTY: say what this mode does.
                            ui.colored_label(
                                egui::Color32::from_rgb(0xc9, 0x8a, 0x00),
                                self.t.t("export.hide_only_warn"),
                            );
                        }
                    } else if f.format == 1 {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut f.image_png, true, "PNG");
                            ui.selectable_value(&mut f.image_png, false, "JPG");
                            egui::ComboBox::from_id_salt("exdpi")
                                .selected_text(format!("{} dpi", f.dpi))
                                .show_ui(ui, |ui| {
                                    for d in [72, 150, 200, 300, 600] {
                                        ui.selectable_value(&mut f.dpi, d, format!("{d} dpi"));
                                    }
                                });
                        });
                    }
                    ui.separator();
                    ui.label(self.t.t("export.folder"));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(
                                self.settings
                                    .output_dir
                                    .clone()
                                    .unwrap_or_else(|| self.t.t("export.beside_source")),
                            )
                            .weak(),
                        );
                        if ui.small_button(self.t.t("export.choose_folder")).clicked() {
                            self.spawn(|b| {
                                let r = match crate::platform::pick_folder() {
                                    Some(dir) => b.set_output_dir(Some(&dir)),
                                    None => Ok(b.settings()),
                                };
                                Msg::Settings(r.map_err(|e| ErrorDto::from(&e)))
                            });
                        }
                        if ui.small_button(self.t.t("export.reset_folder")).clicked() {
                            self.spawn(|b| Msg::Settings(b.set_output_dir(None).map_err(|e| ErrorDto::from(&e))));
                        }
                    });
                    if let Some(e) = &f.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(self.t.t("export.start")).fill(ui.visuals().selection.bg_fill))
                    .clicked()
                {
                    start = true;
                }
                if ui.button(self.t.t("common.cancel")).clicked() {
                    close = true;
                }
            });
        });
        if ui_escape(ctx) {
            close = true;
        }
        if start {
            if f.format == 0 && f.protect && f.user_pw.is_empty() && f.owner_pw.is_empty() {
                f.error = Some(self.t.t("lock.need_password"));
            } else {
                let spec = self.build_spec(&f);
                self.start_job(move |b| b.export(spec));
                return None;
            }
        }
        (!close).then_some(f)
    }

    fn build_spec(&self, f: &ExportForm) -> ExportSpecDto {
        let split = match SPLITS[f.split] {
            "EveryN" => SplitRule::EveryN(f.split_arg.trim().parse().unwrap_or(1u32).max(1)),
            "BlankSheet" => SplitRule::BlankSheet,
            "TextMatch" => SplitRule::TextMatch(f.split_arg.clone()),
            "Ranges" => SplitRule::Ranges(f.split_arg.clone()),
            _ => SplitRule::None,
        };
        let compress = f
            .compress
            .map(|c| [CompressLevel::Low, CompressLevel::Recommended, CompressLevel::Extreme][c.min(2)]);
        let protect = (f.format == 0 && f.protect).then(|| Protect {
            user_pw: sheaf_core::Password::new(f.user_pw.as_str()),
            owner_pw: sheaf_core::Password::new(f.owner_pw.as_str()),
            allow_print: true,
            allow_copy: true,
            allow_edit: false,
        });
        ExportSpecDto {
            format: ["pdf", "images", "text"][f.format].into(),
            pages: f.scope_selected.then(|| f.selected.clone()),
            split,
            name: f.name.clone(),
            compress,
            protect,
            flatten_forms: f.flatten,
            crop_hide_only: f.hide_only,
            image_format: if f.image_png { "png" } else { "jpg" }.into(),
            dpi: f.dpi,
            ..Default::default()
        }
    }
}

fn ui_escape(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

// ---------------------------------------------------------------------
// UI-SCANDLG
// ---------------------------------------------------------------------

const SOURCES: [&str; 2] = ["flatbed", "feeder"];
const COLORS: [&str; 3] = ["color", "gray", "bw"];
const PAPERS: [&str; 4] = ["a4", "a3", "letter", "auto"];

/// What the dialog remembers between scans (in memory for this run).
#[derive(Clone, PartialEq, Debug)]
pub struct ScanPrefs {
    pub device_id: Option<String>,
    pub source: usize,
    pub duplex: bool,
    pub dpi: u32,
    pub color: usize,
    pub paper: usize,
    pub ocr: Option<bool>,
}

impl Default for ScanPrefs {
    fn default() -> Self {
        ScanPrefs {
            device_id: None,
            source: 0,
            duplex: false,
            dpi: 300,
            color: 0,
            paper: 0,
            ocr: None,
        }
    }
}

pub struct ScanForm {
    prefs: ScanPrefs,
    /// `None` while the system is still being asked.
    devices: Option<Result<Vec<ScanDeviceDto>, ErrorDto>>,
    ocr: bool,
}

impl ScanForm {
    pub fn new(prefs: ScanPrefs, ocr_default: bool) -> ScanForm {
        ScanForm {
            ocr: prefs.ocr.unwrap_or(ocr_default),
            prefs,
            devices: None,
        }
    }

    /// The list arrived: keep the remembered scanner if it is still there, else the first.
    pub fn on_devices(&mut self, result: Result<Vec<ScanDeviceDto>, ErrorDto>) {
        if let Ok(list) = &result {
            let known = self
                .prefs
                .device_id
                .as_ref()
                .is_some_and(|id| list.iter().any(|d| &d.id == id));
            if !known {
                self.prefs.device_id = list.first().map(|d| d.id.clone());
            }
        }
        self.devices = Some(result);
    }

    /// What CMD-SCAN-START takes.
    pub fn profile(&self) -> Option<ScanProfileDto> {
        let device_id = self.prefs.device_id.clone()?;
        let p = &self.prefs;
        Some(ScanProfileDto {
            device_id,
            source: SOURCES[p.source.min(1)].into(),
            duplex: p.duplex && p.source == 1,
            dpi: p.dpi,
            color: COLORS[p.color.min(2)].into(),
            paper: PAPERS[p.paper.min(3)].into(),
            ocr: Some(self.ocr),
        })
    }
}

impl SheafApp {
    fn scan_dialog(&mut self, ctx: &egui::Context, mut f: ScanForm) -> Option<ScanForm> {
        let mut close = false;
        let mut start = false;
        let mut refresh = false;
        egui::Modal::new(egui::Id::unique("scan")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(self.t.t("scan.title"));
            ui.add_space(4.0);
            match &f.devices {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.t.t("scan.looking"));
                    });
                }
                Some(Err(e)) => {
                    let key = if e.code == sheaf_core::ErrorCode::Unsupported {
                        "scan.unavailable"
                    } else {
                        e.key
                    };
                    ui.colored_label(ui.visuals().error_fg_color, self.t.t(key));
                    refresh |= ui.button(self.t.t("scan.refresh")).clicked();
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(self.t.t("scan.none"));
                    refresh |= ui.button(self.t.t("scan.refresh")).clicked();
                }
                Some(Ok(list)) => {
                    egui::Grid::new("scan-grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(self.t.t("scan.device"));
                            ui.horizontal(|ui| {
                                let current = list
                                    .iter()
                                    .find(|d| Some(&d.id) == f.prefs.device_id.as_ref())
                                    .map_or("", |d| d.name.as_str());
                                egui::ComboBox::from_id_salt("scan-device")
                                    .selected_text(current)
                                    .width(250.0)
                                    .show_ui(ui, |ui| {
                                        for d in list {
                                            ui.selectable_value(&mut f.prefs.device_id, Some(d.id.clone()), &d.name);
                                        }
                                    });
                                refresh |= ui.button(self.t.t("scan.refresh")).clicked();
                            });
                            ui.end_row();

                            ui.label(self.t.t("scan.source"));
                            egui::ComboBox::from_id_salt("scan-source")
                                .selected_text(self.t.t(&format!("scan.source_{}", SOURCES[f.prefs.source.min(1)])))
                                .show_ui(ui, |ui| {
                                    for (i, v) in SOURCES.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut f.prefs.source,
                                            i,
                                            self.t.t(&format!("scan.source_{v}")),
                                        );
                                    }
                                });
                            ui.end_row();

                            ui.label("");
                            ui.add_enabled(
                                f.prefs.source == 1,
                                egui::Checkbox::new(&mut f.prefs.duplex, self.t.t("scan.duplex")),
                            );
                            ui.end_row();

                            ui.label(self.t.t("scan.dpi"));
                            egui::ComboBox::from_id_salt("scan-dpi")
                                .selected_text(format!("{} dpi", f.prefs.dpi))
                                .show_ui(ui, |ui| {
                                    for dpi in sheaf_app::api::SCAN_DPIS {
                                        ui.selectable_value(&mut f.prefs.dpi, dpi, format!("{dpi} dpi"));
                                    }
                                });
                            ui.end_row();

                            ui.label(self.t.t("scan.color"));
                            egui::ComboBox::from_id_salt("scan-color")
                                .selected_text(self.t.t(&format!("scan.color_{}", COLORS[f.prefs.color.min(2)])))
                                .show_ui(ui, |ui| {
                                    for (i, v) in COLORS.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut f.prefs.color,
                                            i,
                                            self.t.t(&format!("scan.color_{v}")),
                                        );
                                    }
                                });
                            ui.end_row();

                            ui.label(self.t.t("scan.paper"));
                            egui::ComboBox::from_id_salt("scan-paper")
                                .selected_text(self.t.t(&format!("scan.paper_{}", PAPERS[f.prefs.paper.min(3)])))
                                .show_ui(ui, |ui| {
                                    for (i, v) in PAPERS.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut f.prefs.paper,
                                            i,
                                            self.t.t(&format!("scan.paper_{v}")),
                                        );
                                    }
                                });
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    ui.add_enabled(
                        self.engines.tesseract,
                        egui::Checkbox::new(&mut f.ocr, self.t.t("scan.ocr")),
                    );
                    ui.label(RichText::new(self.t.t("scan.hint")).weak().small());
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = f.profile().is_some() && matches!(&f.devices, Some(Ok(l)) if !l.is_empty());
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(self.t.t("scan.start")).fill(ui.visuals().selection.bg_fill),
                    )
                    .clicked()
                {
                    start = true;
                }
                if ui.button(self.t.t("common.cancel")).clicked() {
                    close = true;
                }
            });
        });
        if ui_escape(ctx) {
            close = true;
        }
        if refresh {
            f.devices = None;
            self.spawn(|b| Msg::ScanDevices(b.scan_devices().map_err(|e| ErrorDto::from(&e))));
        }
        if start {
            if let Some(dto) = f.profile() {
                f.prefs.ocr = Some(f.ocr);
                self.scan_prefs = f.prefs.clone();
                self.start_job(move |b| b.scan_start(dto));
                return None;
            }
        }
        if close {
            // Choices are kept even when the scan is not started.
            f.prefs.ocr = Some(f.ocr);
            self.scan_prefs = f.prefs;
            return None;
        }
        Some(f)
    }
}
