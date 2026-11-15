//! Where the pointer is, in the screen points the overlay places itself in.
//! Only the card for an Explorer verb asks: it appears where the person just clicked.
#![allow(unsafe_code)]

#[cfg(windows)]
pub fn position() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out-pointer for the duration of the call.
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return (400, 300);
    }
    // The overlay places itself in screen points; the cursor is in pixels.
    // SAFETY: no arguments, no pointers.
    let scale = (unsafe { GetDpiForSystem() } as f32 / 96.0).max(1.0);
    ((p.x as f32 / scale).round() as i32, (p.y as f32 / scale).round() as i32)
}

#[cfg(not(windows))]
pub fn position() -> (i32, i32) {
    (600, 400)
}
