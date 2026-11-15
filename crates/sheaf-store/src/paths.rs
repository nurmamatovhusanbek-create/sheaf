//! SEC-PATHS: which files the app may read and where it may write.
//!
//! Inputs come only from dialogs, drops or CLI args, never from UI strings.
//! Outputs never overwrite anything (ADR-NO-OVERWRITE).

use crate::token;
use sheaf_core::consts::MAX_FILE_BYTES;
use sheaf_core::{AppError, ErrorCode, Result};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Longest file name (without directory) the app will create.
const MAX_NAME_LEN: usize = 120;

fn rejected(why: &str) -> AppError {
    AppError::new(ErrorCode::PathRejected, why)
}

/// An input file that passed the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedInput {
    /// Canonical absolute path.
    pub path: PathBuf,
    pub size: u64,
    /// On a network share. Allowed, but callers surface it to the user.
    pub is_unc: bool,
}

/// DOS device names; reserved with any extension, in any directory.
fn is_reserved_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem[3..].chars().all(|c| c.is_ascii_digit() || "¹²³".contains(c)))
}

/// Lexical Windows rules, applied before the file system is touched: no
/// device namespace, no reserved device names, no alternate data streams.
/// Pure, so it is tested on every platform.
pub fn check_windows_path_rules(raw: &str) -> Result<()> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(rejected("empty path or NUL byte"));
    }
    let norm = raw.replace('/', "\\");
    let upper = norm.to_ascii_uppercase();
    if upper.starts_with("\\\\.\\") || upper.starts_with("\\\\?\\GLOBALROOT") || upper.starts_with("\\??\\") {
        return Err(rejected("device path"));
    }
    // Strip the prefixes that legitimately contain ':' or '?'.
    let mut rest = norm.as_str();
    if let Some(r) = rest.strip_prefix("\\\\?\\") {
        rest = r.strip_prefix("UNC\\").or_else(|| r.strip_prefix("unc\\")).unwrap_or(r);
    }
    let bytes = rest.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        rest = &rest[2..];
    }
    for component in rest.split('\\').filter(|c| !c.is_empty()) {
        if component.contains(':') {
            return Err(rejected("alternate data stream"));
        }
        if is_reserved_name(component) {
            return Err(rejected("reserved device name"));
        }
    }
    Ok(())
}

fn looks_unc(raw: &str) -> bool {
    let norm = raw.replace('/', "\\").to_ascii_uppercase();
    norm.starts_with("\\\\?\\UNC\\") || (norm.starts_with("\\\\") && !norm.starts_with("\\\\?\\"))
}

/// Validate a file the user picked, dropped or passed on the command line.
pub fn allow_input(p: &Path) -> Result<AllowedInput> {
    let raw = p.to_str().ok_or_else(|| rejected("path is not valid Unicode"))?;
    if raw.is_empty() || raw.contains('\0') {
        return Err(rejected("empty path or NUL byte"));
    }
    if cfg!(windows) {
        check_windows_path_rules(raw)?;
    }
    let path = p.canonicalize().map_err(|e| match e.kind() {
        ErrorKind::NotFound => AppError::not_found("input file"),
        _ => AppError::io("resolve input", &e),
    })?;
    if cfg!(windows) {
        // Again on the resolved form: a link may point somewhere the rules forbid.
        check_windows_path_rules(path.to_str().ok_or_else(|| rejected("path is not valid Unicode"))?)?;
    }
    let meta = fs::metadata(&path).map_err(|e| AppError::io("stat input", &e))?;
    if !meta.is_file() {
        return Err(rejected("not a regular file"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(AppError::limit("file is larger than the size limit"));
    }
    let is_unc = cfg!(windows) && (looks_unc(raw) || path.to_str().is_some_and(looks_unc));
    Ok(AllowedInput {
        path,
        size: meta.len(),
        is_unc,
    })
}

/// Make `name` safe as a single file name: no separators or control
/// characters, no reserved names, no trailing dot or space, bounded length.
pub fn sanitize_file_name(name: &str) -> String {
    let mut clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let (stem, ext) = match clean.rfind('.') {
        Some(i) if i > 0 && clean.len() - i <= 10 => {
            let ext = clean.split_off(i);
            (clean, ext)
        }
        _ => (clean, String::new()),
    };
    let budget = MAX_NAME_LEN.saturating_sub(ext.chars().count());
    let mut stem: String = stem.chars().take(budget).collect();
    stem = stem.trim_matches(|c| c == ' ' || c == '.').to_string();
    if stem.is_empty() || is_reserved_name(&stem) {
        stem.insert(0, '_');
    }
    let ext = ext.trim_end_matches([' ', '.']);
    format!("{stem}{ext}")
}

fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}-{n}{}", &name[..i], &name[i..]),
        _ => format!("{name}-{n}"),
    }
}

