//! Stage 2a/2b 精准执行器 Tauri 命令 + Stage 3 审计/审批层。
//!
//! 审计层（core，非破坏性）：每次 `execute_recording` 调用都落盘一条 `replay_audit`
//! 记录（含完整 ExecReport JSON），无论成功/失败/仅预览，都可供「审计历史」追溯。
//! 审批层（可选闸门）：当 `approval_mode="check"` 且计划含敏感步骤（hotkey/copy/
//! paste/drag）且用户尚未批准时，直接返回 `needs-approval` 且**不下发任何桌面动作**，
//! 由前端先调 `preview_replay` 展示计划、用户点「批准」后再带 `approved=true` 重放。

use crate::DbState;
use crate::executor::backend::{DryRunBackend, WindowsBackend};
use crate::executor::replay::{
    load_replay_plan, run_action, classify_sensitivity, plan_requires_approval, approval_engaged,
    ReplayAction,
};
use crate::executor::verify::verify_step;
use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use std::time::Duration;
use uuid::Uuid;

/// 单步执行结果（审计/校验产物）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutcome {
    pub step_number: i32,
    pub action_type: String,
    /// ok | dry-run | verify-fail | verify-skip
    pub status: String,
    pub note: String,
    /// 执行后截图与预期截图的差异率（0~1），无校验时为 null
    pub diff_ratio: Option<f32>,
}

/// 整份录制的执行报告
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecReport {
    pub recording_id: String,
    pub dry_run: bool,
    pub total: usize,
    pub outcomes: Vec<StepOutcome>,
    /// 整体状态：success | partial | failed | needs-approval | no-steps
    pub overall_status: String,
}

/// 单步重放预览（前端审批 UI 用，不执行）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStep {
    pub step_number: i32,
    pub action_type: String,
    /// 人类可读的该步目标描述（坐标 / 控件 / 组合键 / 文本输入等）
    pub target: String,
    /// 是否敏感步骤（需审批）
    pub sensitive: bool,
}

/// 重放预览（不执行、不碰桌面）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayPreview {
    pub recording_id: String,
    pub total: usize,
    pub requires_approval: bool,
    pub steps: Vec<PreviewStep>,
}

/// 审计历史记录（get_replay_audit 列表视图，不含完整 report_json 以控体积）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    pub run_id: String,
    pub recording_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub dry_run: bool,
    pub total_steps: i32,
    pub ok_steps: i32,
    pub failed_steps: i32,
    pub overall_status: String,
    pub target_app: String,
}

/// 取计划的主要目标应用（首个非空 window_title），用于审计留痕。
fn primary_target_app(plan: &[ReplayAction]) -> String {
    plan.iter()
        .filter_map(|a| {
            if a.window_title.trim().is_empty() {
                None
            } else {
                Some(a.window_title.clone())
            }
        })
        .next()
        .unwrap_or_default()
}

/// 生成单步的人类可读目标描述（供审批 UI 展示）。
fn describe_target(action: &ReplayAction) -> String {
    match action.action_type.as_str() {
        "click" | "drag" => {
            if let Some(anchor) = action.uia_anchor.as_deref() {
                if !anchor.trim().is_empty() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(anchor) {
                        let cls = v.get("cls").and_then(|c| c.as_str()).unwrap_or("控件");
                        return format!("控件 {} @ 归一化锚点", cls);
                    }
                }
            }
            if let (Some(x), Some(y)) = (action.x, action.y) {
                format!("坐标 ({}, {})", x, y)
            } else {
                "窗口内点击".into()
            }
        }
        "type" | "input" => "输入文本".into(),
        "copy" => "复制（剪贴板）".into(),
        "paste" => "粘贴（剪贴板）".into(),
        "hotkey" => format!("组合键 {}", action.hotkey_combo.clone().unwrap_or_default()),
        _ => {
            if action.window_title.trim().is_empty() {
                "（未知动作）".into()
            } else {
                format!("聚焦窗口 {}", action.window_title)
            }
        }
    }
}

