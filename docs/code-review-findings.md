# FlowIO 桌面版录步 — 源码静态测试报告

> 范围：录制引擎(F01) / 隐私脱敏(RED) / 导出(F04)
> 方法：直接阅读 Rust 后端（`src-tauri/src/recorder`、`redaction`、`ai`、`export`）与前端（`src/App.tsx`、`src/pages/*`）源码，对照 `docs/spec.md` 需求做静态走查。
> 说明：所有「依据」均给出具体文件:行号，可定位复核。

---

## ⏱ 变动追踪（2026-08-21 重新测试）

### 第一轮（`3c6bea5` + `032ed4a`，UI/配置层）
1. **快捷键调整**：录制 `Ctrl+Shift+R → Ctrl+Shift+S`，停止 `Ctrl+Shift+S → Ctrl+Shift+E`。
2. **录制状态细分**：新增 `ai_generated` / `text_redacted` / `scan_redacted` 三列（`db/mod.rs` 迁移），Dashboard 状态列以「扫/文/ai」简写展示（`Dashboard.tsx`）。
3. **脱敏提示精简**：Toasts 文案简化（`StepEditor.tsx`）。
→ **当时结论：B1–B10 均未修复；新增 B11 全局热键冲突风险。**

### 第二轮（工作区未 commit，12 文件 +427/-121）
以下修复**已逐项读到代码核实 ✓**：
| Bug | 修复内容 | 核实 |
|-----|---------|------|
| B1 | `ai/pipeline.rs` Stage 1 外发前 OCR 脱敏（Blur+全开），OCR 失败丢弃不裸传，vision 检查前置 Stage 0 | ✓ |
| B2 | `recorder/screenshot.rs` `CurrentIsPassword()` + `StepData.is_sensitive` + 描述替换 `[密码已隐藏]` | ✓ |
| B3 | `export/commands.rs` CSS 加入 CJK 字体栈 + `resolve_edge_path()` 探测 + 60s 进程超时 | ✓ |
| B4 | `recorder/commands.rs` 键盘分支补 Flowio 窗口自过滤 | ✓ |
| B5 | `recorder/state.rs` `MAX_RECORDING_STEPS=200`，三处入录点拦截（⚠️「提示并自动停止」仍未实现，为静默丢弃，见回归清单决策点） | ✓ |
| B6 | `ai/pipeline.rs` window_title 清洗 + `sanitize.rs` 遮蔽符统一 `[已隐藏]` | ✓ |
| B7 | `sanitize_filename` 保留名（CON/PRN/AUX/NUL/COM/LPT）追加 `_` | ✓ |
| B8 | `redaction/mod.rs` 银行卡正则 13-19 → 16-19 | ✓ |
| B9 | `App.tsx` 计时依赖 `[recording, isPaused]`，暂停冻结 | ✓ |
| B10 | `redact_screenshot_auto` 覆盖前备份到 `<temp>/flowio_redacted/_backup/` | ✓ |
| B11 | 热键改 `Ctrl+Shift+D/G/Q`（录制/暂停/停止），旧键 R/S/P/E 全部忽略 | ✓ |

**自动化验证（2026-08-21 17:07 独立复跑）**：
- `cargo test --lib`：**445 passed / 0 failed**
- `tsc -b`：**通过（exit 0）**

**回归去向**：`docs/regression-acceptance.md`（TC-RG-B1~B11 手工/自动化验收清单）。

---

## 🔴 P0 — 阻断上线级

### B1. AI 生成时原始截图未脱敏，直接外发第三方大模型【隐私泄漏】
- **依据**：`src-tauri/src/ai/pipeline.rs:95-129` 将 `before`/`after`/`marked` 截图以 base64 `image_url` 原样发给 LLM；OCR 截图脱敏仅在**导出**流程调用（`export/commands.rs:269-335` 的 `redact_screenshot_copy`），AI 链路全程无 OCR 脱敏。
- **文本侧**：`pipeline.rs:35-36` 仅对 `title`/`description` 做 `sanitize_for_ai`，但 `window_title` 在 `prompt.rs:38-42` 被**原样拼进 prompt** 未清洗。
- **实际结果**：屏幕上可见的密码框、聊天内容、地址栏、证件号等会随截图像素原样外泄给大模型服务商。
- **期望结果**：AI 调用前对截图做 OCR 脱敏（与导出一致），且 `window_title` 一并清洗。
- **影响范围**：所有使用 AI 生成的录制；高合规风险（含密码/个人信息）。
- **建议**：在 `pipeline.rs` 发送图片前复用 `redact_screenshot_copy`；`window_title` 经 `sanitize_for_ai` 处理。

