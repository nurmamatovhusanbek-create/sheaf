//! Explorer's right-click menu (ADR-EXPLORER-VERBS, FLOW-EXPLORER): one
//! "Sheaf" entry with a submenu of the actions that make sense for the file
//! type, the saved presets, and "Open in Sheaf". Written under
//! `HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.<ext>\shell\Sheaf`:
//! no administrator rights, nothing outside the user's own hive, removed
//! completely by `uninstall`.
//!
//! Explorer starts one process per selected file for entries like these; each
//! command carries `--collect`, so the first process gathers the others'
//! files and acts once ([`crate::aggregate`]), and `--card`, so the work shows
//! its progress and result in a small card where the person clicked. On
//! Windows 11 the entry is under "Show more options".
//!
//! [`plan`] is plain data, tested everywhere; the registry code is Windows only.

#![cfg_attr(not(windows), allow(dead_code))]

use sheaf_core::{FileKind, QuickAction};
use sheaf_ui::i18n::I18n;
use std::path::Path;

pub const ROOT: &str = r"Software\Classes\SystemFileAssociations";
pub const PDF: &[&str] = &["pdf"];
pub const IMAGES: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];
/// Most presets listed in the menu: a menu longer than this is not a menu.
pub const MAX_PRESET_ITEMS: usize = 12;

/// One registry key and its string values; a value named "" is the key's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub values: Vec<(String, String)>,
}

fn entry(key: String, values: &[(&str, String)]) -> Entry {
    Entry {
        key,
        values: values.iter().map(|(n, v)| (n.to_string(), v.clone())).collect(),
    }
}

/// A preset name that can sit inside a command line without being misread: the shell
/// expands `%` words and quotes end the argument.
fn safe_in_command(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c.is_control() || c == '%' || c == '"')
}

/// Every key to write. `sheaf` is `sheaf.exe`: the program with no console window, so nothing flashes.
pub fn plan(sheaf: &Path, lang: &str, presets: &[String]) -> Vec<Entry> {
    let t = I18n::new(lang);
    let exe = sheaf.display().to_string();
    let command = |args: &str| format!("\"{exe}\" {args} --collect \"%1\"");
    let mut out = Vec::new();
    for (exts, kind) in [(PDF, FileKind::Pdf), (IMAGES, FileKind::Image)] {
        // What applies to a selection of this kind (merge needs two or more; a menu may be used on many).
        let actions: Vec<QuickAction> = QuickAction::ALL
            .into_iter()
            .filter(|a| a.applies_to(&[kind, kind]))
            .collect();
        for ext in exts {
            let root = format!(r"{ROOT}\.{ext}\shell\Sheaf");
            out.push(entry(
                root.clone(),
                &[
                    ("MUIVerb", "Sheaf".into()),
                    ("SubCommands", String::new()),
                    ("Icon", format!("\"{exe}\",0")),
                ],
            ));
            let mut item = |order: &str, id: &str, label: String, args: String| {
                let key = format!(r"{root}\shell\{order}-{id}");
                out.push(entry(key.clone(), &[("MUIVerb", label)]));
                out.push(entry(format!(r"{key}\command"), &[("", command(&args))]));
            };
            item("10", "open", t.t("wheel.open"), "open".into());
            for (i, action) in actions.iter().enumerate() {
                item(
                    &format!("{:02}", 20 + i),
                    action.id(),
                    t.t(&format!("tool.{}.name", action.id())),
                    format!("{} --card", action.id()),
                );
            }
            for (i, name) in presets
                .iter()
                .filter(|n| safe_in_command(n))
                .take(MAX_PRESET_ITEMS)
                .enumerate()
            {
                item(
                    &format!("{:02}", 50 + i),
                    &format!("preset{i}"),
                    t.tv("wheel.preset", &[("name", name)]),
                    format!("preset \"{name}\" --card"),
                );
            }
        }
    }
    out
}

/// The keys `uninstall` removes: the Sheaf entry of every type this writes.
pub fn roots() -> Vec<String> {
    PDF.iter()
        .chain(IMAGES)
        .map(|ext| format!(r"{ROOT}\.{ext}\shell\Sheaf"))
        .collect()
}

#[cfg(windows)]
mod registry {
    #![allow(unsafe_code)]

    use super::{plan, roots};
    use sheaf_core::{AppError, ErrorCode, Result};
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn failed(what: &str) -> AppError {
        AppError::new(ErrorCode::Io, format!("could not {what} in the registry"))
    }

