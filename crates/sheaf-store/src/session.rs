//! The per-run session directory: thumbnails, caches, intermediate results.
//! Everything in it is deleted on exit; leftovers from a crash are swept at
//! the next start (SEC-TEMP, FLOW-LIFECYCLE).

use crate::token;
use sheaf_core::{AppError, ErrorCode, Result, APP_NAME};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

const LOCK_FILE: &str = ".lock";
/// A session dir younger than this is never swept: another instance may be in the middle of
/// creating it (directory made, lock file not yet locked).
const SWEEP_GRACE: Duration = Duration::from_secs(30);
const PREFIX: &str = "session-";

/// Where session dirs live: `%LOCALAPPDATA%\Sheaf` on Windows, the XDG cache
/// dir elsewhere (non-Windows builds exist for tests, NG-XPLAT).
pub fn default_base_dir() -> Result<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    base.map(|b| b.join(APP_NAME))
        .ok_or_else(|| AppError::new(ErrorCode::Io, "no per-user data directory"))
}

#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
    /// Held locked for the life of the owning process; `None` for a worker
    /// that merely works inside its parent's session.
    lock: Option<File>,
}

impl Session {
    /// Create `<base>/session-<pid>-<rand>/`, private to the current user.
    pub fn create(base: &Path) -> Result<Session> {
        fs::create_dir_all(base).map_err(|e| AppError::io("create session base", &e))?;
        let base = base
            .canonicalize()
            .map_err(|e| AppError::io("resolve session base", &e))?;
        let base = crate::paths::simplify_verbatim(base);
        let dir = base.join(format!("{PREFIX}{}-{}", std::process::id(), token()));
        create_private_dir(&dir)?;
        // Before anything is made inside it, so all of it inherits the label a low-integrity worker needs.
        crate::harden::label_session_low(&dir);
        let lock = File::create(dir.join(LOCK_FILE)).map_err(|e| AppError::io("create session lock", &e))?;
        lock.try_lock()
            .map_err(|_| AppError::new(ErrorCode::Io, "session lock is already held"))?;
        let session = Session { dir, lock: Some(lock) };
        for sub in ["thumbs", "cache", "render", "results", "scans"] {
            fs::create_dir(session.dir.join(sub)).map_err(|e| AppError::io("create session subdir", &e))?;
        }
        Ok(session)
    }

    /// Use an existing session dir without owning it (worker side).
    pub fn attach(dir: &Path) -> Result<Session> {
        let dir = dir.canonicalize().map_err(|e| AppError::io("open session dir", &e))?;
        if !dir.join(LOCK_FILE).is_file() {
            return Err(AppError::new(ErrorCode::PathRejected, "not a session directory"));
        }
        Ok(Session { dir, lock: None })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Resolve a session-relative path. Rejects anything that could leave the
    /// session dir: absolute paths, `..`, drive or UNC prefixes (SEC-PATHS).
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let p = Path::new(rel);
        let plain = !rel.is_empty()
            && !rel.contains(['\\', ':', '\0'])
            && rel.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
            && p.components().all(|c| matches!(c, Component::Normal(_)));
        if !plain {
            return Err(AppError::new(
                ErrorCode::PathRejected,
                "path must stay inside the session dir",
            ));
        }
        Ok(self.dir.join(p))
    }

    /// Delete the session dir now. Also happens on drop.
    pub fn close(mut self) -> Result<()> {
        self.remove()
    }

    fn remove(&mut self) -> Result<()> {
        if let Some(lock) = self.lock.take() {
            drop(lock); // Windows will not delete a directory holding an open file.
            fs::remove_dir_all(&self.dir).map_err(|e| AppError::io("remove session dir", &e))?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    // On Windows the directory inherits the ACL of %LOCALAPPDATA%, which
    // already grants access to the owning user, SYSTEM and administrators only.
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    builder.create(dir).map_err(|e| AppError::io("create session dir", &e))
}

/// Remove session dirs left behind by processes that are gone. A live
/// session keeps its lock file locked, so it is never touched. Returns how
/// many were removed.
pub fn sweep_stale(base: &Path) -> usize {
    sweep_stale_after(base, SWEEP_GRACE)
}

/// [`sweep_stale`] with an explicit grace period for fresh directories.
pub fn sweep_stale_after(base: &Path, grace: Duration) -> usize {
    let Ok(entries) = fs::read_dir(base) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        let named = entry.file_name().to_str().is_some_and(|n| n.starts_with(PREFIX));
        let real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if !named || !real_dir {
            continue;
        }
        // A directory is created before its lock file exists and before the lock is held, so
        // "nobody holds it" only means "dead" once it is not brand new.
        let settled = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() >= grace);
        let unowned = match File::open(dir.join(LOCK_FILE)) {
            Ok(lock) => lock.try_lock().is_ok(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        if settled && unowned && fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}
