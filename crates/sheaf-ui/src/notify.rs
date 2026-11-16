//! UI-NOTIFY: what the app tells the person, and how it stays findable.
//!
//! Every message (done, warning, error) is a [`Note`] in a short history. Plain ones appear as a toast
//! and leave by themselves; errors stay until they are closed, so nobody misses one by looking away.
//! The bell in the toolbar counts what has not been seen and opens the history. An error has a
//! details window (what happened, what to try, technical detail, copy, report, help).
//!
//! Notes never hold document text or file contents (SEC-LOG): only what [`sheaf_app::api::ErrorDto`]
//! already carries.

use crate::app::{Modal, SheafApp};
use crate::icons::{icon_button, paint, Icon};
use eframe::egui::{self, vec2, Color32, RichText};
use sheaf_app::api::ErrorDto;
use sheaf_core::ErrorCode;
use std::time::{Duration, Instant};

/// Notes kept in the history.
pub const HISTORY: usize = 50;
/// Toasts on screen at once.
const MAX_TOASTS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub id: u64,
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    pub code: Option<ErrorCode>,
    /// Help topic that explains this (see `help.rs`).
    pub help: Option<&'static str>,
    pub at: Instant,
    /// Seen in the history panel.
    pub read: bool,
    /// Shown as a toast right now.
    pub toast: bool,
    /// When a toast leaves by itself; `None` for errors, which wait to be closed.
    pub until: Option<Instant>,
}

/// The history and what is on screen.
#[derive(Default)]
pub struct Notices {
    items: Vec<Note>,
    next: u64,
    pub panel: bool,
}

fn lifetime(kind: Kind) -> Option<Duration> {
    match kind {
        Kind::Info | Kind::Success => Some(Duration::from_secs(6)),
        Kind::Warning => Some(Duration::from_secs(12)),
        Kind::Error => None,
    }
}

impl Notices {
    pub fn push(
        &mut self,
        kind: Kind,
        title: String,
        detail: String,
        code: Option<ErrorCode>,
        help: Option<&'static str>,
        now: Instant,
    ) -> u64 {
        self.next += 1;
        self.items.push(Note {
            id: self.next,
            kind,
            title,
            detail,
            code,
            help,
            at: now,
            read: self.panel,
            toast: true,
            until: lifetime(kind).map(|d| now + d),
        });
        if self.items.len() > HISTORY {
            self.items.remove(0);
        }
        self.next
    }

    /// Hide the toasts whose time has run out (they stay in the history).
    pub fn expire(&mut self, now: Instant) {
        for n in &mut self.items {
            if n.toast && n.until.is_some_and(|u| u <= now) {
                n.toast = false;
            }
        }
    }

    /// What is on screen: the newest few.
    pub fn toasts(&self) -> Vec<&Note> {
        let shown: Vec<&Note> = self.items.iter().filter(|n| n.toast).collect();
        let skip = shown.len().saturating_sub(MAX_TOASTS);
        shown.into_iter().skip(skip).collect()
    }

