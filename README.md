# 录步 Flowio · 桌面端

> 录一遍操作，AI 自动生成带截图的操作指南。

[![Platform](https://img.shields.io/badge/platform-Windows_10+-blue?logo=windows)](https://gitee.com/zhangruoxuan/flowio)
[![Release](https://img.shields.io/badge/release-v0.2.0-orange)](https://gitee.com/zhangruoxuan/flowio/releases)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

**录步 Flowio** 是一款 Windows 桌面操作录制工具。录制任意软件的操作过程，边录边说话自动转成文字讲解，AI 自动标注每一步标题和描述，一键导出多格式文档或发布为在线教程。

[产品页](https://flowio.cn) · [下载安装](https://flowio.cn/download) · [浏览器扩展](https://gitee.com/zhangruoxuan/flowio-extension)

---

## 为什么用 Flowio

| 传统方式 | Flowio |
|---------|--------|
| 手动截图 → 粘贴到 Word → 逐张标注 | 录一遍 → AI 自动标注 → 导出 |
| 每一步都要打字写说明 | 边操作边说，语音自动转文字 |
| 英文软件教程标注中文困难 | 原生中文视觉 AI，界面术语识别精准 |
| 截图里的手机号、密码不敢直接发 | 自动扫描脱敏，模糊或打码 |
| 更新教程需要全部重做 | 修改单步即可，其他不变 |
| 数据存云端，担心泄露 | 完全本地，数据在本地 SQLite |

---

## 功能

| 功能 | 说明 |
|------|------|
| **全屏 / 指定软件录制** | 录制整个屏幕或只录某个软件窗口，点击与输入自动截图 |
| **语音转文字** | 录制时对着麦克风讲解，按步骤自动转写并 AI 纠错（支持自定义热词） |
| **AI 智能标注** | 智谱 GLM 视觉模型理解每步截图，自动生成标题和描述 |
| **可视化编辑器** | 全图 / 裁图双视图、区域裁剪、批注、拖拽排序、AI 精简冗余步骤 |
| **隐私脱敏** | 扫描截图中身份证号、手机号、密钥等敏感信息，一键模糊或实心打码 |
| **AI 翻译** | 教程一键翻译为多语言，面向不同受众分发 |
| **多格式导出** | PDF / HTML / Markdown / Word，排版完整可直接分享 |
| **一键发布** | 上传到 [flowio.cn](https://flowio.cn) 生成在线教程链接 |

---

## 快速开始

前往 [Releases](https://gitee.com/zhangruoxuan/flowio/releases) 或[官网下载页](https://flowio.cn/download)获取安装包。

- 支持 Windows 10+ (x64)
- 双击安装即可

### 三步上手

1. **录制** — 点击录制按钮，正常操作你的软件，需要讲解时直接说话
2. **AI 生成** — 录制结束，AI 自动标注每一步，语音已转为文字
3. **导出 / 发布** — 导出 PDF / Word / Markdown，或一键发布到 flowio.cn

---

## 本地开发

```bash
# 前置要求：Node 20+、Rust 1.77+（MSVC 工具链）
pnpm install

npm run tauri dev      # 开发模式运行
npm run tauri build    # 构建安装包
npm run check:geo      # 裁图几何一致性检查
```

---

## 技术栈

| 层 | 技术 |
|---|------|
| 桌面框架 | Tauri 2 |
| 前端 | React 19 + TypeScript + Tailwind CSS |
| 后端 | Rust |
| 截屏引擎 | xcap |
| 语音采集 | cpal（WASAPI） |
| AI 引擎 | 智谱 GLM 系列（视觉理解 + ASR 语音识别），支持用户自定义 API Key 与模型 |
| 数据存储 | SQLite（本地） |

---

## 相关仓库

- [flowio-extension](https://gitee.com/zhangruoxuan/flowio-extension) — 浏览器扩展版（Chrome / Edge），录制网页操作
- [flowio-site](https://gitee.com/zhangruoxuan/flowio-site) — 教程社区网站（Next.js 14，私有仓库）

---

## 反馈与贡献

MVP 阶段，欢迎试用和反馈。

- Bug / 建议 → [提交 Issue](https://gitee.com/zhangruoxuan/flowio/issues)

---

## 许可证

[MIT License](LICENSE) © 2026 Flowio
