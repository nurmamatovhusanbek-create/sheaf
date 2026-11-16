//! OCR-TESS-CLI: the bundled `tesseract` executable, one process per call.

use crate::engine::{OcrEngine, Osd, RecOpts};
use crate::{langs, tsv};
use sheaf_core::{AppError, ErrorCode, OcrResult, Result, Script};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Orientation confidence below this is treated as "leave the page alone".
const MIN_ORIENTATION_CONF: f32 = 2.0;
/// Script confidence below this is treated as unknown.
const MIN_SCRIPT_CONF: f32 = 1.0;
/// Most stdout kept from one run.
const MAX_STDOUT: u64 = 4 << 20;

pub struct Tesseract {
    exe: PathBuf,
    tessdata: PathBuf,
    /// Working directory for the process; inside the session dir.
    cwd: PathBuf,
    timeout: Duration,
}

impl Tesseract {
    pub fn new(exe: &Path, tessdata: &Path, cwd: &Path) -> Tesseract {
        Tesseract {
            exe: exe.to_path_buf(),
            tessdata: tessdata.to_path_buf(),
            cwd: cwd.to_path_buf(),
            timeout: Duration::from_secs(120),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Tesseract {
        self.timeout = timeout;
        self
    }

    /// Run with an argument array (never a shell string), a timeout, the
    /// session dir as working directory and one thread: parallelism comes
    /// from MOD-JOBS, not from inside Tesseract.
    fn run(&self, args: Vec<OsString>) -> Result<(bool, String)> {
        let mut child = Command::new(&self.exe)
            .args(args)
            .current_dir(&self.cwd)
            .env("OMP_THREAD_LIMIT", "1")
            // The bundled build links libcurl and would fetch a URL given as an image name. We never
            // give it one (see `local_input`); should that ever change, a dead proxy keeps the
            // request from leaving the machine (SEC-NONET). Best effort, not a firewall.
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| AppError::new(ErrorCode::EngineMissing, "tesseract could not be started"))?;
        let stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(out) = stdout {
                let _ = out.take(MAX_STDOUT).read_to_end(&mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::new(ErrorCode::Timeout, "tesseract exceeded its time limit"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => {
                    let _ = child.kill();
                    return Err(AppError::io("wait for tesseract", &e));
                }
            }
        };
        let text = reader.join().unwrap_or_default();
        Ok((status.success(), text))
    }

    fn base_args(&self, img: &Path, out: &str) -> Vec<OsString> {
        vec![
            img.into(),
            out.into(),
            "--tessdata-dir".into(),
            self.tessdata.clone().into(),
        ]
    }
}

/// Tesseract treats `http://…` as an image to download and `-` or `stdin` as standard input.
/// Inputs here are always files the worker wrote itself, so anything else is a bug or an attack
/// and is refused before a process starts.
fn local_input(img: &Path) -> Result<()> {
    let name = img.to_string_lossy();
    if name.contains("://") || name == "-" || name == "stdin" {
        return Err(AppError::invalid("OCR input must be a local file"));
    }
    Ok(())
}

/// Parse `--psm 0` output.
pub(crate) fn parse_osd(text: &str) -> Osd {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .map(|v| v.trim_start_matches(':').trim().to_string())
    };
    let num = |name: &str| field(name).and_then(|v| v.parse::<f32>().ok());
    let rotate = match (num("Rotate"), num("Orientation confidence")) {
        (Some(r), Some(c)) if c >= MIN_ORIENTATION_CONF && [90.0, 180.0, 270.0].contains(&r) => r as u16,
        _ => 0,
    };
    let script = match (field("Script").as_deref(), num("Script confidence")) {
        (Some("Latin"), Some(c)) if c >= MIN_SCRIPT_CONF => Script::Latin,
        (Some("Cyrillic"), Some(c)) if c >= MIN_SCRIPT_CONF => Script::Cyrillic,
        _ => Script::Unknown,
    };
    Osd { rotate, script }
}

impl OcrEngine for Tesseract {
    fn id(&self) -> &str {
        "tesseract"
    }

