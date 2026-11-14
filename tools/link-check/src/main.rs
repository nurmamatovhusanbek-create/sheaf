//! `cargo run -p link-check [-- --write] [PATH]`
//!
//! Checks `ARCHITECTURE.md` (default path, relative to the workspace root).
//! `--write` refreshes the generated appendices in place before checking.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut write = false;
    let mut path: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--write" => write = true,
            "-h" | "--help" => {
                println!("usage: link-check [--write] [PATH]");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("link-check: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => path = Some(PathBuf::from(other)),
        }
    }
    let path = path
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ARCHITECTURE.md"));

    let mut doc = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("link-check: cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let mut report = link_check::check(&doc);
    if write && report.regenerated != doc {
        if let Err(e) = std::fs::write(&path, &report.regenerated) {
            eprintln!("link-check: cannot write {}: {e}", path.display());
            return ExitCode::from(2);
        }
        doc = report.regenerated;
        report = link_check::check(&doc);
        println!("link-check: appendices regenerated");
    }

    for e in &report.errors {
        eprintln!("link-check: {e}");
    }
    println!(
        "link-check: {} anchors, {} links, {} open VERIFY items, {} errors",
        report.anchors,
        report.links,
        report.verify_items,
        report.errors.len()
    );
    if report.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
