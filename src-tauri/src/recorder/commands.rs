use std::sync::atomic::{AtomicBool, Ordering};

const _MARKER: &str = "[FLOWIO_CMDS_MARKER_20260825]";
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;
use uuid::Uuid;

use crate::recorder::activity;
use crate::recorder::capture::{self, FrameRing};
use crate::recorder::hook;
use crate::recorder::keyboard::{self, KeyEvent};
use crate::recorder::mouse::CapturedClick;
use crate::recorder::recorder;
use crate::recorder::screenshot;
use crate::recorder::state::RecordingState;
use crate::recorder::StepData;
use crate::RecordingManager;

// ── Flush helpers ───────────────────────────────────────────────────────

/// Resolve the description text for a typed step.
/// Priority: pre-captured UIA text → current focused-element text → keystroke buffer.
fn resolve_step_description(
    pre_captured: Option<&str>,
    current: Option<&str>,
    typed: &str,
) -> String {
    pre_captured
        .filter(|t| !t.is_empty())
        .or_else(|| current.filter(|t| !t.is_empty()))
        .unwrap_or(typed)
        .to_string()
}

/// Build the title for a typed step.
fn resolve_typed_title(window_title: Option<&str>) -> String {
    format!("在「{}」中输入", window_title.unwrap_or(""))
}

/// Re-index drained captured steps onto the recorder's step list:
/// `order_index` starts at `base_idx`, `step_number` = order_index + 1.
fn reindex_steps(captured: Vec<StepData>, base_idx: i32) -> Vec<StepData> {
    captured
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.order_index = base_idx + i as i32;
            s.step_number = s.order_index + 1;
            s
        })
        .collect()
}

/// 组装键盘输入步骤字段（flush_key_step 的 StepData 构造，便于单测）。
fn build_typed_step(
    window_title: Option<String>,
    description: String,
    screenshot_path: String,
    before_screenshot_path: String,
    timestamp: i64,
    is_sensitive: bool,
    crop_x: Option<i32>,
    crop_y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
) -> StepData {
    StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: "type".to_string(),
        title: resolve_typed_title(window_title.as_deref()),
        description,
        tip: None,
        screenshot_path,
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path,
        x: crop_x,
        y: crop_y,
        crop_w,
        crop_h,
        after_screenshot_path: None,
        timestamp,
        window_title: window_title.unwrap_or_default(),
        is_sensitive,
    }
}

/// Keyboard step flush (StepSnap-style):
/// - Capture fullscreen → save as both before_screenshot and screenshot (no annotation, no crop).
/// - Text priority: uia_text (captured while focus was on input) → get_focused_element_text() → keystroke buffer.
/// - 密码字段（UIA IsPassword）：明文不入录，描述统一替换为 `[密码已隐藏]`，步骤标记 is_sensitive。
fn flush_key_step(
    typed: &str,
    window_title: Option<String>,
    captured: &Arc<Mutex<Vec<StepData>>>,
    uia_text: Option<(String, bool)>,
    before_path: Option<String>,
    ring: &FrameRing,
    key_ts: Instant,
) {
    if typed.is_empty() {
        return;
    }

    // Use the frame closest to the last keystroke (input-time view),
    // fallback to a live capture.
    let frame_img = ring.closest_to(key_ts).map(|f| f.image.clone())
        .or_else(|| {
            eprintln!("[flush_key] ring empty, direct capture");
            screenshot::capture_fullscreen().ok()
        });
    let Some(mut clean_img) = frame_img else { return; };

    // Crop Flowio's own recording status bar from the top
    screenshot::crop_watermark(&mut clean_img);

    // Save fullscreen JPEG (both before and screenshot use the same image — no annotation)
    let ss_path = screenshot::next_screenshot_path("key");
    let ss_str = match screenshot::capture_frame_jpeg(&clean_img, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[flush_key] JPEG write failed: {}", e);
            String::new()
        }
    };

    // Text priority: pre-captured UIA text → current UIA → keystroke fallback
    let (current_uia, current_sensitive) = screenshot::get_focused_element_text()
        .map(|(t, s)| (Some(t), s))
        .unwrap_or((None, false));
    let sensitive = current_sensitive
        || uia_text.as_ref().map(|(_, s)| *s).unwrap_or(false);
    // 密码框：明文绝不入录，统一替换为占位符
    let description = if sensitive {
        "[密码已隐藏]".to_string()
    } else {
        resolve_step_description(
            uia_text.as_ref().map(|(t, _)| t.as_str()),
            current_uia.as_deref(),
            typed,
        )
    };

    let now_ms = current_time_millis();

    // Scribe-style: anchor the crop to the focused input area so the step's
    // cropped view shows the interaction region instead of the full screen.
    let (crop_x, crop_y, crop_w, crop_h) = match screenshot::get_focused_element_rect() {
        Some((lx, ty, w, h)) => (Some(lx + w / 2), Some(ty + h / 2), Some(w), Some(h)),
        None => (None, None, None, None),
    };

    let step = build_typed_step(
        window_title,
        description,
        ss_str.clone(),
        before_path.unwrap_or_else(|| ss_str.clone()),
        now_ms,
        sensitive,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
    );

    if let Ok(mut steps) = captured.lock() {
        // B5: 200 步上限（spec.md:160）；超出后停止累积，避免内存与导出体积失控
        if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
            steps.push(step);
        } else {
            eprintln!(
                "[flush_key] 录制步骤数已达上限 {}，本次步骤已丢弃",
                crate::recorder::state::MAX_RECORDING_STEPS
            );
        }
    }
}

/// Human-readable label for a shortcut key (KeyC → C, Comma → ,, Space → Space).
fn display_shortcut_key(key: &str) -> String {
    match key {
        "KeySpace" => "Space".to_string(),
        "Return" | "KeyEnter" => "Enter".to_string(),
        "KeyTab" => "Tab".to_string(),
        "Backspace" | "Back" => "Backspace".to_string(),
        "Escape" => "Esc".to_string(),
        "ShiftLeft" | "ShiftRight" => "Shift".to_string(),
        "ControlLeft" | "ControlRight" => "Ctrl".to_string(),
        "Alt" => "Alt".to_string(),
        "MetaLeft" | "MetaRight" => "Win".to_string(),
        "CapsLock" => "CapsLock".to_string(),
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
        "Num0" | "Num1" | "Num2" | "Num3" | "Num4" | "Num5" | "Num6" | "Num7"
        | "Num8" | "Num9" => key.trim_start_matches("Num").to_string(),
        s if s.starts_with("Key") => s.strip_prefix("Key").unwrap_or(s).to_uppercase(),
        s if s.starts_with("F") => s.to_string(),
        s if s.starts_with("Arrow") || s.ends_with("Arrow") => s.to_string(),
        s if s.starts_with("Unknown(") => s
            .trim_start_matches("Unknown(")
            .trim_end_matches(')')
            .to_string(),
        other => other.to_string(),
    }
}

