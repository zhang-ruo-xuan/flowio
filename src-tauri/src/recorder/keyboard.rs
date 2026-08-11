#[derive(Debug, Clone)]
pub enum KeyEvent {
    Press(String),
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
