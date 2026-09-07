//! 重放后端：真实 Win32 执行层 + 测试用 DryRun（无副作用）。

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::anchor::{locate_click, ControlAnchor};

/// 重放执行后端抽象。真实环境用 [`WindowsBackend`]，单测用 [`DryRunBackend`]。
pub trait ReplayBackend {
    fn focus(&self, title: &str);
    fn click(&self, x: i32, y: i32);
    fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32);
    fn type_text(&self, text: &str);
    /// 下发组合键（如 "Ctrl+C" / "Alt+Tab" / "Win+D"）。
    /// 修饰键先按下、主键后按、逆序释放；解析失败应记录日志而非崩溃。
    fn press_combo(&self, combo: &str);
    /// 按控件锚点（Stage 2b）反算点击坐标；无锚点或定位失败时回退到 (fallback_x, fallback_y)。
    /// 锚点 JSON 为空 / 解析失败 → 直接按回退坐标点击。
    fn click_anchored(&self, anchor_json: &str, fallback_x: i32, fallback_y: i32);
}

// ── 测试后端：只记录动作，不碰真实桌面 ──────────────────────
pub struct DryRunBackend {
    pub actions: Mutex<Vec<String>>,
}
impl DryRunBackend {
    pub fn new() -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
        }
    }
}
impl ReplayBackend for DryRunBackend {
    fn focus(&self, title: &str) {
        self.actions.lock().unwrap().push(format!("focus({})", title));
    }
    fn click(&self, x: i32, y: i32) {
        self.actions.lock().unwrap().push(format!("click({},{}", x, y));
    }
    fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32) {
        self.actions
            .lock()
            .unwrap()
            .push(format!("drag({},{}->{},{}", x1, y1, x2, y2));
    }
    fn type_text(&self, text: &str) {
        self.actions
            .lock()
            .unwrap()
            .push(format!("type({} chars)", text.chars().count()));
    }
    fn press_combo(&self, combo: &str) {
        self.actions
            .lock()
            .unwrap()
            .push(format!("hotkey({})", combo));
    }
    fn click_anchored(&self, anchor_json: &str, fallback_x: i32, fallback_y: i32) {
        self.actions.lock().unwrap().push(format!(
            "click_anchored({},{},{})",
            fallback_x, fallback_y, anchor_json
        ));
    }
}

// ── 真实 Win32 后端 ────────────────────────────────────────
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    SendInput, VIRTUAL_KEY, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, mouse_event, VK_APPS,
    VK_BACK, VK_CAPITAL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_INSERT, VK_LEFT,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SNAPSHOT,
    VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, HWND_NOTOPMOST, HWND_TOPMOST, IsWindowVisible,
    SetCursorPos, SetForegroundWindow, SetWindowPos, ShowWindow, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE,
};

struct EnumCtx<'a> {
    substring: &'a str,
    found: Option<HWND>,
}

/// 按窗口标题子串查找可见窗口句柄（标题常被截断，故用 contains 模糊匹配）。
fn find_window(substring: &str) -> Option<HWND> {
    let mut ctx = EnumCtx {
        substring,
        found: None,
    };
    unsafe {
        EnumWindows(
            Some(enum_windows_proc),
            windows::Win32::Foundation::LPARAM(&mut ctx as *mut _ as isize),
        )
        .ok();
    }
    ctx.found
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: windows::Win32::Foundation::LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut EnumCtx);
    if IsWindowVisible(hwnd).as_bool() {
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title.contains(ctx.substring) {
                ctx.found = Some(hwnd);
                return BOOL(0); // 停止枚举
            }
        }
    }
    BOOL(1)
}

/// 重放期间记住「上一次成功聚焦的目标窗口」。全屏录制每步标题虽大多有值，
/// 但个别步（桌面/无标题窗口/保存对话框标题模糊）标题为空或匹配不到，
/// 此时复用上一次成功聚焦的窗口，避免键盘输入落到桌面导致「文件没建出来」。
static LAST_FOCUS: Mutex<Option<usize>> = Mutex::new(None);

