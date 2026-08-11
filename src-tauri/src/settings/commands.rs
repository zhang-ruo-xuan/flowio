use tauri::State;

use crate::DbState;

// ── Hotkey configuration ──────────────────────────────────────

#[tauri::command]
pub fn get_hotkey_config(db: State<DbState>) -> Result<serde_json::Value, String> {
    todo!()
}

#[tauri::command]
pub fn set_hotkey_config(db: State<DbState>, config: serde_json::Value) -> Result<(), String> {
    todo!()
}

// ── Appearance configuration ──────────────────────────────────

#[tauri::command]
pub fn get_appearance_config(db: State<DbState>) -> Result<serde_json::Value, String> {
    todo!()
}

#[tauri::command]
pub fn set_appearance_config(db: State<DbState>, config: serde_json::Value) -> Result<(), String> {
    todo!()
}