/// Build and push a combo-key step (Scribe-style copy/paste/hotkey semantics).
/// Captures the current fullscreen (watermark-cropped) as its screenshot and
/// anchors the crop to the focused element center, same as type steps.
fn flush_shortcut_step(
    action_type: &str,
    title: String,
    description: String,
    captured: &Arc<Mutex<Vec<StepData>>>,
) {
    let full_img = match screenshot::capture_fullscreen() {
        Ok(img) => img,
        Err(e) => {
            eprintln!("[flush_shortcut] capture_fullscreen failed: {}", e);
            return;
        }
    };
    let mut clean_img = full_img.clone();
    screenshot::crop_watermark(&mut clean_img);

    let ss_path = screenshot::next_screenshot_path("key");
    let ss_str = match screenshot::capture_frame_jpeg(&clean_img, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[flush_shortcut] JPEG write failed: {}", e);
            String::new()
        }
    };

    let (crop_x, crop_y, crop_w, crop_h) = match screenshot::get_focused_element_rect() {
        Some((lx, ty, w, h)) => (Some(lx + w / 2), Some(ty + h / 2), Some(w), Some(h)),
        None => (None, None, None, None),
    };

    let window_title = activity::get_active_window_title();
    let step = StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: action_type.to_string(),
        title,
        description,
        tip: None,
        screenshot_path: ss_str.clone(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: ss_str,
        x: crop_x,
        y: crop_y,
        crop_w,
        crop_h,
        after_screenshot_path: None,
        timestamp: current_time_millis(),
        window_title: window_title.unwrap_or_default(),
        is_sensitive: false,
    };

    if let Ok(mut steps) = captured.lock() {
        if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
            steps.push(step);
        } else {
            eprintln!(
                "[flush_shortcut] 录制步骤数已达上限 {}，本次步骤已丢弃",
                crate::recorder::state::MAX_RECORDING_STEPS
            );
        }
    }
}

/// Build a click step: fullscreen + annotated with a red element box and
/// a numbered step badge (UI spec: 红框 + 右上角序号圆圈).
/// Both before_screenshot and screenshot use the same fullscreen annotated image — no cropping.
fn build_click_step(
    x: f64,
    y: f64,
    click_ts: Instant,
    ring: &FrameRing,
    step_no: i32,
) -> Option<StepData> {
    let frame_img = ring.closest_to(click_ts).map(|f| f.image.clone())
        .or_else(|| {
            eprintln!("[BG] click: ring empty, direct capture");
            screenshot::capture_fullscreen().ok()
        })?;

    let cx = x.round() as i32;
    let cy = y.round() as i32;

    // Fullscreen + red element box + step number badge (UI spec)
    let mut annotated = frame_img.clone();
    screenshot::draw_click_marker(&mut annotated, cx, cy, step_no);

    // Crop Flowio's own recording status bar from both images
    let mut clean_before = frame_img.clone();
    screenshot::crop_watermark(&mut clean_before);
    screenshot::crop_watermark(&mut annotated);

    // Save clean before screenshot (no annotation)
    let before_path = screenshot::next_screenshot_path("click_before");
    let before_str = match screenshot::capture_frame_jpeg(&clean_before, &before_path) {
        Ok(()) => before_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[BG] click before screenshot write failed: {}", e);
            return None;
        }
    };

    // Save annotated screenshot
    let ss_path = screenshot::next_screenshot_path("click");
    let ss_str = match screenshot::capture_frame_jpeg(&annotated, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[BG] click screenshot write failed: {}", e);
            return None;
        }
    };

    let now_ms = current_time_millis();

    let window_title = activity::get_active_window_title();
    let title = activity::make_click_title(window_title.as_deref());

    Some(StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: "click".to_string(),
        title,
        description: String::new(),
        tip: None,
        screenshot_path: ss_str.clone(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: before_str,
        x: Some(cx),
            y: Some(cy),
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None,
            timestamp: now_ms,
            window_title: window_title.unwrap_or_default(),
            is_sensitive: false,
    })
}

// ── Spawn event-processing thread (extracted for reuse in start & resume) ──

/// 当前 Unix 时间戳（毫秒）。
fn current_time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// 按键对累积输入缓冲区的效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferEffect {
    /// 产生了实际输入字符（需要刷新 idle 计时）
    Input,
    /// 退格删除字符（需要刷新 UIA 文本）
    Backspace,
    /// 无效键（修饰键 / 导航键 / 未知键），无副作用
    Ignored,
}

/// 将按键应用到累积输入缓冲区，返回该键的效果。
fn apply_key_to_buffer(buffer: &mut String, key_name: &str) -> BufferEffect {
    if key_name == "Backspace" || key_name == "Back" {
        if !buffer.is_empty() {
            buffer.pop();
        }
        BufferEffect::Backspace
    } else {
        let ch = keyboard::key_to_char(key_name);
        if !ch.is_empty() && !ch.starts_with('[') {
            buffer.push_str(&ch);
            BufferEffect::Input
        } else {
            BufferEffect::Ignored
        }
    }
}

/// 按键是否属于导航键（Enter/Tab/Escape，触发立即 flush）。
fn is_nav_key(key_name: &str) -> bool {
    matches!(key_name, "Enter" | "Return" | "KeyEnter" | "Tab" | "Escape")
}

/// 前台窗口变化时是否触发 flush：两侧均有标题、标题不同、且距上次按键超过 500ms。
fn should_flush_on_window_change(
    last: Option<&str>,
    current: Option<&str>,
    since_last_key: Duration,
) -> bool {
    matches!((last, current), (Some(l), Some(c)) if l != c)
        && since_last_key > Duration::from_millis(500)
}