fn checked_out_dir(dir: &Path) -> Result<PathBuf> {
    if cfg!(windows) {
        check_windows_path_rules(dir.to_str().ok_or_else(|| rejected("path is not valid Unicode"))?)?;
    }
    // The plain form (`C:\dir`, not `\\?\C:\dir`): every path built from this one is shown
    // to people and handed to scripts and other programs.
    let dir = simplify_verbatim(dir.canonicalize().map_err(|e| AppError::io("resolve output dir", &e))?);
    if !dir.is_dir() {
        return Err(rejected("output location is not a directory"));
    }
    Ok(dir)
}

/// Validate a folder the user picked for outputs: it must exist, be a
/// directory and pass the Windows path rules (SEC-PATHS). Returns it canonical.
pub fn check_output_dir(dir: &Path) -> Result<PathBuf> {
    checked_out_dir(dir)
}

/// A path in `dir` for `name` that does not exist right now; collisions get
/// `-2`, `-3`, … This is advisory: [`publish`] is what guarantees no overwrite.
pub fn allow_output(dir: &Path, name: &str) -> Result<PathBuf> {
    let dir = checked_out_dir(dir)?;
    let name = sanitize_file_name(name);
    (1..10_000)
        .map(|n| dir.join(numbered(&name, n)))
        .find(|p| !p.exists())
        .ok_or_else(|| AppError::limit("too many files with the same name"))
}

/// Copy a finished file out of the session dir into `dir` under `name`,
/// never replacing an existing file and never leaving a partial one behind
/// (ADR-NO-OVERWRITE, FLOW-TOOL-RUN). Returns the path actually used.
pub fn publish(src: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    let dir = checked_out_dir(dir)?;
    let name = sanitize_file_name(name);
    let tmp = dir.join(format!(".{}.part", token()));
    let staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut out| {
            let mut input = fs::File::open(src)?;
            std::io::copy(&mut input, &mut out)?;
            out.sync_all()
        });
    if let Err(e) = staged {
        let _ = fs::remove_file(&tmp);
        return Err(AppError::io("write output", &e));
    }
    for n in 1..10_000 {
        let target = dir.join(numbered(&name, n));
        // A hard link fails if the target exists: an atomic "create new".
        match fs::hard_link(&tmp, &target) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                return Ok(target);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => {
                // No hard links here (FAT, some network shares): claim the
                // name with create_new, then rename the staged file over our
                // own empty placeholder.
                match OpenOptions::new().write(true).create_new(true).open(&target) {
                    Ok(_) => {
                        return fs::rename(&tmp, &target).map(|()| target.clone()).map_err(|e| {
                            let _ = fs::remove_file(&tmp);
                            let _ = fs::remove_file(&target);
                            AppError::io("move output into place", &e)
                        });
                    }
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                    Err(e) => {
                        let _ = fs::remove_file(&tmp);
                        return Err(AppError::io("create output", &e));
                    }
                }
            }
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(AppError::limit("too many files with the same name"))
}

/// Create a new folder in `dir` for a job that produces several files
/// (TOOL-PDF2IMG). Never reuses an existing folder: collisions get `-2`, `-3`, …
pub fn create_output_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    let dir = checked_out_dir(dir)?;
    let name = sanitize_file_name(name);
    for n in 1..10_000 {
        let target = dir.join(if n == 1 { name.clone() } else { format!("{name}-{n}") });
        match fs::create_dir(&target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(AppError::io("create output folder", &e)),
        }
    }
    Err(AppError::limit("too many folders with the same name"))
}

/// Turn `\\?\C:\dir` back into `C:\dir` when that is safe. `canonicalize`
/// produces the verbatim form on Windows, which Rust and PDFium accept but
/// which many programs (Tesseract among them) do not take as a working
/// directory or file argument. Anything else is returned unchanged.
pub fn simplify_verbatim(path: PathBuf) -> PathBuf {
    let Some(s) = path.to_str() else { return path };
    let Some(rest) = s.strip_prefix(r"\\?\") else {
        return path;
    };
    let b = rest.as_bytes();
    let plain_drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    if plain_drive && rest.len() < 240 && check_windows_path_rules(rest).is_ok() {
        PathBuf::from(rest)
    } else {
        path
    }
}