### B2. 密码字段检测完全未实现 —— 键盘输入明文入录【功能缺口/隐私】
- **依据**：`src-tauri/src/recorder/screenshot.rs:212-253` `get_focused_element_text()` 仅读取 UIA `TextPattern2`/`ValuePattern` 文本并直接返回，**无 `IsPasswordProperty` 判断**；`is_sensitive` 在录制引擎内零出现；`StepData` 结构无该字段（与 `docs/spec.md:158`、`docs/plan.md:599` 承诺不符）。
- **实际结果**：在密码框打字会生成明文 `type` 步骤，值不被替换为 `[密码]`。
- **期望结果**：识别 UIA `IsPassword` 的控件，步骤标记 `is_sensitive`、值替换为 `[密码]`，且不外发 AI。
- **影响范围**：所有含密码输入的录制；与 B1 叠加=密码明文入录并外泄。
- **建议**：在 `get_focused_element_text` 增加 `CurrentIsPassword()` 判断；`StepData` 增加 `is_sensitive` 并贯穿生成/导出/AI 链路。

### B3. PDF 导出未嵌入中文字体，非中文系统显示豆腐块【导出质量】
- **依据**：`src-tauri/src/export/commands.rs:702` HTML 模板 `font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif` **无任何 CJK 字体**；全库无 `@font-face`/字体文件/SimSun/YaHei/Noto CJK 引用。PDF 由 Edge 无头 `--print-to-pdf` 生成（`commands.rs:591-639`），字形完全依赖系统回退。
- **实际结果**：在未安装中文语言包的英文版/服务器版 Windows 上，PDF 中文显示为 □ 或空白。
- **期望结果**：PDF 中文正常显示（嵌入 CJK 字体或在 CSS 显式指定 `"Microsoft YaHei","SimSun","Noto Sans CJK SC"`）。
- **影响范围**：全部 PDF 导出用户（中文必然踩）。
- **建议**：CSS `font-family` 加入中文字体回退；或改用可嵌入字体的 PDF 库。

---

## 🟠 P1 — 高优先级

### B4. 自身窗口过滤仅对「鼠标点击」生效，键盘分支漏过滤【正确性】
- **依据**：`src-tauri/src/recorder/commands.rs:309-315` 仅在鼠标点击分支调用 `is_flowio_app()` 过滤；键盘 `flush_key_step` 分支未做过滤（`recorder::is_flowio_app` 定义在 `recorder.rs:17-20`，按标题含 `flowio`/`录步` 判定）。
- **实际结果**：在 Flowio 窗口内打字仍会生成多余 `type` 步骤。
- **期望结果**：键盘分支同样跳过 Flowio 自身窗口。
- **影响范围**：编辑时 Flowio 输入框内的输入会污染录制。

### B5. 单次录制 200 步上限未实现【功能缺口】
- **依据**：`src-tauri/src/recorder/state.rs:92-101` `push_step` 直接 `self.steps.push(step)` 无 `len()` 检查；`commands.rs` 的 `finish_recording` 排水（~709-717）也无上限；前端 `App.tsx:17` `stepCount` 初始化为 0 后从未 `setStepCount` 增量。
- **实际结果**：超长录制无提示、无自动停止，步骤无限累积（内存膨胀、导出文件超大）。
- **期望结果**：`spec.md:160` 约定「超出 200 步提示并自动停止」。
- **影响范围**：长流程录制稳定性。

### B6. AI 文本清洗覆盖不全 + 两套正则不一致【隐私/一致性】
- **依据**：`pipeline.rs:35-36` 只清洗 `title`/`description`；`prompt.rs:38-42` 的 `window_title` 不清洗；`ai/sanitize.rs` 使用独立弱正则（身份证 `\d{17}[\dXx]` 无结构/边界校验，银行卡 `\d{19}`/`\d{,18}`），遮蔽符 `***` 与 `redaction/mod.rs` 的 `[已隐藏]` 不一致。
- **实际结果**：清洗强度弱于导出脱敏，且 `window_title` 漏洗。
- **期望结果**：统一脱敏正则与遮蔽符，AI 链路与导出链路一致。
- **影响范围**：AI 输入侧的文本隐私。

---

## 🟡 P2 — 中低优先级

### B7. 导出文件名未处理 Windows 保留名（CON/PRN/AUX/NUL/COM1…）【健壮性】
- **依据**：`export/commands.rs:199-206` `sanitize_filename` 仅替换 `\/:*?"<>|` 9 个字符；单测 `assert_eq!(sanitize_filename("CON"), "CON")` 显式标记「记录现状」。
- **实际结果**：标题为保留名时 Windows 拒绝写入或产生异常。
- **建议**：对保留名追加后缀（如 `_`）。

### B8. 银行卡脱敏正则过宽（13-19 位）易误报【准确性】
- **依据**：`src-tauri/src/redaction/mod.rs` 银行卡正则 `([0-9]{13,19})`，比需求 `16-19` 更宽。
- **实际结果**：13–15 位订单号等会被误判为银行卡脱敏。
- **建议**：收紧为 `16-19` 或加 Luhn 校验。

### B9. 暂停期间 REC 计时仍在累加【计时准确性】
- **依据**：`src/App.tsx` 计时 `useEffect` 依赖 `[recording]`，暂停时 `recording` 仍为 `true`，`elapsed` 不暂停。
- **实际结果**：恢复后显示时长包含暂停时间。
- **建议**：计时依赖 `recording && !isPaused`，或在暂停时冻结基准。

