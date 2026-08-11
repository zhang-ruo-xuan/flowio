//! Active window title detection via Windows FFI.
//! Used during recording to generate natural-language step titles.

extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max_count: i32) -> i32;
}

/// Retrieve the caption of the currently focused foreground window.
///
/// Uses raw Win32 FFI and therefore only works on Windows.
pub fn get_active_window_title() -> Option<String> {
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

/// Generate a natural-language step title from a window title.
/// Format: "在「{title}」中点击", fallback: "点击".
pub fn make_click_title(window_title: Option<&str>) -> String {
    match window_title {
        Some(title) if !title.is_empty() => format!("在「{}」中点击", title),
        _ => "点击".to_string(),
    }
}
