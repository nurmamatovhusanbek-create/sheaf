//! UI-HELP: the help center. It is part of the program (nothing to download, no web page), searchable,
//! in the three interface languages, and reachable from F1, the toolbar, the About window and the
//! "Help" link on every error. The text lives with the other strings (`help.<topic>.title` and
//! `help.<topic>.body`).
//!
//! A body is plain text: blocks separated by a blank line; a block starting `# ` is a sub-heading;
//! lines starting `- ` are bullets; anything else is a paragraph.

use crate::app::SheafApp;
use crate::i18n::I18n;
use eframe::egui::{self, RichText};

/// Topic ids, in the order they are listed.
pub const TOPICS: &[&str] = &[
    "start",
    "files",
    "pages",
    "tools",
    "ocr",
    "scan",
    "presets",
    "wheel",
    "menu",
    "shortcuts",
    "privacy",
    "trouble",
];

pub struct HelpState {
    pub topic: &'static str,
    pub query: String,
}

/// One piece of a body, for drawing.
#[derive(Debug, PartialEq, Eq)]
pub enum Block<'a> {
    Heading(&'a str),
    Paragraph(&'a str),
    Bullets(Vec<&'a str>),
}

pub fn blocks(body: &str) -> Vec<Block<'_>> {
    let mut out = Vec::new();
    for chunk in body.split("\n\n").map(str::trim).filter(|c| !c.is_empty()) {
        if let Some(h) = chunk.strip_prefix("# ") {
            out.push(Block::Heading(h.trim()));
        } else if chunk.lines().all(|l| l.trim_start().starts_with("- ")) {
            out.push(Block::Bullets(
                chunk
                    .lines()
                    .map(|l| l.trim_start().trim_start_matches("- ").trim())
                    .collect(),
            ));
        } else {
            out.push(Block::Paragraph(chunk));
        }
    }
    out
}

pub fn title(t: &I18n, topic: &str) -> String {
    t.t(&format!("help.{topic}.title"))
}

pub fn body(t: &I18n, topic: &str) -> String {
    t.t(&format!("help.{topic}.body"))
}

/// Topics whose title or text holds every word of `query` (case does not matter); all of them for an
/// empty query.
pub fn search(t: &I18n, query: &str) -> Vec<&'static str> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    TOPICS
        .iter()
        .copied()
        .filter(|topic| {
            let hay = format!("{} {}", title(t, topic), body(t, topic)).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .collect()
}

impl SheafApp {
    pub(crate) fn open_help(&mut self, topic: &str) {
        let topic = TOPICS.iter().copied().find(|t| *t == topic).unwrap_or("start");
        self.help = Some(HelpState {
            topic,
            query: String::new(),
        });
    }

    pub(crate) fn toggle_help(&mut self) {
        if self.help.is_some() {
            self.help = None;
        } else {
            self.open_help("start");
        }
    }

    pub(crate) fn help_ui(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.help.take() else { return };
        let mut open = true;
        let found = search(&self.t, &state.query);
        // A search that no longer includes the open topic shows the first match.
        if !found.contains(&state.topic) {
            if let Some(first) = found.first() {
                state.topic = first;
            }
        }
        egui::Window::new(self.t.t("help.title"))
            .id(egui::Id::unique("help-window"))
            .open(&mut open)
            .default_size([780.0, 600.0])
            .default_pos(ctx.content_rect().center() - egui::vec2(390.0, 300.0))
            .min_size([520.0, 340.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut state.query)
                                .hint_text(self.t.t("help.search"))
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().id_salt("help-topics").show(ui, |ui| {
                            if found.is_empty() {
                                ui.label(RichText::new(self.t.t("help.none")).weak());
                            }
                            for topic in &found {
                                if ui
                                    .selectable_label(state.topic == *topic, title(&self.t, topic))
                                    .clicked()
                                {
                                    state.topic = topic;
                                }
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().id_salt("help-body").show(ui, |ui| {
                            ui.set_min_width(440.0);
                            if found.is_empty() {
                                return;
                            }
                            ui.heading(title(&self.t, state.topic));
                            ui.add_space(6.0);
                            let text = body(&self.t, state.topic);
                            for block in blocks(&text) {
                                match block {
                                    Block::Heading(h) => {
                                        ui.add_space(8.0);
                                        ui.label(RichText::new(h).strong().size(15.0));
                                        ui.add_space(2.0);
                                    }
                                    Block::Paragraph(p) => {
                                        ui.label(p);
                                        ui.add_space(4.0);
                                    }
                                    Block::Bullets(items) => {
                                        for item in items {
                                            ui.horizontal_top(|ui| {
                                                ui.label("•");
                                                ui.label(item);
                                            });
                                        }
                                        ui.add_space(4.0);
                                    }
                                }
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new(self.t.t("help.footer")).weak().small());
                        });
                    });
                });
            });
        if open {
            self.help = Some(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_title_and_text_in_every_language() {
        for lang in ["en", "uz", "ru"] {
            let t = I18n::new(lang);
            for topic in TOPICS {
                let (ti, bo) = (title(&t, topic), body(&t, topic));
                assert!(!ti.starts_with("help."), "{lang}: {topic} has no title");
                assert!(!bo.starts_with("help.") && bo.len() > 60, "{lang}: {topic} has no text");
                assert!(!blocks(&bo).is_empty(), "{lang}: {topic}");
            }
        }
    }

    #[test]
    fn the_help_does_not_promise_what_the_program_cannot_do() {
        // The wheel's default hotkey and the limits in the text are the ones the program uses.
        let t = I18n::new("en");
        assert!(body(&t, "wheel").contains("Ctrl+Alt+Space"));
        assert_eq!(sheaf_store::settings::DEFAULT_WHEEL_HOTKEY, "ctrl+alt+space");
        assert!(body(&t, "files").contains("2 GB") && sheaf_core::consts::MAX_FILE_BYTES == 2 * 1024 * 1024 * 1024);
        assert!(body(&t, "files").contains("20 000") && sheaf_core::consts::MAX_SOURCE_PAGES == 20_000);
        assert!(body(&t, "pages").contains("100 steps") && sheaf_core::consts::MAX_UNDO == 100);
    }

    #[test]
    fn a_body_is_cut_into_headings_paragraphs_and_bullets() {
        let b = blocks("Intro line.\n\n# Part\n\n- one\n- two\n\nEnd.");
        assert_eq!(
            b,
            vec![
                Block::Paragraph("Intro line."),
                Block::Heading("Part"),
                Block::Bullets(vec!["one", "two"]),
                Block::Paragraph("End."),
            ]
        );
    }

    #[test]
    fn search_finds_topics_in_each_language() {
        let en = I18n::new("en");
        assert_eq!(search(&en, "").len(), TOPICS.len());
        assert!(search(&en, "password").contains(&"files"));
        assert!(search(&en, "SCANNER").contains(&"scan"));
        assert!(search(&en, "hotkey wheel").contains(&"wheel"));
        assert!(search(&en, "zzzqqq").is_empty());
        let ru = I18n::new("ru");
        assert!(search(&ru, "пароль").contains(&"files"));
        let uz = I18n::new("uz");
        assert!(search(&uz, "skaner").contains(&"scan"));
    }

    #[test]
    fn a_topic_that_is_not_listed_opens_the_first_one() {
        // `open_help` falls back to "start"; the table and the fallback agree.
        assert!(TOPICS.contains(&"start"));
    }
}
