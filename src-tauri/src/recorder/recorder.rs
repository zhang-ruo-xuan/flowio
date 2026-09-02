//! Core recording pipeline helpers:
//! - Self-filtering (skip Flowio's own windows)
//! - Foreground window title detection (Windows FFI)
//! - 录制指定软件（enumerate / locate / PID-filter target app windows）
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
    fn EnumWindows(
        lp_enum_func: Option<unsafe extern "system" fn(isize, isize) -> i32>,
        l_param: isize,
    ) -> i32;
    fn IsWindowVisible(h_wnd: isize) -> i32;
    fn GetWindowRect(h_wnd: isize, lp_rect: *mut Rect) -> i32;
}

/// Win32 RECT (screen coordinates, pixels).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }
    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

/// A selectable top-level window (used by the "录制指定软件" feature).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TargetWindow {
    pub hwnd: u64,
    pub title: String,
    pub process_name: String,
    /// User-friendly Chinese description of the owning process
    /// (e.g. `wps.exe` -> "WPS 文字"), empty when unknown.
    #[serde(default)]
    pub description: String,
}

/// Map a process name to a plain-language Chinese description so ordinary
/// users can tell what each entry in the "录制指定软件" picker actually is.
/// Unknown processes get an empty string (frontend shows the process name
/// alone).
pub fn process_description(process_name: &str) -> String {
    let lower = process_name.to_ascii_lowercase();
    let desc = match lower.as_str() {
        // ── 系统/文件管理 ──
        "explorer.exe" => "文件资源管理器（桌面/文件夹窗口）",
        "textinputhost.exe" => "Windows 输入面板（触摸键盘/语音输入）",
        "applicationframehost.exe" => "系统应用容器（设置/照片等自带应用）",
        "searchapp.exe" | "searchhost.exe" => "Windows 搜索",
        "taskmgr.exe" => "任务管理器",
        "shellexperiencehost.exe" => "Windows 开始菜单/通知中心",
        "startmenuexperiencehost.exe" => "Windows 开始菜单",
        "dwm.exe" => "桌面窗口管理器（系统进程）",
        "runtimebroker.exe" => "系统进程（应用权限后台）",
        "registry" => "注册表编辑器",
        // ── 浏览器 ──
        "chrome.exe" => "Chrome 浏览器",
        "msedge.exe" => "Edge 浏览器",
        "firefox.exe" => "Firefox 浏览器",
        "360se.exe" | "360chrome.exe" => "360 安全浏览器",
        "qqbrowser.exe" => "QQ 浏览器",
        "sogouexplorer.exe" => "搜狗浏览器",
        // ── WPS / Office ──
        "wps.exe" => "WPS 文字（文档）",
        "wpp.exe" => "WPS 演示（PPT）",
        "et.exe" => "WPS 表格（Excel）",
        "wpspdf.exe" => "WPS PDF",
        "wpscloudsvr.exe" => "WPS 云服务",
        "winword.exe" => "Microsoft Word",
        "excel.exe" => "Microsoft Excel",
        "powerpnt.exe" => "Microsoft PowerPoint",
        "outlook.exe" => "Microsoft Outlook",
        "onenote.exe" => "Microsoft OneNote",
        "teams.exe" => "Microsoft Teams",
        // ── 即时通讯 ──
        "wechat.exe" | "weixin.exe" => "微信",
        "wechatwork.exe" | "wxwork.exe" => "企业微信",
        "qq.exe" => "QQ",
        "tim.exe" => "TIM（轻聊版 QQ）",
        "dingtalk.exe" | "dingding.exe" => "钉钉",
        "feishu.exe" | "lark.exe" => "飞书",
        "telegram.exe" => "Telegram",
        "discord.exe" => "Discord",
        // ── 会议 ──
        "wemeetapp.exe" | "tencentmeeting.exe" => "腾讯会议",
        "zoom.exe" => "Zoom 会议",
        // ── 开发者工具 ──
        "code.exe" => "VS Code 编辑器",
        "devenv.exe" => "Visual Studio",
        "idea64.exe" => "IntelliJ IDEA",
        "pycharm64.exe" => "PyCharm",
        "webstorm64.exe" => "WebStorm",
        "goland64.exe" => "GoLand",
        "clion64.exe" => "CLion",
        "studio64.exe" => "Android Studio",
        "xshell.exe" => "Xshell（SSH 终端）",
        "windowsTerminal.exe" | "terminal.exe" => "Windows 终端",
        "cmd.exe" => "命令提示符",
        "powershell.exe" | "pwsh.exe" => "PowerShell",
        "git-bash.exe" => "Git Bash",
        // ── 工具/影音/其他 ──
        "notepad.exe" => "记事本",
        "notepad++.exe" => "Notepad++",
        "mspaint.exe" => "画图",
        "calc.exe" => "计算器",
        "winrar.exe" => "WinRAR 压缩",
        "7zfm.exe" => "7-Zip 压缩",
        "bandizip.exe" => "Bandizip 压缩",
        "potplayermini64.exe" | "potplayer.exe" => "PotPlayer 播放器",
        "vlc.exe" => "VLC 播放器",
        "spotify.exe" => "Spotify 音乐",
        "steam.exe" => "Steam 游戏平台",
        "wegame.exe" => "WeGame 游戏平台",
        "obs64.exe" | "obs.exe" => "OBS 录屏/直播",
        "everything.exe" => "Everything 文件搜索",
        "qqmusic.exe" => "QQ 音乐",
        "cloudmusic.exe" => "网易云音乐",
        "youdao dict.exe" => "有道词典",
        _ => "",
    };
    desc.to_string()
}

