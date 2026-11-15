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
use sheaf_core::{classify, AppError, ErrorCode, FileKind, QuickAction, QuickRequest};
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

pub const OUTER: f32 = 200.0;
pub const INNER: f32 = 66.0;
/// A flyout (the options of a slot) opens beside the ring, a little apart from it, which is what the
/// safe triangle ([`crate::aim`]) is for.
const FLY_GAP: f32 = 14.0;
const FLY_W: f32 = 176.0;
const ITEM_H: f32 = 36.0;
/// The window holds the ring and a flyout on either side.
const WIN_W: f32 = 2.0 * (OUTER + FLY_GAP + FLY_W) + 16.0;
const WIN_H: f32 = 2.0 * OUTER + 16.0;
/// Bounds on the file list handed over by the tray helper.
const MAX_LIST_BYTES: u64 = 1 << 20;
const MAX_LIST_FILES: usize = 2_000;
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

/// One choice in a slot's flyout.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Level(sheaf_core::CompressLevel),
    /// OCR languages; `None` detects them.
    Langs(Option<Vec<String>>),
}

/// The options a slot offers, as (translation key or plain text, choice). A key starts with `@`.
pub fn options_of(slot: Slot) -> Vec<(&'static str, Choice)> {
    use sheaf_core::CompressLevel::{Extreme, Low, Recommended};
    let langs = |codes: &[&str]| Choice::Langs(Some(codes.iter().map(|c| c.to_string()).collect()));
    match slot {
        Slot::Quick(QuickAction::Compress) => vec![
            ("@presets.compress_Low", Choice::Level(Low)),
            ("@presets.compress_Recommended", Choice::Level(Recommended)),
            ("@presets.compress_Extreme", Choice::Level(Extreme)),
        ],
        Slot::Quick(QuickAction::Ocr) => vec![
            ("@wheel.lang_auto", Choice::Langs(None)),
            ("English", langs(&["eng"])),
            ("Русский", langs(&["rus"])),
            ("Oʻzbekcha", langs(&["uzb"])),
            ("Ўзбекча", langs(&["uzb_cyrl"])),
        ],
        _ => Vec::new(),
    }
}

/// Where the flyout of slot `i` goes: one rectangle per option, and the edge facing the ring (its
/// two corners), which is the far side of the safe triangle. Right-hand slots open to the right,
/// left-hand slots to the left; the list is centred on the slot and kept inside the window.
pub fn flyout_layout(center: Pos2, i: usize, n_slots: usize, options: usize) -> (Vec<Rect>, (Pos2, Pos2)) {
    let a = i as f32 * TAU / n_slots as f32;
    let dir = vec2(a.sin(), -a.cos());
    let anchor = center + dir * OUTER;
    let right = dir.x >= 0.0;
    let total = options as f32 * ITEM_H;
    let top = (anchor.y - total / 2.0).clamp(center.y - WIN_H / 2.0 + 4.0, center.y + WIN_H / 2.0 - 4.0 - total);
    // Clear of the ring over the whole height of the list: the ring is widest at the row of the list
    // that is nearest the centre, so the gap starts beyond that.
    let (dy_top, dy_bottom) = (top - center.y, top + total - center.y);
    let nearest = if dy_top <= 0.0 && dy_bottom >= 0.0 {
        0.0
    } else {
        dy_top.abs().min(dy_bottom.abs())
    };
    let reach = (OUTER * OUTER - nearest * nearest).max(0.0).sqrt();
    let near_x = if right {
        center.x + reach + FLY_GAP
    } else {
        center.x - reach - FLY_GAP
    };
    let x0 = if right { near_x } else { near_x - FLY_W };
    let rects = (0..options)
        .map(|k| Rect::from_min_size(pos2(x0, top + k as f32 * ITEM_H), vec2(FLY_W, ITEM_H - 3.0)))
        .collect();
    (rects, (pos2(near_x, top), pos2(near_x, top + total)))
}

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
    /// Work to start at once, with no wheel to choose from: the card shows its progress and
    /// result at `at` (an Explorer verb, FLOW-EXPLORER).
    pub job: Option<WheelJob>,
}

/// What the card runs when there is nothing to choose.
pub enum WheelJob {
    Quick(QuickRequest),
    /// A saved preset (PRESET-FORMAT) on files.
    Preset {
        name: String,
        files: Vec<PathBuf>,
        out: Option<PathBuf>,
    },
}

impl WheelJob {
    pub fn files(&self) -> &[PathBuf] {
        match self {
            WheelJob::Quick(r) => &r.files,
            WheelJob::Preset { files, .. } => files,
        }
    }
}

