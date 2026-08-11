pub mod commands;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSave {
    pub order_index: i32,
    pub step_number: i32,
    pub title: String,
    pub description: String,
    pub action_type: String,
    pub tip: String,
    pub screenshot_path: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub before_screenshot_path: String,
    pub after_screenshot_path: Option<String>,
    pub timestamp: i64,
    pub window_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSave {
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub steps: Vec<StepSave>,
}

/// Save a completed recording and its steps into the database in a single transaction.
pub fn save_recording_to_db(conn: &Connection, recording: &RecordingSave) -> Result<(), String> {
    conn.execute("BEGIN", [])
        .map_err(|e| format!("BEGIN: {}", e))?;

    if let Err(e) = conn.execute(
        "INSERT INTO recordings (id, title, app_name, status, total_steps) VALUES (?1, ?2, ?3, '', ?4)",
        params![recording.id, recording.title, recording.app_name, recording.steps.len() as i32],
    ) {
        let _ = conn.execute("ROLLBACK", []);
        return Err(format!("Insert recording failed: {}", e));
    }

    for step in &recording.steps {
        let step_id = uuid::Uuid::new_v4().to_string();
        if let Err(e) = conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, \
             title, description, tip, screenshot_path, before_screenshot_path, x, y, after_screenshot_path, timestamp, window_title) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                step_id,
                recording.id,
                step.order_index,
                step.step_number,
                step.action_type,
                step.title,
                step.description,
                step.tip,
                step.screenshot_path,
                step.before_screenshot_path,
                step.x,
                step.y,
                step.after_screenshot_path,
                step.timestamp,
                step.window_title,
            ],
        ) {
            let _ = conn.execute("ROLLBACK", []);
            return Err(format!("Insert step failed: {}", e));
        }
    }

    conn.execute("COMMIT", [])
        .map_err(|e| format!("COMMIT: {}", e))?;

    Ok(())
}

pub fn init_db(conn: &Connection) {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS recordings (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL DEFAULT '',
            app_name    TEXT NOT NULL DEFAULT '',
            status      TEXT NOT NULL DEFAULT 'idle',
            total_steps INTEGER NOT NULL DEFAULT 0,
            duration_secs INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS steps (
            id                      TEXT PRIMARY KEY,
            recording_id            TEXT NOT NULL,
            order_index             INTEGER NOT NULL DEFAULT 0,
            step_number             INTEGER NOT NULL DEFAULT 0,
            action_type             TEXT NOT NULL DEFAULT '',
            title                   TEXT NOT NULL DEFAULT '',
            description             TEXT NOT NULL DEFAULT '',
            tip                     TEXT NOT NULL DEFAULT '',
            screenshot_path         TEXT NOT NULL DEFAULT '',
            annotated_path          TEXT NOT NULL DEFAULT '',
            before_screenshot_path  TEXT NOT NULL DEFAULT '',
            created_at              TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS api_keys (
            id            TEXT PRIMARY KEY,
            provider      TEXT NOT NULL DEFAULT '',
            key_encrypted TEXT NOT NULL DEFAULT '',
            is_active     INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )
    .expect("Failed to create database tables");

    // ── Migrations: add columns that may not exist in older DBs ──
    for col in &["x", "y", "after_screenshot_path", "timestamp", "window_title"] {
        // rusqlite ALTER TABLE … ADD COLUMN ignores duplicates via
        // a no-op when the column already exists (SQLite >= 3.35.0).
        let sql = format!(
            "ALTER TABLE steps ADD COLUMN {} {}",
            col,
            if *col == "timestamp" {
                "INTEGER NOT NULL DEFAULT 0"
            } else if col.starts_with("after") || *col == "window_title" {
                "TEXT"
            } else {
                "INTEGER"
            }
        );
        let _ = conn.execute_batch(&sql);
    }
}

/// Open (or create) the database file and run migrations.
pub fn init_db_file() -> Connection {
    let conn = Connection::open("flowio.db").expect("Failed to open database");
    init_db(&conn);
    conn
}

pub fn init_db_managed() -> Connection {
    init_db_file()
}
