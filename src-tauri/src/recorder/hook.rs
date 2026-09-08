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
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetWindowThreadProcessId,
    SetWindowsHookExW,
    UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;

use crate::recorder::keyboard::KeyEvent;
use crate::recorder::mouse::CapturedClick;

/// Senders populated before the message pump starts.
/// Use Mutex so they can be replaced across multiple recording sessions
/// (OnceLock only allows one-time set; on second recording, old disconnected
/// senders would be used and all events silently lost).
static MOUSE_TX: Mutex<Option<Sender<CapturedClick>>> = Mutex::new(None);
static KEY_TX: Mutex<Option<Sender<KeyEvent>>> = Mutex::new(None);

/// 上一轮钩子线程句柄 + 它使用的停止标志。
///
/// 用途：保证**任何时刻只有一个** `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 钩子存在。
/// `init_hooks()` 每次调用都会 spawn 新线程并 `SetWindowsHookExW` 装新钩子，
/// 但旧钩子只在旧线程跑到末尾时才 `UnhookWindowsHookEx`。若两个钩子并存，
/// 每次点击会触发两次回调，而两个回调都读同一个 `MOUSE_TX` static（指向
/// 最新 session 的 sender），于是同一次点击被投递两遍 —— 表现为每个操作
/// 被记成两步（用户反馈的「1 和 2 重复、3 和 4 重复」）。
/// 装新钩子前先把旧线程停掉并 join，可彻底杜绝这种情况。
static HOOK_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static HOOK_STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Capture the foreground window PID inside a hook callback.
/// Called at event-capture time so the event thread can filter based on
/// which window was foreground when the event actually happened.
unsafe fn get_fg_pid() -> u32 {
    let hwnd = GetForegroundWindow();
    if hwnd.is_invalid() {
        return 0;
    }
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    pid
}

/// Like `get_fg_pid`, but also returns the foreground window's keyboard-layout
/// handle (HKL) so `ToUnicodeEx` can translate with the *target app's* layout
/// rather than flowio's thread default.
unsafe fn get_fg_pid_and_hkl() -> (u32, usize) {
    let hwnd = GetForegroundWindow();
    if hwnd.is_invalid() {
        return (0, 0);
    }
    let mut pid: u32 = 0;
    let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let hkl = GetKeyboardLayout(tid).0 as usize;
    (pid, hkl)
}

/// Modifier key down-state tracking (combo-key semantics for Ctrl+C/V, etc.).
/// Updated inside the WH_KEYBOARD_LL callback before each Press is forwarded,
/// so the event carries the modifier state at the moment the key went down.
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static META_DOWN: AtomicBool = AtomicBool::new(false); // Win key (Meta)

