//! UI-REPORT: a problem report that is written on this computer and goes nowhere by itself.
//!
//! The app has no network (SEC-NONET), so "report a problem" means: build a short text about the
//! program and its last errors, show it, and let the person copy it or save it and send it
//! themselves. It never holds documents, file names or folder names: error messages are scrubbed of
//! anything that looks like a path, a file name or an e-mail address before they go in, and so is
//! what the person types.

use crate::app::{Modal, Msg, SheafApp};
use eframe::egui::{self, RichText};
use sheaf_app::api::EngineDto;
use std::path::PathBuf;

/// Where people are told to send a report. Set this to the real contact before a release.
pub const SUPPORT_CONTACT: &str = "https://github.com/nurmamatovhusanbek-create/sheaf/issues";

/// Longest text the person can add.
const MAX_WORDS: usize = 2_000;
/// Errors listed in a report.
const MAX_ERRORS: usize = 10;
/// Longest piece of an error message that goes into a report.
const MAX_DETAIL: usize = 300;

const FILE_ENDINGS: &[&str] = &[
    ".pdf", ".jpg", ".jpeg", ".png", ".tif", ".tiff", ".bmp", ".webp", ".docx", ".doc", ".xlsx", ".txt", ".zip",
];

fn is_path_like(token: &str) -> bool {
    let b = token.as_bytes();
    token.contains('\\')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        || token.starts_with('/')
        || token.starts_with("~/")
        || token.matches('/').count() >= 2
        || token.starts_with("file:")
}

fn is_file_name(token: &str) -> bool {
    let lower = token.to_lowercase();
    FILE_ENDINGS.iter().any(|e| lower.ends_with(e)) && lower.len() > 4
}

