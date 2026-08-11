---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_b18ab6828d7011f1b82d525400287e28
    ReservedCode1: cKialpfJySS+KqTyoUG9onHO8roe4p2VutYmYGNuSC42g7iG1iJNYGEOaT7XCk5C782HRoAyYPpOAh/LmcCo6cY69RK5Jx/J5LHpie6cZG4r0h/KkjjTBBjYmLXlqO/+QWjXPikOLNGix7OnszY5NRg9ZebOnC/8380J1l9Jgbv2Mi5RSzRQmd7WYI0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_b18ab6828d7011f1b82d525400287e28
    ReservedCode2: cKialpfJySS+KqTyoUG9onHO8roe4p2VutYmYGNuSC42g7iG1iJNYGEOaT7XCk5C782HRoAyYPpOAh/LmcCo6cY69RK5Jx/J5LHpie6cZG4r0h/KkjjTBBjYmLXlqO/+QWjXPikOLNGix7OnszY5NRg9ZebOnC/8380J1l9Jgbv2Mi5RSzRQmd7WYI0=
---

# 录步 Flowio Chrome 扩展开发路线图

> **文档版本**：v1.0  
> **最后更新**：2026-08-01  
> **前置依赖**：[spec.md](./spec.md) | [plan.md](./plan.md) | [AGENTS.md](../AGENTS.md)

---

## 第 1 章：战略定位与对标分析

### 1.1 为什么需要 Chrome 扩展

桌面端（Tauri 2）已覆盖 Windows 全应用录制场景，但存在以下盲区：

| 盲区 | 影响 | 扩展解决方案 |
|------|------|-------------|
| 纯 Web 工作流用户（ChromeOS / 企业 Web 应用为主） | 下载安装 2.69MB 桌面应用门槛过高 | 一键安装即用 |
| 浏览器内流程文档天然需要网页端上下文 | 桌面端截图含浏览器工具栏/桌面背景，信息冗余 | 仅捕获页面可视区域 |
| Chrome Web Store 分发渠道 | 桌面端仅 GitHub Releases / 官网分发，获客渠道单一 | 利用 Web Store 自然流量 |
| Scribe 市场教育已完成，用户习惯"一键装扩展就开始" | 竞品对标需双端覆盖 | 降低迁移门槛 |

### 1.2 Scribe Chrome 扩展对标拆解

Scribe 是目前流程文档赛道的事实标准。以下是其 Chrome 扩展核心产品形态的逆向工程分析：

#### 产品形态

| 维度 | Scribe 实现 | 录步对标策略 |
|------|-----------|------------|
| **安装方式** | Chrome 网上应用店一键安装 | Chrome Web Store 发布（免费） |
| **入口位置** | 浏览器工具栏图标 + 侧边栏弹出 | 工具栏图标 Popup → 侧边栏面板 |
| **录制触发** | 点击图标 → Start Capture → 在页面内操作 | 同流程，增加快捷键 `Ctrl+Shift+R` 启动/停止 |
| **事件监控** | 浏览器原生事件监听（click / input / URL change） | 同方案，增加 MutationObserver 监听 DOM 变化 |
| **截图机制** | 每次点击/输入触发区域截图（裁剪版）+ 全页截图 | `chrome.tabs.captureVisibleTab` 整页截图 + 元素位置裁剪 |
| **AI 生成** | 云端 AI 生成步骤描述 | 复用桌面端 AI Pipeline（智谱 GLM-4-flash），Rust→JS 端口适配 |
| **编辑面板** | 新标签页打开全屏编辑器 | 扩展内置侧边栏 + 新标签页两种模式 |
| **导出** | PDF / HTML / Markdown / 链接分享 | 复用桌面端导出引擎（JS 版本适配） |
| **敏感数据** | 自动模糊（Pro 版） | Phase 2 实现，复用桌面端逻辑 |
| **数据存储** | 云端 | 优先 IndexedDB 本地存储，Phase 3 增加云端同步 |
| **定价** | 基础免费，Pro $23/月 | 基础免费，高级功能按桌面端订阅体系 |

#### Scribe 技术栈推断

