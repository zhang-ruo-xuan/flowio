//! Stage 2a：自研精准执行器（坐标确定性重放）。
//!
//! 读录制源 DB 的坐标锚点（x/y/window_title/description/after_screenshot_path），
//! 用 Win32 确定性重放，每步后截图校验。相较 Stage 1 的通用 SKILL.md 导出
//! （无坐标、靠视觉语义定位、不精准），这里是壁垒的第一层。

pub mod ai_fallback;
pub mod backend;
pub mod commands;
pub mod replay;
pub mod verify;

pub use backend::{DryRunBackend, ReplayBackend, WindowsBackend};
pub use commands::{ExecReport, StepOutcome};
pub use replay::{load_replay_plan, run_action, run_replay, ReplayAction};
pub use verify::{diff_ratio, verify_step};
