---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_2a0591e2882c11f1a68c525400826444
    ReservedCode1: 427sGRa12VbZei1V3Ou2DfhGtMxII5rnHU/u4q5/fcdXLyhAMGmBmlvffikbs0He2gUAOAoPDEV9PcjSg2a9mOpHE5AIaXLdeaonw7fFJ9Qz3KpI28ZyIsCma/XUhfQ9nEQQBTJvzDC1eMBkQQMMP6euXJ84bD4QCK7aSZAZjnuMy254l41QfF1k4j4=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_2a0591e2882c11f1a68c525400826444
    ReservedCode2: 427sGRa12VbZei1V3Ou2DfhGtMxII5rnHU/u4q5/fcdXLyhAMGmBmlvffikbs0He2gUAOAoPDEV9PcjSg2a9mOpHE5AIaXLdeaonw7fFJ9Qz3KpI28ZyIsCma/XUhfQ9nEQQBTJvzDC1eMBkQQMMP6euXJ84bD4QCK7aSZAZjnuMy254l41QfF1k4j4=
---

# ADR-003：Tauri 2 + React + TypeScript 技术栈

> 状态：已采纳
> 日期：2026-07-25
> 决策者：录步 (Flowio) 项目

## 背景

录步需要选择桌面应用开发框架。两个主要候选方案：

| 维度 | Electron | Tauri 2 |
|------|----------|---------|
| 安装包大小 | ~200MB | ~10MB |
| 内存占用 | ~300MB | ~80MB |
| 后端语言 | Node.js | Rust |
| 前端 | 任意 | 任意（WebView） |
| 跨平台 | ✅ Win/Mac/Linux | ✅ Win/Mac/Linux |
| 成熟度 | 极高（VS Code/Discord 等） | 较新（2.x 2024 年稳定） |
| 生态 | npm 海量 | Cargo + 社区插件 |

Scribe 使用 Electron，安装包约 200MB。录步需要在安装包体积上形成差异化优势。

## 决策

**选择 Tauri 2 作为桌面框架。** 完整技术栈为：

| 层 | 技术 | 版本 |
|----|------|------|
| 桌面框架 | Tauri 2 | 2.x |
| 前端框架 | React | 18.3 |
| 类型系统 | TypeScript (strict) | 5.7 |
| CSS | Tailwind CSS | 4.x |
| 构建工具 | Vite | 6.x |
| 后端语言 | Rust | 1.82+ |

## 理由

1. **体积优势**：Tauri 2 安装包 ~10MB vs Electron ~200MB，体积差距 20 倍，是录步相对于 Scribe 的显著差异化
2. **性能优势**：Rust 后端性能远超 Node.js，录制引擎（高频率事件监听 + 截图）对性能敏感
3. **Rust 安全性**：内存安全 + 线程安全，录制引擎常驻后台，稳定性要求高
4. **React + AI 友好**：React 是当前 AI 编码工具最熟悉的前端框架，AI 辅助开发效率最高
5. **TypeScript strict mode**：类型安全保证代码质量，符合 AGENTS.md 硬性要求
6. **Tailwind CSS**：原子化 CSS 避免样式冲突，不写自定义 CSS 提高可维护性

## 后果

- **正面**：安装包轻量（≤15MB），性能优于 Scribe；Rust 安全保证录制引擎稳定
- **负面**：
  - Rust 学习成本高于 JS/TS，新成员上手慢
  - Tauri 2 依赖 GTK/WebKit（Linux），编译耗时长（完整 build 需 5-10 分钟）
  - 2 核 1.6GB 服务器 OOM 风险高——`cargo build` 在编译 webkit2gtk 时可能超内存，对策：使用 `cargo check` 验证编译，`cargo build` 在本地执行
- **缓解**：核心团队提前学习 Rust；CI/CD 用 GitHub Actions 分担编译压力
*（内容由AI生成，仅供参考）*
