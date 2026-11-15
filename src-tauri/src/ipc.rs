//! DOC-IPC commands. Each is a thin call into `Backend`, run off the UI
//! thread (workers can take seconds). Paths never arrive from the page: they
//! come out of the dialogs run here (SEC-PATHS).

use crate::shell::Shell;
use serde::Serialize;
use serde_json::Value;
use sheaf_app::api::{
    ExportSpecDto, HitDto, ImportReportDto, OcrPolicyDto, OcrResultDto, PagesDto, PickDto, SettingsDto, SettingsPatch,
    SnapshotDto, UiOp, UnlockDto,
};
use sheaf_app::jobs::JobView;
use sheaf_core::{AppError, ErrorCode};
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

/// What the page receives when a command fails (EVT-ERROR shape).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    pub key: &'static str,
    pub msg: String,
}

impl From<AppError> for IpcError {
    fn from(e: AppError) -> Self {
        IpcError {
            code: e.code,
            key: e.code.i18n_key(),
            msg: e.msg,
        }
    }
}

type Reply<T> = Result<T, IpcError>;

/// Run blocking work on the runtime's blocking pool, never on the UI thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, AppError> + Send + 'static) -> Reply<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| IpcError::from(AppError::internal("a command stopped unexpectedly")))?
        .map_err(IpcError::from)
}

