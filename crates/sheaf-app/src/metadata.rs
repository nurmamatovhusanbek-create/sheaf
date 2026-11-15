//! TOOL-METADATA: what a PDF says about itself, and changed copies of it. A worker reads
//! the file (never the main process); the copy is written next to the original under a new
//! name and nothing is overwritten (ADR-NO-OVERWRITE). The information returned for a copy
//! is read back from the file that was written.

use crate::plan::{encode, JobCtl, Shared};
use sheaf_core::proto::{ops, MetaEdit, MetadataArgs, MetadataInfo, MetadataResult};
use sheaf_core::{AppError, ErrorCode, Priority, Result};
use sheaf_jobs::JobSpec;
use std::path::{Path, PathBuf};

fn checked(shared: &Shared, file: &Path) -> Result<String> {
    let allowed = sheaf_store::allow_input(file)?;
    if allowed.is_unc && !shared.allow_network() {
        return Err(AppError::new(
            ErrorCode::PathRejected,
            "files on network shares are turned off in Settings",
        ));
    }
    allowed
        .path
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::new(ErrorCode::PathRejected, "path is not valid Unicode"))
}

/// What `file` says about itself.
pub fn read(shared: &Shared, file: &Path, ctl: &dyn JobCtl) -> Result<MetadataInfo> {
    let args = encode(&MetadataArgs {
        path: checked(shared, file)?,
        out: None,
        edit: None,
    })?;
    let done: MetadataResult = shared.run_typed(JobSpec::new(ops::METADATA, args, Priority::Selection), ctl)?;
    Ok(done.info)
}

/// A changed copy of `file`: `NAME.clean.pdf` after a strip, `NAME.edited.pdf` after a set. `out`
/// replaces the folder next to the original. Returns the new file and what is in it.
pub fn edit(
    shared: &Shared,
    file: &Path,
    change: &MetaEdit,
    out: Option<&Path>,
    ctl: &dyn JobCtl,
) -> Result<(PathBuf, MetadataInfo)> {
    if let MetaEdit::Set(fields) = change {
        fields.validate()?;
    }
    let path = checked(shared, file)?;
    let out_dir = match out {
        Some(dir) => sheaf_store::check_output_dir(dir)?,
        None => file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .canonicalize()
            .map(sheaf_store::paths::simplify_verbatim)
            .map_err(|e| AppError::io("resolve output folder", &e))?,
    };
    let suffix = match change {
        MetaEdit::Strip => "clean",
        MetaEdit::Set(_) => "edited",
    };
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staged = format!("results/metadata-{}.pdf", shared.next());
    let args = encode(&MetadataArgs {
        path,
        out: Some(staged.clone()),
        edit: Some(change.clone()),
    })?;
    let done: MetadataResult = shared.run_typed(JobSpec::new(ops::METADATA, args, Priority::Selection), ctl)?;
    let staged_path = shared.session.resolve(&staged)?;
    let published = sheaf_store::publish(&staged_path, &out_dir, &format!("{stem}.{suffix}.pdf"));
    let _ = std::fs::remove_file(&staged_path);
    Ok((published?, done.info))
}
