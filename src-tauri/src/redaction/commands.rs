//! 脱敏命令（P1-03）
//!
//! - `redact_recording_text`：对录制所有步骤的 title/description/tip 做正则脱敏，写回 DB
//! - `redact_step_screenshot`：对指定步骤的截图（或 before 截图）做区域高斯模糊，写回磁盘

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::redaction::{
    redact_image_full, redact_image_region, redact_screenshot_auto, RedactMode, RedactionConfig,
};
use crate::DbState;

/// 步骤文本脱敏三元组：对 title/description/tip 应用配置脱敏，
/// 任一字段发生变化时返回替换后的三元组，否则返回 `None`（纯逻辑）。
fn redact_step_texts(
    title: &str,
    desc: &str,
    tip: &str,
    config: &RedactionConfig,
) -> Option<(String, String, String)> {
    let new_title = crate::redaction::redact_text_with_config(title, config);
    let new_desc = crate::redaction::redact_text_with_config(desc, config);
    let new_tip = crate::redaction::redact_text_with_config(tip, config);
    if new_title != title || new_desc != desc || new_tip != tip {
        Some((new_title, new_desc, new_tip))
    } else {
        None
    }
}

/// 截图字段 → DB 列名映射（纯逻辑）。
fn screenshot_column_for(field: Option<&str>) -> &'static str {
    match field {
        Some("before") => "before_screenshot_path",
        _ => "screenshot_path",
    }
}

/// 判断是否整图模糊：区域宽/高非正时整图处理（纯逻辑）。
fn blur_full_image(w: i32, h: i32) -> bool {
    w <= 0 || h <= 0
}

/// 按固定顺序收集一张步骤图的非空图片槽位（screenshot/before/after）（纯逻辑）。
fn step_screenshot_slots<'a>(shot: &'a str, before: &'a str, after: &'a str) -> Vec<(&'static str, &'a str)> {
    let mut v = Vec::with_capacity(3);
    if !shot.is_empty() {
        v.push(("screenshot", shot));
    }
    if !before.is_empty() {
        v.push(("before", before));
    }
    if !after.is_empty() {
        v.push(("after", after));
    }
    v
}

/// 统计全部步骤的非空图片槽位数量（纯逻辑，不检查文件存在性）。
fn count_non_empty_slots(rows: &[(String, String, String, String)]) -> usize {
    rows.iter()
        .flat_map(|(_, shot, before, after)| step_screenshot_slots(shot, before, after))
        .count()
}

/// `redact_recording_text` 的返回结果。
///
/// 🔴 必须 camelCase：前端 `StepEditor.tsx` 按 `changed` / `droppedTranslations` 读，
/// 蛇形会静默失配（编译期查不出，见 2026-09-04「翻译中...」卡死的同源踩坑）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactTextResult {
    /// 原文被脱敏改动过的步骤数
    pub changed: usize,
    /// 同步作废的译文条目数（跨所有语言）
    pub dropped_translations: usize,
    /// ★ v310: 被改动步骤的序号（order_index，从 1 计），供前端结果弹窗逐条列出
    pub changed_indices: Vec<i64>,
}

/// 作废「原文被脱敏改动过」的那些步骤的译文副本（纯逻辑，便于单测）。
///
/// 为什么必须作废：译文是**从原文译出的派生物**，而脱敏只改原文。译文留着的话，
/// 切到英文导出/发布时被脱敏掉的手机号、身份证号会从旧译文里原样漏出来，
/// 脱敏形同虚设。作废后导出按 T6 回退到**已脱敏的原文**，不会留空白。
///
/// 为什么不是「对译文再跑一遍脱敏规则」：规则只认得它认得的形态，译文里换了
/// 说法的残留（口令关键词被译成 password、姓名音译等）它照样漏。
/// 宁可丢一条可重新生成的译文，也不留无法验证的内容。
///
/// `changed_order_indices` 是被改动的步骤 order_index 的**字符串**形式
/// （译文 `steps` 以 order_index 字符串为键）。返回真正删掉的条目数。
pub fn purge_translations_for_steps(
    translations: &mut Value,
    changed_order_indices: &[String],
) -> usize {
    let mut dropped = 0usize;
    let Some(langs) = translations.as_object_mut() else {
        return 0;
    };
    for (_lang, guide) in langs.iter_mut() {
        let Some(steps) = guide.get_mut("steps").and_then(|s| s.as_object_mut()) else {
            continue; // 该语言没有 steps 字段 → 无译文可作废
        };
        for key in changed_order_indices {
            // Map::remove 对不存在的键返回 None，天然不会虚高计数
            if steps.remove(key).is_some() {
                dropped += 1;
            }
        }
    }
    dropped
}

