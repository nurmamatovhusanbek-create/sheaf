//! The DOC-IPC command layer, without a window toolkit.
//!
//! [`Backend`] is what MOD-SHELL's Tauri commands call. It treats everything
//! that comes from the UI as hostile (SEC-IPC): ids are looked up, ranges are
//! capped, enums are closed, free text never becomes a path (SEC-PATHS), and
//! no failure panics (DOC-AGENT-RULES rule 4). It never holds the workspace
//! lock while a worker runs, so the tray keeps scrolling during an export.

use crate::engines::verify_vendor;
use crate::jobs::{Ctx, JobEntry, JobOutput, JobView, Registry};
use crate::plan::{CancelFlag, JobCtl, NoCtl};
use crate::{page_version, App, Engines, ExportSpec, OcrOutcome, Shared};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sheaf_core::consts::{MAX_SOURCE_PAGES, MAX_WORKSPACE_PAGES, THUMB_PX};
use sheaf_core::proto::ImageFormat;
use sheaf_core::{
    AppError, CompressLevel, ErrorCode, Filter, ImageFit, NormRect, OcrResult, OcrState, Op, Page, PageContent, PageId,
    Password, Protect, Rot, Source, SourceId, SourceKind, SplitRule, Stamp,
};
use sheaf_ocr::Hit;
use sheaf_store::Settings;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

type Result<T> = sheaf_core::Result<T>;

/// Most pages one `pages` call returns.
pub const MAX_PAGE_RANGE: usize = 2_000;
/// Preview and thumbnail sizes the renderer will produce, longest side in pixels.
pub const RENDER_PX: std::ops::RangeInclusive<u32> = 32..=4_096;
/// Longest name pattern or folder name accepted from the UI.
const MAX_NAME_CHARS: usize = 200;

// ---------------------------------------------------------------------
// What goes out: DTOs and events
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDto {
    pub code: ErrorCode,
    /// Key in `ui/i18n/*.json` (FLOW-ERROR).
    pub key: &'static str,
    /// Developer-facing detail without document content or passwords (SEC-LOG).
    pub msg: String,
}

