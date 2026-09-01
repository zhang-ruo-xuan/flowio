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
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
    fn QueryFullProcessImageNameW(
        h_process: isize,
        dw_flags: u32,
        lp_exe_name: *mut u16,
        lpdw_size: *mut u32,
    ) -> i32;
    fn CloseHandle(h_object: isize) -> i32;
}

/// Return `true` when the window title contains "flowio" or "录步".
/// Kept for tests/legacy; the runtime self-filter uses
/// [`foreground_is_flowio`] (process-name based) instead.
pub fn is_flowio_app(window_title: &str) -> bool {
    let lower = window_title.to_lowercase();
    lower.contains("flowio") || lower.contains("录步")
}

/// `PROCESS_QUERY_LIMITED_INFORMATION` (0x1000) is enough to read the image
/// name and works on un-elevated processes.
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

/// Return the image file name (e.g. `"chrome.exe"`) of the foreground
/// window's process, or `None` when there is no foreground window or the
/// query fails.
pub fn get_foreground_process_name() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(proc);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or("").to_string())
    }
}

/// Return `true` when the foreground window belongs to the Flowio process
/// itself (`flowio.exe`), based on the process image name rather than the
/// window title.
///
/// This avoids self-filter false positives when the user records a tutorial
/// about "录步" itself (browser pages / other apps whose titles contain
/// "录步" or "flowio" would otherwise be skipped).
pub fn foreground_is_flowio() -> bool {
    get_foreground_process_name()
        .map(|name| name.eq_ignore_ascii_case("flowio.exe"))
        .unwrap_or(false)
}

