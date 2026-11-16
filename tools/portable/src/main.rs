//! `sheaf`: the whole program in one executable.
//!
//! Three ways to start it, one file:
//!
//! - `--session …` first: a worker, the process that opens documents
//!   (SEC-ISOLATE), launched by the main process like the separate
//!   `sheaf-worker` binary.
//! - Files dropped on it, or double-clicked with nothing: the friendly entry
//!   for a program that has no window yet. Dropped files are made searchable
//!   (OCR) into a new `<name>.ocr.pdf` beside each one, and the window waits
//!   for Enter so the result can be read.
//! - Anything else: the command line (`harness::cli`).
//!
//! The engines (PDFium, Tesseract, language data, font) are embedded and
//! unpacked once into the per-user data folder, verified against hashes
//! compiled in (`payload`).

mod payload;

use harness::Engines;
use sheaf_core::{AppError, Result};
use sheaf_store::BundleGuard;
use std::ffi::OsString;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

const HELP: &str = "\
Sheaf: offline PDF and scan toolkit (test build, no window yet)

EASIEST: drag one or more PDF / image files onto sheaf.exe.
  Each one is made searchable (OCR) and saved next to the original as
  <name>.ocr.pdf. Originals are never changed or overwritten.

FROM POWERSHELL (open it in this folder):
  .\\sheaf.exe ocr    --out C:\\out C:\\scans\\page1.png        searchable PDF
  .\\sheaf.exe ocr    --out C:\\out --text-only page1.png        text file only
  .\\sheaf.exe ocr    --out C:\\out --langs rus+eng file.pdf     choose languages
  .\\sheaf.exe export --out C:\\out --name merged.pdf a.pdf b.jpg  merge (also --rotate 90, --drop 2)
  .\\sheaf.exe probe  file.pdf scan.png                          page counts, text layers
  .\\sheaf.exe bench                                             speed check

Languages: English, Russian, Uzbek (Latin and Cyrillic), picked automatically.
Nothing is sent over the network.
";

fn engines() -> Result<Engines> {
    // Held for the life of the process: on Windows the files stay locked.
    static GUARD: OnceLock<BundleGuard> = OnceLock::new();
    let (dir, guard) = payload::install(&sheaf_store::default_base_dir()?)?;
    let _ = GUARD.set(guard);
    let exe = std::env::current_exe().map_err(|e| AppError::io("locate executable", &e))?;
    // Workers are this same file, started with `--session` first.
    Ok(Engines::from_vendor(exe, &dir))
}

/// Keep a double-clicked window open until the person has read it.
fn wait_for_enter() {
    println!("\nPress Enter to close this window.");
    let _ = std::io::stdin().lock().lines().next();
}

/// Files dropped on the executable: OCR each one into a new file beside it.
fn dropped(files: Vec<PathBuf>) -> ExitCode {
    let mut worst = ExitCode::SUCCESS;
    for file in files {
        println!("\n== {}", file.display());
        let dir = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let args = vec![
            "ocr".to_string(),
            "--out".to_string(),
            dir.to_string_lossy().into_owned(),
            file.to_string_lossy().into_owned(),
        ];
        if harness::cli::run_args(args, engines) != ExitCode::SUCCESS {
            worst = ExitCode::FAILURE;
        }
    }
    wait_for_enter();
    worst
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|first| first == "--session") {
        return sheaf_worker::run(args);
    }
    let asks_for_help = args
        .first()
        .and_then(|a| a.to_str())
        .is_some_and(|a| matches!(a, "help" | "-h" | "--help" | "/?"));
    if args.is_empty() || asks_for_help {
        println!("{HELP}");
        if args.is_empty() {
            wait_for_enter();
        }
        return ExitCode::SUCCESS;
    }
    // Everything given is an existing file: it was dropped on the executable.
    if args.iter().all(|a| std::path::Path::new(a).is_file()) {
        return dropped(args.drain(..).map(PathBuf::from).collect());
    }
    harness::cli::run(engines)
}
