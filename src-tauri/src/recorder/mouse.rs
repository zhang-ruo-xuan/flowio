#[derive(Debug, Clone)]
pub enum MouseEvent {
    Click { x: f64, y: f64, button: String },
}

/// A click event captured by the native WH_MOUSE_LL hook.
///
/// The timestamp is recorded inside the hook callback BEFORE CallNextHookEx
/// dispatches the click. The event thread uses this timestamp to find the
/// pre-click frame from the continuous capture ring buffer.
#[derive(Debug, Clone)]
pub struct CapturedClick {
    pub x: f64,
    pub y: f64,
    pub button: String,
    /// Instant captured inside the hook callback, before the click is dispatched.
    pub timestamp: std::time::Instant,
}

// NOTE: mouse hook initialization has been moved into
// `hook::init_hooks()` which uses native SetWindowsHookExW
// for both WH_KEYBOARD_LL + WH_MOUSE_LL.
