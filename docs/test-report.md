# 录步 Flowio — 端到端测试报告

> 版本: v0.2.1
> 日期: 2026-07-26
> 总计: 18 项 / 18 项通过

---

## 测试场景回归矩阵

| # | 场景 | 验证项 | 结果 |
|---|------|--------|------|
| 1 | 录制启动 | `Ctrl+Alt+R` 启动录制，状态切换为"录制中"，OSD 浮窗显示 | PASS |
| 2 | 录制停止 | `Ctrl+Alt+R` 停止录制，步骤列表加载，OSD 浮窗关闭 | PASS |
| 3 | 空状态引导 | 空闲状态下截图预览区显示 Video 图标 + 引导文案 | PASS |
| 4 | AI 生成操作指南 | 停止后点击「AI 生成操作指南」，流式内容正常输出 | PASS |
| 5 | AI 流式 JSON 截断容错 | 截断 JSON 自动修复，不导致崩溃 | PASS |
| 6 | AI 生成防重复点击 | useRef(false) 防重复锁生效 | PASS |
| 7 | AI 生成 Toast 提示 | AI 生成完成/失败时 Toast 提示正常显示 | PASS |
| 8 | 录制防重入锁 | 录制中再次触发录制被拒绝 | PASS |
| 9 | 进入编辑器 | 停止录制后点击「编辑」按钮跳转 Editor 页 | PASS |
| 10 | 编辑器步骤拖拽排序 | 停止录制后拖拽 GripVertical 手柄，步骤顺序实时调整 | PASS |
| 11 | 暗色模式切换 | 设置页开关暗色模式，所有页面配色跟随切换 | PASS |
| 12 | ESC 关闭设置面板 | 设置面板打开时按 Escape 键关闭 | PASS |
| 13 | 截图格式转换 | RGBA8 → JPEG 格式截图正常生成 | PASS |
| 14 | AI 模型三层架构 | 智谱(内置)/DeepSeek(自定义)/通义千问/OpenAI 正确分层 | PASS |
| 15 | 设置面板所有 Tab | 通用/ AI 模型/快捷键/关于 四个 Tab 正常切换 | PASS |
| 16 | 分享功能 | 停止后分享按钮可用，局域网 HTTP 分享正常 | PASS |
| 17 | 导出功能 | PDF/HTML/Markdown 三种格式导出正确 | PASS |
| 18 | 窗口尺寸 | 默认 1000×700，最小 800×600，溢出滚动正确 | PASS |

---

## P0 修复项（核心阻塞）

| 编号 | 问题 | 修复内容 | 文件 |
|------|------|----------|------|
| P0-01 | 截图格式转换 | screenshot.rs 中 RGBA8 到 JpegEncoder 格式转换修复 | src-tauri/src/recorder/screenshot.rs |
| P0-02 | AI JSON 截断 + Toast | Recording.tsx 引入 Toast 提示，step_parser.rs 新增 try_repair_truncated_json | src/pages/Recording.tsx, src-tauri/src/ai_pipeline/step_parser.rs |
| P0-03 | AI 模型三层架构 | 重写 AiModelTab 实现内置/自定义模型分层 | src/components/settings/AiModelTab.tsx |

## P1 修复项（次要问题）

| 编号 | 问题 | 修复内容 | 文件 |
|------|------|----------|------|
| P1-01 | 版本号与 GitHub 链接 | 版本号 v0.1.0→v0.2.0，修复 GitHub 链接 | src/components/settings/AboutTab.tsx, src-tauri/tauri.conf.json |
| P1-02 | 隐私政策/用户协议 | 创建 docs/privacy.md 和 docs/terms.md，AboutTab 按钮改为 window.open | src/components/settings/AboutTab.tsx, docs/ |
| P1-04 | AI 生成防重复点击 | Recording.tsx 添加 useRef(false) 防重复锁 | src/pages/Recording.tsx |
| P1-05 | 录制防重入锁 | lib.rs 的 AppState 添加 AtomicBool，start_recording compare_exchange 防护 | src-tauri/src/lib.rs |
| P1-06 | 窗口尺寸 | 默认 1200×800→1000×700，Recording.tsx 添加 overflow-hidden+overflow-auto | src/pages/Recording.tsx, src-tauri/tauri.conf.json |

## P2 修复项（体验优化）

| 编号 | 问题 | 修复内容 | 文件 |
|------|------|----------|------|
| P2-01 | 空状态引导不足 | 空闲状态下截图预览区显示 Video 图标 + "点击 [开始录制]，录步会自动捕获屏幕操作并生成中文步骤说明" | src/pages/Recording.tsx |
| P2-02 | ESC 关闭设置面板 | SettingsPanel 添加 useEffect 监听 Escape 键 | src/components/settings/SettingsPanel.tsx |
| P2-03 | 暗色模式 | GeneralTab 添加开关(localStorage)，主要组件(Recording/Editor/StepList/ControlBar/SettingsPanel及子Tab)按 design-system 第2章添加 dark: 前缀 | src/hooks/useDarkMode.ts, src/components/settings/GeneralTab.tsx, src/pages/Recording.tsx, src/pages/Editor.tsx, src/components/StepList.tsx, src/components/ControlBar.tsx, src/components/settings/*.tsx |
| P2-06 | 录制时无视觉反馈 | 新增 OSD 浮窗(src-tauri/osd.html)，lib.rs 录制启动时创建 always_on_top+decorations:false+transparent+skip_taskbar 窗口，停止时关闭 | src-tauri/osd.html, src-tauri/src/lib.rs |
| P2-07 | 录制页步骤无拖拽排序 | Recording.tsx 引入 @hello-pangea/dnd，停止录制后步骤列表支持拖拽排序(调用 reorder_steps 命令) | src/pages/Recording.tsx |

---

## 编译结果

| 检查项 | 结果 |
|--------|------|
| TypeScript 编译 | 0 错误 |
| Vite 生产构建 | 通过 (dist/) |
| Rust cargo check | 0 错误 |

---

*报告完毕。*
