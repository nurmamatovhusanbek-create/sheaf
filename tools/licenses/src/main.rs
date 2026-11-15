//! `cargo run -p licenses` writes `THIRD_PARTY_LICENSES`; `-- --check` fails when the file is out
//! of date (CI). It covers every crate that is compiled into `sheaf.exe`, `sheafc.exe` and
//! `sheaf-worker.exe` for Windows (normal dependencies, not tests or build scripts) and the
//! bundled components in `vendor/`. The output depends only on the lock file and the vendor
//! folder, so it is the same on every machine.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "x86_64-pc-windows-msvc";
const ROOTS: [&str; 2] = ["sheaf-shell", "sheaf-worker"];

fn fail(msg: &str) -> ! {
    eprintln!("licenses: {msg}");
    std::process::exit(1);
}

struct Krate {
    name: String,
    version: String,
    license: String,
    repository: String,
    dir: PathBuf,
}

fn metadata() -> Value {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            TARGET,
        ])
        .output()
        .unwrap_or_else(|e| fail(&format!("cannot run cargo metadata: {e}")));
    if !out.status.success() {
        fail(&format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| fail(&format!("cannot read cargo metadata: {e}")))
}

/// The registry crates the Windows build compiles in, as `cargo tree` reports them for the
/// programs that ship (normal dependencies only: no tests, no build scripts' own tooling).
fn compiled() -> BTreeSet<(String, String)> {
    let mut args: Vec<&str> = vec![
        "tree", "--locked", "-e", "normal", "--target", TARGET, "--prefix", "none", "--format", "{p}",
    ];
    for root in ROOTS {
        args.extend(["-p", root]);
    }
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(&args)
        .output()
        .unwrap_or_else(|e| fail(&format!("cannot run cargo tree: {e}")));
    if !out.status.success() {
        fail(&format!("cargo tree failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        // Our own crates print their folder in parentheses; registry crates print nothing.
        .filter(|l| !l.contains("(/") && !l.contains("(\\") && !l.contains(":\\"))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let name = words.next()?;
            let version = words.next()?.strip_prefix('v')?;
            Some((name.to_string(), version.to_string()))
        })
        .collect()
}

fn shipped(meta: &Value) -> Vec<Krate> {
    let wanted = compiled();
    let packages = meta["packages"].as_array().unwrap_or_else(|| fail("no packages"));
    let mut out: Vec<Krate> = packages
        .iter()
        .filter(|p| p["source"].is_string())
        .filter(|p| {
            wanted.contains(&(
                p["name"].as_str().unwrap_or("").to_string(),
                p["version"].as_str().unwrap_or("").to_string(),
            ))
        })
        .map(|p| Krate {
            name: p["name"].as_str().unwrap_or("").to_string(),
            version: p["version"].as_str().unwrap_or("").to_string(),
            license: p["license"].as_str().unwrap_or("(none declared)").to_string(),
            repository: p["repository"].as_str().unwrap_or("").to_string(),
            dir: Path::new(p["manifest_path"].as_str().unwrap_or(""))
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        })
        .collect();
    out.sort_by(|a, b| (a.name.as_str(), a.version.as_str()).cmp(&(b.name.as_str(), b.version.as_str())));
    if out.len() < wanted.len() {
        fail("a crate that cargo tree lists is missing from cargo metadata");
    }
    out
}

/// The licence and notice files at the top of a crate's folder.
fn licence_files(dir: &Path) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            let wanted = ["license", "licence", "copying", "notice", "unlicense"]
                .iter()
                .any(|p| lower.starts_with(p));
            let text = std::fs::read_to_string(e.path()).ok()?;
            (wanted && e.path().is_file()).then_some((name, text.replace("\r\n", "\n")))
        })
        .collect();
    found.sort();
    found
}

fn rule() -> String {
    "=".repeat(78)
}

