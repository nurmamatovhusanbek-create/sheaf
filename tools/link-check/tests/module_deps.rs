//! Dependency test required by DOC-MODULES / DOC-AGENT-RULES rule 3:
//! a crate may depend only on the workspace crates its module entry lists.
//!
//! The table below mirrors the "Depends on" lines in ARCHITECTURE.md section 6.
//! Change the spec and this table together.

use std::collections::BTreeSet;
use std::path::Path;

const ALLOWED: &[(&str, &[&str])] = &[
    ("sheaf-core", &[]),
    ("sheaf-store", &["sheaf-core"]),
    ("sheaf-image", &["sheaf-core"]),
    ("sheaf-pdf", &["sheaf-core", "sheaf-image"]),
    ("sheaf-ocr", &["sheaf-core", "sheaf-image", "sheaf-store"]),
    ("sheaf-scan", &["sheaf-core", "sheaf-image"]),
    ("sheaf-jobs", &["sheaf-core", "sheaf-store"]),
    (
        "sheaf-worker",
        &["sheaf-core", "sheaf-pdf", "sheaf-image", "sheaf-ocr", "sheaf-store"],
    ),
    ("sheaf-cli", &[]),
];

/// `sheaf-*` keys in every dependency table of a manifest (normal, dev, build, target-specific).
fn sheaf_deps(manifest: &str) -> BTreeSet<String> {
    let mut deps = BTreeSet::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deps = line.trim_end_matches(']').ends_with("dependencies");
            continue;
        }
        if in_deps {
            if let Some((key, _)) = line.split_once('=') {
                let key = key.trim().trim_matches('"');
                let key = key.split('.').next().unwrap_or(key);
                if key.starts_with("sheaf-") {
                    deps.insert(key.to_string());
                }
            }
        }
    }
    deps
}

#[test]
fn crates_depend_only_on_what_the_spec_lists() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("crates/ exists") {
        let dir = entry.expect("readable entry").path();
        let Ok(manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let Some((_, allowed)) = ALLOWED.iter().find(|(n, _)| *n == name) else {
            problems.push(format!("{name}: crate is not in the DOC-MODULES table"));
            continue;
        };
        for dep in sheaf_deps(&manifest) {
            if !allowed.contains(&dep.as_str()) {
                problems.push(format!(
                    "{name}: depends on {dep}, which its module entry does not list"
                ));
            }
        }
        seen.insert(name);
    }
    for (name, _) in ALLOWED {
        if !seen.contains(*name) {
            problems.push(format!("{name}: listed in DOC-LAYOUT but missing from crates/"));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn parser_sees_all_dependency_tables() {
    let m = "[dependencies]\nsheaf-core = { path = \"x\" }\nserde = \"1\"\n[dev-dependencies]\nsheaf-pdf.workspace = true\n[target.'cfg(windows)'.dependencies]\nsheaf-scan = \"0\"\n[package]\nsheaf-nope = 1\n";
    let got: Vec<_> = sheaf_deps(m).into_iter().collect();
    assert_eq!(got, ["sheaf-core", "sheaf-pdf", "sheaf-scan"]);
}