/// 对录制全部步骤文本脱敏，返回替换条数（含同步作废的译文条目数）。
#[tauri::command]
pub fn redact_recording_text(
    db: State<DbState>,
    recording_id: String,
) -> Result<RedactTextResult, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 🔴 脱敏功能总开关：关闭时后端强制拒绝（前端也会预检提示，这里是兜底）
    if !crate::settings::commands::read_redaction_master_enabled(&conn) {
        return Err("脱敏功能未开启：请到 设置 → 脱敏 打开「脱敏功能」".to_string());
    }
    // Phase A: 按设置中的脱敏配置（类别开关 + 自定义正则）执行
    let config = crate::settings::commands::read_redaction_config(&conn);
    // 类别全不勾 = 用户选择什么都不检测：直接明说，而不是静默跑空让用户以为坏了
    if config.enabled.is_empty() && config.custom_patterns.is_empty() {
        return Err("未启用任何脱敏类别：请到 设置 → 脱敏 勾选要检测的内容（手机号 / QQ号 / 邮箱…）".to_string());
    }
    // 文字脱敏会改写步骤标题/描述 → 先压全局撤销栈，脱敏结果可一键撤销
    let _ = crate::editor::commands::push_edit_history(&conn, &recording_id);
    let mut stmt = conn
        .prepare(
            "SELECT id, order_index, title, description, tip FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| format!("prepare steps failed: {}", e))?;
    let rows: Vec<(String, i64, String, String, String)> = stmt
        .query_map([&recording_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| format!("query steps failed: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    let mut changed = 0usize;
    let mut changed_indices: Vec<i64> = Vec::new();
    let mut changed_order_keys: Vec<String> = Vec::new();
    for (id, order_index, title, desc, tip) in rows {
        if let Some((new_title, new_desc, new_tip)) = redact_step_texts(&title, &desc, &tip, &config) {
            conn.execute(
                "UPDATE steps SET title = ?1, description = ?2, tip = ?3 WHERE id = ?4",
                rusqlite::params![new_title, new_desc, new_tip, id],
            )
            .map_err(|e| format!("update step failed: {}", e))?;
            changed += 1;
            changed_indices.push(order_index);
            changed_order_keys.push(order_index.to_string());
        }
    }

    // 🔴 隐私一致性：原文被改动的步骤，其译文副本一并作废。
    // 否则切到译文语言导出/发布时，脱敏掉的敏感信息会从旧译文里原样漏出来。
    let mut dropped_translations = 0usize;
    if changed > 0 {
        if let Ok(mut tr) = crate::ai::translate::load_translations(&conn, &recording_id) {
            let d = purge_translations_for_steps(&mut tr, &changed_order_keys);
            if d > 0 {
                let json = serde_json::to_string(&tr)
                    .map_err(|e| format!("译文字段序列化失败：{}", e))?;
                crate::db::set_recording_translations_raw(&conn, &recording_id, &json)?;
                dropped_translations = d;
            }
        }
        // 译文缺失或 JSON 损坏 → 无译文可作废，静默跳过（不阻断脱敏本身）
    }

    // Mark recording as text-redacted after successful processing
    let _ = conn.execute(
        "UPDATE recordings SET text_redacted = 1 WHERE id = ?1",
        rusqlite::params![recording_id],
    );
    Ok(RedactTextResult { changed, dropped_translations, changed_indices })
}

/// 对指定步骤的截图做区域高斯模糊并写回。
///
/// `field` 取值 `screenshot`（默认）或 `before`；`x/y/w/h` 为像素区域，传全 0 表示整图模糊。
/// 返回更新后的截图路径。
#[tauri::command]
pub fn redact_step_screenshot(
    db: State<DbState>,
    step_id: String,
    field: Option<String>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let col = screenshot_column_for(field.as_deref());
    let sql = format!("SELECT {} FROM steps WHERE id = ?1", col);
    let path: String = conn
        .query_row(&sql, [&step_id], |row| row.get(0))
        .map_err(|e| format!("step not found: {}", e))?;
    if path.is_empty() {
        return Err("step has no screenshot".to_string());
    }

    let img = image::open(&path).map_err(|e| format!("open image failed: {}", e))?;
    let blurred = if blur_full_image(w, h) {
        redact_image_full(&img)
    } else {
        redact_image_region(&img, x, y, w, h)
    };
    blurred
        .save(&path)
        .map_err(|e| format!("save blurred image failed: {}", e))?;
    Ok(path)
}

/// 扫描取消标志：点击取消命令置 true；每次扫描开始时复位。
static SCAN_CANCEL: AtomicBool = AtomicBool::new(false);

/// 扫描脱敏结果（序列化为 camelCase 供前端读取）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRedactResult {
    /// 本次参与扫描的图片总数（存在的截图 + before 截图）
    pub total: usize,
    /// 实际处理完成的图片数
    pub processed: usize,
    /// 发生脱敏（有命中并写回）的图片数
    pub redacted_count: usize,
    /// 处理失败（OCR 超时/读取失败等被跳过）的图片数
    pub failed_count: usize,
    /// 是否被用户取消
    pub cancelled: bool,
    /// ★ v310: 发生脱敏的步骤序号（order_index，从 1 计；一步多图只记一次），供前端结果弹窗列出
    pub redacted_steps: Vec<i64>,
}

/// 单图进度事件负载。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgressPayload {
    done: usize,
    total: usize,
    current_step_id: String,
}

/// 取消正在进行的截图扫描。幂等：未在扫描时调用无副作用。
#[tauri::command]
pub fn cancel_scan_redact() -> Result<(), String> {
    SCAN_CANCEL.store(true, Ordering::SeqCst);
    Ok(())
}

/// 自动扫描录制全部步骤截图（含 before 截图）中的敏感文字区域并脱敏（Phase B）。
///
/// 本地 OCR（Windows 内置引擎）逐行识别 → 按 redaction_config 检测敏感内容 →
/// 对命中行做模糊（默认）或实心色块覆盖（mode="block"），写回原图。
///
/// 卡死修复（回归 P1-03）：
/// - async 命令 + `spawn_blocking` 后台分批处理，不阻塞 UI；
/// - 每处理一张图 emit `scan_redact_progress`（{done,total,currentStepId}）供前端进度条；
/// - 支持 `cancel_scan_redact` 中途取消（AtomicBool 标志，幂等）；
/// - 单图 OCR 超时（10s）自动跳过该图继续，失败计入 failed_count。
///
/// 返回 `ScanRedactResult { total, processed, redactedCount, failedCount, cancelled }`。
#[tauri::command]
pub async fn scan_redact_screenshots(
    app: AppHandle,
    db: State<'_, DbState>,
    recording_id: String,
    mode: Option<String>,
) -> Result<ScanRedactResult, String> {
    let rmode = RedactMode::from_key(mode.as_deref());
    // 每次扫描开始复位取消标志
    SCAN_CANCEL.store(false, Ordering::SeqCst);

    // 读取教程标题，供完成提醒展示（失败不致命，降级为空串）
    let title = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT title FROM recordings WHERE id = ?1",
            rusqlite::params![recording_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
    };

    // 先取路径列表并释放锁，避免 OCR 期间长时间占用 DB 连接
    let rows: Vec<(String, i64, String, String, String)> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        // 🔴 脱敏功能总开关：关闭时后端强制拒绝（前端也会预检提示，这里是兜底）
        if !crate::settings::commands::read_redaction_master_enabled(&conn) {
            return Err("脱敏功能未开启：请到 设置 → 脱敏 打开「脱敏功能」".to_string());
        }
        let mut stmt = conn
            .prepare(
                "SELECT id, order_index, screenshot_path, before_screenshot_path, after_screenshot_path FROM steps WHERE recording_id = ?1 ORDER BY order_index",
            )
            .map_err(|e| format!("prepare steps failed: {}", e))?;
        let mut out: Vec<(String, i64, String, String, String)> = Vec::new();
        let rows = stmt
            .query_map([&recording_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                ))
            })
            .map_err(|e| format!("query steps failed: {}", e))?;
        for r in rows.flatten() {
            out.push(r);
        }
        out
    };

    // 锁外：重新读取配置
    let config = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::settings::commands::read_redaction_config(&conn)
    };
    // 类别全不勾 = 用户选择什么都不检测：直接明说，而不是静默跑空让用户以为坏了
    if config.enabled.is_empty() && config.custom_patterns.is_empty() {
        return Err("未启用任何脱敏类别：请到 设置 → 脱敏 勾选要检测的内容（手机号 / QQ号 / 邮箱…）".to_string());
    }

    // 统计实际参与扫描的图片总数（存在的截图 + before 截图 + after 截图）
    let total = rows
        .iter()
        .flat_map(|(_, _, shot, before, after)| step_screenshot_slots(shot, before, after))
        .filter(|(_, p)| std::path::Path::new(p).exists())
        .count();

    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> ScanRedactResult {
        let mut done = 0usize;
        let mut redacted_count = 0usize;
        let mut failed_count = 0usize;
        let mut redacted_steps_set: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();

        for (step_id, order_index, shot, before, after) in rows {
            for (_field, p) in step_screenshot_slots(&shot, &before, &after) {
                if SCAN_CANCEL.load(Ordering::SeqCst) {
                    return ScanRedactResult {
                        total,
                        processed: done,
                        redacted_count,
                        failed_count,
                        cancelled: true,
                        redacted_steps: redacted_steps_set.into_iter().collect(),
                    };
                }
                if !std::path::Path::new(p).exists() {
                    continue;
                }
                done += 1;
                // 每处理一张图推送进度
                let payload = ScanProgressPayload {
                    done,
                    total,
                    current_step_id: step_id.clone(),
                };
                let _ = app2.emit("scan_redact_progress", &payload);
                match redact_screenshot_auto(std::path::Path::new(p), &config, rmode) {
                    Ok(h) if h > 0 => {
                        redacted_count += 1;
                        redacted_steps_set.insert(order_index);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        failed_count += 1;
                        log::warn!("scan redact skipped {}: {}", p, e);
                    }
                }
            }
        }

        ScanRedactResult {
            total,
            processed: done,
            redacted_count,
            failed_count,
            cancelled: false,
            redacted_steps: redacted_steps_set.into_iter().collect(),
        }
    })
    .await
    .map_err(|e| format!("scan thread panicked: {}", e))?;

    // Mark recording as scan-redacted after successful processing
    let _ = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE recordings SET scan_redacted = 1 WHERE id = ?1",
            rusqlite::params![recording_id],
        )
    };

    // 结束通知（供前端兜底刷新）
    let _ = app.emit("scan_redact_done", &result);

    // 全局完成提醒（任意页面弹窗 + Windows 系统通知，带教程名）
    let ok = !result.cancelled;
    let summary = if result.cancelled {
        "扫描脱敏已取消".to_string()
    } else {
        format!(
            "扫描脱敏完成：处理 {} 张，脱敏 {} 张，失败 {} 张",
            result.processed, result.redacted_count, result.failed_count
        )
    };
    crate::job_notify::emit_job_done(
        &app,
        &recording_id,
        &title,
        crate::job_notify::JobOp::ScanRedact,
        ok,
        &summary,
    );
    Ok(result)
}