/// 新一次重放开始时清空缓存，避免上一轮残留的窗口句柄干扰本轮首步。
pub fn reset_replay_focus_cache() {
    if let Ok(mut g) = LAST_FOCUS.lock() {
        *g = None;
    }
}

/// 强制把目标窗口提到前台。
/// `SetForegroundWindow` 在重放进行数秒后（用户「近期输入」宽限期过期）会**静默失败**，
/// 此后键盘输入就会打到桌面而非目标软件——这正是「鼠标在动、但文件没生成」的根因。
/// 这里用「瞬置顶→取消置顶」技巧绕过前台锁把窗口强抢到最前，失败再退路 `ShowWindow+SetForegroundWindow`。
fn force_foreground(hwnd: HWND) -> bool {
    unsafe {
        if GetForegroundWindow() == hwnd {
            return true;
        }
        // 技巧：先置顶再取消置顶，可绕过前台锁把窗口带到最前
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        ShowWindow(hwnd, SW_RESTORE);
        if SetForegroundWindow(hwnd).as_bool() {
            return true;
        }
        // 仍失败：再试一次瞬置顶 + 前台
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        SetForegroundWindow(hwnd).as_bool()
    }
}

/// 解析目标窗口句柄，带有限重试。
/// 重放中某些步（如 Ctrl+S 后弹出的「另存为」对话框）目标窗口可能尚未被枚举到，
/// 立即查找会失败；重试几次可等到对话框真正出现在窗口列表中，避免键盘落到错误窗口。
fn find_window_retry(substring: &str) -> Option<HWND> {
    for attempt in 0..3 {
        if let Some(h) = find_window(substring) {
            return Some(h);
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(120));
        }
    }
    None
}

/// 取当前前台窗口标题（诊断用）。
fn foreground_title() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

/// 重放诊断日志：每步记录聚焦结果，供「重放鼠标在动但文件没生成」类问题远程定位。
/// 仅在重放时写入，频率 = 每步一次（非录制期每按键），且只在执行真实桌面动作时触发，开销可忽略。
pub(crate) fn replay_log(line: &str) {
    use std::io::Write;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) =
        std::fs::OpenOptions::new().create(true).append(true).open(r"D:\录步\flowio_replay.log")
    {
        let _ = writeln!(f, "[{}] {}", ts, line);
    }
}

fn focus_window(title: &str) -> bool {
    let t = title.trim();
    replay_log(&format!("[focus] >>> enter title='{}'", t));
    // 解析目标句柄：有标题按子串找（带重试，等对话框就绪）；无标题/找不到则复用上一次成功聚焦的窗口。
    let candidate = if t.is_empty() {
        None
    } else {
        find_window_retry(t)
    };
    let cached = LAST_FOCUS
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|raw| HWND(raw as *mut std::ffi::c_void))
        .filter(|h| unsafe { IsWindowVisible(*h).as_bool() });
    let hwnd = candidate.or(cached);
    let Some(hwnd) = hwnd else {
        replay_log(&format!(
            "[focus] <<< NO WINDOW for title='{}' (no candidate & no cached) -> focus SKIPPED",
            t
        ));
        return false;
    };
    let ok = force_foreground(hwnd);
    if ok {
        if let Ok(mut g) = LAST_FOCUS.lock() {
            *g = Some(hwnd.0 as usize);
        }
    }
    replay_log(&format!(
        "[focus] title='{}' ok={} fg_after='{}'",
        t,
        ok,
        foreground_title()
    ));
    ok
}

