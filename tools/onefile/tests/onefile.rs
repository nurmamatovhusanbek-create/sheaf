//! The one-file build must need nothing but itself: no sibling worker, no `vendor/` folder. It
//! unpacks its engines on first run, verifies them on every run, and repairs what has been
//! changed, removed or added.

use fixture_gen::Scratch;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A copy of the executable alone in an otherwise empty folder, with its own per-user data dirs.
struct Install {
    exe: PathBuf,
    data: PathBuf,
    _dir: Scratch,
}

impl Install {
    fn new() -> Install {
        let dir = Scratch::new("onefile");
        let exe = dir.path().join(if cfg!(windows) { "sheaf.exe" } else { "sheaf" });
        std::fs::copy(env!("CARGO_BIN_EXE_sheaf-onefile"), &exe).expect("copy the executable");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).expect("data dir");
        Install { exe, data, _dir: dir }
    }

    fn run(&self, args: &[&Path]) -> Output {
        Command::new(&self.exe)
            .args(args)
            .env("XDG_CACHE_HOME", &self.data)
            .env("XDG_CONFIG_HOME", &self.data)
            .env("LOCALAPPDATA", &self.data)
            .env("APPDATA", &self.data)
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

fn compress(install: &Install, pdf: &Path) -> Output {
    install.run(&[Path::new("run"), Path::new("compress"), pdf])
}

#[test]
fn one_file_is_enough_and_it_repairs_its_engines() {
    let docs = Scratch::new("onefile-docs");
    let pdf = docs.file("three.pdf", &fixture_gen::text_pdf(3));
    let install = Install::new();

    // First run: unpacks, then the headless action runs in a worker that is this same file.
    let first = compress(&install, &pdf);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(
        docs.path().join("three.compressed.pdf").is_file(),
        "the result was written"
    );
    let engines = install.engines();
    assert!(engines.join("fonts").is_dir() && engines.join("tessdata").is_dir());
    assert!(engines.join("SHA256SUMS").is_file());

    // Second run reuses the folder.
    assert!(compress(&install, &pdf).status.success());
    install.engines();

    // A changed engine file, a removed one and a planted extra file are all undone.
    let model = engines.join("tessdata").join("eng.traineddata");
    let original = std::fs::read(&model).expect("model");
    std::fs::write(&model, b"tampered").expect("tamper");
    let font = engines.join("fonts").join("NotoSans-Regular.ttf");
    std::fs::remove_file(&font).expect("remove");
    let planted = engines.join("tessdata").join("planted.dll");
    std::fs::write(&planted, b"evil").expect("plant");
    let repaired = compress(&install, &pdf);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert_eq!(std::fs::read(&model).expect("model again"), original, "restored");
    assert!(font.is_file(), "the removed file is back");
    assert!(!planted.exists(), "extra files are removed");

    // A hash list edited next to the files proves nothing: it is rewritten from the executable's own.
    std::fs::write(engines.join("SHA256SUMS"), b"").expect("blank the list");
    assert!(compress(&install, &pdf).status.success());
    assert!(std::fs::metadata(engines.join("SHA256SUMS")).expect("list").len() > 0);
}
