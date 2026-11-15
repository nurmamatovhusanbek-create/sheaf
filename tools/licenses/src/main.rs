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
        // Our own crates print their folder in parentheses; registry crates print nothing. A patched copy of
        // someone else's crate (third_party/) also prints its folder, and is not ours.
        .filter(|l| is_vendored_copy(l) || (!l.contains("(/") && !l.contains("(\\") && !l.contains(":\\")))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let name = words.next()?;
            let version = words.next()?.strip_prefix('v')?;
            Some((name.to_string(), version.to_string()))
        })
        .collect()
}

/// A copy of someone else's crate kept in `third_party/` (with a change), used through `[patch]`.
fn is_vendored_copy(text: &str) -> bool {
    text.contains("/third_party/") || text.contains("\\third_party\\")
}

fn shipped(meta: &Value) -> Vec<Krate> {
    let wanted = compiled();
    let packages = meta["packages"].as_array().unwrap_or_else(|| fail("no packages"));
    let mut out: Vec<Krate> = packages
        .iter()
        .filter(|p| p["source"].is_string() || is_vendored_copy(p["manifest_path"].as_str().unwrap_or("")))
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

/// One row of `licenses/tesseract-bundle.tsv`.
struct Lib {
    file: String,
    name: String,
    licence: String,
    url: String,
    note: String,
}

fn bundle_rows(root: &Path) -> Vec<Lib> {
    let text = std::fs::read_to_string(root.join("licenses/tesseract-bundle.tsv"))
        .unwrap_or_else(|e| fail(&format!("cannot read licenses/tesseract-bundle.tsv: {e}")));
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            if c.len() < 4 {
                fail(&format!(
                    "a row of licenses/tesseract-bundle.tsv has too few columns: {l}"
                ));
            }
            Lib {
                file: c[0].to_string(),
                name: c[1].to_string(),
                licence: c[2].to_string(),
                url: c[3].to_string(),
                note: c.get(4).copied().unwrap_or("").to_string(),
            }
        })
        .collect()
}

/// The files that the pinned extraction list takes out of the Tesseract installer.
fn extracted_tesseract_files(root: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(root.join("vendor/extract.tsv"))
        .unwrap_or_else(|e| fail(&format!("cannot read vendor/extract.tsv: {e}")));
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (c.len() >= 3 && c[0].contains("tesseract")).then(|| c[2].rsplit('/').next().unwrap_or("").to_string())
        })
        .collect()
}

/// The licence ids named in an SPDX expression.
fn spdx_ids(expr: &str) -> Vec<String> {
    expr.replace(['(', ')'], " ")
        .split_whitespace()
        .filter(|w| !["OR", "AND", "WITH"].contains(w))
        .map(str::to_string)
        .collect()
}

/// The text file for a licence id (`-or-later` and `-only` share a text).
fn spdx_text(root: &Path, id: &str) -> Option<String> {
    let base = id.replace("-or-later", "-only");
    std::fs::read_to_string(root.join("licenses/spdx").join(format!("{base}.txt")))
        .ok()
        .map(|t| t.replace("\r\n", "\n"))
}

