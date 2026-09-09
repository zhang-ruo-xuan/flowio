//! 非自动化构建的 executor 桩模块（`cargo build --no-default-features` 时编译）。
//!
//! 目的：让「非自动化版」（stable 线正式包）不携带任何重放/审计/AI 兜底代码，
//! 同时保持命令名与模块路径和自动化版一致——lib.rs 的命令注册表与
//! 其他模块的日志调用点（如 uia.rs → backend::replay_log）无需任何条件分支。
//! 自动化版（default features）完全不走本文件，真实实现在 `executor/` 目录。

/// no-op 日志：录制侧 uia.rs 的锚点定位日志调用点在非自动化构建下静默。
pub mod backend {
    pub fn replay_log(_msg: &str) {}
}

pub mod commands {
    use serde_json::Value;
    use tauri::AppHandle;

    const NOT_AVAILABLE: &str = "当前版本不包含自动化功能（非自动化构建）";

    #[tauri::command]
    pub fn execute_recording(
        _app: AppHandle,
        _db: tauri::State<'_, crate::DbState>,
        _recording_id: String,
        _dry_run: bool,
        _step_delay_ms: Option<u64>,
        _approval_mode: Option<String>,
        _approved: Option<bool>,
        _ai_fallback: Option<bool>,
    ) -> Result<Value, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    #[tauri::command]
    pub fn preview_replay(
        _db: tauri::State<'_, crate::DbState>,
        _recording_id: String,
    ) -> Result<Value, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    #[tauri::command]
    pub fn get_replay_audit(
        _db: tauri::State<'_, crate::DbState>,
        _recording_id: Option<String>,
    ) -> Result<Value, String> {
        Err(NOT_AVAILABLE.to_string())
    }
}