const DOCUMENTS: &[&str] = &["pdf", "jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

#[tauri::command]
pub async fn snapshot(shell: State<'_, Shell>) -> Reply<SnapshotDto> {
    let b = shell.backend()?;
    blocking(move || b.snapshot()).await
}

#[tauri::command]
pub async fn pages(shell: State<'_, Shell>, start: usize, count: usize) -> Reply<PagesDto> {
    let b = shell.backend()?;
    blocking(move || b.pages(start, count)).await
}

#[tauri::command]
pub async fn apply(shell: State<'_, Shell>, op: UiOp) -> Reply<u64> {
    let b = shell.backend()?;
    blocking(move || b.apply(op)).await
}

#[tauri::command]
pub async fn undo(shell: State<'_, Shell>, redo: bool) -> Reply<Option<u64>> {
    let b = shell.backend()?;
    blocking(move || b.undo(redo)).await
}

/// CMD-RENDER: the key the page appends to `sheaf-thumb://` to load a preview.
#[tauri::command]
pub async fn render(shell: State<'_, Shell>, page_id: u64, zoom: f32) -> Reply<String> {
    let b = shell.backend()?;
    blocking(move || b.render_key(page_id, zoom)).await
}

/// CMD-IMPORT from the Open dialog.
#[tauri::command]
pub async fn open_files(app: AppHandle, shell: State<'_, Shell>) -> Reply<Vec<ImportReportDto>> {
    let b = shell.backend()?;
    blocking(move || {
        let picked = app
            .dialog()
            .file()
            .add_filter("PDF, images", DOCUMENTS)
            .blocking_pick_files();
        let paths: Vec<PathBuf> = picked
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .collect();
        b.import(paths)
    })
    .await
}

#[tauri::command]
pub async fn unlock_pw(shell: State<'_, Shell>, source: u32, password: String) -> Reply<UnlockDto> {
    let b = shell.backend()?;
    blocking(move || b.unlock(source, password)).await
}

#[tauri::command]
pub async fn ocr_run(shell: State<'_, Shell>, pages: Option<Vec<u64>>, policy: OcrPolicyDto) -> Reply<u64> {
    let b = shell.backend()?;
    blocking(move || b.ocr_run(pages, policy)).await
}

#[tauri::command]
pub async fn ocr_result(shell: State<'_, Shell>, page_id: u64) -> Reply<OcrResultDto> {
    let b = shell.backend()?;
    blocking(move || b.ocr_result(page_id)).await
}

#[tauri::command]
pub async fn search(shell: State<'_, Shell>, query: String) -> Reply<Vec<HitDto>> {
    let b = shell.backend()?;
    blocking(move || b.search(&query)).await
}

#[tauri::command]
pub async fn tool_run(shell: State<'_, Shell>, tool_id: String, params: Value, pages: Option<Vec<u64>>) -> Reply<u64> {
    let b = shell.backend()?;
    blocking(move || b.tool_run(&tool_id, params, pages)).await
}

#[tauri::command]
pub async fn export(shell: State<'_, Shell>, spec: ExportSpecDto) -> Reply<u64> {
    let b = shell.backend()?;
    blocking(move || b.export(spec)).await
}

#[tauri::command]
pub async fn job_cancel(shell: State<'_, Shell>, job_id: u64) -> Reply<()> {
    let b = shell.backend()?;
    blocking(move || {
        b.job_cancel(job_id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn jobs(shell: State<'_, Shell>) -> Reply<Vec<JobView>> {
    let b = shell.backend()?;
    blocking(move || Ok(b.jobs())).await
}

/// CMD-REVEAL: show a file this app produced in Explorer.
#[tauri::command]
pub async fn reveal(shell: State<'_, Shell>, job_id: u64, index: usize) -> Reply<()> {
    let b = shell.backend()?;
    blocking(move || reveal_in_explorer(&b.reveal_path(job_id, index)?)).await
}

#[cfg(windows)]
fn reveal_in_explorer(path: &std::path::Path) -> Result<(), AppError> {
    use std::os::windows::process::CommandExt;
    // Explorer wants `/select,"<path>"` as one argument with its own quotes.
    // The path is one this app produced, and a Windows path cannot contain a quote.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::io("start Explorer", &e))
}

#[cfg(not(windows))]
fn reveal_in_explorer(_path: &std::path::Path) -> Result<(), AppError> {
    Err(AppError::new(ErrorCode::Unsupported, "Reveal needs Windows Explorer"))
}

#[tauri::command]
pub async fn settings(shell: State<'_, Shell>) -> Reply<SettingsDto> {
    let b = shell.backend()?;
    blocking(move || Ok(b.settings())).await
}

#[tauri::command]
pub async fn set_settings(shell: State<'_, Shell>, patch: SettingsPatch) -> Reply<SettingsDto> {
    let b = shell.backend()?;
    blocking(move || b.set_settings(patch)).await
}

/// The output folder is picked in a dialog here; the page cannot type a path.
#[tauri::command]
pub async fn choose_output_dir(app: AppHandle, shell: State<'_, Shell>) -> Reply<SettingsDto> {
    let b = shell.backend()?;
    blocking(move || match app.dialog().file().blocking_pick_folder() {
        Some(dir) => {
            let dir = dir
                .into_path()
                .map_err(|_| AppError::new(ErrorCode::PathRejected, "not a local folder"))?;
            b.set_output_dir(Some(&dir))
        }
        None => Ok(b.settings()),
    })
    .await
}

#[tauri::command]
pub async fn clear_output_dir(shell: State<'_, Shell>) -> Reply<SettingsDto> {
    let b = shell.backend()?;
    blocking(move || b.set_output_dir(None)).await
}

/// A picture for an image watermark. The page gets a token, not a path.
#[tauri::command]
pub async fn pick_image(app: AppHandle, shell: State<'_, Shell>) -> Reply<Option<PickDto>> {
    let b = shell.backend()?;
    blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("PNG, JPEG", &["png", "jpg", "jpeg"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file
            .into_path()
            .map_err(|_| AppError::new(ErrorCode::PathRejected, "not a local file"))?;
        b.register_pick(&path).map(Some)
    })
    .await
}

// PH-5 and PH-6 commands: part of the surface now so the capability list does
// not change later; they answer `Unsupported` until the phases land.

#[tauri::command]
pub async fn scan_devices(shell: State<'_, Shell>) -> Reply<Vec<Value>> {
    Err(shell.backend()?.scan_unavailable().into())
}

#[tauri::command]
pub async fn scan_start(shell: State<'_, Shell>, profile: Value, policy: Value) -> Reply<u64> {
    let _ = (profile, policy);
    Err(shell.backend()?.scan_unavailable().into())
}

#[tauri::command]
pub async fn scan_cancel(shell: State<'_, Shell>, job_id: u64) -> Reply<()> {
    let _ = job_id;
    Err(shell.backend()?.scan_unavailable().into())
}

#[tauri::command]
pub async fn presets(shell: State<'_, Shell>) -> Reply<Vec<String>> {
    Ok(shell.backend()?.presets_list())
}

#[tauri::command]
pub async fn preset_save(shell: State<'_, Shell>, preset: Value) -> Reply<()> {
    let _ = preset;
    Err(shell.backend()?.presets_unavailable().into())
}

#[tauri::command]
pub async fn preset_delete(shell: State<'_, Shell>, name: String) -> Reply<()> {
    let _ = name;
    Err(shell.backend()?.presets_unavailable().into())
}

#[tauri::command]
pub async fn preset_run(shell: State<'_, Shell>, name: String, pages: Option<Vec<u64>>) -> Reply<u64> {
    let _ = (name, pages);
    Err(shell.backend()?.presets_unavailable().into())
}

/// The whole command surface, registered once.
pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        snapshot,
        pages,
        apply,
        undo,
        render,
        open_files,
        unlock_pw,
        ocr_run,
        ocr_result,
        search,
        tool_run,
        export,
        job_cancel,
        jobs,
        reveal,
        settings,
        set_settings,
        choose_output_dir,
        clear_output_dir,
        pick_image,
        scan_devices,
        scan_start,
        scan_cancel,
        presets,
        preset_save,
        preset_delete,
        preset_run,
    ]
}