fn render(root: &Path) -> String {
    let meta = metadata();
    let crates = shipped(&meta);
    let mut out = String::new();
    out.push_str("THIRD-PARTY LICENSES\n\n");
    out.push_str("Sheaf includes the software below. This file is generated by `cargo run -p licenses`\n");
    out.push_str("from the lock file and the vendor folder; do not edit it by hand.\n\n");

    // Bundled components.
    out.push_str(&format!("{}\nBUNDLED COMPONENTS (vendor/)\n{}\n\n", rule(), rule()));
    let vendored: [(&str, &str, &str, Option<&str>); 4] = [
        (
            "PDFium (build 8125, bblanchon/pdfium-binaries)",
            "BSD-3-Clause and Apache-2.0, with the licences of its own dependencies",
            "vendor/pdfium/LICENSE",
            None,
        ),
        (
            "Noto Sans Regular 2.015",
            "SIL Open Font License 1.1",
            "vendor/fonts/OFL.txt",
            None,
        ),
        (
            "tessdata_fast 4.1.0 (eng, rus, uzb, uzb_cyrl, osd) and pdf.ttf",
            "Apache-2.0",
            "",
            Some("Apache-2.0"),
        ),
        (
            "Tesseract 5.5.0 for Windows (UB Mannheim build) and the libraries in it",
            "Apache-2.0 for Tesseract; the licences of the bundled libraries are collected when the build is unpacked",
            "",
            Some("Apache-2.0"),
        ),
    ];
    let apache = crates
        .iter()
        .flat_map(|k| licence_files(&k.dir))
        .find(|(n, t)| n.to_ascii_lowercase().contains("apache") && t.contains("Apache License"))
        .map(|(_, t)| t);
    for (name, licence, file, generic) in vendored {
        out.push_str(&format!("{name}\nLicence: {licence}\n\n"));
        if !file.is_empty() {
            match std::fs::read_to_string(root.join(file)) {
                Ok(text) => out.push_str(&format!("{}\n\n", text.replace("\r\n", "\n").trim_end())),
                Err(_) => out.push_str(&format!(
                    "(licence text not found at {file}: run tools/fetch-vendor)\n\n"
                )),
            }
        } else if generic == Some("Apache-2.0") {
            match &apache {
                Some(text) => out.push_str(&format!("{}\n\n", text.trim_end())),
                None => out.push_str("(Apache-2.0 text not found)\n\n"),
            }
        }
    }
    out.push_str("OPEN: the licence texts of the libraries inside the Tesseract Windows installer (Leptonica, libpng,\nlibjpeg, libtiff, zlib and others) must be added from the unpacked build before a release.\n\n");

    // Rust crates, grouped by identical licence text.
    out.push_str(&format!(
        "{}\nRUST CRATES COMPILED INTO THE WINDOWS BUILD ({})\n{}\n\n",
        rule(),
        crates.len(),
        rule()
    ));
    let mut texts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut without_file: Vec<String> = Vec::new();
    for k in &crates {
        let label = format!("{} {} ({})", k.name, k.version, k.license);
        out.push_str(&label);
        if !k.repository.is_empty() {
            out.push_str(&format!(" - {}", k.repository));
        }
        out.push('\n');
        let files = licence_files(&k.dir);
        if files.is_empty() {
            without_file.push(
                format!("{} {} ({}) {}", k.name, k.version, k.license, k.repository)
                    .trim_end()
                    .to_string(),
            );
        }
        for (_, text) in files {
            texts
                .entry(text.trim_end().to_string())
                .or_default()
                .push(format!("{} {}", k.name, k.version));
        }
    }
    if !without_file.is_empty() {
        out.push_str("\nThese crates carry no licence file in their package. They are used under the licence\n");
        out.push_str("named here, which is the one their authors declare; the copyright notices and the full\n");
        out.push_str("terms are in the repository named with each:\n\n");
        for line in &without_file {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out.push_str(&format!("\n{}\nLICENCE TEXTS\n{}\n\n", rule(), rule()));
    let mut grouped: Vec<(&String, &Vec<String>)> = texts.iter().collect();
    grouped.sort_by(|a, b| a.1.cmp(b.1));
    for (text, users) in grouped {
        out.push_str(&format!("--- used by: {}\n\n{text}\n\n", users.join(", ")));
    }
    out
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().unwrap_or(root);
    let path = root.join("THIRD_PARTY_LICENSES");
    let text = render(&root);
    if std::env::args().any(|a| a == "--check") {
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current != text {
            fail("THIRD_PARTY_LICENSES is out of date: run `cargo run -p licenses` and commit the result");
        }
        println!("licenses: THIRD_PARTY_LICENSES is up to date");
    } else {
        std::fs::write(&path, &text).unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
        println!("licenses: wrote {} ({} bytes)", path.display(), text.len());
    }
}
