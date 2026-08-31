# FlowIO 桌面版录步 — 回归验收清单（B1–B11 修复验证）

> 配套：`docs/code-review-findings.md`（缺陷详情）、`docs/test-cases-desktop.md`（全量用例）
> 版本基线：修复改动 **12 文件 +427/-121**（未 commit，工作区状态）
>
> 本文档用于：研发修复后 → 测试回归验收。题头字段说明缺陷修复方式（代码已核实 ✓），后面是每条的手工/自动化验证步骤与判定标准。
> 验收结论填写示例：`PASS / FAIL + 备注`

---

## 状态总览

| Bug | 缺陷 | 修复状态（代码核实） | 回归用例 | 验收结论 |
|-----|------|--------------------|---------|---------|
| B1 | AI 外发未脱敏截图（隐私泄漏） | ✅ 已修（pipeline Stage 1 OCR 脱敏+失败丢弃） | TC-RG-B1 | ☐ |
| B2 | 密码字段不检测 | ✅ 已修（UIA IsPassword + is_sensitive + [密码已隐藏]） | TC-RG-B2 | ☐ |
| B3 | PDF 中文豆腐块 / Edge 缺失 | ✅ 已修（CJK 字体栈 + Edge 探测 + 60s 超时） | TC-RG-B3 | ☐ |
| B4 | 自身窗口过滤键盘分支漏 | ✅ 已修（键盘分支 + Flowio 窗口自过滤） | TC-RG-B4 | ☐ |
| B5 | 200 步上限未实现 | ✅ 已修（MAX_RECORDING_STEPS=200，三处入录点拦截） | TC-RG-B5 | ☐ |
| B6 | AI 文本清洗覆盖不全/遮蔽符不一致 | ✅ 已修（window_title 清洗 + 统一 [已隐藏]） | TC-RG-B6 | ☐ |
| B7 | 导出文件名保留名 | ✅ 已修（CON/PRN/AUX/NUL/COM/LPT + `_` 后缀） | TC-RG-B7 | ☐ |
| B8 | 银行卡正则过宽 | ✅ 已修（13-19 → 16-19） | TC-RG-B8 | ☐ |
| B9 | 暂停计时累加 | ✅ 已修（App.tsx 依赖 [recording, isPaused]） | TC-RG-B9 | ☐ |
| B10 | 手动扫描脱敏覆盖原图 | ✅ 已修（覆盖前备份到 temp） | TC-RG-B10 | ☐ |
| B11 | 全局热键抢占另存为 | ✅ 已修（D/G/Q 组合） | TC-RG-B11 | ☐ |

**自动化测试**：`cargo test --lib`（应含新增密码敏感/保留名/B5 边界等用例）+ 前端 `tsc -b`。

---

## 各项回归细则

### TC-RG-B1 — AI 外发前截图 OCR 脱敏（P0）
- **代码依据**：`src-tauri/src/ai/pipeline.rs` Stage 1 对每步 before/marked 截图调 `redact_base64_screenshot`（Blur 模式，`RedactionConfig::default_all()`）；OCR 引擎失败时 `Err` → **丢弃该截图而非裸传**。Vision 检查前置到 Stage 0（先于脱敏）。
- **回归步骤**：
  1. 打开一个含明文手机号/银行卡的网页（如打印预览，让号码进入截图像素）
  2. 录制「点击 + 输入」几步 → 编辑页 → AI 生成
  3. 抓包或日志确认**发给模型的 image_url 中的图片已被模糊**（敏感区域不可读）
  4. 断开/禁用 OCR 引擎（或注入假失败）→ 再次 AI 生成 → 确认该步截图被丢弃（`[ai] 截图脱敏失败，丢弃该图` 日志），**不崩溃、正常出标题描述**
- **判定**：步骤 3 模糊有效；步骤 4 无截图裸传记录。
- **残留风险**：OCR 本身漏识别的行仍可能裸奔（依赖 Windows.Media.Ocr 准确率）；无中英文语言包环境 OCR 不可用 → 截图全部丢弃，属预期降级。

