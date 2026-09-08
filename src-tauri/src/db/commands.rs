use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

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
    app: AppHandle,
    db: State<DbState>,
    mut recording: RecordingSave,
) -> Result<(), String> {
    let already_enhanced = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        // 自动命名（时间戳）可能与其他录制撞名（如快速连续停止两次录制），
        // 此处统一做去重兜底，确保入库标题唯一。
        recording.title = find_unique_title(&conn, &recording.title);
        crate::db::save_recording_to_db(&conn, &recording)?;
        // 已自动补写过的录制（ai_generated=1）不再重复触发，
        // 避免编辑/另存为后反复调用 LLM 烧额度/撞频限。
        conn.query_row(
            "SELECT ai_generated FROM recordings WHERE id = ?1",
            params![recording.id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            == 1
    };

    // 录制后自动 AI 补写：后台、非阻塞、失败静默。缺省关闭（opt-in），
    // 需在 settings 表写入 `ai_auto_enhance = "true"` 才开启（用户决定主要做自动化、AI 生成暂不需要）。
    if ai_auto_enhance_enabled(&db) && !already_enhanced {
        let rid = recording.id.clone();
        tauri::async_runtime::spawn(async move {
            // State 有生命周期约束、不能直接移入 'static 任务；
            // 改用 AppHandle 在任务内重新取状态句柄（AppHandle 本身是 'static）。
            let db = app.state::<DbState>();
            let _ = crate::ai::commands::run_ai_enhancement(db, rid, None).await;
        });
    }
    Ok(())
}

/// 录制后是否自动跑 AI 补写（生成语义化标题/描述）。
/// settings 表 `ai_auto_enhance` 为 "true"/"1" 时开启，缺省默认关闭（opt-in）。
fn ai_auto_enhance_enabled(db: &State<DbState>) -> bool {
    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(_) => return false, // 锁异常时默认关闭，不阻断保存
    };
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'ai_auto_enhance'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| match v.as_str() {
        "false" | "0" => Some(false),
        "true" | "1" => Some(true),
        _ => None,
    })
    .unwrap_or(false)
}

/// 根据基础标题生成一个在 recordings 表中不重复的标题。
/// 策略：先查「base」是否已存在；不存在直接返回 base。
/// 录制标题最大长度（字符数）：超长标题会把编辑器顶栏挤变形（按钮被压成竖排）。
/// 手动重命名与另存为统一按此封顶；存量数据不回改。
const RECORDING_TITLE_MAX_CHARS: usize = 30;

pub fn cap_title_chars(s: &str) -> String {
    let mut out: String = s.chars().take(RECORDING_TITLE_MAX_CHARS).collect();
    // 截断可能产生残缺结尾，清理悬挂的半个括号
    while out.ends_with('（') {
        out.pop();
    }
    out
}

/// Strip trailing (copy) or (copy N) suffix from a title to get the base name.
/// e.g. "X（副本 2）" => "X", "X（副本）" => "X", "X" => "X"
/// 兼容历史半角变体 "X (副本)" / "X (副本 2)"；
/// 循环剥离：历史版本 bug 会产生「X（副本）（副本）」叠罗汉标题，一次性剥干净。
fn strip_copy_suffix(s: &str) -> &str {
    let mut cur = s;
    while let Some(stripped) = strip_one_copy_suffix(cur) {
        cur = stripped;
    }
    cur.trim_end()
}

fn strip_one_copy_suffix(s: &str) -> Option<&str> {
    let inner = s.strip_suffix('）').or_else(|| s.strip_suffix(')'))?;
    // 找最后一个开括号（全角/半角兼容历史标题）
    let pos = inner.rfind(['（', '('])?;
    // 跳过开括号之后必须紧跟「副本」或「副本 N」
    //（此前这里 pos+3 只跳过了 '（' 却拿「本」去比对，永远匹配不上 → 叠罗汉 bug 根因）
    let after_paren = &inner[pos + inner[pos..].chars().next().unwrap().len_utf8()..];
    if after_paren == "副本" {
        return Some(&s[..pos]);
    }
    let num_part = after_paren.strip_prefix("副本")?;
    let trimmed = num_part.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(&s[..pos]);
    }
    None
}

