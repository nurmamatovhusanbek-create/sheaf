//! The resident helper (`sheaf tray`): a tray icon and the wheel's global
//! hotkey (ADR-WHEEL). It draws nothing itself. When the hotkey is pressed it
//! asks the shell which files are selected in the window in front, writes them
//! to a private list and starts the wheel overlay (`sheaf wheel`) at the cursor.
//! Idle, it sleeps in the Windows message loop and uses no CPU.
//!
//! Written without a Windows machine to run it on (RISK-WIN-SHELL): it
//! type-checks and has not yet been tried.

#![allow(unsafe_code)]

use crate::selection;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use sheaf_store::Settings;
use sheaf_ui::i18n::I18n;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::str::FromStr;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows::core::PWSTR;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, LPARAM, POINT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, GetCursorPos, GetMessageW, PostThreadMessageW, TranslateMessage,
    ASFW_ANY, MSG, WM_APP,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Sheaf");

/// Sent to the helper's message loop by [`stop_helper`]: leave, taking the tray icon with you.
const WM_STOP: u32 = WM_APP + 1;

/// Where the running helper says who it is: its process and message-loop thread, so the window that
/// started it can ask it to quit when the window closes.
fn id_file() -> Option<PathBuf> {
    sheaf_store::default_base_dir().ok().map(|dir| dir.join("tray.id"))
}

/// Removes the identity file when the helper ends, however it ends.
struct IdFile(Option<PathBuf>);

impl Drop for IdFile {
    fn drop(&mut self) {
        if let Some(file) = self.0.take() {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// A path as Windows reports it for a process, comparable with `current_exe` (which may carry a `\\?\` prefix).
fn plain(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_lowercase()
}

/// Closing the window closes the helper too (a window that leaves a program running in the
/// background keeps its own file locked and is not what closing means), unless the person turned on
/// "Start with Windows": that is a request for the helper to stay.
pub fn stop_helper_unless_resident() {
    if !autostart_enabled() {
        stop_helper();
    }
}

/// Ask the running helper (if there is one, and it really is this program) to quit, and end it if it
/// has not after two seconds.
pub fn stop_helper() {
    let Some(file) = id_file() else { return };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let mut numbers = text.split_whitespace().map(str::parse::<u32>);
    let (Some(Ok(pid)), Some(Ok(thread))) = (numbers.next(), numbers.next()) else {
        let _ = std::fs::remove_file(&file);
        return;
    };
    if pid == std::process::id() {
        return;
    }
    let Ok(me) = std::env::current_exe() else { return };
    // SAFETY: the process handle is opened and closed here; the buffer is large enough for the
    // length it is told and the length is read back; no pointer outlives the calls.
    unsafe {
        let Ok(process) = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
            false,
            pid,
        ) else {
            // Not running any more: a stale file.
            let _ = std::fs::remove_file(&file);
            return;
        };
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let named =
            QueryFullProcessImageNameW(process, PROCESS_NAME_FORMAT(0), PWSTR(buffer.as_mut_ptr()), &mut length)
                .is_ok();
        // Only a copy of this very program: a recycled process number must never be ended by mistake.
        let is_helper =
            named && plain(&String::from_utf16_lossy(&buffer[..length as usize])) == plain(&me.display().to_string());
        if is_helper {
            let _ = PostThreadMessageW(thread, WM_STOP, WPARAM(0), LPARAM(0));
            if WaitForSingleObject(process, 2000) != WAIT_OBJECT_0 {
                let _ = TerminateProcess(process, 0);
                let _ = WaitForSingleObject(process, 1000);
            }
        }
        let _ = CloseHandle(process);
    }
    let _ = std::fs::remove_file(file);
}

/// `"C:\...\sheaf.exe" tray`, as written to the Run key.
fn run_command() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|exe| format!("\"{}\" tray", exe.display()))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Is the helper set to start with Windows?
fn autostart_enabled() -> bool {
    // SAFETY: the key handle is opened here and closed before returning; the
    // value buffer is only read for its presence.
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        )
        .is_err()
        {
            return false;
        }
        let present = RegQueryValueExW(key, RUN_VALUE, None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        present
    }
}

