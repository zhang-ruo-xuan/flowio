# 录步 Flowio

> 操作即文档 —— 录一遍，自动生成操作指南。

[![Platform](https://img.shields.io/badge/platform-Windows-blue?logo=windows)](https://github.com/zhang-ruo-xuan/flowio)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

**Flowio** 是一款 Windows 桌面端的操作录制与教程自动生成工具。录制任意桌面软件的操作过程，AI 自动标注每一步的标题和描述，一键导出为 PDF / HTML / Markdown / 微信长图。

---

## 为什么用 Flowio

| 传统方式 | Flowio |
|---------|--------|
| 手动截图 → 粘贴到 Word → 逐张标注 | 录一遍 → AI 自动标注 → 导出 |
| 更新教程需要全部重做 | 修改单步即可，其他步骤不变 |
| 发给同事要传大文件 | 分享链接 / 微信长图 / 导出 PDF |
| 英文软件教程标注中文困难 | 原生中文 AI 引擎，中文术语识别精准 |

---

## 核心功能

- **全局录制** — 录制任何 Windows 桌面应用的操作过程，自动截图
- **AI 智能标注** — 接入智谱 GLM-4 / DeepSeek 等国产大模型，自动生成步骤标题和操作描述
- **可视化编辑器** — Before → After 双截图对比，步骤拖拽排序，内联编辑
- **隐私脱敏** — 自动识别并模糊身份证、手机号、银行卡、密码框等敏感信息
- **多格式导出** — PDF / HTML / Markdown / 微信长图
- **本地优先** — 完全离线可用，数据不上传云端

---

## 快速开始

### 下载安装

前往 [Releases](https://github.com/zhang-ruo-xuan/flowio/releases) 下载最新安装包。

- 支持 Windows 10+ (x64)
- 双击安装，一路下一步即可

### 三步上手

1. **录制** — 点击录制按钮，正常操作你的软件
2. **AI 生成** — 录制结束后，AI 自动为每一步生成标题和描述
3. **导出** — 导出为 PDF / HTML / 微信长图，分享给同事

---

## 技术栈

| 层 | 技术 |
|---|------|
| 桌面框架 | Tauri 2 |
| 前端 | React 19 + TypeScript + Tailwind v4 |
| 后端 | Rust |
| AI 引擎 | 智谱 GLM-4-flash / DeepSeek-V4 / 通义千问 VL |
| 录制引擎 | Windows UI Automation + Win32 Hook |
| 数据存储 | SQLite（本地） |

---

## 路线图

| 阶段 | 内容 | 状态 |
|------|------|:---:|
| 桌面 MVP | 录制 + AI 标注 + 导出 | ✅ 进行中 |
| 桌面 V1 | 编辑器增强 + 无水印导出 | 🔲 |
| 桌面 V2 | Free/Pro 双版 + 隐私脱敏 + 区域录制 | 🔲 |
| 桌面 V3 | Pro 上线 + 多语言 + 统计面板 | 🔲 |
| 浏览器插件 | Chrome/Edge 扩展 + 账号互通 | 🔲 |
| 飞书/企微 | 企业集成 + Team 版 | 🔲 |
| 生态开放 | 模板市场 + API + 开发者工具 | 🔲 |

[查看完整路线图](https://github.com/zhang-ruo-xuan/flowio/blob/main/docs/scribe-full-product-matrix.md)

---

## 参与贡献

Flowio 目前处于桌面 MVP 阶段，欢迎提 Issue 和反馈。

- 遇到 Bug？[提交 Issue](https://github.com/zhang-ruo-xuan/flowio/issues)
- 有功能建议？[发起讨论](https://github.com/zhang-ruo-xuan/flowio/discussions)

---

## 许可证

MIT License © 2026 Flowio
