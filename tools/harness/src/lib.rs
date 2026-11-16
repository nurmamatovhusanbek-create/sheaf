//! Headless application core for PH-1: the main-process side of FLOW-IMPORT,
//! FLOW-EDIT and FLOW-EXPORT without a window. MOD-SHELL (PH-4) will own this
//! wiring; until then it lives here so it can be driven from tests, the
//! command line and the perf bench.
//!
//! Like the real main process, this code never parses a document: it talks
//! to worker processes through MOD-JOBS (SEC-ISOLATE).

use serde_json::Value;
use sheaf_core::consts::{MAX_SOURCE_PAGES, THUMB_PX};
use sheaf_core::proto::{
    ops, BuildArgs, BuildResult, ExportImagesArgs, ExtractImagesArgs, FactsArgs, FactsResult, FilesResult, ImageFormat,
    OcrArgs, OcrPageResult, PageSpec, PageTextArgs, PageTextResult, ProbeArgs, ProbeResult, RenderArgs, RenderResult,
    SourceRef,
};
use sheaf_core::{
    AppError, CompressLevel, ErrorCode, ImageFit, NameContext, NewPage, NormRect, OcrResult, OcrState, Op, Page,
    PageContent, PageFacts, PageId, Password, Priority, Protect, Result, Rot, Source, SourceId, SourceKind, SplitRule,
    Stamp, Workspace,
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
    /// TrueType font for stamps; without it stamping fails with `EngineMissing`.
    pub font: Option<PathBuf>,
}

/// FLOW-EXPORT settings (DATA-EXPORTSPEC), PDF output.
#[derive(Debug, Clone, Default)]
pub struct ExportSpec {
    /// Pages to export, in workspace order; `None` is all of them (TOOL-EXTRACT).
    pub pages: Option<Vec<PageId>>,
    pub split: SplitRule,
    /// DATA-NAMEPATTERN for the output files, without the `.pdf`.
    pub name: String,
    pub stamps: Vec<Stamp>,
    pub image_fit: ImageFit,
    pub compress: Option<CompressLevel>,
    pub protect: Option<Protect>,
    /// Apply crops on PDF pages as a CropBox: hidden, not removed (TOOL-CROP).
    pub crop_hide_only: bool,
    /// Draw form fields into their pages (TOOL-MERGE, RISK-FORMS-MERGE).
    pub flatten_forms: bool,
}

impl ExportSpec {
    pub fn named(name: &str) -> ExportSpec {
        ExportSpec {
            name: name.to_string(),
            ..Default::default()
        }
    }
}

/// One file written by an export.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub path: PathBuf,
    pub built: BuildResult,
}

impl Engines {
    /// The vendored engines of a development checkout, with the worker
    /// binary that sits beside the running executable.
    pub fn vendored() -> Result<Engines> {
        let worker = sibling_worker()?;
        // A portable bundle carries its engines in `vendor/` beside the
        // executables, laid out like the checkout's `vendor/`. It wins over
        // the checkout, whose location is only known on the build machine.
        if let Some(vendor) = worker.parent().map(|d| d.join("vendor")).filter(|d| d.is_dir()) {
            let (platform, tesseract) = if cfg!(windows) {
                (
                    "win-x64",
                    vendor.join("tesseract").join("win-x64").join("tesseract.exe"),
                )
            } else {
                ("linux-x64", PathBuf::from("tesseract"))
            };
            return Ok(Engines {
                worker,
                pdfium_dir: vendor.join("pdfium").join(platform),
                tesseract: Some((tesseract, vendor.join("tessdata"))),
                font: Some(vendor.join("fonts").join("NotoSans-Regular.ttf")),
            });
        }
        Ok(Engines {
            worker,
            pdfium_dir: fixture_gen::vendored_pdfium_dir(),
            tesseract: Some((fixture_gen::tesseract_exe(), fixture_gen::tessdata_dir())),
            font: Some(fixture_gen::noto_font()),
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
            pool: Pool::new(cfg, |_| {}),
            session,
            counter: 0,
            index: SearchIndex::new(),
            passwords: Default::default(),
        })
    }

