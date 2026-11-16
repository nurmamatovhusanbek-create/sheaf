//! MOD-STORE: session dir, cache, settings, path policy, content hashing.
//!
//! See `ARCHITECTURE.md`, anchor `mod-store`. Presets I/O arrives with PH-6.

pub mod paths;
pub mod session;
pub mod settings;

pub use paths::{allow_input, allow_output, create_output_dir, publish, AllowedInput};
pub use session::{default_base_dir, sweep_stale, sweep_stale_after, Session};
pub use settings::{config_dir, Settings};

use sha2::{Digest, Sha256};
use sheaf_core::{AppError, ErrorCode, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-unique, hard-to-guess token for temp names. Not cryptographic;
/// the session dir's permissions are what keep other users out (SEC-TEMP).
pub(crate) fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

fn sibling_temp(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::invalid("path has no file name"))?;
    Ok(path.with_file_name(format!(".{name}.{}.part", token())))
}

/// Write `bytes` to `path` so readers see the old content or the new, never a
/// mix: temp file in the same directory, flush, rename (PERF-IO).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = sibling_temp(path)?;
    let write = || -> std::io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        AppError::io("atomic write", &e)
    })
}

/// SHA-256 of a file, streamed.
pub fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let mut f = File::open(path).map_err(|e| AppError::io("open for hashing", &e))?;
    hash_open_file(&mut f)
}

/// SHA-256 of an already open file, from its current position.
fn hash_open_file(f: &mut File) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| AppError::io("read for hashing", &e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// SHA-256 of a byte string.
pub fn hash_bytes(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// SEC-SUPPLY: check the bundled third-party files against `<root>/SHA256SUMS`
/// (`sha256sum` format, paths relative to `root`) before anything is loaded
/// from them. Listed files that are not in this bundle are skipped, and so is
/// `dl/`, which holds build inputs; every path in `required` must be present,
/// listed and intact. A mismatch is an `EngineMissing` error naming the file.
///
/// On Windows every verified file is opened so that nobody else can write to,
/// rename or delete it, hashed through that same handle, and kept open for as
/// long as the returned [`BundleGuard`] lives: what was checked is what gets
/// loaded, with no gap in between. Elsewhere the guard holds the files open
/// but cannot stop a replacement, so the install folder must not be writable
/// by other software there (SEC-TB4).
pub fn verify_bundle(root: &Path, required: &[&str]) -> Result<BundleGuard> {
    let failed =
        |what: &str, path: &str| AppError::new(ErrorCode::EngineMissing, format!("bundled component {what}: {path}"));
    let list = fs::read_to_string(root.join("SHA256SUMS")).map_err(|_| failed("has no hash list", "SHA256SUMS"))?;
    let mut checked: Vec<&str> = Vec::new();
    let mut held = Vec::new();
    for line in list.lines() {
        let Some((want, path)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let path = path.trim_start().trim_start_matches('*');
        let escapes = path.is_empty()
            || path.starts_with(['/', '\\'])
            || path.contains(':')
            || path.split('/').any(|s| s == "..");
        if escapes {
            return Err(failed("lists an unsafe path", path));
        }
        if path.starts_with("dl/") {
            continue;
        }
        let file = root.join(path);
        if !file.is_file() {
            continue;
        }
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            open.share_mode(1); // FILE_SHARE_READ: no writers, no rename or delete
        }
        let mut handle = open
            .open(&file)
            .map_err(|_| failed("cannot be opened for checking", path))?;
        let got = hex(&hash_open_file(&mut handle)?);
        if !got.eq_ignore_ascii_case(want.trim()) {
            return Err(failed("does not match its pinned hash", path));
        }
        held.push(handle);
        checked.push(path);
    }
    match required.iter().find(|r| !checked.contains(r)) {
        Some(missing) => Err(failed("is missing or not pinned", missing)),
        None => Ok(BundleGuard { _files: held }),
    }
}

/// The verified files, held open (see [`verify_bundle`]). Keep it for the life of the process.
#[derive(Debug)]
pub struct BundleGuard {
    _files: Vec<File>,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
