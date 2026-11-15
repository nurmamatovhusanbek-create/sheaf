//! TOOL-REPAIR: rebuild a damaged PDF into a new file. The file is read by a
//! worker, never by the main process (a damaged file is the most hostile kind of
//! input); the rebuilt file is opened again by the renderer and the page count
//! reported is the one the renderer sees, so a repair never claims more than
//! was recovered. The result goes next to the original under a new name and
//! nothing is overwritten (ADR-NO-OVERWRITE). Locked files are not repaired.

use crate::plan::{encode, JobCtl, Shared};
use crate::quick::QuickReport;
use sheaf_core::proto::{ops, RepairArgs, RepairResult};
use sheaf_core::{AppError, ErrorCode, Priority, Result};
use sheaf_jobs::JobSpec;
use std::path::{Path, PathBuf};

/// One repaired file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repaired {
    pub output: PathBuf,
    /// Pages in the rebuilt file, as the renderer sees them.
    pub pages: u32,
    /// The original did not open cleanly, or opened with another page count.
    pub was_damaged: bool,
}

#[derive(Debug, Default)]
pub struct RepairReport {
    pub quick: QuickReport,
    pub repaired: Vec<Repaired>,
}

/// Repair `files`. `out` replaces the folder next to each input. `progress` gets a
/// fraction and a file name; `ctl` supplies cancellation between files.
pub fn repair_files(
    shared: &Shared,
    files: &[PathBuf],
    out: Option<&Path>,
    progress: &dyn Fn(f32, &str),
    ctl: &dyn JobCtl,
) -> Result<RepairReport> {
    if files.is_empty() {
        return Err(AppError::invalid("no files given"));
    }
    let fixed = match out {
        Some(dir) => Some(sheaf_store::check_output_dir(dir)?),
        None => None,
    };
    let mut report = RepairReport::default();
    let total = files.len() as f32;
    for (i, file) in files.iter().enumerate() {
        if ctl.cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
        }
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        progress(i as f32 / total, &name);
        match one(shared, file, fixed.as_deref(), ctl) {
            Ok(done) => {
                report.quick.bytes_before += std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                report.quick.bytes_after += std::fs::metadata(&done.output).map(|m| m.len()).unwrap_or(0);
                report.quick.outputs.push(done.output.clone());
                report.repaired.push(done);
            }
            Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
            Err(e) => report.quick.failures.push((file.clone(), e)),
        }
    }
    progress(1.0, "");
    Ok(report)
}

fn one(shared: &Shared, file: &Path, fixed: Option<&Path>, ctl: &dyn JobCtl) -> Result<Repaired> {
    // The same path policy as every import: a real file, within the size limit, not a device.
    let allowed = sheaf_store::allow_input(file)?;
    if allowed.is_unc && !shared.allow_network() {
        return Err(AppError::new(
            ErrorCode::PathRejected,
            "files on network shares are turned off in Settings",
        ));
    }
    let out_dir = match fixed {
        Some(dir) => dir.to_path_buf(),
        None => file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .canonicalize()
            .map(sheaf_store::paths::simplify_verbatim)
            .map_err(|e| AppError::io("resolve output folder", &e))?,
    };
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staged = format!("results/repair-{}.pdf", shared.next());
    let args = encode(&RepairArgs {
        path: allowed
            .path
            .to_str()
            .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))?
            .to_string(),
        out: staged.clone(),
    })?;
    let done: RepairResult = shared.run_typed(JobSpec::new(ops::REPAIR, args, Priority::Selection), ctl)?;
    let staged_path = shared.session.resolve(&staged)?;
    let published = sheaf_store::publish(&staged_path, &out_dir, &format!("{stem}.repaired.pdf"));
    let _ = std::fs::remove_file(&staged_path);
    Ok(Repaired {
        output: published?,
        pages: done.pages,
        was_damaged: done.was_damaged,
    })
}
