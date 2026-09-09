// 自动化能力构建开关（与 src-tauri Cargo feature "automation" 配套）。
//
// - 自动化版（develop 线，默认）：正常构建，AUTOMATION_ENABLED = true
// - 非自动化版（stable 线正式包）：构建前设置 VITE_AUTOMATION=0，
//   自动化 UI（执行 / 演练 / AI 兜底 / 执行历史）整体隐藏；
//   即使误调用后端桩命令，也会收到「当前版本不包含自动化功能」的明确错误。
export const AUTOMATION_ENABLED = import.meta.env.VITE_AUTOMATION !== "0";
