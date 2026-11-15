//! Quick actions with no window (ADR-WHEEL, CLI-GRAMMAR): what the action
//! wheel and `sheaf run` do to a set of files. Each action imports the files
//! through the normal path (so the path policy, size limits and worker
//! isolation all apply), builds its output with the normal export, and never
//! overwrites anything (ADR-NO-OVERWRITE). One bad file does not stop the others.

use crate::{App, ExportSpec};
use sheaf_core::proto::ImageFormat;
use sheaf_core::{AppError, ErrorCode, Op, PageId, QuickAction, QuickRequest, Result, Rot, SplitRule};
use std::path::{Path, PathBuf};

/// What a quick action did.
#[derive(Debug, Default)]
pub struct QuickReport {
    /// Files and folders written, in order.
    pub outputs: Vec<PathBuf>,
    /// Inputs that could not be processed, with why (no document text in the message).
    pub failures: Vec<(PathBuf, AppError)>,
    /// Sizes before and after, for compression.
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl QuickReport {
    /// Exit code of the headless command: 0 all done, 2 partly, 1 nothing.
    pub fn exit_code(&self) -> u8 {
        match (self.outputs.is_empty(), self.failures.is_empty()) {
            (_, true) => 0,
            (false, false) => 2,
            (true, false) => 1,
        }
    }
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Import one file; a locked one is reported, not asked about (no window to ask in).
fn import_one(app: &mut App, file: &Path) -> Result<Vec<PageId>> {
    let report = app.import(file)?;
    if report.encrypted {
        return Err(AppError::new(ErrorCode::NeedsPassword, "the file is locked"));
    }
    Ok(report.pages)
}

/// Run `request`. `progress(fraction, file_name)` is called as inputs complete.
pub fn run_quick(app: &mut App, request: &QuickRequest, progress: &dyn Fn(f32, &str)) -> Result<QuickReport> {
    let first = request
        .files
        .first()
        .ok_or_else(|| AppError::invalid("no files given"))?;
    let out_dir = match &request.out {
        Some(dir) => sheaf_store::check_output_dir(dir)?,
        None => first
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .canonicalize()
            .map(sheaf_store::paths::simplify_verbatim)
            .map_err(|e| AppError::io("resolve output folder", &e))?,
    };
    let mut report = QuickReport::default();
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let total = request.files.len().max(1) as f32;

    match request.action {
        // Several inputs, one output.
        QuickAction::Merge | QuickAction::ImagesToPdf => {
            let mut ids = Vec::new();
            for (i, file) in request.files.iter().enumerate() {
                progress(i as f32 / total, &name(file));
                match import_one(app, file) {
                    Ok(mut pages) => ids.append(&mut pages),
                    Err(e) => report.failures.push((file.clone(), e)),
                }
            }
            if !ids.is_empty() {
                let spec = ExportSpec {
                    pages: Some(ids),
                    ..ExportSpec::named(if request.action == QuickAction::Merge {
                        "{source}.merged"
                    } else {
                        "{source}"
                    })
                };
                match app.export_pdf(&out_dir, &spec) {
                    Ok(done) => report.outputs.extend(done.into_iter().map(|d| d.path)),
                    Err(e) => report.failures.push((first.clone(), e)),
                }
            }
        }
        // One output per input.
        _ => {
            for (i, file) in request.files.iter().enumerate() {
                progress(i as f32 / total, &name(file));
                let before = size(file);
                let one = (|| -> Result<Vec<PathBuf>> {
                    let ids = import_one(app, file)?;
                    match request.action {
                        QuickAction::Compress => Ok(app
                            .export_pdf(
                                &out_dir,
                                &ExportSpec {
                                    pages: Some(ids),
                                    compress: Some(request.level),
                                    ..ExportSpec::named("{source}.compressed")
                                },
                            )?
                            .into_iter()
                            .map(|d| d.path)
                            .collect()),
                        QuickAction::Ocr => {
                            let outcomes = app.ocr(&ids, request.langs.clone(), false)?;
                            let failed = outcomes.iter().find_map(|(_, o)| match o {
                                crate::OcrOutcome::Failed(e) => Some(e.clone()),
                                _ => None,
                            });
                            if let Some(e) = failed {
                                // Say so rather than hand back a file that looks searchable and is not.
                                return Err(e);
                            }
                            Ok(app
                                .export_pdf(
                                    &out_dir,
                                    &ExportSpec {
                                        pages: Some(ids),
                                        ..ExportSpec::named("{source}.ocr")
                                    },
                                )?
                                .into_iter()
                                .map(|d| d.path)
                                .collect())
                        }
                        QuickAction::Split => Ok(app
                            .export_pdf(
                                &out_dir,
                                &ExportSpec {
                                    pages: Some(ids),
                                    split: SplitRule::EveryN(1),
                                    ..ExportSpec::named("{source}-{counter}")
                                },
                            )?
                            .into_iter()
                            .map(|d| d.path)
                            .collect()),
                        QuickAction::Rotate => {
                            app.ws.apply(Op::Rotate {
                                ids: ids.clone(),
                                delta: Rot::R90,
                            })?;
                            Ok(app
                                .export_pdf(
                                    &out_dir,
                                    &ExportSpec {
                                        pages: Some(ids),
                                        ..ExportSpec::named("{source}.rotated")
                                    },
                                )?
                                .into_iter()
                                .map(|d| d.path)
                                .collect())
                        }
                        QuickAction::PdfToImages => {
                            let stem = file
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            app.export_images(&out_dir, &format!("{stem}-images"), Some(ids), ImageFormat::Png, 150)
                                .map(|files| {
                                    files
                                        .first()
                                        .and_then(|f| f.parent())
                                        .map(|d| vec![d.to_path_buf()])
                                        .unwrap_or_default()
                                })
                        }
                        QuickAction::Merge | QuickAction::ImagesToPdf => unreachable_action(),
                    }
                })();
                match one {
                    Ok(paths) => {
                        report.bytes_before += before;
                        report.bytes_after += paths.iter().map(|p| size(p)).sum::<u64>();
                        report.outputs.extend(paths);
                    }
                    Err(e) => report.failures.push((file.clone(), e)),
                }
            }
        }
    }
    progress(1.0, "");
    Ok(report)
}

fn unreachable_action() -> Result<Vec<PathBuf>> {
    Err(AppError::internal("action handled elsewhere"))
}
