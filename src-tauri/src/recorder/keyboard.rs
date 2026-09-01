#[derive(Debug, Clone)]
pub enum KeyEvent {
    /// Key press with modifier state and the wall-clock Instant at which the
    /// hook fired (used to pick the frame closest to the real key moment when
    /// recording a shortcut step).
    Press {
        key: String,
        ctrl: bool,
        shift: bool,
        alt: bool,
        ts: std::time::Instant,
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
}