impl From<&AppError> for ErrorDto {
    fn from(e: &AppError) -> Self {
        ErrorDto {
            code: e.code,
            key: e.code.i18n_key(),
            msg: e.msg.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrDto {
    /// `none`, `queued`, `running`, `done`, `skipped` or `failed`.
    pub state: &'static str,
    pub langs: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDto {
    pub id: u64,
    /// `source` or `blank`.
    pub kind: &'static str,
    pub source: Option<u32>,
    pub src_index: u32,
    pub rot: u16,
    pub crop: Option<NormRect>,
    pub filter: Filter,
    pub ocr: OcrDto,
    pub has_text: Option<bool>,
    /// Fingerprint of everything that changes how the page looks; goes into
    /// the thumbnail URL so an edit refreshes that page's picture only.
    pub ver: String,
}

impl From<&Page> for PageDto {
    fn from(p: &Page) -> Self {
        let (kind, source, src_index) = match p.content {
            PageContent::Source { src, index } => ("source", Some(src.0), index),
            PageContent::Blank { .. } => ("blank", None, 0),
        };
        let ocr = match &p.ocr {
            OcrState::None => OcrDto {
                state: "none",
                langs: vec![],
                reason: None,
            },
            OcrState::Queued => OcrDto {
                state: "queued",
                langs: vec![],
                reason: None,
            },
            OcrState::Running => OcrDto {
                state: "running",
                langs: vec![],
                reason: None,
            },
            OcrState::Done { langs, .. } => OcrDto {
                state: "done",
                langs: langs.clone(),
                reason: None,
            },
            OcrState::SkippedHasText => OcrDto {
                state: "skipped",
                langs: vec![],
                reason: None,
            },
            OcrState::Failed { reason } => OcrDto {
                state: "failed",
                langs: vec![],
                reason: Some(reason.clone()),
            },
        };
        PageDto {
            id: p.id.0,
            kind,
            source,
            src_index,
            rot: p.rot.degrees(),
            crop: p.crop,
            filter: p.filter,
            ocr,
            has_text: p.has_text,
            ver: format!("{:016x}", page_version(p)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesDto {
    pub pages: Vec<PageDto>,
    pub start: usize,
    pub total: usize,
    pub rev: u64,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDto {
    pub id: u32,
    /// File name only (SEC-LOG: no directories in what the UI sees).
    pub name: String,
    pub kind: &'static str,
    pub pages: u32,
    pub encrypted: bool,
    pub unlocked: bool,
}

impl From<&Source> for SourceDto {
    fn from(s: &Source) -> Self {
        SourceDto {
            id: s.id.0,
            name: file_name(&s.path),
            kind: match s.kind {
                SourceKind::Pdf => "pdf",
                SourceKind::Image => "image",
            },
            pages: s.page_count,
            encrypted: s.encrypted,
            unlocked: s.unlocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineDto {
    /// Bundled files passed their hash check (SEC-SUPPLY); `None` while it runs.
    pub verified: Option<bool>,
    pub tesseract: bool,
    pub font: bool,
    pub error: Option<ErrorDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    pub rev: u64,
    pub total: usize,
    pub can_undo: bool,
    pub can_redo: bool,
    pub sources: Vec<SourceDto>,
    pub settings: SettingsDto,
    pub engines: EngineDto,
    pub version: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub language: String,
    /// Display name of the chosen output folder, if any (never edited as text).
    pub output_dir: Option<String>,
    pub default_preset: Option<String>,
    pub workers: Option<usize>,
    pub ocr_quality: String,
    pub ocr_mode: String,
    pub theme: String,
}

impl From<&Settings> for SettingsDto {
    fn from(s: &Settings) -> Self {
        SettingsDto {
            language: s.language.clone(),
            output_dir: s.output_dir.as_ref().map(|d| d.display().to_string()),
            default_preset: s.default_preset.clone(),
            workers: s.workers,
            ocr_quality: s.ocr_quality.clone(),
            ocr_mode: s.ocr_mode.clone(),
            theme: s.theme.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReportDto {
    pub source: u32,
    pub name: String,
    pub pages: Vec<u64>,
    pub encrypted: bool,
    pub restricted: bool,
    pub has_forms: bool,
    pub is_unc: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlockDto {
    pub ok: bool,
    pub pages: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickDto {
    pub token: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub page: u64,
    pub word: usize,
    pub snippet: String,
}

impl From<Hit> for HitDto {
    fn from(h: Hit) -> Self {
        HitDto {
            page: h.page.0,
            word: h.word,
            snippet: h.snippet,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesEvent {
    pub rev: u64,
    pub total: usize,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageReadyEvent {
    pub page: PageDto,
    pub rev: u64,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// DOC-IPC events (EVT-*).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// EVT-PAGES: pages added, removed or changed.
    Pages(PagesEvent),
    /// EVT-PAGE-READY: OCR state (or an orientation fix) changed for one page.
    PageReady(PageReadyEvent),
    /// EVT-JOB: progress, done, failed, cancelled.
    Job(JobView),
    /// EVT-IMPORT: files came in through a dialog, a drop or the command line.
    Import(Vec<ImportReportDto>),
    /// EVT-ERROR: a user-presentable error.
    Error(ErrorDto),
    /// EVT-ENGINES: the startup hash check finished.
    Engines(EngineDto),
}

impl Event {
    /// Event name for the transport.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Pages(_) => "pages",
            Event::PageReady(_) => "page-ready",
            Event::Job(_) => "job",
            Event::Import(_) => "import",
            Event::Error(_) => "error",
            Event::Engines(_) => "engines",
        }
    }

    pub fn payload(&self) -> Value {
        let v = match self {
            Event::Pages(e) => serde_json::to_value(e),
            Event::PageReady(e) => serde_json::to_value(e),
            Event::Job(e) => serde_json::to_value(e),
            Event::Import(e) => serde_json::to_value(e),
            Event::Error(e) => serde_json::to_value(e),
            Event::Engines(e) => serde_json::to_value(e),
        };
        v.unwrap_or(Value::Null)
    }
}

/// Where events go: the Tauri shell emits them to the window, tests collect them.
pub trait Emit: Send + Sync {
    fn emit(&self, event: Event);
}

/// An [`Emit`] that drops everything.
pub struct NoEmit;
impl Emit for NoEmit {
    fn emit(&self, _event: Event) {}
}

// ---------------------------------------------------------------------
// What comes in: requests from the UI
// ---------------------------------------------------------------------

/// CMD-APPLY. Deliberately has no `Insert`: pages enter the workspace only
/// through import, never by naming a source and an index.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum UiOp {
    Delete {
        ids: Vec<u64>,
    },
    Move {
        ids: Vec<u64>,
        to: usize,
    },
    Rotate {
        ids: Vec<u64>,
        deg: i32,
    },
    Crop {
        ids: Vec<u64>,
        rect: Option<NormRect>,
    },
    SetFilter {
        ids: Vec<u64>,
        filter: Filter,
    },
    Duplicate {
        ids: Vec<u64>,
    },
    /// `size` in points; A4 when absent.
    InsertBlank {
        at: usize,
        size: Option<(f32, f32)>,
    },
}

/// CMD-OCR-RUN: DATA-OCRPOLICY as far as one run is concerned.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OcrPolicyDto {
    /// `add` (default: pages that have text are skipped), `redo` (replace
    /// existing text) or `text` (recognize what is missing, then write a `.txt`).
    pub mode: String,
    /// `None` or an empty list detects script and picks languages (OCR-LANGS).
    pub langs: Option<Vec<String>>,
}

impl Default for OcrPolicyDto {
    fn default() -> Self {
        OcrPolicyDto {
            mode: "add".into(),
            langs: None,
        }
    }
}

/// CMD-EXPORT: DATA-EXPORTSPEC. The output folder is not part of it: it is
/// the one chosen through a dialog (CMD-SETTINGS) or the source's own folder.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportSpecDto {
    /// `pdf`, `images` or `text`.
    pub format: String,
    /// Page ids in the selection; `None` is every page.
    pub pages: Option<Vec<u64>>,
    pub split: SplitRule,
    /// DATA-NAMEPATTERN; empty picks a default from the source name.
    pub name: String,
    pub compress: Option<CompressLevel>,
    pub protect: Option<Protect>,
    pub stamps: Vec<Stamp>,
    pub image_fit: ImageFit,
    pub flatten_forms: bool,
    /// Crops on PDF pages hide instead of remove (TOOL-CROP "hide only").
    pub crop_hide_only: bool,
    /// For `images`: `png` or `jpg`.
    pub image_format: String,
    pub dpi: u32,
}

impl Default for ExportSpecDto {
    fn default() -> Self {
        ExportSpecDto {
            format: "pdf".into(),
            pages: None,
            split: SplitRule::None,
            name: String::new(),
            compress: None,
            protect: None,
            stamps: Vec::new(),
            image_fit: ImageFit::default(),
            flatten_forms: false,
            crop_hide_only: false,
            image_format: "png".into(),
            dpi: 150,
        }
    }
}

/// CMD-SETTINGS set. Absent fields stay as they are.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatch {
    pub language: Option<String>,
    pub theme: Option<String>,
    pub ocr_quality: Option<String>,
    pub ocr_mode: Option<String>,
    /// 0 means "cores minus one". Takes effect at the next start.
    pub workers: Option<usize>,
    /// Empty string clears it.
    pub default_preset: Option<String>,
}

// ---------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------

pub struct BackendConfig {
    pub engines: Engines,
    /// Where session dirs live (`%LOCALAPPDATA%\Sheaf`).
    pub base_dir: PathBuf,
    /// Where `settings.json` lives (`%APPDATA%\Sheaf`); `None` keeps settings in memory only.
    pub config_dir: Option<PathBuf>,
    /// Check the vendored files' hashes at startup (SEC-SUPPLY).
    pub verify_vendor: bool,
}

pub struct Backend {
    app: Mutex<Option<App>>,
    shared: Arc<Shared>,
    emit: Arc<dyn Emit>,
    jobs: Registry,
    settings: Mutex<Settings>,
    config_dir: Option<PathBuf>,
    out_dir: Mutex<Option<PathBuf>>,
    picks: Mutex<BTreeMap<String, PathBuf>>,
    pick_counter: std::sync::atomic::AtomicU64,
    tesseract: bool,
    font: bool,
    /// `None` until the startup check finishes, then its outcome.
    verified: (Mutex<Option<std::result::Result<(), AppError>>>, Condvar),
    closing: CancelFlag,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in one command must not wedge the app.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn page_ids(ids: &[u64]) -> Result<Vec<PageId>> {
    if ids.len() > MAX_WORKSPACE_PAGES {
        return Err(AppError::limit("too many pages in one request"));
    }
    Ok(ids.iter().map(|&i| PageId(i)).collect())
}

/// A name pattern or folder name from the UI: bounded, no NUL. What is left
/// is made file-safe where it becomes a name (DATA-NAMEPATTERN, SEC-PATHS).
fn check_pattern(s: &str) -> Result<()> {
    if s.chars().count() > MAX_NAME_CHARS || s.contains('\0') {
        return Err(AppError::invalid("name pattern too long or invalid"));
    }
    Ok(())
}

impl Backend {
    /// Start the app core: sweep stale sessions, create ours, load settings,
    /// check the bundled files in the background (FLOW-LIFECYCLE).
    pub fn start(cfg: BackendConfig, emit: Arc<dyn Emit>) -> Result<Arc<Backend>> {
        let settings = cfg.config_dir.as_deref().map_or_else(Settings::default, Settings::load);
        let app = App::start(&cfg.engines, &cfg.base_dir, settings.worker_count())?;
        let shared = app.shared();
        let out_dir = settings
            .output_dir
            .as_deref()
            .and_then(|d| sheaf_store::check_output_dir(d).ok());
        let backend = Arc::new(Backend {
            app: Mutex::new(Some(app)),
            shared,
            emit,
            jobs: Registry::default(),
            settings: Mutex::new(settings),
            config_dir: cfg.config_dir,
            out_dir: Mutex::new(out_dir),
            picks: Mutex::new(BTreeMap::new()),
            pick_counter: Default::default(),
            tesseract: cfg.engines.tesseract.is_some(),
            font: cfg.engines.font.is_some(),
            verified: (Mutex::new(None), Condvar::new()),
            closing: CancelFlag::default(),
        });
        match (cfg.verify_vendor, cfg.engines.vendor_dir().map(Path::to_path_buf)) {
            (true, Some(vendor)) => {
                let me = Arc::clone(&backend);
                std::thread::Builder::new()
                    .name("verify-vendor".into())
                    .spawn(move || {
                        let outcome = verify_vendor(&vendor).map(|_| ());
                        me.finish_verification(outcome);
                    })
                    .map_err(|e| AppError::io("start hash check", &e))?;
            }
            _ => backend.finish_verification(Ok(())),
        }
        Ok(backend)
    }

    fn finish_verification(&self, outcome: std::result::Result<(), AppError>) {
        if let Err(e) = &outcome {
            self.emit.emit(Event::Error(ErrorDto::from(e)));
        }
        *lock(&self.verified.0) = Some(outcome);
        // Announce before anyone waiting in `ready` is let go.
        self.emit.emit(Event::Engines(self.engine_status()));
        self.verified.1.notify_all();
    }

    fn engine_status(&self) -> EngineDto {
        let state = lock(&self.verified.0).clone();
        EngineDto {
            verified: state.as_ref().map(Result::is_ok),
            tesseract: self.tesseract,
            font: self.font,
            error: state.and_then(Result::err).as_ref().map(ErrorDto::from),
        }
    }

    /// Wait for the startup hash check; a worker must not start before bundled
    /// files are known good, and a failed check blocks all document work.
    fn ready(&self) -> Result<()> {
        let mut g = lock(&self.verified.0);
        while g.is_none() {
            g = self.verified.1.wait(g).unwrap_or_else(|p| p.into_inner());
        }
        match g.as_ref() {
            Some(Err(e)) => Err(e.clone()),
            _ => Ok(()),
        }
    }

    /// Run `f` on the app core under the workspace lock. Keep `f` short: no worker jobs.
    fn with<R>(&self, f: impl FnOnce(&mut App) -> Result<R>) -> Result<R> {
        let mut g = lock(&self.app);
        match g.as_mut() {
            Some(app) => f(app),
            None => Err(AppError::new(ErrorCode::Cancelled, "the app is shutting down")),
        }
    }

    fn emit_error(&self, e: &AppError) {
        self.emit.emit(Event::Error(ErrorDto::from(e)));
    }

    fn pages_event(&self, app: &App) -> Event {
        Event::Pages(PagesEvent {
            rev: app.ws.rev(),
            total: app.ws.len(),
            can_undo: app.ws.can_undo(),
            can_redo: app.ws.can_redo(),
        })
    }

    fn emit_job(&self, entry: &JobEntry) {
        self.emit.emit(Event::Job(entry.view()));
    }

    // -- state and pages -------------------------------------------------

    /// Everything the UI needs when it starts or reconnects.
    pub fn snapshot(&self) -> Result<SnapshotDto> {
        let (rev, total, can_undo, can_redo, sources) = self.with(|app| {
            Ok((
                app.ws.rev(),
                app.ws.len(),
                app.ws.can_undo(),
                app.ws.can_redo(),
                app.ws.sources().map(SourceDto::from).collect(),
            ))
        })?;
        Ok(SnapshotDto {
            rev,
            total,
            can_undo,
            can_redo,
            sources,
            settings: SettingsDto::from(&*lock(&self.settings)),
            engines: self.engine_status(),
            version: env!("CARGO_PKG_VERSION"),
        })
    }

    /// CMD-PAGES.
    pub fn pages(&self, start: usize, count: usize) -> Result<PagesDto> {
        let count = count.min(MAX_PAGE_RANGE);
        self.with(|app| {
            let total = app.ws.len();
            let start = start.min(total);
            Ok(PagesDto {
                pages: app
                    .ws
                    .pages(start..start.saturating_add(count))
                    .iter()
                    .map(PageDto::from)
                    .collect(),
                start,
                total,
                rev: app.ws.rev(),
                can_undo: app.ws.can_undo(),
                can_redo: app.ws.can_redo(),
            })
        })
    }

    /// CMD-APPLY.
    pub fn apply(&self, op: UiOp) -> Result<u64> {
        let (core_op, touched_pixels, deleted) = match op {
            UiOp::Delete { ids } => {
                let ids = page_ids(&ids)?;
                (Op::Delete { ids: ids.clone() }, Vec::new(), ids)
            }
            UiOp::Move { ids, to } => (
                Op::Move {
                    ids: page_ids(&ids)?,
                    to,
                },
                Vec::new(),
                Vec::new(),
            ),
            UiOp::Rotate { ids, deg } => (
                Op::Rotate {
                    ids: page_ids(&ids)?,
                    delta: Rot::from_degrees(deg)?,
                },
                Vec::new(),
                Vec::new(),
            ),
            UiOp::Crop { ids, rect } => {
                let ids = page_ids(&ids)?;
                (Op::Crop { ids: ids.clone(), rect }, ids, Vec::new())
            }
            UiOp::SetFilter { ids, filter } => {
                let ids = page_ids(&ids)?;
                (
                    Op::SetFilter {
                        ids: ids.clone(),
                        filter,
                    },
                    ids,
                    Vec::new(),
                )
            }
            UiOp::Duplicate { ids } => (Op::Duplicate { ids: page_ids(&ids)? }, Vec::new(), Vec::new()),
            UiOp::InsertBlank { at, size } => (
                Op::InsertBlank {
                    at,
                    size: size.unwrap_or((595.0, 842.0)),
                },
                Vec::new(),
                Vec::new(),
            ),
        };
        let (rev, event) = self.with(|app| {
            let rev = app.ws.apply(core_op)?;
            for id in &deleted {
                app.forget_page(*id);
            }
            // Recognized text describes the pixels it was read from (FLOW-EDIT).
            for id in &touched_pixels {
                if matches!(app.ws.find(*id).map(|p| &p.ocr), Some(OcrState::Done { .. })) {
                    app.ws.set_ocr_state(*id, OcrState::None)?;
                    app.forget_page(*id);
                }
            }
            Ok((rev, self.pages_event(app)))
        })?;
        self.emit.emit(event);
        Ok(rev)
    }

    /// CMD-UNDO. `None` when there was nothing to undo or redo.
    pub fn undo(&self, redo: bool) -> Result<Option<u64>> {
        let (rev, event) = self.with(|app| {
            let rev = if redo { app.ws.redo() } else { app.ws.undo() };
            Ok((rev, self.pages_event(app)))
        })?;
        if rev.is_some() {
            self.emit.emit(event);
        }
        Ok(rev)
    }

    /// CMD-RENDER: the key of a preview image, `<pageId>/<ver>/<px>`, which the
    /// shell serves as `sheaf-thumb://…` (CMD-THUMB). `zoom` 1.0 is 1000 px on
    /// the longest side.
    pub fn render_key(&self, page: u64, zoom: f32) -> Result<String> {
        if !zoom.is_finite() || zoom <= 0.0 {
            return Err(AppError::invalid("zoom out of range"));
        }
        let px = ((zoom * 1000.0).round() as u32).clamp(THUMB_PX + 1, *RENDER_PX.end());
        self.with(|app| {
            let p = app.ws.find(PageId(page)).ok_or_else(|| AppError::not_found("page"))?;
            Ok(format!("{page}/{:016x}/{px}", page_version(p)))
        })
    }

    /// CMD-THUMB: PNG bytes of a page. Sizes up to [`THUMB_PX`] are thumbnails,
    /// larger ones previews. Renders in a worker unless already cached.
    pub fn thumb(&self, page: u64, size: u32) -> Result<Vec<u8>> {
        self.ready()?;
        if !RENDER_PX.contains(&size) {
            return Err(AppError::invalid("image size out of range"));
        }
        let (spec, rel) = self.with(|app| app.render_job(PageId(page), size))?;
        let path = self.shared.session.resolve(&rel)?;
        if !path.is_file() {
            self.shared.run(spec, &NoCtl)?;
        }
        std::fs::read(&path).map_err(|e| AppError::io("read rendered page", &e))
    }

    // -- import ----------------------------------------------------------

    /// CMD-IMPORT / FLOW-IMPORT. The paths come from a dialog, a drop or the
    /// command line, never from UI text. Each file is looked at in a worker
    /// without the workspace lock; one bad file does not stop the others.
    pub fn import(self: &Arc<Self>, paths: Vec<PathBuf>) -> Result<Vec<ImportReportDto>> {
        self.ready()?;
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let entry = self.jobs.create("import");
        entry.set_running();
        self.emit_job(&entry);
        let mut reports = Vec::new();
        let mut errors: Vec<AppError> = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let one = App::begin_import(&self.shared, path).and_then(|probed| {
                if probed.page_count() > MAX_SOURCE_PAGES {
                    return Err(AppError::limit("file has too many pages"));
                }
                self.with(|app| {
                    let r = app.commit_import(probed)?;
                    let name = app.ws.source(r.source).map(|s| file_name(&s.path)).unwrap_or_default();
                    Ok((r, name, self.pages_event(app)))
                })
            });
            match one {
                Ok((r, name, event)) => {
                    self.emit.emit(event);
                    reports.push(ImportReportDto {
                        source: r.source.0,
                        name,
                        pages: r.pages.iter().map(|p| p.0).collect(),
                        encrypted: r.encrypted,
                        restricted: r.restricted,
                        has_forms: r.has_forms,
                        is_unc: r.is_unc,
                    });
                }
                Err(e) => errors.push(e),
            }
            entry.set_progress((i + 1) as f32 / paths.len() as f32);
            self.emit_job(&entry);
        }
        entry.finish(match (errors.first(), reports.is_empty()) {
            (Some(e), true) => Err(e.clone()),
            _ => Ok(JobOutput::default()),
        });
        self.emit_job(&entry);
        self.jobs.trim(50);
        if reports.is_empty() {
            // The caller gets the error; nothing else to announce.
            return Err(errors
                .into_iter()
                .next()
                .unwrap_or_else(|| AppError::internal("nothing imported")));
        }
        // Some files came in and some did not: the ones that did not are announced.
        for e in &errors {
            self.emit_error(e);
        }
        self.emit.emit(Event::Import(reports.clone()));
        self.auto_ocr(&reports);
        Ok(reports)
    }

    /// FLOW-OCR-AUTO: with the policy on Auto, pages that arrived without text are queued.
    fn auto_ocr(self: &Arc<Self>, reports: &[ImportReportDto]) {
        if lock(&self.settings).ocr_mode != "Auto" {
            return;
        }
        let ids: Vec<u64> = reports
            .iter()
            .flat_map(|r| r.pages.iter().copied())
            .filter(|id| {
                self.with(|app| Ok(app.ws.find(PageId(*id)).is_some_and(|p| p.has_text == Some(false))))
                    .unwrap_or(false)
            })
            .collect();
        if !ids.is_empty() {
            if let Err(e) = self.ocr_run(Some(ids), OcrPolicyDto::default()) {
                self.emit_error(&e);
            }
        }
    }

    /// CMD-UNLOCK-PW. A wrong password answers `ok: false` and reveals nothing.
    pub fn unlock(&self, source: u32, mut password: String) -> Result<UnlockDto> {
        // The incoming string moves straight into a wiping wrapper (SEC-SECRETS).
        let pw = Password::new(std::mem::take(&mut password));
        if pw.expose().chars().count() > 1_000 {
            return Err(AppError::invalid("password too long"));
        }
        self.ready()?;
        let id = SourceId(source);
        let path = self.with(|app| app.locked_source_path(id))?;
        match App::try_unlock(&self.shared, &path, &pw) {
            Ok(probe) => {
                let (pages, event) = self.with(|app| {
                    let pages = app.commit_unlock(id, pw, &probe)?;
                    Ok((pages, self.pages_event(app)))
                })?;
                self.emit.emit(event);
                Ok(UnlockDto {
                    ok: true,
                    pages: pages.iter().map(|p| p.0).collect(),
                })
            }
            Err(e) if e.code == ErrorCode::NeedsPassword => Ok(UnlockDto {
                ok: false,
                pages: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }

    // -- OCR and search --------------------------------------------------

    /// CMD-OCR-RUN. Returns the job id at once; pages report through EVT-PAGE-READY.
    pub fn ocr_run(self: &Arc<Self>, pages: Option<Vec<u64>>, policy: OcrPolicyDto) -> Result<u64> {
        self.ready()?;
        let (force, text_only) = match policy.mode.as_str() {
            "add" => (false, false),
            "redo" => (true, false),
            "text" => (false, true),
            _ => return Err(AppError::invalid("unknown OCR mode")),
        };
        let langs = policy.langs.filter(|l| !l.is_empty());
        if let Some(l) = &langs {
            sheaf_ocr::langs::validate(l)?;
        }
        let ids = self.selection(pages)?;
        if ids.is_empty() {
            return Err(AppError::invalid("no pages to recognize"));
        }
        let jobs = self.with(|app| app.ocr_plan(&ids, langs, force))?;
        let me = Arc::clone(self);
        let specs: Vec<_> = jobs.iter().map(|j| j.spec.clone()).collect();
        let total = jobs.len();
        // Planning marked the pages queued; show that before the first one finishes.
        if let Ok(event) = self.with(|app| Ok(self.pages_event(app))) {
            self.emit.emit(event);
        }
        Ok(self.spawn_job("ocr", move |ctx| {
            let counts = Mutex::new((0u32, 0u32, 0u32, 0usize)); // done, skipped, failed, finished
            ctx.shared.run_stream(&specs, ctx, &|i, result| {
                let cancelled = matches!(&result, Err(e) if e.code == ErrorCode::Cancelled);
                let page = jobs[i].page;
                let applied = me.with(|app| {
                    let outcome = if cancelled {
                        // Not recognized and not failed: back to how it was.
                        let _ = app.ws.set_ocr_state(page, OcrState::None);
                        None
                    } else {
                        Some(app.ocr_apply(page, result))
                    };
                    let dto = app.ws.find(page).map(PageDto::from);
                    Ok((outcome, dto, app.ws.rev(), app.ws.can_undo(), app.ws.can_redo()))
                });
                let Ok((outcome, dto, rev, can_undo, can_redo)) = applied else {
                    return;
                };
                if let Some(page) = dto {
                    me.emit.emit(Event::PageReady(PageReadyEvent {
                        page,
                        rev,
                        can_undo,
                        can_redo,
                    }));
                }
                let mut c = lock(&counts);
                match outcome {
                    Some(OcrOutcome::Done(_)) => c.0 += 1,
                    Some(OcrOutcome::SkippedHasText) => c.1 += 1,
                    Some(OcrOutcome::Failed(_)) => c.2 += 1,
                    None => {}
                }
                c.3 += 1;
                ctx.progress(c.3 as f32 / total.max(1) as f32);
            });
            let (done, skipped, failed, _) = *lock(&counts);
            if ctx.cancelled() {
                return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
            }
            let mut out = JobOutput {
                outputs: Vec::new(),
                detail: json!({ "done": done, "skipped": skipped, "failed": failed }),
            };
            if text_only {
                let plan = me.with(|app| app.plan_text())?;
                let dir = me.output_dir_for(None)?;
                let stem = me.first_source_stem(None);
                out.outputs
                    .push(plan.execute(&ctx.shared, &dir, &format!("{stem}.ocr.txt"), ctx)?);
            }
            Ok(out)
        }))
    }

    /// CMD-OCR-RESULT.
    pub fn ocr_result(&self, page: u64) -> Result<OcrResult> {
        self.with(|app| app.ocr_result(PageId(page)))
    }

    /// CMD-SEARCH. An empty query finds nothing.
    pub fn search(&self, query: &str) -> Result<Vec<HitDto>> {
        if query.chars().count() > 200 {
            return Err(AppError::invalid("search text too long"));
        }
        self.with(|app| Ok(app.search(query).into_iter().map(HitDto::from).collect()))
    }

    // -- exports and tools -----------------------------------------------

    /// The pages a request names, in the order given; every page if `None`.
    fn selection(&self, pages: Option<Vec<u64>>) -> Result<Vec<PageId>> {
        match pages {
            None => self.with(|app| Ok(app.ws.all_pages().map(|p| p.id).collect())),
            Some(ids) => page_ids(&ids),
        }
    }

    /// Where outputs go: the chosen folder, else beside the first selected source (ADR-NO-OVERWRITE).
    fn output_dir_for(&self, pages: Option<&[PageId]>) -> Result<PathBuf> {
        if let Some(dir) = lock(&self.out_dir).clone() {
            return Ok(dir);
        }
        self.with(|app| {
            let first = app
                .ws
                .all_pages()
                .filter(|p| pages.is_none_or(|ids| ids.contains(&p.id)))
                .find_map(|p| match p.content {
                    PageContent::Source { src, .. } => app.ws.source(src),
                    PageContent::Blank { .. } => None,
                });
            first
                .and_then(|s| s.path.parent().map(Path::to_path_buf))
                .ok_or_else(|| AppError::invalid("choose an output folder first"))
        })
    }

    fn first_source_stem(&self, pages: Option<&[PageId]>) -> String {
        self.with(|app| {
            Ok(app
                .ws
                .all_pages()
                .filter(|p| pages.is_none_or(|ids| ids.contains(&p.id)))
                .find_map(|p| match p.content {
                    PageContent::Source { src, .. } => app.ws.source(src),
                    PageContent::Blank { .. } => None,
                })
                .and_then(|s| s.path.file_stem().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "document".into()))
        })
        .unwrap_or_else(|_| "document".into())
    }

    /// Swap picked-file tokens in stamps for their paths (SEC-PATHS: the UI
    /// never names a path; it can only name a file it was shown a dialog for).
    fn resolve_stamps(&self, mut stamps: Vec<Stamp>) -> Result<Vec<Stamp>> {
        if stamps.len() > 8 {
            return Err(AppError::invalid("too many stamps"));
        }
        for s in &mut stamps {
            s.validate()?;
            if let Some(token) = s.image.take() {
                let path = lock(&self.picks)
                    .get(&token)
                    .cloned()
                    .ok_or_else(|| AppError::not_found("picked file"))?;
                s.image = Some(
                    path.to_str()
                        .map(str::to_string)
                        .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?,
                );
            }
        }
        Ok(stamps)
    }

    /// CMD-EXPORT / FLOW-EXPORT.
    pub fn export(self: &Arc<Self>, spec: ExportSpecDto) -> Result<u64> {
        self.start_export("export", spec, "")
    }

    /// CMD-TOOL-RUN / FLOW-TOOL-RUN. `params` depend on the tool:
    ///
    /// | tool | params |
    /// |---|---|
    /// | `split` | `{rule: SplitRule, name?}` |
    /// | `compress` | `{level: Low\|Recommended\|Extreme}` |
    /// | `stamp` | `{stamps: Stamp[]}` (TOOL-WATERMARK, TOOL-PAGENUM) |
    /// | `protect` | `{protect: {userPw, ownerPw, allowPrint, allowCopy, allowEdit}}` |
    /// | `unlock` | `{}` (copies pages of unlocked sources into a file with no password) |
    /// | `pdf2img` | `{format: png\|jpg, dpi, mode?: render\|extract}` |
    pub fn tool_run(self: &Arc<Self>, tool: &str, params: Value, pages: Option<Vec<u64>>) -> Result<u64> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Split {
            rule: SplitRule,
            #[serde(default)]
            name: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Compress {
            level: CompressLevel,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Stamps {
            stamps: Vec<Stamp>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Lock {
            protect: Protect,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Pdf2Img {
            #[serde(default = "png")]
            format: String,
            #[serde(default = "dpi")]
            dpi: u32,
            #[serde(default)]
            mode: String,
        }
        fn png() -> String {
            "png".into()
        }
        fn dpi() -> u32 {
            150
        }
        fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
            serde_json::from_value(v).map_err(|_| AppError::invalid("tool parameters are not valid"))
        }
        let mut spec = ExportSpecDto {
            pages,
            ..Default::default()
        };
        let suffix = match tool {
            "split" => {
                let p: Split = parse(params)?;
                spec.split = p.rule;
                spec.name = if p.name.trim().is_empty() {
                    "{source}-{counter}".into()
                } else {
                    p.name
                };
                ""
            }
            "compress" => {
                spec.compress = Some(parse::<Compress>(params)?.level);
                ".compressed"
            }
            "stamp" => {
                spec.stamps = parse::<Stamps>(params)?.stamps;
                ".stamped"
            }
            "protect" => {
                spec.protect = Some(parse::<Lock>(params)?.protect);
                ".locked"
            }
            "unlock" => {
                if !params.is_null() && params != json!({}) {
                    return Err(AppError::invalid("tool parameters are not valid"));
                }
                ".unlocked"
            }
            "pdf2img" => {
                let p: Pdf2Img = parse(params)?;
                spec.format = if p.mode == "extract" { "extract" } else { "images" }.into();
                spec.image_format = p.format;
                spec.dpi = p.dpi;
                ""
            }
            _ => return Err(AppError::new(ErrorCode::Unsupported, "this tool is not available yet")),
        };
        self.start_export(&format!("tool:{tool}"), spec, suffix)
    }

    fn start_export(self: &Arc<Self>, kind: &str, dto: ExportSpecDto, suffix: &str) -> Result<u64> {
        self.ready()?;
        check_pattern(&dto.name)?;
        let ids = dto.pages.as_deref().map(page_ids).transpose()?;
        if let Some(ids) = &ids {
            self.with(|app| {
                if ids.is_empty() || ids.iter().any(|id| app.ws.find(*id).is_none()) {
                    return Err(AppError::not_found("page"));
                }
                Ok(())
            })?;
        }
        let out_dir = self.output_dir_for(ids.as_deref())?;
        let stem = self.first_source_stem(ids.as_deref());
        let input_bytes = self.input_bytes(ids.as_deref());
        match dto.format.as_str() {
            "pdf" => {
                if !(1..=2_400).contains(&dto.dpi) {
                    return Err(AppError::invalid("dpi out of range"));
                }
                let spec = ExportSpec {
                    pages: ids,
                    split: dto.split,
                    name: if dto.name.trim().is_empty() {
                        format!("{{source}}{}", if suffix.is_empty() { ".edited" } else { suffix })
                    } else {
                        dto.name
                    },
                    stamps: self.resolve_stamps(dto.stamps)?,
                    image_fit: dto.image_fit,
                    compress: dto.compress,
                    protect: dto.protect,
                    crop_hide_only: dto.crop_hide_only,
                    flatten_forms: dto.flatten_forms,
                };
                let plan = self.with(|app| app.plan_export(&spec))?;
                Ok(self.spawn_job(kind, move |ctx| {
                    let done = plan.execute(&ctx.shared, &out_dir, ctx)?;
                    let output_bytes: u64 = done
                        .iter()
                        .filter_map(|d| std::fs::metadata(&d.path).ok())
                        .map(|m| m.len())
                        .sum();
                    let files: Vec<Value> = done
                        .iter()
                        .map(|d| {
                            json!({
                                "name": file_name(&d.path),
                                "pages": d.built.page_count,
                                "copied": d.built.copied,
                                "rasterized": d.built.rasterized,
                                "textLayers": d.built.text_layers,
                                "stamps": d.built.stamps,
                                "flattened": d.built.flattened,
                                "encryption": d.built.encryption,
                            })
                        })
                        .collect();
                    Ok(JobOutput {
                        outputs: done.into_iter().map(|d| d.path).collect(),
                        detail: json!({ "files": files, "inputBytes": input_bytes, "outputBytes": output_bytes }),
                    })
                }))
            }
            "images" => {
                let format = match dto.image_format.as_str() {
                    "png" => ImageFormat::Png,
                    "jpg" => ImageFormat::Jpg,
                    _ => return Err(AppError::invalid("unknown image format")),
                };
                if !(36..=1_200).contains(&dto.dpi) {
                    return Err(AppError::invalid("dpi out of range"));
                }
                let plan = self.with(|app| app.plan_images(&ids, format, dto.dpi))?;
                let folder = if dto.name.trim().is_empty() {
                    format!("{stem}-images")
                } else {
                    dto.name
                };
                Ok(self.spawn_job(kind, move |ctx| {
                    let files = plan.execute(&ctx.shared, &out_dir, &folder, ctx)?;
                    Ok(JobOutput {
                        detail: json!({ "count": files.len() }),
                        outputs: files,
                    })
                }))
            }
            "extract" => {
                let page = match ids.as_deref() {
                    Some([one]) => *one,
                    _ => return Err(AppError::invalid("choose exactly one page")),
                };
                let plan = self.with(|app| app.plan_extract(page))?;
                let folder = format!("{stem}-embedded");
                Ok(self.spawn_job(kind, move |ctx| {
                    let (files, skipped) = plan.execute(&ctx.shared, &out_dir, &folder, ctx)?;
                    Ok(JobOutput {
                        detail: json!({ "count": files.len(), "skipped": skipped }),
                        outputs: files,
                    })
                }))
            }
            "text" => {
                let plan = self.with(|app| app.plan_text())?;
                let name = if dto.name.trim().is_empty() {
                    format!("{stem}.txt")
                } else {
                    format!("{}.txt", sheaf_store::sanitize_file_name(&dto.name))
                };
                Ok(self.spawn_job(kind, move |ctx| {
                    let file = plan.execute(&ctx.shared, &out_dir, &name, ctx)?;
                    Ok(JobOutput {
                        outputs: vec![file],
                        detail: Value::Null,
                    })
                }))
            }
            _ => Err(AppError::invalid("unknown export format")),
        }
    }

    /// Size on disk of the distinct source files behind the selected pages
    /// (for the before/after of TOOL-COMPRESS).
    fn input_bytes(&self, pages: Option<&[PageId]>) -> u64 {
        self.with(|app| {
            let mut seen = std::collections::BTreeSet::new();
            let mut total = 0;
            for p in app
                .ws
                .all_pages()
                .filter(|p| pages.is_none_or(|ids| ids.contains(&p.id)))
            {
                if let PageContent::Source { src, .. } = p.content {
                    if let Some(s) = app.ws.source(src).filter(|_| seen.insert(src)) {
                        total += std::fs::metadata(&s.path).map(|m| m.len()).unwrap_or(0);
                    }
                }
            }
            Ok(total)
        })
        .unwrap_or(0)
    }

    // -- jobs ------------------------------------------------------------

    /// Run `work` on its own thread as an app job: events for every state
    /// change, panics turned into an `Internal` failure.
    fn spawn_job(self: &Arc<Self>, kind: &str, work: impl FnOnce(&Ctx) -> Result<JobOutput> + Send + 'static) -> u64 {
        let entry = self.jobs.create(kind);
        let id = entry.id;
        let ctx = Ctx {
            entry,
            shared: Arc::clone(&self.shared),
            emit: Arc::clone(&self.emit),
        };
        ctx.emit_view();
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name(format!("job-{id}")).spawn(move || {
            ctx.entry.set_running();
            ctx.emit_view();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&ctx)))
                .unwrap_or_else(|_| Err(AppError::internal("a job stopped unexpectedly")));
            if let Err(e) = &result {
                if e.code != ErrorCode::Cancelled {
                    me.emit_error(e);
                }
            }
            ctx.entry.finish(result);
            ctx.emit_view();
            me.jobs.trim(50);
        });
        if let Err(e) = spawned {
            // No thread: report the job as failed instead of leaving it queued forever.
            if let Some(entry) = self.jobs.get(id) {
                entry.finish(Err(AppError::io("start job", &e)));
                self.emit_job(&entry);
            }
        }
        id
    }

    /// CMD-JOB-CANCEL. Unknown ids are ignored.
    pub fn job_cancel(&self, id: u64) {
        if let Some(entry) = self.jobs.get(id) {
            entry.request_cancel(&self.shared);
        }
    }

    pub fn jobs(&self) -> Vec<JobView> {
        self.jobs.all().iter().map(|e| e.view()).collect()
    }

    /// CMD-REVEAL: the file to show in Explorer. Only a file this app
    /// produced as the output of a job, and only while it still exists.
    pub fn reveal_path(&self, id: u64, index: usize) -> Result<PathBuf> {
        let entry = self.jobs.get(id).ok_or_else(|| AppError::not_found("job"))?;
        let path = entry
            .outputs()
            .get(index)
            .cloned()
            .ok_or_else(|| AppError::not_found("output"))?;
        if !path.exists() {
            return Err(AppError::not_found("output"));
        }
        Ok(path)
    }

    // -- settings and picks ----------------------------------------------

    /// CMD-SETTINGS get.
    pub fn settings(&self) -> SettingsDto {
        SettingsDto::from(&*lock(&self.settings))
    }

    fn store_settings(&self, next: Settings) -> Result<SettingsDto> {
        let next = next.sanitized();
        if let Some(dir) = &self.config_dir {
            next.save(dir)?;
        }
        let dto = SettingsDto::from(&next);
        *lock(&self.settings) = next;
        Ok(dto)
    }

    /// CMD-SETTINGS set.
    pub fn set_settings(&self, patch: SettingsPatch) -> Result<SettingsDto> {
        let mut next = lock(&self.settings).clone();
        let check = |value: &str, allowed: &[&str]| {
            allowed
                .contains(&value)
                .then_some(())
                .ok_or_else(|| AppError::invalid("setting out of range"))
        };
        if let Some(v) = patch.language {
            check(&v, &["en", "uz", "ru"])?;
            next.language = v;
        }
        if let Some(v) = patch.theme {
            check(&v, &["system", "light", "dark"])?;
            next.theme = v;
        }
        if let Some(v) = patch.ocr_quality {
            check(&v, &["Fast", "Accurate"])?;
            next.ocr_quality = v;
        }
        if let Some(v) = patch.ocr_mode {
            check(&v, &["Auto", "Manual", "Off"])?;
            next.ocr_mode = v;
        }
        if let Some(v) = patch.workers {
            next.workers = (v != 0).then_some(v);
        }
        if let Some(v) = patch.default_preset {
            if v.chars().count() > 100 {
                return Err(AppError::invalid("preset name too long"));
            }
            next.default_preset = (!v.is_empty()).then_some(v);
        }
        self.store_settings(next)
    }

    /// The output folder, after the shell ran a folder dialog (CMD-SETTINGS;
    /// the UI cannot type a path). `None` goes back to "beside the source".
    pub fn set_output_dir(&self, dir: Option<&Path>) -> Result<SettingsDto> {
        let checked = dir.map(sheaf_store::check_output_dir).transpose()?;
        let mut next = lock(&self.settings).clone();
        next.output_dir = checked.clone();
        if let Some(d) = &checked {
            next.recent_folders.retain(|r| r != d);
            next.recent_folders.insert(0, d.clone());
        }
        let dto = self.store_settings(next)?;
        *lock(&self.out_dir) = checked;
        Ok(dto)
    }

    /// Remember a file the shell let the user pick (stamp image) and hand the
    /// UI a token for it. The token is all the UI can ever pass back.
    pub fn register_pick(&self, path: &Path) -> Result<PickDto> {
        let allowed = sheaf_store::allow_input(path)?;
        let n = self.pick_counter.fetch_add(1, Ordering::Relaxed);
        let token = sheaf_store::hex(&sheaf_store::hash_bytes(&[
            &n.to_le_bytes(),
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
                .to_le_bytes(),
            allowed.path.to_string_lossy().as_bytes(),
        ]))[..16]
            .to_string();
        let mut picks = lock(&self.picks);
        if picks.len() >= 32 {
            picks.clear();
        }
        picks.insert(token.clone(), allowed.path.clone());
        Ok(PickDto {
            token,
            name: file_name(&allowed.path),
        })
    }

    // -- not yet ---------------------------------------------------------

    /// CMD-SCAN-DEVICES, CMD-SCAN-START, CMD-SCAN-CANCEL arrive with PH-5.
    pub fn scan_unavailable(&self) -> AppError {
        AppError::new(ErrorCode::Unsupported, "scanning arrives with PH-5")
    }

    /// CMD-PRESETS arrives with PH-6; until then the list is empty.
    pub fn presets_list(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn presets_unavailable(&self) -> AppError {
        AppError::new(ErrorCode::Unsupported, "presets arrive with PH-6")
    }

    // -- shutdown --------------------------------------------------------

    /// FLOW-LIFECYCLE shutdown: cancel jobs, wait briefly for them, kill the
    /// workers, delete the session dir.
    pub fn shutdown(&self) {
        self.closing.set();
        self.jobs.cancel_all(&self.shared);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.jobs.all().iter().any(|e| !e.is_finished()) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let app = lock(&self.app).take();
        if let Some(app) = app {
            let _ = app.close();
        }
    }

    pub fn session_dir(&self) -> PathBuf {
        self.shared.session.dir().to_path_buf()
    }
}
