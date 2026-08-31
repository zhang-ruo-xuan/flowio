//! 脱敏命令（P1-03）
//!
//! - `redact_recording_text`：对录制所有步骤的 title/description/tip 做正则脱敏，写回 DB
//! - `redact_step_screenshot`：对指定步骤的截图（或 before 截图）做区域高斯模糊，写回磁盘

use std::sync::atomic::{AtomicBool, Ordering};

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

/// 对录制全部步骤文本脱敏，返回替换条数。
#[tauri::command]
pub fn redact_recording_text(
    db: State<DbState>,
    recording_id: String,
) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // Phase A: 按设置中的脱敏配置（类别开关 + 自定义正则）执行
    let config = crate::settings::commands::read_redaction_config(&conn);
    let mut stmt = conn
        .prepare(
            "SELECT id, title, description, tip FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| format!("prepare steps failed: {}", e))?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([&recording_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| format!("query steps failed: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    let mut changed = 0usize;
    for (id, title, desc, tip) in rows {
        if let Some((new_title, new_desc, new_tip)) = redact_step_texts(&title, &desc, &tip, &config) {
            conn.execute(
                "UPDATE steps SET title = ?1, description = ?2, tip = ?3 WHERE id = ?4",
                rusqlite::params![new_title, new_desc, new_tip, id],
            )
            .map_err(|e| format!("update step failed: {}", e))?;
            changed += 1;
        }
    }
    // Mark recording as text-redacted after successful processing
    let _ = conn.execute(
        "UPDATE recordings SET text_redacted = 1 WHERE id = ?1",
        rusqlite::params![recording_id],
    );
    Ok(changed)
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

    // 先取路径列表并释放锁，避免 OCR 期间长时间占用 DB 连接
    let rows: Vec<(String, String, String, String)> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, screenshot_path, before_screenshot_path, after_screenshot_path FROM steps WHERE recording_id = ?1 ORDER BY order_index",
            )
            .map_err(|e| format!("prepare steps failed: {}", e))?;
        let mut out: Vec<(String, String, String, String)> = Vec::new();
        let rows = stmt
            .query_map([&recording_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(3)?.unwrap_or_default(),
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

    // 统计实际参与扫描的图片总数（存在的截图 + before 截图 + after 截图）
    let total = rows
        .iter()
        .flat_map(|(_, shot, before, after)| step_screenshot_slots(shot, before, after))
        .filter(|(_, p)| std::path::Path::new(p).exists())
        .count();

    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> ScanRedactResult {
        let mut done = 0usize;
        let mut redacted_count = 0usize;
        let mut failed_count = 0usize;

        for (step_id, shot, before, after) in rows {
            for (_field, p) in step_screenshot_slots(&shot, &before, &after) {
                if SCAN_CANCEL.load(Ordering::SeqCst) {
                    return ScanRedactResult {
                        total,
                        processed: done,
                        redacted_count,
                        failed_count,
                        cancelled: true,
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
}
