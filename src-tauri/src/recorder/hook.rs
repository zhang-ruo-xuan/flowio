//! Native Windows low-level hooks (WH_KEYBOARD_LL + WH_MOUSE_LL).
//!
//! Replaces `rdev` for precise event capture. Screenshot capture is
//! decoupled from hook callbacks — the continuous capture ring buffer
//! (capture.rs) handles all screenshots, keeping hook latency near zero.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::Mutex;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::recorder::keyboard::KeyEvent;
use crate::recorder::mouse::CapturedClick;

/// Senders populated before the message pump starts.
/// Use Mutex so they can be replaced across multiple recording sessions
/// (OnceLock only allows one-time set; on second recording, old disconnected
/// senders would be used and all events silently lost).
static MOUSE_TX: Mutex<Option<Sender<CapturedClick>>> = Mutex::new(None);
static KEY_TX: Mutex<Option<Sender<KeyEvent>>> = Mutex::new(None);

// ── VK → key name (matches rdev's Debug format for backwards compat) ──────

fn vk_to_key_name(vk_code: u32) -> String {
    // Letters A-Z (0x41-0x5A)
    if (0x41..=0x5A).contains(&vk_code) {
        return format!("Key{}", char::from_u32(vk_code).unwrap_or('?'));
    }
    // Digits 0-9 (0x30-0x39)
    if (0x30..=0x39).contains(&vk_code) {
        return format!("Key{}", char::from_u32(vk_code).unwrap_or('?'));
    }
    // Numpad 0-9 (0x60-0x69)
    if (0x60..=0x69).contains(&vk_code) {
        return format!("Num{}", vk_code - 0x60);
    }
    // F1-F12 (0x70-0x7B)
    if (0x70..=0x7B).contains(&vk_code) {
        return format!("F{}", vk_code - 0x6F);
    }
    // OEM keys (keyboard-layout dependent; US layout names)
    match vk_code {
        0xBA => return "SemiColon".into(),     // ;:
        0xBB => return "Equal".into(),          // +=
        0xBC => return "Comma".into(),          // ,<
        0xBD => return "Minus".into(),          // -_
        0xBE => return "Period".into(),         // .>
        0xBF => return "Slash".into(),          // /?
        0xC0 => return "Grave".into(),          // `~
        0xDB => return "LeftBracket".into(),    // [{
        0xDC => return "BackSlash".into(),      // \|
        0xDD => return "RightBracket".into(),   // ]}
        0xDE => return "Quote".into(),          // '"
        0xE2 => return "BackSlash".into(),      // \| (non-US)
        _ => {}
    }
    // Standard VK_* constants — use raw u32 values to avoid
    // VIRTUAL_KEY newtype mismatch with u16/u32 casts.
    match vk_code {
        0x08 => "Backspace".into(),    // VK_BACK
        0x09 => "KeyTab".into(),       // VK_TAB
        0x0D => "Return".into(),       // VK_RETURN
        0x20 => "KeySpace".into(),     // VK_SPACE
        0x1B => "Escape".into(),       // VK_ESCAPE
        0x2E => "Delete".into(),       // VK_DELETE
        0x2D => "Insert".into(),       // VK_INSERT
        0x24 => "Home".into(),         // VK_HOME
        0x23 => "End".into(),          // VK_END
        0x21 => "PageUp".into(),       // VK_PRIOR
        0x22 => "PageDown".into(),     // VK_NEXT
        0x25 => "LeftArrow".into(),    // VK_LEFT
        0x27 => "RightArrow".into(),   // VK_RIGHT
        0x26 => "UpArrow".into(),      // VK_UP
        0x28 => "DownArrow".into(),    // VK_DOWN
        0x11 => "ControlLeft".into(),  // VK_CONTROL
        0x12 => "Alt".into(),          // VK_MENU
        0x10 => "ShiftLeft".into(),    // VK_SHIFT
        0x5B => "MetaLeft".into(),     // VK_LWIN
        0x5C => "MetaRight".into(),    // VK_RWIN
        0x14 => "CapsLock".into(),     // VK_CAPITAL
        0x90 => "NumLock".into(),      // VK_NUMLOCK
        _ => format!("Unknown({})", vk_code),
    }
}

// ── Hook procedures ────────────────────────────────────────────────────────

unsafe extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = info.vkCode;

        let is_press = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let is_release = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);

        if is_press {
            let name = vk_to_key_name(vk);
            if let Ok(guard) = KEY_TX.lock() {
                if let Some(ref tx) = *guard {
                    let _ = tx.send(KeyEvent::Press(name));
                }
            }
        } else if is_release {
            if let Ok(guard) = KEY_TX.lock() {
                if let Some(ref tx) = *guard {
                    let _ = tx.send(KeyEvent::Release);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn mouse_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let x = info.pt.x as f64;
            let y = info.pt.y as f64;
            let button = match msg {
                WM_LBUTTONDOWN => "Left",
                WM_RBUTTONDOWN => "Right",
                _ => "Middle",
            };

            // Record timestamp BEFORE dispatching click.
            // The event thread will find the pre-click frame from
            // the continuous capture ring buffer using this timestamp.
            let click_ts = std::time::Instant::now();

            if let Ok(guard) = MOUSE_TX.lock() {
                if let Some(ref tx) = *guard {
                    let _ = tx.send(CapturedClick {
                        x,
                        y,
                        button: button.to_string(),
                        timestamp: click_ts,
                    });
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialise native `WH_KEYBOARD_LL` + `WH_MOUSE_LL` hooks on a
/// dedicated background thread that runs a Windows message pump.
///
/// Keyboard events → `key_tx`, mouse clicks → `mouse_tx` (prefixed
/// with a synchronously-captured "before" screenshot).
///
/// The thread runs until `running` is set to `false`, at which point
/// it unhooks and posts `WM_QUIT`.
pub fn init_hooks(
    key_tx: Sender<KeyEvent>,
    mouse_tx: Sender<CapturedClick>,
    running: Arc<AtomicBool>,
) {
    // Store senders in statics so the hook callbacks can access them.
    // Using Mutex<Option<>> instead of OnceLock allows replacement across
    // multiple recording sessions.
    {
        let mut guard = KEY_TX.lock().unwrap();
        *guard = Some(key_tx);
    }
    {
        let mut guard = MOUSE_TX.lock().unwrap();
        *guard = Some(mouse_tx);
    }

    std::thread::spawn(move || {
        eprintln!("[HOOK] installing native hooks");

        // Install keyboard hook
        let khook = match unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), HINSTANCE::default(), 0)
        } {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[HOOK] SetWindowsHookExW(WH_KEYBOARD_LL) failed: {:?}", e);
                return;
            }
        };

        // Install mouse hook
        let mhook = match unsafe {
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), HINSTANCE::default(), 0)
        } {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[HOOK] SetWindowsHookExW(WH_MOUSE_LL) failed: {:?}", e);
                unsafe { let _ = UnhookWindowsHookEx(khook); }
                return;
            }
        };

        eprintln!("[HOOK] hooks installed, entering message pump");

        // Message pump — required for low-level hooks to fire.
        let mut msg = MSG::default();
        loop {
            let has_msg = unsafe {
                windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                    &mut msg,
                    None,
                    0,
                    0,
                    windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
                )
            };

            if has_msg.as_bool() {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    break;
                }
                unsafe {
                    let _ = DispatchMessageW(&msg);
                }
            } else {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        eprintln!("[HOOK] unhooking and exiting");
        unsafe {
            let _ = UnhookWindowsHookEx(khook);
            let _ = UnhookWindowsHookEx(mhook);
        }
    });
}
