use rusqlite::params;
use serde::Serialize;
use tauri::State;

use crate::db::RecordingSave;
use crate::DbState;

#[derive(Debug, Serialize)]
pub struct RecordingSummary {
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub status: String,
    pub total_steps: i32,
    pub created_at: String,
}

#[tauri::command]
pub fn list_recordings(db: State<DbState>) -> Result<Vec<RecordingSummary>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, title, app_name, status, total_steps, created_at FROM recordings ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(RecordingSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                app_name: row.get(2)?,
                status: row.get(3)?,
                total_steps: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| e.to_string())?);
    }
    Ok(result)
}

#[tauri::command]
pub fn delete_recording(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Collect screenshot paths before deleting rows
    let mut stmt = conn
        .prepare("SELECT screenshot_path, after_screenshot_path FROM steps WHERE recording_id = ?1")
        .map_err(|e| e.to_string())?;

    let screenshot_paths: Vec<String> = stmt
        .query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, Option<String>>(1).unwrap_or(None),
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .flat_map(|(a, b)| {
            let mut v = vec![a];
            if let Some(b) = b { v.push(b); }
            v
        })
        .collect();

    // Delete steps first (to be explicit, even though FK ON DELETE CASCADE may handle it)
    conn.execute("DELETE FROM steps WHERE recording_id = ?1", params![id])
        .map_err(|e| e.to_string())?;

    // Delete recording
    conn.execute("DELETE FROM recordings WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;

    // Delete screenshot files from disk
    for path in &screenshot_paths {
        if !path.is_empty() {
            let _ = std::fs::remove_file(path);
        }
    }

    Ok(())
}

#[tauri::command]
pub fn save_recording(
    db: State<DbState>,
    recording: RecordingSave,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::save_recording_to_db(&conn, &recording)
}

/// Update the recording app name.
#[tauri::command]
pub fn update_recording_app_name(
    db: State<DbState>,
    id: String,
    app_name: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE recordings SET app_name = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![app_name, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
