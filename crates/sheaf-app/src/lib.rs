//! MOD-APP: the main-process application core behind MOD-SHELL. FLOW-IMPORT,
//! FLOW-EDIT, FLOW-EXPORT and the DOC-IPC commands, with no window toolkit,
//! so the same code is driven by the Tauri shell, the headless harness and
//! the tests.
//!
//! Like the real main process, this code never parses a document: it talks
//! to worker processes through MOD-JOBS (SEC-ISOLATE).
//!
//! Two layers. [`App`] is the single-threaded core (workspace, passwords,
//! search index) and offers blocking operations. [`Backend`] wraps it for a
//! multi-threaded host: commands that never hold the workspace lock while a
//! worker runs, jobs with progress and cancellation, events.

pub mod api;
pub mod engines;
pub mod jobs;
pub mod plan;

pub use api::Backend;
pub use engines::Engines;
pub use plan::{
    CancelFlag, ExportPlan, ExportSpec, Exported, ExtractPlan, ImagesPlan, JobCtl, NoCtl, Shared, TextPlan,
};

use plan::{decode, encode, TextSource};
use serde_json::Value;
use sheaf_core::consts::THUMB_PX;
use sheaf_core::proto::{
    ops, ImageFormat, OcrArgs, OcrPageResult, PageSpec, ProbeArgs, ProbeResult, RenderArgs, RenderResult, SourceRef,
};
use sheaf_core::{
    AppError, ErrorCode, NewPage, NormRect, OcrResult, OcrState, Op, Page, PageContent, PageId, Password, Priority,
    Result, Rot, Source, SourceId, SourceKind, Workspace,
};
use sheaf_jobs::{JobEvent, JobSpec, Pool, PoolConfig};
use sheaf_ocr::{CacheEntry, Hit, SearchIndex};
use sheaf_store::Session;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How OCR went for one page (FLOW-OCR-MANUAL).
#[derive(Debug, Clone, PartialEq)]
pub enum OcrOutcome {
    Done(OcrPageResult),
    SkippedHasText,
    Failed(AppError),
}

pub struct App {
    pub ws: Workspace,
    shared: Arc<Shared>,
    index: SearchIndex,
    /// Passwords of unlocked sources, kept in memory for this session only
    /// and wiped on drop: every job that opens the file needs it again
    /// (SEC-SECRETS). Never written to disk, settings or logs.
    passwords: std::collections::BTreeMap<SourceId, Password>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    pub source: SourceId,
    pub pages: Vec<PageId>,
    /// The file is locked: nothing was imported yet (CMD-UNLOCK-PW, PH-3).
    pub encrypted: bool,
    /// The file opens freely but an owner password restricts it; exporting lifts that (TOOL-UNLOCK).
    pub restricted: bool,
    /// The file has fillable form fields, which do not survive export as fields (RISK-FORMS-MERGE).
    pub has_forms: bool,
    /// The file is on a network share (SEC-PATHS: allowed but flagged).
    pub is_unc: bool,
}

/// A file that was looked at in a worker but is not in the workspace yet.
/// Probing takes a while, so it happens without the workspace lock; the
/// result is then committed under it ([`App::commit_import`]).
#[derive(Debug, Clone)]
pub struct Probed {
    path: PathBuf,
    is_unc: bool,
    sha256: [u8; 32],
    probe: ProbeResult,
}

impl Probed {
    pub fn page_count(&self) -> u32 {
        self.probe.page_count
    }
}

/// One page of OCR work, planned under the lock ([`App::ocr_plan`]).
#[derive(Debug, Clone)]
pub struct OcrJob {
    pub page: PageId,
    pub spec: JobSpec,
}

/// The worker binary that sits beside this executable (or one level up,
/// where cargo puts binaries relative to test executables).
pub fn sibling_worker() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "sheaf-worker.exe"
    } else {
        "sheaf-worker"
    };
    let exe = std::env::current_exe().map_err(|e| AppError::io("locate executable", &e))?;
    exe.ancestors()
        .skip(1)
        .take(2)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::EngineMissing,
                "sheaf-worker not found beside this executable",
            )
        })
}