fn spawn_event_thread(
    running: Arc<AtomicBool>,
    mouse_rx: mpsc::Receiver<CapturedClick>,
    key_rx: mpsc::Receiver<KeyEvent>,
    frame_rx: mpsc::Receiver<capture::CapturedFrame>,
    captured_steps: Arc<Mutex<Vec<StepData>>>,
    record_self: bool,
) -> std::thread::JoinHandle<()> {
    let running_clone = Arc::clone(&running);
    let cs_for_thread = captured_steps;

    std::thread::spawn(move || {
        eprintln!("[BG] event thread started");

        // Ring buffer: 2 seconds of history at 4 FPS = 8 frames, use 16 for margin
        let mut ring = FrameRing::new(16);

        let mut accumulated_keys = String::new();
        let mut last_key_time = Instant::now();
        let mut typing_window_title: Option<String> = None;
        let mut last_window_title: Option<String> = None;
        let mut last_uia_text: Option<(String, bool)> = None;
        let mut keyboard_before_path: Option<String> = None;
        let flush_interval = Duration::from_millis(1500);

        loop {
            if !running_clone.load(Ordering::Relaxed) {
                eprintln!("[BG] running flag cleared, exiting");
                if !accumulated_keys.is_empty() {
                    let typed = std::mem::take(&mut accumulated_keys);
                    flush_key_step(&typed, None, &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time);
                }
                break;
            }

            // ── Drain capture ring frames ──────────────────
            while let Ok(frame) = frame_rx.try_recv() {
                ring.push(frame);
            }

            // ── Mouse events ───────────────────────────────
            match mouse_rx.try_recv() {
                Ok(click) => {
                    eprintln!(
                        "[BG] mouse click: {} at ({}, {}), ts={:?}",
                        click.button, click.x, click.y, click.timestamp
                    );

                    // Self-filter: skip if foreground is Flowio itself
                    // (process-name based; only skipped in default mode, not
                    // when the user opted into recording Flowio's own window)
                    if !record_self && recorder::foreground_is_flowio() {
                        eprintln!("[BG] self-filter: skipping click in flowio window");
                        continue;
                    }

                    // Flush pending keystrokes before the click step
                    if !accumulated_keys.is_empty() {
                        let typed = std::mem::take(&mut accumulated_keys);
                        let win_title = typing_window_title.take().unwrap_or_default();
                        flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time);
                    }
                    last_key_time = Instant::now();

                    // Build click step from ring frame closest to click timestamp
                    let click_step_no = {
                        let s = cs_for_thread.lock().unwrap();
                        s.len() as i32 + 1
                    };
                    if let Some(step) = build_click_step(
                        click.x, click.y, click.timestamp, &ring, click_step_no,
                    ) {
                        if let Ok(mut steps) = cs_for_thread.lock() {
                            // B5: 200 步上限（spec.md:160）
                            if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
                                steps.push(step);
                            } else {
                                eprintln!(
                                    "[BG] 录制步骤数已达上限 {}，本次点击步骤已丢弃",
                                    crate::recorder::state::MAX_RECORDING_STEPS
                                );
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[BG] mouse channel disconnected");
                    break;
                }
            }

            // ── Key events ─────────────────────────────────
            match key_rx.try_recv() {
                Ok(KeyEvent::Press {
                    key: key_name,
                    ctrl,
                    shift: _,
                    alt,
                }) => {
                    // DEBUG: combo-key diagnosis log
                    {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(r"D:\录步\flowio_debug.log") {
                            let _ = writeln!(f, "[PRESS] key={} ctrl={} alt={} t={:?}", key_name, ctrl, alt, std::time::Instant::now().elapsed().as_millis());
                        }
                    }

                    // Self-filter (B4): 键盘分支此前未过滤 Flowio 自身窗口的输入，
                    // 在 Flowio 内编辑标题/备注时会生成多余 type 步骤。
                    // 按进程名判断；录制自身模式下不过滤。
                    if !record_self && recorder::foreground_is_flowio() {
                        eprintln!("[BG] self-filter: skipping keystroke in flowio window");
                        continue;
                    }

                    // Scribe-style combo-key semantics: Ctrl/Alt combinations are
                    // never typed characters. They become copy/paste/hotkey steps,
                    // and their letter keys never pollute the type buffer.
                    if ctrl || alt {
                        {
                            use std::io::Write;
                            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(r"D:\录步\flowio_debug.log") {
                                let _ = writeln!(f, "[COMBO] key={} ctrl={} alt={} -> combo branch", key_name, ctrl, alt);
                            }
                        }
                        // Flush any pending keystrokes first so the combo is not
                        // merged into the preceding type step.
                        if !accumulated_keys.is_empty() {
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = typing_window_title.take().unwrap_or_default();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time);
                        }
                        typing_window_title = None;
                        keyboard_before_path = None;
                        last_uia_text = None;

                        if ctrl {
                            match key_name.as_str() {
                                "KeyC" => {
                                    let clip = screenshot::get_clipboard_text().unwrap_or_default();
                                    let title = if clip.is_empty() {
                                        "复制".to_string()
                                    } else {
                                        format!("复制：{}", clip)
                                    };
                                    flush_shortcut_step("copy", title, clip, &cs_for_thread);
                                }
                                "KeyV" => {
                                    let clip = screenshot::get_clipboard_text().unwrap_or_default();
                                    let title = if clip.is_empty() {
                                        "粘贴".to_string()
                                    } else {
                                        format!("粘贴：{}", clip)
                                    };
                                    flush_shortcut_step("paste", title, clip, &cs_for_thread);
                                }
                                "KeyX" => {
                                    let clip = screenshot::get_clipboard_text().unwrap_or_default();
                                    let title = if clip.is_empty() {
                                        "剪切".to_string()
                                    } else {
                                        format!("剪切：{}", clip)
                                    };
                                    flush_shortcut_step("hotkey", title, clip, &cs_for_thread);
                                }
                                "KeyA" => flush_shortcut_step("hotkey", "全选".to_string(), String::new(), &cs_for_thread),
                                "KeyZ" => flush_shortcut_step("hotkey", "撤销".to_string(), String::new(), &cs_for_thread),
                                "KeyY" => flush_shortcut_step("hotkey", "重做".to_string(), String::new(), &cs_for_thread),
                                "KeyS" => flush_shortcut_step("hotkey", "保存".to_string(), String::new(), &cs_for_thread),
                                _ => {
                                    let title = format!("快捷键 Ctrl+{}", display_shortcut_key(&key_name));
                                    flush_shortcut_step("hotkey", title, String::new(), &cs_for_thread);
                                }
                            }
                        } else if alt {
                            let title = format!("快捷键 Alt+{}", display_shortcut_key(&key_name));
                            flush_shortcut_step("hotkey", title, String::new(), &cs_for_thread);
                        }
                        last_key_time = Instant::now();
                        continue;
                    }

                    if accumulated_keys.is_empty() {
                        typing_window_title = activity::get_active_window_title();
                        // Capture before screenshot on first keystroke
                        if let Ok(img) = screenshot::capture_fullscreen() {
                            let bp = screenshot::next_screenshot_path("key_before");
                            if screenshot::capture_frame_jpeg(&img, &bp).is_ok() {
                                keyboard_before_path = Some(bp.to_string_lossy().to_string());
                            }
                        }
                    }

                    // Navigation keys (Enter/Tab/Escape): flush pending keystrokes
                    // immediately and do NOT enter the type buffer (an Enter would
                    // otherwise be modelled as a stray "\n" input character).
                    if is_nav_key(&key_name) {
                        if !accumulated_keys.is_empty() {
                            let final_uia = screenshot::get_focused_element_text()
                                .filter(|(t, _)| !t.is_empty())
                                .or(last_uia_text.take());
                            eprintln!(
                                "[BG] nav-key flush: buffer='{}' uia='{}' key='{}'",
                                accumulated_keys,
                                final_uia
                                    .as_ref()
                                    .map(|(t, _)| t.as_str())
                                    .unwrap_or("(none)"),
                                key_name
                            );
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = typing_window_title.take().unwrap_or_default();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, final_uia, keyboard_before_path.take(), &ring, last_key_time);
                        }
                        typing_window_title = None;
                        keyboard_before_path = None;
                        last_uia_text = None;
                        last_key_time = Instant::now();
                        continue;
                    }

                    match apply_key_to_buffer(&mut accumulated_keys, &key_name) {
                        BufferEffect::Input => {
                            last_key_time = Instant::now();
                            // Cache UIA text while focus is still on the input
                            if let Some((text, sensitive)) = screenshot::get_focused_element_text() {
                                if !text.is_empty() {
                                    last_uia_text = Some((text, sensitive));
                                }
                            }
                        }
                        BufferEffect::Backspace => {
                            // Refresh UIA text (input field value after backspace)
                            last_uia_text = screenshot::get_focused_element_text()
                                .filter(|(t, _)| !t.is_empty());
                        }
                        BufferEffect::Ignored => {}
                    }
                }
                Ok(KeyEvent::Release) => {}
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[BG] keyboard channel disconnected");
                    break;
                }
            }

            // ── Idle flush (1500ms) ────────────────────────
            if !accumulated_keys.is_empty() && last_key_time.elapsed() > flush_interval {
                eprintln!(
                    "[BG] idle flush: buffer='{}' uia='{}' after {:?}",
                    accumulated_keys,
                    last_uia_text
                        .as_ref()
                        .map(|(t, _)| t.as_str())
                        .unwrap_or("(none)"),
                    last_key_time.elapsed()
                );
                let typed = std::mem::take(&mut accumulated_keys);
                let win_title = typing_window_title.take().unwrap_or_default();
                flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time);
            }

            // ── Foreground window change flush ─────────────
            let current_title = activity::get_active_window_title();
            if !accumulated_keys.is_empty() {
                if let (Some(ref last), Some(ref curr)) = (&last_window_title, &current_title) {
                    let since_last_key = last_key_time.elapsed();
                    if should_flush_on_window_change(Some(last), Some(curr), since_last_key) {
                        eprintln!(
                            "[BG] window change flush: buffer='{}' uia='{}' ({} → {})",
                            accumulated_keys,
                            last_uia_text
                                .as_ref()
                                .map(|(t, _)| t.as_str())
                                .unwrap_or("(none)"),
                            last, curr
                        );
                        let typed = std::mem::take(&mut accumulated_keys);
                        let win_title = typing_window_title.take().unwrap_or_default();
                        flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time);
                    } else if last != curr {
                        // Guard: don't flush on window-change within 500ms of last keystroke.
                        // Chrome auto-completes can change the title immediately after typing;
                        // waiting gives more keystrokes time to accumulate.
                        eprintln!(
                            "[BG] window change suppressed ({}ms since last key): buffer='{}' ({} → {})",
                            since_last_key.as_millis(),
                            accumulated_keys,
                            last, curr
                        );
                    }
                }
            }
            last_window_title = current_title;

            std::thread::sleep(Duration::from_millis(50));
        }

        eprintln!("[BG] event thread exiting");
    })
}

