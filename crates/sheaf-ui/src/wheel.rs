//! The action wheel (ADR-WHEEL): a radial menu that appears at the cursor,
//! offers what makes sense for the files selected in Explorer or on the
//! desktop, does the work with no main window, and shows the result.
//!
//! This file holds the pure geometry and the overlay app. Finding the selected
//! files and the hotkey are the shell's job (MOD-SHELL).

use crate::i18n::I18n;
use eframe::egui::{self, epaint::Mesh, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use sheaf_app::quick::{run_quick, QuickReport};
use sheaf_app::{App, Engines};
use sheaf_core::{classify, AppError, FileKind, QuickAction, QuickRequest};
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

pub const OUTER: f32 = 200.0;
pub const INNER: f32 = 66.0;
const CARD_W: f32 = 380.0;
const CARD_H: f32 = 250.0;

/// What a slot of the wheel does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Quick(QuickAction),
    /// Open the files in the main window.
    Open,
}

/// Clockwise from the top. The positions never change, so they can be learned.
pub const SLOTS: [Slot; 8] = [
    Slot::Quick(QuickAction::Merge),
    Slot::Quick(QuickAction::Compress),
    Slot::Quick(QuickAction::Ocr),
    Slot::Quick(QuickAction::Split),
    Slot::Quick(QuickAction::PdfToImages),
    Slot::Quick(QuickAction::ImagesToPdf),
    Slot::Quick(QuickAction::Rotate),
    Slot::Open,
];

impl Slot {
    pub fn enabled(self, kinds: &[FileKind]) -> bool {
        match self {
            Slot::Quick(a) => a.applies_to(kinds),
            Slot::Open => true,
        }
    }
}

/// The sector under `p` for a wheel of `n` sectors centred on `center`, sector 0 at the top and
/// counting clockwise; `None` inside the hub or outside the ring.
pub fn sector_at(center: Pos2, p: Pos2, inner: f32, outer: f32, n: usize) -> Option<usize> {
    let d = p - center;
    let r = d.length();
    if n == 0 || r < inner || r > outer {
        return None;
    }
    let angle = d.x.atan2(-d.y).rem_euclid(TAU); // clockwise from up
    let step = TAU / n as f32;
    Some((((angle + step / 2.0) / step).floor() as usize) % n)
}

/// One ring sector as a triangle strip: no concave polygon for the tessellator to get wrong.
fn sector_mesh(center: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, color: Color32) -> Mesh {
    let steps = (((a1 - a0).abs() / 0.06).ceil() as usize).max(2);
    let mut mesh = Mesh::default();
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32;
        let dir = vec2(a.sin(), -a.cos());
        mesh.colored_vertex(center + dir * r0, color);
        mesh.colored_vertex(center + dir * r1, color);
    }
    for i in 0..steps as u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    mesh
}

pub struct WheelConfig {
    pub engines: Result<Engines, AppError>,
    pub base_dir: PathBuf,
    pub config_dir: Option<PathBuf>,
    /// Where the wheel's centre goes, in screen points.
    pub at: (i32, i32),
    pub files: Vec<PathBuf>,
}

enum Phase {
    Pick,
    Running {
        action: QuickAction,
        name: String,
        progress: f32,
    },
    Done(Summary),
}

struct Summary {
    outputs: Vec<PathBuf>,
    problems: Vec<(String, String)>,
    exit: u8,
}

enum Work {
    Progress(f32, String),
    Done(Result<QuickReport, AppError>),
}

pub struct WheelApp {
    t: I18n,
    files: Vec<PathBuf>,
    kinds: Vec<FileKind>,
    center_pt: (f32, f32),
    phase: Phase,
    rx: Option<Receiver<Work>>,
    config: Option<WheelConfig>,
    placed: bool,
    opened: std::time::Instant,
    hover: Option<usize>,
    closing: bool,
    had_focus: bool,
}