/// 把一次执行报告持久化到 `replay_audit`（Stage 3 审计落盘，非破坏性）。
fn persist_audit(
    conn: &rusqlite::Connection,
    run_id: &str,
    recording_id: &str,
    started_at: &str,
    finished_at: &str,
    dry_run: bool,
    report: &ExecReport,
    target_app: &str,
) -> Result<(), String> {
    let total = report.total as i32;
    let ok = report
        .outcomes
        .iter()
        .filter(|o| o.status == "ok" || o.status == "dry-run")
        .count() as i32;
    let failed = (total - ok).max(0);
    let json = serde_json::to_string(report).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO replay_audit \
         (run_id, recording_id, started_at, finished_at, dry_run, total_steps, ok_steps, failed_steps, overall_status, target_app, report_json) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            run_id,
            recording_id,
            started_at,
            finished_at,
            dry_run as i32,
            total,
            ok,
            failed,
            report.overall_status,
            target_app,
            json
        ],
    )
    .map_err(|e| format!("persist audit failed: {}", e))?;
    Ok(())
}

/// 把一行 rusqlite 查询结果映射为 AuditRecord。
fn map_audit_row(row: &rusqlite::Row) -> Result<AuditRecord, rusqlite::Error> {
    Ok(AuditRecord {
        run_id: row.get(0)?,
        recording_id: row.get(1)?,
        started_at: row.get(2)?,
        finished_at: row.get(3)?,
        dry_run: row.get::<_, i32>(4)? != 0,
        total_steps: row.get(5)?,
        ok_steps: row.get(6)?,
        failed_steps: row.get(7)?,
        overall_status: row.get(8)?,
        target_app: row.get(9)?,
    })
}

/// 执行（重放）一份录制。
///
/// - `dry_run=true`：只生成动作序列、不碰桌面，用于验证闭环/CI（对比动作序列）。
/// - `dry_run=false`：用 Win32 确定性重放真实点选 WPS 等桌面应用；
///   每步后抓屏与录制预期截图比对，差异超阈值标记为 `verify-fail`。
///
/// Stage 3 扩展（均向后兼容，默认不启用闸门）：
/// - `approval_mode`："off"（默认）永不上闸；"check" 时若计划含敏感步骤且未批准则
///   返回 `overall_status="needs-approval"` 且**不下发任何桌面动作**。
/// - `approved`：用户在预览 UI 点「批准」后传 true，解除闸门。
///
/// 已知边界（MVP）：同机同分辨率；drag 降级为 click（端点未存）；
/// 无坐标步仅聚焦窗口。详见 executor-stage2a-trae-brief.md。
#[tauri::command]
pub fn execute_recording(
    db: tauri::State<'_, DbState>,
    recording_id: String,
    dry_run: bool,
    step_delay_ms: Option<u64>,
    approval_mode: Option<String>,
    approved: Option<bool>,
) -> Result<ExecReport, String> {
    let delay = step_delay_ms.unwrap_or(300);
    let run_id = Uuid::new_v4().to_string();
    let started_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let plan = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_replay_plan(&conn, &recording_id)?
    };

    // ── Stage 3 审批闸门：上闸则不做任何桌面动作，仅留痕并返回 needs-approval ──
    let requires = plan_requires_approval(&plan);
    let mode = approval_mode.unwrap_or_else(|| "off".to_string());
    let approved = approved.unwrap_or(false);
    if approval_engaged(requires, &mode, approved) {
        let finished_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let target_app = primary_target_app(&plan);
        let report = ExecReport {
            recording_id: recording_id.clone(),
            dry_run,
            total: plan.len(),
            outcomes: vec![],
            overall_status: "needs-approval".into(),
        };
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            persist_audit(
                &conn,
                &run_id,
                &recording_id,
                &started_at,
                &finished_at,
                dry_run,
                &report,
                &target_app,
            )?;
        }
        return Ok(report);
    }

    // ── 正常重放（与 Stage 2a/2b 逻辑一致）──
    let total = plan.len();
    let mut outcomes = Vec::with_capacity(total);

    if dry_run {
        let backend = DryRunBackend::new();
        for action in &plan {
            run_action(action, &backend);
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
        }
        let actions = backend.actions.lock().unwrap().clone();
        for (i, action) in plan.iter().enumerate() {
            outcomes.push(StepOutcome {
                step_number: action.step_number,
                action_type: action.action_type.clone(),
                status: "dry-run".into(),
                note: actions.get(i).cloned().unwrap_or_default(),
                diff_ratio: None,
            });
        }
    } else {
        let backend = WindowsBackend;
        for action in &plan {
            let mut status = "ok".to_string();
            let mut note = String::new();
            let mut diff: Option<f32> = None;

            // 单步隔离：任何一步的 unsafe Windows 调用或截图校验若 panic，
            // 只标记该步失败并继续，绝不拖垮整个进程（直接缓解"闪退"）。
            let step_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_action(action, &backend);
                verify_step(&action.after_screenshot)
            }));
            match step_result {
                Ok(d) => diff = d,
                Err(_) => {
                    status = "step-panic".into();
                    note = "该步执行时发生异常，已隔离，不影响后续步骤".into();
                }
            }

            if status != "step-panic" {
                if action.action_type == "drag" {
                    note = "drag 降级为 click：源 DB 未存拖拽终点，Stage 2b 补齐".into();
                }

                // 校验门：执行后截图 vs 预期
                if let Some(d) = diff {
                    if d > 0.08 {
                        status = "verify-fail".into();
                        note = format!("截图差异 {:.2} 超阈值 0.08", d);
                    }
                } else if action
                    .after_screenshot
                    .as_deref()
                    .map_or(false, |p| !p.is_empty())
                {
                    note = "verify-skip：抓屏或预期图不可用".into();
                }
            }

            outcomes.push(StepOutcome {
                step_number: action.step_number,
                action_type: action.action_type.clone(),
                status,
                note,
                diff_ratio: diff,
            });

            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
        }
    }

    let ok = outcomes
        .iter()
        .filter(|o| o.status == "ok" || o.status == "dry-run")
        .count();
    let failed = total.saturating_sub(ok);
    let overall_status = if total == 0 {
        "no-steps".into()
    } else if failed == 0 {
        "success".into()
    } else if ok == 0 {
        "failed".into()
    } else {
        "partial".into()
    };

    let finished_at = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let target_app = primary_target_app(&plan);
    let report = ExecReport {
        recording_id: recording_id.clone(),
        dry_run,
        total,
        outcomes,
        overall_status,
    };
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        persist_audit(
            &conn,
            &run_id,
            &recording_id,
            &started_at,
            &finished_at,
            dry_run,
            &report,
            &target_app,
        )?;
    }
    Ok(report)
}