enum Phase {
    Pick,
    Running {
        /// What is being done, in words.
        title: String,
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
    /// Which slot's options are showing, decided with the safe triangle.
    aim: crate::aim::Aim,
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
            aim: Default::default(),
        }
    }

    fn start(&mut self, ctx: &egui::Context, job: WheelJob) {
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
        let name = job
            .files()
            .first()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = match &job {
            WheelJob::Quick(r) => self.t.t(&format!("tool.{}.name", r.action.id())),
            WheelJob::Preset { name, .. } => self.t.tv("wheel.preset", &[("name", name)]),
        };
        self.phase = Phase::Running {
            title,
            name,
            progress: 0.0,
        };
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
                let progress = move |f: f32, name: &str| {
                    let _ = progress_tx.send(Work::Progress(f, name.to_string()));
                    progress_ctx.request_repaint();
                };
                let report = match &job {
                    WheelJob::Quick(request) => run_quick(&mut app, request, &progress),
                    WheelJob::Preset { name, files, out } => {
                        let dir = config
                            .config_dir
                            .as_deref()
                            .ok_or_else(|| AppError::new(ErrorCode::Unsupported, "presets need a config folder"))?;
                        sheaf_store::presets::load(dir, name).and_then(|preset| {
                            sheaf_app::preset::run_preset(&mut app, &preset, files, out.as_deref(), &progress, &|| {
                                false
                            })
                        })
                    }
                };
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
        let under = pointer.and_then(|p| sector_at(center, p, INNER, OUTER, n));
        // The options of the slot the pointer rests on, with the safe triangle between the slot
        // and its options so that crossing a neighbouring slot on the way does not close them.
        let layout = |i: usize| flyout_layout(center, i, n, options_of(SLOTS[i]).len());
        let open_layout = self.aim.open().map(layout);
        let over_flyout = pointer
            .zip(open_layout.as_ref())
            .is_some_and(|(p, (rects, _))| rects.iter().any(|r| r.contains(p)));
        let kinds = self.kinds.clone();
        let has_files = !self.files.is_empty();
        let offers = move |i: usize| SLOTS[i].enabled(&kinds) && has_files && !options_of(SLOTS[i]).is_empty();
        let open = self.aim.step(&crate::aim::Frame {
            time: ctx.input(|i| i.time),
            pointer,
            item: under,
            over_submenu: over_flyout,
            has_submenu: &offers,
            near_edge: open_layout.as_ref().map(|(_, edge)| *edge),
        });
        // While aiming, the slot that owns the options stays lit instead of the one being crossed.
        self.hover = if self.aim.aiming() || over_flyout { open } else { under };
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

        // The options of the open slot.
        let mut picked: Option<Choice> = None;
        if let Some(i) = open {
            let options = options_of(SLOTS[i]);
            let (rects, _) = flyout_layout(center, i, n, options.len());
            for ((label, choice), rect) in options.iter().zip(&rects) {
                let hot_item = pointer.is_some_and(|p| rect.contains(p));
                painter.rect_filled(*rect, 9.0, if hot_item { hot } else { base });
                painter.rect_stroke(
                    *rect,
                    9.0,
                    Stroke::new(1.0, text.gamma_multiply(0.18)),
                    egui::StrokeKind::Inside,
                );
                let words = match label.strip_prefix('@') {
                    Some(key) => self.t.t(key),
                    None => label.to_string(),
                };
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    words,
                    FontId::proportional(14.0),
                    if hot_item { Color32::WHITE } else { text },
                );
                if resp.clicked() && hot_item {
                    picked = Some(choice.clone());
                }
            }
        }
        // A small mark on the slots that have options, at the outer edge of their sector.
        for (i, slot) in SLOTS.iter().enumerate() {
            if options_of(*slot).is_empty() || !slot.enabled(&self.kinds) {
                continue;
            }
            let a = i as f32 * step;
            let dir = vec2(a.sin(), -a.cos());
            let tip = center + dir * (OUTER - 10.0);
            let side = vec2(-dir.y, dir.x);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tip + dir * 6.0,
                    tip - dir * 3.0 + side * 5.0,
                    tip - dir * 3.0 - side * 5.0,
                ],
                text.gamma_multiply(0.55),
                Stroke::NONE,
            ));
        }

        // Choose.
        let mut chosen = None;
        if resp.clicked() && picked.is_none() {
            chosen = self.hover;
        }
        if let (Some(choice), Some(i)) = (picked, open) {
            if let Slot::Quick(a) = SLOTS[i] {
                let mut request = QuickRequest::new(a, self.files.clone());
                match choice {
                    Choice::Level(level) => request.level = level,
                    Choice::Langs(langs) => request.langs = langs,
                }
                self.start(ctx, WheelJob::Quick(request));
                return;
            }
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
                Slot::Quick(a) if a.applies_to(&self.kinds) && !self.files.is_empty() => {
                    self.start(ctx, WheelJob::Quick(QuickRequest::new(a, self.files.clone())))
                }
                Slot::Quick(_) => {}
            }
        } else if resp.clicked() && pointer.is_some_and(|p| (p - center).length() > OUTER) && !over_flyout {
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
                    Phase::Running { title, name, progress } => {
                        ui.strong(title);
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
                x - WIN_W / 2.0,
                y - WIN_H / 2.0,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            // An Explorer verb names its work: no wheel to choose from, straight to the card.
            if let Some(job) = self.config.as_mut().and_then(|c| c.job.take()) {
                self.start(&ctx, job);
            }
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
    // A verb's job goes straight to the card, so the window starts card-sized.
    let (w, h) = if config.job.is_some() {
        (CARD_W, CARD_H)
    } else {
        (WIN_W, WIN_H)
    };
    let (x, y) = (config.at.0 as f32, config.at.1 as f32);
    let options = crate::renderer::options(
        egui::ViewportBuilder::default()
            .with_title("Sheaf")
            .with_inner_size([w, h])
            .with_position([x - w / 2.0, y - h / 2.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false),
        true,
    );
    eframe::run_native(
        "Sheaf wheel",
        options,
        Box::new(|cc| Ok(Box::new(WheelApp::new(cc, config)))),
    )
}

/// Read the file list the helper left for the overlay: one path per line, then delete the file.
pub fn take_file_list(list: &Path) -> Vec<PathBuf> {
    // The list is a private file the helper just wrote, but it is read like any input: bounded.
    let small = std::fs::metadata(list).is_ok_and(|m| m.is_file() && m.len() <= MAX_LIST_BYTES);
    let text = if small {
        std::fs::read_to_string(list).unwrap_or_default()
    } else {
        String::new()
    };
    let _ = std::fs::remove_file(list);
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.chars().count() <= 4_096)
        .take(MAX_LIST_FILES)
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

    #[test]
    fn flyouts_sit_clear_of_the_ring_inside_the_window_and_open_outward() {
        let window = Rect::from_center_size(C, vec2(WIN_W, WIN_H));
        for (i, slot) in SLOTS.iter().enumerate() {
            let options = options_of(*slot);
            if options.is_empty() {
                continue;
            }
            let (rects, (a, b)) = flyout_layout(C, i, SLOTS.len(), options.len());
            assert_eq!(rects.len(), options.len());
            for r in &rects {
                assert!(window.contains_rect(*r), "slot {i}: {r:?} is outside the window");
                // No corner of any item is inside the ring, and the nearest is at least the gap away.
                for corner in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                    assert!(
                        (corner - C).length() >= OUTER + FLY_GAP - 0.5,
                        "slot {i}: {corner:?} touches the ring"
                    );
                }
            }
            // The near edge is the side that faces the wheel, and spans the whole list.
            assert_eq!(a.x, b.x);
            let facing = if slot_dir(i).x >= 0.0 {
                rects[0].left()
            } else {
                rects[0].right()
            };
            assert!((facing - a.x).abs() < 0.01, "slot {i}");
            assert!(a.y <= rects[0].top() + 0.01 && b.y >= rects.last().expect("items").bottom());
            // Items do not overlap each other.
            for w in rects.windows(2) {
                assert!(w[0].bottom() <= w[1].top());
            }
        }
    }

    fn slot_dir(i: usize) -> egui::Vec2 {
        let a = i as f32 * TAU / SLOTS.len() as f32;
        vec2(a.sin(), -a.cos())
    }

    #[test]
    fn only_compress_and_ocr_have_options_and_each_choice_means_something() {
        let with: Vec<usize> = SLOTS
            .iter()
            .enumerate()
            .filter(|(_, s)| !options_of(**s).is_empty())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(with, [1, 2], "Compress and OCR");
        let compress = options_of(SLOTS[1]);
        assert_eq!(compress.len(), 3);
        assert!(compress.iter().all(|(_, c)| matches!(c, Choice::Level(_))));
        let ocr = options_of(SLOTS[2]);
        assert!(
            matches!(ocr[0].1, Choice::Langs(None)),
            "the first option detects the language"
        );
        // Every language named is one whose data is bundled (vendor/tessdata).
        for (_, c) in &ocr[1..] {
            let Choice::Langs(Some(codes)) = c else { panic!("{c:?}") };
            assert!(
                codes
                    .iter()
                    .all(|l| ["eng", "rus", "uzb", "uzb_cyrl"].contains(&l.as_str())),
                "{codes:?}"
            );
        }
    }
}
