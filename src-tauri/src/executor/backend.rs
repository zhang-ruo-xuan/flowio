//! 重放后端：真实 Win32 执行层 + 测试用 DryRun（无副作用）。

use std::sync::Mutex;
use std::time::Duration;

/// 重放执行后端抽象。真实环境用 [`WindowsBackend`]，单测用 [`DryRunBackend`]。
pub trait ReplayBackend {
    fn focus(&self, title: &str);
    fn click(&self, x: i32, y: i32);
    fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32);
    fn type_text(&self, text: &str);
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
}

// ── 真实 Win32 后端 ────────────────────────────────────────
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
    VIRTUAL_KEY, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, mouse_event,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, IsWindowVisible, SetCursorPos, SetForegroundWindow,
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
}