/// Resolve the process image name (e.g. `wps.exe`) for a PID.
fn process_name_of_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if proc == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(proc) };
    if ok == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    Some(path.rsplit(['\\', '/']).next().unwrap_or("").to_string())
}

/// Enumerate visible top-level windows. Results are deduplicated by process
/// name (keeps the window with the longest title for each process), which
/// matches the "pick an app, not a window" UX of 录制指定软件.
pub fn list_target_windows() -> Vec<TargetWindow> {
    let mut windows: Vec<TargetWindow> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_windows_callback), &mut windows as *mut Vec<TargetWindow> as isize);
    }
    // Deduplicate by process_name (keep longest title).
    let mut by_proc: Vec<TargetWindow> = Vec::new();
    for w in windows {
        if let Some(existing) = by_proc.iter_mut().find(|e| {
            e.process_name.eq_ignore_ascii_case(&w.process_name)
        }) {
            if w.title.chars().count() > existing.title.chars().count() {
                *existing = w;
            }
        } else {
            by_proc.push(w);
        }
    }
    // Attach user-friendly Chinese descriptions for common processes.
    for w in by_proc.iter_mut() {
        w.description = process_description(&w.process_name);
    }
    by_proc
}

unsafe extern "system" fn enum_windows_callback(hwnd: isize, lparam: isize) -> i32 {
    // Only visible windows.
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return 1;
    }
    let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
    let actual = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if actual <= 0 {
        return 1;
    }
    let title = String::from_utf16_lossy(&buf[..actual as usize]);
    if title.trim().is_empty() {
        return 1;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let Some(process_name) = process_name_of_pid(pid) else {
        return 1;
    };
    // Skip ourselves (recording Flowio's own window is handled by 录制录步自身).
    if process_name.eq_ignore_ascii_case("flowio.exe") {
        return 1;
    }
    let windows = &mut *(lparam as *mut Vec<TargetWindow>);
    windows.push(TargetWindow {
        hwnd: hwnd as u64,
        title,
        process_name,
        description: String::new(), // filled in list_target_windows()
    });
    1
}

/// Find the screen rectangle of the largest visible top-level window owned by
/// the given process (e.g. `chrome.exe`).
///
/// Why "largest" instead of "first hit": some processes — most notably
/// Chromium-based browsers and PWAs — own many auxiliary windows besides the
/// main UI: taskbar previews, popup favicon chips, URL-bar suggestions. These
/// are individually visible, on the same process, and their rect is often
/// 32×32 (just a favicon). Returning the first match yields an icon-sized
/// crop. Collecting every candidate, dropping rects smaller than 200×100, and
/// picking the biggest by area reliably lands on the actual app window.
///
/// Returns `(left, top, right, bottom)` of the chosen rect, or `None` if no
/// top-level visible window with a non-empty title is large enough.
pub fn find_window_rect_by_process(process_name: &str) -> Option<Rect> {
    let mut ctx: (Vec<WindowCandidate>, &str) = (Vec::new(), process_name);
    unsafe {
        EnumWindows(
            Some(enum_windows_rect_collect_callback),
            (&mut ctx as *mut (Vec<WindowCandidate>, &str)) as isize,
        );
    }
    let candidates = ctx.0;
    // 200 px wide × 100 px tall = 20 000 px². Well below any realistic main UI,
    // well above auxiliary controls (favicon chips are 32×32, URL-bar
    // suggestions ~1×20 px). A tighter floor (100×60) was considered but some
    // legitimate dock-style apps are genuinely small.
    const MIN_W: i32 = 200;
    const MIN_H: i32 = 100;

    // Rank = (has_title, area). Tuple comparison gives us "prefer windows that
    // actually declare a title; among those, prefer the largest".
    //
    // `has_title` is deliberately a *soft* preference, not a filter: Chromium
    // main windows sometimes report an empty `GetWindowText` while their
    // auxiliary child windows do carry one. Hard-filtering on title would then
    // discard the real app window and every frame would be skipped (the user
    // sees no screenshots at all) — a worse failure than a slightly-off crop.
    let mut best: Option<(bool, i64, Rect)> = None;
    for c in candidates {
        let w = c.rect.width();
        let h = c.rect.height();
        if w < MIN_W || h < MIN_H {
            continue;
        }
        let area = (w as i64) * (h as i64);
        let better = match &best {
            None => true,
            Some((bh, ba, _)) => (c.has_title, area) > (*bh, *ba),
        };
        if better {
            best = Some((c.has_title, area, c.rect));
        }
    }
    if let Some((has_title, area, r)) = best {
        capture_dbg_log(&format!(
            "[CAPTURE] find_window_rect_by_process({}): chose {}x{} at ({},{}), area={}, has_title={}",
            process_name,
            r.width(),
            r.height(),
            r.left,
            r.top,
            area,
            has_title
        ));
        Some(r)
    } else {
        capture_dbg_log(&format!(
            "[CAPTURE] find_window_rect_by_process({}): no candidate passed the {}x{} floor",
            process_name, MIN_W, MIN_H
        ));
        None
    }
}