fn click(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
        std::thread::sleep(Duration::from_millis(10));
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        std::thread::sleep(Duration::from_millis(10));
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

fn drag(x1: i32, y1: i32, x2: i32, y2: i32) {
    unsafe {
        let _ = SetCursorPos(x1, y1);
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        for i in 1..=10 {
            let ix = x1 + (x2 - x1) * i / 10;
            let iy = y1 + (y2 - y1) * i / 10;
            let _ = SetCursorPos(ix, iy);
            std::thread::sleep(Duration::from_millis(8));
        }
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

/// 通过 SendInput 以 Unicode 方式注入文本（跳过输入法，中文直接落字）。
fn send_unicode_char(ch: char) {
    let mut down = unsafe { std::mem::zeroed::<INPUT>() };
    down.r#type = INPUT_KEYBOARD;
    unsafe {
        down.Anonymous.ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(0),
            wScan: ch as u16,
            dwFlags: KEYEVENTF_UNICODE,
            time: 0,
            dwExtraInfo: 0,
        };
    }
    let mut up = unsafe { std::mem::zeroed::<INPUT>() };
    up.r#type = INPUT_KEYBOARD;
    unsafe {
        up.Anonymous.ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(0),
            wScan: ch as u16,
            dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
        };
    }
    for input in [down, up] {
        let mut arr = [input];
        unsafe {
            SendInput(&mut arr, std::mem::size_of::<INPUT>() as i32);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn send_text(text: &str) {
    for ch in text.chars() {
        send_unicode_char(ch);
    }
}

/// 通过 SendInput 下发一个虚拟键（按下/抬起由 down 决定）。用于组合键（如 Ctrl+C）。
fn send_key(vk: VIRTUAL_KEY, down: bool) {
    let mut inp = unsafe { std::mem::zeroed::<INPUT>() };
    inp.r#type = INPUT_KEYBOARD;
    unsafe {
        inp.Anonymous.ki = KEYBDINPUT {
            wVk: vk,
            wScan: 0,
            dwFlags: if down {
                KEYBD_EVENT_FLAGS(0)
            } else {
                KEYEVENTF_KEYUP
            },
            time: 0,
            dwExtraInfo: 0,
        };
    }
    let mut arr = [inp];
    unsafe {
        SendInput(&mut arr, std::mem::size_of::<INPUT>() as i32);
    }
    std::thread::sleep(Duration::from_millis(5));
}

/// 将组合键中的单个 token（如 "Ctrl" / "C" / "F5" / "Enter"）映射为虚拟键码。
fn token_to_vk(token: &str) -> Option<VIRTUAL_KEY> {
    match token.trim() {
        "Ctrl" | "Control" | "LCtrl" => Some(VK_LCONTROL),
        "Alt" => Some(VK_LMENU),
        "Shift" => Some(VK_LSHIFT),
        "Win" | "Meta" => Some(VK_LWIN),
        "Enter" | "Return" => Some(VK_RETURN),
        "Tab" => Some(VK_TAB),
        "Space" => Some(VK_SPACE),
        "Esc" | "Escape" => Some(VK_ESCAPE),
        "Backspace" => Some(VK_BACK),
        "Delete" => Some(VK_DELETE),
        "Home" => Some(VK_HOME),
        "End" => Some(VK_END),
        "Insert" => Some(VK_INSERT),
        "PageUp" | "PgUp" => Some(VK_PRIOR),
        "PageDown" | "PgDn" => Some(VK_NEXT),
        "Left" | "ArrowLeft" => Some(VK_LEFT),
        "Right" | "ArrowRight" => Some(VK_RIGHT),
        "Up" | "ArrowUp" => Some(VK_UP),
        "Down" | "ArrowDown" => Some(VK_DOWN),
        "PrintScreen" | "PrtSc" => Some(VK_SNAPSHOT),
        "CapsLock" => Some(VK_CAPITAL),
        "Menu" => Some(VK_APPS),
        s if s.len() == 1 => {
            let c = s.as_bytes()[0];
            if c.is_ascii_alphabetic() {
                Some(VIRTUAL_KEY((c.to_ascii_uppercase() - b'A' + 0x41) as u16))
            } else if c.is_ascii_digit() {
                Some(VIRTUAL_KEY((c - b'0' + 0x30) as u16))
            } else {
                None
            }
        }
        s if s.starts_with('F') && s[1..].parse::<u32>().map(|n| n >= 1 && n <= 12).unwrap_or(false) => {
            Some(VIRTUAL_KEY((0x70 + s[1..].parse::<u32>().unwrap()) as u16))
        }
        _ => None,
    }
}

/// 真实 Win32 执行后端。
pub struct WindowsBackend;
impl ReplayBackend for WindowsBackend {
    fn focus(&self, title: &str) {
        focus_window(title);
    }
    fn click(&self, x: i32, y: i32) {
        click(x, y);
    }
    fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32) {
        drag(x1, y1, x2, y2);
    }
    fn type_text(&self, text: &str) {
        send_text(text);
    }
    fn press_combo(&self, combo: &str) {
        let mut mods: Vec<VIRTUAL_KEY> = Vec::new();
        let mut main_key: Option<VIRTUAL_KEY> = None;
        for part in combo.split('+') {
            match part.trim() {
                "Ctrl" | "Control" | "LCtrl" => mods.push(VK_LCONTROL),
                "Alt" => mods.push(VK_LMENU),
                "Shift" => mods.push(VK_LSHIFT),
                "Win" | "Meta" => mods.push(VK_LWIN),
                other => {
                    if let Some(vk) = token_to_vk(other) {
                        main_key = Some(vk);
                    }
                }
            }
        }
        let Some(mk) = main_key else {
            eprintln!("[replay] press_combo: 无法解析组合键 '{}'", combo);
            return;
        };
        // 修饰键先按下（Ctrl → Alt → Win → Shift），主键后按，再逆序释放。
        for m in &mods {
            send_key(*m, true);
        }
        send_key(mk, true);
        send_key(mk, false);
        for m in mods.iter().rev() {
            send_key(*m, false);
        }
    }
    fn click_anchored(&self, anchor_json: &str, fallback_x: i32, fallback_y: i32) {
        let anchor = match ControlAnchor::from_str(anchor_json) {
            Some(a) if !a.is_empty() => a,
            _ => {
                self.click(fallback_x, fallback_y);
                return;
            }
        };
        // focus() 已把目标窗口设为前景窗口；取其句柄在当前窗口内定位控件。
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            self.click(fallback_x, fallback_y);
            return;
        }
        match locate_click(&anchor, hwnd) {
            Some((cx, cy)) => self.click(cx, cy),
            None => self.click(fallback_x, fallback_y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锁定「录制端 display_shortcut_key 产出的 combo 串 == 重放端 token_to_vk 能解析」
    /// 的契约。nav/edit 键（Enter/Tab/Esc/方向/Backspace/Delete/Home/End/翻页）此前被整段
    /// 丢弃导致「文件没生成」，修复后它们以这些 combo 串落步；若任一侧映射被改，此测试会
    /// 立即失败，防止静默回归。
    #[test]
    fn replay_combo_tokens_resolve_to_expected_vk() {
        let cases: &[(&str, VIRTUAL_KEY)] = &[
            ("Enter", VK_RETURN),
            ("Tab", VK_TAB),
            ("Esc", VK_ESCAPE),
            ("ArrowUp", VK_UP),
            ("ArrowDown", VK_DOWN),
            ("ArrowLeft", VK_LEFT),
            ("ArrowRight", VK_RIGHT),
            ("Backspace", VK_BACK),
            ("Delete", VK_DELETE),
            ("Home", VK_HOME),
            ("End", VK_END),
            ("PageUp", VK_PRIOR),
            ("PageDown", VK_NEXT),
        ];
        for (token, expected) in cases {
            assert_eq!(
                token_to_vk(token),
                Some(*expected),
                "combo 串 '{}' 应能被 token_to_vk 解析为对应虚拟键",
                token
            );
        }
    }
}