    fn version(&self) -> String {
        self.run(vec!["--version".into()])
            .ok()
            .and_then(|(_, out)| out.lines().next().map(|l| l.trim().to_string()))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn detect(&self, img: &Path) -> Result<Osd> {
        local_input(img)?;
        let mut args = self.base_args(img, "stdout");
        args.extend(["--psm".into(), "0".into(), "-l".into(), "osd".into()]);
        // A page with too little text makes OSD fail; that is "unknown", not an error.
        let (ok, text) = self.run(args)?;
        Ok(if ok { parse_osd(&text) } else { Osd::default() })
    }

    fn recognize(&self, img: &Path, opts: &RecOpts) -> Result<OcrResult> {
        langs::validate(&opts.langs)?;
        local_input(img)?;
        let out_base = img.with_extension("out");
        let out_str = out_base
            .to_str()
            .ok_or_else(|| AppError::internal("non-UTF-8 work path"))?;
        let mut args = self.base_args(img, out_str);
        let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
        push(&[
            "-l",
            &opts.langs.join("+"),
            "--oem",
            "1",
            "--psm",
            "3",
            "--dpi",
            &opts.dpi.clamp(70, 2400).to_string(),
        ]);
        push(&["-c", "tessedit_create_tsv=1"]);
        if opts.text_pdf.is_some() {
            // Invisible text only, no image: laid over the original page later (OCR-TEXTLAYER).
            push(&["-c", "tessedit_create_pdf=1", "-c", "textonly_pdf=1"]);
        }
        let (ok, _) = self.run(args)?;
        // Tesseract resolved `out_base` against its working directory; so must we.
        let tsv_path = self.cwd.join(out_base.with_extension("out.tsv"));
        let pdf_path = self.cwd.join(out_base.with_extension("out.pdf"));
        let cleanup = || {
            let _ = std::fs::remove_file(&tsv_path);
            let _ = std::fs::remove_file(&pdf_path);
        };
        if !ok {
            cleanup();
            return Err(AppError::new(ErrorCode::ParseError, "tesseract failed on the page"));
        }
        let tsv_text = std::fs::read_to_string(&tsv_path).map_err(|e| {
            cleanup();
            AppError::io("read OCR output", &e)
        })?;
        let keep = |from: &Path, dest: &Option<PathBuf>, what: &str| -> Result<()> {
            match dest {
                Some(dest) => std::fs::rename(from, dest).map_err(|e| AppError::io(what, &e)),
                None => {
                    let _ = std::fs::remove_file(from);
                    Ok(())
                }
            }
        };
        keep(&tsv_path, &opts.tsv, "store OCR words").inspect_err(|_| cleanup())?;
        if opts.text_pdf.is_some() {
            keep(&pdf_path, &opts.text_pdf, "store text layer").inspect_err(|_| cleanup())?;
        }
        let mut result = tsv::parse(&tsv_text);
        result.langs_used = opts.langs.clone();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_files_reach_tesseract() {
        // The executable does not exist: the refusal must come before any process is started.
        let t = Tesseract::new(Path::new("/nonexistent/tesseract"), Path::new("/x"), Path::new("."));
        for bad in ["http://example.com/a.png", "https://x/y", "ftp://x/y", "-", "stdin"] {
            assert_eq!(
                t.detect(Path::new(bad)).expect_err(bad).code,
                ErrorCode::InvalidArg,
                "{bad}"
            );
            let opts = RecOpts {
                langs: vec!["eng".into()],
                dpi: 300,
                text_pdf: None,
                tsv: None,
            };
            assert_eq!(
                t.recognize(Path::new(bad), &opts).expect_err(bad).code,
                ErrorCode::InvalidArg
            );
        }
    }

    #[test]
    fn osd_output_is_parsed_with_confidence_gates() {
        let sideways = "Page number: 0\nOrientation in degrees: 270\nRotate: 90\nOrientation confidence: 12.4\nScript: Cyrillic\nScript confidence: 3.1\n";
        assert_eq!(
            parse_osd(sideways),
            Osd {
                rotate: 90,
                script: Script::Cyrillic
            }
        );
        let unsure = "Rotate: 180\nOrientation confidence: 0.4\nScript: Latin\nScript confidence: 0.2\n";
        assert_eq!(
            parse_osd(unsure),
            Osd {
                rotate: 0,
                script: Script::Unknown
            }
        );
        let other = "Rotate: 0\nOrientation confidence: 9\nScript: Arabic\nScript confidence: 9\n";
        assert_eq!(
            parse_osd(other),
            Osd {
                rotate: 0,
                script: Script::Unknown
            }
        );
        assert_eq!(parse_osd("Rotate: 45\nOrientation confidence: 9\n").rotate, 0);
        assert_eq!(parse_osd(""), Osd::default());
    }
}
