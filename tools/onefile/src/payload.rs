//! The engines embedded in the executable, and putting them on disk.
//!
//! On start the files are unpacked once into `<per-user dir>/engines/<id>/`, where `<id>` comes
//! from the embedded hash list, and verified against that list, which is compiled into this
//! executable and so cannot be edited without changing the executable (SEC-SUPPLY). The folder is
//! checked again on every start; anything missing, changed or extra (a planted DLL next to
//! `tesseract.exe` would be loaded in its place) is replaced from the executable's own copy.
//!
//! The folder has the layout of the repository's `vendor/`, so the rest of the program finds the
//! engines the way it always does. Its `SHA256SUMS` is rewritten from the embedded list on every
//! start, so the program's own startup check compares against the same trusted hashes.

use flate2::read::ZlibDecoder;
use sheaf_core::{AppError, ErrorCode, Result};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
/// `sha256sum`-format hashes of everything in [`DATA`], paths relative to the engines folder.
const SUMS: &str = include_str!(concat!(env!("OUT_DIR"), "/payload.sums"));
const SUMS_FILE: &str = "SHA256SUMS";

fn corrupt() -> AppError {
    AppError::new(ErrorCode::EngineMissing, "embedded engines are damaged")
}

fn io(what: &str, e: &std::io::Error) -> AppError {
    AppError::io(what, e)
}

/// Names the folder, so a build with different engines gets its own.
fn id() -> String {
    sheaf_store::hex(&sheaf_store::hash_bytes(&[SUMS.as_bytes()]))[..16].to_string()
}

struct Entry<'a> {
    path: &'a str,
    raw_len: u64,
    packed: &'a [u8],
}

fn entries() -> Result<Vec<Entry<'static>>> {
    let mut rest = DATA;
    let mut out = Vec::new();
    let take = |rest: &mut &'static [u8], n: usize| -> Result<&'static [u8]> {
        if rest.len() < n {
            return Err(corrupt());
        }
        let (head, tail) = rest.split_at(n);
        *rest = tail;
        Ok(head)
    };
    while !rest.is_empty() {
        let len = u32::from_le_bytes(take(&mut rest, 4)?.try_into().map_err(|_| corrupt())?) as usize;
        let path = std::str::from_utf8(take(&mut rest, len)?).map_err(|_| corrupt())?;
        let raw_len = u64::from_le_bytes(take(&mut rest, 8)?.try_into().map_err(|_| corrupt())?);
        let packed_len = u64::from_le_bytes(take(&mut rest, 8)?.try_into().map_err(|_| corrupt())?);
        let packed = take(&mut rest, usize::try_from(packed_len).map_err(|_| corrupt())?)?;
        out.push(Entry { path, raw_len, packed });
    }
    Ok(out)
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty() && Path::new(path).components().all(|c| matches!(c, Component::Normal(_)))
}

fn unpack(into: &Path) -> Result<()> {
    for entry in entries()? {
        if !safe_relative(entry.path) {
            return Err(corrupt());
        }
        let target = into.join(entry.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io("create engines folder", &e))?;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(entry.raw_len).unwrap_or(0));
        ZlibDecoder::new(entry.packed)
            .take(entry.raw_len + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| corrupt())?;
        if bytes.len() as u64 != entry.raw_len {
            return Err(corrupt());
        }
        fs::write(&target, bytes).map_err(|e| io("write engine file", &e))?;
    }
    fs::write(into.join(SUMS_FILE), SUMS).map_err(|e| io("write hash list", &e))
}

/// Every file under `dir`, as `/`-separated paths relative to it.
fn files_under(dir: &Path, prefix: &str, out: &mut BTreeSet<String>) -> std::io::Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = format!("{prefix}{}", item.file_name().to_string_lossy());
        if item.file_type()?.is_dir() {
            files_under(&item.path(), &format!("{name}/"), out)?;
        } else {
            out.insert(name);
        }
    }
    Ok(())
}

/// Hashes of every file against the embedded list, the hash list itself against the embedded text,
/// and nothing else in the folder.
fn verify(dir: &Path) -> Result<()> {
    let bad =
        |what: &str, path: &str| AppError::new(ErrorCode::EngineMissing, format!("bundled component {what}: {path}"));
    let mut expected = BTreeSet::from([SUMS_FILE.to_string()]);
    for line in SUMS.lines() {
        let Some((want, path)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let path = path.trim_start();
        let got = sheaf_store::hash_file(&dir.join(path)).map_err(|_| bad("is missing", path))?;
        if !sheaf_store::hex(&got).eq_ignore_ascii_case(want) {
            return Err(bad("does not match its pinned hash", path));
        }
        expected.insert(path.to_string());
    }
    if fs::read_to_string(dir.join(SUMS_FILE)).ok().as_deref() != Some(SUMS) {
        return Err(bad("has a changed hash list", SUMS_FILE));
    }
    let mut present = BTreeSet::new();
    files_under(dir, "", &mut present).map_err(|e| io("list engines folder", &e))?;
    if present != expected {
        return Err(AppError::new(
            ErrorCode::EngineMissing,
            "engines folder has files that are not part of the build",
        ));
    }
    Ok(())
}

/// Make sure the verified engines are on disk under `base` and return their folder.
pub fn install(base: &Path) -> Result<PathBuf> {
    let engines = base.join("engines");
    let dir = engines.join(id());
    if verify(&dir).is_ok() {
        return Ok(dir);
    }
    // Not unpacked yet, damaged, or changed: start over from this executable.
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| io("replace damaged engines", &e))?;
        // another running copy may hold them
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&engines).map_err(|e| io("create engines folder", &e))?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let staging = engines.join(format!("{}.tmp-{}-{nonce}", id(), std::process::id()));
    if let Err(e) = unpack(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Err(e) = fs::rename(&staging, &dir) {
        let _ = fs::remove_dir_all(&staging);
        // Another copy of the program finished first: use its folder if it is good.
        if !dir.is_dir() {
            return Err(io("put engines in place", &e));
        }
    }
    verify(&dir)?;
    discard_older(&engines, &dir);
    Ok(dir)
}

/// Folders left by other builds, and staging folders from interrupted runs. Best effort: one that
/// another running copy still uses stays.
fn discard_older(engines: &Path, keep: &Path) {
    let Ok(items) = fs::read_dir(engines) else { return };
    for item in items.flatten() {
        let path = item.path();
        if path != keep && item.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(&path);
        }
    }
}