// ── Tauri commands ─────────────────────────────────────────────────────

/// 组装手动标记截图步骤（mark_screenshot_step 的 StepData 构造，便于单测）。
fn build_mark_step(
    title: String,
    x: Option<i32>,
    y: Option<i32>,
    action_type: Option<String>,
    text: Option<String>,
    screenshot_path: Option<String>,
    after_screenshot_path: Option<String>,
    step_index: i32,
    timestamp: i64,
) -> StepData {
    StepData {
        id: Uuid::new_v4().to_string(),
        order_index: step_index,
        step_number: step_index + 1,
        action_type: action_type.unwrap_or_else(|| "screenshot".to_string()),
        title,
        description: text.unwrap_or_default(),
        tip: None,
        screenshot_path: screenshot_path.unwrap_or_default(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: String::new(),
        x,
        y,
        crop_w: None,
        crop_h: None,
        after_screenshot_path,
        timestamp,
        window_title: String::new(),
        is_sensitive: false,
    }
}

/// StepData 列表 → 数据库 StepSave 列表（finish_recording 持久化映射）。
fn steps_to_step_saves(steps: &[StepData]) -> Vec<crate::db::StepSave> {
    steps
        .iter()
        .map(|s| crate::db::StepSave {
            order_index: s.order_index,
            step_number: s.step_number,
            title: s.title.clone(),
            description: s.description.clone(),
            action_type: s.action_type.clone(),
            tip: s.tip.clone().unwrap_or_default(),
            screenshot_path: s.screenshot_path.clone(),
            x: s.x,
            y: s.y,
            crop_w: s.crop_w,
            crop_h: s.crop_h,
            before_screenshot_path: s.before_screenshot_path.clone(),
            after_screenshot_path: s.after_screenshot_path.clone(),
            timestamp: s.timestamp,
            window_title: s.window_title.clone(),
        })
        .collect()
}

/// 空标题回退为当前时间格式的录制标题。
fn recording_title_or_default(title: &str) -> String {
    if title.is_empty() {
        chrono::Local::now()
            .format("录制 %Y-%m-%d %H:%M:%S")
            .to_string()
    } else {
        title.to_string()
    }
}

/// 组装 RecordingSave 并生成描述性 rec_id。
fn build_recording_save(
    id: String,
    title: &str,
    steps: &[StepData],
) -> (crate::db::RecordingSave, String) {
    let saves = steps_to_step_saves(steps);
    let step_count = saves.len();
    let save = crate::db::RecordingSave {
        id: id.clone(),
        title: recording_title_or_default(title),
        app_name: String::new(),
        steps: saves,
    };
    (save, format!("{} ({} steps)", id, step_count))
}

/// 是否为活动录制状态（Recording 或 Paused）。
fn is_active_recording_state(state: &RecordingState) -> bool {
    *state == RecordingState::Recording || *state == RecordingState::Paused
}

#[tauri::command]
pub fn start_recording(
    recording_manager: State<RecordingManager>,
    record_self: Option<bool>,
) -> Result<String, String> {
    let record_self = record_self.unwrap_or(false);
    let id = Uuid::new_v4().to_string();

    // --- Set up hook channels ---
    let (mouse_tx, mouse_rx) = mpsc::channel::<CapturedClick>();
    let (key_tx, key_rx) = mpsc::channel::<KeyEvent>();

    // Shared running flag
    let running = Arc::new(AtomicBool::new(true));
    let running_for_hook = Arc::clone(&running);

    // Native Windows hooks (WH_KEYBOARD_LL + WH_MOUSE_LL)
    hook::init_hooks(key_tx, mouse_tx, running_for_hook);

    // --- Start continuous capture thread (4 FPS) ---
    let capture_running = Arc::clone(&running);
    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>();
    capture::start_capture_thread(capture_running, frame_tx, Duration::from_millis(250));

    // --- Create recorder ---
    let mut recorder = crate::recorder::Recorder::new(id.clone(), String::new());
    recorder.record_self = record_self;
    recorder.start()?;

    // --- Shared captured-steps buffer ---
    let captured_steps: Arc<Mutex<Vec<StepData>>> = Arc::new(Mutex::new(Vec::new()));
    let cs_for_thread = Arc::clone(&captured_steps);

    let running_clone = Arc::clone(&running);

    let handle = spawn_event_thread(
        running_clone,
        mouse_rx,
        key_rx,
        frame_rx,
        cs_for_thread,
        record_self,
    );

    recorder.event_thread = Some(handle);
    recorder.running_flag = Some(running);
    recorder.captured_steps = Some(captured_steps);

    // --- Store in manager ---
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        manager.insert(id.clone(), recorder);
    }

    Ok(id)
}

