use std::sync::atomic::{AtomicBool, Ordering};
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

/// Keyboard step flush (StepSnap-style):
/// - Capture fullscreen → save as both before_screenshot and screenshot (no annotation, no crop).
/// - Text priority: uia_text (captured while focus was on input) → get_focused_element_text() → keystroke buffer.
fn flush_key_step(
    typed: &str,
    window_title: Option<String>,
    captured: &Arc<Mutex<Vec<StepData>>>,
    uia_text: Option<String>,
    before_path: Option<String>,
) {
    if typed.is_empty() {
        return;
    }

    // Fullscreen capture
    let full_img = match screenshot::capture_fullscreen() {
        Ok(img) => img,
        Err(e) => {
            eprintln!("[flush_key] capture_fullscreen failed: {}", e);
            return;
        }
    };

    // Crop Flowio's own recording status bar from the top
    let mut clean_img = full_img.clone();
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
    let description = uia_text
        .filter(|t| !t.is_empty())
        .or_else(|| screenshot::get_focused_element_text().filter(|t| !t.is_empty()))
        .unwrap_or_else(|| typed.to_string());

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let step = StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: "type".to_string(),
        title: format!("在「{}」中输入", window_title.as_deref().unwrap_or("")),
        description,
        tip: None,
        screenshot_path: ss_str.clone(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: before_path.unwrap_or_else(|| ss_str.clone()),
        x: None,
        y: None,
        after_screenshot_path: None,
        timestamp: now_ms,
        window_title: window_title.unwrap_or_default(),
    };

    if let Ok(mut steps) = captured.lock() {
        steps.push(step);
    }
}

/// Build a click step (StepSnap-style): fullscreen + red circle annotation.
/// Both before_screenshot and screenshot use the same fullscreen annotated image — no cropping.
fn build_click_step(
    x: f64,
    y: f64,
    click_ts: Instant,
    ring: &FrameRing,
) -> Option<StepData> {
    let frame_img = ring.closest_to(click_ts).map(|f| f.image.clone())
        .or_else(|| {
            eprintln!("[BG] click: ring empty, direct capture");
            screenshot::capture_fullscreen().ok()
        })?;

    let cx = x.round() as i32;
    let cy = y.round() as i32;

    // Fullscreen + red circle annotation (StepSnap: R=20 hollow circle + R=3 center dot)
    let mut annotated = frame_img.clone();
    screenshot::draw_scribe_red_circle(&mut annotated, cx, cy);

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

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

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
        after_screenshot_path: None,
        timestamp: now_ms,
        window_title: window_title.unwrap_or_default(),
    })
}

// ── Spawn event-processing thread (extracted for reuse in start & resume) ──