    /// Remove every Sheaf entry from the right-click menu.
    pub fn uninstall() -> Result<()> {
        for root in roots() {
            let name = wide(&root);
            // SAFETY: `name` is a NUL-terminated buffer that outlives the call.
            let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())) };
            if status.is_err() && status != ERROR_FILE_NOT_FOUND {
                return Err(failed("remove the menu"));
            }
        }
        Ok(())
    }

    /// Write the menu afresh (so entries for deleted presets go).
    pub fn install(sheaf: &Path, lang: &str, presets: &[String]) -> Result<()> {
        uninstall()?;
        for entry in plan(sheaf, lang, presets) {
            let key_name = wide(&entry.key);
            let mut key = HKEY::default();
            // SAFETY: the name is NUL-terminated and outlives the call; `key` is a valid out-pointer.
            let status = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key_name.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    None,
                    &mut key,
                    None,
                )
            };
            if status.is_err() {
                return Err(failed("write the menu"));
            }
            for (name, value) in &entry.values {
                let name_w = wide(name);
                let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
                // The default value has no name: a null pointer.
                let name_ptr = if name.is_empty() {
                    PCWSTR::null()
                } else {
                    PCWSTR(name_w.as_ptr())
                };
                // SAFETY: `key` is open; the name and data buffers outlive the call.
                let status = unsafe { RegSetValueExW(key, name_ptr, None, REG_SZ, Some(&data)) };
                if status.is_err() {
                    // SAFETY: `key` was opened above.
                    let _ = unsafe { RegCloseKey(key) };
                    return Err(failed("write the menu"));
                }
            }
            // SAFETY: `key` was opened above.
            let _ = unsafe { RegCloseKey(key) };
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use registry::{install, uninstall};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn plan_for(presets: &[&str]) -> BTreeMap<String, Vec<(String, String)>> {
        let presets: Vec<String> = presets.iter().map(|s| s.to_string()).collect();
        plan(Path::new(r"C:\Program Files\Sheaf\sheaf.exe"), "en", &presets)
            .into_iter()
            .map(|e| (e.key, e.values))
            .collect()
    }

    fn command_of(map: &BTreeMap<String, Vec<(String, String)>>, key: &str) -> String {
        map.get(&format!(r"{key}\command"))
            .and_then(|v| v.first())
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| panic!("no command under {key}"))
    }

    #[test]
    fn a_pdf_gets_every_action_and_an_image_only_the_ones_that_fit() {
        let map = plan_for(&[]);
        let pdf = format!(r"{ROOT}\.pdf\shell\Sheaf\shell");
        let png = format!(r"{ROOT}\.png\shell\Sheaf\shell");
        let has = |base: &str, id: &str| {
            map.keys()
                .any(|k| k.starts_with(&format!(r"{base}\")) && k.ends_with(&format!("-{id}")))
        };
        for id in ["open", "merge", "compress", "ocr", "split", "pdf2img", "rotate"] {
            assert!(has(&pdf, id), "pdf: {id}");
        }
        assert!(!has(&pdf, "img2pdf"), "pictures to PDF is not for a PDF");
        for id in ["open", "merge", "ocr", "img2pdf", "rotate"] {
            assert!(has(&png, id), "png: {id}");
        }
        for id in ["compress", "split", "pdf2img"] {
            assert!(!has(&png, id), "png: {id}");
        }
        // The same menu for every picture type.
        assert!(map.contains_key(&format!(r"{ROOT}\.jpeg\shell\Sheaf")));
        assert!(map.contains_key(&format!(r"{ROOT}\.webp\shell\Sheaf")));
    }

    #[test]
    fn commands_collect_the_selection_and_show_a_card_and_the_menu_is_one_cascade() {
        let map = plan_for(&[]);
        let root = format!(r"{ROOT}\.pdf\shell\Sheaf");
        let top = &map[&root];
        assert!(top.contains(&("MUIVerb".into(), "Sheaf".into())));
        assert!(top.contains(&("SubCommands".into(), String::new())), "a cascade");
        let compress = command_of(&map, &format!(r"{root}\shell\21-compress"));
        assert_eq!(
            compress,
            r#""C:\Program Files\Sheaf\sheaf.exe" compress --card --collect "%1""#
        );
        assert_eq!(
            command_of(&map, &format!(r"{root}\shell\10-open")),
            r#""C:\Program Files\Sheaf\sheaf.exe" open --collect "%1""#
        );
        // What it writes parses back into the commands it means.
        let args: Vec<std::ffi::OsString> = ["compress", "--card", "--collect", "a.pdf"]
            .iter()
            .map(Into::into)
            .collect();
        assert!(matches!(sheaf_cli::parse(&args), Ok(sheaf_cli::Command::Collected(_))));
    }

    #[test]
    fn presets_get_entries_but_a_name_the_shell_could_misread_does_not() {
        let map = plan_for(&["Contract scan", "100% sure", "say \"hi\""]);
        let root = format!(r"{ROOT}\.pdf\shell\Sheaf");
        assert_eq!(
            command_of(&map, &format!(r"{root}\shell\50-preset0")),
            r#""C:\Program Files\Sheaf\sheaf.exe" preset "Contract scan" --card --collect "%1""#
        );
        assert_eq!(
            map[&format!(r"{root}\shell\50-preset0")],
            [("MUIVerb".to_string(), "Preset: Contract scan".to_string())]
        );
        assert!(
            !map.contains_key(&format!(r"{root}\shell\51-preset1")),
            "a % would be expanded by the shell"
        );
        let many: Vec<String> = (0..30).map(|i| format!("p{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let n = plan_for(&many)
            .keys()
            .filter(|k| k.contains(r".pdf\shell\Sheaf\shell\") && k.ends_with("command") && k.contains("-preset"))
            .count();
        assert_eq!(n, MAX_PRESET_ITEMS);
    }

    #[test]
    fn everything_written_is_under_the_sheaf_entry_and_removed_with_it() {
        let map = plan_for(&["x"]);
        let roots = roots();
        assert_eq!(roots.len(), PDF.len() + IMAGES.len());
        for key in map.keys() {
            assert!(key.starts_with(ROOT), "{key}");
            assert!(
                roots.iter().any(|r| key == r || key.starts_with(&format!(r"{r}\"))),
                "{key} would be left behind"
            );
        }
    }

    #[test]
    fn the_labels_follow_the_language() {
        let ru = plan(Path::new(r"C:\s.exe"), "ru", &[]);
        let merge = ru.iter().find(|e| e.key.ends_with("-merge")).expect("merge entry");
        assert_eq!(merge.values[0].1, I18n::new("ru").t("tool.merge.name"));
        assert_ne!(merge.values[0].1, I18n::new("en").t("tool.merge.name"));
    }
}