| 层面 | Scribe 推断方案 | 录步方案 |
|------|---------------|---------|
| **扩展框架** | Manifest V3（推断） | Manifest V3（确定性选型） |
| **前端框架** | React（推断，基于交互复杂度） | React 18 + TypeScript 5（复用） |
| **样式** | 自研组件库（推断） | Tailwind CSS 4（复用桌面端设计系统） |
| **事件录制** | 浏览器 Event Listener + MutationObserver | Content Script 注入 + 全局事件代理 |
| **截图** | `captureVisibleTab` + Canvas 裁剪 | 同方案，增加 `html2canvas` 备选 |
| **数据通路** | Content Script → 云端 API | Content Script → Background SW → 本地/云端 |
| **AI 模型** | 未公开 | 智谱 GLM-4-flash（与桌面端一致） |

### 1.3 扩展 vs 桌面端职责边界

| 能力 | 桌面端（Tauri） | 扩展（Chrome） | 共享层 |
|------|---------------|---------------|--------|
| 全局系统录制（任意应用） | ✅ rdev + xcap + UIA | ❌ 浏览器沙箱限制 | — |
| 浏览器内录制 | ✅（作为桌面应用的一部分） | ✅ 主力场景 | — |
| 截图 | ✅ xcap 全屏 | ✅ captureVisibleTab | — |
| AI 步骤生成 | ✅ Rust 端 ai_pipeline | ✅ 适配 JS 版 ai_pipeline | AI Prompt 模板 / 模型配置 |
| 步骤编辑（拖拽排序/增删改） | ✅ React 组件 | ✅ React 组件（直接复用） | StepEditor / StepList / StepCard |
| 导出（PDF/HTML/MD） | ✅ Rust printpdf + 渲染 | ✅ JS 版导出引擎 | 导出模板 / 样式 |
| 本地存储 | ✅ SQLite | ✅ IndexedDB | 数据 Schema |
| 设计系统 | ✅ Tailwind 4 | ✅ 直接复用 | 全量复用 |
| 设置管理 | ✅ keyring + plugin-store | ✅ chrome.storage.sync | 设置 Schema |

---

## 第 2 章：技术选型

### 2.1 整体架构

```
┌─────────────────────────────────────────────────┐
│                  Chrome 扩展                      │
│                                                   │
│  ┌──────────────┐  ┌──────────────────────────┐  │
│  │   Popup UI   │  │     Side Panel / Tab     │  │
│  │ (React 18)   │  │       (React 18)          │  │
│  └──────┬───────┘  └───────────┬──────────────┘  │
│         │ chrome.runtime       │                  │
│         ▼           .sendMessage                  │
│  ┌─────────────────────────────────────────────┐ │
│  │        Background Service Worker             │ │
│  │  ┌──────────┐ ┌──────────┐ ┌─────────────┐  │ │
│  │  │ 录制状态  │ │ AI Client│ │ 导出引擎    │  │ │
│  │  │ 管理      │ │ (fetch)  │ │ (JS)        │  │ │
│  │  └──────────┘ └──────────┘ └─────────────┘  │ │
│  │  ┌──────────┐ ┌──────────┐ ┌─────────────┐  │ │
│  │  │ IndexedDB│ │ 消息路由 │ │ 截图管理    │  │ │
│  │  │ 持久化    │ │          │ │             │  │ │
│  │  └──────────┘ └──────────┘ └─────────────┘  │ │
│  └─────────────────────────────────────────────┘ │
│         │ chrome.scripting / chrome.tabs          │
│         ▼                                         │
│  ┌─────────────────────────────────────────────┐ │
│  │          Content Script (注入目标页)          │ │
│  │  ┌──────────┐ ┌──────────┐ ┌─────────────┐  │ │
│  │  │事件监听器│ │DOM 分析器│ │ 截图裁剪器  │  │ │
│  │  │(click/   │ │(元素定位 │ │ (Canvas)    │  │ │
│  │  │ input/   │ │ / 文本提 │ │             │  │ │
│  │  │ scroll/  │ │ 取)      │ │             │  │ │
│  │  │ nav)     │ │          │ │             │  │ │
│  │  └──────────┘ └──────────┘ └─────────────┘  │ │
│  └─────────────────────────────────────────────┘ │
│                                                   │
└─────────────────────────────────────────────────┘
```

### 2.2 确定技术选型

