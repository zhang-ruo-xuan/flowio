//! 长任务完成后的统一回传通道。
//!
//! 所有「退出教程页也不应中断」的后台长任务（AI 精简 / 翻译 / 语音转写 /
//! 扫描脱敏 / 导出）在**各自完成后**都调用 [`emit_job_done`]：
//!
//! 1. 向前端全局层 emit `flowio://job-done` 事件（载荷含 recording_id / title /
//!    op / ok / summary），由 App 根挂载的卡片层在**任何页面**展示并附带教程名；
//! 2. 同时发一条 Windows 系统通知（软件最小化 / 后台也可见），失败静默降级。
//!
//! 注意：本模块只负责「完成后通知」，不负责「后台执行」——后台执行由各任务
//! 自己的 spawn thread / spawn_blocking 保证（SPA 内部路由切换不杀后端进程）。

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

/// 操作类型 → 前端展示用的中文标签（映射集中在前端，这里仅留机器键）。
#[derive(Debug, Clone, Copy)]
pub enum JobOp {
    AiSimplify,
    /// 录制结束后自动生成「流程摘要」（第十六轮）
    AiSummary,
    Translate,
    Voice,
    ScanRedact,
    Export,
}

impl JobOp {
    pub fn key(&self) -> &'static str {
        match self {
            JobOp::AiSimplify => "ai_simplify",
            JobOp::AiSummary => "ai_summary",
            JobOp::Translate => "translate",
            JobOp::Voice => "voice",
            JobOp::ScanRedact => "scan_redact",
            JobOp::Export => "export",
        }
    }
}

/// 长任务完成事件载荷。所有字段均为前端全局卡片层展示所需。
#[derive(Clone, Debug, Serialize)]
pub struct JobDoneEvent {
    pub recording_id: String,
    pub title: String,
    /// 机器操作键，前端据此渲染「AI 精简完成」等文案。
    pub op: String,
    pub ok: bool,
    pub summary: String,
    /// 任务产出的文件绝对路径（如导出完成后的文件）；非文件类任务为 None。
    pub file_path: Option<String>,
}

/// 统一完成回传：emit 前端事件 + 发 Windows 系统通知。
///
/// 任何子步骤失败都必须**静默降级**（不会让长任务本身失败或崩进程）。
pub fn emit_job_done(
    app: &AppHandle,
    recording_id: &str,
    title: &str,
    op: JobOp,
    ok: bool,
    summary: &str,
    file_path: Option<&str>,
) {
    let payload = JobDoneEvent {
        recording_id: recording_id.to_string(),
        title: title.to_string(),
        op: op.key().to_string(),
        ok,
        summary: summary.to_string(),
        file_path: file_path.map(|s| s.to_string()),
    };

    // 1) 前端全局卡片层（任何页面都接收）
    let _ = app.emit("flowio://job-done", payload);

    // 2) Windows 系统通知（最小化 / 后台也可见）。失败静默忽略。
    let body = if title.is_empty() {
        summary.to_string()
    } else {
        format!("《{}》{}", title, summary)
    };
    let _ = app
        .notification()
        .builder()
        .title("录步 Flowio")
        .body(body)
        .show();
}