#[cfg(test)]
mod commands_tests {
    use super::*;
    use crate::redaction::RedactionConfig;

    fn phone_only_config() -> RedactionConfig {
        RedactionConfig {
            enabled: vec![crate::redaction::SensitiveKind::Phone],
            custom_patterns: vec![],
        }
    }

    fn all_config() -> RedactionConfig {
        RedactionConfig {
            enabled: crate::redaction::SensitiveKind::ALL.to_vec(),
            custom_patterns: vec![],
        }
    }

    // ── screenshot_column_for ───────────────────────────

    #[test]
    fn column_default_screenshot() {
        assert_eq!(screenshot_column_for(None), "screenshot_path");
        assert_eq!(screenshot_column_for(Some("screenshot")), "screenshot_path");
        assert_eq!(screenshot_column_for(Some("after")), "screenshot_path");
        assert_eq!(screenshot_column_for(Some("")), "screenshot_path");
    }

    #[test]
    fn column_before() {
        assert_eq!(screenshot_column_for(Some("before")), "before_screenshot_path");
    }

    // ── blur_full_image ─────────────────────────────────

    #[test]
    fn blur_full_on_zero_or_negative() {
        assert!(blur_full_image(0, 0));
        assert!(blur_full_image(0, 100));
        assert!(blur_full_image(100, 0));
        assert!(blur_full_image(-1, 50));
        assert!(blur_full_image(50, -1));
        assert!(blur_full_image(-5, -5));
    }

