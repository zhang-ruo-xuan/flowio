//! Active window title detection via Windows FFI.
//! Used during recording to generate natural-language step titles.

use crate::ai::domain::{browser_page_name, AppDomain};

extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max_count: i32) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut i32) -> i32;  // (left, top, right, bottom)
    fn GetClientRect(hwnd: isize, rect: *mut i32) -> i32;  // (left, top, right, bottom)
    fn ClientToScreen(hwnd: isize, point: *mut i32) -> i32; // (x, y) 原地改写
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

/// 前台窗口**客户区**在屏幕上的矩形 `(x, y, width, height)`，物理像素。
///
/// ## 与 `get_active_window_rect()` 的区别（关键）
/// `GetWindowRect` 含 DWM 的**隐形缩放边框**：最大化窗口会返回比屏幕大一圈的
/// 矩形（每边约 7px @96dpi），底部还会多垫一个任务栏高度 —— 拿它当坐标锚点
/// 会整体偏移。客户区恰好等于窗口的**视觉边界**，也就是浏览器
/// `window.outerWidth / outerHeight` 的语义，是把扩展上报的"视口相对坐标"
/// 换算成屏幕坐标时唯一可靠的锚点。
///
/// 对 Chrome/Edge 这类**自绘标题栏**的窗口，客户区还包含标签栏与地址栏，
/// 因此 `客户区高 - 视口高 = 浏览器 chrome 高度`，正是垂直换算需要的量。
///
/// 前提是进程为 DPI 感知（tao 启动时会设 `PER_MONITOR_AWARE_V2`），
/// 否则拿到的是被系统虚拟化的 96-dpi 坐标。
pub fn get_active_client_rect() -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        // 客户坐标恒为 (0, 0, w, h)
        let mut rect = [0i32; 4];
        if GetClientRect(hwnd, rect.as_mut_ptr()) == 0 {
            return None;
        }
        // 客户区原点 (0,0) → 屏幕坐标
        let mut pt = [0i32; 2];
        if ClientToScreen(hwnd, pt.as_mut_ptr()) == 0 {
            return None;
        }
        let w = rect[2] - rect[0];
        let h = rect[3] - rect[1];
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((pt[0], pt[1], w, h))
    }
}

/// Generate a natural-language step title from a window title.
/// Format: "在「{title}」中点击", fallback: "点击".
pub fn make_click_title(window_title: Option<&str>) -> String {
    make_click_title_with_app(window_title, None)
}

/// [`make_click_title`] 的可注入版本：窗口标题为空（开始菜单、任务管理器
/// 等无标题系统 UI）时，回退为前台应用的显示名（如「Windows 开始菜单」）。
pub fn make_click_title_with_app(window_title: Option<&str>, app: Option<&str>) -> String {
    match window_title {
        Some(title) if !title.is_empty() => format!("在「{}」中点击", title),
        _ => match app.filter(|a| !a.is_empty()) {
            Some(a) => format!("在「{}」中点击", a),
            None => "点击".to_string(),
        },
    }
}

/// 领域感知的点击步骤标题：浏览器域自动剥离「 - 浏览器」后缀只保留网页名，
/// 其余领域沿用窗口标题原样。
pub fn make_click_title_for_domain(window_title: Option<&str>, domain: AppDomain) -> String {
    match window_title {
        Some(title) if !title.is_empty() => {
            let display = match domain {
                AppDomain::Browser => browser_page_name(title),
                _ => title.to_string(),
            };
            format!("在「{}」中点击", display)
        }
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

    #[test]
    fn click_title_empty_title_falls_back_to_app() {
        assert_eq!(
            make_click_title_with_app(Some(""), Some("Windows 开始菜单")),
            "在「Windows 开始菜单」中点击"
        );
        // 应用名也为空时仍回退「点击」
        assert_eq!(make_click_title_with_app(None, Some("")), "点击");
        assert_eq!(make_click_title_with_app(None, None), "点击");
        // 有窗口标题时优先用标题
        assert_eq!(
            make_click_title_with_app(Some("微信"), Some("QQ")),
            "在「微信」中点击"
        );
    }

    #[test]
    fn click_title_for_domain_browser_strips_suffix() {
        use crate::ai::domain::AppDomain;
        assert_eq!(
            make_click_title_for_domain(Some("百度 - Google Chrome"), AppDomain::Browser),
            "在「百度」中点击"
        );
        assert_eq!(
            make_click_title_for_domain(Some("GitHub - Microsoft Edge"), AppDomain::Browser),
            "在「GitHub」中点击"
        );
    }

    #[test]
    fn click_title_for_domain_non_browser_keeps_title() {
        use crate::ai::domain::AppDomain;
        assert_eq!(
            make_click_title_for_domain(Some("Excel - 工作簿1"), AppDomain::Spreadsheet),
            "在「Excel - 工作簿1」中点击"
        );
        assert_eq!(
            make_click_title_for_domain(Some("记事本"), AppDomain::Other),
            "在「记事本」中点击"
        );
        assert_eq!(
            make_click_title_for_domain(None, AppDomain::Browser),
            "点击"
        );
    }
}
