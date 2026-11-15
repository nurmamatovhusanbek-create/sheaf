//! Types behind the PH-3 tools: DATA-SPLITRULE, DATA-NAMEPATTERN, stamps
//! (TOOL-WATERMARK, TOOL-PAGENUM), protection (TOOL-PROTECT), compression
//! levels (TOOL-COMPRESS). Pure logic; the work happens in MOD-PDF.

use crate::error::{AppError, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Longest regex accepted from a preset or dialog.
const MAX_PATTERN_LEN: usize = 500;
/// Longest text handed to a regex: one page of text is far below this.
const MAX_HAYSTACK: usize = 200_000;

/// Compile a user-supplied regex. The `regex` crate matches in linear time
/// and has no backtracking, so a hostile pattern cannot hang the app; the
/// size limits bound memory.
pub fn compile_user_regex(pattern: &str) -> Result<Regex> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_LEN {
        return Err(AppError::invalid("pattern is empty or too long"));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|_| AppError::invalid("pattern is not a valid regular expression"))
}

fn haystack(text: &str) -> &str {
    let mut end = text.len().min(MAX_HAYSTACK);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A password in transit (SEC-SECRETS): wiped when dropped, never printed.
/// Serialized as a plain string because it has to cross the worker pipe once.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Password(String);

impl Password {
    pub fn new(value: impl Into<String>) -> Self {
        Password(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(***)")
    }
}

/// DATA-SPLITRULE.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SplitRule {
    /// One output with every page.
    #[default]
    None,
    /// A new output every `n` pages.
    EveryN(u32),
    /// Blank pages separate the outputs and are dropped.
    BlankSheet,
    /// A page whose text matches the regex starts a new output.
    TextMatch(String),
    /// `"1-3,5,8-"`: each comma-separated group is one output; pages are
    /// 1-based and a group may be open-ended. Pages not listed are left out.
    Ranges(String),
}

/// What the split planner knows about one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageFacts {
    pub blank: bool,
    pub text: String,
}

impl SplitRule {
    /// Which facts the planner needs gathered: `(blank, text)`.
    pub fn needs(&self) -> (bool, bool) {
        (
            matches!(self, SplitRule::BlankSheet),
            matches!(self, SplitRule::TextMatch(_)),
        )
    }

    /// Group page positions `0..facts.len()` into outputs, in order. Except
    /// for `Ranges` (an explicit choice) and dropped blank separators, every
    /// page lands in exactly one output. Empty outputs are never produced.
    pub fn plan(&self, facts: &[PageFacts]) -> Result<Vec<Vec<usize>>> {
        let n = facts.len();
        if n == 0 {
            return Err(AppError::invalid("nothing to split"));
        }
        let all: Vec<usize> = (0..n).collect();
        let groups: Vec<Vec<usize>> = match self {
            SplitRule::None => vec![all],
            SplitRule::EveryN(k) => {
                if *k == 0 {
                    return Err(AppError::invalid("pages per file must be at least 1"));
                }
                all.chunks(*k as usize).map(<[usize]>::to_vec).collect()
            }
            SplitRule::BlankSheet => {
                let mut groups = vec![Vec::new()];
                for i in all {
                    if facts[i].blank {
                        groups.push(Vec::new());
                    } else if let Some(last) = groups.last_mut() {
                        last.push(i);
                    }
                }
                groups
            }
            SplitRule::TextMatch(pattern) => {
                let re = compile_user_regex(pattern)?;
                let mut groups: Vec<Vec<usize>> = vec![Vec::new()];
                for i in all {
                    if re.is_match(haystack(&facts[i].text)) && groups.last().is_some_and(|g| !g.is_empty()) {
                        groups.push(Vec::new());
                    }
                    if let Some(last) = groups.last_mut() {
                        last.push(i);
                    }
                }
                groups
            }
            SplitRule::Ranges(spec) => parse_ranges(spec, n)?,
        };
        let groups: Vec<Vec<usize>> = groups.into_iter().filter(|g| !g.is_empty()).collect();
        if groups.is_empty() {
            return Err(AppError::invalid("the split leaves no pages"));
        }
        Ok(groups)
    }
}

