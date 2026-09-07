//! 重放后端：真实 Win32 执行层 + 测试用 DryRun（无副作用）。

use std::sync::Mutex;
use std::time::Duration;

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
    EnumWindows, GetForegroundWindow, GetWindowTextW, IsWindowVisible, SetCursorPos,
    SetForegroundWindow,
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

fn focus_window(title: &str) -> bool {
    if let Some(hwnd) = find_window(title) {
        unsafe { SetForegroundWindow(hwnd).as_bool() }
    } else {
        false
    }
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
