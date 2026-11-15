//! MOD-SHELL: the `sheaf` executable (ADR-NATIVE-UI). One binary, several
//! modes (CLI-GRAMMAR): the window, headless quick actions, the resident tray
//! helper and the action wheel overlay.

// No console window in release builds; the headless mode prints through the parent's console if any.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let command = match sheaf_cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("sheaf: {e}\n{}", sheaf_cli::USAGE);
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
            eprintln!("sheaf: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run_tray() -> ExitCode {
    match tray::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sheaf: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_tray() -> ExitCode {
    eprintln!("sheaf: the tray helper needs Windows");
    ExitCode::FAILURE
}
