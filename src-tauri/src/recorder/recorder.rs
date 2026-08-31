//! Core recording pipeline helpers:
//! - Self-filtering (skip Flowio's own windows)
//! - Foreground window title detection (Windows FFI)
//!
//! The actual event-processing loop lives in `commands.rs` and uses
//! these helpers together with `screenshot.rs`.

extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max_count: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
    fn QueryFullProcessImageNameW(
        h_process: isize,
        dw_flags: u32,
        lp_exe_name: *mut u16,
        lpdw_size: *mut u32,
    ) -> i32;
    fn CloseHandle(h_object: isize) -> i32;
}

/// Return `true` when the window title contains "flowio" or "录步".
/// Kept for tests/legacy; the runtime self-filter uses
/// [`foreground_is_flowio`] (process-name based) instead.
pub fn is_flowio_app(window_title: &str) -> bool {
    let lower = window_title.to_lowercase();
    lower.contains("flowio") || lower.contains("录步")
}

/// `PROCESS_QUERY_LIMITED_INFORMATION` (0x1000) is enough to read the image
/// name and works on un-elevated processes.
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

/// Return `true` when the foreground window belongs to the Flowio process
/// itself (`flowio.exe`), based on the process image name rather than the
/// window title.
///
/// This avoids self-filter false positives when the user records a tutorial
/// about "录步" itself (browser pages / other apps whose titles contain
/// "录步" or "flowio" would otherwise be skipped).
pub fn foreground_is_flowio() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return false;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc == 0 {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(proc);
        if ok == 0 {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        let name = path.rsplit(['\\', '/']).next().unwrap_or("");
        name.eq_ignore_ascii_case("flowio.exe")
    }
}

/// Retrieve the caption of the currently focused foreground window.
///
/// Uses raw Win32 FFI (`GetForegroundWindow` + `GetWindowTextW`) and
/// therefore only works on Windows.
pub fn get_foreground_window_title() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
        let actual = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if actual <= 0 {
            return None;
        }
        buf.truncate(actual as usize);
        Some(String::from_utf16_lossy(&buf))
    }
}

#[cfg(test)]
mod recorder_tests {
    use super::*;

    #[test]
    fn is_flowio_detects_variants() {
        assert!(is_flowio_app("Flowio - 录步"));
        assert!(is_flowio_app("flowio 编辑器"));
        assert!(is_flowio_app("录步 - 录制中"));
        assert!(is_flowio_app("FLOWIO"));
    }

    #[test]
    fn is_flowio_rejects_others() {
        assert!(!is_flowio_app("微信"));
        assert!(!is_flowio_app("Google Chrome"));
        assert!(!is_flowio_app("录屏助手")); // 含"录"但不含"录步"
    }

    #[test]
    fn is_flowio_empty_false() {
        assert!(!is_flowio_app(""));
    }
}
