//! Core recording pipeline helpers:
//! - Self-filtering (skip Flowio's own windows)
//! - Foreground window title detection (Windows FFI)
//!
//! The actual event-processing loop lives in `commands.rs` and uses
//! these helpers together with `screenshot.rs`.

extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max_count: i32) -> i32;
}

/// Return `true` when the window title contains "flowio" or "录步".
/// This is used to skip self-capture so the recording tool never
/// appears in its own screenshots.
pub fn is_flowio_app(window_title: &str) -> bool {
    let lower = window_title.to_lowercase();
    lower.contains("flowio") || lower.contains("录步")
}

/// Retrieve the caption of the currently focused foreground window.
///
/// Uses raw Win32 FFI (`GetForegroundWindow` + `GetWindowTextW`) and
/// therefore only works on Windows.
pub fn get_foreground_window_title() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
        let actual = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if actual <= 0 {
            return None;
        }
        buf.truncate(actual as usize);
        Some(String::from_utf16_lossy(&buf))
    }
}
