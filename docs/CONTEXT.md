# 录步 Flowio — 项目上下文

## 基本信息

| 项目 | 详情 |
|------|------|
| 名称 | 录步 Flowio |
| 描述 | Windows 桌面操作录制工具 — 自动捕获鼠标/键盘操作生成操作指南，AI 智能标注步骤 |
| 技术栈 | Tauri 2 + React 18 + TypeScript strict + Tailwind v4 |
| AI 模型 | 智谱 GLM-4-flash（内置默认，支持扩展） |
| 当前阶段 | 阶段 7：商业化准备 |
| 当前任务 | 定价页面 + 支付集成规划 |
| 最后更新 | 2026-07-26 19:30 |

## 里程碑

| 里程碑 | 状态 |
|--------|------|
| Week 1: 录制 + AI (MVP 核心) | ✅ 完成 |
| Week 2: 编辑 + 导出 + 设置 + 测试 | ✅ 完成 |
| Week 2.5: P0 修复（API Key 持久化 + 合规） | ✅ 完成 |
| Week 3: 打包 + 发布 | ✅ 完成 |

## v0.2.0 发布 (2026-07-26)

- 版本号统一至 0.2.0（package.json / Cargo.toml / tauri.conf.json）
- CHANGELOG.md 补全 P0 修复项、已知问题、编译信息
- Release Build: Windows 11 本机构建成功
  - app.exe: 11.29 MB
  - NSIS 安装包: 录步_0.2.0_x64-setup.exe (2.68 MB)
  - 安装包位于 `releases/v0.2.0/`
- MSI 包因 WiX 中文编码兼容性问题暂未生成

## P0 修复记录 (2026-07-26)

- **API Key 持久化**: 从 HashMap 内存存储迁移到 Windows Credential Manager（tauri-plugin-keyring v0.1.0）
- **启动加载**: setup hook 自动从 Credential Manager 读取 Key
- **合规补全**: AboutTab 添加隐私政策/用户协议入口 + 开源许可证声明
- **编译验证**: cargo check 0 error / pnpm build 0 error

## 双机分工

| 机器 | IP | 角色 |
|------|-----|------|
| 阿里云 | 47.102.154.233 | 源码仓库、前端构建、Rust 编译 |
| 腾讯云 | 49.235.161.164 | 编译备用机（当前不可用） |

