//! `sheaf` in one file: the app, its worker and its engines.
//!
//! One executable, three roles. Started with `--session …` it is a worker, the process that opens
//! documents (SEC-ISOLATE), launched by the app like the separate `sheaf-worker` binary. Started
//! with `--scan-helper` it is the scanner helper (SCAN-ISOLATE). Otherwise it is the program
//! (`sheaf_shell::run`). The engines (PDFium, Tesseract, language data, font, licence texts) are
//! embedded and unpacked once into the per-user data folder, verified against hashes compiled in
//! (`payload`), and the program is told to find them there and to use itself as its worker.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod payload;

use std::process::ExitCode;

fn main() -> ExitCode {
    let first = std::env::args_os().nth(1);
    match first.as_deref().and_then(|a| a.to_str()) {
        Some("--session") => return sheaf_worker::run(std::env::args_os().skip(1)),
        // The scanner helper needs no engines; the shell handles it before anything else.
        Some(flag) if flag == sheaf_app::SCAN_HELPER_FLAG => return sheaf_shell::run(),
        _ => {}
    }
    match sheaf_store::default_base_dir().and_then(|base| payload::install(&base)) {
        Ok(dir) => {
            sheaf_app::set_vendor_dir(dir);
            sheaf_app::use_this_executable_as_worker();
        }
        // Left as is, the program reports that its engines are missing and says what to try.
        Err(e) => eprintln!("sheaf: could not prepare the engines: {e}"),
    }
    sheaf_shell::run()
}
