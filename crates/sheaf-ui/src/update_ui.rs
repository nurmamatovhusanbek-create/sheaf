//! UI-UPDATE: the "Update" window. Pressing the toolbar button asks GitHub for the newest release; if
//! there is one, the person sees what it is and decides. Nothing is fetched until a button is pressed
//! ([`SEC-NONET`](../../../ARCHITECTURE.md)): there is no check at startup and nothing in the background.
//! The work is `sheaf_app::update`; this file is the window and the threads around it.

use crate::app::{Modal, Msg, SheafApp};
use eframe::egui::{self, RichText};
use sheaf_app::update::{self, Check, Curl, Release, UpdateError};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Latest(String),
    Available(Box<Release>),
    Installing(Box<Release>),
    Failed(UpdateError),
}

pub enum UpdateMsg {
    Checked(Result<Check, UpdateError>),
    Installed(Result<PathBuf, UpdateError>),
}

/// Set once an update is in place: the program to start when this window has closed. The shell
/// reads it after the window is gone, so the old and the new program never run side by side.
static RESTART: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn take_restart_request() -> Option<PathBuf> {
    RESTART.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Can this build replace itself? Only the Windows program is shipped, so only there is it done.
const CAN_SELF_UPDATE: bool = cfg!(windows);

impl SheafApp {
    pub(crate) fn open_update(&mut self) {
        self.modal = Some(Modal::Update);
        if matches!(
            self.update,
            UpdateState::Idle | UpdateState::Failed(_) | UpdateState::Latest(_)
        ) {
            self.check_for_update();
        }
    }

    fn check_for_update(&mut self) {
        self.update = UpdateState::Checking;
        let (tx, ctx, current) = (self.tx.clone(), self.ctx.clone(), self.version.clone());
        std::thread::spawn(move || {
            let result = update::check(&Curl, &current);
            let _ = tx.send(Msg::Update(UpdateMsg::Checked(result)));
            ctx.request_repaint();
        });
    }

    fn install_update(&mut self, release: Box<Release>) {
        self.update = UpdateState::Installing(release.clone());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = std::env::current_exe()
                .map_err(|e| UpdateError::Other(format!("cannot find this program: {e}")))
                .and_then(|exe| update::apply(&Curl, &release, &exe).map(|()| exe));
            let _ = tx.send(Msg::Update(UpdateMsg::Installed(result)));
            ctx.request_repaint();
        });
    }

    pub(crate) fn on_update_msg(&mut self, msg: UpdateMsg) {
        match msg {
            UpdateMsg::Checked(Ok(Check::Available(r))) => self.update = UpdateState::Available(r),
            UpdateMsg::Checked(Ok(Check::UpToDate(v))) => self.update = UpdateState::Latest(v.to_string()),
            UpdateMsg::Checked(Err(e)) | UpdateMsg::Installed(Err(e)) => self.update = UpdateState::Failed(e),
            UpdateMsg::Installed(Ok(exe)) => {
                *RESTART.lock().unwrap_or_else(|p| p.into_inner()) = Some(exe);
                self.modal = None;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The window; true while it stays open.
    pub(crate) fn update_dialog(&mut self, ctx: &egui::Context) -> bool {
        let (mut close, mut recheck, mut install, mut page) = (false, false, None, None);
        let has_work = !self.pages.is_empty();
        crate::theme::modal(ctx, "update").show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(self.t.t("update.title"));
            ui.label(RichText::new(self.t.tv("about.version", &[("version", &self.version)])).weak());
            ui.add_space(8.0);
            match &self.update {
                UpdateState::Idle | UpdateState::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.t.t("update.checking"));
                    });
                }
                UpdateState::Latest(v) => {
                    ui.label(RichText::new(self.t.tv("update.latest", &[("version", v)])).strong());
                    ui.label(RichText::new(self.t.t("update.latest_hint")).weak());
                }
                UpdateState::Available(r) => {
                    ui.label(
                        RichText::new(self.t.tv("update.available", &[("version", &r.version.to_string())])).strong(),
                    );
                    if !r.notes.trim().is_empty() {
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                            ui.label(RichText::new(r.notes.trim()).small());
                        });
                    }
                    ui.add_space(6.0);
                    if !CAN_SELF_UPDATE {
                        ui.label(RichText::new(self.t.t("update.windows_only")).weak());
                    } else {
                        ui.label(RichText::new(self.t.t("update.what_happens")).weak());
                        if has_work {
                            ui.label(RichText::new(self.t.t("update.unsaved")).color(ui.visuals().warn_fg_color));
                        }
                    }
                }
                UpdateState::Installing(r) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.t.tv("update.installing", &[("version", &r.version.to_string())]));
                    });
                }
                UpdateState::Failed(e) => {
                    // Nothing published yet is news, not an error.
                    let color = if matches!(e, UpdateError::NoRelease) {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().error_fg_color
                    };
                    ui.label(RichText::new(self.t.t(e.key())).strong().color(color));
                    let detail = e.to_string();
                    if !detail.is_empty() && !matches!(e, UpdateError::NoRelease | UpdateError::Mismatch) {
                        ui.label(RichText::new(detail).small().weak());
                    }
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| match &self.update {
                UpdateState::Available(r) => {
                    ui.add_enabled_ui(CAN_SELF_UPDATE, |ui| {
                        let label = self.t.t(if has_work {
                            "update.install_restart"
                        } else {
                            "update.install"
                        });
                        let button = egui::Button::new(RichText::new(label).color(egui::Color32::WHITE))
                            .fill(ui.visuals().selection.bg_fill);
                        if ui.add(button).clicked() {
                            install = Some(r.clone());
                        }
                    });
                    if !r.page.is_empty() && ui.button(self.t.t("update.release_page")).clicked() {
                        page = Some(r.page.clone());
                    }
                }
                UpdateState::Failed(e) => {
                    if ui.button(self.t.t("update.retry")).clicked() {
                        recheck = true;
                    }
                    if matches!(e, UpdateError::NotWritable(_)) && ui.button(self.t.t("update.release_page")).clicked()
                    {
                        page = Some(format!("https://github.com/{}/releases/latest", update::REPO));
                    }
                }
                _ => {}
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!matches!(self.update, UpdateState::Installing(_)), |ui| {
                    if ui.button(self.t.t("common.close")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if recheck {
            self.check_for_update();
        }
        if let Some(r) = install {
            self.install_update(r);
        }
        if let Some(url) = page {
            crate::platform::open_url(&url);
        }
        let installing = matches!(self.update, UpdateState::Installing(_));
        !(close || (crate::dialogs::ui_escape(ctx) && !installing))
    }
}