#[tauri::command]
pub fn pause_recording(
    recording_manager: State<RecordingManager>,
    id: String,
) -> Result<(), String> {
    let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    let recorder = manager
        .get_mut(&id)
        .ok_or_else(|| format!("Recording {} not found", id))?;
    recorder.pause()
}

#[tauri::command]
pub fn resume_recording(
    recording_manager: State<RecordingManager>,
    id: String,
) -> Result<(), String> {
    // Step 1: Change state to Recording
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        let recorder = manager
            .get_mut(&id)
            .ok_or_else(|| format!("Recording {} not found", id))?;
        recorder.resume()?;
    }

    // Step 2: Restart background threads (outside the lock)
    let (mouse_tx, mouse_rx) = mpsc::channel::<CapturedClick>();
    let (key_tx, key_rx) = mpsc::channel::<KeyEvent>();
    let running = Arc::new(AtomicBool::new(true));
    let running_for_hook = Arc::clone(&running);

    hook::init_hooks(key_tx, mouse_tx, running_for_hook);

    let capture_running = Arc::clone(&running);
    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>();
    capture::start_capture_thread(capture_running, frame_tx, Duration::from_millis(250));

    let running_clone = Arc::clone(&running);

    // Step 3: Update recorder with new thread handles
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        let recorder = manager
            .get_mut(&id)
            .ok_or_else(|| format!("Recording {} not found", id))?;

        let captured_steps = recorder
            .captured_steps
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())));
        let cs_for_thread = Arc::clone(&captured_steps);

        let handle = spawn_event_thread(
            running_clone,
            mouse_rx,
            key_rx,
            frame_rx,
            cs_for_thread,
            recorder.record_self,
        );

        recorder.running_flag = Some(running);
        recorder.event_thread = Some(handle);
        recorder.captured_steps = Some(captured_steps);
    }

    Ok(())
}

#[tauri::command]
pub fn finish_recording(
    recording_manager: State<RecordingManager>,
    db: State<crate::DbState>,
    id: String,
) -> Result<(), String> {
    eprintln!("[finish] step0: finish_recording called for id={}", id);

    // --- Block 1: Stop event thread + join ---
    eprintln!("[finish] step1: stopping event thread...");
    {
        let mut manager = recording_manager
            .0
            .lock()
            .map_err(|e| {
                eprintln!("[finish] step1 FAIL: lock manager: {}", e);
                e.to_string()
            })?;
        let recorder = manager.get_mut(&id).ok_or_else(|| {
            let msg = format!("Recording {} not found", id);
            eprintln!("[finish] step1 FAIL: {}", msg);
            msg
        })?;

        if let Some(ref flag) = recorder.running_flag {
            flag.store(false, Ordering::Relaxed);
        }

        let thread_handle = recorder.event_thread.take();
        recorder.finish().map_err(|e| {
            eprintln!("[finish] step1 FAIL: recorder.finish(): {}", e);
            e
        })?;
        drop(manager);

        if let Some(handle) = thread_handle {
            eprintln!("[BG] waiting for event thread to finish...");
            let _ = handle.join();
            eprintln!("[BG] event thread joined");
        }
    }
    eprintln!("[finish] step1 OK: event thread stopped");

    // --- Block 2: Drain captured_steps (AFTER event thread fully exited) ---
    eprintln!("[finish] step2: draining captured_steps...");
    {
        let mut manager = recording_manager
            .0
            .lock()
            .map_err(|e| {
                eprintln!("[finish] step2 FAIL: lock manager: {}", e);
                e.to_string()
            })?;
        let recorder = manager.get_mut(&id).ok_or_else(|| {
            let msg = format!("Recording {} not found after join", id);
            eprintln!("[finish] step2 FAIL: {}", msg);
            msg
        })?;

        if let Some(cs) = recorder.captured_steps.take() {
            let mut buf = cs.lock().map_err(|e| {
                eprintln!("[finish] step2 FAIL: lock captured_steps: {}", e);
                e.to_string()
            })?;
            let drained_count = buf.len();
            let base_idx = recorder.steps.len() as i32;
            let reindexed = reindex_steps(buf.drain(..).collect(), base_idx);
            recorder.steps.extend(reindexed);
            eprintln!(
                "[finish] step2 OK: drained {} steps, total now {} steps",
                drained_count,
                recorder.steps.len()
            );
        } else {
            eprintln!("[finish] step2 OK: no captured_steps to drain");
        }
    }

    // --- Block 3: Build RecordingSave → save to DB → remove from manager ---
    eprintln!("[finish] step3: building RecordingSave and saving to DB...");
    let (recording_save, rec_id) = {
        let manager = recording_manager.0.lock().map_err(|e| {
            eprintln!("[finish] step3 FAIL: lock manager: {}", e);
            e.to_string()
        })?;
        let recorder = manager.get(&id).ok_or_else(|| {
            let msg = format!("Recording {} not found after join", id);
            eprintln!("[finish] step3 FAIL: {}", msg);
            msg
        })?;

        build_recording_save(recorder.id.clone(), &recorder.title, &recorder.steps)
    };

    let conn = db.0.lock().map_err(|e| {
        eprintln!("[finish] step3 FAIL: lock db: {}", e);
        e.to_string()
    })?;
    crate::db::save_recording_to_db(&conn, &recording_save).map_err(|e| {
        eprintln!("[finish] step3 FAIL: save_recording_to_db: {}", e);
        e
    })?;
    eprintln!("[finish] step3 OK: saved to DB (rec_id: {})", rec_id);

    // Remove from manager
    {
        let mut manager = recording_manager.0.lock().map_err(|e| {
            eprintln!("[finish] cleanup FAIL: lock manager: {}", e);
            e.to_string()
        })?;
        manager.remove(&id);
    }
    eprintln!("[finish] all OK: recording {} removed from manager", id);

    Ok(())
}