/// What is inside the bundled Tesseract: every file, its library and licence, the conditions that come
/// with the copyleft ones, and the licence texts. Fails when a file of the pinned build has no row (or a
/// row names a file that is not in the build) or when a licence named has no text.
fn tesseract_section(root: &Path) -> String {
    let rows = bundle_rows(root);
    let extracted = extracted_tesseract_files(root);
    let listed: BTreeSet<String> = rows.iter().map(|r| r.file.clone()).collect();
    let missing: Vec<&String> = extracted.difference(&listed).collect();
    if !missing.is_empty() {
        fail(&format!(
            "in the bundled Tesseract build but without a row in licenses/tesseract-bundle.tsv: {}",
            missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let stale: Vec<&String> = listed.difference(&extracted).collect();
    if !stale.is_empty() {
        fail(&format!(
            "licenses/tesseract-bundle.tsv lists files that vendor/extract.tsv does not extract: {}",
            stale.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let mut out = format!(
        "{}\nLIBRARIES INSIDE THE BUNDLED TESSERACT BUILD ({} files)\n{}\n\n",
        rule(),
        rows.len(),
        rule()
    );
    out.push_str("Tesseract for Windows is started as a separate program (tesseract.exe) beside its libraries.\n");
    out.push_str("Each file, the library it belongs to, the licence it is used under and where it comes from:\n\n");
    let mut texts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in &rows {
        out.push_str(&format!("  {} - {} - {} - {}", r.file, r.name, r.licence, r.url));
        if !r.note.is_empty() {
            out.push_str(&format!(" ({})", r.note));
        }
        out.push('\n');
        for id in spdx_ids(&r.licence) {
            texts.entry(id).or_default().push(r.file.clone());
        }
    }
    let lgpl: Vec<&str> = rows
        .iter()
        .filter(|r| r.note.contains("copyleft: LGPL"))
        .map(|r| r.file.as_str())
        .collect();
    let gpl: Vec<&str> = rows
        .iter()
        .filter(|r| r.note.contains("copyleft: GPL"))
        .map(|r| r.file.as_str())
        .collect();
    out.push_str("\nConditions that come with some of these:\n\n");
    out.push_str(&format!(
        "  LGPL libraries ({}): shipped unmodified as separate files that can be replaced by a build of the\n  same library; their source is available from the projects named above.\n",
        lgpl.join(", ")
    ));
    out.push_str("  OPEN: a written offer of the corresponding source of the exact versions shipped, with a contact, is\n  to be added before a release.\n\n");
    out.push_str(&format!(
        "  GPL library ({}): JBIG-KIT is GPL-2.0-or-later and is loaded by the TIFF reader that Tesseract\n  starts with. Whether that puts tesseract.exe's distribution under the GPL is a legal question this file\n  does not answer.\n",
        gpl.join(", ")
    ));
    out.push_str("  OPEN: decide between shipping a Tesseract build without JBIG support and meeting the GPL for that\n  library, before a release.\n\n");
    out.push_str(
        "  The GNU runtime libraries (libgcc, libstdc++) are under the GPL with the GCC runtime library exception.\n\n",
    );
    out.push_str(&format!(
        "{}\nLICENCE TEXTS FOR THE BUNDLED TESSERACT BUILD\n{}\n\n",
        rule(),
        rule()
    ));
    out.push_str(
        "The standard wordings (SPDX licence list); the copyright notices of each project are in that project.\n\n",
    );
    // One text per file, even when several ids (-or-later and -only) share it.
    let mut by_text: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (id, files) in texts {
        let text = spdx_text(root, &id).unwrap_or_else(|| fail(&format!("licenses/spdx has no text for {id}")));
        let entry = by_text.entry(text.trim_end().to_string()).or_default();
        entry.0.push(id);
        entry.1.extend(files);
    }
    let mut grouped: Vec<_> = by_text.into_iter().collect();
    grouped.sort_by(|a, b| a.1 .0.cmp(&b.1 .0));
    for (text, (ids, files)) in grouped {
        let mut files = files;
        files.sort();
        files.dedup();
        out.push_str(&format!("--- {}: {}\n\n{text}\n\n", ids.join(", "), files.join(", ")));
    }
    out
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
    out.push_str(&tesseract_section(root));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patched_copy_in_third_party_is_listed_like_a_registry_crate() {
        assert!(is_vendored_copy(
            "egui-winit v0.37.0 (/work/sheaf/third_party/egui-winit)"
        ));
        assert!(is_vendored_copy(r"D:\a\sheaf\third_party\egui-winit\Cargo.toml"));
        assert!(!is_vendored_copy("sheaf-ui v0.0.0 (/work/sheaf/crates/sheaf-ui)"));
    }

    #[test]
    fn licence_expressions_are_split_into_ids() {
        assert_eq!(spdx_ids("MIT"), ["MIT"]);
        assert_eq!(
            spdx_ids("GPL-3.0-or-later WITH GCC-exception-3.1"),
            ["GPL-3.0-or-later", "GCC-exception-3.1"]
        );
        assert_eq!(
            spdx_ids("(MIT OR Apache-2.0) AND Unicode-3.0"),
            ["MIT", "Apache-2.0", "Unicode-3.0"]
        );
    }

    #[test]
    fn every_licence_the_table_names_has_a_text_and_the_table_covers_the_build() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let rows = bundle_rows(&root);
        assert!(rows.len() > 50);
        for r in &rows {
            for id in spdx_ids(&r.licence) {
                assert!(
                    spdx_text(&root, &id).is_some_and(|t| t.len() > 200),
                    "{} needs a text for {id}",
                    r.file
                );
            }
            assert!(r.url.starts_with("http"), "{} has no upstream", r.file);
        }
        let listed: BTreeSet<String> = rows.iter().map(|r| r.file.clone()).collect();
        assert_eq!(listed, extracted_tesseract_files(&root));
    }
}
