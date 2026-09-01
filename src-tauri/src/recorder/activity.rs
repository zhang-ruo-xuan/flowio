//! Active window title detection via Windows FFI.
//! Used during recording to generate natural-language step titles.

extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max_count: i32) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut i32) -> i32;  // (left, top, right, bottom)
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

/// Bounding rectangle of the currently focused foreground window.
///
/// Returns `(left, top, width, height)` in virtual-screen coordinates.
/// Returns `None` when there is no foreground window or the call fails.
pub fn get_active_window_rect() -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut rect = [0i32; 4]; // left, top, right, bottom
        if GetWindowRect(hwnd, rect.as_mut_ptr()) == 0 {
            return None;
        }
        let (l, t, r, b) = (rect[0], rect[1], rect[2], rect[3]);
        let w = r - l;
        let h = b - t;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((l, t, w, h))
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

#[cfg(test)]
mod activity_tests {
    use super::*;

    #[test]
    fn click_title_with_title() {
        assert_eq!(
            make_click_title(Some("Chrome - 百度")),
            "在「Chrome - 百度」中点击"
        );
    }

    #[test]
    fn click_title_empty_title_falls_back() {
        assert_eq!(make_click_title(Some("")), "点击");
    }

    #[test]
    fn click_title_none_falls_back() {
        assert_eq!(make_click_title(None), "点击");
    }
}
