//! CMD-TOOL-RUN for TOOL-REPAIR: damaged PDFs rebuilt into new files, as an app job.
//! Works on files, not on the open document, so the workspace is not touched.

use super::*;

/// Most files one repair run takes.
pub const MAX_REPAIR_FILES: usize = 200;

impl Backend {
    /// Rebuild the damaged PDFs in `files` (each next to its original, as `NAME.repaired.pdf`).
    pub fn repair_run(self: &Arc<Self>, files: Vec<PathBuf>) -> Result<u64> {
        self.ready()?;
        if files.is_empty() || files.len() > MAX_REPAIR_FILES {
            return Err(AppError::invalid("a repair run takes 1 to 200 files"));
        }
        Ok(self.spawn_job("repair", move |ctx| {
            let report = crate::repair::repair_files(&ctx.shared, &files, None, &|f, _| ctx.progress(f), ctx)?;
            for out in &report.quick.outputs {
                ctx.add_output(out);
            }
            if report.quick.outputs.is_empty() {
                // Nothing could be rebuilt: the first reason is the job's error.
                return Err(report
                    .quick
                    .failures
                    .into_iter()
                    .next()
                    .map_or_else(|| AppError::internal("nothing was produced"), |(_, e)| e));
            }
            Ok(JobOutput {
                outputs: report.quick.outputs.clone(),
                detail: json!({
                    "count": report.quick.outputs.len(),
                    "failed": report.quick.failures.len(),
                    "pages": report.repaired.iter().map(|r| u64::from(r.pages)).sum::<u64>(),
                    "damaged": report.repaired.iter().filter(|r| r.was_damaged).count(),
                }),
            })
        }))
    }
}
