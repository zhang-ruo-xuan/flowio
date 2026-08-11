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
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, GCS_COMPSTR,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

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
        ImmReleaseContext(hwnd, himc);
        len > 0
    }
}
