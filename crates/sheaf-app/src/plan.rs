//! Work that runs without the workspace lock.
//!
//! A command first *plans* under the lock (turn workspace pages into
//! self-contained [`PageSpec`]s, read settings), then *executes* against
//! [`Shared`], which holds only the pool and the session dir and is safe to
//! use from any thread. That is what lets a long export run while the tray
//! keeps scrolling and thumbnails keep arriving (PERF-BUDGETS).

use serde_json::Value;
use sheaf_core::proto::{
    ops, BuildArgs, BuildResult, ExportImagesArgs, ExtractImagesArgs, FactsArgs, FactsResult, FilesResult, ImageFormat,
    PageSpec, PageTextArgs, PageTextResult,
};
use sheaf_core::{
    AppError, CompressLevel, ErrorCode, ImageFit, JobId, NameContext, NormRect, PageFacts, PageId, Priority, Protect,
    Result, SplitRule, Stamp,
};
use sheaf_jobs::{JobSpec, Pool};
use sheaf_store::Session;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

pub(crate) fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| AppError::internal("worker result has the wrong shape"))
}

pub(crate) fn encode<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| AppError::internal(format!("encode args: {e}")))
}

/// What a running job tells its caller, and what its caller tells it.
pub trait JobCtl: Sync {
    /// The user asked to stop; checked before every worker job.
    fn cancelled(&self) -> bool {
        false
    }
    /// Fraction done, 0 to 1.
    fn progress(&self, _fraction: f32) {}
    /// A worker job was queued under this pool id (so it can be cancelled).
    fn running(&self, _id: JobId) {}
}

/// No cancellation, no progress.
pub struct NoCtl;
impl JobCtl for NoCtl {}

/// The thread-safe half of the application: worker pool and session dir.
pub struct Shared {
    // Field order matters: the pool (and its workers) must go before the session dir.
    pub pool: Pool,
    pub session: Session,
    workers: usize,
    counter: AtomicU64,
}

impl Shared {
    pub(crate) fn new(pool: Pool, session: Session, workers: usize) -> Shared {
        Shared {
            pool,
            session,
            workers: workers.max(1),
            counter: AtomicU64::new(0),
        }
    }

    /// A number never handed out before, for staged file names.
    pub fn next(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Run one worker job and wait for it, honouring cancellation.
    pub fn run(&self, spec: JobSpec, ctl: &dyn JobCtl) -> Result<Value> {
        if ctl.cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
        }
        self.pool.run_notify(spec, |id| ctl.running(id))
    }

    pub fn run_typed<T: serde::de::DeserializeOwned>(&self, spec: JobSpec, ctl: &dyn JobCtl) -> Result<T> {
        decode(self.run(spec, ctl)?)
    }

