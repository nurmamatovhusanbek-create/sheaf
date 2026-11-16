//! The main window: state, message pump, layout, toolbar, job bar, toasts.
//!
//! Everything that can block (a worker job, a dialog) runs on a helper thread
//! and comes back as a [`Msg`]; the UI thread only draws and handles messages
//! (PERF-BUDGETS: the UI thread is never blocked).

use crate::i18n::I18n;
use crate::textures::Textures;
use eframe::egui;
use sheaf_app::api::{
    Backend, BackendConfig, Emit, EngineDto, ErrorDto, Event, HitDto, ImportReportDto, OcrResultDto, PageDto, PickDto,
    SettingsDto, SnapshotDto, SourceDto, UnlockDto,
};
use sheaf_app::jobs::JobView;
use sheaf_app::Engines;
use sheaf_core::AppError;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// How the app starts.
pub struct UiConfig {
    pub engines: Result<Engines, AppError>,
    pub base_dir: PathBuf,
    pub config_dir: Option<PathBuf>,
    /// Files to import at start (`sheaf open …`).
    pub files: Vec<PathBuf>,
    /// The last run of the program never reached its clean end (a crash, a forced close).
    pub previous_run_ended_badly: bool,
}

/// Results coming back from helper threads and from the backend.
pub enum Msg {
    Event(Event),
    Refreshed(Result<(SnapshotDto, Vec<PageDto>), ErrorDto>),
    Thumb {
        key: crate::textures::TexKey,
        image: Option<egui::ColorImage>,
        note: Option<sheaf_core::proto::FilterNote>,
    },
    Imported(Result<Vec<ImportReportDto>, ErrorDto>),
    /// A command that has nothing to report but may have failed.
    Done(Result<(), ErrorDto>),
    OcrResult {
        page: u64,
        result: Result<OcrResultDto, ErrorDto>,
    },
    Search(Result<Vec<HitDto>, ErrorDto>),
    Settings(Result<SettingsDto, ErrorDto>),
    Unlock {
        source: u32,
        result: Result<UnlockDto, ErrorDto>,
    },
    Pick(Result<Option<PickDto>, ErrorDto>),
    /// A job was started; nothing else to do, the job bar follows its events.
    JobStarted(Result<u64, ErrorDto>),
    /// Files chosen in the Open dialog or dropped on the window.
    Paths(Vec<PathBuf>),
    /// The scanners the system lists (the Scan dialog is waiting for them).
    ScanDevices(Result<Vec<sheaf_app::api::ScanDeviceDto>, ErrorDto>),
    /// Work for the Presets dialog.
    Preset(crate::presets_ui::PresetMsg),
    /// Files chosen for Repair.
    RepairFiles(Vec<PathBuf>),
    /// Work for the document information dialog.
    Meta(crate::meta_ui::MetaMsg),
    /// Work for the problem report window.
    Report(crate::report::ReportMsg),
    /// A helper thread's result for the welcome window.
    Say(crate::about::Say),
    Update(crate::update_ui::UpdateMsg),
}

struct ChannelEmit {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Emit for ChannelEmit {
    fn emit(&self, event: Event) {
        let _ = self.tx.send(Msg::Event(event));
        self.ctx.request_repaint();
    }
}

pub enum Modal {
    Export(crate::dialogs::ExportForm),
    Settings(crate::dialogs::SettingsForm),
    Password(crate::dialogs::PasswordForm),
    Scan(crate::dialogs::ScanForm),
    Presets(crate::presets_ui::PresetsForm),
    Info(crate::meta_ui::MetaForm),
    /// Details of one error (by its id in the notification history).
    Error(u64),
    Report(crate::report::ReportForm),
    About,
    Update,
    Welcome,
}

#[derive(Default)]
pub struct Selection {
    pub ids: HashSet<u64>,
    /// Index of the anchor (for Shift) and of the focused page.
    pub anchor: Option<usize>,
    pub focus: Option<usize>,
}

pub struct SheafApp {
    pub(crate) backend: Option<Arc<Backend>>,
    pub(crate) fatal: Option<ErrorDto>,
    pub(crate) tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub(crate) ctx: egui::Context,
    pub(crate) t: I18n,

