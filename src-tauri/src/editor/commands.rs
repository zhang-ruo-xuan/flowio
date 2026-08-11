use base64::Engine;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::DbState;

// ── Data types ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub status: String,
    pub total_steps: i32,
    pub duration_secs: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub recording_id: String,
    pub order_index: i32,
    pub step_number: i32,
    pub action_type: String,
    pub title: String,
    pub description: String,
    pub tip: String,
    pub screenshot_path: String,
    pub annotated_path: String,
    pub before_screenshot_path: String,
    pub created_at: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub after_screenshot_path: Option<String>,
    pub timestamp: i64,
    pub screenshot_base64: Option<String>,
    pub after_screenshot_base64: Option<String>,
    pub before_screenshot_base64: Option<String>,
    pub window_title: String,
}

#[derive(Debug, Serialize)]
pub struct RecordingWithSteps {
    pub recording: Recording,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
pub struct StepData {
    pub order_index: i32,
    pub step_number: i32,
    pub title: String,
    pub description: String,
    pub action_type: String,
    pub tip: String,
    pub screenshot_path: String,
    #[serde(default)]
    pub window_title: String,
}

// ── Tauri Commands ────────────────────────────────────────────

/// Load a recording and all its steps from the database.
#[tauri::command]
pub fn load_recording(db: State<DbState>, id: String) -> Result<RecordingWithSteps, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let recording = conn
        .query_row(
            "SELECT id, title, app_name, status, total_steps, duration_secs, \
             created_at, updated_at FROM recordings WHERE id = ?1",
            params![id],
            |row| {
                Ok(Recording {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    total_steps: row.get(4)?,
                    duration_secs: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| format!("Recording not found: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, recording_id, order_index, step_number, action_type, \
             title, description, tip, screenshot_path, annotated_path, \
             before_screenshot_path, created_at, \
             x, y, after_screenshot_path, timestamp, window_title \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![id], |row| {
            Ok(Step {
                id: row.get(0)?,
                recording_id: row.get(1)?,
                order_index: row.get(2)?,
                step_number: row.get(3)?,
                action_type: row.get(4)?,
                title: row.get(5)?,
                description: row.get(6)?,
                tip: row.get(7)?,
                screenshot_path: row.get(8)?,
                annotated_path: row.get(9)?,
                before_screenshot_path: row.get(10)?,
                created_at: row.get(11)?,
                x: row.get(12)?,
                y: row.get(13)?,
                after_screenshot_path: row.get(14)?,
                timestamp: row.get(15)?,
                screenshot_base64: None,
                after_screenshot_base64: None,
                before_screenshot_base64: None,
                window_title: row.get(16)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut steps = Vec::new();
    for row in rows {
        steps.push(row.map_err(|e| e.to_string())?);
    }

    // Convert screenshot files to base64 data URLs
    for step in &mut steps {
        if !step.screenshot_path.is_empty() {
            if let Ok(bytes) = std::fs::read(&step.screenshot_path) {
                step.screenshot_base64 = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
            }
        }
        if !step.before_screenshot_path.is_empty() {
            if let Ok(bytes) = std::fs::read(&step.before_screenshot_path) {
                step.before_screenshot_base64 = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
            }
        }
        if let Some(ref path) = step.after_screenshot_path {
            if !path.is_empty() {
                if let Ok(bytes) = std::fs::read(path) {
                    step.after_screenshot_base64 = Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
                }
            }
        }
    }

    Ok(RecordingWithSteps { recording, steps })
}

/// Update a single step's editable fields.
#[tauri::command]
pub fn update_step(
    db: State<DbState>,
    step_id: String,
    title: String,
    description: String,
    action_type: String,
    tip: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE steps SET title = ?1, description = ?2, action_type = ?3, tip = ?4 \
         WHERE id = ?5",
        params![title, description, action_type, tip, step_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a single step by its ID.
#[tauri::command]
pub fn delete_step(db: State<DbState>, step_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM steps WHERE id = ?1", params![step_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Batch-update the order_index of steps for a recording.
#[tauri::command]
pub fn reorder_steps(
    db: State<DbState>,
    _recording_id: String,
    step_ids: Vec<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    for (i, step_id) in step_ids.iter().enumerate() {
        conn.execute(
            "UPDATE steps SET order_index = ?1 WHERE id = ?2",
            params![i as i32, step_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Insert a new step at the specified order_index.
#[tauri::command]
pub fn add_step(
    db: State<DbState>,
    recording_id: String,
    order_index: i32,
    step_data: StepData,
) -> Result<Step, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE steps SET order_index = order_index + 1 \
         WHERE recording_id = ?1 AND order_index >= ?2",
        params![recording_id, order_index],
    )
    .map_err(|e| e.to_string())?;

    let step_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();

    conn.execute(
        "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, \
         title, description, tip, screenshot_path, created_at, timestamp, window_title) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            step_id,
            recording_id,
            order_index,
            step_data.step_number,
            step_data.action_type,
            step_data.title,
            step_data.description,
            step_data.tip,
            step_data.screenshot_path,
            now,
            now_ms,
            step_data.window_title,
        ],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE recordings SET total_steps = total_steps + 1, \
         updated_at = datetime('now') WHERE id = ?1",
        params![recording_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(Step {
        id: step_id,
        recording_id: recording_id.clone(),
        order_index,
        step_number: step_data.step_number,
        action_type: step_data.action_type,
        title: step_data.title,
        description: step_data.description,
        tip: step_data.tip,
        screenshot_path: step_data.screenshot_path,
        annotated_path: String::new(),
        before_screenshot_path: String::new(),
        created_at: now,
        x: None,
        y: None,
        after_screenshot_path: None,
        timestamp: now_ms,
        screenshot_base64: None,
        after_screenshot_base64: None,
        before_screenshot_base64: None,
        window_title: step_data.window_title,
    })
}

/// Update the recording title.
#[tauri::command]
pub fn update_recording_title(
    db: State<DbState>,
    id: String,
    title: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE recordings SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![title, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Crop a step's screenshot and save the cropped version to disk.
/// `is_after`: true = crop the after_screenshot, false = crop the main screenshot.
#[tauri::command]
pub fn crop_step_screenshot(
    db: State<DbState>,
    step_id: String,
    cropped_base64: String,
    is_after: bool,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Decode base64 → JPEG bytes
    let jpeg_bytes = base64::engine::general_purpose::STANDARD
        .decode(&cropped_base64)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    // Determine which path column to update
    let path_col = if is_after { "screenshot_path" } else { "before_screenshot_path" };

    // Get current path
    let current_path: String = conn
        .query_row(
            &format!("SELECT {} FROM steps WHERE id = ?1", path_col),
            params![step_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Step not found: {}", e))?;

    // Generate new path with _cropped suffix
    let current = std::path::Path::new(&current_path);
    let stem = current.file_stem().unwrap_or_default().to_string_lossy();
    let parent = current.parent().unwrap_or(std::path::Path::new("."));
    let new_path = parent.join(format!("{}_cropped.jpg", stem));

    // Write cropped JPEG
    std::fs::write(&new_path, &jpeg_bytes).map_err(|e| format!("Failed to write cropped image: {}", e))?;

    let new_path_str = new_path.to_string_lossy().to_string();

    // Update DB
    conn.execute(
        &format!("UPDATE steps SET {} = ?1 WHERE id = ?2", path_col),
        params![new_path_str, step_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(new_path_str)
}

/// Upload a screenshot (base64 JPEG) for a step, save to disk and update DB.
/// `is_after`: true = save as after_screenshot, false = save as main screenshot.
/// `is_marked`: true = save as marked/annotated screenshot.
#[tauri::command]
pub fn upload_step_screenshot(
    db: State<DbState>,
    step_id: String,
    base64_data: String,
    is_after: bool,
    is_marked: bool,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Decode base64 → JPEG bytes
    let jpeg_bytes = base64::engine::general_purpose::STANDARD
        .decode(&base64_data)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    // Generate new screenshot path
    let prefix = if is_marked {
        "marked"
    } else if is_after {
        "after"
    } else {
        "upload"
    };
    let new_path = crate::recorder::screenshot::next_screenshot_path(prefix);
    let new_path_str = new_path.to_string_lossy().to_string();

    // Write JPEG to disk
    std::fs::write(&new_path, &jpeg_bytes)
        .map_err(|e| format!("Failed to write uploaded image: {}", e))?;

    // Determine which DB column to update
    let path_col = if is_marked {
        "annotated_path"
    } else if is_after {
        "after_screenshot_path"
    } else {
        "screenshot_path"
    };

    // Update DB
    conn.execute(
        &format!("UPDATE steps SET {} = ?1 WHERE id = ?2", path_col),
        params![new_path_str, step_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(new_path_str)
}

/// Delete a step's screenshot: remove files from disk and clear DB fields.
#[tauri::command]
pub fn delete_step_screenshot(
    db: State<DbState>,
    step_id: String,
    is_after: bool,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    if is_after {
        // Delete "after" screenshot (screenshot_path) only
        let screenshot_path: String = conn
            .query_row(
                "SELECT screenshot_path FROM steps WHERE id = ?1",
                params![step_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        if !screenshot_path.is_empty() {
            std::fs::remove_file(&screenshot_path).ok();
        }

        conn.execute(
            "UPDATE steps SET screenshot_path = '' WHERE id = ?1",
            params![step_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        // Delete "before" screenshot (before_screenshot_path) only
        let before_screenshot_path: String = conn
            .query_row(
                "SELECT before_screenshot_path FROM steps WHERE id = ?1",
                params![step_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        if !before_screenshot_path.is_empty() {
            std::fs::remove_file(&before_screenshot_path).ok();
        }

        conn.execute(
            "UPDATE steps SET before_screenshot_path = '' WHERE id = ?1",
            params![step_id],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}
