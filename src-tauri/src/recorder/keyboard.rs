use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, HKL, ToUnicodeEx};

#[derive(Debug, Clone)]
pub enum KeyEvent {
    /// Key press with modifier state and the wall-clock Instant at which the
    /// hook fired (used to pick the frame closest to the real key moment when
    /// recording a shortcut step).
    Press {
        key: String,
        /// Win32 virtual-key code (e.g. 0x38 = VK_8). Carried so the consumer
        /// can resolve the *actual typed character* via ToUnicodeEx, which needs
        /// the vk + scan to honour the current layout, Shift, CapsLock, etc.
        vk: u32,
        /// Scan code (low byte of KBDLLHOOKSTRUCT.scanCode). Distinguishes
        /// left/right variants and feeds ToUnicodeEx for correct chars.
        scan: u32,
        /// Keyboard-layout handle (HKL) of the foreground window at key time.
        /// Lets ToUnicodeEx translate using the *target app's* layout, not
        /// flowio's thread default — important when the user has multiple
        /// input languages installed.
        hkl: usize,
        ctrl: bool,
        shift: bool,
        alt: bool,
        ts: std::time::Instant,
        /// Foreground window PID captured inside the hook callback at key-press
        /// time (self-filter / target-filter use it to decide whether the event
        /// belongs to the recorded app).
        foreground_pid: u32,
    },
    Release,
}

/// Convert a raw key name (from the native VK mapping in hook.rs) into
/// a typed character string.
/// Returns empty string for modifier / lock / navigation keys.
pub fn key_to_char(key_name: &str) -> String {
    match key_name {
        "KeySpace" => " ".to_string(),
        "Return" | "KeyEnter" => "\n".to_string(),
        "KeyTab" => "\t".to_string(),
        "Backspace" | "Back" => "\u{0008}".to_string(), // sentinel – caller handles pop
        s if s.starts_with("Shift")
            || s.starts_with("Control")
            || s.starts_with("Alt")
            || s.starts_with("Meta")
            || s.starts_with("Num")
            || s.starts_with("Caps")
            || s.starts_with("Fn")
            || s.starts_with("Super") =>
        {
            String::new()
        }
        s if s.starts_with("Key") => {
            s.strip_prefix("Key").unwrap_or(s).to_lowercase()
        }
        "Comma" => ",".to_string(),
        "Period" => ".".to_string(),
        "SemiColon" => ";".to_string(),
        "Slash" => "/".to_string(),
        "BackSlash" => "\\".to_string(),
        "Minus" => "-".to_string(),
        "Equal" => "=".to_string(),
        "LeftBracket" => "[".to_string(),
        "RightBracket" => "]".to_string(),
        "Quote" => "'".to_string(),
        "Apostrophe" => "'".to_string(),
        "Grave" => "`".to_string(),
        other => format!("[{}]", other),
    }
}

/// Virtual-key codes that never produce a typed character (modifier keys).
fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5
    )
}

/// Virtual-key codes for navigation keys that should flush rather than type.
fn is_nav_vk(vk: u32) -> bool {
    matches!(vk, 0x0D /* Enter */ | 0x09 /* Tab */ | 0x1B /* Esc */)
}

/// Resolve the *actual character(s)* the user typed for a given physical key,
/// honouring the current keyboard layout and live modifier state.
///
/// Uses Win32 `ToUnicodeEx` so:
/// - `Shift+8` → `*` (symbol on the upper row, not the bare digit)
/// - `Shift+=` → `+`, `Shift+a` → `A`
/// - CapsLock latches uppercase (`GetKeyState(VK_CAPITAL)` toggle bit)
/// - the *target app's* keyboard layout (HKL) is used, not flowio's default
///
/// This is what lets desktop capture match Scribe's "what was actually typed"
/// behaviour, including symbols, case and non-US layouts.
///
/// Returns an empty string for keys that produce no character: modifier keys,
/// navigation keys, F-keys, dead keys in isolation, etc.
pub fn to_unicode_char(vk: u32, scan: u32, shift: bool, ctrl: bool, alt: bool, hkl: usize) -> String {
    // Modifiers and navigation keys never yield typed text.
    if is_modifier_vk(vk) || is_nav_vk(vk) {
        return String::new();
    }
    unsafe {
        // CapsLock is a toggle — read its live state (bit 0x01 = on).
        let caps = (GetKeyState(0x14) & 0x01) != 0;
        let mut state = [0u8; 256];
        if shift {
            state[0x10] = 0x80;
        }
        if ctrl {
            state[0x11] = 0x80;
        }
        if alt {
            state[0x12] = 0x80;
        }
        if caps {
            state[0x14] = 0x01;
        }
        let mut buf = [0u16; 16];
        let hkl_handle = HKL(hkl as *mut core::ffi::c_void);
        let ret = ToUnicodeEx(vk, scan, &state, &mut buf, 0, hkl_handle);
        if ret > 0 {
            // ret = number of UTF-16 code units written.
            String::from_utf16_lossy(&buf[..ret as usize])
        } else if ret == -1 {
            // Dead key returned — flush its internal buffer so the next call is
            // clean, then produce no character for this isolated key.
            let _ = ToUnicodeEx(0, 0, &state, &mut buf, 0, hkl_handle);
            String::new()
        } else {
            String::new()
        }
    }
}