/// 已知应用进程名 → 中文显示名映射表。
///
/// 录制开始时根据前台进程识别目标应用（含浏览器、常用桌面软件与系统
/// 进程），识别结果自动填充录制卡片的「应用」字段；窗口标题为空的
/// 系统 UI（开始菜单/任务管理器/输入面板等）也能得到可读名称。
/// 未识别的进程返回 `None`（前端回退为「网页应用」）。
pub fn app_display_name(process_name: &str) -> Option<&'static str> {
    let name = process_name.to_ascii_lowercase();
    match name.as_str() {
        // ── 工具 / 媒体 / 压缩 ──
        "dict.exe" => Some("有道词典"),
        "cloudmusic.exe" => Some("网易云音乐"),
        "qqmusic.exe" => Some("QQ 音乐"),
        "everything.exe" => Some("Everything 文件搜索"),
        "obs64.exe" => Some("OBS 录屏/直播"),
        "wegame.exe" => Some("WeGame 游戏平台"),
        "steam.exe" => Some("Steam 游戏平台"),
        "spotify.exe" => Some("Spotify 音乐"),
        "vlc.exe" => Some("VLC 播放器"),
        "potplayermini64.exe" => Some("PotPlayer 播放器"),
        "bandizip.exe" => Some("Bandizip 压缩"),
        "7zfm.exe" => Some("7-Zip 压缩"),
        "winrar.exe" => Some("WinRAR 压缩"),
        // ── Windows 自带应用 ──
        "calc.exe" | "calculator.exe" => Some("计算器"),
        "mspaint.exe" => Some("画图"),
        "notepad++.exe" => Some("Notepad++"),
        "notepad.exe" => Some("记事本"),
        "bash.exe" | "mintty.exe" => Some("Git Bash"),
        "powershell.exe" | "pwsh.exe" => Some("PowerShell"),
        "cmd.exe" => Some("命令提示符"),
        "windowsterminal.exe" => Some("Windows 终端"),
        "regedit.exe" => Some("注册表编辑器"),
        // ── 开发工具 ──
        "xshell.exe" => Some("Xshell（SSH 终端）"),
        "studio64.exe" => Some("Android Studio"),
        "clion64.exe" => Some("CLion"),
        "goland64.exe" => Some("GoLand"),
        "webstorm64.exe" => Some("WebStorm"),
        "pycharm64.exe" => Some("PyCharm"),
        "idea64.exe" => Some("IntelliJ IDEA"),
        "devenv.exe" => Some("Visual Studio"),
        "code.exe" => Some("VS Code 编辑器"),
        // ── 通讯 / 会议 ──
        "zoom.exe" => Some("Zoom 会议"),
        "tencentmeeting.exe" | "wemeetapp.exe" => Some("腾讯会议"),
        "discord.exe" => Some("Discord"),
        "telegram.exe" => Some("Telegram"),
        "lark.exe" => Some("飞书"),
        "dingtalk.exe" => Some("钉钉"),
        "tim.exe" => Some("TIM（轻聊版 QQ）"),
        "qq.exe" => Some("QQ"),
        "wxwork.exe" => Some("企业微信"),
        "weixin.exe" | "wechat.exe" => Some("微信"),
        "teams.exe" | "ms-teams.exe" => Some("Microsoft Teams"),
        // ── Office ──
        "onenote.exe" => Some("Microsoft OneNote"),
        "outlook.exe" => Some("Microsoft Outlook"),
        "powerpnt.exe" => Some("Microsoft PowerPoint"),
        "excel.exe" => Some("Microsoft Excel"),
        "winword.exe" => Some("Microsoft Word"),
        "wpscloudsvr.exe" => Some("WPS 云服务"),
        "wpspdf.exe" => Some("WPS PDF"),
        "et.exe" => Some("WPS 表格（Excel）"),
        "wpp.exe" => Some("WPS 演示（PPT）"),
        "wps.exe" => Some("WPS 文字（文档）"),
        // ── 浏览器 ──
        "sogouexplorer.exe" => Some("搜狗浏览器"),
        "qqbrowser.exe" => Some("QQ 浏览器"),
        "360se.exe" | "360chrome.exe" => Some("360 安全浏览器"),
        "firefox.exe" => Some("Firefox 浏览器"),
        "msedge.exe" => Some("Edge 浏览器"),
        "chrome.exe" => Some("Chrome 浏览器"),
        // ── 系统进程 / 系统 UI ──
        "svchost.exe" | "runtimebroker.exe" => Some("系统进程（应用权限后台）"),
        "dwm.exe" => Some("桌面窗口管理器（系统进程）"),
        "startmenuexperiencehost.exe" => Some("Windows 开始菜单"),
        "shellexperiencehost.exe" => Some("Windows 开始菜单/通知中心"),
        "taskmgr.exe" => Some("任务管理器"),
        "searchhost.exe" => Some("Windows 搜索"),
        "applicationframehost.exe" => Some("系统应用容器（设置/照片等自带应用）"),
        "textinputhost.exe" => Some("Windows 输入面板（触摸键盘/语音输入）"),
        "explorer.exe" => Some("文件资源管理器（桌面/文件夹窗口）"),
        _ => None,
    }
}

/// 前台窗口所属应用的中文显示名。
///
/// Flowio 自身返回 `None`（不给录制起应用名）；未识别的进程也返回
/// `None`，由前端回退为「网页应用」。
pub fn get_foreground_app_display_name() -> Option<String> {
    let proc = get_foreground_process_name()?;
    if proc.eq_ignore_ascii_case("flowio.exe") {
        return None;
    }
    app_display_name(&proc).map(|s| s.to_string())
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

#[cfg(test)]
mod recorder_tests {
    use super::*;

    #[test]
    fn is_flowio_detects_variants() {
        assert!(is_flowio_app("Flowio - 录步"));
        assert!(is_flowio_app("flowio 编辑器"));
        assert!(is_flowio_app("录步 - 录制中"));
        assert!(is_flowio_app("FLOWIO"));
    }

    #[test]
    fn is_flowio_rejects_others() {
        assert!(!is_flowio_app("微信"));
        assert!(!is_flowio_app("Google Chrome"));
        assert!(!is_flowio_app("录屏助手")); // 含"录"但不含"录步"
    }

    #[test]
    fn is_flowio_empty_false() {
        assert!(!is_flowio_app(""));
    }
}
