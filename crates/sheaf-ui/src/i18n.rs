//! ADR-I18N: key-based strings in English, Uzbek (Latin) and Russian. The
//! language files are embedded in the binary; nothing is read at run time.

use serde_json::Value;
use std::collections::HashMap;

pub const LANGUAGES: [(&str, &str); 3] = [("en", "English"), ("uz", "Oʻzbekcha"), ("ru", "Русский")];

const EN: &str = include_str!("../i18n/en.json");
const UZ: &str = include_str!("../i18n/uz.json");
const RU: &str = include_str!("../i18n/ru.json");

type Table = HashMap<String, Value>;

fn parse(json: &str) -> Table {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn raw(code: &str) -> &'static str {
    match code {
        "uz" => UZ,
        "ru" => RU,
        _ => EN,
    }
}

/// Plural form index for a count. Russian has three forms, the others two.
pub fn plural_index(code: &str, n: i64) -> usize {
    let k = n.unsigned_abs();
    if code == "ru" {
        let (m10, m100) = (k % 10, k % 100);
        if m10 == 1 && m100 != 11 {
            0
        } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
            1
        } else {
            2
        }
    } else if k == 1 {
        0
    } else {
        1
    }
}

pub struct I18n {
    code: &'static str,
    table: Table,
    fallback: Table,
}