/// Probe a file in a worker. Does not touch any workspace.
fn probe(shared: &Shared, path: &Path, password: Option<&Password>) -> Result<ProbeResult> {
    let path = path
        .to_str()
        .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?;
    let args = encode(&ProbeArgs {
        path: path.to_string(),
        password: password.cloned(),
    })?;
    shared.run_typed(JobSpec::new(ops::IMPORT_PROBE, args, Priority::Visible), &NoCtl)
}

impl App {
    /// FLOW-LIFECYCLE startup: sweep stale sessions, create ours, set up the
    /// pool. Workers start lazily, on the first job.
    pub fn start(engines: &Engines, base: &Path, workers: usize) -> Result<App> {
        Self::start_with(engines, base, workers, |_| {})
    }

    /// [`App::start`] with a callback for every pool event.
    pub fn start_with(
        engines: &Engines,
        base: &Path,
        workers: usize,
        on_event: impl Fn(JobEvent) + Send + Sync + 'static,
    ) -> Result<App> {
        std::fs::create_dir_all(base).map_err(|e| AppError::io("create base dir", &e))?;
        sheaf_store::sweep_stale(base);
        let session = Session::create(base)?;
        let mut args: Vec<std::ffi::OsString> = vec![
            "--session".into(),
            session.dir().into(),
            "--pdfium".into(),
            (&engines.pdfium_dir).into(),
        ];
        if let Some((exe, tessdata)) = &engines.tesseract {
            args.extend(["--tesseract".into(), exe.into(), "--tessdata".into(), tessdata.into()]);
        }
        if let Some(font) = &engines.font {
            args.extend(["--font".into(), font.into()]);
        }
        let cfg = PoolConfig {
            program: engines.worker.clone(),
            args,
            workers,
        };
        Ok(App {
            ws: Workspace::new(),
            shared: Arc::new(Shared::new(Pool::new(cfg, on_event), session, workers)),
            index: SearchIndex::new(),
            passwords: Default::default(),
        })
    }

    pub fn session_dir(&self) -> &Path {
        self.shared.session.dir()
    }

    /// The thread-safe half: what jobs run against without holding the app.
    pub fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    /// Look at a file without importing it: page count, sizes, text flags, protection.
    pub fn inspect(&self, path: &Path, password: Option<&Password>) -> Result<ProbeResult> {
        probe(&self.shared, &sheaf_store::allow_input(path)?.path, password)
    }

    /// FLOW-IMPORT steps 2 and 3: validate the path, probe in a worker, hash.
    /// Needs no workspace, so a host can do it without holding a lock.
    pub fn begin_import(shared: &Shared, path: &Path) -> Result<Probed> {
        let allowed = sheaf_store::allow_input(path)?;
        let probe = probe(shared, &allowed.path, None)?;
        let sha256 = sheaf_store::hash_file(&allowed.path)?;
        Ok(Probed {
            path: allowed.path,
            is_unc: allowed.is_unc,
            sha256,
            probe,
        })
    }

    /// FLOW-IMPORT steps 4 and 5: register the source and insert its pages at
    /// the end. A locked file is registered without pages; [`App::unlock`]
    /// brings them in.
    pub fn commit_import(&mut self, probed: Probed) -> Result<ImportReport> {
        let Probed {
            path,
            is_unc,
            sha256,
            probe,
        } = probed;
        let source = self.ws.add_source(|id| Source {
            id,
            path,
            kind: probe.kind,
            sha256,
            page_count: probe.page_count,
            encrypted: probe.encrypted,
            unlocked: !probe.encrypted,
        });
        let pages = match probe.encrypted {
            true => Vec::new(),
            false => self.insert_pages(source, &probe)?,
        };
        Ok(ImportReport {
            source,
            pages,
            encrypted: probe.encrypted,
            restricted: probe.restricted,
            has_forms: probe.has_forms,
            is_unc,
        })
    }