/// 解析重放计划用于审批 UI（不执行、不碰桌面）。
#[tauri::command]
pub fn preview_replay(
    db: tauri::State<'_, DbState>,
    recording_id: String,
) -> Result<ReplayPreview, String> {
    let plan = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_replay_plan(&conn, &recording_id)?
    };
    let requires = plan_requires_approval(&plan);
    let steps = plan
        .iter()
        .map(|a| PreviewStep {
            step_number: a.step_number,
            action_type: a.action_type.clone(),
            target: describe_target(a),
            sensitive: classify_sensitivity(&a.action_type),
        })
        .collect();
    Ok(ReplayPreview {
        recording_id,
        total: plan.len(),
        requires_approval: requires,
        steps,
    })
}

/// 查询重放审计历史（Stage 3）。`recording_id` 为 None 时返回全部（按时间倒序）。
#[tauri::command]
pub fn get_replay_audit(
    db: tauri::State<'_, DbState>,
    recording_id: Option<String>,
) -> Result<Vec<AuditRecord>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut records: Vec<AuditRecord> = Vec::new();
    match &recording_id {
        Some(rid) => {
            let mut stmt = conn
                .prepare(
                    "SELECT run_id, recording_id, started_at, finished_at, dry_run, total_steps, ok_steps, failed_steps, overall_status, target_app \
                     FROM replay_audit WHERE recording_id = ?1 ORDER BY started_at DESC",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![rid], map_audit_row)
                .map_err(|e| e.to_string())?;
            for r in rows {
                records.push(r.map_err(|e| e.to_string())?);
            }
        }
        None => {
            let mut stmt = conn
                .prepare(
                    "SELECT run_id, recording_id, started_at, finished_at, dry_run, total_steps, ok_steps, failed_steps, overall_status, target_app \
                     FROM replay_audit ORDER BY started_at DESC",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], map_audit_row)
                .map_err(|e| e.to_string())?;
            for r in rows {
                records.push(r.map_err(|e| e.to_string())?);
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn audit_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE replay_audit (
                run_id TEXT PRIMARY KEY, recording_id TEXT, started_at TEXT, finished_at TEXT,
                dry_run INTEGER, total_steps INTEGER, ok_steps INTEGER, failed_steps INTEGER,
                overall_status TEXT, target_app TEXT, report_json TEXT
            )",
        )
        .unwrap();
        conn
    }

    #[test]
    fn persist_audit_writes_row_and_roundtrips() {
        let conn = audit_db();
        let report = ExecReport {
            recording_id: "rec1".into(),
            dry_run: false,
            total: 3,
            outcomes: vec![
                StepOutcome { step_number: 1, action_type: "click".into(), status: "ok".into(), note: "".into(), diff_ratio: None },
                StepOutcome { step_number: 2, action_type: "copy".into(), status: "ok".into(), note: "".into(), diff_ratio: None },
                StepOutcome { step_number: 3, action_type: "type".into(), status: "verify-fail".into(), note: "diff".into(), diff_ratio: Some(0.2) },
            ],
            overall_status: "partial".into(),
        };
        persist_audit(&conn, "run-1", "rec1", "2026-09-07 10:00:00", "2026-09-07 10:00:05", false, &report, "WPS").unwrap();

        let (total, ok, failed, status, target, json): (i32, i32, i32, String, String, String) = conn
            .query_row(
                "SELECT total_steps, ok_steps, failed_steps, overall_status, target_app, report_json FROM replay_audit WHERE run_id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(ok, 2); // click + copy
        assert_eq!(failed, 1); // type verify-fail
        assert_eq!(status, "partial");
        assert_eq!(target, "WPS");
        assert!(
            json.contains("\"overallStatus\":\"partial\""),
            "report_json 应保留 camelCase 字段，got: {}",
            json
        );
    }

    #[test]
    fn persist_audit_needs_approval_status() {
        let conn = audit_db();
        let report = ExecReport {
            recording_id: "rec2".into(),
            dry_run: false,
            total: 2,
            outcomes: vec![],
            overall_status: "needs-approval".into(),
        };
        persist_audit(&conn, "run-2", "rec2", "t", "t", false, &report, "App").unwrap();
        let status: String = conn
            .query_row("SELECT overall_status FROM replay_audit WHERE run_id='run-2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "needs-approval");
    }

    #[test]
    fn describe_target_variants() {
        let click = ReplayAction { step_number: 1, action_type: "click".into(), x: Some(10), y: Some(20), window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: None, uia_anchor: None };
        assert_eq!(describe_target(&click), "坐标 (10, 20)");

        let anchor = ReplayAction { step_number: 2, action_type: "click".into(), x: Some(1), y: Some(2), window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: None, uia_anchor: Some("{\"cls\":\"Button\",\"fx\":0.5,\"fy\":0.5,\"wx\":0.3,\"wy\":0.2}".into()) };
        assert!(describe_target(&anchor).contains("Button"), "got: {}", describe_target(&anchor));

        let copy = ReplayAction { step_number: 3, action_type: "copy".into(), x: None, y: None, window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: Some("Ctrl+C".into()), uia_anchor: None };
        assert_eq!(describe_target(&copy), "复制（剪贴板）");

        let hk = ReplayAction { step_number: 4, action_type: "hotkey".into(), x: None, y: None, window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: Some("Ctrl+S".into()), uia_anchor: None };
        assert_eq!(describe_target(&hk), "组合键 Ctrl+S");

        let ty = ReplayAction { step_number: 5, action_type: "type".into(), x: None, y: None, window_title: "WPS".into(), text: Some("hi".into()), after_screenshot: None, hotkey_combo: None, uia_anchor: None };
        assert_eq!(describe_target(&ty), "输入文本");
    }

    #[test]
    fn primary_target_app_picks_first_nonempty() {
        let plan: Vec<ReplayAction> = vec![
            ReplayAction { step_number: 1, action_type: "click".into(), x: Some(1), y: Some(1), window_title: "".into(), text: None, after_screenshot: None, hotkey_combo: None, uia_anchor: None },
            ReplayAction { step_number: 2, action_type: "type".into(), x: None, y: None, window_title: "WPS 文档".into(), text: None, after_screenshot: None, hotkey_combo: None, uia_anchor: None },
        ];
        assert_eq!(primary_target_app(&plan), "WPS 文档");
    }
}
