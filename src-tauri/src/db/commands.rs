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
    pub ai_generated: bool,
    pub text_redacted: bool,
    pub scan_redacted: bool,
}

#[tauri::command]
pub fn list_recordings(db: State<DbState>) -> Result<Vec<RecordingSummary>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, title, app_name, status, total_steps, created_at, ai_generated, text_redacted, scan_redacted FROM recordings ORDER BY created_at DESC")
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
                ai_generated: row.get::<_, i64>(6)? != 0,
                text_redacted: row.get::<_, i64>(7)? != 0,
                scan_redacted: row.get::<_, i64>(8)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| e.to_string())?);
    }
    Ok(result)
}

/// Collect screenshot paths (main + optional after) from step rows,
/// preserving row order. Empty strings are kept; callers skip them
/// when deleting files from disk.
fn collect_step_screenshot_paths(rows: Vec<(String, Option<String>)>) -> Vec<String> {
    rows.into_iter()
        .flat_map(|(a, b)| {
            let mut v = vec![a];
            if let Some(b) = b { v.push(b); }
            v
        })
        .collect()
}

#[tauri::command]
pub fn delete_recording(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Collect screenshot paths before deleting rows
    let mut stmt = conn
        .prepare("SELECT screenshot_path, after_screenshot_path FROM steps WHERE recording_id = ?1")
        .map_err(|e| e.to_string())?;

    let rows: Vec<(String, Option<String>)> = stmt
        .query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, Option<String>>(1).unwrap_or(None),
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    let screenshot_paths = collect_step_screenshot_paths(rows);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{init_db, RecordingSave, StepSave};
    use rusqlite::Connection;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn);
        conn
    }

    fn step(num: i32, with_after: bool) -> StepSave {
        StepSave {
            order_index: num,
            step_number: num,
            title: format!("Step {}", num),
            description: "desc".into(),
            action_type: "click".into(),
            tip: "tip".into(),
            screenshot_path: format!("shot_{}.png", num),
            x: Some(num),
            y: Some(num * 2),
            crop_w: Some(num * 3),
            crop_h: Some(num * 4),
            before_screenshot_path: format!("before_{}.png", num),
            after_screenshot_path: if with_after {
                Some(format!("after_{}.png", num))
            } else {
                None
            },
            timestamp: 1_700_000_000_000 + num as i64,
            window_title: format!("Window {}", num),
        }
    }

    fn recording(id: &str, n: usize) -> RecordingSave {
        RecordingSave {
            id: id.into(),
            title: format!("Rec {}", id),
            app_name: "TestApp".into(),
            steps: (1..=n as i32).map(|i| step(i, i % 2 == 0)).collect(),
        }
    }

    // ---------- collect_step_screenshot_paths ----------

    #[test]
    fn collect_paths_preserves_order_and_includes_after() {
        let rows = vec![
            ("a.png".to_string(), Some("a_after.png".to_string())),
            ("b.png".to_string(), None),
        ];
        assert_eq!(
            collect_step_screenshot_paths(rows),
            vec!["a.png", "a_after.png", "b.png"]
        );
    }

    #[test]
    fn collect_paths_keeps_empty_main_skips_none_after() {
        let rows = vec![
            (String::new(), None),
            ("c.png".to_string(), Some("c_after.png".to_string())),
        ];
        let paths = collect_step_screenshot_paths(rows);
        // 空 main 保留（删除时跳过）；None after 不产出
        assert_eq!(paths, vec!["", "c.png", "c_after.png"]);
    }

    #[test]
    fn collect_paths_empty_input() {
        assert!(collect_step_screenshot_paths(vec![]).is_empty());
    }

    // ---------- list_recordings ----------

    #[test]
    fn list_recordings_empty_db() {
        let conn = mem_db();
        let mut stmt = conn
            .prepare("SELECT id, title, app_name, status, total_steps, created_at, ai_generated, text_redacted, scan_redacted FROM recordings ORDER BY created_at DESC")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok(RecordingSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    total_steps: row.get(4)?,
                    created_at: row.get(5)?,
                    ai_generated: row.get::<_, i64>(6)? != 0,
                    text_redacted: row.get::<_, i64>(7)? != 0,
                    scan_redacted: row.get::<_, i64>(8)? != 0,
                })
            })
            .unwrap();
        let result: Vec<RecordingSummary> = rows.collect::<Result<_, _>>().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_recordings_orders_by_created_at_desc() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO recordings (id, title, app_name, status, total_steps, created_at) \
             VALUES ('r1','T1','A','idle',2,'2026-08-19 10:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recordings (id, title, app_name, status, total_steps, created_at) \
             VALUES ('r2','T2','A','finished',5,'2026-08-20 09:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recordings (id, title, app_name, status, total_steps, created_at) \
             VALUES ('r3','T3','A','finished',0,'2026-08-20 10:00:00')",
            [],
        )
        .unwrap();

        let mut stmt = conn
            .prepare("SELECT id, title, app_name, status, total_steps, created_at, ai_generated, text_redacted, scan_redacted FROM recordings ORDER BY created_at DESC")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok(RecordingSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    app_name: row.get(2)?,
                    status: row.get(3)?,
                    total_steps: row.get(4)?,
                    created_at: row.get(5)?,
                    ai_generated: row.get::<_, i64>(6)? != 0,
                    text_redacted: row.get::<_, i64>(7)? != 0,
                    scan_redacted: row.get::<_, i64>(8)? != 0,
                })
            })
            .unwrap();
        let result: Vec<RecordingSummary> = rows.collect::<Result<_, _>>().unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].id, "r3");
        assert_eq!(result[1].id, "r2");
        assert_eq!(result[2].id, "r1");
        assert_eq!(result[1].total_steps, 5);
        assert_eq!(result[1].status, "finished");
    }

    // ---------- delete_recording ----------

    #[test]
    fn delete_recording_removes_steps_then_recording() {
        let conn = mem_db();
        let rec = recording("rec-del", 3);
        crate::db::save_recording_to_db(&conn, &rec).unwrap();

        let mut stmt = conn
            .prepare("SELECT screenshot_path, after_screenshot_path FROM steps WHERE recording_id = ?1")
            .unwrap();
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(params!["rec-del"], |row| {
                Ok((
                    row.get::<_, String>(0).unwrap_or_default(),
                    row.get::<_, Option<String>>(1).unwrap_or(None),
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        let paths = collect_step_screenshot_paths(rows);

        conn.execute("DELETE FROM steps WHERE recording_id = ?1", params!["rec-del"])
            .unwrap();
        conn.execute("DELETE FROM recordings WHERE id = ?1", params!["rec-del"])
            .unwrap();

        let step_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE recording_id = ?1",
                params!["rec-del"],
                |r| r.get(0),
            )
            .unwrap();
        let rec_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recordings WHERE id = ?1",
                params!["rec-del"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(step_count, 0);
        assert_eq!(rec_count, 0);
        // 3 个 step：奇数无 after、偶数有 after → 主截图 3 + after 1 = 4
        assert_eq!(paths.len(), 4);
        assert!(paths.iter().any(|p| p == "after_2.png"));
    }

    #[test]
    fn delete_recording_unknown_id_is_noop() {
        let conn = mem_db();
        conn.execute("DELETE FROM steps WHERE recording_id = ?1", params!["ghost"])
            .unwrap();
        conn.execute("DELETE FROM recordings WHERE id = ?1", params!["ghost"])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recordings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    // ---------- update_recording_app_name ----------

    #[test]
    fn update_app_name_changes_name_and_updated_at() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO recordings (id, title, app_name, status, total_steps) \
             VALUES ('r9','T9','OldApp','idle',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE recordings SET app_name = ?1, updated_at = datetime('now') WHERE id = ?2",
            params!["NewApp", "r9"],
        )
        .unwrap();

        let (name, updated): (String, String) = conn
            .query_row(
                "SELECT app_name, updated_at FROM recordings WHERE id = 'r9'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "NewApp");
        assert!(!updated.is_empty());
    }

    #[test]
    fn update_app_name_unknown_id_affects_zero_rows() {
        let conn = mem_db();
        let n = conn
            .execute(
                "UPDATE recordings SET app_name = ?1, updated_at = datetime('now') WHERE id = ?2",
                params!["X", "no-such-id"],
            )
            .unwrap();
        assert_eq!(n, 0);
    }
}