    pub fn session_dir(&self) -> &Path {
        self.session.dir()
    }

    fn probe(&self, path: &Path, password: Option<&Password>) -> Result<ProbeResult> {
        let path = path
            .to_str()
            .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?;
        let args = encode(&ProbeArgs {
            path: path.to_string(),
            password: password.cloned(),
        })?;
        let probe: ProbeResult = decode(
            self.pool
                .run(JobSpec::new(ops::IMPORT_PROBE, args, Priority::Visible))?,
        )?;
        // A worker handles hostile files, so its answer is checked like any
        // other input before pages are allocated for it (SEC-TB2, SEC-LIMITS).
        let n = probe.page_count as usize;
        let lists_fit = probe.has_text.len() <= n && probe.sizes_pt.len() <= n;
        if probe.page_count > MAX_SOURCE_PAGES || !lists_fit {
            return Err(AppError::limit("worker reported an impossible page count"));
        }
        Ok(probe)
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

    /// Look at a file without importing it: page count, sizes, text flags, protection.
    pub fn inspect(&self, path: &Path, password: Option<&Password>) -> Result<ProbeResult> {
        self.probe(&sheaf_store::allow_input(path)?.path, password)
    }

    /// FLOW-IMPORT steps 2 to 5: validate the path, probe in a worker,
    /// register the source, insert its pages at the end. A locked file is
    /// registered without pages; [`App::unlock`] brings them in.
    pub fn import(&mut self, path: &Path) -> Result<ImportReport> {
        let allowed = sheaf_store::allow_input(path)?;
        let probe = self.probe(&allowed.path, None)?;
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
            is_unc: allowed.is_unc,
        })
    }

    /// CMD-UNLOCK-PW: open a locked source with its password and bring its
    /// pages in. A wrong password fails with `NeedsPassword` and reveals
    /// nothing; there is no recovery or guessing (NG-CRACK).
    pub fn unlock(&mut self, source: SourceId, password: Password) -> Result<Vec<PageId>> {
        let src = self.ws.source(source).ok_or_else(|| AppError::not_found("source"))?;
        if src.unlocked {
            return Err(AppError::invalid("source is not locked"));
        }
        let path = src.path.clone();
        let probe = self.probe(&path, Some(&password))?;
        if let Some(src) = self.ws.source_mut(source) {
            src.page_count = probe.page_count;
            src.unlocked = true;
        }
        self.passwords.insert(source, password);
        self.insert_pages(source, &probe)
    }

    fn page_spec(&self, page: &Page) -> Result<PageSpec> {
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

    fn thumb_job(&self, id: PageId, px: u32) -> Result<(JobSpec, String)> {
        let page = self.ws.find(id).ok_or_else(|| AppError::not_found("page"))?;
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
        let done: PageTextResult = decode(self.pool.run(JobSpec::new(ops::PAGE_TEXT, args, Priority::Selection))?)?;
        Ok(done.text)
    }

    /// "Extract text only": the text of every page, recognized or already
    /// present, as a `.txt` file. The documents themselves are untouched.
    pub fn export_text(&mut self, out_dir: &Path, name: &str) -> Result<PathBuf> {
        let mut text = String::new();
        for page in self.ws.all_pages() {
            let page_text = match (&page.ocr, page.content) {
                (OcrState::Done { key, .. }, _) => CacheEntry::at(&self.session, key)?.result()?.text(),
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
        let staged = self.session.resolve(&format!("results/text-{}.txt", self.counter))?;
        sheaf_store::write_atomic(&staged, text.as_bytes())?;
        let published = sheaf_store::publish(&staged, out_dir, name);
        let _ = std::fs::remove_file(&staged);
        published
    }

    /// Export every page as one PDF named `name`.
    pub fn export(&mut self, out_dir: &Path, name: &str) -> Result<(PathBuf, BuildResult)> {
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

    /// Run one worker job per item, spread over the pool, results in order.
    fn run_all<T: serde::de::DeserializeOwned>(&self, jobs: Vec<JobSpec>) -> Vec<Result<T>> {
        std::thread::scope(|scope| {
            let pool = &self.pool;
            let handles: Vec<_> = jobs
                .iter()
                .map(|spec| scope.spawn(move || pool.run(spec.clone())))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(AppError::internal("job thread panicked")))
                        .and_then(decode)
                })
                .collect()
        })
    }

    fn facts(&self, specs: &[PageSpec], blank: bool, text: bool) -> Result<Vec<PageFacts>> {
        if !blank && !text {
            return Ok(vec![PageFacts::default(); specs.len()]);
        }
        let jobs = specs
            .iter()
            .map(|page| {
                Ok(JobSpec::new(
                    ops::PAGE_FACTS,
                    encode(&FactsArgs {
                        page: page.clone(),
                        blank,
                        text,
                    })?,
                    Priority::Selection,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.run_all::<FactsResult>(jobs)
            .into_iter()
            .map(|r| {
                r.map(|f| PageFacts {
                    blank: f.blank,
                    text: f.text,
                })
            })
            .collect()
    }

    /// Stage a stamp image inside the session dir, where workers may read it.
    fn stage_stamp_image(&mut self, path: &str) -> Result<String> {
        let allowed = sheaf_store::allow_input(Path::new(path))?;
        self.counter += 1;
        let rel = format!("results/stamp-{}.img", self.counter);
        std::fs::copy(&allowed.path, self.session.resolve(&rel)?).map_err(|e| AppError::io("stage stamp image", &e))?;
        Ok(rel)
    }

    /// FLOW-EXPORT to PDF: select, split, name, build (with stamps), compress,
    /// protect, publish. Returns one entry per file written, in order.
    pub fn export_pdf(&mut self, out_dir: &Path, spec: &ExportSpec) -> Result<Vec<Exported>> {
        let pages = self.selected(&spec.pages)?;
        if pages.is_empty() {
            return Err(AppError::invalid("nothing to export"));
        }
        let mut specs = pages.iter().map(|p| self.page_spec(p)).collect::<Result<Vec<_>>>()?;
        for s in &mut specs {
            s.crop_hide_only = spec.crop_hide_only;
            s.flatten_forms = spec.flatten_forms;
        }
        // What the split rule and the name pattern need to know about the pages.
        let (blank, text) = spec.split.needs();
        let wants_match = spec.name.contains("{match:");
        let mut facts = self.facts(&specs, blank, text)?;
        let groups = spec.split.plan(&facts)?;
        if wants_match && !text {
            // Only the first page of each output is ever looked at.
            let firsts: Vec<usize> = groups.iter().map(|g| g[0]).collect();
            let first_specs: Vec<PageSpec> = firsts.iter().map(|&i| specs[i].clone()).collect();
            for (i, f) in firsts.into_iter().zip(self.facts(&first_specs, false, true)?) {
                facts[i].text = f.text;
            }
        }
        let mut stamps = spec.stamps.clone();
        for stamp in &mut stamps {
            stamp.validate()?;
            if let Some(path) = stamp.image.take() {
                stamp.image = Some(self.stage_stamp_image(&path)?);
            }
        }
        let date = today_utc();
        let source_stem = pages
            .iter()
            .find_map(|p| match p.content {
                PageContent::Source { src, .. } => self.ws.source(src),
                PageContent::Blank { .. } => None,
            })
            .and_then(|s| s.path.file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "document".to_string());

        let mut outputs = Vec::with_capacity(groups.len());
        for (n, group) in groups.iter().enumerate() {
            let ctx = NameContext {
                date: &date,
                n: n as u32 + 1,
                source: &source_stem,
                first_page_text: &facts[group[0]].text,
            };
            let name = format!("{}.pdf", sheaf_core::tools::expand_name(&spec.name, &ctx)?);
            self.counter += 1;
            let staged = format!("results/export-{}.pdf", self.counter);
            let args = encode(&BuildArgs {
                pages: group.iter().map(|&i| specs[i].clone()).collect(),
                out: staged.clone(),
                stamps: stamps.clone(),
                image_fit: spec.image_fit,
                compress: spec.compress,
                protect: spec.protect.clone(),
            })?;
            let built: BuildResult = decode(self.pool.run(JobSpec::new(ops::BUILD, args, Priority::Selection))?)?;
            let staged_path = self.session.resolve(&staged)?;
            let published = sheaf_store::publish(&staged_path, out_dir, &name);
            let _ = std::fs::remove_file(&staged_path);
            outputs.push(Exported {
                path: published?,
                built,
            });
        }
        Ok(outputs)
    }

    /// Move a job's files from the session dir into a new folder under `out_dir`.
    fn publish_folder(
        &self,
        files: &FilesResult,
        staged_dir: &str,
        out_dir: &Path,
        folder: &str,
    ) -> Result<Vec<PathBuf>> {
        let target = sheaf_store::create_output_dir(out_dir, folder)?;
        let mut published = Vec::with_capacity(files.files.len());
        for rel in &files.files {
            let staged = self.session.resolve(rel)?;
            let name = staged
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            published.push(sheaf_store::publish(&staged, &target, &name)?);
        }
        let _ = std::fs::remove_dir_all(self.session.resolve(staged_dir)?);
        Ok(published)
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
        let specs = self
            .selected(&pages)?
            .iter()
            .map(|p| self.page_spec(p))
            .collect::<Result<Vec<_>>>()?;
        self.counter += 1;
        let staged_dir = format!("results/images-{}", self.counter);
        let args = encode(&ExportImagesArgs {
            pages: specs,
            format,
            dpi,
            quality: 85,
            out_dir: staged_dir.clone(),
        })?;
        let files: FilesResult = decode(
            self.pool
                .run(JobSpec::new(ops::EXPORT_IMAGES, args, Priority::Selection))?,
        )?;
        self.publish_folder(&files, &staged_dir, out_dir, folder)
    }

    /// TOOL-PDF2IMG, alternative mode: the images embedded in one PDF page,
    /// unchanged where the format allows. Returns the files and how many
    /// images had no common file form and were left out.
    pub fn extract_images(&mut self, out_dir: &Path, folder: &str, page: PageId) -> Result<(Vec<PathBuf>, u32)> {
        let page = self.ws.find(page).ok_or_else(|| AppError::not_found("page"))?.clone();
        let spec = self.page_spec(&page)?;
        let source = spec
            .source
            .ok_or_else(|| AppError::invalid("a blank page has no images"))?;
        self.counter += 1;
        let staged_dir = format!("results/extract-{}", self.counter);
        let args = encode(&ExtractImagesArgs {
            source,
            index: spec.index,
            out_dir: staged_dir.clone(),
        })?;
        let files: FilesResult = decode(self.pool.run(JobSpec::new(
            ops::EXTRACT_IMAGES,
            args,
            Priority::Selection,
        ))?)?;
        Ok((
            self.publish_folder(&files, &staged_dir, out_dir, folder)?,
            files.skipped,
        ))
    }

    /// FLOW-LIFECYCLE shutdown: cancel jobs, kill workers, delete the session dir.
    pub fn close(self) -> Result<()> {
        let App { pool, session, .. } = self;
        drop(pool);
        session.close()
    }
}

/// Today's date as `YYYY-MM-DD`, in UTC: the standard library has no access
/// to the local time zone, and a file name should not depend on one anyway.
fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn date_has_the_expected_shape() {
        let d = super::today_utc();
        let parts: Vec<u32> = d.split('-').map(|p| p.parse().expect("number")).collect();
        assert!(
            d.len() == 10 && parts[0] >= 2026 && (1..=12).contains(&parts[1]) && (1..=31).contains(&parts[2]),
            "{d}"
        );
    }
}