/// Replace paths, file names and e-mail addresses with placeholders. Works word by word, so the rest of
/// a sentence stays readable. Whitespace is kept as it was.
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        // Quotes and brackets around a word do not make it something else.
        let core = word.trim_matches(|c: char| "\"'()[]<>,;:!?".contains(c) || c == '`');
        let replaced = if is_path_like(core) {
            Some("<path>")
        } else if core.contains('@') && core.contains('.') {
            Some("<email>")
        } else if is_file_name(core) {
            Some("<file>")
        } else {
            None
        };
        match replaced {
            Some(r) => out.push_str(&word.replace(core, r)),
            None => out.push_str(word),
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn limited(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// Everything a report says, gathered by the window.
pub struct ReportInput {
    pub version: String,
    pub system: String,
    pub renderer: String,
    pub language: String,
    pub theme: String,
    pub engines: EngineDto,
    pub workers: Option<usize>,
    pub ocr_mode: String,
    pub ocr_quality: String,
    pub pages: usize,
    pub sources: usize,
    /// (code name, message, age in seconds), oldest first.
    pub errors: Vec<(String, String, u64)>,
    /// What the person wrote.
    pub words: String,
}

pub fn build(i: &ReportInput) -> String {
    let verified = match i.engines.verified {
        Some(true) => "yes",
        Some(false) => "NO",
        None => "still checking",
    };
    let mut out = String::new();
    out.push_str("Sheaf problem report\n====================\n");
    out.push_str(&format!("Version: {}\n", i.version));
    out.push_str(&format!("System: {}\n", i.system));
    out.push_str(&format!("Drawing: {}\n", i.renderer));
    out.push_str(&format!("Language: {}   Theme: {}\n", i.language, i.theme));
    out.push_str(&format!(
        "Components: files verified: {verified}; OCR engine: {}; stamp font: {}\n",
        if i.engines.tesseract { "found" } else { "missing" },
        if i.engines.font { "found" } else { "missing" },
    ));
    if let Some(e) = &i.engines.error {
        out.push_str(&format!("Component problem: {:?}\n", e.code));
    }
    out.push_str(&format!(
        "Settings: workers {}; OCR {} / {}\n",
        i.workers.map_or("auto".to_string(), |w| w.to_string()),
        i.ocr_mode,
        i.ocr_quality
    ));
    out.push_str(&format!("Open now: {} pages from {} files\n", i.pages, i.sources));
    out.push_str("\nRecent errors (oldest first):\n");
    if i.errors.is_empty() {
        out.push_str("  none\n");
    }
    for (n, (code, msg, age)) in i.errors.iter().take(MAX_ERRORS).enumerate() {
        let msg = limited(&scrub(msg), MAX_DETAIL);
        out.push_str(&format!(
            "  {}. {code}, {} min ago{}\n",
            n + 1,
            age / 60,
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ));
    }
    let words = limited(&scrub(i.words.trim()), MAX_WORDS);
    if !words.is_empty() {
        out.push_str("\nWhat the person wrote:\n");
        out.push_str(&words);
        out.push('\n');
    }
    out.push_str("\nThis report holds no documents, file names or folder names; paths and file names are removed.\n");
    out
}

/// The operating system, as far as it can be told without asking for more than it gives.
fn system() -> String {
    let base = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let ver = std::process::Command::new("cmd")
            .args(["/C", "ver"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|v| !v.is_empty());
        if let Some(v) = ver {
            return format!("{base} ({v})");
        }
    }
    base
}

fn renderer() -> String {
    if std::env::var(crate::RENDERER_ENV).is_ok_and(|v| v == "wgpu") {
        "Direct3D 12 (fallback; OpenGL was not usable)".into()
    } else {
        "OpenGL".into()
    }
}

pub enum ReportMsg {
    Saved(Option<PathBuf>),
}

pub struct ReportForm {
    /// What the person adds.
    words: String,
    /// The system line, read once (it asks the system).
    system: String,
}

impl SheafApp {
    pub(crate) fn open_report(&mut self, _about: Option<u64>) {
        self.modal = Some(Modal::Report(ReportForm {
            words: String::new(),
            system: system(),
        }));
    }

    fn report_input(&self, f: &ReportForm) -> ReportInput {
        ReportInput {
            version: self.version.clone(),
            system: f.system.clone(),
            renderer: renderer(),
            language: self.settings.language.clone(),
            theme: self.settings.theme.clone(),
            engines: self.engines.clone(),
            workers: self.settings.workers,
            ocr_mode: self.settings.ocr_mode.clone(),
            ocr_quality: self.settings.ocr_quality.clone(),
            pages: self.pages.len(),
            sources: self.sources.len(),
            errors: self
                .notices
                .recent_errors(MAX_ERRORS)
                .into_iter()
                .map(|n| {
                    (
                        n.code.map_or("Error".to_string(), |c| format!("{c:?}")),
                        n.detail.clone(),
                        n.at.elapsed().as_secs(),
                    )
                })
                .collect(),
            words: f.words.clone(),
        }
    }

    pub(crate) fn on_report_msg(&mut self, msg: ReportMsg) {
        match msg {
            ReportMsg::Saved(Some(path)) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let text = self.t.tv("report.saved", &[("name", &name)]);
                self.success(text, "");
            }
            ReportMsg::Saved(None) => {}
        }
    }

    pub(crate) fn report_dialog(&mut self, ctx: &egui::Context, mut f: ReportForm) -> Option<ReportForm> {
        let text = build(&self.report_input(&f));
        let (mut close, mut copy, mut save) = (false, false, false);
        crate::theme::modal(ctx, "report").show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(self.t.t("report.title"));
            ui.add_space(4.0);
            ui.label(self.t.t("report.intro"));
            ui.add_space(8.0);
            ui.label(RichText::new(self.t.t("report.words")).strong());
            ui.add(
                egui::TextEdit::multiline(&mut f.words)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_WORDS),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(self.t.t("report.preview")).strong());
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&text).monospace().small()).wrap());
            });
            ui.add_space(8.0);
            ui.label(RichText::new(self.t.tv("report.send", &[("where", SUPPORT_CONTACT)])).weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(self.t.t("notify.copy")).clicked() {
                    copy = true;
                }
                if ui.button(self.t.t("report.save")).clicked() {
                    save = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(self.t.t("common.close")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if copy {
            ctx.copy_text(text.clone());
            self.success(self.t.t("notify.copied"), "");
        }
        if save {
            let (tx, ctx2) = (self.tx.clone(), self.ctx.clone());
            let body = text;
            std::thread::spawn(move || {
                let saved = crate::platform::save_text("sheaf-report.txt", &body);
                let _ = tx.send(Msg::Report(ReportMsg::Saved(saved)));
                ctx2.request_repaint();
            });
        }
        if close || crate::dialogs::ui_escape(ctx) {
            None
        } else {
            Some(f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_file_names_and_addresses_are_removed_and_the_sentence_survives() {
        let s = scrub(
            r#"open C:\Users\Ann\Documents\tax return.pdf failed; see /home/ann/x/y.log or mail ann@example.com"#,
        );
        assert!(
            !s.contains("Ann") && !s.contains("ann@") && !s.contains("home/ann"),
            "{s}"
        );
        assert!(s.contains("open") && s.contains("failed"), "{s}");
        assert!(s.contains("<path>") && s.contains("<email>"), "{s}");
    }

    #[test]
    fn a_bare_file_name_is_removed_too() {
        let s = scrub("could not read invoice-2024.PDF (page 3)");
        assert_eq!(s, "could not read <file> (page 3)", "{s}");
    }

    #[test]
    fn ordinary_words_are_left_alone() {
        for plain in [
            "the worker stopped",
            "and/or both",
            "timeout after 30 s",
            "read: invalid data at offset 12",
            "10:30 was the time",
        ] {
            assert_eq!(scrub(plain), plain, "{plain}");
        }
    }

    #[test]
    fn whitespace_and_quotes_are_kept() {
        assert_eq!(scrub("a  \"C:\\x\\y\"\n b"), "a  \"<path>\"\n b");
    }

    fn input() -> ReportInput {
        ReportInput {
            version: "1.2.3".into(),
            system: "windows x86_64".into(),
            renderer: "OpenGL".into(),
            language: "en".into(),
            theme: "system".into(),
            engines: EngineDto {
                verified: Some(true),
                tesseract: true,
                font: false,
                error: None,
            },
            workers: None,
            ocr_mode: "Manual".into(),
            ocr_quality: "Fast".into(),
            pages: 7,
            sources: 2,
            errors: vec![("Io".into(), r"read D:\Docs\secret plan.pdf: access denied".into(), 180)],
            words: "It broke when I opened C:\\Users\\Ann\\a.pdf".into(),
        }
    }

    #[test]
    fn a_report_says_what_is_needed_and_nothing_personal() {
        let r = build(&input());
        for needed in [
            "Version: 1.2.3",
            "windows x86_64",
            "OpenGL",
            "7 pages from 2 files",
            "Io, 3 min ago",
            "stamp font: missing",
        ] {
            assert!(r.contains(needed), "missing {needed:?} in\n{r}");
        }
        for personal in ["secret plan", "Docs", "Ann", "a.pdf"] {
            assert!(!r.contains(personal), "{personal:?} leaked into\n{r}");
        }
    }

    #[test]
    fn what_the_person_writes_is_capped() {
        let mut i = input();
        i.words = "x".repeat(MAX_WORDS * 3);
        let r = build(&i);
        assert!(r.len() < MAX_WORDS + 2_000, "{}", r.len());
    }

    #[test]
    fn many_errors_are_cut_to_the_limit() {
        let mut i = input();
        i.errors = (0..50).map(|n| ("Io".to_string(), format!("e{n}"), 0)).collect();
        let r = build(&i);
        assert_eq!(r.matches("Io, 0 min ago").count(), MAX_ERRORS);
    }
}