### TC-RG-B2 — 密码字段敏感检测（P0）
- **代码依据**：`recorder/screenshot.rs` `get_focused_element_text()` 返回 `(text, is_password)`，`CurrentIsPassword()` 判定；`recorder/commands.rs` `flush_key_step` 敏感时描述= `[密码已隐藏]`、`StepData.is_sensitive=true`。
- **回归步骤**：
  1. 录制中打开记事本 → 再打开一个真实密码框（如浏览器登录页 type=password）
  2. 在密码框输入「Secret123」
  3. 停止录制 → 编辑页检查该 type 步骤：**描述不含 Secret123，显示 [密码已隐藏]**；接口/DB 中 `is_sensitive=true`
  4. AI 生成后再次确认标题/描述无密码明文
- **判定**：明文零出现；`is_sensitive` 正确标记。
- **残留风险**：仅 UIA `IsPassword` 被正确设置的控件生效；自定义控件/部分 Electron 应用可能不设置该属性 → 不识别。回归需选「原生 Windows 密码框」验证（记事本密码输入不是原生密码框，注意选型）。

### TC-RG-B3 — PDF 中文 + Edge 可用性（P0）
- **代码依据**：`export/commands.rs` CSS `font-family` 加入 `Microsoft YaHei / PingFang SC / Noto Sans CJK SC / SimSun`；`resolve_edge_path()` 探测 ProgramFiles/(x86)/LOCALAPPDATA 三环境变量 + 两个固定路径；`EDGE_PDF_TIMEOUT=60s`，超时 kill 进程。
- **回归步骤**：
  1. 中文标题 + 中文描述录制导出 PDF → 打开检查**中文正常显示**（无 □ 豆腐块）
  2. 非中文/精简系统（或临时改 PATH 模拟）：确认 Edge 路径回退逻辑生效或不存在的场景下返回明确错误「未找到 Microsoft Edge」
  3. 超时场景（可选，难造）：确认 60s 后导出返回超时错误而非挂起
- **判定**：中文渲染正确；Edge 缺失时给出友好错误。

### TC-RG-B4 — 键盘分支自身窗口过滤（P1）
- **代码依据**：`recorder/commands.rs` 键盘 `KeyEvent::Press` 分支加入 `is_flowio_app` 自过滤（与点击分支一致）。
- **回归步骤**：
  1. 录制中 → 回到 Flowio 窗口，在标题/搜索框打字
  2. 停止录制 → 检查**无源于 Flowio 自身的 type 步骤**
  3. 再在其他应用（记事本）打字 → 正常生成 type 步骤（对比确认过滤未误伤）
- **判定**：Flowio 内输入被过滤；外部输入正常记录。

### TC-RG-B5 — 200 步上限（P1）
- **代码依据**：`recorder/state.rs` `MAX_RECORDING_STEPS=200`，`push_step` 超限返回 Err；`recorder/commands.rs` 点击/键盘两处入录点均 `>= 200` 时丢弃并打日志。
- **回归步骤**：
  1. 录制并制造 >200 次点击/输入（脚本或快速操作）
  2. 停止录制 → 编辑页确认**步骤总数 ≤ 200**
  3. 查看日志确认「步骤数已达上限…丢弃」出现
- **判定**：上限生效、无内存失控。
- ⚠️ **需注意（文档与实现差异）**：spec 原文是「**提示并自动停止**」，当前实现是「静默丢弃新步骤 + eprintln 日志」，**前端无用户可见提示、也不自动停止**。属部分满足——建议确认产品是否接受，不接受则提单补「超出时 Toast + 自动停止」。

### TC-RG-B6 — window_title 清洗 + 遮蔽符统一（P1）
- **代码依据**：`ai/pipeline.rs` 增加 `s.window_title = sanitize_for_ai(...)`；`ai/sanitize.rs` 全部 `***` → `[已隐藏]`。
- **回归步骤**：
  1. 录制窗口标题含手机号（如「订单-13800138000-详情」标题的窗口）
  2. AI 生成 → 检查发给模型的 prompt 中 window_title **不含 13800138000**
  3. 编辑页/导出文本确认遮蔽符统一为 `[已隐藏]`