    pub(crate) pages: Vec<PageDto>,
    pub(crate) rev: u64,
    pub(crate) can_undo: bool,
    pub(crate) can_redo: bool,
    pub(crate) sources: Vec<SourceDto>,
    pub(crate) settings: SettingsDto,
    pub(crate) engines: EngineDto,
    pub(crate) version: String,
    pub(crate) update: crate::update_ui::UpdateState,
    refreshing: bool,
    refresh_again: bool,

    pub(crate) sel: Selection,
    pub(crate) jobs: BTreeMap<u64, JobView>,
    dismissed: HashSet<u64>,
    pub(crate) tex: Textures,
    pub(crate) tray: crate::tray::TrayState,
    pub(crate) preview: crate::preview::PreviewState,
    pub(crate) side: crate::side::SideState,
    pub(crate) search: crate::search::SearchState,
    pub(crate) notices: crate::notify::Notices,
    pub(crate) help: Option<crate::help::HelpState>,
    /// Where settings live (`None`: kept in memory only) and where scratch sessions are made.
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) base_dir: PathBuf,
    pub(crate) modal: Option<Modal>,
    pub(crate) locked_queue: VecDeque<SourceDto>,
    /// What the Scan dialog had last time, so the next scan starts from it.
    pub(crate) scan_prefs: crate::dialogs::ScanPrefs,
    pub(crate) drop_hover: bool,
    font_installed: bool,
}