impl WheelApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: WheelConfig) -> WheelApp {
        let lang = config
            .config_dir
            .as_deref()
            .map(sheaf_store::Settings::load)
            .map_or("en".to_string(), |s| s.language);
        if let Ok(e) = &config.engines {
            crate::theme::install_fonts(&cc.egui_ctx, e.font.as_deref());
        }
        crate::theme::apply(&cc.egui_ctx, "system");
        let kinds = config.files.iter().map(|f| classify(f)).collect();
        WheelApp {
            t: I18n::new(&lang),
            files: config.files.clone(),
            kinds,
            center_pt: (config.at.0 as f32, config.at.1 as f32),
            phase: Phase::Pick,
            rx: None,
            config: Some(config),
            placed: false,
            opened: std::time::Instant::now(),
            hover: None,
            closing: false,
            had_focus: false,
        }
    }

    fn start(&mut self, ctx: &egui::Context, action: QuickAction) {
        let Some(config) = self.config.take() else { return };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        // From here on the window is a small card, not a transparent square that would catch clicks.
        let (x, y) = self.center_pt;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(CARD_W, CARD_H)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
            x - CARD_W / 2.0,
            y - CARD_H / 2.0,
        )));
        let name = self
            .files
            .first()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.phase = Phase::Running {
            action,
            name,
            progress: 0.0,
        };
        let request = QuickRequest::new(action, self.files.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let engines = config.engines?;
                engines.verify()?; // SEC-SUPPLY: the wheel checks the bundled files too
                let mut app = App::start(&engines, &config.base_dir, 2)?;
                if let Some(dir) = config.config_dir.as_deref() {
                    app.shared()
                        .set_allow_network(sheaf_store::Settings::load(dir).allow_network_paths);
                }
                let progress_tx = tx.clone();
                let progress_ctx = ctx.clone();
                let report = run_quick(&mut app, &request, &move |f, name| {
                    let _ = progress_tx.send(Work::Progress(f, name.to_string()));
                    progress_ctx.request_repaint();
                });
                let _ = app.close();
                report
            })();
            let _ = tx.send(Work::Done(result));
            ctx.request_repaint();
        });
    }

    fn open_in_main(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("open").args(&self.files).spawn();
        }
        self.closing = true;
    }

    fn pump(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Work::Progress(f, n) => {
                    if let Phase::Running { progress, name, .. } = &mut self.phase {
                        *progress = f;
                        if !n.is_empty() {
                            *name = n;
                        }
                    }
                }
                Work::Done(result) => {
                    self.phase = Phase::Done(match result {
                        Ok(r) => Summary {
                            exit: r.exit_code(),
                            problems: r
                                .failures
                                .iter()
                                .map(|(p, e)| {
                                    (
                                        p.file_name()
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .unwrap_or_default(),
                                        self.t.t(e.code.i18n_key()),
                                    )
                                })
                                .collect(),
                            outputs: r.outputs,
                        },
                        Err(e) => Summary {
                            exit: 1,
                            outputs: vec![],
                            problems: vec![(String::new(), self.t.t(e.code.i18n_key()))],
                        },
                    });
                    self.opened = std::time::Instant::now();
                }
            }
        }
    }

    fn pick_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let rect = ui.max_rect();
        let center = rect.center();
        let painter = ui.painter();
        let resp = ui.interact(rect, ui.scope_id().with("wheel"), Sense::click());
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let n = SLOTS.len();
        self.hover = pointer.and_then(|p| sector_at(center, p, INNER, OUTER, n));
        let dark = ui.visuals().dark_mode;
        let (base, hot, off) = if dark {
            (
                Color32::from_rgba_unmultiplied(34, 38, 54, 238),
                Color32::from_rgb(0x4a, 0x7d, 0xe8),
                Color32::from_rgba_unmultiplied(34, 38, 54, 150),
            )
        } else {
            (
                Color32::from_rgba_unmultiplied(250, 251, 253, 240),
                Color32::from_rgb(0x1f, 0x5f, 0xd6),
                Color32::from_rgba_unmultiplied(250, 251, 253, 150),
            )
        };
        let text = ui.visuals().strong_text_color();
        let step = TAU / n as f32;
        let gap = 0.012;
        // The ring.
        for (i, slot) in SLOTS.iter().enumerate() {
            let enabled = slot.enabled(&self.kinds);
            let a = i as f32 * step;
            let fill = if self.hover == Some(i) && enabled {
                hot
            } else if enabled {
                base
            } else {
                off
            };
            painter.add(sector_mesh(
                center,
                INNER + 4.0,
                OUTER,
                a - step / 2.0 + gap,
                a + step / 2.0 - gap,
                fill,
            ));
        }
        // Labels and the digit that selects each slot.
        for (i, slot) in SLOTS.iter().enumerate() {
            let enabled = slot.enabled(&self.kinds);
            let a = i as f32 * step;
            let mid = center + vec2(a.sin(), -a.cos()) * (INNER + (OUTER - INNER) * 0.55);
            let color = if self.hover == Some(i) && enabled {
                Color32::WHITE
            } else if enabled {
                text
            } else {
                text.gamma_multiply(0.4)
            };
            let label = match slot {
                Slot::Quick(act) => self.t.t(&format!("tool.{}.name", act.id())),
                Slot::Open => self.t.t("wheel.open"),
            };
            painter.text(mid, Align2::CENTER_CENTER, label, FontId::proportional(14.5), color);
            painter.text(
                mid + vec2(0.0, 17.0),
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                FontId::proportional(11.0),
                color.gamma_multiply(0.7),
            );
        }
        // The hub.
        painter.circle_filled(center, INNER, base);
        painter.circle_stroke(center, INNER, Stroke::new(1.0, text.gamma_multiply(0.25)));
        let summary = if self.files.is_empty() {
            String::new()
        } else {
            self.t.tn("wheel.files", self.files.len() as i64, &[])
        };
        painter.text(
            center + vec2(0.0, -8.0),
            Align2::CENTER_CENTER,
            summary,
            FontId::proportional(15.0),
            text,
        );
        painter.text(
            center + vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            "Sheaf",
            FontId::proportional(11.0),
            text.gamma_multiply(0.55),
        );
        if self.files.is_empty() {
            let r = Rect::from_center_size(center + vec2(0.0, OUTER + 18.0), vec2(2.0 * OUTER, 30.0));
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                self.t.t("wheel.none"),
                FontId::proportional(13.0),
                Color32::WHITE,
            );
        }

        // Choose.
        let mut chosen = None;
        if resp.clicked() {
            chosen = self.hover;
        }
        ctx.input(|i| {
            for (k, key) in [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
            ]
            .iter()
            .enumerate()
            {
                if i.key_pressed(*key) {
                    chosen = Some(k);
                }
            }
        });
        if let Some(i) = chosen {
            match SLOTS[i] {
                Slot::Open => self.open_in_main(),
                Slot::Quick(a) if a.applies_to(&self.kinds) && !self.files.is_empty() => self.start(ctx, a),
                Slot::Quick(_) => {}
            }
        } else if resp.clicked() && pointer.is_some_and(|p| (p - center).length() > OUTER) {
            self.closing = true;
        }
    }

    fn card_ui(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        let mut reveal: Option<PathBuf> = None;
        let mut open_result: Option<Vec<PathBuf>> = None;
        let t = &self.t;
        egui::Frame::popup(ui.style())
            .corner_radius(10.0)
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_min_width(340.0);
                match &self.phase {
                    Phase::Running { action, name, progress } => {
                        ui.strong(t.t(&format!("tool.{}.name", action.id())));
                        ui.label(t.tv("wheel.running", &[("name", name)]));
                        ui.add(egui::ProgressBar::new(*progress).desired_width(f32::INFINITY));
                    }
                    Phase::Done(s) => {
                        let title = match s.exit {
                            0 => t.t("wheel.done"),
                            2 => t.t("wheel.partial"),
                            _ => t.t("wheel.failed"),
                        };
                        let color = match s.exit {
                            0 => Color32::from_rgb(0x2e, 0x9e, 0x5b),
                            2 => Color32::from_rgb(0xc9, 0x8a, 0x00),
                            _ => ui.visuals().error_fg_color,
                        };
                        ui.label(egui::RichText::new(title).strong().color(color));
                        if !s.outputs.is_empty() {
                            ui.label(t.tn("wheel.made", s.outputs.len() as i64, &[]));
                            for o in s.outputs.iter().take(4) {
                                ui.label(
                                    egui::RichText::new(
                                        o.file_name()
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .unwrap_or_default(),
                                    )
                                    .weak(),
                                );
                            }
                        }
                        for (name, why) in s.problems.iter().take(4) {
                            ui.label(
                                egui::RichText::new(t.tv("wheel.problem", &[("name", name), ("why", why)]))
                                    .color(ui.visuals().error_fg_color)
                                    .small(),
                            );
                        }
                        ui.horizontal(|ui| {
                            if let Some(first) = s.outputs.first() {
                                if ui.button(t.t("wheel.show")).clicked() {
                                    reveal = Some(first.clone());
                                }
                                if first.is_file() && ui.button(t.t("wheel.open_result")).clicked() {
                                    open_result = Some(s.outputs.iter().filter(|p| p.is_file()).cloned().collect());
                                }
                            }
                            if ui.button(t.t("wheel.close")).clicked() {
                                close = true;
                            }
                        });
                    }
                    Phase::Pick => {}
                }
            });
        if let Some(p) = reveal {
            let _ = crate::platform::reveal(&p);
            close = true;
        }
        if let Some(files) = open_result {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("open").args(files).spawn();
            }
            close = true;
        }
        if close {
            self.closing = true;
        }
    }
}