fn parse_ranges(spec: &str, n: usize) -> Result<Vec<Vec<usize>>> {
    let bad = || AppError::invalid("page ranges look like 1-3,5,8-");
    let page = |s: &str| -> Result<usize> {
        let v: usize = s.trim().parse().map_err(|_| bad())?;
        if v == 0 || v > n {
            return Err(AppError::invalid("page range is outside the document"));
        }
        Ok(v)
    };
    let mut groups = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(bad());
        }
        let (from, to) = match part.split_once('-') {
            Some((a, "")) => (page(a)?, n),
            Some((a, b)) => (page(a)?, page(b)?),
            None => (page(part)?, page(part)?),
        };
        if from > to {
            return Err(bad());
        }
        groups.push((from - 1..to).collect());
    }
    if groups.len() > 10_000 {
        return Err(AppError::limit("too many ranges"));
    }
    Ok(groups)
}

/// What a name pattern can draw on for one output (DATA-NAMEPATTERN).
#[derive(Debug, Clone, Default)]
pub struct NameContext<'a> {
    /// `YYYY-MM-DD`.
    pub date: &'a str,
    /// Number of this output within the run, starting at 1.
    pub n: u32,
    /// Stem of the source file's name.
    pub source: &'a str,
    /// Text of the output's first page, for `{match:…}`.
    pub first_page_text: &'a str,
}

/// Expand a DATA-NAMEPATTERN. Tokens: `{date}`, `{n}`, `{counter}` (the same
/// number, zero-padded to three digits), `{source}`, `{match:<regex>}` (the
/// first capture group, or the whole match, found in the first page's text;
/// empty if nothing matches). The result is a raw name: MOD-STORE replaces
/// unsafe characters, caps the length and resolves collisions.
pub fn expand_name(pattern: &str, ctx: &NameContext) -> Result<String> {
    if pattern.is_empty() || pattern.len() > 1_000 {
        return Err(AppError::invalid("name pattern is empty or too long"));
    }
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        // Regexes contain braces of their own: find the brace that closes this token.
        let body = &rest[open + 1..];
        let (mut depth, mut escaped, mut close) = (0usize, false, None);
        for (i, c) in body.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '{' => depth += 1,
                '}' if depth == 0 => {
                    close = Some(i);
                    break;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        let close = close.ok_or_else(|| AppError::invalid("name pattern has an unclosed {"))?;
        let token = &body[..close];
        match token {
            "date" => out.push_str(ctx.date),
            "n" => out.push_str(&ctx.n.to_string()),
            "counter" => out.push_str(&format!("{:03}", ctx.n)),
            "source" => out.push_str(ctx.source),
            _ => match token.strip_prefix("match:") {
                Some(pattern) => {
                    let re = compile_user_regex(pattern)?;
                    if let Some(caps) = re.captures(haystack(ctx.first_page_text)) {
                        let hit = caps.get(1).or_else(|| caps.get(0)).map_or("", |m| m.as_str());
                        out.push_str(hit.trim());
                    }
                }
                None => return Err(AppError::invalid("name pattern has an unknown token")),
            },
        }
        rest = &body[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// TOOL-COMPRESS levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressLevel {
    /// Lossless re-pack only.
    Low,
    /// Images to 150 dpi, JPEG quality 70.
    Recommended,
    /// Images to 96 dpi, JPEG quality 50.
    Extreme,
}

impl CompressLevel {
    /// Target resolution and JPEG quality for images; `None` leaves images alone.
    pub fn image_target(self) -> Option<(u32, u8)> {
        match self {
            CompressLevel::Low => None,
            CompressLevel::Recommended => Some((150, 70)),
            CompressLevel::Extreme => Some((96, 50)),
        }
    }
}

/// TOOL-PROTECT settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protect {
    /// Needed to open the file. May be empty: the file then opens for
    /// anyone, with the restrictions below.
    pub user_pw: Password,
    /// Lifts the restrictions. Empty means a random one nobody knows.
    #[serde(default)]
    pub owner_pw: Password,
    pub allow_print: bool,
    pub allow_copy: bool,
    pub allow_edit: bool,
}

/// Where a stamp sits on the page as displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    Center,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

/// Text or an image drawn over pages at build time: a watermark or a page
/// number (TOOL-WATERMARK, TOOL-PAGENUM).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    /// `{n}` is the page number, `{N}` the page count. Ignored when `image` is set.
    pub text: String,
    /// Session-relative path of a PNG to draw instead of text.
    #[serde(default)]
    pub image: Option<String>,
    /// Text size in points, or the image's width in points.
    pub size_pt: f32,
    /// 0 (invisible) to 1 (solid).
    pub opacity: f32,
    /// Counter-clockwise, in degrees, about the stamp's own centre.
    #[serde(default)]
    pub rotate_deg: f32,
    #[serde(default)]
    pub anchor: Anchor,
    /// Distance from the page edge, in points.
    pub margin_pt: f32,
    pub color: [u8; 3],
    /// First and last page (1-based, in the output) that get the stamp; `None` is the last page.
    pub from_page: u32,
    #[serde(default)]
    pub to_page: Option<u32>,
    /// `{n}` on `from_page` shows this number.
    pub first_number: i32,
}

