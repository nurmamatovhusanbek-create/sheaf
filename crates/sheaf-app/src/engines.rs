//! Where the bundled engines live, and the startup hash check (SEC-SUPPLY,
//! FLOW-LIFECYCLE).

use sheaf_core::{AppError, ErrorCode, Result};
use std::path::{Path, PathBuf};

/// Where the bundled engines live.
#[derive(Debug, Clone)]
pub struct Engines {
    pub worker: PathBuf,
    pub pdfium_dir: PathBuf,
    /// Tesseract executable and tessdata dir; without them OCR jobs fail with `EngineMissing`.
    pub tesseract: Option<(PathBuf, PathBuf)>,
    /// TrueType font for stamps; without it stamping fails with `EngineMissing`.
    pub font: Option<PathBuf>,
}

/// Folder name of PDFium inside `vendor/pdfium/` for this platform.
const PDFIUM_PLATFORM: &str = if cfg!(windows) { "win-x64" } else { "linux-x64" };

impl Engines {
    /// Engines laid out as `vendor/` is: `pdfium/<platform>/`, `tesseract/win-x64/`,
    /// `tessdata/`, `fonts/`. The worker is the `sheaf-worker` beside the
    /// running executable. On Windows a missing Tesseract is not an error
    /// (OCR then reports `EngineMissing`); elsewhere the `tesseract` on `PATH` is used.
    pub fn from_vendor(vendor: &Path) -> Result<Engines> {
        let pdfium_dir = vendor.join("pdfium").join(PDFIUM_PLATFORM);
        let lib = if cfg!(windows) { "pdfium.dll" } else { "libpdfium.so" };
        if !pdfium_dir.join(lib).is_file() {
            return Err(AppError::new(
                ErrorCode::EngineMissing,
                "PDFium is not in the vendor folder",
            ));
        }
        let tessdata = vendor.join("tessdata");
        let exe = if cfg!(windows) {
            vendor.join("tesseract").join("win-x64").join("tesseract.exe")
        } else {
            PathBuf::from("tesseract")
        };
        let tesseract = (tessdata.is_dir() && (!cfg!(windows) || exe.is_file())).then_some((exe, tessdata));
        let font = Some(vendor.join("fonts").join("NotoSans-Regular.ttf")).filter(|p| p.is_file());
        Ok(Engines {
            worker: crate::sibling_worker()?,
            pdfium_dir,
            tesseract,
            font,
        })
    }

    /// Find the vendor folder and build the engines from it. Looked for, in
    /// order: `SHEAF_VENDOR_DIR`, each of `roots` (an installed app passes its
    /// resource dir), `vendor/` beside the executable, then `vendor/` in the
    /// executable's ancestors (a development checkout, where the executable
    /// sits in `target/<profile>/`).
    pub fn discover(roots: &[PathBuf]) -> Result<Engines> {
        let mut found = vendor_candidates(roots).into_iter().filter(|dir| dir.is_dir());
        let mut last: Option<AppError> = None;
        for dir in found.by_ref() {
            match Engines::from_vendor(&dir) {
                Ok(engines) => return Ok(engines),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| AppError::new(ErrorCode::EngineMissing, "no vendor folder found")))
    }

    /// SEC-SUPPLY: check the bundled files against their pinned hashes. Every entry
    /// point that starts workers calls this first (the window does it in the
    /// background; the wheel and `sheaf run` do it before they begin), so a
    /// swapped DLL is never loaded by any mode. Fails closed: no pinned hashes,
    /// no start.
    pub fn verify(&self) -> Result<()> {
        let vendor = self
            .vendor_dir()
            .ok_or_else(|| AppError::new(ErrorCode::EngineMissing, "cannot tell where the bundled files are"))?;
        verify_vendor(vendor).map(|_| ())
    }

    /// The vendor folder these engines came from: the parent of `pdfium/`.
    pub fn vendor_dir(&self) -> Option<&Path> {
        self.pdfium_dir.parent().and_then(Path::parent)
    }
}