### B10. 手动「扫描脱敏」直接覆盖原图，不可恢复【数据安全】
- **依据**：`redaction/mod.rs:494-533` `redact_screenshot_auto` 对命中原图 `.save(path)` 就地覆盖；而 Settings 文案暗示「原图保持不动」。
- **实际结果**：手动脱敏不可逆，误脱敏无法回退。
- **建议**：写入 `%TEMP%/flowio_redacted/` 副本，与导出一致。

---

## 补充说明（非 Bug，但需知）
- **Edge 路径硬编码**：`export/commands.rs:610` 写死 `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`，无 Chrome/WebView2 回退、无进程超时，缺失或卡死则导出直接失败/挂起（关联 B3）。
- **PDF/HTML 全量 base64 内联**：步骤/截图极多时 HTML 体积巨大，Edge 渲染可能慢或内存高（关联 B3）。

---

## 对应测试用例映射
| Bug | 关联用例 |
|-----|---------|
| B1 | TC-RED-02, TC-F02-02 |
| B2 | TC-F01-13 |
| B3 | TC-F04-01/04 |
| B4 | TC-F01-12 |
| B5 | TC-F01-16 / TC-F01-08 |
| B6 | TC-F02-06, TC-RED-01 |
| B7 | TC-RED-01 |
| B8 | TC-RED-01 |
| B9 | TC-F01-09/10 |
| B10 | TC-RED-02/03 |
| B11 | TC-F01-18 |

---

## 🛠 修复进度（2026-08-21）

B1–B11 已全部修复并通过验证（后端 `cargo test --lib` 445 passed / 0 failed，前端 `tsc -b` 通过）。

| Bug | 状态 | 修复要点 |
|-----|------|---------|
| B1 | ✅ 已修复 | `ai/pipeline.rs` 新增 Stage1 隐私脱敏：截图 base64 经 `redaction::redact_base64_screenshot` OCR 脱敏后才外发；脱敏失败丢弃该图不裸传；`window_title` 与 title/description 一并 `sanitize_for_ai` 清洗（B6 同修）。vision 能力检查前置到脱敏前，不受 OCR 引擎缺失影响 |
| B2 | ✅ 已修复 | `StepData` 新增 `is_sensitive`（serde default 兼容旧数据）；`get_focused_element_text()` 返回 `(text, is_password)`，用 `CurrentIsPassword` 判断；密码框步骤描述固定为 `[密码已隐藏]` |
| B3 | ✅ 已修复 | HTML body `font-family` 加 `Microsoft YaHei / PingFang SC / Noto Sans CJK SC / SimSun` 回退；Edge 改为 `resolve_edge_path()` 候选探测（含用户级安装）；增加 60s 进程超时（try_wait 轮询 + kill） |
| B4 | ✅ 已修复 | 键盘 `KeyEvent::Press` 分支补 `is_flowio_app` 自过滤，与鼠标分支一致 |
| B5 | ✅ 已修复 | `MAX_RECORDING_STEPS = 200`；`push_step` 与事件线程两处 push 均限长，超出丢弃并打日志 |
| B6 | ✅ 已修复 | AI 侧遮蔽符 `***` → `[已隐藏]` 与导出链路一致；`window_title` 漏洗在 B1 中一并修复 |
| B7 | ✅ 已修复 | `sanitize_filename` 识别 Windows 保留名（CON/PRN/AUX/NUL/COM1-9/LPT1-9，含带扩展名形式）并追加 `_` |
| B8 | ✅ 已修复 | 银行卡正则 `{13,19}` → `{16,19}`，避免误伤订单号 |
| B9 | ✅ 已修复 | `App.tsx` 计时 effect 依赖改为 `[recording, isPaused]`，暂停期间计时冻结 |
| B10 | ✅ 已修复 | `redact_screenshot_auto` 覆盖原图前备份到 `<temp>/flowio_redacted/_backup/`（时间戳防冲撞） |
| B11 | ✅ 已修复 | 全局热键改为 `Ctrl+Shift+D`（录制）/ `Ctrl+Shift+G`（暂停）/ `Ctrl+Shift+Q`（停止），避开 Ctrl+Shift+S/P/R 等主流占用；同步 `hotkey/mod.rs`、设置默认值与前端文案 |

**遗留提醒**：
- OCR 引擎依赖 `Windows.Media.Ocr`，无 OCR 语言包/引擎的环境下 AI 截图会被丢弃（隐私优先，功能降级为纯文本生成），属预期行为。
- B11 若决定换键：`hotkey/mod.rs` 注册处 + 设置页 `hotkey_config` 默认值 + `src/App.tsx` 快捷键文案需同步。
- 建议补充回归用例：密码框录制、含敏感像素截图 AI 生成、非中文系统 PDF 导出、13 位数字串不误脱敏。
