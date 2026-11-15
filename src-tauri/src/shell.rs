//! Startup, window, events, shutdown (FLOW-LIFECYCLE).

use crate::ipc::{self, IpcError};
use sheaf_app::api::{Backend, BackendConfig, Emit, Event};
use sheaf_app::Engines;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{DragDropEvent, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Managed state. A failed start is kept, not fatal: the window opens and the
/// UI shows what went wrong (FLOW-ERROR) instead of the app vanishing.
pub struct Shell {
    backend: Result<Arc<Backend>, IpcError>,
}

impl Shell {
    pub fn backend(&self) -> Result<Arc<Backend>, IpcError> {
        self.backend.clone()
    }
}

/// Events go to the window as `sheaf:<name>` (DOC-IPC).
struct TauriEmit(tauri::AppHandle);

impl Emit for TauriEmit {
    fn emit(&self, event: Event) {
        // A window that is closing has nobody to tell; nothing to do about it.
        let _ = self.0.emit(&format!("sheaf:{}", event.name()), event.payload());
    }
}

/// The only places the window may ever show: the embedded UI. `tauri://localhost`
/// on macOS and Linux builds, `http(s)://tauri.localhost` on Windows (SEC-NONET).
fn is_app_origin(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost"),
        _ => false,
    }
}

fn start_backend(app: &tauri::App) -> Result<Arc<Backend>, IpcError> {
    let roots: Vec<PathBuf> = app.path().resource_dir().into_iter().collect();
    let engines = Engines::discover(&roots)?;
    let base_dir = sheaf_store::default_base_dir()?;
    let backend = Backend::start(
        BackendConfig {
            engines,
            base_dir,
            config_dir: sheaf_store::config_dir().ok(),
            verify_vendor: true,
        },
        Arc::new(TauriEmit(app.handle().clone())),
    )?;
    Ok(backend)
}

/// CLI-GRAMMAR, the part PH-4 needs: `sheaf open <files...>`. Arguments are
/// untrusted like any other path (SEC-PATHS); the path policy runs on import.
fn files_from_args() -> Vec<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        Some(first) if first == "open" => args.map(PathBuf::from).collect(),
        _ => Vec::new(),
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let backend = start_backend(app);
    app.manage(Shell { backend });

    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title(sheaf_core::APP_NAME)
        .inner_size(1280.0, 820.0)
        .min_inner_size(900.0, 560.0)
        // Nothing the page shows is kept in a browser profile on disk (SEC-TEMP).
        .incognito(true)
        // The window can only ever show the embedded UI (SEC-NONET).
        .on_navigation(is_app_origin)
        .build()?;

    // After the window exists, so it is on screen before anything is imported (FLOW-LIFECYCLE).
    let files = files_from_args();
    if !files.is_empty() {
        let state = app.state::<Shell>();
        if let Ok(backend) = state.backend() {
            std::thread::spawn(move || {
                if let Err(e) = backend.import(files) {
                    backend.report(&e);
                }
            });
        }
    }
    Ok(())
}

/// Files dropped on the window arrive here, from the toolkit, not from the
/// page: the UI never supplies a path (SEC-PATHS).
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::DragDrop(drag) = event else { return };
    let app = window.app_handle();
    match drag {
        DragDropEvent::Enter { .. } => {
            let _ = app.emit("sheaf:drop-hover", true);
        }
        DragDropEvent::Leave => {
            let _ = app.emit("sheaf:drop-hover", false);
        }
        DragDropEvent::Drop { paths, .. } => {
            let _ = app.emit("sheaf:drop-hover", false);
            let paths = paths.clone();
            if let Ok(backend) = app.state::<Shell>().backend() {
                std::thread::spawn(move || {
                    if let Err(e) = backend.import(paths) {
                        backend.report(&e);
                    }
                });
            }
        }
        _ => {}
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("sheaf-thumb", crate::thumb::handle)
        .invoke_handler(ipc::handler())
        .on_window_event(on_window_event)
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("the window toolkit failed to start");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // FLOW-LIFECYCLE: cancel jobs, kill workers and tesseract, delete the session dir.
            if let Some(shell) = handle.try_state::<Shell>() {
                if let Ok(backend) = shell.backend() {
                    backend.shutdown();
                }
            }
        }
    });
}