/// Generate a unique title by stripping any existing (copy) suffix first,
/// then trying base, base(copy), base(copy 2)...base(copy N).
/// Uses full-width parentheses （） to match UI display.
fn find_unique_title(conn: &rusqlite::Connection, base: &str) -> String {
    let stripped = strip_copy_suffix(base);
    // 先剥后缀再封顶：标题超长时截断，防止顶栏变形（副本后缀在封顶后重新追加）
    let base_owned = cap_title_chars(stripped);
    let base = base_owned.as_str();

    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM recordings WHERE title = ?1")
        .expect("prepare find_unique_title");
    let mut exists = |t: &str| -> bool {
        stmt.query_row(params![t], |row| row.get::<_, i64>(0))
            .unwrap_or(0) > 0
    };

    if !exists(base) {
        return base.to_string();
    }
    if !exists(&format!("{}（副本）", base)) {
        return format!("{}（副本）", base);
    }
    for n in 2..=999 {
        let candidate = format!("{}（副本 {}）", base, n);
        if !exists(&candidate) {
            return candidate;
        }
    }
    format!("{}（副本 {}）", base, uuid::Uuid::new_v4())
}

/// 复制一份教程为新录制（另存为）。
/// - 生成全新的 recording id 与全新的 step id；
/// - **截图文件连带拷贝**到 temp/flowio_screenshots 下的新文件名，避免删副本误删原图；
/// - 副本标题去重：先剥掉源标题结尾的「（副本）/（副本 N）」（含历史叠罗汉），
///   再依次尝试 base / base（副本）/ base（副本 2）…，并在 30 字符内封顶；
/// - 副本沿用源教程的 user_id（归属同一账号），编辑历史/精简快照按新 id 天然隔离 → 改副本不伤原教程；
/// - 编辑器视图偏好（全图/裁图，settings 表 `flowio_view::<rec_id>`）一并复制 → 副本打开时显示状态与源一致；
/// - 译文副本（recordings.translations，按步骤序号键控）一并复制 → 副本无需重新翻译。
/// 返回新录制 id，前端据此 navigate 到副本继续编辑。
#[tauri::command]
pub fn duplicate_recording(db: State<DbState>, id: String) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // 1) 读取源录制（含 user_id 隔离标记、步骤数、译文副本与内容标记）
    let src = conn
        .query_row(
            "SELECT title, app_name, target_app, user_id, total_steps, translations, \
             ai_generated, text_redacted, scan_redacted \
             FROM recordings WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i32>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .map_err(|e| format!("源教程不存在: {}", e))?;

    let new_id = uuid::Uuid::new_v4().to_string();
    let new_title = find_unique_title(&conn, &src.0);

    // 2) 读取源步骤（按 order_index 稳定排序）
    let mut stmt = conn
        .prepare(
            "SELECT id, order_index, step_number, action_type, title, description, tip, \
             screenshot_path, annotated_path, before_screenshot_path, x, y, crop_w, crop_h, \
             after_screenshot_path, timestamp, window_title, is_variable, param_name, mouse_button \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<i32>>(10)?,
                row.get::<_, Option<i32>>(11)?,
                row.get::<_, Option<i32>>(12)?,
                row.get::<_, Option<i32>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, i64>(15)?,
                row.get::<_, String>(16)?,
                row.get::<_, i64>(17)?,
                row.get::<_, String>(18)?,
                row.get::<_, Option<String>>(19)?.unwrap_or_default(),
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut steps: Vec<(
        String, i32, i32, String, String, String, String,
        String, String, String, Option<i32>, Option<i32>, Option<i32>, Option<i32>,
        Option<String>, i64, String, i64, String, String,
    )> = Vec::new();
    for r in rows {
        steps.push(r.map_err(|e| e.to_string())?);
    }

    conn.execute("BEGIN", []).map_err(|e| format!("BEGIN: {}", e))?;

    // 3) 插入副本录制行。translations 译文副本一并复制：JSON 按步骤序号键控，
    //    副本步骤 order_index/step_number 原样保留 → 序号对得上，无需改写。
    //    ai_generated/text_redacted/scan_redacted 是内容事实标记（副本内容与源相同），
    //    必须随内容走，否则副本在列表上丢失「ai/文/扫」徽标、脱敏状态失真。
    conn.execute(
        "INSERT INTO recordings (id, title, app_name, target_app, user_id, status, total_steps, translations, ai_generated, text_redacted, scan_redacted, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'completed', ?6, ?7, ?8, ?9, ?10, datetime('now'), datetime('now'))",
        params![new_id, new_title, src.1, src.2, src.3, steps.len() as i32, src.5, src.6, src.7, src.8],
    )
    .map_err(|e| {
        let _ = conn.execute("ROLLBACK", []);
        format!("复制录制失败: {}", e)
    })?;

    // 4) 逐条插入副本步骤（截图文件连带拷贝），同时收集「源步骤id → 副本步骤id」映射，
    //    供下面复制视图偏好时把逐步骤的「裁图/全图」开关改键使用。
    let mut step_remap: Vec<(String, String)> = Vec::new();
    for st in &steps {
        let new_step_id = uuid::Uuid::new_v4().to_string();
        let new_ss = copy_screenshot_file(&st.7);
        let new_ann = copy_screenshot_file(&st.8);
        let new_before = copy_screenshot_file(&st.9);
        let new_after = copy_screenshot_file(st.14.clone().unwrap_or_default().as_str());
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, title, description, tip, \
             screenshot_path, annotated_path, before_screenshot_path, x, y, crop_w, crop_h, after_screenshot_path, timestamp, window_title, \
             is_variable, param_name, mouse_button) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
            params![
                new_step_id, new_id, st.1, st.2, st.3, st.4, st.5, st.6,
                new_ss, new_ann, new_before, st.10, st.11, st.12, st.13, new_after, st.15, st.16,
                st.17, st.18, st.19
            ],
        )
        .map_err(|e| {
            let _ = conn.execute("ROLLBACK", []);
            format!("复制步骤失败: {}", e)
        })?;
        step_remap.push((st.0.clone(), new_step_id));
    }

    // 5) 复制编辑器视图偏好（全图/裁图显示状态）：settings 表 `flowio_view::<rec_id>`，
    //    并把逐步骤「裁图/全图」开关（stepViewCropped，按步骤 id 键控）从源 id 改键到副本 id
    //    → 副本打开时每步的裁图/全图显示与源完全一致（不会回退成全图）。
    //    源无记录则跳过（新教程默认全图）。best-effort：失败不影响另存为本身。
    let _ = copy_view_preference(&conn, &id, &new_id, &step_remap);

    conn.execute("COMMIT", []).map_err(|e| format!("COMMIT: {}", e))?;
    Ok(new_id)
}