    /// FLOW-IMPORT steps 2 to 5 in one call.
    pub fn import(&mut self, path: &Path) -> Result<ImportReport> {
        let probed = Self::begin_import(&self.shared, path)?;
        self.commit_import(probed)
    }

    fn insert_pages(&mut self, source: SourceId, probe: &ProbeResult) -> Result<Vec<PageId>> {
        let new: Vec<NewPage> = (0..probe.page_count)
            .map(|index| NewPage {
                content: PageContent::Source { src: source, index },
                has_text: probe.has_text.get(index as usize).copied(),
            })
            .collect();
        let at = self.ws.len();
        self.ws.apply(Op::Insert { at, pages: new })?;
        Ok(self.ws.pages(at..self.ws.len()).iter().map(|p| p.id).collect())
    }

    /// The path of a locked source, for a host that probes it without the lock.
    pub fn locked_source_path(&self, source: SourceId) -> Result<PathBuf> {
        let src = self.ws.source(source).ok_or_else(|| AppError::not_found("source"))?;
        if src.unlocked {
            return Err(AppError::invalid("source is not locked"));
        }
        Ok(src.path.clone())
    }

    /// Try a password on a locked source without changing anything.
    pub fn try_unlock(shared: &Shared, path: &Path, password: &Password) -> Result<ProbeResult> {
        probe(shared, path, Some(password))
    }

    /// Bring a source's pages in once its password has been proven.
    pub fn commit_unlock(&mut self, source: SourceId, password: Password, probe: &ProbeResult) -> Result<Vec<PageId>> {
        if let Some(src) = self.ws.source_mut(source) {
            src.page_count = probe.page_count;
            src.unlocked = true;
        }
        self.passwords.insert(source, password);
        self.insert_pages(source, probe)
    }

    /// CMD-UNLOCK-PW: open a locked source with its password and bring its
    /// pages in. A wrong password fails with `NeedsPassword` and reveals
    /// nothing; there is no recovery or guessing (NG-CRACK).
    pub fn unlock(&mut self, source: SourceId, password: Password) -> Result<Vec<PageId>> {
        let path = self.locked_source_path(source)?;
        let probed = Self::try_unlock(&self.shared, &path, &password)?;
        self.commit_unlock(source, password, &probed)
    }

    /// The page as a worker needs to see it.
    pub fn page_spec(&self, page: &Page) -> Result<PageSpec> {
        let (source, index, blank_pt) = match page.content {
            PageContent::Source { src, index } => {
                let s = self.ws.source(src).ok_or_else(|| AppError::not_found("source"))?;
                let path = s
                    .path
                    .to_str()
                    .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?;
                (
                    Some(SourceRef {
                        path: path.to_string(),
                        kind: s.kind,
                        password: self.passwords.get(&src).cloned(),
                    }),
                    index,
                    None,
                )
            }
            PageContent::Blank { width_pt, height_pt } => (None, 0, Some((width_pt, height_pt))),
        };
        // A page whose text was recognized carries its text layer along. If it
        // also had text of its own, that text was replaced: rasterize, so the
        // old layer does not survive under the new one.
        let text_layer = match &page.ocr {
            OcrState::Done { key, .. } => Some(key.clone()),
            _ => None,
        };
        let rasterize = text_layer.is_some() && page.has_text == Some(true);
        Ok(PageSpec {
            source,
            index,
            blank_pt,
            rot: page.rot,
            crop: page.crop,
            filter: page.filter,
            text_layer,
            rasterize,
            crop_hide_only: false,
            flatten_forms: false,
        })
    }

    /// A thumbnail or preview render of one page, as a job to run and the
    /// session-relative file it writes. `max_px` is the longest side.
    /// Keyed by the page's edits, like the `sheaf-thumb://` scheme (CMD-THUMB).
    pub fn render_job(&self, id: PageId, max_px: u32) -> Result<(JobSpec, String)> {
        let page = self.ws.find(id).ok_or_else(|| AppError::not_found("page"))?;
        let preview = max_px > THUMB_PX;
        let out = format!(
            "{}/{}-{:016x}-{max_px}.png",
            if preview { "render" } else { "thumbs" },
            id.0,
            page_version(page)
        );
        let args = encode(&RenderArgs {
            page: self.page_spec(page)?,
            max_px: (max_px, max_px),
            out: out.clone(),
        })?;
        let op = if preview { ops::RENDER } else { ops::THUMB };
        Ok((JobSpec::new(op, args, Priority::Visible), out))
    }