/// Force-reset all modifier down-state flags.
///
/// Called at the start of every recording session so that a stuck flag from a
/// previous session (e.g. Alt keyup swallowed by RegisterHotKey) does not
/// poison every subsequent keystroke into an artificial "Alt+X" combo.
pub fn reset_modifier_flags() {
    CTRL_DOWN.store(false, Ordering::SeqCst);
    SHIFT_DOWN.store(false, Ordering::SeqCst);
    ALT_DOWN.store(false, Ordering::SeqCst);
    META_DOWN.store(false, Ordering::SeqCst);
}

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
        0xDE => return "Quote".into(),          // '
        0x2C => return "PrintScreen".into(),    // VK_SNAPSHOT（截图键）
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
        0xA0 => "ShiftLeft".into(),    // VK_LSHIFT
        0xA1 => "ShiftRight".into(),   // VK_RSHIFT
        0xA2 => "ControlLeft".into(),  // VK_LCONTROL
        0xA3 => "ControlRight".into(), // VK_RCONTROL
        0xA4 => "AltLeft".into(),      // VK_LMENU
        0xA5 => "AltRight".into(),     // VK_RMENU
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
        // Low byte is the scan code; the high byte carries the extended-key flag
        // (0xE0) which ToUnicodeEx does not want.
        let scan = info.scanCode & 0xFF;

        let is_press = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let is_release = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);

        // Track modifier key state so combo keys (Ctrl+C/V, Ctrl+A/Z, Alt+Tab…)
        // can be modelled as semantic steps instead of polluting the type buffer.
        if is_press {
            match vk {
                // 兼容左右区分码（VK_LCONTROL=0xA2/VK_RCONTROL=0xA3/VK_LSHIFT=0xA0/
                // VK_RSHIFT=0xA1/VK_LMENU=0xA4/VK_RMENU=0xA5）：低层钩子可能只上报具体侧码
                0x11 | 0xA2 | 0xA3 => CTRL_DOWN.store(true, Ordering::SeqCst),
                0x10 | 0xA0 | 0xA1 => SHIFT_DOWN.store(true, Ordering::SeqCst),
                0x12 | 0xA4 | 0xA5 => ALT_DOWN.store(true, Ordering::SeqCst),
                // Win 键（VK_LWIN=0x5B / VK_RWIN=0x5C）：用于识别 Win+Combo 快捷键
                // （Win+D 显示桌面等），否则 D 会漏进 type buffer 被记成字符 "d"。
                0x5B | 0x5C => META_DOWN.store(true, Ordering::SeqCst),
                _ => {}
            }
        } else if is_release {
            match vk {
                0x11 | 0xA2 | 0xA3 => CTRL_DOWN.store(false, Ordering::SeqCst),
                0x10 | 0xA0 | 0xA1 => SHIFT_DOWN.store(false, Ordering::SeqCst),
                0x12 | 0xA4 | 0xA5 => ALT_DOWN.store(false, Ordering::SeqCst),
                0x5B | 0x5C => META_DOWN.store(false, Ordering::SeqCst),
                _ => {}
            }
        }

        // Modifier keys (Ctrl/Shift/Alt/Win) only update the down-state; they must
        // NOT emit standalone Press events. Otherwise a bare Ctrl press would be
        // modelled as a bogus "快捷键 Ctrl+ControlLeft" hotkey step, and combos
        // like Ctrl+C would produce two steps (hotkey + copy) instead of one.
        // Win (0x5B/0x5C) is included so the key itself never emits — only the
        // *other* key of a Win+Combo arrives with `meta=true`.
        let is_modifier = matches!(vk, 0x11 | 0x10 | 0x12 | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5 | 0x5B | 0x5C);
        if is_press && !is_modifier {
            let name = vk_to_key_name(vk);
            // 真实按键时刻（钩子触发的瞬间）。处理线程消费事件时也可能是
            // 几百毫秒之后，但帧环里要找的是这一刻的屏幕，所以必须带上。
            let ts = std::time::Instant::now();
            let (fg_pid, fg_hkl) = get_fg_pid_and_hkl();
            if let Ok(guard) = KEY_TX.lock() {
                if let Some(ref tx) = *guard {
                    let _ = tx.send(KeyEvent::Press {
                        key: name,
                        vk,
                        scan,
                        hkl: fg_hkl,
                        ctrl: CTRL_DOWN.load(Ordering::SeqCst),
                        shift: SHIFT_DOWN.load(Ordering::SeqCst),
                        alt: ALT_DOWN.load(Ordering::SeqCst),
                        meta: META_DOWN.load(Ordering::SeqCst),
                        ts,
                        foreground_pid: fg_pid,
                    });
                }
            }
        } else if is_release && !is_modifier {
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
        // Press (down) AND release (up) are captured so the event thread can
        // pair them and detect drags. See `ClickPhase` in mouse.rs.
        let phase = if msg == WM_LBUTTONUP || msg == WM_RBUTTONUP || msg == WM_MBUTTONUP {
            crate::recorder::mouse::ClickPhase::Up
        } else if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN {
            crate::recorder::mouse::ClickPhase::Down
        } else if msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL {
            // ★ 滚轮：以前被直接丢弃（"e.g. wheel. Skip"）→ 教程里的滚动操作
            // 完全录不下来，重放不滚动，后续点击全落在错误位置。
            // 现在按 CapturedClick 上报（button="wheel"，增量在 wheel_delta），
            // 由事件线程做时间窗合并后生成一条 scroll 步骤。
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let raw = (((info.mouseData >> 16) & 0xFFFF) as u16) as i16 as i32;
            let fg_pid = get_fg_pid();
            if let Ok(guard) = MOUSE_TX.lock() {
                if let Some(ref tx) = *guard {
                    let _ = tx.send(CapturedClick {
                        x: info.pt.x as f64,
                        y: info.pt.y as f64,
                        button: "wheel".to_string(),
                        phase: crate::recorder::mouse::ClickPhase::Down,
                        timestamp: std::time::Instant::now(),
                        foreground_pid: fg_pid,
                        wheel_delta: raw,
                    });
                }
            }
            CallNextHookEx(None, code, wparam, lparam);
            return LRESULT(0);
        } else {
            // Not a button press/release we care about. Skip.
            CallNextHookEx(None, code, wparam, lparam);
            return LRESULT(0);
        };
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let x = info.pt.x as f64;
        let y = info.pt.y as f64;
        let button = match msg {
            WM_LBUTTONDOWN | WM_LBUTTONUP => "left",
            WM_RBUTTONDOWN | WM_RBUTTONUP => "right",
            _ => "middle",
        };

        // Record timestamp BEFORE dispatching click.
        // The event thread will find the pre-click frame from
        // the continuous capture ring buffer using this timestamp.
        let click_ts = std::time::Instant::now();
        // Capture foreground PID at click time for accurate filtering.
        let fg_pid = get_fg_pid();

        if let Ok(guard) = MOUSE_TX.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(CapturedClick {
                    x,
                    y,
                    button: button.to_string(),
                    phase,
                    timestamp: click_ts,
                    foreground_pid: fg_pid,
                    wheel_delta: 0,
                });
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
    // ── 钩子单例：先停掉并回收上一轮钩子线程 ──
    // 不这么做的话，两个 WH_MOUSE_LL 并存会让每次点击上报两次，
    // 最终每个操作被记成两步（成对重复）。
    {
        if let Ok(guard) = HOOK_STOP.lock() {
            if let Some(prev_stop) = guard.as_ref() {
                prev_stop.store(false, Ordering::Relaxed);
            }
        }
        if let Ok(mut guard) = HOOK_THREAD.lock() {
            if let Some(prev) = guard.take() {
                if !prev.is_finished() {
                    eprintln!(
                        "[HOOK] previous hook thread still alive — joining before installing new hooks"
                    );
                    let _ = prev.join();
                    eprintln!("[HOOK] previous hook thread joined");
                }
            }
        }
    }
    if let Ok(mut guard) = HOOK_STOP.lock() {
        *guard = Some(Arc::clone(&running));
    }

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

    let handle = std::thread::spawn(move || {
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

    // 记录句柄，供下一次 init_hooks 回收（保证钩子单例）
    if let Ok(mut guard) = HOOK_THREAD.lock() {
        *guard = Some(handle);
    }
}

#[cfg(test)]
mod hook_tests {
    use super::{vk_to_key_name, reset_modifier_flags, CTRL_DOWN, SHIFT_DOWN, ALT_DOWN, META_DOWN};
    use std::sync::atomic::Ordering;

    #[test]
    fn letters_map_to_key_prefix() {
        assert_eq!(vk_to_key_name(0x41), "KeyA");
        assert_eq!(vk_to_key_name(0x5A), "KeyZ");
        assert_eq!(vk_to_key_name(0x4D), "KeyM");
    }

    #[test]
    fn digits_map_to_key_prefix() {
        assert_eq!(vk_to_key_name(0x30), "Key0");
        assert_eq!(vk_to_key_name(0x39), "Key9");
    }

    #[test]
    fn numpad_map_to_num_prefix() {
        assert_eq!(vk_to_key_name(0x60), "Num0");
        assert_eq!(vk_to_key_name(0x69), "Num9");
    }

    #[test]
    fn function_keys_map_to_f_prefix() {
        assert_eq!(vk_to_key_name(0x70), "F1");
        assert_eq!(vk_to_key_name(0x7B), "F12");
    }

    #[test]
    fn oem_punctuation_mapping() {
        assert_eq!(vk_to_key_name(0xBA), "SemiColon");
        assert_eq!(vk_to_key_name(0xBB), "Equal");
        assert_eq!(vk_to_key_name(0xBC), "Comma");
        assert_eq!(vk_to_key_name(0xBD), "Minus");
        assert_eq!(vk_to_key_name(0xBE), "Period");
        assert_eq!(vk_to_key_name(0xBF), "Slash");
        assert_eq!(vk_to_key_name(0xC0), "Grave");
        assert_eq!(vk_to_key_name(0xDB), "LeftBracket");
        assert_eq!(vk_to_key_name(0xDC), "BackSlash");
        assert_eq!(vk_to_key_name(0xDD), "RightBracket");
        assert_eq!(vk_to_key_name(0xDE), "Quote");
    }

    #[test]
    fn control_keys_mapping() {
        assert_eq!(vk_to_key_name(0x08), "Backspace");
        assert_eq!(vk_to_key_name(0x0D), "Return");
        assert_eq!(vk_to_key_name(0x20), "KeySpace");
        assert_eq!(vk_to_key_name(0x09), "KeyTab");
        assert_eq!(vk_to_key_name(0x1B), "Escape");
    }

    #[test]
    fn arrow_keys_mapping() {
        assert_eq!(vk_to_key_name(0x25), "LeftArrow");
        assert_eq!(vk_to_key_name(0x26), "UpArrow");
        assert_eq!(vk_to_key_name(0x27), "RightArrow");
        assert_eq!(vk_to_key_name(0x28), "DownArrow");
    }

    #[test]
    fn modifier_keys_mapping() {
        assert_eq!(vk_to_key_name(0x10), "ShiftLeft");
        assert_eq!(vk_to_key_name(0x11), "ControlLeft");
        assert_eq!(vk_to_key_name(0x12), "Alt");
        assert_eq!(vk_to_key_name(0x5B), "MetaLeft");
        assert_eq!(vk_to_key_name(0x5C), "MetaRight");
        assert_eq!(vk_to_key_name(0x14), "CapsLock");
        assert_eq!(vk_to_key_name(0x90), "NumLock");
    }

    #[test]
    fn unknown_vk_returns_unknown_format() {
        assert_eq!(vk_to_key_name(0xFF), "Unknown(255)");
        assert_eq!(vk_to_key_name(0x01), "Unknown(1)");
    }

    #[test]
    fn reset_modifier_flags_clears_all() {
        // Simulate stuck flags
        CTRL_DOWN.store(true, Ordering::SeqCst);
        SHIFT_DOWN.store(true, Ordering::SeqCst);
        ALT_DOWN.store(true, Ordering::SeqCst);
        META_DOWN.store(true, Ordering::SeqCst);

        reset_modifier_flags();

        assert!(!CTRL_DOWN.load(Ordering::SeqCst));
        assert!(!SHIFT_DOWN.load(Ordering::SeqCst));
        assert!(!ALT_DOWN.load(Ordering::SeqCst));
        assert!(!META_DOWN.load(Ordering::SeqCst));
    }
}
