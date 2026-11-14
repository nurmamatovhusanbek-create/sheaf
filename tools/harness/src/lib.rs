//! Headless application core for PH-1: the main-process side of FLOW-IMPORT,
//! FLOW-EDIT and FLOW-EXPORT without a window. MOD-SHELL (PH-4) will own this
//! wiring; until then it lives here so it can be driven from tests, the
//! command line and the perf bench.
//!
//! Like the real main process, this code never parses a document: it talks
//! to worker processes through MOD-JOBS (SEC-ISOLATE).

use serde_json::Value;
use sheaf_core::consts::THUMB_PX;
use sheaf_core::proto::{
    ops, BuildArgs, BuildResult, OcrArgs, OcrPageResult, PageSpec, PageTextArgs, PageTextResult,
    ProbeArgs, ProbeResult, RenderArgs, RenderResult, SourceRef,
};
use sheaf_core::{
    AppError, ErrorCode, NewPage, NormRect, OcrResult, OcrState, Op, Page, PageContent, PageId,
    Priority, Result, Rot, Source, SourceId, SourceKind, Workspace,
};
use sheaf_jobs::{JobSpec, Pool, PoolConfig};
use sheaf_ocr::{CacheEntry, Hit, SearchIndex};
use sheaf_store::Session;
use std::path::{Path, PathBuf};

/// Where the bundled engines live.
#[derive(Debug, Clone)]
pub struct Engines {
    pub worker: PathBuf,
    pub pdfium_dir: PathBuf,
    /// Tesseract executable and tessdata dir; without them OCR jobs fail with `EngineMissing`.
    pub tesseract: Option<(PathBuf, PathBuf)>,
}

impl Engines {
    /// The vendored engines of a development checkout, with the worker
    /// binary that sits beside the running executable.
    pub fn vendored() -> Result<Engines> {
        Ok(Engines {
            worker: sibling_worker()?,
            pdfium_dir: fixture_gen::vendored_pdfium_dir(),
            tesseract: Some((fixture_gen::tesseract_exe(), fixture_gen::tessdata_dir())),
        })
    }
}

/// How OCR went for one page (FLOW-OCR-MANUAL).
#[derive(Debug, Clone, PartialEq)]
pub enum OcrOutcome {
    Done(OcrPageResult),
    SkippedHasText,
    Failed(AppError),
}

pub struct App {
    pub ws: Workspace,
    // Field order matters: the pool (and its workers) must go before the session dir.
    pool: Pool,
    session: Session,
    counter: u64,
    index: SearchIndex,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    pub source: SourceId,
    pub pages: Vec<PageId>,
    /// The file is locked: nothing was imported yet (CMD-UNLOCK-PW, PH-3).
    pub encrypted: bool,
    /// The file is on a network share (SEC-PATHS: allowed but flagged).
    pub is_unc: bool,
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| AppError::internal("worker result has the wrong shape"))
}

fn encode<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| AppError::internal(format!("encode args: {e}")))
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