/// 复制编辑器视图偏好（settings 表 `flowio_view::<rec_id>`，另存为时随副本走）。
/// `step_remap` 把「源步骤 id → 副本步骤 id」映射进去，用于把 `stepViewCropped`
/// 这张「步骤id → 是否裁图」的表同步改键——副本步骤是全新 uuid，不改键的话
/// 所有逐步骤的「裁图/全图」开关都会键不中、回退成全图（即「当前截图显示状态」丢失）。
/// 源无记录返回 false（跳过，新教程默认全图）。
pub fn copy_view_preference(
    conn: &Connection,
    src_id: &str,
    dst_id: &str,
    step_remap: &[(String, String)],
) -> bool {
    let src_key = format!("flowio_view::{}", src_id);
    let dst_key = format!("flowio_view::{}", dst_id);
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![src_key],
            |r| r.get(0),
        )
        .ok();
    let value = match value {
        Some(v) => v,
        None => return false,
    };
    // 解析失败则原样照搬（best-effort，不丢数据）；解析成功则把逐步骤开关改键。
    let remapped = remap_view_value(&value, step_remap).unwrap_or_else(|| value.clone());
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![dst_key, remapped],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// 把视图偏好 JSON 里的 `stepViewCropped`（步骤id → 是否裁图）的键从源 id 改为副本 id。
/// 解析失败 / 非对象 → 返回 None（调用方原样照搬）。
fn remap_view_value(value: &str, step_remap: &[(String, String)]) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(value).ok()?;
    if let Some(obj) = v.as_object_mut() {
        if let Some(svc) = obj.get("stepViewCropped") {
            if let Some(map) = svc.as_object() {
                let mut new_map = serde_json::Map::new();
                for (old_id, flag) in map {
                    if let Some((_, new_id)) = step_remap.iter().find(|(o, _)| o == old_id) {
                        new_map.insert(new_id.clone(), flag.clone());
                    }
                }
                obj.insert("stepViewCropped".to_string(), serde_json::Value::Object(new_map));
            }
        }
    }
    serde_json::to_string(&v).ok()
}