// NOTE: hook initialisation has moved to `hook::init_hooks()`.
// The native SetWindowsHookExW callbacks (WH_KEYBOARD_LL + WH_MOUSE_LL)
// capture screenshots synchronously before dispatching clicks.

#[cfg(test)]
mod keyboard_tests {
    use super::*;

    #[test]
    fn letter_keys_lowercase() {
        assert_eq!(key_to_char("KeyA"), "a");
        assert_eq!(key_to_char("KeyZ"), "z");
        assert_eq!(key_to_char("KeyM"), "m");
    }

    #[test]
    fn digit_keys_passthrough() {
        assert_eq!(key_to_char("Key0"), "0");
        assert_eq!(key_to_char("Key9"), "9");
    }

    #[test]
    fn sentinel_keys() {
        assert_eq!(key_to_char("KeySpace"), " ");
        assert_eq!(key_to_char("Return"), "\n");
        assert_eq!(key_to_char("KeyEnter"), "\n");
        assert_eq!(key_to_char("KeyTab"), "\t");
        assert_eq!(key_to_char("Backspace"), "\u{0008}");
        assert_eq!(key_to_char("Back"), "\u{0008}");
    }

    #[test]
    fn punctuation_keys() {
        assert_eq!(key_to_char("Comma"), ",");
        assert_eq!(key_to_char("Period"), ".");
        assert_eq!(key_to_char("SemiColon"), ";");
        assert_eq!(key_to_char("Slash"), "/");
        assert_eq!(key_to_char("BackSlash"), "\\");
        assert_eq!(key_to_char("Minus"), "-");
        assert_eq!(key_to_char("Equal"), "=");
        assert_eq!(key_to_char("LeftBracket"), "[");
        assert_eq!(key_to_char("RightBracket"), "]");
        assert_eq!(key_to_char("Quote"), "'");
        assert_eq!(key_to_char("Apostrophe"), "'");
        assert_eq!(key_to_char("Grave"), "`");
    }

    #[test]
    fn modifier_keys_empty() {
        for k in [
            "ShiftLeft",
            "ShiftRight",
            "ControlLeft",
            "Alt",
            "MetaLeft",
            "Num0",
            "NumLock",
            "CapsLock",
            "Fn1",
            "Super",
        ] {
            assert_eq!(key_to_char(k), "", "modifier {} should be empty", k);
        }
    }

    #[test]
    fn unknown_keys_bracket_format() {
        assert_eq!(key_to_char("F1"), "[F1]");
        assert_eq!(key_to_char("Escape"), "[Escape]");
        assert_eq!(key_to_char("LeftArrow"), "[LeftArrow]");
    }

    // ── to_unicode_char (Scribe-aligned actual-character capture) ──────────

    /// Grab the current thread's keyboard layout for tests.
    fn test_hkl() -> usize {
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
            GetKeyboardLayout(0).0 as usize
        }
    }

    #[test]
    fn to_unicode_modifier_keys_empty() {
        let h = test_hkl();
        assert_eq!(to_unicode_char(0x10, 0x2A, false, false, false, h), "");
        assert_eq!(to_unicode_char(0x11, 0x1D, false, false, false, h), "");
        assert_eq!(to_unicode_char(0x12, 0x38, false, false, false, h), "");
    }

    #[test]
    fn to_unicode_nav_keys_empty() {
        let h = test_hkl();
        assert_eq!(to_unicode_char(0x0D, 0x1C, false, false, false, h), ""); // Enter
        assert_eq!(to_unicode_char(0x09, 0x0F, false, false, false, h), ""); // Tab
        assert_eq!(to_unicode_char(0x1B, 0x01, false, false, false, h), ""); // Esc
    }

    #[test]
    fn to_unicode_digit_is_layout_independent() {
        let h = test_hkl();
        // Bare digit → itself (no shift). Digits are identical across layouts.
        assert_eq!(to_unicode_char(0x38, 0x09, false, false, false, h), "8");
        assert_eq!(to_unicode_char(0x31, 0x02, false, false, false, h), "1");
    }

    #[test]
    fn to_unicode_shift_8_is_star() {
        let h = test_hkl();
        // The reported bug: Shift+8 must be "*", not "8".
        assert_eq!(to_unicode_char(0x38, 0x09, true, false, false, h), "*");
    }

    #[test]
    fn to_unicode_shift_equals_is_plus() {
        let h = test_hkl();
        assert_eq!(to_unicode_char(0xBB, 0x0D, true, false, false, h), "+");
    }

    #[test]
    fn to_unicode_letter_case_follows_shift_and_caps() {
        let h = test_hkl();
        assert_eq!(to_unicode_char(0x41, 0x1E, false, false, false, h), "a");
        assert_eq!(to_unicode_char(0x41, 0x1E, true, false, false, h), "A");
    }

    #[test]
    fn to_unicode_fkey_empty() {
        let h = test_hkl();
        assert_eq!(to_unicode_char(0x70, 0x3B, false, false, false, h), "");
    }
}
