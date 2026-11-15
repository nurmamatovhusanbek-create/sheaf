//! MOD-STORE: session dir, cache, settings, path policy, content hashing.
//!
//! See `ARCHITECTURE.md`, anchor `mod-store`. Presets I/O arrives with PH-6.

pub mod paths;
pub mod session;
pub mod settings;

pub use paths::{
    allow_input, allow_output, check_output_dir, create_output_dir, publish, sanitize_file_name, AllowedInput,
};
pub use session::{default_base_dir, sweep_stale, Session};
pub use settings::{config_dir, Settings};

use sha2::{Digest, Sha256};
use sheaf_core::{AppError, Result};
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

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