impl SheafApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: UiConfig) -> SheafApp {
        let (tx, rx) = channel();
        let ctx = cc.egui_ctx.clone();
        let mut fatal = None;
        let mut font_path = None;
        let (config_dir_keep, base_dir_keep) = (config.config_dir.clone(), config.base_dir.clone());
        let backend = match config.engines {
            Ok(engines) => {
                font_path = engines.font.clone();
                match Backend::start(
                    BackendConfig {
                        engines,
                        base_dir: config.base_dir,
                        config_dir: config.config_dir,
                        verify_vendor: true,
                    },
                    Arc::new(ChannelEmit {
                        tx: tx.clone(),
                        ctx: ctx.clone(),
                    }),
                ) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        fatal = Some(ErrorDto::from(&e));
                        None
                    }
                }
            }
            Err(e) => {
                fatal = Some(ErrorDto::from(&e));
                None
            }
        };
        let settings = backend.as_ref().map(|b| b.settings());
        let (config_dir, base_dir) = (config_dir_keep, base_dir_keep);
        let lang = settings.as_ref().map_or("en", |s| s.language.as_str()).to_string();
        let mut app = SheafApp {
            backend,
            fatal,
            tx,
            rx,
            ctx: ctx.clone(),
            t: I18n::new(&lang),
            pages: Vec::new(),
            rev: 0,
            can_undo: false,
            can_redo: false,
            sources: Vec::new(),
            settings: settings.unwrap_or_else(|| SettingsDto {
                language: lang,
                output_dir: None,
                default_preset: None,
                workers: None,
                ocr_quality: "Fast".into(),
                ocr_mode: "Auto".into(),
                theme: "system".into(),
                wheel_hotkey: sheaf_store::settings::DEFAULT_WHEEL_HOTKEY.into(),
                allow_network_paths: true,
            }),
            engines: EngineDto {
                verified: None,
                tesseract: true,
                font: true,
                error: None,
            },
            version: env!("CARGO_PKG_VERSION").to_string(),
            update: Default::default(),
            refreshing: false,
            refresh_again: false,
            sel: Selection::default(),
            jobs: BTreeMap::new(),
            dismissed: HashSet::new(),
            tex: Textures::default(),
            tray: Default::default(),
            preview: Default::default(),
            side: Default::default(),
            search: Default::default(),
            notices: Default::default(),
            help: None,
            config_dir: config_dir.clone(),
            base_dir: base_dir.clone(),
            modal: None,
            locked_queue: VecDeque::new(),
            scan_prefs: Default::default(),
            drop_hover: false,
            font_installed: false,
        };
        crate::theme::install_fonts(&ctx, font_path.as_deref());
        app.font_installed = true;
        crate::theme::apply(&ctx, &app.settings.theme);
        if app.backend.is_some() && crate::about::welcome_due(app.config_dir.as_deref(), !config.files.is_empty()) {
            app.modal = Some(Modal::Welcome);
        }
        if config.previous_run_ended_badly {
            // Stays on screen until closed, with Details and "Report a problem" behind it.
            let (title, detail) = (app.t.t("notice.unclean"), app.t.t("notice.unclean_detail"));
            app.notify(
                crate::notify::Kind::Warning,
                title,
                detail,
                Some(sheaf_core::ErrorCode::Internal),
            );
        }
        if app.backend.is_some() {
            app.refresh();
            if !config.files.is_empty() {
                app.import(config.files);
            }
        }
        app
    }

    // ---- helpers ---------------------------------------------------------

    /// Run `work` off the UI thread and send its message back.
    pub(crate) fn spawn<F>(&self, work: F)
    where
        F: FnOnce(Arc<Backend>) -> Msg + Send + 'static,
    {
        let Some(backend) = self.backend.clone() else { return };
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work(backend));
            ctx.request_repaint();
        });
    }

    /// Re-read the page list and the snapshot, coalescing bursts of events.
    pub(crate) fn refresh(&mut self) {
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        self.refreshing = true;
        self.spawn(|b| {
            let snap = b.snapshot();
            let result = snap.map_err(|e| ErrorDto::from(&e)).and_then(|snap| {
                let mut pages = Vec::with_capacity(snap.total);
                while pages.len() < snap.total {
                    let chunk = b
                        .pages(pages.len(), sheaf_app::api::MAX_PAGE_RANGE)
                        .map_err(|e| ErrorDto::from(&e))?;
                    if chunk.pages.is_empty() {
                        break;
                    }
                    pages.extend(chunk.pages);
                }
                Ok((snap, pages))
            });
            Msg::Refreshed(result)
        });
    }

    pub(crate) fn import(&mut self, files: Vec<PathBuf>) {
        self.spawn(move |b| Msg::Imported(b.import(files).map_err(|e| ErrorDto::from(&e))));
    }

    /// Run a command that only succeeds or fails.
    pub(crate) fn command<F>(&self, f: F)
    where
        F: FnOnce(&Backend) -> Result<(), AppError> + Send + 'static,
    {
        self.spawn(move |b| Msg::Done(f(&b).map_err(|e| ErrorDto::from(&e))));
    }

    pub(crate) fn start_job<F>(&self, f: F)
    where
        F: FnOnce(&Arc<Backend>) -> Result<u64, AppError> + Send + 'static,
    {
        self.spawn(move |b| Msg::JobStarted(f(&b).map_err(|e| ErrorDto::from(&e))));
    }

    // ---- message pump ----------------------------------------------------

    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Event(ev) => self.on_event(ev),
                Msg::Refreshed(r) => {
                    self.refreshing = false;
                    match r {
                        Ok((snap, pages)) => self.on_refreshed(snap, pages),
                        Err(e) => self.show_error(&e),
                    }
                    if std::mem::take(&mut self.refresh_again) {
                        self.refresh();
                    }
                }
                Msg::Thumb { key, image, note } => self.tex.arrived(&self.ctx, key, image, note),
                Msg::Imported(r) => match r {
                    Ok(_) => {}
                    Err(e) => self.show_error(&e),
                },
                Msg::Done(Err(e)) | Msg::JobStarted(Err(e)) => self.show_error(&e),
                Msg::Done(Ok(())) | Msg::JobStarted(Ok(_)) => {}
                Msg::OcrResult { page, result } => self.preview.on_ocr_result(page, result),
                Msg::Search(r) => self.search.on_result(r),
                Msg::Preset(m) => self.on_preset_msg(m),
                Msg::Meta(m) => self.on_meta_msg(m),
                Msg::Report(m) => self.on_report_msg(m),
                Msg::Say(m) => self.on_say(m),
                Msg::Update(m) => self.on_update_msg(m),
                Msg::RepairFiles(files) => self.start_job(move |b| b.repair_run(files)),
                Msg::ScanDevices(r) => {
                    if let Some(Modal::Scan(f)) = &mut self.modal {
                        f.on_devices(r);
                    }
                }
                Msg::Settings(r) => match r {
                    Ok(s) => self.apply_settings(s),
                    Err(e) => self.show_error(&e),
                },
                Msg::Unlock { source, result } => self.on_unlock(source, result),
                Msg::Pick(r) => match r {
                    Ok(p) => self.side.on_pick(p),
                    Err(e) => self.show_error(&e),
                },
                Msg::Paths(paths) => self.import(paths),
            }
        }
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Pages(_) => self.refresh(),
            Event::PageReady(r) => {
                self.can_undo = r.can_undo;
                self.can_redo = r.can_redo;
                self.rev = r.rev;
                if let Some(p) = self.pages.iter_mut().find(|p| p.id == r.page.id) {
                    *p = r.page;
                }
            }
            Event::Job(view) => {
                self.jobs.insert(view.id, view);
            }
            Event::Import(reports) => {
                for r in reports {
                    if r.encrypted {
                        self.queue_password(SourceDto {
                            id: r.source,
                            name: r.name.clone(),
                            kind: "pdf",
                            pages: 0,
                            encrypted: true,
                            unlocked: false,
                        });
                    } else if r.is_unc {
                        let text = self.t.tv("import.network", &[("name", &r.name)]);
                        self.toast(text, "", false);
                    } else if r.has_forms {
                        let text = self.t.tv("import.forms", &[("name", &r.name)]);
                        self.toast(text, "", false);
                    }
                }
            }
            Event::Error(e) => self.show_error(&e),
            Event::Engines(e) => self.engines = e,
        }
    }

    fn on_refreshed(&mut self, snap: SnapshotDto, pages: Vec<PageDto>) {
        self.rev = snap.rev;
        self.can_undo = snap.can_undo;
        self.can_redo = snap.can_redo;
        self.sources = snap.sources;
        self.engines = snap.engines;
        self.version = snap.version.to_string();
        if snap.settings != self.settings {
            self.apply_settings(snap.settings);
        }
        self.pages = pages;
        if self.sel.focus.is_none() && !self.pages.is_empty() {
            self.select_only(0);
        }
        // Drop selected ids that are gone; keep the rest.
        let present: HashSet<u64> = self.pages.iter().map(|p| p.id).collect();
        self.sel.ids.retain(|id| present.contains(id));
        let last = self.pages.len().saturating_sub(1);
        self.sel.focus = self.sel.focus.map(|f| f.min(last)).filter(|_| !self.pages.is_empty());
        self.sel.anchor = self.sel.anchor.map(|f| f.min(last)).filter(|_| !self.pages.is_empty());
        // Locked files that have no pages yet need their password.
        let waiting: Vec<SourceDto> = self
            .sources
            .iter()
            .filter(|s| s.encrypted && !s.unlocked)
            .cloned()
            .collect();
        for s in waiting {
            if !self.locked_queue.iter().any(|q| q.id == s.id)
                && !matches!(&self.modal, Some(Modal::Password(f)) if f.source.id == s.id)
                && !self.tray.skipped_locked.contains(&s.id)
            {
                self.queue_password(s);
            }
        }
    }

    pub(crate) fn apply_settings(&mut self, s: SettingsDto) {
        if s.language != self.t.code() {
            self.t = I18n::new(&s.language);
        }
        crate::theme::apply(&self.ctx, &s.theme);
        self.settings = s;
    }

    // ---- selection helpers -------------------------------------------------

    /// Ids an action applies to: the selection, else the focused page.
    pub(crate) fn targets(&self) -> Vec<u64> {
        if !self.sel.ids.is_empty() {
            // In list order, so the result is stable.
            return self
                .pages
                .iter()
                .filter(|p| self.sel.ids.contains(&p.id))
                .map(|p| p.id)
                .collect();
        }
        self.sel
            .focus
            .and_then(|i| self.pages.get(i))
            .map(|p| vec![p.id])
            .into_iter()
            .flatten()
            .collect()
    }

    pub(crate) fn current_page(&self) -> Option<&PageDto> {
        self.sel
            .focus
            .and_then(|i| self.pages.get(i))
            .or_else(|| self.pages.iter().find(|p| self.sel.ids.contains(&p.id)))
    }

    pub(crate) fn select_only(&mut self, index: usize) {
        if let Some(p) = self.pages.get(index) {
            self.sel.ids = HashSet::from([p.id]);
            self.sel.anchor = Some(index);
            self.sel.focus = Some(index);
        }
    }

    // ---- toolbar and shortcuts ---------------------------------------------

    pub(crate) fn open_dialog(&self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let files = crate::platform::pick_files();
            if !files.is_empty() {
                let _ = tx.send(Msg::Paths(files));
                ctx.request_repaint();
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        use crate::icons::{icon_button, labeled_button, Icon};
        let t = &self.t;
        let no_pages = self.pages.is_empty();
        let (open, ocr, export, undo, redo, settings) = (
            t.t("toolbar.open"),
            t.t("toolbar.ocr"),
            t.t("toolbar.export"),
            t.t("toolbar.undo"),
            t.t("toolbar.redo"),
            t.t("toolbar.settings"),
        );
        let (scan, presets) = (t.t("toolbar.scan"), t.t("toolbar.presets"));
        let help_tip = format!("{}  F1", t.t("help.tooltip"));
        let ocr_off = self.settings.ocr_mode == "Off";
        let (can_undo, can_redo) = (self.can_undo, self.can_redo);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if labeled_button(ui, Icon::Open, &open, false, "Ctrl+O").clicked() {
                self.open_dialog();
            }
            if labeled_button(ui, Icon::Scan, &scan, false, "").clicked() {
                self.open_scan();
            }
            ui.add_enabled_ui(!no_pages && !ocr_off, |ui| {
                if labeled_button(ui, Icon::Ocr, &ocr, false, "F").clicked() {
                    self.run_ocr_selection();
                }
            });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.add_enabled_ui(can_undo, |ui| {
                if icon_button(ui, Icon::Undo, &format!("{undo}  Ctrl+Z")).clicked() {
                    self.undo(false);
                }
            });
            ui.add_enabled_ui(can_redo, |ui| {
                if icon_button(ui, Icon::Redo, &format!("{redo}  Ctrl+Y")).clicked() {
                    self.undo(true);
                }
            });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            if labeled_button(ui, Icon::Presets, &presets, false, "").clicked() {
                self.open_presets();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right to left: the one primary action sits on the edge, then the quiet utilities.
                ui.add_space(6.0);
                ui.add_enabled_ui(!no_pages, |ui| {
                    if labeled_button(ui, Icon::Export, &export, true, "Ctrl+E").clicked() {
                        self.open_export(false);
                    }
                });
                ui.add_space(6.0);
                if icon_button(ui, Icon::Settings, &settings).clicked() {
                    self.modal = Some(Modal::Settings(crate::dialogs::SettingsForm::new(&self.settings)));
                }
                self.help_menu(ui, &help_tip);
                self.bell(ui);
                ui.add_space(6.0);
                self.search_box(ui);
            });
        });
    }

    /// Help, updates, About and the problem report behind one button: three of the four are used
    /// rarely, so they do not each get a permanent place in the toolbar.
    fn help_menu(&mut self, ui: &mut egui::Ui, tip: &str) {
        let response = crate::icons::icon_button(ui, crate::icons::Icon::Help, tip);
        let (mut help, mut update, mut about, mut report) = (false, false, false, false);
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(190.0);
            help = ui.button(format!("{}   F1", self.t.t("help.tooltip"))).clicked();
            update = ui.button(self.t.t("update.tooltip")).clicked();
            ui.separator();
            about = ui.button(self.t.t("about.title")).clicked();
            report = ui.button(self.t.t("report.open")).clicked();
            if help || update || about || report {
                ui.close();
            }
        });
        if help {
            self.toggle_help();
        } else if update {
            self.open_update();
        } else if about {
            self.open_about();
        } else if report {
            self.open_report(None);
        }
    }

    pub(crate) fn undo(&mut self, redo: bool) {
        self.sel.ids.clear();
        self.command(move |b| b.undo(redo).map(|_| ()));
    }

    pub(crate) fn run_ocr_selection(&mut self) {
        let ids = self.targets();
        if ids.is_empty() {
            let text = self.t.t("ocr.nothing_selected");
            self.toast(text, "", false);
            return;
        }
        self.side.tab = crate::side::Tab::Ocr;
        self.start_job(move |b| b.ocr_run(Some(ids), Default::default()));
    }

    /// TOOL-REPAIR: choose the damaged files in the native dialog, then run.
    pub(crate) fn open_repair(&self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let files = crate::platform::pick_files();
            if !files.is_empty() {
                let _ = tx.send(Msg::RepairFiles(files));
                ctx.request_repaint();
            }
        });
    }

    /// UI-SCANDLG: open the dialog and ask the system for scanners (it can take seconds).
    pub(crate) fn open_scan(&mut self) {
        // Recognizing pages as they are scanned is the normal way to scan; only "Never" turns it off.
        let ocr_default = self.settings.ocr_mode != "Off";
        self.modal = Some(Modal::Scan(crate::dialogs::ScanForm::new(
            self.scan_prefs.clone(),
            ocr_default,
        )));
        self.spawn(|b| Msg::ScanDevices(b.scan_devices().map_err(|e| ErrorDto::from(&e))));
    }

    pub(crate) fn open_export(&mut self, selected: bool) {
        self.modal = Some(Modal::Export(crate::dialogs::ExportForm::new(
            self.targets(),
            self.pages.len(),
            selected,
        )));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.modal.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        let pressed = |ctx: &egui::Context, m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        if pressed(ctx, Modifiers::COMMAND, Key::O) {
            self.open_dialog();
        }
        if pressed(ctx, Modifiers::COMMAND, Key::E) && !self.pages.is_empty() {
            self.open_export(false);
        }
        if pressed(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || pressed(ctx, Modifiers::COMMAND, Key::Y) {
            self.undo(true);
        } else if pressed(ctx, Modifiers::COMMAND, Key::Z) {
            self.undo(false);
        }
        if pressed(ctx, Modifiers::COMMAND, Key::D) {
            let ids = self.targets();
            if !ids.is_empty() {
                self.command(move |b| b.apply(sheaf_app::api::UiOp::Duplicate { ids }).map(|_| ()));
            }
        }
        if pressed(ctx, Modifiers::COMMAND, Key::F) {
            self.search.focus_next = true;
        }
        if pressed(ctx, Modifiers::SHIFT, Key::R) {
            self.rotate(-90);
        } else if pressed(ctx, Modifiers::NONE, Key::R) {
            self.rotate(90);
        }
        if pressed(ctx, Modifiers::NONE, Key::Delete) {
            self.delete_selected();
        }
        if pressed(ctx, Modifiers::NONE, Key::F) {
            self.run_ocr_selection();
        }
        if pressed(ctx, Modifiers::NONE, Key::CloseBracket) {
            self.preview.next_doubtful();
        }
    }

    pub(crate) fn rotate(&mut self, deg: i32) {
        let ids = self.targets();
        if !ids.is_empty() {
            self.command(move |b| b.apply(sheaf_app::api::UiOp::Rotate { ids, deg }).map(|_| ()));
        }
    }

    pub(crate) fn delete_selected(&mut self) {
        let ids = self.targets();
        if ids.is_empty() {
            return;
        }
        self.sel.ids.clear();
        self.command(move |b| b.apply(sheaf_app::api::UiOp::Delete { ids }).map(|_| ()));
    }

    // ---- job bar and toasts --------------------------------------------------

    /// The jobs worth showing: the running ones and the last few that ended, minus what was dismissed.
    fn visible_jobs(&self) -> Vec<JobView> {
        let finished: Vec<u64> = self
            .jobs
            .values()
            .filter(|j| matches!(j.state, "done" | "failed" | "cancelled"))
            .map(|j| j.id)
            .collect();
        let keep: HashSet<u64> = finished.iter().rev().take(4).copied().collect();
        self.jobs
            .values()
            .filter(|j| !self.dismissed.contains(&j.id) && !(j.kind == "import" && j.state == "done"))
            .filter(|j| !matches!(j.state, "done" | "failed" | "cancelled") || keep.contains(&j.id))
            .cloned()
            .collect()
    }

    fn job_bar(&mut self, ui: &mut egui::Ui, rows: Vec<JobView>) {
        use crate::kit::{track, Track};
        for job in rows {
            let running = matches!(job.state, "running" | "queued");
            ui.horizontal(|ui| {
                let pal = crate::theme::Palette::of(ui);
                ui.allocate_ui_with_layout(
                    egui::vec2(124.0, 18.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(124.0);
                        ui.add(egui::Label::new(crate::jobs::kind_label(&self.t, &job.kind)).truncate());
                    },
                );
                // The bar, then its figure: the same place on every row, so a column of jobs reads at a glance.
                let known = crate::jobs::knows_progress(&job);
                let (kind, colour, figure) = match job.state {
                    "done" => (Track::Fraction(1.0), pal.ok, "100%".to_string()),
                    // Red and full: it ended, and not well. No percentage that could read as success.
                    "failed" => (Track::Fraction(1.0), pal.bad, "—".to_string()),
                    "cancelled" => (
                        Track::Fraction(job.progress),
                        pal.faint,
                        format!("{}%", (job.progress * 100.0).floor() as u32),
                    ),
                    // A scan: how far the page in transfer is (the driver's figure, or "about" from the pace
                    // of the earlier pages). The bar starts over with each page; the words say which page.
                    _ if job.kind == "scan" => match crate::jobs::scan_fraction(&job) {
                        Some((f, estimate)) => (
                            Track::Fraction(f),
                            pal.accent,
                            format!("{}{}%", if estimate { "~" } else { "" }, (f * 100.0).round() as u32),
                        ),
                        // Before the first page there is nothing to measure yet.
                        None => (Track::Moving, pal.accent, String::new()),
                    },
                    _ if known => (
                        Track::Fraction(job.progress),
                        pal.accent,
                        format!("{}%", (job.progress * 100.0).floor() as u32),
                    ),
                    _ => (Track::Moving, pal.accent, String::new()),
                };
                track(ui, kind, colour, 150.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(38.0, 18.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label(egui::RichText::new(figure).monospace().color(pal.weak));
                    },
                );
                let text = crate::jobs::summary(&self.t, &job);
                if running {
                    ui.label(egui::RichText::new(text).color(pal.weak));
                } else {
                    let color = if job.state == "failed" { pal.bad } else { pal.weak };
                    ui.label(egui::RichText::new(text).color(color));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if ui.button(self.t.t("common.cancel")).clicked() {
                            let id = job.id;
                            self.command(move |b| {
                                b.job_cancel(id);
                                Ok(())
                            });
                        }
                    } else {
                        if crate::icons::icon_button(ui, crate::icons::Icon::Close, &self.t.t("job.dismiss")).clicked()
                        {
                            self.dismissed.insert(job.id);
                        }
                        if !job.outputs.is_empty() && ui.button(self.t.t("job.reveal")).clicked() {
                            let id = job.id;
                            self.command(move |b| {
                                let path = b.reveal_path(id, 0)?;
                                crate::platform::reveal(&path)
                            });
                        }
                    }
                });
            });
        }
    }

    // ---- drag and drop ----------------------------------------------------------

    fn drops(&mut self, ctx: &egui::Context) {
        let (hovering, dropped): (bool, Vec<PathBuf>) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path())
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.to_path_buf())
                    .collect(),
            )
        });
        self.drop_hover = hovering;
        if !dropped.is_empty() {
            self.import(dropped);
        }
        if hovering {
            egui::Area::new(egui::Id::unique("drop"))
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(2.0, crate::theme::Palette::of(ui).accent))
                        .inner_margin(28.0)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(self.t.t("drop.text")).size(18.0));
                        });
                });
        }
    }

    fn fatal_screen(&mut self, ui: &mut egui::Ui) {
        let Some(e) = self.fatal.clone() else { return };
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading(self.t.t("fatal.title"));
                let key = if e.key.starts_with("error.") {
                    e.key
                } else {
                    "error.internal"
                };
                ui.label(self.t.t(key));
                ui.label(egui::RichText::new(&e.msg).weak());
                ui.label(egui::RichText::new(self.t.t("fatal.hint")).weak());
            });
        });
    }
}