impl I18n {
    pub fn new(code: &str) -> I18n {
        let code = LANGUAGES.iter().find(|(c, _)| *c == code).map_or("en", |(c, _)| *c);
        I18n {
            code,
            table: parse(raw(code)),
            fallback: parse(EN),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    fn lookup(&self, key: &str) -> Option<&Value> {
        self.table.get(key).or_else(|| self.fallback.get(key))
    }

    fn fill(template: &str, vars: &[(&str, &str)]) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            match after.find('}') {
                Some(end) if after[..end].chars().all(|c| c.is_alphanumeric() || c == '_') && end > 0 => {
                    let name = &after[..end];
                    match vars.iter().find(|(k, _)| *k == name) {
                        Some((_, v)) => out.push_str(v),
                        None => out.push_str(&rest[start..start + end + 2]), // left visible
                    }
                    rest = &after[end + 1..];
                }
                _ => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Translate `key` with `{name}` placeholders from `vars`. An unknown key is shown as itself.
    pub fn tv(&self, key: &str, vars: &[(&str, &str)]) -> String {
        match self.lookup(key) {
            Some(Value::String(s)) => Self::fill(s, vars),
            Some(Value::Array(forms)) => forms
                .first()
                .and_then(Value::as_str)
                .map_or_else(|| key.to_string(), |s| Self::fill(s, vars)),
            _ => key.to_string(),
        }
    }

    pub fn t(&self, key: &str) -> String {
        self.tv(key, &[])
    }

    /// Translate a plural-aware key for the count `n` (also available as `{n}`).
    pub fn tn(&self, key: &str, n: i64, vars: &[(&str, &str)]) -> String {
        let ns = n.to_string();
        let mut all: Vec<(&str, &str)> = vec![("n", &ns)];
        all.extend_from_slice(vars);
        match self.lookup(key) {
            Some(Value::Array(forms)) if !forms.is_empty() => {
                let i = plural_index(self.code, n).min(forms.len() - 1);
                forms[i]
                    .as_str()
                    .map_or_else(|| key.to_string(), |s| Self::fill(s, &all))
            }
            Some(Value::String(s)) => Self::fill(s, &all),
            _ => key.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn table(code: &str) -> Table {
        parse(raw(code))
    }

    fn placeholders(v: &Value) -> BTreeSet<String> {
        let first = match v {
            Value::Array(a) => a.first().and_then(Value::as_str).unwrap_or(""),
            other => other.as_str().unwrap_or(""),
        };
        let mut out = BTreeSet::new();
        let mut rest = first;
        while let Some(i) = rest.find('{') {
            if let Some(j) = rest[i..].find('}') {
                let name = &rest[i + 1..i + j];
                // Only plain names are placeholders; `{match:pattern}` is literal text.
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.insert(name.to_string());
                }
                rest = &rest[i + j + 1..];
            } else {
                break;
            }
        }
        out
    }

    #[test]
    fn every_language_has_exactly_the_same_keys() {
        let en: BTreeSet<_> = table("en").into_keys().collect();
        for code in ["uz", "ru"] {
            let other: BTreeSet<_> = table(code).into_keys().collect();
            assert_eq!(
                en.difference(&other).collect::<Vec<_>>(),
                Vec::<&String>::new(),
                "{code} lacks"
            );
            assert_eq!(
                other.difference(&en).collect::<Vec<_>>(),
                Vec::<&String>::new(),
                "{code} has extra"
            );
        }
    }

    #[test]
    fn translations_keep_their_placeholders_and_plural_forms() {
        let en = table("en");
        for code in ["uz", "ru"] {
            let t = table(code);
            for (key, value) in &en {
                let other = &t[key];
                assert_eq!(placeholders(other), placeholders(value), "{code}:{key}");
                if let Value::Array(forms) = other {
                    let ok = if code == "ru" { [1, 3] } else { [1, 2] };
                    assert!(ok.contains(&forms.len()), "{code}:{key} has {} forms", forms.len());
                    for f in forms {
                        assert_eq!(
                            placeholders(&Value::String(f.as_str().unwrap_or("").into())),
                            placeholders(value),
                            "{code}:{key}"
                        );
                    }
                }
            }
        }
        for key in ["tray.count", "job.files", "scope.selected", "ocr.doubtful"] {
            assert_eq!(table("ru")[key].as_array().map(Vec::len), Some(3), "{key}");
        }
    }

    /// Every `t("…")`, `tv("…"`, `tn("…"` in the sources names a key that exists,
    /// and so does every key built from a known family.
    #[test]
    fn every_key_the_code_asks_for_exists() {
        let en = table("en");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(dir).expect("src") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") || path.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read");
            for marker in [".t(\"", ".tv(\"", ".tn(\""] {
                for piece in src.split(marker).skip(1) {
                    if let Some(end) = piece.find('"') {
                        let key = &piece[..end];
                        if !en.contains_key(key) {
                            missing.push(format!(
                                "{}: {key}",
                                path.file_name().and_then(|n| n.to_str()).unwrap_or("")
                            ));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn keys_built_at_run_time_all_exist() {
        let en = table("en");
        let mut want = Vec::new();
        for id in [
            "merge", "split", "extract", "remove", "organize", "rotate", "crop", "compress", "img2pdf", "pdf2img",
            "stamp", "lock", "ocr", "scan",
        ] {
            want.push(format!("tool.{id}.name"));
            want.push(format!("tool.{id}.hint"));
        }
        for id in ["merge", "compress", "ocr", "split", "pdf2img", "img2pdf", "rotate"] {
            want.push(format!("tool.{id}.name"));
        }
        for a in [
            "TopLeft",
            "TopCenter",
            "TopRight",
            "Center",
            "BottomLeft",
            "BottomCenter",
            "BottomRight",
        ] {
            want.push(format!("anchor.{a}"));
        }
        for m in ["Color", "Gray", "BW"] {
            want.push(format!("filter.{m}"));
        }
        for s in ["None", "EveryN", "BlankSheet", "TextMatch", "Ranges"] {
            want.push(format!("export.split_{s}"));
            want.push(format!("export.split_arg_{s}"));
        }
        for m in ["add", "redo", "text"] {
            want.push(format!("ocr.mode_{m}"));
            want.push(format!("ocr.mode_{m}_hint"));
        }
        for s in ["none", "queued", "running", "done", "skipped", "failed"] {
            want.push(format!("ocr.state_{s}"));
        }
        for v in ["system", "light", "dark"] {
            want.push(format!("settings.theme_{v}"));
        }
        for v in ["Fast", "Accurate"] {
            want.push(format!("settings.quality_{v}"));
        }
        for v in ["Manual", "Auto", "Off"] {
            want.push(format!("settings.ocr_{v}"));
        }
        for k in ["import", "export", "ocr"] {
            want.push(format!("job.{k}"));
        }
        for c in [
            "invalid_arg",
            "not_found",
            "path_rejected",
            "limit_exceeded",
            "parse",
            "needs_password",
            "unsupported",
            "io",
            "worker_crashed",
            "timeout",
            "cancelled",
            "engine_missing",
            "internal",
        ] {
            want.push(format!("error.{c}"));
        }
        let missing: Vec<_> = want.into_iter().filter(|k| !en.contains_key(k)).collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn plurals_and_placeholders() {
        let ru = I18n::new("ru");
        assert_eq!(ru.tn("tray.count", 1, &[]), "1 страница");
        assert_eq!(ru.tn("tray.count", 3, &[]), "3 страницы");
        assert_eq!(ru.tn("tray.count", 11, &[]), "11 страниц");
        assert_eq!(ru.tn("tray.count", 21, &[]), "21 страница");
        assert_eq!(ru.tv("tray.page", &[("n", "4")]), "Страница 4");
        let en = I18n::new("en");
        assert_eq!(en.tn("tray.count", 1, &[]), "1 page");
        assert_eq!(en.tn("tray.count", 2, &[]), "2 pages");
        assert_eq!(en.t("no.such.key"), "no.such.key");
        assert_eq!(
            en.tv("preview.page_of", &[("n", "2")]),
            "Page 2 of {total}",
            "a missing variable stays visible"
        );
        assert_eq!(I18n::new("xx").code(), "en");
    }
}
