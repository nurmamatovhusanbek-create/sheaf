//! DATA-SETTINGS: `%APPDATA%\Sheaf\settings.json`. No document content, no
//! passwords, no history of file names.

use crate::write_atomic;
use serde::{Deserialize, Serialize};
use sheaf_core::{AppError, ErrorCode, Result, APP_NAME};
use std::path::{Path, PathBuf};

pub const MAX_WORKERS: usize = 32;
const MAX_RECENT_FOLDERS: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `en`, `uz` or `ru` (ADR-I18N).
    pub language: String,
    pub output_dir: Option<PathBuf>,
    pub default_preset: Option<String>,
    /// `None` means cores minus one, at least one.
    pub workers: Option<usize>,
    /// `Fast` or `Accurate`.
    pub ocr_quality: String,
    /// `system`, `light` or `dark`.
    pub theme: String,
    pub recent_folders: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            output_dir: None,
            default_preset: None,
            workers: None,
            ocr_quality: "Fast".into(),
            theme: "system".into(),
            recent_folders: Vec::new(),
        }
    }
}

/// `%APPDATA%\Sheaf` on Windows, the XDG config dir elsewhere.
pub fn config_dir() -> Result<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join(APP_NAME))
        .ok_or_else(|| AppError::new(ErrorCode::Io, "no per-user config directory"))
}

impl Settings {
    /// Worker processes to run (PERF-PARALLEL).
    pub fn worker_count(&self) -> usize {
        let auto = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1));
        self.workers.unwrap_or(auto).clamp(1, MAX_WORKERS)
    }

    /// Replace anything out of range with its default. Settings files are
    /// user-editable, so they are input like any other.
    pub fn sanitized(mut self) -> Self {
        let d = Settings::default();
        if !["en", "uz", "ru"].contains(&self.language.as_str()) {
            self.language = d.language;
        }
        if !["Fast", "Accurate"].contains(&self.ocr_quality.as_str()) {
            self.ocr_quality = d.ocr_quality;
        }
        if !["system", "light", "dark"].contains(&self.theme.as_str()) {
            self.theme = d.theme;
        }
        self.workers = self.workers.map(|w| w.clamp(1, MAX_WORKERS));
        self.recent_folders.truncate(MAX_RECENT_FOLDERS);
        self
    }

    /// Load from `dir`; a missing or unreadable file yields the defaults.
    pub fn load(dir: &Path) -> Settings {
        std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).map_err(|e| AppError::io("create config dir", &e))?;
        let json = serde_json::to_vec_pretty(&self.clone().sanitized())
            .map_err(|e| AppError::internal(format!("serialize settings: {e}")))?;
        write_atomic(&dir.join("settings.json"), &json)
    }
}
