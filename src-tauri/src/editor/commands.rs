use base64::Engine;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

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
    /// 该步骤输入值是否为变量（每次运行都会变，SKILL.md 导出用）
    #[serde(default)]
    pub is_variable: bool,
    /// 变量参数名（如 order_id），is_variable 为 true 时有意义
    #[serde(default)]
    pub param_name: String,
    /// 鼠标按键（"left"/"right"，空=left）。右键步骤重放下发右键。
    #[serde(default)]
    pub mouse_button: String,
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
             x, y, crop_w, crop_h, after_screenshot_path, timestamp, window_title, \
             is_variable, param_name, mouse_button \
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
                is_variable: row.get::<_, i64>(19)? != 0,
                param_name: row.get(20)?,
                mouse_button: row.get::<_, Option<String>>(21)?.unwrap_or_default(),
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
///
/// `is_variable` / `param_name` 为可选：不传（None）时保留数据库现值（COALESCE），
/// 老调用方（翻译同步、标题/描述自动保存）无需感知变量字段。
#[tauri::command]
pub fn update_step(
    db: State<DbState>,
    step_id: String,
    title: String,
    description: String,
    action_type: String,
    tip: String,
    is_variable: Option<bool>,
    param_name: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    push_edit_history_for_step(&conn, &step_id);
    conn.execute(
        "UPDATE steps SET title = ?1, description = ?2, action_type = ?3, tip = ?4, \
         is_variable = COALESCE(?5, is_variable), param_name = COALESCE(?6, param_name) \
         WHERE id = ?7",
        params![title, description, action_type, tip, is_variable, param_name, step_id],
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
    push_edit_history_for_step(&conn, &step_id);
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
    // 删除前先压栈（此时还能反查 recording_id）
    push_edit_history_for_step(&conn, &step_id);
    let recording_id: String = conn
        .query_row(
            "SELECT recording_id FROM steps WHERE id = ?1",
            params![step_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM steps WHERE id = ?1", params![step_id])
        .map_err(|e| e.to_string())?;
    // ── 删除后重排 order_index / step_number（与合并流程同口径）──
    // 之前只 DELETE 不重排：DB 里序号留空洞（1,2,4,5…），前端仅本地重编号
    // 未落库，一导出就露馅（2026-09-09 用户实测：删两步后导出文件序号全错）。
    let mut re_stmt = conn
        .prepare("SELECT id FROM steps WHERE recording_id = ?1 ORDER BY order_index")
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = re_stmt
        .query_map(params![recording_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(re_stmt);
    for (i, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE steps SET order_index = ?1, step_number = ?2 WHERE id = ?3",
            params![i as i32, i as i32 + 1, id],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "UPDATE recordings SET total_steps = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![ids.len() as i32, recording_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 查找替换结果：被修改的步骤数 + 替换总处数。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceResult {
    pub changed_steps: usize,
    pub replaced: usize,
}

/// 大小写不敏感替换（与前端 JS 版语义一致）：返回 (新文本, 替换处数)。
/// 仅当 to_lowercase 不改变字节长度时走精确字节匹配（ASCII+中文恒成立）；
/// 罕见 Unicode（如 'İ'）会改变长度 → 退化为大小写敏感 replace，保证索引安全。
fn replace_ci(hay: &str, search: &str, rep: &str) -> (String, usize) {
    if search.is_empty() {
        return (hay.to_string(), 0);
    }
    let hay_lc = hay.to_lowercase();
    let search_lc = search.to_lowercase();
    if hay_lc.len() != hay.len() || search_lc.len() != search.len() {
        let n = hay.matches(search).count();
        return (hay.replace(search, rep), n);
    }
    let hay_b = hay.as_bytes();
    let hay_lcb = hay_lc.as_bytes();
    let search_b = search_lc.as_bytes();
    let mut out = String::with_capacity(hay.len() + 16);
    let mut count = 0usize;
    let mut last = 0usize;
    let mut i = 0usize;
    while i + search_b.len() <= hay_b.len() {
        if &hay_lcb[i..i + search_b.len()] == search_b {
            out.push_str(&hay[last..i]);
            out.push_str(rep);
            count += 1;
            i += search_b.len();
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&hay[last..]);
    (out, count)
}

/// 批量查找替换：在指定教程所有步骤的 title/description 中把 search 替换为 replace。
/// 大小写不敏感（查找与替换同口径）；整个操作只压**一次**撤销快照——
/// 用户 Ctrl+Z 一次即可整体还原，不会像逐个 update_step 那样产生 N 层撤销。
#[tauri::command]
pub fn replace_in_steps(
    db: State<DbState>,
    recording_id: String,
    search: String,
    replace: String,
) -> Result<ReplaceResult, String> {
    if search.trim().is_empty() {
        return Err("查找内容不能为空".into());
    }
    let mut conn = db.0.lock().map_err(|e| e.to_string())?;

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            params![recording_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Err("教程不存在".into());
    }

    // 读出全部步骤文本，先在内存里算好要改哪些行
    let mut stmt = conn
        .prepare("SELECT id, title, description FROM steps WHERE recording_id = ?1 ORDER BY order_index")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map(params![recording_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    let mut updates: Vec<(String, String, String)> = Vec::new(); // (step_id, new_title, new_desc)
    let mut replaced_total = 0usize;
    for (id, title, desc) in rows {
        let (nt, c1) = replace_ci(&title, &search, &replace);
        let (nd, c2) = replace_ci(&desc, &search, &replace);
        if c1 + c2 > 0 {
            replaced_total += c1 + c2;
            updates.push((id, nt, nd));
        }
    }
    if updates.is_empty() {
        return Ok(ReplaceResult { changed_steps: 0, replaced: 0 });
    }

    // 一次性撤销快照（替换前状态）→ 单事务写回
    push_edit_history(&conn, &recording_id)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for (id, nt, nd) in &updates {
        tx.execute(
            "UPDATE steps SET title = ?1, description = ?2 WHERE id = ?3",
            params![nt, nd, id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE recordings SET updated_at = datetime('now') WHERE id = ?1",
        params![recording_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ReplaceResult {
        changed_steps: updates.len(),
        replaced: replaced_total,
    })
}

/// Batch-update the order_index of steps for a recording.
#[tauri::command]
pub fn reorder_steps(
    db: State<DbState>,
    _recording_id: String,
    step_ids: Vec<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = push_edit_history(&conn, &_recording_id);
    for (i, step_id) in id_order_updates(&step_ids) {
        conn.execute(
            "UPDATE steps SET order_index = ?1 WHERE id = ?2",
            params![i, step_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ★ P2 手动合并：编辑器多选若干步骤 → 合并为一步。
///
/// 合并语义（与 PRD 验收一致）：
/// - 主图 / 裁剪框 / after 图 / 标注图取**末步**（最终状态）；
/// - 操作前图（before_screenshot_path）取**首步**（操作发生前的画面）；
/// - 标题 / action_type / tip / timestamp / window_title 取首步；
/// - 描述按 order 顺序以换行拼接（合并后可再编辑）；
/// - 其余被选步骤删除，剩余步骤 order_index 重排为 0..n-1（step_number 同步）。
///
/// 导出 / AI / 发布链路读到的是普通步骤行，无需特殊兼容。
#[tauri::command]
pub fn merge_steps(
    db: State<DbState>,
    recording_id: String,
    step_ids: Vec<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = push_edit_history(&conn, &recording_id);
    merge_steps_conn(&conn, &recording_id, &step_ids)
}

/// merge_steps 的核心逻辑（拆出以便单测，直接吃连接）。
pub(crate) fn merge_steps_conn(
    conn: &rusqlite::Connection,
    recording_id: &str,
    step_ids: &[String],
) -> Result<(), String> {
    if step_ids.len() < 2 {
        return Err("至少选择两个步骤才能合并".to_string());
    }
    let rows = load_merge_steps(conn, recording_id)?;
    for id in step_ids {
        if !rows.iter().any(|r| r.0 == *id) {
            return Err(format!("步骤 {} 不属于该录制或不存在", id));
        }
    }
    // 手动合并语义：描述按 order 顺序换行拼接（rows 已按 order_index 升序）
    let joined_desc: String = rows
        .iter()
        .filter(|r| step_ids.contains(&r.0))
        .map(|r| r.3.trim())
        .filter(|d| !d.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    apply_merge_plan(
        conn,
        recording_id,
        &[MergeGroup {
            ids: step_ids.to_vec(),
            desc: Some(joined_desc),
            title: None,
        }],
        &[],
        true,
    )
    .map(|_| ())
}

// ══════════════════════════════════════════════════════════════════
// 步骤自动精简（2026-09-03）
//
// 用户诉求：「不要手动合并，太费事；录完自动就是干净的」。
//
// 分两层，共用同一套落库执行器 `apply_merge_plan`：
//   · 规则层 `plan_auto_merge` —— 纯函数、零延迟、零成本，录制结束自动跑。
//     实测降噪 15%~23%（重复点击 / 连续输入 / 点击后输入 / 删掉停止按钮那一步）。
//   · AI 层 `simplify_steps_ai` —— 编辑器一键，让模型输出「相邻步骤分区方案」
//     （不是自由改写），校验通过后按同样方式落库。能压下弹框里那一串点击。
//
// 两层都先写全量快照到 `step_snapshots`，前端提供「撤销」一键还原。
// ══════════════════════════════════════════════════════════════════

/// 合并/精简落库用的步骤行（steps 表全字段的子集，够用即可）。
type MergeStepRow = (
    String,          // 0 id
    String,          // 1 action_type
    String,          // 2 title
    String,          // 3 description
    String,          // 4 tip
    String,          // 5 screenshot_path
    String,          // 6 annotated_path
    String,          // 7 before_screenshot_path
    Option<String>,  // 8 after_screenshot_path
    Option<i32>,     // 9 x
    Option<i32>,     // 10 y
    Option<i32>,     // 11 crop_w
    Option<i32>,     // 12 crop_h
    i64,             // 13 timestamp
    String,          // 14 window_title
);

/// 一次合并组：`ids` 中第一个是承载行，其余删除。
///
/// `desc = Some(t)` → 覆盖描述（AI 精简 / 手动拼接）；
/// `desc = None`    → 取组内末步的非空描述（规则层：输入类描述本身是累积全文）。
pub(crate) struct MergeGroup {
    pub ids: Vec<String>,
    pub desc: Option<String>,
    /// 覆盖标题（规则层 R1 把「点击」改成「双击」）；None = 保持首步标题
    pub title: Option<String>,
}

/// 读出某录制的全部步骤（按 order_index 升序）。
fn load_merge_steps(
    conn: &rusqlite::Connection,
    recording_id: &str,
) -> Result<Vec<MergeStepRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, action_type, title, description, tip, screenshot_path, \
             annotated_path, before_screenshot_path, after_screenshot_path, \
             x, y, crop_w, crop_h, timestamp, window_title \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![recording_id], |r| {
            Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?,
                r.get(13)?, r.get::<_, Option<String>>(14)?.unwrap_or_default(),
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect::<Vec<MergeStepRow>>();
    Ok(rows)
}

/// 合并组的「视觉承载行」：组内最后一个输入类步骤（type/paste）的 id。
///
/// 输入步骤的截图 = 填完之后的界面状态；合并若一律取组末步，
/// 「填完 A 框又点 B 框」的组就会把 A 填完的截图跳掉，读者无法照做。
/// 组内没有输入步骤时返回 None（承载行退回组末步）。
/// 纯函数：`actions` 与 `ids` 一一对应。
pub(crate) fn last_input_step_index(ids: &[String], actions: &[&str]) -> Option<usize> {
    ids.iter()
        .enumerate()
        .filter(|(i, _)| matches!(actions.get(*i).copied().unwrap_or(""), "type" | "paste"))
        .map(|(i, _)| i)
        .last()
}

/// 把合并方案落到数据库（事务内）。
///
/// - 每组首步承载结果：主图 / 标注图 / after 图 / 裁剪框 ← 组内**末步**（最终画面），
///   操作前图 / 标题 / action_type / tip / timestamp / window_title 保持**首步**。
///   ⚠️ timestamp 取首步而非末步：语音切片按步骤起点切，取末步会丢掉整段输入的录音。
///   ⚠️ 2026-09-05 起例外：组内含输入步骤（type/paste）时，截图/坐标改取**最后一个
///   输入步骤**（填完后的界面状态），见 [`last_input_step_index`]。
/// - `removed` 里的步骤直接删除（不保留任何行）。
/// - 最后重排 order_index / step_number，并同步 `recordings.total_steps`。
///
/// 返回被合并掉（减少）的步骤数。
pub(crate) fn apply_merge_plan(
    conn: &rusqlite::Connection,
    recording_id: &str,
    groups: &[MergeGroup],
    removed: &[String],
    snapshot: bool,
) -> Result<usize, String> {
    let rows = load_merge_steps(conn, recording_id)?;
    if rows.is_empty() {
        return Err("该录制没有步骤".to_string());
    }
    let by_id: std::collections::HashMap<&str, &MergeStepRow> =
        rows.iter().map(|r| (r.0.as_str(), r)).collect();

    // ── 校验：id 必须存在且互不重复 ────────────────────────────
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for g in groups {
        for id in &g.ids {
            if !by_id.contains_key(id.as_str()) {
                return Err(format!("步骤 {} 不属于该录制或不存在", id));
            }
            if !seen.insert(id.as_str()) {
                return Err(format!("步骤 {} 在合并方案里重复出现", id));
            }
        }
    }
    for id in removed {
        if !by_id.contains_key(id.as_str()) {
            return Err(format!("步骤 {} 不属于该录制或不存在", id));
        }
        if !seen.insert(id.as_str()) {
            return Err(format!("步骤 {} 在合并方案里重复出现", id));
        }
    }

    let before = rows.len();
    if snapshot {
        snapshot_steps(conn, recording_id, "merge")?;
    }

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

    for g in groups {
        if g.ids.is_empty() {
            continue;
        }
        let first = by_id[g.ids[0].as_str()];
        let last = by_id[g.ids.last().unwrap().as_str()];
        // 视觉承载行：组内含输入类步骤（type/paste）时，截图/坐标取**最后一个输入步骤**
        // ——它的截图是「填完之后」的界面状态。一律取组末步会把填值过程的截图跳掉
        // （比如填完查找值又点下一格，读者就看不到查找值填完后的样子），2026-09-05 用户反馈。
        let actions: Vec<&str> = g.ids.iter().map(|id| by_id[id.as_str()].1.as_str()).collect();
        let carrier = last_input_step_index(&g.ids, &actions)
            .map(|i| by_id[g.ids[i].as_str()])
            .unwrap_or(last);
        let desc = match &g.desc {
            Some(d) => d.clone(),
            None if !last.3.trim().is_empty() => last.3.clone(),
            None => first.3.clone(),
        };
        tx.execute(
            "UPDATE steps SET description = ?1, screenshot_path = ?2, annotated_path = ?3, \
             after_screenshot_path = ?4, x = ?5, y = ?6, crop_w = ?7, crop_h = ?8 \
             WHERE id = ?9",
            params![
                desc, carrier.5, carrier.6, carrier.8,
                carrier.9, carrier.10, carrier.11, carrier.12, first.0,
            ],
        )
        .map_err(|e| e.to_string())?;
        if let Some(t) = &g.title {
            tx.execute("UPDATE steps SET title = ?1 WHERE id = ?2", params![t, first.0])
                .map_err(|e| e.to_string())?;
        }

        for id in &g.ids[1..] {
            tx.execute("DELETE FROM steps WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
    }
    for id in removed {
        tx.execute("DELETE FROM steps WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }

    // ── 重排 order_index / step_number ─────────────────────────
    let mut re_stmt = tx
        .prepare("SELECT id FROM steps WHERE recording_id = ?1 ORDER BY order_index")
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = re_stmt
        .query_map(params![recording_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(re_stmt);
    for (i, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE steps SET order_index = ?1, step_number = ?2 WHERE id = ?3",
            params![i as i32, i as i32 + 1, id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE recordings SET total_steps = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![ids.len() as i32, recording_id],
    )
    .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(before - ids.len())
}

/// 精简前把当前全部步骤序列化成快照（每个录制只留最近 3 条）。
fn snapshot_steps(
    conn: &rusqlite::Connection,
    recording_id: &str,
    kind: &str,
) -> Result<(), String> {
    let rows = load_merge_steps(conn, recording_id)?;
    let payload = serde_json::to_string(&rows).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO step_snapshots (id, recording_id, kind, payload, step_count) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            uuid::Uuid::new_v4().to_string(),
            recording_id,
            kind,
            payload,
            rows.len() as i32
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM step_snapshots WHERE recording_id = ?1 AND id NOT IN \
         (SELECT id FROM step_snapshots WHERE recording_id = ?2 ORDER BY created_at DESC LIMIT 3)",
        params![recording_id, recording_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── 规则层（纯函数，便于单测）────────────────────────────────

/// 规则降噪用的轻量步骤视图。
#[derive(Debug, Clone, PartialEq)]
pub struct MergeStepLite {
    pub action_type: String,
    pub title: String,
    pub window_title: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub timestamp: i64,
}

/// 规则阈值。
///
/// 数值来自 2026-09-03 对 4 份真实 WPS 录制的实测回放：
/// 三连击间隔 0.2~0.5s（故 click_gap 取 1100ms 覆盖三连击累计），
/// 同一输入框的连续输入间隔 1.8~8.3s（故 type_gap 取 15s，靠同坐标约束防误伤）。
#[derive(Debug, Clone)]
pub struct AutoMergeRules {
    /// 相邻两次点击间隔上限（ms）→ 视为重复点击
    pub click_gap_ms: i64,
    /// 相邻两次输入间隔上限（ms）→ 视为同一输入框的连续输入
    pub type_gap_ms: i64,
    /// 「点击 → 输入」间隔上限（ms）→ 合并为「点击并输入」
    pub pair_gap_ms: i64,
    /// 点击/输入判同位置的像素容差
    pub pos_px: i32,
    /// 「点击 → 输入」判同位置的像素容差（输入框点击点与光标位置略有偏差）
    pub pair_pos_px: i32,
    /// 自家窗口标题关键字（录到这些窗口的步骤视为操作停止按钮，直接删除）
    pub self_app_markers: Vec<String>,
}

impl Default for AutoMergeRules {
    fn default() -> Self {
        Self {
            click_gap_ms: 1_100,
            type_gap_ms: 15_000,
            pair_gap_ms: 6_000,
            pos_px: 8,
            pair_pos_px: 24,
            self_app_markers: vec!["录步".to_string(), "Flowio".to_string()],
        }
    }
}

/// 合并方案（索引版；纯函数产出，与具体步骤 id 解耦，便于单测）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergePlan {
    /// 每个元素是一组「要合并成一步」的原始步骤下标（组内按升序）
    pub groups: Vec<Vec<usize>>,
    /// 直接删除的步骤下标（不保留任何行）
    pub removed: Vec<usize>,
    /// 与 groups 一一对应的新描述；None = 沿用组内末步描述
    pub descs: Vec<Option<String>>,
    /// 与 groups 一一对应的新标题；None = 保持首步标题
    pub titles: Vec<Option<String>>,
}

impl MergePlan {
    /// 精简后减少的步骤数 = 原步数 − 保留下来的组数
    ///（`removed` 里的步骤不产生行，已体现在「没进 groups」上，不可再减一次）。
    pub fn reduced(&self, total: usize) -> usize {
        total.saturating_sub(self.groups.len())
    }
}

/// 两个坐标是否足够接近（缺坐标视为不接近，避免把无坐标步骤误并）。
fn near_pos(a: (&Option<i32>, &Option<i32>), b: (&Option<i32>, &Option<i32>), px: i32) -> bool {
    match (a.0, a.1, b.0, b.1) {
        (Some(ax), Some(ay), Some(bx), Some(by)) => (ax - bx).abs() <= px && (ay - by).abs() <= px,
        _ => false,
    }
}

/// 规则降噪：贪心扫描相邻步骤，可合并就并，直到没有新的合并发生。
///
/// 规则（按优先级）：
/// - **R1 重复点击**：click→click，同窗口、坐标 ≤8px、间隔 ≤1.1s → 并成一步（标题改「双击」）。
/// - **R2 连续输入**：type→type，同窗口、坐标 ≤8px、间隔 ≤15s → 并成一步（描述取后者，
///   因为输入框描述是累积全文，拼接会重复）。
/// - **R3 点击后输入**：click→type，同窗口、坐标 ≤24px、间隔 ≤6s → 并成一步「在…输入」。
///   合并后该组的 action 视为 type，可继续参与 R2（正是「点输入框后连打几段」的场景）。
/// - **R4 删除收尾**：末步若落在自家窗口（点的是「停止录制」）→ 删除。
///
/// 不会合并：跨窗口、跨坐标、快捷键/复制/粘贴/导航、密码等敏感步骤（不在此视图内）。
pub fn plan_auto_merge(steps: &[MergeStepLite], rules: &AutoMergeRules) -> MergePlan {
    if steps.is_empty() {
        return MergePlan::default();
    }

    #[derive(Clone)]
    struct Acc {
        idx: Vec<usize>,
        action: String,
        title: String,
        win: String,
        x: Option<i32>,
        y: Option<i32>,
        ts: i64,
        doubled: bool,
    }
    let mut acc: Vec<Acc> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| Acc {
            idx: vec![i],
            action: s.action_type.clone(),
            title: s.title.clone(),
            win: s.window_title.clone(),
            x: s.x,
            y: s.y,
            ts: s.timestamp,
            doubled: false,
        })
        .collect();

    loop {
        let mut merged_any = false;
        let mut i = 0usize;
        while i + 1 < acc.len() {
            // split_at_mut 取得 &mut a 与 &b，绕开双重可变借用
            let (left, right) = acc.split_at_mut(i + 1);
            let a = &mut left[i];
            let b = &right[0];

            if a.win != b.win {
                i += 1;
                continue;
            }
            let gap = b.ts - a.ts; // a.ts 恒为组内首步时间 → 累计间隔，三连击也能串上
            if gap < 0 {
                i += 1;
                continue; // 时间戳异常，不猜
            }
            let near = near_pos((&a.x, &a.y), (&b.x, &b.y), rules.pos_px);
            let pair_near = near_pos((&a.x, &a.y), (&b.x, &b.y), rules.pair_pos_px);

            let rule = if a.action == "click" && b.action == "click" && near && gap <= rules.click_gap_ms {
                Some(1u8)
            } else if a.action == "type" && b.action == "type" && near && gap <= rules.type_gap_ms {
                Some(2)
            } else if a.action == "click" && b.action == "type" && pair_near && gap <= rules.pair_gap_ms {
                Some(3)
            } else {
                None
            };

            match rule {
                Some(1) => {
                    if !a.doubled {
                        a.title = a.title.replacen("点击", "双击", 1);
                        a.doubled = true; // 标题只改一次，三连击不会变「双击双击」
                    }
                    a.x = b.x;
                    a.y = b.y;
                    a.idx.extend(b.idx.iter().copied());
                }
                Some(2) | Some(3) => {
                    if rule == Some(3) {
                        a.action = "type".to_string();
                    }
                    a.x = b.x;
                    a.y = b.y;
                    a.idx.extend(b.idx.iter().copied());
                }
                _ => {
                    i += 1;
                    continue;
                }
            }
            acc.remove(i + 1);
            merged_any = true;
        }
        if !merged_any {
            break;
        }
    }

    // R4：末步落在自家窗口 → 视为「点停止录制」，删掉
    let mut removed: Vec<usize> = Vec::new();
    if let Some(last) = acc.last() {
        let last_idx = *last.idx.last().unwrap();
        let is_self = rules
            .self_app_markers
            .iter()
            .any(|m| steps[last_idx].window_title.contains(m.as_str()));
        if is_self {
            removed.push(last_idx);
            acc.pop();
        }
    }

    let titles: Vec<Option<String>> = acc
        .iter()
        .map(|a| if a.doubled { Some(a.title.clone()) } else { None })
        .collect();
    MergePlan {
        groups: acc.into_iter().map(|a| a.idx).collect(),
        removed,
        descs: Vec::new(),
        titles,
    }
}

/// 把索引版方案按当前步骤列表翻译成 id 版（供落库使用）。
fn plan_to_groups(plan: &MergePlan, rows: &[MergeStepRow]) -> Vec<MergeGroup> {
    plan.groups
        .iter()
        .enumerate()
        .filter_map(|(gi, idxs)| {
            let ids: Vec<String> = idxs
                .iter()
                .filter_map(|&i| rows.get(i).map(|r| r.0.clone()))
                .collect();
            if ids.is_empty() {
                None
            } else {
                // ⚠️ 单步组：只要原描述非空就**不重写**。
                // 用户可能已经跑过「AI 生成步骤」（描述是几十字的详细文案），
                // AI 精简的职责是合并，不该把没合并的步骤描述也冲成一句话。
                let desc = if idxs.len() == 1 {
                    let existing = rows
                        .get(idxs[0])
                        .map(|r| r.3.trim().to_string())
                        .unwrap_or_default();
                    if existing.is_empty() {
                        plan.descs.get(gi).cloned().flatten()
                    } else {
                        None
                    }
                } else {
                    plan.descs.get(gi).cloned().flatten()
                };
                Some(MergeGroup {
                    ids,
                    desc,
                    // ★ 精简产出的标题同样防撑爆（LLM 软约束，落库前兜底截 10 字）
                    title: plan
                        .titles
                        .get(gi)
                        .cloned()
                        .flatten()
                        .map(|t| {
                            crate::recorder::activity::truncate_label(
                                &t,
                                crate::recorder::activity::TITLE_LABEL_MAX_CHARS,
                            )
                        }),
                })
            }
        })
        .collect()
}

/// 规则层落库入口（录制结束时自动调用；编辑器「一键精简」也走这里）。
pub(crate) fn auto_merge_steps_conn(
    conn: &rusqlite::Connection,
    recording_id: &str,
) -> Result<SimplifyResult, String> {
    let rows = load_merge_steps(conn, recording_id)?;
    let before = rows.len();
    if before < 2 {
        return Ok(SimplifyResult {
            before,
            after: before,
            reduced: 0,
            kind: "rules".to_string(),
        });
    }
    let lite: Vec<MergeStepLite> = rows
        .iter()
        .map(|r| MergeStepLite {
            action_type: r.1.clone(),
            title: r.2.clone(),
            window_title: r.14.clone(),
            x: r.9,
            y: r.10,
            timestamp: r.13,
        })
        .collect();
    let plan = plan_auto_merge(&lite, &AutoMergeRules::default());
    let reduced = plan.reduced(before);
    if reduced == 0 {
        return Ok(SimplifyResult {
            before,
            after: before,
            reduced: 0,
            kind: "rules".to_string(),
        });
    }
    let groups = plan_to_groups(&plan, &rows);
    let removed: Vec<String> = plan
        .removed
        .iter()
        .filter_map(|&i| rows.get(i).map(|r| r.0.clone()))
        .collect();
    let applied = apply_merge_plan(conn, recording_id, &groups, &removed, true)?;
    Ok(SimplifyResult {
        before,
        after: before - applied,
        reduced: applied,
        kind: "rules".to_string(),
    })
}

/// 精简结果（返回给前端做 toast）。
#[derive(Debug, Clone, Serialize)]
pub struct SimplifyResult {
    pub before: usize,
    pub after: usize,
    pub reduced: usize,
    pub kind: String,
}

#[tauri::command]
pub fn auto_merge_steps(db: State<DbState>, recording_id: String) -> Result<SimplifyResult, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    push_edit_history(&conn, &recording_id)?;
    auto_merge_steps_conn(&conn, &recording_id)
}

// ── 全局撤销/重做（WPS 式 ↶↷）────────────────────────────────
//
// 设计：**快照由后端在每个写命令入口自动捕获**，前端不参与快照纪律——
// 这样无论现在还是将来加新的写命令，只要记得在入口调一次 push_edit_history，
// 撤销就自动覆盖；且快照直接从 DB 行读出、直接写回 DB 行，
// 不存在 Rust 结构体 ↔ TS 类型的键名转换（那正是事件载荷踩过的坑）。
//
// 两个栈存在 edit_history 表：stack='undo' / 'redo'，栈内按 rowid（自增）排序。
// · push_edit_history：当前状态压入 undo 栈，并清空 redo 栈（新编辑使 redo 失效）。
// · edit_undo：把当前状态压入 redo 栈 → 还原 undo 栈顶 → 删掉栈顶那条。
// · edit_redo：对称。
// 限制（v1）：图片文件本身不做版本管理——删除截图后再撤销会恢复数据库引用
// 但文件已不在磁盘上，restore 时会把不存在的文件路径置空（图片缺失但不悬空）；
// 裁剪/上传会覆盖同名 _cropped.jpg / 新文件，撤销无法还原旧图内容。

/// 快照里的单条步骤（覆盖 steps 表除 created_at 外的全部业务列）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EditorSnapshotStep {
    id: String,
    order_index: i32,
    step_number: i32,
    action_type: String,
    title: String,
    description: String,
    tip: String,
    screenshot_path: String,
    annotated_path: String,
    before_screenshot_path: String,
    after_screenshot_path: Option<String>,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    timestamp: i64,
    window_title: Option<String>,
    is_variable: bool,
    param_name: String,
    /// 鼠标按键（"left"/"right"，空=left）。旧快照 JSON 缺该字段 → serde default ''。
    #[serde(default)]
    mouse_button: String,
}

/// 一次快照 = 录制标题 + 全部步骤行。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EditorSnapshot {
    title: String,
    steps: Vec<EditorSnapshotStep>,
}

/// 撤销/重做栈深度（前端按钮显隐用）。
#[derive(Debug, Clone, Serialize)]
pub struct HistSizes {
    pub undo: i64,
    pub redo: i64,
}

const EDIT_HISTORY_LIMIT: i64 = 50;

/// 从 DB 读出当前完整编辑状态（录制标题 + 全部步骤行）。
fn capture_editor_snapshot(
    conn: &rusqlite::Connection,
    recording_id: &str,
) -> Result<EditorSnapshot, String> {
    let title: String = conn
        .query_row(
            "SELECT title FROM recordings WHERE id = ?1",
            params![recording_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("读取录制标题失败: {}", e))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, order_index, step_number, action_type, title, description, tip, \
             screenshot_path, annotated_path, before_screenshot_path, after_screenshot_path, \
             x, y, crop_w, crop_h, timestamp, window_title, is_variable, param_name, mouse_button \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index ASC",
        )
        .map_err(|e| e.to_string())?;
    let steps = stmt
        .query_map(params![recording_id], |r| {
            Ok(EditorSnapshotStep {
                id: r.get(0)?,
                order_index: r.get(1)?,
                step_number: r.get(2)?,
                action_type: r.get(3)?,
                title: r.get(4)?,
                description: r.get(5)?,
                tip: r.get(6)?,
                screenshot_path: r.get(7)?,
                annotated_path: r.get(8)?,
                before_screenshot_path: r.get(9)?,
                after_screenshot_path: r.get(10)?,
                x: r.get(11)?,
                y: r.get(12)?,
                crop_w: r.get(13)?,
                crop_h: r.get(14)?,
                timestamp: r.get(15)?,
                window_title: r.get(16)?,
                is_variable: r.get::<_, i64>(17)? != 0,
                param_name: r.get(18)?,
                mouse_button: r.get::<_, Option<String>>(19)?.unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(EditorSnapshot { title, steps })
}

/// 把「当前状态」压入 undo 栈并清空 redo 栈（每次写操作前调用）。
pub(crate) fn push_edit_history(
    conn: &rusqlite::Connection,
    recording_id: &str,
) -> Result<(), String> {
    let snap = capture_editor_snapshot(conn, recording_id)?;
    let payload = serde_json::to_string(&snap).map_err(|e| e.to_string())?;
    let n = snap.steps.len() as i64;
    conn.execute(
        "INSERT INTO edit_history (id, recording_id, stack, payload, step_count) \
         VALUES (?1, ?2, 'undo', ?3, ?4)",
        params![uuid::Uuid::new_v4().to_string(), recording_id, payload, n],
    )
    .map_err(|e| e.to_string())?;
    // 新编辑使 redo 失效
    conn.execute(
        "DELETE FROM edit_history WHERE recording_id = ?1 AND stack = 'redo'",
        params![recording_id],
    )
    .map_err(|e| e.to_string())?;
    prune_edit_history(conn, recording_id, "undo")
}

/// 步骤级写命令用的包装：先由 step_id 反查 recording_id 再压栈。
/// 步骤已不存在（并发删除等）时静默跳过——撤销少一条总比写命令直接报错好。
pub(crate) fn push_edit_history_for_step(
    conn: &rusqlite::Connection,
    step_id: &str,
) {
    let rid: Option<String> = conn
        .query_row(
            "SELECT recording_id FROM steps WHERE id = ?1",
            params![step_id],
            |r| r.get(0),
        )
        .ok();
    if let Some(ref rid) = rid {
        let _ = push_edit_history(conn, rid);
    }
}

/// 每栈每录制只保留最近 EDIT_HISTORY_LIMIT 条。
fn prune_edit_history(
    conn: &rusqlite::Connection,
    recording_id: &str,
    stack: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM edit_history WHERE recording_id = ?1 AND stack = ?2 AND id NOT IN \
         (SELECT id FROM edit_history WHERE recording_id = ?1 AND stack = ?2 \
          ORDER BY rowid DESC LIMIT ?3)",
        params![recording_id, stack, EDIT_HISTORY_LIMIT],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 用快照还原（步骤全删全插 + 重排 + 恢复录制标题/total_steps）。
/// 文件已不在磁盘上的截图路径会被置空，避免界面挂着悬空引用。
fn restore_editor_snapshot(
    conn: &rusqlite::Connection,
    recording_id: &str,
    snap: &EditorSnapshot,
) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM steps WHERE recording_id = ?1",
        params![recording_id],
    )
    .map_err(|e| e.to_string())?;
    for (i, s) in snap.steps.iter().enumerate() {
        // 快照之后图片文件可能已被物理删除（如删除截图操作），置空而不是留悬空路径
        let exists = |p: &str| p.is_empty() || std::path::Path::new(p).exists();
        let shot = if exists(&s.screenshot_path) { &s.screenshot_path } else { "" };
        let ann = if exists(&s.annotated_path) { &s.annotated_path } else { "" };
        let before = if exists(&s.before_screenshot_path) { &s.before_screenshot_path } else { "" };
        let after = s.after_screenshot_path.as_deref().filter(|p| exists(p));
        tx.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, title, \
             description, tip, screenshot_path, annotated_path, before_screenshot_path, \
             after_screenshot_path, x, y, crop_w, crop_h, timestamp, window_title, \
             is_variable, param_name, mouse_button) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
            params![
                s.id, recording_id, i as i32, i as i32 + 1, s.action_type, s.title,
                s.description, s.tip, shot, ann, before, after,
                s.x, s.y, s.crop_w, s.crop_h, s.timestamp, s.window_title,
                s.is_variable, s.param_name, s.mouse_button,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE recordings SET title = ?1, total_steps = ?2, updated_at = datetime('now') \
         WHERE id = ?3",
        params![snap.title, snap.steps.len() as i32, recording_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(snap.steps.len())
}

/// 弹出指定栈的栈顶并还原（undo/redo 共用）。
fn pop_and_restore(
    conn: &rusqlite::Connection,
    recording_id: &str,
    pop_stack: &str,
    push_stack: &str,
) -> Result<usize, String> {
    let (snap_id, payload): (String, String) = conn
        .query_row(
            "SELECT id, payload FROM edit_history \
             WHERE recording_id = ?1 AND stack = ?2 ORDER BY rowid DESC LIMIT 1",
            params![recording_id, pop_stack],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| if pop_stack == "undo" {
            "没有可撤销的操作".to_string()
        } else {
            "没有可恢复的操作".to_string()
        })?;
    let snap: EditorSnapshot =
        serde_json::from_str(&payload).map_err(|e| format!("快照已损坏: {}", e))?;
    // 当前状态转入对侧栈（这是「重做/再撤销」能回来的原因）
    let cur = capture_editor_snapshot(conn, recording_id)?;
    let cur_payload = serde_json::to_string(&cur).map_err(|e| e.to_string())?;
    let n = cur.steps.len() as i64;
    conn.execute(
        "INSERT INTO edit_history (id, recording_id, stack, payload, step_count) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![uuid::Uuid::new_v4().to_string(), recording_id, push_stack, cur_payload, n],
    )
    .map_err(|e| e.to_string())?;
    prune_edit_history(conn, recording_id, push_stack)?;
    restore_editor_snapshot(conn, recording_id, &snap)?;
    conn.execute("DELETE FROM edit_history WHERE id = ?1", params![snap_id])
        .map_err(|e| e.to_string())?;
    Ok(snap.steps.len())
}

#[tauri::command]
pub fn push_edit_snapshot(db: State<DbState>, recording_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    push_edit_history(&conn, &recording_id)
}

#[tauri::command]
pub fn edit_undo(db: State<DbState>, recording_id: String) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    pop_and_restore(&conn, &recording_id, "undo", "redo")
}

#[tauri::command]
pub fn edit_redo(db: State<DbState>, recording_id: String) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    pop_and_restore(&conn, &recording_id, "redo", "undo")
}

#[tauri::command]
pub fn edit_history_sizes(db: State<DbState>, recording_id: String) -> Result<HistSizes, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let count = |stack: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM edit_history WHERE recording_id = ?1 AND stack = ?2",
            params![recording_id, stack],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    Ok(HistSizes { undo: count("undo"), redo: count("redo") })
}

// ── 编辑器视图偏好持久化（全图/裁图）──────────────────────────
// 历史方案只写 localStorage（WebView2），用户实测「退出重进丢裁图视图」，
// WebView 存储不可靠 → 升级为 SQLite settings 表权威存储，localStorage
// 仅作首屏同步缓存。读写都落 [VIEW] 日志，再丢视图有据可查。

fn view_dbg_log(msg: &str) {
    crate::ai::commands::ai_dbg_log(&format!("[VIEW] {}", msg));
}

/// settings 表 key：`flowio_view::<recording_id>`
fn view_setting_key(recording_id: &str) -> String {
    format!("flowio_view::{}", recording_id)
}

/// 读取某教程的视图偏好 JSON（全图/裁图），无记录返回 null。
///
/// # Errors
/// 数据库锁/查询失败时返回错误字符串。
#[tauri::command]
pub fn editor_view_get(db: State<'_, DbState>, recording_id: String) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![view_setting_key(&recording_id)],
            |r| r.get(0),
        )
        .ok();
    view_dbg_log(&format!(
        "get rec={} found={} len={}",
        recording_id,
        v.is_some(),
        v.as_deref().map(str::len).unwrap_or(0)
    ));
    Ok(v)
}

/// 保存某教程的视图偏好 JSON（编辑器每次切换全图/裁图都会调用）。
///
/// # Errors
/// 数据库写入失败时返回错误字符串。
#[tauri::command]
pub fn editor_view_set(db: State<'_, DbState>, recording_id: String, value: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![view_setting_key(&recording_id), value],
    )
    .map_err(|e| e.to_string())?;
    view_dbg_log(&format!("set rec={} len={}", recording_id, value.len()));
    Ok(())
}

// ── 撤销 ──────────────────────────────────────────────────────

/// 用最近一次快照还原步骤（并消费掉该快照）。
pub(crate) fn undo_last_merge_conn(
    conn: &rusqlite::Connection,
    recording_id: &str,
) -> Result<usize, String> {
    let (snap_id, payload): (String, String) = conn
        .query_row(
            "SELECT id, payload FROM step_snapshots WHERE recording_id = ?1 \
             ORDER BY created_at DESC LIMIT 1",
            params![recording_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "没有可撤销的精简记录".to_string())?;

    let rows: Vec<MergeStepRow> =
        serde_json::from_str(&payload).map_err(|e| format!("快照已损坏，无法撤销: {}", e))?;

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM steps WHERE recording_id = ?1", params![recording_id])
        .map_err(|e| e.to_string())?;
    for r in &rows {
        tx.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, title, \
             description, tip, screenshot_path, annotated_path, before_screenshot_path, \
             after_screenshot_path, x, y, crop_w, crop_h, timestamp, window_title) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![
                r.0, recording_id, 0i32, 0i32, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8,
                r.9, r.10, r.11, r.12, r.13, r.14,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    // 用快照里的原始顺序重排（快照按 order_index 升序存，行号即 order）
    for (i, r) in rows.iter().enumerate() {
        tx.execute(
            "UPDATE steps SET order_index = ?1, step_number = ?2 WHERE id = ?3",
            params![i as i32, i as i32 + 1, r.0],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE recordings SET total_steps = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![rows.len() as i32, recording_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM step_snapshots WHERE id = ?1", params![snap_id])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(rows.len())
}

#[tauri::command]
pub fn undo_last_merge(db: State<DbState>, recording_id: String) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    undo_last_merge_conn(&conn, &recording_id)
}

/// 该录制是否有可撤销的精简记录（前端据此显示/隐藏「撤销」按钮）。
#[tauri::command]
pub fn can_undo_merge(db: State<DbState>, recording_id: String) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM step_snapshots WHERE recording_id = ?1",
            params![recording_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(n > 0)
}

// ── AI 层 ─────────────────────────────────────────────────────

/// 构造「步骤精简」提示词：让模型输出**相邻步骤的分区方案**，而不是自由改写。
///
/// 这样程序能严格校验（分区必须连续、覆盖、不重叠），模型幻觉最多导致
/// 合并得不对，不会凭空造出步骤或改坏截图/坐标。
pub fn build_simplify_prompt(
    steps: &[MergeStepLite],
    titles: &[String],
    descs: &[String],
) -> String {
    build_simplify_prompt_ext(steps, titles, descs, "")
}

/// [`build_simplify_prompt`] 的可扩展版：`extra_rules` 追加在基础规则之后
/// （带截图调用时传入视觉相关规则，纯文本调用传空）。
pub fn build_simplify_prompt_ext(
    steps: &[MergeStepLite],
    titles: &[String],
    descs: &[String],
    extra_rules: &str,
) -> String {
    let mut lines = String::new();
    for (i, s) in steps.iter().enumerate() {
        let title = titles.get(i).cloned().unwrap_or_default();
        let desc = descs.get(i).cloned().unwrap_or_default();
        let desc = if desc.chars().count() > 60 {
            format!("{}…", desc.chars().take(60).collect::<String>())
        } else {
            desc
        };
        lines.push_str(&format!(
            "{}\t{}\t({},{})\t[{}]\t{}\t{}\n",
            i + 1,
            s.action_type,
            s.x.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
            s.y.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
            short_window(&s.window_title),
            if title.is_empty() { "-" } else { &title },
            if desc.is_empty() { "-" } else { &desc },
        ));
    }

    // 2026-09-05 规则收紧（用户实测 WPS XLOOKUP 教程暴露的四类问题）：
    //   · 规则 5：一组只覆盖一个输入框 → 保住「填完后的界面」截图（配合落库侧
    //     last_input_step_index 承载逻辑，双保险）；
    //   · 规则 8：禁合并历史注解（「（先误输后修正）」读者看不懂）；
    //   · 规则 9：禁编造位置词（「另一个单元格」是模型瞎猜的）。
    format!(
        "你是屏幕录制教程的「步骤精简」助手。下面是按时间顺序自动捕获的原始操作步骤，\
         里面混着大量机械性冗余：双击单元格被记成两次点击、点输入框和紧接着的输入被拆成两步、\
         同一个输入框里的多次输入被拆成好几步、以及最后点「停止录制」的那一下。\n\n\
         请只把**机械性冗余**合并掉（重复点击、点击+输入被拆成两步、同一输入框多次输入、\
         误操作后立刻重做），其余操作一律保持独立——目标是读者能看懂每一步在干嘛，\
         而不是步数少。\n\n\
         原始步骤（序号\\t类型\\t坐标\\t窗口\\t标题\\t描述）：\n{n}\
         \n合并规则：\n\
         1. 只能把**相邻的若干步**合成一步。禁止跨步重组、禁止改变先后顺序。\n\
         2. 每个序号必须且只能出现一次：要么在某个 group 里，要么在 removed 里。\n\
         3. 可以合并的典型情况：同一位置的连续点击；点击某输入框后立即在同一位置输入；\
         同一输入框内的多次连续输入；误操作后立刻重做（保留最终有效的那次）。\
         每一组必须满足：组内所有操作针对**同一个控件/位置**且服务**同一个目的**。\n\
         4. 不要合并语义独立的操作：点了不同的按钮/单元格、切换窗口、复制/粘贴/快捷键、\
         打开或关闭对话框。特别注意：复制(Ctrl+C)/剪切与粘贴是两个独立步骤，\
         永远不并入其他组；选择/拖拽文本、点击链接、点击按钮各自独立成步。\n\
         5. 一组最多覆盖**一个输入框/控件**的「点击+输入」过程；输入完成后去点击其他\
         输入框或控件，必须另起一组——保证每一次填完的界面状态都有对应截图。\n\
         6. removed 只放完全没有意义的收尾动作（例如录到最后点了一下录制工具自己的窗口）。\n\
         7. 不确定能否合并时，一律不合并。合并后如果读者会看不懂「这一步在干嘛」，\
         说明合过头了，把该组拆回去。步数多不是问题，看不懂才是。\n\
         8. descriptions 只写**读者要做的最终操作**：禁止解释合并历史（如「先误输后修正」\
         「之前输错了」），禁止任何括号补充说明；误输入与修正合并后，只写最终正确的输入。\n\
         9. 描述里的位置信息只能来自原始标题/描述或坐标：坐标相同或非常接近 = 同一个位置；\
         严禁凭空使用「另一个」「相邻」「某个」「相关位置」等无法佐证的定位词。{extra}\
         \n\n输出严格 JSON，不要 markdown 代码块、不要任何解释：\n\
         {{\"groups\": [[1,2,3],[4],[5,6]], \"removed\": [], \
         \"descriptions\": {{\"1\": \"合并后这一步的一句话说明\", \"4\": \"…\"}}}}\n\
         - groups 里每个数组的元素必须是**连续递增**的序号；\n\
         - descriptions 的键是所在组的**起始序号**，值用中文一句话说明这一步做什么，\
         30~60 字，写给要照着操作的读者看（合并了哪些动作要交代清楚）；\n\
         - **只给合并了多步的组写描述**，单步组不要出现在 descriptions 里\
         （这些步骤的描述保持原样，不要动）。",
        n = lines,
        extra = extra_rules,
    )
}

/// 带截图调用时追加的视觉规则（2026-09-05：AI 精简接入截图）。
///
/// 没有图模型看不到单元格和菜单，只能写出「另一个单元格」「相关位置」这类模糊话，
/// 更不可能写出「工具栏-公式-函数-XLOOKUP」这种具体路径。
pub(crate) const SIMPLIFY_VISION_RULES: &str = "\n\
         10. 每一步都附有截图（按步骤顺序，含点击位置标注）：请结合截图、坐标和原始文字\
         综合判断操作对象（单元格、按钮、菜单项）；截图与坐标冲突时以截图为准。\n\
         11. 打开菜单/功能区/入口的步骤，若截图中能看到菜单和按钮文字，必须写出**完整路径**，\
         格式如：工具栏-公式-函数-XLOOKUP；截图上认不出来就沿用原始描述，\
         禁止写成「相关位置」「相关设置」这类模糊话。";

/// 窗口标题缩短（去掉文件名后缀等噪声，只给模型看关键信息）。
fn short_window(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return "-".to_string();
    }
    let t = t.split(" - ").next().unwrap_or(t);
    if t.chars().count() > 24 {
        format!("{}…", t.chars().take(24).collect::<String>())
    } else {
        t.to_string()
    }
}

/// 解析模型返回的分区方案。
///
/// 容错：允许 ```json 代码块包裹、允许 groups 乱序（会按组内最小值重排）。
/// 严格：校验序号范围、连续性、无重复、无遗漏（遗漏的序号自动补成单步组，
/// 宁可少合并不丢步骤）。
pub fn parse_merge_plan(raw: &str, total: usize) -> Result<MergePlan, String> {
    let trimmed = raw.trim();
    let json_str = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(s), Some(e)) if e >= s => &trimmed[s..=e],
        _ => return Err("AI 没有返回可解析的 JSON".to_string()),
    };
    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("AI 返回的 JSON 无法解析: {}", e))?;

    let groups_val = v
        .get("groups")
        .and_then(|g| g.as_array())
        .ok_or_else(|| "AI 返回缺少 groups 数组".to_string())?;

    #[derive(Debug)]
    struct G {
        idx: Vec<usize>, // 0-based
        start_no: usize, // 1-based 起始序号，用于取描述
    }
    let mut gs: Vec<G> = Vec::new();
    for g in groups_val {
        let arr = g
            .as_array()
            .ok_or_else(|| "groups 里有元素不是数组".to_string())?;
        let mut nos: Vec<usize> = Vec::new();
        for item in arr {
            let n = item
                .as_u64()
                .ok_or_else(|| "groups 里有非数字的序号".to_string())?
                as usize;
            if n < 1 || n > total {
                return Err(format!("序号 {} 超出范围 1~{}", n, total));
            }
            nos.push(n);
        }
        if nos.is_empty() {
            continue;
        }
        nos.sort_unstable();
        nos.dedup();
        // 连续性：组内必须是连续区间（模型偶尔会给出跳跃的序号）
        let first = nos[0];
        let last = *nos.last().unwrap();
        if last - first + 1 != nos.len() {
            return Err(format!(
                "分组 [{:?}] 不是连续区间（不允许跨步合并）",
                nos
            ));
        }
        gs.push(G {
            idx: nos.iter().map(|n| n - 1).collect(),
            start_no: first,
        });
    }

    gs.sort_by_key(|g| g.start_no);

    // 去重校验 + 覆盖检查
    let mut seen = vec![false; total];
    for g in &gs {
        for &i in &g.idx {
            if seen[i] {
                return Err(format!("序号 {} 出现在多个分组里", i + 1));
            }
            seen[i] = true;
        }
    }
    let removed_val: Vec<usize> = v
        .get("removed")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64())
                .map(|n| n as usize)
                .filter(|&n| n >= 1 && n <= total)
                .collect()
        })
        .unwrap_or_default();
    let mut removed: Vec<usize> = Vec::new();
    for n in removed_val {
        let i = n - 1;
        if seen[i] {
            return Err(format!("序号 {} 同时出现在 groups 和 removed 里", n));
        }
        seen[i] = true;
        removed.push(i);
    }
    removed.sort_unstable();

    // 遗漏的序号补成单步组（宁可少合并，绝不丢步骤）
    let mut groups: Vec<Vec<usize>> = gs.iter().map(|g| g.idx.clone()).collect();
    for i in 0..total {
        if !seen[i] {
            groups.push(vec![i]);
        }
    }
    groups.sort_by_key(|g| g[0]);

    let descs_map = v.get("descriptions").and_then(|d| d.as_object());
    let descs: Vec<Option<String>> = groups
        .iter()
        .map(|g| {
            let start_no = g[0] + 1;
            descs_map
                .and_then(|m| m.get(&start_no.to_string()))
                .and_then(|s| s.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .collect();

    Ok(MergePlan {
        groups,
        removed,
        descs,
        titles: Vec::new(),
    })
}

/// 单次精简请求最多附带的截图数（防超大录制把请求体撑爆；超出部分模型按纯文本处理）。
const SIMPLIFY_MAX_IMAGES: usize = 40;

/// 读出一步的截图（优先带点击位置标注的 annotated 图）并按 action_type 裁剪，
/// 返回 JPEG base64；无图/读取失败返回空串。
fn simplify_step_image(r: &MergeStepRow) -> String {
    let path = if r.6.is_empty() { r.5.as_str() } else { r.6.as_str() };
    let raw = crate::ai::commands::read_file_base64(path);
    if raw.is_empty() {
        return String::new();
    }
    crate::ai::pipeline::crop_and_encode_typed(&raw, r.9, r.10, r.11, r.12, &r.1)
}

/// 组装精简请求的 user 消息：文本提示词 + 按步骤顺序穿插的截图。
/// 纯函数：`step_images` 为空时退化为纯文本（旧行为）。
fn build_simplify_user_content(prompt: &str, step_images: &[String]) -> serde_json::Value {
    if step_images.iter().all(|s| s.is_empty()) {
        return serde_json::Value::String(prompt.to_string());
    }
    let mut parts = vec![serde_json::json!({ "type": "text", "text": prompt })];
    for (i, img) in step_images.iter().enumerate() {
        if img.is_empty() {
            continue;
        }
        parts.push(serde_json::json!({
            "type": "text",
            "text": format!("上面步骤表中序号 {} 的截图：", i + 1),
        }));
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:image/jpeg;base64,{}", img) },
        }));
    }
    serde_json::Value::Array(parts)
}

/// AI 精简：把步骤序列交给模型出分区方案，校验通过后落库。
///
/// 外壳只负责「取标题 + 派发完成通知」；实际逻辑在 `simplify_steps_ai_inner`。
/// 这样无论成功还是失败（`?` 提前返回），用户都能在**任意页面**收到带教程名的
/// 完成/失败提醒（退出教程页也不丢失）。
#[tauri::command]
pub async fn simplify_steps_ai(
    db: State<'_, DbState>,
    app: AppHandle,
    recording_id: String,
) -> Result<SimplifyResult, String> {
    let title = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT title FROM recordings WHERE id = ?1",
            rusqlite::params![recording_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
    };
    let result = simplify_steps_ai_inner(&db, &recording_id).await;
    match &result {
        Ok(r) => {
            let summary = if r.kind == "ai" {
                format!(
                    "AI 精简完成：从 {} 步减到 {} 步（减少 {} 步）",
                    r.before, r.after, r.reduced
                )
            } else {
                format!("已自动合并重复步骤，减少 {} 步", r.reduced)
            };
            crate::job_notify::emit_job_done(
                &app,
                &recording_id,
                &title,
                crate::job_notify::JobOp::AiSimplify,
                true,
                &summary,
            );
        }
        Err(e) => {
            crate::job_notify::emit_job_done(
                &app,
                &recording_id,
                &title,
                crate::job_notify::JobOp::AiSimplify,
                false,
                e,
            );
        }
    }
    result
}

/// `simplify_steps_ai` 的实际实现（见上方外壳的注释）。
async fn simplify_steps_ai_inner(
    db: &State<'_, DbState>,
    recording_id: &str,
) -> Result<SimplifyResult, String> {
    // ── 前置：先跑规则层 ──────────────────────────────────────
    // 两个好处：① 交给 AI 的序列更短（省 token、判断更容易）；
    // ② 万一 AI 不可用（没配 key / 限流 / 返回垃圾），用户也已经拿到规则层的收益，
    //    不会点完按钮什么都没发生。
    let rules = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        push_edit_history(&conn, recording_id)?;
        auto_merge_steps_conn(&conn, recording_id)?
    };

    let (config, rows) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            crate::ai::commands::resolve_ai_config(&conn),
            load_merge_steps(&conn, recording_id)?,
        )
    };
    let before = rules.before;
    if rows.len() < 3 {
        // 规则层已经处理过，剩下的步骤太少，没必要再花一次 AI 调用
        return Ok(SimplifyResult {
            before,
            after: rows.len(),
            reduced: rules.reduced,
            kind: "rules".to_string(),
        });
    }

    let lite: Vec<MergeStepLite> = rows
        .iter()
        .map(|r| MergeStepLite {
            action_type: r.1.clone(),
            title: r.2.clone(),
            window_title: r.14.clone(),
            x: r.9,
            y: r.10,
            timestamp: r.13,
        })
        .collect();
    let titles: Vec<String> = rows.iter().map(|r| r.2.clone()).collect();
    let descs: Vec<String> = rows.iter().map(|r| r.3.clone()).collect();

    // ── 视觉增强（2026-09-05）：附上每步截图，模型才能写出具体位置 ──
    // 没有图它看不到单元格/菜单，只会写「另一个单元格」「相关位置」这类模糊话。
    // 模型不支持视觉时退化为纯文本（旧行为，规则层收益不受影响）。
    let use_vision = crate::ai::pipeline::model_supports_vision(&config.provider.model);
    let (prompt, step_images) = if use_vision {
        (
            build_simplify_prompt_ext(&lite, &titles, &descs, SIMPLIFY_VISION_RULES),
            rows.iter()
                .take(SIMPLIFY_MAX_IMAGES)
                .map(simplify_step_image)
                .collect::<Vec<String>>(),
        )
    } else {
        (build_simplify_prompt(&lite, &titles, &descs), Vec::new())
    };
    let content = build_simplify_user_content(&prompt, &step_images);

    // 输出是「40+ 个分组 + 每组一句话描述」的完整 JSON，4096 会被截断
    // （截断 → 解析必然失败），留足余量到 8192。
    let raw = tauri::async_runtime::spawn_blocking(move || {
        crate::ai::http::call_llm_with_tokens(
            &config,
            vec![crate::ai::http::Message {
                role: "user".to_string(),
                content,
            }],
            8192,
        )
    })
    .await
    .map_err(|e| format!("AI 精简任务异常: {}", e))??;

    let plan = parse_merge_plan(&raw, rows.len()).map_err(|e| {
        format!(
            "{}。规则层已自动合并 {} 步，AI 部分未生效（原始返回前 120 字：{}）",
            e,
            rules.reduced,
            raw.chars().take(120).collect::<String>()
        )
    })?;
    let groups = plan_to_groups(&plan, &rows);
    let removed: Vec<String> = plan
        .removed
        .iter()
        .filter_map(|&i| rows.get(i).map(|r| r.0.clone()))
        .collect();

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let applied = apply_merge_plan(&conn, recording_id, &groups, &removed, true)?;
    Ok(SimplifyResult {
        before,
        after: before - rules.reduced - applied,
        reduced: rules.reduced + applied,
        kind: "ai".to_string(),
    })
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
    let _ = push_edit_history(&conn, &recording_id);

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
/// Returns error if another recording (excluding self) already has this title.
#[tauri::command]
pub fn update_recording_title(
    db: State<DbState>,
    id: String,
    title: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 标题长度封顶：超长标题会把编辑器顶栏挤变形（与另存为 find_unique_title 同一上限）
    let title = crate::db::commands::cap_title_chars(title.trim());
    // Check for duplicate title (exclude current recording)
    let dup_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recordings WHERE title = ?1 AND id != ?2",
            params![title, id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if dup_count > 0 {
        return Err(format!("标题已存在：{}", title));
    }
    let _ = push_edit_history(&conn, &id);
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
    push_edit_history_for_step(&conn, &step_id);

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

/// Save the fully-annotated (non-cropped) screenshot as a step's main image.
///
/// 与 `crop_step_screenshot` 的区别（关键，对应「全图裁图互不影响」原则）：
/// - `crop_step_screenshot` 把「裁切后的图」写进 screenshot_path → 全图被裁切图覆盖，
///   而裁图视图又从全图二次派生，结果全图和裁图都变成裁切图（bug 来源）；
/// - 本命令把「**完整图 + 注释**」原地覆盖回 screenshot_path（同一个文件，不另存
///   `_cropped.jpg`），只动全图；裁图则交给 `update_step_crop` 更新锚点独立控制。
///
/// 这样：全图 = 完整标注图；裁图 = 完整标注图按锚点派生。两者互不覆盖，
/// 之后还能随时改裁图框而绝不丢全图。
#[tauri::command]
pub fn save_step_screenshot_full(
    db: State<DbState>,
    step_id: String,
    base64_data: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    push_edit_history_for_step(&conn, &step_id);

    // 解码 base64 → 图像字节（沿用 crop_step_screenshot 的 decoder，PNG/JPEG 字节都接受）
    let bytes = decode_jpeg_base64(&base64_data)?;

    let current_path: String = conn
        .query_row(
            "SELECT screenshot_path FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Step not found: {}", e))?;

    if current_path.is_empty() {
        return Err("该步骤没有主截图路径，无法写入标注".into());
    }

    // 原地覆盖同一文件：路径不变、不残留 _cropped 副本，裁图缓存随锚点派生保持一致
    std::fs::write(&current_path, &bytes)
        .map_err(|e| format!("Failed to write screenshot: {}", e))?;

    Ok(())
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
    push_edit_history_for_step(&conn, &step_id);

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
    push_edit_history_for_step(&conn, &step_id);

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
        is_variable: false,
        param_name: String::new(),
        mouse_button: String::new(),
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
    fn replace_ci_basic_mixed_case() {
        // 大小写不敏感：Search 能命中 sEARCH 等混写
        let (out, n) = replace_ci("Click the Search button, then search again", "search", "查找");
        assert_eq!(n, 2);
        assert_eq!(out, "Click the 查找 button, then 查找 again");
    }

    #[test]
    fn replace_ci_chinese_exact() {
        let (out, n) = replace_ci("在工具栏点击保存，保存后关闭", "保存", "导出");
        assert_eq!(n, 2);
        assert_eq!(out, "在工具栏点击导出，导出后关闭");
    }

    #[test]
    fn replace_ci_no_match_returns_original() {
        let (out, n) = replace_ci("纯中文步骤", "Excel", "表格");
        assert_eq!(n, 0);
        assert_eq!(out, "纯中文步骤");
    }

    #[test]
    fn replace_ci_empty_search_noop() {
        let (out, n) = replace_ci("abc", "", "x");
        assert_eq!(n, 0);
        assert_eq!(out, "abc");
    }

    #[test]
    fn replace_ci_replace_with_empty_deletes() {
        let (out, n) = replace_ci("步骤A，步骤B", "，", "");
        assert_eq!(n, 1);
        assert_eq!(out, "步骤A步骤B");
    }

    #[test]
    fn replace_ci_no_double_match_on_overlap() {
        // 命中后跳过整段，不做重叠匹配：aaa 中搜 aa 只替换 1 处
        let (out, n) = replace_ci("aaa", "aa", "b");
        assert_eq!(n, 1);
        assert_eq!(out, "ba");
    }

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
            is_variable: false,
            param_name: String::new(),
            mouse_button: String::new(),
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
            is_variable: false,
            param_name: String::new(),
            mouse_button: String::new(),
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
            is_variable: false,
            param_name: String::new(),
            mouse_button: String::new(),
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

    // ── P2 手动合并 merge_steps_conn ──

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn);
        conn.execute(
            "INSERT INTO recordings (id, title) VALUES ('r1', '测试录制')",
            [],
        )
        .unwrap();
        conn
    }

    /// 插入一个步骤，返回 id。
    fn insert_step(
        conn: &rusqlite::Connection,
        id: &str,
        order: i32,
        action: &str,
        title: &str,
        desc: &str,
    ) {
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, \
             title, description, tip, screenshot_path, annotated_path, before_screenshot_path, \
             x, y, crop_w, crop_h, after_screenshot_path, timestamp, window_title) \
             VALUES (?1, 'r1', ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'w')",
            rusqlite::params![
                id, order, order + 1, action, title, desc,
                format!("{}.jpg", id), format!("{}.ann.jpg", id), format!("{}.before.jpg", id),
                Some(order * 10), Some(order * 10), Some(100), Some(80),
                format!("{}.after.jpg", id), 1000 + order as i64,
            ],
        )
        .unwrap();
    }

    fn step_row(
        conn: &rusqlite::Connection,
        id: &str,
    ) -> (String, String, String, String, Option<i32>, i32) {
        conn.query_row(
            "SELECT action_type, title, description, screenshot_path, x, order_index \
             FROM steps WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn merge_steps_merges_fields_and_reindexes() {
        let conn = mem_db();
        insert_step(&conn, "s0", 0, "click", "第一步", "点开对话框");
        insert_step(&conn, "s1", 1, "click", "第二步", "点查找值框");
        insert_step(&conn, "s2", 2, "type", "第三步", "输入查找值");
        insert_step(&conn, "s3", 3, "click", "第四步", "点确定");

        merge_steps_conn(&conn, "r1", &["s1".into(), "s2".into(), "s3".into()]).unwrap();

        // s1 承载合并结果：主图/坐标取组内最后一个输入步 s2（type，填完后的状态，
        // 2026-09-05 起不再取末步——末步 s3 已点去「确定」，填完状态会被跳掉）、
        // 描述拼接、标题取首步 s1
        let (action, title, desc, ss, x, order) = step_row(&conn, "s1");
        assert_eq!(action, "click");
        assert_eq!(title, "第二步");
        assert_eq!(desc, "点查找值框\n输入查找值\n点确定");
        assert_eq!(ss, "s2.jpg"); // 组内最后一个输入步的主图
        assert_eq!(x, Some(20)); // 输入步的裁剪锚点
        assert_eq!(order, 1);

        // s2/s3 已删除，剩 2 步且 order_index 重排
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM steps WHERE recording_id='r1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        let orders: Vec<i32> = conn
            .prepare("SELECT order_index FROM steps WHERE recording_id='r1' ORDER BY order_index")
            .unwrap()
            .query_map([], |r| r.get::<_, i32>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(orders, vec![0, 1]);
    }

    #[test]
    fn merge_steps_rejects_single_or_missing_ids() {
        let conn = mem_db();
        insert_step(&conn, "s0", 0, "click", "a", "d");
        insert_step(&conn, "s1", 1, "click", "b", "d");

        // 只选 1 个 → 报错
        assert!(merge_steps_conn(&conn, "r1", &["s0".into()]).is_err());
        // 含不存在的 id → 报错且不落库
        assert!(merge_steps_conn(&conn, "r1", &["s0".into(), "ghost".into()]).is_err());
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM steps WHERE recording_id='r1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "失败的合并不应改动任何步骤");
    }

    // ══════════════════════════════════════════════════════════
    // 自动精简（规则层 + AI 层 + 撤销）
    // 用真实 WPS 录制里出现的模式构造用例（2026-09-03 实测回放）
    // ══════════════════════════════════════════════════════════

    fn lite(action: &str, title: &str, win: &str, x: i32, y: i32, ts: i64) -> MergeStepLite {
        MergeStepLite {
            action_type: action.to_string(),
            title: title.to_string(),
            window_title: win.to_string(),
            x: Some(x),
            y: Some(y),
            timestamp: ts,
        }
    }

    /// 插入一个可指定窗口/坐标/时间的步骤（规则测试需要精确控制这些）。
    fn insert_full(
        conn: &rusqlite::Connection,
        id: &str,
        order: i32,
        action: &str,
        title: &str,
        desc: &str,
        win: &str,
        x: i32,
        y: i32,
        ts: i64,
    ) {
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, \
             title, description, tip, screenshot_path, annotated_path, before_screenshot_path, \
             x, y, crop_w, crop_h, after_screenshot_path, timestamp, window_title) \
             VALUES (?1, 'r1', ?2, ?3, ?4, ?5, ?6, '', ?7, '', '', ?8, ?9, 100, 80, '', ?10, ?11)",
            rusqlite::params![
                id, order, order + 1, action, title, desc,
                format!("{}.jpg", id), x, y, ts, win,
            ],
        )
        .unwrap();
    }

    #[test]
    fn plan_auto_merge_collapses_double_and_triple_click() {
        // 真实数据：同一单元格被点了三次（+0.2s / +0.5s），应并成一步
        let steps = vec![
            lite("click", "在「WPS」中点击", "WPS", 203, 298, 1000),
            lite("click", "在「WPS」中点击", "WPS", 203, 298, 1200),
            lite("click", "在「WPS」中点击", "WPS", 203, 298, 1700),
            lite("click", "在「WPS」中点击", "WPS", 500, 400, 9000), // 远处另一次点击
        ];
        let plan = plan_auto_merge(&steps, &AutoMergeRules::default());
        assert_eq!(plan.groups, vec![vec![0, 1, 2], vec![3]]);
        assert!(plan.removed.is_empty());
        // 标题改成「双击」，且只改一次（三连击不会变成「双击双击」）
        assert_eq!(
            plan.titles[0].as_deref(),
            Some("在「WPS」中双击"),
            "三连击标题应只替换一次：{:?}",
            plan.titles
        );
        assert_eq!(plan.titles[1], None);
        assert_eq!(plan.reduced(4), 2);
    }

    #[test]
    fn plan_auto_merge_merges_continuous_typing_same_box() {
        // 真实数据：同一输入框连续输入被拆成 3 步（+4.4s / +1.8s）
        let steps = vec![
            lite("click", "在「函数参数」中点击", "函数参数", 874, 515, 1000),
            lite("type", "在「函数参数」中输入", "函数参数", 874, 515, 3000),
            lite("type", "在「函数参数」中输入", "函数参数", 874, 515, 7400),
            lite("type", "在「函数参数」中输入", "函数参数", 874, 515, 9200),
        ];
        let plan = plan_auto_merge(&steps, &AutoMergeRules::default());
        // R3 把 click+type 并成一步，随后 R2 把剩余两次输入也吃进来
        assert_eq!(plan.groups, vec![vec![0, 1, 2, 3]], "实际：{:?}", plan.groups);
        assert_eq!(plan.reduced(4), 3);
    }

    #[test]
    fn plan_auto_merge_keeps_independent_operations() {
        let steps = vec![
            lite("click", "点击 A", "WPS", 100, 100, 1000),
            lite("click", "点击 B", "WPS", 900, 700, 1100), // 同窗口但坐标差很远
            lite("copy", "Ctrl+C 复制", "WPS", 900, 700, 3000), // 快捷键不参与合并
            lite("click", "点击 C", "WPS", 900, 700, 4000),
            lite("click", "点击 D", "函数参数", 900, 700, 4200), // 跨窗口不合并
            lite("type", "输入", "函数参数", 300, 300, 5000),    // 坐标差太远不配对
        ];
        let plan = plan_auto_merge(&steps, &AutoMergeRules::default());
        assert_eq!(plan.groups.len(), 6, "这些操作都该保持独立：{:?}", plan.groups);
        assert_eq!(plan.reduced(6), 0);
    }

    #[test]
    fn plan_auto_merge_type_gap_boundary() {
        // 同坐标两次输入：间隔 15s 合并，15.1s 不合并
        let within = vec![
            lite("type", "t", "w", 10, 10, 0),
            lite("type", "t", "w", 10, 10, 15_000),
        ];
        assert_eq!(plan_auto_merge(&within, &AutoMergeRules::default()).groups.len(), 1);
        let beyond = vec![
            lite("type", "t", "w", 10, 10, 0),
            lite("type", "t", "w", 10, 10, 15_001),
        ];
        assert_eq!(plan_auto_merge(&beyond, &AutoMergeRules::default()).groups.len(), 2);
    }

    #[test]
    fn plan_auto_merge_removes_self_window_tail() {
        // 最后一步点的是「录步」自己的窗口（= 停止录制），应删除
        let steps = vec![
            lite("click", "点击单元格", "WPS", 100, 100, 1000),
            lite("click", "在「录步」中点击", "录步", 1400, 1055, 9000),
        ];
        let plan = plan_auto_merge(&steps, &AutoMergeRules::default());
        assert_eq!(plan.removed, vec![1]);
        assert_eq!(plan.groups, vec![vec![0]]);
        assert_eq!(plan.reduced(2), 1);

        // 自家窗口在中间不该被删（只处理收尾那一下）
        let steps2 = vec![
            lite("click", "a", "录步", 100, 100, 1000),
            lite("click", "b", "WPS", 100, 100, 2000),
        ];
        assert!(plan_auto_merge(&steps2, &AutoMergeRules::default()).removed.is_empty());
    }

    #[test]
    fn plan_auto_merge_empty_and_single_are_noop() {
        assert_eq!(plan_auto_merge(&[], &AutoMergeRules::default()).groups.len(), 0);
        let one = vec![lite("click", "a", "w", 1, 1, 0)];
        assert_eq!(plan_auto_merge(&one, &AutoMergeRules::default()).groups, vec![vec![0]]);
    }

    #[test]
    fn plan_auto_merge_missing_coords_never_merges() {
        let mut a = lite("click", "a", "w", 10, 10, 0);
        a.x = None;
        a.y = None;
        let b = lite("click", "b", "w", 10, 10, 100);
        let plan = plan_auto_merge(&[a, b], &AutoMergeRules::default());
        assert_eq!(plan.groups.len(), 2, "缺坐标不应判定为同位置");
    }

    // ── AI 分区解析 ────────────────────────────────────────────

    #[test]
    fn parse_merge_plan_accepts_valid_partition_with_descriptions() {
        let raw = r#"{"groups":[[1,2,3],[4],[5,6]],"removed":[],
                     "descriptions":{"1":"双击 C2 单元格并输入公式","4":"点击确定","5":"填入未找到值"}}"#;
        let plan = parse_merge_plan(raw, 6).unwrap();
        assert_eq!(plan.groups, vec![vec![0, 1, 2], vec![3], vec![4, 5]]);
        assert_eq!(
            plan.descs[0].as_deref(),
            Some("双击 C2 单元格并输入公式")
        );
        assert_eq!(plan.descs[2].as_deref(), Some("填入未找到值"));
        assert!(plan.removed.is_empty());
        assert_eq!(plan.reduced(6), 3);
    }

    #[test]
    fn parse_merge_plan_strips_code_fence_and_reorders_groups() {
        let raw = "```json\n{\"groups\":[[4,5],[1,2]],\"removed\":[3]}\n```";
        let plan = parse_merge_plan(raw, 5).unwrap();
        assert_eq!(plan.groups, vec![vec![0, 1], vec![3, 4]], "应按组内最小值重排");
        assert_eq!(plan.removed, vec![2]);
    }

    #[test]
    fn parse_merge_plan_fills_missing_indices_as_singletons() {
        // AI 漏了 3 号和 6 号：补成单步组，绝不丢步骤
        let raw = r#"{"groups":[[1,2],[4,5]],"removed":[]}"#;
        let plan = parse_merge_plan(raw, 6).unwrap();
        let flat: Vec<usize> = plan.groups.iter().flat_map(|g| g.iter().copied()).collect();
        assert_eq!(flat, vec![0, 1, 2, 3, 4, 5], "所有步骤必须保留且有序");
        assert_eq!(plan.groups.len(), 4);
    }

    #[test]
    fn parse_merge_plan_rejects_non_contiguous_group() {
        let raw = r#"{"groups":[[1,3]],"removed":[2]}"#;
        assert!(parse_merge_plan(raw, 3).is_err(), "跨步重组必须拒绝");
    }

    #[test]
    fn parse_merge_plan_rejects_out_of_range_and_duplicates() {
        assert!(parse_merge_plan(r#"{"groups":[[1,9]]}"#, 5).is_err(), "越界序号");
        assert!(
            parse_merge_plan(r#"{"groups":[[1,2],[2,3]]}"#, 4).is_err(),
            "重复序号"
        );
        assert!(
            parse_merge_plan(r#"{"groups":[[1,2]],"removed":[2]}"#, 2).is_err(),
            "groups 与 removed 冲突"
        );
    }

    #[test]
    fn parse_merge_plan_rejects_garbage() {
        assert!(parse_merge_plan("抱歉，我无法完成", 3).is_err());
        assert!(parse_merge_plan(r#"{"nope":1}"#, 3).is_err());
    }

    // ── 落库 + 撤销（集成）──────────────────────────────────────

    #[test]
    fn auto_merge_conn_reduces_steps_and_undo_restores_them() {
        let conn = mem_db();
        // 双击单元格（3 次点击）+ 一次远处点击 + 点「录步」停止
        insert_full(&conn, "s0", 0, "click", "在「WPS」中点击", "老描述", "WPS", 203, 298, 1000);
        insert_full(&conn, "s1", 1, "click", "在「WPS」中点击", "", "WPS", 203, 298, 1200);
        insert_full(&conn, "s2", 2, "click", "在「WPS」中点击", "", "WPS", 203, 298, 1700);
        insert_full(&conn, "s3", 3, "click", "点别处", "d3", "WPS", 900, 700, 5000);
        insert_full(&conn, "s4", 4, "click", "在「录步」中点击", "", "录步", 1400, 1055, 9000);

        let res = auto_merge_steps_conn(&conn, "r1").unwrap();
        assert_eq!(res.before, 5);
        assert_eq!(res.after, 2, "3 步点击并 1 步 + 删掉停止那一步");
        assert_eq!(res.reduced, 3);
        assert_eq!(res.kind, "rules");

        // 承载行 s0：标题改「双击」、主图取末步 s2、操作前图保持 s0
        let (action, title, desc, ss, x, order) = step_row(&conn, "s0");
        assert_eq!(action, "click");
        assert_eq!(title, "在「WPS」中双击");
        assert_eq!(desc, "老描述", "末步描述为空 → 沿用首步");
        assert_eq!(ss, "s2.jpg", "主图取组内末步");
        assert_eq!(x, Some(203));
        assert_eq!(order, 0);

        // order_index 重排为 0..n-1
        let orders: Vec<i32> = conn
            .prepare("SELECT order_index FROM steps WHERE recording_id='r1' ORDER BY order_index")
            .unwrap()
            .query_map([], |r| r.get::<_, i32>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(orders, vec![0, 1]);
        let total: i32 = conn
            .query_row("SELECT total_steps FROM recordings WHERE id='r1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "录制步骤数要同步");

        // 撤销 → 5 步全部还原（含已删除的 s4）
        let restored = undo_last_merge_conn(&conn, "r1").unwrap();
        assert_eq!(restored, 5);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM steps WHERE recording_id='r1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 5);
        let (_, t0, d0, _, _, o0) = step_row(&conn, "s0");
        assert_eq!(t0, "在「WPS」中点击", "撤销后标题还原");
        assert_eq!(d0, "老描述");
        assert_eq!(o0, 0);
        assert!(step_row(&conn, "s4").1.contains("录步"), "被删的收尾步也要回来");

        // 快照已被消费 → 再撤销应报错
        assert!(undo_last_merge_conn(&conn, "r1").is_err());
    }

    #[test]
    fn auto_merge_conn_no_change_writes_no_snapshot() {
        let conn = mem_db();
        insert_full(&conn, "s0", 0, "click", "a", "", "WPS", 100, 100, 0);
        insert_full(&conn, "s1", 1, "click", "b", "", "WPS", 900, 900, 8000);
        let res = auto_merge_steps_conn(&conn, "r1").unwrap();
        assert_eq!(res.reduced, 0);
        assert_eq!(res.after, 2);
        assert!(undo_last_merge_conn(&conn, "r1").is_err(), "没合并就不该产生快照");
    }

    #[test]
    fn apply_merge_plan_uses_last_step_screenshot_and_keeps_timestamp_of_first() {
        let conn = mem_db();
        // click(ts=1000) → type(ts=4000)：合并后 timestamp 必须是首步的，
        // 否则语音切片会丢掉 1000~4000 这段录音。
        insert_full(&conn, "s0", 0, "click", "点击输入框", "", "W", 100, 100, 1000);
        insert_full(&conn, "s1", 1, "type", "输入", "最终文本", "W", 100, 100, 4000);
        let plan = plan_auto_merge(
            &[
                lite("click", "点击输入框", "W", 100, 100, 1000),
                lite("type", "输入", "W", 100, 100, 4000),
            ],
            &AutoMergeRules::default(),
        );
        let groups = plan_to_groups(&plan, &load_merge_steps(&conn, "r1").unwrap());
        let reduced = apply_merge_plan(&conn, "r1", &groups, &[], true).unwrap();
        assert_eq!(reduced, 1);
        let ts: i64 = conn
            .query_row("SELECT timestamp FROM steps WHERE id='s0'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ts, 1000, "timestamp 取首步（语音切片依赖它）");
        let desc: String = conn
            .query_row("SELECT description FROM steps WHERE id='s0'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(desc, "最终文本", "描述取组内末步");
    }

    #[test]
    fn build_simplify_prompt_contains_all_steps_and_rules() {
        let steps = vec![
            lite("click", "在「函数参数」中点击", "函数参数", 1, 2, 0),
            lite("type", "输入", "函数参数", 1, 2, 100),
        ];
        let p = build_simplify_prompt(
            &steps,
            &["在「函数参数」中点击".to_string(), "输入".to_string()],
            &["".to_string(), "concat(".to_string()],
        );
        assert!(p.contains("1\tclick\t(1,2)"), "步骤行格式要稳定");
        assert!(p.contains("2\ttype"));
        assert!(p.contains("concat("));
        assert!(p.contains("groups"), "必须要求输出 groups");
        assert!(p.contains("连续递增"), "必须声明连续性约束");
        // 2026-09-05 收紧的四条规则
        assert!(p.contains("一个输入框"), "一组只覆盖一个输入框的填写");
        assert!(p.contains("最终操作"), "描述只写读者要做的最终操作");
        assert!(p.contains("禁止任何括号补充说明"), "禁合并历史注解");
        assert!(p.contains("另一个"), "禁编造位置词（纯文本版也要禁）");
        assert!(!p.contains("附有截图"), "纯文本版不能提截图（模型没收到图）");
    }

    #[test]
    fn build_simplify_prompt_ext_appends_vision_rules() {
        let steps = vec![lite("click", "t", "w", 1, 2, 0)];
        let p = build_simplify_prompt_ext(
            &steps,
            &["t".to_string()],
            &["".to_string()],
            SIMPLIFY_VISION_RULES,
        );
        assert!(p.contains("附有截图"), "视觉版要告知模型收到了截图");
        assert!(p.contains("完整路径"), "要求写出菜单完整路径");
        assert!(p.contains("同一个位置"), "坐标相同=同一位置");
        // 基础规则仍在
        assert!(p.contains("连续递增"));
    }

    #[test]
    fn build_simplify_user_content_text_only_when_no_images() {
        let c = build_simplify_user_content("PROMPT", &[]);
        assert!(c.is_string(), "无图时退化为纯字符串");
        let c2 = build_simplify_user_content("PROMPT", &[String::new()]);
        assert!(c2.is_string(), "全空图也退化为纯字符串");
    }

    #[test]
    fn build_simplify_user_content_interleaves_text_and_images() {
        let imgs = vec!["IMG1".to_string(), String::new(), "IMG3".to_string()];
        let c = build_simplify_user_content("PROMPT", &imgs);
        let arr = c.as_array().unwrap();
        // text(prompt) + (text+img)×2（空图跳过）
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "text");
        assert!(arr[1]["text"].as_str().unwrap().contains("1"));
        assert_eq!(arr[2]["image_url"]["url"], "data:image/jpeg;base64,IMG1");
        assert_eq!(arr[4]["image_url"]["url"], "data:image/jpeg;base64,IMG3");
        // 空图步（序号 2）不应出现对应标签
        let labels: Vec<&str> = arr
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect();
        assert!(!labels.iter().any(|t| t.contains("2 的截图")));
    }

    #[test]
    fn last_input_step_index_prefers_last_type_or_paste() {
        let ids = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let actions = ["click", "type", "click", "paste"];
        assert_eq!(last_input_step_index(&ids, &actions), Some(3));
        let actions2 = ["click", "type", "click", "click"];
        assert_eq!(last_input_step_index(&ids, &actions2), Some(1));
        let actions3 = ["click", "click", "hotkey", "click"];
        assert_eq!(last_input_step_index(&ids, &actions3), None);
    }

    #[test]
    fn apply_merge_plan_input_step_carries_screenshot_and_coords() {
        let conn = mem_db();
        // click → type → click 合并成一组：截图/坐标必须取中间的 type 步（填完后的状态），
        // 而不是组末步（又点去了下一格，填完状态被跳掉 → 2026-09-05 用户反馈）。
        insert_full(&conn, "s0", 0, "click", "点输入框", "", "W", 0, 0, 1000);
        insert_full(&conn, "s1", 1, "type", "输入", "填了值", "W", 10, 10, 2000);
        insert_full(&conn, "s2", 2, "click", "点下一格", "", "W", 20, 20, 3000);
        let groups = vec![MergeGroup {
            ids: vec!["s0".into(), "s1".into(), "s2".into()],
            desc: Some("点击输入框并填写".into()),
            title: None,
        }];
        let reduced = apply_merge_plan(&conn, "r1", &groups, &[], true).unwrap();
        assert_eq!(reduced, 2);
        let (ss, x, y): (String, Option<i32>, Option<i32>) = conn
            .query_row(
                "SELECT screenshot_path, x, y FROM steps WHERE id='s0'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(ss, "s1.jpg", "截图取组内最后一个输入步骤");
        assert_eq!(x, Some(10), "坐标随截图取输入步骤（热点=输入框）");
        assert_eq!(y, Some(10));
    }

    #[test]
    fn apply_merge_plan_click_only_group_still_uses_last_step() {
        let conn = mem_db();
        // 纯点击组：维持原行为（末步承载）
        insert_full(&conn, "s0", 0, "click", "a", "", "W", 0, 0, 1000);
        insert_full(&conn, "s1", 1, "click", "b", "", "W", 10, 10, 2000);
        let groups = vec![MergeGroup {
            ids: vec!["s0".into(), "s1".into()],
            desc: None,
            title: None,
        }];
        apply_merge_plan(&conn, "r1", &groups, &[], true).unwrap();
        let ss: String = conn
            .query_row("SELECT screenshot_path FROM steps WHERE id='s0'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ss, "s1.jpg", "无输入步骤的组仍取组末步截图");
    }

    /// 黄金值测试：真实录制 2026-09-03 14:44:01（WPS 表格 XLOOKUP 教程，52 步）。
    ///
    /// 这份数据是用户抱怨「步骤太多」的原始样本，序列从生产库原样导出
    /// （action_type / 窗口序号 / x / y / 相对首步的毫秒偏移）。
    /// 锁住它的精简结果 = 锁住规则层的实际收益；改阈值导致收益下降时这里会红。
    #[test]
    fn plan_auto_merge_golden_real_wps_recording() {
        // 0 = WPS 表格主窗口，1 = 「函数参数」弹框，2 = 录步自己（停止按钮）
        let wins = ["WPS", "FN", "录步"];
        let raw: Vec<(&str, usize, i32, i32, i64)> = vec![
            ("click", 0, 203, 298, 0),
            ("click", 0, 203, 298, 223),
            ("click", 0, 203, 298, 748),
            ("copy", 0, 960, 510, 2838),
            ("click", 0, 498, 297, 3974),
            ("paste", 0, 960, 597, 4699),
            ("hotkey", 0, 960, 597, 10109),
            ("click", 0, 640, 302, 12547),
            ("click", 0, 640, 302, 12770),
            ("type", 0, 651, 297, 15457),
            ("click", 0, 153, 155, 16887),
            ("click", 0, 1113, 870, 18974),
            ("click", 1, 768, 264, 21164),
            ("type", 1, 874, 515, 31031),
            ("type", 1, 874, 515, 35445),
            ("type", 1, 874, 515, 37252),
            ("click", 1, 474, 293, 38852),
            ("type", 1, 874, 515, 45955),
            ("click", 1, 768, 512, 50826),
            ("type", 1, 874, 515, 57956),
            ("type", 1, 874, 515, 66254),
            ("click", 1, 807, 524, 67088),
            ("type", 1, 874, 515, 72552),
            ("click", 1, 1009, 561, 75505),
            ("click", 1, 838, 247, 78451),
            ("click", 1, 1328, 493, 80452),
            ("click", 1, 1014, 607, 82712),
            ("click", 1, 1009, 247, 84753),
            ("click", 1, 1334, 495, 86952),
            ("click", 1, 883, 654, 88759),
            ("type", 1, 874, 650, 95309),
            ("type", 1, 874, 650, 102232),
            ("click", 1, 865, 701, 103487),
            ("click", 0, 1197, 898, 108474),
            ("click", 0, 902, 577, 116777),
            ("click", 0, 1015, 361, 123705),
            ("click", 0, 602, 297, 125946),
            ("click", 0, 602, 297, 126163),
            ("click", 0, 569, 301, 128247),
            ("hotkey", 0, 466, 211, 147526),
            ("type", 0, 755, 297, 151396),
            ("paste", 0, 799, 297, 154090),
            ("click", 0, 659, 298, 158127),
            ("click", 0, 470, 297, 163750),
            ("click", 0, 677, 302, 165088),
            ("click", 0, 862, 292, 167800),
            ("click", 0, 828, 319, 168590),
            ("click", 0, 818, 362, 169482),
            ("click", 0, 658, 302, 170655),
            ("click", 0, 744, 309, 172532),
            ("click", 0, 744, 309, 172694),
            ("click", 2, 1400, 1055, 180041),
        ];
        let steps: Vec<MergeStepLite> = raw
            .iter()
            .map(|(a, w, x, y, ts)| MergeStepLite {
                action_type: a.to_string(),
                title: format!("在「{}」中点击", wins[*w]),
                window_title: wins[*w].to_string(),
                x: Some(*x),
                y: Some(*y),
                timestamp: *ts,
            })
            .collect();

        let plan = plan_auto_merge(&steps, &AutoMergeRules::default());

        // 52 步 → 41 步（省 11 步，21%）：
        //   5 次重复点击合并（含开头那组三连击）、4 次连续输入合并、
        //   1 次「点击后输入」配对、1 次删掉末尾点「录步」停止按钮。
        assert_eq!(plan.groups.len(), 41, "实际保留组数：{}", plan.groups.len());
        assert_eq!(plan.reduced(52), 11);
        assert_eq!(plan.removed, vec![51], "只删末尾那一步停止录制点击");

        // 所有步骤要么进组要么被删，一个都不能丢
        let mut covered: Vec<usize> = plan.groups.iter().flat_map(|g| g.iter().copied()).collect();
        covered.extend(plan.removed.iter().copied());
        covered.sort_unstable();
        assert_eq!(covered, (0..52).collect::<Vec<usize>>(), "步骤覆盖必须完整");

        // 开头三连击 → 一组，且标题改成了「双击」
        assert_eq!(plan.groups[0], vec![0, 1, 2]);
        assert_eq!(plan.titles[0].as_deref(), Some("在「WPS」中双击"));

        // 「双击单元格 → 输入 =」应并成一步（第 8~10 步，0-based 7/8/9）
        assert!(
            plan.groups.iter().any(|g| g == &vec![7, 8, 9]),
            "双击后的输入应并进同一步：{:?}",
            plan.groups
        );
        // 函数参数弹框里同一输入框的连续输入应并成一步（第 14~16 步，0-based 13/14/15）。
        // 注意它前面的「点输入框」(768,264) 与输入坐标 (874,515) 相差 250px，
        // 不满足 24px 配对阈值，故不并入——这正是规则层的边界，交给 AI 层处理。
        assert!(
            plan.groups.iter().any(|g| g == &vec![13, 14, 15]),
            "同一输入框的连续输入应并成一步：{:?}",
            plan.groups
        );
        // 复制/粘贴/快捷键这些语义独立的操作一个都不该被合并
        for (i, (a, _, _, _, _)) in raw.iter().enumerate() {
            if *a == "copy" || *a == "paste" || *a == "hotkey" {
                assert!(
                    plan.groups.iter().any(|g| g == &vec![i]),
                    "第 {} 步（{}）应保持独立，实际：{:?}",
                    i + 1, a, plan.groups
                );
            }
        }
    }

    /// AI 精简时，单步组如果已经有描述（多半是用户跑过「AI 生成步骤」的几十字详情），
    /// 绝不能被 AI 的一句话描述冲掉——AI 精简的职责是合并，不是重写全文。
    #[test]
    fn plan_to_groups_keeps_original_desc_for_single_step_groups() {
        let conn = mem_db();
        insert_full(&conn, "s0", 0, "click", "t0", "原描述0", "W", 1, 1, 0);
        insert_full(&conn, "s1", 1, "click", "t1", "", "W", 2, 2, 1000);
        insert_full(&conn, "s2", 2, "type", "t2", "原描述2", "W", 3, 3, 2000);
        let rows = load_merge_steps(&conn, "r1").unwrap();

        let plan = MergePlan {
            groups: vec![vec![0], vec![1, 2]],
            removed: vec![],
            descs: vec![
                Some("不该被采纳".to_string()),
                Some("合并后的说明".to_string()),
            ],
            titles: vec![],
        };
        let gs = plan_to_groups(&plan, &rows);
        assert_eq!(gs[0].desc, None, "单步组已有描述 → 必须保持原样");
        assert_eq!(gs[1].desc.as_deref(), Some("合并后的说明"), "多步组采纳 AI 描述");
    }

    #[test]
    fn plan_to_groups_fills_desc_when_single_step_has_none() {
        let conn = mem_db();
        insert_full(&conn, "s0", 0, "click", "t0", "", "W", 1, 1, 0); // 原描述为空
        let rows = load_merge_steps(&conn, "r1").unwrap();
        let plan = MergePlan {
            groups: vec![vec![0]],
            removed: vec![],
            descs: vec![Some("AI 补的描述".to_string())],
            titles: vec![],
        };
        let gs = plan_to_groups(&plan, &rows);
        assert_eq!(
            gs[0].desc.as_deref(),
            Some("AI 补的描述"),
            "原描述为空时应该采纳"
        );
    }

    #[test]
    fn short_window_truncates_long_titles() {
        assert_eq!(short_window(""), "-");
        assert_eq!(short_window("函数参数"), "函数参数");
        assert_eq!(short_window("WPS 表格 - 兼容模式"), "WPS 表格");
        let long = "啊".repeat(40);
        assert_eq!(short_window(&long).chars().count(), 25); // 24 + …
    }

    // ── 全局撤销/重做（edit_history 两栈）──

    fn step_count(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM steps WHERE recording_id='r1'", [], |r| r.get(0))
            .unwrap()
    }
    fn hist_count(conn: &rusqlite::Connection, stack: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM edit_history WHERE recording_id='r1' AND stack=?1",
            params![stack], |r| r.get(0)).unwrap()
    }

    #[test]
    fn edit_undo_redo_roundtrip_restores_steps_and_title() {
        let conn = mem_db();
        insert_step(&conn, "s1", 0, "click", "第一步", "d1");
        insert_step(&conn, "s2", 1, "click", "第二步", "d2");

        // 编辑前压栈 → 删一步 + 改标题
        push_edit_history(&conn, "r1").unwrap();
        conn.execute("DELETE FROM steps WHERE id='s1'", []).unwrap();
        conn.execute("UPDATE recordings SET title='改名了' WHERE id='r1'", []).unwrap();
        assert_eq!(step_count(&conn), 1);
        assert_eq!(hist_count(&conn, "undo"), 1);
        assert_eq!(hist_count(&conn, "redo"), 0);

        // 撤销：步骤恢复为 2，标题也恢复
        let n = edit_undo_conn_test(&conn, "r1").unwrap();
        assert_eq!(n, 2);
        assert_eq!(step_count(&conn), 2);
        let title: String = conn.query_row("SELECT title FROM recordings WHERE id='r1'", [], |r| r.get(0)).unwrap();
        assert_eq!(title, "测试录制");
        assert_eq!(hist_count(&conn, "undo"), 0);
        assert_eq!(hist_count(&conn, "redo"), 1);

        // 重做：回到「删一步+改名」之后的状态
        let n = pop_and_restore(&conn, "r1", "redo", "undo").unwrap();
        assert_eq!(n, 1);
        assert_eq!(step_count(&conn), 1);
        let title: String = conn.query_row("SELECT title FROM recordings WHERE id='r1'", [], |r| r.get(0)).unwrap();
        assert_eq!(title, "改名了");
        assert_eq!(hist_count(&conn, "undo"), 1);
        assert_eq!(hist_count(&conn, "redo"), 0);
    }

    // 测试用包装：直接吃连接（命令版本吃 Tauri State）
    fn edit_undo_conn_test(conn: &rusqlite::Connection, rid: &str) -> Result<usize, String> {
        super::pop_and_restore(conn, rid, "undo", "redo")
    }

    #[test]
    fn new_edit_invalidates_redo_stack() {
        let conn = mem_db();
        insert_step(&conn, "s1", 0, "click", "t", "d");
        push_edit_history(&conn, "r1").unwrap();
        conn.execute("UPDATE steps SET title='x' WHERE id='s1'", []).unwrap();
        pop_and_restore(&conn, "r1", "undo", "redo").unwrap();
        assert_eq!(hist_count(&conn, "redo"), 1);
        // 新编辑 → redo 清空
        push_edit_history(&conn, "r1").unwrap();
        assert_eq!(hist_count(&conn, "redo"), 0);
    }

    #[test]
    fn edit_history_prunes_to_limit() {
        let conn = mem_db();
        insert_step(&conn, "s1", 0, "click", "t", "d");
        for _ in 0..(super::EDIT_HISTORY_LIMIT + 5) {
            push_edit_history(&conn, "r1").unwrap();
        }
        assert_eq!(hist_count(&conn, "undo"), super::EDIT_HISTORY_LIMIT);
    }

    #[test]
    fn restore_blanks_missing_screenshot_file() {
        let conn = mem_db();
        // screenshot_path 指向一个不存在的文件
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type,              title, description, tip, screenshot_path, annotated_path, before_screenshot_path)              VALUES ('s1', 'r1', 0, 1, 'click', 't', 'd', '', 'gone.jpg', '', '')",
            [],
        ).unwrap();
        push_edit_history(&conn, "r1").unwrap();
        conn.execute("UPDATE steps SET screenshot_path='changed.jpg'", []).unwrap();
        pop_and_restore(&conn, "r1", "undo", "redo").unwrap();
        let p: String = conn.query_row("SELECT screenshot_path FROM steps WHERE id='s1'", [], |r| r.get(0)).unwrap();
        // 文件不存在 → 置空，不留悬空引用
        assert_eq!(p, "");
    }
}
