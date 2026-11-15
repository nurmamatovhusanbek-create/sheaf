//! MOD-SHELL: the `sheaf` programs (ADR-NATIVE-UI). One body of code, several
//! modes (CLI-GRAMMAR): the window, headless quick actions, the resident tray
//! helper and the action wheel overlay.
//!
//! Two executables share it. `sheaf.exe` is a Windows GUI program: no console
//! window when started from Explorer, the tray or a shortcut. `sheafc.exe` is
//! the same program built for the console: scripts and Explorer verbs use it,
//! because a GUI program neither prints into a pipe nor lets the shell wait
//! for its exit code.

mod aggregate;
mod crash;
mod cursor;
mod headless;
#[cfg(windows)]
mod selection;
#[cfg(windows)]
mod tray;
mod verbs;

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
    sheaf_store::harden_process();
    crash::install();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let command = match sheaf_cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            say_err(&format!("sheaf: {e}\n{}", sheaf_cli::USAGE));
            return ExitCode::from(64);
        }
    };
    dispatch(command)
}

/// Where the processes Explorer starts for one selection leave their file lists.
fn verbs_dir() -> PathBuf {
    base_dir().join("verbs")
}

fn dispatch(command: Command) -> ExitCode {
    match command {
        Command::Collected(inner) => {
            // FLOW-EXPLORER: one action for the whole selection, however many processes Explorer started.
            let joined = aggregate::join(
                &verbs_dir(),
                &inner.collect_key(),
                inner.files(),
                &aggregate::Timing::default(),
            );
            match joined {
                Ok(aggregate::Joined::Leader(all)) => dispatch(inner.with_files(all)),
                Ok(aggregate::Joined::Follower) => ExitCode::SUCCESS,
                // No place to collect in: do this process's own files.
                Err(_) => dispatch(*inner),
            }
        }
        Command::Window { files } => finish(sheaf_ui::run_window(ui_config(files))),
        Command::Run(request) => headless::run(&request),
        Command::Preset(run) => headless::preset(&run),
        Command::Presets => headless::presets(),
        Command::Repair(run) => headless::repair(&run),
        Command::InstallVerbs => run_verbs(true),
        Command::UninstallVerbs => run_verbs(false),
        Command::Wheel { at, list } => {
            let files = sheaf_ui::take_file_list(&list);
            finish(sheaf_ui::run_wheel(wheel_config(at, files, None)))
        }
        Command::Card(inner) => {
            // An Explorer verb: the work starts at once and its progress and result show in a card.
            let job = match *inner {
                Command::Run(request) => sheaf_ui::WheelJob::Quick(request),
                Command::Preset(run) => sheaf_ui::WheelJob::Preset {
                    name: run.name,
                    files: run.files,
                    out: run.out,
                },
                _ => return ExitCode::from(64),
            };
            finish(sheaf_ui::run_wheel(wheel_config(
                cursor::position(),
                job.files().to_vec(),
                Some(job),
            )))
        }
        Command::Tray => run_tray(),
    }
}

fn wheel_config(at: (i32, i32), files: Vec<PathBuf>, job: Option<sheaf_ui::WheelJob>) -> sheaf_ui::WheelConfig {
    sheaf_ui::WheelConfig {
        engines: Engines::discover(&[]),
        base_dir: base_dir(),
        config_dir: sheaf_store::config_dir().ok(),
        at,
        files,
        job,
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

/// `sheaf install-verbs` / `uninstall-verbs`: the right-click menu (FLOW-EXPLORER).
#[cfg(windows)]
fn run_verbs(install: bool) -> ExitCode {
    let result = if install {
        let config = sheaf_store::config_dir().ok();
        let lang = config
            .as_deref()
            .map_or("en".to_string(), |d| sheaf_store::Settings::load(d).language);
        let presets = config.as_deref().map(sheaf_store::presets::list).unwrap_or_default();
        std::env::current_exe()
            .map_err(|e| sheaf_core::AppError::io("find sheaf.exe", &e))
            .and_then(|exe| verbs::install(&exe.with_file_name("sheaf.exe"), &lang, &presets))
    } else {
        verbs::uninstall()
    };
    match result {
        Ok(()) => {
            say(if install {
                "the Sheaf entry is in the right-click menu"
            } else {
                "the Sheaf entry is gone from the right-click menu"
            });
            ExitCode::SUCCESS
        }
        Err(e) => {
            say_err(&format!("sheaf: {e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_verbs(_install: bool) -> ExitCode {
    say_err("sheaf: the right-click menu is for Windows Explorer");
    ExitCode::FAILURE
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