/// 把一张截图文件拷贝到 temp/flowio_screenshots 下的新文件名，返回新路径；
/// 源为空或不存在时返回空串（与现有 delete 逻辑一致：空路径跳过）。
fn copy_screenshot_file(src: &str) -> String {
    if src.is_empty() {
        return String::new();
    }
    let src_path = std::path::Path::new(src);
    if !src_path.exists() {
        return String::new();
    }
    let dir = std::env::temp_dir().join("flowio_screenshots");
    let _ = std::fs::create_dir_all(&dir);
    let dst = dir.join(format!("{}.jpg", uuid::Uuid::new_v4()));
    match std::fs::copy(src_path, &dst) {
        Ok(_) => dst.to_string_lossy().to_string(),
        Err(_) => String::new(),
    }
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
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None,
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
            hotkey_combo: String::new(),
            uia_anchor: String::new(),
            mouse_button: String::new(),
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

    // ---------- copy_view_preference（另存为带视图状态） ----------

    #[test]
    fn copy_view_preference_copies_and_reports_missing() {
        let conn = mem_db();
        // 源无记录 → false，且不产生目标 key
        assert!(!copy_view_preference(&conn, "src", "dst", &[]));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings WHERE key = 'flowio_view::dst'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn copy_view_preference_remaps_step_ids() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('flowio_view::src', '{\"stepViewCropped\":{\"old-1\":true,\"old-2\":false},\"globalView\":\"crop\"}')",
            [],
        )
        .unwrap();
        // 副本步骤 id 是全新 uuid，必须按序改键
        let remap = vec![
            ("old-1".to_string(), "new-1".to_string()),
            ("old-2".to_string(), "new-2".to_string()),
        ];
        assert!(copy_view_preference(&conn, "src", "dst", &remap));
        let v: String = conn
            .query_row("SELECT value FROM settings WHERE key = 'flowio_view::dst'", [], |r| r.get(0))
            .unwrap();
        // 改键成功：新 id 出现、旧 id 消失、全局视图保留
        assert!(v.contains("new-1") && v.contains("new-2"));
        assert!(!v.contains("old-1") && !v.contains("old-2"));
        assert!(v.contains("globalView") && v.contains("crop"));
        // 值跟随：new-1 应为 true（裁图），new-2 应为 false
        assert!(v.contains("\"new-1\":true"));
        assert!(v.contains("\"new-2\":false"));
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

    #[test]
    fn find_unique_title_no_collision_returns_base() {
        let conn = mem_db();
        assert_eq!(find_unique_title(&conn, "hello"), "hello");
    }

    #[test]
    fn find_unique_title_one_collision_appends_copy() {
        let conn = mem_db();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('a', 'hello')", []).unwrap();
        assert_eq!(find_unique_title(&conn, "hello"), "hello（副本）");
    }

    #[test]
    fn find_unique_title_two_collisions_appends_number() {
        let conn = mem_db();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('a', 'hello')", []).unwrap();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('b', 'hello（副本）')", []).unwrap();
        assert_eq!(find_unique_title(&conn, "hello"), "hello（副本 2）");
    }

    #[test]
    fn find_unique_title_many_collisions_increments() {
        let conn = mem_db();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('a', 'x')", []).unwrap();
        for i in 1..=5 {
            let t = if i == 1 { "x（副本）" } else { &format!("x（副本 {}）", i) };
            conn.execute("INSERT INTO recordings (id, title) VALUES (?, ?)", params![format!("d{}", i), t]).unwrap();
        }
        // base + (copy) + (copy2..5) = 7 existing => next should be (copy 6)
        assert_eq!(find_unique_title(&conn, "x"), "x（副本 6）");
    }

    #[test]
    fn find_unique_title_base_already_has_copy_suffix() {
        let conn = mem_db();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('a', 'hello')", []).unwrap();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('b', 'hello（副本）')", []).unwrap();
        // base already has (copy) suffix -> strip to "hello", then find next -> hello(copy 2)
        assert_eq!(find_unique_title(&conn, "hello（副本）"), "hello（副本 2）");
    }

    #[test]
    fn find_unique_title_base_already_has_copy_number_suffix() {
        let conn = mem_db();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('a', 'demo')", []).unwrap();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('b', 'demo（副本）')", []).unwrap();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('c', 'demo（副本 2）')", []).unwrap();
        // base="demo(copy 2)" stripped to "demo", existing: demo, demo(copy), demo(copy 2)
        assert_eq!(find_unique_title(&conn, "demo（副本 2）"), "demo（副本 3）");
    }

    #[test]
    fn strip_copy_suffix_basic() {
        assert_eq!(strip_copy_suffix("X"), "X");
        assert_eq!(strip_copy_suffix("X（副本）"), "X");
        assert_eq!(strip_copy_suffix("X（副本 2）"), "X");
        assert_eq!(strip_copy_suffix("X（副本 99）"), "X");
        // not a copy suffix - no change
        assert_eq!(strip_copy_suffix("X（其他）"), "X（其他）");
        assert_eq!(strip_copy_suffix("X（副本abc）"), "X（副本abc）");
    }

    #[test]
    fn strip_copy_suffix_stacked_legacy_titles() {
        // 历史 bug 产生的叠罗汉标题：循环剥离一次剥干净
        assert_eq!(strip_copy_suffix("X（副本）（副本）"), "X");
        assert_eq!(strip_copy_suffix("X（副本）（副本 2）"), "X");
        assert_eq!(strip_copy_suffix("X（副本）（副本）（副本 3）"), "X");
    }

    #[test]
    fn strip_copy_suffix_halfwidth_legacy_variants() {
        // 历史半角变体同样剥掉（含叠罗汉与混合宽度）
        assert_eq!(strip_copy_suffix("X (副本)"), "X");
        assert_eq!(strip_copy_suffix("X (副本 2)"), "X");
        assert_eq!(strip_copy_suffix("X (副本)(副本)"), "X");
        assert_eq!(strip_copy_suffix("X (副本)（副本 2）"), "X");
        // 半角但不是副本后缀 → 不动
        assert_eq!(strip_copy_suffix("X (其他)"), "X (其他)");
        assert_eq!(strip_copy_suffix("X (副本abc)"), "X (副本abc)");
    }

    #[test]
    fn find_unique_title_stacked_source_gets_clean_numbered_title() {
        let conn = mem_db();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('a', 'demo')", []).unwrap();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('b', 'demo（副本）')", []).unwrap();
        conn.execute("INSERT INTO recordings (id, title) VALUES ('c', 'demo（副本）（副本）')", []).unwrap();
        // 对叠罗汉源再另存为：剥干净后落到第一个空位（副本 2），不再叠第四层
        assert_eq!(find_unique_title(&conn, "demo（副本）（副本）"), "demo（副本 2）");
    }

    #[test]
    fn cap_title_chars_limits_and_keeps_utf8_safe() {
        assert_eq!(cap_title_chars("短标题"), "短标题");
        let long = "录".repeat(50);
        assert_eq!(cap_title_chars(&long).chars().count(), 30);
        // 截断落在「（」上时应清理悬挂括号
        let tricky: String = "录".repeat(29) + "（副本）";
        assert_eq!(cap_title_chars(&tricky), "录".repeat(29));
    }
}
