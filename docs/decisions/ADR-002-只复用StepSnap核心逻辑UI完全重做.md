---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_29377945882c11f1a68c525400826444
    ReservedCode1: 6o/br4jeO79Hz+mnNpNdlVDvBp67YLr7aSQBJ/I6tNSNymuNapJRlsu+T0oeZ7SbY0RQR9AhgjEej+amndIlQH1pVB/jIbm57qZqCJQ0S4dYgxAqIsX9t6QzJyeo9CyktMv/UZbf/EfxDx9sFH9Cxq91e1JVX6KzJUbHVKHTPVcsXzdvaiNg8/IWYUU=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_29377945882c11f1a68c525400826444
    ReservedCode2: 6o/br4jeO79Hz+mnNpNdlVDvBp67YLr7aSQBJ/I6tNSNymuNapJRlsu+T0oeZ7SbY0RQR9AhgjEej+amndIlQH1pVB/jIbm57qZqCJQ0S4dYgxAqIsX9t6QzJyeo9CyktMv/UZbf/EfxDx9sFH9Cxq91e1JVX6KzJUbHVKHTPVcsXzdvaiNg8/IWYUU=
---

# ADR-002：只复用 StepSnap 核心逻辑，UI 完全重做

> 状态：已采纳
> 日期：2026-07-25
> 决策者：录步 (Flowio) 项目

## 背景

StepSnap（https://github.com/pimzino/stepsnap）是基于 Tauri 2 的开源桌面录屏文档工具，技术实现与录步高度重合。项目启动时面临选择：

- **方案 A**：Fork StepSnap，在其基础上修改 UI 和功能
- **方案 B**：只复用核心技术逻辑，UI 从零构建

StepSnap 技术评估结论（详见 `docs/stepsnap-analysis.md`）：

| 维度 | 评价 |
|------|------|
| DOM 事件捕获 (rdev) | ⭐⭐⭐⭐⭐ 核心能力强，跨平台稳定 |
| 截图机制 (xcap) | ⭐⭐⭐⭐⭐ 全屏/区域截图 + after-frame |
| UI 元素识别 (UIA) | ⭐⭐⭐⭐ 控件名/类型/宿主应用 |
| 原生覆盖层窗口 | ⭐⭐⭐⭐ 独立透明窗口，无窗口装饰 |
| **UI/UX 设计** | ❌ 界面老旧，开源项目感强 |
| **中文支持** | ❌ 无中文界面，无中文步骤生成 |
| **AI 集成** | ❌ 仅基础 API 调用，无国产大模型 |

## 决策

**方案 B：只复用核心技术逻辑，UI 完全重做。**

复用范围（✅）：
- `recorder.rs` — 全局输入监听引擎（rdev）
- 截图机制 — xcap + after-frame 稳定截图
- `accessibility.rs` — Windows UI Automation 元素识别
- `overlay.rs` — 原生覆盖层窗口逻辑（仅逻辑，不参考配色/样式）
- Tauri State 管理 + emit 通信模式

不复用范围（❌）：
- 任何 React 组件（App.tsx 及所有 components/）
- CSS 样式（index.css）
- 页面布局/路由结构
- 图标/配色方案
- 导出功能 UI（ExportDropdown 等）
- 设置面板 UI（SettingsPanel 等）

## 理由

1. **消除抄袭嫌疑**：StepSnap 与 Scribe 在视觉上高度相似，若 Fork 后只改配色，会被市场定位为「Scribe 的汉化版」，损害品牌独立性
2. **国产化设计语言**：StepSnap 的西式交互逻辑不符合国内用户习惯，需要飞书/钉钉风格的国产化设计
3. **满足差异化战略**：录步的定位是「Scribe 国产替代 + 中文原生」，UI 是差异化最直观的体现
4. **质量把控**：StepSnap 的前端代码质量一般，存在大量技术债，从零编写反而更可控
5. **技术复用的高效性**：录制引擎是纯逻辑代码（~3000 行 Rust），与 UI 完全解耦，迁移成本低

## 后果

- **正面**：UI 完全自主可控，品牌独立性高，长期竞争力强；符合「国产化」市场定位
- **负面**：UI 开发工作量显著增加，MVP 阶段前端开发时间约占整体 40%
- **缓解**：使用 React + TypeScript + Tailwind CSS（AI 最熟悉的技术栈），组件化开发加速
- **风险**：录制引擎迁移过程中可能遇到 StepSnap 代码耦合问题——Week 1 预留 2 天 buffer 处理
*（内容由AI生成，仅供参考）*
