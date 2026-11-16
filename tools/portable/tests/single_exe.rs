//! The single-file build must need nothing but itself: no sibling worker, no
//! `vendor/` folder. It unpacks its engines on first run, verifies them on
//! every run, and repairs what has been changed or added.

use fixture_gen::Scratch;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A copy of the executable alone in an otherwise empty folder, with its own per-user data dir.
struct Install {
    exe: PathBuf,
    data: PathBuf,
    _dir: Scratch,
}

impl Install {
    fn new() -> Install {
        let dir = Scratch::new("portable");
        let exe = dir.path().join(if cfg!(windows) { "sheaf.exe" } else { "sheaf" });
        std::fs::copy(env!("CARGO_BIN_EXE_sheaf"), &exe).expect("copy the executable");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).expect("data dir");
        Install { exe, data, _dir: dir }
    }

    fn run(&self, args: &[&Path]) -> Output {
        Command::new(&self.exe)
            .args(args)
            .env("XDG_CACHE_HOME", &self.data)
            .env("LOCALAPPDATA", &self.data)
            .env("HOME", &self.data)
            .output()
            .expect("run sheaf")
    }

    fn engines(&self) -> PathBuf {
        let root = self.data.join("Sheaf").join("engines");
        let mut dirs: Vec<_> = std::fs::read_dir(&root)
            .expect("engines folder exists")
            .map(|e| e.expect("entry").path())
            .collect();
        assert_eq!(dirs.len(), 1, "exactly one engines folder: {dirs:?}");
        dirs.remove(0)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn one_file_is_enough_and_it_repairs_its_engines() {
    let docs = Scratch::new("portable-docs");
    let pdf = docs.file("three.pdf", &fixture_gen::text_pdf(3));
    let install = Install::new();

    // First run: unpacks, then opens the PDF in a worker that is this same file.
    let first = install.run(&[Path::new("probe"), &pdf]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(stdout(&first).contains("3 pages"), "{}", stdout(&first));
    let engines = install.engines();
    assert!(engines.join("fonts").is_dir() && engines.join("tessdata").is_dir());

    // Second run reuses the folder.
    assert!(install.run(&[Path::new("probe"), &pdf]).status.success());
    install.engines();

    // A changed engine file and a planted extra file are both undone.
    let model = engines.join("tessdata").join("eng.traineddata");
    let original = std::fs::read(&model).expect("model");
    std::fs::write(&model, b"tampered").expect("tamper");
    let planted = engines.join("tessdata").join("planted.dll");
    std::fs::write(&planted, b"evil").expect("plant");
    let repaired = install.run(&[Path::new("probe"), &pdf]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert_eq!(std::fs::read(&model).expect("model again"), original, "restored");
    assert!(!planted.exists(), "extra files are removed");
}

#[test]
fn a_missing_engine_file_is_restored() {
    let docs = Scratch::new("portable-junk");
    let pdf = docs.file("one.pdf", &fixture_gen::text_pdf(1));
    let install = Install::new();
    assert!(install.run(&[Path::new("probe"), &pdf]).status.success());
    let engines = install.engines();
    std::fs::remove_file(engines.join("tessdata").join("osd.traineddata")).expect("remove");
    let out = install.run(&[Path::new("probe"), &pdf]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(engines.join("tessdata").join("osd.traineddata").is_file());
}