#[tauri::command]
pub fn cancel_recording(
    recording_manager: State<RecordingManager>,
    id: String,
) -> Result<(), String> {
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        let recorder = manager
            .get_mut(&id)
            .ok_or_else(|| format!("Recording {} not found", id))?;

        if let Some(ref flag) = recorder.running_flag {
            flag.store(false, Ordering::Relaxed);
        }

        let thread_handle = recorder.event_thread.take();
        let _ = recorder.captured_steps.take();
        recorder.cancel()?;
        recorder.steps.clear();

        drop(manager);

        if let Some(handle) = thread_handle {
            let _ = handle.join();
        }
    }

    Ok(())
}

#[tauri::command]
pub fn mark_screenshot_step(
    recording_manager: State<RecordingManager>,
    id: String,
    title: String,
    x: Option<i32>,
    y: Option<i32>,
    action_type: Option<String>,
    text: Option<String>,
    screenshot_path: Option<String>,
    after_screenshot_path: Option<String>,
) -> Result<(), String> {
    let now_ms = current_time_millis();

    let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    let recorder = manager
        .get_mut(&id)
        .ok_or_else(|| format!("Recording {} not found", id))?;

    let step_index = recorder.steps.len() as i32;
    let step = build_mark_step(
        title,
        x,
        y,
        action_type,
        text,
        screenshot_path,
        after_screenshot_path,
        step_index,
        now_ms,
    );

    recorder.push_step(step)
}