impl eframe::App for SheafApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump();
        if self.fatal.is_some() {
            self.fatal_screen(ui);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
            self.toggle_help();
        }
        self.shortcuts(&ctx);
        self.drops(&ctx);

        let p = crate::theme::Palette::of(ui);
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(6, 5)),
            )
            .show(ui, |ui| {
                self.toolbar(ui);
            });
        // Jobs appear when there are any and go away when there are none: no permanent "no jobs" strip.
        let jobs = self.visible_jobs();
        if !jobs.is_empty() {
            egui::Panel::bottom("jobbar")
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| self.job_bar(ui, jobs));
                });
        }
        // With no pages the tray has nothing to hold: the desk gets the room and says what to do.
        if !self.pages.is_empty() {
            egui::Panel::left("tray")
                .default_size(168.0)
                .size_range(130.0..=420.0)
                .frame(
                    egui::Frame::new()
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(8, 6)),
                )
                .show(ui, |ui| self.tray_ui(ui));
        }
        egui::Panel::right("side")
            .default_size(288.0)
            .size_range(260.0..=520.0)
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| self.side_ui(ui));
        if let Some(page) = self.current_page().cloned() {
            egui::Panel::top("previewbar")
                .frame(
                    egui::Frame::new()
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(8, 3)),
                )
                .show(ui, |ui| self.preview_bar(ui, &page));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.desk))
            .show(ui, |ui| self.preview_ui(ui));

        self.modal_ui(&ctx);
        self.help_ui(&ctx);
        self.history_ui(&ctx);
        self.toasts_ui(&ctx);
    }
}

/// Open the main window and run until it closes (FLOW-LIFECYCLE).
pub fn run_window(config: UiConfig) -> eframe::Result {
    let options = crate::renderer::options(
        egui::ViewportBuilder::default()
            .with_title(sheaf_core::APP_NAME)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            .with_icon(egui::IconData {
                rgba: crate::appicon::rgba(64),
                width: 64,
                height: 64,
            })
            .with_drag_and_drop(true),
        false,
    );
    eframe::run_native(
        sheaf_core::APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(SheafApp::new(cc, config)))),
    )
}