    pub fn close_toast(&mut self, id: u64) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.toast = false;
        }
    }

    pub fn get(&self, id: u64) -> Option<&Note> {
        self.items.iter().find(|n| n.id == id)
    }

    /// Newest first.
    pub fn history(&self) -> impl Iterator<Item = &Note> {
        self.items.iter().rev()
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    pub fn unread_errors(&self) -> usize {
        self.items.iter().filter(|n| !n.read && n.kind == Kind::Error).count()
    }

    pub fn mark_all_read(&mut self) {
        for n in &mut self.items {
            n.read = true;
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// The most recent errors, oldest first, for a problem report.
    pub fn recent_errors(&self, n: usize) -> Vec<&Note> {
        let errors: Vec<&Note> = self.items.iter().filter(|x| x.kind == Kind::Error).collect();
        let skip = errors.len().saturating_sub(n);
        errors.into_iter().skip(skip).collect()
    }
}

/// Which help topic explains an error (every one exists in `help::TOPICS`; a test checks).
pub fn help_topic(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NeedsPassword | ErrorCode::ParseError | ErrorCode::PathRejected | ErrorCode::LimitExceeded => {
            "files"
        }
        ErrorCode::Unsupported => "tools",
        ErrorCode::DeviceError | ErrorCode::DeviceBusy | ErrorCode::DeviceCrashed | ErrorCode::NoPaper => "scan",
        ErrorCode::Cancelled => "start",
        ErrorCode::InvalidArg
        | ErrorCode::NotFound
        | ErrorCode::Io
        | ErrorCode::WorkerCrashed
        | ErrorCode::Timeout
        | ErrorCode::EngineMissing
        | ErrorCode::Internal => "trouble",
    }
}

/// The last word of an error's message key (`error.needs_password` gives `needs_password`).
fn suffix(key: &str) -> &str {
    key.strip_prefix("error.").unwrap_or("internal")
}

/// "just now", "3 min ago", "2 h ago".
pub fn ago(t: &crate::i18n::I18n, age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 45 {
        t.t("notify.now")
    } else if secs < 3600 {
        t.tv("notify.min_ago", &[("n", &((secs + 30) / 60).to_string())])
    } else {
        t.tv("notify.h_ago", &[("n", &(secs / 3600).to_string())])
    }
}

fn accent(kind: Kind, ui: &egui::Ui) -> Color32 {
    match kind {
        Kind::Error => crate::theme::Palette::of(ui).bad,
        Kind::Warning => crate::theme::Palette::of(ui).warn,
        Kind::Success => crate::theme::Palette::of(ui).ok,
        Kind::Info => crate::theme::Palette::of(ui).accent,
    }
}

impl SheafApp {
    pub(crate) fn notify(&mut self, kind: Kind, title: String, detail: String, code: Option<ErrorCode>) {
        let help = code.map(help_topic);
        self.notices.push(kind, title, detail, code, help, Instant::now());
        self.ctx.request_repaint();
    }

    /// A plain message. `error` makes it an error that stays until closed.
    pub(crate) fn toast(&mut self, text: impl Into<String>, detail: impl Into<String>, error: bool) {
        let kind = if error { Kind::Error } else { Kind::Info };
        self.notify(kind, text.into(), detail.into(), None);
    }

    pub(crate) fn success(&mut self, text: impl Into<String>, detail: impl Into<String>) {
        self.notify(Kind::Success, text.into(), detail.into(), None);
    }

    pub(crate) fn show_error(&mut self, e: &ErrorDto) {
        let key = if e.key.starts_with("error.") {
            e.key
        } else {
            "error.internal"
        };
        let title = self.t.t(key);
        // Cancelling is something the person did, not something that went wrong.
        let kind = if e.code == ErrorCode::Cancelled {
            Kind::Info
        } else {
            Kind::Error
        };
        self.notify(kind, title, e.msg.clone(), Some(e.code));
    }

    pub(crate) fn open_notice(&mut self, id: u64) {
        self.modal = Some(Modal::Error(id));
    }

    // ---- toasts ---------------------------------------------------------------------

    pub(crate) fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.notices.expire(Instant::now());
        let shown: Vec<Note> = self.notices.toasts().into_iter().cloned().collect();
        if shown.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let (mut close, mut details, mut help) = (None, None, None);
        let (details_text, help_text) = (self.t.t("notify.details"), self.t.t("notify.help"));
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-14.0, -56.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for note in &shown {
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.5, accent(note.kind, ui)))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal_top(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                                paint(
                                    ui.painter(),
                                    r,
                                    if note.kind == Kind::Error || note.kind == Kind::Warning {
                                        Icon::Alert
                                    } else {
                                        Icon::Check
                                    },
                                    accent(note.kind, ui),
                                );
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&note.title).strong());
                                    if note.kind == Kind::Error {
                                        // The detail is for the details window; a toast stays short.
                                        ui.horizontal(|ui| {
                                            if ui.link(&details_text).clicked() {
                                                details = Some(note.id);
                                            }
                                            if let Some(topic) = note.help {
                                                if ui.link(&help_text).clicked() {
                                                    help = Some(topic);
                                                }
                                            }
                                        });
                                    } else if !note.detail.is_empty() {
                                        ui.label(RichText::new(&note.detail).small().weak());
                                    }
                                });
                                if ui.small_button("x").clicked() {
                                    close = Some(note.id);
                                }
                            });
                        });
                    ui.add_space(4.0);
                }
            });
        if let Some(id) = close {
            self.notices.close_toast(id);
        }
        if let Some(id) = details {
            self.notices.close_toast(id);
            self.open_notice(id);
        }
        if let Some(topic) = help {
            self.open_help(topic);
        }
    }

    // ---- the bell and the history ----------------------------------------------------

    pub(crate) fn bell(&mut self, ui: &mut egui::Ui) {
        let tip = self.t.t("notify.title");
        let response = icon_button(ui, Icon::Bell, &tip);
        let unread = self.notices.unread();
        if unread > 0 {
            let color = if self.notices.unread_errors() > 0 {
                ui.visuals().error_fg_color
            } else {
                crate::theme::Palette::of(ui).accent
            };
            let at = response.rect.right_top() + vec2(-6.0, 7.0);
            ui.painter().circle_filled(at, 4.5, color);
        }
        if response.clicked() {
            self.notices.panel = !self.notices.panel;
            if self.notices.panel {
                self.notices.mark_all_read();
            }
        }
    }

    pub(crate) fn history_ui(&mut self, ctx: &egui::Context) {
        if !self.notices.panel {
            return;
        }
        let rows: Vec<Note> = self.notices.history().cloned().collect();
        let (mut open, mut clear, mut details, mut help) = (true, false, None, None);
        egui::Window::new(self.t.t("notify.title"))
            .open(&mut open)
            .anchor(egui::Align2::RIGHT_TOP, vec2(-12.0, 52.0))
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                if rows.is_empty() {
                    ui.label(RichText::new(self.t.t("notify.empty")).weak());
                    return;
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for note in &rows {
                        ui.horizontal_top(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, accent(note.kind, ui));
                            ui.vertical(|ui| {
                                ui.label(&note.title);
                                if !note.detail.is_empty() && note.kind != Kind::Error {
                                    ui.label(RichText::new(&note.detail).small().weak());
                                }
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(ago(&self.t, note.at.elapsed())).small().weak());
                                    if note.kind == Kind::Error {
                                        if ui.small_button(self.t.t("notify.details")).clicked() {
                                            details = Some(note.id);
                                        }
                                        if let Some(topic) = note.help {
                                            if ui.small_button(self.t.t("notify.help")).clicked() {
                                                help = Some(topic);
                                            }
                                        }
                                    }
                                });
                            });
                        });
                        ui.separator();
                    }
                });
                if ui.button(self.t.t("notify.clear")).clicked() {
                    clear = true;
                }
            });
        if clear {
            self.notices.clear();
        }
        if !open {
            self.notices.panel = false;
        }
        if let Some(id) = details {
            self.notices.panel = false;
            self.open_notice(id);
        }
        if let Some(topic) = help {
            self.open_help(topic);
        }
    }

    // ---- the error details window ----------------------------------------------------

    pub(crate) fn error_dialog(&mut self, ctx: &egui::Context, id: u64) -> Option<u64> {
        let note = self.notices.get(id).cloned()?;
        let suffix = note.code.map_or("internal", |c| suffix(c.i18n_key()));
        let hint_key = format!("errorhint.{suffix}");
        let (mut close, mut copy, mut report, mut help) = (false, false, false, None);
        // A scan that failed can be started again from here, with the same settings.
        let scan_failed = matches!(
            note.code,
            Some(ErrorCode::DeviceError | ErrorCode::DeviceBusy | ErrorCode::DeviceCrashed | ErrorCode::NoPaper)
        );
        let mut scan_again = false;
        crate::theme::modal(ctx, "error-details").show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::hover());
                paint(ui.painter(), r, Icon::Alert, accent(note.kind, ui));
                ui.heading(&note.title);
            });
            ui.add_space(6.0);
            ui.label(self.t.t(&hint_key));
            ui.add_space(8.0);
            egui::CollapsingHeader::new(self.t.t("notify.technical"))
                .default_open(false)
                .show(ui, |ui| {
                    if let Some(code) = note.code {
                        ui.label(RichText::new(format!("{code:?}")).monospace());
                    }
                    if !note.detail.is_empty() {
                        ui.label(RichText::new(&note.detail).monospace().small());
                    }
                    ui.label(RichText::new(ago(&self.t, note.at.elapsed())).small().weak());
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if scan_failed && ui.button(self.t.t("notify.scan_again")).clicked() {
                    scan_again = true;
                }
                if ui.button(self.t.t("notify.copy")).clicked() {
                    copy = true;
                }
                if ui.button(self.t.t("report.open")).clicked() {
                    report = true;
                }
                if let Some(topic) = note.help {
                    if ui.button(self.t.t("notify.help")).clicked() {
                        help = Some(topic);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(self.t.t("common.close")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if scan_again {
            self.open_scan();
            return None;
        }
        if copy {
            ctx.copy_text(details_text(&note));
            self.success(self.t.t("notify.copied"), "");
        }
        if let Some(topic) = help {
            self.open_help(topic);
            return None;
        }
        if report {
            self.open_report(Some(id));
            return None;
        }
        if close || crate::dialogs::ui_escape(ctx) {
            return None;
        }
        Some(id)
    }
}

/// What "Copy" puts on the clipboard for one error.
pub fn details_text(note: &Note) -> String {
    let code = note.code.map_or("none".to_string(), |c| format!("{c:?}"));
    format!(
        "{}\ncode: {code}\ndetail: {}",
        note.title,
        crate::report::scrub(&note.detail)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64, base: Instant) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn plain_messages_leave_by_themselves_and_errors_stay() {
        let base = Instant::now();
        let mut n = Notices::default();
        n.push(Kind::Info, "saved".into(), String::new(), None, None, base);
        n.push(
            Kind::Error,
            "broke".into(),
            String::new(),
            Some(ErrorCode::Io),
            Some("trouble"),
            base,
        );
        assert_eq!(n.toasts().len(), 2);
        n.expire(at(7, base));
        let left: Vec<&str> = n.toasts().iter().map(|x| x.title.as_str()).collect();
        assert_eq!(left, ["broke"], "the info toast left, the error waits");
        n.expire(at(3600, base));
        assert_eq!(n.toasts().len(), 1, "an error is never timed out");
        assert_eq!(n.history().count(), 2, "both are still in the history");
    }

    #[test]
    fn closing_a_toast_keeps_the_note() {
        let base = Instant::now();
        let mut n = Notices::default();
        let id = n.push(Kind::Error, "broke".into(), "x".into(), None, None, base);
        n.close_toast(id);
        assert!(n.toasts().is_empty());
        assert!(n.get(id).is_some());
    }

    #[test]
    fn the_bell_counts_what_was_not_seen() {
        let base = Instant::now();
        let mut n = Notices::default();
        n.push(Kind::Info, "a".into(), String::new(), None, None, base);
        n.push(Kind::Error, "b".into(), String::new(), None, None, base);
        assert_eq!((n.unread(), n.unread_errors()), (2, 1));
        n.mark_all_read();
        assert_eq!(n.unread(), 0);
        // A note that arrives while the panel is open is seen at once.
        n.panel = true;
        n.push(Kind::Info, "c".into(), String::new(), None, None, base);
        assert_eq!(n.unread(), 0);
    }

    #[test]
    fn only_the_newest_toasts_are_shown_and_the_history_is_bounded() {
        let base = Instant::now();
        let mut n = Notices::default();
        for i in 0..(HISTORY + 10) {
            n.push(Kind::Error, format!("e{i}"), String::new(), None, None, base);
        }
        assert_eq!(n.toasts().len(), MAX_TOASTS);
        assert_eq!(n.toasts().last().map(|x| x.title.as_str()), Some("e59"));
        assert_eq!(n.history().count(), HISTORY);
        assert_eq!(
            n.history().next().map(|x| x.title.as_str()),
            Some("e59"),
            "newest first"
        );
    }

    #[test]
    fn recent_errors_skip_the_rest_and_keep_order() {
        let base = Instant::now();
        let mut n = Notices::default();
        for (kind, title) in [
            (Kind::Error, "a"),
            (Kind::Info, "b"),
            (Kind::Error, "c"),
            (Kind::Error, "d"),
        ] {
            n.push(kind, title.into(), String::new(), None, None, base);
        }
        let got: Vec<&str> = n.recent_errors(2).iter().map(|x| x.title.as_str()).collect();
        assert_eq!(got, ["c", "d"]);
    }

    #[test]
    fn every_error_code_has_a_message_a_hint_and_a_help_topic_in_every_language() {
        use crate::help::TOPICS;
        let all = [
            ErrorCode::InvalidArg,
            ErrorCode::NotFound,
            ErrorCode::PathRejected,
            ErrorCode::LimitExceeded,
            ErrorCode::ParseError,
            ErrorCode::NeedsPassword,
            ErrorCode::Unsupported,
            ErrorCode::Io,
            ErrorCode::WorkerCrashed,
            ErrorCode::Timeout,
            ErrorCode::Cancelled,
            ErrorCode::EngineMissing,
            ErrorCode::DeviceError,
            ErrorCode::DeviceBusy,
            ErrorCode::DeviceCrashed,
            ErrorCode::NoPaper,
            ErrorCode::Internal,
        ];
        for lang in ["en", "uz", "ru"] {
            let t = crate::i18n::I18n::new(lang);
            for code in all {
                let key = code.i18n_key();
                assert_ne!(t.t(key), key, "{lang}: {key} has no text");
                let hint = format!("errorhint.{}", suffix(key));
                assert_ne!(t.t(&hint), hint, "{lang}: {hint} has no text");
                assert!(
                    TOPICS.contains(&help_topic(code)),
                    "{code:?} points to a topic that does not exist"
                );
            }
        }
    }

    #[test]
    fn times_read_naturally() {
        let t = crate::i18n::I18n::new("en");
        assert_eq!(ago(&t, Duration::from_secs(5)), "just now");
        assert_eq!(ago(&t, Duration::from_secs(5 * 60)), "5 min ago");
        assert_eq!(ago(&t, Duration::from_secs(3 * 3600 + 5)), "3 h ago");
    }
}