impl eframe::App for WheelApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump();
        if !self.placed {
            self.placed = true;
            // The window was created at an approximate place; the exact one is known now.
            let (x, y) = self.center_pt;
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                x - OUTER - 8.0,
                y - OUTER - 8.0,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        // Closes when the person clicks elsewhere (focus lost after having had it), or leaves it alone.
        let focused = ctx.input(|i| i.focused);
        self.had_focus |= focused;
        let unfocused = self.had_focus && !focused && self.opened.elapsed() > std::time::Duration::from_millis(400);
        let idle = self.opened.elapsed() > std::time::Duration::from_secs(25);
        if escape || ((unfocused || idle) && matches!(self.phase, Phase::Pick)) || self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| match self.phase {
                Phase::Pick => self.pick_ui(ui, &ctx),
                _ => self.card_ui(ui),
            });
        if matches!(self.phase, Phase::Running { .. }) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if matches!(self.phase, Phase::Pick) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500)); // for the idle timeout
        }
        // A finished card goes away by itself after a while, unless it is being read.
        if matches!(self.phase, Phase::Done(_)) {
            let hovered = ctx.input(|i| i.pointer.hover_pos()).is_some();
            if !hovered && self.opened.elapsed() > std::time::Duration::from_secs(12) {
                self.closing = true;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

/// Show the wheel at `config.at`; returns when it closes.
pub fn run_wheel(config: WheelConfig) -> eframe::Result {
    let size = 2.0 * OUTER + 16.0;
    let (x, y) = (config.at.0 as f32, config.at.1 as f32);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sheaf")
            .with_inner_size([size, size])
            .with_position([x - size / 2.0, y - size / 2.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Sheaf wheel",
        options,
        Box::new(|cc| Ok(Box::new(WheelApp::new(cc, config)))),
    )
}

/// Read the file list the helper left for the overlay: one path per line, then delete the file.
pub fn take_file_list(list: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(list).unwrap_or_default();
    let _ = std::fs::remove_file(list);
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::f32::consts::PI;
    const C: Pos2 = pos2(200.0, 200.0);

    fn at(angle_deg: f32, r: f32) -> Pos2 {
        let a = angle_deg * PI / 180.0;
        C + vec2(a.sin(), -a.cos()) * r
    }

    #[test]
    fn sectors_count_clockwise_from_the_top() {
        for (deg, want) in [
            (0.0, 0),
            (45.0, 1),
            (90.0, 2),
            (135.0, 3),
            (180.0, 4),
            (225.0, 5),
            (270.0, 6),
            (315.0, 7),
            (359.0, 0),
            (20.0, 0),
            (25.0, 1),
        ] {
            assert_eq!(sector_at(C, at(deg, 120.0), INNER, OUTER, 8), Some(want), "{deg}°");
        }
    }

    #[test]
    fn the_hub_and_the_outside_are_not_sectors() {
        assert_eq!(sector_at(C, C, INNER, OUTER, 8), None);
        assert_eq!(sector_at(C, at(90.0, INNER - 1.0), INNER, OUTER, 8), None);
        assert_eq!(sector_at(C, at(90.0, OUTER + 1.0), INNER, OUTER, 8), None);
        assert_eq!(sector_at(C, at(90.0, 100.0), INNER, OUTER, 0), None);
    }

    #[test]
    fn slots_follow_the_files_that_are_selected() {
        use FileKind::*;
        let enabled = |k: &[FileKind]| SLOTS.map(|s| s.enabled(k));
        // Two PDFs: merge, compress, ocr, split, to-images, rotate and open; not images-to-pdf.
        assert_eq!(enabled(&[Pdf, Pdf]), [true, true, true, true, true, false, true, true]);
        // One PDF: no merge.
        assert_eq!(enabled(&[Pdf]), [false, true, true, true, true, false, true, true]);
        // Photos: merge, ocr, images-to-pdf, rotate, open.
        assert_eq!(
            enabled(&[Image, Image]),
            [true, false, true, false, false, true, true, true]
        );
        // Nothing useful: only "open" stays.
        assert_eq!(
            enabled(&[Other]),
            [false, false, false, false, false, false, false, true]
        );
        // Every quick action has a slot.
        for a in QuickAction::ALL {
            assert!(SLOTS.contains(&Slot::Quick(a)), "{a:?}");
        }
    }

    #[test]
    fn a_ring_sector_is_a_valid_mesh() {
        let m = sector_mesh(C, INNER, OUTER, -0.3, 0.3, Color32::WHITE);
        assert!(!m.indices.is_empty() && m.indices.iter().all(|i| (*i as usize) < m.vertices.len()));
        assert_eq!(m.indices.len() % 3, 0);
    }

    #[test]
    fn the_file_list_is_read_once_and_removed() {
        let dir = std::env::temp_dir().join(format!("sheaf-wheel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let list = dir.join("list.txt");
        std::fs::write(&list, "C:\\a.pdf\n\n  D:\\b c.pdf \n").expect("write");
        assert_eq!(
            take_file_list(&list),
            [PathBuf::from("C:\\a.pdf"), PathBuf::from("D:\\b c.pdf")]
        );
        assert!(!list.exists());
        assert!(take_file_list(&list).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