- **判定**：window_title 清洗生效；遮蔽符一致。

### TC-RG-B7 — 导出文件名保留名（P2）
- **代码依据**：`export/commands.rs` `sanitize_filename` 对 `con/prn/aux/nul/com1-9/lpt1-9`（含带扩展名形式）追加 `_`。
- **回归步骤**：录制标题改为 `CON`、`prn.txt`、`正常报告` → 分别导出 PDF → 确认文件名 `CON_.pdf` / `prn.txt_.pdf` / `正常报告.pdf` 且**写入成功**。
- **判定**：保留名成功写入，普通名不受影响。

### TC-RG-B8 — 银行卡正则 16-19（P2）
- **代码依据**：`redaction/mod.rs` 银行卡正则 `[0-9]{13,19}` → `[0-9]{16,19}`。
- **回归步骤**：
  1. 文本含 13 位纯数字（如订单号 1234567890123）→ 文本脱敏 → **不被识别归银行卡**（保持原样）
  2. 文本含 16/19 位数字（构造测试卡号）→ 脱敏 → 命中替换为 [已隐藏]
- **判定**：13-15 位不误伤；16-19 位正常命中。

### TC-RG-B9 — 暂停计时冻结（P2）
- **代码依据**：`src/App.tsx` 计时 `useEffect` 依赖 `[recording, isPaused]`，暂停时清除 interval。
- **回归步骤**：
  1. 开始录制 → 等 10s → 暂停（看标题栏 PAUSED 计时**停止在 0:10**）
  2. 等 5s → 恢复 → 计时从 0:10 继续（**不含暂停的 5s**）
- **判定**：暂停期间计时不累加。

### TC-RG-B10 — 手动扫描脱敏备份（P2）
- **代码依据**：`redaction/mod.rs` `redact_screenshot_auto` 覆盖原图前先备份到 `<temp>/flowio_redacted/_backup/<ts>_<file>`。
- **回归步骤**：含敏感截图的录制 → 编辑页「扫描脱敏」→ 确认 `%TEMP%/flowio_redacted/_backup/` 出现备份原图 → 原图已被覆盖脱敏。
- **判定**：存在备份，可恢复。

### TC-RG-B11 — 全局热键 D/G/Q（P1）
- **代码依据**：`hotkey/mod.rs` + `settings/commands.rs` 默认 `Ctrl+Shift+D/G/Q`；旧键 R/S/P/E 全部忽略。
- **回归步骤**：
  1. 应用后台运行时：Ctrl+Shift+D 开始录制 / Ctrl+Shift+G 暂停恢复 / Ctrl+Shift+Q 停止，三键逐一验证
  2. 确认旧键 Ctrl+Shift+S/P/R/E **均不触发** Flowio
  3. 在浏览器/编辑器（占用 Ctrl+Shift+G 等的应用）前台验证 Flowio **不拦截**：这些应用本身的快捷键仍可用
  4. 设置页快捷键文案与实际一致（`src/pages/Settings.tsx` 已改 D/G/Q）
- **判定**：新键全部生效、旧键无残留、不抢占目标应用。

---

## 回归环境与数据要求
- **环境**：Windows 11 中文（主验证）+ 一台英文或无中文语言包的 Windows（PDF 字体验证）
- **数据**：构造含手机号/银行卡/邮箱的文本与网页截图；13 位订单号样本；`CON` 等保留名样本
- **前置**：录制前在设置页配置有效 API Key（AI 链路用例需要）

## 验收结论标准
- **全部 PASS** → 可发起 commit / 发布前最终冒烟
- **任一 FAIL** → 打回对应研发，附复现（本文 TC 编号）
- ⚠️ **决策点**：
  1. B5 「提示并自动停止」是否属于验收必须？（当前为静默丢弃）
  2. B1 隐私优先降级（无 OCR 语言包时丢弃截图）是否符合产品预期？