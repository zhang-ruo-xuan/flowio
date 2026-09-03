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
    pub target_app: String,
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
    pub crop_w: Option<i32>,
    pub crop_h: Option<i32>,
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
            "SELECT id, title, app_name, target_app, status, total_steps, duration_secs, \
             created_at, updated_at FROM recordings WHERE id = ?1",
            params![id],
            |row| {
                Ok(Recording {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    app_name: row.get(2)?,
                    // target_app 列对旧录制为 NULL（v66 之前的记录），必须按 Option 读
                    target_app: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    status: row.get(4)?,
                    total_steps: row.get(5)?,
                    duration_secs: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .map_err(|e| format!("Recording not found: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, recording_id, order_index, step_number, action_type, \
             title, description, tip, screenshot_path, annotated_path, \
             before_screenshot_path, created_at, \
             x, y, crop_w, crop_h, after_screenshot_path, timestamp, window_title \
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
                crop_w: row.get(14)?,
                crop_h: row.get(15)?,
                after_screenshot_path: row.get(16)?,
                timestamp: row.get(17)?,
                screenshot_base64: None,
                after_screenshot_base64: None,
                before_screenshot_base64: None,
                window_title: row.get(18)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut steps = Vec::new();
    for row in rows {
        steps.push(row.map_err(|e| e.to_string())?);
    }

    // Convert screenshot files to base64 data URLs
    for step in &mut steps {
        populate_step_screenshot_base64(step);
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

/// Update a step's crop anchor: center (x, y) + element rect (crop_w, crop_h).
///
/// Unlike `crop_step_screenshot`, this does **not** touch any stored image —
/// the original full screenshot is preserved so the crop can be redone later.
///
/// Used by the editor's manual "re-crop" flow: the user drags a box on the full
/// screenshot, the frontend inverts that box back into an anchor, and the
/// existing adaptive-crop pipeline then reproduces exactly that box.
///
/// Passing `None` clears the anchor (step falls back to click-point cropping).
#[tauri::command]
pub fn update_step_crop(
    db: State<DbState>,
    step_id: String,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE steps SET x = ?1, y = ?2, crop_w = ?3, crop_h = ?4 WHERE id = ?5",
        params![x, y, crop_w, crop_h, step_id],
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
    for (i, step_id) in id_order_updates(&step_ids) {
        conn.execute(
            "UPDATE steps SET order_index = ?1 WHERE id = ?2",
            params![i, step_id],
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

    Ok(build_new_step(
        step_id,
        recording_id,
        order_index,
        step_data,
        now,
        now_ms,
    ))
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
    let jpeg_bytes = decode_jpeg_base64(&cropped_base64)?;

    // Determine which path column to update
    let path_col = screenshot_col(is_after, false);

    // Get current path
    let current_path: String = conn
        .query_row(
            &select_col_sql(path_col),
            params![step_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Step not found: {}", e))?;

    // Generate new path with _cropped suffix
    let new_path_str = cropped_path_for(&current_path);

    // Write cropped JPEG
    std::fs::write(&new_path_str, &jpeg_bytes).map_err(|e| format!("Failed to write cropped image: {}", e))?;

    // Update DB
    conn.execute(
        &update_col_sql(path_col),
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
    let jpeg_bytes = decode_jpeg_base64(&base64_data)?;

    // Generate new screenshot path
    let prefix = upload_prefix(is_after, is_marked);
    let new_path = crate::recorder::screenshot::next_screenshot_path(prefix);
    let new_path_str = new_path.to_string_lossy().to_string();

    // Write JPEG to disk
    std::fs::write(&new_path, &jpeg_bytes)
        .map_err(|e| format!("Failed to write uploaded image: {}", e))?;

    // Determine which DB column to update
    let path_col = upload_path_col(is_after, is_marked);

    // Update DB
    conn.execute(
        &update_col_sql(path_col),
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
        let col = screenshot_col(true, false);
        let screenshot_path: String = conn
            .query_row(
                &select_col_sql(col),
                params![step_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        if !screenshot_path.is_empty() {
            std::fs::remove_file(&screenshot_path).ok();
        }

        conn.execute(
            &clear_col_sql(col),
            params![step_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        // Delete "before" screenshot (before_screenshot_path) only
        let col = screenshot_col(false, false);
        let before_screenshot_path: String = conn
            .query_row(
                &select_col_sql(col),
                params![step_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        if !before_screenshot_path.is_empty() {
            std::fs::remove_file(&before_screenshot_path).ok();
        }

        conn.execute(
            &clear_col_sql(col),
            params![step_id],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ── Pure helpers ─────────────────────────────────────────────

/// 选择截图字段列名（与前端 `is_after`/`is_marked` 约定一致）。
/// is_marked 优先于 is_after：标注图存 annotated_path；其次 after 存 after_screenshot_path；
/// 其余存主截图 screenshot_path。crop 场景（is_marked=false）里 is_after=true 裁剪主截图。
fn screenshot_col(is_after: bool, is_marked: bool) -> &'static str {
    if is_marked {
        "annotated_path"
    } else if is_after {
        "screenshot_path"
    } else {
        "before_screenshot_path"
    }
}

/// 由当前截图路径生成 `_cropped.jpg` 裁剪副本路径（保持同目录）。
/// 空路径/无扩展名时生成 "_cropped.jpg"（相对当前目录，记录现状）。
fn cropped_path_for(current_path: &str) -> String {
    let current = std::path::Path::new(current_path);
    let stem = current.file_stem().unwrap_or_default().to_string_lossy();
    let parent = current.parent().unwrap_or(std::path::Path::new("."));
    parent.join(format!("{}_cropped.jpg", stem)).to_string_lossy().to_string()
}

/// 解码 base64 JPEG 数据（crop/upload 共用，错误统一带前缀）。
fn decode_jpeg_base64(data: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("Base64 decode failed: {}", e))
}

/// 上传截图的文件前缀（marked > after > upload）。
fn upload_prefix(is_after: bool, is_marked: bool) -> &'static str {
    if is_marked {
        "marked"
    } else if is_after {
        "after"
    } else {
        "upload"
    }
}

/// upload 场景的截图列名。注意与 crop/delete 的 `is_after` 语义不同：
/// 此处 is_after=true 写 after_screenshot_path、false 写主截图 screenshot_path；
/// 而 `screenshot_col` 中 is_after=true 对应主截图 screenshot_path、false 对应
/// before_screenshot_path（见 upload_step_screenshot 调用处，勿互相替换）。
fn upload_path_col(is_after: bool, is_marked: bool) -> &'static str {
    if is_marked {
        "annotated_path"
    } else if is_after {
        "after_screenshot_path"
    } else {
        "screenshot_path"
    }
}

/// 批量重排：step_id 顺序即新 order_index（从 0 开始）。
fn id_order_updates(step_ids: &[String]) -> Vec<(i32, &str)> {
    step_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (i as i32, id.as_str()))
        .collect()
}

/// 组装新插入步骤（add_step 的 Step 构造，便于单测）。
fn build_new_step(
    step_id: String,
    recording_id: String,
    order_index: i32,
    step_data: StepData,
    now: String,
    now_ms: i64,
) -> Step {
    Step {
        id: step_id,
        recording_id,
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
        crop_w: None,
        crop_h: None,
        after_screenshot_path: None,
        timestamp: now_ms,
        screenshot_base64: None,
        after_screenshot_base64: None,
        before_screenshot_base64: None,
        window_title: step_data.window_title,
    }
}

/// 读取文件并编码为 base64 字符串；路径为空或读取失败返回 None。
fn maybe_read_base64(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    std::fs::read(path)
        .ok()
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// 为步骤填充三个截图字段的 base64（load_recording 用，等价于原内联循环）。
fn populate_step_screenshot_base64(step: &mut Step) {
    step.screenshot_base64 = maybe_read_base64(&step.screenshot_path);
    step.before_screenshot_base64 = maybe_read_base64(&step.before_screenshot_path);
    step.after_screenshot_base64 = step
        .after_screenshot_path
        .as_deref()
        .and_then(maybe_read_base64);
}

/// 动态列名 SELECT 语句（列名来自白名单常量，无注入风险）。
fn select_col_sql(col: &str) -> String {
    format!("SELECT {} FROM steps WHERE id = ?1", col)
}

/// 动态列名 UPDATE 语句（?1 为列值，?2 为 step_id）。
fn update_col_sql(col: &str) -> String {
    format!("UPDATE steps SET {} = ?1 WHERE id = ?2", col)
}

/// 动态列名清空语句（置空串，?1 为 step_id）。
fn clear_col_sql(col: &str) -> String {
    format!("UPDATE steps SET {} = '' WHERE id = ?1", col)
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    #[test]
    fn step_data_window_title_defaults_to_empty() {
        // 前端可能不传 window_title，serde(default) 应回退为空串
        let json = r#"{"order_index":0,"step_number":1,"title":"t","description":"d","action_type":"click","tip":"","screenshot_path":""}"#;
        let sd: StepData = serde_json::from_str(json).unwrap();
        assert_eq!(sd.window_title, "");
        assert_eq!(sd.order_index, 0);
        assert_eq!(sd.step_number, 1);
    }

    #[test]
    fn step_data_parses_full_fields() {
        let json = r#"{"order_index":2,"step_number":3,"title":"t","description":"d","action_type":"type","tip":"tip","screenshot_path":"s.png","window_title":"记事本"}"#;
        let sd: StepData = serde_json::from_str(json).unwrap();
        assert_eq!(sd.order_index, 2);
        assert_eq!(sd.step_number, 3);
        assert_eq!(sd.action_type, "type");
        assert_eq!(sd.window_title, "记事本");
    }

    #[test]
    fn step_data_missing_required_field_fails() {
        // 缺少必填字段 title 应解析失败（防静默脏数据）
        let json = r#"{"order_index":0,"step_number":1,"description":"d","action_type":"click","tip":"","screenshot_path":""}"#;
        assert!(serde_json::from_str::<StepData>(json).is_err());
    }

    #[test]
    fn recording_json_roundtrip() {
        let r = Recording {
            id: "r1".into(),
            title: "标题".into(),
            app_name: "App".into(),
            target_app: String::new(),
            status: "completed".into(),
            total_steps: 3,
            duration_secs: 12,
            created_at: "2026-08-09".into(),
            updated_at: "2026-08-09".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Recording = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "completed");
        assert_eq!(back.total_steps, 3);
    }

    #[test]
    fn screenshot_col_priority_marked_over_after() {
        assert_eq!(screenshot_col(false, false), "before_screenshot_path");
        assert_eq!(screenshot_col(true, false), "screenshot_path");
        assert_eq!(screenshot_col(false, true), "annotated_path");
        assert_eq!(screenshot_col(true, true), "annotated_path"); // marked 优先
    }

    #[test]
    fn cropped_path_for_normal_path() {
        let p = cropped_path_for("D:\\shots\\step_1.png");
        assert_eq!(p, "D:\\shots\\step_1_cropped.jpg");
        let p2 = cropped_path_for("D:/shots/step_2.PNG");
        // Windows PathBuf 分隔符可能规范化为 \，统一替换后断言
        assert!(p2.replace('\\', "/").ends_with("D:/shots/step_2_cropped.jpg"));
    }

    #[test]
    fn cropped_path_for_missing_ext_or_empty() {
        // 无扩展名：追加 _cropped.jpg（记录现状）
        let p = cropped_path_for("D:\\shots\\step");
        assert_eq!(p, "D:\\shots\\step_cropped.jpg");
        // 空路径：相对当前目录生成 _cropped.jpg（不 panic）
        let p2 = cropped_path_for("");
        assert!(p2.ends_with("_cropped.jpg"));
    }

    #[test]
    fn step_serde_serializes_none_fields() {
        // Step 序列化后 Option 字段以 null 呈现，前端 JSON 解析应兼容
        let s = Step {
            id: "s1".into(),
            recording_id: "r1".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "t".into(),
            description: "d".into(),
            tip: "".into(),
            screenshot_path: "".into(),
            annotated_path: "".into(),
            before_screenshot_path: "".into(),
            created_at: "2026-08-09".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None,
            timestamp: 123,
            screenshot_base64: None,
            after_screenshot_base64: None,
            before_screenshot_base64: None,
            window_title: "记事本".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["x"], serde_json::Value::Null);
        assert_eq!(v["after_screenshot_path"], serde_json::Value::Null);
        assert_eq!(v["screenshot_base64"], serde_json::Value::Null);
        assert_eq!(v["window_title"], "记事本");
        // 反序列化回 Step 不丢字段
        let back: Step = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "s1");
        assert!(back.x.is_none());
    }

    #[test]
    fn recording_with_steps_roundtrip() {
        let ws = RecordingWithSteps {
            recording: Recording {
                id: "r1".into(),
                title: "标题".into(),
                app_name: "App".into(),
                target_app: String::new(),
                status: "recording".into(),
                total_steps: 1,
                duration_secs: 5,
                created_at: "2026-08-09".into(),
                updated_at: "2026-08-09".into(),
            },
            steps: vec![],
        };
        let json = serde_json::to_string(&ws).unwrap();
        assert!(json.contains("\"recording\""));
        assert!(json.contains("\"steps\":[]"));
    }

    // ── decode_jpeg_base64 ─────────────────────────────────────

    #[test]
    fn decode_jpeg_base64_valid_roundtrip() {
        let bytes = vec![0xFFu8, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let decoded = decode_jpeg_base64(&b64).unwrap();
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn decode_jpeg_base64_empty_string() {
        // 空串可解码为空字节（base64 合法），等价原行为
        let decoded = decode_jpeg_base64("").unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_jpeg_base64_invalid_returns_prefix_error() {
        let err = decode_jpeg_base64("!!!not-base64!!!").unwrap_err();
        assert!(err.starts_with("Base64 decode failed"));
    }

    #[test]
    fn decode_jpeg_base64_whitespace_rejected() {
        // 含空白字符的 base64 应报错（原实现行为）
        assert!(decode_jpeg_base64("aGVsbG8= ").is_err());
    }

    // ── upload_prefix / upload_path_col ────────────────────────

    #[test]
    fn upload_prefix_marked_takes_priority() {
        assert_eq!(upload_prefix(false, false), "upload");
        assert_eq!(upload_prefix(true, false), "after");
        assert_eq!(upload_prefix(false, true), "marked");
        assert_eq!(upload_prefix(true, true), "marked");
    }

    #[test]
    fn upload_path_col_mapping() {
        assert_eq!(upload_path_col(false, false), "screenshot_path");
        assert_eq!(upload_path_col(true, false), "after_screenshot_path");
        assert_eq!(upload_path_col(false, true), "annotated_path");
        assert_eq!(upload_path_col(true, true), "annotated_path");
    }

    #[test]
    fn upload_path_col_diverges_from_screenshot_col() {
        // 记录两个列选择器的语义差异（is_after 含义不同），防止误合并
        // screenshot_col(true,false)=screenshot_path（crop/delete 语义）
        // upload_path_col(true,false)=after_screenshot_path（upload 语义）
        assert_eq!(screenshot_col(true, false), "screenshot_path");
        assert_ne!(upload_path_col(true, false), screenshot_col(true, false));
        assert_ne!(upload_path_col(false, false), screenshot_col(false, false));
    }

    // ── id_order_updates ───────────────────────────────────────

    #[test]
    fn id_order_updates_empty() {
        assert!(id_order_updates(&[]).is_empty());
    }

    #[test]
    fn id_order_updates_sequential_indices() {
        let ids = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        let updates = id_order_updates(&ids);
        assert_eq!(updates.len(), 3);
        assert_eq!(updates[0], (0, "s1"));
        assert_eq!(updates[1], (1, "s2"));
        assert_eq!(updates[2], (2, "s3"));
    }

    // ── build_new_step ─────────────────────────────────────────

    #[test]
    fn build_new_step_maps_all_fields() {
        let sd = StepData {
            order_index: 2,
            step_number: 3,
            title: "点击按钮".into(),
            description: "确认".into(),
            action_type: "click".into(),
            tip: "hint".into(),
            screenshot_path: "D:/shots/a.jpg".into(),
            window_title: "记事本".into(),
        };
        let s = build_new_step(
            "step-1".into(),
            "rec-9".into(),
            2,
            sd,
            "2026-08-20 10:00:00".into(),
            123456789,
        );
        assert_eq!(s.id, "step-1");
        assert_eq!(s.recording_id, "rec-9");
        assert_eq!(s.order_index, 2);
        assert_eq!(s.step_number, 3);
        assert_eq!(s.action_type, "click");
        assert_eq!(s.title, "点击按钮");
        assert_eq!(s.description, "确认");
        assert_eq!(s.tip, "hint");
        assert_eq!(s.screenshot_path, "D:/shots/a.jpg");
        assert_eq!(s.window_title, "记事本");
        assert_eq!(s.created_at, "2026-08-20 10:00:00");
        assert_eq!(s.timestamp, 123456789);
        assert_eq!(s.x, None);
        assert_eq!(s.y, None);
        assert_eq!(s.after_screenshot_path, None);
        assert_eq!(s.annotated_path, "");
        assert_eq!(s.before_screenshot_path, "");
        assert!(s.screenshot_base64.is_none());
    }

    #[test]
    fn build_new_step_empty_strings_preserved() {
        let sd = StepData {
            order_index: 0,
            step_number: 1,
            title: String::new(),
            description: String::new(),
            action_type: String::new(),
            tip: String::new(),
            screenshot_path: String::new(),
            window_title: String::new(),
        };
        let s = build_new_step("s".into(), "r".into(), 0, sd, String::new(), 0);
        assert_eq!(s.title, "");
        assert_eq!(s.window_title, "");
        assert_eq!(s.screenshot_path, "");
        assert_eq!(s.timestamp, 0);
    }

    // ── maybe_read_base64 / populate_step_screenshot_base64 ────

    #[test]
    fn maybe_read_base64_empty_path_is_none() {
        assert_eq!(maybe_read_base64(""), None);
    }

    #[test]
    fn maybe_read_base64_missing_file_is_none() {
        assert_eq!(maybe_read_base64("Z:/no/such/file_20260820.jpg"), None);
    }

    #[test]
    fn maybe_read_base64_roundtrip_temp_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("flowio_editor_test_{}.bin", uuid::Uuid::new_v4()));
        let payload = b"\x89PNG\r\n\x1a\nfake-image-bytes";
        std::fs::write(&path, payload).unwrap();
        let b64 = maybe_read_base64(&path.to_string_lossy()).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD.decode(&b64).unwrap();
        assert_eq!(decoded, payload);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn populate_screenshot_base64_all_three_fields() {
        let dir = std::env::temp_dir();
        let p1 = dir.join(format!("fio_main_{}.jpg", uuid::Uuid::new_v4()));
        let p2 = dir.join(format!("fio_before_{}.jpg", uuid::Uuid::new_v4()));
        let p3 = dir.join(format!("fio_after_{}.jpg", uuid::Uuid::new_v4()));
        std::fs::write(&p1, b"main").unwrap();
        std::fs::write(&p2, b"before").unwrap();
        std::fs::write(&p3, b"after").unwrap();
        let mut step = Step {
            id: "s".into(),
            recording_id: "r".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "t".into(),
            description: "d".into(),
            tip: "".into(),
            screenshot_path: p1.to_string_lossy().to_string(),
            annotated_path: String::new(),
            before_screenshot_path: p2.to_string_lossy().to_string(),
            created_at: "".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            after_screenshot_path: Some(p3.to_string_lossy().to_string()),
            timestamp: 0,
            screenshot_base64: None,
            after_screenshot_base64: None,
            before_screenshot_base64: None,
            window_title: "".into(),
        };
        populate_step_screenshot_base64(&mut step);
        assert!(step.screenshot_base64.is_some());
        assert!(step.before_screenshot_base64.is_some());
        assert!(step.after_screenshot_base64.is_some());
        assert_eq!(step.screenshot_base64.as_deref().unwrap(), "bWFpbg=="); // "main"
        assert_eq!(step.before_screenshot_base64.as_deref().unwrap(), "YmVmb3Jl"); // "before"
        assert_eq!(step.after_screenshot_base64.as_deref().unwrap(), "YWZ0ZXI="); // "after"
        std::fs::remove_file(&p1).ok();
        std::fs::remove_file(&p2).ok();
        std::fs::remove_file(&p3).ok();
    }

    #[test]
    fn populate_screenshot_base64_missing_and_empty_stays_none() {
        let mut step = Step {
            id: "s".into(),
            recording_id: "r".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "t".into(),
            description: "d".into(),
            tip: "".into(),
            screenshot_path: String::new(), // 空路径
            annotated_path: String::new(),
            before_screenshot_path: "Z:/no/such/before.jpg".into(), // 缺失文件
            created_at: "".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None, // None
            timestamp: 0,
            screenshot_base64: None,
            after_screenshot_base64: None,
            before_screenshot_base64: None,
            window_title: "".into(),
        };
        populate_step_screenshot_base64(&mut step);
        assert!(step.screenshot_base64.is_none());
        assert!(step.before_screenshot_base64.is_none());
        assert!(step.after_screenshot_base64.is_none());
    }

    // ── SQL 语句组装 ───────────────────────────────────────────

    #[test]
    fn select_col_sql_injects_column() {
        assert_eq!(
            select_col_sql("screenshot_path"),
            "SELECT screenshot_path FROM steps WHERE id = ?1"
        );
        assert_eq!(
            select_col_sql("after_screenshot_path"),
            "SELECT after_screenshot_path FROM steps WHERE id = ?1"
        );
    }

    #[test]
    fn update_col_sql_injects_column() {
        assert_eq!(
            update_col_sql("annotated_path"),
            "UPDATE steps SET annotated_path = ?1 WHERE id = ?2"
        );
    }

    #[test]
    fn clear_col_sql_injects_column() {
        assert_eq!(
            clear_col_sql("before_screenshot_path"),
            "UPDATE steps SET before_screenshot_path = '' WHERE id = ?1"
        );
    }

    #[test]
    fn sql_helpers_keep_placeholder_count() {
        // 防止未来改列名时误破坏参数占位符数量
        for col in ["screenshot_path", "before_screenshot_path", "after_screenshot_path", "annotated_path"] {
            let sel = select_col_sql(col);
            assert_eq!(sel.matches('?').count(), 1, "select 应只有 1 个 ?: {}", sel);
            let upd = update_col_sql(col);
            assert_eq!(upd.matches('?').count(), 2, "update 应有 2 个 ?: {}", upd);
            let clr = clear_col_sql(col);
            assert_eq!(clr.matches('?').count(), 1, "clear 应只有 1 个 ?: {}", clr);
        }
    }

    // ── StepData 序列化边界 ────────────────────────────────────

    #[test]
    fn step_data_extra_unknown_field_ignored() {
        // 前端多传字段不应导致解析失败（serde 默认忽略未知字段）
        let json = r#"{"order_index":1,"step_number":2,"title":"t","description":"d","action_type":"type","tip":"","screenshot_path":"","window_title":"w","extra_field":123}"#;
        let sd: StepData = serde_json::from_str(json).unwrap();
        assert_eq!(sd.window_title, "w");
        assert_eq!(sd.order_index, 1);
    }

    #[test]
    fn step_data_wrong_type_fails() {
        // order_index 传字符串类型应解析失败
        let json = r#"{"order_index":"0","step_number":1,"title":"t","description":"d","action_type":"click","tip":"","screenshot_path":""}"#;
        assert!(serde_json::from_str::<StepData>(json).is_err());
    }

    #[test]
    fn step_serde_deserialize_with_null_optionals() {
        // 反序列化含 null 的 JSON（前端可能发送 null）
        let json = r#"{"id":"s","recording_id":"r","order_index":0,"step_number":1,"action_type":"click","title":"t","description":"d","tip":"","screenshot_path":"","annotated_path":"","before_screenshot_path":"","created_at":"","x":null,"y":null,"after_screenshot_path":null,"timestamp":0,"screenshot_base64":null,"after_screenshot_base64":null,"before_screenshot_base64":null,"window_title":""}"#;
        let s: Step = serde_json::from_str(json).unwrap();
        assert!(s.x.is_none());
        assert!(s.after_screenshot_path.is_none());
        assert_eq!(s.id, "s");
    }
}
