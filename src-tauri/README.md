# src-tauri (MOD-SHELL)

Tauri 2 shell for Windows (ADR-TAURI). It translates between the window toolkit and `sheaf-app`
(MOD-APP) and holds no application logic of its own.

| File | Role |
|---|---|
| `src/shell.rs` | startup, window (navigation lock, incognito), drag-drop, events, shutdown |
| `src/ipc.rs` | the DOC-IPC commands; dialogs run here so paths never come from the page |
| `src/thumb.rs` | the `sheaf-thumb://` scheme (CMD-THUMB) |
| `build.rs` + `capabilities/main.json` | the commands the page may call, and nothing else (SEC-IPC) |
| `tauri.conf.json` | CSP, embedded `ui/`, no windows declared (the window is built in code) |

Windows only: elsewhere Tauri needs the GTK and WebKit toolchain, which this project does not use
(NG-XPLAT). CI builds it on Windows; `cargo check --target x86_64-pc-windows-msvc -p sheaf-shell`
type-checks it from Linux.