    /// Run `jobs` over the pool with at most twice as many in flight as there
    /// are workers, handing each result to `done(index, result)` as it
    /// arrives. Stops starting new jobs once `ctl` is cancelled (the rest
    /// report `Cancelled`).
    pub fn run_stream(&self, jobs: &[JobSpec], ctl: &dyn JobCtl, done: &(dyn Fn(usize, Result<Value>) + Sync)) {
        let next = AtomicUsize::new(0);
        let threads = jobs.len().min(self.workers * 2).max(1);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(spec) = jobs.get(i) else { return };
                    done(i, self.run(spec.clone(), ctl));
                });
            }
        });
    }

    /// Like [`Shared::run_stream`] but collects the results in job order.
    pub fn run_all<T: serde::de::DeserializeOwned>(&self, jobs: &[JobSpec], ctl: &dyn JobCtl) -> Vec<Result<T>> {
        let slots: Vec<Mutex<Option<Result<Value>>>> = jobs.iter().map(|_| Mutex::new(None)).collect();
        self.run_stream(jobs, ctl, &|i, result| {
            if let Some(slot) = slots.get(i) {
                *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(|p| p.into_inner())
                    .unwrap_or_else(|| Err(AppError::internal("job did not finish")))
                    .and_then(decode)
            })
            .collect()
    }

    fn facts(&self, specs: &[PageSpec], blank: bool, text: bool, ctl: &dyn JobCtl) -> Result<Vec<PageFacts>> {
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
        self.run_all::<FactsResult>(&jobs, ctl)
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
    fn stage_stamp_image(&self, path: &str) -> Result<String> {
        let allowed = sheaf_store::allow_input(Path::new(path))?;
        let rel = format!("results/stamp-{}.img", self.next());
        std::fs::copy(&allowed.path, self.session.resolve(&rel)?).map_err(|e| AppError::io("stage stamp image", &e))?;
        Ok(rel)
    }

    /// Move a job's files from the session dir into a new folder under `out_dir`.
    pub(crate) fn publish_folder(
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

    /// Text of one page of a PDF file, optionally only inside a region of the
    /// page as displayed. Extraction happens in a worker.
    pub fn file_text(&self, path: &Path, index: u32, within: Option<NormRect>, ctl: &dyn JobCtl) -> Result<String> {
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
        let done: PageTextResult = self.run_typed(JobSpec::new(ops::PAGE_TEXT, args, Priority::Selection), ctl)?;
        Ok(done.text)
    }
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

/// FLOW-EXPORT to PDF, ready to run: everything it needs from the workspace.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    pub(crate) specs: Vec<PageSpec>,
    pub(crate) spec: ExportSpec,
    pub(crate) source_stem: String,
}

impl ExportPlan {
    /// Select, split, name, build (with stamps), compress, protect, publish.
    /// Returns one entry per file written, in order.
    pub fn execute(&self, shared: &Shared, out_dir: &Path, ctl: &dyn JobCtl) -> Result<Vec<Exported>> {
        let spec = &self.spec;
        let specs = &self.specs;
        // What the split rule and the name pattern need to know about the pages.
        let (blank, text) = spec.split.needs();
        let wants_match = spec.name.contains("{match:");
        let mut facts = shared.facts(specs, blank, text, ctl)?;
        let groups = spec.split.plan(&facts)?;
        if wants_match && !text {
            // Only the first page of each output is ever looked at.
            let firsts: Vec<usize> = groups.iter().map(|g| g[0]).collect();
            let first_specs: Vec<PageSpec> = firsts.iter().map(|&i| specs[i].clone()).collect();
            for (i, f) in firsts.into_iter().zip(shared.facts(&first_specs, false, true, ctl)?) {
                facts[i].text = f.text;
            }
        }
        let mut stamps = spec.stamps.clone();
        for stamp in &mut stamps {
            stamp.validate()?;
            if let Some(path) = stamp.image.take() {
                stamp.image = Some(shared.stage_stamp_image(&path)?);
            }
        }
        let date = today_utc();

        let mut outputs = Vec::with_capacity(groups.len());
        for (n, group) in groups.iter().enumerate() {
            ctl.progress(n as f32 / groups.len() as f32);
            let ctx = NameContext {
                date: &date,
                n: n as u32 + 1,
                source: &self.source_stem,
                first_page_text: &facts[group[0]].text,
            };
            let name = format!("{}.pdf", sheaf_core::tools::expand_name(&spec.name, &ctx)?);
            let staged = format!("results/export-{}.pdf", shared.next());
            let args = encode(&BuildArgs {
                pages: group.iter().map(|&i| specs[i].clone()).collect(),
                out: staged.clone(),
                stamps: stamps.clone(),
                image_fit: spec.image_fit,
                compress: spec.compress,
                protect: spec.protect.clone(),
            })?;
            let built: BuildResult = shared.run_typed(JobSpec::new(ops::BUILD, args, Priority::Selection), ctl)?;
            let staged_path = shared.session.resolve(&staged)?;
            let published = sheaf_store::publish(&staged_path, out_dir, &name);
            let _ = std::fs::remove_file(&staged_path);
            outputs.push(Exported {
                path: published?,
                built,
            });
        }
        ctl.progress(1.0);
        Ok(outputs)
    }
}

/// TOOL-PDF2IMG ready to run.
#[derive(Debug, Clone)]
pub struct ImagesPlan {
    pub(crate) specs: Vec<PageSpec>,
    pub format: ImageFormat,
    pub dpi: u32,
}

impl ImagesPlan {
    /// One PNG or JPEG per page, as displayed, into a new folder.
    pub fn execute(&self, shared: &Shared, out_dir: &Path, folder: &str, ctl: &dyn JobCtl) -> Result<Vec<PathBuf>> {
        let staged_dir = format!("results/images-{}", shared.next());
        let args = encode(&ExportImagesArgs {
            pages: self.specs.clone(),
            format: self.format,
            dpi: self.dpi,
            quality: 85,
            out_dir: staged_dir.clone(),
        })?;
        let files: FilesResult = shared.run_typed(JobSpec::new(ops::EXPORT_IMAGES, args, Priority::Selection), ctl)?;
        let published = shared.publish_folder(&files, &staged_dir, out_dir, folder);
        ctl.progress(1.0);
        published
    }
}

/// TOOL-PDF2IMG, alternative mode, ready to run.
#[derive(Debug, Clone)]
pub struct ExtractPlan {
    pub(crate) source: sheaf_core::proto::SourceRef,
    pub(crate) index: u32,
}

impl ExtractPlan {
    /// The files and how many images had no common file form and were left out.
    pub fn execute(
        &self,
        shared: &Shared,
        out_dir: &Path,
        folder: &str,
        ctl: &dyn JobCtl,
    ) -> Result<(Vec<PathBuf>, u32)> {
        let staged_dir = format!("results/extract-{}", shared.next());
        let args = encode(&ExtractImagesArgs {
            source: self.source.clone(),
            index: self.index,
            out_dir: staged_dir.clone(),
        })?;
        let files: FilesResult = shared.run_typed(JobSpec::new(ops::EXTRACT_IMAGES, args, Priority::Selection), ctl)?;
        let skipped = files.skipped;
        let published = shared.publish_folder(&files, &staged_dir, out_dir, folder)?;
        ctl.progress(1.0);
        Ok((published, skipped))
    }
}

/// Where one page's text comes from for "extract text only".
#[derive(Debug, Clone)]
pub(crate) enum TextSource {
    Recognized { text: String },
    File { path: PathBuf, index: u32 },
    Nothing,
}

/// "Extract text only" ready to run: the text of every page, recognized or
/// already present, as one `.txt` file. The documents are untouched.
#[derive(Debug, Clone)]
pub struct TextPlan {
    pub(crate) pages: Vec<TextSource>,
}

impl TextPlan {
    pub fn execute(&self, shared: &Shared, out_dir: &Path, name: &str, ctl: &dyn JobCtl) -> Result<PathBuf> {
        let mut text = String::new();
        for (i, page) in self.pages.iter().enumerate() {
            ctl.progress(i as f32 / self.pages.len().max(1) as f32);
            let page_text = match page {
                TextSource::Recognized { text } => text.clone(),
                TextSource::File { path, index } => shared.file_text(path, *index, None, ctl)?,
                TextSource::Nothing => String::new(),
            };
            text.push_str(page_text.trim_end());
            text.push_str("\n\u{c}\n"); // form feed between pages
        }
        let staged = shared.session.resolve(&format!("results/text-{}.txt", shared.next()))?;
        sheaf_store::write_atomic(&staged, text.as_bytes())?;
        let published = sheaf_store::publish(&staged, out_dir, name);
        let _ = std::fs::remove_file(&staged);
        ctl.progress(1.0);
        published
    }
}

/// A flag a job's thread checks and the job's owner sets.
#[derive(Debug, Default)]
pub struct CancelFlag(pub AtomicBool);

impl CancelFlag {
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Today's date as `YYYY-MM-DD`, in UTC: the standard library has no access
/// to the local time zone, and a file name should not depend on one anyway.
pub(crate) fn today_utc() -> String {
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
