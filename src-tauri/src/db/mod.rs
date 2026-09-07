pub mod commands;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// 本地未归属录制的账号标识（未登录录制的教程均归属该账号）
pub const LOCAL_USER: &str = "__local__";

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
    pub crop_w: Option<i32>,
    pub crop_h: Option<i32>,
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
    pub target_app: Option<String>,
    pub steps: Vec<StepSave>,
}

/// Save a completed recording and its steps into the database in a single transaction.
pub fn save_recording_to_db(conn: &Connection, recording: &RecordingSave) -> Result<(), String> {
    conn.execute("BEGIN", [])
        .map_err(|e| format!("BEGIN: {}", e))?;

    if let Err(e) = conn.execute(
        "INSERT INTO recordings (id, title, app_name, target_app, status, total_steps) VALUES (?1, ?2, ?3, ?4, 'completed', ?5)",
        params![recording.id, recording.title, recording.app_name, recording.target_app, recording.steps.len() as i32],
    ) {
        let _ = conn.execute("ROLLBACK", []);
        return Err(format!("Insert recording failed: {}", e));
    }

    for step in &recording.steps {
        let step_id = uuid::Uuid::new_v4().to_string();
        if let Err(e) = conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, \
             title, description, tip, screenshot_path, before_screenshot_path, x, y, crop_w, crop_h, after_screenshot_path, timestamp, window_title) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
                step.crop_w,
                step.crop_h,
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

/// 将一条仍属本地（`__local__`）的录制精确归属到指定账号。
/// - 幂等：重复调用不会报错，第二次起返回 false（已不再属于 `__local__`）
/// - 不抢占：记录已归属其他账号时直接返回 false，不做任何修改
/// - 目标 user_id 为空或等于 `__local__` 时拒绝更新
/// - 返回 true 表示本次实际发生了归属更新；false 表示未归属/无需变更
pub fn claim_recording_to_user(
    conn: &Connection,
    recording_id: &str,
    user_id: &str,
) -> Result<bool, String> {
    if user_id.is_empty() || user_id == LOCAL_USER {
        return Ok(false);
    }
    let n = conn
        .execute(
            "UPDATE recordings SET user_id = ?1, updated_at = datetime('now') \
             WHERE id = ?2 AND user_id = ?3",
            params![user_id, recording_id, LOCAL_USER],
        )
        .map_err(|e| format!("claim recording failed: {}", e))?;
    Ok(n > 0)
}

pub fn init_db(conn: &Connection) {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS recordings (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL DEFAULT '',
            app_name      TEXT NOT NULL DEFAULT '',
            status        TEXT NOT NULL DEFAULT 'idle',
            total_steps   INTEGER NOT NULL DEFAULT 0,
            duration_secs INTEGER NOT NULL DEFAULT 0,
            ai_generated  INTEGER NOT NULL DEFAULT 0,
            text_redacted INTEGER NOT NULL DEFAULT 0,
            scan_redacted INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
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

        -- 步骤精简（自动合并 / AI 精简）前的全量快照，用于「撤销」。
        -- 每个录制只保留最近 3 条（写入时清理更旧的）。
        CREATE TABLE IF NOT EXISTS step_snapshots (
            id           TEXT PRIMARY KEY,
            recording_id TEXT NOT NULL,
            kind         TEXT NOT NULL DEFAULT 'merge',
            payload      TEXT NOT NULL,
            step_count   INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- 全局撤销/重做（WPS 式 ↶↷）：编辑器每个写命令执行前，
        -- 把 {录制标题 + 全部步骤行} 捕成一份快照压入 undo 栈并清空 redo 栈；
        -- edit_undo 把「当前状态」压入 redo 栈后还原 undo 栈顶，edit_redo 对称。
        -- 每栈每录制最多保留 50 条（写入时清理更旧的）。栈内排序用 rowid（自增）。
        CREATE TABLE IF NOT EXISTS edit_history (
            id           TEXT PRIMARY KEY,
            recording_id TEXT NOT NULL,
            stack        TEXT NOT NULL DEFAULT 'undo',
            payload      TEXT NOT NULL,
            step_count   INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_edit_history_rec
            ON edit_history(recording_id, stack);

        -- 撤销/重做历史只在本次运行内有效：启动即清空（用户要求退出软件后不保留，
        -- 避免重启后还能撤销到上一次会话的旧编辑状态）。
        DELETE FROM edit_history;
        ",
    )
    .expect("Failed to create database tables");

    // ── Migrations: add columns that may not exist in older DBs ──
    for col in &["x", "y", "after_screenshot_path", "timestamp", "window_title", "crop_w", "crop_h"] {
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

    // ── Migrations: recordings 功能标记列（AI 生成 / 文本脱敏 / 扫描脱敏）──
    for col in &[
        "ai_generated",
        "text_redacted",
        "scan_redacted",
    ] {
        let sql = format!(
            "ALTER TABLE recordings ADD COLUMN {} INTEGER NOT NULL DEFAULT 0",
            col
        );
        let _ = conn.execute_batch(&sql);
    }

    // ── Migrations: recordings 账号归属列（旧库兼容，默认本地未归属）──
    // SQLite >= 3.35.0 对已存在的列执行 ADD COLUMN 为 no-op，天然幂等。
    let _ = conn.execute_batch(&format!(
        "ALTER TABLE recordings ADD COLUMN user_id TEXT NOT NULL DEFAULT '{}'",
        LOCAL_USER
    ));

    // ── Migrations: 录制指定软件的目标进程名（旧库兼容）──
    let _ = conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN target_app TEXT",
    );

    // ── Migrations: 教程译文副本（JSON，按语言存储；NULL = 从未翻译过）──
    let _ = conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN translations TEXT",
    );

    // ── Migrations: 步骤变量标记（SKILL.md 自动化导出用，总纲 §3）──
    // is_variable：该步骤的输入值是「每次运行都会变的参数」；
    // param_name：参数名（如 order_id），导出时渲染成 {{param_name}} 占位符。
    // 旧库/未标记步骤默认 0/''，天然兼容存量数据。
    let _ = conn.execute_batch(
        "ALTER TABLE steps ADD COLUMN is_variable INTEGER NOT NULL DEFAULT 0",
    );
    let _ = conn.execute_batch(
        "ALTER TABLE steps ADD COLUMN param_name TEXT NOT NULL DEFAULT ''",
    );
}

/// 读取译文本来的 JSON 字符串（旧库/未翻译 → None；必须 Option 读，NULL 安全）。
pub fn get_recording_translations_raw(
    conn: &Connection,
    recording_id: &str,
) -> Result<Option<String>, String> {
    let json: Option<String> = conn
        .query_row(
            "SELECT translations FROM recordings WHERE id = ?1",
            params![recording_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("read translations failed: {}", e))?;
    Ok(json.filter(|s| !s.is_empty()))
}

/// 写入译文副本 JSON（整体覆盖；同时刷新 updated_at）。
pub fn set_recording_translations_raw(
    conn: &Connection,
    recording_id: &str,
    json: &str,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE recordings SET translations = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![json, recording_id],
        )
        .map_err(|e| format!("write translations failed: {}", e))?;
    if n == 0 {
        return Err(format!("recording {} not found", recording_id));
    }
    Ok(())
}

/// 后台线程专用 DB 绝对路径（由 setup 阶段注入）。
///
/// 🔴 历史坑（2026-09-03）：`init_db_file()` 曾用相对路径 `"flowio.db"`，
/// 从桌面快捷方式启动时 cwd = 桌面 → 语音转写/后台翻译线程在桌面另开一个
/// 空库，步骤读到 0 条（「本次录制没有步骤」）、翻译结果写丢。
/// 主连接用 `app_data_dir/flowio.db` 绝对路径，这里必须保持一致。
static DB_PATH_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// 注入后台线程使用的 DB 绝对路径（lib.rs setup 阶段调用一次）。
pub fn set_db_path(path: PathBuf) {
    let _ = DB_PATH_OVERRIDE.set(path);
}

/// Open (or create) the database file and run migrations.
pub fn init_db_file() -> Connection {
    let conn = match DB_PATH_OVERRIDE.get() {
        Some(p) => Connection::open(p),
        None => Connection::open("flowio.db"), // 兜底：未注入时保持旧行为（仅测试场景）
    }
    .expect("Failed to open database");
    init_db(&conn);
    conn
}

pub fn init_db_managed() -> Connection {
    init_db_file()
}

#[cfg(test)]
mod db_tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn);
        conn
    }

    fn step(order: i32, num: i32, opt: bool) -> StepSave {
        StepSave {
            order_index: order,
            step_number: num,
            title: format!("Step {}", num),
            description: "description".into(),
            action_type: "click".into(),
            tip: "tip".into(),
            screenshot_path: format!("screenshot_{}.png", num),
            x: if opt { Some(100 + num) } else { None },
            y: if opt { Some(200 + num) } else { None },
            crop_w: if opt { Some(300 + num) } else { None },
            crop_h: if opt { Some(400 + num) } else { None },
            before_screenshot_path: format!("before_{}.png", num),
            after_screenshot_path: if opt {
                Some(format!("after_{}.png", num))
            } else {
                None
            },
            timestamp: 1_700_000_000_000 + num as i64,
            window_title: format!("Window {}", num),
        }
    }

    fn recording(id: &str, n: usize, opt: bool) -> RecordingSave {
        RecordingSave {
            id: id.into(),
            title: format!("Recording {}", id),
            app_name: "TestApp".into(),
            target_app: None,
            steps: (0..n).map(|i| step(i as i32, (i + 1) as i32, opt)).collect(),
        }
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table)).unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    #[test]
    fn init_db_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn);
        // 第二次调用不得 panic（迁移会重复执行 ADD COLUMN）
        init_db(&conn);
        let cols = columns(&conn, "steps");
        for c in ["x", "y", "after_screenshot_path", "timestamp", "window_title"] {
            assert!(cols.contains(&c.to_string()), "missing column {}", c);
        }
    }

    #[test]
    fn migrations_add_missing_columns_to_legacy_table() {
        let conn = Connection::open_in_memory().unwrap();
        // 模拟旧库：steps 表缺 5 个迁移列
        conn.execute_batch(
            "CREATE TABLE recordings (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', app_name TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'idle', total_steps INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE steps (id TEXT PRIMARY KEY, recording_id TEXT NOT NULL, order_index INTEGER NOT NULL DEFAULT 0, step_number INTEGER NOT NULL DEFAULT 0, action_type TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', tip TEXT NOT NULL DEFAULT '', screenshot_path TEXT NOT NULL DEFAULT '', before_screenshot_path TEXT NOT NULL DEFAULT '');",
        )
        .unwrap();
        init_db(&conn);
        let cols = columns(&conn, "steps");
        for c in ["x", "y", "after_screenshot_path", "timestamp", "window_title"] {
            assert!(cols.contains(&c.to_string()), "migration missing column {}", c);
        }
        // 补列后能正常保存含新字段的记录
        let rec = recording("legacy", 1, true);
        save_recording_to_db(&conn, &rec).unwrap();
        let n: i32 = conn
            .query_row("SELECT COUNT(*) FROM steps WHERE recording_id = 'legacy'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn save_recording_roundtrip_with_optional_fields() {
        let conn = mem_db();
        let rec = recording("rec-opt", 2, true);
        save_recording_to_db(&conn, &rec).unwrap();

        let (title, app, total): (String, String, i32) = conn
            .query_row(
                "SELECT title, app_name, total_steps FROM recordings WHERE id = 'rec-opt'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "Recording rec-opt");
        assert_eq!(app, "TestApp");
        assert_eq!(total, 2);

        let mut stmt = conn
            .prepare(
                "SELECT order_index, step_number, title, description, action_type, tip, \
                 screenshot_path, x, y, before_screenshot_path, after_screenshot_path, \
                 timestamp, window_title FROM steps WHERE recording_id = 'rec-opt' ORDER BY order_index",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<i32>>(7)?,
                    r.get::<_, Option<i32>>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, i64>(11)?,
                    r.get::<_, String>(12)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        let (order, num, title, desc, at, tip, shot, x, y, before, after, ts, wt) = &rows[0];
        assert_eq!(*order, 0);
        assert_eq!(*num, 1);
        assert_eq!(title, "Step 1");
        assert_eq!(desc, "description");
        assert_eq!(at, "click");
        assert_eq!(tip, "tip");
        assert_eq!(shot, "screenshot_1.png");
        assert_eq!(*x, Some(101));
        assert_eq!(*y, Some(201));
        assert_eq!(before, "before_1.png");
        assert_eq!(after.as_deref(), Some("after_1.png"));
        assert_eq!(*ts, 1_700_000_000_001);
        assert_eq!(wt, "Window 1");
        let (_, _, _, _, _, _, _, x2, y2, _, after2, ts2, _) = &rows[1];
        assert_eq!(*x2, Some(102));
        assert_eq!(*y2, Some(202));
        assert_eq!(after2.as_deref(), Some("after_2.png"));
        assert_eq!(*ts2, 1_700_000_000_002);
    }

    #[test]
    fn save_recording_none_optional_fields() {
        let conn = mem_db();
        let rec = recording("rec-none", 1, false);
        save_recording_to_db(&conn, &rec).unwrap();
        let (x, y, after): (Option<i32>, Option<i32>, Option<String>) = conn
            .query_row(
                "SELECT x, y, after_screenshot_path FROM steps WHERE recording_id = 'rec-none'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(x, None);
        assert_eq!(y, None);
        assert_eq!(after, None);
    }

    #[test]
    fn duplicate_recording_id_rolls_back() {
        let conn = mem_db();
        let rec = recording("dup", 3, true);
        save_recording_to_db(&conn, &rec).unwrap();
        let err = save_recording_to_db(&conn, &rec).unwrap_err();
        assert!(err.contains("Insert recording failed"), "err: {}", err);
        // 回滚后 steps 不残留（仍只有第一次保存的 3 条）
        let n: i32 = conn
            .query_row("SELECT COUNT(*) FROM steps WHERE recording_id = 'dup'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        // 连接仍可用：再次保存其他记录成功
        save_recording_to_db(&conn, &recording("other", 1, false)).unwrap();
        let n2: i32 = conn
            .query_row("SELECT COUNT(*) FROM steps WHERE recording_id = 'other'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 1);
    }

    #[test]
    fn save_recording_empty_steps() {
        let conn = mem_db();
        let rec = RecordingSave {
            id: "empty".into(),
            title: "Empty".into(),
            app_name: "App".into(),
            target_app: None,
            steps: vec![],
        };
        save_recording_to_db(&conn, &rec).unwrap();
        let (total, n): (i32, i32) = conn
            .query_row(
                "SELECT total_steps, (SELECT COUNT(*) FROM steps WHERE recording_id = 'empty') FROM recordings WHERE id = 'empty'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(total, 0);
        assert_eq!(n, 0);
    }

    #[test]
    fn save_recording_many_steps_1000() {
        let conn = mem_db();
        let rec = recording("big", 1000, true);
        save_recording_to_db(&conn, &rec).unwrap();
        let (total, n): (i32, i32) = conn
            .query_row(
                "SELECT total_steps, (SELECT COUNT(*) FROM steps WHERE recording_id = 'big') FROM recordings WHERE id = 'big'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(total, 1000);
        assert_eq!(n, 1000);
    }

    #[test]
    fn foreign_keys_reject_orphan_step() {
        let conn = mem_db();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let res = conn.execute(
            "INSERT INTO steps (id, recording_id) VALUES ('orphan', 'missing')",
            [],
        );
        assert!(res.is_err(), "orphan step should be rejected when foreign_keys=ON");
    }

    #[test]
    fn foreign_keys_cascade_delete() {
        let conn = mem_db();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        save_recording_to_db(&conn, &recording("fk", 2, false)).unwrap();
        conn.execute("DELETE FROM recordings WHERE id = 'fk'", []).unwrap();
        let n: i32 = conn.query_row("SELECT COUNT(*) FROM steps", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn saved_recording_status_is_completed() {
        // Bug4 修复回归：save_recording_to_db 保存已完成录制，status 必须为 'completed'，
        // 与表默认 'idle'、AI 生成后置 'completed' 的命令层判断保持一致。
        let conn = mem_db();
        save_recording_to_db(&conn, &recording("st", 1, false)).unwrap();
        let status: String = conn
            .query_row("SELECT status FROM recordings WHERE id = 'st'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "completed");
    }


    #[test]
    fn recording_roundtrip_unicode_and_special_chars() {
        let conn = mem_db();
        let mut rec = recording("uni", 2, true);
        rec.title = "中文标题 '单引号' \"双引号\" 换行\n<Tag> & 符号 \u{1F600}".into();
        rec.app_name = "App#1 / (测试)".into();
        rec.steps[0].title = "步骤 <b>加粗</b> & \"引号\"".into();
        rec.steps[0].description = "描述含\n换行\n多行".into();
        rec.steps[0].window_title = "窗口 [1] - 测试".into();
        rec.steps[0].screenshot_path = "D:\\目录\\含空格 截图.png".into();
        save_recording_to_db(&conn, &rec).unwrap();
        let (title, app): (String, String) = conn
            .query_row("SELECT title, app_name FROM recordings WHERE id = 'uni'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(title, rec.title);
        assert_eq!(app, rec.app_name);
        let (t, d, wt, shot): (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT title, description, window_title, screenshot_path FROM steps WHERE recording_id = 'uni' AND order_index = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(t, rec.steps[0].title);
        assert_eq!(d, rec.steps[0].description);
        assert_eq!(wt, rec.steps[0].window_title);
        assert_eq!(shot.as_deref(), Some(rec.steps[0].screenshot_path.as_str()));
    }

    #[test]
    fn save_recording_with_negative_and_zero_indexes() {
        let conn = mem_db();
        let mut rec = recording("neg", 1, true);
        rec.steps[0].order_index = -5;
        rec.steps[0].step_number = 0;
        save_recording_to_db(&conn, &rec).unwrap();
        let (oi, sn): (i32, i32) = conn
            .query_row("SELECT order_index, step_number FROM steps WHERE recording_id = 'neg'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(oi, -5);
        assert_eq!(sn, 0);
    }

    #[test]
    fn save_recording_zero_timestamp_and_empty_strings() {
        let conn = mem_db();
        let mut rec = recording("zt", 1, true);
        rec.steps[0].timestamp = 0;
        rec.steps[0].title = String::new();
        rec.steps[0].action_type = String::new();
        rec.steps[0].window_title = String::new();
        save_recording_to_db(&conn, &rec).unwrap();
        let (ts, title, at, wt): (i64, String, String, String) = conn
            .query_row("SELECT timestamp, title, action_type, window_title FROM steps WHERE recording_id = 'zt'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        assert_eq!(ts, 0);
        assert_eq!(title, "");
        assert_eq!(at, "");
        assert_eq!(wt, "");
    }

    #[test]
    fn save_recording_very_long_fields() {
        let conn = mem_db();
        let long = "x".repeat(100_000);
        let mut rec = recording("long", 1, false);
        rec.steps[0].description = long.clone();
        save_recording_to_db(&conn, &rec).unwrap();
        let d: String = conn
            .query_row("SELECT description FROM steps WHERE recording_id = 'long'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(d.len(), 100_000);
    }

    #[test]
    fn save_recording_many_steps_5000_integrity() {
        let conn = mem_db();
        let rec = recording("big5k", 5000, true);
        save_recording_to_db(&conn, &rec).unwrap();
        let (total, n): (i32, i32) = conn
            .query_row(
                "SELECT total_steps, (SELECT COUNT(*) FROM steps WHERE recording_id = 'big5k') FROM recordings WHERE id = 'big5k'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(total, 5000);
        assert_eq!(n, 5000);
    }

    #[test]
    fn two_recordings_same_step_payload_no_cross_contamination() {
        let conn = mem_db();
        let rec_a = recording("A", 2, true);
        let rec_b = recording("B", 2, true);
        save_recording_to_db(&conn, &rec_a).unwrap();
        save_recording_to_db(&conn, &rec_b).unwrap();
        let n_a: i32 = conn.query_row("SELECT COUNT(*) FROM steps WHERE recording_id = 'A'", [], |r| r.get(0)).unwrap();
        let n_b: i32 = conn.query_row("SELECT COUNT(*) FROM steps WHERE recording_id = 'B'", [], |r| r.get(0)).unwrap();
        assert_eq!(n_a, 2);
        assert_eq!(n_b, 2);
        let w_b: String = conn.query_row("SELECT window_title FROM steps WHERE recording_id = 'B' AND order_index = 0", [], |r| r.get(0)).unwrap();
        assert_eq!(w_b, "Window 1");
    }

    #[test]
    fn save_recording_zero_x_y() {
        let conn = mem_db();
        let mut rec = recording("xy", 1, true);
        rec.steps[0].x = Some(0);
        rec.steps[0].y = Some(0);
        save_recording_to_db(&conn, &rec).unwrap();
        let (x, y): (Option<i32>, Option<i32>) = conn
            .query_row("SELECT x, y FROM steps WHERE recording_id = 'xy'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(x, Some(0));
        assert_eq!(y, Some(0));
    }

    #[test]
    fn settings_key_value_roundtrip() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('k1', 'v1')", []).unwrap();
        let v: String = conn.query_row("SELECT value FROM settings WHERE key = 'k1'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "v1");
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('k1', 'v2')", []).unwrap();
        let v2: String = conn.query_row("SELECT value FROM settings WHERE key = 'k1'", [], |r| r.get(0)).unwrap();
        assert_eq!(v2, "v2");
    }

    // ---------- 账号归属（claim_recording_to_user） ----------

    fn seed_recording(conn: &Connection, id: &str, user_id: &str) {
        conn.execute(
            "INSERT INTO recordings (id, title, app_name, status, total_steps, user_id) \
             VALUES (?1, ?2, 'App', 'idle', 0, ?3)",
            params![id, format!("Rec {}", id), user_id],
        )
        .unwrap();
    }

    fn user_id_of(conn: &Connection, id: &str) -> String {
        conn.query_row("SELECT user_id FROM recordings WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrations_add_user_id_column_to_legacy_table() {
        // 模拟旧库：recordings 表没有 user_id 列
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE recordings (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', app_name TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'idle', total_steps INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        init_db(&conn);
        let cols = columns(&conn, "recordings");
        assert!(cols.contains(&"user_id".to_string()), "migration missing user_id column");
        // 旧记录默认归属 __local__
        seed_recording(&conn, "old", LOCAL_USER);
        assert_eq!(user_id_of(&conn, "old"), LOCAL_USER);
    }

    #[test]
    fn claim_transfers_local_recording_to_user() {
        let conn = mem_db();
        seed_recording(&conn, "r1", LOCAL_USER);
        let claimed = claim_recording_to_user(&conn, "r1", "user-123").unwrap();
        assert!(claimed, "should claim a __local__ recording");
        assert_eq!(user_id_of(&conn, "r1"), "user-123");
    }

    #[test]
    fn claim_is_idempotent() {
        let conn = mem_db();
        seed_recording(&conn, "r2", LOCAL_USER);
        assert!(claim_recording_to_user(&conn, "r2", "user-123").unwrap());
        // 已归属后再次调用：不得报错、不得改归属
        assert!(!claim_recording_to_user(&conn, "r2", "user-123").unwrap());
        assert_eq!(user_id_of(&conn, "r2"), "user-123");
        // 尝试抢归属到其他账号：不生效
        assert!(!claim_recording_to_user(&conn, "r2", "user-456").unwrap());
        assert_eq!(user_id_of(&conn, "r2"), "user-123");
    }

    #[test]
    fn claim_does_not_steal_other_users_recording() {
        let conn = mem_db();
        seed_recording(&conn, "r3", "user-owner");
        let claimed = claim_recording_to_user(&conn, "r3", "user-other").unwrap();
        assert!(!claimed, "must not steal a recording owned by another user");
        assert_eq!(user_id_of(&conn, "r3"), "user-owner");
    }

    #[test]
    fn claim_rejects_invalid_user() {
        let conn = mem_db();
        seed_recording(&conn, "r4", LOCAL_USER);
        assert!(!claim_recording_to_user(&conn, "r4", "").unwrap());
        assert!(!claim_recording_to_user(&conn, "r4", LOCAL_USER).unwrap());
        assert_eq!(user_id_of(&conn, "r4"), LOCAL_USER);
    }

    #[test]
    fn claim_missing_recording_is_noop() {
        let conn = mem_db();
        let claimed = claim_recording_to_user(&conn, "ghost", "user-123").unwrap();
        assert!(!claimed);
    }
}
