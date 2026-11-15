//! SEC-IPC: the UI may call exactly the commands of DOC-IPC and nothing else.
//! Listing them here makes tauri-build generate an `allow-<command>` permission
//! for each; `capabilities/main.json` grants those and no others.

const COMMANDS: &[&str] = &[
    "snapshot",
    "pages",
    "apply",
    "undo",
    "render",
    "open_files",
    "unlock_pw",
    "ocr_run",
    "ocr_result",
    "search",
    "tool_run",
    "export",
    "job_cancel",
    "jobs",
    "reveal",
    "settings",
    "set_settings",
    "choose_output_dir",
    "clear_output_dir",
    "pick_image",
    "scan_devices",
    "scan_start",
    "scan_cancel",
    "presets",
    "preset_save",
    "preset_delete",
    "preset_run",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build configuration");
}