fn vendor_candidates(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // A one-file build says where it unpacked its engines, and nothing else counts then.
    if let Some(dir) = crate::vendor_dir_override() {
        return vec![dir.to_path_buf()];
    }
    if let Some(dir) = std::env::var_os("SHEAF_VENDOR_DIR").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(dir));
    }
    out.extend(roots.iter().map(|r| r.join("vendor")));
    if let Ok(exe) = std::env::current_exe() {
        out.extend(exe.ancestors().skip(1).take(5).map(|dir| dir.join("vendor")));
    }
    out
}

/// What [`verify_vendor`] looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// Files whose hash matched.
    pub checked: usize,
}

/// SEC-SUPPLY at startup: hash every bundled file that `vendor/SHA256SUMS`
/// pins and that is present, and fail on the first mismatch. Download
/// archives (`dl/`) are build inputs and never shipped, so they are skipped;
/// files that are pinned but absent belong to another platform.
pub fn verify_vendor(vendor: &Path) -> Result<Verified> {
    let sums = std::fs::read_to_string(vendor.join("SHA256SUMS"))
        .map_err(|_| AppError::new(ErrorCode::EngineMissing, "vendor/SHA256SUMS is missing"))?;
    let mut checked = 0;
    for line in sums.lines() {
        let Some((want, rel)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rel = rel.trim().trim_start_matches('*');
        let safe = !rel.is_empty() && !rel.starts_with("dl/") && rel.split('/').all(|s| !s.is_empty() && s != "..");
        if !safe {
            continue;
        }
        let file = vendor.join(rel);
        if !file.is_file() {
            continue;
        }
        let got = sheaf_store::hex(&sheaf_store::hash_file(&file)?);
        if !got.eq_ignore_ascii_case(want.trim()) {
            // The name of the file is not secret, and it is what a person needs to see.
            return Err(AppError::new(
                ErrorCode::EngineMissing,
                format!("bundled file failed its hash check: {rel}"),
            ));
        }
        checked += 1;
    }
    Ok(Verified { checked })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sheaf-engines-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn verify_accepts_matching_files_and_skips_absent_and_downloads() {
        let dir = scratch("ok");
        std::fs::write(dir.join("a.bin"), b"hello").expect("write");
        let hash = sheaf_store::hex(&sheaf_store::hash_bytes(&[b"hello"]));
        std::fs::write(
            dir.join("SHA256SUMS"),
            format!("{hash}  a.bin\n{hash}  not/here.bin\n{hash}  dl/big.exe\n"),
        )
        .expect("write sums");
        assert_eq!(verify_vendor(&dir).expect("verified"), Verified { checked: 1 });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verify_rejects_a_changed_file_and_names_it() {
        let dir = scratch("bad");
        std::fs::write(dir.join("a.bin"), b"tampered").expect("write");
        let hash = sheaf_store::hex(&sheaf_store::hash_bytes(&[b"hello"]));
        std::fs::write(dir.join("SHA256SUMS"), format!("{hash}  a.bin\n")).expect("write sums");
        let err = verify_vendor(&dir).expect_err("must fail");
        assert_eq!(err.code, ErrorCode::EngineMissing);
        assert!(err.msg.contains("a.bin"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn engines_verify_checks_the_folder_they_came_from_and_fails_closed() {
        let dir = scratch("engines");
        std::fs::create_dir_all(dir.join("pdfium").join(PDFIUM_PLATFORM)).expect("dirs");
        let engines = Engines {
            worker: PathBuf::from("w"),
            pdfium_dir: dir.join("pdfium").join(PDFIUM_PLATFORM),
            tesseract: None,
            font: None,
        };
        let err = engines.verify().expect_err("no SHA256SUMS: no start");
        assert_eq!(err.code, ErrorCode::EngineMissing);
        std::fs::write(dir.join("SHA256SUMS"), "").expect("sums");
        assert!(engines.verify().is_ok());
        std::fs::write(dir.join("file.bin"), b"x").expect("file");
        std::fs::write(dir.join("SHA256SUMS"), format!("{}  file.bin\n", "0".repeat(64))).expect("sums");
        assert!(engines.verify().is_err(), "a changed file stops every mode");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verify_ignores_paths_that_climb_out() {
        let dir = scratch("climb");
        std::fs::write(
            dir.join("SHA256SUMS"),
            format!("{}  ../outside\n", sheaf_store::hex(&[0u8; 32])),
        )
        .expect("write sums");
        assert_eq!(verify_vendor(&dir).expect("verified").checked, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