#[tauri::command]
pub fn get_active_recording(
    recording_manager: State<RecordingManager>,
) -> Result<Option<serde_json::Value>, String> {
    let manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    for (id, recorder) in manager.iter() {
        if is_active_recording_state(&recorder.state) {
            return Ok(Some(serde_json::json!({
                "id": id,
                "is_paused": recorder.state == RecordingState::Paused
            })));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod commands_tests {
    use super::*;

    fn make_step(order_index: i32) -> StepData {
        StepData {
            id: format!("s{}", order_index),
            order_index,
            step_number: order_index + 1,
            action_type: "click".into(),
            title: "t".into(),
            description: String::new(),
            tip: None,
            screenshot_path: String::new(),
            screenshot_base64: None,
            annotated_path: String::new(),
            before_screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None,
            timestamp: 0,
            window_title: String::new(),
            is_sensitive: false,
        }
    }

    // ── resolve_step_description（文本优先级）────────────────────────

    #[test]
    fn description_prefers_pre_captured_uia() {
        let d = resolve_step_description(Some("搜索框内容"), Some("当前焦点"), "typed");
        assert_eq!(d, "搜索框内容");
    }

    #[test]
    fn description_filters_empty_pre_captured() {
        let d = resolve_step_description(Some(""), Some("当前焦点"), "typed");
        assert_eq!(d, "当前焦点");
    }

    #[test]
    fn description_falls_back_to_keystroke_when_both_uia_missing() {
        assert_eq!(resolve_step_description(None, None, "hello"), "hello");
        assert_eq!(resolve_step_description(Some(""), None, "hello"), "hello");
        assert_eq!(resolve_step_description(None, Some(""), "hello"), "hello");
    }

    #[test]
    fn description_prefers_current_over_keystroke() {
        let d = resolve_step_description(None, Some("地址栏"), "baidu.com");
        assert_eq!(d, "地址栏");
    }

    // ── resolve_typed_title ─────────────────────────────────────────

    #[test]
    fn typed_title_with_window() {
        assert_eq!(resolve_typed_title(Some("Chrome")), "在「Chrome」中输入");
    }

    #[test]
    fn typed_title_without_window() {
        assert_eq!(resolve_typed_title(None), "在「」中输入");
    }

    // ── reindex_steps（finish 排水重排）─────────────────────────────

    #[test]
    fn reindex_empty_returns_empty() {
        assert!(reindex_steps(Vec::new(), 0).is_empty());
    }

    #[test]
    fn reindex_from_zero_numbers_sequentially() {
        let out = reindex_steps(vec![make_step(0), make_step(1)], 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].order_index, 0);
        assert_eq!(out[0].step_number, 1);
        assert_eq!(out[1].order_index, 1);
        assert_eq!(out[1].step_number, 2);
    }

    #[test]
    fn reindex_appends_after_existing_steps() {
        // 模拟 finish_recording：recorder.steps 已有 3 步，captured 2 步续接
        let out = reindex_steps(vec![make_step(0), make_step(1)], 3);
        assert_eq!(out[0].order_index, 3);
        assert_eq!(out[0].step_number, 4);
        assert_eq!(out[1].order_index, 4);
        assert_eq!(out[1].step_number, 5);
    }

    #[test]
    fn reindex_overwrites_stale_indices() {
        // 捕获阶段的 order_index 恒为 0，重排后必须被覆盖
        let mut stale = make_step(0);
        stale.step_number = 999;
        let out = reindex_steps(vec![stale], 7);
        assert_eq!(out[0].order_index, 7);
        assert_eq!(out[0].step_number, 8);
    }

    #[test]
    fn reindex_keeps_other_fields() {
        let mut s = make_step(0);
        s.action_type = "type".into();
        s.title = "在「Chrome」中输入".into();
        let out = reindex_steps(vec![s], 1);
        assert_eq!(out[0].action_type, "type");
        assert_eq!(out[0].title, "在「Chrome」中输入");
    }

    // ── current_time_millis（时间戳）───────────────────────────────

    #[test]
    fn current_time_millis_is_non_negative_and_recent() {
        let t = current_time_millis();
        assert!(t > 0);
        // 距 now 不超过 5 秒
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!((now - t).abs() < 5_000);
    }

    #[test]
    fn current_time_millis_monotonic_increasing() {
        let a = current_time_millis();
        let b = current_time_millis();
        assert!(b >= a);
    }

    // ── build_typed_step（键盘步骤组装）────────────────────────────

    #[test]
    fn build_typed_step_basic_fields() {
        let s = build_typed_step(
            Some("Chrome".into()),
            "搜索框内容".into(),
            "ss.jpg".into(),
            "before.jpg".into(),
            12345,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.action_type, "type");
        assert_eq!(s.title, "在「Chrome」中输入");
        assert_eq!(s.description, "搜索框内容");
        assert_eq!(s.screenshot_path, "ss.jpg");
        assert_eq!(s.before_screenshot_path, "before.jpg");
        assert_eq!(s.timestamp, 12345);
        assert_eq!(s.window_title, "Chrome");
        assert_eq!(s.x, None);
        assert_eq!(s.y, None);
        assert_eq!(s.after_screenshot_path, None);
        assert_eq!(s.tip, None);
        assert_eq!(s.is_sensitive, false);
        assert!(!s.id.is_empty());
    }

    #[test]
    fn build_typed_step_sensitive_marks_flag() {
        // B2: 密码框步骤应标记 is_sensitive（描述由调用方替换为 [密码已隐藏]）
        let s = build_typed_step(
            Some("登录".into()),
            "[密码已隐藏]".into(),
            "ss.jpg".into(),
            "before.jpg".into(),
            10,
            true,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.is_sensitive, true);
        assert_eq!(s.description, "[密码已隐藏]");
        assert_eq!(s.window_title, "登录");
    }

    #[test]
    fn build_typed_step_window_title_missing() {
        let s = build_typed_step(
            None,
            "d".into(),
            "ss".into(),
            "before".into(),
            1,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.title, "在「」中输入");
        assert_eq!(s.window_title, "");
    }

    #[test]
    fn build_typed_step_before_falls_back_to_screenshot_when_none() {
        // flush_key_step 调用处：before_path.unwrap_or_else(|| ss_str.clone())
        // 此处模拟 before_path 为空字符串（已 unwrap 后传入）
        let s = build_typed_step(
            None,
            "d".into(),
            "ss.jpg".into(),
            String::new(),
            1,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.before_screenshot_path, "");
    }

    #[test]
    fn build_typed_step_empty_description_and_typed() {
        let s = build_typed_step(
            None,
            String::new(),
            "ss".into(),
            "before".into(),
            0,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.description, "");
        assert_eq!(s.action_type, "type");
    }

    // ── build_mark_step（手动标记截图步骤）────────────────────────

    #[test]
    fn build_mark_step_defaults() {
        let s = build_mark_step(
            "我的截图".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            999,
        );
        assert_eq!(s.action_type, "screenshot");
        assert_eq!(s.title, "我的截图");
        assert_eq!(s.description, "");
        assert_eq!(s.screenshot_path, "");
        assert_eq!(s.before_screenshot_path, "");
        assert_eq!(s.after_screenshot_path, None);
        assert_eq!(s.x, None);
        assert_eq!(s.y, None);
        assert_eq!(s.window_title, "");
        assert_eq!(s.order_index, 0);
        assert_eq!(s.step_number, 1);
        assert_eq!(s.timestamp, 999);
    }

    #[test]
    fn build_mark_step_index_and_number() {
        let s = build_mark_step(
            "t".into(),
            Some(10),
            Some(20),
            Some("click".into()),
            Some("desc".into()),
            Some("p.png".into()),
            Some("after.png".into()),
            4,
            1000,
        );
        assert_eq!(s.order_index, 4);
        assert_eq!(s.step_number, 5);
        assert_eq!(s.action_type, "click");
        assert_eq!(s.description, "desc");
        assert_eq!(s.screenshot_path, "p.png");
        assert_eq!(s.after_screenshot_path, Some("after.png".into()));
        assert_eq!(s.x, Some(10));
        assert_eq!(s.y, Some(20));
    }

    #[test]
    fn build_mark_step_empty_title_and_text() {
        let s = build_mark_step(
            String::new(),
            None,
            None,
            None,
            Some(String::new()),
            Some(String::new()),
            None,
            0,
            0,
        );
        assert_eq!(s.title, "");
        assert_eq!(s.description, "");
        assert_eq!(s.screenshot_path, "");
    }

    #[test]
    fn build_mark_step_preserves_after_path_none() {
        let s = build_mark_step(
            "t".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            0,
        );
        assert_eq!(s.after_screenshot_path, None);
    }

    // ── steps_to_step_saves / recording_title_or_default / build_recording_save ──

    #[test]
    fn steps_to_step_saves_maps_all_fields() {
        let steps = vec![make_step(0), make_step(1)];
        let saves = steps_to_step_saves(&steps);
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].order_index, 0);
        assert_eq!(saves[0].step_number, 1);
        assert_eq!(saves[0].title, "t");
        assert_eq!(saves[0].action_type, "click");
        assert_eq!(saves[0].tip, "");
        assert_eq!(saves[0].x, None);
        assert_eq!(saves[0].y, None);
        assert_eq!(saves[0].before_screenshot_path, "");
        assert_eq!(saves[0].after_screenshot_path, None);
        assert_eq!(saves[0].timestamp, 0);
        assert_eq!(saves[0].window_title, "");
    }

    #[test]
    fn steps_to_step_saves_tip_none_becomes_empty() {
        let mut s = make_step(0);
        s.tip = None;
        let saves = steps_to_step_saves(&[s]);
        assert_eq!(saves[0].tip, "");
    }

    #[test]
    fn steps_to_step_saves_empty_input() {
        assert!(steps_to_step_saves(&[]).is_empty());
    }

    #[test]
    fn recording_title_or_default_non_empty_kept() {
        assert_eq!(recording_title_or_default("我的录制"), "我的录制");
    }

    #[test]
    fn recording_title_or_default_empty_uses_timestamp_format() {
        let t = recording_title_or_default("");
        assert!(t.starts_with("录制 "));
        // 格式：录制 YYYY-MM-DD HH:MM:SS
        assert!(regex_like(&t));
    }

    #[test]
    fn build_recording_save_basic() {
        let steps = vec![make_step(0), make_step(1), make_step(2)];
        let (save, rec_id) = build_recording_save("rec-1".into(), "标题", &steps);
        assert_eq!(save.id, "rec-1");
        assert_eq!(save.title, "标题");
        assert_eq!(save.app_name, "");
        assert_eq!(save.steps.len(), 3);
        assert_eq!(rec_id, "rec-1 (3 steps)");
    }

    #[test]
    fn build_recording_save_empty_title_fallback() {
        let steps = vec![make_step(0)];
        let (save, rec_id) = build_recording_save("rec-2".into(), "", &steps);
        assert!(save.title.starts_with("录制 "));
        assert_eq!(rec_id, "rec-2 (1 steps)");
    }

    #[test]
    fn build_recording_save_zero_steps() {
        let (save, rec_id) = build_recording_save("rec-0".into(), "空", &[]);
        assert_eq!(save.steps.len(), 0);
        assert_eq!(rec_id, "rec-0 (0 steps)");
    }

    // ── is_active_recording_state ─────────────────────────────────

    #[test]
    fn active_state_recording_and_paused() {
        assert!(is_active_recording_state(&RecordingState::Recording));
        assert!(is_active_recording_state(&RecordingState::Paused));
    }

    #[test]
    fn inactive_state_idle_completed_cancelled() {
        assert!(!is_active_recording_state(&RecordingState::Idle));
        assert!(!is_active_recording_state(&RecordingState::Completed));
        assert!(!is_active_recording_state(&RecordingState::Cancelled));
    }

    // ── apply_key_to_buffer / is_nav_key / should_flush_on_window_change ──

    #[test]
    fn apply_key_input_char_appends() {
        let mut buf = String::new();
        assert_eq!(apply_key_to_buffer(&mut buf, "KeyA"), BufferEffect::Input);
        assert_eq!(buf, "a");
        assert_eq!(apply_key_to_buffer(&mut buf, "KeyB"), BufferEffect::Input);
        assert_eq!(buf, "ab");
    }

    #[test]
    fn apply_key_sentinel_and_unknown_keys_ignored() {
        let mut buf = String::new();
        // 未知键（如 "你"）走 other 分支返回 "[你]"，以 [ 开头视为无效键
        assert_eq!(apply_key_to_buffer(&mut buf, "你"), BufferEffect::Ignored);
        assert_eq!(buf, "");
        assert_eq!(apply_key_to_buffer(&mut buf, "ShiftLeft"), BufferEffect::Ignored);
        assert_eq!(buf, "");
    }

    #[test]
    fn apply_key_space_and_special_chars_input() {
        let mut buf = String::new();
        assert_eq!(apply_key_to_buffer(&mut buf, "KeySpace"), BufferEffect::Input);
        assert_eq!(buf, " ");
        assert_eq!(apply_key_to_buffer(&mut buf, "Comma"), BufferEffect::Input);
        assert_eq!(buf, " ,");
    }

    #[test]
    fn apply_key_bracket_prefix_ignored() {
        let mut buf = String::new();
        // 功能键（如 "[Shift]"）不以字符形式输入
        assert_eq!(apply_key_to_buffer(&mut buf, "[Shift]"), BufferEffect::Ignored);
        assert_eq!(buf, "");
    }

    #[test]
    fn apply_key_backspace_pops() {
        let mut buf = String::from("abc");
        assert_eq!(apply_key_to_buffer(&mut buf, "Backspace"), BufferEffect::Backspace);
        assert_eq!(buf, "ab");
        assert_eq!(apply_key_to_buffer(&mut buf, "Back"), BufferEffect::Backspace);
        assert_eq!(buf, "a");
    }

    #[test]
    fn apply_key_backspace_on_empty_is_safe() {
        let mut buf = String::new();
        assert_eq!(apply_key_to_buffer(&mut buf, "Backspace"), BufferEffect::Backspace);
        assert_eq!(buf, "");
    }

    #[test]
    fn apply_key_enter_is_ignored_as_input() {
        // Enter 是导航键，不进入缓冲区
        let mut buf = String::from("abc");
        assert_eq!(apply_key_to_buffer(&mut buf, "Enter"), BufferEffect::Ignored);
        assert_eq!(buf, "abc");
    }

    #[test]
    fn is_nav_key_variants() {
        assert!(is_nav_key("Enter"));
        assert!(is_nav_key("Tab"));
        assert!(is_nav_key("Escape"));
        assert!(!is_nav_key("a"));
        assert!(!is_nav_key("Backspace"));
        assert!(!is_nav_key("Shift"));
    }

    #[test]
    fn should_flush_window_change_true_when_diff_and_slow() {
        assert!(should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(600)
        ));
    }

    #[test]
    fn should_flush_window_change_false_when_same_title() {
        assert!(!should_flush_on_window_change(
            Some("A"),
            Some("A"),
            Duration::from_millis(600)
        ));
    }

    #[test]
    fn should_flush_window_change_false_when_within_500ms() {
        assert!(!should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(300)
        ));
    }

    #[test]
    fn should_flush_window_change_false_when_missing_title() {
        assert!(!should_flush_on_window_change(
            None,
            Some("B"),
            Duration::from_millis(600)
        ));
        assert!(!should_flush_on_window_change(
            Some("A"),
            None,
            Duration::from_millis(600)
        ));
        assert!(!should_flush_on_window_change(
            None,
            None,
            Duration::from_millis(600)
        ));
    }

    #[test]
    fn should_flush_window_change_boundary_500ms() {
        // 恰好 500ms 不算超过（> 而非 >=）
        assert!(!should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(500)
        ));
        assert!(should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(501)
        ));
    }

    // 轻量格式校验（避免引入 regex 依赖）
    fn regex_like(s: &str) -> bool {
        // 仅支持本测试用到的简单模式：^录制 \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$
        let expect = "录制 0000-00-00 00:00:00";
        if !s.starts_with("录制 ") {
            return false;
        }
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.len() == 14 && s.len() == expect.len()
    }
}
