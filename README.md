---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_c29a57ee952911f1b6b5525400287e28
    ReservedCode1: iKqJ8IVTp/bQV3sfDVITJEP1foNFeHRDp8uSihJ6BxS6vgcXt32mgn1YJBn9Lc3+U0aYXheWa74pbQgYu4g+VMFoihq8RF+e8M7zKvW30uFxt97WgDCM7yZ56/UbFBkbZnkydfKTsQOvpcn1wryghyP3BaJyZleFqelc8WNEi6+O+HF3oVzZZs9ppe0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_c29a57ee952911f1b6b5525400287e28
    ReservedCode2: iKqJ8IVTp/bQV3sfDVITJEP1foNFeHRDp8uSihJ6BxS6vgcXt32mgn1YJBn9Lc3+U0aYXheWa74pbQgYu4g+VMFoihq8RF+e8M7zKvW30uFxt97WgDCM7yZ56/UbFBkbZnkydfKTsQOvpcn1wryghyP3BaJyZleFqelc8WNEi6+O+HF3oVzZZs9ppe0=
---

# 录步 Flowio

> 录一遍操作，AI 自动生成带截图的操作指南。

[![Platform](https://img.shields.io/badge/platform-Windows_10+-blue?logo=windows)](https://github.com/zhang-ruo-xuan/flowio)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

**Flowio** 是一款 Windows 桌面操作录制工具。录制任意软件的操作过程，AI 自动标注每一步标题和描述，一键导出 PDF / HTML。

[产品页](https://zhang-ruo-xuan.github.io/flowio/) · [下载安装包](https://github.com/zhang-ruo-xuan/flowio/releases)

---

## 为什么用 Flowio

| 传统方式 | Flowio |
|---------|--------|
| 手动截图 → 粘贴到 Word → 逐张标注 | 录一遍 → AI 自动标注 → 导出 |
| 更新教程需要全部重做 | 修改单步即可，其他不变 |
| 英文软件教程标注中文困难 | 原生中文 AI 引擎，术语识别精准 |
| 数据存云端，担心泄露 | 完全离线，数据在本地 SQLite |

---

## 已实现功能

| 功能 | 说明 |
|------|------|
| **全局录制** | 录制任何 Windows 桌面应用的操作过程，自动截图 |
| **AI 智能标注** | 接入智谱 GLM-4 / DeepSeek，自动生成步骤标题和描述 |
| **可视化编辑器** | Before / After 双截图对比，内联编辑标题与描述 |
| **PDF 导出** | 一键导出 PDF 文档 |
| **HTML 导出** | 一键导出 HTML 离线查看器 |
| **完全离线** | 本地存储，不依赖云端 |

---

## 正在开发

| 功能 | 状态 |
|------|:---:|
| Markdown 导出 | 🟡 |
| 微信长图导出 | 🟡 |
| 隐私脱敏（身份证/手机号/银行卡） | 🟡 |
| 编辑器拖拽排序 | 🟡 |

---

## 快速开始

前往 [Releases](https://github.com/zhang-ruo-xuan/flowio/releases) 下载最新安装包。

- 支持 Windows 10+ (x64)
- 双击安装即可

### 三步上手

1. **录制** — 点击录制按钮，正常操作你的软件
2. **AI 生成** — 录制结束，AI 自动标注每一步
3. **导出** — 导出为 PDF / HTML，分享给同事

---

## 技术栈

| 层 | 技术 |
|---|------|
| 桌面框架 | Tauri 2 |
| 前端 | React 19 + TypeScript + Tailwind v4 |
| 后端 | Rust |
| AI 引擎 | 智谱 GLM-4-flash / DeepSeek-V4 |
| 录制引擎 | Windows UI Automation + Win32 Hook |
| 数据存储 | SQLite（本地） |

---

## 反馈与贡献

MVP 阶段，欢迎试用和反馈。

- Bug / 建议 → [提交 Issue](https://github.com/zhang-ruo-xuan/flowio/issues)

---

## 许可证

MIT License © 2026 Flowio
*（内容由AI生成，仅供参考）*