    /// Render thumbnails for `ids`, spread over the worker pool. Returns the
    /// PNG paths in the same order.
    pub fn thumbs(&self, ids: &[PageId]) -> Result<Vec<PathBuf>> {
        let jobs = ids
            .iter()
            .map(|&id| self.render_job(id, THUMB_PX).map(|(spec, _)| spec))
            .collect::<Result<Vec<_>>>()?;
        self.shared
            .run_all::<RenderResult>(&jobs, &NoCtl)
            .into_iter()
            .map(|r| self.shared.session.resolve(&r?.out))
            .collect()
    }

    /// FLOW-OCR-AUTO / FLOW-OCR-MANUAL, planning step: one worker job per
    /// page, and the pages marked queued. `langs: None` detects script and
    /// orientation per page; `force` recognizes pages that already have text
    /// ("redo all").
    pub fn ocr_plan(&mut self, ids: &[PageId], langs: Option<Vec<String>>, force: bool) -> Result<Vec<OcrJob>> {
        let mut jobs = Vec::with_capacity(ids.len());
        for &id in ids {
            let page = self.ws.find(id).ok_or_else(|| AppError::not_found("page"))?;
            let mut spec = self.page_spec(page)?;
            // Recognition looks at the pixels alone.
            spec.text_layer = None;
            spec.rasterize = false;
            let args = encode(&OcrArgs {
                page: spec,
                langs: langs.clone(),
                force,
            })?;
            jobs.push(OcrJob {
                page: id,
                spec: JobSpec::new(ops::OCR_PAGE, args, Priority::Selection),
            });
        }
        for job in &jobs {
            self.ws.set_ocr_state(job.page, OcrState::Queued)?;
        }
        Ok(jobs)
    }

    /// Record how one planned OCR job ended: page state, the orientation fix
    /// as an op, the search index.
    pub fn ocr_apply(&mut self, id: PageId, result: Result<Value>) -> OcrOutcome {
        let outcome = match result.and_then(decode::<OcrPageResult>) {
            Ok(r) if r.skipped_has_text => match self.ws.set_ocr_state(id, OcrState::SkippedHasText) {
                Ok(_) => OcrOutcome::SkippedHasText,
                Err(e) => OcrOutcome::Failed(e),
            },
            Ok(r) => match self.record_ocr(id, &r) {
                Ok(()) => OcrOutcome::Done(r),
                Err(e) => OcrOutcome::Failed(e),
            },
            Err(e) => OcrOutcome::Failed(e),
        };
        if let OcrOutcome::Failed(e) = &outcome {
            // The page may be gone (deleted while recognizing); nothing to record then.
            let _ = self.ws.set_ocr_state(
                id,
                OcrState::Failed {
                    reason: format!("{:?}", e.code),
                },
            );
        }
        outcome
    }

    /// FLOW-OCR-AUTO / FLOW-OCR-MANUAL: recognize `ids` across the worker
    /// pool and wait. One page failing does not stop the others.
    pub fn ocr(
        &mut self,
        ids: &[PageId],
        langs: Option<Vec<String>>,
        force: bool,
    ) -> Result<Vec<(PageId, OcrOutcome)>> {
        let jobs = self.ocr_plan(ids, langs, force)?;
        let specs: Vec<JobSpec> = jobs.iter().map(|j| j.spec.clone()).collect();
        let results: Vec<Result<Value>> = self.shared.run_all::<Value>(&specs, &NoCtl).into_iter().collect();
        Ok(jobs
            .iter()
            .zip(results)
            .map(|(job, result)| (job.page, self.ocr_apply(job.page, result)))
            .collect())
    }