    #[test]
    fn blur_region_on_positive() {
        assert!(!blur_full_image(1, 1));
        assert!(!blur_full_image(100, 200));
        assert!(!blur_full_image(1, 1000));
    }

    // ── redact_step_texts ───────────────────────────────

    #[test]
    fn redact_texts_no_change_returns_none() {
        let cfg = phone_only_config();
        assert!(redact_step_texts("普通标题", "普通描述", "普通提示", &cfg).is_none());
    }

    #[test]
    fn redact_texts_title_changed() {
        let cfg = phone_only_config();
        let r = redact_step_texts("联系 13800138000", "普通描述", "普通提示", &cfg);
        let (t, d, tip) = r.expect("should change");
        assert_ne!(t, "联系 13800138000");
        assert_eq!(d, "普通描述");
        assert_eq!(tip, "普通提示");
    }

    #[test]
    fn redact_texts_desc_changed() {
        let cfg = phone_only_config();
        let r = redact_step_texts("标题", "电话 13912345678", "提示", &cfg);
        let (t, d, tip) = r.expect("should change");
        assert_eq!(t, "标题");
        assert_ne!(d, "电话 13912345678");
        assert_eq!(tip, "提示");
    }

    #[test]
    fn redact_texts_tip_changed() {
        let cfg = phone_only_config();
        let r = redact_step_texts("标题", "描述", "手机 13700001111", &cfg);
        let (t, d, tip) = r.expect("should change");
        assert_eq!(t, "标题");
        assert_eq!(d, "描述");
        assert_ne!(tip, "手机 13700001111");
    }

