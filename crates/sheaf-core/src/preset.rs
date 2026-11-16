//! PRESET-FORMAT: a saved chain of stages that runs on files in one go.
//!
//! A preset holds settings only: it never holds a password (SEC-SECRETS). The
//! JSON is read from a folder the person owns, so every field is checked
//! again here before anything uses it (SEC-IPC): closed sets are words, text
//! has a length cap, and an unknown field is refused rather than ignored.

use crate::model::{ColorMode, Deskew, Filter};
use crate::tools::{CompressLevel, SplitRule};
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};

/// Longest preset name, in characters.
pub const MAX_PRESET_NAME: usize = 60;
const MAX_TEXT: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    /// The scan stage; skipped when the preset runs on files that exist already.
    pub scan: Option<PresetScan>,
    pub cleanup: PresetCleanup,
    pub ocr: PresetOcr,
    pub split: PresetSplit,
    /// DATA-NAMEPATTERN for the output files, without `.pdf`.
    pub name_pattern: String,
    /// `Low`, `Recommended` or `Extreme`; `None` leaves the file as it is.
    pub compress: Option<String>,
    /// Where outputs go; `None` is the folder of each input. Checked by the path policy when used.
    pub out_dir: Option<String>,
}

impl Default for Preset {
    fn default() -> Self {
        Preset {
            name: String::new(),
            scan: None,
            cleanup: PresetCleanup::default(),
            ocr: PresetOcr::default(),
            split: PresetSplit::default(),
            name_pattern: "{source}.processed".into(),
            compress: None,
            out_dir: None,
        }
    }
}

/// DATA-SCANPROFILE in words (the scan dialog's own closed sets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct PresetScan {
    /// `flatbed` or `feeder`.
    pub source: String,
    pub duplex: bool,
    pub dpi: u32,
    /// `color`, `gray` or `bw`.
    pub color: String,
    /// `a4`, `a3`, `letter` or `auto`.
    pub paper: String,
}