| 层级 | 技术 | 版本 | 决策理由 |
|------|------|------|----------|
| **扩展标准** | Manifest V3 | — | Chrome 强制要求，Manifest V2 已停止接受新提交 |
| **前端框架** | React | 18.3 | 桌面端已验证，组件可跨端复用 |
| **类型系统** | TypeScript | 5.7 strict | AGENTS.md 宪法级规则 |
| **构建工具** | Vite | 6.x | 与桌面端统一，HMR 极快，支持扩展构建插件 `@crxjs/vite-plugin` |
| **样式** | Tailwind CSS | 4.x | 桌面端设计系统直接复用 |
| **图标库** | lucide-react | 最新 | 与桌面端统一 |
| **Markdown 渲染** | marked | 最新 | 与桌面端统一 |
| **拖拽排序** | @hello-pangea/dnd | 最新 | 与桌面端统一 |
| **扩展构建插件** | @crxjs/vite-plugin | 2.x | Vite 原生支持 Manifest V3，HMR 开发体验 |
| **本地存储** | IndexedDB (via idb) | 最新 | 扩展环境无 SQLite，idb 包装 Promise API |
| **截图** | chrome.tabs.captureVisibleTab | Chrome API | 内置能力，无需额外依赖 |
| **AI 调用** | fetch API（OpenAI 兼容格式） | — | 扩展内从 JS 端发起，API Key 存 chrome.storage.session |
| **数据同步** | chrome.storage.sync | — | Phase 3 双端同步用 |

### 2.3 已否决方案

| 方案 | 否决原因 |
|------|----------|
| Manifest V2 | Chrome 已停止接受新扩展提交 |
| 用 JS 重写 Rust 录制引擎 | 扩展沙箱内 `rdev` 不可用，架构完全不同，重写无意义 |
| 使用 Puppeteer 截图 | 扩展内无法运行 Node 进程 |
| 引入 Redux 状态管理 | 扩展数据流较简单，useState + useContext 足够 |
| IndexedDB 裸操作 | 开发体验差，idb 库 stars 高且维护活跃 |
| 手写 Webpack 扩展构建 | Vite + @crxjs 插件生态已成熟 |

---

## 第 3 章：可复用 vs 需新建

### 3.1 从桌面端直接复用的资产

#### 3.1.1 React 组件（源码级复用）

| 组件 | 桌面端路径 | 复用率 | 适配说明 |
|------|----------|--------|----------|
| `StepEditor` | `src/components/StepEditor` | ~90% | 去掉 Tauri invoke，改用 chrome.runtime.sendMessage |
| `StepList` | `src/components/StepList` | ~95% | 无改动，纯 UI 组件 |
| `StepCard` | `src/components/StepCard` | ~90% | 截图来源从本地文件路径改为 base64 Data URL |
| `ExportPanel` | `src/components/ExportPanel` | ~85% | 导出逻辑从 Tauri Command 改为 JS 版导出引擎 |
| `SettingsPanel` | `src/components/SettingsPanel` | ~80% | 存储从 plugin-store 改为 chrome.storage.sync |
| `RecordingPanel` | `src/components/RecordingPanel` | ~70% | 录制触发从 rdev 全局快捷键改为浏览器事件 |

#### 3.1.2 设计系统（全量复用）

| 资产 | 桌面端 | 扩展端 | 复用率 |
|------|--------|--------|--------|
| Tailwind 配置 | `tailwind.config.ts` | 直接复制 | 100% |
| 主题变量（暗色/亮色） | CSS 变量体系 | 直接复制 | 100% |
| 组件样式 | 所有组件的 Tailwind class | 直接复用 | 100% |
| lucide-react 图标 | 已安装 | 直接复用 | 100% |

#### 3.1.3 AI Pipeline 核心资产（逻辑复用）

| 资产 | 形式 | 复用方式 |
|------|------|----------|
| Prompt 模板（system prompt / user prompt） | Rust 常量 → 提取为独立 JSON/TS 文件 | 桌面端也改为引用此共享模板文件 |
| 步骤数据 Schema（Step / Recording） | Rust struct → TypeScript interface | `shared/types/` 目录维护统一 Schema |
| AI 模型配置（GLM-4 / DeepSeek / OpenAI endpoint 映射） | Rust enum → TypeScript enum | 保持完全一致 |
| SSE 流式解析逻辑 | Rust `ai_pipeline.rs` → TypeScript 重写 | 逻辑平移，约 60% 可参考 |

#### 3.1.4 导出样式与模板

| 资产 | 桌面端 | 扩展端 |
|------|--------|--------|
| PDF 排版样式 | Rust printpdf 逻辑 | JS 版（html2pdf / jsPDF）平移 |
| HTML 导出模板 | Rust 模板字符串 | JS 模板字符串（逻辑直译） |
| Markdown 导出模板 | Rust 格式化逻辑 | JS 格式化逻辑（逻辑直译） |

