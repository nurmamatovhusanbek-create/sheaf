//! DATA-OCRRESULT and DATA-OCRPOLICY pieces shared across modules.

use serde::{Deserialize, Serialize};

/// One recognized word. `bbox` is left, top, width, height in pixels of the
/// image the engine saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub bbox: [u32; 4],
    /// 0 to 100.
    pub conf: f32,
    /// Running line number on the page; words of one line share it.
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Script {
    Latin,
    Cyrillic,
    /// Detection failed, was not confident, or found something else.
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OcrResult {
    pub words: Vec<Word>,
    /// Clockwise quarter turns (0, 90, 180, 270) applied to the image before
    /// recognition because detection found the page sideways or upside down.
    pub orientation_deg: u16,
    pub script: Script,
    pub langs_used: Vec<String>,
    pub mean_conf: f32,
}

impl OcrResult {
    /// Plain text, one line per recognized line.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut last = None;
        for w in &self.words {
            match last {
                Some(line) if line == w.line => out.push(' '),
                Some(_) => out.push('\n'),
                None => {}
            }
            out.push_str(&w.text);
            last = Some(w.line);
        }
        out
    }
}
