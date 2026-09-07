//! Stage 2a 精准执行器 Tauri 命令：读源 DB 坐标锚点做确定性重放。

use crate::DbState;
use crate::executor::backend::{DryRunBackend, ReplayBackend, WindowsBackend};
use crate::executor::replay::{load_replay_plan, run_action};
use crate::executor::verify::verify_step;
use serde::Serialize;
use std::time::Duration;

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
}

/// 执行（重放）一份录制。
///
/// - `dry_run=true`：只生成动作序列、不碰桌面，用于验证闭环/CI（对比动作序列）。
/// - `dry_run=false`：用 Win32 确定性重放真实点选 WPS 等桌面应用；
///   每步后抓屏与录制预期截图比对，差异超阈值标记为 `verify-fail`。
///
/// 已知边界（MVP）：同机同分辨率；drag 降级为 click（端点未存）；
/// 无坐标步仅聚焦窗口。详见 executor-stage2a-trae-brief.md。
#[tauri::command]
pub fn execute_recording(
    db: tauri::State<'_, DbState>,
    recording_id: String,
    dry_run: bool,
    step_delay_ms: Option<u64>,
) -> Result<ExecReport, String> {
    let delay = step_delay_ms.unwrap_or(300);
    let plan = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_replay_plan(&conn, &recording_id)?
    };
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

            run_action(action, &backend);

            if action.action_type == "drag" {
                note = "drag 降级为 click：源 DB 未存拖拽终点，Stage 2b 补齐".into();
            }

            // 校验门：执行后截图 vs 预期
            let diff = verify_step(&action.after_screenshot);
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

    Ok(ExecReport {
        recording_id,
        dry_run,
        total,
        outcomes,
    })
}
