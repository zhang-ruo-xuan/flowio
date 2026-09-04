//! IME composition detection via Windows IMM32 API.
//!
//! Detects whether an IME (Input Method Editor) is currently in
//! composition mode — e.g., user typing Pinyin for Chinese characters.
//! During composition, keystrokes should NOT trigger flush because
//! the user hasn't finalized their input yet.
//!
//! The principle: low-level keyboard hooks (WH_KEYBOARD_LL) intercept
//! raw key events before they reach the IME pipeline, so we cannot
//! rely on WM_IME_COMPOSITION window messages. Instead, we poll the
//! IME context directly via ImmGetCompositionStringW, which queries
//! the IME's current composition state regardless of hook layer.

use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, GCS_COMPSTR, GCS_RESULTSTR,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// A composition string length > 0 means an active IME composition is in
/// progress. Negative / zero lengths (including GCS error returns) mean
/// no active composition.
fn composition_active(composition_string_len: i32) -> bool {
    composition_string_len > 0
}

/// Check if an IME is actively composing (e.g., Pinyin input in progress).
///
/// Returns `true` if the foreground window has a non-empty composition string,
/// indicating the user is in the middle of IME input and keystrokes should be
/// accumulated rather than flushed.
pub fn is_ime_composing() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let himc = ImmGetContext(hwnd);
        if himc.0.is_null() {
            return false;
        }
        // Get composition string length (in bytes).
        // A length > 0 means there's an active composition in progress.
        // For Chinese Pinyin: non-empty when user is typing pinyin,
        // becomes empty when the candidate is selected (composition ends).
        let len = ImmGetCompositionStringW(himc, GCS_COMPSTR, None, 0);
        let _ = ImmReleaseContext(hwnd, himc);
        composition_active(len)
    }
}

/// Retrieve the final committed string of an IME composition (the Chinese
/// characters the user just finalised, e.g. "你好"), by reading
/// `GCS_RESULTSTR` from the foreground window's IME context.
///
/// Call this at the moment a composition *ends* (transition from composing to
/// not-composing) so the typed Chinese text can be appended to the type buffer
/// exactly like a sequence of real characters — matching Scribe's behaviour of
/// capturing the final input text rather than the intermediate pinyin.
///
/// Returns `None` when there is no IME context or no committed result.
pub fn get_composition_result() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let himc = ImmGetContext(hwnd);
        if himc.0.is_null() {
            return None;
        }
        // Byte length of the result string (UTF-16; every char is 2 bytes).
        let byte_len = ImmGetCompositionStringW(himc, GCS_RESULTSTR, None, 0);
        let result = if byte_len > 0 {
            let nchars = (byte_len as usize) / 2;
            let mut buf: Vec<u16> = vec![0u16; nchars];
            let got = ImmGetCompositionStringW(
                himc,
                GCS_RESULTSTR,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                byte_len as u32,
            );
            let _ = ImmReleaseContext(hwnd, himc);
            if got > 0 {
                Some(String::from_utf16_lossy(&buf[..(got as usize) / 2]))
            } else {
                None
            }
        } else {
            let _ = ImmReleaseContext(hwnd, himc);
            None
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::composition_active;

    #[test]
    fn positive_length_means_composing() {
        // 中文拼音组合串（UTF-16 字节数）非空即视为组合中
        assert!(composition_active(1));
        assert!(composition_active(1024));
    }

    #[test]
    fn zero_length_means_not_composing() {
        assert!(!composition_active(0));
    }

    #[test]
    fn negative_length_error_means_not_composing() {
        // ImmGetCompositionStringW 返回负值表示失败/无上下文
        assert!(!composition_active(-1));
        assert!(!composition_active(i32::MIN));
    }
}