### 3.2 必须从零新建的模块

#### 3.2.1 扩展框架基础设施

| 模块 | 说明 | 工作量估计 |
|------|------|-----------|
| `manifest.json` | Manifest V3 配置：权限声明、Service Worker 注册、Content Script 注入规则 | 0.5 天 |
| `src/background/` | Background Service Worker：消息路由、录制状态机、IndexedDB 管理、AI 客户端、截图管理 | 3 天 |
| `src/content/` | Content Script：事件监听器（click/input/scroll/navigation）、DOM 分析器（元素定位/文本提取）、截图裁剪器 | 4 天 |
| `src/popup/` | Popup UI：录制控制按钮组、最近录制列表、快速操作入口 | 1 天 |
| `src/sidepanel/` | Side Panel UI：步骤编辑器（复用 StepEditor 组件）、实时预览 | 1.5 天 |
| `vite.config.ts` | Vite + @crxjs/vite-plugin 扩展构建配置 | 0.5 天 |

#### 3.2.2 浏览器端录制引擎

这是与桌面端差异最大的模块。桌面端依赖 `rdev`（全局输入监听）+ `xcap`（全屏截图）+ `UIA`（元素识别），扩展端完全不可用。

| 子模块 | 技术方案 | 对标桌面端 | 关键差异 |
|--------|----------|----------|----------|
| **事件监听器** | `document.addEventListener` 在 Content Script 中全局捕获 click / input / change / scroll / popstate / hashchange | `rdev` 全局 Hook | 仅限当前标签页 DOM 事件，无法跨标签 |
| **URL 变更检测** | `MutationObserver` 监听 `<title>` + `history.pushState/replaceState` 拦截 | N/A（桌面端截图对比） | 浏览器原生能力更精确 |
| **元素定位** | `element.getBoundingClientRect()` + CSS selector 路径 | UIA（Windows UI Automation） | 仅限 DOM 元素，定位精度更高 |
| **文本提取** | `element.textContent / aria-label / title / placeholder` 多策略提取 | UIA Name/Value 属性 | DOM 信息更丰富 |
| **截图** | `chrome.tabs.captureVisibleTab` 整页截图 + Canvas 按元素 rect 裁剪 | `xcap` 全屏截图 | 仅捕获可视区域，无法捕获滚动外内容 |
| **高亮标注** | Canvas 叠加层：在截图裁剪区绘制橙色边框 + 点击标记圆圈 | 桌面端 `overlay` 模块（Rust 原生窗口叠加） | 改为图片后处理方式 |
| **密码字段保护** | 检测 `input[type="password"]`，截图时该区域覆盖黑色色块 | UIA `IsPassword` 属性 | DOM API 更直接可靠 |

#### 3.2.3 截图机制设计

```
用户点击页面元素
    │
    ▼
Content Script 记录事件 + 元素坐标 (x, y, w, h)
    │
    ▼
发送消息到 Background SW
    │
    ▼
Background SW 调用 chrome.tabs.captureVisibleTab
    │
    ▼
得到 tab 可视区域整页截图（Data URL）
    │
    ▼
传递回 Content Script（或 Background SW 用 OffscreenCanvas）
    │
    ▼
Canvas 按元素 rect 裁剪 + 绘制高亮标注
    │
    ▼
压缩为 WebP（质量 0.8）→ 存入 IndexedDB
```

**截图时机策略**：Scribe 在每次点击/输入时截图。录步采用同样策略，但增加去重机制——若同一元素 500ms 内连续事件，只截一次。

#### 3.2.4 JS 版导出引擎

桌面端导出由 Rust `export.rs` 驱动（PDF 用 `printpdf` crate），扩展端需完全重写：

| 格式 | 桌面端实现 | 扩展端方案 | 工作量 |
|------|----------|----------|--------|
| **PDF** | Rust `printpdf` | `jsPDF` 或 `html2pdf.js`（推荐后者，支持中文字体嵌入） | 2 天 |
| **HTML** | Rust 字符串模板 | JS 模板字符串，逻辑直译 | 0.5 天 |
| **Markdown** | Rust 字符串格式化 | JS 字符串格式化，逻辑直译 | 0.5 天 |

---

## 第 4 章：分阶段里程碑

### 4.1 总览