impl Stamp {
    /// Page numbers bottom-centre, "n / N".
    pub fn page_numbers() -> Stamp {
        Stamp {
            text: "{n} / {N}".into(),
            image: None,
            size_pt: 10.0,
            opacity: 1.0,
            rotate_deg: 0.0,
            anchor: Anchor::BottomCenter,
            margin_pt: 24.0,
            color: [0, 0, 0],
            from_page: 1,
            to_page: None,
            first_number: 1,
        }
    }

    /// A diagonal watermark across the middle of every page.
    pub fn watermark(text: &str) -> Stamp {
        Stamp {
            text: text.into(),
            size_pt: 64.0,
            opacity: 0.25,
            rotate_deg: 45.0,
            anchor: Anchor::Center,
            margin_pt: 0.0,
            color: [128, 128, 128],
            ..Stamp::page_numbers()
        }
    }

    pub fn validate(&self) -> Result<()> {
        let finite = [self.size_pt, self.opacity, self.rotate_deg, self.margin_pt]
            .iter()
            .all(|v| v.is_finite());
        let ok = finite
            && (1.0..=2_000.0).contains(&self.size_pt)
            && (0.0..=1.0).contains(&self.opacity)
            && (0.0..=2_000.0).contains(&self.margin_pt)
            && self.from_page >= 1
            && self.to_page.is_none_or(|t| t >= self.from_page)
            && self.text.chars().count() <= 500
            && (self.image.is_some() || !self.text.trim().is_empty());
        if ok {
            Ok(())
        } else {
            Err(AppError::invalid("stamp settings out of range"))
        }
    }

    /// The text for output page `page` (1-based) of `total`, or `None` if the
    /// stamp does not apply to that page.
    pub fn text_for(&self, page: u32, total: u32) -> Option<String> {
        if page < self.from_page || self.to_page.is_some_and(|t| page > t) {
            return None;
        }
        let number = i64::from(self.first_number) + i64::from(page - self.from_page);
        Some(
            self.text
                .replace("{n}", &number.to_string())
                .replace("{N}", &total.to_string()),
        )
    }
}

