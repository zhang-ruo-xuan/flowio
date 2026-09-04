#[derive(Debug, Clone)]
pub enum MouseEvent {
    Click { x: f64, y: f64, button: String },
}

/// Whether a captured mouse event is the button press or release.
///
/// The recorder only hooked `WM_*BUTTONDOWN` originally, so drags collapsed
/// into a single "click" at the press point and the crop missed the whole
/// dragged region. Capturing both phases lets the event thread pair a
/// down+up and, when the two points differ, treat the gesture as a *drag*
/// and crop the bounding box of the two points instead of the press point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickPhase {
    Down,
    Up,
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
    /// Press (`Down`) or release (`Up`). Both phases are captured so drag
    /// gestures can be detected (see `ClickPhase` doc).
    pub phase: ClickPhase,
    /// Instant captured inside the hook callback, before the click is dispatched.
    pub timestamp: std::time::Instant,
    /// Foreground window PID captured inside the hook callback at click time.
    /// Used for self-filter / target-filter so filtering decisions are based on
    /// the foreground window at the moment of the click, not at processing time.
    pub foreground_pid: u32,
}

// NOTE: mouse hook initialization has been moved into
// `hook::init_hooks()` which uses native SetWindowsHookExW
// for both WH_KEYBOARD_LL + WH_MOUSE_LL.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_click_preserves_fields() {
        let now = std::time::Instant::now();
        let click = CapturedClick {
            x: 12.5,
            y: 88.25,
            button: "left".into(),
            phase: ClickPhase::Down,
            timestamp: now,
            foreground_pid: 1234,
        };
        assert_eq!(click.x, 12.5);
        assert_eq!(click.y, 88.25);
        assert_eq!(click.button, "left");
        assert_eq!(click.foreground_pid, 1234);
        assert_eq!(click.phase, ClickPhase::Down);
        assert!(click.timestamp <= std::time::Instant::now());
    }

    #[test]
    fn captured_click_is_clone_and_debug() {
        let now = std::time::Instant::now();
        let a = CapturedClick {
            x: 1.0,
            y: 2.0,
            button: "right".into(),
            phase: ClickPhase::Up,
            timestamp: now,
            foreground_pid: 0,
        };
        let b = a.clone();
        assert_eq!(a.button, b.button);
        assert_eq!(a.x, b.x);
        let _ = format!("{:?}", a);
    }

    #[test]
    fn mouse_event_click_matches_fields() {
        let ev = MouseEvent::Click {
            x: 320.0,
            y: 240.0,
            button: "middle".into(),
        };
        match ev {
            MouseEvent::Click { x, y, button } => {
                assert_eq!(x, 320.0);
                assert_eq!(y, 240.0);
                assert_eq!(button, "middle");
            }
        }
    }

    #[test]
    fn mouse_event_is_clone_and_debug() {
        let ev = MouseEvent::Click {
            x: -5.0,
            y: 0.0,
            button: "left".into(),
        };
        let ev2 = ev.clone();
        let _ = format!("{:?}", ev2);
    }
}
