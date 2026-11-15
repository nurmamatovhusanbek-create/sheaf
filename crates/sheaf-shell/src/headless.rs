//! `sheaf run <action> <files...>`: quick actions with no window. Prints one
//! output path per line on stdout and problems on stderr; exit code 0 all done,
//! 2 partly, 1 nothing (CLI-GRAMMAR).

use sheaf_app::quick::run_quick;
use sheaf_app::{App, Engines};
use sheaf_core::QuickRequest;
use std::process::ExitCode;

pub fn run(request: &QuickRequest) -> ExitCode {
    let started = Engines::discover(&[]).and_then(|engines| {
        engines.verify()?; // SEC-SUPPLY: before any worker starts
        let base = sheaf_store::default_base_dir().unwrap_or_else(|_| std::env::temp_dir().join("Sheaf"));
        let workers = sheaf_store::Settings::default().worker_count();
        let app = App::start(&engines, &base, workers)?;
        if let Ok(dir) = sheaf_store::config_dir() {
            app.shared()
                .set_allow_network(sheaf_store::Settings::load(&dir).allow_network_paths);
        }
        Ok(app)
    });
    let mut app = match started {
        Ok(app) => app,
        Err(e) => {
            crate::say_err(&format!("sheaf: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let result = run_quick(&mut app, request, &|_, _| {});
    let _ = app.close();
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
        Err(e) => {
            crate::say_err(&format!("sheaf: {e}"));
            ExitCode::FAILURE
        }
    }
}
