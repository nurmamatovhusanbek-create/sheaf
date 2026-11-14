//! MOD-OCR: engine trait, tesseract driver, langs, cache, search.
//!
//! Recognition runs inside PROC-WORKER; the cache reader and the search
//! index are used by the main process. See `ARCHITECTURE.md`, anchor `mod-ocr`.

pub mod cache;
pub mod engine;
pub mod index;
pub mod langs;
pub mod tesseract;
pub mod tsv;

pub use cache::{cache_key, CacheEntry, Meta};
pub use engine::{OcrEngine, Osd, RecOpts};
pub use index::{Hit, SearchIndex};
pub use langs::choose_langs;
pub use tesseract::Tesseract;
