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
/// Format: "点击在「{title}」中" (操作在前、环境后置), fallback: "点击".
pub fn make_click_title(window_title: Option<&str>) -> String {
    make_click_title_with_app(window_title, None)
}

/// [`make_click_title`] 的可注入版本：窗口标题为空（开始菜单、任务管理器
/// 等无标题系统 UI）时，回退为前台应用的显示名（如「Windows 开始菜单」）。
pub fn make_click_title_with_app(window_title: Option<&str>, app: Option<&str>) -> String {
    match window_title {
        Some(title) if !title.is_empty() => {
            format!("点击在「{}」中", shorten_window_label(title, TITLE_LABEL_MAX_CHARS))
        }
        _ => match app.filter(|a| !a.is_empty()) {
            Some(a) => format!("点击在「{}」中", a),
            None => "点击".to_string(),
        },
    }
}

/// 领域感知的点击步骤标题：浏览器域自动剥离「 - 浏览器」后缀只保留网页名，
/// 其余领域沿用窗口标题原样。格式：操作（点击）在前、环境（点击在「…」中）在后。
pub fn make_click_title_for_domain(window_title: Option<&str>, domain: AppDomain) -> String {
    match window_title {
        Some(title) if !title.is_empty() => {
            let display = match domain {
                AppDomain::Browser => browser_page_name(title),
                _ => title.to_string(),
            };
            format!("点击在「{}」中", shorten_window_label(&display, TITLE_LABEL_MAX_CHARS))
        }
        _ => "点击".to_string(),
    }
}

// ── 窗口标题缩短（步骤标题防撑爆）─────────────────────────────

/// 窗口标题片段在步骤标题里的最大字符数，超过截断加「…」。
/// 用户诉求：像「常用求和函数 SUM函数.xlsx - WPS Office」这种长串只显示 10 个字。
pub const TITLE_LABEL_MAX_CHARS: usize = 10;

/// 已知应用后缀白名单（小写比较）：窗口标题形如「文档名 - 应用名」时剥掉应用段。
/// 注意：**不含**裸 chrome/edge/firefox —— 浏览器窗口标题是「网页名 - 浏览器名」，
/// 剥掉浏览器名由 `browser_page_name` 在浏览器域处理，这里误剥反而丢网页名。
const KNOWN_APP_SUFFIXES: &[&str] = &[
    "wps office",
    "microsoft excel",
    "microsoft word",
    "microsoft powerpoint",
    "microsoft edge",
    "google chrome",
    "internet explorer",
    "mozilla firefox",
    "excel",
    "word",
    "powerpoint",
    "记事本",
];

/// 文档扩展名（小写比较）：「报表.xlsx - WPS Office」先剥应用后缀再剥扩展名。
const DOC_EXTENSIONS: &[&str] = &[
    ".xlsx", ".xls", ".xlsm", ".docx", ".doc", ".pptx", ".ppt", ".pdf",
    ".txt", ".csv", ".md", ".rtf", ".wps", ".et", ".dps",
];

/// 把窗口标题缩短成适合放进步骤标题的短标签：
/// 1) 反复剥「已知应用后缀」（` - WPS Office`）和「文档扩展名」（`.xlsx`）直到稳定；
/// 2) 仍超过 `max` 个字符 → 截断加「…」（按字符数，中文英文一视同仁）。
///
/// 例：`常用求和函数 SUM函数.xlsx - WPS Office` → `常用求和函数 SUM…`
pub fn shorten_window_label(raw: &str, max: usize) -> String {
    let mut s = raw.trim().to_string();
    loop {
        let before = s.clone();
        let lower = s.to_lowercase();
        for app in KNOWN_APP_SUFFIXES {
            let suf = format!(" - {}", app);
            if lower.ends_with(&suf) {
                // 后缀必为 ASCII，UTF-8 边界必然安全
                s = s[..s.len() - suf.len()].trim_end().to_string();
                break;
            }
        }
        if s == before {
            for ext in DOC_EXTENSIONS {
                if lower.ends_with(ext) && s.chars().count() > 1 {
                    s = s[..s.len() - ext.len()].trim_end().to_string();
                    break;
                }
            }
        }
        if s == before {
            break;
        }
    }
    truncate_label(&s, max)
}