    #[test]
    fn redact_texts_all_three_changed() {
        let cfg = all_config();
        let r = redact_step_texts(
            "电话 13800138000",
            "邮箱 a@b.com",
            "身份证 110101199003078888",
            &cfg,
        );
        let (t, d, tip) = r.expect("should change");
        assert_ne!(t, "电话 13800138000");
        assert_ne!(d, "邮箱 a@b.com");
        assert_ne!(tip, "身份证 110101199003078888");
    }

    #[test]
    fn redact_texts_empty_inputs() {
        let cfg = all_config();
        // 空字符串不应 panic，且不应触发变化
        assert!(redact_step_texts("", "", "", &cfg).is_none());
    }

    #[test]
    fn redact_texts_phone_only_keeps_email() {
        let cfg = phone_only_config();
        // 仅启用 phone，邮箱不应被脱敏
        let r = redact_step_texts("邮箱 a@b.com 电话 13800138000", "描述", "提示", &cfg);
        let (t, _, _) = r.expect("should change");
        assert!(t.contains("a@b.com"));
        assert!(!t.contains("13800138000"));
    }

    // ── step_screenshot_slots ───────────────────────────

    #[test]
    fn slots_all_empty() {
        assert!(step_screenshot_slots("", "", "").is_empty());
    }

    #[test]
    fn slots_only_screenshot() {
        let v = step_screenshot_slots("a.png", "", "");
        assert_eq!(v, vec![("screenshot", "a.png")]);
    }

    #[test]
    fn slots_order_screenshot_before_after() {
        let v = step_screenshot_slots("a.png", "b.png", "c.png");
        assert_eq!(
            v,
            vec![("screenshot", "a.png"), ("before", "b.png"), ("after", "c.png")]
        );
    }

    #[test]
    fn slots_skips_middle_empty() {
        let v = step_screenshot_slots("a.png", "", "c.png");
        assert_eq!(v, vec![("screenshot", "a.png"), ("after", "c.png")]);
    }

