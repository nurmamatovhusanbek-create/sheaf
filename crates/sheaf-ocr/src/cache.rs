//! OCR-CACHE: results keyed by the pixels the engine saw (DATA-CACHEKEY).
//! Lives at `<session>/cache/<key>/{result.tsv, page.pdf, meta.json}`.

use crate::tsv;
use serde::{Deserialize, Serialize};
use sheaf_core::{AppError, ErrorCode, OcrResult, Result, Script};
use sheaf_store::Session;
use std::path::PathBuf;

/// `sha256(pixels ‖ size) ‖ engine ‖ version ‖ langs ‖ params`, folded into
/// one hex string. Any edit that changes pixels changes the key; edits that
/// do not (reorder, rotate by quarter turns) keep it.
pub fn cache_key(
    pixels: &[u8],
    size: (u32, u32),
    engine: &str,
    version: &str,
    langs: Option<&[String]>,
    params: &str,
) -> String {
    let langs = match langs {
        Some(list) => list.join("+"),
        None => "auto".to_string(),
    };
    let header = format!("{}x{}\n{engine}\n{version}\n{langs}\n{params}\n", size.0, size.1);
    sheaf_store::hex(&sheaf_store::hash_bytes(&[header.as_bytes(), pixels]))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub engine: String,
    pub version: String,
    pub langs: Vec<String>,
    pub script: Script,
    /// Clockwise degrees the image was turned before recognition.
    pub rotate: u16,
    pub mean_conf: f32,
    pub words: u32,
    /// Size of the image the engine saw, before `rotate`.
    pub image_px: (u32, u32),
    pub dpi: u32,
}

pub struct CacheEntry {
    dir: PathBuf,
}

fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl CacheEntry {
    /// The entry for `key`, whether or not it exists yet. Keys arrive over
    /// IPC, so anything that is not a plain hash is refused (SEC-PATHS).
    pub fn at(session: &Session, key: &str) -> Result<CacheEntry> {
        if !valid_key(key) {
            return Err(AppError::new(ErrorCode::InvalidArg, "malformed cache key"));
        }
        Ok(CacheEntry {
            dir: session.resolve(&format!("cache/{key}"))?,
        })
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }
    pub fn text_pdf(&self) -> PathBuf {
        self.dir.join("page.pdf")
    }
    pub fn tsv(&self) -> PathBuf {
        self.dir.join("result.tsv")
    }

    /// `Some` when the entry is complete.
    pub fn meta(&self) -> Option<Meta> {
        let bytes = std::fs::read(self.dir.join("meta.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Words and confidences (CMD-OCR-RESULT).
    pub fn result(&self) -> Result<OcrResult> {
        let meta = self.meta().ok_or_else(|| AppError::not_found("OCR result"))?;
        let text = std::fs::read_to_string(self.tsv()).map_err(|e| AppError::io("read OCR result", &e))?;
        let mut result = tsv::parse(&text);
        result.langs_used = meta.langs;
        result.script = meta.script;
        result.orientation_deg = meta.rotate;
        Ok(result)
    }

    /// Publish a finished work directory as this entry. `meta.json` is written
    /// last inside the work dir, so a visible entry is always complete. If
    /// another worker got there first, theirs stays and ours is dropped.
    pub fn commit(&self, work: &std::path::Path, meta: &Meta) -> Result<()> {
        let json = serde_json::to_vec(meta).map_err(|e| AppError::internal(format!("encode OCR meta: {e}")))?;
        sheaf_store::write_atomic(&work.join("meta.json"), &json)?;
        if std::fs::rename(work, &self.dir).is_err() {
            let _ = std::fs::remove_dir_all(work);
            if self.meta().is_none() {
                return Err(AppError::new(ErrorCode::Io, "could not store OCR result"));
            }
        }
        Ok(())
    }
}
