//! The files selected in the foreground Explorer window, or on the desktop
//! (ADR-WHEEL). Asked of the shell through its COM interfaces: `IShellWindows`
//! lists the open Explorer windows, and the active view of the one in front
//! answers "what is selected" (the same route Explorer's own extensions use).
//!
//! COM wrapper module: the only `unsafe` in the shell (DOC-AGENT-RULES rule 5).
//! Every block states what it relies on. Written without a Windows machine to
//! run it on (RISK-WIN-SHELL): it type-checks and has not yet been tried.

#![allow(unsafe_code)]

use std::path::PathBuf;
use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IDispatch, IServiceProvider, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView2, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp, SID_STopLevelBrowser, ShellWindows,
    CSIDL_DESKTOP, SIGDN_FILESYSPATH, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, IsWindowVisible};

/// Prepare the calling thread for COM. Call once on the thread that will call [`selected_paths`].
pub fn init_com() {
    // SAFETY: plain COM initialisation of the calling thread; a repeat call or an
    // already-initialised thread returns a benign HRESULT that is ignored.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Window class name of `hwnd`, e.g. `CabinetWClass` (Explorer) or `Progman` (the desktop).
fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: `buf` is a valid, writable buffer of the length passed.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The paths of the items selected in the window in front: an Explorer window
/// (the active tab) or the desktop. Empty when something else has the focus.
pub fn selected_paths() -> Vec<PathBuf> {
    // SAFETY: COM calls on a thread initialised by `init_com`; every interface
    // pointer comes from a call that returned it and is released when dropped.
    unsafe {
        let front = GetForegroundWindow();
        if front.0.is_null() {
            return Vec::new();
        }
        let class = class_of(front);
        let Ok(windows): Result<IShellWindows, _> = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) else {
            return Vec::new();
        };
        if class == "Progman" || class == "WorkerW" {
            return desktop_selection(&windows).unwrap_or_default();
        }
        if class != "CabinetWClass" && class != "ExploreWClass" {
            return Vec::new();
        }
        let count = windows.Count().unwrap_or(0);
        for i in 0..count {
            let Ok(item) = windows.Item(&VARIANT::from(i)) else {
                continue;
            };
            let Ok(app) = item.cast::<IWebBrowserApp>() else {
                continue;
            };
            let Ok(handle) = app.HWND() else { continue };
            if handle.0 != front.0 as isize {
                continue;
            }
            // Explorer with tabs lists one entry per tab under the same window; only the
            // active tab's view is visible.
            if let Ok(paths) = selection_of(&item, true) {
                if !paths.is_empty() {
                    return paths;
                }
            }
        }
        Vec::new()
    }
}

/// SAFETY (callers): `windows` is a live `IShellWindows`.
unsafe fn desktop_selection(windows: &IShellWindows) -> windows::core::Result<Vec<PathBuf>> {
    let mut hwnd = 0i32;
    // SAFETY: `hwnd` outlives the call; the VARIANTs are plain values.
    let item = unsafe {
        windows.FindWindowSW(
            &VARIANT::from(CSIDL_DESKTOP as i32),
            &VARIANT::default(),
            SWC_DESKTOP,
            &mut hwnd,
            SWFO_NEEDDISPATCH,
        )?
    };
    selection_of(&item, false)
}

/// The selection of the shell view behind `dispatch`. With `only_visible`, a view
/// that is not on screen (a background tab) yields nothing.
fn selection_of(dispatch: &IDispatch, only_visible: bool) -> windows::core::Result<Vec<PathBuf>> {
    // SAFETY: the interfaces are obtained by QueryInterface/QueryService on live objects
    // and used only within this call.
    unsafe {
        let provider: IServiceProvider = dispatch.cast()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
        let view = browser.QueryActiveShellView()?;
        if only_visible {
            let window = view.GetWindow()?;
            if !IsWindowVisible(window).as_bool() {
                return Ok(Vec::new());
            }
        }
        let folder_view: IFolderView2 = view.cast()?;
        let items: IShellItemArray = folder_view.GetSelection(false)?;
        paths_of(&items)
    }
}

/// SAFETY (callers): `items` is a live `IShellItemArray`.
unsafe fn paths_of(items: &IShellItemArray) -> windows::core::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    // SAFETY: indices stay below the reported count; the returned string is owned by us
    // and freed with CoTaskMemFree once copied.
    unsafe {
        for i in 0..items.GetCount()? {
            let name: PWSTR = items.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            if !name.is_null() {
                out.push(PathBuf::from(name.to_string().unwrap_or_default()));
                CoTaskMemFree(Some(name.0 as *const _));
            }
        }
    }
    out.retain(|p| !p.as_os_str().is_empty());
    Ok(out)
}
