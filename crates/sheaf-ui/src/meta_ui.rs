//! UI for TOOL-METADATA: what a PDF says about itself, with the fields editable. Changes are
//! always saved as a copy next to the original (`NAME.edited.pdf`, `NAME.clean.pdf`); the
//! original is never touched. The file is chosen in a native dialog and read by a worker.

use crate::app::{Modal, Msg, SheafApp};
use eframe::egui::{self, RichText};
use sheaf_app::api::ErrorDto;
use sheaf_core::proto::{MetaEdit, MetaFields, MetadataInfo};
use std::path::PathBuf;

pub enum MetaMsg {
    /// A file was chosen; read it.
    Picked(PathBuf),
    Loaded(PathBuf, Result<MetadataInfo, ErrorDto>),
}

pub struct MetaForm {
    file: PathBuf,
    /// `None` while a worker is reading the file.
    info: Option<MetadataInfo>,
    edit: MetaFields,
}

impl SheafApp {
    /// TOOL-METADATA: choose a PDF in the native dialog.
    pub(crate) fn open_info(&self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Some(file) = crate::platform::pick_files().into_iter().next() {
                let _ = tx.send(Msg::Meta(MetaMsg::Picked(file)));
                ctx.request_repaint();
            }
        });
    }

    pub(crate) fn on_meta_msg(&mut self, msg: MetaMsg) {
        match msg {
            MetaMsg::Picked(file) => {
                self.modal = Some(Modal::Info(MetaForm {
                    file: file.clone(),
                    info: None,
                    edit: MetaFields::default(),
                }));
                self.spawn(move |b| {
                    let result = b.metadata_read(&file).map_err(|e| ErrorDto::from(&e));
                    Msg::Meta(MetaMsg::Loaded(file, result))
                });
            }
            MetaMsg::Loaded(file, result) => match (&mut self.modal, result) {
                (Some(Modal::Info(f)), Ok(info)) if f.file == file => {
                    f.edit = info.fields.clone();
                    f.info = Some(info);
                }
                (Some(Modal::Info(f)), Err(e)) if f.file == file => {
                    self.modal = None;
                    self.show_error(&e);
                }
                _ => {}
            },
        }
    }

    pub(crate) fn info_dialog(&mut self, ctx: &egui::Context, mut f: MetaForm) -> Option<MetaForm> {
        let mut close = false;
        let mut change: Option<MetaEdit> = None;
        let name = f
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        egui::Modal::new(egui::Id::unique("info")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(self.t.t("meta.title"));
            ui.label(RichText::new(&name).weak());
            ui.add_space(4.0);
            match &f.info {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.t.t("meta.reading"));
                    });
                }
                Some(info) => {
                    egui::Grid::new("info-grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            let e = &mut f.edit;
                            for (key, slot) in [
                                ("title", &mut e.title),
                                ("author", &mut e.author),
                                ("subject", &mut e.subject),
                                ("keywords", &mut e.keywords),
                                ("creator", &mut e.creator),
                                ("producer", &mut e.producer),
                            ] {
                                // "meta.title" is the dialog's heading; the field has its own word.
                                let label = if key == "title" {
                                    "meta.field_title".to_string()
                                } else {
                                    format!("meta.{key}")
                                };
                                ui.label(self.t.t(&label));
                                ui.add(egui::TextEdit::singleline(slot).desired_width(300.0));
                                ui.end_row();
                            }
                            ui.label(self.t.t("meta.dates"));
                            let dates = format!("{} · {}", show_date(&info.created), show_date(&info.modified));
                            ui.label(RichText::new(dates).weak());
                            ui.end_row();
                            ui.label(self.t.t("meta.hidden"));
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    self.t.tn("job.pages", i64::from(info.pages), &[]),
                                    self.t.t(if info.has_xmp { "meta.xmp_yes" } else { "meta.xmp_no" })
                                ))
                                .weak(),
                            );
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    ui.label(RichText::new(self.t.t("meta.hint")).weak().small());
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = f.info.is_some();
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(self.t.t("meta.save")).fill(ui.visuals().selection.bg_fill),
                    )
                    .clicked()
                {
                    change = Some(MetaEdit::Set(f.edit.clone()));
                }
                if ui
                    .add_enabled(ready, egui::Button::new(self.t.t("meta.strip")))
                    .clicked()
                {
                    change = Some(MetaEdit::Strip);
                }
                if ui.button(self.t.t("common.close")).clicked() {
                    close = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if let Some(change) = change {
            let file = f.file.clone();
            self.start_job(move |b| b.metadata_edit(file, change));
            return None;
        }
        if close {
            return None;
        }
        Some(f)
    }
}

/// `D:20260101120000Z` as `2026-01-01`; anything else as the file spells it.
fn show_date(raw: &str) -> String {
    let digits = raw.strip_prefix("D:").unwrap_or(raw);
    if digits.len() >= 8 && digits.as_bytes()[..8].iter().all(u8::is_ascii_digit) {
        format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8])
    } else if raw.is_empty() {
        "–".to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::show_date;

    #[test]
    fn pdf_dates_are_shown_as_days() {
        assert_eq!(show_date("D:20260115093000+05'00'"), "2026-01-15");
        assert_eq!(show_date("D:20260115"), "2026-01-15");
        assert_eq!(show_date(""), "–");
        assert_eq!(show_date("yesterday"), "yesterday");
    }
}