    /// OCR-PIPELINE steps 7 and 8 on the main side: page state, the
    /// orientation fix as an op, the search index.
    fn record_ocr(&mut self, id: PageId, r: &OcrPageResult) -> Result<()> {
        let key = r
            .key
            .clone()
            .ok_or_else(|| AppError::internal("OCR result without a key"))?;
        let words = CacheEntry::at(&self.shared.session, &key)?.result()?;
        let turn = Rot::from_degrees(i32::from(r.rotate))?;
        let already = matches!(self.ws.find(id).map(|p| &p.ocr), Some(OcrState::Done { key: k, .. }) if *k == key);
        if turn != Rot::R0 && !already {
            self.ws.apply(Op::Rotate {
                ids: vec![id],
                delta: turn,
            })?;
        }
        self.ws.set_ocr_state(
            id,
            OcrState::Done {
                engine: r.engine.clone(),
                langs: r.langs.clone(),
                key,
            },
        )?;
        self.index.set_page(id, &words);
        Ok(())
    }

    /// CMD-OCR-RESULT: words and confidences of a recognized page.
    pub fn ocr_result(&self, id: PageId) -> Result<OcrResult> {
        match self.ws.find(id).map(|p| &p.ocr) {
            Some(OcrState::Done { key, .. }) => CacheEntry::at(&self.shared.session, key)?.result(),
            Some(_) => Err(AppError::not_found("OCR result")),
            None => Err(AppError::not_found("page")),
        }
    }

    /// CMD-SEARCH over recognized pages.
    pub fn search(&self, query: &str) -> Vec<Hit> {
        self.index.search(query, 200)
    }

    /// Forget a page's words (it was deleted).
    pub fn forget_page(&mut self, id: PageId) {
        self.index.remove_page(id);
    }

    /// Text of one page of a PDF file, optionally only inside a region of the
    /// page as displayed. Extraction happens in a worker.
    pub fn file_text(&self, path: &Path, index: u32, within: Option<NormRect>) -> Result<String> {
        self.shared.file_text(path, index, within, &NoCtl)
    }

    /// "Extract text only", planning step.
    pub fn plan_text(&self) -> Result<TextPlan> {
        let mut pages = Vec::with_capacity(self.ws.len());
        for page in self.ws.all_pages() {
            pages.push(match (&page.ocr, page.content) {
                (OcrState::Done { key, .. }, _) => TextSource::Recognized {
                    text: CacheEntry::at(&self.shared.session, key)?.result()?.text(),
                },
                (_, PageContent::Source { src, index }) => match self.ws.source(src) {
                    Some(s) if s.kind == SourceKind::Pdf && page.has_text == Some(true) => TextSource::File {
                        path: s.path.clone(),
                        index,
                    },
                    _ => TextSource::Nothing,
                },
                _ => TextSource::Nothing,
            });
        }
        Ok(TextPlan { pages })
    }

    /// "Extract text only": the text of every page, recognized or already
    /// present, as a `.txt` file. The documents themselves are untouched.
    pub fn export_text(&mut self, out_dir: &Path, name: &str) -> Result<PathBuf> {
        self.plan_text()?.execute(&self.shared, out_dir, name, &NoCtl)
    }

    /// Export every page as one PDF named `name`.
    pub fn export(&mut self, out_dir: &Path, name: &str) -> Result<(PathBuf, sheaf_core::proto::BuildResult)> {
        let stem = name.strip_suffix(".pdf").unwrap_or(name);
        let mut done = self.export_pdf(out_dir, &ExportSpec::named(stem))?;
        let first = done.remove(0);
        Ok((first.path, first.built))
    }

    /// The pages an export covers, in workspace order whatever order they were given in.
    fn selected(&self, pages: &Option<Vec<PageId>>) -> Result<Vec<Page>> {
        let Some(ids) = pages else {
            return Ok(self.ws.all_pages().cloned().collect());
        };
        if ids.is_empty() || ids.iter().any(|id| self.ws.find(*id).is_none()) {
            return Err(AppError::not_found("page"));
        }
        Ok(self.ws.all_pages().filter(|p| ids.contains(&p.id)).cloned().collect())
    }

