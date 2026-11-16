//! FLOW-PRESET on files that exist already: the stages of a preset run on
//! each input file in turn, through the normal import and export, so the path
//! policy, limits and worker isolation all apply and nothing is overwritten
//! (ADR-NO-OVERWRITE). One bad file does not stop the others. The scan stage
//! is skipped here: there is no scanner for files from Explorer.

use crate::quick::QuickReport;
use crate::{App, ExportSpec};
use sheaf_core::{AppError, ErrorCode, Op, PageId, Preset, Result};
use std::path::{Path, PathBuf};

/// Run `preset` on `files`. `out` replaces the preset's own output folder
/// (`--out`); with neither, each output goes beside its input. `progress`
/// gets a fraction and a file name; `cancelled` is asked between files and
/// between stages.
pub fn run_preset(
    app: &mut App,
    preset: &Preset,
    files: &[PathBuf],
    out: Option<&Path>,
    progress: &dyn Fn(f32, &str),
    cancelled: &dyn Fn() -> bool,
) -> Result<QuickReport> {
    preset.validate()?;
    if files.is_empty() {
        return Err(AppError::invalid("no files given"));
    }
    let fixed_dir = match out
        .map(Path::to_path_buf)
        .or_else(|| preset.out_dir.as_ref().map(PathBuf::from))
    {
        Some(dir) => Some(sheaf_store::check_output_dir(&dir)?),
        None => None,
    };
    let mut report = QuickReport::default();
    let total = files.len() as f32;
    for (i, file) in files.iter().enumerate() {
        if cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
        }
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        progress(i as f32 / total, &name);
        let before = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        match one_file(app, preset, file, fixed_dir.as_deref(), cancelled) {
            Ok(paths) => {
                report.bytes_before += before;
                report.bytes_after += paths
                    .iter()
                    .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
                    .sum::<u64>();
                report.outputs.extend(paths);
            }
            Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
            Err(e) => report.failures.push((file.clone(), e)),
        }
    }
    progress(1.0, "");
    Ok(report)
}

fn one_file(
    app: &mut App,
    preset: &Preset,
    file: &Path,
    fixed_dir: Option<&Path>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<PathBuf>> {
    let out_dir = match fixed_dir {
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
    let imported = app.import(file)?;
    if imported.encrypted {
        // No window to ask in: a locked file is reported, not guessed at.
        return Err(AppError::new(ErrorCode::NeedsPassword, "the file is locked"));
    }
    let ids: Vec<PageId> = imported.pages;

    // Cleanup: how the pages look. Applied when the output is built.
    let filter = preset.filter();
    if !filter.is_identity() {
        app.ws.apply(Op::SetFilter {
            ids: ids.clone(),
            filter,
        })?;
    }
    if cancelled() {
        return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
    }

    // OCR: pages that already have text are skipped; a failure is an error, not a
    // file that looks searchable and is not.
    if preset.recognizes() {
        let langs = preset.ocr.langs.clone().filter(|l| !l.is_empty());
        if let Some(l) = &langs {
            sheaf_ocr::langs::validate(l)?;
        }
        let outcomes = app.ocr(&ids, langs, false)?;
        let failed = outcomes.iter().find_map(|(_, o)| match o {
            crate::OcrOutcome::Failed(e) => Some(e.clone()),
            _ => None,
        });
        if let Some(e) = failed {
            return Err(e);
        }
        if cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
        }
    }

    let spec = ExportSpec {
        pages: Some(ids),
        split: preset.split_rule()?,
        name: preset.name_pattern.clone(),
        compress: preset.compress_level(),
        drop_blank: preset.cleanup.drop_blank,
        ..ExportSpec::default()
    };
    Ok(app.export_pdf(&out_dir, &spec)?.into_iter().map(|d| d.path).collect())
}