/// 纯截断：超过 `max` 个字符 → 前 `max` 字符 +「…」（按字符数，中英文一视同仁）。
/// 不剥任何后缀——给 AI 生成 / AI 精简等非窗口标题来源的兜底用。
pub fn truncate_label(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() > max {
        let head: String = s.chars().take(max).collect();
        format!("{}\u{2026}", head.trim_end())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;

    #[test]
    fn click_title_with_title() {
        // 「Chrome - 百度」11 字符超上限 → 截断加省略号
        assert_eq!(
            make_click_title(Some("Chrome - 百度")),
            "点击在「Chrome - 百…」中"
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
            "点击在「Windows 开始菜单」中"
        );
        // 应用名也为空时仍回退「点击」
        assert_eq!(make_click_title_with_app(None, Some("")), "点击");
        assert_eq!(make_click_title_with_app(None, None), "点击");
        // 有窗口标题时优先用标题
        assert_eq!(
            make_click_title_with_app(Some("微信"), Some("QQ")),
            "点击在「微信」中"
        );
    }

    #[test]
    fn click_title_for_domain_browser_strips_suffix() {
        use crate::ai::domain::AppDomain;
        assert_eq!(
            make_click_title_for_domain(Some("百度 - Google Chrome"), AppDomain::Browser),
            "点击在「百度」中"
        );
        assert_eq!(
            make_click_title_for_domain(Some("GitHub - Microsoft Edge"), AppDomain::Browser),
            "点击在「GitHub」中"
        );
    }

    #[test]
    fn click_title_for_domain_non_browser_keeps_title() {
        use crate::ai::domain::AppDomain;
        // 真实 Excel 窗口标题是「工作簿1 - Excel」（文档在前、应用在后）→ 剥掉「 - Excel」
        assert_eq!(
            make_click_title_for_domain(Some("工作簿1 - Excel"), AppDomain::Spreadsheet),
            "点击在「工作簿1」中"
        );
        // 应用在前的非真实格式剥不掉 → 走 10 字截断兜底
        assert_eq!(
            make_click_title_for_domain(Some("Excel - 工作簿1"), AppDomain::Spreadsheet),
            "点击在「Excel - 工作…」中"
        );
        assert_eq!(
            make_click_title_for_domain(Some("记事本"), AppDomain::Other),
            "点击在「记事本」中"
        );
        assert_eq!(
            make_click_title_for_domain(None, AppDomain::Browser),
            "点击"
        );
    }

    #[test]
    fn shorten_label_strips_office_suffix_and_extension() {
        // 用户实测案例：WPS 长窗口标题 → 剥「 - WPS Office」+「.xlsx」→ 截 10 字
        assert_eq!(
            shorten_window_label("常用求和函数 SUM函数.xlsx - WPS Office", TITLE_LABEL_MAX_CHARS),
            "常用求和函数 SUM…"
        );
        // 只剥扩展名（后缀不是已知应用时保留）
        assert_eq!(shorten_window_label("总结报告.docx", 20), "总结报告");
        // 连剥两轮：后缀剥完剩扩展名
        assert_eq!(shorten_window_label("报表.xlsx - Excel", 20), "报表");
    }

    #[test]
    fn shorten_label_keeps_short_and_never_truncates_twice() {
        assert_eq!(shorten_window_label("微信", 10), "微信");
        assert_eq!(shorten_window_label("", 10), "");
        // 恰好 10 字不截
        assert_eq!(shorten_window_label("一二三四五六七八九十", 10), "一二三四五六七八九十");
        // 11 字 → 10 字 + …
        assert_eq!(shorten_window_label("一二三四五六七八九十A", 10), "一二三四五六七八九十…");
        // 不剥非白名单后缀（浏览器网页名交给 browser_page_name）
        assert_eq!(shorten_window_label("百度 - 某网站", 20), "百度 - 某网站");
    }
}