impl App {
    /// FLOW-LIFECYCLE startup: sweep stale sessions, create ours, set up the
    /// pool. Workers start lazily, on the first job.
    pub fn start(engines: &Engines, base: &Path, workers: usize) -> Result<App> {
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
            args.extend([
                "--tesseract".into(),
                exe.into(),
                "--tessdata".into(),
                tessdata.into(),
            ]);
        }
        let cfg = PoolConfig {
            program: engines.worker.clone(),
            args,
            workers,
        };
        Ok(App {
            ws: Workspace::new(),
            pool: Pool::new(cfg, |_| {}),
            session,
            counter: 0,
            index: SearchIndex::new(),
        })
    }

    pub fn session_dir(&self) -> &Path {
        self.session.dir()
    }

    /// FLOW-IMPORT steps 2 to 5: validate the path, probe in a worker,
    /// register the source, insert its pages at the end.
    pub fn import(&mut self, path: &Path) -> Result<ImportReport> {
        let allowed = sheaf_store::allow_input(path)?;
        let path_str = allowed
            .path
            .to_str()
            .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?;
        let args = encode(&ProbeArgs {
            path: path_str.to_string(),
        })?;
        let probe: ProbeResult = decode(self.pool.run(JobSpec::new(
            ops::IMPORT_PROBE,
            args,
            Priority::Visible,
        ))?)?;
        let sha256 = sheaf_store::hash_file(&allowed.path)?;
        let source = self.ws.add_source(|id| Source {
            id,
            path: allowed.path.clone(),
            kind: probe.kind,
            sha256,
            page_count: probe.page_count,
            encrypted: probe.encrypted,
            unlocked: !probe.encrypted,
        });
        if probe.encrypted {
            return Ok(ImportReport {
                source,
                pages: Vec::new(),
                encrypted: true,
                is_unc: allowed.is_unc,
            });
        }
        let new: Vec<NewPage> = (0..probe.page_count)
            .map(|index| NewPage {
                content: PageContent::Source { src: source, index },
                has_text: probe.has_text.get(index as usize).copied(),
            })
            .collect();
        let at = self.ws.len();
        self.ws.apply(Op::Insert { at, pages: new })?;
        let pages = self
            .ws
            .pages(at..self.ws.len())
            .iter()
            .map(|p| p.id)
            .collect();
        Ok(ImportReport {
            source,
            pages,
            encrypted: false,
            is_unc: allowed.is_unc,
        })
    }

    fn page_spec(&self, page: &Page) -> Result<PageSpec> {
        let (source, index, blank_pt) = match page.content {
            PageContent::Source { src, index } => {
                let s = self
                    .ws
                    .source(src)
                    .ok_or_else(|| AppError::not_found("source"))?;
                let path = s.path.to_str().ok_or_else(|| {
                    AppError::new(ErrorCode::PathRejected, "path is not valid Unicode")
                })?;
                (
                    Some(SourceRef {
                        path: path.to_string(),
                        kind: s.kind,
                    }),
                    index,
                    None,
                )
            }
            PageContent::Blank {
                width_pt,
                height_pt,
            } => (None, 0, Some((width_pt, height_pt))),
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
        })
    }

    fn thumb_job(&self, id: PageId, px: u32) -> Result<(JobSpec, String)> {
        let page = self
            .ws
            .find(id)
            .ok_or_else(|| AppError::not_found("page"))?;
        // Keyed by page and revision, like the `sheaf-thumb://` scheme will be (CMD-THUMB).
        let out = format!("thumbs/{}-{}-{px}.png", id.0, self.ws.rev());
        let args = encode(&RenderArgs {
            page: self.page_spec(page)?,
            max_px: (px, px),
            out: out.clone(),
        })?;
        Ok((JobSpec::new(ops::THUMB, args, Priority::Visible), out))
    }

    /// Render thumbnails for `ids`, spread over the worker pool. Returns the
    /// PNG paths in the same order.
    pub fn thumbs(&self, ids: &[PageId]) -> Result<Vec<PathBuf>> {
        let jobs = ids
            .iter()
            .map(|&id| self.thumb_job(id, THUMB_PX))
            .collect::<Result<Vec<_>>>()?;
        let results: Vec<Result<Value>> = std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .iter()
                .map(|(spec, _)| scope.spawn(|| self.pool.run(spec.clone())))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(AppError::internal("thumbnail thread panicked")))
                })
                .collect()
        });
        results
            .into_iter()
            .map(|r| {
                let done: RenderResult = decode(r?)?;
                self.session.resolve(&done.out)
            })
            .collect()
    }

    /// FLOW-OCR-AUTO / FLOW-OCR-MANUAL: recognize `ids` across the worker
    /// pool. `langs: None` detects script and orientation per page; `force`
    /// recognizes pages that already have text ("redo all"). One page failing
    /// does not stop the others.
    pub fn ocr(
        &mut self,
        ids: &[PageId],
        langs: Option<Vec<String>>,
        force: bool,
    ) -> Result<Vec<(PageId, OcrOutcome)>> {
        let mut jobs = Vec::with_capacity(ids.len());
        for &id in ids {
            let page = self
                .ws
                .find(id)
                .ok_or_else(|| AppError::not_found("page"))?;
            let mut spec = self.page_spec(page)?;
            // Recognition looks at the pixels alone.
            spec.text_layer = None;
            spec.rasterize = false;
            let args = encode(&OcrArgs {
                page: spec,
                langs: langs.clone(),
                force,
            })?;
            jobs.push(JobSpec::new(ops::OCR_PAGE, args, Priority::Selection));
            self.ws.set_ocr_state(id, OcrState::Queued)?;
        }
        let results: Vec<Result<Value>> = std::thread::scope(|scope| {
            let pool = &self.pool;
            let handles: Vec<_> = jobs
                .iter()
                .map(|spec| scope.spawn(move || pool.run(spec.clone())))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(AppError::internal("OCR thread panicked")))
                })
                .collect()
        });
        let mut outcomes = Vec::with_capacity(ids.len());
        for (&id, result) in ids.iter().zip(results) {
            let outcome = match result.and_then(decode::<OcrPageResult>) {
                Ok(r) if r.skipped_has_text => {
                    self.ws.set_ocr_state(id, OcrState::SkippedHasText)?;
                    OcrOutcome::SkippedHasText
                }
                Ok(r) => match self.record_ocr(id, &r) {
                    Ok(()) => OcrOutcome::Done(r),
                    Err(e) => OcrOutcome::Failed(e),
                },
                Err(e) => OcrOutcome::Failed(e),
            };
            if let OcrOutcome::Failed(e) = &outcome {
                self.ws.set_ocr_state(
                    id,
                    OcrState::Failed {
                        reason: format!("{:?}", e.code),
                    },
                )?;
            }
            outcomes.push((id, outcome));
        }
        Ok(outcomes)
    }

    /// OCR-PIPELINE steps 7 and 8 on the main side: page state, the
    /// orientation fix as an op, the search index.
    fn record_ocr(&mut self, id: PageId, r: &OcrPageResult) -> Result<()> {
        let key = r
            .key
            .clone()
            .ok_or_else(|| AppError::internal("OCR result without a key"))?;
        let words = CacheEntry::at(&self.session, &key)?.result()?;
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
            Some(OcrState::Done { key, .. }) => CacheEntry::at(&self.session, key)?.result(),
            Some(_) => Err(AppError::not_found("OCR result")),
            None => Err(AppError::not_found("page")),
        }
    }

    /// CMD-SEARCH over recognized pages.
    pub fn search(&self, query: &str) -> Vec<Hit> {
        self.index.search(query, 200)
    }

    /// Text of one page of a PDF file, optionally only inside a region of the
    /// page as displayed. Extraction happens in a worker.
    pub fn file_text(&self, path: &Path, index: u32, within: Option<NormRect>) -> Result<String> {
        let allowed = sheaf_store::allow_input(path)?;
        let path = allowed
            .path
            .to_str()
            .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?;
        let args = encode(&PageTextArgs {
            path: path.to_string(),
            index,
            within,
        })?;
        let done: PageTextResult = decode(self.pool.run(JobSpec::new(
            ops::PAGE_TEXT,
            args,
            Priority::Selection,
        ))?)?;
        Ok(done.text)
    }

    /// "Extract text only": the text of every page, recognized or already
    /// present, as a `.txt` file. The documents themselves are untouched.
    pub fn export_text(&mut self, out_dir: &Path, name: &str) -> Result<PathBuf> {
        let mut text = String::new();
        for page in self.ws.all_pages() {
            let page_text = match (&page.ocr, page.content) {
                (OcrState::Done { key, .. }, _) => {
                    CacheEntry::at(&self.session, key)?.result()?.text()
                }
                (_, PageContent::Source { src, index }) => match self.ws.source(src) {
                    Some(s) if s.kind == SourceKind::Pdf && page.has_text == Some(true) => {
                        self.file_text(&s.path, index, None)?
                    }
                    _ => String::new(),
                },
                _ => String::new(),
            };
            text.push_str(page_text.trim_end());
            text.push_str("\n\u{c}\n"); // form feed between pages
        }
        self.counter += 1;
        let staged = self
            .session
            .resolve(&format!("results/text-{}.txt", self.counter))?;
        sheaf_store::write_atomic(&staged, text.as_bytes())?;
        let published = sheaf_store::publish(&staged, out_dir, name);
        let _ = std::fs::remove_file(&staged);
        published
    }

    /// FLOW-EXPORT, PDF only for now: build in a worker inside the session
    /// dir, then publish to `out_dir` without overwriting anything.
    pub fn export(&mut self, out_dir: &Path, name: &str) -> Result<(PathBuf, BuildResult)> {
        let pages = self
            .ws
            .all_pages()
            .map(|p| self.page_spec(p))
            .collect::<Result<Vec<_>>>()?;
        self.counter += 1;
        let staged = format!("results/export-{}.pdf", self.counter);
        let args = encode(&BuildArgs {
            pages,
            out: staged.clone(),
        })?;
        let built: BuildResult = decode(self.pool.run(JobSpec::new(
            ops::BUILD,
            args,
            Priority::Selection,
        ))?)?;
        let staged_path = self.session.resolve(&staged)?;
        let published = sheaf_store::publish(&staged_path, out_dir, name);
        let _ = std::fs::remove_file(&staged_path);
        Ok((published?, built))
    }

    /// FLOW-LIFECYCLE shutdown: cancel jobs, kill workers, delete the session dir.
    pub fn close(self) -> Result<()> {
        let App { pool, session, .. } = self;
        drop(pool);
        session.close()
    }
}
