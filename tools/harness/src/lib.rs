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
    ops, BuildArgs, BuildResult, PageSpec, ProbeArgs, ProbeResult, RenderArgs, RenderResult,
    SourceRef,
};
use sheaf_core::{
    AppError, ErrorCode, NewPage, Op, Page, PageContent, PageId, Priority, Result, Source,
    SourceId, Workspace,
};
use sheaf_jobs::{JobSpec, Pool, PoolConfig};
use sheaf_store::Session;
use std::path::{Path, PathBuf};

pub struct App {
    pub ws: Workspace,
    // Field order matters: the pool (and its workers) must go before the session dir.
    pool: Pool,
    session: Session,
    counter: u64,
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
    pub fn start(worker_exe: &Path, pdfium_dir: &Path, base: &Path, workers: usize) -> Result<App> {
        std::fs::create_dir_all(base).map_err(|e| AppError::io("create base dir", &e))?;
        sheaf_store::sweep_stale(base);
        let session = Session::create(base)?;
        let cfg = PoolConfig {
            program: worker_exe.to_path_buf(),
            args: vec![
                "--session".into(),
                session.dir().into(),
                "--pdfium".into(),
                pdfium_dir.into(),
            ],
            workers,
        };
        Ok(App {
            ws: Workspace::new(),
            pool: Pool::new(cfg, |_| {}),
            session,
            counter: 0,
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
        Ok(PageSpec {
            source,
            index,
            blank_pt,
            rot: page.rot,
            crop: page.crop,
            filter: page.filter,
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
