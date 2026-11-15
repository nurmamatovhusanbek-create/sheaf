//! Writes a small born-digital PDF for smoke tests: `write-fixture <path> [pages]`.

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: write-fixture <path> [pages]");
        return std::process::ExitCode::from(64);
    };
    let pages = args.next().and_then(|n| n.parse().ok()).unwrap_or(3);
    match std::fs::write(&path, fixture_gen::text_pdf(pages)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("write-fixture: {:?}", e.kind());
            std::process::ExitCode::FAILURE
        }
    }
}
