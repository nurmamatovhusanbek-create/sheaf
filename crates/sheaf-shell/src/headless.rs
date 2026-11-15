//! `sheaf run <action> <files...>` and `sheaf preset <name> <files...>`: work with
//! no window. Prints one output path per line on stdout and problems on stderr;
//! exit code 0 all done, 2 partly, 1 nothing (CLI-GRAMMAR).

use sheaf_app::quick::{run_quick, QuickReport};
use sheaf_app::{App, Engines};
use sheaf_cli::{PresetRun, RepairRun};
use sheaf_core::{AppError, QuickRequest};
use std::process::ExitCode;

/// The engines, checked, and an app on them (SEC-SUPPLY: before any worker starts).
fn start_app() -> Result<App, AppError> {
    let engines = Engines::discover(&[])?;
    engines.verify()?;
    let base = sheaf_store::default_base_dir().unwrap_or_else(|_| std::env::temp_dir().join("Sheaf"));
    let workers = sheaf_store::Settings::default().worker_count();
    let app = App::start(&engines, &base, workers)?;
    if let Ok(dir) = sheaf_store::config_dir() {
        app.shared()
            .set_allow_network(sheaf_store::Settings::load(&dir).allow_network_paths);
    }
    Ok(app)
}

fn fail(e: &AppError) -> ExitCode {
    crate::say_err(&format!("sheaf: {e}"));
    ExitCode::FAILURE
}

/// Print what a run produced and how it went; the exit code says whether all of it worked.
fn finish(result: Result<QuickReport, AppError>) -> ExitCode {
    match result {
        Ok(report) => {
            for out in &report.outputs {
                crate::say(&out.display().to_string());
            }
            for (file, e) in &report.failures {
                let name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                crate::say_err(&format!("sheaf: {name}: {:?}", e.code));
            }
            ExitCode::from(report.exit_code())
        }
        Err(e) => fail(&e),
    }
}

pub fn run(request: &QuickRequest) -> ExitCode {
    let mut app = match start_app() {
        Ok(app) => app,
        Err(e) => return fail(&e),
    };
    let result = run_quick(&mut app, request, &|_, _| {});
    let _ = app.close();
    finish(result)
}

/// A saved preset on files.
pub fn preset(run: &PresetRun) -> ExitCode {
    let config = match sheaf_store::config_dir() {
        Ok(dir) => dir,
        Err(e) => return fail(&e),
    };
    let preset = match sheaf_store::presets::load(&config, &run.name) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    let mut app = match start_app() {
        Ok(app) => app,
        Err(e) => return fail(&e),
    };
    let result =
        sheaf_app::preset::run_preset(&mut app, &preset, &run.files, run.out.as_deref(), &|_, _| {}, &|| false);
    let _ = app.close();
    finish(result)
}

/// `sheaf repair FILES...`: rebuild damaged PDFs. The paths go to stdout; what was recovered, to stderr.
pub fn repair(run: &RepairRun) -> ExitCode {
    let app = match start_app() {
        Ok(app) => app,
        Err(e) => return fail(&e),
    };
    let result = sheaf_app::repair::repair_files(
        &app.shared(),
        &run.files,
        run.out.as_deref(),
        &|_, _| {},
        &sheaf_app::plan::NoCtl,
    );
    let _ = app.close();
    match result {
        Ok(report) => {
            for done in &report.repaired {
                let name = done
                    .output
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                crate::say_err(&format!(
                    "sheaf: {name}: {} pages recovered{}",
                    done.pages,
                    if done.was_damaged {
                        ""
                    } else {
                        " (the file was not damaged)"
                    }
                ));
            }
            finish(Ok(report.quick))
        }
        Err(e) => fail(&e),
    }
}

/// The saved presets, one per line.
pub fn presets() -> ExitCode {
    match sheaf_store::config_dir() {
        Ok(dir) => {
            for name in sheaf_store::presets::list(&dir) {
                crate::say(&name);
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}