```
Phase 1 (MVP)              Phase 2 (编辑+导出)         Phase 3 (双端互通+发布)
  4 周                        3 周                        3 周
 ┌──────────────┐         ┌──────────────┐          ┌──────────────┐
 │ 录制 + AI 生成│   →     │ 编辑 + 导出   │    →    │ 双端互通 +    │
 │              │         │              │          │ Web Store 发布│
 │ 扩展 MVP 对标│         │ 功能完整对标 │          │ 生态闭环      │
 │ Scribe Basic │         │ Scribe Pro   │          │              │
 └──────────────┘         └──────────────┘          └──────────────┘
```

---

### 4.2 Phase 1：扩展 MVP（录制 + AI 生成）

**目标**：用户在浏览器内完成录制并得到 AI 生成的步骤文档。对标 Scribe Basic 免费版功能。

**时间**：4 周

#### 4.2.1 技术任务

| 编号 | 任务 | 负责模块 | 预计工时 | 依赖 |
|------|------|----------|----------|------|
| P1-01 | 初始化扩展项目：Vite + @crxjs + React 18 + TS 5 + Tailwind 4 | 脚手架 | 1 天 | — |
| P1-02 | 编写 `manifest.json`：声明权限（activeTab / scripting / storage / tabs / sidePanel） | 扩展框架 | 0.5 天 | P1-01 |
| P1-03 | 实现 Background Service Worker：消息路由 + 录制状态机 + IndexedDB 初始化 | 扩展框架 | 2 天 | P1-02 |
| P1-04 | 实现 Content Script 事件监听器：click / input / change / scroll / popstate | 录制引擎 | 2 天 | P1-02 |
| P1-05 | 实现 Content Script DOM 分析器：元素定位（CSS selector + rect）+ 文本提取 | 录制引擎 | 1.5 天 | P1-04 |
| P1-06 | 实现截图管线：captureVisibleTab → Canvas 裁剪 → WebP 压缩 → IndexedDB 存储 | 录制引擎 | 2 天 | P1-04, P1-05 |
| P1-07 | 实现密码字段保护：检测 `input[type="password"]` 并在截图上覆盖黑色遮罩 | 录制引擎 | 0.5 天 | P1-06 |
| P1-08 | 实现 Popup UI：录制开始/停止按钮 + 步骤数实时计数 + 最近录制列表 | 前端 UI | 1.5 天 | P1-03 |
| P1-09 | 实现 JS 版 AI 客户端：OpenAI 兼容格式 fetch 调用 + SSE 流式解析 | AI 管线 | 2 天 | P1-03 |
| P1-10 | 提取共享 Prompt 模板为独立文件：`shared/prompts/` 目录 | AI 管线 | 0.5 天 | — |
| P1-11 | 实现步骤预览界面（新标签页）：AI 生成结果展示 + 基础步骤列表只读视图 | 前端 UI | 1.5 天 | P1-09 |
| P1-12 | 实现基础设置界面：API Key 配置（chrome.storage.session 加密存储）+ 模型选择 | 前端 UI | 1 天 | P1-03 |
| P1-13 | 端到端集成测试：录制 10 步 → AI 生成 → 预览 | 测试 | 1.5 天 | 全部 |
| P1-14 | 编译验证 + 扩展打包（生产构建） | 构建 | 0.5 天 | P1-13 |

#### 4.2.2 Phase 1 验收标准

| 编号 | 验收条件 | 验证方式 |
|------|----------|----------|
| E1-01 | `pnpm build` 编译通过，0 error | CI / 手动验证 |
| E1-02 | 扩展可在 Chrome 131+ 上正常加载（开发者模式） | 手动加载测试 |
| E1-03 | 录制捕获准确率 > 90%（click / input / URL 变更全部捕获） | 录制 20 步标准流程，人工核对 |
| E1-04 | 截图清晰度可辨认被点击元素，高亮标注圈颜色为品牌橙色 `#FF6B35` | 视觉审查 |
| E1-05 | `input[type="password"]` 字段截图 100% 被黑色遮罩覆盖 | 专项测试 |
| E1-06 | AI 生成 10 步完成时间 ≤ 8 秒 | 秒表计时 |
| E1-07 | AI 生成步骤文本为简体中文 | 逐条审查 |
| E1-08 | API Key 存储于 chrome.storage.session，页面刷新后不丢失 | 刷新测试 |
| E1-09 | 扩展安装后大小 ≤ 500KB（不含 AI 模型） | 文件属性 |
| E1-10 | 录制期间页面正常交互，无明显卡顿 | 主观体验 |