    /// FLOW-EXPORT to PDF, planning step.
    pub fn plan_export(&self, spec: &ExportSpec) -> Result<ExportPlan> {
        let pages = self.selected(&spec.pages)?;
        if pages.is_empty() {
            return Err(AppError::invalid("nothing to export"));
        }
        let mut specs = pages.iter().map(|p| self.page_spec(p)).collect::<Result<Vec<_>>>()?;
        for s in &mut specs {
            s.crop_hide_only = spec.crop_hide_only;
            s.flatten_forms = spec.flatten_forms;
        }
        let source_stem = pages
            .iter()
            .find_map(|p| match p.content {
                PageContent::Source { src, .. } => self.ws.source(src),
                PageContent::Blank { .. } => None,
            })
            .and_then(|s| s.path.file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "document".to_string());
        Ok(ExportPlan {
            specs,
            spec: spec.clone(),
            source_stem,
        })
    }

    /// FLOW-EXPORT to PDF: select, split, name, build (with stamps), compress,
    /// protect, publish. Returns one entry per file written, in order.
    pub fn export_pdf(&mut self, out_dir: &Path, spec: &ExportSpec) -> Result<Vec<Exported>> {
        self.plan_export(spec)?.execute(&self.shared, out_dir, &NoCtl)
    }

    /// TOOL-PDF2IMG, planning step.
    pub fn plan_images(&self, pages: &Option<Vec<PageId>>, format: ImageFormat, dpi: u32) -> Result<ImagesPlan> {
        let specs = self
            .selected(pages)?
            .iter()
            .map(|p| self.page_spec(p))
            .collect::<Result<Vec<_>>>()?;
        Ok(ImagesPlan { specs, format, dpi })
    }

    /// TOOL-PDF2IMG: one PNG or JPEG per page, as displayed, into a new folder.
    pub fn export_images(
        &mut self,
        out_dir: &Path,
        folder: &str,
        pages: Option<Vec<PageId>>,
        format: ImageFormat,
        dpi: u32,
    ) -> Result<Vec<PathBuf>> {
        self.plan_images(&pages, format, dpi)?
            .execute(&self.shared, out_dir, folder, &NoCtl)
    }

    /// TOOL-PDF2IMG, alternative mode, planning step.
    pub fn plan_extract(&self, page: PageId) -> Result<ExtractPlan> {
        let page = self.ws.find(page).ok_or_else(|| AppError::not_found("page"))?;
        let spec = self.page_spec(page)?;
        let source = spec
            .source
            .ok_or_else(|| AppError::invalid("a blank page has no images"))?;
        Ok(ExtractPlan {
            source,
            index: spec.index,
        })
    }

    /// TOOL-PDF2IMG, alternative mode: the images embedded in one PDF page,
    /// unchanged where the format allows. Returns the files and how many
    /// images had no common file form and were left out.
    pub fn extract_images(&mut self, out_dir: &Path, folder: &str, page: PageId) -> Result<(Vec<PathBuf>, u32)> {
        self.plan_extract(page)?.execute(&self.shared, out_dir, folder, &NoCtl)
    }

    /// FLOW-LIFECYCLE shutdown: cancel jobs, kill workers, delete the session dir.
    /// Anything else still holding [`App::shared`] keeps the session alive
    /// until it lets go.
    pub fn close(self) -> Result<()> {
        let App { shared, .. } = self;
        match Arc::try_unwrap(shared) {
            Ok(Shared { pool, session, .. }) => {
                drop(pool);
                session.close()
            }
            Err(_) => Ok(()),
        }
    }
}

/// A short fingerprint of everything that changes how a page looks: the page
/// it points at and its rotation, crop and filter. The tray's thumbnail URL
/// carries it, so an edit invalidates that page's picture and nothing else.
pub fn page_version(page: &Page) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Text is stable across runs within one build, which is all a cache key here needs.
    let look = serde_json::to_string(&(page.content, page.rot, page.crop, page.filter)).unwrap_or_default();
    look.hash(&mut h);
    h.finish()
}
