//! `sheaf-harness`: drive the PH-1 core from the command line.
//!
//! ```text
//! sheaf-harness probe  FILE...
//! sheaf-harness export --out DIR [--name NAME] [--rotate DEG] [--reverse] [--drop 2,5] FILE...
//! sheaf-harness bench  [--pages N] [--enforce]
//! ```
//! Exit code 0 ok, 1 failed, 2 usage. Flags `--pdfium DIR` and `--workers N` apply to all.

use harness::App;
use sheaf_core::{AppError, Op, PageId, Result, Rot};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

struct Args {
    cmd: String,
    files: Vec<PathBuf>,
    out: Option<PathBuf>,
    name: String,
    rotate: i32,
    reverse: bool,
    drop: Vec<usize>,
    pages: u32,
    enforce: bool,
    pdfium: PathBuf,
    workers: usize,
}

fn parse() -> std::result::Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or("missing command")?;
    let auto = std::thread::available_parallelism()
        .map_or(1, |n| n.get().saturating_sub(1))
        .max(1);
    let mut a = Args {
        cmd,
        files: vec![],
        out: None,
        name: "export.pdf".into(),
        rotate: 0,
        reverse: false,
        drop: vec![],
        pages: 100,
        enforce: false,
        pdfium: fixture_gen::vendored_pdfium_dir(),
        workers: auto,
    };
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--out" => a.out = Some(value("--out")?.into()),
            "--name" => a.name = value("--name")?,
            "--rotate" => {
                a.rotate = value("--rotate")?
                    .parse()
                    .map_err(|_| "--rotate needs degrees")?
            }
            "--reverse" => a.reverse = true,
            "--drop" => {
                for n in value("--drop")?.split(',') {
                    a.drop.push(
                        n.trim()
                            .parse()
                            .map_err(|_| "--drop needs page numbers like 2,5")?,
                    );
                }
            }
            "--pages" => {
                a.pages = value("--pages")?
                    .parse()
                    .map_err(|_| "--pages needs a number")?
            }
            "--enforce" => a.enforce = true,
            "--pdfium" => a.pdfium = value("--pdfium")?.into(),
            "--workers" => {
                a.workers = value("--workers")?
                    .parse()
                    .map_err(|_| "--workers needs a number")?
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            file => a.files.push(file.into()),
        }
    }
    Ok(a)
}

fn start(a: &Args) -> Result<App> {
    let base = std::env::temp_dir().join("sheaf-harness");
    App::start(&harness::sibling_worker()?, &a.pdfium, &base, a.workers)
}

fn probe(a: &Args) -> Result<()> {
    let mut app = start(a)?;
    for file in &a.files {
        let report = app.import(file)?;
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if report.encrypted {
            println!("{name}: locked (password needed)");
            continue;
        }
        let with_text = report
            .pages
            .iter()
            .filter(|id| app.ws.find(**id).is_some_and(|p| p.has_text == Some(true)))
            .count();
        println!(
            "{name}: {} pages, {with_text} with a text layer",
            report.pages.len()
        );
    }
    app.close()
}

fn export(a: &Args) -> Result<()> {
    let out = a
        .out
        .as_ref()
        .ok_or_else(|| AppError::invalid("--out DIR is required"))?;
    if a.files.is_empty() {
        return Err(AppError::invalid("no input files"));
    }
    let mut app = start(a)?;
    for file in &a.files {
        if app.import(file)?.encrypted {
            return Err(AppError::new(
                sheaf_core::ErrorCode::NeedsPassword,
                "an input file is locked",
            ));
        }
    }
    let ids: Vec<PageId> = app.ws.all_pages().map(|p| p.id).collect();
    // Page numbers refer to the imported order, as the user sees it.
    let dropped: Vec<PageId> = a
        .drop
        .iter()
        .map(|&n| {
            n.checked_sub(1)
                .and_then(|i| ids.get(i).copied())
                .ok_or_else(|| AppError::invalid("--drop: no such page"))
        })
        .collect::<Result<_>>()?;
    if !dropped.is_empty() {
        app.ws.apply(Op::Delete { ids: dropped })?; // TOOL-REMOVE
    }
    if a.reverse {
        // TOOL-ORGANIZE: move the last page to the front until the order is reversed.
        for i in 0..app.ws.len() {
            let last = app.ws.pages(app.ws.len() - 1..app.ws.len())[0].id;
            app.ws.apply(Op::Move {
                ids: vec![last],
                to: i,
            })?;
        }
    }
    let delta = Rot::from_degrees(a.rotate)?;
    if delta != Rot::R0 {
        let all = app.ws.all_pages().map(|p| p.id).collect();
        app.ws.apply(Op::Rotate { ids: all, delta })?; // TOOL-ROTATE
    }
    let (path, built) = app.export(out, &a.name)?;
    println!("{}", path.display());
    eprintln!(
        "{} pages ({} copied, {} rasterized)",
        built.page_count, built.copied, built.rasterized
    );
    app.close()
}

/// PERF-BUDGETS, the part PH-1 can measure: opening a 100-page PDF, its
/// first 20 thumbnails, and a lossless export.
fn bench(a: &Args) -> Result<()> {
    const THUMBS_BUDGET_MS: u128 = 500;
    const EXPORT_BUDGET_MS: u128 = 5_000;
    let scratch = fixture_gen::Scratch::new("bench");
    let file = scratch.file("bench.pdf", &fixture_gen::text_pdf(a.pages));
    let mut app = start(a)?;

    let t = Instant::now();
    let report = app.import(&file)?;
    let open_ms = t.elapsed().as_millis();
    let first: Vec<PageId> = report.pages.iter().copied().take(20).collect();
    let thumbs = app.thumbs(&first)?;
    let thumbs_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let all: Vec<PageId> = app.ws.all_pages().map(|p| p.id).collect();
    app.ws.apply(Op::Rotate {
        ids: all[..all.len() / 2].to_vec(),
        delta: Rot::R90,
    })?;
    let (out, built) = app.export(scratch.path(), "bench-out.pdf")?;
    let export_ms = t.elapsed().as_millis();
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    app.close()?;

    println!("pages                          {}", a.pages);
    println!("workers                        {}", a.workers);
    println!("open (probe, cold worker)      {open_ms} ms");
    println!(
        "open + first {} thumbnails     {thumbs_ms} ms   (budget {THUMBS_BUDGET_MS} ms)",
        thumbs.len()
    );
    println!("export, half the pages rotated {export_ms} ms   (budget {EXPORT_BUDGET_MS} ms)");
    println!(
        "output                         {} pages, {} copied, {size} bytes",
        built.page_count, built.copied
    );
    let within =
        thumbs_ms <= THUMBS_BUDGET_MS && export_ms <= EXPORT_BUDGET_MS && built.copied == a.pages;
    println!(
        "within budget                  {}",
        if within { "yes" } else { "NO" }
    );
    if a.enforce && !within {
        return Err(AppError::limit("performance budget exceeded"));
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "sheaf-harness: {e}\nusage: sheaf-harness probe|export|bench [flags] [FILE...]"
            );
            return ExitCode::from(2);
        }
    };
    let result = match args.cmd.as_str() {
        "probe" => probe(&args),
        "export" => export(&args),
        "bench" => bench(&args),
        other => {
            eprintln!("sheaf-harness: unknown command {other}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sheaf-harness: {e}");
            ExitCode::FAILURE
        }
    }
}