---

### 4.3 Phase 2：编辑 + 导出

**目标**：用户可编辑 AI 生成的步骤，并导出为 PDF / HTML / Markdown。对标 Scribe Pro 编辑与导出能力。

**时间**：3 周

#### 4.3.1 技术任务

| 编号 | 任务 | 负责模块 | 预计工时 | 依赖 |
|------|------|----------|----------|------|
| P2-01 | 复用桌面端 StepEditor / StepList / StepCard 组件到扩展 | 组件复用 | 1.5 天 | Phase 1 |
| P2-02 | 适配拖拽排序（@hello-pangea/dnd）到扩展环境 | 组件复用 | 0.5 天 | P2-01 |
| P2-03 | 实现步骤增删改：添加步骤 / 删除步骤 / 编辑标题与描述 / 替换截图 | 前端 UI | 1.5 天 | P2-01 |
| P2-04 | 实现 JS 版 HTML 导出引擎 | 导出引擎 | 0.5 天 | Phase 1 |
| P2-05 | 实现 JS 版 Markdown 导出引擎 | 导出引擎 | 0.5 天 | Phase 1 |
| P2-06 | 实现 JS 版 PDF 导出引擎（html2pdf.js，含中文字体嵌入） | 导出引擎 | 2 天 | Phase 1 |
| P2-07 | 实现导出面板 UI（ExportPanel 复用） | 前端 UI | 1 天 | P2-04/05/06 |
| P2-08 | 实现截图敏感信息模糊（Smart Blur）：姓名 / 邮箱 / 手机号 / 身份证号自动检测 + Canvas 高斯模糊 | 截图处理 | 2 天 | Phase 1 |
| P2-09 | 实现快捷键支持：`Ctrl+Shift+R` 启动/停止录制，`Ctrl+Shift+E` 导出 | 扩展框架 | 0.5 天 | Phase 1 |
| P2-10 | 端到端集成测试：录制 → 编辑 5 步（拖拽/文本修改/删步）→ 3 格式导出 | 测试 | 1.5 天 | 全部 |

#### 4.3.2 Phase 2 验收标准

| 编号 | 验收条件 | 验证方式 |
|------|----------|----------|
| E2-01 | `pnpm build` 编译通过，0 error | CI / 手动验证 |
| E2-02 | 拖拽排序支持 ≥ 30 步，无卡顿、无错位 | 手动测试 |
| E2-03 | PDF 导出中文无乱码，图片不模糊 | 打开 PDF 审查 |
| E2-04 | HTML 导出在 Chrome/Edge 中渲染正常 | 打开 HTML 审查 |
| E2-05 | Markdown 导出在 VS Code / Typora 中渲染正常 | 打开 MD 审查 |
| E2-06 | 敏感信息正则检测准确率 > 95%（姓名/邮箱/手机号/身份证号） | 专项测试 |
| E2-07 | 模糊区域视觉确认不可辨认原始内容 | 视觉审查 |
| E2-08 | 快捷键 `Ctrl+Shift+R` 可在任意标签页启动/停止录制 | 手动测试 |
| E2-09 | 导出的 HTML/PDF 文件大小 ≤ 5MB（10 步含截图） | 文件属性 |
| E2-10 | 编辑 30 步文档 + 导出 3 个格式 ≤ 15 秒 | 秒表计时 |

---

### 4.4 Phase 3：双端互通 + Chrome Web Store 发布

**目标**：桌面端与扩展端数据互通，扩展发布到 Chrome Web Store。构建完整的录步产品矩阵。

**时间**：3 周

#### 4.4.1 技术任务

