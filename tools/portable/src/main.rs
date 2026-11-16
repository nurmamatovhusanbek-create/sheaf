//! `sheaf`: the whole program in one executable.
//!
//! Two roles, one file. Started with `--session …` it is a worker: the
//! process that opens documents (SEC-ISOLATE), launched by the main process
//! exactly like the separate `sheaf-worker` binary. Otherwise it is the
//! command line (`harness::cli`). The engines (PDFium, Tesseract, language
//! data, font) are embedded and unpacked once into the per-user data folder,
//! verified against hashes compiled in (`payload`).

mod payload;

use harness::Engines;
use sheaf_core::{AppError, Result};
use sheaf_store::BundleGuard;
use std::process::ExitCode;
use std::sync::OnceLock;

fn engines() -> Result<Engines> {
    // Held for the life of the process: on Windows the files stay locked.
    static GUARD: OnceLock<BundleGuard> = OnceLock::new();
    let (dir, guard) = payload::install(&sheaf_store::default_base_dir()?)?;
    let _ = GUARD.set(guard);
    let exe = std::env::current_exe().map_err(|e| AppError::io("locate executable", &e))?;
    // Workers are this same file, started with `--session` first.
    Ok(Engines::from_vendor(exe, &dir))
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|first| first == "--session") {
        return sheaf_worker::run(args);
    }
    harness::cli::run(engines)
}