/// How an image source is placed on its page (TOOL-IMG2PDF).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ImageFit {
    /// The page is exactly as large as the image.
    #[default]
    Fit,
    /// A4, image centred inside the margin, never enlarged.
    A4 {
        margin_pt: f32,
    },
    Letter {
        margin_pt: f32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(blank: &[bool], text: &[&str]) -> Vec<PageFacts> {
        blank
            .iter()
            .zip(text)
            .map(|(b, t)| PageFacts {
                blank: *b,
                text: (*t).to_string(),
            })
            .collect()
    }
    fn plain(n: usize) -> Vec<PageFacts> {
        vec![PageFacts::default(); n]
    }

    #[test]
    fn split_rules_group_pages() {
        assert_eq!(SplitRule::None.plan(&plain(3)).expect("plan"), [[0, 1, 2]]);
        assert_eq!(
            SplitRule::EveryN(2).plan(&plain(5)).expect("plan"),
            vec![vec![0, 1], vec![2, 3], vec![4]]
        );
        assert!(SplitRule::EveryN(0).plan(&plain(5)).is_err());
        assert!(SplitRule::None.plan(&[]).is_err());

        // Blank sheets separate and are dropped; doubled or trailing blanks make no empty files.
        let f = facts(&[false, false, true, false, true, true, false, true], &[""; 8]);
        assert_eq!(
            SplitRule::BlankSheet.plan(&f).expect("plan"),
            vec![vec![0, 1], vec![3], vec![6]]
        );
        assert!(SplitRule::BlankSheet.plan(&facts(&[true, true], &["", ""])).is_err());

        // A matching page starts a new output; pages before the first match stay together.
        let f = facts(
            &[false; 5],
            &["cover", "Shartnoma № 12", "more", "Shartnoma № 13", "end"],
        );
        let rule = SplitRule::TextMatch(r"Shartnoma\s*№".into());
        assert_eq!(rule.plan(&f).expect("plan"), vec![vec![0], vec![1, 2], vec![3, 4]]);
        assert!(SplitRule::TextMatch("(".into()).plan(&f).is_err());

        let ranges = |s: &str| SplitRule::Ranges(s.into()).plan(&plain(8));
        assert_eq!(
            ranges("1-3,5, 7-").expect("plan"),
            vec![vec![0, 1, 2], vec![4], vec![6, 7]]
        );
        for bad in ["", "0-2", "3-1", "9", "a-b", "1,,2", "1-2-3"] {
            assert!(ranges(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_page_lands_in_exactly_one_output() {
        // TEST-SPLIT: no page lost or duplicated (blank separators aside).
        let f = facts(
            &[false, true, false, false, true, false, false],
            &["a", "", "start b", "c", "", "start d", "e"],
        );
        for rule in [
            SplitRule::None,
            SplitRule::EveryN(3),
            SplitRule::BlankSheet,
            SplitRule::TextMatch("start".into()),
        ] {
            let mut seen: Vec<usize> = rule.plan(&f).expect("plan").concat();
            let ordered = seen.clone();
            seen.sort_unstable();
            assert_eq!(seen, ordered, "{rule:?}: order is kept");
            seen.dedup();
            let expected: Vec<usize> = match rule {
                SplitRule::BlankSheet => vec![0, 2, 3, 5, 6],
                _ => (0..7).collect(),
            };
            assert_eq!(seen, expected, "{rule:?}");
        }
    }

    #[test]
    fn name_patterns_expand() {
        let ctx = NameContext {
            date: "2026-11-14",
            n: 7,
            source: "scan",
            first_page_text: "Shartnoma № 45/A dated",
        };
        let name = |p: &str| expand_name(p, &ctx);
        assert_eq!(name("{date}_{source}_{n}").expect("name"), "2026-11-14_scan_7");
        assert_eq!(name("{counter}.pdf").expect("name"), "007.pdf");
        assert_eq!(
            name(r"{date}_{match:Shartnoma\s*№\s*(\S+)}_{counter}").expect("name"),
            "2026-11-14_45/A_007"
        );
        assert_eq!(
            name(r"{match:\d{2}}").expect("name"),
            "45",
            "braces inside the regex are fine"
        );
        assert_eq!(
            name(r"{match:no such thing}-x").expect("name"),
            "-x",
            "no match expands to nothing"
        );
        assert_eq!(name("plain").expect("name"), "plain");
        for bad in ["", "{unknown}", "{date", "{match:(}", "{match:}"] {
            assert!(name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn hostile_patterns_are_bounded() {
        // The classic catastrophic-backtracking pattern runs in linear time here.
        let re = compile_user_regex("(a+)+$").expect("compiles");
        let text = format!("{}!", "a".repeat(100_000));
        let t = std::time::Instant::now();
        assert!(!re.is_match(&text));
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        assert!(compile_user_regex(&"a".repeat(501)).is_err());
        assert!(
            compile_user_regex(r"(\w{1000}){1000}").is_err(),
            "oversized programs are refused"
        );
    }

    #[test]
    fn passwords_do_not_print() {
        let pw = Password::new("hunter2");
        assert_eq!(format!("{pw:?}"), "Password(***)");
        assert_eq!(pw.expose(), "hunter2");
        let protect = Protect {
            user_pw: pw,
            owner_pw: Password::default(),
            allow_print: true,
            allow_copy: false,
            allow_edit: false,
        };
        assert!(!format!("{protect:?}").contains("hunter2"));
    }

    #[test]
    fn stamps_number_pages_and_validate() {
        let mut s = Stamp::page_numbers();
        assert_eq!(s.text_for(3, 10).as_deref(), Some("3 / 10"));
        s.from_page = 2; // skip the first page, start numbering at 1 on page 2
        s.to_page = Some(4);
        assert_eq!(s.text_for(1, 10), None);
        assert_eq!(s.text_for(2, 10).as_deref(), Some("1 / 10"));
        assert_eq!(s.text_for(5, 10), None);
        s.validate().expect("valid");
        for bad in [
            Stamp {
                opacity: 1.5,
                ..Stamp::page_numbers()
            },
            Stamp {
                size_pt: f32::NAN,
                ..Stamp::page_numbers()
            },
            Stamp {
                text: " ".into(),
                ..Stamp::page_numbers()
            },
            Stamp {
                from_page: 0,
                ..Stamp::page_numbers()
            },
            Stamp {
                to_page: Some(1),
                from_page: 2,
                ..Stamp::page_numbers()
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }
}