| 编号 | 任务 | 负责模块 | 预计工时 | 依赖 |
|------|------|----------|----------|------|
| P3-01 | 设计统一数据交换格式：JSON Schema（`shared/types/schema.json`） | 双端互通 | 1 天 | — |
| P3-02 | 桌面端导出兼容格式 + 导入扩展格式（Rust 端适配） | 双端互通 | 1.5 天 | P3-01 |
| P3-03 | 扩展端导入桌面端格式（IndexedDB 写入） | 双端互通 | 1 天 | P3-01 |
| P3-04 | 实现 chrome.storage.sync 跨设备同步（录制列表 + 设置） | 双端互通 | 1.5 天 | Phase 2 |
| P3-05 | 实现分享链接功能：生成临时分享 URL（预留后端接口） | 分享 | 1.5 天 | Phase 2 |
| P3-06 | 扩展 UI 适配暗色模式（复用桌面端 CSS 变量） | 前端 UI | 1 天 | Phase 2 |
| P3-07 | 编写中文用户文档 + 首次引导（3 步 Onboarding） | 文档 | 1 天 | Phase 2 |
| P3-08 | 生成 Chrome Web Store 截图（1280x800）+ 宣传文案 | 发布 | 1 天 | Phase 2 |
| P3-09 | 生成推广视频/GIF：展示录制 → AI 生成 → 导出 全流程 | 发布 | 1 天 | Phase 2 |
| P3-10 | Chrome Web Store 开发者账号注册 + 扩展提交审核 | 发布 | 1 天 | P3-08/09 |
| P3-11 | 端到端双端互通测试：桌面端录制 → 导入扩展编辑 → 导出 | 测试 | 1.5 天 | P3-02/03 |

#### 4.4.2 Phase 3 验收标准

| 编号 | 验收条件 | 验证方式 |
|------|----------|----------|
| E3-01 | 扩展通过 Chrome Web Store 审核并上架 | Web Store 可搜索到 |
| E3-02 | 桌面端导出的 .flowio 文件可被扩展导入且步骤/截图/描述完整 | 手动测试 |
| E3-03 | 扩展导出的 .flowio 文件可被桌面端导入且步骤/截图/描述完整 | 手动测试 |
| E3-04 | chrome.storage.sync 同步后，A 设备录制可出现在 B 设备列表中 | 手动测试 |
| E3-05 | 暗色模式下所有 UI 可读、对比度正常 | 视觉审查 |
| E3-06 | 首次安装显示 3 步引导：安装 → 录制 → 导出 | 清空存储后重新加载 |
| E3-07 | 扩展在 Chrome + Edge 上功能完全一致 | 双浏览器测试 |
| E3-08 | `pnpm build` 编译通过，0 error | CI / 手动验证 |
| E3-09 | 生产构建扩展包 ≤ 800KB | 文件属性 |
| E3-10 | Web Store 页面描述为中文，截图清晰展示产品价值 | 页面审查 |

---

## 第 5 章：不做的事情（范围边界）

| 不做的事情 | 原因 | 计划版本 |
|------------|------|----------|
| 扩展内视频录制/导出（MP4） | 与核心流程文档定位不同，且性能要求高 | 不做 |
| Firefox / Safari 扩展 | Chrome 优先验证市场，后续根据需求决定 | 待评估 |
| 扩展内离线 AI 推理（WebGPU / WebNN） | 模型体积大（> 100MB），扩展包大小受限 | 不做 |
| 扩展内语音输入生成步骤 | 成本高、准确度不稳定 | 不做 |
| 实时协作编辑（多人） | 需要后端 WebSocket 服务 | V2.0 |
| 网页内容 OCR（扫描图片中的文字） | 非核心路径，浏览器内文字可直接提取 DOM | 不做 |
| 广告拦截 / 隐私保护扩展功能合并 | 定位单一（流程文档），不扩展品类 | 不做 |

---

## 第 6 章：风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| Chrome Web Store 审核不通过 | 中 | 高 | 提前研读政策，权限最小化，避免申请不必要的 `host_permissions` |
| `captureVisibleTab` 截图时序问题（截图与实际事件不同步） | 高 | 中 | 增加事件截图前 100ms 延迟 + 事件序号校验 |
| IndexedDB 在 Service Worker 中性能瓶颈（大量截图 base64 存储） | 中 | 中 | 截图 WebP 压缩至 80% 质量，单张控制在 100KB 以内 |
| Manifest V3 Service Worker 生命周期终止导致录制中断 | 中 | 高 | 使用 `chrome.sidePanel` 保持扩展活跃 + 定期心跳 |
| @crxjs/vite-plugin 兼容性问题（Chrome 新版本 API） | 低 | 中 | 锁定插件版本，关注 GitHub issue 动态 |
| AI API Key 在扩展环境泄露风险 | 中 | 高 | 仅存 `chrome.storage.session`（内存级，扩展重载后清除），不存 `local`/`sync` |

---

## 第 7 章：Chrome Web Store 发布检查清单

