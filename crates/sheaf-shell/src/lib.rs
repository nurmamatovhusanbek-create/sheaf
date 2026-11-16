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

fn ui_config(files: Vec<PathBuf>, previous_run_ended_badly: bool) -> UiConfig {
    UiConfig {
        previous_run_ended_badly,
        engines: Engines::discover(&[]),
        base_dir: base_dir(),
        config_dir: sheaf_store::config_dir().ok(),
        files,
    }
}

/// Run the mode the arguments ask for.
pub fn run() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // The scanner helper (SCAN-ISOLATE) comes first and bare: no DLL-search or image-load
    // restrictions, because scanner drivers load their own parts from their own folders, and no
    // crash dialog, because the app that started it reports what happened.
    if args.first().is_some_and(|a| a == sheaf_app::SCAN_HELPER_FLAG) {
        return ExitCode::from(sheaf_app::run_scan_helper(&args[1..]) as u8);
    }
    sheaf_store::harden_process();
    crash::install();
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
        Command::Window { files } => {
            let again = Command::Window { files: files.clone() };
            // What an update left beside this program (the old copy, a half-finished download).
            sheaf_app::update::clean_up_after_update();
            // A mark that says "running" until the window closes properly; if it is still there at the
            // next start, that run ended some other way and the person is told (see RunMark). It is
            // cleared before a relaunch with the fallback renderer, which makes its own.
            let mark = sheaf_store::config_dir()
                .ok()
                .map(|dir| sheaf_store::RunMark::begin(&dir));
            let ended_badly = mark
                .as_ref()
                .is_some_and(sheaf_store::RunMark::previous_run_ended_badly);
            let result = sheaf_ui::run_window(ui_config(files, ended_badly));
            if let Some(mark) = mark {
                mark.finish();
            }
            // Closing the window means closing the program: the action-wheel helper the window may have
            // started goes too, unless "Start with Windows" says it should stay (and not when this was
            // only a failed start that is about to be retried with another renderer).
            #[cfg(windows)]
            if !sheaf_ui::opengl_unusable(&result) {
                tray::stop_helper_unless_resident();
            }
            // An update was put in place: start the new program now that this one is gone.
            if result.is_ok() {
                if let Some(exe) = sheaf_ui::take_restart_request() {
                    let _ = sheaf_app::update::restart(&exe);
                }
            }
            finish_or_retry(result, &again)
        }
        Command::Run(request) => headless::run(&request),
        Command::Preset(run) => headless::preset(&run),
        Command::Presets => headless::presets(),
        Command::Repair(run) => headless::repair(&run),
        Command::Info(run) => headless::info(&run),
        Command::InstallVerbs => run_verbs(true),
        Command::UninstallVerbs => run_verbs(false),
        Command::Wheel { at, list } => {
            let files = sheaf_ui::take_file_list(&list);
            let result = sheaf_ui::run_wheel(wheel_config(at, files.clone(), None));
            if sheaf_ui::opengl_unusable(&result) {
                // The list was taken when the wheel started: a fresh one for the second start.
                return match write_list(&files) {
                    Some(list) => relaunch(&[
                        "wheel".into(),
                        "--at".into(),
                        format!("{},{}", at.0, at.1).into(),
                        "--files".into(),
                        list.into_os_string(),
                    ]),
                    None => finish(result),
                };
            }
            finish(result)
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
            let files = job.files().to_vec();
            let at = cursor::position();
            let again = match &job {
                sheaf_ui::WheelJob::Quick(r) => Command::Run(r.clone()),
                sheaf_ui::WheelJob::Preset { name, files, out } => Command::Preset(sheaf_cli::PresetRun {
                    name: name.clone(),
                    files: files.clone(),
                    out: out.clone(),
                }),
            };
            finish_or_retry(
                sheaf_ui::run_wheel(wheel_config(at, files, Some(job))),
                &Command::Card(Box::new(again)),
            )
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

/// Finish a window, and when OpenGL could not be used start the same command again drawing
/// through Direct3D 12 (once: the second program sees the variable and does not ask again).
fn finish_or_retry(result: sheaf_ui::UiResult, command: &Command) -> ExitCode {
    if sheaf_ui::opengl_unusable(&result) {
        if let Some(args) = command.to_args() {
            return relaunch(&args);
        }
    }
    finish(result)
}

/// Start this program again with the same arguments and the fallback renderer, and wait for it.
fn relaunch(args: &[std::ffi::OsString]) -> ExitCode {
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    match std::process::Command::new(exe)
        .args(args)
        .env(sheaf_ui::RENDERER_ENV, "wgpu")
        .status()
    {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(_) => ExitCode::FAILURE,
    }
}

/// A private list of files for a second start of the wheel overlay.
fn write_list(files: &[PathBuf]) -> Option<PathBuf> {
    let dir = base_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("wheel-{}-retry-{nanos:x}.txt", std::process::id()));
    let text: String = files.iter().map(|f| format!("{}\n", f.display())).collect();
    std::fs::write(&path, text).ok()?;
    Some(path)
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
