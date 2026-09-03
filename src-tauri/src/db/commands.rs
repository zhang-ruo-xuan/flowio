use rusqlite::params;
use serde::Serialize;
use tauri::State;

use crate::db::{RecordingSave, LOCAL_USER};
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

/// 读取 settings 表中的当前登录账号标记；空值/缺失视为未登录。
fn get_current_user(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'current_user'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|v| !v.is_empty())
}

/// 列表按登录态隔离：
/// - 已登录（settings.current_user 非空）→ 只返回该账号的记录
/// - 未登录（无 current_user 或为空）→ 只返回 __local__ 记录
/// 保证不重复、不漏。
fn list_recordings_inner(conn: &rusqlite::Connection) -> Result<Vec<RecordingSummary>, String> {
    let filter = get_current_user(conn).unwrap_or_else(|| LOCAL_USER.to_string());
    let mut stmt = conn
        .prepare("SELECT id, title, app_name, status, total_steps, created_at, ai_generated, text_redacted, scan_redacted FROM recordings WHERE user_id = ?1 ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![filter], |row| {
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

#[tauri::command]
pub fn list_recordings(db: State<DbState>) -> Result<Vec<RecordingSummary>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    list_recordings_inner(&conn)
}

/// 同步登录态到本地 DB：仅写 settings.current_user 作为列表隔离标记。
/// 不做任何批量迁移；user_id 为空字符串表示清除登录标记（退出登录）。
fn set_current_user_inner(conn: &rusqlite::Connection, user_id: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES ('current_user', ?1)",
        params![user_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_current_user(db: State<DbState>, user_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let res = set_current_user_inner(&conn, &user_id);
    // 调试日志：验证前端 invoke 是否真正到达后端并写入 DB（定位"登录没同步"）
    {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"D:\录步\flowio_debug.log")
        {
            let _ = writeln!(
                f,
                "[set_current_user] user_id={:?} result={:?}",
                user_id,
                res.as_ref().map(|_| "ok").map_err(|e| e.clone())
            );
        }
    }
    res
}

/// 将当前编辑的教程精确归属到登录账号。
/// - user_id 优先取前端传入值；为空/非法时回退 settings.current_user
/// - 底层 claim_recording_to_user 保证仅 __local__ 记录可归属、不抢占其他账号
/// - 返回 true 表示本次实际发生归属
fn claim_recording_inner(
    conn: &rusqlite::Connection,
    recording_id: &str,
    user_id: Option<&str>,
) -> Result<bool, String> {
    let uid = match user_id {
        Some(u) if !u.is_empty() && u != LOCAL_USER => u.to_string(),
        _ => get_current_user(conn).unwrap_or_default(),
    };
    if uid.is_empty() || uid == LOCAL_USER {
        return Ok(false);
    }
    crate::db::claim_recording_to_user(conn, recording_id, &uid)
}

#[tauri::command]
pub fn claim_recording(
    db: State<DbState>,
    recording_id: String,
    user_id: Option<String>,
) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    claim_recording_inner(&conn, &recording_id, user_id.as_deref())
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

/// 单条删除的共用实现（供 delete_recording / delete_recordings 复用）。
/// 返回是否真的删除了一条录制（id 不存在时为 false，视为 noop）。
fn delete_recording_conn(conn: &rusqlite::Connection, id: &str) -> Result<bool, String> {
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

    // Delete recording（记录是否真的删除了行：id 不存在 = noop）
    let deleted = conn
        .execute("DELETE FROM recordings WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;

    // Delete screenshot files from disk
    for path in &screenshot_paths {
        if !path.is_empty() {
            let _ = std::fs::remove_file(path);
        }
    }

    Ok(deleted > 0)
}

#[tauri::command]
pub fn delete_recording(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    delete_recording_conn(&conn, &id)?;
    Ok(())
}

/// ★ 批量删除录制：逐条复用单条删除逻辑（收集截图路径 → 删 steps → 删 recordings → 删截图文件）。
/// 返回实际删除的条数；不存在的 id 静默跳过（幂等），任何一条失败即整体报错。
#[tauri::command]
pub fn delete_recordings(db: State<DbState>, ids: Vec<String>) -> Result<u32, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut deleted: u32 = 0;
    for id in &ids {
        if delete_recording_conn(&conn, id)? {
            deleted += 1;
        }
    }
    Ok(deleted)
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
            target_app: None,
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

    #[test]
    fn delete_recordings_conn_batch_removes_multiple_and_skips_unknown() {
        let conn = mem_db();
        for id in ["b1", "b2", "b3"] {
            crate::db::save_recording_to_db(&conn, &recording(id, 2)).unwrap();
        }

        // 批量删除 2 条存在的 + 1 条不存在的（幂等跳过）
        let mut deleted = 0;
        for id in ["b1", "b2", "ghost"] {
            if delete_recording_conn(&conn, id).unwrap() {
                deleted += 1;
            }
        }
        assert_eq!(deleted, 2);

        let remain: Vec<String> = {
            let mut stmt = conn.prepare("SELECT id FROM recordings ORDER BY id").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert_eq!(remain, vec!["b3".to_string()]);

        // 步骤级联清理：b1/b2 的 steps 必须为 0，b3 的保留
        let orphan: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE recording_id IN ('b1','b2')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan, 0);
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE recording_id = 'b3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 2);
    }

    #[test]
    fn delete_recordings_conn_empty_ids_is_noop() {
        let conn = mem_db();
        crate::db::save_recording_to_db(&conn, &recording("keep", 1)).unwrap();
        // 空列表：不触发任何删除
        let ids: Vec<String> = vec![];
        let mut deleted = 0;
        for id in &ids {
            if delete_recording_conn(&conn, id).unwrap() {
                deleted += 1;
            }
        }
        assert_eq!(deleted, 0);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recordings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
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

    // ---------- 账号归属（set_current_user / claim_recording / 列表隔离） ----------

    fn seed_rec(conn: &rusqlite::Connection, id: &str, user_id: &str) {
        conn.execute(
            "INSERT INTO recordings (id, title, app_name, status, total_steps, user_id) \
             VALUES (?1, ?2, 'App', 'idle', 0, ?3)",
            params![id, format!("T-{}", id), user_id],
        )
        .unwrap();
    }

    #[test]
    fn set_current_user_writes_settings_only() {
        let conn = mem_db();
        set_current_user_inner(&conn, "user-1").unwrap();
        let v: String = conn
            .query_row("SELECT value FROM settings WHERE key = 'current_user'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "user-1");
        // 清空标记（退出登录）
        set_current_user_inner(&conn, "").unwrap();
        assert!(get_current_user(&conn).is_none());
    }

    #[test]
    fn list_recordings_isolates_by_login_state() {
        let conn = mem_db();
        seed_rec(&conn, "a-local", LOCAL_USER);
        seed_rec(&conn, "b-alice", "alice");
        seed_rec(&conn, "c-alice", "alice");
        seed_rec(&conn, "d-bob", "bob");

        // 未登录：只看到 __local__
        let ids: Vec<String> = list_recordings_inner(&conn).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["a-local"]);

        // 登录 alice：只看到 alice 的记录，不重复、不漏
        set_current_user_inner(&conn, "alice").unwrap();
        let ids: Vec<String> = list_recordings_inner(&conn).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"b-alice".to_string()) && ids.contains(&"c-alice".to_string()));

        // 登录 bob：只看到 bob 的记录
        set_current_user_inner(&conn, "bob").unwrap();
        let ids: Vec<String> = list_recordings_inner(&conn).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["d-bob"]);

        // 退出登录：回到 __local__
        set_current_user_inner(&conn, "").unwrap();
        let ids: Vec<String> = list_recordings_inner(&conn).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["a-local"]);
    }

    #[test]
    fn claim_recording_prefers_explicit_user_then_falls_back() {
        let conn = mem_db();
        seed_rec(&conn, "r1", LOCAL_USER);
        seed_rec(&conn, "r2", LOCAL_USER);

        // 显式 user_id 优先
        assert!(claim_recording_inner(&conn, "r1", Some("alice")).unwrap());
        let uid: String = conn.query_row("SELECT user_id FROM recordings WHERE id = 'r1'", [], |r| r.get(0)).unwrap();
        assert_eq!(uid, "alice");

        // user_id 非法时回退 settings.current_user
        set_current_user_inner(&conn, "bob").unwrap();
        assert!(claim_recording_inner(&conn, "r2", Some("")).unwrap());
        let uid: String = conn.query_row("SELECT user_id FROM recordings WHERE id = 'r2'", [], |r| r.get(0)).unwrap();
        assert_eq!(uid, "bob");

        // 无显式 user_id 且未登录：拒绝
        set_current_user_inner(&conn, "").unwrap();
        seed_rec(&conn, "r3", LOCAL_USER);
        assert!(!claim_recording_inner(&conn, "r3", None).unwrap());
    }

    #[test]
    fn claim_recording_does_not_steal_or_double_claim() {
        let conn = mem_db();
        seed_rec(&conn, "r1", "alice");
        // 已属 alice 的记录不可被 bob 抢占
        assert!(!claim_recording_inner(&conn, "r1", Some("bob")).unwrap());
        let uid: String = conn.query_row("SELECT user_id FROM recordings WHERE id = 'r1'", [], |r| r.get(0)).unwrap();
        assert_eq!(uid, "alice");

        // 重复归属同一记录：幂等
        seed_rec(&conn, "r2", LOCAL_USER);
        assert!(claim_recording_inner(&conn, "r2", Some("bob")).unwrap());
        assert!(!claim_recording_inner(&conn, "r2", Some("bob")).unwrap());
    }
}
