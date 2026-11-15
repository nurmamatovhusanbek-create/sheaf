//! MOD-SHELL: the `sheaf` programs (ADR-NATIVE-UI). One body of code, several
//! modes (CLI-GRAMMAR): the window, headless quick actions, the resident tray
//! helper and the action wheel overlay.
//!
//! Two executables share it. `sheaf.exe` is a Windows GUI program: no console
//! window when started from Explorer, the tray or a shortcut. `sheafc.exe` is
//! the same program built for the console: scripts and Explorer verbs use it,
//! because a GUI program neither prints into a pipe nor lets the shell wait
//! for its exit code.

mod headless;
#[cfg(windows)]
mod selection;
#[cfg(windows)]
mod tray;

use sheaf_app::Engines;
use sheaf_cli::Command;
use sheaf_ui::UiConfig;
use std::path::PathBuf;
use std::process::ExitCode;

fn base_dir() -> PathBuf {
    sheaf_store::default_base_dir().unwrap_or_else(|_| std::env::temp_dir().join("Sheaf"))
}

fn ui_config(files: Vec<PathBuf>) -> UiConfig {
    UiConfig {
        engines: Engines::discover(&[]),
        base_dir: base_dir(),
        config_dir: sheaf_store::config_dir().ok(),
        files,
    }
}

/// Run the mode the arguments ask for.
pub fn run() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let command = match sheaf_cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            say_err(&format!("sheaf: {e}\n{}", sheaf_cli::USAGE));
            return ExitCode::from(64);
        }
    };
    match command {
        Command::Window { files } => finish(sheaf_ui::run_window(ui_config(files))),
        Command::Run(request) => headless::run(&request),
        Command::Wheel { at, list } => {
            let files = sheaf_ui::take_file_list(&list);
            finish(sheaf_ui::run_wheel(sheaf_ui::WheelConfig {
                engines: Engines::discover(&[]),
                base_dir: base_dir(),
                config_dir: sheaf_store::config_dir().ok(),
                at,
                files,
            }))
        }
        Command::Tray => run_tray(),
    }
}

fn finish(result: sheaf_ui::UiResult) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_err(&format!("sheaf: {e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run_tray() -> ExitCode {
    match tray::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_err(&format!("sheaf: {e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_tray() -> ExitCode {
    say_err("sheaf: the tray helper needs Windows");
    ExitCode::FAILURE
}

/// Write a line to stderr; a closed or missing stream is not an error worth a crash.
pub(crate) fn say_err(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// Write a line to stdout, likewise.
pub(crate) fn say(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "{line}");
}