/// Append a one-line message to the Flowio debug log. Mirrors the inline
/// pattern used in `commands.rs` for `[BG] target-filter:` and similar — keeps
/// every cross-thread diagnostic on a single tail-this-file point that the
/// user (and we) can grep through.
///
/// Why not just `eprintln!`: Tauri release builds are GUI-subsystem
/// (no console), so stdout / stderr go nowhere. The debug log is the only
/// place a deployed build ever leaves behind.
pub(super) fn capture_dbg_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"D:\录步\flowio_debug.log")
    {
        let _ = writeln!(f, "{}", msg);
    }
}

/// One visible window owned by the target process, collected during enumeration.
struct WindowCandidate {
    rect: Rect,
    has_title: bool,
}

unsafe extern "system" fn enum_windows_rect_collect_callback(
    hwnd: isize,
    lparam: isize,
) -> i32 {
    // 1) Visible — invisible / minimised / cloaked windows are not what the
    //    user means by "the app I'm looking at".
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }
    // (Ownership by the target process is checked *after* the title read below
    // so the expensive process-name lookup runs on as few windows as possible.)
    // 2) Record whether this window declares a non-empty title. NOT a filter:
    //    Chromium main windows can report an empty title.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let mut has_title = false;
    if len > 0 {
        let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
        let actual = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if actual > 0 {
            let title = String::from_utf16_lossy(&buf[..actual as usize]);
            has_title = !title.trim().is_empty();
        }
    }
    // 3) Owned by the requested process. `process_name_of_pid` opens a handle
    //    per call, so bail out early for windows we can't resolve.
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let Some(name) = process_name_of_pid(pid) else {
        return 1;
    };
    let (candidates, process_name) = &mut *(lparam as *mut (Vec<WindowCandidate>, &str));
    if !name.eq_ignore_ascii_case(process_name) {
        return 1;
    }
    // 4) Read its rect.
    let mut rect = Rect::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } != 0
        && rect.right > rect.left
        && rect.bottom > rect.top
    {
        candidates.push(WindowCandidate { rect, has_title });
    }
    1
}

/// Return the process image name of the currently focused foreground window.
pub fn foreground_process_name() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        process_name_of_pid(pid)
    }
}

/// Return `true` when the foreground window belongs to the given process
/// (used by 录制指定软件 to only record events inside the target app).
pub fn foreground_is_target(process_name: &str) -> bool {
    foreground_process_name()
        .map(|n| n.eq_ignore_ascii_case(process_name))
        .unwrap_or(false)
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

/// Return `true` when the foreground window belongs to the Flowio process
/// itself (`flowio.exe`), based on the process image name rather than the
/// window title.
///
/// This avoids self-filter false positives when the user records a tutorial
/// about "录步" itself (browser pages / other apps whose titles contain
/// "录步" or "flowio" would otherwise be skipped).
pub fn foreground_is_flowio() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid_is_flowio(pid)
    }
}

/// Return `true` when the given PID belongs to the Flowio process.
/// Used by the event thread with the PID captured at hook-callback time
/// so that filtering is based on the foreground window at event capture,
/// not at (later) event processing.
pub fn pid_is_flowio(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if proc == 0 {
        return false;
    }
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(proc) };
    if ok == 0 {
        return false;
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or("");
    name.eq_ignore_ascii_case("flowio.exe")
}

/// Return `true` when the given PID matches the target process name.
/// PID-based variant of [`foreground_is_target`] for use in the event thread
/// with the PID captured at hook-callback time.
pub fn pid_is_target(pid: u32, target_process_name: &str) -> bool {
    if pid == 0 {
        return false;
    }
    process_name_of_pid(pid)
        .map(|n| n.eq_ignore_ascii_case(target_process_name))
        .unwrap_or(false)
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

    #[test]
    fn process_description_known_processes() {
        assert_eq!(process_description("wps.exe"), "WPS 文字（文档）");
        assert_eq!(process_description("WeChat.exe"), "微信");
        assert_eq!(process_description("OBS64.EXE"), "OBS 录屏/直播");
        assert_eq!(
            process_description("explorer.exe"),
            "文件资源管理器（桌面/文件夹窗口）"
        );
        assert_eq!(process_description("unknownapp.exe"), "");
    }
}
