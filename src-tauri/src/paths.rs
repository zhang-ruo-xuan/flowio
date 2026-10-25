//! 统一路径工具：应用数据目录 / 截图持久目录。
//!
//! 🔴 历史坑（2026-10-04）：截图曾保存到 `%TEMP%\flowio_screenshots\`，
//! 系统清理临时目录后全部截图丢失（DB 中 188 条路径失效）。从本轮起
//! **所有截图/媒体落盘统一走 `%APPDATA%\com.flowio.app\screenshots\`**，
//! 严禁再向系统临时目录写入用户可见产物。目录常量集中在 `screenshots_dir()`，
//! 各模块禁止散落硬编码路径。

use std::path::PathBuf;
use std::sync::OnceLock;

/// 应用数据目录（`%APPDATA%\com.flowio.app`），由 lib.rs setup 注入。
/// 未注入时（仅测试场景）回落到系统临时目录，保证 cargo test 不崩。
static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 注入应用数据目录（lib.rs setup 阶段调用一次；重复调用幂等）。
pub fn set_app_data_dir(path: PathBuf) {
    let _ = APP_DATA_DIR.set(path);
}

/// 应用数据目录；未注入时回落到 `temp_dir()`（仅测试兜底）。
pub fn app_data_dir() -> PathBuf {
    APP_DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir())
}

/// 截图/媒体持久目录 `{app_data_dir}/screenshots`，自动创建。
/// **所有截图、媒体文件必须写到这里**，禁止使用系统临时目录。
pub fn screenshots_dir() -> PathBuf {
    let dir = app_data_dir().join("screenshots");
    let _ = std::fs::create_dir_all(&dir);
    dir
}