    #[test]
    fn slots_skips_first_empty() {
        let v = step_screenshot_slots("", "b.png", "c.png");
        assert_eq!(v, vec![("before", "b.png"), ("after", "c.png")]);
    }

    // ── count_non_empty_slots ───────────────────────────

    #[test]
    fn count_slots_empty_rows() {
        let rows: Vec<(String, String, String, String)> = vec![];
        assert_eq!(count_non_empty_slots(&rows), 0);
    }

    #[test]
    fn count_slots_mixed() {
        let rows = vec![
            (
                "s1".to_string(),
                "a.png".to_string(),
                "".to_string(),
                "c.png".to_string(),
            ),
            (
                "s2".to_string(),
                "".to_string(),
                "b.png".to_string(),
                "".to_string(),
            ),
        ];
        // s1: 2 非空, s2: 1 非空 → 3
        assert_eq!(count_non_empty_slots(&rows), 3);
    }

    #[test]
    fn count_slots_all_empty() {
        let rows = vec![(
            "s1".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
        )];
        assert_eq!(count_non_empty_slots(&rows), 0);
    }

    // ── purge_translations_for_steps ────────────────────
    // 隐私一致性：原文被脱敏改动过的步骤，其译文副本必须作废，
    // 否则切到译文语言导出时敏感信息会从旧译文里原样漏出来。

    fn sample_translations() -> Value {
        serde_json::json!({
            "en": {
                "title": "EN Title",
                "steps": {
                    "1": { "title": "One", "description": "desc one" },
                    "2": { "title": "Two", "description": "desc two" },
                    "3": { "title": "Three", "description": "desc three" }
                }
            },
            "ja": {
                "title": "JA Title",
                "steps": {
                    "1": { "title": "イチ", "description": "説明1" },
                    "2": { "title": "ニ", "description": "説明2" }
                }
            }
        })
    }

    #[test]
    fn purge_drops_changed_steps_in_every_lang() {
        let mut tr = sample_translations();
        let changed = vec!["2".to_string()];
        assert_eq!(purge_translations_for_steps(&mut tr, &changed), 2);
        assert!(tr["en"]["steps"].get("2").is_none());
        assert!(tr["ja"]["steps"].get("2").is_none());
        // 未改动的步骤保留
        assert_eq!(tr["en"]["steps"]["1"]["title"], "One");
        assert_eq!(tr["en"]["steps"]["3"]["title"], "Three");
        // 标题译文不属于步骤，不受影响
        assert_eq!(tr["en"]["title"], "EN Title");
    }

    #[test]
    fn purge_no_change_keeps_everything() {
        let mut tr = sample_translations();
        let before = tr.clone();
        assert_eq!(purge_translations_for_steps(&mut tr, &[]), 0);
        assert_eq!(tr, before);
    }

    #[test]
    fn purge_only_counts_keys_that_existed() {
        let mut tr = sample_translations();
        // ja 没有第 3 步 → 只应算 en 那一条
        assert_eq!(purge_translations_for_steps(&mut tr, &["3".to_string()]), 1);
        assert_eq!(tr["ja"]["steps"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn purge_missing_key_is_noop() {
        let mut tr = sample_translations();
        let before = tr.clone();
        assert_eq!(purge_translations_for_steps(&mut tr, &["99".to_string()]), 0);
        assert_eq!(tr, before);
    }

    #[test]
    fn purge_tolerates_malformed_shapes() {
        // 非对象 / 缺 steps / steps 非对象：都不应 panic，一律 0
        let mut not_obj = serde_json::json!([1, 2, 3]);
        assert_eq!(purge_translations_for_steps(&mut not_obj, &["1".to_string()]), 0);

        let mut no_steps = serde_json::json!({ "en": { "title": "T" } });
        assert_eq!(purge_translations_for_steps(&mut no_steps, &["1".to_string()]), 0);

        let mut steps_not_obj = serde_json::json!({ "en": { "steps": [1, 2] } });
        assert_eq!(purge_translations_for_steps(&mut steps_not_obj, &["1".to_string()]), 0);

        let mut empty = serde_json::json!({});
        assert_eq!(purge_translations_for_steps(&mut empty, &["1".to_string()]), 0);
    }
}
