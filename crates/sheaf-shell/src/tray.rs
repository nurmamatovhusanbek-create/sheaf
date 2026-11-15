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
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, POINT};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, GetCursorPos, GetMessageW, TranslateMessage, ASFW_ANY, MSG,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Sheaf");

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

/// A 32 by 32 wheel, drawn here so no image file has to ship.
fn icon() -> Icon {
    let n = 32usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let r = (dx * dx + dy * dy).sqrt();
            if !(8.0..=15.0).contains(&r) {
                continue;
            }
            let angle = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
            let sector = (angle / (std::f32::consts::TAU / 8.0)) as usize;
            // A thin gap between sectors.
            let within = (angle % (std::f32::consts::TAU / 8.0)) * r;
            if within < 1.0 {
                continue;
            }
            let shade = if sector.is_multiple_of(2) {
                [0x1f, 0x5f, 0xd6]
            } else {
                [0x4a, 0x7d, 0xe8]
            };
            let i = (y * n + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[shade[0], shade[1], shade[2], 255]);
        }
    }
    Icon::from_rgba(rgba, n as u32, n as u32)
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
    let keys = if settings.wheel_hotkey.is_empty() {
        None
    } else {
        HotKey::from_str(&settings.wheel_hotkey).ok()
    };
    let hotkey_id = keys.and_then(|k| manager.register(k).ok().map(|()| k.id()));
    let shown_keys = if hotkey_id.is_some() {
        pretty(&settings.wheel_hotkey)
    } else {
        "-".into()
    };

    let menu = Menu::new();
    let open = MenuItem::new(t.t("tray.open"), true, None);
    let info = MenuItem::new(t.tv("tray.wheel", &[("keys", &shown_keys)]), false, None);
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
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(t.tv("tray.tip", &[("keys", &shown_keys)]))
        .with_icon(icon())
        .build()
        .map_err(|e| format!("tray icon: {e}"))?;

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