fn spawn_event_thread(
    running: Arc<AtomicBool>,
    mouse_rx: mpsc::Receiver<CapturedClick>,
    key_rx: mpsc::Receiver<KeyEvent>,
    frame_rx: mpsc::Receiver<capture::CapturedFrame>,
    captured_steps: Arc<Mutex<Vec<StepData>>>,
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
        let mut last_uia_text: Option<String> = None;
        let mut keyboard_before_path: Option<String> = None;
        let flush_interval = Duration::from_millis(1500);

        loop {
            if !running_clone.load(Ordering::Relaxed) {
                eprintln!("[BG] running flag cleared, exiting");
                if !accumulated_keys.is_empty() {
                    let typed = std::mem::take(&mut accumulated_keys);
                    flush_key_step(&typed, None, &cs_for_thread, last_uia_text.take(), keyboard_before_path.take());
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
                    if let Some(title) = recorder::get_foreground_window_title() {
                        if recorder::is_flowio_app(&title) {
                            eprintln!("[BG] self-filter: skipping flowio window '{}'", title);
                            continue;
                        }
                    }

                    // Flush pending keystrokes before the click step
                    if !accumulated_keys.is_empty() {
                        let typed = std::mem::take(&mut accumulated_keys);
                        let win_title = typing_window_title.take().unwrap_or_default();
                        flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take());
                    }
                    last_key_time = Instant::now();

                    // Build click step from ring frame closest to click timestamp
                    if let Some(step) = build_click_step(
                        click.x, click.y, click.timestamp, &ring,
                    ) {
                        if let Ok(mut steps) = cs_for_thread.lock() {
                            steps.push(step);
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
                Ok(KeyEvent::Press(key_name)) => {
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

                    if key_name == "Backspace" || key_name == "Back" {
                        if !accumulated_keys.is_empty() {
                            accumulated_keys.pop();
                        }
                        // Refresh UIA text (input field value after backspace)
                        last_uia_text = screenshot::get_focused_element_text()
                            .filter(|t| !t.is_empty());
                    } else {
                        let ch = keyboard::key_to_char(&key_name);
                        if !ch.is_empty() && !ch.starts_with('[') {
                            accumulated_keys.push_str(&ch);
                            last_key_time = Instant::now();
                            // Cache UIA text while focus is still on the input
                            if let Some(uia) = screenshot::get_focused_element_text() {
                                if !uia.is_empty() {
                                    last_uia_text = Some(uia);
                                }
                            }
                        }
                    }

                    // Navigation keys: read UIA text BEFORE flushing (focus still on input)
                    if !accumulated_keys.is_empty()
                        && matches!(key_name.as_str(), "Enter" | "Tab" | "Escape")
                    {
                        // Final UIA read while focus is still on the address bar / input
                        let final_uia = screenshot::get_focused_element_text()
                            .filter(|t| !t.is_empty())
                            .or(last_uia_text.take());

                        eprintln!(
                            "[BG] nav-key flush: buffer='{}' uia='{}' key='{}'",
                            accumulated_keys,
                            final_uia.as_deref().unwrap_or("(none)"),
                            key_name
                        );
                        let typed = std::mem::take(&mut accumulated_keys);
                        let win_title = typing_window_title.take().unwrap_or_default();
                        flush_key_step(&typed, Some(win_title), &cs_for_thread, final_uia, keyboard_before_path.take());
                        last_key_time = Instant::now();
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
                    last_uia_text.as_deref().unwrap_or("(none)"),
                    last_key_time.elapsed()
                );
                let typed = std::mem::take(&mut accumulated_keys);
                let win_title = typing_window_title.take().unwrap_or_default();
                flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take());
            }

            // ── Foreground window change flush ─────────────
            let current_title = activity::get_active_window_title();
            if !accumulated_keys.is_empty() {
                if let (Some(ref last), Some(ref curr)) = (&last_window_title, &current_title) {
                    if last != curr {
                        // Guard: don't flush on window-change within 500ms of last keystroke.
                        // Chrome auto-completes can change the title immediately after typing;
                        // waiting gives more keystrokes time to accumulate.
                        let since_last_key = last_key_time.elapsed();
                        if since_last_key > Duration::from_millis(500) {
                            eprintln!(
                                "[BG] window change flush: buffer='{}' uia='{}' ({} → {})",
                                accumulated_keys,
                                last_uia_text.as_deref().unwrap_or("(none)"),
                                last, curr
                            );
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = typing_window_title.take().unwrap_or_default();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take());
                        } else {
                            eprintln!(
                                "[BG] window change suppressed ({}ms since last key): buffer='{}' ({} → {})",
                                since_last_key.as_millis(),
                                accumulated_keys,
                                last, curr
                            );
                        }
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

#[tauri::command]
pub fn start_recording(recording_manager: State<RecordingManager>) -> Result<String, String> {
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
    recorder.start()?;

    // --- Shared captured-steps buffer ---
    let captured_steps: Arc<Mutex<Vec<StepData>>> = Arc::new(Mutex::new(Vec::new()));
    let cs_for_thread = Arc::clone(&captured_steps);

    let running_clone = Arc::clone(&running);

    let handle = spawn_event_thread(running_clone, mouse_rx, key_rx, frame_rx, cs_for_thread);

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

        let handle = spawn_event_thread(running_clone, mouse_rx, key_rx, frame_rx, cs_for_thread);

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
            for (i, mut s) in buf.drain(..).enumerate() {
                s.order_index = base_idx + i as i32;
                s.step_number = s.order_index + 1;
                recorder.steps.push(s);
            }
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

        let steps: Vec<crate::db::StepSave> = recorder
            .steps
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
                before_screenshot_path: s.before_screenshot_path.clone(),
                after_screenshot_path: s.after_screenshot_path.clone(),
                timestamp: s.timestamp,
                window_title: s.window_title.clone(),
            })
            .collect();

        let step_count = steps.len();

        let title = if recorder.title.is_empty() {
            chrono::Local::now()
                .format("录制 %Y-%m-%d %H:%M:%S")
                .to_string()
        } else {
            recorder.title.clone()
        };

        let save = crate::db::RecordingSave {
            id: recorder.id.clone(),
            title,
            app_name: String::new(),
            steps,
        };

        (save, format!("{} ({} steps)", recorder.id.clone(), step_count))
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
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    let recorder = manager
        .get_mut(&id)
        .ok_or_else(|| format!("Recording {} not found", id))?;

    let step_index = recorder.steps.len() as i32;
    let step = StepData {
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
        after_screenshot_path,
        timestamp: now_ms,
        window_title: String::new(),
    };

    recorder.push_step(step)
}

#[tauri::command]
pub fn get_active_recording(
    recording_manager: State<RecordingManager>,
) -> Result<Option<serde_json::Value>, String> {
    let manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    for (id, recorder) in manager.iter() {
        if recorder.state == RecordingState::Recording || recorder.state == RecordingState::Paused {
            return Ok(Some(serde_json::json!({
                "id": id,
                "is_paused": recorder.state == RecordingState::Paused
            })));
        }
    }
    Ok(None)
}