```
Phase 3 完成时逐项检查：

[ ] 扩展无 console.error（生产构建）
[ ] manifest.json 权限最小化（仅申请实际使用的 API）
[ ] 所有 UI 文本为简体中文
[ ] 所有截图无个人信息泄露
[ ] 隐私政策 URL 可访问（GitHub Pages 托管）
[ ] 1280x800 截图 × 5 张（录制 / AI 生成 / 编辑 / 导出 / 设置）
[ ] 宣传文案（中文，≤ 132 字符标题，≤ 1000 字符描述）
[ ] 推广图片（小型 440x280 + 大型 920x680 +  Marquee 1400x560）
[ ] 扩展包大小 ≤ 800KB
[ ] 版本号与 CHANGELOG.md 一致
[ ] 已注册 Chrome Web Store 开发者账号（$5 一次性费用）
[ ] 在 Edge Add-ons 同步提交（可选）
```

---

## 第 8 章：附录

### 8.1 项目结构（目标状态）

```
flowio/
├── shared/                        # 新增：双端共享代码
│   ├── types/
│   │   ├── step.ts                # 步骤数据 Schema
│   │   ├── recording.ts           # 录制数据 Schema
│   │   └── ai.ts                  # AI 模型配置 + Prompt 模板
│   └── prompts/
│       ├── system-prompt.md       # AI System Prompt（共享）
│       └── user-prompt-template.md
│
├── src-tauri/                     # 桌面端（已有，Phase 3 微调）
│   └── ...
│
├── extension/                     # 新增：Chrome 扩展项目
│   ├── src/
│   │   ├── background/            # Service Worker
│   │   │   ├── index.ts           # 入口 + 消息路由
│   │   │   ├── recorder-state.ts  # 录制状态机
│   │   │   ├── ai-client.ts       # AI API 调用 + SSE 解析
│   │   │   ├── export-engine.ts   # JS 版导出引擎
│   │   │   ├── storage.ts         # IndexedDB 封装
│   │   │   └── screenshot.ts      # 截图管理
│   │   ├── content/               # Content Script
│   │   │   ├── index.ts           # 入口 + 初始化
│   │   │   ├── event-listener.ts  # 事件监听器
│   │   │   ├── dom-analyzer.ts    # DOM 元素分析
│   │   │   └── screenshot-cropper.ts # Canvas 截图裁剪
│   │   ├── popup/                 # Popup UI
│   │   │   ├── main.tsx
│   │   │   └── App.tsx
│   │   ├── sidepanel/             # Side Panel UI
│   │   │   ├── main.tsx
│   │   │   └── App.tsx
│   │   └── components/            # 从桌面端复用 + 扩展专属组件
│   │       ├── StepEditor/        # 复用
│   │       ├── StepList/          # 复用
│   │       ├── StepCard/          # 复用
│   │       ├── ExportPanel/       # 复用 + 适配
│   │       ├── SettingsPanel/     # 复用 + 适配
│   │       ├── RecordingPanel/    # 复用 + 适配
│   │       └── OnboardingGuide/   # 新增：首次引导
│   ├── manifest.json
│   ├── vite.config.ts
│   ├── package.json
│   └── tsconfig.json
│
├── docs/
│   ├── spec.md                    # 已有
│   ├── plan.md                    # 已有
│   └── plugin-roadmap.md          # 本文档
└── AGENTS.md                      # 已有
```

### 8.2 关键 API 权限清单

```json
{
  "permissions": [
    "activeTab",        // 录制当前标签页
    "scripting",        // 注入 Content Script
    "storage",          // chrome.storage（设置 + 同步）
    "sidePanel",        // 侧边栏编辑面板
    "tabs"              // captureVisibleTab + 标签页信息
  ],
  "host_permissions": [
    "<all_urls>"        // Phase 1 最小化：仅 activeTab 可免此权限
                        // Phase 3 考虑改为具体域名以通过审核
  ]
}
```

### 8.3 参考资源

| 资源 | 链接 |
|------|------|
| Chrome Extension Manifest V3 文档 | https://developer.chrome.com/docs/extensions/mv3/ |
| @crxjs/vite-plugin | https://crxjs.dev/vite-plugin/ |
| Scribe Chrome 扩展 | https://chromewebstore.google.com/detail/scribe |
| Chrome Web Store 发布指南 | https://developer.chrome.com/docs/webstore/ |
| Chrome Extension 最佳实践 | https://developer.chrome.com/docs/extensions/mv3/intro/ |

---

*文档完毕。后续任务：将桌面端共享 Prompt 模板提取到 `shared/prompts/`，然后启动 Phase 1 开发。*
*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
