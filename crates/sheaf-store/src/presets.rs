//! Preset files: one `NAME.json` per preset in `<config dir>/presets/`
//! (PRESET-FORMAT). The folder belongs to the person, so every file read from
//! it is untrusted input: size-capped, parsed strictly, validated. A file
//! that does not pass is skipped in the list and refused on load, never
//! partly used. Presets never hold passwords (SEC-SECRETS).

use crate::{sanitize_file_name, write_atomic};
use sheaf_core::{AppError, ErrorCode, Preset, Result};
use std::path::{Path, PathBuf};

/// Largest preset file read, in bytes.
pub const MAX_PRESET_BYTES: u64 = 64 * 1024;
/// Most presets kept.
pub const MAX_PRESETS: usize = 100;

/// The folder presets live in, under the config dir.
pub fn dir(config: &Path) -> PathBuf {
    config.join("presets")
}

fn file_for(config: &Path, name: &str) -> PathBuf {
    dir(config).join(format!("{}.json", sanitize_file_name(name)))
}

fn read(path: &Path) -> Result<Preset> {
    let meta = std::fs::metadata(path).map_err(|e| AppError::io("read preset", &e))?;
    if !meta.is_file() || meta.len() > MAX_PRESET_BYTES {
        return Err(AppError::new(ErrorCode::ParseError, "preset file is not usable"));
    }
    let bytes = std::fs::read(path).map_err(|e| AppError::io("read preset", &e))?;
    let mut preset: Preset =
        serde_json::from_slice(&bytes).map_err(|_| AppError::new(ErrorCode::ParseError, "preset file is not valid"))?;
    // The file name is the preset's name: renaming the file renames the preset.
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        preset.name = stem.to_string();
    }
    preset.validate()?;
    Ok(preset)
}

/// Names of the presets that can be used, sorted.
pub fn list(config: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir(config)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| read(p).is_ok())
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.truncate(MAX_PRESETS);
    names
}

pub fn load(config: &Path, name: &str) -> Result<Preset> {
    let path = file_for(config, name);
    if !path.is_file() {
        return Err(AppError::new(ErrorCode::NotFound, "no such preset"));
    }
    read(&path)
}

/// Save (replacing a preset of the same name). Returns it as stored: the name
/// is made safe to use as a file name.
pub fn save(config: &Path, preset: &Preset) -> Result<Preset> {
    let mut preset = preset.clone();
    preset.validate()?;
    preset.name = sanitize_file_name(preset.name.trim());
    let path = file_for(config, &preset.name);
    if !path.exists() && list(config).len() >= MAX_PRESETS {
        return Err(AppError::limit("too many presets"));
    }
    std::fs::create_dir_all(dir(config)).map_err(|e| AppError::io("create presets folder", &e))?;
    let json = serde_json::to_vec_pretty(&preset).map_err(|e| AppError::internal(format!("serialize preset: {e}")))?;
    write_atomic(&path, &json)?;
    Ok(preset)
}

pub fn delete(config: &Path, name: &str) -> Result<()> {
    match std::fs::remove_file(file_for(config, name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::new(ErrorCode::NotFound, "no such preset")),
        Err(e) => Err(AppError::io("delete preset", &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sheaf-presets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    fn preset(name: &str) -> Preset {
        Preset {
            name: name.into(),
            compress: Some("Recommended".into()),
            ..Preset::default()
        }
    }

    #[test]
    fn save_list_load_and_delete() {
        let cfg = scratch("crud");
        assert!(list(&cfg).is_empty());
        save(&cfg, &preset("Contract scan")).expect("save");
        save(&cfg, &preset("archive")).expect("save");
        assert_eq!(
            list(&cfg),
            ["archive", "Contract scan"],
            "sorted without regard to case"
        );
        let loaded = load(&cfg, "Contract scan").expect("load");
        assert_eq!(loaded.compress.as_deref(), Some("Recommended"));
        // Saving again replaces.
        let mut changed = preset("archive");
        changed.compress = Some("Extreme".into());
        save(&cfg, &changed).expect("replace");
        assert_eq!(
            load(&cfg, "archive").expect("load").compress.as_deref(),
            Some("Extreme")
        );
        assert_eq!(list(&cfg).len(), 2);
        delete(&cfg, "archive").expect("delete");
        assert_eq!(list(&cfg), ["Contract scan"]);
        assert_eq!(delete(&cfg, "archive").expect_err("gone").code, ErrorCode::NotFound);
        assert_eq!(load(&cfg, "archive").expect_err("gone").code, ErrorCode::NotFound);
        let _ = std::fs::remove_dir_all(cfg);
    }

    #[test]
    fn a_hostile_name_stays_inside_the_folder() {
        let cfg = scratch("names");
        let stored = save(&cfg, &preset("../../evil")).expect("save");
        assert!(!stored.name.contains(['/', '\\']), "{}", stored.name);
        assert!(!cfg.join("evil.json").exists());
        assert_eq!(std::fs::read_dir(dir(&cfg)).expect("dir").count(), 1);
        assert!(load(&cfg, "../../evil").is_ok());
        let _ = std::fs::remove_dir_all(cfg);
    }

    #[test]
    fn damaged_and_oversized_files_are_skipped_not_used() {
        let cfg = scratch("bad");
        save(&cfg, &preset("good")).expect("save");
        let d = dir(&cfg);
        std::fs::write(d.join("broken.json"), b"{ not json").expect("write");
        std::fs::write(d.join("odd.json"), br#"{"name":"odd","ocr":{"mode":"Sometimes"}}"#).expect("write");
        std::fs::write(d.join("huge.json"), vec![b' '; (MAX_PRESET_BYTES + 1) as usize]).expect("write");
        std::fs::write(d.join("notes.txt"), b"hello").expect("write");
        assert_eq!(list(&cfg), ["good"]);
        for name in ["broken", "odd", "huge"] {
            assert!(load(&cfg, name).is_err(), "{name}");
        }
        let _ = std::fs::remove_dir_all(cfg);
    }

    #[test]
    fn the_file_name_is_the_name_and_invalid_presets_are_not_saved() {
        let cfg = scratch("rename");
        save(&cfg, &preset("one")).expect("save");
        std::fs::rename(file_for(&cfg, "one"), file_for(&cfg, "two")).expect("rename");
        assert_eq!(load(&cfg, "two").expect("load").name, "two");
        let mut bad = preset("three");
        bad.ocr.mode = "Maybe".into();
        assert_eq!(save(&cfg, &bad).expect_err("invalid").code, ErrorCode::InvalidArg);
        assert_eq!(list(&cfg), ["two"]);
        let _ = std::fs::remove_dir_all(cfg);
    }
}