impl Default for PresetScan {
    fn default() -> Self {
        PresetScan {
            source: "feeder".into(),
            duplex: false,
            dpi: 300,
            color: "gray".into(),
            paper: "a4".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct PresetCleanup {
    /// `color`, `gray` or `bw`; empty leaves the pages as they are.
    pub mode: String,
    pub deskew: bool,
    pub despeckle: bool,
    /// Leave out pages that are blank.
    pub drop_blank: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct PresetOcr {
    /// `Off` or `Auto` (recognize the pages that have no text).
    pub mode: String,
    /// `None` detects the script and picks languages.
    pub langs: Option<Vec<String>>,
}

impl Default for PresetOcr {
    fn default() -> Self {
        PresetOcr {
            mode: "Off".into(),
            langs: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct PresetSplit {
    /// `None`, `EveryN`, `BlankSheet`, `TextMatch` or `Ranges`.
    pub rule: String,
    pub n: u32,
    pub pattern: String,
    pub ranges: String,
}

impl Default for PresetSplit {
    fn default() -> Self {
        PresetSplit {
            rule: "None".into(),
            n: 2,
            pattern: String::new(),
            ranges: String::new(),
        }
    }
}

fn oneof(what: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(AppError::invalid(format!("unknown {what} in preset")))
    }
}

fn short(what: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(AppError::invalid(format!(
            "{what} in preset is too long or has control characters"
        )));
    }
    Ok(())
}

impl Preset {
    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_PRESET_NAME || name.chars().any(char::is_control) {
            return Err(AppError::invalid("preset name must be 1 to 60 characters"));
        }
        if let Some(scan) = &self.scan {
            oneof("scan source", &scan.source, &["flatbed", "feeder"])?;
            oneof("scan colour", &scan.color, &["color", "gray", "bw"])?;
            oneof("scan paper", &scan.paper, &["a4", "a3", "letter", "auto"])?;
            if ![150, 200, 300, 600].contains(&scan.dpi) {
                return Err(AppError::invalid("unknown scan resolution in preset"));
            }
            if scan.duplex && scan.source == "flatbed" {
                return Err(AppError::invalid("two-sided scanning needs the feeder"));
            }
        }
        oneof("cleanup mode", &self.cleanup.mode, &["", "color", "gray", "bw"])?;
        oneof("OCR mode", &self.ocr.mode, &["Off", "Auto"])?;
        if let Some(langs) = &self.ocr.langs {
            if langs.len() > 8
                || langs
                    .iter()
                    .any(|l| l.len() > 12 || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            {
                return Err(AppError::invalid("bad OCR languages in preset"));
            }
        }
        self.split_rule()?;
        short("name pattern", &self.name_pattern, MAX_TEXT)?;
        if self.name_pattern.is_empty() {
            return Err(AppError::invalid("the name pattern is empty"));
        }
        if let Some(level) = &self.compress {
            oneof("compression level", level, &["Low", "Recommended", "Extreme"])?;
        }
        if let Some(dir) = &self.out_dir {
            short("output folder", dir, 1_024)?;
        }
        Ok(())
    }

    /// The split rule, checked.
    pub fn split_rule(&self) -> Result<SplitRule> {
        let s = &self.split;
        short("split pattern", &s.pattern, MAX_TEXT)?;
        short("split ranges", &s.ranges, 200)?;
        Ok(match s.rule.as_str() {
            "None" => SplitRule::None,
            "EveryN" if (1..=10_000).contains(&s.n) => SplitRule::EveryN(s.n),
            "EveryN" => return Err(AppError::invalid("split every N needs N between 1 and 10000")),
            "BlankSheet" => SplitRule::BlankSheet,
            "TextMatch" if !s.pattern.is_empty() => SplitRule::TextMatch(s.pattern.clone()),
            "Ranges" if !s.ranges.is_empty() => SplitRule::Ranges(s.ranges.clone()),
            "TextMatch" | "Ranges" => return Err(AppError::invalid("the split rule needs its text")),
            _ => return Err(AppError::invalid("unknown split rule in preset")),
        })
    }

    pub fn compress_level(&self) -> Option<CompressLevel> {
        match self.compress.as_deref() {
            Some("Low") => Some(CompressLevel::Low),
            Some("Recommended") => Some(CompressLevel::Recommended),
            Some("Extreme") => Some(CompressLevel::Extreme),
            _ => None,
        }
    }

    /// The page filter the cleanup stage asks for.
    pub fn filter(&self) -> Filter {
        let c = &self.cleanup;
        Filter {
            mode: match c.mode.as_str() {
                "gray" => ColorMode::Gray,
                "bw" => ColorMode::BW,
                _ => ColorMode::Color,
            },
            deskew: if c.deskew { Deskew::Auto } else { Deskew::Off },
            despeckle: c.despeckle,
            ..Filter::default()
        }
    }

    pub fn recognizes(&self) -> bool {
        self.ocr.mode == "Auto"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Preset {
        Preset {
            name: name.into(),
            ..Preset::default()
        }
    }

    #[test]
    fn the_example_in_the_spec_parses_and_validates() {
        let json = r#"{
          "name": "Contract scan",
          "scan":    { "source": "feeder", "duplex": true, "dpi": 300, "color": "gray", "paper": "a4" },
          "cleanup": { "deskew": true, "despeckle": true, "dropBlank": true },
          "ocr":     { "mode": "Auto" },
          "split":   { "rule": "BlankSheet" },
          "namePattern": "{date}_{counter}",
          "compress": "Recommended",
          "outDir":  "D:\\Scans\\Contracts"
        }"#;
        let p: Preset = serde_json::from_str(json).expect("parses");
        p.validate().expect("valid");
        assert_eq!(p.split_rule().expect("rule"), SplitRule::BlankSheet);
        assert_eq!(p.compress_level(), Some(CompressLevel::Recommended));
        assert!(p.recognizes() && p.cleanup.drop_blank);
        let f = p.filter();
        assert_eq!((f.deskew, f.despeckle), (Deskew::Auto, true));
        // And it survives a round trip.
        let back: Preset = serde_json::from_str(&serde_json::to_string(&p).expect("json")).expect("again");
        assert_eq!(back, p);
    }

    #[test]
    fn a_default_preset_with_a_name_is_valid_and_changes_nothing() {
        let p = named("Plain");
        p.validate().expect("valid");
        assert!(p.filter().is_identity());
        assert_eq!(p.split_rule().expect("rule"), SplitRule::None);
        assert_eq!(p.compress_level(), None);
        assert!(!p.recognizes());
    }

    #[test]
    fn bad_values_are_refused() {
        let bad = |f: &dyn Fn(&mut Preset)| {
            let mut p = named("x");
            f(&mut p);
            p.validate().expect_err("must be refused")
        };
        bad(&|p| p.name = String::new());
        bad(&|p| p.name = "   ".into());
        bad(&|p| p.name = "a".repeat(61));
        bad(&|p| p.name = "tab\there".into());
        bad(&|p| p.ocr.mode = "Always".into());
        bad(&|p| p.cleanup.mode = "sepia".into());
        bad(&|p| p.compress = Some("Tiny".into()));
        bad(&|p| p.split.rule = "Halves".into());
        bad(&|p| {
            p.split.rule = "EveryN".into();
            p.split.n = 0;
        });
        bad(&|p| p.split.rule = "TextMatch".into()); // no pattern
        bad(&|p| p.name_pattern = String::new());
        bad(&|p| p.name_pattern = "x".repeat(2_000));
        bad(&|p| p.ocr.langs = Some(vec!["../../etc".into()]));
        bad(&|p| p.out_dir = Some("a\nb".into()));
        bad(&|p| {
            p.scan = Some(PresetScan {
                dpi: 77,
                ..PresetScan::default()
            })
        });
        bad(&|p| {
            p.scan = Some(PresetScan {
                source: "flatbed".into(),
                duplex: true,
                ..PresetScan::default()
            })
        });
        // An unknown field is an error, not something quietly dropped.
        assert!(serde_json::from_str::<Preset>(r#"{"name":"x","surprise":1}"#).is_err());
    }
}
