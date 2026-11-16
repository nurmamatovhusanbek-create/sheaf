//! The command line of `sheaf-harness` and of the single-file build: drive the PH-1 core from the command line.
//!
//! ```text
//! sheaf-harness probe  FILE...
//! sheaf-harness export --out DIR [--name NAME] [--rotate DEG] [--reverse] [--drop 2,5] FILE...
//! sheaf-harness ocr    --out DIR [--langs eng+rus] [--replace] [--text-only] FILE
//! sheaf-harness bench  [--pages N] [--enforce]
//! ```
//! Exit code 0 ok, 2 partial (some pages failed), 1 failed, 64 usage.
//! Flag `--workers N` applies to all.

use crate::{App, Engines, OcrOutcome};
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
    workers: usize,
    langs: Option<Vec<String>>,
    replace: bool,
    text_only: bool,
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
        workers: auto,
        langs: None,
        replace: false,
        text_only: false,
    };
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .ok_or(format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--out" => a.out = Some(value("--out")?.into()),
            "--name" => a.name = value("--name")?,
            "--rotate" => a.rotate = value("--rotate")?.parse().map_err(|_| "--rotate needs degrees")?,
            "--reverse" => a.reverse = true,
            "--drop" => {
                for n in value("--drop")?.split(',') {
                    a.drop
                        .push(n.trim().parse().map_err(|_| "--drop needs page numbers like 2,5")?);
                }
            }
            "--pages" => a.pages = value("--pages")?.parse().map_err(|_| "--pages needs a number")?,
            "--enforce" => a.enforce = true,
            "--langs" => {
                let v = value("--langs")?;
                a.langs = (v != "auto").then(|| v.split('+').map(str::to_string).collect());
            }
            "--replace" => a.replace = true,
            "--text-only" => a.text_only = true,
            "--workers" => a.workers = value("--workers")?.parse().map_err(|_| "--workers needs a number")?,
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            file => a.files.push(file.into()),
        }
    }
    Ok(a)
}

/// How the running executable finds its engines.
static ENGINES: std::sync::OnceLock<fn() -> Result<Engines>> = std::sync::OnceLock::new();

fn start(a: &Args) -> Result<App> {
    // Per-user cache dir, not the shared temp dir (SEC-TEMP).
    let base = sheaf_store::default_base_dir()?;
    let find = ENGINES.get().copied().unwrap_or(Engines::vendored);
    App::start(&find()?, &base, a.workers)
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
        println!("{name}: {} pages, {with_text} with a text layer", report.pages.len());
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
            app.ws.apply(Op::Move { ids: vec![last], to: i })?;
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

/// FLOW-OCR-MANUAL, headless: add missing text (default), redo all
/// (`--replace`), or extract text only (`--text-only`). Writes `name.ocr.pdf`
/// or `name.ocr.txt` and never touches the input (ADR-NO-OVERWRITE).
fn ocr(a: &Args) -> Result<ExitCode> {
    let out = a
        .out
        .as_ref()
        .ok_or_else(|| AppError::invalid("--out DIR is required"))?;
    let [file] = a.files.as_slice() else {
        return Err(AppError::invalid("ocr takes exactly one file"));
    };
    let mut app = start(a)?;
    let report = app.import(file)?;
    if report.encrypted {
        return Err(AppError::new(
            sheaf_core::ErrorCode::NeedsPassword,
            "the input file is locked",
        ));
    }
    let outcomes = app.ocr(&report.pages, a.langs.clone(), a.replace)?;
    let (mut done, mut skipped, mut failed) = (0, 0, 0);
    for (_, outcome) in &outcomes {
        match outcome {
            OcrOutcome::Done(_) => done += 1,
            OcrOutcome::SkippedHasText => skipped += 1,
            OcrOutcome::Failed(_) => failed += 1,
        }
    }
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into());
    let path = if a.text_only {
        app.export_text(out, &format!("{stem}.ocr.txt"))?
    } else {
        app.export(out, &format!("{stem}.ocr.pdf"))?.0
    };
    println!("{}", path.display());
    eprintln!("{done} pages recognized, {skipped} already had text, {failed} failed");
    app.close()?;
    Ok(if failed > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
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
    let within = thumbs_ms <= THUMBS_BUDGET_MS && export_ms <= EXPORT_BUDGET_MS && built.copied == a.pages;
    println!("within budget                  {}", if within { "yes" } else { "NO" });
    if a.enforce && !within {
        return Err(AppError::limit("performance budget exceeded"));
    }
    Ok(())
}

/// Parse the process arguments and run the command. `engines` says where the
/// bundled engines are for this executable.
pub fn run(engines: fn() -> Result<Engines>) -> ExitCode {
    let _ = ENGINES.set(engines);
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("sheaf-harness: {e}\nusage: sheaf-harness probe|export|ocr|bench [flags] [FILE...]");
            return ExitCode::from(64);
        }
    };
    let result = match args.cmd.as_str() {
        "probe" => probe(&args).map(|()| ExitCode::SUCCESS),
        "export" => export(&args).map(|()| ExitCode::SUCCESS),
        "ocr" => ocr(&args),
        "bench" => bench(&args).map(|()| ExitCode::SUCCESS),
        other => {
            eprintln!("sheaf-harness: unknown command {other}");
            return ExitCode::from(64);
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("sheaf-harness: {e}");
            ExitCode::FAILURE
        }
    }
}