fn set_autostart(on: bool) {
    // SAFETY: as above; the data slice outlives the call.
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .is_err()
        {
            return;
        }
        if on {
            if let Some(cmd) = run_command() {
                let data: Vec<u8> = wide(&cmd).iter().flat_map(|u| u.to_le_bytes()).collect();
                let _ = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(&data));
            }
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

/// The app's icon (the same wheel as the window's), drawn in code so no image file has to ship.
fn icon() -> Icon {
    Icon::from_rgba(sheaf_ui::app_icon_rgba(32), 32, 32)
        .unwrap_or_else(|_| Icon::from_rgba(vec![0x1f, 0x5f, 0xd6, 255], 1, 1).expect("1x1 icon"))
}

/// `ctrl+alt+space` → `Ctrl+Alt+Space`, for display.
fn pretty(keys: &str) -> String {
    keys.split('+')
        .map(|k| {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn spawn_main(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).args(args).spawn();
    }
}

/// Collect the selection and the cursor, hand them to a new wheel overlay.
fn show_wheel(child: &mut Option<Child>) {
    if let Some(c) = child {
        if matches!(c.try_wait(), Ok(None)) {
            return; // a wheel is already up
        }
    }
    let files = selection::selected_paths();
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out-pointer for the duration of the call.
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return;
    }
    // The overlay places itself in screen points; the cursor is in pixels.
    // SAFETY: no arguments, no pointers.
    let scale = (unsafe { GetDpiForSystem() } as f32 / 96.0).max(1.0);
    let at = format!(
        "{},{}",
        (p.x as f32 / scale).round() as i32,
        (p.y as f32 / scale).round() as i32
    );
    let base = sheaf_store::default_base_dir().unwrap_or_else(|_| std::env::temp_dir().join("Sheaf"));
    let _ = std::fs::create_dir_all(&base);
    let list: PathBuf = base.join(format!(
        "wheel-{}-{:x}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let text: String = files.iter().map(|f| format!("{}\n", f.display())).collect();
    if std::fs::write(&list, text).is_err() {
        return;
    }
    // The overlay should come to the front and take the keyboard: this process just
    // received the hotkey, so it may pass that right on.
    // SAFETY: no pointers.
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    if let Ok(exe) = std::env::current_exe() {
        *child = Command::new(exe)
            .arg("wheel")
            .arg("--at")
            .arg(at)
            .arg("--files")
            .arg(&list)
            .spawn()
            .ok();
    }
}

/// (Re)register the wheel hotkey; the previous one is released first. `None` when the
/// setting is empty, unreadable, or the combination is taken by another program.
fn register(manager: &GlobalHotKeyManager, keys: &str, current: &mut Option<HotKey>) -> Option<u32> {
    if let Some(old) = current.take() {
        let _ = manager.unregister(old);
    }
    let hotkey = if keys.is_empty() {
        None
    } else {
        HotKey::from_str(keys).ok()
    }?;
    manager.register(hotkey).ok()?;
    *current = Some(hotkey);
    Some(hotkey.id())
}

/// Poll the settings file's modification time and post `WM_APP` to the helper's loop when it changes.
fn watch_settings(loop_thread: u32) {
    let Ok(dir) = sheaf_store::config_dir() else { return };
    let file = dir.join("settings.json");
    std::thread::spawn(move || {
        let stamp = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut last = stamp(&file);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let now = stamp(&file);
            if now != last {
                last = now;
                // SAFETY: posting a plain thread message; a loop that has ended just ignores it.
                unsafe {
                    let _ = PostThreadMessageW(loop_thread, WM_APP, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

pub fn run() -> Result<(), String> {
    // One helper per user session.
    // SAFETY: a named mutex held for the life of the process; the handle is
    // intentionally never closed.
    unsafe {
        let _ = CreateMutexW(None, true, w!("Local\\SheafTrayHelper"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
    }
    selection::init_com();

    let settings = sheaf_store::config_dir()
        .map(|d| Settings::load(&d))
        .unwrap_or_default();
    let t = I18n::new(&settings.language);

    let manager = GlobalHotKeyManager::new().map_err(|e| format!("hotkeys: {e}"))?;
    let mut registered: Option<HotKey> = None;
    let mut hotkey_id = register(&manager, &settings.wheel_hotkey, &mut registered);
    let shown_keys = |on: bool, keys: &str| if on { pretty(keys) } else { "-".to_string() };
    let shown = shown_keys(hotkey_id.is_some(), &settings.wheel_hotkey);

    let menu = Menu::new();
    let open = MenuItem::new(t.t("tray.open"), true, None);
    let info = MenuItem::new(t.tv("tray.wheel", &[("keys", &shown)]), false, None);
    let autostart = CheckMenuItem::new(t.t("tray.autostart"), true, autostart_enabled(), None);
    let quit = MenuItem::new(t.t("tray.quit"), true, None);
    for item in [
        &open as &dyn tray_icon::menu::IsMenuItem,
        &info,
        &PredefinedMenuItem::separator(),
        &autostart,
        &PredefinedMenuItem::separator(),
        &quit,
    ] {
        menu.append(item).map_err(|e| format!("menu: {e}"))?;
    }
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(t.tv("tray.tip", &[("keys", &shown)]))
        .with_icon(icon())
        .build()
        .map_err(|e| format!("tray icon: {e}"))?;

    // Settings are changed in the window; a small thread notices the file change and
    // wakes this loop, so a new hotkey applies without restarting the helper.
    // SAFETY: no arguments, no pointers.
    let loop_thread = unsafe { GetCurrentThreadId() };
    watch_settings(loop_thread);
    // Say who this is, so the window that started it can end it when that window closes.
    let _identity = IdFile(id_file().inspect(|file| {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, format!("{} {loop_thread}", std::process::id()));
    }));

    let mut child: Option<Child> = None;
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid MSG; the loop ends on WM_QUIT or error.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break;
        }
        // SAFETY: `msg` was filled in by GetMessageW just above.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if msg.message == WM_STOP {
            return Ok(()); // dropping `tray` takes the icon off the screen
        }
        if msg.message == WM_APP {
            let settings = sheaf_store::config_dir()
                .map(|d| Settings::load(&d))
                .unwrap_or_default();
            hotkey_id = register(&manager, &settings.wheel_hotkey, &mut registered);
            let shown = shown_keys(hotkey_id.is_some(), &settings.wheel_hotkey);
            info.set_text(t.tv("tray.wheel", &[("keys", &shown)]));
            let _ = tray.set_tooltip(Some(t.tv("tray.tip", &[("keys", &shown)])));
        }
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if Some(ev.id) == hotkey_id && ev.state == HotKeyState::Pressed {
                show_wheel(&mut child);
            }
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == *open.id() {
                spawn_main(&[]);
            } else if ev.id == *autostart.id() {
                set_autostart(autostart.is_checked());
            } else if ev.id == *quit.id() {
                return Ok(());
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                spawn_main(&[]);
            }
        }
    }
    Ok(())
}
