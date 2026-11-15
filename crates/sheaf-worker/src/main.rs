//! MOD-WORKER: the worker binary. Reads DATA-WORKERPROTO requests on stdin,
//! writes responses on stdout. This is the only process that opens documents
//! (SEC-ISOLATE); it writes nothing outside the session dir (SEC-PATHS).
//!
//! `sheaf-worker --session <dir> --pdfium <dir> [--tesseract <exe> --tessdata <dir>] [--font <ttf>]`
//!
//! Ops: `ping`, `import_probe`, `thumb`, `render`, `build` (with stamps,
//! compression and protection), `ocr_page`, `page_text`, `page_facts`,
//! `export_images`, `extract_images`.

mod handlers;

use handlers::{Bundled, Worker};
use sheaf_core::proto::{Request, Response};
use sheaf_core::AppError;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn say(resp: &Response) {
    let mut out = std::io::stdout().lock();
    if let Ok(line) = serde_json::to_string(resp) {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

const USAGE: &str =
    "usage: sheaf-worker --session <dir> --pdfium <dir> [--tesseract <exe> --tessdata <dir>] [--font <ttf>]";

fn main() -> ExitCode {
    sheaf_store::harden_process();
    sheaf_store::harden_worker();
    let mut session = None;
    let mut pdfium = None;
    let mut tesseract = None;
    let mut tessdata = None;
    let mut font = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        match (flag.to_str(), args.next()) {
            (Some("--session"), Some(v)) => session = Some(PathBuf::from(v)),
            (Some("--pdfium"), Some(v)) => pdfium = Some(PathBuf::from(v)),
            (Some("--tesseract"), Some(v)) => tesseract = Some(PathBuf::from(v)),
            (Some("--tessdata"), Some(v)) => tessdata = Some(PathBuf::from(v)),
            (Some("--font"), Some(v)) => font = Some(PathBuf::from(v)),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(session), Some(pdfium)) = (session, pdfium) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut worker = match Worker::new(
        &session,
        pdfium,
        Bundled {
            tess: tesseract.zip(tessdata),
            font,
        },
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("sheaf-worker: {e}");
            return ExitCode::from(2);
        }
    };

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(_) => {
                say(&Response::failed(0, AppError::invalid("request is not valid JSON")));
                continue;
            }
        };
        let id = req.id;
        // No panic crosses the process boundary as a dead pipe if we can help it.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            worker.handle(&req, &mut |p| say(&Response::progress(id, p)))
        }));
        match outcome {
            Ok(Ok(result)) => say(&Response::done(id, result)),
            Ok(Err(e)) => say(&Response::failed(id, e)),
            Err(_) => {
                say(&Response::failed(id, AppError::internal("handler panicked")));
                // State after a panic is unknown: exit and let the pool start a fresh worker.
                return ExitCode::from(3);
            }
        }
    }
    ExitCode::SUCCESS
}
